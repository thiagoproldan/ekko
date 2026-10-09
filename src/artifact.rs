//! Artifacts (task 1019): the plan for one goal, which lives across
//! sessions. A session writes it with the MCP tool `artifact`, the user reads
//! it on a web page ekko writes, and approves it in ekko's menu or with a
//! review on that page (task 1106), whose answer makes tasks of its steps.
//!
//! An artifact is a task with `Item::artifact` set. Its text is the plan, in
//! Markdown under `HEADINGS`, and the field holds what ekko acts on: the
//! steps, in order, the tasks the approval made of them, and the plan's
//! earlier texts. A task rather than a note, so the decisions and questions
//! about the plan attach to it, and the tasks its approval makes block it.
//!
//! The page is one HTML file, its style and script inline, beside a script
//! holding its version and the fonts it draws with, under the board's
//! `artifacts/`; nothing comes from the network. Every write that changes
//! what a page shows rewrites it, the page first, and the page reloads when
//! the version in the script changes: a classic script loads beside a
//! `file://` page where `fetch` is refused, as rustdoc loads its search
//! index (measured in Firefox 157, task 1059).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::item::{Artifact, Earlier, Item, Review, State, Step};
use crate::storage::ItemMap;

/// The headings a plan holds, in this order: what the goal is and why, what
/// was found before designing, the design with the options weighed, and
/// what could go wrong or is still open.
pub const HEADINGS: [&str; 4] = ["## Goal", "## What is known", "## Design", "## Risks and open questions"];

/// How many of its earlier texts an artifact keeps.
pub const EARLIER_KEPT: usize = 10;

/// The folder of the pages, under the board's directory.
pub const PAGES: &str = "artifacts";

/// The answer that approved a plan on a question asked without answers of
/// the session's own; every question asks with them since task 1044, so
/// this only stands in for one that somehow has none.
pub const APPROVE: &str = "Approve";

/// The most steps a plan holds: past this, it is several plans.
const STEPS_MOST: usize = 40;

/// The longest key a step takes.
const KEY_LONGEST: usize = 24;

/// How often the page looks for a newer version of itself, in milliseconds.
const POLL_MS: u32 = 2_000;

/// The themes a comment can carry (task 1213): each a color, and the name
/// the page offers for it until the person renames it there. Six, between
/// Kindle's four or five and Zotero's eight (decision 1214).
pub const THEMES: [(&str, &str); 6] =
    [("yellow", "Note"), ("blue", "Question"), ("pink", "Problem"), ("green", "Agree"), ("purple", "Change"), ("orange", "Idea")];

/// The longest name a theme takes.
const THEME_LONGEST: usize = 40;

/// Why a comment cannot carry the theme named `theme` in `color`, if it
/// cannot: a color that is not one of `THEMES`, or a name that is not one
/// short line.
pub fn theme_refused(theme: Option<&str>, color: Option<&str>) -> Option<String> {
    if let Some(color) = color.filter(|color| !THEMES.iter().any(|(key, _)| key == color)) {
        return Some(format!("{color:?} is not a theme's color: the colors are {}", THEMES.map(|(key, _)| key).join(", ")));
    }
    let theme = theme?;
    let fits = !theme.trim().is_empty() && theme.trim() == theme && theme.chars().count() <= THEME_LONGEST && !theme.chars().any(char::is_control);
    (!fits).then(|| format!("a theme's name is one line of 1 to {THEME_LONGEST} characters, with no space at either end: {theme:?}"))
}

/// A step as a session gives it, to write over the plan's steps.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StepSpec {
    pub key: String,
    /// What its task will say: a title, then the rest. A step the user
    /// approved keeps its own and takes none.
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub done_when: Option<String>,
    /// The earlier steps it waits on, by key.
    #[serde(default)]
    pub after: Vec<String>,
}

/// Why a plan's text cannot be an artifact's, if it cannot: a title on the
/// first line, then every heading of `HEADINGS`, each on a line of its own
/// and in order.
pub fn unplanned(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
    let title = lines.first().map(|line| line.trim()).unwrap_or_default();
    if title.is_empty() || title.starts_with('#') {
        return Some("an artifact's text starts with its title, on a line of its own, without a #".to_string());
    }
    let mut from = 1;
    for heading in HEADINGS {
        match lines[from..].iter().position(|line| *line == heading) {
            Some(at) => from += at + 1,
            None => {
                return Some(format!(
                    "an artifact's plan holds these headings, each on a line of its own and in this order: {}; {heading} is missing or out of order",
                    HEADINGS.join(", ")
                ))
            }
        }
    }
    None
}

/// The steps `given` makes of the plan's steps `old`, checked: keys a few
/// lower-case letters, digits or dashes, each once; a text whose first line
/// is a task's title, cut to one where it runs longer, with what the reply
/// says of each cut; `after` naming earlier steps only, which keeps them in
/// an order the work can follow. A step the user approved is a task now: it
/// stays, as it was, and the task is where it changes; named by its key
/// alone, it is kept whole.
pub fn steps(old: &[Step], given: &[StepSpec]) -> Result<(Vec<Step>, Vec<String>), String> {
    if given.len() > STEPS_MOST {
        return Err(format!("a plan holds at most {STEPS_MOST} steps, and this one gives {}: split the goal", given.len()));
    }
    // The text a step keeps of what was given: its title cut, if need be.
    let as_kept = |text: &str| crate::ekko::titled(text).map_or_else(|| text.to_string(), |(text, _)| text);
    let mut made: Vec<Step> = Vec::new();
    let mut told: Vec<String> = Vec::new();
    for spec in given {
        let key = spec.key.trim();
        let well_formed = !key.is_empty()
            && key.len() <= KEY_LONGEST
            && key.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            && !key.starts_with('-');
        if !well_formed {
            return Err(format!(
                "a step's key is 1 to {KEY_LONGEST} lower-case letters, digits or dashes, not starting with a dash: {:?}",
                spec.key
            ));
        }
        if made.iter().any(|step| step.key == key) {
            return Err(format!("two steps are keyed {key}"));
        }
        for earlier in &spec.after {
            if !made.iter().any(|step| step.key == earlier.trim()) {
                return Err(format!("step {key} waits on {earlier}, which is not a step before it: after names earlier steps only"));
            }
        }
        let after: Vec<String> = spec.after.iter().map(|key| key.trim().to_string()).collect();
        let done_when = spec.done_when.as_deref().map(str::trim).filter(|text| !text.is_empty()).map(str::to_string);
        if let Some(approved) = old.iter().find(|step| step.key == key && step.task.is_some()) {
            let text = spec.text.as_deref().map(str::trim);
            let changed = text.is_some_and(|text| as_kept(text) != approved.text)
                || (spec.done_when.is_some() && done_when != approved.done_when)
                || (!after.is_empty() && after != approved.after);
            if changed {
                return Err(format!("step {key} is approved, a task now: it stays as it is, and the task is where it changes"));
            }
            made.push(approved.clone());
            continue;
        }
        let text = spec.text.as_deref().map(str::trim).unwrap_or_default();
        if text.is_empty() {
            return Err(format!("step {key} needs its text: what its task will say, a title first"));
        }
        crate::ekko::fits("step", text).map_err(|error| error.to_string())?;
        let text = match crate::ekko::titled(text) {
            Some((text, cut)) => {
                told.push(cut.told(&format!("Step {key}'s")));
                text
            }
            None => text.to_string(),
        };
        let kept = old.iter().find(|step| step.key == key).map(|step| step.unknown.clone()).unwrap_or_default();
        made.push(Step { key: key.to_string(), text, done_when, after, task: None, unknown: kept });
    }
    if let Some(dropped) = old.iter().find(|step| step.task.is_some() && !made.iter().any(|kept| kept.key == step.key)) {
        return Err(format!(
            "step {} is approved, a task now: keep it among the steps, and cancel its task to drop it",
            dropped.key
        ));
    }
    Ok((made, told))
}

/// Raises the version of each artifact among `changed` whose plan this write
/// changed -- its text, or its steps but for the tasks an approval recorded
/// on them -- unless the write raised it already, and keeps the text it
/// replaced. One the write made starts at version 1.
pub fn keep_versions(before: &ItemMap, data: &mut ItemMap, changed: &[u32], now: i64) {
    let plan = |steps: &[Step]| steps.iter().map(|step| (step.key.clone(), step.text.clone(), step.done_when.clone(), step.after.clone())).collect::<Vec<_>>();
    for id in changed {
        let Some(item) = data.get_mut(id) else { continue };
        let Some(artifact) = item.artifact.as_mut() else { continue };
        let Some((was, old)) = before.get(id).and_then(|was| Some((was, was.artifact.as_deref()?))) else {
            artifact.version = artifact.version.max(1);
            continue;
        };
        let retexted = was.description != item.description;
        if artifact.version != old.version || (!retexted && plan(&old.steps) == plan(&artifact.steps)) {
            continue;
        }
        if retexted {
            artifact.earlier.push(Earlier { version: old.version, at: now, text: was.description.clone(), unknown: Default::default() });
            let excess = artifact.earlier.len().saturating_sub(EARLIER_KEPT);
            artifact.earlier.drain(..excess);
        }
        artifact.version = old.version + 1;
    }
}

/// Where an artifact stands on its board.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Standing {
    /// The artifact's own state, as a task.
    pub state: State,
    /// The open question asking the user to approve it.
    pub waiting: Option<u32>,
    pub steps: usize,
    /// Steps whose task is on the board, and of those, the ones done and
    /// the ones cancelled.
    pub tasks: usize,
    pub done: usize,
    pub cancelled: usize,
    /// Steps not approved yet.
    pub proposed: usize,
    /// The version is past the one the user approved.
    pub changed: bool,
    /// The user's reviews and sent comments on it, from its page, that no
    /// session resolved yet (task 1108).
    pub reviews: usize,
    pub comments: usize,
}

impl Standing {
    /// Of `item`, an artifact, on the board `all`: `None` for anything else.
    pub fn of(item: &Item, all: &ItemMap) -> Option<Standing> {
        let artifact = item.artifact.as_deref()?;
        let state = State::of(item)?;
        let index = crate::ekko::uid_index(all);
        let task = |step: &Step| step.task.as_deref().and_then(|uid| index.get(uid)).and_then(|id| all.get(id));
        let (mut tasks, mut done, mut cancelled, mut proposed) = (0, 0, 0, 0);
        for step in &artifact.steps {
            match (step.task.is_some(), task(step)) {
                (false, _) => proposed += 1,
                (true, Some(task)) => {
                    tasks += 1;
                    match State::of(task) {
                        Some(State::Done) => done += 1,
                        Some(State::Cancelled) => cancelled += 1,
                        _ => {}
                    }
                }
                (true, None) => {}
            }
        }
        let (mut reviews, mut comments) = (0, 0);
        for note in all.values().filter(|note| item.uid.is_some() && note.attached_to == item.uid && feedback(note)) {
            if note.review.is_some() {
                reviews += 1;
            } else {
                comments += 1;
            }
        }
        Some(Standing {
            state,
            waiting: approval_asked(item, all).map(|question| question.id),
            steps: artifact.steps.len(),
            tasks,
            done,
            cancelled,
            proposed,
            changed: artifact.approved_version.is_some_and(|approved| artifact.version > approved),
            reviews,
            comments,
        })
    }

    /// Still a plan being worked: neither done nor cancelled.
    pub fn open(&self) -> bool {
        self.state.is_open()
    }

    /// In a few words, as the prime, context and the page say it.
    pub fn words(&self) -> String {
        if !self.open() {
            return self.state.word().to_string();
        }
        let steps = |n: usize| if n == 1 { "1 step".to_string() } else { format!("{n} steps") };
        let mut words = Vec::new();
        if let Some(question) = self.waiting {
            words.push(format!("waiting on you: question {question}"));
        }
        if self.tasks == 0 {
            if self.waiting.is_none() {
                words.push(format!("draft, {}", steps(self.steps)));
            }
        } else {
            words.push(format!("approved, {} of {} done", self.done, self.tasks - self.cancelled));
            if self.proposed > 0 && self.waiting.is_none() {
                words.push(format!("{} to approve", steps(self.proposed)));
            }
        }
        if self.changed {
            words.push("changed since approved".to_string());
        }
        let count = |n: usize, what: &str| if n == 1 { format!("1 {what}") } else { format!("{n} {what}s") };
        match (self.reviews, self.comments) {
            (0, 0) => {}
            (reviews, 0) => words.push(format!("{} to resolve", count(reviews, "review"))),
            (0, comments) => words.push(format!("{} to resolve", count(comments, "comment"))),
            (reviews, comments) => words.push(format!("{} and {} to resolve", count(reviews, "review"), count(comments, "comment"))),
        }
        words.join(", ")
    }
}

/// An artifact as context shows it: its version, where it stands, and each
/// step with its task.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Planned {
    pub version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approved_version: Option<u32>,
    pub standing: String,
    pub steps: Vec<PlannedStep>,
    /// Its id while feedback from its page waits on a session, which
    /// context points to the artifact tool's read of (task 1107).
    #[serde(skip)]
    pub feedback: Option<u32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannedStep {
    pub key: String,
    pub title: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub after: Vec<String>,
    /// The task the approval made of it, while it is on the board.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<u32>,
    /// That task's state, "to approve", or "not on this board".
    pub state: &'static str,
}

impl Planned {
    /// Of `item`, on the board `all`: `None` for anything but an artifact.
    pub fn of(item: &Item, all: &ItemMap) -> Option<Planned> {
        let artifact = item.artifact.as_deref()?;
        let standing = Standing::of(item, all)?;
        let index = crate::ekko::uid_index(all);
        let steps = artifact
            .steps
            .iter()
            .map(|step| {
                let task = step.task.as_deref().and_then(|uid| index.get(uid)).and_then(|id| all.get(id));
                let state = match (&step.task, task) {
                    (None, _) => "to approve",
                    (Some(_), Some(task)) => State::of(task).map_or("a note", State::word),
                    (Some(_), None) => "not on this board",
                };
                PlannedStep {
                    key: step.key.clone(),
                    title: crate::ekko::title(&step.text).trim().to_string(),
                    after: step.after.clone(),
                    task: task.map(|task| task.id),
                    state,
                }
            })
            .collect();
        let feedback = (standing.reviews + standing.comments > 0).then_some(item.id);
        Some(Planned { version: artifact.version, approved_version: artifact.approved_version, standing: standing.words(), steps, feedback })
    }

    /// The lines context prints under the artifact's own.
    pub fn lines(&self) -> Vec<String> {
        let approved = self.approved_version.map(|version| format!(", version {version} approved")).unwrap_or_default();
        let mut lines = vec![format!("plan version {}{approved}: {}", self.version, self.standing)];
        if let Some(id) = self.feedback {
            lines.push(format!("feedback open: the artifact tool reads it whole with artifact {id}, and its apply, reply and resolve answer it"));
        }
        for step in &self.steps {
            let after = if step.after.is_empty() { String::new() } else { format!(" (after {})", step.after.join(", ")) };
            let task = match step.task {
                Some(id) => format!("task {id}, {}", step.state),
                None => step.state.to_string(),
            };
            lines.push(format!("step {}: {}{after} -> {task}", step.key, step.title));
        }
        lines
    }
}

/// The step of an artifact a task is, by the approval that made it.
#[derive(Debug, Serialize)]
pub struct StepOf {
    pub artifact: u32,
    pub key: String,
}

impl StepOf {
    /// Of task `item`, on the board `all`, if an approval made it.
    pub fn of(item: &Item, all: &ItemMap) -> Option<StepOf> {
        let uid = item.uid.as_deref()?;
        all.values().find_map(|artifact| {
            let step = artifact.artifact.as_deref()?.steps.iter().find(|step| step.task.as_deref() == Some(uid))?;
            Some(StepOf { artifact: artifact.id, key: step.key.clone() })
        })
    }
}

/// The open questions asking the user to approve `item`'s plan, newest
/// first: a review from the page answers the newest about the version the
/// plan is at with Approve, and the newest with Request changes (task 1106).
pub fn approvals_asked<'a>(item: &Item, all: &'a ItemMap) -> Vec<&'a Item> {
    let Some(uid) = item.uid.as_deref() else { return Vec::new() };
    let mut asking: Vec<&Item> = all
        .values()
        .filter(|note| note.trashed.is_none())
        .filter(|note| note.question.as_ref().is_some_and(|question| question.answer.is_none() && question.approve.as_ref().is_some_and(|approve| approve.artifact == uid)))
        .collect();
    asking.sort_by_key(|note| std::cmp::Reverse((note.timestamp, note.id)));
    asking
}

/// The two answers of a question asking to approve a plan: the one that
/// approves, which the session wrote first (ekko's word on a question
/// without its own), and the other, which a review requesting changes gives.
pub fn approval_answers(question: &Item) -> (String, String) {
    let approves = question.question.as_ref().and_then(|asked| asked.applies.clone()).unwrap_or_else(|| APPROVE.to_string());
    let (_, _, options, _) = crate::menu::parse(&question.description);
    let other = options.into_iter().map(|option| option.label).find(|label| *label != approves).unwrap_or_else(|| "Request changes".to_string());
    (approves, other)
}

/// What a review says when the user wrote nothing: its verdict, the version
/// and the comments it sent, by id (task 1106).
pub fn review_said(verdict: &str, version: u32, comments: &[u32]) -> String {
    let sending = match comments {
        [] => String::new(),
        [one] => format!(", sending comment {one}"),
        many => format!(", sending comments {}", many.iter().map(u32::to_string).collect::<Vec<_>>().join(", ")),
    };
    match verdict {
        Review::APPROVE => format!("Approved version {version}{sending}."),
        Review::CHANGES => format!("Changes requested on version {version}{sending}."),
        _ => format!("Commented on version {version}{sending}."),
    }
}

/// Whether `note` is the user's feedback from an artifact's page that waits
/// on the sessions (task 1108): a review, or a comment sent, written as the
/// person and neither trashed nor resolved. An approval with nothing to say,
/// no words and no comments, waits on nobody: its answer made the tasks.
pub fn feedback(note: &Item) -> bool {
    if note.trashed.is_some() || note.created_by.as_ref().is_some_and(|by| by.pid.is_some()) {
        return false;
    }
    if let Some(review) = note.review.as_deref() {
        let bare = review.verdict == Review::APPROVE && review.comments.is_empty() && note.description == review_said(Review::APPROVE, review.version, &[]);
        return review.resolved.is_none() && !bare;
    }
    note.comment.as_deref().is_some_and(|comment| comment.sent.is_some() && comment.resolved.is_none())
}

/// What Start on a ready step of the page sends (task 1111): a comment on
/// that step, sent at once, which is told to the sessions working the plan
/// as any comment sent alone is -- Kiro's Start task, with no session
/// started from the page.
pub const START: &str = "Start this";

/// Whether `note` is a Start: the person's comment on a step that says
/// `START` and nothing else, sent.
pub fn is_start(note: &Item) -> bool {
    note.created_by.as_ref().is_none_or(|by| by.pid.is_none())
        && note.description.trim() == START
        && note.comment.as_deref().is_some_and(|comment| comment.step.is_some() && comment.quote.is_none() && comment.sent.is_some())
}

/// The Start that still waits on step `key` of the artifact `item`: in no
/// trash, and not resolved, as it is once the step's task is taken up.
pub fn open_start<'a>(item: &Item, key: &str, all: &'a ItemMap) -> Option<&'a Item> {
    let uid = item.uid.as_deref()?;
    all.values().find(|note| {
        note.trashed.is_none()
            && note.attached_to.as_deref() == Some(uid)
            && is_start(note)
            && note.comment.as_deref().is_some_and(|comment| comment.step.as_deref() == Some(key) && comment.resolved.is_none())
    })
}

/// Why a step's `task` cannot be started from the page, if it cannot.
/// Start asks a session to take up work it may begin now: pending or
/// paused, with no one in particular, and waiting on nothing open.
pub fn startable(task: &Item, all: &ItemMap) -> Result<(), String> {
    if task.trashed.is_some() {
        return Err(format!("task {} is in the trash", task.id));
    }
    match State::of(task) {
        Some(State::Pending | State::Paused) => {}
        Some(State::Waiting) => return Err(format!("task {} is waiting on something outside the board", task.id)),
        Some(state) => return Err(format!("task {} is {} already", task.id, state.word())),
        None => return Err(format!("{} is no task", task.id)),
    }
    if let Some(with) = &task.with {
        return Err(format!("task {} is with {with}: theirs to take up, not a session's", task.id));
    }
    let index = crate::ekko::uid_index(all);
    let mut open: Vec<u32> = task
        .blocked_by
        .iter()
        .flatten()
        .filter_map(|uid| index.get(uid.as_str()).copied())
        .filter(|id| all.get(id).is_some_and(crate::ekko::holds))
        .collect();
    if open.is_empty() {
        return Ok(());
    }
    open.sort_unstable();
    let open: Vec<String> = open.iter().map(u32::to_string).collect();
    Err(format!("task {} waits on {}, still open", task.id, open.join(", ")))
}

/// The open question asking the user to approve `item`'s plan, if one does:
/// the newest, on a board that holds more than one.
pub fn approval_asked<'a>(item: &Item, all: &'a ItemMap) -> Option<&'a Item> {
    approvals_asked(item, all).into_iter().next()
}

/// The page of artifact `item` on the board `all`, whose project folder is
/// `folder`, and the version it carries. Its look is akqa.com's (decision
/// 1355, plan 1364): scenes told as one scrolls, each in a mode, light or
/// dark, the page takes as it reaches the window's middle. The first opens
/// with where the plan stands and its title; then the plan, the Goal's
/// first sentence as a statement and each other section opened by its
/// heading in capitals; then the steps, the map their `after` draws, the
/// notes and how the text changed. Medium's section bars at the right edge
/// name the scenes, AKQA's mark stays at the top, and AKQA's bar at the
/// bottom finds a section, step or note (note 1149) and runs the page's
/// commands, with Review beside it (task 1337).
pub fn page(item: &Item, all: &ItemMap, folder: Option<&Path>) -> (String, String) {
    let artifact = item.artifact.as_deref();
    let standing = Standing::of(item, all);
    let (title, plan) = item.description.split_once('\n').unwrap_or((item.description.as_str(), ""));
    let title = title.trim();
    let board = folder.and_then(Path::file_name).map_or_else(|| "the default board".to_string(), |name| format!("project {}", name.to_string_lossy()));
    let uid = item.uid.as_deref().unwrap_or_default();
    let steps = artifact.map(|artifact| shown(artifact, all)).unwrap_or_default();
    let mut notes: Vec<&Item> =
        all.values().filter(|note| !note.is_task && note.trashed.is_none() && !uid.is_empty() && note.attached_to.as_deref() == Some(uid)).collect();
    notes.sort_by_key(|note| std::cmp::Reverse((note.timestamp, note.id)));
    // A comment on the plan's words is those words, colored (task 1213),
    // and an entry under Comments, oldest first; the other notes go under
    // Notes.
    let (mut comments, notes): (Vec<&Item>, Vec<&Item>) = notes.into_iter().partition(|note| note.comment.is_some());
    comments.reverse();
    let tasks = steps.iter().filter(|step| step.task.is_some() && step.class != "cancelled").count();
    let done = steps.iter().filter(|step| step.class == "done").count();
    let version = artifact.map_or(0, |artifact| artifact.version);
    let command = format!("ekko artifact {}", item.id);
    let by = written_by(item);
    // The page's parts, by id and name, as the section bars and the bar list them.
    let mut parts: Vec<(String, String)> = Vec::new();

    let mut body = String::new();

    // The page as AKQA builds one (task 1367): scenes the window's width,
    // each in its mode, light or dark, which the page takes as the scene
    // reaches the window's middle. No column beside the text and no head
    // above it: the first scene opens with where the plan stands, and
    // History says who wrote it and where it is. Nor a top bar (task 1337):
    // what it held is run from the pill at the bottom.
    let _ = write!(
        body,
        "<main class=\"page\"><article class=\"prose\" data-version=\"{version}\"><section class=\"scene hero\" id=\"top\" data-mode=\"light\"><p class=\"kicker\"><span>Artifact {}</span>",
        item.id
    );
    if let Some(priority) = item.priority.filter(|priority| *priority > 1) {
        let _ = write!(body, "<span>Priority {priority}</span>");
    }
    if let Some(standing) = &standing {
        let waiting = if standing.waiting.is_some() { " waiting" } else { "" };
        let _ = write!(body, "<span class=\"state{waiting}\">{}</span>", phase(standing));
    }
    let _ = write!(body, "</p>{}", headline(title));
    // What waits on the user, AKQA's black banner under the title (task
    // 1368): the approval asked, with Review beside it, then each other
    // question open on the artifact or on a step's task, as the menu shows
    // it, answered here where the page writes (task 1110). What only
    // informs is a quiet box.
    let approval = standing.as_ref().and_then(|standing| standing.waiting);
    if let Some(question) = approval {
        let answer = format!(
            "It asks you to approve this plan: answer it in ekko's menu<span class=\"writes-only\">, with Review on this page</span>, or with <code>ekko --answer {question}</code> in a terminal."
        );
        banner(&mut body, &format!("Waiting on you: question {question}"), &answer, "<button class=\"pill writes-only\" type=\"button\" data-review>Review</button>");
    }
    for (question, step) in asked_here(item, all, &steps) {
        if Some(question.id) == approval {
            continue;
        }
        // Another question asking to approve a plan, on a board holding
        // more than one: Review answers the newest, the menu the others.
        if question.question.as_ref().is_some_and(|asked| asked.approve.is_some()) {
            let answer = format!("{}: answer it in ekko's menu, or with <code>ekko --answer {}</code> in a terminal.", esc(crate::ekko::title(&question.description)), question.id);
            banner(&mut body, &format!("Waiting on you: question {}", question.id), &answer, "");
            continue;
        }
        question_card(&mut body, question, step);
    }
    if let (Some(artifact), Some(true)) = (artifact, standing.as_ref().map(|standing| standing.changed)) {
        let changed = format!("The plan changed after version {} was approved: History shows how.", artifact.approved_version.unwrap_or_default());
        callout(&mut body, "Changed since it was approved", &changed);
    }
    if item.trashed.is_some() || item.stashed.is_some() {
        let away = if item.trashed.is_some() { "in the trash" } else { "stashed" };
        callout(&mut body, &capitalized(away), &format!("This artifact is {away}."));
    }
    // The plan's facts once, AKQA's row of results, each leading to its
    // part: the steps done, the notes, the comments, replies counted, and
    // the version.
    body.push_str("<div class=\"facts\">");
    let mut fact = |to: &str, big: String, small: String| {
        let _ = write!(body, "<a class=\"fact\" href=\"#{to}\"><b>{}</b><span>{}</span></a>", esc(&big), esc(&small));
    };
    let unit = |count: usize, word: &str| if count == 1 { word.to_string() } else { format!("{word}s") };
    let proposed = standing.as_ref().map_or(0, |standing| standing.proposed);
    if tasks > 0 {
        let more = if proposed > 0 { format!(" \u{b7} {proposed} to approve") } else { String::new() };
        fact("steps", format!("{done}/{tasks}"), format!("steps done{more}"));
    } else if !steps.is_empty() {
        fact("steps", steps.len().to_string(), unit(steps.len(), "step"));
    }
    if !notes.is_empty() {
        fact("notes", notes.len().to_string(), unit(notes.len(), "note"));
    }
    if !comments.is_empty() {
        fact("comments", comments.len().to_string(), unit(comments.len(), "comment"));
    }
    fact("history", format!("v{version}"), if version > 1 { "the current version" } else { "the first version" }.to_string());
    body.push_str("</div>");

    // The plan, a scene to a section: the Goal's first sentence as its
    // statement, each other section opened by its heading; what comes
    // before the first leads it, in the first scene.
    let mut ids = Vec::new();
    let mut cut = sections(plan).into_iter().peekable();
    if cut.peek().is_none() {
        body.push_str("<p>The plan has no text yet.</p>");
    }
    if let Some((_, text)) = cut.next_if(|(heading, _)| heading.is_empty()) {
        let _ = write!(body, "<section class=\"lead\" data-plan>{}</section>", markdown(&text));
    }
    body.push_str("</section>");
    let (mut goal_shown, mut known_shown, mut design_shown, mut risks_shown) = (false, false, false, false);
    for (heading, text) in cut {
        let id = unique(&mut ids, &format!("plan-{}", slug(&heading)));
        let known = if heading == "What is known" && !known_shown { known_scene(&text, &id) } else { None };
        let design = if heading == "Design" && !design_shown { design_scene(&text) } else { None };
        let risks = if heading == "Risks and open questions" && !risks_shown { risks_scene(&text, &id) } else { None };
        if heading == "Goal" && !goal_shown {
            goal_shown = true;
            let _ = write!(body, "<section class=\"scene goal\" id=\"{id}\" data-part=\"Goal\" data-mode=\"{}\" data-plan>{}</section>", mode(&id), goal_scene(&text));
        } else if let Some((scene, points)) = design {
            design_shown = true;
            let _ = write!(
                body,
                "<section class=\"scene part design\" id=\"{id}\" data-part=\"{}\" data-mode=\"{}\" data-plan>{}{scene}</section>",
                esc(&heading),
                mode(&id),
                opener(&heading, Some(&format!("{points} decisions")))
            );
        } else if let Some(scene) = known {
            known_shown = true;
            let (first, second) = two_tones(&heading);
            let _ = write!(
                body,
                "<section class=\"scene part known\" id=\"{id}\" data-part=\"{}\" data-mode=\"{}\" data-plan>{}{scene}</section>",
                esc(&heading),
                mode(&id),
                opener(&first, second.as_deref())
            );
        } else if let Some(scene) = risks {
            risks_shown = true;
            let (first, second) = two_tones(&heading);
            let _ = write!(
                body,
                "<section class=\"scene part risks\" id=\"{id}\" data-part=\"{}\" data-mode=\"{}\" data-plan>{}{scene}</section>",
                esc(&heading),
                mode(&id),
                opener(&first, second.as_deref())
            );
        } else {
            let (first, second) = two_tones(&heading);
            let _ = write!(
                body,
                "<section class=\"scene part\" id=\"{id}\" data-part=\"{}\" data-mode=\"{}\" data-plan>{}{}</section>",
                esc(&heading),
                mode(&id),
                opener(&first, second.as_deref()),
                markdown(&text)
            );
        }
        parts.push((id, heading));
    }
    // The comments as data the script anchors to their words, and the
    // themes it offers; both are written even with no comment yet, for the
    // first one.
    let data: Vec<serde_json::Value> = comments
        .iter()
        .map(|note| {
            let mine = note.created_by.as_ref().is_none_or(|by| by.pid.is_none());
            serde_json::json!({"id": note.id, "uid": note.uid, "text": note.description, "when": when(note.timestamp), "by": written_by(note), "mine": mine, "comment": note.comment})
        })
        .collect();
    let script_data = |value: &serde_json::Value| value.to_string().replace('<', "\\u003c");
    let themes = serde_json::json!(THEMES.map(|(color, name)| [color, name]));
    let _ = write!(
        body,
        "<script type=\"application/json\" id=\"comments-data\">{}</script><script type=\"application/json\" id=\"themes\">{}</script><script type=\"application/json\" id=\"review-data\">{}</script>",
        script_data(&serde_json::json!(data)),
        script_data(&themes),
        script_data(&review_data(item, all, version))
    );

    // Steps (task 1373): AKQA's ROLE carousel as a stage, one step on it at
    // a time, whole: the first not done or cancelled, or else the first. A
    // segment a step above it; under each the steps it waits on, as chips
    // leading to them; under the stage how far along it is, a switch that
    // shows the whole list, and the way back and on. In the list, as with
    // one step alone, each step has its task and opens in place to the rest
    // of its text and when it is done.
    parts.push(("steps".to_string(), "Steps".to_string()));
    let progress = match (tasks, steps.len()) {
        (0, 0) => None,
        (0, count) => Some(plural(count, "step")),
        (tasks, _) => Some(format!("{done} of {tasks} done")),
    };
    let staged = (steps.len() > 1).then(|| steps.iter().position(|step| !matches!(step.class, "done" | "cancelled")).unwrap_or(0));
    let _ = write!(
        body,
        "<section class=\"scene part{}\" id=\"steps\" data-part=\"Steps\" data-mode=\"{}\">{}",
        if staged.is_some() { " staged" } else { "" },
        mode("steps"),
        opener("Steps", progress.as_deref())
    );
    if steps.is_empty() {
        body.push_str("<p>No steps yet: the artifact tool writes them.</p>");
    } else {
        if let Some(on) = staged {
            body.push_str("<div class=\"segs\" role=\"group\" aria-label=\"Steps\">");
            for (at, step) in steps.iter().enumerate() {
                let current = if at == on { " aria-current=\"step\"" } else { " tabindex=\"-1\"" };
                let _ = write!(body, "<button class=\"seg {}\" type=\"button\"{current}><span>{:02} {}</span></button>", step.class, at + 1, esc(step.title));
            }
            body.push_str("</div><ol class=\"steps\" tabindex=\"0\" aria-label=\"Steps, one at a time\">");
        } else {
            body.push_str("<ol class=\"steps\">");
        }
        for (at, step) in steps.iter().enumerate() {
            let mut meta = esc(&step.state);
            if let Some(id) = step.task {
                let _ = write!(meta, " \u{b7} task {id}");
            }
            if !step.step.after.is_empty() {
                let _ = write!(meta, " \u{b7} after {}", esc(&step.step.after.join(", ")));
            }
            let head = format!(
                "<span class=\"num\">{:02}</span><span class=\"title\">{}</span><span class=\"meta\"><span class=\"dot {}\"></span>{meta}</span>",
                at + 1,
                esc(step.title),
                step.class
            );
            // The step on the stage comes open; the others wait hidden.
            let open = staged == Some(at);
            let more = !step.rest.is_empty() || step.step.done_when.is_some();
            let _ = write!(
                body,
                "<li class=\"step {}{}\" id=\"step-{}\" data-state=\"{}\"{}>",
                step.class,
                if open && more { " open" } else { "" },
                esc(&step.step.key),
                esc(&step.state),
                if staged.is_some() && !open { " hidden" } else { "" }
            );
            if more {
                let _ = write!(body, "<button class=\"step-head\" type=\"button\" aria-expanded=\"{}\">{head}</button>", open);
            } else {
                let _ = write!(body, "<div class=\"step-head\">{head}</div>");
            }
            let waits: Vec<String> = step
                .step
                .after
                .iter()
                .filter_map(|key| steps.iter().position(|other| other.step.key == *key).map(|place| (key, place)))
                .map(|(key, place)| format!("<button class=\"pill chip\" type=\"button\" data-step=\"{}\"><i aria-hidden=\"true\">\u{2196}</i> after {:02} {}</button>", esc(key), place + 1, esc(key)))
                .collect();
            if staged.is_some() && !waits.is_empty() {
                let _ = write!(body, "<div class=\"after\">{}</div>", waits.concat());
            }
            // Start, on a step whose task a session may take up now (task
            // 1111), where the page writes; once pressed, the Start it sent,
            // until the task is taken up.
            if step.task.and_then(|id| all.get(&id)).is_some_and(|task| startable(task, all).is_ok()) {
                match open_start(item, &step.step.key, all) {
                    Some(asked) => {
                        let _ = write!(body, "<p class=\"start asked\">Start sent in comment {}: waiting for a session to take it up</p>", asked.id);
                    }
                    None => {
                        let _ = write!(
                            body,
                            "<div class=\"start writes-only\"><button class=\"pill\" type=\"button\" data-start=\"{}\">Start</button><span class=\"status\" role=\"status\"></span></div>",
                            esc(&step.step.key)
                        );
                    }
                }
            }
            if !more {
                body.push_str("</li>");
                continue;
            }
            // Its text is Markdown, as the plan's is, and said by the same
            // rules (task 1360).
            body.push_str("<div class=\"more\">");
            if !step.rest.is_empty() {
                body.push_str(&markdown(step.rest));
            }
            if let Some(done_when) = &step.step.done_when {
                let _ = write!(body, "<p class=\"when\"><b>Done when</b> {}</p>", inline(done_when));
            }
            body.push_str("</div></li>");
        }
        body.push_str("</ol>");
        if let Some(on) = staged {
            let _ = write!(
                body,
                "<div class=\"controls\"><span class=\"count\">{:02} / {:02}</span><span class=\"grow\"></span><button class=\"toggle\" type=\"button\" role=\"switch\" aria-checked=\"false\"><i aria-hidden=\"true\"></i><span>All steps</span></button>{}</div>",
                on + 1,
                steps.len(),
                arrows(on == 0, on + 1 == steps.len())
            );
        }
    }
    body.push_str("</section>");

    // Map: the steps as the graph their `after` draws, on a strip the
    // window's width, with arrows while it is wider than the window. With
    // two stages or more, a player that unfolds it stage by stage (task
    // 1374): Play, a slider through the stages and a line saying the one
    // shown, written as a page without its script shows the map, whole. The
    // slider's autocomplete is off: Firefox gave it back its value across
    // the file page's reload, apart from the stage shown (task 1393).
    if !steps.is_empty() {
        parts.push(("map".to_string(), "Map".to_string()));
        let stages = layout(&steps).iter().map(|(column, _)| column + 1).max().unwrap_or(1);
        let player = if stages > 1 {
            format!(
                "<div class=\"player\"><button class=\"round play\" type=\"button\" aria-label=\"Play\"><svg viewBox=\"0 0 24 24\" aria-hidden=\"true\"><path d=\"M8.5 5.8v12.4L18.2 12z\"/></svg></button><input class=\"scrub\" type=\"range\" autocomplete=\"off\" min=\"1\" max=\"{stages}\" step=\"1\" value=\"{stages}\" style=\"--p: 100%\" aria-label=\"How far the plan unfolds\" aria-valuetext=\"Stage {stages} of {stages}\"><p class=\"at\" aria-live=\"polite\">The whole plan, in {stages} stages</p></div>"
            )
        } else {
            String::new()
        };
        let _ = write!(
            body,
            "<section class=\"scene part\" id=\"map\" data-part=\"Map\" data-mode=\"{}\">{}<p>What each step waits on, from the first on the left. A step pointed at lights what it waits on and what waits on it, and a click leads to it among the steps.</p><div class=\"legend\"><span><span class=\"dot proposed\"></span>to approve</span><span><span class=\"dot pending\"></span>pending</span><span><span class=\"dot progress\"></span>in progress</span><span><span class=\"dot done\"></span>done</span><span>an arrow: what a step waits on</span></div>{player}<div class=\"bleed\"><div class=\"strip\">{}</div><div class=\"arrows\" hidden><button type=\"button\" data-by=\"-480\" aria-label=\"Back\">\u{2039}</button><button type=\"button\" data-by=\"480\" aria-label=\"On\">\u{203a}</button></div></div></section>",
            mode("map"),
            opener("Map", Some("what waits on what")),
            map(&steps)
        );
    }

    // Notes and comments, one scene (task 1375).
    if let Some((id, name, scene)) = talk_scene(&notes, &comments, all) {
        parts.push((id.to_string(), name.to_string()));
        body.push_str(&scene);
    }

    // History (task 1376): who wrote the plan and when, the board it is on
    // and whom the page writes as, in a line at its head; the version
    // against the one approved; then the texts the plan kept on a line and
    // the changes between them, one at a time.
    parts.push(("history".to_string(), "History".to_string()));
    let versions = if version > 1 { format!("{version} versions") } else { "the first version".to_string() };
    let mut about = Vec::new();
    if artifact.is_some() {
        about.push(format!("Version {version}"));
    }
    about.push(format!("updated {}", esc(&when(item.updated_at.unwrap_or(item.timestamp)))));
    if let Some(by) = &by {
        about.push(format!("written by {}", esc(by)));
    }
    about.push(esc(&board));
    let _ = write!(
        body,
        "<section class=\"scene part changes\" id=\"history\" data-part=\"History\" data-mode=\"{}\">{}<p class=\"about\">{}<span id=\"who\"></span></p>",
        mode("history"),
        opener("History", Some(&versions)),
        capitalized(&about.join(" \u{b7} "))
    );
    if let Some(artifact) = artifact {
        let approved = match artifact.approved_version {
            Some(approved) if approved == version => format!("Version {version} is the current one, and the one you approved."),
            Some(approved) => format!("Version {version} is the current one; version {approved} is the one you approved."),
            None => format!("Version {version} is the current one; none is approved yet."),
        };
        let _ = write!(body, "<p class=\"history\">{approved}</p>");
        let mut earlier: Vec<&Earlier> = artifact.earlier.iter().collect();
        earlier.sort_by_key(|earlier| earlier.version);
        if earlier.is_empty() {
            body.push_str("<p class=\"history\">One text so far: nothing to compare. Each change to the plan's text keeps the one it replaced here.</p>");
        } else {
            body.push_str(&texts_kept(item, &earlier, version));
        }
    }
    body.push_str("</section>");

    // What's next (task 1377), AKQA's closing scene, dark: the review asked
    // for, else the next step, the first not done or cancelled, as the stage
    // takes it; then the page's last moves, Review, the way to that step,
    // the command and the way back to the top; the footer under them.
    parts.push(("next".to_string(), "What's next".to_string()));
    let next = steps.iter().position(|step| !matches!(step.class, "done" | "cancelled"));
    let said = match (approval, next) {
        (Some(_), _) => "This plan waits on your review: approve it, or ask for changes with comments on its words.".to_string(),
        (None, Some(at)) => format!("Step {:02}, {}: {}", at + 1, esc(&steps[at].state), esc(steps[at].title)),
        (None, None) if steps.is_empty() => "No steps yet: the artifact tool writes them.".to_string(),
        (None, None) if steps.iter().all(|step| step.class == "done") => "Every step is done.".to_string(),
        (None, None) => "Every step is done or cancelled.".to_string(),
    };
    let _ = write!(
        body,
        "<section class=\"scene part close\" id=\"next\" data-part=\"What's next\" data-mode=\"{}\">{}<p class=\"next\">{said}</p><div class=\"acts\"><button class=\"pill writes-only\" type=\"button\" data-review>Review</button>",
        mode("next"),
        opener("What's", Some("next?"))
    );
    if let (None, Some(at)) = (approval, next) {
        let _ = write!(body, "<button class=\"pill\" type=\"button\" data-step=\"{}\">Go to step {:02}</button>", esc(&steps[at].step.key), at + 1);
    }
    let _ = write!(
        body,
        "<button class=\"pill\" type=\"button\" data-copy>Copy {command}</button><button class=\"pill\" type=\"button\" data-top>Back to the top <i aria-hidden=\"true\">\u{2191}</i></button></div><footer class=\"foot\">Written by ekko {} from the board. It reloads by itself when the board changes, and <code>{command}</code> writes it again. Set in Inter and in Ekko Serif, Adobe's Source Serif 4 cut for this page, under the <a href=\"{FONTS_DIR}/{}\">SIL Open Font License</a>.</footer></section></article></main>",
        env!("CARGO_PKG_VERSION"),
        FONT_LICENSE.0
    );
    // The sheet a note opens in (task 1375), out of the scenes: a modal
    // dialog, in the page's top layer, which the note is moved into.
    if !notes.is_empty() {
        body.push_str("<dialog class=\"side prose\" aria-label=\"Note\"><div class=\"side-in\"><button class=\"round shut\" type=\"button\" aria-label=\"Close\"><svg viewBox=\"0 0 24 24\" aria-hidden=\"true\"><path d=\"M6.5 6.5l11 11M17.5 6.5l-11 11\"/></svg></button><div class=\"side-note\"></div></div></dialog>");
    }

    // Medium's section bars, naming the scenes, AKQA's mark at the top,
    // and AKQA's bar.
    let scenes: Vec<(&str, &str)> = std::iter::once(("top", "Overview")).chain(parts.iter().map(|(id, name)| (id.as_str(), name.as_str()))).collect();
    body.push_str("<nav class=\"toc\" aria-label=\"Sections\"><button class=\"toc-bars\" type=\"button\" aria-label=\"Sections\">");
    body.push_str(&"<span></span>".repeat(scenes.len()));
    body.push_str("</button><div class=\"toc-card\"><ol>");
    for (id, name) in &scenes {
        let _ = write!(body, "<li><a href=\"#{id}\"><span class=\"d\"></span><span class=\"t\">{}</span></a></li>", esc(name));
    }
    let _ = write!(body, "</ol></div></nav><div class=\"mark\" aria-hidden=\"true\">ekko <b>\u{b7}</b> artifact {}</div>", item.id);
    let _ = write!(body, "<div class=\"bar\" id=\"bar\" data-command=\"{command}\">");
    body.push_str("<div class=\"bar-surface\"><div class=\"bar-fold list-fold\" inert><div class=\"fold-in\"><div class=\"bar-list\" role=\"listbox\" aria-label=\"Suggestions\"></div></div></div><div class=\"bar-fold quote-fold\" inert><div class=\"fold-in\"><div class=\"bar-quote\"><span class=\"bar-quote-text\"></span><button class=\"bar-suggest\" type=\"button\" aria-pressed=\"false\" title=\"Suggest the words to put in their place\">Suggest</button><button class=\"bar-quote-drop\" type=\"button\" aria-label=\"Drop the quote\">\u{d7}</button></div><div class=\"bar-themes\" role=\"radiogroup\" aria-label=\"Theme\"></div><div class=\"bar-tell\"></div><div class=\"bar-why\"></div></div></div><div class=\"bar-hairline\" aria-hidden=\"true\"></div><div class=\"bar-line\"><div class=\"bar-field\"><span class=\"bar-cursor\" aria-hidden=\"true\"></span><span class=\"bar-hint\" aria-hidden=\"true\"></span><input aria-label=\"Jump to a section, step or note\" autocomplete=\"off\" spellcheck=\"false\"><textarea class=\"bar-note\" rows=\"1\" aria-label=\"Comment on the quoted words\" placeholder=\"Comment on these words\" hidden></textarea></div><button class=\"bar-send\" type=\"button\" aria-label=\"Send the comment\" hidden>\u{2191}</button><button class=\"bar-menu\" type=\"button\" aria-label=\"Open menu\" aria-expanded=\"false\"><span></span><span></span></button></div><div class=\"bar-fold nav-fold\" inert><div class=\"fold-in\"><nav class=\"bar-nav\" aria-label=\"Parts\">");
    let (plan_parts, ours): (Vec<_>, Vec<_>) = parts.iter().partition(|(id, _)| id.starts_with("plan-"));
    if let Some((id, _)) = plan_parts.first() {
        let _ = write!(body, "<a href=\"#{id}\">Plan</a>");
    }
    for (id, name) in ours {
        // The scene of notes and comments is a part each in the menu, which
        // leads to its tab.
        if id == "notes-and-comments" {
            body.push_str("<a href=\"#notes\">Notes</a><a href=\"#comments\">Comments</a>");
        } else {
            let _ = write!(body, "<a href=\"#{id}\">{name}</a>");
        }
    }
    // The commands the menu offers besides the parts, which the script
    // writes, and Review, a round button beside the pill (task 1337).
    body.push_str("</nav><div class=\"bar-acts\" role=\"group\" aria-label=\"Commands\"></div></div></div></div>");
    body.push_str("<button class=\"bar-side writes-only\" type=\"button\" data-review aria-label=\"Review\" title=\"Review (R)\"><svg viewBox=\"0 0 24 24\" aria-hidden=\"true\"><path d=\"M12 3.75c4.83 0 8.75 3.25 8.75 7.25S16.83 18.25 12 18.25c-.95 0-1.86-.12-2.72-.36L5.2 19.9l1.07-3.47C4.48 15.11 3.25 13.16 3.25 11c0-4 3.92-7.25 8.75-7.25z\"/><path d=\"M8.6 11.1l2.25 2.25 4.6-4.6\"/></svg><span class=\"count\"></span></button></div>");

    // The page before its version and after it: the version is the hash of
    // the two, so a change to anything the page holds, its style and script
    // as a new ekko writes them too, is a version of its own.
    let faces = font_faces();
    let before = format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>{} \u{b7} artifact {}</title>\n<script>{THEME}</script>\n<style>\n{faces}{STYLE}</style>\n</head>\n<body>\n{body}\n<script>{SCRIPT}</script>\n<script>(function () {{\n  var version = \"",
        esc(title),
        item.id
    );
    // The page follows its version (task 1104). Served, it does so through
    // the one stream of /events a browser holds: the tab holding the Web
    // Lock opens it and relays it to the others (decision 1165), and every
    // tab loads its script once whenever the stream opens, so a new server
    // learns the page and a version missed meanwhile still comes. As a
    // file, or without locks or BroadcastChannel, it loads its script every
    // POLL_MS. A version waits while a field other than the bar's, whose
    // words ekkoKeep keeps, holds words being typed.
    let after = format!(
        "\";\n  var check = function () {{\n    var script = document.createElement(\"script\");\n    script.src = \"{}.js?t=\" + Date.now();\n    script.onload = script.onerror = function () {{ script.remove(); }};\n    document.head.appendChild(script);\n  }};\n  window.ekkoArtifact = function (seen) {{\n    if (seen === version) return;\n    var field = document.activeElement;\n    if (field && (field.tagName === \"TEXTAREA\" || field.isContentEditable || (field.tagName === \"INPUT\" && !field.closest(\".bar-line\")))) {{\n      field.addEventListener(\"blur\", function () {{ ekkoArtifact(seen); }}, {{ once: true }});\n      return;\n    }}\n    if (window.ekkoKeep) ekkoKeep();\n    location.reload();\n  }};\n  if (location.protocol === \"http:\" && navigator.locks && window.BroadcastChannel && window.EventSource) {{\n    var channel = new BroadcastChannel(\"ekko-events\");\n    var take = function (data) {{\n      if (data === \"open\") return check();\n      var at = data.indexOf(\" \");\n      if (data.slice(0, at) === location.pathname) ekkoArtifact(data.slice(at + 1));\n    }};\n    channel.onmessage = function (message) {{ take(message.data); }};\n    navigator.locks.request(\"ekko-events\", function () {{\n      return new Promise(function () {{\n        var source = new EventSource(\"/events\");\n        source.onopen = function () {{ channel.postMessage(\"open\"); check(); }};\n        source.addEventListener(\"version\", function (event) {{ channel.postMessage(event.data); take(event.data); }});\n      }});\n    }});\n  }} else setInterval(check, {POLL_MS});\n  if (location.protocol === \"http:\") fetch(\"/api/who\", {{ method: \"POST\" }}).then(function (answer) {{ return answer.json(); }}).then(function (who) {{\n    var text = who.person ? \" \\u00b7 writes as you\" : \" \\u00b7 reads only: \" + who.why;\n    document.getElementById(\"who\").textContent = text;\n    if (who.person) {{\n      document.body.dataset.writes = \"\";\n      if (window.ekkoWrites) ekkoWrites();\n    }}\n  }}, function () {{}});\n}})();</script>\n</body>\n</html>\n",
        js_string(uid)
    );
    let version = format!("{:016x}", fnv_from(fnv(before.as_bytes()), after.as_bytes()));
    (format!("{before}{version}{after}"), version)
}

/// A step as the page shows it: its title and the rest of its text, and its
/// task while the board holds it.
struct Shown<'a> {
    step: &'a Step,
    title: &'a str,
    rest: &'a str,
    task: Option<u32>,
    /// Its task's state as the views word it, or why it has no task.
    state: String,
    /// The class the page styles that state with.
    class: &'static str,
}

fn shown<'a>(artifact: &'a Artifact, all: &ItemMap) -> Vec<Shown<'a>> {
    let index = crate::ekko::uid_index(all);
    artifact
        .steps
        .iter()
        .map(|step| {
            let (title, rest) = step.text.split_once('\n').unwrap_or((step.text.as_str(), ""));
            let (task, state, class) = match step.task.as_deref() {
                None => (None, "to approve".to_string(), "proposed"),
                Some(uid) => match index.get(uid).and_then(|id| all.get(id)) {
                    Some(task) => {
                        let word = State::of(task).map_or("a note", State::word);
                        (Some(task.id), word.to_string(), state_class(word))
                    }
                    None => (None, "not on this board".to_string(), "gone"),
                },
            };
            Shown { step, title: title.trim(), rest: rest.trim(), task, state, class }
        })
        .collect()
}

/// Who wrote `item`, as the page says it: the user, or a session named by
/// its process; `None` for one written before authors were recorded
/// (0.19.0).
fn written_by(item: &Item) -> Option<String> {
    let by = item.created_by.as_ref()?;
    Some(if by.pid.is_none() { "the user".to_string() } else { format!("a session, {}", by.label()) })
}

/// A comment's theme as the page shows it: its color, and the name it was
/// written under, else its color's own name; one written before themes is
/// in the first theme, as the page's filters count it (task 1213).
fn theme_of(comment: &crate::item::Comment) -> (&'static str, String) {
    let (color, name) = THEMES.iter().find(|(key, _)| comment.color.as_deref() == Some(*key)).unwrap_or(&THEMES[0]);
    (color, comment.theme.clone().unwrap_or_else(|| name.to_string()))
}

/// The state a comment is in: pending until a review or Send now sends it,
/// then sent, and resolved once a session settles it, or applied once its
/// suggestion is (task 1107).
fn comment_state(comment: &crate::item::Comment) -> &'static str {
    match (comment.applied, comment.resolved, comment.sent) {
        (Some(_), ..) => "applied",
        (None, Some(_), _) => "resolved",
        (None, None, Some(_)) => "sent",
        (None, None, None) => "pending",
    }
}

/// One entry under Comments: the comment's theme, id and state, who wrote
/// it and when, the words it is on, its text, and the replies to it.
fn comment_entry(body: &mut String, note: &Item, replies: &[&&Item]) {
    let Some(comment) = note.comment.as_deref() else { return };
    let (color, theme) = theme_of(comment);
    let state = comment_state(comment);
    let by = written_by(note).map(|by| format!("{} \u{b7} ", esc(&by))).unwrap_or_default();
    let _ = write!(
        body,
        "<li class=\"entry\" id=\"comment-{id}\" role=\"comment\" data-id=\"{id}\" data-color=\"{color}\" data-state=\"{state}\"><button class=\"entry-head\" type=\"button\"><span class=\"theme\">{}</span><span class=\"id\">{id}</span><span class=\"state\">{state}</span><span class=\"meta\">{by}{}</span></button>",
        esc(&theme),
        esc(&when(note.timestamp)),
        id = note.id
    );
    let mut told = "";
    match (&comment.quote, &comment.replacement) {
        // A suggestion: its words struck through, and the ones it puts in
        // their place (task 1107); what it says, when the person wrote
        // nothing else, is the same and is not shown again.
        (Some(quote), Some(replacement)) => {
            let put = if replacement.is_empty() { String::new() } else { format!("<ins>{}</ins>", esc(replacement)) };
            let _ = write!(body, "<div class=\"suggests\"><del>{}</del>{put}</div>", esc(&quote.exact));
            if note.description == crate::feedback::suggested(quote, replacement) {
                told = " told";
            }
        }
        (Some(quote), None) => {
            let _ = write!(body, "<blockquote class=\"said\">{}</blockquote>", esc(&quote.exact));
        }
        _ => {}
    }
    let _ = write!(body, "<div class=\"text{told}\">{}</div>", esc(&note.description));
    for reply in replies {
        let by = written_by(reply).map(|by| format!("{} \u{b7} ", esc(&by))).unwrap_or_default();
        let _ = write!(
            body,
            "<div class=\"reply\" id=\"comment-{}\" role=\"comment\"><div class=\"meta\">{by}{}</div><div class=\"text\">{}</div></div>",
            reply.id,
            esc(&when(reply.timestamp)),
            esc(&reply.description)
        );
    }
    body.push_str("</li>");
}

/// Notes and comments as one scene (task 1375), after the map: with both,
/// a tab each, the notes picked first. The notes, newest first, are AKQA's
/// latest news as rows: a small line with the kind, a title and an arrow,
/// with pills for each kind when there are two kinds or more. A row opens
/// its note in the sheet after the page. The notes themselves wait whole
/// under the rows, out of sight on a screen, so the pill finds them and
/// print shows them. Comments are the notebook of task 1213: the script
/// colors each comment's words in the text, sorts the entries into reading
/// order, keeps the ones whose words are gone, fills the filters, and makes
/// the entries the words' aria-details; a reply goes under what it answers.
/// The scene's id and name, and the scene; None when there is neither.
fn talk_scene(notes: &[&Item], comments: &[&Item], all: &ItemMap) -> Option<(&'static str, &'static str, String)> {
    let (id, name) = match (notes.is_empty(), comments.is_empty()) {
        (true, true) => return None,
        (false, true) => ("notes", "Notes"),
        (true, false) => ("comments", "Comments"),
        (false, false) => ("notes-and-comments", "Notes and comments"),
    };
    let both = !notes.is_empty() && !comments.is_empty();
    let (first, second) = two_tones(name);
    let mut out = format!("<section class=\"scene part talk\" id=\"{id}\" data-part=\"{name}\" data-mode=\"{}\">{}", mode(id), opener(&first, second.as_deref()));
    if both {
        let _ = write!(
            out,
            "<div class=\"tabs\" role=\"tablist\" aria-label=\"Notes and comments\"><button class=\"pill tab\" type=\"button\" role=\"tab\" id=\"notes-tab\" aria-controls=\"notes\" aria-selected=\"true\">Notes<sup>{}</sup></button><button class=\"pill tab\" type=\"button\" role=\"tab\" id=\"comments-tab\" aria-controls=\"comments\" aria-selected=\"false\" tabindex=\"-1\">Comments<sup>{}</sup></button></div><div class=\"panels\"><div class=\"panel\" id=\"notes\" role=\"tabpanel\" aria-labelledby=\"notes-tab\" data-name=\"Notes\">",
            notes.len(),
            comments.len()
        );
    }
    if !notes.is_empty() {
        let mut kinds: Vec<(String, usize)> = Vec::new();
        for note in notes {
            let kind = note_kind(note);
            match kinds.iter_mut().find(|(seen, _)| *seen == kind) {
                Some((_, count)) => *count += 1,
                None => kinds.push((kind, 1)),
            }
        }
        if kinds.len() > 1 {
            let _ = write!(out, "<div class=\"kinds\" role=\"toolbar\" aria-label=\"Which notes show\"><button class=\"pill\" type=\"button\" aria-pressed=\"true\">All<sup>{}</sup></button>", notes.len());
            for (kind, count) in &kinds {
                let _ = write!(out, "<button class=\"pill\" type=\"button\" data-kind=\"{kind}\" aria-pressed=\"false\">{kind}s<sup>{count}</sup></button>");
            }
            out.push_str("</div>");
        }
        let open = |note: &Item| if note.question.as_ref().is_some_and(|question| question.answer.is_none()) { " open" } else { "" };
        out.push_str("<ul class=\"rows\">");
        for note in notes {
            let kind = note_kind(note);
            let _ = write!(
                out,
                "<li data-kind=\"{kind}\"><button class=\"row{}\" type=\"button\" data-note=\"{}\" aria-haspopup=\"dialog\"><small>{kind} {} \u{b7} {}</small><strong>{}</strong><i aria-hidden=\"true\">\u{2198}</i></button></li>",
                open(note),
                note.id,
                note.id,
                esc(&when(note.timestamp)),
                esc(note_text(&note.description).0)
            );
        }
        out.push_str("</ul><div class=\"whole\">");
        for note in notes {
            let (head, rest) = note_text(&note.description);
            let kind = note_kind(note);
            let _ = write!(
                out,
                "<div class=\"note{}\" id=\"note-{}\" data-kind=\"{kind}\"><div class=\"kind\">{kind} {} \u{b7} {}</div><h3>{}</h3>",
                open(note),
                note.id,
                note.id,
                esc(&when(note.timestamp)),
                esc(head)
            );
            if let Some(answer) = note.question.as_ref().and_then(|question| question.answer.as_ref()) {
                let _ = write!(out, "<span class=\"answer\">\u{2713} {}</span>", esc(crate::menu::picked(&answer.text)));
            }
            if let Some(review) = note.review.as_deref() {
                let _ = write!(out, "<p class=\"verdict\" data-verdict=\"{}\">{}</p>", esc(&review.verdict), esc(&review_words(review, all)));
            }
            match (rest.as_str(), note.question.is_some()) {
                ("", _) => {}
                (rest, true) => {
                    let _ = write!(out, "<details><summary>What it explained and offered</summary><p class=\"text\">{}</p></details>", esc(rest));
                }
                (rest, false) => {
                    let _ = write!(out, "<p class=\"text\">{}</p>", esc(rest));
                }
            }
            out.push_str("</div>");
        }
        out.push_str("</div>");
    }
    if both {
        out.push_str("</div><div class=\"panel\" id=\"comments\" role=\"tabpanel\" aria-labelledby=\"comments-tab\" data-name=\"Comments\" hidden>");
    }
    if !comments.is_empty() {
        out.push_str("<div class=\"filters\" role=\"toolbar\" aria-label=\"Which comments show\"></div><ol class=\"comments\">");
        let known: Vec<&str> = comments.iter().filter_map(|note| note.uid.as_deref()).collect();
        let answers = |note: &Item| note.comment.as_ref().and_then(|comment| comment.reply_to.as_deref()).filter(|to| known.contains(to)).map(str::to_string);
        for note in comments.iter().filter(|note| answers(note).is_none()) {
            let replies = comments.iter().filter(|reply| reply.uid.is_some() && answers(reply).as_deref() == note.uid.as_deref());
            comment_entry(&mut out, note, &replies.collect::<Vec<_>>());
        }
        out.push_str("</ol>");
    }
    if both {
        out.push_str("</div></div>");
    }
    out.push_str("</section>");
    Some((id, name, out))
}

/// The texts an artifact kept, `earlier` in order and then its own at
/// `version`, as History lays them (task 1376): a point each on a line,
/// oldest first, under its version and the date it was made, which the
/// first one has only when it is version 1, the plan's own; then each
/// change, a text against the next, its newest first. With two changes or
/// more, a slider runs through them, oldest at its start, and the page
/// shows one at a time: the newest, the others hidden until the slider or
/// a point picks them. The points of the change shown are current.
fn texts_kept(item: &Item, earlier: &[&Earlier], version: u32) -> String {
    let count = earlier.len();
    let mut out = String::from("<div class=\"versions\"><ol class=\"line\" aria-label=\"Versions kept\">");
    for node in 0..=count {
        let number = earlier.get(node).map_or(version, |old| old.version);
        let made = match node {
            0 => (number == 1).then_some(item.timestamp),
            _ => Some(earlier[node - 1].at),
        };
        let _ = write!(
            out,
            "<li><button type=\"button\" data-node=\"{node}\" aria-current=\"{}\"><b>v{number}</b><span>{}</span></button></li>",
            node + 1 >= count,
            made.map(|at| esc(&when(at))).unwrap_or_default()
        );
    }
    out.push_str("</ol>");
    // Its autocomplete off, as the map's: Firefox gave a slider back its
    // value across the file page's reload, apart from the change the page
    // shows (task 1393).
    if count > 1 {
        let last = earlier[count - 1].version;
        let _ = write!(
            out,
            "<input class=\"scrub\" type=\"range\" autocomplete=\"off\" min=\"1\" max=\"{count}\" step=\"1\" value=\"{count}\" style=\"--p: 100%\" aria-label=\"Which change shows\" aria-valuetext=\"Version {last} \u{2192} {}\">",
            last + 1
        );
    }
    out.push_str("</div><div class=\"changed\">");
    for (at, old) in earlier.iter().enumerate().rev() {
        let new = earlier.get(at + 1).map_or(item.description.as_str(), |next| next.text.as_str());
        let _ = write!(
            out,
            "<div class=\"version\" id=\"version-{}\" data-change=\"{}\"{}><div class=\"kind\">Replaced {}</div><h3>Version {} \u{2192} {}</h3><div class=\"diff\">{}</div></div>",
            old.version,
            at + 1,
            if at + 1 < count { " hidden" } else { "" },
            esc(&when(old.at)),
            old.version,
            old.version + 1,
            diff_html(&old.text, new)
        );
    }
    out.push_str("</div>");
    out
}

/// What a review from the page needs (task 1106): the version it is made
/// on; whether the plan is still worked; the question Approve answers, the
/// newest asking to approve this version, with the answer it gives and the
/// tasks it makes; and the question Request changes answers, the newest,
/// with its answer and the version it asked about.
fn review_data(item: &Item, all: &ItemMap, version: u32) -> serde_json::Value {
    let asking = approvals_asked(item, all);
    let about = |question: &Item| question.question.as_ref().and_then(|asked| asked.approve.as_ref()).map(|approve| approve.version);
    let approve = asking.iter().find(|question| about(question) == Some(version)).map(|question| {
        let steps = item.artifact.as_deref().map(|plan| plan.steps.as_slice()).unwrap_or_default();
        let keys = question.question.as_ref().and_then(|asked| asked.approve.as_ref()).map(|approve| approve.steps.as_slice()).unwrap_or_default();
        let tasks: Vec<String> =
            keys.iter().filter_map(|key| steps.iter().find(|step| &step.key == key)).map(|step| format!("{}: {}", step.key, crate::ekko::title(&step.text))).collect();
        serde_json::json!({"id": question.id, "answers": approval_answers(question).0, "tasks": tasks})
    });
    let changes = asking.first().map(|question| serde_json::json!({"id": question.id, "answers": approval_answers(question).1, "version": about(question)}));
    let open = State::of(item).is_some_and(State::is_open);
    serde_json::json!({"version": version, "open": open, "approve": approve, "changes": changes})
}

/// A review as the page sums it up: its verdict and version, the comments
/// it sent and the question it answered (task 1106).
fn review_words(review: &Review, all: &ItemMap) -> String {
    let index = crate::ekko::uid_index(all);
    let id = |uid: &String| index.get(uid.as_str()).map_or_else(|| "one no longer here".to_string(), u32::to_string);
    let verdict = match review.verdict.as_str() {
        Review::APPROVE => "Approved",
        Review::CHANGES => "Changes requested on",
        _ => "Commented on",
    };
    let mut words = format!("{verdict} version {}", review.version);
    match review.comments.as_slice() {
        [] => {}
        [one] => words.push_str(&format!(", sending comment {}", id(one))),
        many => words.push_str(&format!(", sending comments {}", many.iter().map(id).collect::<Vec<_>>().join(", "))),
    }
    if let Some(question) = &review.answered {
        words.push_str(&format!("; it answered question {}", id(question)));
    }
    words.push('.');
    words
}

/// `text` with its first letter a capital.
fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// The mode a scene puts the page in as it reaches the window's middle
/// (task 1367): dark for the Goal, the map and What's next, as AKQA's home
/// turns black under its statement and its pages end on black, light for
/// the rest.
fn mode(id: &str) -> &'static str {
    if ["plan-goal", "map", "next"].contains(&id) { "dark" } else { "light" }
}

/// `text`'s words, escaped, each a span the page's reveal brings in after
/// the one before, as AKQA's headings come, 35 ms apart.
fn word_spans(text: &str) -> String {
    text.split_whitespace().map(|word| format!("<span class=\"w\">{}</span>", esc(word))).collect::<Vec<_>>().join(" ")
}

/// A scene's opener as AKQA sets one: in capitals, word by word, `second`
/// on a line of its own in grey. The page's, not the plan's words, even
/// where it opens a section of the plan: chrome, which the comments' words
/// skip.
fn opener(first: &str, second: Option<&str>) -> String {
    let second = second.map(|second| format!(" <span class=\"l2\">{}</span>", word_spans(second))).unwrap_or_default();
    format!("<h2 class=\"opener words\" data-chrome><span class=\"l1\">{}</span>{second}</h2>", word_spans(first))
}

/// A heading of the plan as an opener's two lines: from its "and" on, or
/// else the last third of a heading of three words or more, goes grey.
fn two_tones(heading: &str) -> (String, Option<String>) {
    if let Some((first, rest)) = heading.split_once(" and ") {
        return (first.to_string(), Some(format!("and {rest}")));
    }
    let words: Vec<&str> = heading.split_whitespace().collect();
    if words.len() < 3 {
        return (heading.to_string(), None);
    }
    let cut = words.len() - words.len().div_ceil(3);
    (words[..cut].join(" "), Some(words[cut..].join(" ")))
}

/// A box in the first scene for what the user should know first: `head`,
/// and `text`, which is HTML.
fn callout(body: &mut String, head: &str, text: &str) {
    let _ = write!(body, "<div class=\"callout\"><b>{}</b>{text}</div>", esc(head));
}

/// What waits on the user, as AKQA's black banner: `head`, `text`, which is
/// HTML, and `act` beside them, the button that answers it if the page has
/// one.
fn banner(body: &mut String, head: &str, text: &str, act: &str) {
    let _ = write!(body, "<div class=\"callout waits\"><div class=\"said\"><b>{}</b>{text}</div>{act}</div>", esc(head));
}

/// The questions open on artifact `item` or on one of its steps' tasks, in
/// the order they were asked, as the prime lists them: each with the key
/// of the step whose task it is about, if it is about one.
fn asked_here<'a>(item: &Item, all: &'a ItemMap, steps: &[Shown<'_>]) -> Vec<(&'a Item, Option<String>)> {
    let Some(uid) = item.uid.as_deref() else { return Vec::new() };
    let mut asked: Vec<(&Item, Option<String>)> = all
        .values()
        .filter(|note| !note.is_task && note.trashed.is_none() && note.stashed.is_none())
        .filter(|note| note.question.as_ref().is_some_and(|question| question.answer.is_none()))
        .filter_map(|note| {
            let on = note.attached_to.as_deref()?;
            if on == uid {
                return Some((note, None));
            }
            let step = steps.iter().find(|step| step.step.task.as_deref() == Some(on))?;
            Some((note, Some(step.step.key.clone())))
        })
        .collect();
    asked.sort_by_key(|(note, _)| (note.timestamp, note.id));
    asked
}

/// A question waiting on the user, as ekko's menu shows it (task 1110):
/// its text and explanation, its options numbered, the recommended one
/// marked, and beside them -- under them, on a narrow window -- why one
/// would pick the option in focus, an example of it and its preview, which
/// the board keeps for it. The menu opens on the recommended option, and so
/// does the card. Where the page writes, an option is picked here, or
/// several where the question allows, "Other answer…" writes another, a
/// note may go with it, and Answer records it as the menu would.
fn question_card(body: &mut String, note: &Item, step: Option<String>) {
    let posed = crate::menu::posed(note);
    let id = note.id;
    let free = posed.options.is_empty();
    let about = step.map(|key| format!(" \u{b7} about step {key}")).unwrap_or_default();
    let _ = write!(
        body,
        "<div class=\"callout waits asks\" id=\"question-{id}\" data-question=\"{}\"{}{}><div class=\"said\"><b>{}</b><p class=\"asked\">{}</p>",
        esc(&posed.uid),
        if posed.multiple { " data-multiple" } else { "" },
        if free { " data-free" } else { "" },
        esc(&format!("Waiting on you: question {id}{about}")),
        esc(posed.text.trim())
    );
    for paragraph in posed.explain.as_deref().unwrap_or_default().split("\n\n").map(str::trim).filter(|paragraph| !paragraph.is_empty()) {
        let _ = write!(body, "<p class=\"explain\">{}</p>", esc(paragraph));
    }
    if posed.multiple {
        body.push_str("<p class=\"any\">Any number of them</p>");
    }
    body.push_str("</div>");
    if !free {
        let role = if posed.multiple { "checkbox" } else { "radio" };
        let focused = posed.options.iter().position(|option| option.recommended).unwrap_or(0);
        let prose = |text: &Option<String>| text.as_deref().map(str::trim).filter(|text| !text.is_empty()).map(esc);
        let preview = |option: &crate::ops::Choice| option.preview.as_deref().filter(|preview| !preview.trim().is_empty()).map(esc);
        let aided = posed.options.iter().any(|option| prose(&option.why).is_some() || prose(&option.example).is_some() || preview(option).is_some());
        let _ = write!(
            body,
            "<div class=\"choose{}\"><div class=\"options\" role=\"{}\" aria-label=\"Answers to question {id}\">",
            if aided { " aided" } else { "" },
            if posed.multiple { "group" } else { "radiogroup" }
        );
        for (n, option) in posed.options.iter().enumerate() {
            // Picked to begin with: the recommended option, as the menu's
            // Enter takes it; where several may be, each recommended one.
            let picked = if posed.multiple { option.recommended } else { n == focused };
            let recommended = if option.recommended { "<span class=\"tag\">recommended</span>" } else { "" };
            let description = option.description.as_deref().map(str::trim).filter(|description| !description.is_empty());
            let _ = write!(
                body,
                "<button class=\"option\" type=\"button\" role=\"{role}\" aria-checked=\"{picked}\" data-n=\"{n}\" data-label=\"{}\"{}><span class=\"n\">{}</span><span class=\"label\">{}{recommended}</span>{}</button>",
                esc(option.label.trim()),
                if n == focused { "" } else { " tabindex=\"-1\"" },
                n + 1,
                esc(option.label.trim()),
                description.map(|description| format!("<span class=\"desc\">{}</span>", esc(description))).unwrap_or_default()
            );
        }
        let _ = write!(
            body,
            "<button class=\"option other writes-only\" type=\"button\" role=\"{role}\" aria-checked=\"false\" tabindex=\"-1\" data-other><span class=\"n\">{}</span><span class=\"label\">Other answer\u{2026}</span></button></div>",
            posed.options.len() + 1
        );
        if aided {
            body.push_str("<div class=\"aids\">");
            for (n, option) in posed.options.iter().enumerate() {
                let _ = write!(body, "<div class=\"aid\" data-for=\"{n}\"{}>", if n == focused { "" } else { " hidden" });
                let parts = [
                    prose(&option.why).map(|why| format!("<p class=\"why\">{why}</p>")),
                    prose(&option.example).map(|example| format!("<p class=\"example\"><span>Example:</span> {example}</p>")),
                    preview(option).map(|preview| format!("<pre class=\"preview\">{preview}</pre>")),
                ];
                let said: String = parts.into_iter().flatten().collect();
                body.push_str(if said.is_empty() { "<p class=\"none\">(no preview)</p>" } else { &said });
                body.push_str("</div>");
            }
            body.push_str("</div>");
        }
        body.push_str("</div>");
    }
    let written = if free { "Your answer" } else { "Your other answer" };
    let _ = write!(
        body,
        "<div class=\"reply writes-only\"><textarea class=\"other-text\" rows=\"2\" placeholder=\"{written}\" aria-label=\"{written} to question {id}\"{}></textarea><input class=\"note-text\" type=\"text\" placeholder=\"A note with the answer, if any\" aria-label=\"A note with the answer to question {id}\"><button class=\"pill\" type=\"button\" data-answer>Answer</button><p class=\"why-not\" role=\"status\"></p></div><p class=\"how\">Answer it <span class=\"writes-only\">here, </span>in ekko's menu, or with <code>ekko --answer {id}</code> in a terminal.</p></div>",
        if free { "" } else { " hidden" }
    );
}

/// Where an artifact stands, in the word the first scene's chip gives.
fn phase(standing: &Standing) -> &'static str {
    match standing.state {
        State::Done => "Done",
        State::Cancelled => "Cancelled",
        _ if standing.waiting.is_some() => "Waiting on you",
        _ if standing.tasks == 0 => "Draft",
        _ => "Approved",
    }
}

/// A title as AKQA sets a headline (task 1368), word by word: up to its
/// first ": ", " -- " or " \u{2014} " in ink, the rest in grey on lines of
/// its own. The title whole is its name, the mark being left out.
fn headline(title: &str) -> String {
    let cut = [": ", " -- ", " \u{2014} "].iter().filter_map(|mark| title.find(mark).map(|at| (at, at + mark.len()))).min();
    match cut {
        Some((end, start)) if end > 0 && start < title.len() => format!(
            "<h1 class=\"words\" aria-label=\"{}\"><span class=\"l1\">{}</span> <span class=\"l2\">{}</span></h1>",
            esc(title),
            word_spans(&title[..end]),
            word_spans(&title[start..])
        ),
        _ => format!("<h1 class=\"words\">{}</h1>", word_spans(title)),
    }
}

/// A note's kind, as the page heads it: an open question stands apart.
fn note_kind(note: &Item) -> String {
    if note.review.is_some() {
        return "Review".to_string();
    }
    match (&note.question, note.mark()) {
        (Some(question), _) if question.answer.is_none() => "Open question".to_string(),
        (Some(_), _) => "Question".to_string(),
        (None, Some(mark)) => capitalized(mark),
        (None, None) => "Note".to_string(),
    }
}

/// A note's text as the page sets it: its first sentence as its title, and
/// the rest -- on the board, often the rest of a long first line -- then
/// the lines after it.
fn note_text(text: &str) -> (&str, String) {
    let text = text.trim();
    let (first, after) = text.split_once('\n').unwrap_or((text, ""));
    let first = first.trim();
    let (head, more) = match sentence_end(first, true) {
        Some(end) => (&first[..end], first[end..].trim_start()),
        None => (first, ""),
    };
    let rest: Vec<&str> = [more, after.trim()].into_iter().filter(|part| !part.is_empty()).collect();
    (head, rest.join("\n"))
}

/// `text` as the end of an id: its letters and digits in lower case, each
/// run of anything else one dash between them.
fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-');
    if out.is_empty() { "section".to_string() } else { out.to_string() }
}

/// `id`, or past it the first of `id-2`, `id-3`... not among `taken`, which
/// it joins.
fn unique(taken: &mut Vec<String>, id: &str) -> String {
    let mut made = id.to_string();
    let mut count = 1;
    while taken.contains(&made) {
        count += 1;
        made = format!("{id}-{count}");
    }
    taken.push(made.clone());
    made
}

/// The plan's text split at its `## ` headings, outside code fences: each
/// heading with the Markdown under it, and what comes before the first under
/// an empty heading.
fn sections(plan: &str) -> Vec<(String, String)> {
    section_ranges(plan).into_iter().map(|(heading, range)| (heading, plan[range].lines().map(|line| format!("{line}\n")).collect())).collect()
}

/// `sections`, each with the bytes of `plan` its Markdown is, its heading's
/// line left out: where a suggestion from the page applies (task 1107).
pub(crate) fn section_ranges(plan: &str) -> Vec<(String, std::ops::Range<usize>)> {
    let mut sections = vec![(String::new(), 0..0)];
    let mut fenced = false;
    let mut at = 0;
    for line in plan.split_inclusive('\n') {
        let end = at + line.len();
        let bare = line.strip_suffix('\n').map_or(line, |line| line.strip_suffix('\r').unwrap_or(line));
        let trimmed = bare.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
        }
        match bare.strip_prefix("## ") {
            Some(heading) if !fenced => sections.push((heading.trim().to_string(), end..end)),
            _ => {
                if let Some((_, range)) = sections.last_mut() {
                    range.end = end;
                }
            }
        }
        at = end;
    }
    sections.retain(|(heading, range)| !heading.is_empty() || !plan[range.clone()].trim().is_empty());
    sections
}

/// The width and height of a step on the map, and the room between them, in
/// pixels.
const NODE: (usize, usize) = (200, 92);
const NODE_GAP: (usize, usize) = (64, 28);

/// Where each step sits on the map: its column, one past the latest of the
/// steps it waits on, and its row in that column.
fn layout(steps: &[Shown]) -> Vec<(usize, usize)> {
    let mut placed: Vec<(usize, usize)> = Vec::with_capacity(steps.len());
    let mut rows: Vec<usize> = Vec::new();
    for (at, step) in steps.iter().enumerate() {
        let column = step
            .step
            .after
            .iter()
            .filter_map(|key| steps[..at].iter().position(|earlier| &earlier.step.key == key))
            .map(|earlier| placed[earlier].0 + 1)
            .max()
            .unwrap_or(0);
        if rows.len() <= column {
            rows.resize(column + 1, 0);
        }
        placed.push((column, rows[column]));
        rows[column] += 1;
    }
    placed
}

/// The map: a box per step, placed by `layout` with each column centred on
/// the tallest, and a curve from each step to every step that waits on it.
/// Each box says its stage, its column counted from 1, which the map plays
/// in order (task 1374); each curve says the steps it joins and the stage
/// of the later, and is one long, so that a dash draws it.
fn map(steps: &[Shown]) -> String {
    let (width, height) = NODE;
    let (gap_x, gap_y) = NODE_GAP;
    let placed = layout(steps);
    let columns = placed.iter().map(|(column, _)| column + 1).max().unwrap_or(0);
    let mut rows = vec![0; columns];
    for (column, _) in &placed {
        rows[*column] += 1;
    }
    let tallest = rows.iter().copied().max().unwrap_or(0);
    let at = |step: usize| {
        let (column, row) = placed[step];
        let offset = (tallest - rows[column]) * (height + gap_y) / 2;
        (column * (width + gap_x), offset + row * (height + gap_y))
    };
    let (full_width, full_height) = (columns * (width + gap_x) - gap_x, tallest * (height + gap_y) - gap_y);
    let mut out = format!(
        "<div class=\"map\" style=\"width:{full_width}px;height:{full_height}px\"><svg viewBox=\"0 0 {full_width} {full_height}\" width=\"{full_width}\" height=\"{full_height}\" aria-hidden=\"true\"><defs><marker id=\"arrow\" viewBox=\"0 0 10 10\" refX=\"9\" refY=\"5\" markerWidth=\"7\" markerHeight=\"7\" orient=\"auto-start-reverse\"><path class=\"arrowhead\" d=\"M0 0 L10 5 L0 10 z\"/></marker></defs>"
    );
    for (later, step) in steps.iter().enumerate() {
        for key in &step.step.after {
            let Some(earlier) = steps[..later].iter().position(|earlier| &earlier.step.key == key) else { continue };
            let ((x1, y1), (x2, y2)) = (at(earlier), at(later));
            let (from_x, from_y, to_x, to_y) = (x1 + width, y1 + height / 2, x2.saturating_sub(2), y2 + height / 2);
            let middle = (from_x + to_x) / 2;
            let _ = write!(
                out,
                "<path class=\"edge\" data-from=\"{}\" data-to=\"{}\" data-stage=\"{}\" pathLength=\"1\" d=\"M{from_x} {from_y} C {middle} {from_y}, {middle} {to_y}, {to_x} {to_y}\" marker-end=\"url(#arrow)\"/>",
                esc(key),
                esc(&step.step.key),
                placed[later].0 + 1
            );
        }
    }
    out.push_str("</svg>");
    for (index, step) in steps.iter().enumerate() {
        let (x, y) = at(index);
        let task = step.task.map(|id| format!("task {id}")).unwrap_or_default();
        let _ = write!(
            out,
            "<button class=\"node {}\" data-step=\"{}\" data-stage=\"{}\" style=\"left:{x}px;top:{y}px\"><span class=\"k\">{}</span><span class=\"t\">{}</span><span class=\"s\"><span><span class=\"dot {}\"></span>{}</span><span>{task}</span></span></button>",
            step.class,
            esc(&step.step.key),
            placed[index].0 + 1,
            esc(&step.step.key),
            esc(step.title),
            step.class,
            esc(&step.state)
        );
    }
    out.push_str("</div>");
    out
}

/// The lines of `old` and `new`, each marked kept (' '), taken out ('-') or
/// put in ('+'), by their longest common subsequence.
fn diff<'a>(old: &'a str, new: &'a str) -> Vec<(char, &'a str)> {
    let (old, new): (Vec<&str>, Vec<&str>) = (old.lines().collect(), new.lines().collect());
    let (n, m) = (old.len(), new.len());
    let mut common = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            common[i][j] = if old[i] == new[j] { common[i + 1][j + 1] + 1 } else { common[i + 1][j].max(common[i][j + 1]) };
        }
    }
    let (mut i, mut j, mut lines) = (0, 0, Vec::with_capacity(n.max(m)));
    while i < n && j < m {
        if old[i] == new[j] {
            lines.push((' ', old[i]));
            i += 1;
            j += 1;
        } else if common[i + 1][j] >= common[i][j + 1] {
            lines.push(('-', old[i]));
            i += 1;
        } else {
            lines.push(('+', new[j]));
            j += 1;
        }
    }
    lines.extend(old[i..].iter().map(|line| ('-', *line)));
    lines.extend(new[j..].iter().map(|line| ('+', *line)));
    lines
}

/// Lines kept around a change, on each side; the others fold into a count.
const DIFF_CONTEXT: usize = 2;

/// `diff` as the page shows it: the changed lines, with `DIFF_CONTEXT` kept
/// lines around each run of them.
fn diff_html(old: &str, new: &str) -> String {
    let lines = diff(old, new);
    let changed: Vec<usize> = lines.iter().enumerate().filter(|(_, (mark, _))| *mark != ' ').map(|(at, _)| at).collect();
    if changed.is_empty() {
        return "<div class=\"hunk\">The text is the same: only the steps changed.</div>".to_string();
    }
    let near = |at: usize| changed.iter().any(|change| change.abs_diff(at) <= DIFF_CONTEXT);
    let mut out = String::new();
    let mut folded = 0;
    for (at, (mark, line)) in lines.iter().enumerate() {
        if *mark == ' ' && !near(at) {
            folded += 1;
            continue;
        }
        if folded > 0 {
            let _ = write!(out, "<div class=\"hunk\">\u{2026} {}</div>", plural(folded, "line kept"));
            folded = 0;
        }
        let class = match mark {
            '+' => "add",
            '-' => "del",
            _ => "ctx",
        };
        let _ = write!(out, "<div class=\"{class}\">{mark} {}</div>", esc(line));
    }
    if folded > 0 {
        let _ = write!(out, "<div class=\"hunk\">\u{2026} {}</div>", plural(folded, "line kept"));
    }
    out
}

/// `count` of `word`, in the plural past one: "1 step", "3 steps".
fn plural(count: usize, word: &str) -> String {
    match (count, word.split_once(' ')) {
        (1, _) => format!("1 {word}"),
        (_, Some((noun, rest))) => format!("{count} {noun}s {rest}"),
        (_, None) => format!("{count} {word}s"),
    }
}

/// A moment on the board, as the page shows it, in local time.
fn when(millis: i64) -> String {
    chrono::DateTime::from_timestamp_millis(millis).map(|at| at.with_timezone(&chrono::Local).format("%b %d, %H:%M").to_string()).unwrap_or_default()
}

/// The page of the artifact `uid` under the board's directory `dir`.
pub fn page_path(dir: &Path, uid: &str) -> PathBuf {
    dir.join(PAGES).join(format!("{uid}.html"))
}

/// Writes the page of artifact `item` and the script holding its version,
/// unless both already hold this version: the page first, then the script,
/// each by rename, so a page that reloads on the new version reads the new
/// page (task 1059). The fonts go beside them first, where they are not
/// yet. The page's path.
pub fn write(dir: &Path, item: &Item, all: &ItemMap, folder: Option<&Path>) -> std::io::Result<PathBuf> {
    let uid = item.uid.as_deref().ok_or_else(|| std::io::Error::other("an artifact without a uid has no page"))?;
    let pages = dir.join(PAGES);
    std::fs::create_dir_all(&pages)?;
    write_fonts(&pages)?;
    let (html, version) = page(item, all, folder);
    let script = script(&version);
    let path = page_path(dir, uid);
    let script_path = pages.join(format!("{uid}.js"));
    if path.exists() && std::fs::read_to_string(&script_path).is_ok_and(|kept| kept == script) {
        return Ok(path);
    }
    replace(&path, html.as_bytes())?;
    replace(&script_path, script.as_bytes())?;
    Ok(path)
}

/// The fonts a page draws with, cut by scripts/fonts.sh from Inter and
/// Source Serif 4 (task 1152): the family the page's style names, the
/// style, the file's name before its hash, and the bytes.
const FONTS: [(&str, &str, &str, &[u8]); 3] = [
    ("Inter", "normal", "inter", include_bytes!("../assets/fonts/inter.woff2")),
    ("Ekko Serif", "normal", "ekko-serif", include_bytes!("../assets/fonts/ekko-serif.woff2")),
    ("Ekko Serif", "italic", "ekko-serif-italic", include_bytes!("../assets/fonts/ekko-serif-italic.woff2")),
];

/// The folder of the fonts, beside the pages and under the address of each.
pub const FONTS_DIR: &str = "fonts";

/// The fonts' licence, which goes where they go, under this name.
pub const FONT_LICENSE: (&str, &str) = ("OFL.txt", include_str!("../assets/fonts/OFL.txt"));

/// The file each font is beside the pages, and under the server's address
/// of each: its name and its bytes' hash, so a font cut again is a file of
/// its own, which a browser may keep for good.
pub fn font_files() -> &'static [(String, &'static [u8])] {
    static FILES: std::sync::OnceLock<Vec<(String, &'static [u8])>> = std::sync::OnceLock::new();
    FILES.get_or_init(|| FONTS.iter().map(|(_, _, name, bytes)| (format!("{name}-{:08x}.woff2", fnv(bytes) >> 32), *bytes)).collect())
}

/// The page's rules for its fonts, which load from the files beside it:
/// from the disk beside a file page, as rustdoc's do (Firefox allows a
/// page opened from a file a font in its folder or below, measured in
/// Firefox 157), and from the server beside a served one. A font loads only
/// once something on the page is set in it.
fn font_faces() -> String {
    let mut out = String::new();
    for ((family, style, _, _), (file, _)) in FONTS.iter().zip(font_files()) {
        let _ = writeln!(
            out,
            "@font-face {{ font-family: \"{family}\"; src: url(\"{FONTS_DIR}/{file}\") format(\"woff2\"); font-weight: 400 700; font-style: {style}; font-display: block; }}"
        );
    }
    out
}

/// Writes the fonts and their licence into `fonts` under the folder of the
/// pages, each that is not there whole yet.
fn write_fonts(pages: &Path) -> std::io::Result<()> {
    let fonts = pages.join(FONTS_DIR);
    let (license, text) = FONT_LICENSE;
    let files = font_files().iter().map(|(name, bytes)| (name.as_str(), *bytes)).chain([(license, text.as_bytes())]);
    for (name, bytes) in files {
        let path = fonts.join(name);
        if std::fs::metadata(&path).is_ok_and(|meta| meta.len() == bytes.len() as u64) {
            continue;
        }
        std::fs::create_dir_all(&fonts)?;
        replace(&path, bytes)?;
    }
    Ok(())
}

/// Rewrites every page under the board's directory `dir` whose artifact is
/// on the board `all` and shows something else now. Only pages someone
/// opened exist, so a board nobody reads this way pays a missing folder.
/// Best effort: the write that called it has landed already.
pub fn refresh(dir: &Path, all: &ItemMap, folder: Option<&Path>) {
    let Ok(entries) = std::fs::read_dir(dir.join(PAGES)) else { return };
    let uids: Vec<String> = entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.strip_suffix(".html").map(str::to_string))
        .collect();
    if uids.is_empty() {
        return;
    }
    let index = crate::ekko::uid_index(all);
    for uid in uids {
        if let Some(item) = index.get(uid.as_str()).and_then(|id| all.get(id)).filter(|item| item.artifact.is_some()) {
            let _ = write(dir, item, all, folder);
        }
    }
}

/// The script beside a page that holds its `version`, which the page loads
/// every `POLL_MS` as a file, or each time its stream of /events opens when
/// served (task 1104), and reloads on when it changed: from the file next
/// to a written page, or from `ekko serve` (task 1102).
pub fn script(version: &str) -> String {
    format!("ekkoArtifact(\"{version}\");\n")
}

/// Opens `page`, a file or an address, in the default browser, as `cargo doc
/// --open` does, without waiting for it.
pub fn open(page: impl AsRef<std::ffi::OsStr>) -> std::io::Result<()> {
    open_with(if cfg!(target_os = "macos") { "open" } else { "xdg-open" }, page.as_ref())
}

/// Runs `opener page` in the background of a shell that exits at once, so
/// the opener, and a browser it has to start, lose their parent and leave
/// the process tree of whoever ran ekko: a browser started from a session's
/// command is then the user's (task 1103). An opener the shell does not find
/// is an error, as it is to a spawn.
fn open_with(opener: &str, page: &std::ffi::OsStr) -> std::io::Result<()> {
    std::process::Command::new("sh")
        .arg("-c")
        .arg("command -v \"$0\" >/dev/null || exit 127; \"$0\" \"$1\" >/dev/null 2>&1 &")
        .arg(opener)
        .arg(page)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .and_then(|status| if status.success() { Ok(()) } else { Err(std::io::Error::other(format!("{opener} does not run: sh exited with {status}"))) })
}

/// Replaces `path` with `content` by rename from a file beside it.
fn replace(path: &Path, content: &[u8]) -> std::io::Result<()> {
    let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let temp = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    std::fs::write(&temp, content)?;
    std::fs::rename(&temp, path)
}

/// The Markdown a plan is written in: CommonMark, with GitHub's tables,
/// strikethrough and task lists.
pub(crate) fn markdown_options() -> pulldown_cmark::Options {
    use pulldown_cmark::Options;
    Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS
}

/// The plan's Markdown as the events the page renders, each run of text
/// one event. Raw HTML in it is text, not run: the page is the board's, and
/// a script a plan carried would run there. For the same reason a link
/// keeps its address only when `linkable`, and is its words alone
/// otherwise. An image is a link to its source, never loaded, since the
/// page loads nothing a plan names (task 1097).
fn events(text: &str) -> Vec<pulldown_cmark::Event<'_>> {
    use pulldown_cmark::{Event, LinkType, Parser, Tag, TagEnd, TextMergeStream};
    let options = markdown_options();
    let mut events = Vec::new();
    // Each link and image open here, innermost last: whether it kept its
    // tag, and a kept image's source, which names it when it has no words.
    let mut open = Vec::new();
    for event in TextMergeStream::new(Parser::new_ext(text, options)) {
        match event {
            Event::Html(raw) | Event::InlineHtml(raw) => events.push(Event::Text(raw)),
            // One inside a kept link would nest a link in a link.
            Event::Start(Tag::Link { link_type, dest_url, title, id }) => {
                let keep = !open.iter().any(|(kept, _)| *kept) && (link_type == LinkType::Email || linkable(&dest_url));
                open.push((keep, None));
                if keep {
                    events.push(Event::Start(Tag::Link { link_type, dest_url, title, id }));
                }
            }
            Event::Start(Tag::Image { link_type, dest_url, title, id }) => {
                let keep = !open.iter().any(|(kept, _)| *kept) && linkable(&dest_url);
                open.push((keep, keep.then(|| dest_url.clone())));
                if keep {
                    events.push(Event::Start(Tag::Link { link_type, dest_url, title, id }));
                }
            }
            Event::End(TagEnd::Link | TagEnd::Image) => {
                let Some((true, source)) = open.pop() else { continue };
                let bare = matches!(events.last(), Some(Event::Start(Tag::Link { .. })));
                if let Some(source) = source.filter(|_| bare) {
                    events.push(Event::Text(source));
                }
                events.push(Event::End(TagEnd::Link));
            }
            other => events.push(other),
        }
    }
    events
}

/// The plan's words as the page shows them, each run of white space one
/// space and every block apart from the next: where a comment's quote,
/// taken from the page, is found again (task 1105).
pub fn shown_words(text: &str) -> String {
    use pulldown_cmark::{Event, Tag, TagEnd};
    let mut out = String::new();
    for event in events(text) {
        match event {
            Event::Text(words) | Event::Code(words) => out.push_str(&words),
            Event::Start(Tag::Emphasis | Tag::Strong | Tag::Strikethrough | Tag::Link { .. } | Tag::Image { .. })
            | Event::End(TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link | TagEnd::Image) => {}
            Event::Start(_) | Event::End(_) | Event::SoftBreak | Event::HardBreak | Event::Rule => out.push(' '),
            _ => {}
        }
    }
    collapsed(&out)
}

/// `text` with each run of white space one space, and none at either end.
pub fn collapsed(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The plan's Markdown as HTML, by the rules of `events`.
fn markdown(text: &str) -> String {
    let mut out = String::new();
    pulldown_cmark::html::push_html(&mut out, events(text).into_iter());
    out
}

/// One line of the plan's Markdown as inline HTML, by the rules of
/// `events`: a paragraph's insides, or the text's own words, escaped, when
/// it is anything but one paragraph.
fn inline(text: &str) -> String {
    let html = markdown(text);
    html.strip_prefix("<p>").and_then(|rest| rest.strip_suffix("</p>\n")).filter(|inside| !inside.contains("<p>")).map_or_else(|| esc(text), str::to_string)
}

/// The Goal cut as the page opens it: its first sentence, the statement,
/// as inline HTML, when the Goal opens with a paragraph, and the rest as it
/// is written.
fn goal_split(text: &str) -> (Option<String>, String) {
    let (mut statement, mut rest) = (String::new(), String::new());
    match first_sentence(events(text)) {
        Ok((sentence, after)) => {
            pulldown_cmark::html::push_html(&mut statement, sentence.into_iter());
            pulldown_cmark::html::push_html(&mut rest, after.into_iter());
            (Some(statement), rest)
        }
        Err(events) => {
            pulldown_cmark::html::push_html(&mut rest, events.into_iter());
            (None, rest)
        }
    }
}

/// The Goal as its scene holds it (task 1369): the statement in large
/// serif, each word a span the scroll lights, held in place with the rest
/// of the Goal after it; its label and the hairline that says how far the
/// statement is lit are the page's, chrome.
fn goal_scene(text: &str) -> String {
    let (statement, rest) = goal_split(text);
    let mut out = String::from("<div class=\"held\"><div class=\"label\" data-chrome>Goal</div>");
    if let Some(statement) = statement {
        let _ = write!(out, "<p class=\"statement\">{}</p>", lit_words(&statement));
    }
    let _ = write!(out, "<div class=\"goal-rest\">{rest}</div><div class=\"progress\" data-chrome aria-hidden=\"true\"><i></i></div></div>");
    out
}

/// `html`'s words, outside its tags and its code, each a span the Goal's
/// scroll lights; a code span is lit whole, as one word.
fn lit_words(html: &str) -> String {
    let mut out = String::new();
    let mut code = 0usize;
    let mut rest = html;
    while !rest.is_empty() {
        if rest.starts_with('<') {
            let end = rest.find('>').map_or(rest.len(), |at| at + 1);
            let tag = &rest[..end];
            if tag.starts_with("<code") {
                code += 1;
            } else if tag.starts_with("</code") {
                code = code.saturating_sub(1);
            }
            out.push_str(tag);
            rest = &rest[end..];
            continue;
        }
        let end = rest.find('<').unwrap_or(rest.len());
        let text = &rest[..end];
        if code > 0 {
            out.push_str(text);
        } else {
            let mut word = String::new();
            for c in text.chars().chain(std::iter::once(' ')) {
                if !c.is_whitespace() {
                    word.push(c);
                    continue;
                }
                if !word.is_empty() {
                    let _ = write!(out, "<span class=\"w\">{word}</span>");
                    word.clear();
                }
                out.push(c);
            }
            out.pop();
        }
        rest = &rest[end..];
    }
    out
}

/// A text's events cut in two: a sentence's, and those after it.
type Split<'a> = (Vec<pulldown_cmark::Event<'a>>, Vec<pulldown_cmark::Event<'a>>);

/// The longest first sentence, in characters, a card sets large as its
/// lead.
const LEAD_LONGEST: usize = 220;

/// What is known as its scene holds it (task 1370): each group, a
/// paragraph and the list after it, behind a tab named by the paragraph's
/// first words, and each item of the list a card on a track, its first
/// sentence its lead, opening in place to the whole of it. The plan's
/// elements keep their order, so its words stay where comments find them;
/// the tabs, the cards' numbers and buttons and the track's controls are
/// chrome. A card's id says its group and its place, `<id>-card-2-3`, for
/// a jump to it (task 1378). None when the section holds no list.
fn known_scene(text: &str, id: &str) -> Option<String> {
    use pulldown_cmark::{Event, Tag};
    enum Piece<'a> {
        Loose(Vec<Event<'a>>),
        Group(Option<Vec<Event<'a>>>, Option<u64>, Vec<Vec<Event<'a>>>),
    }
    let list = |block: &Vec<Event>| matches!(block.first(), Some(Event::Start(Tag::List(_))));
    let mut pieces = Vec::new();
    let mut heads = Vec::new();
    let mut blocks = blocks(text).into_iter().peekable();
    while let Some(block) = blocks.next() {
        let (lead, items) = if list(&block) {
            (None, block)
        } else if matches!(block.first(), Some(Event::Start(Tag::Paragraph))) && blocks.peek().is_some_and(list) {
            (Some(block), blocks.next().unwrap_or_default())
        } else {
            pieces.push(Piece::Loose(block));
            continue;
        };
        let (start, items) = list_items(items);
        heads.push((tab_name(lead.as_deref()), items.len()));
        pieces.push(Piece::Group(lead, start, items));
    }
    if heads.is_empty() {
        return None;
    }
    let tabbed = heads.len() > 1;
    let mut out = String::new();
    let mut n = 0;
    for piece in pieces {
        let (lead, start, items) = match piece {
            Piece::Loose(block) => {
                pulldown_cmark::html::push_html(&mut out, block.into_iter());
                continue;
            }
            Piece::Group(lead, start, items) => (lead, start, items),
        };
        n += 1;
        if tabbed && n == 1 {
            out.push_str("<div class=\"tabs\" role=\"tablist\" aria-label=\"What is known\" data-chrome>");
            for (at, (name, count)) in heads.iter().enumerate() {
                let (tab, chosen) = (at + 1, at == 0);
                let away = if chosen { "" } else { " tabindex=\"-1\"" };
                let _ = write!(out, "<button class=\"pill tab\" type=\"button\" role=\"tab\" id=\"{id}-tab-{tab}\" aria-controls=\"{id}-group-{tab}\" aria-selected=\"{chosen}\"{away}>{}<sup>{count}</sup></button>", esc(name));
            }
            out.push_str("</div>");
        }
        if tabbed {
            let _ = write!(out, "<div class=\"group\" id=\"{id}-group-{n}\" role=\"tabpanel\" aria-labelledby=\"{id}-tab-{n}\"{}>", if n > 1 { " hidden" } else { "" });
        } else {
            out.push_str("<div class=\"group\">");
        }
        if let Some(lead) = lead {
            pulldown_cmark::html::push_html(&mut out, lead.into_iter());
        }
        let (tag, first) = list_tag(start);
        let _ = write!(out, "<{tag} class=\"track\"{first} tabindex=\"0\" aria-label=\"{}\">", esc(&heads[n - 1].0));
        let of = items.len();
        for (at, item) in items.into_iter().enumerate() {
            let _ = write!(out, "<li class=\"card\" id=\"{id}-card-{n}-{}\"><span class=\"num\" data-chrome>{:02} / {of:02}</span><div class=\"body\">", at + 1, at + 1);
            pulldown_cmark::html::push_html(&mut out, with_lead(item).into_iter());
            out.push_str("</div><button class=\"more\" type=\"button\" aria-expanded=\"false\" data-chrome><span>Read</span> <i aria-hidden=\"true\">\u{2198}</i></button></li>");
        }
        let _ = write!(out, "</{tag}></div>");
        // The track's controls, under the last group's: the card at the
        // track's start, of how many, and the way back and on.
        if n == heads.len() {
            let count = heads[0].1;
            let _ = write!(out, "<div class=\"controls\" data-chrome><span class=\"count\">01 / {count:02}</span><span class=\"grow\"></span>{}</div>", arrows(true, count < 2));
        }
    }
    Some(out)
}

/// The way back and on under a track, a stack or the stage, as round
/// buttons: Back disabled at the `first`, Next at the `last`.
fn arrows(first: bool, last: bool) -> String {
    let off = |end: bool| if end { " disabled" } else { "" };
    format!(
        "<button class=\"round\" type=\"button\" data-by=\"-1\" aria-label=\"Back\"{}><svg viewBox=\"0 0 24 24\" aria-hidden=\"true\"><path d=\"M14.5 5.5L8 12l6.5 6.5\"/></svg></button><button class=\"round\" type=\"button\" data-by=\"1\" aria-label=\"Next\"{}><svg viewBox=\"0 0 24 24\" aria-hidden=\"true\"><path d=\"M9.5 5.5L16 12l-6.5 6.5\"/></svg></button>",
        off(first),
        off(last)
    )
}

/// A list's tag as the plan wrote it, and the number an ordered one starts
/// at, as an attribute, when it is not 1.
fn list_tag(start: Option<u64>) -> (&'static str, String) {
    let first = start.filter(|first| *first != 1).map(|first| format!(" start=\"{first}\"")).unwrap_or_default();
    (if start.is_some() { "ol" } else { "ul" }, first)
}

/// A section cut around one of its lists: the blocks before it, the
/// number it starts at and its items, as `list_items` gives them, and the
/// blocks after it.
type Around<'a> = (Vec<Vec<pulldown_cmark::Event<'a>>>, Option<u64>, Vec<Vec<pulldown_cmark::Event<'a>>>, Vec<Vec<pulldown_cmark::Event<'a>>>);

/// `text` cut around its first list of `least` items or more; None without
/// one.
fn around_list(text: &str, least: usize) -> Option<Around<'_>> {
    use pulldown_cmark::{Event, Tag};
    let mut before = blocks(text);
    let at = before.iter().position(|block| matches!(block.first(), Some(Event::Start(Tag::List(_)))) && list_items(block.clone()).1.len() >= least)?;
    let after = before.split_off(at + 1);
    let (start, items) = list_items(before.pop()?);
    Some((before, start, items, after))
}

/// The Design as its scene holds it (task 1371): its first list of three
/// points or more told a point at a time, beside a numeral held in place
/// that turns to the point at the window's middle, with how many there are
/// and a tick for each; what comes before the list and after it as it is
/// written. The numeral, the count and the ticks are chrome. The scene and
/// its number of points; None without such a list.
fn design_scene(text: &str) -> Option<(String, usize)> {
    let (before, start, items, after) = around_list(text, 3)?;
    let mut out = String::new();
    for block in before {
        pulldown_cmark::html::push_html(&mut out, block.into_iter());
    }
    let of = items.len();
    let _ = write!(out, "<div class=\"story\"><div class=\"hold\" data-chrome><div class=\"numeral\" aria-hidden=\"true\"><span>01</span></div><div class=\"of\" aria-hidden=\"true\">of {of:02}</div><div class=\"ticks\" role=\"group\" aria-label=\"Points\">");
    for n in 1..=of {
        let _ = write!(out, "<button type=\"button\" aria-label=\"Point {n}\" aria-current=\"{}\"></button>", n == 1);
    }
    let (tag, first) = list_tag(start);
    let _ = write!(out, "</div></div><{tag} class=\"points\"{first}>");
    for (n, item) in items.into_iter().enumerate() {
        out.push_str(if n == 0 { "<li class=\"point on\">" } else { "<li class=\"point\">" });
        pulldown_cmark::html::push_html(&mut out, with_lead(item).into_iter());
        out.push_str("</li>");
    }
    let _ = write!(out, "</{tag}></div>");
    for block in after {
        pulldown_cmark::html::push_html(&mut out, block.into_iter());
    }
    Some((out, of))
}

/// The Risks as their scene holds them (task 1372): the first list of two
/// items or more a stack of sheets, one in front and the next two showing
/// under it, each saying whether it is a risk or an open question; under
/// the stack how far along it is, a switch that lays every sheet out at
/// once, and the way back and on. What comes before the list and after it
/// stays as written. A sheet's distance from the one in front, --d, sets
/// it back; those behind are inert until they come in front. The sheets'
/// numbers and kinds and the controls are chrome. A sheet's id says its
/// place, `<id>-risk-2`, for a jump to it (task 1378). None without such a
/// list.
fn risks_scene(text: &str, id: &str) -> Option<String> {
    let (before, start, items, after) = around_list(text, 2)?;
    let mut out = String::new();
    for block in before {
        pulldown_cmark::html::push_html(&mut out, block.into_iter());
    }
    let (tag, first) = list_tag(start);
    let of = items.len();
    let _ = write!(out, "<{tag} class=\"stack\"{first} tabindex=\"0\" aria-label=\"Risks and open questions\">");
    for (at, item) in items.into_iter().enumerate() {
        let (ask, kind) = if asks(&item) { (" ask", "Open question") } else { ("", "Risk") };
        let behind = if at > 0 { " inert" } else { "" };
        let _ = write!(out, "<li class=\"sheet{ask}\" id=\"{id}-risk-{}\" style=\"--d: {at}\"{behind}><div class=\"num\" data-chrome><span>{:02} / {of:02}</span><span>{kind}</span></div><div class=\"body\">", at + 1, at + 1);
        pulldown_cmark::html::push_html(&mut out, with_lead(item).into_iter());
        out.push_str("</div></li>");
    }
    let _ = write!(
        out,
        "</{tag}><div class=\"controls\" data-chrome><span class=\"count\">01 / {of:02}</span><span class=\"grow\"></span><button class=\"toggle\" type=\"button\" role=\"switch\" aria-checked=\"false\"><i aria-hidden=\"true\"></i><span>All at once</span></button>{}</div>",
        arrows(true, false)
    );
    for block in after {
        pulldown_cmark::html::push_html(&mut out, block.into_iter());
    }
    Some(out)
}

/// Whether an item of the Risks asks rather than warns: its text opens
/// with the word Open, as "Open, for the user:" does, or its first
/// sentence ends with a question mark.
fn asks(item: &[pulldown_cmark::Event<'_>]) -> bool {
    use pulldown_cmark::{Event, Tag, TagEnd};
    let mut text = String::new();
    for event in item {
        match event {
            Event::Text(words) | Event::Code(words) => text.push_str(words),
            Event::SoftBreak | Event::HardBreak => text.push(' '),
            Event::Start(Tag::Paragraph | Tag::Emphasis | Tag::Strong | Tag::Strikethrough | Tag::Link { .. }) | Event::End(TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link) => {}
            Event::Start(_) | Event::End(_) => break,
            _ => {}
        }
    }
    let text = collapsed(&text);
    let sentence = sentence_end(&text, true).map_or(text.as_str(), |end| &text[..end]);
    text.split(|c: char| !c.is_alphanumeric()).next() == Some("Open") || sentence.trim_end_matches(['"', '\'', ')', ']', '\u{201d}', '\u{2019}']).ends_with('?')
}

/// A section's Markdown cut into its blocks, by the rules of `events`:
/// each block's events from its start to its end.
fn blocks(text: &str) -> Vec<Vec<pulldown_cmark::Event<'_>>> {
    use pulldown_cmark::Event;
    let (mut blocks, mut depth): (Vec<Vec<Event>>, usize) = (Vec::new(), 0);
    for event in events(text) {
        if depth == 0 {
            blocks.push(Vec::new());
        }
        match event {
            Event::Start(_) => depth += 1,
            Event::End(_) => depth = depth.saturating_sub(1),
            _ => {}
        }
        if let Some(block) = blocks.last_mut() {
            block.push(event);
        }
    }
    blocks
}

/// A list's events cut into the number it starts at, for an ordered one,
/// and each item's events inside it.
fn list_items(list: Vec<pulldown_cmark::Event<'_>>) -> (Option<u64>, Vec<Vec<pulldown_cmark::Event<'_>>>) {
    use pulldown_cmark::{Event, Tag, TagEnd};
    let (mut start, mut items, mut depth) = (None, Vec::new(), 0usize);
    for event in list {
        match &event {
            Event::Start(Tag::List(first)) if depth == 0 => start = *first,
            Event::Start(Tag::Item) if depth == 1 => items.push(Vec::new()),
            Event::End(TagEnd::Item) if depth == 2 => {}
            Event::End(TagEnd::List(_)) if depth == 1 => {}
            _ => {
                match event {
                    Event::Start(_) => depth += 1,
                    Event::End(_) => depth -= 1,
                    _ => {}
                }
                if let Some(item) = items.last_mut() {
                    item.push(event);
                }
                continue;
            }
        }
        match event {
            Event::Start(_) => depth += 1,
            _ => depth -= 1,
        }
    }
    (start, items)
}

/// A group of findings' name on its tab: the first words of the paragraph
/// that leads it, up to a comma, a colon or a bracket, shortened at a word
/// past 36 characters; Findings when no paragraph leads it.
fn tab_name(lead: Option<&[pulldown_cmark::Event<'_>]>) -> String {
    use pulldown_cmark::Event;
    let words: String = lead
        .unwrap_or_default()
        .iter()
        .filter_map(|event| match event {
            Event::Text(text) | Event::Code(text) => Some(text.as_ref()),
            Event::SoftBreak | Event::HardBreak => Some(" "),
            _ => None,
        })
        .collect();
    let name = collapsed(words.split([',', ':', '(']).next().unwrap_or_default());
    if name.is_empty() {
        return "Findings".to_string();
    }
    if name.chars().count() <= 36 {
        return name;
    }
    let mut short = String::new();
    for word in name.split(' ') {
        if short.chars().count() + word.chars().count() >= 34 {
            break;
        }
        if !short.is_empty() {
            short.push(' ');
        }
        short.push_str(word);
    }
    if short.is_empty() {
        short = name.chars().take(34).collect();
    }
    format!("{short}\u{2026}")
}

/// A card's events with the first sentence of the text it opens with in a
/// span, its lead, when that sentence ends within `LEAD_LONGEST`
/// characters. A sentence ends as `sentence_end` says, or at a colon past
/// the first few words, as a finding names what it then details; outside
/// any emphasis, link or code, and before any block in the item. The white
/// space after it stays, so the card's words are the plan's. A lead that
/// opens with bold words is named by them.
fn with_lead(mut item: Vec<pulldown_cmark::Event<'_>>) -> Vec<pulldown_cmark::Event<'_>> {
    use pulldown_cmark::{CowStr, Event, Tag, TagEnd};
    let inline = |event: Option<&Event>| matches!(event, Some(Event::Text(_) | Event::Code(_) | Event::Start(Tag::Emphasis | Tag::Strong | Tag::Strikethrough | Tag::Link { .. })));
    // A loose item's text opens in its first paragraph.
    let from = usize::from(matches!(item.first(), Some(Event::Start(Tag::Paragraph))));
    let (mut depth, mut length, mut cut) = (0usize, 0usize, None);
    for at in from..item.len() {
        match &item[at] {
            Event::Start(Tag::Emphasis | Tag::Strong | Tag::Strikethrough | Tag::Link { .. }) => depth += 1,
            Event::End(TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link) => depth = depth.saturating_sub(1),
            Event::Start(_) | Event::End(_) => break,
            Event::Text(text) if depth == 0 => {
                let line_ends = !inline(item.get(at + 1));
                let colon = text.char_indices().find(|&(index, mark)| mark == ':' && length + index > 8 && text[index + 1..].chars().next().map_or(line_ends, char::is_whitespace)).map(|(index, _)| index + 1);
                if let Some(end) = [sentence_end(text, line_ends), colon].into_iter().flatten().min() {
                    if length + text[..end].chars().count() <= LEAD_LONGEST {
                        cut = Some((at, text[..end].to_string(), text[end..].to_string()));
                    }
                    break;
                }
                length += text.chars().count();
            }
            Event::Text(text) | Event::Code(text) => length += text.chars().count(),
            _ => length += 1,
        }
        if length > LEAD_LONGEST {
            break;
        }
    }
    let Some((at, head, tail)) = cut else { return item };
    let named = matches!(item.get(from), Some(Event::Start(Tag::Strong)));
    let rest = item.split_off(at + 1);
    item.pop();
    let mut out: Vec<Event> = item.drain(..from).collect();
    out.push(Event::InlineHtml(CowStr::from(if named { "<span class=\"lead named\">" } else { "<span class=\"lead\">" })));
    out.append(&mut item);
    out.push(Event::Text(CowStr::from(head)));
    out.push(Event::InlineHtml(CowStr::from("</span>")));
    if !tail.is_empty() {
        out.push(Event::Text(CowStr::from(tail)));
    }
    out.extend(rest);
    out
}

/// The first sentence of `events`, when they open with a paragraph: its
/// inline events, and the events after it, where what is left of the
/// paragraph stays a paragraph. A sentence ends as `sentence_end` says, in
/// text outside any emphasis, link or code, so a cut never splits one; a
/// paragraph where none does is one sentence. The events unchanged when
/// they open with anything else.
fn first_sentence(mut events: Vec<pulldown_cmark::Event<'_>>) -> Result<Split<'_>, Vec<pulldown_cmark::Event<'_>>> {
    use pulldown_cmark::{CowStr, Event, Tag, TagEnd};
    if !matches!(events.first(), Some(Event::Start(Tag::Paragraph))) {
        return Err(events);
    }
    // Where it ends: in the text at an index, at a byte, or at the
    // paragraph's own end, which is the event at an index.
    let mut depth = 0usize;
    let mut cut = None;
    for (at, event) in events.iter().enumerate().skip(1) {
        match event {
            Event::Start(_) => depth += 1,
            Event::End(TagEnd::Paragraph) if depth == 0 => {
                cut = Some((at, None));
                break;
            }
            Event::End(_) => depth = depth.saturating_sub(1),
            Event::Text(text) if depth == 0 => {
                let line_ends = matches!(events.get(at + 1), Some(Event::SoftBreak | Event::HardBreak | Event::End(TagEnd::Paragraph)));
                if let Some(end) = sentence_end(text, line_ends) {
                    cut = Some((at, Some(end)));
                    break;
                }
            }
            _ => {}
        }
    }
    let Some((at, end)) = cut else { return Err(events) };
    let mut rest = events.split_off(at + 1);
    let last = events.pop();
    events.remove(0);
    let mut sentence = events;
    let (Some(end), Some(Event::Text(text))) = (end, last) else { return Ok((sentence, rest)) };
    let (head, tail) = text.split_at(end);
    sentence.push(Event::Text(CowStr::from(head.to_string())));
    let tail = tail.trim_start();
    if tail.is_empty() {
        while matches!(rest.first(), Some(Event::SoftBreak | Event::HardBreak)) {
            rest.remove(0);
        }
        if matches!(rest.first(), Some(Event::End(TagEnd::Paragraph))) {
            rest.remove(0);
            return Ok((sentence, rest));
        }
    }
    let mut after = vec![Event::Start(Tag::Paragraph)];
    if !tail.is_empty() {
        after.push(Event::Text(CowStr::from(tail.to_string())));
    }
    after.extend(rest);
    Ok((sentence, after))
}

/// Where the first sentence in `text` ends: past a full stop, question or
/// exclamation mark and any closing quote or bracket after it, where a
/// space follows, or the text ends and `line_ends` there. Not after an
/// abbreviation that seldom ends one.
fn sentence_end(text: &str, line_ends: bool) -> Option<usize> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    for (index, &(at, mark)) in chars.iter().enumerate() {
        if !matches!(mark, '.' | '?' | '!') {
            continue;
        }
        let mut next = index + 1;
        while chars.get(next).is_some_and(|(_, close)| matches!(close, '"' | '\'' | ')' | ']' | '\u{201d}' | '\u{2019}')) {
            next += 1;
        }
        let ends = chars.get(next).map_or(line_ends, |(_, after)| after.is_whitespace());
        let word = text[..at + mark.len_utf8()].rsplit(char::is_whitespace).next().unwrap_or_default();
        let word = word.trim_start_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
        if ends && !["e.g.", "i.e.", "cf.", "vs."].contains(&word.as_str()) {
            return Some(chars.get(next).map_or(text.len(), |(at, _)| *at));
        }
    }
    None
}

/// Whether a link to `url` keeps its address on the page: one to the web,
/// to mail, or to a place on the page itself. Any other -- javascript:,
/// data:, a file -- would run or load something from the page.
pub(crate) fn linkable(url: &str) -> bool {
    let url = url.to_ascii_lowercase();
    ["http://", "https://", "mailto:", "#"].iter().any(|start| url.starts_with(start))
}

fn esc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// `text` inside a JavaScript string literal.
fn js_string(text: &str) -> String {
    text.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect()
}

/// The class a step's state is styled by. Never "open", the class that
/// unfolds a step (task 1362).
fn state_class(state: &str) -> &'static str {
    match state {
        "done" => "done",
        "cancelled" => "cancelled",
        "in progress" => "progress",
        _ => "pending",
    }
}

/// FNV-1a, 64 bits: the page's version, which only has to change when the
/// page does, the fonts' names, and a repeating failure's (task 1442).
pub(crate) fn fnv(bytes: &[u8]) -> u64 {
    fnv_from(FNV_OFFSET, bytes)
}

/// Where FNV-1a starts.
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

/// FNV-1a over `bytes`, on from `hash`: over several runs of bytes, as over
/// them joined.
fn fnv_from(hash: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(hash, |hash, byte| (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3))
}

/// Sets the page's theme before it draws: the one the user picked last, or
/// the system's. And marks the page moving where its script can watch what
/// comes into view, which AKQA's reveals bring in (task 1367): the style
/// hides nothing on a page that cannot bring it back.
const THEME: &str = r##"(function () { var root = document.documentElement, theme = null; try { theme = localStorage.getItem("ekko-theme"); } catch (e) {} root.dataset.theme = theme || (matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light"); if ("IntersectionObserver" in window) root.dataset.motion = ""; })();"##;

/// What the page does: the steps that open in place, the map played stage
/// by stage and its arrows,
/// Medium's section bars, AKQA's command bar with the page's commands and
/// Review beside it, and the place a reader keeps across the reload a new
/// version makes.
const SCRIPT: &str = r##"(function () {
  "use strict";
  var root = document.documentElement;
  function smooth() { return matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth"; }
  // AKQA's grouped sections (note 1357): what a switch changes leaves in
  // 280 ms and comes back changed in 420, from a blur and below; at once
  // under reduced motion. Given keep, the switch under what it folds back,
  // the window follows it so that it stays where it was pressed (task
  // 1395): left alone, the window stayed and the Risks' switch went 906 px
  // up, out of a 390 px phone's window.
  function regroup(element, change, keep) {
    var held = function () {
      var top = keep ? keep.getBoundingClientRect().top : 0;
      change();
      if (keep) scrollBy(0, keep.getBoundingClientRect().top - top);
    };
    if (matchMedia("(prefers-reduced-motion: reduce)").matches || !element.animate) return held();
    var curve = "cubic-bezier(0.2, 0.65, 0.3, 1)";
    var leave = element.animate([{ opacity: 1, transform: "none" }, { opacity: 0, transform: "translateY(12px)" }], { duration: 280, easing: curve, fill: "forwards" });
    var back = function () {
      held();
      element.animate([{ opacity: 0, transform: "translateY(24px)", filter: "blur(6px)" }, { opacity: 1, transform: "none", filter: "blur(0)" }], { duration: 420, easing: curve });
      leave.cancel();
    };
    leave.finished.then(back, back);
  }

  // AKQA's reveals (task 1367, note 1357): what a scene holds comes into
  // view from a blur and below, once, and a heading word by word, 35 ms
  // apart. The head's script marked the page moving where this one can
  // watch what comes into view, and only then does the style hide it.
  if ("motion" in root.dataset) {
    var revealer = new IntersectionObserver(function (seen) {
      seen.forEach(function (entry) {
        if (!entry.isIntersecting) return;
        entry.target.classList.add("in");
        revealer.unobserve(entry.target);
      });
    }, { rootMargin: "0px 0px -8% 0px" });
    document.querySelectorAll(".scene > *, .words").forEach(function (node) { revealer.observe(node); });
    document.querySelectorAll(".words").forEach(function (heading) {
      heading.querySelectorAll(".w").forEach(function (word, i) { word.style.setProperty("--i", i); });
    });
  }

  // The theme: the one picked last, or the system's, switched by a command
  // in the pill (task 1337).
  function switchTheme() {
    root.dataset.theme = root.dataset.theme === "dark" ? "light" : "dark";
    try { localStorage.setItem("ekko-theme", root.dataset.theme); } catch (e) {}
  }
  function otherTheme() { return root.dataset.theme === "dark" ? "Light theme" : "Dark theme"; }

  // Steps open in place; a step on the map or found in the pill leads to
  // it, put on the stage where the steps have one (task 1373).
  function setOpen(step, open) {
    step.classList.toggle("open", open);
    var head = step.querySelector("button.step-head");
    if (head) head.setAttribute("aria-expanded", String(open));
  }
  function openStep(key) {
    var step = document.getElementById("step-" + key);
    if (!step) return;
    if (window.ekkoStage) ekkoStage(step);
    setOpen(step, true);
    step.scrollIntoView({ behavior: smooth(), block: "center" });
  }
  document.querySelectorAll("button.step-head").forEach(function (head) {
    head.addEventListener("click", function () {
      var step = head.parentElement;
      step.classList.remove("by-stage");
      setOpen(step, !step.classList.contains("open"));
    });
  });
  document.querySelectorAll(".map .node").forEach(function (node) {
    node.addEventListener("click", function () { openStep(node.dataset.step); });
  });

  // The map's arrows, while the map is wider than the window.
  var strip = document.querySelector(".strip"), arrows = document.querySelector(".arrows");
  if (strip && arrows) {
    arrows.querySelectorAll("button").forEach(function (button) {
      button.addEventListener("click", function () { strip.scrollBy({ left: Number(button.dataset.by), behavior: smooth() }); });
    });
    // Shown while the strip overflows, each off at the end it reaches.
    var ends = arrows.querySelectorAll("button");
    var fit = function () {
      var had = document.activeElement;
      arrows.hidden = strip.scrollWidth <= strip.clientWidth;
      ends[0].disabled = strip.scrollLeft <= 0;
      ends[1].disabled = strip.scrollLeft + strip.clientWidth >= strip.scrollWidth - 1;
      // An arrow gone out of use under the focus leaves it on the other,
      // which goes back the way it came, not on the page (task 1408).
      var other = had === ends[0] ? ends[1] : had === ends[1] ? ends[0] : null;
      if (other && had.disabled && !other.disabled && !arrows.hidden) other.focus({ preventScroll: true });
    };
    addEventListener("resize", fit);
    strip.addEventListener("scroll", fit);
    fit();
  }

  // The scenes (task 1367), AKQA's sections. The one holding the window's
  // middle puts the page in its mode, light or dark, which the registered
  // colours crossfade to as AKQA's do; Medium's section bars mark it, with
  // a card naming them all, and the pill's menu marks its part. AKQA's mark
  // at the top shows once the first scene has gone by.
  var scenes = Array.prototype.slice.call(document.querySelectorAll(".scene"));
  var parts = Array.prototype.slice.call(document.querySelectorAll("[data-part]"));
  var bars = Array.prototype.slice.call(document.querySelectorAll(".toc-bars span"));
  var rows = Array.prototype.slice.call(document.querySelectorAll(".toc a"));
  var mark = document.querySelector(".mark");
  // What a jump shows before it goes there (task 1375): a scene that hides
  // some of what it holds shows the part asked for, and may name what to
  // go to instead, as the scene of a note it opens in its sheet.
  var surfacers = [];
  function uncover(target) {
    var to = target;
    surfacers.forEach(function (show) { to = show(target) || to; });
    return to;
  }
  window.ekkoSurface = uncover;
  // What a scene keeps across the reload a new version makes (task 1374):
  // under its name, what keep() returns, which back() gets once the page
  // has loaded again, before anything comes into view.
  var keepers = {};
  function jump(id) {
    var target = document.getElementById(id);
    if (target) uncover(target).scrollIntoView({ behavior: smooth(), block: "start" });
  }
  rows.forEach(function (row) {
    row.addEventListener("click", function (event) { event.preventDefault(); jump(row.getAttribute("href").slice(1)); });
  });
  // The first scene's facts lead to their parts as the index does.
  document.querySelectorAll(".fact").forEach(function (fact) {
    fact.addEventListener("click", function (event) { event.preventDefault(); jump(fact.getAttribute("href").slice(1)); });
  });
  var current = -1;
  function follow() {
    var at = 0;
    scenes.forEach(function (scene, i) { if (scene.getBoundingClientRect().top < innerHeight / 2) at = i; });
    if (!scenes[at]) return;
    root.dataset.mode = scenes[at].dataset.mode;
    if (mark) mark.classList.toggle("on", at > 0);
    if (at === current) return;
    current = at;
    bars.forEach(function (bar, i) { bar.classList.toggle("on", i === at); });
    rows.forEach(function (row, i) { row.classList.toggle("on", i === at); });
    marks();
  }
  // The pill's menu marks it too, as AKQA's marks the page it is on: the
  // plan's parts all under Plan, and of a scene's tabs the one picked.
  function marks() {
    var scene = scenes[current];
    if (!scene) return;
    document.querySelectorAll(".bar-nav a").forEach(function (a) {
      var to = a.getAttribute("href").slice(1), target = document.getElementById(to);
      var on = (to.indexOf("plan-") === 0 && scene.id.indexOf("plan-") === 0) || (!!target && scene.contains(target) && target.getClientRects().length > 0);
      if (on) a.setAttribute("aria-current", "true"); else a.removeAttribute("aria-current");
    });
  }
  var following = false;
  function moved() {
    if (following) return;
    following = true;
    requestAnimationFrame(function () { following = false; follow(); });
  }
  addEventListener("scroll", moved, { passive: true });
  addEventListener("resize", moved);
  follow();

  // The Goal (task 1369): its statement held in the window while the
  // scroll lights its words one after another, the rest of the Goal coming
  // after them and a hairline saying how far. The scene is as tall as its
  // words need, which STYLE reads from --tall; under reduced motion, as tall
  // as its text, all of it lit. A function of its own, so its names are its
  // own.
  (function () {
    var goal = document.querySelector(".scene.goal");
    if (!goal || !("motion" in root.dataset)) return;
    var units = Array.prototype.slice.call(goal.querySelectorAll(".statement .w, .statement code"));
    var rest = goal.querySelector(".goal-rest"), line = goal.querySelector(".progress i");
    var still = matchMedia("(prefers-reduced-motion: reduce)");
    if (units.length) goal.style.setProperty("--tall", (1.3 + units.length / 36).toFixed(3));
    var light = function () {
      var p = 1;
      if (!still.matches) {
        var room = goal.offsetHeight - innerHeight;
        p = room > 0 ? Math.min(1, Math.max(0, -goal.getBoundingClientRect().top / room)) : 1;
      }
      var lit = Math.ceil(Math.min(1, p / 0.72) * units.length);
      units.forEach(function (unit, i) { unit.classList.toggle("lit", i < lit); });
      if (rest) rest.classList.toggle("in", p > 0.76 || !units.length);
      if (line) line.style.transform = "scaleX(" + p.toFixed(3) + ")";
    };
    var lighting = false;
    addEventListener("scroll", function () {
      if (lighting) return;
      lighting = true;
      requestAnimationFrame(function () { lighting = false; light(); });
    }, { passive: true });
    addEventListener("resize", light);
    if (still.addEventListener) still.addEventListener("change", light);
    light();
  })();

  // What is known (task 1370): a tab a group, a card a finding, the cards
  // on a track that slides in with the scroll as the scene comes, as
  // AKQA's carousels do (about 560 px). A card opens in place to the whole
  // of it, by its button or a click on it, and Esc closes it; the arrow
  // keys and the buttons under the track move it a card at a time.
  (function () {
    var known = document.querySelector(".scene.known");
    if (!known) return;
    var still = matchMedia("(prefers-reduced-motion: reduce)");
    var tabs = Array.prototype.slice.call(known.querySelectorAll(".tab"));
    var groups = Array.prototype.slice.call(known.querySelectorAll(".group"));
    var count = known.querySelector(".controls .count"), arrows = known.querySelectorAll(".controls .round");
    var pad = function (n) { return (n < 10 ? "0" : "") + n; };
    var shown = function () { return groups.filter(function (group) { return !group.hidden; })[0] || groups[0]; };
    var cards = function (group) { return Array.prototype.slice.call(group.querySelectorAll(".track > .card")); };
    var edge = function (track) { return track.getBoundingClientRect().left + parseFloat(getComputedStyle(track).paddingLeft); };
    // The card nearest the track's start.
    var at = function (group) {
      var from = edge(group.querySelector(".track")), best = 0, gap = Infinity;
      cards(group).forEach(function (card, n) {
        var off = Math.abs(card.getBoundingClientRect().left - from);
        if (off < gap) { gap = off; best = n; }
      });
      return best;
    };
    var show = function (group, n) {
      var list = cards(group), track = group.querySelector(".track");
      var card = list[Math.max(0, Math.min(list.length - 1, n))];
      if (card) track.scrollBy({ left: card.getBoundingClientRect().left - edge(track), behavior: smooth() });
    };
    var update = function () {
      var group = shown(), track = group.querySelector(".track"), had = document.activeElement;
      if (!count || arrows.length < 2) return;
      count.textContent = pad(at(group) + 1) + " / " + pad(cards(group).length);
      arrows[0].disabled = track.scrollLeft <= 1;
      arrows[1].disabled = track.scrollLeft + track.clientWidth >= track.scrollWidth - 2;
      // An arrow gone out of use under the focus leaves it on the track,
      // where the arrow keys go on, as the Risks' leave it on their stack
      // (task 1408).
      if ((had === arrows[0] || had === arrows[1]) && had.disabled) track.focus({ preventScroll: true });
    };
    var open = function (card, wide) {
      var more = card.querySelector(".more");
      card.classList.toggle("open", wide);
      more.setAttribute("aria-expanded", String(wide));
      more.querySelector("span").textContent = wide ? "Close" : "Read";
      more.querySelector("i").textContent = wide ? "\u2196" : "\u2198";
      // Open, it grows wider; once it has, the track shows the whole of it.
      if (wide) setTimeout(function () {
        var track = card.parentNode, box = card.getBoundingClientRect(), frame = track.getBoundingClientRect();
        if (box.right > frame.right || box.left < frame.left) track.scrollBy({ left: box.left - edge(track), behavior: smooth() });
      }, still.matches ? 0 : 620);
    };
    var pick = function (tab, moved) {
      tabs.forEach(function (other) {
        var chosen = other === tab, group = document.getElementById(other.getAttribute("aria-controls"));
        other.setAttribute("aria-selected", String(chosen));
        other.tabIndex = chosen ? 0 : -1;
        if (group) group.hidden = !chosen;
      });
      var group = shown();
      group.classList.add("in");
      group.querySelector(".track").scrollLeft = 0;
      if (moved && !still.matches) cards(group).forEach(function (card, n) {
        card.animate([{ opacity: 0, transform: "translateX(48px)", filter: "blur(4px)" }, { opacity: 1, transform: "none", filter: "blur(0)" }], { duration: 700, delay: n * 70, easing: "cubic-bezier(0.2, 0.65, 0.3, 1)", fill: "backwards" });
      });
      update();
    };
    tabs.forEach(function (tab) { tab.addEventListener("click", function () { pick(tab, true); }); });
    known.addEventListener("keydown", function (event) {
      var key = event.key, tab = event.target.closest(".tab"), card = event.target.closest(".card"), to;
      if (tab) {
        var n = tabs.indexOf(tab);
        to = { ArrowRight: n + 1, ArrowLeft: n - 1, Home: 0, End: tabs.length - 1 }[key];
        if (to === undefined) return;
        event.preventDefault();
        tab = tabs[(to + tabs.length) % tabs.length];
        tab.focus();
        pick(tab, true);
      } else if (key === "Escape" && card && card.classList.contains("open")) {
        event.preventDefault();
        open(card, false);
        card.querySelector(".more").focus();
      } else if ((key === "ArrowRight" || key === "ArrowLeft") && event.target.closest(".track")) {
        event.preventDefault();
        var group = event.target.closest(".group"), list = cards(group);
        to = Math.max(0, Math.min(list.length - 1, (card ? list.indexOf(card) : at(group)) + (key === "ArrowRight" ? 1 : -1)));
        if (card) list[to].querySelector(".more").focus({ preventScroll: true });
        show(group, to);
      }
    });
    known.addEventListener("click", function (event) {
      var card = event.target.closest(".card");
      if (!card) return;
      if (event.target.closest(".more")) return open(card, !card.classList.contains("open"));
      // A link still goes where it points, a comment's words open the
      // comment, and a drag over the words is a selection to comment on.
      if (card.classList.contains("open") || event.target.closest("a, mark.c, .pin, input") || !getSelection().isCollapsed) return;
      open(card, true);
    });
    Array.prototype.forEach.call(arrows, function (arrow) {
      arrow.addEventListener("click", function () { var group = shown(); show(group, at(group) + Number(arrow.dataset.by)); });
    });
    groups.forEach(function (group) {
      var moving = false;
      group.querySelector(".track").addEventListener("scroll", function () {
        if (moving) return;
        moving = true;
        requestAnimationFrame(function () { moving = false; update(); });
      }, { passive: true });
    });
    // The track comes from the right as the scene comes up, linked to the
    // scroll; still under reduced motion.
    var slide = function () {
      var x = 0;
      if ("motion" in root.dataset && !still.matches) {
        var p = Math.min(1, Math.max(0, (innerHeight - known.getBoundingClientRect().top) / (innerHeight * 0.85)));
        x = Math.round(560 * Math.pow(1 - p, 2));
      }
      groups.forEach(function (group) { group.querySelector(".track").style.transform = x ? "translateX(" + x + "px)" : ""; });
    };
    var sliding = false;
    addEventListener("scroll", function () {
      if (sliding) return;
      sliding = true;
      requestAnimationFrame(function () { sliding = false; slide(); });
    }, { passive: true });
    addEventListener("resize", function () { slide(); update(); });
    if (still.addEventListener) still.addEventListener("change", slide);
    slide();
    update();
    // A jump to what a tab not picked holds picks the tab first, and one to
    // a card brings it to the track's start, open (task 1378): the jump
    // then goes to the tabs, or to the group where there are none, so the
    // tab picked shows above the card. A reload keeps the tab, the cards
    // open and where the track is.
    var tabOf = function (group) { return tabs.filter(function (tab) { return tab.getAttribute("aria-controls") === group.id; })[0]; };
    surfacers.push(function (target) {
      var group = target.closest && target.closest(".group");
      if (!group || !known.contains(group)) return null;
      if (group.hidden) pick(tabOf(group), false);
      var card = target.closest(".card");
      if (!card) return null;
      if (!card.classList.contains("open")) open(card, true);
      var track = group.querySelector(".track");
      track.scrollLeft += card.getBoundingClientRect().left - edge(track);
      return known.querySelector(".tabs") || group;
    });
    keepers.known = {
      keep: function () {
        var group = shown();
        return { tab: group.id, open: cards(group).filter(function (card) { return card.classList.contains("open"); }).map(function (card) { return card.id; }), left: group.querySelector(".track").scrollLeft };
      },
      back: function (was) {
        var group = was.tab && document.getElementById(was.tab);
        if (group && groups.indexOf(group) >= 0 && group.hidden) pick(tabOf(group), false);
        (was.open || []).forEach(function (id) {
          var card = document.getElementById(id);
          if (card && shown().contains(card) && !card.classList.contains("open")) open(card, true);
        });
        shown().querySelector(".track").scrollLeft = was.left || 0;
        update();
      }
    };
  })();

  // The Design (task 1371): its points told one at a time as one scrolls,
  // beside a numeral held in place that turns to the point at the window's
  // middle, as AKQA's grouped sections do: the old number leaves in 280 ms
  // and the new one comes in 420 ms after it. That point is lit, the others
  // grey; a tick leads to each.
  (function () {
    var design = document.querySelector(".scene.design");
    if (!design) return;
    var still = matchMedia("(prefers-reduced-motion: reduce)");
    var points = Array.prototype.slice.call(design.querySelectorAll(".points > .point"));
    var ticks = Array.prototype.slice.call(design.querySelectorAll(".ticks button"));
    var numeral = design.querySelector(".numeral"), shown = 0;
    var pad = function (n) { return (n < 10 ? "0" : "") + n; };
    var turn = function (n) {
      if (n === shown) return;
      shown = n;
      points.forEach(function (point, i) { point.classList.toggle("on", i === n); });
      ticks.forEach(function (tick, i) { tick.setAttribute("aria-current", String(i === n)); });
      var next = document.createElement("span");
      next.textContent = pad(n + 1);
      if (still.matches) {
        numeral.textContent = "";
        numeral.appendChild(next);
        return;
      }
      Array.prototype.forEach.call(numeral.children, function (old) {
        old.className = "out";
        setTimeout(function () { old.remove(); }, 320);
      });
      next.className = "enter";
      numeral.appendChild(next);
    };
    // The point whose top is above the window's middle, the last of them.
    var follow = function () {
      var middle = innerHeight / 2, n = 0;
      points.forEach(function (point, i) { if (point.getBoundingClientRect().top < middle) n = i; });
      turn(n);
    };
    ticks.forEach(function (tick, i) {
      tick.addEventListener("click", function () { points[i].scrollIntoView({ behavior: smooth(), block: "center" }); });
    });
    design.classList.add("turning");
    var following = false;
    addEventListener("scroll", function () {
      if (following) return;
      following = true;
      requestAnimationFrame(function () { following = false; follow(); });
    }, { passive: true });
    addEventListener("resize", follow);
    follow();
  })();

  // The Risks (task 1372): a stack of sheets, one in front, as a deck is
  // dealt. Next and the arrow keys send the one in front away, Back brings
  // it again, and a switch lays them all out at once: the stack leaves in
  // 280 ms and comes back laid out in 420, as AKQA's grouped sections do.
  // A click on a sheet does nothing, so a word double-clicked there is
  // selected to comment on.
  (function () {
    var risks = document.querySelector(".scene.risks");
    if (!risks) return;
    var stack = risks.querySelector(".stack"), sheets = Array.prototype.slice.call(stack.children);
    var count = risks.querySelector(".controls .count"), all = risks.querySelector(".controls .toggle");
    var back = risks.querySelector(".controls [data-by='-1']"), on = risks.querySelector(".controls [data-by='1']");
    var pad = function (n) { return (n < 10 ? "0" : "") + n; };
    var at = 0;
    var lay = function () {
      var flat = stack.classList.contains("all"), had = document.activeElement;
      sheets.forEach(function (sheet, n) {
        sheet.style.setProperty("--d", String(n - at));
        sheet.classList.toggle("gone", n < at);
        sheet.inert = !flat && n !== at;
      });
      count.textContent = flat ? pad(sheets.length) + " in all" : pad(at + 1) + " / " + pad(sheets.length);
      back.disabled = flat || at === 0;
      on.disabled = flat || at === sheets.length - 1;
      // A button gone out of use leaves the focus on the stack, where the
      // arrow keys go on.
      if (had && had.disabled) stack.focus({ preventScroll: true });
    };
    var move = function (by) {
      var to = Math.max(0, Math.min(sheets.length - 1, at + by));
      if (to === at) return;
      at = to;
      lay();
    };
    back.addEventListener("click", function () { move(-1); });
    on.addEventListener("click", function () { move(1); });
    stack.addEventListener("keydown", function (event) {
      var by = { ArrowRight: 1, ArrowLeft: -1, Home: -at, End: sheets.length - 1 - at }[event.key];
      if (by === undefined || stack.classList.contains("all")) return;
      event.preventDefault();
      move(by);
    });
    all.addEventListener("click", function () {
      var flat = all.getAttribute("aria-checked") !== "true";
      all.setAttribute("aria-checked", String(flat));
      regroup(stack, function () { stack.classList.toggle("all", flat); lay(); }, flat ? null : all);
    });
    // A jump to a sheet not in front deals the stack to it (task 1378); a
    // reload keeps the sheet in front and the switch.
    surfacers.push(function (target) {
      var n = sheets.indexOf(target.closest && target.closest(".sheet"));
      if (n >= 0 && n !== at && !stack.classList.contains("all")) move(n - at);
      return null;
    });
    keepers.risks = {
      keep: function () { return { at: at, all: stack.classList.contains("all") }; },
      back: function (was) {
        at = Math.max(0, Math.min(sheets.length - 1, was.at || 0));
        all.setAttribute("aria-checked", String(!!was.all));
        stack.classList.toggle("all", !!was.all);
        lay();
      }
    };
  })();

  // The Steps (task 1373): one step on a stage at a time, as AKQA's ROLE
  // carousel shows one role, coming in from the side one moves toward: the
  // step leaving goes in 280 ms and the next comes in 420 ms after it.
  // Back, Next, the arrow keys, a segment or a chip put a step on the
  // stage, and so does a jump to it from the pill or the map; the switch
  // shows the whole list.
  (function () {
    var scene = document.querySelector("#steps.staged");
    if (!scene) return;
    var still = matchMedia("(prefers-reduced-motion: reduce)");
    var stage = scene.querySelector(".steps"), steps = Array.prototype.slice.call(stage.children);
    var segs = Array.prototype.slice.call(scene.querySelectorAll(".segs .seg"));
    var count = scene.querySelector(".controls .count"), all = scene.querySelector(".controls .toggle");
    var back = scene.querySelector(".controls [data-by='-1']"), on = scene.querySelector(".controls [data-by='1']");
    var pad = function (n) { return (n < 10 ? "0" : "") + n; };
    var at = Math.max(0, steps.findIndex(function (step) { return !step.hidden; }));
    // The stage opens the step it shows, and folds it back when it moves on
    // or the list shows, unless the reader opened it: the list stays as the
    // reader left it. The step Rust put on the stage came open by it.
    var unfold = function (step) {
      steps.forEach(function (other) {
        if (other === step || !other.classList.contains("by-stage")) return;
        other.classList.remove("by-stage");
        setOpen(other, false);
      });
      if (!step || step.classList.contains("open") || !step.querySelector("button.step-head")) return;
      setOpen(step, true);
      step.classList.add("by-stage");
    };
    if (steps[at].classList.contains("open")) steps[at].classList.add("by-stage");
    var lay = function () {
      var list = stage.classList.contains("all"), had = document.activeElement;
      steps.forEach(function (step, n) { step.hidden = !list && n !== at && !step.classList.contains("leaving"); });
      segs.forEach(function (seg, n) {
        if (n === at) seg.setAttribute("aria-current", "step");
        else seg.removeAttribute("aria-current");
        seg.tabIndex = n === at ? 0 : -1;
      });
      count.textContent = list ? pad(steps.length) + " steps" : pad(at + 1) + " / " + pad(steps.length);
      back.disabled = list || at === 0;
      on.disabled = list || at === steps.length - 1;
      if (had && had.disabled) stage.focus({ preventScroll: true });
    };
    var show = function (n) {
      n = Math.max(0, Math.min(steps.length - 1, n));
      if (n === at) return;
      var from = steps[at], to = steps[n], toward = n > at ? 1 : -1;
      at = n;
      // What leaves the stage hands the focus back to it.
      if (from.contains(document.activeElement)) stage.focus({ preventScroll: true });
      steps.forEach(function (step) {
        step.classList.remove("leaving");
        step.getAnimations().forEach(function (motion) { motion.cancel(); });
      });
      unfold(to);
      if (still.matches || !to.animate) return lay();
      var curve = "cubic-bezier(0.2, 0.65, 0.3, 1)";
      from.classList.add("leaving");
      lay();
      var leave = from.animate([{ opacity: 1, transform: "none" }, { opacity: 0, transform: "translateX(" + -48 * toward + "px)" }], { duration: 280, easing: curve, fill: "forwards" });
      leave.finished.then(function () { from.classList.remove("leaving"); leave.cancel(); lay(); }, function () {});
      to.animate([{ opacity: 0, transform: "translateX(" + 48 * toward + "px)" }, { opacity: 1, transform: "none" }], { duration: 420, delay: 280, easing: curve, fill: "backwards" });
    };
    // On the stage, out of the list if it showed.
    var put = function (n) {
      if (!stage.classList.contains("all")) return show(n);
      all.setAttribute("aria-checked", "false");
      stage.classList.remove("all");
      at = Math.max(0, Math.min(steps.length - 1, n));
      unfold(steps[at]);
      lay();
    };
    back.addEventListener("click", function () { show(at - 1); });
    on.addEventListener("click", function () { show(at + 1); });
    all.addEventListener("click", function () {
      var list = all.getAttribute("aria-checked") !== "true";
      all.setAttribute("aria-checked", String(list));
      regroup(stage, function () {
        stage.classList.toggle("all", list);
        unfold(list ? null : steps[at]);
        lay();
      }, list ? null : all);
    });
    segs.forEach(function (seg, n) { seg.addEventListener("click", function () { put(n); }); });
    stage.addEventListener("click", function (event) {
      var chip = event.target.closest(".after .chip"), n = chip ? steps.indexOf(document.getElementById("step-" + chip.dataset.step)) : -1;
      if (n >= 0) put(n);
    });
    // The arrow keys, Home and End, on the stage or on the segments, whose
    // focus follows the step.
    var keys = function (event, onSegment) {
      var by = { ArrowRight: 1, ArrowLeft: -1, Home: -at, End: steps.length - 1 - at }[event.key];
      if (by === undefined || event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return;
      if (!onSegment && stage.classList.contains("all")) return;
      event.preventDefault();
      put(at + by);
      if (onSegment) segs[at].focus();
    };
    stage.addEventListener("keydown", function (event) { keys(event, false); });
    scene.querySelector(".segs").addEventListener("keydown", function (event) { keys(event, true); });
    // A jump to a step, from the pill or the map, puts it on the stage, and
    // so does any other jump to one off the stage (task 1378), as an
    // address naming it. A reload keeps the step on the stage, or the list.
    window.ekkoStage = function (step) {
      var n = steps.indexOf(step);
      if (n >= 0) put(n);
    };
    surfacers.push(function (target) {
      var step = target.closest && target.closest(".step");
      if (step && step.hidden) ekkoStage(step);
      return null;
    });
    keepers.stage = {
      keep: function () { return { at: steps[at].id, list: stage.classList.contains("all") }; },
      back: function (was) {
        var n = steps.indexOf(document.getElementById(was.at));
        if (n >= 0) at = n;
        all.setAttribute("aria-checked", String(!!was.list));
        stage.classList.toggle("all", !!was.list);
        unfold(was.list ? null : steps[at]);
        lay();
      }
    };
  })();

  // The Map (task 1374), played as a film: once it comes into view, its
  // stages one after another, each step a stage past the latest it waits
  // on, the curves to them drawn as they come. Play and Pause, and a
  // slider through the stages, with a line saying the stage; a map wider
  // than the window has its strip follow it. A step pointed at lights what
  // it waits on and what waits on it, all the way along. Under reduced
  // motion it shows whole, and plays only when asked; a reload keeps the
  // stage, and plays nothing played already.
  (function () {
    var scene = document.getElementById("map"), map = scene && scene.querySelector(".map");
    if (!map) return;
    var nodes = Array.prototype.slice.call(map.querySelectorAll(".node"));
    var edges = Array.prototype.slice.call(map.querySelectorAll(".edge"));
    var chain = function (key) {
      var keys = [key], lines = [];
      [["to", "from"], ["from", "to"]].forEach(function (way) {
        for (var queue = [key]; queue.length;) {
          var at = queue.shift();
          edges.forEach(function (edge) {
            if (edge.dataset[way[0]] !== at) return;
            if (lines.indexOf(edge) < 0) lines.push(edge);
            var other = edge.dataset[way[1]];
            if (keys.indexOf(other) < 0) { keys.push(other); queue.push(other); }
          });
        }
      });
      return { keys: keys, edges: lines };
    };
    var light = function (node) {
      var lit = node && chain(node.dataset.step);
      map.classList.toggle("focus", !!lit);
      nodes.forEach(function (one) { one.classList.toggle("chain", !!lit && lit.keys.indexOf(one.dataset.step) >= 0); });
      edges.forEach(function (edge) { edge.classList.toggle("chain", !!lit && lit.edges.indexOf(edge) >= 0); });
    };
    nodes.forEach(function (node) {
      node.addEventListener("mouseenter", function () { light(node); });
      node.addEventListener("focus", function () { light(node); });
      node.addEventListener("mouseleave", function () { light(nodes.indexOf(document.activeElement) >= 0 ? document.activeElement : null); });
      node.addEventListener("blur", function () { light(null); });
    });

    var player = scene.querySelector(".player");
    if (!player) return;
    var play = player.querySelector(".play"), scrub = player.querySelector(".scrub"), said = player.querySelector(".at");
    var strip = scene.querySelector(".strip"), still = matchMedia("(prefers-reduced-motion: reduce)");
    var stages = Number(scrub.max), shown = stages, timer = 0, played = still.matches || !window.IntersectionObserver;
    var stage = function (element) { return Number(element.dataset.stage); };
    // The strip keeps the stage's column three quarters across.
    var follow = function () {
      var column = nodes.filter(function (node) { return stage(node) === shown; })[0];
      if (!column || strip.scrollWidth <= strip.clientWidth) return;
      var frame = strip.getBoundingClientRect(), box = column.getBoundingClientRect();
      var left = Math.max(0, Math.min(strip.scrollWidth - strip.clientWidth, strip.scrollLeft + box.right - frame.left - strip.clientWidth * 0.75));
      if (Math.abs(left - strip.scrollLeft) > 8) strip.scrollTo({ left: left, behavior: smooth() });
    };
    var set = function (n) {
      shown = n;
      nodes.forEach(function (node) { node.classList.toggle("unlit", stage(node) > n); });
      edges.forEach(function (edge) { edge.classList.toggle("unlit", stage(edge) > n); });
      scrub.value = String(n);
      scrub.style.setProperty("--p", (n - 1) / (stages - 1) * 100 + "%");
      scrub.setAttribute("aria-valuetext", "Stage " + n + " of " + stages);
      var keys = nodes.filter(function (node) { return stage(node) === n; }).map(function (node) { return node.dataset.step; });
      said.textContent = n === stages && !timer ? "The whole plan, in " + stages + " stages" : "Stage " + n + " of " + stages + ": " + keys.join(", ");
    };
    // The button says what it does; while it plays, the line is not read
    // out at each stage, as a carousel that rotates by itself is not.
    var press = function (playing) {
      play.setAttribute("aria-label", playing ? "Pause" : "Play");
      play.querySelector("path").setAttribute("d", playing ? "M9 6v12M15 6v12" : "M8.5 5.8v12.4L18.2 12z");
      said.setAttribute("aria-live", playing ? "off" : "polite");
    };
    var stop = function () {
      clearInterval(timer);
      timer = 0;
      press(false);
      set(shown);
    };
    var start = function (from) {
      clearInterval(timer);
      timer = setInterval(function () {
        if (shown >= stages) return stop();
        set(shown + 1);
        follow();
      }, 1100);
      press(true);
      set(from);
      follow();
    };
    // What the reader does to it first, before it comes into view, it does
    // not undo by playing by itself.
    play.addEventListener("click", function () {
      played = true;
      if (timer) stop(); else start(shown >= stages ? 1 : shown + 1);
    });
    scrub.addEventListener("input", function () {
      played = true;
      if (timer) stop();
      set(Number(scrub.value));
      follow();
    });
    if (!played) {
      set(1);
      var watch = new IntersectionObserver(function (seen) {
        if (!seen[0].isIntersecting) return;
        watch.disconnect();
        if (played) return;
        played = true;
        start(1);
      }, { rootMargin: "0px 0px -28% 0px" });
      watch.observe(map);
    }
    // The whole plan stays whole, which a new version may have given
    // another stage (task 1378).
    keepers.map = {
      keep: function () { return { shown: shown, played: played, playing: !!timer, whole: shown === stages }; },
      back: function (was) {
        if (!was.played) return;
        played = true;
        var n = was.whole ? stages : Math.max(1, Math.min(stages, was.shown));
        if (was.playing && n < stages) start(n); else set(n);
      }
    };
  })();

  // Notes and comments (task 1375): a tab each when both are there, the
  // notes as rows a kind's pill filters, as AKQA's switches regroup. A row
  // opens its note in a sheet from the right, a modal dialog: the page
  // behind it is inert, so Tab stays in it, but for the browser's own
  // controls, which a keyboard may still reach. Esc closes it, and so does
  // a click outside it; the note goes back to its place, and the focus to
  // its row. A jump to the notes, the comments or a note shows its tab, or
  // its sheet, first.
  (function () {
    var scene = document.querySelector(".scene.talk");
    if (!scene) return;
    var still = matchMedia("(prefers-reduced-motion: reduce)");
    var curve = "cubic-bezier(0.2, 0.65, 0.3, 1)";
    var tabs = Array.prototype.slice.call(scene.querySelectorAll(".tab"));
    var panels = scene.querySelector(".panels"), rows = scene.querySelector(".rows");
    var kinds = Array.prototype.slice.call(scene.querySelectorAll(".kinds .pill"));
    var tabOf = function (panel) { return tabs.filter(function (tab) { return tab.getAttribute("aria-controls") === panel.id; })[0]; };
    var pick = function (tab, moved) {
      if (!tab || tab.getAttribute("aria-selected") === "true") return;
      var change = function () {
        tabs.forEach(function (other) {
          var chosen = other === tab;
          other.setAttribute("aria-selected", String(chosen));
          other.tabIndex = chosen ? 0 : -1;
          document.getElementById(other.getAttribute("aria-controls")).hidden = !chosen;
        });
        marks();
      };
      if (moved) regroup(panels, change); else change();
    };
    tabs.forEach(function (tab) { tab.addEventListener("click", function () { pick(tab, true); }); });
    if (tabs.length) tabs[0].parentNode.addEventListener("keydown", function (event) {
      var n = tabs.indexOf(event.target), to = { ArrowRight: n + 1, ArrowLeft: n - 1, Home: 0, End: tabs.length - 1 }[event.key];
      if (n < 0 || to === undefined) return;
      event.preventDefault();
      var tab = tabs[(to + tabs.length) % tabs.length];
      tab.focus();
      pick(tab, true);
    });
    // A kind picked shows its notes alone; picked again, or All, every one.
    var chosen = null;
    var lay = function () {
      kinds.forEach(function (pill) { pill.setAttribute("aria-pressed", String((pill.dataset.kind || null) === chosen)); });
      if (rows) Array.prototype.forEach.call(rows.children, function (row) { row.hidden = chosen !== null && row.dataset.kind !== chosen; });
    };
    kinds.forEach(function (pill) {
      pill.addEventListener("click", function () {
        var kind = pill.dataset.kind && pill.dataset.kind !== chosen ? pill.dataset.kind : null;
        if (kind !== chosen) regroup(rows, function () { chosen = kind; lay(); });
      });
    });
    // A reload keeps the tab, the kind picked and the note in the sheet
    // (task 1378).
    keepers.talk = {
      keep: function () {
        var tab = tabs.filter(function (one) { return one.getAttribute("aria-selected") === "true"; })[0];
        return { tab: tab ? tab.id : null, kind: chosen, note: shown ? shown.id : null };
      },
      back: function (was) {
        if (was.tab) pick(document.getElementById(was.tab), false);
        if (was.kind !== undefined && was.kind !== chosen && kinds.some(function (pill) { return pill.dataset.kind === was.kind; })) { chosen = was.kind; lay(); }
        var note = was.note && document.getElementById(was.note);
        if (note && open) open(note, rowOf(note));
      }
    };

    // The sheet. Its motion is the script's, so it runs the same in either
    // engine: in 560 ms as it opens, out in AKQA's 280 as it closes, the
    // veil behind it fading; none under reduced motion.
    var sheet = document.querySelector("dialog.side");
    if (!sheet || !rows) return;
    var place = sheet.querySelector(".side-note"), shut = sheet.querySelector(".shut");
    var shown = null, home = null, from = null, moving = [], closing = false;
    var rowOf = function (note) { return rows.querySelector(".row[data-note='" + note.id.slice(5) + "']"); };
    var motion = function (frames, veil, how) {
      if (still.matches || !sheet.animate) return;
      moving.push(sheet.animate(frames, how));
      try { moving.push(sheet.animate(veil, Object.assign({ pseudoElement: "::backdrop" }, how))); } catch (e) {}
    };
    var settle = function () {
      moving.forEach(function (one) { one.cancel(); });
      moving = [];
    };
    // Closed, by the script or the browser: the note back, the focus to
    // the row it was opened from.
    var closed = function () {
      settle();
      closing = false;
      if (shown) home.parent.insertBefore(shown, home.next);
      var back = from;
      shown = home = from = null;
      if (back && back.isConnected && document.activeElement !== back) back.focus({ preventScroll: true });
    };
    var open = function (note, row) {
      if (!note) return;
      if (sheet.open) { sheet.close(); closed(); }
      settle();
      home = { parent: note.parentNode, next: note.nextSibling };
      shown = note;
      from = row || rowOf(note);
      place.appendChild(note);
      var title = note.querySelector("h3");
      sheet.setAttribute("aria-label", title ? title.textContent : "Note");
      sheet.showModal();
      shut.focus();
      sheet.querySelector(".side-in").scrollTop = 0;
      motion([{ transform: "translateX(100%)" }, { transform: "none" }], [{ opacity: 0 }, { opacity: 1 }], { duration: 560, easing: curve });
    };
    var close = function () {
      if (!sheet.open || closing) return;
      settle();
      motion([{ transform: "none" }, { transform: "translateX(100%)" }], [{ opacity: 1 }, { opacity: 0 }], { duration: 280, easing: curve, fill: "forwards" });
      if (!moving.length) { sheet.close(); return closed(); }
      closing = true;
      moving[0].finished.then(function () { sheet.close(); closed(); }, function () {});
    };
    rows.addEventListener("click", function (event) {
      var row = event.target.closest(".row");
      if (row) open(document.getElementById("note-" + row.dataset.note), row);
    });
    shut.addEventListener("click", close);
    // Esc asks the sheet to close: it goes out as it came. A browser that
    // will not let the page hold it, as one pressed again without a click
    // between, closes it at once.
    sheet.addEventListener("cancel", function (event) {
      if (!event.cancelable) return;
      event.preventDefault();
      close();
    });
    sheet.addEventListener("close", function () { if (!sheet.open) closed(); });
    // A click on the veil, begun there too: a selection in the note that
    // ends outside it is no click outside.
    var outside = false;
    sheet.addEventListener("pointerdown", function (event) { outside = event.target === sheet; });
    sheet.addEventListener("click", function (event) { if (event.target === sheet && outside) close(); });

    // A jump shows what it goes to: a note in its sheet, the notes' tab or
    // the comments' first.
    surfacers.push(function (target) {
      var note = target.closest && target.closest(".note");
      if (note && (scene.contains(note) || sheet.contains(note))) {
        pick(tabs[0], false);
        var row = rowOf(note);
        if (row && row.parentNode.hidden) { chosen = null; lay(); }
        open(note, row);
        return scene;
      }
      var panel = scene.contains(target) && target !== scene && target.closest(".panel");
      if (!panel) return null;
      pick(tabOf(panel), false);
      return target === panel ? scene : target;
    });
  })();

  // History (task 1376): the slider and the points on the line pick the
  // change shown, which comes in as AKQA's grouped sections arrive, in
  // 420 ms from a blur and below; still under reduced motion. A jump to a
  // change picks it.
  (function () {
    var scene = document.querySelector(".scene.changes");
    var changes = scene ? Array.prototype.slice.call(scene.querySelectorAll(".changed > .version")) : [];
    if (!changes.length) return;
    var still = matchMedia("(prefers-reduced-motion: reduce)");
    var points = Array.prototype.slice.call(scene.querySelectorAll(".line button"));
    var scrub = scene.querySelector(".scrub");
    var number = function (change) { return Number(change.dataset.change); };
    var shown = changes.length;
    var choose = function (n) {
      n = Math.max(1, Math.min(changes.length, n));
      points.forEach(function (point) {
        var at = Number(point.dataset.node);
        point.setAttribute("aria-current", String(at === n || at === n - 1));
      });
      var change = changes.filter(function (one) { return number(one) === n; })[0];
      if (scrub) {
        scrub.value = String(n);
        scrub.style.setProperty("--p", (n - 1) / (changes.length - 1) * 100 + "%");
        scrub.setAttribute("aria-valuetext", change.querySelector("h3").textContent);
      }
      if (n === shown) return;
      shown = n;
      changes.forEach(function (one) {
        one.getAnimations().forEach(function (motion) { motion.cancel(); });
        one.hidden = one !== change;
      });
      if (!still.matches && change.animate) change.animate([{ opacity: 0, transform: "translateY(24px)", filter: "blur(6px)" }, { opacity: 1, transform: "none", filter: "blur(0)" }], { duration: 420, easing: "cubic-bezier(0.2, 0.65, 0.3, 1)" });
    };
    if (scrub) scrub.addEventListener("input", function () { choose(Number(scrub.value)); });
    points.forEach(function (point) {
      point.addEventListener("click", function () { choose(Number(point.dataset.node)); });
    });
    surfacers.push(function (target) {
      var change = target.closest && target.closest(".changed > .version");
      if (change && scene.contains(change)) choose(number(change));
      return null;
    });
    // A reload keeps the change shown (task 1378), unless it was the newest:
    // then the newest, which the new version may have made another.
    keepers.history = {
      keep: function () { return { shown: shown, newest: shown === changes.length }; },
      back: function (was) { if (!was.newest && was.shown) choose(was.shown); }
    };
  })();

  // What's next (task 1377): the next step put on the stage, the command
  // copied, as the menu's Copy does, the button saying so for a moment, and
  // the way back to the top. Review is the pill's, as everywhere.
  (function () {
    var close = document.getElementById("next");
    if (!close) return;
    close.addEventListener("click", function (event) {
      var button = event.target.closest("button");
      if (!button) return;
      if (button.dataset.step) return openStep(button.dataset.step);
      if ("top" in button.dataset) return scrollTo({ top: 0, behavior: smooth() });
      if (!("copy" in button.dataset)) return;
      var command = document.getElementById("bar").dataset.command, label = button.dataset.label || (button.dataset.label = button.textContent);
      var tell = function (text) {
        button.textContent = text;
        setTimeout(function () { button.textContent = label; }, 1600);
      };
      var copied = navigator.clipboard ? navigator.clipboard.writeText(command) : Promise.reject();
      copied.then(function () { tell("Copied " + command); }, function () { tell("Not copied: the browser refused"); });
    });
  })();

  // The questions waiting on the user (task 1110), as ekko's menu shows
  // them: beside the options, why pick the one in focus or under the
  // pointer, else the one picked, an example and its preview. The arrow
  // keys move along the options, picking as they go where one answer is
  // taken. Where the page writes, an option is picked, or several where the
  // question allows, "Other answer..." writes another, a note may go with
  // it, and Answer records it with POST /api/answer, as the menu would;
  // what was picked and written is kept across a reload.
  (function () {
    var cards = Array.prototype.slice.call(document.querySelectorAll(".asks[data-question]"));
    if (!cards.length) return;
    var checked = function (option) { return option.getAttribute("aria-checked") === "true"; };
    var parts = function (card) {
      return { options: Array.prototype.slice.call(card.querySelectorAll(".option")), other: card.querySelector(".option.other"),
        text: card.querySelector(".other-text"), note: card.querySelector(".note-text") };
    };
    // The other answer's field, shown while "Other answer..." is picked,
    // and always on a question without options.
    function fold(card) {
      var got = parts(card);
      if (got.other) got.text.hidden = !checked(got.other);
    }
    cards.forEach(function (card) {
      var got = parts(card), options = got.options, other = got.other, text = got.text, note = got.note;
      var multiple = "multiple" in card.dataset, free = "free" in card.dataset;
      var button = card.querySelector("[data-answer]"), status = card.querySelector(".why-not"), sending = false;
      var show = function (option) {
        var n = option ? option.dataset.n : undefined;
        if (n === undefined) {
          var first = options.filter(function (each) { return checked(each) && each.dataset.n !== undefined; })[0];
          if (!first) return;
          n = first.dataset.n;
        }
        card.querySelectorAll(".aid").forEach(function (aid) { aid.hidden = aid.dataset.for !== n; });
      };
      // A page that reads only shows the aids, and picks nothing.
      var pick = function (option) {
        if (!writes()) return;
        if (multiple) option.setAttribute("aria-checked", checked(option) ? "false" : "true");
        else options.forEach(function (each) { each.setAttribute("aria-checked", each === option ? "true" : "false"); });
        fold(card);
        status.textContent = "";
      };
      options.forEach(function (option) {
        option.addEventListener("click", function () {
          pick(option);
          show(option);
          if (option === other && !text.hidden) text.focus();
        });
        option.addEventListener("focus", function () { show(option); });
        option.addEventListener("mouseenter", function () { show(option); });
        option.addEventListener("keydown", function (event) {
          var by = { ArrowDown: 1, ArrowRight: 1, ArrowUp: -1, ArrowLeft: -1 }[event.key];
          if (!by) return;
          event.preventDefault();
          var shown = options.filter(function (each) { return each.getClientRects().length; });
          var next = shown[(shown.indexOf(option) + by + shown.length) % shown.length];
          options.forEach(function (each) { each.tabIndex = each === next ? 0 : -1; });
          next.focus();
          if (!multiple && next !== other) pick(next);
        });
      });
      var group = card.querySelector(".options");
      if (group) group.addEventListener("mouseleave", function () { if (!group.contains(document.activeElement)) show(null); });
      var say = function (words) { status.textContent = words; };
      function answer() {
        if (sending) return;
        var labels = options.filter(function (each) { return checked(each) && each !== other; }).map(function (each) { return each.dataset.label; });
        var otherOn = free || (other && checked(other)), written = otherOn ? text.value.trim() : "";
        if (otherOn && !written) { text.focus(); return say(free ? "Write the answer first." : "Write the other answer first."); }
        if (!labels.length && !written) return say("Pick an answer first.");
        var posted = { page: location.pathname, question: card.dataset.question, picked: labels, other: written || undefined, note: note.value.trim() || undefined };
        sending = true;
        button.disabled = true;
        say("");
        fetch("/api/answer", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(posted) })
          .then(function (reply) {
            return reply.text().then(function (body) {
              var got = {};
              try { got = JSON.parse(body); } catch (e) { got = { why: body.trim() }; }
              if (!reply.ok) throw new Error(got.why || "the server answered " + reply.status);
              return got;
            });
          })
          .then(function (got) {
            card.classList.add("answered");
            say("Answered: " + got.answer);
          })
          .catch(function (error) {
            button.disabled = false;
            say("Not answered: " + (window.ekkoUnreached ? ekkoUnreached(error) : error.message));
          })
          .then(function () { sending = false; });
      }
      button.addEventListener("click", answer);
      [text, note].forEach(function (field) {
        field.addEventListener("keydown", function (event) {
          if (event.key === "Enter" && !event.shiftKey && !event.isComposing) {
            event.preventDefault();
            answer();
          }
        });
      });
    });
    // A reload keeps, for each question still open, what was picked and
    // written (task 1378, as the bar keeps its words).
    keepers.questions = {
      keep: function () {
        var kept = {};
        cards.forEach(function (card) {
          if (card.classList.contains("answered")) return;
          var got = parts(card);
          kept[card.dataset.question] = {
            picked: got.options.filter(checked).map(function (each) { return each === got.other ? null : each.dataset.label; }),
            other: got.text.value, note: got.note.value
          };
        });
        return kept;
      },
      back: function (kept) {
        cards.forEach(function (card) {
          var was = kept && kept[card.dataset.question];
          if (!was) return;
          var got = parts(card);
          got.options.forEach(function (each) {
            var on = was.picked.indexOf(each === got.other ? null : each.dataset.label) >= 0;
            each.setAttribute("aria-checked", on ? "true" : "false");
          });
          got.text.value = was.other || "";
          got.note.value = was.note || "";
          fold(card);
          var first = got.options.filter(checked)[0];
          if (first && first.dataset.n !== undefined) card.querySelectorAll(".aid").forEach(function (aid) { aid.hidden = aid.dataset.for !== first.dataset.n; });
        });
      }
    };
  })();

  // Start, on a step a session may take up now (task 1111), where the page
  // writes: POST /api/start sends the comment "Start this" on the step,
  // told to the sessions working the plan; the board's next version then
  // shows it sent in place of the button.
  Array.prototype.forEach.call(document.querySelectorAll("button[data-start]"), function (button) {
    var status = button.parentNode.querySelector(".status");
    button.addEventListener("click", function () {
      if (button.disabled) return;
      button.disabled = true;
      status.textContent = "";
      fetch("/api/start", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ page: location.pathname, step: button.dataset.start }) })
        .then(function (reply) {
          return reply.text().then(function (body) {
            var got = {};
            try { got = JSON.parse(body); } catch (e) { got = { why: body.trim() }; }
            if (!reply.ok) throw new Error(got.why || "the server answered " + reply.status);
            return got;
          });
        })
        .then(function (got) { status.textContent = "Start sent in comment " + got.id + ": waiting for a session to take it up"; })
        .catch(function (error) {
          button.disabled = false;
          status.textContent = "Not started: " + (window.ekkoUnreached ? ekkoUnreached(error) : error.message);
        });
    });
  });

  // AKQA's bar (task 1223, measured on akqa.com): a pill a click unfolds
  // into a panel, which finds a part, a step, a note or a comment of the
  // page, and runs the page's commands. The surface's width and radius, its
  // folds and its menu's lines move as AKQA's do, by the springs and tweens
  // Motion runs there. Beside it, Review is a round button of its glass.
  var bar = document.getElementById("bar"), surface = bar.querySelector(".bar-surface"), side = bar.querySelector(".bar-side");
  var input = bar.querySelector("input"), hint = bar.querySelector(".bar-hint"), list = bar.querySelector(".bar-list");
  var listFold = bar.querySelector(".list-fold"), navFold = bar.querySelector(".nav-fold"), menu = bar.querySelector(".bar-menu");
  var quoteFold = bar.querySelector(".quote-fold"), quoteText = bar.querySelector(".bar-quote-text"), why = bar.querySelector(".bar-why");
  var note = bar.querySelector(".bar-note"), line = bar.querySelector(".bar-line"), hairline = bar.querySelector(".bar-hairline");
  var themesRow = bar.querySelector(".bar-themes"), send = bar.querySelector(".bar-send");
  // The plan's words a comment is being written on, and whether it is on
  // its way to the board.
  var quoting = null, sending = false;
  // Its theme, the comment it edits if it edits one, and whether the
  // themes are being renamed.
  var color = null, editing = null, renaming = false;
  // A review being written (task 1106): its verdict, and whether Approve
  // was pressed once, its tasks listed for the second press to confirm.
  var reviewing = null, confirming = false, tell = bar.querySelector(".bar-tell"), drop = bar.querySelector(".bar-quote-drop");
  // Whether the comment suggests words in place of the quoted ones (task
  // 1107), and the field's other text: what was typed as the comment while
  // it suggests, the words it suggests while it does not, null until then.
  var suggest = bar.querySelector(".bar-suggest"), suggesting = false, said = "", put = null;
  // Whether the comment being edited is a suggestion.
  function editsSuggestion() { return !!editing && typeof editing.replacement === "string"; }
  function writing() { return !!(quoting || reviewing); }
  // The page's commands (task 1337), which a top bar held before: listed
  // first among what the words find, as GitHub's command palette suggests
  // them, or alone after ">", as in its command mode. The review's are the
  // person's alone; the menu offers those marked `menu` too.
  function writes() { return "writes" in document.body.dataset; }
  var COMMANDS = [
    { label: "Review", words: "review send pending comments verdict", writes: true, run: function () { review(); } },
    { label: "Approve", words: "approve plan review", writes: true, when: function () { return !!(asked && asked.approve); }, run: function () { review("approve"); } },
    { label: "Request changes", words: "request changes review", writes: true, run: function () { review("changes"); } },
    { label: otherTheme, words: "theme dark light mode switch", menu: true, run: function () { switchTheme(); close(true); acts(); } },
    { label: function () { return "Copy " + bar.dataset.command; }, short: "Copy command", words: "copy command terminal", menu: true, run: function () {
      close(true);
      var copied = navigator.clipboard ? navigator.clipboard.writeText(bar.dataset.command) : Promise.reject();
      copied.then(function () { say("Copied " + bar.dataset.command); }, function () { say("Not copied: the browser refused"); });
    } }
  ];
  function commands() {
    return COMMANDS.filter(function (command) { return (!command.writes || writes()) && (!command.when || command.when()); }).map(function (command) {
      var label = typeof command.label === "function" ? command.label() : command.label;
      return { kind: "Command", badge: "\u203a", label: label, short: command.short || label, menu: command.menu, run: command.run, search: (label + " " + command.words).toLowerCase() };
    });
  }
  var items = parts.map(function (part) {
    return { kind: "Section", badge: "\u00a7", label: part.dataset.part, target: part.id, search: ("section " + part.dataset.part).toLowerCase() };
  });
  // Each finds by the names the list shows it under too (task 1361): a
  // step by its place, "05", "5" or "step 5"; a note or a comment by its
  // number, alone or after its kind, "1321" or "question 1321".
  document.querySelectorAll(".step").forEach(function (step) {
    var key = step.id.slice(5), title = step.querySelector(".title").textContent, more = step.querySelector(".more");
    var badge = step.querySelector(".num").textContent, place = String(Number(badge));
    items.push({ kind: "Step \u00b7 " + step.dataset.state, badge: badge, label: title, step: key, names: [badge, place, "step " + place, "step " + badge],
      search: ("step steps " + badge + " " + key + " " + title + " " + step.querySelector(".meta").textContent + " " + (more ? more.textContent : "")).toLowerCase() });
  });
  document.querySelectorAll(".note").forEach(function (note) {
    var kind = note.dataset.kind, number = note.id.slice(5), title = note.querySelector("h3").textContent, answer = note.querySelector(".answer");
    var text = Array.prototype.map.call(note.querySelectorAll(".text"), function (part) { return part.textContent; }).join(" ");
    items.push({ kind: kind + " " + number, badge: kind.charAt(0), label: title, target: note.id, names: [number, (kind + " " + number).toLowerCase(), "note " + number],
      search: ("note notes " + kind + " " + kind + "s " + number + " " + title + " " + (answer ? answer.textContent : "") + " " + text).toLowerCase() });
  });
  // The findings of What is known, by their tab's name and their words
  // (task 1378): a jump to one picks its tab and opens it.
  document.querySelectorAll(".scene.known .card").forEach(function (card) {
    var group = card.closest(".group"), tab = group.id ? document.querySelector("[aria-controls='" + group.id + "']") : null;
    var name = tab ? tab.firstChild.textContent : "", lead = card.querySelector(".lead"), body = card.querySelector(".body").textContent.replace(/\s+/g, " ").trim();
    items.push({ kind: "Finding" + (name ? " · " + name : ""), badge: card.querySelector(".num").textContent.split("/")[0].trim(), label: lead ? lead.textContent : body, target: card.id,
      search: ("finding findings " + name + " " + body).toLowerCase() });
  });
  // The comments, by their theme, state, words and text (task 1213).
  document.querySelectorAll("#comments .entry").forEach(function (entry) {
    var theme = entry.querySelector(".theme").textContent, text = entry.querySelector(".text").textContent, said = entry.querySelector(".said, .suggests");
    items.push({ kind: "Comment " + entry.dataset.id + " \u00b7 " + theme, badge: theme.charAt(0), label: text, target: entry.id, comment: Number(entry.dataset.id), names: [entry.dataset.id, "comment " + entry.dataset.id],
      search: ("comment comments " + entry.dataset.id + " " + theme + " " + entry.dataset.state + " " + text + " " + (said ? said.textContent : "")).toLowerCase() });
  });
  function find(text) {
    var only = text.charAt(0) === ">", words = (only ? text.slice(1) : text).toLowerCase().split(/\s+/).filter(Boolean);
    var hit = function (item) { return words.every(function (word) { return item.search.indexOf(word) >= 0; }); };
    var found = commands().filter(hit);
    if (only) return found;
    // What the words name comes first, then what they are found in.
    var typed = words.join(" "), named = function (item) { return !!item.names && item.names.indexOf(typed) >= 0; };
    var hits = items.filter(hit);
    return found.concat(hits.filter(named), hits.filter(function (item) { return !named(item); }));
  }
  // The suggestions the pill cycles through, each with what it looks for:
  // one that would find nothing on this page is not offered.
  var hints = [["Steps in progress", "in progress"], ["Open questions", "open question"], ["Jump to a step", "step"], ["Show the map", "map"], ["What the design says", "design"]]
    .filter(function (hint) { return find(hint[1]).length > 0; });
  if (!hints.length) hints = [["Jump to a section", "section"]];
  // The suggestion showing, the item picked by the arrows (none at first,
  // as in AKQA's list), what the list shows, whether the bar is a panel,
  // and whether a pointer is what is focusing it.
  var shown = 0, selected = -1, matches = [], expanded = false, pointed = false;

  // Motion's animations (motion.dev), as AKQA's bar runs them: a spring by
  // its stiffness, damping and mass, or by the time it seems to take and its
  // bounce; a tween by its duration and cubic Bezier. A value sent somewhere
  // new leaves from where it is, at the speed it has. The settings are
  // AKQA's: its surface's spring, its menu lines', its panels'.
  var SURFACE = { stiffness: 270, damping: 24, mass: 1.9 };
  var LINES = { stiffness: 380, damping: 26, mass: 1.9 };
  var EASE = [0.2, 0.65, 0.3, 1];
  var still = matchMedia("(prefers-reduced-motion: reduce)");
  function panel(delay) { return { visualDuration: 0.5, bounce: 0, delay: delay }; }
  function bezier(x1, y1, x2, y2) {
    var at = function (t, a, b) { return (((1 - 3 * b + 3 * a) * t + (3 * b - 6 * a)) * t + 3 * a) * t; };
    return function (x) {
      if (x <= 0 || x >= 1) return x <= 0 ? 0 : 1;
      var low = 0, high = 1, t, off, i = 0;
      do { t = low + (high - low) / 2; off = at(t, x1, x2) - x; if (off > 0) high = t; else low = t; } while (Math.abs(off) > 1e-7 && ++i < 12);
      return at(t, y1, y2);
    };
  }
  // Where a spring is t ms after it left `from` at `speed` a second.
  function spring(how, from, to, speed) {
    var k = how.stiffness, c = how.damping, m = how.mass;
    if (how.visualDuration !== undefined) {
      var root = 2 * Math.PI / (how.visualDuration * 1.2);
      k = root * root;
      c = 2 * Math.min(Math.max(1 - (how.bounce || 0), 0.05), 1) * Math.sqrt(k);
      m = 1;
    }
    var zeta = c / (2 * Math.sqrt(k * m)), w0 = Math.sqrt(k / m) / 1000, delta = to - from, v0 = -speed / 1000;
    if (zeta < 1) {
      var wd = w0 * Math.sqrt(1 - zeta * zeta);
      return function (t) { return to - Math.exp(-zeta * w0 * t) * ((v0 + zeta * w0 * delta) / wd * Math.sin(wd * t) + delta * Math.cos(wd * t)); };
    }
    if (zeta === 1) return function (t) { return to - Math.exp(-w0 * t) * (delta + (v0 + w0 * delta) * t); };
    var wh = w0 * Math.sqrt(zeta * zeta - 1);
    return function (t) {
      var f = Math.min(wh * t, 300);
      return to - Math.exp(-zeta * w0 * t) * ((v0 + zeta * w0 * delta) * Math.sinh(f) + wh * delta * Math.cosh(f)) / wh;
    };
  }
  // One value the bar moves, drawn by `apply` each frame; `rest` runs once
  // it settles. All of them share one frame loop.
  var moving = [], ticking = false;
  function tick(now) {
    var was = moving;
    moving = [];
    was.forEach(function (motion) { if (motion.step(now) && moving.indexOf(motion) < 0) moving.push(motion); });
    ticking = moving.length > 0;
    if (ticking) requestAnimationFrame(tick);
  }
  function Motion(value, apply, rest) { this.value = this.target = value; this.apply = apply; this.rest = rest; this.run = null; apply(value); }
  Motion.prototype.speed = function (now) {
    var run = this.run, t = run ? now - run.start : 0;
    if (t <= 0) return 0;
    var back = Math.max(t - 5, 0);
    return (run.at(t) - run.at(back)) / (t - back) * 1000;
  };
  Motion.prototype.to = function (target, how) {
    if (target === this.target) return;
    var now = performance.now(), from = this.value, speed = this.speed(now), at, done;
    this.target = target;
    if (still.matches) how = { duration: 0.15 };
    if (how.duration !== undefined) {
      var ease = bezier.apply(null, how.ease || [0.42, 0, 0.58, 1]), span = how.duration * 1000;
      at = function (t) { return from + (target - from) * ease(Math.min(t / span, 1)); };
      done = function (t) { return t >= span; };
    } else {
      at = spring(how, from, target, speed);
      // Motion's rest: within half a unit and slower than 2 a second, or
      // a hundredth of that for a move under 5 units.
      var fine = Math.abs(target - from) < 5, slow = fine ? 0.01 : 2, near = fine ? 0.005 : 0.5;
      done = function (t) {
        var back = Math.max(t - 5, 0);
        return Math.abs(target - at(t)) <= near && Math.abs((at(t) - at(back)) / (t - back || 1) * 1000) <= slow;
      };
    }
    this.run = { at: at, done: done, start: now + (how.delay || 0) * 1000 };
    if (moving.indexOf(this) < 0) moving.push(this);
    if (!ticking) { ticking = true; requestAnimationFrame(tick); }
  };
  Motion.prototype.step = function (now) {
    var run = this.run;
    if (!run) return false;
    var t = now - run.start;
    if (t < 0) return true;
    if (run.done(t)) {
      this.run = null;
      this.value = this.target;
      this.apply(this.value);
      if (this.rest) this.rest();
      return false;
    }
    this.value = run.at(t);
    this.apply(this.value);
    return true;
  };
  // Where it is heading at once, with no motion.
  Motion.prototype.set = function (value) {
    this.run = null;
    this.value = this.target = value;
    this.apply(value);
  };

  // The pill's width and the panel's, AKQA's on a desktop and its phone's
  // margins on a narrow window; and, with Review beside it (12px off, as
  // its style sets), how far left the pill moves where the window cannot
  // hold the two with the pill centred: the two together are centred then,
  // within the same margins.
  function aside() { return side && side.getClientRects().length ? side.offsetWidth + 12 : 0; }
  function widths() {
    var room = aside(), slim = Math.min(320, innerWidth - 46 - room);
    return { wide: innerWidth < 720 ? innerWidth - 24 : 600, slim: slim, shift: -Math.min(room / 2, Math.max(0, (innerWidth + slim) / 2 + room - (innerWidth - 23))) };
  }
  var lines = menu.querySelectorAll("span"), turn = { y: 3.5, r: 0 };
  function cross() {
    lines[0].style.transform = "translateY(" + -turn.y + "px) rotate(" + turn.r + "deg)";
    lines[1].style.transform = "translateY(" + turn.y + "px) rotate(" + -turn.r + "deg)";
  }
  // A field written in while the pill still widens wraps at the width it
  // has then: once wide, it is measured again.
  var width = new Motion(widths().slim, function (v) { surface.style.width = v + "px"; }, function () { if (writing()) grow(); });
  var shift = new Motion(0, function (v) { bar.style.transform = "translateX(calc(-50% + " + v + "px))"; });
  var radius = new Motion(100, function (v) { surface.style.borderRadius = v + "px"; });
  var lineY = new Motion(3.5, function (v) { turn.y = v; cross(); });
  var lineR = new Motion(0, function (v) { turn.r = v; cross(); });
  function Fold(element) {
    this.element = element;
    this.height = new Motion(0, function (v) { element.style.height = v + "px"; });
    this.opacity = new Motion(0, function (v) { element.style.opacity = v; });
  }
  var folds = { list: new Fold(listFold), quote: new Fold(quoteFold), nav: new Fold(navFold) }, rule = new Fold(hairline);

  // Where the bar is heading: the pill, or the panel with what it holds.
  // A fold's height is a tween, slower open than shut and a moment late
  // open; its opacity, and the rule's, the panels' spring, a moment late
  // while the bar has the focus. Each fold's content lays out at the
  // panel's width from the first frame, so it is measured at the height it
  // ends at (and its scrollHeight is never read: note 1216).
  function size() {
    var w = widths(), delay = bar.contains(document.activeElement) ? 0.08 : 0;
    Array.prototype.forEach.call(bar.querySelectorAll(".fold-in"), function (inner) { inner.style.width = w.wide + "px"; });
    width.to(expanded ? w.wide : w.slim, SURFACE);
    shift.to(expanded ? 0 : w.shift, SURFACE);
    radius.to(expanded ? 20 : 100, SURFACE);
    lineY.to(expanded ? 0 : 3.5, LINES);
    lineR.to(expanded ? 45 : 0, LINES);
    menu.setAttribute("aria-expanded", expanded ? "true" : "false");
    unfold(folds.list, expanded && !writing() && matches.length > 0, delay);
    unfold(folds.quote, expanded && writing(), delay);
    unfold(folds.nav, expanded && !writing(), delay);
    rule.height.to(expanded ? 1 : 0, panel(delay));
    rule.opacity.to(expanded ? 1 : 0, panel(delay));
  }
  function unfold(fold, open, delay) {
    fold.element.inert = !open;
    fold.height.to(open ? fold.element.firstElementChild.getBoundingClientRect().height : 0, open ? { duration: 0.55, ease: EASE, delay: 0.08 } : { duration: 0.32, ease: EASE });
    fold.opacity.to(open ? 1 : 0, panel(delay));
  }

  // The hint, AKQA's prompt cycler: a suggestion's words come in one after
  // another from a blur, 35 ms apart, and leave the same way 20 ms apart;
  // the next starts to leave 3.2 s after the last did, while the bar is
  // shut and empty. Each word but the last ends in a no-break space, which
  // the end of its inline block keeps where it drops a space.
  var cycle = 0, leaving = 0, saying = false;
  function words(text, enter) {
    hint.textContent = "";
    text.split(" ").forEach(function (word, i, all) {
      var span = document.createElement("span");
      span.textContent = word + (i < all.length - 1 ? "\u00a0" : "");
      if (enter) { span.className = "out"; span.style.transitionDelay = 0.035 * i + "s"; }
      hint.appendChild(span);
    });
    if (!enter) return;
    void hint.offsetWidth;
    Array.prototype.forEach.call(hint.children, function (span) { span.className = ""; });
  }
  function idle() { return !expanded && !input.value && !writing() && !still.matches && hints.length > 1; }
  // The next comes in a frame after the last word is gone, as AKQA's does
  // once its exit has finished (measured: 17 to 26 ms); a word hidden
  // meanwhile ends no transition, so a timer stands in for it.
  function advance() {
    clearTimeout(cycle);
    clearTimeout(leaving);
    saying = false;
    shown = (shown + 1) % hints.length;
    var spans = hint.children, last = spans[spans.length - 1];
    var next = function () { requestAnimationFrame(function () { if (leaving) { leaving = 0; words(hints[shown][0], true); } }); };
    Array.prototype.forEach.call(spans, function (span, i) { span.style.transitionDelay = 0.02 * i + "s"; span.className = "out"; });
    leaving = setTimeout(next, 400 + 20 * spans.length);
    if (last) last.addEventListener("transitionend", function gone(event) {
      if (event.propertyName !== "opacity") return;
      last.removeEventListener("transitionend", gone);
      clearTimeout(leaving);
      next();
    });
    else next();
    if (idle()) cycle = setTimeout(advance, 3200);
  }
  // Open, the suggestion showing stays, whole; shut and empty again, the
  // next comes soon after, and the cycle goes on.
  function hold() {
    clearTimeout(cycle);
    if (leaving || saying) { clearTimeout(leaving); leaving = 0; saying = false; words(hints[shown][0], false); }
  }
  function resume(after) {
    clearTimeout(cycle);
    if (idle()) cycle = setTimeout(advance, after);
  }
  // What a command did, said in the pill where the suggestions show, until
  // the next suggestion takes its place.
  function say(text) {
    hold();
    words(text, true);
    saying = true;
    cycle = setTimeout(advance, 2400);
  }
  // The commands the menu offers besides the parts, as chips of their own.
  var actsRow = bar.querySelector(".bar-acts");
  function acts() {
    actsRow.textContent = "";
    commands().filter(function (command) { return command.menu; }).forEach(function (command) {
      var chip = document.createElement("button");
      chip.type = "button";
      chip.className = "bar-act";
      chip.textContent = command.short;
      chip.addEventListener("mousedown", function (event) { event.preventDefault(); });
      chip.addEventListener("click", function () { command.run(); });
      actsRow.appendChild(chip);
    });
  }
  function typed() { bar.classList.toggle("typed", !!input.value); }

  // What the input looks for: a suggestion's own search when it holds the
  // suggestion's words, as AKQA's prompts carry theirs.
  function query() {
    var text = input.value.trim();
    for (var i = 0; i < hints.length; i++) if (hints[i][0] === text) return hints[i][1];
    return text;
  }
  // AKQA's picture for a result without one: a gradient its title's hash picks.
  function tint(text) {
    var hash = 0x811c9dc5;
    for (var i = 0; i < text.length; i++) hash = Math.imul(hash ^ text.charCodeAt(i), 0x1000193);
    var hue = Math.abs(hash) % 360;
    return "linear-gradient(135deg, hsl(" + hue + " 35% 28%), hsl(" + (hue + 40) % 360 + " 45% 18%))";
  }
  // The list, an item coming in from a blur the first time it is listed.
  var listed = {};
  function render() {
    var text = writing() ? "" : query(), had = listed;
    matches = text ? find(text).slice(0, 40) : [];
    selected = -1;
    listed = {};
    list.textContent = "";
    matches.forEach(function (item) {
      var key = item.kind + "\n" + item.label, button = document.createElement("button");
      button.type = "button";
      button.className = had[key] ? "item" : "item in";
      button.setAttribute("role", "option");
      button.setAttribute("aria-selected", "false");
      button.innerHTML = '<span class="badge"></span><span class="what"><span class="kind"></span><span class="label"></span></span>';
      var badge = button.querySelector(".badge");
      badge.textContent = item.badge;
      if (item.run) button.classList.add("command");
      else badge.style.backgroundImage = tint(item.label);
      button.querySelector(".kind").textContent = item.kind;
      button.querySelector(".label").textContent = item.label;
      // The input keeps the focus, as in AKQA's list.
      button.addEventListener("mousedown", function (event) { event.preventDefault(); });
      button.addEventListener("click", function () { go(item); });
      listed[key] = true;
      list.appendChild(button);
    });
    size();
  }
  function choose(i) {
    selected = i;
    Array.prototype.forEach.call(list.children, function (button, j) {
      button.classList.toggle("sel", j === i);
      button.setAttribute("aria-selected", j === i ? "true" : "false");
    });
    if (list.children[i]) list.children[i].scrollIntoView({ block: "nearest" });
  }

  // Open, as AKQA's bar opens: a click on the empty pill fills it with the
  // suggestion showing, picked, so the first key typed takes its place;
  // otherwise what it holds is picked.
  function open(fill) {
    hold();
    if (fill && !input.value && !writing()) input.value = hints[shown][0];
    var was = expanded, field = writing() ? note : input;
    expanded = true;
    bar.classList.add("open");
    typed();
    if (document.activeElement !== field) field.focus({ preventScroll: true });
    // Picked again a frame later, as AKQA does, past what the press that
    // focused it does to the selection; not once a key has changed it in
    // that frame, or the next key would replace what was typed (task 1363).
    if (!writing()) {
      input.select();
      var picked = input.value;
      requestAnimationFrame(function () { if (document.activeElement === input && expanded && input.value === picked) input.select(); });
    }
    if (!was || fill) render(); else size();
  }
  // Shut, back to the pill: Esc and a click elsewhere keep the text, the
  // menu's cross and a jump clear it (task 1223: a part's name left in the
  // field had to be erased before anything else could be typed).
  function close(clear) {
    if (clear) input.value = "";
    expanded = false;
    bar.classList.remove("open");
    typed();
    input.blur();
    note.blur();
    if (clear) render(); else size();
    resume(450);
  }
  function go(item) {
    // A command runs from the panel, which it shuts or turns into its own.
    if (item.run) {
      input.value = "";
      typed();
      item.run();
      return;
    }
    close(true);
    if (item.comment && window.ekkoOpenComment) ekkoOpenComment(item.comment);
    else if (item.step) openStep(item.step);
    else jump(item.target);
  }

  // A comment on the plan's words (tasks 1105 and 1213), written the way
  // one replies to part of an answer in Claude: the words picked come into
  // the pill, quoted above the themes and its line, which becomes a field of
  // its own that grows with the comment; Enter sends it to the board,
  // Shift+Enter starts a line. The words
  // stay marked in the text, in the theme's color, by ekkoMark. An edit of
  // a comment is written the same way, over its text.
  function themes() { return window.ekkoThemes || { list: [["yellow", "Note"]], names: function () { return { yellow: "Note" }; }, last: function () { return "yellow"; }, setLast: function () {}, rename: function () {} }; }
  function chips() {
    themesRow.textContent = "";
    var named = themes().names();
    themes().list.forEach(function (theme) {
      var key = theme[0];
      if (renaming) {
        var field = document.createElement("input");
        field.className = "bar-theme-name";
        field.value = named[key];
        field.maxLength = 40;
        field.dataset.color = key;
        field.setAttribute("aria-label", "Name of the " + key + " theme");
        field.style.setProperty("--ink", "var(--ink-" + key + ")");
        themesRow.appendChild(field);
        return;
      }
      var chip = document.createElement("button");
      chip.type = "button";
      chip.className = "bar-theme";
      chip.dataset.color = key;
      chip.setAttribute("role", "radio");
      chip.setAttribute("aria-checked", key === color ? "true" : "false");
      chip.title = "Comment as " + named[key];
      chip.style.setProperty("--ink", "var(--ink-" + key + ")");
      chip.textContent = named[key];
      themesRow.appendChild(chip);
    });
    var edit = document.createElement("button");
    edit.type = "button";
    edit.className = "bar-theme-edit";
    edit.textContent = renaming ? "Done" : "Rename";
    edit.title = renaming ? "Keep these names" : "Name the themes as you use them";
    themesRow.appendChild(edit);
  }
  function pick(key) {
    color = key;
    quoteFold.style.setProperty("--ink", "var(--ink-" + key + ")");
    Array.prototype.forEach.call(themesRow.querySelectorAll(".bar-theme"), function (chip) {
      chip.setAttribute("aria-checked", chip.dataset.color === key ? "true" : "false");
    });
    if (window.ekkoMark) ekkoMark(quoting, key);
  }
  function renamed(keep) {
    if (keep) {
      var named = {};
      Array.prototype.forEach.call(themesRow.querySelectorAll(".bar-theme-name"), function (field) { named[field.dataset.color] = field.value; });
      themes().rename(named);
    }
    renaming = false;
    chips();
    size();
    note.focus();
  }
  themesRow.addEventListener("click", function (event) {
    var verdict = event.target.closest(".bar-verdict");
    if (verdict && reviewing) {
      reviewing.verdict = verdict.dataset.verdict;
      confirming = false;
      verdicts();
      told();
      size();
      note.focus();
      return;
    }
    var chip = event.target.closest(".bar-theme"), edit = event.target.closest(".bar-theme-edit");
    if (chip) { pick(chip.dataset.color); note.focus(); }
    if (!edit) return;
    if (renaming) return renamed(true);
    renaming = true;
    chips();
    size();
    themesRow.querySelector(".bar-theme-name[data-color=\"" + color + "\"]").select();
  });
  themesRow.addEventListener("keydown", function (event) {
    if (!event.target.matches(".bar-theme-name")) return;
    if (event.key === "Enter") { event.preventDefault(); renamed(true); }
    if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); renamed(false); }
  });
  function quote(words, key, edited) {
    if (reviewing) unreview(false);
    if (editing && !edited) note.value = "";
    quoting = words;
    editing = edited || null;
    renaming = false;
    unsuggest();
    suggest.hidden = !!editing && !editsSuggestion();
    quoteText.textContent = words.exact;
    why.textContent = "";
    input.hidden = true;
    note.hidden = false;
    menu.hidden = true;
    bar.classList.add("quoting");
    send.hidden = false;
    note.placeholder = editing ? "Edit the comment" : "Comment on these words";
    send.setAttribute("aria-label", editing ? "Save the comment" : "Send the comment");
    send.title = editing ? "Save (Enter)" : "Send (Enter)";
    chips();
    pick(key || themes().last());
    grow();
    open(false);
  }
  window.ekkoQuote = function (words, key) { quote(words, key, null); };
  window.ekkoEdit = function (target) {
    quote(target.quote || { exact: "" }, target.color, target);
    note.value = target.text;
    // A suggestion opens on its words, and its text, when it said only
    // what it suggests, is not offered to edit.
    if (editsSuggestion()) {
      note.value = target.told ? "" : target.text;
      put = target.replacement;
      suggestion(true);
    }
    grow();
  };
  // A suggestion, as GitHub's suggestion block starts from the lines
  // selected: the field holds the quoted words, which the person changes
  // into the words to put in their place; emptied, it suggests deleting
  // them (task 1107, decision 1267).
  function suggestion(on) {
    if (on === suggesting || !quoting || (editing && !editsSuggestion())) return;
    suggesting = on;
    var other = note.value;
    note.value = on ? (put === null ? quoting.exact : put) : said;
    if (on) said = other; else put = other;
    suggest.setAttribute("aria-pressed", on ? "true" : "false");
    bar.classList.toggle("suggesting", on);
    note.placeholder = on ? "Nothing here deletes the words" : "Comment on these words";
    note.setAttribute("aria-label", on ? "The words to put in place of the quoted ones" : "Comment on the quoted words");
    send.setAttribute("aria-label", on ? "Send the suggestion" : "Send the comment");
    why.textContent = "";
    grow();
    note.focus();
  }
  function unsuggest() {
    suggesting = false;
    said = "";
    put = null;
    suggest.setAttribute("aria-pressed", "false");
    bar.classList.remove("suggesting");
    note.setAttribute("aria-label", "Comment on the quoted words");
  }
  suggest.addEventListener("click", function () { suggestion(!suggesting); });
  function unquote() {
    quoting = null;
    editing = null;
    renaming = false;
    unsuggest();
    color = null;
    why.textContent = "";
    quoteText.textContent = "";
    themesRow.textContent = "";
    note.value = "";
    note.hidden = true;
    input.hidden = false;
    send.hidden = true;
    menu.hidden = false;
    bar.classList.remove("quoting");
    grow();
    if (window.ekkoMark) ekkoMark(null);
  }
  // The field one line high until the comment needs more. Empty, its rows
  // set it: a placeholder measured while the pill still widens would wrap,
  // and keep the field two lines high once it is wide.
  function grow() {
    note.style.height = "auto";
    if (!note.hidden && note.value) note.style.height = note.scrollHeight + "px";
    send.disabled = sending || (!reviewing && !suggesting && !editsSuggestion() && !note.value.trim());
    size();
  }
  function refused(text) {
    why.textContent = text;
    size();
  }
  function comment() {
    var flat = function (text) { return text.replace(/\s+/g, " ").trim(); };
    // A suggestion's words, and its text, which may be empty: then the
    // server says what it suggests.
    var suggests = suggesting || editsSuggestion();
    var text = suggesting ? said : note.value, words = suggesting ? note.value.trim() : (put === null ? editing && editing.replacement : put.trim());
    if (!suggests && !text.trim()) return refused("Write the comment first.");
    if (suggests && flat(words) === flat(quoting.exact)) return refused("Change the words first: these are the plan's.");
    var article = document.querySelector("article.prose"), name = themes().names()[color];
    var path = editing ? "/api/comment/edit" : "/api/comment";
    var made = { version: Number(article.dataset.version), quote: quoting, theme: name, color: color };
    if (suggests) made.replacement = words;
    var posted = editing
      ? { page: location.pathname, uid: editing.uid, text: text.trim() ? text : undefined, theme: name, color: color, replacement: suggests ? words : undefined }
      : { page: location.pathname, text: text, comment: made };
    // Sent, it is no longer kept across a reload: the write itself makes
    // the new version that reloads the page.
    sending = true;
    note.readOnly = true;
    send.disabled = true;
    why.textContent = "";
    fetch(path, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(posted) })
      .then(window.ekkoAnswered || function (answer) { return { ok: answer.ok, why: "the server answered " + answer.status }; })
      .then(function (got) {
        if (!got.ok) throw new Error(got.why);
        themes().setLast(color);
        close(false);
        unquote();
      })
      .catch(function (error) { refused("Not written: " + (window.ekkoUnreached ? ekkoUnreached(error) : error.message)); })
      .then(function () {
        sending = false;
        note.readOnly = false;
        grow();
      });
  }
  function sent() {
    if (sending) return;
    if (reviewing) submit(); else comment();
  }
  drop.addEventListener("click", function () {
    if (reviewing) unreview(false); else unquote();
    open(false);
  });
  send.addEventListener("click", sent);
  note.addEventListener("input", function () {
    if (confirming) { confirming = false; told(); }
    grow();
  });
  note.addEventListener("keydown", function (event) {
    if (event.key === "Escape") {
      if (reviewing) unreview(false); else unquote();
      close(false);
      return;
    }
    if (event.key === "Enter" && !event.shiftKey && !event.isComposing) {
      event.preventDefault();
      sent();
    }
  });

  // A review (task 1106), written in the pill as GitHub's is in its box:
  // what it says, and its verdict, Comment, Approve or Request changes. It
  // sends every comment of the person's still pending. Approve answers the
  // question asking to approve this version once a second press confirms
  // the tasks it lists; Request changes answers the newest such question
  // the other way. What it says is kept while a comment is written.
  var reviewNode = document.getElementById("review-data");
  var asked = reviewNode ? JSON.parse(reviewNode.textContent) : null;
  var VERDICTS = [["comment", "Comment"], ["approve", "Approve"], ["changes", "Request changes"]];
  var summary = "", chosen = "comment", DOT = " " + String.fromCharCode(183) + " ";
  function pendingMine() { return window.ekkoPending ? ekkoPending() : []; }
  // Why Approve cannot be pressed when no question asks for it: in its
  // title, and said in the fold too, since a title shows to a pointer only
  // (task 1349).
  function unapproved() {
    return asked.changes
      ? "Question " + asked.changes.id + " asks to approve version " + asked.changes.version + ": the session asks again"
      : "No question asks to approve this plan: the session asks when it is ready";
  }
  function verdicts() {
    themesRow.textContent = "";
    themesRow.setAttribute("aria-label", "Verdict");
    VERDICTS.forEach(function (verdict) {
      var chip = document.createElement("button");
      chip.type = "button";
      chip.className = "bar-verdict";
      chip.dataset.verdict = verdict[0];
      chip.setAttribute("role", "radio");
      chip.setAttribute("aria-checked", verdict[0] === reviewing.verdict ? "true" : "false");
      chip.textContent = verdict[1];
      if (verdict[0] === "approve" && !asked.approve) {
        chip.disabled = true;
        chip.title = unapproved();
        chip.setAttribute("aria-describedby", "unapproved");
      }
      themesRow.appendChild(chip);
    });
  }
  // What the review sends and answers, said in the fold: Approve's tasks
  // listed, and once it was pressed, what the next press does; or, when
  // Approve cannot be pressed, why.
  function told() {
    var pending = pendingMine().length;
    quoteText.textContent = "Review of version " + asked.version + DOT + (pending === 1 ? "1 pending comment" : (pending || "No") + " pending comments");
    tell.textContent = "";
    var say = function (text, className) {
      var line = document.createElement("p");
      line.textContent = text;
      if (className) line.className = className;
      tell.appendChild(line);
    };
    if (reviewing.verdict === "approve" && asked.approve) {
      var tasks = asked.approve.tasks;
      say("Approve answers question " + asked.approve.id + " with \"" + asked.approve.answers + "\" and makes " + (tasks.length === 1 ? "this task:" : "these " + tasks.length + " tasks:"));
      var list = document.createElement("ul");
      tasks.forEach(function (task) {
        var item = document.createElement("li");
        item.textContent = task;
        list.appendChild(item);
      });
      if (tasks.length) tell.appendChild(list);
      if (confirming) say("Send again to approve.", "confirm");
    } else if (reviewing.verdict === "changes") {
      say(asked.changes
        ? "Request changes answers question " + asked.changes.id + " with \"" + asked.changes.answers + "\", naming this review."
        : "No question waits on an answer: the review goes to the board as it is.");
    } else {
      say("Comment sends the pending comments, and answers no question.");
    }
    if (!asked.approve) {
      say(unapproved() + ".");
      tell.lastChild.id = "unapproved";
    }
  }
  function review(verdict) {
    if (!asked) return;
    if (quoting && note.value.trim()) {
      note.focus();
      return refused("Send or drop the comment first.");
    }
    if (quoting) unquote();
    var picked = typeof verdict === "string" ? verdict : reviewing ? reviewing.verdict : chosen;
    reviewing = { verdict: picked === "approve" && !asked.approve ? "comment" : picked };
    confirming = false;
    why.textContent = "";
    input.hidden = true;
    note.hidden = false;
    menu.hidden = true;
    bar.classList.add("quoting", "reviewing");
    send.hidden = false;
    note.value = summary;
    note.placeholder = "What the review says";
    note.setAttribute("aria-label", "What the review says");
    drop.setAttribute("aria-label", "Close the review");
    send.setAttribute("aria-label", "Send the review");
    send.title = "Send the review (Enter)";
    quoteFold.style.setProperty("--ink", "var(--bar-ink-muted)");
    verdicts();
    told();
    grow();
    open(false);
  }
  window.ekkoReview = review;
  // Sent, what it said and its verdict go with it; given up, both are
  // kept for the next.
  function unreview(sent) {
    summary = sent ? "" : note.value;
    chosen = sent || !reviewing ? "comment" : reviewing.verdict;
    reviewing = null;
    confirming = false;
    why.textContent = "";
    quoteText.textContent = "";
    themesRow.textContent = "";
    themesRow.setAttribute("aria-label", "Theme");
    tell.textContent = "";
    note.value = "";
    note.hidden = true;
    input.hidden = false;
    send.hidden = true;
    menu.hidden = false;
    bar.classList.remove("quoting", "reviewing");
    send.setAttribute("aria-label", "Send the comment");
    send.title = "Send (Enter)";
    note.setAttribute("aria-label", "Comment on the quoted words");
    drop.setAttribute("aria-label", "Drop the quote");
    grow();
  }
  function submit() {
    if (reviewing.verdict !== "approve" && !note.value.trim() && !pendingMine().length) {
      return refused("Write what the review says, or comment on the plan's words first.");
    }
    if (reviewing.verdict === "approve" && !confirming) {
      confirming = true;
      told();
      size();
      return;
    }
    var article = document.querySelector("article.prose");
    var posted = { page: location.pathname, verdict: reviewing.verdict, version: Number(article.dataset.version), text: note.value };
    sending = true;
    note.readOnly = true;
    send.disabled = true;
    why.textContent = "";
    fetch("/api/review", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(posted) })
      .then(window.ekkoAnswered || function (answer) { return { ok: answer.ok, why: "the server answered " + answer.status }; })
      .then(function (got) {
        if (!got.ok) throw new Error(got.why);
        // Put away before the bar shuts: its blur lets a version that came
        // meanwhile reload the page, which would keep what was sent for the
        // next review (task 1343).
        unreview(true);
        close(false);
      })
      .catch(function (error) {
        confirming = false;
        told();
        refused("Not sent: " + (window.ekkoUnreached ? ekkoUnreached(error) : error.message));
      })
      .then(function () {
        sending = false;
        note.readOnly = false;
        grow();
      });
  }
  document.querySelectorAll("[data-review]").forEach(function (button) {
    button.addEventListener("click", review);
  });

  // A press on the pill focuses its input, and the focus opens it; the
  // menu, the list and the nav keep the focus where it is, as AKQA's do.
  line.addEventListener("mousedown", function (event) {
    if (event.target.closest(".bar-menu, .bar-send") || event.target === note) return;
    if (expanded && event.target === input) return;
    event.preventDefault();
    if (writing()) { note.focus(); return; }
    pointed = !expanded;
    input.focus();
  });
  input.addEventListener("focus", function () {
    var fill = pointed;
    pointed = false;
    if (!expanded) open(fill);
  });
  menu.addEventListener("mousedown", function (event) { event.preventDefault(); });
  menu.addEventListener("click", function (event) {
    event.stopPropagation();
    if (expanded) close(true); else open(false);
  });
  input.addEventListener("input", function () {
    if (writing()) return;
    typed();
    render();
  });
  input.addEventListener("keydown", function (event) {
    if (event.key === "Escape") { event.preventDefault(); close(false); return; }
    if (!expanded) { if (event.key !== "Tab") open(false); return; }
    if ((event.key === "ArrowDown" || event.key === "ArrowUp") && matches.length) {
      event.preventDefault();
      var down = event.key === "ArrowDown";
      choose(selected < 0 ? (down ? 0 : matches.length - 1) : (selected + (down ? 1 : matches.length - 1)) % matches.length);
    }
    if (event.key === "Enter" && matches.length) { event.preventDefault(); go(matches[Math.max(selected, 0)]); }
  });
  bar.querySelectorAll(".bar-nav a").forEach(function (a) {
    a.addEventListener("mousedown", function (event) { event.preventDefault(); });
    a.addEventListener("click", function (event) {
      event.preventDefault();
      go({ target: a.getAttribute("href").slice(1) });
    });
  });
  document.addEventListener("pointerdown", function (event) {
    // A comment being written stays open while other words are picked.
    if (expanded && !writing() && !bar.contains(event.target)) close(false);
  });
  // A focus gone elsewhere, as Tab takes it out of the panel, does what a
  // click elsewhere does: left open, the panel hid where the focus went
  // (task 1349, question 1405).
  document.addEventListener("focusin", function (event) {
    if (expanded && !writing() && !bar.contains(event.target)) close(false);
  });
  // What Tab focuses comes clear of the pill, as the root's
  // scroll-padding-bottom asks. Chromium's own scroll does it; Firefox's
  // leaves a focus already inside the window where it is, under the pill or
  // not, so the window is moved up by what it lacks (task 1349). By its
  // box's top and bottom alone: scrollIntoView moved nothing in Firefox, 5
  // Tabs of 6, for What is known's track still waiting 560 px to the side,
  // as it does until the frame after a scroll brings the scene. The Tab is
  // forgotten once its focus has moved, so a click later is no Tab.
  var tabbed = false;
  addEventListener("keydown", function (event) {
    tabbed = event.key === "Tab";
    if (tabbed) setTimeout(function () { tabbed = false; });
  }, true);
  document.addEventListener("focusin", function (event) {
    if (!tabbed || event.target.closest("#bar, dialog, .toc, .mark")) return;
    tabbed = false;
    var box = event.target.getBoundingClientRect(), clear = innerHeight - parseFloat(getComputedStyle(document.documentElement).scrollPaddingBottom);
    if (box.bottom > clear && box.height <= clear) scrollBy(0, box.bottom - clear);
  });
  // "/" opens the pill, and R the review where the page writes; not while a
  // note's sheet holds the page.
  addEventListener("keydown", function (event) {
    if ((event.key !== "/" && event.key !== "r") || event.ctrlKey || event.metaKey || event.altKey || document.querySelector("dialog:modal")) return;
    var active = document.activeElement;
    if (active && (active.tagName === "INPUT" || active.tagName === "TEXTAREA" || active.isContentEditable)) return;
    if (event.key === "r") {
      if (!writes() || reviewing) return;
      event.preventDefault();
      review();
      return;
    }
    event.preventDefault();
    if (writing()) open(false); else input.focus();
  });
  addEventListener("resize", size);
  // Review shows once the server says the page writes as the person, just
  // after the page is drawn: the pill makes room for it at once, and the
  // menu offers the person's commands.
  new MutationObserver(function () {
    var w = widths();
    if (!expanded) {
      width.set(w.slim);
      shift.set(w.shift);
    }
    acts();
    size();
  }).observe(document.body, { attributes: true, attributeFilter: ["data-writes"] });
  words(hints[shown][0], false);
  typed();
  acts();
  size();
  resume(3200);

  // Where the reader was, kept across the reload a new version of the page
  // makes: the last part, step or note begun above the window's top and how
  // far above, the steps open, and the bar's text while it is open.
  var keptAt = "ekko-kept " + location.pathname;
  // Where an element begins on the page as laid out, leaving out what
  // moves it: the 24 px a part not yet revealed sits lower would otherwise
  // move the page by as much at each reload (task 1388).
  function laidTop(element) {
    var y = 0;
    for (; element; element = element.offsetParent) y += element.offsetTop;
    return y;
  }
  // Of what has a box only: one without, as the map's arrowhead, a tab not
  // picked or a step off the stage, says its top is 0 wherever the reader
  // is (task 1388).
  function anchor() {
    var found = null;
    document.querySelectorAll("main [id]").forEach(function (element) {
      if (element instanceof HTMLElement && element.getClientRects().length && laidTop(element) - scrollY <= 1) found = element;
    });
    return found;
  }
  window.ekkoKeep = function () {
    var at = anchor();
    var kept = { y: scrollY, id: at ? at.id : "", offset: at ? laidTop(at) - scrollY : 0,
      open: Array.prototype.map.call(document.querySelectorAll(".step.open:not(.by-stage)"), function (step) { return step.id; }), scenes: {} };
    Object.keys(keepers).forEach(function (name) { kept.scenes[name] = keepers[name].keep(); });
    if (bar.classList.contains("open") && !quoting && !sending) kept.bar = input.value;
    if (quoting && !sending) { kept.quote = quoting; kept.note = note.value; kept.color = color; kept.editing = editing; kept.suggesting = suggesting; kept.said = said; kept.put = put; }
    if (reviewing && !sending) kept.review = { verdict: reviewing.verdict, note: note.value };
    if (summary) kept.summary = summary;
    if (chosen !== "comment") kept.chosen = chosen;
    try { sessionStorage.setItem(keptAt, JSON.stringify(kept)); history.scrollRestoration = "manual"; } catch (e) {}
  };
  var kept = null;
  try { kept = JSON.parse(sessionStorage.getItem(keptAt)); sessionStorage.removeItem(keptAt); } catch (e) {}
  if (kept) {
    (kept.open || []).forEach(function (id) { var step = document.getElementById(id); if (step) setOpen(step, true); });
    Object.keys(kept.scenes || {}).forEach(function (name) { if (keepers[name]) keepers[name].back(kept.scenes[name]); });
    var back = function () {
      // An element the page hides now, as an entry of a tab not picked, is
      // shown as a jump shows it (task 1375); one that stays hidden, as a
      // step the stage no longer shows, has no top to go back by: the place
      // is then the one kept (task 1391).
      var at = kept.id && document.getElementById(kept.id);
      if (at && !at.getClientRects().length) uncover(at);
      if (at && !at.getClientRects().length) at = null;
      scrollTo(0, at ? laidTop(at) - kept.offset : kept.y);
      try { history.scrollRestoration = "auto"; } catch (e) {}
      if (kept.quote && kept.editing) ekkoEdit(kept.editing);
      else if (kept.quote) quote(kept.quote, kept.color, null);
      if (kept.quote) {
        if (!!kept.suggesting !== suggesting) suggestion(!!kept.suggesting);
        said = kept.said || "";
        put = kept.put === undefined ? null : kept.put;
        note.value = kept.note || "";
        grow();
      }
      summary = kept.summary || "";
      chosen = kept.chosen || "comment";
      if (kept.review) { summary = kept.review.note || ""; review(kept.review.verdict); }
      if (typeof kept.bar === "string") { input.value = kept.bar; open(false); }
    };
    var settled = function () { document.fonts.ready.then(back); };
    if (document.readyState === "complete") settled(); else addEventListener("load", settled);
  }
  // An address naming what a scene hides, as a step off the stage or a
  // card on a tab not picked, shows it first, as a jump does (task 1378),
  // once the page has loaded; and so does a link to one in the page. A
  // reload goes back where the reader was, as the browser's own does.
  var toNamed = function (smoothly) {
    var id = "";
    try { id = decodeURIComponent(location.hash.slice(1)); } catch (e) {}
    var target = id && document.getElementById(id);
    if (target) uncover(target).scrollIntoView({ behavior: smoothly ? smooth() : "auto", block: "start" });
  };
  var arrived = performance.getEntriesByType ? performance.getEntriesByType("navigation")[0] : null;
  if (!kept && location.hash.length > 1 && !(arrived && arrived.type === "reload")) {
    var landed = function () { document.fonts.ready.then(function () { toNamed(false); }); };
    if (document.readyState === "complete") landed(); else addEventListener("load", landed);
  }
  addEventListener("hashchange", function () { toNamed(true); });
})();

// Comments on the plan's words (tasks 1105 and 1213), kept as an ebook keeps
// its notes (decision 1214): each is its words, tinted in its theme's color,
// with a pin at their end counting the comments that end there. A click on
// them opens every comment at that point beside them, and Comments, after
// the plan, is the notebook. A comment is found again by its quote in
// whatever version the page shows: exactly, else between the same words on
// either side when its own changed a little, else it is outdated and kept
// in the notebook. Served to the person, words selected in the plan offer
// Comment and the themes; the comment is written in the pill.
(function () {
  var dataNode = document.getElementById("comments-data"), themesNode = document.getElementById("themes");
  var article = document.querySelector("article.prose");
  if (!dataNode || !themesNode || !article) return;
  var comments = JSON.parse(dataNode.textContent);
  var THEMES = JSON.parse(themesNode.textContent);
  var plan = Array.prototype.slice.call(document.querySelectorAll("[data-plan]"));
  var notebook = document.getElementById("comments");
  var BLOCK = "p, li, h1, h2, h3, h4, h5, h6, pre, td, th, blockquote, section, dt, dd";
  var AROUND = 32;
  var WORD = /[\p{L}\p{N}]/u;
  var byId = {}, byUid = {}, replies = {};
  comments.forEach(function (note) {
    byId[note.id] = note;
    if (note.uid) byUid[note.uid] = note;
  });
  // A reply goes under the comment it answers, and is no words of its own.
  comments.forEach(function (note) {
    var to = note.comment && note.comment.replyTo;
    if (to && byUid[to]) (replies[to] = replies[to] || []).push(note);
  });
  function isReply(note) { return !!(note.comment && note.comment.replyTo && byUid[note.comment.replyTo]); }
  function element(tag, className, text) {
    var made = document.createElement(tag);
    if (className) made.className = className;
    if (text) made.textContent = text;
    return made;
  }
  function writes() { return "writes" in document.body.dataset; }

  // The themes: their colors, and their names as the person renamed them
  // in this browser; else as the person's newest comment in that color
  // calls it, which reaches another browser, or this one once the server
  // runs on another port, another origin to localStorage; else as ekko
  // offers them. A comment keeps the name it was written under, which is
  // what its reader learns it meant.
  var NAMES = "ekko-theme-names", LAST = "ekko-theme-last", SHOWN = "ekko-highlights";
  function stored(key) { try { return JSON.parse(localStorage.getItem(key)); } catch (e) { return null; } }
  function store(key, value) { try { localStorage.setItem(key, JSON.stringify(value)); } catch (e) {} }
  function known(color) { return THEMES.some(function (theme) { return theme[0] === color; }); }
  var used = {};
  comments.slice().sort(function (a, b) { return b.id - a.id; }).forEach(function (note) {
    var c = note.comment || {};
    if (note.mine && c.theme && known(c.color) && !used[c.color]) used[c.color] = c.theme;
  });
  function names() {
    var own = stored(NAMES) || {}, named = {};
    THEMES.forEach(function (theme) {
      var key = theme[0];
      named[key] = typeof own[key] === "string" && own[key] ? own[key] : used[key] || theme[1];
    });
    return named;
  }
  // A name as the server takes it: one line of 1 to 40 characters.
  function fitted(name) {
    return Array.from(String(name || "").replace(/[\u0000-\u001f\u007f]/g, " ").trim()).slice(0, 40).join("").trim();
  }
  function colorOf(note) { var c = note.comment || {}; return known(c.color) ? c.color : THEMES[0][0]; }
  // A comment written before themes, or without one, goes by its color's
  // name, as the filters count it.
  function themeOf(note) {
    var c = note.comment || {};
    return c.theme || names()[colorOf(note)];
  }
  function stateOf(note) { var c = note.comment || {}; return c.applied ? "applied" : c.resolved ? "resolved" : c.sent ? "sent" : "pending"; }
  // What a suggestion does to the plan's words: those words struck through,
  // and the ones it puts in their place (task 1107).
  function suggested(c) {
    var change = element("div", "suggests");
    change.appendChild(element("del", "", c.quote.exact));
    if (c.replacement) change.appendChild(element("ins", "", c.replacement));
    return change;
  }
  // What a suggestion says when the person wrote nothing else, as the
  // server words it (`feedback::suggested`).
  function suggestedText(c) { return c.replacement ? "Replace " + c.quote.exact + " with " + c.replacement : "Delete: " + c.quote.exact; }
  // The person's comments still pending, which a review sends (task 1106),
  // counted on Review beside the pill.
  window.ekkoPending = function () { return comments.filter(function (note) { return note.mine && stateOf(note) === "pending"; }); };
  Array.prototype.forEach.call(document.querySelectorAll("[data-review] .count"), function (count) {
    var pending = ekkoPending().length;
    count.textContent = pending ? String(pending) : "";
    count.parentElement.setAttribute("aria-label", pending ? "Review, " + (pending === 1 ? "1 pending comment" : pending + " pending comments") : "Review");
  });
  window.ekkoThemes = {
    list: THEMES,
    names: names,
    // Every name kept, ekko's own too, so a name set back to ekko's is not
    // taken again from a comment; an emptied one is ekko's again.
    rename: function (named) {
      var own = {};
      THEMES.forEach(function (theme) { own[theme[0]] = fitted(named[theme[0]]) || theme[1]; });
      store(NAMES, own);
      relabel();
      filters();
    },
    last: function () { var color = stored(LAST); return known(color) ? color : THEMES[0][0]; },
    setLast: function (color) { if (known(color)) store(LAST, color); }
  };

  // The plan's words as shown_words has them on the server: each run of
  // white space one space, every block apart from the next; and for each
  // character, the text node and offset it came from, the space between
  // two blocks marked as no character of either. What the page adds to the
  // plan's parts, their openers too, is chrome, and no words of the plan.
  function words() {
    var text = "", at = [], last = null, space = true;
    plan.forEach(function (root) {
      var walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, { acceptNode: function (node) {
        return node.parentElement.closest("[data-chrome]") ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT;
      } });
      for (var node = walker.nextNode(); node; node = walker.nextNode()) {
        var block = node.parentElement.closest(BLOCK);
        if (block !== last && !space) { text += " "; at.push([node, 0, true]); space = true; }
        last = block;
        var value = node.nodeValue;
        for (var i = 0; i < value.length; i++) {
          var white = /\s/.test(value.charAt(i));
          if (white && space) continue;
          text += white ? " " : value.charAt(i);
          at.push([node, i]);
          space = white;
        }
      }
    });
    return { text: text, at: at };
  }
  function same(a, b, fromEnd) {
    var n = 0;
    while (n < a.length && n < b.length && (fromEnd ? a.charAt(a.length - 1 - n) === b.charAt(b.length - 1 - n) : a.charAt(n) === b.charAt(n))) n++;
    return n;
  }
  function partOf(node) {
    var part = node.parentElement.closest("[data-plan]");
    return part && part.dataset.part || "";
  }
  function flat(text) { return String(text || "").replace(/\s+/g, " "); }
  // How alike two runs of words are, from 0 to 1: by edit distance, or by
  // the words they share when they are too long to compare letter by letter.
  function alike(a, b) {
    var longest = Math.max(a.length, b.length);
    if (!longest || a === b) return 1;
    if (a.length * b.length > 4000000) {
      var seen = {}, both = 0, x = a.toLowerCase().split(" "), y = b.toLowerCase().split(" ");
      x.forEach(function (word) { seen[word] = true; });
      y.forEach(function (word) { if (seen[word]) both++; });
      return both / Math.max(x.length, y.length);
    }
    var row = [];
    for (var j = 0; j <= b.length; j++) row[j] = j;
    for (var i = 1; i <= a.length; i++) {
      var diagonal = row[0];
      row[0] = i;
      for (j = 1; j <= b.length; j++) {
        var above = row[j];
        row[j] = Math.min(above + 1, row[j - 1] + 1, diagonal + (a.charAt(i - 1) === b.charAt(j - 1) ? 0 : 1));
        diagonal = above;
      }
    }
    return 1 - row[b.length] / longest;
  }
  // Where a quote's words are now, as the W3C's quote selector finds them:
  // of the places that hold them, the one whose surroundings and section
  // match best. Else, as Hypothesis anchors, the words between the same
  // words on either side, when they are close enough to the quote's: the
  // comment then says its words changed.
  function find(index, quote) {
    var exact = flat(quote.exact).trim(), prefix = flat(quote.prefix), suffix = flat(quote.suffix);
    if (!exact) return null;
    var best = null, score = -1;
    for (var at = index.text.indexOf(exact); at >= 0; at = index.text.indexOf(exact, at + 1)) {
      var here = same(index.text.slice(Math.max(0, at - AROUND), at), prefix, true) + same(index.text.slice(at + exact.length), suffix, false)
        + (partOf(index.at[at][0]) === (quote.section || "") ? 1 : 0);
      if (here > score) { best = [at, at + exact.length]; score = here; }
    }
    if (best) return { span: best, changed: false };
    var before = prefix.slice(-20), after = suffix.slice(0, 20), text = index.text, close = 0;
    if (before.trim().length < 8 || after.trim().length < 8) return null;
    for (var from = text.indexOf(before); from >= 0; from = text.indexOf(before, from + 1)) {
      var start = from + before.length, end = text.indexOf(after, start);
      if (end < 0 || end - start > exact.length * 2 + 40) continue;
      while (start < end && text.charAt(start) === " ") start++;
      while (end > start && text.charAt(end - 1) === " ") end--;
      if (end <= start) continue;
      var near = alike(text.slice(start, end), exact);
      if (near > close) { best = [start, end]; close = near; }
    }
    return close >= 0.5 ? { span: best, changed: true } : null;
  }
  function range(index, span) {
    var r = document.createRange(), first = index.at[span[0]], last = index.at[span[1] - 1];
    r.setStart(first[0], first[1]);
    r.setEnd(last[0], last[1] + 1);
    return r;
  }

  // Each comment found again, but a reply's or a resolved one's, which
  // color no words.
  var index = words(), anchored = [];
  comments.forEach(function (note) {
    var c = note.comment || {};
    note.found = c.quote && !isReply(note) && !c.resolved ? find(index, c.quote) : null;
    if (note.found) anchored.push(note);
  });
  // The words cut where any comment begins or ends: each piece colored for
  // the comments over it, one mark per text node, wrapped from the end so
  // the offsets still to wrap hold.
  var cuts = [];
  anchored.forEach(function (note) { cuts.push(note.found.span[0], note.found.span[1]); });
  cuts = cuts.sort(function (a, b) { return a - b; }).filter(function (cut, i, all) { return i === 0 || cut !== all[i - 1]; });
  var pieces = [], ends = {};
  for (var k = 0; k + 1 < cuts.length; k++) {
    var from = cuts[k], to = cuts[k + 1];
    var over = anchored.filter(function (note) { return note.found.span[0] <= from && note.found.span[1] >= to; });
    if (!over.length) continue;
    var piece = null;
    for (var i = from; i < to; i++) {
      var entry = index.at[i];
      if (entry[2]) { piece = null; continue; }
      if (piece && piece.node === entry[0]) { piece.end = entry[1] + 1; continue; }
      piece = { node: entry[0], start: entry[1], end: entry[1] + 1, order: i, to: to, notes: over };
      pieces.push(piece);
    }
  }
  function paint(mark, notes) {
    var colors = [];
    THEMES.forEach(function (theme) {
      if (notes.some(function (note) { return colorOf(note) === theme[0]; })) colors.push(theme[0]);
    });
    if (colors.length === 1) {
      mark.style.setProperty("--tint", "var(--tint-" + colors[0] + ")");
      if (notes.length > 1) mark.dataset.stack = Math.min(notes.length, 3);
      return;
    }
    var share = 100 / colors.length;
    mark.style.backgroundColor = "transparent";
    mark.style.backgroundImage = "linear-gradient(" + colors.map(function (color, i) {
      return "var(--tint-" + color + ") " + (i * share).toFixed(2) + "% " + ((i + 1) * share).toFixed(2) + "%";
    }).join(", ") + ")";
  }
  pieces.sort(function (a, b) { return b.order - a.order; }).forEach(function (piece) {
    var r = document.createRange();
    r.setStart(piece.node, piece.start);
    r.setEnd(piece.node, piece.end);
    var mark = element("mark", "c");
    mark.dataset.ids = piece.notes.map(function (note) { return note.id; }).join(" ");
    mark.setAttribute("aria-details", piece.notes.map(function (note) { return "comment-" + note.id; }).join(" "));
    paint(mark, piece.notes);
    r.surroundContents(mark);
    if (!ends[piece.to]) ends[piece.to] = mark;
  });
  // A pin where comments end, as Kindle marks a note: a dot in their color,
  // with their count when they are several; a button, so the keyboard
  // reaches them too.
  Object.keys(ends).forEach(function (to) {
    var ending = anchored.filter(function (note) { return note.found.span[1] === Number(to); });
    if (!ending.length) return;
    var pin = element("button", "pin");
    pin.type = "button";
    pin.dataset.ids = ending.map(function (note) { return note.id; }).join(" ");
    if (ending.length > 1) pin.dataset.n = ending.length;
    var inks = [];
    ending.forEach(function (note) { if (inks.indexOf(colorOf(note)) < 0) inks.push(colorOf(note)); });
    pin.style.background = inks.length === 1 ? "var(--ink-" + inks[0] + ")" : "linear-gradient(90deg, " + inks.map(function (color, i) {
      return "var(--ink-" + color + ") " + (i * 100 / inks.length).toFixed(2) + "% " + ((i + 1) * 100 / inks.length).toFixed(2) + "%";
    }).join(", ") + ")";
    pin.setAttribute("aria-label", ending.length === 1 ? themeOf(ending[0]) + " comment " + ending[0].id + ": " + ending[0].text : ending.length + " comments end here");
    var mark = ends[to];
    (mark.closest("a") || mark).after(pin);
  });
  function marksOf(id) {
    return Array.prototype.filter.call(article.querySelectorAll("mark.c"), function (mark) { return (" " + mark.dataset.ids + " ").indexOf(" " + id + " ") >= 0; });
  }
  // The words of one comment underlined in its color: which words it is
  // on, among others over the same ones.
  function light(id) {
    Array.prototype.forEach.call(article.querySelectorAll("mark.c.lit"), function (mark) { mark.classList.remove("lit"); });
    if (id == null || !byId[id]) return;
    marksOf(id).forEach(function (mark) {
      mark.classList.add("lit");
      mark.style.setProperty("--lit", "var(--ink-" + colorOf(byId[id]) + ")");
    });
  }

  // The comments at a point, beside the words: each with its theme, id,
  // state, text and replies, and, served to its writer, Edit and Delete.
  var pop = element("div", "pop");
  pop.hidden = true;
  pop.tabIndex = -1;
  pop.setAttribute("role", "dialog");
  document.body.appendChild(pop);
  var popIds = null, popFrom = null;
  function command() { var bar = document.getElementById("bar"); return bar && bar.dataset.command ? bar.dataset.command : "ekko artifact"; }
  // What the server answered: whether it wrote, and why not.
  function answered(answer) {
    return answer.text().then(function (body) {
      var why = "";
      try { why = JSON.parse(body).why || ""; } catch (e) { why = body.trim(); }
      return { ok: answer.ok, why: why || "the server answered " + answer.status };
    });
  }
  window.ekkoAnswered = answered;
  window.ekkoUnreached = function (error) { return error instanceof TypeError ? "the server does not answer; " + command() + " starts it again" : error.message; };
  function entryOf(note, several, same) {
    var c = note.comment || {}, color = colorOf(note);
    var box = element("div", "pop-item" + (same ? " same" : ""));
    box.dataset.id = note.id;
    box.style.setProperty("--ink", "var(--ink-" + color + ")");
    var head = element("div", "pop-head");
    head.appendChild(element("span", "theme", themeOf(note)));
    head.appendChild(element("span", "id", String(note.id)));
    head.appendChild(element("span", "state", stateOf(note)));
    if (note.found && note.found.changed) head.appendChild(element("span", "state changed", "words changed"));
    if (same) head.appendChild(element("span", "same-words", "same words"));
    box.appendChild(head);
    var suggests = c.quote && typeof c.replacement === "string";
    if (c.quote && !same && !suggests && (several || (note.found && note.found.changed))) {
      var said = element("div", "said" + (note.found && note.found.changed ? " was" : ""), c.quote.exact);
      if (note.found && note.found.changed) said.setAttribute("aria-label", "It was on: " + c.quote.exact);
      box.appendChild(said);
    }
    if (suggests) box.appendChild(suggested(c));
    box.appendChild(element("div", "text" + (suggests && note.text === suggestedText(c) ? " told" : ""), note.text));
    (replies[note.uid] || []).forEach(function (reply) {
      var answer = element("div", "pop-reply");
      answer.appendChild(element("div", "meta", (reply.by || "") + " · " + reply.when));
      answer.appendChild(element("div", "text", reply.text));
      box.appendChild(answer);
    });
    // Who wrote it and when, and, served to its writer, Edit and Delete on
    // the same line, so many comments at a point still read as a list.
    var foot = element("div", "pop-foot");
    foot.appendChild(element("span", "meta", (note.by ? note.by + " · " : "") + note.when));
    box.appendChild(foot);
    if (writes() && note.mine && note.uid) {
      var actions = element("span", "pop-actions");
      var edit = element("button", "", "Edit"), remove = element("button", "danger", "Delete");
      edit.type = remove.type = "button";
      edit.addEventListener("click", function () {
        shut(false);
        if (window.ekkoEdit) ekkoEdit({ id: note.id, uid: note.uid, text: note.text, quote: c.quote, color: color, replacement: c.replacement, told: suggests && note.text === suggestedText(c) });
      });
      remove.addEventListener("click", function () {
        actions.textContent = "";
        actions.appendChild(element("span", "ask", "Delete this comment? The trash keeps it 30 days."));
        var yes = element("button", "danger", "Delete"), no = element("button", "", "Keep");
        yes.type = no.type = "button";
        no.addEventListener("click", function () { open(popIds, popFrom); });
        yes.addEventListener("click", function () {
          yes.disabled = no.disabled = true;
          fetch("/api/comment/delete", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ page: location.pathname, uid: note.uid }) })
            .then(answered)
            .then(function (got) {
              if (!got.ok) throw new Error(got.why);
              actions.textContent = "";
              actions.appendChild(element("span", "ask", "Deleted."));
            })
            .catch(function (error) {
              yes.disabled = no.disabled = false;
              actions.appendChild(element("span", "why", "Not deleted: " + ekkoUnreached(error)));
            });
        });
        actions.appendChild(yes);
        actions.appendChild(no);
        yes.focus();
      });
      // A sent suggestion whose words are still where it was made applies
      // to the plan, as GitHub's Apply suggestion does: the next version
      // puts its words there (task 1107).
      if (suggests && stateOf(note) === "sent" && note.found && !note.found.changed) {
        var apply = element("button", "", "Apply");
        apply.type = "button";
        apply.title = "Put these words in the plan, in its next version";
        apply.addEventListener("click", function () {
          apply.disabled = true;
          fetch("/api/comment/apply", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ page: location.pathname, uid: note.uid }) })
            .then(answered)
            .then(function (got) {
              if (!got.ok) throw new Error(got.why);
              actions.textContent = "";
              actions.appendChild(element("span", "ask", "Applied."));
            })
            .catch(function (error) {
              apply.disabled = false;
              actions.appendChild(element("span", "why", "Not applied: " + ekkoUnreached(error)));
            });
        });
        actions.appendChild(apply);
      }
      // A pending comment sent alone, outside a review, as GitHub's single
      // comment goes (task 1106).
      if (stateOf(note) === "pending") {
        var now = element("button", "", "Send now");
        now.type = "button";
        now.title = "Send it to the session now, outside a review";
        now.addEventListener("click", function () {
          now.disabled = true;
          fetch("/api/comment/send", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ page: location.pathname, uid: note.uid }) })
            .then(answered)
            .then(function (got) {
              if (!got.ok) throw new Error(got.why);
              actions.textContent = "";
              actions.appendChild(element("span", "ask", "Sent."));
            })
            .catch(function (error) {
              now.disabled = false;
              actions.appendChild(element("span", "why", "Not sent: " + ekkoUnreached(error)));
            });
        });
        actions.appendChild(now);
      }
      actions.appendChild(edit);
      actions.appendChild(remove);
      foot.appendChild(actions);
    }
    box.addEventListener("mouseenter", function () { light(note.id); });
    box.addEventListener("focusin", function () { light(note.id); });
    return box;
  }
  // Below the line clicked, or above it when there is more room there, as
  // Floating UI's flip and size place a popover: never past the window's
  // top or into the pill's band, scrolling inside when the comments are
  // taller than the room; never wider than the column.
  var EDGE = 12, BAND = 128;
  function place(from, y) {
    var rects = Array.prototype.slice.call(from.getClientRects());
    var line = rects.filter(function (rect) { return y >= rect.top && y <= rect.bottom; })[0] || rects[rects.length - 1] || from.getBoundingClientRect();
    var column = article.getBoundingClientRect(), width = Math.min(400, column.width);
    pop.style.width = width + "px";
    pop.style.left = scrollX + Math.max(column.left, Math.min(line.left, column.right - width)) + "px";
    pop.style.maxHeight = "";
    var height = pop.offsetHeight, below = innerHeight - BAND - line.bottom - 8, above = line.top - 8 - EDGE;
    var under = height <= below || below >= above, room = under ? below : above;
    if (height > room) {
      pop.style.maxHeight = Math.max(room, 96) + "px";
      height = pop.offsetHeight;
    }
    pop.style.top = scrollY + (under ? line.bottom + 8 : line.top - 8 - height) + "px";
  }
  function open(ids, from, y) {
    ids = ids.filter(function (id) { return byId[id]; });
    if (!ids.length || !from) return;
    // In reading order, those on the very same words one after another,
    // their words quoted once.
    var spanOf = function (id) { var found = byId[id].found; return found ? found.span : [0, 0]; };
    ids.sort(function (a, b) { return spanOf(a)[0] - spanOf(b)[0] || spanOf(a)[1] - spanOf(b)[1] || a - b; });
    popIds = ids;
    popFrom = from;
    pop.textContent = "";
    pop.setAttribute("aria-label", ids.length === 1 ? "Comment " + ids[0] : ids.length + " comments on these words");
    if (ids.length > 1) pop.appendChild(element("div", "pop-count", ids.length + " comments on these words"));
    ids.forEach(function (id, i) {
      var same = i > 0 && spanOf(id).join() === spanOf(ids[i - 1]).join();
      pop.appendChild(entryOf(byId[id], ids.length > 1, same));
    });
    pop.style.visibility = "hidden";
    pop.hidden = false;
    place(from, y == null ? from.getBoundingClientRect().top + 1 : y);
    pop.style.visibility = "";
    light(ids[0]);
    pop.focus({ preventScroll: true });
  }
  function shut(back) {
    if (pop.hidden) return;
    pop.hidden = true;
    popIds = null;
    light(null);
    if (back && popFrom && popFrom.isConnected && popFrom.focus) popFrom.focus({ preventScroll: true });
  }
  window.ekkoShut = shut;
  // A popover built before the page knew it writes, as one kept across a
  // reload is, has none of the buttons that write: once the page knows, it
  // is built again with them (task 1384).
  window.ekkoWrites = function () { if (!pop.hidden && popIds && popFrom && popFrom.isConnected) open(popIds, popFrom); };
  pop.addEventListener("mouseleave", function () { if (popIds) light(popIds[0]); });
  article.addEventListener("click", function (event) {
    var hit = event.target.closest("mark.c, .pin");
    if (!hit || hit.classList.contains("off")) return;
    // A link in the words still goes where it points, and a drag over them
    // is a selection to comment on.
    if (!hit.classList.contains("pin") && (event.target.closest("a") || !getSelection().isCollapsed)) return;
    event.preventDefault();
    open(hit.dataset.ids.split(" ").map(Number), hit, event.clientY);
  });
  document.addEventListener("pointerdown", function (event) {
    if (!pop.hidden && !pop.contains(event.target) && !event.target.closest("mark.c, .pin")) shut(false);
  });
  addEventListener("keydown", function (event) {
    if (event.key === "Escape" && !pop.hidden) { event.preventDefault(); shut(true); }
  });
  addEventListener("resize", function () { if (popIds && popFrom && popFrom.isConnected) place(popFrom, popFrom.getBoundingClientRect().top + 1); });
  // A comment on words still in the plan opens at them; one whose words
  // are gone, or one resolved, at its entry under Comments, so it can be
  // read, edited and deleted there too. What the words sit in shows them
  // first, even where they have a box: a risk dealt away, a card shut
  // (task 1378).
  window.ekkoOpenComment = function (id) {
    var at = marksOf(id)[0] || document.getElementById("comment-" + id);
    if (!at) return;
    if (window.ekkoSurface) ekkoSurface(at);
    at.scrollIntoView({ block: "center" });
    open([id], at);
  };
  window.ekkoComment = function (id) { return byId[id]; };
  // The comments open and the notebook's filter, kept across the reload a
  // new version makes, as ekkoKeep keeps where the reader was.
  var keptAt = "ekko-comments " + location.pathname, keep = window.ekkoKeep;
  var filter = null, shown = stored(SHOWN) !== false;
  window.ekkoKeep = function () {
    if (keep) keep();
    try { sessionStorage.setItem(keptAt, JSON.stringify({ open: popIds, filter: filter })); } catch (e) {}
  };
  var kept = null;
  try { kept = JSON.parse(sessionStorage.getItem(keptAt)); sessionStorage.removeItem(keptAt); } catch (e) {}

  // The notebook: in reading order, the outdated ones last; each says
  // where its words stand, and opens them.
  var list = notebook && notebook.querySelector("ol.comments"), chipsBar = notebook && notebook.querySelector(".filters");
  var entries = list ? Array.prototype.slice.call(list.children) : [];
  function placeOf(entry) { var note = byId[entry.dataset.id]; return note && note.found ? note.found.span[0] : Infinity; }
  entries.sort(function (a, b) { return placeOf(a) - placeOf(b) || a.dataset.id - b.dataset.id; }).forEach(function (entry) { list.appendChild(entry); });
  entries.forEach(function (entry) {
    var note = byId[entry.dataset.id];
    if (!note) return;
    var c = note.comment || {}, head = entry.querySelector(".entry-head");
    entry.style.setProperty("--ink", "var(--ink-" + colorOf(note) + ")");
    if (c.quote && !note.found && !c.resolved) {
      entry.classList.add("outdated");
      head.appendChild(element("span", "state gone", "outdated: made on version " + c.version));
    }
    if (note.found && note.found.changed) head.appendChild(element("span", "state changed", "words changed"));
    if (c.resolved) entry.classList.add("resolved");
    head.addEventListener("click", function () { ekkoOpenComment(note.id); });
  });
  // A comment with no theme's name of its own goes by its color's, as this
  // browser names it.
  function relabel() {
    entries.forEach(function (entry) {
      var note = byId[entry.dataset.id];
      if (note && !(note.comment || {}).theme) entry.querySelector(".theme").textContent = themeOf(note);
    });
  }
  relabel();
  function filters() {
    if (!chipsBar) return;
    chipsBar.textContent = "";
    var named = names(), counts = {}, outdated = entries.filter(function (entry) { return entry.classList.contains("outdated"); }).length;
    entries.forEach(function (entry) { counts[entry.dataset.color] = (counts[entry.dataset.color] || 0) + 1; });
    var chip = function (label, value, count, ink) {
      var button = element("button", "filter");
      button.type = "button";
      button.setAttribute("aria-pressed", filter === value ? "true" : "false");
      if (ink) { button.appendChild(element("span", "dot")); button.style.setProperty("--ink", "var(--ink-" + ink + ")"); }
      button.appendChild(document.createTextNode(label));
      button.appendChild(element("span", "count", String(count)));
      button.addEventListener("click", function () { filter = filter === value ? null : value; apply(); filters(); });
      chipsBar.appendChild(button);
    };
    chip("All", null, entries.length);
    THEMES.forEach(function (theme) { if (counts[theme[0]]) chip(named[theme[0]], theme[0], counts[theme[0]], theme[0]); });
    if (outdated) chip("Outdated", "outdated", outdated);
    var toggle = element("button", "filter switch", "Highlights");
    toggle.type = "button";
    toggle.setAttribute("aria-pressed", shown ? "true" : "false");
    toggle.title = "Show the comments' colors in the text";
    toggle.addEventListener("click", function () { shown = !shown; store(SHOWN, shown); apply(); filters(); });
    chipsBar.appendChild(toggle);
  }
  // A theme picked shows its comments alone, here and in the text; the
  // switch takes every color off the text, to read it clean.
  function apply() {
    entries.forEach(function (entry) {
      entry.hidden = filter === "outdated" ? !entry.classList.contains("outdated") : !!filter && entry.dataset.color !== filter;
    });
    Array.prototype.forEach.call(article.querySelectorAll("mark.c, .pin"), function (hit) {
      var on = shown && (!filter || filter === "outdated" || hit.dataset.ids.split(" ").some(function (id) { return byId[id] && colorOf(byId[id]) === filter; }));
      hit.classList.toggle("off", !on);
    });
    if (!shown || (filter && filter !== "outdated")) shut(false);
  }
  if (kept && kept.filter !== undefined) filter = kept.filter;
  filters();
  apply();
  if (kept && kept.open) {
    var reopen = function () {
      var ids = kept.open.filter(function (id) { return marksOf(id).length; });
      if (ids.length) open(ids, marksOf(ids[0])[0]);
    };
    document.fonts.ready.then(function () { requestAnimationFrame(reopen); });
  }

  // The words the pill is writing a comment on, marked in their theme's
  // color until it is sent or given up; found again by their quote after
  // a reload.
  var quoted = {};
  if (window.Highlight && window.CSS && CSS.highlights) {
    THEMES.forEach(function (theme) {
      var highlight = new Highlight();
      highlight.priority = 1;
      CSS.highlights.set("ekko-quoted-" + theme[0], highlight);
      quoted[theme[0]] = highlight;
    });
  }
  window.ekkoMark = function (quote, color) {
    Object.keys(quoted).forEach(function (key) { quoted[key].clear(); });
    if (!quote) return;
    var now = words(), found = find(now, quote), highlight = quoted[known(color) ? color : THEMES[0][0]];
    if (found && highlight) highlight.add(range(now, found.span));
  };

  // Words selected in the plan, served to the person who can write: one
  // button and the themes, as Claude offers Reply on part of an answer and
  // an ebook its colors. Either takes the words, snapped to whole words as
  // an ebook's selection is, into the pill.
  var tools = element("div", "select-tools");
  tools.hidden = true;
  var take = element("button", "take"), takeLabel = element("span", "label", "Comment"), takeKey = element("kbd", "", "C");
  take.type = "button";
  take.appendChild(takeLabel);
  take.appendChild(takeKey);
  tools.appendChild(take);
  var swatches = element("span", "swatches");
  THEMES.forEach(function (theme) {
    var swatch = element("button", "swatch");
    swatch.type = "button";
    swatch.dataset.color = theme[0];
    swatch.style.setProperty("--ink", "var(--ink-" + theme[0] + ")");
    swatches.appendChild(swatch);
  });
  tools.appendChild(swatches);
  document.body.appendChild(tools);
  var chosen = null;
  // A color pointed at says its theme and its key where Comment stood.
  function told(swatch) {
    var i = swatch ? Array.prototype.indexOf.call(swatches.children, swatch) : -1;
    takeLabel.textContent = swatch ? names()[swatch.dataset.color] : "Comment";
    takeKey.textContent = swatch ? String(i + 1) : "C";
  }
  swatches.addEventListener("mouseover", function (event) { told(event.target.closest(".swatch")); });
  swatches.addEventListener("focusin", function (event) { told(event.target.closest(".swatch")); });
  swatches.addEventListener("mouseleave", function () { told(null); });
  swatches.addEventListener("focusout", function () { told(null); });

  function selected(snap) {
    if (!writes() || !window.ekkoQuote) return null;
    var selection = getSelection();
    if (!selection.rangeCount || selection.isCollapsed) return null;
    var r = selection.getRangeAt(0);
    if (!plan.some(function (part) { return r.intersectsNode(part); })) return null;
    var now = words(), start = -1, end = -1;
    for (var i = 0; i < now.at.length; i++) {
      var node = now.at[i][0], offset = now.at[i][1];
      if (r.comparePoint(node, offset) === 0 && r.comparePoint(node, offset + 1) === 0) {
        if (start < 0) start = i;
        end = i + 1;
      }
    }
    while (start >= 0 && start < end && now.text.charAt(start) === " ") start++;
    while (end > start && now.text.charAt(end - 1) === " ") end--;
    if (start < 0 || end <= start) return null;
    while (start > 0 && WORD.test(now.text.charAt(start - 1)) && WORD.test(now.text.charAt(start))) start--;
    while (end < now.text.length && WORD.test(now.text.charAt(end)) && WORD.test(now.text.charAt(end - 1))) end++;
    var whole = range(now, [start, end]);
    if (snap) { selection.removeAllRanges(); selection.addRange(whole); }
    return {
      quote: { exact: now.text.slice(start, end), prefix: now.text.slice(Math.max(0, start - AROUND), start),
        suffix: now.text.slice(end, end + AROUND), section: partOf(now.at[start][0]) },
      box: whole.getBoundingClientRect()
    };
  }
  function offer(snap) {
    chosen = selected(snap);
    tools.hidden = !chosen;
    if (!chosen) return;
    shut(false);
    var named = names();
    Array.prototype.forEach.call(swatches.children, function (swatch, i) {
      swatch.title = named[swatch.dataset.color] + " (" + (i + 1) + ")";
      swatch.setAttribute("aria-label", "Comment as " + named[swatch.dataset.color]);
    });
    take.title = "Comment as " + named[ekkoThemes.last()];
    told(null);
    // Above the words, else below them when the window's top would cut it;
    // never past either side of the window.
    var top = chosen.box.top - tools.offsetHeight - 8;
    if (top < EDGE) top = chosen.box.bottom + 8;
    tools.style.top = scrollY + top + "px";
    tools.style.left = scrollX + Math.max(8, Math.min(chosen.box.left + chosen.box.width / 2 - tools.offsetWidth / 2, document.documentElement.clientWidth - tools.offsetWidth - 8)) + "px";
  }
  document.addEventListener("pointerup", function (event) { if (!tools.contains(event.target)) setTimeout(function () { offer(true); }, 0); });
  document.addEventListener("keyup", function (event) { if (event.shiftKey || event.key === "Shift") offer(false); });
  tools.addEventListener("pointerdown", function (event) { event.preventDefault(); });
  // The tools go with the selection: when it is gone, on Esc, and to where
  // the words are after the window changes size.
  document.addEventListener("selectionchange", function () { if (!tools.hidden && getSelection().isCollapsed) tools.hidden = true; });
  addEventListener("keydown", function (event) { if (event.key === "Escape" && !tools.hidden) tools.hidden = true; });
  addEventListener("resize", function () { if (!tools.hidden) offer(false); });
  function taken(color) {
    if (!chosen) return;
    tools.hidden = true;
    getSelection().removeAllRanges();
    ekkoQuote(chosen.quote, color || ekkoThemes.last());
  }
  take.addEventListener("click", function () { taken(null); });
  swatches.addEventListener("click", function (event) {
    var swatch = event.target.closest(".swatch");
    if (swatch) taken(swatch.dataset.color);
  });
  // C takes the words in the last theme, 1 to 6 in that theme, so a
  // selection made with Shift and the arrows needs no mouse.
  addEventListener("keydown", function (event) {
    if (tools.hidden || !chosen || event.ctrlKey || event.metaKey || event.altKey) return;
    var active = document.activeElement;
    if (active && (active.tagName === "INPUT" || active.tagName === "TEXTAREA" || active.isContentEditable)) return;
    var digit = Number(event.key);
    if (event.key === "c") { event.preventDefault(); taken(null); }
    else if (digit >= 1 && digit <= THEMES.length) { event.preventDefault(); taken(THEMES[digit - 1][0]); }
  });
})();"##;

/// The look of akqa.com (decision 1355, plan 1364), measured in note 1357:
/// its palette, light and dark, which a scene's mode crossfades in 1125 ms,
/// its openers in capitals and its reveals. The command bar is AKQA's too
/// (task 1223), and the section bars Medium's (note 1149). Sizes are the
/// references' own, in px. The fonts come before it, from `font_faces`.
const STYLE: &str = r##"
/* The colours a scene's mode changes, registered so they crossfade as
   AKQA's do when a dark scene comes in (task 1367): the page's in 1125 ms,
   the bar's glass and ink in 0.3 s, as AKQA's change. */
@property --page { syntax: "<color>"; inherits: true; initial-value: #fff; }
@property --bg { syntax: "<color>"; inherits: true; initial-value: #fff; }
@property --fg { syntax: "<color>"; inherits: true; initial-value: #191919; }
@property --fg-strong { syntax: "<color>"; inherits: true; initial-value: #000; }
@property --fg-2 { syntax: "<color>"; inherits: true; initial-value: #555; }
@property --fg-3 { syntax: "<color>"; inherits: true; initial-value: #8a8a8a; }
@property --dim { syntax: "<color>"; inherits: true; initial-value: rgba(0, 0, 0, 0.16); }
@property --rule { syntax: "<color>"; inherits: true; initial-value: #e9e9e9; }
@property --chip { syntax: "<color>"; inherits: true; initial-value: #f5f5f5; }
@property --raise { syntax: "<color>"; inherits: true; initial-value: #f5f5f5; }
@property --card { syntax: "<color>"; inherits: true; initial-value: #fff; }
@property --node { syntax: "<color>"; inherits: true; initial-value: #fff; }
@property --node-line { syntax: "<color>"; inherits: true; initial-value: #e2e2e2; }
@property --bars-bg { syntax: "<color>"; inherits: true; initial-value: rgba(255, 255, 255, 0.38); }
@property --bar-black { syntax: "<number>"; inherits: true; initial-value: 0.06; }
@property --bar-white { syntax: "<number>"; inherits: true; initial-value: 0.58; }
@property --bar-ink { syntax: "<color>"; inherits: true; initial-value: #000; }
@property --bar-ink-strong { syntax: "<color>"; inherits: true; initial-value: #191919; }
@property --bar-ink-idle { syntax: "<color>"; inherits: true; initial-value: #323232; }
@property --bar-ink-muted { syntax: "<color>"; inherits: true; initial-value: rgba(0, 0, 0, 0.66); }
@property --bar-ink-nav { syntax: "<color>"; inherits: true; initial-value: #393939; }
@property --bar-fill { syntax: "<color>"; inherits: true; initial-value: rgba(0, 0, 0, 0.04); }
@property --bar-fill-on { syntax: "<color>"; inherits: true; initial-value: rgba(0, 0, 0, 0.16); }
@property --bar-line { syntax: "<color>"; inherits: true; initial-value: rgba(0, 0, 0, 0.08); }

:root {
  --serif: "Ekko Serif", Georgia, Cambria, "Times New Roman", Times, serif;
  --sans: "Inter", "Helvetica Neue", Helvetica, Arial, sans-serif;
  --mono: ui-monospace, "SF Mono", Menlo, Consolas, "DejaVu Sans Mono", monospace;
  /* AKQA's frame: its curve, its gutters, its widest content, and the
     measure of a paragraph. */
  --curve: cubic-bezier(0.2, 0.65, 0.3, 1);
  --gutter: clamp(20px, 4.4vw, 64px);
  --wide: 1312px;
  --measure: 720px;
  /* AKQA's home: white, black ink, grey for what comes second. */
  --page: #fff;
  --bg: #fff;
  --fg: #191919;
  --fg-strong: #000;
  --fg-2: #555;
  --fg-3: #8a8a8a;
  --dim: rgba(0, 0, 0, 0.16);
  --rule: #e9e9e9;
  --chip: #f5f5f5;
  --raise: #f5f5f5;
  --node: #fff;
  --node-line: #e2e2e2;
  --accent: #1a8917;
  --card: #fff;
  --card-shadow: 0 0 4px rgba(36, 36, 36, 0.05), 0 2px 8px rgba(36, 36, 36, 0.15);
  --bars-bg: rgba(255, 255, 255, 0.38);
  --add: #e6ffec;
  --del: #ffebe9;
  /* AKQA's bar over a light page (task 1223, measured on akqa.com): its
     glass, a black wash under a white one and a backdrop blur, and the
     ink AKQA draws on it. */
  --bar-black: 0.06;
  --bar-white: 0.58;
  --bar-blur: 27px;
  --bar-ink: #000;
  --bar-ink-strong: #191919;
  --bar-ink-idle: #323232;
  --bar-ink-muted: rgba(0, 0, 0, 0.66);
  --bar-ink-nav: #393939;
  --bar-fill: rgba(0, 0, 0, 0.04);
  --bar-fill-on: rgba(0, 0, 0, 0.16);
  --bar-line: rgba(0, 0, 0, 0.08);
  --bar-rim: rgba(0, 0, 0, 0.16);
  --bar-halo: rgba(0, 0, 0, 0.25);
  --bar-core: #000;
  --pop-chip: rgba(0, 0, 0, 0.06);
  --pop-hover: rgba(0, 0, 0, 0.035);
  /* The comments' themes (task 1213): the tint their words take, and the
     ink of their pins and stripes, one pair per color of THEMES. */
  --tint-yellow: rgba(255, 200, 0, 0.3);
  --tint-blue: rgba(40, 120, 255, 0.18);
  --tint-pink: rgba(255, 40, 120, 0.16);
  --tint-green: rgba(30, 170, 80, 0.2);
  --tint-purple: rgba(130, 70, 255, 0.16);
  --tint-orange: rgba(255, 120, 0, 0.22);
  --ink-yellow: #d9a400;
  --ink-blue: #2f6fdf;
  --ink-pink: #e0336e;
  --ink-green: #1f9d55;
  --ink-purple: #7c4ddb;
  --ink-orange: #e86f00;
  color-scheme: light;
  transition: --page 1125ms ease, --bg 1125ms ease, --fg 1125ms ease, --fg-strong 1125ms ease, --fg-2 1125ms ease, --fg-3 1125ms ease, --dim 1125ms ease, --rule 1125ms ease, --chip 1125ms ease, --raise 1125ms ease, --card 1125ms ease, --node 1125ms ease, --node-line 1125ms ease, --bars-bg 0.3s ease, --bar-black 0.3s ease, --bar-white 0.3s ease, --bar-ink 0.3s ease, --bar-ink-strong 0.3s ease, --bar-ink-idle 0.3s ease, --bar-ink-muted 0.3s ease, --bar-ink-nav 0.3s ease, --bar-fill 0.3s ease, --bar-fill-on 0.3s ease, --bar-line 0.3s ease;
}

/* AKQA's case study: the dark theme, and a dark scene on the light page,
   as AKQA's home turns black under its statement. */
:root[data-theme="dark"], :root[data-mode="dark"] {
  --page: #000;
  --bg: #191919;
  --fg: #d9d9d9;
  --fg-strong: #fff;
  --fg-2: #ababab;
  --fg-3: #7b7b7b;
  --dim: rgba(255, 255, 255, 0.2);
  --rule: #434343;
  --chip: #262626;
  --raise: #262626;
  --node: #262626;
  --node-line: #434343;
  --accent: #fff;
  --card: #262626;
  --card-shadow: 0 8px 40px rgba(0, 0, 0, 0.25);
  --bars-bg: rgba(0, 0, 0, 0.38);
  --add: rgba(46, 160, 67, 0.18);
  --del: rgba(248, 81, 73, 0.18);
  /* AKQA's bar over a dark page. */
  --bar-black: 0.42;
  --bar-white: 0.1;
  --bar-blur: 25px;
  --bar-ink: #fff;
  --bar-ink-strong: #e9e9e9;
  --bar-ink-idle: #d9d9d9;
  --bar-ink-muted: rgba(255, 255, 255, 0.64);
  --bar-ink-nav: #bbb;
  --bar-fill: rgba(255, 255, 255, 0.04);
  --bar-fill-on: rgba(255, 255, 255, 0.16);
  --bar-line: rgba(255, 255, 255, 0.1);
  --bar-rim: rgba(255, 255, 255, 0.16);
  --bar-halo: rgba(255, 255, 255, 0.25);
  --bar-core: #fff;
  --pop-chip: rgba(255, 255, 255, 0.12);
  --pop-hover: rgba(255, 255, 255, 0.06);
  --tint-yellow: rgba(255, 210, 0, 0.26);
  --tint-blue: rgba(80, 150, 255, 0.3);
  --tint-pink: rgba(255, 80, 150, 0.28);
  --tint-green: rgba(60, 200, 110, 0.26);
  --tint-purple: rgba(160, 110, 255, 0.32);
  --tint-orange: rgba(255, 140, 30, 0.28);
  --ink-yellow: #ffd60a;
  --ink-blue: #5aa0ff;
  --ink-pink: #ff5a96;
  --ink-green: #46c878;
  --ink-purple: #aa78ff;
  --ink-orange: #ff9628;
}
:root[data-theme="dark"] { color-scheme: dark; }
/* In the dark theme a dark scene goes to black, the case study's deepest. */
:root[data-theme="dark"][data-mode="dark"] { --bg: #000; --chip: #191919; --raise: #191919; --card: #191919; --node: #191919; }

/* What a focus brings into view, as Tab moves it, stops clear of the pill
   and the round Review button beside it: their top edge is 116 px up the
   window, 60 under them and 56 high, and 16 more (WCAG's technique C43,
   task 1349). In Firefox the page's script asks for it again. */
html { background: var(--page); scroll-padding-bottom: 132px; }
body { margin: 0; overflow-x: clip; background: var(--bg); color: var(--fg); font: 400 16px/24px var(--sans); -webkit-font-smoothing: antialiased; -moz-osx-font-smoothing: grayscale; text-rendering: optimizeLegibility; }
a { color: inherit; }
button { font: inherit; color: inherit; }
[hidden] { display: none !important; }

/* What only the person's page offers: Review, the person's commands. */
body:not([data-writes]) .writes-only { display: none !important; }

/* ---- the frame: scenes the window's width, in AKQA's gutters ---------- */
/* No column beside the text, no head above it and no bar (tasks 1337,
   1367): what they held opens the first scene, heads History, or is run
   from the pill. A scene's content lines up on AKQA's widest line, and a
   paragraph keeps to the measure. What waits beside its scene, as What is
   known's track before it slides in, is cut at the window's edge, so the
   page is never wider than the window: body's clip reaches the window as
   hidden, which a script and a focus still scroll and a phone still opens
   wider (task 1403). A clip, unlike a hidden, scrolls nothing, so what
   sticks inside keeps sticking. */
.page { padding: 0 0 240px; overflow-x: clip; }
.scene { position: relative; box-sizing: border-box; padding: clamp(96px, 14vh, 160px) max(var(--gutter), calc((100% - var(--wide)) / 2)); }
.scene > * { max-width: var(--measure); }
.scene > :is(h1, .opener, .bleed, .facts, .waits, .held) { max-width: none; }

/* ---- the first scene: where the plan stands, and its title ------------ */
.hero { display: flex; flex-direction: column; justify-content: center; min-height: 100vh; padding-top: 96px; padding-bottom: 168px; }
.prose .kicker { display: flex; flex-wrap: wrap; align-items: center; gap: 12px 16px; margin: 0 0 clamp(24px, 4vh, 48px); font: 400 14px/20px var(--sans); color: var(--fg-2); }
.kicker .state { display: inline-flex; align-items: center; gap: 8px; min-height: 32px; padding: 0 14px; border-radius: 999px; background: var(--raise); color: var(--fg-strong); }
.kicker .state::before { content: ""; flex: none; width: 8px; height: 8px; border-radius: 50%; background: currentColor; }
.kicker .state.waiting { background: var(--fg-strong); color: var(--bg); }
.kicker .state.waiting::before { animation: breathe 1.6s ease-in-out infinite; }
@keyframes breathe { 50% { opacity: 0.25; } }
.hero h1 { margin: 0; font: 400 clamp(40px, 7.2vw, 104px)/0.92 var(--sans); letter-spacing: -0.04em; text-transform: uppercase; overflow-wrap: anywhere; color: var(--fg-strong); }
.hero h1 .l1, .hero h1 .l2 { display: block; }
.hero h1 .l2 { color: var(--fg-3); }
/* A plan that waits on the user gives its banner the room: the title steps
   down, so the facts stay clear of the pill. */
.hero:has(.waits) h1 { font-size: clamp(36px, 5.6vw, 80px); }
/* The plan's facts, AKQA's row of results: a number in serif over what it
   counts, rising as the pointer comes. */
.facts { display: grid; grid-template-columns: repeat(auto-fit, minmax(160px, 1fr)); margin-top: clamp(40px, 8vh, 96px); border-top: 1px solid var(--rule); }
.hero .fact { display: flex; flex-direction: column; align-items: flex-start; gap: 6px; padding: 20px 24px 0 0; color: var(--fg-strong); text-decoration: none; }
.fact b { font: 400 clamp(36px, 4.4vw, 64px)/1 var(--serif); letter-spacing: -0.02em; transition: transform 0.6s var(--curve); }
.fact span { font: 400 14px/20px var(--sans); color: var(--fg-2); transition: color 0.3s ease; }
.fact:hover b { transform: translateY(-4px); }
.fact:hover span { color: var(--fg-strong); }
.fact:focus-visible, .pill:focus-visible { outline: 2px solid var(--fg-strong); outline-offset: 3px; }
/* AKQA's pill button. */
.pill { display: inline-flex; flex: none; align-items: center; gap: 8px; height: 40px; padding: 0 18px; border: 0; border-radius: 999px; background: var(--raise); font: 400 14px/40px var(--sans); color: var(--fg); cursor: pointer; transition: background-color 0.3s ease, color 0.3s ease; }
.callout { margin: 32px 0 0; padding: 20px 24px; border-radius: 16px; background: var(--chip); font: 400 16px/24px var(--sans); color: var(--fg); }
.callout + .callout { margin-top: 16px; }
.callout b { display: block; margin: 0 0 4px; font-weight: 600; color: var(--fg-strong); }
/* What waits on the user: AKQA's black banner, inverted in either mode. */
.callout.waits { display: flex; flex-wrap: wrap; align-items: center; gap: 16px 24px; border-radius: 20px; background: var(--fg-strong); color: var(--bg); }
.callout.waits .said { flex: 1 1 320px; font: 400 20px/28px var(--serif); }
.callout.waits b { margin: 0 0 6px; font: 400 13px/16px var(--sans); letter-spacing: 0.08em; text-transform: uppercase; color: inherit; opacity: 0.7; }
.prose .callout.waits code { background: rgba(127, 127, 127, 0.25); color: inherit; }
.callout.waits .pill { background: var(--bg); color: var(--fg-strong); }
/* A question waiting on the user, as ekko's menu shows it (task 1110): its
   options numbered, the recommended one marked, and beside them -- under
   them, on a narrow window -- why pick the one in focus, an example and its
   preview; where the page writes, the fields that answer it. */
.callout.waits.asks { display: block; }
.callout.waits.asks p { margin: 0; color: inherit; }
.callout.waits.asks .asked { font: 400 20px/28px var(--serif); white-space: pre-line; }
.callout.waits.asks .explain { margin-top: 12px; font: 400 16px/24px var(--sans); white-space: pre-line; opacity: 0.85; }
.callout.waits.asks .any { margin-top: 12px; font: 400 13px/16px var(--sans); letter-spacing: 0.08em; text-transform: uppercase; opacity: 0.7; }
.asks .choose { display: grid; gap: 20px 32px; margin-top: 20px; }
.asks .choose.aided { grid-template-columns: minmax(0, 1fr) minmax(0, 1fr); }
.asks .options { display: flex; flex-direction: column; border-top: 1px solid rgba(127, 127, 127, 0.35); }
.asks .option { display: grid; grid-template-columns: 28px minmax(0, 1fr); gap: 2px 12px; align-items: start; padding: 12px 8px; border: 0; border-bottom: 1px solid rgba(127, 127, 127, 0.35); background: none; color: inherit; font: 400 16px/24px var(--sans); text-align: left; cursor: pointer; transition: background-color 0.2s ease; }
.asks .option .n { display: inline-flex; align-items: center; justify-content: center; width: 24px; height: 24px; margin-top: 0; border: 1px solid currentColor; border-radius: 50%; font: 400 12px/1 var(--sans); opacity: 0.7; }
.asks .option[role="checkbox"] .n { border-radius: 7px; }
.asks .option[aria-checked="true"] .n { border-color: var(--bg); background: var(--bg); color: var(--fg-strong); opacity: 1; }
.asks .option .label { font-weight: 600; overflow-wrap: anywhere; }
.asks .option .tag { display: inline-block; margin-left: 8px; padding: 0 8px; border-radius: 999px; background: rgba(127, 127, 127, 0.3); font: 400 12px/20px var(--sans); letter-spacing: 0.02em; vertical-align: 1px; }
.asks .option .desc { grid-column: 2; font-size: 15px; line-height: 22px; opacity: 0.75; }
.asks .option:hover { background: rgba(127, 127, 127, 0.16); }
.asks .option:focus-visible { outline: 2px solid var(--bg); outline-offset: -2px; }
.asks .aids { padding: 0 0 0 24px; border-left: 1px solid rgba(127, 127, 127, 0.35); }
.callout.waits.asks .aid > * + * { margin-top: 12px; }
.callout.waits.asks .aid .why { font: 400 18px/26px var(--serif); }
.callout.waits.asks .aid .example { font: 400 15px/22px var(--sans); }
.asks .aid .example span { font-weight: 600; }
.asks .aid .preview { max-height: 320px; margin: 0; padding: 12px 16px; overflow: auto; border-radius: 12px; background: rgba(127, 127, 127, 0.2); color: inherit; font: 400 13px/20px var(--mono); }
.callout.waits.asks .aid .none { font: 400 15px/22px var(--sans); opacity: 0.6; }
.asks .reply { display: flex; flex-wrap: wrap; align-items: center; gap: 12px; margin-top: 20px; }
.asks .reply textarea, .asks .reply input { box-sizing: border-box; min-height: 40px; padding: 8px 16px; border: 1px solid rgba(127, 127, 127, 0.45); border-radius: 20px; background: rgba(127, 127, 127, 0.12); color: inherit; font: 400 15px/22px var(--sans); }
.asks .reply textarea { flex: 1 1 100%; min-height: 64px; resize: vertical; }
.asks .reply input { flex: 1 1 240px; }
.asks .reply :is(textarea, input)::placeholder { color: inherit; opacity: 0.6; }
.asks .reply :is(textarea, input):focus-visible { outline: 2px solid var(--bg); outline-offset: 1px; }
.asks .reply .pill:disabled { opacity: 0.5; cursor: default; }
.callout.waits.asks .why-not { flex: 1 1 100%; font: 400 14px/20px var(--sans); }
.asks .why-not:empty { display: none; }
.callout.waits.asks .how { margin-top: 16px; font: 400 14px/20px var(--sans); opacity: 0.75; }
.asks.answered :is(.choose, .reply > :not(.why-not)) { opacity: 0.5; pointer-events: none; }
@media (max-width: 760px) { .asks .choose.aided { grid-template-columns: minmax(0, 1fr); } .asks .aids { padding: 16px 0 0; border-left: 0; border-top: 1px solid rgba(127, 127, 127, 0.35); } }
.hero .lead { margin-top: 40px; }

/* ---- openers, as AKQA's: capitals, weight 400, tight, line two grey --- */
.opener { margin: 0 0 clamp(40px, 6vh, 72px); font: 400 clamp(40px, 6vw, 88px)/0.92 var(--sans); letter-spacing: -0.035em; text-transform: uppercase; overflow-wrap: anywhere; color: var(--fg-strong); }
.opener .l1, .opener .l2 { display: block; }
.opener .l2 { color: var(--fg-3); }

/* ---- the plan: the Goal's statement in AKQA's serif, the text in its sans */
.prose .statement { max-width: 1100px; margin: 0 0 40px; font: 400 clamp(28px, 3.4vw, 50px)/1.16 var(--serif); letter-spacing: -0.015em; color: var(--fg-strong); }
:where(.prose) :is(p, li) { margin: 0 0 16px; font: 400 16px/24px var(--sans); color: var(--fg); }
:where(.prose) :is(ul, ol) { margin: 8px 0 16px; padding-left: 20px; }
:where(.prose) li li, :where(.prose) li p { margin: 6px 0; }
.prose code { padding: 1px 5px; border-radius: 4px; background: var(--chip); font: 400 0.82em/1 var(--mono); color: var(--fg-strong); }
.prose .callout code { background: var(--bg); }
.prose strong { font-weight: 700; }
.prose a { text-decoration: underline; text-decoration-thickness: 1px; text-underline-offset: 3px; }
:where(.prose [data-plan]) :is(h1, h2):not(.opener) { margin: 40px 0 16px; font: 400 28px/34px var(--sans); letter-spacing: -0.016em; color: var(--fg-strong); }
:where(.prose) h3 { margin: 32px 0 12px; font: 600 22px/28px var(--sans); letter-spacing: -0.016em; color: var(--fg-strong); }
:where(.prose) :is(h4, h5, h6) { margin: 24px 0 8px; font: 600 18px/24px var(--sans); color: var(--fg-strong); }
:where(.prose) pre { margin: 24px 0; padding: 16px 20px; overflow-x: auto; border-radius: 16px; background: var(--chip); font: 400 14px/22px var(--mono); }
.prose pre code { padding: 0; background: none; font: inherit; }
:where(.prose) blockquote { margin: 24px 0; padding-left: 20px; border-left: 2px solid var(--fg-strong); }
:where(.prose) blockquote p { font-style: italic; }
:where(.prose) table { width: 100%; margin: 24px 0; border-collapse: collapse; font: 400 15px/22px var(--sans); }
.prose th, .prose td { padding: 8px 10px; border-bottom: 1px solid var(--rule); text-align: left; vertical-align: top; }
.prose th { font-weight: 600; color: var(--fg-strong); }
:where(.prose) hr { height: 1px; margin: 32px 0; border: 0; background: var(--rule); }

/* ---- the Goal: its statement held while the scroll lights its words --- */
/* As tall as the room the scroll takes to light the statement (--tall, in
   the reveals' block), the holder keeping it in the window (task 1369). */
.scene.goal { padding-top: 0; padding-bottom: 0; }
.goal .held { position: sticky; top: 0; display: flex; flex-direction: column; justify-content: center; box-sizing: border-box; min-height: 100vh; max-width: 1100px; padding: 96px 0; }
.goal .label { margin: 0 0 20px; font: 400 13px/16px var(--sans); letter-spacing: 0.08em; text-transform: uppercase; color: var(--fg-2); }
.prose .goal .statement { margin: 0; }
.goal-rest { max-width: 640px; margin-top: 40px; }
:where(.goal-rest) :is(p, li) { font-size: 18px; line-height: 28px; }
.goal .progress { position: absolute; right: 0; bottom: 28px; left: 0; height: 1px; background: var(--rule); }
.goal .progress i { display: block; height: 100%; background: var(--fg-strong); transform: scaleX(0); transform-origin: 0 50%; }

/* ---- What is known: a tab a group, a card a finding (task 1370) ------- */
/* The cards ride a track the window's width, as AKQA's carousels do, the
   first at the text's edge; one opens in place, wider, to the whole of it. */
.known .tabs { display: flex; flex-wrap: wrap; gap: 8px; margin: 0 0 16px; }
.tab sup { font-size: 11px; line-height: 1; color: var(--fg-2); }
.tab[aria-selected="true"] { background: var(--fg-strong); color: var(--bg); }
.tab[aria-selected="true"] sup { color: inherit; opacity: 0.7; }
.scene > :is(.group, .controls) { max-width: none; }
.prose .group > p { max-width: var(--measure); margin: 0 0 32px; font: 400 15px/24px var(--sans); color: var(--fg-2); }
.prose .track { display: flex; align-items: stretch; gap: 16px; margin: 0 calc(50% - 50vw); padding: 8px max(var(--gutter), calc((100vw - var(--wide)) / 2)) 24px; overflow-x: auto; list-style: none; scroll-snap-type: x mandatory; scroll-padding-inline: max(var(--gutter), calc((100vw - var(--wide)) / 2)); scrollbar-width: none; }
.track::-webkit-scrollbar { display: none; }
.prose .card { position: relative; flex: none; box-sizing: border-box; display: flex; flex-direction: column; width: min(380px, 82vw); min-height: 300px; margin: 0; padding: 24px; border-radius: 20px; background: var(--raise); font: 400 15px/22px var(--sans); scroll-snap-align: start; cursor: pointer; transition: width 0.6s var(--curve); }
.card .num { font: 400 13px/16px var(--sans); color: var(--fg-2); }
.card .body { display: -webkit-box; margin: 40px 0 20px; overflow: hidden; overflow-wrap: anywhere; color: var(--fg); -webkit-box-orient: vertical; -webkit-line-clamp: 7; }
.card .lead { display: block; margin-bottom: 12px; font: 400 22px/28px var(--serif); letter-spacing: -0.01em; color: var(--fg-strong); }
.prose .card .body :is(p, li) { margin: 0 0 8px; font: inherit; color: inherit; }
.prose .card .body :is(ul, ol) { margin: 8px 0 0; padding-left: 18px; }
.card .more { align-self: flex-start; margin-top: auto; padding: 0; border: 0; background: none; font: 400 15px/24px var(--sans); color: var(--fg-2); cursor: pointer; transition: color 0.15s ease; }
.card .more:hover { color: var(--fg-strong); }
.prose .card.open { width: min(680px, 90vw); cursor: default; }
.card.open .body { display: block; overflow: visible; -webkit-line-clamp: none; }
:is(.known, .risks, .staged) .controls { display: flex; align-items: center; gap: 12px; margin: 8px 0 0; }
.controls .count { min-width: 64px; font: 400 14px/20px var(--sans); font-variant-numeric: tabular-nums; color: var(--fg-2); }
.controls .grow { flex: 1; }
.round { display: inline-grid; place-items: center; flex: none; width: 48px; height: 48px; padding: 0; border: 0; border-radius: 50%; background: var(--raise); color: var(--fg-strong); cursor: pointer; transition: transform 0.45s var(--curve), opacity 0.3s ease; }
.round:hover:not(:disabled) { transform: scale(1.06); }
.round:active:not(:disabled) { transform: scale(0.94); }
.round:disabled { opacity: 0.3; cursor: default; }
.round svg { width: 20px; height: 20px; fill: none; stroke: currentColor; stroke-width: 1.6; stroke-linecap: round; stroke-linejoin: round; }
:is(.tab, .round, .card .more, .track):focus-visible { outline: 2px solid var(--fg-strong); outline-offset: 3px; }

/* ---- the Design: a numeral held beside its points (task 1371) -------- */
/* As AKQA's grouped sections: the number of the point at the window's
   middle, held in place and turning as the next comes up; that point lit
   and the others grey, once the script follows them. A lead that opens
   with bold words names its point: the rest of it is grey. */
.scene > .story { max-width: none; }
.story { display: grid; grid-template-columns: minmax(200px, 5fr) 7fr; gap: 0 clamp(32px, 5vw, 96px); }
.hold { position: sticky; top: 0; display: flex; flex-direction: column; justify-content: center; height: 100vh; }
.numeral { position: relative; height: clamp(120px, 15vw, 220px); overflow: hidden; font: 400 clamp(120px, 15vw, 220px)/1 var(--sans); letter-spacing: -0.06em; color: var(--fg-strong); }
.numeral span { position: absolute; top: 0; left: 0; }
.numeral .out { animation: turn-out 0.28s var(--curve) forwards; }
.numeral .enter { animation: turn-in 0.42s var(--curve) 0.28s both; }
@keyframes turn-out { to { opacity: 0; transform: translateY(-80px); } }
@keyframes turn-in { from { opacity: 0; transform: translateY(80px); } }
.hold .of { margin-top: 12px; font: 400 14px/20px var(--sans); color: var(--fg-2); }
.ticks { display: flex; gap: 6px; margin-top: 20px; }
.ticks button { width: 28px; height: 20px; padding: 0; border: 0; background: none; cursor: pointer; }
.ticks button::before { content: ""; display: block; height: 2px; border-radius: 1px; background: var(--fg-strong); opacity: 0.18; transition: opacity 0.3s ease; }
.ticks button[aria-current="true"]::before { opacity: 1; }
.ticks button:focus-visible { outline: 2px solid var(--fg-strong); outline-offset: 3px; }
.prose .points { margin: 0; padding: 30vh 0 34vh; list-style: none; }
.prose .point { margin: 0; padding: 0 0 96px; transition: opacity 0.5s var(--curve); }
.turning .point:not(.on) { opacity: 0.22; }
.point .lead { display: block; margin-bottom: 16px; font: 400 clamp(22px, 2.1vw, 30px)/1.25 var(--serif); letter-spacing: -0.012em; color: var(--fg-strong); }
.point .lead.named { color: var(--fg-3); }
.prose .point .lead strong { font-weight: inherit; color: var(--fg-strong); }
/* Narrow, the numeral holds above the points, under the mark. */
@media (max-width: 900px) {
  .story { grid-template-columns: 1fr; }
  .hold { z-index: 1; flex-direction: row; justify-content: flex-start; align-items: flex-end; gap: 16px; height: auto; padding: 60px 0 12px; background: var(--bg); }
  .numeral { flex: none; width: 1.3em; height: 64px; font-size: 64px; }
  .ticks { display: none; }
  .prose .points { padding: 24px 0; }
  .turning .point:not(.on) { opacity: 1; }
}

/* ---- the Risks: a stack of sheets, one in front (task 1372) ----------- */
/* Every sheet in the stack's one cell, so the stack is as tall as its
   tallest sheet and nothing under it moves as it turns. --d, a sheet's
   distance from the one in front, sets it back by its bottom edge, the
   next two showing under it and the rest out of sight; one sent away goes
   off to the left, turning, above the rest, as a card dealt does. Their
   z-index stays inside the stack, under the bar and the popovers. An open
   question is the dark sheet. All at once, a grid. */
.prose .stack { display: grid; isolation: isolate; margin: 0; padding: 0 0 36px; list-style: none; }
.prose .sheet { grid-area: 1 / 1; z-index: calc(100 - var(--d, 0)); box-sizing: border-box; min-height: 240px; margin: 0; padding: 32px; border-radius: 24px; background: var(--card); box-shadow: 0 8px 40px rgba(0, 0, 0, 0.08), inset 0 0 0 1px var(--rule); font: 400 16px/24px var(--sans); transform: translateY(calc(var(--d, 0) * 18px)) scale(calc(1 - var(--d, 0) * 0.045)); transform-origin: 50% 100%; opacity: clamp(0, 3 - var(--d, 0), 1); transition: transform 0.6s var(--curve), opacity 0.45s var(--curve); }
.prose .sheet.gone { transform: translate(-28%, 24px) rotate(-7deg); opacity: 0; }
.sheet .num { display: flex; justify-content: space-between; gap: 16px; font: 400 13px/16px var(--sans); color: var(--fg-2); }
.sheet .body { margin-top: 32px; overflow-wrap: anywhere; color: var(--fg); }
.sheet .lead { display: block; margin-bottom: 12px; font: 400 24px/30px var(--serif); letter-spacing: -0.01em; color: var(--fg-strong); }
.prose .sheet .body :is(p, li) { margin: 0 0 8px; font: inherit; color: inherit; }
.prose .sheet .body :is(ul, ol) { margin: 8px 0 0; padding-left: 18px; }
.prose .sheet.ask { background: var(--fg-strong); box-shadow: 0 8px 40px rgba(0, 0, 0, 0.12); }
.sheet.ask :is(.num, .body, .lead) { color: var(--bg); }
.scene.risks > .controls { max-width: var(--measure); }
.scene > .stack.all, .scene.risks > .stack.all + .controls { max-width: none; }
.prose .stack.all { grid-template-columns: repeat(auto-fill, minmax(300px, 1fr)); gap: 16px; padding: 0; }
.prose .stack.all .sheet { grid-area: auto; z-index: auto; transform: none; opacity: 1; transition: none; }
.toggle { display: inline-flex; align-items: center; gap: 10px; padding: 0; border: 0; background: none; font: 400 14px/20px var(--sans); color: var(--fg-2); cursor: pointer; }
.toggle i { position: relative; width: 40px; height: 24px; border-radius: 12px; background: var(--raise); box-shadow: inset 0 0 0 1px var(--rule); transition: background-color 0.3s ease; }
.toggle i::after { content: ""; position: absolute; top: 3px; left: 3px; width: 18px; height: 18px; border-radius: 50%; background: var(--fg-strong); transition: transform 0.45s var(--curve), background-color 0.3s ease; }
.toggle[aria-checked="true"] i { background: var(--fg-strong); }
.toggle[aria-checked="true"] i::after { background: var(--bg); transform: translateX(16px); }
:is(.stack, .toggle):focus-visible { outline: 2px solid var(--fg-strong); outline-offset: 3px; }
@media (max-width: 600px) {
  .prose .sheet { min-height: 200px; padding: 24px; }
  .sheet .lead { font-size: 21px; line-height: 27px; }
}

/* ---- steps: AKQA's numbered items -------------------------------------- */
.prose .steps { margin: 0; padding: 0; list-style: none; border-top: 1px solid var(--rule); }
.prose .step { margin: 0; padding: 0; border-bottom: 1px solid var(--rule); scroll-margin: 96px 0; }
.step-head { display: grid; grid-template-columns: 40px 1fr; gap: 4px 8px; box-sizing: border-box; width: 100%; padding: 24px 0; border: 0; background: none; text-align: left; }
button.step-head { cursor: pointer; }
.step .num { font: 400 20px/24px var(--sans); color: var(--fg-2); }
.step .title { font: 400 20px/24px var(--serif); color: var(--fg-strong); }
.step .meta { grid-column: 2; display: flex; flex-wrap: wrap; align-items: center; gap: 0 8px; font: 400 14px/21px var(--sans); color: var(--fg-2); }
.step.cancelled .title, .step.gone .title { color: var(--fg-2); text-decoration: line-through; }
.dot { display: inline-block; flex: none; width: 8px; height: 8px; border-radius: 50%; box-shadow: inset 0 0 0 1.5px var(--fg-2); }
.dot.done { background: var(--fg); box-shadow: none; }
.dot.progress { background: var(--accent); box-shadow: none; }
.dot.proposed { box-shadow: none; outline: 1.5px dashed var(--fg-2); outline-offset: -1.5px; }
.dot.cancelled, .dot.gone { background: var(--chip); }
.step .more { display: none; padding: 0 0 24px 48px; }
.step.open .more { display: block; }
.prose .step .more p { margin: 0 0 12px; font: 400 16px/24px var(--sans); white-space: pre-wrap; overflow-wrap: anywhere; color: var(--fg); }
.prose .step .more .when { color: var(--fg-2); }
.step .more .when b { font-weight: 600; color: var(--fg); }

/* ---- the Steps on a stage (task 1373) ---------------------------------- */
/* AKQA's ROLE carousel: a segment a step above the stage, the done ones
   darker and the one on the stage solid; that step whole, its number large
   beside its title, the steps it waits on as chips. One leaving the stage
   is out of the flow while it goes. The stage holds on a screen only:
   print is the list, every step in it. */
.scene.staged > :is(.segs, .steps) { max-width: none; }
.staged .segs { display: flex; gap: 4px; margin: 0 0 40px; }
.seg { position: relative; flex: 1; min-width: 0; height: 44px; padding: 14px 0 0; border: 0; background: none; text-align: left; cursor: pointer; }
.seg::before { content: ""; position: absolute; top: 0; right: 0; left: 0; height: 2px; border-radius: 1px; background: var(--fg-strong); opacity: 0.14; transition: opacity 0.3s ease; }
.seg.done::before { opacity: 0.55; }
.seg[aria-current="step"]::before { opacity: 1; }
.seg span { display: block; overflow: hidden; font: 400 12px/16px var(--sans); white-space: nowrap; text-overflow: ellipsis; color: var(--fg-2); }
.seg[aria-current="step"] span { color: var(--fg-strong); }
.step .after { display: flex; flex-wrap: wrap; gap: 8px; margin: 12px 0 0; }
.after .chip { height: 32px; padding: 0 14px; font-size: 13px; line-height: 32px; }
.staged .steps.all .after { display: none; }
/* Start on a step a session may take up now (task 1111): Kiro's Start task,
   the one filled pill of the step; once sent, a line saying so. */
.step .start { display: flex; flex-wrap: wrap; align-items: center; gap: 12px; margin: 12px 0 16px 48px; }
.step .start .pill { background: var(--fg-strong); color: var(--bg); }
.step .start .pill:disabled { opacity: 0.5; cursor: default; }
.step .start .status:empty { display: none; }
.step :is(.start .status, .start.asked) { font: 400 14px/20px var(--sans); color: var(--fg-2); }
.staged .steps:not(.all) .step .start { margin-left: 0; }
@media print { .step .start:not(.asked) { display: none; } }
.staged .controls { margin-top: 32px; }
:is(.seg, .staged .steps):focus-visible { outline: 2px solid var(--fg-strong); outline-offset: 3px; }
@media screen {
  .staged .steps:not(.all) { position: relative; min-height: 340px; border: 0; }
  .staged .steps:not(.all) .step { position: relative; min-height: 200px; padding: 0 0 0 clamp(150px, 24%, 320px); border: 0; }
  .staged .steps:not(.all) .step.leaving { position: absolute; top: 0; right: 0; left: 0; pointer-events: none; }
  .staged .steps:not(.all) .step-head { display: block; padding: 0; }
  .staged .steps:not(.all) .step .num { position: absolute; top: 0; left: 0; font: 400 clamp(88px, 10vw, 160px)/0.85 var(--sans); letter-spacing: -0.06em; color: var(--fg-strong); }
  .staged .steps:not(.all) .step .title { display: block; font: 400 clamp(26px, 2.6vw, 38px)/1.18 var(--serif); letter-spacing: -0.015em; }
  .staged .steps:not(.all) .step .meta { margin-top: 16px; }
  .staged .steps:not(.all) .step .more { max-width: 720px; padding: 24px 0 0; }
  .prose .staged .steps:not(.all) .step .more p { font-size: 17px; line-height: 27px; }
  .prose .staged .steps:not(.all) .step .more .when { padding: 16px 20px; border-radius: 16px; background: var(--raise); font-size: 15px; line-height: 23px; }
}
@media screen and (max-width: 720px) {
  .staged .steps:not(.all) .step { padding: 84px 0 0; }
  .staged .steps:not(.all) .step .num { font-size: 72px; }
  .seg { height: 20px; }
  .seg span { display: none; }
}

/* ---- the map: AKQA's strip, the window's width ------------------------ */
.bleed { position: relative; margin: 0 calc(50% - 50vw); }
.strip { overflow-x: auto; padding: 8px max(var(--gutter), calc((100% - var(--wide)) / 2)) 24px; scrollbar-width: none; }
.strip::-webkit-scrollbar { display: none; }
.map { position: relative; }
.map svg { position: absolute; inset: 0; overflow: visible; }
.map .edge { fill: none; stroke: var(--fg-2); stroke-opacity: 0.5; stroke-width: 1.2; }
.map .arrowhead { fill: var(--fg-2); fill-opacity: 0.5; }
.map .node { position: absolute; box-sizing: border-box; display: flex; flex-direction: column; width: 200px; height: 92px; padding: 12px 14px; border: 0; border-radius: 8px; background: var(--node); box-shadow: inset 0 0 0 1px var(--node-line); text-align: left; cursor: pointer; }
.map .node:hover { box-shadow: inset 0 0 0 1px var(--fg-2); }
.map .node.proposed { box-shadow: none; outline: 1px dashed var(--fg-2); outline-offset: -1px; }
.map .k { font: 400 11px/16px var(--mono); color: var(--fg-2); }
.map .t { display: -webkit-box; margin-top: 2px; overflow: hidden; font: 400 13px/18px var(--sans); color: var(--fg-strong); -webkit-line-clamp: 2; -webkit-box-orient: vertical; }
.map .node.cancelled .t, .map .node.gone .t { color: var(--fg-2); text-decoration: line-through; }
.map .s { position: absolute; right: 14px; bottom: 10px; left: 14px; display: flex; justify-content: space-between; font: 400 11px/16px var(--sans); color: var(--fg-2); }
.map .s .dot { width: 6px; height: 6px; margin-right: 6px; vertical-align: 1px; }
.arrows { display: flex; justify-content: flex-end; gap: 8px; max-width: var(--wide); margin: 8px auto 0; padding: 0 var(--gutter); }
.arrows button { display: grid; place-items: center; width: 40px; height: 40px; padding: 0 0 2px; border: 1px solid var(--rule); border-radius: 50%; background: var(--bg); font: 400 24px/1 var(--sans); color: var(--fg-strong); cursor: pointer; }
.arrows button:hover:not(:disabled) { background: var(--chip); }
.arrows button:disabled { opacity: 0.3; cursor: default; }
.legend { display: flex; flex-wrap: wrap; gap: 8px 20px; margin: 24px 0 0; font: 400 13px/20px var(--sans); color: var(--fg-2); }
.legend > span { display: inline-flex; align-items: center; gap: 8px; }
/* The map played (task 1374): Play, the slider and the line saying the
   stage above it; a stage not reached yet faint and smaller, the curves to
   it undrawn, each drawn from the step it leaves as its stage comes. A step
   pointed at lights what it waits on and what waits on it, in ink, and the
   rest fades. On a screen only: print shows the map whole. The line keeps
   to one, so that the map under it stays where it is as the stages go. */
.player { display: grid; grid-template-columns: auto minmax(0, 1fr); align-items: center; column-gap: 20px; margin: 32px 0 0; }
.player .scrub { margin: 0; }
.prose .player .at { grid-column: 2; margin: 2px 0 0; overflow: hidden; font: 400 14px/20px var(--sans); white-space: nowrap; text-overflow: ellipsis; color: var(--fg-2); }
@media screen {
  .map .node { transition: opacity 0.5s var(--curve), transform 0.5s var(--curve), box-shadow 0.3s ease; }
  .map .edge { stroke-dasharray: 1; transition: stroke-dashoffset 0.6s var(--curve), opacity 0.3s ease, stroke 0.3s ease, stroke-opacity 0.3s ease; }
  .map .node.unlit { opacity: 0.12; transform: scale(0.96); }
  .map .edge.unlit { opacity: 0; stroke-dashoffset: 1; }
  .map.focus .node:not(.chain) { opacity: 0.18; }
  .map.focus .edge:not(.chain) { opacity: 0.1; }
  .map.focus .node.chain { opacity: 1; transform: none; box-shadow: inset 0 0 0 1.5px var(--fg-strong); }
  .map.focus .edge.chain { opacity: 1; stroke-dashoffset: 0; stroke: var(--fg-strong); stroke-opacity: 1; }
}

/* ---- notes and history ------------------------------------------------- */
.note, .version { padding: 24px 0; border-top: 1px solid var(--rule); scroll-margin-top: 24px; }
.note:last-child, .version:last-child { border-bottom: 1px solid var(--rule); }
.note .kind, .version .kind { font: 400 13px/20px var(--sans); color: var(--fg-2); }
.note.open .kind { color: var(--accent); }
.prose .note h3, .prose .version h3 { margin: 4px 0 0; font: 600 20px/24px var(--sans); letter-spacing: -0.01em; color: var(--fg-strong); }
.note .answer { display: inline-block; margin: 12px 0 0; padding: 4px 12px; border-radius: 999px; background: var(--chip); font: 400 13px/20px var(--sans); color: var(--fg); }
.note .verdict { margin: 8px 0 0; font: 400 14px/20px var(--sans); color: var(--fg-2); }
.prose .note .text { margin: 12px 0 0; font: 400 18px/28px var(--serif); white-space: pre-wrap; overflow-wrap: anywhere; color: var(--fg); }
.note details { margin: 12px 0 0; }
.note summary { font: 400 14px/20px var(--sans); color: var(--fg-2); cursor: pointer; }
/* ---- History: the texts kept on a line, a slider through the changes - */
/* As AKQA's timelines (task 1376): a point a text, its version in serif
   over the date it was made, the points of the change shown filled; under
   them, with two changes or more, a slider whose start is the oldest. The
   change shown alone on a screen; print shows each. A line of seven points
   or more leaves the dates to the changes. */
.prose .history { margin: 0 0 24px; font: 400 clamp(22px, 2vw, 28px)/1.3 var(--serif); color: var(--fg-strong); }
.scene > :is(.versions, .changed) { max-width: 920px; }
.prose .line { position: relative; display: flex; justify-content: space-between; margin: 40px 0 8px; padding: 0; list-style: none; }
.line::before { content: ""; position: absolute; top: 6px; right: 6px; left: 6px; height: 1px; background: var(--rule); }
.prose .line li { margin: 0; }
.line button { position: relative; display: flex; flex-direction: column; align-items: center; gap: 8px; padding: 0; border: 0; background: none; font: 400 13px/16px var(--sans); white-space: nowrap; color: var(--fg-2); cursor: pointer; }
.line li:first-child button { align-items: flex-start; }
.line li:last-child button { align-items: flex-end; }
.line button::before { content: ""; width: 13px; height: 13px; border-radius: 50%; background: var(--bg); box-shadow: inset 0 0 0 1px var(--fg-2); transition: background-color 0.3s ease, transform 0.45s var(--curve); }
.line button[aria-current="true"] { color: var(--fg-strong); }
.line button[aria-current="true"]::before { background: var(--fg-strong); box-shadow: none; transform: scale(1.25); }
.line button b { font: 400 28px/1 var(--serif); }
.line:has(li:nth-child(7)) button span { display: none; }
:is(.line button, .scrub):focus-visible { outline: 2px solid var(--fg-strong); outline-offset: 3px; }
/* The slider thin, as AKQA's: a hairline, the part before the thumb in ink
   (--p, which the script keeps; Firefox draws it as the progress). */
.scrub { -webkit-appearance: none; appearance: none; display: block; width: 100%; height: 24px; margin: 16px 0 32px; background: none; cursor: pointer; }
.scrub::-webkit-slider-runnable-track { height: 2px; border-radius: 1px; background: linear-gradient(to right, var(--fg-strong) var(--p, 100%), var(--rule) var(--p, 100%)); }
.scrub::-webkit-slider-thumb { -webkit-appearance: none; width: 18px; height: 18px; margin-top: -8px; border: 0; border-radius: 50%; background: var(--fg-strong); }
.scrub::-moz-range-track { height: 2px; border-radius: 1px; background: var(--rule); }
.scrub::-moz-range-progress { height: 2px; border-radius: 1px; background: var(--fg-strong); }
.scrub::-moz-range-thumb { width: 18px; height: 18px; border: 0; border-radius: 50%; background: var(--fg-strong); }
.changed .diff { border-radius: 16px; }
@media screen {
  .changed .version { padding: 24px 0 0; border: 0; }
}
@media (max-width: 600px) {
  .line button span { display: none; }
  .line button b { font-size: 22px; }
}
.diff { margin: 12px 0 0; overflow: hidden; border-radius: 4px; background: var(--chip); font: 400 13px/20px var(--mono); }
.diff div { padding: 1px 12px; white-space: pre-wrap; overflow-wrap: anywhere; }
.diff .ctx, .diff .hunk { color: var(--fg-2); }
.diff .hunk { font-style: italic; }
.diff .add { background: var(--add); }
.diff .del { background: var(--del); }
.prose .about { margin: 0 0 8px; font: 400 15px/24px var(--sans); color: var(--fg-2); }
.prose .about + :not(.about) { margin-top: 32px; }

/* ---- Notes and comments: one scene, a tab each (task 1375) ------------- */
/* AKQA's latest news: the notes as rows in two columns, a small line with
   the kind over a title in serif and an arrow that leans in as the pointer
   comes; pills pick a kind. A row opens its note in a sheet from the right,
   over a veil, the note whole: on a screen the page keeps the notes out of
   sight for it, and print shows them. */
.talk .tabs, .talk .kinds { display: flex; flex-wrap: wrap; gap: 8px; margin: 0 0 16px; }
.kinds .pill sup { font-size: 11px; line-height: 1; color: var(--fg-2); }
.kinds .pill[aria-pressed="true"] { background: var(--fg-strong); color: var(--bg); }
.kinds .pill[aria-pressed="true"] sup { color: inherit; opacity: 0.7; }
.scene > :is(.panels, .rows) { max-width: none; }
.panels > #comments { max-width: var(--measure); }
.prose .rows { display: grid; grid-template-columns: repeat(auto-fill, minmax(320px, 1fr)); gap: 0 32px; margin: 24px 0 0; padding: 0; list-style: none; }
.prose .rows li { margin: 0; }
.row { display: grid; grid-template-columns: 1fr auto; align-content: start; gap: 4px 16px; box-sizing: border-box; width: 100%; height: 100%; padding: 20px 0; border: 0; border-top: 1px solid var(--rule); background: none; text-align: left; cursor: pointer; }
.row small { font: 400 12px/16px var(--sans); letter-spacing: 0.04em; text-transform: uppercase; color: var(--fg-2); }
.row.open small { color: var(--accent); }
.row strong { grid-column: 1; font: 400 20px/26px var(--serif); color: var(--fg-strong); }
.row i { grid-column: 2; grid-row: 1 / span 2; align-self: center; font: normal 400 20px/1 var(--sans); color: var(--fg-2); transition: transform 0.45s var(--curve), color 0.3s ease; }
.row:hover i { color: var(--fg-strong); transform: translate(3px, 3px); }
:is(.row, .kinds .pill):focus-visible { outline: 2px solid var(--fg-strong); outline-offset: 3px; }
@media screen {
  .whole { display: none; }
}
dialog.side { position: fixed; inset: 0 0 0 auto; box-sizing: border-box; width: min(560px, 100vw); max-width: none; height: 100%; max-height: none; margin: 0; padding: 0; border: 0; overflow: visible; background: var(--bg); color: var(--fg); box-shadow: -24px 0 64px rgba(0, 0, 0, 0.18); }
dialog.side::backdrop { background: rgba(0, 0, 0, 0.32); }
.side-in { box-sizing: border-box; height: 100%; padding: 80px 40px 120px; overflow-y: auto; overscroll-behavior: contain; }
.side .shut { position: absolute; top: 16px; right: 16px; }
.side .shut:focus-visible { outline: 2px solid var(--fg-strong); outline-offset: 3px; }
.side.prose .note { padding: 0; border: 0; }
.side.prose .note h3 { margin: 12px 0 0; font: 400 30px/36px var(--serif); letter-spacing: -0.015em; }
@media (max-width: 600px) {
  .side-in { padding: 72px 20px 120px; }
}
.foot { box-sizing: border-box; max-width: calc(var(--wide) + 2 * var(--gutter)); margin: 0 auto; padding: 48px var(--gutter) 0; font: 400 13px/20px var(--sans); color: var(--fg-2); }

/* ---- What's next: AKQA's closing scene (task 1377) ------------------- */
/* Dark, the window's height, everything centred: the opener large, the
   next move in serif under it, the page's last moves as pills, the footer
   at the foot. Behind them two glows drift, as AKQA's closing colours do;
   still under reduced motion, and gone in print. */
.scene.close { display: flex; flex-direction: column; justify-content: center; min-height: 100vh; overflow: hidden; text-align: center; }
.scene.close::before { content: ""; position: absolute; inset: -20%; background: radial-gradient(40% 50% at 30% 60%, rgba(255, 140, 60, 0.26), transparent 70%), radial-gradient(35% 45% at 70% 40%, rgba(120, 90, 255, 0.22), transparent 70%); filter: blur(40px); animation: drift 18s ease-in-out infinite alternate; pointer-events: none; }
@keyframes drift { to { transform: translate(6%, -4%) scale(1.08); } }
.scene.close > * { position: relative; margin-right: auto; margin-left: auto; }
.close .opener { font-size: clamp(48px, 8vw, 120px); }
.prose .close .next { max-width: 720px; margin: 0 auto; font: 400 clamp(20px, 1.8vw, 26px)/1.35 var(--serif); color: var(--fg-strong); }
.close .acts { display: flex; flex-wrap: wrap; justify-content: center; gap: 8px; max-width: none; margin-top: 40px; }
.close .acts i { font-style: normal; }
.scene.close > .foot { max-width: 760px; margin-top: 96px; padding: 0; }

/* ---- AKQA's mark at the top centre ------------------------------------- */
/* As AKQA's logo stays: it comes in once the first scene has gone by, in
   the bar's glass, since the text runs under it. */
.mark { position: fixed; top: 16px; left: 50%; z-index: 25; padding: 8px 16px; border-radius: 999px; background-color: rgba(0, 0, 0, var(--bar-black)); background-image: linear-gradient(rgba(255, 255, 255, var(--bar-white)), rgba(255, 255, 255, var(--bar-white))); -webkit-backdrop-filter: blur(var(--bar-blur)); backdrop-filter: blur(var(--bar-blur)); font: 400 13px/16px var(--sans); letter-spacing: 0.08em; text-transform: uppercase; white-space: nowrap; color: var(--bar-ink); opacity: 0; pointer-events: none; transform: translateX(-50%); transition: opacity 0.5s ease; }
.mark.on { opacity: 1; }
.mark b { font-weight: 400; color: var(--bar-ink-muted); }

/* ---- AKQA's reveals (note 1357) ---------------------------------------- */
/* What a scene holds comes in from a 6 px blur and 24 px below, in 800 ms,
   once, as it comes into view; a heading word by word, 35 ms apart. Hidden
   only on a page whose script can bring it back, which THEME marks, on a
   screen, with motion allowed: under reduced motion all of it is still,
   and print is a plain document. */
@media screen and (prefers-reduced-motion: no-preference) {
  :root[data-motion] .scene > :not(.words, .held) { transition: opacity 0.8s var(--curve), filter 0.8s var(--curve), transform 0.8s var(--curve); }
  :root[data-motion] .scene > :not(.words, .held, .in) { opacity: 0; filter: blur(6px); transform: translateY(24px); }
  :root[data-motion] .words .w { display: inline-block; transition: opacity 0.45s var(--curve) calc(var(--i, 0) * 35ms), filter 0.45s var(--curve) calc(var(--i, 0) * 35ms); }
  :root[data-motion] .words:not(.in) .w { opacity: 0; filter: blur(8px); }
  /* The Goal's words wait dim for the scroll to light them, and the rest
     of the Goal for the statement to be lit. */
  :root[data-motion] .goal .statement :is(.w, code) { color: var(--dim); transition: color 0.25s ease; }
  :root[data-motion] .goal .statement :is(.w, code).lit { color: var(--fg-strong); }
  :root[data-motion] .goal-rest { transition: opacity 0.8s var(--curve), transform 0.8s var(--curve); }
  :root[data-motion] .goal-rest:not(.in) { opacity: 0; transform: translateY(24px); }
  /* And the Goal is as tall as that takes: the window's height and a
     thirty-sixth of it more per word, the --tall its script sets. In lvh:
     a phone's URL bar, coming and going, resizes the window but not lvh,
     so the words stay under the reader's thumb. Without --tall the height
     is invalid at computed-value time, so auto. */
  :root[data-motion] .scene.goal { height: calc(var(--tall) * 100lvh); }
}

/* ---- Medium's section bars, at the right edge -------------------------- */
.toc { position: fixed; top: 50%; right: 16px; z-index: 20; transform: translateY(-50%); }
.toc-bars { display: flex; flex-direction: column; align-items: center; gap: 8px; width: 24px; padding: 12px 0; border: 0; border-radius: 999px; background: var(--bars-bg); cursor: pointer; transition: opacity 0.18s, visibility 0.18s; }
.toc-bars span { width: 16px; height: 2px; border-radius: 1px; background: var(--fg); opacity: 0.15; transition: opacity 0.15s, background-color 0.15s; }
.toc-bars span.on { opacity: 1; }
.toc-card { position: absolute; top: 50%; right: 0; box-sizing: border-box; width: 300px; padding: 12px 40px 12px 16px; border-radius: 8px; background: var(--card); box-shadow: var(--card-shadow); opacity: 0; visibility: hidden; transform: translateY(-50%); transition: opacity 0.18s, visibility 0.18s; }
.toc:hover .toc-card, .toc:focus-within .toc-card { opacity: 1; visibility: visible; }
.toc:hover .toc-bars, .toc:focus-within .toc-bars { opacity: 0; visibility: hidden; }
.toc ol { margin: 0; padding: 0; list-style: none; }
.toc li { padding: 2px 0; }
.toc a { display: flex; align-items: center; gap: 8px; font: 400 14px/20px var(--sans); color: var(--fg-2); text-decoration: none; white-space: nowrap; }
.toc a .t { overflow: hidden; text-overflow: ellipsis; }
.toc a .d { flex: none; width: 4px; height: 4px; border-radius: 50%; }
.toc a:hover { color: var(--fg); }
.toc a.on { color: var(--fg-strong); }
.toc a.on .d { background: var(--fg-strong); }

/* ---- AKQA's bar: a pill at the bottom, a panel once clicked ------------ */
/* As measured on akqa.com (task 1223): the bar fixed 60px above the
   window's bottom and centred; in it the surface, whose width and radius
   the page's script moves by AKQA's springs and whose height is its
   content's, so it grows upward as its folds open: the list above the
   field, then a hairline, the field, the nav below. Its glass and ink are
   the theme's, and change with it as AKQA's do, in 0.3 s. */
@property --bar-gleam { syntax: "<angle>"; inherits: false; initial-value: 0deg; }
.bar { position: fixed; bottom: 60px; left: 50%; z-index: 30; transform: translateX(-50%); }
.bar-surface { position: relative; box-sizing: border-box; width: 320px; overflow: hidden; border-radius: 100px; background-color: rgba(0, 0, 0, var(--bar-black)); background-image: linear-gradient(rgba(255, 255, 255, var(--bar-white)), rgba(255, 255, 255, var(--bar-white))); box-shadow: 0 8px 40px 0 rgba(0, 0, 0, 0.05); color: var(--bar-ink); -webkit-backdrop-filter: blur(var(--bar-blur)); backdrop-filter: blur(var(--bar-blur)); }
.bar-surface, .bar-surface * { transition: background-color 0.3s, background-image 0.3s, color 0.3s, border-color 0.3s, box-shadow 0.3s, -webkit-backdrop-filter 0.3s, backdrop-filter 0.3s; }
/* The gleam: a ring of the surface's edge, faint but for a highlight going
   round it every 3 s; with the focus in the bar, a still line instead. */
.bar-surface::before { content: ""; position: absolute; inset: 0; z-index: 2; box-sizing: border-box; padding: 1px; border-radius: inherit; background: conic-gradient(from var(--bar-gleam), var(--bar-rim) 0deg, var(--bar-rim) 116deg, var(--bar-halo) 160deg, var(--bar-core) 180deg, var(--bar-halo) 200deg, var(--bar-rim) 244deg, var(--bar-rim) 360deg); opacity: 0.82; pointer-events: none; -webkit-mask: linear-gradient(#000 0 0) content-box, linear-gradient(#000 0 0); -webkit-mask-composite: xor; mask: linear-gradient(#000 0 0) content-box exclude, linear-gradient(#000 0 0); animation: bar-gleam 3s linear infinite; }
@keyframes bar-gleam { to { --bar-gleam: 360deg; } }
.bar-surface:focus-within { outline: 1px solid var(--bar-line); outline-offset: -1px; }
.bar-surface:focus-within::before { content: none; }
/* What the panel adds, folded to nothing in the pill. Each fold's content
   lays out at the panel's width (the script sets it), so the fold is
   measured at the height it ends at from the first frame, and the surface
   uncovers it as it widens. */
.bar-fold { height: 0; overflow: hidden; opacity: 0; }
.fold-in { display: flow-root; }
.bar-hairline { height: 0; overflow: hidden; opacity: 0; background: var(--bar-line); pointer-events: none; }
.bar-list { max-height: min(327px, calc(100dvh - 140px)); overflow-y: auto; overscroll-behavior: contain; scrollbar-width: thin; }
.bar-list:empty { display: none; }
.bar-list .item { display: flex; align-items: center; gap: 8px; box-sizing: border-box; width: 100%; padding: 8px 16px; border: 1px solid transparent; background: none; color: var(--bar-ink); text-align: left; cursor: pointer; }
.bar-list .item.in { animation: bar-item-in 0.35s cubic-bezier(0.2, 0.65, 0.3, 1) both; }
@keyframes bar-item-in { from { opacity: 0; filter: blur(6px); } }
.bar-list .item:hover, .bar-list .item.sel { background: var(--bar-fill); }
.bar-list .badge { display: grid; flex: none; place-items: center; width: 40px; height: 40px; border-radius: 2.667px; font: 500 14px/1 var(--sans); color: #fff; }
.bar-list .what { display: flex; flex: 1; flex-direction: column; gap: 2px; min-width: 0; }
.bar-list .kind { overflow: hidden; font: 400 14px/21px var(--sans); color: var(--bar-ink-muted); text-overflow: ellipsis; white-space: nowrap; }
.bar-list .label { overflow: hidden; font: 400 14px/19.25px var(--sans); text-overflow: ellipsis; white-space: nowrap; }
/* The field's row: 56px in the pill, 60px in the panel, as AKQA's. */
.bar-line { position: relative; display: flex; align-items: center; gap: 12px; box-sizing: border-box; min-height: 56px; padding: 16px 40px 16px 16px; font: 400 14px/21px var(--sans); }
.bar.open .bar-line { padding: 18px 56px 18px 16px; }
.bar-field { position: relative; display: flex; flex: 1; align-items: center; min-width: 0; min-height: 24px; }
.bar input { position: relative; z-index: 1; display: block; box-sizing: border-box; width: 100%; min-width: 0; height: 24px; padding: 0; border: 0; outline: 0; background: transparent; font: 400 16px/24px var(--sans); color: var(--bar-ink-strong); caret-color: var(--bar-ink-strong); }
.bar input::selection, .bar-note::selection { background: rgba(0, 0, 0, 0.16); }
/* The hint: the suggestion showing, word by word, each coming in from a
   blur and leaving into one (the script staggers them). */
.bar-hint { position: absolute; inset: 0; display: flex; align-items: center; overflow: hidden; color: var(--bar-ink-idle); white-space: nowrap; pointer-events: none; }
.bar-hint span { display: inline-block; transition: opacity 0.4s cubic-bezier(0.2, 0.65, 0.3, 1), filter 0.4s cubic-bezier(0.2, 0.65, 0.3, 1); }
.bar-hint span.out { opacity: 0; filter: blur(8px); transition-duration: 0.3s; transition-timing-function: cubic-bezier(0.25, 0.1, 0.35, 1); }
/* A caret that breathes before the hint while the pill waits. */
.bar-cursor { position: absolute; top: 50%; left: 0; width: 2px; height: 16px; border-radius: 1px; background: var(--bar-ink-strong); opacity: 0.18; pointer-events: none; transform: translate(-3px, -50%); transition: opacity 0.2s ease-out; animation: bar-cursor 1.6s ease-in-out infinite; }
@keyframes bar-cursor { 50% { opacity: 0.55; } }
.bar-surface:focus-within .bar-cursor { opacity: 0; animation: none; }
.bar.typed .bar-hint, .bar.typed .bar-cursor, .bar.quoting .bar-hint, .bar.quoting .bar-cursor { display: none; }
/* The menu: two lines a spring crosses while the bar is a panel. */
.bar-menu { position: absolute; top: 50%; right: 16px; z-index: 1; display: grid; place-items: center; width: 24px; height: 24px; padding: 0; border: 0; border-radius: 50%; background: none; color: var(--bar-ink); cursor: pointer; transform: translateY(-50%); transition: opacity 0.15s, color 0.3s; }
.bar-menu:hover { opacity: 0.8; }
.bar-menu:active { transform: translateY(-50%) scale(0.97); }
.bar-menu span { position: absolute; top: 50%; left: 4px; width: 16px; height: 1px; margin-top: -0.5px; border-radius: 1px; background: currentColor; }
.bar-menu span:first-child { transform: translateY(-3.5px); }
.bar-menu span:last-child { transform: translateY(3.5px); }
.bar-nav { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; padding: 16px; }
.bar-nav a { display: inline-flex; flex: none; align-items: center; padding: 6px 14px; border: 1px solid transparent; border-radius: 999px; background: var(--bar-fill); font: 400 13px/19.5px var(--sans); letter-spacing: -0.025em; color: var(--bar-ink-nav); text-decoration: none; }
.bar-nav a:hover, .bar-nav a[aria-current] { background: var(--bar-fill-on); }
/* The commands (task 1337): in the menu, chips drawn as lines below the
   parts' filled ones; in the list, a badge of the bar's own fill. */
.bar-acts { display: flex; flex-wrap: wrap; gap: 8px; padding: 0 16px 16px; }
.bar-acts:empty { display: none; }
.bar-act { display: inline-flex; flex: none; align-items: center; padding: 5px 13px; border: 1px solid var(--bar-line); border-radius: 999px; background: none; font: 400 13px/19.5px var(--sans); letter-spacing: -0.025em; color: var(--bar-ink-nav); cursor: pointer; }
.bar-act:hover { background: var(--bar-fill); }
.bar-list .item.command .badge { background: var(--bar-fill-on); font-size: 20px; color: var(--bar-ink); }
/* Review, a round button beside the pill of its glass, size and ring, the
   person's pending comments counted on its edge. It opens the panel, and
   steps into it while the panel is open. */
.bar-side { position: absolute; bottom: 0; left: calc(100% + 12px); display: grid; place-items: center; box-sizing: border-box; width: 56px; height: 56px; padding: 0; border: 0; border-radius: 50%; background-color: rgba(0, 0, 0, var(--bar-black)); background-image: linear-gradient(rgba(255, 255, 255, var(--bar-white)), rgba(255, 255, 255, var(--bar-white))); box-shadow: 0 8px 40px 0 rgba(0, 0, 0, 0.05); color: var(--bar-ink); -webkit-backdrop-filter: blur(var(--bar-blur)); backdrop-filter: blur(var(--bar-blur)); cursor: pointer; transition: opacity 0.25s, transform 0.45s cubic-bezier(0.2, 0.65, 0.3, 1), background-color 0.3s, background-image 0.3s, color 0.3s; }
.bar-side::before { content: ""; position: absolute; inset: 0; box-sizing: border-box; padding: 1px; border-radius: inherit; background: var(--bar-rim); opacity: 0.82; pointer-events: none; -webkit-mask: linear-gradient(#000 0 0) content-box, linear-gradient(#000 0 0); -webkit-mask-composite: xor; mask: linear-gradient(#000 0 0) content-box exclude, linear-gradient(#000 0 0); }
.bar-side:hover { background-color: rgba(0, 0, 0, calc(var(--bar-black) + 0.04)); }
.bar-side:active { transform: scale(0.97); }
.bar-side:focus-visible { outline: 1px solid var(--bar-ink-muted); outline-offset: 2px; }
.bar-side svg { width: 24px; height: 24px; fill: none; stroke: currentColor; stroke-width: 1.5; stroke-linecap: round; stroke-linejoin: round; }
.bar-side .count { position: absolute; top: -3px; right: -3px; box-sizing: border-box; min-width: 20px; height: 20px; padding: 0 6px; border-radius: 999px; background: var(--bar-ink); font: 600 12px/20px var(--sans); color: var(--page); text-align: center; }
.bar-side .count:empty { display: none; }
.bar.open .bar-side { opacity: 0; transform: translateX(-16px) scale(0.86); pointer-events: none; }

/* Comments (task 1213, decision 1214), an ebook's notes: the words tinted
   in their theme, darker where comments of one theme stack and banded where
   themes meet, a pin where comments end, and the comments at a point in a
   popover beside their words. A selection offers Comment and the themes;
   the comment is written in the pill, below the words it quotes and the
   themes. */
.prose mark.c { padding: 0; border-radius: 0; background-color: var(--tint, var(--tint-yellow)); color: inherit; cursor: pointer; -webkit-box-decoration-break: clone; box-decoration-break: clone; }
.prose mark.c[data-stack="2"] { background-image: linear-gradient(var(--tint), var(--tint)); }
.prose mark.c[data-stack="3"] { background-image: linear-gradient(var(--tint), var(--tint)), linear-gradient(var(--tint), var(--tint)); }
.prose mark.c.off { background: none !important; box-shadow: none; cursor: auto; }
/* After .off: a comment opened from the list underlines its words even
   with the highlights off. */
.prose mark.c.lit { box-shadow: inset 0 -0.16em 0 var(--lit); }
.prose .pin { display: inline-block; box-sizing: border-box; width: 9px; height: 9px; margin: 0 2px 0 1px; padding: 0; border: 0; border-radius: 50%; vertical-align: 0.7em; font: 700 10px/15px var(--sans); color: var(--page); text-align: center; cursor: pointer; }
.prose .pin[data-n] { width: auto; min-width: 15px; height: 15px; padding: 0 4px; border-radius: 8px; vertical-align: 0.45em; }
.prose .pin[data-n]::after { content: attr(data-n); }
.prose .pin.off { display: none; }
.prose .pin:focus-visible { outline: 2px solid var(--fg-strong); outline-offset: 2px; }
::highlight(ekko-quoted-yellow) { background-color: var(--tint-yellow); }
::highlight(ekko-quoted-blue) { background-color: var(--tint-blue); }
::highlight(ekko-quoted-pink) { background-color: var(--tint-pink); }
::highlight(ekko-quoted-green) { background-color: var(--tint-green); }
::highlight(ekko-quoted-purple) { background-color: var(--tint-purple); }
::highlight(ekko-quoted-orange) { background-color: var(--tint-orange); }
.pop { position: absolute; z-index: 40; box-sizing: border-box; max-height: min(60vh, 520px); overflow-y: auto; padding: 6px; border-radius: 12px; background: var(--card); box-shadow: var(--card-shadow), 0 0 0 1px var(--rule); font: 400 14px/20px var(--sans); color: var(--fg); outline: none; overscroll-behavior: contain; }
.pop-item { position: relative; padding: 10px 12px 10px 18px; border-radius: 8px; }
.pop-item::before { content: ""; position: absolute; top: 12px; bottom: 12px; left: 6px; width: 3px; border-radius: 2px; background: var(--ink); }
.pop-item + .pop-item { margin-top: 2px; }
.pop-count { position: sticky; top: 0; z-index: 1; margin: -6px -6px 0; padding: 10px 18px 6px; background: var(--card); font: 500 12px/18px var(--sans); color: var(--fg-2); }
.pop-head .same-words { font-style: italic; }
.pop-item:hover, .pop-item:focus-within { background: var(--pop-hover); }
.pop-head, .entry-head { display: flex; flex-wrap: wrap; align-items: center; gap: 4px 8px; font: 400 12px/18px var(--sans); color: var(--fg-2); }
.pop-head .theme, .entry-head .theme { font-weight: 600; color: var(--fg-strong); }
.pop-head .state, .entry-head .state { padding: 0 7px; border-radius: 999px; background: var(--pop-chip); }
.pop-head .changed, .entry-head .changed, .entry-head .gone { color: #b54708; }
.pop .said { display: -webkit-box; overflow: hidden; margin: 6px 0 0; font: italic 400 14px/20px var(--serif); color: var(--fg-2); -webkit-line-clamp: 2; -webkit-box-orient: vertical; }
.pop .said.was, .prose .entry.outdated .said { text-decoration: line-through; }
.pop .text, .prose .entry .text { margin: 6px 0 0; white-space: pre-wrap; overflow-wrap: anywhere; color: var(--fg); }
.pop .meta { margin: 6px 0 0; font-size: 12px; color: var(--fg-2); }
.pop-reply { margin: 8px 0 0; padding: 0 0 0 10px; border-left: 2px solid var(--rule); }
.pop-foot { display: flex; flex-wrap: wrap; align-items: center; gap: 6px 8px; margin: 6px 0 0; }
.pop-foot .meta { margin: 0; }
.pop-actions { display: flex; flex-wrap: wrap; align-items: center; gap: 6px; margin: 0 0 0 auto; }
.pop-actions button { padding: 3px 10px; border: 0; border-radius: 999px; background: var(--bg); box-shadow: inset 0 0 0 1px var(--rule); font: 500 12px/18px var(--sans); color: var(--fg); cursor: pointer; }
.pop-actions .danger { color: #d92d20; }
.pop-actions .ask { font-size: 12px; color: var(--fg); }
.pop-actions .why { flex-basis: 100%; font-size: 12px; color: #d92d20; }
.select-tools { position: absolute; z-index: 30; display: flex; align-items: center; gap: 2px; padding: 4px; border-radius: 999px; background: var(--fg); box-shadow: 0 4px 16px rgba(0, 0, 0, 0.18); }
.select-tools .take { padding: 6px 12px; border: 0; border-radius: 999px; background: none; font: 500 13px/18px var(--sans); color: var(--bg); cursor: pointer; }
.select-tools .take:hover { background: rgba(127, 127, 127, 0.35); }
.select-tools .take .label { display: inline-block; min-width: 4.6em; }
.select-tools kbd { margin: 0 0 0 8px; padding: 0 5px; border-radius: 4px; background: rgba(127, 127, 127, 0.35); font: 500 11px/16px var(--sans); }
.select-tools .swatches { display: flex; gap: 6px; padding: 0 10px 0 6px; }
.select-tools .swatch { width: 16px; height: 16px; padding: 0; border: 2px solid var(--fg); border-radius: 50%; background: var(--ink); box-shadow: 0 0 0 1px rgba(127, 127, 127, 0.6); cursor: pointer; }
.select-tools .swatch:hover, .select-tools .swatch:focus-visible { transform: scale(1.25); }
.bar-quote { display: flex; align-items: flex-start; gap: 8px; margin: 12px 12px 8px; padding: 8px 10px; border-left: 3px solid var(--ink, var(--bar-ink-muted)); border-radius: 4px; background: var(--bar-fill); font: 400 13px/18px var(--sans); color: var(--bar-ink-muted); }
.bar-quote-text { flex: 1; display: -webkit-box; overflow: hidden; -webkit-line-clamp: 3; -webkit-box-orient: vertical; }
.bar-quote-drop { flex: none; padding: 0 4px; border: 0; background: none; font: 400 16px/18px var(--sans); color: var(--bar-ink-muted); cursor: pointer; }
.bar-suggest { flex: none; height: 22px; padding: 0 10px; border: 0; border-radius: 999px; background: var(--bar-fill); font: 500 12px/22px var(--sans); color: var(--bar-ink-muted); cursor: pointer; }
.bar-suggest[aria-pressed="true"] { background: var(--bar-fill-on); color: var(--bar-ink); box-shadow: inset 0 0 0 1.5px var(--ink, var(--bar-ink)); }
.bar.suggesting .bar-quote-text { text-decoration: line-through; }
.bar.reviewing .bar-suggest { display: none; }
.bar-themes { display: flex; flex-wrap: wrap; gap: 6px; margin: 0 12px 10px; }
.bar-themes:empty { display: none; }
.bar-theme, .bar-theme-name { box-sizing: border-box; height: 26px; padding: 0 10px 0 24px; border: 0; border-radius: 999px; background: var(--bar-fill) radial-gradient(circle at 13px 50%, var(--ink) 0 4px, transparent 5px) no-repeat; font: 400 12px/26px var(--sans); color: var(--bar-ink-muted); cursor: pointer; }
.bar-theme[aria-checked="true"] { background-color: var(--bar-fill-on); color: var(--bar-ink); box-shadow: inset 0 0 0 1.5px var(--ink); }
.bar-theme-name { width: 104px; outline: 0; color: var(--bar-ink); cursor: text; box-shadow: inset 0 0 0 1px var(--bar-line); }
.bar-theme-name:focus { box-shadow: inset 0 0 0 1.5px var(--ink); }
.bar-theme-edit { height: 26px; padding: 0 8px; border: 0; background: none; font: 400 12px/26px var(--sans); color: var(--bar-ink-muted); text-decoration: underline; text-underline-offset: 3px; cursor: pointer; }
.bar-verdict { box-sizing: border-box; height: 26px; padding: 0 12px; border: 0; border-radius: 999px; background: var(--bar-fill); font: 400 12px/26px var(--sans); color: var(--bar-ink-muted); cursor: pointer; }
.bar-verdict[aria-checked="true"] { background: var(--bar-fill-on); color: var(--bar-ink); box-shadow: inset 0 0 0 1.5px var(--bar-ink); }
.bar-verdict:disabled { opacity: 0.4; cursor: default; }
.bar-tell { margin: 0 16px 8px; font: 400 12px/16px var(--sans); color: var(--bar-ink-muted); }
.bar-tell:empty { display: none; }
.bar-tell p { margin: 0; }
.bar-tell ul { max-height: 112px; margin: 4px 0 0; padding: 0 0 0 16px; overflow: auto; }
.bar-tell .confirm { margin-top: 6px; font-weight: 600; color: var(--bar-ink); }
.bar-why { margin: 0 16px 8px; font: 400 12px/16px var(--sans); color: #d92d20; }
.bar-why:empty { display: none; }
.bar input::placeholder, .bar-note::placeholder { color: var(--bar-ink-idle); }
.bar-note { display: block; box-sizing: border-box; width: 100%; max-height: 160px; padding: 16px 0; border: 0; outline: 0; background: transparent; resize: none; font: 400 16px/24px var(--sans); color: var(--bar-ink-strong); caret-color: var(--bar-ink-strong); }
.bar.quoting .bar-line { padding: 0 56px 0 16px; }
.bar-send { position: absolute; right: 12px; bottom: 12px; display: grid; place-items: center; width: 32px; height: 32px; padding: 0 0 2px; border: 0; border-radius: 50%; background: var(--bar-ink); font: 600 17px/1 var(--sans); color: var(--page); cursor: pointer; }
.bar-send:disabled { opacity: 0.3; cursor: default; }
#comments .filters { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; margin: 32px 0 0; }
#comments .filter { display: inline-flex; align-items: center; gap: 6px; height: 30px; padding: 0 12px; border: 0; border-radius: 999px; background: none; box-shadow: inset 0 0 0 1px var(--rule); font: 400 13px/30px var(--sans); color: var(--fg); cursor: pointer; }
#comments .filter .dot { width: 8px; height: 8px; border-radius: 50%; background: var(--ink); }
#comments .filter .count { color: var(--fg-2); }
#comments .filter[aria-pressed="true"] { background: var(--fg); color: var(--bg); box-shadow: none; }
#comments .filter[aria-pressed="true"] .count { color: inherit; opacity: 0.7; }
#comments .filter.switch { margin-left: auto; }
.prose ol.comments { margin: 24px 0 0; padding: 0; list-style: none; border-top: 1px solid var(--rule); }
.prose li.entry { position: relative; margin: 0; padding: 16px 0 16px 18px; border-bottom: 1px solid var(--rule); font: 400 14px/20px var(--sans); letter-spacing: normal; }
.prose li.entry::before { content: ""; position: absolute; top: 18px; bottom: 18px; left: 0; width: 3px; border-radius: 2px; background: var(--ink, var(--ink-yellow)); }
.entry-head { width: 100%; padding: 0; border: 0; background: none; text-align: left; cursor: pointer; }
.prose .entry .said { display: -webkit-box; overflow: hidden; margin: 8px 0 0; padding: 0 0 0 12px; border-left: 2px solid var(--rule); font: italic 400 16px/24px var(--serif); color: var(--fg-2); -webkit-line-clamp: 3; -webkit-box-orient: vertical; }
.suggests { margin: 8px 0 0; font: 400 15px/22px var(--serif); overflow-wrap: anywhere; }
.suggests del { color: var(--fg-2); text-decoration: line-through; }
.suggests ins { margin: 0 0 0 6px; padding: 0 3px; border-radius: 3px; background: color-mix(in srgb, var(--ink-green) 18%, transparent); color: var(--fg); text-decoration: none; }
.pop .text.told, .prose .entry .text.told { display: none; }
.prose .entry .text { font: 400 16px/24px var(--sans); }
.prose .entry .reply { margin: 12px 0 0; padding: 0 0 0 12px; border-left: 2px solid var(--rule); }
.prose .entry .reply .meta { font-size: 12px; color: var(--fg-2); }
.prose li.entry.resolved { opacity: 0.6; }

/* The column at the left needs 232px beside the text, and the window
   1176px for that; a narrower one puts it above the text. */
@media (max-width: 720px) {
  .toc { display: none; }
}
/* Still: the page takes a scene's mode at once, and nothing breathes. */
@media (prefers-reduced-motion: reduce) {
  :root, .mark, .fact b, .fact span, .pill, .bar-hint span, .toc-card, .toc-bars, .toc-bars span, .bar-side, .prose .card, .card .more, .round, .prose .point, .ticks button::before, .prose .sheet, .toggle i, .toggle i::after, .seg::before, .row i, .line button::before, .map .node, .map .edge { transition: none; }
  .numeral .out, .numeral .enter, .scene.close::before { animation: none; }
  .goal .held { position: static; min-height: 0; }
  .bar-surface::before, .bar-cursor, .bar-list .item.in, .kicker .state.waiting::before { animation: none; }
  /* Stopped, the gleam would hold its highlight on the pill's bottom edge:
     the ring is the even rim of the round button beside it (task 1349). */
  .bar-surface::before { background: var(--bar-rim); }
}
/* A plain document: black on white whatever the theme or the scene, each
   scene as tall as what it holds, and nothing the page moves. What a scene
   hides on a screen shows, a tab not picked or a step off the stage: as
   important as [hidden], and more particular (task 1389). */
@media print {
  :root, :root[data-theme], :root[data-mode], :root[data-theme][data-mode] { --page: #fff; --bg: #fff; --fg: #191919; --fg-strong: #000; --fg-2: #555; --fg-3: #8a8a8a; --rule: #e9e9e9; --chip: #f5f5f5; --raise: #f5f5f5; --card: #fff; --node: #fff; --node-line: #e2e2e2; --accent: #1a8917; color-scheme: light; transition: none; }
  .toc, .bar, .mark, .arrows, .select-tools, .pop, .prose .pin, #comments .filters, .callout .writes-only { display: none; }
  .scene { padding: 24px 0; }
  .hero, .goal .held { min-height: 0; }
  .goal .held { position: static; padding: 0; }
  .goal .progress { display: none; }
  .known .tabs, .known .controls, .card .num, .card .more { display: none; }
  .known .group[hidden] { display: block !important; }
  .prose .track { display: block; margin: 0 0 16px; padding: 0 0 0 20px; overflow: visible; list-style: disc; transform: none !important; }
  .prose .card { display: list-item; width: auto; min-height: 0; margin: 0 0 8px; padding: 0; border-radius: 0; background: none; }
  .card .body { display: block; margin: 0; overflow: visible; -webkit-line-clamp: none; }
  .card .lead { display: inline; margin: 0; font: inherit; color: inherit; }
  .hold { display: none; }
  .story { display: block; }
  .prose .points { padding: 0 0 0 20px; list-style: revert; }
  .prose .point { padding: 0 0 12px; opacity: 1 !important; }
  .risks .controls, .sheet .num span:first-child { display: none; }
  .prose .stack { display: block; padding: 0 0 0 20px; list-style: revert; }
  .prose .sheet { min-height: 0; margin: 0 0 12px; padding: 0; border-radius: 0; background: none !important; box-shadow: none !important; transform: none !important; opacity: 1 !important; }
  .sheet.ask :is(.num, .body, .lead) { color: inherit; }
  .sheet .body { margin: 0; }
  .sheet .lead { display: inline; margin: 0; font: inherit; color: inherit; }
  .staged .segs, .staged .controls, .step .after { display: none; }
  .staged .steps .step[hidden] { display: block !important; }
  .talk .tabs, .talk .kinds, .talk .rows, dialog.side { display: none; }
  .talk .panel[hidden] { display: block !important; }
  .scrub, .player { display: none; }
  .changed .version[hidden] { display: block !important; }
  .scene.close { min-height: 0; text-align: left; }
  .scene.close::before, .close .acts { display: none; }
  .scene.close > * { margin-left: 0; }
  .panels > .panel::before { content: attr(data-name); display: block; margin: 24px 0 8px; font: 600 18px/24px var(--sans); color: var(--fg-strong); }
  .prose mark.c { background: none !important; text-decoration: underline; }
  .step .more { display: block; }
}
"##;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::Artifact;
    use std::collections::BTreeMap;

    fn spec(key: &str, text: Option<&str>, after: &[&str]) -> StepSpec {
        StepSpec { key: key.to_string(), text: text.map(str::to_string), done_when: None, after: after.iter().map(|key| key.to_string()).collect() }
    }

    fn plan(title: &str) -> String {
        format!("{title}\n\n{}\n", HEADINGS.iter().map(|heading| format!("{heading}\nText.\n")).collect::<Vec<_>>().join("\n"))
    }

    #[test]
    fn a_plan_holds_its_title_then_every_heading_in_order() {
        assert_eq!(unplanned(&plan("Ship the page")), None);
        assert!(unplanned(&plan("# Ship the page")).unwrap().contains("title"));
        assert!(unplanned("\n## Goal").unwrap().contains("title"));
        let out_of_order = "Ship\n## What is known\n## Goal\n## Design\n## Risks and open questions\n";
        assert!(unplanned(out_of_order).unwrap().contains("## What is known is missing or out of order"));
        let missing = plan("Ship").replace("## Design\n", "");
        assert!(unplanned(&missing).unwrap().contains("## Design is missing"));
        assert!(unplanned(&plan("Ship").replace("## Goal", "## Goal:")).is_some(), "a heading is a line of its own");
    }

    #[test]
    fn steps_are_keyed_once_and_wait_on_earlier_steps_only() {
        let made = steps(&[], &[spec("a", Some("First\nwhy"), &[]), spec("b", Some("Second"), &["a"])]).unwrap().0;
        assert_eq!(made.iter().map(|step| (step.key.as_str(), step.after.clone())).collect::<Vec<_>>(), [("a", vec![]), ("b", vec!["a".to_string()])]);
        for (given, refusal) in [
            (vec![spec("a", Some("x"), &["b"]), spec("b", Some("y"), &[])], "not a step before it"),
            (vec![spec("a", Some("x"), &["a"])], "not a step before it"),
            (vec![spec("a", Some("x"), &[]), spec("a", Some("y"), &[])], "two steps are keyed a"),
            (vec![spec("A", Some("x"), &[])], "lower-case"),
            (vec![spec("-a", Some("x"), &[])], "lower-case"),
            (vec![spec("", Some("x"), &[])], "lower-case"),
            (vec![spec("a", None, &[])], "needs its text"),
        ] {
            let error = steps(&[], &given).unwrap_err();
            assert!(error.contains(refusal), "{refusal}: {error}");
        }
        let (cut, told) = steps(&[], &[spec("a", Some(&"long ".repeat(20)), &[])]).unwrap();
        assert_eq!(cut[0].text, format!("{}\n{}", ["long"; 16].join(" "), ["long"; 4].join(" ")), "a title past 80 characters is cut, not refused");
        assert_eq!(told.len(), 1);
        assert!(told[0].starts_with("Step a's first line ran 99 characters"), "{told:?}");
        let many: Vec<StepSpec> = (0..=STEPS_MOST).map(|n| spec(&format!("s{n}"), Some("x"), &[])).collect();
        assert!(steps(&[], &many).unwrap_err().contains("at most"));
    }

    #[test]
    fn an_approved_step_stays_as_it_was() {
        let mut old = steps(&[], &[spec("a", Some("First"), &[]), spec("b", Some("Second"), &[])]).unwrap().0;
        old[0].task = Some("t-a".to_string());
        let kept = steps(&old, &[spec("a", None, &[]), spec("c", Some("Third"), &["a"])]).unwrap().0;
        assert_eq!(kept[0], old[0], "named by key alone, it is kept whole, task and all");
        assert_eq!(kept[1].key, "c");
        assert!(steps(&old, &[spec("a", Some("First"), &[])]).is_ok(), "its own text again changes nothing");
        assert!(steps(&old, &[spec("a", Some("Changed"), &[])]).unwrap_err().contains("step a is approved"));
        assert!(steps(&old, &[spec("z", Some("x"), &[]), spec("a", None, &["z"])]).unwrap_err().contains("step a is approved"));
        let finished = StepSpec { done_when: Some("Shipped".to_string()), ..spec("a", None, &[]) };
        assert!(steps(&old, &[finished]).unwrap_err().contains("step a is approved"));
        assert!(steps(&old, &[spec("b", Some("Second"), &[])]).unwrap_err().contains("keep it among the steps"));

        // A long title was cut when the step was written: the same text
        // again is the same step.
        let long = format!("{}\nwhy", "word ".repeat(20).trim_end());
        let mut old = steps(&[], &[spec("a", Some(&long), &[])]).unwrap().0;
        old[0].task = Some("t-a".to_string());
        let (again, told) = steps(&old, &[spec("a", Some(&long), &[])]).unwrap();
        assert_eq!((again, told.len()), (old, 0), "its own long text again changes nothing");
    }

    fn artifact_item(id: u32, text: &str, steps: Vec<Step>) -> Item {
        let mut item = Item::new_task(id, text.to_string(), vec!["My Board".to_string()], 1);
        item.artifact = Some(Box::new(Artifact { steps, version: 1, earlier: Vec::new(), approved_version: None, unknown: BTreeMap::new() }));
        item
    }

    #[test]
    fn a_write_that_changes_the_plan_raises_its_version_and_keeps_the_text_it_replaced() {
        let made = steps(&[], &[spec("a", Some("First"), &[])]).unwrap().0;
        let before: ItemMap = BTreeMap::from([(1, artifact_item(1, &plan("Ship"), made.clone()))]);
        let version = |data: &ItemMap| data[&1].artifact.as_ref().unwrap().version;

        let mut retexted = before.clone();
        retexted.get_mut(&1).unwrap().description = plan("Ship it");
        keep_versions(&before, &mut retexted, &[1], 7);
        assert_eq!(version(&retexted), 2);
        assert_eq!(retexted[&1].artifact.as_ref().unwrap().earlier.iter().map(|earlier| (earlier.version, earlier.at, earlier.text.clone())).collect::<Vec<_>>(), [(1, 7, plan("Ship"))]);

        let mut restepped = before.clone();
        restepped.get_mut(&1).unwrap().artifact.as_mut().unwrap().steps[0].text = "Other".to_string();
        keep_versions(&before, &mut restepped, &[1], 7);
        assert_eq!(version(&restepped), 2);
        assert!(restepped[&1].artifact.as_ref().unwrap().earlier.is_empty(), "the text did not change");

        let mut approved = before.clone();
        approved.get_mut(&1).unwrap().artifact.as_mut().unwrap().steps[0].task = Some("t".to_string());
        keep_versions(&before, &mut approved, &[1], 7);
        assert_eq!(version(&approved), 1, "a task recorded on a step changes no plan");

        let mut starred = before.clone();
        starred.get_mut(&1).unwrap().is_starred = true;
        keep_versions(&before, &mut starred, &[1], 7);
        assert_eq!(version(&starred), 1);

        let mut many = before.clone();
        for n in 0..EARLIER_KEPT + 3 {
            let was = many.clone();
            many.get_mut(&1).unwrap().description = plan(&format!("Ship {n}"));
            keep_versions(&was, &mut many, &[1], n as i64);
        }
        let earlier = &many[&1].artifact.as_ref().unwrap().earlier;
        assert_eq!(earlier.len(), EARLIER_KEPT);
        assert_eq!(earlier.last().unwrap().version, version(&many) - 1, "the newest are kept");

        let mut fresh = BTreeMap::new();
        let mut new = artifact_item(2, &plan("New"), Vec::new());
        new.artifact.as_mut().unwrap().version = 0;
        fresh.insert(2, new);
        keep_versions(&BTreeMap::new(), &mut fresh, &[2], 7);
        assert_eq!(fresh[&2].artifact.as_ref().unwrap().version, 1);
    }

    #[test]
    fn the_standing_counts_the_steps_tasks_and_what_waits_on_the_user() {
        let mut made = steps(&[], &[spec("a", Some("First"), &[]), spec("b", Some("Second"), &[]), spec("c", Some("Third"), &[])]).unwrap().0;
        let mut all: ItemMap = BTreeMap::new();
        let draft = artifact_item(1, &plan("Ship"), made.clone());
        all.insert(1, draft.clone());
        assert_eq!(Standing::of(&draft, &all).unwrap().words(), "draft, 3 steps");

        let mut done = Item::new_task(2, "First".into(), vec![], 1);
        State::Done.write(&mut done);
        let open = Item::new_task(3, "Second".into(), vec![], 1);
        made[0].task = done.uid.clone();
        made[1].task = open.uid.clone();
        let mut approved = artifact_item(1, &plan("Ship"), made);
        approved.artifact.as_mut().unwrap().approved_version = Some(1);
        all.insert(1, approved.clone());
        all.insert(2, done);
        all.insert(3, open);
        let standing = Standing::of(&approved, &all).unwrap();
        assert_eq!((standing.tasks, standing.done, standing.proposed), (2, 1, 1));
        assert_eq!(standing.words(), "approved, 1 of 2 done, 1 step to approve");

        approved.artifact.as_mut().unwrap().version = 2;
        assert_eq!(Standing::of(&approved, &all).unwrap().words(), "approved, 1 of 2 done, 1 step to approve, changed since approved");

        let mut closed = approved.clone();
        State::Cancelled.write(&mut closed);
        assert_eq!(Standing::of(&closed, &all).unwrap().words(), "cancelled");
        assert!(Standing::of(&all[&2], &all).is_none(), "a task without a plan has no standing");
    }

    /// A step can start from the page when a session may take its task up
    /// now (task 1111), and the refusal says why it cannot.
    #[test]
    fn a_step_can_start_when_its_task_may_be_taken_up_now() {
        let task = |id: u32, state: State| {
            let mut task = Item::new_task(id, format!("Task {id}"), vec![], 1);
            state.write(&mut task);
            task
        };
        let (open, closed) = (task(2, State::Pending), task(3, State::Done));
        let mut all: ItemMap = BTreeMap::from([(2, open.clone()), (3, closed.clone())]);
        let judge = |item: &Item, all: &ItemMap| startable(item, all);
        assert_eq!(judge(&task(1, State::Pending), &all), Ok(()));
        assert_eq!(judge(&task(1, State::Paused), &all), Ok(()));
        assert_eq!(judge(&task(1, State::Progress), &all), Err("task 1 is in progress already".to_string()));
        assert_eq!(judge(&task(1, State::Done), &all), Err("task 1 is done already".to_string()));
        assert_eq!(judge(&task(1, State::Cancelled), &all), Err("task 1 is cancelled already".to_string()));
        assert_eq!(judge(&task(1, State::Waiting), &all), Err("task 1 is waiting on something outside the board".to_string()));
        let mut with = task(1, State::Pending);
        with.with = Some("thiago".into());
        assert_eq!(judge(&with, &all), Err("task 1 is with thiago: theirs to take up, not a session's".to_string()));
        let mut trashed = task(1, State::Pending);
        trashed.trashed = Some(1);
        assert_eq!(judge(&trashed, &all), Err("task 1 is in the trash".to_string()));
        let mut waits = task(1, State::Pending);
        waits.blocked_by = Some(vec![closed.uid.clone().unwrap(), open.uid.clone().unwrap(), "no-such-uid".into()]);
        assert_eq!(judge(&waits, &all), Err("task 1 waits on 2, still open".to_string()), "a closed blocker or one not on the board holds nothing");
        all.get_mut(&2).unwrap().trashed = Some(1);
        assert_eq!(judge(&waits, &all), Ok(()), "nor one in the trash");
    }

    /// A Start is the person's comment on a step saying "Start this" and
    /// nothing else, sent (task 1111): not one pending, a session's, one on
    /// words or the whole plan, or one that says more.
    #[test]
    fn a_start_is_the_persons_sent_start_this_on_a_step() {
        let mut start = Item::new_note(1, START.into(), vec![]);
        start.comment = Some(Box::new(crate::item::Comment {
            version: 1,
            quote: None,
            replacement: None,
            step: Some("one".into()),
            reply_to: None,
            sent: Some(1),
            resolved: None,
            applied: None,
            theme: None,
            color: None,
            unknown: Default::default(),
        }));
        assert!(is_start(&start));
        let varied = |change: &dyn Fn(&mut Item)| {
            let mut note = start.clone();
            change(&mut note);
            is_start(&note)
        };
        assert!(!varied(&|note| note.comment.as_mut().unwrap().sent = None), "pending");
        assert!(!varied(&|note| note.created_by = Some(crate::holder::test_sessions().0.holder(1))), "a session's");
        assert!(!varied(&|note| note.comment.as_mut().unwrap().step = None), "on the whole plan");
        assert!(!varied(&|note| {
            note.comment.as_mut().unwrap().quote = Some(crate::item::Quote { exact: "x".into(), prefix: String::new(), suffix: String::new(), section: String::new(), unknown: Default::default() })
        }), "on words");
        assert!(!varied(&|note| note.description = "Start this, after the review".into()), "it says more");
        assert!(varied(&|note| note.description = format!("{START}\n")), "its words as written, around them aside");
    }

    #[test]
    fn the_page_shows_the_plan_its_steps_and_escapes_what_it_quotes() {
        let made = steps(&[], &[spec("a", Some("First <step>\nwhy"), &[])]).unwrap().0;
        let text = plan("Ship <it>").replace("## Goal\nText.", "## Goal\nA **bold** goal.\n\n<script>alert(1)</script>");
        let item = artifact_item(1, &text, made);
        let all: ItemMap = BTreeMap::from([(1, item.clone())]);
        let (html, version) = page(&item, &all, Some(Path::new("/projects/site")));
        assert!(html.contains("<h1 class=\"words\"><span class=\"w\">Ship</span> <span class=\"w\">&lt;it&gt;</span></h1>") && html.contains("<title>Ship &lt;it&gt; \u{b7} artifact 1</title>"), "{html}");
        assert!(html.contains(" \u{b7} project site<span id=\"who\"></span></p>"), "{html}");
        assert!(html.contains("<p class=\"statement\"><span class=\"w\">A</span> <strong><span class=\"w\">bold</span></strong> <span class=\"w\">goal.</span></p>"), "{html}");
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;") && !html.contains("<script>alert"), "raw HTML in a plan is shown, not run: {html}");
        assert!(html.contains("<span class=\"title\">First &lt;step&gt;</span>") && html.contains("data-state=\"to approve\""), "{html}");
        assert!(html.contains("<p class=\"kicker\"><span>Artifact 1</span><span class=\"state\">Draft</span></p>"), "{html}");
        assert_eq!(version.len(), 16);
        let (again, same) = page(&item, &all, Some(Path::new("/projects/site")));
        assert_eq!((again, same), (html.clone(), version.clone()), "the same board writes the same page");
        let mut changed = item.clone();
        changed.description = changed.description.replace("bold", "bolder");
        assert_ne!(page(&changed, &all, Some(Path::new("/projects/site"))).1, version, "a change shows in the version");
    }

    #[test]
    fn a_plan_links_to_the_web_mail_or_the_page_and_loads_no_image() {
        let html = markdown(concat!(
            "[run](javascript:alert(1)) [shout](JaVaScRiPt:alert(2)) [coded](javascript&#58;alert(3)) <javascript:alert(4)>\n\n",
            "[data](data:text/html,x) [file](file:///etc/passwd) [relative](notes.md) [spaced](< https://example.com/s>)\n\n",
            "[web](https://example.com/a) [plain](http://example.com/b) [upper](HTTPS://example.com/c) [mail](mailto:a@example.com) <b@example.com> [here](#plan)\n\n",
            "![a chart](https://example.com/chart.png) ![](https://example.com/bare.png) ![local](chart.png)\n\n",
            "[![inner](https://example.com/in.png)](https://example.com/out) ![see [x](https://example.com/y)](https://example.com/z)\n",
        ));
        let hrefs: Vec<&str> = html.split("href=\"").skip(1).map(|rest| &rest[..rest.find('"').unwrap_or(rest.len())]).collect();
        assert_eq!(
            hrefs,
            [
                "https://example.com/a",
                "http://example.com/b",
                "HTTPS://example.com/c",
                "mailto:a@example.com",
                "mailto:b@example.com",
                "#plan",
                "https://example.com/chart.png",
                "https://example.com/bare.png",
                "https://example.com/out",
                "https://example.com/z",
            ],
            "{html}"
        );
        assert!(!html.contains("<img"), "no image is loaded: {html}");
        for words in ["run", "shout", "coded", "javascript:alert(4)", "data", "file", "relative", "spaced", "local"] {
            assert!(html.contains(words), "a link refused keeps its words, {words}: {html}");
        }
        assert!(html.contains(">a chart</a>") && html.contains(">https://example.com/bare.png</a>"), "an image is a link named by its words, or by its source: {html}");
        assert!(html.contains("<a href=\"https://example.com/out\">inner</a>"), "an image inside a link is its words: {html}");
        assert!(html.contains("<a href=\"https://example.com/z\">see x</a>"), "a link inside an image is its words: {html}");
        assert_eq!(html.matches("<a ").count(), html.matches("</a>").count(), "every link opened is closed, and no other: {html}");
    }

    #[test]
    fn the_map_puts_each_step_one_column_past_the_latest_it_waits_on() {
        let made = steps(
            &[],
            &[spec("a", Some("A"), &[]), spec("b", Some("B"), &["a"]), spec("c", Some("C"), &["a"]), spec("d", Some("D"), &["c", "b"]), spec("e", Some("E"), &[]), spec("f", Some("F"), &["a", "d"])],
        )
        .unwrap().0;
        let item = artifact_item(1, &plan("Ship"), made);
        let all: ItemMap = BTreeMap::from([(1, item.clone())]);
        let shown = shown(item.artifact.as_deref().unwrap(), &all);
        assert_eq!(layout(&shown), [(0, 0), (1, 0), (1, 1), (2, 0), (0, 1), (3, 0)]);
        let drawn = map(&shown);
        assert_eq!(drawn.matches("<path class=\"edge\"").count(), 6, "one curve per step waited on: {drawn}");
        assert_eq!(drawn.matches("<button class=\"node proposed\"").count(), 6, "{drawn}");
        let (width, height) = NODE;
        let (gap_x, gap_y) = NODE_GAP;
        let size = format!("width:{}px;height:{}px", 4 * width + 3 * gap_x, 2 * height + gap_y);
        assert!(drawn.contains(&size), "four columns, two rows: {drawn}");
        assert!(drawn.contains(&format!("left:{}px;top:{}px", width + gap_x, 0)) && drawn.contains(&format!("left:{}px;top:{}px", 2 * (width + gap_x), (height + gap_y) / 2)), "a column shorter than the tallest is centred on it: {drawn}");
    }

    /// The map says each step's stage, its column counted from 1, and each
    /// curve the steps it joins and the stage of the later, so the page's
    /// script plays it stage by stage (task 1374). The player, with two
    /// stages or more, starts on the whole plan, as a page without its
    /// script shows it.
    #[test]
    fn the_map_plays_stage_by_stage() {
        let made = steps(
            &[],
            &[spec("a", Some("A"), &[]), spec("b", Some("B"), &["a"]), spec("c", Some("C"), &["a"]), spec("d", Some("D"), &["c", "b"]), spec("e", Some("E"), &[]), spec("f", Some("F"), &["a", "d"])],
        )
        .unwrap().0;
        let item = artifact_item(1, &plan("Ship"), made);
        let all: ItemMap = BTreeMap::from([(1, item.clone())]);
        let drawn = map(&shown(item.artifact.as_deref().unwrap(), &all));
        for (key, stage) in [("a", 1), ("b", 2), ("c", 2), ("d", 3), ("e", 1), ("f", 4)] {
            assert!(drawn.contains(&format!("data-step=\"{key}\" data-stage=\"{stage}\" style=")), "{key} at stage {stage}: {drawn}");
        }
        for (from, to, stage) in [("a", "b", 2), ("a", "c", 2), ("c", "d", 3), ("b", "d", 3), ("a", "f", 4), ("d", "f", 4)] {
            assert!(drawn.contains(&format!("<path class=\"edge\" data-from=\"{from}\" data-to=\"{to}\" data-stage=\"{stage}\" pathLength=\"1\" d=\"M")), "{from} to {to}, at stage {stage}: {drawn}");
        }
        let (html, _) = page(&item, &all, None);
        assert!(
            html.contains("<span>an arrow: what a step waits on</span></div><div class=\"player\"><button class=\"round play\" type=\"button\" aria-label=\"Play\"><svg viewBox=\"0 0 24 24\" aria-hidden=\"true\"><path d=\"M8.5 5.8v12.4L18.2 12z\"/></svg></button><input class=\"scrub\" type=\"range\" autocomplete=\"off\" min=\"1\" max=\"4\" step=\"1\" value=\"4\" style=\"--p: 100%\" aria-label=\"How far the plan unfolds\" aria-valuetext=\"Stage 4 of 4\"><p class=\"at\" aria-live=\"polite\">The whole plan, in 4 stages</p></div><div class=\"bleed\">"),
            "the player, on the whole plan, between the legend and the map: {html}"
        );

        // Steps that wait on none are one stage: nothing to play.
        let made = steps(&[], &[spec("a", Some("A"), &[]), spec("b", Some("B"), &[])]).unwrap().0;
        let item = artifact_item(1, &plan("Ship"), made);
        let all: ItemMap = BTreeMap::from([(1, item.clone())]);
        let (html, _) = page(&item, &all, None);
        assert!(html.contains("data-step=\"b\" data-stage=\"1\"") && !html.contains("class=\"player\""), "{html}");
    }

    #[test]
    fn a_plan_is_cut_at_its_headings_but_inside_code() {
        let cut = sections("Before.\n## Goal\nThe goal.\n```\n## not a heading\n```\n## Design\nThe design.\n");
        assert_eq!(
            cut,
            [
                (String::new(), "Before.\n".to_string()),
                ("Goal".to_string(), "The goal.\n```\n## not a heading\n```\n".to_string()),
                ("Design".to_string(), "The design.\n".to_string()),
            ]
        );
        assert_eq!(sections("## Goal\nText.\n").len(), 1, "nothing before the first heading, no section for it");
    }

    #[test]
    fn a_diff_shows_what_changed_with_the_lines_around_it() {
        assert_eq!(diff("a\nb\nc", "a\nx\nc"), [(' ', "a"), ('-', "b"), ('+', "x"), (' ', "c")]);
        assert_eq!(diff("a", "a\nb"), [(' ', "a"), ('+', "b")]);
        assert_eq!(diff("a\nb", "b"), [('-', "a"), (' ', "b")]);
        let shown = diff_html("1\n2\n3\n4\n5\n6\n7\n8", "1\n2\n3\n4\nfive\n6\n7\n8");
        assert!(shown.starts_with("<div class=\"hunk\">\u{2026} 2 lines kept</div><div class=\"ctx\">  3</div>"), "{shown}");
        assert!(shown.contains("<div class=\"del\">- 5</div><div class=\"add\">+ five</div>"), "{shown}");
        assert!(shown.ends_with("<div class=\"ctx\">  7</div><div class=\"hunk\">\u{2026} 1 line kept</div>"), "{shown}");
        assert!(diff_html("same", "same").contains("The text is the same"));
        assert!(diff_html("<b>", "<i>").contains("<div class=\"add\">+ &lt;i&gt;</div>"), "escaped");
    }

    #[test]
    fn a_comment_is_its_words_and_an_entry_under_comments() {
        let text = "Ship\n\n## Design\nThe `comment` field, un**kept**, out*in*side, at[here](https://example.com), s~~trike~~d.\n\n- one\n- two\n";
        assert_eq!(shown_words(text), "Ship Design The comment field, unkept, outinside, athere, striked. one two", "markup glued to words adds no space");
        let item = artifact_item(1, text, Vec::new());
        let on = |id: u32, words: &str, theme: Option<&str>, color: Option<&str>, reply_to: Option<String>| {
            let mut note = Item::new_note(id, format!("Comment {id}"), vec!["My Board".to_string()]);
            note.attached_to = item.uid.clone();
            note.comment = Some(Box::new(crate::item::Comment {
                version: 1,
                quote: Some(crate::item::Quote { exact: words.into(), prefix: String::new(), suffix: String::new(), section: "Design".into(), unknown: BTreeMap::new() }),
                replacement: None,
                step: None,
                reply_to,
                sent: None,
                resolved: None,
                applied: None,
                theme: theme.map(str::to_string),
                color: color.map(str::to_string),
                unknown: BTreeMap::new(),
            }));
            note
        };
        let plain = on(2, "comment field", None, None, None);
        let themed = on(3, "unkept", Some("Dúvida"), Some("blue"), None);
        let reply = on(4, "unkept", None, None, themed.uid.clone());
        let mut note = Item::new_note(5, "A plain note".to_string(), vec!["My Board".to_string()]);
        note.attached_to = item.uid.clone();
        let all = BTreeMap::from([(1, item.clone()), (2, plain), (3, themed), (4, reply), (5, note)]);
        let (html, _) = page(&item, &all, None);
        let data = |id: &str| html.split(&format!("<script type=\"application/json\" id=\"{id}\">")).nth(1).and_then(|rest| rest.split("</script>").next()).unwrap_or_default().to_string();
        let comments: serde_json::Value = serde_json::from_str(&data("comments-data")).unwrap();
        assert_eq!(comments.as_array().map(Vec::len), Some(3), "{comments}");
        assert_eq!(comments[1]["comment"]["color"], "blue");
        assert_eq!(comments[1]["mine"], true, "written by the person, no process");
        let themes: serde_json::Value = serde_json::from_str(&data("themes")).unwrap();
        assert_eq!(themes, serde_json::json!(THEMES.map(|(color, name)| [color, name])), "the themes go out even before the first comment");
        assert!(html.contains("id=\"note-5\"") && !html.contains("id=\"note-2\"") && !html.contains("id=\"note-3\""), "comments are no Notes: {html}");
        let entry = |id: u32| html.split(&format!("<li class=\"entry\" id=\"comment-{id}\"")).nth(1).and_then(|rest| rest.split("</li>").next()).map(str::to_string);
        let plain = entry(2).expect("the plain comment's entry");
        assert!(plain.contains("data-color=\"yellow\"") && plain.contains("<span class=\"theme\">Note</span>") && plain.contains("comment field"), "{plain}");
        let themed = entry(3).expect("the themed comment's entry");
        assert!(themed.contains("data-color=\"blue\"") && themed.contains("<span class=\"theme\">Dúvida</span>") && themed.contains("role=\"comment\""), "{themed}");
        assert!(themed.contains("<div class=\"reply\" id=\"comment-4\" role=\"comment\">"), "the reply is under what it answers: {themed}");
        assert!(entry(4).is_none(), "a reply is no entry of its own");
        assert!(html.contains("<li><a href=\"#notes-and-comments\"><span class=\"d\"></span><span class=\"t\">Notes and comments</span></a></li>"), "the index leads to their scene: {html}");
        assert!(html.contains("<a href=\"#notes\">Notes</a><a href=\"#comments\">Comments</a>"), "the pill's menu to their tab: {html}");
        assert!(!html.contains("class=\"margin\""), "no card is beside the text");
    }

    /// Notes and comments share a scene after the map (task 1375): a tab
    /// each, the notes picked; the notes as rows, newest first, with a pill
    /// a kind, and whole under them for the sheet and for print; the sheet
    /// after the page, out of every scene. Alone, either is the scene, with
    /// no tabs; notes of one kind have no pills.
    #[test]
    fn the_notes_and_comments_share_a_scene() {
        use crate::item::Knowledge;
        let made = steps(&[], &[spec("a", Some("First"), &[])]).unwrap().0;
        let item = artifact_item(1, &plan("Ship"), made);
        let note = |id: u32, text: &str, kind: Option<Knowledge>| {
            let mut note = Item::new_note(id, text.to_string(), vec!["My Board".to_string()]);
            note.attached_to = item.uid.clone();
            note.knowledge = kind;
            note.timestamp = i64::from(id) * 86_400_000;
            note
        };
        let mut comment = note(5, "A comment", None);
        comment.comment = Some(Box::new(crate::item::Comment {
            version: 1,
            quote: None,
            replacement: None,
            step: None,
            reply_to: None,
            sent: None,
            resolved: None,
            applied: None,
            theme: None,
            color: None,
            unknown: BTreeMap::new(),
        }));
        let mut all: ItemMap = BTreeMap::from([
            (1, item.clone()),
            (2, note(2, "Keep it light. It reloads often.", Some(Knowledge::Decision))),
            (3, note(3, "Mind the lock.", Some(Knowledge::Gotcha))),
            (4, note(4, "Read this first.", Some(Knowledge::Decision))),
            (5, comment),
        ]);
        let (html, _) = page(&item, &all, None);
        assert_eq!(html.matches("class=\"scene part talk\"").count(), 1, "one scene: {html}");
        let scene = html.split("<section class=\"scene part talk\"").nth(1).and_then(|rest| rest.split("</section>").next()).unwrap_or_default();
        assert!(
            scene.starts_with(" id=\"notes-and-comments\" data-part=\"Notes and comments\" data-mode=\"light\"><h2 class=\"opener words\" data-chrome><span class=\"l1\"><span class=\"w\">Notes</span></span> <span class=\"l2\"><span class=\"w\">and</span> <span class=\"w\">comments</span></span></h2><div class=\"tabs\" role=\"tablist\" aria-label=\"Notes and comments\"><button class=\"pill tab\" type=\"button\" role=\"tab\" id=\"notes-tab\" aria-controls=\"notes\" aria-selected=\"true\">Notes<sup>3</sup></button><button class=\"pill tab\" type=\"button\" role=\"tab\" id=\"comments-tab\" aria-controls=\"comments\" aria-selected=\"false\" tabindex=\"-1\">Comments<sup>1</sup></button></div><div class=\"panels\"><div class=\"panel\" id=\"notes\" role=\"tabpanel\" aria-labelledby=\"notes-tab\" data-name=\"Notes\"><div class=\"kinds\" role=\"toolbar\" aria-label=\"Which notes show\"><button class=\"pill\" type=\"button\" aria-pressed=\"true\">All<sup>3</sup></button><button class=\"pill\" type=\"button\" data-kind=\"Decision\" aria-pressed=\"false\">Decisions<sup>2</sup></button><button class=\"pill\" type=\"button\" data-kind=\"Gotcha\" aria-pressed=\"false\">Gotchas<sup>1</sup></button></div><ul class=\"rows\"><li data-kind=\"Decision\"><button class=\"row\" type=\"button\" data-note=\"4\" aria-haspopup=\"dialog\"><small>Decision 4 \u{b7} "
            ),
            "the opener, the tabs, the kinds and the newest note's row: {scene}"
        );
        assert!(scene.contains("</small><strong>Read this first.</strong><i aria-hidden=\"true\">\u{2198}</i></button></li>"), "{scene}");
        let (rows, rest) = scene.split_once("</ul><div class=\"whole\">").expect("the notes whole after the rows");
        let (whole, comments) = rest.split_once("</div></div><div class=\"panel\" id=\"comments\" role=\"tabpanel\" aria-labelledby=\"comments-tab\" data-name=\"Comments\" hidden><div class=\"filters\" role=\"toolbar\" aria-label=\"Which comments show\"></div><ol class=\"comments\">").expect("the comments' tab, not picked");
        let order = |text: &str, marks: &[&str]| marks.iter().map(|mark| text.find(mark)).collect::<Option<Vec<usize>>>().is_some_and(|at| at.windows(2).all(|two| two[0] < two[1]));
        assert!(order(rows, &["data-note=\"4\"", "data-note=\"3\"", "data-note=\"2\""]), "newest first: {rows}");
        assert!(order(whole, &["<div class=\"note\" id=\"note-4\" data-kind=\"Decision\">", "<div class=\"note\" id=\"note-3\" data-kind=\"Gotcha\">", "<div class=\"note\" id=\"note-2\" data-kind=\"Decision\">"]), "{whole}");
        assert!(whole.contains("<h3>Keep it light.</h3><p class=\"text\">It reloads often.</p>"), "each note whole: {whole}");
        assert!(comments.starts_with("<li class=\"entry\" id=\"comment-5\"") && comments.ends_with("</ol></div></div>"), "{comments}");
        assert!(order(&html, &["id=\"map\"", "id=\"notes-and-comments\"", "id=\"history\""]), "after the map, before History: {html}");
        let (main, after) = html.split_once("</main>").unwrap_or_default();
        assert!(!main.contains("<dialog") && after.matches("<dialog class=\"side prose\" aria-label=\"Note\"><div class=\"side-in\"><button class=\"round shut\" type=\"button\" aria-label=\"Close\">").count() == 1, "one sheet, after the page: {html}");
        let index = html.split("<div class=\"toc-card\"><ol>").nth(1).and_then(|rest| rest.split("</ol>").next()).unwrap_or_default();
        let names: Vec<&str> = index.split("<span class=\"t\">").skip(1).filter_map(|name| name.split('<').next()).collect();
        assert_eq!(names, ["Overview", "Goal", "What is known", "Design", "Risks and open questions", "Steps", "Map", "Notes and comments", "History", "What&#39;s next"], "the scene once in the index");
        assert!(html.contains(&format!("aria-label=\"Sections\">{}</button>", "<span></span>".repeat(names.len()))), "a bar each: {html}");

        // Alone, and of one kind.
        all.remove(&5);
        let (html, _) = page(&item, &all, None);
        let scene = html.split("<section class=\"scene part talk\"").nth(1).and_then(|rest| rest.split("</section>").next()).unwrap_or_default();
        assert!(scene.starts_with(" id=\"notes\" data-part=\"Notes\" data-mode=\"light\"><h2 class=\"opener words\" data-chrome><span class=\"l1\"><span class=\"w\">Notes</span></span></h2><div class=\"kinds\""), "{scene}");
        assert!(!scene.contains("class=\"tabs\"") && !scene.contains("class=\"panel") && html.contains("<a href=\"#notes\">Notes</a><a href=\"#history\">"), "{scene}");
        all.remove(&3);
        let (html, _) = page(&item, &all, None);
        assert!(!html.contains("class=\"kinds\"") && html.contains("<ul class=\"rows\"><li data-kind=\"Decision\">"), "one kind, no pills: {html}");
        all.retain(|id, _| *id == 1);
        all.insert(5, {
            let mut comment = note(5, "A comment", None);
            comment.comment = Some(Box::new(crate::item::Comment { version: 1, quote: None, replacement: None, step: None, reply_to: None, sent: None, resolved: None, applied: None, theme: None, color: None, unknown: BTreeMap::new() }));
            comment
        });
        let (html, _) = page(&item, &all, None);
        let scene = html.split("<section class=\"scene part talk\"").nth(1).and_then(|rest| rest.split("</section>").next()).unwrap_or_default();
        assert!(scene.starts_with(" id=\"comments\" data-part=\"Comments\" data-mode=\"light\"><h2 class=\"opener words\" data-chrome><span class=\"l1\"><span class=\"w\">Comments</span></span></h2><div class=\"filters\""), "{scene}");
        assert!(!html.contains("<dialog") && !html.contains("class=\"rows\"") && !html.contains("class=\"tabs\""), "no notes, no sheet: {html}");
    }

    /// What's next closes the page (task 1377), dark, after History: the
    /// review asked for, else the next step, the first not done or
    /// cancelled, with the way to it; Review, the command and the way to the
    /// top always; the footer in it.
    #[test]
    fn the_close_says_what_comes_next() {
        let mut made = steps(&[], &[spec("a", Some("First\nWhy."), &[]), spec("b", Some("Second <it>"), &["a"])]).unwrap().0;
        let mut done = Item::new_task(2, "First".into(), vec![], 1);
        State::Done.write(&mut done);
        made[0].task = done.uid.clone();
        let pending = Item::new_task(3, "Second".into(), vec![], 1);
        made[1].task = pending.uid.clone();
        let item = artifact_item(1, &plan("Ship"), made);
        let mut all: ItemMap = BTreeMap::from([(1, item.clone()), (2, done), (3, pending)]);
        let close = |all: &ItemMap| {
            let (html, _) = page(&all[&1], all, None);
            let scene = html.split("<section class=\"scene part close\" id=\"next\" data-part=\"What's next\" data-mode=\"dark\">").nth(1).and_then(|rest| rest.split("</section>").next()).map(str::to_string);
            (html, scene.unwrap_or_default())
        };
        let (html, scene) = close(&all);
        let acts = |middle: &str| {
            format!(
                "<div class=\"acts\"><button class=\"pill writes-only\" type=\"button\" data-review>Review</button>{middle}<button class=\"pill\" type=\"button\" data-copy>Copy ekko artifact 1</button><button class=\"pill\" type=\"button\" data-top>Back to the top <i aria-hidden=\"true\">\u{2191}</i></button></div><footer class=\"foot\">Written by ekko "
            )
        };
        assert!(scene.starts_with("<h2 class=\"opener words\" data-chrome><span class=\"l1\"><span class=\"w\">What&#39;s</span></span> <span class=\"l2\"><span class=\"w\">next?</span></span></h2><p class=\"next\">Step 02, pending: Second &lt;it&gt;</p>"), "the next step, the first not done: {scene}");
        assert!(scene.contains(&acts("<button class=\"pill\" type=\"button\" data-step=\"b\">Go to step 02</button>")) && scene.ends_with("</footer>"), "{scene}");
        let order = ["id=\"history\"", "id=\"next\"", "</article></main>"].map(|mark| html.find(mark));
        assert!(order.windows(2).all(|two| two[0].is_some() && two[0] < two[1]), "after History, the page's end: {order:?}");
        assert!(html.contains("<li><a href=\"#next\"><span class=\"d\"></span><span class=\"t\">What&#39;s next</span></a></li></ol>"), "the index's last: {html}");

        // An approval asked: the review, and no step to go to.
        let mut asked = Item::new_note(4, "Approve the plan?".to_string(), vec!["My Board".to_string()]);
        asked.attached_to = item.uid.clone();
        asked.question = Some(serde_json::from_value(serde_json::json!({"rev": 1, "approve": {"artifact": item.uid, "version": 1, "steps": ["a", "b"]}})).unwrap());
        all.insert(4, asked);
        let (_, scene) = close(&all);
        assert!(scene.contains("<p class=\"next\">This plan waits on your review: approve it, or ask for changes with comments on its words.</p>") && scene.contains(&acts("")), "{scene}");

        // Every step done.
        all.remove(&4);
        State::Done.write(all.get_mut(&3).unwrap());
        let (_, scene) = close(&all);
        assert!(scene.contains("<p class=\"next\">Every step is done.</p>") && scene.contains(&acts("")), "{scene}");
    }

    /// History lays the texts the plan kept on a line, oldest first, each
    /// under the date it was made, and shows one change at a time, the
    /// newest, with a slider from the oldest to it (task 1376). Versions
    /// that changed only the steps kept no text, so the line can skip them;
    /// one change has no slider, and none no line.
    #[test]
    fn the_history_lays_the_texts_kept_on_a_line() {
        let day = |n: i64| n * 86_400_000;
        let mut item = artifact_item(1, &plan("Ship"), Vec::new());
        item.timestamp = day(1);
        let text = |design: &str| plan("Ship").replace("## Design\nText.", &format!("## Design\n{design}"));
        item.description = text("Third.");
        let plan_now = item.artifact.as_mut().unwrap();
        plan_now.version = 4;
        plan_now.earlier = vec![Earlier { version: 1, at: day(2), text: text("First."), unknown: BTreeMap::new() }, Earlier { version: 3, at: day(4), text: text("Second."), unknown: BTreeMap::new() }];
        let (html, _) = page(&item, &BTreeMap::from([(1, item.clone())]), None);
        let scene = html.split("<section class=\"scene part changes\" id=\"history\"").nth(1).and_then(|rest| rest.split("</section>").next()).unwrap_or_default();
        let line = format!(
            "<div class=\"versions\"><ol class=\"line\" aria-label=\"Versions kept\"><li><button type=\"button\" data-node=\"0\" aria-current=\"false\"><b>v1</b><span>{}</span></button></li><li><button type=\"button\" data-node=\"1\" aria-current=\"true\"><b>v3</b><span>{}</span></button></li><li><button type=\"button\" data-node=\"2\" aria-current=\"true\"><b>v4</b><span>{}</span></button></li></ol><input class=\"scrub\" type=\"range\" autocomplete=\"off\" min=\"1\" max=\"2\" step=\"1\" value=\"2\" style=\"--p: 100%\" aria-label=\"Which change shows\" aria-valuetext=\"Version 3 \u{2192} 4\"></div><div class=\"changed\"><div class=\"version\" id=\"version-3\" data-change=\"2\"><div class=\"kind\">Replaced {}</div><h3>Version 3 \u{2192} 4</h3>",
            when(day(1)),
            when(day(2)),
            when(day(4)),
            when(day(4))
        );
        assert!(scene.contains(&line), "the line, the slider and the newest change shown: {scene}");
        let older = scene.split("<div class=\"version\" id=\"version-1\" data-change=\"1\" hidden><div class=\"kind\">").nth(1).unwrap_or_default();
        assert!(older.contains("<h3>Version 1 \u{2192} 2</h3>") && older.contains("<div class=\"del\">- First.</div><div class=\"add\">+ Second.</div>"), "the oldest, hidden, against the text after it: {scene}");

        item.artifact.as_mut().unwrap().earlier.remove(0);
        let (html, _) = page(&item, &BTreeMap::from([(1, item.clone())]), None);
        assert!(html.contains("<b>v3</b><span></span>") && !html.contains("class=\"scrub\"") && html.contains("<div class=\"version\" id=\"version-3\" data-change=\"1\"><div"), "one change: no date known for v3, no slider, the change shown: {html}");
        item.artifact.as_mut().unwrap().earlier.clear();
        let (html, _) = page(&item, &BTreeMap::from([(1, item.clone())]), None);
        assert!(!html.contains("class=\"line\"") && html.contains("<p class=\"history\">One text so far: nothing to compare."), "{html}");
    }

    /// Each color of THEMES has its tint and its ink in both looks, and the
    /// highlight that marks words being quoted: the rule over every theme,
    /// so a seventh color cannot ship half drawn.
    #[test]
    fn every_theme_has_its_colors_in_both_looks() {
        let (light, dark) = STYLE.split_once(":root[data-theme=\"dark\"]").expect("a dark look");
        let dark = dark.split_once('}').map_or("", |(block, _)| block);
        for (color, _) in THEMES {
            for name in [format!("--tint-{color}:"), format!("--ink-{color}:")] {
                assert!(light.contains(&name) && dark.contains(&name), "{name} in both looks");
            }
            assert!(STYLE.contains(&format!("::highlight(ekko-quoted-{color}) {{ background-color: var(--tint-{color}); }}")), "{color} marks words being quoted");
        }
        assert_eq!(theme_refused(Some("Question"), Some("blue")), None);
        assert!(theme_refused(None, Some("teal")).is_some() && theme_refused(Some(""), None).is_some() && theme_refused(Some(&"x".repeat(41)), None).is_some());
        assert_eq!(theme_refused(Some(&"é".repeat(40)), None), None, "forty characters, not bytes");
    }

    #[test]
    fn the_page_holds_its_parts_in_order_its_notes_and_how_its_text_changed() {
        let made = steps(&[], &[spec("a", Some("First"), &[]), spec("b", Some("Second"), &["a"])]).unwrap().0;
        let mut item = artifact_item(1, &plan("Ship"), made);
        item.description = item.description.replace("## Design\nText.", "## Design\nThe new design.");
        let plan_now = item.artifact.as_mut().unwrap();
        plan_now.version = 3;
        plan_now.earlier.push(Earlier { version: 2, at: 0, text: plan("Ship"), unknown: BTreeMap::new() });
        let mut note = Item::new_note(2, "Keep the page light. It reloads often.\nAnd it reads well.".to_string(), vec!["My Board".to_string()]);
        note.attached_to = item.uid.clone();
        let all: ItemMap = BTreeMap::from([(1, item.clone()), (2, note)]);
        let (html, _) = page(&item, &all, None);
        assert_eq!(html.matches(" data-command=\"ekko artifact 1\"").count(), 1, "the command is offered once, by the pill: {html}");
        assert!(!html.contains("<header") && !html.contains("id=\"theme\""), "no bar above the page: its commands are the pill's (task 1337)");
        let about = format!("<p class=\"about\">Version 3 \u{b7} updated {} \u{b7} the default board<span id=\"who\"></span></p>", when(item.updated_at.unwrap_or(item.timestamp)));
        assert!(html.contains(&about), "the version, when, the board and whom it writes as, at History's head: {html}");
        let legend = html.split("id=\"map\"").nth(1).and_then(|map| map.split_once("<div class=\"legend\">")).map(|(before, after)| (before.contains("class=\"strip\""), after.split("</div>").next().unwrap_or("")));
        assert_eq!(legend.map(|(late, _)| late), Some(false), "the map has a legend above its strip: {html}");
        for state in ["dot proposed\"></span>to approve", "dot pending\"></span>pending", "dot progress\"></span>in progress", "dot done\"></span>done", "an arrow: what a step waits on"] {
            assert!(legend.is_some_and(|(_, text)| text.contains(state)), "{state}: {legend:?}");
        }
        let parts = [
            ("plan-goal", "Goal"),
            ("plan-what-is-known", "What is known"),
            ("plan-design", "Design"),
            ("plan-risks-and-open-questions", "Risks and open questions"),
            ("steps", "Steps"),
            ("map", "Map"),
            ("notes", "Notes"),
            ("history", "History"),
        ];
        let mut from = 0;
        for (id, name) in parts {
            from += html[from..].find(&format!("id=\"{id}\" data-part=\"{name}\"")).unwrap_or_else(|| panic!("{id}, in this order: {html}"));
        }
        assert!(html[from..].contains("id=\"next\" data-part=\"What's next\""), "What's next closes it: {html}");
        let rows: String = std::iter::once(&("top", "Overview"))
            .chain(&parts)
            .chain(std::iter::once(&("next", "What&#39;s next")))
            .map(|(id, name)| format!("<li><a href=\"#{id}\"><span class=\"d\"></span><span class=\"t\">{name}</span></a></li>"))
            .collect();
        assert!(html.contains(&format!("<div class=\"toc-card\"><ol>{rows}</ol></div>")), "the section bars name the first scene and each part, in the same order: {html}");
        assert!(html.contains(&format!("aria-label=\"Sections\">{}</button>", "<span></span>".repeat(parts.len() + 2))), "a bar each: {html}");
        let nav = "<a href=\"#plan-goal\">Plan</a><a href=\"#steps\">Steps</a><a href=\"#map\">Map</a><a href=\"#notes\">Notes</a><a href=\"#history\">History</a><a href=\"#next\">What's next</a>";
        assert!(html.contains(&format!("aria-label=\"Parts\">{nav}</nav>")), "the bar's menu: {html}");
        assert!(html.contains("data-part=\"What is known\" data-mode=\"light\" data-plan><h2 class=\"opener words\" data-chrome>") && html.contains("data-part=\"Goal\" data-mode=\"dark\" data-plan><div class=\"held\"><div class=\"label\" data-chrome>Goal</div><p class=\"statement\">"), "the Goal opens with its statement: {html}");
        assert!(html.contains("<h3>Keep the page light.</h3>") && html.contains("<p class=\"text\">It reloads often.\nAnd it reads well.</p>"), "a note's first sentence is its title: {html}");
        assert!(html.contains("<div class=\"note\" id=\"note-2\" data-kind=\"Note\"><div class=\"kind\">Note 2 \u{b7} "), "{html}");
        assert!(html.contains("Version 2 \u{2192} 3") && html.contains("<div class=\"del\">- Text.</div><div class=\"add\">+ The new design.</div>"), "{html}");
        assert!(html.contains("<p class=\"history\">Version 3 is the current one; none is approved yet.</p>"), "{html}");
        assert!(html.contains("<path class=\"edge\""), "b waits on a: {html}");

        let bare = artifact_item(1, &plan("Ship"), Vec::new());
        let (html, _) = page(&bare, &BTreeMap::from([(1, bare.clone())]), None);
        assert!(html.contains("id=\"steps\" data-part=\"Steps\" data-mode=\"light\"><h2 class=\"opener words\" data-chrome><span class=\"l1\"><span class=\"w\">Steps</span></span></h2><p>No steps yet: the artifact tool writes them.</p>"), "{html}");
        assert!(!html.contains("data-part=\"Map\"") && !html.contains("data-part=\"Notes\""), "no map without steps, no notes without one: {html}");
        assert!(!html.contains("href=\"#map\"") && !html.contains("href=\"#notes\""), "nor do the bars name them: {html}");
    }

    /// The words a part of the page shows, as the comments read them in the
    /// page's script: its text but what `data-chrome` marks, each block
    /// apart from the next (BLOCK in SCRIPT), runs of white space one space.
    fn page_words(html: &str) -> String {
        const BLOCKS: [&str; 15] = ["p", "li", "h1", "h2", "h3", "h4", "h5", "h6", "pre", "td", "th", "blockquote", "section", "dt", "dd"];
        let mut words = String::new();
        let mut rest = html;
        // The element left out, by its tag, and how deep it nests in itself.
        let mut chrome: Option<(String, usize)> = None;
        while let Some(at) = rest.find('<') {
            if chrome.is_none() {
                words.push_str(&rest[..at]);
            }
            let end = rest[at..].find('>').map_or(rest.len(), |end| at + end + 1);
            let tag = &rest[at + 1..end - 1];
            let closing = tag.starts_with('/');
            let name: String = tag.trim_start_matches('/').chars().take_while(char::is_ascii_alphanumeric).collect();
            let void = tag.ends_with('/') || ["br", "hr", "img", "input"].contains(&name.as_str());
            if let Some((open, depth)) = chrome.as_mut() {
                if *open == name && !void {
                    *depth = if closing { *depth - 1 } else { *depth + 1 };
                }
                if *depth == 0 {
                    chrome = None;
                }
            } else if !closing && !void && tag.contains(" data-chrome") {
                chrome = Some((name.clone(), 1));
            }
            if BLOCKS.contains(&name.as_str()) {
                words.push(' ');
            }
            rest = &rest[end..];
        }
        words.push_str(rest);
        collapsed(&words.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&amp;", "&"))
    }

    #[test]
    fn the_page_is_a_run_of_scenes_each_in_its_mode_and_the_index_names_them() {
        let made = steps(&[], &[spec("a", Some("First"), &[]), spec("b", Some("Second"), &["a"])]).unwrap().0;
        let text = plan("Ship the page")
            .replace("Ship the page\n\n", "Ship the page\n\nA lead, *before* the goal.\n\n")
            .replace("## Goal\nText.", "## Goal\nShip the page. Then `measure` it.")
            .replace("## What is known\nText.", "## What is known\nFound so far:\n\n- one **thing**\n  - inside it\n- two & <three>\n\n| a | b |\n|---|---|\n| c | d |")
            .replace("## Design\nText.", "## Design\n### Its parts\n\n```\ncode here\n```\n\n> Quoted, [linked](https://example.com).");
        let mut item = artifact_item(7, &text, made);
        item.priority = Some(3);
        let mut note = Item::new_note(2, "Keep it light.".to_string(), vec!["My Board".to_string()]);
        note.attached_to = item.uid.clone();
        let all: ItemMap = BTreeMap::from([(7, item.clone()), (2, note)]);
        let (html, _) = page(&item, &all, Some(Path::new("/projects/site")));

        // The column beside the text and the head above it are gone: what
        // they said opens the first scene, and heads History.
        for gone in ["class=\"standing\"", "class=\"tags\"", "class=\"byline\"", "class=\"actions\"", " min read"] {
            assert!(!html.contains(gone), "{gone}: {html}");
        }
        assert!(html.contains("<p class=\"kicker\"><span>Artifact 7</span><span>Priority 3</span><span class=\"state\">Draft</span></p>"), "{html}");
        assert!(html.contains(" \u{b7} project site<span id=\"who\"></span></p>"), "the board, and whom the page writes as: {html}");

        // Scenes, in the page's order, each in its mode; the index names
        // them all, a bar each.
        let scenes: Vec<(String, String)> = html
            .split("<section class=\"scene")
            .skip(1)
            .map(|scene| {
                let tag = scene.split('>').next().unwrap_or_default();
                let attr = |name: &str| tag.split_once(&format!(" {name}=\"")).and_then(|(_, rest)| rest.split('"').next()).unwrap_or_default().to_string();
                (attr("id"), attr("data-mode"))
            })
            .collect();
        let want = [
            ("top", "light", "Overview"),
            ("plan-goal", "dark", "Goal"),
            ("plan-what-is-known", "light", "What is known"),
            ("plan-design", "light", "Design"),
            ("plan-risks-and-open-questions", "light", "Risks and open questions"),
            ("steps", "light", "Steps"),
            ("map", "dark", "Map"),
            ("notes", "light", "Notes"),
            ("history", "light", "History"),
            ("next", "dark", "What&#39;s next"),
        ];
        assert_eq!(scenes, want.map(|(id, mode, _)| (id.to_string(), mode.to_string())), "{html}");
        let rows: String = want.iter().map(|(id, _, name)| format!("<li><a href=\"#{id}\"><span class=\"d\"></span><span class=\"t\">{name}</span></a></li>")).collect();
        assert!(html.contains(&format!("<div class=\"toc-card\"><ol>{rows}</ol></div>")), "{html}");
        assert!(html.contains(&format!("aria-label=\"Sections\">{}</button>", "<span></span>".repeat(want.len()))), "{html}");
        assert!(html.contains("<div class=\"mark\" aria-hidden=\"true\">ekko <b>\u{b7}</b> artifact 7</div>"), "{html}");
        assert!(html.contains("<h2 class=\"opener words\" data-chrome><span class=\"l1\"><span class=\"w\">What</span> <span class=\"w\">is</span></span> <span class=\"l2\"><span class=\"w\">known</span></span></h2>"), "AKQA's opener, its second line grey: {html}");

        // In the plan's parts the page's words are the plan's: what the page
        // adds there is chrome, which the comments' words skip.
        let said: Vec<String> = section_ranges(text.split_once('\n').unwrap().1).into_iter().map(|(_, range)| shown_words(&text.split_once('\n').unwrap().1[range])).collect();
        let shown: Vec<String> = html
            .split("<section ")
            .skip(1)
            .filter(|section| section.split('>').next().is_some_and(|tag| tag.contains(" data-plan")))
            .map(|section| page_words(&format!("<section {}", section.split("</section>").next().unwrap_or_default())))
            .collect();
        assert_eq!(shown, said, "{html}");
        assert!(SCRIPT.contains("closest(\"[data-chrome]\")"), "the comments' words skip chrome");

        // Reveals hide only where the script can bring them back, on a
        // screen, with motion allowed: still under reduced motion, plain in
        // print.
        assert!(THEME.contains("root.dataset.motion = \"\""), "the head marks a page that can reveal");
        let (_, moving) = STYLE.split_once("@media screen and (prefers-reduced-motion: no-preference) {").expect("reveals in a block of their own");
        assert!(moving.split("\n}\n").next().is_some_and(|block| block.contains(":root[data-motion] .scene >") && block.contains("blur(6px)")), "{moving}");
        assert_eq!(STYLE.matches(":root[data-motion]").count(), moving.split("\n}\n").next().unwrap_or_default().matches(":root[data-motion]").count(), "nothing hides outside it");
    }

    #[test]
    fn the_goal_opens_with_its_first_sentence() {
        for (text, statement, rest) in [
            ("Ship the page. Then measure it.", "Ship the page.", "<p>Then measure it.</p>\n"),
            ("Ship the page.\nThen measure it.", "Ship the page.", "<p>Then measure it.</p>\n"),
            ("Ship the page.\n\nThen measure it.", "Ship the page.", "<p>Then measure it.</p>\n"),
            ("Ship the page", "Ship the page", ""),
            ("Is it shipped? Yes.", "Is it shipped?", "<p>Yes.</p>\n"),
            ("Ship it (\"today.\") Then rest.", "Ship it (\"today.\")", "<p>Then rest.</p>\n"),
            ("Ship it, e.g. today. Then rest.", "Ship it, e.g. today.", "<p>Then rest.</p>\n"),
            ("Version 1.5 ships.", "Version 1.5 ships.", ""),
            ("Ship **the page. Now** and rest. Later.", "Ship <strong>the page. Now</strong> and rest.", "<p>Later.</p>\n"),
            ("Ship [the page. Now](https://example.com) here! Then.", "Ship <a href=\"https://example.com\">the page. Now</a> here!", "<p>Then.</p>\n"),
            ("Ship `a. b` now. Then.", "Ship <code>a. b</code> now.", "<p>Then.</p>\n"),
            ("<b>Raw</b> first. Then.", "&lt;b&gt;Raw&lt;/b&gt; first.", "<p>Then.</p>\n"),
            ("Ship it. And *this* too.\nMore.", "Ship it.", "<p>And <em>this</em> too.\nMore.</p>\n"),
        ] {
            assert_eq!(goal_split(text), (Some(statement.to_string()), rest.to_string()), "{text}");
        }
        assert_eq!(goal_split("- A list first.\n"), (None, "<ul>\n<li>A list first.</li>\n</ul>\n".to_string()), "no statement unless it opens with a paragraph");
        assert_eq!(goal_split("### A heading. First\n\nThen.\n"), (None, "<h3>A heading. First</h3>\n<p>Then.</p>\n".to_string()));
        assert_eq!(goal_split(""), (None, String::new()));
    }

    #[test]
    fn the_goal_scene_holds_its_statement_word_by_word() {
        assert_eq!(
            lit_words("Ship <strong>the page</strong> with <code>a b</code>, now &amp; then."),
            "<span class=\"w\">Ship</span> <strong><span class=\"w\">the</span> <span class=\"w\">page</span></strong> <span class=\"w\">with</span> <code>a b</code><span class=\"w\">,</span> <span class=\"w\">now</span> <span class=\"w\">&amp;</span> <span class=\"w\">then.</span>",
            "a word a span, code one, the tags kept"
        );
        assert_eq!(
            goal_scene("Ship the page. Then measure it."),
            "<div class=\"held\"><div class=\"label\" data-chrome>Goal</div><p class=\"statement\"><span class=\"w\">Ship</span> <span class=\"w\">the</span> <span class=\"w\">page.</span></p><div class=\"goal-rest\"><p>Then measure it.</p>\n</div><div class=\"progress\" data-chrome aria-hidden=\"true\"><i></i></div></div>"
        );
        assert!(!goal_scene("- A list first.\n").contains("statement"), "no statement to light");
    }

    #[test]
    fn what_is_known_puts_each_group_behind_a_tab_and_each_finding_on_a_card() {
        let known = |section: &str| {
            let text = plan("Ship").replace("## What is known\nText.", &format!("## What is known\n{section}"));
            let item = artifact_item(1, &text, Vec::new());
            let (html, _) = page(&item, &BTreeMap::from([(1, item.clone())]), None);
            let scene = html.split("<section class=\"scene part").nth(1).and_then(|scene| scene.split("</section>").next()).unwrap_or_default().to_string();
            // The words the scene shows, chrome left out, and the plan's.
            let body = text.split_once('\n').unwrap().1;
            let said = section_ranges(body).into_iter().find(|(heading, _)| heading == "What is known").map(|(_, range)| shown_words(&body[range])).unwrap_or_default();
            let shown = page_words(&format!("<section class=\"scene part{scene}</section>"));
            (scene, shown, said)
        };

        let (scene, shown, said) = known("On this machine, read today:\n- First finding. With more words.\n- A `code` one, with no stop\n\nFrom the web (read twice):\n1. Third. Last.\n");
        assert!(scene.starts_with(" known\" id=\"plan-what-is-known\" data-part=\"What is known\" data-mode=\"light\" data-plan>"), "{scene}");
        // A tab a group, named by its paragraph's first words and counting
        // its findings; the first group shows.
        assert!(
            scene.contains("<div class=\"tabs\" role=\"tablist\" aria-label=\"What is known\" data-chrome><button class=\"pill tab\" type=\"button\" role=\"tab\" id=\"plan-what-is-known-tab-1\" aria-controls=\"plan-what-is-known-group-1\" aria-selected=\"true\">On this machine<sup>2</sup></button><button class=\"pill tab\" type=\"button\" role=\"tab\" id=\"plan-what-is-known-tab-2\" aria-controls=\"plan-what-is-known-group-2\" aria-selected=\"false\" tabindex=\"-1\">From the web<sup>1</sup></button></div>"),
            "{scene}"
        );
        assert!(scene.contains("<div class=\"group\" id=\"plan-what-is-known-group-1\" role=\"tabpanel\" aria-labelledby=\"plan-what-is-known-tab-1\"><p>On this machine, read today:</p>\n<ul class=\"track\" tabindex=\"0\" aria-label=\"On this machine\">"), "{scene}");
        assert!(scene.contains("<div class=\"group\" id=\"plan-what-is-known-group-2\" role=\"tabpanel\" aria-labelledby=\"plan-what-is-known-tab-2\" hidden><p>From the web (read twice):</p>\n<ol class=\"track\" tabindex=\"0\" aria-label=\"From the web\">"), "{scene}");
        // A card a finding: its number and its button are chrome, and its
        // first sentence is its lead, the space after it kept.
        let read = "<button class=\"more\" type=\"button\" aria-expanded=\"false\" data-chrome><span>Read</span> <i aria-hidden=\"true\">\u{2198}</i></button>";
        assert!(scene.contains(&format!("<li class=\"card\" id=\"plan-what-is-known-card-1-1\"><span class=\"num\" data-chrome>01 / 02</span><div class=\"body\"><span class=\"lead\">First finding.</span> With more words.</div>{read}</li>")), "{scene}");
        assert!(scene.contains(&format!("<li class=\"card\" id=\"plan-what-is-known-card-1-2\"><span class=\"num\" data-chrome>02 / 02</span><div class=\"body\">A <code>code</code> one, with no stop</div>{read}</li>")), "no stop, no lead: {scene}");
        assert!(scene.contains(&format!("<li class=\"card\" id=\"plan-what-is-known-card-2-1\"><span class=\"num\" data-chrome>01 / 01</span><div class=\"body\"><span class=\"lead\">Third.</span> Last.</div>{read}</li>")), "{scene}");
        assert!(scene.ends_with("<div class=\"controls\" data-chrome><span class=\"count\">01 / 02</span><span class=\"grow\"></span><button class=\"round\" type=\"button\" data-by=\"-1\" aria-label=\"Back\" disabled><svg viewBox=\"0 0 24 24\" aria-hidden=\"true\"><path d=\"M14.5 5.5L8 12l6.5 6.5\"/></svg></button><button class=\"round\" type=\"button\" data-by=\"1\" aria-label=\"Next\"><svg viewBox=\"0 0 24 24\" aria-hidden=\"true\"><path d=\"M9.5 5.5L16 12l-6.5 6.5\"/></svg></button></div>"), "{scene}");
        assert_eq!(shown, said, "the plan's words, in its order");

        // One group needs no tab; a loose list's paragraphs, a nested list
        // and what follows the list stay as they are.
        let (scene, shown, said) = known("- **First** finding, *a long one*. Then more.\n\n  Its second paragraph.\n  - inside it\n\nAfter the list.\n");
        assert!(!scene.contains("role=\"tab"), "{scene}");
        assert!(scene.contains("<div class=\"group\"><ul class=\"track\" tabindex=\"0\" aria-label=\"Findings\"><li class=\"card\" id=\"plan-what-is-known-card-1-1\">"), "a card has an id, a jump to it shows it: {scene}");
        assert!(scene.contains("<div class=\"body\"><p><span class=\"lead named\"><strong>First</strong> finding, <em>a long one</em>.</span> Then more.</p>\n<p>Its second paragraph.</p>\n<ul>\n<li>inside it</li>\n</ul>\n</div>"), "{scene}");
        assert!(scene.contains("</div><div class=\"controls\" data-chrome><span class=\"count\">01 / 01</span>"), "the controls under the track: {scene}");
        assert!(scene.ends_with("</div><p>After the list.</p>\n"), "{scene}");
        assert_eq!(shown, said);

        // A colon past the first few words ends a lead too.
        let (scene, ..) = known("- There are 232 notes: 98 decisions. More.\n- Note: one. Two.\n");
        assert!(scene.contains("<span class=\"lead\">There are 232 notes:</span> 98 decisions. More."), "{scene}");
        assert!(scene.contains("<span class=\"lead\">Note: one.</span> Two."), "{scene}");

        // A first sentence too long to set large is no lead.
        let long = format!("- {} words. Then.\n", "many ".repeat(60));
        assert!(!known(&long).0.contains("class=\"lead\""));

        // With no list there is nothing to put on a track.
        let (scene, ..) = known("Nothing listed.\n");
        assert!(scene.starts_with("\" id=\"plan-what-is-known\""), "a plain scene: {scene}");
    }

    #[test]
    fn the_design_holds_a_numeral_beside_its_points() {
        let design = |section: &str| {
            let text = plan("Ship").replace("## Design\nText.", &format!("## Design\n{section}"));
            let item = artifact_item(1, &text, Vec::new());
            let (html, _) = page(&item, &BTreeMap::from([(1, item.clone())]), None);
            let scene = html.split(" id=\"plan-design\"").nth(1).and_then(|scene| scene.split("</section>").next()).unwrap_or_default().to_string();
            let tag = html.split(" id=\"plan-design\"").next().and_then(|before| before.rsplit("<section ").next()).unwrap_or_default().to_string();
            let body = text.split_once('\n').unwrap().1;
            let said = section_ranges(body).into_iter().find(|(heading, _)| heading == "Design").map(|(_, range)| shown_words(&body[range])).unwrap_or_default();
            (tag, scene.clone(), page_words(&format!("<section{scene}</section>")), said)
        };

        let (tag, scene, shown, said) = design("Three points.\n\n1. **One.** First, and more.\n2. The second: its words.\n3. Three\n   - inside it\n\nAfter them.\n");
        assert_eq!(tag, "class=\"scene part design\"", "{scene}");
        assert!(scene.contains("<h2 class=\"opener words\" data-chrome><span class=\"l1\"><span class=\"w\">Design</span></span> <span class=\"l2\"><span class=\"w\">3</span> <span class=\"w\">decisions</span></span></h2><p>Three points.</p>\n"), "the opener counts the points: {scene}");
        // The numeral, how many and the ticks are chrome, held beside the
        // points; the first point is the one lit.
        assert!(
            scene.contains("<div class=\"story\"><div class=\"hold\" data-chrome><div class=\"numeral\" aria-hidden=\"true\"><span>01</span></div><div class=\"of\" aria-hidden=\"true\">of 03</div><div class=\"ticks\" role=\"group\" aria-label=\"Points\"><button type=\"button\" aria-label=\"Point 1\" aria-current=\"true\"></button><button type=\"button\" aria-label=\"Point 2\" aria-current=\"false\"></button><button type=\"button\" aria-label=\"Point 3\" aria-current=\"false\"></button></div></div><ol class=\"points\">"),
            "{scene}"
        );
        assert!(scene.contains("<li class=\"point on\"><span class=\"lead named\"><strong>One.</strong> First, and more.</span></li>"), "a lead opening in bold is named by it: {scene}");
        assert!(scene.contains("<li class=\"point\"><span class=\"lead\">The second:</span> its words.</li>"), "{scene}");
        assert!(scene.contains("<li class=\"point\">Three\n<ul>\n<li>inside it</li>\n</ul>\n</li></ol></div><p>After them.</p>\n"), "{scene}");
        assert_eq!(shown, said, "the plan's words, in its order");

        // Two points are no story to tell: a plain scene.
        let (tag, scene, ..) = design("1. One.\n2. Two.\n");
        assert_eq!(tag, "class=\"scene part\"", "{scene}");
        assert!(!scene.contains("class=\"story\""), "{scene}");
    }

    #[test]
    fn the_risks_stack_their_sheets_one_in_front() {
        let risks = |section: &str| {
            let text = plan("Ship").replace("## Risks and open questions\nText.", &format!("## Risks and open questions\n{section}"));
            let item = artifact_item(1, &text, Vec::new());
            let (html, _) = page(&item, &BTreeMap::from([(1, item.clone())]), None);
            let scene = html.split(" id=\"plan-risks-and-open-questions\"").nth(1).and_then(|scene| scene.split("</section>").next()).unwrap_or_default().to_string();
            let tag = html.split(" id=\"plan-risks-and-open-questions\"").next().and_then(|before| before.rsplit("<section ").next()).unwrap_or_default().to_string();
            let body = text.split_once('\n').unwrap().1;
            let said = section_ranges(body).into_iter().find(|(heading, _)| heading == "Risks and open questions").map(|(_, range)| shown_words(&body[range])).unwrap_or_default();
            (tag, scene.clone(), page_words(&format!("<section{scene}</section>")), said)
        };

        let (tag, scene, shown, said) = risks("Two kinds.\n\n- The page is long. More words.\n- Open, for the user: the page or the menu. More.\n- Which way (a, b)?\n- **Slow** pages. Measured.\n  - inside it\n\nAfter them.\n");
        assert_eq!(tag, "class=\"scene part risks\"", "{scene}");
        assert!(scene.contains("<span class=\"l1\"><span class=\"w\">Risks</span></span> <span class=\"l2\"><span class=\"w\">and</span> <span class=\"w\">open</span> <span class=\"w\">questions</span></span></h2><p>Two kinds.</p>\n<ul class=\"stack\" tabindex=\"0\" aria-label=\"Risks and open questions\">"), "{scene}");
        // The first sheet in front, each after it a step further back and
        // inert; each says what it is, an open question by its first word
        // or by the question its first sentence asks.
        assert!(scene.contains("<li class=\"sheet\" id=\"plan-risks-and-open-questions-risk-1\" style=\"--d: 0\"><div class=\"num\" data-chrome><span>01 / 04</span><span>Risk</span></div><div class=\"body\"><span class=\"lead\">The page is long.</span> More words.</div></li>"), "{scene}");
        assert!(scene.contains("<li class=\"sheet ask\" id=\"plan-risks-and-open-questions-risk-2\" style=\"--d: 1\" inert><div class=\"num\" data-chrome><span>02 / 04</span><span>Open question</span></div><div class=\"body\"><span class=\"lead\">Open, for the user:</span> the page or the menu. More.</div></li>"), "{scene}");
        assert!(scene.contains("<li class=\"sheet ask\" id=\"plan-risks-and-open-questions-risk-3\" style=\"--d: 2\" inert><div class=\"num\" data-chrome><span>03 / 04</span><span>Open question</span></div><div class=\"body\"><span class=\"lead\">Which way (a, b)?</span></div></li>"), "{scene}");
        assert!(scene.contains("<li class=\"sheet\" id=\"plan-risks-and-open-questions-risk-4\" style=\"--d: 3\" inert><div class=\"num\" data-chrome><span>04 / 04</span><span>Risk</span></div><div class=\"body\"><span class=\"lead named\"><strong>Slow</strong> pages.</span> Measured.\n<ul>\n<li>inside it</li>\n</ul>\n</div></li></ul>"), "{scene}");
        // Under the stack: how far along, the switch, back and on.
        assert!(
            scene.ends_with("</ul><div class=\"controls\" data-chrome><span class=\"count\">01 / 04</span><span class=\"grow\"></span><button class=\"toggle\" type=\"button\" role=\"switch\" aria-checked=\"false\"><i aria-hidden=\"true\"></i><span>All at once</span></button><button class=\"round\" type=\"button\" data-by=\"-1\" aria-label=\"Back\" disabled><svg viewBox=\"0 0 24 24\" aria-hidden=\"true\"><path d=\"M14.5 5.5L8 12l6.5 6.5\"/></svg></button><button class=\"round\" type=\"button\" data-by=\"1\" aria-label=\"Next\"><svg viewBox=\"0 0 24 24\" aria-hidden=\"true\"><path d=\"M9.5 5.5L16 12l-6.5 6.5\"/></svg></button></div><p>After them.</p>\n"),
            "{scene}"
        );
        assert_eq!(shown, said, "the plan's words, in its order");

        // A word that only starts with Open warns.
        let (_, scene, ..) = risks("- Opening the page is slow.\n- Is it? Yes.\n");
        assert!(scene.contains("<span>01 / 02</span><span>Risk</span>") && scene.contains("<span>02 / 02</span><span>Open question</span>"), "{scene}");

        // One item is no stack: a plain scene.
        let (tag, scene, ..) = risks("- Only one.\n");
        assert_eq!(tag, "class=\"scene part\"", "{scene}");
        assert!(!scene.contains("class=\"stack\""), "{scene}");
    }

    #[test]
    fn a_heading_gives_its_section_an_id_of_its_own() {
        assert_eq!(slug("Risks and open questions"), "risks-and-open-questions");
        assert_eq!(slug("  Ça va? Été!  "), "a-va-t");
        assert_eq!(slug("!!!"), "section");
        let mut taken = Vec::new();
        assert_eq!([unique(&mut taken, "plan-x"), unique(&mut taken, "plan-x"), unique(&mut taken, "plan-x")], ["plan-x", "plan-x-2", "plan-x-3"]);
        let text = plan("Ship").replace("## Design\nText.", "## Design\nText.\n## Steps\nTheirs.\n## Steps\nAgain.\n## Goal\nOnce more.");
        let item = artifact_item(1, &text, Vec::new());
        let (html, _) = page(&item, &BTreeMap::from([(1, item.clone())]), None);
        for id in ["plan-steps", "plan-steps-2", "steps"] {
            assert_eq!(html.matches(&format!(" id=\"{id}\" data-part=\"Steps\"")).count(), 1, "{id}: {html}");
        }
        assert!(html.contains("<section class=\"scene part\" id=\"plan-goal-2\" data-part=\"Goal\" data-mode=\"light\" data-plan><h2 class=\"opener words\" data-chrome><span class=\"l1\"><span class=\"w\">Goal</span></span></h2><p>Once more.</p>"), "only the first Goal opens the plan: {html}");
    }

    #[test]
    fn a_step_opens_in_place_when_it_has_more_to_show() {
        let mut made = steps(&[], &[spec("a", Some("First\nWhy it comes first."), &[]), spec("b", Some("Second"), &["a"])]).unwrap().0;
        made[1].done_when = Some("It ships.".to_string());
        let mut done = Item::new_task(2, "First".into(), vec![], 1);
        State::Done.write(&mut done);
        made[0].task = done.uid.clone();
        let item = artifact_item(1, &plan("Ship"), made);
        let all: ItemMap = BTreeMap::from([(1, item.clone()), (2, done)]);
        let (html, _) = page(&item, &all, None);
        assert!(
            html.contains("<li class=\"step done\" id=\"step-a\" data-state=\"done\" hidden><button class=\"step-head\" type=\"button\" aria-expanded=\"false\"><span class=\"num\">01</span><span class=\"title\">First</span><span class=\"meta\"><span class=\"dot done\"></span>done \u{b7} task 2</span></button><div class=\"more\"><p>Why it comes first.</p>\n</div></li>"),
            "{html}"
        );
        assert!(html.contains("<span class=\"num\">02</span><span class=\"title\">Second</span><span class=\"meta\"><span class=\"dot proposed\"></span>to approve \u{b7} after a</span></button><div class=\"after\">"), "{html}");
        assert!(html.contains("</div><div class=\"more\"><p class=\"when\"><b>Done when</b> It ships.</p></div></li>"), "{html}");
        let bare = artifact_item(1, &plan("Ship"), steps(&[], &[spec("c", Some("Third"), &[])]).unwrap().0);
        let (html, _) = page(&bare, &BTreeMap::from([(1, bare.clone())]), None);
        assert!(html.contains("<li class=\"step proposed\" id=\"step-c\" data-state=\"to approve\"><div class=\"step-head\">"), "nothing more to show, nothing to open: {html}");
        assert!(!html.contains("<div class=\"more\">"), "{html}");
    }

    #[test]
    fn the_steps_put_one_on_a_stage() {
        let mut made = steps(&[], &[spec("a", Some("First\nWhy."), &[]), spec("b", Some("Second\nHow."), &["a"]), spec("c", Some("Third"), &["a", "b"])]).unwrap().0;
        let mut done = Item::new_task(2, "First".into(), vec![], 1);
        State::Done.write(&mut done);
        made[0].task = done.uid.clone();
        let pending = Item::new_task(3, "Second".into(), vec![], 1);
        made[1].task = pending.uid.clone();
        let item = artifact_item(1, &plan("Ship"), made);
        let (html, _) = page(&item, &BTreeMap::from([(1, item.clone()), (2, done), (3, pending)]), None);
        let scene = html.split("<section class=\"scene part staged\" id=\"steps\"").nth(1).and_then(|scene| scene.split("</section>").next()).unwrap_or_default();
        // A segment a step; the one on the stage, the first not done, is
        // the current one and the way into the segments.
        assert!(
            scene.contains("<div class=\"segs\" role=\"group\" aria-label=\"Steps\"><button class=\"seg done\" type=\"button\" tabindex=\"-1\"><span>01 First</span></button><button class=\"seg pending\" type=\"button\" aria-current=\"step\"><span>02 Second</span></button><button class=\"seg proposed\" type=\"button\" tabindex=\"-1\"><span>03 Third</span></button></div><ol class=\"steps\" tabindex=\"0\" aria-label=\"Steps, one at a time\">"),
            "{scene}"
        );
        // It alone shows, and open; the steps it waits on are chips that
        // lead to them.
        assert!(scene.contains("<li class=\"step done\" id=\"step-a\" data-state=\"done\" hidden><button class=\"step-head\" type=\"button\" aria-expanded=\"false\">"), "{scene}");
        assert!(
            scene.contains("<li class=\"step pending open\" id=\"step-b\" data-state=\"pending\"><button class=\"step-head\" type=\"button\" aria-expanded=\"true\"><span class=\"num\">02</span><span class=\"title\">Second</span><span class=\"meta\"><span class=\"dot pending\"></span>pending \u{b7} task 3 \u{b7} after a</span></button><div class=\"after\"><button class=\"pill chip\" type=\"button\" data-step=\"a\"><i aria-hidden=\"true\">\u{2196}</i> after 01 a</button></div><div class=\"start writes-only\"><button class=\"pill\" type=\"button\" data-start=\"b\">Start</button><span class=\"status\" role=\"status\"></span></div><div class=\"more\"><p>How.</p>\n</div></li>"),
            "{scene}"
        );
        assert!(
            scene.contains("<li class=\"step proposed\" id=\"step-c\" data-state=\"to approve\" hidden><div class=\"step-head\"><span class=\"num\">03</span><span class=\"title\">Third</span><span class=\"meta\"><span class=\"dot proposed\"></span>to approve \u{b7} after a, b</span></div><div class=\"after\"><button class=\"pill chip\" type=\"button\" data-step=\"a\"><i aria-hidden=\"true\">\u{2196}</i> after 01 a</button><button class=\"pill chip\" type=\"button\" data-step=\"b\"><i aria-hidden=\"true\">\u{2196}</i> after 02 b</button></div></li></ol>"),
            "{scene}"
        );
        // Under the stage: where it is, the switch to the list, back and on.
        assert!(
            scene.ends_with("</ol><div class=\"controls\"><span class=\"count\">02 / 03</span><span class=\"grow\"></span><button class=\"toggle\" type=\"button\" role=\"switch\" aria-checked=\"false\"><i aria-hidden=\"true\"></i><span>All steps</span></button><button class=\"round\" type=\"button\" data-by=\"-1\" aria-label=\"Back\"><svg viewBox=\"0 0 24 24\" aria-hidden=\"true\"><path d=\"M14.5 5.5L8 12l6.5 6.5\"/></svg></button><button class=\"round\" type=\"button\" data-by=\"1\" aria-label=\"Next\"><svg viewBox=\"0 0 24 24\" aria-hidden=\"true\"><path d=\"M9.5 5.5L16 12l-6.5 6.5\"/></svg></button></div>"),
            "{scene}"
        );

        // Every step done: the first is on the stage, with no way back.
        let mut made = steps(&[], &[spec("x", Some("X"), &[]), spec("y", Some("Y"), &[])]).unwrap().0;
        let (mut one, mut two) = (Item::new_task(2, "X".into(), vec![], 1), Item::new_task(3, "Y".into(), vec![], 1));
        State::Done.write(&mut one);
        State::Done.write(&mut two);
        made[0].task = one.uid.clone();
        made[1].task = two.uid.clone();
        let item = artifact_item(1, &plan("Ship"), made);
        let (html, _) = page(&item, &BTreeMap::from([(1, item.clone()), (2, one), (3, two)]), None);
        assert!(html.contains("<button class=\"seg done\" type=\"button\" aria-current=\"step\"><span>01 X</span></button>") && html.contains("<span class=\"count\">01 / 02</span>"), "{html}");
        assert!(html.contains("aria-label=\"Back\" disabled><svg") && !html.contains("aria-label=\"Next\" disabled>"), "{html}");

        // One step is no stage.
        let item = artifact_item(1, &plan("Ship"), steps(&[], &[spec("x", Some("X"), &[])]).unwrap().0);
        let (html, _) = page(&item, &BTreeMap::from([(1, item.clone())]), None);
        assert!(html.contains("<section class=\"scene part\" id=\"steps\"") && !html.contains("class=\"segs\""), "{html}");
    }

    #[test]
    fn a_pending_step_renders_folded_like_any_other() {
        let mut made = steps(&[], &[spec("a", Some("First\nWhy it comes first."), &[])]).unwrap().0;
        let pending = Item::new_task(2, "First".into(), vec![], 1);
        made[0].task = pending.uid.clone();
        let item = artifact_item(1, &plan("Ship"), made);
        let (html, _) = page(&item, &BTreeMap::from([(1, item.clone()), (2, pending)]), None);
        assert!(html.contains("<li class=\"step pending\" id=\"step-a\" data-state=\"pending\"><button class=\"step-head\" type=\"button\" aria-expanded=\"false\">"), "{html}");
        assert!(html.contains("<span class=\"dot pending\"></span>pending \u{b7} task 2"), "{html}");
        // "open" is the class that unfolds a step: no state may be named so.
        for state in ["pending", "in progress", "done", "cancelled", "waiting", "paused", "anything else"] {
            assert_ne!(state_class(state), "open", "{state}");
        }
    }

    #[test]
    fn a_step_says_its_text_by_the_plans_markdown_rules() {
        let mut made = steps(&[], &[spec("a", Some("First\nReads the `Rests on:` line, *once*.\n\n- <b>raw</b> stays words\n- [a link](javascript:alert(1)) its words"), &[])]).unwrap().0;
        made[0].done_when = Some("Tests for `each` anchor.".to_string());
        let item = artifact_item(1, &plan("Ship"), made);
        let (html, _) = page(&item, &BTreeMap::from([(1, item.clone())]), None);
        let more = html.split("<div class=\"more\">").nth(1).and_then(|more| more.split("</div>").next()).unwrap_or_default();
        assert_eq!(
            more,
            "<p>Reads the <code>Rests on:</code> line, <em>once</em>.</p>\n<ul>\n<li>&lt;b&gt;raw&lt;/b&gt; stays words</li>\n<li>a link its words</li>\n</ul>\n<p class=\"when\"><b>Done when</b> Tests for <code>each</code> anchor.</p>",
            "{html}"
        );
        assert_eq!(inline("- a list\n- of two"), "- a list\n- of two", "what is not one paragraph is its words");
        assert_eq!(inline("a <i>tag</i> & more"), "a &lt;i&gt;tag&lt;/i&gt; &amp; more");
    }

    #[test]
    fn a_banner_says_what_waits_on_the_user_and_nothing_else_is_one() {
        let made = steps(&[], &[spec("a", Some("First"), &[])]).unwrap().0;
        let item = artifact_item(1, &plan("Ship"), made);
        let (html, _) = page(&item, &BTreeMap::from([(1, item.clone())]), None);
        assert!(!html.contains("class=\"callout\""), "a draft nobody is asked about: {html}");

        let mut asked = Item::new_note(2, "Which comes first?\nThe page or the server.".to_string(), vec!["My Board".to_string()]);
        asked.attached_to = item.uid.clone();
        asked.question = Some(serde_json::from_value(serde_json::json!({"rev": 1})).unwrap());
        let mut answered = Item::new_note(3, "Which comes last?".to_string(), vec!["My Board".to_string()]);
        answered.attached_to = item.uid.clone();
        answered.question = Some(serde_json::from_value(serde_json::json!({"rev": 1, "answer": {"text": "The docs", "at": 0, "rev": 2}})).unwrap());
        let all: ItemMap = BTreeMap::from([(1, item.clone()), (2, asked), (3, answered)]);
        let (html, _) = page(&item, &all, None);
        assert_eq!(html.matches("class=\"callout").count(), 1, "the open question only: {html}");
        assert!(
            html.contains("<div class=\"said\"><b>Waiting on you: question 2</b><p class=\"asked\">Which comes first?\nThe page or the server.</p></div><div class=\"reply writes-only\">")
                && html.contains("<p class=\"how\">Answer it <span class=\"writes-only\">here, </span>in ekko's menu, or with <code>ekko --answer 2</code> in a terminal.</p></div>"),
            "a banner, without Review, which does not answer it: {html}"
        );
        assert!(html.contains("<div class=\"note open\" id=\"note-2\" data-kind=\"Open question\">") && html.contains("<span class=\"answer\">\u{2713} The docs</span>"), "{html}");

        let mut changed = item.clone();
        let plan_now = changed.artifact.as_mut().unwrap();
        (plan_now.version, plan_now.approved_version) = (2, Some(1));
        changed.trashed = Some(1);
        let (html, _) = page(&changed, &BTreeMap::from([(1, changed.clone())]), None);
        assert!(html.contains("<div class=\"callout\"><b>Changed since it was approved</b>The plan changed after version 1 was approved: History shows how.</div>"), "{html}");
        assert!(html.contains("<div class=\"callout\"><b>In the trash</b>This artifact is in the trash.</div>"), "{html}");
        assert!(!html.contains("class=\"callout waits\""), "they wait on nobody: no banner: {html}");
    }

    #[test]
    fn the_first_scene_says_each_fact_once_and_its_title_in_two_tones() {
        let mut made = steps(&[], &[spec("a", Some("First"), &[]), spec("b", Some("Second"), &["a"]), spec("c", Some("Third"), &[])]).unwrap().0;
        let mut done = Item::new_task(2, "First".into(), vec![], 1);
        State::Done.write(&mut done);
        let open = Item::new_task(3, "Second".into(), vec![], 1);
        made[0].task = done.uid.clone();
        made[1].task = open.uid.clone();
        let mut item = artifact_item(1, &plan("Ship: the page -- at last"), made);
        let plan_now = item.artifact.as_mut().unwrap();
        (plan_now.version, plan_now.approved_version) = (3, Some(3));
        let mut note = Item::new_note(4, "Keep it light.".to_string(), vec!["My Board".to_string()]);
        note.attached_to = item.uid.clone();
        let comment = |id: u32, reply_to: Option<String>| {
            let mut note = Item::new_note(id, format!("Comment {id}"), vec!["My Board".to_string()]);
            note.attached_to = item.uid.clone();
            note.comment = Some(Box::new(crate::item::Comment {
                version: 3,
                quote: None,
                replacement: None,
                step: None,
                reply_to,
                sent: None,
                resolved: None,
                applied: None,
                theme: None,
                color: None,
                unknown: BTreeMap::new(),
            }));
            note
        };
        let first = comment(5, None);
        let reply = comment(6, first.uid.clone());
        let mut all: ItemMap = BTreeMap::from([(1, item.clone()), (2, done), (3, open), (4, note), (5, first), (6, reply), (7, comment(7, None))]);
        let hero = |all: &ItemMap| {
            let (html, _) = page(&all[&1], all, None);
            html.split("<section class=\"scene hero\"").nth(1).and_then(|rest| rest.split("<section class=\"scene ").next()).unwrap_or_default().to_string()
        };
        let shown = hero(&all);
        assert!(
            shown.contains("<h1 class=\"words\" aria-label=\"Ship: the page -- at last\"><span class=\"l1\"><span class=\"w\">Ship</span></span> <span class=\"l2\"><span class=\"w\">the</span> <span class=\"w\">page</span> <span class=\"w\">--</span> <span class=\"w\">at</span> <span class=\"w\">last</span></span></h1>"),
            "the title in two tones, cut at its first mark: {shown}"
        );
        assert!(shown.contains("<p class=\"kicker\"><span>Artifact 1</span><span class=\"state\">Approved</span></p>"), "{shown}");
        let facts = "<div class=\"facts\"><a class=\"fact\" href=\"#steps\"><b>1/2</b><span>steps done \u{b7} 1 to approve</span></a><a class=\"fact\" href=\"#notes\"><b>1</b><span>note</span></a><a class=\"fact\" href=\"#comments\"><b>3</b><span>comments</span></a><a class=\"fact\" href=\"#history\"><b>v3</b><span>the current version</span></a></div>";
        assert!(shown.contains(facts), "the plan's facts, a reply counted among the comments: {shown}");
        for fact in ["1/2", "done", "to approve", "<b>1</b>", "<b>3</b>", "v3", "version", "Approved"] {
            assert_eq!(shown.matches(fact).count(), 1, "{fact}, once: {shown}");
        }
        assert!(!shown.contains("callout"), "nothing waits on the user: no banner: {shown}");

        let mut asked = Item::new_note(8, "Approve it?".to_string(), vec!["My Board".to_string()]);
        asked.attached_to = item.uid.clone();
        asked.question = Some(serde_json::from_value(serde_json::json!({"rev": 1, "approve": {"artifact": item.uid, "version": 3, "steps": ["c"]}})).unwrap());
        all.insert(8, asked);
        let shown = hero(&all);
        assert!(shown.contains("<span class=\"state waiting\">Waiting on you</span>"), "{shown}");
        assert_eq!(shown.matches("<div class=\"callout waits\">").count(), 1, "{shown}");
        assert!(shown.contains("in a terminal.</div><button class=\"pill writes-only\" type=\"button\" data-review>Review</button></div>"), "Review beside what it answers: {shown}");
    }

    #[test]
    fn the_page_draws_with_the_fonts_written_beside_it() {
        let names: Vec<&str> = font_files().iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names.len(), 3);
        for (name, stem) in names.iter().zip(["inter-", "ekko-serif-", "ekko-serif-italic-"]) {
            let hash = name.strip_prefix(stem).and_then(|rest| rest.strip_suffix(".woff2")).unwrap_or_else(|| panic!("{name}"));
            assert!(hash.len() == 8 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()), "named by its hash: {name}");
        }
        let faces = font_faces();
        for (name, rule) in names.iter().zip(["\"Inter\"; src: url(\"fonts/", "\"Ekko Serif\"; src: url(\"fonts/", "\"Ekko Serif\"; src: url(\"fonts/"]) {
            assert!(faces.contains(&format!("{rule}{name}\") format(\"woff2\")")), "{name}: {faces}");
        }
        assert!(faces.contains("font-style: italic") && faces.matches("font-weight: 400 700").count() == 3, "{faces}");
        let item = artifact_item(1, &plan("Ship"), Vec::new());
        let (html, _) = page(&item, &BTreeMap::from([(1, item.clone())]), None);
        assert!(html.contains(&format!("<style>\n{faces}")), "the page's style opens with them: {html}");

        let dir = crate::paths::test_dir("ekko-artifact-fonts");
        let fonts = dir.join(PAGES).join(FONTS_DIR);
        write(&dir, &item, &BTreeMap::from([(1, item.clone())]), None).unwrap();
        for (name, bytes) in font_files() {
            assert_eq!(std::fs::read(fonts.join(name)).unwrap(), *bytes, "{name}");
        }
        assert_eq!(std::fs::read_to_string(fonts.join(FONT_LICENSE.0)).unwrap(), FONT_LICENSE.1);
        assert!(FONT_LICENSE.1.contains("SIL OPEN FONT LICENSE Version 1.1") && FONT_LICENSE.1.contains("Reserved Font Name"));
        // A font gone or cut short is written again by the next write, even
        // of a page that did not change; one whole is left as it is.
        std::fs::remove_file(fonts.join(names[0])).unwrap();
        std::fs::write(fonts.join(names[1]), b"short").unwrap();
        let kept = std::fs::metadata(fonts.join(names[2])).unwrap().modified().unwrap();
        write(&dir, &item, &BTreeMap::from([(1, item.clone())]), None).unwrap();
        assert_eq!(std::fs::read(fonts.join(names[0])).unwrap(), font_files()[0].1);
        assert_eq!(std::fs::read(fonts.join(names[1])).unwrap(), font_files()[1].1);
        assert_eq!(std::fs::metadata(fonts.join(names[2])).unwrap().modified().unwrap(), kept);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_page_is_written_once_per_version_and_rewritten_when_it_changes() {
        let dir = crate::paths::test_dir("ekko-artifact-page");
        let item = artifact_item(1, &plan("Ship"), steps(&[], &[spec("a", Some("First"), &[])]).unwrap().0);
        let mut all: ItemMap = BTreeMap::from([(1, item.clone())]);
        let path = write(&dir, &item, &all, None).unwrap();
        let uid = item.uid.clone().unwrap();
        assert_eq!(path, dir.join(PAGES).join(format!("{uid}.html")));
        let script = std::fs::read_to_string(dir.join(PAGES).join(format!("{uid}.js"))).unwrap();
        assert!(script.starts_with("ekkoArtifact(\""), "{script}");
        assert!(std::fs::read_to_string(&path).unwrap().contains(&format!("script.src = \"{uid}.js?t=\"")));

        let stamp = std::fs::metadata(&path).unwrap().modified().unwrap();
        refresh(&dir, &all, None);
        assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), stamp, "nothing changed, nothing is written");

        all.get_mut(&1).unwrap().description = plan("Ship today");
        refresh(&dir, &all, None);
        assert!(std::fs::read_to_string(&path).unwrap().contains("<h1 class=\"words\"><span class=\"w\">Ship</span> <span class=\"w\">today</span></h1>"));
        assert_ne!(std::fs::read_to_string(dir.join(PAGES).join(format!("{uid}.js"))).unwrap(), script);
        let listed = |dir: PathBuf| -> Vec<String> { std::fs::read_dir(dir).unwrap().flatten().map(|entry| entry.file_name().to_string_lossy().into_owned()).collect() };
        let mut left = listed(dir.join(PAGES));
        left.sort();
        assert_eq!(left, [format!("{uid}.html"), format!("{uid}.js"), FONTS_DIR.to_string()], "no temp file is left");
        assert_eq!(listed(dir.join(PAGES).join(FONTS_DIR)).len(), 4, "nor among the fonts");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_version_is_the_hash_of_the_whole_page_but_itself() {
        // Every byte of the page, the theme, style and scripts an ekko
        // writes as much as the plan, so a new ekko's page reloads too.
        let item = artifact_item(1, &plan("Ship"), steps(&[], &[spec("a", Some("First"), &[])]).unwrap().0);
        let (html, version) = page(&item, &BTreeMap::from([(1, item.clone())]), None);
        assert_eq!(html.matches(&version).count(), 1, "{version}: {html}");
        let (before, after) = html.split_once(&version).unwrap();
        assert!(before.ends_with("var version = \"") && before.contains(THEME) && before.contains(STYLE) && before.contains(SCRIPT));
        assert_eq!(version, format!("{:016x}", fnv(format!("{before}{after}").as_bytes())));
    }

    /// The approval end to end on a board (task 1019): asked by a session,
    /// about the artifact, with ekko's list of the tasks under the session's
    /// explanation; answered by the user in the menu, whose first answer,
    /// with a note or without, makes the tasks -- and nothing else does.
    mod board {
        use super::*;
        use crate::ekko::{Ekko, EkkoError};
        use crate::holder::{test_sessions, Actor};
        use crate::ops::{ArtifactSpec, Draft, Inquiry, Op, Ref};
        use crate::storage::Storage;
        use serde_json::{json, Value};

        fn ekko_at(dir: &Path, actor: &Actor) -> Ekko {
            Ekko::new(Storage::new(dir).unwrap()).acting_as(actor.clone())
        }

        fn artifact(ekko: &Ekko, spec: Value) -> Result<u32, EkkoError> {
            let mut draft = Draft::open(ekko)?;
            let id = draft.artifact(&serde_json::from_value::<ArtifactSpec>(spec).unwrap())?;
            draft.commit(false)?;
            Ok(id)
        }

        fn apply(ekko: &Ekko, op: Value) -> Result<Vec<String>, EkkoError> {
            let mut draft = Draft::open(ekko)?;
            draft.apply(&serde_json::from_value::<Op>(op).unwrap())?;
            Ok(draft.commit(false)?.notices)
        }

        fn ask(ekko: &Ekko, home: &Path, target: u32) -> Result<u32, EkkoError> {
            let inquiry: Inquiry = serde_json::from_value(json!({
                "text": "Aprovo o plano?",
                "approve": target,
                "explain": "O que a aprovação faz.",
                "options": [
                    {"label": "Aprovar", "recommended": true, "why": "as tarefas nascem", "example": "uma tarefa por passo"},
                    {"label": "Ainda não", "why": "nada muda", "example": "o plano segue como está"}
                ]
            }))
            .unwrap();
            let mut draft = Draft::open(ekko)?;
            let (id, _) = draft.ask_inquiry(&inquiry, None, home)?;
            draft.commit(false)?;
            Ok(id)
        }

        fn answer(ekko: &Ekko, question: u32, text: &str) -> Result<Vec<String>, EkkoError> {
            let mut draft = Draft::open(ekko)?;
            draft.answer(&Ref::Id(question), text)?;
            Ok(draft.commit(false)?.notices)
        }

        #[test]
        fn the_users_first_answer_makes_tasks_of_the_steps_and_nothing_else_does() {
            let home = crate::paths::test_dir("ekko-artifact-approve");
            let dir = home.join(".ekko");
            let (session, _, _) = test_sessions();
            let (as_session, as_user) = (ekko_at(&dir, &session), ekko_at(&dir, &Actor::person()));
            let steps = json!([
                {"key": "field", "text": "The field\nWhere it is stored.", "done_when": "it is stored"},
                {"key": "tool", "text": "The tool", "after": ["field"]},
                {"key": "page", "text": "The page", "after": ["field"]}
            ]);
            let target = artifact(&as_session, json!({"text": plan("Ship the page"), "steps": steps})).unwrap();
            assert!(artifact(&as_session, json!({"text": "Ship\n## Goal"})).unwrap_err().to_string().contains("headings"));
            assert!(artifact(&as_session, json!({"artifact": target, "text": "x"})).unwrap_err().to_string().contains("edit"));
            let board = || as_user.storage.get().unwrap();
            let made = || board()[&target].artifact.as_ref().unwrap().steps.iter().filter(|step| step.task.is_some()).count();
            let drafted = crate::agent::prime(&as_user, "test").unwrap().text();
            assert!(drafted.contains(&format!("Artifacts, open (1)\n{target:>4}. Ship the page \u{b7} draft, 3 steps")), "{drafted}");
            assert!(!drafted.contains("Ready, best first"), "its line among the ready work gives way to its own: {drafted}");

            let first = ask(&as_session, &home, target).unwrap();
            let asked = &board()[&first];
            assert_eq!(asked.attached_to, board()[&target].uid, "about the artifact, unless the session says what else");
            let text = &asked.description;
            assert!(text.contains("O que a aprovação faz.\n\nThe approval proposed: artifact"), "{text}");
            assert!(text.contains("- field: The field\n- tool: The tool (after field)\n- page: The page (after field)"), "{text}");
            assert!(ask(&as_session, &home, target).unwrap_err().to_string().contains("asks to approve artifact"), "one question at a time");
            assert!(answer(&as_session, first, "Aprovar").is_err(), "a session's answer makes nothing");
            let callouts = || {
                let data = board();
                let (html, _) = page(&data[&target], &data, None);
                html.split("<div class=\"callout waits\"><div class=\"said\">").skip(1).map(|rest| rest.split("</div>").next().unwrap_or_default().to_string()).collect::<Vec<_>>()
            };
            let waiting = format!(
                "<b>Waiting on you: question {first}</b>It asks you to approve this plan: answer it in ekko's menu<span class=\"writes-only\">, with Review on this page</span>, or with <code>ekko --answer {first}</code> in a terminal."
            );
            assert_eq!(callouts(), [waiting], "the approval asked, once, though the question is attached to the artifact too");
            let shown = || {
                let data = board();
                page(&data[&target], &data, None).0
            };
            assert!(shown().contains("<span class=\"state waiting\">Waiting on you</span>") && shown().contains(&format!("<b>Waiting on you: question {first}</b>")) && shown().contains(" \u{b7} written by a session, "), "{}", shown());
            answer(&as_user, first, "Ainda não").unwrap();
            assert_eq!(callouts(), Vec::<String>::new(), "answered, it waits no more");
            assert_eq!(made(), 0, "the second answer");
            let second = ask(&as_session, &home, target).unwrap();
            answer(&as_user, second, APPROVE).unwrap();
            assert_eq!(made(), 0, "ekko's own word, which the question did not offer");

            let third = ask(&as_session, &home, target).unwrap();
            apply(&as_session, json!({"op": "edit", "item": target, "append": "\nOne more line."})).unwrap();
            assert_eq!(board()[&target].artifact.as_ref().unwrap().version, 2, "edit made a new version");
            let notices = answer(&as_user, third, "Aprovar").unwrap();
            assert!(notices.iter().any(|notice| notice.contains("changed after question")), "{notices:?}");
            assert_eq!(made(), 0, "the plan changed after it was asked");

            let fourth = ask(&as_session, &home, target).unwrap();
            let notices = answer(&as_user, fourth, "Aprovar \u{2014} note: start with the field").unwrap();
            assert!(notices.iter().any(|notice| notice.contains("plan is approved")), "{notices:?}");
            let data = board();
            let plan_now = data[&target].artifact.as_ref().unwrap();
            assert_eq!(plan_now.approved_version, Some(2));
            let task = |key: &str| {
                let uid = plan_now.steps.iter().find(|step| step.key == key).unwrap().task.clone().unwrap();
                data.values().find(|item| item.uid.as_deref() == Some(uid.as_str())).unwrap().clone()
            };
            let (field, tool, page) = (task("field"), task("tool"), task("page"));
            assert!(field.description.starts_with("The field\nWhere it is stored.\nDone when: it is stored\nStep field of artifact"), "{}", field.description);
            assert_eq!(tool.blocked_by, Some(vec![field.uid.clone().unwrap()]));
            assert_eq!(page.blocked_by, Some(vec![field.uid.clone().unwrap()]));
            let blockers = data[&target].blocked_by.clone().unwrap();
            assert_eq!(blockers, [&field, &tool, &page].map(|task| task.uid.clone().unwrap()), "the tasks block the artifact");
            assert!(field.id < tool.id && tool.id < page.id, "made in the plan's order");
            assert_eq!(Standing::of(&data[&target], &data).unwrap().words(), "approved, 0 of 3 done");
            assert!(shown().contains("<span class=\"state\">Approved</span>") && shown().contains("<b>0/3</b><span>steps done</span>"), "{}", shown());

            let context = crate::agent::context(&as_user, &target.to_string()).unwrap().text();
            assert!(context.contains("artifact, a task pending"), "{context}");
            assert!(context.contains("plan version 2, version 2 approved: approved, 0 of 3 done"), "{context}");
            assert!(context.contains(&format!("step tool: The tool (after field) -> task {}, pending", tool.id)), "{context}");
            let of_step = crate::agent::context(&as_user, &field.id.to_string()).unwrap().text();
            assert!(of_step.contains(&format!("step field of artifact {target}")), "{of_step}");
            let prime = crate::agent::prime(&as_user, "test").unwrap().text();
            assert!(prime.contains(&format!("Artifacts, open (1)\n{target:>4}. Ship the page \u{b7} approved, 0 of 3 done")), "{prime}");
            let blocked = prime.split_once("\nBlocked (").map(|(_, rest)| rest).unwrap_or_default();
            assert!(blocked.starts_with("2)"), "tool and page, which wait on field: {prime}");
            assert!(!blocked.contains(&format!("{target:>4}. Ship the page")), "its line among the blocked work gives way to its own: {prime}");

            let steps = json!([{"key": "field"}, {"key": "tool"}, {"key": "page"}, {"key": "docs", "text": "The docs", "after": ["page"]}]);
            artifact(&as_session, json!({"artifact": target, "steps": steps})).unwrap();
            let fifth = ask(&as_session, &home, target).unwrap();
            let text = board()[&fifth].description.clone();
            assert!(text.contains("- docs: The docs (after page)") && !text.contains("- field:"), "only the steps not approved yet: {text}");
            answer(&as_user, fifth, "Aprovar").unwrap();
            let docs = {
                let data = board();
                let uid = data[&target].artifact.as_ref().unwrap().steps[3].task.clone().unwrap();
                data.values().find(|item| item.uid.as_deref() == Some(uid.as_str())).unwrap().clone()
            };
            assert_eq!(docs.blocked_by, Some(vec![page.uid.clone().unwrap()]), "after a step approved before");
            assert_eq!(board()[&target].blocked_by.as_ref().unwrap().len(), 4);
            assert!(ask(&as_session, &home, target).unwrap_err().to_string().contains("no step left"));
            std::fs::remove_dir_all(&home).ok();
        }

        /// A question that records no answer of its own -- none asks so
        /// since task 1044, but one might be read from elsewhere -- takes
        /// ekko's word, and nothing else.
        #[test]
        fn a_question_without_answers_of_its_own_takes_ekkos_word() {
            let home = crate::paths::test_dir("ekko-artifact-word");
            let dir = home.join(".ekko");
            let (session, _, _) = test_sessions();
            let (as_session, as_user) = (ekko_at(&dir, &session), ekko_at(&dir, &Actor::person()));
            let target = artifact(&as_session, json!({"text": plan("Ship"), "steps": [{"key": "one", "text": "The step"}]})).unwrap();
            let asked_before = || {
                let asked = ask(&as_session, &home, target).unwrap();
                let mut data = as_user.storage.get().unwrap();
                data.get_mut(&asked).unwrap().question.as_mut().unwrap().applies = None;
                as_user.storage.set(&data).unwrap();
                asked
            };
            let made = || as_user.storage.get().unwrap()[&target].artifact.as_ref().unwrap().steps[0].task.is_some();
            answer(&as_user, asked_before(), "Aprovar").unwrap();
            assert!(!made(), "the session's word, on a question that recorded none");
            answer(&as_user, asked_before(), APPROVE).unwrap();
            assert!(made());
            std::fs::remove_dir_all(&home).ok();
        }

        #[test]
        fn a_closed_artifact_is_neither_asked_about_nor_approved() {
            let home = crate::paths::test_dir("ekko-artifact-closed");
            let dir = home.join(".ekko");
            let (session, _, _) = test_sessions();
            let (as_session, as_user) = (ekko_at(&dir, &session), ekko_at(&dir, &Actor::person()));
            let target = artifact(&as_session, json!({"text": plan("Ship"), "steps": [{"key": "one", "text": "The step"}]})).unwrap();
            let asked = ask(&as_session, &home, target).unwrap();
            apply(&as_user, json!({"op": "set_state", "items": [target], "state": "cancelled"})).unwrap();
            let notices = answer(&as_user, asked, "Aprovar").unwrap();
            assert!(notices.iter().any(|notice| notice.contains("is cancelled: no task was made")), "{notices:?}");
            assert!(as_user.storage.get().unwrap()[&target].artifact.as_ref().unwrap().steps[0].task.is_none());
            assert!(ask(&as_session, &home, target).unwrap_err().to_string().contains("nothing to approve"));
            apply(&as_session, json!({"op": "create", "text": "A plain task"})).unwrap();
            let plain = as_user.storage.get().unwrap().values().find(|item| item.description == "A plain task").unwrap().id;
            assert!(ask(&as_session, &home, plain).unwrap_err().to_string().contains("is not an artifact"));
            std::fs::remove_dir_all(&home).ok();
        }

        /// A step no task can be made of -- written by hand, or by another
        /// version -- leaves the board as the answer found it: the tasks of
        /// the steps before it go too.
        #[test]
        fn a_step_no_task_can_be_made_of_makes_no_task_of_the_others() {
            let home = crate::paths::test_dir("ekko-artifact-restore");
            let dir = home.join(".ekko");
            let (session, _, _) = test_sessions();
            let (as_session, as_user) = (ekko_at(&dir, &session), ekko_at(&dir, &Actor::person()));
            let steps = json!([{"key": "one", "text": "The step"}, {"key": "two", "text": "The other"}]);
            let target = artifact(&as_session, json!({"text": plan("Ship"), "steps": steps})).unwrap();
            let asked = ask(&as_session, &home, target).unwrap();
            let mut data = as_user.storage.get().unwrap();
            let written = &mut data.get_mut(&target).unwrap().artifact.as_mut().unwrap().steps;
            written[0].text = format!("The step, {}", "written by hand past the length of a title ".repeat(2));
            written[1].text = "x".repeat(crate::ekko::MAX_DESCRIPTION + 1);
            as_user.storage.set(&data).unwrap();
            let notices = answer(&as_user, asked, "Aprovar").unwrap();
            assert!(notices.iter().any(|notice| notice.contains("no task was made for artifact")), "{notices:?}");
            assert!(!notices.iter().any(|notice| notice.contains("first line ran")), "the task undone leaves no word of its title cut: {notices:?}");
            let data = as_user.storage.get().unwrap();
            assert!(!data.values().any(|item| item.description.starts_with("The step")), "the first step's task goes with the rest");
            assert!(data[&target].blocked_by.as_ref().is_none_or(Vec::is_empty));
            assert!(data[&asked].question.as_ref().unwrap().answer.is_some(), "the answer stays recorded");

            // Once every task can be made, the long title written by hand is
            // cut as the task is made, and the answer says so.
            let mut data = as_user.storage.get().unwrap();
            data.get_mut(&target).unwrap().artifact.as_mut().unwrap().steps[1].text = "The other".to_string();
            as_user.storage.set(&data).unwrap();
            let again = ask(&as_session, &home, target).unwrap();
            let notices = answer(&as_user, again, "Aprovar").unwrap();
            assert!(notices.iter().any(|notice| notice.contains("first line ran 95 characters")), "{notices:?}");
            assert!(notices.iter().any(|notice| notice.contains("plan is approved")), "{notices:?}");
            std::fs::remove_dir_all(&home).ok();
        }

        /// A review from the page (task 1106), as GitHub's go: it sends the
        /// person's pending comments and no one else's. Request changes
        /// answers the question asking to approve the plan with its other
        /// answer, naming the review; Approve answers with the one that
        /// approves, which makes the tasks, only once a question asks about
        /// the version the plan is at. Comment answers nothing and needs
        /// something to say; Send now sends one comment alone; a comment is
        /// written pending; and only the person reviews.
        #[test]
        fn a_review_sends_the_pending_comments_and_answers_the_approval() {
            let home = crate::paths::test_dir("ekko-artifact-review");
            let dir = home.join(".ekko");
            let (session, _, _) = test_sessions();
            let (as_session, as_user) = (ekko_at(&dir, &session), ekko_at(&dir, &Actor::person()));
            let steps = json!([{"key": "field", "text": "The field\nWhere it is stored."}, {"key": "tool", "text": "The tool", "after": ["field"]}]);
            let target = artifact(&as_session, json!({"text": plan("Ship the page"), "steps": steps})).unwrap();
            let board = || as_user.storage.get().unwrap();
            let on = Ref::Id(target);
            let pending = |sent: Option<i64>| crate::item::Comment {
                version: board()[&target].artifact.as_ref().unwrap().version,
                quote: Some(crate::item::Quote { exact: "Text.".into(), prefix: String::new(), suffix: String::new(), section: String::new(), unknown: Default::default() }),
                replacement: None,
                step: None,
                reply_to: None,
                sent,
                resolved: None,
                applied: None,
                theme: None,
                color: None,
                unknown: Default::default(),
            };
            let comment = |ekko: &Ekko, text: &str| {
                let mut draft = Draft::open(ekko).unwrap();
                let id = draft.comment(&on, text, pending(None)).unwrap();
                draft.commit(false).unwrap();
                id
            };
            let review = |ekko: &Ekko, verdict: &str, text: &str, version: u32| -> Result<(u32, Vec<String>), EkkoError> {
                let mut draft = Draft::open(ekko)?;
                let id = draft.review(&on, verdict, text, version)?;
                Ok((id, draft.commit(false)?.notices))
            };
            let sent = |id: u32| board()[&id].comment.as_ref().unwrap().sent.is_some();
            let uid = |id: u32| board()[&id].uid.clone().unwrap();
            let made = || board()[&target].artifact.as_ref().unwrap().steps.iter().filter(|step| step.task.is_some()).count();

            let refused = Draft::open(&as_user).unwrap().comment(&on, "Sent already?", pending(Some(1))).unwrap_err().to_string();
            assert!(refused.contains("written pending"), "{refused}");
            let (first, second, theirs) = (comment(&as_user, "Why text?"), comment(&as_user, "And here?"), comment(&as_session, "A session's"));
            assert!(review(&as_session, "comment", "Mine", 1).unwrap_err().to_string().contains("the user's"), "a session does not review");
            assert!(review(&as_user, "merge", "", 1).unwrap_err().to_string().contains("no verdict"));
            assert!(review(&as_user, "comment", "", 2).unwrap_err().to_string().contains("plan is at version 1"));
            assert!(review(&as_user, "approve", "", 1).unwrap_err().to_string().contains("no question asks to approve"));
            assert!(!sent(first) && !sent(second), "a refused review sends nothing");

            let asked = ask(&as_session, &home, target).unwrap();
            let (changes, _) = review(&as_user, "changes", "Split the field step.\nIt does two things.", 1).unwrap();
            let data = board();
            let answer = &data[&asked].question.as_ref().unwrap().answer.as_ref().unwrap().text;
            assert_eq!(answer, &format!("Ainda não \u{2014} note: changes requested on the artifact's page, in review {changes}: Split the field step."));
            assert!(data[&asked].question.as_ref().unwrap().answer.as_ref().unwrap().by_person(), "the person's answer");
            let note = &data[&changes];
            assert_eq!((note.description.as_str(), note.attached_to.clone()), ("Split the field step.\nIt does two things.", data[&target].uid.clone()));
            let written = note.review.as_deref().unwrap();
            assert_eq!((written.verdict.as_str(), written.version), ("changes", 1));
            assert_eq!(written.comments, [uid(first), uid(second)], "the person's pending comments, oldest first");
            assert_eq!(written.answered, Some(uid(asked)));
            assert!(note.created_by.as_ref().is_none_or(|by| by.pid.is_none()), "the person's note");
            assert!(sent(first) && sent(second) && !sent(theirs), "sent: the person's, not the session's");
            assert_eq!(made(), 0, "changes requested make nothing");
            let (html, _) = page(&data[&target], &data, None);
            assert!(html.contains(&format!("<div class=\"kind\">Review {changes} ")), "a review in Notes");
            assert!(html.contains(&format!("<p class=\"verdict\" data-verdict=\"changes\">Changes requested on version 1, sending comments {first}, {second}; it answered question {asked}.</p>")), "{html}");
            assert!(html.contains("<button class=\"bar-side writes-only\" type=\"button\" data-review aria-label=\"Review\" title=\"Review (R)\">") && html.contains("</svg><span class=\"count\"></span></button></div>"), "Review, beside the pill");

            assert!(review(&as_user, "comment", "  ", 1).unwrap_err().to_string().contains("nothing to send"), "nothing pending, nothing said");
            let (commented, _) = review(&as_user, "comment", "Looks closer.", 1).unwrap();
            assert_eq!(board()[&commented].review.as_ref().unwrap().answered, None, "Comment answers nothing");

            let third = comment(&as_user, "One more");
            let theirs_uid = uid(theirs);
            let third_uid = uid(third);
            let mut draft = Draft::open(&as_user).unwrap();
            assert!(draft.send_comment(&on, &theirs_uid).is_err(), "a session's comment is the session's to send");
            draft.send_comment(&on, &third_uid).unwrap();
            draft.commit(false).unwrap();
            assert!(sent(third), "Send now");
            let refused = Draft::open(&as_user).unwrap().send_comment(&on, &uid(third)).unwrap_err().to_string();
            assert!(refused.contains("not pending"), "{refused}");

            let stale = ask(&as_session, &home, target).unwrap();
            apply(&as_session, json!({"op": "edit", "item": target, "append": "\nOne more line."})).unwrap();
            let refused = review(&as_user, "approve", "", 2).unwrap_err().to_string();
            assert!(refused.contains(&format!("question {stale} asks to approve version 1")), "{refused}");
            let data = board();
            assert_eq!(review_data(&data[&target], &data, 2)["approve"], Value::Null, "no question asks about version 2");
            assert_eq!(review_data(&data[&target], &data, 2)["changes"]["id"], json!(stale));
            review(&as_user, "comment", "Still reading.", 2).unwrap();
            assert!(board()[&stale].question.as_ref().unwrap().answer.is_none(), "Comment leaves the question open");
            let (_, _) = review(&as_user, "changes", "Older words.", 2).unwrap();
            assert!(board()[&stale].question.as_ref().unwrap().answer.is_some(), "Request changes answers the newest question, whatever its version");
            let current = ask(&as_session, &home, target).unwrap();
            let data = board();
            let shown = review_data(&data[&target], &data, 2);
            assert_eq!(shown["approve"]["id"], json!(current));
            assert_eq!(shown["approve"]["answers"], json!("Aprovar"));
            assert_eq!(shown["approve"]["tasks"], json!(["field: The field", "tool: The tool"]));
            assert_eq!(shown["changes"]["answers"], json!("Ainda não"));
            let (approved, notices) = review(&as_user, "approve", "", 2).unwrap();
            assert!(notices.iter().any(|notice| notice.contains("plan is approved")), "{notices:?}");
            assert_eq!(made(), 2, "Approve makes the tasks");
            let data = board();
            assert_eq!(data[&current].question.as_ref().unwrap().answer.as_ref().unwrap().text, format!("Aprovar \u{2014} note: approved on the artifact's page, in review {approved}"));
            assert_eq!(data[&approved].description, "Approved version 2.", "said for the person, who wrote nothing");
            assert_eq!(review_data(&data[&target], &data, 2)["approve"], Value::Null, "answered, nothing waits");
            apply(&as_user, json!({"op": "set_state", "items": [target], "state": "cancelled"})).unwrap();
            assert!(review(&as_user, "comment", "Late.", 2).unwrap_err().to_string().contains("is cancelled"), "a closed plan takes no review");
            std::fs::remove_dir_all(&home).ok();
        }

        /// The user's feedback from the page is told once to each session
        /// working the artifact, and to no other (task 1108): a comment sent
        /// alone, then a review, told with the comments it sent and the
        /// answer it gave a question that session's ask left open; the hook
        /// wakes a session with it once. Until a session resolves them, the
        /// artifact's standing counts them.
        #[test]
        fn feedback_from_the_page_is_told_once_to_the_sessions_working_the_artifact() {
            let home = crate::paths::test_dir("ekko-artifact-told");
            let dir = home.join(".ekko");
            let (holder, asker, idle) = test_sessions();
            let (as_holder, as_asker, as_user) = (ekko_at(&dir, &holder), ekko_at(&dir, &asker), ekko_at(&dir, &Actor::person()));
            let target = artifact(&as_holder, json!({"text": plan("Ship the page"), "steps": [{"key": "one", "text": "The step"}]})).unwrap();
            apply(&as_holder, json!({"op": "set_state", "items": [target], "state": "progress"})).unwrap();
            let asked = ask(&as_asker, &home, target).unwrap();
            let board = || as_user.storage.get().unwrap();
            let uid = |id: u32| board()[&id].uid.clone().unwrap();
            let told = |actor: &Actor| crate::wake::Told::of(&home, actor.process.as_ref().unwrap());
            told(&asker).left_open(&uid(asked));
            let untold = |actor: &Actor, since: u64| crate::wake::untold(&ekko_at(&dir, actor), actor, &told(actor), since, false).unwrap();
            let words = || Standing::of(&board()[&target], &board()).unwrap().words();
            let on = Ref::Id(target);
            let comment = |text: &str| {
                let quote = crate::item::Quote { exact: "Text.".into(), prefix: String::new(), suffix: String::new(), section: String::new(), unknown: Default::default() };
                let pending = crate::item::Comment { version: 1, quote: Some(quote), replacement: None, step: None, reply_to: None, sent: None, resolved: None, applied: None, theme: None, color: None, unknown: Default::default() };
                let mut draft = Draft::open(&as_user).unwrap();
                let id = draft.comment(&on, text, pending).unwrap();
                draft.commit(false).unwrap();
                id
            };
            let send = |id: u32| {
                let mut draft = Draft::open(&as_user).unwrap();
                draft.send_comment(&on, &uid(id)).unwrap();
                draft.commit(false).unwrap();
            };

            let first = comment("Why text?");
            assert!(untold(&holder, 0).is_empty(), "a pending comment waits on the user");
            send(first);
            let alone = format!("Comment {first} from the user, sent alone from the page of artifact {target} (Ship the page), on \"Text.\": Why text?");
            assert_eq!(untold(&holder, 0), vec![alone.clone()], "the session holding the artifact");
            assert!(untold(&holder, 0).is_empty(), "once");
            assert_eq!(untold(&asker, 0), vec![alone], "the session that asked about it");
            assert!(untold(&idle, 0).is_empty(), "no other session");
            assert_eq!(words(), format!("waiting on you: question {asked}, 1 comment to resolve"));

            let (second, third) = (comment("And here?"), comment("And there?"));
            let revision = as_user.storage.get_counters().unwrap().revision;
            let mut draft = Draft::open(&as_user).unwrap();
            let changes = draft.review(&on, "changes", "Split the step.\nIt does two things.", 1).unwrap();
            draft.commit(false).unwrap();
            assert!(untold(&holder, revision + 1).is_empty(), "a session that started after it had it in its prime");
            let review = format!(
                "Review {changes} from the user, on artifact {target} (Ship the page): changes requested on version 1, answering question {asked} with \"Ainda não\", sending comments {second}, {third}. It says: Split the step. It does two things."
            );
            assert_eq!(untold(&holder, revision), vec![review.clone()], "the review, and not the comments it sent");
            assert_eq!(untold(&asker, 0), vec![review], "the answer the asker's ask left open is told within the review");
            assert!(untold(&asker, 0).is_empty(), "once");
            assert!(untold(&idle, 0).is_empty(), "no other session");
            assert_eq!(words(), "draft, 1 step, 1 review and 3 comments to resolve");

            let fourth = comment("Last one.");
            send(fourth);
            let input = json!({"hook_event_name": "FileChanged", "file_path": as_holder.storage.storage_path()}).to_string();
            assert_eq!(crate::wake::hook(&as_holder, &input, &home, "default board"), std::process::ExitCode::from(2), "the hook wakes the session");
            assert_eq!(crate::wake::hook(&as_holder, &input, &home, "default board"), std::process::ExitCode::SUCCESS, "once");
            assert!(untold(&holder, 0).is_empty(), "and its reply does not tell it again");

            let data = board();
            let mut resolved = data[&first].clone();
            resolved.comment.as_mut().unwrap().resolved = Some(1);
            let mut settled = data[&changes].clone();
            settled.review.as_mut().unwrap().resolved = Some(1);
            let mut bare = data[&changes].clone();
            let written = bare.review.as_mut().unwrap();
            (written.verdict, written.comments) = ("approve".into(), Vec::new());
            bare.description = "Approved version 1.".into();
            let mut worded = bare.clone();
            worded.description = "Approved, start with the step.".into();
            let mut theirs = data[&fourth].clone();
            theirs.created_by = Some(holder.holder(1));
            assert!(feedback(&data[&first]) && feedback(&data[&changes]) && feedback(&worded), "feedback");
            assert!(!feedback(&data[&asked]) && !feedback(&resolved) && !feedback(&settled) && !feedback(&bare) && !feedback(&theirs), "not feedback");
            std::fs::remove_dir_all(&home).ok();
        }

        /// Start on the page (task 1111): on a step whose task a session may
        /// take up now, where the page writes, it sends "Start this" on the
        /// step, which is told to the sessions working the plan, naming the
        /// task, and to no other; the page then says it was sent. Taking the
        /// task up resolves it. Only the person starts, and only a step that
        /// can start, once.
        #[test]
        fn start_on_a_ready_step_asks_the_sessions_working_the_plan_to_take_it_up() {
            let home = crate::paths::test_dir("ekko-artifact-start");
            let dir = home.join(".ekko");
            let (holder, _, idle) = test_sessions();
            let (as_holder, as_user) = (ekko_at(&dir, &holder), ekko_at(&dir, &Actor::person()));
            let steps = json!([
                {"key": "one", "text": "The field"},
                {"key": "two", "text": "The tool", "after": ["one"]},
                {"key": "three", "text": "The page"},
                {"key": "four", "text": "The docs"}
            ]);
            let target = artifact(&as_holder, json!({"text": plan("Ship the page"), "steps": steps})).unwrap();
            let board = || as_user.storage.get().unwrap();
            let start = |ekko: &Ekko, key: &str| -> Result<(u32, Vec<String>), EkkoError> {
                let mut draft = Draft::open(ekko)?;
                let id = draft.start_step(&Ref::Id(target), key)?;
                Ok((id, draft.commit(false)?.notices))
            };
            let refused = |ekko: &Ekko, key: &str| start(ekko, key).unwrap_err().to_string();
            assert!(refused(&as_user, "three").contains("step three has no task yet: approving the plan makes it"));
            let asked = ask(&as_holder, &home, target).unwrap();
            answer(&as_user, asked, "Aprovar").unwrap();
            let task = |key: &str| {
                let data = board();
                let uid = data[&target].artifact.as_ref().unwrap().steps.iter().find(|step| step.key == key).unwrap().task.clone().unwrap();
                data.values().find(|item| item.uid.as_deref() == Some(uid.as_str())).unwrap().id
            };
            let (one, two, three) = (task("one"), task("two"), task("three"));
            apply(&as_holder, json!({"op": "set_state", "items": [one], "state": "progress"})).unwrap();
            let shown = || page(&board()[&target], &board(), None).0;
            let button = |key: &str| format!("<div class=\"start writes-only\"><button class=\"pill\" type=\"button\" data-start=\"{key}\">Start</button>");
            let html = shown();
            assert!(html.contains(&button("three")) && html.contains(&button("four")), "steps a session may take up now: {html}");
            assert!(!html.contains(&button("one")) && !html.contains(&button("two")), "not one in progress, nor one waiting on it: {html}");

            assert!(refused(&as_holder, "three").contains("Start is the user's"));
            assert!(refused(&as_user, "one").contains(&format!("step one cannot start: task {one} is in progress already")));
            assert!(refused(&as_user, "two").contains(&format!("step two cannot start: task {two} waits on {one}, still open")));
            assert!(refused(&as_user, "five").contains("the plan has no step five"));
            let (note, _) = start(&as_user, "three").unwrap();
            let data = board();
            let comment = data[&note].comment.as_deref().unwrap();
            assert_eq!(data[&note].description, START);
            assert!(comment.step.as_deref() == Some("three") && comment.sent.is_some() && comment.resolved.is_none() && is_start(&data[&note]));
            assert!(refused(&as_user, "three").contains(&format!("step three was sent Start already, in comment {note}")));

            let html = shown();
            assert!(html.contains(&format!("<p class=\"start asked\">Start sent in comment {note}: waiting for a session to take it up</p>")), "{html}");
            assert!(!html.contains(&button("three")) && html.contains(&button("four")), "{html}");
            let told = |actor: &Actor| crate::wake::Told::of(&home, actor.process.as_ref().unwrap());
            let untold = |actor: &Actor| crate::wake::untold(&ekko_at(&dir, actor), actor, &told(actor), 0, false).unwrap();
            let said = format!(
                "Start from the user, on the page of artifact {target} (Ship the page): step three, task {three} (The page). Set its task in progress and take it up, which resolves comment {note}."
            );
            assert_eq!(untold(&holder), vec![said], "the session working the plan, through its step one");
            assert!(untold(&idle).is_empty(), "no other session");
            let read = crate::feedback::read(&board()[&target], &board(), Some(&holder));
            assert!(
                read.contains(&format!("Comment {note} from the user, sent alone, on step three (task {three}): Start this -- the user's Start: set its task in progress and take it up, which resolves it\n")),
                "{read}"
            );
            let words = || Standing::of(&board()[&target], &board()).unwrap().words();
            assert_eq!(words(), "approved, 0 of 4 done, 1 comment to resolve");

            let notices = apply(&as_holder, json!({"op": "set_state", "items": [three], "state": "progress"})).unwrap();
            assert!(notices.contains(&format!("comment {note}, the user's Start, is resolved: task {three} is in progress")), "{notices:?}");
            assert!(board()[&note].comment.as_ref().unwrap().resolved.is_some());
            assert_eq!(words(), "approved, 0 of 4 done");
            let html = shown();
            assert!(!html.contains("data-start=\"three\"") && !html.contains("class=\"start asked\""), "{html}");
            let notices = apply(&as_holder, json!({"op": "set_state", "items": [one], "state": "done"})).unwrap();
            assert!(!notices.iter().any(|notice| notice.contains("the user's Start")), "said once: {notices:?}");
            apply(&as_holder, json!({"op": "set_state", "items": [three], "state": "unstarted"})).unwrap();
            let (again, _) = start(&as_user, "three").expect("a Start resolved waits on nothing: the step starts again");
            assert_ne!(again, note);
            // Paused, a Start waits on; every state past pending and paused
            // takes it up and resolves it.
            let notices = apply(&as_holder, json!({"op": "set_state", "items": [three], "state": "paused"})).unwrap();
            assert!(!notices.iter().any(|notice| notice.contains("the user's Start")) && board()[&again].comment.as_ref().unwrap().resolved.is_none(), "{notices:?}");
            let mut sent = again;
            for (state, word) in [("waiting", "waiting"), ("progress", "in progress"), ("done", "done"), ("cancelled", "cancelled")] {
                let notices = apply(&as_holder, json!({"op": "set_state", "items": [three], "state": state})).unwrap();
                assert!(notices.contains(&format!("comment {sent}, the user's Start, is resolved: task {three} is {word}")), "{state}: {notices:?}");
                apply(&as_holder, json!({"op": "set_state", "items": [three], "state": "unstarted"})).unwrap();
                sent = start(&as_user, "three").expect("resolved, the step starts again").0;
            }

            apply(&as_user, json!({"op": "set_state", "items": [target], "state": "cancelled"})).unwrap();
            assert!(refused(&as_user, "four").contains(&format!("artifact {target} is cancelled")));
            std::fs::remove_dir_all(&home).ok();
        }

        #[test]
        fn a_page_opened_follows_every_write_that_changes_what_it_shows() {
            let home = crate::paths::test_dir("ekko-artifact-follows");
            let dir = home.join(".ekko");
            let as_user = ekko_at(&dir, &Actor::person());
            let steps = json!([{"key": "one", "text": "The first step"}]);
            let target = artifact(&as_user, json!({"text": plan("Ship"), "steps": steps})).unwrap();
            let data = as_user.storage.get().unwrap();
            let path = write(&dir, &data[&target], &data, None).unwrap();
            assert!(std::fs::read_to_string(&path).unwrap().contains("<span class=\"state\">Draft</span>"));
            apply(&as_user, json!({"op": "edit", "item": target, "append": "\nAnd a line more."})).unwrap();
            let page = std::fs::read_to_string(&path).unwrap();
            assert!(page.contains("And a line more.") && page.contains("data-version=\"2\""), "an edit through any path rewrites it: {page}");
            apply(&as_user, json!({"op": "create", "text": "Unrelated"})).unwrap();
            assert_eq!(std::fs::read_to_string(&path).unwrap(), page, "a write that changes nothing it shows leaves it");
            std::fs::remove_dir_all(&home).ok();
        }

        /// The questions about a plan or about a step's task, on its page
        /// (task 1110): each as ekko's menu shows it, the board keeping the
        /// why, example and preview of its options for it, in the order they
        /// were asked; answered there by the person, as the menu records an
        /// answer. The approval is Review's, and a question about anything
        /// else is not the page's to answer.
        #[test]
        fn the_page_shows_each_question_about_the_plan_as_the_menu_does_and_answers_it() {
            let home = crate::paths::test_dir("ekko-artifact-questions");
            let dir = home.join(".ekko");
            let (session, _, _) = test_sessions();
            let (as_session, as_user) = (ekko_at(&dir, &session), ekko_at(&dir, &Actor::person()));
            let steps = json!([{"key": "field", "text": "The field"}, {"key": "tool", "text": "The tool", "after": ["field"]}]);
            let target = artifact(&as_session, json!({"text": plan("Ship the page"), "steps": steps})).unwrap();
            let board = || as_user.storage.get().unwrap();
            let on_page = |question: u32, picked: &[&str], other: Option<&str>, note: Option<&str>| -> Result<String, EkkoError> {
                let uid = board()[&question].uid.clone().unwrap();
                let picked: Vec<String> = picked.iter().map(|label| label.to_string()).collect();
                let mut draft = Draft::open(&as_user)?;
                let id = draft.answer_on_page(&Ref::Id(target), &uid, &picked, other, note)?;
                let committed = draft.commit(false)?;
                Ok(committed.data[&id].question.as_ref().and_then(|question| question.answer.as_ref()).unwrap().text.clone())
            };
            let approval = ask(&as_session, &home, target).unwrap();
            let proposing = board()[&approval].question.as_ref().unwrap().aids.clone();
            assert_eq!(proposing.iter().map(|aid| aid.why.as_deref().unwrap_or_default()).collect::<Vec<_>>(), ["as tarefas nascem", "nada muda"], "a question that proposes keeps them too");
            let refused = on_page(approval, &["Aprovar"], None, None).unwrap_err().to_string();
            assert!(refused.contains(&format!("question {approval} asks to approve a plan: Review answers it")), "{refused}");
            answer(&as_user, approval, "Aprovar").unwrap();
            let tool = board()[&target].artifact.as_ref().unwrap().steps[1].task.clone().expect("the approval made the step's task");
            let asked = |about: Ref, question: Value| -> u32 {
                let mut draft = Draft::open(&as_session).unwrap();
                let (id, _) = draft.ask_inquiry(&serde_json::from_value::<Inquiry>(question).unwrap(), Some(&about), &home).unwrap();
                draft.commit(false).unwrap();
                id
            };
            let which = asked(
                Ref::Id(target),
                json!({"text": "Which store?", "explain": "Where the page keeps its state.\n\nIt changes how a reload behaves.", "options": [
                    {"label": "Disk", "description": "a file", "recommended": true, "why": "it outlives the browser", "example": "~/.ekko/state", "preview": "<b>state</b>\n  kept"},
                    {"label": "Memory", "why": "nothing to clean", "example": "a Map"},
                    {"label": "Nowhere"}
                ]}),
            );
            let several = asked(
                Ref::Text(tool.clone()),
                json!({"text": "Which flags?", "explain": "What the tool takes.", "multiple": true, "options": [
                    {"label": "--fast", "recommended": true, "why": "speed", "example": "ekko --fast"},
                    {"label": "--safe", "recommended": true, "why": "safety", "example": "ekko --safe"},
                    {"label": "--loud", "why": "noise", "example": "ekko --loud"}
                ]}),
            );
            let free = asked(Ref::Id(target), json!({"text": "What should the page say?", "quick": true}));
            apply(&as_user, json!({"op": "create", "text": "Other work"})).unwrap();
            let other = board().values().find(|item| item.description == "Other work").unwrap().id;
            let elsewhere = asked(Ref::Id(other), json!({"text": "And this?", "quick": true}));

            // The board keeps each option's aids beside the note's text, for
            // the options that have any, and nothing for a question without.
            let data = board();
            let kept = &data[&which].question.as_ref().unwrap().aids;
            assert_eq!(kept.iter().map(|aid| aid.label.as_str()).collect::<Vec<_>>(), ["Disk", "Memory"], "{kept:?}");
            assert_eq!(kept[0].preview.as_deref(), Some("<b>state</b>\n  kept"));
            let stored = serde_json::to_value(&data[&free]).unwrap();
            assert_eq!(stored["question"].get("aids"), None, "a question without aids is stored as before: {stored}");
            let posed = crate::menu::posed(&data[&which]);
            assert_eq!(posed.options[0].why.as_deref(), Some("it outlives the browser"), "ekko --answer shows what the menu showed");
            assert_eq!((posed.options[1].example.as_deref(), posed.options[2].why.as_deref()), (Some("a Map"), None));

            let (html, _) = page(&data[&target], &data, None);
            let uid = |id: u32| data[&id].uid.clone().unwrap();
            assert!(!html.contains(&format!("Waiting on you: question {approval}</b>")), "the approval answered waits no more: {html}");
            assert!(
                html.contains(&format!(
                    "<div class=\"callout waits asks\" id=\"question-{which}\" data-question=\"{}\"><div class=\"said\"><b>Waiting on you: question {which}</b><p class=\"asked\">Which store?</p><p class=\"explain\">Where the page keeps its state.</p><p class=\"explain\">It changes how a reload behaves.</p></div>",
                    uid(which)
                )),
                "the question and its explanation: {html}"
            );
            assert!(
                html.contains("<button class=\"option\" type=\"button\" role=\"radio\" aria-checked=\"true\" data-n=\"0\" data-label=\"Disk\"><span class=\"n\">1</span><span class=\"label\">Disk<span class=\"tag\">recommended</span></span><span class=\"desc\">a file</span></button>")
                    && html.contains("<button class=\"option\" type=\"button\" role=\"radio\" aria-checked=\"false\" data-n=\"1\" data-label=\"Memory\" tabindex=\"-1\"><span class=\"n\">2</span><span class=\"label\">Memory</span></button>"),
                "the options numbered, the recommended one marked and picked to begin with: {html}"
            );
            assert!(
                html.contains("<div class=\"aid\" data-for=\"0\"><p class=\"why\">it outlives the browser</p><p class=\"example\"><span>Example:</span> ~/.ekko/state</p><pre class=\"preview\">&lt;b&gt;state&lt;/b&gt;\n  kept</pre></div><div class=\"aid\" data-for=\"1\" hidden><p class=\"why\">nothing to clean</p><p class=\"example\"><span>Example:</span> a Map</p></div><div class=\"aid\" data-for=\"2\" hidden><p class=\"none\">(no preview)</p></div>"),
                "beside them the aids, the recommended option's shown, a preview as written: {html}"
            );
            assert!(
                html.contains(&format!("<b>Waiting on you: question {several} \u{b7} about step tool</b><p class=\"asked\">Which flags?</p><p class=\"explain\">What the tool takes.</p><p class=\"any\">Any number of them</p>"))
                    && html.contains("role=\"checkbox\" aria-checked=\"true\" data-n=\"1\" data-label=\"--safe\"")
                    && html.contains("role=\"checkbox\" aria-checked=\"false\" data-n=\"2\" data-label=\"--loud\""),
                "a step's question, its recommended options picked: {html}"
            );
            assert!(
                html.contains(&format!("id=\"question-{free}\" data-question=\"{}\" data-free>", uid(free)))
                    && html.contains(&format!("<textarea class=\"other-text\" rows=\"2\" placeholder=\"Your answer\" aria-label=\"Your answer to question {free}\"></textarea>")),
                "a question without options is answered in words: {html}"
            );
            assert!(!html.contains(&format!("question-{elsewhere}")), "a question about other work is not the page's: {html}");
            let at = |id: u32| html.find(&format!("id=\"question-{id}\"")).unwrap();
            assert!(at(which) < at(several) && at(several) < at(free), "in the order they were asked");

            let refused = on_page(elsewhere, &[], Some("yes"), None).unwrap_err().to_string();
            assert!(refused.contains(&format!("question {elsewhere} is about neither artifact {target} nor one of its steps' tasks")), "{refused}");
            let refused = on_page(which, &["Disk", "Memory"], None, None).unwrap_err().to_string();
            assert!(refused.contains(&format!("question {which} takes one answer")), "{refused}");
            assert_eq!(on_page(which, &["Memory"], None, Some(" for now ")).unwrap(), "Memory \u{2014} note: for now");
            assert!(on_page(which, &["Disk"], None, None).unwrap_err().to_string().contains("already answered"));
            assert_eq!(on_page(several, &["--loud", "--fast"], Some("--dry"), None).unwrap(), "--fast, --loud, --dry", "in the options' order, the other last");
            assert_eq!(on_page(free, &[], Some("Say hello"), None).unwrap(), "Say hello");
            let data = board();
            let by = data[&which].question.as_ref().and_then(|question| question.answer.as_ref()).and_then(|answer| answer.by.clone()).unwrap();
            assert!(by.pid.is_none(), "the person's answer: {by:?}");
            let (html, _) = page(&data[&target], &data, None);
            assert!(!html.contains("callout waits asks"), "answered, nothing waits: {html}");
            std::fs::remove_dir_all(&home).ok();
        }
    }

    /// The opener runs outside the tree of the process that opened the page,
    /// which a session's command is in, and gets the page as one argument; an
    /// opener not found is an error (task 1103). It writes what it got, then
    /// its ancestors, once the shell that ran it had time to exit.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_opener_runs_outside_the_tree_of_the_process_that_opened_the_page() {
        let dir = crate::paths::test_dir("ekko-artifact-opener");
        std::fs::create_dir_all(&dir).unwrap();
        let opener = dir.join("opener");
        let seen = dir.join("seen");
        let script = r#"#!/bin/sh
sleep 0.5
{
  printf '%s\n' "$1"
  pid=$$
  while [ "$pid" -gt 1 ]; do
    pid=$(sed 's/.*) //' "/proc/$pid/stat" | cut -d' ' -f2)
    printf '%s\n' "$pid"
  done
} > 'SEEN.part'
mv 'SEEN.part' 'SEEN'
"#;
        crate::paths::write_executable(&opener, &script.replace("SEEN", &seen.display().to_string()));
        let page = "/a page's \"name\" & $HOME.html";
        open_with(&opener.display().to_string(), page.as_ref()).unwrap();
        let began = std::time::Instant::now();
        while !seen.exists() && began.elapsed() < std::time::Duration::from_secs(10) {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let seen = std::fs::read_to_string(&seen).unwrap();
        let (given, ancestors) = seen.split_once('\n').unwrap();
        assert_eq!(given, page);
        assert!(!ancestors.lines().any(|pid| pid == std::process::id().to_string()), "the opener is in this process's tree: {ancestors}");
        assert!(open_with("ekko-no-such-opener", page.as_ref()).is_err(), "an opener not found");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_board_whose_pages_nobody_opened_is_not_touched() {
        let dir = crate::paths::test_dir("ekko-artifact-none");
        std::fs::create_dir_all(&dir).unwrap();
        let item = artifact_item(1, &plan("Ship"), Vec::new());
        refresh(&dir, &BTreeMap::from([(1, item)]), None);
        assert!(!dir.join(PAGES).exists());
        std::fs::remove_dir_all(&dir).ok();
    }
}
