"""Task 915, a probe: do the board's own links group its items into topics,
with no model? Edges: a note attached to a task, a note superseding another, a
task blocked by another, and an id cited in the text ('task 983'). Grouped by
weighted label propagation, seeded so a rerun gives the same topics; a probe
for the design, not ekko docs itself.

    nix develop -c python3 evals/docs/topics.py .ekko/storage/storage.json
"""
import collections, json, random, re, sys

items = json.load(open(sys.argv[1]))
live = {v["_id"]: v for v in items.values() if not v.get("stashed")}
edges = collections.defaultdict(collections.Counter)
by_uid = {v["uid"]: v["_id"] for v in live.values() if v.get("uid")}

def link(a, b, w=1):
    if a in live and b in live and a != b:
        edges[a][b] += w
        edges[b][a] += w

REF = re.compile(r"(?i)\b(?:task|note|decision|gotcha|procedure|question)s?\s+(\d+)")
for i, v in live.items():
    attached = v.get("attachedTo") or []
    for t in ([attached] if isinstance(attached, str) else attached):
        link(i, by_uid.get(t, t) if isinstance(t, str) else t, 3)
    s = v.get("supersedes")
    if s:
        link(i, by_uid.get(s, s) if isinstance(s, str) else s, 3)
    for t in v.get("blockedBy") or []:
        link(i, by_uid.get(t, t) if isinstance(t, str) else t, 2)
    for m in REF.finditer(str(v.get("description", ""))):
        link(i, int(m.group(1)))

# label propagation, weighted, a fixed seed so a rerun gives the same topics
random.seed(915)
label = {i: i for i in live}
order = list(live)
for _ in range(30):
    random.shuffle(order)
    moved = 0
    for i in order:
        if not edges[i]:
            continue
        votes = collections.Counter()
        for j, w in edges[i].items():
            votes[label[j]] += w
        best = max(votes.values())
        pick = min(l for l, c in votes.items() if c == best)
        if pick != label[i]:
            label[i], moved = pick, moved + 1
    if not moved:
        break

groups = collections.defaultdict(list)
for i, l in label.items():
    groups[l].append(i)
sizes = sorted((len(g) for g in groups.values()), reverse=True)
alone = sum(1 for i in live if not edges[i])
print(f"items {len(live)}, linked {len(live) - alone}, alone {alone}, edges {sum(len(e) for e in edges.values()) // 2}")
print(f"topics with 5+ items: {sum(1 for s in sizes if s >= 5)}, sizes of the largest: {sizes[:12]}")
for l, g in sorted(groups.items(), key=lambda kv: -len(kv[1]))[:14]:
    tasks = sorted((i for i in g if live[i].get("_isTask")), key=lambda i: -sum(edges[i].values()))
    kinds = collections.Counter(live[i].get("knowledge") or ("task" if live[i].get("_isTask") else "note") for i in g)
    title = lambda i: str(live[i]["description"]).split("\n")[0][:70]
    print(f"\n[{len(g)} items: {dict(kinds)}]")
    for i in tasks[:3]:
        print(f"   {i}. {title(i)}")
