"""Is a call's thinking kept in the context the next calls re-read?
Compare the context's growth between two consecutive calls with what entered:
the output with or without its thinking, plus the tool results."""
import sys, os, datetime, statistics, json, glob
import replay
from replay import scan, calls_left, LOCAL, tokens

# thinking tokens per call: scan() keeps no thinking count, so read it again by message id
think = {}
for base in replay.profiles():
    for path in glob.glob(os.path.join(base, "projects", "**", "*.jsonl"), recursive=True):
        with open(path, errors="replace") as fh:
            for raw in fh:
                if '"thinking_tokens"' not in raw: continue
                try: row = json.loads(raw)
                except ValueError: continue
                m = row.get("message") or {}
                u = m.get("usage") or {}
                t = (u.get("output_tokens_details") or {}).get("thinking_tokens")
                if m.get("id") and t is not None: think[m["id"]] = t

since = datetime.datetime(2026, 9, 1, tzinfo=LOCAL)
within, across = [], []
for path, events in scan(since):
    calls_left(events)
    prev, results, prompt_chars, crossed = None, 0.0, 0, False
    for ev in events:
        if ev[0] == "call":
            call = ev[1]
            if prev is not None and call.left == prev.left - 1 and prev.mid in think:
                grew = call.ctx - prev.ctx
                entered = results + tokens(prompt_chars)
                t = think[prev.mid]
                row = (grew - entered, prev.out, prev.out - t, t)
                (across if crossed else within).append(row)
            prev, results, prompt_chars, crossed = call, 0.0, 0, False
        elif ev[0] == "result":
            results += tokens(len(ev[2]))
        elif ev[0] == "prompt":
            prompt_chars += len(ev[2]); crossed = True
        elif ev[0] in ("start", "compact"):
            prev = None

def show(name, rows):
    rows = [r for r in rows if r[3] >= 300]  # calls that thought at least a little
    if not rows: print(name, "none"); return
    kept = [r[0] / r[1] for r in rows]          # growth less inputs, over the whole output
    dropped = [r[0] / max(1, r[2]) for r in rows]  # over the output without thinking
    print(f"{name}: {len(rows)} pairs; median thinking {statistics.median(r[3] for r in rows):.0f} tok, visible {statistics.median(r[2] for r in rows):.0f}")
    print(f"   (growth - inputs) / whole output:      median {statistics.median(kept):.2f}  q1 {statistics.quantiles(kept, n=4)[0]:.2f}  q3 {statistics.quantiles(kept, n=4)[2]:.2f}")
    print(f"   (growth - inputs) / output w/o thinking: median {statistics.median(dropped):.2f}")
    neg = sum(1 for r in rows if r[0] < -0.5 * r[3])
    print(f"   pairs where the context shrank by more than half the thinking: {neg}")
show("WITHIN a tool loop", within)
show("ACROSS a new prompt", across)
