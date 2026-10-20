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

    nix develop -c python3 evals/paired/harness.py check [--live]
    nix develop -c python3 evals/paired/harness.py probe
    nix develop -c python3 evals/paired/harness.py run TASK CELL [--rep N]
    nix develop -c python3 evals/paired/harness.py pilot [--dry-run] [--anytime] [--long]

`check` tests the guard that keeps a run from where the answer is; --live
spends two tiny sessions to see the deny rules refuse what they name.
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
from transcripts import W_CW1H, W_CW5M, W_IN, W_OUT, profiles, read_weight  # noqa: E402

HOME = os.path.expanduser("~")
# The account that pays, `--account`: the pilot's short runs went on trabalho,
# 259's long runs on default (decisions 741 and 744). PROFILE, PROFILE_ENV and
# POINT follow it (use_account).
ACCOUNT = "default"
# Each account's Claude Code config folder, which a run's CLAUDE_CONFIG_DIR
# names; the `claude` wrapper exports the default's when it is unset. They
# were ~/.claude and ~/.claude-trabalho until the profiles moved, found on
# 2026-10-19 (task 2214), so preflight() checks the folder before a run
# spends.
PROFILES = {"default": os.path.join(HOME, ".nyx"), "trabalho": os.path.join(HOME, ".nox")}
# The run repos: big once built, outside the projects folder, and a folder
# name the transcript studies skip (evals/claude-code/transcripts.py, SKIP).
ROOT = os.path.join(HOME, ".cache", "ekko-paired")
OUT = os.path.join(REPO, "target", "evals", "paired")
BOARD = os.path.join(REPO, ".ekko")
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
# 1.85M units took it from 67% to 91% on 2026-09-24. Those were priced at
# Opus 5's read weight, and every call ran Opus 5.5: priced by model (task
# 2204) the same calls come to 0.754 of that on default (that window, 4.02M
# to 3.03M) and 0.779 on trabalho (runs 397-chigh-1 and 397-cmax-1, 1.84M to
# 1.43M), so 2.98M over 63 points and 1.44M over 24.
POINTS = {"default": 47_000, "trabalho": 60_000}
# The headless stream reports the account's windows (rate_limit_event): a run
# starts only if the 5-hour window can take it whole (its expected points on top
# stay under FIVE_HOUR), and the pilot stops at SEVEN_DAY of the week, so that
# the user's own days keep their quota; `pilot --week` moves it, at the user's
# word. A run in progress is stopped at week_stop(), short of the limit itself.
FIVE_HOUR, SEVEN_DAY = 0.95, 0.70
SEGMENTS = 8  # sessions a run may take before it is stopped
AFTER_HANDOFF = 180  # seconds a session may go on once its handoff is written
# Budgets below were set in units at Opus 5's read weight; each is that
# budget priced by model, at the ratios POINTS measured (task 2204).
STAGE_CAP = 19_000_000  # stage 1's units, every run together: 25M before
NIGHT = (datetime.time(23, 0), datetime.time(7, 30))  # when a run may start

# The prompt the user typed, the commit that answered it, when the prompt was
# typed (ms: the board is cut there), and the run's size. Read off the
# transcripts on 2026-09-24.
TASKS = {
    "396": ("faz a 125 junto com a 396", "bccf056", 1790280143642, "long"),
    "394": ("faz a 394", "2779afb", 1790206340347, "short"),
    "397": ("faz a 397", "e01e6d1", 1790205358820, "short"),
}
CAP = {"long": 6_800_000, "short": 2_300_000}  # units: a run is stopped past this (9M and 3M before)
EXPECTED = {"long": 2_900_000, "short": 780_000}  # units a run is expected to take (3.8M and 1M before)
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
# What a session reads in a config folder by Claude Code's own direction: its
# plugins' and skills' files and the user's CLAUDE.md. Beside these, its own
# folder in projects/ stays open, where a large tool result is saved for it to
# read: runs 396-cmax-1 and probe-cmax-0, denied all of projects/, were each
# refused their own hook output.
KEEP = ("plugins", "skills", "CLAUDE.md")
# Seconds a session may run before its transcript must be found: a session
# that cannot be priced cannot be capped either.
UNPRICED = 120


def git(cwd, *args):
    return subprocess.run(["git", *args], cwd=cwd, check=True, capture_output=True, text=True).stdout.strip()


def now():
    return datetime.datetime.now(LOCAL)


def log(text):
    print(f"{now():%m-%d %H:%M:%S} {text}", flush=True)


# ---- the guard: where the answer is --------------------------------------------


def projects_folder():
    """The folder the real repos live in, by its real path: REPO's."""
    return os.path.dirname(os.path.realpath(REPO))


def links_to_projects():
    """The home folder's links to the projects folder, as ~/Projects."""
    top = projects_folder()
    return [path for path in (os.path.join(HOME, name) for name in sorted(os.listdir(HOME)))
            if os.path.islink(path) and os.path.realpath(path) == top]


def folder_name(path):
    """The folder of a profile's projects/ that holds the sessions started in
    `path`: each character but an ASCII letter or digit turned to '-', as in
    src/repeats.rs."""
    return re.sub(r"[^A-Za-z0-9]", "-", path)


def guarded(run_dir):
    """Where the answer is, for the run in run_dir, as (path, is_dir) as it
    stands now: the projects folder; in every profile, each entry but KEEP,
    and in projects/ each folder but the run's own; and each other entry of
    ROOT, other runs' repos and the studies' copies among them."""
    own_run, own_folder = os.path.basename(run_dir), folder_name(os.path.join(run_dir, "repo"))
    places = [(projects_folder(), True)]
    for profile in profiles():
        for entry in sorted(os.listdir(profile)):
            path = os.path.join(profile, entry)
            if entry in KEEP:
                continue
            if entry == "projects":
                places += [(os.path.join(path, name), os.path.isdir(os.path.join(path, name)))
                           for name in sorted(os.listdir(path)) if name != own_folder]
            else:
                places.append((path, os.path.isdir(path)))
    if os.path.isdir(ROOT):
        places += [(os.path.join(ROOT, name), os.path.isdir(os.path.join(ROOT, name)))
                   for name in sorted(os.listdir(ROOT)) if name != own_run]
    return places


def literal(path):
    """`path` as a gitignore pattern that matches it alone."""
    return re.sub(r"([\\\[\]*?])", r"\\\1", path)


def deny_rules(run_dir):
    """The run's deny rules: Read on every guarded place, written from the
    filesystem root (//), and Edit on the projects folder and ~/NixOS. A deny
    rule also refuses a path through a link to what it names (permissions
    docs, Symlinks; check --live saw a read through ~/Projects refused on
    2026-10-19), so the links need no rules of their own. A place made after
    the run starts goes unnamed, and a command that opens files itself, as a
    script does, is past any deny rule (the same docs); peek_pattern() sees
    both in the transcripts."""
    rules = [f"Read(/{literal(path)}{'/**' if is_dir else ''})" for path, is_dir in guarded(run_dir)]
    return rules + [f"Edit(/{literal(projects_folder())}/**)", "Edit(~/NixOS/**)"]


def spelled(path):
    """A regex for `path` as a tool's input may write it: absolute, or from
    the home folder as ~, $HOME or ${HOME}; never the tail of a longer path."""
    tail = r"(?<![\w.~$}/-])"
    for home in sorted({HOME, os.path.realpath(HOME)}):
        if path.startswith(home + "/"):
            return r"(?:" + tail + re.escape(home) + r"|~|\$HOME|\$\{HOME\})" + re.escape(path[len(home):])
    return tail + re.escape(path)


def peek_pattern(run_dir):
    """What makes a tool call a peek, for the run in run_dir: naming the
    projects folder, by its path or a link; an entry of a profile but KEEP
    and the run's own transcript folder; an entry of ROOT but the run's own;
    or a task's reference commit. Unlike the deny rules, it also covers
    places made after the run started."""
    own_run, own_folder = os.path.basename(run_dir), folder_name(os.path.join(run_dir, "repo"))
    end = r"(?![\w.-])"
    parts = [spelled(path) + end for path in [projects_folder(), *links_to_projects()]]
    kept = "|".join([re.escape(name) + end for name in KEEP] + ["projects/" + re.escape(own_folder) + end])
    parts += [spelled(profile) + "/(?!" + kept + ")" for profile in profiles()]
    parts.append(spelled(ROOT) + "/(?!" + re.escape(own_run) + end + ")")
    commits = [commit for _, commit, _, _ in TASKS.values() if re.fullmatch(r"[0-9a-f]{7,40}", commit)]
    parts.append(r"\b(?:" + "|".join(commits) + ")")
    return re.compile("|".join(parts))


def strings(value):
    """Every string a tool's input holds, however nested."""
    if isinstance(value, str):
        yield value
    elif isinstance(value, dict):
        for item in value.values():
            yield from strings(item)
    elif isinstance(value, list):
        for item in value:
            yield from strings(item)


def peeked(peek, given):
    """Whether a tool call's input matches `peek`, string by string: in the
    input's JSON, a path that starts a command's second line follows the
    letter of its escaped newline, and would not be at a start."""
    return any(peek.search(text) for text in strings(given))


def handoff_skill(profile):
    """ctx's /handoff, as `profile` serves it to a session."""
    return os.path.join(profile, "skills", "handoff", "SKILL.md")


def preflight():
    """What a run needs of this machine, checked before it spends: the
    account's config folder, which holds its own .claude.json, with the
    /handoff it serves; and the runs kept outside the projects folder, which
    every run is denied. Whether Claude Code accepts the account shows only
    in a session: one it refuses ends in an error (run's status "error")."""
    problems = []
    if not os.path.isfile(os.path.join(PROFILE, ".claude.json")):
        problems.append(f"{PROFILE} holds no .claude.json, so it is not {ACCOUNT}'s config folder")
    if not os.path.isfile(handoff_skill(PROFILE)):
        problems.append(f"{handoff_skill(PROFILE)} is missing: a C cell would not ask in /handoff's words")
    if (os.path.realpath(ROOT) + "/").startswith(projects_folder() + "/"):
        problems.append(f"{ROOT} is inside {projects_folder()}, which every run is denied")
    return problems


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


def settings(run_dir, trigger, guard=True):
    """The wrapper's own settings, with the run's deny rules (unless guard is
    off, for check_live's control) and, for the C cells, the trigger. Written
    whole, so it holds whether Claude Code merges a second --settings or keeps
    only the last."""
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
    if guard:
        permissions["deny"] = list(dict.fromkeys(permissions.get("deny", []) + deny_rules(run_dir)))
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
    """A session's transcripts, main thread and subagents, in whichever
    profile Claude Code wrote them: looked for in PROFILE alone, they were
    not found once the profiles moved, and a run read as costing nothing
    would never have reached its cap (task 2214)."""
    main, subagents = [], []
    for profile in profiles():
        main += glob.glob(os.path.join(profile, "projects", "*", sid + ".jsonl"))
        subagents += glob.glob(os.path.join(profile, "projects", "*", sid, "subagents", "*.jsonl"))
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


def units(usage, model):
    split = usage.get("cache_creation") or {}
    cw1h, cw5m = split.get("ephemeral_1h_input_tokens"), split.get("ephemeral_5m_input_tokens")
    if cw1h is None and cw5m is None:
        cw1h, cw5m = usage.get("cache_creation_input_tokens") or 0, 0
    return (W_IN * (usage.get("input_tokens") or 0) + W_CW1H * (cw1h or 0) + W_CW5M * (cw5m or 0)
            + read_weight(model) * (usage.get("cache_read_input_tokens") or 0) + W_OUT * (usage.get("output_tokens") or 0))


def spent(sids):
    total = calls = 0
    for sid in sids:
        main, subagents = files_of(sid)
        for path in main + subagents:
            for usage, row in calls_in(path):
                total += units(usage, (row.get("message") or {}).get("model"))
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
    # The hook runs in the session's environment, whose CLAUDE_CONFIG_DIR is
    # the run's profile; preflight() checked its /handoff before the run.
    try:
        text = open(handoff_skill(os.environ.get("CLAUDE_CONFIG_DIR") or PROFILE)).read().split("---", 2)[2].strip()
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
    Each profile holds its own .claude.json, so CLAUDE_CONFIG_DIR names it
    for the default as for any other."""
    global ACCOUNT, PROFILE, PROFILE_ENV, POINT
    ACCOUNT, POINT, PROFILE = name, POINTS[name], PROFILES[name]
    PROFILE_ENV = {"CLAUDE_CONFIG_DIR": PROFILE}


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
    # The settings the run ran with, deny rules and all, beside its record.
    shutil.copy(settings_path, os.path.join(out_dir, "settings.json"))
    record["guards"]["deny_rules"] = len(json.load(open(settings_path))["permissions"]["deny"])
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
                if not files_of(sid)[0] and time.time() - began > UNPRICED:
                    status = "unpriced"
                    stop(proc)
                    break
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
        # A session Claude Code ended in an error is no measurement: when it
        # refuses the account, each session ends in one synthetic message
        # (seen 2026-10-19), and read as done such a run is priced at 0.
        if status == "done" and result.get("is_error"):
            status = "limit" if re.search(r"limit|rate", str(result.get("result")), re.I) else "error"
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
    peek, peeks = peek_pattern(run_dir), []
    for path in glob.glob(os.path.join(out_dir, "transcripts", "**", "*.jsonl"), recursive=True):
        for usage, row in calls_in(path):
            for block in (row.get("message") or {}).get("content") or []:
                if isinstance(block, dict) and block.get("type") == "tool_use" and peeked(peek, block.get("input")):
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
        if record["status"] == "unpriced":
            log(f"stopped: no profile holds the transcript of {name}/{cell}/{rep}, so no run can be capped; {len(queue)} runs left")
            break
        if record["status"] == "error":
            log(f"stopped: {name}/{cell}/{rep} ended in an error, {record['sessions'][-1]['result_text'][:200]!r}; {len(queue)} runs left")
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


# ---- the guard's check ------------------------------------------------------------


def check():
    """The guard, tested as for a run in the folder of an old run whose
    transcripts are still on disk: the peek pattern on calls it must flag and
    calls it must let pass, and the deny rules over each entry of each profile
    and of ROOT. Returns what is wrong, a line each."""
    runs = sorted(os.listdir(ROOT)) if os.path.isdir(ROOT) else []
    olds = [name for name in runs if any(os.path.isdir(os.path.join(profile, "projects", folder_name(os.path.join(ROOT, name, "repo"))))
                                         for profile in profiles())]
    own_run = olds[0] if olds else "397-cmax-1-0101-000000"
    other = next((name for name in runs if name != own_run), "394-chigh-1-0101-000000")
    run_dir = os.path.join(ROOT, own_run)
    own = folder_name(os.path.join(run_dir, "repo"))
    peek, rules = peek_pattern(run_dir), deny_rules(run_dir)
    repo, top = os.path.realpath(REPO), projects_folder()

    def tilde(path):
        return "~" + path[len(HOME):] if path.startswith(HOME + "/") else path

    flag = [f"cat {repo}/src/ops.rs", f"{top}/ctx/README.md", f"cd /tmp\n{top}/ekko/scripts/release.sh",
            f"{tilde(ROOT)}/{other}/repo/src/ops.rs", f"ls {ROOT}/", "git show " + TASKS["396"][1]]
    flag += [f"{tilde(link)}/ekko/Cargo.toml" for link in links_to_projects()]
    let = [f"{run_dir}/repo/src/ops.rs", "src/ops.rs", f"{tilde(run_dir)}/ctx-state/x", "/tmp/x", f"{top}2/x",
           f"/srv{top}/x", "git log HEAD", "/nix/store/abc-ctx/share/ctx/skills/handoff/SKILL.md"]
    for profile in profiles():
        flag += [f"{profile}/projects/{folder_name(repo)}/x.jsonl", f"{tilde(profile)}/history.jsonl",
                 f"cat ${{HOME}}{tilde(profile)[1:]}/file-history/x", f"ls {profile}/projects/"]
        let += [f"{profile}/projects/{own}/x/tool-results/hook-x-stdout.txt", f"{tilde(profile)}/skills/handoff/SKILL.md",
                f"{profile}/plugins/x", f"{profile}/CLAUDE.md", f"{profile}x/projects/y"]
    wrong = [f"peeks miss {text!r}" for text in flag if not peeked(peek, {"command": text})]
    wrong += [f"peeks take {text!r}" for text in let if peeked(peek, {"command": text})]
    named = set(rules)

    def denied(path):
        return f"Read(/{literal(path)})" in named or f"Read(/{literal(path)}/**)" in named

    seen_own = False
    for profile in profiles():
        for entry in sorted(os.listdir(profile)):
            path = os.path.join(profile, entry)
            for place in [os.path.join(path, name) for name in sorted(os.listdir(path))] if entry == "projects" else [path]:
                must = entry not in KEEP and os.path.basename(place) != own
                seen_own |= os.path.basename(place) == own
                if denied(place) != must:
                    wrong.append(f"deny rules {'miss' if must else 'name'} {place}")
    for name in runs:
        if denied(os.path.join(ROOT, name)) != (name != own_run):
            wrong.append(f"deny rules {'miss' if name != own_run else 'name'} {os.path.join(ROOT, name)}")
    for rule in (f"Read(/{literal(top)}/**)", f"Edit(/{literal(top)}/**)"):
        if rule not in named:
            wrong.append(f"deny rules miss {rule}")
    log(f"guard, as for run {own_run}: {len(flag)} calls to flag, {len(let)} to let pass; {len(rules)} deny rules over "
        f"{len(profiles())} profiles and {len(runs)} entries of {ROOT}; the run's own transcript folder "
        + ("is among them" if seen_own else "is on disk in no profile, so its case is untried"))
    return wrong


def tool_answers(paths):
    """How each tool call in the transcripts was answered: its file or
    command -> (refused, the answer's start)."""
    calls, answers = {}, {}
    for path in paths:
        for raw in open(path, errors="replace"):
            try:
                row = json.loads(raw)
            except ValueError:
                continue
            content = (row.get("message") or {}).get("content")
            for block in content if isinstance(content, list) else []:
                if not isinstance(block, dict):
                    continue
                if block.get("type") == "tool_use":
                    given = block.get("input") or {}
                    calls[block.get("id")] = given.get("file_path") or given.get("command")
                elif block.get("type") == "tool_result" and block.get("tool_use_id") in calls:
                    text = block.get("content")
                    if isinstance(text, list):
                        text = " ".join(part.get("text", "") for part in text if isinstance(part, dict))
                    answers[calls[block["tool_use_id"]]] = (bool(block.get("is_error")), str(text)[:300].replace("\n", " "))
    return answers


def check_live():
    """Two tiny sessions alike but for the deny rules, each asked to read the
    same paths: guarded ones, and ones a run reads by right. Right when the
    session with the rules is refused just the guarded ones, each with a
    permission refusal, and the one without them is refused none: then the
    refusals are the rules' doing. Haiku, ctx's hooks off, a line a read.
    Returns what is wrong, a line each."""
    stamp = f"{now():%m%d-%H%M%S}"
    repo = os.path.realpath(REPO)
    sessions = sorted(glob.glob(os.path.join(PROFILE, "projects", folder_name(repo), "*.jsonl")), key=os.path.getmtime)
    olds = [name for name in sorted(os.listdir(ROOT)) if os.path.isfile(os.path.join(ROOT, name, "repo", "Cargo.toml"))]
    shut = [os.path.join(repo, "Cargo.toml")]
    shut += [os.path.join(link, os.path.basename(repo), "Cargo.toml") for link in links_to_projects()[:1]]
    shut += sessions[-1:] + [os.path.join(PROFILE, "history.jsonl")]
    shut += [os.path.join(ROOT, name, "repo", "Cargo.toml") for name in olds[:1]]
    command = f"head -1 {shut[0]}"
    wrong = []
    for arm in ("guard", "control"):
        run_dir = os.path.join(ROOT, f"check-{arm}-{stamp}")
        work = os.path.join(run_dir, "repo")
        os.makedirs(work)
        mine = os.path.join(work, "notes.txt")
        with open(mine, "w") as handle:
            handle.write("A file of the run's own.\n")
        sid = str(uuid.uuid4())
        right = [mine, os.path.join(PROFILE, "projects", folder_name(work), sid + ".jsonl"), handoff_skill(PROFILE)]
        reads = shut + right
        prompt = ("This is a permissions check. Call the Read tool once for each path below, in this order, with limit 1, "
                  "and go on to the next whatever the result:\n" + "\n".join(f"{n}. {path}" for n, path in enumerate(reads, 1))
                  + f"\nThen run this Bash command once, exactly as written: {command}\nThen reply DONE.")
        env = child_env(run_dir, False)
        env["CTX_DISABLE"] = "1"
        path = settings(run_dir, False, guard=arm == "guard")
        out = subprocess.run(["claude", "-p", prompt, "--model", "haiku", "--output-format", "stream-json", "--verbose",
                              "--session-id", sid, "--permission-mode", "auto", "--settings", path,
                              "--disallowedTools", *DISALLOWED],
                             cwd=work, env=env, capture_output=True, text=True, timeout=600, stdin=subprocess.DEVNULL)
        stream = os.path.join(run_dir, "stream.jsonl")
        with open(stream, "w") as handle:
            handle.write(out.stdout + out.stderr)
        answers = tool_answers(files_of(sid)[0])
        units, calls = spent([sid])
        deny = len(json.load(open(path)).get("permissions", {}).get("deny", []))
        result = result_of(stream)
        log(f"{arm}: {deny} deny rules, session {sid}, {calls} calls, {units / 1e3:.0f}k units, exit {out.returncode}"
            + (f", ended in error: {str(result.get('result'))[:200]}" if result.get("is_error") else ""))
        for target in reads + [command]:
            must = arm == "guard" and target not in right
            if target not in answers:
                wrong.append(f"{arm}: no call reads {target}")
                log(f"  {'refuse' if must else 'allow '} -> no call: {target}")
                continue
            refused, text = answers[target]
            log(f"  {'refuse' if must else 'allow '} -> {'refused' if refused else 'read'}: {target} | {text[:110]}")
            if refused != must or (must and "permission" not in text.lower()):
                wrong.append(f"{arm}: {'refused' if refused else 'read'} {target}: {text[:110]}")
    return wrong


def main():
    global SEVEN_DAY
    parser = argparse.ArgumentParser()
    parser.add_argument("--account", choices=sorted(POINTS), default=ACCOUNT)
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("trigger")
    guard = sub.add_parser("check")
    guard.add_argument("--live", action="store_true", help="also spend two tiny Haiku sessions on the deny rules")
    plumbing = sub.add_parser("probe")
    plumbing.add_argument("--parts", default="cb")
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
    if args.command != "trigger" and (args.command != "check" or args.live):
        problems = preflight()
        for problem in problems:
            log(f"not ready: {problem}")
        if problems:
            sys.exit(1)
    if args.command == "trigger":
        trigger()
    elif args.command == "check":
        wrong = check() + (check_live() if args.live else [])
        for line in wrong:
            log(f"wrong: {line}")
        log("guard: right" if not wrong else f"guard: {len(wrong)} wrong")
        sys.exit(1 if wrong else 0)
    elif args.command == "probe":
        probe(args.parts)
    elif args.command == "run":
        run(args.task, args.cell, args.rep, window=args.window)
    else:
        pilot(args.dry_run, args.anytime, args.window, args.long)


if __name__ == "__main__":
    main()
