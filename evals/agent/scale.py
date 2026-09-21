"""Scale eval: synthetic boards from a few hundred to twenty thousand items,
per-call latency inside one warm server, and the size of what each call hands
the model.

    nix develop -c python3 evals/agent/scale.py            # 150, 1k, 5k, chain of 5k
    nix develop -c python3 evals/agent/scale.py --full     # adds 20k, layered 10k, roots over a chain
    nix develop -c python3 evals/agent/scale.py r1k chain5k

The topologies are the ones that separate a linear graph pass from a
per-item walk: random local dependencies, one long chain, ten layers each
blocked by the one above, and a fifth of the tasks all blocking the head of
a chain made of the rest. Boards are written to target/evals/boards/, and the
writes at the end of each run change the board they ran on.
"""

import json
import os
import random
import subprocess
import sys
import time

from ekko_mcp import EKKO, Mcp, board_dir, cursor_of, env_for, provenance, results_file, scratch, size, storage_file

WORDS = (
    "storage render graph cursor phase board note task parser lock schema index "
    "compositor wayland shader buffer socket token cache merge rebase deploy metric "
    "alpha beta gamma delta epsilon zeta theta kappa lambda sigma omega"
).split()


def text(rng, length):
    words = []
    while sum(len(word) + 1 for word in words) < length:
        words.append(rng.choice(WORDS))
    return " ".join(words)


def uid(i):
    return f"{0x18D000000000000 + i:x}-1"


def item(i, is_task, description, stamp):
    base = {
        "_id": i,
        "_date": "Mon Sep 14 2026",
        "_timestamp": stamp,
        "description": description,
        "isStarred": False,
        "boards": ["My Board"],
        "_isTask": is_task,
        "uid": uid(i),
        "updatedAt": stamp,
    }
    if is_task:
        base.update({"isComplete": False, "inProgress": False, "priority": 1})
    return base


def generate(tasks, notes, topology, seed=7, done_ratio=0.3):
    rng = random.Random(seed)
    stamp0 = 1789000000000
    blockers = {}
    ids = list(range(1, tasks + 1))
    if topology == "random":
        for i in ids:
            if i > 1 and rng.random() < 0.5:
                low = max(1, i - 60)
                blockers[i] = sorted({rng.randint(low, i - 1) for _ in range(rng.choice([1, 2]))})
    elif topology == "chain":
        for i in ids[1:]:
            blockers[i] = [i - 1]
    elif topology == "layered":
        width = max(1, tasks // 10)
        for i in ids:
            layer = (i - 1) // width
            if layer > 0:
                low, high = (layer - 1) * width + 1, layer * width
                blockers[i] = sorted({rng.randint(low, high) for _ in range(3)})
    elif topology == "roots_chain":
        roots = tasks // 5
        for i in ids:
            if i == roots + 1:
                blockers[i] = list(range(1, roots + 1))
            elif i > roots + 1:
                blockers[i] = [i - 1]

    items, done = {}, set()
    for i in ids:
        task = item(i, True, text(rng, rng.randint(60, 160)), stamp0 + i)
        if rng.random() < 0.15:
            task["priority"] = rng.choice([2, 3])
        if rng.random() < 0.05:
            task["dueDate"] = f"2026-{rng.randint(9, 12):02d}-{rng.randint(1, 28):02d}"
        waits = blockers.get(i, [])
        if waits:
            task["blockedBy"] = [uid(b) for b in waits]
        if topology == "random" and rng.random() < done_ratio and all(b in done for b in waits):
            task["isComplete"] = True
            done.add(i)
        items[str(i)] = task
    for k in range(notes):
        i = tasks + k + 1
        note = item(i, False, text(rng, rng.randint(200, 1200)), stamp0 + i)
        if rng.random() < 0.3:
            note["attachedTo"] = uid(rng.randint(1, tasks))
        items[str(i)] = note
    return items


BOARDS = {
    "real_like": (75, 75, "random"),
    "r1k": (500, 500, "random"),
    "r5k": (2500, 2500, "random"),
    "chain5k": (5000, 0, "chain"),
    "r20k": (10000, 10000, "random"),
    "layered10k": (10000, 0, "layered"),
    "roots_chain10k": (10000, 0, "roots_chain"),
}
QUICK = ["real_like", "r1k", "r5k", "chain5k"]


def build(name):
    tasks, notes, topology = BOARDS[name]
    path = scratch(name)
    os.makedirs(os.path.dirname(storage_file(path)))
    with open(storage_file(path), "w") as f:
        json.dump(generate(tasks, notes, topology), f, indent=4)
    return path


def median_call(m, name, reps, **arguments):
    runs = sorted((m.call(name, **arguments) for _ in range(reps)), key=lambda run: run[2])
    text, is_error, seconds = runs[len(runs) // 2]
    return seconds, text, is_error


def bench(name):
    tasks, notes, topology = BOARDS[name]
    path = build(name)
    m = Mcp(path)
    row = {"board": name, "tasks": tasks, "notes": notes, "topology": topology}
    row["file_kb"] = os.path.getsize(storage_file(path)) // 1024
    reps = 3 if tasks + notes <= 5000 else 1
    for label, tool, arguments in [
        ("prime", "prime", {}),
        ("next10", "next", {"limit": 10}),
        ("context", "context", {"item": 1}),
        ("search_words", "search", {"text": "kappa lambda"}),
        ("search_ready", "search", {"filters": ["ready"]}),
        ("search_all", "search", {}),
    ]:
        seconds, out, is_error = median_call(m, tool, reps, **arguments)
        row[f"{label}_ms"] = round(seconds * 1000, 1)
        row[f"{label}_kb"] = round(size(out) / 1024, 1)
        if label == "prime":
            row["prime_chars"] = len(out)
        if is_error:
            row[f"{label}_error"] = out[:100]
    cursor = cursor_of(m.call("prime")[0])
    seconds, _, _ = median_call(m, "changes", reps, since=cursor)
    row["changes_ms"] = round(seconds * 1000, 1)

    row["create_ms"] = round(m.call("create", text="probe task")[2] * 1000, 1)
    rng = random.Random(11)
    ops = [{"op": "create", "text": f"batch {k}", "blocked_by": [rng.randint(1, tasks)]} for k in range(50)]
    out, is_error, seconds = m.call("batch", ops=ops)
    row["batch50_ms"] = round(seconds * 1000, 1)
    if is_error:
        row["batch50_error"] = out[:120]
    m.close()

    start = time.perf_counter()
    subprocess.run([EKKO, "--prime"], env=env_for(path), capture_output=True)
    row["hook_prime_ms"] = round((time.perf_counter() - start) * 1000, 1)
    return row


def main():
    arguments = [arg for arg in sys.argv[1:] if arg != "--full"]
    names = arguments or (list(BOARDS) if "--full" in sys.argv else QUICK)
    unknown = [name for name in names if name not in BOARDS]
    if unknown:
        raise SystemExit(f"unknown boards {unknown}; known: {', '.join(BOARDS)}")
    print(provenance())
    rows = []
    for name in names:
        row = bench(name)
        rows.append(row)
        print(json.dumps(row), flush=True)

    columns = ["board", "prime_ms", "prime_chars", "next10_ms", "context_ms", "search_all_ms", "search_all_kb", "create_ms", "batch50_ms", "hook_prime_ms"]
    print()
    print("  ".join(f"{column:>14}" for column in columns))
    for row in rows:
        print("  ".join(f"{str(row.get(column, '')):>14}" for column in columns))
    out = results_file("scale")
    with open(out, "w") as f:
        json.dump({"provenance": provenance(), "boards": board_dir(""), "rows": rows}, f, indent=2)
    print(f"results: {out}")


if __name__ == "__main__":
    main()
