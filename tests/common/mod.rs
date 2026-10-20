//! How every integration test starts a process (task 1666): with none of
//! the variables ekko reads that the environment running the tests may hold
//! -- a Claude Code session's, one `ekko agents start` opened, the user's
//! own -- and with HOME where no board is, so that a test which sets none
//! fails rather than write the board of whoever runs it. A test sets the
//! ones it means, after. tests/cli.rs holds these lists to ekko's sources:
//! a variable read anew fails a test until it is placed in one of them.

#![allow(dead_code)]

use std::ffi::OsStr;
use std::process::Command;

/// The variables no test inherits.
pub const SESSION_VARIABLES: &[&str] = &[
    // Another board.
    "EKKO_DIR",
    "EKKO_PROJECT",
    // A born session's tasks: every other task refused; and its worktree
    // and branch, which it keeps to (task 1800).
    "EKKO_AGENT_TASK",
    "EKKO_AGENT_WORKTREE",
    "EKKO_AGENT_BRANCH",
    // How long a born session waits on the user before its watcher tells
    // them (task 1832).
    "EKKO_AGENTS_TELL_AFTER",
    // The session's own: ekko would act as it, and read its transcripts and
    // its task list.
    "CLAUDECODE",
    "CLAUDE_CONFIG_DIR",
    "CLAUDE_CODE_TASK_LIST_ID",
    // Born sessions' state, out of the test's home.
    "XDG_STATE_HOME",
    // Sessions started, and windows renamed, in the session's own tmux.
    "EKKO_MUX",
    "EKKO_MUX_SOCKET",
    "EKKO_CLAUDE",
    "TMUX",
    "TMUX_PANE",
    // A menu opened on the screen of whoever runs the tests.
    "EKKO_TERMINAL",
    "TERMINAL",
    "DISPLAY",
    "WAYLAND_DISPLAY",
    // Colors in what ekko prints.
    "NO_COLOR",
    "FORCE_COLOR",
];

/// HOME until a test sets its own: a folder that does not exist.
pub const NO_HOME: &str = "/nonexistent/ekko-test-home";

/// The session bus every process a test starts is given: an address where
/// no bus is (task 2250). A desktop notification -- a born session's
/// watcher telling of a wait, a menu's reminder -- goes through busctl,
/// which finds the bus of whoever runs the tests through this variable, or
/// else XDG_RUNTIME_DIR, which tests inherit; with it here, busctl fails
/// to connect, and nothing reaches their desktop.
pub const NO_BUS: &str = "unix:path=/nonexistent/ekko-test-bus";

/// The variables ekko reads that tests inherit as they are, each for a
/// reason: PATH finds the programs a test runs, XDG_RUNTIME_DIR is where a
/// menu's file goes, and a test that opens one sets it; the rest are read
/// only by tests in src/ run by hand, never by the binary.
pub const INHERITED: &[&str] = &[
    "PATH",
    "XDG_RUNTIME_DIR",
    "EKKO_CUES",
    "EKKO_CUE_CORPUS",
    "EKKO_CUE_OUT",
    "EKKO_CUE_READING",
    "EKKO_SHELL_CORPUS",
    "EKKO_SHELL_OUT",
];

/// `program`, started without any of `SESSION_VARIABLES`, with HOME at
/// `NO_HOME`, and with the session bus at `NO_BUS`.
pub fn command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    for name in SESSION_VARIABLES {
        command.env_remove(name);
    }
    command.env("HOME", NO_HOME);
    command.env("DBUS_SESSION_BUS_ADDRESS", NO_BUS);
    command
}

/// The ekko under test, started as `command` starts a program.
pub fn ekko() -> Command {
    command(env!("CARGO_BIN_EXE_ekko"))
}

/// The board in `ekko_dir`, a board's `.ekko/` folder, as it is stored, each
/// item's JSON as it was written: from the board's database, or from
/// storage.json before the board has one (task 2233). What a test reads
/// where it read storage.json, which is now the stamp a watcher follows.
pub fn stored_board(ekko_dir: &std::path::Path) -> serde_json::Value {
    let database = ekko_dir.join("storage").join("board.sqlite");
    if !database.exists() {
        let raw = std::fs::read_to_string(ekko_dir.join("storage").join("storage.json")).expect("the board has no database and no storage.json");
        return serde_json::from_str(&raw).expect("storage.json is a board");
    }
    let conn = rusqlite::Connection::open_with_flags(&database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let mut statement = conn.prepare("SELECT id, body FROM items ORDER BY id").unwrap();
    let rows = statement.query_map([], |row| Ok((row.get::<_, u32>(0)?, row.get::<_, String>(1)?))).unwrap();
    let mut board = serde_json::Map::new();
    for row in rows {
        let (id, body) = row.unwrap();
        board.insert(id.to_string(), serde_json::from_str(&body).unwrap());
    }
    serde_json::Value::Object(board)
}

/// `stored_board` as text, for a test that looks for words in it.
pub fn stored_text(ekko_dir: &std::path::Path) -> String {
    stored_board(ekko_dir).to_string()
}

/// `stored_text`, or nothing where the board has neither a database nor a
/// storage.json.
pub fn stored_text_or_empty(ekko_dir: &std::path::Path) -> String {
    let storage = ekko_dir.join("storage");
    if storage.join("board.sqlite").exists() || storage.join("storage.json").exists() {
        stored_text(ekko_dir)
    } else {
        String::new()
    }
}

/// Puts `board` in place of the board stored in `ekko_dir`, as a hand would
/// edit storage.json before the board's database: each item's JSON written
/// as given, with the count of writes moved so that every process reads it
/// again. For a test that sets a board up the way only the user's menu
/// could leave it.
pub fn store_board(ekko_dir: &std::path::Path, board: &serde_json::Value) {
    let database = ekko_dir.join("storage").join("board.sqlite");
    if !database.exists() {
        std::fs::write(ekko_dir.join("storage").join("storage.json"), board.to_string()).unwrap();
        return;
    }
    let mut conn = rusqlite::Connection::open(&database).unwrap();
    let tx = conn.transaction().unwrap();
    tx.execute("DELETE FROM items", []).unwrap();
    for (id, item) in board.as_object().expect("a board is an object of items by id") {
        tx.execute("INSERT INTO items (id, body) VALUES (?1, ?2)", rusqlite::params![id.parse::<u32>().unwrap(), item.to_string()]).unwrap();
    }
    tx.execute("UPDATE meta SET value = value + 1 WHERE key IN ('writes', 'commits')", []).unwrap();
    tx.commit().unwrap();
}
