mod agent;
mod anchors;
mod artifact;
mod cli;
mod commits;
mod config;
mod dialog;
mod directory;
mod docs;
mod doctor;
mod ekko;
mod feedback;
mod guard;
mod holder;
mod item;
mod json;
mod json_output;
mod lexical;
mod mcp;
mod memory;
mod menu;
mod move_to;
mod ops;
mod project;
mod paths;
mod render;
mod repeats;
mod serve;
mod shell;
mod storage;
mod tasklist;
mod wake;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;

use ekko::{Ekko, EkkoError, Outcome};
use render::{Painter, Renderer};

const HELP: &str = r#"
  Usage
    $ ekko [<options> ...]
    $ ekko init [<folder>] [--name <name>]
    $ ekko docs [<folder>] [--project <name>]
    $ ekko artifact <id> [--project <name>] [--no-open]
    $ ekko serve [--idle <seconds>] [--stop]

    Options
        none              Display board view
      --answer <ID>       Answer a question asked on the board; the id alone opens a menu
      --archive, -a       Display archived items
      --attached-to <IDS> Attach a note to the task it explains
      --begin, -b         Start/pause task
      --blocked-by <IDS>  Record what an item is blocked by
      --check, -c         Check/uncheck task
      --clear             Delete all checked items
      --context <ID>      Show one item with its dependencies and notes
      --copy, -y          Copy item description
      --delete, -d        Delete item
      --destroy           Move a project's board to the trash
      --dismiss <IDS>     With --repeats: set fingerprints aside until they recur again
      --doctor            Check that every Claude Code session runs ekko's current binary and its hooks; exit 1 on a fail
      --probe             With --doctor: write the board again, then wait for each session on it to hear it
      --edit, -e          Edit item description
      --find, -f          Search for items
      --force             Override the blocked-by rule or a running session's hold
      --help, -h          Display help message
      --json, -j          Output machine-readable JSON instead of formatted text
      --link-project <NAME> Link this project's board and NAME's, for Claude Code sessions
      --list, -l          List items by attributes
      --mcp               Serve the board to an agent over MCP (stdio)
      --resources         With --mcp: serve only the board as @-mentionable resources
      --move, -m          Move item between boards
      --move-to <PROJECT> Move items to another project's board, or ~ for the default one
      --next [N]          List what to take up next, best first
      --note, -n          Create note
      --kind <KIND>       With --note: a decision, gotcha or procedure
      --supersedes <ID>   With --kind: the earlier note of that kind it replaces
      --phase <NAME>      Scope work to one phase of a project
      --phases <NAME>...  Declare the project's ordered phase sequence
      --prime             Summarise the board for picking work back up
      --hook              With --prime, --memory, --tasklist or --guard: answer a Claude Code hook's event on stdin
      --memory            With --hook: put the project's memory page in a starting session's context
      --tasklist          With --hook: draw the board in the session's Claude Code task list
      --guard             With --hook: refuse the Bash calls a gotcha's cue names
      --refuse <REASON>   With --guard: another guard's refusal, which the user may let through
      --priority, -p      Update priority of task
      --project <NAME>    Work against a named project instead of the default board
      --projects          List the projects that exist
      --repeats           List the failed tool calls that repeat across this project's sessions
      --restore, -r       Restore items from archive
      --roadmap           Show the project's roadmap through its phases
      --sessions         Show each Claude Code session on this board and its work
      --set               Set item state idempotently (retry-safe)
      --since <MILLIS>    Only items changed at or after a timestamp
      --star, -s          Star/unstar item
      --stash [IDS]       Put items or a board away; no ids lists the stash
      --trash             Show the trash, and how long each thing has left
      --unlink-project <NAME> Take away the link between this project's board and NAME's
      --unstash <IDS>     Bring items back out of the stash
      --untrash <IDS>     Bring items back out of the trash
      --ekko-dir          Define a custom ekko directory
      --task, -t          Create task
      --timeline, -i      Display timeline view
      --version, -v       Display installed version
      --with              Say who a task is with; no name, nobody

    Examples
      $ ekko
      $ ekko init
      $ ekko --archive
      $ ekko --attached-to @16 12
      $ ekko --begin 2 3
      $ ekko --check 1 2
      $ ekko --check 2 --force
      $ ekko --clear
      $ ekko --context 12
      $ ekko --copy 1 2 3
      $ ekko --delete 4
      $ ekko --doctor
      $ ekko --doctor --probe
      $ ekko --edit @3 Merge PR #42
      $ ekko --find documentation
      $ ekko --project old --destroy
      $ ekko --json --task @coding Review PR #42
      $ ekko --list pending coding
      $ ekko --list with:rodrigo
      $ ekko --move @1 cooking
      $ ekko --move-to zettelkasten 3 5
      $ ekko --next 5
      $ ekko --note @coding Mergesort worse-case O(nlogn)
      $ ekko --note --kind gotcha Run the migrations before the tests
      $ ekko --note @coding - < why.txt
      $ ekko --prime
      $ ekko --priority @3 2
      $ ekko --repeats
      $ ekko --restore 4
      $ ekko --project demo --roadmap
      $ ekko --star 2
      $ ekko --stash @due
      $ ekko --unstash 9
      $ ekko --task @coding @reviews Review PR #42
      $ ekko --task @coding Improve documentation
      $ ekko --task Make some buttercream
      $ ekko --task Send the contract with:rodrigo
      $ ekko --timeline
      $ ekko --with @3 rodrigo
"#;

/// Argument that re-invokes this same binary as a detached clipboard
/// server. Never typed by a user, only ever passed by `write_clipboard`
/// spawning itself -- picked to be vanishingly unlikely to collide with a
/// real task description someone passes on the command line.
const CLIPBOARD_DAEMON_ARG: &str = "__ekko_clipboard_daemon";

fn main() -> ExitCode {
    // Rust disables SIGPIPE at startup, which turns `ekko --json | head`
    // into a panic on a broken pipe rather than the quiet exit every other
    // Unix tool gives you. Restoring the default handler makes the process
    // die on the signal instead, which is what a pipeline expects -- and it
    // covers every output path at once, including ones added later.
    //
    // SAFETY: sets a signal disposition before any thread is spawned or any
    // output is written.
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };

    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.first().map(String::as_str) == Some(CLIPBOARD_DAEMON_ARG) {
        return run_clipboard_daemon();
    }

    // Handled before clap ever sees argv, same as meow's behavior this is
    // replacing: --help/--version anywhere in the invocation wins,
    // regardless of what else was passed.
    if args.iter().any(|a| a == "-h" || a == "--help") {
        print!("{HELP}");
        return ExitCode::SUCCESS;
    }
    if args.iter().any(|a| a == "-v" || a == "--version") {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }

    // `init` is a command word, as in git, and comes first -- after `--json`
    // at most. Anywhere else it is an ordinary word.
    let leading_json = args.iter().take_while(|a| *a == "--json" || *a == "-j").count();
    if args.get(leading_json).map(String::as_str) == Some("init") {
        return run_init(&args[leading_json + 1..], leading_json > 0);
    }
    // `docs` too (task 915), and `artifact` (task 1019).
    if args.get(leading_json).map(String::as_str) == Some("docs") {
        return run_docs(&args[leading_json + 1..], leading_json > 0);
    }
    if args.get(leading_json).map(String::as_str) == Some("artifact") {
        return run_artifact(&args[leading_json + 1..], leading_json > 0);
    }
    // And `serve` (task 1102).
    if args.get(leading_json).map(String::as_str) == Some("serve") {
        return run_serve(&args[leading_json + 1..]);
    }

    let cli = match cli::Cli::try_parse_from(std::iter::once("ekko".to_string()).chain(args)) {
        Ok(cli) => cli,
        Err(e) => {
            eprint!("{e}");
            return ExitCode::FAILURE;
        }
    };

    let json_mode = cli.json;
    // `std::env::home_dir` rather than the `home` crate: it was
    // un-deprecated once its Windows behaviour was fixed, and the crate
    // was the single thing holding this crate's MSRV at 1.88.
    let home_dir = std::env::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let ekko_dir_env = std::env::var("EKKO_DIR").ok();
    // Same role as EKKO_DIR, one level up: sticks a project to a shell
    // without a persistent "current project" file, which would change what
    // `ekko` shows from invisible state.
    let project_env = std::env::var("EKKO_PROJECT").ok();
    // A PreToolUse hook on every Bash call, before any board is located or
    // opened: it reads every board, through an index of its own.
    if cli.guard {
        let input = read_all_stdin();
        return match (&cli.refuse, cli.hook) {
            (Some(reason), _) => guard::refuse(&home_dir, &input, reason),
            (None, true) => guard::hook(&home_dir, &input),
            (None, false) => {
                eprintln!("--guard takes --hook, as Claude Code's PreToolUse, or --refuse REASON");
                ExitCode::FAILURE
            }
        };
    }
    // Before any board is opened: the server opens one per call, because each
    // call may name a different project.
    if cli.mcp {
        let mode = if cli.resources { mcp::Mode::Resources } else { mcp::Mode::Board };
        return mcp::run(home_dir, cwd, ekko_dir_env, project_env, mode);
    }
    // About every session on this machine, whatever board it is on; with
    // --probe, after writing the board found here as any command finds it.
    if cli.doctor {
        let mut probe = None;
        if cli.probe {
            let project_name = cli.project.as_deref().or(project_env.as_deref());
            let written = directory::locate(&home_dir, &cwd, cli.ekko_dir.as_deref(), ekko_dir_env.as_deref(), project_name)
                .map_err(EkkoError::from)
                .and_then(|location| doctor::probe(&Ekko::at(&location)?.storage, &home_dir));
            match written {
                Ok(written) => probe = Some(written),
                Err(err) => return finish_with_error(&err, json_mode, &home_dir),
            }
        }
        return doctor::run(&home_dir, json_mode, probe);
    }

    // Before opening anything: an old flag gets the same answer whatever
    // board it was aimed at, and a caller should learn what the flag is
    // called now, or that it is gone, before learning, say, that a project
    // is missing.
    if let Some(err) = retired_flag(&cli) {
        return finish_with_error(&err, json_mode, &home_dir);
    }
    // `--force` overrides one rule, from either side: completing a blocked
    // task, or reopening one that completed work depends on. Beside
    // --move-to, it moves what a running session holds or watches.
    // Anywhere else it would be accepted and do nothing, and a flag that
    // silently does nothing is one somebody eventually believes did something.
    if cli.force && !(cli.check || cli.set || cli.move_to.is_some()) {
        return finish_with_error(&EkkoError::ForceWithoutCompleting, json_mode, &home_dir);
    }

    // Which board: one named, the project found from the folder, or the
    // default -- `directory::locate` has the order. A project found from the
    // folder is named above the board, so the folder never changes what
    // `ekko` shows without saying so.
    let project_name = cli.project.as_deref().or(project_env.as_deref());
    let location =
        match directory::locate(&home_dir, &cwd, cli.ekko_dir.as_deref(), ekko_dir_env.as_deref(), project_name) {
            Ok(location) => location,
            // A project whose folder no longer holds its board has none to
            // open: --destroy forgets it instead (task 503).
            Err(directory::DirectoryError::ProjectMoved { name, .. }) if cli.destroy => {
                return match project::forget(&home_dir, &name, chrono::Local::now().timestamp_millis()) {
                    Ok(forgotten) => {
                        let outcome = Outcome::Forgotten(forgotten);
                        if json_mode {
                            json_output::print_success(&outcome);
                        } else {
                            with_renderer(&home_dir, |r| outcome.render(r));
                        }
                        ExitCode::SUCCESS
                    }
                    Err(err) => finish_with_error(&EkkoError::from(err), json_mode, &home_dir),
                };
            }
            Err(err) => return finish_with_error(&EkkoError::from(err), json_mode, &home_dir),
        };
    let board_label = location.label();
    // A page beside the board, read without opening the board.
    if cli.memory {
        let event = agent::SessionEvent::from_hook_input(&read_hook_input());
        print!("{}", memory::hook(&location.dir, &board_label, &event.source));
        return ExitCode::SUCCESS;
    }
    // A command run by an agent through Bash, or by a hook, acts for the
    // Claude Code session it runs under; one typed at a terminal, for the user.
    let ekko = match Ekko::at(&location) {
        Ok(ekko) => ekko
            .acting_as(holder::Actor::of_this_command().with_registry(holder::Registry::at(agent::processes_dir(&home_dir))))
            .in_folder(location.project.as_ref().and_then(|project| project.root.clone())),
        Err(err) => return finish_with_error(&err, json_mode, &home_dir),
    };

    // Its own exit code: 2 wakes the session, and anything else must stay
    // silent on a hook that runs on every write to the board.
    if cli.wake {
        return wake::hook(&ekko, &read_hook_input(), &home_dir, &board_label);
    }

    match dispatch(&cli, &ekko, location.project.as_ref(), &home_dir, &cwd, &board_label) {
        Ok(outcomes) => {
            if json_mode {
                for outcome in &outcomes {
                    json_output::print_success(outcome);
                }
            } else {
                let blockers = ekko.blocker_map().unwrap_or_default();
                with_renderer(&home_dir, |r| {
                    r.with_blockers(blockers);
                    r.with_registry(holder::Registry::at(agent::processes_dir(&home_dir)));
                    r.with_steps(agent::steps(&ekko).unwrap_or_default());
                    // Named before the board, and only when one is active: an
                    // EKKO_PROJECT set and forgotten would otherwise show a
                    // different board with nothing on screen saying so.
                    // Suppressed for --projects, which is already about them,
                    // and for --destroy, whose reply names the project it
                    // just removed -- a header above that would announce a
                    // board nobody can open any more. And for a hook's reply,
                    // which Claude Code reads as JSON only when it is nothing
                    // else: --tasklist's watchPaths would become context. And
                    // for --sessions and --repeats, whose first line names the board.
                    if let Some(project) = &location.project {
                        if !cli.projects && !cli.destroy && !cli.prime && !cli.tasklist && !cli.sessions && !cli.repeats {
                            r.display_project(&project.name);
                        }
                    }
                    for outcome in &outcomes {
                        outcome.render(r);
                    }
                });
            }
            ExitCode::SUCCESS
        }
        Err(err) => finish_with_error(&err, json_mode, &home_dir),
    }
}

/// The error for a flag used under a name it no longer has, or one that is
/// gone.
///
/// `--anchor` became `--attached-to` and `--path` became `--roadmap`, each
/// renamed to the word for what it does. The old spellings still parse,
/// only so they can be answered with the new one.
fn retired_flag(cli: &cli::Cli) -> Option<EkkoError> {
    if cli.anchor.is_some() {
        return Some(EkkoError::RenamedFlag { old: "--anchor", new: "--attached-to" });
    }
    if cli.path {
        return Some(EkkoError::RenamedFlag { old: "--path", new: "--roadmap" });
    }
    // Not a rename, strictly: projects moved into their folders, and what
    // made one is `ekko init` there now. Answered the same way, because what
    // the caller needs is the same -- the command to use instead.
    if cli.create {
        return Some(EkkoError::RenamedFlag { old: "--create", new: "ekko init" });
    }
    // Gone with nothing under a new name: the interactive mode was removed.
    // A person who types it from habit is told so, and where the board is.
    if cli.ui {
        return Some(EkkoError::RemovedFlag { old: "--ui", instead: "For the board, run ekko with no options" });
    }
    None
}

/// `ekko init [<folder>] [--name <name>]`: makes a folder a project -- see
/// `project::init`.
fn run_init(args: &[String], json_first: bool) -> ExitCode {
    let home_dir = std::env::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let init = match cli::InitCli::try_parse_from(std::iter::once("ekko init".to_string()).chain(args.iter().cloned())) {
        Ok(init) => init,
        Err(e) => {
            eprint!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let json_mode = json_first || init.json;
    let now = chrono::Local::now().timestamp_millis();
    match project::init(&home_dir, &cwd, init.folder.as_deref(), init.name.as_deref(), now) {
        Ok(done) => {
            let outcome = Outcome::Init(Box::new(done));
            if json_mode {
                json_output::print_success(&outcome);
            } else {
                with_renderer(&home_dir, |r| outcome.render(r));
            }
            ExitCode::SUCCESS
        }
        Err(err) => finish_with_error(&EkkoError::from(err), json_mode, &home_dir),
    }
}

/// `ekko docs [<folder>]`: the board's documentation, written as markdown
/// into the folder given, or else the project's docs/. The default board
/// belongs to no folder, so it needs one named.
fn run_docs(args: &[String], json_first: bool) -> ExitCode {
    let home_dir = std::env::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let docs = match cli::DocsCli::try_parse_from(std::iter::once("ekko docs".to_string()).chain(args.iter().cloned())) {
        Ok(docs) => docs,
        Err(e) => {
            eprint!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let json_mode = json_first || docs.json;
    let ekko_dir_env = std::env::var("EKKO_DIR").ok();
    let project_env = std::env::var("EKKO_PROJECT").ok();
    let project_name = docs.project.as_deref().or(project_env.as_deref());
    let location = match directory::locate(&home_dir, &cwd, None, ekko_dir_env.as_deref(), project_name) {
        Ok(location) => location,
        Err(err) => return finish_with_error(&EkkoError::from(err), json_mode, &home_dir),
    };
    let root = location.project.as_ref().and_then(|project| project.root.clone());
    let folder = match (&docs.folder, &root) {
        (Some(folder), _) => cwd.join(folder),
        (None, Some(root)) => root.join("docs"),
        (None, None) => {
            let err = EkkoError::InvalidInput(
                "The default board belongs to no project's folder, so its docs have nowhere to go: name the folder, as ekko docs <folder>"
                    .to_string(),
            );
            return finish_with_error(&err, json_mode, &home_dir);
        }
    };
    let project = location.project.as_ref().map(|project| project.name.as_str());
    let written = Ekko::at(&location).and_then(|ekko| ekko.in_folder(root.clone()).write_docs(&folder, &location.dir, project));
    match written {
        Ok(outcome) => {
            if json_mode {
                json_output::print_success(&outcome);
            } else {
                with_renderer(&home_dir, |r| outcome.render(r));
            }
            ExitCode::SUCCESS
        }
        Err(err) => finish_with_error(&err, json_mode, &home_dir),
    }
}

/// `ekko artifact <id>`: the artifact's page, written from the board and
/// opened in the default browser, where it keeps itself current.
fn run_artifact(args: &[String], json_first: bool) -> ExitCode {
    let home_dir = std::env::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let cli = match cli::ArtifactCli::try_parse_from(std::iter::once("ekko artifact".to_string()).chain(args.iter().cloned())) {
        Ok(cli) => cli,
        Err(e) => {
            eprint!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let json_mode = json_first || cli.json;
    let ekko_dir_env = std::env::var("EKKO_DIR").ok();
    let project_env = std::env::var("EKKO_PROJECT").ok();
    let project_name = cli.project.as_deref().or(project_env.as_deref());
    let location = match directory::locate(&home_dir, &cwd, None, ekko_dir_env.as_deref(), project_name) {
        Ok(location) => location,
        Err(err) => return finish_with_error(&EkkoError::from(err), json_mode, &home_dir),
    };
    let root = location.project.as_ref().and_then(|project| project.root.clone());
    let written = Ekko::at(&location).and_then(|ekko| {
        let all = ekko.storage.get_shared()?;
        let id = ekko.validate_ids(std::slice::from_ref(&cli.id), &all)?[0];
        let item = &all[&id];
        if item.artifact.is_none() {
            return Err(EkkoError::InvalidInput(format!("{id} is not an artifact: a session writes one with ekko's artifact tool")));
        }
        let path = artifact::write(&location.dir, item, &all, root.as_deref())
            .map_err(|error| EkkoError::InvalidInput(format!("its page could not be written: {error}")))?;
        Ok((path, item.uid.clone().unwrap_or_default()))
    });
    let (path, uid) = match written {
        Ok(written) => written,
        Err(err) => return finish_with_error(&err, json_mode, &home_dir),
    };
    // The server's page where one can serve it, the file otherwise (task 1102).
    let served = serve::page_address(&home_dir, &location, &uid);
    let page = served.clone().unwrap_or_else(|_| path.display().to_string());
    // The browser goes through the redirect file, which hands it the token
    // a write from the page needs (task 1103).
    let opening = served.as_ref().ok().and_then(|address| serve::redirect_file(&home_dir, address).ok());
    let opened = !cli.no_open && artifact::open(opening.as_deref().map_or(page.as_ref(), std::path::Path::as_os_str)).is_ok();
    if json_mode {
        let mut out = serde_json::json!({"page": page, "opened": opened});
        if let Err(why) = &served {
            out["unserved"] = serde_json::json!(why);
        }
        println!("{out}");
        return ExitCode::SUCCESS;
    }
    if let Err(why) = &served {
        eprintln!("ekko: the page is its file, served by no server: {why}");
    }
    if opened {
        println!("{page} (opened in the browser)");
    } else {
        println!("{page}");
    }
    ExitCode::SUCCESS
}

/// `ekko serve`: the local server of artifact pages, in the foreground, or
/// with `--stop`, the end of the one that runs (task 1102).
fn run_serve(args: &[String]) -> ExitCode {
    let home_dir = std::env::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let cli = match cli::ServeCli::try_parse_from(std::iter::once("ekko serve".to_string()).chain(args.iter().cloned())) {
        Ok(cli) => cli,
        Err(e) => {
            eprint!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let done = if cli.stop {
        serve::stop_running(&home_dir).map(|port| match port {
            Some(port) => println!("Stopped the server on 127.0.0.1:{port}"),
            None => println!("No server runs"),
        })
    } else {
        serve::run(&home_dir, cli.idle.map_or(serve::IDLE, std::time::Duration::from_secs))
    };
    match done {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("ekko serve: {error}");
            ExitCode::FAILURE
        }
    }
}

/// All of stdin, for a guard: a Bash command can run past what
/// `read_hook_input` takes, and a guard reading half of one would judge
/// another command. Nothing from a terminal.
fn read_all_stdin() -> String {
    use std::io::{IsTerminal, Read as _};
    let mut input = String::new();
    if !std::io::stdin().is_terminal() {
        let _ = std::io::stdin().lock().take(16 * 1024 * 1024).read_to_string(&mut input);
    }
    input
}

/// What a hook was handed on stdin, or nothing when stdin is a terminal: a
/// person typing `--prime --hook` gets the prime, not a prompt waiting on
/// input that never comes.
fn read_hook_input() -> String {
    use std::io::{IsTerminal, Read as _};
    let mut input = String::new();
    if !std::io::stdin().is_terminal() {
        let _ = std::io::stdin().lock().take(64 * 1024).read_to_string(&mut input);
    }
    input
}

/// `input` with a lone `-` replaced by the description read from stdin, for
/// --task, --note and --edit. A terminal on stdin is refused rather than
/// waited on: the `-` is for a pipe or a heredoc.
fn described(input: &[String]) -> Result<Vec<String>, EkkoError> {
    use std::io::{IsTerminal, Read as _};
    if !ekko::reads_stdin(input) {
        return Ok(input.to_vec());
    }
    let stdin = std::io::stdin();
    if stdin.is_terminal() {
        return Err(EkkoError::InvalidInput(
            "- reads the description from stdin: pipe it in, or use a heredoc".into(),
        ));
    }
    let mut text = String::new();
    stdin
        .lock()
        .read_to_string(&mut text)
        .map_err(|e| EkkoError::InvalidInput(format!("could not read the description from stdin: {e}")))?;
    Ok(ekko::with_description(input, &text))
}

/// Priority order copied from index.js's chain of `if (flags.x)` checks --
/// when more than one command flag is somehow set at once, the first
/// match in this exact order wins, the rest are silently ignored, same as
/// there. `--timeline`/`--list`/the default (no flag) view each produce
/// *two* outcomes -- the view, then the stats line -- matching the two
/// separate render calls index.js made for those three cases.
fn dispatch(
    cli: &cli::Cli,
    ekko: &Ekko,
    project: Option<&project::Project>,
    home_dir: &Path,
    cwd: &Path,
    board_label: &str,
) -> Result<Vec<Outcome>, EkkoError> {
    if let Some(args) = cli.attached_to.as_deref() {
        return Ok(vec![ekko.set_attached_to(args)?]);
    }
    if let Some(args) = cli.blocked_by.as_deref() {
        return Ok(vec![ekko.set_blocked_by(args)?]);
    }
    if let Some(names) = cli.phases.as_deref() {
        return Ok(vec![ekko.set_phases(names)?]);
    }
    if cli.roadmap {
        return Ok(vec![ekko.display_roadmap()?]);
    }
    if cli.tasklist {
        return Ok(vec![Outcome::Hook(tasklist::hook(ekko, &read_hook_input(), home_dir))]);
    }
    if cli.sessions {
        return Ok(vec![Outcome::Sessions(Box::new(agent::sessions(ekko, board_label)?))]);
    }
    if cli.repeats {
        let root = ekko.folder.as_deref();
        let report = match cli.dismiss.as_deref() {
            Some(ids) => repeats::dismiss(home_dir, root, board_label, ids)?,
            None => repeats::report(home_dir, root, board_label)?,
        };
        return Ok(vec![Outcome::Repeats(Box::new(report))]);
    }
    if cli.prime {
        if cli.hook {
            let event = agent::SessionEvent::from_hook_input(&read_hook_input());
            let repeats = || ekko.folder.as_deref().and_then(|root| repeats::announce(home_dir, root));
            let me = ekko.actor.as_ref().and_then(|actor| actor.process.as_ref()).map(|process| process.pid);
            let doctor = || doctor::attention(home_dir, ekko.storage.storage_path(), me);
            let text = agent::session_start(ekko, board_label, &event, &agent::session_state_dir(home_dir), repeats, doctor)?;
            return Ok(vec![Outcome::Hook(text)]);
        }
        return Ok(vec![Outcome::Prime(Box::new(agent::prime(ekko, board_label)?))]);
    }
    if let Some(limit) = cli.next {
        return Ok(vec![Outcome::Next(agent::next(ekko, limit)?)]);
    }
    if let Some(target) = cli.context.as_deref() {
        return Ok(vec![Outcome::Context(Box::new(agent::context(ekko, target)?))]);
    }
    if let Some(args) = cli.stash.as_deref() {
        return Ok(vec![if args.is_empty() {
            ekko.display_stash()?
        } else {
            ekko.set_stashed(args, true)?
        }]);
    }
    if let Some(args) = cli.unstash.as_deref() {
        return Ok(vec![ekko.set_stashed(args, false)?]);
    }
    if cli.trash {
        return Ok(vec![ekko.display_trash()?]);
    }
    if let Some(args) = cli.untrash.as_deref() {
        return Ok(vec![ekko.set_trashed(args, false)?]);
    }
    if cli.destroy {
        // Ahead of every view and every write: whatever else the line said,
        // a --destroy line is about removing the project, and running some
        // other command first would act on something about to disappear.
        let Some(project) = project else {
            return Err(directory::DirectoryError::DestroyNeedsProject.into());
        };
        return Ok(vec![ekko.destroy_project(home_dir, project, chrono::Local::now().timestamp_millis())?]);
    }
    if let Some(name) = cli.link_project.as_deref() {
        return Ok(vec![ekko.link_project(name, true)?]);
    }
    if let Some(name) = cli.unlink_project.as_deref() {
        return Ok(vec![ekko.link_project(name, false)?]);
    }
    if let Some(destination) = cli.move_to.as_deref() {
        return Ok(vec![move_to::move_to(ekko, home_dir, cwd, destination, &cli.input, cli.force)?]);
    }
    if cli.projects {
        return Ok(vec![Outcome::Projects(project::list(home_dir))]);
    }
    if cli.archive {
        return Ok(vec![ekko.display_archive()?]);
    }
    if cli.task {
        return Ok(vec![ekko.create_task_in(&described(&cli.input)?, cli.phase.as_deref())?]);
    }
    if cli.restore {
        return Ok(vec![ekko.restore_items(&cli.input)?]);
    }
    if cli.note {
        return Ok(vec![ekko.create_note_in(&described(&cli.input)?, cli.phase.as_deref(), cli.kind.as_deref(), cli.supersedes.as_deref())?]);
    }
    if cli.delete {
        return Ok(vec![ekko.delete_items(&cli.input)?]);
    }
    if cli.check {
        return Ok(vec![ekko.check_tasks(&cli.input, cli.force)?]);
    }
    if cli.begin {
        return Ok(vec![ekko.begin_tasks(&cli.input)?]);
    }
    if cli.star {
        return Ok(vec![ekko.star_items(&cli.input)?]);
    }
    if cli.set {
        return Ok(vec![ekko.set_state(&cli.input, cli.force)?]);
    }
    if cli.priority {
        return Ok(vec![ekko.update_priority(&cli.input)?]);
    }
    if cli.with {
        return Ok(vec![ekko.set_with(&cli.input)?]);
    }
    if cli.copy {
        return Ok(vec![ekko.copy_to_clipboard(&cli.input, write_clipboard)?]);
    }
    if let Some(since) = cli.since {
        return Ok(vec![ekko.display_since(since)?]);
    }
    if cli.timeline {
        return Ok(vec![ekko.display_by_date()?, ekko.display_stats()?]);
    }
    if cli.find {
        return Ok(vec![ekko.find_items(&cli.input)?]);
    }
    if cli.list {
        return Ok(vec![ekko.list_by_attributes(&cli.input)?, ekko.display_stats()?]);
    }
    if cli.edit {
        return Ok(vec![ekko.edit_description(&described(&cli.input)?)?]);
    }
    if let Some(file) = &cli.menu {
        return menu::run_file(ekko, file);
    }
    if cli.answer {
        // An id alone, in a terminal: ekko's menu on that question.
        if cli.input.len() == 1 && std::io::IsTerminal::is_terminal(&std::io::stdin().lock()) {
            return menu::run_one(ekko, &cli.input[0]);
        }
        return Ok(vec![ekko.answer_question(&described(&cli.input)?)?]);
    }
    if cli.r#move {
        return Ok(vec![ekko.move_boards(&cli.input)?]);
    }
    if cli.clear {
        return Ok(vec![ekko.clear()?]);
    }

    Ok(vec![ekko.display_by_board()?, ekko.display_stats()?])
}

/// X11/Wayland make the *copying application* responsible for answering
/// paste requests -- documented behavior of arboard's own README, not a
/// quirk of this code: content set with a plain `set_text` and an
/// immediately-exiting process can vanish before anything reads it, unless
/// a clipboard manager happens to be running to take ownership over. The
/// old JS version got this for free because `xsel --input` backgrounds
/// itself to keep serving; arboard's own recommended equivalent (see its
/// `daemonize.rs` example) is to spawn a detached copy of the process that
/// calls `.set().wait()`, blocking *that* process until something actually
/// receives the data, while the command the user is running returns
/// immediately. Confirmed empirically, not just from the docs: a plain
/// `set_text` measurably failed to survive process exit in this sandbox
/// (readable back seconds later by a *different* tool that had last
/// written the clipboard, i.e. the content never actually changed); this
/// version does.
fn write_clipboard(text: &str) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut child = std::process::Command::new(exe)
        .arg(CLIPBOARD_DAEMON_ARG)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;

    {
        use std::io::Write as _;
        let mut stdin = child.stdin.take().expect("stdin was piped");
        stdin.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
        // Dropped here (end of block), closing the pipe so the daemon's
        // read-to-end of stdin actually completes.
    }

    // Deliberately not `child.wait()`-ed: it's meant to outlive this
    // process, serving the clipboard until something else claims
    // ownership. Once we exit, it's reparented to init and reaped
    // normally when it eventually does too -- no zombie risk from us
    // never collecting its exit status.
    Ok(())
}

/// Reads the full clipboard text from stdin, then blocks -- this call does
/// not return until something else has requested and received the
/// clipboard contents (or, lacking that, forever; see `write_clipboard`'s
/// doc comment for why that's the documented, arboard-recommended
/// trade-off for a short-lived CLI rather than a bug).
fn run_clipboard_daemon() -> ExitCode {
    use std::io::Read as _;
    let mut text = String::new();
    if std::io::stdin().read_to_string(&mut text).is_err() {
        return ExitCode::FAILURE;
    }

    use arboard::SetExtLinux as _;

    let Ok(mut clipboard) = arboard::Clipboard::new() else {
        return ExitCode::FAILURE;
    };
    match clipboard.set().wait().text(text) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}

fn with_renderer(home_dir: &Path, f: impl FnOnce(&mut Renderer)) {
    // A config read failure here (rather than at a command that already
    // needed it) is rare enough -- and the fallback harmless enough -- to
    // just fall back to defaults rather than fail the whole command over
    // a display preference.
    let config = config::get(home_dir).unwrap_or_default();
    let mut stdout = std::io::stdout();
    let mut renderer = Renderer::new(Painter::auto(), config, &mut stdout);
    f(&mut renderer);
}

fn finish_with_error(err: &EkkoError, json_mode: bool, home_dir: &Path) -> ExitCode {
    if json_mode {
        json_output::print_error(err);
    } else {
        with_renderer(home_dir, |r| err.render(r));
    }
    ExitCode::FAILURE
}
