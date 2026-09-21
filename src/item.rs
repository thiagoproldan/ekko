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
    /// The earlier note of the same kind this one replaces, by `uid`.
    ///
    /// Stored on the newer note only: the one it replaces is never rewritten,
    /// and stays on the board as history. Whether a note is superseded is
    /// worked out from the notes pointing at it, so trashing the newer one
    /// makes the older one current again with nothing to undo -- the way a
    /// trashed blocker stops blocking. By uid, for the reason `blocked_by` is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<String>,
    // Old data may have this stored as a JSON string (a bug in the JS
    // version's --priority path, fixed here rather than carried forward) --
    // still readable, but always written back out as a number now.
    #[serde(default, deserialize_with = "deserialize_priority", skip_serializing_if = "Option::is_none")]
    pub priority: Option<u8>,
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
            phase: None,
            blocked_by: None,
            attached_to: None,
            handoff: false,
            knowledge: None,
            supersedes: None,
            stashed: None,
            trashed: None,
            priority: Some(priority),
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
            phase: None,
            blocked_by: None,
            attached_to: None,
            handoff: false,
            knowledge: None,
            supersedes: None,
            stashed: None,
            trashed: None,
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

/// The five states a task can be in, as one value.
///
/// Storage keeps four flags -- `isComplete` and `inProgress` from taskbook,
/// `paused` and `cancelled` from Ekko -- which is sixteen combinations for
/// five meaningful states. Reading the flags in several places, each with its
/// own precedence, is how a task once came out cancelled on the board, cancelled
/// in the stats line and done in `--list done`, all at the same time. So every
/// surface reads a task's state through `State::of`, and every command writes
/// through `State::write`, which only ever produces one of five encodings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Pending,
    Progress,
    Paused,
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
    pub const ALL: [State; 5] =
        [State::Pending, State::Progress, State::Paused, State::Done, State::Cancelled];

    /// The state of a task, or `None` for a note.
    ///
    /// Flag combinations that are not one of the five encodings -- older
    /// data, a hand-edited file -- resolve by one fixed precedence: cancelled,
    /// then done, then in progress, then paused. It is the precedence the
    /// board has always drawn with, so reading through here changed no icon.
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
        } else {
            State::Pending
        })
    }

    /// Writes this state onto a task as exactly one encoding. `isComplete`
    /// and `inProgress` are always present, as taskbook wrote them; `paused`
    /// and `cancelled` appear only when true, so a board that never uses them
    /// stays byte-identical to what taskbook would have written.
    pub fn write(self, item: &mut Item) {
        item.is_complete = Some(self == State::Done);
        item.in_progress = Some(self == State::Progress);
        item.paused = (self == State::Paused).then_some(true);
        item.cancelled = (self == State::Cancelled).then_some(true);
    }

    /// The one transition function. Total -- every state and every change
    /// has an answer -- and every answer is one of the five states.
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

    /// Still work to do: pending, in progress or paused.
    pub fn is_open(self) -> bool {
        matches!(self, State::Pending | State::Progress | State::Paused)
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

    /// The four flags a state is stored in -- compared on their own, because
    /// every freshly made item also carries its own uid and timestamp.
    fn flags(item: &Item) -> (Option<bool>, Option<bool>, Option<bool>, Option<bool>) {
        (item.is_complete, item.in_progress, item.paused, item.cancelled)
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
            (Done, ToggleDone, Pending), (Done, ToggleProgress, Progress), (Done, Undo, Pending),
            (Cancelled, ToggleDone, Done), (Cancelled, ToggleProgress, Progress), (Cancelled, Undo, Cancelled),
        ];
        let mut cases: Vec<(State, Change, State)> = table.to_vec();
        for from in State::ALL {
            for target in State::ALL {
                cases.push((from, Become(target), target));
            }
        }
        assert_eq!(cases.len(), 5 * 8, "five states times eight changes");

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

    /// All sixteen flag combinations read as one state, by one precedence,
    /// and writing that state back yields its canonical encoding.
    #[test]
    fn every_flag_combination_reads_as_exactly_one_state() {
        for bits in 0u8..16 {
            let mut item = Item::new_task(1, "t".into(), vec![], 1);
            item.is_complete = Some(bits & 1 != 0);
            item.in_progress = Some(bits & 2 != 0);
            item.paused = (bits & 4 != 0).then_some(true);
            item.cancelled = (bits & 8 != 0).then_some(true);

            let expected = if bits & 8 != 0 {
                State::Cancelled
            } else if bits & 1 != 0 {
                State::Done
            } else if bits & 2 != 0 {
                State::Progress
            } else if bits & 4 != 0 {
                State::Paused
            } else {
                State::Pending
            };
            let state = State::of(&item).expect("a task has a state");
            assert_eq!(state, expected, "flags {bits:04b}");

            state.write(&mut item);
            assert_eq!(flags(&item), flags(&task_in(expected)), "flags {bits:04b} did not normalise");
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
            assert!(json.get("paused").is_none() && json.get("cancelled").is_none(), "{json}");
        }
    }

    #[test]
    fn the_total_leaves_out_notes_and_cancelled_tasks() {
        let done = task_in(State::Done);
        let pending = task_in(State::Pending);
        let paused = task_in(State::Paused);
        let cancelled = task_in(State::Cancelled);
        let note = Item::new_note(9, "n".into(), vec![]);

        assert_eq!(tally([&done, &pending, &paused, &cancelled, &note]), (1, 3));
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
}
