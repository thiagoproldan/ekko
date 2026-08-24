mod cli;
mod config;
mod directory;
mod ekko;
mod item;
mod json;
mod json_output;
mod paths;
mod render;
mod storage;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;

use ekko::{Ekko, EkkoError, Outcome};
use render::{Painter, Renderer};

const HELP: &str = r#"
  Usage
    $ ekko [<options> ...]

    Options
        none             Display board view
      --archive, -a      Display archived items
      --begin, -b        Start/pause task
      --check, -c        Check/uncheck task
      --clear            Delete all checked items
      --copy, -y         Copy item description
      --delete, -d       Delete item
      --edit, -e         Edit item description
      --find, -f         Search for items
      --help, -h         Display help message
      --json, -j         Output machine-readable JSON instead of formatted text
      --list, -l         List items by attributes
      --move, -m         Move item between boards
      --note, -n         Create note
      --priority, -p     Update priority of task
      --restore, -r      Restore items from archive
      --set              Set item state idempotently (retry-safe)
      --star, -s         Star/unstar item
      --ekko-dir         Define a custom ekko directory
      --task, -t         Create task
      --timeline, -i     Display timeline view
      --version, -v      Display installed version

    Examples
      $ ekko
      $ ekko --archive
      $ ekko --begin 2 3
      $ ekko --check 1 2
      $ ekko --clear
      $ ekko --copy 1 2 3
      $ ekko --delete 4
      $ ekko --edit @3 Merge PR #42
      $ ekko --find documentation
      $ ekko --json --task @coding Review PR #42
      $ ekko --list pending coding
      $ ekko --move @1 cooking
      $ ekko --note @coding Mergesort worse-case O(nlogn)
      $ ekko --priority @3 2
      $ ekko --restore 4
      $ ekko --star 2
      $ ekko --task @coding @reviews Review PR #42
      $ ekko --task @coding Improve documentation
      $ ekko --task Make some buttercream
      $ ekko --timeline
"#;

/// Argument that re-invokes this same binary as a detached clipboard
/// server. Never typed by a user, only ever passed by `write_clipboard`
/// spawning itself -- picked to be vanishingly unlikely to collide with a
/// real task description someone passes on the command line.
const CLIPBOARD_DAEMON_ARG: &str = "__ekko_clipboard_daemon";

fn main() -> ExitCode {
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

    let ekko = match Ekko::open(&home_dir, &cwd, cli.ekko_dir.as_deref(), ekko_dir_env.as_deref()) {
        Ok(ekko) => ekko,
        Err(err) => return finish_with_error(&err, json_mode, &home_dir),
    };

    match dispatch(&cli, &ekko) {
        Ok(outcomes) => {
            if json_mode {
                for outcome in &outcomes {
                    json_output::print_success(outcome);
                }
            } else {
                with_renderer(&home_dir, |r| {
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

/// Priority order copied from index.js's chain of `if (flags.x)` checks --
/// when more than one command flag is somehow set at once, the first
/// match in this exact order wins, the rest are silently ignored, same as
/// there. `--timeline`/`--list`/the default (no flag) view each produce
/// *two* outcomes -- the view, then the stats line -- matching the two
/// separate render calls index.js made for those three cases.
fn dispatch(cli: &cli::Cli, ekko: &Ekko) -> Result<Vec<Outcome>, EkkoError> {
    if cli.archive {
        return Ok(vec![ekko.display_archive()?]);
    }
    if cli.task {
        return Ok(vec![ekko.create_task(&cli.input)?]);
    }
    if cli.restore {
        return Ok(vec![ekko.restore_items(&cli.input)?]);
    }
    if cli.note {
        return Ok(vec![ekko.create_note(&cli.input)?]);
    }
    if cli.delete {
        return Ok(vec![ekko.delete_items(&cli.input)?]);
    }
    if cli.check {
        return Ok(vec![ekko.check_tasks(&cli.input)?]);
    }
    if cli.begin {
        return Ok(vec![ekko.begin_tasks(&cli.input)?]);
    }
    if cli.star {
        return Ok(vec![ekko.star_items(&cli.input)?]);
    }
    if cli.set {
        return Ok(vec![ekko.set_state(&cli.input)?]);
    }
    if cli.priority {
        return Ok(vec![ekko.update_priority(&cli.input)?]);
    }
    if cli.copy {
        return Ok(vec![ekko.copy_to_clipboard(&cli.input, write_clipboard)?]);
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
        return Ok(vec![ekko.edit_description(&cli.input)?]);
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
