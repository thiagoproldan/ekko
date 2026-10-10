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
    // A born session's tasks: every other task refused.
    "EKKO_AGENT_TASK",
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

/// `program`, started without any of `SESSION_VARIABLES` and with HOME at
/// `NO_HOME`.
pub fn command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    for name in SESSION_VARIABLES {
        command.env_remove(name);
    }
    command.env("HOME", NO_HOME);
    command
}

/// The ekko under test, started as `command` starts a program.
pub fn ekko() -> Command {
    command(env!("CARGO_BIN_EXE_ekko"))
}
