//! `ekko agents start` (task 1642, artifact 1640): a session of Claude Code
//! born to do board tasks, opened in a window of its own in a terminal
//! multiplexer, with the model and effort chosen for those tasks, in a
//! worktree of its own. It lives for its tasks; the user sees its window,
//! and may step into it and talk to it. Its hook (task 1643) closes it once
//! its tasks are finished, unless the user talks to it after that.

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
    std::thread::sleep(SETTLE);
    if !mux.alive(&pane) {
        return Err(refused(format!(
            "the session opened in window {window} ended within {} s: {} did not start there; run it in {} to see why",
            SETTLE.as_secs(),
            claude().to_string_lossy(),
            cwd.display()
        )));
    }
    Ok(Started { tasks: ids, window, pane, worktree: worktree.path, branch, made: worktree.made, attach: mux.attach() })
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
/// task back to pending (design item 3).
fn first_prompt(ids: &[u32], branch: &str, worktree: &Path) -> String {
    let (tasks, them, each) = match ids {
        [id] => (format!("task {id}"), "it", format!("task {id}")),
        _ => (format!("tasks {}", listed(ids)), "them", "each task".to_string()),
    };
    format!(
        "ekko agents started this session to do {tasks} on this project's ekko board, and nothing else. \
         Before you change anything, set {tasks} in progress with ekko's set_state, then read {them} in full with ekko's context. \
         Work in this worktree, {}, on its branch {branch}, and commit there; do not push, merge into another branch, release, or start other sessions. \
         When {each} is done, attach a closing note to it -- what you did, the files you changed, the tests you ran and their outcomes, the commit, and what is left -- then set it done. \
         If you stop short of that, write a handoff on the task instead, and set it back to unstarted.",
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
struct Mux {
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

    /// The one whose pane this process runs in, as the server tells its
    /// panes: the socket before the first comma of TMUX.
    fn of_this_pane() -> Option<Mux> {
        let tmux = std::env::var_os("TMUX")?;
        let socket = tmux.as_bytes().split(|byte| *byte == b',').next().filter(|socket| !socket.is_empty())?;
        let server = vec!["-S".into(), OsStr::from_bytes(socket).to_os_string()];
        Some(Mux { binary: set("EKKO_MUX").unwrap_or_else(|| "tmux".into()), server, environment: None })
    }

    fn run<S: AsRef<OsStr>>(&self, args: &[S]) -> Result<Output, String> {
        let mut command = Command::new(&self.binary);
        command.args(&self.server).args(args).stdin(Stdio::null());
        if let Some(environment) = &self.environment {
            command.env_clear().envs(environment.iter().map(|(name, value)| (name, value)));
        }
        command.output().map_err(|error| format!("{} could not be run: {error}", self.binary.to_string_lossy()))
    }

    fn failed(&self, what: &str, output: &Output) -> String {
        format!("{} {what} failed: {}", self.binary.to_string_lossy(), String::from_utf8_lossy(&output.stderr).trim())
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

    fn attach(&self) -> String {
        let binary = Path::new(&self.binary).file_name().unwrap_or(self.binary.as_os_str()).to_string_lossy().into_owned();
        let server: String = self.server.iter().map(|arg| format!(" {}", arg.to_string_lossy())).collect();
        format!("{binary}{server} attach -t {SESSION}")
    }
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

/// The argument that runs ekko as a born session's closer, which its hook
/// starts: never typed, as `__ekko_clipboard_daemon` is not.
pub const CLOSER_ARG: &str = "__ekko_close";

/// How long the closer waits after a turn ends before it ends the session:
/// a prompt that reaches it meanwhile keeps it.
const GRACE: Duration = Duration::from_secs(1);

/// How long the closer waits for the session's process, then its pane, to
/// be gone.
const GONE_WITHIN: Duration = Duration::from_secs(10);

/// What the hook keeps of a born session between its events, in ekko's
/// state directory: how many prompts reached it, and whether one reached it
/// once its tasks were finished.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Born {
    prompts: u64,
    talked: bool,
    /// What a later version keeps here that this one does not know, written
    /// back as read; see `Item::unknown`.
    #[serde(flatten)]
    unknown: BTreeMap<String, serde_json::Value>,
}

/// The hook of a born session (task 1643), on Claude Code's
/// UserPromptSubmit and Stop; nothing in a session `start` did not open,
/// which has no EKKO_AGENT_TASK. Each prompt is counted, and whether the
/// session's tasks were all finished -- done or cancelled -- when it came:
/// the user talking to a session whose work is over makes it theirs (design
/// item 2). A prompt typed while a turn runs counts when it is typed, which
/// is when Claude Code 2.1.295 fires the event for it, into the turn that
/// runs. At the end of a turn, a session whose tasks are all finished, that
/// no prompt reached since, with no background work in flight, is closed by
/// a process of its own, which outlives the hook.
pub fn hook(home: &Path, cwd: &Path, input: &str) -> std::process::ExitCode {
    let Some(tasks) = std::env::var(TASKS_VAR).ok().filter(|tasks| !tasks.trim().is_empty()) else {
        return std::process::ExitCode::SUCCESS;
    };
    let input: serde_json::Value = serde_json::from_str(input).unwrap_or_default();
    let session = input["session_id"].as_str().unwrap_or_default();
    let finished = finished(home, cwd, &tasks);
    match input["hook_event_name"].as_str().unwrap_or_default() {
        "UserPromptSubmit" => {
            let mut born = read_born(home, session);
            born.prompts += 1;
            if finished.is_ok() && !born.talked {
                born.talked = true;
                log(home, session, &tasks, "a prompt reached it after its tasks were finished: it stays, the user's now");
            }
            write_born(home, session, &born);
        }
        "Stop" => {
            let born = read_born(home, session);
            let busy = |key: &str| input[key].as_array().is_some_and(|work| !work.is_empty());
            let stays = match finished {
                Err(why) => Some(why),
                Ok(()) if born.talked => Some("a prompt reached it after its tasks were finished".to_string()),
                Ok(()) if busy("background_tasks") || busy("session_crons") => Some("background work is in flight".to_string()),
                Ok(()) => None,
            };
            if let Some(why) = stays {
                log(home, session, &tasks, &format!("stays: {why}"));
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
        _ => {}
    }
    std::process::ExitCode::SUCCESS
}

/// Whether the tasks `named` -- EKKO_AGENT_TASK's uids, or display ids where
/// a task has none -- are all finished, done or cancelled, on the board this
/// session works on; why not, if not.
fn finished(home: &Path, cwd: &Path, named: &str) -> Result<(), String> {
    let (ekko_dir, project) = (std::env::var("EKKO_DIR").ok(), std::env::var("EKKO_PROJECT").ok());
    let location = crate::directory::locate(home, cwd, None, ekko_dir.as_deref(), project.as_deref()).map_err(|error| EkkoError::from(error).to_string())?;
    let all = Ekko::at(&location).and_then(|ekko| ekko.storage.get_shared().map_err(EkkoError::from)).map_err(|error| error.to_string())?;
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
    use std::os::unix::process::CommandExt;
    let exe = std::env::current_exe().map_err(|error| format!("ekko's own binary is not found: {error}"))?;
    let mut command = Command::new(exe);
    command
        .arg(CLOSER_ARG)
        .args([session.to_string(), prompts.to_string(), process.pid.to_string(), process.start.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: the closure runs between fork and exec, and calls only setsid,
    // which is async-signal-safe.
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    command.spawn().map(drop).map_err(|error| format!("its closer did not start: {error}"))
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

    #[test]
    fn ids_are_listed_as_a_sentence_says_them() {
        assert_eq!(listed(&[1]), "1");
        assert_eq!(listed(&[1, 2]), "1 and 2");
        assert_eq!(listed(&[1, 2, 3]), "1, 2 and 3");
    }
}
