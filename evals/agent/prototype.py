"""Prototype eval: the rigor plan's graph math and search, done in Python on
the same boards the binary is measured on. Not ekko -- a reference to check
the Rust against, and a floor to beat: when an interpreted prototype outruns
the compiled binary, the difference is the algorithm.

    nix develop -c python3 evals/agent/prototype.py

Needs the scale boards (evals/agent/scale.py) for the timing section; each
section that finds nothing to read says so and moves on. Real boards are only
read: copied into scratch, or asked for their prime.
"""

import datetime
import math
import os
import re
import subprocess
import time
import unicodedata
from collections import defaultdict, deque

from ekko_mcp import EKKO, EKKO_HOME, Mcp, board_dir, copy_real, load, provenance, registered_projects, scratch, storage_file

# ---- the graph -----------------------------------------------------------


def state(item):
    if not item.get("_isTask"):
        return None
    if item.get("cancelled"):
        return "cancelled"
    if item.get("isComplete"):
        return "done"
    if item.get("inProgress"):
        return "progress"
    if item.get("paused"):
        return "paused"
    return "pending"


def holds(item):
    return item.get("trashed") is None and state(item) in ("pending", "progress", "paused")


def visible(item):
    return item.get("stashed") is None and item.get("trashed") is None


class Graph:
    """uids interned to dense indices once, adjacency in both directions."""

    def __init__(self, data):
        self.data = data
        self.ids = sorted(data)
        self.at = {i: k for k, i in enumerate(self.ids)}
        by_uid = {item.get("uid"): self.at[i] for i, item in data.items() if item.get("uid")}
        n = len(self.ids)
        self.blockers = [[] for _ in range(n)]
        self.dependents = [[] for _ in range(n)]
        for i, item in data.items():
            for uid in item.get("blockedBy") or []:
                b = by_uid.get(uid)
                if b is not None:
                    self.blockers[self.at[i]].append(b)
                    self.dependents[b].append(self.at[i])
        self.open = [holds(data[i]) for i in self.ids]
        self.indegree = [sum(self.open[b] for b in self.blockers[v]) if self.open[v] else 0 for v in range(n)]

    def topological_open(self):
        """Kahn over the open subgraph: blockers before what they block."""
        remaining = list(self.indegree)
        queue = deque(v for v in range(len(self.ids)) if self.open[v] and remaining[v] == 0)
        order = []
        while queue:
            v = queue.popleft()
            order.append(v)
            for w in self.dependents[v]:
                if self.open[w]:
                    remaining[w] -= 1
                    if remaining[w] == 0:
                        queue.append(w)
        return order

    def descendant_counts(self, order):
        """Exact open work waiting on every open task, through open tasks only:
        bitset union in reverse topological order, O(V * E / wordsize)."""
        reach = [0] * len(self.ids)
        for v in reversed(order):
            acc = 0
            for w in self.dependents[v]:
                if self.open[w]:
                    acc |= reach[w] | (1 << w)
            reach[v] = acc
        return [r.bit_count() for r in reach]

    def propagate(self, order):
        """Inherited priority (max), effective deadline (min, the CPM backward
        pass with one-day durations) and the longest chain gated, in one
        reverse pass. Exact on a DAG: max and min never count a diamond twice."""
        n = len(self.ids)
        priority, deadline, depth = [0] * n, [math.inf] * n, [0] * n
        for v in reversed(order):
            item = self.data[self.ids[v]]
            p = item.get("priority") or 1
            due = datetime.date.fromisoformat(item["dueDate"]).toordinal() if item.get("dueDate") else math.inf
            d = 0
            for w in self.dependents[v]:
                if self.open[w]:
                    p, due, d = max(p, priority[w]), min(due, deadline[w] - 1), max(d, depth[w] + 1)
            priority[v], deadline[v], depth[v] = p, due, d
        return priority, deadline, depth

    def roots_above(self, item_id):
        start = self.at[item_id]
        seen, stack, roots = {start}, [start], []
        while stack:
            v = stack.pop()
            above = [b for b in self.blockers[v] if self.open[b]]
            if not above and v != start:
                roots.append(self.ids[v])
            for b in above:
                if b not in seen:
                    seen.add(b)
                    stack.append(b)
        return sorted(roots)


# ---- urgency (Taskwarrior's coefficients on propagated inputs) ------------

PRIORITY_WEIGHT = {3: 6.0, 2: 3.9, 1: 0.0}


def due_ramp(deadline, today):
    """Task::urgency_due in Taskwarrior: 21 days mapped onto 0.2..1.0."""
    if deadline == math.inf:
        return 0.0
    overdue = today - deadline
    if overdue >= 7:
        return 1.0
    if overdue >= -14:
        return (overdue + 14) * 0.8 / 21 + 0.2
    return 0.2


def urgency_order(data, today):
    g = Graph(data)
    order = g.topological_open()
    priority, deadline, depth = g.propagate(order)
    ready = [v for v in range(len(g.ids)) if g.open[v] and visible(data[g.ids[v]]) and (g.indegree[v] == 0 or state(data[g.ids[v]]) == "progress")]

    def score(v):
        return 12.0 * due_ramp(deadline[v], today) + (8.0 if depth[v] >= 1 else 0.0) + PRIORITY_WEIGHT[priority[v]]

    ready.sort(key=lambda v: (state(data[g.ids[v]]) != "progress", -score(v), g.ids[v]))
    return [(g.ids[v], round(score(v), 1)) for v in ready]


# ---- search --------------------------------------------------------------


def fold(text):
    text = unicodedata.normalize("NFKD", text.lower())
    return "".join(ch for ch in text if not unicodedata.combining(ch))


def stem(token):
    for suffix in ("ies", "ing", "ed", "es", "s"):
        if len(token) > len(suffix) + 3 and token.endswith(suffix):
            return token[: -len(suffix)]
    return token


def terms(text):
    return [stem(token) for token in re.findall(r"[a-z0-9]+", fold(text))]


class Bm25:
    """Lucene's BM25: idf = ln(1 + (N - n + 0.5) / (n + 0.5)), k1 = 1.2, b = 0.75."""

    def __init__(self, docs, k1=1.2, b=0.75):
        self.docs, self.k1, self.b = docs, k1, b
        self.tf, self.df, self.length = {}, defaultdict(int), {}
        for i, text in docs.items():
            counts = defaultdict(int)
            for term in terms(text):
                counts[term] += 1
            self.tf[i], self.length[i] = counts, sum(counts.values())
            for term in counts:
                self.df[term] += 1
        self.average = sum(self.length.values()) / max(1, len(self.length))
        self.n = len(docs)

    def search(self, query, k=3):
        wanted = terms(query)
        scores = {}
        for i, counts in self.tf.items():
            total = 0.0
            for term in wanted:
                f = counts.get(term, 0)
                if f:
                    idf = math.log(1 + (self.n - self.df[term] + 0.5) / (self.df[term] + 0.5))
                    total += idf * f * (self.k1 + 1) / (f + self.k1 * (1 - self.b + self.b * self.length[i] / self.average))
            if total > 0:
                scores[i] = total
        return sorted(scores.items(), key=lambda kv: -kv[1])[:k], len(scores)


def timed(fn, *args):
    start = time.perf_counter()
    out = fn(*args)
    return out, (time.perf_counter() - start) * 1000


def main():
    print(provenance())
    today = datetime.date.today().toordinal()

    print("\n== graph math on the scale boards, all counts at once")
    for name in ("chain5k", "layered10k", "roots_chain10k", "r20k"):
        if not os.path.exists(storage_file(board_dir(name))):
            print(f"  {name}: not generated (evals/agent/scale.py --full)")
            continue
        data, t_parse = timed(load, board_dir(name))
        g, t_graph = timed(Graph, data)
        order, t_topo = timed(g.topological_open)
        counts, t_counts = timed(g.descendant_counts, order)
        _, t_propagate = timed(g.propagate, order)
        total = t_parse + t_graph + t_topo + t_counts + t_propagate
        print(
            f"  {name}: parse {t_parse:.0f} ms, graph {t_graph:.0f}, topological {t_topo:.0f}, "
            f"all descendant counts {t_counts:.0f}, propagation {t_propagate:.0f} -- total {total:.0f} ms, "
            f"most waiting on one task {max(counts, default=0)}"
        )

    print("\n== the priority inversion, ordered by urgency on propagated inputs")
    path = scratch("prototype-inversion")
    m = Mcp(path)
    tomorrow = time.strftime("%Y-%m-%d", time.localtime(time.time() + 86400))
    m.call(
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
    m.close()
    print(f"  {urgency_order(load(path), today)}   (plan target: 1, 5, 3, 4)")

    project = "ekko" if "ekko" in registered_projects() else None
    try:
        path, label = copy_real("prototype-real", project)
    except SystemExit as missing:
        print(f"\n== real board: {missing}")
        return
    data = load(path)
    g = Graph(data)
    print(f"\n== a copy of {label}")
    print(f"  urgency order, first 8: {urgency_order(data, today)[:8]}")
    blocked = [i for i in data if g.open[g.at[i]] and visible(data[i]) and g.indegree[g.at[i]] > 0]
    for item in blocked[:5]:
        roots, ms = timed(g.roots_above, item)
        print(f"  roots above {item}: {roots} ({ms:.2f} ms)")
    index, ms = timed(Bm25, {i: item["description"] for i, item in data.items() if visible(item)})
    print(f"  BM25 index over {index.n} visible items in {ms:.1f} ms")
    for query in ("dependency cycle", "check cycle", "cycle", "why is the timeline shelved", "decisao"):
        (hits, total), ms = timed(index.search, query)
        print(f"  {query!r}: {total} hits in {ms:.1f} ms, top {[i for i, _ in hits]}")

    print("\n== prime size in characters against the hook's 10,000 cap (real boards, read only)")
    boards = [("default board", ["--ekko-dir", EKKO_HOME])] + [(name, ["--project", name]) for name in registered_projects()]
    for name, scope in boards:
        out = subprocess.run([EKKO, *scope, "--prime"], capture_output=True, text=True)
        if out.returncode != 0 or not out.stdout.strip():
            print(f"  {name}: no prime ({out.stderr.strip()[:60]})")
            continue
        print(f"  {name}: {len(out.stdout)} characters, {len(out.stdout.encode())} bytes")


if __name__ == "__main__":
    main()
