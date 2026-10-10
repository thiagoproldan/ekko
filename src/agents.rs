//! `ekko agents start` (task 1642, artifact 1640): a session of Claude Code
//! born to do board tasks, opened in a window of its own in a terminal
//! multiplexer, with the model and effort chosen for those tasks, in a
//! worktree of its own. It lives for its tasks; the user sees its window,
//! and may step into it and talk to it. Its hook (task 1643) closes it once
//! its tasks are finished, unless the user talks to it after that, and says
//! on the board and in its window's name when it waits on the user (task
//! 1645). What it is for and what it does now are kept on its pane, for
//! the views to draw (task 1675).

use std::collections::{BTreeMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::directory::Location;
use crate::ekko::{Ekko, EkkoError};
use crate::holder::Process;
use crate::item::{Item, State};
use crate::storage::ItemMap;

/// The multiplexer's session holding a window for each born session.
pub const SESSION: &str = "ekko";

/// Names, by uid and comma-separated, the tasks a born session was started
/// for, for the hooks that run in it.
pub const TASKS_VAR: &str = "EKKO_AGENT_TASK";

/// The levels Claude Code's `--effort` takes (its CLI reference, read
/// 2026-10-09); which of them a model allows is Claude Code's to say.
pub const EFFORTS: &[&str] = &["low", "medium", "high", "xhigh", "max", "ultracode"];

/// How long a born session must still run after its window opens for the
/// start to count: a command that is not there, or a Claude Code that
/// refuses to start, ends well within it.
const SETTLE: Duration = Duration::from_secs(1);

/// The user options a born session's pane holds for the views (task 1675,
/// decision 1674), which every view reads and none writes: its tasks' ids,
/// the first one's title, its model and effort, set as it starts; its state
/// -- `working`, `waiting` on the user or `idle` -- and since when, in
/// seconds since the epoch, set by its hook as it changes; and what it
/// waits on, while it does. On the pane, not its window, so they go with it
/// into another window.
pub const TASKS_OPTION: &str = "@ekko_tasks";
pub const TITLE_OPTION: &str = "@ekko_title";
pub const MODEL_OPTION: &str = "@ekko_model";
pub const EFFORT_OPTION: &str = "@ekko_effort";
pub const STATE_OPTION: &str = "@ekko_state";
pub const SINCE_OPTION: &str = "@ekko_since";
pub const WAITS_OPTION: &str = "@ekko_waits";

/// The states `STATE_OPTION` takes.
pub const WORKING: &str = "working";
pub const WAITING: &str = "waiting";
pub const IDLE: &str = "idle";

/// How a born session's window reads in the status line (task 1675): its
/// index, then its name after a symbol for its pane's state -- ⚙ working,
/// ● waiting on you, black on yellow, ○ idle, dimmed -- and tmux's flags,
/// as tmux's own format ends. No colour of a status line hides it: tmux's
/// own is green, which a green symbol would vanish into.
pub const STATUS_FORMAT: &str = concat!(
    "#I:#{?#{==:#{@ekko_state},waiting},#[fg=black#,bg=yellow#,bold]\u{25cf} #W#[default],",
    "#{?#{==:#{@ekko_state},working},\u{2699} #W,",
    "#{?#{==:#{@ekko_state},idle},#[dim]\u{25cb} #W#[default],#W}}}",
    "#{?window_flags,#{window_flags}, }"
);

/// What `start` opened.
pub struct Started {
    pub tasks: Vec<u32>,
    pub window: String,
    pub pane: String,
    pub worktree: PathBuf,
    pub branch: String,
    /// Whether the worktree was made by this start, rather than left by an
    /// earlier one.
    pub made: bool,
    /// The command that shows the session's window.
    pub attach: String,
    /// Why the pane's options and its window's status line could not be
    /// set, if they could not: the session runs all the same.
    pub undrawn: Option<String>,
}

/// Opens a session of Claude Code for the tasks `raw_ids` of the board at
/// `location`, with `model` and `effort`: in a worktree of the project's
/// repository, on a branch named for the tasks, in a window named for them
/// and the model in the multiplexer's session `ekko`, told by its first
/// prompt to claim them before it changes anything. Refused, opening
/// nothing, for a task the page's Start would refuse -- done, in progress,
/// in the trash, with someone, waiting on open work -- and for one that has
/// a window already.
pub fn start(
    ekko: &Ekko,
    location: &Location,
    home: &Path,
    raw_ids: &[String],
    model: &str,
    effort: Option<&str>,
) -> Result<Started, EkkoError> {
    let refused = EkkoError::InvalidInput;
    let Some(project) = location.project.as_ref() else {
        return Err(refused(
            "ekko agents works on a project's board, whose sessions work in worktrees of the project's repository; this is the default board".into(),
        ));
    };
    let Some(root) = project.root.as_deref() else {
        return Err(refused(format!("project {} has no folder of its own, so no repository for a session to work in", project.name)));
    };
    // The born session finds the board by the project's name: walking up
    // from a worktree leads to the repository's top, and misses a board in
    // a folder under it.
    if !crate::project::resolve_named(home, &project.name).is_ok_and(|found| same(&found.dir, &location.dir)) {
        return Err(refused(format!(
            "project {} is not registered under its name, which a session needs to reach its board: ekko init {} registers it",
            project.name,
            root.display()
        )));
    }
    ekko.storage.writable()?;
    let all = ekko.storage.get_shared()?;
    let ids = ekko.validate_ids(raw_ids, &all)?;
    for id in &ids {
        refusal(&all[id], &all).map_err(refused)?;
    }

    let environment = starting_environment().map_err(refused)?;
    let mux = Mux::new(environment.clone());
    for name in mux.windows().map_err(refused)? {
        if let Some(id) = ids.iter().find(|id| tasks_of(&name).contains(id)) {
            return Err(refused(format!("task {id} has a window already, {name} in session {SESSION}: step into it there")));
        }
    }
    let branch = format!("task-{}", joined(&ids, "-"));
    let worktree = worktree(root, &branch).map_err(refused)?;
    let real_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let cwd = match real_root.strip_prefix(&worktree.top) {
        Ok(under) if !under.as_os_str().is_empty() => worktree.path.join(under),
        _ => worktree.path.clone(),
    };
    let window = format!("{} \u{b7} {model}", joined(&ids, ","));
    let uids: Vec<String> = ids.iter().map(|id| all[id].uid.clone().unwrap_or_else(|| id.to_string())).collect();
    let argv = command_line(&environment, &mux.binary, &uids.join(","), &project.name, model, effort, &first_prompt(&ids, &branch, &worktree.path));
    let pane = mux.open(&window, &cwd, &argv).map_err(refused)?;
    let title = match ids.as_slice() {
        [] => String::new(),
        [first] => crate::ekko::title(&all[first].description).to_string(),
        [first, rest @ ..] => format!("{} (+{})", crate::ekko::title(&all[first].description), rest.len()),
    };
    let mut drawn = vec![
        set_pane(&pane, TASKS_OPTION, &joined(&ids, ",")),
        set_pane(&pane, TITLE_OPTION, &title),
        set_pane(&pane, MODEL_OPTION, model),
        effort.map_or_else(|| unset_pane(&pane, EFFORT_OPTION), |effort| set_pane(&pane, EFFORT_OPTION, effort)),
    ];
    drawn.extend(state_commands(&pane, WORKING, None));
    for option in ["window-status-format", "window-status-current-format"] {
        drawn.push(["set-option", "-w", "-t", &pane, option, STATUS_FORMAT].map(str::to_string).to_vec());
    }
    let undrawn = mux.batch(&drawn).err();
    std::thread::sleep(SETTLE);
    if !mux.alive(&pane) {
        return Err(refused(format!(
            "the session opened in window {window} ended within {} s: {} did not start there; run it in {} to see why",
            SETTLE.as_secs(),
            claude().to_string_lossy(),
            cwd.display()
        )));
    }
    Ok(Started { tasks: ids, window, pane, worktree: worktree.path, branch, made: worktree.made, attach: mux.attach(), undrawn })
}

/// The command that sets the user option `option` of `pane` to `value`, as
/// a status line or a border shows it (`shown`).
fn set_pane(pane: &str, option: &str, value: &str) -> Vec<String> {
    ["set-option", "-p", "-t", pane, option].map(str::to_string).into_iter().chain([shown(value)]).collect()
}

/// The command that unsets the user option `option` of `pane`.
fn unset_pane(pane: &str, option: &str) -> Vec<String> {
    ["set-option", "-p", "-u", "-t", pane, option].map(str::to_string).to_vec()
}

/// The commands that set `pane`'s state, since now, and what it waits on --
/// or that it waits on nothing.
fn state_commands(pane: &str, state: &str, waits: Option<&str>) -> Vec<Vec<String>> {
    let since = chrono::Local::now().timestamp().to_string();
    vec![
        set_pane(pane, STATE_OPTION, state),
        set_pane(pane, SINCE_OPTION, &since),
        match waits {
            Some(what) => set_pane(pane, WAITS_OPTION, what),
            None => unset_pane(pane, WAITS_OPTION),
        },
    ]
}

/// `value` as a status line or a pane's border shows it, kept as given: on
/// one line, its `#` doubled, which would start a style or a variable there,
/// and a `;` at its end escaped, which tmux would take for the end of the
/// command (both measured on tmux 3.7c and ztmux 3.7.47, task 1675).
fn shown(value: &str) -> String {
    let line = value.replace(['\n', '\r', '\t'], " ").replace('#', "##");
    match line.strip_suffix(';') {
        Some(rest) => format!("{rest}\\;"),
        None => line,
    }
}

/// Why `task` may not go to a born session now, if it may not: what the
/// page's Start refuses (`artifact::startable`), and for a task in progress,
/// who holds it and whether that one still runs.
fn refusal(task: &Item, all: &ItemMap) -> Result<(), String> {
    crate::artifact::startable(task, all).map_err(|why| match (&task.held_by, State::of(task)) {
        (Some(holder), Some(State::Progress)) => {
            let still = match holder.process() {
                None => "",
                Some(_) if holder.alive() => ", which still runs",
                Some(_) => ", which has ended: pause the task to hand it over",
            };
            format!("{why}, held by {}{still}", holder.label())
        }
        _ => why,
    })
}

/// The environment a born session starts from. Under a Claude Code session
/// -- the orchestrator, running this command -- the one that session's
/// process started with, what the user's launcher gave it, read from /proc:
/// without what Claude Code adds to each command it runs, which ties the
/// command to it (CLAUDECODE, CLAUDE_CODE_SESSION_ID, CLAUDE_PID and more,
/// which a version may add to) and has a Claude Code started there refuse to
/// run, nested in another. This command's own otherwise: a person's.
fn starting_environment() -> Result<Vec<(OsString, OsString)>, String> {
    if std::env::var_os("CLAUDECODE").is_none() {
        return Ok(std::env::vars_os().collect());
    }
    let pid = crate::holder::claude_among(std::os::unix::process::parent_id()).ok_or_else(|| {
        "this runs under a Claude Code session whose process is not among its ancestors, so the environment that session started with is not known".to_string()
    })?;
    let read = std::fs::read(format!("/proc/{pid}/environ"))
        .map_err(|error| format!("the environment Claude Code (pid {pid}) started with could not be read: {error}"))?;
    let environment: Vec<(OsString, OsString)> = read
        .split(|byte| *byte == 0)
        .filter_map(|entry| {
            let at = entry.iter().position(|byte| *byte == b'=').filter(|at| *at > 0)?;
            Some((OsStr::from_bytes(&entry[..at]).to_os_string(), OsStr::from_bytes(&entry[at + 1..]).to_os_string()))
        })
        .collect();
    if environment.is_empty() {
        return Err(format!("the environment Claude Code (pid {pid}) started with reads empty"));
    }
    Ok(environment)
}

/// What a born session's window runs: Claude Code through env(1), which
/// unsets CLAUDECODE and every other variable this command has and
/// `environment` has not -- a multiplexer's server started by a session's
/// command holds them all -- sets the Claude Code profile `environment`
/// names, CLAUDE_CONFIG_DIR, or unsets it for the default one, and names the
/// tasks, the project and the multiplexer's binary, for the hook that closes
/// the window. The rest is the multiplexer's, as in any window the user
/// opens there: no value but these goes on a command line, which any
/// process on the machine may read.
fn command_line(
    environment: &[(OsString, OsString)],
    mux: &OsStr,
    uids: &str,
    project: &str,
    model: &str,
    effort: Option<&str>,
    prompt: &str,
) -> Vec<OsString> {
    let started: HashSet<&OsStr> = environment.iter().map(|(name, _)| name.as_os_str()).collect();
    let mut unset: Vec<OsString> = std::env::vars_os().map(|(name, _)| name).filter(|name| !started.contains(name.as_os_str())).collect();
    unset.push("CLAUDECODE".into());
    let profile = environment.iter().find(|(name, _)| name == "CLAUDE_CONFIG_DIR").map(|(_, dir)| dir);
    if profile.is_none() {
        unset.push("CLAUDE_CONFIG_DIR".into());
    }
    unset.sort();
    unset.dedup();
    // env(1) reads its options up to the first assignment: every -u first.
    let mut argv: Vec<OsString> = vec!["env".into()];
    for name in unset {
        argv.push("-u".into());
        argv.push(name);
    }
    if let Some(dir) = profile {
        let mut set = OsString::from("CLAUDE_CONFIG_DIR=");
        set.push(dir);
        argv.push(set);
    }
    argv.push(format!("{TASKS_VAR}={uids}").into());
    argv.push(format!("EKKO_PROJECT={project}").into());
    let mut binary = OsString::from("EKKO_MUX=");
    binary.push(mux);
    argv.push(binary);
    argv.push(claude());
    argv.extend(["--model".into(), model.into()]);
    if let Some(effort) = effort {
        argv.extend(["--effort".into(), effort.into()]);
    }
    argv.push(prompt.into());
    argv
}

/// The Claude Code a born session runs: `EKKO_CLAUDE`, else `claude` as the
/// window's PATH finds it -- the user's own launcher, with their settings.
fn claude() -> OsString {
    set("EKKO_CLAUDE").unwrap_or_else(|| "claude".into())
}

/// A born session's first prompt: what it was started for, that it claims
/// its tasks before it changes anything (design item 1), and how it ends --
/// a closing note on each task and the task done, or else a handoff and the
/// task back to pending (design item 3). Each note is named by the tool and
/// argument that write it: told only to attach a closing note, both
/// sessions of one live run appended it to the task's own text with edit
/// (task 1644), where the four sessions before them had attached one.
fn first_prompt(ids: &[u32], branch: &str, worktree: &Path) -> String {
    let (tasks, them, each, attached) = match ids {
        [id] => (format!("task {id}"), "it", format!("task {id}"), format!("attached_to {id}")),
        _ => (format!("tasks {}", listed(ids)), "them", "each task".to_string(), "attached_to that task".to_string()),
    };
    format!(
        "ekko agents started this session to do {tasks} on this project's ekko board, and nothing else. \
         Before you change anything, set {tasks} in progress with ekko's set_state, then read {them} in full with ekko's context. \
         Work in this worktree, {}, on its branch {branch}, and commit there; do not push, merge into another branch, release, or start other sessions. \
         When {each} is done, write its closing note with ekko's create, {attached} -- a note of its own, not an edit of the task's text: \
         what you did, the files you changed, the tests you ran and their outcomes, the commit, and what is left -- then set it done. \
         If you stop short of that, write a handoff instead, with ekko's create, kind handoff, {attached}, and set the task back to unstarted.",
        worktree.display()
    )
}

/// `1`, `1 and 2`, `1, 2 and 3`.
fn listed(ids: &[u32]) -> String {
    match ids.split_last() {
        Some((last, [])) => last.to_string(),
        Some((last, rest)) => format!("{} and {last}", joined(rest, ", ")),
        None => String::new(),
    }
}

fn joined(ids: &[u32], between: &str) -> String {
    ids.iter().map(u32::to_string).collect::<Vec<_>>().join(between)
}

/// The tasks a window's name says it was opened for: the ids before its
/// first ` · `, as `start` names it; none for a name it did not give.
fn tasks_of(name: &str) -> Vec<u32> {
    let Some((ids, _)) = name.split_once(" \u{b7} ") else { return Vec::new() };
    ids.split(',').map(|id| id.trim().parse::<u32>()).collect::<Result<_, _>>().unwrap_or_default()
}

/// The multiplexer, spoken to in tmux's command language (design item 8):
/// the binary `EKKO_MUX` names, `tmux` otherwise, on the server some
/// arguments name -- `-L <name>`, `-S <path>` -- or else the one it finds
/// itself: the server this command runs inside, or the user's default.
pub(crate) struct Mux {
    binary: OsString,
    server: Vec<OsString>,
    /// The environment it runs with; this process's own when `None`.
    environment: Option<Vec<(OsString, OsString)>>,
}

impl Mux {
    /// The one `start` speaks to, on the socket `EKKO_MUX_SOCKET` names, as
    /// `-L` takes it. It runs with the environment born sessions start
    /// from, so a server it starts hands that one to every window.
    fn new(environment: Vec<(OsString, OsString)>) -> Mux {
        let server = set("EKKO_MUX_SOCKET").map(|socket| vec!["-L".into(), socket]).unwrap_or_default();
        Mux { binary: set("EKKO_MUX").unwrap_or_else(|| "tmux".into()), server, environment: Some(environment) }
    }

    /// The one `start` speaks to, as this process's environment has it:
    /// inside a pane, without `EKKO_MUX_SOCKET`, the server of that pane, as
    /// tmux finds it from TMUX (task 1676).
    pub(crate) fn here() -> Mux {
        let server = set("EKKO_MUX_SOCKET").map(|socket| vec!["-L".into(), socket]).unwrap_or_default();
        Mux { binary: set("EKKO_MUX").unwrap_or_else(|| "tmux".into()), server, environment: None }
    }

    /// The arguments that run it on its server: the binary, then `-L` or
    /// `-S` and the socket, if one is named.
    pub(crate) fn argv(&self) -> Vec<OsString> {
        std::iter::once(self.binary.clone()).chain(self.server.iter().cloned()).collect()
    }

    /// The panes of the session `ekko` that `start` opened, each with what it
    /// keeps there for the views (task 1675); none without the session.
    pub(crate) fn born_panes(&self) -> Result<Vec<BornPane>, String> {
        if !self.has_session()? {
            return Ok(Vec::new());
        }
        let fields = ["#{pane_id}", "#{pane_dead}", "#{window_name}"]
            .into_iter()
            .map(str::to_string)
            .chain([TASKS_OPTION, TITLE_OPTION, MODEL_OPTION, EFFORT_OPTION, STATE_OPTION, SINCE_OPTION, WAITS_OPTION].map(|option| format!("#{{{option}}}")))
            .collect::<Vec<_>>()
            .join("\t");
        let output = self.run(&["list-panes", "-s", "-t", &format!("={SESSION}"), "-F", &fields])?;
        if !output.status.success() {
            return Err(self.failed("list-panes", &output));
        }
        Ok(String::from_utf8_lossy(&output.stdout).lines().filter_map(BornPane::parse).collect())
    }

    /// The one whose pane this process runs in, as the server tells its
    /// panes: the socket before the first comma of TMUX.
    fn of_this_pane() -> Option<Mux> {
        let tmux = std::env::var_os("TMUX")?;
        let socket = tmux.as_bytes().split(|byte| *byte == b',').next().filter(|socket| !socket.is_empty())?;
        let server = vec!["-S".into(), OsStr::from_bytes(socket).to_os_string()];
        Some(Mux { binary: set("EKKO_MUX").unwrap_or_else(|| "tmux".into()), server, environment: None })
    }

    pub(crate) fn run<S: AsRef<OsStr>>(&self, args: &[S]) -> Result<Output, String> {
        let mut command = Command::new(&self.binary);
        command.args(&self.server).args(args).stdin(Stdio::null());
        if let Some(environment) = &self.environment {
            command.env_clear().envs(environment.iter().map(|(name, value)| (name, value)));
        }
        command.output().map_err(|error| format!("{} could not be run: {error}", self.binary.to_string_lossy()))
    }

    pub(crate) fn failed(&self, what: &str, output: &Output) -> String {
        format!("{} {what} failed: {}", self.binary.to_string_lossy(), String::from_utf8_lossy(&output.stderr).trim())
    }

    /// Runs `commands` in one call, as tmux runs a sequence: each separated
    /// from the next by a `;` of its own, in order, up to the first that
    /// fails.
    fn batch(&self, commands: &[Vec<String>]) -> Result<(), String> {
        let mut args: Vec<&str> = Vec::new();
        for command in commands {
            if !args.is_empty() {
                args.push(";");
            }
            args.extend(command.iter().map(String::as_str));
        }
        let output = self.run(&args)?;
        if !output.status.success() {
            return Err(self.failed("set-option", &output));
        }
        Ok(())
    }

    fn has_session(&self) -> Result<bool, String> {
        Ok(self.run(&["has-session", "-t", &format!("={SESSION}")])?.status.success())
    }

    /// The names of the windows in the session `ekko`; none without it.
    fn windows(&self) -> Result<Vec<String>, String> {
        if !self.has_session()? {
            return Ok(Vec::new());
        }
        let output = self.run(&["list-windows", "-t", &format!("={SESSION}"), "-F", "#{window_name}"])?;
        if !output.status.success() {
            return Err(self.failed("list-windows", &output));
        }
        Ok(String::from_utf8_lossy(&output.stdout).lines().map(str::to_string).collect())
    }

    /// Opens the window `name`, in the folder `cwd`, running `argv` without
    /// a shell, in the session `ekko`, which it starts if it is not there;
    /// not made the session's current window, so the user's view stays.
    /// Its pane's id.
    fn open(&self, name: &str, cwd: &Path, argv: &[OsString]) -> Result<String, String> {
        let mut args: Vec<OsString> = if self.has_session()? {
            ["new-window", "-d", "-t", &format!("={SESSION}:")].map(OsString::from).to_vec()
        } else {
            ["new-session", "-d", "-s", SESSION].map(OsString::from).to_vec()
        };
        args.extend(["-n", name, "-c"].map(OsString::from));
        args.push(cwd.into());
        args.extend(["-P", "-F", "#{pane_id}"].map(OsString::from));
        args.extend(argv.iter().cloned());
        let output = self.run(&args)?;
        let pane = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !output.status.success() || pane.is_empty() {
            return Err(self.failed("could not open a window, and", &output));
        }
        Ok(pane)
    }

    /// The panes on its server, each as its id and whether it is dead: `%3
    /// 0` runs, `%3 1` is one a remain-on-exit keeps. tmux's
    /// display-message answers for a pane that is gone as for one that runs
    /// (gotcha 1637); this list tells them apart.
    fn panes(&self) -> Result<Vec<String>, String> {
        let output = self.run(&["list-panes", "-a", "-F", "#{pane_id} #{pane_dead}"])?;
        if !output.status.success() {
            return Err(self.failed("list-panes", &output));
        }
        Ok(String::from_utf8_lossy(&output.stdout).lines().map(str::to_string).collect())
    }

    /// Whether `pane` still runs; not when no server answers.
    fn alive(&self, pane: &str) -> bool {
        let running = format!("{pane} 0");
        self.panes().is_ok_and(|panes| panes.contains(&running))
    }

    /// Whether `pane` is on its server, running or dead; `Err` with what
    /// the server said when none answers, which takes its panes with it.
    fn listed(&self, pane: &str) -> Result<bool, String> {
        Ok(self.panes()?.iter().any(|line| line.split(' ').next() == Some(pane)))
    }

    /// The name of the window `pane` is in.
    fn window_name(&self, pane: &str) -> Result<String, String> {
        let output = self.run(&["display-message", "-p", "-t", pane, "#{window_name}"])?;
        if !output.status.success() {
            return Err(self.failed("display-message", &output));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim_end_matches('\n').to_string())
    }

    /// Names the window `pane` is in `name`.
    fn rename(&self, pane: &str, name: &str) -> Result<(), String> {
        let output = self.run(&["rename-window", "-t", pane, name])?;
        if !output.status.success() {
            return Err(self.failed("rename-window", &output));
        }
        Ok(())
    }

    fn attach(&self) -> String {
        let binary = Path::new(&self.binary).file_name().unwrap_or(self.binary.as_os_str()).to_string_lossy().into_owned();
        let server: String = self.server.iter().map(|arg| format!(" {}", arg.to_string_lossy())).collect();
        format!("{binary}{server} attach -t {SESSION}")
    }
}

/// A pane of the session `ekko` that `start` opened, as `born_panes` reads
/// it: what its options say, `#` no longer doubled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BornPane {
    pub pane: String,
    pub dead: bool,
    pub window: String,
    pub tasks: Vec<u32>,
    pub title: String,
    pub model: String,
    pub effort: Option<String>,
    pub state: String,
    /// Since when it is in its state, in seconds since the epoch.
    pub since: Option<i64>,
    pub waits: Option<String>,
}

impl BornPane {
    /// A line of `born_panes`' list, its fields apart by tabs, which no
    /// value holds (`shown`); none for a pane `start` did not open.
    fn parse(line: &str) -> Option<BornPane> {
        let fields: Vec<&str> = line.split('\t').collect();
        let [pane, dead, window, tasks, title, model, effort, state, since, waits] = fields.as_slice() else { return None };
        let tasks: Vec<u32> = tasks.split(',').map(|id| id.trim().parse::<u32>()).collect::<Result<_, _>>().unwrap_or_default();
        if tasks.is_empty() {
            return None;
        }
        let given = |value: &str| Some(value.replace("##", "#")).filter(|value| !value.is_empty());
        Some(BornPane {
            pane: pane.to_string(),
            dead: *dead == "1",
            window: window.to_string(),
            tasks,
            title: given(title).unwrap_or_default(),
            model: model.to_string(),
            effort: given(effort),
            state: state.to_string(),
            since: since.parse().ok(),
            waits: given(waits),
        })
    }
}

/// What born sessions did today, by `born.log`: each line written since
/// midnight, local time.
pub(crate) fn born_today(home: &Path) -> Vec<String> {
    let today = chrono::Local::now().format("%Y-%m-%dT").to_string();
    std::fs::read_to_string(born_dir(home).join("born.log")).unwrap_or_default().lines().filter(|line| line.starts_with(&today)).map(str::to_string).collect()
}

/// The value of the variable `name`, unless it is unset or empty.
fn set(name: &str) -> Option<OsString> {
    std::env::var_os(name).filter(|value| !value.is_empty())
}

/// A born session's worktree: where it is, the top of its repository, and
/// whether this start made it.
struct Worktree {
    path: PathBuf,
    top: PathBuf,
    made: bool,
}

/// The worktree a born session works in: `.claude/worktrees/<branch>` at the
/// top of the repository holding `root`, where Claude Code's own
/// `--worktree` puts its own, on the branch `<branch>`. Made from the HEAD
/// of that checkout, unpushed commits and all -- Claude Code's default
/// branches from the remote's -- or, where an earlier start left one for
/// the branch, that one: the work of a session that stopped short goes on.
fn worktree(root: &Path, branch: &str) -> Result<Worktree, String> {
    let top = git(root, &["rev-parse", "--show-toplevel"])
        .map_err(|why| format!("{} is in no git repository, and a born session works in a worktree of its own: {why}", root.display()))?;
    let top = PathBuf::from(top);
    let path = top.join(".claude").join("worktrees").join(branch);
    let listed = git(&top, &["worktree", "list", "--porcelain"])?;
    for entry in listed.split("\n\n") {
        let field = |key: &str| entry.lines().find_map(|line| line.strip_prefix(key));
        if !field("worktree ").is_some_and(|at| same(Path::new(at), &path)) {
            continue;
        }
        let on = field("branch ").map(|on| on.trim_start_matches("refs/heads/"));
        return match on {
            Some(on) if on == branch && path.is_dir() => Ok(Worktree { path, top, made: false }),
            Some(on) if on == branch => Err(format!("{} is a worktree git still lists, and is gone: git worktree prune forgets it", path.display())),
            Some(on) => Err(format!("{} is a worktree on branch {on}, not {branch}", path.display())),
            None => Err(format!("{} is a worktree on no branch, not {branch}", path.display())),
        };
    }
    if path.exists() {
        return Err(format!("{} is there already, and is no worktree of {}", path.display(), top.display()));
    }
    let made = if git(&top, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")]).is_ok() {
        git(&top, &[OsStr::new("worktree"), OsStr::new("add"), path.as_os_str(), OsStr::new(branch)])
    } else {
        git(&top, &[OsStr::new("worktree"), OsStr::new("add"), OsStr::new("-b"), OsStr::new(branch), path.as_os_str(), OsStr::new("HEAD")])
    };
    made.map_err(|why| format!("no worktree was made at {}: {why}", path.display()))?;
    Ok(Worktree { path, top, made: true })
}

/// `git -C dir args`: what it printed, trimmed, or else what it said.
fn git<S: AsRef<OsStr>>(dir: &Path, args: &[S]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("git could not be run: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// Whether two paths name the same place, through symlinks.
fn same(one: &Path, other: &Path) -> bool {
    let real = |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    real(one) == real(other)
}

/// The tasks this process acts for when a session ekko agents started runs
/// it -- EKKO_AGENT_TASK's uids, or display ids where a task has none --
/// and `None` anywhere else, the orchestrator included. Read where a
/// command, the MCP server or a hook starts, never by the board itself, so
/// a test run in a born session writes its own boards as before.
pub fn born_tasks() -> Option<Vec<String>> {
    let tasks: Vec<String> = std::env::var(TASKS_VAR).ok()?.split(',').map(str::trim).filter(|task| !task.is_empty()).map(str::to_string).collect();
    (!tasks.is_empty()).then_some(tasks)
}

/// What a session ekko agents started leaves to the orchestrator or the
/// user (task 1646, design item 7 of artifact 1640): the guard refuses its
/// Bash calls that do it, each as `limit_of` reads it, and its board refuses
/// it a write to another's task (`EkkoError::NotBornFor`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit {
    /// What sends its branch out: `git push`, and `gh pr create` or `merge`.
    Push,
    /// What releases or deploys: a `gh release` that writes, a `publish` of
    /// `cargo`, `npm`, `pnpm` or `yarn`, `nixos-rebuild switch`, `boot` or
    /// `test`, and a program whose name starts with `release`, run itself
    /// or by a shell, as `scripts/release.sh` is.
    Release,
    /// What starts or opens a Claude Code session: `ekko agents start`, and
    /// `claude` but for `--version`, `--help` and the commands that start
    /// none (`NO_SESSION`).
    Session,
}

impl Limit {
    /// Why a born session leaves it to another, as its refusal says.
    fn why(self) -> &'static str {
        match self {
            Limit::Push => "does not push or open a pull request: its branch is the orchestrator's to merge, and what reaches a remote is the user's to send",
            Limit::Release => "does not release or deploy: that waits on the user's word",
            Limit::Session => "does not start sessions: the orchestrator starts them, each with a context and a cost of its own",
        }
    }
}

/// The options `claude` takes a value after, as `claude --help` lists them
/// (Claude Code 2.1.295): never its command, nor a prompt.
const CLAUDE_VALUED: &[&str] = &[
    "--add-dir", "--agent", "--agents", "--allowedTools", "--allowed-tools", "--append-system-prompt", "--autocompact", "--betas",
    "--debug-file", "--disallowedTools", "--disallowed-tools", "--effort", "--environment", "--fallback-model", "--file",
    "--input-format", "--json-schema", "--max-budget-usd", "--mcp-config", "--model", "-n", "--name", "--output-format",
    "--permission-mode", "--permission-prompts", "--plugin-dir", "--plugin-url", "--remote-control-session-name-prefix",
    "--session-id", "--setting-sources", "--settings", "--system-prompt", "--system-prompt-snapshot", "--tools",
];

/// Claude Code's commands that start or open no session, as `claude --help`
/// lists them (2.1.295). Its others -- `agents`, `attach`, `respawn`,
/// `ultrareview` -- start or open one, and any other first operand is a
/// prompt.
const NO_SESSION: &[&str] = &[
    "auth", "auto-mode", "doctor", "gateway", "import", "install", "logs", "mcp", "plugin", "plugins", "purge", "rm", "setup-token",
    "stop", "kill", "update", "upgrade",
];

/// The limit the call `call`, run in `folder`, reaches, if any.
pub fn limit_of(call: &crate::shell::Call, folder: &Path) -> Option<Limit> {
    let operands = |valued: &[&str]| crate::shell::operands(&call.args, valued);
    match call.name.as_str() {
        "git" => crate::claims::git_command(&call.args, folder).filter(|(git, _, _)| *git == "push").map(|_| Limit::Push),
        "gh" => match operands(&["-R", "--repo"]).as_slice() {
            ["pr", "create" | "merge", ..] => Some(Limit::Push),
            ["release", "create" | "upload" | "edit" | "delete" | "delete-asset", ..] => Some(Limit::Release),
            _ => None,
        },
        "cargo" => {
            let operands = operands(&["--manifest-path", "-Z", "--config", "-C", "--color"]);
            (operands.iter().find(|operand| !operand.starts_with('+')) == Some(&"publish")).then_some(Limit::Release)
        }
        // `yarn npm publish` and `npm run publish` too.
        "npm" | "pnpm" | "yarn" => operands(&[]).iter().take(2).any(|operand| *operand == "publish").then_some(Limit::Release),
        "nixos-rebuild" => {
            let operands = operands(&["--flake", "-I", "--target-host", "--build-host", "--profile-name", "-p", "--specialisation", "-c"]);
            operands.iter().any(|operand| matches!(*operand, "switch" | "boot" | "test")).then_some(Limit::Release)
        }
        "ekko" => operands(&[]).starts_with(&["agents", "start"]).then_some(Limit::Session),
        "claude" => {
            let asks = call.args.iter().any(|arg| matches!(arg.as_str(), "--version" | "-v" | "--help" | "-h"));
            let command = operands(CLAUDE_VALUED).first().copied();
            (!asks && !command.is_some_and(|command| NO_SESSION.contains(&command))).then_some(Limit::Session)
        }
        "bash" | "sh" | "zsh" | "dash" => {
            let script = operands(&[]).first().map(|script| Path::new(script).file_name().unwrap_or_default().to_string_lossy().into_owned());
            script.filter(|script| script.starts_with("release")).map(|_| Limit::Release)
        }
        name if name.starts_with("release") => Some(Limit::Release),
        _ => None,
    }
}

/// Why the guard refuses the Bash call `command`, run from `cwd`, in a
/// session ekko agents started: a reason for each limit its calls reach,
/// read as the guard reads calls (`crate::shell`); none for a call that
/// reaches none, or that only names one -- in an echo, a commit message, a
/// here-document.
pub fn limits(command: &str, cwd: &Path) -> Vec<String> {
    let (calls, placed) = crate::shell::located(command, cwd, crate::shell::Reading::GUARD);
    let mut found: Vec<Limit> = Vec::new();
    for place in &placed {
        if let Some(limit) = limit_of(&calls[place.at], &place.folder).filter(|limit| !found.contains(limit)) {
            found.push(limit);
        }
    }
    found.iter().map(|limit| format!("{LIMIT_REASON}: this session, started for its tasks, {}.", limit.why())).collect()
}

/// How the reason a born session's limit refuses with opens.
pub const LIMIT_REASON: &str = "ekko agents";

/// The argument that runs ekko as a born session's closer, which its hook
/// starts: never typed, as `__ekko_clipboard_daemon` is not.
pub const CLOSER_ARG: &str = "__ekko_close";

/// How long the closer waits after a turn ends before it ends the session:
/// a prompt that reaches it meanwhile keeps it.
const GRACE: Duration = Duration::from_secs(1);

/// How long the closer waits for the session's process, then its pane, to
/// be gone.
const GONE_WITHIN: Duration = Duration::from_secs(10);

/// The argument that runs ekko as the watcher of a born session's
/// permission prompt, which its hook starts (task 1679): never typed.
pub const ANSWER_ARG: &str = "__ekko_answer";

/// How often the watcher reads Claude Code's record of the session.
const ANSWER_EVERY: Duration = Duration::from_millis(100);

/// What the hook keeps of a born session between its events, in ekko's
/// state directory: how many prompts reached it, whether one reached it
/// once its tasks were finished, what it waits on the user for, what the
/// permission it asks for is for, and the state its pane was last given.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Born {
    prompts: u64,
    talked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    waiting: Option<Waiting>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    asked: Option<String>,
    /// `STATE_OPTION`'s value as the hook last set it (task 1675), so the
    /// multiplexer is called only as it changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    state: Option<String>,
    /// Whether the last turn's end found background work in flight (task
    /// 1686): a turn over a minute ago is then no wait on the user, since
    /// the work's end wakes the session itself.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    background: bool,
    /// What a later version keeps here that this one does not know, written
    /// back as read; see `Item::unknown`.
    #[serde(flatten)]
    unknown: BTreeMap<String, serde_json::Value>,
}

/// A born session waiting on the user (task 1645): since when, for what,
/// and the name its window was given then, if it was.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Waiting {
    since: i64,
    what: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    window: Option<String>,
    /// What a later version keeps here that this one does not know, written
    /// back as read; see `Item::unknown`.
    #[serde(flatten)]
    unknown: BTreeMap<String, serde_json::Value>,
}

/// The notifications that say a born session waits on the user, as Claude
/// Code 2.1.295 names them (its hooks page): a permission prompt, or an MCP
/// server's dialog -- ekko's ask among them -- unanswered for about six
/// seconds, and a turn over a minute ago with nothing typed since.
const WAITS_ON_USER: &[&str] = &["permission_prompt", "elicitation_dialog", "elicitation_url_dialog", "idle_prompt"];

/// What a born session's window name ends with while it waits on the user.
pub const WAITING_ON_YOU: &str = " \u{b7} waiting on you";

/// The events of Claude Code the born hook acts on, each of which the
/// plugin runs it on: the prompts and the turn's end (task 1643), the
/// permission asked for, the notifications that the session waits on the
/// user, and the events that say it moved on (task 1645).
pub const EVENTS: &[&str] = &["UserPromptSubmit", "Stop", "PermissionRequest", "Notification", "PostToolUse", "PostToolUseFailure", "ElicitationResult"];

/// The hook of a born session (tasks 1643 and 1645), on each event of
/// `EVENTS`; nothing in a session `start` did not open, which has no
/// EKKO_AGENT_TASK. One run at a time for a session, as Claude Code fires
/// PostToolUse for tools that ran side by side at once.
///
/// A notification that the session waits on the user marks it so (design
/// item 4), with what it waits for -- for a permission, the tool and what
/// it was to do, as the request for it said: on the board, on each of its
/// tasks it holds, and in its window's name. Whichever event comes next --
/// a tool that ran once the user let it, the user's prompt, the turn's
/// end, a dialog answered -- says it moved on, and clears both; for a
/// permission prompt, the watcher it starts says so first, as the user
/// answers (task 1679).
///
/// Each prompt is counted, and whether the session's tasks were all
/// finished -- done or cancelled -- when it came: the user talking to a
/// session whose work is over makes it theirs (design item 2). A prompt
/// typed while a turn runs counts when it is typed, which is when Claude
/// Code 2.1.295 fires the event for it, into the turn that runs. At the end
/// of a turn, a session whose tasks are all finished, that no prompt
/// reached since, with no background work in flight, is closed by a process
/// of its own, which outlives the hook.
pub fn hook(home: &Path, cwd: &Path, input: &str) -> std::process::ExitCode {
    let Some(tasks) = std::env::var(TASKS_VAR).ok().filter(|tasks| !tasks.trim().is_empty()) else {
        return std::process::ExitCode::SUCCESS;
    };
    let input: serde_json::Value = serde_json::from_str(input).unwrap_or_default();
    let session = input["session_id"].as_str().unwrap_or_default();
    let event = input["hook_event_name"].as_str().unwrap_or_default();
    if !EVENTS.contains(&event) {
        return std::process::ExitCode::SUCCESS;
    }
    let _alone = lock_born(home, session);
    let mut born = read_born(home, session);
    match event {
        // Kept for the notification that may follow, whose message names
        // no tool: "Claude needs your permission", in Claude Code 2.1.295.
        "PermissionRequest" => {
            born.asked = Some(asked_for(&input["tool_name"], &input["tool_input"]));
            write_born(home, session, &born);
            return std::process::ExitCode::SUCCESS;
        }
        "Notification" => {
            let kind = input["notification_type"].as_str().unwrap_or_default();
            if kind == "idle_prompt" && born.background && born.waiting.is_none() {
                log(home, session, &tasks, "does not wait on you (idle_prompt): background work was in flight at its turn's end");
                return std::process::ExitCode::SUCCESS;
            }
            if WAITS_ON_USER.contains(&kind) && born.waiting.is_none() {
                let mut what = input["message"].as_str().map(str::trim).filter(|what| !what.is_empty()).unwrap_or(kind).to_string();
                if let Some(asked) = born.asked.as_ref().filter(|_| kind == "permission_prompt") {
                    what = format!("{what} -- {asked}");
                }
                let what = crate::agent::clip(&what, 200);
                born.waiting = wait_on_user(home, cwd, &tasks, session, kind, &what);
                if born.waiting.is_some() {
                    show_state(home, session, &tasks, &mut born, WAITING, Some(&what));
                }
                write_born(home, session, &born);
                // No event comes as the user answers a permission prompt,
                // until the tool they let run ends (task 1679). Started once
                // the wait is written, which is what it watches.
                if born.waiting.is_some() && kind == "permission_prompt" {
                    let process = crate::holder::claude_among(std::os::unix::process::parent_id()).and_then(Process::of);
                    let watching = process.ok_or_else(|| "no Claude Code process is among its ancestors".to_string()).and_then(|process| spawn_watcher(session, &process));
                    if let Err(why) = watching {
                        log(home, session, &tasks, &format!("its answer is not watched: {why}"));
                    }
                }
            }
            return std::process::ExitCode::SUCCESS;
        }
        _ => {}
    }
    if born.waiting.is_some() || born.asked.is_some() {
        born.asked = None;
        if let Some(waiting) = born.waiting.take() {
            let process = crate::holder::claude_among(std::os::unix::process::parent_id()).and_then(Process::of);
            answered(home, cwd, &tasks, session, event, &waiting, process.as_ref());
        }
        write_born(home, session, &born);
    }
    match event {
        "UserPromptSubmit" => {
            born.prompts += 1;
            // Claude Code tells a session that its background work ended
            // with a prompt of its own, `<task-notification>` and the task
            // (2.1.295): no one talking to it (task 1688).
            let notice = input["prompt"].as_str().is_some_and(|prompt| prompt.trim_start().starts_with("<task-notification>"));
            if !notice && finished(home, cwd, &tasks).is_ok() && !born.talked {
                born.talked = true;
                log(home, session, &tasks, "a prompt reached it after its tasks were finished: it stays, the user's now");
            }
            show_state(home, session, &tasks, &mut born, WORKING, None);
            write_born(home, session, &born);
        }
        "Stop" => {
            let busy = |key: &str| input[key].as_array().is_some_and(|work| !work.is_empty());
            let background = busy("background_tasks") || busy("session_crons");
            let stays = match finished(home, cwd, &tasks) {
                Err(why) => Some(why),
                Ok(()) if born.talked => Some("a prompt reached it after its tasks were finished".to_string()),
                Ok(()) if background => Some("background work is in flight".to_string()),
                Ok(()) => None,
            };
            if let Some(why) = stays {
                log(home, session, &tasks, &format!("stays: {why}"));
                let shown = show_state(home, session, &tasks, &mut born, if background { WORKING } else { IDLE }, None);
                if shown || born.background != background {
                    born.background = background;
                    write_born(home, session, &born);
                }
                return std::process::ExitCode::SUCCESS;
            }
            let process = crate::holder::claude_among(std::os::unix::process::parent_id()).and_then(Process::of);
            let closing = process.ok_or_else(|| "no Claude Code process is among its ancestors".to_string()).and_then(|process| spawn_closer(session, born.prompts, &process));
            match closing {
                Ok(()) => {
                    log(home, session, &tasks, "its tasks are finished: closing");
                    println!("{}", serde_json::json!({"systemMessage": "ekko: this session's tasks are finished, so it closes now"}));
                }
                Err(why) => log(home, session, &tasks, &format!("not closed: {why}")),
            }
        }
        // A tool ran, or a dialog was answered: the turn goes on.
        _ => {
            if show_state(home, session, &tasks, &mut born, WORKING, None) {
                write_born(home, session, &born);
            }
        }
    }
    std::process::ExitCode::SUCCESS
}

/// Sets the state of the session's pane for the views (task 1675) --
/// `state`, since now, and what it waits on, if it does -- unless the hook
/// set it so last; in no multiplexer's pane, nothing. Whether it set it.
fn show_state(home: &Path, session: &str, tasks: &str, born: &mut Born, state: &str, waits: Option<&str>) -> bool {
    if born.state.as_deref() == Some(state) && waits.is_none() {
        return false;
    }
    let (Some(mux), Some(pane)) = (Mux::of_this_pane(), set("TMUX_PANE").map(|pane| pane.to_string_lossy().into_owned())) else {
        return false;
    };
    match mux.batch(&state_commands(&pane, state, waits)) {
        Ok(()) => {
            born.state = Some(state.to_string());
            true
        }
        Err(why) => {
            log(home, session, tasks, &format!("its pane's state, {state}, not set: {why}"));
            false
        }
    }
}

/// What a permission request is for, as a person reads it: the tool, and
/// the command, the file, the address or the pattern it was given, else
/// its input whole, on one line, clipped.
fn asked_for(tool: &serde_json::Value, input: &serde_json::Value) -> String {
    let tool = tool.as_str().unwrap_or("a tool");
    let given = ["command", "file_path", "notebook_path", "url", "path", "pattern"].iter().find_map(|key| input[key].as_str().map(str::to_string));
    let given = given.unwrap_or_else(|| if input.is_null() { String::new() } else { input.to_string() });
    let asked = if given.is_empty() { tool.to_string() } else { format!("{tool}: {given}") };
    crate::agent::clip(&asked, 100)
}

/// Marks that the session waits on the user, for `what`: on the board, on
/// each of its tasks its process holds in progress, and at the end of its
/// window's name. A turn over with nothing typed since is no wait on the
/// user while the session waits on the board, through ekko's wait: then
/// nothing is marked. What it marked, to clear.
fn wait_on_user(home: &Path, cwd: &Path, tasks: &str, session: &str, kind: &str, what: &str) -> Option<Waiting> {
    let since = chrono::Local::now().timestamp_millis();
    let board = match crate::holder::claude_among(std::os::unix::process::parent_id()).and_then(Process::of) {
        None => "not marked on the board: no Claude Code process is among its ancestors".to_string(),
        Some(process) => {
            let ekko = board(home, cwd);
            if kind == "idle_prompt" {
                if let Some(note) = ekko.as_ref().ok().and_then(|ekko| waits_on_board(ekko, &process)) {
                    log(home, session, tasks, &format!("does not wait on you ({kind}): it waits on the board, through note {note}"));
                    return None;
                }
            }
            let on = crate::holder::OnUser { since, what: what.to_string(), unknown: BTreeMap::new() };
            match ekko.and_then(|ekko| mark_holds(&ekko, tasks, &process, Some(on))) {
                Ok(ids) if ids.is_empty() => "no task of its own in progress to mark on the board".to_string(),
                Ok(ids) => format!("marked on task {}", listed(&ids)),
                Err(why) => format!("not marked on the board: {why}"),
            }
        }
    };
    let pane = Mux::of_this_pane().zip(set("TMUX_PANE").map(|pane| pane.to_string_lossy().into_owned()));
    let renamed = pane.ok_or_else(|| "in no multiplexer's pane".to_string()).and_then(|(mux, pane)| {
        let name = mux.window_name(&pane)?;
        if name.ends_with(WAITING_ON_YOU) {
            return Ok(name);
        }
        let renamed = format!("{name}{WAITING_ON_YOU}");
        mux.rename(&pane, &renamed).map(|()| renamed)
    });
    let window = match &renamed {
        Ok(name) => format!("its window named \"{name}\""),
        Err(why) => format!("its window not renamed: {why}"),
    };
    log(home, session, tasks, &format!("waits on you ({kind}): {what}; {board}; {window}"));
    Some(Waiting { since, what: what.to_string(), window: renamed.ok(), unknown: BTreeMap::new() })
}

/// Clears what `wait_on_user` marked, now that `event` says the session
/// moved on: on the board, for the tasks `process`, the session's Claude
/// Code, holds. A window the user renamed meanwhile keeps their name.
fn answered(home: &Path, cwd: &Path, tasks: &str, session: &str, event: &str, waiting: &Waiting, process: Option<&Process>) {
    let board = match process {
        None => "left on the board: no Claude Code process is among its ancestors".to_string(),
        Some(process) => match board(home, cwd).and_then(|ekko| mark_holds(&ekko, tasks, process, None)) {
            Ok(ids) if ids.is_empty() => "nothing to clear on the board".to_string(),
            Ok(ids) => format!("cleared on task {}", listed(&ids)),
            Err(why) => format!("left on the board: {why}"),
        },
    };
    let window = match (&waiting.window, Mux::of_this_pane(), set("TMUX_PANE")) {
        (None, _, _) => "its window was not renamed".to_string(),
        (Some(renamed), Some(mux), Some(pane)) => {
            let pane = pane.to_string_lossy();
            let named = renamed.strip_suffix(WAITING_ON_YOU).unwrap_or(renamed);
            match mux.window_name(&pane) {
                Ok(now) if now == *renamed => match mux.rename(&pane, named) {
                    Ok(()) => format!("its window named \"{named}\" again"),
                    Err(why) => format!("its window not renamed back: {why}"),
                },
                Ok(now) => format!("its window kept the name it was given meanwhile, \"{now}\""),
                Err(why) => format!("its window not renamed back: {why}"),
            }
        }
        (Some(_), _, _) => "its window not renamed back: in no multiplexer's pane".to_string(),
    };
    log(home, session, tasks, &format!("waits on you no more ({event}): {board}; {window}"));
}

/// The board this session works on: the project EKKO_PROJECT names, as
/// `start` gave it.
fn board(home: &Path, cwd: &Path) -> Result<Ekko, String> {
    let (ekko_dir, project) = (std::env::var("EKKO_DIR").ok(), std::env::var("EKKO_PROJECT").ok());
    let location = crate::directory::locate(home, cwd, None, ekko_dir.as_deref(), project.as_deref()).map_err(|error| EkkoError::from(error).to_string())?;
    Ekko::at(&location).map_err(|error| error.to_string())
}

/// The note through which `process` waits on an item, with ekko's wait,
/// while it does.
fn waits_on_board(ekko: &Ekko, process: &Process) -> Option<u32> {
    let all = ekko.storage.get_shared().ok()?;
    let mut waits: Vec<u32> = all
        .values()
        .filter(|note| note.trashed.is_none() && note.stashed.is_none())
        .filter(|note| note.wait.as_ref().is_some_and(|wait| wait.over.is_none() && wait.by.process().as_ref() == Some(process)))
        .map(|note| note.id)
        .collect();
    waits.sort_unstable();
    waits.first().copied()
}

/// Sets `on` -- what the session waits on the user for, or `None` to clear
/// it -- on each of the tasks `named` that `process` holds in progress, in
/// one write. A task marked already keeps the mark it has. The ids it
/// changed.
fn mark_holds(ekko: &Ekko, named: &str, process: &Process, on: Option<crate::holder::OnUser>) -> Result<Vec<u32>, String> {
    let _lock = ekko.storage.acquire_lock().map_err(|error| EkkoError::from(error).to_string())?;
    let before = ekko.storage.get_shared().map_err(|error| EkkoError::from(error).to_string())?;
    let mut data = ItemMap::clone(&before);
    let index = crate::ekko::uid_index(&before);
    let mut changed = Vec::new();
    for name in named.split(',').map(str::trim).filter(|name| !name.is_empty()) {
        let Some(task) = index.get(name).copied().or_else(|| name.parse::<u32>().ok()).and_then(|id| data.get_mut(&id)) else { continue };
        if State::of(task) != Some(State::Progress) {
            continue;
        }
        let Some(holder) = task.held_by.as_mut().filter(|holder| holder.process().as_ref() == Some(process)) else { continue };
        if holder.waits_on_user.is_some() == on.is_some() {
            continue;
        }
        holder.waits_on_user = on.clone();
        changed.push(task.id);
    }
    if !changed.is_empty() {
        ekko.save_against(&before, &mut data).map_err(|error| error.to_string())?;
    }
    changed.sort_unstable();
    Ok(changed)
}

/// Whether the tasks `named` -- EKKO_AGENT_TASK's uids, or display ids where
/// a task has none -- are all finished, done or cancelled, on the board this
/// session works on; why not, if not.
fn finished(home: &Path, cwd: &Path, named: &str) -> Result<(), String> {
    let all = board(home, cwd)?.storage.get_shared().map_err(|error| EkkoError::from(error).to_string())?;
    let index = crate::ekko::uid_index(&all);
    for name in named.split(',').map(str::trim).filter(|name| !name.is_empty()) {
        let id = index.get(name).copied().or_else(|| name.parse::<u32>().ok());
        let Some(task) = id.and_then(|id| all.get(&id)) else {
            return Err(format!("task {name} is not on its board"));
        };
        match State::of(task) {
            Some(State::Done | State::Cancelled) => {}
            Some(state) => return Err(format!("task {} is {}", task.id, state.word())),
            None => return Err(format!("{} is no task", task.id)),
        }
    }
    Ok(())
}

/// Starts the closer: ekko again, in a session of its own with none of the
/// hook's pipes, so that the hook returns at once and the closer outlives
/// it.
fn spawn_closer(session: &str, prompts: u64, process: &Process) -> Result<(), String> {
    detached(&[CLOSER_ARG, session, &prompts.to_string(), &process.pid.to_string(), &process.start.to_string()]).map_err(|why| format!("its closer did not start: {why}"))
}

/// Starts the watcher of a permission prompt, as `spawn_closer` starts the
/// closer.
fn spawn_watcher(session: &str, process: &Process) -> Result<(), String> {
    detached(&[ANSWER_ARG, session, &process.pid.to_string(), &process.start.to_string()]).map_err(|why| format!("its watcher did not start: {why}"))
}

/// Runs ekko with `args` in a session of its own, with none of this
/// process's pipes, so that it outlives the hook that starts it.
fn detached(args: &[&str]) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    let exe = std::env::current_exe().map_err(|error| format!("ekko's own binary is not found: {error}"))?;
    let mut command = Command::new(exe);
    command.args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    // SAFETY: the closure runs between fork and exec, and calls only setsid,
    // which is async-signal-safe.
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    command.spawn().map(drop).map_err(|error| error.to_string())
}

/// The watcher (ANSWER_ARG `<session> <pid> <start>`, task 1679): Claude
/// Code fires no hook as the user answers a permission prompt, and the
/// tool they let run may take minutes before PostToolUse says the session
/// moved on. Its own record of the session does say it at once:
/// `sessions/<pid>.json` in its config folder, which `claude agents --json`
/// reports, whose `status` read `waiting` while the prompt showed and
/// `busy` 17 to 26 ms after Enter answered it, in three runs of Claude
/// Code 2.1.295. Every 100 ms the watcher reads it; once it has said
/// `waiting` and then `busy` or `idle`, it clears the mark as the hook
/// would, and the pane's state. It ends then, or once the session waits no
/// more, the hook having cleared it, or its Claude Code has ended. A record
/// that does not say `waiting` first, as one of another version might,
/// leaves the mark to the hook.
pub fn answer(home: &Path, cwd: &Path, args: &[String]) -> std::process::ExitCode {
    use std::process::ExitCode;
    let tasks = std::env::var(TASKS_VAR).unwrap_or_default();
    let [session, pid, start] = args else { return ExitCode::FAILURE };
    let (Ok(pid), Ok(start), Some(boot)) = (pid.parse::<u32>(), start.parse::<u64>(), crate::holder::boot()) else {
        return ExitCode::FAILURE;
    };
    let process = Process { pid, start, boot };
    let mut seen_waiting = false;
    loop {
        if !running(&process) || read_born(home, session).waiting.is_none() {
            return ExitCode::SUCCESS;
        }
        let status = claude_status(home, session, pid);
        match status.as_deref() {
            Some("waiting") => seen_waiting = true,
            Some(now @ ("busy" | "idle")) if seen_waiting => {
                let _alone = lock_born(home, session);
                let mut born = read_born(home, session);
                if let Some(waiting) = born.waiting.take() {
                    born.asked = None;
                    answered(home, cwd, &tasks, session, &format!("answered: Claude Code's record says {now}"), &waiting, Some(&process));
                    show_state(home, session, &tasks, &mut born, if now == "idle" { IDLE } else { WORKING }, None);
                    write_born(home, session, &born);
                }
                return ExitCode::SUCCESS;
            }
            _ => {}
        }
        std::thread::sleep(ANSWER_EVERY);
    }
}

/// The `status` Claude Code's record of the session gives, if the record
/// at `sessions/<pid>.json`, under `CLAUDE_CONFIG_DIR` or else `~/.claude`,
/// is this session's.
fn claude_status(home: &Path, session: &str, pid: u32) -> Option<String> {
    let dir = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|dir| !dir.is_empty()).map_or_else(|| home.join(".claude"), PathBuf::from);
    let record: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("sessions").join(format!("{pid}.json"))).ok()?).ok()?;
    if record["sessionId"].as_str() != Some(session) || record["pid"].as_u64() != Some(u64::from(pid)) {
        return None;
    }
    record["status"].as_str().map(str::to_string)
}

/// The closer (CLOSER_ARG `<session> <prompts> <pid> <start>`): a second
/// after the turn ended, unless a prompt reached the session meanwhile or
/// its tasks are no longer all finished, ends its Claude Code with SIGTERM,
/// which ends it at once -- 5 ms, measured with Claude Code 2.1.295, and
/// its SessionEnd hooks do not run -- where /exit typed into its pane would
/// land on whatever the user has half typed there. Then it makes sure the
/// pane is gone, by the list of panes, and closes one a remain-on-exit
/// keeps.
pub fn close(home: &Path, cwd: &Path, args: &[String]) -> std::process::ExitCode {
    use std::process::ExitCode;
    let tasks = std::env::var(TASKS_VAR).unwrap_or_default();
    let [session, prompts, pid, start] = args else { return ExitCode::FAILURE };
    let (Ok(prompts), Ok(pid), Ok(start), Some(boot)) = (prompts.parse::<u64>(), pid.parse::<u32>(), start.parse::<u64>(), crate::holder::boot()) else {
        return ExitCode::FAILURE;
    };
    std::thread::sleep(GRACE);
    if read_born(home, session).prompts != prompts {
        log(home, session, &tasks, "stays: a prompt reached it as it was closing");
        return ExitCode::SUCCESS;
    }
    if let Err(why) = finished(home, cwd, &tasks) {
        log(home, session, &tasks, &format!("stays: {why}"));
        return ExitCode::SUCCESS;
    }
    let process = Process { pid, start, boot };
    if running(&process) {
        // SAFETY: a plain kill(2) of the process this pid names now, which
        // `running` has just found to be the session's own.
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
        if !within(GONE_WITHIN, || !running(&process)) {
            log(home, session, &tasks, &format!("not closed: its Claude Code, pid {pid}, still runs {} s after SIGTERM", GONE_WITHIN.as_secs()));
            return ExitCode::FAILURE;
        }
    }
    let (Some(mux), Some(pane)) = (Mux::of_this_pane(), set("TMUX_PANE").map(|pane| pane.to_string_lossy().into_owned())) else {
        log(home, session, &tasks, "closed: its Claude Code ended, in no multiplexer's pane");
        return ExitCode::SUCCESS;
    };
    if mux.listed(&pane) == Ok(true) {
        let _ = mux.run(&["kill-pane", "-t", &pane]);
    }
    let gone = within(GONE_WITHIN, || mux.listed(&pane) != Ok(true));
    match (gone, mux.listed(&pane)) {
        (true, Ok(_)) => log(home, session, &tasks, &format!("closed: its Claude Code ended, and its pane {pane} is gone")),
        (true, Err(why)) => log(home, session, &tasks, &format!("closed: its Claude Code ended, and its pane {pane} is gone with its server ({why})")),
        (false, _) => {
            log(home, session, &tasks, &format!("not closed: its Claude Code ended, and its pane {pane} is still listed"));
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}

/// Whether `process` still runs: alive, and not a zombie waiting for its
/// parent to reap it.
fn running(process: &Process) -> bool {
    let state = std::fs::read_to_string(format!("/proc/{}/stat", process.pid))
        .ok()
        .and_then(|stat| Some(stat.get(stat.rfind(')')? + 1..)?.split_whitespace().next()?.to_string()));
    process.alive() && state.is_some_and(|state| state != "Z")
}

/// Whether `done` holds within `limit`, asked every 50 ms.
fn within(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let until = std::time::Instant::now() + limit;
    loop {
        if done() {
            return true;
        }
        if std::time::Instant::now() >= until {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Where the hook keeps born sessions: `born/` in ekko's state directory, a
/// file for each session and `born.log`, what was decided and why.
fn born_dir(home: &Path) -> PathBuf {
    crate::agent::state_dir(home).join("born")
}

fn born_file(home: &Path, session: &str) -> PathBuf {
    let name: String = session.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
    born_dir(home).join(format!("{}.json", if name.is_empty() { "unknown" } else { &name }))
}

/// The lock that has the hook's runs for `session` take turns, held until
/// it is dropped; none where it cannot be taken, which leaves the runs as
/// they were before there was one.
fn lock_born(home: &Path, session: &str) -> Option<std::fs::File> {
    std::fs::create_dir_all(born_dir(home)).ok()?;
    crate::storage::lock_path(&born_file(home, session).with_extension("lock")).ok()
}

fn read_born(home: &Path, session: &str) -> Born {
    std::fs::read_to_string(born_file(home, session)).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

/// Written whole and put in place by a rename, for the closer, which reads
/// it from another process.
fn write_born(home: &Path, session: &str, born: &Born) {
    let file = born_file(home, session);
    let written = std::fs::create_dir_all(born_dir(home)).and_then(|()| {
        let fresh = file.with_extension(format!("json.{}", std::process::id()));
        std::fs::write(&fresh, serde_json::to_string(born).unwrap_or_default())?;
        std::fs::rename(&fresh, &file)
    });
    if let Err(error) = written {
        eprintln!("ekko: what this session's prompts were could not be kept: {error}");
    }
}

/// A line of `born.log`: when, the session, its tasks, and what was decided.
fn log(home: &Path, session: &str, tasks: &str, what: &str) {
    use std::io::Write as _;
    let line = format!("{} {} {tasks} {what}\n", chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.3f"), session.get(..8).unwrap_or(session));
    let _ = std::fs::create_dir_all(born_dir(home))
        .and_then(|()| std::fs::OpenOptions::new().create(true).append(true).open(born_dir(home).join("born.log")))
        .and_then(|mut file| file.write_all(line.as_bytes()));
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_names_its_tasks_before_its_first_dot() {
        assert_eq!(tasks_of("12 \u{b7} haiku"), vec![12]);
        assert_eq!(tasks_of("3,4 \u{b7} sonnet \u{b7} waiting on you"), vec![3, 4]);
        assert_eq!(tasks_of("12"), Vec::<u32>::new(), "a name start did not give");
        assert_eq!(tasks_of("notes \u{b7} haiku"), Vec::<u32>::new());
    }

    /// Each option `claude` takes a value after hides no command: what
    /// follows the value is, a command that starts no session or a prompt
    /// (task 1646).
    #[test]
    fn a_value_of_claude_s_options_is_never_taken_for_its_command() {
        let limit = |args: &[&str]| {
            let (calls, placed) = crate::shell::located(&format!("claude {}", args.join(" ")), Path::new("/"), crate::shell::Reading::GUARD);
            limit_of(&calls[placed[0].at], Path::new("/"))
        };
        for option in CLAUDE_VALUED {
            assert_eq!(limit(&[option, "value", "mcp", "list"]), None, "{option}");
            assert_eq!(limit(&[option, "value", "'write it'"]), Some(Limit::Session), "{option}");
        }
        for command in NO_SESSION {
            assert_eq!(limit(&[command]), None, "{command}");
        }
    }

    /// A value a pane holds for the views reads back as given in a status
    /// line or a border (task 1675): on one line, a `#` doubled, which
    /// would start a style there, and a `;` that ends it escaped, which
    /// tmux would take for the end of the command; a `;` inside is tmux's
    /// to keep.
    #[test]
    fn a_value_for_the_views_is_shown_as_given() {
        assert_eq!(shown("Fix #3 #[fg=red]"), "Fix ##3 ##[fg=red]");
        assert_eq!(shown("then ship;"), "then ship\\;");
        assert_eq!(shown(";"), "\\;");
        assert_eq!(shown("a ; b;c"), "a ; b;c");
        assert_eq!(shown("two\nlines\tand a tab"), "two lines and a tab");
        assert_eq!(shown("plain"), "plain");
    }

    #[test]
    fn ids_are_listed_as_a_sentence_says_them() {
        assert_eq!(listed(&[1]), "1");
        assert_eq!(listed(&[1, 2]), "1 and 2");
        assert_eq!(listed(&[1, 2, 3]), "1, 2 and 3");
    }

    /// What a permission is asked for reads as the tool and what it was
    /// given -- its command, file, address or pattern, else its input --
    /// on one line, and short.
    #[test]
    fn a_permission_is_asked_for_a_tool_and_what_it_was_given() {
        let asked = |tool: &str, input: serde_json::Value| asked_for(&serde_json::json!(tool), &input);
        assert_eq!(asked("Bash", serde_json::json!({"command": "date > stamp.txt", "description": "Write the date"})), "Bash: date > stamp.txt");
        assert_eq!(asked("Write", serde_json::json!({"file_path": "/a/b.rs", "content": "x"})), "Write: /a/b.rs");
        assert_eq!(asked("WebFetch", serde_json::json!({"url": "https://example.com", "prompt": "p"})), "WebFetch: https://example.com");
        assert_eq!(asked("mcp__ekko__trash", serde_json::json!({"items": [3]})), "mcp__ekko__trash: {\"items\":[3]}");
        assert_eq!(asked_for(&serde_json::Value::Null, &serde_json::Value::Null), "a tool");
        let long = asked("Bash", serde_json::json!({"command": format!("cat <<'EOF'\n{}\nEOF", "word ".repeat(40))}));
        assert!(!long.contains('\n') && long.ends_with(" chars)") && long.chars().count() < 120, "{long}");
    }

    /// The plugin runs the born hook on every event it acts on, for every
    /// tool where the event has tools, and on Notification for each kind
    /// that says the session waits on the user: an event or a kind left out
    /// of plugin.json is one the hook never hears of (task 1645).
    #[test]
    fn the_plugin_runs_the_born_hook_on_every_event_it_acts_on() {
        let plugin = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("plugin/.claude-plugin/plugin.json")).unwrap();
        let plugin: serde_json::Value = serde_json::from_str(&plugin).unwrap();
        let born = |entry: &&serde_json::Value| entry["hooks"].as_array().is_some_and(|hooks| hooks.iter().any(|hook| hook["command"] == "ekko --born --hook"));
        let mut events = Vec::new();
        for (event, entries) in plugin["hooks"].as_object().unwrap() {
            for entry in entries.as_array().unwrap().iter().filter(born) {
                events.push(event.as_str());
                let matcher = entry["matcher"].as_str();
                if event == "Notification" {
                    let mut kinds: Vec<&str> = matcher.unwrap_or_default().split('|').collect();
                    let mut waits = WAITS_ON_USER.to_vec();
                    kinds.sort_unstable();
                    waits.sort_unstable();
                    assert_eq!(kinds, waits, "the kinds of Notification the plugin sends the born hook");
                } else {
                    assert_eq!(matcher, None, "{event} reaches the born hook for some tools only");
                }
            }
        }
        let mut expected = EVENTS.to_vec();
        events.sort_unstable();
        expected.sort_unstable();
        assert_eq!(events, expected, "the events the plugin runs the born hook on");
    }
}
