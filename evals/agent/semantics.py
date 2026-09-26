"""Semantics eval: every behaviour the rigor plan calls wrong, run against an
isolated server and reported as OPEN (as measured when the plan was written)
or FIXED (the plan's target holds). Each row names the plan item it belongs to.

    nix develop -c python3 evals/agent/semantics.py [-v]

-v prints every reply. Scratch boards land in target/evals/boards/.
"""

import json
import re
import sys
import time

from ekko_mcp import Mcp, cli, cursor_of, line_ids, provenance, results_file, scratch, show, size, written_ids

VERBOSE = "-v" in sys.argv
rows = []


def check(item, what, observed, target, fixed):
    rows.append({"item": item, "what": what, "observed": observed, "target": target, "fixed": bool(fixed)})


def call(m, name, **arguments):
    text, is_error, seconds = m.call(name, **arguments)
    if VERBOSE:
        show(f"{name} {json.dumps(arguments, ensure_ascii=False)}", text, is_error, seconds)
    return text, is_error


def changed_count(text):
    match = re.search(r"(\d+) changed", text)
    return int(match.group(1)) if match else None


def ids_of(values):
    return [value["id"] if isinstance(value, dict) else value for value in values]


def line_for(text, item):
    return next((line for line in text.splitlines() if line_ids(line)[:1] == [item]), "")


def cursor_and_released_work():
    m = Mcp(scratch("semantics-cursor"))
    call(m, "create", text="first task")
    prime, _ = call(m, "prime")
    count = changed_count(call(m, "changes", since=cursor_of(prime))[0])
    check("3.1", "changes(cursor) right after prime, nothing written", f"{count} changed", "0 changed", count == 0)

    reply, _ = call(
        m,
        "batch",
        ops=[
            {"op": "create", "text": "blocker"},
            {"op": "create", "text": "dependent one", "blocked_by": ["$1"]},
            {"op": "create", "text": "dependent two", "blocked_by": ["$1"]},
        ],
    )
    blocker, one, two = written_ids(reply)
    cursor = cursor_of(call(m, "prime")[0])
    time.sleep(0.01)
    reply, _ = call(m, "set_state", items=[blocker], state="done")
    released = json.loads(reply).get("nowReady")
    named = isinstance(released, list) and {one, two} <= set(ids_of(released))
    check("1.3", "completing a blocker names the work it released", f"nowReady {released}", f"nowReady [{one}, {two}]", named)

    text, _ = call(m, "changes", since=cursor)
    ready_lines = sum("ready" in line_for(text, item).lower() for item in (one, two))
    check("3.2", "changes reports the released dependents as ready", f"{ready_lines} of 2", "2 of 2", ready_lines == 2)

    line = line_for(call(m, "search", filters=["done"])[0], blocker)
    check("0.4", "a listed done task says it is done", "says done" if "done" in line else "no state", "says done", "done" in line)
    check("0.4", "a done task carries no unblocks count", "unblocks shown" if "unblocks" in line else "none", "none", "unblocks" not in line)
    m.close()


def priority_inversion():
    m = Mcp(scratch("semantics-inversion"))
    tomorrow = time.strftime("%Y-%m-%d", time.localtime(time.time() + 86400))
    call(
        m,
        "batch",
        ops=[
            {"op": "create", "text": "low-priority prerequisite of the urgent task"},
            {"op": "create", "text": "urgent p3 due tomorrow", "priority": 3, "due": tomorrow, "blocked_by": ["$1"]},
            {"op": "create", "text": "unrelated p2 task", "priority": 2},
            {"op": "create", "text": "unrelated p1 task due in 2028", "due": "2028-12-31"},
            {"op": "create", "text": "p1 task that unblocks five"},
            *[{"op": "create", "text": f"waits on five ({k})", "blocked_by": ["$5"]} for k in range(1, 6)],
        ],
    )
    order = line_ids(call(m, "next")[0])
    # The prerequisite first; then priority before work others wait on, which
    # only orders tasks of one priority (the blocking weight, task 284).
    check("1.5", "next puts the prerequisite of urgent work first", order, [1, 3, 4, 5], order == [1, 3, 4, 5])
    m.close()


def notes_and_cancellation():
    m = Mcp(scratch("semantics-notes"))
    note = written_ids(call(m, "create", kind="note", text="just a note")[0])[0]
    _, refused = call(m, "create", text="task waiting on a note", blocked_by=[note])
    check("0.5", "a note is refused as a blocker", "refused" if refused else "accepted", "refused", refused)
    text, refused = call(m, "set_state", items=[note], state="done")
    check("0.5", "a task state is refused on a note", "refused" if refused else "accepted, nothing changed", "refused", refused)

    reply, _ = call(
        m,
        "batch",
        ops=[
            {"op": "create", "text": "prerequisite that gets cancelled"},
            {"op": "create", "text": "work that needed it", "blocked_by": ["$1"]},
        ],
    )
    prerequisite, work = written_ids(reply)
    call(m, "set_state", items=[prerequisite], state="cancelled")
    prime, _ = call(m, "prime")
    attention = prime.split("Needs attention", 1)[1] if "Needs attention" in prime else ""
    flagged = "cancel" in attention.lower() and str(work) in attention
    check("0.5", "prime flags work a cancelled blocker released", "flagged" if flagged else "silent", "flagged", flagged)
    m.close()


def chain_root():
    m = Mcp(scratch("semantics-chain"))
    reply, _ = call(
        m,
        "batch",
        ops=[
            {"op": "create", "text": "chain root"},
            {"op": "create", "text": "chain one", "blocked_by": ["$1"]},
            {"op": "create", "text": "chain two", "blocked_by": ["$2"]},
            {"op": "create", "text": "chain three", "blocked_by": ["$3"]},
            {"op": "create", "text": "chain tip", "blocked_by": ["$4"]},
        ],
    )
    ids = written_ids(reply)
    root, tip = ids[0], ids[-1]
    text, _ = call(m, "context", item=tip)
    lines = text.splitlines()
    named = root in line_ids(text) or any(re.match(r"^\s*roots?\b", line, re.I) and str(root) in line for line in lines)
    check("1.6", "context of a chain's tip names its root", "named" if named else f"only {line_ids(text)[1:]}", f"root {root}", named)
    m.close()


def search_quality():
    m = Mcp(scratch("semantics-search"))
    reply, _ = call(
        m,
        "batch",
        ops=[
            {"op": "create", "kind": "note", "text": "Decisão tomada: usar grafo de dependências com ordenação topológica"},
            {"op": "create", "text": "recycled ids break caches"},
            {"op": "create", "kind": "note", "text": ("padding " * 40) + "the important keyword is here at the end"},
            *[{"op": "create", "text": f"filler task {k}"} for k in range(30)],
        ],
    )
    decision, recycled, padded = written_ids(reply)[:3]

    found = line_ids(call(m, "search", text="decisao")[0])
    check("2.1", "search folds accents: decisao finds Decisão", found, [decision], decision in found)
    found = line_ids(call(m, "search", text="dependencias grafo")[0])
    check("2.1", "search matches words in any order", found, [decision], decision in found)
    found = line_ids(call(m, "search", text="cycle")[0])
    check("2.1", "cycle does not match recycled", found, [], recycled not in found)
    line = line_for(call(m, "search", text="keyword")[0], padded)
    check("2.1", "a hit past the clip shows the matching words", "shown" if "keyword" in line else "clipped away", "shown", "keyword" in line)
    text, _ = call(m, "search")
    listed = len(line_ids(text))
    check("0.3", "search with no text or filter is bounded", f"{listed} lines, {size(text)} bytes", "at most 20 lines and a total", listed <= 20)
    m.close()


def recycled_ids():
    path = scratch("semantics-recycle")
    m = Mcp(path)
    call(m, "create", text="keeps the lower id")
    old = written_ids(call(m, "create", text="done, then cleared by the user")[0])[0]
    call(m, "set_state", items=[old], state="done")
    cli(path, "--clear")
    call(m, "create", text="a brand new unrelated task")
    text, missing = call(m, "context", item=old)
    reused = not missing and "brand new unrelated" in text
    check("3.4", "a cleared item's id is not handed to a new item", "reused" if reused else "not reused", "not reused", not reused)
    m.close()


def wording_and_limits():
    path = scratch("semantics-wording")
    m = Mcp(path)
    text, _ = call(m, "roadmap")
    check("0.8", "roadmap gives the agent no CLI command", "names one" if "ekko --" in text else "none", "none", "ekko --" not in text)
    text, refused = call(m, "next", limit=0)
    check("0.4", "next(limit=0) is refused", "refused" if refused else f"answered: {text.strip()[:40]}", "refused", refused)

    before = m.instructions
    call(m, "create", text="a write between two handshakes")
    again = Mcp(path)
    same = again.instructions == before
    check("0.8", "server instructions stay byte-identical across writes", "identical" if same else "changed", "identical", same)
    length = len(before.encode())
    check("0.8", "server instructions fit Claude Code's 2 KB cut", f"{length} bytes", "at most 2,000 bytes", length <= 2000)
    longest = max(len(tool["description"].encode()) for tool in again.tools())
    check("0.8", "every tool description fits the 2 KB cut", f"longest {longest} bytes", "at most 2,000 bytes", longest <= 2000)
    again.close()
    m.close()


def new_surfaces():
    m = Mcp(scratch("semantics-surfaces"))
    one, two = written_ids(call(m, "batch", ops=[{"op": "create", "text": "one"}, {"op": "create", "text": "two"}])[0])
    text, refused = call(m, "context", items=[one, two])
    both = not refused and {one, two} <= set(line_ids(text))
    check("2.3", "context reads several items in one call", "refused" if refused else f"ids {line_ids(text)}", "both items", both)
    text, refused = call(m, "context", item=one, detail="concise")
    check("2.2", "context takes a concise detail level", "refused" if refused else f"{size(text)} bytes", "accepted", not refused)
    prime, _ = call(m, "prime")
    text, refused = call(m, "prime", if_rev=cursor_of(prime))
    short = not refused and size(text) <= 60
    check("3.3", "a conditional prime answers unchanged in a few bytes", "refused" if refused else f"{size(text)} bytes", "at most 60 bytes", short)
    m.close()


def session_aware_hook():
    path = scratch("semantics-hook")
    m = Mcp(path)
    call(m, "batch", ops=[{"op": "create", "text": f"task {k}", "priority": 2} for k in range(12)])
    m.close()
    event = {"hook_event_name": "SessionStart", "session_id": "semantics-eval"}
    startup = cli(path, "--prime", "--hook", stdin=json.dumps({**event, "source": "startup"}))
    resume = cli(path, "--prime", "--hook", stdin=json.dumps({**event, "source": "resume"}))
    real = "Ready, best first (12)" in startup and "cursor" in resume
    observed = f"{len(resume)} characters (startup {len(startup)})" if real else f"no hook answer: {resume.strip()[:60]}"
    check("0.7", "a resume with nothing written gets a line, not the prime", observed, "at most 200 characters", real and len(resume) <= 200)


def prime_budget():
    m = Mcp(scratch("semantics-budget"))
    reason = "a reason that runs long enough to matter " * 7
    task = "titled at the length real titles run to\nits body, described at the length real tasks on a board run to, so a listing fills"
    ops = [{"op": "create", "text": f"ready task number {k}, {task}"} for k in range(1, 81)]
    ops += [{"op": "create", "kind": "note", "text": reason, "attached_to": f"${k}"} for k in range(1, 81)]
    call(m, "batch", ops=ops)
    text, _ = call(m, "prime")
    announced = "more: next lists them" in text
    observed = f"{len(text)} characters, {'cut announced' if announced else 'no cut announced'}"
    check("0.2", "prime stays within 6,000 characters on a heavy board", observed, "at most 6,000, cut announced", len(text) <= 6000 and announced)
    m.close()


def main():
    print(provenance())
    for scenario in (
        cursor_and_released_work,
        priority_inversion,
        notes_and_cancellation,
        chain_root,
        search_quality,
        recycled_ids,
        wording_and_limits,
        new_surfaces,
        session_aware_hook,
        prime_budget,
    ):
        scenario()
    rows.sort(key=lambda row: [int(part) for part in row["item"].split(".")])
    width = max(len(row["what"]) for row in rows)
    print()
    for row in rows:
        status = "FIXED" if row["fixed"] else "OPEN "
        print(f"{status} {row['item']:<4} {row['what']:<{width}}  {row['observed']}  (target: {row['target']})")
    print(f"\n{sum(row['fixed'] for row in rows)} of {len(rows)} fixed")
    out = results_file("semantics")
    with open(out, "w") as f:
        json.dump({"provenance": provenance(), "rows": rows}, f, indent=2, ensure_ascii=False, default=str)
    print(f"results: {out}")
    # A row back to OPEN is a regression: fail, so CI says so.
    if not all(row["fixed"] for row in rows):
        sys.exit(1)


if __name__ == "__main__":
    main()
