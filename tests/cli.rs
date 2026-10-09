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

/// Writes `content` as an executable at `to` through a child process, so
/// that this test process never holds a descriptor open for writing on it.
/// A test's thread that forks while one is open hands it to its child,
/// which keeps it until its own exec, and running the file in that window
/// fails with ETXTBSY, "Text file busy" (task 1579).
fn write_executable(to: &std::path::Path, content: &str) {
    use std::io::Write as _;
    let mut child = Command::new("sh").args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"]).arg(to).stdin(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(content.as_bytes()).unwrap();
    assert!(child.wait().unwrap().success(), "{} was not written", to.display());
}

/// Copies the executable `from` to `to` through a child process, for the
/// reason `write_executable` gives.
fn copy_executable(from: &str, to: &std::path::Path) {
    assert!(Command::new("cp").arg(from).arg(to).status().unwrap().success(), "{} was not copied", to.display());
}

/// Every executable the integration tests run is put in place by a child
/// process (task 1579): none is written, nor copied, by a test process.
#[test]
fn no_test_here_writes_an_executable_it_runs() {
    let sources = [("cli.rs", include_str!("cli.rs")), ("mcp.rs", include_str!("mcp.rs")), ("serve.rs", include_str!("serve.rs")), ("concurrency.rs", include_str!("concurrency.rs"))];
    for (file, source) in sources {
        for written in [concat!("fs::", "copy("), concat!("from_", "mode(0o7")] {
            assert!(!source.contains(written), "tests/{file} puts an executable in place with {written}: use write_executable or copy_executable (task 1579)");
        }
    }
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

/// A board cleaned out of its folder, as `git clean -fdx` does, is not lost:
/// every write copied it under ~/.ekko/copies/, the prime in that folder
/// says so, --projects names the way back, and `ekko init` there takes it.
/// --destroy forgets a project whose folder is gone (task 503).
#[test]
fn a_board_cleaned_out_of_its_folder_comes_back_with_init() {
    let home = temp_ekko_dir();
    let app = home.join("work").join("app");
    let gone = home.join("work").join("gone");
    fs::create_dir_all(app.join(".git")).unwrap();
    fs::create_dir_all(&gone).unwrap();
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
    let text = |output: &process::Output| String::from_utf8_lossy(&output.stdout).into_owned();
    let reply = |output: &process::Output| -> serde_json::Value {
        serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&output.stdout)))
    };

    assert!(ekko(&app, &["init"]).status.success());
    assert!(ekko(&app, &["--task", "survives git clean"]).status.success());
    fs::remove_dir_all(app.join(".ekko")).unwrap();

    let prime = text(&ekko(&app, &["--prime"]));
    assert!(prime.contains("was project app, whose board is gone: ekko init"), "{prime}");
    let projects = text(&ekko(&home, &["--projects"]));
    assert!(projects.contains("restores it"), "{projects}");

    let init = reply(&ekko(&app, &["--json", "init"]));
    assert!(init["restored"]["copied"].is_string(), "{init}");
    let board = fs::read_to_string(app.join(".ekko").join("storage").join("storage.json")).unwrap();
    assert!(board.contains("survives git clean"), "{board}");

    assert!(ekko(&gone, &["init"]).status.success());
    assert!(ekko(&gone, &["--task", "forgotten with its folder"]).status.success());
    fs::remove_dir_all(&gone).unwrap();
    let destroyed = reply(&ekko(&home, &["--json", "--project", "gone", "--destroy"]));
    assert_eq!((&destroyed["project"], &destroyed["forgotten"]), (&serde_json::json!("gone"), &serde_json::json!(true)));
    assert!(destroyed["trash"].is_string(), "{destroyed}");
    let projects = text(&ekko(&home, &["--projects"]));
    assert!(projects.contains("app") && !projects.contains("gone"), "{projects}");

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

/// `with:NAME` and `--with` through the real parser: the token comes off the
/// description, `--with` changes it and with no name clears it, and `--list
/// with:NAME` finds the task, case aside.
#[test]
fn with_says_who_a_task_is_with_and_the_list_finds_it() {
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

    let made = ekko(&["--task", "Send", "the", "contract", "with:Rodrigo"]);
    assert!(made.status.success(), "{}", reply(&made));
    assert_eq!((&reply(&made)["item"]["description"], &reply(&made)["item"]["with"]), (&"Send the contract".into(), &"rodrigo".into()));
    assert!(ekko(&["--task", "Something else"]).status.success());

    // The list, then the stats line: one JSON reply per line.
    let listed = ekko(&["--list", "with:RODRIGO"]);
    let listed: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&listed.stdout).lines().next().unwrap()).unwrap();
    let found: Vec<&serde_json::Value> = listed["boards"].as_object().unwrap().values().flat_map(|items| items.as_array().unwrap()).collect();
    assert_eq!(found.iter().map(|item| item["_id"].as_u64().unwrap()).collect::<Vec<_>>(), vec![1], "{listed}");

    assert_eq!(reply(&ekko(&["--with", "@1", "ana"]))["item"]["with"], "ana");
    let cleared = reply(&ekko(&["--with", "@1"]));
    assert!(cleared["item"].get("with").is_none(), "{cleared}");
    let refused = ekko(&["--with", "@1", "two", "words"]);
    assert!(!refused.status.success(), "two words were taken for a name");
    assert_eq!(reply(&refused)["code"], "INVALID_INPUT");

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

/// A typed note written or edited at the terminal has its `Rests on:` line
/// read from the project's folder, and what does not hold, or is no anchor,
/// is said under the message (task 1324) -- under the one that created it,
/// what each anchor that holds found too, or that the line is missing, with
/// how to add one (task 1327).
#[test]
fn the_terminal_says_what_a_notes_rests_on_line_does_not_hold() {
    let home = temp_ekko_dir();
    let app = home.join("app");
    fs::create_dir_all(app.join("src")).unwrap();
    fs::write(app.join("src").join("lib.rs"), "pub fn kept() {}\n").unwrap();
    let ekko = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_ekko"))
            .args(args)
            .current_dir(&app)
            .env("HOME", &home)
            .env("EKKO_TERMINAL", "none")
            .env_remove("EKKO_DIR")
            .env_remove("EKKO_PROJECT")
            .output()
            .expect("failed to run ekko")
    };
    assert!(ekko(&["init"]).status.success());

    let text = "Kept stays\nRests on: `src/lib.rs` \"pub fn kept\"; `src/gone.rs`; the readme";
    let created = ekko(&["--note", "--kind", "gotcha", text]);
    assert!(created.status.success());
    let out = String::from_utf8_lossy(&created.stdout);
    let said: Vec<&str> = out.lines().filter(|line| line.trim_start().starts_with("Rests on:")).map(str::trim).collect();
    assert_eq!(
        said,
        [
            "Rests on: `src/lib.rs` holds \"pub fn kept\" at line 1",
            "Rests on: `src/gone.rs` is not there",
            "Rests on: \"the readme\" is no anchor: an anchor is a path in backticks, a path and words in double quotes, \
             Claude Code and its version, or recheck after a date",
        ],
        "created, every anchor is said (task 1327): {out}"
    );
    let board: serde_json::Value =
        serde_json::from_slice(&fs::read(app.join(".ekko").join("storage").join("storage.json")).unwrap()).unwrap();
    assert_eq!(board["1"]["restsOn"]["anchors"][0], serde_json::json!({"path": "src/lib.rs", "words": "pub fn kept", "held": true, "line": 1}));

    let edited = ekko(&["--edit", "@1", "Kept stays\nRests on: `src/lib.rs`"]);
    assert!(edited.status.success());
    let out = String::from_utf8_lossy(&edited.stdout);
    assert!(!out.contains("Rests on:"), "every anchor holds now: {out}");
    let board: serde_json::Value =
        serde_json::from_slice(&fs::read(app.join(".ekko").join("storage").join("storage.json")).unwrap()).unwrap();
    assert_eq!(board["1"]["restsOn"]["anchors"], serde_json::json!([{"path": "src/lib.rs", "held": true}]), "read again");

    let bare = ekko(&["--note", "--kind", "decision", "Ship weekly"]);
    let out = String::from_utf8_lossy(&bare.stdout);
    let said: Vec<&str> = out.lines().filter(|line| line.trim_start().starts_with("Rests on:")).map(str::trim).collect();
    let none = "Rests on: none, so no read can tell it may be stale: if it rests on code, a path, a version or a date, \
                add a line Rests on: `path` \"words its file holds\"; Claude Code X.Y.Z; recheck after YYYY-MM-DD";
    assert_eq!(said, [none], "the terminal knows no version: {out}");
    let plain = ekko(&["--note", "a plain note"]);
    assert!(!String::from_utf8_lossy(&plain.stdout).contains("Rests on"));

    fs::remove_dir_all(&home).ok();
}

/// The terminal's prime and context check a note's anchors as they show it
/// (task 1325): once its file no longer holds the words, the note is marked
/// to recheck, with why. The terminal judges no version: only a session's
/// client says which Claude Code reads.
#[test]
fn the_terminal_says_to_recheck_a_note_whose_ground_moved() {
    let home = temp_ekko_dir();
    let app = home.join("app");
    fs::create_dir_all(app.join("src")).unwrap();
    fs::write(app.join("src").join("lib.rs"), "pub fn kept() {}\n").unwrap();
    let ekko = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_ekko"))
            .args(args)
            .current_dir(&app)
            .env("HOME", &home)
            .env("EKKO_TERMINAL", "none")
            .env_remove("EKKO_DIR")
            .env_remove("EKKO_PROJECT")
            .output()
            .expect("failed to run ekko");
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    ekko(&["init"]);
    ekko(&["--note", "--kind", "gotcha", "Kept stays\nRests on: `src/lib.rs` \"pub fn kept\"; Claude Code 2.1.200"]);
    let read = ekko(&["--context", "1"]);
    assert!(!read.contains("recheck"), "{read}");

    fs::write(app.join("src").join("lib.rs"), "pub fn renamed() {}\n").unwrap();
    let why = "      to recheck: `src/lib.rs` no longer holds \"pub fn kept\"\n";
    let read = ekko(&["--context", "1"]);
    assert!(read.contains(&format!("      note, a gotcha \u{b7} My Board\n{why}")), "{read}");
    let prime = ekko(&["--prime"]);
    assert!(prime.contains(&format!("\n   1. [gotcha, to recheck] Kept stays\n{why}")), "{prime}");

    fs::remove_dir_all(&home).ok();
}

/// A recheck at the terminal (task 1326): an edit that adds a `Still true`
/// line answers a date to recheck after that it came after, so the note is
/// marked no more; the terminal knows no Claude Code version, and the
/// message under the edit says the recheck answers none.
#[test]
fn a_still_true_line_at_the_terminal_answers_a_date_and_says_it_knows_no_version() {
    let home = temp_ekko_dir();
    let app = home.join("app");
    fs::create_dir_all(&app).unwrap();
    let ekko = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_ekko"))
            .args(args)
            .current_dir(&app)
            .env("HOME", &home)
            .env("EKKO_TERMINAL", "none")
            .env_remove("EKKO_DIR")
            .env_remove("EKKO_PROJECT")
            .output()
            .expect("failed to run ekko");
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    ekko(&["init"]);
    let text = "A trap\nRests on: recheck after 2026-10-01; Claude Code 2.1.200";
    ekko(&["--note", "--kind", "gotcha", text]);
    let read = ekko(&["--context", "1"]);
    assert!(read.contains("      to recheck: recheck after 2026-10-01 is past\n") && read.contains("\nTo recheck a note: still true"), "{read}");

    let edited = ekko(&["--edit", "@1", &format!("{text}\nStill true, 2026-10-07: it is")]);
    let said = "Rests on: this Still true knew no Claude Code version, so the note is still seen with 2.1.200: \
                write the version you checked with in its place";
    assert!(edited.contains(said), "{edited}");
    let read = ekko(&["--context", "1"]);
    assert!(!read.contains("to recheck") && !read.contains("To recheck"), "{read}");

    fs::remove_dir_all(&home).ok();
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

/// A task's title past 80 characters, from --task or --edit, is cut at a
/// word rather than refused, and the terminal says where under the message
/// that it was written, as --json says it in notices (decision 1504).
#[test]
fn the_terminal_cuts_a_long_title_and_says_where() {
    let dir = temp_ekko_dir();
    let ekko = |args: &[&str]| -> String {
        let output = Command::new(env!("CARGO_BIN_EXE_ekko")).args(["--ekko-dir", dir.to_str().unwrap()]).args(args).output().expect("failed to run ekko");
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stdout));
        String::from_utf8(output.stdout).unwrap()
    };
    let long = "A title past eighty characters is cut at the last space within them, and the reply says where";
    let title = "A title past eighty characters is cut at the last space within them, and the";
    let said = |ran: usize| {
        format!(
            "Its first line ran {ran} characters, past the 80 a title takes, so it was cut at the last space within them and the rest starts the second line: the title reads \"{title}\". Edit it if it reads badly"
        )
    };

    let created = ekko(&["--task", long]);
    assert!(created.contains("Created task:") && created.contains(&said(93)), "{created}");
    let edited: serde_json::Value = serde_json::from_str(&ekko(&["--json", "--edit", "@1", &format!("{long}, again")])).unwrap();
    assert_eq!(edited["item"]["description"], format!("{title}\nreply says where, again"), "{edited}");
    assert_eq!(edited["notices"], serde_json::json!([said(100)]), "{edited}");
    let short = ekko(&["--task", "short"]);
    assert!(!short.contains("first line ran"), "{short}");
    let fits: serde_json::Value = serde_json::from_str(&ekko(&["--json", "--edit", "@2", "still short"])).unwrap();
    assert!(fits.get("notices").is_none(), "{fits}");

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

/// The memory hook as the plugin runs it at SessionStart (task 885): the
/// page in the project's board folder for a session that starts, and nothing
/// for one that resumes, nor on a board without a page -- plain text with no
/// project header above it, since all of stdout becomes context.
#[test]
fn the_memory_hook_gives_a_starting_session_the_project_page() {
    use std::io::Write as _;
    let dir = temp_ekko_dir();
    let app = dir.join("app");
    fs::create_dir_all(&app).unwrap();
    let ekko = |args: &[&str], stdin: &str| {
        let mut child = Command::new(env!("CARGO_BIN_EXE_ekko"))
            .args(args)
            .current_dir(&app)
            .env("HOME", &dir)
            .env_remove("EKKO_DIR")
            .env_remove("EKKO_PROJECT")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to run ekko");
        child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
        child.wait_with_output().unwrap()
    };
    let hook = |source: &str| {
        let output = ekko(&["--memory", "--hook"], &format!(r#"{{"session_id":"s-1","hook_event_name":"SessionStart","source":"{source}"}}"#));
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8(output.stdout).unwrap()
    };

    assert!(ekko(&["init"], "").status.success());
    assert_eq!(hook("startup"), "", "a board without a page put something in context");
    fs::write(app.join(".ekko").join("memory.md"), "# app\n\nWhat app is, as a whole.\n").unwrap();
    let started = hook("startup");
    let page = app.join(".ekko").join("memory.md");
    assert_eq!(
        started,
        format!(
            "ekko memory \u{b7} project app, found from this folder \u{b7} {} \u{b7} the user keeps this page: a change to it is theirs to make or approve\n# app\n\nWhat app is, as a whole.\n",
            page.display()
        )
    );
    assert_eq!(hook("resume"), "", "a resumed session was given the page it holds");
    assert!(!ekko(&["--memory"], "").status.success(), "--memory without --hook was accepted");

    fs::remove_dir_all(&dir).ok();
}

/// The guard as Claude Code runs it (task 805): a PreToolUse event on stdin,
/// a refusal on stdout, and silence for everything else -- a call no cue
/// names, and input it cannot read, since a broken guard must not break the
/// shell. `--refuse` answers another guard with its exit code.
#[test]
fn the_guard_answers_a_pre_tool_use_event_and_stays_silent_otherwise() {
    use std::io::Write as _;
    let home = temp_ekko_dir();
    let ekko = |args: &[&str], stdin: &str| {
        let mut child = Command::new(env!("CARGO_BIN_EXE_ekko"))
            .args(args)
            .current_dir(&home)
            .env("HOME", &home)
            .env_remove("EKKO_DIR")
            .env_remove("EKKO_PROJECT")
            .env_remove("CLAUDECODE")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to run ekko");
        child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
        child.wait_with_output().unwrap()
    };
    assert!(ekko(&["--note", "--kind", "gotcha", "Changing the Status field clears every item's status."], "").status.success());
    // The user's answer in ekko's menu is what sets a cue; here, the board as it leaves it.
    let file = home.join(".ekko").join("storage").join("storage.json");
    let mut board: serde_json::Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
    let gotcha = board.as_object_mut().unwrap().values_mut().find(|item| item["knowledge"] == "gotcha").unwrap();
    gotcha["cue"] = serde_json::json!({"command": "gh", "words": ["updateProjectV2Field"], "question": "q", "at": 0});
    fs::write(&file, board.to_string()).unwrap();

    let event = |command: &str| serde_json::json!({"tool_name": "Bash", "tool_input": {"command": command}, "cwd": home, "tool_use_id": "t1"}).to_string();
    let refused = ekko(&["--guard", "--hook"], &event("gh api graphql -f query='mutation { updateProjectV2Field }'"));
    assert!(refused.status.success());
    let reply: serde_json::Value = serde_json::from_slice(&refused.stdout).unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&refused.stdout)));
    assert_eq!(reply["hookSpecificOutput"]["permissionDecision"], "deny");
    let reason = reply["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap();
    assert!(reason.contains("clears every item's status") && reason.contains("allow set to \""), "{reason}");

    for silent in [event("gh issue list"), event("echo updateProjectV2Field"), "not json".to_string(), String::new()] {
        let passed = ekko(&["--guard", "--hook"], &silent);
        assert!(passed.status.success() && passed.stdout.is_empty(), "{silent}: {}", String::from_utf8_lossy(&passed.stdout));
    }

    let read = serde_json::json!({"tool_name": "Read", "tool_input": {"file_path": "/x"}, "cwd": home, "tool_use_id": "t2"}).to_string();
    let other = ekko(&["--guard", "--refuse", "ctx: it would bring secret material into the context"], &read);
    assert_eq!(other.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&other.stdout).contains("allow set to \""));
    assert_eq!(ekko(&["--guard", "--refuse", "ctx"], "not json").status.code(), Some(2));
    let bare = ekko(&["--guard"], "");
    assert!(!bare.status.success() && String::from_utf8_lossy(&bare.stderr).contains("--guard takes --hook"));

    fs::remove_dir_all(&home).ok();
}

/// `--move-to` through the real parser (task 897): a project by name, the
/// default board as `~`, a folder by its path; the reply gives each item's id
/// there, a note that went with its task says so, and the id it had answers
/// MOVED, naming where it went. `--force` is accepted beside it.
#[test]
fn move_to_takes_items_to_another_board_and_the_old_id_says_where() {
    let home = temp_ekko_dir();
    let notes = home.join("work").join("notes");
    fs::create_dir_all(&notes).unwrap();
    let ekko = |cwd: &PathBuf, args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_ekko"))
            .args(args)
            .current_dir(cwd)
            .env("HOME", &home)
            .env_remove("EKKO_DIR")
            .env_remove("EKKO_PROJECT")
            .env_remove("CLAUDECODE")
            .output()
            .expect("failed to run ekko")
    };
    let reply = |output: &process::Output| -> serde_json::Value {
        serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&output.stdout)))
    };
    let ids = |moved: &serde_json::Value| -> Vec<(u64, u64)> {
        moved["items"].as_array().unwrap().iter().map(|item| (item["id"].as_u64().unwrap(), item["as"].as_u64().unwrap())).collect()
    };

    assert!(ekko(&notes, &["init"]).status.success());
    assert!(ekko(&home, &["--task", "belongs to notes"]).status.success());
    assert!(ekko(&home, &["--note", "why it does"]).status.success());
    assert!(ekko(&home, &["--attached-to", "@2", "1"]).status.success());

    let moved = reply(&ekko(&home, &["--json", "--move-to", "notes", "1"]));
    assert_eq!((&moved["command"], &moved["project"]), (&serde_json::json!("move-to"), &serde_json::json!("notes")), "{moved}");
    assert_eq!(ids(&moved), [(1, 1), (2, 2)]);
    assert_eq!(moved["items"][1]["noteOf"], 1);

    let gone = reply(&ekko(&home, &["--json", "--context", "1"]));
    assert_eq!(gone["code"], "MOVED", "{gone}");
    assert_eq!((&gone["moved"]["project"], &gone["moved"]["as"]), (&serde_json::json!("notes"), &serde_json::json!(1)));
    let said = String::from_utf8_lossy(&ekko(&home, &["--context", "1"]).stdout).into_owned();
    assert!(said.contains("Item 1 moved to project notes at ") && said.contains(", where it is 1"), "{said}");

    let back = reply(&ekko(&notes, &["--json", "--move-to", "~", "1", "--force"]));
    assert_eq!((&back["ok"], &back["project"]), (&serde_json::json!(true), &serde_json::Value::Null), "{back}");
    assert_eq!(ids(&back), [(1, 3), (2, 4)]);

    let text = String::from_utf8_lossy(&ekko(&home, &["--move-to", "work/notes", "3"]).stdout).into_owned();
    assert!(text.contains("Moved 2 items to project notes:") && text.contains("3 as 3, 4 as 4 (a note on 3)"), "{text}");

    // A cue that is on and names no folder guards the board it is on, and
    // the reply says where it guards after the move. Only ekko's menu turns
    // one on, so the file is written as the user's answer would leave it.
    assert!(ekko(&home, &["--note", "--kind", "gotcha", "a trap"]).status.success());
    let storage = home.join(".ekko").join("storage").join("storage.json");
    let mut board: serde_json::Value = serde_json::from_slice(&fs::read(&storage).unwrap()).unwrap();
    let (id, gotcha) = board.as_object_mut().unwrap().iter_mut().find(|(_, item)| item["description"] == "a trap").unwrap();
    let id = id.clone();
    gotcha["cue"] = serde_json::json!({"command": "cargo", "words": ["fmt"], "question": "q", "at": 1});
    fs::write(&storage, serde_json::to_vec(&board).unwrap()).unwrap();
    let text = String::from_utf8_lossy(&ekko(&home, &["--move-to", "notes", &id]).stdout).into_owned();
    assert!(text.contains(&format!("{id}'s cue now guards {}, not the whole machine", notes.display())), "{text}");

    let nowhere = reply(&ekko(&home, &["--json", "--move-to", "/nowhere/at/all", "1"]));
    assert!(nowhere["error"].as_str().unwrap().contains("No such folder: /nowhere/at/all"), "{nowhere}");
    let unknown = reply(&ekko(&home, &["--json", "--move-to", "elsewhere", "1"]));
    assert!(unknown["error"].as_str().unwrap().starts_with("No such project: elsewhere"), "{unknown}");

    fs::remove_dir_all(&home).ok();
}

/// `ekko docs` writes a project's docs from its board (task 915): a page per
/// kind of note with the replaced ones at the end, the history, and a page
/// per task with its commits, its blockers and its notes, under the
/// project's own page. A board that did not move rewrites nothing, the page
/// of a task that left the board goes, the stash stays out, and a file ekko
/// docs did not write is never overwritten.
#[test]
fn docs_are_written_from_the_board_and_only_over_their_own_files() {
    let home = temp_ekko_dir();
    let app = home.join("app");
    let elsewhere = home.join("elsewhere");
    for dir in [&app, &elsewhere] {
        fs::create_dir_all(dir).unwrap();
    }
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .arg("-C")
            .arg(&app)
            .args(["-c", "user.name=ekko", "-c", "user.email=ekko@example.com", "-c", "commit.gpgsign=false", "-c", "init.defaultBranch=main"])
            .args(args)
            .env("HOME", &home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .expect("git runs");
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };
    let ekko = |cwd: &PathBuf, args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_ekko"))
            .args(args)
            .current_dir(cwd)
            .env("HOME", &home)
            .env("TZ", "UTC")
            .env_remove("EKKO_DIR")
            .env_remove("EKKO_PROJECT")
            .output()
            .expect("failed to run ekko")
    };
    let reply = |output: &process::Output| -> serde_json::Value {
        serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&output.stdout)))
    };
    let docs = app.join("docs");
    let read = |page: &str| fs::read_to_string(docs.join(page)).unwrap_or_else(|e| panic!("{page}: {e}"));

    git(&["init", "-q"]);
    assert!(ekko(&app, &["init"]).status.success());
    // 2026-09-20 12:00 UTC, a minute apart; the answer a day later.
    let at = |n: i64| 1_789_905_600_000_i64 + n * 60_000;
    let uid = |n: u32| format!("18d00000000000{n:02}-1");
    let item = |id: u32, text: &str, task: bool, extra: serde_json::Value| {
        let mut item = serde_json::json!({
            "_id": id, "_date": "Sun Sep 20 2026", "_timestamp": at(i64::from(id)), "description": text,
            "isStarred": false, "boards": ["My Board"], "_isTask": task, "uid": uid(id),
        });
        if task {
            item.as_object_mut().unwrap().extend(serde_json::json!({"isComplete": false, "inProgress": false, "priority": 1}).as_object().unwrap().clone());
        }
        item.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        (id.to_string(), item)
    };
    let board: serde_json::Map<String, serde_json::Value> = [
        item(1, "Ship the parser", true, serde_json::json!({"isComplete": true})),
        item(2, "Try a generated parser", true, serde_json::json!({"cancelled": true})),
        item(3, "Write the docs", true, serde_json::json!({"inProgress": true, "blockedBy": [uid(1)]})),
        item(4, "Hand-write the parser\nThe generated one was slower: note 6.", false, serde_json::json!({"knowledge": "decision", "supersedes": uid(5), "attachedTo": uid(1)})),
        item(5, "Generate the parser", false, serde_json::json!({"knowledge": "decision"})),
        item(6, "Measured: 3x faster than the one task 2 tried, as `<parser>` and <parser>.", false, serde_json::json!({"attachedTo": uid(1)})),
        item(7, "Which folder do the docs go to?\nOptions:\n- docs/\n- .ekko/docs", false, serde_json::json!({
            "attachedTo": uid(3), "question": {"rev": 1, "answer": {"text": "docs/", "at": at(24 * 60), "rev": 2}},
        })),
        item(8, "Run the tests before the release", false, serde_json::json!({"knowledge": "gotcha"})),
        item(9, "Write the docs\n1) ekko docs\n2) review the diff", false, serde_json::json!({"knowledge": "procedure", "attachedTo": uid(3)})),
        item(10, "A thought about the whole app", false, serde_json::json!({})),
        item(11, "a stashed thought", false, serde_json::json!({"stashed": at(30)})),
    ]
    .into_iter()
    .collect();
    fs::create_dir_all(app.join(".ekko").join("storage")).unwrap();
    fs::write(app.join(".ekko").join("storage").join("storage.json"), serde_json::Value::Object(board).to_string()).unwrap();
    fs::write(app.join(".ekko").join("memory.md"), "# app\n\nWhat the app is, kept by the user.\n").unwrap();
    git(&["commit", "-q", "--allow-empty", "-m", "feat: the parser\n\nEkko: 1"]);
    let sha = git(&["rev-parse", "--short", "HEAD"]);

    let first = ekko(&app, &["--json", "docs"]);
    assert!(first.status.success(), "{}", String::from_utf8_lossy(&first.stdout));
    let first = reply(&first);
    assert_eq!(first["folder"], docs.display().to_string());
    assert_eq!(first["decisions"], serde_json::json!({"current": 1, "replaced": 1}));
    assert_eq!(first["tasks"], serde_json::json!({"done": 1, "cancelled": 1, "open": 1}));
    assert_eq!((first["written"].as_u64(), first["loose"].as_u64()), (Some(9), Some(1)), "{first}");

    let index = read("index.md");
    assert!(index.starts_with("<!-- Written by ekko docs from the board of app"), "{index}");
    assert!(index.contains("# app\n\nWhat the app is, kept by the user.\n\n## Documentation"), "{index}");
    assert!(index.contains("- [Decisions](decisions.md): what was settled, and why. 1 in force, 1 replaced."), "{index}");
    let decisions = read("decisions.md");
    let (current, replaced) = decisions.split_once("## Replaced").expect("the replaced decision has its section");
    assert!(current.contains("## <a id=\"4\"></a>4. Hand-write the parser") && !current.contains("<a id=\"5\">"), "{decisions}");
    assert!(current.contains("The generated one was slower: note [6](tasks/1.md#6)."), "{decisions}");
    assert!(replaced.contains("### <a id=\"5\"></a>5. Generate the parser") && replaced.contains("replaced by [4](#4)"), "{decisions}");
    let shipped = read("tasks/1.md");
    assert!(shipped.contains(&format!("- `{sha}` ")) && shipped.contains("feat: the parser"), "{shipped}");
    assert!(shipped.contains("- Decision [4](../decisions.md#4): Hand-write the parser"), "{shipped}");
    assert!(shipped.contains("than the one task [2](2.md) tried, as `<parser>` and &lt;parser>."), "{shipped}");
    let open = read("tasks/3.md");
    assert!(open.contains("Blocked by [1](1.md) (done)."), "{open}");
    assert!(open.contains("**Answer**, 2026-09-21: docs/"), "{open}");
    assert!(open.contains("- Procedure [9](../procedures.md#9): Write the docs"), "{open}");
    assert!(read("history.md").contains("## Open\n\n- [3](tasks/3.md) Write the docs · in progress\n"), "{}", read("history.md"));
    assert!(read("notes.md").contains("10. A thought about the whole app"), "{}", read("notes.md"));
    for page in ["index.md", "decisions.md", "gotchas.md", "procedures.md", "history.md", "notes.md", "tasks/1.md", "tasks/2.md", "tasks/3.md"] {
        assert!(!read(page).contains("a stashed thought"), "the stash shows in {page}");
    }

    let again = reply(&ekko(&app, &["--json", "docs"]));
    assert_eq!((again["written"].as_u64(), again["unchanged"].as_u64()), (Some(0), Some(9)), "{again}");

    assert!(ekko(&app, &["--delete", "2"]).status.success());
    let after = reply(&ekko(&app, &["--json", "docs"]));
    assert_eq!(after["removed"].as_u64(), Some(1), "{after}");
    assert!(!docs.join("tasks").join("2.md").exists());

    // On the board @private, a task leaves the docs with its notes, and
    // where another item names it, only its id stays, with no link.
    assert!(ekko(&app, &["--move", "@3", "myboard", "private"]).status.success());
    let private = reply(&ekko(&app, &["--json", "docs"]));
    assert_eq!((private["private"].as_u64(), private["removed"].as_u64()), (Some(3), Some(1)), "{private}");
    assert!(!docs.join("tasks").join("3.md").exists());
    assert!(read("tasks/1.md").contains("Blocks 3 (in progress)."), "{}", read("tasks/1.md"));
    for page in ["index.md", "decisions.md", "gotchas.md", "procedures.md", "history.md", "notes.md", "tasks/1.md"] {
        assert!(!read(page).contains("Write the docs") && !read(page).contains("3.md"), "the private task shows in {page}");
    }
    assert!(read("index.md").contains("The items on the board @private, and the notes on a task there, are left out."));

    fs::write(docs.join("notes.md"), "# My own notes\n").unwrap();
    assert!(ekko(&app, &["--task", "One more"]).status.success());
    let refused = ekko(&app, &["--json", "docs"]);
    assert!(!refused.status.success());
    assert_eq!(reply(&refused)["code"], "NOT_GENERATED", "{}", reply(&refused));
    assert_eq!(fs::read_to_string(docs.join("notes.md")).unwrap(), "# My own notes\n");
    assert!(!read("history.md").contains("One more"), "a refused run wrote something");

    let nowhere = reply(&ekko(&elsewhere, &["--json", "docs"]));
    assert_eq!(nowhere["code"], "INVALID_INPUT", "{nowhere}");

    fs::remove_dir_all(&home).ok();
}

/// `ekko --repeats` reads the project's Claude Code transcripts, under
/// ~/.claude/projects in the folder named after the project's, and lists a
/// failure seen in two sessions on two days; the SessionStart prime tells it
/// once (task 1442). The default board has no sessions of its own to read.
#[test]
fn repeats_lists_a_failure_that_recurs_and_the_prime_tells_it_once() {
    use std::io::Write as _;
    let dir = temp_ekko_dir();
    let app = dir.join("app");
    fs::create_dir_all(&app).unwrap();
    let ekko = |cwd: &PathBuf, args: &[&str], stdin: &str| {
        let mut child = Command::new(env!("CARGO_BIN_EXE_ekko"))
            .args(args)
            .current_dir(cwd)
            .env("HOME", &dir)
            .env_remove("XDG_STATE_HOME")
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("EKKO_DIR")
            .env_remove("EKKO_PROJECT")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to run ekko");
        child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
        child.wait_with_output().unwrap()
    };
    assert!(ekko(&app, &["init"], "").status.success());

    let named: String = app.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let folder = dir.join(".claude").join("projects").join(named);
    fs::create_dir_all(&folder).unwrap();
    let failure = "Exit code 127\n/run/current-system/sw/bin/bash: line 1: python3: command not found";
    for (session, day) in [("s1", "2026-10-01"), ("s2", "2026-10-02")] {
        let at = format!("{day}T10:00:00.000Z");
        let id = format!("toolu_{session}");
        let row = |kind: &str, block: serde_json::Value| {
            serde_json::json!({"type": kind, "sessionId": session, "timestamp": at, "cwd": app, "entrypoint": "cli",
                               "message": {"role": kind, "content": [block]}})
        };
        let call = row("assistant", serde_json::json!({"type": "tool_use", "id": id, "name": "Bash", "input": {"command": "python3 check.py"}}));
        let failed = row("user", serde_json::json!({"type": "tool_result", "content": failure, "is_error": true, "tool_use_id": id}));
        fs::write(folder.join(format!("{session}.jsonl")), format!("{call}\n{failed}\n")).unwrap();
    }

    let start = r#"{"session_id":"s-9","hook_event_name":"SessionStart","source":"startup"}"#;
    let hook = || {
        let output = ekko(&app, &["--prime", "--hook"], start);
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8(output.stdout).unwrap()
    };
    let told = hook();
    assert!(
        told.contains("  ! failures that newly recur across sessions: Bash: Exit code 127 | /run/current-system/sw/bin/bash: line 1: python3: command not found (2 sessions, "),
        "{told}"
    );
    let again = hook();
    assert!(!again.contains("newly recur"), "the prime told the same recurrence twice:\n{again}");

    let listed = ekko(&app, &["--repeats"], "");
    let text = String::from_utf8(listed.stdout).unwrap();
    assert!(listed.status.success(), "{text}");
    assert!(text.starts_with("Failures that repeat across sessions \u{b7} project app, found from this folder \u{b7} 1 of 1 fingerprints"), "{text}");
    assert!(text.contains(" 2 sessions \u{b7} 2 days \u{b7} 2 times \u{b7} last 2026-10-02 "), "{text}");
    let json = ekko(&app, &["--json", "--repeats"], "");
    let reply: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(reply["repeats"]["recurring"][0]["sessions"], 2, "{reply}");
    assert_eq!(reply["repeats"]["recurring"][0].get("new"), None, "a recurrence the prime told is still new: {reply}");

    let default = ekko(&dir, &["--json", "--repeats"], "");
    assert!(!default.status.success());
    let refused: serde_json::Value = serde_json::from_slice(&default.stdout).unwrap();
    assert_eq!(refused["code"], "INVALID_INPUT", "{refused}");

    fs::remove_dir_all(&dir).ok();
}

/// `ekko --doctor` on a real process tree (task 1282): a stand-in for Claude
/// Code -- a script named `claude`, as /proc names a script -- runs the
/// ekko on its PATH as an MCP server. With no SessionStart record it is too
/// new to judge at first, and fails Hooks loaded once its server is 5 s old;
/// with a record it passes both checks; and once the binary on its
/// PATH is swapped it fails Old binary. Other sessions on the machine are
/// judged too, against this test's empty HOME, so only the stand-in's lines
/// are asserted, and the exit against the count of fails.
#[test]
fn the_doctor_names_a_session_running_a_swapped_binary_or_without_its_hooks() {
    use std::io::Write as _;
    let dir = temp_ekko_dir();
    let bin = dir.join("bin");
    fs::create_dir_all(&bin).unwrap();
    copy_executable(env!("CARGO_BIN_EXE_ekko"), &bin.join("ekko"));
    let claude = dir.join("claude");
    write_executable(&claude, "#!/bin/sh\nekko --mcp\nexit $?\n");
    let mut session = Command::new(&claude)
        .env("PATH", &bin)
        .env("HOME", &dir)
        .env("EKKO_DIR", dir.join(".ekko"))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let pid = session.id();
    let server = (0..100).find_map(|_| {
        let found = fs::read_dir("/proc").unwrap().flatten().find(|entry| {
            let at = entry.path();
            let parent = fs::read_to_string(at.join("stat")).ok().and_then(|stat| stat.rsplit(')').next()?.split_whitespace().nth(1)?.parse::<u32>().ok());
            parent == Some(pid) && fs::read(at.join("cmdline")).is_ok_and(|args| args == b"ekko\0--mcp\0")
        });
        if found.is_none() {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        found
    });
    assert!(server.is_some(), "the stand-in's server did not start");

    let doctor = || {
        let output = Command::new(env!("CARGO_BIN_EXE_ekko"))
            .args(["--doctor", "--json"])
            .env("HOME", &dir)
            .env_remove("XDG_STATE_HOME")
            .output()
            .unwrap();
        let reply: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let failed = reply["failed"].as_u64().unwrap();
        assert_eq!(output.status.success(), failed == 0, "the exit disagrees with the fails: {reply}");
        assert_eq!(reply["ok"], true, "{reply}");
        let mine: Vec<(String, String)> = reply["checks"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|check| check["session"]["pid"] == pid)
            .map(|check| (check["check"].as_str().unwrap().to_string(), check["verdict"].as_str().unwrap().to_string()))
            .collect();
        (mine, failed)
    };
    let verdicts = |old: &str, hooks: &str| vec![("Old binary".to_string(), old.to_string()), ("Hooks loaded".to_string(), hooks.to_string())];

    // A server just started is ahead of the hook that records it (task
    // 1555), and is judged once it is 5 s old.
    let started = std::time::Instant::now();
    assert_eq!(doctor().0, verdicts("ok", "skipped"), "no SessionStart record yet, on a server just started");
    std::thread::sleep(std::time::Duration::from_millis(5_200).saturating_sub(started.elapsed()));
    let (mine, failed) = doctor();
    assert_eq!(mine, verdicts("ok", "fail"), "no SessionStart record");
    assert!(failed >= 1);

    // The record the SessionStart hook writes for the process.
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    let start: u64 = stat.rsplit(')').next().unwrap().split_whitespace().nth(19).unwrap().parse().unwrap();
    let boot = fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap().trim().to_string();
    let processes = dir.join(".local/state/ekko/processes");
    fs::create_dir_all(&processes).unwrap();
    let boot8: String = boot.chars().filter(char::is_ascii_alphanumeric).take(8).collect();
    let record = serde_json::json!({"pid": pid, "start": start, "boot": boot, "conversation": "c-1", "since": 0});
    fs::write(processes.join(format!("{boot8}-{pid}-{start}.json")), record.to_string()).unwrap();
    assert_eq!(doctor().0, verdicts("ok", "ok"), "a clean session");

    // An upgrade puts another binary under the same name.
    copy_executable(env!("CARGO_BIN_EXE_ekko"), &bin.join("ekko.new"));
    fs::rename(bin.join("ekko.new"), bin.join("ekko")).unwrap();
    let (mine, failed) = doctor();
    assert_eq!(mine, verdicts("fail", "ok"), "the binary on its PATH was swapped");
    assert!(failed >= 1);

    session.stdin.take().unwrap().write_all(b"").unwrap();
    let ended = session.wait().unwrap();
    assert!(ended.success(), "{ended:?}");
    fs::remove_dir_all(&dir).ok();
}

/// The prime names the sessions on its board that need a restart (task
/// 1287): a stand-in for Claude Code, a script named `claude` (procedure
/// 1552), runs ekko's MCP server in project `app`, and its SessionStart
/// hook when told. With no record of its hook it is too new to judge at
/// first and named once its server is 5 s old; recorded by its hook it is
/// not; once the ekko on its PATH is swapped it is named again, as "this
/// session" in its own prime. The prime of another project never names it.
#[test]
fn the_prime_names_the_sessions_on_its_board_that_need_a_restart() {
    use std::io::Write as _;
    let dir = temp_ekko_dir();
    let bin = dir.join("bin");
    fs::create_dir_all(&bin).unwrap();
    copy_executable(env!("CARGO_BIN_EXE_ekko"), &bin.join("ekko"));
    let ekko = |cwd: &PathBuf, args: &[&str], stdin: &str| -> String {
        let mut child = Command::new(env!("CARGO_BIN_EXE_ekko"))
            .args(args)
            .current_dir(cwd)
            .env("HOME", &dir)
            .env_remove("XDG_STATE_HOME")
            .env_remove("CLAUDECODE")
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("EKKO_DIR")
            .env_remove("EKKO_PROJECT")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to run ekko");
        child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8(output.stdout).unwrap()
    };
    let (app, other) = (dir.join("app"), dir.join("other"));
    for project in [&app, &other] {
        fs::create_dir_all(project).unwrap();
        ekko(project, &["init"], "");
    }
    let start = r#"{"hook_event_name":"SessionStart","source":"startup"}"#;
    let line = |prime: &str| prime.lines().find_map(|line| line.strip_prefix("  ! sessions on this board to restart: ")).map(str::to_string);

    // Its server reads a FIFO the stand-in holds open until it ends; each
    // event file named on the stand-in's stdin runs as its SessionStart hook.
    let fifo = dir.join("mcp-in");
    assert!(Command::new("mkfifo").arg(&fifo).status().unwrap().success());
    let claude = dir.join("claude");
    let script = "#!/bin/sh\nekko --mcp < \"$1\" > /dev/null &\nexec 3> \"$1\"\nwhile read event; do\n  CLAUDECODE=1 ekko --prime --hook < \"$event\" > \"$event.out\"\n  : > \"$event.done\"\ndone\n";
    write_executable(&claude, script);
    let mut session = Command::new(&claude)
        .arg(&fifo)
        .current_dir(&app)
        .env("PATH", &bin)
        .env("HOME", &dir)
        .env_remove("XDG_STATE_HOME")
        .env_remove("CLAUDECODE")
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("EKKO_DIR")
        .env_remove("EKKO_PROJECT")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let pid = session.id();
    let mut events = session.stdin.take().unwrap();
    let mut hook = |source: &str, conversation: &str| -> String {
        let event = dir.join(format!("{conversation}.json"));
        fs::write(&event, serde_json::json!({"hook_event_name": "SessionStart", "source": source, "session_id": conversation}).to_string()).unwrap();
        writeln!(events, "{}", event.display()).unwrap();
        let done = dir.join(format!("{conversation}.json.done"));
        for _ in 0..250 {
            if done.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(done.exists(), "the stand-in's hook did not end");
        fs::read_to_string(dir.join(format!("{conversation}.json.out"))).unwrap()
    };
    let server = (0..100).any(|_| {
        let found = fs::read_dir("/proc").unwrap().flatten().any(|entry| {
            let at = entry.path();
            let parent = fs::read_to_string(at.join("stat")).ok().and_then(|stat| stat.rsplit(')').next()?.split_whitespace().nth(1)?.parse::<u32>().ok());
            parent == Some(pid) && fs::read(at.join("cmdline")).is_ok_and(|args| args == b"ekko\0--mcp\0")
        });
        if !found {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        found
    });
    assert!(server, "the stand-in's server did not start");
    let started = std::time::Instant::now();

    let first = ekko(&app, &["--prime", "--hook"], start);
    assert_eq!(line(&first), None, "a server just started is too new to judge:\n{first}");
    std::thread::sleep(std::time::Duration::from_millis(5_200).saturating_sub(started.elapsed()));
    let hookless = ekko(&app, &["--prime", "--hook"], start);
    assert_eq!(line(&hookless), Some(format!("Claude Code pid {pid} (its hooks did not run) -- ekko --doctor says why")), "{hookless}");
    assert_eq!(line(&ekko(&other, &["--prime", "--hook"], start)), None, "a session on another board");

    let own = hook("startup", "c-1");
    assert!(own.contains("project app"), "{own}");
    assert_eq!(line(&own), None, "{own}");
    assert_eq!(line(&ekko(&app, &["--prime", "--hook"], start)), None, "a session its hook recorded");

    // An upgrade puts another binary under the same name.
    copy_executable(env!("CARGO_BIN_EXE_ekko"), &bin.join("ekko.new"));
    fs::rename(bin.join("ekko.new"), bin.join("ekko")).unwrap();
    let runs = fs::canonicalize(&bin).unwrap().join("ekko");
    let replaced = format!("(it runs {}, replaced since) -- ekko --doctor says why", runs.display());
    let upgraded = ekko(&app, &["--prime", "--hook"], start);
    assert_eq!(line(&upgraded), Some(format!("default, pid {pid} \u{b7} c-1 {replaced}")), "{upgraded}");
    let cleared = hook("clear", "c-2");
    assert_eq!(line(&cleared), Some(format!("this session {replaced}")), "{cleared}");
    assert_eq!(line(&ekko(&other, &["--prime", "--hook"], start)), None, "a session on another board");

    drop(events);
    assert!(session.wait().unwrap().success());
    fs::remove_dir_all(&dir).ok();
}

/// `ekko --doctor --probe` writes the board again and waits for its
/// sessions to hear it (task 1286): two stand-ins for Claude Code, scripts
/// named `claude` (procedure 1552), run ekko's MCP server and their
/// SessionStart hook in project `app`; one runs its wake hook when the
/// board's file is replaced, as Claude Code's FileChanged does, the other
/// never; a third, deaf too, runs in another project. The probe finds one of
/// the two on its board heard it, the first passes Wake heard and the second
/// fails it, and the board holds the same items.
#[test]
fn the_probe_writes_the_board_again_and_finds_the_session_that_did_not_hear_it() {
    use std::os::unix::process::CommandExt as _;
    let dir = temp_ekko_dir();
    let bin = dir.join("bin");
    fs::create_dir_all(&bin).unwrap();
    copy_executable(env!("CARGO_BIN_EXE_ekko"), &bin.join("ekko"));
    let (app, other) = (dir.join("app"), dir.join("other"));
    let ekko_in = |cwd: &PathBuf, args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_ekko"))
            .args(args)
            .current_dir(cwd)
            .env("HOME", &dir)
            .env_remove("XDG_STATE_HOME")
            .env_remove("CLAUDECODE")
            .env_remove("EKKO_DIR")
            .env_remove("EKKO_PROJECT")
            .output()
            .unwrap()
    };
    let ekko = |args: &[&str]| ekko_in(&app, args);
    for project in [&app, &other] {
        fs::create_dir_all(project).unwrap();
        assert!(ekko_in(project, &["init"]).status.success());
    }
    assert!(ekko(&["--task", "something", "to", "hear"]).status.success());
    let board = |project: &PathBuf| project.join(".ekko").join("storage").join("storage.json");

    // Each stand-in records itself through its SessionStart hook, then
    // watches the board's file by its inode, as Claude Code's FileChanged
    // follows it, and runs its wake hook on each new one when it hears.
    let script = "#!/bin/sh\nekko --mcp < \"$1\" > /dev/null &\nexec 3> \"$1\"\nCLAUDECODE=1 ekko --prime --hook < \"$1.start\" > /dev/null\n: > \"$1.started\"\nseen=$(stat -c %i \"$2\")\nwhile :; do\n  now=$(stat -c %i \"$2\")\n  if [ \"$now\" != \"$seen\" ]; then\n    seen=$now\n    [ \"$3\" = 1 ] && echo '{\"hook_event_name\":\"FileChanged\"}' | CLAUDECODE=1 ekko --wake --hook > /dev/null 2>&1\n  fi\n  sleep 0.05\ndone\n";
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default());
    let mut stand_ins = Vec::new();
    // The third, on another project's board, is no session of the probe's.
    for (name, project, hears) in [("hearing", &app, "1"), ("deaf", &app, "0"), ("elsewhere", &other, "0")] {
        let folder = dir.join(name);
        fs::create_dir_all(&folder).unwrap();
        let claude = folder.join("claude");
        write_executable(&claude, script);
        let fifo = folder.join("mcp-in");
        assert!(Command::new("mkfifo").arg(&fifo).status().unwrap().success());
        fs::write(folder.join("mcp-in.start"), format!(r#"{{"hook_event_name":"SessionStart","source":"startup","session_id":"{name}"}}"#)).unwrap();
        let child = Command::new(&claude)
            .args([fifo.as_os_str(), board(project).as_os_str(), std::ffi::OsStr::new(hears)])
            .current_dir(project)
            .env("PATH", &path)
            .env("HOME", &dir)
            .env_remove("XDG_STATE_HOME")
            .env_remove("CLAUDECODE")
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("EKKO_DIR")
            .env_remove("EKKO_PROJECT")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .unwrap();
        stand_ins.push((child, folder.join("mcp-in.started")));
    }
    for (_, started) in &stand_ins {
        for _ in 0..250 {
            if started.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(started.exists(), "a stand-in's SessionStart hook did not end");
    }
    let items = fs::read(board(&app)).unwrap();
    let before = fs::metadata(board(&app)).unwrap();
    use std::os::unix::fs::MetadataExt as _;
    let alone = ekko(&["--probe"]);
    assert!(!alone.status.success(), "--probe ran without --doctor: {}", String::from_utf8_lossy(&alone.stdout));
    assert_eq!(fs::metadata(board(&app)).unwrap().ino(), before.ino(), "--probe without --doctor wrote the board");

    let output = ekko(&["--doctor", "--probe", "--json"]);
    let reply: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let ids: Vec<u32> = stand_ins.iter().map(|(child, _)| child.id()).collect();
    for (child, _) in &mut stand_ins {
        let _ = Command::new("kill").args(["-TERM", "--", &format!("-{}", child.id())]).status();
        let _ = child.wait();
    }
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!((reply["probe"]["sessions"].as_u64(), reply["probe"]["heard"].as_u64()), (Some(2), Some(1)), "{reply}");
    let wake = |pid: u32| -> (String, String) {
        let check = reply["checks"].as_array().unwrap().iter().find(|check| check["check"] == "Wake heard" && check["session"]["pid"] == pid).unwrap();
        (check["verdict"].as_str().unwrap().to_string(), check["says"].as_str().unwrap().to_string())
    };
    let (verdict, says) = wake(ids[0]);
    assert_eq!(verdict, "ok", "{says}");
    assert!(says.contains(" ms after"), "{says}");
    let (verdict, says) = wake(ids[1]);
    assert_eq!(verdict, "fail", "{says}");
    assert!(says.contains("and its wake hook recorded hearing nothing"), "{says}");
    assert!(!output.status.success(), "the exit says nothing failed: {reply}");
    let after = fs::metadata(board(&app)).unwrap();
    assert_ne!(after.ino(), before.ino(), "the probe wrote no new version");
    assert_eq!(fs::read(board(&app)).unwrap(), items, "the probe changed the items");
    fs::remove_dir_all(&dir).ok();
}

/// The guard records each Bash call it sees in the session it runs under
/// (task 1284): a stand-in for Claude Code, a script named `claude`, runs
/// it as a PreToolUse hook, on a Bash call and on a Read.
#[test]
fn the_guard_records_each_bash_call_it_sees_for_its_session() {
    let dir = temp_ekko_dir();
    let claude = dir.join("claude");
    // The stand-in says when the guard is done, and holds on until told to
    // end, so its folder stays whole. It sets CLAUDECODE as Claude Code does
    // for its hooks: without it the guard runs as the person, who has no
    // session to record. The test's own CLAUDECODE is taken away, so a run
    // inside a Claude Code session judges as CI does, where it is unset.
    write_executable(&claude, "#!/bin/sh\nCLAUDECODE=1 \"$EKKO\" --guard --hook < \"$1\"\n: > \"$1.done\"\nread end\nexit 0\n");
    let event = |tool: &str, id: &str| {
        let path = dir.join(format!("{id}.json"));
        let input = if tool == "Bash" { serde_json::json!({"command": "ls"}) } else { serde_json::json!({"file_path": "/x"}) };
        let event = serde_json::json!({"hook_event_name": "PreToolUse", "tool_name": tool, "tool_input": input, "tool_use_id": id, "cwd": dir});
        fs::write(&path, event.to_string()).unwrap();
        path
    };
    let guarded = |tool: &str, id: &str| -> Option<serde_json::Value> {
        let event = event(tool, id);
        let mut session = Command::new(&claude)
            .arg(&event)
            .env("EKKO", env!("CARGO_BIN_EXE_ekko"))
            .env("HOME", &dir)
            .env_remove("XDG_STATE_HOME")
            .env_remove("EKKO_DIR")
            .env_remove("CLAUDECODE")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let pid = session.id();
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
        let start = stat.rsplit(')').next().unwrap().split_whitespace().nth(19).unwrap().to_string();
        let boot: String = fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap().chars().filter(char::is_ascii_alphanumeric).take(8).collect();
        let file = dir.join(format!(".local/state/ekko/told/{boot}-{pid}-{start}/guarded"));
        let done = event.with_extension("json.done");
        for _ in 0..250 {
            if done.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(done.exists(), "the stand-in's guard did not end");
        let found = fs::read_to_string(&file).ok();
        drop(session.stdin.take());
        assert!(session.wait().unwrap().success());
        found.map(|text| serde_json::from_str(&text).unwrap())
    };

    let before = chrono_now_millis();
    let record = guarded("Bash", "toolu_bash").expect("the guard recorded the Bash call");
    assert_eq!(record["tool_use_id"], "toolu_bash", "{record}");
    assert!(record["at"].as_i64().unwrap() >= before, "{record}");
    assert_eq!(guarded("Read", "toolu_read"), None, "the guard records only the Bash calls it is asked about");
    fs::remove_dir_all(&dir).ok();
}

fn chrono_now_millis() -> i64 {
    i64::try_from(SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis()).unwrap()
}

/// A copy of a project's folder elsewhere writes its own board only: the
/// project's copy outside the folder is written from the registered folder
/// alone, and the copy's first write says so, once (task 1585).
#[test]
fn a_copy_of_a_projects_folder_leaves_the_projects_copy_alone() {
    let dir = temp_ekko_dir();
    let (site, elsewhere) = (dir.join("site"), dir.join("elsewhere"));
    fs::create_dir_all(&site).unwrap();
    let ekko = |cwd: &PathBuf, args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_ekko"))
            .args(args)
            .current_dir(cwd)
            .env("HOME", &dir)
            .env_remove("EKKO_DIR")
            .env_remove("EKKO_PROJECT")
            .env_remove("XDG_STATE_HOME")
            .env_remove("CLAUDECODE")
            .output()
            .unwrap()
    };
    assert!(ekko(&site, &["init"]).status.success());
    assert!(ekko(&site, &["--task", "from the project"]).status.success());
    let copies = dir.join(".ekko").join("copies");
    let copy = fs::read_dir(&copies).unwrap().next().unwrap().unwrap().path().join("storage").join("storage.json");
    let kept = fs::read(&copy).unwrap();
    assert!(Command::new("cp").arg("-r").arg(&site).arg(&elsewhere).status().unwrap().success());

    let output = ekko(&elsewhere, &["--task", "from a copy of the folder"]);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stdout));
    let said = String::from_utf8_lossy(&output.stderr);
    assert_eq!(said.matches("registered at").count(), 1, "{said}");
    assert_eq!(fs::read(&copy).unwrap(), kept, "a copy of the folder wrote the project's copy");
    let own = fs::read_to_string(elsewhere.join(".ekko").join("storage").join("storage.json")).unwrap();
    assert!(own.contains("from a copy of the folder"), "{own}");

    let output = ekko(&site, &["--task", "from the project again"]);
    assert!(output.status.success() && output.stderr.is_empty(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(fs::read_to_string(&copy).unwrap().contains("from the project again"));
    fs::remove_dir_all(&dir).ok();
}

/// Each file and folder under `dir`, with its inode and its bytes: what a
/// write anywhere in it changes.
fn files_under(dir: &std::path::Path) -> std::collections::BTreeMap<PathBuf, (u64, Vec<u8>)> {
    use std::os::unix::fs::MetadataExt as _;
    let mut found = std::collections::BTreeMap::new();
    let mut left = vec![dir.to_path_buf()];
    while let Some(at) = left.pop() {
        for entry in fs::read_dir(&at).unwrap() {
            let path = entry.unwrap().path();
            let meta = fs::symlink_metadata(&path).unwrap();
            let bytes = if meta.is_dir() { Vec::new() } else { fs::read(&path).unwrap() };
            if meta.is_dir() {
                left.push(path.clone());
            }
            found.insert(path, (meta.ino(), bytes));
        }
    }
    found
}

/// An ekko whose HOME is not the user's -- a test's, a script's -- writes
/// nothing on a project's board it found by walking up outside that HOME,
/// opens no menu there and writes no page beside it, and says how to name
/// a board; it still reads (task 1576). Four times a check run that way
/// wrote the real board. With EKKO_DIR naming the board, or from the HOME
/// the board lies in, the same write lands.
#[test]
fn a_scratch_home_writes_nothing_on_a_board_it_found_outside_it() {
    let dir = temp_ekko_dir();
    let (site, scratch) = (dir.join("site"), dir.join("scratch"));
    fs::create_dir_all(&site).unwrap();
    fs::create_dir_all(&scratch).unwrap();
    let ekko = |home: &PathBuf, named: Option<&PathBuf>, args: &[&str]| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ekko"));
        command
            .args(args)
            .current_dir(&site)
            .env("HOME", home)
            .env_remove("EKKO_DIR")
            .env_remove("EKKO_PROJECT")
            .env_remove("XDG_STATE_HOME")
            .env_remove("CLAUDECODE");
        if let Some(named) = named {
            command.env("EKKO_DIR", named);
        }
        let output = command.output().unwrap();
        (output.status.success(), format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr)))
    };
    assert!(ekko(&dir, None, &["init"]).0);
    assert!(ekko(&dir, None, &["--task", "on the project's board"]).0);
    let board = site.join(".ekko");
    let before = files_under(&board);

    let menu = scratch.join("questions.json");
    fs::write(&menu, "[]").unwrap();
    let writes: [&[&str]; 5] =
        [&["--task", "from a scratch HOME"], &["--check", "1"], &["--answer", "1", "yes"], &["--menu", menu.to_str().unwrap()], &["artifact", "1", "--no-open"]];
    for args in writes {
        let (succeeded, said) = ekko(&scratch, None, args);
        assert!(!succeeded, "ekko {args:?} with a scratch HOME: {said}");
        assert!(said.contains("which is not your home") && said.contains("EKKO_DIR"), "ekko {args:?}: {said}");
    }
    assert_eq!(files_under(&board), before, "a scratch HOME changed the project's board");
    let (listed, said) = ekko(&scratch, None, &["--list", "pending"]);
    assert!(listed && said.contains("on the project's board"), "{said}");

    assert!(ekko(&scratch, Some(&site), &["--task", "named with EKKO_DIR"]).0);
    assert!(ekko(&dir, None, &["--task", "from the home it lies in"]).0);
    let storage = fs::read_to_string(board.join("storage").join("storage.json")).unwrap();
    assert!(storage.contains("named with EKKO_DIR") && storage.contains("from the home it lies in"), "{storage}");
    assert!(!storage.contains("from a scratch HOME"), "{storage}");
    fs::remove_dir_all(&dir).ok();
}
