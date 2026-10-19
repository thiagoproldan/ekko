"""Task 2197: the expected cost of a start under each arm, from part B's runs
(what a resume costs, by arm and by whether the handoff is stale) and part A's
transcripts (how often a handoff is stale, how long a session goes on).

    nix develop -c python3 evals/staleness/model.py [--grades FILE ...] [--exclude RUN]

Per start, for arm a and a share p of stale handoffs:

    J(a) = p * O(a | stale) + (1 - p) * O(a | fresh) + I_later(a)

O is what the run measured from the prompt to the answer: units by the
model's own weights with the first call priced cold (units_cold), calls,
seconds, dollars. The arm's line is inside O already (written in the first
call, read by the rest); I_later is its price on the calls after the answer:
its tokens read at the model's weight by each later call of the session
(part A: a median of 90 later calls). The paired gate is procedure 1435's:
per scenario, arm minus control; d is their mean, SE their standard error; a
cost wins when lower by more than SE, a tie goes to the cheaper.
"""

import argparse
import collections
import json
import math
import os
import statistics
import sys

import config as C

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "claude-code"))
from transcripts import read_weight  # noqa: E402

RUNS = os.path.join(C.DATA, "runs")
TRUTH = json.load(open(os.path.join(C.DATA, "truth.json")))
CPT = 2.59           # characters per token of ekko's text (note 286)
LATER_CALLS = 90     # median calls after the first in a user's session (part A)
METRICS = (("units_cold", True), ("calls", True), ("seconds", True), ("cost_usd", True),
           ("git_looks", True), ("ekko_reads", True), ("tool_calls", True))
QUALITY = (("stale_claims", True), ("moved_named", False), ("false_alarm", True))


def load(grades=None, exclude=()):
    runs = []
    for name in sorted(os.listdir(RUNS)):
        path = os.path.join(RUNS, name, "run.json")
        if not os.path.exists(path) or name in exclude:
            continue
        run = json.load(open(path))
        if run.get("rep", 1) == 0 or run.get("prompt") != "onde paramos?" or run["scenario"] not in TRUTH:
            continue
        run["class"] = TRUTH[run["scenario"]]["class"]
        runs.append(run)
    if grades:
        # The first file's grade of a run counts; a later file's grade of the same run is a re-grade, for consistency only.
        by_run = {}
        for path in grades:
            by_label = {k: v for k, v in json.load(open(C.data_path(path))).items() if not k.startswith("_")}
            key = json.load(open(os.path.join(C.DATA, "answers.key2.json" if any(l.startswith("s") for l in by_label) else "answers.key.json")))
            for label, grade in by_label.items():
                by_run.setdefault(key[label], grade)
        for run in runs:
            grade = by_run.get(run["run"])
            for field, _ in QUALITY:
                run[field] = None if grade is None else grade.get(field)
    return runs


def cells(runs, metric):
    out = collections.defaultdict(list)
    for run in runs:
        if run.get(metric) is not None:
            out[(run["scenario"], run["arm"])].append(run[metric])
    return {k: statistics.fmean(v) for k, v in out.items()}


def gate(runs, metric, arm, lower_is_better=True, control="A"):
    c = cells(runs, metric)
    diffs = [c[(s, arm)] - c[(s, control)] for s in sorted({s for s, _ in c}) if (s, arm) in c and (s, control) in c]
    if not diffs:
        return None
    d = statistics.fmean(diffs)
    se = statistics.stdev(diffs) / math.sqrt(len(diffs)) if len(diffs) > 1 else float("nan")
    better = -d if lower_is_better else d
    verdict = "wins" if better > se else "loses" if -better > se else "tie"
    return {"k": len(diffs), "d": d, "se": se, "verdict": verdict, "diffs": diffs}


def mean_by_class(runs, metric, arm, klass):
    c = cells([r for r in runs if r["class"] == klass], metric)
    values = [v for (s, a), v in c.items() if a == arm]
    return statistics.fmean(values) if values else None


def read_weight_of(runs):
    """The cache read weight of the model the runs ran: one model for them all,
    since I_later prices the line on the calls of the session that follows."""
    models = {run.get("model") for run in runs}
    if len(models) != 1:
        raise SystemExit(f"the runs ran {sorted(map(str, models))}: I_later needs one model's read weight")
    return read_weight(models.pop())


def line_tokens(runs, arm, klass):
    texts = [len(r.get("text") or "") for r in runs if r["arm"] == arm and r["class"] == klass]
    return statistics.fmean(texts) / CPT if texts else 0.0


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--grades", nargs="+")
    parser.add_argument("--exclude", nargs="+", default=(), metavar="RUN", help="runs left out, for a sensitivity check")
    args = parser.parse_args()
    runs = load(args.grades, set(args.exclude))
    w_read = read_weight_of(runs)
    if args.exclude:
        print(f"left out: {sorted(args.exclude)}")
    arms = sorted({r["arm"] for r in runs})
    print(f"{len(runs)} runs; scenarios: stale {sorted({r['scenario'] for r in runs if r['class'] == 'stale'})}, "
          f"fresh {sorted({r['scenario'] for r in runs if r['class'] == 'fresh'})}; arms {arms}")
    print(f"Claude Code {sorted({r.get('claude_code_version') for r in runs})}, models {sorted({r.get('model') for r in runs})}, "
          f"effort {sorted({r.get('effort') for r in runs})}")
    print()
    print("== means by arm and class (per scenario first, then over scenarios)")
    for klass in ("stale", "fresh"):
        for metric, _ in METRICS + (QUALITY if args.grades else ()):
            row = []
            for arm in arms:
                m = mean_by_class(runs, metric, arm, klass)
                row.append(f"{arm} {m:>10,.1f}" if m is not None else f"{arm} {'-':>10}")
            print(f"  {klass:5} {metric:13} " + "  ".join(row))
        print()
    print("== the gate, each arm against A, paired by scenario")
    for scope in ("stale", "fresh", "all"):
        subset = [r for r in runs if scope == "all" or r["class"] == scope]
        for arm in arms:
            if arm == "A":
                continue
            for metric, lower in METRICS + (QUALITY if args.grades else ()):
                g = gate(subset, metric, arm, lower)
                if g:
                    print(f"  {scope:5} {arm} vs A {metric:13} k {g['k']:2}  d {g['d']:+12,.2f}  SE {g['se']:10,.2f}  {g['verdict']:5}  "
                          f"per scenario {[round(x, 1) for x in g['diffs']]}")
            print()
    print("== head to head: B against C and D, C against D (the first lower is better)")
    for scope in ("stale", "fresh", "all"):
        subset = [r for r in runs if scope == "all" or r["class"] == scope]
        for arm, control in (("B", "C"), ("B", "D"), ("C", "D")):
            for metric, lower in (("units_cold", True), ("calls", True), ("seconds", True)) + ((QUALITY if args.grades else ())):
                g = gate(subset, metric, arm, lower, control=control)
                if g:
                    print(f"  {scope:5} {arm} vs {control} {metric:13} k {g['k']:2}  d {g['d']:+12,.2f}  SE {g['se']:10,.2f}  {g['verdict']}")
    print()
    print("== noise: cells run twice (the same scenario and arm), the spread between the two runs")
    twice = collections.defaultdict(list)
    for r in runs:
        twice[(r["scenario"], r["arm"])].append(r)
    for metric in ("units_cold", "calls", "seconds"):
        sds = [statistics.stdev([r[metric] for r in rs]) for rs in twice.values() if len(rs) > 1]
        if sds:
            pooled = math.sqrt(statistics.fmean([s * s for s in sds]))
            print(f"  {metric:11} cells {len(sds)}: pooled within-cell SD {pooled:,.1f}")
    print()
    print("== sign test, each arm against A: scenarios where it cost less, and the two-sided binomial p of that many or more by chance")
    for arm in arms:
        if arm == "A":
            continue
        row = []
        for metric in ("units_cold", "calls", "seconds", "cost_usd"):
            g = gate(runs, metric, arm)
            if not g:
                continue
            lower = sum(1 for x in g["diffs"] if x < 0)
            higher = sum(1 for x in g["diffs"] if x > 0)
            n = lower + higher
            k = max(lower, higher)
            p = min(1.0, 2 * sum(math.comb(n, i) for i in range(k, n + 1)) / 2 ** n) if n else float("nan")
            row.append(f"{metric} {lower}/{n} lower (p {p:.3f})")
        print(f"  {arm} vs A: " + "; ".join(row))
    print()
    print("== power: the spread of per-scenario differences, and the scenarios a difference of a given size needs to clear 2 SE")
    for arm, control in (("B", "C"), ("B", "A"), ("C", "A"), ("D", "A")):
        for metric, effect in (("units_cold", 30_000), ("calls", 1.0), ("seconds", 30.0)):
            g = gate(runs, metric, arm, True, control=control)
            if g and g["k"] > 1:
                sd = statistics.stdev(g["diffs"])
                need = math.ceil((2 * sd / effect) ** 2)
                print(f"  {arm} vs {control} {metric:10}: SD of per-scenario differences {sd:12,.1f}; to see {effect:,} at 2 SE: {need} scenarios")
    print()
    print("== expected cost of a resume ask, J(a) = p O(a|stale) + (1-p) O(a|fresh) + I_later(a)")
    shares = (("resume asks, lower", 0.02), ("resume asks, expected", 0.08), ("user starts, lower", 0.12),
              ("user starts, expected", 0.17), ("user starts, upper", 0.49), ("resume asks, upper", 0.55))
    for metric, _ in (("units_cold", True), ("seconds", True), ("calls", True), ("cost_usd", True)):
        print(f"  {metric}:")
        for label, p in shares:
            row = []
            for arm in arms:
                s, f = mean_by_class(runs, metric, arm, "stale"), mean_by_class(runs, metric, arm, "fresh")
                if s is None or f is None:
                    row.append(f"{arm} {'-':>10}")
                    continue
                later = 0.0
                if metric == "units_cold":
                    later = (p * line_tokens(runs, arm, "stale") + (1 - p) * line_tokens(runs, arm, "fresh")) * w_read * LATER_CALLS
                row.append(f"{arm} {p * s + (1 - p) * f + later:>10,.1f}")
            print(f"    p {p:4.2f} ({label:22}) " + "  ".join(row))
    print()
    print("== the expected saving against A per resume ask, paired: p d_stale + (1-p) d_fresh + the line's price after, SE sqrt(p^2 SE_s^2 + (1-p)^2 SE_f^2)")
    for metric in ("units_cold", "seconds", "calls", "cost_usd"):
        print(f"  {metric}:")
        for label, p in shares:
            row = []
            for arm in arms:
                if arm == "A":
                    continue
                gs = gate([r for r in runs if r["class"] == "stale"], metric, arm)
                gf = gate([r for r in runs if r["class"] == "fresh"], metric, arm)
                if not gs or not gf:
                    continue
                later = 0.0
                if metric == "units_cold":
                    later = (p * line_tokens(runs, arm, "stale") + (1 - p) * line_tokens(runs, arm, "fresh")) * w_read * LATER_CALLS
                d = p * gs["d"] + (1 - p) * gf["d"] + later
                se = math.sqrt((p * gs["se"]) ** 2 + ((1 - p) * gf["se"]) ** 2)
                row.append(f"{arm} {d:+12,.1f} (SE {se:9,.1f})")
            print(f"    p {p:4.2f} ({label:22}) " + "  ".join(row))
    print()
    print("== head to head at the mix of starts: B against C, C against D, B against D, p d_stale + (1-p) d_fresh + the lines' price difference after")
    for metric in ("units_cold", "seconds", "calls"):
        print(f"  {metric}:")
        for label, p in shares:
            row = []
            for arm, control in (("B", "C"), ("B", "D"), ("C", "D")):
                gs = gate([r for r in runs if r["class"] == "stale"], metric, arm, control=control)
                gf = gate([r for r in runs if r["class"] == "fresh"], metric, arm, control=control)
                if not gs or not gf:
                    continue
                later = 0.0
                if metric == "units_cold":
                    later = sum(sign * (p * line_tokens(runs, a, "stale") + (1 - p) * line_tokens(runs, a, "fresh")) * w_read * LATER_CALLS
                                for sign, a in ((1, arm), (-1, control)))
                d = p * gs["d"] + (1 - p) * gf["d"] + later
                se = math.sqrt((p * gs["se"]) ** 2 + ((1 - p) * gf["se"]) ** 2)
                verdict = "wins" if -d > se else "loses" if d > se else "tie"
                row.append(f"{arm}-{control} {d:+10,.1f} (SE {se:8,.1f}) {verdict:5}")
            print(f"    p {p:4.2f} ({label:22}) " + "  ".join(row))
    print()
    print(f"== the line's price after the answer, units per start (its tokens x {w_read} x {LATER_CALLS} later calls)")
    for arm in arms:
        print(f"  {arm}: stale {line_tokens(runs, arm, 'stale') * w_read * LATER_CALLS:6.0f}, fresh {line_tokens(runs, arm, 'fresh') * w_read * LATER_CALLS:6.0f}"
              f"  (chars: stale {line_tokens(runs, arm, 'stale') * CPT:5.0f}, fresh {line_tokens(runs, arm, 'fresh') * CPT:5.0f})")


if __name__ == "__main__":
    main()
