//! Structured writes: the operations an agent sends, applied to a draft of
//! the board and written once.
//!
//! The CLI reads words out of the description it is given -- `@board`, `p:2`,
//! `d:2026-09-01` -- which is right at a prompt and wrong for a caller writing
//! prose: a note that mentions `@mentions` or `p:3` would lose those words to
//! board names and priorities. These take every field separately and never
//! look inside the text.
//!
//! Every operation lands on a `Draft`, a copy of the board taken under the
//! storage lock, and nothing reaches the disk until `commit`. So a batch of
//! twenty operations is one write, the dependency rule is checked once on the
//! board the whole batch would leave, and any refusal leaves the file exactly
//! as it was.

use std::path::Path;

use serde::{Deserialize, Deserializer};
use serde_json::{json, Value};

use crate::ekko::{
    holds, parse_due_date, person, phase_inversion, phase_order, remove_duplicates, uid_index, Ekko, EkkoError,
    Linked,
};
use crate::holder::Whose;
use crate::item::{Answer, Approving, Artifact, Cue, CueOn, How, Item, Knowledge, Linking, Proposal, Question, Review, Setting, State, Until, Wait};
use crate::storage::{ItemMap, LockGuard};

/// An item as a caller names it: a display id, a uid, or `$N` for the item
/// the N-th operation of the same batch created. Numbers and strings both.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Ref {
    Id(u32),
    Text(String),
}

impl Ref {
    fn text(&self) -> String {
        match self {
            Ref::Id(id) => id.to_string(),
            Ref::Text(text) => text.trim_start_matches('@').to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Task,
    Note,
    /// A note that hands a task over to the next session; see `Item::handoff`.
    Handoff,
    /// Notes of what stays true; see `Item::knowledge`.
    Decision,
    Gotcha,
    Procedure,
}

impl Kind {
    /// The lasting knowledge a note of this kind holds, if it holds any.
    pub fn knowledge(self) -> Option<Knowledge> {
        match self {
            Kind::Decision => Some(Knowledge::Decision),
            Kind::Gotcha => Some(Knowledge::Gotcha),
            Kind::Procedure => Some(Knowledge::Procedure),
            Kind::Task | Kind::Note | Kind::Handoff => None,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Create {
    /// A task when absent, or a note when `attached_to` is given: only a
    /// note is ever attached, so that is the one reading that can be meant.
    pub kind: Option<Kind>,
    pub text: String,
    #[serde(default)]
    pub boards: Vec<String>,
    pub priority: Option<i64>,
    pub due: Option<String>,
    /// Who the task is with; see `Item::with`.
    pub with: Option<String>,
    pub phase: Option<String>,
    #[serde(default)]
    pub blocked_by: Vec<Ref>,
    pub attached_to: Option<Ref>,
    /// The earlier note of the same kind a decision, gotcha or procedure
    /// replaces.
    pub supersedes: Option<Ref>,
    #[serde(default)]
    pub starred: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Edit {
    pub item: Ref,
    /// The whole new description.
    pub text: Option<String>,
    /// Replace one exact occurrence of `old` with `new`.
    pub replace: Option<Replace>,
    /// Add to the end of the description.
    pub append: Option<String>,
    /// Refuse the edit (STALE) unless the item still has this `updatedAt`.
    pub if_updated_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Replace {
    pub old: String,
    pub new: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Update {
    pub item: Ref,
    pub boards: Option<Vec<String>>,
    pub priority: Option<i64>,
    /// A date sets it, `null` clears it, absent leaves it.
    #[serde(default, deserialize_with = "present")]
    pub due: Option<Option<String>>,
    /// A name says who the task is with, `null` leaves it with nobody.
    #[serde(default, deserialize_with = "present")]
    pub with: Option<Option<String>>,
    /// A declared phase moves the item there, `null` to the project root.
    #[serde(default, deserialize_with = "present")]
    pub phase: Option<Option<String>>,
    pub starred: Option<bool>,
    /// Retypes a note: decision, gotcha or procedure, or `note` for an
    /// ordinary one.
    pub kind: Option<Kind>,
    /// Boards put on the item, and taken off it, beside the ones it has:
    /// unlike `boards`, which replaces them, two sessions changing them at
    /// once both land.
    #[serde(default)]
    pub add_boards: Vec<String>,
    #[serde(default)]
    pub remove_boards: Vec<String>,
    /// The item's updatedAt as the caller read it: STALE if it has changed.
    pub if_updated_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Link {
    pub item: Ref,
    /// Replaces what the item is blocked by; empty clears it.
    pub blocked_by: Option<Vec<Ref>>,
    /// The task a note explains; `null` detaches it.
    #[serde(default, deserialize_with = "present")]
    pub attached_to: Option<Option<Ref>>,
    /// The earlier note a decision, gotcha or procedure replaces; `null`
    /// makes it replace nothing.
    #[serde(default, deserialize_with = "present")]
    pub supersedes: Option<Option<Ref>>,
    /// Blockers put on the item, and taken off it, beside the ones it has:
    /// unlike `blocked_by`, which replaces them, two sessions linking at
    /// once both land.
    #[serde(default)]
    pub add_blocked_by: Vec<Ref>,
    #[serde(default)]
    pub remove_blocked_by: Vec<Ref>,
    /// The item's updatedAt as the caller read it: STALE if it has changed.
    pub if_updated_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetState {
    pub items: Vec<Ref>,
    pub state: String,
}

/// Questions for the user, put to them together, and the task they are about.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ask {
    pub questions: Vec<Inquiry>,
    pub about: Option<Ref>,
}

/// One question, and the answers it offers.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Inquiry {
    pub text: String,
    /// What the question is about, for a user who has not followed the work
    /// (task 979): shown under it, and kept in the board's note.
    #[serde(default)]
    pub explain: Option<String>,
    /// A quick, direct question -- push this? commit that? -- which needs no
    /// explain, recommended option, why or example.
    #[serde(default)]
    pub quick: bool,
    #[serde(default)]
    pub options: Vec<Choice>,
    /// The user may pick several of the options.
    #[serde(default)]
    pub multiple: bool,
    /// Proposes a cue for a gotcha, or turning its cue off (task 805): ekko
    /// adds what the cue refuses under the session's explain, and the user's
    /// first answer in ekko's menu applies it (task 1044).
    #[serde(default)]
    pub cue: Option<CueAsk>,
    /// The code a guard's refusal gave: the question asks the user to let
    /// that call through once, and ekko quotes the call as refused under the
    /// session's explain.
    #[serde(default)]
    pub allow: Option<String>,
    /// A project to link this board's project with, both ways (task 811):
    /// ekko adds what the link does under the session's explain, and the
    /// user's first answer in ekko's menu makes it (task 1044).
    #[serde(default)]
    pub link_project: Option<String>,
    /// An artifact whose plan the user is asked to approve (task 1019): ekko
    /// adds the tasks it makes of the steps not approved yet under the
    /// session's explain, and the user's first answer in ekko's menu makes
    /// them.
    #[serde(default)]
    pub approve: Option<Ref>,
}

impl Inquiry {
    /// Whether it proposes what the user's answer applies; see
    /// `Question::proposes`.
    pub fn proposes(&self) -> bool {
        self.proposals() > 0
    }

    /// How many of cue, allow, link_project and approve it carries.
    fn proposals(&self) -> usize {
        [self.cue.is_some(), self.allow.is_some(), self.link_project.is_some(), self.approve.is_some()].into_iter().filter(|given| *given).count()
    }
}

/// What the MCP tool `artifact` writes (task 1019): a new artifact, from its
/// plan, or the steps of one there.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactSpec {
    /// The artifact to change; absent, one is created.
    #[serde(default)]
    pub artifact: Option<Ref>,
    /// The plan, to create one: its title, then `crate::artifact::HEADINGS`.
    #[serde(default)]
    pub text: Option<String>,
    /// Every step, in order, written over the steps there.
    #[serde(default)]
    pub steps: Option<Vec<crate::artifact::StepSpec>>,
    #[serde(default)]
    pub boards: Vec<String>,
    #[serde(default)]
    pub priority: Option<i64>,
    #[serde(default)]
    pub phase: Option<String>,
}

impl ArtifactSpec {
    /// Whether it names an artifact and writes nothing: a read.
    pub fn reads(&self) -> bool {
        self.artifact.is_some() && self.text.is_none() && self.steps.is_none() && self.boards.is_empty() && self.priority.is_none() && self.phase.is_none()
    }
}

/// A cue proposed through ask: for `gotcha`, refuse the calls of `command`
/// whose arguments hold `words`, run in `folder` -- or, with `off`, turn the
/// gotcha's cue off.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CueAsk {
    pub gotcha: Ref,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub words: Vec<String>,
    #[serde(default)]
    pub folder: Option<String>,
    #[serde(default)]
    pub off: bool,
}

impl CueAsk {
    /// The cue it proposes, checked; `None` with `off`.
    fn cue(&self) -> Result<Option<Cue>, EkkoError> {
        if self.off {
            if self.command.is_some() || !self.words.is_empty() || self.folder.is_some() {
                return Err(invalid("cue with off takes no command, words or folder: it turns the gotcha's cue off"));
            }
            return Ok(None);
        }
        let command = self.command.as_deref().map(str::trim).unwrap_or_default();
        if command.is_empty() || command.contains(char::is_whitespace) || command.contains('/') {
            return Err(invalid(
                "cue needs command: one word, as a call names it with no directory -- gh, not /usr/bin/gh or gh api",
            ));
        }
        let mut words = Vec::new();
        for word in &self.words {
            let word = word.trim();
            if word.is_empty() {
                return Err(invalid("cue's words are each a word its arguments hold, never empty"));
            }
            if !words.iter().any(|kept: &String| kept == word) {
                words.push(word.to_string());
            }
        }
        let folder = match self.folder.as_deref().map(str::trim).filter(|folder| !folder.is_empty()) {
            None => None,
            Some(folder) => {
                let home = std::env::home_dir().unwrap_or_default();
                let path = crate::paths::expand_tilde(&home, folder);
                if !path.is_absolute() {
                    return Err(invalid(format!("cue's folder is absolute, or starts with ~: {folder}")));
                }
                Some(crate::paths::resolve_path(&home, Path::new("/"), &path.to_string_lossy()).to_string_lossy().into_owned())
            }
        };
        Ok(Some(Cue { command: command.to_string(), words, folder, unknown: Default::default() }))
    }
}

/// One answer a question offers: a few words, what choosing it means, and
/// what to show beside the options while it is focused -- why pick it, an
/// example of it, a preview.
#[derive(Debug, Clone, Default, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Choice {
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    /// The answer the asking session recommends (task 979): the menu opens
    /// on it, and the board's note marks it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub recommended: bool,
    /// Why one would pick it, and how it differs from the others.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// What picking it looks like in practice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub example: Option<String>,
}

/// The user's answer to a question.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    pub question: Ref,
    pub text: String,
}

/// A session waiting on an item, or, with `cancel`, no longer waiting on it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WaitOn {
    pub item: Ref,
    /// done, free or answered: by default answered for a question and done
    /// for a task.
    pub until: Option<String>,
    /// What the session will do once the wait is over.
    pub text: Option<String>,
    #[serde(default)]
    pub cancel: bool,
}

/// What `Draft::wait` did.
#[derive(Debug, PartialEq)]
pub enum Waited {
    /// The note it recorded the wait in.
    Recorded(u32),
    /// Nothing: the item, by display id, already is where the wait would
    /// end, as `How` says.
    Met(u32, How),
    /// Nothing: this session already waits on it for the same, in this note.
    Already(u32),
}

/// One operation of a batch, tagged by `op`.
#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    Create(Create),
    SetState(SetState),
    Edit(Edit),
    Update(Update),
    Link(Link),
}

impl Op {
    /// What the operation writes into a description: a create's text, and an
    /// edit's whole text, replacement or append.
    fn texts(&self) -> Vec<&str> {
        match self {
            Op::Create(spec) => vec![spec.text.as_str()],
            Op::Edit(spec) => spec
                .text
                .iter()
                .chain(spec.replace.iter().map(|replace| &replace.new))
                .chain(spec.append.iter())
                .map(String::as_str)
                .collect(),
            Op::SetState(_) | Op::Update(_) | Op::Link(_) => Vec::new(),
        }
    }
}

/// The numbers a text writes as `$N`: a dollar sign, then digits that end the
/// word -- `$2`, `$2,` and `$2.` but not `$2.50` or `$2nd`.
fn dollar_numbers(text: &str) -> Vec<usize> {
    let mut numbers = Vec::new();
    let mut from = 0;
    while let Some(offset) = text[from..].find('$') {
        let start = from + offset + 1;
        let end = start + text[start..].bytes().take_while(u8::is_ascii_digit).count();
        from = end;
        let mut after = text[end..].chars();
        let next = after.next();
        let decimal = matches!(next, Some('.' | ',')) && after.next().is_some_and(|c| c.is_ascii_digit());
        let word = next.is_some_and(|c| c.is_alphanumeric() || c == '_');
        if end > start && !decimal && !word {
            if let Ok(n) = text[start..end].parse() {
                numbers.push(n);
            }
        }
    }
    numbers
}

/// Tells a field given as `null` from a field left out: `Some(None)` for the
/// first, and `None`, through `default`, for the second.
fn present<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

fn invalid(message: impl Into<String>) -> EkkoError {
    EkkoError::InvalidInput(message.into())
}

/// The refusal of a note given someone to be with.
const NOTE_WITH: &str = "A note is with nobody; only a task is with someone";

/// The refusal of a question proposing more than one thing for the user's
/// answer to apply.
const ONE_PROPOSAL: &str = "a question takes one of cue, allow, link_project and approve";

/// A priority as given, read wide so that -1 or 300 is refused as a priority
/// out of range, as 0 and 4 are, rather than as JSON that is not a u8.
fn priority_of(priority: i64) -> Result<u8, EkkoError> {
    u8::try_from(priority).ok().filter(|p| (1..=3).contains(p)).ok_or(EkkoError::InvalidPriority)
}

/// Refuses a board name the CLI could not address: empty, or @ alone, or
/// holding a space, which the terminal would read as two words. The default
/// board keeps its name, space and all.
fn addressable(names: &[String]) -> Result<(), EkkoError> {
    for name in names {
        let bare = name.trim().trim_start_matches('@');
        if bare == "My Board" {
            continue;
        }
        if bare.is_empty() || bare.contains(char::is_whitespace) {
            return Err(invalid(format!("{name:?} cannot be a board name: a board is one word, as @name, so the terminal can address it")));
        }
    }
    Ok(())
}

/// `["coding", "@reviews"]` to `["@coding", "@reviews"]`, the default board
/// by either of its names, and the default board when there are none.
fn boards(names: &[String]) -> Vec<String> {
    let named = named(names);
    if named.is_empty() { vec!["My Board".to_string()] } else { remove_duplicates(named) }
}

/// The boards `names` name, as they are stored, and none for none: what
/// add_boards and remove_boards change, which an empty list must not turn
/// into the default board (task 1011).
fn named(names: &[String]) -> Vec<String> {
    names
        .iter()
        .map(|name| name.trim())
        .filter(|name| !name.is_empty())
        .map(|name| match name.trim_start_matches('@') {
            "myboard" | "My Board" => "My Board".to_string(),
            bare => format!("@{bare}"),
        })
        .collect()
}

/// How a refusal names the items it involves.
///
/// An item already on the board is its display id. One this draft created
/// exists nowhere -- a refusal writes nothing -- so the id it held in the
/// draft would name nothing, or worse, whatever takes that number next. It is
/// named by the operation that created it instead, the way `$N` refers to it.
pub struct Names {
    existing: std::collections::HashSet<u32>,
    created: Vec<Option<u32>>,
}

impl Names {
    pub fn name(&self, id: u32) -> String {
        if self.existing.contains(&id) {
            return id.to_string();
        }
        match self.created.iter().position(|created| *created == Some(id)) {
            Some(at) => format!("${} (from operation {})", at + 1, at + 1),
            None => "the new item".to_string(),
        }
    }
}

/// A board being written: the lock, what storage held when it was taken, and
/// the copy every operation changes.
pub struct Draft<'a> {
    ekko: &'a Ekko,
    _lock: LockGuard<'a>,
    /// The board as the lock found it: the version every read shares, which
    /// only `data` is copied from to change (task 844).
    before: std::sync::Arc<ItemMap>,
    data: ItemMap,
    phases: Vec<String>,
    /// For each operation applied so far, the item it created, if any --
    /// what `$N` resolves through.
    created: Vec<Option<u32>>,
    /// For each operation applied so far, what it wrote into a description,
    /// which `commit` reads for a `$N` left as written.
    texts: Vec<Vec<String>>,
    /// Tasks this draft set in progress, whether or not they already were:
    /// a claim, which `commit` settles against whoever holds each one.
    claims: Vec<u32>,
    /// What an operation found worth saying though it wrote nothing wrong.
    notices: Vec<String>,
}

/// What a commit wrote, what `force` pushed past to write it, and the tasks
/// whose readiness it changed.
pub struct Committed {
    pub data: ItemMap,
    pub overridden: Linked,
    pub reopened: Linked,
    /// Tasks that waited on open work before the write and no longer do.
    pub released: Vec<u32>,
    /// Tasks that were free to start before the write and now wait.
    pub blocked: Vec<u32>,
    /// What the write found about who holds the tasks it set in progress:
    /// already its own, or taken over from a session that is gone.
    pub notices: Vec<String>,
}

impl<'a> Draft<'a> {
    pub fn open(ekko: &'a Ekko) -> Result<Self, EkkoError> {
        let lock = ekko.storage.acquire_lock()?;
        let before = ekko.storage.get_shared()?;
        let phases = ekko.storage.get_phases()?;
        Ok(Draft {
            ekko,
            _lock: lock,
            data: ItemMap::clone(&before),
            before,
            phases,
            created: Vec::new(),
            texts: Vec::new(),
            claims: Vec::new(),
            notices: Vec::new(),
        })
    }

    /// How a refusal of this draft should name its items; see `Names`.
    pub fn names(&self) -> Names {
        Names { existing: self.before.keys().copied().collect(), created: self.created.clone() }
    }

    fn resolve(&self, reference: &Ref) -> Result<u32, EkkoError> {
        let raw = reference.text();
        if let Some(position) = raw.strip_prefix('$') {
            let op: usize = position
                .parse()
                .map_err(|_| invalid(format!("{raw} is not a reference: $1 is the item the first operation created")))?;
            return op
                .checked_sub(1)
                .and_then(|at| self.created.get(at).copied().flatten())
                .ok_or_else(|| invalid(format!("{raw} does not name an item created earlier in this batch")));
        }
        Ok(self.ekko.validate_ids(&[raw], &self.data)?[0])
    }

    fn resolve_all(&self, references: &[Ref]) -> Result<Vec<u32>, EkkoError> {
        let mut ids = Vec::new();
        for reference in references {
            let id = self.resolve(reference)?;
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        Ok(ids)
    }

    /// A comment on the artifact `on` (task 1105): a note attached to it,
    /// holding `comment`, made on the version the plan is at, and on words
    /// that version holds. Its text says what the comment says, or what it
    /// suggests when it says nothing else.
    pub fn comment(&mut self, on: &Ref, text: &str, comment: crate::item::Comment) -> Result<u32, EkkoError> {
        let id = self.resolve(on)?;
        let item = &self.data[&id];
        let Some(artifact) = item.artifact.as_deref().filter(|_| item.trashed.is_none()) else {
            return Err(invalid(format!("{id} is not an artifact")));
        };
        if comment.version != artifact.version {
            return Err(invalid(format!("the plan is at version {} now, and the comment was made on version {}", artifact.version, comment.version)));
        }
        // The words as written, or as the page shows them, which a selection
        // there takes without the Markdown around them.
        let held = |exact: &str| item.description.contains(exact) || crate::artifact::shown_words(&item.description).contains(&crate::artifact::collapsed(exact));
        if let Some(quote) = comment.quote.as_ref().filter(|quote| quote.exact.trim().is_empty() || !held(&quote.exact)) {
            return Err(invalid(format!("version {} of the plan does not hold {:?}", artifact.version, quote.exact)));
        }
        if let Some(step) = comment.step.as_ref().filter(|step| !artifact.steps.iter().any(|known| &known.key == *step)) {
            return Err(invalid(format!("the plan has no step {step}")));
        }
        if let Some(why) = crate::artifact::theme_refused(comment.theme.as_deref(), comment.color.as_deref()) {
            return Err(invalid(why));
        }
        // Pending as written: a review or Send now sends it (task 1106), and
        // a session resolves it.
        if comment.sent.is_some() || comment.resolved.is_some() {
            return Err(invalid("a comment is written pending: a review or Send now sends it, and the session resolves it"));
        }
        let text = match (text.trim(), &comment.quote, &comment.replacement) {
            ("", Some(quote), Some(replacement)) if replacement.is_empty() => format!("Delete: {}", quote.exact),
            ("", Some(quote), Some(replacement)) => format!("Replace {} with {replacement}", quote.exact),
            (text, _, _) => text.to_string(),
        };
        let spec = Create {
            kind: None,
            text,
            boards: Vec::new(),
            priority: None,
            due: None,
            with: None,
            phase: None,
            blocked_by: Vec::new(),
            attached_to: Some(Ref::Id(id)),
            supersedes: None,
            starred: false,
        };
        let note = self.create(&spec)?;
        self.item(note).comment = Some(Box::new(comment));
        Ok(note)
    }

    /// The person's comment `uid` on the artifact `on`, by id: refused when
    /// it is no comment of that artifact, is in the trash, or a session
    /// wrote it, since a session's words are its own to change.
    fn own_comment(&self, on: &Ref, uid: &str) -> Result<u32, EkkoError> {
        let artifact = self.resolve(on)?;
        let of = self.data[&artifact].uid.clone();
        let found = self.data.values().find(|note| note.uid.as_deref() == Some(uid));
        let Some(note) = found.filter(|note| note.comment.is_some() && note.trashed.is_none() && of.is_some() && note.attached_to == of) else {
            return Err(invalid(format!("{uid} is no comment on artifact {artifact}")));
        };
        if note.created_by.as_ref().is_some_and(|by| by.pid.is_some()) {
            return Err(invalid(format!("comment {} is a session's, and only the session changes it", note.id)));
        }
        Ok(note.id)
    }

    /// Changes the person's comment `uid` on the artifact `on` (task 1213):
    /// its text, its theme or both; what is `None` stays as it is.
    pub fn edit_comment(&mut self, on: &Ref, uid: &str, text: Option<&str>, theme: Option<&str>, color: Option<&str>) -> Result<u32, EkkoError> {
        let id = self.own_comment(on, uid)?;
        if let Some(why) = crate::artifact::theme_refused(theme, color) {
            return Err(invalid(why));
        }
        if let Some(text) = text {
            if text.trim().is_empty() {
                return Err(EkkoError::MissingDesc);
            }
            crate::ekko::fits("description", text)?;
        }
        let note = self.item(id);
        if let Some(text) = text {
            note.description = text.to_string();
        }
        let comment = note.comment.as_mut().expect("own_comment found a comment");
        if let Some(theme) = theme {
            comment.theme = Some(theme.to_string());
        }
        if let Some(color) = color {
            comment.color = Some(color.to_string());
        }
        Ok(id)
    }

    /// Puts the person's comment `uid` on the artifact `on` in the trash,
    /// which keeps it 30 days (task 1213).
    pub fn trash_comment(&mut self, on: &Ref, uid: &str) -> Result<u32, EkkoError> {
        let id = self.own_comment(on, uid)?;
        self.item(id).trashed = Some(chrono::Local::now().timestamp_millis());
        Ok(id)
    }

    /// Sends the person's pending comment `uid` on the artifact `on` alone,
    /// as GitHub's single comment goes, outside any review (task 1106).
    pub fn send_comment(&mut self, on: &Ref, uid: &str) -> Result<u32, EkkoError> {
        let id = self.own_comment(on, uid)?;
        let comment = self.item(id).comment.as_mut().expect("own_comment found a comment");
        if comment.sent.is_some() || comment.resolved.is_some() {
            return Err(invalid(format!("comment {id} is not pending: it was sent already")));
        }
        comment.sent = Some(chrono::Local::now().timestamp_millis());
        Ok(id)
    }

    /// The person's review of the artifact `on`, sent from its page (task
    /// 1106), as GitHub's reviews go: a note holding `text`, its summary,
    /// and the verdict, which sends every comment of theirs still pending
    /// there. While a question asks the user to approve the plan, Approve
    /// answers it with the answer that approves, which makes the tasks, and
    /// Request changes with the other, naming the review; Comment leaves it
    /// open. Refused when the plan moved on from `version`, when Approve has
    /// no question asking to approve that version, when the artifact is
    /// closed, and when Request changes or Comment has nothing to say.
    pub fn review(&mut self, on: &Ref, verdict: &str, text: &str, version: u32) -> Result<u32, EkkoError> {
        if !self.ekko.actor.as_ref().is_none_or(|actor| actor.is_person()) {
            return Err(invalid("a review is the user's, sent from the artifact's page"));
        }
        if !Review::VERDICTS.contains(&verdict) {
            return Err(invalid(format!("{verdict:?} is no verdict: a review is approve, changes or comment")));
        }
        crate::ekko::fits("review", text)?;
        let id = self.resolve(on)?;
        let item = &self.data[&id];
        let (Some(artifact), Some(uid)) = (item.artifact.as_deref().filter(|_| item.trashed.is_none()), item.uid.clone()) else {
            return Err(invalid(format!("{id} is not an artifact")));
        };
        if version != artifact.version {
            return Err(invalid(format!("the plan is at version {} now, and the review was made on version {version}", artifact.version)));
        }
        if !State::of(item).is_some_and(State::is_open) {
            return Err(invalid(format!("artifact {id} is {}: a review is of a plan still worked", State::of(item).map_or("closed", State::word))));
        }
        // Approve answers the newest question about this version, Request
        // changes the newest.
        let asking = crate::artifact::approvals_asked(item, &self.data);
        let answers = match verdict {
            Review::APPROVE => {
                let about = |note: &&Item| note.question.as_ref().and_then(|question| question.approve.as_ref()).is_some_and(|approve| approve.version == version);
                match (asking.iter().copied().find(about), asking.first()) {
                    (Some(question), _) => Some(question.id),
                    (None, Some(question)) => {
                        let asked = question.question.as_ref().and_then(|question| question.approve.as_ref()).map_or(0, |approve| approve.version);
                        return Err(invalid(format!("question {} asks to approve version {asked}, and the plan is at version {version}: the session asks again", question.id)));
                    }
                    (None, None) => return Err(invalid("no question asks to approve this plan: the session asks with ask's approve when it is ready")),
                }
            }
            Review::CHANGES => asking.first().map(|question| question.id),
            _ => None,
        };
        // The person's comments still pending there, oldest first.
        let mut pending: Vec<(i64, u32, String)> = self
            .data
            .values()
            .filter(|note| note.attached_to.as_deref() == Some(uid.as_str()) && note.trashed.is_none())
            .filter(|note| note.created_by.as_ref().is_none_or(|by| by.pid.is_none()))
            .filter(|note| note.comment.as_ref().is_some_and(|comment| comment.sent.is_none() && comment.resolved.is_none()))
            .filter_map(|note| Some((note.timestamp, note.id, note.uid.clone()?)))
            .collect();
        pending.sort();
        let text = text.trim();
        if verdict != Review::APPROVE && text.is_empty() && pending.is_empty() {
            return Err(invalid("nothing to send: write what the review says, or comment on the plan's words first"));
        }
        let comments = match pending.len() {
            0 => String::new(),
            1 => format!(", sending comment {}", pending[0].1),
            _ => format!(", sending comments {}", pending.iter().map(|(_, id, _)| id.to_string()).collect::<Vec<_>>().join(", ")),
        };
        let said = crate::artifact::review_said(verdict, version, &pending.iter().map(|(_, id, _)| *id).collect::<Vec<_>>());
        let spec = Create {
            kind: None,
            text: if text.is_empty() { said.clone() } else { text.to_string() },
            boards: Vec::new(),
            priority: None,
            due: None,
            with: None,
            phase: None,
            blocked_by: Vec::new(),
            attached_to: Some(Ref::Id(id)),
            supersedes: None,
            starred: false,
        };
        let note = self.create(&spec)?;
        let now = chrono::Local::now().timestamp_millis();
        for (_, comment, _) in &pending {
            if let Some(comment) = self.item(*comment).comment.as_mut() {
                comment.sent = Some(now);
            }
        }
        let mut answered = None;
        if let Some(question) = answers {
            let (approves, other) = crate::artifact::approval_answers(&self.data[&question]);
            let label = if verdict == Review::APPROVE { approves } else { other };
            let named = format!("{} on the artifact's page, in review {note}", if verdict == Review::APPROVE { "approved" } else { "changes requested" });
            let summary = if text.is_empty() { comments.trim_start_matches(", ").to_string() } else { crate::ekko::title(text).to_string() };
            let answer = if summary.is_empty() { format!("{label}{}{named}", crate::menu::NOTE) } else { format!("{label}{}{named}: {summary}", crate::menu::NOTE) };
            self.answer(&Ref::Id(question), &answer)?;
            answered = self.data[&question].uid.clone();
        }
        let comments = pending.into_iter().map(|(_, _, uid)| uid).collect();
        self.item(note).review = Some(Box::new(Review { verdict: verdict.to_string(), version, comments, answered, resolved: None, unknown: Default::default() }));
        Ok(note)
    }

    fn item(&mut self, id: u32) -> &mut Item {
        self.data.get_mut(&id).expect("ids are resolved against the draft")
    }

    /// Applies one operation to the draft, returning the ids it touched.
    pub fn apply(&mut self, op: &Op) -> Result<Vec<u32>, EkkoError> {
        let result = match op {
            Op::Create(spec) => self.create(spec).map(|id| vec![id]),
            Op::SetState(spec) => self.set_state(&spec.items, &spec.state),
            Op::Edit(spec) => self.edit(spec),
            Op::Update(spec) => self.update(spec),
            Op::Link(spec) => self.link(spec),
        };
        let created = match (op, &result) {
            (Op::Create(_), Ok(ids)) => ids.first().copied(),
            _ => None,
        };
        self.created.push(created);
        self.texts.push(op.texts().into_iter().map(str::to_string).collect());
        result
    }

    pub fn create(&mut self, spec: &Create) -> Result<u32, EkkoError> {
        let text = spec.text.trim();
        if text.is_empty() {
            return Err(EkkoError::MissingDesc);
        }
        crate::ekko::fits("description", text)?;
        let kind = spec.kind.unwrap_or(if spec.attached_to.is_some() { Kind::Note } else { Kind::Task });
        if kind == Kind::Task {
            crate::ekko::titled(text)?;
        }
        if kind != Kind::Task {
            for (field, given) in [
                ("priority", spec.priority.is_some()),
                ("due date", spec.due.is_some()),
                ("blocked_by", !spec.blocked_by.is_empty()),
            ] {
                if given {
                    return Err(invalid(format!("A note has no {field}; only a task does")));
                }
            }
            if spec.with.is_some() {
                return Err(invalid(NOTE_WITH));
            }
        }
        if kind == Kind::Handoff && spec.attached_to.is_none() {
            return Err(invalid("A handoff is attached_to the task it hands over"));
        }
        if spec.supersedes.is_some() && kind.knowledge().is_none() {
            return Err(invalid("Only a decision, a gotcha or a procedure supersedes an earlier note"));
        }
        let priority = priority_of(spec.priority.unwrap_or(1))?;
        let due = match &spec.due {
            Some(date) => Some(parse_due_date(&format!("d:{date}")).ok_or_else(|| EkkoError::InvalidDueDate(date.clone()))?),
            None => None,
        };
        let phase = self.declared_phase(spec.phase.as_deref())?;
        let with = spec.with.as_deref().map(person).transpose()?;

        addressable(&spec.boards)?;
        let id = self.ekko.generate_id(&self.data);
        let mut item = self.ekko.authored(match kind {
            Kind::Task => Item::new_task(id, text.to_string(), boards(&spec.boards), priority),
            Kind::Note | Kind::Handoff | Kind::Decision | Kind::Gotcha | Kind::Procedure => {
                Item::new_note(id, text.to_string(), boards(&spec.boards))
            }
        });
        item.due_date = due;
        item.with = with;
        item.phase = phase;
        item.is_starred = spec.starred;
        item.knowledge = kind.knowledge();
        self.data.insert(id, item);

        // Relations after the item is in the draft: both checks read it there.
        if !spec.blocked_by.is_empty() {
            let blockers = self.resolve_all(&spec.blocked_by)?;
            let uids = self.ekko.blocker_uids(&self.data, &self.phases, id, &blockers)?;
            self.item(id).blocked_by = Some(uids);
        }
        if let Some(target) = &spec.attached_to {
            let target = self.resolve(target)?;
            let attached = Ekko::attach_target(&self.data, id, Some(target))?;
            self.item(id).attached_to = attached.map(|(_, uid)| uid);
            if kind == Kind::Handoff {
                self.hand_over(id, target)?;
            }
        }
        if let Some(older) = &spec.supersedes {
            let older = self.resolve(older)?;
            let names = self.names();
            supersede(&mut self.data, id, Some(older), &|id| names.name(id))?;
        }
        Ok(id)
    }

    /// Asks the user `text`: a note that waits for an answer, attached to the
    /// task it is about, if any, and carrying the session that asked and the
    /// board's revision.
    pub fn ask(&mut self, text: &str, about: Option<&Ref>) -> Result<u32, EkkoError> {
        let spec = Create {
            kind: Some(Kind::Note),
            text: text.to_string(),
            boards: Vec::new(),
            priority: None,
            due: None,
            with: None,
            phase: None,
            blocked_by: Vec::new(),
            attached_to: about.cloned(),
            supersedes: None,
            starred: false,
        };
        let id = self.create(&spec)?;
        let now = chrono::Local::now().timestamp_millis();
        let asked_by = self.ekko.actor.as_ref().filter(|actor| !actor.is_person()).map(|actor| actor.holder(now));
        // The revision is the write's own, stamped as it is saved.
        self.item(id).question =
            Some(Box::new(Question { asked_by, rev: 0, answer: None, cue: None, allow: None, link: None, applies: None, approve: None, unknown: Default::default() }));
        Ok(id)
    }

    /// Records `text` as the answer to `question`, with who recorded it and
    /// when, which closes it. A question is answered once: asking again is
    /// how a newer answer is sought, so the first is never lost.
    pub fn answer(&mut self, question: &Ref, text: &str) -> Result<u32, EkkoError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(invalid("An answer needs its text"));
        }
        crate::ekko::fits("answer", text)?;
        let id = self.resolve(question)?;
        let Some(asked) = self.data[&id].question.clone() else {
            return Err(invalid(format!("{id} is not a question; ask records one")));
        };
        if let Some(given) = &asked.answer {
            return Err(invalid(format!("{id} was already answered: {}", given.text)));
        }
        // A cue, a refused call or a link is the user's to settle (tasks 805 and
        // 811): an answer a session wrote would lift a guard on its own calls,
        // or open another board to it.
        let person = self.ekko.actor.as_ref().is_none_or(|actor| actor.is_person());
        if asked.proposes() && !person {
            // Named, since the question may be on a board linked to the
            // session's, and the same id on another board is another item.
            let board = self.ekko.linkable.as_ref().map(|(_, project)| format!("--project {} ", project.name)).unwrap_or_default();
            return Err(invalid(format!(
                "{id} is the user's to answer, in ekko's menu: an answer a session records never turns a cue on, \
                 lets a refused call through nor links two boards. They can open it with ekko {board}--answer {id} in a terminal"
            )));
        }
        let now = chrono::Local::now().timestamp_millis();
        let by = self.ekko.actor.as_ref().map(|actor| actor.holder(now));
        let answer = Answer { text: text.to_string(), by, at: now, rev: 0, unknown: Default::default() };
        let proposal = asked.cue.clone();
        let linking = asked.link.clone();
        let applies = asked.applies.clone();
        let approving = asked.approve.clone();
        self.item(id).question = Some(Box::new(Question { answer: Some(answer), ..*asked }));
        if let Some(proposal) = proposal {
            self.settle_proposal(id, &proposal, applies.as_deref(), text, now);
        }
        if let Some(linking) = linking {
            self.settle_link(&linking, applies.as_deref(), text);
        }
        if let Some(approving) = approving {
            self.settle_approval(id, &approving, applies.as_deref(), text);
        }
        Ok(id)
    }

    /// Makes tasks of the steps question `id` asked the user to approve
    /// (task 1019), when their answer is the one that approves: `applies`,
    /// or on a question without it `crate::artifact::APPROVE`, a note added
    /// to it or not. Each step becomes a task, in the plan's order, blocked
    /// by the tasks of the steps it waits on and blocking the artifact.
    /// Nothing is made when the plan changed since the question was asked,
    /// or the artifact is closed, and a notice says so; nor, all of it, when
    /// a task cannot be made.
    fn settle_approval(&mut self, id: u32, approving: &Approving, applies: Option<&str>, answer: &str) {
        if crate::menu::picked(answer) != applies.unwrap_or(crate::artifact::APPROVE) {
            return;
        }
        let target = self.data.values().find(|item| item.uid.as_deref() == Some(approving.artifact.as_str())).map(|item| item.id);
        let Some((target, plan)) = target.and_then(|target| Some((target, self.data[&target].artifact.as_deref()?))) else {
            self.notices.push(format!("the artifact question {id} asks to approve is no longer on the board as one: no task was made"));
            return;
        };
        if plan.version != approving.version {
            self.notices.push(format!(
                "artifact {target}'s plan changed after question {id} was asked, from version {} to {}: no task was made, so ask again",
                approving.version, plan.version
            ));
            return;
        }
        let state = State::of(&self.data[&target]);
        if !state.is_some_and(State::is_open) {
            self.notices.push(format!("artifact {target} is {}: no task was made", state.map_or("closed", State::word)));
            return;
        }
        let saved = self.data.clone();
        match self.make_steps(target, approving) {
            Ok(made) if made.is_empty() => self.notices.push(format!("artifact {target} had no step left to make a task of")),
            Ok(made) => {
                let ids: Vec<String> = made.iter().map(u32::to_string).collect();
                self.notices.push(format!(
                    "artifact {target}'s plan is approved: its steps are tasks {}, in its order, and they block it",
                    ids.join(", ")
                ));
            }
            Err(error) => {
                self.data = saved;
                self.notices.push(format!("no task was made for artifact {target}'s plan: {error}"));
            }
        }
    }

    /// The tasks `approving` makes of artifact `target`'s steps, in order,
    /// each recorded on its step and blocking the artifact.
    fn make_steps(&mut self, target: u32, approving: &Approving) -> Result<Vec<u32>, EkkoError> {
        let artifact = self.data[&target].clone();
        let Some(plan) = artifact.artifact.as_deref() else { return Ok(Vec::new()) };
        let mut tasks: std::collections::HashMap<String, String> =
            plan.steps.iter().filter_map(|step| Some((step.key.clone(), step.task.clone()?))).collect();
        let mut made: Vec<(String, u32, String)> = Vec::new();
        for key in &approving.steps {
            let Some(step) = plan.steps.iter().find(|step| &step.key == key && step.task.is_none()) else { continue };
            let mut text = step.text.clone();
            if let Some(done_when) = &step.done_when {
                text.push_str(&format!("\nDone when: {done_when}"));
            }
            text.push_str(&format!("\nStep {} of artifact {target}.", step.key));
            let blocked_by = step.after.iter().filter_map(|key| tasks.get(key)).map(|uid| Ref::Text(uid.clone())).collect();
            let spec = Create {
                kind: Some(Kind::Task),
                text,
                boards: artifact.boards.clone(),
                priority: artifact.priority.map(i64::from),
                due: None,
                with: None,
                phase: artifact.phase.clone(),
                blocked_by,
                attached_to: None,
                supersedes: None,
                starred: false,
            };
            let id = self.create(&spec)?;
            let uid = self.data[&id].uid.clone().ok_or_else(|| invalid(format!("task {id} was made without a uid")))?;
            tasks.insert(step.key.clone(), uid.clone());
            made.push((step.key.clone(), id, uid));
        }
        let item = self.item(target);
        if let Some(plan) = item.artifact.as_mut() {
            for (key, _, uid) in &made {
                if let Some(step) = plan.steps.iter_mut().find(|step| &step.key == key) {
                    step.task = Some(uid.clone());
                }
            }
            plan.approved_version = Some(approving.version);
        }
        let blockers = item.blocked_by.get_or_insert_with(Vec::new);
        for (_, _, uid) in &made {
            if !blockers.contains(uid) {
                blockers.push(uid.clone());
            }
        }
        Ok(made.into_iter().map(|(_, id, _)| id).collect())
    }

    /// Writes an artifact (task 1019): a new one from `spec.text`, its plan,
    /// or the steps of `spec.artifact` over those it has. Its text changes
    /// with edit, as any task's does, and each change is a new version of
    /// the plan (`crate::artifact::keep_versions`).
    pub fn artifact(&mut self, spec: &ArtifactSpec) -> Result<u32, EkkoError> {
        let Some(target) = &spec.artifact else {
            let text = spec.text.as_deref().map(str::trim).unwrap_or_default();
            if let Some(why) = crate::artifact::unplanned(text) {
                return Err(invalid(why));
            }
            let steps = crate::artifact::steps(&[], spec.steps.as_deref().unwrap_or_default()).map_err(invalid)?;
            let create = Create {
                kind: Some(Kind::Task),
                text: text.to_string(),
                boards: spec.boards.clone(),
                priority: spec.priority,
                due: None,
                with: None,
                phase: spec.phase.clone(),
                blocked_by: Vec::new(),
                attached_to: None,
                supersedes: None,
                starred: false,
            };
            let id = self.create(&create)?;
            self.item(id).artifact =
                Some(Box::new(Artifact { steps, version: 1, earlier: Vec::new(), approved_version: None, unknown: Default::default() }));
            return Ok(id);
        };
        let id = self.resolve(target)?;
        if spec.text.is_some() || !spec.boards.is_empty() || spec.priority.is_some() || spec.phase.is_some() {
            return Err(invalid(
                "the artifact tool changes an artifact's steps; its text changes with edit, and its boards, priority and phase with update, as any task's",
            ));
        }
        let Some(plan) = self.data[&id].artifact.as_deref() else {
            return Err(invalid(format!("{id} is not an artifact: the artifact tool creates one from a plan")));
        };
        let steps = crate::artifact::steps(&plan.steps, spec.steps.as_deref().unwrap_or_default()).map_err(invalid)?;
        if let Some(plan) = self.item(id).artifact.as_mut() {
            plan.steps = steps;
        }
        Ok(id)
    }

    /// Applies the cue `proposal` to its gotcha, when the user's answer to
    /// question `id` is the one that applies it: `applies`, the first answer
    /// the asking session offered, or on a question without it ekko's own
    /// "Turn on" or "Turn off", a note added to it or not. The cue proposed
    /// is set, or a proposal of none drops the gotcha's, and any other answer
    /// changes nothing.
    fn settle_proposal(&mut self, id: u32, proposal: &Proposal, applies: Option<&str>, answer: &str, now: i64) {
        let own = if proposal.cue.is_some() { crate::guard::TURN_ON } else { crate::guard::TURN_OFF };
        if crate::menu::picked(answer) != applies.unwrap_or(own) {
            return;
        }
        let cue = proposal.cue.clone();
        let gotcha = self.data.values().find(|item| item.uid.as_deref() == Some(proposal.gotcha.as_str())).map(|item| item.id);
        let Some(gotcha) = gotcha.filter(|gotcha| self.data[gotcha].knowledge == Some(Knowledge::Gotcha)) else {
            self.notices.push(format!("the gotcha question {id} is about is no longer on the board as a gotcha: its cue was left as it was"));
            return;
        };
        let question = self.data[&id].uid.clone().unwrap_or_default();
        self.item(gotcha).cue = cue.map(|cue| CueOn { cue, question, at: now });
    }

    /// Links the two projects `linking` names when the user's answer is the
    /// one that applies it (task 811): `applies`, or on a question without it
    /// ekko's own "Link", a note added to it or not. Any other answer changes
    /// nothing. The link is
    /// written to the registry, outside the board, as the answer is
    /// recorded: a write that then fails leaves the link made and the
    /// question open.
    fn settle_link(&mut self, linking: &Linking, applies: Option<&str>, answer: &str) {
        if crate::menu::picked(answer) != applies.unwrap_or(crate::project::LINK) {
            return;
        }
        let Some((home, _)) = &self.ekko.linkable else {
            self.notices.push("this board is not a project's, so nothing was linked".to_string());
            return;
        };
        let [a, b] = &linking.projects;
        match crate::project::join(home, a, b) {
            Ok(_) => self.notices.push("the two projects are linked: a session on either, this one included, reaches the other's board with project".to_string()),
            Err(error) => self.notices.push(format!("nothing was linked: {error}")),
        }
    }

    /// Asks `inquiry`, as `ask` does. A question proposing what the user's
    /// answer applies -- a cue (task 805), a refused call let through once,
    /// two projects linked (task 811) -- gets the block ekko quotes, what
    /// would be applied, after the session's explanation. The session writes
    /// the two answers, in the user's language, and the first applies it
    /// (task 1044). The question as put to the user comes back with its id.
    pub fn ask_inquiry(&mut self, inquiry: &Inquiry, about: Option<&Ref>, home: &Path) -> Result<(u32, Inquiry), EkkoError> {
        if inquiry.proposals() > 1 {
            return Err(invalid(ONE_PROPOSAL));
        }
        if inquiry.proposes() {
            if let Some(why) = crate::dialog::check(inquiry) {
                return Err(invalid(why));
            }
        }
        if let Some(name) = &inquiry.link_project {
            let (block, linking) = self.linking(name)?;
            return self.ask_proposing(inquiry, &block, about, |question| question.link = Some(linking));
        }
        if let Some(artifact) = &inquiry.approve {
            let (block, approving, target) = self.approving(artifact)?;
            // About the artifact unless the session says what else.
            let about = about.cloned().unwrap_or(Ref::Id(target));
            return self.ask_proposing(inquiry, &block, Some(&about), |question| question.approve = Some(approving));
        }
        let (block, proposal, allowance) = match (&inquiry.cue, &inquiry.allow) {
            (None, None) => {
                let noted = crate::dialog::noted(&inquiry.text, inquiry.explain.as_deref(), &inquiry.options, inquiry.multiple);
                let id = self.ask(&noted, about)?;
                return Ok((id, inquiry.clone()));
            }
            (Some(_), Some(_)) => return Err(invalid(ONE_PROPOSAL)),
            (Some(spec), None) => {
                let target = self.resolve(&spec.gotcha)?;
                let gotcha = &self.data[&target];
                if gotcha.is_task || gotcha.knowledge != Some(Knowledge::Gotcha) {
                    return Err(invalid(format!("{target} is not a gotcha: a cue belongs to a gotcha, whose text is the refusal's reason")));
                }
                let cue = spec.cue()?;
                if cue.is_some() && cue.as_ref() == gotcha.cue.as_ref().map(|on| &on.cue) {
                    return Err(invalid(format!("gotcha {target}'s cue is that one already")));
                }
                let Some(uid) = gotcha.uid.clone() else {
                    return Err(invalid(format!("{target} has no uid, which a proposal names it by")));
                };
                let proposal = Proposal { gotcha: uid, cue, unknown: Default::default() };
                if proposal.cue.is_none() && gotcha.cue.is_none() {
                    return Err(invalid(format!("gotcha {target} has no cue to turn off")));
                }
                if proposal.cue.is_some() && !crate::guard::reads(home, self.ekko.storage.storage_path()) {
                    return Err(invalid(format!(
                        "ekko's guard does not read this board, {}: it reads the default board and the registered projects', \
                         never one opened through EKKO_DIR or --ekko-dir, so a cue on gotcha {target} would never refuse",
                        self.ekko.storage.storage_path().display()
                    )));
                }
                let block = crate::guard::proposal_block(gotcha, &proposal, self.ekko.folder.as_deref());
                (block, Some(proposal), None)
            }
            (None, Some(code)) => {
                let session = self.ekko.actor.as_ref().and_then(|actor| actor.process.as_ref());
                let now = chrono::Local::now().timestamp_millis();
                let refused = crate::guard::refusal(home, code.trim(), session, now).map_err(invalid)?;
                let (block, allowance) = crate::guard::allow_block(&refused);
                (block, None, Some(allowance))
            }
        };
        self.ask_proposing(inquiry, &block, about, |question| {
            question.cue = proposal;
            question.allow = allowance;
        })
    }

    /// Records `inquiry` with `block` after its explanation and the
    /// session's two options as its answers, the first of which applies
    /// what the question proposes; then sets on the question what `propose`
    /// does. The question as put to the user comes back, with its id.
    fn ask_proposing(
        &mut self,
        inquiry: &Inquiry,
        block: &str,
        about: Option<&Ref>,
        propose: impl FnOnce(&mut Question),
    ) -> Result<(u32, Inquiry), EkkoError> {
        let explain = format!("{}\n\n{}", inquiry.explain.as_deref().unwrap_or_default().trim(), block.trim());
        let id = self.ask(&crate::dialog::noted(&inquiry.text, Some(&explain), &inquiry.options, false), about)?;
        let applies = inquiry.options.first().map(|option| option.label.trim().to_string());
        if let Some(question) = self.item(id).question.as_mut() {
            question.applies = applies;
            propose(question);
        }
        Ok((id, Inquiry { explain: Some(explain), quick: false, cue: None, allow: None, link_project: None, approve: None, ..inquiry.clone() }))
    }

    /// What a question asking the user to approve artifact `artifact`'s plan
    /// records and quotes (task 1019): the steps not approved yet, which the
    /// approval makes tasks of in the plan's order, and the version of the
    /// plan they belong to; and the artifact, by id.
    fn approving(&self, artifact: &Ref) -> Result<(String, Approving, u32), EkkoError> {
        let target = self.resolve(artifact)?;
        let item = &self.data[&target];
        let Some(plan) = item.artifact.as_deref() else {
            return Err(invalid(format!("{target} is not an artifact: approve takes one, which the artifact tool writes")));
        };
        let state = State::of(item);
        if !state.is_some_and(State::is_open) {
            return Err(invalid(format!("artifact {target} is {}: there is nothing to approve", state.map_or("closed", State::word))));
        }
        if let Some(asked) = crate::artifact::approval_asked(item, &self.data) {
            return Err(invalid(format!(
                "question {} asks to approve artifact {target} already: the user answers it in ekko's menu, or with Review on the artifact's page",
                asked.id
            )));
        }
        let proposed: Vec<&crate::item::Step> = plan.steps.iter().filter(|step| step.task.is_none()).collect();
        if proposed.is_empty() {
            return Err(invalid(format!("artifact {target} has no step left to approve: the artifact tool adds steps")));
        }
        let Some(uid) = item.uid.clone() else {
            return Err(invalid(format!("{target} has no uid, which a question names it by")));
        };
        let mut block = format!(
            "The approval proposed: artifact {target}, version {} of its plan, {}.\nIt makes these tasks, in the plan's order, each blocking the artifact:\n",
            plan.version,
            crate::ekko::title(&item.description).trim()
        );
        for step in &proposed {
            let after = if step.after.is_empty() { String::new() } else { format!(" (after {})", step.after.join(", ")) };
            block.push_str(&format!("- {}: {}{after}\n", step.key, crate::ekko::title(&step.text).trim()));
        }
        block.push_str(&format!("The plan in full is on its page, which ekko artifact {target} opens."));
        let steps = proposed.iter().map(|step| step.key.clone()).collect();
        Ok((block, Approving { artifact: uid, version: plan.version, steps, unknown: Default::default() }, target))
    }

    /// What a question proposing to link this board's project and `name`'s
    /// records and quotes (task 811): the two projects, by id, and what the
    /// link would do, which the user reads before answering.
    fn linking(&self, name: &str) -> Result<(String, Linking), EkkoError> {
        let Some((home, here)) = &self.ekko.linkable else {
            return Err(invalid("only a project's board links to another: this is the default board, or one opened through EKKO_DIR"));
        };
        let there = crate::project::resolve_named(home, name.trim())?;
        let projects = crate::project::linkable(here, &there)?;
        if self.ekko.linked().iter().any(|linked| linked.name == there.name) {
            return Err(invalid(format!("{} and {} are linked already: every tool but wait takes project: {} here", here.name, there.name, there.name)));
        }
        let named = |project: &crate::project::Project| match &project.root {
            Some(root) => format!("{} ({})", project.name, root.display()),
            None => project.name.clone(),
        };
        let block = format!(
            "\n\nThe link proposed: the boards of projects {} and {}, both ways. A Claude Code session on either would read and write \
             the other's through ekko's MCP, without a prompt: every tool but wait takes project: {} on {}'s board, and \
             project: {} on {}'s. ekko --unlink-project, in either folder, takes it away.",
            named(here),
            named(&there),
            there.name,
            here.name,
            here.name,
            there.name
        );
        Ok((block, Linking { projects, unknown: Default::default() }))
    }

    /// Records on question `uid` that the tool call `tool_use_id` went
    /// through on the user's answer to it: true unless another call did
    /// first. The same call again stays true, since ekko's guard and
    /// another's each ask about it.
    pub fn spend_allowance(&mut self, uid: &str, tool_use_id: &str) -> Result<bool, EkkoError> {
        let id = self.resolve(&Ref::Text(uid.to_string()))?;
        let Some(allow) = self.item(id).question.as_mut().and_then(|question| question.allow.as_mut()) else {
            return Ok(false);
        };
        match &allow.used {
            Some(used) => Ok(used.tool_use_id == tool_use_id),
            None => {
                let at = chrono::Local::now().timestamp_millis();
                allow.used = Some(crate::item::Used { tool_use_id: tool_use_id.to_string(), at, unknown: Default::default() });
                Ok(true)
            }
        }
    }

    /// The session this draft writes for, which alone can wait: the user at
    /// the terminal is told nothing.
    fn waiter(&self) -> Result<&crate::holder::Actor, EkkoError> {
        self.ekko
            .actor
            .as_ref()
            .filter(|actor| !actor.is_person())
            .ok_or_else(|| invalid("A wait is a Claude Code session's, told when it is over: the user at the terminal has nothing to wait with"))
    }

    /// Records that this session waits on `spec.item` until it is what
    /// `spec.until` names: a note whose text is what the session will do
    /// then, attached to the task waited on, or to the task a question is
    /// about. An item already there, or already waited on by this session
    /// for the same, writes nothing.
    pub fn wait(&mut self, spec: &WaitOn) -> Result<Waited, EkkoError> {
        let target = self.resolve(&spec.item)?;
        let item = &self.data[&target];
        if item.trashed.is_some() || item.stashed.is_some() {
            let away = if item.trashed.is_some() { "the trash" } else { "the stash" };
            return Err(invalid(format!("{target} is in {away}: nothing moves there to wait on")));
        }
        let question = item.question.is_some();
        if !item.is_task && !question {
            return Err(invalid(format!("{target} is a note: a wait is on a task or a question")));
        }
        let until = match spec.until.as_deref() {
            None if question => Until::Answered,
            None => Until::Done,
            Some(word) => Until::parse(word)
                .ok_or_else(|| invalid(format!("until is done, free or answered, not {word}")))?,
        };
        match (until, question) {
            (Until::Answered, false) => return Err(invalid(format!("{target} is not a question: until answered waits on one"))),
            (Until::Done | Until::Free, true) => {
                return Err(invalid(format!("{target} is a question: it is waited on until answered")))
            }
            _ => {}
        }
        let text = spec.text.as_deref().map(str::trim).unwrap_or_default();
        if text.is_empty() {
            return Err(invalid("wait needs text: what this session will do once the wait is over"));
        }
        crate::ekko::fits("wait", text)?;
        let Some(on) = item.uid.clone() else {
            return Err(invalid(format!("{target} has no uid to wait on; any write to it gives it one")));
        };
        let about = if question { item.attached_to.clone().map(Ref::Text) } else { Some(Ref::Id(target)) };

        let waiter = self.waiter()?;
        let now = chrono::Local::now().timestamp_millis();
        let wait = Wait { on, until, by: waiter.holder(now), rev: 0, over: None, unknown: Default::default() };
        if let Some(how) = wait.over_on(Some(item)) {
            return Ok(Waited::Met(target, how));
        }
        let mut already: Vec<&Item> = self
            .data
            .values()
            .filter(|note| note.trashed.is_none() && note.stashed.is_none())
            .filter(|note| {
                note.wait.as_ref().is_some_and(|open| {
                    open.over.is_none() && open.on == wait.on && open.until == until && waiter.is(&open.by)
                })
            })
            .collect();
        already.sort_by_key(|note| note.id);
        if let Some(note) = already.first() {
            return Ok(Waited::Already(note.id));
        }

        let spec = Create {
            kind: Some(Kind::Note),
            text: text.to_string(),
            boards: Vec::new(),
            priority: None,
            due: None,
            with: None,
            phase: None,
            blocked_by: Vec::new(),
            attached_to: about,
            supersedes: None,
            starred: false,
        };
        let id = self.create(&spec)?;
        self.item(id).wait = Some(Box::new(wait));
        Ok(Waited::Recorded(id))
    }

    /// Ends this session's open waits on `item`, as dropped: the notes it
    /// ended, none where it waited on nothing there.
    pub fn unwait(&mut self, item: &Ref) -> Result<Vec<u32>, EkkoError> {
        let target = self.resolve(item)?;
        let Some(on) = self.data[&target].uid.clone() else { return Ok(Vec::new()) };
        let waiter = self.waiter()?.clone();
        let now = chrono::Local::now().timestamp_millis();
        let mut mine: Vec<u32> = self
            .data
            .values()
            .filter(|note| {
                note.wait.as_ref().is_some_and(|wait| wait.over.is_none() && wait.on == on && waiter.is(&wait.by))
            })
            .map(|note| note.id)
            .collect();
        mine.sort_unstable();
        for id in &mine {
            if let Some(wait) = self.item(*id).wait.as_mut() {
                wait.over = Some(crate::item::Over { how: How::Dropped, by: Some(waiter.holder(now)), at: now, rev: 0, unknown: Default::default() });
            }
        }
        Ok(mine)
    }

    /// Makes note `id` the handoff of `task`: an open task only, since a
    /// finished one has nothing left to hand over. The handoff it replaces
    /// stays on the task as an ordinary note, which the new one supersedes,
    /// so a read can tell it from the notes around it and show it as
    /// history. It replaces this session's, one by a session that has ended
    /// or by nobody known; another session's that still runs stays, its own
    /// to resume (task 514).
    fn hand_over(&mut self, id: u32, task: u32) -> Result<(), EkkoError> {
        if !holds(&self.data[&task]) {
            return Err(invalid(format!("{task} is finished, and a finished task has nothing to hand over")));
        }
        let uid = self.data[&task].uid.clone();
        let actor = self.ekko.actor.as_ref();
        let earlier: Vec<u32> = self
            .data
            .values()
            .filter(|note| note.handoff && note.id != id && note.attached_to.is_some() && note.attached_to == uid)
            .filter(|note| note.created_by.as_ref().and_then(|author| author.whose(actor)) != Some(Whose::Running))
            .map(|note| note.id)
            .collect();
        let mut replaced = None;
        for note in earlier {
            let demoted = self.item(note);
            demoted.handoff = false;
            if demoted.trashed.is_none() {
                replaced = demoted.uid.clone();
            }
        }
        let handoff = self.item(id);
        handoff.handoff = true;
        handoff.supersedes = replaced;
        Ok(())
    }

    fn declared_phase(&self, phase: Option<&str>) -> Result<Option<String>, EkkoError> {
        match phase {
            None => Ok(None),
            Some(name) if self.phases.iter().any(|declared| declared == name) => Ok(Some(name.to_string())),
            Some(name) => Err(invalid(if self.phases.is_empty() {
                format!("Unknown phase {name}: this board declares no phases")
            } else {
                format!("Unknown phase {name}: the declared phases are {}", self.phases.join(", "))
            })),
        }
    }

    pub fn set_state(&mut self, items: &[Ref], state: &str) -> Result<Vec<u32>, EkkoError> {
        let setting = Setting::from_word(state).ok_or_else(|| EkkoError::UnknownState(state.to_string()))?;
        if items.is_empty() {
            return Err(EkkoError::MissingId);
        }
        let ids = self.resolve_all(items)?;
        // The terminal skips task states on notes quietly, as taskbook did; a
        // structured write that changed nothing would still answer ok.
        if setting.is_state() {
            if let Some(note) = ids.iter().find(|id| !self.data[*id].is_task) {
                return Err(invalid(format!(
                    "{note} is a note, and a note has no state; of the states only starred and unstarred apply to it"
                )));
            }
        }
        for id in &ids {
            // undone reopens only done work, so on a cancelled task it is a
            // quiet no-op; say so, and name the state that does revive it.
            if setting == Setting::Undone && State::of(&self.data[id]) == Some(State::Cancelled) {
                self.notices.push(format!("{id} is cancelled, and undone reopens only done work: nothing changed; unstarted revives it"));
            }
            setting.apply(self.item(*id));
        }
        if setting == Setting::Become(State::Progress) {
            self.claims.extend(&ids);
        }
        Ok(ids)
    }

    pub fn edit(&mut self, spec: &Edit) -> Result<Vec<u32>, EkkoError> {
        let id = self.resolve(&spec.item)?;
        let item = self.item(id);

        // Against the version the caller read, which is the version before
        // this batch: the stamp only moves when the batch is written.
        if let Some(expected) = spec.if_updated_at {
            let current = item.updated_at.unwrap_or(item.timestamp);
            if current != expected {
                return Err(EkkoError::Stale { id, current });
            }
        }

        let description = match (&spec.text, &spec.replace, &spec.append) {
            (Some(text), None, None) => text.clone(),
            (None, Some(replace), None) => {
                if replace.old.is_empty() {
                    return Err(invalid("replace.old is empty, so there is nothing to find"));
                }
                let found = item.description.matches(replace.old.as_str()).count();
                if found != 1 {
                    return Err(EkkoError::EditMatch { id, found });
                }
                item.description.replacen(&replace.old, &replace.new, 1)
            }
            (None, None, Some(more)) => {
                if more.trim().is_empty() {
                    return Err(invalid("append is empty, so there is nothing to add"));
                }
                if item.is_task && !item.description.trim_end().contains('\n') {
                    // What is added to a task goes to its body, never its title.
                    format!("{}\n{}", item.description.trim_end(), more.trim_start())
                } else {
                    let joins = item.description.ends_with(char::is_whitespace) || more.starts_with(char::is_whitespace);
                    if joins { format!("{}{more}", item.description) } else { format!("{} {more}", item.description) }
                }
            }
            _ => return Err(invalid("An edit takes exactly one of text, replace or append")),
        };
        if description.trim().is_empty() {
            return Err(EkkoError::MissingDesc);
        }
        crate::ekko::fits("description", &description)?;
        if item.is_task {
            crate::ekko::retitled(&item.description, &description)?;
        }
        item.description = description;
        Ok(vec![id])
    }

    pub fn update(&mut self, spec: &Update) -> Result<Vec<u32>, EkkoError> {
        let id = self.resolve(&spec.item)?;
        let changes_boards = !spec.add_boards.is_empty() || !spec.remove_boards.is_empty();
        if spec.boards.is_none()
            && !changes_boards
            && spec.priority.is_none()
            && spec.due.is_none()
            && spec.with.is_none()
            && spec.phase.is_none()
            && spec.starred.is_none()
            && spec.kind.is_none()
        {
            return Err(invalid(
                "An update needs at least one of boards, add_boards, remove_boards, priority, due, with, phase, starred or kind",
            ));
        }
        self.fresh(id, spec.if_updated_at)?;
        let is_task = self.data[&id].is_task;

        if let Some(names) = &spec.boards {
            if changes_boards {
                return Err(invalid("boards replaces the item's boards; add_boards and remove_boards change them: give one or the other"));
            }
            if names.iter().all(|name| name.trim().is_empty()) {
                return Err(EkkoError::MissingBoards);
            }
            addressable(names)?;
            self.item(id).boards = boards(names);
        }
        addressable(&spec.add_boards)?;
        if changes_boards {
            let removed = named(&spec.remove_boards);
            let mut kept: Vec<String> = self.data[&id].boards.iter().filter(|name| !removed.contains(name)).cloned().collect();
            for name in named(&spec.add_boards) {
                if !kept.contains(&name) {
                    kept.push(name);
                }
            }
            // Taking the last board off leaves the item on the default one,
            // the board an item with none is on.
            self.item(id).boards = boards(&kept);
        }
        if let Some(priority) = spec.priority {
            if !is_task {
                return Err(invalid("A note has no priority; only a task does"));
            }
            self.item(id).priority = Some(priority_of(priority)?);
        }
        if let Some(due) = &spec.due {
            let parsed = match due {
                Some(_) if !is_task => return Err(invalid("A note has no due date; only a task does")),
                Some(date) => Some(parse_due_date(&format!("d:{date}")).ok_or_else(|| EkkoError::InvalidDueDate(date.clone()))?),
                None => None,
            };
            self.item(id).due_date = parsed;
        }
        if let Some(with) = &spec.with {
            let with = match with {
                Some(_) if !is_task => return Err(invalid(NOTE_WITH)),
                Some(name) => Some(person(name)?),
                None => None,
            };
            self.item(id).with = with;
        }
        if let Some(phase) = &spec.phase {
            let phase = self.declared_phase(phase.as_deref())?;
            self.item(id).phase = phase;
            self.refuse_phase_inversions(id)?;
        }
        if let Some(starred) = spec.starred {
            self.item(id).is_starred = starred;
        }
        if let Some(kind) = spec.kind {
            self.retype(id, kind)?;
        }
        Ok(vec![id])
    }

    /// Gives note `id` the kind `kind` names -- decision, gotcha, procedure,
    /// or `note` for none -- which is how a note written before typed notes
    /// existed becomes one. A task has no kind, a handoff expires and so is
    /// never a decision, nor is a wait, which ends on its own, and a note in
    /// a line of supersession keeps the kind
    /// of that line: a decision replaced by a gotcha would be neither.
    fn retype(&mut self, id: u32, kind: Kind) -> Result<(), EkkoError> {
        let named = self.names().name(id);
        let item = &self.data[&id];
        if item.is_task {
            return Err(invalid(format!("{named} is a task; only a note takes a kind")));
        }
        let knowledge = match kind {
            Kind::Task | Kind::Handoff => {
                return Err(invalid("update gives a note the kind note, decision, gotcha or procedure"));
            }
            other => other.knowledge(),
        };
        if knowledge == item.knowledge {
            return Ok(());
        }
        if item.handoff {
            return Err(invalid(format!(
                "{named} is a handoff, which the next one replaces; write what stays true as a note of its own"
            )));
        }
        if item.wait.is_some() {
            return Err(invalid(format!(
                "{named} is a session's wait, which ends on its own; write what stays true as a note of its own"
            )));
        }
        let replaced = item.uid.as_deref().is_some_and(|uid| {
            self.data.values().any(|note| note.trashed.is_none() && note.supersedes.as_deref() == Some(uid))
        });
        // A note without a kind is in a line of supersession only as a handoff
        // a later one replaced (see `hand_over`).
        if item.knowledge.is_none() && (item.supersedes.is_some() || replaced) {
            return Err(invalid(format!(
                "{named} is a handoff a later one replaced; write what stays true as a note of its own"
            )));
        }
        if item.supersedes.is_some() || replaced {
            return Err(invalid(format!(
                "{named} supersedes a note or is superseded by one, and a line of them keeps one kind: clear supersedes with link first"
            )));
        }
        self.item(id).knowledge = knowledge;
        Ok(())
    }

    /// Moving an item between phases can turn its dependencies, in either
    /// direction, against the phase order -- refused the way `--blocked-by`
    /// refuses making one.
    fn refuse_phase_inversions(&self, id: u32) -> Result<(), EkkoError> {
        let order = phase_order(&self.phases);
        let index = uid_index(&self.data);
        let item = &self.data[&id];
        for uid in item.blocked_by.iter().flatten() {
            if let Some(blocker) = index.get(uid.as_str()).and_then(|b| self.data.get(b)) {
                if let Some(inversion) = phase_inversion(&order, item, blocker) {
                    return Err(EkkoError::PhaseOrder(inversion));
                }
            }
        }
        let Some(uid) = item.uid.as_deref() else { return Ok(()) };
        for dependent in self.data.values() {
            if dependent.blocked_by.iter().flatten().any(|b| b == uid) {
                if let Some(inversion) = phase_inversion(&order, dependent, item) {
                    return Err(EkkoError::PhaseOrder(inversion));
                }
            }
        }
        Ok(())
    }

    pub fn link(&mut self, spec: &Link) -> Result<Vec<u32>, EkkoError> {
        let id = self.resolve(&spec.item)?;
        self.fresh(id, spec.if_updated_at)?;
        let changes_blockers = !spec.add_blocked_by.is_empty() || !spec.remove_blocked_by.is_empty();
        match (&spec.blocked_by, &spec.attached_to, &spec.supersedes) {
            (None, None, None) if changes_blockers => {
                let added = self.resolve_all(&spec.add_blocked_by)?;
                let removed = self.resolve_all(&spec.remove_blocked_by)?;
                let mut blockers: Vec<u32> = self.blocker_ids(id).into_iter().filter(|b| !removed.contains(b)).collect();
                for blocker in added {
                    if !blockers.contains(&blocker) {
                        blockers.push(blocker);
                    }
                }
                let uids = self.ekko.blocker_uids(&self.data, &self.phases, id, &blockers)?;
                self.item(id).blocked_by = if uids.is_empty() { None } else { Some(uids) };
            }
            _ if changes_blockers => {
                return Err(invalid(
                    "add_blocked_by and remove_blocked_by change what the item is blocked by, on their own: not beside blocked_by, attached_to or supersedes",
                ))
            }
            (Some(blockers), None, None) => {
                let blockers = self.resolve_all(blockers)?;
                let uids = self.ekko.blocker_uids(&self.data, &self.phases, id, &blockers)?;
                self.item(id).blocked_by = if uids.is_empty() { None } else { Some(uids) };
            }
            (None, Some(target), None) => {
                let target = target.as_ref().map(|t| self.resolve(t)).transpose()?;
                let attached = Ekko::attach_target(&self.data, id, target)?;
                let moved = self.data[&id].attached_to != attached.as_ref().map(|(_, uid)| uid.clone());
                self.item(id).attached_to = attached.map(|(_, uid)| uid);
                // A handoff hands over the task it was written on; moved or
                // detached, it is an ordinary note about that work, and the
                // handoff it replaced there is no longer replaced by it. (A
                // note without a kind supersedes only as a handoff.)
                if moved {
                    let note = self.item(id);
                    note.handoff = false;
                    if note.knowledge.is_none() {
                        note.supersedes = None;
                    }
                }
            }
            (None, None, Some(older)) => {
                let older = older.as_ref().map(|o| self.resolve(o)).transpose()?;
                let names = self.names();
                supersede(&mut self.data, id, older, &|id| names.name(id))?;
            }
            _ => {
                return Err(invalid(
                    "A link takes exactly one of blocked_by, add_blocked_by and remove_blocked_by, attached_to or supersedes",
                ))
            }
        }
        Ok(vec![id])
    }

    /// Refuses a write against a version of `id` that is no longer current,
    /// when the caller says which one it read: its updatedAt before this
    /// batch, since the stamp only moves when the batch is written.
    fn fresh(&self, id: u32, if_updated_at: Option<i64>) -> Result<(), EkkoError> {
        let item = &self.data[&id];
        match if_updated_at {
            Some(expected) if item.updated_at.unwrap_or(item.timestamp) != expected => {
                Err(EkkoError::Stale { id, current: item.updated_at.unwrap_or(item.timestamp) })
            }
            _ => Ok(()),
        }
    }

    /// What `id` is blocked by now, by display id, in the order it has them.
    fn blocker_ids(&self, id: u32) -> Vec<u32> {
        let uids = self.data[&id].blocked_by.clone().unwrap_or_default();
        uids.iter().filter_map(|uid| self.data.values().find(|item| item.uid.as_deref() == Some(uid)).map(|item| item.id)).collect()
    }

    /// Writes the draft, once, if the board it leaves keeps the dependency
    /// rule -- or, with `force`, anyway, saying what it pushed past.
    pub fn commit(mut self, force: bool) -> Result<Committed, EkkoError> {
        let (overridden, reopened) = Ekko::refuse_broken_dependencies(&self.before, &self.data, force)?;
        let mut notices = std::mem::take(&mut self.notices);
        notices.extend(self.kept_references());
        notices.extend(self.settle_holders(force)?);
        notices.extend(self.loose_notes_of_the_done());
        notices.extend(self.trailer_of_the_started());
        let saved = self.ekko.save_against(&self.before, &mut self.data)?;
        notices.extend(self.ended_waits(&saved.ended));
        Ok(Committed { data: self.data, overridden, reopened, released: saved.released, blocked: saved.blocked, notices })
    }

    /// Each wait this write ended, told to the session that wrote it: whose
    /// wait it was and on what, and that they are told in turn (task 389).
    fn ended_waits(&self, ended: &[u32]) -> Vec<String> {
        let actor = self.ekko.actor.as_ref();
        ended
            .iter()
            .filter_map(|id| {
                let wait = self.data.get(id)?.wait.as_ref()?;
                let how = wait.over.as_ref()?.how;
                let target = self.data.values().find(|item| item.uid.as_deref() == Some(wait.on.as_str()));
                let named = target.map_or_else(|| "what it waited on".to_string(), |item| item.id.to_string());
                if actor.is_some_and(|actor| actor.is(&wait.by)) {
                    let what = crate::agent::ended_phrase(how, true);
                    return Some(format!("{named} {what}: this session's wait on it (note {id}) is over"));
                }
                let what = crate::agent::ended_phrase(how, false);
                let waiter = actor.map_or_else(|| wait.by.label(), |actor| actor.name(&wait.by));
                Some(format!("{named} {what}: {waiter} waited on it (note {id}) and is told"))
            })
            .collect()
    }

    /// The trailer by which a commit names the tasks it carries (task 396),
    /// told once, to the session taking the tasks up: `context` lists a
    /// task's commits by it, whatever a rebase later does to their SHAs.
    /// Only on a project whose folder is in a git repository.
    fn trailer_of_the_started(&self) -> Vec<String> {
        let started = |item: &Item| State::of(item) == Some(State::Progress);
        let mut ids: Vec<u32> = self
            .data
            .values()
            .filter(|task| task.is_task && started(task) && !self.before.get(&task.id).is_some_and(started))
            .map(|task| task.id)
            .collect();
        let session = self.ekko.actor.as_ref().is_some_and(|actor| !actor.is_person());
        if ids.is_empty() || !session || !self.ekko.folder.as_deref().is_some_and(crate::commits::in_repository) {
            return Vec::new();
        }
        ids.sort_unstable();
        let trailer = |ids: &[u32]| ids.iter().map(u32::to_string).collect::<Vec<_>>().join(", ");
        vec![match ids.as_slice() {
            [id] => format!("Commits for {id}: end the message with the trailer 'Ekko: {id}', by which context lists them"),
            [rest @ .., last] => format!(
                "Commits for {} and {last}: end each message with a trailer naming the tasks it carries, as \
                 'Ekko: {}' for all of them, by which context lists them",
                trailer(rest),
                trailer(&ids)
            ),
            [] => unreachable!("returned above when no task was started"),
        }]
    }

    /// Each `$N` a batch's text holds for another operation N that created an
    /// item, told in a notice naming that item. A text is left as written,
    /// since prose may mean a dollar sign, and so it once kept 'task $2'
    /// where the batch's second operation had created the task meant (task
    /// 394). Only the fields that take an item read `$N` as one. An item's
    /// own number is not told: its text has no need of it, and a batch of
    /// one create would be told that 'costs $1' names what it wrote.
    fn kept_references(&self) -> Vec<String> {
        let mut notices = Vec::new();
        for (at, texts) in self.texts.iter().enumerate() {
            let mut told = Vec::new();
            for n in texts.iter().flat_map(|text| dollar_numbers(text)).filter(|&n| n != at + 1) {
                let Some(id) = n.checked_sub(1).and_then(|op| self.created.get(op).copied().flatten()) else { continue };
                if !told.contains(&n) {
                    told.push(n);
                    notices.push(format!(
                        "operation {}'s text holds ${n}, which batch does not replace: that item is {id}",
                        at + 1
                    ));
                }
            }
        }
        notices
    }

    /// The notes still attached to each task this draft completes, told in a
    /// notice: they leave the prime with the task, and one that proposes
    /// something, or holds a lesson, would leave unsettled with nobody told
    /// (task 398). Only notes without a kind: a decision, a gotcha or a
    /// procedure stays in force on its own, a handoff is history once its
    /// task is done, a question waits on the user wherever it is, and a wait
    /// ends on its own.
    /// Nothing is written on its own.
    fn loose_notes_of_the_done(&self) -> Vec<String> {
        let done = |item: &Item| State::of(item) == Some(State::Done);
        let replaced: std::collections::HashSet<&str> =
            self.data.values().filter(|note| note.trashed.is_none()).filter_map(|note| note.supersedes.as_deref()).collect();
        let mut finished: Vec<&Item> = self
            .data
            .values()
            .filter(|task| task.is_task && done(task) && !self.before.get(&task.id).is_some_and(done))
            .collect();
        finished.sort_by_key(|task| task.id);
        let mut notices = Vec::new();
        for task in finished {
            let Some(uid) = task.uid.as_deref() else { continue };
            let mut loose: Vec<u32> = self
                .data
                .values()
                .filter(|note| !note.is_task && note.attached_to.as_deref() == Some(uid))
                .filter(|note| note.trashed.is_none() && note.stashed.is_none())
                .filter(|note| note.knowledge.is_none() && !note.handoff && note.question.is_none() && note.wait.is_none())
                .filter(|note| !note.uid.as_deref().is_some_and(|uid| replaced.contains(uid)))
                .map(|note| note.id)
                .collect();
            if loose.is_empty() {
                continue;
            }
            loose.sort_unstable();
            let id = task.id;
            notices.push(match loose.as_slice() {
                [only] => format!(
                    "{id} is done; note {only} stays attached to it and leaves the prime with it. If it still \
                     proposes something, make that a task of its own or append why not; a lesson it holds goes \
                     into a decision, gotcha or procedure"
                ),
                [rest @ .., last] => format!(
                    "{id} is done; notes {} and {last} stay attached to it and leave the prime with it. One that \
                     still proposes something becomes a task of its own or gets a line saying why not; a lesson \
                     one holds goes into a decision, gotcha or procedure",
                    rest.iter().map(u32::to_string).collect::<Vec<_>>().join(", ")
                ),
                [] => unreachable!("an empty list was skipped"),
            });
        }
        notices
    }

    /// Who holds what, once the draft is whole. A change of state to a task
    /// another Claude Code session holds in progress, while that session
    /// runs, is refused as HELD unless `force` -- a second session asks the
    /// user first; a person at the terminal is never refused. The terminal's
    /// writes are judged the same way (`Ekko::held_elsewhere`). A task set in
    /// progress again is taken over from a holder that is gone, or forced
    /// past, and each such claim is told in a notice. Tasks entering progress
    /// are claimed when the draft is saved (`Ekko::save_against`).
    fn settle_holders(&mut self, force: bool) -> Result<Vec<String>, EkkoError> {
        let held = if force { Vec::new() } else { self.ekko.held_elsewhere(&self.before, &self.data, &self.claims) };
        if !held.is_empty() {
            return Err(EkkoError::Held(held));
        }

        let actor = self.ekko.actor.clone();
        let now = chrono::Local::now().timestamp_millis();
        let mut claims = std::mem::take(&mut self.claims);
        claims.sort_unstable();
        claims.dedup();
        let mut notices = Vec::new();
        for id in claims {
            // Only a task that already was in progress: one entering it now
            // is claimed as it is saved.
            let Some(old) = self.before.get(&id).filter(|old| State::of(old) == Some(State::Progress)) else { continue };
            let Some(actor) = &actor else { continue };
            let notice = match &old.held_by {
                Some(holder) if actor.is(holder) => {
                    format!("{id} was already in progress, yours since {}", crate::holder::when(holder.since))
                }
                Some(holder) if holder.alive() && actor.continues(holder) => {
                    format!("{id} was in progress under this conversation until Claude Code moved it out of {}: yours again", actor.name(holder))
                }
                Some(holder) if holder.alive() => format!("{id} was in progress under {}, still running: taken over", actor.name(holder)),
                Some(holder) => format!("{id} was in progress under {}, which is gone: now yours", actor.name(holder)),
                None => format!("{id} was already in progress, held by no one: now yours"),
            };
            if !old.held_by.as_ref().is_some_and(|holder| actor.is(holder)) {
                self.item(id).held_by = Some(actor.holder(now));
            }
            notices.push(notice);
        }
        Ok(notices)
    }
}

/// Makes note `id` supersede note `older`, or, with `None`, supersede
/// nothing. Both are notes of one kind -- a decision replaces a decision --
/// the older one is out of the trash, and a note has one successor at most,
/// so notes replacing one another form a single line whose newest is the one
/// in force. A line turned back on itself would have no newest, and is
/// refused. The CLI and the structured writes all set it through here, each
/// saying through `name` how a refusal names an item -- see `Names`.
pub(crate) fn supersede(
    data: &mut ItemMap,
    id: u32,
    older: Option<u32>,
    name: &dyn Fn(u32) -> String,
) -> Result<(), EkkoError> {
    let note = &data[&id];
    let Some(kind) = note.knowledge else {
        return Err(invalid(format!(
            "{} is {}, and only a decision, a gotcha or a procedure supersedes",
            name(id),
            described(note)
        )));
    };
    let Some(older) = older else {
        data.get_mut(&id).expect("ids are resolved against the board").supersedes = None;
        return Ok(());
    };
    if older == id {
        return Err(invalid(format!("{} cannot supersede itself", name(id))));
    }
    let replaced = &data[&older];
    if replaced.knowledge != Some(kind) {
        return Err(invalid(format!(
            "{} is {}, and a {kind} supersedes only a {kind}",
            name(older),
            described(replaced),
            kind = kind.word()
        )));
    }
    if replaced.trashed.is_some() {
        return Err(invalid(format!("{} is in the trash, so it is no longer in force to be replaced", name(older))));
    }
    let uid = replaced.uid.clone().ok_or_else(|| invalid(format!("{} has no uid for a note to point at", name(older))))?;
    let newer = data.values().find(|other| {
        other.id != id && other.trashed.is_none() && other.supersedes.as_deref() == Some(uid.as_str())
    });
    if let Some(newer) = newer {
        return Err(invalid(format!("{} is already superseded by {1}: supersede {1} instead", name(older), name(newer.id))));
    }
    let index = uid_index(data);
    let mut seen = std::collections::HashSet::new();
    let mut at = older;
    while let Some(next) = data[&at].supersedes.as_deref().and_then(|uid| index.get(uid)).copied() {
        if next == id {
            return Err(invalid(format!(
                "{} already supersedes {}, directly or through others: the newer note supersedes the older",
                name(older),
                name(id)
            )));
        }
        if !seen.insert(next) {
            break;
        }
        at = next;
    }
    data.get_mut(&id).expect("ids are resolved against the board").supersedes = Some(uid);
    Ok(())
}

/// What an item is, for a refusal: "a task", "an ordinary note", "a gotcha".
fn described(item: &Item) -> String {
    match (item.is_task, item.knowledge) {
        (true, _) => "a task".to_string(),
        (false, None) => "an ordinary note".to_string(),
        (false, Some(kind)) => format!("a {}", kind.word()),
    }
}

/// How a written item is reported back: enough to act on it again safely --
/// the uid to hold, and the `updatedAt` a later edit can be conditioned on.
pub fn written(data: &ItemMap, id: u32) -> Value {
    match data.get(&id) {
        Some(item) => json!({"id": id, "uid": item.uid, "updatedAt": item.updated_at}),
        None => json!({"id": id}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::holder::Actor;
    use crate::item::State;
    use crate::storage::Storage;
    use std::path::PathBuf;

    fn board(tag: &str) -> (Ekko, PathBuf) {
        let dir = crate::paths::test_dir(&format!("ekko-ops-{tag}"));
        std::fs::create_dir_all(&dir).unwrap();
        (Ekko::new(Storage::new(&dir).unwrap()), dir)
    }

    fn op(value: Value) -> Op {
        serde_json::from_value(value).expect("a valid operation")
    }

    /// Run `ops` as one batch and commit it, the way a caller would.
    fn batch(ekko: &Ekko, ops: &[Value]) -> Result<Committed, EkkoError> {
        let mut draft = Draft::open(ekko)?;
        for value in ops {
            draft.apply(&op(value.clone()))?;
        }
        draft.commit(false)
    }

    /// Nothing in the text is read as a board, a priority or a date: the words
    /// the CLI would have taken out are exactly the ones prose uses.
    #[test]
    fn the_text_is_kept_whole_and_every_field_comes_separately() {
        let (ekko, dir) = board("create");
        let text = "mention @someone, rate it p:3, and d:2026-01-01 is just a date";
        let written = batch(
            &ekko,
            &[json!({"op": "create", "text": text, "boards": ["coding", "@reviews"], "priority": 2, "due": "2026-9-1"})],
        )
        .unwrap();

        let item = &written.data[&1];
        assert_eq!(item.description, text);
        assert_eq!(item.boards, vec!["@coding", "@reviews"]);
        assert_eq!((item.priority, item.due_date.as_deref()), (Some(2), Some("2026-09-01")));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Who a task is with, given to create and changed by update, where null
    /// leaves it with nobody: one word, kept in lower case, on a task only.
    #[test]
    fn with_is_given_by_create_changed_by_update_and_cleared_by_null() {
        let (ekko, dir) = board("with");
        let made = batch(&ekko, &[json!({"op": "create", "text": "call the vendor", "with": " Rodrigo "})]).unwrap();
        assert_eq!(made.data[&1].with.as_deref(), Some("rodrigo"));
        batch(&ekko, &[json!({"op": "create", "kind": "note", "text": "a note"})]).unwrap();

        let moved = batch(&ekko, &[json!({"op": "update", "item": 1, "with": "Ana"})]).unwrap();
        assert_eq!(moved.data[&1].with.as_deref(), Some("ana"));
        let cleared = batch(&ekko, &[json!({"op": "update", "item": 1, "with": null})]).unwrap();
        assert_eq!(cleared.data[&1].with, None);

        for refused in [
            json!({"op": "update", "item": 1, "with": "two words"}),
            json!({"op": "update", "item": 1, "with": ""}),
            json!({"op": "update", "item": 2, "with": "ana"}),
            json!({"op": "create", "kind": "note", "text": "n", "with": "ana"}),
        ] {
            let refusal = batch(&ekko, std::slice::from_ref(&refused));
            assert!(matches!(refusal, Err(EkkoError::InvalidInput(_))), "{refused}: {:?}", refusal.err());
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A priority outside 1 to 3 is refused as one, whether it would fit a
    /// u8 or not: -1 used to fail as JSON, with serde's 'expected u8'.
    #[test]
    fn a_priority_out_of_range_is_refused_as_a_priority() {
        let (ekko, dir) = board("priority-range");
        batch(&ekko, &[json!({"op": "create", "text": "a task"})]).unwrap();

        for priority in [-1, 0, 4, 300] {
            let created = batch(&ekko, &[json!({"op": "create", "text": "x", "priority": priority})]);
            assert!(matches!(created, Err(EkkoError::InvalidPriority)), "{priority}: {:?}", created.err());
            let updated = batch(&ekko, &[json!({"op": "update", "item": 1, "priority": priority})]);
            assert!(matches!(updated, Err(EkkoError::InvalidPriority)), "{priority}: {:?}", updated.err());
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A description takes up to MAX_DESCRIPTION characters, counted as
    /// characters, not bytes, however it gets there: created whole, or grown
    /// past it by an append.
    #[test]
    fn a_description_is_capped() {
        let (ekko, dir) = board("capped");
        let full = format!("capped\n{}", "é".repeat(crate::ekko::MAX_DESCRIPTION - "capped\n".len()));
        batch(&ekko, &[json!({"op": "create", "text": full})]).unwrap();

        let over = batch(&ekko, &[json!({"op": "create", "text": format!("{full}x")})]);
        assert!(matches!(&over, Err(EkkoError::InvalidInput(m)) if m.contains("runs 20001 characters")), "{:?}", over.err());
        let grown = batch(&ekko, &[json!({"op": "edit", "item": 1, "append": "more"})]);
        assert!(matches!(&grown, Err(EkkoError::InvalidInput(m)) if m.contains("past the 20000")), "{:?}", grown.err());
        let cli = ekko.edit_description(&["@1".to_string(), full.clone(), "x".to_string()]);
        assert!(matches!(&cli, Err(EkkoError::InvalidInput(m)) if m.contains("past the 20000")), "the terminal too: {:?}", cli.err());

        // An answer is held to the same length, and named as an answer.
        let mut draft = Draft::open(&ekko).unwrap();
        let asked = draft.ask("How long may an answer run?", None).unwrap();
        draft.commit(false).unwrap();
        let mut draft = Draft::open(&ekko).unwrap();
        let long = draft.answer(&Ref::Id(asked), &format!("{full}x"));
        assert!(matches!(&long, Err(EkkoError::InvalidInput(m)) if m.starts_with("The answer runs 20001 characters")), "{:?}", long.err());
        draft.answer(&Ref::Id(asked), &full).unwrap();

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A task's first line is its title, which the lists show: past
    /// MAX_TITLE characters it is refused on creation, and so is an edit
    /// that makes it so. What is appended to a task goes below its title, a
    /// note's first line runs as long as it likes, and a task written before
    /// titles keeps its long first line through edits that leave that line
    /// alone (task 260).
    #[test]
    fn a_task_starts_with_a_short_title() {
        let (ekko, dir) = board("titled");
        let long = "t".repeat(crate::ekko::MAX_TITLE + 1);
        let refused = batch(&ekko, &[json!({"op": "create", "text": long})]);
        assert!(
            matches!(&refused, Err(EkkoError::InvalidInput(m)) if m.contains("at most 80 characters, and this one runs 81")),
            "{:?}",
            refused.err()
        );
        let cli = ekko.create_task(std::slice::from_ref(&long));
        assert!(matches!(&cli, Err(EkkoError::InvalidInput(m)) if m.contains("its title")), "the terminal too: {:?}", cli.err());

        let titled = format!("{}\n{long}", "t".repeat(crate::ekko::MAX_TITLE));
        batch(&ekko, &[json!({"op": "create", "text": titled})]).unwrap();
        batch(&ekko, &[json!({"op": "create", "kind": "note", "text": long})]).unwrap();
        let longer = batch(&ekko, &[json!({"op": "edit", "item": 1, "replace": {"old": "t\n", "new": "tt\n"}})]);
        assert!(matches!(&longer, Err(EkkoError::InvalidInput(m)) if m.contains("runs 81")), "{:?}", longer.err());
        let whole = batch(&ekko, &[json!({"op": "edit", "item": 1, "text": long})]);
        assert!(matches!(&whole, Err(EkkoError::InvalidInput(_))), "{:?}", whole.err());
        let cli = ekko.edit_description(&["@1".to_string(), long.clone()]);
        assert!(matches!(&cli, Err(EkkoError::InvalidInput(m)) if m.contains("its title")), "the terminal too: {:?}", cli.err());

        batch(&ekko, &[json!({"op": "create", "text": "short"})]).unwrap();
        batch(&ekko, &[json!({"op": "edit", "item": 3, "append": "why it matters"})]).unwrap();
        let grown = batch(&ekko, &[json!({"op": "edit", "item": 3, "append": "and more"})]).unwrap();
        assert_eq!(grown.data[&3].description, "short\nwhy it matters and more");

        // A task written before titles, one long line.
        let mut data = ekko.storage.get().unwrap();
        data.get_mut(&3).unwrap().description = long.clone();
        ekko.storage.set(&data).unwrap();
        let kept = batch(&ekko, &[json!({"op": "edit", "item": 3, "append": "more"})]).unwrap();
        assert_eq!(kept.data[&3].description, format!("{long}\nmore"));
        batch(&ekko, &[json!({"op": "edit", "item": 3, "replace": {"old": "more", "new": "less"}})]).unwrap();

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A board the terminal could not address -- empty, a bare @, or two
    /// words -- is refused wherever a name is put on an item; the default
    /// board's own name, space and all, still is one.
    #[test]
    fn a_board_name_is_one_word() {
        let (ekko, dir) = board("board-names");
        batch(&ekko, &[json!({"op": "create", "text": "a task", "boards": ["My Board", "@ok"]})]).unwrap();

        for name in ["", "  ", "@", "two words", "@tab\there"] {
            let refusals = [
                batch(&ekko, &[json!({"op": "create", "text": "x", "boards": [name]})]),
                batch(&ekko, &[json!({"op": "update", "item": 1, "boards": ["ok", name]})]),
                batch(&ekko, &[json!({"op": "update", "item": 1, "add_boards": [name]})]),
            ];
            for refused in refusals {
                assert!(matches!(&refused, Err(EkkoError::InvalidInput(m)) if m.contains("cannot be a board name")), "{name:?}: {:?}", refused.err());
            }
        }
        batch(&ekko, &[json!({"op": "update", "item": 1, "remove_boards": ["two words"]})]).unwrap();

        std::fs::remove_dir_all(&dir).ok();
    }

    /// undone on a cancelled task changes nothing, by design, and the reply
    /// says so instead of a bare ok; a done task reopens without a word.
    #[test]
    fn undone_on_a_cancelled_task_says_it_changed_nothing() {
        let (ekko, dir) = board("undone-cancelled");
        batch(&ekko, &[json!({"op": "create", "text": "dropped"}), json!({"op": "create", "text": "finished"})]).unwrap();
        batch(&ekko, &[json!({"op": "set_state", "items": [1], "state": "cancelled"})]).unwrap();
        batch(&ekko, &[json!({"op": "set_state", "items": [2], "state": "done"})]).unwrap();

        let undone = batch(&ekko, &[json!({"op": "set_state", "items": [1, 2], "state": "undone"})]).unwrap();
        assert_eq!(undone.notices, vec!["1 is cancelled, and undone reopens only done work: nothing changed; unstarted revives it"]);
        assert_eq!((state_of(&ekko, 1), state_of(&ekko, 2)), (Some(State::Cancelled), Some(State::Pending)));

        std::fs::remove_dir_all(&dir).ok();
    }

    fn state_of(ekko: &Ekko, id: u32) -> Option<State> {
        State::of(&ekko.storage.get().unwrap()[&id])
    }

    /// A session that sets a task in progress holds it; another, while the
    /// first runs, is refused as HELD on any change of its state, and takes
    /// it over only by force; leaving progress lets it go.
    #[test]
    fn a_task_in_progress_is_held_by_the_session_that_started_it() {
        let (me, other, _) = crate::holder::test_sessions();
        let (board, dir) = board("held");
        let mine = Ekko::new(Storage::new(&dir).unwrap()).acting_as(me.clone());
        let theirs = Ekko::new(Storage::new(&dir).unwrap()).acting_as(other.clone());
        batch(&mine, &[json!({"op": "create", "text": "the work"})]).unwrap();
        batch(&mine, &[json!({"op": "set_state", "items": [1], "state": "progress"})]).unwrap();
        let holder = board.storage.get().unwrap()[&1].held_by.clone().expect("held once in progress");
        assert!(me.is(&holder));

        for state in ["progress", "done", "paused"] {
            let refused = batch(&theirs, &[json!({"op": "set_state", "items": [1], "state": state})]);
            assert!(matches!(&refused, Err(EkkoError::Held(held)) if held[0].0 == 1 && held[0].1 == "default on pts/1"), "{state}: {:?}", refused.err());
        }
        assert_eq!(state_of(&board, 1), Some(State::Progress), "nothing was written");

        let mut draft = Draft::open(&theirs).unwrap();
        draft.set_state(&[Ref::Id(1)], "progress").unwrap();
        let forced = draft.commit(true).unwrap();
        assert_eq!(forced.notices, vec!["1 was in progress under default on pts/1, still running: taken over"]);
        assert!(other.is(board.storage.get().unwrap()[&1].held_by.as_ref().unwrap()));

        batch(&theirs, &[json!({"op": "set_state", "items": [1], "state": "done"})]).unwrap();
        assert_eq!(board.storage.get().unwrap()[&1].held_by, None, "done lets it go");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A task whose session is gone is free: setting it in progress takes it
    /// over and says so; the same session setting it again is told it is
    /// already its own; and a person at the terminal is never refused.
    #[test]
    fn a_gone_session_leaves_its_task_free_and_a_person_is_never_refused() {
        let (me, other, gone) = crate::holder::test_sessions();
        let (board, dir) = board("held-gone");
        let dead = Ekko::new(Storage::new(&dir).unwrap()).acting_as(gone);
        let mine = Ekko::new(Storage::new(&dir).unwrap()).acting_as(me.clone());
        batch(&dead, &[json!({"op": "create", "text": "left behind"}), json!({"op": "set_state", "items": ["$1"], "state": "progress"})]).unwrap();

        let taken = batch(&mine, &[json!({"op": "set_state", "items": [1], "state": "progress"})]).unwrap();
        assert_eq!(taken.notices, vec!["1 was in progress under default on pts/3, which is gone: now yours"]);
        let again = batch(&mine, &[json!({"op": "set_state", "items": [1], "state": "progress"})]).unwrap();
        assert!(again.notices[0].starts_with("1 was already in progress, yours since "), "{:?}", again.notices);

        let person = Ekko::new(Storage::new(&dir).unwrap()).acting_as(Actor::person());
        let theirs = Ekko::new(Storage::new(&dir).unwrap()).acting_as(other);
        assert!(batch(&theirs, &[json!({"op": "set_state", "items": [1], "state": "paused"})]).is_err());
        batch(&person, &[json!({"op": "set_state", "items": [1], "state": "paused"})]).unwrap();
        assert_eq!(state_of(&board, 1), Some(State::Paused), "the user decides");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A question asked on the board carries the session that asked and the
    /// revision; an answer closes it with who recorded it, once: a second is
    /// refused, and so is an answer to a note that asks nothing.
    #[test]
    fn a_question_waits_for_one_answer() {
        let (me, _, _) = crate::holder::test_sessions();
        let (board, dir) = board("questions");
        let mine = Ekko::new(Storage::new(&dir).unwrap()).acting_as(me.clone());
        batch(&mine, &[json!({"op": "create", "text": "the merge"})]).unwrap();
        let mut draft = Draft::open(&mine).unwrap();
        let asked = draft.ask("Merge auditoria now?", Some(&Ref::Id(1))).unwrap();
        draft.commit(false).unwrap();

        let note = board.storage.get().unwrap()[&asked].clone();
        let question = note.question.clone().expect("a question");
        assert!(me.is(question.asked_by.as_ref().unwrap()) && question.answer.is_none());
        assert_eq!(Some(question.rev), note.rev, "the revision of the write that asked it");
        assert_eq!(note.attached_to, board.storage.get().unwrap()[&1].uid);

        let person = Ekko::new(Storage::new(&dir).unwrap()).acting_as(Actor::person());
        let mut draft = Draft::open(&person).unwrap();
        draft.answer(&Ref::Id(asked), "  yes, after the rebase ").unwrap();
        draft.commit(false).unwrap();
        let answer = board.storage.get().unwrap()[&asked].question.clone().unwrap().answer.expect("answered");
        assert_eq!((answer.text.as_str(), answer.by.unwrap().label()), ("yes, after the rebase", "the user".to_string()));

        let mut draft = Draft::open(&mine).unwrap();
        let again = draft.answer(&Ref::Id(asked), "no").unwrap_err().to_string();
        assert!(again.contains("already answered: yes, after the rebase"), "{again}");
        assert!(draft.answer(&Ref::Id(1), "yes").unwrap_err().to_string().contains("1 is not a question"));

        std::fs::remove_dir_all(&dir).ok();
    }

    fn wait_on(ekko: &Ekko, item: u32, until: Option<&str>, text: &str) -> Result<Waited, EkkoError> {
        let mut draft = Draft::open(ekko)?;
        let spec = WaitOn { item: Ref::Id(item), until: until.map(str::to_string), text: Some(text.to_string()), cancel: false };
        let waited = draft.wait(&spec)?;
        draft.commit(false)?;
        Ok(waited)
    }

    /// A session waits on a task another holds in a note attached to it,
    /// which carries who waits and until what (task 389). The write that
    /// completes the task ends the wait, recording how and by whom, and
    /// tells its writer whose wait it was. Waiting again for the same, or on
    /// what is already there, writes nothing.
    #[test]
    fn a_wait_is_recorded_and_ended_by_the_write_that_brings_it() {
        let (me, other, _) = crate::holder::test_sessions();
        let (board, dir) = board("waits");
        let as_ = |actor: &Actor| Ekko::new(Storage::new(&dir).unwrap()).acting_as(actor.clone());
        let held = [json!({"op": "create", "text": "Release v0.15.0"}), json!({"op": "set_state", "items": [1], "state": "progress"})];
        batch(&as_(&other), &held).unwrap();

        let Waited::Recorded(note) = wait_on(&as_(&me), 1, None, "land 380: rebase, test, push").unwrap() else {
            panic!("a wait recorded")
        };
        let written = board.storage.get().unwrap()[&note].clone();
        let wait = written.wait.clone().expect("a wait");
        assert_eq!((wait.until, wait.over.is_none(), written.description.as_str()), (Until::Done, true, "land 380: rebase, test, push"));
        assert!(me.is(&wait.by));
        assert_eq!(Some(wait.rev), written.rev, "the revision of the write that recorded it");
        assert_eq!(written.attached_to, board.storage.get().unwrap()[&1].uid);
        assert_eq!(wait_on(&as_(&me), 1, Some("done"), "again").unwrap(), Waited::Already(note));

        let done = batch(&as_(&other), &[json!({"op": "set_state", "items": [1], "state": "done"})]).unwrap();
        assert_eq!(done.notices, vec![format!("1 is done: default on pts/1 waited on it (note {note}) and is told")]);
        let written = board.storage.get().unwrap()[&note].clone();
        let over = written.wait.unwrap().over.expect("over");
        assert_eq!(over.how, How::Done);
        assert!(other.is(over.by.as_ref().unwrap()));
        assert_eq!(Some(over.rev), written.rev, "the revision of the write that ended it");

        assert_eq!(wait_on(&as_(&me), 1, None, "late").unwrap(), Waited::Met(1, How::Done));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Until free, a wait ends as the task leaves its holder's hands: set
    /// back to pending, held by a session that has ended, or taken up by the
    /// one waiting. Until answered, as its question gets an answer. cancel
    /// drops a wait, and a wait is refused where it could never end.
    #[test]
    fn a_wait_ends_as_its_condition_says_or_is_dropped() {
        let (me, other, gone) = crate::holder::test_sessions();
        let (board, dir) = board("waits-free");
        let as_ = |actor: &Actor| Ekko::new(Storage::new(&dir).unwrap()).acting_as(actor.clone());
        let how = |note: u32| board.storage.get().unwrap()[&note].wait.clone().unwrap().over.map(|over| over.how);
        let tasks = [
            json!({"op": "create", "text": "held, then let go"}),
            json!({"op": "create", "text": "held by a session that has ended"}),
            json!({"op": "create", "text": "taken up by the one waiting"}),
            json!({"op": "set_state", "items": [1, 3], "state": "progress"}),
        ];
        batch(&as_(&other), &tasks).unwrap();
        batch(&as_(&gone), &[json!({"op": "set_state", "items": [2], "state": "progress"})]).unwrap();

        let Waited::Recorded(let_go) = wait_on(&as_(&me), 1, Some("free"), "take it up").unwrap() else { panic!() };
        batch(&as_(&other), &[json!({"op": "set_state", "items": [1], "state": "unstarted"})]).unwrap();
        assert_eq!(how(let_go), Some(How::Released));
        assert_eq!(wait_on(&as_(&me), 2, Some("free"), "take it up").unwrap(), Waited::Met(2, How::Ended));

        let Waited::Recorded(taken) = wait_on(&as_(&me), 3, Some("free"), "take it up").unwrap() else { panic!() };
        let writer = as_(&me);
        let mut draft = Draft::open(&writer).unwrap();
        draft.set_state(&[Ref::Id(3)], "progress").unwrap();
        let took = draft.commit(true).unwrap();
        assert_eq!(how(taken), Some(How::Yours));
        assert!(took.notices.contains(&format!("3 is yours now: this session's wait on it (note {taken}) is over")), "{:?}", took.notices);

        let writer = as_(&other);
        let mut draft = Draft::open(&writer).unwrap();
        let asked = draft.ask("Merge now?", Some(&Ref::Id(1))).unwrap();
        draft.commit(false).unwrap();
        let Waited::Recorded(answer) = wait_on(&as_(&me), asked, None, "merge after it").unwrap() else { panic!() };
        assert_eq!(board.storage.get().unwrap()[&answer].wait.clone().unwrap().until, Until::Answered);
        let writer = as_(&Actor::person());
        let mut draft = Draft::open(&writer).unwrap();
        draft.answer(&Ref::Id(asked), "yes").unwrap();
        draft.commit(false).unwrap();
        assert_eq!(how(answer), Some(How::Answered));

        let Waited::Recorded(dropped) = wait_on(&as_(&me), 1, None, "never mind").unwrap() else { panic!() };
        let writer = as_(&me);
        let mut draft = Draft::open(&writer).unwrap();
        assert_eq!(draft.unwait(&Ref::Id(1)).unwrap(), vec![dropped]);
        draft.commit(false).unwrap();
        assert_eq!(how(dropped), Some(How::Dropped));
        let writer = as_(&me);
        let mut draft = Draft::open(&writer).unwrap();
        assert!(draft.unwait(&Ref::Id(1)).unwrap().is_empty(), "nothing left to drop");
        drop(draft);

        let refused = |actor: &Actor, item: u32, until: Option<&str>, text: &str| {
            wait_on(&as_(actor), item, until, text).unwrap_err().to_string()
        };
        let person = refused(&Actor::person(), 1, None, "x");
        assert!(person.contains("the user at the terminal"), "{person}");
        assert!(refused(&me, dropped, None, "x").contains("is a note: a wait is on a task or a question"));
        assert!(refused(&me, 1, Some("answered"), "x").contains("1 is not a question"));
        assert!(refused(&me, asked, Some("done"), "x").contains("is a question: it is waited on until answered"));
        assert!(refused(&me, 1, Some("soon"), "x").contains("until is done, free or answered, not soon"));
        assert!(refused(&me, 1, None, "  ").contains("wait needs text"));
        let Err(retyped) = batch(&as_(&me), &[json!({"op": "update", "item": dropped, "kind": "decision"})]) else {
            panic!("a wait takes no kind")
        };
        assert!(retyped.to_string().contains("is a session's wait, which ends on its own"), "{retyped}");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Two sessions adding a blocker each, or a board each, both land: the
    /// add and remove forms change what is there instead of replacing it.
    /// And if_updated_at refuses an update or a link made against an older
    /// read, as it does an edit.
    #[test]
    fn adding_and_removing_commute_and_a_stale_read_is_refused() {
        let (ekko, dir) = board("commute");
        let ops = [json!({"op": "create", "text": "the task"}), json!({"op": "create", "text": "a"}), json!({"op": "create", "text": "b"})];
        batch(&ekko, &ops).unwrap();
        batch(&ekko, &[json!({"op": "link", "item": 1, "add_blocked_by": [2]})]).unwrap();
        batch(&ekko, &[json!({"op": "link", "item": 1, "add_blocked_by": [3]})]).unwrap();
        let data = ekko.storage.get().unwrap();
        let uid = |id: u32| data[&id].uid.clone().unwrap();
        assert_eq!(data[&1].blocked_by, Some(vec![uid(2), uid(3)]), "the second link kept the first");
        batch(&ekko, &[json!({"op": "link", "item": 1, "remove_blocked_by": [2]})]).unwrap();
        assert_eq!(ekko.storage.get().unwrap()[&1].blocked_by, Some(vec![uid(3)]));

        batch(&ekko, &[json!({"op": "update", "item": 1, "add_boards": ["x"]})]).unwrap();
        batch(&ekko, &[json!({"op": "update", "item": 1, "add_boards": ["y"], "remove_boards": ["My Board"]})]).unwrap();
        assert_eq!(ekko.storage.get().unwrap()[&1].boards, vec!["@x", "@y"]);
        let mixed = batch(&ekko, &[json!({"op": "update", "item": 1, "boards": ["z"], "add_boards": ["w"]})]);
        assert!(matches!(mixed, Err(EkkoError::InvalidInput(_))));

        let read = ekko.storage.get().unwrap()[&1].updated_at.unwrap();
        batch(&ekko, &[json!({"op": "update", "item": 1, "add_boards": ["z"]})]).unwrap();
        let stale = batch(&ekko, &[json!({"op": "update", "item": 1, "remove_boards": ["x"], "if_updated_at": read})]);
        assert!(matches!(stale, Err(EkkoError::Stale { id: 1, .. })));
        let stale = batch(&ekko, &[json!({"op": "link", "item": 1, "add_blocked_by": [2], "if_updated_at": read})]);
        assert!(matches!(stale, Err(EkkoError::Stale { id: 1, .. })));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// add_boards and remove_boards change only the boards they name, each
    /// way an update can give them: the item keeps every other board, the
    /// default one too, and only taking off the last leaves it on the
    /// default (task 1011).
    #[test]
    fn adding_or_removing_boards_keeps_every_board_not_named() {
        let (ekko, dir) = board("boards-kept");
        let cases = [
            (json!({"add_boards": ["b"]}), vec!["My Board", "@a", "@b"]),
            (json!({"add_boards": ["a"]}), vec!["My Board", "@a"]),
            (json!({"remove_boards": ["a"]}), vec!["My Board"]),
            (json!({"remove_boards": ["myboard"]}), vec!["@a"]),
            (json!({"remove_boards": [" "]}), vec!["My Board", "@a"]),
            (json!({"add_boards": ["b"], "remove_boards": ["a"]}), vec!["My Board", "@b"]),
            (json!({"remove_boards": ["My Board", "a"]}), vec!["My Board"]),
        ];
        for (id, (change, expected)) in (1..).zip(cases) {
            batch(&ekko, &[json!({"op": "create", "text": format!("item {id}"), "boards": ["My Board", "@a"]})]).unwrap();
            let mut update = json!({"op": "update", "item": id});
            update.as_object_mut().unwrap().extend(change.as_object().unwrap().clone());
            batch(&ekko, &[update]).unwrap();
            assert_eq!(ekko.storage.get().unwrap()[&id].boards, expected, "{change}");
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    /// With attached_to and no kind, a create makes a note: a task is never
    /// attached, and the refusal this used to get only cost the agent a call.
    /// A task asked for by name is still refused.
    #[test]
    fn attached_with_no_kind_makes_a_note() {
        let (ekko, dir) = board("attach-default");
        let written = batch(
            &ekko,
            &[json!({"op": "create", "text": "the task"}), json!({"op": "create", "text": "why it waits", "attached_to": "$1"})],
        )
        .unwrap();

        let data = &written.data;
        assert!(!data[&2].is_task, "a note");
        assert_eq!(data[&2].attached_to, data[&1].uid);
        let task = batch(&ekko, &[json!({"op": "create", "kind": "task", "text": "no", "attached_to": 1})]);
        assert!(matches!(task, Err(EkkoError::AttachNotANote(_))));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A batch can build a small plan in one write: later operations name the
    /// items earlier ones created, by position.
    #[test]
    fn a_batch_refers_to_what_it_created_and_writes_once() {
        let (ekko, dir) = board("refs");
        let written = batch(
            &ekko,
            &[
                json!({"op": "create", "text": "design"}),
                json!({"op": "create", "text": "build", "blocked_by": ["$1"]}),
                json!({"op": "create", "kind": "note", "text": "why build waits", "attached_to": "$2"}),
                json!({"op": "set_state", "items": ["$1"], "state": "done"}),
            ],
        )
        .unwrap();

        let data = &written.data;
        assert_eq!(data[&2].blocked_by, Some(vec![data[&1].uid.clone().unwrap()]));
        assert_eq!(data[&3].attached_to, data[&2].uid);
        assert_eq!(State::of(&data[&1]), Some(State::Done));
        assert_eq!(ekko.storage.get().unwrap(), *data, "what was reported is not what was written");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A handoff is a note on an open task, and the newest one is the only
    /// handoff there: the one it replaces stays on the task as a plain note.
    #[test]
    fn a_handoff_replaces_the_one_before_on_its_task() {
        let (ekko, dir) = board("handoff");
        batch(
            &ekko,
            &[
                json!({"op": "create", "text": "the work"}),
                json!({"op": "create", "kind": "handoff", "text": "stopped at the parser", "attached_to": "$1"}),
            ],
        )
        .unwrap();
        let written = batch(&ekko, &[json!({"op": "create", "kind": "handoff", "text": "parser done, tests next", "attached_to": 1})]).unwrap();

        let data = &written.data;
        assert!(!data[&3].is_task && data[&3].handoff);
        assert_eq!(data[&3].attached_to, data[&1].uid);
        assert!(!data[&2].handoff, "the earlier handoff is demoted");
        assert_eq!(data[&2].attached_to, data[&1].uid, "and kept on the task");
        assert_eq!(data[&3].supersedes, data[&2].uid, "which the new one supersedes");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A handoff replaces this session's earlier one on its task, and one by
    /// a session that has ended, but not one another session that still runs
    /// wrote: that session resumes from it (task 514). Seen on 2026-09-24,
    /// when two sessions handed off on one board and the later handoff on a
    /// task would have taken the earlier one from the session still running.
    #[test]
    fn a_handoff_leaves_another_running_sessions_one_in_place() {
        let (me, other, gone) = crate::holder::test_sessions();
        let (ekko, dir) = board("handoff-sessions");
        batch(&ekko, &[json!({"op": "create", "text": "the work"})]).unwrap();
        let handoff = |actor: &Actor, text: &str| {
            let ekko = Ekko::new(Storage::new(&dir).unwrap()).acting_as(actor.clone());
            batch(&ekko, &[json!({"op": "create", "kind": "handoff", "text": text, "attached_to": 1})]).unwrap().data
        };
        handoff(&gone, "left behind");
        let data = handoff(&other, "their stop");
        assert!(!data[&2].handoff, "an ended session's is replaced");
        assert_eq!(data[&3].supersedes, data[&2].uid);

        let data = handoff(&me, "my stop");
        assert!(data[&3].handoff, "a running session's stays");
        assert!(data[&4].handoff);
        assert_eq!(data[&4].supersedes, None, "and is superseded by nothing");

        let data = handoff(&me, "my later stop");
        assert!(!data[&4].handoff, "this session's own is replaced");
        assert!(data[&3].handoff, "and the running session's still stays");
        assert_eq!(data[&5].supersedes, data[&4].uid);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Completing a task names the notes without a kind still attached to it,
    /// which leave the prime with it; a typed note, a handoff and a question
    /// are left out, and nothing is written for any of them (task 398).
    #[test]
    fn completing_a_task_names_the_notes_it_leaves_behind() {
        let (ekko, dir) = board("loose-notes");
        batch(
            &ekko,
            &[
                json!({"op": "create", "text": "the work"}),
                json!({"op": "create", "text": "an idea for later", "attached_to": "$1"}),
                json!({"op": "create", "kind": "gotcha", "text": "a trap", "attached_to": "$1"}),
                json!({"op": "create", "kind": "handoff", "text": "where it stopped", "attached_to": "$1"}),
                json!({"op": "create", "text": "BUILT in abc123", "attached_to": "$1"}),
                json!({"op": "create", "text": "small work"}),
                json!({"op": "create", "text": "its one note", "attached_to": "$6"}),
                json!({"op": "create", "text": "bare work"}),
            ],
        )
        .unwrap();
        let mut draft = Draft::open(&ekko).unwrap();
        draft.ask("Which way?", Some(&Ref::Id(1))).unwrap();
        draft.commit(false).unwrap();
        let before = ekko.storage.get().unwrap();

        let done = batch(&ekko, &[json!({"op": "set_state", "items": [1, 6, 8], "state": "done"})]).unwrap();
        assert_eq!(
            done.notices,
            vec![
                "1 is done; notes 2 and 5 stay attached to it and leave the prime with it. One that still proposes \
                 something becomes a task of its own or gets a line saying why not; a lesson one holds goes into a \
                 decision, gotcha or procedure",
                "6 is done; note 7 stays attached to it and leaves the prime with it. If it still proposes something, \
                 make that a task of its own or append why not; a lesson it holds goes into a decision, gotcha or \
                 procedure",
            ]
        );
        for note in [2, 3, 4, 5, 7, 9] {
            assert_eq!(done.data[&note], before[&note], "note {note} is left as it was");
        }
        let again = batch(&ekko, &[json!({"op": "set_state", "items": [1], "state": "done"})]).unwrap();
        assert!(again.notices.is_empty(), "only the write that completes it says so: {:?}", again.notices);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A handoff a later one replaced keeps no kind: what it holds that stays
    /// true goes into a note of its own, as with the handoff in force. Moved
    /// to another task, the later one is an ordinary note there, and the one
    /// it replaced is no longer replaced by it.
    #[test]
    fn a_replaced_handoff_takes_no_kind_and_a_moved_one_lets_go_of_it() {
        let (ekko, dir) = board("handoff-line");
        batch(
            &ekko,
            &[
                json!({"op": "create", "text": "the work"}),
                json!({"op": "create", "kind": "handoff", "text": "stopped at the parser", "attached_to": "$1"}),
                json!({"op": "create", "kind": "handoff", "text": "parser done, tests next", "attached_to": "$1"}),
                json!({"op": "create", "text": "other work"}),
            ],
        )
        .unwrap();

        let retyped = refusal(batch(&ekko, &[json!({"op": "update", "item": 2, "kind": "decision"})]));
        assert!(retyped.contains("a later one replaced") && retyped.contains("a note of its own"), "{retyped}");

        let moved = batch(&ekko, &[json!({"op": "link", "item": 3, "attached_to": 4})]).unwrap();
        assert!(!moved.data[&3].handoff && moved.data[&3].supersedes.is_none(), "{:?}", moved.data[&3]);
        batch(&ekko, &[json!({"op": "update", "item": 2, "kind": "decision"})]).unwrap();

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Nothing to hand over without a task, or on one already finished.
    #[test]
    fn a_handoff_needs_an_open_task() {
        let (ekko, dir) = board("handoff-refused");
        batch(&ekko, &[json!({"op": "create", "text": "finished work"}), json!({"op": "set_state", "items": [1], "state": "done"})])
            .unwrap();

        let loose = batch(&ekko, &[json!({"op": "create", "kind": "handoff", "text": "where I stopped"})]);
        assert!(matches!(loose, Err(EkkoError::InvalidInput(ref m)) if m.contains("attached_to")), "{:?}", loose.as_ref().err());
        let finished = batch(&ekko, &[json!({"op": "create", "kind": "handoff", "text": "where I stopped", "attached_to": 1})]);
        assert!(matches!(finished, Err(EkkoError::InvalidInput(ref m)) if m.contains("finished")), "{:?}", finished.as_ref().err());
        assert_eq!(ekko.storage.get().unwrap().len(), 1, "a refused handoff wrote nothing");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A handoff hands over the task it was written on: moved to another task
    /// or detached, it is an ordinary note.
    #[test]
    fn a_moved_handoff_is_an_ordinary_note() {
        let (ekko, dir) = board("handoff-moved");
        batch(
            &ekko,
            &[
                json!({"op": "create", "text": "first"}),
                json!({"op": "create", "text": "second"}),
                json!({"op": "create", "kind": "handoff", "text": "where I stopped", "attached_to": "$1"}),
            ],
        )
        .unwrap();
        let moved = batch(&ekko, &[json!({"op": "link", "item": 3, "attached_to": 2})]).unwrap();
        assert!(!moved.data[&3].handoff);
        assert_eq!(moved.data[&3].attached_to, moved.data[&2].uid);

        std::fs::remove_dir_all(&dir).ok();
    }

    fn refusal(result: Result<Committed, EkkoError>) -> String {
        match result {
            Err(EkkoError::InvalidInput(message)) => message,
            Err(other) => panic!("refused as {other:?}, not as invalid input"),
            Ok(_) => panic!("written, not refused"),
        }
    }

    /// A decision names the one it replaces, and only the newer note is
    /// written: the older one stays exactly as it was, as history.
    #[test]
    fn a_decision_supersedes_an_earlier_one_without_touching_it() {
        let (ekko, dir) = board("supersede");
        batch(&ekko, &[json!({"op": "create", "kind": "decision", "text": "ship weekly"})]).unwrap();
        let before = ekko.storage.get().unwrap()[&1].clone();
        let written =
            batch(&ekko, &[json!({"op": "create", "kind": "decision", "text": "ship on demand", "supersedes": 1})]).unwrap();

        let data = &written.data;
        assert!(!data[&2].is_task && !data[&2].handoff);
        assert_eq!(data[&2].knowledge, Some(Knowledge::Decision));
        assert_eq!(data[&2].supersedes, before.uid);
        assert_eq!(data[&1], before, "the superseded note is not rewritten");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A note replaces one of its own kind that is still in force, and each
    /// note is replaced once: the rest is refused and writes nothing.
    #[test]
    fn supersedes_takes_one_note_of_the_same_kind_still_in_force() {
        let (ekko, dir) = board("supersede-refused");
        batch(
            &ekko,
            &[
                json!({"op": "create", "kind": "decision", "text": "one"}),
                json!({"op": "create", "kind": "gotcha", "text": "a trap"}),
                json!({"op": "create", "text": "a task"}),
                json!({"op": "create", "kind": "decision", "text": "thrown away"}),
                json!({"op": "create", "kind": "decision", "text": "two", "supersedes": "$1"}),
            ],
        )
        .unwrap();
        ekko.set_trashed(&["4".to_string()], true).unwrap();
        let create = |kind: &str, supersedes: u32| json!({"op": "create", "kind": kind, "text": "x", "supersedes": supersedes});

        assert!(refusal(batch(&ekko, &[create("gotcha", 1)])).contains("1 is a decision, and a gotcha supersedes only a gotcha"));
        assert!(refusal(batch(&ekko, &[create("decision", 2)])).contains("2 is a gotcha"));
        assert!(refusal(batch(&ekko, &[create("decision", 3)])).contains("3 is a task"));
        assert!(refusal(batch(&ekko, &[create("decision", 4)])).contains("in the trash"));
        assert!(refusal(batch(&ekko, &[create("note", 1)])).contains("Only a decision, a gotcha or a procedure"));
        assert!(refusal(batch(&ekko, &[create("decision", 1)])).contains("already superseded by 5: supersede 5 instead"));
        assert_eq!(ekko.storage.get().unwrap().len(), 5, "no refusal wrote anything");

        // A replacement in the trash replaces nothing, and frees its place.
        ekko.set_trashed(&["5".to_string()], true).unwrap();
        let again = batch(&ekko, &[create("decision", 1)]).unwrap();
        assert_eq!(again.data[&6].supersedes, again.data[&1].uid);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// link sets what a note supersedes after the fact, or clears it, and
    /// refuses a line of replacements that would turn back on itself.
    #[test]
    fn link_sets_and_clears_supersedes_and_refuses_a_loop() {
        let (ekko, dir) = board("supersede-link");
        batch(
            &ekko,
            &[
                json!({"op": "create", "kind": "procedure", "text": "release by hand"}),
                json!({"op": "create", "kind": "procedure", "text": "release with the script"}),
                json!({"op": "create", "kind": "procedure", "text": "release from CI"}),
                json!({"op": "create", "text": "an ordinary note", "kind": "note"}),
            ],
        )
        .unwrap();
        let link = |item: u32, supersedes: Value| json!({"op": "link", "item": item, "supersedes": supersedes});

        let linked = batch(&ekko, &[link(2, json!(1)), link(3, json!(2))]).unwrap();
        assert_eq!(linked.data[&3].supersedes, linked.data[&2].uid);
        assert!(refusal(batch(&ekko, &[link(1, json!(3))])).contains("3 already supersedes 1"), "a loop through 2 is refused");
        assert!(refusal(batch(&ekko, &[link(1, json!(1))])).contains("cannot supersede itself"));
        assert!(refusal(batch(&ekko, &[link(4, json!(1))])).contains("4 is an ordinary note"));

        let cleared = batch(&ekko, &[link(3, Value::Null)]).unwrap();
        assert_eq!(cleared.data[&3].supersedes, None);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// update types a note written before typed notes existed, or makes a
    /// typed one ordinary again -- never a task, a handoff, or a note in a
    /// line of supersession, whose kind the whole line shares.
    #[test]
    fn update_retypes_a_note_but_not_a_task_a_handoff_or_a_line() {
        let (ekko, dir) = board("retype");
        batch(
            &ekko,
            &[
                json!({"op": "create", "text": "the work"}),
                json!({"op": "create", "kind": "note", "text": "settled long ago"}),
                json!({"op": "create", "kind": "handoff", "text": "stopped here", "attached_to": "$1"}),
                json!({"op": "create", "kind": "gotcha", "text": "a trap"}),
                json!({"op": "create", "kind": "gotcha", "text": "the same trap, better", "supersedes": "$4"}),
            ],
        )
        .unwrap();
        let retype = |item: u32, kind: &str| json!({"op": "update", "item": item, "kind": kind});

        let typed = batch(&ekko, &[retype(2, "decision")]).unwrap();
        assert_eq!(typed.data[&2].knowledge, Some(Knowledge::Decision));
        let plain = batch(&ekko, &[retype(2, "note")]).unwrap();
        assert_eq!(plain.data[&2].knowledge, None);

        assert!(refusal(batch(&ekko, &[retype(1, "decision")])).contains("1 is a task"));
        assert!(refusal(batch(&ekko, &[retype(3, "decision")])).contains("handoff"));
        assert!(refusal(batch(&ekko, &[retype(2, "task")])).contains("note, decision, gotcha or procedure"));
        assert!(refusal(batch(&ekko, &[retype(4, "procedure")])).contains("clear supersedes with link first"));
        assert!(refusal(batch(&ekko, &[retype(5, "procedure")])).contains("clear supersedes with link first"));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// All or nothing: a refusal anywhere in the batch, including the
    /// dependency rule checked on the board the batch would leave, writes
    /// nothing at all.
    #[test]
    fn a_refused_batch_writes_nothing() {
        let (ekko, dir) = board("refused");
        let cycle = batch(
            &ekko,
            &[
                json!({"op": "create", "text": "a"}),
                json!({"op": "create", "text": "b", "blocked_by": ["$1"]}),
                json!({"op": "link", "item": "$1", "blocked_by": ["$2"]}),
            ],
        );
        assert!(matches!(cycle, Err(EkkoError::BlockingCycle(_, _))), "{:?}", cycle.err());

        let rule = batch(
            &ekko,
            &[
                json!({"op": "create", "text": "a"}),
                json!({"op": "create", "text": "b", "blocked_by": ["$1"]}),
                json!({"op": "set_state", "items": ["$2"], "state": "done"}),
            ],
        );
        assert!(matches!(rule, Err(EkkoError::Blocked(_))), "{:?}", rule.err());

        let bad_ref = batch(&ekko, &[json!({"op": "set_state", "items": ["$1"], "state": "done"})]);
        assert!(matches!(bad_ref, Err(EkkoError::InvalidInput(_))));

        assert!(ekko.storage.get().unwrap().is_empty(), "a refused batch wrote something");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A refusal names an item on the board by its id, and one the refused
    /// write would have created -- which exists nowhere -- by the operation
    /// that created it, never by the number it held in the draft.
    #[test]
    fn a_refusal_names_what_it_would_have_created_by_operation() {
        let (ekko, dir) = board("names");
        batch(&ekko, &[json!({"op": "create", "text": "on the board"})]).unwrap();

        let mut draft = Draft::open(&ekko).unwrap();
        draft.apply(&op(json!({"op": "create", "text": "drafted"}))).unwrap();
        let names = draft.names();
        assert_eq!(names.name(1), "1");
        assert_eq!(names.name(2), "$1 (from operation 1)");
        assert_eq!(names.name(3), "the new item");
        drop(draft);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A text keeps `$N` as written, since prose may mean a dollar sign, and
    /// the reply names the item it would be: an edit once appended 'task $2'
    /// for the task the second operation created, and nothing said so (task
    /// 394). Only an N whose operation created another item is named, earlier
    /// or later than the text, once per operation.
    #[test]
    fn a_text_keeps_dollar_n_as_written_and_the_reply_names_the_item() {
        let (ekko, dir) = board("kept-references");
        let text = "why $1 waits on $5 and $1, not $2, at $0 or $1.50, on the $1st";
        let written = batch(
            &ekko,
            &[
                json!({"op": "create", "text": "the task"}),
                json!({"op": "create", "text": text}),
                json!({"op": "update", "item": "$1", "priority": 2}),
                json!({"op": "edit", "item": "$2", "append": " (see $3 and $4)"}),
                json!({"op": "create", "text": "a later one", "attached_to": "$1"}),
                json!({"op": "edit", "item": "$1", "replace": {"old": "task", "new": "task before $5"}}),
            ],
        )
        .unwrap();

        assert_eq!(
            written.notices,
            vec![
                "operation 2's text holds $1, which batch does not replace: that item is 1",
                "operation 2's text holds $5, which batch does not replace: that item is 3",
                "operation 6's text holds $5, which batch does not replace: that item is 3",
            ]
        );
        assert_eq!(written.data[&2].description, format!("{text}\n(see $3 and $4)"), "what is added to a task goes below its title");
        assert_eq!(written.data[&1].description, "the task before $5");
        let alone = batch(&ekko, &[json!({"op": "create", "text": "costs $1"})]).unwrap();
        assert!(alone.notices.is_empty(), "{:?}", alone.notices);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_dollar_n_is_digits_that_end_the_word() {
        assert_eq!(dollar_numbers("$1, $2. ($3) $10 $4.50 $5,00 $6th $7_ $ 8 $"), vec![1, 2, 3, 10]);
        assert_eq!(dollar_numbers("é$2é $3é"), Vec::<usize>::new());
    }

    /// A commit names the tasks its write set free and the ones it left
    /// waiting, from either side of a dependency -- and only tasks that
    /// existed before it: work the write created is the caller's own doing.
    #[test]
    fn a_commit_names_what_it_released_and_what_it_blocked() {
        let (ekko, dir) = board("readiness");
        batch(
            &ekko,
            &[
                json!({"op": "create", "text": "blocker"}),
                json!({"op": "create", "text": "one", "blocked_by": ["$1"]}),
                json!({"op": "create", "text": "two", "blocked_by": ["$1"]}),
                json!({"op": "create", "text": "free"}),
            ],
        )
        .unwrap();

        let done = batch(&ekko, &[json!({"op": "set_state", "items": [1], "state": "done"})]).unwrap();
        assert_eq!((done.released, done.blocked), (vec![2, 3], vec![]));

        let again = batch(&ekko, &[json!({"op": "set_state", "items": [1], "state": "done"})]).unwrap();
        assert!(again.released.is_empty() && again.blocked.is_empty(), "a retry released something new");

        let reopened = batch(&ekko, &[json!({"op": "set_state", "items": [1], "state": "undone"})]).unwrap();
        assert_eq!((reopened.released, reopened.blocked), (vec![], vec![2, 3]));

        let linked = batch(&ekko, &[json!({"op": "link", "item": 4, "blocked_by": [1]})]).unwrap();
        assert_eq!((linked.released, linked.blocked), (vec![], vec![4]));

        let created = batch(&ekko, &[json!({"op": "create", "text": "new and waiting", "blocked_by": [1]})]).unwrap();
        assert!(created.released.is_empty() && created.blocked.is_empty(), "a task the write created was reported");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A note is neither a blocker nor something with a state: blocking on one
    /// would block nothing, and a task state set on one would change nothing
    /// while the reply said ok. Both are refused; a star still applies.
    #[test]
    fn a_note_can_neither_block_nor_take_a_task_state() {
        let (ekko, dir) = board("notes");
        batch(&ekko, &[json!({"op": "create", "kind": "note", "text": "a note"})]).unwrap();

        let blocked = batch(&ekko, &[json!({"op": "create", "text": "waits on a note", "blocked_by": [1]})]);
        assert!(matches!(blocked, Err(EkkoError::InvalidInput(_))), "{:?}", blocked.err());
        let stated = batch(&ekko, &[json!({"op": "set_state", "items": [1], "state": "done"})]);
        assert!(matches!(stated, Err(EkkoError::InvalidInput(_))), "{:?}", stated.err());
        assert!(batch(&ekko, &[json!({"op": "set_state", "items": [1], "state": "starred"})]).is_ok());

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A misspelled field is refused, not ignored: an ignored `blockedBy`
    /// would create the task and silently drop the dependency.
    #[test]
    fn an_unknown_field_is_refused() {
        let wrong = serde_json::from_value::<Op>(json!({"op": "create", "text": "x", "blockedBy": [1]}));
        assert!(wrong.is_err());
        let unknown_op = serde_json::from_value::<Op>(json!({"op": "destroy"}));
        assert!(unknown_op.is_err());
    }

    /// A replacement names one exact occurrence, and an edit conditioned on a
    /// version refuses once the item has moved on.
    #[test]
    fn an_edit_replaces_one_exact_occurrence_against_the_version_it_read() {
        let (ekko, dir) = board("edit");
        let written = batch(&ekko, &[json!({"op": "create", "kind": "note", "text": "alpha beta alpha"})]).unwrap();
        let version = written.data[&1].updated_at.unwrap();

        let twice = batch(&ekko, &[json!({"op": "edit", "item": 1, "replace": {"old": "alpha", "new": "x"}})]);
        assert!(matches!(twice, Err(EkkoError::EditMatch { found: 2, .. })));
        let nowhere = batch(&ekko, &[json!({"op": "edit", "item": 1, "replace": {"old": "gamma", "new": "x"}})]);
        assert!(matches!(nowhere, Err(EkkoError::EditMatch { found: 0, .. })));

        std::thread::sleep(std::time::Duration::from_millis(5));
        let replaced = batch(
            &ekko,
            &[json!({"op": "edit", "item": 1, "replace": {"old": "beta", "new": "gamma"}, "if_updated_at": version})],
        )
        .unwrap();
        assert_eq!(replaced.data[&1].description, "alpha gamma alpha");

        let stale = batch(&ekko, &[json!({"op": "edit", "item": 1, "append": "late", "if_updated_at": version})]);
        assert!(matches!(stale, Err(EkkoError::Stale { .. })), "{:?}", stale.err());

        let appended = batch(&ekko, &[json!({"op": "edit", "item": 1, "append": "omega"})]).unwrap();
        assert_eq!(appended.data[&1].description, "alpha gamma alpha omega");
        for empty in ["", "  "] {
            let nothing = batch(&ekko, &[json!({"op": "edit", "item": 1, "append": empty})]);
            assert!(matches!(&nothing, Err(EkkoError::InvalidInput(m)) if m.contains("append is empty")), "{:?}", nothing.err());
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Moving an item between phases is held to the phase order its
    /// dependencies already follow, and `null` clears what it names.
    #[test]
    fn an_update_keeps_dependencies_along_the_phase_order() {
        let (ekko, dir) = board("update");
        ekko.set_phases(&["early".to_string(), "late".to_string()]).unwrap();
        batch(
            &ekko,
            &[
                json!({"op": "create", "text": "first", "phase": "early", "due": "2026-01-01"}),
                json!({"op": "create", "text": "second", "phase": "late", "blocked_by": ["$1"]}),
            ],
        )
        .unwrap();

        let same = batch(&ekko, &[json!({"op": "update", "item": 2, "phase": "early"})]);
        assert!(same.is_ok(), "moving into its blocker's phase was refused: {:?}", same.err());
        let against = batch(&ekko, &[json!({"op": "update", "item": 1, "phase": "late"})]);
        assert!(
            matches!(against, Err(EkkoError::PhaseOrder(_))),
            "a blocker moved to a phase after what it blocks: {:?}",
            against.err()
        );

        let unknown = batch(&ekko, &[json!({"op": "update", "item": 1, "phase": "nowhere"})]);
        assert!(matches!(unknown, Err(EkkoError::InvalidInput(_))));

        let cleared = batch(&ekko, &[json!({"op": "update", "item": 1, "due": null})]).unwrap();
        assert_eq!(cleared.data[&1].due_date, None);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The person changes and deletes their own comments only, and only on
    /// the artifact they are on (task 1213); a theme takes one of the
    /// colors and a short name; a deleted comment goes to the trash, where
    /// it is no longer changed.
    #[test]
    fn the_person_edits_and_trashes_their_own_comments_only() {
        let (me, _, _) = crate::holder::test_sessions();
        let (board, dir) = board("comments");
        let person = Ekko::new(Storage::new(&dir).unwrap()).acting_as(Actor::person());
        let session = Ekko::new(Storage::new(&dir).unwrap()).acting_as(me);
        let plan = "Ship\n\n## Goal\nWhy.\n## What is known\nFacts.\n## Design\nHow.\n## Risks and open questions\nNone.";
        let spec: ArtifactSpec = serde_json::from_value(json!({"text": plan, "steps": [{"key": "one", "text": "First"}]})).unwrap();
        let mut draft = Draft::open(&session).unwrap();
        let (first, second) = (draft.artifact(&spec).unwrap(), draft.artifact(&spec).unwrap());
        draft.commit(false).unwrap();
        let comment = |theme: Option<&str>, color: Option<&str>| crate::item::Comment {
            version: 1,
            quote: Some(crate::item::Quote { exact: "Facts.".into(), prefix: String::new(), suffix: String::new(), section: String::new(), unknown: Default::default() }),
            replacement: None,
            step: None,
            reply_to: None,
            sent: None,
            resolved: None,
            theme: theme.map(str::to_string),
            color: color.map(str::to_string),
            unknown: Default::default(),
        };
        let long = "x".repeat(41);
        for (theme, color) in [(None, Some("teal")), (Some(""), Some("blue")), (Some(" Question"), Some("blue")), (Some(long.as_str()), None), (Some("a\nb"), None)] {
            let mut draft = Draft::open(&person).unwrap();
            assert!(draft.comment(&Ref::Id(first), "Why?", comment(theme, color)).is_err(), "{theme:?} in {color:?}");
        }
        let mut draft = Draft::open(&person).unwrap();
        let mine = draft.comment(&Ref::Id(first), "Why facts?", comment(Some("Question"), Some("blue"))).unwrap();
        draft.commit(false).unwrap();
        let mut draft = Draft::open(&session).unwrap();
        let theirs = draft.comment(&Ref::Id(first), "Because.", comment(None, None)).unwrap();
        draft.commit(false).unwrap();
        let uid = |id: u32| board.storage.get().unwrap()[&id].uid.clone().unwrap();

        let mut draft = Draft::open(&person).unwrap();
        assert!(draft.edit_comment(&Ref::Id(first), &uid(theirs), Some("Mine now"), None, None).is_err(), "a session's comment is the session's");
        assert!(draft.trash_comment(&Ref::Id(first), &uid(theirs)).is_err(), "a session's comment is the session's");
        assert!(draft.edit_comment(&Ref::Id(second), &uid(mine), Some("Elsewhere"), None, None).is_err(), "a comment on another artifact");
        assert!(draft.edit_comment(&Ref::Id(first), &uid(first), Some("The plan"), None, None).is_err(), "the artifact is no comment");
        assert!(draft.edit_comment(&Ref::Id(first), &uid(mine), Some("Which facts?"), Some("Dúvida"), Some("teal")).is_err(), "an unknown color");
        assert!(draft.edit_comment(&Ref::Id(first), &uid(mine), Some("  "), None, None).is_err(), "a comment keeps words");
        draft.edit_comment(&Ref::Id(first), &uid(mine), Some("Which facts?"), Some("Dúvida"), Some("pink")).unwrap();
        draft.commit(false).unwrap();
        let data = board.storage.get().unwrap();
        let kept = data[&mine].comment.as_deref().unwrap();
        assert_eq!((data[&mine].description.as_str(), kept.theme.as_deref(), kept.color.as_deref()), ("Which facts?", Some("Dúvida"), Some("pink")));
        assert_eq!(data[&theirs].description, "Because.", "the session's comment is untouched");

        let mut draft = Draft::open(&person).unwrap();
        draft.trash_comment(&Ref::Id(first), &uid(mine)).unwrap();
        draft.commit(false).unwrap();
        assert!(board.storage.get().unwrap()[&mine].trashed.is_some(), "in the trash, kept 30 days");
        let mut draft = Draft::open(&person).unwrap();
        assert!(draft.edit_comment(&Ref::Id(first), &uid(mine), Some("Back"), None, None).is_err(), "a comment in the trash is not changed");
        std::fs::remove_dir_all(&dir).ok();
    }
}
