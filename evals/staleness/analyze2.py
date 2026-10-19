"""Task 2197, part A by who started the session: prevalence, what moved,
orientation and corrections for the user's own sessions; the handoff shown to
born sessions."""

import collections
import json
import os
import statistics
import sys

import config as C
from classes import born_tasks, kind, resume_ask

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "claude-code"))
from transcripts import read_weight  # noqa: E402

rows = [json.loads(line) for line in open(os.path.join(C.DATA, "starts.jsonl"))]
CPT = 2.59


def w(r):
    """The read weight of the model the start ran (gotcha 2198)."""
    return read_weight(r.get("model"))


def q(values, p):
    values = sorted(v for v in values if v is not None)
    return values[min(len(values) - 1, int(round(p * (len(values) - 1))))] if values else None


def describe(values):
    values = [v for v in values if v is not None]
    if not values:
        return "n 0"
    return f"n {len(values)}, median {q(values, .5)}, q1 {q(values, .25)}, q3 {q(values, .75)}, p90 {q(values, .9)}, mean {statistics.fmean(values):.1f}"


def stale(r):
    return (r.get("commits_since") or 0) > 0 or bool(r.get("named_changed"))


known = [r for r in rows if "commits_since" in r and "named_changed" in r]
print("starts by who typed the first prompt:", dict(collections.Counter(kind(r.get("first_prompt")) for r in rows)))
print("by who and source:", dict(collections.Counter((kind(r.get("first_prompt")), r["source"]) for r in rows)))
print()
user = [r for r in known if kind(r.get("first_prompt")) == "user" and r["source"] in ("startup", "clear", "resume")]
n = len(user)
print(f"== the user's new sessions: n {n}")
for source in ("startup", "clear", "resume"):
    g = [r for r in user if r["source"] == source]
    s = sum(map(stale, g))
    print(f"  {source:8} n {len(g):3}, stale {s:3} ({s / len(g):.0%})")
s = sum(map(stale, user))
print(f"  all      n {n:3}, stale {s:3} ({s / n:.0%})")
for label, g in (("resume asks", [r for r in user if resume_ask(r.get("first_prompt"))]), ("other prompts", [r for r in user if not resume_ask(r.get("first_prompt"))])):
    s = sum(map(stale, g))
    print(f"  {label}: n {len(g)}, stale {s} ({s / len(g):.0%})")
print("  commits since, stale ones:", describe([r.get("commits_since") for r in user if stale(r)]))
print("  named tasks closed since, stale ones:", describe([len(r.get("named_changed") or {}) for r in user if stale(r)]))
print("  age h, stale:", describe([r.get("age_h") for r in user if stale(r)]), "| fresh:", describe([r.get("age_h") for r in user if not stale(r)]))
print()
print("== orientation, the user's new sessions, by first prompt and staleness")
keys = ("R_calls", "R_units", "R_seconds", "git_looks", "git_look_units", "ekko_reads", "result_chars_before")
for ask in (True, False):
    for st in (True, False):
        g = [r for r in user if resume_ask(r.get("first_prompt")) == ask and stale(r) == st]
        print(f"  {'resume ask' if ask else 'other':10} {'stale' if st else 'fresh'}: n {len(g)}")
        for key in keys:
            print(f"     {key:20} {describe([r.get(key) for r in g])}")
print()
print("== git looks before the first productive action: the share of starts with any")
for ask in (True, False):
    for st in (True, False):
        g = [r for r in user if resume_ask(r.get("first_prompt")) == ask and stale(r) == st]
        a = sum(1 for r in g if (r.get("git_looks") or 0) > 0)
        print(f"  {'resume ask' if ask else 'other':10} {'stale' if st else 'fresh'}: {a} of {len(g)}")
print()
born = [r for r in rows if kind(r.get("first_prompt")) == "born"]
unrelated = [r for r in born if r["task"] not in born_tasks(r.get("first_prompt")) and r["handoff"] not in born_tasks(r.get("first_prompt"))]
print(f"== born sessions: {len(born)} starts with a handoff in the prime; on a task other than theirs: {len(unrelated)}")
chars = [r.get("handoff_chars") or 0 for r in unrelated]
price = [(c + 150) / CPT * (2 + w(r) * r.get("later_calls", 0)) for c, r in zip(chars, unrelated)]
print(f"  the handoff block, chars: {describe(chars)}")
print(f"  its price per born session, units: {describe([round(p) for p in price])}; sum {sum(price):,.0f}")
print(f"  share of those sessions' bill: {sum(price) / max(1, sum(r.get('seg_units', 0) for r in unrelated)):.3%}")
print(f"  stale among them: {sum(map(stale, [r for r in unrelated if r in known]))} of {len(unrelated)}")

print()
print("== bounds: a named task whose close no record settles counted as moved (upper) or not (lower)")
def stale_upper(r):
    return stale(r) or bool(r.get("named_unknown"))
for label, g in (("all", user), ("startup", [r for r in user if r["source"] == "startup"]), ("clear", [r for r in user if r["source"] == "clear"]),
                 ("resume", [r for r in user if r["source"] == "resume"]), ("resume asks", [r for r in user if resume_ask(r.get("first_prompt"))]),
                 ("other prompts", [r for r in user if not resume_ask(r.get("first_prompt"))])):
    lo, hi = sum(map(stale, g)), sum(map(stale_upper, g))
    print(f"  {label:14} n {len(g):3}: stale {lo} to {hi} ({lo / len(g):.0%} to {hi / len(g):.0%}); commits moved alone {sum(1 for r in g if (r.get('commits_since') or 0) > 0)}")
