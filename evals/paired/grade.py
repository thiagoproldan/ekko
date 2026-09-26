"""Grades the runs of the paired test (evals/paired/harness.py; note 549).

  validate  the hidden tests against each task's reference commit, which must
            pass them, and its parent: a check the parent passes too only
            guards what was there before.
  checks    the repo's own checks on each result (cargo test, clippy; not
            rustfmt, which the project does not use: 975 hunks at 397's parent),
            and the hidden tests on the binary it built: the result's own
            ekko driven over the CLI and MCP, on what the task's text fixes.
  judge     a blind judge: a fresh `claude -p` at max with no tools, no MCP
            servers, no hooks and no settings, given the task's text, a
            checklist it wrote from that text alone before seeing any answer,
            the reference diff as one accepted answer, and the runs' diffs as
            X, Y, ... in random order; twice, the second time in reverse.
  report    units, calls and wall time per run, thinking and growth per call
            by effort, R and x per reset, and the quality columns.

    nix develop -c python3 evals/paired/grade.py validate|judge [TASK...]
    nix develop -c python3 evals/paired/grade.py checks [RUN...]
    nix develop -c python3 evals/paired/grade.py report
"""

import argparse
import datetime
import glob
import json
import os
import random
import re
import shutil
import statistics
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import harness  # noqa: E402
from harness import OUT, POINTS, REPO, ROOT, TASKS, git, log, units  # noqa: E402

JUDGE = os.path.join(OUT, "judge")
HIDDEN_OUT = os.path.join(OUT, "hidden")
MODEL = "claude-opus-5-5"  # the model every cell ran
EDIT_TOOLS = {"Edit", "Write", "MultiEdit", "NotebookEdit"}
# The interactive sessions that did the tasks first, read off the transcripts
# on 2026-09-24 (note 549): calls and units, subagents included.
ORIGINAL = {"394": (19, 630_000), "397": (36, 1_000_000), "396": (77, 3_000_000)}


def records():
    """Every finished run's record, the probes left out, oldest first."""
    found = []
    for path in glob.glob(os.path.join(OUT, "runs", "*", "run.json")):
        record = json.load(open(path))
        if record["task"] in TASKS:
            found.append(record)
    return sorted(found, key=lambda record: record["started"])


def cargo(repo, *args):
    # The devshell that runs this script may point cargo at the real repo's
    # target folder.
    env = {k: v for k, v in os.environ.items() if k != "CARGO_TARGET_DIR"}
    return subprocess.run(["nix", "develop", "-c", "cargo", *args], cwd=repo, env=env, capture_output=True, text=True,
                          timeout=1800, stdin=subprocess.DEVNULL)


# ---- hidden tests ---------------------------------------------------------------

ANSI = re.compile(r"\x1b\[[0-9;]*[A-Za-z]")
LISTED = re.compile(r"(?m)^\s*(\d+)\.\s")


def board_home():
    home = tempfile.mkdtemp(prefix="ekko-paired-hidden-")
    os.makedirs(os.path.join(home, ".ekko", "storage"))
    return home


def ekko_env(home, folder=None, profile=None):
    """ekko's environment on the board at `home` or, given `folder`, on the
    project found from there, as Claude Code starts a session in a project's
    folder. `profile` names the Claude Code profile a session runs under."""
    env = {k: v for k, v in os.environ.items() if not k.startswith(("EKKO", "CLAUDE", "GIT_"))}
    env.update(HOME=home, EKKO_TERMINAL="none", GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL="/dev/null")
    if not folder:
        env["EKKO_DIR"] = home
    if profile:
        env["CLAUDE_CONFIG_DIR"] = os.path.join(home, ".claude-" + profile)
    return env


def mcp(binary, home, calls, folder=None, profile=None):
    """Each call's reply over `ekko --mcp`, spoken to as tests/mcp.rs speaks to
    it: every line in, stdin closed, the replies read back by id. A reply is
    its text, or the error it carried."""
    lines = [json.dumps({"jsonrpc": "2.0", "id": 0, "method": "initialize",
                         "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "grade", "version": "0"}}}),
             json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"})]
    for index, (tool, arguments) in enumerate(calls, 1):
        lines.append(json.dumps({"jsonrpc": "2.0", "id": index, "method": "tools/call", "params": {"name": tool, "arguments": arguments}}))
    out = subprocess.run([binary, "--mcp"], input="\n".join(lines) + "\n", env=ekko_env(home, folder, profile), cwd=folder or home,
                         capture_output=True, text=True, timeout=120).stdout
    replies = {}
    for raw in out.splitlines():
        try:
            reply = json.loads(raw)
        except ValueError:
            continue
        if "id" in reply:
            result = reply.get("result") or {}
            text = "\n".join(block.get("text", "") for block in result.get("content") or [] if isinstance(block, dict))
            replies[reply["id"]] = ANSI.sub("", text) or json.dumps(reply.get("error") or result)
    return [replies.get(index, "") for index in range(1, len(calls) + 1)]


def cli(binary, home, *args, folder=None):
    done = subprocess.run([binary, *args], env=ekko_env(home, folder), cwd=folder or home, capture_output=True, text=True,
                          timeout=60)
    return ANSI.sub("", done.stdout + done.stderr)


def listed(text):
    return {int(n) for n in LISTED.findall(text)}


def hidden_397(binary):
    """Task 397's text fixes a stashed filter, and a line saying how many
    stashed items match when a search finds nothing or little. The board is
    the reference's own test's (e01e6d1): two tasks and a note stashed, one
    of them then trashed."""
    home = board_home()
    replies = mcp(binary, home, [
        ("create", {"text": "tab completion for ids"}),
        ("create", {"text": "tab completion for projects and phases"}),
        ("create", {"text": "phases are declared in order", "kind": "note"}),
        ("create", {"text": "tab completion for phases, thrown away"}),
        ("stash", {"items": [2, 3, 4]}),
        ("trash", {"items": [4]}),
        ("search", {"filters": ["stashed"]}),
        ("search", {"text": "completion phases", "filters": ["stashed"]}),
        ("search", {"text": "phases"}),
    ])
    listing = cli(binary, home, "--list", "stashed")
    shutil.rmtree(home, ignore_errors=True)
    counted = [line for line in replies[8].splitlines() if re.search(r"stash", line, re.I) and re.search(r"\b(2|two|both)\b", line, re.I)]
    return [
        ("search's stashed filter lists the stash, the trash left out", {2, 3} <= listed(replies[6]) and 4 not in listed(replies[6]), replies[6]),
        ("a text search with the stashed filter reaches a stashed task", 2 in listed(replies[7]) and 4 not in listed(replies[7]), replies[7]),
        ("a search that finds nothing says how many stashed items match", bool(counted), replies[8]),
        ("--list stashed lists the stash, the trash left out", {2, 3} <= listed(listing) and 4 not in listed(listing), listing),
    ]


def hidden_394(binary):
    """Task 394's text fixes a notice naming the item when a create's or an
    edit's text in a batch holds $N for an N the batch defines, and the text
    kept as written. Ten items first, so that the batch's own are 11 and on
    and a notice naming one cannot be taken for the $N it explains."""
    home = board_home()
    replies = mcp(binary, home, [("create", {"text": f"filler {n}"}) for n in range(1, 11)] + [
        ("batch", {"ops": [{"op": "create", "text": "the base task"},
                           {"op": "create", "text": "see $1 before starting", "kind": "note"}]}),
        ("batch", {"ops": [{"op": "create", "text": "another task"},
                           {"op": "edit", "item": 12, "append": " then $1"}]}),
        ("batch", {"ops": [{"op": "create", "text": "costs $5 today"}]}),
        ("context", {"item": 12}),
    ])[10:]
    shutil.rmtree(home, ignore_errors=True)

    def names(reply, item):
        return any("$1" in line and re.search(rf"(?<![\w$]){item}\b", line) for line in reply.splitlines())

    stray = [line for line in replies[2].splitlines() if "$5" in line and "costs $5 today" not in line]
    return [
        ("a create's text holding $1 gets a notice naming item 11", names(replies[0], 11), replies[0]),
        ("an edit's text holding $1 gets a notice naming item 13", names(replies[1], 13), replies[1]),
        ("the text keeps $1 as written", "see $1 before starting then $1" in replies[3], replies[3]),
        ("no notice for a $N the batch does not define", not stray, replies[2]),
    ]


def hidden_396(binary):
    """Tasks 125 and 396, done as one. 125's text fixes that an item says who
    wrote it; 396's, that a commit names its task with the trailer
    'Ekko: <id>' and the board reads where the task landed off git, a rebase
    included. by:NAME was a choice, question 504's recommended option, which
    the runs had to make alone. The boards are the reference's own tests'
    (bccf056): a session under a profile of its own, the person at the
    terminal, and a git repository the project lives in."""
    home = board_home()
    cli(binary, home, "--task", "written at the terminal")
    replies = mcp(binary, home, [
        ("create", {"text": "written by a session"}),
        ("context", {"item": 2}),
        ("search", {"filters": ["by:hiddenprof"]}),
    ], profile="hiddenprof")
    by_user = cli(binary, home, "--list", "by:user")
    shutil.rmtree(home, ignore_errors=True)
    found = [
        ("context names the session that wrote an item", "hiddenprof" in replies[1], replies[1]),
        ("search's by:PROFILE finds what a session wrote", listed(replies[2]) == {2}, replies[2]),
        ("--list by:user finds what the person wrote", listed(by_user) == {1}, by_user),
    ]

    home = board_home()
    repo = os.path.join(home, "repo")
    os.makedirs(repo)

    def git_in(*args, env=None):
        return subprocess.run(["git", "-c", "user.name=ekko", "-c", "user.email=ekko@example.com", "-c", "commit.gpgsign=false",
                               "-c", "init.defaultBranch=main", *args], cwd=repo, env={**ekko_env(home, repo), **(env or {})},
                              capture_output=True, text=True, check=True).stdout.strip()

    def commit(message):
        git_in("commit", "-q", "--allow-empty", "-m", message)
        return git_in("rev-parse", "HEAD")

    git_in("init", "-q")
    cli(binary, home, "init", folder=repo)
    mcp(binary, home, [("create", {"text": "ship it"}), ("create", {"text": "and this"})], folder=repo, profile="hiddenprof")
    # After the items: a read may skip commits older than the item.
    landed = commit("feat: the first half\n\nEkko: 1")
    other = commit("chore: another task's\n\nEkko: 12")
    before = commit("fix: the second half\n\nEkko: 2")
    # A rebase in small: the message kept, the sha changed.
    later = f"{int(datetime.datetime.now().timestamp()) + 60} +0000"
    git_in("commit", "-q", "--amend", "--allow-empty", "--no-edit", env={"GIT_COMMITTER_DATE": later})
    rebased = git_in("rev-parse", "HEAD")
    one, two = mcp(binary, home, [("context", {"item": 1}), ("context", {"item": 2})], folder=repo, profile="hiddenprof")
    terminal = cli(binary, home, "--context", "1", folder=repo)
    shutil.rmtree(home, ignore_errors=True)

    def names(text, sha):
        return re.search(rf"\b{sha[:7]}", text) is not None

    return found + [
        ("context lists the commit whose trailer is 'Ekko: 1'", names(one, landed), one),
        ("a trailer naming 12 is not listed on 1", not names(one, other), one),
        ("a rebased commit is listed by its new sha", rebased != before and names(two, rebased) and not names(two, before), two),
        ("the terminal's --context lists the commit too", names(terminal, landed), terminal),
    ]


HIDDEN = {"397": hidden_397, "394": hidden_394, "396": hidden_396}


def validate(task):
    """The hidden tests on the reference commit and on its parent, built in a
    clone of their own under ROOT."""
    _, commit, _, _ = TASKS[task]
    commit, parent = git(REPO, "rev-parse", commit), git(REPO, "rev-parse", commit + "^")
    folder = os.path.join(ROOT, f"ref-{task}")
    repo = os.path.join(folder, "repo")
    if not os.path.isdir(repo):
        os.makedirs(repo)
        git(repo, "init", "-q", "-b", "main")
        for rev in (parent, commit):
            git(repo, "fetch", "-q", "--no-tags", REPO, rev)
    found = {}
    for which, rev in (("parent", parent), ("reference", commit)):
        git(repo, "checkout", "-q", "--force", "--detach", rev)
        built = cargo(repo, "build", "--locked", "--quiet")
        if built.returncode:
            raise SystemExit(f"{task} {which}: the build failed\n{built.stderr[-2000:]}")
        binary = os.path.join(folder, f"ekko-{which}")
        shutil.copy2(os.path.join(repo, "target", "debug", "ekko"), binary)
        found[which] = HIDDEN[task](binary)
    checks = []
    for (name, parent_ok, _), (_, reference_ok, reply) in zip(found["parent"], found["reference"]):
        kind = "broken" if not reference_ok else ("guard" if parent_ok else "new")
        checks.append({"name": name, "kind": kind, "parent": parent_ok, "reference": reference_ok, "reply": reply[:600]})
        log(f"hidden {task}: {kind:6} {name}")
    os.makedirs(HIDDEN_OUT, exist_ok=True)
    with open(os.path.join(HIDDEN_OUT, f"{task}.json"), "w") as handle:
        json.dump({"task": task, "reference": commit, "parent": parent, "checks": checks}, handle, indent=2)


def checks(run_id):
    """The repo's own checks on a run's result, then the hidden tests on the
    binary cargo test built."""
    record = json.load(open(os.path.join(OUT, "runs", run_id, "run.json")))
    repo = os.path.join(ROOT, run_id, "repo")
    found = {"run": run_id, "head": git(repo, "rev-parse", "--short", "HEAD"), "dirty": bool(git(repo, "status", "--porcelain"))}
    test = cargo(repo, "test", "--locked")
    found["tests"] = {"ok": test.returncode == 0,
                      "passed": sum(int(n) for n in re.findall(r"test result: \w+\. (\d+) passed", test.stdout)),
                      "failed": sum(int(n) for n in re.findall(r"test result: \w+\. \d+ passed; (\d+) failed", test.stdout)),
                      "failures": re.findall(r"(?m)^test (\S+) \.\.\. FAILED$", test.stdout),
                      "tail": (test.stdout + test.stderr)[-1500:] if test.returncode else ""}
    binary = os.path.join(repo, "target", "debug", "ekko")
    hidden = HIDDEN.get(record["task"])
    found["hidden"] = [{"name": name, "ok": ok, "reply": reply[:600]} for name, ok, reply in hidden(binary)] if hidden and os.path.exists(binary) else []
    found["clippy"] = cargo(repo, "clippy", "--locked", "--all-targets", "--", "-D", "warnings").returncode == 0
    # The flake's build runs the tests in Nix's sandbox, as CI does: a test
    # that needs git finds none there unless the flake gives it, which
    # cargo test in the devshell never shows (396/cmax/1, 2026-09-26).
    found["nix"] = subprocess.run(["nix", "build", ".#default", "--no-link"], cwd=repo, capture_output=True, text=True,
                                  timeout=3600, stdin=subprocess.DEVNULL).returncode == 0
    with open(os.path.join(OUT, "runs", run_id, "checks.json"), "w") as handle:
        json.dump(found, handle, indent=2)
    passed = sum(1 for check in found["hidden"] if check["ok"])
    log(f"{run_id}: tests {'ok' if found['tests']['ok'] else 'FAILED'} ({found['tests']['passed']} passed, {found['tests']['failed']} failed), "
        f"hidden {passed}/{len(found['hidden'])}, clippy {'ok' if found['clippy'] else 'FAILED'}, nix build {'ok' if found['nix'] else 'FAILED'}")
    return found


# ---- the judge ------------------------------------------------------------------

SYSTEM = (
    "You grade answers to one programming task in ekko, a Rust command-line task board that is also an MCP server "
    "for coding agents. You have no tools: grade by reading what you are given. The answers were written unattended, "
    "with nobody to ask; where the task leaves a choice open, any reasonable choice is right."
)
CHECKLIST = """{spec}

Write the checklist a reviewer would grade an answer to this task against. Each item is one behavior or property the task states or clearly implies, which a reader of the diff can check. Three to eight items, the most important first; end an item with (open) where the task leaves the choice to whoever does it. Answer with the numbered list only."""
PROMPT = """# The task

{spec}

# The checklist, written from the task alone

{checklist}

# An accepted answer

The project's owner accepted this diff. It is one good answer, not the only one: a different design that meets the task is as good.

`````diff
{reference}
`````

{answers}

# How to grade

Grade each answer on its own, against the task and the checklist, by reading the code; no tests were run for you. Score each from 1 to 5:
- correctness: the code does what it sets out to do, without bugs or regressions;
- completeness: the checklist is met;
- scope: it changes what the task needs, no less and no more;
- fit: it reads like the code around it -- naming, structure, comments, tests.
Then say which answer you would merge, or tie, and by how much.

End with one JSON object in a ```json block, in this shape:
{{"answers": {{"X": {{"correctness": 4, "completeness": 5, "scope": 4, "fit": 4, "checklist": ["met", "partly", "missed"], "bugs": ["..."]}}}}, "prefer": "X", "margin": "slight", "why": "one sentence"}}
with one entry per answer, "checklist" holding met, partly or missed for each checklist item in order, "prefer" one of the answers' letters or "tie", and "margin" slight, clear or strong."""


def claude(prompt, effort, stream_path):
    """One call with no tools, no MCP servers, no hooks and no settings: the
    binary under the `claude` wrapper (whose own --settings and --mcp-config
    would bring the plugins back), logged in with the profile the runs use."""
    wrapper = open(os.path.realpath(shutil.which("claude"))).read()
    binary = re.findall(r'exec -a "\$0" "([^"]+)"', wrapper)[-1]
    env = {k: v for k, v in os.environ.items() if not k.startswith("CLAUDE")}
    env.update(dict(re.findall(r"(?m)^export (\w+)=(\S+)$", wrapper)))
    env.update(**harness.PROFILE_ENV, CTX_DISABLE="1")
    folder = tempfile.mkdtemp(prefix="ekko-paired-judge-")
    command = [binary, "-p", "--output-format", "stream-json", "--verbose", "--effort", effort, "--model", MODEL,
               "--tools", "", "--strict-mcp-config", "--setting-sources", "", "--system-prompt", SYSTEM,
               "--no-session-persistence", "--disable-slash-commands"]
    with open(stream_path, "w") as out:
        subprocess.run(command, input=prompt, cwd=folder, env=env, stdout=out, stderr=subprocess.STDOUT, text=True, timeout=3600)
    shutil.rmtree(folder, ignore_errors=True)
    result = harness.result_of(stream_path)
    with open(stream_path, errors="replace") as handle:
        five = harness.windows(handle).get("five_hour", (0, 0))[0]
    cost = units(result.get("usage") or {})
    log(f"{os.path.basename(stream_path)}: {cost / 1e6:.2f}M units, the 5-hour window at {five:.0%}")
    return result.get("result") or "", cost


def spec_of(task):
    """The request as the user typed it, and the text of each task it names as
    the board holds it."""
    items = json.load(open(os.path.join(harness.BOARD, "storage", "storage.json")))
    by_id = {int(item["_id"]): item for item in items.values()}
    prompt = TASKS[task][0]
    parts = [f"The request, as the user typed it: '{prompt}'."]
    parts += [f"Task {n}: {by_id[int(n)]['description']}" for n in re.findall(r"\d+", prompt)]
    return "\n\n".join(parts)


def verdict(text):
    for block in reversed(re.findall(r"```json\s*(\{.*?\})\s*```", text, re.S)):
        try:
            return json.loads(block)
        except ValueError:
            continue
    return {}


def judge(task):
    runs = [record for record in records() if record["task"] == task and record["status"] == "done"]
    if len(runs) < 2:
        log(f"judge {task}: {len(runs)} finished runs, nothing to compare")
        return
    os.makedirs(JUDGE, exist_ok=True)
    spec, spent = spec_of(task), 0
    checklist_path = os.path.join(JUDGE, f"{task}.checklist.md")
    if not os.path.exists(checklist_path):
        # Written from the task alone, before the judge sees any answer, and
        # kept for every later run of the task.
        text, cost = claude(CHECKLIST.format(spec=spec), "max", os.path.join(JUDGE, f"{task}-checklist.stream.jsonl"))
        spent += cost
        with open(checklist_path, "w") as handle:
            handle.write(text.strip() + "\n")
    checklist = open(checklist_path).read().strip()
    commit = TASKS[task][1]
    reference = git(REPO, "diff", commit + "^", commit)
    diffs = {record["run"]: open(os.path.join(OUT, "runs", record["run"], "result.diff")).read() for record in runs}
    order = sorted(diffs)
    random.Random(f"549-{task}").shuffle(order)
    verdicts = []
    for turn, names in enumerate((order, order[::-1])):
        labels = dict(zip("XYZWV", names))
        answers = "\n\n".join(f"# Answer {label}\n\n`````diff\n{diffs[name]}\n`````" for label, name in labels.items())
        text, cost = claude(PROMPT.format(spec=spec, checklist=checklist, reference=reference, answers=answers), "max",
                            os.path.join(JUDGE, f"{task}-{turn}.stream.jsonl"))
        spent += cost
        found = verdict(text)
        verdicts.append({"labels": labels, "units": round(cost), "parsed": bool(found),
                         "prefer": labels.get(found.get("prefer"), found.get("prefer")), "margin": found.get("margin"),
                         "why": found.get("why"),
                         "answers": {labels[label]: scores for label, scores in (found.get("answers") or {}).items() if label in labels}})
        log(f"judge {task} #{turn}: {' '.join(f'{label}={name}' for label, name in labels.items())} -> prefers {verdicts[-1]['prefer']} ({found.get('margin')})")
    with open(os.path.join(JUDGE, f"{task}.json"), "w") as handle:
        json.dump({"task": task, "account": harness.ACCOUNT, "checklist": checklist, "verdicts": verdicts, "units": round(spent)}, handle,
                  indent=2)


# ---- the report -----------------------------------------------------------------


class Call:
    def __init__(self, row, usage):
        self.at = datetime.datetime.fromisoformat(row["timestamp"].replace("Z", "+00:00")).timestamp()
        self.ctx = (usage.get("input_tokens") or 0) + (usage.get("cache_creation_input_tokens") or 0) + (usage.get("cache_read_input_tokens") or 0)
        self.out = usage.get("output_tokens") or 0
        self.think = (usage.get("output_tokens_details") or {}).get("thinking_tokens") or 0
        self.units = units(usage)
        self.effort = row.get("effort")
        self.tools = []

    def productive(self):
        for name, arg in self.tools:
            if name in EDIT_TOOLS:
                return "edit"
            if name == "Bash" and "git commit" in arg:
                return "commit"
        return None


def main_calls(path):
    """A transcript's main-thread calls in order, each once however many rows
    its content blocks took, and the index of the first call after each
    compaction."""
    calls, by_id, cuts = [], {}, []
    with open(path, errors="replace") as handle:
        for raw in handle:
            try:
                row = json.loads(raw)
            except ValueError:
                continue
            if row.get("type") == "system" and row.get("subtype") == "compact_boundary":
                cuts.append(len(calls))
                continue
            message = row.get("message") or {}
            if row.get("type") != "assistant" or row.get("isSidechain") or message.get("model") == "<synthetic>":
                continue
            mid = message.get("id") or row.get("requestId")
            if mid not in by_id:
                if not message.get("usage"):
                    continue
                by_id[mid] = Call(row, message["usage"])
                calls.append(by_id[mid])
            for block in message.get("content") or []:
                if isinstance(block, dict) and block.get("type") == "tool_use":
                    args = block.get("input") or {}
                    arg = args.get("command") if block.get("name") == "Bash" else args.get("file_path") or args.get("notebook_path")
                    by_id[mid].tools.append((block.get("name") or "", str(arg or "")[:400]))
    return calls, cuts


def measure(record):
    """A run's segments -- a session, or the part of one after a compaction --
    with their main-thread calls, and its subagents' calls apart."""
    folder = os.path.join(OUT, "runs", record["run"])
    segments, sub_units, sub_calls, versions = [], 0.0, 0, set()
    for number, session in enumerate(record["sessions"]):
        path = os.path.join(folder, "transcripts", session["sid"] + ".jsonl")
        calls, cuts = main_calls(path) if os.path.exists(path) else ([], [])
        if os.path.exists(path):
            versions |= set(re.findall(r'"version":"([^"]+)"', open(path, errors="replace").read()))
        bounds = [0, *cuts, len(calls)]
        for part, (start, end) in enumerate(zip(bounds, bounds[1:])):
            segments.append({"session": number, "part": part, "calls": calls[start:end],
                             "reset": "compaction" if part else ("handoff" if number else None),
                             "fired": session.get("fired") if end == len(calls) else None})
        for sub in glob.glob(os.path.join(folder, "transcripts", session["sid"], "*.jsonl")):
            for usage, _ in harness.calls_in(sub):
                sub_units += units(usage)
                sub_calls += 1
    return segments, sub_units, sub_calls, versions


def billed(record):
    """A run's cost as each session's own tally has it -- modelUsage and
    total_cost_usd in its result -- which also counts the calls its transcript
    leaves out: a compaction's summary (~63k units each in 396/bmax/1) and
    helper models. In units and dollars."""
    total = dollars = 0.0
    for index in range(len(record["sessions"])):
        for stream in glob.glob(os.path.join(OUT, "runs", record["run"], f"{index}-*.stream.jsonl")):
            result = harness.result_of(stream)
            dollars += result.get("total_cost_usd") or 0
            for usage in (result.get("modelUsage") or {}).values():
                total += units({"input_tokens": usage.get("inputTokens"), "output_tokens": usage.get("outputTokens"),
                                "cache_read_input_tokens": usage.get("cacheReadInputTokens"),
                                "cache_creation_input_tokens": usage.get("cacheCreationInputTokens")})
    return total, dollars


def reset_cost(segment, earlier):
    """R, as note 407 reads it: how far the context rose, over how many calls
    and units, from a segment's start to its first edit or commit; and x, the
    files it read again and the commands it ran again before then."""
    calls = segment["calls"]
    first = next((index for index, call in enumerate(calls) if call.productive()), None)
    before = calls[:first] if first is not None else calls
    read = {arg for seg in earlier for call in seg["calls"] for name, arg in call.tools if name == "Read"}
    ran = {arg for seg in earlier for call in seg["calls"] for name, arg in call.tools if name == "Bash"}
    reads = [arg for call in before for name, arg in call.tools if name == "Read"]
    commands = [arg for call in before for name, arg in call.tools if name == "Bash"]
    return {"start": calls[0].ctx if calls else None, "R": calls[first].ctx - calls[0].ctx if first is not None else None,
            "R_calls": first, "R_units": sum(call.units for call in before),
            "reads": len(reads), "reads_again": sum(1 for arg in reads if arg in read),
            "commands": len(commands), "commands_again": sum(1 for arg in commands if arg in ran)}


def median(values):
    return statistics.median(values) if values else 0


def mean(values):
    return statistics.fmean(values) if values else 0


def m(value):
    return f"{value / 1e6:.2f}M"


def report():
    runs = records()
    lines = [f"# Paired test: {len(runs)} runs, graded {harness.now():%Y-%m-%d %H:%M}", ""]
    measured, by_effort = {}, {}
    for record in runs:
        measured[record["run"]] = segments, _, _, _ = measure(record)
        effort = by_effort.setdefault(record["effort"], {"calls": [], "growth": []})
        for segment in segments:
            calls = segment["calls"]
            effort["calls"] += calls
            effort["growth"] += [b.ctx - a.ctx for a, b in zip(calls, calls[1:])]
            if segment["fired"]:
                handoff = [call for call in calls if call.at >= segment["fired"]["at"]]
                segment["handoff"] = (len(handoff), sum(call.units for call in handoff))

    lines += ["## Runs", "",
              "| run | status | account | sessions | calls | subagent calls | units | billed units | billed $ | points | minutes | peak context | tests | clippy | nix build | hidden new | hidden guard |",
              "|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|"]
    hidden_kinds = {}
    for task in {record["task"] for record in runs}:
        path = os.path.join(HIDDEN_OUT, f"{task}.json")
        if os.path.exists(path):
            hidden_kinds[task] = {check["name"]: check["kind"] for check in json.load(open(path))["checks"]}
    for record in runs:
        segments, _, sub_calls, _ = measured[record["run"]]
        path = os.path.join(OUT, "runs", record["run"], "checks.json")
        found = json.load(open(path)) if os.path.exists(path) else None
        kinds = hidden_kinds.get(record["task"], {})

        def hidden(kind):
            picked = [check["ok"] for check in (found or {}).get("hidden", []) if kinds.get(check["name"]) == kind]
            return f"{sum(picked)}/{len(picked)}" if picked else "-"

        tests = "-" if not found else ("ok" if found["tests"]["ok"] else f"FAILED {found['tests']['failed']}") + f" ({found['tests']['passed']})"
        # Runs from before the record named its account went on trabalho.
        account = record.get("account", "trabalho")
        cost, dollars = billed(record)
        lines.append(f"| {record['run']} | {record['status']} | {account} | {len(record['sessions'])} | {record['calls']} | {sub_calls} | "
                     f"{m(record['units'])} | {m(cost)} | {dollars:.2f} | {cost / POINTS[account]:.0f} | "
                     f"{sum(s['seconds'] for s in record['sessions']) / 60:.0f} | "
                     f"{max((c.ctx for seg in segments for c in seg['calls']), default=0) // 1000}k | {tests} | "
                     f"{'-' if not found else 'ok' if found['clippy'] else 'FAILED'} | "
                     f"{'-' if not found or 'nix' not in found else 'ok' if found['nix'] else 'FAILED'} | "
                     f"{hidden('new')} | {hidden('guard')} |")
    versions = sorted(set().union(*(measured[record["run"]][3] for record in runs))) if runs else []
    lines += ["", f"Claude Code versions in the runs: {', '.join(versions) or '-'}. "
              "Units are the transcripts' calls; billed units and dollars are each session's own tally, which also counts "
              "a compaction's summary call and helper models. Points are of billed units, in the 5-hour window of the account each run went on, at "
              + ", ".join(f"{point // 1000}k units a point on {name}" for name, point in POINTS.items()) + ".", ""]

    lines += ["## 526: high against max, per task", "",
              "| task | max units | high units | high / max | max calls | high calls | max resets | high resets | the original session |",
              "|---|---|---|---|---|---|---|---|---|"]
    ratios = []
    for task in sorted({record["task"] for record in runs}):
        pick = {cell: next((r for r in runs if r["task"] == task and r["cell"] == cell and r["rep"] == 1 and r["status"] == "done"), None)
                for cell in ("cmax", "chigh")}
        if not all(pick.values()):
            continue
        high, top = pick["chigh"], pick["cmax"]
        ratios.append(high["units"] / top["units"])
        calls, spent = ORIGINAL.get(task, (0, 0))
        lines.append(f"| {task} | {m(top['units'])} | {m(high['units'])} | {high['units'] / top['units']:.2f} | {top['calls']} | {high['calls']} | "
                     f"{len(top['sessions']) - 1} | {len(high['sessions']) - 1} | {calls} calls, {m(spent)} |")
    if ratios:
        lines += ["", f"Median high / max over {len(ratios)} pairs: {median(ratios):.2f} (the rule: high becomes the default at 0.90 or less over 10 pairs, "
                  "with the hidden tests passing as often and the judge preferring max in at most 3 of 10)."]
    repeated = {}
    for record in runs:
        repeated.setdefault((record["task"], record["cell"]), []).append(record["units"])
    for (task, cell), values in sorted(repeated.items()):
        if len(values) > 1:
            lines.append(f"Spread, {task} {cell} over {len(values)} runs: {', '.join(m(v) for v in values)}; max / min {max(values) / min(values):.2f}.")
    lines.append("")

    lines += ["## Per call, by effort (main thread)", "",
              "| effort | calls | thinking median | thinking mean | thinking share of output | output median | output mean | growth median | growth mean |",
              "|---|---|---|---|---|---|---|---|---|"]
    for effort, found in sorted(by_effort.items()):
        calls = found["calls"]
        out = sum(call.out for call in calls)
        lines.append(f"| {effort} | {len(calls)} | {median([c.think for c in calls]):.0f} | {mean([c.think for c in calls]):.0f} | "
                     f"{sum(c.think for c in calls) / out if out else 0:.0%} | {median([c.out for c in calls]):.0f} | {mean([c.out for c in calls]):.0f} | "
                     f"{median(found['growth']):.0f} | {mean(found['growth']):.0f} |")
    lines.append("")

    lines += ["## Resets", "",
              "| run | session | kind | the handoff (calls, units) | starts at | R | calls to the first edit | units till then | files read again | commands again |",
              "|---|---|---|---|---|---|---|---|---|---|"]
    for record in runs:
        segments = measured[record["run"]][0]
        for index, segment in enumerate(segments):
            if segment.get("handoff"):
                count, spent = segment["handoff"]
                lines.append(f"| {record['run']} | {segment['session']} | writes the handoff | {count}, {m(spent)} | | | | | | |")
            if segment["reset"]:
                cost = reset_cost(segment, segments[:index])
                rise = "-" if cost["R"] is None else f"{cost['R'] // 1000}k"
                upto = "-" if cost["R_calls"] is None else cost["R_calls"]
                lines.append(f"| {record['run']} | {segment['session']}.{segment['part']} | {segment['reset']} | | {(cost['start'] or 0) // 1000}k | "
                             f"{rise} | {upto} | {m(cost['R_units'])} | {cost['reads_again']} of {cost['reads']} | "
                             f"{cost['commands_again']} of {cost['commands']} |")
    lines.append("")

    judged = sorted(glob.glob(os.path.join(JUDGE, "*.json")))
    if judged:
        lines += ["## The judge", "", "| task | run | correctness | completeness | scope | fit | checklist met | preferred |", "|---|---|---|---|---|---|---|---|"]
        spent = points = 0
        for path in judged:
            found = json.load(open(path))
            spent += found["units"]
            # Verdicts from before the record named its account came from trabalho.
            points += found["units"] / POINTS[found.get("account", "trabalho")]
            names = sorted({name for v in found["verdicts"] for name in v["labels"].values()})
            for name in names:
                scores = [v["answers"].get(name) or {} for v in found["verdicts"]]
                cols = [f"{mean([s.get(key) or 0 for s in scores]):.1f}" for key in ("correctness", "completeness", "scope", "fit")]
                met = [sum(1 for item in s.get("checklist") or [] if item == "met") for s in scores]
                size = max((len(s.get("checklist") or []) for s in scores), default=0)
                wins = [f"{v['margin']}" for v in found["verdicts"] if v["prefer"] == name]
                lines.append(f"| {found['task']} | {name} | {' | '.join(cols)} | {'/'.join(map(str, met))} of {size} | "
                             f"{len(wins)} of {len(found['verdicts'])}{' (' + ', '.join(wins) + ')' if wins else ''} |")
            for v in found["verdicts"]:
                lines.append(f"| {found['task']} | order {' '.join(f'{k}={n}' for k, n in v['labels'].items())} | | | | | | {v['prefer']}: {v['why']} |")
        lines += ["", f"The judge's calls: {m(spent)} ({points:.0f} points, each on the account that paid for it).", ""]

    text = "\n".join(lines)
    with open(os.path.join(OUT, "report.md"), "w") as handle:
        handle.write(text + "\n")
    print(text)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--account", choices=sorted(POINTS), default=harness.ACCOUNT, help="who pays for the judge")
    sub = parser.add_subparsers(dest="command", required=True)
    for name in ("validate", "judge"):
        sub.add_parser(name).add_argument("tasks", nargs="*")
    sub.add_parser("checks").add_argument("runs", nargs="*")
    sub.add_parser("report")
    args = parser.parse_args()
    harness.use_account(args.account)
    if args.command == "validate":
        for task in args.tasks or sorted(HIDDEN):
            validate(task)
    elif args.command == "checks":
        done = [record["run"] for record in records() if record["status"] == "done"]
        for run_id in args.runs or [r for r in done if not os.path.exists(os.path.join(OUT, "runs", r, "checks.json"))]:
            checks(run_id)
    elif args.command == "judge":
        for task in args.tasks or sorted({record["task"] for record in records()}):
            if args.tasks or not os.path.exists(os.path.join(JUDGE, f"{task}.json")):
                judge(task)
    else:
        report()


if __name__ == "__main__":
    main()
