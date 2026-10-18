//! `ekko agents merge <task>...` (task 1714, answer 1708, decision 1717):
//! the branch a born session committed its tasks' work on, merged into the
//! branch the project's checkout is on -- main -- as a merge queue takes a
//! pull request: rebased onto main, tested there, then fast-forwarded, one
//! merge at a time on a repository. It fails closed. A conflict, a test that
//! fails or work left uncommitted leaves main and the branch as they were
//! and hands the tasks back: a note on each says what failed and what to
//! do, and each is set back to pending, for its session or a new one to
//! rebase. It never pushes: what reaches a remote is the user's to send.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::agents::{self, Mux};
use crate::ekko::{Ekko, EkkoError};
use crate::item::State;
use crate::ops::{Create, Draft, Kind, Ref};

/// The lines of a failing test's output a note quotes, from its end.
const QUOTED_LINES: usize = 30;

/// The widest a quoted line of output is.
const QUOTED_WIDTH: usize = 200;

/// How often the born session's pane is looked for while it closes.
const LOOK_EVERY: Duration = Duration::from_millis(200);

/// How often `--wait` looks again once the session has run past
/// `agents::GONE_WITHIN`.
const WAIT_EVERY: Duration = Duration::from_secs(1);

/// The file, in the git directory of the worktree being merged, that says a
/// merge put it on a detached HEAD, and the branch's commit before: a merge
/// cut short -- killed by a timeout -- leaves it, and the next one puts the
/// worktree back on its branch.
const UNDERWAY: &str = "ekko-merge";

/// What `merge` did.
pub enum Outcome {
    Merged(Merged),
    /// The branch could not go in: main and the branch are as they were, and
    /// the tasks went back to their sessions.
    HandedBack(HandedBack),
}

pub struct Merged {
    pub tasks: Vec<u32>,
    pub branch: String,
    /// The branch it went into: the one the project's checkout is on.
    pub into: String,
    /// `into` before, and after: the same where there was nothing to merge.
    pub from: String,
    pub to: String,
    /// The branch's commits that `into` did not have.
    pub commits: usize,
    pub tested: Tested,
    /// The checks the merge was asked to run after the test (task 1831),
    /// each passed.
    pub checks: Vec<Checked>,
    /// What became of the branch's worktree and the branch.
    pub left: Left,
    pub notes: Vec<u32>,
}

/// How the tests ran, when they ran.
pub enum Tested {
    Passed { command: String, seconds: f64, log: PathBuf },
    /// Not run: the user said the project has no tests.
    Untested,
    /// Not run: nothing was merged.
    Unneeded,
}

/// A check a merge was asked to run (task 1831), and how it passed.
pub struct Checked {
    pub command: String,
    pub seconds: f64,
    pub log: PathBuf,
}

/// What a merge was asked for beyond its tasks.
#[derive(Default)]
pub struct Asked {
    /// Keep the branch's worktree and the branch once merged.
    pub keep: bool,
    /// Wait, with no bound, while a session of the tasks runs, rather than
    /// refuse once `agents::GONE_WITHIN` has passed (task 1829).
    pub wait: bool,
    /// Commands run after the test, in the branch's worktree rebased onto
    /// the branch it goes into, each to exit 0 for it to go in (task 1831):
    /// what the tasks' work needs checked against what it lands on, as a
    /// version above that branch's.
    pub checks: Vec<String>,
}

/// The test a rebased branch passes before it goes in, as the user set it
/// with `ekko agents test`.
pub enum Test {
    Command(String),
    /// The user said the project has none: a branch goes in untested.
    None,
}

/// The project's test, as `ekko agents test` set it; refused until it is
/// set, as going in untested is the user's to say.
pub fn test_command(ekko: &Ekko) -> Result<Test, EkkoError> {
    let settings = ekko.storage.get_agents()?;
    match settings.test {
        Some(command) => Ok(Test::Command(command)),
        None if settings.untested => Ok(Test::None),
        None => Err(EkkoError::InvalidInput(
            "No test command is set for this project, and ekko agents merge tests each branch, rebased, before it goes in: ekko agents \
             test --derive sets the one the repository declares, if it declares one; else the user sets one with ekko agents test \
             'COMMAND', or says the project has none with ekko agents test --none. Nothing was merged"
                .into(),
        )),
    }
}

/// What a merge leaves of the branch and its worktree.
pub enum Left {
    Removed,
    /// Kept as `--keep` asked.
    Kept,
    /// Kept, as git would not remove them: what it said.
    NotRemoved(String),
}

pub struct HandedBack {
    pub tasks: Vec<u32>,
    pub branch: String,
    pub into: String,
    /// What failed, in a line.
    pub why: String,
    pub notes: Vec<u32>,
    /// The tasks set back to pending.
    pub reopened: Vec<u32>,
    /// Why they were not, if the board refused it.
    pub not_reopened: Option<String>,
    /// The test's whole output, when a test failed.
    pub log: Option<PathBuf>,
}

/// Why a branch goes back to its tasks' sessions.
enum Back {
    Uncommitted(String),
    Conflict(Vec<String>),
    /// Conflict markers a commit of the rebased branch holds, as `commit
    /// subject: file:line` lines: a later commit may have removed them, so
    /// the tested tip does not show them.
    Markers(Vec<String>),
    Failed { command: String, code: String, seconds: f64, tail: String, log: PathBuf },
}

/// Merges the branch `ekko agents start` made for the tasks `raw_ids` --
/// `task-12`, `task-12-13` for two started together -- into the branch the
/// project's checkout is on, running `test` in the branch's worktree once it
/// is rebased; leaves the worktree and the branch, once merged, only where
/// `asked.keep` asks, and running `asked.checks` after the test, each a
/// gate as the test is (task 1831). Refused, changing nothing, for tasks
/// not all finished, or none done, for a branch that is not there, for a
/// session of theirs that still runs -- unless `asked.wait`, which waits
/// until it has gone, then reads the board again (task 1829) --, while
/// another merge runs on the repository, and for a task whose done
/// blocker's branch is not merged yet (task 1728).
pub fn merge(ekko: &Ekko, location: &crate::directory::Location, home: &Path, raw_ids: &[String], test: &Test, asked: &Asked) -> Result<Outcome, EkkoError> {
    let keep = asked.keep;
    let refused = EkkoError::InvalidInput;
    let Some(project) = location.project.as_ref() else {
        return Err(refused("ekko agents merge works on a project's board, whose sessions work in worktrees of the project's repository; this is the default board".into()));
    };
    let Some(root) = project.root.as_deref() else {
        return Err(refused(format!("project {} has no folder of its own, so no repository to merge in", project.name)));
    };
    let mut all = ekko.storage.get_shared()?;
    let ids = ekko.validate_ids(raw_ids, &all)?;
    let mut done = settled(&all, &ids).map_err(refused)?;
    let top = PathBuf::from(agents::git(root, &["rev-parse", "--show-toplevel"]).map_err(|why| refused(format!("{} is in no git repository: {why}", root.display())))?);
    let branch = format!("task-{}", agents::joined(&ids, "-"));
    if agents::git(&top, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")]).is_err() {
        return Err(refused(no_branch(&top, &branch, &ids)));
    }
    let uids: Vec<String> = ids.iter().map(|id| all[id].uid.clone().unwrap_or_else(|| id.to_string())).collect();
    if gone(home, &project.name, &ids, &uids, asked.wait).map_err(refused)? {
        // Its session may have changed the tasks before it went.
        all = ekko.storage.get_shared()?;
        done = settled(&all, &ids).map_err(refused)?;
    }
    let _lock = lock(&top).map_err(refused)?;
    let into = agents::git(&top, &["symbolic-ref", "--quiet", "--short", "HEAD"])
        .map_err(|_| refused(format!("the project's checkout, {}, is on no branch: ekko agents merge fast-forwards the branch it is on", top.display())))?;
    if into == branch {
        return Err(refused(format!("the project's checkout, {}, is on {branch} itself: put it back on the branch {branch} goes into", top.display())));
    }
    // Its blockers' work goes in first (task 1728).
    for id in &done {
        if let Some((blocker, theirs, made_for)) = agents::unmerged_blocker(&top, &into, &all[id], &all, &ids).map_err(refused)? {
            return Err(refused(format!(
                "task {id} waits on task {blocker}, whose work is on branch {theirs}, not in {into} yet: merge it first with ekko agents merge {}, \
                 so {into} takes task {id}'s work after it. {into} and {branch} are as they were",
                agents::joined(&made_for, " ")
            )));
        }
    }
    let worktree = worktree_of(root, &top, &branch).map_err(refused)?;
    let under = std::fs::canonicalize(root).ok().and_then(|real| real.strip_prefix(std::fs::canonicalize(&top).ok()?).ok().map(Path::to_path_buf));
    let folder = under.map_or_else(|| worktree.clone(), |under| worktree.join(under));

    let git = |args: &[&str]| agents::git(&worktree, args);
    let uncommitted = status(&worktree).map_err(refused)?;
    if !uncommitted.is_empty() {
        return hand_back(ekko, &ids, &done, &branch, &into, &worktree, test, Back::Uncommitted(uncommitted));
    }
    let old = git(&["rev-parse", &format!("refs/heads/{branch}")]).map_err(refused)?;
    let base = git(&["rev-parse", &format!("refs/heads/{into}")]).map_err(refused)?;
    let commits: usize = git(&["rev-list", "--count", &format!("{base}..{old}")]).map_err(refused)?.parse().unwrap_or(0);
    if commits == 0 {
        let left = leave(&top, &worktree, &branch, keep);
        let text = format!(
            "Nothing to merge: {into} has every commit of {branch} already, at {}. {}",
            short(&base),
            left_said(&left, &branch)
        );
        let notes = noted(ekko, &done, &text)?;
        return Ok(Outcome::Merged(Merged { tasks: ids, branch, into, from: base.clone(), to: base, commits, tested: Tested::Unneeded, checks: Vec::new(), left, notes }));
    }

    // Rebased on a detached HEAD, so the branch stays where its session left
    // it until main has its work.
    let underway = PathBuf::from(git(&["rev-parse", "--path-format=absolute", "--git-path", UNDERWAY]).map_err(refused)?);
    std::fs::write(&underway, format!("{old}\n")).map_err(|error| refused(format!("{} could not be written: {error}", underway.display())))?;
    let back_on_branch = |force: bool| {
        let put = if force { git(&["checkout", "--quiet", "--force", &branch]) } else { git(&["checkout", "--quiet", &branch]) };
        if put.is_ok() {
            let _ = std::fs::remove_file(&underway);
        }
        put
    };
    git(&["checkout", "--quiet", "--detach"]).map_err(refused)?;
    if let Err(why) = git(&["-c", "rebase.updateRefs=false", "rebase", "--quiet", "--no-autostash", &base]) {
        let conflicts: Vec<String> = git(&["diff", "--name-only", "--diff-filter=U"]).unwrap_or_default().lines().map(str::to_string).collect();
        if rebasing(&worktree) {
            git(&["rebase", "--abort"]).map_err(|abort| refused(format!("the rebase of {branch} stopped, and git rebase --abort failed in {}: {abort}", worktree.display())))?;
        }
        back_on_branch(false).map_err(refused)?;
        // Not the branch's to fix: an identity git lacks, a hook that refused.
        if conflicts.is_empty() {
            return Err(refused(format!("{branch} could not be rebased onto {into}, for a reason other than a conflict, so {into} and {branch} are as they were: {why}")));
        }
        return hand_back(ekko, &ids, &done, &branch, &into, &worktree, test, Back::Conflict(conflicts));
    }
    match markers(&worktree, &base) {
        Ok(found) if found.is_empty() => {}
        Ok(found) => {
            back_on_branch(true).map_err(refused)?;
            return hand_back(ekko, &ids, &done, &branch, &into, &worktree, test, Back::Markers(found));
        }
        Err(why) => {
            back_on_branch(true).map_err(refused)?;
            return Err(refused(format!("{into} was not moved, and {branch} is as its session left it: {why}")));
        }
    }
    let tested = match test {
        Test::Command(command) => {
            let ran = match run_test(home, &folder, &project.name, &branch, command) {
                Ok(ran) => ran,
                Err(why) => {
                    back_on_branch(true).map_err(refused)?;
                    return Err(refused(format!("{into} was not moved, and {branch} is as its session left it: {why}")));
                }
            };
            if !ran.passed {
                back_on_branch(true).map_err(refused)?;
                let back = Back::Failed { command: command.clone(), code: ran.code, seconds: ran.seconds, tail: ran.tail, log: ran.log };
                return hand_back(ekko, &ids, &done, &branch, &into, &worktree, test, back);
            }
            Tested::Passed { command: command.clone(), seconds: ran.seconds, log: ran.log }
        }
        Test::None => Tested::Untested,
    };
    let mut checks = Vec::new();
    for check in &asked.checks {
        let ran = match run_test(home, &folder, &project.name, &branch, check) {
            Ok(ran) => ran,
            Err(why) => {
                back_on_branch(true).map_err(refused)?;
                return Err(refused(format!("{into} was not moved, and {branch} is as its session left it: {why}")));
            }
        };
        if !ran.passed {
            back_on_branch(true).map_err(refused)?;
            let back = Back::Failed { command: check.clone(), code: ran.code, seconds: ran.seconds, tail: ran.tail, log: ran.log };
            return hand_back(ekko, &ids, &done, &branch, &into, &worktree, test, back);
        }
        checks.push(Checked { command: check.clone(), seconds: ran.seconds, log: ran.log });
    }
    let new = git(&["rev-parse", "HEAD"]).map_err(refused)?;
    if let Err(why) = fast_forward(&top, &into, &base, &new) {
        back_on_branch(true).map_err(refused)?;
        return Err(refused(format!("{into} was not moved, and {branch} is as its session left it: {why}")));
    }
    git(&["checkout", "--quiet", "-B", &branch]).map_err(refused)?;
    let _ = std::fs::remove_file(&underway);
    let left = leave(&top, &worktree, &branch, keep);
    let passed = match &tested {
        Tested::Passed { command, seconds: took, .. } => format!("passed `{command}` in {}", seconds(*took)),
        Tested::Untested | Tested::Unneeded => "untested, as the user said the project has no tests".to_string(),
    };
    let passed = match checks.as_slice() {
        [] => passed,
        _ => format!(
            "{passed}, and passed the check{} it was asked to run, {}",
            if checks.len() == 1 { "" } else { "s" },
            checks.iter().map(|check| format!("`{}` in {}", check.command, seconds(check.seconds))).collect::<Vec<_>>().join(", ")
        ),
    };
    let text = format!(
        "Merged into {into} by ekko agents merge: {commits} commit{} of {branch}, rebased onto {into} at {}, {passed}; {into} is at {} now.\n\n{}",
        if commits == 1 { "" } else { "s" },
        short(&base),
        short(&new),
        left_said(&left, &branch)
    );
    let notes = noted(ekko, &done, &text)?;
    Ok(Outcome::Merged(Merged { tasks: ids, branch, into, from: base, to: new, commits, tested, checks, left, notes }))
}

/// The tasks of `ids` that are done, once each is done or cancelled; why
/// not, where one is neither, or none is done.
fn settled(all: &crate::storage::ItemMap, ids: &[u32]) -> Result<Vec<u32>, String> {
    for id in ids {
        match State::of(&all[id]) {
            Some(State::Done | State::Cancelled) => {}
            Some(state) => return Err(format!("task {id} is {}: ekko agents merge takes the branch of tasks their session set done", state.word())),
            None => return Err(format!("{id} is a note, which no session is started for")),
        }
    }
    let done: Vec<u32> = ids.iter().copied().filter(|id| State::of(&all[id]) == Some(State::Done)).collect();
    if done.is_empty() {
        return Err(match ids {
            [id] => format!("task {id} is cancelled: no work of its goes in"),
            _ => format!("tasks {} are cancelled, none done: no work of theirs goes in", agents::listed(ids)),
        });
    }
    Ok(done)
}

/// Why there is no branch for the tasks: the branches of `ekko agents
/// start` that name any of them, if one does.
fn no_branch(top: &Path, branch: &str, ids: &[u32]) -> String {
    let others: Vec<String> = agents::git(top, &["for-each-ref", "--format=%(refname:short)", "refs/heads/task-*"])
        .unwrap_or_default()
        .lines()
        .filter(|other| {
            let named: Vec<u32> = other.trim_start_matches("task-").split('-').filter_map(|id| id.parse().ok()).collect();
            named.iter().any(|id| ids.contains(id))
        })
        .map(str::to_string)
        .collect();
    let started = "ekko agents start makes one for the tasks it starts together";
    match others.as_slice() {
        [] => format!("there is no branch {branch} in {}: {started}, and none of these tasks has one", top.display()),
        [other] => {
            let named = other.trim_start_matches("task-").replace('-', " ");
            format!("there is no branch {branch} in {}: {started}, and theirs is {other}: ekko agents merge {named}", top.display())
        }
        _ => format!("there is no branch {branch} in {}: {started}, and these name some of them: {}", top.display(), others.join(", ")),
    }
}

/// Whether every born session of the tasks -- `uids`, as born.log names
/// them -- has gone, waiting the while one takes to close itself once they
/// are done, or, with `wait`, as long as it runs (task 1829): whether one
/// ran when it looked. Why not, if one stays: it is someone's to talk to,
/// and its worktree is not to move under it; with why it stays, as the
/// born hook last said in born.log (task 1809).
fn gone(home: &Path, project: &str, ids: &[u32], uids: &[String], wait: bool) -> Result<bool, String> {
    let mux = Mux::here();
    let started = Instant::now();
    let mut ran = false;
    let mut said = String::new();
    loop {
        let panes = mux.born_panes().unwrap_or_default();
        let windows = mux.windows().unwrap_or_default();
        let running = match (agents::pane_of(project, ids, &panes), agents::window_of(project, ids, &windows, &panes)) {
            (Some((_, pane)), _) => format!("in pane {} of window {}", pane.pane, pane.window),
            (None, Some((_, window))) => format!("in window {window}"),
            (None, None) => return Ok(ran),
        };
        ran = true;
        if started.elapsed() < agents::GONE_WITHIN {
            std::thread::sleep(LOOK_EVERY);
            continue;
        }
        let why = match agents::born_said(home, uids) {
            Some(last) => format!("; born.log last said of it, at {last}"),
            None => "; born.log says nothing of it".to_string(),
        };
        if !wait {
            return Err(format!(
                "a session for these tasks still runs, {running} of session {}, {} s after this merge looked{why}. A session closes itself once its tasks \
                 are done and no one talks to it, and one someone talks to is theirs; merge once it has gone, or with --wait, which waits for it",
                agents::SESSION,
                agents::GONE_WITHIN.as_secs()
            ));
        }
        // Said once, and again as born.log says more.
        let now = format!("ekko agents merge waits: a session for these tasks still runs, {running} of session {}{why}", agents::SESSION);
        if now != said {
            eprintln!("{now}");
            said = now;
        }
        std::thread::sleep(WAIT_EVERY);
    }
}

/// The repository's lock for merges, in its git directory, which every
/// worktree shares: one merge at a time, so main moves once at a time.
fn lock(top: &Path) -> Result<std::fs::File, String> {
    use std::os::fd::AsRawFd as _;
    let common = PathBuf::from(agents::git(top, &["rev-parse", "--path-format=absolute", "--git-common-dir"])?);
    let path = common.join("ekko-merge.lock");
    let file = std::fs::OpenOptions::new().write(true).create(true).truncate(false).open(&path).map_err(|error| format!("{} could not be opened: {error}", path.display()))?;
    // SAFETY: `file` owns this descriptor and outlives the call.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(format!("another ekko agents merge runs on {}: one merge at a time, so the branch they go into moves once at a time; merge again once it ends", top.display()));
    }
    Ok(file)
}

/// The worktree on `branch`, put back on it if a merge cut short left it on
/// a detached HEAD; one made for it where none is.
fn worktree_of(root: &Path, top: &Path, branch: &str) -> Result<PathBuf, String> {
    let listed = agents::git(top, &["worktree", "list", "--porcelain"])?;
    let standard = top.join(".claude").join("worktrees").join(branch);
    for entry in listed.split("\n\n") {
        let field = |key: &str| entry.lines().find_map(|line| line.strip_prefix(key));
        let Some(path) = field("worktree ").map(PathBuf::from) else { continue };
        let on = field("branch ").map(|on| on.trim_start_matches("refs/heads/"));
        if on == Some(branch) {
            return Ok(path);
        }
        // Cut short: on a detached HEAD, with the file that says a merge put
        // it there.
        if on.is_none() && agents::same(&path, &standard) {
            let underway = agents::git(&path, &["rev-parse", "--path-format=absolute", "--git-path", UNDERWAY]).map(PathBuf::from)?;
            if !underway.exists() {
                continue;
            }
            if rebasing(&path) {
                agents::git(&path, &["rebase", "--abort"])?;
            }
            agents::git(&path, &["checkout", "--quiet", branch]).map_err(|why| {
                format!("an earlier merge of {branch} was cut short and left {} on a detached HEAD, which does not go back on {branch}: {why}", path.display())
            })?;
            let _ = std::fs::remove_file(&underway);
            return Ok(path);
        }
    }
    agents::worktree(root, branch).map(|made| made.path)
}

/// What `git status --porcelain` says of the worktree at `path`, each line
/// whole: its first column is the index's.
fn status(path: &Path) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["status", "--porcelain"])
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("git could not be run: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim_end().to_string())
}

/// Whether a rebase stopped part way in the worktree at `path`.
fn rebasing(path: &Path) -> bool {
    ["rebase-merge", "rebase-apply"]
        .iter()
        .any(|dir| agents::git(path, &["rev-parse", "--path-format=absolute", "--git-path", dir]).is_ok_and(|dir| Path::new(&dir).exists()))
}

/// What a test run did.
struct Ran {
    passed: bool,
    /// Its exit code, or the signal that ended it.
    code: String,
    seconds: f64,
    tail: String,
    log: PathBuf,
}

/// Runs `test` through `sh -c` in `folder`, its output and its errors
/// together in a log of their own under ekko's state directory.
fn run_test(home: &Path, folder: &Path, project: &str, branch: &str, test: &str) -> Result<Ran, String> {
    let dir = crate::agent::state_dir(home).join("merges");
    std::fs::create_dir_all(&dir).map_err(|error| format!("{} could not be made: {error}", dir.display()))?;
    let name: String = format!("{project}-{branch}-{}", chrono::Local::now().format("%Y%m%dT%H%M%S%.3f"))
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '.') { c } else { '_' })
        .collect();
    // A check run in the same millisecond as the test before it gets a log
    // of its own (task 1831).
    let (log, out) = (1..)
        .map(|n| dir.join(if n == 1 { format!("{name}.log") } else { format!("{name}-{n}.log") }))
        .take(100)
        .find_map(|log| std::fs::OpenOptions::new().write(true).create_new(true).open(&log).ok().map(|out| (log, out)))
        .ok_or_else(|| format!("no log could be made in {}", dir.display()))?;
    let err = out.try_clone().map_err(|error| format!("{} could not be written: {error}", log.display()))?;
    // CI has no Claude Code session around its tests (task 1942): they run
    // without what a born session's window is started without (task 1803).
    // Where the session's own environment cannot be read, the variables named
    // outright still go.
    let environment = agents::starting_environment().unwrap_or_else(|_| std::env::vars_os().collect());
    let started = Instant::now();
    let mut command = Command::new("sh");
    for name in agents::session_variables(&environment) {
        command.env_remove(name);
    }
    let status = command
        .args([OsStr::new("-c"), OsStr::new(test)])
        .current_dir(folder)
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(err)
        .status()
        .map_err(|error| format!("sh could not be run for the tests: {error}"))?;
    let seconds = started.elapsed().as_secs_f64();
    let code = match (status.code(), std::os::unix::process::ExitStatusExt::signal(&status)) {
        (Some(code), _) => format!("exit code {code}"),
        (None, Some(signal)) => format!("signal {signal}"),
        (None, None) => "no exit code".to_string(),
    };
    let text = String::from_utf8_lossy(&std::fs::read(&log).unwrap_or_default()).into_owned();
    let lines: Vec<&str> = text.lines().collect();
    let tail = lines[lines.len().saturating_sub(QUOTED_LINES)..].iter().map(|line| crate::agent::clip(line, QUOTED_WIDTH)).collect::<Vec<_>>().join("\n");
    Ok(Ran { passed: status.success(), code, seconds, tail, log })
}

/// Moves `into` from `base` to `new` in the project's checkout at `top`, a
/// fast-forward only, its files following; refused where the checkout left
/// `into`, or `into` moved, while the tests ran.
fn fast_forward(top: &Path, into: &str, base: &str, new: &str) -> Result<(), String> {
    let on = agents::git(top, &["symbolic-ref", "--quiet", "--short", "HEAD"]).unwrap_or_default();
    if on != into {
        let now = if on.is_empty() { "a detached HEAD".to_string() } else { on };
        return Err(format!("the project's checkout left {into} for {now} while the tests ran: merge again"));
    }
    let now = agents::git(top, &["rev-parse", "HEAD"])?;
    if now != base {
        return Err(format!("{into} moved while the tests ran, from {} to {}: merge again", short(base), short(&now)));
    }
    agents::git(top, &["merge", "--quiet", "--ff-only", new]).map(|_| ())
}

/// Removes the merged branch's worktree, then the branch, unless `keep`:
/// as git does without being forced, so it refuses work not committed.
fn leave(top: &Path, worktree: &Path, branch: &str, keep: bool) -> Left {
    if keep {
        return Left::Kept;
    }
    let removed = agents::git(top, &[OsStr::new("worktree"), OsStr::new("remove"), worktree.as_os_str()])
        .and_then(|_| agents::git(top, &["branch", "--quiet", "-d", branch]));
    match removed {
        Ok(_) => Left::Removed,
        Err(why) => Left::NotRemoved(why),
    }
}

fn left_said(left: &Left, branch: &str) -> String {
    match left {
        Left::Removed => format!("Its worktree and the branch {branch} were removed."),
        Left::Kept => format!("Its worktree and the branch {branch} were kept, as --keep asked."),
        Left::NotRemoved(why) => format!("Its worktree and the branch {branch} were kept: git would not remove them: {why}"),
    }
}

/// Attaches a note holding `text` to each of `tasks`: the notes' ids.
pub(crate) fn noted(ekko: &Ekko, tasks: &[u32], text: &str) -> Result<Vec<u32>, EkkoError> {
    let mut draft = Draft::open(ekko)?;
    let notes = tasks.iter().map(|id| draft.create(&note(*id, text))).collect::<Result<Vec<_>, _>>()?;
    draft.commit(false)?;
    Ok(notes)
}

fn note(task: u32, text: &str) -> Create {
    Create {
        kind: Some(Kind::Note),
        text: text.to_string(),
        boards: Vec::new(),
        priority: None,
        due: None,
        with: None,
        phase: None,
        blocked_by: Vec::new(),
        attached_to: Some(Ref::Id(task)),
        supersedes: None,
        starred: false,
    }
}

/// Hands the done tasks back: a note on each saying why their branch did not
/// go into `into` and what to do, and each set back to pending; their
/// notes alone where the board refuses that.
#[allow(clippy::too_many_arguments)]
fn hand_back(ekko: &Ekko, ids: &[u32], done: &[u32], branch: &str, into: &str, worktree: &Path, test: &Test, back: Back) -> Result<Outcome, EkkoError> {
    let at = worktree.display();
    let (why, details, to_do, log) = match back {
        Back::Uncommitted(status) => (
            format!("its worktree holds work not committed on {branch}"),
            format!("git status in {at}:\n```\n{status}\n```"),
            format!("in {at}, commit that work on {branch}, or remove it, and set the task done again"),
            None,
        ),
        Back::Conflict(files) => (
            format!("rebased onto {into}, {branch} conflicts in {}", if files.is_empty() { "files git did not list".to_string() } else { files.join(", ") }),
            format!("The rebase was undone: {branch} is where its session left it."),
            match test {
                Test::Command(command) => format!("in {at}, rebase {branch} onto {into} (git rebase {into}), resolve the conflicts, run `{command}`, commit, and set the task done again"),
                Test::None => format!("in {at}, rebase {branch} onto {into} (git rebase {into}), resolve the conflicts, commit, and set the task done again"),
            },
            None,
        ),
        Back::Markers(found) => (
            format!("rebased onto {into}, {branch} holds leftover conflict markers in {}", if found.len() == 1 { "a commit".to_string() } else { format!("{} places", found.len()) }),
            format!(
                "`git log --check` over the commits of {branch} that {into} lacks found, as commit and file:line:\n```\n{}\n```\nThe rebase was undone: {branch} is where its session left it. A later commit that removes them does not help: each commit goes into {into} as it is.",
                found.join("\n")
            ),
            format!("in {at}, remove the markers from the commits that hold them (git rebase -i {into}, or fixup commits), and set the task done again"),
            None,
        ),
        Back::Failed { command, code, seconds: took, tail, log } => (
            format!("rebased onto {into}, {branch} failed `{command}` ({code}, after {})", seconds(took)),
            format!("The end of its output:\n```\n{tail}\n```\nThe whole of it: {}. The rebase was undone: {branch} is where its session left it.", log.display()),
            format!("in {at}, rebase {branch} onto {into} (git rebase {into}), make `{command}` pass, commit, and set the task done again"),
            Some(log),
        ),
    };
    let text = format!("Not merged into {into}: {why}. ekko agents merge left {into} and {branch} as they were, and set this task back to pending. To do: {to_do}.\n\n{details}");
    let mut draft = Draft::open(ekko)?;
    let notes = done.iter().map(|id| draft.create(&note(*id, &text))).collect::<Result<Vec<_>, _>>()?;
    let refs: Vec<Ref> = done.iter().map(|id| Ref::Id(*id)).collect();
    let reopened = draft.set_state(&refs, "unstarted").and_then(|_| draft.commit(false).map(|_| ()));
    let (notes, reopened, not_reopened) = match reopened {
        Ok(()) => (notes, done.to_vec(), None),
        // Notes alone: set back, a task done work depends on would leave
        // that work done on what is not there.
        Err(why) => {
            let unreopened = text.replace(", and set this task back to pending", ", and could not set this task back to pending");
            (noted(ekko, done, &format!("{unreopened}\n\nNot set back to pending: {why}"))?, Vec::new(), Some(why.to_string()))
        }
    };
    Ok(Outcome::HandedBack(HandedBack { tasks: ids.to_vec(), branch: branch.to_string(), into: into.to_string(), why, notes, reopened, not_reopened, log }))
}

/// The leftover conflict markers in the commits of the rebased branch,
/// checked out in `worktree`, that `base` does not have, as `abc1234
/// subject: file:12`. Whitespace errors, which `git log --check` also
/// prints, are not counted: they build.
fn markers(worktree: &Path, base: &str) -> Result<Vec<String>, String> {
    let range = format!("{base}..HEAD");
    // Exits 2 for any problem it finds, so not through `agents::git`.
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(["log", "--check", "--format=commit %h %s", &range])
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|error| format!("git could not be run: {error}"))?;
    if !output.status.success() && output.status.code() != Some(2) {
        return Err(format!("git log --check {range} failed: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    let mut found = Vec::new();
    let mut commit = String::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if let Some(header) = line.strip_prefix("commit ") {
            commit = header.to_string();
        } else if let Some(place) = line.strip_suffix(": leftover conflict marker") {
            found.push(format!("{commit}: {place}"));
        }
    }
    Ok(found)
}

/// A commit, as git abbreviates one in a line.
fn short(commit: &str) -> &str {
    commit.get(..7).unwrap_or(commit)
}

/// `52 s`, `1.4 s`.
fn seconds(seconds: f64) -> String {
    if seconds < 10.0 { format!("{seconds:.1} s") } else { format!("{seconds:.0} s") }
}
