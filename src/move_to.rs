//! `ekko --move-to <PROJECT|FOLDER> IDS`: items from this board to another,
//! a project's or the default board (task 897).
//!
//! Done the way an issue moves between Jira projects or GitHub repositories:
//! it keeps who it is -- uid, dates, author, state, text -- and takes the next
//! display id where it goes; what hangs under it goes with it, as sub-tasks go
//! with their parent in Jira; and the board it left says where it went, as the
//! old key or URL redirects. A task takes every note attached to it, and a
//! note attached to a task moves only with it.
//!
//! Every field an item holds is one of four kinds, and `arriving` and
//! `references` destructure each, so a field added later does not compile
//! until it is placed:
//!
//! - its own, kept as it is: text, dates, author, state, holders, boards;
//! - the board's, restamped by the write it arrives in: the display id and
//!   every revision, which count that board's writes and not this one's;
//! - another item's uid: what blocks it, the task a note is attached to, the
//!   note it supersedes, what a wait waits on, the question that turned a cue
//!   on, the gotcha a question proposes a cue for. Each has to name an item on
//!   its own board after the move as it did before, or that board drops it
//!   without a word -- a blocker not on the board blocks nothing, and an older
//!   decision becomes current again -- so a link between an item that moves
//!   and one that stays is refused;
//! - a place: its phase, which the board it goes to must declare, and the
//!   folder a cue guards. A cue naming none guards the board it is on, so
//!   after the move the board it went to. No folder is written out for it:
//!   a path written out outlives its board -- the project's folder moves,
//!   `--destroy` forgets the project -- and would guard a place no board
//!   stands for. The reply names each cue that is on and now guards another
//!   place.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::directory::{self, DirectoryError, Location};
use crate::ekko::{Ekko, EkkoError, Outcome};
use crate::holder::Holder;
use crate::item::{Answer, Approving, Artifact, Comment, CueOn, Item, Over, Proposal, Question, Review, Step, Wait};
use crate::storage::{ItemMap, Moved};

/// An item `--move-to` took: its id on the board it left and on the one it
/// went to, its uid, and, for a note that went because its task did, that
/// task's id on the board it left.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Carried {
    pub id: u32,
    #[serde(rename = "as")]
    pub as_id: u32,
    pub uid: String,
    #[serde(rename = "noteOf", skip_serializing_if = "Option::is_none")]
    pub note_of: Option<u32>,
    /// Where its cue guarded and guards now, when the cue is on and names no
    /// folder, and the board it went to guards another place.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cue: Option<Rescoped>,
}

/// The place a cue that names no folder guarded on the board it left and
/// guards on the one it went to: a project's folder, or `/`, the whole
/// machine, from the default board.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Rescoped {
    pub guarded: String,
    pub guards: String,
}

/// A link a move would split between two boards: `from` names `to` as `how`
/// says, and `moves` tells whether `from` is the one that would leave.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Split {
    pub from: u32,
    pub how: &'static str,
    pub to: u32,
    #[serde(rename = "fromMoves")]
    pub moves: bool,
}

impl Split {
    pub fn text(&self) -> String {
        if self.moves {
            format!("{} {} {}, which stays", self.from, self.how, self.to)
        } else {
            format!("{}, which stays, {} {}", self.from, self.how, self.to)
        }
    }
}

/// Moves the items `ids` names from `ekko`'s board to the one `destination`
/// names -- a project, by name, or a folder, whose board is the one `ekko`
/// finds there -- with every note attached to a task among them.
///
/// Nothing is written unless everything can move: a link that would be split,
/// an item in the trash, a phase the other board does not declare, and what a
/// running session holds or watches here (short of `force`) each refuse the
/// whole move. The other board is written first, then the redirect, then this
/// board, the order `--restore` uses: a failure in between leaves an item on
/// both boards, and running the move again finds it there by uid and takes it
/// off this one, instead of leaving it on neither.
pub fn move_to(
    ekko: &Ekko,
    home: &Path,
    cwd: &Path,
    destination: &str,
    ids: &[String],
    force: bool,
) -> Result<Outcome, EkkoError> {
    let location = located(home, cwd, destination)?;
    let mut there = Ekko::at(&location)?.in_folder(location.project.as_ref().and_then(|project| project.root.clone()));
    if let Some(actor) = &ekko.actor {
        there = there.acting_as(actor.clone());
    }
    if board_dir(ekko) == board_dir(&there) {
        return Err(invalid(format!("{destination} is this board: nothing to move")));
    }
    let project = location.project.as_ref().map(|project| project.name.clone());

    // Both locks, taken in one order whichever way the move goes, so two moves
    // between the same boards in opposite directions cannot each hold one lock
    // and wait for the other.
    let here_first = board_dir(ekko) < board_dir(&there);
    let first = if here_first { &ekko.storage } else { &there.storage };
    let second = if here_first { &there.storage } else { &ekko.storage };
    let _first = first.acquire_lock()?;
    let _second = second.acquire_lock()?;

    let mut before = ekko.storage.get_shared()?;
    let named = ekko.validate_ids(ids, &before)?;
    let moving = gathered(&before, &named)?;

    let splits = splits(&before, &moving);
    if !splits.is_empty() {
        return Err(EkkoError::SplitLinks(splits));
    }
    let phases = there.storage.get_phases()?;
    if let Some((id, phase)) = moving.keys().find_map(|id| {
        let phase = before[id].phase.as_deref()?;
        (!phases.iter().any(|declared| declared == phase)).then_some((*id, phase))
    }) {
        let board = board_name(project.as_deref());
        return Err(invalid(format!(
            "{id} is in phase {phase}, which {board} does not declare: declare it there with --phases first, or move {id} to the root, with no phase"
        )));
    }

    let mut left = ItemMap::clone(&before);
    left.retain(|id, _| !moving.contains_key(id));
    if !force {
        let held = ekko.held_elsewhere(&before, &left, &[]);
        if !held.is_empty() {
            return Err(EkkoError::Held(held));
        }
        let watched = watched(ekko, &before, &moving);
        if !watched.is_empty() {
            return Err(EkkoError::Watched(watched));
        }
    }

    // A move keeps an item by its uid. One from before uids gets its uid in a
    // write of its own first, so a move run again after a failure finds the
    // item it already took. Nothing names it, so no check above changes.
    if moving.keys().any(|id| before[id].uid.is_none()) {
        let mut data = ItemMap::clone(&before);
        for id in moving.keys() {
            data.get_mut(id).expect("gathered from the board").uid.get_or_insert_with(crate::item::new_uid);
        }
        ekko.save_against(&before, &mut data)?;
        before = ekko.storage.get_shared()?;
        left = ItemMap::clone(&before);
        left.retain(|id, _| !moving.contains_key(id));
    }

    // A cue that names no folder guards the board's: a project's folder, or
    // the whole machine from the default board. It goes as it is, and the
    // reply says where it guards now.
    let place = |folder: &Option<PathBuf>| folder.as_ref().map_or_else(|| "/".to_string(), |folder| folder.display().to_string());
    let (guarded, guards) = (place(&ekko.folder), place(&there.folder));
    let rescoped = |item: &Item| {
        let moved = crate::guard::cue_of(item).is_some_and(|cue| cue.folder.is_none());
        moved.then(|| Rescoped { guarded: guarded.clone(), guards: guards.clone() })
    };
    let there_before = there.storage.get_shared()?;
    let mut arrived_at = ItemMap::clone(&there_before);
    let mut arrived = ItemMap::new();
    let mut carried = Vec::new();
    for (id, note_of) in &moving {
        let item = &before[id];
        let uid = item.uid.clone().expect("given one above");
        let as_id = match there_before.iter().find(|(_, there)| there.uid.as_deref() == Some(uid.as_str())) {
            Some((as_id, _)) => *as_id,
            None => {
                let as_id = there.generate_id(&arrived_at);
                let copy = arriving(item, as_id);
                arrived_at.insert(as_id, copy.clone());
                arrived.insert(as_id, copy);
                as_id
            }
        };
        carried.push(Carried { id: *id, as_id, uid, note_of: *note_of, cue: rescoped(item) });
    }
    there.save_arriving(&there_before, &mut arrived_at, &arrived)?;

    // One redirect for each id that left, kept when the item comes back or
    // moves on: each board it left says where it went from there, as Jira
    // stacks the keys an issue had. A move run again replaces its own.
    let now = chrono::Local::now().timestamp_millis();
    let mut redirects = ekko.storage.get_moved()?;
    redirects.retain(|moved| !carried.iter().any(|item| item.id == moved.id));
    redirects.extend(carried.iter().map(|item| Moved {
        id: item.id,
        uid: item.uid.clone(),
        project: project.clone(),
        as_id: item.as_id,
        at: now,
        unknown: BTreeMap::new(),
    }));
    ekko.storage.set_moved(&redirects)?;
    ekko.save_against(&before, &mut left)?;

    Ok(Outcome::MovedTo { project, items: carried })
}

/// The board `destination` names: a project by its name, or a folder --
/// anything with a slash, `~`, `.` or `..`, which no project name can be --
/// read the way `ekko` run in it would read it: `~` is the default board.
pub(crate) fn located(home: &Path, cwd: &Path, destination: &str) -> Result<Location, EkkoError> {
    let folder = destination.contains('/') || matches!(destination, "~" | "." | "..");
    if !folder {
        return Ok(directory::locate(home, cwd, None, None, Some(destination))?);
    }
    let path = crate::paths::resolve_path(home, cwd, destination);
    if !path.is_dir() {
        return Err(DirectoryError::NotAFolder(destination.to_string()).into());
    }
    let location = directory::locate(home, &path, None, None, None)?;
    if let Some(lost) = &location.lost {
        return Err(invalid(format!("{destination} has no board to move to: {}", lost.warning())));
    }
    Ok(location)
}

/// Where a board lives on disk, the same path however it was reached.
fn board_dir(ekko: &Ekko) -> PathBuf {
    let dir = ekko.storage.storage_path().parent().unwrap_or(Path::new("/")).to_path_buf();
    std::fs::canonicalize(&dir).unwrap_or(dir)
}

/// What moves, by id, each with the task it goes with when it is a note that
/// was not named: the items named, and every note attached to a task among
/// them, whether in the trash, the stash or neither.
fn gathered(board: &ItemMap, named: &[u32]) -> Result<BTreeMap<u32, Option<u32>>, EkkoError> {
    let uids = crate::ekko::uid_index(board);
    let task_of = |item: &Item| item.attached_to.as_deref().and_then(|uid| uids.get(uid)).copied();
    let mut moving = BTreeMap::new();
    for id in named {
        let item = &board[id];
        if item.trashed.is_some() {
            return Err(invalid(format!("{id} is in the trash: take it out with --untrash first, or leave it to expire here")));
        }
        if let Some(task) = task_of(item).filter(|task| !named.contains(task)) {
            return Err(invalid(format!(
                "{id} is a note on task {task}, and a note moves with its task: move {task}, or detach {id} first with --attached-to @{id}"
            )));
        }
        moving.insert(*id, None);
    }
    for (id, item) in board {
        if let Some(task) = task_of(item).filter(|task| named.contains(task)) {
            moving.entry(*id).or_insert(Some(task));
        }
    }
    Ok(moving)
}

/// Each link between an item that moves and one that stays, in id order.
fn splits(board: &ItemMap, moving: &BTreeMap<u32, Option<u32>>) -> Vec<Split> {
    let uids = crate::ekko::uid_index(board);
    let mut found = Vec::new();
    for (from, item) in board {
        for (how, uid) in references(item) {
            let Some(&to) = uids.get(uid) else { continue };
            let moves = moving.contains_key(from);
            if moves != moving.contains_key(&to) {
                found.push(Split { from: *from, how, to, moves });
            }
        }
    }
    found
}

/// Every item `item` names by uid, each with how it names it.
///
/// Destructured field by field, as `arriving` is: a field added to any of
/// these does not compile until it is placed here, as a uid another item is
/// named by, or among the fields that name none.
pub(crate) fn references(item: &Item) -> Vec<(&'static str, &str)> {
    let Item {
        blocked_by,
        attached_to,
        supersedes,
        question,
        wait,
        cue,
        artifact,
        comment,
        review,
        id: _,
        date: _,
        timestamp: _,
        description: _,
        is_starred: _,
        boards: _,
        is_task: _,
        is_complete: _,
        in_progress: _,
        due_date: _,
        uid: _,
        updated_at: _,
        rev: _,
        paused: _,
        cancelled: _,
        waiting: _,
        held_by: _,
        done_by: _,
        created_by: _,
        with: _,
        phase: _,
        stashed: _,
        trashed: _,
        handoff: _,
        knowledge: _,
        priority: _,
        unknown: _,
    } = item;
    let mut found: Vec<(&'static str, &str)> = Vec::new();
    found.extend(blocked_by.iter().flatten().map(|uid| ("is blocked by", uid.as_str())));
    found.extend(attached_to.as_deref().map(|uid| ("is attached to", uid)));
    found.extend(supersedes.as_deref().map(|uid| ("supersedes", uid)));
    if let Some(question) = question.as_deref() {
        let Question { cue: proposal, approve, asked_by: _, rev: _, answer: _, allow: _, link: _, applies: _, unknown: _ } = question;
        if let Some(Proposal { gotcha, cue: _, unknown: _ }) = proposal {
            found.push(("proposes a cue for", gotcha));
        }
        if let Some(Approving { artifact, version: _, steps: _, unknown: _ }) = approve {
            found.push(("asks to approve the plan of", artifact));
        }
    }
    if let Some(artifact) = artifact.as_deref() {
        let Artifact { steps, version: _, earlier: _, approved_version: _, unknown: _ } = artifact;
        for Step { task, key: _, text: _, done_when: _, after: _, unknown: _ } in steps {
            found.extend(task.as_deref().map(|uid| ("plans", uid)));
        }
    }
    if let Some(wait) = wait.as_deref() {
        let Wait { on, until: _, by: _, rev: _, over: _, unknown: _ } = wait;
        found.push(("waits on", on));
    }
    if let Some(CueOn { question, cue: _, at: _ }) = cue {
        found.push(("has its cue from", question));
    }
    if let Some(comment) = comment.as_deref() {
        let Comment { reply_to, version: _, quote: _, replacement: _, step: _, sent: _, resolved: _, theme: _, color: _, unknown: _ } = comment;
        found.extend(reply_to.as_deref().map(|uid| ("answers", uid)));
    }
    if let Some(review) = review.as_deref() {
        let Review { comments, answered, verdict: _, version: _, resolved: _, unknown: _ } = review;
        found.extend(comments.iter().map(|uid| ("sends", uid.as_str())));
        found.extend(answered.as_deref().map(|uid| ("answers", uid)));
    }
    found
}

/// `item` as it arrives on the other board, as `id`.
fn arriving(item: &Item, id: u32) -> Item {
    let Item {
        // The board's: the next id there, and the revision and time of the
        // write there, which stamps a field left empty.
        id: _,
        updated_at: _,
        rev: _,
        // Its own.
        date,
        timestamp,
        description,
        is_starred,
        boards,
        is_task,
        is_complete,
        in_progress,
        due_date,
        uid,
        paused,
        cancelled,
        waiting,
        held_by,
        done_by,
        created_by,
        with,
        stashed,
        trashed,
        handoff,
        knowledge,
        priority,
        unknown,
        // Declared there, as `move_to` checks.
        phase,
        // Uids of items that move with it, as `splits` checks.
        blocked_by,
        attached_to,
        supersedes,
        // Each holds some of each kind.
        question,
        wait,
        cue,
        artifact,
        comment,
        review,
    } = item;
    let question = question.as_deref().map(|question| {
        let Question { rev: _, answer, asked_by, cue, allow, link, applies, approve, unknown } = question;
        let answer = answer.as_ref().map(|answer| {
            let Answer { rev: _, text, by, at, unknown } = answer;
            Answer { rev: 0, text: text.clone(), by: by.clone(), at: *at, unknown: unknown.clone() }
        });
        Box::new(Question {
            rev: 0,
            answer,
            asked_by: asked_by.clone(),
            cue: cue.clone(),
            allow: allow.clone(),
            link: link.clone(),
            applies: applies.clone(),
            approve: approve.clone(),
            unknown: unknown.clone(),
        })
    });
    let artifact = artifact.as_deref().map(|artifact| {
        let Artifact { steps, version, earlier, approved_version, unknown } = artifact;
        Box::new(Artifact {
            steps: steps.clone(),
            version: *version,
            earlier: earlier.clone(),
            approved_version: *approved_version,
            unknown: unknown.clone(),
        })
    });
    let comment = comment.as_deref().map(|comment| {
        let Comment { version, quote, replacement, step, reply_to, sent, resolved, theme, color, unknown } = comment;
        Box::new(Comment {
            version: *version,
            quote: quote.clone(),
            replacement: replacement.clone(),
            step: step.clone(),
            reply_to: reply_to.clone(),
            sent: *sent,
            resolved: *resolved,
            theme: theme.clone(),
            color: color.clone(),
            unknown: unknown.clone(),
        })
    });
    let review = review.as_deref().map(|review| {
        let Review { verdict, version, comments, answered, resolved, unknown } = review;
        Box::new(Review { verdict: verdict.clone(), version: *version, comments: comments.clone(), answered: answered.clone(), resolved: *resolved, unknown: unknown.clone() })
    });
    let wait = wait.as_deref().map(|wait| {
        let Wait { rev: _, over, on, until, by, unknown } = wait;
        let over = over.as_ref().map(|over| {
            let Over { rev: _, how, by, at, unknown } = over;
            Over { rev: 0, how: *how, by: by.clone(), at: *at, unknown: unknown.clone() }
        });
        Box::new(Wait { rev: 0, over, on: on.clone(), until: *until, by: by.clone(), unknown: unknown.clone() })
    });
    let cue = cue.as_ref().map(|on| {
        let CueOn { cue, question, at } = on;
        CueOn { cue: cue.clone(), question: question.clone(), at: *at }
    });
    Item {
        id,
        updated_at: None,
        rev: None,
        date: date.clone(),
        timestamp: *timestamp,
        description: description.clone(),
        is_starred: *is_starred,
        boards: boards.clone(),
        is_task: *is_task,
        is_complete: *is_complete,
        in_progress: *in_progress,
        due_date: due_date.clone(),
        uid: uid.clone(),
        paused: *paused,
        cancelled: *cancelled,
        waiting: *waiting,
        held_by: held_by.clone(),
        done_by: done_by.clone(),
        created_by: created_by.clone(),
        with: with.clone(),
        stashed: *stashed,
        trashed: *trashed,
        handoff: *handoff,
        knowledge: *knowledge,
        priority: *priority,
        unknown: unknown.clone(),
        phase: phase.clone(),
        blocked_by: blocked_by.clone(),
        attached_to: attached_to.clone(),
        supersedes: supersedes.clone(),
        question,
        wait,
        cue,
        artifact,
        comment,
        review,
    }
}

/// What a Claude Code session that still runs watches among `moving`, other
/// than the session moving it: an open wait, and a question it asked that has
/// no answer yet. Its wake hook follows this board's file, and would never see
/// either change on another board. Put away, neither is watched.
fn watched(ekko: &Ekko, board: &ItemMap, moving: &BTreeMap<u32, Option<u32>>) -> Vec<(u32, String)> {
    let actor = ekko.actor.as_ref();
    let running = |holder: &Holder| {
        holder.process().is_some_and(|process| process.alive()) && !actor.is_some_and(|actor| actor.is(holder))
    };
    let name = |holder: &Holder| actor.map_or_else(|| holder.label(), |actor| actor.name(holder));
    let mut found = Vec::new();
    for id in moving.keys() {
        let item = &board[id];
        if item.trashed.is_some() || item.stashed.is_some() {
            continue;
        }
        if let Some(wait) = item.wait.as_ref().filter(|wait| wait.over.is_none() && running(&wait.by)) {
            found.push((*id, format!("the wait of {}", name(&wait.by))));
        }
        let asked = item.question.as_ref().filter(|question| question.answer.is_none()).and_then(|question| question.asked_by.as_ref());
        if let Some(by) = asked.filter(|by| running(by)) {
            found.push((*id, format!("a question {} asked, not answered yet", name(by))));
        }
    }
    found
}

/// How a board an item moved to is named: `project NAME`, or the default
/// board, which has no name.
pub(crate) fn board_name(project: Option<&str>) -> String {
    project.map_or_else(|| "the default board".to_string(), |name| format!("project {name}"))
}

fn invalid(message: String) -> EkkoError {
    EkkoError::InvalidInput(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::holder::{Actor, Process};
    use crate::item::{Cue, State};
    use serde_json::{json, Value};
    use std::fs;

    const NOW: i64 = 1_790_000_000_000;

    /// A home whose default board is one board, and a project `notes` in the
    /// folder `work/notes` the other. Removed when dropped.
    struct Home {
        home: PathBuf,
        folder: PathBuf,
    }

    impl Home {
        fn new(tag: &str) -> Home {
            let home = crate::paths::test_dir(&format!("ekko-move-{tag}"));
            fs::create_dir_all(&home).unwrap();
            let home = fs::canonicalize(&home).unwrap();
            let folder = home.join("work").join("notes");
            fs::create_dir_all(&folder).unwrap();
            crate::project::init(&home, &folder, None, None, NOW).unwrap();
            Home { home, folder }
        }

        fn default_board(&self, actor: &Actor) -> Ekko {
            let location = directory::locate(&self.home, &self.home, None, None, None).unwrap();
            assert!(location.project.is_none());
            Ekko::at(&location).unwrap().acting_as(actor.clone())
        }

        fn project(&self, actor: &Actor) -> Ekko {
            let location = directory::locate(&self.home, &self.home, None, None, Some("notes")).unwrap();
            Ekko::at(&location).unwrap().in_folder(Some(self.folder.clone())).acting_as(actor.clone())
        }

        fn move_to(&self, from: &Ekko, destination: &str, ids: &[&str]) -> Result<Vec<Carried>, EkkoError> {
            self.move_forced(from, destination, ids, false)
        }

        fn move_forced(&self, from: &Ekko, destination: &str, ids: &[&str], force: bool) -> Result<Vec<Carried>, EkkoError> {
            let ids: Vec<String> = ids.iter().map(|id| id.to_string()).collect();
            let Outcome::MovedTo { items, .. } = move_to(from, &self.home, &self.home, destination, &ids, force)? else {
                unreachable!("--move-to answers with what it moved")
            };
            Ok(items)
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.home).ok();
        }
    }

    /// `ops`, one write, as a batch through the MCP server makes it.
    fn batch(ekko: &Ekko, ops: &[Value]) {
        let mut draft = crate::ops::Draft::open(ekko).unwrap();
        for value in ops {
            draft.apply(&serde_json::from_value(value.clone()).unwrap()).unwrap();
        }
        draft.commit(false).unwrap();
    }

    /// One write that makes `change` to the board.
    fn write(ekko: &Ekko, change: impl FnOnce(&mut ItemMap)) {
        let _lock = ekko.storage.acquire_lock().unwrap();
        let before = ekko.storage.get_shared().unwrap();
        let mut data = ItemMap::clone(&before);
        change(&mut data);
        ekko.save_against(&before, &mut data).unwrap();
    }

    fn board(ekko: &Ekko) -> ItemMap {
        ItemMap::clone(&ekko.storage.get_shared().unwrap())
    }

    fn uid(board: &ItemMap, id: u32) -> String {
        board[&id].uid.clone().unwrap()
    }

    /// A task goes with every note attached to it, and keeps who it is: uid,
    /// author, dates, text, boards and fields, and the uid its notes are
    /// attached by. It takes the next id there, every revision it carries is
    /// the write's there, and the id it had here says where it went, to a
    /// lookup and to changes.
    #[test]
    fn a_task_goes_with_its_notes_and_keeps_who_it_is() {
        let home = Home::new("keeps");
        let person = Actor::person();
        let (here, there) = (home.default_board(&person), home.project(&person));
        batch(&there, &[json!({"op": "create", "text": "one there"}), json!({"op": "create", "text": "two there"})]);
        batch(&here, &[json!({"op": "create", "text": "stays"})]);
        batch(&here, &[json!({"op": "create", "text": "file the notes", "boards": ["zk"], "priority": 2, "due": "2026-10-01"})]);
        batch(&here, &[json!({"op": "create", "kind": "note", "text": "why", "attached_to": 2})]);
        batch(&here, &[json!({"op": "create", "kind": "decision", "text": "settled", "attached_to": 2})]);
        write(&here, |data| {
            let answer = Answer { text: "yes".into(), by: None, at: NOW, rev: 0, unknown: BTreeMap::new() };
            let question = Question { asked_by: None, rev: 0, answer: Some(answer), cue: None, allow: None, link: None, applies: None, approve: None, unknown: BTreeMap::new() };
            data.get_mut(&3).unwrap().question = Some(Box::new(question));
        });
        let before = board(&here);
        let cursor = here.storage.get_counters().unwrap().revision as i64;

        let moved = home.move_to(&here, "notes", &["2"]).unwrap();
        assert_eq!(
            moved,
            [
                Carried { id: 2, as_id: 3, uid: uid(&before, 2), note_of: None, cue: None },
                Carried { id: 3, as_id: 4, uid: uid(&before, 3), note_of: Some(2), cue: None },
                Carried { id: 4, as_id: 5, uid: uid(&before, 4), note_of: Some(2), cue: None },
            ]
        );
        let (left, arrived) = (board(&here), board(&there));
        assert_eq!(left.keys().copied().collect::<Vec<_>>(), [1], "the rest stays");
        let revision = there.storage.get_counters().unwrap().revision;
        for (id, as_id) in [(2, 3), (3, 4), (4, 5)] {
            let (was, is) = (&before[&id], &arrived[&as_id]);
            assert_eq!(
                (&is.uid, &is.created_by, &is.date, is.timestamp, &is.description, &is.boards),
                (&was.uid, &was.created_by, &was.date, was.timestamp, &was.description, &was.boards)
            );
            assert_eq!(
                (is.priority, &is.due_date, &is.attached_to, is.knowledge, State::of(is)),
                (was.priority, &was.due_date, &was.attached_to, was.knowledge, State::of(was))
            );
            assert_eq!(is.rev, Some(revision), "{as_id} carries the revision of the write there");
        }
        let (was, is) = (before[&3].question.as_ref().unwrap(), arrived[&4].question.as_ref().unwrap());
        assert_ne!(was.rev, revision, "a revision here that the check can tell from one there");
        assert_eq!((is.rev, is.answer.as_ref().unwrap().rev), (revision, revision), "a question's revisions are the board's too");

        for asked in ["2".to_string(), uid(&before, 2)] {
            let Err(EkkoError::Moved { to, .. }) = here.validate_ids(std::slice::from_ref(&asked), &left) else {
                panic!("{asked} should say where it went")
            };
            assert_eq!((to.project.as_deref(), to.as_id), (Some("notes"), 3));
        }
        let changes = crate::agent::changes(&here, cursor).unwrap();
        let went: Vec<(u32, Option<u32>)> =
            changes.removed.iter().map(|gone| (gone.id, gone.moved.as_ref().map(|to| to.as_id))).collect();
        assert_eq!(went, [(2, Some(3)), (3, Some(4)), (4, Some(5))]);
        assert!(changes.text().contains("   2. [moved to project notes as 3] file the notes"), "{}", changes.text());
    }

    /// Blocked by and supersedes, from either side, hold only on one board:
    /// splitting either is refused, naming it, and nothing moves. Moved
    /// together, they hold there.
    #[test]
    fn a_link_between_an_item_that_moves_and_one_that_stays_is_refused() {
        let home = Home::new("links");
        let person = Actor::person();
        let (here, there) = (home.default_board(&person), home.project(&person));
        batch(
            &here,
            &[
                json!({"op": "create", "text": "blocker"}),
                json!({"op": "create", "text": "blocked", "blocked_by": ["$1"]}),
                json!({"op": "create", "kind": "decision", "text": "older"}),
                json!({"op": "create", "kind": "decision", "text": "newer", "supersedes": "$3"}),
            ],
        );
        let before = board(&here);
        let split = |ids: &[&str]| match home.move_to(&here, "notes", ids) {
            Err(EkkoError::SplitLinks(splits)) => splits,
            other => panic!("{other:?}"),
        };
        assert_eq!(split(&["2"]), [Split { from: 2, how: "is blocked by", to: 1, moves: true }]);
        assert_eq!(split(&["1"]), [Split { from: 2, how: "is blocked by", to: 1, moves: false }]);
        assert_eq!(split(&["4"]), [Split { from: 4, how: "supersedes", to: 3, moves: true }]);
        assert_eq!(split(&["3"]), [Split { from: 4, how: "supersedes", to: 3, moves: false }]);
        assert_eq!(board(&here), before, "nothing left");
        assert!(board(&there).is_empty(), "nothing arrived");

        home.move_to(&here, "notes", &["1", "2", "3", "4"]).unwrap();
        let arrived = board(&there);
        assert_eq!(arrived[&2].blocked_by, Some(vec![uid(&arrived, 1)]));
        assert_eq!(arrived[&4].supersedes, Some(uid(&arrived, 3)));
    }

    /// A note attached to a task moves with it, one in the trash included,
    /// and not by itself; what is in the trash does not move by name.
    #[test]
    fn a_note_moves_only_with_its_task() {
        let home = Home::new("notes");
        let person = Actor::person();
        let (here, there) = (home.default_board(&person), home.project(&person));
        batch(
            &here,
            &[
                json!({"op": "create", "text": "task"}),
                json!({"op": "create", "kind": "note", "text": "on it", "attached_to": "$1"}),
                json!({"op": "create", "kind": "note", "text": "on it, thrown away", "attached_to": "$1"}),
                json!({"op": "create", "text": "thrown away"}),
            ],
        );
        write(&here, |data| {
            for id in [3, 4] {
                data.get_mut(&id).unwrap().trashed = Some(NOW);
            }
        });
        let refused = |ids: &[&str]| match home.move_to(&here, "notes", ids) {
            Err(EkkoError::InvalidInput(message)) => message,
            other => panic!("{other:?}"),
        };
        assert!(refused(&["2"]).starts_with("2 is a note on task 1, and a note moves with its task"), "{}", refused(&["2"]));
        assert!(refused(&["4"]).starts_with("4 is in the trash"), "{}", refused(&["4"]));

        let moved = home.move_to(&here, "notes", &["1"]).unwrap();
        assert_eq!(moved.iter().map(|item| (item.id, item.note_of)).collect::<Vec<_>>(), [(1, None), (2, Some(1)), (3, Some(1))]);
        assert_eq!(board(&there)[&3].trashed, Some(NOW), "still in the trash, with the time it has left");
    }

    /// A phase goes only where it is declared.
    #[test]
    fn a_phase_must_be_declared_where_the_item_goes() {
        let home = Home::new("phases");
        let person = Actor::person();
        let (here, there) = (home.default_board(&person), home.project(&person));
        there.set_phases(&["build".to_string()]).unwrap();
        batch(&there, &[json!({"op": "create", "text": "in a phase", "phase": "build"})]);
        let Err(EkkoError::InvalidInput(message)) = home.move_to(&there, "~", &["1"]) else { panic!() };
        assert!(message.contains("1 is in phase build, which the default board does not declare"), "{message}");

        here.set_phases(&["build".to_string()]).unwrap();
        home.move_to(&there, "~", &["1"]).unwrap();
        assert_eq!(board(&here)[&1].phase.as_deref(), Some("build"));
    }

    /// A cue the user turned on that names no folder guards the board it is
    /// on, so after a move the board it went to, and the reply says so; one
    /// with a folder of its own guards that folder wherever it goes. No
    /// folder is written out for the first.
    #[test]
    fn a_cue_naming_no_folder_guards_the_board_it_went_to() {
        let home = Home::new("cues");
        let person = Actor::person();
        let (here, there) = (home.default_board(&person), home.project(&person));
        let cue = |folder: Option<&str>| CueOn {
            cue: Cue { command: "cargo".into(), words: vec!["fmt".into()], folder: folder.map(str::to_string), unknown: BTreeMap::new() },
            question: "18d0000000000000-1".into(),
            at: NOW,
        };
        batch(&here, &[json!({"op": "create", "kind": "gotcha", "text": "anywhere"})]);
        batch(&there, &[json!({"op": "create", "kind": "gotcha", "text": "in the project"})]);
        batch(&there, &[json!({"op": "create", "kind": "gotcha", "text": "in its own folder"})]);
        write(&here, |data| data.get_mut(&1).unwrap().cue = Some(cue(None)));
        write(&there, |data| {
            data.get_mut(&1).unwrap().cue = Some(cue(None));
            data.get_mut(&2).unwrap().cue = Some(cue(Some("/srv")));
        });

        let project = home.folder.display().to_string();
        let rescoped = |guarded: &str, guards: &str| Some(Rescoped { guarded: guarded.into(), guards: guards.into() });
        let went = home.move_to(&here, "notes", &["1"]).unwrap();
        assert_eq!(went[0].cue, rescoped("/", &project), "from the whole machine to the project's folder");
        let back = home.move_to(&there, "~", &["1", "2"]).unwrap();
        assert_eq!(back[0].cue, rescoped(&project, "/"), "from the project's folder to the whole machine");
        assert_eq!(back[1].cue, None, "a folder of its own guards the same place");
        let folder = |ekko: &Ekko, id: u32| board(ekko)[&id].cue.as_ref().unwrap().cue.folder.clone();
        assert_eq!(folder(&there, 3), None, "none written out on the way to the project");
        assert_eq!(folder(&here, 2), None, "none written out on the way back");
        assert_eq!(folder(&here, 3).as_deref(), Some("/srv"), "a folder of its own stays its own");

        // A cue that is not on -- its gotcha stashed -- guards nothing here,
        // so the reply says nothing of it.
        write(&here, |data| data.get_mut(&2).unwrap().stashed = Some(NOW));
        let stashed = home.move_to(&here, "notes", &["2"]).unwrap();
        assert_eq!(stashed[0].cue, None, "a cue that is off");
    }

    /// A task a running session holds stays, for another session, as any
    /// write to it does; a wait and an unanswered question a running session
    /// keeps stay for anyone, since its wake hook follows this board only.
    /// --force moves them all.
    #[test]
    fn what_a_running_session_holds_or_watches_stays_unless_forced() {
        let home = Home::new("watched");
        let running = Actor { process: Process::of(std::process::id()), ..Actor::default() };
        let another = Actor { process: Some(Process { pid: u32::MAX, start: 0, boot: "another".into() }), ..Actor::default() };
        let person = Actor::person();
        let theirs = home.default_board(&running);
        batch(
            &theirs,
            &[
                json!({"op": "create", "text": "held"}),
                json!({"op": "set_state", "items": ["$1"], "state": "progress"}),
                json!({"op": "create", "kind": "note", "text": "asked"}),
                json!({"op": "create", "text": "waited on"}),
                json!({"op": "create", "kind": "note", "text": "waiting", "attached_to": "$4"}),
                json!({"op": "create", "text": "done by them"}),
                json!({"op": "set_state", "items": ["$6"], "state": "done"}),
            ],
        );
        write(&theirs, |data| {
            let by = running.holder(NOW);
            let question = Question { asked_by: Some(by.clone()), rev: 0, answer: None, cue: None, allow: None, link: None, applies: None, approve: None, unknown: BTreeMap::new() };
            data.get_mut(&2).unwrap().question = Some(Box::new(question));
            let on = data[&3].uid.clone().unwrap();
            let wait = Wait { on, until: crate::item::Until::Done, by, rev: 0, over: None, unknown: BTreeMap::new() };
            data.get_mut(&4).unwrap().wait = Some(Box::new(wait));
        });
        let (here, elsewhere) = (home.default_board(&person), home.default_board(&another));
        let watched = |ekko: &Ekko, ids: &[&str]| match home.move_to(ekko, "notes", ids) {
            Err(EkkoError::Watched(watched)) => watched.into_iter().map(|(id, _)| id).collect::<Vec<_>>(),
            other => panic!("{other:?}"),
        };
        assert_eq!(watched(&here, &["2"]), [2], "a question it asked");
        assert_eq!(watched(&here, &["3"]), [4], "a wait, on a note that goes with its task");
        assert!(matches!(home.move_to(&elsewhere, "notes", &["1"]), Err(EkkoError::Held(held)) if held[0].0 == 1));
        let theirs_now = running.holder(NOW);
        let is_theirs = |holder: &Option<Holder>| holder.as_ref().and_then(Holder::process) == theirs_now.process();
        assert!(is_theirs(&board(&here)[&1].held_by) && is_theirs(&board(&here)[&5].done_by));
        let there = home.project(&person);
        home.move_to(&here, "notes", &["1"]).unwrap();
        assert!(!board(&here).contains_key(&1), "a person takes a held task, as with any write");
        assert!(is_theirs(&board(&there)[&1].held_by), "and it stays in the hands that held it");

        let moved = home.move_forced(&elsewhere, "notes", &["2", "3", "5"], true).unwrap();
        assert_eq!(moved.len(), 4);
        assert!(board(&here).is_empty());
        assert!(is_theirs(&board(&there)[&5].done_by), "whose work it was, not the mover's");
    }

    /// An item from before uids gets one to move by, so a move run again finds
    /// it, and its old id says where it went.
    #[test]
    fn an_item_from_before_uids_is_given_one_to_move() {
        let home = Home::new("legacy");
        let person = Actor::person();
        let (here, there) = (home.default_board(&person), home.project(&person));
        batch(&here, &[json!({"op": "create", "text": "from taskbook"})]);
        write(&here, |data| data.get_mut(&1).unwrap().uid = None);
        assert_eq!(board(&here)[&1].uid, None);

        let moved = home.move_to(&here, "notes", &["1"]).unwrap();
        assert_eq!(board(&there)[&1].uid.as_ref(), Some(&moved[0].uid));
        assert!(matches!(here.validate_ids(&["1".to_string()], &board(&here)), Err(EkkoError::Moved { .. })));
    }

    /// Run again after it wrote the other board and the redirect, and failed
    /// before this board, a move finds what it already took there by uid, and
    /// takes it off here, with one redirect for each id.
    #[test]
    fn a_move_run_again_after_a_failure_takes_each_item_once() {
        let home = Home::new("again");
        let person = Actor::person();
        let (here, there) = (home.default_board(&person), home.project(&person));
        batch(&here, &[json!({"op": "create", "text": "twice?"}), json!({"op": "create", "kind": "note", "text": "on it", "attached_to": "$1"})]);
        let before = board(&here);
        write(&there, |data| {
            let mut copy = before[&1].clone();
            copy.id = 7;
            data.insert(7, copy);
        });
        let redirect = Moved { id: 1, uid: uid(&before, 1), project: Some("notes".into()), as_id: 7, at: NOW, unknown: BTreeMap::new() };
        here.storage.set_moved(&[redirect]).unwrap();

        let moved = home.move_to(&here, "notes", &["1"]).unwrap();
        assert_eq!(moved.iter().map(|item| (item.id, item.as_id)).collect::<Vec<_>>(), [(1, 7), (2, 8)]);
        let arrived = board(&there);
        assert_eq!(arrived.values().filter(|item| item.uid == before[&1].uid).count(), 1);
        assert!(board(&here).is_empty());
        let ids: Vec<u32> = here.storage.get_moved().unwrap().iter().map(|moved| moved.id).collect();
        assert_eq!(ids, [1, 2]);
    }

    /// Moved back where it once was, an item is found there by its uid again,
    /// and each id it left says where it went from there, as Jira stacks the
    /// keys an issue had.
    #[test]
    fn an_item_that_comes_back_is_found_by_its_uid() {
        let home = Home::new("back");
        let person = Actor::person();
        let (here, there) = (home.default_board(&person), home.project(&person));
        batch(&here, &[json!({"op": "create", "text": "round trip"})]);
        let uid = uid(&board(&here), 1);
        home.move_to(&here, "notes", &["1"]).unwrap();
        home.move_to(&there, "~", &[&uid]).unwrap();

        assert_eq!(here.validate_ids(std::slice::from_ref(&uid), &board(&here)).unwrap(), [2]);
        let went = |ekko: &Ekko, asked: &str| match ekko.validate_ids(&[asked.to_string()], &board(ekko)) {
            Err(EkkoError::Moved { to, .. }) => (to.project, to.as_id),
            other => panic!("{other:?}"),
        };
        assert_eq!(went(&here, "1"), (Some("notes".to_string()), 1));
        assert_eq!(went(&there, "1"), (None, 2));
        assert_eq!(went(&there, &uid), (None, 2));
        home.move_to(&here, "notes", &["2"]).unwrap();
        assert_eq!(went(&here, &uid), (Some("notes".to_string()), 2), "by uid, the last time it left");
        assert!(matches!(home.move_to(&here, "~", &["2"]), Err(EkkoError::InvalidInput(message)) if message.contains("is this board")));
    }

    /// Every field is placed: on an item that sets each one, `arriving` keeps
    /// all but the board's, and `references` names every uid.
    #[test]
    fn every_field_is_kept_restamped_or_checked() {
        let holder = json!({"pid": 1, "start": 2, "boot": "b", "profile": "p", "tty": "t", "conversation": "c", "since": 3});
        let mut value = json!({
            "_id": 5, "_date": "Mon Sep 28 2026", "_timestamp": 10, "description": "d", "isStarred": true,
            "boards": ["@b"], "_isTask": true, "isComplete": false, "inProgress": true, "dueDate": "2026-10-01",
            "uid": "u", "updatedAt": 11, "rev": 12, "paused": false, "cancelled": false, "waiting": false,
            "heldBy": holder, "doneBy": holder, "createdBy": holder, "with": "ana", "phase": "build",
            "blockedBy": ["b1", "b2"], "stashed": 13, "trashed": 14, "attachedTo": "t", "handoff": true,
            "knowledge": "decision", "supersedes": "s", "priority": 3, "a later field": "kept"
        });
        value["question"] = json!({
            "askedBy": holder, "rev": 15,
            "answer": {"text": "yes", "by": holder, "at": 16, "rev": 17},
            "cue": {"gotcha": "g", "cue": {"command": "gh", "words": ["pr"], "folder": "/f"}},
            "allow": {"code": "c", "tool": "Bash", "call": "ls", "cwd": "/", "reasons": ["r"], "used": {"toolUseId": "x", "at": 18}},
            "link": {"projects": ["p1", "p2"]},
            "applies": "Sim",
            "approve": {"artifact": "a", "version": 2, "steps": ["s1"]}
        });
        value["artifact"] = json!({
            "steps": [{"key": "s1", "text": "Step", "doneWhen": "d", "after": ["s0"], "task": "t1"}],
            "version": 3,
            "earlier": [{"version": 2, "at": 23, "text": "before"}],
            "approvedVersion": 2
        });
        value["wait"] = json!({"on": "w", "until": "done", "by": holder, "rev": 19, "over": {"how": "done", "by": holder, "at": 20, "rev": 21}});
        value["cue"] = json!({"command": "cargo", "words": ["fmt"], "question": "q", "at": 22});
        let item: Item = serde_json::from_value(value).unwrap();
        assert_eq!(item.unknown.keys().collect::<Vec<_>>(), ["a later field"], "every other key is a field");
        assert!(item.question.as_ref().unwrap().unknown.is_empty(), "every key of the question is a field");
        let artifact = item.artifact.as_ref().unwrap();
        assert!(artifact.unknown.is_empty() && artifact.steps[0].unknown.is_empty() && artifact.earlier[0].unknown.is_empty(), "every key of the artifact is a field");

        let arrived = arriving(&item, 99);
        let board_s = |item: &Item| {
            let mut value = serde_json::to_value(item).unwrap();
            for key in ["_id", "rev", "updatedAt"] {
                value.as_object_mut().unwrap().remove(key);
            }
            for path in ["/question", "/question/answer", "/wait", "/wait/over"] {
                value.pointer_mut(path).unwrap().as_object_mut().unwrap().remove("rev");
            }
            value
        };
        assert_eq!(board_s(&arrived), board_s(&item), "all but the board's fields arrive as they left");
        let (question, wait) = (arrived.question.as_ref().unwrap(), arrived.wait.as_ref().unwrap());
        assert_eq!((arrived.id, arrived.rev, arrived.updated_at), (99, None, None));
        assert_eq!(
            [question.rev, question.answer.as_ref().unwrap().rev, wait.rev, wait.over.as_ref().unwrap().rev],
            [0; 4],
            "left for the write there to stamp"
        );
        assert_eq!(arrived.cue.as_ref().unwrap().cue.folder, None, "a cue naming no folder arrives naming none");
        assert_eq!(
            references(&item),
            [
                ("is blocked by", "b1"),
                ("is blocked by", "b2"),
                ("is attached to", "t"),
                ("supersedes", "s"),
                ("proposes a cue for", "g"),
                ("asks to approve the plan of", "a"),
                ("plans", "t1"),
                ("waits on", "w"),
                ("has its cue from", "q"),
            ]
        );
    }
}
