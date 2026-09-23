//! CLI-level behaviours that only show up when the real binary meets a real
//! shell: signals, pipes, exit codes. None of it is reachable from the unit
//! tests, which drive the library side and never touch a file descriptor
//! they did not create.

use std::fs;
use std::io::Read as _;
use std::path::PathBuf;
use std::process::{self, Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

/// A counter beside the clock: tests run in parallel, and two can read the
/// same clock value (task 393).
fn temp_ekko_dir() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "ekko-e2e-cli-{}-{}-{}",
        process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(dir.join(".ekko").join("storage")).unwrap();
    dir
}

/// Writes a board big enough that its `--json` output cannot fit in a pipe
/// buffer, so the child is still writing when the reader goes away. Built
/// directly rather than by spawning the binary a few hundred times.
fn seed_large_board(dir: &std::path::Path, count: u32) {
    let items: Vec<String> = (1..=count)
        .map(|id| {
            format!(
                r#""{id}":{{"_id":{id},"_date":"Mon Aug 24 2026","_timestamp":1787600000000,"description":"item number {id}, padded out so the whole board comfortably exceeds a pipe buffer","isStarred":false,"boards":["@bench"],"_isTask":true,"isComplete":false,"inProgress":false,"priority":1}}"#
            )
        })
        .collect();
    fs::write(
        dir.join(".ekko").join("storage").join("storage.json"),
        format!("{{{}}}", items.join(",")),
    )
    .unwrap();
}

/// Rust disables SIGPIPE at startup, which turns the most ordinary shell
/// idiom there is -- `ekko --json | head -1` -- into a panic. Restoring the
/// default disposition is what every other Unix tool does.
#[test]
fn a_reader_that_goes_away_does_not_produce_a_panic() {
    let dir = temp_ekko_dir();
    seed_large_board(&dir, 600);

    let mut child = Command::new(env!("CARGO_BIN_EXE_ekko"))
        .args(["--ekko-dir", dir.to_str().unwrap(), "--json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn ekko");

    // Read a token amount, then drop the pipe while the child is still
    // writing -- exactly what `head -1` does.
    let mut stdout = child.stdout.take().expect("stdout was piped");
    let mut head = [0u8; 32];
    stdout.read_exact(&mut head).expect("expected some output before the pipe closed");
    drop(stdout);

    let output = child.wait_with_output().expect("failed to wait on ekko");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!stderr.contains("panicked"), "broken pipe produced a panic:\n{stderr}");
    assert!(
        !stderr.contains("Broken pipe"),
        "broken pipe was reported to the user:\n{stderr}"
    );

    fs::remove_dir_all(&dir).ok();
}

/// The same run, uninterrupted, still has to behave normally -- it would be
/// easy to "fix" the pipe case by breaking the ordinary one.
#[test]
fn output_that_is_read_to_the_end_still_succeeds() {
    let dir = temp_ekko_dir();
    seed_large_board(&dir, 20);

    let output = Command::new(env!("CARGO_BIN_EXE_ekko"))
        .args(["--ekko-dir", dir.to_str().unwrap(), "--json"])
        .output()
        .expect("failed to run ekko");

    assert!(output.status.success(), "exit status was {:?}", output.status);
    let stdout = String::from_utf8(output.stdout).expect("--json output should be UTF-8");
    assert_eq!(stdout.lines().count(), 2, "board line plus stats line");

    fs::remove_dir_all(&dir).ok();
}

/// `--anchor` and `--path` were renamed. An agent working from an older copy
/// of the skill still sends the old names, and clap's own "unexpected
/// argument" would read as the feature being gone -- so each old name is
/// answered, under a stable code, with the name it has now.
#[test]
fn an_old_flag_name_is_answered_with_the_new_one() {
    let dir = temp_ekko_dir();

    for (old, new) in [("--anchor", "--attached-to"), ("--path", "--roadmap")] {
        let output = Command::new(env!("CARGO_BIN_EXE_ekko"))
            .args(["--ekko-dir", dir.to_str().unwrap(), "--json", old])
            .output()
            .expect("failed to run ekko");

        assert!(!output.status.success(), "{old} succeeded");
        let reply: serde_json::Value =
            serde_json::from_slice(&output.stdout).expect("a --json error reply");
        assert_eq!(reply["code"], "RENAMED_FLAG", "{old}: {reply}");
        assert_eq!(reply["renamedTo"], new, "{old}: {reply}");
    }

    fs::remove_dir_all(&dir).ok();
}

/// `--ui` opened the interactive mode, which was removed. Someone who types
/// it from habit is told so under a stable code, and where the board is,
/// before anything beside it runs -- not clap's "unexpected argument", which
/// reads as a typo.
#[test]
fn the_removed_ui_flag_says_it_was_removed() {
    let dir = temp_ekko_dir();
    let ekko = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_ekko"))
            .args(["--ekko-dir", dir.to_str().unwrap()])
            .args(args)
            .output()
            .expect("failed to run ekko")
    };

    let output = ekko(&["--json", "--ui", "--task", "never written"]);
    assert!(!output.status.success(), "--ui succeeded");
    let reply: serde_json::Value = serde_json::from_slice(&output.stdout).expect("a --json error reply");
    assert_eq!(reply["code"], "REMOVED_FLAG", "{reply}");
    assert!(reply["error"].as_str().unwrap_or("").contains("--ui was removed"), "{reply}");
    let board = fs::read_to_string(dir.join(".ekko").join("storage").join("storage.json")).unwrap_or_default();
    assert!(!board.contains("never written"), "the task beside --ui was created: {board}");

    let output = ekko(&["--ui"]);
    assert!(!output.status.success(), "--ui succeeded");
    let said = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert!(said.contains("--ui was removed"), "{said}");

    fs::remove_dir_all(&dir).ok();
}

/// `ekko init` makes a folder a project, and from then on `ekko` anywhere
/// inside that folder works on the project, while outside it works on the
/// default board. The old way of making a project answers with the new one.
#[test]
fn init_makes_a_folder_a_project_that_ekko_finds_from_inside_it() {
    let home = temp_ekko_dir();
    let app = home.join("work").join("app");
    let deep = app.join("src");
    let elsewhere = home.join("elsewhere");
    for dir in [&deep, &elsewhere] {
        fs::create_dir_all(dir).unwrap();
    }
    let ekko = |cwd: &PathBuf, args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_ekko"))
            .args(args)
            .current_dir(cwd)
            .env("HOME", &home)
            .env_remove("EKKO_DIR")
            .env_remove("EKKO_PROJECT")
            .output()
            .expect("failed to run ekko")
    };
    let reply = |output: &process::Output| -> serde_json::Value {
        serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&output.stdout)))
    };

    let init = ekko(&app, &["--json", "init"]);
    assert!(init.status.success(), "{}", reply(&init));
    assert_eq!(reply(&init)["project"]["name"], "app");

    assert!(ekko(&deep, &["--task", "inside the project"]).status.success());
    assert!(ekko(&elsewhere, &["--task", "on the default board"]).status.success());

    let project_board = fs::read_to_string(app.join(".ekko").join("storage").join("storage.json")).unwrap();
    let default_board = fs::read_to_string(home.join(".ekko").join("storage").join("storage.json")).unwrap();
    assert!(project_board.contains("inside the project"), "{project_board}");
    assert!(!project_board.contains("on the default board"), "{project_board}");
    assert!(default_board.contains("on the default board"), "{default_board}");
    assert!(!default_board.contains("inside the project"), "{default_board}");

    let retired = ekko(&elsewhere, &["--json", "--project", "app", "--create"]);
    assert_eq!(reply(&retired)["code"], "RENAMED_FLAG");
    assert_eq!(reply(&retired)["renamedTo"], "ekko init");

    fs::remove_dir_all(&home).ok();
}

/// The refusal and the override both have to survive the real argument
/// parser: `--force` is a flag no unit test ever parses, and one accepted
/// where it means nothing would be a flag that silently does nothing.
#[test]
fn a_blocked_task_needs_force_and_force_needs_a_task_to_complete() {
    let dir = temp_ekko_dir();
    let ekko = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_ekko"))
            .args(["--ekko-dir", dir.to_str().unwrap(), "--json"])
            .args(args)
            .output()
            .expect("failed to run ekko")
    };
    let reply = |output: &process::Output| -> serde_json::Value {
        serde_json::from_slice(&output.stdout).expect("a --json reply")
    };

    assert!(ekko(&["--task", "blocker"]).status.success());
    assert!(ekko(&["--task", "blocked"]).status.success());
    assert!(ekko(&["--blocked-by", "@2", "1"]).status.success());

    let refused = ekko(&["--check", "2"]);
    assert!(!refused.status.success(), "a blocked task was completed");
    assert_eq!(reply(&refused)["code"], "BLOCKED");

    let forced = ekko(&["--check", "2", "--force"]);
    assert!(forced.status.success(), "{}", reply(&forced));
    assert_eq!(reply(&forced)["overridden"][0]["blockers"][0], 1);

    let pointless = ekko(&["--force"]);
    assert!(!pointless.status.success(), "--force alone was accepted");
    assert_eq!(reply(&pointless)["code"], "FORCE_WITHOUT_COMPLETING");

    fs::remove_dir_all(&dir).ok();
}

/// `--kind` and `--supersedes` only mean something beside `--note`, and the
/// real parser has to say so rather than accept them and write an ordinary
/// note -- or a task -- with the kind quietly dropped.
#[test]
fn a_typed_note_takes_its_kind_from_the_flags_beside_note() {
    let dir = temp_ekko_dir();
    let ekko = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_ekko"))
            .args(["--ekko-dir", dir.to_str().unwrap(), "--json"])
            .args(args)
            .output()
            .expect("failed to run ekko")
    };
    let reply = |output: &process::Output| -> serde_json::Value {
        serde_json::from_slice(&output.stdout).expect("a --json reply")
    };

    let first = ekko(&["--note", "--kind", "decision", "ship weekly"]);
    assert!(first.status.success(), "{}", reply(&first));
    assert_eq!(reply(&first)["item"]["knowledge"], "decision");
    let second = ekko(&["--note", "--kind", "decision", "--supersedes", "1", "ship on demand"]);
    assert!(second.status.success(), "{}", reply(&second));
    assert_eq!(reply(&second)["item"]["supersedes"], reply(&first)["item"]["uid"]);

    let stray = ekko(&["--task", "--kind", "gotcha", "a task"]);
    assert!(!stray.status.success(), "--kind was accepted beside --task");
    let board = fs::read_to_string(dir.join(".ekko").join("storage").join("storage.json")).unwrap();
    assert!(!board.contains("a task"), "{board}");

    fs::remove_dir_all(&dir).ok();
}

/// A lone `-` takes the description from stdin, verbatim: apostrophes,
/// quotes and newlines kept, and a first word like `@x` or `d:` not read as a
/// board or a due date. A `-` among other words is only a word, and stdin is
/// never read for it.
#[test]
fn a_lone_dash_reads_the_description_from_stdin() {
    use std::io::Write as _;
    let dir = temp_ekko_dir();
    let ekko = |args: &[&str], stdin: &str| -> serde_json::Value {
        let mut child = Command::new(env!("CARGO_BIN_EXE_ekko"))
            .args(["--ekko-dir", dir.to_str().unwrap(), "--json"])
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("failed to run ekko");
        child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
        let output = child.wait_with_output().unwrap();
        serde_json::from_slice(&output.stdout).expect("a --json reply")
    };

    let text = "it's the user's \"call\"\nsecond line";
    let task = ekko(&["--task", "@coding", "p:2", "-"], &format!("{text}\n"));
    assert_eq!(task["item"]["description"], text, "{task}");
    assert_eq!((task["item"]["boards"][0].as_str(), task["item"]["priority"].as_u64()), (Some("@coding"), Some(2)));

    let note = ekko(&["--note", "--kind", "gotcha", "-"], "@x d:soon is not a board or a date");
    assert_eq!(note["item"]["description"], "@x d:soon is not a board or a date", "{note}");
    assert_eq!(note["item"]["boards"][0], "My Board");

    let edited = ekko(&["--edit", "@1", "-"], "don't drop the apostrophe");
    assert_eq!(edited["item"]["description"], "don't drop the apostrophe", "{edited}");

    let literal = ekko(&["--task", "fix", "-", "now"], "never read");
    assert_eq!(literal["item"]["description"], "fix - now", "{literal}");

    let empty = ekko(&["--task", "-"], "  \n");
    assert_eq!(empty["code"], "MISSING_DESC", "{empty}");

    fs::remove_dir_all(&dir).ok();
}

/// The plugin's task-list hook through the real binary, in a project found
/// from the folder, as a session runs it: the event on stdin, Claude Code's
/// config directory from CLAUDE_CONFIG_DIR, the list written under the
/// session's id, and a reply that is JSON and nothing else -- a project's
/// header above it would turn the watchPaths into context. `--tasklist`
/// means nothing without `--hook`, and `--hook` nothing without `--prime` or
/// `--tasklist`, so each alone is refused.
#[test]
fn the_tasklist_hook_writes_the_sessions_list() {
    use std::io::Write as _;
    let dir = temp_ekko_dir();
    let app = dir.join("app");
    fs::create_dir_all(&app).unwrap();
    let config = dir.join("claude");
    let ekko = |args: &[&str], stdin: &str| {
        let mut child = Command::new(env!("CARGO_BIN_EXE_ekko"))
            .args(args)
            .current_dir(&app)
            .env("HOME", &dir)
            .env_remove("EKKO_DIR")
            .env_remove("EKKO_PROJECT")
            .env("CLAUDE_CONFIG_DIR", &config)
            .env_remove("CLAUDE_CODE_TASK_LIST_ID")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to run ekko");
        child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
        child.wait_with_output().unwrap()
    };

    assert!(ekko(&["init"], "").status.success());
    assert!(ekko(&["--task", "the work in progress"], "").status.success());
    assert!(ekko(&["--begin", "1"], "").status.success());
    let started = ekko(&["--tasklist", "--hook"], r#"{"session_id":"s-1","hook_event_name":"SessionStart"}"#);
    assert!(started.status.success(), "{}", String::from_utf8_lossy(&started.stderr));
    let reply: serde_json::Value = serde_json::from_slice(&started.stdout).expect("a JSON hook reply");
    assert_eq!(reply["hookSpecificOutput"]["watchPaths"][0].as_str(), app.join(".ekko/storage/storage.json").to_str(), "{reply}");
    let task: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(config.join("tasks").join("s-1").join("1.json")).unwrap()).unwrap();
    assert_eq!((task["status"].as_str(), task["activeForm"].as_str()), (Some("in_progress"), Some("1. the work in progress")));

    assert!(!ekko(&["--tasklist"], "").status.success(), "--tasklist without --hook was accepted");
    assert!(!ekko(&["--hook"], "").status.success(), "--hook alone was accepted");

    fs::remove_dir_all(&dir).ok();
}
