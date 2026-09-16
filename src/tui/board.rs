//! The board as the interactive mode holds it: read whole, replaced whole.
//!
//! Every box draws from one `Snapshot`. Reading it in one place, and swapping
//! it in one assignment when the board changes, is what keeps the list, the
//! tree, the tabs, the problems and the status bar from showing two different
//! moments of the same board side by side.

use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;

use super::tabs::Doc;
use crate::agent::{self, Entry, Prime};
use crate::ekko::{uid_index, Ekko, EkkoError, Outcome};
use crate::item::{tally, Item};
use crate::project;
use crate::render::{Inversion, ProjectSummary, RoadmapStep};
use crate::storage::ItemMap;

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

/// An item's relations in both directions, by display id.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Links {
    /// Every recorded blocker, open or not, so a closed one still explains
    /// why the dependency was there.
    pub blockers: Vec<u32>,
    /// The items recording this one as a blocker.
    pub dependents: Vec<u32>,
    /// The notes attached to this task.
    pub notes: Vec<u32>,
    /// The task this note is attached to.
    pub attached_to: Option<u32>,
}

/// What a snapshot is read with: how `prime` names the board, where the
/// project registry lives, and when the session began.
pub struct Reading<'a> {
    pub label: &'a str,
    pub home: &'a Path,
    pub since: i64,
}

pub struct Snapshot {
    /// Everything in storage, put away or not.
    pub all: ItemMap,
    /// What `--clear` moved to the archive.
    pub archived: ItemMap,
    /// Every visible item in board order. An item on two boards is here twice,
    /// once under each, as the board view draws it.
    pub items: Vec<Item>,
    pub groups: Vec<Group>,
    pub prime: Prime,
    /// The blockers still open, for every item waiting on any, by display id.
    pub blockers: HashMap<u32, Vec<u32>>,
    pub problems: Vec<Problem>,
    pub links: HashMap<u32, Links>,
    pub phases: Vec<String>,
    pub roadmap: Vec<RoadmapStep>,
    /// Items in a project with phases that sit in none of them.
    pub rootless: u32,
    pub inversions: Vec<Inversion>,
    pub stash: Vec<(String, Vec<Item>)>,
    pub trash: Vec<Item>,
    pub archive: Vec<(String, Vec<Item>)>,
    pub projects: Vec<ProjectSummary>,
    /// Items written since the session began, newest first.
    pub changes: Vec<Entry>,
    /// Display ids by key, for storage and the archive apart.
    keys: HashMap<String, u32>,
    archived_keys: HashMap<String, u32>,
}

/// What names an item across reloads: its uid, which survives renumbering.
pub fn key(item: &Item) -> String {
    item.uid.clone().unwrap_or_else(|| format!("#{}", item.id))
}

fn visible(item: &Item) -> bool {
    item.stashed.is_none() && item.trashed.is_none()
}

impl Snapshot {
    /// Reads the board `ekko` has open.
    pub fn load(ekko: &Ekko, reading: &Reading) -> Result<Snapshot, EkkoError> {
        let Outcome::Board(boards) = ekko.display_by_board()? else {
            unreachable!("the board view always answers grouped by board")
        };
        let prime = agent::prime(ekko, reading.label)?;
        let blockers = ekko.blocker_map()?;
        let all = ekko.storage.get()?;
        let archived = ekko.storage.get_archive()?;
        let phases = ekko.storage.get_phases()?;
        let (roadmap, rootless, inversions) = match ekko.display_roadmap()? {
            Outcome::Roadmap { steps, rootless, inversions } if !phases.is_empty() => (steps, rootless, inversions),
            _ => (Vec::new(), 0, Vec::new()),
        };
        let Outcome::Stash(stash) = ekko.display_stash()? else { unreachable!("the stash answers grouped by board") };
        let Outcome::Trash(trash) = ekko.display_trash()? else { unreachable!("the trash answers as a list") };
        let Outcome::Archive(archive) = ekko.display_archive()? else { unreachable!("the archive answers by date") };
        let mut changes = agent::changes(ekko, reading.since)?.items;
        changes.reverse();
        let projects = project::list(reading.home);

        let mut items = Vec::new();
        let mut groups = Vec::new();
        for (name, group) in boards {
            let (complete, tasks) = tally(group.iter());
            let start = items.len();
            items.extend(group);
            groups.push(Group { name, complete, tasks, items: start..items.len() });
        }

        let problems = problems(&prime, &items);
        let links = links(&all);
        let keys = all.values().map(|item| (key(item), item.id)).collect();
        let archived_keys = archived.values().map(|item| (key(item), item.id)).collect();
        Ok(Snapshot {
            all,
            archived,
            items,
            groups,
            prime,
            blockers,
            problems,
            links,
            phases,
            roadmap,
            rootless,
            inversions,
            stash,
            trash,
            archive,
            projects,
            changes,
            keys,
            archived_keys,
        })
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

    /// The item a key names, and whether it is only in the archive.
    pub fn find(&self, key: &str) -> Option<(&Item, bool)> {
        if let Some(item) = self.keys.get(key).and_then(|id| self.all.get(id)) {
            return Some((item, false));
        }
        self.archived_keys.get(key).and_then(|id| self.archived.get(id)).map(|item| (item, true))
    }

    /// Visible items in phase `name`, in id order.
    pub fn in_phase(&self, name: &str) -> Vec<&Item> {
        self.all.values().filter(|item| visible(item) && item.phase.as_deref() == Some(name)).collect()
    }

    /// Visible tasks due in `month` of `year`, by day of the month.
    pub fn due_in(&self, year: i32, month: u32) -> HashMap<u32, Vec<&Item>> {
        let prefix = format!("{year:04}-{month:02}-");
        let mut days: HashMap<u32, Vec<&Item>> = HashMap::new();
        for item in self.all.values().filter(|item| visible(item) && item.is_task) {
            let day = item.due_date.as_deref().and_then(|due| due.strip_prefix(&prefix)).and_then(|day| day.parse().ok());
            if let Some(day) = day {
                days.entry(day).or_default().push(item);
            }
        }
        days
    }

    /// A tab's title: the page's name, or an item's id and description.
    pub fn title(&self, doc: &Doc) -> String {
        match doc {
            Doc::Board => "Board".to_string(),
            Doc::Welcome => "Welcome".to_string(),
            Doc::Roadmap => "Roadmap".to_string(),
            Doc::Calendar => "Calendar".to_string(),
            Doc::Graph => "Graph".to_string(),
            Doc::Item(key) => match self.find(key) {
                Some((item, _)) => format!("{} {}", item.id, one_line(&item.description)),
                None => "Deleted item".to_string(),
            },
        }
    }
}

/// A description on one line.
pub fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn matches(item: &Item, needle: &str) -> bool {
    item.description.to_lowercase().contains(needle)
        || needle.strip_prefix('#').unwrap_or(needle) == item.id.to_string()
}

fn join(ids: &[u32]) -> String {
    ids.iter().map(u32::to_string).collect::<Vec<_>>().join(", ")
}

/// Every item's relations, resolved once from the uids storage keeps.
fn links(all: &ItemMap) -> HashMap<u32, Links> {
    let index = uid_index(all);
    let mut links: HashMap<u32, Links> = HashMap::new();
    for (id, item) in all {
        for uid in item.blocked_by.iter().flatten() {
            if let Some(&blocker) = index.get(uid.as_str()) {
                links.entry(*id).or_default().blockers.push(blocker);
                links.entry(blocker).or_default().dependents.push(*id);
            }
        }
        if let Some(&task) = item.attached_to.as_deref().and_then(|uid| index.get(uid)) {
            links.entry(*id).or_default().attached_to = Some(task);
            links.entry(task).or_default().notes.push(*id);
        }
    }
    for entry in links.values_mut() {
        for ids in [&mut entry.blockers, &mut entry.dependents, &mut entry.notes] {
            ids.sort_unstable();
            ids.dedup();
        }
    }
    links
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

    pub(crate) fn snapshot(dir: &Path, ekko: &Ekko) -> Snapshot {
        Snapshot::load(ekko, &Reading { label: "test", home: dir, since: 0 }).unwrap()
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
        let snapshot = snapshot(&dir, &ekko);

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
        let snapshot = snapshot(&dir, &ekko);

        assert_eq!(snapshot.count(Severity::Blocked), 1);
        assert_eq!(snapshot.count(Severity::Warning), 1);
        let blocked = snapshot.problems.iter().find(|p| p.severity == Severity::Blocked).unwrap();
        assert_eq!((blocked.id, blocked.detail.as_str()), (2, "blocked by 1"));
        assert_eq!(snapshot.blockers.get(&2), Some(&vec![1]));
        assert_eq!(snapshot.next().iter().map(|entry| entry.id).collect::<Vec<_>>(), vec![3, 1]);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Relations are resolved both ways, and a note hangs off its task.
    #[test]
    fn links_run_both_ways() {
        let (dir, ekko) = board("links");
        ekko.create_task(&words(&["@a", "base"])).unwrap();
        ekko.create_task(&words(&["@a", "on top"])).unwrap();
        ekko.create_note(&words(&["@a", "why"])).unwrap();
        ekko.set_blocked_by(&words(&["@2", "1"])).unwrap();
        ekko.set_attached_to(&words(&["@3", "2"])).unwrap();
        let snapshot = snapshot(&dir, &ekko);

        assert_eq!(snapshot.links[&1].dependents, vec![2]);
        assert_eq!(snapshot.links[&2].blockers, vec![1]);
        assert_eq!(snapshot.links[&2].notes, vec![3]);
        assert_eq!(snapshot.links[&3].attached_to, Some(2));
        let (found, archived) = snapshot.find(&key(&snapshot.all[&2])).unwrap();
        assert_eq!((found.id, archived), (2, false));
        assert_eq!(snapshot.title(&Doc::Item(key(&snapshot.all[&2]))), "2 on top");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The stash, the trash, the archive and what changed are all read, and
    /// an archived item can still be found by its key.
    #[test]
    fn a_snapshot_holds_what_is_put_away_and_what_changed() {
        let (dir, ekko) = board("away");
        for description in ["finished", "set aside", "thrown away"] {
            ekko.create_task(&words(&["@a", description])).unwrap();
        }
        let uid = ekko.storage.get().unwrap()[&1].uid.clone().unwrap();
        ekko.check_tasks(&words(&["1"]), false).unwrap();
        ekko.clear().unwrap();
        ekko.set_stashed(&words(&["2"]), true).unwrap();
        ekko.set_trashed(&words(&["3"]), true).unwrap();
        let snapshot = snapshot(&dir, &ekko);

        assert_eq!(snapshot.stash.iter().map(|(_, items)| items.len()).sum::<usize>(), 1);
        assert_eq!(snapshot.trash.len(), 1);
        assert_eq!(snapshot.archive.iter().map(|(_, items)| items.len()).sum::<usize>(), 1);
        assert!(snapshot.find(&uid).is_some_and(|(_, archived)| archived), "an archived item was not found");
        assert!(!snapshot.changes.is_empty());
        assert!(snapshot.projects.is_empty(), "a scratch home has no projects");
        std::fs::remove_dir_all(&dir).ok();
    }
}
