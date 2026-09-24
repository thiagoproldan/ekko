//! The task/note data model.
//!
//! JS taskbook modeled this as `Item` (base) with `Task`/`Note` subclasses.
//! Rust has no classical inheritance, and more importantly the two are
//! serialized to the *same* flat JSON shape on disk (a `Note` simply never
//! has `isComplete`/`inProgress`/`priority` at all, rather than having them
//! set to some "N/A" value) — so a single struct with `Option`al task-only
//! fields matches the wire format exactly, which matters: this needs to
//! read `storage.json`/`archive.json` files an existing JS install already
//! produced, unchanged.

use std::collections::BTreeMap;
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::Local;
use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Item {
    #[serde(rename = "_id")]
    pub id: u32,
    #[serde(rename = "_date")]
    pub date: String,
    #[serde(rename = "_timestamp")]
    pub timestamp: i64,
    pub description: String,
    #[serde(rename = "isStarred", default)]
    pub is_starred: bool,
    #[serde(default)]
    pub boards: Vec<String>,
    #[serde(rename = "_isTask")]
    pub is_task: bool,

    // Task-only. `None` for notes, and omitted from the JSON entirely in
    // that case -- matching how the JS `Note` class never set these at all,
    // rather than setting them to some placeholder value.
    #[serde(rename = "isComplete", default, skip_serializing_if = "Option::is_none")]
    pub is_complete: Option<bool>,
    #[serde(rename = "inProgress", default, skip_serializing_if = "Option::is_none")]
    pub in_progress: Option<bool>,
    // Task-only, and absent from the JSON when unset -- so a file
    // written by taskbook, or by an Ekko that never saw a `d:` token,
    // round-trips byte-identically through this field.
    #[serde(rename = "dueDate", default, skip_serializing_if = "Option::is_none")]
    pub due_date: Option<String>,
    /// Stable across everything the display id is not: it survives
    /// `--restore`, which hands the item a fresh `_id`. Display ids are no
    /// longer recycled once an item leaves storage (see `Counters`), but a
    /// restore still renumbers, so callers that hold a reference across time
    /// should hold this.
    ///
    /// `Option` because items written before this existed -- and any
    /// written by taskbook -- do not have one, and backfilling would
    /// rewrite files that are otherwise untouched. Absent means "legacy",
    /// not "unknown".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
    /// When this item last changed, in epoch milliseconds. Distinct from
    /// `_timestamp`, which is when it was *created* and never moves --
    /// that difference is the whole reason `--since` needs its own field.
    ///
    /// `Option` for the same reason `uid` is: items written before this
    /// existed do not have one, and inventing values for them would be
    /// lying about when they changed.
    #[serde(rename = "updatedAt", default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<i64>,
    /// The board revision of the write that last changed this item: a counter
    /// every write raises by one, kept in counters.json beside storage.json.
    /// It is what `changes` compares a cursor against, because a clock makes a
    /// poor cursor -- two writes in one millisecond tie, and a cursor equal to
    /// the newest `updatedAt` hands that item back on every call.
    ///
    /// `Option` like `updatedAt`: an item no write has touched since this
    /// existed has none, and backfilling would rewrite untouched files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<u64>,
    /// Set aside after having been started, as opposed to never started at
    /// all -- two situations taskbook collapsed into the same empty box,
    /// because it modelled "paused" as the absence of in-progress rather
    /// than a state of its own.
    ///
    /// `Option`, and omitted when unset, so a board nobody ever pauses --
    /// and anything taskbook wrote -- is byte-identical to before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused: Option<bool>,
    /// Abandoned on purpose, and kept. Distinct from deleting it, which
    /// loses why it was dropped -- often the part worth having later.
    ///
    /// `Option`, omitted when unset, so a board that cancels nothing is
    /// byte-identical to before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancelled: Option<bool>,
    /// Cannot move until something outside the board happens -- a reply, a
    /// release, a date. Unlike paused, which is set aside by choice and can
    /// be taken up any time, a waiting task is not ready, whatever its
    /// dependencies say.
    ///
    /// `Option`, omitted when unset, so a board that waits on nothing is
    /// byte-identical to before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting: Option<bool>,
    /// Who works a task in progress: the Claude Code process that set it in
    /// progress, or the person, so another session can tell it is taken.
    /// Only a task in progress holds one, and leaving progress lets it go;
    /// see `crate::holder`. Absent unless set, so a board nobody claims work
    /// on is stored exactly as before.
    #[serde(rename = "heldBy", default, skip_serializing_if = "Option::is_none")]
    pub held_by: Option<crate::holder::Holder>,
    /// Whose work a done task was: the Claude Code session that held it when
    /// it was done, or else the session that did it -- so a session's task
    /// list checks off what it finished, not what another session finished
    /// meanwhile. Set as the task is done and cleared if it is reopened; a
    /// person's own work records none, so a board nobody claims work on is
    /// stored exactly as before.
    #[serde(rename = "doneBy", default, skip_serializing_if = "Option::is_none")]
    pub done_by: Option<crate::holder::Holder>,
    /// Who wrote the item (task 125): the Claude Code session that created
    /// it -- its process, profile, terminal and the conversation it ran then
    /// -- or, with no process, the person at the terminal. Set once, as the
    /// item is created, and kept by every later write, a restore included.
    /// Absent on an item written before it was recorded, whose author is not
    /// known.
    #[serde(rename = "createdBy", default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<crate::holder::Holder>,
    /// Who a task is with: a person or a party outside the sessions -- the
    /// user, a colleague, a client -- by a name of one word, stored in lower
    /// case. Unlike `held_by`, which a session takes by starting the work,
    /// this is said before anyone starts, and stays until someone changes it.
    ///
    /// A task with nobody is anyone's, which on a board an agent works is the
    /// agent's; one with someone leaves the agent's queue -- next, the prime's
    /// ready work, the task list -- and is listed apart, by name. The other
    /// way round from marking the agent's work, so a board that never names
    /// anyone reads as it always did and an agent never has to mark the work
    /// it gives itself. Absent unless set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub with: Option<String>,
    /// Which phase of a project this belongs to, when it belongs to one.
    ///
    /// `None` means the project root -- outside the roadmap, and the only shape
    /// the default board ever has. Areas are scoped by phase, so `@render`
    /// under `setup` and `@render` under `compositor` are two distinct
    /// areas; this field is what tells them apart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    /// Items blocking this one, by `uid`. While any of them is open, this
    /// task cannot be completed short of `--force`.
    ///
    /// By uid and not by display id: ids are recycled, so a dependency
    /// stored as `3` would silently start pointing at a different item the
    /// moment the original was deleted and the number reused. That is the
    /// exact hazard `uid` was added for.
    ///
    /// A blocker that no longer exists, or sits in the trash, does not
    /// block -- deleting it is a way to unblock, and the alternative is an
    /// item stuck forever on something nobody can finish. A stashed one
    /// still does: stashing hides an item, it does not finish it.
    #[serde(rename = "blockedBy", default, skip_serializing_if = "Option::is_none")]
    pub blocked_by: Option<Vec<String>>,
    /// When this was put away, in epoch millis.
    ///
    /// A timestamp rather than a flag so the stash view can say how long
    /// something has been sitting there -- which is most of what tells you
    /// whether you are ever going back for it.
    ///
    /// Stashing hides an item; it does not change what the item *is*. A
    /// stashed done task is still done, and comes back done. That is why
    /// this is a separate field rather than another `state`: a state would
    /// have to overwrite the one it found, and unstashing would then have
    /// to guess.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stashed: Option<i64>,
    /// When this was thrown away, in epoch millis.
    ///
    /// Also a timestamp, and for a sharper reason: the trash expires, so
    /// the moment it went in is the only thing that can say when it goes
    /// out. A boolean could never answer "how long do I have".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trashed: Option<i64>,
    /// The task this note is attached to, by `uid`.
    ///
    /// Only a note carries one, and it only ever points at a task. Both
    /// restrictions keep the feature to one level: a task under a task
    /// would be a subtask, which raises questions about whose total it
    /// counts toward, and a note under a note would allow chains and
    /// therefore cycles. Neither is what this is for.
    ///
    /// By uid for the same reason `blocked_by` is: display ids are
    /// recycled, and a reason pointing at a recycled number would end up
    /// explaining a different piece of work.
    ///
    /// Stored as `attachedTo`, and read from `anchor` as well, which is what
    /// it was called before `--anchor` became `--attached-to`. Boards
    /// attached before the rename keep their attachments with nobody
    /// migrating anything; the next write stores the new name.
    #[serde(rename = "attachedTo", alias = "anchor", default, skip_serializing_if = "Option::is_none")]
    pub attached_to: Option<String>,
    /// Marks a note as the handoff of the task it is attached to: where the
    /// last session stopped, what it decided and why, the files it touched
    /// and the next step -- what a new session reads under the prime instead
    /// of a transcript it would pay to re-read.
    ///
    /// Only a note attached to an open task carries it, and only the newest
    /// on a task: writing another demotes the one before to an ordinary note,
    /// which keeps it as history. A flag on the note rather than a board
    /// named `handoff`: a board is a name anyone can pick, move an item off,
    /// or already use for something else, and none of those should silently
    /// change what a session resumes from. Absent unless set, so a board
    /// without handoffs is stored exactly as before.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub handoff: bool,
    /// What a note records that stays true after the work around it is done:
    /// a decision (what was settled, and why), a gotcha (a trap, and how to
    /// avoid it) or a procedure (steps that work). Written on purpose, by the
    /// user or an agent, never captured: the durable half of what a session
    /// learns, where a handoff is the half that expires.
    ///
    /// Apart from `handoff` because the two age differently: a handoff is
    /// demoted by the next one, and a decision stays one until another
    /// supersedes it. Absent unless set, so a board without typed notes is
    /// stored exactly as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub knowledge: Option<Knowledge>,
    /// The earlier note of the same kind this one replaces, by `uid`. A
    /// decision, gotcha or procedure names it when written; a handoff gets it
    /// when it demotes the one before on its task, so a note without a kind
    /// is superseded only as a handoff a later one replaced.
    ///
    /// Stored on the newer note only: the one it replaces is never rewritten
    /// for it (a demoted handoff loses its flag, which `handoff` owns), and
    /// stays on the board as history. Whether a note is superseded is
    /// worked out from the notes pointing at it, so trashing the newer one
    /// makes the older one current again with nothing to undo -- the way a
    /// trashed blocker stops blocking. By uid, for the reason `blocked_by` is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<String>,
    /// On a note that asks the user something: who asked, when, and the
    /// answer once given. A question held only in a session's prompt is lost
    /// when the session clears or restarts, and the user's answer given in
    /// another session never reaches it; on the board it does. Absent unless
    /// set, so a board that asks nothing is stored exactly as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<Question>,
    // Old data may have this stored as a JSON string (a bug in the JS
    // version's --priority path, fixed here rather than carried forward) --
    // still readable, but always written back out as a number now.
    #[serde(default, deserialize_with = "deserialize_priority", skip_serializing_if = "Option::is_none")]
    pub priority: Option<u8>,
    /// Every field this version does not know, with the value it was read
    /// with, written back after the fields it does, in name order.
    ///
    /// A write rewrites the whole file, so without this an older ekko drops
    /// whatever a newer one added from every item at once -- a note's kind,
    /// what it supersedes, a waiting state -- the moment it writes anything.
    /// Kept, such a field outlives every version that cannot read it, and
    /// means what it meant again as soon as one that can reads it, which also
    /// puts it back in its place. Empty on anything this version made, so a
    /// board with nothing unknown on it is byte-identical to before.
    ///
    /// A `BTreeMap` rather than serde_json's order-keeping map, which carries
    /// a hasher on every item: a cold read of 5,000 items was 5% slower with
    /// it, and is 2% slower with this.
    #[serde(flatten)]
    pub unknown: BTreeMap<String, serde_json::Value>,
}

impl Item {
    pub fn new_task(id: u32, description: String, boards: Vec<String>, priority: u8) -> Self {
        let (date, timestamp) = now();
        Item {
            id,
            date,
            timestamp,
            description,
            is_starred: false,
            boards,
            is_task: true,
            is_complete: Some(false),
            in_progress: Some(false),
            due_date: None,
            uid: Some(new_uid()),
            updated_at: Some(timestamp),
            rev: None,
            paused: None,
            cancelled: None,
            waiting: None,
            phase: None,
            blocked_by: None,
            attached_to: None,
            handoff: false,
            knowledge: None,
            supersedes: None,
            question: None,
            held_by: None,
            done_by: None,
            created_by: None,
            with: None,
            stashed: None,
            trashed: None,
            priority: Some(priority),
            unknown: BTreeMap::new(),
        }
    }

    pub fn new_note(id: u32, description: String, boards: Vec<String>) -> Self {
        let (date, timestamp) = now();
        Item {
            id,
            date,
            timestamp,
            description,
            is_starred: false,
            boards,
            is_task: false,
            is_complete: None,
            in_progress: None,
            priority: None,
            due_date: None,
            uid: Some(new_uid()),
            updated_at: Some(timestamp),
            rev: None,
            paused: None,
            cancelled: None,
            waiting: None,
            phase: None,
            blocked_by: None,
            attached_to: None,
            handoff: false,
            knowledge: None,
            supersedes: None,
            question: None,
            held_by: None,
            done_by: None,
            created_by: None,
            with: None,
            stashed: None,
            trashed: None,
            unknown: BTreeMap::new(),
        }
    }

    /// The word the views a person reads put on a note that is more than a
    /// note: "handoff", or the kind of knowledge it holds. None for anything
    /// else. The two never meet on one note, since a handoff cannot be given
    /// a kind.
    pub fn mark(&self) -> Option<&'static str> {
        if self.is_task {
            None
        } else if self.handoff {
            Some("handoff")
        } else {
            self.knowledge.map(Knowledge::word)
        }
    }
}

/// A question asked of the user, which waits for an answer; see
/// `Item::question`. The note's text is the question.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Question {
    /// The Claude Code session that asked, with the conversation it ran:
    /// the answer is for it, after a /clear or a restart too. `None` when
    /// the user wrote the question down themselves.
    #[serde(rename = "askedBy", default, skip_serializing_if = "Option::is_none")]
    pub asked_by: Option<crate::holder::Holder>,
    /// The revision of the write that asked it, stamped as that write is
    /// saved. How far the board has moved since tells an answer that may
    /// have come too late for its question.
    pub rev: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<Answer>,
}

/// The user's answer to a question, and who recorded it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    pub text: String,
    /// The session that heard it and recorded it, or, with no process, the
    /// user at the terminal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<crate::holder::Holder>,
    /// When, in milliseconds, and the revision of the write that recorded
    /// it, stamped as the question's is.
    pub at: i64,
    pub rev: u64,
}

/// The kinds of lasting knowledge a note can hold; see `Item::knowledge`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Knowledge {
    Decision,
    Gotcha,
    Procedure,
}

impl Knowledge {
    /// The word for it, as stored and as every view prints it.
    pub fn word(self) -> &'static str {
        match self {
            Knowledge::Decision => "decision",
            Knowledge::Gotcha => "gotcha",
            Knowledge::Procedure => "procedure",
        }
    }

    /// The kind a word names, singular or plural, the way `--list` takes
    /// `note` and `notes` alike.
    pub fn from_word(word: &str) -> Option<Knowledge> {
        match word {
            "decision" | "decisions" => Some(Knowledge::Decision),
            "gotcha" | "gotchas" => Some(Knowledge::Gotcha),
            "procedure" | "procedures" => Some(Knowledge::Procedure),
            _ => None,
        }
    }
}

/// The six states a task can be in, as one value.
///
/// Storage keeps five flags -- `isComplete` and `inProgress` from taskbook,
/// `paused`, `cancelled` and `waiting` from Ekko -- which is thirty-two
/// combinations for six meaningful states. Reading the flags in several
/// places, each with its own precedence, is how a task once came out cancelled
/// on the board, cancelled in the stats line and done in `--list done`, all at
/// the same time. So every surface reads a task's state through `State::of`,
/// and every command writes through `State::write`, which only ever produces
/// one of six encodings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Pending,
    Progress,
    Paused,
    Waiting,
    Done,
    Cancelled,
}

/// What a command asks of a task's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// `--check`: done becomes pending, anything else becomes done.
    ToggleDone,
    /// `--begin`: in progress becomes pending, anything else becomes in
    /// progress. Pending rather than paused on the way back, because that is
    /// what taskbook wrote and the flags have to stay readable by it.
    ToggleProgress,
    /// `--set done`, `--set paused`, ...: the state the task should end up in.
    Become(State),
    /// `--set undone`: done becomes pending, and anything else is left alone,
    /// because a task that is not done already is what "undone" asks for.
    Undo,
}

impl State {
    #[cfg(test)]
    pub const ALL: [State; 6] =
        [State::Pending, State::Progress, State::Paused, State::Waiting, State::Done, State::Cancelled];

    /// The state of a task, or `None` for a note.
    ///
    /// Flag combinations that are not one of the six encodings -- older
    /// data, a hand-edited file -- resolve by one fixed precedence: cancelled,
    /// then done, then in progress, then paused, then waiting. It is the
    /// precedence the board has always drawn with, waiting added last, so
    /// reading through here changed no icon.
    pub fn of(item: &Item) -> Option<State> {
        if !item.is_task {
            return None;
        }
        Some(if item.cancelled.unwrap_or(false) {
            State::Cancelled
        } else if item.is_complete.unwrap_or(false) {
            State::Done
        } else if item.in_progress.unwrap_or(false) {
            State::Progress
        } else if item.paused.unwrap_or(false) {
            State::Paused
        } else if item.waiting.unwrap_or(false) {
            State::Waiting
        } else {
            State::Pending
        })
    }

    /// Writes this state onto a task as exactly one encoding. `isComplete`
    /// and `inProgress` are always present, as taskbook wrote them; `paused`,
    /// `cancelled` and `waiting` appear only when true, so a board that never
    /// uses them stays byte-identical to what taskbook would have written.
    pub fn write(self, item: &mut Item) {
        item.is_complete = Some(self == State::Done);
        item.in_progress = Some(self == State::Progress);
        item.paused = (self == State::Paused).then_some(true);
        item.cancelled = (self == State::Cancelled).then_some(true);
        item.waiting = (self == State::Waiting).then_some(true);
    }

    /// The one transition function. Total -- every state and every change
    /// has an answer -- and every answer is one of the six states.
    pub fn after(self, change: Change) -> State {
        match change {
            Change::ToggleDone if self == State::Done => State::Pending,
            Change::ToggleDone => State::Done,
            Change::ToggleProgress if self == State::Progress => State::Pending,
            Change::ToggleProgress => State::Progress,
            Change::Become(target) => target,
            Change::Undo if self == State::Done => State::Pending,
            Change::Undo => self,
        }
    }

    /// Still work to do: pending, in progress, paused or waiting.
    pub fn is_open(self) -> bool {
        matches!(self, State::Pending | State::Progress | State::Paused | State::Waiting)
    }

    /// Open work that can be taken up once nothing on the board holds it:
    /// every open state but waiting, whose hold is outside the board.
    pub fn can_start(self) -> bool {
        self.is_open() && self != State::Waiting
    }

    /// The word the views print for this state: the prime, `next`, `context`
    /// and their JSON alike. Spelled here and nowhere else, so nothing reads
    /// a state back from its word.
    pub fn word(self) -> &'static str {
        match self {
            State::Pending => "pending",
            State::Progress => "in progress",
            State::Paused => "paused",
            State::Waiting => "waiting",
            State::Done => "done",
            State::Cancelled => "cancelled",
        }
    }
}

/// What `--set` and `set_state` ask for: a state for a task to end up in,
/// `undone`, or a star put on or taken off.
///
/// The words for these are spelled in `word`, and read back in `from_word`,
/// and nowhere else. They used to be strings matched in several places, one
/// of them with a catch-all arm -- which is how `cancelled` and `unstarted`
/// once shipped with no confirmation at all. Matched as this instead, a new
/// state does not compile until every one of those places says what it does
/// with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    /// The state to end up in, whatever the task is in now. `unstarted` is
    /// `Become(Pending)`: the way back to never-started, from anywhere.
    Become(State),
    /// `undone`: done becomes pending, and anything else is left alone,
    /// because a task that is not done already is what "undone" asks for.
    Undone,
    Starred,
    Unstarred,
}

impl Setting {
    /// Every setting, in the order a caller is told them.
    pub const ALL: [Setting; 9] = [
        Setting::Become(State::Done),
        Setting::Undone,
        Setting::Become(State::Progress),
        Setting::Become(State::Paused),
        Setting::Become(State::Waiting),
        Setting::Become(State::Cancelled),
        Setting::Become(State::Pending),
        Setting::Starred,
        Setting::Unstarred,
    ];

    /// The word for it: what `--json` reports and the MCP schema lists.
    pub fn word(self) -> &'static str {
        match self {
            Setting::Become(State::Done) => "done",
            Setting::Undone => "undone",
            Setting::Become(State::Progress) => "progress",
            Setting::Become(State::Paused) => "paused",
            Setting::Become(State::Waiting) => "waiting",
            Setting::Become(State::Cancelled) => "cancelled",
            Setting::Become(State::Pending) => "unstarted",
            Setting::Starred => "starred",
            Setting::Unstarred => "unstarred",
        }
    }

    /// The setting a word asks for: its own word, or one of the others `--set`
    /// has always taken for it. Deliberately the same words `--list` filters
    /// on, so there is one set of names to learn rather than two.
    pub fn from_word(word: &str) -> Option<Setting> {
        Some(match word {
            "done" | "checked" | "complete" => Setting::Become(State::Done),
            "undone" | "unchecked" | "incomplete" | "pending" => Setting::Undone,
            "progress" | "started" | "begun" => Setting::Become(State::Progress),
            "paused" => Setting::Become(State::Paused),
            "waiting" => Setting::Become(State::Waiting),
            // Repointed: with a real paused state these became opposites.
            // Also the way back from a mistyped `--set progress`.
            "unstarted" | "unstart" => Setting::Become(State::Pending),
            "cancel" | "cancelled" | "canceled" => Setting::Become(State::Cancelled),
            "star" | "starred" => Setting::Starred,
            "unstar" | "unstarred" => Setting::Unstarred,
            _ => return None,
        })
    }

    /// Whether this is about a task's state, which a note does not have.
    /// Starring is the one setting that applies to both.
    pub fn is_state(self) -> bool {
        !matches!(self, Setting::Starred | Setting::Unstarred)
    }

    /// Applies this to an item. A task's state goes through `State::after`
    /// and `State::write`, so it always lands on one of the six encodings; a
    /// note has no state and is left alone, the way `--check` and `--begin`
    /// leave notes alone.
    pub fn apply(self, item: &mut Item) {
        let change = match self {
            Setting::Become(state) => Change::Become(state),
            Setting::Undone => Change::Undo,
            Setting::Starred | Setting::Unstarred => {
                item.is_starred = self == Setting::Starred;
                return;
            }
        };
        if let Some(current) = State::of(item) {
            current.after(change).write(item);
        }
    }
}

/// `(done, total)` for a set of items, counted the one way Ekko counts
/// progress everywhere. Notes are not work, and neither is a cancelled task,
/// so both stay out of the total -- which is what lets a board that dropped
/// something still reach 100%. The board title, `--projects`, the roadmap and
/// the percentage agree because they all count through here.
pub fn tally<'a>(items: impl IntoIterator<Item = &'a Item>) -> (u32, u32) {
    items.into_iter().filter_map(State::of).fold((0, 0), |(done, total), state| match state {
        State::Cancelled => (done, total),
        State::Done => (done + 1, total + 1),
        _ => (done, total + 1),
    })
}

/// `(_date, _timestamp)` for a freshly created item, computed *now* -- not
/// once at startup and reused. That was a real bug in the JS version: `now`
/// was a module-level `const`, so every item created within one long-lived
/// process shared a single frozen timestamp. Harmless for a CLI that starts
/// a new process per command, but wrong the moment this is used as a
/// library (as our own test suite does). Computing it fresh on every call
/// makes that bug structurally impossible here rather than merely fixed.
fn now() -> (String, i64) {
    let now = Local::now();
    // Matches JS `Date.prototype.toDateString()` exactly, e.g. "Sun Aug 23
    // 2026" -- zero-padded day, no leading zero suppression. Verified
    // against a real `new Date(...).toDateString()` call, not assumed.
    (now.format("%a %b %d %Y").to_string(), now.timestamp_millis())
}

fn deserialize_priority<'de, D>(deserializer: D) -> Result<Option<u8>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrNumber {
        Number(u8),
        String(String),
    }

    Ok(match Option::<StringOrNumber>::deserialize(deserializer)? {
        None => None,
        Some(StringOrNumber::Number(n)) => Some(n),
        Some(StringOrNumber::String(s)) => s.parse().ok(),
    })
}


/// Unique per item, and cheap: nanosecond clock plus pid, hex-encoded.
/// Same trick `storage::temp_file_path` already uses, and for the same
/// reason -- unique enough without pulling in a `rand` dependency. Two
/// items created back to back in one process get different nanos; two
/// processes racing get different pids.
pub(crate) fn new_uid() -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("{:x}-{:x}", nanos, process::id())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_for_clock_tick() {
        // `_timestamp` is millisecond-truncated, so waiting for *any*
        // nonzero `Instant` delta isn't enough -- wait for the millisecond
        // value itself to roll over, the same technique the JS regression
        // test used.
        let start = Local::now().timestamp_millis();
        while Local::now().timestamp_millis() == start {
            // Busy-wait for the millisecond to roll over.
        }
    }

    #[test]
    fn each_item_gets_its_own_timestamp() {
        let first = Item::new_task(1, "first".into(), vec![], 1);
        wait_for_clock_tick();
        let second = Item::new_task(2, "second".into(), vec![], 1);

        assert_ne!(first.timestamp, second.timestamp);
    }

    #[test]
    fn date_and_timestamp_agree() {
        let item = Item::new_task(1, "x".into(), vec![], 1);
        let from_timestamp = chrono::DateTime::from_timestamp_millis(item.timestamp)
            .unwrap()
            .with_timezone(&Local)
            .format("%a %b %d %Y")
            .to_string();

        assert_eq!(item.date, from_timestamp);
    }

    #[test]
    fn note_has_no_task_fields() {
        let note = Item::new_note(1, "a note".into(), vec!["@x".into()]);

        assert!(!note.is_task);
        assert_eq!(note.is_complete, None);
        assert_eq!(note.in_progress, None);
        assert_eq!(note.priority, None);

        let json = serde_json::to_value(&note).unwrap();
        assert!(json.get("isComplete").is_none());
        assert!(json.get("inProgress").is_none());
        assert!(json.get("priority").is_none());
    }

    #[test]
    fn task_serializes_with_the_exact_js_field_names() {
        let task = Item::new_task(7, "ship it".into(), vec!["@coding".into()], 2);
        let json = serde_json::to_value(&task).unwrap();

        assert_eq!(json["_id"], 7);
        assert_eq!(json["description"], "ship it");
        assert_eq!(json["isStarred"], false);
        assert_eq!(json["boards"], serde_json::json!(["@coding"]));
        assert_eq!(json["_isTask"], true);
        assert_eq!(json["isComplete"], false);
        assert_eq!(json["inProgress"], false);
        assert_eq!(json["priority"], 2);
    }

    #[test]
    fn reads_legacy_string_priority_and_legacy_number_priority_alike() {
        let string_priority = serde_json::json!({
            "_id": 1, "_date": "Sun Aug 23 2026", "_timestamp": 0,
            "description": "x", "isStarred": false, "boards": [],
            "_isTask": true, "isComplete": false, "inProgress": false,
            "priority": "3"
        });
        let number_priority = serde_json::json!({
            "_id": 2, "_date": "Sun Aug 23 2026", "_timestamp": 0,
            "description": "x", "isStarred": false, "boards": [],
            "_isTask": true, "isComplete": false, "inProgress": false,
            "priority": 1
        });

        let a: Item = serde_json::from_value(string_priority).unwrap();
        let b: Item = serde_json::from_value(number_priority).unwrap();

        assert_eq!(a.priority, Some(3));
        assert_eq!(b.priority, Some(1));
    }

    #[test]
    fn reads_a_real_note_produced_by_the_js_version_unchanged() {
        // A verbatim row copied from a JS-produced archive.json during
        // this project's own dogfooding session.
        let raw = serde_json::json!({
            "_id": 1,
            "_date": "Sun Aug 23 2026",
            "_timestamp": 1787531257957_i64,
            "description": "A reference note",
            "isStarred": false,
            "boards": ["@coding"],
            "_isTask": false
        });

        let note: Item = serde_json::from_value(raw).unwrap();

        assert_eq!(note.id, 1);
        assert!(!note.is_task);
        assert_eq!(note.is_complete, None);
    }

    fn task_in(state: State) -> Item {
        let mut item = Item::new_task(1, "t".into(), vec![], 1);
        state.write(&mut item);
        item
    }

    /// The five flags a state is stored in -- compared on their own, because
    /// every freshly made item also carries its own uid and timestamp.
    fn flags(item: &Item) -> [Option<bool>; 5] {
        [item.is_complete, item.in_progress, item.paused, item.cancelled, item.waiting]
    }

    /// Every state against every change, spelled out rather than derived, so
    /// the table is a statement of what the commands mean and not a copy of
    /// the code under test. Each answer must also land as exactly the one
    /// encoding of its state.
    #[test]
    fn every_change_from_every_state_lands_on_one_canonical_state() {
        use Change::*;
        use State::*;
        let table = [
            (Pending, ToggleDone, Done), (Pending, ToggleProgress, Progress), (Pending, Undo, Pending),
            (Progress, ToggleDone, Done), (Progress, ToggleProgress, Pending), (Progress, Undo, Progress),
            (Paused, ToggleDone, Done), (Paused, ToggleProgress, Progress), (Paused, Undo, Paused),
            (Waiting, ToggleDone, Done), (Waiting, ToggleProgress, Progress), (Waiting, Undo, Waiting),
            (Done, ToggleDone, Pending), (Done, ToggleProgress, Progress), (Done, Undo, Pending),
            (Cancelled, ToggleDone, Done), (Cancelled, ToggleProgress, Progress), (Cancelled, Undo, Cancelled),
        ];
        let mut cases: Vec<(State, Change, State)> = table.to_vec();
        for from in State::ALL {
            for target in State::ALL {
                cases.push((from, Become(target), target));
            }
        }
        assert_eq!(cases.len(), 6 * 9, "six states times nine changes");

        for (from, change, to) in cases {
            assert_eq!(from.after(change), to, "{from:?} after {change:?}");

            let mut item = task_in(from);
            from.after(change).write(&mut item);
            assert_eq!(State::of(&item), Some(to), "{from:?} after {change:?} read back");
            assert_eq!(flags(&item), flags(&task_in(to)), "{from:?} after {change:?} is not the canonical {to:?}");
        }
    }

    /// `--set` is retry-safe only if every target is idempotent.
    #[test]
    fn asking_for_the_same_state_twice_is_asking_once() {
        for from in State::ALL {
            for change in State::ALL.map(Change::Become).into_iter().chain([Change::Undo]) {
                let once = from.after(change);
                assert_eq!(once.after(change), once, "{from:?} then {change:?} twice");
            }
        }
    }

    /// All thirty-two flag combinations read as one state, by one precedence,
    /// and writing that state back yields its canonical encoding.
    #[test]
    fn every_flag_combination_reads_as_exactly_one_state() {
        for bits in 0u8..32 {
            let mut item = Item::new_task(1, "t".into(), vec![], 1);
            item.is_complete = Some(bits & 1 != 0);
            item.in_progress = Some(bits & 2 != 0);
            item.paused = (bits & 4 != 0).then_some(true);
            item.cancelled = (bits & 8 != 0).then_some(true);
            item.waiting = (bits & 16 != 0).then_some(true);

            let expected = if bits & 8 != 0 {
                State::Cancelled
            } else if bits & 1 != 0 {
                State::Done
            } else if bits & 2 != 0 {
                State::Progress
            } else if bits & 4 != 0 {
                State::Paused
            } else if bits & 16 != 0 {
                State::Waiting
            } else {
                State::Pending
            };
            let state = State::of(&item).expect("a task has a state");
            assert_eq!(state, expected, "flags {bits:05b}");

            state.write(&mut item);
            assert_eq!(flags(&item), flags(&task_in(expected)), "flags {bits:05b} did not normalise");
        }
    }

    /// `--check` and `--begin` on the three states taskbook knew must write
    /// what taskbook wrote: both flags present, and no Ekko-only key.
    #[test]
    fn the_toggles_write_what_taskbook_wrote() {
        for (from, change) in [
            (State::Pending, Change::ToggleDone),
            (State::Done, Change::ToggleDone),
            (State::Pending, Change::ToggleProgress),
            (State::Progress, Change::ToggleProgress),
        ] {
            let mut item = task_in(from);
            from.after(change).write(&mut item);
            let json = serde_json::to_value(&item).unwrap();
            assert!(json.get("isComplete").is_some() && json.get("inProgress").is_some());
            assert!(json.get("paused").is_none() && json.get("cancelled").is_none() && json.get("waiting").is_none(), "{json}");
        }
    }

    #[test]
    fn the_total_leaves_out_notes_and_cancelled_tasks() {
        let done = task_in(State::Done);
        let pending = task_in(State::Pending);
        let paused = task_in(State::Paused);
        let waiting = task_in(State::Waiting);
        let cancelled = task_in(State::Cancelled);
        let note = Item::new_note(9, "n".into(), vec![]);

        assert_eq!(tally([&done, &pending, &paused, &waiting, &cancelled, &note]), (1, 4));
    }

    /// `--anchor` became `--attached-to`, and the stored field was renamed
    /// with it. Every board attached before then says `anchor`: those
    /// attachments have to survive being read, and the next write has to
    /// store the new name rather than carry the old one forward.
    #[test]
    fn an_attachment_stored_under_its_old_name_is_read_and_written_under_the_new_one() {
        let legacy = serde_json::json!({
            "_id": 3, "_date": "Fri Aug 28 2026", "_timestamp": 0,
            "description": "why", "isStarred": false, "boards": ["@a"],
            "_isTask": false, "uid": "18d0-1", "anchor": "18cf-2"
        });

        let note: Item = serde_json::from_value(legacy).unwrap();
        assert_eq!(note.attached_to.as_deref(), Some("18cf-2"));

        let json = serde_json::to_value(&note).unwrap();
        assert_eq!(json["attachedTo"], "18cf-2");
        assert!(json.get("anchor").is_none(), "the old name was written back out");
    }

    /// A typed note stores its kind and what it supersedes as two plain
    /// fields; a note without them stores neither key, so a board with no
    /// typed notes is written exactly as before they existed.
    #[test]
    fn a_typed_note_adds_two_fields_and_an_untyped_one_adds_none() {
        let plain = Item::new_note(1, "a note".into(), vec!["My Board".into()]);
        let json = serde_json::to_value(&plain).unwrap();
        assert!(json.get("knowledge").is_none() && json.get("supersedes").is_none(), "{json}");

        let mut typed = plain.clone();
        typed.knowledge = Some(Knowledge::Decision);
        typed.supersedes = Some("18d0-1".into());
        let json = serde_json::to_value(&typed).unwrap();
        assert_eq!((json["knowledge"].as_str(), json["supersedes"].as_str()), (Some("decision"), Some("18d0-1")));
        assert_eq!(serde_json::from_value::<Item>(json).unwrap(), typed);
    }

    /// Each setting's word reads back as that setting and no other -- so no
    /// two share a word, and `--json` never reports one `--set` would refuse
    /// -- and every state is one a task can be set to.
    #[test]
    fn every_setting_word_reads_back_as_that_setting() {
        for setting in Setting::ALL {
            assert_eq!(Setting::from_word(setting.word()), Some(setting), "{setting:?}");
        }
        for state in State::ALL {
            assert!(Setting::ALL.contains(&Setting::Become(state)), "{state:?} cannot be set");
        }
    }

    /// No two states print as the same word, so a view never says one state
    /// where it means another.
    #[test]
    fn every_state_prints_as_a_word_of_its_own() {
        let words: std::collections::HashSet<&str> = State::ALL.iter().map(|state| state.word()).collect();
        assert_eq!(words.len(), State::ALL.len(), "{words:?}");
    }

    /// A board as ekko 0.10.2 wrote it, with every field it stores on an item
    /// that carries it. Captured from the binary rather than typed out, apart
    /// from `phase` and `handoff`, which the CLI cannot set on a bare
    /// directory and were placed where the serializer puts them.
    const BOARD: &str = r#"{
    "1": {
        "_id": 1,
        "_date": "Mon Sep 21 2026",
        "_timestamp": 1789993060230,
        "description": "Ship the release",
        "isStarred": true,
        "boards": [
            "@work"
        ],
        "_isTask": true,
        "isComplete": false,
        "inProgress": false,
        "dueDate": "2026-10-01",
        "uid": "18d7553459722c26-6426a",
        "updatedAt": 1789993060287,
        "rev": 10,
        "phase": "release",
        "blockedBy": [
            "18d755345a4df713-6426b"
        ],
        "priority": 2
    },
    "2": {
        "_id": 2,
        "_date": "Mon Sep 21 2026",
        "_timestamp": 1789993060244,
        "description": "Wait for the reply",
        "isStarred": false,
        "boards": [
            "@work"
        ],
        "_isTask": true,
        "isComplete": false,
        "inProgress": false,
        "uid": "18d755345a4df713-6426b",
        "updatedAt": 1789993060292,
        "rev": 11,
        "waiting": true,
        "priority": 1
    },
    "3": {
        "_id": 3,
        "_date": "Mon Sep 21 2026",
        "_timestamp": 1789993060250,
        "description": "Pause this one",
        "isStarred": false,
        "boards": [
            "My Board"
        ],
        "_isTask": true,
        "isComplete": false,
        "inProgress": false,
        "uid": "18d755345aa3083d-6426c",
        "updatedAt": 1789993060307,
        "rev": 14,
        "paused": true,
        "stashed": 1789993060307,
        "priority": 1
    },
    "4": {
        "_id": 4,
        "_date": "Mon Sep 21 2026",
        "_timestamp": 1789993060255,
        "description": "Drop this one",
        "isStarred": false,
        "boards": [
            "My Board"
        ],
        "_isTask": true,
        "isComplete": false,
        "inProgress": false,
        "uid": "18d755345af38d72-6426d",
        "updatedAt": 1789993060313,
        "rev": 15,
        "cancelled": true,
        "priority": 1
    },
    "5": {
        "_id": 5,
        "_date": "Mon Sep 21 2026",
        "_timestamp": 1789993060260,
        "description": "A done one",
        "isStarred": false,
        "boards": [
            "My Board"
        ],
        "_isTask": true,
        "isComplete": true,
        "inProgress": false,
        "uid": "18d755345b4568d2-6426e",
        "updatedAt": 1789993060319,
        "rev": 16,
        "priority": 1
    },
    "6": {
        "_id": 6,
        "_date": "Mon Sep 21 2026",
        "_timestamp": 1789993060266,
        "description": "Ship it as a patch",
        "isStarred": false,
        "boards": [
            "My Board"
        ],
        "_isTask": false,
        "uid": "18d755345b973028-6426f",
        "updatedAt": 1789993060326,
        "rev": 17,
        "trashed": 1789993060326,
        "knowledge": "decision"
    },
    "7": {
        "_id": 7,
        "_date": "Mon Sep 21 2026",
        "_timestamp": 1789993060271,
        "description": "Ship it as a minor",
        "isStarred": false,
        "boards": [
            "@work"
        ],
        "_isTask": false,
        "uid": "18d755345be5e14a-64270",
        "updatedAt": 1789993060333,
        "rev": 18,
        "attachedTo": "18d7553459722c26-6426a",
        "knowledge": "decision",
        "supersedes": "18d755345b973028-6426f"
    },
    "8": {
        "_id": 8,
        "_date": "Mon Sep 21 2026",
        "_timestamp": 1789993060276,
        "description": "Where the last session stopped",
        "isStarred": false,
        "boards": [
            "My Board"
        ],
        "_isTask": false,
        "uid": "18d755345c32f7ff-64271",
        "updatedAt": 1789993060340,
        "rev": 19,
        "attachedTo": "18d755345a4df713-6426b",
        "handoff": true
    }
}"#;

    /// A board with nothing unknown on it is written back byte for byte:
    /// keeping the fields this version does not know costs a board that has
    /// none nothing at all.
    #[test]
    fn a_board_with_nothing_unknown_is_written_back_byte_for_byte() {
        let board: BTreeMap<u32, Item> = serde_json::from_str(BOARD).unwrap();
        assert_eq!(crate::json::to_pretty_string(&board).unwrap(), BOARD);
    }

    /// Fields a later version added, which this one does not know, come
    /// through a read and a write with their values, instead of being dropped
    /// the way every field was before `unknown` held them. They are written
    /// after the fields this version knows; the later version puts them back
    /// in their places on its own next write.
    #[test]
    fn fields_from_a_later_version_survive_a_read_and_a_write() {
        let later = r#"{"_id": 1, "_date": "Mon Sep 21 2026", "_timestamp": 0, "description": "x",
            "isStarred": false, "boards": ["My Board"], "_isTask": true, "isComplete": false,
            "inProgress": false, "watchers": ["upstream", {"since": 1789993060230}], "priority": 1,
            "checkBack": "2026-10-01"}"#;

        let item: Item = serde_json::from_str(later).unwrap();
        let written = serde_json::to_value(&item).unwrap();

        assert_eq!(written["watchers"], serde_json::json!(["upstream", {"since": 1789993060230_u64}]));
        assert_eq!(written["checkBack"], "2026-10-01");
        assert_eq!(item.priority, Some(1), "the known fields around them are still read");
        assert_eq!(serde_json::from_value::<Item>(written).unwrap(), item);
    }
}
