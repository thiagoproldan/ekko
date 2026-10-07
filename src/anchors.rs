//! What a decision, gotcha or procedure rests on (task 1324, plan 1309).
//!
//! A typed note says it in a line of its text, so create's definition, which
//! every session pays for, takes no new field:
//!
//! ```text
//! Rests on: `src/storage.rs` "fn keep_unkept_version"; `evals/`; Claude Code 2.1.289; recheck after 2026-12-01
//! ```
//!
//! Each anchor, between semicolons, is one of:
//! - a path in backticks, which must be there;
//! - a path in backticks and words in double quotes, which its file must
//!   hold: found by their text, each run of whitespace read as one space, so
//!   words that only moved, or were indented or wrapped again where a space
//!   was, are still found;
//! - `Claude Code` and the version the note was seen with;
//! - `recheck after` and a date, for what no anchor can watch.
//!
//! A path is read from the folder of the project whose board it is, unless
//! it starts at `/` or `~/`. Every write that changes such a note's text, or
//! makes a note one of those kinds, reads the line again, whatever wrote it,
//! and records what it found, with when and by whose write: the fingerprints
//! a later read of the note compares against.
//!
//! prime, context and search check the anchors of the notes they show (task
//! 1325): a path gone, words their file no longer holds, a date past, or a
//! version older than the Claude Code reading say to recheck the note, which
//! shows why and is never dropped.
//!
//! A recheck ends one of two ways (task 1326). A note found still true takes
//! a line that starts with `Still true`, dated by convention -- `Still true,
//! 2026-10-07: retried, the same failure` -- and the write that adds it
//! reads the line again and records the recheck, with when, by whom and the
//! Claude Code its session runs. The recheck answers what time moves: a
//! version up to the one it ran, and a date to recheck after that it came
//! after. A path gone, or words their file no longer holds, it does not
//! answer: the line names what holds now, or leaves them out, as Swimm asks
//! of a snippet whose code is gone. A note no longer true is superseded, and
//! a superseded note is history, checked no more.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{NaiveDate, TimeZone};

use crate::holder::{Actor, Holder};
use crate::item::{Anchor, Item, Knowledge, RestsOn, StillTrue, Unread};
use crate::storage::ItemMap;

/// The words that open the line, in any case.
const OPENING: &str = "rests on:";

/// What an anchor is, said where a part of the line names none.
const ANCHOR: &str =
    "an anchor is a path in backticks, a path and words in double quotes, Claude Code and its version, or recheck after a date";

/// The words that open a line saying a recheck found the note still true,
/// in any case.
const STILL_TRUE: &str = "still true";

/// Said to a recheck that leaves a path or words that do not hold.
const UNANSWERED: &str =
    "a Still true line answers a version or a date, not a path or words that do not hold: name what holds now, or take them out";

/// Who writes: the session's or the person's actor, and the version of
/// Claude Code the session runs, where its MCP client gave one.
#[derive(Clone, Copy, Default)]
pub struct Writer<'a> {
    pub actor: Option<&'a Actor>,
    pub claude_code: Option<&'a str>,
}

/// Reads the line again on each decision, gotcha or procedure among
/// `changed` whose text this write changed, or which it made one of those
/// kinds, and drops the record from any task or plain note among them; a
/// note of a kind a later version added keeps what it had. `before` and
/// `arrived` hold the items as they were, and `writer` writes; a write that
/// adds a `Still true` line records the recheck too. Returns the notes
/// whose line it read.
pub fn keep(
    before: &ItemMap,
    arrived: &ItemMap,
    data: &mut ItemMap,
    changed: &[u32],
    folder: Option<&Path>,
    writer: Writer<'_>,
    now: i64,
) -> Vec<u32> {
    let mut read = Vec::new();
    for id in changed {
        let Some(item) = data.get_mut(id) else { continue };
        if item.is_task || item.knowledge.is_none() {
            item.rests_on = None;
            continue;
        }
        // A kind a later version added (task 845): what it rests on, and the
        // record of it, are that version's.
        if !lasting(item) {
            continue;
        }
        let old = before.get(id).or_else(|| arrived.get(id));
        if old.is_some_and(|old| lasting(old) && old.description == item.description) {
            continue;
        }
        let mut rests = rests_on(&item.description, folder, now, || writer.actor.map(|actor| actor.holder(now)));
        if let Some(rests) = rests.as_mut() {
            rests.still_true = still_true(&item.description, old, writer, now);
        }
        item.rests_on = rests.map(Box::new);
        if item.rests_on.is_some() {
            read.push(*id);
        }
    }
    read
}

/// The recheck a note's `text` keeps after a write by `writer` at `now`
/// (task 1326): this write's, where it added a `Still true` line to what
/// the note said as `old`; else `old`'s, while such a line is left; else
/// none.
fn still_true(text: &str, old: Option<&Item>, writer: Writer<'_>, now: i64) -> Option<StillTrue> {
    let said: Vec<&str> = text.lines().filter(|line| is_still_true(line)).collect();
    if said.is_empty() {
        return None;
    }
    let earlier = old.and_then(|old| old.rests_on.as_deref()).and_then(|rests| rests.still_true.clone());
    let was = old.map_or("", |old| old.description.as_str());
    if said.iter().all(|line| was.lines().any(|then| then == *line)) {
        return earlier;
    }
    // The newest version a recheck ran: an earlier one's stays where this
    // one gave none, or an older one.
    let claude_code = match (writer.claude_code, earlier.and_then(|earlier| earlier.claude_code)) {
        (Some(now), Some(then)) if newer(&then, now) => Some(then),
        (Some(now), _) => Some(now.to_string()),
        (None, then) => then,
    };
    Some(StillTrue { at: now, by: writer.actor.map(|actor| actor.holder(now)), claude_code, unknown: Default::default() })
}

/// Whether `line` starts with `Still true`, in any case and however
/// indented, with no letter or digit running on from it.
fn is_still_true(line: &str) -> bool {
    let line = line.trim_start();
    line.get(..STILL_TRUE.len()).is_some_and(|head| head.eq_ignore_ascii_case(STILL_TRUE))
        && !line[STILL_TRUE.len()..].starts_with(char::is_alphanumeric)
}

/// Whether the recheck `still` came after `date`, a date to recheck after
/// written YYYY-MM-DD, and so answers it.
fn answers(still: &StillTrue, date: &str) -> bool {
    NaiveDate::parse_from_str(date, "%Y-%m-%d").is_ok_and(|date| day_of(still.at) > date)
}

/// The day, here, of `at` in epoch milliseconds.
fn day_of(at: i64) -> NaiveDate {
    chrono::Local.timestamp_millis_opt(at).single().unwrap_or_else(chrono::Local::now).date_naive()
}

/// Whether `item` is a decision, gotcha or procedure: a note of a kind this
/// version knows.
pub fn lasting(item: &Item) -> bool {
    !item.is_task && matches!(item.knowledge, Some(Knowledge::Decision | Knowledge::Gotcha | Knowledge::Procedure))
}

/// What the `Rests on:` lines of `text` name, read at `now` for whoever `by`
/// makes, only once there is a line; None when it has none.
pub fn rests_on(text: &str, folder: Option<&Path>, now: i64, by: impl FnOnce() -> Option<Holder>) -> Option<RestsOn> {
    let lines: Vec<&str> = text.lines().filter_map(opened).collect();
    if lines.is_empty() {
        return None;
    }
    let today = day_of(now);
    let files = Files::default();
    let mut rests = RestsOn { at: now, by: by(), anchors: Vec::new(), unread: Vec::new(), still_true: None, unknown: Default::default() };
    for line in lines {
        let parts = split(line);
        if parts.is_empty() {
            rests.unread.push(Unread { text: String::new(), why: format!("the line names nothing: {ANCHOR}"), unknown: Default::default() });
        }
        for part in parts {
            match anchor(part, folder, today, &files) {
                Ok(anchor) => rests.anchors.push(anchor),
                Err(why) => rests.unread.push(Unread { text: part.to_string(), why, unknown: Default::default() }),
            }
        }
    }
    Some(rests)
}

/// What `rests` found not to hold, or could not read, each in a few words:
/// what the reply to the write says. A date a recheck came after is
/// answered, and a recheck made by this very write is told what it leaves
/// to recheck (task 1326).
pub fn problems(rests: &RestsOn) -> Vec<String> {
    let mut said = Vec::new();
    let still = rests.still_true.as_ref();
    for anchor in rests.anchors.iter().filter(|anchor| anchor.held == Some(false)) {
        said.push(match (&anchor.path, &anchor.words, &anchor.recheck_after) {
            (Some(path), Some(words), _) => format!("`{path}` does not hold \"{words}\""),
            (Some(path), None, _) => format!("`{path}` is not there"),
            (None, _, Some(date)) if still.is_some_and(|still| answers(still, date)) => continue,
            (None, _, Some(date)) => format!("recheck after {date} is past"),
            _ => continue,
        });
    }
    // A recheck this very write made is taken at the moment the line is read.
    if let Some(still) = still.filter(|still| still.at == rests.at) {
        if rests.anchors.iter().any(|anchor| anchor.path.is_some() && anchor.held == Some(false)) {
            said.push(UNANSWERED.to_string());
        }
        let seen = rests.anchors.iter().find_map(|anchor| anchor.claude_code.as_deref()).filter(|_| still.claude_code.is_none());
        if let Some(seen) = seen {
            said.push(format!(
                "this Still true knew no Claude Code version, so the note is still seen with {seen}: write the version you checked with in its place"
            ));
        }
    }
    for unread in &rests.unread {
        said.push(match unread.text.as_str() {
            "" => unread.why.clone(),
            text => format!("\"{text}\" is no anchor: {}", unread.why),
        });
    }
    said
}

/// Why the `Rests on:` lines of a note's `text` say to recheck it, read now
/// (task 1325), each in a few words: an anchor that does not hold today --
/// worded by whether it held when `kept`, the record of the write, read it
/// -- and a version of Claude Code older than `claude_code`, the one the
/// session reading runs, where it is known. The recheck `kept` holds, if
/// any, answers a version up to the one it ran and a date it came after
/// (task 1326). Empty when every anchor holds. A part that names no anchor
/// says nothing here: the reply to the write that stored it did.
pub fn recheck(
    text: &str,
    kept: Option<&RestsOn>,
    folder: Option<&Path>,
    claude_code: Option<&str>,
    today: NaiveDate,
    files: &Files,
) -> Vec<String> {
    let still = kept.and_then(|kept| kept.still_true.as_ref());
    let mut why = Vec::new();
    for part in text.lines().filter_map(opened).flat_map(split) {
        let Ok(now) = anchor(part, folder, today, files) else { continue };
        if let Some(seen) = &now.claude_code {
            let rechecked = still.and_then(|still| still.claude_code.as_deref()).filter(|then| newer(then, seen));
            if let Some(reading) = claude_code.filter(|reading| newer(reading, rechecked.unwrap_or(seen))) {
                why.push(match rechecked {
                    Some(then) => format!("still true with Claude Code {then}, now {reading}"),
                    None => format!("seen with Claude Code {seen}, now {reading}"),
                });
            }
            continue;
        }
        if now.held != Some(false) || still.zip(now.recheck_after.as_deref()).is_some_and(|(still, date)| answers(still, date)) {
            continue;
        }
        let held = kept.is_some_and(|kept| {
            kept.anchors.iter().any(|then| {
                then.held == Some(true) && (&then.path, &then.words, &then.recheck_after) == (&now.path, &now.words, &now.recheck_after)
            })
        });
        let there = |path: &str| resolve(path, folder).is_ok_and(|file| files.there(&file));
        why.push(match (&now.path, &now.words, &now.recheck_after, held) {
            (Some(path), _, _, true) if !there(path) => format!("`{path}` is gone"),
            (Some(path), _, _, false) if !there(path) => format!("`{path}` is not there"),
            (Some(path), Some(words), _, true) => format!("`{path}` no longer holds \"{words}\""),
            (Some(path), Some(words), _, false) => format!("`{path}` does not hold \"{words}\""),
            (None, _, Some(date), _) => format!("recheck after {date} is past"),
            _ => continue,
        });
    }
    why
}

/// Whether `reading`, the version of Claude Code a session runs, is past
/// `seen`, the one a note was seen with, to the precision `seen` gives: seen
/// with 2.1, a note holds through every 2.1.x. A version that does not start
/// with a number is past none.
fn newer(reading: &str, seen: &str) -> bool {
    let numbers = |version: &str| -> Vec<u64> {
        let version = version.trim().trim_start_matches('v');
        let lead = version.split(|c: char| !c.is_ascii_digit() && c != '.').next().unwrap_or_default();
        lead.split('.').map_while(|number| number.parse().ok()).collect()
    };
    let (reading, seen) = (numbers(reading), numbers(seen));
    if reading.is_empty() {
        return false;
    }
    let reading: Vec<u64> = (0..seen.len()).map(|at| reading.get(at).copied().unwrap_or(0)).collect();
    reading > seen
}

/// The files a read of anchors looks at, each once however many anchors
/// name it: a view checks every note it shows, and several often rest on
/// one file.
#[derive(Default)]
pub struct Files {
    there: RefCell<HashMap<PathBuf, bool>>,
    /// Each file's text squeezed (see `squeeze`); None for what is no file
    /// or cannot be read.
    texts: RefCell<HashMap<PathBuf, Option<Squeezed>>>,
}

impl Files {
    /// Whether anything is at `path`.
    fn there(&self, path: &Path) -> bool {
        *self.there.borrow_mut().entry(path.to_path_buf()).or_insert_with(|| path.exists())
    }

    /// The line, from 1, where `file` holds `words`, each run of whitespace
    /// in either read as one space; None where it holds them nowhere, or is
    /// no file.
    fn holds(&self, file: &Path, words: &str) -> Option<u32> {
        let (wanted, _) = squeeze(words);
        let wanted = wanted.trim();
        if wanted.is_empty() {
            return None;
        }
        let mut texts = self.texts.borrow_mut();
        let (text, starts) = texts.entry(file.to_path_buf()).or_insert_with(|| squeezed(file)).as_ref()?;
        let at = text.find(wanted)?;
        Some(starts.partition_point(|&start| start <= at) as u32)
    }
}

/// What follows the opening words, on a line that starts with them.
fn opened(line: &str) -> Option<&str> {
    let line = line.trim_start();
    let head = line.get(..OPENING.len())?;
    head.eq_ignore_ascii_case(OPENING).then(|| &line[OPENING.len()..])
}

/// The parts of `line` between semicolons, but for one inside backticks or
/// double quotes, trimmed, with the empty ones left out. A backtick or quote
/// left open would take the rest of the line into one part, so a line with
/// one is split at every semicolon instead.
fn split(line: &str) -> Vec<&str> {
    let (mut parts, mut start, mut tick, mut quote) = (Vec::new(), 0, false, false);
    for (at, c) in line.char_indices() {
        match c {
            '`' if !quote => tick = !tick,
            '"' if !tick => quote = !quote,
            ';' if !tick && !quote => {
                parts.push(&line[start..at]);
                start = at + 1;
            }
            _ => {}
        }
    }
    parts.push(&line[start..]);
    if tick || quote {
        parts = line.split(';').collect();
    }
    parts.into_iter().map(str::trim).filter(|part| !part.is_empty()).collect()
}

/// The anchor `part` names, read at `today` through `files`, or why it
/// names none.
fn anchor(part: &str, folder: Option<&Path>, today: NaiveDate, files: &Files) -> Result<Anchor, String> {
    let mut anchor =
        Anchor { path: None, words: None, claude_code: None, recheck_after: None, held: None, line: None, unknown: Default::default() };
    if let Some(rest) = part.strip_prefix('`') {
        let (path, after) = rest.split_once('`').ok_or("its path has no closing backtick")?;
        let path = path.trim();
        if path.is_empty() {
            return Err("its backticks hold no path".into());
        }
        let after = after.trim();
        let words = match after {
            "" => None,
            _ => Some(
                after
                    .strip_prefix('"')
                    .and_then(|words| words.strip_suffix('"'))
                    .filter(|words| !words.trim().is_empty())
                    .ok_or("after a path come words in double quotes, or nothing")?,
            ),
        };
        let file = resolve(path, folder)?;
        anchor.path = Some(path.to_string());
        match words {
            None => anchor.held = Some(files.there(&file)),
            Some(words) => {
                anchor.line = files.holds(&file, words);
                anchor.held = Some(anchor.line.is_some());
                anchor.words = Some(words.to_string());
            }
        }
        return Ok(anchor);
    }
    if let Some(version) = after_words(part, "claude code") {
        let version = version.strip_prefix('v').unwrap_or(version);
        if !is_version(version) {
            return Err("Claude Code takes the version the note was seen with, numbers between dots, as 2.1.289".into());
        }
        anchor.claude_code = Some(version.to_string());
        return Ok(anchor);
    }
    if let Some(date) = after_words(part, "recheck after") {
        let date = NaiveDate::parse_from_str(date, "%Y-%m-%d").map_err(|_| "a date to recheck after is written YYYY-MM-DD, as 2026-12-01")?;
        anchor.recheck_after = Some(date.format("%Y-%m-%d").to_string());
        anchor.held = Some(today <= date);
        return Ok(anchor);
    }
    Err(ANCHOR.into())
}

/// What follows `words` at the start of `part`, in any case, when a space or
/// nothing follows them.
fn after_words<'a>(part: &'a str, words: &str) -> Option<&'a str> {
    let head = part.get(..words.len())?;
    let rest = &part[words.len()..];
    (head.eq_ignore_ascii_case(words) && (rest.is_empty() || rest.starts_with(char::is_whitespace))).then(|| rest.trim())
}

/// Numbers between dots, one to four of them: 2.1.289.
fn is_version(text: &str) -> bool {
    let numbers: Vec<&str> = text.split('.').collect();
    numbers.len() <= 4 && numbers.iter().all(|number| !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()))
}

/// Where `path` is: from `folder`, unless it starts at `/` or `~/`.
fn resolve(path: &str, folder: Option<&Path>) -> Result<PathBuf, String> {
    if path == "~" || path.starts_with("~/") {
        let home = std::env::var_os("HOME").map(PathBuf::from).filter(|home| home.is_absolute());
        let home = home.ok_or_else(|| format!("{path} starts at ~, and HOME names no folder"))?;
        return Ok(path.strip_prefix("~/").map_or_else(|| home.clone(), |rest| home.join(rest)));
    }
    let given = Path::new(path);
    if given.is_absolute() {
        return Ok(given.to_path_buf());
    }
    folder
        .map(|folder| folder.join(given))
        .ok_or_else(|| format!("{path} is read from the project's folder, and this board is no project's: write it from / or ~/"))
}

/// The text of `file` squeezed (see `squeeze`); None where it is no file, or
/// cannot be read.
fn squeezed(file: &Path) -> Option<Squeezed> {
    if !std::fs::metadata(file).is_ok_and(|meta| meta.is_file()) {
        return None;
    }
    Some(squeeze(&String::from_utf8_lossy(&std::fs::read(file).ok()?)))
}

/// A text with each run of whitespace made one space, and where in it each
/// line of the text starts.
type Squeezed = (String, Vec<usize>);

/// `text` with each run of whitespace made one space, and where in it each
/// line of `text` starts: a line that starts inside a run starts after it.
fn squeeze(text: &str) -> Squeezed {
    let (mut squeezed, mut starts, mut space) = (String::with_capacity(text.len()), vec![0], false);
    for c in text.chars() {
        if !c.is_whitespace() {
            squeezed.push(c);
            space = false;
            continue;
        }
        if !space {
            squeezed.push(' ');
            space = true;
        }
        if c == '\n' {
            starts.push(squeezed.len());
        }
    }
    (squeezed, starts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 2026-10-07 at noon, here.
    fn noon() -> i64 {
        chrono::Local.with_ymd_and_hms(2026, 10, 7, 12, 0, 0).unwrap().timestamp_millis()
    }

    /// A project's folder holding src/storage.rs.
    fn project(tag: &str) -> PathBuf {
        let dir = crate::paths::test_dir(&format!("ekko-anchors-{tag}"));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/storage.rs"), "use std::fs;\n\npub fn keep_unkept_version(\n    data: &mut ItemMap,\n) {}\n").unwrap();
        dir
    }

    fn anchors(rests: &RestsOn) -> serde_json::Value {
        serde_json::to_value(&rests.anchors).unwrap()
    }

    /// Each kind of anchor is read, with what ekko found: a path there, from
    /// the project's folder or from `/`, and one not; words a file holds,
    /// with the line they begin on, and words it does not; the version as
    /// written; a date ahead, today's and one past.
    #[test]
    fn each_kind_of_anchor_is_read_with_what_it_found() {
        let dir = project("kinds");
        let absolute = dir.join("src/storage.rs").display().to_string();
        let text = format!(
            "Keep the version\nRests on: `src/storage.rs`; `{absolute}`; `src/gone.rs`; `src/storage.rs` \"pub fn keep_unkept_version\"; \
             `src/storage.rs` \"fn drop_it\"; Claude Code 2.1.289; recheck after 2026-12-01; recheck after 2026-10-07; recheck after 2026-10-06"
        );
        let rests = rests_on(&text, Some(&dir), noon(), || None).unwrap();
        assert_eq!(
            anchors(&rests),
            json!([
                {"path": "src/storage.rs", "held": true},
                {"path": absolute, "held": true},
                {"path": "src/gone.rs", "held": false},
                {"path": "src/storage.rs", "words": "pub fn keep_unkept_version", "held": true, "line": 3},
                {"path": "src/storage.rs", "words": "fn drop_it", "held": false},
                {"claudeCode": "2.1.289"},
                {"recheckAfter": "2026-12-01", "held": true},
                {"recheckAfter": "2026-10-07", "held": true},
                {"recheckAfter": "2026-10-06", "held": false},
            ])
        );
        assert!(rests.unread.is_empty(), "{:?}", rests.unread);
        assert_eq!((rests.at, rests.by.is_none()), (noon(), true));
        assert_eq!(
            problems(&rests),
            ["`src/gone.rs` is not there", "`src/storage.rs` does not hold \"fn drop_it\"", "recheck after 2026-10-06 is past"]
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Words are found by their text, each run of whitespace in the file and
    /// in the words read as one space: re-indented, wrapped where a space
    /// was, or moved down the file, they still hold, and the line says where
    /// they begin. Changed, or wrapped where no space was, they do not.
    #[test]
    fn words_are_found_by_their_text_whatever_whitespace_runs_between_them() {
        let dir = project("whitespace");
        let read = |words: &str| {
            let rests = rests_on(&format!("Rests on: `src/storage.rs` \"{words}\""), Some(&dir), noon(), || None).unwrap();
            (rests.anchors[0].held, rests.anchors[0].line)
        };
        assert_eq!(read("pub fn keep_unkept_version( data: &mut ItemMap, )"), (Some(true), Some(3)), "wrapped across three lines");
        assert_eq!(read("pub  fn\tkeep_unkept_version"), (Some(true), Some(3)));
        assert_eq!(read("data: &mut ItemMap"), (Some(true), Some(4)), "indented");
        assert_eq!(read("use std::fs;"), (Some(true), Some(1)));
        assert_eq!(read("keep_unkept_version(data"), (Some(false), None), "the file breaks the line where the words have no space");
        assert_eq!(read("pub fn keep_kept_version"), (Some(false), None));

        std::fs::write(dir.join("src/storage.rs"), "// moved\n\n\nuse std::fs;\n\n    pub fn keep_unkept_version(data: &mut ItemMap) {}\n").unwrap();
        assert_eq!(read("pub fn keep_unkept_version( data: &mut ItemMap, )"), (Some(false), None), "its words changed");
        assert_eq!(read("pub fn keep_unkept_version"), (Some(true), Some(6)), "moved down and indented");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A part that names no anchor is kept as written, with why, and the
    /// parts around it are still read; a backtick left open does not take
    /// the rest of the line with it. A line that names nothing is said too.
    #[test]
    fn a_part_ekko_cannot_read_is_kept_with_why() {
        let dir = project("unread");
        let text = "x\nRests on: the readme; `src/storage.rs; Claude Code; Claude Code 2.1.x; recheck after 2026-13-01; \
                    `src/storage.rs` words; ``; `src/storage.rs` \"\"; Claude Code v2.1.289\nRests on:  ";
        let rests = rests_on(text, Some(&dir), noon(), || None).unwrap();
        assert_eq!(anchors(&rests), json!([{"claudeCode": "2.1.289"}]));
        let unread: Vec<(&str, &str)> = rests.unread.iter().map(|unread| (unread.text.as_str(), unread.why.as_str())).collect();
        assert_eq!(
            unread,
            [
                ("the readme", ANCHOR),
                ("`src/storage.rs", "its path has no closing backtick"),
                ("Claude Code", "Claude Code takes the version the note was seen with, numbers between dots, as 2.1.289"),
                ("Claude Code 2.1.x", "Claude Code takes the version the note was seen with, numbers between dots, as 2.1.289"),
                ("recheck after 2026-13-01", "a date to recheck after is written YYYY-MM-DD, as 2026-12-01"),
                ("`src/storage.rs` words", "after a path come words in double quotes, or nothing"),
                ("``", "its backticks hold no path"),
                ("`src/storage.rs` \"\"", "after a path come words in double quotes, or nothing"),
                ("", &*format!("the line names nothing: {ANCHOR}")),
            ]
        );
        assert_eq!(problems(&rests)[0], format!("\"the readme\" is no anchor: {ANCHOR}"));
        assert_eq!(problems(&rests)[8], format!("the line names nothing: {ANCHOR}"));

        let nowhere = rests_on("Rests on: `src/storage.rs`; `/etc`", None, noon(), || None).unwrap();
        assert_eq!(anchors(&nowhere), json!([{"path": "/etc", "held": true}]), "a board that is no project's reads a path from /");
        assert_eq!(
            nowhere.unread[0].why,
            "src/storage.rs is read from the project's folder, and this board is no project's: write it from / or ~/"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A semicolon inside backticks or quotes is the path's or the words',
    /// not the line's.
    #[test]
    fn a_semicolon_inside_backticks_or_quotes_splits_nothing() {
        let dir = project("semicolon");
        let rests = rests_on("Rests on: `a;b.rs`; `src/storage.rs` \"use std::fs;\"", Some(&dir), noon(), || None).unwrap();
        assert_eq!(
            anchors(&rests),
            json!([{"path": "a;b.rs", "held": false}, {"path": "src/storage.rs", "words": "use std::fs;", "held": true, "line": 1}])
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Only a line that starts with the words, in any case and however
    /// indented, is read, and every such line is: text that only mentions
    /// them rests on nothing.
    #[test]
    fn only_a_line_that_starts_with_rests_on_is_read() {
        let dir = project("lines");
        assert_eq!(rests_on("Plain words", Some(&dir), noon(), || None), None);
        assert_eq!(rests_on("The `Rests on:` line names what a note rests on", Some(&dir), noon(), || None), None);
        assert_eq!(rests_on("Rests on `src/storage.rs`", Some(&dir), noon(), || None), None, "no colon");
        let rests = rests_on("a\n  rests ON: `src/storage.rs`\nb\nRests on: Claude Code 2.1.289", Some(&dir), noon(), || None).unwrap();
        assert_eq!(anchors(&rests), json!([{"path": "src/storage.rs", "held": true}, {"claudeCode": "2.1.289"}]));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A day, here.
    fn day(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    /// A read says to recheck a note whose words their file no longer holds,
    /// or whose path is gone -- "no longer" and "gone" for what held when the
    /// note was written, and "not" for what never did, or where no record of
    /// the write says it did.
    #[test]
    fn changed_words_or_a_path_gone_say_to_recheck() {
        let dir = project("recheck-moved");
        std::fs::write(dir.join("src/old.rs"), "x").unwrap();
        let text = "Keep it\nRests on: `src/storage.rs` \"pub fn keep_unkept_version\"; `src/old.rs`; `src/never.rs`; `src/storage.rs` \"fn never\"";
        let kept = rests_on(text, Some(&dir), noon(), || None).unwrap();
        let read = |kept: Option<&RestsOn>| recheck(text, kept, Some(&dir), None, day(2026, 10, 7), &Files::default());
        assert_eq!(read(Some(&kept)), ["`src/never.rs` is not there", "`src/storage.rs` does not hold \"fn never\""], "as written");

        std::fs::write(dir.join("src/storage.rs"), "use std::fs;\n\npub fn keep_kept_version(\n").unwrap();
        std::fs::remove_file(dir.join("src/old.rs")).unwrap();
        assert_eq!(
            read(Some(&kept)),
            [
                "`src/storage.rs` no longer holds \"pub fn keep_unkept_version\"",
                "`src/old.rs` is gone",
                "`src/never.rs` is not there",
                "`src/storage.rs` does not hold \"fn never\"",
            ]
        );
        assert_eq!(read(None)[..2], ["`src/storage.rs` does not hold \"pub fn keep_unkept_version\"", "`src/old.rs` is not there"]);
        std::fs::remove_file(dir.join("src/storage.rs")).unwrap();
        assert_eq!(read(Some(&kept))[0], "`src/storage.rs` is gone", "the file of the words");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Words found again by their text say nothing when another line of
    /// their file changed, or when they moved down it, re-indented and
    /// wrapped where a space was; changed themselves, they do.
    #[test]
    fn an_unrelated_line_changed_or_the_words_moved_say_nothing() {
        let dir = project("recheck-still");
        let text = "Rests on: `src/storage.rs` \"pub fn keep_unkept_version( data: &mut ItemMap\"";
        let kept = rests_on(text, Some(&dir), noon(), || None).unwrap();
        let read = || recheck(text, Some(&kept), Some(&dir), None, day(2026, 10, 7), &Files::default());
        assert_eq!(read(), Vec::<String>::new(), "as written");
        std::fs::write(dir.join("src/storage.rs"), "use std::io;\n\npub fn keep_unkept_version(\n    data: &mut ItemMap,\n) {}\n").unwrap();
        assert_eq!(read(), Vec::<String>::new(), "another line changed");
        std::fs::write(dir.join("src/storage.rs"), "// first\n\nmod a;\n\n        pub fn keep_unkept_version(  data:\n &mut ItemMap) {}\n").unwrap();
        assert_eq!(read(), Vec::<String>::new(), "moved down, indented and wrapped");
        std::fs::write(dir.join("src/storage.rs"), "pub fn keep_unkept_version(state: &mut ItemMap) {}\n").unwrap();
        assert_eq!(read(), ["`src/storage.rs` no longer holds \"pub fn keep_unkept_version( data: &mut ItemMap\""]);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A read says to recheck a note seen with a version of Claude Code
    /// older than the one reading, to the precision its line gives, and one
    /// whose date to recheck after is past; the same version or an older one,
    /// none known, or the date itself say nothing.
    #[test]
    fn a_newer_claude_code_or_a_past_date_say_to_recheck() {
        let text = "Rests on: Claude Code 2.1.289; recheck after 2026-10-07";
        let read = |claude_code: Option<&str>, today: NaiveDate| recheck(text, None, None, claude_code, today, &Files::default());
        assert_eq!(read(Some("2.1.301"), day(2026, 10, 7)), ["seen with Claude Code 2.1.289, now 2.1.301"]);
        assert_eq!(read(Some("2.1.289"), day(2026, 10, 8)), ["recheck after 2026-10-07 is past"]);
        assert_eq!(read(Some("3.0.0"), day(2027, 1, 1)), ["seen with Claude Code 2.1.289, now 3.0.0", "recheck after 2026-10-07 is past"]);
        for not_newer in [Some("2.1.289"), Some("2.1.200"), Some("2.0.999"), Some("next"), None] {
            assert_eq!(read(not_newer, day(2026, 10, 7)), Vec::<String>::new(), "{not_newer:?}");
        }
        assert!(newer("2.2.0", "2.1") && !newer("2.1.999", "2.1"), "seen with 2.1, a note holds through every 2.1.x");
        assert!(newer("2.1.290-beta.1", "2.1.289") && newer("v2.1.290", "2.1.289") && !newer("2", "2.0.1"));
    }

    /// A gotcha whose text is `text`, as item 1 of a board.
    fn gotcha(text: &str) -> ItemMap {
        let mut note = Item::new_note(1, text.into(), vec!["My Board".into()]);
        note.knowledge = Some(Knowledge::Gotcha);
        ItemMap::from([(1, note)])
    }

    /// Item 1 of `data`, its text changed by `change` in a write by
    /// `writer` at `at`.
    fn edit(data: &mut ItemMap, change: impl FnOnce(&mut String), folder: Option<&Path>, writer: Writer<'_>, at: i64) {
        let before = data.clone();
        change(&mut data.get_mut(&1).unwrap().description);
        keep(&before, &ItemMap::new(), data, &[1], folder, writer, at);
    }

    /// Noon, `days` after `noon`.
    fn days_on(days: i64) -> i64 {
        noon() + days * 86_400_000
    }

    /// A write that adds a `Still true` line records the recheck, with when,
    /// by whom and the Claude Code its session ran (task 1326), and a read
    /// says no more to recheck what it answers: a version up to the one it
    /// ran, a date it came after. A newer version says to recheck again; a
    /// later write keeps the recheck while the line stays, a later recheck
    /// keeps the newest version, and a recheck that knew no version, as in
    /// the terminal, answers none and is told so.
    #[test]
    fn a_still_true_line_answers_a_newer_version_and_a_date_it_came_after() {
        let (me, other, _) = crate::holder::test_sessions();
        let session = |actor, version| Writer { actor: Some(actor), claude_code: Some(version) };
        let read = |data: &ItemMap, claude_code: &str, today: NaiveDate| {
            let note = &data[&1];
            recheck(&note.description, note.rests_on.as_deref(), None, Some(claude_code), today, &Files::default())
        };
        let still = |data: &ItemMap| data[&1].rests_on.as_deref().unwrap().still_true.clone();
        let mut data = gotcha("A trap\nRests on: Claude Code 2.1.200; recheck after 2026-10-01; recheck after 2026-10-07");
        keep(&ItemMap::new(), &ItemMap::new(), &mut data, &[1], None, session(&me, "2.1.292"), noon());
        assert_eq!(read(&data, "2.1.292", day(2026, 10, 7)), ["seen with Claude Code 2.1.200, now 2.1.292", "recheck after 2026-10-01 is past"]);
        assert_eq!(still(&data), None, "written without the line");

        edit(&mut data, |text| text.push_str("\nStill true, 2026-10-07: retried, the same failure"), None, session(&me, "2.1.292"), noon());
        let rechecked = still(&data).expect("the recheck is recorded");
        assert_eq!((rechecked.at, rechecked.claude_code.as_deref()), (noon(), Some("2.1.292")));
        assert!(rechecked.by.as_ref().is_some_and(|by| me.is(by)), "{:?}", rechecked.by);
        assert_eq!(read(&data, "2.1.292", day(2026, 10, 7)), Vec::<String>::new());
        assert!(problems(data[&1].rests_on.as_deref().unwrap()).is_empty());
        assert_eq!(read(&data, "2.1.300", day(2026, 10, 7)), ["still true with Claude Code 2.1.292, now 2.1.300"]);
        assert_eq!(read(&data, "2.1.292", day(2026, 10, 8)), ["recheck after 2026-10-07 is past"], "a recheck on the day comes after nothing");

        edit(&mut data, |text| *text = text.replacen("A trap", "A trap, restated", 1), None, session(&other, "2.1.250"), days_on(1));
        assert_eq!(still(&data), Some(rechecked.clone()), "a later write keeps the recheck while its line stays");
        edit(&mut data, |text| text.push_str("\nStill true, 2026-10-08"), None, session(&other, "2.1.250"), days_on(1));
        let again = still(&data).unwrap();
        assert_eq!((again.at, again.claude_code.as_deref()), (days_on(1), Some("2.1.292")), "the newest version a recheck ran");
        assert!(again.by.as_ref().is_some_and(|by| other.is(by)), "{:?}", again.by);
        assert_eq!(read(&data, "2.1.292", day(2026, 10, 9)), Vec::<String>::new(), "the 8th came after the 7th");
        edit(&mut data, |text| *text = text.lines().filter(|line| !is_still_true(line)).collect::<Vec<_>>().join("\n"), None, session(&me, "2.1.292"), days_on(1));
        assert_eq!(still(&data), None, "the lines gone, so is the recheck");

        let mut data = gotcha("A trap\nRests on: Claude Code 2.1.200; recheck after 2026-10-01");
        keep(&ItemMap::new(), &ItemMap::new(), &mut data, &[1], None, Writer::default(), noon());
        edit(&mut data, |text| text.push_str("\n  still TRUE"), None, Writer::default(), noon());
        assert_eq!((still(&data).map(|still| still.claude_code), read(&data, "2.1.292", day(2026, 10, 7))), (Some(None), vec![
            "seen with Claude Code 2.1.200, now 2.1.292".to_string()
        ]));
        assert_eq!(
            problems(data[&1].rests_on.as_deref().unwrap()),
            ["this Still true knew no Claude Code version, so the note is still seen with 2.1.200: write the version you checked with in its place"]
        );
        edit(&mut data, |text| *text = text.replacen("A trap", "A trap, restated", 1), None, Writer::default(), days_on(1));
        assert!(problems(data[&1].rests_on.as_deref().unwrap()).is_empty(), "a later write is no recheck, and its date stays answered");

        for line in ["Still true", "Still true, 2026-10-07: retried", "Still true: yes", "Still true (2026-10-07)", "  sTILL true."] {
            assert!(is_still_true(line), "{line}");
        }
        for line in ["Still trueish", "It is still true", "Still-true", "Still", "Still truth"] {
            assert!(!is_still_true(line), "{line}");
        }
    }

    /// A recheck does not answer a path gone, or words their file no longer
    /// holds (task 1326): the note still says to recheck them, now as the
    /// recheck found them, and the reply to it says to name what holds now.
    /// Named anew, they hold, and the recheck stays.
    #[test]
    fn a_still_true_line_answers_no_path_or_words_that_do_not_hold() {
        let dir = project("still-words");
        std::fs::write(dir.join("src/old.rs"), "x").unwrap();
        let mut data = gotcha("Keep it\nRests on: `src/storage.rs` \"pub fn keep_unkept_version\"; `src/old.rs`");
        keep(&ItemMap::new(), &ItemMap::new(), &mut data, &[1], Some(&dir), Writer::default(), noon());
        std::fs::write(dir.join("src/storage.rs"), "pub fn keep_kept_version() {}\n").unwrap();
        std::fs::remove_file(dir.join("src/old.rs")).unwrap();
        let read = |data: &ItemMap| {
            let note = &data[&1];
            recheck(&note.description, note.rests_on.as_deref(), Some(&dir), None, day(2026, 10, 7), &Files::default())
        };
        assert_eq!(read(&data), ["`src/storage.rs` no longer holds \"pub fn keep_unkept_version\"", "`src/old.rs` is gone"]);

        edit(&mut data, |text| text.push_str("\nStill true, 2026-10-07: renamed, the same rule"), Some(&dir), Writer::default(), noon());
        assert_eq!(read(&data), ["`src/storage.rs` does not hold \"pub fn keep_unkept_version\"", "`src/old.rs` is not there"], "still to recheck");
        let rests = data[&1].rests_on.as_deref().unwrap();
        assert!(rests.still_true.is_some());
        assert_eq!(
            problems(rests),
            ["`src/storage.rs` does not hold \"pub fn keep_unkept_version\"", "`src/old.rs` is not there", UNANSWERED]
        );
        edit(&mut data, |text| *text = text.replacen("Keep it", "Keep it so", 1), Some(&dir), Writer::default(), days_on(1));
        let unheld = ["`src/storage.rs` does not hold \"pub fn keep_unkept_version\"", "`src/old.rs` is not there"];
        assert_eq!(problems(data[&1].rests_on.as_deref().unwrap()), unheld, "a later write is no recheck");

        let named = |text: &mut String| *text = text.replace("keep_unkept_version", "keep_kept_version").replace("; `src/old.rs`", "");
        edit(&mut data, named, Some(&dir), Writer::default(), days_on(1));
        assert_eq!(read(&data), Vec::<String>::new(), "named anew");
        let rests = data[&1].rests_on.as_deref().unwrap();
        assert_eq!((rests.still_true.as_ref().map(|still| still.at), problems(rests)), (Some(noon()), vec![]), "the recheck stays");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A kind a later version added is no decision, gotcha or procedure
    /// (task 845): a write leaves its line unread, and whatever record that
    /// version keeps on it as it was.
    #[test]
    fn a_kind_a_later_version_added_is_left_as_it_is() {
        let dir = project("later");
        let mut note = Item::new_note(1, "x\nRests on: `src/storage.rs`".into(), vec!["My Board".into()]);
        note.knowledge = Some(Knowledge::Unknown("lesson"));
        let mut data = ItemMap::from([(1, note)]);
        let read = keep(&ItemMap::new(), &ItemMap::new(), &mut data, &[1], Some(&dir), Writer::default(), noon());
        assert_eq!((read, data[&1].rests_on.is_none()), (vec![], true), "a new note of that kind");

        let kept = rests_on("Rests on: `src/gone.rs`", Some(&dir), noon(), || None).unwrap();
        let before = data.clone();
        let note = data.get_mut(&1).unwrap();
        note.rests_on = Some(Box::new(kept.clone()));
        note.description.push_str("\nmore");
        keep(&before, &ItemMap::new(), &mut data, &[1], Some(&dir), Writer::default(), noon());
        assert_eq!(data[&1].rests_on.as_deref(), Some(&kept), "its text changed");

        let before = data.clone();
        data.get_mut(&1).unwrap().knowledge = Some(Knowledge::Gotcha);
        let read = keep(&before, &ItemMap::new(), &mut data, &[1], Some(&dir), Writer::default(), noon());
        assert_eq!(read, [1], "made a gotcha, its text as it was");
        assert_eq!(anchors(data[&1].rests_on.as_deref().unwrap()), json!([{"path": "src/storage.rs", "held": true}]));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// `~` is the home folder, and `~/` starts a path there.
    #[test]
    fn a_path_from_home_starts_at_home() {
        let home = PathBuf::from(std::env::var_os("HOME").unwrap());
        assert_eq!(resolve("~/notes/a.md", None), Ok(home.join("notes/a.md")));
        assert_eq!(resolve("~", None), Ok(home));
        assert_eq!(resolve("~x/a.md", Some(Path::new("/p"))), Ok(PathBuf::from("/p/~x/a.md")), "only ~ and ~/ are home");
    }
}
