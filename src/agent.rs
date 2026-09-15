//! The board as an agent reads it.
//!
//! A person scans the board view, colours and all. An agent picking work back
//! up needs three answers before anything else -- what is in motion, what can
//! move next and in which order, and why -- and pays for every byte it reads
//! to get them. On a real board the full view ran to 72 KB to deliver those
//! answers, and `--list ready` was 4 KB and dropped every reason. These views
//! are built for that reader: plain text, one line per item, each line
//! starting with the item's id, and the reasons attached under their work.
//!
//! Nothing here writes, and nothing here decides anything the board does not
//! already say: readiness, blockers, totals and the roadmap come from the
//! same functions the board view and the dependency rule use.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use serde::Serialize;

use crate::ekko::{broken_dependencies, holds, phase_order, uid_index, Ekko, EkkoError, Outcome};
use crate::item::{Item, State};
use crate::render::{Inversion, RoadmapStep, Stats};
use crate::storage::ItemMap;

/// How much of a reason `prime` quotes before pointing at `context`.
const NOTE_CLIP: usize = 300;
/// How much of a task's description a one-line listing carries.
const TASK_CLIP: usize = 160;
/// Unattached notes `prime` shows, newest first.
const RECENT_NOTES: usize = 5;
/// Blocked tasks `prime` lists before counting the rest.
const BLOCKED_SHOWN: usize = 20;

/// Dependencies resolved once for a whole board, in both directions.
struct Graph<'a> {
    all: &'a ItemMap,
    /// Each item's recorded blockers, by display id.
    blockers: HashMap<u32, Vec<u32>>,
    /// Each item's dependents -- the items recording it as a blocker.
    dependents: HashMap<u32, Vec<u32>>,
}

impl<'a> Graph<'a> {
    fn new(all: &'a ItemMap) -> Self {
        let index = uid_index(all);
        let mut blockers: HashMap<u32, Vec<u32>> = HashMap::new();
        let mut dependents: HashMap<u32, Vec<u32>> = HashMap::new();
        for (id, item) in all {
            for uid in item.blocked_by.iter().flatten() {
                if let Some(&blocker) = index.get(uid.as_str()) {
                    blockers.entry(*id).or_default().push(blocker);
                    dependents.entry(blocker).or_default().push(*id);
                }
            }
        }
        Graph { all, blockers, dependents }
    }

    /// The blockers of `id` that still hold, in id order.
    fn open_blockers(&self, id: u32) -> Vec<u32> {
        let mut ids: Vec<u32> = self
            .blockers
            .get(&id)
            .into_iter()
            .flatten()
            .copied()
            .filter(|blocker| self.all.get(blocker).is_some_and(holds))
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    /// Open tasks waiting on `id`, directly or through other open tasks:
    /// what finishing it would eventually let move. Each is counted once
    /// however many paths lead to it, and a closed task ends a path, since
    /// nothing past it is waiting on `id` through it any more.
    fn downstream(&self, id: u32) -> usize {
        let mut seen = HashSet::new();
        let mut stack = vec![id];
        while let Some(at) = stack.pop() {
            for &dependent in self.dependents.get(&at).into_iter().flatten() {
                if self.all.get(&dependent).is_some_and(holds) && seen.insert(dependent) {
                    stack.push(dependent);
                }
            }
        }
        seen.remove(&id);
        seen.len()
    }
}

/// One item as the agent views list it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: u32,
    pub uid: Option<String>,
    pub state: &'static str,
    /// `stashed` or `trashed` for an item put away; absent otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub away: Option<&'static str>,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
    pub boards: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub starred: bool,
    /// Blockers still holding, by display id.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub blocked_by: Vec<u32>,
    /// Open tasks waiting on this one, directly or through others.
    #[serde(skip_serializing_if = "is_zero")]
    pub unblocks: usize,
    pub updated_at: i64,
    /// Notes attached to this task.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<NoteRef>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NoteRef {
    pub id: u32,
    pub uid: Option<String>,
    pub description: String,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// Everything the views share, read off one load of the board.
struct Reader<'a> {
    graph: Graph<'a>,
    /// Visible notes, by the uid of the task each is attached to.
    attached: HashMap<&'a str, Vec<&'a Item>>,
    order: HashMap<&'a str, usize>,
    today: String,
}

impl<'a> Reader<'a> {
    fn new(all: &'a ItemMap, phases: &'a [String]) -> Self {
        let mut attached: HashMap<&str, Vec<&Item>> = HashMap::new();
        for item in all.values().filter(|item| visible(item)) {
            if let Some(task) = item.attached_to.as_deref() {
                attached.entry(task).or_default().push(item);
            }
        }
        Reader {
            graph: Graph::new(all),
            attached,
            order: phase_order(phases),
            today: chrono::Local::now().format("%Y-%m-%d").to_string(),
        }
    }

    fn entry(&self, item: &Item) -> Entry {
        let notes = item
            .uid
            .as_deref()
            .and_then(|uid| self.attached.get(uid))
            .into_iter()
            .flatten()
            .map(|note| NoteRef { id: note.id, uid: note.uid.clone(), description: note.description.clone() })
            .collect();
        Entry {
            id: item.id,
            uid: item.uid.clone(),
            state: state_word(item),
            away: if item.trashed.is_some() {
                Some("trashed")
            } else if item.stashed.is_some() {
                Some("stashed")
            } else {
                None
            },
            description: item.description.clone(),
            priority: item.priority.filter(|_| item.is_task),
            due: item.due_date.clone(),
            boards: item.boards.clone(),
            phase: item.phase.clone(),
            starred: item.is_starred,
            blocked_by: self.graph.open_blockers(item.id),
            unblocks: if item.is_task { self.graph.downstream(item.id) } else { 0 },
            updated_at: updated(item),
            notes,
        }
    }

    fn ready(&self, item: &Item) -> bool {
        visible(item) && holds(item) && self.graph.open_blockers(item.id).is_empty()
    }

    /// What to take up next, best first: work already in progress, then
    /// earlier phases before later ones (the root after every phase), higher
    /// priority, the nearer deadline (none last), more work waiting
    /// downstream, and the older item. A lexicographic order, so each key
    /// only breaks ties left by the ones before it.
    fn next(&self, limit: Option<usize>) -> Vec<Entry> {
        let mut candidates: Vec<&Item> = self
            .graph
            .all
            .values()
            .filter(|item| {
                visible(item) && (State::of(item) == Some(State::Progress) || self.ready(item))
            })
            .collect();
        candidates.sort_by_cached_key(|item| {
            (
                State::of(item) != Some(State::Progress),
                item.phase.as_deref().and_then(|p| self.order.get(p)).copied().unwrap_or(usize::MAX),
                std::cmp::Reverse(item.priority.unwrap_or(1)),
                (item.due_date.is_none(), item.due_date.clone()),
                std::cmp::Reverse(self.graph.downstream(item.id)),
                item.id,
            )
        });
        candidates.into_iter().take(limit.unwrap_or(usize::MAX)).map(|item| self.entry(item)).collect()
    }
}

fn visible(item: &Item) -> bool {
    item.stashed.is_none() && item.trashed.is_none()
}

fn updated(item: &Item) -> i64 {
    item.updated_at.unwrap_or(item.timestamp)
}

fn state_word(item: &Item) -> &'static str {
    match State::of(item) {
        None => "note",
        Some(State::Pending) => "pending",
        Some(State::Progress) => "in progress",
        Some(State::Paused) => "paused",
        Some(State::Done) => "done",
        Some(State::Cancelled) => "cancelled",
    }
}

/// `text` on one line, cut at `max` characters with a count of what was cut.
fn clip(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let count = flat.chars().count();
    if count <= max {
        return flat;
    }
    let head: String = flat.chars().take(max).collect();
    format!("{}\u{2026} (+{} chars)", head.trim_end(), count - max)
}

/// The resume view: what an agent needs to pick a board back up.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Prime {
    pub board: String,
    /// The newest `updatedAt` on the board: pass it to `changes` later to
    /// hear only what moved since this read.
    pub cursor: i64,
    pub stats: Stats,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub roadmap: Vec<RoadmapStep>,
    #[serde(skip_serializing_if = "is_zero_u32")]
    pub rootless: u32,
    pub doing: Vec<Entry>,
    pub ready: Vec<Entry>,
    pub blocked: Vec<Entry>,
    pub recent_notes: Vec<NoteRef>,
    /// Done tasks still waiting on open work, as (task, blocker) pairs.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub broken: Vec<(u32, u32)>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub inversions: Vec<Inversion>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub overdue: Vec<u32>,
}

fn is_zero_u32(n: &u32) -> bool {
    *n == 0
}

/// Builds the resume view of the board `ekko` has open, labelled `board`.
pub fn prime(ekko: &Ekko, board: &str) -> Result<Prime, EkkoError> {
    let all = ekko.storage.get()?;
    let phases = ekko.storage.get_phases()?;
    let reader = Reader::new(&all, &phases);

    let next = reader.next(None);
    let (doing, ready): (Vec<Entry>, Vec<Entry>) =
        next.into_iter().partition(|entry| entry.state == "in progress");

    let mut blocked: Vec<Entry> = all
        .values()
        .filter(|item| visible(item) && holds(item) && State::of(item) != Some(State::Progress))
        .filter(|item| !reader.graph.open_blockers(item.id).is_empty())
        .map(|item| reader.entry(item))
        .collect();
    blocked.sort_by_key(|entry| (std::cmp::Reverse(entry.priority.unwrap_or(1)), entry.id));

    let mut notes: Vec<&Item> =
        all.values().filter(|item| visible(item) && !item.is_task && item.attached_to.is_none()).collect();
    notes.sort_by_key(|item| (std::cmp::Reverse(updated(item)), std::cmp::Reverse(item.id)));
    let recent_notes = notes
        .into_iter()
        .take(RECENT_NOTES)
        .map(|note| NoteRef { id: note.id, uid: note.uid.clone(), description: note.description.clone() })
        .collect();

    let (roadmap, rootless, inversions) = match (phases.is_empty(), ekko.display_roadmap()?) {
        (false, Outcome::Roadmap { steps, rootless, inversions }) => (steps, rootless, inversions),
        _ => (Vec::new(), 0, Vec::new()),
    };

    let overdue = all
        .values()
        .filter(|item| visible(item) && holds(item))
        .filter(|item| item.due_date.as_deref().is_some_and(|due| due < reader.today.as_str()))
        .map(|item| item.id)
        .collect();

    Ok(Prime {
        board: board.to_string(),
        cursor: all.values().map(updated).max().unwrap_or(0),
        stats: ekko.compute_stats(&all),
        roadmap,
        rootless,
        doing,
        ready,
        blocked,
        recent_notes,
        broken: broken_dependencies(&all).into_iter().collect(),
        inversions,
        overdue,
    })
}

/// What to take up next, best first; see `Reader::next` for the order.
pub fn next(ekko: &Ekko, limit: Option<usize>) -> Result<Vec<Entry>, EkkoError> {
    let all = ekko.storage.get()?;
    let phases = ekko.storage.get_phases()?;
    Ok(Reader::new(&all, &phases).next(limit))
}

/// An item and its neighbourhood: one hop along every relation, plus how
/// much open work waits on it further out.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Context {
    pub item: Entry,
    pub created: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub stashed: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub trashed: bool,
    /// Every recorded blocker, open or not, so a closed one still explains
    /// why the dependency was there.
    pub blockers: Vec<Link>,
    pub dependents: Vec<Link>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attached_to: Option<Link>,
}

#[derive(Debug, Serialize)]
pub struct Link {
    pub id: u32,
    pub uid: Option<String>,
    pub state: &'static str,
    pub description: String,
}

/// The neighbourhood of one item, given by display id or uid.
pub fn context(ekko: &Ekko, target: &str) -> Result<Context, EkkoError> {
    let all = ekko.storage.get()?;
    let phases = ekko.storage.get_phases()?;
    let id = ekko.validate_ids(&[target.trim_start_matches('@').to_string()], &all)?[0];
    let reader = Reader::new(&all, &phases);
    let item = &all[&id];

    let link = |id: &u32| {
        all.get(id).map(|other| Link {
            id: other.id,
            uid: other.uid.clone(),
            state: state_word(other),
            description: clip(&other.description, TASK_CLIP),
        })
    };
    let mut blockers: Vec<Link> = reader.graph.blockers.get(&id).into_iter().flatten().filter_map(link).collect();
    let mut dependents: Vec<Link> =
        reader.graph.dependents.get(&id).into_iter().flatten().filter_map(link).collect();
    blockers.sort_by_key(|l| l.id);
    dependents.sort_by_key(|l| l.id);
    let attached_to = item
        .attached_to
        .as_deref()
        .and_then(|uid| uid_index(&all).get(uid).copied())
        .and_then(|task| link(&task));

    Ok(Context {
        item: reader.entry(item),
        created: item.date.clone(),
        stashed: item.stashed.is_some(),
        trashed: item.trashed.is_some(),
        blockers,
        dependents,
        attached_to,
    })
}

/// Visible items matching every filter `--list` knows and, when given, a
/// text, case-insensitively. Each item once, in id order, however many
/// boards it is on.
pub fn search(ekko: &Ekko, text: Option<&str>, filters: &[String]) -> Result<Vec<Entry>, EkkoError> {
    let groups = match ekko.list_by_attributes(filters)? {
        Outcome::List(groups) => groups,
        _ => Vec::new(),
    };
    let all = ekko.storage.get()?;
    let phases = ekko.storage.get_phases()?;
    let reader = Reader::new(&all, &phases);
    let needle = text.map(str::to_lowercase);

    let mut seen = HashSet::new();
    let mut entries: Vec<Entry> = groups
        .iter()
        .flat_map(|(_, items)| items)
        .filter(|item| needle.as_deref().is_none_or(|n| item.description.to_lowercase().contains(n)))
        .filter(|item| seen.insert(item.id))
        .filter_map(|item| all.get(&item.id))
        .map(|item| reader.entry(item))
        .collect();
    entries.sort_by_key(|entry| entry.id);
    Ok(entries)
}

/// What moved since a cursor from an earlier read.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Changes {
    pub since: i64,
    /// The cursor to pass next time.
    pub cursor: i64,
    pub items: Vec<Entry>,
}

/// Every item written at or after `since`, put away or not, oldest change
/// first. A trashed item still shows, as `trashed`: the trash keeps what it
/// takes. What leaves storage outright -- archived by `--clear`, or expired
/// from the trash -- leaves nothing to list, which the text says.
pub fn changes(ekko: &Ekko, since: i64) -> Result<Changes, EkkoError> {
    let all = ekko.storage.get()?;
    let phases = ekko.storage.get_phases()?;
    let reader = Reader::new(&all, &phases);
    let mut items: Vec<Entry> =
        all.values().filter(|item| updated(item) >= since).map(|item| reader.entry(item)).collect();
    items.sort_by_key(|entry| (entry.updated_at, entry.id));
    let cursor = all.values().map(updated).max().unwrap_or(since).max(since);
    Ok(Changes { since, cursor, items })
}

impl Changes {
    pub fn text(&self) -> String {
        let mut out = format!("cursor {} \u{b7} {} changed since {}\n", self.cursor, self.items.len(), self.since);
        for entry in &self.items {
            let away = entry.away.map(|away| format!(", {away}")).unwrap_or_default();
            let _ = writeln!(out, "{:>4}. [{}{away}] {}", entry.id, entry.state, clip(&entry.description, TASK_CLIP));
        }
        out.push_str("Items archived by --clear or expired from the trash leave nothing to list; prime again for the whole board.\n");
        out
    }
}

// ---- text -------------------------------------------------------------

/// One listing line: the id first, so a reader can find the item by its
/// number, then the description and whatever sets it apart.
fn entry_line(entry: &Entry, today: &str) -> String {
    let mut line = format!("{:>4}. {}", entry.id, clip(&entry.description, TASK_CLIP));
    let mut meta = Vec::new();
    if entry.state == "paused" {
        meta.push("paused".to_string());
    }
    if let Some(priority @ 2..) = entry.priority {
        meta.push(format!("p{priority}"));
    }
    if let Some(due) = &entry.due {
        meta.push(if due.as_str() < today { format!("due {due}, overdue") } else { format!("due {due}") });
    }
    let boards: Vec<&str> = entry.boards.iter().map(String::as_str).filter(|b| *b != "My Board").collect();
    if !boards.is_empty() {
        meta.push(boards.join(" "));
    }
    if let Some(phase) = &entry.phase {
        meta.push(format!("phase {phase}"));
    }
    if !entry.blocked_by.is_empty() {
        meta.push(format!("\u{21e0} {}", join(&entry.blocked_by)));
    }
    if entry.unblocks > 0 {
        meta.push(format!("unblocks {}", entry.unblocks));
    }
    if entry.starred {
        meta.push("\u{2605}".to_string());
    }
    if !meta.is_empty() {
        let _ = write!(line, " \u{b7} {}", meta.join(" \u{b7} "));
    }
    line
}

/// A note under its task, indented further, id still first on the line.
fn note_line(note: &NoteRef, max: usize) -> String {
    format!("{:>8}. {}", note.id, clip(&note.description, max))
}

fn join(ids: &[u32]) -> String {
    ids.iter().map(u32::to_string).collect::<Vec<_>>().join(", ")
}

impl Prime {
    pub fn text(&self) -> String {
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let mut out = String::new();
        let _ = writeln!(out, "ekko \u{b7} {} \u{b7} cursor {}", self.board, self.cursor);

        let s = &self.stats;
        let tasks = s.complete + s.in_progress + s.paused + s.pending;
        let mut counts = vec![format!("{}/{tasks} tasks done ({}%)", s.complete, s.percent)];
        for (n, word) in [
            (s.in_progress, "in progress"),
            (s.paused, "paused"),
            (s.pending, "pending"),
            (s.cancelled, "cancelled"),
            (s.notes, "notes"),
            (s.stashed, "stashed"),
            (s.trashed, "in the trash"),
        ] {
            if n > 0 {
                counts.push(format!("{n} {word}"));
            }
        }
        let _ = writeln!(out, "{}", counts.join(" \u{b7} "));

        if !self.roadmap.is_empty() {
            let steps: Vec<String> = self
                .roadmap
                .iter()
                .map(|step| {
                    let here = if step.current { " (in progress)" } else { "" };
                    format!("{} {}/{}{here}", step.name, step.complete, step.total)
                })
                .collect();
            let root = if self.rootless > 0 { format!(" \u{b7} {} at the root", self.rootless) } else { String::new() };
            let _ = writeln!(out, "roadmap: {}{root}", steps.join(" \u{2192} "));
        }

        let section = |out: &mut String, title: String, entries: &[Entry], with_notes: bool| {
            if entries.is_empty() {
                return;
            }
            let _ = writeln!(out, "\n{title}");
            for entry in entries {
                let _ = writeln!(out, "{}", entry_line(entry, &today));
                if with_notes {
                    for note in &entry.notes {
                        let _ = writeln!(out, "{}", note_line(note, NOTE_CLIP));
                    }
                }
            }
        };
        section(&mut out, format!("In progress ({})", self.doing.len()), &self.doing, true);
        section(&mut out, format!("Ready, best first ({})", self.ready.len()), &self.ready, true);
        let shown = &self.blocked[..self.blocked.len().min(BLOCKED_SHOWN)];
        section(&mut out, format!("Blocked ({})", self.blocked.len()), shown, false);
        if self.blocked.len() > BLOCKED_SHOWN {
            let _ = writeln!(out, "      +{} more blocked", self.blocked.len() - BLOCKED_SHOWN);
        }

        if !self.recent_notes.is_empty() {
            let _ = writeln!(out, "\nRecent notes, not attached to a task");
            for note in &self.recent_notes {
                let _ = writeln!(out, "{}", note_line(note, NOTE_CLIP).trim_start_matches("    "));
            }
        }

        let mut attention = Vec::new();
        if !self.broken.is_empty() {
            let pairs: Vec<String> = self.broken.iter().map(|(task, blocker)| format!("{task} \u{21e0} {blocker}")).collect();
            attention.push(format!("done work still waiting on open work: {}", pairs.join("; ")));
        }
        if !self.inversions.is_empty() {
            let pairs: Vec<String> = self
                .inversions
                .iter()
                .map(|i| format!("{} ({}) \u{21e0} {} ({})", i.blocked, i.blocked_phase, i.blocker, i.blocker_phase))
                .collect();
            attention.push(format!("dependencies against the phase order: {}", pairs.join("; ")));
        }
        if !self.overdue.is_empty() {
            attention.push(format!("overdue: {}", join(&self.overdue)));
        }
        if !attention.is_empty() {
            let _ = writeln!(out, "\nNeeds attention");
            for line in attention {
                let _ = writeln!(out, "  ! {line}");
            }
        }

        let _ = writeln!(out, "\nAn item in full, with its dependencies and notes: context <id>.");
        out
    }
}

/// Entries one per line, or `empty` when there are none.
pub fn list_text(entries: &[Entry], empty: &str) -> String {
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    if entries.is_empty() {
        return format!("{empty}\n");
    }
    let mut out = String::new();
    for entry in entries {
        let _ = writeln!(out, "{}", entry_line(entry, &today));
    }
    out
}

impl Context {
    pub fn text(&self) -> String {
        let item = &self.item;
        let mut out = String::new();
        let _ = writeln!(out, "{:>4}. {}", item.id, item.description);

        let mut facts = vec![if item.state == "note" { "note".to_string() } else { format!("task, {}", item.state) }];
        if let Some(priority) = item.priority {
            facts.push(format!("priority {priority}"));
        }
        if let Some(due) = &item.due {
            facts.push(format!("due {due}"));
        }
        facts.push(item.boards.join(" "));
        if let Some(phase) = &item.phase {
            facts.push(format!("phase {phase}"));
        }
        if item.starred {
            facts.push("starred".to_string());
        }
        if self.stashed {
            facts.push("stashed".to_string());
        }
        if self.trashed {
            facts.push("in the trash".to_string());
        }
        let _ = writeln!(out, "      {}", facts.join(" \u{b7} "));
        let _ = writeln!(
            out,
            "      uid {} \u{b7} created {} \u{b7} updatedAt {}",
            item.uid.as_deref().unwrap_or("none"),
            self.created,
            item.updated_at
        );

        let links = |out: &mut String, title: &str, links: &[Link]| {
            if links.is_empty() {
                return;
            }
            let _ = writeln!(out, "\n{title}");
            for link in links {
                let _ = writeln!(out, "{:>4}. [{}] {}", link.id, link.state, link.description);
            }
        };
        if let Some(task) = &self.attached_to {
            links(&mut out, "Attached to", std::slice::from_ref(task));
        }
        links(&mut out, "Blocked by", &self.blockers);
        links(&mut out, "Blocks", &self.dependents);
        if item.unblocks > 0 {
            let _ = writeln!(out, "      {} open tasks wait on this, directly or through others", item.unblocks);
        }
        if !item.notes.is_empty() {
            let _ = writeln!(out, "\nNotes attached");
            for note in &item.notes {
                let _ = writeln!(out, "{:>4}. {}", note.id, note.description);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Storage;
    use std::path::PathBuf;

    fn words(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn scratch(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("ekko-agent-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn board(tag: &str) -> (Ekko, PathBuf) {
        let dir = scratch(tag);
        (Ekko::new(Storage::new(&dir).unwrap()), dir)
    }

    fn ids(entries: &[Entry]) -> Vec<u32> {
        entries.iter().map(|entry| entry.id).collect()
    }

    /// The ids a text view delivers, read the way the resume eval reads them:
    /// from lines that start with one.
    fn line_ids(text: &str) -> Vec<u32> {
        text.lines().filter_map(|line| line.trim_start().split_once(". ")?.0.parse().ok()).collect()
    }

    /// Each key only breaks the ties the keys before it leave: work under way,
    /// then priority, then the nearer deadline, then how much waits on it,
    /// then age. Blocked work is not a candidate at all.
    #[test]
    fn next_orders_by_progress_priority_deadline_what_waits_and_age() {
        let (ekko, dir) = board("next");
        ekko.create_task(&words(&["plain"])).unwrap();
        ekko.create_task(&words(&["urgent", "p:3"])).unwrap();
        ekko.create_task(&words(&["urgent and due", "p:3", "d:2026-01-01"])).unwrap();
        ekko.create_task(&words(&["unlocks work"])).unwrap();
        ekko.create_task(&words(&["waits on 4"])).unwrap();
        ekko.create_task(&words(&["waits on 5"])).unwrap();
        ekko.create_task(&words(&["under way"])).unwrap();
        ekko.set_blocked_by(&words(&["@5", "4"])).unwrap();
        ekko.set_blocked_by(&words(&["@6", "5"])).unwrap();
        ekko.set_state(&words(&["@7", "progress"]), false).unwrap();

        let order = next(&ekko, None).unwrap();
        assert_eq!(ids(&order), vec![7, 3, 2, 4, 1]);
        assert_eq!(order[3].unblocks, 2, "4 lets 5 move, and 6 after it");
        assert_eq!(ids(&next(&ekko, Some(2)).unwrap()), vec![7, 3]);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Phase order outranks priority, and the root comes after every phase.
    #[test]
    fn an_earlier_phase_comes_before_a_higher_priority_in_a_later_one() {
        let (ekko, dir) = board("phases");
        ekko.set_phases(&words(&["early", "late"])).unwrap();
        ekko.create_task_in(&words(&["late but urgent", "p:3"]), Some("late")).unwrap();
        ekko.create_task_in(&words(&["early"]), Some("early")).unwrap();
        ekko.create_task_in(&words(&["at the root", "p:3"]), None).unwrap();

        assert_eq!(ids(&next(&ekko, None).unwrap()), vec![2, 1, 3]);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A diamond reaches its far corner twice and counts it once, and a
    /// closed task passes nothing on through itself.
    #[test]
    fn what_waits_is_counted_once_per_task_and_only_through_open_work() {
        let (ekko, dir) = board("downstream");
        for name in ["root", "left", "right", "join"] {
            ekko.create_task(&words(&[name])).unwrap();
        }
        ekko.set_blocked_by(&words(&["@2", "1"])).unwrap();
        ekko.set_blocked_by(&words(&["@3", "1"])).unwrap();
        ekko.set_blocked_by(&words(&["@4", "2", "3"])).unwrap();
        let unblocks = |ekko: &Ekko| next(ekko, None).unwrap().iter().find(|e| e.id == 1).unwrap().unblocks;

        assert_eq!(unblocks(&ekko), 3);
        ekko.set_state(&words(&["@2", "cancelled"]), false).unwrap();
        assert_eq!(unblocks(&ekko), 2, "4 still waits through 3, and 2 no longer waits at all");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The resume view delivers every item it lists on a line that starts with
    /// its id, with each reason directly under the work it explains.
    #[test]
    fn prime_lists_each_item_by_id_with_its_reason_under_its_work() {
        let (ekko, dir) = board("prime");
        ekko.create_task(&words(&["doing"])).unwrap();
        ekko.create_note(&words(&["why doing"])).unwrap();
        ekko.create_task(&words(&["ready"])).unwrap();
        ekko.create_task(&words(&["waiting"])).unwrap();
        ekko.create_note(&words(&["loose thought"])).unwrap();
        ekko.set_state(&words(&["@1", "progress"]), false).unwrap();
        ekko.set_attached_to(&words(&["@2", "1"])).unwrap();
        ekko.set_blocked_by(&words(&["@4", "1"])).unwrap();

        let view = prime(&ekko, "default board").unwrap();
        assert_eq!((ids(&view.doing), ids(&view.ready), ids(&view.blocked)), (vec![1], vec![3], vec![4]));

        let text = view.text();
        assert_eq!(line_ids(&text), vec![1, 2, 3, 4, 5], "{text}");
        let lines: Vec<&str> = text.lines().collect();
        let reason = lines.iter().position(|line| line.trim_start().starts_with("2. ")).unwrap();
        assert!(lines[reason - 1].trim_start().starts_with("1. "), "the reason is not under its work:\n{text}");

        let newest = ekko.storage.get().unwrap().values().map(updated).max().unwrap();
        assert_eq!(view.cursor, newest);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// What the board holds against itself is named, not left to be noticed.
    #[test]
    fn prime_names_what_needs_attention() {
        let (ekko, dir) = board("attention");
        ekko.create_task(&words(&["blocker"])).unwrap();
        ekko.create_task(&words(&["forced"])).unwrap();
        ekko.create_task(&words(&["late", "d:2020-01-01"])).unwrap();
        ekko.set_blocked_by(&words(&["@2", "1"])).unwrap();
        ekko.set_state(&words(&["@2", "done"]), true).unwrap();

        let text = prime(&ekko, "default board").unwrap().text();
        assert!(text.contains("done work still waiting on open work: 2 \u{21e0} 1"), "{text}");
        assert!(text.contains("overdue: 3"), "{text}");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// One hop along every relation, from either end, by id or by uid.
    #[test]
    fn context_follows_every_relation_one_hop_by_id_or_uid() {
        let (ekko, dir) = board("context");
        ekko.create_task(&words(&["blocker"])).unwrap();
        ekko.create_task(&words(&["blocked"])).unwrap();
        ekko.create_note(&words(&["the reason"])).unwrap();
        ekko.set_blocked_by(&words(&["@2", "1"])).unwrap();
        ekko.set_attached_to(&words(&["@3", "2"])).unwrap();

        let blocked = context(&ekko, "2").unwrap();
        assert_eq!(blocked.blockers.iter().map(|link| link.id).collect::<Vec<_>>(), vec![1]);
        assert_eq!(blocked.item.notes.iter().map(|note| note.id).collect::<Vec<_>>(), vec![3]);

        let uid = ekko.storage.get().unwrap()[&1].uid.clone().unwrap();
        let blocker = context(&ekko, &uid).unwrap();
        assert_eq!(blocker.dependents.iter().map(|link| link.id).collect::<Vec<_>>(), vec![2]);
        assert_eq!(blocker.item.unblocks, 1);

        assert_eq!(context(&ekko, "@3").unwrap().attached_to.map(|link| link.id), Some(2));
        assert!(matches!(context(&ekko, "9"), Err(EkkoError::InvalidId(_))));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A session opened anywhere inside a repository primes the project named
    /// after it, and a directory with no such project matches nothing.
    #[test]
    fn a_repository_is_matched_to_the_project_named_after_it() {
        let home = scratch("home");
        crate::directory::retrieve_project_directory(&home, "minium", true).unwrap();
        let repo = home.join("src").join("minium");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(repo.join("deep").join("inside")).unwrap();
        let elsewhere = home.join("src").join("other");
        std::fs::create_dir_all(&elsewhere).unwrap();

        let matched = crate::directory::project_named_after(&home, &repo.join("deep").join("inside"));
        assert_eq!(matched.as_deref(), Some("minium"));
        assert_eq!(crate::directory::project_named_after(&home, &elsewhere), None);

        std::fs::remove_dir_all(&home).ok();
    }

    /// Everything written at or after the cursor, put away or not, and
    /// nothing written before it.
    #[test]
    fn changes_lists_what_moved_since_a_cursor_including_what_was_put_away() {
        let (ekko, dir) = board("changes");
        ekko.create_task(&words(&["untouched"])).unwrap();
        ekko.create_task(&words(&["will be done"])).unwrap();
        ekko.create_task(&words(&["will be deleted"])).unwrap();
        let cursor = prime(&ekko, "default board").unwrap().cursor;
        std::thread::sleep(std::time::Duration::from_millis(5));
        ekko.set_state(&words(&["@2", "done"]), false).unwrap();
        ekko.delete_items(&words(&["3"])).unwrap();

        let seen = changes(&ekko, cursor + 1).unwrap();
        assert_eq!(ids(&seen.items), vec![2, 3]);
        assert_eq!(seen.items[1].away, Some("trashed"));
        assert!(seen.cursor > cursor);

        std::fs::remove_dir_all(&dir).ok();
    }
}
