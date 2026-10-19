"""Task 2197: what the staleness line would cost on each start, against the
most it could save there, in units of one uncached input token.

    nix develop -c python3 evals/staleness/economics.py

The line, as the reduced 1433 would print it under the handoff:
  B1 (only when something moved):
    "Since it was written, at f9564f4 on main: 39 commits, released v0.43.0 to
     v0.44.3; of the tasks it names, closed since: 1746, 1755, 1800."
  B2 (always): B1, or "Nothing moved since it was written, at f9564f4." when
    nothing did.
Its price on a start: its tokens (2.59 characters each, note 286) written once
into the cache (2) and read by every later call of the segment (0.1 each).
The most it could save: every git look the session made before its first
productive action (the calls and their results carried, starts.py), had the
line made them all needless. The share it must remove to pay for itself is
f* = (sum of prices) / (sum of savings at most).
"""

import json
import os
import statistics

import config as C

rows = [json.loads(line) for line in open(os.path.join(C.DATA, "starts.jsonl"))]
CPT = 2.59


def w(r):
    """The read weight of the model the start ran (gotcha 2198)."""
    return 0.05 if any(n in (r.get("model") or "") for n in ("opus-5-5", "sonnet-5-5")) else 0.1


def stale(r):
    return (r.get("commits_since") or 0) > 0 or bool(r.get("named_changed"))


def line_chars(r, always):
    if not stale(r):
        return len("Nothing moved since it was written, at f9564f4.") if always else 0
    text = "Since it was written, at f9564f4 on main: "
    n = r.get("commits_since") or 0
    parts = [f"{n} commit{'s' if n != 1 else ''}"] if n else []
    tags = r.get("tags_since") or []
    if len(tags) == 1:
        parts.append(f"released {tags[0]}")
    elif tags:
        parts.append(f"released {tags[0]} to {tags[-1]}")
    text += ", ".join(parts) if parts else "no commits"
    closed = sorted(int(k) for k, v in (r.get("named_changed") or {}).items())
    if closed:
        shown = closed[:8]
        more = f" and {len(closed) - 8} more" if len(closed) > 8 else ""
        text += "; of the tasks it names, closed or reopened since: " + ", ".join(map(str, shown)) + more
    return len(text + ".")


def q(values, p):
    values = sorted(values)
    return values[min(len(values) - 1, int(round(p * (len(values) - 1))))] if values else None


new = [r for r in rows if r["source"] in ("startup", "clear", "resume") and "commits_since" in r and "named_changed" in r]
ps = sum(map(stale, new)) / len(new)
print(f"new sessions with a handoff, both sides known: {len(new)}; stale share p_s = {ps:.3f}")
for label, always in (("B1 only when stale", False), ("B2 always", True)):
    chars = [line_chars(r, always) for r in new]
    price = [c / CPT * (2 + w(r) * r.get("later_calls", 0)) for c, r in zip(chars, new)]
    shown = [c for c in chars if c]
    print(f"\n{label}: shown on {len(shown)} of {len(new)} starts; chars median {q(shown, .5)}, p90 {q(shown, .9)}, max {max(shown)}")
    print(f"  price per start, units: mean {statistics.fmean(price):,.0f}, median {q(price, .5):,.0f}, p90 {q(price, .9):,.0f}, max {max(price):,.0f}")
    total_price = sum(price)
    save = [r.get("git_look_units", 0) for r in new if stale(r)]
    total_save = sum(save)
    print(f"  most it could save, stale starts: mean {statistics.fmean(save):,.0f}, median {q(save, .5):,.0f}, p90 {q(save, .9):,.0f} units")
    print(f"  break-even share f* = {total_price / total_save:.3f} of the stale starts' git looks")
    seg = sum(r.get("seg_units", 0) for r in new)
    print(f"  price as a share of these segments' whole bill: {total_price / seg:.5%}; the most it could save: {total_save / seg:.3%}")
fresh_looks = [r.get("git_look_units", 0) for r in new if not stale(r)]
print(f"\nfresh starts' git looks before the first productive action (what B2's 'nothing moved' could spare): mean {statistics.fmean(fresh_looks):,.0f}, sum {sum(fresh_looks):,.0f} units")
print(f"stale starts' git looks: sum {sum(r.get('git_look_units', 0) for r in new if stale(r)):,.0f} units")
print(f"all these segments' bill: {sum(r.get('seg_units', 0) for r in new):,.0f} units over {sum(r.get('later_calls', 0) + 1 for r in new):,} calls")
