//! The board as the interactive mode holds it: read whole, replaced whole.
//!
//! Every box draws from one `Snapshot`. Reading it in one place, and swapping
//! it in one assignment when the board changes, is what keeps the list, the
//! queue, the problems and the status bar from showing two different moments
//! of the same board side by side.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use crate::agent::{self, Entry, Prime};
use crate::ekko::{Ekko, EkkoError, Outcome};
use crate::item::{tally, Item};

/// A board, and where its items sit in `Snapshot::items`.
pub struct Group {
    pub name: String,
    pub complete: u32,
    pub tasks: u32,
    pub items: Range<usize>,
}

/// One line of the board list: a board's heading, or an item under it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    Board(usize),
    Item { group: usize, item: usize },
}

/// Which counter a problem adds to in the status bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    /// Open work that cannot move until other work finishes.
    Blocked,
    /// Past its date, completed over an open blocker, or waiting against the
    /// phase order.
    Warning,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    pub severity: Severity,
    pub id: u32,
    pub description: String,
    pub detail: String,
}

pub struct Snapshot {
    /// Every visible item in board order. An item on two boards is here twice,
    /// once under each, as the board view draws it.
    pub items: Vec<Item>,
    pub groups: Vec<Group>,
    pub prime: Prime,
    /// The blockers still open, for every item waiting on any, by display id.
    pub blockers: HashMap<u32, Vec<u32>>,
    pub problems: Vec<Problem>,
    /// Visible notes attached to each task, by the task's uid.
    pub attached: HashMap<String, usize>,
}

impl Snapshot {
    /// Reads the board `ekko` has open; `label` is how `prime` names it.
    pub fn load(ekko: &Ekko, label: &str) -> Result<Snapshot, EkkoError> {
        let Outcome::Board(boards) = ekko.display_by_board()? else {
            unreachable!("the board view always answers grouped by board")
        };
        let prime = agent::prime(ekko, label)?;
        let blockers = ekko.blocker_map()?;

        let mut items = Vec::new();
        let mut groups = Vec::new();
        for (name, group) in boards {
            let (complete, tasks) = tally(group.iter());
            let start = items.len();
            items.extend(group);
            groups.push(Group { name, complete, tasks, items: start..items.len() });
        }

        let mut attached: HashMap<String, usize> = HashMap::new();
        let mut counted = HashSet::new();
        for item in &items {
            if let Some(task) = item.attached_to.as_ref().filter(|_| counted.insert(item.id)) {
                *attached.entry(task.clone()).or_default() += 1;
            }
        }

        let problems = problems(&prime, &items);
        Ok(Snapshot { items, groups, prime, blockers, problems, attached })
    }

    /// The list for `filter`: every board with something matching, headed by
    /// its name and its counts. The counts stay those of the whole board --
    /// `[3/7]` is a fact about the board, and a filter that changed it would
    /// have the same board report different totals depending on what was typed.
    ///
    /// A filter matches an item's description, its id (`12` or `#12`), or the
    /// name of the board it is under, which brings the whole board.
    pub fn rows(&self, filter: &str) -> Vec<Row> {
        let needle = filter.trim().to_lowercase();
        let mut rows = Vec::new();
        for (at, group) in self.groups.iter().enumerate() {
            let whole = needle.is_empty() || group.name.to_lowercase().contains(&needle);
            let matching: Vec<Row> = group
                .items
                .clone()
                .filter(|&item| whole || matches(&self.items[item], &needle))
                .map(|item| Row::Item { group: at, item })
                .collect();
            if !matching.is_empty() {
                rows.push(Row::Board(at));
                rows.extend(matching);
            }
        }
        rows
    }

    /// Work in progress, then what is ready, in the order to take it up.
    pub fn next(&self) -> Vec<&Entry> {
        self.prime.doing.iter().chain(&self.prime.ready).collect()
    }

    pub fn count(&self, severity: Severity) -> usize {
        self.problems.iter().filter(|problem| problem.severity == severity).count()
    }
}

fn matches(item: &Item, needle: &str) -> bool {
    item.description.to_lowercase().contains(needle)
        || needle.strip_prefix('#').unwrap_or(needle) == item.id.to_string()
}

fn join(ids: &[u32]) -> String {
    ids.iter().map(u32::to_string).collect::<Vec<_>>().join(", ")
}

/// What the Problems tab lists, from what `prime` already found: the same
/// answers an agent gets, so the two frontends cannot disagree on what is
/// wrong with a board.
fn problems(prime: &Prime, items: &[Item]) -> Vec<Problem> {
    let find = |id: u32| items.iter().find(|item| item.id == id);
    let about = |id: u32| find(id).map(|item| item.description.clone()).unwrap_or_default();
    let mut found = Vec::new();

    for entry in &prime.blocked {
        found.push(Problem {
            severity: Severity::Blocked,
            id: entry.id,
            description: entry.description.clone(),
            detail: format!("blocked by {}", join(&entry.blocked_by)),
        });
    }
    for &id in &prime.overdue {
        let due = find(id).and_then(|item| item.due_date.clone()).unwrap_or_default();
        found.push(Problem { severity: Severity::Warning, id, description: about(id), detail: format!("was due {due}") });
    }
    for &(task, blocker) in &prime.broken {
        found.push(Problem {
            severity: Severity::Warning,
            id: task,
            description: about(task),
            detail: format!("completed while {blocker} is still open"),
        });
    }
    for inversion in &prime.inversions {
        found.push(Problem {
            severity: Severity::Warning,
            id: inversion.blocked,
            description: about(inversion.blocked),
            detail: format!(
                "in {} but waits on {} in the later {}",
                inversion.blocked_phase, inversion.blocker, inversion.blocker_phase
            ),
        });
    }
    found
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::PathBuf;

    use super::*;

    pub(crate) fn board(tag: &str) -> (PathBuf, Ekko) {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("ekko-tui-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ekko = Ekko::at(&dir).unwrap();
        (dir, ekko)
    }

    pub(crate) fn words(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| word.to_string()).collect()
    }

    /// A filter keeps the heading of each board it matches in, with that
    /// board's whole counts, and drops boards with nothing matching.
    #[test]
    fn a_filter_keeps_headings_with_whole_board_counts() {
        let (dir, ekko) = board("filter");
        ekko.create_task(&words(&["@a", "wire the watcher"])).unwrap();
        ekko.create_task(&words(&["@a", "draw the frame"])).unwrap();
        ekko.create_task(&words(&["@b", "write the docs"])).unwrap();
        ekko.check_tasks(&words(&["2"]), false).unwrap();
        let snapshot = Snapshot::load(&ekko, "test").unwrap();

        assert_eq!(snapshot.rows("").len(), 5);
        let rows = snapshot.rows("WIRE");
        assert_eq!(rows.len(), 2, "{rows:?}");
        let Row::Board(group) = rows[0] else { panic!("the first row is not a heading: {rows:?}") };
        assert_eq!((snapshot.groups[group].complete, snapshot.groups[group].tasks), (1, 2));

        assert_eq!(snapshot.rows("#3").len(), 2, "an id did not match");
        assert_eq!(snapshot.rows("@b").len(), 2, "a board name did not bring its board");
        assert!(snapshot.rows("nothing like this").is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Blocked work counts as blocked, a missed date as a warning, and the
    /// blockers still open are there for every row that waits.
    #[test]
    fn problems_name_blocked_and_overdue_work() {
        let (dir, ekko) = board("problems");
        ekko.create_task(&words(&["@a", "base"])).unwrap();
        ekko.create_task(&words(&["@a", "on top"])).unwrap();
        ekko.create_task(&words(&["@a", "late", "d:2020-01-01"])).unwrap();
        ekko.set_blocked_by(&words(&["@2", "1"])).unwrap();
        let snapshot = Snapshot::load(&ekko, "test").unwrap();

        assert_eq!(snapshot.count(Severity::Blocked), 1);
        assert_eq!(snapshot.count(Severity::Warning), 1);
        let blocked = snapshot.problems.iter().find(|p| p.severity == Severity::Blocked).unwrap();
        assert_eq!((blocked.id, blocked.detail.as_str()), (2, "blocked by 1"));
        assert_eq!(snapshot.blockers.get(&2), Some(&vec![1]));
        assert_eq!(snapshot.next().iter().map(|entry| entry.id).collect::<Vec<_>>(), vec![3, 1]);
        std::fs::remove_dir_all(&dir).ok();
    }
}
