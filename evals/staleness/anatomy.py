"""Task 2197, widened at the user's word (2026-10-19: improve ekko as a whole,
not only decide 1433): where a resume's orientation goes, call by call, in
part B's runs -- what the agent read or ran before answering, what each
returned, and what it would cost had the prime carried it instead.

    nix develop -c python3 evals/staleness/anatomy.py

A tool call's result is priced as part A prices it: its tokens (2.28
characters each for tool results, note 286) written once (2) and read by every
later call of the run (0.05 each, Opus 5.5). A round trip is the call that
asked for it: one more API call, whose seconds the run measured.
"""

import collections
import json
import os
import re
import statistics

import config as C

RUNS = os.path.join(C.DATA, "runs")
TRUTH = json.load(open(os.path.join(C.DATA, "truth.json")))
CPT_RESULT = 2.28
GIT = re.compile(r"\bgit\b[^|;&]*?\b(log|status|diff|show|rev-list|rev-parse|describe|reflog|shortlog|branch|tag|fetch|merge-base|worktree|stash|remote|ls-files)\b")
PROC = re.compile(r"\b(ps|pgrep|pidof|kmux|tmux|systemctl|journalctl|kill -0)\b")
FILES = re.compile(r"^\s*(cat|head|tail|sed -n|ls|find|wc|grep|rg|tree|stat|du)\b")


def category(name, args, named, handoff_task):
    if name.startswith("mcp__") and "ekko" in name:
        verb = name.rsplit("__", 1)[-1]
        if verb == "context":
            ids = set()
            for key in ("item", "items"):
                value = args.get(key)
                for v in (value if isinstance(value, list) else [value] if value is not None else []):
                    try:
                        ids.add(int(v))
                    except (TypeError, ValueError):
                        pass
            if ids and ids <= (named | {handoff_task}):
                return "ekko context: the handoff's own items"
            if ids & (named | {handoff_task}):
                return "ekko context: its items and others"
            return "ekko context: other items"
        return f"ekko {verb}"
    if name.startswith("mcp__") and "graff" in name:
        return "graff"
    if name == "ToolSearch":
        return "ToolSearch (loading a deferred tool)"
    if name == "Bash":
        command = args.get("command") or ""
        if GIT.search(command):
            return "git"
        if PROC.search(command):
            return "processes, panes, services"
        if command.strip().startswith("ekko"):
            return "ekko CLI"
        if FILES.search(command):
            return "files (shell)"
        return "other shell"
    if name in ("Read", "Glob", "Grep"):
        return "files (Read, Glob, Grep)"
    return name


def run_calls(path):
    calls, order, results = {}, [], {}
    for raw in open(path, errors="replace"):
        try:
            row = json.loads(raw)
        except ValueError:
            continue
        if row.get("type") == "assistant":
            message = row.get("message") or {}
            mid = message.get("id")
            if mid not in calls:
                calls[mid] = []
                order.append(mid)
            for block in message.get("content") or []:
                if isinstance(block, dict) and block.get("type") == "tool_use":
                    calls[mid].append(block)
        elif row.get("type") == "user":
            for block in (row.get("message") or {}).get("content") or []:
                if isinstance(block, dict) and block.get("type") == "tool_result":
                    content = block.get("content")
                    text = content if isinstance(content, str) else "\n".join(
                        part.get("text", "") for part in content or [] if isinstance(part, dict))
                    results[block.get("tool_use_id")] = text
    return order, calls, results


def main():
    per_arm = collections.defaultdict(lambda: collections.defaultdict(lambda: {"calls": 0, "chars": 0, "units": 0.0, "runs": set()}))
    first_moves = collections.Counter()
    runs_by_arm = collections.Counter()
    tool_round_trips = collections.defaultdict(list)
    for name in sorted(os.listdir(RUNS)):
        path = os.path.join(RUNS, name, "run.json")
        if not os.path.exists(path):
            continue
        run = json.load(open(path))
        if run.get("rep", 1) == 0 or run["scenario"] not in TRUTH or run.get("prompt") != "onde paramos?":
            continue
        facts = json.load(open(os.path.join(C.SCRATCH, "template", run["scenario"], "facts.json")))
        named, task = set(facts.get("named") or []), facts.get("task")
        order, calls, results = run_calls(os.path.join(RUNS, name, "stream.jsonl"))
        arm = run["arm"]
        runs_by_arm[arm] += 1
        n_calls = len(order)
        tool_round_trips[arm].append(sum(1 for mid in order if calls[mid]))
        for index, mid in enumerate(order):
            later = n_calls - index - 1
            for block in calls[mid]:
                cat = category(block.get("name") or "", block.get("input") or {}, named, task)
                chars = len(results.get(block.get("id")) or "")
                cell = per_arm[arm][cat]
                cell["calls"] += 1
                cell["chars"] += chars
                cell["units"] += chars / CPT_RESULT * (2 + 0.05 * later)
                cell["runs"].add(name)
                if index == 0:
                    first_moves[cat] += 1
    print("== what the runs did before answering, by arm: tool calls, result characters, and their price in units (per run)")
    for arm in sorted(per_arm):
        n = runs_by_arm[arm]
        print(f"  arm {arm}: {n} runs; API calls with tools, median {statistics.median(tool_round_trips[arm]):.0f}")
        for cat, cell in sorted(per_arm[arm].items(), key=lambda kv: -kv[1]["units"]):
            print(f"    {cat:42} calls/run {cell['calls'] / n:4.1f}  in {len(cell['runs']):2}/{n} runs  chars/run {cell['chars'] / n:7,.0f}  units/run {cell['units'] / n:8,.0f}")
    print()
    print("== the first call's tools, every arm:", dict(first_moves.most_common()))


if __name__ == "__main__":
    main()


RECHECK = re.compile(r"(?i)\b(rests on|still true|checked 20|rechecked|corrected|correction|not rechecked|not settled|recheck after)\b")
GAP = re.compile(r"(?i)\b(prime|handoff)\b.*\b(stale|old|obsolete|week|days?-old|leads)\b|\bstale handoff\b")


def per_run():
    out = []
    for name in sorted(os.listdir(RUNS)):
        path = os.path.join(RUNS, name, "run.json")
        if not os.path.exists(path):
            continue
        run = json.load(open(path))
        if run.get("rep", 1) == 0 or run["scenario"] not in TRUTH or run.get("prompt") != "onde paramos?":
            continue
        order, calls, results = run_calls(os.path.join(RUNS, name, "stream.jsonl"))
        ts_only = sum(1 for mid in order if calls[mid] and all(b.get("name") == "ToolSearch" for b in calls[mid]))
        rechecks = gaps = writes = 0
        for mid in order:
            for b in calls[mid]:
                n, inp = b.get("name") or "", b.get("input") or {}
                if "ekko" in n and n.rsplit("__", 1)[-1] in ("create", "edit"):
                    writes += 1
                    text = json.dumps(inp, ensure_ascii=False)
                    if RECHECK.search(text):
                        rechecks += 1
                    elif GAP.search(text):
                        gaps += 1
        out.append({"run": name, "scenario": run["scenario"], "arm": run["arm"], "class": TRUTH[run["scenario"]]["class"],
                    "calls": run["calls"], "seconds": run["seconds"], "units_cold": run.get("units_cold"),
                    "toolsearch_only_calls": ts_only, "anchor_rechecks": rechecks, "gap_notes": gaps, "board_writes": writes,
                    "refused_by_guard": run.get("refused_elsewhere", 0)})
    return out


def report_runs():
    rows = per_run()
    print()
    print("== per run: board writes while answering 'onde paramos?', and what they were")
    for key in ("board_writes", "anchor_rechecks", "gap_notes", "toolsearch_only_calls", "refused_by_guard"):
        by_arm = collections.defaultdict(list)
        for r in rows:
            by_arm[r["arm"]].append(r[key])
        print(f"  {key:22} " + "  ".join(f"{a}: {sum(v)} in {sum(1 for x in v if x)}/{len(v)} runs" for a, v in sorted(by_arm.items())))
    with_rc = [r for r in rows if r["anchor_rechecks"]]
    without = [r for r in rows if not r["anchor_rechecks"]]
    for label, group in (("runs that rechecked notes' anchors", with_rc), ("runs that did not", without)):
        if group:
            print(f"  {label:36} n {len(group):2}: calls median {statistics.median(r['calls'] for r in group):4.1f}, "
                  f"seconds median {statistics.median(r['seconds'] for r in group):6.1f}, units_cold median {statistics.median(r['units_cold'] for r in group):9,.0f}; "
                  f"scenarios {sorted({r['scenario'] for r in group})}")
    print("  gap notes about the prime's handoff, by run:", [(r["run"][:24], r["gap_notes"]) for r in rows if r["gap_notes"]])
    # The same comparison within a scenario: the scenario's runs that rechecked anchors against its runs that did not,
    # so a scenario that is costly to resume does not pass for the price of rechecking.
    by_scenario = collections.defaultdict(lambda: ([], []))
    for r in rows:
        by_scenario[r["scenario"]][0 if r["anchor_rechecks"] else 1].append(r)
    for metric in ("calls", "seconds", "units_cold"):
        diffs = [statistics.fmean(x[metric] for x in w) - statistics.fmean(x[metric] for x in wo)
                 for s, (w, wo) in sorted(by_scenario.items()) if w and wo]
        if len(diffs) > 1:
            se = statistics.stdev(diffs) / len(diffs) ** 0.5
            print(f"  within a scenario, rechecked minus not, {metric:10}: k {len(diffs)}  d {statistics.fmean(diffs):+12,.1f}  SE {se:10,.1f}  "
                  f"per scenario {[round(d, 1) for d in diffs]}")
    by_arm_scenario = collections.defaultdict(list)
    for r in rows:
        by_arm_scenario[(r["scenario"], r["arm"])].append(r["board_writes"])
    for arm in ("B", "C", "D"):
        diffs = [statistics.fmean(v) - statistics.fmean(by_arm_scenario[(s, "A")]) for (s, a), v in sorted(by_arm_scenario.items())
                 if a == arm and (s, "A") in by_arm_scenario]
        if len(diffs) > 1:
            se = statistics.stdev(diffs) / len(diffs) ** 0.5
            print(f"  board writes, {arm} minus A, paired by scenario: k {len(diffs)}  d {statistics.fmean(diffs):+.2f}  SE {se:.2f}")


if __name__ == "__main__":
    report_runs()
