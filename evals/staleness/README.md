# Study 2197: what a resume pays when the handoff is stale

Task 2197 asked whether a line under the prime's handoff, saying what moved
since it was written, pays for itself, against three other ways of saying it.
The findings are on the board, note 2211 on task 2197; what was built from
them is task 2205.

- A: the prime as ekko printed it then (the control).
- B: the line: commits since the handoff, releases, the tasks it names that
  closed since, or that nothing moved.
- C: Basic Memory's record: the commit the handoff was written at, no more.
- D: a content-free warning to check before trusting the handoff.

Part A reads every session start whose prime showed a handoff, over this
machine's Claude Code transcripts, and what had moved between the handoff and
the start. Part B runs the same resume, "onde paramos?", headless under each
arm on copies of real projects, and compares the arms paired by scenario
under procedure 1435's gate.

## The scripts

Run each from the repository's root inside the dev shell:
`nix develop -c python3 evals/staleness/<script>`.

| Script | What it does |
| --- | --- |
| `starts.py [OUT]` | Part A: every start with a handoff in its prime, what moved since the handoff, and what the session spent before its first productive action. Writes `starts.jsonl`. |
| `analyze.py`, `analyze2.py` | Part A's tables, and the same by who started the session (the user, a born session, a probe: `classes.py`). |
| `economics.py`, `expected.py`, `age_auc.py` | The line's price against the most it could save; the expected share of stale starts; whether a handoff's age tells a stale one. |
| `ab.py prepare / run / queue / remeasure` | Part B: build a scenario's copy, run one arm on it, run a plan of them under a quota guard, measure the stored streams again. |
| `ab_report.py` | Each arm against the control, paired by scenario; `--blind` and `--blind-more N` write the answers under random labels for grading. |
| `model.py` | The expected cost of a start under each arm, at the shares of stale starts part A measured; sign test, power, head to head. |
| `anatomy.py` | Where a resume's orientation went, call by call, and what the runs wrote on the board. |
| `bin/no-elsewhere` | The runs' PreToolUse guard: refuses a tool call that names the real folders. |

## What stays on this machine

Nothing below is committed: it holds the user's own prompts, other projects'
boards and this machine's paths.

- `target/evals/staleness/` (`config.DATA`): `study.json`, the runs
  (`runs/<run>/stream.jsonl` and `run.json`), `starts.jsonl`, `truth.json`,
  the grades and the blind answers, `out/`, `timing/`, `findings.md`.
- `~/.cache/ekko-paired/staleness/` (`config.SCRATCH`): each scenario's copy
  (`template/<scenario>/`) and each run's scratch folder, deleted when the run
  ends. The folder name keeps the runs out of the transcript studies
  (`evals/claude-code/transcripts.py`, `SKIP`).

`study.json` holds this machine's settings:

| Key | What it holds |
| --- | --- |
| `profile` | The Claude Code config folder the runs log in with (`CLAUDE_CONFIG_DIR`), so they run with the user's plugins and hooks. |
| `moved` | Project folders that moved since a transcript was written: old prefix to new. |
| `deny` | Permission rules the runs get: the real repositories and transcripts. |
| `guard` | The extended regex `bin/no-elsewhere` refuses a tool call by. |
| `peek` | The Python regex that finds, in a stored stream, a call that named the real folders anyway. |
| `scenarios` | Replays of a past start: name to repository, board folder, start, handoff shown, when it was written, transcript id. |
| `live` | Live scenarios: name to the repository copied whole as it is now. |
| `old` | Old-handoff scenarios: name to repository and the older handoff kept, the later ones dropped from the copy. |
| `study_items` | Repository to the ids of the study's own board items, left out of its copies. |

A shape to start from, with one project:

```json
{
  "profile": "~/.claude",
  "moved": {},
  "deny": ["Read(//projects/**)", "Edit(//projects/**)", "Write(//projects/**)"],
  "guard": "(^|[^[:alnum:]_.-])/projects/",
  "peek": "(?<![\\w.-])/projects/",
  "scenarios": {},
  "live": {"l-ekko": "/projects/ekko"},
  "old": {},
  "study_items": {}
}
```

## Checked when the harness moved here

The code ran from `target/evals/staleness/` during the study. Moved to
`evals/staleness/` with this machine's settings taken out into `study.json`,
it was checked against the code it replaced, on the same data: `measure()` of
all 61 stored streams, the guard's answer to all 790 tool calls the runs made
and to 11 probes (83 refused, both), every report's output (`analyze`,
`analyze2`, `economics`, `expected`, `age_auc`, `anatomy`, `ab_report`,
`model` with and without `--exclude`, `queue --dry-run`) and `starts.py` on
the transcripts of the day all came out identical. With `NO_ELSEWHERE` unset
the guard refuses every call.
