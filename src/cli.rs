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
    #[arg(long, short = 'p')]
    pub priority: bool,
    #[arg(long, short = 'r')]
    pub restore: bool,
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
