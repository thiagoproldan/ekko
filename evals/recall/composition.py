"""Task 507, free exploration: what a session's context is made of, and what
each part costs while it stays there. Every token in the context is read
again on every later call, so what a tool result costs is its size times the
calls it survives: r*(2 + 0.1*T) in units of one uncached input token, a
1-hour cache write 2 and a cache read 0.1, over the T main-thread calls left
before the context is cleared or compacted.

Each tool result is priced that way and summed by tool, and Bash by the
command it ran; the model's own output by the same rule; the prefix every
call re-reads (the floor, note 441) at 0.1 a call. What the usage fields show
beyond those -- the growth nothing here names -- is left as the remainder.

    nix develop -c python3 evals/recall/composition.py [--since 2026-08-25]
"""

import argparse
import collections
import datetime
import json
import os
import re
import statistics

from replay import LOCAL, OUT, W_CR, W_CW1H, W_OUT, calls_left, ekko_op, scan, tokens

# Note 441: the floor each profile's sessions start from, in tokens.
FLOOR = {".claude-trabalho": 38_300, ".claude": 42_300}


def family(command):
    """The program a Bash command runs: the first one after any `cd x &&`,
    variable settings and wrappers; under `nix develop -c`, the one it runs."""
    for part in re.split(r"&&|\|\||;|\n|\|", command):
        found = part.strip().split()
        while found and re.match(r"^[A-Za-z_][A-Za-z0-9_]*=", found[0]):
            found = found[1:]
        if not found or found[0] in ("cd", "export", "set", "source", "."):
            continue
        if found[0] == "timeout" and len(found) > 2:
            found = found[2:]
        if found[0] in ("time", "sudo", "env", "exec", "command") and len(found) > 1:
            found = found[1:]
        if found[0] == "nix" and len(found) > 1:
            if "-c" in found and found.index("-c") + 1 < len(found):
                return "nix>" + os.path.basename(found[found.index("-c") + 1])
            return "nix " + found[1]
        return os.path.basename(found[0])
    return "?"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--since", default="2026-08-25")
    parser.add_argument("--out", default=OUT)
    args = parser.parse_args()
    since = datetime.datetime.fromisoformat(args.since).replace(tzinfo=LOCAL)

    by_tool = collections.defaultdict(lambda: {"n": 0, "tokens": 0.0, "units": 0.0, "sizes": []})
    by_bash = collections.defaultdict(lambda: {"n": 0, "tokens": 0.0, "units": 0.0})
    results = []
    bill = floor_units = output_units = calls_total = 0.0
    growth_units = 0.0
    for path, events in scan(since):
        calls_left(events)
        profile = next((p for p in FLOOR if f"/{p}/" in path), ".claude")
        owner = {}
        calls = [event[1] for event in events if event[0] == "call"]
        for call in calls:
            bill += call.cost
            calls_total += 1
            floor_units += W_CR * FLOOR[profile]
            # what the model wrote stays: written once at the next call, read after
            output_units += call.out * (W_CW1H + W_CR * call.left)
            for name, use_id, arguments in call.uses:
                if name == "Bash":
                    label = family(arguments.get("command") or "")
                else:
                    label = None
                owner[use_id] = (name, label, call)
        # the growth the usage fields show between two calls of one stretch
        for before, after in zip(calls, calls[1:]):
            if after.left == before.left - 1 and after.ctx > before.ctx:
                growth_units += (after.ctx - before.ctx) * (W_CW1H + W_CR * after.left)
        for event in events:
            if event[0] != "result" or event[1] not in owner:
                continue
            name, label, call = owner[event[1]]
            size = tokens(len(event[2]))
            units = size * (W_CW1H + W_CR * call.left)
            op = ekko_op(name)
            key = f"ekko {op}" if op else ("mcp other" if name.startswith("mcp__") else name)
            row = by_tool[key]
            row["n"] += 1
            row["tokens"] += size
            row["units"] += units
            row["sizes"].append(size)
            results.append((units, size, key, label or "", call.left))
            if name == "Bash":
                bash = by_bash[label]
                bash["n"] += 1
                bash["tokens"] += size
                bash["units"] += units

    named = sum(row["units"] for row in by_tool.values())
    print(f"since {args.since}: {calls_total:.0f} main-thread calls, bill {bill / 1e6:.0f}M units")
    print(f"  floor re-read at 0.1 a call  {floor_units / 1e6:7.1f}M  {floor_units / bill:6.1%}")
    print(f"  tool results, kept          {named / 1e6:7.1f}M  {named / bill:6.1%}")
    print(f"  the model's output, kept    {output_units / 1e6:7.1f}M  {output_units / bill:6.1%}  (thinking included)")
    print(f"  all growth, from usage      {growth_units / 1e6:7.1f}M  {growth_units / bill:6.1%}  (results + output + reminders + prompts)")
    print("\nTOOL RESULTS by tool: count, tokens in, units while kept, share of the bill, median and p90 size")
    for key, row in sorted(by_tool.items(), key=lambda pair: -pair[1]["units"])[:16]:
        sizes = sorted(row["sizes"])
        p90 = sizes[int(0.9 * (len(sizes) - 1))]
        print(f"  {key:22s} {row['n']:6d} {row['tokens'] / 1e6:6.2f}M tok {row['units'] / 1e6:7.1f}M {row['units'] / bill:6.1%}"
              f"  med {statistics.median(sizes):6.0f} p90 {p90:6.0f}")
    print("\nBASH by program")
    for key, row in sorted(by_bash.items(), key=lambda pair: -pair[1]["units"])[:14]:
        print(f"  {key:22s} {row['n']:6d} {row['tokens'] / 1e6:6.2f}M tok {row['units'] / 1e6:7.1f}M {row['units'] / bill:6.1%}")
    results.sort(reverse=True)
    total = sum(r[0] for r in results)
    print("\nHEAVY TAIL: share of the results' units held by the largest-costing results")
    for share in (0.01, 0.05, 0.10):
        top = results[: max(1, int(share * len(results)))]
        print(f"  top {share:4.0%} ({len(top)} results): {sum(r[0] for r in top) / total:6.1%}  median size {statistics.median(r[1] for r in top):7.0f} tok")
    by_size = sorted(results, key=lambda r: -r[1])
    for cap in (2_000, 5_000, 10_000):
        over = [r for r in results if r[1] > cap]
        excess = sum((r[1] - cap) / r[1] * r[0] for r in over)
        print(f"  results over {cap:6d} tok: {len(over):5d}, a cap there would drop {excess / 1e6:6.1f}M ({excess / bill:5.1%} of the bill)")
    print("  the 10 costliest:")
    for units, size, key, label, left in results[:10]:
        print(f"    {units / 1e6:5.2f}M  {size:7.0f} tok x {left:4d} calls left  {key} {label}")
    os.makedirs(args.out, exist_ok=True)
    with open(os.path.join(args.out, "composition.json"), "w") as handle:
        json.dump({
            "since": args.since, "bill": bill, "floor": floor_units, "output": output_units, "growth": growth_units,
            "tools": {k: {x: v for x, v in row.items() if x != "sizes"} for k, row in by_tool.items()},
            "bash": by_bash,
        }, handle, indent=1)


if __name__ == "__main__":
    main()
