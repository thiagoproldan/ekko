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

use crate::item::{Earlier, Item, State, Step};
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
/// `folder`, and the version it carries.
pub fn page(item: &Item, all: &ItemMap, folder: Option<&Path>) -> (String, String) {
    let artifact = item.artifact.as_deref();
    let standing = Standing::of(item, all);
    let (title, plan) = item.description.split_once('\n').unwrap_or((item.description.as_str(), ""));
    let board = folder.and_then(Path::file_name).map_or_else(|| "the default board".to_string(), |name| format!("project {}", name.to_string_lossy()));
    let index = crate::ekko::uid_index(all);
    let by_uid = |uid: &str| index.get(uid).and_then(|id| all.get(id));

    let mut body = String::new();
    let _ = writeln!(body, "<header><p class=\"where\">ekko \u{b7} {} \u{b7} artifact {}</p>", esc(&board), item.id);
    let _ = writeln!(body, "<h1>{}</h1>", esc(title.trim()));
    if let (Some(artifact), Some(standing)) = (artifact, &standing) {
        let approved = match artifact.approved_version {
            Some(version) => format!(", version {version} approved"),
            None => String::new(),
        };
        let _ = writeln!(
            body,
            "<p class=\"standing {}\">{} \u{b7} version {}{approved}</p>",
            if standing.open() { "open" } else { "closed" },
            esc(&standing.words()),
            artifact.version
        );
        if let Some(question) = standing.waiting {
            let _ = writeln!(
                body,
                "<p class=\"banner\">Waiting on you: question {question}. Answer it in ekko's menu, or with <code>ekko --answer {question}</code> in a terminal.</p>"
            );
        }
        if standing.changed {
            let _ = writeln!(
                body,
                "<p class=\"banner\">The plan changed after version {} was approved.</p>",
                artifact.approved_version.unwrap_or_default()
            );
        }
    }
    if item.trashed.is_some() || item.stashed.is_some() {
        let away = if item.trashed.is_some() { "in the trash" } else { "stashed" };
        let _ = writeln!(body, "<p class=\"banner\">This artifact is {away}.</p>");
    }
    body.push_str("</header>\n");

    if let Some(artifact) = artifact.filter(|artifact| !artifact.steps.is_empty()) {
        body.push_str("<section><h2>Steps</h2>\n<ol class=\"steps\">\n");
        for step in &artifact.steps {
            let (step_title, rest) = step.text.split_once('\n').unwrap_or((step.text.as_str(), ""));
            let (mark, status) = match step.task.as_deref() {
                None => ("proposed", "to approve".to_string()),
                Some(uid) => match by_uid(uid) {
                    Some(task) => {
                        let state = State::of(task).map_or("note", State::word);
                        (state_class(state), format!("task {} \u{b7} {state}", task.id))
                    }
                    None => ("gone", "its task is not on this board".to_string()),
                },
            };
            let _ = write!(body, "<li class=\"step {mark}\"><span class=\"status\">{}</span> <span class=\"key\">{}</span> {}", esc(&status), esc(&step.key), esc(step_title.trim()));
            if !step.after.is_empty() {
                let _ = write!(body, " <span class=\"after\">after {}</span>", esc(&step.after.join(", ")));
            }
            if let Some(done_when) = &step.done_when {
                let _ = write!(body, "<div class=\"done-when\">Done when: {}</div>", esc(done_when));
            }
            if !rest.trim().is_empty() {
                let _ = write!(body, "<div class=\"step-text\">{}</div>", esc(rest.trim()));
            }
            body.push_str("</li>\n");
        }
        body.push_str("</ol></section>\n");
    }

    let _ = writeln!(body, "<section class=\"plan\"><h2>Plan</h2>\n{}</section>", markdown(plan));

    let uid = item.uid.as_deref().unwrap_or_default();
    let mut notes: Vec<&Item> = all.values().filter(|note| note.trashed.is_none() && note.attached_to.as_deref() == Some(uid)).collect();
    notes.sort_by_key(|note| note.timestamp);
    if !notes.is_empty() {
        body.push_str("<section><h2>Notes</h2>\n");
        for note in notes {
            let kind = match (&note.question, note.mark()) {
                (Some(question), _) if question.answer.is_none() => "question, open".to_string(),
                (Some(_), _) => "question, answered".to_string(),
                (None, Some(mark)) => mark.to_string(),
                (None, None) => "note".to_string(),
            };
            let answer = note
                .question
                .as_ref()
                .and_then(|question| question.answer.as_ref())
                .map(|answer| format!("<div class=\"answer\">Answer: {}</div>", esc(&answer.text)))
                .unwrap_or_default();
            let _ = writeln!(
                body,
                "<article class=\"note\"><p class=\"note-head\">{} \u{b7} {} \u{b7} {}</p><div class=\"note-text\">{}</div>{answer}</article>",
                note.id,
                esc(&kind),
                esc(&note.date),
                esc(note.description.trim())
            );
        }
        body.push_str("</section>\n");
    }
    let _ = writeln!(
        body,
        "<footer>Written by ekko {} from the board. It reloads by itself when the board changes; <code>ekko artifact {}</code> writes it again.</footer>",
        env!("CARGO_PKG_VERSION"),
        item.id
    );

    let version = format!("{:016x}", fnv(body.as_bytes()));
    let script = format!(
        "(function () {{\n  var version = \"{version}\";\n  window.ekkoArtifact = function (seen) {{ if (seen !== version) location.reload(); }};\n  setInterval(function () {{\n    var script = document.createElement(\"script\");\n    script.src = \"{}.js?t=\" + Date.now();\n    script.onload = script.onerror = function () {{ script.remove(); }};\n    document.head.appendChild(script);\n  }}, {POLL_MS});\n}})();",
        js_string(uid)
    );
    let html = format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>{} \u{b7} artifact {}</title>\n<style>{STYLE}</style>\n</head>\n<body>\n<main>\n{body}</main>\n<script>{script}</script>\n</body>\n</html>\n",
        esc(title.trim()),
        item.id
    );
    (html, version)
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
    let script = format!("ekkoArtifact(\"{version}\");\n");
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

/// Opens `path` in the default browser, as `cargo doc --open` does, without
/// waiting for it.
pub fn open(path: &Path) -> std::io::Result<()> {
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    std::process::Command::new(opener)
        .arg(path)
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
fn markdown(text: &str) -> String {
    use pulldown_cmark::{html, Event, Options, Parser};
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let events = Parser::new_ext(text, options).map(|event| match event {
        Event::Html(raw) | Event::InlineHtml(raw) => Event::Text(raw),
        other => other,
    });
    let mut out = String::new();
    html::push_html(&mut out, events);
    out
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

const STYLE: &str = "
:root { color-scheme: light dark; --fg: #1f2328; --muted: #656d76; --line: #d0d7de; --bg: #ffffff; --soft: #f6f8fa; --accent: #0969da; --warn: #fff8c5; --done: #1a7f37; }
@media (prefers-color-scheme: dark) { :root { --fg: #e6edf3; --muted: #8d96a0; --line: #30363d; --bg: #0d1117; --soft: #161b22; --accent: #4493f8; --warn: #3b2e00; --done: #3fb950; } }
body { margin: 0; background: var(--bg); color: var(--fg); font: 16px/1.6 system-ui, -apple-system, 'Segoe UI', sans-serif; }
main { max-width: 52rem; margin: 0 auto; padding: 2rem 1.25rem 4rem; }
h1 { font-size: 1.9rem; line-height: 1.25; margin: 0.2rem 0 0.4rem; }
h2 { font-size: 1.3rem; border-bottom: 1px solid var(--line); padding-bottom: 0.3rem; margin-top: 2.2rem; }
.where, .standing, footer, .note-head, .after, .key { color: var(--muted); }
.where { font-size: 0.85rem; margin: 0; }
.standing { margin: 0; }
.standing.open { color: var(--accent); }
.banner { background: var(--warn); border: 1px solid var(--line); border-radius: 6px; padding: 0.6rem 0.9rem; }
code, pre { font-family: ui-monospace, 'SFMono-Regular', Menlo, monospace; font-size: 0.9em; background: var(--soft); border-radius: 4px; }
code { padding: 0.1em 0.3em; }
pre { padding: 0.8rem; overflow-x: auto; }
pre code { padding: 0; background: none; }
table { border-collapse: collapse; }
th, td { border: 1px solid var(--line); padding: 0.3rem 0.6rem; }
a { color: var(--accent); }
.steps { padding-left: 1.6rem; }
.step { margin: 0.5rem 0; }
.status { display: inline-block; font-size: 0.8rem; border: 1px solid var(--line); border-radius: 999px; padding: 0 0.55rem; background: var(--soft); }
.step.done .status { color: var(--done); border-color: var(--done); }
.step.progress .status { color: var(--accent); border-color: var(--accent); }
.step.cancelled { text-decoration: line-through; color: var(--muted); }
.key { font-family: ui-monospace, monospace; font-size: 0.85rem; }
.done-when, .step-text { color: var(--muted); font-size: 0.92rem; white-space: pre-wrap; }
.note { border: 1px solid var(--line); border-radius: 6px; padding: 0.4rem 0.9rem; margin: 0.8rem 0; }
.note-head { font-size: 0.85rem; margin: 0.2rem 0; }
.note-text { white-space: pre-wrap; }
.answer { margin-top: 0.4rem; font-weight: 600; }
footer { margin-top: 3rem; font-size: 0.85rem; }
";

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
