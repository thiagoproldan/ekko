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
//! The page is one HTML file, its style and script inline and nothing
//! fetched, beside a script holding its version, under the board's
//! `artifacts/`. Every write that changes what a page shows rewrites it, the
//! page first, and the page reloads when the version in the script changes:
//! a classic script loads beside a `file://` page where `fetch` is refused,
//! as rustdoc loads its search index (measured in Firefox 157, task 1059).

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
/// `folder`, and the version it carries. Its look is the one the user picked
/// from three mockups (task 1085, decision 1089): a sidebar and tabs, the
/// plan's sections as cards beside its progress, its steps as a table and as
/// the graph their `after` draws, its notes, and its earlier texts compared.
pub fn page(item: &Item, all: &ItemMap, folder: Option<&Path>) -> (String, String) {
    let artifact = item.artifact.as_deref();
    let standing = Standing::of(item, all);
    let (title, plan) = item.description.split_once('\n').unwrap_or((item.description.as_str(), ""));
    let title = title.trim();
    let board = folder.and_then(Path::file_name).map_or_else(|| "the default board".to_string(), |name| format!("project {}", name.to_string_lossy()));
    let uid = item.uid.as_deref().unwrap_or_default();
    let steps = artifact.map(|artifact| shown(artifact, all)).unwrap_or_default();
    let sections = sections(plan);
    let mut notes: Vec<&Item> =
        all.values().filter(|note| !note.is_task && note.trashed.is_none() && !uid.is_empty() && note.attached_to.as_deref() == Some(uid)).collect();
    notes.sort_by_key(|note| std::cmp::Reverse((note.timestamp, note.id)));
    let open_questions: Vec<&Item> = notes.iter().copied().filter(|note| note.question.as_ref().is_some_and(|question| question.answer.is_none())).collect();
    let tasks = steps.iter().filter(|step| step.task.is_some() && step.class != "cancelled").count();
    let done = steps.iter().filter(|step| step.class == "done").count();
    let version = artifact.map_or(0, |artifact| artifact.version);
    let tabs = [
        ("overview", "Overview", String::new()),
        ("plan", "Plan", sections.iter().filter(|(heading, _)| !heading.is_empty()).count().to_string()),
        ("steps", "Steps", if tasks > 0 { format!("{done}/{tasks}") } else { steps.len().to_string() }),
        ("map", "Map", String::new()),
        ("notes", "Notes", notes.len().to_string()),
        ("history", "History", format!("v{version}")),
    ];

    let mut body = String::new();
    // The sidebar: what the page is of, its tabs, and what waits on the user.
    let _ = write!(
        body,
        "<aside class=\"side\"><div class=\"brand\"><span class=\"logo\">e</span><div><b>ekko</b><small>{} \u{b7} artifact {}</small></div></div><nav class=\"nav\"><div class=\"group\">Artifact {}</div>",
        esc(&board),
        item.id,
        item.id
    );
    for (tab, name, count) in &tabs {
        let _ = write!(body, "<a href=\"#{tab}\" data-tab=\"{tab}\">{name}<span class=\"count\">{}</span></a>", esc(count));
    }
    body.push_str("<div class=\"group\">Waiting on you</div>");
    if open_questions.is_empty() {
        body.push_str("<span class=\"none\">Nothing</span>");
    }
    for question in &open_questions {
        let _ = write!(body, "<a href=\"#notes\" data-tab=\"notes\">Question {}<span class=\"count\">open</span></a>", question.id);
    }
    let _ = write!(body, "</nav><div class=\"side-foot\"><span class=\"live\"></span>Follows the board \u{b7} ekko {}</div></aside>", env!("CARGO_PKG_VERSION"));

    // The head of the page: where it stands, and what asks for the user.
    let _ = write!(
        body,
        "<main><div class=\"topbar\"><span class=\"crumb\">ekko \u{b7} {} \u{b7} artifact {}</span><span class=\"spacer\"></span><button class=\"btn\" data-copy=\"ekko artifact {}\">Copy command</button><button class=\"btn ghost\" id=\"theme\">Dark</button></div><div class=\"content\">",
        esc(&board),
        item.id,
        item.id
    );
    let _ = write!(body, "<h1>{}</h1><div class=\"badges\">", esc(title));
    if let (Some(artifact), Some(standing)) = (artifact, &standing) {
        let tone = if !standing.open() {
            "outline"
        } else if standing.waiting.is_some() || standing.changed {
            "warn"
        } else if standing.tasks > 0 {
            "ok"
        } else {
            "plain"
        };
        let _ = write!(body, "<span class=\"badge {tone}\">{}</span>", esc(&standing.words()));
        let approved = match artifact.approved_version {
            Some(approved) if approved == artifact.version => " \u{b7} approved".to_string(),
            Some(approved) => format!(" \u{b7} version {approved} approved"),
            None => String::new(),
        };
        let _ = write!(body, "<span class=\"badge plain\">version {}{approved}</span>", artifact.version);
        let _ = write!(body, "<span class=\"badge outline\">{}</span>", plural(artifact.steps.len(), "step"));
    }
    if let Some(priority) = item.priority.filter(|priority| *priority > 1) {
        let _ = write!(body, "<span class=\"badge outline\">priority {priority}</span>");
    }
    let _ = write!(body, "<span class=\"muted small\">updated {}</span></div>", esc(&when(item.updated_at.unwrap_or(item.timestamp))));
    if let (Some(artifact), Some(standing)) = (artifact, &standing) {
        if let Some(question) = standing.waiting {
            let _ = write!(
                body,
                "<p class=\"banner warn\">Waiting on you: question {question} asks you to approve this plan. Answer it in ekko's menu, or with <code>ekko --answer {question}</code> in a terminal.</p>"
            );
        }
        if standing.changed {
            let _ = write!(
                body,
                "<p class=\"banner\">The plan changed after version {} was approved: History shows how.</p>",
                artifact.approved_version.unwrap_or_default()
            );
        }
    }
    if item.trashed.is_some() || item.stashed.is_some() {
        let away = if item.trashed.is_some() { "in the trash" } else { "stashed" };
        let _ = write!(body, "<p class=\"banner\">This artifact is {away}.</p>");
    }
    body.push_str("<div class=\"tabs\" role=\"tablist\">");
    for (tab, name, _) in &tabs {
        let _ = write!(body, "<button data-tab=\"{tab}\">{name}</button>");
    }
    body.push_str("</div>");

    // Overview: the plan's sections but the research, beside its progress.
    body.push_str("<section data-panel=\"overview\" class=\"grid\"><div class=\"stack\">");
    let mut cards = sections.iter().filter(|(heading, _)| heading != "What is known").peekable();
    if cards.peek().is_none() {
        body.push_str("<div class=\"card\"><div class=\"card-b muted\">The plan has no text yet.</div></div>");
    }
    for (heading, text) in cards {
        let heading = if heading.is_empty() { "Plan" } else { heading.as_str() };
        let _ = write!(body, "<div class=\"card\"><div class=\"card-h\"><h3>{}</h3></div><div class=\"card-b prose\">{}</div></div>", esc(heading), markdown(text));
    }
    body.push_str("</div><div class=\"stack\">");
    progress_card(&mut body, &steps, tasks, done);
    if !steps.is_empty() {
        body.push_str("<div class=\"card\"><div class=\"card-h\"><h3>Steps</h3><a href=\"#steps\" data-tab=\"steps\" class=\"small\">all</a></div><div class=\"card-b\">");
        for step in &steps {
            let task = step.task.map(|id| id.to_string()).unwrap_or_default();
            let _ = write!(
                body,
                "<div class=\"mini\"><span class=\"dot {}\" title=\"{}\"></span><span class=\"k\">{}</span><span class=\"t\">{}</span><span class=\"id\">{task}</span></div>",
                step.class,
                esc(&step.state),
                esc(&step.step.key),
                esc(step.title)
            );
        }
        body.push_str("</div></div>");
    }
    if !notes.is_empty() {
        body.push_str("<div class=\"card\"><div class=\"card-h\"><h3>Questions and decisions</h3><a href=\"#notes\" data-tab=\"notes\" class=\"small\">all notes</a></div><div class=\"card-b\">");
        let listed: Vec<&&Item> = notes.iter().filter(|note| note.question.is_some() || note.mark() == Some("decision")).take(6).collect();
        if listed.is_empty() {
            body.push_str("<p class=\"muted\">None yet.</p>");
        }
        for note in listed {
            let _ = write!(body, "<div class=\"mini wrap\"><span class=\"t\">{}</span>{}</div>", esc(crate::ekko::title(&note.description)), note_badge(note));
        }
        body.push_str("</div></div>");
    }
    if let Some(artifact) = artifact {
        body.push_str("<div class=\"card\"><div class=\"card-h\"><h3>Versions</h3>");
        if !artifact.earlier.is_empty() {
            body.push_str("<a href=\"#history\" data-tab=\"history\" class=\"small\">compare</a>");
        }
        let current = if artifact.approved_version == Some(artifact.version) { "current \u{b7} approved" } else { "current" };
        let _ = write!(body, "</div><div class=\"card-b\"><div class=\"mini\"><span class=\"badge ok\">v{}</span><span class=\"t\">{current}</span></div>", artifact.version);
        for earlier in artifact.earlier.iter().rev() {
            let approved = if artifact.approved_version == Some(earlier.version) { " \u{b7} approved" } else { "" };
            let _ = write!(
                body,
                "<div class=\"mini\"><span class=\"badge outline\">v{}</span><span class=\"t\">replaced {}{approved}</span></div>",
                earlier.version,
                esc(&when(earlier.at))
            );
        }
        body.push_str("</div></div>");
    }
    body.push_str("</div></section>");

    // Plan: the whole text, to read.
    let _ = write!(body, "<section data-panel=\"plan\" hidden><article class=\"prose doc\">{}</article></section>", markdown(plan));

    // Steps: each with its task, what it waits on, and when it is done.
    body.push_str("<section data-panel=\"steps\" hidden>");
    if steps.is_empty() {
        body.push_str("<p class=\"muted\">No steps yet: the artifact tool writes them.</p>");
    } else {
        body.push_str("<div class=\"card\"><table><thead><tr><th>Key</th><th>Step</th><th>After</th><th>Task</th><th>State</th></tr></thead><tbody>");
        for step in &steps {
            let _ = write!(body, "<tr id=\"step-{}\"><td><code>{}</code></td><td><div class=\"step-title\">{}</div>", esc(&step.step.key), esc(&step.step.key), esc(step.title));
            step_details(&mut body, step);
            let after = if step.step.after.is_empty() { "\u{2014}".to_string() } else { step.step.after.join(", ") };
            let task = step.task.map_or_else(|| "\u{2014}".to_string(), |id| id.to_string());
            let _ = write!(body, "</td><td class=\"muted\">{}</td><td>{task}</td><td><span class=\"badge {}\">{}</span></td></tr>", esc(&after), step.class, esc(&step.state));
        }
        body.push_str("</tbody></table></div>");
    }
    body.push_str("</section>");

    // Map: the steps as the graph their `after` draws.
    body.push_str("<section data-panel=\"map\" hidden>");
    if steps.is_empty() {
        body.push_str("<p class=\"muted\">No steps yet: the artifact tool writes them.</p>");
    } else {
        let _ = write!(body, "<div class=\"map-wrap\">{}</div>", map(&steps));
        body.push_str("<div class=\"legend\"><span><span class=\"dot proposed\"></span>to approve</span><span><span class=\"dot open\"></span>pending</span><span><span class=\"dot progress\"></span>in progress</span><span><span class=\"dot done\"></span>done</span><span>an arrow: what a step waits on</span></div>");
        body.push_str("<p class=\"muted hint\">Pick a step to see it.</p>");
        for step in &steps {
            let task = step.task.map(|id| format!(" \u{b7} task {id}")).unwrap_or_default();
            let after = if step.step.after.is_empty() { String::new() } else { format!(" \u{b7} after {}", step.step.after.join(", ")) };
            let _ = write!(
                body,
                "<div class=\"card detail\" id=\"detail-{}\" hidden><div class=\"card-h\"><h3>{}</h3><span class=\"badge {}\">{}</span></div><div class=\"card-b\"><p class=\"muted small\"><code>{}</code>{}{}</p>",
                esc(&step.step.key),
                esc(step.title),
                step.class,
                esc(&step.state),
                esc(&step.step.key),
                task,
                esc(&after)
            );
            step_details(&mut body, step);
            body.push_str("</div></div>");
        }
    }
    body.push_str("</section>");

    // Notes: the questions, decisions and notes attached, newest first.
    body.push_str("<section data-panel=\"notes\" hidden>");
    if notes.is_empty() {
        body.push_str("<p class=\"muted\">No note is attached to this artifact.</p>");
    }
    for note in &notes {
        let (head, rest) = note.description.trim().split_once('\n').unwrap_or((note.description.trim(), ""));
        let _ = write!(
            body,
            "<article class=\"card note\"><div class=\"card-h\"><h3>{}</h3>{}</div><div class=\"card-b\"><p class=\"muted small\">{} \u{b7} {}</p>",
            esc(head.trim()),
            note_badge(note),
            note.id,
            esc(&when(note.timestamp))
        );
        if !rest.trim().is_empty() {
            if note.question.is_some() {
                let _ = write!(body, "<details><summary>What it explained and offered</summary><div class=\"note-text\">{}</div></details>", esc(rest.trim()));
            } else {
                let _ = write!(body, "<div class=\"note-text\">{}</div>", esc(rest.trim()));
            }
        }
        body.push_str("</div></article>");
    }
    body.push_str("</section>");

    // History: each earlier text against the one that replaced it.
    body.push_str("<section data-panel=\"history\" hidden>");
    match artifact {
        Some(artifact) if !artifact.earlier.is_empty() => {
            if let Some(approved) = artifact.approved_version {
                let _ = write!(body, "<p class=\"muted\">Version {approved} is the one you approved.</p>");
            }
            let mut earlier: Vec<&Earlier> = artifact.earlier.iter().collect();
            earlier.sort_by_key(|earlier| earlier.version);
            for (at, old) in earlier.iter().enumerate().rev() {
                let new = earlier.get(at + 1).map_or(item.description.as_str(), |next| next.text.as_str());
                let _ = write!(
                    body,
                    "<div class=\"card\"><div class=\"card-h\"><h3>Version {} \u{2192} {}</h3><span class=\"muted small\">{}</span></div><div class=\"card-b\"><div class=\"diff\">{}</div></div></div>",
                    old.version,
                    old.version + 1,
                    esc(&when(old.at)),
                    diff_html(&old.text, new)
                );
            }
        }
        _ => body.push_str("<p class=\"muted\">One text so far: nothing to compare. Each change to the plan's text keeps the one it replaced here.</p>"),
    }
    body.push_str("</section>");

    let _ = write!(
        body,
        "<footer>Written by ekko {} from the board. It reloads by itself when the board changes; <code>ekko artifact {}</code> writes it again.</footer></div></main>",
        env!("CARGO_PKG_VERSION"),
        item.id
    );

    let version = format!("{:016x}", fnv(body.as_bytes()));
    let poll = format!(
        "(function () {{\n  var version = \"{version}\";\n  window.ekkoArtifact = function (seen) {{ if (seen !== version) location.reload(); }};\n  setInterval(function () {{\n    var script = document.createElement(\"script\");\n    script.src = \"{}.js?t=\" + Date.now();\n    script.onload = script.onerror = function () {{ script.remove(); }};\n    document.head.appendChild(script);\n  }}, {POLL_MS});\n}})();",
        js_string(uid)
    );
    let html = format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>{} \u{b7} artifact {}</title>\n<script>{THEME}</script>\n<style>{STYLE}</style>\n</head>\n<body>\n<div class=\"app\">\n{body}\n</div>\n<script>{SCRIPT}</script>\n<script>{poll}</script>\n</body>\n</html>\n",
        esc(title),
        item.id
    );
    (html, version)
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

/// What a step says past its title, and when it is done.
fn step_details(body: &mut String, step: &Shown) {
    if !step.rest.is_empty() {
        let _ = write!(body, "<div class=\"step-text\">{}</div>", esc(step.rest));
    }
    if let Some(done_when) = &step.step.done_when {
        let _ = write!(body, "<div class=\"done-when\"><b>Done when</b> {}</div>", esc(done_when));
    }
}

/// The progress card: the steps done of the tasks the approvals made, and a
/// segment per step in its state's colour.
fn progress_card(body: &mut String, steps: &[Shown], tasks: usize, done: usize) {
    body.push_str("<div class=\"card\"><div class=\"card-h\"><h3>Progress</h3><span class=\"muted small\">tasks on the board</span></div><div class=\"card-b\">");
    if tasks > 0 {
        let _ = write!(body, "<div class=\"big\">{done}<span> / {tasks} done</span></div>");
    } else {
        let _ = write!(body, "<div class=\"big\">{}<span> to approve</span></div>", plural(steps.len(), "step"));
    }
    body.push_str("<div class=\"segments\">");
    for step in steps {
        let _ = write!(body, "<span class=\"{}\" title=\"{}: {}\"></span>", step.class, esc(&step.step.key), esc(&step.state));
    }
    body.push_str("</div><div class=\"legend\">");
    for (class, word) in [("done", "done"), ("progress", "in progress"), ("open", "pending"), ("proposed", "to approve"), ("cancelled", "cancelled"), ("gone", "not on this board")] {
        let count = steps.iter().filter(|step| step.class == class).count();
        if count > 0 {
            let _ = write!(body, "<span><span class=\"dot {class}\"></span>{count} {word}</span>");
        }
    }
    body.push_str("</div></div></div>");
}

/// A note's kind as a badge: an open question stands out.
fn note_badge(note: &Item) -> String {
    match (&note.question, note.mark()) {
        (Some(question), _) => match &question.answer {
            None => "<span class=\"badge warn\">open question</span>".to_string(),
            Some(answer) => format!("<span class=\"badge ok\">\u{2713} {}</span>", esc(crate::menu::picked(&answer.text))),
        },
        (None, Some(mark)) => format!("<span class=\"badge plain\">{}</span>", esc(mark)),
        (None, None) => "<span class=\"badge outline\">note</span>".to_string(),
    }
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
/// page (task 1059). The page's path.
pub fn write(dir: &Path, item: &Item, all: &ItemMap, folder: Option<&Path>) -> std::io::Result<PathBuf> {
    let uid = item.uid.as_deref().ok_or_else(|| std::io::Error::other("an artifact without a uid has no page"))?;
    let pages = dir.join(PAGES);
    std::fs::create_dir_all(&pages)?;
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
/// every `POLL_MS` and reloads on when it changed: from the file next to a
/// written page, or from `ekko serve` (task 1102).
pub fn script(version: &str) -> String {
    format!("ekkoArtifact(\"{version}\");\n")
}

/// Opens `page`, a file or an address, in the default browser, as `cargo doc
/// --open` does, without waiting for it.
pub fn open(page: impl AsRef<std::ffi::OsStr>) -> std::io::Result<()> {
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    std::process::Command::new(opener)
        .arg(page)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
}

/// Replaces `path` with `content` by rename from a file beside it.
fn replace(path: &Path, content: &[u8]) -> std::io::Result<()> {
    let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let temp = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    std::fs::write(&temp, content)?;
    std::fs::rename(&temp, path)
}

/// The plan's Markdown as HTML. Raw HTML in it is shown as text, not run:
/// the page is the board's, and a script a plan carried would run there.
/// For the same reason a link keeps its address only when `linkable`, and
/// is its words alone otherwise. An image is a link to its source, never
/// loaded, since the page fetches nothing (task 1097).
fn markdown(text: &str) -> String {
    use pulldown_cmark::{html, Event, LinkType, Options, Parser, Tag, TagEnd};
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut events = Vec::new();
    // Each link and image open here, innermost last: whether it kept its
    // tag, and a kept image's source, which names it when it has no words.
    let mut open = Vec::new();
    for event in Parser::new_ext(text, options) {
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
    let mut out = String::new();
    html::push_html(&mut out, events.into_iter());
    out
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
/// page does.
fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3))
}

/// Sets the page's theme before it draws: the one the user picked last, or
/// the system's.
const THEME: &str = r##"(function () { var theme = null; try { theme = localStorage.getItem("ekko-theme"); } catch (e) {} document.documentElement.dataset.theme = theme || (matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light"); })();"##;

/// The page's tabs, its theme toggle, the map's steps and the copy button.
/// The open tab is the address's fragment, which a reload keeps.
const SCRIPT: &str = r##"(function () {
  var root = document.documentElement, toggle = document.getElementById("theme");
  function label() { toggle.textContent = root.dataset.theme === "dark" ? "Light" : "Dark"; }
  label();
  toggle.addEventListener("click", function () {
    root.dataset.theme = root.dataset.theme === "dark" ? "light" : "dark";
    label();
    try { localStorage.setItem("ekko-theme", root.dataset.theme); } catch (e) {}
  });
  var tabs = Array.prototype.map.call(document.querySelectorAll(".tabs [data-tab]"), function (tab) { return tab.dataset.tab; });
  function open(tab) {
    if (tabs.indexOf(tab) < 0) tab = tabs[0];
    document.querySelectorAll("[data-panel]").forEach(function (panel) { panel.hidden = panel.dataset.panel !== tab; });
    document.querySelectorAll("[data-tab]").forEach(function (link) { link.classList.toggle("on", link.dataset.tab === tab); });
    if (location.hash !== "#" + tab) { try { history.replaceState(null, "", "#" + tab); } catch (e) { location.hash = tab; } }
  }
  document.querySelectorAll("[data-tab]").forEach(function (link) {
    link.addEventListener("click", function (event) { event.preventDefault(); open(link.dataset.tab); });
  });
  open(location.hash.slice(1));
  document.querySelectorAll(".node").forEach(function (node) {
    node.addEventListener("click", function () {
      document.querySelectorAll(".node").forEach(function (other) { other.classList.toggle("sel", other === node); });
      document.querySelectorAll(".detail").forEach(function (detail) { detail.hidden = detail.id !== "detail-" + node.dataset.step; });
      document.querySelectorAll(".hint").forEach(function (hint) { hint.hidden = true; });
    });
  });
  document.querySelectorAll("[data-copy]").forEach(function (button) {
    button.addEventListener("click", function () {
      var label = button.textContent;
      var copied = navigator.clipboard ? navigator.clipboard.writeText(button.dataset.copy) : Promise.reject();
      copied.then(function () { button.textContent = "Copied"; }, function () { button.textContent = "Copy failed"; }).then(function () {
        setTimeout(function () { button.textContent = label; }, 1200);
      });
    });
  });
})();"##;

/// The look: shadcn/ui's neutral theme, written by hand (decision 1089).
const STYLE: &str = r##"
:root {
  color-scheme: light;
  --bg: #ffffff; --fg: #09090b; --card: #ffffff; --muted: #f4f4f5; --muted-fg: #71717a; --border: #e4e4e7;
  --primary: #18181b; --primary-fg: #fafafa; --ring: #a1a1aa; --side: #fafafa;
  --ok: #15803d; --ok-bg: #dcfce7; --warn: #a16207; --warn-bg: #fef9c3; --accent: #2563eb; --accent-bg: #dbeafe;
  --add: #dcfce7; --add-fg: #166534; --del: #fee2e2; --del-fg: #991b1b;
  --radius: 10px;
  --sans: "Inter", "Geist", "Segoe UI", "Helvetica Neue", "Liberation Sans", Arial, "DejaVu Sans", sans-serif;
  --mono: "Geist Mono", "JetBrains Mono", "JetBrainsMono Nerd Font", ui-monospace, "SFMono-Regular", Menlo, "DejaVu Sans Mono", monospace;
}
html[data-theme="dark"] {
  color-scheme: dark;
  --bg: #09090b; --fg: #fafafa; --card: #0c0c0f; --muted: #27272a; --muted-fg: #a1a1aa; --border: #27272a;
  --primary: #fafafa; --primary-fg: #18181b; --ring: #52525b; --side: #0c0c0f;
  --ok: #4ade80; --ok-bg: #14532d66; --warn: #facc15; --warn-bg: #713f1266; --accent: #93c5fd; --accent-bg: #1e3a8a66;
  --add: #14532d55; --add-fg: #86efac; --del: #7f1d1d55; --del-fg: #fca5a5;
}
* { box-sizing: border-box; }
body { margin: 0; background: var(--bg); color: var(--fg); font: 14px/1.55 var(--sans); }
button { font: inherit; color: inherit; }
a { color: var(--accent); }
code { font: 0.86em var(--mono); background: var(--muted); border-radius: 5px; padding: 0.1em 0.35em; }
pre { background: var(--muted); border-radius: 8px; padding: 0.8rem 0.9rem; overflow-x: auto; }
pre code { background: none; padding: 0; }
[hidden] { display: none !important; }
.muted { color: var(--muted-fg); }
.small { font-size: 0.8rem; }

.app { display: grid; grid-template-columns: 15rem minmax(0, 1fr); min-height: 100vh; }
.side { position: sticky; top: 0; height: 100vh; display: flex; flex-direction: column; gap: 0.9rem; padding: 0.9rem 0.7rem; background: var(--side); border-right: 1px solid var(--border); overflow-y: auto; }
.brand { display: flex; gap: 0.6rem; align-items: center; padding: 0.2rem 0.4rem; }
.brand b { display: block; font-size: 0.92rem; }
.brand small { color: var(--muted-fg); font-size: 0.78rem; }
.logo { width: 2rem; height: 2rem; flex: none; border-radius: 8px; display: grid; place-items: center; background: var(--primary); color: var(--primary-fg); font-weight: 700; }
.nav { display: flex; flex-direction: column; gap: 0.1rem; }
.group { padding: 0.7rem 0.55rem 0.3rem; font-size: 0.72rem; font-weight: 600; color: var(--muted-fg); }
.nav a { display: flex; align-items: center; gap: 0.5rem; padding: 0.38rem 0.55rem; border-radius: 7px; color: var(--fg); text-decoration: none; }
.nav a:hover { background: var(--muted); }
.nav a.on { background: var(--muted); font-weight: 600; }
.nav .count { margin-left: auto; font-size: 0.75rem; font-weight: 400; color: var(--muted-fg); }
.none { padding: 0.38rem 0.55rem; color: var(--muted-fg); }
.side-foot { margin-top: auto; padding: 0 0.5rem; font-size: 0.75rem; color: var(--muted-fg); }
.live { display: inline-block; width: 0.5rem; height: 0.5rem; margin-right: 0.4rem; border-radius: 50%; background: var(--ok); box-shadow: 0 0 0 3px var(--ok-bg); }

main { min-width: 0; }
.topbar { position: sticky; top: 0; z-index: 4; display: flex; align-items: center; gap: 0.5rem; padding: 0.6rem 1.6rem; border-bottom: 1px solid var(--border); background: color-mix(in srgb, var(--bg) 90%, transparent); backdrop-filter: blur(8px); }
.crumb { color: var(--muted-fg); }
.spacer { flex: 1; }
.btn { display: inline-flex; align-items: center; height: 2rem; padding: 0 0.8rem; border: 1px solid var(--border); border-radius: 8px; background: var(--bg); font-size: 0.85rem; font-weight: 500; white-space: nowrap; cursor: pointer; }
.btn:hover { background: var(--muted); }
.btn.ghost { border-color: transparent; }

.content { max-width: 84rem; padding: 1.6rem 1.6rem 4rem; }
h1 { margin: 0 0 0.7rem; font-size: 1.6rem; line-height: 1.25; letter-spacing: -0.02em; font-weight: 650; }
.badges { display: flex; flex-wrap: wrap; align-items: center; gap: 0.4rem; }
.badge { display: inline-flex; align-items: center; gap: 0.3rem; padding: 0.12rem 0.55rem; border: 1px solid transparent; border-radius: 999px; font-size: 0.75rem; font-weight: 600; white-space: nowrap; background: var(--muted); }
.badge.ok, .badge.done { background: var(--ok-bg); color: var(--ok); }
.badge.warn { background: var(--warn-bg); color: var(--warn); }
.badge.progress { background: var(--accent-bg); color: var(--accent); }
.badge.outline, .badge.open { background: none; border-color: var(--border); color: var(--muted-fg); }
.badge.proposed { background: none; border: 1px dashed var(--ring); color: var(--muted-fg); }
.badge.cancelled, .badge.gone { background: none; border-color: var(--border); color: var(--muted-fg); text-decoration: line-through; }
.banner { margin: 1rem 0 0; padding: 0.7rem 0.9rem; border: 1px solid var(--border); border-radius: var(--radius); background: var(--muted); }
.banner.warn { background: var(--warn-bg); border-color: transparent; }
.tabs { display: inline-flex; flex-wrap: wrap; gap: 0.2rem; margin: 1.2rem 0; padding: 0.25rem; border-radius: 9px; background: var(--muted); }
.tabs button { padding: 0.3rem 0.85rem; border: 0; border-radius: 7px; background: none; color: var(--muted-fg); font-weight: 500; cursor: pointer; }
.tabs button.on { background: var(--bg); color: var(--fg); box-shadow: 0 1px 2px rgb(0 0 0 / 0.08); }

.grid { display: grid; grid-template-columns: minmax(0, 1fr) 21rem; gap: 1.2rem; align-items: start; }
.stack { display: flex; flex-direction: column; gap: 1.2rem; min-width: 0; }
.card { min-width: 0; border: 1px solid var(--border); border-radius: var(--radius); background: var(--card); box-shadow: 0 1px 2px rgb(0 0 0 / 0.04); }
.card + .card { margin-top: 0; }
[data-panel] > .card + .card { margin-top: 1.2rem; }
.card-h { display: flex; align-items: center; justify-content: space-between; gap: 0.6rem; padding: 0.95rem 1.1rem 0; }
.card-h h3 { margin: 0; font-size: 0.95rem; font-weight: 600; }
.card-b { padding: 0.6rem 1.1rem 1rem; }
.prose p { margin: 0.3rem 0 0.6rem; }
.prose ul, .prose ol { margin: 0.3rem 0; padding-left: 1.2rem; }
.prose li { margin: 0.3rem 0; }
.prose table { border-collapse: collapse; }
.prose th, .prose td { padding: 0.3rem 0.6rem; border: 1px solid var(--border); }
.doc { max-width: 48rem; font-size: 0.98rem; line-height: 1.7; }
.doc h2 { margin: 1.8rem 0 0.5rem; padding-bottom: 0.3rem; border-bottom: 1px solid var(--border); font-size: 1.2rem; }
.doc h2:first-child { margin-top: 0.4rem; }

.big { font-size: 2rem; font-weight: 700; letter-spacing: -0.03em; }
.big span { font-size: 1rem; font-weight: 500; color: var(--muted-fg); }
.segments { display: flex; gap: 3px; margin: 0.5rem 0 0.5rem; }
.segments span { flex: 1; height: 8px; border-radius: 3px; background: var(--muted); }
.segments .done { background: var(--ok); }
.segments .progress { background: var(--accent); }
.segments .open { background: var(--ring); opacity: 0.45; }
.segments .proposed { background: none; border: 1px dashed var(--ring); }
.segments .cancelled, .segments .gone { opacity: 0.3; }
.legend { display: flex; flex-wrap: wrap; gap: 0.4rem 1rem; font-size: 0.78rem; color: var(--muted-fg); }
.dot { display: inline-block; width: 0.62rem; height: 0.62rem; margin-right: 0.35rem; border: 1.5px solid var(--ring); border-radius: 50%; vertical-align: -0.06rem; flex: none; }
.dot.done { background: var(--ok); border-color: var(--ok); }
.dot.progress { background: var(--accent); border-color: var(--accent); }
.dot.proposed { border-style: dashed; }
.dot.cancelled, .dot.gone { background: var(--muted); }
.mini { display: flex; align-items: center; gap: 0.55rem; padding: 0.42rem 0; border-top: 1px solid var(--border); font-size: 0.85rem; }
.mini:first-child { border-top: 0; }
.mini .k { width: 4.8rem; flex: none; font: 0.75rem var(--mono); color: var(--muted-fg); overflow: hidden; text-overflow: ellipsis; }
.mini .t { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.mini.wrap .t { white-space: normal; }
.mini .id { font-size: 0.75rem; color: var(--muted-fg); }

table { width: 100%; border-collapse: collapse; font-size: 0.88rem; }
th { padding: 0.6rem 0.8rem; border-bottom: 1px solid var(--border); text-align: left; font-weight: 500; color: var(--muted-fg); }
td { padding: 0.65rem 0.8rem; border-bottom: 1px solid var(--border); vertical-align: top; }
tr:last-child td { border-bottom: 0; }
td:first-child code { white-space: nowrap; }
.step-title { font-weight: 500; }
.step-text, .done-when { margin-top: 0.25rem; font-size: 0.84rem; color: var(--muted-fg); white-space: pre-wrap; }
.done-when b { color: var(--fg); font-weight: 600; }

.map-wrap { overflow-x: auto; padding: 0.4rem 0.2rem 0.8rem; }
.map { position: relative; }
.map svg { position: absolute; inset: 0; overflow: visible; }
.edge { fill: none; stroke: var(--ring); stroke-width: 1.6; }
.arrowhead { fill: var(--ring); }
.node { position: absolute; display: flex; flex-direction: column; justify-content: flex-start; align-items: stretch; width: 200px; height: 92px; padding: 0.55rem 0.7rem; border: 1px solid var(--border); border-radius: 12px; background: var(--card); text-align: left; cursor: pointer; }
.node:hover { border-color: var(--ring); }
.node.sel { border-color: var(--accent); box-shadow: 0 0 0 3px var(--accent-bg); }
.node.done { border-color: color-mix(in srgb, var(--ok) 50%, var(--border)); }
.node.progress { border-color: color-mix(in srgb, var(--accent) 60%, var(--border)); }
.node.proposed { border-style: dashed; }
.node .k { display: block; font: 0.72rem var(--mono); color: var(--accent); }
.node .t { display: -webkit-box; margin-top: 0.15rem; overflow: hidden; font-size: 0.82rem; line-height: 1.3; -webkit-line-clamp: 2; -webkit-box-orient: vertical; }
.node.cancelled .t, .node.gone .t { text-decoration: line-through; color: var(--muted-fg); }
.node .s { position: absolute; right: 0.7rem; bottom: 0.45rem; left: 0.7rem; display: flex; justify-content: space-between; font-size: 0.72rem; color: var(--muted-fg); }
.hint { margin-top: 1rem; }
.detail { margin-top: 1rem; max-width: 48rem; }

.note-text { white-space: pre-wrap; }
details summary { cursor: pointer; color: var(--muted-fg); }
details .note-text { margin-top: 0.5rem; }
.diff { overflow: hidden; border: 1px solid var(--border); border-radius: 8px; font: 0.82rem/1.6 var(--mono); }
.diff div { padding: 0.1rem 0.8rem; white-space: pre-wrap; word-break: break-word; }
.diff .ctx { color: var(--muted-fg); }
.diff .add { background: var(--add); color: var(--add-fg); }
.diff .del { background: var(--del); color: var(--del-fg); }
.diff .hunk { background: var(--muted); color: var(--muted-fg); font-style: italic; }
footer { margin-top: 2.5rem; font-size: 0.8rem; color: var(--muted-fg); }

@media (max-width: 1200px) { .grid { grid-template-columns: minmax(0, 1fr); } }
@media (max-width: 860px) { .app { grid-template-columns: 1fr; } .side { display: none; } .content { padding: 1rem; } }
@media print { .side, .topbar, .tabs { display: none; } .app { display: block; } [data-panel] { display: block !important; margin-bottom: 1.5rem; } }
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
        assert!(html.contains("<h1>Ship &lt;it&gt;</h1>"), "{html}");
        assert!(html.contains("ekko \u{b7} project site \u{b7} artifact 1"), "{html}");
        assert!(html.contains("<strong>bold</strong>"), "{html}");
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;") && !html.contains("<script>alert"), "raw HTML in a plan is shown, not run: {html}");
        assert!(html.contains("First &lt;step&gt;") && html.contains("to approve"), "{html}");
        assert!(html.contains("draft, 1 step"), "{html}");
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
    fn the_page_holds_its_tabs_its_notes_and_how_its_text_changed() {
        let made = steps(&[], &[spec("a", Some("First"), &[]), spec("b", Some("Second"), &["a"])]).unwrap();
        let mut item = artifact_item(1, &plan("Ship"), made);
        item.description = item.description.replace("## Design\nText.", "## Design\nThe new design.");
        let plan_now = item.artifact.as_mut().unwrap();
        plan_now.version = 3;
        plan_now.earlier.push(Earlier { version: 2, at: 0, text: plan("Ship"), unknown: BTreeMap::new() });
        let mut note = Item::new_note(2, "Keep the page light\nIt reloads often.".to_string(), vec!["My Board".to_string()]);
        note.attached_to = item.uid.clone();
        let all: ItemMap = BTreeMap::from([(1, item.clone()), (2, note)]);
        let (html, _) = page(&item, &all, None);
        for tab in ["overview", "plan", "steps", "map", "notes", "history"] {
            assert!(html.contains(&format!("<section data-panel=\"{tab}\"")) && html.contains(&format!("<button data-tab=\"{tab}\">")), "{tab}: {html}");
        }
        assert!(html.contains("<h3>Keep the page light</h3>") && html.contains("It reloads often."), "the note attached: {html}");
        assert!(html.contains("Version 2 \u{2192} 3") && html.contains("<div class=\"del\">- Text.</div><div class=\"add\">+ The new design.</div>"), "{html}");
        assert!(html.contains("<path class=\"edge\""), "b waits on a: {html}");
        assert!(!html.contains("What is known</h3>"), "the research is the plan tab's, not a card: {html}");
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
        let left: Vec<String> = std::fs::read_dir(dir.join(PAGES)).unwrap().flatten().map(|entry| entry.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(left.len(), 2, "no temp file is left: {left:?}");
        std::fs::remove_dir_all(&dir).ok();
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
            answer(&as_user, first, "Ainda não").unwrap();
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
            assert!(std::fs::read_to_string(&path).unwrap().contains("draft, 1 step"));
            apply(&as_user, json!({"op": "edit", "item": target, "append": "\nAnd a line more."})).unwrap();
            let page = std::fs::read_to_string(&path).unwrap();
            assert!(page.contains("And a line more.") && page.contains("version 2"), "an edit through any path rewrites it: {page}");
            apply(&as_user, json!({"op": "create", "text": "Unrelated"})).unwrap();
            assert_eq!(std::fs::read_to_string(&path).unwrap(), page, "a write that changes nothing it shows leaves it");
            std::fs::remove_dir_all(&home).ok();
        }
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
