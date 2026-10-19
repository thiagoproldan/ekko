"""Where study 2197's harness finds what belongs to this machine.

The code is here, in evals/staleness. What it reads and writes stays out of
git, as every eval here keeps its numbers under target/evals: the runs, the
starts read off the transcripts, the truth and the grades, all in
target/evals/staleness, with study.json, this machine's own settings -- the
projects and sessions the scenarios came from, where the real repositories
are (a run must not read them), which Claude Code profile the runs use.
README.md says what each key of study.json holds.
"""

import json
import os

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.normpath(os.path.join(HERE, "..", ".."))
DATA = os.path.join(REPO, "target", "evals", "staleness")
# The scenarios' copies and each run's scratch folder: big once built, and
# under a folder name the transcript studies skip (transcripts.py, SKIP).
SCRATCH = os.path.join(os.path.expanduser("~"), ".cache", "ekko-paired", "staleness")


def study():
    path = os.path.join(DATA, "study.json")
    try:
        with open(path) as handle:
            return json.load(handle)
    except FileNotFoundError:
        raise SystemExit(f"{path} is missing: it holds this machine's scenarios and paths (evals/staleness/README.md)") from None


def data_path(path):
    """A file named on the command line: as given when it exists, else in DATA."""
    return path if os.path.isabs(path) or os.path.exists(path) else os.path.join(DATA, path)
