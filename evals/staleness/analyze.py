"""Task 2197, part A's tables, from starts.jsonl.

    nix develop -c python3 evals/staleness/analyze.py
"""

import collections
import json
import os
import statistics

import config as C
rows = [json.loads(line) for line in open(os.path.join(C.DATA, "starts.jsonl"))]


def q(values, p):
    values = sorted(v for v in values if v is not None)
    if not values:
        return None
    return values[min(len(values) - 1, int(round(p * (len(values) - 1))))]


def describe(values):
    values = [v for v in values if v is not None]
    if not values:
        return "n 0"
    return f"n {len(values)}, median {q(values, .5)}, q1 {q(values, .25)}, q3 {q(values, .75)}, p90 {q(values, .9)}, max {max(values)}, mean {statistics.fmean(values):.1f}"


def stale_commit(r):
    return (r.get("commits_since") or 0) > 0


def stale_task(r):
    return bool(r.get("named_changed"))


def stale(r):
    return stale_commit(r) or stale_task(r)


known = [r for r in rows if "commits_since" in r and "named_changed" in r]
print(f"starts with a handoff: {len(rows)}; with both a repository and a board read: {len(known)}")
print("by source:", dict(collections.Counter(r["source"] for r in rows)))
print("whose handoff:", dict(collections.Counter(r.get("whose") for r in rows)))
print()

print("== prevalence of a mismatch between the handoff and the world, by source")
for source in ("startup", "clear", "resume", "compact", None):
    group = [r for r in known if source is None or r["source"] == source]
    if not group:
        continue
    n = len(group)
    sc = sum(map(stale_commit, group))
    st = sum(map(stale_task, group))
    sa = sum(map(stale, group))
    print(f"  {source or 'all':8} n {n:4}  commits moved {sc:4} ({sc / n:.0%})  named tasks moved {st:4} ({st / n:.0%})  either {sa:4} ({sa / n:.0%})")
new = [r for r in known if r["source"] in ("startup", "clear", "resume")]
n = len(new)
print(f"  new sessions (startup, clear, resume): n {n}, either {sum(map(stale, new))} ({sum(map(stale, new)) / n:.0%})")
print()

print("== how much moved, on new sessions whose handoff was stale")
group = [r for r in new if stale(r)]
print("  commits since:", describe([r.get("commits_since") for r in group]))
print("  named tasks changed:", describe([len(r.get("named_changed") or {}) for r in group]))
print("  tags since:", describe([len(r.get("tags_since") or []) for r in group]))
print("  tasks created since:", describe([r.get("tasks_created_since") for r in group]))
print("  age, hours:", describe([r.get("age_h") for r in group]))
print("  age of fresh ones, hours:", describe([r.get("age_h") for r in new if not stale(r)]))
print()

print("== distinct handoffs shown, and how many starts each")
shown = collections.Counter((r["board"], r["handoff"]) for r in rows)
print("  distinct:", len(shown), "; starts per handoff:", describe(list(shown.values())))
for (bd, h), count in shown.most_common(8):
    sample = [r for r in rows if r["board"] == bd and r["handoff"] == h]
    st = sum(map(stale, [r for r in sample if r in known]))
    print(f"   {count:3} starts  handoff {h} on {os.path.basename(os.path.dirname(bd)) or bd}  stale in {st}  ages {q([r.get('age_h') for r in sample], 0)}..{q([r.get('age_h') for r in sample], 1)} h")
print()

print("== orientation before the first productive action, new sessions, stale against fresh")
keys = ["R_calls", "R_units", "R_seconds", "E_calls", "E_seconds", "git_looks", "git_looks_before_edit", "git_look_chars", "ekko_reads", "ekko_read_chars", "tool_calls_before", "result_chars_before"]
for label, group in (("stale", [r for r in new if stale(r)]), ("fresh", [r for r in new if not stale(r)])):
    print(f"  {label}: n {len(group)}")
    for key in keys:
        print(f"    {key:22} {describe([r.get(key) for r in group])}")
print()

print("== first productive kind")
for label, group in (("stale", [r for r in new if stale(r)]), ("fresh", [r for r in new if not stale(r)])):
    print(f"  {label}:", dict(collections.Counter(r.get("first_kind") for r in group)))
print()

print("== corrections in the first six prompts")
for label, group in (("stale", [r for r in new if stale(r)]), ("fresh", [r for r in new if not stale(r)])):
    hits = [r for r in group if r.get("corrections")]
    print(f"  {label}: {len(hits)} of {len(group)}")
for r in [r for r in new if r.get("corrections")]:
    for text in r["corrections"]:
        print(f"    [{'stale' if stale(r) else 'fresh'}] {r['file']} {r['at']} h{r['handoff']}: {text[:160]!r}")
print()

print("== hook times as Claude Code measured them (durationMs), every start")
for hook in ("prime", "tasklist", "memory", "ctx", "graff"):
    print(f"  {hook:9} ms {describe([(r['hooks'].get(hook) or {}).get('ms') for r in rows])}")
    print(f"  {hook:9} chars {describe([(r['hooks'].get(hook) or {}).get('chars') for r in rows])}")
print("  prime handoff block chars:", describe([r.get("handoff_chars") for r in rows]))
print()

print("== first prompts of new sessions (folded kinds)")
def kind(text):
    t = (text or "").lower()
    if not t:
        return "none"
    if any(w in t for w in ("onde paramos", "continu", "continue", "where did we", "seguir", "retom", "prossig", "go on", "resume")):
        return "resume-ask"
    return "other"
print("  ", dict(collections.Counter(kind(r.get("first_prompt")) for r in new)))
for label, group in (("stale", [r for r in new if stale(r)]), ("fresh", [r for r in new if not stale(r)])):
    for k in ("resume-ask", "other"):
        sub = [r for r in group if kind(r.get("first_prompt")) == k]
        print(f"  {label} {k}: n {len(sub)}; R_calls {describe([r.get('R_calls') for r in sub])}")
        print(f"  {label} {k}: n {len(sub)}; R_units {describe([r.get('R_units') for r in sub])}")
        print(f"  {label} {k}: n {len(sub)}; git_looks {describe([r.get('git_looks') for r in sub])}")
