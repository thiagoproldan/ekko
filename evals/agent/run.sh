#!/usr/bin/env bash
# The agent evals behind the rigor plan, in one go, against one binary.
#
#   nix develop -c evals/agent/run.sh [--full]
#
# Builds nothing. Build the checkout first (cargo build --release) and the
# evals measure target/release/ekko, or point EKKO_BIN at another binary; each
# report opens with the binary, its version and the checkout it came from.
# Scratch boards and results land in target/evals/, which git ignores. Real
# boards are only read.
#
#   semantics.py  the behaviours the plan calls wrong, OPEN or FIXED per item
#   retrieval.py  calls and bytes per question on a copy of a real board
#   scale.py      latency and response size from 150 to 20,000 items
#   prototype.py  the plan's graph math and search in Python, as a reference
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"

python3 "$here/semantics.py"
echo
python3 "$here/retrieval.py"
echo
python3 "$here/scale.py" "$@"
echo
python3 "$here/prototype.py"
