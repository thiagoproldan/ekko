"""Task 507: the large context rewrites -- after how long idle, with or
without a handoff written before, and what starting over from one would have
cost instead (note 407's orientation, 331k units)."""
import sys, datetime, statistics, collections
from replay import scan, calls_left, LOCAL, W_CW1H, W_CW5M, ekko_op
since = datetime.datetime(2026, 8, 25, tzinfo=LOCAL)
events_out = []
for path, events in scan(since):
    calls_left(events)
    prev = None; handoff_at = None; prompted = False
    for ev in events:
        if ev[0] == "start":
            prev = None if ev[1] in ("startup", "clear") else prev
        elif ev[0] == "compact":
            prev = None
        elif ev[0] == "prompt":
            prompted = True
        elif ev[0] == "call":
            call = ev[1]
            for name, _, args in call.uses:
                if ekko_op(name) in ("create", "batch") and "handoff" in str(args).lower()[:4000]:
                    handoff_at = call.at
            if prev is not None and call.ctx >= 100_000 and (call.cw1h + call.cw5m) >= 0.8 * call.ctx:
                gap = (call.at - prev.at).total_seconds() / 60
                recent = handoff_at is not None and (prev.at - handoff_at).total_seconds() < 3 * 3600
                events_out.append((gap, call.ctx, W_CW1H * call.cw1h + W_CW5M * call.cw5m, recent, prompted, path))
            prev = call; prompted = False
print(f"{len(events_out)} large rewrites after an earlier call in the same context")
buckets = collections.Counter()
for gap, ctx, cost, recent, prompted, path in events_out:
    b = "<5m" if gap < 5 else "5-60m" if gap < 60 else "1-3h" if gap < 180 else "3-12h" if gap < 720 else ">12h"
    buckets[b] += 1
print("gap before the rewrite:", dict(buckets))
cold = [e for e in events_out if e[0] >= 60]
print(f"after 60+ minutes idle: {len(cold)}, {sum(e[2] for e in cold)/1e6:.1f}M units, median context {statistics.median(e[1] for e in cold)/1e3:.0f}k")
print(f"   with a handoff written in the 3h before going idle: {sum(1 for e in cold if e[3])}")
print(f"   the rewrite came with a typed prompt: {sum(1 for e in cold if e[4])}")
warm = [e for e in events_out if e[0] < 60]
print(f"under 60 minutes: {len(warm)}, {sum(e[2] for e in warm)/1e6:.1f}M units (a model or tool change, a 5-minute cache that lapsed, a resume)")
# what starting over from a prime would have cost instead: a fresh prefix of ~45k written, plus note 407's orientation
fresh = 2 * 45_000 + 331_000
saved = sum(max(0.0, e[2] - fresh) for e in cold)
print(f"if each cold one had started over from a handoff instead (write ~45k, orientation 331k units, note 407): ~{saved/1e6:.0f}M units saved at most")
