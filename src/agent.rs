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
use crate::item::{Item, Knowledge, State};
use crate::lexical::{self, Query};
use crate::render::{Inversion, ProjectSummary, RoadmapStep, Stats};
use crate::storage::ItemMap;

/// How much of a reason `prime` quotes before pointing at `context`.
const NOTE_CLIP: usize = 300;
/// How much of a task's description a one-line listing carries.
const TASK_CLIP: usize = 160;
/// How new a handoff is for the handoff prompt to take it for this session's
/// own: an hour, longer than a session takes between writing one and asking.
const RECENT_HANDOFF_MS: i64 = 60 * 60 * 1000;
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
// Task::urgency_due in src/Task.cpp), applied by `urgency` -- all but one.
const URGENCY_DUE: f64 = 12.0;
/// Taskwarrior's default is 8, which outweighs the whole gap between p3 and
/// p2 (2.1): any p2 task holding up p2 work ranked above every p3 task on its
/// own. `man taskrc` advises 0 where urgency is inherited, since the blocker
/// already takes on what it holds up, and here it inherits the highest
/// priority and the latest finish of that work. 1 keeps "more work waiting on
/// it" as what orders tasks of one priority, below priority itself, which is
/// the order `next` describes (decision on task 284).
const URGENCY_BLOCKING: f64 = 1.0;
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
const WAITING_KEPT: usize = 5;
const NOTES_KEPT: usize = 3;
/// Gotchas and procedures a prime lists, newest first, each by its first
/// line: the traps and the steps a session should have in mind before it
/// starts. Decisions are only counted -- there are more of them, and search
/// finds the one that matters when it matters.
const KNOWLEDGE_SHOWN: usize = 5;
/// Where a prime says the gotchas and procedures it did not list are.
const KNOWLEDGE_REST: &str = "search with the gotcha or procedure filter";
/// The longest a "+N more" line gets, reserved for each section a cut leaves
/// short: the indent, a count of up to six digits, and the longest place to
/// look, "search with the blocked filter".
const MORE_LINE: usize = 52;
/// Items each line of "Needs attention" names before it counts the rest. The
/// block is outside the prime's budget, so it has to stay short on any board.
const ATTENTION_SHOWN: usize = 10;
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
    /// Superseded notes, each with the notes superseding it, in id order.
    /// One in the trash supersedes nothing, the way a trashed blocker blocks
    /// nothing, and one stashed still does: stashing hides, it does not undo.
    superseded_by: HashMap<u32, Vec<u32>>,
}

impl Links {
    fn build(all: &ItemMap) -> Self {
        let uids: HashMap<String, u32> =
            all.iter().filter_map(|(id, item)| Some((item.uid.clone()?, *id))).collect();
        let mut blockers: HashMap<u32, Vec<u32>> = HashMap::new();
        let mut dependents: HashMap<u32, Vec<u32>> = HashMap::new();
        let mut attached: HashMap<String, Vec<u32>> = HashMap::new();
        let mut superseded_by: HashMap<u32, Vec<u32>> = HashMap::new();
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
            if let Some(&older) = item.supersedes.as_deref().filter(|_| item.trashed.is_none()).and_then(|uid| uids.get(uid)) {
                superseded_by.entry(older).or_default().push(*id);
            }
        }
        for newer in superseded_by.values_mut() {
            newer.sort_unstable();
        }
        Links { uids, blockers, dependents, attached, superseded_by }
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
/// 21-day ramp, 6 or 3.9 for priority 3 or 2, 1 for holding up open work, and
/// up to 2 for age. The weights are Taskwarrior's defaults but the one for
/// holding up work; see `URGENCY_BLOCKING`.
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
    /// The task's state, or `None` for a note; written as the state's word,
    /// or `note`.
    #[serde(serialize_with = "state_or_note")]
    pub state: Option<State>,
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
    /// Who holds a task in progress, as the reader sees it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub held: Option<Held>,
    /// A note's lasting kind -- decision, gotcha or procedure; see
    /// `Item::knowledge`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub knowledge: Option<Knowledge>,
    /// The note this one supersedes, by display id, while it is on the board.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<u32>,
    /// The notes superseding this one: it is history, no longer in force.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub superseded_by: Vec<u32>,
    /// Where this task stands in a sequence of steps, as (step, of): see
    /// `Reader::sequence`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<(usize, usize)>,
    pub updated_at: i64,
    /// Notes attached to this task.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<NoteRef>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteRef {
    pub id: u32,
    pub uid: Option<String>,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub knowledge: Option<Knowledge>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub superseded_by: Vec<u32>,
    /// On a question: whether it has been answered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answered: Option<bool>,
}

impl NoteRef {
    /// What a listing puts before a typed note's text -- "[gotcha] ", or
    /// "[decision, superseded by 9] ", "[question] " -- and nothing before an
    /// ordinary one.
    fn mark(&self) -> String {
        if let Some(answered) = self.answered {
            return if answered { "[answered] " } else { "[question] " }.to_string();
        }
        match (self.knowledge, self.superseded_by.as_slice()) {
            (None, _) => String::new(),
            (Some(kind), []) => format!("[{}] ", kind.word()),
            (Some(kind), newer) => format!("[{}, superseded by {}] ", kind.word(), join(newer)),
        }
    }
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// Everything the views share, read off one load of the board.
struct Reader<'a> {
    graph: Graph<'a>,
    /// Who reads, to tell a claim that is theirs from one another session
    /// holds; `None` reads every claim as another's.
    me: Option<crate::holder::Actor>,
    /// Visible notes, by the uid of the task each is attached to.
    order: HashMap<&'a str, usize>,
    today: String,
}

impl<'a> Reader<'a> {
    fn new(all: &'a ItemMap, links: Arc<Links>, phases: &'a [String]) -> Self {
        Reader {
            graph: Graph::new(all, links),
            me: None,
            order: phase_order(phases),
            today: chrono::Local::now().format("%Y-%m-%d").to_string(),
        }
    }

    /// A reader over a shared board, on the links its version already has.
    fn shared(all: &'a Arc<ItemMap>, phases: &'a [String]) -> Self {
        Reader::new(all, links_of(all), phases)
    }

    /// This reader, reading for `ekko`'s actor.
    fn seen_by(mut self, ekko: &Ekko) -> Self {
        self.me = ekko.actor.clone();
        self
    }

    /// Who holds `item`, if it is a task in progress that someone holds.
    fn held(&self, item: &Item) -> Option<Held> {
        let holder = item.held_by.as_ref().filter(|_| State::of(item) == Some(State::Progress))?;
        Some(Held {
            by: self.me.as_ref().map_or_else(|| holder.label(), |me| me.name(holder)),
            since: holder.since,
            yours: self.me.as_ref().is_some_and(|me| me.is(holder)),
            gone: !holder.alive(),
        })
    }

    /// The display id holding `uid`.
    fn uid(&self, uid: &str) -> Option<u32> {
        self.graph.links.uids.get(uid).copied()
    }

    fn entry(&self, item: &Item) -> Entry {
        self.entry_counting(item, true)
    }

    /// The notes superseding `id`: none while it is still in force.
    fn superseded_by(&self, id: u32) -> Vec<u32> {
        self.graph.links.superseded_by.get(&id).cloned().unwrap_or_default()
    }

    fn note_ref(&self, note: &Item) -> NoteRef {
        NoteRef {
            id: note.id,
            uid: note.uid.clone(),
            description: note.description.clone(),
            knowledge: note.knowledge,
            superseded_by: self.superseded_by(note.id),
            answered: note.question.as_ref().map(|question| question.answer.is_some()),
        }
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
            .map(|note| self.note_ref(note))
            .collect();
        Entry {
            id: item.id,
            uid: item.uid.clone(),
            state: State::of(item),
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
            held: self.held(item),
            knowledge: item.knowledge,
            supersedes: item.supersedes.as_deref().and_then(|uid| self.uid(uid)),
            superseded_by: self.superseded_by(item.id),
            step: {
                let sequence = self.sequence(item.id);
                sequence.iter().position(|id| *id == item.id).map(|at| (at + 1, sequence.len()))
            },
        }
    }

    /// The sequence of steps task `id` stands in, first step first: tasks
    /// linked by blocked_by into one line, each link the only one on both
    /// sides -- a step blocked by the one before and by nothing else, and
    /// holding up the one after and nothing else. A branch ends the line.
    /// Empty when `id` stands in no line of two or more.
    fn sequence(&self, id: u32) -> Vec<u32> {
        let links = &self.graph.links;
        let only = |map: &HashMap<u32, Vec<u32>>, id: u32| match map.get(&id).map(Vec::as_slice) {
            Some([one]) => Some(*one),
            _ => None,
        };
        let step = |id: &u32| self.graph.all.get(id).is_some_and(|item| item.is_task && visible(item));
        if !step(&id) {
            return Vec::new();
        }
        let mut sequence = vec![id];
        let mut at = id;
        while let Some(before) =
            only(&links.blockers, at).filter(|before| only(&links.dependents, *before) == Some(at) && step(before) && !sequence.contains(before))
        {
            sequence.insert(0, before);
            at = before;
        }
        at = id;
        while let Some(after) =
            only(&links.dependents, at).filter(|after| only(&links.blockers, *after) == Some(at) && step(after) && !sequence.contains(after))
        {
            sequence.push(after);
            at = after;
        }
        if sequence.len() < 2 { Vec::new() } else { sequence }
    }

    fn ready(&self, item: &Item) -> bool {
        visible(item)
            && holds(item)
            && State::of(item).is_some_and(State::can_start)
            && self.graph.open_blockers(item.id).is_empty()
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
    word_or_note(State::of(item))
}

/// A task's state as the views print it, or `note` for what has none.
fn word_or_note(state: Option<State>) -> &'static str {
    state.map_or("note", State::word)
}

fn state_or_note<S: serde::Serializer>(state: &Option<State>, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(word_or_note(*state))
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
    /// The first tasks in the waiting state, by priority; `waiting_total`
    /// counts them all. Absent on a board that waits on nothing.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub waiting: Vec<Entry>,
    #[serde(skip_serializing_if = "is_zero")]
    pub waiting_total: usize,
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
    /// Where the last session stopped: the newest handoff on open work,
    /// this reader's own first.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handoff: Option<Handoff>,
    /// The other handoffs on open work written in the last hour, newest
    /// first: other sessions stopping on the same board.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub other_handoffs: Vec<OtherHandoff>,
    /// The newest gotchas and procedures in force; `knowledge_total` counts
    /// them all.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub knowledge: Vec<NoteRef>,
    #[serde(skip_serializing_if = "is_zero")]
    pub knowledge_total: usize,
    /// Decisions in force, counted: search with the decision filter reads them.
    #[serde(skip_serializing_if = "is_zero")]
    pub decisions: usize,
    /// The questions no one has answered yet, oldest first: what the user
    /// is waiting to be asked, or asked and has not answered.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub waiting_on_you: Vec<Asked>,
    /// The questions this session asked, answered in the last day.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub answered: Vec<Asked>,
}

/// A question as the prime lists it: the note, the task it is about, who
/// asked and how far the board has moved since, and the answer once given.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Asked {
    pub id: u32,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub about: Option<u32>,
    pub by: String,
    /// Writes to the board since it was asked.
    pub moved: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answered_by: Option<String>,
}

/// Who holds a task in progress, as one reader sees it: by whom and since
/// when, whether it is the reader, and whether the holder is gone -- a
/// Claude Code process that no longer runs, whose task is free to take up.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Held {
    pub by: String,
    pub since: i64,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub yours: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub gone: bool,
}

impl Held {
    /// Held by another session that still runs: not work to take up.
    pub fn elsewhere(&self) -> bool {
        !self.yours && !self.gone
    }

    fn text(&self) -> String {
        match (self.yours, self.gone) {
            (true, _) => "yours".to_string(),
            (false, true) => format!("held by {}, gone", self.by),
            (false, false) => format!("held by {}", self.by),
        }
    }
}

/// A handoff the prime names under the one it shows: the note, its task,
/// when it was written, and who holds the task, as the reader sees it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OtherHandoff {
    pub id: u32,
    pub task: u32,
    pub updated_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub held: Option<String>,
}

/// How many other handoffs a prime names before it counts the rest.
const OTHER_HANDOFFS_SHOWN: usize = 5;

/// The open questions a prime quotes, oldest first: the rest are counted, and
/// `ekko --sessions` lists them by the session that asked.
const WAITING_ON_YOU_SHOWN: usize = 10;

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
    // The revision before the board: a cursor may lag the board it came
    // with, and then only repeats a change; it never runs ahead and skips one.
    let cursor = ekko.storage.get_counters()?.revision as i64;
    let all = ekko.storage.get_shared()?;
    let phases = ekko.storage.get_phases()?;
    let reader = Reader::shared(&all, &phases).seen_by(ekko);

    let (next, _) = reader.next(None);
    let (mut doing, mut ready): (Vec<Entry>, Vec<Entry>) =
        next.into_iter().partition(|entry| entry.state == Some(State::Progress));

    let mut blocked: Vec<&Item> = all
        .values()
        .filter(|item| visible(item) && holds(item) && !matches!(State::of(item), Some(State::Progress | State::Waiting)))
        .filter(|item| !reader.graph.open_blockers(item.id).is_empty())
        .collect();
    blocked.sort_by_key(|item| (std::cmp::Reverse(item.priority.unwrap_or(1)), item.id));
    let blocked_total = blocked.len();
    // Entries only for what a prime shows: working one out walks the graph,
    // and a long chain holds thousands of blocked tasks nobody reads.
    let mut blocked: Vec<Entry> = blocked.into_iter().take(BLOCKED_SHOWN).map(|item| reader.entry(item)).collect();

    // Held by something outside the board, so in no list of work to take
    // up, blocked or not: listed apart, where a session sees what it should
    // not start and what to ask about.
    let mut waiting: Vec<&Item> =
        all.values().filter(|item| visible(item) && State::of(item) == Some(State::Waiting)).collect();
    waiting.sort_by_key(|item| (std::cmp::Reverse(item.priority.unwrap_or(1)), item.id));
    let waiting_total = waiting.len();
    let mut waiting: Vec<Entry> = waiting.into_iter().take(BLOCKED_SHOWN).map(|item| reader.entry(item)).collect();

    // Where the last session stopped, shown once in its own section rather
    // than clipped again among the notes of its task. With several sessions
    // on the board, the newest handoff on a task this reader holds, then on
    // one no other running session holds, then the newest of all; the rest
    // written in the last hour are named under it, so a session cleared in
    // one terminal does not resume the work another still holds.
    let mut handoffs: Vec<(&Item, &Item)> = all
        .values()
        .filter(|note| note.handoff && visible(note))
        .filter_map(|note| {
            let task = &all[&reader.uid(note.attached_to.as_deref()?)?];
            (visible(task) && holds(task)).then_some((note, task))
        })
        .collect();
    let rank = |task: &Item| match reader.held(task) {
        Some(held) if held.yours => 0,
        Some(held) if held.elsewhere() => 2,
        _ => 1,
    };
    handoffs.sort_by_key(|(note, task)| (rank(task), std::cmp::Reverse((updated(note), note.id))));
    let hour_ago = chrono::Local::now().timestamp_millis() - 3_600_000;
    let other_handoffs: Vec<OtherHandoff> = handoffs
        .iter()
        .skip(1)
        .filter(|(note, _)| updated(note) >= hour_ago)
        .map(|(note, task)| OtherHandoff {
            id: note.id,
            task: task.id,
            updated_at: updated(note),
            held: reader.held(task).map(|held| held.text()),
        })
        .collect();
    let handoff = handoffs.first().map(|(note, task)| Handoff {
        id: note.id,
        uid: note.uid.clone(),
        task: task.id,
        task_state: state_word(task),
        updated_at: updated(note),
        description: note.description.clone(),
    });
    if let Some(shown) = &handoff {
        for entry in doing.iter_mut().chain(ready.iter_mut()).chain(blocked.iter_mut()).chain(waiting.iter_mut()) {
            entry.notes.retain(|note| note.id != shown.id);
        }
    }

    // What stays true, beside the work: gotchas and procedures listed, loose
    // or attached to any task, done ones included; decisions counted. A
    // superseded note is history, and no section of a prime shows it.
    let in_force =
        |note: &&Item| visible(note) && !note.is_task && !reader.graph.links.superseded_by.contains_key(&note.id);
    let mut lasting: Vec<&Item> = all
        .values()
        .filter(in_force)
        .filter(|note| matches!(note.knowledge, Some(Knowledge::Gotcha | Knowledge::Procedure)))
        .collect();
    lasting.sort_by_key(|note| (std::cmp::Reverse(updated(note)), std::cmp::Reverse(note.id)));
    let knowledge_total = lasting.len();
    let knowledge: Vec<NoteRef> = lasting.into_iter().take(KNOWLEDGE_SHOWN).map(|note| reader.note_ref(note)).collect();
    let decisions = all.values().filter(in_force).filter(|note| note.knowledge == Some(Knowledge::Decision)).count();
    for entry in doing.iter_mut().chain(ready.iter_mut()).chain(blocked.iter_mut()).chain(waiting.iter_mut()) {
        entry.notes.retain(|note| note.superseded_by.is_empty() && !knowledge.iter().any(|shown| shown.id == note.id));
    }

    // Under a handoff, only the loose notes changed after it: an older one is
    // history the last session had the chance to carry into its handoff, and
    // quoting it at every start cost a fifth of the prime and once led a
    // session to recommend work long done. Without a handoff, the newest.
    let since = handoff.as_ref().map_or(i64::MIN, |shown| shown.updated_at);
    let mut notes: Vec<&Item> = all
        .values()
        .filter(|item| visible(item) && !item.is_task && item.attached_to.is_none() && item.knowledge.is_none())
        .filter(|item| updated(item) > since)
        .collect();
    notes.retain(|note| note.question.as_ref().is_none_or(|question| question.answer.is_some()));
    notes.sort_by_key(|item| (std::cmp::Reverse(updated(item)), std::cmp::Reverse(item.id)));
    let recent_notes = notes.into_iter().take(RECENT_NOTES).map(|note| reader.note_ref(note)).collect();

    // Questions in their own sections: the open ones for the user, whoever
    // asked, and the answers this session is waiting for -- its own, or its
    // conversation's from before a restart. An open one is not quoted again
    // under its task.
    let registry = reader.me.as_ref().and_then(|me| me.registry.as_ref());
    let mine = |asker: &crate::holder::Holder| {
        reader.me.as_ref().is_some_and(|me| {
            me.is(asker) || (!asker.alive() && me.conversation().is_some_and(|now| asker.conversation_in(registry).as_deref() == Some(now.as_str())))
        })
    };
    let day_ago = chrono::Local::now().timestamp_millis() - 86_400_000;
    let mut questions: Vec<&Item> = all.values().filter(|item| visible(item) && item.question.is_some()).collect();
    questions.sort_by_key(|item| (item.timestamp, item.id));
    let (mut waiting_on_you, mut answered) = (Vec::new(), Vec::new());
    for note in questions {
        let Some(question) = &note.question else { continue };
        let asked = asked(note, question, &reader, cursor as u64);
        match &question.answer {
            None => waiting_on_you.push(asked),
            Some(answer) if answer.at >= day_ago && question.asked_by.as_ref().is_some_and(mine) => answered.push(asked),
            Some(_) => {}
        }
    }
    for entry in doing.iter_mut().chain(ready.iter_mut()).chain(blocked.iter_mut()).chain(waiting.iter_mut()) {
        entry.notes.retain(|note| !waiting_on_you.iter().any(|asked| asked.id == note.id));
    }

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
        .filter(|item| reader.ready(item))
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
        cursor,
        stats: ekko.compute_stats(&all),
        roadmap,
        rootless,
        doing,
        ready,
        blocked,
        blocked_total,
        waiting,
        waiting_total,
        recent_notes,
        broken: broken_dependencies(&all).into_iter().collect(),
        inversions,
        overdue,
        freed_by_cancelling,
        handoff,
        other_handoffs,
        knowledge,
        knowledge_total,
        decisions,
        waiting_on_you,
        answered,
    })
}

/// What to take up next, best first; see `Reader::next` for the order.
pub fn next(ekko: &Ekko, limit: Option<usize>) -> Result<Vec<Entry>, EkkoError> {
    Ok(next_listed(ekko, limit)?.0)
}

/// Where each task stands in a sequence of steps (see `Reader::sequence`),
/// for the board view, which reads items rather than entries.
pub fn steps(ekko: &Ekko) -> Result<HashMap<u32, (usize, usize)>, EkkoError> {
    Ok(sequences(ekko)?
        .into_iter()
        .map(|(id, sequence)| {
            let at = sequence.iter().position(|step| *step == id).unwrap_or_default();
            (id, (at + 1, sequence.len()))
        })
        .collect())
}

/// Each task that stands in a sequence of steps, with the whole sequence,
/// first step first.
pub fn sequences(ekko: &Ekko) -> Result<HashMap<u32, Vec<u32>>, EkkoError> {
    let all = ekko.storage.get_shared()?;
    let phases = ekko.storage.get_phases()?;
    let reader = Reader::shared(&all, &phases);
    let mut sequences = HashMap::new();
    for id in all.keys() {
        if sequences.contains_key(id) {
            continue;
        }
        let sequence = reader.sequence(*id);
        for step in &sequence {
            sequences.insert(*step, sequence.clone());
        }
    }
    Ok(sequences)
}

/// `next`, with how many tasks were candidates in all.
pub fn next_listed(ekko: &Ekko, limit: Option<usize>) -> Result<(Vec<Entry>, usize), EkkoError> {
    let all = ekko.storage.get_shared()?;
    let phases = ekko.storage.get_phases()?;
    Ok(Reader::shared(&all, &phases).seen_by(ekko).next(limit))
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
    /// Roots that are not free to start -- in progress, waiting or stashed --
    /// but still hold this up: listed apart, so none is offered as work to
    /// take up that `next` would not offer.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub held_roots: Vec<Link>,
    pub dependents: Vec<Link>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attached_to: Option<Link>,
    /// The earlier note this one replaces, kept as history.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<Link>,
    /// The notes replacing this one: while any is there, it is not in force.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub superseded_by: Vec<Link>,
    /// On a question: who asked, and the answer once given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub question: Option<Asked>,
    /// The sequence of steps it stands in, first step first; see
    /// `Reader::sequence`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sequence: Vec<Link>,
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
    let reader = Reader::shared(&all, &phases).seen_by(ekko);
    Ok(ids.into_iter().map(|id| neighbourhood(&all, &reader, id)).collect())
}

/// Several neighbourhoods as one reply, each item described once: one that
/// has a block of its own is pointed to from the others, not repeated.
pub fn contexts_text(read: &[Context], detail: Detail) -> String {
    let order: Vec<u32> = read.iter().map(|context| context.item.id).collect();
    read.iter().map(|context| context.text_among(detail, &order)).collect::<Vec<_>>().join("\n")
}

fn neighbourhood(all: &ItemMap, reader: &Reader<'_>, id: u32) -> Context {
    let item = &all[&id];

    let link = |id: &u32| {
        all.get(id).map(|other| Link {
            id: other.id,
            uid: other.uid.clone(),
            // A trashed blocker holds nothing up, so its state would mislead.
            state: match other.trashed {
                Some(_) => "in the trash",
                None => other.knowledge.map_or_else(|| state_word(other), Knowledge::word),
            },
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
    let free = |root: &&u32| {
        reader.graph.all.get(*root).is_some_and(|item| reader.ready(item) && State::of(item) != Some(State::Progress))
    };
    let (free, held): (Vec<&u32>, Vec<&u32>) = root_ids.iter().partition(free);
    let roots = free.into_iter().filter_map(link).collect();
    let held_roots = held
        .into_iter()
        .filter_map(|root| {
            let stashed = reader.graph.all.get(root).is_some_and(|item| item.stashed.is_some());
            link(root).map(|found| Link { state: if stashed { "stashed" } else { found.state }, ..found })
        })
        .collect();
    let entry = reader.entry(item);
    let supersedes = entry.supersedes.as_ref().and_then(link);
    let superseded_by = entry.superseded_by.iter().filter_map(link).collect();
    let revision = all.values().filter_map(|other| other.rev).max().unwrap_or(0);
    let question = item.question.as_ref().map(|question| asked(item, question, reader, revision));
    let sequence = reader.sequence(id).iter().filter_map(link).collect();

    Context {
        item: entry,
        created: item.date.clone(),
        stashed: item.stashed.is_some(),
        trashed: item.trashed.is_some(),
        blockers,
        waits_on,
        roots,
        held_roots,
        dependents,
        attached_to,
        supersedes,
        superseded_by,
        question,
        sequence,
    }
}

/// A question as the views show it, the board being at `revision`: who
/// asked and who answered as the reader names them, and how far the board
/// moved before the answer came, or since it was asked while none has.
fn asked(note: &Item, question: &crate::item::Question, reader: &Reader<'_>, revision: u64) -> Asked {
    let registry = reader.me.as_ref().and_then(|me| me.registry.as_ref());
    let name = |holder: Option<&crate::holder::Holder>| holder.map_or_else(|| "the user".to_string(), |holder| holder.label_in(registry));
    Asked {
        id: note.id,
        text: note.description.clone(),
        about: note.attached_to.as_deref().and_then(|uid| reader.uid(uid)),
        by: name(question.asked_by.as_ref()),
        moved: match &question.answer {
            Some(answer) => answer.rev.saturating_sub(question.rev + 1),
            None => revision.saturating_sub(question.rev),
        },
        answer: question.answer.as_ref().map(|answer| answer.text.clone()),
        answered_by: question.answer.as_ref().map(|answer| name(answer.by.as_ref())),
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
    let reader = Reader::shared(&all, &phases).seen_by(ekko);

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
    let reader = Reader::shared(&all, &phases).seen_by(ekko);
    let now = chrono::Local::now().timestamp_millis();
    let mut out = String::new();
    let mut section = |title: &str, items: Vec<&Item>, empty: &str| {
        let _ = writeln!(out, "{title} ({})", items.len());
        if items.is_empty() {
            let _ = writeln!(out, "      {empty}");
        }
        for item in items.iter().take(limit) {
            let entry = reader.entry_counting(item, false);
            let mut body = format!("[{}] {}", listed_state(&entry), clip(&item.description, TASK_CLIP));
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
    let reader = Reader::shared(&all, &phases).seen_by(ekko);
    let tasks: Vec<&Item> = match task {
        Some(target) => {
            let ids = ekko.validate_ids(&[target.trim_start_matches('@').to_string()], &all)?;
            let mut item = &all[&ids[0]];
            // A note's id stands for the task it explains: the id a session
            // has at hand is often its own handoff's, from the write before.
            if let Some(&task) = item.attached_to.as_deref().filter(|_| !item.is_task).and_then(|uid| reader.graph.links.uids.get(uid)) {
                item = &all[&task];
            }
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
         - The next step, concrete enough to start on without asking. When it needs the user's word first (it spends their quota, publishes, deletes, or is their choice), write only that the next session asks them whether to do it, and leave out how: a plan on the page reads as leave to start.\n\
         - Open questions for the user.\n\
         Leave out what the board or the code already says, and name by id any note the next session must read: the prime leaves out loose notes older than the handoff. A handoff replaces the task's earlier one, which stays on the task as an ordinary note. Then tell the user it is safe to /clear, and that after it any message, even just \"continue\", starts the next session -- \"continue <task id>\" when other sessions hand off on this board too: Claude Code never starts a turn on its own.\n"
    );
    // A handoff written minutes ago is most likely this session's own, and a
    // second one would demote it, or land on a task the session never
    // touched because it happened to be the one left in progress.
    let now = chrono::Local::now().timestamp_millis();
    let recent = all
        .values()
        .filter(|note| note.handoff && visible(note))
        .map(|note| (note, now - note.updated_at.unwrap_or(note.timestamp)))
        .filter(|(_, age)| *age < RECENT_HANDOFF_MS)
        .min_by_key(|(_, age)| *age);
    if let Some((note, age)) = recent {
        let on = note.attached_to.as_deref().and_then(|uid| reader.graph.links.uids.get(uid)).copied();
        let minutes = age.max(0) / 60_000;
        let when = if minutes < 1 { "under a minute ago".to_string() } else { format!("{minutes} min ago") };
        let target = match (tasks.as_slice(), on) {
            ([task], Some(on)) if task.id == on => "this task".to_string(),
            (_, Some(on)) => format!("task {on}"),
            (_, None) => "no open task".to_string(),
        };
        let _ = writeln!(
            out,
            "\nHandoff {} on {target} was written {when}. If this session wrote it, do not write another: bring it up to date with edit append on {}.",
            note.id, note.id
        );
    }
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

/// How much of an item's text names it where a person picks it from a list.
const MENTION_CLIP: usize = 60;

/// The items a person can mention by name, as (id, what to call it): the
/// open tasks, the handoffs, and the decisions, gotchas and procedures in
/// force, by id. Closed work and plain notes are left out, so the list stays
/// one a person can pick from; context still reads any of them.
pub fn mentionable(ekko: &Ekko) -> Result<Vec<(u32, String)>, EkkoError> {
    let all = ekko.storage.get_shared()?;
    let links = links_of(&all);
    Ok(all
        .values()
        .filter(|item| visible(item))
        .filter(|item| {
            holds(item) || item.handoff || (item.knowledge.is_some() && !links.superseded_by.contains_key(&item.id))
        })
        .map(|item| (item.id, format!("{} \u{b7} {}", item.id, mention_name(&item.description))))
        .collect())
}

/// An item's text cut to fit a menu line, with an ellipsis where it was cut
/// and no count: a menu is read at a glance, and the item reads whole.
fn mention_name(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= MENTION_CLIP {
        return flat;
    }
    let head: String = flat.chars().take(MENTION_CLIP).collect();
    format!("{}\u{2026}", head.trim_end())
}

/// What a board holds, for a search that named nothing to look for: counts
/// by state and by board, in a few lines, instead of every item.
fn summary(all: &ItemMap) -> String {
    let shown: Vec<&Item> = all.values().filter(|item| visible(item)).collect();
    let tasks: Vec<&Item> = shown.iter().copied().filter(|item| item.is_task).collect();
    let notes = shown.len() - tasks.len();
    let count = |n: usize, one: &str, many: &str| if n == 1 { format!("1 {one}") } else { format!("{n} {many}") };

    let states: Vec<String> =
        [State::Progress, State::Paused, State::Waiting, State::Pending, State::Done, State::Cancelled]
            .into_iter()
            .filter_map(|state| {
                let n = tasks.iter().filter(|item| State::of(item) == Some(state)).count();
                (n > 0).then(|| format!("{n} {}", state.word()))
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
    /// Whether `since` is a revision past the board's own: counters.json went
    /// back, and it is that, not the journal, that keeps the list from being told.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub ahead: bool,
    /// The questions among the items, each with its answer once given: a
    /// session resumed after its question was answered reads the answer
    /// here, in what moved, and not only in a prime it may not read.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub questions: Vec<(u32, Option<String>)>,
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
    // The revision first, then the board and the journal: a writer publishes
    // the revision last, so every write up to `cursor` is already on disk.
    // A write past it shows in the board but waits for the next call, so no
    // change is skipped and none is told twice.
    let cursor = ekko.storage.get_counters()?.revision as i64;
    let all = ekko.storage.get_shared()?;
    let phases = ekko.storage.get_phases()?;
    let reader = Reader::shared(&all, &phases).seen_by(ekko);
    let moved = |item: &&Item| {
        if since >= CLOCK_CURSOR {
            updated(item) >= since
        } else {
            let rev = item.rev.unwrap_or(0) as i64;
            rev > since && rev <= cursor
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
            entry["rev"].as_i64().is_some_and(|rev| rev > since && rev <= cursor)
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
    // A cursor ahead of the board's revision means counters.json went back
    // (lost, or restored from an older copy): what moved cannot be told.
    let complete = (since >= CLOCK_CURSOR || cursor >= since) && from.is_none_or(|from| since < CLOCK_CURSOR && since + 1 >= from);
    let ahead = since < CLOCK_CURSOR && cursor < since;
    let questions = items
        .iter()
        .filter_map(|entry| Some((entry.id, all.get(&entry.id)?.question.as_ref()?.answer.as_ref().map(|answer| answer.text.clone()))))
        .collect();

    Ok(Changes {
        since,
        cursor,
        items,
        released: released.into_iter().collect(),
        blocked: blocked.into_iter().collect(),
        removed,
        complete,
        ahead,
        questions,
    })
}

/// What moved since `since`, or the prime once the list would run longer than
/// the prime's budget and the prime is the shorter read: on a board that moved
/// a lot, the list is unbounded -- 429 KB for changes(0) over 5,000 items --
/// and the resume view says where things stand in a few thousand characters.
/// Either way the answer carries a cursor to go on from.
pub fn changes_within(ekko: &Ekko, since: i64, board: &str) -> Result<String, EkkoError> {
    let moved = changes(ekko, since)?.text();
    if moved.chars().count() <= PRIME_BUDGET {
        return Ok(moved);
    }
    let view = prime(ekko, board)?.text();
    if view.chars().count() >= moved.chars().count() {
        return Ok(moved);
    }
    Ok(format!("More moved since {since} than a list should hold, so this is the resume view instead; go on from its cursor.\n{view}"))
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

/// Where ekko keeps what it knows of sessions: the XDG state directory,
/// outside every board, so that reading a board still writes nothing to it.
fn state_dir(home: &Path) -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local").join("state"))
        .join("ekko")
}

/// Where the hook keeps the cursor it served each session.
pub fn session_state_dir(home: &Path) -> PathBuf {
    state_dir(home).join("sessions")
}

/// Where the hook records the conversation each Claude Code process runs:
/// see `holder::Registry`.
pub fn processes_dir(home: &Path) -> PathBuf {
    state_dir(home).join("processes")
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
    // The conversation now running in this session's process: what its
    // claims name, and how a conversation resumed after a restart knows the
    // ones it made before, which it takes back.
    let mut taken = Vec::new();
    if let (Some(actor), Some(conversation)) = (&ekko.actor, &event.session_id) {
        actor.record(conversation, ekko.storage.storage_path());
        taken = ekko.take_back(conversation).unwrap_or_else(|error| {
            eprintln!("ekko: what this conversation held before was not taken back: {error}");
            Vec::new()
        });
    }
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
    let taken: String = taken.iter().map(|notice| format!("{notice}\n")).collect();
    Ok(format!("{taken}{text}"))
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

/// The Claude Code sessions on a board, as `ekko --sessions` shows them to
/// the user: each running one that started on it, and each, running or
/// ended, that holds work on it, finished some today or asked what still
/// waits -- with the command that resumes its conversation.
#[derive(Debug, Serialize)]
pub struct Sessions {
    pub board: String,
    pub sessions: Vec<SessionView>,
}

/// One session as `Sessions` lists it; the user at the terminal is one too,
/// when they hold work.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionView {
    pub name: String,
    pub running: bool,
    /// When the conversation it runs began in it, where the registry knows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resume: Option<String>,
    pub holding: Vec<SessionItem>,
    pub finished: Vec<SessionItem>,
    pub asking: Vec<SessionItem>,
}

#[derive(Debug, Serialize)]
pub struct SessionItem {
    pub id: u32,
    pub text: String,
}

pub fn sessions(ekko: &Ekko, board: &str) -> Result<Sessions, EkkoError> {
    let all = ekko.storage.get_shared()?;
    let registry = ekko.actor.as_ref().and_then(|actor| actor.registry.as_ref());
    let recorded = registry.map(crate::holder::Registry::all).unwrap_or_default();
    let view = |running: &crate::holder::Running| SessionView {
        name: running.label(),
        running: running.process().alive(),
        since: Some(running.since),
        resume: Some(running.resume()),
        holding: Vec::new(),
        finished: Vec::new(),
        asking: Vec::new(),
    };
    let here = ekko.storage.storage_path();
    let mut found: Vec<(Option<crate::holder::Process>, SessionView)> = recorded
        .iter()
        .filter(|running| running.board.as_deref() == Some(here) && running.process().alive())
        .map(|running| (Some(running.process()), view(running)))
        .collect();
    // The session behind a claim, a signature or a question: listed once,
    // from the registry where it is recorded, else from the claim itself.
    let mut session_of = |holder: &crate::holder::Holder| {
        let process = holder.process();
        if let Some(at) = found.iter().position(|(known, _)| *known == process) {
            return at;
        }
        let recorded = process.as_ref().and_then(|process| recorded.iter().find(|running| running.process() == *process));
        let session = recorded.map(view).unwrap_or_else(|| SessionView {
            name: holder.label_in(registry),
            running: holder.alive(),
            since: None,
            resume: holder.conversation.as_ref().map(|conversation| format!("claude --resume {conversation}")),
            holding: Vec::new(),
            finished: Vec::new(),
            asking: Vec::new(),
        });
        found.push((process, session));
        found.len() - 1
    };

    let today = chrono::Local::now().date_naive();
    let done_today = |item: &Item| {
        use chrono::TimeZone as _;
        let at = item.updated_at.and_then(|at| chrono::Local.timestamp_millis_opt(at).single());
        at.is_some_and(|at| at.date_naive() == today)
    };
    let mut items: Vec<&Item> = all.values().filter(|item| item.stashed.is_none() && item.trashed.is_none()).collect();
    items.sort_by_key(|item| item.id);
    let mut listed: Vec<(usize, u8, SessionItem)> = Vec::new();
    for item in items {
        let line = || SessionItem { id: item.id, text: clip(&item.description, TASK_CLIP) };
        match (State::of(item), &item.held_by, &item.done_by) {
            (Some(State::Progress), Some(holder), _) => listed.push((session_of(holder), 0, line())),
            (Some(State::Done), _, Some(by)) if done_today(item) => listed.push((session_of(by), 1, line())),
            _ => {}
        }
        if let Some(asker) = item.question.as_ref().filter(|question| question.answer.is_none()).and_then(|question| question.asked_by.as_ref()) {
            listed.push((session_of(asker), 2, line()));
        }
    }
    for (at, kind, line) in listed {
        let session = &mut found[at].1;
        match kind {
            0 => session.holding.push(line),
            1 => session.finished.push(line),
            _ => session.asking.push(line),
        }
    }
    found.sort_by(|(_, a), (_, b)| b.running.cmp(&a.running).then_with(|| a.name.cmp(&b.name)));
    Ok(Sessions { board: board.to_string(), sessions: found.into_iter().map(|(_, session)| session).collect() })
}

impl Sessions {
    pub fn text(&self) -> String {
        let running = self.sessions.iter().filter(|session| session.running).count();
        let ended = match self.sessions.len() - running {
            0 => String::new(),
            n => format!(" \u{b7} {n} ended with work here"),
        };
        let mut out = format!("ekko \u{b7} {} \u{b7} {running} running{ended}\n", self.board);
        if self.sessions.is_empty() {
            out.push_str("No Claude Code session has started on this board or holds work on it.\n");
        }
        for session in &self.sessions {
            let state = match (session.running, session.since) {
                (true, Some(since)) => format!("since {}", crate::holder::when(since)),
                (true, None) => "running".to_string(),
                (false, _) => "ended".to_string(),
            };
            let _ = writeln!(out, "\n{} \u{b7} {state}", session.name);
            if session.holding.is_empty() && session.finished.is_empty() && session.asking.is_empty() {
                let _ = writeln!(out, "      nothing in progress");
            }
            for (what, lines) in [("in progress", &session.holding), ("done today", &session.finished), ("asking", &session.asking)] {
                for line in lines {
                    let _ = writeln!(out, "      {what:<11} {:>4}. {}", line.id, line.text);
                }
            }
            if let Some(resume) = &session.resume {
                let _ = writeln!(out, "      resume with {resume}");
            }
        }
        out
    }
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
            let (state, answer) = match self.questions.iter().find(|(id, _)| *id == entry.id) {
                Some((_, None)) => ("question".to_string(), String::new()),
                Some((_, Some(answer))) => ("answered".to_string(), format!(" -> {}", clip(answer, NOTE_CLIP))),
                None => (listed_state(entry), String::new()),
            };
            let _ = writeln!(out, "{:>4}. [{state}{away}] {}{answer}{now}", entry.id, clip(&entry.description, TASK_CLIP));
        }
        for gone in &self.removed {
            let _ = writeln!(out, "{:>4}. [removed] {}", gone.id, gone.text);
        }
        // Said only when it is so, naming what fell short: the board's revision
        // behind the cursor, or the journal no longer reaching it.
        if self.ahead {
            let _ = writeln!(
                out,
                "This cursor is ahead of the board, whose revision is {}: counters.json went back, lost or restored from an older copy, so what moved cannot be told; prime again for the whole board.",
                self.cursor
            );
        } else if !self.complete {
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
    if let Some(state @ (State::Paused | State::Progress)) = entry.state.filter(|_| !stated) {
        meta.push(state.word().to_string());
    }
    if let Some(held) = &entry.held {
        meta.push(held.text());
    }
    if let Some((step, of)) = entry.step {
        meta.push(format!("step {step} of {of}"));
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
    format!("{:>8}. {}{}", note.id, note.mark(), clip(&note.description, max))
}

/// A gotcha or a procedure as a prime lists it: its kind, then its first
/// line -- which is what a note written to be found leads with.
fn knowledge_line(note: &NoteRef) -> String {
    let first = note.description.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or_default();
    format!("{:>4}. {}{}", note.id, note.mark(), clip(first, TASK_CLIP))
}

/// What a listing says an item is, in brackets: a task's state or a note's
/// kind, and for a superseded note, what supersedes it.
fn listed_state(entry: &Entry) -> String {
    let word = entry.knowledge.map_or_else(|| word_or_note(entry.state), Knowledge::word);
    match entry.superseded_by.as_slice() {
        [] => word.to_string(),
        newer => format!("{word}, superseded by {}", join(newer)),
    }
}

fn join(ids: &[u32]) -> String {
    ids.iter().map(u32::to_string).collect::<Vec<_>>().join(", ")
}

/// The first `ATTENTION_SHOWN` of `items`, joined by `separator`, and a count
/// of the rest with where to find them.
fn capped(items: Vec<String>, separator: &str, rest: &str) -> String {
    let more = items.len().saturating_sub(ATTENTION_SHOWN);
    let mut text = items.into_iter().take(ATTENTION_SHOWN).collect::<Vec<_>>().join(separator);
    if more > 0 {
        let _ = write!(text, "{separator}+{more} more{rest}");
    }
    text
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
    /// best first, blocked work, waiting work, loose notes -- and a section
    /// cut short says how much it left out and where the rest is.
    pub fn text_within(&self, budget: usize) -> String {
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let mut out = String::new();
        let _ = writeln!(out, "ekko \u{b7} {} \u{b7} cursor {}", self.board, self.cursor);

        let s = &self.stats;
        let mut counts = vec![format!("{}/{} tasks done ({}%)", s.complete, s.total, s.percent)];
        for (n, word) in [
            (s.in_progress, State::Progress.word()),
            (s.paused, State::Paused.word()),
            (s.waiting, State::Waiting.word()),
            (s.pending, State::Pending.word()),
            (s.cancelled, State::Cancelled.word()),
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
        // Only a board with typed notes pays for them: without any, these two
        // cost nothing and the prime is what it was before they existed.
        let decisions = match self.decisions {
            0 => String::new(),
            n => format!("\nDecisions ({n}): search with the decision filter\n"),
        };
        let knowledge_more = if self.knowledge.is_empty() {
            0
        } else {
            format!("      +{} more: {KNOWLEDGE_REST}\n", self.knowledge_total).chars().count()
        };
        // The waiting section's own "+N more" line is only reserved on a board
        // that has one, which leaves every other prime as it was.
        let waiting_more = if self.waiting.is_empty() { 0 } else { MORE_LINE };
        let fixed = out.chars().count()
            + attention.chars().count()
            + close.chars().count()
            + 4 * MORE_LINE
            + waiting_more
            + knowledge_more
            + decisions.chars().count();
        let mut room = Room(budget.saturating_sub(fixed));
        if let Some(handoff) = &self.handoff {
            out.push_str(&handoff.text());
        }
        if !self.other_handoffs.is_empty() {
            let named: Vec<String> = self
                .other_handoffs
                .iter()
                .take(OTHER_HANDOFFS_SHOWN)
                .map(|other| {
                    let at = crate::holder::when(other.updated_at);
                    match &other.held {
                        Some(held) => format!("{} on task {} ({at}, {held})", other.id, other.task),
                        None => format!("{} on task {} ({at})", other.id, other.task),
                    }
                })
                .collect();
            let more = self.other_handoffs.len().saturating_sub(OTHER_HANDOFFS_SHOWN);
            let more = if more > 0 { format!("; +{more} more") } else { String::new() };
            let _ = writeln!(out, "    Other handoffs in the last hour: {}{more}", named.join("; "));
        }

        // Questions before the work: the user's answers are what unblocks it.
        if !self.waiting_on_you.is_empty() {
            let _ = writeln!(out, "\nWaiting on you ({})", self.waiting_on_you.len());
            for asked in self.waiting_on_you.iter().take(WAITING_ON_YOU_SHOWN) {
                let about = asked.about.map(|task| format!(", about {task}")).unwrap_or_default();
                let moved = match asked.moved {
                    0 => String::new(),
                    1 => ", 1 write ago".to_string(),
                    n => format!(", {n} writes ago"),
                };
                let _ = writeln!(out, "{:>4}. [asked by {}{about}{moved}] {}", asked.id, asked.by, clip(&asked.text, NOTE_CLIP));
            }
            if let Some(more @ 1..) = self.waiting_on_you.len().checked_sub(WAITING_ON_YOU_SHOWN) {
                let _ = writeln!(out, "      +{more} more: ekko --sessions lists them by who asked");
            }
        }
        if !self.answered.is_empty() {
            let _ = writeln!(out, "\nAnswered, for this session ({})", self.answered.len());
            for asked in &self.answered {
                let (answer, by) = (asked.answer.as_deref().unwrap_or_default(), asked.answered_by.as_deref().unwrap_or("the user"));
                // The board moving in between is what makes an answer stale.
                let moved = match asked.moved {
                    0 => String::new(),
                    1 => ", 1 write after it was asked".to_string(),
                    n => format!(", {n} writes after it was asked"),
                };
                let _ =
                    writeln!(out, "{:>4}. {}\n      -> {} (recorded by {by}{moved})", asked.id, clip(&asked.text, TASK_CLIP), clip(answer, NOTE_CLIP));
            }
        }

        let sections = [
            Section::of_entries("In progress", &self.doing, usize::MAX, self.doing.len(), "context <id> reads one", usize::MAX, &today),
            Section::of_entries("Ready, best first", &self.ready, READY_WITH_NOTES, self.ready.len(), "next lists them", READY_KEPT, &today),
            Section::of_entries("Blocked", &self.blocked, 0, self.blocked_total, "search with the blocked filter", BLOCKED_KEPT, &today),
            Section::of_entries("Waiting", &self.waiting, 0, self.waiting_total, "search with the waiting filter", WAITING_KEPT, &today),
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
                reserved: false,
            },
            Section {
                heading: format!("\nGotchas and procedures ({})", self.knowledge_total),
                blocks: self.knowledge.iter().map(|note| (knowledge_line(note), Vec::new())).collect(),
                total: self.knowledge_total,
                rest: Some(KNOWLEDGE_REST),
                kept: KNOWLEDGE_SHOWN,
                reserved: true,
            },
        ];
        let shown = fit(&sections, &mut room);
        for (section, shown) in sections.iter().zip(&shown) {
            section.write(&mut out, shown);
        }
        out.push_str(&decisions);

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
            attention.push(format!("done work still waiting on open work: {}", capped(pairs, "; ", "")));
        }
        if !self.inversions.is_empty() {
            let pairs: Vec<String> = self
                .inversions
                .iter()
                .map(|i| format!("{} ({}) \u{21e0} {} ({})", i.blocked, i.blocked_phase, i.blocker, i.blocker_phase))
                .collect();
            attention.push(format!("dependencies against the phase order: {}", capped(pairs, "; ", "")));
        }
        if !self.overdue.is_empty() {
            let ids = self.overdue.iter().map(u32::to_string).collect();
            attention.push(format!("overdue: {}", capped(ids, ", ", ": search with the overdue filter")));
        }
        if !self.freed_by_cancelling.is_empty() {
            let pairs: Vec<String> =
                self.freed_by_cancelling.iter().map(|(task, blocker)| format!("{task} \u{21e0} {blocker}")).collect();
            attention.push(format!("ready only because what it waited on was cancelled: {}", capped(pairs, "; ", "")));
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
    /// Fitted ahead of every other section in the first pass, though written
    /// in its place: a few short lines a resume must not lose to a board the
    /// work already fills, the way the gotchas once fell off a full prime.
    reserved: bool,
}

impl Section {
    /// A section of entries best first, quoting the notes of the first
    /// `with_notes`, titled with `total`. The notes of work another running
    /// session holds are named, not quoted: they are that session's to read,
    /// and quoting them costs every other session the same characters.
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
                let notes = if at >= with_notes || entry.notes.is_empty() {
                    Vec::new()
                } else if entry.held.as_ref().is_some_and(Held::elsewhere) {
                    let ids: Vec<String> = entry.notes.iter().map(|note| note.id.to_string()).collect();
                    vec![format!("{:>10}notes {}: context {}", "", ids.join(", "), entry.id)]
                } else {
                    entry.notes.iter().map(|note| note_line(note, NOTE_CLIP)).collect()
                };
                (entry_line(entry, today), notes)
            })
            .collect();
        Section { heading: format!("\n{title} ({total})"), blocks, total, rest: Some(rest), kept, reserved: false }
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
/// alone when only it does. A reserved section goes first in the first pass.
/// When everything fits, everything shows, in order.
fn fit(sections: &[Section], room: &mut Room) -> Vec<Vec<bool>> {
    let cost = |line: &str| line.chars().count() + 1;
    let mut shown: Vec<Vec<bool>> = vec![Vec::new(); sections.len()];
    let mut order: Vec<usize> = (0..sections.len()).collect();
    for first in [true, false] {
        if first {
            order.sort_by_key(|&at| !sections[at].reserved);
        } else {
            order.sort_unstable();
        }
        for &at in &order {
            let (section, shown) = (&sections[at], &mut shown[at]);
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
            let _ = writeln!(out, "{}", listed_line(entry, &format!("[{}] {body}", listed_state(entry)), true, &today));
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
        self.text_among(detail, &[])
    }

    /// This item's block in a reply whose blocks are `order`, by id and in
    /// the order printed: a related item with a block of its own is pointed
    /// to rather than described again -- above all a note asked for beside
    /// its task, which a full read would otherwise print whole twice.
    fn text_among(&self, detail: Detail, order: &[u32]) -> String {
        let item = &self.item;
        let here = order.iter().position(|&id| id == item.id);
        let elsewhere = |id: u32| {
            let there = order.iter().position(|&other| other == id)?;
            (id != item.id).then_some(if Some(there) < here { "listed above" } else { "listed below" })
        };
        let mut out = String::new();
        let _ = writeln!(out, "{:>4}. {}", item.id, item.description);

        let mut facts = vec![match (item.state, item.handoff, item.knowledge) {
            (None, true, _) => "note, the handoff of the task it is attached to".to_string(),
            (None, false, Some(kind)) => format!("note, a {}", kind.word()),
            (None, false, None) => "note".to_string(),
            (Some(state), _, _) => format!("task, {}", state.word()),
        }];
        if let Some(held) = &item.held {
            facts[0] = format!("{}, {} since {}", facts[0], held.text(), crate::holder::when(held.since));
        }
        if let Some(asked) = &self.question {
            facts[0] = format!("note, a question asked by {}", asked.by);
        }
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
        if let Some(asked) = &self.question {
            let moved = |after: &str| match asked.moved {
                0 => String::new(),
                1 => format!(", 1 write {after}"),
                n => format!(", {n} writes {after}"),
            };
            let _ = match (&asked.answer, &asked.answered_by) {
                (Some(answer), Some(by)) => {
                    writeln!(out, "      answered, recorded by {by}{}: {answer}", moved("after it was asked"))
                }
                _ => writeln!(out, "      not answered yet{}", moved("since it was asked")),
            };
        }
        if let (Some((step, of)), false) = (item.step, self.sequence.is_empty()) {
            let mark = |state: &str| match state {
                "done" => "\u{2714}",
                "in progress" => "\u{25fc}",
                "cancelled" => "\u{2716}",
                _ => "\u{25fb}",
            };
            let steps: Vec<String> = self.sequence.iter().map(|link| format!("{} {}", mark(link.state), link.id)).collect();
            let _ = writeln!(out, "      step {step} of {of}: {}", steps.join(" \u{2192} "));
        }

        let links = |out: &mut String, title: &str, links: &[Link]| {
            if links.is_empty() {
                return;
            }
            let _ = writeln!(out, "\n{title}");
            for link in links {
                let told = elsewhere(link.id).unwrap_or(link.description.as_str());
                let _ = writeln!(out, "{:>4}. [{}] {told}", link.id, link.state);
            }
        };
        if let Some(task) = &self.attached_to {
            links(&mut out, "Attached to", std::slice::from_ref(task));
        }
        links(&mut out, "Superseded by, so no longer in force", &self.superseded_by);
        if let Some(older) = &self.supersedes {
            links(&mut out, "Supersedes", std::slice::from_ref(older));
        }
        links(&mut out, "Blocked by", &self.blockers);
        if self.waits_on > 0 {
            let _ = writeln!(out, "      waits on {}, directly or through others", open_tasks(self.waits_on));
        }
        for (heading, roots) in [("Roots, free to start", &self.roots), ("Roots, under way or held", &self.held_roots)] {
            if roots.is_empty() {
                continue;
            }
            let _ = writeln!(out, "\n{heading}");
            for root in roots {
                let told = if self.blockers.iter().any(|blocker| blocker.id == root.id) {
                    "listed above"
                } else {
                    elsewhere(root.id).unwrap_or(root.description.as_str())
                };
                let _ = writeln!(out, "{:>4}. [{}] {told}", root.id, root.state);
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
                let body = match (elsewhere(note.id), detail) {
                    (Some(place), _) => place.to_string(),
                    (None, Detail::Full) => note.description.clone(),
                    (None, Detail::Concise) => clip(&note.description, NOTE_CLIP),
                };
                let _ = writeln!(out, "{:>4}. {}{body}", note.id, note.mark());
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
        let dir = crate::paths::test_dir(&format!("ekko-agent-{tag}"));
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

    /// Work under way first, then urgency: an overdue p3 (12 + 6), a p3 alone
    /// (6), work that others wait on (1), then the rest. Blocked work is not
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
        assert_eq!(ids(&order), vec![7, 3, 2, 4, 1]);
        assert_eq!(order[3].unblocks, 2, "4 lets 5 move, and 6 after it");
        assert_eq!(ids(&next(&ekko, Some(2)).unwrap()), vec![7, 3]);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A low-priority prerequisite of urgent work inherits that urgency and
    /// leads -- the lexicographic ranking put it last (3, 4, 5, 1). Below it,
    /// priority comes before work that others wait on, which only orders tasks
    /// of one priority: a p1 holding up p1 work ranks after a p2, and after a
    /// p1 with any date, since a date two years out still weighs 0.2 x 12.
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
        assert_eq!(ids(&order), vec![1, 3, 4, 5]);
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

    /// A waiting task is in no list of work to take up -- not ready, not next,
    /// not blocked even when it is -- and has a section of its own, which a
    /// board that waits on nothing never shows. The task list Claude Code
    /// draws leaves it out with the rest.
    #[test]
    fn prime_lists_waiting_work_apart_from_the_work_to_take_up() {
        let (ekko, dir) = board("waiting");
        ekko.create_task(&words(&["ready"])).unwrap();
        ekko.create_task(&words(&["on the vendor", "p:2"])).unwrap();
        ekko.create_task(&words(&["on the vendor and on 1"])).unwrap();
        ekko.set_blocked_by(&words(&["@3", "1"])).unwrap();
        assert!(!prime(&ekko, "default board").unwrap().text().contains("Waiting"));

        ekko.set_state(&words(&["@2", "@3", "waiting"]), false).unwrap();
        let view = prime(&ekko, "default board").unwrap();
        assert_eq!((ids(&view.ready), ids(&view.blocked), ids(&view.waiting)), (vec![1], vec![], vec![2, 3]));
        assert_eq!(ids(&next(&ekko, None).unwrap()), vec![1]);

        let text = view.text();
        let section = text.split("\nWaiting (2)\n").nth(1).expect("a waiting section").split("\n\n").next().unwrap();
        assert_eq!(line_ids(section), vec![2, 3], "{text}");
        assert!(section.contains("\u{21e0} 1"), "a waiting task still names its blocker: {text}");
        assert!(text.contains("2 waiting \u{b7} 1 pending"), "{text}");

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

    /// A cursor past the board's revision -- counters.json went back -- is
    /// named as such, not blamed on a journal that still reaches it.
    #[test]
    fn changes_past_the_boards_revision_say_the_cursor_is_ahead() {
        let (ekko, dir) = board("ahead");
        ekko.create_task(&words(&["a task"])).unwrap();
        let cursor = prime(&ekko, "default board").unwrap().cursor;

        let seen = changes(&ekko, cursor + 5).unwrap();
        let text = seen.text();
        assert!(seen.ahead && !seen.complete, "{text}");
        assert!(text.contains(&format!("ahead of the board, whose revision is {cursor}")) && !text.contains("journal"), "{text}");
        assert!(!changes(&ekko, cursor).unwrap().ahead);

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

    /// A note read beside the task it explains has a block of its own, so
    /// the other block points to it rather than printing it whole a second
    /// time, and says which way to look.
    #[test]
    fn contexts_describe_each_item_once_in_a_reply() {
        let (ekko, dir) = board("once");
        let long = "a handoff long enough to cost something when printed twice ".repeat(10);
        ekko.create_task(&words(&["the task"])).unwrap();
        ekko.create_note(&words(&[long.as_str()])).unwrap();
        ekko.set_attached_to(&words(&["@2", "1"])).unwrap();
        let reply = |ids: &[&str]| contexts_text(&contexts(&ekko, &words(ids)).unwrap(), Detail::Full);

        let task_first = reply(&["1", "2"]);
        assert_eq!(task_first.matches(long.trim()).count(), 1, "{task_first}");
        assert!(task_first.contains("   2. listed below\n"), "{task_first}");
        assert!(task_first.contains("   1. [pending] listed above\n"), "{task_first}");

        let note_first = reply(&["2", "1"]);
        assert_eq!(note_first.matches(long.trim()).count(), 1, "{note_first}");
        assert!(note_first.contains("   1. [pending] listed below\n"), "{note_first}");
        assert!(note_first.contains("   2. listed above\n"), "{note_first}");

        let alone = reply(&["1"]);
        assert!(alone.contains(long.trim()), "a single read still prints its notes: {alone}");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A blocker in the trash holds nothing up -- the task it blocked can
    /// be completed -- so context says where it is instead of its state.
    #[test]
    fn context_names_a_trashed_blocker_as_in_the_trash() {
        let (ekko, dir) = board("trashed-blocker");
        ekko.create_task(&words(&["the blocker"])).unwrap();
        ekko.create_task(&words(&["the blocked"])).unwrap();
        ekko.set_blocked_by(&words(&["@2", "1"])).unwrap();
        ekko.set_trashed(&words(&["1"]), true).unwrap();

        let text = context(&ekko, "2").unwrap().text();
        assert!(text.contains("   1. [in the trash] the blocker\n"), "{text}");
        assert!(!text.contains("waits on"), "{text}");

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

    /// A cursor far behind gets the resume view rather than a list longer than
    /// it, and a cursor that little moved past gets the list.
    #[test]
    fn changes_past_the_budget_answer_with_the_prime() {
        let (ekko, dir) = board("changes-limit");
        let data: ItemMap = (1..=300)
            .map(|id| {
                let mut item = Item::new_task(id, format!("task {id}, with a description of the length tasks run to on a board"), vec!["My Board".to_string()], 1);
                item.rev = Some(u64::from(id));
                (id, item)
            })
            .collect();
        ekko.storage.set(&data).unwrap();
        let counters = crate::storage::Counters { revision: 300, highest_id: 300, ..Default::default() };
        ekko.storage.set_counters(&counters).unwrap();

        let far = changes_within(&ekko, 0, "default board").unwrap();
        assert!(far.starts_with("More moved since 0 than a list should hold"), "{far}");
        assert!(far.chars().count() <= PRIME_BUDGET + 200, "{} characters", far.chars().count());
        let near = changes_within(&ekko, 298, "default board").unwrap();
        assert!(near.starts_with("cursor 300 \u{b7} 2 changed since 298"), "{near}");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// "Needs attention" sits outside the budget, so each of its lines names
    /// a few items and counts the rest: 2,000 overdue tasks used to make an
    /// 11,000-character prime, past what the SessionStart hook keeps.
    #[test]
    fn needs_attention_stays_short_on_a_board_of_overdue_work() {
        let (ekko, dir) = board("overdue");
        let data: ItemMap = (1..=2_000)
            .map(|id| {
                let mut item = Item::new_task(id, format!("overdue task {id}"), vec!["My Board".to_string()], 1);
                item.due_date = Some("2020-01-01".to_string());
                (id, item)
            })
            .collect();
        ekko.storage.set(&data).unwrap();

        let text = prime(&ekko, "default board").unwrap().text();
        assert!(text.chars().count() <= PRIME_BUDGET, "{} characters:\n{text}", text.chars().count());
        assert!(text.contains("overdue: 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, +1990 more: search with the overdue filter"), "{text}");

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

    /// Under a handoff, the recent notes are the loose notes changed after it:
    /// what came before is history the handoff had the chance to take in.
    /// Without one, the newest are listed as always, and a note edited after
    /// the handoff counts as new.
    #[test]
    fn a_handoff_leaves_out_the_loose_notes_older_than_it() {
        let (ekko, dir) = board("handoff-notes");
        let tick = || std::thread::sleep(std::time::Duration::from_millis(3));
        ekko.create_task(&words(&["the work"])).unwrap();
        ekko.create_note(&words(&["written before the handoff"])).unwrap();
        ekko.create_note(&words(&["also before, edited after"])).unwrap();
        ekko.set_state(&words(&["@1", "progress"]), false).unwrap();
        let before = prime(&ekko, "default board").unwrap();
        assert_eq!(before.recent_notes.iter().map(|note| note.id).collect::<Vec<_>>(), vec![3, 2]);

        tick();
        hand_over(&ekko, 1, "Stopped after the parser.");
        tick();
        ekko.create_note(&words(&["written after the handoff"])).unwrap();
        ekko.edit_description(&words(&["@3", "also before, edited after it"])).unwrap();

        let text = prime(&ekko, "default board").unwrap().text();
        assert!(text.contains("\nRecent notes, not attached to a task\n   3. also before, edited after it\n   5. written after the handoff\n"), "{text}");
        assert!(!text.contains("written before the handoff"), "{text}");

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
        assert!(text.contains("any message, even just \"continue\", starts the next session"), "{text}");
        assert!(text.contains("write only that the next session asks them whether to do it"), "{text}");
        assert!(text.contains("Handoff 2 on this task was written under a minute ago"), "{text}");

        let by_note = handoff_prompt(&ekko, Some("2")).unwrap();
        assert!(by_note.contains("Write the handoff for task 1 now"), "a handoff's id stands for its task: {by_note}");
        ekko.create_note(&words(&["a loose thought"])).unwrap();
        assert!(matches!(handoff_prompt(&ekko, Some("3")), Err(EkkoError::InvalidInput(_))), "a loose note has nothing to hand over");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// With a fresh handoff on one task and another task left in progress,
    /// the prompt points at the fresh one rather than asking for a second.
    #[test]
    fn the_handoff_prompt_points_at_a_fresh_handoff_on_another_task() {
        let (ekko, dir) = board("handoff-prompt-fresh");
        ekko.create_task(&words(&["the task left in progress"])).unwrap();
        ekko.create_task(&words(&["the task this session worked on"])).unwrap();
        ekko.set_state(&words(&["@1", "progress"]), false).unwrap();
        hand_over(&ekko, 2, "stopped after the parser");

        let text = handoff_prompt(&ekko, None).unwrap();
        assert!(text.contains("Write the handoff for task 1 now"), "{text}");
        assert!(text.contains("Handoff 3 on task 2 was written under a minute ago"), "{text}");
        assert!(text.contains("edit append on 3"), "{text}");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Writes `ops` as one batch, the way an agent writes through MCP.
    fn write(ekko: &Ekko, ops: &[serde_json::Value]) {
        let mut draft = crate::ops::Draft::open(ekko).unwrap();
        for value in ops {
            let op: crate::ops::Op = serde_json::from_value(value.clone()).unwrap();
            draft.apply(&op).unwrap();
        }
        draft.commit(false).unwrap();
    }

    /// Gotchas and procedures in force are listed once, by first line and
    /// newest first; decisions in force are counted; a superseded note shows
    /// nowhere, and a typed note is not repeated among the loose notes or
    /// under its task.
    #[test]
    fn prime_lists_gotchas_and_procedures_counts_decisions_and_hides_the_superseded() {
        let (ekko, dir) = board("knowledge-prime");
        let note = |kind: &str, text: &str| serde_json::json!({"op": "create", "kind": kind, "text": text});
        write(
            &ekko,
            &[
                serde_json::json!({"op": "create", "text": "the work"}),
                serde_json::json!({"op": "create", "kind": "gotcha", "text": "Tab clears isComplete\nsecond line, only in context", "attached_to": 1}),
                note("procedure", "Release: bump, tag, push"),
                note("decision", "ship weekly"),
                serde_json::json!({"op": "create", "kind": "decision", "text": "ship on demand", "supersedes": 4}),
                note("gotcha", "the old trap"),
                serde_json::json!({"op": "create", "kind": "gotcha", "text": "the trap, restated", "supersedes": 6}),
                note("note", "a plain note"),
                serde_json::json!({"op": "create", "kind": "decision", "text": "crossterm, not ratatui", "attached_to": 1}),
            ],
        );
        ekko.set_state(&words(&["@1", "progress"]), false).unwrap();

        let text = prime(&ekko, "default board").unwrap().text();
        let listed = "\nGotchas and procedures (3)\n   7. [gotcha] the trap, restated\n   3. [procedure] Release: bump, tag, push\n   2. [gotcha] Tab clears isComplete\n";
        assert!(text.contains(listed), "{text}");
        assert!(text.contains("\nDecisions (2): search with the decision filter\n"), "{text}");
        assert!(text.contains("       9. [decision] crossterm, not ratatui"), "a decision still explains its task: {text}");
        for gone in ["second line", "the old trap", "ship weekly", "ship on demand"] {
            assert!(!text.contains(gone), "{gone}: {text}");
        }
        assert_eq!(line_ids(&text).iter().filter(|id| **id == 2).count(), 1, "listed once: {text}");
        assert!(text.contains("\nRecent notes, not attached to a task\n   8. a plain note\n\n"), "{text}");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// On a board the work already fills, typed notes still fit the prime's
    /// budget: the section keeps its first lines and says where the rest are,
    /// and the decisions line is always there.
    #[test]
    fn typed_notes_fit_a_prime_the_work_already_fills() {
        let (ekko, dir) = board("knowledge-budget");
        let reason = "a reason that runs long enough to matter ".repeat(7);
        let mut ops = Vec::new();
        for k in 1..=80 {
            let text = format!("ready task number {k}, described at the length real tasks on a board run to, so a listing fills");
            ops.push(serde_json::json!({"op": "create", "text": text}));
        }
        for k in 1..=80 {
            ops.push(serde_json::json!({"op": "create", "kind": "note", "text": reason, "attached_to": k}));
        }
        for k in 1..=12 {
            let kind = if k % 2 == 0 { "gotcha" } else { "procedure" };
            let text = format!("lesson {k}: {}", "a first line long enough to be clipped ".repeat(6));
            ops.push(serde_json::json!({"op": "create", "kind": kind, "text": text}));
        }
        for k in 1..=20 {
            ops.push(serde_json::json!({"op": "create", "kind": "decision", "text": format!("decision {k}")}));
        }
        write(&ekko, &ops);

        let text = prime(&ekko, "default board").unwrap().text();
        assert!(text.chars().count() <= PRIME_BUDGET, "{} characters", text.chars().count());
        assert!(text.contains("more: next lists them"), "the work filled it: {text}");
        assert!(text.contains("\nGotchas and procedures (12)\n"), "{text}");
        assert!(text.contains(&format!("      +7 more: {KNOWLEDGE_REST}\n")), "{text}");
        assert!(text.contains("\nDecisions (20): search with the decision filter\n"), "{text}");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Work in progress with long notes fills a prime without a cap of its
    /// own; the gotchas are still listed, and a cut says what it left out.
    #[test]
    fn gotchas_survive_a_prime_filled_by_work_in_progress() {
        let (ekko, dir) = board("knowledge-reserved");
        let reason = "a reason that runs long enough to matter ".repeat(7);
        let mut ops = Vec::new();
        for k in 1..=40 {
            ops.push(serde_json::json!({"op": "create", "text": format!("task {k} in progress")}));
        }
        for k in 1..=40 {
            for _ in 0..3 {
                ops.push(serde_json::json!({"op": "create", "kind": "note", "text": reason, "attached_to": k}));
            }
        }
        ops.push(serde_json::json!({"op": "create", "kind": "gotcha", "text": "restart before writing after an upgrade"}));
        ops.push(serde_json::json!({"op": "create", "kind": "procedure", "text": "release in the order note 184 gives"}));
        write(&ekko, &ops);
        for k in 1..=40 {
            ekko.set_state(&words(&[format!("@{k}").as_str(), "progress"]), false).unwrap();
        }

        let text = prime(&ekko, "default board").unwrap().text();
        assert!(text.chars().count() <= PRIME_BUDGET, "{} characters", text.chars().count());
        assert!(text.contains("more: context <id> reads one"), "the work in progress filled it: {text}");
        assert!(text.contains("\nGotchas and procedures (2)\n"), "{text}");
        assert!(text.contains("restart before writing after an upgrade"), "{text}");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A board with no typed notes gets no trace of them in its prime.
    #[test]
    fn a_prime_without_typed_notes_says_nothing_of_them() {
        let (ekko, dir) = board("knowledge-none");
        ekko.create_task(&words(&["a task"])).unwrap();
        ekko.create_note(&words(&["a note"])).unwrap();

        let text = prime(&ekko, "default board").unwrap().text();
        assert!(!text.contains("Gotchas") && !text.contains("Decisions"), "{text}");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// context names a note's kind and both ends of a supersession; trashing
    /// the newer note puts the older one back in force.
    #[test]
    fn context_names_the_kind_and_both_ends_of_a_supersession() {
        let (ekko, dir) = board("knowledge-context");
        write(
            &ekko,
            &[
                serde_json::json!({"op": "create", "kind": "decision", "text": "ship weekly"}),
                serde_json::json!({"op": "create", "kind": "decision", "text": "ship on demand", "supersedes": 1}),
            ],
        );

        let older = context(&ekko, "1").unwrap().text();
        assert!(older.contains("      note, a decision \u{b7} "), "{older}");
        assert!(older.contains("\nSuperseded by, so no longer in force\n   2. [decision] ship on demand\n"), "{older}");
        let newer = context(&ekko, "2").unwrap().text();
        assert!(newer.contains("\nSupersedes\n   1. [decision] ship weekly\n"), "{newer}");

        ekko.set_trashed(&words(&["2"]), true).unwrap();
        let older = context(&ekko, "1").unwrap().text();
        assert!(!older.contains("Superseded by"), "{older}");
        assert!(prime(&ekko, "default board").unwrap().text().contains("Decisions (1)"));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// search finds notes by kind, superseded ones included and marked, so
    /// the history is there and never reads as current.
    #[test]
    fn search_finds_notes_by_kind_and_marks_the_superseded() {
        let (ekko, dir) = board("knowledge-search");
        write(
            &ekko,
            &[
                serde_json::json!({"op": "create", "kind": "decision", "text": "ship weekly"}),
                serde_json::json!({"op": "create", "kind": "decision", "text": "ship on demand", "supersedes": 1}),
                serde_json::json!({"op": "create", "kind": "gotcha", "text": "a trap"}),
                serde_json::json!({"op": "create", "text": "a task"}),
            ],
        );

        let decisions = search(&ekko, None, &words(&["decision"]), 20).unwrap();
        assert_eq!(decisions.hits.iter().map(|(entry, _)| entry.id).collect::<Vec<_>>(), vec![1, 2]);
        let text = decisions.text();
        assert!(text.contains("   1. [decision, superseded by 2] ship weekly"), "{text}");
        assert!(text.contains("   2. [decision] ship on demand"), "{text}");
        let gotchas = search(&ekko, Some("trap"), &words(&["gotchas"]), 20).unwrap();
        assert_eq!(gotchas.hits.iter().map(|(entry, _)| entry.id).collect::<Vec<_>>(), vec![3]);

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

    /// Work in progress says who holds it, as the reader sees it: its own,
    /// another session's while that one runs, or one that is gone.
    #[test]
    fn work_in_progress_says_who_holds_it() {
        let (me, other, gone) = crate::holder::test_sessions();
        let (_, dir) = board("held-view");
        let as_ = |actor: &crate::holder::Actor| Ekko::new(Storage::new(&dir).unwrap()).acting_as(actor.clone());
        for (actor, name) in [(&me, "mine"), (&other, "theirs"), (&gone, "abandoned")] {
            as_(actor).create_task(&words(&[name])).unwrap();
        }
        for (actor, id) in [(&me, "@1"), (&other, "@2"), (&gone, "@3")] {
            as_(actor).set_state(&words(&[id, "progress"]), false).unwrap();
        }

        let text = prime(&as_(&me), "default board").unwrap().text();
        assert!(text.contains("   1. mine \u{b7} in progress \u{b7} yours\n"), "{text}");
        assert!(text.contains("   2. theirs \u{b7} in progress \u{b7} held by default on pts/2\n"), "{text}");
        assert!(text.contains("   3. abandoned \u{b7} in progress \u{b7} held by default on pts/3, gone\n"), "{text}");
        let held = next(&as_(&me), None).unwrap().into_iter().filter_map(|entry| entry.held).collect::<Vec<_>>();
        assert_eq!(held.iter().map(Held::elsewhere).collect::<Vec<_>>(), vec![false, true, false]);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// With two sessions handing off on one board, each prime resumes from
    /// its own session's handoff, newer or not, and names the other's.
    #[test]
    fn each_session_resumes_from_its_own_handoff() {
        let (me, other, _) = crate::holder::test_sessions();
        let (_, dir) = board("handoffs");
        let as_ = |actor: &crate::holder::Actor| Ekko::new(Storage::new(&dir).unwrap()).acting_as(actor.clone());
        let write = |ekko: &Ekko, op: serde_json::Value| {
            let mut draft = crate::ops::Draft::open(ekko).unwrap();
            draft.apply(&serde_json::from_value(op).unwrap()).unwrap();
            draft.commit(false).unwrap();
        };
        for (actor, name) in [(&me, "mine"), (&other, "theirs")] {
            as_(actor).create_task(&words(&[name])).unwrap();
        }
        as_(&me).set_state(&words(&["@1", "progress"]), false).unwrap();
        as_(&other).set_state(&words(&["@2", "progress"]), false).unwrap();
        write(&as_(&me), serde_json::json!({"op": "create", "kind": "handoff", "text": "my stop", "attached_to": 1}));
        write(&as_(&other), serde_json::json!({"op": "create", "kind": "handoff", "text": "their stop", "attached_to": 2}));

        let mine = prime(&as_(&me), "default board").unwrap().text();
        assert!(mine.contains("Where the last session stopped: handoff 3 on task 1 [in progress]"), "{mine}");
        assert!(mine.contains("Other handoffs in the last hour: 4 on task 2 ("), "{mine}");
        assert!(mine.contains(", held by default on pts/2)"), "{mine}");
        let theirs = prime(&as_(&other), "default board").unwrap().text();
        assert!(theirs.contains("Where the last session stopped: handoff 4 on task 2 [in progress]"), "{theirs}");
        assert!(theirs.contains("Other handoffs in the last hour: 3 on task 1 ("), "{theirs}");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The notes of work another running session holds are named, not
    /// quoted: that session reads them, and every other session would pay for
    /// them in its prime. Seen on 2026-09-22, where a second session's prime
    /// quoted the first one's handoff under its task, besides naming it among
    /// the other handoffs.
    #[test]
    fn the_prime_names_the_notes_of_work_another_session_holds() {
        let (me, other, _) = crate::holder::test_sessions();
        let (_, dir) = board("held-notes");
        let as_ = |actor: &crate::holder::Actor| Ekko::new(Storage::new(&dir).unwrap()).acting_as(actor.clone());
        let write = |ekko: &Ekko, op: serde_json::Value| {
            let mut draft = crate::ops::Draft::open(ekko).unwrap();
            draft.apply(&serde_json::from_value(op).unwrap()).unwrap();
            draft.commit(false).unwrap();
        };
        for (actor, name) in [(&me, "mine"), (&other, "theirs")] {
            as_(actor).create_task(&words(&[name])).unwrap();
        }
        as_(&me).set_state(&words(&["@1", "progress"]), false).unwrap();
        as_(&other).set_state(&words(&["@2", "progress"]), false).unwrap();
        for (task, text) in [(1, "why mine"), (2, "why theirs"), (2, "and more")] {
            write(&as_(&me), serde_json::json!({"op": "create", "kind": "note", "text": text, "attached_to": task}));
        }

        let mine = prime(&as_(&me), "default board").unwrap().text();
        assert!(mine.contains("3. why mine\n"), "{mine}");
        assert!(mine.contains("\n          notes 4, 5: context 2\n") && !mine.contains("why theirs"), "{mine}");
        let theirs = prime(&as_(&other), "default board").unwrap().text();
        assert!(theirs.contains("4. why theirs\n") && theirs.contains("\n          notes 3: context 1\n"), "{theirs}");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The SessionStart hook records the conversation its session runs, so a
    /// claim names it, and after a /clear the name moves on to the new one --
    /// the conversation to resume, where the claim's own would resume the
    /// work as it stood before the /clear.
    #[test]
    fn a_claim_names_the_conversation_its_session_runs() {
        let (me, other, _) = crate::holder::test_sessions();
        let (_, dir) = board("conversations");
        let registry = crate::holder::Registry::at(dir.join("processes"));
        let as_ = |actor: &crate::holder::Actor| {
            Ekko::new(Storage::new(&dir).unwrap()).acting_as(actor.clone().with_registry(registry.clone()))
        };
        let start = |actor: &crate::holder::Actor, source: &str, conversation: &str| {
            let event = SessionEvent { source: source.to_string(), session_id: Some(conversation.to_string()) };
            session_start(&as_(actor), "default board", &event, &dir.join("sessions")).unwrap();
        };
        start(&me, "startup", "c1-mine");
        start(&other, "startup", "c2-theirs");
        as_(&me).create_task(&words(&["mine"])).unwrap();
        as_(&me).set_state(&words(&["@1", "progress"]), false).unwrap();
        let claim = as_(&me).storage.get().unwrap()[&1].held_by.clone().unwrap();
        assert_eq!(claim.conversation.as_deref(), Some("c1-mine"));

        let theirs = prime(&as_(&other), "default board").unwrap().text();
        assert!(theirs.contains("held by default on pts/1 \u{b7} c1-mine"), "{theirs}");
        start(&me, "clear", "c3-after-clear");
        let theirs = prime(&as_(&other), "default board").unwrap().text();
        assert!(theirs.contains("held by default on pts/1 \u{b7} c3-after\n"), "{theirs}");
        let refused = as_(&other).set_state(&words(&["@1", "paused"]), false).unwrap_err().to_string();
        assert!(refused.contains("default on pts/1 \u{b7} c3-after, since"), "{refused}");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A conversation resumed in a new process takes back, at SessionStart,
    /// what it held in the process that ended, and says so. Another
    /// conversation does not, and a holder that still runs keeps its task.
    #[test]
    fn a_resumed_conversation_takes_back_what_it_held() {
        let (me, other, gone) = crate::holder::test_sessions();
        let (_, dir) = board("take-back");
        let registry = crate::holder::Registry::at(dir.join("processes"));
        let as_ = |actor: &crate::holder::Actor| {
            Ekko::new(Storage::new(&dir).unwrap()).acting_as(actor.clone().with_registry(registry.clone()))
        };
        let start = |actor: &crate::holder::Actor, source: &str, conversation: &str| {
            let event = SessionEvent { source: source.to_string(), session_id: Some(conversation.to_string()) };
            session_start(&as_(actor), "default board", &event, &dir.join("sessions")).unwrap()
        };
        start(&gone, "startup", "c-before-the-restart");
        start(&other, "startup", "c-other");
        for (actor, name, id) in [(&gone, "held before the restart", "@1"), (&other, "held elsewhere", "@2")] {
            as_(actor).create_task(&words(&[name])).unwrap();
            as_(actor).set_state(&words(&[id, "progress"]), false).unwrap();
        }

        let unrelated = start(&me, "resume", "c-unrelated");
        assert!(!unrelated.contains("yours again"), "{unrelated}");
        let resumed = start(&me, "resume", "c-before-the-restart");
        assert!(
            resumed.starts_with("1 was held by this conversation until its process ended (default on pts/3 \u{b7} c-before): yours again\n"),
            "{resumed}"
        );
        let data = as_(&me).storage.get().unwrap();
        let claim = data[&1].held_by.as_ref().unwrap();
        assert!(me.is(claim) && claim.conversation.as_deref() == Some("c-before-the-restart"));
        assert!(other.is(data[&2].held_by.as_ref().unwrap()), "a holder that still runs keeps its task");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A question waits on the board, in every prime, until the user answers
    /// it -- here from the terminal -- and the answer then reaches the
    /// conversation that asked, resumed in a new process after a restart.
    /// Seen on 2026-09-22: four questions held only in a prompt were lost at
    /// a restart, one of them already stale.
    #[test]
    fn a_question_waits_on_the_board_and_its_answer_reaches_who_asked() {
        let (me, other, gone) = crate::holder::test_sessions();
        let (_, dir) = board("prime-questions");
        let registry = crate::holder::Registry::at(dir.join("processes"));
        let as_ = |actor: &crate::holder::Actor| {
            Ekko::new(Storage::new(&dir).unwrap()).acting_as(actor.clone().with_registry(registry.clone()))
        };
        let start = |actor: &crate::holder::Actor, conversation: &str| {
            let event = SessionEvent { source: "resume".to_string(), session_id: Some(conversation.to_string()) };
            session_start(&as_(actor), "default board", &event, &dir.join("sessions")).unwrap()
        };
        let _ = start(&gone, "c-asker");
        as_(&gone).create_task(&words(&["the merge"])).unwrap();
        let asker = as_(&gone);
        let mut draft = crate::ops::Draft::open(&asker).unwrap();
        let asked = draft.ask("Merge auditoria now?", Some(&crate::ops::Ref::Id(1))).unwrap();
        draft.commit(false).unwrap();
        as_(&other).create_task(&words(&["later work"])).unwrap();

        let theirs = prime(&as_(&other), "default board").unwrap().text();
        let waiting = format!("\nWaiting on you (1)\n   {asked}. [asked by default on pts/3 \u{b7} c-asker, about 1, 1 write ago] Merge auditoria now?\n");
        assert!(theirs.contains(&waiting), "{theirs}");
        assert!(!theirs.contains("[question]"), "an open question is not quoted again under its task: {theirs}");

        as_(&crate::holder::Actor::person()).answer_question(&words(&[&asked.to_string(), "yes,", "after", "the", "rebase"])).unwrap();
        let resumed = start(&me, "c-asker");
        assert!(resumed.contains(&format!("{asked}. [answered] Merge auditoria now? -> yes, after the rebase\n")), "what moved names the answer: {resumed}");
        let mine = prime(&as_(&me), "default board").unwrap().text();
        let answered = format!(
            "\nAnswered, for this session (1)\n   {asked}. Merge auditoria now?\n      -> yes, after the rebase (recorded by the user, 1 write after it was asked)\n"
        );
        assert!(mine.contains(&answered) && !mine.contains("Waiting on you"), "{mine}");
        let theirs = prime(&as_(&other), "default board").unwrap().text();
        assert!(!theirs.contains("Answered, for this session") && theirs.contains("[answered] Merge auditoria now?"), "{theirs}");

        let read = contexts(&as_(&me), &[asked.to_string()]).unwrap()[0].text();
        assert!(read.contains("note, a question asked by default on pts/3 \u{b7} c-asker"), "{read}");
        assert!(read.contains("answered, recorded by the user, 1 write after it was asked: yes, after the rebase"), "{read}");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// `ekko --sessions` shows the user each session running on the board,
    /// an idle one too, and each that ended while it still holds work here,
    /// with what it holds, finished today and asked, and how to resume it.
    /// The user's ask of 2026-09-22 (task 370): every session shows the same
    /// board, and nothing said which work was whose.
    #[test]
    fn sessions_shows_each_session_on_the_board_and_its_work() {
        let (me, other, gone) = crate::holder::test_sessions();
        let (_, dir) = board("sessions");
        let registry = crate::holder::Registry::at(dir.join("processes"));
        let as_ = |actor: &crate::holder::Actor| {
            Ekko::new(Storage::new(&dir).unwrap()).acting_as(actor.clone().with_registry(registry.clone()))
        };
        for (actor, conversation) in [(&me, "c-mine"), (&other, "c-idle"), (&gone, "c-gone")] {
            let event = SessionEvent { source: "startup".to_string(), session_id: Some(conversation.to_string()) };
            session_start(&as_(actor), "default board", &event, &dir.join("sessions")).unwrap();
        }
        for name in ["mine", "finished", "left behind"] {
            as_(&me).create_task(&words(&[name])).unwrap();
        }
        as_(&me).set_state(&words(&["@1", "progress"]), false).unwrap();
        as_(&me).set_state(&words(&["@2", "done"]), false).unwrap();
        as_(&gone).set_state(&words(&["@3", "progress"]), false).unwrap();
        let mine = as_(&me);
        let mut draft = crate::ops::Draft::open(&mine).unwrap();
        draft.ask("Merge now?", Some(&crate::ops::Ref::Id(1))).unwrap();
        draft.commit(false).unwrap();

        let text = sessions(&mine, "default board").unwrap().text();
        assert!(text.starts_with("ekko \u{b7} default board \u{b7} 2 running \u{b7} 1 ended with work here\n"), "{text}");
        let own = "      in progress    1. mine\n      done today     2. finished\n      asking         4. Merge now?\n";
        assert!(text.contains("\ndefault on pts/1 \u{b7} c-mine \u{b7} since ") && text.contains(own), "{text}");
        assert!(text.contains("\ndefault on pts/2 \u{b7} c-idle \u{b7} since ") && text.contains("      nothing in progress\n"), "{text}");
        assert!(text.contains("\ndefault on pts/3 \u{b7} c-gone \u{b7} ended\n      in progress    3. left behind\n"), "{text}");
        assert!(text.contains("claude --resume c-gone\n"), "{text}");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Tasks linked by blocked_by into one line are a sequence of steps, and
    /// every view says where each stands: 'step 2 of 3' in a listing, the
    /// whole line in context, '(2/3)' in the task list, 'step 2/3' on the
    /// board. A branch ends the line. The user's ask of 2026-09-22: a
    /// sequence of steps, and seeing where it stands.
    #[test]
    fn a_line_of_blocked_tasks_is_a_sequence_of_steps() {
        let (me, _, _) = crate::holder::test_sessions();
        let (_, dir) = board("sequence");
        let ekko = Ekko::new(Storage::new(&dir).unwrap()).acting_as(me);
        for name in ["plan", "build", "ship", "aside"] {
            ekko.create_task(&words(&[name])).unwrap();
        }
        ekko.set_blocked_by(&words(&["@2", "1"])).unwrap();
        ekko.set_blocked_by(&words(&["@3", "2"])).unwrap();
        ekko.set_state(&words(&["@1", "done"]), false).unwrap();
        ekko.set_state(&words(&["@2", "progress"]), false).unwrap();

        let text = prime(&ekko, "default board").unwrap().text();
        assert!(text.contains("   2. build \u{b7} in progress \u{b7} yours \u{b7} step 2 of 3 \u{b7} unblocks 1\n"), "{text}");
        let read = contexts(&ekko, &["3".to_string()]).unwrap()[0].text();
        assert!(read.contains("      step 3 of 3: \u{2714} 1 \u{2192} \u{25fc} 2 \u{2192} \u{25fb} 3\n"), "{read}");
        let list = crate::tasklist::tasks(&ekko, 0).unwrap();
        let drawn: Vec<(&str, &str)> = list.iter().map(|task| (task.status.as_str(), task.subject.as_str())).collect();
        assert_eq!(
            drawn[..3],
            [("completed", "1. (1/3) plan"), ("in_progress", "2. (2/3) build"), ("pending", "3. (3/3) ship")],
            "the whole way, the step still blocked included"
        );
        assert_eq!(steps(&ekko).unwrap().get(&4), None, "a task on no line is no step");

        ekko.set_blocked_by(&words(&["@4", "2"])).unwrap();
        let steps = steps(&ekko).unwrap();
        assert_eq!((steps.get(&2), steps.get(&3)), (Some(&(2, 2)), None), "2 holds up two now: the line ends there");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A root that `next` would not offer -- in progress, waiting or stashed
    /// -- is not called free to start: it is listed apart, as what still
    /// holds the item up.
    #[test]
    fn context_lists_roots_that_cannot_be_started_apart() {
        let (ekko, dir) = board("held-roots");
        for name in ["stashed root", "waiting root", "root under way", "free root", "tip"] {
            ekko.create_task(&words(&[name])).unwrap();
        }
        ekko.set_blocked_by(&words(&["@5", "1", "2", "3", "4"])).unwrap();
        ekko.set_stashed(&words(&["1"]), true).unwrap();
        ekko.set_state(&words(&["@2", "waiting"]), false).unwrap();
        ekko.set_state(&words(&["@3", "progress"]), false).unwrap();

        let tip = context(&ekko, "5").unwrap();
        assert_eq!(tip.roots.iter().map(|root| root.id).collect::<Vec<_>>(), vec![4]);
        assert_eq!(tip.held_roots.iter().map(|root| (root.id, root.state)).collect::<Vec<_>>(), vec![(1, "stashed"), (2, "waiting"), (3, "in progress")]);
        let text = tip.text();
        assert!(text.contains("\nRoots, free to start\n   4. [pending] listed above\n"), "{text}");
        assert!(text.contains("\nRoots, under way or held\n   1. [stashed] listed above\n"), "{text}");

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
