//! Artifacts (task 1019): the plan for one goal, which lives across
//! sessions. A session writes it with the MCP tool `artifact`, the user reads
//! it on a web page ekko writes, and approves it in ekko's menu, where the
//! answer makes tasks of its steps.
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

use crate::item::{Artifact, Earlier, Item, State, Step};
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
/// is a task's title; `after` naming earlier steps only, which keeps them
/// in an order the work can follow. A step the user approved is a task now:
/// it stays, as it was, and the task is where it changes; named by its key
/// alone, it is kept whole.
pub fn steps(old: &[Step], given: &[StepSpec]) -> Result<Vec<Step>, String> {
    if given.len() > STEPS_MOST {
        return Err(format!("a plan holds at most {STEPS_MOST} steps, and this one gives {}: split the goal", given.len()));
    }
    let mut made: Vec<Step> = Vec::new();
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
            let changed = text.is_some_and(|text| text != approved.text)
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
        crate::ekko::titled(text).map_err(|error| format!("step {key}: {error}"))?;
        crate::ekko::fits("step", text).map_err(|error| error.to_string())?;
        let kept = old.iter().find(|step| step.key == key).map(|step| step.unknown.clone()).unwrap_or_default();
        made.push(Step { key: key.to_string(), text: text.to_string(), done_when, after, task: None, unknown: kept });
    }
    if let Some(dropped) = old.iter().find(|step| step.task.is_some() && !made.iter().any(|kept| kept.key == step.key)) {
        return Err(format!(
            "step {} is approved, a task now: keep it among the steps, and cancel its task to drop it",
            dropped.key
        ));
    }
    Ok(made)
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
        Some(Standing {
            state,
            waiting: approval_asked(item, all).map(|question| question.id),
            steps: artifact.steps.len(),
            tasks,
            done,
            cancelled,
            proposed,
            changed: artifact.approved_version.is_some_and(|approved| artifact.version > approved),
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
        Some(Planned { version: artifact.version, approved_version: artifact.approved_version, standing: standing.words(), steps })
    }

    /// The lines context prints under the artifact's own.
    pub fn lines(&self) -> Vec<String> {
        let approved = self.approved_version.map(|version| format!(", version {version} approved")).unwrap_or_default();
        let mut lines = vec![format!("plan version {}{approved}: {}", self.version, self.standing)];
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

/// The open question asking the user to approve `item`'s plan, if one does.
pub fn approval_asked<'a>(item: &Item, all: &'a ItemMap) -> Option<&'a Item> {
    let uid = item.uid.as_deref()?;
    all.values()
        .filter(|note| note.trashed.is_none())
        .find(|note| note.question.as_ref().is_some_and(|question| question.answer.is_none() && question.approve.as_ref().is_some_and(|approve| approve.artifact == uid)))
}

/// The page of artifact `item` on the board `all`, whose project folder is
/// `folder`, and the version it carries. Its look is mockup D, which the
/// user approved (task 1092, decision 1151): the plan read as an article,
/// Medium's in the light theme and AKQA's case study in the dark (note
/// 1148), with where it stands in a column beside it; the Goal's first
/// sentence as a statement, each other section opened by its heading in
/// capitals, then the steps, the map their `after` draws, the notes and how
/// the text changed; Medium's section bars at the right edge, and AKQA's
/// bar at the bottom, which finds a section, step or note (note 1149).
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
    let tasks = steps.iter().filter(|step| step.task.is_some() && step.class != "cancelled").count();
    let done = steps.iter().filter(|step| step.class == "done").count();
    let version = artifact.map_or(0, |artifact| artifact.version);
    let command = format!("ekko artifact {}", item.id);
    let phase = standing.as_ref().map(phase);
    let by = written_by(item);
    // The page's parts, by id and name, as the section bars and the bar list them.
    let mut parts: Vec<(String, String)> = Vec::new();

    let mut body = String::new();
    let _ = write!(
        body,
        "<header class=\"top\"><span class=\"wordmark\">ekko</span><span class=\"where\">{} \u{b7} artifact {}<span id=\"who\"></span></span><span class=\"spacer\"></span><button class=\"link\" id=\"theme\" type=\"button\">Dark</button><button class=\"solid\" type=\"button\" data-copy=\"{command}\">Copy command</button></header>",
        esc(&board),
        item.id
    );

    // Where it stands, in Medium's author column beside the text.
    let _ = write!(body, "<main class=\"page\"><aside class=\"standing\"><div class=\"standing-inner\"><div class=\"mark\">{}</div><p class=\"name\">Artifact {}</p><p class=\"about\">", item.id, item.id);
    if let Some(standing) = &standing {
        let _ = write!(body, "{}.", esc(&capitalized(&standing.words())));
    }
    if let Some(by) = &by {
        let _ = write!(body, " Written by {}.", esc(by));
    }
    body.push_str("</p>");
    if !steps.is_empty() {
        body.push_str("<div class=\"segments\">");
        for (at, step) in steps.iter().enumerate() {
            let _ = write!(body, "<span class=\"{}\" title=\"{:02} {}: {}\"></span>", step.class, at + 1, esc(&step.step.key), esc(&step.state));
        }
        body.push_str("</div>");
    }
    body.push_str("</div></aside>");

    // The head of the text: tags, title, byline and actions, then what waits
    // on the user and what the user should know first.
    body.push_str("<article class=\"prose\"><ul class=\"tags\"><li>Artifact</li>");
    if let Some(phase) = phase {
        let _ = write!(body, "<li>{phase}</li>");
    }
    let progress = if tasks > 0 { format!("{done} of {tasks} done") } else { plural(steps.len(), "step") };
    let _ = write!(body, "<li>{progress}</li>");
    if let Some(priority) = item.priority.filter(|priority| *priority > 1) {
        let _ = write!(body, "<li>Priority {priority}</li>");
    }
    if !notes.is_empty() {
        let _ = write!(body, "<li>{}</li>", plural(notes.len(), "note"));
    }
    let _ = write!(body, "<li>Version {version}</li></ul><h1>{}</h1><div class=\"byline\">", esc(title));
    if let Some(by) = &by {
        let _ = write!(body, "<span class=\"who\">Written by {}</span>", esc(by));
    }
    if let Some(phase) = phase {
        let _ = write!(body, "<span class=\"state\">{phase}</span>");
    }
    let _ = write!(
        body,
        "<span>{} min read</span><span>\u{b7}</span><span>Updated {}</span></div><div class=\"actions\"><span>{}</span><span>{}</span><span>v{version}</span></div>",
        reading_minutes(plan),
        esc(&when(item.updated_at.unwrap_or(item.timestamp))),
        plural(steps.len(), "step"),
        plural(notes.len(), "note")
    );
    let approval = standing.as_ref().and_then(|standing| standing.waiting);
    if let Some(question) = approval {
        let answer = format!("It asks you to approve this plan: answer it in ekko's menu, or with <code>ekko --answer {question}</code> in a terminal.");
        callout(&mut body, &format!("Waiting on you: question {question}"), &answer);
    }
    for question in notes.iter().filter(|note| note.question.as_ref().is_some_and(|question| question.answer.is_none()) && Some(note.id) != approval) {
        let answer = format!("{}: answer it in ekko's menu, or with <code>ekko --answer {}</code> in a terminal.", esc(crate::ekko::title(&question.description)), question.id);
        callout(&mut body, &format!("Waiting on you: question {}", question.id), &answer);
    }
    if let (Some(artifact), Some(true)) = (artifact, standing.as_ref().map(|standing| standing.changed)) {
        let changed = format!("The plan changed after version {} was approved: History shows how.", artifact.approved_version.unwrap_or_default());
        callout(&mut body, "Changed since it was approved", &changed);
    }
    if item.trashed.is_some() || item.stashed.is_some() {
        let away = if item.trashed.is_some() { "in the trash" } else { "stashed" };
        callout(&mut body, &capitalized(away), &format!("This artifact is {away}."));
    }

    // The plan: the Goal's first sentence as its statement, each other
    // section opened by its heading, what comes before the first as it is.
    let mut ids = Vec::new();
    let cut = sections(plan);
    if cut.is_empty() {
        body.push_str("<p>The plan has no text yet.</p>");
    }
    let mut goal_shown = false;
    for (heading, text) in &cut {
        if heading.is_empty() {
            let _ = write!(body, "<section class=\"lead\">{}</section>", markdown(text));
            continue;
        }
        let id = unique(&mut ids, &format!("plan-{}", slug(heading)));
        if heading == "Goal" && !goal_shown {
            goal_shown = true;
            let _ = write!(body, "<section class=\"goal\" id=\"{id}\" data-part=\"Goal\">{}</section>", goal(text));
        } else {
            let _ = write!(body, "<section class=\"part\" id=\"{id}\" data-part=\"{}\"><h2 class=\"opener\">{}</h2>{}</section>", esc(heading), esc(heading), markdown(text));
        }
        parts.push((id, heading.clone()));
    }

    // Steps: AKQA's numbered items, each with its task, opening in place to
    // the rest of its text and when it is done.
    parts.push(("steps".to_string(), "Steps".to_string()));
    body.push_str("<section class=\"part\" id=\"steps\" data-part=\"Steps\"><h2 class=\"opener\">Steps</h2>");
    if steps.is_empty() {
        body.push_str("<p>No steps yet: the artifact tool writes them.</p>");
    } else {
        body.push_str("<ol class=\"steps\">");
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
            let _ = write!(body, "<li class=\"step {}\" id=\"step-{}\" data-state=\"{}\">", step.class, esc(&step.step.key), esc(&step.state));
            if step.rest.is_empty() && step.step.done_when.is_none() {
                let _ = write!(body, "<div class=\"step-head\">{head}</div></li>");
                continue;
            }
            let _ = write!(body, "<button class=\"step-head\" type=\"button\" aria-expanded=\"false\">{head}</button><div class=\"more\">");
            if !step.rest.is_empty() {
                let _ = write!(body, "<p>{}</p>", esc(step.rest));
            }
            if let Some(done_when) = &step.step.done_when {
                let _ = write!(body, "<p class=\"when\"><b>Done when</b> {}</p>", esc(done_when));
            }
            body.push_str("</div></li>");
        }
        body.push_str("</ol>");
    }
    body.push_str("</section>");

    // Map: the steps as the graph their `after` draws, on a strip wider
    // than the column, with arrows while it is wider than the window.
    if !steps.is_empty() {
        parts.push(("map".to_string(), "Map".to_string()));
        let _ = write!(
            body,
            "<section class=\"part\" id=\"map\" data-part=\"Map\"><h2 class=\"opener\">Map</h2><p>What each step waits on, from the first on the left. A step leads to its place in the list.</p><div class=\"legend\"><span><span class=\"dot proposed\"></span>to approve</span><span><span class=\"dot open\"></span>pending</span><span><span class=\"dot progress\"></span>in progress</span><span><span class=\"dot done\"></span>done</span><span>an arrow: what a step waits on</span></div><div class=\"bleed\"><div class=\"strip\">{}</div><div class=\"arrows\" hidden><button type=\"button\" data-by=\"-480\" aria-label=\"Back\">\u{2039}</button><button type=\"button\" data-by=\"480\" aria-label=\"On\">\u{203a}</button></div></div></section>",
            map(&steps)
        );
    }

    // Notes: the questions, decisions and notes attached, newest first.
    if !notes.is_empty() {
        parts.push(("notes".to_string(), "Notes".to_string()));
        body.push_str("<section class=\"part\" id=\"notes\" data-part=\"Notes\"><h2 class=\"opener\">Notes</h2>");
        for note in &notes {
            let (head, rest) = note_text(&note.description);
            let kind = note_kind(note);
            let open = if note.question.as_ref().is_some_and(|question| question.answer.is_none()) { " open" } else { "" };
            let _ = write!(
                body,
                "<div class=\"note{open}\" id=\"note-{}\" data-kind=\"{kind}\"><div class=\"kind\">{kind} {} \u{b7} {}</div><h3>{}</h3>",
                note.id,
                note.id,
                esc(&when(note.timestamp)),
                esc(head)
            );
            if let Some(answer) = note.question.as_ref().and_then(|question| question.answer.as_ref()) {
                let _ = write!(body, "<span class=\"answer\">\u{2713} {}</span>", esc(crate::menu::picked(&answer.text)));
            }
            match (rest.as_str(), note.question.is_some()) {
                ("", _) => {}
                (rest, true) => {
                    let _ = write!(body, "<details><summary>What it explained and offered</summary><p class=\"text\">{}</p></details>", esc(rest));
                }
                (rest, false) => {
                    let _ = write!(body, "<p class=\"text\">{}</p>", esc(rest));
                }
            }
            body.push_str("</div>");
        }
        body.push_str("</section>");
    }

    // History: the version against the one approved, and each earlier text
    // against the one that replaced it.
    parts.push(("history".to_string(), "History".to_string()));
    body.push_str("<section class=\"part\" id=\"history\" data-part=\"History\"><h2 class=\"opener\">History</h2>");
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
        }
        for (at, old) in earlier.iter().enumerate().rev() {
            let new = earlier.get(at + 1).map_or(item.description.as_str(), |next| next.text.as_str());
            let _ = write!(
                body,
                "<div class=\"version\" id=\"version-{}\"><div class=\"kind\">Replaced {}</div><h3>Version {} \u{2192} {}</h3><div class=\"diff\">{}</div></div>",
                old.version,
                esc(&when(old.at)),
                old.version,
                old.version + 1,
                diff_html(&old.text, new)
            );
        }
    }
    let _ = write!(
        body,
        "</section><footer class=\"foot\">Written by ekko {} from the board. It reloads by itself when the board changes, and <code>{command}</code> writes it again. Set in Inter and in Ekko Serif, Adobe's Source Serif 4 cut for this page, under the <a href=\"{FONTS_DIR}/{}\">SIL Open Font License</a>.</footer></article></main>",
        env!("CARGO_PKG_VERSION"),
        FONT_LICENSE.0
    );

    // Medium's section bars, and AKQA's bar.
    body.push_str("<nav class=\"toc\" aria-label=\"Sections\"><button class=\"toc-bars\" type=\"button\" aria-label=\"Sections\">");
    body.push_str(&"<span></span>".repeat(parts.len()));
    body.push_str("</button><div class=\"toc-card\"><ol>");
    for (id, name) in &parts {
        let _ = write!(body, "<li><a href=\"#{id}\"><span class=\"d\"></span><span class=\"t\">{}</span></a></li>", esc(name));
    }
    body.push_str("</ol></div></nav><div class=\"bar\" id=\"bar\"><div class=\"bar-inner\"><div class=\"bar-fold list-fold\"><div class=\"bar-list\" role=\"listbox\"></div><div class=\"bar-rule\"></div></div><div class=\"bar-line\"><span class=\"bar-hint\" aria-hidden=\"true\"></span><input aria-label=\"Jump to a section, step or note\" autocomplete=\"off\" spellcheck=\"false\"><button class=\"bar-menu\" type=\"button\" aria-label=\"Open menu\"><span></span><span></span></button></div><div class=\"bar-fold nav-fold\"><nav class=\"bar-nav\" aria-label=\"Parts\">");
    let (plan_parts, ours): (Vec<_>, Vec<_>) = parts.iter().partition(|(id, _)| id.starts_with("plan-"));
    if let Some((id, _)) = plan_parts.first() {
        let _ = write!(body, "<a href=\"#{id}\">Plan</a>");
    }
    for (id, name) in ours {
        let _ = write!(body, "<a href=\"#{id}\">{name}</a>");
    }
    body.push_str("</nav></div></div></div>");

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
        "\";\n  var check = function () {{\n    var script = document.createElement(\"script\");\n    script.src = \"{}.js?t=\" + Date.now();\n    script.onload = script.onerror = function () {{ script.remove(); }};\n    document.head.appendChild(script);\n  }};\n  window.ekkoArtifact = function (seen) {{\n    if (seen === version) return;\n    var field = document.activeElement;\n    if (field && (field.tagName === \"TEXTAREA\" || field.isContentEditable || (field.tagName === \"INPUT\" && !field.closest(\"#bar\")))) {{\n      field.addEventListener(\"blur\", function () {{ ekkoArtifact(seen); }}, {{ once: true }});\n      return;\n    }}\n    if (window.ekkoKeep) ekkoKeep();\n    location.reload();\n  }};\n  if (location.protocol === \"http:\" && navigator.locks && window.BroadcastChannel && window.EventSource) {{\n    var channel = new BroadcastChannel(\"ekko-events\");\n    var take = function (data) {{\n      if (data === \"open\") return check();\n      var at = data.indexOf(\" \");\n      if (data.slice(0, at) === location.pathname) ekkoArtifact(data.slice(at + 1));\n    }};\n    channel.onmessage = function (message) {{ take(message.data); }};\n    navigator.locks.request(\"ekko-events\", function () {{\n      return new Promise(function () {{\n        var source = new EventSource(\"/events\");\n        source.onopen = function () {{ channel.postMessage(\"open\"); check(); }};\n        source.addEventListener(\"version\", function (event) {{ channel.postMessage(event.data); take(event.data); }});\n      }});\n    }});\n  }} else setInterval(check, {POLL_MS});\n  if (location.protocol === \"http:\") fetch(\"/api/who\", {{ method: \"POST\" }}).then(function (answer) {{ return answer.json(); }}).then(function (who) {{\n    var text = who.person ? \" \\u00b7 writes as you\" : \" \\u00b7 reads only: \" + who.why;\n    document.getElementById(\"who\").textContent = text;\n  }}, function () {{}});\n}})();</script>\n</body>\n</html>\n",
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

/// Where an artifact stands, in the word its tag and its byline give.
fn phase(standing: &Standing) -> &'static str {
    match standing.state {
        State::Done => "Done",
        State::Cancelled => "Cancelled",
        _ if standing.waiting.is_some() => "Waiting on you",
        _ if standing.tasks == 0 => "Draft",
        _ => "Approved",
    }
}

/// `text` with its first letter a capital.
fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// The words a minute Medium's reading time counts (265).
const READING_PACE: usize = 265;

/// How many minutes the plan takes to read, at `READING_PACE`, rounded, and
/// at least one.
fn reading_minutes(plan: &str) -> usize {
    ((plan.split_whitespace().count() + READING_PACE / 2) / READING_PACE).max(1)
}

/// A box above the plan for what the user should know first: `head`, and
/// `text`, which is HTML.
fn callout(body: &mut String, head: &str, text: &str) {
    let _ = write!(body, "<div class=\"callout\"><b>{}</b>{text}</div>", esc(head));
}

/// A note's kind, as the page heads it: an open question stands apart.
fn note_kind(note: &Item) -> String {
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
    let mut sections = vec![(String::new(), String::new())];
    let mut fenced = false;
    for line in plan.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
        }
        match line.strip_prefix("## ") {
            Some(heading) if !fenced => sections.push((heading.trim().to_string(), String::new())),
            _ => {
                if let Some((_, text)) = sections.last_mut() {
                    text.push_str(line);
                    text.push('\n');
                }
            }
        }
    }
    sections.retain(|(heading, text)| !heading.is_empty() || !text.trim().is_empty());
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
            let _ = write!(out, "<path class=\"edge\" d=\"M{from_x} {from_y} C {middle} {from_y}, {middle} {to_y}, {to_x} {to_y}\" marker-end=\"url(#arrow)\"/>");
        }
    }
    out.push_str("</svg>");
    for (index, step) in steps.iter().enumerate() {
        let (x, y) = at(index);
        let task = step.task.map(|id| format!("task {id}")).unwrap_or_default();
        let _ = write!(
            out,
            "<button class=\"node {}\" data-step=\"{}\" style=\"left:{x}px;top:{y}px\"><span class=\"k\">{}</span><span class=\"t\">{}</span><span class=\"s\"><span><span class=\"dot {}\"></span>{}</span><span>{task}</span></span></button>",
            step.class,
            esc(&step.step.key),
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

/// The plan's Markdown as the events the page renders, each run of text
/// one event. Raw HTML in it is text, not run: the page is the board's, and
/// a script a plan carried would run there. For the same reason a link
/// keeps its address only when `linkable`, and is its words alone
/// otherwise. An image is a link to its source, never loaded, since the
/// page loads nothing a plan names (task 1097).
fn events(text: &str) -> Vec<pulldown_cmark::Event<'_>> {
    use pulldown_cmark::{Event, LinkType, Options, Parser, Tag, TagEnd, TextMergeStream};
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
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

/// The plan's Markdown as HTML, by the rules of `events`.
fn markdown(text: &str) -> String {
    let mut out = String::new();
    pulldown_cmark::html::push_html(&mut out, events(text).into_iter());
    out
}

/// The Goal as the page opens it: its first sentence as the statement, in
/// large serif, then the rest as it is written.
fn goal(text: &str) -> String {
    let mut out = String::new();
    match first_sentence(events(text)) {
        Ok((sentence, rest)) => {
            out.push_str("<p class=\"statement\">");
            pulldown_cmark::html::push_html(&mut out, sentence.into_iter());
            out.push_str("</p>\n");
            pulldown_cmark::html::push_html(&mut out, rest.into_iter());
        }
        Err(events) => pulldown_cmark::html::push_html(&mut out, events.into_iter()),
    }
    out
}

/// A text's events cut in two: a sentence's, and those after it.
type Split<'a> = (Vec<pulldown_cmark::Event<'a>>, Vec<pulldown_cmark::Event<'a>>);

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
fn linkable(url: &str) -> bool {
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

fn state_class(state: &str) -> &'static str {
    match state {
        "done" => "done",
        "cancelled" => "cancelled",
        "in progress" => "progress",
        _ => "open",
    }
}

/// FNV-1a, 64 bits: the page's version, which only has to change when the
/// page does, and the fonts' names.
fn fnv(bytes: &[u8]) -> u64 {
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
/// the system's.
const THEME: &str = r##"(function () { var theme = null; try { theme = localStorage.getItem("ekko-theme"); } catch (e) {} document.documentElement.dataset.theme = theme || (matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light"); })();"##;

/// What the page does: its theme toggle and copy buttons, the steps that
/// open in place, the map's arrows, Medium's section bars, AKQA's command
/// bar, and the place a reader keeps across the reload a new version makes.
const SCRIPT: &str = r##"(function () {
  "use strict";
  var root = document.documentElement;
  function smooth() { return matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth"; }

  // The theme: the one picked last, or the system's.
  var toggle = document.getElementById("theme");
  function showTheme() { toggle.textContent = root.dataset.theme === "dark" ? "Light" : "Dark"; }
  toggle.addEventListener("click", function () {
    root.dataset.theme = root.dataset.theme === "dark" ? "light" : "dark";
    try { localStorage.setItem("ekko-theme", root.dataset.theme); } catch (e) {}
    showTheme();
  });
  showTheme();

  document.querySelectorAll("[data-copy]").forEach(function (button) {
    var label = button.innerHTML;
    button.addEventListener("click", function () {
      var copied = navigator.clipboard ? navigator.clipboard.writeText(button.dataset.copy) : Promise.reject();
      copied.then(function () { button.textContent = "Copied"; }, function () { button.textContent = "Copy failed"; }).then(function () {
        setTimeout(function () { button.innerHTML = label; }, 1200);
      });
    });
  });

  // Steps open in place; a step on the map leads to its place in the list.
  function setOpen(step, open) {
    step.classList.toggle("open", open);
    var head = step.querySelector("button.step-head");
    if (head) head.setAttribute("aria-expanded", String(open));
  }
  function openStep(key) {
    var step = document.getElementById("step-" + key);
    if (!step) return;
    setOpen(step, true);
    step.scrollIntoView({ behavior: smooth(), block: "center" });
  }
  document.querySelectorAll("button.step-head").forEach(function (head) {
    head.addEventListener("click", function () { setOpen(head.parentElement, !head.parentElement.classList.contains("open")); });
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
      arrows.hidden = strip.scrollWidth <= strip.clientWidth;
      ends[0].disabled = strip.scrollLeft <= 0;
      ends[1].disabled = strip.scrollLeft + strip.clientWidth >= strip.scrollWidth - 1;
    };
    addEventListener("resize", fit);
    strip.addEventListener("scroll", fit);
    fit();
  }

  // Medium's section bars: the part being read, and a card naming them all.
  var parts = Array.prototype.slice.call(document.querySelectorAll("[data-part]"));
  var bars = Array.prototype.slice.call(document.querySelectorAll(".toc-bars span"));
  var rows = Array.prototype.slice.call(document.querySelectorAll(".toc a"));
  function jump(id) {
    var target = document.getElementById(id);
    if (target) target.scrollIntoView({ behavior: smooth(), block: "start" });
  }
  rows.forEach(function (row) {
    row.addEventListener("click", function (event) { event.preventDefault(); jump(row.getAttribute("href").slice(1)); });
  });
  var current = -1;
  function follow() {
    var at = 0;
    parts.forEach(function (part, i) { if (part.getBoundingClientRect().top < innerHeight * 0.4) at = i; });
    if (at === current) return;
    current = at;
    bars.forEach(function (bar, i) { bar.classList.toggle("on", i === at); });
    rows.forEach(function (row, i) { row.classList.toggle("on", i === at); });
  }
  var following = false;
  addEventListener("scroll", function () {
    if (following) return;
    following = true;
    requestAnimationFrame(function () { following = false; follow(); });
  }, { passive: true });
  follow();

  // AKQA's bar: a pill a click unfolds into a panel, which finds a part, a
  // step or a note of the page.
  var bar = document.getElementById("bar");
  var input = bar.querySelector("input"), hint = bar.querySelector(".bar-hint"), list = bar.querySelector(".bar-list");
  var listFold = bar.querySelector(".list-fold"), navFold = bar.querySelector(".nav-fold"), menu = bar.querySelector(".bar-menu");
  var items = parts.map(function (part) {
    return { kind: "Section", badge: "\u00a7", label: part.dataset.part, target: part.id, search: ("section " + part.dataset.part).toLowerCase() };
  });
  document.querySelectorAll(".step").forEach(function (step) {
    var key = step.id.slice(5), title = step.querySelector(".title").textContent, more = step.querySelector(".more");
    items.push({ kind: "Step \u00b7 " + step.dataset.state, badge: step.querySelector(".num").textContent, label: title, step: key,
      search: ("step steps " + key + " " + title + " " + step.querySelector(".meta").textContent + " " + (more ? more.textContent : "")).toLowerCase() });
  });
  document.querySelectorAll(".note").forEach(function (note) {
    var kind = note.dataset.kind, title = note.querySelector("h3").textContent, answer = note.querySelector(".answer");
    var text = Array.prototype.map.call(note.querySelectorAll(".text"), function (part) { return part.textContent; }).join(" ");
    items.push({ kind: kind + " " + note.id.slice(5), badge: kind.charAt(0), label: title, target: note.id,
      search: ("note notes " + kind + " " + kind + "s " + title + " " + (answer ? answer.textContent : "") + " " + text).toLowerCase() });
  });
  function find(text) {
    var words = text.toLowerCase().split(/\s+/).filter(Boolean);
    return items.filter(function (item) { return words.every(function (word) { return item.search.indexOf(word) >= 0; }); });
  }
  // The suggestions the pill cycles through, each with what it looks for:
  // one that would find nothing on this page is not offered.
  var hints = [["Steps in progress", "in progress"], ["Open questions", "open question"], ["Jump to a step", ""], ["Show the map", "map"], ["What the design says", "design"]]
    .filter(function (hint) { return hint[1] ? find(hint[1]).length > 0 : items.some(function (item) { return item.step; }); });
  if (!hints.length) hints = [["Jump to a section", ""]];
  var shown = 0, selected = 0, matches = [];

  // A suggestion comes in word by word, each from a blur, and leaves the same
  // way. Each word but the last ends in a no-break space, which the end of
  // its inline block keeps where it drops a space.
  function showHint() {
    var words = hints[shown][0].split(" ");
    hint.textContent = "";
    words.forEach(function (word, i) {
      var span = document.createElement("span");
      span.textContent = word + (i < words.length - 1 ? "\u00a0" : "");
      hint.appendChild(span);
      setTimeout(function () { span.classList.add("in"); }, 60 * i + 20);
    });
    setTimeout(function () {
      Array.prototype.forEach.call(hint.children, function (span, i) { setTimeout(function () { span.classList.remove("in"); }, 40 * i); });
      setTimeout(function () { shown = (shown + 1) % hints.length; showHint(); }, 700);
    }, 2600);
  }
  function hintVisible() { hint.style.visibility = input.value || bar.classList.contains("open") ? "hidden" : "visible"; }

  function render() {
    matches = find(input.value).slice(0, 40);
    selected = Math.min(selected, Math.max(matches.length - 1, 0));
    list.textContent = "";
    matches.forEach(function (item, i) {
      var button = document.createElement("button");
      button.type = "button";
      button.className = "item" + (i === selected ? " sel" : "");
      button.setAttribute("role", "option");
      button.innerHTML = '<span class="badge"></span><span class="what"><span class="kind"></span><span class="label"></span></span>';
      button.querySelector(".badge").textContent = item.badge;
      button.querySelector(".kind").textContent = item.kind;
      button.querySelector(".label").textContent = item.label;
      button.addEventListener("click", function () { go(item); });
      list.appendChild(button);
    });
    size();
  }

  // The heights the spring moves to: the folds' content, or nothing folded.
  function size() {
    var open = bar.classList.contains("open");
    var listHeight = open && !bar.dataset.navOnly && matches.length ? listFold.scrollHeight : 0;
    var navHeight = open ? navFold.firstElementChild.offsetHeight : 0;
    listFold.style.height = listHeight + "px";
    navFold.style.height = navHeight + "px";
    bar.style.height = 56 + listHeight + navHeight + "px";
  }

  function open(navOnly) {
    if (navOnly) bar.dataset.navOnly = "1"; else delete bar.dataset.navOnly;
    bar.classList.add("open");
    var hinted = !navOnly && !input.value;
    if (hinted) input.value = hints[shown][1];
    hintVisible();
    render();
    input.focus();
    // The hint's words, picked, so the first key typed takes their place.
    if (hinted) input.select();
  }
  function close() {
    bar.classList.remove("open");
    delete bar.dataset.navOnly;
    hintVisible();
    size();
    input.blur();
  }
  function go(item) {
    close();
    if (item.step) openStep(item.step); else jump(item.target);
  }

  bar.querySelector(".bar-line").addEventListener("click", function (event) {
    if (event.target.closest(".bar-menu")) return;
    if (!bar.classList.contains("open")) open(false);
  });
  menu.addEventListener("click", function (event) {
    event.stopPropagation();
    if (bar.classList.contains("open")) close(); else open(true);
  });
  input.addEventListener("input", function () {
    delete bar.dataset.navOnly;
    selected = 0;
    render();
  });
  input.addEventListener("keydown", function (event) {
    // Esc gives up the search: the bar folds empty, back to its hints.
    if (event.key === "Escape") { input.value = ""; close(); return; }
    if (!bar.classList.contains("open")) { if (event.key !== "Tab") open(false); return; }
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      selected = (selected + (event.key === "ArrowDown" ? 1 : -1) + matches.length) % Math.max(matches.length, 1);
      render();
      var sel = list.querySelector(".sel");
      if (sel) sel.scrollIntoView({ block: "nearest" });
    }
    if (event.key === "Enter" && matches[selected]) go(matches[selected]);
  });
  bar.querySelectorAll(".bar-nav a").forEach(function (a) {
    a.addEventListener("click", function (event) {
      event.preventDefault();
      go({ target: a.getAttribute("href").slice(1) });
    });
  });
  document.addEventListener("pointerdown", function (event) {
    if (bar.classList.contains("open") && !bar.contains(event.target)) close();
  });
  addEventListener("keydown", function (event) {
    if (event.key !== "/" || event.ctrlKey || event.metaKey || event.altKey) return;
    var active = document.activeElement;
    if (active && (active.tagName === "INPUT" || active.tagName === "TEXTAREA" || active.isContentEditable)) return;
    event.preventDefault();
    open(false);
  });
  showHint();
  hintVisible();
  size();

  // Where the reader was, kept across the reload a new version of the page
  // makes: the last part, step or note begun above the window's top and how
  // far above, the steps open, and the bar's text while it is open.
  var keptAt = "ekko-kept " + location.pathname;
  function anchor() {
    var found = null;
    document.querySelectorAll("main [id]").forEach(function (element) { if (element.getBoundingClientRect().top <= 1) found = element; });
    return found;
  }
  window.ekkoKeep = function () {
    var at = anchor();
    var kept = { y: scrollY, id: at ? at.id : "", offset: at ? at.getBoundingClientRect().top : 0,
      open: Array.prototype.map.call(document.querySelectorAll(".step.open"), function (step) { return step.id; }) };
    if (bar.classList.contains("open")) kept.bar = input.value;
    try { sessionStorage.setItem(keptAt, JSON.stringify(kept)); history.scrollRestoration = "manual"; } catch (e) {}
  };
  var kept = null;
  try { kept = JSON.parse(sessionStorage.getItem(keptAt)); sessionStorage.removeItem(keptAt); } catch (e) {}
  if (kept) {
    (kept.open || []).forEach(function (id) { var step = document.getElementById(id); if (step) setOpen(step, true); });
    var back = function () {
      var at = kept.id && document.getElementById(kept.id);
      scrollTo(0, at ? scrollY + at.getBoundingClientRect().top - kept.offset : kept.y);
      try { history.scrollRestoration = "auto"; } catch (e) {}
      if (typeof kept.bar === "string") { input.value = kept.bar; open(false); }
    };
    var settled = function () { document.fonts.ready.then(back); };
    if (document.readyState === "complete") settled(); else addEventListener("load", settled);
  }
})();"##;

/// The look of mockup D (decision 1151): Medium's article in the light
/// theme and AKQA's case study in the dark, both measured in note 1148; the
/// command bar is AKQA's and the section bars Medium's, measured in note
/// 1149. Sizes are the references' own, in px. The fonts come before it,
/// from `font_faces`.
const STYLE: &str = r##"
:root {
  --serif: "Ekko Serif", Georgia, Cambria, "Times New Roman", Times, serif;
  --sans: "Inter", "Helvetica Neue", Helvetica, Arial, sans-serif;
  --mono: ui-monospace, "SF Mono", Menlo, Consolas, "DejaVu Sans Mono", monospace;
  --column: 680px;
  /* Medium's article. */
  --page: #fff;
  --bg: #fff;
  --fg: #242424;
  --fg-strong: #000;
  --fg-2: #6b6b6b;
  --rule: #f2f2f2;
  --chip: #f2f2f2;
  --node: #fff;
  --node-line: #e6e6e6;
  --accent: #1a8917;
  --card: #fff;
  --card-shadow: 0 0 4px rgba(36, 36, 36, 0.05), 0 2px 8px rgba(36, 36, 36, 0.15);
  --bars-bg: rgba(255, 255, 255, 0.38);
  --add: #e6ffec;
  --del: #ffebe9;
  /* The bar as light glass over a light page, where AKQA's dark glass
     read as a grey block (task 1153), opaque enough to read where a
     browser draws no blur behind it; the dark theme keeps AKQA's. */
  --bar-bg: rgba(255, 255, 255, 0.88);
  --bar-edge: rgba(0, 0, 0, 0.1);
  --bar-fg: #242424;
  --bar-hint: #6b6b6b;
  --bar-muted: #6b6b6b;
  --bar-chip: rgba(0, 0, 0, 0.04);
  --bar-chip-on: rgba(0, 0, 0, 0.08);
  --bar-line: rgba(0, 0, 0, 0.1);
  /* AKQA's spring, sampled from its bar opening (note 1149): past the
     target by 14% at a third of the way, settled by 0.9 s. */
  --spring: linear(0, 0.086 3.9%, 0.168 5.8%, 0.3 8.3%, 0.604 13.8%, 0.754 16.7%, 0.893 19.9%, 0.989 22.6%, 1.061 25.4%, 1.114 28.8%, 1.132 31.1%, 1.139 33.9%, 1.136 36.3%, 1.111 41.2%, 1.089 44%, 1.068 46.6%, 1.046 49.4%, 1.029 52.1%, 1.014 54.7%, 0.996 58.6%, 0.989 61.2%, 0.982 63.6%, 0.982 67.3%, 0.986 76.8%, 0.989 80.8%, 0.996 86%, 1 90.9%, 1);
  color-scheme: light;
}

/* AKQA's case study. */
:root[data-theme="dark"] {
  --page: #000;
  --bg: #191919;
  --fg: #d9d9d9;
  --fg-strong: #fff;
  --fg-2: #bbb;
  --rule: rgba(255, 255, 255, 0.32);
  --chip: #262626;
  --node: #262626;
  --node-line: rgba(255, 255, 255, 0.16);
  --accent: #fff;
  --card: #262626;
  --card-shadow: 0 8px 40px rgba(0, 0, 0, 0.25);
  --bars-bg: rgba(0, 0, 0, 0.38);
  --add: rgba(46, 160, 67, 0.18);
  --del: rgba(248, 81, 73, 0.18);
  --bar-bg: rgba(0, 0, 0, 0.42);
  --bar-edge: transparent;
  --bar-fg: #fff;
  --bar-hint: #d9d9d9;
  --bar-muted: #bbb;
  --bar-chip: rgba(255, 255, 255, 0.04);
  --bar-chip-on: rgba(255, 255, 255, 0.16);
  --bar-line: rgba(255, 255, 255, 0.16);
  color-scheme: dark;
}

html { background: var(--page); }
body { margin: 0; overflow-x: clip; background: var(--bg); color: var(--fg); font: 400 16px/24px var(--sans); -webkit-font-smoothing: antialiased; -moz-osx-font-smoothing: grayscale; text-rendering: optimizeLegibility; }
a { color: inherit; }
button { font: inherit; color: inherit; }
[hidden] { display: none !important; }

/* ---- the top bar: Medium's, 56px and a rule ---------------------------- */
.top { display: flex; align-items: center; gap: 16px; height: 56px; padding: 0 24px; border-bottom: 1px solid var(--rule); }
.wordmark { font: 700 28px/1 var(--serif); letter-spacing: -0.04em; color: var(--fg-strong); }
.where { font: 400 14px/20px var(--sans); color: var(--fg-2); }
.top .spacer { flex: 1; }
.top .link { padding: 0; border: 0; background: none; font: 400 14px/20px var(--sans); color: var(--fg-2); cursor: pointer; }
.top .link:hover { color: var(--fg); }
.top .solid { padding: 8px 16px; border: 0; border-radius: 999px; background: var(--fg); color: var(--bg); font: 400 14px/20px var(--sans); cursor: pointer; }

/* ---- the page: Medium's column, where it stands beside it -------------- */
.page { position: relative; max-width: var(--column); margin: 0 auto; padding: 32px 24px 240px; box-sizing: content-box; }
.standing { position: absolute; top: 32px; bottom: 0; left: -232px; width: 160px; }
.standing-inner { position: sticky; top: 32px; }
.standing .mark { display: grid; place-items: center; width: 40px; height: 40px; border-radius: 50%; background: var(--chip); font: 600 13px/1 var(--sans); color: var(--fg); }
.standing .name { margin: 16px 0 0; font: 500 16px/20px var(--sans); color: var(--fg-strong); }
.standing .about { margin: 12px 0 0; font: 400 14px/20px var(--sans); color: var(--fg-2); }
.segments { display: flex; gap: 1px; margin: 16px 0 0; }
.segments span { flex: 1 1 0; min-width: 1px; height: 6px; border-radius: 1px; background: var(--fg); opacity: 0.15; }
.segments .done { opacity: 1; }
.segments .progress { opacity: 1; background: var(--accent); }

.prose .tags { display: flex; flex-wrap: wrap; gap: 8px; margin: 0; padding: 0; list-style: none; }
.prose .tags li { margin: 0; padding: 4px 12px; border-radius: 999px; box-shadow: inset 0 0 0 1px var(--rule); font: 400 13px/20px var(--sans); letter-spacing: normal; color: var(--fg); }
h1 { margin: 24px 0 0; font: 700 42px/52px var(--sans); letter-spacing: -0.011em; color: var(--fg); }
.byline { display: flex; flex-wrap: wrap; align-items: center; gap: 4px 8px; margin: 32px 0 0; font: 400 14px/20px var(--sans); color: var(--fg-2); }
.byline .who { color: var(--fg); }
.byline .state { padding: 7px 15px; border: 1px solid var(--fg); border-radius: 999px; color: var(--fg); }
.actions { display: flex; align-items: center; gap: 24px; margin: 32px 0 0; padding: 10px 8px; border-top: 1px solid var(--rule); border-bottom: 1px solid var(--rule); font: 400 13px/20px var(--sans); color: var(--fg-2); }

.callout { margin: 40px 0 0; padding: 20px 24px; border-radius: 8px; background: var(--chip); font: 400 16px/24px var(--sans); color: var(--fg); }
.callout + .callout { margin-top: 16px; }
.callout b { display: block; margin: 0 0 4px; font-weight: 600; color: var(--fg-strong); }

/* ---- the plan: AKQA's openers and statement, Medium's text ------------- */
.prose .statement { margin: 56px 0 0; font: 400 32px/40px var(--serif); letter-spacing: -0.01em; color: var(--fg-strong); }
.goal, .part { scroll-margin-top: 24px; }
section.part::before { content: ""; display: block; height: 1px; margin: 80px calc(50% - 50vw + 24px); background: var(--rule); }
.opener { margin: 0 0 40px; font: 400 56px/0.873 var(--sans); letter-spacing: -0.027em; text-transform: uppercase; overflow-wrap: anywhere; color: var(--fg-strong); }
.prose p, .prose li { font: 400 20px/32px var(--serif); letter-spacing: -0.003em; color: var(--fg); }
.prose p { margin: 2.14em 0 -0.46em; }
.prose .opener + p, .prose .statement + p { margin-top: 0.94em; }
.prose ul, .prose ol { margin: 1.4em 0 -0.46em; padding-left: 30px; }
.prose li { margin: 1.14em 0 -0.46em; padding-left: 0.3em; }
.prose li p { margin: 0; }
.prose code { padding: 2px 4px; border-radius: 3px; background: var(--chip); font: 400 0.75em/1 var(--mono); }
.prose .callout code { background: var(--bg); }
.prose strong { font-weight: 700; }
.prose a { text-decoration: underline; text-decoration-thickness: 1px; text-underline-offset: 3px; }
.prose section :is(h1, h2):not(.opener) { margin: 1.6em 0 -0.3em; font: 700 28px/34px var(--sans); letter-spacing: -0.016em; color: var(--fg-strong); }
.prose h3 { margin: 1.72em 0 -0.28em; font: 600 24px/30px var(--sans); letter-spacing: -0.016em; color: var(--fg-strong); }
.prose :is(h4, h5, h6) { margin: 1.6em 0 -0.3em; font: 600 20px/26px var(--sans); color: var(--fg-strong); }
.prose pre { margin: 2em 0 -0.46em; padding: 16px 20px; overflow-x: auto; border-radius: 4px; background: var(--chip); font: 400 14px/22px var(--mono); }
.prose pre code { padding: 0; background: none; font: inherit; }
.prose blockquote { margin: 2em 0 -0.46em; padding-left: 20px; border-left: 3px solid var(--fg); }
.prose blockquote p { margin-top: 0; font-style: italic; }
.prose table { width: 100%; margin: 2em 0 -0.46em; border-collapse: collapse; font: 400 15px/22px var(--sans); }
.prose th, .prose td { padding: 8px 10px; border-bottom: 1px solid var(--rule); text-align: left; vertical-align: top; }
.prose th { font-weight: 600; color: var(--fg-strong); }
.prose hr { height: 1px; margin: 2.5em 0 -0.46em; border: 0; background: var(--rule); }

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

/* ---- the map: AKQA's strip, wider than the column ---------------------- */
.bleed { position: relative; margin: 0 calc(50% - 50vw); }
.strip { overflow-x: auto; padding: 8px 24px 24px; scrollbar-width: none; }
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
.arrows { display: flex; justify-content: flex-end; gap: 8px; max-width: var(--column); margin: 8px auto 0; padding: 0 24px; }
.arrows button { display: grid; place-items: center; width: 40px; height: 40px; padding: 0 0 2px; border: 1px solid var(--rule); border-radius: 50%; background: var(--bg); font: 400 24px/1 var(--sans); color: var(--fg-strong); cursor: pointer; }
.arrows button:hover:not(:disabled) { background: var(--chip); }
.arrows button:disabled { opacity: 0.3; cursor: default; }
.legend { display: flex; flex-wrap: wrap; gap: 8px 20px; margin: 24px 0 0; font: 400 13px/20px var(--sans); color: var(--fg-2); }
.legend > span { display: inline-flex; align-items: center; gap: 8px; }

/* ---- notes and history ------------------------------------------------- */
.note, .version { padding: 24px 0; border-top: 1px solid var(--rule); scroll-margin-top: 24px; }
.note:last-child, .version:last-child { border-bottom: 1px solid var(--rule); }
.note .kind, .version .kind { font: 400 13px/20px var(--sans); color: var(--fg-2); }
.note.open .kind { color: var(--accent); }
.prose .note h3, .prose .version h3 { margin: 4px 0 0; font: 600 20px/24px var(--sans); letter-spacing: -0.01em; color: var(--fg-strong); }
.note .answer { display: inline-block; margin: 12px 0 0; padding: 4px 12px; border-radius: 999px; background: var(--chip); font: 400 13px/20px var(--sans); color: var(--fg); }
.prose .note .text { margin: 12px 0 0; font: 400 18px/28px var(--serif); white-space: pre-wrap; overflow-wrap: anywhere; color: var(--fg); }
.note details { margin: 12px 0 0; }
.note summary { font: 400 14px/20px var(--sans); color: var(--fg-2); cursor: pointer; }
.prose .history { font: 400 20px/32px var(--serif); color: var(--fg-2); }
.diff { margin: 12px 0 0; overflow: hidden; border-radius: 4px; background: var(--chip); font: 400 13px/20px var(--mono); }
.diff div { padding: 1px 12px; white-space: pre-wrap; overflow-wrap: anywhere; }
.diff .ctx, .diff .hunk { color: var(--fg-2); }
.diff .hunk { font-style: italic; }
.diff .add { background: var(--add); }
.diff .del { background: var(--del); }
.foot { margin: 120px 0 0; font: 400 13px/20px var(--sans); color: var(--fg-2); }

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
.toc a.on { color: var(--accent); }
.toc a.on .d { background: var(--accent); }

/* ---- AKQA's bar: a pill at the bottom, a panel once clicked ------------ */
.bar { position: fixed; bottom: 60px; left: 50%; z-index: 30; box-sizing: border-box; width: 320px; height: 56px; overflow: hidden; border-radius: 100px; background: var(--bar-bg); box-shadow: 0 0 0 1px var(--bar-edge), 0 8px 40px rgba(0, 0, 0, 0.05); color: var(--bar-fg); -webkit-backdrop-filter: blur(25px); backdrop-filter: blur(25px); transform: translateX(-50%); transition: width 0.9s var(--spring), height 0.9s var(--spring), border-radius 0.9s var(--spring); }
.bar.open { width: 600px; border-radius: 20px; }
.bar-inner { position: absolute; right: 0; bottom: 0; left: 0; display: flex; flex-direction: column; }
/* What the panel adds, folded to nothing in the pill, unfolded by the same spring. */
.bar-fold { height: 0; overflow: hidden; opacity: 0; transition: height 0.9s var(--spring), opacity 0.55s; }
.bar.open .bar-fold { opacity: 1; }
.bar-list { max-height: 327px; overflow-y: auto; padding: 8px; scrollbar-width: thin; }
.bar-list:empty { display: none; }
.bar-list .item { display: flex; align-items: center; gap: 10px; width: 100%; padding: 6px 8px; border: 0; border-radius: 8px; background: none; color: var(--bar-fg); text-align: left; cursor: pointer; }
.bar-list .item:hover, .bar-list .item.sel { background: var(--bar-chip-on); }
.bar-list .badge { display: grid; flex: none; place-items: center; width: 24px; height: 24px; border-radius: 4px; background: #000; font: 500 10px/1 var(--sans); color: #fff; }
.bar-list .what { display: flex; flex-direction: column; min-width: 0; }
.bar-list .kind { font: 400 12px/16px var(--sans); color: var(--bar-muted); }
.bar-list .label { overflow: hidden; font: 400 15px/20px var(--sans); text-overflow: ellipsis; white-space: nowrap; }
.bar-rule { height: 1px; background: var(--bar-line); }
.bar-line { position: relative; display: flex; align-items: center; min-height: 56px; padding: 0 40px 0 16px; box-sizing: border-box; font: 400 14px/20px var(--sans); }
.bar-hint { position: absolute; top: 50%; left: 16px; right: 40px; overflow: hidden; color: var(--bar-hint); white-space: nowrap; pointer-events: none; transform: translateY(-50%); }
.bar-hint span { display: inline-block; opacity: 0; transform: translateY(0.3em); transition: opacity 0.5s, transform 0.5s; }
.bar-hint span.in { opacity: 1; transform: none; }
.bar input { position: relative; z-index: 1; width: 100%; min-width: 0; padding: 0; border: 0; outline: 0; background: transparent; font: inherit; color: var(--bar-fg); caret-color: var(--bar-fg); }
.bar-menu { position: absolute; top: 50%; right: 16px; display: grid; place-items: center; width: 24px; height: 24px; padding: 0; border: 0; background: none; color: var(--bar-fg); cursor: pointer; transform: translateY(-50%); }
.bar-menu span { position: absolute; width: 16px; height: 1px; border-radius: 1px; background: currentColor; transition: transform 0.3s; }
.bar-menu span:first-child { transform: translateY(-3px); }
.bar-menu span:last-child { transform: translateY(3px); }
.bar.open .bar-menu span:first-child { transform: rotate(45deg); }
.bar.open .bar-menu span:last-child { transform: rotate(-45deg); }
.bar-nav { display: flex; flex-wrap: wrap; gap: 8px; padding: 0 16px 16px; }
.bar-nav a { padding: 6px 14px; border-radius: 999px; background: var(--bar-chip); font: 400 13px/19.5px var(--sans); letter-spacing: -0.025em; color: var(--bar-muted); text-decoration: none; transition: background-color 0.3s, color 0.3s; }
.bar-nav a:hover, .bar-nav a.on { background: var(--bar-chip-on); color: var(--bar-fg); }

/* The column at the left needs 232px beside the text, and the window
   1176px for that; a narrower one puts it above the text. */
@media (max-width: 1175px) {
  .standing { position: static; width: auto; margin: 0 0 32px; }
  .standing-inner { position: static; }
}
@media (max-width: 720px) {
  .opener { font-size: 40px; }
  h1 { font-size: 32px; line-height: 40px; }
  .bar.open { width: calc(100vw - 32px); }
  .toc { display: none; }
}
@media (prefers-reduced-motion: reduce) {
  .bar, .bar-fold, .bar-hint span, .toc-card, .toc-bars, .toc-bars span { transition: none; }
}
@media print {
  .top .link, .top .solid, .toc, .bar, .arrows { display: none; }
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
        let made = steps(&[], &[spec("a", Some("First\nwhy"), &[]), spec("b", Some("Second"), &["a"])]).unwrap();
        assert_eq!(made.iter().map(|step| (step.key.as_str(), step.after.clone())).collect::<Vec<_>>(), [("a", vec![]), ("b", vec!["a".to_string()])]);
        for (given, refusal) in [
            (vec![spec("a", Some("x"), &["b"]), spec("b", Some("y"), &[])], "not a step before it"),
            (vec![spec("a", Some("x"), &["a"])], "not a step before it"),
            (vec![spec("a", Some("x"), &[]), spec("a", Some("y"), &[])], "two steps are keyed a"),
            (vec![spec("A", Some("x"), &[])], "lower-case"),
            (vec![spec("-a", Some("x"), &[])], "lower-case"),
            (vec![spec("", Some("x"), &[])], "lower-case"),
            (vec![spec("a", None, &[])], "needs its text"),
            (vec![spec("a", Some(&"long ".repeat(20)), &[])], "title"),
        ] {
            let error = steps(&[], &given).unwrap_err();
            assert!(error.contains(refusal), "{refusal}: {error}");
        }
        let many: Vec<StepSpec> = (0..=STEPS_MOST).map(|n| spec(&format!("s{n}"), Some("x"), &[])).collect();
        assert!(steps(&[], &many).unwrap_err().contains("at most"));
    }

    #[test]
    fn an_approved_step_stays_as_it_was() {
        let mut old = steps(&[], &[spec("a", Some("First"), &[]), spec("b", Some("Second"), &[])]).unwrap();
        old[0].task = Some("t-a".to_string());
        let kept = steps(&old, &[spec("a", None, &[]), spec("c", Some("Third"), &["a"])]).unwrap();
        assert_eq!(kept[0], old[0], "named by key alone, it is kept whole, task and all");
        assert_eq!(kept[1].key, "c");
        assert!(steps(&old, &[spec("a", Some("First"), &[])]).is_ok(), "its own text again changes nothing");
        assert!(steps(&old, &[spec("a", Some("Changed"), &[])]).unwrap_err().contains("step a is approved"));
        assert!(steps(&old, &[spec("z", Some("x"), &[]), spec("a", None, &["z"])]).unwrap_err().contains("step a is approved"));
        let finished = StepSpec { done_when: Some("Shipped".to_string()), ..spec("a", None, &[]) };
        assert!(steps(&old, &[finished]).unwrap_err().contains("step a is approved"));
        assert!(steps(&old, &[spec("b", Some("Second"), &[])]).unwrap_err().contains("keep it among the steps"));
    }

    fn artifact_item(id: u32, text: &str, steps: Vec<Step>) -> Item {
        let mut item = Item::new_task(id, text.to_string(), vec!["My Board".to_string()], 1);
        item.artifact = Some(Box::new(Artifact { steps, version: 1, earlier: Vec::new(), approved_version: None, unknown: BTreeMap::new() }));
        item
    }

    #[test]
    fn a_write_that_changes_the_plan_raises_its_version_and_keeps_the_text_it_replaced() {
        let made = steps(&[], &[spec("a", Some("First"), &[])]).unwrap();
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
        let mut made = steps(&[], &[spec("a", Some("First"), &[]), spec("b", Some("Second"), &[]), spec("c", Some("Third"), &[])]).unwrap();
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

    #[test]
    fn the_page_shows_the_plan_its_steps_and_escapes_what_it_quotes() {
        let made = steps(&[], &[spec("a", Some("First <step>\nwhy"), &[])]).unwrap();
        let text = plan("Ship <it>").replace("## Goal\nText.", "## Goal\nA **bold** goal.\n\n<script>alert(1)</script>");
        let item = artifact_item(1, &text, made);
        let all: ItemMap = BTreeMap::from([(1, item.clone())]);
        let (html, version) = page(&item, &all, Some(Path::new("/projects/site")));
        assert!(html.contains("<h1>Ship &lt;it&gt;</h1>") && html.contains("<title>Ship &lt;it&gt; \u{b7} artifact 1</title>"), "{html}");
        assert!(html.contains("<span class=\"where\">project site \u{b7} artifact 1<span id=\"who\"></span></span>"), "{html}");
        assert!(html.contains("<p class=\"statement\">A <strong>bold</strong> goal.</p>"), "{html}");
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;") && !html.contains("<script>alert"), "raw HTML in a plan is shown, not run: {html}");
        assert!(html.contains("<span class=\"title\">First &lt;step&gt;</span>") && html.contains("data-state=\"to approve\""), "{html}");
        assert!(html.contains("<p class=\"about\">Draft, 1 step.</p>") && html.contains("<ul class=\"tags\"><li>Artifact</li><li>Draft</li><li>1 step</li><li>Version 1</li></ul>"), "{html}");
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
        .unwrap();
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
    fn the_page_holds_its_parts_in_order_its_notes_and_how_its_text_changed() {
        let made = steps(&[], &[spec("a", Some("First"), &[]), spec("b", Some("Second"), &["a"])]).unwrap();
        let mut item = artifact_item(1, &plan("Ship"), made);
        item.description = item.description.replace("## Design\nText.", "## Design\nThe new design.");
        let plan_now = item.artifact.as_mut().unwrap();
        plan_now.version = 3;
        plan_now.earlier.push(Earlier { version: 2, at: 0, text: plan("Ship"), unknown: BTreeMap::new() });
        let mut note = Item::new_note(2, "Keep the page light. It reloads often.\nAnd it reads well.".to_string(), vec!["My Board".to_string()]);
        note.attached_to = item.uid.clone();
        let all: ItemMap = BTreeMap::from([(1, item.clone()), (2, note)]);
        let (html, _) = page(&item, &all, None);
        assert_eq!(html.matches(" data-copy=").count(), 1, "the command is offered once, at the top: {html}");
        let legend = html.split("id=\"map\"").nth(1).and_then(|map| map.split_once("<div class=\"legend\">")).map(|(before, after)| (before.contains("class=\"strip\""), after.split("</div>").next().unwrap_or("")));
        assert_eq!(legend.map(|(late, _)| late), Some(false), "the map has a legend above its strip: {html}");
        for state in ["dot proposed\"></span>to approve", "dot open\"></span>pending", "dot progress\"></span>in progress", "dot done\"></span>done", "an arrow: what a step waits on"] {
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
        let rows: String = parts.iter().map(|(id, name)| format!("<li><a href=\"#{id}\"><span class=\"d\"></span><span class=\"t\">{name}</span></a></li>")).collect();
        assert!(html.contains(&format!("<div class=\"toc-card\"><ol>{rows}</ol></div>")), "the section bars name each part, in the same order: {html}");
        assert!(html.contains(&format!("aria-label=\"Sections\">{}</button>", "<span></span>".repeat(parts.len()))), "a bar per part: {html}");
        let nav = "<a href=\"#plan-goal\">Plan</a><a href=\"#steps\">Steps</a><a href=\"#map\">Map</a><a href=\"#notes\">Notes</a><a href=\"#history\">History</a>";
        assert!(html.contains(&format!("aria-label=\"Parts\">{nav}</nav>")), "the bar's menu: {html}");
        assert!(html.contains("<h2 class=\"opener\">What is known</h2>") && !html.contains("<h2 class=\"opener\">Goal</h2>"), "the Goal opens with its statement: {html}");
        assert!(html.contains("<h3>Keep the page light.</h3>") && html.contains("<p class=\"text\">It reloads often.\nAnd it reads well.</p>"), "a note's first sentence is its title: {html}");
        assert!(html.contains("<div class=\"note\" id=\"note-2\" data-kind=\"Note\"><div class=\"kind\">Note 2 \u{b7} "), "{html}");
        assert_eq!((reading_minutes("word"), reading_minutes(&"word ".repeat(397)), reading_minutes(&"word ".repeat(398))), (1, 1, 2), "Medium's 265 words a minute, rounded");
        assert!(html.contains("Version 2 \u{2192} 3") && html.contains("<div class=\"del\">- Text.</div><div class=\"add\">+ The new design.</div>"), "{html}");
        assert!(html.contains("<p class=\"history\">Version 3 is the current one; none is approved yet.</p>"), "{html}");
        assert!(html.contains("<path class=\"edge\""), "b waits on a: {html}");

        let bare = artifact_item(1, &plan("Ship"), Vec::new());
        let (html, _) = page(&bare, &BTreeMap::from([(1, bare.clone())]), None);
        assert!(html.contains("id=\"steps\" data-part=\"Steps\"><h2 class=\"opener\">Steps</h2><p>No steps yet: the artifact tool writes them.</p>"), "{html}");
        assert!(!html.contains("data-part=\"Map\"") && !html.contains("data-part=\"Notes\""), "no map without steps, no notes without one: {html}");
        assert!(!html.contains("href=\"#map\"") && !html.contains("href=\"#notes\""), "nor do the bars name them: {html}");
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
            assert_eq!(goal(text), format!("<p class=\"statement\">{statement}</p>\n{rest}"), "{text}");
        }
        assert_eq!(goal("- A list first.\n"), "<ul>\n<li>A list first.</li>\n</ul>\n", "no statement unless it opens with a paragraph");
        assert_eq!(goal("### A heading. First\n\nThen.\n"), "<h3>A heading. First</h3>\n<p>Then.</p>\n");
        assert_eq!(goal(""), "");
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
        assert!(html.contains("<section class=\"part\" id=\"plan-goal-2\" data-part=\"Goal\"><h2 class=\"opener\">Goal</h2><p>Once more.</p>"), "only the first Goal opens the plan: {html}");
    }

    #[test]
    fn a_step_opens_in_place_when_it_has_more_to_show() {
        let mut made = steps(&[], &[spec("a", Some("First\nWhy it comes first."), &[]), spec("b", Some("Second"), &["a"])]).unwrap();
        made[1].done_when = Some("It ships.".to_string());
        let mut done = Item::new_task(2, "First".into(), vec![], 1);
        State::Done.write(&mut done);
        made[0].task = done.uid.clone();
        let item = artifact_item(1, &plan("Ship"), made);
        let all: ItemMap = BTreeMap::from([(1, item.clone()), (2, done)]);
        let (html, _) = page(&item, &all, None);
        assert!(
            html.contains("<li class=\"step done\" id=\"step-a\" data-state=\"done\"><button class=\"step-head\" type=\"button\" aria-expanded=\"false\"><span class=\"num\">01</span><span class=\"title\">First</span><span class=\"meta\"><span class=\"dot done\"></span>done \u{b7} task 2</span></button><div class=\"more\"><p>Why it comes first.</p></div></li>"),
            "{html}"
        );
        assert!(html.contains("<span class=\"num\">02</span><span class=\"title\">Second</span><span class=\"meta\"><span class=\"dot proposed\"></span>to approve \u{b7} after a</span></button><div class=\"more\"><p class=\"when\"><b>Done when</b> It ships.</p></div>"), "{html}");
        let bare = artifact_item(1, &plan("Ship"), steps(&[], &[spec("c", Some("Third"), &[])]).unwrap());
        let (html, _) = page(&bare, &BTreeMap::from([(1, bare.clone())]), None);
        assert!(html.contains("<li class=\"step proposed\" id=\"step-c\" data-state=\"to approve\"><div class=\"step-head\">"), "nothing more to show, nothing to open: {html}");
        assert!(!html.contains("<div class=\"more\">"), "{html}");
    }

    #[test]
    fn a_callout_says_what_waits_on_the_user_and_nothing_else_has_one() {
        let made = steps(&[], &[spec("a", Some("First"), &[])]).unwrap();
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
        assert_eq!(html.matches("class=\"callout\"").count(), 1, "the open question only: {html}");
        assert!(html.contains("<div class=\"callout\"><b>Waiting on you: question 2</b>Which comes first?: answer it in ekko's menu, or with <code>ekko --answer 2</code> in a terminal.</div>"), "{html}");
        assert!(html.contains("<div class=\"note open\" id=\"note-2\" data-kind=\"Open question\">") && html.contains("<span class=\"answer\">\u{2713} The docs</span>"), "{html}");

        let mut changed = item.clone();
        let plan_now = changed.artifact.as_mut().unwrap();
        (plan_now.version, plan_now.approved_version) = (2, Some(1));
        changed.trashed = Some(1);
        let (html, _) = page(&changed, &BTreeMap::from([(1, changed.clone())]), None);
        assert!(html.contains("<b>Changed since it was approved</b>The plan changed after version 1 was approved: History shows how."), "{html}");
        assert!(html.contains("<b>In the trash</b>This artifact is in the trash."), "{html}");
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
        let item = artifact_item(1, &plan("Ship"), steps(&[], &[spec("a", Some("First"), &[])]).unwrap());
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
        assert!(std::fs::read_to_string(&path).unwrap().contains("<h1>Ship today</h1>"));
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
        let item = artifact_item(1, &plan("Ship"), steps(&[], &[spec("a", Some("First"), &[])]).unwrap());
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
                html.split("<div class=\"callout\">").skip(1).map(|rest| rest.split("</div>").next().unwrap_or_default().to_string()).collect::<Vec<_>>()
            };
            let waiting = format!("<b>Waiting on you: question {first}</b>It asks you to approve this plan: answer it in ekko's menu, or with <code>ekko --answer {first}</code> in a terminal.");
            assert_eq!(callouts(), [waiting], "the approval asked, once, though the question is attached to the artifact too");
            let shown = || {
                let data = board();
                page(&data[&target], &data, None).0
            };
            assert!(shown().contains("<li>Artifact</li><li>Waiting on you</li>") && shown().contains("<span class=\"who\">Written by a session, "), "{}", shown());
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
            assert!(shown().contains("<li>Artifact</li><li>Approved</li><li>0 of 3 done</li>"), "{}", shown());

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
            data.get_mut(&target).unwrap().artifact.as_mut().unwrap().steps[1].text = "x".repeat(81);
            as_user.storage.set(&data).unwrap();
            let notices = answer(&as_user, asked, "Aprovar").unwrap();
            assert!(notices.iter().any(|notice| notice.contains("no task was made for artifact")), "{notices:?}");
            let data = as_user.storage.get().unwrap();
            assert!(!data.values().any(|item| item.description.starts_with("The step")), "the first step's task goes with the rest");
            assert!(data[&target].blocked_by.as_ref().is_none_or(Vec::is_empty));
            assert!(data[&asked].question.as_ref().unwrap().answer.is_some(), "the answer stays recorded");
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
            assert!(std::fs::read_to_string(&path).unwrap().contains("Draft, 1 step."));
            apply(&as_user, json!({"op": "edit", "item": target, "append": "\nAnd a line more."})).unwrap();
            let page = std::fs::read_to_string(&path).unwrap();
            assert!(page.contains("And a line more.") && page.contains("<li>Version 2</li>"), "an edit through any path rewrites it: {page}");
            apply(&as_user, json!({"op": "create", "text": "Unrelated"})).unwrap();
            assert_eq!(std::fs::read_to_string(&path).unwrap(), page, "a write that changes nothing it shows leaves it");
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
        std::fs::write(&opener, script.replace("SEEN", &seen.display().to_string())).unwrap();
        std::fs::set_permissions(&opener, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
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
