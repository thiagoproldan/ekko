"""A board's items as ekko keeps them, for the evals (task 2233).

From the ekko that brought it, a board lives in storage/board.sqlite, and
storage.json beside it is only a stamp a watcher follows: {"0": "An ekko
from before boards were kept in SQLite cannot read this board: ..."}, the
words of STAMP in src/storage.rs. A board no such ekko has written to yet
is storage.json alone, and its first write moves it into the database.
Every eval reads, seeds, copies and scrubs boards through here, so
neither form is assumed, and the database's layout is known in this one place
on the Python side, as tests/common/mod.rs knows it on the Rust side. Reading
the database straight, not through `ekko --export`, keeps the ground truth an
eval computes independent of ekko's own code.

    python3 evals/boards.py BOARD    prints the items of the board whose
                                     .ekko folder is BOARD, as JSON
"""

import glob
import json
import os
import sqlite3
import sys

DATABASE = "board.sqlite"
# The layout these functions know: the database's user_version.
LAYOUT = 1
LOGS = ("-wal", "-shm", "-journal")


def database_of(board):
    return os.path.join(board, "storage", DATABASE)


def storage_json(board):
    return os.path.join(board, "storage", "storage.json")


def is_stamp(items):
    return isinstance(items, dict) and list(items) == ["0"] and isinstance(items["0"], str)


def exists(board):
    return os.path.exists(database_of(board)) or os.path.exists(storage_json(board))


def _connect(path, write=False):
    conn = sqlite3.connect(path if write else f"file:{path}?mode=ro", uri=not write)
    layout = conn.execute("PRAGMA user_version").fetchone()[0]
    if layout > LAYOUT:
        conn.close()
        raise SystemExit(f"{path} has layout {layout}, from an ekko later than these evals, which know layout {LAYOUT}")
    return conn


def read(board):
    """The items of the board whose .ekko folder is `board`, as storage.json
    held them: by display id, as a string."""
    if os.path.exists(database_of(board)):
        conn = _connect(database_of(board))
        try:
            return {str(id): json.loads(body) for id, body in conn.execute("SELECT id, body FROM items ORDER BY id")}
        finally:
            conn.close()
    with open(storage_json(board)) as handle:
        items = json.load(handle)
    if is_stamp(items):
        raise SystemExit(f"{storage_json(board)} says the board is kept in {DATABASE}, which is missing")
    return items


def counters(board):
    """The board's counters, as counters.json held them; {} when it has none."""
    if os.path.exists(database_of(board)):
        conn = _connect(database_of(board))
        try:
            row = conn.execute("SELECT value FROM meta WHERE key = 'counters'").fetchone()
        finally:
            conn.close()
        return json.loads(row[0]) if row and row[0] else {}
    try:
        with open(os.path.join(board, "storage", "counters.json")) as handle:
            return json.load(handle)
    except (OSError, ValueError):
        return {}


def _remove_database(board):
    for suffix in ("",) + LOGS:
        try:
            os.remove(database_of(board) + suffix)
        except FileNotFoundError:
            pass


def seed(board, items, counters=None):
    """Makes `items` the board in `board`, as storage.json held them, with
    `counters`: a board kept as JSON, which ekko moves into its database at its
    first write. A database there goes first, since it would be read instead."""
    os.makedirs(os.path.join(board, "storage"), exist_ok=True)
    _remove_database(board)
    with open(storage_json(board), "w") as handle:
        json.dump(items, handle, indent=4)
    if counters is not None:
        with open(os.path.join(board, "storage", "counters.json"), "w") as handle:
            json.dump(counters, handle, indent=2)


def copy_database(source, board):
    """Puts a copy of the database of the board in `source` into `board`, made
    through SQLite: a copy of the file, as rsync or cp makes one, can leave out
    the writes its log holds, and its log is no log of the copy. The copy's own
    log and journal go first. Nothing where `source` has no database."""
    if not os.path.exists(database_of(source)):
        return
    _remove_database(board)
    os.makedirs(os.path.join(board, "storage"), exist_ok=True)
    temp = database_of(board) + ".copying"
    if os.path.exists(temp):
        os.remove(temp)
    src = _connect(database_of(source))
    try:
        dst = sqlite3.connect(temp)
        try:
            src.backup(dst)
        finally:
            dst.close()
    finally:
        src.close()
    os.replace(temp, database_of(board))


def remove(board, ids):
    """Takes the items with these display ids out of the board, and every
    change and journal line that names them, so that nothing in its files
    holds their text: the database is vacuumed, storage.json rewritten and,
    for a board kept as JSON, each version history/ kept scrubbed too.
    A file is replaced, never written through: a copy of a board may share
    its files' inodes with the real one by hard link."""
    ids = {int(id) for id in ids}
    if not ids:
        return
    if os.path.exists(database_of(board)):
        temp = database_of(board) + ".scrubbing"
        if os.path.exists(temp):
            os.remove(temp)
        conn = _connect(database_of(board))
        try:
            conn.execute("VACUUM INTO ?", (temp,))
        finally:
            conn.close()
        conn = _connect(temp, write=True)
        try:
            marks = ",".join("?" * len(ids))
            conn.execute(f"DELETE FROM items WHERE id IN ({marks})", tuple(ids))
            conn.execute(f"DELETE FROM changes WHERE id IN ({marks})", tuple(ids))
            for seq, entry in conn.execute("SELECT seq, entry FROM journal").fetchall():
                line = json.loads(entry)
                if any(int(named.get("id", -1)) in ids for key in ("released", "blocked", "removed") for named in line.get(key, [])):
                    for key in ("released", "blocked", "removed"):
                        line[key] = [named for named in line.get(key, []) if int(named.get("id", -1)) not in ids]
                    conn.execute("UPDATE journal SET entry = ? WHERE seq = ?", (json.dumps(line), seq))
            conn.execute("UPDATE meta SET value = value + 1 WHERE key IN ('writes', 'commits')")
            conn.commit()
            conn.execute("VACUUM")
        finally:
            conn.close()
        _remove_database(board)
        os.replace(temp, database_of(board))
        return
    _scrub_json(storage_json(board), ids)
    for file in sorted(glob.glob(os.path.join(board, "history", "*.json"))):
        _scrub_json(file, ids)


def _scrub_json(path, ids):
    with open(path) as handle:
        items = json.load(handle)
    if is_stamp(items):
        return
    kept = {key: item for key, item in items.items() if int(item.get("_id", key)) not in ids}
    if len(kept) == len(items):
        return
    stat = os.stat(path)
    with open(path + ".tmp", "w") as handle:
        json.dump(kept, handle)
    os.replace(path + ".tmp", path)
    os.utime(path, ns=(stat.st_atime_ns, stat.st_mtime_ns))


def snapshots(board):
    """Each time the board can be had back whole, in epoch seconds, with the
    key `at` takes: the versions history/ kept as JSON, by the time in their
    names, and, once the board has a database, the time of each change it
    keeps -- two weeks of them, any of which `at` gives back exactly."""
    out = []
    for file in glob.glob(os.path.join(board, "history", "*.json")):
        stem = os.path.basename(file)[:-5]
        if stem.isdigit() and not _stamp_file(file):
            out.append((int(stem) / 1e9, ("file", file)))
    if os.path.exists(database_of(board)):
        conn = _connect(database_of(board))
        try:
            out += [(at / 1000, ("database", at)) for (at,) in conn.execute("SELECT DISTINCT at FROM changes")]
        finally:
            conn.close()
    out.sort(key=lambda snap: snap[0])
    return out


def _stamp_file(path):
    with open(path, "rb") as handle:
        head = handle.read(64)
    return head.lstrip().startswith(b'{"0":') or head.lstrip().startswith(b'{\n    "0":')


def at(board, key):
    """The items of the snapshot `snapshots` named by `key`, by display id as
    an int: a version history/ kept, or the board just after the changes made
    at that time, undone back from now."""
    kind, value = key
    if kind == "file":
        with open(value) as handle:
            return {int(item["_id"]): item for item in json.load(handle).values()}
    conn = _connect(database_of(board))
    try:
        items = {id: body for id, body in conn.execute("SELECT id, body FROM items")}
        for id, before in conn.execute("SELECT id, before FROM changes WHERE at > ? ORDER BY seq DESC", (value,)):
            if before is None:
                items.pop(id, None)
            else:
                items[id] = before
    finally:
        conn.close()
    return {int(id): json.loads(body) for id, body in items.items()}


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit(__doc__)
    json.dump(read(sys.argv[1]), sys.stdout)
    sys.stdout.write("\n")
