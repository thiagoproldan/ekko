//! The board drawn in Claude Code's own task list: the ✔ ◼ ◻ list under the
//! spinner that TaskCreate and TaskUpdate fill, filled here from the board
//! instead, so a person follows the agent's work where Claude Code already
//! shows work -- with the task in progress as the spinner's line.
//!
//! Claude Code keeps that list as one JSON file per task in
//! `<config dir>/tasks/<list id>/`, where the list id is the session's own id
//! unless `CLAUDE_CODE_TASK_LIST_ID` names another, and it redraws the list
//! when those files change, whoever writes them. None of this is in Claude
//! Code's documentation: the format and the rules come from its 2.1.278
//! binary and from testing it (gotcha 200 on the board), so a Claude Code that
//! changes them breaks the drawing, never the board. On a model it offers no
//! task tools to -- Opus 5 among them -- it draws no list at all unless
//! `CLAUDE_CODE_ENABLE_TODO_TOOLS=1`.
//!
//! The list is ekko's: each write makes it exactly the board's, so a task
//! written there by anything else, the native TaskCreate included, is gone
//! at the next write. The user ruled the native tools out; the board is the
//! one list.

use std::collections::HashSet;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::agent;
use crate::ekko::Ekko;
use crate::item::State;
use crate::storage::lock_path;

/// Ready tasks the list shows after the work in progress, best first.
const NEXT_SHOWN: usize = 5;
/// Tasks the session finished that stay on the list, checked off: the newest.
const DONE_SHOWN: usize = 10;
/// How much of a task's first line a list entry carries, and how much the
/// spinner line does.
const SUBJECT_CLIP: usize = 80;
const ACTIVE_CLIP: usize = 60;
/// What ekko keeps beside the list: the board revision the session began at.
/// Claude Code reads no file in the list whose name starts with a dot.
const SESSION_FILE: &str = ".ekko.json";

/// One task, in the shape Claude Code reads from `<id>.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub subject: String,
    pub description: String,
    /// The line the spinner shows while the task is in progress.
    pub active_form: String,
    /// `completed`, `in_progress` or `pending`.
    pub status: String,
    #[serde(default)]
    pub blocks: Vec<String>,
    #[serde(default)]
    pub blocked_by: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Session {
    /// The board revision when the session began. A task done at a later
    /// revision, as this session's work (`Item::done_by`), is one it finished.
    since: u64,
}

/// The list for a session that began at board revision `since`, in the order
/// Claude Code draws it: what the session finished, then the work in
/// progress, then the next ready tasks. Numbered from 1 in that order,
/// because Claude Code draws its list by id; each subject starts with the
/// task's id on the board.
pub fn tasks(ekko: &Ekko, since: u64) -> Result<Vec<Task>, Box<dyn Error>> {
    let all = ekko.storage.get_shared()?;
    // Done since the session began, and its own work: what another session
    // finished meanwhile is that session's to check off.
    let me = ekko.actor.as_ref();
    let mut done: Vec<_> = all
        .values()
        .filter(|item| item.stashed.is_none() && item.trashed.is_none())
        .filter(|item| State::of(item) == Some(State::Done) && item.rev.unwrap_or(0) > since)
        .filter(|item| item.done_by.as_ref().is_some_and(|holder| me.is_some_and(|me| me.is(holder))))
        .collect();
    done.sort_by_key(|item| (item.updated_at.unwrap_or(item.timestamp), item.id));
    let done = done.split_off(done.len().saturating_sub(DONE_SHOWN));

    // Work another running session holds is that session's, not this one's;
    // and only this session's own is in progress on its list, as its
    // spinner's line. Work in progress that no running session holds -- its
    // session ended, or none ever claimed it -- comes last, marked
    // abandoned: free to take up, and nobody's spinner. Seen on 2026-09-22,
    // when one session's spinner showed a task another had held before a
    // restart.
    let (progress, ready): (Vec<_>, Vec<_>) = agent::next(ekko, None)?
        .into_iter()
        .filter(|entry| !entry.held.as_ref().is_some_and(agent::Held::elsewhere))
        .partition(|entry| entry.state == Some(State::Progress));
    let (doing, abandoned): (Vec<_>, Vec<_>) =
        progress.into_iter().partition(|entry| entry.held.as_ref().is_some_and(|held| held.yours));

    // A step of a sequence says which: "(2/3) ".
    let step = |entry: &agent::Entry| entry.step.map(|(step, of)| format!("({step}/{of}) ")).unwrap_or_default();
    let rows = done
        .iter()
        .map(|item| (item.id, String::new(), item.description.as_str(), "completed"))
        .chain(doing.iter().map(|entry| (entry.id, step(entry), entry.description.as_str(), "in_progress")))
        .chain(ready.iter().take(NEXT_SHOWN).map(|entry| (entry.id, step(entry), entry.description.as_str(), "pending")))
        .chain(abandoned.iter().map(|entry| (entry.id, "(abandoned) ".to_string(), entry.description.as_str(), "pending")));
    Ok(rows.enumerate().map(|(at, (id, mark, text, status))| task(at + 1, id, &mark, text, status)).collect())
}

fn task(number: usize, id: u32, mark: &str, text: &str, status: &str) -> Task {
    let first = text.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or_default();
    Task {
        id: number.to_string(),
        subject: format!("{id}. {mark}{}", shorten(first, SUBJECT_CLIP)),
        description: text.to_string(),
        active_form: format!("{id}. {mark}{}", shorten(first, ACTIVE_CLIP)),
        status: status.to_string(),
        blocks: Vec::new(),
        blocked_by: Vec::new(),
    }
}

/// `text` cut to `max` characters with an ellipsis: a list line has no room
/// for the count of what was cut that `agent::clip` adds.
fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let head: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{}\u{2026}", head.trim_end())
}

/// Makes the list in `dir` exactly `tasks`: writes the files that differ,
/// each through a dotfile and a rename so Claude Code never reads half of
/// one, and removes the task files the list no longer holds. Under the
/// list's `.lock`, the file Claude Code keeps beside it. Says whether
/// anything changed.
pub fn write(dir: &Path, tasks: &[Task]) -> Result<bool, Box<dyn Error>> {
    fs::create_dir_all(dir)?;
    let _lock = lock_path(&dir.join(".lock"))?;
    let mut changed = false;
    let mut kept = HashSet::new();
    for task in tasks {
        let file = dir.join(format!("{}.json", task.id));
        let body = serde_json::to_string_pretty(task)?;
        if fs::read_to_string(&file).ok().as_deref() != Some(body.as_str()) {
            let temp = dir.join(format!(".{}.json.tmp", task.id));
            fs::write(&temp, &body)?;
            fs::rename(&temp, &file)?;
            changed = true;
        }
        kept.insert(file);
    }
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
        if !name.starts_with('.') && name.ends_with(".json") && !kept.contains(&path) {
            fs::remove_file(&path)?;
            changed = true;
        }
    }
    // Claude Code numbers a task it creates after this mark. Kept past
    // ekko's numbers, a task created anyway cannot land on one of ekko's.
    let mark = dir.join(".highwatermark");
    let written = fs::read_to_string(&mark).ok().and_then(|text| text.trim().parse::<usize>().ok()).unwrap_or(0);
    if tasks.len() > written {
        fs::write(&mark, tasks.len().to_string())?;
    }
    Ok(changed)
}

/// `--tasklist --hook`: draws the board into the task list of the session a
/// hook event names. On SessionStart it also answers with the board's file,
/// for Claude Code to watch: a change made anywhere else -- the terminal or
/// another session -- then redraws the list through the FileChanged hook. A
/// failing hook prints an error into the session, too much for a drawing, so
/// this one never fails: what went wrong goes to stderr, which Claude Code
/// keeps for its debug log.
pub fn hook(ekko: &Ekko, input: &str, home: &Path) -> String {
    let config = std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".claude"));
    let list = std::env::var("CLAUDE_CODE_TASK_LIST_ID").ok().filter(|id| !id.is_empty());
    match hook_in(ekko, input, &config, list.as_deref()) {
        Ok(out) => out,
        Err(error) => {
            eprintln!("ekko --tasklist: {error}");
            String::new()
        }
    }
}

/// `hook` with Claude Code's config directory and list id given, rather than
/// read from the environment.
fn hook_in(ekko: &Ekko, input: &str, config: &Path, list: Option<&str>) -> Result<String, Box<dyn Error>> {
    let event: serde_json::Value = serde_json::from_str(input).unwrap_or_default();
    let Some(session) = event["session_id"].as_str().filter(|id| !id.is_empty()) else {
        return Ok(String::new());
    };
    let dir = config.join("tasks").join(folder(list.unwrap_or(session)));
    let since = began(&dir, ekko)?;
    write(&dir, &tasks(ekko, since)?)?;
    // Inside hookSpecificOutput, as every event's own fields are: at the top
    // level Claude Code reads the reply as valid and watches nothing.
    Ok(if event["hook_event_name"] == "SessionStart" {
        serde_json::json!({"hookSpecificOutput": {
            "hookEventName": "SessionStart",
            "watchPaths": [ekko.storage.storage_path()],
        }})
        .to_string()
    } else {
        String::new()
    })
}

/// The board revision the session began at, recorded beside its list the
/// first time the hook sees the session and read back from there after.
fn began(dir: &Path, ekko: &Ekko) -> Result<u64, Box<dyn Error>> {
    let file = dir.join(SESSION_FILE);
    if let Some(session) = fs::read_to_string(&file).ok().and_then(|text| serde_json::from_str::<Session>(&text).ok()) {
        return Ok(session.since);
    }
    let since = ekko.storage.get_counters()?.revision;
    fs::create_dir_all(dir)?;
    fs::write(&file, serde_json::to_string(&Session { since })?)?;
    Ok(since)
}

/// A list id as the folder name Claude Code gives it.
fn folder(id: &str) -> String {
    id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '-' }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Storage;

    fn scratch(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("ekko-tasklist-{tag}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn words(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn board(tag: &str) -> (Ekko, PathBuf) {
        let dir = scratch(tag);
        (Ekko::new(Storage::new(&dir.join("board")).unwrap()), dir)
    }

    fn drawn(tasks: &[Task]) -> Vec<(String, String)> {
        tasks.iter().map(|task| (task.status.clone(), task.subject.clone())).collect()
    }

    /// Work in progress first, then the five best ready tasks in `next`'s
    /// order -- 8 first, because work waits on it; blocked work is not ready,
    /// so it is not on the list.
    #[test]
    fn the_list_is_the_work_in_progress_then_the_next_five_ready() {
        let (me, _, _) = crate::holder::test_sessions();
        let (ekko, dir) = board("order");
        let ekko = ekko.acting_as(me);
        for k in 1..=8 {
            ekko.create_task(&words(&[format!("task {k}").as_str()])).unwrap();
        }
        ekko.set_blocked_by(&words(&["@2", "8"])).unwrap();
        ekko.set_state(&words(&["@5", "progress"]), false).unwrap();

        let list = tasks(&ekko, ekko.storage.get_counters().unwrap().revision).unwrap();
        let expected = [
            ("in_progress", "5. task 5"),
            ("pending", "8. (1/2) task 8"),
            ("pending", "1. task 1"),
            ("pending", "3. task 3"),
            ("pending", "4. task 4"),
            ("pending", "6. task 6"),
        ];
        assert_eq!(drawn(&list), expected.map(|(s, t)| (s.to_string(), t.to_string())));
        assert_eq!(list.iter().map(|task| task.id.as_str()).collect::<Vec<_>>(), ["1", "2", "3", "4", "5", "6"]);
        assert_eq!(list[0].active_form, "5. task 5");

        fs::remove_dir_all(&dir).ok();
    }

    /// Another running session's task in progress is not this session's
    /// work: its list leaves it out, in progress or not. Work in progress
    /// whose session ended is nobody's: last, marked abandoned, and never
    /// this session's spinner -- seen on 2026-09-22, when a task held before
    /// a restart was the spinner line of a session that never worked it.
    #[test]
    fn another_sessions_task_is_not_on_the_list() {
        let (me, other, gone) = crate::holder::test_sessions();
        let dir = scratch("held");
        let as_ = |actor: &crate::holder::Actor| Ekko::new(Storage::new(&dir.join("board")).unwrap()).acting_as(actor.clone());
        for name in ["theirs", "left behind", "mine"] {
            as_(&me).create_task(&words(&[name])).unwrap();
        }
        as_(&other).set_state(&words(&["@1", "progress"]), false).unwrap();
        as_(&gone).set_state(&words(&["@2", "progress"]), false).unwrap();
        as_(&me).set_state(&words(&["@3", "progress"]), false).unwrap();

        let list = tasks(&as_(&me), 0).unwrap();
        let expected = [("in_progress", "3. mine"), ("pending", "2. (abandoned) left behind")];
        assert_eq!(drawn(&list), expected.map(|(s, t)| (s.to_string(), t.to_string())));

        fs::remove_dir_all(&dir).ok();
    }

    /// What the session finished stays on the list, checked off: its own
    /// work, done since it began. Not what was done before, nor what another
    /// session finished meanwhile -- seen on 2026-09-22, when a second
    /// session's list checked off six tasks the first had done. A task the
    /// session held stays its own when the user checks it off, and a task
    /// reopened is nobody's.
    #[test]
    fn what_the_session_finished_is_checked_off() {
        let (me, other, _) = crate::holder::test_sessions();
        let dir = scratch("done");
        let as_ = |actor: &crate::holder::Actor| Ekko::new(Storage::new(&dir.join("board")).unwrap()).acting_as(actor.clone());
        let mine = as_(&me);
        for name in ["done before", "done during", "done by the other", "held, done by the user", "done, then reopened"] {
            mine.create_task(&words(&[name])).unwrap();
        }
        mine.set_state(&words(&["@1", "done"]), false).unwrap();
        let since = mine.storage.get_counters().unwrap().revision;
        mine.set_state(&words(&["@2", "done"]), false).unwrap();
        as_(&other).set_state(&words(&["@3", "done"]), false).unwrap();
        mine.set_state(&words(&["@4", "progress"]), false).unwrap();
        as_(&crate::holder::Actor::person()).check_tasks(&words(&["4"]), false).unwrap();
        mine.set_state(&words(&["@5", "done"]), false).unwrap();
        mine.set_state(&words(&["@5", "undone"]), false).unwrap();

        let completed = |ekko: &Ekko| -> Vec<String> {
            let list = tasks(ekko, since).unwrap();
            drawn(&list).into_iter().filter(|(status, _)| status == "completed").map(|(_, subject)| subject).collect()
        };
        assert_eq!(completed(&mine), ["2. done during", "4. held, done by the user"]);
        assert_eq!(completed(&as_(&other)), ["3. done by the other"]);
        assert_eq!(mine.storage.get().unwrap()[&5].done_by, None, "reopened");

        fs::remove_dir_all(&dir).ok();
    }

    /// A long first line is cut for the list and cut shorter for the spinner;
    /// the whole text stays in the description.
    #[test]
    fn a_long_task_is_cut_to_a_line() {
        let long = "a task whose first line runs on well past what one line of a list can hold, and then some more";
        let entry = task(1, 42, "", &format!("{long}\nsecond line"), "pending");
        assert_eq!(entry.subject.chars().count(), "42. ".len() + SUBJECT_CLIP);
        assert!(entry.subject.ends_with('\u{2026}') && entry.active_form.ends_with('\u{2026}'));
        assert!(entry.active_form.chars().count() < entry.subject.chars().count());
        assert!(entry.description.ends_with("second line"));
    }

    /// A write makes the list exactly the board's: it rewrites only what
    /// changed, removes what the list no longer holds -- a task file written
    /// by anything else included -- and leaves Claude Code's dotfiles alone.
    #[test]
    fn writing_makes_the_list_exactly_the_boards() {
        let dir = scratch("write");
        let list = dir.join("tasks").join("session");
        let first = [task(1, 7, "", "seven", "in_progress"), task(2, 9, "", "nine", "pending")];
        assert!(write(&list, &first).unwrap());
        assert!(!write(&list, &first).unwrap(), "an unchanged list writes nothing");

        fs::write(list.join("3.json"), "{}").unwrap();
        assert!(write(&list, &first[..1]).unwrap());
        let mut names: Vec<String> =
            fs::read_dir(&list).unwrap().map(|entry| entry.unwrap().file_name().into_string().unwrap()).collect();
        names.sort();
        assert_eq!(names, [".highwatermark", ".lock", "1.json"]);
        assert_eq!(fs::read_to_string(list.join(".highwatermark")).unwrap(), "2");
        let read: Task = serde_json::from_str(&fs::read_to_string(list.join("1.json")).unwrap()).unwrap();
        assert_eq!(read, first[0]);

        fs::remove_dir_all(&dir).ok();
    }

    /// The hook draws the list of the session its event names, in Claude
    /// Code's config directory, and on SessionStart names the board's file to
    /// watch; an event without a session draws nothing.
    #[test]
    fn the_hook_draws_the_sessions_list_and_names_the_board_to_watch() {
        let (me, _, _) = crate::holder::test_sessions();
        let (ekko, dir) = board("hook");
        let ekko = ekko.acting_as(me);
        ekko.create_task(&words(&["the work"])).unwrap();
        let config = dir.join("claude");

        let start = hook_in(&ekko, r#"{"session_id":"ab/12","hook_event_name":"SessionStart"}"#, &config, None).unwrap();
        let watched: serde_json::Value = serde_json::from_str(&start).unwrap();
        assert_eq!(watched["hookSpecificOutput"]["hookEventName"], "SessionStart");
        assert_eq!(watched["hookSpecificOutput"]["watchPaths"][0], ekko.storage.storage_path().to_str().unwrap());
        let list = config.join("tasks").join("ab-12");
        assert!(list.join("1.json").exists() && list.join(SESSION_FILE).exists());

        ekko.set_state(&words(&["@1", "done"]), false).unwrap();
        let after = hook_in(&ekko, r#"{"session_id":"ab/12","hook_event_name":"PostToolUse"}"#, &config, None).unwrap();
        assert_eq!(after, "");
        let read: Task = serde_json::from_str(&fs::read_to_string(list.join("1.json")).unwrap()).unwrap();
        assert_eq!((read.status.as_str(), read.subject.as_str()), ("completed", "1. the work"));

        ekko.create_task(&words(&["more work"])).unwrap();
        let named = hook_in(&ekko, r#"{"session_id":"x","hook_event_name":"PostToolUse"}"#, &config, Some("shared")).unwrap();
        assert_eq!(named, "");
        assert!(config.join("tasks").join("shared").join("1.json").exists(), "CLAUDE_CODE_TASK_LIST_ID wins");
        assert_eq!(hook_in(&ekko, "{}", &config, None).unwrap(), "");

        fs::remove_dir_all(&dir).ok();
    }
}
