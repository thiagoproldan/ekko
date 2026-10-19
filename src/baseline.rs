//! What a handoff was written against, and what moved since (task 2205).
//!
//! A handoff says where its session stopped, and the next session reads it
//! as the present. Study 2197 (note 2211) measured what that costs when work
//! moved in between: a session told, under the handoff, what moved since --
//! commits, releases, the tasks it names closed since, or that nothing did
//! -- answered "where did we stop?" as well as one told nothing, for 75k
//! fewer units, 2 fewer calls and 67 fewer seconds, stale and fresh alike.
//!
//! So every write that writes a handoff's text records what it was written
//! against (`Item::baseline`): the commit HEAD was on and its branch, read
//! from the project's repository, and each task the handoff names with
//! whether it was closed, read from the board as the write leaves it -- a
//! task the session closed before writing the handoff is no news to it. The
//! prime compares that with the repository and the board as they are, by
//! the commit graph rather than by clocks: commits on HEAD since that
//! commit, the tags among them, files changed and not committed, and the
//! named tasks closed or reopened since.

use std::path::Path;

use serde::Serialize;

use crate::item::{Baseline, Item, NamedTask, State};
use crate::storage::ItemMap;

/// How many tasks of each list a line names before it counts the rest.
const TASKS_NAMED: usize = 10;

/// Records what each handoff among `changed` was written against, where this
/// write wrote its text or made it a handoff; drops the record from any
/// other item among them. A handoff whose text the write left alone keeps
/// what it had, none included: a handoff written before the record existed
/// gets none from a later write, which would date it from that write.
/// `before` and `arrived` hold the items as they were; one arriving from
/// another board brings none (`crate::move_to`). `folder` is the project's,
/// whose repository is read once, and only when some handoff needs it.
pub fn keep(before: &ItemMap, arrived: &ItemMap, data: &mut ItemMap, changed: &[u32], folder: Option<&Path>, now: i64) {
    let mut written = Vec::new();
    for id in changed {
        let Some(item) = data.get_mut(id) else { continue };
        if !item.handoff {
            item.baseline = None;
            continue;
        }
        let old = before.get(id).or_else(|| arrived.get(id));
        if old.is_none_or(|old| !old.handoff || old.description != item.description) {
            written.push(*id);
        }
    }
    if written.is_empty() {
        return;
    }
    let head = folder.and_then(head);
    for id in written {
        let tasks = named(&data[&id], data);
        let item = data.get_mut(&id).expect("a changed item");
        item.baseline = Some(Box::new(Baseline {
            at: now,
            repository: head.is_some(),
            commit: head.as_ref().and_then(|head| head.commit.clone()),
            branch: head.as_ref().and_then(|head| head.branch.clone()),
            tasks,
            unknown: Default::default(),
        }));
    }
}

/// The tasks `handoff` names, as they stand in `data`: the one it is
/// attached to, each its text cites after a word ("task 12", "notas 3 e
/// 4"), and each number standing alone that is a task's id ("1564 and 1731
/// were born"), by display id.
fn named(handoff: &Item, data: &ItemMap) -> Vec<NamedTask> {
    let mut ids: Vec<u32> = crate::agent::cited_ids(&handoff.description);
    ids.extend(bare_ids(&handoff.description));
    if let Some(uid) = handoff.attached_to.as_deref() {
        ids.extend(data.values().filter(|item| item.uid.as_deref() == Some(uid)).map(|item| item.id));
    }
    ids.sort_unstable();
    ids.dedup();
    ids.into_iter()
        .filter_map(|id| data.get(&id))
        .filter(|item| item.is_task)
        .filter_map(|task| {
            Some(NamedTask { uid: task.uid.clone()?, closed: closed(task), unknown: Default::default() })
        })
        .collect()
}

fn closed(task: &Item) -> bool {
    matches!(State::of(task), Some(State::Done | State::Cancelled))
}

/// The numbers `text` holds standing alone, of two digits or more, that no
/// version, decimal, time, date, size, path or commit runs into -- the bare
/// ids evals/recall/replay.py's `cited_ids` reads, which study 2197's line
/// was tested with, and the dates it took for ids: "2026" in "2026-10-12".
fn bare_ids(text: &str) -> Vec<u32> {
    let bytes = text.as_bytes();
    let wordy = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_' || byte >= 0x80;
    let mut ids = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        if !bytes[at].is_ascii_digit() {
            at += 1;
            continue;
        }
        let start = at;
        while at < bytes.len() && bytes[at].is_ascii_digit() {
            at += 1;
        }
        let joined_before = start > 0 && (wordy(bytes[start - 1]) || b".,/:#%-".contains(&bytes[start - 1]));
        let next_digit = bytes.get(at + 1).is_some_and(u8::is_ascii_digit);
        let joined_after = bytes.get(at).is_some_and(|&byte| wordy(byte) || byte == b'%' || (b".,/:-".contains(&byte) && next_digit));
        if !joined_before && !joined_after && (2..=5).contains(&(at - start)) {
            ids.extend(text[start..at].parse::<u32>().ok().filter(|id| *id >= 10));
        }
    }
    ids
}

/// What moved since a handoff was written, as the prime says it under it.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Since {
    /// The commit it was written at, abbreviated, and its branch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// What the repository says, where the folder was in one then and is now.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git: Option<Moved>,
    /// How many of the tasks it names are still on the board.
    pub named: usize,
    /// Those open when it was written and closed now, by display id.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub closed: Vec<u32>,
    /// Those closed when it was written and open now.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reopened: Vec<u32>,
}

/// What the repository says moved since a handoff's commit.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Moved {
    /// Commits on HEAD since: none where HEAD is still on it.
    pub commits: u32,
    /// The tags among those commits, oldest first.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub releases: Vec<String>,
    /// Written before the repository's first commit: every commit is since.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub first: bool,
    /// HEAD no longer holds the commit: the branch was rewritten, or another
    /// checked out. `head` says where it is.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub left: bool,
    /// The commit is not in the repository any more.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub gone: bool,
    /// HEAD now, abbreviated, where it left the commit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    /// The branch checked out now, where it is another; "detached" for none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch_now: Option<String>,
    /// Tracked files changed and not committed.
    pub uncommitted: usize,
}

/// What moved since `baseline` was recorded: in the repository of `folder`,
/// and among the tasks it names, which `task` finds on the board by uid.
pub fn since<'a>(baseline: &Baseline, folder: Option<&Path>, task: impl Fn(&str) -> Option<&'a Item>) -> Since {
    let mut since = Since {
        commit: baseline.commit.as_deref().map(short),
        branch: baseline.branch.clone(),
        ..Since::default()
    };
    for named in &baseline.tasks {
        let Some(task) = task(&named.uid).filter(|task| task.is_task && task.trashed.is_none()) else { continue };
        since.named += 1;
        match (named.closed, closed(task)) {
            (false, true) => since.closed.push(task.id),
            (true, false) => since.reopened.push(task.id),
            _ => {}
        }
    }
    since.closed.sort_unstable();
    since.reopened.sort_unstable();
    if baseline.repository {
        if let (Some(folder), Some(now)) = (folder, folder.and_then(head)) {
            since.git = Some(moved(folder, baseline, &now));
        }
    }
    since
}

fn moved(folder: &Path, baseline: &Baseline, now: &Head) -> Moved {
    let mut moved = Moved { uncommitted: now.uncommitted, ..Moved::default() };
    if now.branch != baseline.branch {
        moved.branch_now = Some(now.branch.clone().unwrap_or_else(|| "detached".to_string()));
    }
    match (baseline.commit.as_deref(), now.commit.as_deref()) {
        (None, None) => moved.first = true,
        (None, Some(_)) => {
            moved.first = true;
            if let Some(log) = log(folder, "HEAD") {
                (moved.commits, moved.releases) = (log.since, log.tags);
            }
        }
        (Some(_), None) => moved.left = true,
        (Some(then), Some(head)) if then == head => {}
        (Some(then), Some(head)) => match log(folder, &format!("{then}...HEAD")) {
            None => moved.gone = true,
            Some(log) if log.lost => {
                moved.left = true;
                moved.head = Some(short(head));
            }
            Some(log) => (moved.commits, moved.releases) = (log.since, log.tags),
        },
    }
    moved
}

/// What one `git log` over a range says.
struct Log {
    /// Commits on HEAD's side.
    since: u32,
    /// The tags on them, oldest first.
    tags: Vec<String>,
    /// Some commit is on the other side only: HEAD does not hold it.
    lost: bool,
}

/// One log over `range`, each commit marked with the side only it is on and
/// the tags on it: the commits since, the releases among them, and any
/// commit HEAD lost, in one process where a count and a list of tags took
/// two. Measured on a copy of this repository for task 2205, against the
/// same prime without the line: 6.5-7.2 ms more with one commit since, where
/// the two took 9.5-10.1; 3.3-4.5 ms more with none, the status alone.
/// `None` where the range names a commit the repository does not have.
fn log(folder: &Path, range: &str) -> Option<Log> {
    let out = crate::commits::git(folder, &["log", "--left-right", "--format=%m%x1f%D", "--decorate-refs=refs/tags/", range, "--"])?;
    let mut log = Log { since: 0, tags: Vec::new(), lost: false };
    for line in out.lines() {
        let (side, refs) = line.split_once('\x1f').unwrap_or((line, ""));
        match side {
            ">" => {
                log.since += 1;
                log.tags.extend(refs.split(", ").filter_map(|name| name.strip_prefix("tag: ")).map(str::to_string));
            }
            "<" => log.lost = true,
            _ => {}
        }
    }
    // The log runs newest first.
    log.tags.reverse();
    Some(log)
}

impl Since {
    /// The line under the handoff, indented as its quoted lines are; empty
    /// where there is nothing to say: no repository, and no task it names.
    pub fn line(&self) -> String {
        let mut parts = Vec::new();
        let mut at = match (&self.commit, &self.branch) {
            (Some(commit), Some(branch)) => format!(", at {commit} on {branch}"),
            (Some(commit), None) => format!(", at {commit}"),
            _ => String::new(),
        };
        if let Some(git) = &self.git {
            if git.first {
                at = ", before the repository's first commit".to_string();
            }
            parts.push(git.words());
        }
        if self.named > 0 {
            if self.closed.is_empty() && self.reopened.is_empty() {
                parts.push("the tasks it names are as it left them".to_string());
            } else {
                let mut changed = Vec::new();
                if !self.closed.is_empty() {
                    changed.push(format!("closed since: {}", ids(&self.closed)));
                }
                if !self.reopened.is_empty() {
                    changed.push(format!("reopened since: {}", ids(&self.reopened)));
                }
                parts.push(format!("of the tasks it names, {}", changed.join("; ")));
            }
        }
        if let Some(git) = self.git.as_ref().filter(|git| git.uncommitted > 0) {
            parts.push(format!("{} changed file{} not committed", git.uncommitted, plural(git.uncommitted)));
        }
        if parts.is_empty() {
            return String::new();
        }
        format!("    Since it was written{at}: {}.\n", parts.join("; "))
    }
}

impl Moved {
    fn words(&self) -> String {
        if self.gone {
            return "that commit is no longer in the repository".to_string();
        }
        let on = self.branch_now.as_deref().map(|branch| format!(" on {branch}")).unwrap_or_default();
        if self.left {
            return match &self.head {
                Some(head) => format!("HEAD no longer holds it, now at {head}{on}"),
                None => format!("HEAD no longer holds it{on}"),
            };
        }
        let mut words = match (self.commits, self.first) {
            (0, true) => "no commit yet".to_string(),
            (0, false) => "no commit".to_string(),
            (n, _) => format!("{n} commit{}", plural(n as usize)),
        };
        match self.releases.as_slice() {
            [] => {}
            [one] => words.push_str(&format!(", released {one}")),
            [first, .., last] => words.push_str(&format!(", released {first} to {last}")),
        }
        if !on.is_empty() {
            words.push_str(&format!(", HEAD now{on}"));
        }
        words
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn ids(ids: &[u32]) -> String {
    let named: Vec<String> = ids.iter().take(TASKS_NAMED).map(u32::to_string).collect();
    match ids.len().saturating_sub(TASKS_NAMED) {
        0 => named.join(", "),
        more => format!("{} and {more} more", named.join(", ")),
    }
}

/// A commit as `git log --oneline` names it, seven characters long.
fn short(commit: &str) -> String {
    commit.chars().take(7).collect()
}

/// Where HEAD stands in the repository `folder` is in.
#[derive(Debug, PartialEq)]
struct Head {
    /// Absent before the first commit.
    commit: Option<String>,
    /// Absent on a detached HEAD.
    branch: Option<String>,
    /// Tracked files changed, staged or not, and not committed.
    uncommitted: usize,
}

/// One `git status`, in the porcelain git keeps stable for scripts, with
/// the branch's header lines. `--no-optional-locks`, as git-status(1) asks
/// of a status run in the background: it leaves the index unwritten, so it
/// never holds the lock a commit the user runs at the same time needs.
/// `None` outside a repository, or where git cannot run.
fn head(folder: &Path) -> Option<Head> {
    let out = crate::commits::git(folder, &["--no-optional-locks", "status", "--porcelain=v2", "--branch", "--untracked-files=no"])?;
    let mut head = Head { commit: None, branch: None, uncommitted: 0 };
    for line in out.lines() {
        if let Some(commit) = line.strip_prefix("# branch.oid ") {
            head.commit = (commit != "(initial)").then(|| commit.to_string());
        } else if let Some(branch) = line.strip_prefix("# branch.head ") {
            head.branch = (branch != "(detached)").then(|| branch.to_string());
        } else if line.starts_with("1 ") || line.starts_with("2 ") || line.starts_with("u ") {
            head.uncommitted += 1;
        }
    }
    Some(head)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The numbers a handoff names standing alone, and none that a version,
    /// a decimal, a time, a date, a size, a path, a commit or a percentage
    /// runs into, nor a single digit.
    #[test]
    fn bare_ids_are_numbers_standing_alone() {
        let text = "1564, 1731 and 1582 were born; 1800 is PAUSED (handoff 1820).\n\
                    Read 1822 first. main is f9564f4, 4 commits ahead, v0.44.3, 26-67 ms, 5.1 GB, 2026-10-12 ~02:05,\n\
                    src/agent.rs:1063, pts/7, %0, 95%, #469, 12k units, 3 calls, x1234, 1234x, 123456.";
        assert_eq!(bare_ids(text), vec![1564, 1731, 1582, 1800, 1820, 1822]);
    }

    /// A handoff names the task it is attached to, the tasks its text cites
    /// after a word or by a number standing alone, each with whether it is
    /// closed -- and no note, no number that is no item, and none twice.
    #[test]
    fn a_handoff_names_its_task_and_the_tasks_its_text_cites() {
        let mut data = ItemMap::new();
        for id in 1..=14 {
            let mut task = Item::new_task(id, format!("task {id}"), Vec::new(), 1);
            if [3, 12].contains(&id) {
                State::Done.write(&mut task);
            }
            if id == 13 {
                State::Cancelled.write(&mut task);
            }
            data.insert(id, task);
        }
        data.insert(15, Item::new_note(15, "a note".into(), Vec::new()));
        let mut handoff = Item::new_note(16, "On tasks 2 and 3; 12, 13 and 15 too, and 12 again; 99 is no item. Next: 1.".into(), Vec::new());
        handoff.attached_to = data[&14].uid.clone();
        let named: Vec<(u32, bool)> = named(&handoff, &data)
            .iter()
            .map(|task| (data.values().find(|item| item.uid.as_deref() == Some(task.uid.as_str())).unwrap().id, task.closed))
            .collect();
        assert_eq!(named, vec![(2, false), (3, true), (12, true), (13, true), (14, false)]);
    }

    fn since_with(git: Option<Moved>, closed: &[u32], reopened: &[u32], named: usize) -> Since {
        Since {
            commit: Some("f9564f4".into()),
            branch: Some("main".into()),
            git,
            named,
            closed: closed.to_vec(),
            reopened: reopened.to_vec(),
        }
    }

    /// The line says what it checked, both sides: the repository and the
    /// board's tasks, whether either moved or not.
    #[test]
    fn the_line_says_what_moved_and_what_did_not() {
        let still = since_with(Some(Moved::default()), &[], &[], 2);
        assert_eq!(still.line(), "    Since it was written, at f9564f4 on main: no commit; the tasks it names are as it left them.\n");

        let releases = vec!["v0.43.0".to_string(), "v0.44.0".to_string(), "v0.44.3".to_string()];
        let moved = Moved { commits: 39, releases, uncommitted: 2, ..Moved::default() };
        let line = since_with(Some(moved), &[1746, 1800], &[12], 5).line();
        assert_eq!(
            line,
            "    Since it was written, at f9564f4 on main: 39 commits, released v0.43.0 to v0.44.3; \
             of the tasks it names, closed since: 1746, 1800; reopened since: 12; 2 changed files not committed.\n"
        );

        let one = Moved { commits: 1, releases: vec!["v1".into()], uncommitted: 1, ..Moved::default() };
        assert!(since_with(Some(one), &[], &[], 1).line().contains(": 1 commit, released v1; the tasks it names are as it left them; 1 changed file not committed."));

        let elsewhere = Moved { commits: 2, branch_now: Some("side".into()), ..Moved::default() };
        assert!(since_with(Some(elsewhere), &[], &[], 1).line().contains(": 2 commits, HEAD now on side; "));
        let left = Moved { left: true, head: Some("1db2a70".into()), branch_now: Some("side".into()), ..Moved::default() };
        assert!(since_with(Some(left), &[], &[], 1).line().contains(": HEAD no longer holds it, now at 1db2a70 on side; "));
        let gone = Moved { gone: true, ..Moved::default() };
        assert!(since_with(Some(gone), &[], &[], 1).line().contains(": that commit is no longer in the repository; "));

        let first = Since { git: Some(Moved { first: true, commits: 3, ..Moved::default() }), named: 1, ..Since::default() };
        assert_eq!(first.line(), "    Since it was written, before the repository's first commit: 3 commits; the tasks it names are as it left them.\n");
        let none_yet = Since { git: Some(Moved { first: true, ..Moved::default() }), named: 1, ..Since::default() };
        assert!(none_yet.line().contains(": no commit yet; "), "{}", none_yet.line());

        let no_repository = Since { named: 1, closed: vec![5], ..Since::default() };
        assert_eq!(no_repository.line(), "    Since it was written: of the tasks it names, closed since: 5.\n");
        assert_eq!(Since::default().line(), "", "nothing to say says nothing");

        let many: Vec<u32> = (1..=13).collect();
        assert!(since_with(None, &many, &[], 13).line().contains("closed since: 1, 2, 3, 4, 5, 6, 7, 8, 9, 10 and 3 more"));
    }
}
