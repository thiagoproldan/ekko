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

use std::path::{Path, PathBuf};

use chrono::{NaiveDate, TimeZone};

use crate::holder::{Actor, Holder};
use crate::item::{Anchor, Item, Knowledge, RestsOn, Unread};
use crate::storage::ItemMap;

/// The words that open the line, in any case.
const OPENING: &str = "rests on:";

/// What an anchor is, said where a part of the line names none.
const ANCHOR: &str =
    "an anchor is a path in backticks, a path and words in double quotes, Claude Code and its version, or recheck after a date";

/// Reads the line again on each decision, gotcha or procedure among
/// `changed` whose text this write changed, or which it made one of those
/// kinds, and drops the record from any task or plain note among them; a
/// note of a kind a later version added keeps what it had. `before` and
/// `arrived` hold the items as they were, and `actor` writes. Returns the
/// notes whose line it read.
pub fn keep(
    before: &ItemMap,
    arrived: &ItemMap,
    data: &mut ItemMap,
    changed: &[u32],
    folder: Option<&Path>,
    actor: Option<&Actor>,
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
        item.rests_on = rests_on(&item.description, folder, now, || actor.map(|actor| actor.holder(now))).map(Box::new);
        if item.rests_on.is_some() {
            read.push(*id);
        }
    }
    read
}

/// Whether `item` is a decision, gotcha or procedure: a note of a kind this
/// version knows.
fn lasting(item: &Item) -> bool {
    !item.is_task && matches!(item.knowledge, Some(Knowledge::Decision | Knowledge::Gotcha | Knowledge::Procedure))
}

/// What the `Rests on:` lines of `text` name, read at `now` for whoever `by`
/// makes, only once there is a line; None when it has none.
pub fn rests_on(text: &str, folder: Option<&Path>, now: i64, by: impl FnOnce() -> Option<Holder>) -> Option<RestsOn> {
    let lines: Vec<&str> = text.lines().filter_map(opened).collect();
    if lines.is_empty() {
        return None;
    }
    let today = chrono::Local.timestamp_millis_opt(now).single().unwrap_or_else(chrono::Local::now).date_naive();
    let mut rests = RestsOn { at: now, by: by(), anchors: Vec::new(), unread: Vec::new(), unknown: Default::default() };
    for line in lines {
        let parts = split(line);
        if parts.is_empty() {
            rests.unread.push(Unread { text: String::new(), why: format!("the line names nothing: {ANCHOR}"), unknown: Default::default() });
        }
        for part in parts {
            match anchor(part, folder, today) {
                Ok(anchor) => rests.anchors.push(anchor),
                Err(why) => rests.unread.push(Unread { text: part.to_string(), why, unknown: Default::default() }),
            }
        }
    }
    Some(rests)
}

/// What `rests` found not to hold, or could not read, each in a few words:
/// what the reply to the write says.
pub fn problems(rests: &RestsOn) -> Vec<String> {
    let mut said = Vec::new();
    for anchor in rests.anchors.iter().filter(|anchor| anchor.held == Some(false)) {
        said.push(match (&anchor.path, &anchor.words, &anchor.recheck_after) {
            (Some(path), Some(words), _) => format!("`{path}` does not hold \"{words}\""),
            (Some(path), None, _) => format!("`{path}` is not there"),
            (None, _, Some(date)) => format!("recheck after {date} is past"),
            _ => continue,
        });
    }
    for unread in &rests.unread {
        said.push(match unread.text.as_str() {
            "" => unread.why.clone(),
            text => format!("\"{text}\" is no anchor: {}", unread.why),
        });
    }
    said
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

/// The anchor `part` names, read at `today`, or why it names none.
fn anchor(part: &str, folder: Option<&Path>, today: NaiveDate) -> Result<Anchor, String> {
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
            None => anchor.held = Some(file.exists()),
            Some(words) => {
                anchor.line = holds(&file, words);
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

/// The line, from 1, where `file` holds `words`, each run of whitespace in
/// either read as one space; None where it holds them nowhere, or is no file.
fn holds(file: &Path, words: &str) -> Option<u32> {
    if !std::fs::metadata(file).is_ok_and(|meta| meta.is_file()) {
        return None;
    }
    let text = String::from_utf8_lossy(&std::fs::read(file).ok()?).into_owned();
    let (squeezed, starts) = squeeze(&text);
    let (wanted, _) = squeeze(words);
    let wanted = wanted.trim();
    if wanted.is_empty() {
        return None;
    }
    let at = squeezed.find(wanted)?;
    Some(starts.partition_point(|&start| start <= at) as u32)
}

/// `text` with each run of whitespace made one space, and where in it each
/// line of `text` starts: a line that starts inside a run starts after it.
fn squeeze(text: &str) -> (String, Vec<usize>) {
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

    /// A kind a later version added is no decision, gotcha or procedure
    /// (task 845): a write leaves its line unread, and whatever record that
    /// version keeps on it as it was.
    #[test]
    fn a_kind_a_later_version_added_is_left_as_it_is() {
        let dir = project("later");
        let mut note = Item::new_note(1, "x\nRests on: `src/storage.rs`".into(), vec!["My Board".into()]);
        note.knowledge = Some(Knowledge::Unknown("lesson"));
        let mut data = ItemMap::from([(1, note)]);
        let read = keep(&ItemMap::new(), &ItemMap::new(), &mut data, &[1], Some(&dir), None, noon());
        assert_eq!((read, data[&1].rests_on.is_none()), (vec![], true), "a new note of that kind");

        let kept = rests_on("Rests on: `src/gone.rs`", Some(&dir), noon(), || None).unwrap();
        let before = data.clone();
        let note = data.get_mut(&1).unwrap();
        note.rests_on = Some(Box::new(kept.clone()));
        note.description.push_str("\nmore");
        keep(&before, &ItemMap::new(), &mut data, &[1], Some(&dir), None, noon());
        assert_eq!(data[&1].rests_on.as_deref(), Some(&kept), "its text changed");

        let before = data.clone();
        data.get_mut(&1).unwrap().knowledge = Some(Knowledge::Gotcha);
        let read = keep(&before, &ItemMap::new(), &mut data, &[1], Some(&dir), None, noon());
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
