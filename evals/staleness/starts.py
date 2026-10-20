"""Task 2197, part A: every session start whose prime showed a handoff, over
this machine's transcripts, and what had moved between the handoff and the
start -- commits, tags, the tasks the handoff names -- beside what the session
spent before its first productive action. No quota; reads only.

    nix develop -c python3 evals/staleness/starts.py [OUT]

OUT defaults to target/evals/staleness/starts.jsonl.

A start is a SessionStart whose ekko --prime hook output holds "Where the last
session stopped". Its segment runs to the next start or compaction in the same
transcript. Calls are counted once by message id, main thread only, and priced
as evals/claude-code/transcripts.py prices them (units of one uncached input
token: 1 h cache write 2, 5 min write 1.25, read by the call's model as
transcripts.read_weight says -- 0.05 on Opus 5.5, 0.1 on Opus 5 -- output 5).

What moved is read from what exists today, so it is an estimate where history
is gone: commits from each repository's reflog-free history (every ref, by
committer time), tags by their date, and a task's state at a moment from the
board's .ekko/history snapshots when one lies within SNAP_SLACK before it,
otherwise from the item's own doneBy.since. The result says, per start, which
source each answer came from.
"""

import bisect
import datetime
import glob
import json
import os
import re
import subprocess
import sys

import config as C

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "claude-code"))
sys.path.insert(0, os.path.join(HERE, "..", "recall"))
sys.path.insert(0, os.path.join(HERE, ".."))
import boards  # noqa: E402
import transcripts as T  # noqa: E402
from replay import cited_ids  # noqa: E402

HOME = os.path.expanduser("~")
MOVED = [(os.path.expanduser(old), os.path.expanduser(new)) for old, new in C.study().get("moved", {}).items()]
LOCAL = T.LOCAL
SNAP_SLACK = datetime.timedelta(hours=36)
CHARS_PER_TOKEN_TOOL = 2.28  # note 286: other tools' results
CHARS_PER_TOKEN_EKKO = 2.59  # note 286: ekko's text


HEADER = re.compile(
    r"Where the last session stopped: handoff (\d+) on task (\d+) \[([a-z ]+)\]"
    r"(?:, written (\d{4}-\d{2}-\d{2} \d{2}:\d{2}))?(.*)"
)
GIT_LOOK = re.compile(r"\bgit\b[^|;&]*?\b(log|status|diff|show|rev-list|describe|reflog|shortlog|branch|tag)\b")
EKKO_READ = re.compile(r"^mcp__.*ekko.*__(context|search|next|prime|changes|roadmap)$")
EKKO_ANY = re.compile(r"^mcp__.*ekko.*__(\w+)$")
CORRECTION = re.compile(
    r"(?i)\b(j[aá]\s+(foi|fo[iy]\s+feit|est[aá]|t[aá]\b|fiz|fizemos|fez|feit|termin|conclu|resolv|merge|mergead|commit|"
    r"pushad|lan[cç]|exist|tinha|tem\s+isso|era)|isso\s+j[aá]|already\s+(done|merged|fixed|released|pushed|exists?))"
)


class Call:
    __slots__ = ("mid", "at", "inp", "cw1h", "cw5m", "cr", "out", "tools", "stop", "texts", "model")

    @property
    def ctx(self):
        return self.inp + self.cw1h + self.cw5m + self.cr

    @property
    def w_cr(self):
        return T.read_weight(self.model)

    @property
    def cost(self):
        return T.W_IN * self.inp + T.W_CW1H * self.cw1h + T.W_CW5M * self.cw5m + self.w_cr * self.cr + T.W_OUT * self.out

    def productive(self):
        for tool in self.tools:
            name, arg = tool["name"], tool["arg"]
            if name in T.EDIT_TOOLS:
                return "edit"
            if T.BOARD_WRITE.match(name):
                return "board"
            if name == "Bash" and "git commit" in arg:
                return "commit"
        return "answer" if self.stop == "end_turn" else None


def tool_arg(block):
    args = block.get("input") or {}
    name = block.get("name", "")
    if name == "Bash":
        return args.get("command") or ""
    if name.startswith("mcp__"):
        return json.dumps(args, ensure_ascii=False)[:400]
    return args.get("file_path") or args.get("notebook_path") or args.get("pattern") or ""


def result_text(block):
    content = block.get("content")
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "\n".join(part.get("text", "") for part in content if isinstance(part, dict))
    return ""


def scan_file(path):
    """The events of one transcript, in file order: starts, compactions,
    prompts and main-thread calls, and every tool result's text by id."""
    events, calls, results, cwd = [], {}, {}, None
    starts = {}
    with open(path, errors="replace") as handle:
        for line_no, raw in enumerate(handle):
            try:
                row = json.loads(raw)
            except ValueError:
                continue
            cwd = row.get("cwd") or cwd
            stamp = row.get("timestamp")
            kind = row.get("type")
            if kind == "attachment":
                attachment = row.get("attachment") or {}
                name = attachment.get("hookName") or ""
                if not name.startswith("SessionStart:") or not stamp:
                    continue
                key = attachment.get("toolUseID")
                if key not in starts:
                    starts[key] = {"at": T.when(stamp), "source": name.split(":", 1)[1], "hooks": {}, "cwd": cwd, "line": line_no}
                    events.append((line_no, "start", starts[key]))
                command = attachment.get("command") or ""
                content = attachment.get("content") or attachment.get("stdout") or ""
                label = (
                    "prime" if "--prime" in command else "tasklist" if "--tasklist" in command else "memory" if "--memory" in command
                    else "ctx" if "/ctx" in command else "graff" if "graff" in command else command.rsplit("/", 1)[-1][:40]
                )
                starts[key]["hooks"][label] = {"chars": len(content), "ms": attachment.get("durationMs"), "content": content if label in ("prime", "ctx") else None}
            elif kind == "assistant" and stamp:
                if row.get("isSidechain"):
                    continue
                message = row.get("message") or {}
                usage = message.get("usage")
                mid = message.get("id") or row.get("requestId")
                if not usage or not mid or message.get("model") == "<synthetic>":
                    continue
                blocks = [b for b in message.get("content") or [] if isinstance(b, dict)]
                tools = [{"id": b.get("id"), "name": b.get("name", ""), "arg": tool_arg(b)} for b in blocks if b.get("type") == "tool_use"]
                texts = [b.get("text", "") for b in blocks if b.get("type") == "text"]
                if mid in calls:
                    calls[mid].tools += tools
                    calls[mid].texts += texts
                    calls[mid].stop = message.get("stop_reason") or calls[mid].stop
                    continue
                call = Call()
                call.mid, call.at, call.model = mid, T.when(stamp), message.get("model") or ""
                split = usage.get("cache_creation") or {}
                c1, c5 = split.get("ephemeral_1h_input_tokens"), split.get("ephemeral_5m_input_tokens")
                if c1 is None and c5 is None:
                    c1, c5 = usage.get("cache_creation_input_tokens") or 0, 0
                call.cw1h, call.cw5m = c1 or 0, c5 or 0
                call.inp = usage.get("input_tokens") or 0
                call.cr = usage.get("cache_read_input_tokens") or 0
                call.out = usage.get("output_tokens") or 0
                call.tools, call.texts, call.stop = tools, texts, message.get("stop_reason")
                calls[mid] = call
                events.append((line_no, "call", call))
            elif kind == "system" and row.get("subtype") == "compact_boundary" and stamp:
                events.append((line_no, "compact", {"at": T.when(stamp)}))
            elif kind == "user" and stamp and not row.get("isSidechain"):
                content = (row.get("message") or {}).get("content")
                if isinstance(content, list):
                    for block in content:
                        if isinstance(block, dict) and block.get("type") == "tool_result":
                            results[block.get("tool_use_id")] = result_text(block)
                if row.get("isMeta"):
                    continue
                if isinstance(content, str):
                    text = content
                else:
                    text = " ".join(b.get("text", "") for b in content or [] if isinstance(b, dict) and b.get("type") == "text")
                if text.strip() and not text.lstrip().startswith(("<command-", "<local-command", "<task-notification", "<system-reminder")):
                    events.append((line_no, "prompt", {"at": T.when(stamp), "text": text}))
    return events, results


def segments(events):
    out = []
    current = None
    for _, kind, data in events:
        if kind in ("start", "compact"):
            current = {"how": kind, "event": data, "calls": [], "prompts": []}
            out.append(current)
        elif current is None:
            continue
        elif kind == "prompt":
            current["prompts"].append(data)
        elif kind == "call":
            current["calls"].append(data)
    return out


# ---- the world each start met --------------------------------------------------

_repo_cache = {}


def repo_of(cwd):
    """The main repository's top folder for `cwd`, a worktree's included, or None."""
    if not cwd:
        return None
    probe = cwd
    while probe and not os.path.isdir(probe):
        probe = os.path.dirname(probe)
    if probe in _repo_cache:
        return _repo_cache[probe]
    found = None
    try:
        common = subprocess.run(["git", "-C", probe, "rev-parse", "--path-format=absolute", "--git-common-dir"],
                                capture_output=True, text=True, timeout=10)
        if common.returncode == 0:
            path = common.stdout.strip()
            found = os.path.dirname(path) if path.endswith("/.git") else path
    except (OSError, subprocess.TimeoutExpired):
        pass
    _repo_cache[probe] = found
    return found


_history_cache = {}


def history(repo):
    """Every commit reachable from any ref, oldest first, as (committer time, hash, subject),
    and the tags with their dates."""
    if repo in _history_cache:
        return _history_cache[repo]
    log = subprocess.run(["git", "-C", repo, "log", "--all", "--format=%ct%x09%H%x09%s"], capture_output=True, text=True)
    commits = []
    for line in log.stdout.splitlines():
        parts = line.split("\t", 2)
        if len(parts) == 3 and parts[0].isdigit():
            commits.append((int(parts[0]), parts[1], parts[2]))
    commits.sort()
    tags = subprocess.run(["git", "-C", repo, "for-each-ref", "refs/tags", "--format=%(creatordate:unix)%09%(refname:short)"],
                          capture_output=True, text=True)
    tag_list = sorted((int(a), b) for a, b in (line.split("\t", 1) for line in tags.stdout.splitlines() if "\t" in line) if a.isdigit())
    _history_cache[repo] = (commits, tag_list)
    return commits, tag_list


def board_dir(cwd):
    probe = cwd
    while probe and probe != "/":
        if os.path.isfile(os.path.join(probe, ".ekko", "storage", "storage.json")):
            return os.path.join(probe, ".ekko")
        probe = os.path.dirname(probe)
    return os.path.join(HOME, ".ekko")


_board_cache = {}


def board(path):
    """The board today, its snapshots' times, and a loader for one snapshot."""
    if path in _board_cache:
        return _board_cache[path]
    items = {int(item["_id"]): item for item in boards.read(path).values()}
    # The versions history/ kept as JSON, then the changes the board's
    # database keeps (task 2233): history/ holds only stamps from then on.
    snaps = boards.snapshots(path)
    _board_cache[path] = {"path": path, "items": items, "snaps": snaps, "loaded": {}}
    return _board_cache[path]


def state_of(item):
    if not item:
        return "absent"
    if item.get("trashed"):
        return "trashed"
    if not item.get("_isTask"):
        return "note"
    if item.get("cancelled"):
        return "cancelled"
    if item.get("isComplete"):
        return "done"
    if item.get("waiting"):
        return "waiting"
    if item.get("inProgress"):
        return "progress"
    return "pending"


def snapshot_items(b, key):
    if key not in b["loaded"]:
        b["loaded"][key] = boards.at(b["path"], key)
    return b["loaded"][key]


def closed_at(b, item_id, moment):
    """Whether the task was closed (done or cancelled) at `moment` (epoch
    seconds): True, False, or None when nothing on disk settles it.
    No item records when it was closed: doneBy.since is when the session
    that closed it claimed it (src/holder.rs, Holder::since), so a close lies
    between that claim (or the item's creation) and updatedAt. The board's
    history snapshots settle what those bounds leave open: closed in the last
    snapshot before the moment, or open in the first one after it."""
    item = b["items"].get(item_id)
    if not item:
        return None
    if state_of(item) not in ("done", "cancelled"):
        return False
    lower = ((item.get("doneBy") or {}).get("since") or item.get("_timestamp") or 0) / 1000
    upper = (item.get("updatedAt") or 0) / 1000
    if upper and upper <= moment:
        return True
    if lower > moment:
        return False
    times = [t for t, _ in b["snaps"]]
    before = bisect.bisect_right(times, moment) - 1
    if before >= 0:
        then = snapshot_items(b, b["snaps"][before][1]).get(item_id)
        if then is not None and state_of(then) in ("done", "cancelled"):
            return True
    after = bisect.bisect_left(times, moment)
    if after < len(times):
        then = snapshot_items(b, b["snaps"][after][1]).get(item_id)
        if then is not None and state_of(then) not in ("done", "cancelled"):
            return False
    return None


def states_at(b, ids, moment):
    """Each id's state at `moment`: closed, open, absent (made later) or
    unknown, and where the answers came from."""
    out = {}
    for i in ids:
        item = b["items"].get(i)
        if not item or (item.get("_timestamp") or 0) / 1000 > moment:
            out[i] = "absent"
            continue
        closed = closed_at(b, i, moment)
        out[i] = "unknown" if closed is None else "closed" if closed else "open"
    return out, "bounds+snapshots"


def same(a, b):
    """Whether two states read as one; unknown matches anything."""
    return a == b or "unknown" in (a, b)


def main():
    records = []
    paths = []
    for base in T.profiles():
        for path in sorted(glob.glob(os.path.join(base, "projects", "**", "*.jsonl"), recursive=True)):
            if T.SKIP in path or "/subagents/" in path:
                continue
            paths.append((os.path.basename(base), path))
    for profile, path in paths:
        events, results = scan_file(path)
        for segment in segments(events):
            if segment["how"] != "start":
                continue
            start = segment["event"]
            prime = (start["hooks"].get("prime") or {}).get("content") or ""
            match = HEADER.search(prime)
            if not match:
                continue
            handoff_id, task_id, task_state, written, tail = match.groups()
            body_lines = []
            for line in prime[match.end():].splitlines()[1:]:
                if line.startswith("    > "):
                    body_lines.append(line[6:])
                elif line.startswith("    …"):
                    continue
                elif body_lines or line.strip():
                    break
            body = "\n".join(body_lines)
            cwd = start["cwd"] or ""
            # Project folders that moved since (study.json's "moved", old prefix -> new):
            # the repositories and boards moved with them.
            for old, new in MOVED:
                if cwd.startswith(old):
                    cwd = new + cwd[len(old):]
            repo = repo_of(cwd)
            bdir = board_dir(cwd) if cwd else os.path.join(HOME, ".ekko")
            try:
                b = board(bdir)
            except (OSError, ValueError):
                b = None
            t0 = start["at"]
            if written:
                # The header gives the minute; the board the millisecond, while the
                # handoff has not been touched since. Else the minute's end: only
                # what surely came after the handoff counts as moved since.
                t_h = datetime.datetime.strptime(written, "%Y-%m-%d %H:%M").replace(tzinfo=LOCAL)
                item = b["items"].get(int(handoff_id)) if b else None
                exact = ((item.get("updatedAt") or item.get("_timestamp")) / 1000) if item else None
                if exact is not None and t_h.timestamp() <= exact < t_h.timestamp() + 60:
                    t_h, written_from = datetime.datetime.fromtimestamp(exact, LOCAL), "board-exact"
                else:
                    t_h, written_from = t_h + datetime.timedelta(seconds=59.999), "header-minute-end"
            elif b and int(handoff_id) in b["items"]:
                item = b["items"][int(handoff_id)]
                t_h = datetime.datetime.fromtimestamp((item.get("updatedAt") or item.get("_timestamp")) / 1000, LOCAL)
                written_from = "board"
            else:
                t_h, written_from = None, None
            record = {
                "profile": profile, "file": os.path.basename(path)[:8], "source": start["source"], "at": t0.isoformat(timespec="minutes"),
                "cwd": cwd, "repo": repo, "board": bdir, "handoff": int(handoff_id), "task": int(task_id), "task_state": task_state,
                "written": t_h.isoformat(timespec="minutes") if t_h else None, "written_from": written_from,
                "whose": ("this" if "by this session" in tail else "runs" if "still runs" in tail else "ended" if "has ended" in tail else None),
                "prime_chars": len(prime), "prime_ms": (start["hooks"].get("prime") or {}).get("ms"),
                "hooks": {k: {"chars": v["chars"], "ms": v["ms"]} for k, v in start["hooks"].items()},
                "handoff_chars": len(body),
            }
            if t_h:
                record["age_h"] = round((t0 - t_h).total_seconds() / 3600, 2)
            # What moved.
            if repo and t_h:
                commits, tags = history(repo)
                lo, hi = t_h.timestamp(), t0.timestamp()
                moved = [c for c in commits if lo < c[0] <= hi]
                record["commits_since"] = len(moved)
                record["commit_subjects"] = [c[2][:80] for c in moved[-8:]]
                record["tags_since"] = [name for when_, name in tags if lo < when_ <= hi]
            if b and t_h:
                exists = set(b["items"])
                named = sorted(i for i in cited_ids(body, exists) | {int(task_id)} if i in b["items"] and b["items"][i].get("_isTask"))
                before, src_b = states_at(b, named, t_h.timestamp())
                after, src_a = states_at(b, named, t0.timestamp())
                changed = {i: (before[i], after[i]) for i in named if before[i] != "absent" and not same(before[i], after[i])}
                record["named_tasks"] = named
                record["named_changed"] = {str(k): v for k, v in changed.items()}
                record["named_unknown"] = [i for i in named if "unknown" in (before[i], after[i])]
                own = int(task_id)
                if own in after and after[own] == "unknown":
                    # The header says what the handoff's own task was at the start.
                    after[own] = "closed" if task_state in ("done", "cancelled") else "open"
                    if before.get(own) == "open" and after[own] == "closed":
                        record["named_changed"][str(own)] = ["open", "closed"]
                record["states_from"] = [src_b, src_a]
                record["tasks_created_since"] = sum(
                    1 for item in b["items"].values()
                    if item.get("_isTask") and t_h.timestamp() < (item.get("_timestamp") or 0) / 1000 <= t0.timestamp()
                )
            # What the session did before its first productive action.
            calls = segment["calls"]
            first_prompt = segment["prompts"][0] if segment["prompts"] else None
            record["first_prompt"] = (first_prompt["text"].strip()[:120] if first_prompt else None)
            record["calls"] = len(calls)
            found = next(((i, c.productive()) for i, c in enumerate(calls) if c.productive()), None)
            index = found[0] if found else len(calls)
            record["first_kind"] = found[1] if found else None
            record["R_calls"] = index if found else None
            record["R_units"] = round(sum(c.cost for c in calls[:index])) if found else None
            record["R_ctx"] = (calls[index].ctx - calls[0].ctx) if found and calls else None
            begin = first_prompt["at"] if first_prompt and calls and first_prompt["at"] <= calls[0].at else (calls[0].at if calls else None)
            record["R_seconds"] = round((calls[index].at - begin).total_seconds()) if found and begin else None
            edit = next((i for i, c in enumerate(calls) if c.productive() in ("edit", "commit")), None)
            record["E_calls"] = edit
            record["E_seconds"] = round((calls[edit].at - begin).total_seconds()) if edit is not None and begin else None
            look = [t for c in calls[:index] for t in c.tools if t["name"] == "Bash" and GIT_LOOK.search(t["arg"])]
            look_e = [t for c in calls[: (edit if edit is not None else len(calls))] for t in c.tools if t["name"] == "Bash" and GIT_LOOK.search(t["arg"])]
            record["git_looks"] = len(look)
            record["git_looks_before_edit"] = len(look_e)
            record["git_look_chars"] = sum(len(results.get(t["id"]) or "") for t in look)
            # What the git looks before the first productive action cost, in units: each
            # call that made one (all of it, an upper bound when it made other calls too),
            # and its results carried: written once at 2, read at w_cr by every later call.
            look_units = 0.0
            for position, c in enumerate(calls[:index]):
                mine = [t for t in c.tools if t["name"] == "Bash" and GIT_LOOK.search(t["arg"])]
                if not mine:
                    continue
                later = len(calls) - position - 1
                carried = sum(len(results.get(t["id"]) or "") for t in mine) / CHARS_PER_TOKEN_TOOL
                look_units += c.cost + carried * (T.W_CW1H + c.w_cr * max(0, later - 1))
            record["git_look_units"] = round(look_units)
            # The line's price on this start: its tokens written once and read by every call after.
            record["later_calls"] = max(0, len(calls) - 1)
            record["seg_units"] = round(sum(c.cost for c in calls))
            reads = [t for c in calls[:index] for t in c.tools if EKKO_READ.match(t["name"])]
            record["ekko_reads"] = len(reads)
            record["ekko_read_chars"] = sum(len(results.get(t["id"]) or "") for t in reads)
            record["tool_calls_before"] = sum(len(c.tools) for c in calls[:index])
            record["result_chars_before"] = sum(len(results.get(t["id"]) or "") for c in calls[:index] for t in c.tools)
            # The tree as the session found it: its first git status before any edit of its own.
            upto = edit if edit is not None else len(calls)
            status = [t for c in calls[:upto] for t in c.tools if t["name"] == "Bash" and re.search(r"\bgit\b[^|;&]*\bstatus\b", t["arg"])]
            if status:
                text = results.get(status[0]["id"]) or ""
                record["first_status_tracked"] = bool(re.search(r"(?m)^(\s?[MADRCU]{1,2}\s|Changes not staged|Changes to be committed)", text))
                record["first_status_untracked"] = bool(re.search(r"(?m)^(\?\? |Untracked files)", text))
            prompts = segment["prompts"][:6]
            hits = [p["text"][:200] for p in prompts if CORRECTION.search(p["text"])]
            record["corrections"] = hits
            record["first_text"] = next((t for c in calls for t in c.texts if t.strip()), "")[:600]
            record["answer_text"] = next(("\n".join(c.texts) for c in calls if c.stop == "end_turn" and any(t.strip() for t in c.texts)), "")[:3000]
            record["first_bash"] = [t["arg"][:160] for c in calls[:index + 1] for t in c.tools if t["name"] == "Bash"][:12]
            record["model"] = calls[0].model if calls else None
            record["n_prompts"] = len(segment["prompts"])
            records.append(record)
    out = sys.argv[1] if len(sys.argv) > 1 else os.path.join(C.DATA, "starts.jsonl")
    os.makedirs(os.path.dirname(os.path.abspath(out)), exist_ok=True)
    with open(out, "w") as handle:
        for record in records:
            handle.write(json.dumps(record, ensure_ascii=False) + "\n")
    print(f"{len(records)} starts with a handoff in the prime, from {len(paths)} transcripts in {', '.join(T.profiles())}")


if __name__ == "__main__":
    main()
