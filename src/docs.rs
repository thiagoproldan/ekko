//! `ekko docs` (task 915): the project's documentation, written from its
//! board as markdown -- by code, with no model, so it costs no tokens and
//! takes milliseconds however large the board grows (note 995). The layout
//! is the user's choice (question 997): a page per kind of note, the history
//! of the tasks, and a page per task, because a board's decisions fit one
//! page where the record of its tasks, with their notes, handoffs and
//! answers, runs to hundreds of thousands of characters (note 996).
//!
//! Every file begins with a mark, and only a file that has it is ever
//! overwritten or removed: a project's docs/ may hold pages written by hand.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::io::{self, BufRead as _};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::commits::Commit;
use crate::item::{Item, Knowledge, State};
use crate::storage::ItemMap;

/// How every file ekko docs writes begins, and so how it knows its own.
const MARK: &str = "<!-- Written by ekko docs";

/// The most of a first line a heading holds, in characters: a longer line is
/// cut at a word under it, and the note's whole text follows.
const TITLE: usize = 120;

/// What a run wrote, for the reply.
#[derive(Debug, Clone, Serialize)]
pub struct Written {
    pub folder: PathBuf,
    /// Files written this time; those already as they would be are left
    /// alone, so a board that did not move rewrites nothing.
    pub written: usize,
    pub unchanged: usize,
    /// Task pages an earlier run wrote for tasks no longer on the board.
    pub removed: usize,
    pub decisions: Count,
    pub gotchas: Count,
    pub procedures: Count,
    pub tasks: Tasks,
    /// Notes on no task, on the page of other notes.
    pub loose: usize,
}

/// The notes of one kind: those in force, and those a later one replaced.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct Count {
    pub current: usize,
    pub replaced: usize,
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct Tasks {
    pub done: usize,
    pub cancelled: usize,
    pub open: usize,
}

#[derive(Debug)]
pub enum Error {
    /// Files in the way that ekko docs did not write: nothing was written.
    Foreign(Vec<PathBuf>),
    Io(PathBuf, io::Error),
}

/// Writes the docs of the board `items` into `folder`: `project` names the
/// project, `None` for the default board; `memory` is the project's page,
/// which opens the index; `commits` are those naming each task.
pub fn write(
    items: &ItemMap,
    commits: &HashMap<u32, Vec<Commit>>,
    folder: &Path,
    project: Option<&str>,
    memory: Option<&str>,
) -> Result<Written, Error> {
    let board = Board::new(items, commits);
    let source = project.map_or("the default board".to_string(), |name| format!("the board of {name}"));
    let (decisions, decision_count) = board.kind(Page::Decisions, "Decisions", "What was settled, and why:");
    let (gotchas, gotcha_count) = board.kind(Page::Gotchas, "Gotchas", "Traps, and how to avoid them:");
    let (procedures, procedure_count) = board.kind(Page::Procedures, "Procedures", "Steps that work:");
    let (notes, loose) = board.notes();
    let tasks = board.tally();
    let mut written = Written {
        folder: folder.to_path_buf(),
        written: 0,
        unchanged: 0,
        removed: 0,
        decisions: decision_count,
        gotchas: gotcha_count,
        procedures: procedure_count,
        tasks,
        loose,
    };
    let as_of = items.values().map(|item| item.updated_at.unwrap_or(item.timestamp)).max().map(day).unwrap_or_default();
    let mut pages = vec![
        (Page::Index, index(project, &source, memory, &written, &as_of)),
        (Page::Decisions, decisions),
        (Page::Gotchas, gotchas),
        (Page::Procedures, procedures),
        (Page::History, board.history(&tasks)),
        (Page::Notes, notes),
    ];
    pages.extend(items.values().filter(|item| item.is_task).map(|item| (Page::Task(item.id), board.task(item.id))));

    let head = format!("{MARK} from {source}: change the board and run it again, since edits here are overwritten. -->\n\n");
    let files: Vec<(PathBuf, String)> =
        pages.into_iter().map(|(page, body)| (folder.join(page.path()), format!("{head}{}\n", body.trim_end()))).collect();
    let foreign: Vec<PathBuf> =
        files.iter().filter(|(path, _)| path.exists() && !ours(path)).map(|(path, _)| path.clone()).collect();
    if !foreign.is_empty() {
        return Err(Error::Foreign(foreign));
    }

    let pages_dir = folder.join("tasks");
    fs::create_dir_all(&pages_dir).map_err(|error| Error::Io(pages_dir.clone(), error))?;
    for (path, text) in &files {
        if fs::read_to_string(path).is_ok_and(|old| old == *text) {
            written.unchanged += 1;
            continue;
        }
        fs::write(path, text).map_err(|error| Error::Io(path.clone(), error))?;
        written.written += 1;
    }
    let kept: HashSet<&PathBuf> = files.iter().map(|(path, _)| path).collect();
    let entries = fs::read_dir(&pages_dir).map_err(|error| Error::Io(pages_dir.clone(), error))?;
    let mut stale: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "md") && !kept.contains(path) && ours(path))
        .collect();
    stale.sort();
    for path in stale {
        fs::remove_file(&path).map_err(|error| Error::Io(path.clone(), error))?;
        written.removed += 1;
    }
    Ok(written)
}

/// Whether ekko docs wrote the file at `path`: its first line is the mark.
fn ours(path: &Path) -> bool {
    fs::File::open(path)
        .ok()
        .and_then(|file| io::BufReader::new(file).lines().next()?.ok())
        .is_some_and(|line| line.starts_with(MARK))
}

/// A page of the docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Index,
    Decisions,
    Gotchas,
    Procedures,
    History,
    Notes,
    Task(u32),
}

impl Page {
    /// Its path in the folder.
    fn path(self) -> String {
        match self {
            Page::Index => "index.md".to_string(),
            Page::Decisions => "decisions.md".to_string(),
            Page::Gotchas => "gotchas.md".to_string(),
            Page::Procedures => "procedures.md".to_string(),
            Page::History => "history.md".to_string(),
            Page::Notes => "notes.md".to_string(),
            Page::Task(id) => format!("tasks/{id}.md"),
        }
    }

    /// A link from this page to `to`, at the item `at` on it when given.
    fn link(self, to: Page, at: Option<u32>) -> String {
        let fragment = at.map(|id| format!("#{id}")).unwrap_or_default();
        if self == to && !fragment.is_empty() {
            return fragment;
        }
        let file = match (self, to) {
            (Page::Task(_), Page::Task(id)) => format!("{id}.md"),
            (Page::Task(_), other) => format!("../{}", other.path()),
            (_, other) => other.path(),
        };
        format!("{file}{fragment}")
    }

    /// The page of the notes of `kind`; a kind a later version added has
    /// none, and its notes stay with their task.
    fn of_kind(kind: Knowledge) -> Option<Page> {
        match kind {
            Knowledge::Decision => Some(Page::Decisions),
            Knowledge::Gotcha => Some(Page::Gotchas),
            Knowledge::Procedure => Some(Page::Procedures),
            Knowledge::Unknown(_) => None,
        }
    }
}

/// The board as the docs read it: where each item is written, and the links
/// between items, resolved once.
struct Board<'a> {
    items: &'a ItemMap,
    /// A task on its page; a note at its anchor on the page of its kind, on
    /// its task's, or on the page of other notes.
    homes: HashMap<u32, Page>,
    /// The task each note explains, and each task's notes, oldest first.
    task_of: HashMap<u32, u32>,
    notes_of: HashMap<u32, Vec<u32>>,
    /// The note each one replaces, and the newest that replaces each.
    replaces: HashMap<u32, u32>,
    replaced_by: HashMap<u32, u32>,
    blocked_by: HashMap<u32, Vec<u32>>,
    blocks: HashMap<u32, Vec<u32>>,
    commits: &'a HashMap<u32, Vec<Commit>>,
}

impl<'a> Board<'a> {
    fn new(items: &'a ItemMap, commits: &'a HashMap<u32, Vec<Commit>>) -> Self {
        let index = crate::ekko::uid_index(items);
        let resolve = |uid: &str| index.get(uid).copied();
        let mut board = Board {
            items,
            homes: HashMap::new(),
            task_of: HashMap::new(),
            notes_of: HashMap::new(),
            replaces: HashMap::new(),
            replaced_by: HashMap::new(),
            blocked_by: HashMap::new(),
            blocks: HashMap::new(),
            commits,
        };
        for (&id, item) in items {
            for blocker in item.blocked_by.iter().flatten().filter_map(|uid| resolve(uid.as_str())) {
                board.blocked_by.entry(id).or_default().push(blocker);
                board.blocks.entry(blocker).or_default().push(id);
            }
            if item.is_task {
                board.homes.insert(id, Page::Task(id));
                continue;
            }
            let task = item
                .attached_to
                .as_deref()
                .and_then(resolve)
                .filter(|task| items.get(task).is_some_and(|task| task.is_task));
            if let Some(task) = task {
                board.task_of.insert(id, task);
                board.notes_of.entry(task).or_default().push(id);
            }
            if let Some(old) = item.supersedes.as_deref().and_then(resolve) {
                board.replaces.insert(id, old);
                board.replaced_by.insert(old, id);
            }
            let home = item.knowledge.and_then(Page::of_kind).or(task.map(Page::Task)).unwrap_or(Page::Notes);
            board.homes.insert(id, home);
        }
        board
    }

    fn tally(&self) -> Tasks {
        let mut tasks = Tasks::default();
        for item in self.items.values().filter(|item| item.is_task) {
            match State::of(item) {
                Some(State::Done) => tasks.done += 1,
                Some(State::Cancelled) => tasks.cancelled += 1,
                _ => tasks.open += 1,
            }
        }
        tasks
    }

    /// The page of the notes of one kind: those in force, newest first, each
    /// under a list of them all, and at the end those later ones replaced.
    fn kind(&self, page: Page, title: &str, about: &str) -> (String, Count) {
        let (replaced, current): (Vec<u32>, Vec<u32>) = self
            .items
            .keys()
            .rev()
            .copied()
            .filter(|id| self.homes.get(id) == Some(&page))
            .partition(|id| self.replaced_by.contains_key(id));
        let mut out = format!("# {title}\n\n{about} {} in force, newest first", current.len());
        if !replaced.is_empty() {
            let _ = write!(out, "; the {} that later ones replaced are at the end", replaced.len());
        }
        out.push_str(".\n\n");
        for id in &current {
            let _ = writeln!(out, "- [{id}](#{id}) {}", self.linked(&inline(&self.title(*id)), page));
        }
        if !current.is_empty() {
            out.push('\n');
        }
        for id in &current {
            self.entry(&mut out, "##", *id, page);
        }
        if !replaced.is_empty() {
            out.push_str("## Replaced\n\n");
            for id in &replaced {
                self.entry(&mut out, "###", *id, page);
            }
        }
        (out, Count { current: current.len(), replaced: replaced.len() })
    }

    /// The page of the notes on no task, newest first.
    fn notes(&self) -> (String, usize) {
        let ids: Vec<u32> =
            self.items.keys().rev().copied().filter(|id| self.homes.get(id) == Some(&Page::Notes)).collect();
        let mut out = "# Other notes\n\nNotes on no task, newest first: what was written about the project as a whole, and the questions asked about it.\n\n".to_string();
        if ids.is_empty() {
            out.push_str("None yet.\n");
        }
        for id in &ids {
            self.entry(&mut out, "##", *id, Page::Notes);
        }
        (out, ids.len())
    }

    /// Every task, newest first: the open ones, then the rest by the month
    /// each was created in, each line linking to the task's page.
    fn history(&self, tasks: &Tasks) -> String {
        let mut out = format!(
            "# History\n\nEvery task on the board, newest first: {} done, {} cancelled, {} open. Each has a page of its own, with its notes, handoffs, answers and commits.\n\n",
            tasks.done, tasks.cancelled, tasks.open
        );
        let ids: Vec<u32> = self.items.values().rev().filter(|item| item.is_task).map(|item| item.id).collect();
        let open: Vec<u32> = ids.iter().copied().filter(|id| !closed(&self.items[id])).collect();
        if !open.is_empty() {
            out.push_str("## Open\n\n");
            for id in &open {
                self.history_line(&mut out, *id);
            }
            out.push('\n');
        }
        let mut months: BTreeMap<String, Vec<u32>> = BTreeMap::new();
        for id in ids.iter().copied().filter(|id| closed(&self.items[id])) {
            months.entry(month(self.items[&id].timestamp)).or_default().push(id);
        }
        for (month, ids) in months.iter().rev() {
            let _ = write!(out, "## {month}\n\n");
            for id in ids {
                self.history_line(&mut out, *id);
            }
            out.push('\n');
        }
        out
    }

    fn history_line(&self, out: &mut String, id: u32) {
        let commits = match self.commits.get(&id).map_or(0, Vec::len) {
            0 => String::new(),
            1 => " · 1 commit".to_string(),
            n => format!(" · {n} commits"),
        };
        let _ = writeln!(out, "- [{id}](tasks/{id}.md) {} · {}{commits}", self.linked(&inline(&self.title(id)), Page::History), state(&self.items[&id]));
    }

    /// A task's page: what it is, what it waited on and held up, the commits
    /// that name it, the lasting notes it gave, and every other note on it,
    /// oldest first.
    fn task(&self, id: u32) -> String {
        let item = &self.items[&id];
        let on = Page::Task(id);
        let (heading, body) = titled(&item.description);
        let mut out = format!("# {id}. {}\n\n", self.linked(&inline(&heading), on));
        let mut about = vec!["Task".to_string(), state(item).to_string()];
        match item.priority {
            Some(3..) => about.push("high priority".to_string()),
            Some(2) => about.push("medium priority".to_string()),
            _ => {}
        }
        if let Some(phase) = &item.phase {
            about.push(format!("phase {}", inline(phase)));
        }
        if let Some(with) = &item.with {
            about.push(format!("with {}", inline(with)));
        }
        about.push(format!("created {}", day(item.timestamp)));
        about.push(format!("[history]({})", on.link(Page::History, None)));
        let _ = write!(out, "*{}*\n\n", about.join(" · "));
        if !body.trim().is_empty() {
            let _ = write!(out, "{}\n\n", self.prose(&body, on));
        }
        for (word, ids) in [("Blocked by", self.blocked_by.get(&id)), ("Blocks", self.blocks.get(&id))] {
            let Some(ids) = ids else { continue };
            let links: Vec<String> = ids
                .iter()
                .map(|other| format!("[{other}]({}) ({})", on.link(Page::Task(*other), None), state(&self.items[other])))
                .collect();
            let _ = write!(out, "{word} {}.\n\n", links.join(", "));
        }
        if let Some(commits) = self.commits.get(&id).filter(|commits| !commits.is_empty()) {
            out.push_str("## Commits\n\n");
            for commit in commits {
                let elsewhere = if commit.landed { "" } else { " (not on the branch checked out)" };
                let _ = writeln!(out, "- `{}` {} {}{elsewhere}", commit.sha, commit.date, inline(&commit.subject));
            }
            out.push('\n');
        }
        let notes = self.notes_of.get(&id).map(Vec::as_slice).unwrap_or_default();
        let (lasting, own): (Vec<u32>, Vec<u32>) = notes.iter().copied().partition(|note| self.homes.get(note) != Some(&on));
        if !lasting.is_empty() {
            out.push_str("## Decisions, gotchas and procedures\n\n");
            for note in &lasting {
                let replaced = if self.replaced_by.contains_key(note) { " (replaced)" } else { "" };
                let _ = writeln!(
                    out,
                    "- {} [{note}]({}){replaced}: {}",
                    self.label(*note),
                    on.link(self.homes[note], Some(*note)),
                    self.linked(&inline(&self.title(*note)), on)
                );
            }
            out.push('\n');
        }
        if !own.is_empty() {
            out.push_str("## Notes\n\n");
            for note in &own {
                self.entry(&mut out, "###", *note, on);
            }
        }
        out
    }

    /// One note at its anchor: its first line as the heading, the rest of
    /// its text, a question's answer, and a line saying what it is.
    fn entry(&self, out: &mut String, level: &str, id: u32, on: Page) {
        let item = &self.items[&id];
        let (heading, body) = titled(&item.description);
        let _ = write!(out, "{level} <a id=\"{id}\"></a>{id}. {}\n\n", self.linked(&inline(&heading), on));
        if !body.trim().is_empty() {
            let _ = write!(out, "{}\n\n", self.prose(&body, on));
        }
        if let Some(question) = &item.question {
            match &question.answer {
                Some(answer) if !answer.text.trim().contains('\n') => {
                    let _ = write!(out, "**Answer**, {}: {}\n\n", day(answer.at), self.prose(&answer.text, on));
                }
                Some(answer) => {
                    let _ = write!(out, "**Answer**, {}:\n\n{}\n\n", day(answer.at), self.prose(&answer.text, on));
                }
                None => out.push_str("*Not answered.*\n\n"),
            }
        }
        let _ = write!(out, "*{}*\n\n", self.about(id, on).join(" · "));
    }

    /// What a note is, when it was written, and the notes and the task it
    /// links to.
    fn about(&self, id: u32, on: Page) -> Vec<String> {
        let mut about = vec![self.label(id), day(self.items[&id].timestamp)];
        if let Some(&task) = self.task_of.get(&id).filter(|_| !matches!(on, Page::Task(_))) {
            about.push(format!("on task [{task}]({})", on.link(Page::Task(task), None)));
        }
        if let Some(&old) = self.replaces.get(&id) {
            about.push(format!("replaces [{old}]({})", on.link(self.homes[&old], Some(old))));
        }
        if let Some(&new) = self.replaced_by.get(&id) {
            about.push(format!("replaced by [{new}]({})", on.link(self.homes[&new], Some(new))));
        }
        about
    }

    /// What a note is, in a word for a reader. A note without a kind is
    /// replaced only as a handoff a later one demoted, so one that replaces
    /// another, or was replaced, was a handoff.
    fn label(&self, id: u32) -> String {
        let item = &self.items[&id];
        if let Some(kind) = item.knowledge {
            return capitalized(kind.word());
        }
        let word = if item.handoff {
            "Handoff"
        } else if item.question.is_some() {
            "Question"
        } else if self.replaced_by.contains_key(&id) {
            "Earlier handoff"
        } else if self.replaces.contains_key(&id) {
            "Handoff"
        } else {
            "Note"
        };
        word.to_string()
    }

    /// An item's first line, for a list, cut at a word when it runs long.
    fn title(&self, id: u32) -> String {
        shortened(self.items[&id].description.trim().lines().next().unwrap_or("").trim())
    }

    /// A note's text as markdown: each line a paragraph of its own, as the
    /// board keeps them, list items kept together, and each line `escaped`
    /// and `linked`. A block of code a note fences with ``` stays as written,
    /// and one left open is closed, so it does not run on over the page.
    fn prose(&self, text: &str, on: Page) -> String {
        let mut out = String::new();
        let mut listing = false;
        let mut fence: Option<String> = None;
        for line in text.lines().map(str::trim_end) {
            let start = line.trim_start();
            if let Some(open) = &fence {
                out.push('\n');
                out.push_str(line);
                if start.starts_with(open.as_str()) && start.chars().all(|c| open.starts_with(c)) {
                    fence = None;
                    listing = false;
                }
                continue;
            }
            if start.starts_with("```") || start.starts_with("~~~") {
                let mark = start.chars().next().unwrap_or('`');
                fence = Some(start.chars().take_while(|c| *c == mark).collect());
                if !out.is_empty() {
                    out.push_str("\n\n");
                }
                out.push_str(line);
                continue;
            }
            if line.trim().is_empty() {
                listing = false;
                continue;
            }
            let item = list_item(line);
            if !out.is_empty() {
                out.push_str(if item && listing { "\n" } else { "\n\n" });
            }
            out.push_str(&self.linked(&escaped(line), on));
            listing = item;
        }
        if let Some(open) = fence {
            out.push('\n');
            out.push_str(&open);
        }
        out
    }

    /// `line` with every item it cites linked where that item is written:
    /// "task 983", "note 990", and lists after a plural, "decisions 161, 163
    /// and 164". Not inside code, and not a number naming nothing here.
    fn linked(&self, line: &str, on: Page) -> String {
        let mut out = String::with_capacity(line.len());
        for (at, segment) in line.split('`').enumerate() {
            if at > 0 {
                out.push('`');
            }
            if at % 2 == 1 {
                out.push_str(segment);
            } else {
                self.link_text(&mut out, segment, on);
            }
        }
        out
    }

    fn link_text(&self, out: &mut String, text: &str, on: Page) {
        let bytes = text.as_bytes();
        let mut at = 0;
        while at < text.len() {
            let starts_word = bytes[at].is_ascii_alphabetic() && (at == 0 || !word_byte(bytes[at - 1]));
            if let Some((word, plural)) = starts_word.then(|| cited_word(&text[at..])).flatten() {
                let first = at + word + 1;
                let numbers = if text[at + word..].starts_with(' ') { numbers_at(text, first, plural) } else { Vec::new() };
                if !numbers.is_empty() {
                    out.push_str(&text[at..first]);
                    let mut copied = first;
                    for (start, end) in numbers {
                        out.push_str(&text[copied..start]);
                        let number = &text[start..end];
                        match number.parse::<u32>().ok().and_then(|id| Some((id, *self.homes.get(&id)?))) {
                            Some((id, page)) => {
                                let anchor = (!self.items[&id].is_task).then_some(id);
                                let _ = write!(out, "[{number}]({})", on.link(page, anchor));
                            }
                            None => out.push_str(number),
                        }
                        copied = end;
                    }
                    at = copied;
                    continue;
                }
            }
            let next = text[at..].chars().next().map_or(1, char::len_utf8);
            out.push_str(&text[at..at + next]);
            at += next;
        }
    }
}

/// Part of a word, for where one starts: a letter, a digit, an underscore,
/// or any byte of a character past ASCII.
fn word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte >= 0x80
}

/// The length of the word at the start of `text` that names a kind of item,
/// in any case, and whether it is plural: "task", "Notes".
fn cited_word(text: &str) -> Option<(usize, bool)> {
    const WORDS: [&str; 7] = ["task", "note", "decision", "gotcha", "procedure", "question", "handoff"];
    for word in WORDS {
        if !text.get(..word.len()).is_some_and(|start| start.eq_ignore_ascii_case(word)) {
            continue;
        }
        let plural = text[word.len()..].starts_with(['s', 'S']);
        let length = word.len() + usize::from(plural);
        if text[length..].chars().next().is_some_and(char::is_alphanumeric) {
            continue;
        }
        return Some((length, plural));
    }
    None
}

/// The numbers a cited word names from `start`, as byte ranges: one, or
/// after a plural a list of them, "990, 992 and 994". A number running into
/// a letter, as a uid's start does, is none.
fn numbers_at(text: &str, start: usize, plural: bool) -> Vec<(usize, usize)> {
    const GAPS: [&str; 6] = [", and ", ", ", " and ", " or ", " e ", " ou "];
    let mut found = Vec::new();
    let mut at = start;
    loop {
        let digits = text[at..].bytes().take_while(u8::is_ascii_digit).count();
        let end = at + digits;
        if digits == 0 || digits > 9 || text[end..].chars().next().is_some_and(char::is_alphanumeric) {
            break;
        }
        found.push((at, end));
        let gap = GAPS.iter().find(|gap| text[end..].starts_with(**gap));
        match gap {
            Some(gap) if plural && text[end + gap.len()..].starts_with(|c: char| c.is_ascii_digit()) => at = end + gap.len(),
            _ => break,
        }
    }
    found
}

/// A line of a note, which is plain text, as markdown that shows it as it
/// is: a heading, a quote or a rule at its start escaped, a list's marker
/// kept, and the rest `escape_inline`'s.
fn escaped(line: &str) -> String {
    let body = line.trim_start();
    let mut out = String::from(&line[..line.len() - body.len()]);
    if body.starts_with(['#', '>']) || (rule(body) && body.starts_with(['-', '='])) {
        out.push('\\');
    }
    match body.strip_prefix("* ") {
        Some(rest) => {
            out.push_str("* ");
            out.push_str(&escape_inline(rest));
        }
        None => out.push_str(&escape_inline(body)),
    }
    out
}

/// Plain text that markdown would read as markup inside a line, escaped
/// outside code: `<`, which opens a tag; `*`, `~` and `\`; and `_` where a
/// word does not hold it on both sides. Measured on ekko's own board, with
/// GitHub's renderer: '~417k ... ~' came out struck, '0.1*C + 5*' and
/// '__pycache__' emphasised.
fn escape_inline(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut code = false;
    for (at, &c) in chars.iter().enumerate() {
        if c == '`' {
            code = !code;
            out.push(c);
            continue;
        }
        if code {
            out.push(c);
            continue;
        }
        let inside_a_word = |at: usize| {
            at > 0 && chars[at - 1].is_alphanumeric() && chars.get(at + 1).is_some_and(|next| next.is_alphanumeric())
        };
        match c {
            '<' => out.push_str("&lt;"),
            '*' | '~' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            '_' if !inside_a_word(at) => out.push_str("\\_"),
            _ => out.push(c),
        }
    }
    out
}

/// A line markdown reads as a rule, or as the underline of a heading: three
/// or more of one of `-`, `*`, `_` or `=`, spaces aside.
fn rule(line: &str) -> bool {
    let marks: Vec<char> = line.chars().filter(|c| !c.is_whitespace()).collect();
    marks.len() >= 3 && "-*_=".contains(marks[0]) && marks.iter().all(|c| *c == marks[0])
}

/// Whether a line is an item of a list: `- `, `* `, `+ `, or a number with
/// `. ` or `) `.
fn list_item(line: &str) -> bool {
    let line = line.trim_start();
    if line.starts_with("- ") || line.starts_with("* ") || line.starts_with("+ ") {
        return true;
    }
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    (1..=9).contains(&digits) && (line[digits..].starts_with(". ") || line[digits..].starts_with(") "))
}

/// Text inside a heading or a list's line.
fn inline(text: &str) -> String {
    escape_inline(text)
}

/// A heading for `description` and the text under it: the first line and
/// the rest, or, for a first line too long to head anything, its start cut
/// at a word and the whole text.
fn titled(description: &str) -> (String, String) {
    let text = description.trim();
    let (first, rest) = text.split_once('\n').unwrap_or((text, ""));
    let first = first.trim();
    if first.chars().count() <= TITLE {
        (first.to_string(), rest.to_string())
    } else {
        (shortened(first), text.to_string())
    }
}

/// `line` whole when it fits a title, or else cut at the last word that fits.
fn shortened(line: &str) -> String {
    if line.chars().count() <= TITLE {
        return line.to_string();
    }
    let end = line.char_indices().nth(TITLE).map_or(line.len(), |(at, _)| at);
    let head = &line[..end];
    let cut = match head.rfind(' ') {
        Some(at) if at > 0 => &head[..at],
        _ => head,
    };
    format!("{}…", cut.trim_end_matches([',', ';', ':', ' ']))
}

fn closed(item: &Item) -> bool {
    matches!(State::of(item), Some(State::Done | State::Cancelled))
}

/// A task's state, in the words the board uses.
fn state(item: &Item) -> &'static str {
    match State::of(item) {
        Some(State::Done) => "done",
        Some(State::Cancelled) => "cancelled",
        Some(State::Progress) => "in progress",
        Some(State::Paused) => "paused",
        Some(State::Waiting) => "waiting",
        Some(State::Pending) | None => "pending",
    }
}

fn capitalized(word: &str) -> String {
    let mut chars = word.chars();
    chars.next().map_or(String::new(), |first| first.to_uppercase().chain(chars).collect())
}

/// The local date of an instant in milliseconds.
fn day(ms: i64) -> String {
    local(ms, "%Y-%m-%d")
}

fn month(ms: i64) -> String {
    local(ms, "%Y-%m")
}

fn local(ms: i64, format: &str) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|at| at.with_timezone(&chrono::Local).format(format).to_string())
        .unwrap_or_default()
}

/// The index: the project's page, which the user keeps, then what the docs
/// hold.
fn index(project: Option<&str>, source: &str, memory: Option<&str>, written: &Written, as_of: &str) -> String {
    let name = project.unwrap_or("The default board");
    let mut out = String::new();
    match memory.map(str::trim).filter(|page| !page.is_empty()) {
        Some(page) if page.starts_with("# ") => out.push_str(page),
        Some(page) => {
            let _ = write!(out, "# {name}\n\n{page}");
        }
        None => {
            let _ = write!(
                out,
                "# {name}\n\nThis board has no page of its own yet: the user writes one as .ekko/memory.md, and ekko docs puts it here."
            );
        }
    }
    let kind = |count: Count| match count.replaced {
        0 => format!("{} in force.", count.current),
        replaced => format!("{} in force, {replaced} replaced.", count.current),
    };
    let tasks = written.tasks;
    let _ = write!(
        out,
        "\n\n## Documentation\n\n\
         - [Decisions](decisions.md): what was settled, and why. {}\n\
         - [Gotchas](gotchas.md): traps, and how to avoid them. {}\n\
         - [Procedures](procedures.md): steps that work. {}\n\
         - [History](history.md): every task, newest first, each on a page of its own. {} done, {} cancelled, {} open.\n\
         - [Other notes](notes.md): notes on no task. {}.\n\n\
         *Written by ekko docs from {source}, as of {as_of}.*\n",
        kind(written.decisions),
        kind(written.gotchas),
        kind(written.procedures),
        tasks.done,
        tasks.cancelled,
        tasks.open,
        written.loose,
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A board from items written as storage holds them.
    fn board_of(items: serde_json::Value) -> ItemMap {
        let items: Vec<Item> = serde_json::from_value(items).unwrap();
        items.into_iter().map(|item| (item.id, item)).collect()
    }

    fn task(id: u32, text: &str) -> serde_json::Value {
        serde_json::json!({
            "_id": id, "_date": "Mon Sep 28 2026", "_timestamp": 1_790_000_000_000_i64 + i64::from(id),
            "description": text, "isStarred": false, "boards": ["My Board"], "_isTask": true,
            "isComplete": true, "inProgress": false, "priority": 1, "uid": format!("18d0000000000{id:03}-1"),
        })
    }

    fn note(id: u32, text: &str, extra: serde_json::Value) -> serde_json::Value {
        let mut note = serde_json::json!({
            "_id": id, "_date": "Mon Sep 28 2026", "_timestamp": 1_790_000_000_000_i64 + i64::from(id),
            "description": text, "isStarred": false, "boards": ["My Board"], "_isTask": false,
            "uid": format!("18d0000000000{id:03}-1"),
        });
        note.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        note
    }

    fn uid(id: u32) -> String {
        format!("18d0000000000{id:03}-1")
    }

    #[test]
    fn a_cited_item_links_to_where_it_is_written_and_nothing_else_does() {
        let items = board_of(serde_json::json!([
            task(1, "Ship it"),
            task(2, "And this"),
            note(3, "Why", serde_json::json!({"knowledge": "decision", "attachedTo": uid(1)})),
            note(4, "A note on 1", serde_json::json!({"attachedTo": uid(1)})),
        ]));
        let commits = HashMap::new();
        let board = Board::new(&items, &commits);
        let index = Page::Index;
        assert_eq!(board.linked("see task 2.", index), "see task [2](tasks/2.md).");
        assert_eq!(board.linked("see task 2.", Page::Task(1)), "see task [2](2.md).");
        assert_eq!(board.linked("Decision 3 holds", Page::Task(1)), "Decision [3](../decisions.md#3) holds");
        assert_eq!(board.linked("note 4", Page::Task(1)), "note [4](#4)");
        assert_eq!(board.linked("note 4", Page::Decisions), "note [4](tasks/1.md#4)");
        assert_eq!(board.linked("tasks 1, 2 and 9", index), "tasks [1](tasks/1.md), [2](tasks/2.md) and 9");
        assert_eq!(board.linked("task 1, 2 runs", index), "task [1](tasks/1.md), 2 runs", "a list follows a plural only");
        assert_eq!(board.linked("`ekko --context` on task 1, `task 2`", index), "`ekko --context` on task [1](tasks/1.md), `task 2`");
        for untouched in ["task 18d0000000000001-1", "subtask 1", "tasks1", "the task at 2", "note 999"] {
            assert_eq!(board.linked(untouched, index), untouched);
        }
    }

    #[test]
    fn markup_a_note_did_not_mean_is_escaped_and_its_lines_stay_lines() {
        assert_eq!(escaped("# not a heading"), "\\# not a heading");
        assert_eq!(escaped("> not a quote"), "\\> not a quote");
        assert_eq!(escaped("---"), "\\---");
        assert_eq!(escaped("context <id> and `a <b>`"), "context &lt;id> and `a <b>`");
        assert_eq!(escaped("- a list stays a list"), "- a list stays a list");
        assert_eq!(escaped("* so does this one, *starred*"), "* so does this one, \\*starred\\*");
        assert_eq!(escaped("~417k, then ~450k~"), "\\~417k, then \\~450k\\~");
        assert_eq!(escaped("0.1*C + 5*d in `a*b`"), "0.1\\*C + 5\\*d in `a*b`");
        assert_eq!(escaped("__pycache__ and CTX_STATE"), "\\_\\_pycache\\_\\_ and CTX_STATE");
        assert_eq!(escaped(r"(\S+)"), r"(\\S+)");
        assert_eq!(escaped("***"), "\\*\\*\\*");
        let items = board_of(serde_json::json!([task(1, "Ship it")]));
        let commits = HashMap::new();
        let board = Board::new(&items, &commits);
        assert_eq!(board.prose("one\ntwo\n- a\n- b\n1) c\nthree", Page::Index), "one\n\ntwo\n\n- a\n- b\n1) c\n\nthree");
        assert_eq!(
            board.prose("Run:\n   ```\n   exec 3>&\"${SRV[1]}\" 4<&x *a*\n\n   ```\nthen task 1", Page::Index),
            "Run:\n\n   ```\n   exec 3>&\"${SRV[1]}\" 4<&x *a*\n\n   ```\n\nthen task [1](tasks/1.md)",
            "a fenced block stays as written"
        );
        assert_eq!(board.prose("open:\n```\nnever closed", Page::Index), "open:\n\n```\nnever closed\n```");
    }

    #[test]
    fn a_long_first_line_heads_its_note_cut_and_the_whole_text_follows() {
        assert_eq!(titled("Short\nThe rest\nand more"), ("Short".to_string(), "The rest\nand more".to_string()));
        let long = format!("{} end", "word ".repeat(30));
        let (heading, body) = titled(&format!("{long}\nsecond"));
        assert!(heading.ends_with('…') && heading.chars().count() <= TITLE + 1, "{heading}");
        assert!(body.starts_with("word word") && body.ends_with("second"), "{body}");
    }

    #[test]
    fn a_replaced_note_leaves_the_notes_in_force_and_says_what_replaced_it() {
        let items = board_of(serde_json::json!([
            task(1, "Ship it"),
            note(2, "Use the old way", serde_json::json!({"knowledge": "decision"})),
            note(3, "Use the new way", serde_json::json!({"knowledge": "decision", "supersedes": uid(2), "attachedTo": uid(1)})),
            note(4, "Stopped at the parser", serde_json::json!({"attachedTo": uid(1)})),
            note(5, "Stopped at the tests", serde_json::json!({"attachedTo": uid(1), "supersedes": uid(4)})),
        ]));
        let commits = HashMap::new();
        let board = Board::new(&items, &commits);
        let (page, count) = board.kind(Page::Decisions, "Decisions", "What was settled, and why:");
        assert_eq!((count.current, count.replaced), (1, 1));
        let (in_force, replaced) = page.split_once("## Replaced").expect("a section for the replaced");
        assert!(in_force.contains("- [3](#3) Use the new way") && !in_force.contains("- [2](#2)"), "{page}");
        assert!(in_force.contains("replaces [2](#2)") && in_force.contains("on task [1](tasks/1.md)"), "{page}");
        assert!(replaced.contains("<a id=\"2\"></a>2. Use the old way") && replaced.contains("replaced by [3](#3)"), "{page}");
        assert_eq!(board.label(4), "Earlier handoff");
        assert_eq!(board.label(5), "Handoff");
        let task = board.task(1);
        assert!(task.contains("- Decision [3](../decisions.md#3): Use the new way"), "{task}");
        assert!(task.find("4. Stopped at the parser") < task.find("5. Stopped at the tests"), "oldest first: {task}");
    }
}
