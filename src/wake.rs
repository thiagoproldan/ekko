//! Telling a Claude Code session what it waited on (task 389): its waits
//! that another's write ended, and the answers to questions its ask left
//! open -- each once, whichever way reaches it first. The plugin's
//! FileChanged hook runs `ekko --wake --hook` in the background whenever the
//! board's file changes, and its exit 2 wakes a session sitting idle; the
//! session's next ekko reply carries the same lines, for a client without
//! that hook. The session holding a task is told, once and only in a reply,
//! that another waits on it. So are the sessions working an artifact, by the
//! hook too, of the user's feedback from its page (task 1108), and the session
//! holding a task of each note the user or another session writes on it
//! (task 1648), which the hook tells it between its tool calls. A wait on an
//! item of a linked board is kept on that board, beside the item, and told
//! the same ways: the session watches each linked board's file too (task
//! 1443).
//!
//! What was told is kept outside the board, since reading the board writes
//! nothing to it: a marker file per line, in a folder per Claude Code
//! process, made with `create_new` so that two hook runs racing on one
//! write cannot both wake the session.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::agent;
use crate::ekko::{Ekko, EkkoError};
use crate::holder::{Actor, Holder, Process};
use crate::item::{How, Item, State};

/// How much of a wait's text, or of an answer, a line quotes.
const QUOTED: usize = 300;

/// How much of the title of the item waited on a line quotes.
const TITLED: usize = 80;

/// How much of a note written for a task the session holds a line quotes:
/// more than of a wait, as it is what another session has to say to this
/// one, which reads the rest with context.
const NOTED: usize = 1000;

/// What one Claude Code process was told, and which of the questions it
/// asked it was left waiting on: a marker file each.
pub struct Told {
    dir: PathBuf,
}

impl Told {
    /// The markers of `process`, under the ekko state directory in `home`.
    pub fn of(home: &Path, process: &Process) -> Told {
        Told::within(&told_dir(home), process)
    }

    /// The markers of `process` in `dir`, which holds every process's.
    pub(crate) fn within(dir: &Path, process: &Process) -> Told {
        let boot: String = process.boot.chars().filter(char::is_ascii_alphanumeric).take(8).collect();
        Told { dir: dir.join(format!("{boot}-{}-{}", process.pid, process.start)) }
    }

    /// What the hook's last run in this process heard, where one recorded it.
    pub(crate) fn last_heard(&self) -> Option<Heard> {
        serde_json::from_str(&fs::read_to_string(self.dir.join("heard")).ok()?).ok()
    }

    /// Records, in `guarded`, that the PreToolUse guard ran in this process
    /// for the Bash call `tool_use_id`, at `at`: what `ekko --doctor`
    /// compares with the session's last Bash call, to tell a session whose
    /// guard does not run (task 1284). Written whole on every run, best
    /// effort, as `heard` is.
    pub(crate) fn guarded(&self, tool_use_id: &str, at: i64) {
        if let Ok(bytes) = serde_json::to_vec(&Guarded { tool_use_id: tool_use_id.to_string(), at }) {
            let _ = crate::guard::write_atomically(&self.dir.join("guarded"), &bytes);
        }
    }

    /// The guard's last run in this process, where one recorded it.
    pub(crate) fn last_guarded(&self) -> Option<Guarded> {
        serde_json::from_str(&fs::read_to_string(self.dir.join("guarded")).ok()?).ok()
    }

    fn has(&self, key: &str) -> bool {
        self.dir.join(key).exists()
    }

    /// Makes the marker `key`: true only for the call that made it, false
    /// where it was there already or cannot be written -- then nothing is
    /// told, rather than the same line on every reply.
    fn mark(&self, key: &str) -> bool {
        fs::create_dir_all(&self.dir).is_ok()
            && fs::OpenOptions::new().write(true).create_new(true).open(self.dir.join(key)).is_ok()
    }

    /// Records that this process's ask came back without an answer to the
    /// question `uid`: an answer given later, in another terminal, is news
    /// to it. One given while the ask waited came back in its reply.
    pub fn left_open(&self, uid: &str) {
        self.mark(&format!("open-{uid}"));
    }

    /// Records, in `heard`, the version of the board's file this run of the
    /// hook saw and the revision it read: what `ekko --doctor` compares with
    /// the board's last write, to tell a session whose hook went deaf from
    /// one with nothing to hear (task 1281). Written whole on every run, best
    /// effort: a hook run on every write must not fail on it.
    fn heard(&self, ekko: &Ekko) {
        use std::os::unix::fs::MetadataExt as _;
        let (Ok(file), Ok(counters)) = (fs::metadata(ekko.storage.storage_path()), ekko.storage.get_counters()) else { return };
        let heard = Heard {
            inode: file.ino(),
            mtime_ns: file.mtime().saturating_mul(1_000_000_000).saturating_add(file.mtime_nsec()),
            size: file.len(),
            revision: counters.revision,
            at: chrono::Local::now().timestamp_millis(),
        };
        if let Ok(bytes) = serde_json::to_vec(&heard) {
            let _ = crate::guard::write_atomically(&self.dir.join("heard"), &bytes);
        }
    }
}

/// The version of the board's file a wake hook run saw: its inode, its
/// modification time in nanoseconds and its size, as a rename or a write in
/// place changes them; the board revision it read; and when, in
/// milliseconds.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Heard {
    pub inode: u64,
    pub mtime_ns: i64,
    pub size: u64,
    pub revision: u64,
    pub at: i64,
}

/// The Bash call a guard run in a process saw, by its tool_use_id, and when,
/// in milliseconds.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Guarded {
    pub tool_use_id: String,
    pub at: i64,
}

/// Where the markers of every process are kept.
pub(crate) fn told_dir(home: &Path) -> PathBuf {
    agent::state_dir(home).join("told")
}

/// Removes the markers of processes that no longer run: a resumed
/// conversation runs in a new process, and starts from the cursor its hook
/// served it. Best effort.
fn forget_ended(home: &Path) {
    let Ok(entries) = fs::read_dir(told_dir(home)) else { return };
    let boot: Option<String> =
        fs::read_to_string("/proc/sys/kernel/random/boot_id").ok().map(|boot| boot.chars().filter(char::is_ascii_alphanumeric).take(8).collect());
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let mut parts = name.splitn(3, '-');
        let (Some(of_boot), Some(pid), Some(start)) = (parts.next(), parts.next(), parts.next()) else { continue };
        let (Ok(pid), Ok(start)) = (pid.parse::<u32>(), start.parse::<u64>()) else { continue };
        let running = boot.as_deref() == Some(of_boot) && Process::of(pid).is_some_and(|now| now.start == start);
        if !running {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

/// What `me` has not been told yet on the board `ekko` has open, each line
/// marked as told as it is returned: the user's feedback on an artifact it
/// works, its waits that another's write ended, the answers to the
/// questions its ask left open, and the notes others write on the tasks it
/// holds -- and, with `holding`, the waits other sessions keep on the tasks
/// it holds. Only what moved after `since`, the cursor its session last
/// started from: its prime, or the changes its resume was given, already
/// said what came before.
pub fn untold(ekko: &Ekko, me: &Actor, told: &Told, since: u64, holding: bool) -> Result<Vec<String>, EkkoError> {
    let all = ekko.storage.get_shared()?;
    let registry = me.registry.as_ref();
    // Its own, or its conversation's from before a restart or from the
    // process Claude Code moved it out of, as a question's answer is
    // (`agent::prime`).
    let mine = |holder: &Holder| {
        me.is(holder)
            || (!holder.alive()
                && me.conversation().is_some_and(|now| holder.conversation_in(registry).as_deref() == Some(now.as_str())))
            || me.continues(holder)
    };
    let by_uid: HashMap<&str, &Item> = all.values().filter_map(|item| Some((item.uid.as_deref()?, item))).collect();
    let name = |holder: &Holder| holder.label_in(registry);
    let titled = |item: &Item| agent::headline(&item.description, item.is_task, TITLED);

    let mut lines = Vec::new();
    // The user's feedback from an artifact's page (task 1108): each review,
    // and each comment sent alone, told once to the sessions working the
    // artifact -- holding it or a step's task in progress, or having asked a
    // question about it still open when the feedback came. Told before the
    // answers, so a review that answered a question this session's ask left
    // open is told once, as the review.
    let holds = |item: &Item| State::of(item) == Some(State::Progress) && item.held_by.as_ref().is_some_and(mine);
    let works = |artifact: &Item, rev: u64| {
        let steps = artifact.artifact.as_deref().map_or(&[][..], |plan| plan.steps.as_slice());
        holds(artifact)
            || steps.iter().filter_map(|step| by_uid.get(step.task.as_deref()?)).any(|task| holds(task))
            || all.values().any(|asked| {
                asked.trashed.is_none()
                    && asked.attached_to.is_some()
                    && asked.attached_to == artifact.uid
                    && asked.question.as_ref().is_some_and(|question| {
                        question.asked_by.as_ref().is_some_and(mine) && question.answer.as_ref().is_none_or(|answer| answer.rev >= rev)
                    })
            })
    };
    let reviewed: HashSet<&str> = all.values().filter_map(|note| note.review.as_deref()).flat_map(|review| review.comments.iter().map(String::as_str)).collect();
    for note in all.values().filter(|note| crate::artifact::feedback(note)) {
        let (Some(uid), Some(rev)) = (note.uid.as_deref(), note.rev) else { continue };
        if rev <= since || (note.review.is_none() && reviewed.contains(uid)) {
            continue;
        }
        let Some(artifact) = note.attached_to.as_deref().and_then(|on| by_uid.get(on).copied()).filter(|item| item.artifact.is_some()) else { continue };
        if !works(artifact, rev) || !told.mark(&format!("feedback-{uid}")) {
            continue;
        }
        let answered = note.review.as_deref().and_then(|review| review.answered.as_deref()).and_then(|question| by_uid.get(question).copied());
        let asked_here = answered.filter(|question| question.question.as_ref().is_some_and(|asked| asked.asked_by.as_ref().is_some_and(mine)));
        if let Some(question) = asked_here.and_then(|question| question.uid.as_deref()) {
            // An answer to this session's question came back in the reply of
            // the ask that waited on it, naming the review; one its ask left
            // open is told here, within the review.
            if !told.has(&format!("open-{question}")) {
                continue;
            }
            told.mark(&format!("answer-{question}"));
        }
        let on = format!("artifact {} ({})", artifact.id, agent::clip(crate::ekko::title(&artifact.description), TITLED));
        if let Some(review) = note.review.as_deref() {
            let (told, own) = crate::feedback::review_told(note, review, |uid| by_uid.get(uid).copied());
            let says = if own { format!(" It says: {}", agent::clip(&note.description, QUOTED)) } else { String::new() };
            lines.push(format!("Review {} from the user, on {on}: {told}{says}", note.id));
        } else if let Some(comment) = note.comment.as_deref() {
            // A step's task, by the step's key, while the board holds it.
            let task_of = |key: &str| {
                let plan = artifact.artifact.as_deref()?;
                let uid = plan.steps.iter().find(|step| step.key == key)?.task.as_deref()?;
                by_uid.get(uid).copied()
            };
            let step = comment.step.as_deref().map(|key| match task_of(key) {
                Some(task) => format!("step {key}, task {} ({})", task.id, agent::clip(crate::ekko::title(&task.description), TITLED)),
                None => format!("step {key}"),
            });
            if crate::artifact::is_start(note) {
                // Start on the page (task 1111): taking the task up resolves it.
                lines.push(format!(
                    "Start from the user, on the page of {on}: {}. Set its task in progress and take it up, which resolves comment {}.",
                    step.unwrap_or_default(),
                    note.id
                ));
                continue;
            }
            let at = match (&comment.quote, step) {
                (Some(quote), _) => format!(", on \"{}\"", agent::clip(&quote.exact, TITLED)),
                (None, Some(step)) => format!(", on {step}"),
                (None, None) => String::new(),
            };
            lines.push(format!("Comment {} from the user, sent alone from the page of {on}{at}: {}", note.id, agent::clip(&note.description, QUOTED)));
        }
    }
    for note in all.values().filter(|note| note.trashed.is_none()) {
        let Some(uid) = note.uid.as_deref() else { continue };
        if let Some(wait) = &note.wait {
            let target = by_uid.get(wait.on.as_str()).copied();
            let named = target.map_or_else(|| "What it waited on".to_string(), |item| format!("{} ({})", item.id, titled(item)));
            match &wait.over {
                // A wait its own write ended, or one it dropped, it knows of.
                Some(over)
                    if over.rev > since
                        && over.how != How::Dropped
                        && mine(&wait.by)
                        && !over.by.as_ref().is_some_and(|by| me.is(by)) =>
                {
                    if told.mark(&format!("over-{uid}")) {
                        let waited = format!("This session waited on it (note {}) to: {}", note.id, agent::clip(&note.description, QUOTED));
                        if over.how == How::Asked {
                            lines.push(asked(&named, target, &waited));
                        } else {
                            let by = over.by.as_ref().filter(|_| over.how != How::Ended).map(|by| format!(", by {}", name(by))).unwrap_or_default();
                            let now = match (over.how, target.and_then(State::of)) {
                                (How::Released, Some(state)) => format!(", now {}", state.word()),
                                _ => String::new(),
                            };
                            lines.push(format!("{named} {}{now}{by}. {waited}", agent::ended_phrase(over.how, true)));
                        }
                    }
                }
                None if holding && wait.rev > since && !mine(&wait.by) => {
                    let held = target.filter(|item| State::of(item) == Some(State::Progress)).and_then(|item| item.held_by.as_ref());
                    if held.is_some_and(|holder| me.is(holder)) && told.mark(&format!("waited-{uid}")) {
                        lines.push(format!(
                            "{} waits on {named}, which this session holds, until {} (note {}): {}",
                            name(&wait.by),
                            wait.until.word(),
                            note.id,
                            agent::clip(&note.description, QUOTED)
                        ));
                    }
                }
                _ => {}
            }
        }
        let answer = note.question.as_ref().filter(|question| question.asked_by.as_ref().is_some_and(mine)).and_then(|question| question.answer.as_ref());
        if let Some(answer) = answer {
            let theirs = !answer.by.as_ref().is_some_and(|by| me.is(by));
            if answer.rev > since && theirs && told.has(&format!("open-{uid}")) && told.mark(&format!("answer-{uid}")) {
                let by = answer.by.as_ref().map_or_else(|| "the user".to_string(), name);
                lines.push(format!(
                    "Question {}, which this session asked, was answered, recorded by {by}: {}. It asked: {}",
                    note.id,
                    agent::clip(&answer.text, QUOTED),
                    agent::clip(&note.description, QUOTED)
                ));
            }
        }
    }
    // The notes others write for the tasks it holds (task 1648): each note
    // the user or another session attaches to a task this session holds,
    // once it took the task, told once -- by the hook too, which reaches a
    // session between its tool calls (note 1741). So sessions working
    // related tasks talk through notes on each other's, and the user or the
    // orchestrator steers one at work. A question there is the user's, a
    // wait is told above, and a comment or a review from an artifact's page
    // is the feedback told first, once sent.
    for note in all.values() {
        if note.trashed.is_some() || note.stashed.is_some() || note.question.is_some() || note.wait.is_some() || note.comment.is_some() || note.review.is_some() {
            continue;
        }
        let (Some(uid), Some(rev)) = (note.uid.as_deref(), note.rev) else { continue };
        let Some(task) = note.attached_to.as_deref().and_then(|on| by_uid.get(on).copied()).filter(|task| holds(task)) else { continue };
        let Some(holder) = task.held_by.as_ref() else { continue };
        if rev <= since || note.timestamp < holder.since || note.created_by.as_ref().is_some_and(mine) || !told.mark(&format!("note-{uid}")) {
            continue;
        }
        let from = note.created_by.as_ref().map_or_else(|| "someone".to_string(), |by| writer(&all, by, &name(by)));
        lines.push(format!("Note {} on task {} ({}), which this session holds, from {from}: {}", note.id, task.id, titled(task), agent::clip(&note.description, NOTED)));
    }
    Ok(lines)
}

/// Who wrote a note, as the session it is told to reads it: the user, or a
/// session by `label` and, where it holds tasks in progress, by them -- the
/// tasks a reply to it is attached to.
fn writer(all: &crate::storage::ItemMap, by: &Holder, label: &str) -> String {
    let Some(process) = by.process() else { return label.to_string() };
    let mut held: Vec<u32> = all.values().filter(|task| task.held_by.as_ref().and_then(Holder::process).as_ref() == Some(&process)).map(|task| task.id).collect();
    held.sort_unstable();
    match held.as_slice() {
        [] => label.to_string(),
        [id] => format!("the session holding task {id} ({label})"),
        _ => format!("the session holding tasks {} ({label})", crate::agents::listed(&held)),
    }
}

/// The line telling a session that the one holding the task it waited on,
/// `named`, waits on the user (task 1715): in which window and for what,
/// while it still does, for the session to tell the user, who may be
/// looking at it rather than at that window; then that it wait again.
fn asked(named: &str, target: Option<&Item>, waited: &str) -> String {
    let held = target.filter(|item| State::of(item) == Some(State::Progress)).and_then(|item| item.held_by.as_ref());
    match held.and_then(Holder::waits) {
        Some(on) => {
            let window = on.window.as_deref().map(|window| format!(", in the window named \"{window}\"")).unwrap_or_default();
            format!(
                "{named} is held by a session that waits on the user{window}: {}. Tell the user, then wait on it again. {waited}",
                agent::clip(&on.what, QUOTED)
            )
        }
        None => format!("{named} was held by a session that waited on the user, and waits no longer. Wait on it again. {waited}"),
    }
}

/// The cursor the SessionStart hook last served `me`'s conversation on
/// `board`, or 0 where none was: a client without that hook.
pub fn since(home: &Path, me: &Actor, board: &str) -> u64 {
    me.conversation()
        .and_then(|conversation| agent::served_cursor(&agent::session_state_dir(home), &conversation, board))
        .and_then(|cursor| u64::try_from(cursor).ok())
        .unwrap_or(0)
}

/// `ekko --wake --hook`, run by the plugin's FileChanged hook on the
/// board's file: what this session has not been told, on stderr, exiting 2
/// -- which, the hook being asyncRewake, wakes the session with it. Exits 0
/// with nothing to tell, and on any error, which a hook run on every write
/// must not show the user.
pub fn hook(ekko: &Ekko, input: &str, home: &Path, board: &str) -> ExitCode {
    let event: serde_json::Value = serde_json::from_str(input).unwrap_or_default();
    let Some(me) = ekko.actor.as_ref().filter(|actor| !actor.is_person()) else { return ExitCode::SUCCESS };
    let Some(process) = me.process.as_ref() else { return ExitCode::SUCCESS };
    forget_ended(home);
    let told = Told::of(home, process);
    let lines = match event["file_path"].as_str().map(Path::new) {
        // Another board's file, watched by the same session: one linked to
        // this one, where it may wait, or one it reads nothing of.
        Some(changed) if changed != ekko.storage.storage_path() => {
            untold_linked(ekko, me, &told, home, false, |there| there.storage.storage_path() == changed)
        }
        _ => {
            told.heard(ekko);
            let since = event["session_id"]
                .as_str()
                .and_then(|session| agent::served_cursor(&agent::session_state_dir(home), session, board))
                .and_then(|cursor| u64::try_from(cursor).ok())
                .unwrap_or_else(|| self::since(home, me, board));
            untold(ekko, me, &told, since, false).unwrap_or_default()
        }
    };
    if lines.is_empty() {
        return ExitCode::SUCCESS;
    }
    eprintln!("{}", lines.join("\n"));
    ExitCode::from(2)
}

/// What `me` has not been told yet on the boards linked to `ekko`'s (task
/// 1443), as `untold` finds it on each one `keep` takes: above all its waits
/// there that a write there ended. Each line names its board, whose display
/// ids are not this one's. No prime of this session read a linked board, so
/// none gave it a cursor there: what ended there is told once in each
/// process, rather than missed for having ended while none ran.
pub fn untold_linked(ekko: &Ekko, me: &Actor, told: &Told, home: &Path, holding: bool, mut keep: impl FnMut(&Ekko) -> bool) -> Vec<String> {
    let mut lines = Vec::new();
    for project in ekko.linked() {
        let Ok(location) = crate::directory::locate(home, home, None, None, Some(&project.name)) else { continue };
        let Ok(there) = Ekko::at(&location) else { continue };
        let there = there.acting_as(me.clone());
        if !keep(&there) {
            continue;
        }
        let found = untold(&there, me, told, 0, holding).unwrap_or_default();
        lines.extend(found.into_iter().map(|line| format!("On project {}: {line}", project.name)));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::{Draft, Ref, WaitOn};
    use crate::storage::Storage;

    fn wait_on(ekko: &Ekko, item: u32, until: &str, text: &str) -> u32 {
        let mut draft = Draft::open(ekko).unwrap();
        let spec = WaitOn { item: Ref::Id(item), until: Some(until.to_string()), text: Some(text.to_string()), cancel: false };
        let crate::ops::Waited::Recorded(note) = draft.wait(&spec).unwrap() else { panic!("a wait recorded") };
        draft.commit(false).unwrap();
        note
    }

    fn set(ekko: &Ekko, item: u32, state: &str, force: bool) {
        let mut draft = Draft::open(ekko).unwrap();
        draft.set_state(&[Ref::Id(item)], state).unwrap();
        draft.commit(force).unwrap();
    }

    /// A session is told once of each wait of its that another's write
    /// ended, never of one its own write did, and only of what moved after
    /// the cursor it started from; the session holding a task is told once
    /// of each wait on it, in a reply and not by the hook (task 389).
    #[test]
    fn a_session_is_told_of_each_wait_once() {
        let (me, other, _) = crate::holder::test_sessions();
        let dir = crate::paths::test_dir("ekko-wake-waits");
        let home = dir.join("home");
        let as_ = |actor: &Actor| Ekko::new(Storage::new(&dir).unwrap()).acting_as(actor.clone());
        let (mine, theirs) = (Told::of(&home, me.process.as_ref().unwrap()), Told::of(&home, other.process.as_ref().unwrap()));
        as_(&other).create_task(&["Release v0.15.0".to_string()]).unwrap();
        as_(&other).create_task(&["the fix".to_string()]).unwrap();
        set(&as_(&other), 1, "progress", false);
        set(&as_(&other), 2, "progress", false);
        let release = wait_on(&as_(&me), 1, "done", "land 380: rebase, test, push");
        let fix = wait_on(&as_(&me), 2, "free", "take it over");

        assert!(untold(&as_(&other), &other, &theirs, 0, false).unwrap().is_empty(), "the hook does not wake a holder");
        let held = untold(&as_(&other), &other, &theirs, 0, true).unwrap();
        assert_eq!(held[0], format!("default on pts/1 waits on 1 (Release v0.15.0), which this session holds, until done (note {release}): land 380: rebase, test, push"));
        assert_eq!(held.len(), 2, "{held:?}");
        assert!(untold(&as_(&other), &other, &theirs, 0, true).unwrap().is_empty(), "once");
        assert!(untold(&as_(&me), &me, &mine, 0, false).unwrap().is_empty(), "nothing is over yet");

        set(&as_(&other), 1, "done", false);
        let revision = as_(&me).storage.get_counters().unwrap().revision;
        assert!(untold(&as_(&me), &me, &mine, revision, false).unwrap().is_empty(), "told already by what the session started from");
        let told = untold(&as_(&me), &me, &mine, 0, false).unwrap();
        assert_eq!(told, vec![format!("1 (Release v0.15.0) is done, by default on pts/2. This session waited on it (note {release}) to: land 380: rebase, test, push")]);
        assert!(untold(&as_(&me), &me, &mine, 0, false).unwrap().is_empty(), "once");

        set(&as_(&me), 2, "progress", true);
        assert!(untold(&as_(&me), &me, &mine, 0, false).unwrap().is_empty(), "its own write ended it: {fix}");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A session holding a task is told once, by the hook too, of each note
    /// the user or another session attaches to it once the session took it
    /// (task 1648), saying who wrote it and which tasks that one holds. Not
    /// of its own note, of one written before it took the task or before the
    /// cursor it started from, of a question, a wait, a trashed or a stashed
    /// note there, of a note on a task it does not hold, nor of any once it
    /// no longer holds the task.
    #[test]
    fn a_session_is_told_of_each_note_another_writes_on_a_task_it_holds() {
        let (me, other, _) = crate::holder::test_sessions();
        let dir = crate::paths::test_dir("ekko-wake-notes");
        let home = dir.join("home");
        let as_ = |actor: &Actor| Ekko::new(Storage::new(&dir).unwrap()).acting_as(actor.clone());
        let note = |actor: &Actor, task: u32, text: &str| crate::agents_merge::noted(&as_(actor), &[task], text).unwrap()[0];
        let afresh = |name: &str| Told::within(&home.join(name), me.process.as_ref().unwrap());
        as_(&other).create_task(&["Add the API".to_string()]).unwrap();
        as_(&other).create_task(&["Use the API".to_string()]).unwrap();
        note(&other, 1, "written before it was taken");
        std::thread::sleep(std::time::Duration::from_millis(3));
        set(&as_(&me), 1, "progress", false);
        set(&as_(&other), 2, "progress", false);

        // None of these is for it.
        note(&me, 1, "its own note");
        note(&other, 2, "on the task the other holds");
        let asker = as_(&other);
        let mut draft = Draft::open(&asker).unwrap();
        draft.ask("Ship it?", Some(&Ref::Id(1))).unwrap();
        draft.commit(false).unwrap();
        let waited = wait_on(&asker, 1, "done", "use the API");
        let person = as_(&Actor::person());
        let trashed = note(&other, 1, "trashed");
        let stashed = note(&other, 1, "stashed");
        person.set_trashed(&[trashed.to_string()], true).unwrap();
        person.set_stashed(&[stashed.to_string()], true).unwrap();
        let board = as_(&me);
        assert!(untold(&board, &me, &afresh("none"), 0, false).unwrap().is_empty());
        let held = untold(&board, &me, &afresh("held"), 0, true).unwrap();
        assert!(held.len() == 1 && held[0].starts_with(&format!("default on pts/2 waits on 1 (Add the API), which this session holds, until done (note {waited})")), "{held:?}");

        let theirs = note(&other, 1, "The route is /v2/items now.");
        let users = note(&Actor::person(), 1, "Keep the old route too.");
        let lines = vec![
            format!("Note {theirs} on task 1 (Add the API), which this session holds, from the session holding task 2 (default on pts/2): The route is /v2/items now."),
            format!("Note {users} on task 1 (Add the API), which this session holds, from the user: Keep the old route too."),
        ];
        assert_eq!(untold(&board, &me, &afresh("peek"), 0, false).unwrap(), lines);
        let input = serde_json::json!({"hook_event_name": "FileChanged", "file_path": board.storage.storage_path()}).to_string();
        let mine = Told::of(&home, me.process.as_ref().unwrap());
        assert_eq!(hook(&board, &input, &home, "default board"), ExitCode::from(2));
        assert_eq!(hook(&board, &input, &home, "default board"), ExitCode::SUCCESS, "once");
        assert!(untold(&board, &me, &mine, 0, false).unwrap().is_empty(), "once, whichever way");

        let later = note(&other, 1, "One more.");
        let revision = board.storage.get_counters().unwrap().revision;
        assert!(untold(&board, &me, &afresh("cursor"), revision, false).unwrap().is_empty(), "before the cursor: {later}");
        set(&board, 1, "done", false);
        note(&other, 1, "After it was done.");
        assert!(untold(&board, &me, &afresh("done"), 0, false).unwrap().is_empty(), "a task it no longer holds");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Marks the task `task`, which the session `ekko` writes as holds, as
    /// waiting on the user for `what` in `window` from now, or clears the
    /// mark, as a born session's hook does (`agents::mark_holds`).
    fn mark(ekko: &Ekko, task: u32, on: Option<(&str, &str)>) {
        let _lock = ekko.storage.acquire_lock().unwrap();
        let before = ekko.storage.get_shared().unwrap();
        let mut data = crate::storage::ItemMap::clone(&before);
        let holder = data.get_mut(&task).unwrap().held_by.as_mut().unwrap();
        holder.waits_on_user = on.map(|(what, window)| crate::holder::OnUser {
            since: chrono::Local::now().timestamp_millis(),
            what: what.to_string(),
            window: Some(window.to_string()),
            unknown: Default::default(),
        });
        ekko.save_against(&before, &mut data).unwrap();
    }

    /// A wait on a task ends when the session holding it starts to wait on
    /// the user, and on no other write (task 1715): the hook wakes the
    /// session that waited, once, saying in which window and for what. A
    /// wait made again ends not on that mark but on the next one; and told
    /// once the mark is cleared, it says the holder waits no longer.
    #[test]
    fn a_wait_ends_when_its_task_s_holder_waits_on_the_user_and_says_where() {
        let (me, other, _) = crate::holder::test_sessions();
        let dir = crate::paths::test_dir("ekko-wake-asked");
        let home = dir.join("home");
        let as_ = |actor: &Actor| Ekko::new(Storage::new(&dir).unwrap()).acting_as(actor.clone());
        let mine = Told::of(&home, me.process.as_ref().unwrap());
        let (holder, waiter) = (as_(&other), as_(&me));
        holder.create_task(&["Write the docs".to_string()]).unwrap();
        set(&holder, 1, "progress", false);
        let first = wait_on(&waiter, 1, "done", "review the docs");
        let open = |note: u32| waiter.storage.get().unwrap()[&note].wait.as_ref().unwrap().over.is_none();
        let pause = || std::thread::sleep(std::time::Duration::from_millis(3));

        // Every other write leaves it open: the task's text, its star, a
        // task beside it, and a mark cleared that was never set.
        let mut draft = Draft::open(&holder).unwrap();
        draft.edit(&crate::ops::Edit { item: Ref::Id(1), text: None, replace: None, append: Some(" -- started".into()), if_updated_at: None }).unwrap();
        draft.set_state(&[Ref::Id(1)], "starred").unwrap();
        draft.commit(false).unwrap();
        holder.create_task(&["Another task".to_string()]).unwrap();
        mark(&holder, 1, None);
        assert!(open(first), "another write ended it");
        assert!(untold(&waiter, &me, &mine, 0, false).unwrap().is_empty());

        pause();
        mark(&holder, 1, Some(("Claude needs your permission -- Bash: date > stamp.txt", "1 \u{b7} haiku")));
        assert!(!open(first), "the mark did not end it");
        let input = serde_json::json!({"hook_event_name": "FileChanged", "file_path": waiter.storage.storage_path()}).to_string();
        let told = untold(&waiter, &me, &Told::within(&home.join("peek"), me.process.as_ref().unwrap()), 0, false).unwrap();
        assert_eq!(
            told,
            vec![format!(
                "1 (Write the docs (+10 chars)) is held by a session that waits on the user, in the window named \"1 \u{b7} haiku\": \
                 Claude needs your permission -- Bash: date > stamp.txt. Tell the user, then wait on it again. \
                 This session waited on it (note {first}) to: review the docs"
            )]
        );
        assert_eq!(hook(&waiter, &input, &home, "default board"), ExitCode::from(2));
        assert_eq!(hook(&waiter, &input, &home, "default board"), ExitCode::SUCCESS, "once");

        // Made again, the wait does not end on the same mark, nor on the
        // mark cleared; the next one ends it.
        pause();
        let again = wait_on(&waiter, 1, "done", "review the docs");
        mark(&holder, 1, None);
        assert!(open(again), "the mark it was told of ended the wait made again");
        pause();
        mark(&holder, 1, Some(("Claude needs your permission -- Bash: rm -r target", "1 \u{b7} haiku")));
        assert!(!open(again));
        // Told once the mark is cleared, it says so.
        mark(&holder, 1, None);
        let told = untold(&waiter, &me, &mine, 0, false).unwrap();
        assert_eq!(
            told,
            vec![format!(
                "1 (Write the docs (+10 chars)) was held by a session that waited on the user, and waits no longer. Wait on it again. \
                 This session waited on it (note {again}) to: review the docs"
            )]
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// An answer reaches the session that asked only for a question its ask
    /// left open: one given while the ask waited came back in its reply.
    /// And the hook wakes the session with it, exiting 2, once.
    #[test]
    fn an_answer_is_told_only_for_a_question_left_open() {
        let (me, _, _) = crate::holder::test_sessions();
        let dir = crate::paths::test_dir("ekko-wake-answers");
        let home = dir.join("home");
        let as_ = |actor: &Actor| Ekko::new(Storage::new(&dir).unwrap()).acting_as(actor.clone());
        let mine = Told::of(&home, me.process.as_ref().unwrap());
        let writer = as_(&me);
        let mut draft = Draft::open(&writer).unwrap();
        let (open, answered) = (draft.ask("Merge now?", None).unwrap(), draft.ask("Release too?", None).unwrap());
        draft.commit(false).unwrap();
        let uid = |id: u32| as_(&me).storage.get().unwrap()[&id].uid.clone().unwrap();
        mine.left_open(&uid(open));
        let writer = as_(&Actor::person());
        let mut draft = Draft::open(&writer).unwrap();
        draft.answer(&Ref::Id(open), "yes, after the rebase").unwrap();
        draft.answer(&Ref::Id(answered), "no").unwrap();
        draft.commit(false).unwrap();

        let board = as_(&me);
        let input = serde_json::json!({"hook_event_name": "FileChanged", "file_path": board.storage.storage_path()}).to_string();
        assert_eq!(hook(&board, &input, &home, "default board"), ExitCode::from(2));
        assert!(mine.has(&format!("answer-{}", uid(open))) && !mine.has(&format!("answer-{}", uid(answered))));
        assert_eq!(hook(&board, &input, &home, "default board"), ExitCode::SUCCESS, "once");
        let elsewhere = serde_json::json!({"file_path": "/elsewhere/storage.json"}).to_string();
        assert_eq!(hook(&board, &elsewhere, &home, "default board"), ExitCode::SUCCESS, "another board's file");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Each run on this board's file records the version of the file it saw
    /// and the revision it read, whether or not it has anything to tell;
    /// another board's file leaves the record as it was (task 1281).
    #[test]
    fn the_hook_records_the_version_of_the_board_it_heard() {
        use std::os::unix::fs::MetadataExt as _;
        let (me, _, _) = crate::holder::test_sessions();
        let dir = crate::paths::test_dir("ekko-wake-heard");
        let home = dir.join("home");
        let board = Ekko::new(Storage::new(&dir).unwrap()).acting_as(me.clone());
        let heard = Told::of(&home, me.process.as_ref().unwrap()).dir.join("heard");
        let read = || serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(&heard).unwrap()).unwrap();
        let on = |path: &Path| serde_json::json!({"hook_event_name": "FileChanged", "file_path": path}).to_string();
        let elsewhere = on(Path::new("/elsewhere/storage.json"));
        let version = || {
            let file = std::fs::metadata(board.storage.storage_path()).unwrap();
            let revision = board.storage.get_counters().unwrap().revision;
            (Some(file.ino()), Some(file.mtime() * 1_000_000_000 + file.mtime_nsec()), Some(file.len()), Some(revision))
        };
        let recorded = |heard: &serde_json::Value| {
            (heard["inode"].as_u64(), heard["mtime_ns"].as_i64(), heard["size"].as_u64(), heard["revision"].as_u64())
        };

        board.create_task(&["first".to_string()]).unwrap();
        hook(&board, &elsewhere, &home, "default board");
        assert!(!heard.exists(), "another board's file was heard");
        assert_eq!(hook(&board, &on(board.storage.storage_path()), &home, "default board"), ExitCode::SUCCESS, "nothing to tell");
        let first = read();
        assert_eq!(recorded(&first), version());

        board.create_task(&["second".to_string()]).unwrap();
        hook(&board, &elsewhere, &home, "default board");
        assert_eq!(read(), first, "another board's file changed the record");
        hook(&board, &on(board.storage.storage_path()), &home, "default board");
        let second = read();
        assert_eq!(recorded(&second), version());
        assert_ne!(recorded(&second), recorded(&first));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A wait on a task of a linked board, kept on that board, is told by the
    /// hook run on that board's file once the task is done, once, naming the
    /// board (task 1443). A wait on a board not linked is not told, and the
    /// linked board's file leaves the record of what the session heard of
    /// its own as it was.
    #[test]
    fn a_wait_on_a_linked_boards_task_is_told_by_the_hook_on_that_boards_file() {
        let (me, other, _) = crate::holder::test_sessions();
        let dir = crate::paths::test_dir("ekko-wake-linked");
        std::fs::create_dir_all(&dir).unwrap();
        let home = std::fs::canonicalize(&dir).unwrap();
        let boards = crate::project::test_projects(&home, &["site", "blog", "shop"], &[(0, 1)]);
        let as_ = |board: usize, actor: &Actor| Ekko::at(&boards[board]).unwrap().acting_as(actor.clone());
        let (site, blog, shop) = (as_(0, &me), as_(1, &other), as_(2, &other));
        let process = me.process.as_ref().unwrap();
        // A board with a file, whose version the hook could record.
        site.create_task(&["the site's work".to_string()]).unwrap();
        for board in [&blog, &shop] {
            board.create_task(&["the release".to_string()]).unwrap();
            set(board, 1, "progress", false);
        }
        let note = wait_on(&as_(1, &me), 1, "done", "announce it on the site");
        wait_on(&as_(2, &me), 1, "done", "nothing: the shop is not linked");
        let on = |board: &Ekko| serde_json::json!({"hook_event_name": "FileChanged", "file_path": board.storage.storage_path()}).to_string();

        assert_eq!(hook(&site, &on(&blog), &home, "project site"), ExitCode::SUCCESS, "nothing is over yet");
        set(&blog, 1, "done", false);
        set(&shop, 1, "done", false);
        assert_eq!(hook(&site, &on(&shop), &home, "project site"), ExitCode::SUCCESS, "a board not linked");
        assert_eq!(hook(&site, &on(&blog), &home, "project site"), ExitCode::from(2));
        assert_eq!(hook(&site, &on(&blog), &home, "project site"), ExitCode::SUCCESS, "once");
        assert!(!Told::of(&home, process).dir.join("heard").exists(), "a linked board's file heard as the session's own");

        let afresh = Told::within(&home.join("afresh"), process);
        let told = untold_linked(&site, &me, &afresh, &home, false, |_| true);
        assert_eq!(told, vec![format!("On project blog: 1 (the release) is done, by default on pts/2. This session waited on it (note {note}) to: announce it on the site")]);
        std::fs::remove_dir_all(&dir).ok();
    }
}
