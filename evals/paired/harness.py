"""Tasks 526 and 259, note 549: real tasks from the board's history run again
headless, one cell at a time, and priced off their transcripts.

  cmax   the live practice: past T, a hook asks for the handoff in the words
         of ctx's /handoff and the session ends there; a fresh one is told
         'continuando', as the user types it. Effort max: the shared control.
  chigh  the same at effort high (526).
  bmax   Claude Code's own compaction at T (--autocompact), no handoff;
         effort max (259).

A run gets a repo holding only the history up to its task's parent commit (no
remote, and no later commit to find), the board as it stood when the user
asked for the task, and scratch state for ekko's registry and for ctx. It logs
in with ACCOUNT's profile, through the same `claude` wrapper and plugins as a
session of the user's, so today's ekko and ctx serve every cell alike.
Its transcripts, main thread and subagents, are copied into
target/evals/paired/ as soon as each session ends: they outlive the profile.

    nix develop -c python3 evals/paired/harness.py probe
    nix develop -c python3 evals/paired/harness.py run TASK CELL [--rep N]
    nix develop -c python3 evals/paired/harness.py pilot [--dry-run] [--anytime] [--long]

`trigger` is the PostToolUse hook the C cells run under; it reads the hook's
input on stdin.
"""

import argparse
import datetime
import glob
import json
import os
import random
import re
import shutil
import signal
import subprocess
import sys
import time
import uuid

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.normpath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, os.path.join(HERE, "..", "claude-code"))
from transcripts import W_CR, W_CW1H, W_CW5M, W_IN, W_OUT  # noqa: E402

HOME = os.path.expanduser("~")
# The account that pays, `--account`: the pilot's short runs went on trabalho,
# 259's long runs on default (decisions 741 and 744). PROFILE, PROFILE_ENV and
# POINT follow it (use_account).
ACCOUNT = "default"
# The run repos: big once built, outside ~/Projetos, and a folder name the
# transcript studies skip (evals/claude-code/transcripts.py, SKIP).
ROOT = os.path.join(HOME, ".cache", "ekko-paired")
OUT = os.path.join(REPO, "target", "evals", "paired")
BOARD = os.path.join(REPO, ".ekko")
HANDOFF_SKILL = os.path.join(HOME, "Projetos", "ctx", "src", "skills", "handoff", "SKILL.md")
CTX_WORKER_HOME = os.path.join(HOME, ".local", "state", "ctx", "worker-home")
LOCAL = datetime.timezone(datetime.timedelta(hours=-3))

T = 160_000
# --autocompact W compacts at W less 20k kept for the summary's output and a
# 13k buffer: the probe of 2026-09-24 set W to 100k and saw compactions start
# from 67k (and from 78-79k when one file read crossed the line). So bmax's W
# for compaction at T:
WINDOW = T + 33_000
# Units a point of each account's 5-hour window. Default: 3.95M units took it
# from 1% to 64% on 2026-09-26 (00:57-01:43), not note 444's 175k; trabalho:
# 1.85M units took it from 67% to 91% on 2026-09-24.
POINTS = {"default": 63_000, "trabalho": 77_000}
# The headless stream reports the account's windows (rate_limit_event): a run
# starts only if the 5-hour window can take it whole (its expected points on top
# stay under FIVE_HOUR), and the pilot stops at SEVEN_DAY of the week, so that
# the user's own days keep their quota; `pilot --week` moves it, at the user's
# word. A run in progress is stopped at week_stop(), short of the limit itself.
FIVE_HOUR, SEVEN_DAY = 0.95, 0.70
SEGMENTS = 8  # sessions a run may take before it is stopped
AFTER_HANDOFF = 180  # seconds a session may go on once its handoff is written
STAGE_CAP = 25_000_000  # stage 1's units, every run together
NIGHT = (datetime.time(23, 0), datetime.time(7, 30))  # when a run may start

# The prompt the user typed, the commit that answered it, when the prompt was
# typed (ms: the board is cut there), and the run's size. Read off the
# transcripts on 2026-09-24.
TASKS = {
    "396": ("faz a 125 junto com a 396", "bccf056", 1790280143642, "long"),
    "394": ("faz a 394", "2779afb", 1790206340347, "short"),
    "397": ("faz a 397", "e01e6d1", 1790205358820, "short"),
}
CAP = {"long": 9_000_000, "short": 3_000_000}  # units: a run is stopped past this
EXPECTED = {"long": 3_800_000, "short": 1_000_000}  # units a run is expected to take
# The user's word on 2026-09-24: the two short tasks first, the long ones after
# their numbers.
PILOT = [("397", ("cmax", "chigh")), ("394", ("cmax", "chigh"))]
# 259's long runs, cmax twice for the noise between two identical runs. No
# chigh: 526 closed on the pilot (note 610), the user's word on 2026-09-24.
LONG = [("396", ("cmax", "cmax", "bmax"))]

CELLS = {
    "cmax": {"effort": "max", "trigger": True},
    "chigh": {"effort": "high", "trigger": True},
    "bmax": {"effort": "max", "trigger": False, "compact": True},
}

APPEND = (
    "This session runs unattended: nobody will answer a question or approve a plan. "
    "When a choice is open, take the option you would recommend, write the choice and why on the board, and go on. "
    "Finish the work with a commit; never push. Work only in this repository and its board."
)
DISALLOWED = [
    "mcp__plugin_ekko_ekko__ask",  # it opens a dialog on the user's desktop
    "AskUserQuestion",
    "Artifact",
    "ArtifactComments",
    "ArtifactData",
    "mcp__claude_ai_Claude_Docs",
    "CronCreate",
    "ScheduleWakeup",
    "RemoteTrigger",
    "PushNotification",
    "SendFeedback",
    "Bash(gh:*)",
    "Bash(git push:*)",
]
# Where the answer is: the real repos, and the transcripts of the sessions
# that wrote it.
DENY = [
    "Read(~/Projetos/**)",
    "Edit(~/Projetos/**)",
    "Edit(~/NixOS/**)",
    "Read(~/.claude/projects/**)",
    "Read(~/.claude-trabalho/projects/**)",
]
PEEK = re.compile(r"Projetos/(ekko|ctx)|\.claude(-trabalho)?/projects|\bbccf056|\b2779afb|\be01e6d1")


def git(cwd, *args):
    return subprocess.run(["git", *args], cwd=cwd, check=True, capture_output=True, text=True).stdout.strip()


def now():
    return datetime.datetime.now(LOCAL)


def log(text):
    print(f"{now():%m-%d %H:%M:%S} {text}", flush=True)


# ---- the board as it stood ----------------------------------------------------


def snapshot(start_ms, targets, dest):
    """Copy the board into dest as it stood at start_ms: items created later
    left out, stashes and answers made later undone, tasks finished later
    reopened, no session holding anything, the targets pending. Text edited
    later cannot be recovered (the board keeps its last 51 versions only):
    returns the ids whose text may be newer than start_ms, for review."""
    with open(os.path.join(BOARD, "storage", "storage.json")) as handle:
        items = json.load(handle)
    kept, newer = {}, []
    for key, item in items.items():
        if int(item.get("_timestamp") or 0) >= start_ms:
            continue
        item = dict(item)
        item.pop("heldBy", None)
        for field in ("stashed", "trashed"):
            if isinstance(item.get(field), (int, float)) and item[field] >= start_ms:
                item.pop(field)
        answer = (item.get("question") or {}).get("answer")
        if answer and int((answer.get("by") or {}).get("since") or 0) >= start_ms:
            item["question"] = {k: v for k, v in item["question"].items() if k != "answer"}
        done = int((item.get("doneBy") or {}).get("since") or 0)
        if item["_id"] in targets or done >= start_ms:
            item.update(isComplete=False, inProgress=False)
            for field in ("doneBy", "cancelled", "waiting"):
                item.pop(field, None)
        if int(item.get("updatedAt") or 0) >= start_ms and not item.get("_isTask"):
            newer.append(item["_id"])
        kept[key] = item
    counters = json.load(open(os.path.join(BOARD, "storage", "counters.json")))
    counters["highestId"] = max(int(item["_id"]) for item in kept.values())
    os.makedirs(os.path.join(dest, "storage"))
    for folder in ("history", "archive"):
        os.makedirs(os.path.join(dest, folder))
    shutil.copy(os.path.join(BOARD, "project.json"), dest)
    with open(os.path.join(dest, "storage", "storage.json"), "w") as handle:
        json.dump(kept, handle, indent=4)
    with open(os.path.join(dest, "storage", "counters.json"), "w") as handle:
        json.dump(counters, handle, indent=2)
    return len(kept), newer


def prepare(name, run_dir, build=True):
    prompt, commit, start_ms, size = TASKS[name]
    parent = git(REPO, "rev-parse", commit + "^")
    repo = os.path.join(run_dir, "repo")
    os.makedirs(repo)
    git(repo, "init", "-q", "-b", "main")
    git(repo, "fetch", "-q", "--no-tags", REPO, parent)
    git(repo, "reset", "-q", "--hard", "FETCH_HEAD")
    with open(os.path.join(repo, ".git", "info", "exclude"), "a") as handle:
        handle.write("/.ekko/\n")
    targets = {int(n) for n in re.findall(r"\d+", prompt)}
    count, newer = snapshot(start_ms, targets, os.path.join(repo, ".ekko"))
    for folder in ("ctx-state", "marks"):
        os.makedirs(os.path.join(run_dir, folder))
    if build:
        # Built before the clock starts, as the user's own tree always is.
        subprocess.run(["nix", "develop", "-c", "cargo", "test", "--no-run", "--quiet"], cwd=repo,
                       check=True, capture_output=True)
    return repo, parent, count, newer


def settings(run_dir, trigger):
    """The wrapper's own settings, with the harness's deny rules and, for the
    C cells, the trigger. Written whole, so it holds whether Claude Code
    merges a second --settings or keeps only the last."""
    wrapper = open(os.path.realpath(shutil.which("claude"))).read()
    found = re.search(r"--settings (\S+)", wrapper)
    base = json.load(open(found.group(1))) if found else {}
    base.pop("statusLine", None)
    # Default's settings.json turns auto-compact off, and Claude Code 2.1.283
    # reads autoCompactEnabled by the settings' precedence, where these
    # outrank the user's: without this, bmax's --autocompact would never
    # compact there. Every cell alike, as on trabalho, where it is on.
    base["autoCompactEnabled"] = True
    permissions = base.setdefault("permissions", {})
    permissions["deny"] = list(dict.fromkeys(permissions.get("deny", []) + DENY))
    if trigger:
        command = f"{sys.executable} {os.path.abspath(__file__)} trigger"
        base.setdefault("hooks", {}).setdefault("PostToolUse", []).append({"hooks": [{"type": "command", "command": command}]})
    path = os.path.join(run_dir, "settings.json")
    with open(path, "w") as handle:
        json.dump(base, handle, indent=2)
    return path


def child_env(run_dir, trigger):
    # Nothing of the session that launched the harness: the wrapper exports
    # its own CLAUDE_CODE_* settings.
    env = {k: v for k, v in os.environ.items() if not k.startswith("CLAUDE")}
    # No EKKO_DIR: it outranks the board found from the folder (directory.rs,
    # locate), and the run's board is the repo's own .ekko. Finding a board
    # writes nothing to the registry in ~/.ekko.
    env.pop("EKKO_DIR", None)
    env.pop("EKKO_PROJECT", None)
    env.update(
        **PROFILE_ENV,
        CTX_STATE=os.path.join(run_dir, "ctx-state"),
        CTX_WORKER_HOME=CTX_WORKER_HOME,
        CTX_HANDOFF_TOKENS="1000000000",  # the harness drives the resets
        CTX_COLD_TOKENS="0",
    )
    if trigger:
        env.update(PAIRED_T=str(T), PAIRED_MARKS=os.path.join(run_dir, "marks"), PAIRED_REPO=os.path.join(run_dir, "repo"))
    return env


# ---- transcripts --------------------------------------------------------------


def files_of(sid):
    main = glob.glob(os.path.join(PROFILE, "projects", "*", sid + ".jsonl"))
    subagents = glob.glob(os.path.join(PROFILE, "projects", "*", sid, "subagents", "*.jsonl"))
    return main, subagents


def calls_in(path):
    """Each API call a transcript records, once: (usage, row)."""
    seen = set()
    try:
        handle = open(path, errors="replace")
    except OSError:
        return
    with handle:
        for raw in handle:
            if '"usage"' not in raw:
                continue
            try:
                row = json.loads(raw)
            except ValueError:
                continue
            message = row.get("message") or {}
            usage = message.get("usage")
            if row.get("type") != "assistant" or not usage or message.get("model") == "<synthetic>":
                continue
            mid = message.get("id") or row.get("requestId")
            if mid in seen:
                continue
            seen.add(mid)
            yield usage, row


def units(usage):
    split = usage.get("cache_creation") or {}
    cw1h, cw5m = split.get("ephemeral_1h_input_tokens"), split.get("ephemeral_5m_input_tokens")
    if cw1h is None and cw5m is None:
        cw1h, cw5m = usage.get("cache_creation_input_tokens") or 0, 0
    return (W_IN * (usage.get("input_tokens") or 0) + W_CW1H * (cw1h or 0) + W_CW5M * (cw5m or 0)
            + W_CR * (usage.get("cache_read_input_tokens") or 0) + W_OUT * (usage.get("output_tokens") or 0))


def spent(sids):
    total = calls = 0
    for sid in sids:
        main, subagents = files_of(sid)
        for path in main + subagents:
            for usage, _ in calls_in(path):
                total += units(usage)
                calls += 1
    return total, calls


def compactions(path):
    """The context each compaction started from."""
    found = []
    try:
        handle = open(path, errors="replace")
    except OSError:
        return found
    with handle:
        for raw in handle:
            if "compact_boundary" in raw:
                try:
                    row = json.loads(raw)
                except ValueError:
                    continue
                if row.get("subtype") == "compact_boundary":
                    found.append((row.get("compactMetadata") or {}).get("preTokens"))
    return found


def last_context(transcript):
    """The last main-thread call's context, as ctx_last_call reads it."""
    try:
        with open(transcript, "rb") as handle:
            handle.seek(0, os.SEEK_END)
            handle.seek(max(0, handle.tell() - 4_000_000))
            lines = handle.read().decode(errors="replace").splitlines()
    except OSError:
        return 0
    for raw in reversed(lines):
        try:
            row = json.loads(raw)
        except ValueError:
            continue
        message = row.get("message") or {}
        usage = message.get("usage")
        if row.get("type") == "assistant" and not row.get("isSidechain") and usage and message.get("model") != "<synthetic>":
            return (usage.get("input_tokens") or 0) + (usage.get("cache_creation_input_tokens") or 0) + (usage.get("cache_read_input_tokens") or 0)
    return 0


def handoff_text(tokens, limit):
    try:
        text = open(HANDOFF_SKILL).read().split("---", 2)[2].strip()
    except (OSError, IndexError):
        text = "Write a handoff (kind handoff) on the ekko task in progress."
    text = re.sub(r"- Then end with one line telling the user[^\n]*",
                  "- Then end your turn at once: nobody will type /clear in this session, and a fresh session continues from the handoff.", text)
    return f"The context is at {tokens // 1000}k tokens, past {limit // 1000}k.\n\n{text}"


def trigger():
    """PostToolUse: past T, block with the handoff's ask; then, while no
    handoff is on the board, ask again every fifth tool call, as ctx's Stop
    hook would ask again at the next stop. The probe saw a session at effort
    low take the first ask as a remark and finish its work."""
    data = json.load(sys.stdin)
    limit, marks = int(os.environ.get("PAIRED_T") or 0), os.environ.get("PAIRED_MARKS")
    sid = data.get("session_id") or ""
    if not limit or not marks or not sid:
        return
    mark = os.path.join(marks, sid + ".fired")
    if os.path.exists(mark):
        state = json.load(open(mark))
        if handoffs_since(os.environ.get("PAIRED_REPO") or "", state["at"]):
            return
        state["after"] = state.get("after", 0) + 1
        with open(mark, "w") as handle:
            json.dump(state, handle)
        if state["after"] % 5 == 0:
            print(json.dumps({"decision": "block", "reason": f"The handoff asked for at {state['tokens'] // 1000}k is not on the board yet: write it now (kind handoff, on the task in progress), then end your turn."}))
        return
    tokens = last_context(data.get("transcript_path") or "")
    if tokens < limit:
        return
    with open(mark, "w") as handle:
        json.dump({"tokens": tokens, "at": time.time()}, handle)
    print(json.dumps({"decision": "block", "reason": handoff_text(tokens, limit)}))


# ---- a run --------------------------------------------------------------------


def handoffs_since(repo, since):
    try:
        items = json.load(open(os.path.join(repo, ".ekko", "storage", "storage.json")))
    except (OSError, ValueError):
        return []
    return [item["_id"] for item in items.values() if item.get("handoff") and int(item.get("_timestamp") or 0) >= since * 1000]


def windows(lines):
    """The last utilization of each window a stream reported, and when each
    resets: {"five_hour": (0.67, epoch), "seven_day": (0.40, epoch)}."""
    found = {}
    for raw in lines:
        if "rate_limit_event" not in raw:
            continue
        try:
            info = json.loads(raw).get("rate_limit_info") or {}
        except ValueError:
            continue
        for name, window in (info.get("unifiedWindows") or {}).items():
            found[name] = (window.get("utilization") or 0, window.get("resetsAt") or 0)
    return found


def use_account(name):
    """Log in with `name`'s profile from now on, and price runs by its point.
    Default's profile is ~/.claude with its global config in ~/.claude.json,
    which Claude Code reads only while CLAUDE_CONFIG_DIR is unset: set to
    ~/.claude it looks for ~/.claude/.claude.json instead."""
    global ACCOUNT, PROFILE, PROFILE_ENV, POINT
    ACCOUNT, POINT = name, POINTS[name]
    PROFILE = os.path.join(HOME, ".claude" if name == "default" else ".claude-" + name)
    PROFILE_ENV = {} if name == "default" else {"CLAUDE_CONFIG_DIR": PROFILE}


use_account(ACCOUNT)


def week_stop():
    return min(SEVEN_DAY + 0.1, 0.99)


def ping():
    """The account's windows now, for the price of one tiny call."""
    folder = os.path.join(ROOT, "ping")
    os.makedirs(folder, exist_ok=True)
    env = child_env(folder, False)
    env["CTX_DISABLE"] = "1"
    out = subprocess.run(["claude", "-p", "Responda apenas: ok", "--output-format", "stream-json", "--verbose",
                          "--effort", "low", "--disallowedTools", *DISALLOWED], cwd=folder, env=env,
                         capture_output=True, text=True, timeout=600, stdin=subprocess.DEVNULL).stdout
    return windows(out.splitlines())


def result_of(log_path):
    result = {}
    with open(log_path, errors="replace") as handle:
        for raw in handle:
            if '"type":"result"' in raw.replace(" ", ""):
                try:
                    result = json.loads(raw)
                except ValueError:
                    pass
    return result


def stop(proc):
    try:
        os.killpg(proc.pid, signal.SIGTERM)
        proc.wait(timeout=30)
    except (ProcessLookupError, subprocess.TimeoutExpired):
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        proc.wait()


def run(name, cell, rep, window=None, prompt=None, cap=None, effort=None, extra_env=None, build=True):
    task_prompt, commit, start_ms, size = TASKS[name]
    prompt, cap = prompt or task_prompt, cap or CAP[size]
    spec = dict(CELLS[cell])
    if effort:
        spec["effort"] = effort
    if spec.get("compact") and not window:
        window = WINDOW
    run_id = f"{name}-{cell}-{rep}-{now():%m%d-%H%M%S}"
    run_dir, out_dir = os.path.join(ROOT, run_id), os.path.join(OUT, "runs", run_id)
    os.makedirs(out_dir)
    log(f"{run_id}: preparing")
    repo, parent, count, newer = prepare(name, run_dir, build)
    record = {"run": run_id, "task": name, "cell": cell, "rep": rep, "account": ACCOUNT, "effort": spec["effort"],
              "T": T if spec["trigger"] else None,
              "window": window if spec.get("compact") else None, "parent": parent, "reference": commit, "board_items": count,
              "board_text_maybe_newer": newer, "sessions": [], "started": now().isoformat(timespec="seconds"),
              "guards": {"five_hour": FIVE_HOUR, "week": SEVEN_DAY, "week_stop": week_stop(), "cap": cap}}
    settings_path, env = settings(run_dir, spec["trigger"]), child_env(run_dir, spec["trigger"])
    env.update(extra_env or {})
    text, status = prompt, "done"
    for segment in range(SEGMENTS):
        sid = str(uuid.uuid4())
        command = ["claude", "-p", text, "--output-format", "stream-json", "--verbose", "--session-id", sid,
                   "--effort", spec["effort"], "--permission-mode", "auto", "--settings", settings_path,
                   "--append-system-prompt", APPEND, "--disallowedTools", *DISALLOWED]
        if spec.get("compact"):
            command += ["--autocompact", str(window)]
        stream = os.path.join(out_dir, f"{segment}-{sid}.stream.jsonl")
        began = time.time()
        log(f"{run_id}: session {segment} {sid} '{text}'")
        with open(stream, "w") as out:
            proc = subprocess.Popen(command, cwd=repo, env=env, stdout=out, stderr=subprocess.STDOUT,
                                    stdin=subprocess.DEVNULL, start_new_session=True)
            cleared = None
            while proc.poll() is None:
                time.sleep(10)
                total, _ = spent([s["sid"] for s in record["sessions"]] + [sid])
                if total > cap:
                    status = "capped"
                    stop(proc)
                    break
                with open(stream, errors="replace") as handle:
                    week = windows(handle).get("seven_day", (0, 0))[0]
                if week >= week_stop():
                    status = "week"
                    stop(proc)
                    break
                mark = os.path.join(run_dir, "marks", sid + ".fired")
                if os.path.exists(mark):
                    fired = json.load(open(mark))["at"]
                    written = handoffs_since(repo, fired)
                    if written and cleared is None:
                        cleared = time.time()
                    if cleared and time.time() - cleared > AFTER_HANDOFF:
                        stop(proc)  # the user's /clear
        main, subagents = files_of(sid)
        for path in main:
            os.makedirs(os.path.join(out_dir, "transcripts"), exist_ok=True)
            shutil.copy2(path, os.path.join(out_dir, "transcripts", os.path.basename(path)))
        for path in subagents:
            os.makedirs(os.path.join(out_dir, "transcripts", sid), exist_ok=True)
            shutil.copy2(path, os.path.join(out_dir, "transcripts", sid, os.path.basename(path)))
        mark = os.path.join(run_dir, "marks", sid + ".fired")
        fired = json.load(open(mark)) if os.path.exists(mark) else None
        result = result_of(stream)
        session = {"sid": sid, "prompt": text, "seconds": round(time.time() - began), "fired": fired,
                   "handoffs": handoffs_since(repo, fired["at"]) if fired else [], "exit": proc.returncode,
                   "compactions": [c for path in main for c in compactions(path)],
                   "result": {k: result.get(k) for k in ("subtype", "is_error", "num_turns", "total_cost_usd")},
                   "result_text": (result.get("result") or "")[:500]}
        record["sessions"].append(session)
        if result.get("is_error") and re.search(r"limit|rate", str(result.get("result")), re.I):
            status = "limit"
        if status != "done" or not fired:
            break
        text = "continuando"
    else:
        status = "segments"
    total, calls = spent([s["sid"] for s in record["sessions"]])
    record.update(status=status, units=round(total), points=round(total / POINT, 1), calls=calls,
                  ended=now().isoformat(timespec="seconds"),
                  commits=git(repo, "log", "--format=%h %s", f"{parent}..HEAD").splitlines(),
                  dirty=bool(git(repo, "status", "--porcelain")))
    with open(os.path.join(out_dir, "result.diff"), "w") as handle:
        handle.write(git(repo, "diff", parent))
    peeks = []
    for path in glob.glob(os.path.join(out_dir, "transcripts", "**", "*.jsonl"), recursive=True):
        for usage, row in calls_in(path):
            for block in (row.get("message") or {}).get("content") or []:
                if isinstance(block, dict) and block.get("type") == "tool_use" and PEEK.search(json.dumps(block.get("input"))):
                    peeks.append(json.dumps(block.get("input"))[:200])
    record["peeks"] = peeks
    shutil.copy(os.path.join(repo, ".ekko", "storage", "storage.json"), os.path.join(out_dir, "board.json"))
    with open(os.path.join(out_dir, "run.json"), "w") as handle:
        json.dump(record, handle, indent=2)
    log(f"{run_id}: {status}, {calls} calls, {total / 1e6:.2f}M units ({total / POINT:.0f} points), {len(peeks)} peeks")
    return record


# ---- the probe and the pilot ---------------------------------------------------


def probe(parts="cb"):
    """The plumbing, at effort low on a scratch copy of today's board: the
    trigger fires at a low T and a fresh session picks up the handoff; then
    --autocompact at its floor, to see where compaction fires."""
    global T
    # The repo at HEAD's parent and the board as it is now; ctx's hooks off,
    # so that whole files can be read to fill the context on purpose.
    TASKS["probe"] = ("probe", "HEAD", int(time.time() * 1000), "short")
    quiet = {"CTX_DISABLE": "1"}
    T = 25_000  # headless sessions start near 17.5k: the harness disallows Artifact and the Docs connector
    records = []
    if "c" in parts:
        records.append(run("probe", "cmax", 0, cap=1_500_000, effort="low", extra_env=quiet, build=False,
              prompt="Leia README.md e src/ops.rs inteiros, depois crie uma nota no board, solta, resumindo em três linhas o que src/ops.rs faz."))
    if "b" in parts:
        records.append(run("probe", "bmax", 0, window=100_000, cap=1_500_000, effort="low", extra_env=quiet, build=False,
              prompt="Leia inteiros, um de cada vez, src/agent.rs, src/ops.rs, src/mcp.rs e src/main.rs, depois diga quantas funções pub há em cada um."))
    for record in records:
        log(f"{record['run']}: " + json.dumps([{k: s[k] for k in ('prompt', 'fired', 'handoffs', 'compactions', 'result')} for s in record["sessions"]]))
    return records


def pilot(dry_run, anytime, window, long=False):
    log(f"account {ACCOUNT} at {POINT // 1000}k units a point; a run starts while the week is under {SEVEN_DAY:.0%} "
        f"and its expected points fit under {FIVE_HOUR:.0%} of the 5-hour window, and stops at {week_stop():.0%} of the week")
    rng = random.Random(549)
    queue = []
    for name, cells in LONG if long else PILOT:
        cells = list(cells)
        rng.shuffle(cells)
        reps = {}
        for cell in cells:
            reps[cell] = reps.get(cell, 0) + 1
            queue.append((name, cell, reps[cell]))
    log("queue: " + ", ".join(f"{n}/{c}/{r}" for n, c, r in queue))
    if dry_run:
        return
    spent_units, tries = 0, {}
    while queue:
        while not anytime and NIGHT[1] <= now().time() < NIGHT[0]:
            time.sleep(300)
        state = ping()
        five, five_resets = state.get("five_hour", (0, 0))
        week = state.get("seven_day", (0, 0))[0]
        log(f"windows: 5-hour {five:.0%}, week {week:.0%}")
        if week >= SEVEN_DAY:
            log(f"stopped: the week is at {week:.0%}, {len(queue)} runs left")
            break
        need = EXPECTED[TASKS[queue[0][0]][3]] / POINT / 100
        if five + need > FIVE_HOUR:
            wait = max(300, five_resets - time.time() + 120)
            log(f"the 5-hour window is at {five:.0%}: waiting {wait / 60:.0f} minutes")
            time.sleep(wait)
            continue
        name, cell, rep = queue[0]
        record = run(name, cell, rep, window=window)
        spent_units += record["units"]
        if record["status"] == "week":
            log(f"stopped mid-run: the week passed {week_stop():.0%}, {len(queue)} runs left")
            break
        if record["status"] == "limit":
            tries[(name, cell, rep)] = tries.get((name, cell, rep), 0) + 1
            log(f"usage limit: {name}/{cell}/{rep} discarded, again in an hour")
            if tries[(name, cell, rep)] >= 3:
                queue.pop(0)
            time.sleep(3600)
            continue
        queue.pop(0)
        if spent_units > STAGE_CAP:
            log(f"stage cap: {spent_units / 1e6:.1f}M units spent, {len(queue)} runs left")
            break
    log(f"pilot over: {spent_units / 1e6:.1f}M units ({spent_units / POINT:.0f} points)")


def main():
    global SEVEN_DAY
    parser = argparse.ArgumentParser()
    parser.add_argument("--account", choices=sorted(POINTS), default=ACCOUNT)
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("trigger")
    check = sub.add_parser("probe")
    check.add_argument("--parts", default="cb")
    one = sub.add_parser("run")
    one.add_argument("task", choices=sorted(TASKS))
    one.add_argument("cell", choices=sorted(CELLS))
    one.add_argument("--rep", type=int, default=1)
    one.add_argument("--window", type=int)
    one.add_argument("--week", type=float, help=f"the week's share at which a run in progress stops is this plus 0.1, at most 0.99 (default {SEVEN_DAY})")
    many = sub.add_parser("pilot")
    many.add_argument("--dry-run", action="store_true")
    many.add_argument("--anytime", action="store_true")
    many.add_argument("--window", type=int)
    many.add_argument("--long", action="store_true")
    many.add_argument("--week", type=float, help=f"the week's share under which a run may start (default {SEVEN_DAY})")
    args = parser.parse_args()
    use_account(args.account)
    SEVEN_DAY = getattr(args, "week", None) or SEVEN_DAY
    if args.command == "trigger":
        trigger()
    elif args.command == "probe":
        probe(args.parts)
    elif args.command == "run":
        run(args.task, args.cell, args.rep, window=args.window)
    else:
        pilot(args.dry_run, args.anytime, args.window, args.long)


if __name__ == "__main__":
    main()
