"""Task 2197, part B's report: each arm against the control, paired by
scenario, under procedure 1435's gate.

    nix develop -c python3 evals/staleness/ab_report.py [--blind | --blind-more N] [--grades FILE ...]

--blind writes the answers, shuffled under random labels, to answers.blind.md
and the key to answers.key.json, for grading without seeing the arm. A grades
file maps each label to {"stale_claims": n, "moved_named": n, "false_alarm": n}:
claims the answer makes that the handoff made and the world no longer holds,
the moved facts it states rightly, and things it says moved that did not.

The gate (procedure 1435): per scenario, the arm's mean minus the control's;
d is their mean over scenarios, SE the standard error of those differences.
A cost wins if it is lower by more than SE; within one SE it is a tie, and the
cheaper wins. A run with no grade is missing, never a zero.
"""

import argparse
import collections
import json
import math
import os
import random
import statistics

import config as C

RUNS = os.path.join(C.DATA, "runs")
TRUTH = {k: v for k, v in json.load(open(os.path.join(C.DATA, "truth.json"))).items() if not k.startswith("_")}
METRICS = ("calls", "units", "tool_calls", "git_looks", "ekko_reads", "seconds", "duration_api_ms", "cost_usd", "result_chars")


def load():
    out = []
    for name in sorted(os.listdir(RUNS)):
        path = os.path.join(RUNS, name, "run.json")
        if not os.path.exists(path):
            continue
        run = json.load(open(path))
        if run.get("rep", 1) == 0 or run.get("prompt") != "onde paramos?":
            continue
        # The live and old-handoff scenarios only: the replays of the past leaked its future (note on 2197).
        if run["scenario"] not in TRUTH:
            continue
        out.append(run)
    return out


def gate(runs, metric, arm, control="A", lower_is_better=True):
    cells = collections.defaultdict(list)
    for run in runs:
        value = run.get(metric)
        if value is not None:
            cells[(run["scenario"], run["arm"])].append(value)
    diffs = []
    for scenario in sorted({s for s, _ in cells}):
        if (scenario, arm) in cells and (scenario, control) in cells:
            diffs.append(statistics.fmean(cells[(scenario, arm)]) - statistics.fmean(cells[(scenario, control)]))
    if not diffs:
        return None
    d = statistics.fmean(diffs)
    se = statistics.stdev(diffs) / math.sqrt(len(diffs)) if len(diffs) > 1 else float("nan")
    better = -d if lower_is_better else d
    verdict = "wins" if better > se else "loses" if -better > se else "tie"
    return {"k": len(diffs), "d": d, "se": se, "verdict": verdict, "diffs": diffs}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--blind", action="store_true")
    parser.add_argument("--blind-more", type=int, metavar="N",
                        help="answers.blind2.md: the runs answers.key.json lacks, mixed with N already graded ones drawn at random, under new labels")
    parser.add_argument("--grades", nargs="+")
    args = parser.parse_args()
    runs = load()
    print(f"{len(runs)} runs")
    print(f"{'run':28} {'calls':>5} {'units':>9} {'tools':>5} {'git':>4} {'ekko':>4} {'s':>6} {'$':>6}  peeks")
    for run in runs:
        print(f"{run['run']:28} {run['calls']:>5} {run['units']:>9,} {run['tool_calls']:>5} {run['git_looks']:>4} {run['ekko_reads']:>4} "
              f"{run['seconds']:>6} {run['cost_usd'] or 0:>6.2f}  {len(run['peeks'])}")
    print()
    for arm in ("B", "C", "D"):
        for metric in METRICS:
            result = gate(runs, metric, arm)
            if result:
                print(f"{arm} vs A  {metric:16} k {result['k']}  d {result['d']:+,.1f}  SE {result['se']:,.1f}  {result['verdict']}  per scenario {[round(x, 1) for x in result['diffs']]}")
        print()
    if args.blind:
        if os.path.exists(os.path.join(C.DATA, "answers.key.json")):
            raise SystemExit("answers.key.json exists: a new --blind would orphan its grades; use --blind-more")
        rng = random.Random()
        labels = {}
        order = list(runs)
        rng.shuffle(order)
        with open(os.path.join(C.DATA, "answers.blind.md"), "w") as handle:
            for index, run in enumerate(order):
                label = f"r{index:02d}"
                labels[label] = run["run"]
                truth = TRUTH[run["scenario"]]
                facts = "".join(f"- {k}: {v}\n" for k, v in {**truth.get("moved", {}), **truth.get("stale", {})}.items()) or "- fresh: nothing moved\n"
                handle.write(f"## {label} -- scenario {run['scenario']} ({truth['class']})\n\nTruth:\n{facts}\nAnswer:\n\n{run['answer']}\n\n")
        json.dump(labels, open(os.path.join(C.DATA, "answers.key.json"), "w"), indent=2)
        print(f"wrote answers.blind.md ({len(order)} answers) and answers.key.json")
    if args.blind_more is not None:
        key = json.load(open(os.path.join(C.DATA, "answers.key.json")))
        rng = random.Random()
        new = [run for run in runs if run["run"] not in set(key.values())]
        # r28 and r36 name their run's folder in the answer, so their arm is known: not drawn again.
        old_labels = rng.sample(sorted(label for label in key if label not in ("r28", "r36")), args.blind_more)
        by_name = {run["run"]: run for run in runs}
        order = new + [by_name[key[label]] for label in old_labels]
        rng.shuffle(order)
        labels = {}
        with open(os.path.join(C.DATA, "answers.blind2.md"), "w") as handle:
            for index, run in enumerate(order):
                label = f"s{index:02d}"
                labels[label] = run["run"]
                truth = TRUTH[run["scenario"]]
                facts = "".join(f"- {k}: {v}\n" for k, v in {**truth.get("moved", {}), **truth.get("stale", {})}.items()) or "- fresh: nothing moved\n"
                handle.write(f"## {label} -- scenario {run['scenario']} ({truth['class']})\n\nTruth:\n{facts}\nAnswer:\n\n{run['answer']}\n\n")
        json.dump(labels, open(os.path.join(C.DATA, "answers.key2.json"), "w"), indent=2)
        print(f"wrote answers.blind2.md ({len(new)} new answers, {len(old_labels)} graded before) and answers.key2.json")
    if args.grades:
        by_run = {}
        for path in args.grades:
            grades = json.load(open(C.data_path(path)))
            key = json.load(open(os.path.join(C.DATA, "answers.key2.json" if any(l.startswith("s") for l in grades if not l.startswith("_")) else "answers.key.json")))
            for label, grade in grades.items():
                if not label.startswith("_"):
                    by_run.setdefault(key[label], grade)
        for run in runs:
            grade = by_run.get(run["run"])
            for field in ("stale_claims", "moved_named", "false_alarm"):
                run[field] = None if grade is None else grade.get(field)
        for arm in ("B", "C", "D"):
            for metric, lower in (("stale_claims", True), ("moved_named", False), ("false_alarm", True)):
                result = gate(runs, metric, arm, lower_is_better=lower)
                if result:
                    print(f"{arm} vs A  {metric:16} k {result['k']}  d {result['d']:+.2f}  SE {result['se']:.2f}  {result['verdict']}")


if __name__ == "__main__":
    main()
