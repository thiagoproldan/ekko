//! Open tasks a new one may repeat, named in create's reply (task 1582).
//!
//! In the six-session run of 2026-10-09 one failure was filed three times:
//! task 1577, then 1579 85 s later from another session, then 1598 from a
//! third, each create answered with nothing but its id. Their first lines
//! share almost no words -- "Two stand-in tests failed at spawn in other
//! sessions' runs", "tests/cli.rs: a doctor test failed once to spawn the
//! script it had just written", "CLI tests fail at times: a stand-in
//! written, then run, gets Text file busy" -- so a likeness of titles, such
//! as agentmemory's Jaccard over 0.9 (note 1454), misses them. Their bodies
//! share what only that failure has: the test's name, tests/cli.rs:1022,
//! ExecutableFileBusy, ETXTBSY.
//!
//! So the rule is that of Lucene's MoreLikeThis: the new text's terms, kept
//! only when few subjects on the board hold them, matched against each open
//! task with the notes attached to it. A term is a name in code -- an
//! identifier, a path, a file:line, an error's name -- never a word of
//! prose: on a small board every word is rare, and two tasks share three
//! words of English by chance. An open task that holds three of the rare
//! terms is named, newest first. Only a notice: the create is written all
//! the same.
//!
//! Replayed on this board on 2026-10-12 -- each of its 504 tasks against the
//! tasks open when it was filed, a closed one counted open until its last
//! change -- it named something for 13: 1579 (1577), 1598 (1579), 1640
//! (1444, which 1640 replaced), 1364 (1354, the same title), and nine
//! others: for most the task named was its source or a close relative, and
//! for 909 (540) and 1253 (1252) a weak link. Each call took 27 ms over
//! the whole board, built for release.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::hash::{DefaultHasher, Hash as _, Hasher as _};
use std::sync::{Arc, Mutex, PoisonError};

use crate::item::{Item, State};
use crate::storage::ItemMap;

/// A term is rare while at most this many subjects hold it, or one in
/// `RARE_PER` of the board's subjects when that is more.
const RARE: usize = 3;
const RARE_PER: usize = 100;
/// What an open task must share of the new text's rare terms to be named.
const SHARED: usize = 3;
/// What a notice names at most.
const NAMED: usize = 3;
/// A term this short is a word's stub, seldom the point.
const SHORTEST: usize = 3;

/// The terms of `text`, each once: the runs of letters and digits joined by
/// `_ : / . -`, and the parts such a run has between `::`, `/` and `.`,
/// each kept only when it is shaped as a name in code -- see `named` -- and
/// a run ending in :N also without it, so a file:line:column meets the same
/// file:line.
pub(crate) fn terms(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut keep = |run: &str| {
        let run = run.trim_matches(|c: char| !c.is_alphanumeric());
        if named(run) {
            out.insert(crate::lexical::folded(run));
        }
    };
    let joined = |c: char| c.is_alphanumeric() || "_:/.-".contains(c);
    for run in text.split(|c: char| !joined(c)) {
        // Most runs are words of prose, in lower case and joined by nothing:
        // none of them, nor a part of one, is named.
        if !run.contains(|c: char| c.is_uppercase() || "_/.".contains(c)) && !run.contains("::") {
            continue;
        }
        keep(run);
        run.split(|c: char| !(c.is_alphanumeric() || c == '_')).for_each(&mut keep);
        if let Some((line, column)) = run.trim_end_matches(|c: char| !c.is_alphanumeric()).rsplit_once(':') {
            if column.chars().all(|c| c.is_ascii_digit()) {
                keep(line);
            }
        }
    }
    out
}

/// Whether `run` is shaped as a name in code rather than a word of prose:
/// it joins words by `_`, `::`, `/` or `.`, or by a capital inside it
/// (ExecutableFileBusy), or is a capital word of four letters or more
/// (ETXTBSY). A hyphen alone joins prose ("six-session"), and a run with
/// fewer than two letters is a date, a time, an id or a version (v0.24.0),
/// which every day's tasks, or a release and what it ships, share.
fn named(run: &str) -> bool {
    let letters = run.chars().filter(|c| c.is_alphabetic()).count();
    if letters < 2 || run.chars().count() < SHORTEST || longest_part(run) < 2 {
        return false;
    }
    let joined = run.contains(['_', '/', '.']) || run.contains("::");
    let camel = run.chars().zip(run.chars().skip(1)).any(|(one, next)| one.is_lowercase() && next.is_uppercase());
    let capital = letters >= 4 && run.chars().all(|c| c.is_uppercase() || c.is_ascii_digit());
    joined || camel || capital
}

/// The longest run of letters and digits in `run`: "e.g" has none of two.
fn longest_part(run: &str) -> usize {
    run.split(|c: char| !c.is_alphanumeric()).map(|part| part.chars().count()).max().unwrap_or(0)
}

/// Each item's terms, by uid, beside a hash of the text they were read
/// from. An MCP server lives as long as its session, and each create would
/// otherwise read every item's text again. Measured 2026-10-12 over MCP on
/// a copy of this board, 1756 items, 40 creates a run, four runs each, on a
/// machine other sessions loaded: 38 to 58 ms a create without the check,
/// 68 to 135 with it reading every text, 38 to 52 with it keeping them.
static KEPT: Mutex<BTreeMap<String, Kept>> = Mutex::new(BTreeMap::new());

/// The hash of the text an item's terms were read from, and the terms.
type Kept = (u64, Arc<BTreeSet<String>>);

fn hashed(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// An open task `id` may repeat: what it shares of the new text's rare
/// terms.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Alike {
    pub id: u32,
    pub shared: Vec<String>,
}

/// The open tasks of `board` that hold enough of `text`'s rare terms, with
/// their notes, newest first; `left_out` are never named -- what the new
/// task already names as its blockers.
///
/// A term's rarity is counted over subjects, not items: a task with the
/// notes attached to it is one, and a loose note one of its own. A failure
/// discussed in five notes on its task is still one subject's, and a new
/// task on it still finds that task.
pub(crate) fn alike(board: &ItemMap, text: &str, left_out: &[u32]) -> Vec<Alike> {
    let wanted = terms(text);
    if wanted.is_empty() {
        return Vec::new();
    }
    let mut kept = KEPT.lock().unwrap_or_else(PoisonError::into_inner);
    let mut subjects: HashMap<&str, BTreeSet<&str>> = HashMap::new();
    for item in board.values().filter(|item| item.trashed.is_none()) {
        let Some(own) = item.uid.as_deref() else { continue };
        let subject = if item.is_task { own } else { item.attached_to.as_deref().unwrap_or(own) };
        let held = subjects.entry(subject).or_default();
        let read = hashed(&item.description);
        let found = match kept.get(own) {
            Some((from, found)) if *from == read => Arc::clone(found),
            _ => {
                let found = Arc::new(terms(&item.description));
                kept.insert(own.to_string(), (read, Arc::clone(&found)));
                found
            }
        };
        held.extend(found.iter().filter_map(|term| wanted.get(term)).map(String::as_str));
    }
    drop(kept);
    let mut holding: HashMap<&str, usize> = HashMap::new();
    for term in subjects.values().flatten() {
        *holding.entry(term).or_default() += 1;
    }
    let rare = RARE.max(subjects.len() / RARE_PER);
    let open = |item: &&Item| item.is_task && !matches!(State::of(item), Some(State::Done | State::Cancelled));
    let tasks: HashMap<&str, &Item> = board
        .values()
        .filter(|item| item.trashed.is_none() && !left_out.contains(&item.id))
        .filter(open)
        .filter_map(|item| Some((item.uid.as_deref()?, item)))
        .collect();
    let mut found: Vec<(&Item, Vec<String>)> = subjects
        .iter()
        .filter_map(|(subject, held)| {
            let task = tasks.get(subject)?;
            let mut shared: Vec<&str> = held.iter().copied().filter(|term| holding[term] <= rare).collect();
            // A term inside another counts with it: agpl in agpl-3.0, a file:line
            // in its file:line:column.
            let whole = shared.clone();
            shared.retain(|term| !whole.iter().any(|other| other != term && other.contains(term)));
            // The rarest first, and of those the longest: what the notice shows.
            shared.sort_by_key(|term| (holding[term], Reverse(term.chars().count()), *term));
            (shared.len() >= SHARED).then(|| (*task, shared.into_iter().map(str::to_string).collect()))
        })
        .collect();
    found.sort_by_key(|(task, _)| (Reverse(task.timestamp), Reverse(task.id)));
    found.into_iter().map(|(task, shared)| Alike { id: task.id, shared }).collect()
}

/// The notice for task `id`, if any open task may be the same.
pub(crate) fn told(id: u32, found: &[Alike]) -> Option<String> {
    if found.is_empty() {
        return None;
    }
    let named: Vec<String> = found
        .iter()
        .take(NAMED)
        .map(|alike| {
            let shown: Vec<&str> = alike.shared.iter().take(4).map(String::as_str).collect();
            format!("{} (shares {})", alike.id, shown.join(", "))
        })
        .collect();
    let more = found.len().saturating_sub(NAMED);
    let rest = if more > 0 { format!(", and {more} more") } else { String::new() };
    Some(format!(
        "{id} may be the same as open {}{rest}: read {} with context, and if {} is, cancel one in the other's favour, carrying over what it adds",
        named.join("; "),
        if found.len() == 1 { "it" } else { "them" },
        if found.len() == 1 { "it" } else { "one" }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(terms: &[&str]) -> BTreeSet<String> {
        terms.iter().map(|term| term.to_string()).collect()
    }

    #[test]
    fn a_term_is_a_name_in_code_and_never_a_word_of_prose() {
        let text = "the_doctor_names panicked at tests/cli.rs:1022:10 with io::ErrorKind::ExecutableFileBusy, \
            ETXTBSY (\"Text file busy\"), in a six-session run on 2026-10-09 at 03:21:37, task 1577, v0.42.0, e.g. RUN";
        let found = set(&[
            "the_doctor_names",
            "tests/cli.rs:1022:10",
            "tests/cli.rs:1022",
            "io::errorkind::executablefilebusy",
            "errorkind",
            "executablefilebusy",
            "etxtbsy",
        ]);
        assert_eq!(terms(text), found);
        assert_eq!(terms("Two stand-in tests failed at spawn in other sessions' runs, passing on rerun"), set(&[]));
        assert_eq!(terms("see src/ops.rs:12."), set(&["src/ops.rs:12", "src/ops.rs"]), "a run ending in :N is kept without it too");
    }

    fn task(id: u32, text: &str) -> Item {
        Item::new_task(id, text.to_string(), vec!["My Board".to_string()], 1)
    }

    fn note(id: u32, text: &str, on: &Item) -> Item {
        let mut note = Item::new_note(id, text.to_string(), vec!["My Board".to_string()]);
        note.attached_to = on.uid.clone();
        note
    }

    fn board(items: Vec<Item>) -> ItemMap {
        items.into_iter().map(|item| (item.id, item)).collect()
    }

    fn named(board: &ItemMap, text: &str) -> Vec<u32> {
        alike(board, text, &[]).into_iter().map(|alike| alike.id).collect()
    }

    const FAILURE: &str = "alpha_beta_gamma fails in src/delta.rs:12 with EpsilonError";

    #[test]
    fn an_open_task_holding_three_of_the_new_texts_rare_names_is_named() {
        let open = task(1, &format!("Fix it\n{FAILURE}"));
        assert_eq!(named(&board(vec![open.clone()]), &format!("Again\n{FAILURE}")), [1]);
        assert_eq!(named(&board(vec![open.clone()]), "Again\nalpha_beta_gamma fails in src/delta.rs:12"), Vec::<u32>::new(), "two are not enough");
        assert_eq!(named(&board(vec![open.clone()]), "Unrelated\nsrc/render.rs draws PreToolUse"), Vec::<u32>::new());

        let mut done = open.clone();
        done.is_complete = Some(true);
        assert_eq!(named(&board(vec![done]), &format!("Again\n{FAILURE}")), Vec::<u32>::new(), "done work is not repeated");
        // A task in the trash is not named, though trash leaves its notes out
        // of it, still holding the names.
        let mut trashed = task(5, "Fix it");
        trashed.trashed = Some(1);
        let left = note(6, &format!("Seen again: {FAILURE}"), &trashed);
        assert_eq!(named(&board(vec![trashed, left]), &format!("Again\n{FAILURE}")), Vec::<u32>::new(), "a task in the trash is not named");
        assert_eq!(alike(&board(vec![open.clone()]), &format!("Again\n{FAILURE}"), &[1]), [], "its blocker is left out");

        // Its notes are the task's: two of the names here, one there.
        let bare = task(2, "Fix it\nalpha_beta_gamma fails");
        let on = note(3, "Seen at src/delta.rs:12, with EpsilonError", &bare);
        assert_eq!(named(&board(vec![bare, on]), &format!("Again\n{FAILURE}")), [2]);

        // Newest first.
        let mut older = task(4, &format!("Older\n{FAILURE}"));
        older.timestamp -= 1000;
        assert_eq!(named(&board(vec![older, open]), &format!("Again\n{FAILURE}")), [1, 4]);
    }

    #[test]
    fn an_item_whose_text_changed_is_read_anew() {
        let mut open = task(1, "Fix it\nNothing named yet");
        assert_eq!(named(&board(vec![open.clone()]), &format!("Again\n{FAILURE}")), Vec::<u32>::new());
        open.description = format!("Fix it\n{FAILURE}");
        assert_eq!(named(&board(vec![open.clone()]), &format!("Again\n{FAILURE}")), [1], "read anew, not kept from the text before");
        open.description = "Fix it\nNothing named now".to_string();
        assert_eq!(named(&board(vec![open]), &format!("Again\n{FAILURE}")), Vec::<u32>::new());
    }

    #[test]
    fn a_name_many_subjects_hold_is_not_rare() {
        let open = task(1, &format!("Fix it\n{FAILURE}"));
        // EpsilonError in three more subjects, four in all: no longer rare,
        // and the other two names are not enough.
        let mut items = vec![open];
        for id in 2..=4 {
            let mut done = task(id, "Earlier\nEpsilonError");
            done.is_complete = Some(true);
            items.push(done);
        }
        assert_eq!(named(&board(items.clone()), &format!("Again\n{FAILURE}")), Vec::<u32>::new());
        // Three notes on one task are one subject: it stays rare.
        let mut items = vec![items[0].clone(), items[1].clone()];
        for id in 5..=7 {
            items.push(note(id, "Again\nEpsilonError", &items[1].clone()));
        }
        assert_eq!(named(&board(items), &format!("Again\n{FAILURE}")), [1]);
    }

    #[test]
    fn the_notice_names_each_task_with_what_it_shares() {
        let found = [Alike { id: 4, shared: vec!["a_b".into(), "c::d".into(), "EFG_H".into(), "i/j".into(), "k.l".into()] }];
        assert_eq!(
            told(9, &found).unwrap(),
            "9 may be the same as open 4 (shares a_b, c::d, EFG_H, i/j): read it with context, and if it is, cancel one in the other's favour, carrying over what it adds"
        );
        let many: Vec<Alike> = (1..=5).map(|id| Alike { id, shared: vec!["x_y".into()] }).collect();
        assert!(told(9, &many).unwrap().starts_with("9 may be the same as open 1 (shares x_y); 2 (shares x_y); 3 (shares x_y), and 2 more: read them with context, and if one is, cancel"));
        assert_eq!(told(9, &[]), None);
    }
}
