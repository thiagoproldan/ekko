"""Two more parts of the picture: (1) what re-writing a whole context after
the cache expired costs, and (2) whether the user's corrections come more
often as the context grows -- a proxy for quality, not only tokens."""
import sys, os, datetime, statistics, re, collections, math
from replay import scan, calls_left, LOCAL, W_CW1H, W_CW5M

since = datetime.datetime(2026, 8, 25, tzinfo=LOCAL)
FRICTION = re.compile(
    r"(n[aã]o (deu certo|funcionou|era isso|[eé] isso|foi isso|entendi|t[oô] conseguindo|estou conseguindo|pedi|mandei)|\berrad[oa]s?\b|\berrou\b|"
    r"j[aá] (te )?(falei|disse|pedi|expliquei)|voc[eê] (esqueceu|ignorou)|\bcalma\b|nada a ver|continua (errado|igual|dando erro)|"
    r"tem certeza|fica alterando|inventou|mentiu|\bconfus[oa]\b|de novo o mesmo|n[aã]o era para|n[aã]o precisava)",
    re.I)
bill = rewrite_cost = 0.0; rewrites = 0; rewrite_sizes = []
prompts = []  # (context at the prompt, friction?, session, index in session, calls in session)
for path, events in scan(since):
    calls_left(events)
    last_ctx = 0; pending = []
    for ev in events:
        if ev[0] == "call":
            call = ev[1]; bill += call.cost
            if call.ctx >= 100_000 and (call.cw1h + call.cw5m) >= 0.8 * call.ctx:
                rewrites += 1; rewrite_sizes.append(call.ctx)
                rewrite_cost += W_CW1H * call.cw1h + W_CW5M * call.cw5m
            for text in pending:
                prompts.append((call.ctx, bool(FRICTION.search(text[:400])), path, text))
            pending = []
        elif ev[0] == "prompt":
            if not ev[2].lstrip().startswith("<"):
                pending.append(ev[2])
print(f"COLD REWRITES since 08-25: {rewrites} calls re-wrote a context of 100k+ ({statistics.median(rewrite_sizes)/1e3:.0f}k median),"
      f" {rewrite_cost/1e6:.1f}M units, {rewrite_cost/bill:.1%} of the bill")
buckets = [(0, 100e3), (100e3, 200e3), (200e3, 300e3), (300e3, 500e3), (500e3, 1e9)]
print(f"\nFRICTION by context size at the prompt ({len(prompts)} typed prompts):")
for lo, hi in buckets:
    rows = [p for p in prompts if lo <= p[0] < hi]
    if not rows: continue
    k = sum(p[1] for p in rows); n = len(rows); p = k / n
    half = 1.96 * math.sqrt(p * (1 - p) / n)
    print(f"  {lo/1e3:4.0f}-{min(hi, 9e5)/1e3:4.0f}k  {k:4d}/{n:<4d} {p:6.1%}  (+-{half:.1%})")
# within the same session: the first half of its prompts against the second
by_session = collections.defaultdict(list)
for p in prompts: by_session[p[2]].append(p)
early = late = early_n = late_n = 0
for rows in by_session.values():
    if len(rows) < 8: continue
    half = len(rows) // 2
    early += sum(p[1] for p in rows[:half]); early_n += half
    late += sum(p[1] for p in rows[half:]); late_n += len(rows) - half
print(f"\nWITHIN sessions of 8+ prompts: first half {early}/{early_n} {early/early_n:.1%}, second half {late}/{late_n} {late/late_n:.1%}")
print("\nexamples matched:")
import random; random.seed(5)
for p in random.sample([p for p in prompts if p[1]], 12): print("  ", f"{p[0]/1e3:.0f}k", " ".join(p[3].split())[:110])
