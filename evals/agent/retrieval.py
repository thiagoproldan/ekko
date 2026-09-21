"""Retrieval eval: the calls and bytes an agent spends to answer the questions
it asks of a board, measured on a scratch copy of a real one.

    nix develop -c python3 evals/agent/retrieval.py [--project NAME]

The copy is written to target/evals/boards/retrieval/, and the session model
at the end writes to that copy only. With no --project the board is the
project named ekko when one is registered, else the default board.

Items are chosen from the board as it is, not by number: the open task with
the longest chain of open blockers, ready tasks that release other work. A
board keeps changing, and a question pinned to item 93 stops being a question
once 93 is done.

Two prices, both from notes 161-165 on the ekko board (week of 2026-09-08,
Opus 5 weights: cache write 2x, cache read 0.1x, output 5x). A token that
enters the context costs 2 + 0.1*T + 2*R input tokens over T later requests
and R cold returns. A call is also one more request that re-reads the whole
context, 0.1*C + 5*o for a context of C tokens and o output tokens.
"""

import argparse
import json
import re
import statistics
import time
import unicodedata

from ekko_mcp import Mcp, cli, copy_real, cursor_of, line_ids, load, provenance, registered_projects, results_file, size, tokens

OPEN = ("pending", "in progress", "paused")
STATE_IN_LINE = re.compile(r"\[(pending|in progress|paused|done|cancelled|note)[\],]")
BLOCKER_LINE = re.compile(r"^\s*(\d+)\.\s\[([^\],]+)")


# ---- the board as ground truth ----------------------------------------


def state(item):
    if not item.get("_isTask"):
        return None
    if item.get("cancelled"):
        return "cancelled"
    if item.get("isComplete"):
        return "done"
    if item.get("inProgress"):
        return "in progress"
    if item.get("paused"):
        return "paused"
    return "pending"


def holds(item):
    return item.get("trashed") is None and state(item) in OPEN


def visible(item):
    return item.get("stashed") is None and item.get("trashed") is None


class Board:
    def __init__(self, data):
        self.data = data
        self.by_uid = {item["uid"]: i for i, item in data.items() if item.get("uid")}
        self.dependents = {}
        for i, item in data.items():
            for blocker in self.open_blockers(i):
                self.dependents.setdefault(blocker, []).append(i)

    def open_blockers(self, i):
        found = (self.by_uid.get(uid) for uid in self.data[i].get("blockedBy") or [])
        return [b for b in found if b is not None and holds(self.data[b])]

    def depth(self, i, trail=()):
        above = [b for b in self.open_blockers(i) if b not in trail]
        return 1 + max(self.depth(b, trail + (i,)) for b in above) if above else 0

    def deepest_blocked(self):
        blocked = [i for i, item in self.data.items() if visible(item) and holds(item) and self.open_blockers(i)]
        return max(blocked, key=lambda i: (self.depth(i), -i), default=None)

    def releasing_ready(self, count):
        ready = [
            i
            for i, item in self.data.items()
            if visible(item) and holds(item) and state(item) != "in progress" and not self.open_blockers(i) and i in self.dependents
        ]
        return sorted(ready)[:count]


def fold(text):
    text = unicodedata.normalize("NFKD", text.lower())
    return "".join(ch for ch in text if not unicodedata.combining(ch))


def truly_matches(description, query):
    """Every word of the query starts a word of the description: the judgment a
    substring search gets wrong when it finds 'cycle' inside 'recycled'."""
    body = fold(description)
    return all(re.search(rf"\b{re.escape(word)}", body) for word in fold(query).split())


# ---- questions ----------------------------------------------------------


def section(text, title):
    lines = text.splitlines()
    if title not in lines:
        return []
    out = []
    for line in lines[lines.index(title) + 1 :]:
        if not line.strip():
            break
        out.append(line)
    return out


def root_cause(m, target):
    """(calls, bytes, how) to learn the open roots above `target`: one call when
    context names them, else the walk an agent does today, one context per
    open blocker until none is left."""
    text, _, _ = m.call("context", item=target)
    calls, total = 1, size(text)
    if section(text, "Roots") or any(re.match(r"^\s*roots?\b", line, re.I) for line in text.splitlines()):
        return calls, total, "context names the roots"
    queue, seen, roots = [(target, text)], {target}, []
    while queue:
        item, body = queue.pop(0)
        above = []
        for line in section(body, "Blocked by"):
            match = BLOCKER_LINE.match(line)
            if match and match.group(2).strip() in OPEN:
                above.append(int(match.group(1)))
        if not above and item != target:
            roots.append(item)
        for blocker in above:
            if blocker not in seen:
                seen.add(blocker)
                reply, _, _ = m.call("context", item=blocker)
                calls, total = calls + 1, total + size(reply)
                queue.append((blocker, reply))
    return calls, total, f"walked to roots {sorted(roots)}"


def search_question(m, board, query):
    text, is_error, _ = m.call("search", text=query)
    hits = line_ids(text)
    true = [i for i in hits if i in board.data and truly_matches(board.data[i]["description"], query)]
    missed = [
        i
        for i, item in board.data.items()
        if visible(item) and i not in hits and truly_matches(item["description"], query)
    ]
    return {"query": query, "bytes": size(text), "hits": len(hits), "true": len(true), "missed": len(missed), "error": is_error}


# ---- the session model --------------------------------------------------


def session_model(path, m, board):
    """A working session built from sizes measured on this board. Conservative:
    after completing a task the agent asks next(5), not a whole prime."""
    steps = []

    def step(what, calls, nbytes, how=""):
        steps.append({"step": what, "calls": calls, "bytes": nbytes, "how": how})

    event = {"hook_event_name": "SessionStart", "session_id": "retrieval-eval"}
    hook = ["--prime", "--hook"] if "--hook" in cli(path, "--help") else ["--prime"]
    step("session start: prime from the hook", 0, size(cli(path, *hook, stdin=json.dumps({**event, "source": "startup"}))))
    step("one resume with nothing written", 0, size(cli(path, *hook, stdin=json.dumps({**event, "source": "resume"}))))

    cursor = cursor_of(m.call("prime")[0])
    step("5 changes calls with nothing written", 5, sum(size(m.call("changes", since=cursor)[0]) for _ in range(5)))

    found = [m.call("search", text=query)[0] for query in ("cycle", "uid")]
    step("2 searches: cycle, uid", 2, sum(map(size, found)))

    hits = line_ids(found[1])[:3]
    shown = any(STATE_IN_LINE.search(line) for line in found[1].splitlines() if line_ids(line))
    reads = hits[:1] if shown else hits
    how = "state in the lines, one read for the reason" if shown else "no state in the lines"
    together, refused, _ = m.call("context", items=reads) if len(reads) > 1 else ("", True, 0)
    if len(reads) > 1 and not refused:
        step(f"contexts for state and reason of {len(hits)} hits", 1, size(together), how + ", one call for all")
    else:
        step(f"contexts for state and reason of {len(hits)} hits", len(reads), sum(size(m.call("context", item=i)[0]) for i in reads), how)

    target = board.deepest_blocked()
    if target is not None:
        calls, nbytes, how = root_cause(m, target)
        step(f"why {target} cannot start", calls, nbytes, how)

    releasers = board.releasing_ready(2)
    replies = nexts = primes = 0
    named = bool(releasers)
    for item in releasers:
        reply, is_error, _ = m.call("set_state", items=[item], state="done")
        replies += size(reply)
        named = named and not is_error and "nowReady" in reply
        nexts += size(m.call("next", limit=5)[0])
        primes += size(m.call("prime")[0])
    variant = None
    if releasers:
        count = len(releasers)
        if named:
            step(f"complete {count} tasks, released work named in the reply", count, replies, "nowReady")
        else:
            step(f"complete {count} tasks, then next(5)", 2 * count, replies + nexts, f"completed {releasers}")
            variant = primes - nexts
    return steps, variant


def priced(nbytes, calls, args):
    entered = nbytes / 2.5
    token_units = entered * (2 + 0.1 * args.later_requests + 2 * args.cold_returns)
    call_units = calls * (0.1 * args.context_tokens + 5 * args.output_tokens)
    return entered, token_units, call_units


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--project", help="a registered or legacy project; default: ekko if registered, else the default board")
    parser.add_argument("--context-tokens", type=float, default=471_000, help="C, average context per request (note 162)")
    parser.add_argument("--output-tokens", type=float, default=1_439, help="o, output per request (note 163)")
    parser.add_argument("--later-requests", type=float, default=50, help="T, requests after a token enters the context")
    parser.add_argument("--cold-returns", type=float, default=1, help="R, returns after the cache expired")
    args = parser.parse_args()
    project = args.project or ("ekko" if "ekko" in registered_projects() else None)

    path, label = copy_real("retrieval", project)
    board = Board(load(path))
    m = Mcp(path)
    print(provenance())
    print(f"board: a copy of {label}, {len(board.data)} items\n")
    report = {"provenance": provenance(), "board": label}

    target = board.deepest_blocked()
    if target is not None:
        calls, nbytes, how = root_cause(m, target)
        print(f"why {target} cannot start (open chain {board.depth(target)} deep): {calls} calls, {nbytes} bytes, {how}")
        report["root_cause"] = {"item": target, "calls": calls, "bytes": nbytes, "how": how}

    report["search"] = []
    print("\nsearch precision (a true hit starts a word with every query word)")
    for query in ("dependency cycle", "check cycle", "cycle check", "cycle", "uid", "decisao"):
        row = search_question(m, board, query)
        report["search"].append(row)
        print(f"  {query!r:20} {row['hits']:>3} hits, {row['true']:>3} true, {row['missed']:>3} true missed, {row['bytes']:>6} bytes")

    listing, _, _ = m.call("search")
    visible_items = [i for i, item in board.data.items() if visible(item)]
    report["search_all"] = {"bytes": size(listing), "lines": len(line_ids(listing))}
    print(f"\nsearch() with no arguments: {size(listing)} bytes, {len(line_ids(listing))} lines")

    sizes = sorted(size(m.call("context", item=i)[0]) for i in visible_items)
    if sizes:
        report["context_bytes"] = {"median": statistics.median(sizes), "p90": sizes[int(len(sizes) * 0.9)], "max": sizes[-1]}
        print(f"context over {len(sizes)} visible items: median {statistics.median(sizes):.0f}, p90 {sizes[int(len(sizes) * 0.9)]}, max {sizes[-1]} bytes")

    # A time window is a clock reading, which changes still takes, not arithmetic
    # on a cursor: since 3.1 a cursor is a revision.
    now_ms = int(time.time() * 1000)
    report["changes_window"] = {}
    for hours in (1, 24, 168):
        text, _, _ = m.call("changes", since=now_ms - hours * 3_600_000)
        report["changes_window"][hours] = size(text)
    print(f"changes over the last 1h / 24h / 168h: {' / '.join(str(v) for v in report['changes_window'].values())} bytes")

    steps, variant = session_model(path, m, board)
    m.close()
    total_bytes = sum(step["bytes"] for step in steps)
    total_calls = sum(step["calls"] for step in steps)
    print("\nsession model")
    for step in steps:
        print(f"  {step['step']:<58} {step['calls']:>3} calls {step['bytes']:>7} bytes  {step['how']}")
    low, high = tokens(total_bytes)
    print(f"  {'total':<58} {total_calls:>3} calls {total_bytes:>7} bytes  ≈ {low:,.0f}-{high:,.0f} tokens")
    if variant:
        print(f"  with prime instead of next(5) after completing: {total_bytes + variant} bytes")
    entered, token_units, call_units = priced(total_bytes, total_calls, args)
    print(
        f"  priced: the tokens it put in context ≈ {token_units:,.0f} input-equivalent tokens; "
        f"its {total_calls} calls re-read the context ≈ {call_units:,.0f} (C={args.context_tokens:,.0f}, o={args.output_tokens:,.0f})"
    )
    report["session"] = {
        "steps": steps,
        "bytes": total_bytes,
        "calls": total_calls,
        "prime_variant_bytes": total_bytes + variant if variant else None,
        "token_units": token_units,
        "call_units": call_units,
    }

    out = results_file("retrieval")
    with open(out, "w") as f:
        json.dump(report, f, indent=2, ensure_ascii=False)
    print(f"\nresults: {out}")


if __name__ == "__main__":
    main()
