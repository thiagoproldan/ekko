//! Argument definitions. `--help`/`-h` and `--version`/`-v` are handled by
//! `main.rs` before this parser ever runs (see there for why); everything
//! else is a plain clap derive struct.

use clap::Parser;

/// `ekko init [<folder>] [--name <name>]`, parsed apart from the board's
/// flags: `init` is a command word, the way git's is.
#[derive(Parser, Debug)]
#[command(name = "ekko init", disable_help_flag = true, disable_version_flag = true)]
pub struct InitCli {
    /// The folder to make a project -- the current one when absent. Inside a
    /// git repository, the top of the repository.
    pub folder: Option<String>,
    /// The project's name; the folder's own when absent.
    #[arg(long, value_name = "NAME")]
    pub name: Option<String>,
    #[arg(long, short = 'j')]
    pub json: bool,
}

#[derive(Parser, Debug, Default)]
#[command(disable_help_flag = true, disable_version_flag = true)]
#[command(group(clap::ArgGroup::new("hooked").args(["prime", "tasklist"])))]
pub struct Cli {
    /// Attach a note to the task it explains, by id: the note first, then
    /// its task. No task detaches it.
    #[arg(long = "attached-to", num_args = 0.., value_name = "IDS")]
    pub attached_to: Option<Vec<String>>,

    /// The names `--attached-to` and `--roadmap` had before each was renamed
    /// to the word for what it does. Hidden, and kept only so a caller still
    /// carrying an old name -- an agent with an older copy of the skill in
    /// its context, above all -- is told the new one, rather than clap's
    /// bare "unexpected argument", which reads as the feature being gone.
    #[arg(long, num_args = 0.., hide = true)]
    pub anchor: Option<Vec<String>>,
    #[arg(long, hide = true)]
    pub path: bool,

    /// Put items or a whole board away, or with no ids, list what is away.
    #[arg(long, num_args = 0.., value_name = "IDS")]
    pub stash: Option<Vec<String>>,

    /// Bring items back out of the stash.
    #[arg(long, num_args = 1.., value_name = "IDS")]
    pub unstash: Option<Vec<String>>,

    /// Show what is in the trash, and how long it has left.
    #[arg(long)]
    pub trash: bool,

    /// Bring items back out of the trash.
    #[arg(long, num_args = 1.., value_name = "IDS")]
    pub untrash: Option<Vec<String>>,

    /// The current month, drawn. Nothing from the board is on it yet.
    #[arg(long)]
    pub calendar: bool,

    /// Removed along with the interactive mode it opened. Hidden, and kept
    /// only so that a typed `--ui` is told it was removed, rather than clap's
    /// bare "unexpected argument", which reads as a typo.
    #[arg(long, hide = true)]
    pub ui: bool,

    /// Serve the board to an agent over MCP on stdin and stdout, until stdin
    /// closes. The agent's frontend, as the board view is a person's.
    #[arg(long)]
    pub mcp: bool,

    /// With --mcp: serve only the board's resources, prime://board and
    /// item://<id>, for a server registered under a plain name. Claude Code
    /// cannot resolve an @ mention of a plugin server's resources: it splits
    /// the mention at the first colon, and a plugin server's name has two.
    #[arg(long, requires = "mcp")]
    pub resources: bool,

    #[arg(long, short = 'a')]
    pub archive: bool,
    #[arg(long, short = 'b')]
    pub begin: bool,
    /// Record what an item is blocked by: the blocked id prefixed with `@`,
    /// then the ids blocking it. Replaces whatever blocked it before, and no
    /// ids clears it. A blocked task cannot be completed, short of `--force`.
    #[arg(long = "blocked-by", num_args = 0.., value_name = "IDS")]
    pub blocked_by: Option<Vec<String>>,

    #[arg(long, short = 'c')]
    pub check: bool,
    /// Override the blocked-by rule from either side: complete a task that is
    /// still blocked, or reopen one that completed work depends on. And
    /// change a task another running Claude Code session holds, which a
    /// session's own shell is otherwise refused. Only means
    /// something beside `--check` or `--set`, and deliberately has no short
    /// form: `-f` is `--find`, and overriding a rule should take typing the
    /// word.
    #[arg(long)]
    pub force: bool,
    #[arg(long)]
    pub clear: bool,
    #[arg(long, short = 'y')]
    pub copy: bool,
    #[arg(long, short = 'd')]
    pub delete: bool,
    #[arg(long, short = 'e')]
    pub edit: bool,
    /// Answer a question asked on the board: its id, then the answer.
    #[arg(long)]
    pub answer: bool,
    #[arg(long, short = 'f')]
    pub find: bool,
    #[arg(long, short = 'j')]
    pub json: bool,
    #[arg(long, short = 'l')]
    pub list: bool,
    #[arg(long = "move", short = 'm')]
    pub r#move: bool,
    #[arg(long, short = 'n')]
    pub note: bool,
    /// With --note: the lasting kind of note it is -- decision (what was
    /// settled, and why), gotcha (a trap, and how to avoid it) or procedure
    /// (steps that work). Refused without --note, rather than accepted and
    /// ignored.
    #[arg(long, value_name = "KIND", requires = "note")]
    pub kind: Option<String>,
    /// With --note --kind: the earlier note of the same kind this one
    /// replaces, by id. The older one stays on the board, as history.
    #[arg(long, value_name = "ID", requires = "note")]
    pub supersedes: Option<String>,
    /// Scope work to one phase of a project. Areas are phase-scoped, so this
    /// is what distinguishes `@render` under `setup` from `@render` under
    /// `compositor`.
    #[arg(long, value_name = "NAME")]
    pub phase: Option<String>,

    /// Declare the project's ordered phase sequence, replacing whatever was
    /// there. Replacing rather than appending because inserting a phase in
    /// the middle is the common case, and appending cannot express it.
    #[arg(long, num_args = 0.., value_name = "NAME")]
    pub phases: Option<Vec<String>>,

    /// Render the project's roadmap: phases in order, with progress and
    /// where work currently sits.
    #[arg(long)]
    pub roadmap: bool,

    /// The resume view: what is in progress, what is ready in the order to
    /// take it up, what is blocked, and the notes that explain them. Built
    /// for an agent picking a board back up; see `agent`.
    #[arg(long)]
    pub prime: bool,
    /// Each Claude Code session on this board: what it holds, what it
    /// finished today, what it asked that waits, and how to resume it.
    #[arg(long)]
    pub sessions: bool,

    /// With --prime or --tasklist: answer a Claude Code hook, whose event
    /// arrives as JSON on stdin. For --prime, a session that resumes or forks
    /// gets what moved since this hook last served it, not a second prime.
    #[arg(long, requires = "hooked")]
    pub hook: bool,

    /// With --hook: draw the board in the session's own Claude Code task
    /// list, the one under the spinner -- see `tasklist`. The plugin runs it
    /// when a session starts, after each ekko write, and when the board's
    /// file changes.
    #[arg(long, requires = "hook")]
    pub tasklist: bool,

    /// What to take up next, best first, optionally only the first N.
    #[arg(long, num_args = 0..=1, value_name = "N")]
    pub next: Option<Option<usize>>,

    /// One item with everything around it: what blocks it, what it blocks,
    /// the task it explains or the notes explaining it.
    #[arg(long, value_name = "ID")]
    pub context: Option<String>,
    /// Work against a named project instead of the default board. Sugar over
    /// `--ekko-dir`: the project lives at `~/.ekko/projects/<name>`, so the
    /// filesystem is the registry and there is no list to keep in sync.
    #[arg(long, value_name = "NAME")]
    pub project: Option<String>,

    /// Retired with the move of projects into their folders: answered with
    /// `ekko init`, the way a renamed flag is answered with its new name.
    #[arg(long, hide = true)]
    pub create: bool,

    /// Move the project named by `--project` to the trash.
    ///
    /// Deliberately not `--delete`, which already means "remove items":
    /// `--project foo --delete 3` removes item 3 inside foo, and giving the
    /// same word both meanings would make a command that lost its ids
    /// destroy the whole project instead of failing.
    #[arg(long)]
    pub destroy: bool,

    /// List the projects that exist, with their item counts.
    #[arg(long)]
    pub projects: bool,
    #[arg(long, short = 'p')]
    pub priority: bool,
    /// Idempotent counterpart to the `--check`/`--begin`/`--star` toggles:
    /// states the item should end up in, rather than flipping whatever it
    /// is now. Retry-safe, which the toggles are not.
    #[arg(long)]
    pub set: bool,
    #[arg(long, short = 'r')]
    pub restore: bool,
    /// Epoch milliseconds. Restricts the board view to items changed at or
    /// after that instant, for callers syncing incrementally instead of
    /// pulling the whole board every time.
    #[arg(long, value_name = "MILLIS")]
    pub since: Option<i64>,
    #[arg(long, short = 's')]
    pub star: bool,
    #[arg(long = "ekko-dir")]
    pub ekko_dir: Option<String>,
    #[arg(long, short = 't')]
    pub task: bool,
    #[arg(long, short = 'i')]
    pub timeline: bool,

    /// Everything left over: item ids, `@board` tags, descriptions,
    /// `p:N` priority markers -- whatever the chosen command needs, parsed
    /// downstream in ekko.rs exactly like the JS version's `input` array.
    pub input: Vec<String>,
}
