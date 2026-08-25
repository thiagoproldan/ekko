//! Argument definitions. `--help`/`-h` and `--version`/`-v` are handled by
//! `main.rs` before this parser ever runs (see there for why); everything
//! else is a plain clap derive struct.

use clap::Parser;

#[derive(Parser, Debug, Default)]
#[command(disable_help_flag = true, disable_version_flag = true)]
pub struct Cli {
    #[arg(long, short = 'a')]
    pub archive: bool,
    #[arg(long, short = 'b')]
    pub begin: bool,
    /// Record what an item waits on: the target id prefixed with `@`, then
    /// the ids it should wait for. Replaces whatever it waited on before.
    #[arg(long = "blocked-by", num_args = 0.., value_name = "IDS")]
    pub blocked_by: Option<Vec<String>>,

    #[arg(long, short = 'c')]
    pub check: bool,
    #[arg(long)]
    pub clear: bool,
    #[arg(long, short = 'y')]
    pub copy: bool,
    #[arg(long, short = 'd')]
    pub delete: bool,
    #[arg(long, short = 'e')]
    pub edit: bool,
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

    /// Render the project's journey: phases in order, with progress and
    /// where work currently sits.
    #[arg(long)]
    pub path: bool,
    /// Work against a named project instead of the default board. Sugar over
    /// `--ekko-dir`: the project lives at `~/.ekko/projects/<name>`, so the
    /// filesystem is the registry and there is no list to keep in sync.
    #[arg(long, value_name = "NAME")]
    pub project: Option<String>,

    /// Create the project named by `--project` rather than failing when it
    /// does not exist. Separate on purpose: creating on first use would turn
    /// a typo into a new, empty project.
    #[arg(long)]
    pub create: bool,

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
