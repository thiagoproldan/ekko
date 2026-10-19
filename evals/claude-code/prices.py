"""Task 2204: the evals price each cache read at the weight of the model that
ran the call, and take that weight from one place. Needs no transcripts; CI
runs it.

    python3 evals/claude-code/prices.py [EVALS_DIR]

1. read_weight against the pricing page (platform.claude.com/docs/en/build-
   with-claude/prompt-caching, read 2026-10-19).
2. Two calls of study 2197's headless runs, priced by transcripts.py, the
   paired harness and the recall replay, against what Claude Code billed for
   them (modelUsage.costUSD) at Opus 5.5's $4 per million input tokens. With
   Opus 5's read weight, 0.1, they come out 5.55% and 6.04% too high. Over the
   90 Opus 5.5 results stored on the machine where this was written, 0.05
   fits all 90 bills, 88 to the digit with every write a 1-hour one; 0.1
   leaves 15 that no mix of write lengths explains.
3. The class: no eval prices a read on its own. No W_CR or W_READ, no
   read_weight but transcripts.py's, and no 0.1, 0.05 or 0.025 multiplied in
   code or chosen by a condition (`0.05 if ... else 0.1`): a read weight
   written as a number is one transcripts.py does not know about. And every
   call of the paired harness's units() names the model, the one way in that
   takes it as an argument.

EVALS_DIR, by default the folder above this one, is what part 3 walks.
Exits 1 when any check fails, after listing every failure.
"""

import ast
import io
import os
import sys
import tokenize

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.join(HERE, "..", "paired"))
sys.path.insert(0, os.path.join(HERE, "..", "recall"))
import harness  # noqa: E402
import replay  # noqa: E402
import transcripts  # noqa: E402

# Part 1: the model ids calls name, and the weight the pricing page gives each.
EXPECTED = {
    "claude-fable-5-1": 0.025,
    "claude-mythos-5-1": 0.025,
    "claude-opus-5-5": 0.05,
    "claude-opus-5-5[1m]": 0.05,
    "claude-sonnet-5-5": 0.05,
    "claude-opus-5": 0.1,
    "claude-opus-5[1m]": 0.1,
    "claude-sonnet-5": 0.1,
    "claude-haiku-5-5": 0.1,
    "claude-haiku-4-5-20251001": 0.1,
    "": 0.1,
    None: 0.1,
}

# Part 2: one model's usage in a run's result, as Claude Code reported it, and
# the dollars it billed; Opus 5.5's input is $4 per million (gotcha 2198).
BILLED = (
    ("claude-opus-5-5", 4.0, {"input_tokens": 14, "output_tokens": 19_252, "cache_read_input_tokens": 176_209,
                              "cache_creation_input_tokens": 26_784}, 0.6346098),
    ("claude-opus-5-5", 4.0, {"input_tokens": 20, "output_tokens": 28_670, "cache_read_input_tokens": 292_699,
                              "cache_creation_input_tokens": 42_109}, 0.9688918),
)

# Part 3: where the one definition lives, and the numbers that are read weights.
OWNER = os.path.join("claude-code", "transcripts.py")
WEIGHTS = {"0.1", "0.10", ".1", "0.05", ".05", "0.025", ".025"}
NAMES = {"W_CR", "W_READ"}


def weights():
    return [f"read_weight({model!r}) is {transcripts.read_weight(model)}, the pricing page says {weight}"
            for model, weight in EXPECTED.items() if transcripts.read_weight(model) != weight]


def call_of(module, model, usage):
    """A call as `module`'s replay builds one: every write a 1-hour one, as a
    result's modelUsage gives no split."""
    call = module.Call.__new__(module.Call)
    call.model = model
    call.inp, call.out = usage["input_tokens"], usage["output_tokens"]
    call.cr, call.cw1h, call.cw5m = usage["cache_read_input_tokens"], usage["cache_creation_input_tokens"], 0
    return call


def bills():
    failures = []
    for model, per_million, usage, dollars in BILLED:
        priced = {
            "transcripts.Call.cost": call_of(transcripts, model, usage).cost,
            "replay.Call.cost": call_of(replay, model, usage).cost,
            "harness.units": harness.units(usage, model),
        }
        for where, units in priced.items():
            if abs(units * per_million / 1e6 - dollars) > 1e-9 * dollars:
                failures.append(f"{where}: {model} {usage} prices at ${units * per_million / 1e6:.7f}, Claude Code billed ${dollars}")
    return failures


def code_tokens(path):
    """The file's tokens, strings and comments left out."""
    with open(path, encoding="utf-8") as handle:
        source = handle.read()
    skip = (tokenize.COMMENT, tokenize.STRING, tokenize.NL, tokenize.NEWLINE, tokenize.INDENT, tokenize.DEDENT)
    skip += tuple(getattr(tokenize, name) for name in ("FSTRING_START", "FSTRING_MIDDLE", "FSTRING_END") if hasattr(tokenize, name))
    return [token for token in tokenize.generate_tokens(io.StringIO(source).readline) if token.type not in skip]


def unpriced(path, where):
    """Calls of units() that leave out the model."""
    with open(path, encoding="utf-8") as handle:
        tree = ast.parse(handle.read(), path)
    found = []
    for node in ast.walk(tree):
        if not isinstance(node, ast.Call):
            continue
        func = node.func
        name = func.id if isinstance(func, ast.Name) else func.attr if isinstance(func, ast.Attribute) else None
        if name == "units" and len(node.args) + len(node.keywords) < 2:
            found.append(f"{where}:{node.lineno}: units() without the call's model")
    return found


def class_members(root):
    failures, owners = [], []
    for folder, dirs, files in os.walk(root):
        dirs[:] = sorted(d for d in dirs if d != "__pycache__")
        for name in sorted(files):
            if not name.endswith(".py"):
                continue
            path = os.path.join(folder, name)
            where = os.path.relpath(path, root)
            tokens = code_tokens(path)
            for index, token in enumerate(tokens):
                line = token.start[0]
                if token.type == tokenize.NAME and token.string in NAMES:
                    failures.append(f"{where}:{line}: {token.string}, a read weight that ignores the model")
                if token.string == "def" and index + 1 < len(tokens) and tokens[index + 1].string == "read_weight":
                    owners.append(where)
                    if where != OWNER:
                        failures.append(f"{where}:{line}: a read_weight of its own; import transcripts.read_weight")
                if token.type == tokenize.NUMBER and token.string in WEIGHTS:
                    before = tokens[index - 1].string if index else ""
                    after = tokens[index + 1].string if index + 1 < len(tokens) else ""
                    if "*" in (before, after):
                        failures.append(f"{where}:{line}: {token.string} multiplied, a read weight written as a number")
                    elif before == "else" or after == "if":
                        failures.append(f"{where}:{line}: {token.string} chosen by a condition, a read weight of its own")
            failures += unpriced(path, where)
    if owners.count(OWNER) != 1:
        failures.append(f"{OWNER} defines read_weight {owners.count(OWNER)} times, not once")
    return failures


def main():
    root = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else os.path.join(HERE, ".."))
    failed = False
    for title, failures in (("read weights by model", weights()), ("calls against what Claude Code billed", bills()),
                            (f"one read weight for every eval in {root}", class_members(root))):
        print(f"{'ok  ' if not failures else 'FAIL'} {title}")
        for failure in failures:
            print(f"     {failure}")
        failed = failed or bool(failures)
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
