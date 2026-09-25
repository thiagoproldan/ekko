//! Telling a Claude Code session what it waited on (task 389): its waits
//! that another's write ended, and the answers to questions its ask left
//! open -- each once, whichever way reaches it first. The plugin's
//! FileChanged hook runs `ekko --wake --hook` in the background whenever the
//! board's file changes, and its exit 2 wakes a session sitting idle; the
//! session's next ekko reply carries the same lines, for a client without
//! that hook. The session holding a task is told, once and only in a reply,
//! that another waits on it.
//!
//! What was told is kept outside the board, since reading the board writes
//! nothing to it: a marker file per line, in a folder per Claude Code
//! process, made with `create_new` so that two hook runs racing on one
//! write cannot both wake the session.

use std::collections::HashMap;
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

/// What one Claude Code process was told, and which of the questions it
/// asked it was left waiting on: a marker file each.
pub struct Told {
    dir: PathBuf,
}

impl Told {
    /// The markers of `process`, under the ekko state directory in `home`.
    pub fn of(home: &Path, process: &Process) -> Told {
        let boot: String = process.boot.chars().filter(char::is_ascii_alphanumeric).take(8).collect();
        Told { dir: told_dir(home).join(format!("{boot}-{}-{}", process.pid, process.start)) }
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
}

/// Where the markers of every process are kept.
fn told_dir(home: &Path) -> PathBuf {
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
/// marked as told as it is returned: its waits that another's write ended,
/// and the answers to the questions its ask left open -- and, with
/// `holding`, the waits other sessions keep on the tasks it holds. Only what
/// moved after `since`, the cursor its session last started from: its prime,
/// or the changes its resume was given, already said what came before.
pub fn untold(ekko: &Ekko, me: &Actor, told: &Told, since: u64, holding: bool) -> Result<Vec<String>, EkkoError> {
    let all = ekko.storage.get_shared()?;
    let registry = me.registry.as_ref();
    // Its own, or its conversation's from before a restart, as a question's
    // answer is (`agent::prime`).
    let mine = |holder: &Holder| {
        me.is(holder)
            || (!holder.alive()
                && me.conversation().is_some_and(|now| holder.conversation_in(registry).as_deref() == Some(now.as_str())))
    };
    let by_uid: HashMap<&str, &Item> = all.values().filter_map(|item| Some((item.uid.as_deref()?, item))).collect();
    let name = |holder: &Holder| holder.label_in(registry);
    let titled = |item: &Item| agent::headline(&item.description, item.is_task, TITLED);

    let mut lines = Vec::new();
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
                        let by = over.by.as_ref().filter(|_| over.how != How::Ended).map(|by| format!(", by {}", name(by))).unwrap_or_default();
                        let now = match (over.how, target.and_then(State::of)) {
                            (How::Released, Some(state)) => format!(", now {}", state.word()),
                            _ => String::new(),
                        };
                        lines.push(format!(
                            "{named} {}{now}{by}. This session waited on it (note {}) to: {}",
                            agent::ended_phrase(over.how, true),
                            note.id,
                            agent::clip(&note.description, QUOTED)
                        ));
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
                let by = answer.by.as_ref().map_or_else(|| "the user".to_string(), &name);
                lines.push(format!(
                    "Question {}, which this session asked, was answered, recorded by {by}: {}. It asked: {}",
                    note.id,
                    agent::clip(&answer.text, QUOTED),
                    agent::clip(&note.description, QUOTED)
                ));
            }
        }
    }
    Ok(lines)
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
    // Another board's file, watched by the same session, is not this one.
    if let Some(changed) = event["file_path"].as_str() {
        if Path::new(changed) != ekko.storage.storage_path() {
            return ExitCode::SUCCESS;
        }
    }
    let Some(me) = ekko.actor.as_ref().filter(|actor| !actor.is_person()) else { return ExitCode::SUCCESS };
    let Some(process) = me.process.as_ref() else { return ExitCode::SUCCESS };
    forget_ended(home);
    let told = Told::of(home, process);
    let since = event["session_id"]
        .as_str()
        .and_then(|session| agent::served_cursor(&agent::session_state_dir(home), session, board))
        .and_then(|cursor| u64::try_from(cursor).ok())
        .unwrap_or_else(|| self::since(home, me, board));
    match untold(ekko, me, &told, since, false) {
        Ok(lines) if !lines.is_empty() => {
            eprintln!("{}", lines.join("\n"));
            ExitCode::from(2)
        }
        _ => ExitCode::SUCCESS,
    }
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
}
