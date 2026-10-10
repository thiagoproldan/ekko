//! `ekko agents start` (task 1642, artifact 1640): a session of Claude Code
//! born to do board tasks, opened in a window of its own in a terminal
//! multiplexer, with the model and effort chosen for those tasks, in a
//! worktree of its own. It lives for its tasks; the user sees its window,
//! and may step into it and talk to it.

use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Duration;

use crate::directory::Location;
use crate::ekko::{Ekko, EkkoError};
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
    let argv = command_line(&environment, &uids.join(","), &project.name, model, effort, &first_prompt(&ids, &branch, &worktree.path));
    let pane = mux.open(&window, &cwd, &argv).map_err(refused)?;
    std::thread::sleep(SETTLE);
    if !mux.alive(&pane).map_err(refused)? {
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
/// tasks and the project. The rest is the multiplexer's, as in any window
/// the user opens there: no value but these goes on a command line, which
/// any process on the machine may read.
fn command_line(
    environment: &[(OsString, OsString)],
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
    std::env::var_os("EKKO_CLAUDE").filter(|claude| !claude.is_empty()).unwrap_or_else(|| "claude".into())
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
/// the binary `EKKO_MUX` names, `tmux` otherwise, on the socket
/// `EKKO_MUX_SOCKET` names, as `-L` takes it, or else the one it finds
/// itself -- the server this command runs inside, or the user's default. It
/// runs with the environment born sessions start from, so a server it starts
/// hands that one to every window.
struct Mux {
    binary: OsString,
    socket: Option<OsString>,
    environment: Vec<(OsString, OsString)>,
}

impl Mux {
    fn new(environment: Vec<(OsString, OsString)>) -> Mux {
        let set = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
        Mux { binary: set("EKKO_MUX").unwrap_or_else(|| "tmux".into()), socket: set("EKKO_MUX_SOCKET"), environment }
    }

    fn run<S: AsRef<OsStr>>(&self, args: &[S]) -> Result<Output, String> {
        let mut command = Command::new(&self.binary);
        if let Some(socket) = &self.socket {
            command.arg("-L").arg(socket);
        }
        command
            .args(args)
            .env_clear()
            .envs(self.environment.iter().map(|(name, value)| (name, value)))
            .stdin(Stdio::null())
            .output()
            .map_err(|error| format!("{} could not be run: {error}", self.binary.to_string_lossy()))
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

    /// Whether `pane` still runs. tmux's display-message answers for a
    /// pane that is gone as for one that runs (gotcha 1637), so the list of
    /// panes is read, with whether each is dead, for a remain-on-exit that
    /// keeps a dead one listed.
    fn alive(&self, pane: &str) -> Result<bool, String> {
        let output = self.run(&["list-panes", "-a", "-F", "#{pane_id} #{pane_dead}"])?;
        let running = format!("{pane} 0");
        Ok(output.status.success() && String::from_utf8_lossy(&output.stdout).lines().any(|line| line == running))
    }

    fn attach(&self) -> String {
        let binary = Path::new(&self.binary).file_name().unwrap_or(self.binary.as_os_str()).to_string_lossy().into_owned();
        match &self.socket {
            Some(socket) => format!("{binary} -L {} attach -t {SESSION}", socket.to_string_lossy()),
            None => format!("{binary} attach -t {SESSION}"),
        }
    }
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
