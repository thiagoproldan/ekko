"""Task 2197: does a handoff's age tell a stale one from a fresh one? The
prime today shows only its date (an age cue). AUC of age as the score, by the
Mann-Whitney count; the user's own new sessions; labels from starts.py
(commits since, or a named task closed since)."""

import json
import os
import statistics
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import config as C  # noqa: E402
from classes import kind  # noqa: E402

rows = [json.loads(line) for line in open(os.path.join(C.DATA, "starts.jsonl"))]
user = [r for r in rows if r["source"] in ("startup", "clear", "resume") and kind(r.get("first_prompt")) == "user"
        and "named_changed" in r and "commits_since" in r and r.get("age_h") is not None]


def stale(r):
    return (r.get("commits_since") or 0) > 0 or bool(r.get("named_changed"))


pos = [r["age_h"] for r in user if stale(r)]
neg = [r["age_h"] for r in user if not stale(r)]
wins = sum((p > n) + 0.5 * (p == n) for p in pos for n in neg)
auc = wins / (len(pos) * len(neg))
print(f"stale {len(pos)}, fresh {len(neg)}: AUC of age = {auc:.3f}")
for cut in (1, 6, 24, 72):
    tp = sum(a >= cut for a in pos)
    fp = sum(a >= cut for a in neg)
    print(f"  flag when age >= {cut:>3} h: catches {tp}/{len(pos)} stale ({tp / len(pos):.0%}), false alarms on {fp}/{len(neg)} fresh ({fp / len(neg):.0%}), precision {tp / max(1, tp + fp):.0%}")
later = [r.get("later_calls", 0) for r in user]
print(f"later calls per start: median {statistics.median(later)}, mean {statistics.fmean(later):.1f}")
bill = [r.get("seg_units", 0) for r in user]
print(f"segment bill per start (units, model's weights): median {statistics.median(bill):,.0f}, mean {statistics.fmean(bill):,.0f}")
