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

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::Duration;

use chrono::Datelike;
use serde::Serialize;

use crate::ekko::{broken_dependencies, holds, phase_order, Ekko, EkkoError, Outcome};
use crate::item::{Item, State};
use crate::lexical::{self, Query};
use crate::render::{Inversion, ProjectSummary, RoadmapStep, Stats};
use crate::storage::ItemMap;

/// How much of a reason `prime` quotes before pointing at `context`.
const NOTE_CLIP: usize = 300;
/// How much of a task's description a one-line listing carries.
const TASK_CLIP: usize = 160;
/// Unattached notes `prime` shows, newest first.
const RECENT_NOTES: usize = 5;
/// Blocked tasks `prime` lists before counting the rest.
const BLOCKED_SHOWN: usize = 20;
/// Entries of a long listing whose downstream count is worked out. Counting
/// walks the graph once per entry, which on a long chain is quadratic in the
/// board -- 12 seconds for a prime over 10,000 tasks -- for lines no view
/// shows: a prime fits fewer than this many.
const COUNTED: usize = 50;

// Taskwarrior's default urgency coefficients (taskwarrior.org/docs/urgency,
// Task::urgency_due in src/Task.cpp), applied by `urgency`.
const URGENCY_DUE: f64 = 12.0;
const URGENCY_BLOCKING: f64 = 8.0;
const URGENCY_AGE: f64 = 2.0;
const URGENCY_AGE_DAYS: f64 = 365.0;
/// The most a prime says, in characters. Claude Code puts at most 10,000
/// characters of a hook's output into context and turns anything longer into
/// a short preview and a file path (hooks.md; note 165 checked it at 9,900 and
/// 10,100), which would cut the resume view off where it matters. Six
/// thousand leaves room for what a handoff adds under the work in progress.
const PRIME_BUDGET: usize = 6_000;
/// Ready tasks whose attached notes a prime quotes; later ones show the task.
const READY_WITH_NOTES: usize = 3;
/// The most of a handoff a prime quotes, in characters, on top of
/// `PRIME_BUDGET`: together they stay under the 10,000 Claude Code keeps of a
/// hook's output. About a thousand tokens -- where the last session stopped,
/// what it decided and why, the files and the next step; a longer handoff
/// shows its start and points at `context` for the rest.
const HANDOFF_BUDGET: usize = 3_500;
/// What a cut prime keeps of each section before any section grows into the
/// room left: the ten best ready tasks, the first blocked ones -- whose lines
/// name what holds them -- and the newest loose notes. Without these a long
/// ready list filled the whole budget, and on a board of 20,000 items the
/// blocked work and the notes were never mentioned.
const READY_KEPT: usize = 10;
const BLOCKED_KEPT: usize = 5;
const NOTES_KEPT: usize = 3;
/// The longest a "+N more" line gets, reserved for each section a cut leaves short.
const MORE_LINE: usize = 48;
/// The most a resumed session is told about what moved before the whole
/// prime is cheaper to read than the list.
const RESUME_CHANGES: usize = 2_000;
/// How long the hook remembers the cursor it served a session.
const SESSION_KEPT: Duration = Duration::from_secs(30 * 86_400);
/// Hits `search` shows unless asked for more.
pub const SEARCH_LIMIT: usize = 20;

/// How much of the notes around an item a context quotes. Concise clips each
/// attached note to `NOTE_CLIP` characters with a count of the rest -- the way
/// prime already quotes them -- and full prints them whole. The item's own
/// text is whole either way: it is what was asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detail {
    Concise,
    Full,
}
/// How much of a hit's text `search` shows, around where it matched.
const SNIPPET: usize = 160;

/// A board's relations resolved to display ids: worked out once per version
/// of the board and shared by every read of that version, so a warm server
/// answers a `context` without walking twenty thousand items to find three.
struct Links {
    /// Display ids by uid.
    uids: HashMap<String, u32>,
    /// Each item's recorded blockers, by display id.
    blockers: HashMap<u32, Vec<u32>>,
    /// Each item's dependents -- the items recording it as a blocker.
    dependents: HashMap<u32, Vec<u32>>,
    /// Visible notes, in id order, by the uid of the task each is attached to.
    attached: HashMap<String, Vec<u32>>,
}

impl Links {
    fn build(all: &ItemMap) -> Self {
        let uids: HashMap<String, u32> =
            all.iter().filter_map(|(id, item)| Some((item.uid.clone()?, *id))).collect();
        let mut blockers: HashMap<u32, Vec<u32>> = HashMap::new();
        let mut dependents: HashMap<u32, Vec<u32>> = HashMap::new();
        let mut attached: HashMap<String, Vec<u32>> = HashMap::new();
        for (id, item) in all {
            for uid in item.blocked_by.iter().flatten() {
                if let Some(&blocker) = uids.get(uid.as_str()) {
                    blockers.entry(*id).or_default().push(blocker);
                    dependents.entry(blocker).or_default().push(*id);
                }
            }
            if let Some(task) = item.attached_to.as_deref().filter(|_| visible(item)) {
                attached.entry(task.to_string()).or_default().push(*id);
            }
        }
        Links { uids, blockers, dependents, attached }
    }
}

/// Links already built, each beside the version of the board it was built
/// from. Held weakly: storage keeps the version it last read alive, and a
/// version it let go of can never be asked about again.
static LINKS: Mutex<Vec<(Weak<ItemMap>, Arc<Links>)>> = Mutex::new(Vec::new());

/// The links of a shared board, built on the first read of its version.
fn links_of(all: &Arc<ItemMap>) -> Arc<Links> {
    let mut kept = LINKS.lock().unwrap_or_else(PoisonError::into_inner);
    kept.retain(|(board, _)| board.strong_count() > 0);
    if let Some((_, links)) = kept.iter().find(|(board, _)| board.upgrade().is_some_and(|board| Arc::ptr_eq(&board, all)))
    {
        return Arc::clone(links);
    }
    let links = Arc::new(Links::build(all));
    kept.push((Arc::downgrade(all), Arc::clone(&links)));
    links
}

/// Dependencies resolved once for a whole board, in both directions.
struct Graph<'a> {
    all: &'a ItemMap,
    links: Arc<Links>,
}

impl<'a> Graph<'a> {
    fn new(all: &'a ItemMap, links: Arc<Links>) -> Self {
        Graph { all, links }
    }

    /// The blockers of `id` that still hold, in id order.
    fn open_blockers(&self, id: u32) -> Vec<u32> {
        let mut ids: Vec<u32> = self
            .links
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
            for &dependent in self.links.dependents.get(&at).into_iter().flatten() {
                if self.all.get(&dependent).is_some_and(holds) && seen.insert(dependent) {
                    stack.push(dependent);
                }
            }
        }
        seen.remove(&id);
        seen.len()
    }

    /// The open tasks `id` waits on, directly or through other open tasks,
    /// and the roots among them: open blockers with no open blocker of their
    /// own -- the work free to start that, finished, would move `id`. Each is
    /// counted once however many paths lead to it.
    fn upstream(&self, id: u32) -> (usize, Vec<u32>) {
        let mut seen = HashSet::new();
        let mut stack = vec![id];
        let mut roots = Vec::new();
        while let Some(at) = stack.pop() {
            let open = self.open_blockers(at);
            if open.is_empty() && at != id {
                roots.push(at);
            }
            for blocker in open {
                if blocker != id && seen.insert(blocker) {
                    stack.push(blocker);
                }
            }
        }
        roots.sort_unstable();
        (seen.len(), roots)
    }

    /// What every open task inherits from the open work waiting on it, in one
    /// pass from the tasks nothing waits on back to their blockers -- Kahn's
    /// order over open dependents, so each task is settled only after all it
    /// holds up is. Exact on a diamond: a maximum and a minimum count nothing
    /// twice. A task caught in a cycle, which only a hand-edited file can
    /// hold, keeps its own values.
    fn inherited(&self) -> HashMap<u32, Inherited> {
        let open = |id: &u32| self.all.get(id).is_some_and(holds);
        let mut waiting: HashMap<u32, usize> = self
            .all
            .values()
            .filter(|item| holds(item))
            .map(|item| (item.id, self.links.dependents.get(&item.id).into_iter().flatten().filter(|d| open(d)).count()))
            .collect();
        let mut settle: Vec<u32> = waiting.iter().filter(|(_, n)| **n == 0).map(|(id, _)| *id).collect();
        let mut values: HashMap<u32, Inherited> = HashMap::new();
        while let Some(id) = settle.pop() {
            let item = &self.all[&id];
            let mut value = Inherited { priority: item.priority.unwrap_or(1), finish_by: due_day(item), waited_on: false };
            for dependent in self.links.dependents.get(&id).into_iter().flatten() {
                let Some(above) = values.get(dependent) else { continue };
                value.priority = value.priority.max(above.priority);
                if let Some(day) = above.finish_by {
                    value.finish_by = Some(value.finish_by.map_or(day - 1, |own| own.min(day - 1)));
                }
                value.waited_on = true;
            }
            values.insert(id, value);
            for blocker in self.links.blockers.get(&id).into_iter().flatten() {
                if let Some(n) = waiting.get_mut(blocker) {
                    *n -= 1;
                    if *n == 0 {
                        settle.push(*blocker);
                    }
                }
            }
        }
        values
    }
}

/// What a task takes on from the open work waiting on it: the highest
/// priority among that work, the latest day it can finish for that work to
/// meet its due dates -- a day per task in between, the critical path method's
/// backward pass -- and whether any open work waits on it at all.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Inherited {
    priority: u8,
    /// Days from the common era, as chrono counts them.
    finish_by: Option<i32>,
    waited_on: bool,
}

fn due_day(item: &Item) -> Option<i32> {
    let due = item.due_date.as_deref()?;
    chrono::NaiveDate::parse_from_str(due, "%Y-%m-%d").ok().map(|date| date.num_days_from_ce())
}

fn day_date(day: i32) -> Option<String> {
    chrono::NaiveDate::from_num_days_from_ce_opt(day).map(|date| date.format("%Y-%m-%d").to_string())
}

/// Taskwarrior's urgency for a task, on what it inherits: 12 for a date on a
/// 21-day ramp, 8 for holding up open work, 6 or 3.9 for priority 3 or 2, and
/// up to 2 for age. The weights are Taskwarrior's defaults; see note 166 for
/// where they place this board's own work.
fn urgency(item: &Item, inherited: Option<Inherited>, today: i32, now: i64) -> f64 {
    let inherited =
        inherited.unwrap_or(Inherited { priority: item.priority.unwrap_or(1), finish_by: due_day(item), waited_on: false });
    let due = inherited.finish_by.map_or(0.0, |day| URGENCY_DUE * due_ramp(today - day));
    let blocking = if inherited.waited_on { URGENCY_BLOCKING } else { 0.0 };
    let priority = match inherited.priority {
        3 => 6.0,
        2 => 3.9,
        _ => 0.0,
    };
    let age = (now - item.timestamp).max(0) as f64 / 86_400_000.0;
    due + blocking + priority + URGENCY_AGE * (age / URGENCY_AGE_DAYS).min(1.0)
}

/// Taskwarrior's `Task::urgency_due`: 21 days mapped onto 0.2 to 1.0, from two
/// weeks ahead of the date to a week past it.
fn due_ramp(overdue_days: i32) -> f64 {
    let overdue = f64::from(overdue_days);
    if overdue >= 7.0 {
        1.0
    } else if overdue >= -14.0 {
        (overdue + 14.0) * 0.8 / 21.0 + 0.2
    } else {
        0.2
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
    /// The priority this inherits from open work waiting on it, when higher
    /// than its own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inherits: Option<u8>,
    /// The day this has to be done by for the work waiting on it to meet its
    /// dates, when that is not its own due date.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_by: Option<String>,
    /// A note that is its task's handoff; see `Item::handoff`.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub handoff: bool,
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
    order: HashMap<&'a str, usize>,
    today: String,
}

impl<'a> Reader<'a> {
    fn new(all: &'a ItemMap, links: Arc<Links>, phases: &'a [String]) -> Self {
        Reader {
            graph: Graph::new(all, links),
            order: phase_order(phases),
            today: chrono::Local::now().format("%Y-%m-%d").to_string(),
        }
    }

    /// A reader over a shared board, on the links its version already has.
    fn shared(all: &'a Arc<ItemMap>, phases: &'a [String]) -> Self {
        Reader::new(all, links_of(all), phases)
    }

    /// The display id holding `uid`.
    fn uid(&self, uid: &str) -> Option<u32> {
        self.graph.links.uids.get(uid).copied()
    }

    fn entry(&self, item: &Item) -> Entry {
        self.entry_counting(item, true)
    }

    /// An entry, with how much open work waits on it worked out only when
    /// `count` says so.
    fn entry_counting(&self, item: &Item, count: bool) -> Entry {
        let notes = item
            .uid
            .as_deref()
            .and_then(|uid| self.graph.links.attached.get(uid))
            .into_iter()
            .flatten()
            .filter_map(|id| self.graph.all.get(id))
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
            // Only open work holds anything up: a finished task already let
            // go of what waited on it.
            unblocks: if count && holds(item) { self.graph.downstream(item.id) } else { 0 },
            updated_at: updated(item),
            notes,
            inherits: None,
            finish_by: None,
            handoff: item.handoff,
        }
    }

    fn ready(&self, item: &Item) -> bool {
        visible(item) && holds(item) && self.graph.open_blockers(item.id).is_empty()
    }

    /// What to take up next, best first: work already in progress, then
    /// earlier phases before later ones (the root after every phase), then
    /// urgency, then the older item. Returns the entries within `limit` and
    /// how many tasks were candidates.
    ///
    /// Urgency is worked out on what a task inherits from the work waiting on
    /// it, so a low-priority prerequisite of urgent work ranks as urgent. The
    /// lexicographic order this replaces could not see through a dependency:
    /// it put a p1 blocker of a p3 task due tomorrow below an unrelated p2,
    /// and any due date, however far, above work others wait on.
    fn next(&self, limit: Option<usize>) -> (Vec<Entry>, usize) {
        let inherited = self.graph.inherited();
        let now = chrono::Local::now();
        let today = now.date_naive().num_days_from_ce();
        let mut candidates: Vec<(f64, &Item)> = self
            .graph
            .all
            .values()
            .filter(|item| visible(item) && (State::of(item) == Some(State::Progress) || self.ready(item)))
            .map(|item| (urgency(item, inherited.get(&item.id).copied(), today, now.timestamp_millis()), item))
            .collect();
        let rank = |item: &Item| {
            (
                State::of(item) != Some(State::Progress),
                item.phase.as_deref().and_then(|p| self.order.get(p)).copied().unwrap_or(usize::MAX),
            )
        };
        candidates.sort_by(|(a_urgency, a), (b_urgency, b)| {
            rank(a).cmp(&rank(b)).then(b_urgency.total_cmp(a_urgency)).then(a.id.cmp(&b.id))
        });
        let total = candidates.len();
        let entries = candidates
            .into_iter()
            .take(limit.unwrap_or(usize::MAX))
            .enumerate()
            .map(|(at, (_, item))| {
                let mut entry = self.entry_counting(item, at < COUNTED);
                if let Some(taken) = inherited.get(&item.id) {
                    entry.inherits = (taken.priority > item.priority.unwrap_or(1)).then_some(taken.priority);
                    entry.finish_by = taken.finish_by.filter(|day| due_day(item) != Some(*day)).and_then(day_date);
                }
                entry
            })
            .collect();
        (entries, total)
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
pub(crate) fn clip(text: &str, max: usize) -> String {
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
    /// The board revision this read saw: pass it to `changes` later to hear
    /// only what moved since.
    pub cursor: i64,
    pub stats: Stats,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub roadmap: Vec<RoadmapStep>,
    #[serde(skip_serializing_if = "is_zero_u32")]
    pub rootless: u32,
    pub doing: Vec<Entry>,
    pub ready: Vec<Entry>,
    /// The first blocked tasks, by priority; `blocked_total` counts them all.
    pub blocked: Vec<Entry>,
    pub blocked_total: usize,
    pub recent_notes: Vec<NoteRef>,
    /// Done tasks still waiting on open work, as (task, blocker) pairs.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub broken: Vec<(u32, u32)>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub inversions: Vec<Inversion>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub overdue: Vec<u32>,
    /// Open work ready only because what it waited on was cancelled, as
    /// (task, cancelled blocker) pairs: free to start, though the thing it
    /// needed will never happen.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub freed_by_cancelling: Vec<(u32, u32)>,
    /// The newest handoff on open work: where the last session stopped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handoff: Option<Handoff>,
}

/// A handoff as the prime shows it: the note, and the task it hands over.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Handoff {
    pub id: u32,
    pub uid: Option<String>,
    pub task: u32,
    pub task_state: &'static str,
    pub updated_at: i64,
    pub description: String,
}

fn is_zero_u32(n: &u32) -> bool {
    *n == 0
}

/// Builds the resume view of the board `ekko` has open, labelled `board`.
pub fn prime(ekko: &Ekko, board: &str) -> Result<Prime, EkkoError> {
    let all = ekko.storage.get_shared()?;
    let phases = ekko.storage.get_phases()?;
    let reader = Reader::shared(&all, &phases);

    let (next, _) = reader.next(None);
    let (mut doing, mut ready): (Vec<Entry>, Vec<Entry>) =
        next.into_iter().partition(|entry| entry.state == "in progress");

    let mut blocked: Vec<&Item> = all
        .values()
        .filter(|item| visible(item) && holds(item) && State::of(item) != Some(State::Progress))
        .filter(|item| !reader.graph.open_blockers(item.id).is_empty())
        .collect();
    blocked.sort_by_key(|item| (std::cmp::Reverse(item.priority.unwrap_or(1)), item.id));
    let blocked_total = blocked.len();
    // Entries only for what a prime shows: working one out walks the graph,
    // and a long chain holds thousands of blocked tasks nobody reads.
    let mut blocked: Vec<Entry> = blocked.into_iter().take(BLOCKED_SHOWN).map(|item| reader.entry(item)).collect();

    // The newest handoff on open work, shown once in its own section rather
    // than clipped again among the notes of its task.
    let handoff = all
        .values()
        .filter(|note| note.handoff && visible(note))
        .filter_map(|note| {
            let task = &all[&reader.uid(note.attached_to.as_deref()?)?];
            (visible(task) && holds(task)).then_some((note, task))
        })
        .max_by_key(|(note, _)| (updated(note), note.id))
        .map(|(note, task)| Handoff {
            id: note.id,
            uid: note.uid.clone(),
            task: task.id,
            task_state: state_word(task),
            updated_at: updated(note),
            description: note.description.clone(),
        });
    if let Some(shown) = &handoff {
        for entry in doing.iter_mut().chain(ready.iter_mut()).chain(blocked.iter_mut()) {
            entry.notes.retain(|note| note.id != shown.id);
        }
    }

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

    let freed_by_cancelling = all
        .values()
        .filter(|item| visible(item) && holds(item) && reader.graph.open_blockers(item.id).is_empty())
        .flat_map(|item| {
            item.blocked_by
                .iter()
                .flatten()
                .filter_map(|uid| reader.uid(uid))
                .filter(|blocker| all.get(blocker).and_then(State::of) == Some(State::Cancelled))
                .map(move |blocker| (item.id, blocker))
        })
        .collect();

    Ok(Prime {
        board: board.to_string(),
        cursor: ekko.storage.get_counters()?.revision as i64,
        stats: ekko.compute_stats(&all),
        roadmap,
        rootless,
        doing,
        ready,
        blocked,
        blocked_total,
        recent_notes,
        broken: broken_dependencies(&all).into_iter().collect(),
        inversions,
        overdue,
        freed_by_cancelling,
        handoff,
    })
}

/// What to take up next, best first; see `Reader::next` for the order.
pub fn next(ekko: &Ekko, limit: Option<usize>) -> Result<Vec<Entry>, EkkoError> {
    Ok(next_listed(ekko, limit)?.0)
}

/// `next`, with how many tasks were candidates in all.
pub fn next_listed(ekko: &Ekko, limit: Option<usize>) -> Result<(Vec<Entry>, usize), EkkoError> {
    let all = ekko.storage.get_shared()?;
    let phases = ekko.storage.get_phases()?;
    Ok(Reader::shared(&all, &phases).next(limit))
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
    /// Open tasks this waits on, directly or through others.
    #[serde(skip_serializing_if = "is_zero")]
    pub waits_on: usize,
    /// The open work free to start that stands before this -- what an agent
    /// otherwise finds by reading one blocker after another.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub roots: Vec<Link>,
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
    Ok(contexts(ekko, &[target.to_string()])?.remove(0))
}

/// The neighbourhoods of several items from one read of the board, in the
/// order asked and each once -- what an agent reading a few search hits
/// otherwise spends a call apiece on. An id that names nothing refuses them
/// all, the way it does for every other command.
pub fn contexts(ekko: &Ekko, targets: &[String]) -> Result<Vec<Context>, EkkoError> {
    let all = ekko.storage.get_shared()?;
    let phases = ekko.storage.get_phases()?;
    let raw: Vec<String> = targets.iter().map(|target| target.trim_start_matches('@').to_string()).collect();
    let ids = ekko.validate_ids(&raw, &all)?;
    let reader = Reader::shared(&all, &phases);
    Ok(ids.into_iter().map(|id| neighbourhood(&all, &reader, id)).collect())
}

fn neighbourhood(all: &ItemMap, reader: &Reader<'_>, id: u32) -> Context {
    let item = &all[&id];

    let link = |id: &u32| {
        all.get(id).map(|other| Link {
            id: other.id,
            uid: other.uid.clone(),
            state: state_word(other),
            description: clip(&other.description, TASK_CLIP),
        })
    };
    let mut blockers: Vec<Link> = reader.graph.links.blockers.get(&id).into_iter().flatten().filter_map(link).collect();
    let mut dependents: Vec<Link> =
        reader.graph.links.dependents.get(&id).into_iter().flatten().filter_map(link).collect();
    blockers.sort_by_key(|l| l.id);
    dependents.sort_by_key(|l| l.id);
    let attached_to = item
        .attached_to
        .as_deref()
        .and_then(|uid| reader.uid(uid))
        .and_then(|task| link(&task));
    let (waits_on, root_ids) = reader.graph.upstream(id);
    let roots = root_ids.iter().filter_map(link).collect();

    Context {
        item: reader.entry(item),
        created: item.date.clone(),
        stashed: item.stashed.is_some(),
        trashed: item.trashed.is_some(),
        blockers,
        waits_on,
        roots,
        dependents,
        attached_to,
    }
}

/// What `search` found: the hits it shows, each with the text it shows, how
/// many matched in all, and whether they match every word of the text or,
/// none doing so, some. With neither text nor filters there are no hits, only
/// a summary of what the board holds.
#[derive(Debug, Default)]
pub struct Found {
    pub total: usize,
    pub every_word: bool,
    pub hits: Vec<(Entry, String)>,
    pub summary: Option<String>,
}

/// Visible items passing every filter `--list` knows and, when given, holding
/// the words of a text as `lexical` matches and ranks them: best first with
/// the part of each text that matched, or in id order when there is no text.
/// At most `limit` are shown, with the total. Each item once, however many
/// boards it is on.
pub fn search(ekko: &Ekko, text: Option<&str>, filters: &[String], limit: usize) -> Result<Found, EkkoError> {
    let query = Query::new(text.unwrap_or_default());
    let all = ekko.storage.get_shared()?;
    if query.is_empty() && filters.is_empty() {
        return Ok(Found { summary: Some(summary(&all)), ..Found::default() });
    }
    let groups = match ekko.list_by_attributes(filters)? {
        Outcome::List(groups) => groups,
        _ => Vec::new(),
    };
    let phases = ekko.storage.get_phases()?;
    let reader = Reader::shared(&all, &phases);

    let mut seen = HashSet::new();
    let mut candidates: Vec<&Item> = groups
        .iter()
        .flat_map(|(_, items)| items)
        .filter(|item| seen.insert(item.id))
        .filter_map(|item| all.get(&item.id))
        .collect();
    candidates.sort_by_key(|item| item.id);

    if query.is_empty() {
        let hits =
            candidates.iter().take(limit).map(|item| (reader.entry(item), clip(&item.description, TASK_CLIP))).collect();
        return Ok(Found { total: candidates.len(), every_word: true, hits, summary: None });
    }
    let texts: Vec<&str> = candidates.iter().map(|item| item.description.as_str()).collect();
    let ranked = lexical::rank(&query, &texts);
    let hits = ranked
        .hits
        .iter()
        .take(limit)
        .map(|hit| {
            let item = candidates[hit.index];
            (reader.entry(item), lexical::snippet(&item.description, &query, SNIPPET))
        })
        .collect();
    Ok(Found { total: ranked.hits.len(), every_word: ranked.every_word, hits, summary: None })
}

/// What is put away, the way `search` lists what it finds: the stash in id
/// order, then the trash oldest first with the days each item has left there,
/// one line per item with its state and its text clipped, at most `limit` of
/// each and a total when there are more. The terminal's stash view prints
/// every note whole, grouped by board -- 8 KB on this project's own board.
pub fn away(ekko: &Ekko, stash: bool, trash: bool, limit: usize) -> Result<String, EkkoError> {
    let all = ekko.storage.get_shared()?;
    let phases = ekko.storage.get_phases()?;
    let reader = Reader::shared(&all, &phases);
    let now = chrono::Local::now().timestamp_millis();
    let mut out = String::new();
    let mut section = |title: &str, items: Vec<&Item>, empty: &str| {
        let _ = writeln!(out, "{title} ({})", items.len());
        if items.is_empty() {
            let _ = writeln!(out, "      {empty}");
        }
        for item in items.iter().take(limit) {
            let entry = reader.entry_counting(item, false);
            let mut body = format!("[{}] {}", entry.state, clip(&item.description, TASK_CLIP));
            if let Some(at) = item.trashed {
                let left = crate::render::TRASH_DAYS - (now - at) / 86_400_000;
                let _ = write!(body, " \u{b7} {}", match left {
                    d if d <= 0 => "expires today".to_string(),
                    1 => "expires tomorrow".to_string(),
                    d => format!("expires in {d}d"),
                });
            }
            let _ = writeln!(out, "{}", listed_line(&entry, &body, true, &reader.today));
        }
        if items.len() > limit {
            let _ = writeln!(out, "      {} of {} shown: raise limit.", limit, items.len());
        }
    };
    if stash {
        let items: Vec<&Item> = all.values().filter(|item| item.stashed.is_some() && item.trashed.is_none()).collect();
        section("Stash", items, "nothing is stashed");
    }
    if trash {
        let mut items: Vec<&Item> = all.values().filter(|item| item.trashed.is_some()).collect();
        items.sort_by_key(|item| (item.trashed, item.id));
        section("Trash", items, "the trash is empty");
    }
    Ok(out)
}

/// What the `handoff` prompt asks of an agent about to lose its context: to
/// write, now, the note the next session resumes from -- which task it hands
/// over, what to put in it and in what order, and the handoff it replaces.
/// `task` names the task; without it, the task in progress is meant, and
/// when that is not exactly one task the prompt says so and lists them.
pub fn handoff_prompt(ekko: &Ekko, task: Option<&str>) -> Result<String, EkkoError> {
    let all = ekko.storage.get_shared()?;
    let phases = ekko.storage.get_phases()?;
    let reader = Reader::shared(&all, &phases);
    let tasks: Vec<&Item> = match task {
        Some(target) => {
            let ids = ekko.validate_ids(&[target.trim_start_matches('@').to_string()], &all)?;
            let item = &all[&ids[0]];
            if !item.is_task || !holds(item) {
                return Err(EkkoError::InvalidInput(format!(
                    "{} is not open work, and only an open task has anything to hand over",
                    item.id
                )));
            }
            vec![item]
        }
        None => all.values().filter(|item| visible(item) && State::of(item) == Some(State::Progress)).collect(),
    };

    let mut out = String::new();
    match tasks.as_slice() {
        [task] => {
            let _ = writeln!(
                out,
                "Write the handoff for task {} now, before this session's context is cleared: create with kind \"handoff\" and attached_to \"{}\".",
                task.id,
                task.uid.as_deref().unwrap_or_default()
            );
        }
        [] => out.push_str(
            "Write the handoff for the task this session worked on, now, before its context is cleared: create with kind \"handoff\" and attached_to that task. No task is in progress, so ask the user which one if it is not clear.\n",
        ),
        many => {
            out.push_str("Write the handoff for the task this session worked on, now, before its context is cleared: create with kind \"handoff\" and attached_to that task. More than one task is in progress:\n");
            for task in many {
                let _ = writeln!(out, "{:>4}. {}", task.id, clip(&task.description, TASK_CLIP));
            }
        }
    }
    let _ = write!(
        out,
        "\nThe next session reads it under the prime in place of this transcript, so write what only this session knows, in this order and within about 3,000 characters:\n\
         - Where it stopped: the last thing done, and the state it left things in.\n\
         - Decisions, each with its reason, including approaches tried and dropped.\n\
         - Files and lines touched or about to be, as path:line.\n\
         - The next step, concrete enough to start on without asking.\n\
         - Open questions for the user.\n\
         Leave out what the board or the code already says. A handoff replaces the task's earlier one, which stays on the task as an ordinary note. Then tell the user it is safe to /clear.\n"
    );
    if let [task] = tasks.as_slice() {
        let _ = writeln!(out, "\nTask {}: {}", task.id, clip(&task.description, NOTE_CLIP));
        let earlier = task
            .uid
            .as_deref()
            .and_then(|uid| reader.graph.links.attached.get(uid))
            .into_iter()
            .flatten()
            .filter_map(|id| all.get(id))
            .find(|note| note.handoff);
        if let Some(note) = earlier {
            let _ = writeln!(out, "Its current handoff, {}, which this one replaces: {}", note.id, clip(&note.description, NOTE_CLIP));
        }
    }
    Ok(out)
}

/// What a board holds, for a search that named nothing to look for: counts
/// by state and by board, in a few lines, instead of every item.
fn summary(all: &ItemMap) -> String {
    let shown: Vec<&Item> = all.values().filter(|item| visible(item)).collect();
    let tasks: Vec<&Item> = shown.iter().copied().filter(|item| item.is_task).collect();
    let notes = shown.len() - tasks.len();
    let count = |n: usize, one: &str, many: &str| if n == 1 { format!("1 {one}") } else { format!("{n} {many}") };

    let states: Vec<String> = ["in progress", "paused", "pending", "done", "cancelled"]
        .iter()
        .filter_map(|word| {
            let n = tasks.iter().filter(|item| state_word(item) == *word).count();
            (n > 0).then(|| format!("{n} {word}"))
        })
        .collect();
    let tasks_part = if states.is_empty() {
        count(tasks.len(), "task", "tasks")
    } else {
        format!("{} ({})", count(tasks.len(), "task", "tasks"), states.join(", "))
    };

    let mut boards: Vec<(&str, usize)> = Vec::new();
    for item in &shown {
        for board in &item.boards {
            match boards.iter_mut().find(|(name, _)| *name == board.as_str()) {
                Some((_, n)) => *n += 1,
                None => boards.push((board.as_str(), 1)),
            }
        }
    }
    boards.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let boards: Vec<String> = boards.iter().map(|(name, n)| format!("{name} {n}")).collect();

    format!(
        "{}: {tasks_part} and {}.\nBoards: {}.\nGive text or filters to list items.\n",
        count(shown.len(), "item", "items"),
        count(notes, "note", "notes"),
        boards.join(" \u{b7} ")
    )
}

/// What moved since a cursor from an earlier read.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Changes {
    pub since: i64,
    /// The cursor to pass next time.
    pub cursor: i64,
    pub items: Vec<Entry>,
    /// Listed tasks a write set free and that are still free to start.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub released: Vec<u32>,
    /// Listed tasks a write left waiting and that still wait.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub blocked: Vec<u32>,
    /// Items a write took out of storage: archived, or expired from the trash.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<Removed>,
    /// Whether the journal still reaches back to `since`.
    pub complete: bool,
}

/// An item gone from storage, as the journal remembers it.
#[derive(Debug, Serialize)]
pub struct Removed {
    pub id: u32,
    pub uid: Option<String>,
    pub text: String,
}

/// A cursor below this is a board revision; one at or above it is a
/// millisecond clock reading, from a prime older than revisions.
const CLOCK_CURSOR: i64 = 100_000_000_000;

/// Every item a write changed after the revision `since`, put away or not,
/// oldest change first. A trashed item still shows, as `trashed`: the trash
/// keeps what it takes. What leaves storage outright -- archived by `--clear`,
/// or expired from the trash -- leaves nothing to list; the revision still
/// moves, and the text says to prime again.
///
/// A cursor from before revisions, a clock reading, is answered the old way,
/// by `updatedAt`, and handed a revision to use from then on.
pub fn changes(ekko: &Ekko, since: i64) -> Result<Changes, EkkoError> {
    let all = ekko.storage.get_shared()?;
    let phases = ekko.storage.get_phases()?;
    let cursor = ekko.storage.get_counters()?.revision as i64;
    let reader = Reader::shared(&all, &phases);
    let moved = |item: &&Item| {
        if since >= CLOCK_CURSOR {
            updated(item) >= since
        } else {
            item.rev.unwrap_or(0) as i64 > since
        }
    };
    let mut items: Vec<(u64, Entry)> =
        all.values().filter(moved).map(|item| (item.rev.unwrap_or(0), reader.entry(item))).collect();
    items.sort_by_key(|(rev, entry)| (*rev, entry.updated_at, entry.id));
    let mut items: Vec<Entry> = items.into_iter().map(|(_, entry)| entry).collect();

    // What the journal saw after the cursor: work set free or left waiting,
    // named only while it still is, and items taken out of storage.
    let journal = ekko.storage.read_journal()?;
    let resolve = |named: &serde_json::Value| {
        named["uid"]
            .as_str()
            .and_then(|uid| reader.uid(uid))
            .or_else(|| named["id"].as_u64().map(|id| id as u32).filter(|id| all.get(id).is_some_and(|item| item.uid.is_none())))
    };
    let after = |entry: &serde_json::Value| {
        if since >= CLOCK_CURSOR {
            entry["at"].as_i64().is_some_and(|at| at >= since)
        } else {
            entry["rev"].as_i64().is_some_and(|rev| rev > since)
        }
    };
    let (mut released, mut blocked, mut removed) = (BTreeSet::new(), BTreeSet::new(), Vec::new());
    for entry in journal.iter().filter(|entry| after(entry)) {
        for id in entry["released"].as_array().into_iter().flatten().filter_map(&resolve) {
            blocked.remove(&id);
            released.insert(id);
        }
        for id in entry["blocked"].as_array().into_iter().flatten().filter_map(&resolve) {
            released.remove(&id);
            blocked.insert(id);
        }
        for gone in entry["removed"].as_array().into_iter().flatten() {
            removed.push(Removed {
                id: gone["id"].as_u64().unwrap_or_default() as u32,
                uid: gone["uid"].as_str().map(str::to_string),
                text: gone["text"].as_str().unwrap_or_default().to_string(),
            });
        }
    }
    released.retain(|id| all.get(id).is_some_and(|item| reader.ready(item)));
    blocked.retain(|id| all.get(id).is_some_and(|item| visible(item) && holds(item)) && !reader.graph.open_blockers(*id).is_empty());
    for id in released.iter().chain(&blocked) {
        if !items.iter().any(|entry| entry.id == *id) {
            items.push(reader.entry(&all[id]));
        }
    }
    let from = journal.iter().find_map(|entry| entry["from"].as_i64());
    let complete = from.is_none_or(|from| since < CLOCK_CURSOR && since + 1 >= from);

    Ok(Changes {
        since,
        cursor,
        items,
        released: released.into_iter().collect(),
        blocked: blocked.into_iter().collect(),
        removed,
        complete,
    })
}

/// How a Claude Code session began, read off the SessionStart hook's input.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionEvent {
    pub source: String,
    pub session_id: Option<String>,
}

impl SessionEvent {
    /// Input that does not read as an event reads as a session starting: the
    /// prime is always a safe answer, and one line saying nothing moved is not.
    pub fn from_hook_input(input: &str) -> Self {
        let event: serde_json::Value = serde_json::from_str(input).unwrap_or_default();
        SessionEvent {
            source: event["source"].as_str().unwrap_or("startup").to_string(),
            session_id: event["session_id"].as_str().map(str::to_string),
        }
    }
}

/// Where the hook keeps the cursor it served each session: the XDG state
/// directory, outside every board, so that reading a board still writes
/// nothing to it.
pub fn session_state_dir(home: &Path) -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local").join("state"))
        .join("ekko")
        .join("sessions")
}

/// What the SessionStart hook puts in context, by how the session began.
///
/// A session that starts, clears or compacts holds no board, and gets the
/// prime. One that resumes or forks already holds a prime in its transcript,
/// and a second one is paid for again on every later call: it gets one line
/// when nothing moved since the cursor this hook last served it, what moved
/// when little did, and the prime only when the list would be longer. A
/// session this hook never served is pointed at the prime it already holds.
pub fn session_start(ekko: &Ekko, board: &str, event: &SessionEvent, state: &Path) -> Result<String, EkkoError> {
    let revision = ekko.storage.get_counters()?.revision as i64;
    let served = event.session_id.as_deref().and_then(|session| served_cursor(state, session, board));
    let text = match (event.source.as_str(), served) {
        ("resume" | "fork", Some(cursor)) if cursor == revision => {
            format!("ekko \u{b7} {board} \u{b7} cursor {revision} \u{b7} nothing moved since this session last read the board\n")
        }
        ("resume" | "fork", Some(cursor)) => {
            let moved = changes(ekko, cursor)?.text();
            if moved.chars().count() <= RESUME_CHANGES {
                format!("ekko \u{b7} {board} \u{b7} what moved since this session last read the board\n{moved}")
            } else {
                prime(ekko, board)?.text()
            }
        }
        ("resume" | "fork", None) => format!(
            "ekko \u{b7} {board} \u{b7} cursor {revision} \u{b7} the prime earlier in this session still applies; changes with its cursor lists what moved\n"
        ),
        _ => prime(ekko, board)?.text(),
    };
    if let Some(session) = &event.session_id {
        remember(state, session, board, revision);
    }
    Ok(text)
}

/// A session id as a file name: its letters, digits, dashes and underscores.
fn session_file(state: &Path, session: &str) -> Option<PathBuf> {
    let safe: String = session.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')).take(128).collect();
    (!safe.is_empty()).then(|| state.join(format!("{safe}.json")))
}

fn served_cursor(state: &Path, session: &str, board: &str) -> Option<i64> {
    let served: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(session_file(state, session)?).ok()?).ok()?;
    if served["board"].as_str()? != board {
        return None;
    }
    served["cursor"].as_i64()
}

/// Records the cursor served to a session, and forgets sessions older than
/// `SESSION_KEPT` on the way. Best effort: the hook answers even when the
/// state directory cannot be written.
fn remember(state: &Path, session: &str, board: &str, cursor: i64) {
    let Some(file) = session_file(state, session) else { return };
    if std::fs::create_dir_all(state).is_err() {
        return;
    }
    if let Ok(entries) = std::fs::read_dir(state) {
        for entry in entries.flatten() {
            let stale = entry
                .metadata()
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|modified| modified.elapsed().ok())
                .is_some_and(|age| age > SESSION_KEPT);
            if stale {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    let _ = std::fs::write(file, serde_json::json!({"board": board, "cursor": cursor}).to_string());
}

impl Changes {
    pub fn text(&self) -> String {
        let changed = self.items.len() + self.removed.len();
        let mut out = format!("cursor {} \u{b7} {changed} changed since {}\n", self.cursor, self.since);
        for entry in &self.items {
            let away = entry.away.map(|away| format!(", {away}")).unwrap_or_default();
            let now = if self.released.contains(&entry.id) {
                " \u{b7} ready now"
            } else if self.blocked.contains(&entry.id) {
                " \u{b7} blocked now"
            } else {
                ""
            };
            let _ = writeln!(out, "{:>4}. [{}{away}] {}{now}", entry.id, entry.state, clip(&entry.description, TASK_CLIP));
        }
        for gone in &self.removed {
            let _ = writeln!(out, "{:>4}. [removed] {}", gone.id, gone.text);
        }
        // Said only when it is so: the journal no longer reaches the cursor.
        if !self.complete {
            out.push_str("The journal no longer reaches this cursor, so some of what moved is not listed; prime again for the whole board.\n");
        }
        out
    }
}

// ---- text -------------------------------------------------------------

/// One listing line: the id first, so a reader can find the item by its
/// number, then the description and whatever sets it apart.
fn entry_line(entry: &Entry, today: &str) -> String {
    listed_line(entry, &clip(&entry.description, TASK_CLIP), false, today)
}

/// A listing line with `body` for its text, then whatever sets the item
/// apart. `stated` says the body already carries the item's state.
fn listed_line(entry: &Entry, body: &str, stated: bool, today: &str) -> String {
    let mut line = format!("{:>4}. {}", entry.id, body);
    let mut meta = Vec::new();
    if !stated && matches!(entry.state, "paused" | "in progress") {
        meta.push(entry.state.to_string());
    }
    if let Some(priority @ 2..) = entry.priority {
        meta.push(format!("p{priority}"));
    }
    if let Some(priority) = entry.inherits {
        meta.push(format!("inherits p{priority}"));
    }
    if let Some(day) = &entry.finish_by {
        meta.push(format!("finish by {day}"));
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

/// "1 open task", "3 open tasks".
fn open_tasks(n: usize) -> String {
    if n == 1 { "1 open task".to_string() } else { format!("{n} open tasks") }
}

impl Prime {
    pub fn text(&self) -> String {
        self.text_within(PRIME_BUDGET)
    }

    /// The resume view in at most `budget` characters. The header, what needs
    /// attention and the closing line always fit; the sections fill what is
    /// left in the order an agent needs them -- work in progress, ready work
    /// best first, blocked work, loose notes -- and a section cut short says
    /// how much it left out and where the rest is.
    pub fn text_within(&self, budget: usize) -> String {
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
            let _ = writeln!(out, "{}", roadmap_line(&self.roadmap, self.rootless));
        }

        let attention = self.attention();
        let close = "\nAn item in full, with its dependencies and notes: context <id>.\n";
        let fixed = out.chars().count() + attention.chars().count() + close.chars().count() + 4 * MORE_LINE;
        let mut room = Room(budget.saturating_sub(fixed));
        if let Some(handoff) = &self.handoff {
            out.push_str(&handoff.text());
        }

        let sections = [
            Section::of_entries("In progress", &self.doing, usize::MAX, self.doing.len(), "context <id> reads one", usize::MAX, &today),
            Section::of_entries("Ready, best first", &self.ready, READY_WITH_NOTES, self.ready.len(), "next lists them", READY_KEPT, &today),
            Section::of_entries("Blocked", &self.blocked, 0, self.blocked_total, "search with the blocked filter", BLOCKED_KEPT, &today),
            Section {
                heading: "\nRecent notes, not attached to a task".to_string(),
                blocks: self
                    .recent_notes
                    .iter()
                    .map(|note| (note_line(note, NOTE_CLIP).trim_start_matches("    ").to_string(), Vec::new()))
                    .collect(),
                total: self.recent_notes.len(),
                rest: None,
                kept: NOTES_KEPT,
            },
        ];
        let shown = fit(&sections, &mut room);
        for (section, shown) in sections.iter().zip(&shown) {
            section.write(&mut out, shown);
        }

        out.push_str(&attention);
        out.push_str(close);
        out
    }

    /// What the board holds against itself: a block the budget always keeps.
    fn attention(&self) -> String {
        let mut out = String::new();

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
        if !self.freed_by_cancelling.is_empty() {
            let pairs: Vec<String> =
                self.freed_by_cancelling.iter().map(|(task, blocker)| format!("{task} \u{21e0} {blocker}")).collect();
            attention.push(format!("ready only because what it waited on was cancelled: {}", pairs.join("; ")));
        }
        if !attention.is_empty() {
            let _ = writeln!(out, "\nNeeds attention");
            for line in attention {
                let _ = writeln!(out, "  ! {line}");
            }
        }
        out
    }
}

/// The phases in order with how far each has got and where work sits, and
/// how much lies outside every phase: the prime's roadmap line.
fn roadmap_line(steps: &[RoadmapStep], rootless: u32) -> String {
    let steps: Vec<String> = steps
        .iter()
        .map(|step| {
            let here = if step.current { " (in progress)" } else { "" };
            format!("{} {}/{}{here}", step.name, step.complete, step.total)
        })
        .collect();
    let root = if rootless > 0 { format!(" \u{b7} {rootless} at the root") } else { String::new() };
    format!("roadmap: {}{root}", steps.join(" \u{2192} "))
}

/// The roadmap as an agent reads it: the prime's line, and the dependencies
/// that run against the phase order -- not the drawing made for a terminal,
/// and no step that is the user's to take.
pub fn roadmap_text(outcome: &Outcome) -> String {
    let Outcome::Roadmap { steps, rootless, inversions } = outcome else { return String::new() };
    if steps.is_empty() {
        return "No phases are declared on this board; the phase order is the user's to set.\n".to_string();
    }
    let mut out = format!("{}\n", roadmap_line(steps, *rootless));
    if !inversions.is_empty() {
        let pairs: Vec<String> = inversions
            .iter()
            .map(|i| format!("{} ({}) \u{21e0} {} ({})", i.blocked, i.blocked_phase, i.blocker, i.blocker_phase))
            .collect();
        let _ = writeln!(out, "dependencies against the phase order: {}", pairs.join("; "));
    }
    out
}

/// The projects as an agent reads them, one per line. None is said as a fact:
/// making a project is the user's step, in its folder.
pub fn projects_text(projects: &[ProjectSummary]) -> String {
    if projects.is_empty() {
        return "No projects yet. A project is made by the user, in its folder.\n".to_string();
    }
    let mut out = String::new();
    for project in projects {
        let place = match (project.status, project.path.as_deref()) {
            ("missing", Some(path)) => format!("{path}, folder missing"),
            (_, Some(path)) => path.to_string(),
            (status, None) => status.to_string(),
        };
        let _ = writeln!(
            out,
            "{} \u{b7} {}/{} tasks done \u{b7} {} \u{b7} {place}",
            project.name,
            project.complete,
            project.tasks,
            if project.notes == 1 { "1 note".to_string() } else { format!("{} notes", project.notes) }
        );
    }
    out
}

impl Handoff {
    /// The handoff as its own prime section: which task it hands over and
    /// when it was written, then its text line by line, each quoted so a line
    /// such as "1. run the tests" is never read as an item id. Within
    /// `HANDOFF_BUDGET` characters, prefixes included; a longer handoff says
    /// how much it left and where the rest is.
    fn text(&self) -> String {
        let written = chrono::DateTime::from_timestamp_millis(self.updated_at)
            .map(|at| at.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_default();
        let mut out = format!(
            "\nWhere the last session stopped: handoff {} on task {} [{}], written {written}\n",
            self.id, self.task, self.task_state
        );
        let body = self.description.trim();
        let total = body.chars().count();
        let mut spent = 0;
        let mut shown = 0;
        for line in body.lines() {
            let quoted = format!("    > {line}");
            let cost = quoted.chars().count() + 1;
            if spent + cost > HANDOFF_BUDGET {
                let room = HANDOFF_BUDGET.saturating_sub(spent + 8);
                if room > 40 {
                    let head: String = line.chars().take(room).collect();
                    let _ = writeln!(out, "    > {head}");
                    shown += head.chars().count();
                }
                break;
            }
            let _ = writeln!(out, "{quoted}");
            spent += cost;
            shown += line.chars().count() + 1;
        }
        if shown < total {
            let _ = writeln!(out, "    \u{2026} +{} chars: context {} reads it whole", total - shown.min(total), self.id);
        }
        out
    }
}

/// The characters a budgeted view has left.
struct Room(usize);

/// One prime section before it is fitted: its heading, each entry as its own
/// line and the notes quoted under it, how many there are in all, where the
/// rest is, and how many entries the first pass of `fit` keeps.
struct Section {
    heading: String,
    blocks: Vec<(String, Vec<String>)>,
    total: usize,
    rest: Option<&'static str>,
    kept: usize,
}

impl Section {
    /// A section of entries best first, quoting the notes of the first
    /// `with_notes`, titled with `total`.
    fn of_entries(
        title: &str,
        entries: &[Entry],
        with_notes: usize,
        total: usize,
        rest: &'static str,
        kept: usize,
        today: &str,
    ) -> Self {
        let blocks = entries
            .iter()
            .enumerate()
            .map(|(at, entry)| {
                let notes = if at < with_notes {
                    entry.notes.iter().map(|note| note_line(note, NOTE_CLIP)).collect()
                } else {
                    Vec::new()
                };
                (entry_line(entry, today), notes)
            })
            .collect();
        Section { heading: format!("\n{title} ({total})"), blocks, total, rest: Some(rest), kept }
    }

    /// The heading, the entries `shown` admits -- with their notes where it
    /// says so -- and a line counting what was left out and saying where it is.
    fn write(&self, out: &mut String, shown: &[bool]) {
        if shown.is_empty() {
            return;
        }
        let _ = writeln!(out, "{}", self.heading);
        for ((line, notes), with_notes) in self.blocks.iter().zip(shown) {
            let _ = writeln!(out, "{line}");
            if *with_notes {
                for note in notes {
                    let _ = writeln!(out, "{note}");
                }
            }
        }
        if let Some(rest) = self.rest.filter(|_| shown.len() < self.total) {
            let _ = writeln!(out, "      +{} more: {rest}", self.total - shown.len());
        }
    }
}

/// Which entries of each section fit in `room`, and whether each goes with
/// its notes. Two passes, both in the order the sections are given: the first
/// admits up to each section's `kept`, the second spends what is left, so a
/// long ready list cannot crowd out the blocked work and the loose notes a
/// resume also needs. An entry goes with its notes when they fit with it and
/// alone when only it does. When everything fits, everything shows, in order.
fn fit(sections: &[Section], room: &mut Room) -> Vec<Vec<bool>> {
    let cost = |line: &str| line.chars().count() + 1;
    let mut shown: Vec<Vec<bool>> = vec![Vec::new(); sections.len()];
    for first in [true, false] {
        for (section, shown) in sections.iter().zip(shown.iter_mut()) {
            let cap = if first { section.kept } else { usize::MAX };
            while shown.len() < section.blocks.len().min(cap) {
                let (line, notes) = &section.blocks[shown.len()];
                let heading = if shown.is_empty() { cost(&section.heading) } else { 0 };
                let alone = heading + cost(line);
                let together = alone + notes.iter().map(|note| cost(note)).sum::<usize>();
                if !notes.is_empty() && together <= room.0 {
                    room.0 -= together;
                    shown.push(true);
                } else if alone <= room.0 {
                    room.0 -= alone;
                    shown.push(false);
                } else {
                    break;
                }
            }
        }
    }
    shown
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

impl Found {
    /// Hits one per line, each with its state and the text that matched, then
    /// a total when more matched than are shown -- or the summary.
    pub fn text(&self) -> String {
        if let Some(summary) = &self.summary {
            return summary.clone();
        }
        if self.hits.is_empty() {
            return "Nothing matches.\n".to_string();
        }
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let mut out = String::new();
        if !self.every_word {
            out.push_str("No item has every word; these have some.\n");
        }
        for (entry, body) in &self.hits {
            let _ = writeln!(out, "{}", listed_line(entry, &format!("[{}] {body}", entry.state), true, &today));
        }
        if self.total > self.hits.len() {
            let _ = writeln!(out, "{} of {} shown: narrow the text or filters, or raise limit.", self.hits.len(), self.total);
        }
        out
    }
}

impl Context {
    /// Notes in full: what a person reading one item at the terminal wants.
    pub fn text(&self) -> String {
        self.text_with(Detail::Full)
    }

    pub fn text_with(&self, detail: Detail) -> String {
        let item = &self.item;
        let mut out = String::new();
        let _ = writeln!(out, "{:>4}. {}", item.id, item.description);

        let mut facts = vec![match (item.state, item.handoff) {
            ("note", true) => "note, the handoff of the task it is attached to".to_string(),
            ("note", false) => "note".to_string(),
            (state, _) => format!("task, {state}"),
        }];
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
        if self.waits_on > 0 {
            let _ = writeln!(out, "      waits on {}, directly or through others", open_tasks(self.waits_on));
        }
        if !self.roots.is_empty() {
            let _ = writeln!(out, "\nRoots, free to start");
            for root in &self.roots {
                if self.blockers.iter().any(|blocker| blocker.id == root.id) {
                    let _ = writeln!(out, "{:>4}. [{}] listed above", root.id, root.state);
                } else {
                    let _ = writeln!(out, "{:>4}. [{}] {}", root.id, root.state, root.description);
                }
            }
        }
        links(&mut out, "Blocks", &self.dependents);
        if item.unblocks > 0 {
            let verb = if item.unblocks == 1 { "waits" } else { "wait" };
            let _ = writeln!(out, "      {} {verb} on this, directly or through others", open_tasks(item.unblocks));
        }
        if !item.notes.is_empty() {
            let _ = writeln!(out, "\nNotes attached");
            for note in &item.notes {
                let body = match detail {
                    Detail::Full => note.description.clone(),
                    Detail::Concise => clip(&note.description, NOTE_CLIP),
                };
                let _ = writeln!(out, "{:>4}. {body}", note.id);
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

    /// Work under way first, then urgency: an overdue p3 (12 + 6), work that
    /// others wait on (8), a p3 alone (6), then the rest. Blocked work is not
    /// a candidate at all.
    #[test]
    fn next_orders_by_progress_then_urgency() {
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
        assert_eq!(ids(&order), vec![7, 3, 4, 2, 1]);
        assert_eq!(order[2].unblocks, 2, "4 lets 5 move, and 6 after it");
        assert_eq!(ids(&next(&ekko, Some(2)).unwrap()), vec![7, 3]);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A low-priority prerequisite of urgent work inherits that urgency, and a
    /// date two years out does not outrank work others wait on -- the order
    /// the lexicographic ranking got backwards (3, 4, 5, 1).
    #[test]
    fn next_ranks_a_prerequisite_by_the_urgency_it_inherits() {
        let (ekko, dir) = board("inherits");
        let tomorrow = (chrono::Local::now() + chrono::TimeDelta::days(1)).format("d:%Y-%m-%d").to_string();
        ekko.create_task(&words(&["prerequisite of the urgent task"])).unwrap();
        ekko.create_task(&words(&["urgent", "p:3", tomorrow.as_str()])).unwrap();
        ekko.create_task(&words(&["unrelated", "p:2"])).unwrap();
        ekko.create_task(&words(&["far off", "d:2028-12-31"])).unwrap();
        ekko.create_task(&words(&["unblocks five"])).unwrap();
        for k in 6..=10 {
            let name = format!("waits {k}");
            ekko.create_task(&words(&[name.as_str()])).unwrap();
            let target = format!("@{k}");
            ekko.set_blocked_by(&words(&[target.as_str(), "5"])).unwrap();
        }
        ekko.set_blocked_by(&words(&["@2", "1"])).unwrap();

        let order = next(&ekko, None).unwrap();
        assert_eq!(ids(&order), vec![1, 5, 3, 4]);
        assert_eq!(order[0].inherits, Some(3));
        assert!(order[0].finish_by.is_some(), "{:?}", order[0]);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// What each task inherits, read straight off the graph.
    fn inherited_on(ekko: &Ekko) -> HashMap<u32, Inherited> {
        let all = ekko.storage.get().unwrap();
        Graph::new(&all, Arc::new(Links::build(&all))).inherited()
    }

    /// A diamond -- 1 held up by 2 and 3, both held up by 4 -- settles 4
    /// once, with the highest priority and the earliest finish of both paths.
    #[test]
    fn inherited_on_a_diamond_counts_each_path_once() {
        let (ekko, dir) = board("diamond");
        ekko.create_task(&words(&["top", "p:3", "d:2030-01-10"])).unwrap();
        ekko.create_task(&words(&["left"])).unwrap();
        ekko.create_task(&words(&["right", "p:2", "d:2030-01-05"])).unwrap();
        ekko.create_task(&words(&["bottom"])).unwrap();
        ekko.set_blocked_by(&words(&["@1", "2", "3"])).unwrap();
        ekko.set_blocked_by(&words(&["@2", "4"])).unwrap();
        ekko.set_blocked_by(&words(&["@3", "4"])).unwrap();

        let values = inherited_on(&ekko);
        let bottom = values[&4];
        assert_eq!(bottom.priority, 3, "the top's p3 reaches it through either side");
        // Through the left: day before the top's date, then a day before that.
        // Through the right: its own date is earlier, one day before it.
        let day = |date: &str| chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap().num_days_from_ce();
        assert_eq!(bottom.finish_by, Some(day("2030-01-04")));
        assert!(bottom.waited_on);
        assert!(!values[&1].waited_on, "nothing waits on the top");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Down a chain each step finishes a day before the one it holds up.
    #[test]
    fn inherited_down_a_chain_takes_a_day_per_step() {
        let (ekko, dir) = board("chain");
        ekko.create_task(&words(&["end", "p:2", "d:2030-03-10"])).unwrap();
        ekko.create_task(&words(&["middle"])).unwrap();
        ekko.create_task(&words(&["start"])).unwrap();
        ekko.set_blocked_by(&words(&["@1", "2"])).unwrap();
        ekko.set_blocked_by(&words(&["@2", "3"])).unwrap();

        let values = inherited_on(&ekko);
        let day = |date: &str| chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap().num_days_from_ce();
        assert_eq!(values[&2].finish_by, Some(day("2030-03-09")));
        assert_eq!(values[&3].finish_by, Some(day("2030-03-08")));
        assert_eq!(values[&3].priority, 2);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Cancelled work holds nothing up, so its prerequisite inherits nothing
    /// from it: neither its priority, its date, nor the weight of being waited on.
    #[test]
    fn inherited_ignores_a_cancelled_dependent() {
        let (ekko, dir) = board("cancelled");
        ekko.create_task(&words(&["prerequisite"])).unwrap();
        ekko.create_task(&words(&["dropped", "p:3", "d:2030-01-01"])).unwrap();
        ekko.set_blocked_by(&words(&["@2", "1"])).unwrap();
        ekko.set_state(&words(&["@2", "cancelled"]), false).unwrap();

        let values = inherited_on(&ekko);
        assert_eq!(values[&1], Inherited { priority: 1, finish_by: None, waited_on: false });
        assert!(!values.contains_key(&2), "a cancelled task is not open work");

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

        assert_eq!(view.cursor, ekko.storage.get_counters().unwrap().revision as i64);

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

    /// What a write did that no item's fields show still reaches a cursor:
    /// work a completion set free is named ready now, and an item cleared out
    /// of storage is named removed, with the text it had.
    #[test]
    fn changes_names_work_set_free_and_items_taken_out() {
        let (ekko, dir) = board("journal");
        ekko.create_task(&words(&["blocker"])).unwrap();
        ekko.create_task(&words(&["waits"])).unwrap();
        ekko.create_task(&words(&["done and cleared"])).unwrap();
        ekko.set_blocked_by(&words(&["@2", "1"])).unwrap();
        ekko.set_state(&words(&["@3", "done"]), false).unwrap();
        let cursor = prime(&ekko, "default board").unwrap().cursor;

        ekko.set_state(&words(&["@1", "done"]), false).unwrap();
        ekko.clear().unwrap();

        let seen = changes(&ekko, cursor).unwrap();
        let text = seen.text();
        assert!(text.contains("   2. [pending] waits \u{b7} ready now"), "{text}");
        assert!(text.contains("   3. [removed] done and cleared"), "{text}");
        assert!(seen.complete && !text.contains("journal"), "{text}");
        assert!(changes(&ekko, seen.cursor).unwrap().text().starts_with(&format!("cursor {} \u{b7} 0 changed", seen.cursor)));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Work a cancelled blocker let go is named: it is ready, though what it
    /// waited on will never happen.
    #[test]
    fn prime_names_work_a_cancelled_blocker_let_go() {
        let (ekko, dir) = board("cancelled");
        ekko.create_task(&words(&["prerequisite"])).unwrap();
        ekko.create_task(&words(&["needed it"])).unwrap();
        ekko.set_blocked_by(&words(&["@2", "1"])).unwrap();
        ekko.set_state(&words(&["@1", "cancelled"]), false).unwrap();

        let text = prime(&ekko, "default board").unwrap().text();
        assert!(text.contains("ready only because what it waited on was cancelled: 2 \u{21e0} 1"), "{text}");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The agent's roadmap and project list state facts and name no command:
    /// a step that is the user's to take is said as the user's.
    #[test]
    fn roadmap_and_projects_read_as_facts_not_commands() {
        let none = roadmap_text(&Outcome::Roadmap { steps: vec![], rootless: 3, inversions: vec![] });
        assert!(!none.contains("ekko") && none.contains("user"), "{none}");
        let steps = vec![
            RoadmapStep { name: "design".to_string(), complete: 1, total: 2, notes: 0, current: true },
            RoadmapStep { name: "build".to_string(), complete: 0, total: 3, notes: 1, current: false },
        ];
        let line = roadmap_text(&Outcome::Roadmap { steps, rootless: 4, inversions: vec![] });
        assert_eq!(line, "roadmap: design 1/2 (in progress) \u{2192} build 0/3 \u{b7} 4 at the root\n");
        let empty = projects_text(&[]);
        assert!(!empty.contains("ekko init") && empty.contains("user"), "{empty}");
    }

    /// A concise read clips the notes around an item the way prime quotes
    /// them, and a full one prints them whole; the item's own text is whole
    /// either way.
    #[test]
    fn context_clips_attached_notes_unless_asked_for_all_of_them() {
        let (ekko, dir) = board("detail");
        let long = "a note long enough to be worth clipping ".repeat(20);
        ekko.create_task(&words(&["the task"])).unwrap();
        ekko.create_note(&words(&[long.as_str()])).unwrap();
        ekko.set_attached_to(&words(&["@2", "1"])).unwrap();

        let read = context(&ekko, "1").unwrap();
        let concise = read.text_with(Detail::Concise);
        let full = read.text_with(Detail::Full);
        assert!(concise.contains("\u{2026} (+") && concise.chars().count() < full.chars().count(), "{concise}");
        assert!(full.contains(long.trim()), "{full}");
        assert!(concise.starts_with("   1. the task\n"), "{concise}");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// However much is ready, the prime fits the hook's cap: sections fill in
    /// the order an agent needs them, and each cut says what it left out.
    #[test]
    fn prime_stays_within_its_budget_and_says_what_it_left_out() {
        let (ekko, dir) = board("budget");
        let reason = "a reason that runs long enough to matter ".repeat(7);
        for k in 1..=80 {
            let task = format!("ready task number {k}, described at the length real tasks on a board run to, so a listing fills");
            ekko.create_task(&words(&[task.as_str()])).unwrap();
        }
        for k in 1..=80 {
            ekko.create_note(&words(&[reason.as_str()])).unwrap();
            ekko.set_attached_to(&words(&[format!("@{}", 80 + k).as_str(), k.to_string().as_str()])).unwrap();
        }

        let text = prime(&ekko, "default board").unwrap().text();
        assert!(text.chars().count() <= PRIME_BUDGET, "{} characters:\n{text}", text.chars().count());
        assert!(text.contains("Ready, best first (80)") && text.contains("more: next lists them"), "{text}");
        assert!(text.ends_with("context <id>.\n"), "{text}");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A long ready list no longer crowds out the rest of a resume: the cut
    /// keeps the ten best ready tasks, blocked work with what holds it, and
    /// the newest loose notes, then spends what is left on more ready work.
    #[test]
    fn a_cut_prime_keeps_blocked_work_and_notes_beside_ready_work() {
        let (ekko, dir) = board("kept");
        let long = "described at the length real tasks on a board run to, so that a listing fills and then some";
        for k in 1..=120 {
            let task = format!("ready task {k}, {long}");
            ekko.create_task(&words(&[task.as_str()])).unwrap();
        }
        for k in 121..=126 {
            let task = format!("blocked task {k}, {long}");
            ekko.create_task(&words(&[task.as_str()])).unwrap();
            ekko.set_blocked_by(&words(&[format!("@{k}").as_str(), "1"])).unwrap();
        }
        for k in 1..=4 {
            let note = format!("loose note {k}");
            ekko.create_note(&words(&[note.as_str()])).unwrap();
        }

        let text = prime(&ekko, "default board").unwrap().text();
        assert!(text.chars().count() <= PRIME_BUDGET, "{} characters:\n{text}", text.chars().count());
        let section = |title: &str| -> Vec<u32> {
            text.split(title).nth(1).map(|rest| line_ids(rest.split("\n\n").next().unwrap_or_default())).unwrap_or_default()
        };
        let ready = section("\nReady, best first");
        assert!(ready.len() > READY_KEPT, "the room left goes to more ready work: {ready:?}");
        assert_eq!(ready[0], 1, "the task others wait on leads");
        assert_eq!(section("\nBlocked (6)").len(), BLOCKED_KEPT, "{text}");
        assert!(text.contains("+1 more: search with the blocked filter"), "{text}");
        assert!(section("\nRecent notes").len() >= NOTES_KEPT, "{text}");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Writes a handoff through the same structured write an agent uses.
    fn hand_over(ekko: &Ekko, task: u32, text: &str) {
        let mut draft = crate::ops::Draft::open(ekko).unwrap();
        let op: crate::ops::Op =
            serde_json::from_value(serde_json::json!({"op": "create", "kind": "handoff", "text": text, "attached_to": task}))
                .unwrap();
        draft.apply(&op).unwrap();
        draft.commit(false).unwrap();
    }

    /// The newest handoff on open work gets a section of its own, line by
    /// line and quoted, and is not repeated among its task's notes; a done
    /// task's handoff is history and does not show.
    #[test]
    fn prime_shows_the_newest_handoff_on_open_work_in_its_own_section() {
        let (ekko, dir) = board("handoff-prime");
        ekko.create_task(&words(&["the work in progress"])).unwrap();
        ekko.create_task(&words(&["finished work"])).unwrap();
        ekko.set_state(&words(&["@1", "progress"]), false).unwrap();
        hand_over(&ekko, 2, "old news");
        ekko.set_state(&words(&["@2", "done"]), false).unwrap();
        hand_over(&ekko, 1, "Stopped after the parser.\n1. run the tests\n2. wire the flag");

        let text = prime(&ekko, "default board").unwrap().text();
        assert!(text.contains("Where the last session stopped: handoff 4 on task 1 [in progress]"), "{text}");
        assert!(text.contains("    > 1. run the tests\n    > 2. wire the flag\n"), "{text}");
        assert!(!text.contains("old news"), "{text}");
        assert!(!line_ids(&text).contains(&4), "the handoff is not listed again as a note: {text}");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A long handoff shows its start within its own budget, and the prime as
    /// a whole stays under the 10,000 characters a hook keeps.
    #[test]
    fn a_long_handoff_is_cut_and_the_prime_fits_the_hook() {
        let (ekko, dir) = board("handoff-long");
        let reason = "a reason that runs long enough to matter ".repeat(7);
        for k in 1..=80 {
            let task = format!("ready task number {k}, described at the length real tasks on a board run to, so a listing fills");
            ekko.create_task(&words(&[task.as_str()])).unwrap();
        }
        for k in 1..=20 {
            ekko.create_note(&words(&[reason.as_str()])).unwrap();
            ekko.set_attached_to(&words(&[format!("@{}", 80 + k).as_str(), k.to_string().as_str()])).unwrap();
        }
        ekko.set_state(&words(&["@1", "progress"]), false).unwrap();
        let long: String = (1..=200).map(|k| format!("step {k}: something this session found out and the next must know\n")).collect();
        hand_over(&ekko, 1, &long);

        let text = prime(&ekko, "default board").unwrap().text();
        assert!(text.chars().count() < 10_000, "{} characters", text.chars().count());
        assert!(text.contains("chars: context 101 reads it whole"), "{text}");
        assert!(text.contains("Ready, best first"), "{text}");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The prompt names the task in progress and the handoff it replaces, so
    /// the agent writes without reading first.
    #[test]
    fn the_handoff_prompt_names_the_task_and_the_handoff_it_replaces() {
        let (ekko, dir) = board("handoff-prompt");
        ekko.create_task(&words(&["port the parser"])).unwrap();
        ekko.set_state(&words(&["@1", "progress"]), false).unwrap();
        hand_over(&ekko, 1, "halfway through the lexer");

        let text = handoff_prompt(&ekko, None).unwrap();
        let uid = ekko.storage.get().unwrap()[&1].uid.clone().unwrap();
        assert!(text.contains("Write the handoff for task 1 now") && text.contains(&uid), "{text}");
        assert!(text.contains("Its current handoff, 2, which this one replaces: halfway through the lexer"), "{text}");
        assert!(matches!(handoff_prompt(&ekko, Some("2")), Err(EkkoError::InvalidInput(_))), "a note has nothing to hand over");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A session that resumes already holds a prime: it hears one line while
    /// nothing moved since the cursor this hook served it, only what moved
    /// once something did, and a session that starts or clears gets the prime.
    #[test]
    fn session_start_gives_a_resumed_session_only_what_moved() {
        let (ekko, dir) = board("session");
        let state = dir.join("state");
        ekko.create_task(&words(&["first"])).unwrap();
        let event = |source: &str| SessionEvent { source: source.to_string(), session_id: Some("abc-123".to_string()) };

        let started = session_start(&ekko, "default board", &event("startup"), &state).unwrap();
        assert!(started.contains("Ready, best first (1)"), "{started}");

        let resumed = session_start(&ekko, "default board", &event("resume"), &state).unwrap();
        assert!(resumed.lines().count() == 1 && resumed.contains("nothing moved"), "{resumed}");

        ekko.create_task(&words(&["second"])).unwrap();
        let moved = session_start(&ekko, "default board", &event("resume"), &state).unwrap();
        assert!(moved.contains("   2. [pending] second"), "{moved}");

        let stranger = SessionEvent { source: "fork".to_string(), session_id: Some("never-served".to_string()) };
        assert_eq!(session_start(&ekko, "default board", &stranger, &state).unwrap().lines().count(), 1);

        let cleared = session_start(&ekko, "default board", &event("clear"), &state).unwrap();
        assert!(cleared.contains("Ready, best first (2)"), "{cleared}");

        assert_eq!(SessionEvent::from_hook_input("not json").source, "startup");
        assert_eq!(
            SessionEvent::from_hook_input(r#"{"source":"resume","session_id":"s1"}"#),
            SessionEvent { source: "resume".to_string(), session_id: Some("s1".to_string()) }
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A search is ranked, bounded and says what it found: whole words in any
    /// order and never inside another word, each hit's state on its line with
    /// the part of its text that matched, a total when more matched than are
    /// shown, and counts instead of the whole board when nothing is asked.
    #[test]
    fn search_ranks_bounds_and_shows_state_and_the_match() {
        let (ekko, dir) = board("search");
        ekko.create_task(&words(&["recycled ids break caches"])).unwrap();
        ekko.create_task(&words(&["the cycle check walks every path"])).unwrap();
        let padded = format!("{}the cycle keyword sits at the end", "padding ".repeat(40));
        ekko.create_note(&words(&[padded.as_str()])).unwrap();
        ekko.set_state(&words(&["@2", "done"]), false).unwrap();

        let found = search(&ekko, Some("CYCLE"), &[], SEARCH_LIMIT).unwrap();
        assert_eq!((found.total, found.every_word), (2, true));
        let text = found.text();
        assert!(text.contains("   2. [done] the cycle check walks every path\n"), "{text}");
        assert!(text.contains("   3. [note] \u{2026}") && text.contains("cycle keyword sits at the end"), "{text}");
        assert!(!text.contains("recycled"), "{text}");

        let bounded = search(&ekko, Some("cycle"), &[], 1).unwrap().text();
        assert!(bounded.contains("1 of 2 shown"), "{bounded}");

        let summary = search(&ekko, None, &[], SEARCH_LIMIT).unwrap().text();
        assert!(summary.starts_with("3 items: 2 tasks (1 pending, 1 done) and 1 note.\n"), "{summary}");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Several items come from one read, in the order asked and each once,
    /// and one id that names nothing refuses the call rather than being
    /// dropped from it.
    #[test]
    fn contexts_reads_several_items_at_once() {
        let (ekko, dir) = board("contexts");
        for name in ["first", "second", "third"] {
            ekko.create_task(&words(&[name])).unwrap();
        }
        let read = contexts(&ekko, &words(&["3", "1", "@3"])).unwrap();
        assert_eq!(read.iter().map(|context| context.item.id).collect::<Vec<_>>(), vec![3, 1]);
        assert!(matches!(contexts(&ekko, &words(&["1", "9"])), Err(EkkoError::InvalidId(_))));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// What stands before an item is answered in one call, not one call per
    /// hop: how many open tasks, and the roots -- the open ones free to start.
    /// A closed blocker ends a path, and a root already listed as a direct
    /// blocker is not described a second time.
    #[test]
    fn context_names_the_roots_above_an_item() {
        let (ekko, dir) = board("roots");
        for name in ["left root", "right root", "middle", "tip", "closed"] {
            ekko.create_task(&words(&[name])).unwrap();
        }
        ekko.set_blocked_by(&words(&["@3", "1", "2"])).unwrap();
        ekko.set_blocked_by(&words(&["@4", "3", "5"])).unwrap();
        ekko.set_state(&words(&["@5", "done"]), false).unwrap();

        let tip = context(&ekko, "4").unwrap();
        assert_eq!(tip.roots.iter().map(|root| root.id).collect::<Vec<_>>(), vec![1, 2]);
        assert_eq!(tip.waits_on, 3);
        let text = tip.text();
        assert!(text.contains("\nRoots, free to start\n   1. [pending] left root\n"), "{text}");
        assert!(text.contains("waits on 3 open tasks, directly or through others"), "{text}");

        let middle = context(&ekko, "3").unwrap().text();
        assert!(middle.contains("   1. [pending] listed above"), "{middle}");

        let root = context(&ekko, "1").unwrap();
        assert!(root.roots.is_empty() && root.waits_on == 0);
        assert!(root.text().contains("2 open tasks wait on this"), "{}", root.text());

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Everything a write changed after the cursor, put away or not, nothing
    /// from before it and nothing twice: the cursor a read returns hears only
    /// later writes, and a clock cursor from an older prime is still answered.
    #[test]
    fn changes_lists_what_moved_since_a_cursor_including_what_was_put_away() {
        let (ekko, dir) = board("changes");
        ekko.create_task(&words(&["untouched"])).unwrap();
        ekko.create_task(&words(&["will be done"])).unwrap();
        ekko.create_task(&words(&["will be deleted"])).unwrap();
        let cursor = prime(&ekko, "default board").unwrap().cursor;
        assert!(changes(&ekko, cursor).unwrap().items.is_empty(), "the cursor handed back what it had seen");

        ekko.set_state(&words(&["@2", "done"]), false).unwrap();
        ekko.delete_items(&words(&["3"])).unwrap();

        let seen = changes(&ekko, cursor).unwrap();
        assert_eq!(ids(&seen.items), vec![2, 3]);
        assert_eq!(seen.items[1].away, Some("trashed"));
        assert_eq!(seen.cursor, cursor + 2);
        assert!(changes(&ekko, seen.cursor).unwrap().items.is_empty());

        let clock = changes(&ekko, 1_000_000_000_000).unwrap();
        assert_eq!(ids(&clock.items), vec![1, 2, 3], "a clock cursor from an older prime went unanswered");
        assert_eq!(clock.cursor, seen.cursor);

        std::fs::remove_dir_all(&dir).ok();
    }
}
