"""Task 2197, part B: the same resume, headless, under each way of telling a
session what moved since the handoff its prime shows.

  A  the prime as ekko prints it today: the control.
  B  the staleness line the reduced 1433 would print under the handoff:
     commits since it was written, releases, the tasks it names closed since;
     or, when nothing moved, that nothing did.
  C  Basic Memory's record (note 1474): the commit the handoff was written at,
     nothing more; the session may run git log itself.
  D  Context Language Models' resume note (1433's "what did not survive"):
     content-free, a warning to check before trusting the handoff.

Every arm runs the same plumbing: the user's plugins and hooks, plus one
SessionStart hook of the run's own that prints the arm's text, empty for A. A
scenario is a real start of this machine's transcripts: the repository cut at
the commit HEAD was on then, its tags of then, and the board as it stood then
(items made later left out, tasks finished later reopened), in a scratch
folder under ~/.cache/ekko-paired, a name the transcript studies skip. The
prompt is the most common first prompt of a resume, "onde paramos?". The
scenarios, and where the real repositories are, come from this machine's
target/evals/staleness/study.json (config.py, README.md).

    nix develop -c python3 evals/staleness/ab.py prepare
    nix develop -c python3 evals/staleness/ab.py run SCENARIO ARM [--rep N]
    nix develop -c python3 evals/staleness/ab.py queue PLAN [--dry-run]

Each run's stream (stream-json, hook events included) lands in
target/evals/staleness/runs/<run>/, with run.json: calls, units by the model's
own weights, git looks, ekko reads, seconds, the answer, what each hook printed,
and any read of the real repositories (a peek at the future).
"""

import argparse
import datetime
import glob
import json
import os
import random
import re
import shlex
import shutil
import subprocess
import sys
import time
import uuid

import config as C

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "claude-code"))
sys.path.insert(0, os.path.join(HERE, "..", "recall"))
sys.path.insert(0, os.path.join(HERE, ".."))
import boards  # noqa: E402
from transcripts import W_CW1H, W_CW5M, W_IN, W_OUT, profiles, read_weight  # noqa: E402

HOME = os.path.expanduser("~")
STUDY = C.study()
ROOT = C.SCRATCH
OUT = os.path.join(C.DATA, "runs")
LOCAL = datetime.timezone(datetime.timedelta(hours=-3))
# The Claude Code profile the runs log in with: its plugins and hooks are the user's.
PROFILE = os.path.expanduser(STUDY["profile"])
PROMPT = "onde paramos?"
EFFORT = "max"
MODEL = "opus"
BUDGET_USD = "4"
# The quota guard: no run starts past these shares of the account's windows,
# read off the last stream (rate_limit_event); the user's own work keeps the rest.
FIVE_HOUR_STOP, WEEK_STOP = 0.80, 0.66

# The replays of a past start: name -> (repository, board folder, start,
# handoff shown then, when it was written as that prime said, the transcript
# the start is from), each from target/evals/staleness/starts.jsonl; "s" names
# a stale one (something moved between the handoff and the start), "f" a
# fresh one. study.json's "scenarios".
SCENARIOS = {name: tuple(value) for name, value in STUDY["scenarios"].items()}
ARMS = ("A", "B", "C", "D")


DISALLOWED = [
    "mcp__plugin_ekko_ekko__ask", "mcp__plugin_ekko_ekko__wait", "mcp__plugin_ekko_ekko__away",
    "AskUserQuestion", "Artifact", "ArtifactComments", "ArtifactData", "CronCreate", "ScheduleWakeup",
    "RemoteTrigger", "PushNotification", "SendFeedback", "Monitor", "Agent",
    "Bash(gh:*)", "Bash(git push:*)", "Bash(kmux:*)", "Bash(ekko agents:*)", "Bash(nix:*)",
]
# The real repositories and the real transcripts hold the future: permission
# rules refuse them, bin/no-elsewhere refuses any tool call naming them
# (study.json's "guard", an extended regex), and PEEK finds a call that named
# them anyway, in the stream (study.json's "peek", a Python regex).
DENY = STUDY["deny"]
PEEK = re.compile(STUDY["peek"])
GIT_LOOK = re.compile(r"\bgit\b[^|;&]*?\b(log|status|diff|show|rev-list|describe|reflog|shortlog|branch|tag)\b")
EKKO_READ = re.compile(r"^mcp__.*ekko.*__(context|search|next|prime|changes|roadmap)$")
# What a refusal of bin/no-elsewhere says the run is: a replay of a past start,
# or a copy of the repository as it is now. A refusal is counted by the words
# both messages end with, which the study's first runs' message ends with too.
REPLAY = "This session replays a past moment in a copy of the repository"
NOW = "This session works in a copy of the repository as it is now"
REFUSED = "folders outside it are not available here"


def git(cwd, *args, check=True):
    return subprocess.run(["git", "-c", "log.showSignature=false", *args], cwd=cwd, check=check, capture_output=True, text=True).stdout.strip()


def now():
    return datetime.datetime.now(LOCAL)


def log(text):
    print(f"{now():%m-%d %H:%M:%S} {text}", flush=True)


# ---- a scenario's world --------------------------------------------------------------


def historical_prime(file_prefix, start_ms):
    """The prime the start really showed, from its transcript."""
    for base in profiles():
        for path in glob.glob(os.path.join(base, "projects", "*", file_prefix + "*.jsonl")):
            best = None
            with open(path, errors="replace") as handle:
                for raw in handle:
                    if "--prime" not in raw or "SessionStart" not in raw:
                        continue
                    try:
                        row = json.loads(raw)
                    except ValueError:
                        continue
                    attachment = row.get("attachment") or {}
                    if "--prime" not in (attachment.get("command") or ""):
                        continue
                    at = datetime.datetime.fromisoformat(row["timestamp"].replace("Z", "+00:00")).timestamp() * 1000
                    gap = abs(at - start_ms)
                    if best is None or gap < best[0]:
                        best = (gap, attachment.get("content") or attachment.get("stdout") or "")
            if best and best[0] < 5 * 60 * 1000:
                return best[1]
    return ""


def listed(prime, heading):
    """The ids a prime's section lists, by its heading: 'In progress', 'Ready'."""
    found = re.search(rf"(?m)^{heading}[^\n]*\n((?:[ \t]*\d+\.[^\n]*\n?|[ \t]+[^\n]*\n?)*)", prime)
    if not found:
        return set()
    return {int(n) for n in re.findall(r"(?m)^\s{0,8}(\d+)\.\s", found.group(1))}


def earlier_update(b, iid, start_ms):
    """The item's updatedAt in the last snapshot taken before start_ms, if any."""
    import starts as S
    times = [t for t, _ in b["snaps"]]
    for index in range(len(times) - 1, -1, -1):
        if times[index] * 1000 >= start_ms:
            continue
        then = S.snapshot_items(b, b["snaps"][index][1]).get(iid)
        if then is not None and int(then.get("updatedAt") or 0) < start_ms:
            return int(then.get("updatedAt") or 0)
        if then is None:
            return None
    return None


def snapshot(board, start_ms, dest, shown, prime, written_ms):
    """The board as it stood at start_ms: items made later left out; stashes,
    trashes and answers made later undone; each task's close placed by
    closed_at (its claim and last update, then the board's snapshots), and,
    where nothing settles it, by the nearest snapshot; what the real prime
    listed in progress set so; the handoff it showed made the current one on
    its task, as handoffs stood then. Text edited later cannot be recovered:
    returns the ids whose text may be newer."""
    sys.path.insert(0, HERE)
    import starts as S
    items = boards.read(board)
    b = S.board(board)
    moment = start_ms / 1000
    in_progress = listed(prime, "In progress")
    kept, newer, unsettled = {}, [], []
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
        iid = int(item["_id"])
        if item.get("_isTask"):
            closed = S.closed_at(b, iid, moment)
            if closed is None:
                unsettled.append(iid)
                times = [t for t, _ in b["snaps"]]
                if times:
                    nearest = min(range(len(times)), key=lambda k: abs(times[k] - moment))
                    then = S.snapshot_items(b, b["snaps"][nearest][1]).get(iid)
                    closed = bool(then) and S.state_of(then) in ("done", "cancelled")
                else:
                    closed = False
            if iid in in_progress:
                closed = False
            if not closed and (item.get("isComplete") or item.get("cancelled")):
                item.update(isComplete=False)
                for field in ("doneBy", "cancelled", "waiting"):
                    item.pop(field, None)
            if not closed:
                item["inProgress"] = iid in in_progress
        if int(item.get("updatedAt") or 0) >= start_ms:
            newer.append(iid)
            # No date after the start, and none made up: the last update the
            # board's snapshots saw before it, else the item's creation. (A
            # first try set them all to a minute before the start, and the
            # control run of s1 spent calls on "five tasks altered at 12:55:00".)
            item["updatedAt"] = earlier_update(b, iid, start_ms) or int(item.get("_timestamp") or 0)
        if iid == shown:
            item["updatedAt"] = written_ms
        kept[key] = item
    # Handoffs as they stood: a note is of kind handoff if flagged now or
    # superseded by one that is; the current one on a task is the one no kept
    # handoff supersedes.
    kinds = {item.get("uid") for item in items.values() if item.get("handoff")}
    grew = True
    while grew:
        grew = False
        for item in items.values():
            if item.get("uid") in kinds and item.get("supersedes") and item["supersedes"] not in kinds:
                kinds.add(item["supersedes"])
                grew = True
    superseded = {item.get("supersedes") for item in kept.values() if item.get("uid") in kinds}
    for item in kept.values():
        if item.get("uid") in kinds:
            if item["uid"] in superseded:
                item.pop("handoff", None)
            else:
                item["handoff"] = True
    os.makedirs(os.path.join(dest, "storage"))
    for folder in ("history", "archive"):
        os.makedirs(os.path.join(dest, folder))
    for name in ("project.json", "memory.md"):
        if os.path.exists(os.path.join(board, name)):
            shutil.copy(os.path.join(board, name), dest)
    counters = boards.counters(board)
    counters["highestId"] = max(int(item["_id"]) for item in kept.values())
    boards.seed(dest, kept, counters)
    return kept, newer, unsettled


def cited(text, exists):
    """Ids a handoff names: after a citing word, or bare when the board holds them
    (evals/recall/replay.py's cited_ids)."""
    from replay import cited_ids
    return cited_ids(text, exists)


def facts(repo, board, items, handoff_id, written_ms, start_ms):
    """What moved between the handoff and the start, as the arms' texts say it:
    commits and tags by date in the scenario's repository, and the tasks the
    handoff names that closed in between, by closed_at on the real board."""
    sys.path.insert(0, HERE)
    import starts as S
    b = S.board(board)
    item = next(i for i in items.values() if int(i["_id"]) == handoff_id)
    since = written_ms // 1000
    anchor = git(repo, "rev-list", "-1", f"--before=@{since}", "HEAD", check=False)
    head = git(repo, "rev-parse", "HEAD")
    commits = git(repo, "log", "--format=%h %s", f"--since=@{since}", "HEAD").splitlines()
    tags = [t for t in git(repo, "for-each-ref", "--sort=creatordate", "--format=%(creatordate:unix) %(refname:short)", "refs/tags").splitlines()
            if t and since < int(t.split()[0]) <= start_ms // 1000]
    exists = {int(i["_id"]) for i in items.values()}
    by_id = {int(i["_id"]): i for i in items.values()}
    own = {int(i["_id"]) for i in items.values() if i.get("uid") and i.get("uid") == item.get("attachedTo")}
    named = sorted(i for i in cited(item.get("description") or "", exists) | own if i in by_id and by_id[i].get("_isTask"))
    closed, unsettled = [], []
    for i in named:
        before, after = S.closed_at(b, i, written_ms / 1000), S.closed_at(b, i, start_ms / 1000)
        if before is False and after is True:
            closed.append(i)
        elif None in (before, after) and after is not False and before is not True:
            unsettled.append(i)
    return {
        "handoff": handoff_id, "written_ms": written_ms, "written": datetime.datetime.fromtimestamp(written_ms / 1000, LOCAL).strftime("%Y-%m-%d %H:%M"),
        "anchor": anchor[:7], "head": head[:7], "branch": "main", "commits": commits, "tags": [t.split()[1] for t in tags],
        "named": named, "closed": closed, "unsettled": unsettled,
    }


def arm_text(arm, f):
    h, at = f"{f['handoff']} (under \"Where the last session stopped\")", f"{f['anchor']} on {f['branch']}"
    moved = f["commits"] or f["tags"] or f["closed"]
    if arm == "A":
        return ""
    if arm == "B":
        dirty = f.get("dirty") or 0
        tree = f"; the tree holds {dirty} changed file{'s' if dirty != 1 else ''} not committed" if dirty else ""
        if not f.get("has_commits", True):
            tasks = ("of the tasks it names, closed since: " + ", ".join(map(str, f["closed"]))) if f["closed"] else "the tasks it names are as it left them"
            return f"ekko, on handoff {h}: the repository has no commit to anchor it to; {tasks}{tree}.\n"
        if not moved:
            return f"ekko, on handoff {h}: nothing moved since it was written, at {at}: HEAD is still {f['head']}, and the tasks it names are as it left them{tree}.\n"
        n = len(f["commits"])
        parts = [f"{n} commit{'s' if n != 1 else ''}"] if n else ["no commits"]
        if len(f["tags"]) == 1:
            parts.append(f"released {f['tags'][0]}")
        elif f["tags"]:
            parts.append(f"released {f['tags'][0]} to {f['tags'][-1]}")
        text = f"ekko, on handoff {h}: since it was written, at {at}: " + ", ".join(parts)
        if f["closed"]:
            text += "; of the tasks it names, closed since: " + ", ".join(map(str, f["closed"]))
        return text + tree + ".\n"
    if arm == "C":
        if not f.get("has_commits", True):
            return f"ekko, on handoff {h}: it was written when the repository had no commit.\n"
        return f"ekko, on handoff {h}: it was written at commit {f['anchor']} on {f['branch']}.\n"
    if arm == "D":
        return (f"ekko, on handoff {h}: the session that wrote it has ended, and what it ran in the background -- "
                "a build, a test, a subagent -- ended with it; check the folder and the board before trusting its work in progress.\n")
    raise ValueError(arm)


# The study's own items, left out of a copy of its board: task 2197's text
# names what moved since handoff 1826, and 2199 says 1826 is stale, so a run
# that reads them is told the answer (seen in the first l-ekko and x-ekko runs).
# The board's history snapshots hold them too: x-ekko-D-1-1019-155943 read
# .ekko/history and found 2197's title, so the snapshots are scrubbed as well
# (done by hand on both ekko templates at 17:02, before l-ekko's C and D ran).
# study.json's "study_items": repository -> the ids to leave out.
STUDY_ITEMS = {repo: tuple(ids) for repo, ids in STUDY["study_items"].items()}


def scrub_study(src, repo):
    """Takes the study's items, and the notes attached to them, out of the
    copy's board, its database or JSON and the versions history/ kept alike
    (boards.remove)."""
    drop = STUDY_ITEMS.get(src)
    if not drop:
        return []
    board = os.path.join(repo, ".ekko")
    items = boards.read(board)
    uids = {it["uid"] for it in items.values() if it.get("_id") in drop}
    gone = sorted(int(k) for k, it in items.items() if it.get("_id") in drop or it.get("attachedTo") in uids)
    boards.remove(board, set(gone) | set(drop))
    return gone


# Live scenarios: the project as it is now, copied whole -- working tree,
# uncommitted files, branches, remote-tracking refs and board -- so the world
# the run meets is the present one and nothing on the machine is from its
# future. The handoff is whatever the board's prime shows now. study.json's
# "live": name -> repository.
LIVE = dict(STUDY["live"])
EXCLUDE = ["/target", "/.claude/worktrees", "/node_modules", "/result", "/.direnv"]


def live_facts(repo, prime):
    sys.path.insert(0, HERE)
    import starts as S
    m = S.HEADER.search(prime)
    if not m:
        raise SystemExit("no handoff in the prime")
    handoff_id, task_id, task_state, written, _ = m.groups()
    b = S.board(os.path.join(repo, ".ekko"))
    # The prime gives the minute; the board gives the millisecond. A task closed
    # in the same minute before the handoff was written is not news to it.
    minute = int(datetime.datetime.strptime(written, "%Y-%m-%d %H:%M").replace(tzinfo=LOCAL).timestamp() * 1000)
    exact = b["items"][int(handoff_id)].get("updatedAt") or b["items"][int(handoff_id)].get("_timestamp")
    written_ms = exact if minute <= exact < minute + 60000 else minute + 59999
    since, now_s = written_ms // 1000 + 1, int(time.time())
    body = "\n".join(line[6:] for line in prime[m.end():].splitlines()[1:] if line.startswith("    > "))
    exists = set(b["items"])
    named = sorted(i for i in cited(body, exists) | {int(task_id)} if i in b["items"] and b["items"][i].get("_isTask"))
    closed = [i for i in named if S.closed_at(b, i, written_ms / 1000) is False and S.closed_at(b, i, now_s) is True]
    has_commits = bool(git(repo, "rev-parse", "--verify", "-q", "HEAD", check=False))
    anchor = git(repo, "rev-list", "-1", f"--before=@{since}", "HEAD", check=False) if has_commits else ""
    head = git(repo, "rev-parse", "HEAD", check=False) if has_commits else ""
    commits = git(repo, "log", "--format=%h %s", f"--since=@{since}", "HEAD", check=False).splitlines() if has_commits else []
    tags = [t for t in git(repo, "for-each-ref", "--sort=creatordate", "--format=%(creatordate:unix) %(refname:short)", "refs/tags").splitlines()
            if t and int(t.split()[0]) > since]
    branch = git(repo, "rev-parse", "--abbrev-ref", "HEAD", check=False) or "main"
    dirty = len([l for l in git(repo, "status", "--porcelain", "--untracked-files=no", check=False).splitlines() if l.strip()])
    return {
        "handoff": int(handoff_id), "task": int(task_id), "written_ms": written_ms, "written": written,
        "anchor": (anchor or "none")[:7], "head": (head or "none")[:7], "branch": branch, "commits": commits,
        "tags": [t.split()[1] for t in tags], "named": named, "closed": closed, "dirty": dirty, "has_commits": has_commits,
    }


def prepare_live(name):
    src = LIVE[name]
    base = os.path.join(ROOT, "template", name)
    if os.path.exists(base):
        shutil.rmtree(base)
    repo = os.path.join(base, "repo")
    os.makedirs(repo)
    subprocess.run(["rsync", "-a", *[f"--exclude={e}" for e in EXCLUDE], src + "/", repo + "/"], check=True)
    scrub_study(src, repo)
    # The copy's own board, not a registered one; no later write reaches the real board.
    prime = subprocess.run(["ekko", "--prime"], cwd=repo, capture_output=True, text=True, env=child_env(base)).stdout
    real = subprocess.run(["ekko", "--prime"], cwd=src, capture_output=True, text=True).stdout
    with open(os.path.join(base, "prime.txt"), "w") as handle:
        handle.write(prime)
    with open(os.path.join(base, "prime.real.txt"), "w") as handle:
        handle.write(real)
    f = live_facts(repo, prime)
    f.update(scenario=name, start=now().isoformat(timespec="seconds"), live=True)
    texts = {arm: arm_text(arm, f) for arm in ARMS}
    with open(os.path.join(base, "facts.json"), "w") as handle:
        json.dump({**f, "texts": texts}, handle, indent=2)
    ids = lambda text: {int(n) for n in re.findall(r"(?m)^\s{0,8}(\d+)\.\s", text)}
    a, r = ids(prime), ids(real)
    log(f"{name}: copied {src}; prime shows handoff {f['handoff']} on {f['task']}; ids listed: copy {len(a)}, real {len(r)}, "
        f"Jaccard {len(a & r) / max(1, len(a | r)):.2f}; moved: {len(f['commits'])} commits, tags {f['tags']}, closed {f['closed']}, dirty {f['dirty']}")
    for arm in ARMS:
        log(f"   {arm}: {texts[arm].strip()!r}")
    return base


# Old-handoff scenarios: the project as it is now, copied whole, with the
# handoffs written after an older one dropped from the copy's board -- as if
# the sessions since had worked without writing one. The world is the
# present, so nothing on the machine is from its future; the handoff is old.
# study.json's "old": name -> (repository, the handoff to keep). The prime
# shows only a handoff on open work: each must sit on a task still open.
OLD = {name: tuple(value) for name, value in STUDY["old"].items()}


def prepare_old(name):
    src, keep = OLD[name]
    base = os.path.join(ROOT, "template", name)
    if os.path.exists(base):
        shutil.rmtree(base)
    repo = os.path.join(base, "repo")
    os.makedirs(repo)
    subprocess.run(["rsync", "-a", *[f"--exclude={e}" for e in EXCLUDE], src + "/", repo + "/"], check=True)
    # rsync copied the database as a file, which can leave out what its log
    # holds: it is copied again through SQLite (task 2233).
    boards.copy_database(os.path.join(src, ".ekko"), os.path.join(repo, ".ekko"))
    scrub_study(src, repo)
    items = boards.read(os.path.join(repo, ".ekko"))
    since = items[str(keep)]["_timestamp"]
    dropped = sorted(int(k) for k, it in items.items()
                     if not it.get("_isTask") and it.get("handoff") is not None and (it.get("_timestamp") or 0) > since)
    # A hard link shares the real board's inode: boards.remove replaces each
    # file, never writing through it.
    boards.remove(os.path.join(repo, ".ekko"), dropped)
    prime = subprocess.run(["ekko", "--prime"], cwd=repo, capture_output=True, text=True, env=child_env(base)).stdout
    with open(os.path.join(base, "prime.txt"), "w") as handle:
        handle.write(prime)
    shown = re.search(r"Where the last session stopped: handoff (\d+)", prime)
    if not shown or int(shown.group(1)) != keep:
        raise SystemExit(f"{name}: the prime shows handoff {shown.group(1) if shown else None}, not {keep}")
    f = live_facts(repo, prime)
    f.update(scenario=name, start=now().isoformat(timespec="seconds"), live=True, old_handoff=keep, dropped_handoffs=dropped)
    texts = {arm: arm_text(arm, f) for arm in ARMS}
    with open(os.path.join(base, "facts.json"), "w") as handle:
        json.dump({**f, "texts": texts}, handle, indent=2)
    log(f"{name}: copied {src}, dropped handoffs {dropped}; prime shows {keep}; moved: {len(f['commits'])} commits, tags {f['tags']}, "
        f"closed {f['closed']}, dirty {f['dirty']}")
    for arm in ARMS:
        log(f"   {arm}: {texts[arm].strip()!r}")
    return base


def prepare(name):
    repo_src, board_src, start, handoff_id, written, file_prefix = SCENARIOS[name]
    start_ms = int(datetime.datetime.fromisoformat(start).timestamp() * 1000)
    written_ms = int(datetime.datetime.fromisoformat(written).timestamp() * 1000)
    base = os.path.join(ROOT, "template", name)
    if os.path.exists(base):
        shutil.rmtree(base)
    repo = os.path.join(base, "repo")
    os.makedirs(repo)
    commit = git(repo_src, "rev-list", "-1", f"--before=@{start_ms // 1000}", "main")
    git(repo, "init", "-q", "-b", "main")
    git(repo, "fetch", "-q", "--no-tags", repo_src, commit)
    git(repo, "reset", "-q", "--hard", "FETCH_HEAD")
    for line in git(repo_src, "for-each-ref", "--format=%(creatordate:unix) %(refname:short) %(objectname)", "refs/tags").splitlines():
        when_, tag, obj = line.split()
        if int(when_) <= start_ms // 1000:
            git(repo, "fetch", "-q", "--no-tags", repo_src, f"refs/tags/{tag}:refs/tags/{tag}", check=False)
    with open(os.path.join(repo, ".git", "info", "exclude"), "a") as handle:
        handle.write("/.ekko/\n/.claude/\n")
    real = historical_prime(file_prefix, start_ms)
    with open(os.path.join(base, "prime.real.txt"), "w") as handle:
        handle.write(real)
    items, newer, unsettled = snapshot(board_src, start_ms, os.path.join(repo, ".ekko"), handoff_id, real, written_ms)
    f = facts(repo, board_src, items, handoff_id, written_ms, start_ms)
    f.update(scenario=name, start=start, commit=commit[:7], board_items=len(items), text_maybe_newer=len(newer), close_unsettled=len(unsettled))
    texts = {arm: arm_text(arm, f) for arm in ARMS}
    with open(os.path.join(base, "facts.json"), "w") as handle:
        json.dump({**f, "texts": texts}, handle, indent=2)
    prime = subprocess.run(["ekko", "--prime"], cwd=repo, capture_output=True, text=True, env=child_env(base)).stdout
    with open(os.path.join(base, "prime.txt"), "w") as handle:
        handle.write(prime)
    shown = re.search(r"Where the last session stopped: handoff (\d+)", prime)
    # The check: the rebuilt prime against the real one, by the ids each lists.
    ids = lambda text: {int(n) for n in re.findall(r"(?m)^\s{0,8}(\d+)\.\s", text)}
    a, r = ids(prime), ids(real)
    overlap = len(a & r) / max(1, len(a | r))
    log(f"{name}: repo at {commit[:7]}, {len(items)} items ({len(unsettled)} closes unsettled), prime shows handoff "
        f"{shown.group(1) if shown else None} (expected {handoff_id}); ids listed: rebuilt {len(a)}, real {len(r)}, Jaccard {overlap:.2f}; "
        f"moved: {len(f['commits'])} commits, tags {f['tags']}, closed {f['closed']}, unsettled {f['unsettled']}")
    for arm in ARMS:
        log(f"   {arm}: {texts[arm].strip()!r}")
    return base


# ---- a run ----------------------------------------------------------------------------


def child_env(run_dir):
    env = {k: v for k, v in os.environ.items() if not k.startswith("CLAUDE")}
    for key in ("EKKO_DIR", "EKKO_PROJECT"):
        env.pop(key, None)
    state = os.path.join(run_dir, "state")
    os.makedirs(state, exist_ok=True)
    env.update(
        CLAUDE_CONFIG_DIR=PROFILE,
        EKKO_HOME=os.path.join(state, "ekko-home"),
        XDG_STATE_HOME=os.path.join(state, "xdg"),
        CTX_STATE=os.path.join(state, "ctx"),
        CTX_HANDOFF_TOKENS="1000000000",
        CTX_COLD_TOKENS="0",
    )
    return env


def settings(run_dir, extra_path, why=REPLAY):
    wrapper = open(os.path.realpath(shutil.which("claude"))).read()
    found = re.search(r"--settings (\S+)", wrapper)
    base = json.load(open(found.group(1))) if found else {}
    base.pop("statusLine", None)
    permissions = base.setdefault("permissions", {})
    permissions["deny"] = list(dict.fromkeys(permissions.get("deny", []) + DENY))
    base.setdefault("hooks", {}).setdefault("SessionStart", []).append(
        {"hooks": [{"type": "command", "command": f"sleep 1; cat {extra_path}"}]})
    # The real folders hold the future: refuse any call that names them (bin/no-elsewhere).
    # The run's own transcript folder is let through: Claude Code names it after
    # the run's folder, every character not a letter or digit made a dash.
    own = "projects/" + re.sub(r"[^A-Za-z0-9]", "-", os.path.dirname(ROOT))
    guard = (f"NO_ELSEWHERE={shlex.quote(STUDY['guard'])} NO_ELSEWHERE_OWN={shlex.quote(own)} NO_ELSEWHERE_WHY={shlex.quote(why)} "
             + shlex.quote(os.path.join(HERE, "bin", "no-elsewhere")))
    base["hooks"].setdefault("PreToolUse", []).append({"matcher": "*", "hooks": [{"type": "command", "command": guard}]})
    path = os.path.join(run_dir, "settings.json")
    with open(path, "w") as handle:
        json.dump(base, handle, indent=2)
    return path


def windows(lines):
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


def measure(stream_path):
    """What the stream says the session did."""
    calls, tools, results, hooks, result, model = {}, [], {}, [], {}, None
    with open(stream_path, errors="replace") as handle:
        lines = handle.readlines()
    for raw in lines:
        try:
            row = json.loads(raw)
        except ValueError:
            continue
        kind = row.get("type")
        if kind == "assistant":
            message = row.get("message") or {}
            mid = message.get("id")
            usage = message.get("usage") or {}
            model = message.get("model") or model
            if mid and mid not in calls:
                split = usage.get("cache_creation") or {}
                c1, c5 = split.get("ephemeral_1h_input_tokens"), split.get("ephemeral_5m_input_tokens")
                if c1 is None and c5 is None:
                    c1, c5 = usage.get("cache_creation_input_tokens") or 0, 0
                calls[mid] = {"inp": usage.get("input_tokens") or 0, "cw1h": c1 or 0, "cw5m": c5 or 0,
                              "cr": usage.get("cache_read_input_tokens") or 0, "out": usage.get("output_tokens") or 0,
                              "model": message.get("model")}
            elif mid and usage.get("output_tokens"):
                calls[mid]["out"] = max(calls[mid]["out"], usage.get("output_tokens") or 0)
            for block in message.get("content") or []:
                if isinstance(block, dict) and block.get("type") == "tool_use":
                    tools.append({"id": block.get("id"), "name": block.get("name"), "input": block.get("input")})
        elif kind == "user":
            for block in (row.get("message") or {}).get("content") or []:
                if isinstance(block, dict) and block.get("type") == "tool_result":
                    content = block.get("content")
                    text = content if isinstance(content, str) else "\n".join(
                        part.get("text", "") for part in content or [] if isinstance(part, dict))
                    results[block.get("tool_use_id")] = text
        elif kind == "system" and row.get("subtype", "").startswith("hook"):
            hooks.append({k: row.get(k) for k in ("subtype", "hook_name", "hook_event", "output", "stdout", "exit_code") if k in row})
        elif kind == "result":
            result = row
    # The stream's per-message usage carries the output count from the start of
    # streaming (4 to 8 tokens), never the final one: the run's own totals, in
    # the result event, give the output and its thinking. Inputs from either agree.
    total = (result or {}).get("usage") or {}
    split = total.get("cache_creation") or {}
    if total:
        units = (W_IN * (total.get("input_tokens") or 0)
                 + W_CW1H * (split.get("ephemeral_1h_input_tokens") or 0) + W_CW5M * (split.get("ephemeral_5m_input_tokens") or 0)
                 + read_weight(model) * (total.get("cache_read_input_tokens") or 0) + W_OUT * (total.get("output_tokens") or 0))
    else:
        units = sum(W_IN * c["inp"] + W_CW1H * c["cw1h"] + W_CW5M * c["cw5m"] + read_weight(c["model"]) * c["cr"] + W_OUT * c["out"] for c in calls.values())
    output_tokens = total.get("output_tokens")
    thinking_tokens = (total.get("output_tokens_details") or {}).get("thinking_tokens")
    first = next(iter(calls.values()), None)
    # As if no earlier run had warmed the cache: the first call's reads written instead.
    units_cold = units + ((W_CW1H - read_weight(first["model"])) * first["cr"] if first else 0)
    version = next((m.group(1) for m in (re.search(r'"claude_code_version":"([^"]+)"', raw) for raw in lines) if m), None)
    def command(t):
        return (t["input"] or {}).get("command") or "" if t["name"] == "Bash" else json.dumps(t["input"] or {})
    looks = [t for t in tools if t["name"] == "Bash" and GIT_LOOK.search(command(t))]
    reads = [t for t in tools if EKKO_READ.match(t["name"] or "")]
    peeks = [command(t)[:200] for t in tools if PEEK.search(command(t)) and REFUSED not in (results.get(t["id"]) or "")]
    refused = sum(1 for t in tools if REFUSED in (results.get(t["id"]) or ""))
    return {
        "model": model, "claude_code_version": version, "calls": len(calls), "units": round(units), "units_cold": round(units_cold),
        "first_call": first, "output_tokens": output_tokens, "thinking_tokens": thinking_tokens,
        "units_output": round(W_OUT * (output_tokens or 0)), "tool_calls": len(tools),
        "git_looks": len(looks), "git_look_commands": [command(t)[:160] for t in looks],
        "git_look_chars": sum(len(results.get(t["id"]) or "") for t in looks),
        "ekko_reads": len(reads), "ekko_read_chars": sum(len(results.get(t["id"]) or "") for t in reads),
        "tools": [t["name"] for t in tools], "result_chars": sum(len(v) for v in results.values()),
        "num_turns": result.get("num_turns"), "duration_ms": result.get("duration_ms"), "duration_api_ms": result.get("duration_api_ms"),
        "cost_usd": result.get("total_cost_usd"), "is_error": result.get("is_error"), "subtype": result.get("subtype"),
        "answer": result.get("result") or "", "hooks": hooks, "peeks": peeks, "refused_elsewhere": refused, "windows": windows(lines),
    }


def run(name, arm, rep, prompt=None, effort=None):
    prompt, effort = prompt or PROMPT, effort or EFFORT
    template = os.path.join(ROOT, "template", name)
    if not os.path.exists(template):
        prepare_live(name) if name in LIVE else prepare_old(name) if name in OLD else prepare(name)
    stamp = now().strftime("%m%d-%H%M%S")
    run_id = f"{name}-{arm}-{rep}-{stamp}"
    run_dir = os.path.join(ROOT, "runs", run_id)
    out_dir = os.path.join(OUT, run_id)
    os.makedirs(out_dir)
    os.makedirs(os.path.dirname(run_dir), exist_ok=True)
    subprocess.run(["cp", "-a", "--reflink=auto", template, run_dir], check=True)
    facts_ = json.load(open(os.path.join(run_dir, "facts.json")))
    extra = os.path.join(run_dir, "extra.txt")
    with open(extra, "w") as handle:
        handle.write(facts_["texts"][arm])
    settings_path = settings(run_dir, extra, REPLAY if name in SCENARIOS else NOW)
    env = child_env(run_dir)
    sid = str(uuid.uuid4())
    command = ["claude", "-p", prompt, "--output-format", "stream-json", "--verbose", "--include-hook-events",
               "--session-id", sid, "--model", MODEL, "--effort", effort, "--settings", settings_path,
               "--max-budget-usd", BUDGET_USD, "--no-session-persistence", "--disallowedTools", *DISALLOWED]
    stream = os.path.join(out_dir, "stream.jsonl")
    log(f"{run_id}: start")
    began = time.time()
    with open(stream, "w") as out:
        proc = subprocess.run(command, cwd=os.path.join(run_dir, "repo"), env=env, stdout=out, stderr=subprocess.STDOUT,
                              stdin=subprocess.DEVNULL, timeout=1800)
    record = {"run": run_id, "scenario": name, "arm": arm, "rep": rep, "sid": sid, "prompt": prompt, "effort": effort,
              "model_asked": MODEL, "exit": proc.returncode, "seconds": round(time.time() - began, 1),
              "text": facts_["texts"][arm], "started": datetime.datetime.fromtimestamp(began, LOCAL).isoformat(timespec="seconds"),
              "command": command, **measure(stream)}
    with open(os.path.join(out_dir, "run.json"), "w") as handle:
        json.dump(record, handle, indent=2, ensure_ascii=False)
    five = record["windows"].get("five_hour", (None,))[0]
    week = record["windows"].get("seven_day", (None,))[0]
    log(f"{run_id}: {record['calls']} calls, {record['units'] / 1e3:.0f}k units, ${record['cost_usd']}, {record['seconds']} s, "
        f"git looks {record['git_looks']}, ekko reads {record['ekko_reads']}, peeks {len(record['peeks'])}; windows 5h {five} week {week}")
    shutil.rmtree(run_dir, ignore_errors=True)
    return record


def queue(plan, dry_run):
    """plan: 'scenario:arms:reps,...', e.g. 's1:AB:1,f1:AB:1'. Arms are shuffled
    within each scenario and repetition, seeded, and the scenarios interleaved."""
    rng = random.Random(2197)
    batches = []
    for part in plan.split(","):
        name, arms, reps = part.split(":")
        for rep in range(1, int(reps) + 1):
            order = list(arms)
            rng.shuffle(order)
            batches.append([(name, arm, rep) for arm in order])
    items = [item for batch in batches for item in batch]
    # A queue run again goes on where the last stopped: a cell with a run.json is done.
    done = {tuple(os.path.basename(os.path.dirname(p)).rsplit("-", 2)[0].rsplit("-", 2)) for p in glob.glob(os.path.join(OUT, "*", "run.json"))}
    skipped = [(n, a, r) for n, a, r in items if (n, a, str(r)) in done]
    items = [(n, a, r) for n, a, r in items if (n, a, str(r)) not in done]
    if skipped:
        log("already run: " + ", ".join(f"{n}/{a}/{r}" for n, a, r in skipped))
    log("queue: " + ", ".join(f"{n}/{a}/{r}" for n, a, r in items))
    if dry_run:
        return
    for name, arm, rep in items:
        done = glob.glob(os.path.join(OUT, "*", "run.json"))
        if done:
            try:
                windows_ = json.load(open(max(done, key=os.path.getmtime))).get("windows") or {}
            except (OSError, ValueError):
                windows_ = {}
            five, five_reset = (windows_.get("five_hour") or [0, 0])[:2]
            week = (windows_.get("seven_day") or [0])[0] or 0
            if week >= WEEK_STOP:
                log(f"stopped by the quota guard: week {week:.0%}")
                return
            if (five or 0) >= FIVE_HOUR_STOP:
                wait = (five_reset or 0) - time.time() + 60
                if not 0 < wait <= 3600:
                    log(f"stopped by the quota guard: 5h {five:.0%}, resets in {wait / 60:.0f} min")
                    return
                log(f"5h at {five:.0%}: waiting {wait / 60:.0f} min for its reset")
                time.sleep(wait)
        run(name, arm, rep)


def main():
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="command", required=True)
    p = sub.add_parser("prepare")
    p.add_argument("names", nargs="*")
    r = sub.add_parser("run")
    r.add_argument("scenario", choices=sorted(SCENARIOS) + sorted(LIVE) + sorted(OLD))
    r.add_argument("arm", choices=ARMS)
    r.add_argument("--rep", type=int, default=1)
    r.add_argument("--prompt")
    r.add_argument("--effort")
    sub.add_parser("remeasure")
    q = sub.add_parser("queue")
    q.add_argument("plan")
    q.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()
    if args.command == "prepare":
        for name in args.names or sorted(SCENARIOS):
            prepare_live(name) if name in LIVE else prepare_old(name) if name in OLD else prepare(name)
    elif args.command == "remeasure":
        for path in sorted(glob.glob(os.path.join(OUT, "*", "run.json")) + glob.glob(os.path.join(OUT, "contaminated", "*", "run.json"))):
            record = json.load(open(path))
            fresh = measure(os.path.join(os.path.dirname(path), "stream.jsonl"))
            record.setdefault("units_stream_v1", record.get("units"))
            record.update(fresh)
            # Keep the file's time: the queue's quota guard reads the newest run by it.
            stat = os.stat(path)
            with open(path, "w") as handle:
                json.dump(record, handle, indent=2, ensure_ascii=False)
            os.utime(path, ns=(stat.st_atime_ns, stat.st_mtime_ns))
            log(f"{record['run']}: units {record['units_stream_v1']:,} -> {record['units']:,} (output {record['units_output']:,})")
    elif args.command == "run":
        run(args.scenario, args.arm, args.rep, args.prompt, args.effort)
    else:
        queue(args.plan, args.dry_run)


if __name__ == "__main__":
    main()
