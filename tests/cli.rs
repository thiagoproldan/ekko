//! CLI-level behaviours that only show up when the real binary meets a real
//! shell: signals, pipes, exit codes. None of it is reachable from the unit
//! tests, which drive the library side and never touch a file descriptor
//! they did not create.

mod common;

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
    let mut child = common::command("sh").args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"]).arg(to).stdin(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(content.as_bytes()).unwrap();
    assert!(child.wait().unwrap().success(), "{} was not written", to.display());
}

/// Copies the executable `from` to `to` through a child process, for the
/// reason `write_executable` gives.
fn copy_executable(from: &str, to: &std::path::Path) {
    assert!(common::command("cp").arg(from).arg(to).status().unwrap().success(), "{} was not copied", to.display());
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

/// Every process an integration test starts, it starts through
/// tests/common/mod.rs (task 1666), which keeps from it what the
/// environment running the tests holds for ekko.
#[test]
fn every_test_starts_its_processes_through_common() {
    let started = concat!("Command", "::new(");
    let mut files = 0;
    for entry in fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/tests")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|extension| extension == "rs") {
            files += 1;
            let source = fs::read_to_string(&path).unwrap();
            assert!(!source.contains(started), "{} starts a process with {started}: use common::command or common::ekko (task 1666)", path.display());
        }
    }
    assert!(files >= 4, "read {files} test files");
}

/// Every variable ekko's sources read by name is one tests/common/mod.rs
/// keeps from the tests, sets for them, or lets through on purpose, and
/// each it lists is one ekko reads (task 1666): a variable read anew fails
/// here until it is placed. A read through a name only known when it runs
/// is listed here, as the file and the call.
#[test]
fn every_variable_ekko_reads_is_one_the_tests_keep_set_or_let_through() {
    let mut read = std::collections::BTreeSet::new();
    let mut unnamed = Vec::new();
    for entry in fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/src")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|extension| extension != "rs") {
            continue;
        }
        let file = path.file_name().unwrap().to_string_lossy().into_owned();
        let source = fs::read_to_string(&path).unwrap();
        // `const TASKS_VAR: &str = "EKKO_AGENT_TASK";`, and the like.
        let named = |constant: &str| {
            source.split(&format!("const {constant}: &str = \"")).nth(1).and_then(|rest| rest.split('"').next()).map(str::to_string)
        };
        let variable = |word: &str| word.starts_with(|c: char| c.is_ascii_uppercase()) && word.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
        for call in ["env::var(", "env::var_os(", "env::vars_os(", "env::home_dir(", " set("] {
            for after in source.split(call).skip(1) {
                let argument = after.split(')').next().unwrap();
                match argument.strip_prefix('"').and_then(|name| name.strip_suffix('"')) {
                    Some(name) if variable(name) => {
                        read.insert(name.to_string());
                    }
                    _ if call == "env::home_dir(" => {
                        read.insert("HOME".to_string());
                    }
                    _ if call == " set(" => read.extend(named(argument)),
                    _ => match named(argument) {
                        Some(name) => {
                            read.insert(name);
                        }
                        None => unnamed.push(format!("{file}: {call}{argument})")),
                    },
                }
            }
        }
    }
    unnamed.sort();
    let known = [
        // `set`, whose names are read above.
        "agents.rs: env::var_os(name)",
        // The environment a born session starts with, passed on whole.
        "agents.rs: env::vars_os()",
        "agents.rs: env::vars_os()",
        // The environment the test that gates a merge starts from, where the
        // session's own cannot be read (task 1942).
        "agents_merge.rs: env::vars_os()",
        // EKKO_DIR and EKKO_PROJECT, passed on to the menu.
        "menu.rs: env::var(name)",
        // A command's own $VARIABLES, which place it.
        "shell.rs: env::var(name)",
    ];
    assert_eq!(unnamed, known, "ekko reads a variable through a name only known when it runs: place what it can be in tests/common/mod.rs, then list the call here");
    let placed: std::collections::BTreeSet<String> =
        common::SESSION_VARIABLES.iter().chain(common::INHERITED).chain(&["HOME"]).map(|name| name.to_string()).collect();
    let unplaced: Vec<&String> = read.difference(&placed).collect();
    assert!(unplaced.is_empty(), "ekko reads {unplaced:?}: place each in tests/common/mod.rs, kept from the tests or let through for a reason");
    let unread: Vec<&String> = placed.difference(&read).collect();
    assert!(unread.is_empty(), "tests/common/mod.rs places {unread:?}, which ekko does not read");
}

/// What ekko keeps of sessions goes under XDG_STATE_HOME when it is set,
/// and under the home's .local/state otherwise: the unit tests, which keep
/// theirs under their own homes whatever it says, cannot show it (task
/// 1666).
#[test]
fn what_ekko_keeps_of_sessions_goes_under_xdg_state_home_or_else_the_home() {
    use std::io::Write as _;
    let home = temp_ekko_dir();
    let state = home.join("state");
    let start = |state_home: Option<&std::path::Path>| {
        let mut command = common::ekko();
        command.args(["--prime", "--hook"]).current_dir(&home).env("HOME", &home).stdin(Stdio::piped()).stdout(Stdio::null());
        if let Some(state_home) = state_home {
            command.env("XDG_STATE_HOME", state_home);
        }
        let mut child = command.spawn().unwrap();
        let input = serde_json::json!({"session_id": "s-1", "source": "startup", "hook_event_name": "SessionStart", "cwd": home});
        child.stdin.take().unwrap().write_all(input.to_string().as_bytes()).unwrap();
        assert!(child.wait().unwrap().success());
    };
    let kept = |under: &std::path::Path| under.join("ekko/sessions/s-1.json").is_file();
    start(None);
    assert!(kept(&home.join(".local/state")), "not under the home's .local/state");
    assert!(!kept(&state));
    fs::remove_dir_all(home.join(".local")).unwrap();
    start(Some(&state));
    assert!(kept(&state), "not under XDG_STATE_HOME");
    assert!(!kept(&home.join(".local/state")));
    fs::remove_dir_all(&home).ok();
}

/// The readme shows the help `ekko --help` prints, word for word (task
/// 1654): a flag added to one and not the other is a test that fails.
#[test]
fn the_readme_shows_the_help_ekko_prints() {
    let output = common::ekko().arg("--help").output().unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    let readme = include_str!("../readme.md");
    let (_, after) = readme.split_once("$ ekko --help\n").expect("the readme shows `$ ekko --help`");
    let (block, _) = after.split_once("```").expect("the block that shows it ends");
    assert_eq!(block, help, "the readme's block under `$ ekko --help` is not what ekko --help prints");
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

    let mut child = common::ekko()
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

    let output = common::ekko()
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
        let output = common::ekko()
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
        common::ekko()
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
        common::ekko()
            .args(args)
            .current_dir(cwd)
            .env("HOME", &home)
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
        common::ekko()
            .args(args)
            .current_dir(cwd)
            .env("HOME", &home)
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
        common::ekko()
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
        common::ekko()
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
        common::ekko()
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
        common::ekko()
            .args(args)
            .current_dir(&app)
            .env("HOME", &home)
            .env("EKKO_TERMINAL", "none")
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
        let out = common::ekko()
            .args(args)
            .current_dir(&app)
            .env("HOME", &home)
            .env("EKKO_TERMINAL", "none")
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
        let out = common::ekko()
            .args(args)
            .current_dir(&app)
            .env("HOME", &home)
            .env("EKKO_TERMINAL", "none")
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
        let mut child = common::ekko()
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
        let output = common::ekko().args(["--ekko-dir", dir.to_str().unwrap()]).args(args).output().expect("failed to run ekko");
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
        let mut child = common::ekko()
            .args(args)
            .current_dir(&app)
            .env("HOME", &dir)
            .env("CLAUDE_CONFIG_DIR", &config)
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
        let mut child = common::ekko()
            .args(args)
            .current_dir(&app)
            .env("HOME", &dir)
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
        let mut child = common::ekko()
            .args(args)
            .current_dir(&home)
            .env("HOME", &home)
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
        common::ekko()
            .args(args)
            .current_dir(cwd)
            .env("HOME", &home)
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
        let out = common::command("git")
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
        common::ekko()
            .args(args)
            .current_dir(cwd)
            .env("HOME", &home)
            .env("TZ", "UTC")
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
        let mut child = common::ekko()
            .args(args)
            .current_dir(cwd)
            .env("HOME", &dir)
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
    let mut session = common::command(&claude)
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
        let output = common::ekko()
            .args(["--doctor", "--json"])
            .env("HOME", &dir)
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

/// `ekko --doctor` finds a session's transcript where Claude Code moved it
/// (task 1564): a stand-in for Claude Code (procedure 1552) runs ekko's MCP
/// server, recorded with its transcript in the project's folder of the
/// config directory, as its SessionStart hook would. Its guard ran for its
/// last Bash call, and the doctor says so with the transcript in place,
/// moved to the worktree's folder as EnterWorktree moves it, and moved back
/// as ExitWorktree does; and with the record naming a folder the transcript
/// never reached, as a resume from another folder does (note 1628). Once no
/// folder holds it, the doctor says it cannot be read.
#[test]
fn the_doctor_reads_a_transcript_claude_code_moved_to_a_worktrees_folder() {
    use std::io::Write as _;
    use std::time::Duration;
    let dir = temp_ekko_dir();
    let bin = dir.join("bin");
    fs::create_dir_all(&bin).unwrap();
    copy_executable(env!("CARGO_BIN_EXE_ekko"), &bin.join("ekko"));
    let claude = dir.join("claude");
    write_executable(&claude, "#!/bin/sh\nekko --mcp\nexit $?\n");
    let mut session = common::command(&claude)
        .env("PATH", &bin)
        .env("HOME", &dir)
        .env("EKKO_DIR", dir.join(".ekko"))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let pid = session.id();
    let server = (0..100).any(|_| {
        let found = fs::read_dir("/proc").unwrap().flatten().any(|entry| {
            let at = entry.path();
            let parent = fs::read_to_string(at.join("stat")).ok().and_then(|stat| stat.rsplit(')').next()?.split_whitespace().nth(1)?.parse::<u32>().ok());
            parent == Some(pid) && fs::read(at.join("cmdline")).is_ok_and(|args| args == b"ekko\0--mcp\0")
        });
        if !found {
            std::thread::sleep(Duration::from_millis(50));
        }
        found
    });
    assert!(server, "the stand-in's server did not start");

    // Its SessionStart at 2026-01-01 00:00 UTC, its last Bash call a minute
    // later, and the guard's record of that very call.
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    let start: u64 = stat.rsplit(')').next().unwrap().split_whitespace().nth(19).unwrap().parse().unwrap();
    let boot = fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap().trim().to_string();
    let boot8: String = boot.chars().filter(char::is_ascii_alphanumeric).take(8).collect();
    let state = dir.join(".local/state/ekko");
    let record_file = state.join("processes").join(format!("{boot8}-{pid}-{start}.json"));
    fs::create_dir_all(record_file.parent().unwrap()).unwrap();
    let told = state.join("told").join(format!("{boot8}-{pid}-{start}"));
    fs::create_dir_all(&told).unwrap();
    fs::write(told.join("guarded"), r#"{"tool_use_id":"toolu_1","at":1767225660100}"#).unwrap();
    let projects = dir.join(".claude").join("projects");
    let conversation = "0e1d2c3b-4a59-4687-9a1b-2c3d4e5f6a7b";
    let at = |folder: &str| projects.join(folder).join(format!("{conversation}.jsonl"));
    let record = |folder: &str| {
        let board = dir.join(".ekko/storage/storage.json");
        let record = serde_json::json!({"pid": pid, "start": start, "boot": boot, "board": board, "conversation": conversation, "since": 0, "transcript": at(folder)});
        fs::write(&record_file, record.to_string()).unwrap();
        let started = UNIX_EPOCH + Duration::from_secs(1_767_225_600);
        fs::File::options().write(true).open(&record_file).unwrap().set_modified(started).unwrap();
    };
    let call = serde_json::json!([{"type": "tool_use", "id": "toolu_1", "name": "Bash", "input": {"command": "ls"}}]);
    let row = serde_json::json!({"type": "assistant", "timestamp": "2026-01-01T00:01:00Z", "isSidechain": false, "message": {"role": "assistant", "content": call}});
    let (project, worktree) = ("-app", "-app--claude-worktrees-task-1");
    fs::create_dir_all(projects.join(project)).unwrap();
    fs::write(at(project), format!("{row}\n")).unwrap();
    let move_to = |from: &str, to: &str| {
        fs::create_dir_all(projects.join(to)).unwrap();
        fs::rename(at(from), at(to)).unwrap();
    };
    let guard = || -> (String, String) {
        let output = common::ekko().args(["--doctor", "--json"]).env("HOME", &dir).output().unwrap();
        let reply: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let check = reply["checks"].as_array().unwrap().iter().find(|check| check["session"]["pid"] == pid && check["check"] == "Guard ran").cloned();
        let check = check.unwrap_or_else(|| panic!("no Guard ran check on the stand-in: {reply}"));
        (check["verdict"].as_str().unwrap().to_string(), check["says"].as_str().unwrap().to_string())
    };
    let ran = |what: &str| {
        let (verdict, says) = guard();
        assert_eq!(verdict, "ok", "{what}: {says}");
        assert!(says.starts_with("its guard ran for its last Bash call"), "{what}: {says}");
    };

    record(project);
    ran("the transcript where its SessionStart hook was told");
    move_to(project, worktree);
    ran("moved to the worktree's folder");
    move_to(worktree, project);
    ran("moved back");
    record("-home-user-elsewhere");
    ran("resumed from another folder, its transcript left where it was");
    fs::remove_file(at(project)).unwrap();
    let (verdict, says) = guard();
    assert_eq!(verdict, "skipped", "{says}");
    assert!(says.starts_with("its transcript cannot be read"), "{says}");

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
        let mut child = common::ekko()
            .args(args)
            .current_dir(cwd)
            .env("HOME", &dir)
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
    assert!(common::command("mkfifo").arg(&fifo).status().unwrap().success());
    let claude = dir.join("claude");
    let script = "#!/bin/sh\nekko --mcp < \"$1\" > /dev/null &\nexec 3> \"$1\"\nwhile read event; do\n  CLAUDECODE=1 ekko --prime --hook < \"$event\" > \"$event.out\"\n  : > \"$event.done\"\ndone\n";
    write_executable(&claude, script);
    let mut session = common::command(&claude)
        .arg(&fifo)
        .current_dir(&app)
        .env("PATH", &bin)
        .env("HOME", &dir)
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
        common::ekko()
            .args(args)
            .current_dir(cwd)
            .env("HOME", &dir)
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
        assert!(common::command("mkfifo").arg(&fifo).status().unwrap().success());
        fs::write(folder.join("mcp-in.start"), format!(r#"{{"hook_event_name":"SessionStart","source":"startup","session_id":"{name}"}}"#)).unwrap();
        let child = common::command(&claude)
            .args([fifo.as_os_str(), board(project).as_os_str(), std::ffi::OsStr::new(hears)])
            .current_dir(project)
            .env("PATH", &path)
            .env("HOME", &dir)
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
        let _ = common::command("kill").args(["-TERM", "--", &format!("-{}", child.id())]).status();
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
        let mut session = common::command(&claude)
            .arg(&event)
            .env("EKKO", env!("CARGO_BIN_EXE_ekko"))
            .env("HOME", &dir)
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
        common::ekko()
            .args(args)
            .current_dir(cwd)
            .env("HOME", &dir)
            .output()
            .unwrap()
    };
    assert!(ekko(&site, &["init"]).status.success());
    assert!(ekko(&site, &["--task", "from the project"]).status.success());
    let copies = dir.join(".ekko").join("copies");
    let copy = fs::read_dir(&copies).unwrap().next().unwrap().unwrap().path().join("storage").join("storage.json");
    let kept = fs::read(&copy).unwrap();
    assert!(common::command("cp").arg("-r").arg(&site).arg(&elsewhere).status().unwrap().success());

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
        let mut command = common::ekko();
        command
            .args(args)
            .current_dir(&site)
            .env("HOME", home);
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

/// A stand-in for tmux (task 1642): records each call, its arguments apart,
/// and the environment it opens a window with, and answers as tmux would
/// for the session `ekko`: whether it is there, its windows' names, a new
/// window's pane, and that pane running unless a file `dead` says it ended,
/// which kill-pane writes. The window of pane %7 is named by the file
/// `name`, which rename-window writes (task 1645). A set-option succeeds,
/// recorded as every call is, with the commands after it, unless a file
/// `refuse` says the multiplexer takes no such option (task 1675). The
/// panes of the session `ekko`, listed with their options, are the file
/// `born-panes` (task 1676). A file `wall` says the window @9 is the wall,
/// and pane %7 on it; a window made for the wall is @9, its pane %9; the
/// commands that build the wall and take it down succeed (task 1677).
const MUX: &str = r#"#!/bin/sh
state="$(dirname "$0")/mux-state"
mkdir -p "$state"
for arg in "$@"; do printf '%s\037' "$arg"; done >> "$state/calls"
printf '\n' >> "$state/calls"
case "$1" in -L|-S) shift 2 ;; esac
all="$*"
case "$1" in
  has-session) [ -e "$state/session" ] ;;
  list-windows) case "$all" in *ekko_wall*) [ -e "$state/wall" ] && echo '@9 1'; true ;; *) cat "$state/windows" ;; esac ;;
  new-session|new-window)
    touch "$state/session"
    while [ $# -gt 0 ]; do
      [ "$1" = -n ] && printf '%s\n' "$2" >> "$state/windows"
      shift
    done
    env > "$state/env"
    case "$all" in *'#{window_id} #{pane_id}'*) echo '@9 %9' ;; *) echo %7 ;; esac ;;
  list-panes) if [ "$2" = -s ]; then cat "$state/born-panes" 2>/dev/null; true; else [ -e "$state/dead" ] || echo '%7 0'; fi ;;
  kill-pane) touch "$state/dead" ;;
  display-message) case "$all" in *ekko_wall*) [ -e "$state/wall" ] && echo 1; true ;; *ztmux-zellij-mode*) [ -e "$state/zellij" ] && echo on; true ;; *) cat "$state/name" ;; esac ;;
  rename-window) eval "name=\${$#}"; printf '%s\n' "$name" > "$state/name" ;;
  set-option) if [ -e "$state/refuse" ]; then echo 'unknown flag -p' >&2; exit 1; fi ;;
  join-pane|select-layout|break-pane|select-window) ;;
  *) exit 1 ;;
esac
"#;

/// A project `app` in a git repository with one commit, its board made by
/// `ekko init` in a scratch HOME, and the stand-in multiplexer beside it,
/// which `ekko agents` runs as EKKO_MUX (task 1642).
struct Agents {
    home: PathBuf,
    app: PathBuf,
    mux: PathBuf,
}

impl Agents {
    fn new() -> Agents {
        let home = temp_ekko_dir();
        let app = home.join("app");
        fs::create_dir_all(&app).unwrap();
        let agents = Agents { mux: home.join("mux"), home, app };
        agents.git(&agents.app, &["init", "-q"]);
        agents.git(&agents.app, &["commit", "-q", "--allow-empty", "-m", "first"]);
        write_executable(&agents.mux, MUX);
        assert!(agents.ekko(&["init"]).status.success());
        agents.trust(&agents.home.join(".claude.json"));
        agents
    }

    /// Claude Code's global config at `config`, as it holds a repository
    /// whose trust dialog the user accepted (task 1755): the project's root
    /// with hasTrustDialogAccepted true.
    fn trust(&self, config: &std::path::Path) {
        let root = fs::canonicalize(&self.app).unwrap();
        let json = serde_json::json!({"projects": {root.to_str().unwrap(): {"hasTrustDialogAccepted": true}}});
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::write(config, json.to_string()).unwrap();
    }

    fn git(&self, folder: &std::path::Path, args: &[&str]) -> String {
        let output = common::command("git")
            .arg("-C")
            .arg(folder)
            .args(["-c", "user.name=ekko", "-c", "user.email=ekko@example.com", "-c", "commit.gpgsign=false", "-c", "init.defaultBranch=main"])
            .args(args)
            .env("HOME", &self.home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .expect("git runs");
        assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    /// ekko in the project's folder, as the person: no Claude Code session
    /// runs it, whichever runs these tests.
    fn command(&self, args: &[&str]) -> Command {
        self.running(env!("CARGO_BIN_EXE_ekko"), args)
    }

    /// `program` with `args`, as `command` runs ekko.
    fn running(&self, program: impl AsRef<std::ffi::OsStr>, args: &[&str]) -> Command {
        let mut command = common::command(program);
        command
            .args(args)
            .current_dir(&self.app)
            .env("HOME", &self.home)
            .env("EKKO_MUX", &self.mux)
            .env("EKKO_CLAUDE", "/stand-in/claude")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null");
        command
    }

    fn ekko(&self, args: &[&str]) -> process::Output {
        self.command(args).output().expect("failed to run ekko")
    }

    fn json(&self, args: &[&str]) -> serde_json::Value {
        let output = self.ekko(args);
        serde_json::from_slice(&output.stdout).unwrap_or_else(|e| panic!("{e}: {}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr)))
    }

    fn uid(&self, id: u32) -> String {
        let board: serde_json::Value = serde_json::from_str(&fs::read_to_string(self.home.join("app/.ekko/storage/storage.json")).unwrap()).unwrap();
        board[id.to_string()]["uid"].as_str().unwrap().to_string()
    }

    /// Every call the multiplexer was made, each its arguments.
    fn calls(&self) -> Vec<Vec<String>> {
        let calls = fs::read_to_string(self.home.join("mux-state/calls")).unwrap_or_default();
        calls.lines().map(|call| call.split('\u{1f}').filter(|arg| !arg.is_empty()).map(str::to_string).collect()).collect()
    }

    /// The calls that opened a window.
    fn opened(&self) -> Vec<Vec<String>> {
        self.calls().into_iter().filter(|call| matches!(call.first().map(String::as_str), Some("new-session" | "new-window"))).collect()
    }

    /// The command an opening call runs in its window, after tmux's own
    /// arguments, which end with the pane's format.
    fn command_of(call: &[String]) -> Vec<String> {
        let at = call.iter().position(|arg| arg == "#{pane_id}").expect("the pane's format");
        call[at + 1..].to_vec()
    }

    fn worktree(&self, branch: &str) -> PathBuf {
        fs::canonicalize(&self.app).unwrap().join(".claude").join("worktrees").join(branch)
    }

    /// ekko, as `ekko` runs it, with an identity for the commits a rebase
    /// writes: these tests' git reads no configuration of the user's.
    fn merging(&self, args: &[&str]) -> process::Output {
        let mut command = self.command(args);
        for (name, value) in [("GIT_AUTHOR_NAME", "ekko"), ("GIT_AUTHOR_EMAIL", "ekko@example.com"), ("GIT_COMMITTER_NAME", "ekko"), ("GIT_COMMITTER_EMAIL", "ekko@example.com")] {
            command.env(name, value);
        }
        command.output().expect("failed to run ekko")
    }

    /// Whether the repository has the branch `branch`.
    fn has_branch(&self, branch: &str) -> bool {
        common::command("git")
            .arg("-C")
            .arg(&self.app)
            .args(["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")])
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap()
            .status
            .success()
    }

    /// Runs the born hook on `event` in the session `session`, started for
    /// `tasks` (none: a session ekko agents did not start), in the pane %7,
    /// under a stand-in Claude Code: a process named claude that outlives
    /// its hook, as Claude Code does, until something ends it or 30 s pass.
    /// The stand-in, and what the hook printed, once it has returned.
    fn born(&self, tasks: Option<&str>, session: &str, event: &str, extra: serde_json::Value) -> (process::Child, String) {
        let mut stand_in = self.born_start(tasks, session, event, extra);
        let mut said = String::new();
        stand_in.stdout.take().unwrap().read_to_string(&mut said).unwrap();
        (stand_in, said)
    }

    /// `born`, not waiting for the hook: the stand-in, whose output closes
    /// as the hook returns.
    fn born_start(&self, tasks: Option<&str>, session: &str, event: &str, extra: serde_json::Value) -> process::Child {
        self.born_start_within(None, tasks, session, event, extra)
    }

    /// `born_start`, with the closer's grace window set to `grace_ms` (none:
    /// its own second).
    fn born_start_within(&self, grace_ms: Option<u64>, tasks: Option<&str>, session: &str, event: &str, extra: serde_json::Value) -> process::Child {
        use std::io::Write as _;
        let claude = self.home.join("claude");
        if !claude.exists() {
            // Its output closes as the hook returns, which tells the reader.
            write_executable(&claude, "#!/bin/sh\n\"$@\"\nexec sleep 30 >/dev/null 2>&1\n");
        }
        let mut command = self.running(&claude, &[env!("CARGO_BIN_EXE_ekko"), "--born", "--hook"]);
        command
            .env("EKKO_PROJECT", "app")
            .env("TMUX", format!("{},1,0", self.home.join("tmux-socket").display()))
            .env("TMUX_PANE", "%7");
        if let Some(grace_ms) = grace_ms {
            command.env("EKKO_BORN_GRACE_MS", grace_ms.to_string());
        }
        if let Some(tasks) = tasks {
            command.env("EKKO_AGENT_TASK", tasks);
        }
        let mut stand_in = command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
        let mut input = serde_json::json!({"session_id": session, "hook_event_name": event});
        input.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        writeln!(stand_in.stdin.take().unwrap(), "{input}").unwrap();
        stand_in
    }

    /// A prompt reaching the session: the hook runs, and its stand-in goes.
    fn prompt(&self, tasks: &str, session: &str) {
        let (mut stand_in, _) = self.born(Some(tasks), session, "UserPromptSubmit", serde_json::json!({"prompt": "go on"}));
        stand_in.kill().ok();
        stand_in.wait().ok();
    }

    /// The end of a turn, with no background work in flight.
    fn stop(&self, tasks: &str, session: &str) -> (process::Child, String) {
        self.born(Some(tasks), session, "Stop", serde_json::json!({"background_tasks": [], "session_crons": []}))
    }

    /// `stop`, the closer waiting 10 s where it waits 1: what a test does in
    /// the window it leaves is a few processes started, which a machine
    /// under load took over 4 s to start (task 2022).
    fn stop_with_a_wide_window(&self, tasks: &str, session: &str) -> (process::Child, String) {
        let mut stand_in = self.born_start_within(Some(10_000), Some(tasks), session, "Stop", serde_json::json!({"background_tasks": [], "session_crons": []}));
        let mut said = String::new();
        stand_in.stdout.take().unwrap().read_to_string(&mut said).unwrap();
        (stand_in, said)
    }

    fn born_log(&self) -> String {
        fs::read_to_string(self.home.join(".local/state/ekko/born/born.log")).unwrap_or_default()
    }

    /// Whether a line of born.log holds `what` within 5 s.
    fn logged(&self, what: &str) -> bool {
        self.logged_within(what, 100)
    }

    /// `logged`, polling every 50 ms up to `polls` times.
    fn logged_within(&self, what: &str, polls: u32) -> bool {
        let log = self.home.join(".local/state/ekko/born/born.log");
        (0..polls).any(|_| {
            let found = fs::read_to_string(&log).is_ok_and(|log| log.contains(what));
            if !found {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            found
        })
    }

    /// A born session that stays (task 1645): a stand-in Claude Code -- a
    /// process named claude, started for `tasks` in the pane %7 -- that runs
    /// each step `Stays::ekko` hands it, one at a time, as Claude Code runs
    /// its hooks, its tools and its MCP server, so the claims, waits and
    /// hooks of every step are this one process's.
    fn session(&self, tasks: &str) -> Stays {
        self.stays(Some(tasks))
    }

    /// A session that stays, as `session` starts one, that ekko agents did
    /// not start: the orchestrator (task 1646).
    fn orchestrator(&self) -> Stays {
        self.stays(None)
    }

    fn stays(&self, tasks: Option<&str>) -> Stays {
        let dir = self.home.join(format!("stays-{}", self.home.read_dir().unwrap().count()));
        fs::create_dir_all(&dir).unwrap();
        let claude = dir.join("claude");
        write_executable(
            &claude,
            "#!/bin/sh\ndir=\"$(dirname \"$0\")\"\nexport CLAUDECODE=1 EKKO_TEST_ADDED_BY_CLAUDE_CODE=1\nn=0\nwhile :; do\n  n=$((n + 1))\n  while [ ! -e \"$dir/$n.go\" ]; do sleep 0.02; done\n  sh \"$dir/$n.go\" < \"$dir/$n.in\" > \"$dir/$n.out\" 2> \"$dir/$n.err\"\n  touch \"$dir/$n.done\"\ndone\n",
        );
        let mut command = self.running(&claude, &[]);
        command
            .env("EKKO_PROJECT", "app")
            .env("TMUX", format!("{},1,0", self.home.join("tmux-socket").display()))
            .env("TMUX_PANE", "%7");
        if let Some(tasks) = tasks {
            command.env("EKKO_AGENT_TASK", tasks);
        }
        let child = command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        Stays { child, dir, steps: std::cell::Cell::new(0) }
    }

    /// The name the stand-in multiplexer gives the window of pane %7.
    fn window(&self) -> String {
        fs::read_to_string(self.home.join("mux-state/name")).unwrap().trim_end().to_string()
    }

    fn name_window(&self, name: &str) {
        fs::create_dir_all(self.home.join("mux-state")).unwrap();
        fs::write(self.home.join("mux-state/name"), format!("{name}\n")).unwrap();
    }

    fn context(&self, id: &str) -> String {
        String::from_utf8(self.ekko(&["--context", id]).stdout).unwrap()
    }
}

/// A born session that stays, as `Agents::session` starts it; ended as it
/// is dropped.
struct Stays {
    child: process::Child,
    dir: PathBuf,
    steps: std::cell::Cell<u32>,
}

impl Stays {
    /// Runs ekko with `args` in the session, reading `input`, as its own
    /// child: what it printed.
    fn ekko(&self, args: &[&str], input: &str) -> String {
        let step = self.steps.get() + 1;
        self.steps.set(step);
        fs::write(self.dir.join(format!("{step}.in")), input).unwrap();
        let quoted: Vec<String> = std::iter::once(env!("CARGO_BIN_EXE_ekko")).chain(args.iter().copied()).map(|arg| format!("'{}'", arg.replace('\'', "'\\''"))).collect();
        let go = self.dir.join(format!("{step}.go"));
        fs::write(go.with_extension("new"), format!("exec {}\n", quoted.join(" "))).unwrap();
        fs::rename(go.with_extension("new"), &go).unwrap();
        let done = self.dir.join(format!("{step}.done"));
        let ended = (0..200).any(|_| {
            let ended = done.exists();
            if !ended {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            ended
        });
        assert!(ended, "step {step} did not end within 10 s: {args:?}");
        fs::read_to_string(self.dir.join(format!("{step}.out"))).unwrap()
    }

    /// The hook on `event`, as Claude Code runs it in this session.
    fn hook(&self, event: &str, extra: serde_json::Value) -> String {
        let mut input = serde_json::json!({"session_id": "s-1", "hook_event_name": event});
        input.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        self.ekko(&["--born", "--hook"], &input.to_string())
    }

    /// A permission prompt left unanswered, as Claude Code 2.1.295 tells
    /// it: the request, then the notification, six seconds on, whose
    /// message names no tool. What the notification's hook printed. The
    /// request's hook denies it (task 1869); a version that showed the
    /// prompt anyway would still notify.
    fn asks(&self) -> String {
        let denied = self.hook("PermissionRequest", serde_json::json!({"tool_name": "Bash", "tool_input": {"command": "date > stamp.txt", "description": "Write the date"}}));
        assert!(denied.contains("\"behavior\":\"deny\""), "{denied}");
        self.hook("Notification", serde_json::json!({"notification_type": "permission_prompt", "message": "Claude needs your permission", "title": "Permission needed"}))
    }

    /// A tool that ran, once the user let it.
    fn ran(&self) -> String {
        self.hook("PostToolUse", serde_json::json!({"tool_name": "Bash", "tool_input": {"command": "true"}, "tool_response": {"stdout": ""}}))
    }

    /// What the last step printed on stderr: a hook's words to the session.
    fn said(&self) -> String {
        fs::read_to_string(self.dir.join(format!("{}.err", self.steps.get()))).unwrap()
    }

    /// The text of each reply ekko's MCP server, run as this session's
    /// own, gives the tool calls `calls`, each a tool's name and arguments.
    fn tools(&self, calls: &[(&str, serde_json::Value)]) -> Vec<String> {
        let initialize = serde_json::json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}}});
        let lines: Vec<String> = std::iter::once(initialize.to_string())
            .chain(calls.iter().enumerate().map(|(at, (tool, arguments))| {
                serde_json::json!({"jsonrpc": "2.0", "id": at + 1, "method": "tools/call", "params": {"name": tool, "arguments": arguments}}).to_string()
            }))
            .collect();
        let out = self.ekko(&["--mcp"], &format!("{}\n", lines.join("\n")));
        let replies: Vec<serde_json::Value> = out.lines().filter_map(|line| serde_json::from_str(line).ok()).collect();
        (1..=calls.len())
            .map(|id| {
                let reply = replies.iter().find(|reply| reply["id"] == id).unwrap_or_else(|| panic!("no reply {id}: {out}"));
                reply["result"]["content"][0]["text"].as_str().unwrap_or_else(|| panic!("{reply}")).to_string()
            })
            .collect()
    }
}

impl Drop for Stays {
    fn drop(&mut self) {
        self.child.kill().ok();
        self.child.wait().ok();
    }
}

/// Whether the stand-in session was ended by SIGTERM within 5 s.
fn ended_by_sigterm(stand_in: &mut process::Child) -> bool {
    use std::os::unix::process::ExitStatusExt as _;
    for _ in 0..100 {
        if let Some(status) = stand_in.try_wait().unwrap() {
            return status.signal() == Some(libc::SIGTERM);
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    false
}

/// Whether the stand-in session still runs 2.5 s on, past the second the
/// closer waits; then it is ended.
fn still_runs(stand_in: &mut process::Child) -> bool {
    std::thread::sleep(std::time::Duration::from_millis(2_500));
    let running = stand_in.try_wait().unwrap().is_none();
    stand_in.kill().ok();
    stand_in.wait().ok();
    running
}

/// The value env(1) is given for `name` in `command`, before its command.
fn assigned<'a>(command: &'a [String], name: &str) -> Option<&'a str> {
    let at = command.iter().position(|arg| arg == "/stand-in/claude")?;
    command[..at].iter().find_map(|arg| arg.strip_prefix(name)?.strip_prefix('='))
}

/// Whether env(1) unsets `name` in `command`.
fn unsets(command: &[String], name: &str) -> bool {
    command.windows(2).any(|pair| pair[0] == "-u" && pair[1] == name)
}

/// `ekko agents start` (task 1642): a task that may be taken up gets a
/// worktree of its own, on a branch named for it, and a window named for it
/// and its model in the session `ekko`, not made current. Its Claude Code
/// runs through env(1) without CLAUDECODE, with the model and effort given,
/// the task's uid and the project's name, and a first prompt that has it
/// claim the task before it changes anything. Two tasks share a window,
/// opened beside the first. A task with a window already is refused, and
/// nothing more is opened.
#[test]
fn agents_start_opens_a_window_for_its_tasks_in_a_worktree_of_their_own() {
    let agents = Agents::new();
    for text in ["Write hello", "Write the docs", "Write the tests"] {
        assert!(agents.ekko(&["--task", text]).status.success());
    }

    let reply = agents.json(&["agents", "start", "1", "--model", "haiku", "--effort", "low", "--json"]);
    let worktree = agents.worktree("task-1");
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(reply["window"], "1 \u{b7} haiku");
    assert_eq!(reply["branch"], "task-1");
    assert_eq!(reply["pane"], "%7");
    assert_eq!(reply["made"], true);
    assert_eq!(reply["worktree"], worktree.to_str().unwrap());
    assert_eq!(agents.git(&worktree, &["rev-parse", "--abbrev-ref", "HEAD"]), "task-1");
    let opened = agents.opened();
    assert_eq!(opened.len(), 1, "{opened:?}");
    let head = ["new-session", "-d", "-s", "ekko", "-n", "1 \u{b7} haiku", "-c", worktree.to_str().unwrap(), "-P", "-F", "#{pane_id}"];
    assert_eq!(opened[0][..head.len()], head);
    let command = Agents::command_of(&opened[0]);
    assert_eq!(command[0], "env");
    assert!(unsets(&command, "CLAUDECODE"), "{command:?}");
    assert_eq!(assigned(&command, "EKKO_AGENT_TASK"), Some(agents.uid(1).as_str()), "{command:?}");
    assert_eq!(assigned(&command, "EKKO_PROJECT"), Some("app"), "{command:?}");
    assert_eq!(assigned(&command, "EKKO_AGENT_WORKTREE"), worktree.to_str(), "the guard keeps it there: {command:?}");
    assert_eq!(assigned(&command, "EKKO_AGENT_BRANCH"), Some("task-1"), "{command:?}");
    assert_eq!(assigned(&command, "EKKO_MUX"), agents.mux.to_str(), "the hook that closes it speaks to the same one: {command:?}");
    let claude = command.iter().position(|arg| arg == "/stand-in/claude").unwrap();
    assert_eq!(command[claude + 1..command.len() - 1], ["--disallowedTools", "AskUserQuestion", "--model", "haiku", "--effort", "low"], "no question of its own reaches a user who is not there (task 1868)");
    let prompt = command.last().unwrap();
    assert!(prompt.starts_with("ekko agents started this session to do task 1 on"), "{prompt}");
    assert!(prompt.contains("Before you change anything, set task 1 in progress"), "{prompt}");
    assert!(prompt.contains("write its closing note with ekko's create, attached_to 1 -- a note of its own"), "{prompt}");
    assert!(prompt.contains("write a handoff instead, with ekko's create, kind handoff, attached_to 1"), "{prompt}");
    // Stopped short, it keeps its task and waits, whose mark wakes the
    // orchestrator (task 1647), which brings it to the user (task 1868).
    assert!(
        prompt.ends_with(
            "saying what stops you; leave the task in progress and end your turn with no text: \
             the orchestrator is told that this session waits, brings it to the user, and answers with a note on the task, which reaches you."
        ),
        "{prompt}"
    );
    // It says nothing to the user (decision 1870): it decides and records
    // routine choices, and its closing note is its report (task 1866).
    assert!(prompt.contains("No one answers this session's questions or prompts: decide routine choices yourself, and record each in a decision note with ekko's create, kind decision, attached_to 1"), "{prompt}");
    assert!(prompt.contains("That note is your report: once it is done, end with no text, and the session closes."), "{prompt}");
    assert!(prompt.contains(&format!("{}, on its branch task-1", worktree.display())), "{prompt}");
    // How it was started, on the task (task 1647).
    let board: serde_json::Value = serde_json::from_str(&fs::read_to_string(agents.home.join("app/.ekko/storage/storage.json")).unwrap()).unwrap();
    let note = &board[reply["notes"][0].as_u64().unwrap().to_string()];
    assert_eq!(note["attachedTo"].as_str(), Some(agents.uid(1).as_str()), "{note}");
    assert_eq!(
        note["description"].as_str().unwrap(),
        format!("Started by ekko agents start: a session on haiku at effort low, in window \"1 \u{b7} haiku\" of session ekko, on branch task-1 in {} (made for it).", worktree.display())
    );

    // Two tasks share one session, in a window beside the first.
    let reply = agents.json(&["agents", "start", "2", "3", "--model", "sonnet", "--json"]);
    assert_eq!(reply["window"], "2,3 \u{b7} sonnet", "{reply}");
    assert_eq!(reply["branch"], "task-2-3");
    let opened = agents.opened();
    assert_eq!(opened.len(), 2, "{opened:?}");
    assert_eq!(opened[1][..4], ["new-window", "-d", "-t", "=ekko:"]);
    let command = Agents::command_of(&opened[1]);
    assert_eq!(assigned(&command, "EKKO_AGENT_TASK"), Some(format!("{},{}", agents.uid(2), agents.uid(3)).as_str()));
    assert!(!command.iter().any(|arg| arg == "--effort"), "no effort was given: {command:?}");
    assert!(command.last().unwrap().contains("to do tasks 2 and 3 on"), "{command:?}");
    assert!(command.last().unwrap().contains("When each task is done, write its closing note with ekko's create, attached_to that task"), "{command:?}");
    assert!(command.last().unwrap().contains("leave the tasks in progress and end your turn"), "{command:?}");
    assert!(command.last().unwrap().contains("once the last of them is done, end with no text"), "{command:?}");
    // A note on each, the effort its default.
    let board: serde_json::Value = serde_json::from_str(&fs::read_to_string(agents.home.join("app/.ekko/storage/storage.json")).unwrap()).unwrap();
    let notes: Vec<(&str, &str)> = reply["notes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| {
            let note = &board[id.as_u64().unwrap().to_string()];
            (note["attachedTo"].as_str().unwrap(), note["description"].as_str().unwrap())
        })
        .collect();
    let (two, three) = (agents.uid(2), agents.uid(3));
    assert_eq!(notes.iter().map(|(on, _)| *on).collect::<Vec<_>>(), [two.as_str(), three.as_str()]);
    assert!(notes.iter().all(|(_, text)| text.starts_with("Started by ekko agents start: a session on sonnet at its default effort, in window \"2,3 \u{b7} sonnet\"")), "{notes:?}");

    // A task with a window is refused, and nothing more is opened.
    let refused = agents.json(&["agents", "start", "3", "--model", "haiku", "--json"]);
    assert_eq!(refused["code"], "INVALID_INPUT", "{refused}");
    assert!(refused["error"].as_str().unwrap().contains("task 3 has a window already, 2,3 \u{b7} sonnet"), "{refused}");
    assert_eq!(agents.opened().len(), 2);
    assert!(!agents.worktree("task-3").exists());
    fs::remove_dir_all(&agents.home).ok();
}

/// A born session lives in a multiplexer pane, not in the user's terminal
/// tab, so its window is started without the tab's address (task 1803):
/// ctx types into "the session's Konsole tab" through KONSOLE_DBUS_*. What
/// else the launcher gave it stays.
#[test]
fn agents_start_leaves_the_users_terminal_tab_out_of_the_sessions_environment() {
    let agents = Agents::new();
    assert!(agents.ekko(&["--task", "Write hello"]).status.success());
    let tab = [
        ("KONSOLE_DBUS_SERVICE", ":1.17312"),
        ("KONSOLE_DBUS_SESSION", "/Sessions/3"),
        ("KONSOLE_DBUS_WINDOW", "/Windows/1"),
        ("KITTY_WINDOW_ID", "4"),
        ("WEZTERM_PANE", "2"),
        ("WINDOWID", "77"),
    ];
    // A stand-in Claude Code that records the environment it was started with.
    let dump = agents.home.join("dump-env.sh");
    write_executable(&dump, "#!/bin/sh\nenv > \"$(dirname \"$0\")/dumped\"\n");
    let mut start = agents.command(&["agents", "start", "1", "--model", "haiku", "--json"]);
    start.env("EKKO_CLAUDE", &dump).env("KEEP_THIS", "yes");
    for (name, value) in tab {
        start.env(name, value);
    }
    let output = start.output().unwrap();
    assert!(output.status.success(), "{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    let command = Agents::command_of(&agents.opened()[0]);
    for (name, _) in tab {
        assert!(unsets(&command, name), "{name}: {command:?}");
    }
    assert!(!unsets(&command, "KEEP_THIS"), "{command:?}");

    // Run as the window runs it, from the environment ekko had: the dump
    // holds none of the tab. The control: the stand-in run without env(1)
    // holds them all, so the dump can say yes.
    let run = |argv: &[String]| -> String {
        let mut run = common::command(&argv[0]);
        run.args(&argv[1..]).env_clear().env("PATH", std::env::var_os("PATH").unwrap()).env("KEEP_THIS", "yes");
        for (name, value) in tab {
            run.env(name, value);
        }
        assert!(run.status().unwrap().success());
        fs::read_to_string(agents.home.join("dumped")).unwrap()
    };
    let bare = run(&[dump.to_str().unwrap().to_string()]);
    for (name, value) in tab {
        assert!(bare.lines().any(|line| line == format!("{name}={value}")), "the control lost {name}: {bare}");
    }
    let dumped = run(&command);
    for (name, _) in tab {
        assert!(!dumped.lines().any(|line| line.starts_with(&format!("{name}="))), "{name} reached the session: {dumped}");
    }
    assert!(dumped.lines().any(|line| line == "KEEP_THIS=yes"), "{dumped}");
    assert!(dumped.lines().any(|line| line == "EKKO_AGENT_BRANCH=task-1"), "{dumped}");
    fs::remove_dir_all(&agents.home).ok();
}

/// A worktree isolates what git tracks, not the runtime (task 1828): each
/// session gets a TMPDIR of its own in a folder a reboot does not empty,
/// the variables the project declares -- `{dir}` the session's folder --
/// and the ignored paths it names copied from the checkout. All the user's
/// to set; a session started for tasks is refused them.
#[test]
fn agents_start_gives_each_session_a_tmpdir_the_projects_variables_and_its_ignored_files() {
    let agents = Agents::new();
    let app = fs::canonicalize(&agents.app).unwrap();
    fs::write(app.join(".gitignore"), "private/\n.env.local\n").unwrap();
    agents.git(&app, &["add", ".gitignore"]);
    agents.git(&app, &["commit", "-q", "-m", "ignore"]);
    fs::create_dir_all(app.join("private/deep")).unwrap();
    fs::write(app.join("private/deep/data.txt"), "only in the checkout\n").unwrap();
    fs::write(app.join(".env.local"), "TOKEN=1\n").unwrap();
    for text in ["Write hello", "Write the docs"] {
        assert!(agents.ekko(&["--task", text]).status.success());
    }

    let nothing = String::from_utf8(agents.ekko(&["agents", "env"]).stdout).unwrap();
    assert!(nothing.starts_with("No variable is set for the sessions of this project beyond their TMPDIR"), "{nothing}");
    let nothing = String::from_utf8(agents.ekko(&["agents", "copy"]).stdout).unwrap();
    assert!(nothing.starts_with("No path is copied into the worktrees"), "{nothing}");
    let set = agents.json(&["--json", "agents", "env", "CACHE={dir}/cache", "FLAG=a=b"]);
    assert_eq!(set["env"], serde_json::json!({"CACHE": "{dir}/cache", "FLAG": "a=b"}), "{set}");
    let set = agents.json(&["--json", "agents", "copy", "private", "./.env.local", "missing"]);
    assert_eq!(set["copy"], serde_json::json!([".env.local", "missing", "private"]), "{set}");

    // What is refused is refused whole: a name of the session's own, one no
    // shell takes, a value where a name is wanted, a path outside the top.
    for (args, why) in [
        (&["agents", "env", "EKKO_AGENT_TASK=x"][..], "one ekko or Claude Code sets for a session itself"),
        (&["agents", "env", "CLAUDECODE=1"], "one ekko or Claude Code sets for a session itself"),
        (&["agents", "env", "KONSOLE_DBUS_SESSION=/Sessions/3"], "one ekko or Claude Code sets for a session itself"),
        (&["agents", "env", "1X=1"], "no name of an environment variable"),
        (&["agents", "env", "NOEQUALS"], "is no NAME=VALUE"),
        (&["agents", "env", "--remove", "FLAG=a"], "names a value"),
        (&["agents", "copy", "../outside"], "no path under the repository's top"),
        (&["agents", "copy", "/etc"], "no path under the repository's top"),
        (&["agents", "copy", ".git/config"], "no path under the repository's top"),
    ] {
        let refused = String::from_utf8(agents.ekko(&[&["--json"][..], args].concat()).stdout).unwrap();
        assert!(refused.contains(why), "{args:?}: {refused}");
    }
    assert_eq!(agents.json(&["--json", "agents", "env"])["env"].as_object().unwrap().len(), 2);
    let removed = agents.json(&["--json", "agents", "copy", "--remove", "missing"]);
    assert_eq!(removed["copy"], serde_json::json!([".env.local", "private"]), "{removed}");

    let started = agents.json(&["agents", "start", "1", "--model", "haiku", "--json"]);
    assert_eq!(started["ok"], true, "{started}");
    let worktree = agents.worktree("task-1");
    let folder = agents.home.join(".local/state/ekko/agents/app/task-1");
    let command = Agents::command_of(&agents.opened()[0]);
    assert_eq!(assigned(&command, "TMPDIR"), folder.join("tmp").to_str(), "{command:?}");
    assert!(folder.join("tmp").is_dir());
    assert_eq!(assigned(&command, "CACHE"), folder.join("cache").to_str(), "{command:?}");
    assert_eq!(assigned(&command, "FLAG"), Some("a=b"), "{command:?}");
    assert_eq!(fs::read_to_string(worktree.join("private/deep/data.txt")).unwrap(), "only in the checkout\n");
    assert_eq!(fs::read_to_string(worktree.join(".env.local")).unwrap(), "TOKEN=1\n");
    assert!(!worktree.join("missing").exists());
    assert_eq!(agents.git(&worktree, &["status", "--porcelain"]), "", "ignored, so the worktree is clean");

    // Its own folder: another session's differs. The project may set TMPDIR
    // itself, after ekko's own.
    assert!(agents.ekko(&["agents", "env", "TMPDIR={dir}/elsewhere"]).status.success());
    let started = agents.json(&["agents", "start", "2", "--model", "haiku", "--json"]);
    assert_eq!(started["ok"], true, "{started}");
    let command = Agents::command_of(&agents.opened()[1]);
    let other = agents.home.join(".local/state/ekko/agents/app/task-2");
    let at = command.iter().rposition(|arg| arg.starts_with("TMPDIR=")).unwrap();
    assert_eq!(command[at], format!("TMPDIR={}", other.join("elsewhere").display()), "{command:?}");
    assert!(other.join("tmp").is_dir() && other != folder);
    let removed = agents.json(&["--json", "agents", "env", "--remove", "TMPDIR", "CACHE", "FLAG"]);
    assert_eq!(removed["env"], serde_json::json!({}), "{removed}");
    fs::remove_dir_all(&agents.home).ok();
}

/// Another project's session of a same-numbered task, in the one session
/// `ekko` that every project's windows share, neither blocks `ekko agents
/// start` of this project's task, by its window's name or by its pane; one
/// that names this project, or none, still does (task 1731).
#[test]
fn agents_start_is_not_blocked_by_another_projects_same_numbered_session() {
    let agents = Agents::new();
    for text in ["Write hello", "Write the docs"] {
        assert!(agents.ekko(&["--task", text]).status.success());
    }
    let state = agents.home.join("mux-state");
    fs::create_dir_all(&state).unwrap();
    fs::write(state.join("session"), "").unwrap();
    // Task 1 of project `other`, in a window named for it alone; task 2 of it
    // on the wall's way, in a window whose name gives no task.
    let pane = |project: &str| {
        format!(
            "%3\t0\t@3\t1 \u{b7} haiku\t1\tOther's task\thaiku\t\tworking\t1\t\t{project}\n%4\t0\t@4\twall\t2\tOther's task\thaiku\t\tworking\t1\t\t{project}\n"
        )
    };
    let lay = |project: &str| {
        fs::write(state.join("windows"), "1 \u{b7} haiku\nwall\n").unwrap();
        fs::write(state.join("born-panes"), pane(project)).unwrap();
    };
    let refusal = |id: &str| {
        let output = agents.json(&["agents", "start", id, "--model", "haiku", "--json"]);
        output["error"].as_str().map(str::to_string)
    };

    // This project's own, or one that names none: refused, as before.
    lay("app");
    assert!(refusal("1").is_some_and(|why| why.contains("task 1 has a window already, 1 \u{b7} haiku")));
    assert!(refusal("2").is_some_and(|why| why.contains("task 2 has a session already, in pane %4")));
    lay("");
    assert!(refusal("1").is_some_and(|why| why.contains("task 1 has a window already")));
    assert!(refusal("2").is_some_and(|why| why.contains("task 2 has a session already")));
    assert!(agents.opened().is_empty());

    // Another project's: neither blocks.
    lay("other");
    assert_eq!(refusal("1"), None);
    assert_eq!(refusal("2"), None);
    assert_eq!(agents.opened().len(), 2, "{:?}", agents.opened());
    fs::remove_dir_all(&agents.home).ok();
}

/// A task done, held by a session that still runs, or in the trash is
/// refused, each saying why, with no word to the multiplexer and no
/// worktree made; a task that may be taken up, beside them, is started.
#[test]
fn agents_start_refuses_a_task_done_held_by_a_running_session_or_in_the_trash() {
    use std::io::Write as _;
    let agents = Agents::new();
    for text in ["Done", "Held", "Trashed", "Free"] {
        assert!(agents.ekko(&["--task", text]).status.success());
    }
    assert!(agents.ekko(&["--check", "1"]).status.success());
    // Held by this test's process, as the client of the server that claims it.
    let mut server = agents.command(&["--mcp"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    let lines = [
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}}}),
        serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "set_state", "arguments": {"items": [2], "state": "progress"}}}),
    ];
    let mut stdin = server.stdin.take().unwrap();
    for line in lines {
        writeln!(stdin, "{line}").unwrap();
    }
    drop(stdin);
    let mut said = String::new();
    server.stdout.take().unwrap().read_to_string(&mut said).unwrap();
    assert!(server.wait().unwrap().success());
    assert!(!said.contains("\"isError\":true"), "{said}");
    assert!(agents.ekko(&["--delete", "3"]).status.success());

    for (id, why) in [("1", "task 1 is done already"), ("2", "task 2 is in progress already, held by"), ("3", "task 3 is in the trash")] {
        let refused = agents.json(&["agents", "start", id, "--model", "haiku", "--json"]);
        assert_eq!(refused["code"], "INVALID_INPUT", "{refused}");
        let error = refused["error"].as_str().unwrap();
        assert!(error.contains(why), "{refused}");
        if id == "2" {
            assert!(error.ends_with("which still runs"), "{refused}");
        }
        assert!(!agents.worktree(&format!("task-{id}")).exists(), "a worktree for task {id}");
    }
    assert!(agents.calls().is_empty(), "a refused start spoke to the multiplexer: {:?}", agents.calls());

    let started = agents.json(&["agents", "start", "4", "--model", "haiku", "--json"]);
    assert_eq!(started["ok"], true, "{started}");
    assert_eq!(agents.opened().len(), 1);
    fs::remove_dir_all(&agents.home).ok();
}

/// A session started in a group (task 1648): its first prompt names the
/// tasks other sessions work beside its own and how to write to them, and
/// the note on its task names them. Its own tasks, and a task named twice,
/// are no more of its group. Every born session is told how a note reaches
/// it while it works; one in no group is told of none. A note, a task done
/// or cancelled, or one in the trash is refused as one of a group, saying
/// why, with nothing opened.
#[test]
fn agents_start_names_the_group_whose_sessions_talk_through_notes() {
    let agents = Agents::new();
    for text in ["The API", "The client", "The docs", "Done", "Cancelled", "Trashed"] {
        assert!(agents.ekko(&["--task", text]).status.success());
    }
    assert!(agents.ekko(&["--check", "4"]).status.success());
    assert!(agents.ekko(&["--set", "cancelled", "5"]).status.success());
    assert!(agents.ekko(&["--delete", "6"]).status.success());
    assert!(agents.ekko(&["--note", "A note"]).status.success());
    for (member, why) in [("4", "task 4 is done already"), ("5", "task 5 is cancelled already"), ("6", "task 6 is in the trash"), ("7", "7 is no task")] {
        let refused = agents.json(&["agents", "start", "1", "--model", "haiku", "--group", &format!("2,{member}"), "--json"]);
        assert_eq!(refused["code"], "INVALID_INPUT", "{refused}");
        let expected = format!("{why}: a group names the tasks other sessions work beside these; leave it out of --group");
        assert_eq!(refused["error"].as_str(), Some(expected.as_str()), "{refused}");
    }
    assert!(agents.calls().is_empty(), "a refused start spoke to the multiplexer: {:?}", agents.calls());

    let board = || -> serde_json::Value { serde_json::from_str(&fs::read_to_string(agents.home.join("app/.ekko/storage/storage.json")).unwrap()).unwrap() };
    let noted = |reply: &serde_json::Value| board()[reply["notes"][0].as_u64().unwrap().to_string()]["description"].as_str().unwrap().to_string();
    let told = "While you work, ekko tells you of each note the user or another session attaches to task 1, in a line that begins \"ekko:\", \
                at times inside a tool's result: it comes from ekko's hook, not from the tool, so take it into account.";
    let reply = agents.json(&["agents", "start", "1", "--model", "haiku", "--group", "1,3", "--group", "2,3", "--json"]);
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(reply["group"], serde_json::json!([2, 3]), "{reply}");
    let prompt = Agents::command_of(&agents.opened()[0]).last().unwrap().clone();
    let group = "Tasks 2 and 3 are worked beside yours by sessions of their own, as a group: to tell the session working one of them what it needs to know -- \
                 a change it builds on, an answer, a question -- write a note with ekko's create, attached_to that task, and ekko tells that session at once; \
                 it answers you with a note on yours.";
    assert!(prompt.contains(&format!("{told} {group} No one answers this session's questions")), "{prompt}");
    assert!(noted(&reply).ends_with("(made for it). In a group with tasks 2 and 3, each worked by a session of its own: a note on a task reaches the session holding it."), "{}", noted(&reply));

    let reply = agents.json(&["agents", "start", "2", "--model", "haiku", "--group", "1", "--json"]);
    assert_eq!(reply["group"], serde_json::json!([1]), "{reply}");
    let prompt = Agents::command_of(&agents.opened()[1]).last().unwrap().clone();
    assert!(prompt.contains("take it into account. Task 1 is worked beside yours by a session of its own, as a group: to tell"), "{prompt}");
    assert!(noted(&reply).ends_with("(made for it). In a group with task 1, worked by a session of its own: a note on either task reaches the session holding it."), "{}", noted(&reply));
    assert!(agents.ekko(&["agents", "limit", "3"]).status.success());
    let reply = agents.json(&["agents", "start", "3", "--model", "haiku", "--json"]);
    assert_eq!(reply["group"], serde_json::json!([]), "{reply}");
    let prompt = Agents::command_of(&agents.opened()[2]).last().unwrap().clone();
    assert!(prompt.contains(&told.replace("task 1", "task 3")) && !prompt.contains("as a group"), "{prompt}");
    assert!(noted(&reply).ends_with("(made for it)."), "{}", noted(&reply));
    fs::remove_dir_all(&agents.home).ok();
}

/// A session whose window is gone within the first second did not start:
/// the start says so, rather than the pane it opened.
#[test]
fn agents_start_says_when_the_session_ended_at_once() {
    let agents = Agents::new();
    assert!(agents.ekko(&["--task", "Write hello"]).status.success());
    fs::create_dir_all(agents.home.join("mux-state")).unwrap();
    fs::write(agents.home.join("mux-state/dead"), "").unwrap();
    let refused = agents.json(&["agents", "start", "1", "--model", "haiku", "--json"]);
    assert_eq!(refused["code"], "INVALID_INPUT", "{refused}");
    assert!(refused["error"].as_str().unwrap().contains("ended within 1 s: /stand-in/claude did not start there"), "{refused}");
    assert_eq!(agents.opened().len(), 1, "it was opened");
    fs::remove_dir_all(&agents.home).ok();
}

/// A start refuses a repository whose root is not trusted on the profile
/// the born sessions use (task 1755), opening nothing: its sessions would
/// stop at Claude Code's trust dialog, where no hook runs. The profile is
/// the CLAUDE_CONFIG_DIR the orchestrator started with, else the default
/// one's `~/.claude.json`; a trusted parent folder, a root with the flag
/// false and trust on another profile do not count, and the start does not
/// write the config.
#[test]
fn agents_start_refuses_a_repository_whose_root_is_not_trusted_on_the_sessions_profile() {
    let agents = Agents::new();
    assert!(agents.ekko(&["--task", "Write hello"]).status.success());
    let root = fs::canonicalize(&agents.app).unwrap();
    let home_config = agents.home.join(".claude.json");
    let profile = agents.home.join("profile");

    let refuse = |config: &std::path::Path, profile: Option<&std::path::Path>, why: &str| {
        let mut command = agents.command(&["agents", "start", "1", "--model", "haiku", "--json"]);
        if let Some(profile) = profile {
            command.env("CLAUDE_CONFIG_DIR", profile);
        }
        let output = command.output().unwrap();
        let refused: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&output.stderr)));
        assert_eq!(refused["code"], "INVALID_INPUT", "{why}: {refused}");
        let error = refused["error"].as_str().unwrap();
        assert!(
            error.contains(&format!("{} is not trusted", root.display())) && error.contains(&format!("in {}", config.display())) && error.contains("open Claude Code"),
            "{why}: {refused}"
        );
        assert!(agents.calls().is_empty(), "{why}: a refused start spoke to the multiplexer: {:?}", agents.calls());
        assert!(!agents.worktree("task-1").exists(), "{why}: a worktree was made");
    };

    // No config at all, and the root with the flag false.
    fs::remove_file(&home_config).unwrap();
    refuse(&home_config, None, "no config");
    fs::write(&home_config, serde_json::json!({"projects": {root.to_str().unwrap(): {"hasTrustDialogAccepted": false}}}).to_string()).unwrap();
    refuse(&home_config, None, "the flag false");
    // A parent folder trusted is no root trusted.
    let parent = root.parent().unwrap().to_str().unwrap().to_string();
    fs::write(&home_config, serde_json::json!({"projects": {parent: {"hasTrustDialogAccepted": true}}}).to_string()).unwrap();
    refuse(&home_config, None, "a trusted parent");
    // The default profile trusted, the session's own not.
    agents.trust(&home_config);
    refuse(&profile.join(".claude.json"), Some(&profile), "trust on another profile");
    let before = fs::read_to_string(&home_config).unwrap();

    // Trusted on the profile the session runs under: it starts, and the
    // config is as it was.
    agents.trust(&profile.join(".claude.json"));
    let started = agents
        .command(&["agents", "start", "1", "--model", "haiku", "--json"])
        .env("CLAUDE_CONFIG_DIR", &profile)
        .output()
        .unwrap();
    let started: serde_json::Value = serde_json::from_slice(&started.stdout).unwrap();
    assert_eq!(started["ok"], true, "{started}");
    assert_eq!(fs::read_to_string(&home_config).unwrap(), before);
    fs::remove_dir_all(&agents.home).ok();
}

/// A start given part of the tasks a group's last start made a branch for
/// takes that branch up (task 1851), as the start given all of them does,
/// rather than make a branch from HEAD that leaves the group's work behind;
/// a branch of exactly its own tasks comes first; a group branch HEAD has
/// already is no branch to take up; two groups are refused, naming both.
#[test]
fn agents_start_given_part_of_a_group_takes_up_the_groups_branch() {
    let agents = Agents::new();
    for text in ["One", "Two", "Three"] {
        assert!(agents.ekko(&["--task", text]).status.success());
    }
    let windows = agents.home.join("mux-state/windows");
    let started = agents.json(&["agents", "start", "1", "2", "--model", "haiku", "--json"]);
    assert_eq!(started["ok"], true, "{started}");
    let group = agents.worktree("task-1-2");
    assert!(group.is_dir());
    agents.git(&group, &["commit", "-q", "--allow-empty", "-m", "the group's work"]);

    // The group's window is gone: task 2 alone goes on on its branch.
    fs::write(&windows, "").unwrap();
    let again = agents.json(&["agents", "start", "2", "--model", "haiku", "--json"]);
    assert_eq!(again["ok"], true, "{again}");
    assert_eq!(again["branch"], "task-1-2", "{again}");
    assert_eq!(again["made"], false, "{again}");
    assert_eq!(again["worktree"].as_str().unwrap(), group.to_str().unwrap(), "{again}");
    assert!(!agents.has_branch("task-2"), "a branch from HEAD was made");
    assert!(!agents.worktree("task-2").exists());
    let command = Agents::command_of(agents.opened().last().unwrap());
    assert_eq!(assigned(&command, "EKKO_AGENT_BRANCH"), Some("task-1-2"), "{command:?}");

    // HEAD has the group's work: task 2 starts a branch of its own.
    fs::write(&windows, "").unwrap();
    agents.git(&agents.app, &["merge", "-q", "--ff-only", "task-1-2"]);
    let own = agents.json(&["agents", "start", "2", "--model", "haiku", "--json"]);
    assert_eq!(own["branch"], "task-2", "{own}");
    assert_eq!(own["made"], true, "{own}");

    // A branch named for exactly the tasks is the one taken up, a group's
    // beside it or not.
    fs::write(&windows, "").unwrap();
    for name in ["task-3-1", "task-3-2"] {
        agents.git(&agents.app, &["switch", "-q", "-c", name]);
        agents.git(&agents.app, &["commit", "-q", "--allow-empty", "-m", name]);
        agents.git(&agents.app, &["switch", "-q", "main"]);
    }
    let refused = agents.json(&["agents", "start", "3", "--model", "haiku", "--json"]);
    assert_eq!(refused["code"], "INVALID_INPUT", "{refused}");
    let error = refused["error"].as_str().unwrap();
    assert!(error.contains("task 3 was started in groups before, on branches task-3-1, task-3-2 that HEAD does not have yet"), "{refused}");
    assert!(!agents.has_branch("task-3"), "a refused start made a branch");
    agents.git(&agents.app, &["branch", "-q", "task-3"]);
    let exact = agents.json(&["agents", "start", "3", "--model", "haiku", "--json"]);
    assert_eq!(exact["branch"], "task-3", "{exact}");
    fs::remove_dir_all(&agents.home).ok();
}

/// The machine's default limit (task 1902): a project without a limit of
/// its own takes it, in place of 2, and one with its own keeps that; it is
/// set in ~/.ekko.json, from any folder, the default board's included.
#[test]
fn agents_limit_default_is_taken_by_a_project_without_its_own() {
    let agents = Agents::new();
    let shown = agents.json(&["agents", "limit", "--json"]);
    assert_eq!((shown["limit"].as_u64(), shown["from"].as_str(), shown["default"].as_u64()), (Some(2), Some("ekko"), Some(2)), "{shown}");

    let elsewhere = agents.home.join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    let set = agents.command(&["--json", "agents", "limit", "1", "--default"]).current_dir(&elsewhere).output().unwrap();
    let set: serde_json::Value = serde_json::from_slice(&set.stdout).unwrap();
    assert_eq!((set["ok"].as_bool(), set["default"].as_u64(), set["limit"].clone()), (Some(true), Some(1), serde_json::Value::Null), "{set}");
    let config: serde_json::Value = serde_json::from_str(&fs::read_to_string(agents.home.join(".ekko.json")).unwrap()).unwrap();
    assert_eq!(config["agentsLimit"], 1, "{config}");
    let shown = agents.json(&["agents", "limit", "--json"]);
    assert_eq!((shown["limit"].as_u64(), shown["set"].as_bool(), shown["from"].as_str()), (Some(1), Some(false), Some("default")), "{shown}");
    let said = String::from_utf8(agents.ekko(&["agents", "limit"]).stdout).unwrap();
    assert!(said.contains("runs at most 1 session at once for this project, the machine's default"), "{said}");

    // start keeps to it: one session runs, so a second is refused.
    for text in ["One", "Two"] {
        assert!(agents.ekko(&["--task", text]).status.success());
    }
    let state = agents.home.join("mux-state");
    fs::create_dir_all(&state).unwrap();
    fs::write(state.join("session"), "").unwrap();
    fs::write(state.join("born-panes"), "%1\t0\t@1\t1 \u{b7} haiku\t1\tTitle\thaiku\t\tworking\t1\t\tapp\n").unwrap();
    fs::write(state.join("windows"), "1 \u{b7} haiku\n").unwrap();
    let refused = agents.json(&["agents", "start", "2", "--model", "haiku", "--json"]);
    assert!(refused["error"].as_str().unwrap().contains("as many as its limit lets run at once (1,"), "{refused}");

    // The project's own limit wins over the default.
    assert!(agents.ekko(&["agents", "limit", "2"]).status.success());
    let shown = agents.json(&["agents", "limit", "--json"]);
    assert_eq!((shown["limit"].as_u64(), shown["from"].as_str(), shown["default"].as_u64()), (Some(2), Some("project"), Some(1)), "{shown}");
    let started = agents.json(&["agents", "start", "2", "--model", "haiku", "--json"]);
    assert_eq!(started["ok"], true, "{started}");
}

/// The project's brief (task 1927): what the user sets with `ekko agents
/// brief` is in the first prompt of each session `start` opens, once, between
/// how the session decides and how it ends; a project that set none, or took
/// it back, gets the prompt without it.
#[test]
fn agents_brief_is_in_the_first_prompt_only_where_the_project_set_one() {
    let agents = Agents::new();
    for text in ["One", "Two", "Three"] {
        assert!(agents.ekko(&["--task", text]).status.success());
    }
    let brief = "Before grep or whole files, use graff's MCP tools: def and callers take `symbol`, outline takes `file`, search takes `words`; e.g. outline file=src/main.rs.";
    let shown = agents.json(&["agents", "brief", "--json"]);
    assert_eq!((shown["ok"].as_bool(), shown["brief"].clone()), (Some(true), serde_json::Value::Null), "{shown}");
    assert!(String::from_utf8(agents.ekko(&["agents", "brief"]).stdout).unwrap().starts_with("No brief is set:"));
    let empty = agents.json(&["agents", "brief", "  ", "--json"]);
    assert_eq!(empty["code"], "INVALID_INPUT", "{empty}");
    assert!(!agents.home.join("app/.ekko/storage/agents.json").exists(), "an empty brief wrote");

    let windows = agents.home.join("mux-state/windows");
    let prompt_of = |agents: &Agents, n: usize| Agents::command_of(&agents.opened()[n]).last().unwrap().clone();

    // None set: the prompt as it was, with no word of a brief.
    let started = agents.json(&["agents", "start", "1", "--model", "haiku", "--json"]);
    assert_eq!(started["ok"], true, "{started}");
    let bare = prompt_of(&agents, 0);
    assert!(!bare.contains("brief") && !bare.contains("graff"), "{bare}");

    // Set: in the prompt of the next start, once, before how it ends.
    let set = agents.json(&["agents", "brief", brief, "--json"]);
    assert_eq!((set["ok"].as_bool(), set["brief"].as_str()), (Some(true), Some(brief)), "{set}");
    assert_eq!(agents.json(&["agents", "brief", "--json"])["brief"].as_str(), Some(brief));
    assert!(String::from_utf8(agents.ekko(&["agents", "brief"]).stdout).unwrap().contains(brief));
    fs::write(&windows, "").unwrap();
    let started = agents.json(&["agents", "start", "2", "--model", "haiku", "--json"]);
    assert_eq!(started["ok"], true, "{started}");
    let briefed = prompt_of(&agents, 1);
    assert_eq!(briefed.matches(brief).count(), 1, "{briefed}");
    let after = format!("for the review at merge. The project's brief for its sessions: {brief} When task 2 is done, write its closing note");
    assert!(briefed.contains(&after), "{briefed}");
    assert!(briefed.ends_with("answers with a note on the task, which reaches you."), "{briefed}");

    // Taken back: the prompt as it was.
    let cleared = agents.json(&["agents", "brief", "--none", "--json"]);
    assert_eq!((cleared["ok"].as_bool(), cleared["brief"].clone()), (Some(true), serde_json::Value::Null), "{cleared}");
    fs::write(&windows, "").unwrap();
    let started = agents.json(&["agents", "start", "3", "--model", "haiku", "--json"]);
    assert_eq!(started["ok"], true, "{started}");
    let cleared = prompt_of(&agents, 2);
    assert!(!cleared.contains("brief") && !cleared.contains("graff"), "{cleared}");
}

/// `ekko agents test --derive` (task 1902): the gate the repository
/// declares, by the fixed rule, set and said where it came from; nothing
/// set where nothing is declared, nor over what is set already.
#[test]
fn agents_test_derive_sets_what_the_repository_declares() {
    let agents = Agents::new();
    let settings = agents.home.join("app/.ekko/storage/agents.json");
    let shown = String::from_utf8(agents.ekko(&["agents", "test"]).stdout).unwrap();
    assert!(shown.ends_with("ekko derives none from what the repository declares\n"), "{shown}");
    let nothing = agents.json(&["--json", "agents", "test", "--derive"]);
    assert!(nothing["error"].as_str().unwrap().starts_with("ekko derives no test command for this project: no CI test step"), "{nothing}");
    assert!(!settings.exists());

    let workflows = agents.app.join(".github/workflows");
    fs::create_dir_all(&workflows).unwrap();
    fs::write(workflows.join("ci.yml"), "jobs:\n  t:\n    steps:\n      - name: Unit\n        run: |\n          ./build.sh\n          make test\n").unwrap();
    fs::write(agents.app.join("Cargo.toml"), "").unwrap();
    let shown = agents.json(&["--json", "agents", "test"]);
    assert_eq!(shown["derivable"], serde_json::json!({"command": "./build.sh\nmake test", "from": ".github/workflows/ci.yml, step Unit"}), "{shown}");
    assert!(!settings.exists(), "shown, not set");

    let derived = agents.json(&["--json", "agents", "test", "--derive"]);
    assert_eq!((derived["test"].as_str(), derived["derived"]["from"].as_str()), (Some("./build.sh\nmake test"), Some(".github/workflows/ci.yml, step Unit")), "{derived}");
    let said = String::from_utf8(agents.ekko(&["agents", "test", "--derive"]).stdout).unwrap();
    assert!(said.ends_with("derived from .github/workflows/ci.yml, step Unit; ekko agents test 'COMMAND' changes it\n"), "{said}");

    // Set by the user, it stays: --derive says how to take the derived one instead.
    assert!(agents.ekko(&["agents", "test", "true"]).status.success());
    let kept = agents.json(&["--json", "agents", "test", "--derive"]);
    assert!(kept["error"].as_str().unwrap().starts_with("This project's test command is set already, to `true`"), "{kept}");
    assert_eq!(agents.json(&["--json", "agents", "test"])["test"], "true");
    assert!(agents.ekko(&["agents", "test", "--none"]).status.success());
    assert!(agents.json(&["--json", "agents", "test", "--derive"])["error"].as_str().unwrap().starts_with("The user said this project has no tests"));
}

/// At most the project's limit of sessions run at once (task 1727): two
/// until the user sets another with `ekko agents limit`. A start past it is
/// refused, naming those that run, with nothing opened. A session counts
/// while its pane lives; another project's does not. A pane that names no
/// project counts, and so does a window named for tasks where no pane has
/// the options, which the multiplexer would not set.
#[test]
fn agents_start_runs_at_most_the_projects_limit_of_sessions_at_once() {
    let agents = Agents::new();
    for text in ["One", "Two", "Three", "Four"] {
        assert!(agents.ekko(&["--task", text]).status.success());
    }
    let shown = agents.json(&["agents", "limit", "--json"]);
    assert_eq!((shown["limit"].as_u64(), shown["set"].as_bool()), (Some(2), Some(false)), "{shown}");
    let said = String::from_utf8(agents.ekko(&["agents", "limit"]).stdout).unwrap();
    assert!(said.contains("runs at most 2 sessions at once for this project, until the user sets another limit"), "{said}");
    let zero = agents.ekko(&["agents", "limit", "0"]);
    assert!(!zero.status.success() && String::from_utf8_lossy(&zero.stderr).contains("0 is not in 1.."), "{zero:?}");

    let state = agents.home.join("mux-state");
    fs::create_dir_all(&state).unwrap();
    fs::write(state.join("session"), "").unwrap();
    // As born_panes lists them: pane, dead, window id, window, tasks,
    // title, model, effort, state, since, waits, project.
    let pane = |id: u32, dead: u32, project: &str| format!("%{id}\t{dead}\t@{id}\t{id} \u{b7} haiku\t{id}\tTitle\thaiku\t\tworking\t1\t\t{project}\n");
    let running = |panes: &[String], windows: &[&str]| {
        fs::write(state.join("born-panes"), panes.concat()).unwrap();
        fs::write(state.join("windows"), windows.iter().map(|name| format!("{name}\n")).collect::<String>()).unwrap();
    };
    let refused = |id: &str| -> String {
        let reply = agents.json(&["agents", "start", id, "--model", "haiku", "--json"]);
        assert_eq!(reply["code"], "INVALID_INPUT", "{reply}");
        assert!(agents.opened().is_empty(), "a refused start opened a window: {:?}", agents.opened());
        assert!(!agents.worktree(&format!("task-{id}")).exists(), "a worktree for task {id}");
        reply["error"].as_str().unwrap().to_string()
    };
    let (one, two) = ("1 \u{b7} haiku", "2 \u{b7} haiku");

    running(&[pane(1, 0, "app"), pane(2, 0, "app")], &[one, two]);
    let said = refused("3");
    assert!(
        said.starts_with(&format!(
            "2 sessions ekko agents started run for project app already, as many as its limit lets run at once (2, set with ekko agents limit): \
             task 1 in window \"{one}\"; task 2 in window \"{two}\". Start task 3 once one has gone"
        )),
        "{said}"
    );
    // A pane an ekko before task 1727 opened names no project: it counts.
    running(&[pane(1, 0, ""), pane(2, 0, "app")], &[one, two]);
    assert!(refused("3").contains("task 1 in window"));
    // A window named for a task, where no pane has the options, counts.
    running(&[pane(2, 0, "app")], &[two, "4 \u{b7} sonnet"]);
    let said = refused("3");
    assert!(said.contains(&format!("task 2 in window \"{two}\"; window \"4 \u{b7} sonnet\"")), "{said}");

    // Another project's session, and a pane whose session ended, do not.
    running(&[pane(1, 0, "other"), pane(2, 0, "app"), pane(4, 1, "app")], &[one, two, "4 \u{b7} haiku"]);
    let started = agents.json(&["agents", "start", "3", "--model", "haiku", "--json"]);
    assert_eq!(started["ok"], true, "{started}");
    let calls = agents.calls();
    let named = calls.iter().find_map(|call| call.iter().position(|arg| arg == "@ekko_project").map(|at| call[at + 1].clone()));
    assert_eq!(named.as_deref(), Some("app"), "the pane names its project: {calls:?}");

    // The user's limit.
    assert!(agents.ekko(&["agents", "limit", "3"]).status.success());
    let shown = agents.json(&["agents", "limit", "--json"]);
    assert_eq!((shown["limit"].as_u64(), shown["set"].as_bool()), (Some(3), Some(true)), "{shown}");
    let board = fs::read_to_string(agents.home.join("app/.ekko/storage/agents.json")).unwrap();
    assert_eq!(serde_json::from_str::<serde_json::Value>(&board).unwrap(), serde_json::json!({"limit": 3}));
    running(&[pane(1, 0, "app"), pane(2, 0, "app")], &[one, two]);
    let started = agents.json(&["agents", "start", "4", "--model", "haiku", "--json"]);
    assert_eq!(started["ok"], true, "{started}");
    fs::remove_dir_all(&agents.home).ok();
}

/// Under a Claude Code session, a born session starts with the environment
/// that session's process started with, read from /proc (task 1642): what
/// the session adds to the command running ekko -- here CLAUDECODE,
/// CLAUDE_CODE_SESSION_ID and AI_AGENT, as Claude Code adds them -- is not
/// in the environment the multiplexer runs with, and the window's command
/// unsets each; what the session started with stays, its Claude Code
/// profile set in the window.
#[test]
fn a_born_session_starts_with_the_environment_its_orchestrator_started_with() {
    let agents = Agents::new();
    assert!(agents.ekko(&["--task", "Write hello"]).status.success());
    // The stand-in session: a process named claude, which adds to the
    // command it runs what Claude Code adds, and stays while it runs.
    let claude = agents.home.join("claude");
    write_executable(&claude, "#!/bin/sh\nexport CLAUDECODE=1 CLAUDE_CODE_SESSION_ID=s-1 AI_AGENT=claude-code\n\"$@\"\nexit $?\n");
    let profile = agents.home.join("profile");
    agents.trust(&profile.join(".claude.json"));
    let output = agents
        .running(&claude, &[env!("CARGO_BIN_EXE_ekko"), "agents", "start", "1", "--model", "haiku", "--json"])
        .env("KEPT_FROM_THE_START", "yes")
        .env("CLAUDE_CONFIG_DIR", &profile)
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env_remove("AI_AGENT")
        .output()
        .unwrap();
    let reply: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&output.stderr)));
    assert_eq!(reply["ok"], true, "{reply}");

    let environment = fs::read_to_string(agents.home.join("mux-state/env")).unwrap();
    let has = |name: &str| environment.lines().find_map(|line| line.strip_prefix(name)?.strip_prefix('=')).map(str::to_string);
    assert_eq!(has("KEPT_FROM_THE_START").as_deref(), Some("yes"), "{environment}");
    assert_eq!(has("CLAUDE_CONFIG_DIR").as_deref(), profile.to_str(), "{environment}");
    let command = Agents::command_of(&agents.opened()[0]);
    for added in ["CLAUDECODE", "CLAUDE_CODE_SESSION_ID", "AI_AGENT"] {
        assert_eq!(has(added), None, "{added} reached the multiplexer: {environment}");
        assert!(unsets(&command, added), "{added} is not unset: {command:?}");
    }
    assert!(!unsets(&command, "KEPT_FROM_THE_START"), "{command:?}");
    assert_eq!(assigned(&command, "CLAUDE_CONFIG_DIR"), profile.to_str(), "{command:?}");
    fs::remove_dir_all(&agents.home).ok();
}

/// A born session's hook (task 1643, note 1638's cases A and B): at the end
/// of a turn whose task is not done, the session stays; once it is done and
/// no prompt came after, the session's Claude Code is ended by SIGTERM a
/// second later, and its pane, still listed, is closed and gone from the
/// list. Background work in flight keeps it, and a session ekko agents did
/// not start is left alone, whatever its board says.
#[test]
fn a_born_session_closes_once_its_task_is_done_and_not_before() {
    let agents = Agents::new();
    assert!(agents.ekko(&["--task", "Write hello"]).status.success());
    let uid = agents.uid(1);
    agents.prompt(&uid, "s-1");

    let (mut stand_in, said) = agents.stop(&uid, "s-1");
    assert!(said.is_empty(), "{said}");
    assert!(agents.logged(&format!("{uid} stays: task 1 is pending")));
    assert!(still_runs(&mut stand_in), "closed with its task pending");

    assert!(agents.ekko(&["--check", "1"]).status.success());
    let busy = serde_json::json!({"background_tasks": [{"id": "b-1", "type": "shell", "status": "running"}], "session_crons": []});
    let (mut stand_in, _) = agents.born(Some(&uid), "s-1", "Stop", busy);
    assert!(agents.logged("stays: background work is in flight"));
    assert!(still_runs(&mut stand_in), "closed with background work in flight");
    let (mut stand_in, _) = agents.born(None, "s-1", "Stop", serde_json::json!({"background_tasks": [], "session_crons": []}));
    assert!(still_runs(&mut stand_in), "closed a session ekko agents did not start");

    let (mut stand_in, said) = agents.stop(&uid, "s-1");
    assert!(said.contains("systemMessage") && said.contains("closes now"), "{said}");
    // Read before the stand-in is reaped: a process ended and not reaped yet
    // is a zombie, which the closer must take for ended.
    assert!(agents.logged("closed: its Claude Code ended, and its pane %7 is gone"), "{:?}", fs::read_to_string(agents.home.join(".local/state/ekko/born/born.log")));
    assert!(ended_by_sigterm(&mut stand_in), "not ended by SIGTERM once its task was done");
    let socket = agents.home.join("tmux-socket");
    // Its state set as it changed is another test's (task 1675).
    let closing: Vec<Vec<String>> = agents.calls().into_iter().filter(|call| call[..2] == ["-S", socket.to_str().unwrap()] && call[2] != "set-option").collect();
    assert_eq!(closing.iter().map(|call| call[2].as_str()).collect::<Vec<_>>(), ["list-panes", "kill-pane", "list-panes", "list-panes"], "{closing:?}");
    assert_eq!(closing[1][3..], ["-t", "%7"]);
    fs::remove_dir_all(&agents.home).ok();
}

/// A session born for two tasks (task 1648) closes only once both are
/// finished: at the end of a turn with one done and the other pending it
/// stays, saying which; with the other cancelled, it closes.
#[test]
fn a_born_session_for_two_tasks_closes_only_once_both_are_finished() {
    let agents = Agents::new();
    for text in ["Write hello", "Write the docs"] {
        assert!(agents.ekko(&["--task", text]).status.success());
    }
    let tasks = format!("{},{}", agents.uid(1), agents.uid(2));
    agents.prompt(&tasks, "s-1");
    assert!(agents.ekko(&["--check", "1"]).status.success());
    let (mut stand_in, said) = agents.stop(&tasks, "s-1");
    assert!(said.is_empty(), "{said}");
    assert!(agents.logged(&format!("{tasks} stays: task 2 is pending")));
    assert!(still_runs(&mut stand_in), "closed with one of its tasks pending");

    assert!(agents.ekko(&["--set", "cancelled", "2"]).status.success());
    let (mut stand_in, said) = agents.stop(&tasks, "s-1");
    assert!(said.contains("closes now"), "{said}");
    assert!(ended_by_sigterm(&mut stand_in), "not ended once both were finished");
    fs::remove_dir_all(&agents.home).ok();
}

/// A born session ends silently (task 1866, decision 1870): the write that
/// finishes the last of its tasks, each with a closing note it wrote, stops
/// its processing there -- PostToolUse answers `continue: false`, so no
/// text follows in its pane -- and its closer ends it, Stop or no Stop. A
/// task done with no closing note yet, or with only the user's note, a
/// decision or a handoff of the session's own, does not stop it: the note
/// would be lost. Nor does a tool that is no write of ekko's.
#[test]
fn a_born_session_stops_at_the_write_that_finishes_its_last_task_with_its_closing_note() {
    let agents = Agents::new();
    for text in ["Write hello", "Write the docs", "Write the tests"] {
        assert!(agents.ekko(&["--task", text]).status.success());
    }
    let mut session = agents.session(&format!("{},{},{}", agents.uid(1), agents.uid(2), agents.uid(3)));
    session.ekko(&["--begin", "1", "2", "3"], "");
    let wrote = |session: &Stays, tool: &str| session.hook("PostToolUse", serde_json::json!({"tool_name": tool, "tool_input": {}, "tool_response": {}}));
    let note = |text: &str, kind: &str| serde_json::json!({"text": text, "kind": kind, "attached_to": 1});

    // Task 1 done, with its closing note; the others still in progress.
    session.tools(&[("create", note("Closing note for 1", "note")), ("set_state", serde_json::json!({"items": [1], "state": "done"}))]);
    assert_eq!(wrote(&session, "mcp__plugin_ekko_ekko__set_state"), "", "stopped with tasks 2 and 3 in progress");
    // Task 3's closing note, before it is done; task 2 done with none of the
    // session's: the user's note, a decision and a handoff of its own are none.
    session.tools(&[("create", serde_json::json!({"text": "Closing note for 3", "attached_to": 3}))]);
    assert!(agents.ekko(&["--note", "The user's word on 2"]).status.success());
    assert!(agents.ekko(&["--attached-to", "@6", "2"]).status.success());
    session.tools(&[
        ("create", serde_json::json!({"text": "Chose the short name", "kind": "decision", "attached_to": 2})),
        ("create", serde_json::json!({"text": "Where 2 stopped", "kind": "handoff", "attached_to": 2})),
        ("set_state", serde_json::json!({"items": [2, 3], "state": "done"})),
    ]);
    for tool in ["mcp__plugin_ekko_ekko__set_state", "mcp__ekko__set_state"] {
        assert_eq!(wrote(&session, tool), "", "stopped with task 2 done before its closing note");
    }
    assert!(still_alive(&mut session.child), "closed with task 2 done before its closing note");

    // Each with its closing note, task 3 back in progress: the note does not
    // stop it, nor does a read.
    session.tools(&[("set_state", serde_json::json!({"items": [3], "state": "progress"})), ("create", serde_json::json!({"text": "Closing note for 2", "attached_to": 2}))]);
    assert_eq!(wrote(&session, "mcp__plugin_ekko_ekko__create"), "", "stopped with task 3 in progress");
    assert_eq!(wrote(&session, "mcp__plugin_ekko_ekko__context"), "", "a read stopped it");
    // Task 3 done: that write stops it.
    session.tools(&[("set_state", serde_json::json!({"items": [3], "state": "done"}))]);
    let said = wrote(&session, "mcp__plugin_ekko_ekko__set_state");
    let stop: serde_json::Value = serde_json::from_str(&said).unwrap_or_else(|e| panic!("{e}: {said}"));
    assert_eq!(stop["continue"], false, "{said}");
    assert!(stop["stopReason"].as_str().is_some_and(|reason| reason.contains("each with its closing note")), "{said}");
    assert!(agents.logged("its last task is finished, each with its closing note: stopped after the write that finished it"));
    assert!(ended_by_sigterm(&mut session.child), "its closer did not end it after the stop");
    fs::remove_dir_all(&agents.home).ok();
}

/// Whether the stand-in session still runs 2.5 s on, past the second the
/// closer waits; it is left running.
fn still_alive(stand_in: &mut process::Child) -> bool {
    std::thread::sleep(std::time::Duration::from_millis(2_500));
    stand_in.try_wait().unwrap().is_none()
}

/// Note 1638's case C: the user finishes the task, then talks to the
/// session; a prompt that reaches it once its task is done makes it the
/// user's, and it stays at the end of that turn and every one after.
#[test]
fn a_born_session_the_user_talks_to_once_its_task_is_done_stays() {
    let agents = Agents::new();
    assert!(agents.ekko(&["--task", "Write hello"]).status.success());
    let uid = agents.uid(1);
    agents.prompt(&uid, "s-1");
    assert!(agents.ekko(&["--check", "1"]).status.success());
    agents.prompt(&uid, "s-1");
    assert!(agents.logged("a prompt reached it after its tasks were finished: it stays, the user's now"));
    for _ in 0..2 {
        let (mut stand_in, said) = agents.stop(&uid, "s-1");
        assert!(said.is_empty(), "{said}");
        assert!(still_runs(&mut stand_in), "closed after the user talked to it");
    }
    assert!(agents.logged("stays: a prompt reached it after its tasks were finished"));
    fs::remove_dir_all(&agents.home).ok();
}

/// The end of background work, which Claude Code tells the session with a
/// prompt of its own (task 1688), is no one talking to it: a session that
/// set its task done while a shell ran in the background closes once the
/// shell's notice has been answered, where the user's prompt keeps it.
#[test]
fn a_background_shell_s_notice_after_the_task_is_done_does_not_keep_a_born_session() {
    let agents = Agents::new();
    for text in ["Write hello", "Write the docs"] {
        assert!(agents.ekko(&["--task", text]).status.success());
    }
    let notice = serde_json::json!({"prompt": "<task-notification>\n<task-id>b9tgxbuqo</task-id>\n<status>completed</status>\n<summary>Background command \"Sleep 20 seconds\" completed (exit code 0)</summary>\n</task-notification>"});
    let shell = serde_json::json!({"background_tasks": [{"id": "b9tgxbuqo", "type": "shell", "status": "running", "command": "sleep 20"}], "session_crons": []});
    for (id, session) in [(1, "s-1"), (2, "s-2")] {
        let uid = agents.uid(id);
        agents.prompt(&uid, session);
        assert!(agents.ekko(&["--check", &id.to_string()]).status.success());
        let (mut stand_in, _) = agents.born(Some(&uid), session, "Stop", shell.clone());
        stand_in.kill().ok();
        stand_in.wait().ok();
        if id == 1 {
            let (mut stand_in, _) = agents.born(Some(&uid), session, "UserPromptSubmit", notice.clone());
            stand_in.kill().ok();
            stand_in.wait().ok();
        } else {
            agents.prompt(&uid, session);
        }
        let (mut stand_in, _) = agents.stop(&uid, session);
        if id == 1 {
            assert!(ended_by_sigterm(&mut stand_in), "the shell's notice kept it");
        } else {
            assert!(still_runs(&mut stand_in), "the user's prompt did not keep it");
        }
    }
    let log = fs::read_to_string(agents.home.join(".local/state/ekko/born/born.log")).unwrap();
    assert_eq!(log.matches("the user's now").count(), 1, "{log}");
    fs::remove_dir_all(&agents.home).ok();
}

/// A prompt typed while the task is under way, which Claude Code fires as
/// it is typed, does not keep the session once the task is done (design
/// item 2: only a prompt after that does); a prompt that reaches it in the
/// second before it closes does, and so does its task reopened then.
#[test]
fn a_prompt_mid_task_does_not_keep_a_born_session_but_one_as_it_closes_does() {
    let agents = Agents::new();
    for text in ["Write hello", "Write the docs", "Write the tests"] {
        assert!(agents.ekko(&["--task", text]).status.success());
    }
    let (one, two) = (agents.uid(1), agents.uid(2));
    agents.prompt(&one, "s-1");
    assert!(agents.ekko(&["--begin", "1"]).status.success());
    agents.prompt(&one, "s-1");
    assert!(agents.ekko(&["--check", "1"]).status.success());
    let (mut stand_in, _) = agents.stop(&one, "s-1");
    assert!(ended_by_sigterm(&mut stand_in), "a prompt mid-task kept it");

    agents.prompt(&two, "s-2");
    assert!(agents.ekko(&["--check", "2"]).status.success());
    let (mut stand_in, said) = agents.stop_with_a_wide_window(&two, "s-2");
    assert!(said.contains("closes now"), "{said}");
    agents.prompt(&two, "s-2");
    assert!(agents.logged_within(&format!("{two} stays: a prompt reached it as it was closing"), 300), "{}", agents.born_log());
    assert!(still_runs(&mut stand_in), "closed after a prompt reached it in its last second");

    let three = agents.uid(3);
    agents.prompt(&three, "s-3");
    assert!(agents.ekko(&["--check", "3"]).status.success());
    let (mut stand_in, said) = agents.stop_with_a_wide_window(&three, "s-3");
    assert!(said.contains("closes now"), "{said}");
    assert!(agents.ekko(&["--check", "3"]).status.success(), "reopened");
    assert!(agents.logged_within(&format!("{three} stays: task 3 is pending"), 300), "{}", agents.born_log());
    assert!(still_runs(&mut stand_in), "closed with its task reopened in its last second");
    fs::remove_dir_all(&agents.home).ok();
}

/// A born session waiting on the user (task 1645, design item 4): a
/// permission prompt left unanswered marks the task it holds with what it
/// waits for -- which the task's context, the board and `--sessions` say --
/// and ends its window's name with "waiting on you"; the tool that ran once
/// the user let it clears both. Each event after that finds nothing to
/// clear, and says nothing to the multiplexer or the board.
#[test]
fn a_born_session_waiting_on_the_user_says_so_on_its_task_and_window_until_it_moves_on() {
    let agents = Agents::new();
    assert!(agents.ekko(&["--task", "Write hello"]).status.success());
    agents.name_window("1 \u{b7} haiku");
    let session = agents.session(&agents.uid(1));
    session.ekko(&["--begin", "1"], "");

    assert_eq!(session.asks(), "", "a Notification hook prints nothing");
    let context = agents.context("1");
    assert!(context.contains(", waiting on you since ") && context.contains(": Claude needs your permission -- Bash: date > stamp.txt \u{b7} priority 1"), "{context}");
    let read = agents.json(&["--context", "1", "--json"]);
    let held = &read["context"]["item"]["held"];
    assert_eq!(held["waitsOnUser"]["what"], "Claude needs your permission -- Bash: date > stamp.txt", "{read}");
    let board = String::from_utf8(agents.ekko(&[]).stdout).unwrap();
    assert!(board.contains(", waiting on you"), "{board}");
    let sessions = String::from_utf8(agents.ekko(&["--sessions"]).stdout).unwrap();
    assert!(sessions.contains("1. Write hello \u{b7} waiting on you since "), "{sessions}");
    assert_eq!(agents.window(), "1 \u{b7} haiku \u{b7} waiting on you");
    assert!(agents.logged("waits on you (permission_prompt): Claude needs your permission -- Bash: date > stamp.txt; marked on task 1; its window named \"1 \u{b7} haiku \u{b7} waiting on you\""));
    let socket = agents.home.join("tmux-socket");
    // The pane's state, set alongside, is another test's (task 1675).
    let named = |calls: Vec<Vec<String>>| -> Vec<Vec<String>> {
        calls.into_iter().filter(|call| call[..2] == ["-S", socket.to_str().unwrap()] && call[2] != "set-option").map(|call| call[2..].to_vec()).collect()
    };
    // Whether the pane is on the wall first (task 1677).
    assert_eq!(
        named(agents.calls()),
        [
            vec!["display-message", "-p", "-t", "%7", "#{@ekko_wall}"],
            vec!["display-message", "-p", "-t", "%7", "#{window_name}"],
            vec!["rename-window", "-t", "%7", "1 \u{b7} haiku \u{b7} waiting on you"]
        ]
    );

    assert_eq!(session.ran(), "", "a PostToolUse hook prints nothing");
    let context = agents.context("1");
    assert!(context.contains("task, in progress, held by ") && !context.contains("waiting on you"), "{context}");
    let read = agents.json(&["--context", "1", "--json"]);
    assert!(read["context"]["item"]["held"].is_object() && read["context"]["item"]["held"].get("waitsOnUser").is_none(), "{read}");
    assert_eq!(agents.window(), "1 \u{b7} haiku");
    assert!(agents.logged("waits on you no more (PostToolUse): cleared on task 1; its window named \"1 \u{b7} haiku\" again"));

    let (calls, board) = (named(agents.calls()).len(), fs::read_to_string(agents.home.join("app/.ekko/storage/storage.json")).unwrap());
    for event in ["PostToolUse", "PostToolUseFailure", "ElicitationResult", "UserPromptSubmit", "Stop"] {
        session.hook(event, serde_json::json!({"background_tasks": [], "session_crons": []}));
    }
    assert_eq!(named(agents.calls()).len(), calls, "the multiplexer was spoken to with nothing to clear");
    assert_eq!(fs::read_to_string(agents.home.join("app/.ekko/storage/storage.json")).unwrap(), board, "the board was written with nothing to clear");

    // A permission granted within the six seconds leaves nothing behind: a
    // notification with no request before it -- a sandboxed command's
    // network request has none -- says what Claude Code said, and no more.
    session.hook("PermissionRequest", serde_json::json!({"tool_name": "Bash", "tool_input": {"command": "rm -r target"}}));
    session.ran();
    session.hook("Notification", serde_json::json!({"notification_type": "permission_prompt", "message": "Claude needs your permission"}));
    assert!(agents.logged("waits on you (permission_prompt): Claude needs your permission; marked on task 1;"), "{:?}", fs::read_to_string(agents.home.join(".local/state/ekko/born/born.log")));
    fs::remove_dir_all(&agents.home).ok();
}

/// A born session's permission prompt is denied at once (task 1869, decision
/// 1909): no one answers it. Each kind of prompt Claude Code's permission
/// modes page lists as still asked under bypassPermissions ("Actions no mode
/// auto-approves", read 2026-10-18, Claude Code 2.1.302), and a prompt of
/// another mode, gets a deny within a second, whose message says what to do
/// instead; a session ekko agents did not start gets no answer from ekko.
#[test]
fn a_born_session_s_permission_prompt_is_denied_at_once_with_what_to_do_instead() {
    let agents = Agents::new();
    assert!(agents.ekko(&["--task", "Write hello"]).status.success());
    let session = agents.session(&agents.uid(1));
    session.ekko(&["--begin", "1"], "");
    let bash = |command: &str| serde_json::json!({"command": command});
    // Each kind, an example of it, and what its message says to do instead.
    let kinds: [(&str, &str, serde_json::Value, &str); 10] = [
        ("an explicit ask rule (decision 720's)", "Bash", bash("ekko --json --projects"), "without --project"),
        ("a connector tool the organization set to ask", "mcp__claude_ai_Gmail__send", serde_json::json!({"to": "x"}), "another way that needs no prompt"),
        ("AskUserQuestion, which requires user interaction", "AskUserQuestion", serde_json::json!({"questions": []}), "record it in a decision note"),
        ("an MCP tool marked requiresUserInteraction", "mcp__plugin_ekko_ekko__ask", serde_json::json!({"questions": []}), "record it in a decision note"),
        ("rm of a critical path", "Bash", bash("rm -rf \"$TMP\"/*"), "remove a literal path inside your worktree"),
        ("rmdir of a critical path", "Bash", bash("rmdir ~"), "remove a literal path inside your worktree"),
        ("an rm in a bash -c script it cannot check (note 1903)", "Bash", bash("for c in a b; do bash -c \"$c\"; rm -f $d/ran; done"), "not inside a bash -c script"),
        ("the cross-session messaging safeguards", "SendMessage", serde_json::json!({"to": "o", "message": "m"}), "write a note on its task"),
        ("a read outside the working directories", "Bash", bash("cat /etc/hosts"), "another way that needs no prompt"),
        ("any prompt of a mode that asks", "Write", serde_json::json!({"file_path": "/tmp/x", "content": "x"}), "another way that needs no prompt"),
    ];
    for (kind, tool, input, instead) in kinds {
        let asked = std::time::Instant::now();
        let said = session.hook("PermissionRequest", serde_json::json!({"tool_name": tool, "tool_input": input}));
        let took = asked.elapsed();
        assert!(took < std::time::Duration::from_secs(1), "{kind}: answered in {took:?}");
        let answer: serde_json::Value = serde_json::from_str(&said).unwrap_or_else(|e| panic!("{kind}: {e}: {said}"));
        let decision = &answer["hookSpecificOutput"]["decision"];
        assert_eq!(answer["hookSpecificOutput"]["hookEventName"], "PermissionRequest", "{kind}: {said}");
        assert_eq!(decision["behavior"], "deny", "{kind}: {said}");
        let message = decision["message"].as_str().unwrap_or_default();
        assert!(message.starts_with("No one answers this session's prompts"), "{kind}: {message}");
        assert!(message.contains(instead), "{kind}: {message}");
        assert!(message.contains("write it in a handoff on your task"), "{kind}: {message}");
    }
    assert!(agents.logged("its permission prompt denied at once: Bash: ekko --json --projects"));
    // The orchestrator, and any session ekko agents did not start: Claude
    // Code asks its user, as it would without ekko.
    let orchestrator = agents.orchestrator();
    let said = orchestrator.hook("PermissionRequest", serde_json::json!({"tool_name": "Bash", "tool_input": bash("rm -rf \"$TMP\"/*")}));
    assert_eq!(said, "", "a session ekko agents did not start");
    fs::remove_dir_all(&agents.home).ok();
}

/// The orchestrator's wait on a born session's task ends when that session
/// starts to wait on the user (task 1715, answer 1709): its wake hook says
/// in which window and for what, as the born hook marked the task. Waited
/// on again while the mark stands, the wait's reply says so and the wait
/// stays open; the mark cleared, the hook has nothing to tell.
#[test]
fn an_orchestrator_waiting_on_a_born_session_s_task_is_woken_when_it_waits_on_the_user() {
    let agents = Agents::new();
    assert!(agents.ekko(&["--task", "Write hello"]).status.success());
    agents.name_window("1 \u{b7} haiku");
    let session = agents.session(&agents.uid(1));
    session.ekko(&["--begin", "1"], "");
    let orchestrator = agents.orchestrator();
    let wait = ("wait", serde_json::json!({"item": 1, "until": "done", "text": "review hello"}));
    let first: serde_json::Value = serde_json::from_str(&orchestrator.tools(std::slice::from_ref(&wait))[0]).unwrap();
    assert!(first["ok"] == true && first.get("asked").is_none(), "{first}");
    let changed = serde_json::json!({"hook_event_name": "FileChanged", "session_id": "o-1", "file_path": agents.home.join("app/.ekko/storage/storage.json")}).to_string();
    orchestrator.ekko(&["--wake", "--hook"], &changed);
    assert_eq!(orchestrator.said(), "", "nothing ended the wait yet");

    session.asks();
    orchestrator.ekko(&["--wake", "--hook"], &changed);
    let told = orchestrator.said();
    assert!(
        told.starts_with(
            "1 (Write hello) is held by a session that waits on the user, in the window named \"1 \u{b7} haiku\": \
             Claude needs your permission -- Bash: date > stamp.txt. Read its handoff on the task, if it wrote one, and bring it to the user -- with your ask, for what only they can settle -- then answer the session with a note on the task, which reaches it, and wait on it again. This session waited on it (note "
        ),
        "{told}"
    );
    let again: serde_json::Value = serde_json::from_str(&orchestrator.tools(&[wait])[0]).unwrap();
    let asked = again["asked"].as_str().unwrap_or_else(|| panic!("{again}"));
    assert!(asked.starts_with("1 is held by a session that waits on the user already, since "), "{asked}");
    assert!(asked.contains(", in the window named \"1 \u{b7} haiku\": Claude needs your permission -- Bash: date > stamp.txt. Read its handoff on the task, if it wrote one, and bring it to the user; answer the session with a note on the task."), "{asked}");
    orchestrator.ekko(&["--wake", "--hook"], &changed);
    assert_eq!(orchestrator.said(), "", "the wait made again ended on the mark it was told of");

    session.ran();
    orchestrator.ekko(&["--wake", "--hook"], &changed);
    assert_eq!(orchestrator.said(), "", "a mark cleared ends no wait");

    // The next time the session waits on the user ends that wait, and the
    // window is named as it was before it said so.
    agents.name_window("1 \u{b7} haiku \u{b7} waiting on you");
    session.asks();
    orchestrator.ekko(&["--wake", "--hook"], &changed);
    let told = orchestrator.said();
    assert!(told.starts_with("1 (Write hello) is held by a session that waits on the user, in the window named \"1 \u{b7} haiku\": "), "{told}");
    fs::remove_dir_all(&agents.home).ok();
}

/// A born session whose turn ends on an API error (task 1798): Claude Code
/// fires StopFailure instead of Stop, with the error and its text. Its task
/// is marked waiting on the user with them, its window says so, and the
/// orchestrator's wait on the task ends, naming the error; the user's prompt
/// that sets it going again clears the mark. Its task done, the same event
/// closes it as Stop does.
#[test]
fn a_born_session_whose_turn_ends_on_an_api_error_waits_on_the_user_and_wakes_its_orchestrator() {
    let agents = Agents::new();
    for text in ["Write hello", "Write the docs"] {
        assert!(agents.ekko(&["--task", text]).status.success());
    }
    agents.name_window("1 \u{b7} opus");
    let session = agents.session(&agents.uid(1));
    session.ekko(&["--begin", "1"], "");
    let orchestrator = agents.orchestrator();
    let wait = ("wait", serde_json::json!({"item": 1, "until": "done", "text": "review hello"}));
    let first: serde_json::Value = serde_json::from_str(&orchestrator.tools(std::slice::from_ref(&wait))[0]).unwrap();
    assert!(first["ok"] == true, "{first}");
    let changed = serde_json::json!({"hook_event_name": "FileChanged", "session_id": "o-1", "file_path": agents.home.join("app/.ekko/storage/storage.json")}).to_string();

    let failed = serde_json::json!({"error": "server_error", "error_details": "Connection lost", "last_assistant_message": "API Error: Connection lost mid-response"});
    assert_eq!(session.hook("StopFailure", failed), "", "Claude Code ignores this event's output");
    let what = "its turn ended on an error -- API Error: Connection lost mid-response (server_error)";
    let read = agents.json(&["--context", "1", "--json"]);
    assert_eq!(read["context"]["item"]["held"]["waitsOnUser"]["what"], what, "{read}");
    assert_eq!(agents.window(), "1 \u{b7} opus \u{b7} waiting on you");
    assert!(agents.logged(&format!("waits on you (api_error): {what}; marked on task 1;")));
    orchestrator.ekko(&["--wake", "--hook"], &changed);
    let told = orchestrator.said();
    assert!(told.starts_with(&format!("1 (Write hello) is held by a session that waits on the user, in the window named \"1 \u{b7} opus\": {what}. ")), "{told}");

    // The user types "go on" into its window.
    session.hook("UserPromptSubmit", serde_json::json!({"prompt": "go on"}));
    let read = agents.json(&["--context", "1", "--json"]);
    assert!(read["context"]["item"]["held"].get("waitsOnUser").is_none(), "{read}");
    assert_eq!(agents.window(), "1 \u{b7} opus");

    // Its task done, an API error at the end of its turn closes it.
    let uid = agents.uid(2);
    agents.prompt(&uid, "s-2");
    assert!(agents.ekko(&["--check", "2"]).status.success());
    let (mut stand_in, _) = agents.born(Some(&uid), "s-2", "StopFailure", serde_json::json!({"error": "rate_limit", "last_assistant_message": "API Error: Rate limit reached"}));
    assert!(ended_by_sigterm(&mut stand_in), "a finished session's turn ended on an API error, and it stayed");
    fs::remove_dir_all(&agents.home).ok();
}

/// `ekko agents merge` (task 1714, answer 1708): a done task's branch goes
/// into the branch the project's checkout is on, rebased onto it, tested
/// with the command the user set, then fast-forwarded, and its worktree and
/// branch go. A test that fails, a conflict or work left uncommitted leave
/// main and the branch as they were and hand the task back, pending, with
/// a note saying why and what to do. Refused, changing nothing, until a
/// test command is set, for a task not done, and where main's checkout has
/// a file in the way.
#[test]
fn ekko_agents_merge_takes_a_tested_branch_in_and_hands_a_failing_one_back() {
    let agents = Agents::new();
    let app = fs::canonicalize(&agents.app).unwrap();
    fs::write(app.join("check.sh"), "echo checking\ntest ! -e broken\n").unwrap();
    fs::write(app.join("a.txt"), "a\n").unwrap();
    agents.git(&app, &["add", "check.sh", "a.txt"]);
    agents.git(&app, &["commit", "-q", "-m", "check"]);
    for text in ["Add b", "Break the check", "Change a", "Add c", "Leave work uncommitted", "Add d"] {
        assert!(agents.ekko(&["--task", text]).status.success());
    }
    // Each task's branch as start makes it, with its session's commit.
    let made = |n: u32, file: &str| {
        let branch = format!("task-{n}");
        let worktree = agents.worktree(&branch);
        agents.git(&app, &["worktree", "add", "-q", "-b", &branch, worktree.to_str().unwrap(), "HEAD"]);
        fs::write(worktree.join(file), format!("{file} of task {n}\n")).unwrap();
        agents.git(&worktree, &["add", file]);
        agents.git(&worktree, &["commit", "-q", "-m", &format!("Task {n}")]);
        worktree
    };
    let worktrees = [made(1, "b.txt"), made(2, "broken"), made(3, "a.txt"), made(4, "c.txt"), made(5, "e.txt"), made(6, "d.txt")];
    fs::write(worktrees[4].join("e.txt"), "changed, and not committed\n").unwrap();
    fs::write(app.join("a.txt"), "main's\n").unwrap();
    agents.git(&app, &["commit", "-q", "-am", "Main changes a"]);
    assert!(agents.ekko(&["--check", "1", "2", "3", "4", "5", "6"]).status.success());
    let main = || agents.git(&app, &["rev-parse", "main"]);
    let tip = |branch: &str| agents.git(&app, &["rev-parse", branch]);
    let on = |worktree: &std::path::Path| agents.git(worktree, &["symbolic-ref", "--short", "HEAD"]);
    let board = || -> serde_json::Value { serde_json::from_str(&fs::read_to_string(agents.home.join("app/.ekko/storage/storage.json")).unwrap()).unwrap() };
    let state = |id: u32| {
        let task = &board()[id.to_string()];
        (task["isComplete"].as_bool().unwrap(), task["inProgress"].as_bool().unwrap())
    };
    let note = |id: u64| board()[id.to_string()]["description"].as_str().unwrap().to_string();
    let json = |output: process::Output| -> serde_json::Value {
        serde_json::from_slice(&output.stdout).unwrap_or_else(|e| panic!("{e}: {}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr)))
    };

    // No test command yet: refused, and nothing changed.
    let before = main();
    let unset = json(agents.merging(&["--json", "agents", "merge", "1"]));
    assert!(unset["error"].as_str().unwrap().starts_with("No test command is set for this project"), "{unset}");
    let shown = String::from_utf8(agents.ekko(&["agents", "test"]).stdout).unwrap();
    assert!(shown.starts_with("No test command is set: ekko agents merge refuses until the user sets one"), "{shown}");
    let empty = agents.ekko(&["agents", "test", " "]);
    assert!(!empty.status.success() && String::from_utf8_lossy(&empty.stdout).contains("An empty command tests nothing"), "{empty:?}");
    assert!(!agents.home.join("app/.ekko/storage/agents.json").exists());
    assert!(agents.ekko(&["agents", "test", "sh check.sh"]).status.success());
    let shown = String::from_utf8(agents.ekko(&["agents", "test"]).stdout).unwrap();
    assert!(shown.starts_with("ekko agents merge tests each branch with `sh check.sh`"), "{shown}");
    assert_eq!((main(), state(1)), (before.clone(), (true, false)));

    // Merged: rebased onto main, tested, fast-forwarded, and gone.
    let merged = json(agents.merging(&["--json", "agents", "merge", "1"]));
    assert_eq!((merged["ok"].as_bool(), merged["commits"].as_u64(), merged["into"].as_str()), (Some(true), Some(1), Some("main")), "{merged}");
    assert_eq!((merged["from"].as_str(), merged["to"].as_str(), merged["worktree"].as_str()), (Some(before.as_str()), Some(main().as_str()), Some("removed")), "{merged}");
    assert_eq!(merged["tested"]["command"], "sh check.sh", "{merged}");
    assert_eq!(tip("main~1"), before, "task-1's commit, on main as it was");
    assert_eq!(agents.git(&app, &["show", "main:b.txt"]), "b.txt of task 1");
    assert!(!worktrees[0].exists() && !agents.has_branch("task-1"), "its worktree and branch are gone");
    let said = note(merged["notes"][0].as_u64().unwrap());
    assert!(said.starts_with(&format!("Merged into main by ekko agents merge: 1 commit of task-1, rebased onto main at {}, passed `sh check.sh` in ", &before[..7])), "{said}");
    assert_eq!(state(1), (true, false), "still done");

    // A test that fails: main and the branch as they were, the task back.
    let (before, two) = (main(), tip("task-2"));
    let failed = agents.merging(&["--json", "agents", "merge", "2"]);
    assert!(!failed.status.success());
    let failed = json(failed);
    assert_eq!((failed["ok"].as_bool(), failed["code"].as_str(), failed["reopened"].clone()), (Some(false), Some("NOT_MERGED"), serde_json::json!([2])), "{failed}");
    assert!(failed["why"].as_str().unwrap().starts_with("rebased onto main, task-2 failed `sh check.sh` (exit code 1, after "), "{failed}");
    assert_eq!((main(), tip("task-2"), on(&worktrees[1])), (before.clone(), two, "task-2".to_string()));
    assert_eq!(agents.git(&worktrees[1], &["status", "--porcelain"]), "");
    assert_eq!(state(2), (false, false), "pending again");
    let said = note(failed["notes"][0].as_u64().unwrap());
    assert!(said.starts_with("Not merged into main: rebased onto main, task-2 failed `sh check.sh` (exit code 1, after "), "{said}");
    let to_do = format!("To do: in {}, rebase task-2 onto main (git rebase main), make `sh check.sh` pass, commit, and set the task done again.", worktrees[1].display());
    assert!(said.contains(&to_do) && said.contains("The end of its output:\n```\nchecking\n```"), "{said}");
    assert_eq!(fs::read_to_string(failed["log"].as_str().unwrap()).unwrap(), "checking\n");
    let pending = String::from_utf8(agents.merging(&["agents", "merge", "2"]).stdout).unwrap();
    assert!(pending.contains("task 2 is pending: ekko agents merge takes the branch of tasks their session set done"), "{pending}");

    // A conflict: the same.
    let three = tip("task-3");
    let conflict = json(agents.merging(&["--json", "agents", "merge", "3"]));
    assert_eq!((conflict["why"].as_str(), conflict["reopened"].clone()), (Some("rebased onto main, task-3 conflicts in a.txt"), serde_json::json!([3])), "{conflict}");
    assert_eq!((main(), tip("task-3"), on(&worktrees[2]), state(3)), (before.clone(), three, "task-3".to_string(), (false, false)));
    let said = note(conflict["notes"][0].as_u64().unwrap());
    assert!(said.contains("resolve the conflicts, run `sh check.sh`, commit, and set the task done again.") && said.contains("The rebase was undone"), "{said}");

    // A file in the way in main's checkout: refused, and nothing changed;
    // gone, merged and, as --keep asks, kept.
    fs::write(app.join("c.txt"), "the user's own\n").unwrap();
    let four = tip("task-4");
    let blocked = json(agents.merging(&["--json", "agents", "merge", "4"]));
    let why = blocked["error"].as_str().unwrap();
    assert!(why.starts_with("main was not moved, and task-4 is as its session left it: ") && why.contains("c.txt"), "{blocked}");
    assert_eq!((main(), tip("task-4"), on(&worktrees[3]), state(4)), (before.clone(), four, "task-4".to_string(), (true, false)));
    assert_eq!(fs::read_to_string(app.join("c.txt")).unwrap(), "the user's own\n");
    fs::remove_file(app.join("c.txt")).unwrap();
    let kept = json(agents.merging(&["--json", "agents", "merge", "4", "--keep"]));
    assert_eq!((kept["ok"].as_bool(), kept["worktree"].as_str()), (Some(true), Some("kept")), "{kept}");
    assert_eq!((tip("task-4"), on(&worktrees[3])), (main(), "task-4".to_string()), "kept, at main");

    // Work left uncommitted: the task back.
    let before = main();
    let uncommitted = json(agents.merging(&["--json", "agents", "merge", "5"]));
    assert_eq!(uncommitted["why"], "its worktree holds work not committed on task-5", "{uncommitted}");
    let said = note(uncommitted["notes"][0].as_u64().unwrap());
    assert!(said.contains("```\n M e.txt\n```"), "{said}");
    assert_eq!((main(), state(5)), (before, (false, false)));

    // No tests, as the user said: in untested.
    assert!(agents.ekko(&["agents", "test", "--none"]).status.success());
    let untested = json(agents.merging(&["--json", "agents", "merge", "6"]));
    assert_eq!((untested["ok"].as_bool(), untested["tested"].as_str()), (Some(true), Some("untested")), "{untested}");
    assert!(note(untested["notes"][0].as_u64().unwrap()).contains(", untested, as the user said the project has no tests; main is at "), "{untested}");
    fs::remove_dir_all(&agents.home).ok();
}

/// A commit that holds conflict markers, with the next commit removing them,
/// passes the test at the branch's tip, yet goes into main as a commit that
/// does not build (task 2017): merge checks each commit, and hands the branch
/// back naming it. Whitespace errors alone, and a clean branch, go in.
#[test]
fn ekko_agents_merge_hands_back_a_commit_that_holds_conflict_markers() {
    let agents = Agents::new();
    let app = fs::canonicalize(&agents.app).unwrap();
    for text in ["Marked, then cleaned", "Clean", "Whitespace only"] {
        assert!(agents.ekko(&["--task", text]).status.success());
    }
    let made = |n: u32| {
        let branch = format!("task-{n}");
        let worktree = agents.worktree(&branch);
        agents.git(&app, &["worktree", "add", "-q", "-b", &branch, worktree.to_str().unwrap(), "HEAD"]);
        worktree
    };
    let commit = |worktree: &std::path::Path, file: &str, text: &str, message: &str| {
        fs::write(worktree.join(file), text).unwrap();
        agents.git(worktree, &["add", file]);
        agents.git(worktree, &["commit", "-q", "-m", message]);
    };
    let worktrees = [made(1), made(2), made(3)];
    commit(&worktrees[0], "m.txt", "a\n<<<<<<< ours\nb\n=======\nc\n>>>>>>> theirs\n", "Marked");
    commit(&worktrees[0], "m.txt", "a\n", "Cleaned");
    commit(&worktrees[1], "n.txt", "n\n", "Clean");
    commit(&worktrees[2], "w.txt", "trailing \n", "Whitespace");
    assert!(agents.ekko(&["agents", "test", "--none"]).status.success());
    assert!(agents.ekko(&["--check", "1", "2", "3"]).status.success());
    let main = || agents.git(&app, &["rev-parse", "main"]);
    let board = || -> serde_json::Value { serde_json::from_str(&fs::read_to_string(agents.home.join("app/.ekko/storage/storage.json")).unwrap()).unwrap() };
    let json = |output: process::Output| -> serde_json::Value {
        serde_json::from_slice(&output.stdout).unwrap_or_else(|e| panic!("{e}: {}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr)))
    };

    // Handed back, naming the commit and the lines; nothing moved.
    let (before, one) = (main(), agents.git(&app, &["rev-parse", "task-1"]));
    let marked = json(agents.merging(&["--json", "agents", "merge", "1"]));
    assert_eq!((marked["ok"].as_bool(), marked["code"].as_str(), marked["reopened"].clone()), (Some(false), Some("NOT_MERGED"), serde_json::json!([1])), "{marked}");
    assert_eq!(marked["why"], "rebased onto main, task-1 holds leftover conflict markers in 3 places", "{marked}");
    assert_eq!((main(), agents.git(&app, &["rev-parse", "task-1"])), (before.clone(), one));
    assert_eq!(agents.git(&worktrees[0], &["symbolic-ref", "--short", "HEAD"]), "task-1");
    let said = board()[marked["notes"][0].as_u64().unwrap().to_string()]["description"].as_str().unwrap().to_string();
    for line in ["m.txt:2", "m.txt:4", "m.txt:6"] {
        assert!(said.contains(&format!(" Marked: {line}")), "{said}");
    }
    assert!(!said.contains("Cleaned"), "{said}");

    // Whitespace errors alone and a clean branch go in.
    let clean = json(agents.merging(&["--json", "agents", "merge", "2"]));
    assert_eq!(clean["ok"].as_bool(), Some(true), "{clean}");
    let spaced = json(agents.merging(&["--json", "agents", "merge", "3"]));
    assert_eq!(spaced["ok"].as_bool(), Some(true), "{spaced}");
    assert_eq!(agents.git(&app, &["show", "main:w.txt"]), "trailing");
    fs::remove_dir_all(&agents.home).ok();
}

/// A task's session starts, and its branch goes in, only once the branches
/// of its done blockers are in main (task 1728): the session starts from
/// main, and main takes a blocker's work first. Each refusal changes
/// nothing and names the merge to run. A cancelled blocker holds nothing
/// back; a blocker merged does, whether its branch went or `--keep` kept
/// it; a group's branch counts for each task it was made for.
#[test]
fn a_task_starts_and_merges_only_after_its_blockers_branches_are_in_main() {
    let agents = Agents::new();
    let app = fs::canonicalize(&agents.app).unwrap();
    for text in ["Base", "On the base", "Also on the base", "After a dropped one", "Dropped", "Group one", "Group two", "After the group"] {
        assert!(agents.ekko(&["--task", text]).status.success());
    }
    // 7 waits on 6 too, whose work is on the same branch.
    for (blocked, by) in [("@2", "1"), ("@3", "1"), ("@4", "5"), ("@7", "6"), ("@8", "7")] {
        assert!(agents.ekko(&["--blocked-by", blocked, by]).status.success());
    }
    assert!(agents.ekko(&["agents", "test", "true"]).status.success());
    assert!(agents.ekko(&["agents", "limit", "9"]).status.success());
    // Each branch as start makes it, with its session's commit.
    let made = |branch: &str, file: &str| {
        let worktree = agents.worktree(branch);
        agents.git(&app, &["worktree", "add", "-q", "-b", branch, worktree.to_str().unwrap(), "HEAD"]);
        fs::write(worktree.join(file), format!("{file} of {branch}\n")).unwrap();
        agents.git(&worktree, &["add", file]);
        agents.git(&worktree, &["commit", "-q", "-m", branch]);
    };
    made("task-1", "base.txt");
    made("task-3", "three.txt");
    made("task-5", "five.txt");
    made("task-6-7", "group.txt");
    assert!(agents.ekko(&["--check", "1", "3", "6", "7"]).status.success());
    assert!(agents.ekko(&["--set", "cancelled", "5"]).status.success());
    let main = || agents.git(&app, &["rev-parse", "main"]);
    let before = main();
    let refused_start = |id: &str| -> String {
        let opened = agents.opened().len();
        let reply = agents.json(&["agents", "start", id, "--model", "haiku", "--json"]);
        assert_eq!(reply["code"], "INVALID_INPUT", "{reply}");
        assert_eq!(agents.opened().len(), opened, "a refused start opened a window");
        assert!(!agents.worktree(&format!("task-{id}")).exists(), "a worktree for task {id}");
        reply["error"].as_str().unwrap().to_string()
    };
    let started = |id: &str| {
        let reply = agents.json(&["agents", "start", id, "--model", "haiku", "--json"]);
        assert_eq!(reply["ok"], true, "{reply}");
    };

    let said = refused_start("2");
    assert_eq!(
        said,
        "task 2 waits on task 1, whose work is on branch task-1, not in main yet: merge it first with ekko agents merge 1, as task 2's session starts from main"
    );
    let output = agents.merging(&["--json", "agents", "merge", "3"]);
    let reply: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let said = reply["error"].as_str().unwrap_or_else(|| panic!("{reply}"));
    assert!(
        said.starts_with("task 3 waits on task 1, whose work is on branch task-1, not in main yet: merge it first with ekko agents merge 1, so main takes task 3's work after it"),
        "{said}"
    );
    assert_eq!((main(), agents.git(&app, &["rev-parse", "task-3^"])), (before.clone(), before.clone()), "main and task-3 as they were");
    // A group's branch, for each task it was made for.
    assert!(refused_start("8").contains("whose work is on branch task-6-7, not in main yet: merge it first with ekko agents merge 6 7,"));
    // A cancelled blocker's branch holds nothing back.
    started("4");

    // Merged, its branch kept: the blocked start, and merge, go ahead.
    let kept = agents.merging(&["--json", "agents", "merge", "1", "--keep"]);
    assert!(kept.status.success(), "{}", String::from_utf8_lossy(&kept.stdout));
    assert!(agents.has_branch("task-1"));
    started("2");
    let merged = agents.merging(&["--json", "agents", "merge", "3"]);
    assert!(merged.status.success(), "{}", String::from_utf8_lossy(&merged.stdout));
    assert_eq!(agents.git(&app, &["ls-tree", "--name-only", "main"]), "base.txt\nthree.txt");
    // Merged, its branch gone.
    let merged = agents.merging(&["--json", "agents", "merge", "6", "7"]);
    assert!(merged.status.success(), "{}", String::from_utf8_lossy(&merged.stdout));
    assert!(!agents.has_branch("task-6-7"));
    started("8");
    fs::remove_dir_all(&agents.home).ok();
}

/// The test that gates a merge runs as CI runs it, with no Claude Code
/// session around it (task 1942): not the orchestrator's CLAUDECODE, nor
/// what Claude Code adds to each command it runs, nor its terminal tab
/// (task 1803) -- the variables a born session's window is started without
/// -- while the rest of its environment is the test's.
#[test]
fn ekko_agents_merge_runs_the_test_without_the_orchestrators_session_variables() {
    let agents = Agents::new();
    let app = fs::canonicalize(&agents.app).unwrap();
    assert!(agents.ekko(&["--task", "Add it"]).status.success());
    let worktree = agents.worktree("task-1");
    agents.git(&app, &["worktree", "add", "-q", "-b", "task-1", worktree.to_str().unwrap(), "HEAD"]);
    fs::write(worktree.join("it.txt"), "added\n").unwrap();
    agents.git(&worktree, &["add", "."]);
    agents.git(&worktree, &["commit", "-q", "-m", "Task 1"]);
    assert!(agents.ekko(&["--check", "1"]).status.success());
    let dumped = agents.home.join("gate-env");
    assert!(agents.ekko(&["agents", "test", &format!("env > {}", dumped.display())]).status.success());

    // Run by the orchestrator: a Claude Code process whose commands carry
    // CLAUDECODE and what a version of it adds (the stand-in exports both).
    let orchestrator = agents.orchestrator();
    let merged = orchestrator.ekko(&["--json", "agents", "merge", "1"], "");
    assert!(merged.contains("\"ok\":true"), "{merged}");
    let gate = fs::read_to_string(&dumped).unwrap();
    for name in ["CLAUDECODE", "EKKO_TEST_ADDED_BY_CLAUDE_CODE"] {
        assert!(!gate.lines().any(|line| line.starts_with(&format!("{name}="))), "{name} reached the test: {gate}");
    }
    assert!(gate.lines().any(|line| line.starts_with("HOME=")), "the rest of the environment is the test's: {gate}");
    assert!(gate.lines().any(|line| line.starts_with("EKKO_PROJECT=")), "the orchestrator's own launcher variables stay: {gate}");
    fs::remove_dir_all(&agents.home).ok();
}

/// `ekko agents merge` waits while a session of the tasks closes itself,
/// and refuses, changing nothing, while one stays: someone talks to it, and
/// its worktree is not to move under it (task 1714). Nor does what is not
/// the branch's to fix hand it back: another merge running, a rebase git
/// refuses for want of an identity, main moving while the tests run. A
/// merge cut short, which left the worktree on a detached HEAD, is put back
/// on its branch and taken up; a detached HEAD no merge left is not touched.
#[test]
fn ekko_agents_merge_waits_for_the_session_to_close_and_takes_up_a_merge_cut_short() {
    use std::os::fd::AsRawFd as _;
    let agents = Agents::new();
    let app = fs::canonicalize(&agents.app).unwrap();
    for n in 1..=4 {
        assert!(agents.ekko(&["--task", &format!("Add {n}")]).status.success());
        let branch = format!("task-{n}");
        let worktree = agents.worktree(&branch);
        agents.git(&app, &["worktree", "add", "-q", "-b", &branch, worktree.to_str().unwrap(), "HEAD"]);
        fs::write(worktree.join(format!("{n}.txt")), "added\n").unwrap();
        agents.git(&worktree, &["add", "."]);
        agents.git(&worktree, &["commit", "-q", "-m", &format!("Task {n}")]);
    }
    assert!(agents.ekko(&["--check", "1", "2", "3", "4"]).status.success());
    assert!(agents.ekko(&["agents", "test", "true"]).status.success());
    // Main moves, so each merge rebases: one onto main as it is is no
    // rebase, and runs no hook.
    agents.git(&app, &["commit", "-q", "--allow-empty", "-m", "Main moves"]);
    let main = || agents.git(&app, &["rev-parse", "main"]);
    let before = main();
    let done = |id: u32| -> bool {
        let board: serde_json::Value = serde_json::from_str(&fs::read_to_string(agents.home.join("app/.ekko/storage/storage.json")).unwrap()).unwrap();
        board[id.to_string()]["isComplete"].as_bool().unwrap()
    };
    let refused = |output: process::Output| -> String {
        let said = String::from_utf8(output.stdout).unwrap();
        assert!(!output.status.success(), "{said}");
        said
    };
    let four = agents.worktree("task-4");
    let unmoved = || {
        assert_eq!((main(), agents.git(&four, &["symbolic-ref", "--short", "HEAD"]), done(4)), (before.clone(), "task-4".to_string(), true));
    };

    // Another merge runs.
    let lock = fs::OpenOptions::new().write(true).create(true).truncate(false).open(app.join(".git/ekko-merge.lock")).unwrap();
    assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) }, 0);
    let said = refused(agents.merging(&["agents", "merge", "4"]));
    assert!(said.contains("another ekko agents merge runs on "), "{said}");
    drop(lock);
    unmoved();
    // A hook that refuses the rebase.
    let hook = app.join(".git/hooks/pre-rebase");
    write_executable(&hook, "#!/bin/sh\necho 'not today' >&2\nexit 1\n");
    let said = refused(agents.merging(&["agents", "merge", "4"]));
    assert!(said.contains("task-4 could not be rebased onto main, for a reason other than a conflict, so main and task-4 are as they were: "), "{said}");
    assert!(said.contains("not today"), "{said}");
    fs::remove_file(&hook).unwrap();
    unmoved();
    // main moves while the tests run.
    let moving = format!("git -C {} commit -q --allow-empty -m Moved", app.display());
    assert!(agents.ekko(&["agents", "test", &moving]).status.success());
    let said = refused(agents.merging(&["agents", "merge", "4"]));
    assert!(said.contains(&format!("main was not moved, and task-4 is as its session left it: main moved while the tests ran, from {} to ", &before[..7])), "{said}");
    agents.git(&app, &["reset", "-q", "--hard", &before]);
    unmoved();
    // The project's checkout leaves main while the tests run.
    let leaving = format!("git -C {} checkout -q -b elsewhere", app.display());
    assert!(agents.ekko(&["agents", "test", &leaving]).status.success());
    let said = refused(agents.merging(&["agents", "merge", "4"]));
    assert!(said.contains("main was not moved, and task-4 is as its session left it: the project's checkout left main for elsewhere while the tests ran"), "{said}");
    agents.git(&app, &["checkout", "-q", "main"]);
    unmoved();
    assert!(agents.ekko(&["agents", "test", "true"]).status.success());
    // A task cancelled, none done; tasks with no branch, or one of a group.
    assert!(agents.ekko(&["--task", "Dropped"]).status.success() && agents.ekko(&["--set", "cancelled", "5"]).status.success());
    let said = refused(agents.merging(&["agents", "merge", "5"]));
    assert!(said.contains("task 5 is cancelled: no work of its goes in"), "{said}");
    assert!(agents.ekko(&["--task", "Never started"]).status.success() && agents.ekko(&["--check", "6"]).status.success());
    let said = refused(agents.merging(&["agents", "merge", "6"]));
    assert!(said.contains("there is no branch task-6 in ") && said.contains(": ekko agents start makes one for the tasks it starts together, and none of these tasks has one"), "{said}");
    agents.git(&app, &["branch", "task-6-7"]);
    let said = refused(agents.merging(&["agents", "merge", "6"]));
    assert!(said.contains(", and theirs is task-6-7: ekko agents merge 6 7"), "{said}");
    unmoved();

    let mux = agents.home.join("mux-state");
    fs::create_dir_all(&mux).unwrap();
    fs::write(mux.join("session"), "").unwrap();
    // Another project's window and pane for task 1 do not delay it (task
    // 1731): it goes on at once, to be refused further on, the checkout on
    // no branch.
    fs::write(mux.join("windows"), "1 \u{b7} haiku\n").unwrap();
    fs::write(mux.join("born-panes"), "%3\t0\t@3\t1 \u{b7} haiku\t1\tOther's task\thaiku\t\tworking\t1\t\tother\n").unwrap();
    agents.git(&app, &["checkout", "-q", "--detach"]);
    let started = std::time::Instant::now();
    let said = refused(agents.merging(&["agents", "merge", "1"]));
    assert!(said.contains("is on no branch") && !said.contains("still runs"), "{said}");
    assert!(started.elapsed() < std::time::Duration::from_secs(5), "{:?}", started.elapsed());
    agents.git(&app, &["checkout", "-q", "main"]);
    fs::remove_file(mux.join("born-panes")).unwrap();
    unmoved();
    // A session of task 1 stays: refused once the wait is over.
    fs::write(mux.join("windows"), "1 \u{b7} haiku\n").unwrap();
    let started = std::time::Instant::now();
    let stays = agents.merging(&["agents", "merge", "1"]);
    let said = String::from_utf8(stays.stdout).unwrap();
    assert!(!stays.status.success() && said.contains("a session for these tasks still runs, in window 1 \u{b7} haiku of session ekko, 10 s after"), "{said}");
    assert!(started.elapsed() >= std::time::Duration::from_secs(10), "{:?}", started.elapsed());
    assert_eq!(main(), before);
    // It closes while the merge waits: merged.
    let closes = {
        let windows = mux.join("windows");
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(1500));
            fs::write(windows, "").unwrap();
        })
    };
    let started = std::time::Instant::now();
    let merged = agents.merging(&["agents", "merge", "1"]);
    closes.join().unwrap();
    assert!(merged.status.success(), "{}", String::from_utf8_lossy(&merged.stdout));
    assert!((1..10).contains(&started.elapsed().as_secs()), "{:?}", started.elapsed());
    assert_eq!(agents.git(&app, &["rev-parse", "main~1"]), before);

    // Cut short: detached, with the file a merge leaves there.
    let two = agents.worktree("task-2");
    agents.git(&two, &["checkout", "-q", "--detach"]);
    let underway = agents.git(&two, &["rev-parse", "--path-format=absolute", "--git-path", "ekko-merge"]);
    fs::write(&underway, format!("{}\n", agents.git(&two, &["rev-parse", "HEAD"]))).unwrap();
    let taken_up = agents.merging(&["agents", "merge", "2"]);
    assert!(taken_up.status.success(), "{}", String::from_utf8_lossy(&taken_up.stdout));
    assert_eq!(agents.git(&app, &["show", "main:2.txt"]), "added");
    // Detached by someone else: left as it is.
    let three = agents.worktree("task-3");
    agents.git(&three, &["checkout", "-q", "--detach"]);
    let theirs = agents.merging(&["agents", "merge", "3"]);
    let said = String::from_utf8(theirs.stdout).unwrap();
    assert!(!theirs.status.success() && said.contains("is a worktree on no branch, not task-3"), "{said}");
    assert!(common::command("git").arg("-C").arg(&three).args(["symbolic-ref", "-q", "HEAD"]).output().unwrap().status.code() == Some(1), "still detached");
    fs::remove_dir_all(&agents.home).ok();
}

/// The commands of each call that sets options, each its arguments, from
/// the first `set-option` of the call on, split where tmux splits them.
fn option_sets(calls: Vec<Vec<String>>) -> Vec<Vec<Vec<String>>> {
    calls
        .into_iter()
        .filter_map(|call| {
            let at = call.iter().position(|arg| arg == "set-option")?;
            Some(call[at..].split(|arg| arg == ";").map(<[String]>::to_vec).collect())
        })
        .collect()
}

/// A born session's pane holds what the views draw (task 1675): start sets
/// its tasks, the first one's title, its model, effort and state, and its
/// window's status line, in one call, on that pane and window alone; the
/// hook sets its state as it changes -- working on a prompt or a tool,
/// waiting on the user with what for, idle at the end of a turn that leaves
/// it open, working while background work runs -- and calls the
/// multiplexer for a change only.
#[test]
fn a_born_session_s_pane_holds_its_state_for_the_views_as_it_changes() {
    let agents = Agents::new();
    for task in ["Fix #3 in the parser; then ship;", "Second", "Third"] {
        assert!(agents.ekko(&["--task", task]).status.success());
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
    let started = agents.json(&["agents", "start", "1", "--model", "haiku", "--effort", "low", "--json"]);
    assert_eq!(started["drawn"], true, "{started}");
    assert!(started.get("undrawn").is_none(), "{started}");
    let sets = option_sets(agents.calls());
    assert_eq!(sets.len(), 1, "one call: {sets:?}");
    let pane = |option: &str, value: &str| ["set-option", "-p", "-t", "%7", option, value].map(str::to_string).to_vec();
    let since = sets[0][5][5].parse::<i64>().unwrap_or_else(|_| panic!("{:?}", sets[0][5]));
    assert!((now - 5..=now + 60).contains(&since), "{since} against {now}");
    let format = sets[0][7][5].clone();
    let window = |option: &str| ["set-option", "-w", "-t", "%7", option, &format].map(str::to_string).to_vec();
    assert_eq!(
        sets[0],
        [
            pane("@ekko_tasks", "1"),
            // A `#` doubled and a last `;` escaped, as a status line and tmux read them.
            pane("@ekko_title", "Fix ##3 in the parser; then ship\\;"),
            pane("@ekko_model", "haiku"),
            pane("@ekko_effort", "low"),
            pane("@ekko_state", "working"),
            pane("@ekko_since", &since.to_string()),
            ["set-option", "-p", "-u", "-t", "%7", "@ekko_waits"].map(str::to_string).to_vec(),
            window("window-status-format"),
            window("window-status-current-format"),
            // Its project (task 1727), as one session holds every project's.
            pane("@ekko_project", "app"),
        ]
    );
    // Each state read from every pane of the window, not its active one
    // alone (task 1695): a pane beside the session's may be the active one.
    for (symbol, state) in [("\u{2699} #W", "working"), ("\u{25cf} #W", "waiting"), ("\u{25cb} #W", "idle")] {
        assert!(format.contains(symbol) && format.contains(&format!("#{{P:#{{?#{{==:#{{@ekko_state}},{state}}},")), "{state} in {format}");
    }
    assert!(!format.replace("#{P:#{?#{==:#{@ekko_state},", "").contains("@ekko_state"), "a state read from the active pane: {format}");
    // tmux's own form, or, with ztmux's tab bar on, its tab (task 1678):
    // its colours, the current one green, and the symbol before the name.
    let (tab, plain) = format.strip_prefix("#{?#{@ztmux-tab-bar},").and_then(|rest| rest.split_once(",#I:")).unwrap_or_else(|| panic!("{format}"));
    assert!(plain.ends_with("#{?window_flags,#{window_flags}, }}"), "{plain}");
    assert!(tab.contains("#{?window_active,#[fg=colour235#,bg=colour108#,bold],#[fg=colour245#,bg=colour237]}") && tab.ends_with(",\u{25cb} ,}}}#W #[default]"), "{tab}");
    assert!(["\u{25cf} ,", "\u{2699} ,", "#[fg=black#,bg=yellow#,bold]"].iter().all(|part| tab.contains(part)), "{tab}");

    // Several tasks: the first one's title, and how many more.
    let started = agents.json(&["agents", "start", "2", "3", "--model", "sonnet", "--json"]);
    assert_eq!(started["drawn"], true, "{started}");
    let sets = option_sets(agents.calls());
    assert_eq!(sets[1][..4], [pane("@ekko_tasks", "2,3"), pane("@ekko_title", "Second (+1)"), pane("@ekko_model", "sonnet"), ["set-option", "-p", "-u", "-t", "%7", "@ekko_effort"].map(str::to_string).to_vec()]);
    assert!(sets.iter().flatten().all(|command| !command.contains(&"-g".to_string())), "a global option set: {sets:?}");

    // The hook, as the session lives.
    let session = agents.session(&agents.uid(1));
    session.ekko(&["--begin", "1"], "");
    let socket = agents.home.join("tmux-socket");
    let states = || -> Vec<(String, Option<String>)> {
        let calls = agents.calls().into_iter().filter(|call| call[..2] == ["-S", socket.to_str().unwrap()]).collect();
        option_sets(calls)
            .into_iter()
            .map(|commands| {
                let value = |option: &str| commands.iter().find(|command| command[4..].first().is_some_and(|at| at == option) && command[2] == "-t").map(|command| command[5].clone());
                let unset = commands.iter().any(|command| command[..4] == ["set-option", "-p", "-u", "-t"] && command[5] == "@ekko_waits");
                assert!(value("@ekko_since").is_some_and(|at| at.parse::<i64>().is_ok()), "{commands:?}");
                assert_eq!(value("@ekko_waits").is_none(), unset, "{commands:?}");
                (value("@ekko_state").unwrap(), value("@ekko_waits"))
            })
            .collect()
    };
    let working = ("working".to_string(), None);
    session.hook("UserPromptSubmit", serde_json::json!({"prompt": "go on"}));
    assert_eq!(states(), std::slice::from_ref(&working));
    session.ran();
    assert_eq!(states(), std::slice::from_ref(&working), "a tool ran while it worked: no call");
    session.asks();
    let waiting = ("waiting".to_string(), Some("Claude needs your permission -- Bash: date > stamp.txt".to_string()));
    assert_eq!(states(), [working.clone(), waiting.clone()]);
    session.ran();
    assert_eq!(states(), [working.clone(), waiting.clone(), working.clone()]);
    let stop = |background: serde_json::Value| session.hook("Stop", serde_json::json!({"background_tasks": background, "session_crons": []}));
    stop(serde_json::json!([]));
    let idle = ("idle".to_string(), None);
    assert_eq!(states(), [working.clone(), waiting.clone(), working.clone(), idle.clone()]);
    stop(serde_json::json!([]));
    assert_eq!(states().len(), 4, "idle again: no call");
    assert!(agents.ekko(&["--check", "1"]).status.success());
    stop(serde_json::json!([{"id": "b-1", "type": "shell", "status": "running"}]));
    assert_eq!(states(), [working.clone(), waiting, working.clone(), idle, working]);
    fs::remove_dir_all(&agents.home).ok();
}

/// A multiplexer that refuses the options still runs the session (task
/// 1675): start says its state is not drawn, and why, in --json and in
/// words; the hook says so in born.log, and sets the state at the next
/// change once the options are taken.
#[test]
fn a_multiplexer_that_refuses_the_options_still_runs_the_session() {
    let agents = Agents::new();
    for task in ["Write hello", "Write the docs"] {
        assert!(agents.ekko(&["--task", task]).status.success());
    }
    let refuse = agents.home.join("mux-state").join("refuse");
    fs::create_dir_all(refuse.parent().unwrap()).unwrap();
    fs::write(&refuse, "").unwrap();
    let started = agents.json(&["agents", "start", "1", "--model", "haiku", "--json"]);
    assert_eq!((&started["ok"], &started["drawn"]), (&serde_json::json!(true), &serde_json::json!(false)), "{started}");
    assert!(started["undrawn"].as_str().is_some_and(|why| why.ends_with("set-option failed: unknown flag -p")), "{started}");
    let output = agents.ekko(&["agents", "start", "2", "--model", "haiku"]);
    let said = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success() && said.contains("Started task 2"), "{said}");
    assert!(said.lines().any(|line| line.starts_with("  its state is not drawn in the status line: ") && line.ends_with("set-option failed: unknown flag -p")), "{said}");
    assert_eq!(agents.opened().len(), 2, "both windows opened");

    let session = agents.session(&agents.uid(1));
    session.ekko(&["--begin", "1"], "");
    session.hook("UserPromptSubmit", serde_json::json!({"prompt": "go on"}));
    assert!(agents.logged("its pane's state, working, not set: ") && agents.logged("set-option failed: unknown flag -p"), "{:?}", fs::read_to_string(agents.home.join(".local/state/ekko/born/born.log")));
    fs::remove_file(&refuse).unwrap();
    let tried = option_sets(agents.calls()).len();
    session.ran();
    let sets = option_sets(agents.calls());
    assert_eq!(sets.len(), tried + 1, "set once taken: {sets:?}");
    assert_eq!(sets[tried][0], ["set-option", "-p", "-t", "%7", "@ekko_state", "working"]);
    session.ran();
    assert_eq!(option_sets(agents.calls()).len(), tried + 1, "set already: no call");
    fs::remove_dir_all(&agents.home).ok();
}

/// A turn over with background work in flight is no wait on the user (task
/// 1686): the work's end wakes the session itself, so idle_prompt marks
/// nothing, though a permission prompt still does; once a turn ends with
/// none, idle_prompt marks it.
#[test]
fn a_born_session_with_background_work_in_flight_waits_on_no_one_at_idle_prompt() {
    let agents = Agents::new();
    assert!(agents.ekko(&["--task", "Write hello"]).status.success());
    agents.name_window("1 \u{b7} haiku");
    let session = agents.session(&agents.uid(1));
    session.ekko(&["--begin", "1"], "");
    let stop = |background: serde_json::Value| session.hook("Stop", serde_json::json!({"background_tasks": background, "session_crons": []}));
    let idle = || session.hook("Notification", serde_json::json!({"notification_type": "idle_prompt", "message": "Claude is waiting for your input"}));
    stop(serde_json::json!([{"id": "b-1", "type": "shell", "status": "running"}]));
    idle();
    assert!(agents.logged("does not wait on you (idle_prompt): background work was in flight at its turn's end"));
    assert!(!agents.context("1").contains("waiting on you") && agents.window() == "1 \u{b7} haiku");
    session.asks();
    assert!(agents.logged("waits on you (permission_prompt)") && agents.context("1").contains("waiting on you since"));
    session.ran();
    assert!(!agents.context("1").contains("waiting on you"));
    // The pane's state left as it is, the turn's end still keeps whether
    // background work was in flight.
    fs::write(agents.home.join("mux-state/refuse"), "").unwrap();
    stop(serde_json::json!([]));
    idle();
    assert!(agents.logged("waits on you (idle_prompt): Claude is waiting for your input; marked on task 1"));
    assert!(agents.context("1").contains("waiting on you since"));
    fs::remove_dir_all(&agents.home).ok();
}

/// The wall (task 1677): `ekko agents wall` makes a window for it, joins
/// every born session that runs into it, tiled, takes away the window's own
/// pane, and titles each pane's border from its options; a dead pane and a
/// pane start did not open stay where they are, and a session already on
/// it is not moved again. Each joins after the last on the wall, so that
/// the wall keeps the order of their windows. `ekko agents unwall` breaks
/// each back out into a window named as start named it, which sheds every
/// option the wall set: start's status line, the rest unset. start refuses
/// a task whose session is on the wall, though it has no window of its own.
#[test]
fn the_wall_takes_every_running_born_session_and_unwall_gives_each_its_window_back() {
    let agents = Agents::new();
    for task in ["Write hello", "Write the docs", "Write the tests"] {
        assert!(agents.ekko(&["--task", task]).status.success());
    }
    let mux = agents.home.join("mux-state");
    fs::create_dir_all(&mux).unwrap();
    fs::write(mux.join("session"), "").unwrap();
    let panes = |one: &str, two: &str| {
        let lines = [
            format!("%1\t0\t{one}\t1 \u{b7} haiku\t1\tWrite hello\thaiku\tlow\tworking\t1\t\tapp"),
            format!("%2\t0\t{two}\t2 \u{b7} sonnet\t2\tWrite the docs\tsonnet\t\twaiting\t1\tClaude needs your permission\tapp"),
            "%3\t1\t@3\t3 \u{b7} haiku\t3\tWrite the tests\thaiku\t\tidle\t1\t\tapp".to_string(),
            "%4\t0\t@4\tzsh\t\t\t\t\t\t\t\t".to_string(),
        ];
        fs::write(mux.join("born-panes"), lines.join("\n") + "\n").unwrap();
    };
    let calls = |from: usize| -> Vec<Vec<String>> { agents.calls().into_iter().skip(from).collect() };
    panes("@1", "@2");
    let output = agents.ekko(&["agents", "wall"]);
    let said = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success() && said.trim() == "On the wall, window @9 of session ekko: 1 (%1), 2 (%2)", "{said}{}", String::from_utf8_lossy(&output.stderr));
    let commands: Vec<Vec<String>> = calls(0).into_iter().flat_map(|call| call.split(|arg| arg == ";").map(<[String]>::to_vec).collect::<Vec<_>>()).collect();
    let at = |command: &[&str]| commands.iter().position(|seen| seen == command).unwrap_or_else(|| panic!("{command:?} not in {commands:?}"));
    let made = at(&["new-window", "-d", "-t", "=ekko:", "-n", "wall", "-P", "-F", "#{window_id} #{pane_id}"]);
    let one = at(&["join-pane", "-d", "-s", "%1", "-t", "%9"]);
    let two = at(&["join-pane", "-d", "-s", "%2", "-t", "%1"]);
    let placeholder = at(&["kill-pane", "-t", "%9"]);
    let marked = at(&["set-option", "-w", "-t", "@9", "@ekko_wall", "1"]);
    assert!(made < one && one < two && two < placeholder && placeholder < marked, "{commands:?}");
    assert_eq!(commands.iter().filter(|command| command[0] == "join-pane").count(), 2, "{commands:?}");
    assert_eq!(commands.iter().filter(|command| command[..] == ["select-layout", "-t", "@9", "tiled"]).count(), 3, "{commands:?}");
    at(&["set-option", "-w", "-t", "@9", "pane-border-status", "top"]);
    let border = commands.iter().find(|command| command.starts_with(&["set-option".to_string(), "-w".into(), "-t".into(), "@9".into(), "pane-border-format".into()])).unwrap();
    for part in ["#{@ekko_tasks} \u{b7} #{@ekko_model} \u{b7} #{@ekko_title}", "\u{25cf} waiting on you", "\u{2699} working", "\u{25cb} idle"] {
        assert!(border[5].contains(part), "{part} in {border:?}");
    }
    // In the status line, a symbol for each pane on the wall.
    for option in ["window-status-format", "window-status-current-format"] {
        let status = commands.iter().find(|command| command.starts_with(&["set-option".to_string(), "-w".into(), "-t".into(), "@9".into(), option.into()])).unwrap();
        let (tab, plain) = status[5].strip_prefix("#{?#{@ztmux-tab-bar},").and_then(|rest| rest.split_once(",#I:#W #{P:")).unwrap_or_else(|| panic!("{status:?}"));
        for drawn in [tab, plain] {
            assert!(["\u{25cf}", "\u{2699}", "\u{25cb}"].iter().all(|symbol| drawn.contains(symbol)), "{drawn}");
        }
        assert!(tab.contains("colour108") && tab.contains(" #I #W #{P:"), "{tab}");
    }
    assert!(marked < at(&["select-window", "-t", "@9"]), "{commands:?}");
    let walled: Vec<String> = commands.iter().filter(|command| command.len() == 6 && command[..4] == ["set-option", "-w", "-t", "@9"]).map(|command| command[4].clone()).collect();

    // On the wall already: nothing moves.
    fs::write(mux.join("wall"), "").unwrap();
    panes("@9", "@9");
    let before = agents.calls().len();
    let said = String::from_utf8(agents.ekko(&["agents", "wall"]).stdout).unwrap();
    assert_eq!(said.trim(), "Every session is on the wall already, in window @9 of session ekko");
    assert!(calls(before).iter().all(|call| call[0] != "join-pane" && call[0] != "new-window"), "{:?}", calls(before));
    let border_status = |from: usize| -> Vec<String> {
        calls(from).into_iter().flat_map(|call| call.split(|arg| arg == ";").map(<[String]>::to_vec).collect::<Vec<_>>()).filter(|command| command.len() == 6 && command[4] == "pane-border-status").map(|command| command[5].clone()).collect()
    };
    assert_eq!(border_status(before), ["top"]);

    // With ztmux's frames on, which title each pane, no border's status
    // line: it would take the place of a pane's frame (task 1678).
    fs::write(mux.join("zellij"), "").unwrap();
    let before = agents.calls().len();
    assert!(agents.ekko(&["agents", "wall"]).status.success());
    assert_eq!(border_status(before), ["off"]);
    fs::remove_file(mux.join("zellij")).unwrap();

    // One started since joins the wall there is, after its last pane.
    let born = fs::read_to_string(mux.join("born-panes")).unwrap();
    fs::write(mux.join("born-panes"), format!("{born}%5\t0\t@5\t3 \u{b7} haiku\t3\tWrite the tests\thaiku\t\tworking\t1\t\tapp\n")).unwrap();
    let before = agents.calls().len();
    let said = String::from_utf8(agents.ekko(&["agents", "wall"]).stdout).unwrap();
    assert_eq!(said.trim(), "On the wall, window @9 of session ekko: 3 (%5)");
    let joins: Vec<Vec<String>> = calls(before).into_iter().flat_map(|call| call.split(|arg| arg == ";").map(<[String]>::to_vec).collect::<Vec<_>>()).filter(|command| command[0] == "join-pane" || command[0] == "new-window").collect();
    assert_eq!(joins, [["join-pane", "-d", "-s", "%5", "-t", "%2"].map(str::to_string).to_vec()]);
    fs::write(mux.join("born-panes"), born).unwrap();

    // start refuses a task whose session is on the wall.
    let refused = agents.ekko(&["agents", "start", "2", "--model", "haiku"]);
    let why = String::from_utf8_lossy(&refused.stderr).to_string() + &String::from_utf8_lossy(&refused.stdout);
    assert!(!refused.status.success() && why.contains("task 2 has a session already, in pane %2 of window 2 \u{b7} sonnet in session ekko"), "{why}");

    let before = agents.calls().len();
    let said = String::from_utf8(agents.ekko(&["agents", "unwall"]).stdout).unwrap();
    assert_eq!(said.trim(), "Back in windows of their own: 1 (%1), 2 (%2)");
    let commands: Vec<Vec<String>> = calls(before).into_iter().flat_map(|call| call.split(|arg| arg == ";").map(<[String]>::to_vec).collect::<Vec<_>>()).collect();
    let broken: Vec<&Vec<String>> = commands.iter().filter(|command| command[0] == "break-pane").collect();
    assert_eq!(broken, [&["break-pane", "-d", "-s", "%1", "-n", "1 \u{b7} haiku"].map(str::to_string).to_vec(), &["break-pane", "-d", "-s", "%2", "-n", "2 \u{b7} sonnet"].map(str::to_string).to_vec()]);
    // Every option the wall set, as the wall's window is the last pane's
    // once it is alone there: start's status line, or unset.
    assert!(walled.len() >= 5, "{walled:?}");
    for pane in ["%1", "%2"] {
        for option in &walled {
            let shed = commands.iter().any(|command| match &command[..] {
                [set, flag, t, target, name] => (set, flag, t, target, name) == (&"set-option".into(), &"-wu".into(), &"-t".into(), &pane.to_string(), option),
                // start's status line: the symbol before the window's name.
                [set, flag, t, target, name, value] => (set, flag, t, target, name) == (&"set-option".into(), &"-w".into(), &"-t".into(), &pane.to_string(), option) && ["\u{25cf} #W", "\u{2699} #W", "\u{25cb} #W"].iter().all(|symbol| value.contains(symbol)),
                _ => false,
            });
            assert!(shed, "{option} of {pane}'s window: {commands:?}");
        }
    }
    fs::remove_dir_all(&agents.home).ok();
}

/// A session waiting on the user while it is on the wall (task 1677) is
/// marked on the board, and drawn on its pane's border, but the wall,
/// whose name is every session's on it, is not renamed.
#[test]
fn a_born_session_on_the_wall_waits_without_renaming_the_wall() {
    let agents = Agents::new();
    assert!(agents.ekko(&["--task", "Write hello"]).status.success());
    agents.name_window("wall");
    fs::write(agents.home.join("mux-state/wall"), "").unwrap();
    let session = agents.session(&agents.uid(1));
    session.ekko(&["--begin", "1"], "");
    session.asks();
    assert!(agents.logged("marked on task 1; its window not renamed: it is on the wall, whose name is not its own"), "{:?}", fs::read_to_string(agents.home.join(".local/state/ekko/born/born.log")));
    assert!(agents.context("1").contains("waiting on you since"));
    session.ran();
    assert!(agents.logged("cleared on task 1; its window was not renamed"));
    assert_eq!(agents.window(), "wall");
    assert!(agents.calls().iter().all(|call| !call.contains(&"rename-window".to_string())));
    fs::remove_dir_all(&agents.home).ok();
}

/// `ekko agents view --once` (task 1676): the sessions the multiplexer
/// lists in the session ekko, by the state on their panes, titled from the
/// board, and below them the tasks born sessions finished today, by
/// born.log, each with its closing note's first line; a pane start did not
/// open is not shown.
#[test]
fn agents_view_shows_the_sessions_by_state_and_what_they_finished_today() {
    let agents = Agents::new();
    for task in ["Fix #3 in the parser\nwhere it reads dates", "Write the docs", "Review the plan", "Add the flag"] {
        assert!(agents.ekko(&["--task", task]).status.success());
    }
    assert!(agents.ekko(&["--check", "4"]).status.success());
    assert!(agents.ekko(&["--note", "Added --json to agents start\nand its test"]).status.success());
    assert!(agents.ekko(&["--attached-to", "@5", "4"]).status.success());
    let state = agents.home.join(".local/state/ekko/born");
    fs::create_dir_all(&state).unwrap();
    let now = chrono::Local::now();
    fs::write(state.join("born.log"), format!("{} aaaaaaaa {} closed: its Claude Code ended\n", now.format("%Y-%m-%dT%H:%M:%S%.3f"), agents.uid(4))).unwrap();
    let mux = agents.home.join("mux-state");
    fs::create_dir_all(&mux).unwrap();
    fs::write(mux.join("session"), "").unwrap();
    let since = now.timestamp();
    let panes = [
        format!("%1\t0\t@1\t1 \u{b7} haiku\t1\tFix ##3 in the parser\thaiku\tlow\twaiting\t{since}\tClaude needs your permission -- Bash: echo ##3 > stamp.txt\tapp"),
        format!("%2\t0\t@2\t2 \u{b7} sonnet\t2\tWrite the docs\tsonnet\t\tworking\t{since}\t\tapp"),
        format!("%3\t0\t@3\t3 \u{b7} haiku\t3\tReview the plan\thaiku\t\tidle\t{since}\t\tapp"),
        "%9\t0\t@9\tzsh\t\t\t\t\t\t\t\t".to_string(),
    ];
    fs::write(mux.join("born-panes"), panes.join("\n") + "\n").unwrap();
    let output = agents.ekko(&["agents", "view", "--once"]);
    let said = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{said}{}", String::from_utf8_lossy(&output.stderr));
    let at = now.format("%H:%M").to_string();
    let lines: Vec<&str> = said.lines().collect();
    assert!(lines[0].starts_with("ekko agents \u{b7} ") && lines[0].ends_with(" session ekko \u{b7} 3 running, 1 finished today"), "{said}");
    assert_eq!(
        lines[1..],
        [
            "",
            "\u{25cf} Waiting on you",
            &format!("  1  Fix #3 in the parser  haiku \u{b7} low  since {at}  Claude needs your permission -- Bash: echo #3 > stamp.txt"),
            "",
            "\u{2699} Working",
            &format!("  2  Write the docs        sonnet       since {at}"),
            "",
            "\u{25cb} Idle",
            &format!("  3  Review the plan       haiku        since {at}"),
            "",
            "\u{2713} Finished today",
            &format!("  4  Add the flag          done {at}                Added --json to agents start"),
        ],
        "{said}"
    );
    let calls = agents.calls();
    assert!(calls.iter().any(|call| call.contains(&"list-panes".to_string()) && call.contains(&"-s".to_string())), "{calls:?}");
    fs::remove_dir_all(&agents.home).ok();
}

/// `ekko agents view --json` (task 1861): the sessions of every project the
/// multiplexer lists, each read on the board of its project, with the tasks
/// finished today wherever their uids are, and the count of the other
/// Claude Code sessions running on the boards -- not the ones ekko agents
/// started -- all within a status line's time.
#[test]
fn agents_view_json_gives_every_project_s_sessions_and_the_count_of_the_others() {
    let agents = Agents::new();
    let lib = agents.home.join("lib");
    fs::create_dir_all(&lib).unwrap();
    assert!(agents.running(env!("CARGO_BIN_EXE_ekko"), &["init"]).current_dir(&lib).output().unwrap().status.success());
    let in_lib = |args: &[&str]| agents.running(env!("CARGO_BIN_EXE_ekko"), args).current_dir(&lib).output().unwrap();
    for task in ["Fix #3 in the parser", "Write the docs"] {
        assert!(agents.ekko(&["--task", task]).status.success());
    }
    for task in ["Cache the index", "Ship the flag"] {
        assert!(in_lib(&["--task", task]).status.success());
    }
    assert!(in_lib(&["--check", "2"]).status.success());
    let lib_uid = {
        let board: serde_json::Value = serde_json::from_str(&fs::read_to_string(lib.join(".ekko/storage/storage.json")).unwrap()).unwrap();
        board["2"]["uid"].as_str().unwrap().to_string()
    };
    let state = agents.home.join(".local/state/ekko");
    fs::create_dir_all(state.join("born")).unwrap();
    let now = chrono::Local::now();
    fs::write(state.join("born/born.log"), format!("{} aaaaaaaa {lib_uid} closed: its Claude Code ended\n", now.format("%Y-%m-%dT%H:%M:%S%.3f"))).unwrap();
    let mux = agents.home.join("mux-state");
    fs::create_dir_all(&mux).unwrap();
    fs::write(mux.join("session"), "").unwrap();
    let since = now.timestamp();
    let panes = [
        format!("%1\t0\t@1\t1 \u{b7} haiku\t1\tFix the parser\thaiku\tlow\twaiting\t{since}\tClaude needs your permission\tapp"),
        format!("%2\t0\t@2\t1 \u{b7} sonnet\t1\tCache the index\tsonnet\t\tworking\t{since}\t\tlib"),
        "%9\t0\t@9\tzsh\t\t\t\t\t\t\t\t".to_string(),
    ];
    fs::write(mux.join("born-panes"), panes.join("\n") + "\n").unwrap();
    // Three Claude Code processes the registry knows: one on each board,
    // and one of ekko agents' own, which is none of the others.
    let boot = fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap().trim().to_string();
    let children: Vec<process::Child> = (0..3)
        .map(|n| {
            let mut sleeping = common::command("sleep");
            sleeping.arg("60").env_remove("EKKO_AGENT_TASK");
            if n == 2 {
                sleeping.env("EKKO_AGENT_TASK", "uid-1");
            }
            sleeping.spawn().unwrap()
        })
        .collect();
    let processes = state.join("processes");
    fs::create_dir_all(&processes).unwrap();
    for (child, board) in children.iter().zip([agents.app.join(".ekko"), lib.join(".ekko"), agents.app.join(".ekko")]) {
        let stat = fs::read_to_string(format!("/proc/{}/stat", child.id())).unwrap();
        let start: u64 = stat[stat.rfind(')').unwrap() + 1..].split_whitespace().nth(19).unwrap().parse().unwrap();
        let record = serde_json::json!({
            "pid": child.id(), "start": start, "boot": boot, "board": fs::canonicalize(board).unwrap().join("storage/storage.json"),
            "conversation": format!("c-{}", child.id()), "since": 0,
        });
        let name: String = boot.chars().filter(char::is_ascii_alphanumeric).take(8).collect();
        fs::write(processes.join(format!("{name}-{}-{start}.json", child.id())), record.to_string()).unwrap();
    }
    let began = std::time::Instant::now();
    let output = agents.ekko(&["agents", "view", "--json"]);
    let took = began.elapsed();
    let said: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|e| panic!("{e}: {}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr)));
    for mut child in children {
        child.kill().ok();
        child.wait().ok();
    }
    assert!(output.status.success(), "{said}");
    assert!(took < std::time::Duration::from_secs(2), "a status line cannot wait {took:?} for this");
    let sessions = said["sessions"].as_array().unwrap();
    let shown: Vec<(&str, &str, &str, Option<&str>)> =
        sessions.iter().map(|s| (s["state"].as_str().unwrap(), s["project"].as_str().unwrap(), s["title"].as_str().unwrap(), s["pane"].as_str())).collect();
    assert_eq!(
        shown,
        [("waiting", "app", "Fix #3 in the parser", Some("%1")), ("working", "lib", "Cache the index", Some("%2")), ("done", "lib", "Ship the flag", None)],
        "{said}"
    );
    assert_eq!((sessions[0]["model"].as_str(), sessions[0]["effort"].as_str(), sessions[0]["since"].as_i64()), (Some("haiku"), Some("low"), Some(since)));
    assert_eq!(sessions[0]["tasks"], serde_json::json!([1]));
    assert_eq!(sessions[1]["effort"], serde_json::Value::Null);
    assert_eq!(said["counts"], serde_json::json!({"working": 1, "waiting": 1, "idle": 0, "done": 1}));
    assert_eq!(said["others"], serde_json::json!({"total": 2, "byProject": {"app": 1, "lib": 1}}), "{said}");
    fs::remove_dir_all(&agents.home).ok();
}

/// A permission prompt answered clears the mark then, not when the tool it
/// let run ends (task 1679): no hook fires as the user answers, so the hook
/// starts a watcher of Claude Code's own record of the session,
/// `sessions/<pid>.json` in its config folder, which clears the mark -- on
/// the board, in the window's name and on the pane -- once the record has
/// said waiting and then busy. A record that has not said waiting first, or
/// is another session's, clears nothing; the tool's end then has nothing
/// left to clear.
#[test]
fn a_permission_prompt_answered_clears_the_mark_as_claude_code_s_record_says_so() {
    let agents = Agents::new();
    assert!(agents.ekko(&["--task", "Write hello"]).status.success());
    agents.name_window("1 \u{b7} haiku");
    let session = agents.session(&agents.uid(1));
    session.ekko(&["--begin", "1"], "");
    let pid = session.child.id();
    let record = agents.home.join(".claude/sessions").join(format!("{pid}.json"));
    fs::create_dir_all(record.parent().unwrap()).unwrap();
    let says = |status: &str, of: &str| {
        let fresh = record.with_extension("new");
        fs::write(&fresh, serde_json::json!({"pid": pid, "sessionId": of, "status": status, "statusUpdatedAt": 1}).to_string()).unwrap();
        fs::rename(&fresh, &record).unwrap();
    };
    let marked = || agents.context("1").contains("waiting on you since");
    let still = |what: &str| {
        std::thread::sleep(std::time::Duration::from_millis(600));
        assert!(marked() && agents.window().ends_with("waiting on you"), "{what}: the mark went");
    };
    says("busy", "s-1");
    session.asks();
    assert!(marked(), "{}", agents.context("1"));
    still("busy before it said waiting");
    says("waiting", "s-1");
    still("waiting");
    says("busy", "s-2");
    still("another session's record");
    says("busy", "s-1");
    assert!(agents.logged("waits on you no more (answered: Claude Code's record says busy): cleared on task 1; its window named \"1 \u{b7} haiku\" again"), "{:?}", fs::read_to_string(agents.home.join(".local/state/ekko/born/born.log")));
    assert!(!marked() && agents.window() == "1 \u{b7} haiku", "{}", agents.context("1"));
    let states: Vec<String> = option_sets(agents.calls()).iter().flatten().filter(|command| command[4] == "@ekko_state").map(|command| command[5].clone()).collect();
    assert_eq!(states.last().map(String::as_str), Some("working"), "{states:?}");
    assert_eq!(states.iter().rev().nth(1).map(String::as_str), Some("waiting"), "{states:?}");
    let calls = agents.calls().len();
    session.ran();
    assert_eq!(agents.calls().len(), calls, "the tool's end found nothing left to clear");
    let log = fs::read_to_string(agents.home.join(".local/state/ekko/born/born.log")).unwrap();
    assert_eq!(log.matches("waits on you no more").count(), 1, "{log}");
    fs::remove_dir_all(&agents.home).ok();
}

/// The edges of waiting on the user (task 1645): a turn over while the
/// session waits on the board, through ekko's wait, is no wait on the user,
/// and marks nothing; a permission prompt still does. A window the user
/// renamed meanwhile keeps their name. A session holding none of its tasks
/// has its window renamed alone. And once its process has ended, its task
/// no longer says it waits on anyone.
#[test]
fn a_born_session_waiting_on_the_board_holding_nothing_or_gone_is_no_wait_on_the_user() {
    let agents = Agents::new();
    for text in ["Write hello", "Write the docs", "Write the tests"] {
        assert!(agents.ekko(&["--task", text]).status.success());
    }
    // Task 2, the session's too, is the user's to hold.
    assert!(agents.ekko(&["--begin", "2"]).status.success());
    agents.name_window("1,2 \u{b7} haiku");
    let session = agents.session(&format!("{},{}", agents.uid(1), agents.uid(2)));
    session.ekko(&["--begin", "1"], "");
    let lines = [
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}}}),
        serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "wait", "arguments": {"item": 3, "text": "check the tests once they are written"}}}),
    ];
    let said = session.ekko(&["--mcp"], &lines.map(|line| format!("{line}\n")).concat());
    assert!(said.contains("\"id\":2") && !said.contains("\"isError\":true"), "{said}");

    session.hook("Notification", serde_json::json!({"notification_type": "idle_prompt", "message": "Claude is waiting for your input"}));
    assert!(agents.logged("does not wait on you (idle_prompt): it waits on the board, through note 4"));
    assert!(!agents.context("1").contains("waiting on you"));
    assert_eq!(agents.window(), "1,2 \u{b7} haiku");

    session.asks();
    assert!(agents.logged("marked on task 1;"), "a permission prompt is the user's, whatever the session waits on");
    assert!(agents.context("1").contains("waiting on you since"));
    assert!(!agents.context("2").contains("waiting on you"), "the user's own claim was marked");
    agents.name_window("mine");
    session.hook("UserPromptSubmit", serde_json::json!({"prompt": "go on"}));
    assert!(agents.logged("waits on you no more (UserPromptSubmit): cleared on task 1; its window kept the name it was given meanwhile, \"mine\""));
    assert_eq!(agents.window(), "mine");

    agents.name_window("1 \u{b7} haiku");
    assert!(agents.ekko(&["--check", "1"]).status.success());
    session.asks();
    assert!(agents.logged("no task of its own in progress to mark on the board; its window named \"1 \u{b7} haiku \u{b7} waiting on you\""));
    session.ran();
    assert!(agents.logged("waits on you no more (PostToolUse): nothing to clear on the board; its window named \"1 \u{b7} haiku\" again"));

    assert!(agents.ekko(&["--check", "1"]).status.success(), "reopened");
    session.ekko(&["--begin", "1"], "");
    session.asks();
    assert!(agents.context("1").contains("waiting on you since"));
    drop(session);
    let context = agents.context("1");
    assert!(context.contains("task, in progress, held by ") && context.contains(", gone since "), "{context}");
    assert!(!context.contains("waiting on you"), "a session that has ended waits on no one: {context}");
    assert!(!String::from_utf8(agents.ekko(&[]).stdout).unwrap().contains("waiting on you"));
    fs::remove_dir_all(&agents.home).ok();
}

/// The born hook's runs for one session take turns (task 1645): Claude Code
/// fires PostToolUse for tools that ran side by side at once, and a prompt
/// typed while they run, and each run reads what it keeps of the session
/// and writes it back -- a run that wrote back what it read before
/// another's write would lose the prompt that one counted, which keeps a
/// session the user talks to. In a loop, as a race wants.
#[test]
fn the_born_hook_counts_every_prompt_while_tools_end_beside_it() {
    let agents = Agents::new();
    assert!(agents.ekko(&["--task", "Write hello"]).status.success());
    let uid = agents.uid(1);
    agents.name_window("1 \u{b7} haiku");
    let kept = agents.home.join(".local/state/ekko/born/s-1.json");
    fs::create_dir_all(kept.parent().unwrap()).unwrap();
    for round in 1..=10 {
        let mut born: serde_json::Value = fs::read_to_string(&kept).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or(serde_json::json!({"prompts": 0, "talked": false}));
        born["waiting"] = serde_json::json!({"since": 0, "what": "Claude needs your permission to use Bash", "window": "1 \u{b7} haiku \u{b7} waiting on you"});
        fs::write(&kept, born.to_string()).unwrap();
        let mut runs = vec![agents.born_start(Some(&uid), "s-1", "UserPromptSubmit", serde_json::json!({"prompt": "go on"}))];
        for _ in 0..4 {
            runs.push(agents.born_start(Some(&uid), "s-1", "PostToolUse", serde_json::json!({"tool_name": "Read", "tool_input": {}})));
        }
        for run in &mut runs {
            let mut said = String::new();
            run.stdout.take().unwrap().read_to_string(&mut said).unwrap();
            run.kill().ok();
            run.wait().ok();
        }
        let born: serde_json::Value = serde_json::from_str(&fs::read_to_string(&kept).unwrap()).unwrap();
        assert_eq!(born["prompts"], round, "round {round}: {born}");
        assert!(born.get("waiting").is_none(), "round {round}: {born}");
    }
    fs::remove_dir_all(&agents.home).ok();
}

/// What a session ekko agents started leaves to the orchestrator and the
/// user (tasks 1646, 1716 and 1742), each kind in the forms the guard reads,
/// beside calls that only name one or look like one: the guard refuses each
/// there, and in the orchestrator -- a session ekko agents did not start --
/// none. And at the terminal, a write to a task the session was not born
/// for, or to the board's phases: refused there, made by the orchestrator.
#[test]
fn a_born_session_is_refused_a_push_a_release_a_session_and_another_s_task_and_its_orchestrator_none() {
    use std::io::Write as _;
    const PUSH: &str = "does not push or open a pull request";
    const RELEASE: &str = "does not release or deploy";
    const SESSION: &str = "does not start sessions";
    const STOP: &str = "does not stop, respawn or remove Claude Code's sessions";
    const KILL: &str = "does not kill or respawn the multiplexer's panes, windows, sessions or server";
    const SIGNAL: &str = "does not signal processes with kill, pkill or killall";
    const RUNS: &str = "does not have the multiplexer run a command for it";
    const KEYS: &str = "does not type into another pane than its own";
    const VERB: &str = "does not run what the multiplexer reads as no tmux command";
    let agents = Agents::new();
    // The stand-in multiplexer, which the guard's EKKO_MUX names, as the
    // session's names the one `start` used.
    let ours = format!("{} -L ekko kill-pane -t %3", agents.mux.display());
    let refused: &[(&str, &[&str])] = &[
        ("git push", &[PUSH]),
        ("git push origin task-7", &[PUSH]),
        ("git -C ../app push --force-with-lease", &[PUSH]),
        ("git -c push.default=current push", &[PUSH]),
        ("cd .. && git push", &[PUSH]),
        ("nix develop -c git push origin main", &[PUSH]),
        ("timeout 60 git push", &[PUSH]),
        ("bash -c 'git push'", &[PUSH]),
        ("gh pr create --fill", &[PUSH]),
        ("gh pr merge 5 --squash -R thiagoproldan/ekko", &[PUSH]),
        ("gh release create v1.0.0 --notes done", &[RELEASE]),
        ("gh release upload v1.0.0 ekko.tar.gz", &[RELEASE]),
        ("cargo publish", &[RELEASE]),
        ("cargo +nightly publish --dry-run", &[RELEASE]),
        ("npm publish", &[RELEASE]),
        ("yarn npm publish", &[RELEASE]),
        ("sudo -n nixos-rebuild switch --flake /home/roldant/NixOS", &[RELEASE]),
        ("nixos-rebuild boot", &[RELEASE]),
        ("scripts/release.sh --trailer 'Ekko: 1' 1.2.3 notes.md", &[RELEASE]),
        ("bash scripts/release.sh 1.2.3 notes.md", &[RELEASE]),
        ("ekko agents start 5", &[SESSION]),
        ("ekko --json agents start 5 --model haiku", &[SESSION]),
        ("ekko agents merge 1", &[PUSH]),
        ("ekko --json agents merge 1 2 --keep", &[PUSH]),
        ("claude", &[SESSION]),
        ("claude -p 'review the diff'", &[SESSION]),
        ("claude --bg 'write the docs'", &[SESSION]),
        ("claude --model haiku 'write it'", &[SESSION]),
        ("claude attach 4f2a", &[SESSION]),
        ("claude --resume", &[SESSION]),
        ("claude ultrareview", &[SESSION]),
        ("cargo test && git push && claude -p 'go on'", &[PUSH, SESSION]),
        ("git push && gh pr create --fill", &[PUSH]),
        ("claude stop 4f2a", &[STOP]),
        ("claude kill 4f2a", &[STOP]),
        ("claude rm 4f2a", &[STOP]),
        ("claude respawn --all", &[STOP]),
        ("claude --settings /tmp/settings.json stop 4f2a", &[STOP]),
        ("tmux kill-server", &[KILL]),
        ("tmux -L ekko kill-session -t ekko", &[KILL]),
        ("tmux kill-window -t ekko:3", &[KILL]),
        ("tmux kill-pane -t %3", &[KILL]),
        ("tmux respawn-pane -k -t %3", &[KILL]),
        ("tmux respawn-window -k -t ekko:3", &[KILL]),
        ("tmux killw -t ekko:3", &[KILL]),
        ("tmux kill-ser", &[KILL]),
        ("kmux -L ekko kill-session -t ekko", &[KILL]),
        ("ztmux kill-server", &[KILL]),
        (&ours, &[KILL]),
        ("tmux new-window -d \\; kill-pane -t %3", &[KILL]),
        ("tmux display -p done\\; respawnp -k", &[KILL]),
        ("timeout 5 tmux kill-server", &[KILL]),
        ("bash -c 'kmux kill-server'", &[KILL]),
        ("tmux kill-session -t ekko && claude stop 4f2a && claude -p 'again'", &[SESSION, STOP, KILL]),
        ("claude purge", &[STOP]),
        ("claude purge --all -y", &[STOP]),
        ("kill 4242", &[SIGNAL]),
        ("kill -9 -1", &[SIGNAL]),
        ("kill -0 4242; kill 4242", &[SIGNAL]),
        ("/bin/kill -TERM 4242", &[SIGNAL]),
        ("pkill -f claude", &[SIGNAL]),
        ("killall claude", &[SIGNAL]),
        ("pgrep claude | xargs kill", &[SIGNAL]),
        ("sh -c 'pkill -f claude'", &[SIGNAL]),
        ("tmux run-shell 'kill 4242'", &[RUNS]),
        ("tmux if-shell true kill-server", &[RUNS]),
        ("tmux confirm-before -p 'go?' kill-server", &[RUNS]),
        ("tmux bind-key -n F1 kill-server", &[RUNS]),
        ("tmux set-hook -g pane-died kill-server", &[RUNS]),
        ("tmux set -g after-new-window kill-server", &[RUNS]),
        ("tmux display -p '#(pkill claude)'", &[RUNS]),
        ("tmux new-window -d 'claude -p go'", &[RUNS]),
        ("tmux split-window 'sleep 1'", &[RUNS]),
        ("tmux source-file /tmp/kill.conf", &[RUNS]),
        ("tmux -c 'pkill claude'", &[RUNS]),
        ("kmux znative load owner/repo", &[RUNS]),
        ("tmux send-keys -X copy-pipe sh", &[RUNS]),
        ("tmux new-window -k -t ekko:3", &[KILL]),
        ("tmux neww -dk -t ekko:3", &[KILL]),
        ("tmux link-window -k -s app:1 -t ekko:3", &[KILL]),
        ("tmux move-window -k -t ekko:3", &[KILL]),
        ("tmux unlink-window -k -t ekko:3", &[KILL]),
        ("tmux send-keys -t %3 /exit Enter", &[KEYS]),
        ("tmux send -t ekko:3 C-c", &[KEYS]),
        ("tmux paste-buffer -t %3", &[KEYS]),
        ("tmux send-prefix -t %3", &[KEYS]),
        ("tmux -L other send-keys x", &[KEYS]),
        ("kmux revive", &[VERB]),
        ("kmux -L ekko bcast 'echo hi'", &[VERB]),
        ("tmux info", &[VERB]),
        ("kill 4242; tmux send-keys -t %3 C-c && tmux run-shell true", &[SIGNAL, RUNS, KEYS]),
    ];
    let let_through = [
        "git status",
        "git commit -m 'push it later'",
        "git log --oneline origin/main..HEAD",
        "echo git push",
        "printf '%s\\n' 'cargo publish'",
        "cat <<'EOF'\ngit push\nEOF",
        "gh pr view 5",
        "gh release view v1.0.0",
        "cargo build --release",
        "npm run build",
        "nixos-rebuild build --flake .",
        "ls scripts/release.sh",
        "claude --version",
        "claude mcp list",
        "claude --settings /tmp/settings.json plugin list",
        "ekko agents --help",
        "ekko agents test",
        "ekko --list pending",
        "grep -rn 'git push' docs",
        "claude stop --help",
        "claude logs 4f2a",
        "tmux list-panes -a -F '#{pane_id}'",
        "tmux new-window -d -n build",
        "kmux -L ekko display -p 'kill-pane'",
        "tmux display -p 'done\\;' kill-pane",
        "echo tmux kill-server",
        "git commit -m 'tmux kill-server, then claude stop'",
        "grep -rn kill-pane src",
        "claude purge --dry-run",
        "kill -l",
        "kill -0 4242",
        "sleep 1 & kill $!",
        "sh -c 'kill -ABRT $$'",
        "kill -l 9",
        "pkill --help",
        "pgrep -f claude",
        "echo kill 4242",
        "tmux send-keys -t %7 'echo hi' Enter",
        "tmux send-keys x",
        "tmux new-window -d -n k",
        "tmux display -p '#{pane_id}'",
        "tmux set -g status-style bg=red",
        "tmux set-hook -gu pane-died",
        "tmux list-commands run-shell",
    ];
    for text in ["Write hello", "Write the docs"] {
        assert!(agents.ekko(&["--task", text]).status.success());
    }
    let uid = agents.uid(1);
    let event = |command: &str| serde_json::json!({"session_id": "s-1", "hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": {"command": command, "description": "Run it"}, "cwd": agents.app, "tool_use_id": "toolu_1"}).to_string();
    let guard = |args: &[&str], command: &str, born: Option<&str>| -> String {
        let mut guard = agents.command(args);
        // The pane the session runs in, as the multiplexer sets it.
        guard.env("TMUX_PANE", "%7");
        if let Some(born) = born {
            guard.env("EKKO_AGENT_TASK", born);
        }
        let mut child = guard.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
        child.stdin.take().unwrap().write_all(event(command).as_bytes()).unwrap();
        String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap()
    };
    let hook = |command: &str, born: Option<&str>| guard(&["--guard", "--hook"], command, born);
    for (command, kinds) in refused {
        let reply: serde_json::Value = serde_json::from_str(&hook(command, Some(&uid))).unwrap_or_else(|_| panic!("not refused: {command}"));
        let output = &reply["hookSpecificOutput"];
        let reason = output["permissionDecisionReason"].as_str().unwrap();
        assert_eq!(output["permissionDecision"], "deny", "{command}");
        assert!(reason.starts_with("ekko agents: this session, started for its tasks, "), "{command}: {reason}");
        for kind in [PUSH, RELEASE, SESSION, STOP, KILL, SIGNAL, RUNS, KEYS, VERB] {
            assert_eq!(reason.matches(kind).count(), usize::from(kinds.contains(&kind)), "{command}: {reason}");
        }
        assert!(reason.contains("The orchestrator asks the user through ekko's ask with allow set to \""), "{command}: {reason}");
        assert_eq!(hook(command, None), "", "the orchestrator's {command}");
    }
    for command in let_through {
        assert_eq!(hook(command, Some(&uid)), "", "{command}");
        assert_eq!(hook(command, None), "", "the orchestrator's {command}");
    }
    // Another guard refusing a call carries the limit's reason too: calls
    // of their own, since this test's two sessions are one to the guard.
    let other = |command: &str, born: Option<&str>| guard(&["--guard", "--refuse", "ctx: it would lose work"], command, born);
    let born_other = other("git push --tags", Some(&uid));
    assert!(born_other.contains("ekko agents: this session, started for its tasks, does not push"), "{born_other}");
    let orchestrator_other = other("git push --dry-run", None);
    assert!(orchestrator_other.contains("allow set to") && !orchestrator_other.contains("ekko agents"), "{orchestrator_other}");

    assert!(agents.ekko(&["--phases", "build"]).status.success(), "the user declares the phases");
    let born = agents.session(&uid);
    let branch = born.ekko(&["--guard", "--hook"], &event("git checkout -b task-2"));
    assert!(branch.contains("ekko: branch task-2 did not claim task 2: Task 2 is not this session's to change"), "{branch}");
    let theirs = born.ekko(&["--json", "--check", "2"], "");
    let theirs: serde_json::Value = serde_json::from_str(&theirs).unwrap_or_else(|_| panic!("{theirs}"));
    assert_eq!(theirs["code"], "NOT_BORN_FOR", "{theirs}");
    assert_eq!(theirs["notBornFor"], serde_json::json!({"ids": [2], "born": [1]}), "{theirs}");
    assert!(theirs["error"].as_str().unwrap().starts_with("Task 2 is not this session's to change: ekko agents started it for task 1"), "{theirs}");
    assert!(born.ekko(&["--json", "--begin", "1"], "").contains("\"ok\":true"));
    assert!(born.ekko(&["--json", "--task", "Found on the way"], "").contains("\"ok\":true"));
    assert!(born.ekko(&["--json", "--star", "3"], "").contains("\"ok\":true"), "a task it made is its own");
    assert!(born.ekko(&["--json", "--note", "What 2 needs"], "").contains("\"ok\":true"));
    assert!(born.ekko(&["--json", "--star", "2"], "").contains("NOT_BORN_FOR"));
    // The board's phases (task 1716): a change is refused; the sequence the
    // board has, and a declared phase on a task of its own, are free.
    let phases = born.ekko(&["--json", "--phases", "build", "ship"], "");
    let phases: serde_json::Value = serde_json::from_str(&phases).unwrap_or_else(|_| panic!("{phases}"));
    assert_eq!(phases["code"], "NOT_BORN_FOR", "{phases}");
    assert_eq!(phases["notBornFor"], serde_json::json!({"phases": true, "born": [1]}), "{phases}");
    let said = phases["error"].as_str().unwrap();
    assert!(said.starts_with("The board's phases are not this session's to change: ekko agents started it for task 1,"), "{said}");
    assert!(born.ekko(&["--json", "--phases", "build"], "").contains("\"ok\":true"));
    assert!(born.ekko(&["--json", "--phase", "build", "--task", "Built on the way"], "").contains("\"ok\":true"));
    assert_eq!(fs::read_to_string(agents.home.join("app/.ekko/storage/phases.json")).unwrap().matches('"').count(), 2, "build alone");
    // The test its branch passes before it goes in (task 1714): the user's
    // to set, as the phases are.
    let test = born.ekko(&["--json", "agents", "test", "true"], "");
    let test: serde_json::Value = serde_json::from_str(&test).unwrap_or_else(|_| panic!("{test}"));
    assert_eq!((test["code"].as_str(), test["notBornFor"].clone()), (Some("NOT_BORN_FOR"), serde_json::json!({"tests": true, "born": [1]})), "{test}");
    let said = test["error"].as_str().unwrap();
    assert!(said.starts_with("The project's test command is not this session's to set: ekko agents started it for task 1,"), "{said}");
    assert!(born.ekko(&["--json", "agents", "test", "--none"], "").contains("NOT_BORN_FOR"));
    // How many sessions run at once (task 1727): the user's too.
    let limit = born.ekko(&["--json", "agents", "limit", "5"], "");
    let limit: serde_json::Value = serde_json::from_str(&limit).unwrap_or_else(|_| panic!("{limit}"));
    assert_eq!((limit["code"].as_str(), limit["notBornFor"].clone()), (Some("NOT_BORN_FOR"), serde_json::json!({"limit": true, "born": [1]})), "{limit}");
    let said = limit["error"].as_str().unwrap();
    assert!(said.starts_with("The project's limit on sessions at once is not this session's to set: ekko agents started it for task 1, and how many sessions run at once is the user's to set"), "{said}");
    assert!(born.ekko(&["--json", "agents", "limit"], "").contains("\"limit\":2"), "read");
    // What every session is told in its first prompt (task 1927): the user's too.
    let brief = born.ekko(&["--json", "agents", "brief", "Use graff first"], "");
    let brief: serde_json::Value = serde_json::from_str(&brief).unwrap_or_else(|_| panic!("{brief}"));
    assert_eq!((brief["code"].as_str(), brief["notBornFor"].clone()), (Some("NOT_BORN_FOR"), serde_json::json!({"brief": true, "born": [1]})), "{brief}");
    assert!(brief["error"].as_str().unwrap().starts_with("The project's brief for its sessions is not this session's to set: ekko agents started it for task 1,"), "{brief}");
    assert!(born.ekko(&["--json", "agents", "brief"], "").contains("\"ok\":true"), "read");
    assert!(!agents.home.join("app/.ekko/storage/agents.json").exists());
    // The machine's default (task 1902), and the gate ekko derives: the user's too.
    assert!(born.ekko(&["--json", "agents", "limit", "5", "--default"], "").contains("NOT_BORN_FOR"));
    assert!(!fs::read_to_string(agents.home.join(".ekko.json")).unwrap().contains("agentsLimit"));
    fs::write(agents.home.join("app/Cargo.toml"), "").unwrap();
    assert!(born.ekko(&["--json", "agents", "test", "--derive"], "").contains("NOT_BORN_FOR"));
    fs::remove_file(agents.home.join("app/Cargo.toml")).unwrap();
    assert!(!agents.home.join("app/.ekko/storage/agents.json").exists());
    assert!(born.ekko(&["--json", "agents", "test"], "").contains("\"ok\":true"), "read");
    // The sessions' variables and the paths copied into their worktrees
    // (task 1828): the user's too, and read freely.
    let env = born.ekko(&["--json", "agents", "env", "CACHE={dir}/c"], "");
    let env: serde_json::Value = serde_json::from_str(&env).unwrap_or_else(|_| panic!("{env}"));
    assert_eq!((env["code"].as_str(), env["notBornFor"].clone()), (Some("NOT_BORN_FOR"), serde_json::json!({"env": true, "born": [1]})), "{env}");
    assert!(env["error"].as_str().unwrap().starts_with("The project's environment of the sessions is not this session's to set"), "{env}");
    let copy = born.ekko(&["--json", "agents", "copy", "private"], "");
    let copy: serde_json::Value = serde_json::from_str(&copy).unwrap_or_else(|_| panic!("{copy}"));
    assert_eq!((copy["code"].as_str(), copy["notBornFor"].clone()), (Some("NOT_BORN_FOR"), serde_json::json!({"copy": true, "born": [1]})), "{copy}");
    assert!(born.ekko(&["--json", "agents", "env"], "").contains("\"ok\":true"), "read");
    assert!(born.ekko(&["--json", "agents", "copy"], "").contains("\"ok\":true"), "read");
    assert!(!agents.home.join("app/.ekko/storage/agents.json").exists());
    let board = || -> serde_json::Value { serde_json::from_str(&fs::read_to_string(agents.home.join("app/.ekko/storage/storage.json")).unwrap()).unwrap() };
    let second = board()["2"].clone();
    assert_eq!((second["isComplete"].as_bool(), second["isStarred"].as_bool(), second["inProgress"].as_bool()), (Some(false), Some(false), Some(false)), "{second}");
    drop(born);
    // The user, whatever the terminal they type in was started with, writes
    // any task -- one a session made too -- and the phases.
    let mut typed = agents.command(&["--json", "--star", "3"]);
    assert!(String::from_utf8(typed.env("EKKO_AGENT_TASK", &uid).output().unwrap().stdout).unwrap().contains("\"ok\":true"));
    assert_eq!(board()["3"]["isStarred"], false, "starred by the session, and by the user no more");
    let mut typed = agents.command(&["--json", "--phases", "build", "test"]);
    assert!(String::from_utf8(typed.env("EKKO_AGENT_TASK", &uid).output().unwrap().stdout).unwrap().contains("\"ok\":true"), "the user's phases");
    let mut typed = agents.command(&["--json", "agents", "test", "cargo test"]);
    assert!(String::from_utf8(typed.env("EKKO_AGENT_TASK", &uid).output().unwrap().stdout).unwrap().contains("\"ok\":true"), "the user's test");
    let mut typed = agents.command(&["--json", "agents", "limit", "3"]);
    assert!(String::from_utf8(typed.env("EKKO_AGENT_TASK", &uid).output().unwrap().stdout).unwrap().contains("\"ok\":true"), "the user's limit");
    let born = agents.session(&uid);
    assert!(born.ekko(&["--json", "agents", "test", "cargo test"], "").contains("\"ok\":true"), "the test it has, unchanged");
    assert!(born.ekko(&["--json", "agents", "limit", "3"], "").contains("\"ok\":true"), "the limit it has, unchanged");
    drop(born);

    let orchestrator = agents.orchestrator();
    let branch = orchestrator.ekko(&["--guard", "--hook"], &event("git checkout -b task-2"));
    assert!(branch.contains("ekko: task 2 is in progress now, held by this session: creating branch task-2 claimed it"), "{branch}");
    assert!(orchestrator.ekko(&["--json", "--check", "2"], "").contains("\"ok\":true"));
    assert_eq!(board()["2"]["isComplete"], true, "{}", board()["2"]);
    assert!(orchestrator.ekko(&["--json", "--phases", "build", "ship"], "").contains("\"ok\":true"));
    assert!(orchestrator.ekko(&["--json", "agents", "test", "--none"], "").contains("\"ok\":true"), "the orchestrator, at the user's word");
    assert!(orchestrator.ekko(&["--json", "agents", "limit", "4"], "").contains("\"ok\":true"), "the orchestrator, at the user's word");
    drop(orchestrator);
    fs::remove_dir_all(&agents.home).ok();
}

/// One session `ekko` holds every project's windows, and the wall moves only
/// this project's panes: `wall` joins those of this project, and those that
/// name none, and leaves another's where they are, and `unwall` breaks out
/// only the same ones (task 1806).
#[test]
fn the_wall_and_unwall_move_only_this_projects_panes() {
    let agents = Agents::new();
    for task in ["Write hello", "Write the docs", "Write the tests"] {
        assert!(agents.ekko(&["--task", task]).status.success());
    }
    let mux = agents.home.join("mux-state");
    fs::create_dir_all(&mux).unwrap();
    fs::write(mux.join("session"), "").unwrap();
    let lay = |window: &str| {
        let lines = [
            format!("%1\t0\t{window}\t1 \u{b7} haiku\t1\tWrite hello\thaiku\t\tworking\t1\t\tapp"),
            format!("%2\t0\t{window}\t1 \u{b7} haiku\t1\tOther's task\thaiku\t\tworking\t1\t\tother"),
            format!("%3\t0\t{window}\t3 \u{b7} haiku\t3\tWrite the tests\thaiku\t\tidle\t1\t\t"),
        ];
        fs::write(mux.join("born-panes"), lines.join("\n") + "\n").unwrap();
    };
    let commands = |from: usize| -> Vec<Vec<String>> {
        agents.calls().into_iter().skip(from).flat_map(|call| call.split(|arg| arg == ";").map(<[String]>::to_vec).collect::<Vec<_>>()).collect()
    };
    let moved = |commands: &[Vec<String>], verb: &str| -> Vec<String> {
        commands.iter().filter(|command| command[0] == verb).map(|command| command[3].clone()).collect()
    };

    lay("@1");
    let said = String::from_utf8(agents.ekko(&["agents", "wall"]).stdout).unwrap();
    assert_eq!(said.trim(), "On the wall, window @9 of session ekko: 1 (%1), 3 (%3)");
    assert_eq!(moved(&commands(0), "join-pane"), ["%1", "%3"]);

    // Only another project's panes: this project has none to move.
    fs::write(mux.join("born-panes"), "%2\t0\t@2\t1 \u{b7} haiku\t1\tOther's task\thaiku\t\tworking\t1\t\tother\n").unwrap();
    let before = agents.calls().len();
    let output = agents.ekko(&["agents", "wall"]);
    let why = String::from_utf8_lossy(&output.stderr).to_string() + &String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success() && why.contains("for the project app"), "{why}");
    assert!(moved(&commands(before), "join-pane").is_empty());

    // On the wall: only this project's come back out.
    fs::write(mux.join("wall"), "").unwrap();
    lay("@9");
    let before = agents.calls().len();
    let said = String::from_utf8(agents.ekko(&["agents", "unwall"]).stdout).unwrap();
    assert_eq!(said.trim(), "Back in windows of their own: 1 (%1), 3 (%3)");
    let broken = moved(&commands(before), "break-pane");
    assert_eq!(broken, ["%1", "%3"]);
    fs::remove_dir_all(&agents.home).ok();
}

/// A born session keeps to its worktree and branch (task 1800), the cases
/// measured with ekko 0.42.0: from its worktree, a commit in the project's
/// own checkout, by `-C` or a `cd`, a branch moved by `branch -f` or
/// `update-ref`, and a branch made, are refused as a push is; each is the
/// orchestrator's to make. Its commit in its worktree and a read of the
/// project's checkout go through, and an Edit or Write of a file there is
/// refused, while one in its worktree or outside the repository is not.
/// So is a Bash call that writes a file there without git (task 1925),
/// and the stash, with what to do instead (task 1833).
#[test]
fn a_born_session_is_refused_a_write_outside_its_worktree_and_branch_and_its_orchestrator_none() {
    use std::io::Write as _;
    const SHARED: &str = "does not leave its own branch, nor write what every checkout of its repository shares";
    const CHECKOUT: &str = "does not write in another checkout of its repository";
    const PUSH: &str = "does not push or open a pull request";
    const STASH: &str = "does not write the stash";
    let agents = Agents::new();
    assert!(agents.ekko(&["--task", "Write hello"]).status.success());
    let uid = agents.uid(1);
    let worktree = agents.worktree("task-1");
    agents.git(&agents.app, &["worktree", "add", "-q", "-b", "task-1", worktree.to_str().unwrap()]);
    let app = fs::canonicalize(&agents.app).unwrap();
    let hook = |tool: &str, input: serde_json::Value, born: bool| -> String {
        let event = serde_json::json!({"session_id": "s-1", "hook_event_name": "PreToolUse", "tool_name": tool, "tool_input": input, "cwd": worktree, "tool_use_id": "toolu_1"});
        let mut guard = agents.command(&["--guard", "--hook"]);
        guard.current_dir(&worktree).env("TMUX_PANE", "%7");
        if born {
            guard.env("EKKO_AGENT_TASK", &uid).env("EKKO_AGENT_WORKTREE", &worktree).env("EKKO_AGENT_BRANCH", "task-1");
        }
        let mut child = guard.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
        child.stdin.take().unwrap().write_all(event.to_string().as_bytes()).unwrap();
        String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap()
    };
    let bash = |command: &str, born: bool| hook("Bash", serde_json::json!({"command": command, "description": "Run it"}), born);
    let refused = [
        ("git push origin task-1".to_string(), PUSH),
        (format!("git -C {} commit -am x", app.display()), CHECKOUT),
        (format!("cd {} && git merge task-1", app.display()), CHECKOUT),
        ("git branch -f main HEAD".to_string(), SHARED),
        ("git update-ref refs/heads/main HEAD".to_string(), SHARED),
        ("git checkout -b other".to_string(), SHARED),
        ("git stash push -u -m x".to_string(), STASH),
        (format!("echo y > {}/readme.md", app.display()), CHECKOUT),
        (format!("sed -i s/a/b/ {}/readme.md", app.display()), CHECKOUT),
        (format!("cd {} && rm -f readme.md", app.display()), CHECKOUT),
    ];
    for (command, kind) in &refused {
        let reply: serde_json::Value = serde_json::from_str(&bash(command, true)).unwrap_or_else(|_| panic!("not refused: {command}"));
        let output = &reply["hookSpecificOutput"];
        let reason = output["permissionDecisionReason"].as_str().unwrap();
        assert_eq!(output["permissionDecision"], "deny", "{command}");
        for other in [SHARED, CHECKOUT, PUSH, STASH] {
            assert_eq!(reason.matches(other).count(), usize::from(other == *kind), "{command}: {reason}");
        }
        assert_eq!(bash(command, false), "", "the orchestrator's {command}");
    }
    for command in ["git commit -am x".to_string(), format!("cd {} && git log --oneline", app.display()), "git status".to_string(), "echo y > readme.md".to_string(), format!("echo y > {}", agents.home.join("scratch.md").display())] {
        assert_eq!(bash(&command, true), "", "{command}");
    }
    let scratch = agents.home.join("scratch.md");
    for tool in ["Edit", "Write"] {
        let input = |path: &std::path::Path| serde_json::json!({"file_path": path, "content": "x", "old_string": "a", "new_string": "b"});
        let reply: serde_json::Value = serde_json::from_str(&hook(tool, input(&app.join("readme.md")), true)).unwrap_or_else(|_| panic!("{tool} not refused"));
        assert_eq!(reply["hookSpecificOutput"]["permissionDecision"], "deny", "{reply}");
        assert!(reply["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap().contains(CHECKOUT), "{reply}");
        assert_eq!(hook(tool, input(&app.join("readme.md")), false), "", "the orchestrator's {tool}");
        assert_eq!(hook(tool, input(&worktree.join("readme.md")), true), "", "{tool} in its worktree");
        assert_eq!(hook(tool, input(&scratch), true), "", "{tool} outside the repository");
    }
    let notebook = serde_json::json!({"notebook_path": app.join("a.ipynb"), "new_source": "x"});
    assert!(hook("NotebookEdit", notebook, true).contains("\"deny\""), "NotebookEdit in the project's checkout");
}
