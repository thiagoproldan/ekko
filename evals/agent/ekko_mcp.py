"""Shared plumbing for the agent evals: a JSON-RPC client for `ekko --mcp`,
scratch boards, and read-only copies of real ones.

Nothing here can write to a real board. Every server and every CLI call runs
with HOME and EKKO_DIR pointed into a scratch directory under target/evals/,
and a real board is only ever read by copying its storage into scratch first.

The binary under test is EKKO_BIN if set, else this checkout's
target/release/ekko if it has been built, else `ekko` on PATH. Every report
opens with the line `provenance()` returns, because a number measured with the
installed binary describes whatever revision that binary was built from.
"""

import json
import os
import re
import shutil
import subprocess
import time

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))
SCRATCH = os.environ.get("EKKO_EVAL_SCRATCH", os.path.join(REPO, "target", "evals"))
EKKO_HOME = os.environ.get("EKKO_HOME", os.path.expanduser("~/.ekko"))


def _binary():
    if os.environ.get("EKKO_BIN"):
        return os.environ["EKKO_BIN"]
    built = os.path.join(REPO, "target", "release", "ekko")
    return built if os.path.exists(built) else "ekko"


EKKO = _binary()


def _run(*args):
    return subprocess.run(args, capture_output=True, text=True).stdout.strip()


def provenance():
    """Which binary, which version, which checkout: the line every report opens with."""
    resolved = os.path.realpath(shutil.which(EKKO) or EKKO)
    version = _run(EKKO, "--version")
    rev = _run("git", "-C", REPO, "rev-parse", "--short", "HEAD")
    dirty = _run("git", "-C", REPO, "status", "--porcelain", "--untracked-files=no")
    return f"ekko {version} at {resolved} · checkout {rev}{'+dirty' if dirty else ''}"


# ---- boards ------------------------------------------------------------


def board_dir(name):
    return os.path.join(SCRATCH, "boards", name)


def scratch(name):
    """A fresh, empty scratch board: <scratch>/boards/<name>/{home,board}."""
    path = board_dir(name)
    shutil.rmtree(path, ignore_errors=True)
    os.makedirs(os.path.join(path, "home"))
    os.makedirs(os.path.join(path, "board"))
    return path


def storage_file(path):
    return os.path.join(path, "board", ".ekko", "storage", "storage.json")


def env_for(path):
    env = dict(os.environ)
    env["HOME"] = os.path.join(path, "home")
    env["EKKO_DIR"] = os.path.join(path, "board")
    env.pop("EKKO_PROJECT", None)
    return env


def registered_projects():
    """Names in the registry `ekko init` writes, in registry order."""
    registry = os.path.join(EKKO_HOME, "projects.json")
    if not os.path.exists(registry):
        return []
    with open(registry) as f:
        return [entry["name"] for entry in json.load(f).get("projects", [])]


def real_storage(project=None):
    """(label, storage.json) of a real board, found the way ekko finds it since
    7bf8191: a project registered in projects.json, a legacy projects/<name>/
    not adopted yet, or the default board. EKKO_EVAL_STORAGE points straight at
    a storage.json instead."""
    if os.environ.get("EKKO_EVAL_STORAGE"):
        return "EKKO_EVAL_STORAGE", os.environ["EKKO_EVAL_STORAGE"]
    if project is None:
        return "default board", os.path.join(EKKO_HOME, "storage", "storage.json")
    registry = os.path.join(EKKO_HOME, "projects.json")
    if os.path.exists(registry):
        with open(registry) as f:
            for entry in json.load(f).get("projects", []):
                if entry.get("name") == project:
                    return f"project {project}", os.path.join(entry["path"], ".ekko", "storage", "storage.json")
    legacy = os.path.join(EKKO_HOME, "projects", project, ".ekko", "storage", "storage.json")
    if os.path.exists(legacy):
        return f"legacy project {project}", legacy
    raise SystemExit(f"no project named {project}: not in {registry}, and no {legacy}")


def copy_real(name, project=None):
    """A scratch board holding a copy of a real board's storage and phases."""
    label, source = real_storage(project)
    if not os.path.exists(source):
        raise SystemExit(f"{label}: no storage at {source}")
    path = scratch(name)
    target = storage_file(path)
    os.makedirs(os.path.dirname(target))
    shutil.copy(source, target)
    phases = os.path.join(os.path.dirname(source), "phases.json")
    if os.path.exists(phases):
        shutil.copy(phases, os.path.join(os.path.dirname(target), "phases.json"))
    return path, label


def load(path):
    """A scratch board's items, by display id."""
    with open(storage_file(path)) as f:
        return {int(key): item for key, item in json.load(f).items()}


# ---- the server --------------------------------------------------------


class Mcp:
    """One `ekko --mcp` process, spoken to one line at a time. Latency is the
    time from writing a request to reading its reply, inside a warm server."""

    def __init__(self, path, binary=None):
        self.path = path
        self.proc = subprocess.Popen(
            [binary or EKKO, "--mcp"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            env=env_for(path),
            cwd=os.path.join(path, "home"),
            text=True,
            bufsize=1,
        )
        self.next_id = 0
        reply, _ = self.rpc(
            "initialize",
            {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "ekko-evals", "version": "0"}},
        )
        self.instructions = reply.get("result", {}).get("instructions", "")

    def rpc(self, method, params=None):
        self.next_id += 1
        message = {"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params or {}}
        start = time.perf_counter()
        self.proc.stdin.write(json.dumps(message) + "\n")
        self.proc.stdin.flush()
        line = self.proc.stdout.readline()
        return json.loads(line), time.perf_counter() - start

    def call(self, name, **arguments):
        """(text, is_error, seconds) for one tools/call."""
        reply, seconds = self.rpc("tools/call", {"name": name, "arguments": arguments})
        if "error" in reply:
            return "RPC-ERROR " + json.dumps(reply["error"]), True, seconds
        result = reply["result"]
        return result["content"][0]["text"], bool(result.get("isError")), seconds

    def tools(self):
        reply, _ = self.rpc("tools/list")
        return reply["result"]["tools"]

    def close(self):
        self.proc.stdin.close()
        self.proc.wait(timeout=30)


def cli(path, *args, stdin=None):
    """The CLI against a scratch board, stdout and stderr together."""
    out = subprocess.run(
        [EKKO, *args], env=env_for(path), cwd=os.path.join(path, "home"), capture_output=True, text=True, input=stdin
    )
    return out.stdout + out.stderr


# ---- reading replies ---------------------------------------------------

ID_LINE = re.compile(r"^\s*(\d+)\.\s")


def line_ids(text):
    """Ids of the lines that start with one, the way every agent view prints items."""
    return [int(match.group(1)) for match in map(ID_LINE.match, text.splitlines()) if match]


def cursor_of(text):
    match = re.search(r"cursor (\d+)", text)
    return int(match.group(1)) if match else None


def written_ids(reply):
    """Display ids a write reported, from a single write or a batch."""
    data = json.loads(reply)
    if "results" in data:
        return [entry["id"] for group in data["results"] for entry in group]
    return [entry["id"] for entry in data.get("items", [])]


def size(text):
    return len(text.encode())


def tokens(nbytes):
    """Estimated tokens for ekko output: bytes / 2.5, and bytes / 2 as the
    conservative bound. Claude 4.7 and later tokenizers produce about 30% more
    tokens than earlier ones (platform docs, token counting), and the resume
    eval measured about 2 characters per token on real ekko output."""
    return nbytes / 2.5, nbytes / 2


def show(label, text, is_error=False, seconds=None):
    flag = " [isError]" if is_error else ""
    timing = f" ({seconds * 1000:.1f} ms)" if seconds is not None else ""
    print(f"--- {label}{flag}{timing} {size(text)} bytes")
    print(text.rstrip())


def results_file(kind):
    """Where a run's numbers go: target/evals/results/<kind>-<commit>-<time>.json."""
    rev = _run("git", "-C", REPO, "rev-parse", "--short", "HEAD") or "unknown"
    folder = os.path.join(SCRATCH, "results")
    os.makedirs(folder, exist_ok=True)
    return os.path.join(folder, f"{kind}-{rev}-{time.strftime('%Y%m%dT%H%M%S')}.json")
