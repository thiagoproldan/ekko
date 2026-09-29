"""Task 507: the large context rewrites -- after how long idle, with or
without a handoff written before, and what starting over from one would have
cost instead (note 407's orientation, 331k units).

    nix develop -c python3 evals/recall/cold.py [--since 2026-08-25] [--until 2026-09-27T22:33:16] [--list]

Task 540 re-counts these after ctx's cold-return guard went live: --since its
go-live, --until another change's go-live to split the count (note 864)."""
import sys, datetime, statistics, collections, argparse
from replay import scan, calls_left, LOCAL, W_CW1H, W_CW5M, ekko_op
parser = argparse.ArgumentParser()
parser.add_argument("--since", default="2026-08-25", help="local time")
parser.add_argument("--until", help="local time: only the rewrites before it")
parser.add_argument("--list", action="store_true", help="each cold rewrite, with its session")
opts = parser.parse_args()
since = datetime.datetime.fromisoformat(opts.since).replace(tzinfo=LOCAL)
until = datetime.datetime.fromisoformat(opts.until).replace(tzinfo=LOCAL) if opts.until else None
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
            if prev is not None and (until is None or call.at < until) and call.ctx >= 100_000 and (call.cw1h + call.cw5m) >= 0.8 * call.ctx:
                gap = (call.at - prev.at).total_seconds() / 60
                recent = handoff_at is not None and (prev.at - handoff_at).total_seconds() < 3 * 3600
                events_out.append((gap, call.ctx, W_CW1H * call.cw1h + W_CW5M * call.cw5m, recent, prompted, path, call.at))
            prev = call; prompted = False
print(f"since {opts.since}{' until ' + opts.until if until else ''}: {len(events_out)} large rewrites after an earlier call in the same context")
buckets = collections.Counter()
for gap, ctx, cost, recent, prompted, path, at in events_out:
    b = "<5m" if gap < 5 else "5-60m" if gap < 60 else "1-3h" if gap < 180 else "3-12h" if gap < 720 else ">12h"
    buckets[b] += 1
print("gap before the rewrite:", dict(buckets))
cold = [e for e in events_out if e[0] >= 60]
median = f"{statistics.median(e[1] for e in cold)/1e3:.0f}k" if cold else "-"
print(f"after 60+ minutes idle: {len(cold)}, {sum(e[2] for e in cold)/1e6:.1f}M units, median context {median}")
print(f"   with a handoff written in the 3h before going idle: {sum(1 for e in cold if e[3])}")
print(f"   the rewrite came with a typed prompt: {sum(1 for e in cold if e[4])}")
print(f"   at 250k or more, ctx's cold-return threshold: {sum(1 for e in cold if e[1] >= 250_000)}")
if opts.list:
    for gap, ctx, cost, recent, prompted, path, at in cold:
        print(f"   {at.astimezone(LOCAL):%m-%d %H:%M} idle {gap:.0f}m, {ctx/1e3:.0f}k, {cost/1e3:.0f}k units, handoff {'yes' if recent else 'no'}, prompt {'yes' if prompted else 'no'}, {path.rsplit('/', 1)[-1][:8]}")
warm = [e for e in events_out if e[0] < 60]
print(f"under 60 minutes: {len(warm)}, {sum(e[2] for e in warm)/1e6:.1f}M units (a model or tool change, a 5-minute cache that lapsed, a resume)")
# what starting over from a prime would have cost instead: a fresh prefix of ~45k written, plus note 407's orientation
fresh = 2 * 45_000 + 331_000
saved = sum(max(0.0, e[2] - fresh) for e in cold)
print(f"if each cold one had started over from a handoff instead (write ~45k, orientation 331k units, note 407): ~{saved/1e6:.0f}M units saved at most")
