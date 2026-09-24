"""What the model's thinking costs once it stays in the context, and how it
varies with the effort level each call ran at."""
import sys, os, datetime, statistics, json, glob, collections
import replay
from replay import scan, calls_left, LOCAL, W_CR, W_CW1H, W_OUT

meta = {}
for base in replay.profiles():
    for path in glob.glob(os.path.join(base, "projects", "**", "*.jsonl"), recursive=True):
        with open(path, errors="replace") as fh:
            for raw in fh:
                if '"output_tokens"' not in raw: continue
                try: row = json.loads(raw)
                except ValueError: continue
                m = row.get("message") or {}
                u = m.get("usage") or {}
                if m.get("id"):
                    meta[m["id"]] = ((u.get("output_tokens_details") or {}).get("thinking_tokens") or 0, row.get("effort") or "?", m.get("model") or "?")

since = datetime.datetime(2026, 8, 25, tzinfo=LOCAL)
bill = gen_think = kept_think = kept_visible = 0.0
by_effort = collections.defaultdict(lambda: {"calls": 0, "think": [], "out": [], "cost": 0.0, "kept_think": 0.0, "grow": []})
for path, events in scan(since):
    calls_left(events)
    calls = [e[1] for e in events if e[0] == "call"]
    for i, call in enumerate(calls):
        think, effort, model = meta.get(call.mid, (0, "?", "?"))
        think = min(think, call.out)
        bill += call.cost
        gen_think += W_OUT * think
        kept_think += think * (W_CW1H + W_CR * call.left)
        kept_visible += (call.out - think) * (W_CW1H + W_CR * call.left)
        e = by_effort[effort]
        e["calls"] += 1; e["think"].append(think); e["out"].append(call.out); e["cost"] += call.cost
        e["kept_think"] += think * (W_CW1H + W_CR * call.left)
        if i + 1 < len(calls) and calls[i + 1].left == call.left - 1:
            e["grow"].append(calls[i + 1].ctx - call.ctx)
print(f"bill since 08-25: {bill/1e6:.0f}M units")
print(f"  thinking, generated (5x):              {gen_think/1e6:6.1f}M {gen_think/bill:6.1%}")
print(f"  thinking, kept and re-read afterwards: {kept_think/1e6:6.1f}M {kept_think/bill:6.1%}")
print(f"  visible output, kept and re-read:      {kept_visible/1e6:6.1f}M {kept_visible/bill:6.1%}")
print(f"  thinking in all:                       {(gen_think+kept_think)/1e6:6.1f}M {(gen_think+kept_think)/bill:6.1%}")
print("\nBY EFFORT: calls, median/mean thinking per call, mean output, median context growth per call, cost per call, thinking kept share of that effort's cost")
for effort, e in sorted(by_effort.items(), key=lambda kv: -kv[1]["calls"]):
    if e["calls"] < 50: continue
    print(f"  {effort:8s} {e['calls']:6d}  think med {statistics.median(e['think']):5.0f} mean {statistics.fmean(e['think']):5.0f}"
          f"  out mean {statistics.fmean(e['out']):5.0f}  grow med {statistics.median(e['grow']) if e['grow'] else 0:6.0f}"
          f"  cost/call {e['cost']/e['calls']/1e3:5.1f}k  kept-think {e['kept_think']/e['cost']:5.1%}")
