//! Core business logic -- every command the CLI exposes, ported from
//! `taskbook.js`'s `Taskbook` class. Named after the project rather than
//! the class it replaces, since there's no reason for the one file
//! everything else in this rewrite revolves around to still carry the old
//! name.
//!
//! Every public method returns `Result<Outcome, EkkoError>` instead of
//! calling a renderer and possibly `process::exit()` partway through, the
//! way the JS version did. Two things fall out of that for free: the
//! storage lock (acquired as the very first line of every mutating
//! method) is released by normal Rust unwinding no matter which `?`
//! bails out first, and `main.rs` -- the one place that turns a `Result`
//! into either pretty or `--json` output -- doesn't need this module to
//! know which of those two output modes is active at all.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::config;
use crate::directory::DirectoryError;
use crate::item::{tally, Change, Item, State};
use crate::render::{CalendarMonth, Inversion, RoadmapStep, ProjectSummary, Renderer, Stats, TRASH_DAYS};
use crate::storage::{ItemMap, Storage, StorageError};

#[derive(Debug)]
pub enum EkkoError {
    MissingId,
    InvalidId(String),
    MissingDesc,
    InvalidIdsNumber,
    InvalidPriority,
    MissingBoards,
    UnknownListTerm(String),
    InvalidDueDate(String),
    MissingState,
    UnknownState(String),
    BlockingCycle(u32, u32),
    /// Tasks a command would have completed while something they are
    /// blocked by is still open, each with those blockers.
    Blocked(Vec<(u32, Vec<u32>)>),
    ForceWithoutCompleting,
    /// Tasks a command would have reopened while completed work is blocked
    /// by them, each with those completed dependents.
    CompletedDependents(Vec<(u32, Vec<u32>)>),
    /// Completed tasks a --blocked-by would have left waiting on open work,
    /// each with those open blockers.
    AlreadyDone(Vec<(u32, Vec<u32>)>),
    /// A structured request that could not be made sense of -- a missing
    /// field, a value out of range -- said in the message.
    InvalidInput(String),
    /// An edit made against a version of the item that is no longer current.
    Stale { id: u32, current: i64 },
    /// A replacement whose text did not occur exactly once in the item.
    EditMatch { id: u32, found: usize },
    PhaseOrder(Inversion),
    AttachNotANote(u32),
    AttachTargetNotATask(u32),
    AttachTargetHasNoUid(u32),
    RenamedFlag { old: &'static str, new: &'static str },
    InvalidCustomAppDir(String),
    MissingEkkoDirFlagValue,
    LockTimeout(String),
    Storage(StorageError),
    Directory(DirectoryError),
    Config(config::ConfigError),
    Clipboard(String),
}

impl EkkoError {
    /// Stable machine-readable code for `--json` error responses --
    /// deliberately not derived from the `Display` message, so a caller
    /// can branch on it without the message text being load-bearing.
    pub fn code(&self) -> &'static str {
        match self {
            EkkoError::MissingId => "MISSING_ID",
            EkkoError::InvalidId(_) => "INVALID_ID",
            EkkoError::MissingDesc => "MISSING_DESC",
            EkkoError::InvalidIdsNumber => "INVALID_IDS_NUMBER",
            EkkoError::InvalidPriority => "INVALID_PRIORITY",
            EkkoError::MissingBoards => "MISSING_BOARDS",
            EkkoError::UnknownListTerm(_) => "UNKNOWN_LIST_TERM",
            EkkoError::InvalidDueDate(_) => "INVALID_DUE_DATE",
            EkkoError::MissingState => "MISSING_STATE",
            EkkoError::UnknownState(_) => "UNKNOWN_STATE",
            EkkoError::BlockingCycle(_, _) => "BLOCKING_CYCLE",
            EkkoError::Blocked(_) => "BLOCKED",
            EkkoError::ForceWithoutCompleting => "FORCE_WITHOUT_COMPLETING",
            EkkoError::CompletedDependents(_) => "COMPLETED_DEPENDENTS",
            EkkoError::AlreadyDone(_) => "ALREADY_DONE",
            EkkoError::InvalidInput(_) => "INVALID_INPUT",
            EkkoError::Stale { .. } => "STALE",
            EkkoError::EditMatch { .. } => "EDIT_MATCH",
            EkkoError::PhaseOrder(_) => "PHASE_ORDER",
            EkkoError::AttachNotANote(_) => "ATTACH_NOT_A_NOTE",
            EkkoError::AttachTargetNotATask(_) => "ATTACH_TARGET_NOT_A_TASK",
            EkkoError::AttachTargetHasNoUid(_) => "ATTACH_TARGET_HAS_NO_UID",
            EkkoError::RenamedFlag { .. } => "RENAMED_FLAG",
            EkkoError::InvalidCustomAppDir(_) => "INVALID_CUSTOM_APP_DIR",
            EkkoError::MissingEkkoDirFlagValue => "MISSING_EKKO_DIR_FLAG_VALUE",
            EkkoError::LockTimeout(_) => "LOCK_TIMEOUT",
            EkkoError::Storage(_) => "STORAGE_ERROR",
            EkkoError::Directory(_) => "DIRECTORY_ERROR",
            EkkoError::Config(_) => "CONFIG_ERROR",
            EkkoError::Clipboard(_) => "CLIPBOARD_ERROR",
        }
    }

    /// Pretty-prints this error. Most variants have a bespoke renderer
    /// method (matching the JS version's messages exactly); the two wrapped
    /// error types are unexpected-enough failure modes (disk full,
    /// permission denied, a corrupt JSON file) that a generic message with
    /// the underlying cause is more useful than trying to give each one
    /// its own copy.
    pub fn render(&self, out: &mut Renderer) {
        match self {
            EkkoError::MissingId => out.missing_id(),
            EkkoError::InvalidId(id) => out.invalid_id(id),
            EkkoError::MissingDesc => out.missing_desc(),
            EkkoError::InvalidIdsNumber => out.invalid_ids_number(),
            EkkoError::InvalidPriority => out.invalid_priority(),
            EkkoError::MissingBoards => out.missing_boards(),
            EkkoError::UnknownListTerm(_) => out.generic_error(&self.to_string()),
            EkkoError::InvalidDueDate(_) => out.generic_error(&self.to_string()),
            EkkoError::MissingState => out.generic_error(&self.to_string()),
            EkkoError::UnknownState(_) => out.generic_error(&self.to_string()),
            EkkoError::BlockingCycle(_, _)
            | EkkoError::Blocked(_)
            | EkkoError::ForceWithoutCompleting
            | EkkoError::CompletedDependents(_)
            | EkkoError::AlreadyDone(_)
            | EkkoError::InvalidInput(_)
            | EkkoError::Stale { .. }
            | EkkoError::EditMatch { .. }
            | EkkoError::PhaseOrder(_)
            | EkkoError::AttachNotANote(_)
            | EkkoError::AttachTargetNotATask(_)
            | EkkoError::AttachTargetHasNoUid(_)
            | EkkoError::RenamedFlag { .. } => out.generic_error(&self.to_string()),
            EkkoError::InvalidCustomAppDir(path) => out.invalid_custom_app_dir(path),
            EkkoError::MissingEkkoDirFlagValue => out.missing_ekko_dir_flag_value(),
            EkkoError::LockTimeout(path) => out.lock_timeout(path),
            EkkoError::Storage(e) => out.generic_error(&e.to_string()),
            EkkoError::Directory(e) => out.generic_error(&e.to_string()),
            EkkoError::Config(e) => out.generic_error(&e.to_string()),
            EkkoError::Clipboard(message) => out.generic_error(message),
        }
    }
}

impl std::fmt::Display for EkkoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EkkoError::MissingId => write!(f, "No id was given as input"),
            EkkoError::InvalidId(id) => write!(f, "Unable to find item with id: {id}"),
            EkkoError::MissingDesc => write!(f, "No description was given as input"),
            EkkoError::InvalidIdsNumber => write!(f, "More than one ids were given as input"),
            EkkoError::InvalidPriority => write!(f, "Priority can only be 1, 2 or 3"),
            EkkoError::MissingBoards => write!(f, "No boards were given as input"),
            EkkoError::UnknownListTerm(term) => {
                write!(f, "Unknown board or attribute: {term}")
            }
            EkkoError::InvalidDueDate(token) => {
                write!(f, "Due date must look like d:YYYY-MM-DD, got: {token}")
            }
            EkkoError::MissingState => write!(f, "No state was given as input"),
            EkkoError::BlockingCycle(waiter, blocker) => write!(
                f,
                "Item {waiter} cannot be blocked by {blocker}: {blocker} is already blocked by {waiter}"
            ),
            EkkoError::Blocked(blocked) => match blocked.as_slice() {
                [(id, blockers)] => write!(
                    f,
                    "Cannot complete task {id}: blocked by {} (open). Finish or cancel {}, clear the dependency with --blocked-by @{id}, or use --force",
                    join_ids(blockers),
                    if blockers.len() == 1 { "it" } else { "them" },
                ),
                _ => write!(
                    f,
                    "Cannot complete tasks {}: {} (open). Finish or cancel the blockers, clear a wrong dependency with --blocked-by, or use --force",
                    join_ids(&blocked.iter().map(|(id, _)| *id).collect::<Vec<_>>()),
                    blocked
                        .iter()
                        .map(|(id, blockers)| format!("{id} is blocked by {}", join_ids(blockers)))
                        .collect::<Vec<_>>()
                        .join("; "),
                ),
            },
            EkkoError::ForceWithoutCompleting => write!(
                f,
                "--force only applies to --check and --set, where it completes a task that is still blocked or reopens one that completed work depends on"
            ),
            EkkoError::CompletedDependents(found) => match found.as_slice() {
                [(id, dependents)] => write!(
                    f,
                    "Cannot reopen task {id}: completed {} {} blocked by it. Reopen {} first, clear the dependency with --blocked-by, or use --force with --check or --set",
                    if dependents.len() == 1 { format!("task {}", join_ids(dependents)) } else { format!("tasks {}", join_ids(dependents)) },
                    if dependents.len() == 1 { "is" } else { "are" },
                    if dependents.len() == 1 { "it" } else { "them" },
                ),
                _ => write!(
                    f,
                    "Cannot reopen tasks {}: {}. Reopen those first, clear the dependencies with --blocked-by, or use --force with --check or --set",
                    join_ids(&found.iter().map(|(id, _)| *id).collect::<Vec<_>>()),
                    found
                        .iter()
                        .map(|(id, dependents)| format!("completed {} blocked by {id}", join_ids(dependents)))
                        .collect::<Vec<_>>()
                        .join("; "),
                ),
            },
            EkkoError::AlreadyDone(found) => match found.as_slice() {
                [(id, blockers)] => write!(
                    f,
                    "Task {id} is already done, so it cannot be blocked by {} (open): completed work cannot wait on open work. Reopen {id} first, or finish or cancel {}",
                    join_ids(blockers),
                    if blockers.len() == 1 { "it" } else { "them" },
                ),
                _ => write!(
                    f,
                    "Tasks {} are already done, so they cannot be blocked by open work: {}. Reopen them first, or finish or cancel the blockers",
                    join_ids(&found.iter().map(|(id, _)| *id).collect::<Vec<_>>()),
                    found
                        .iter()
                        .map(|(id, blockers)| format!("{id} by {}", join_ids(blockers)))
                        .collect::<Vec<_>>()
                        .join("; "),
                ),
            },
            EkkoError::InvalidInput(message) => write!(f, "{message}"),
            EkkoError::Stale { id, current } => write!(
                f,
                "Item {id} changed since it was read (its updatedAt is now {current}), so the edit was not made. Read it again and redo the edit against what it says now"
            ),
            EkkoError::EditMatch { id, found: 0 } => write!(
                f,
                "Item {id} does not contain that text, so nothing was replaced. Read it again and quote the text exactly"
            ),
            EkkoError::EditMatch { id, found } => write!(
                f,
                "That text occurs {found} times in item {id}, so which one to replace is ambiguous and nothing was replaced. Quote enough of the surrounding text to make it unique"
            ),
            EkkoError::PhaseOrder(inversion) => write!(
                f,
                "Task {} is in phase {} and cannot be blocked by {} in {}, which comes after it: a phase cannot wait on a later one. Reorder the phases with --phases if the order is what is wrong",
                inversion.blocked, inversion.blocked_phase, inversion.blocker, inversion.blocker_phase
            ),
            EkkoError::AttachNotANote(id) => write!(
                f,
                "Only a note can be attached, and {id} is a task. A task under a task would be a subtask, which is a different thing"
            ),
            EkkoError::AttachTargetNotATask(id) => write!(
                f,
                "A note is attached to a task, and {id} is a note. Attaching notes to notes would allow chains, and so cycles"
            ),
            EkkoError::AttachTargetHasNoUid(id) => write!(
                f,
                "Item {id} predates uids, so nothing can point at it reliably. Recreate it to give it one"
            ),
            EkkoError::RenamedFlag { old, new } => write!(f, "{old} was renamed to {new}"),
            EkkoError::UnknownState(term) => {
                write!(f, "Unknown state: {term}. Expected one of: done, undone, progress, paused, cancelled, unstarted, starred, unstarred")
            }
            EkkoError::InvalidCustomAppDir(path) => {
                write!(f, "Custom app directory was not found on your system: {path}")
            }
            EkkoError::MissingEkkoDirFlagValue => {
                write!(f, "Please provide a value for --ekko-dir or remove the flag.")
            }
            EkkoError::LockTimeout(path) => write!(f, "Timed out waiting for the ekko storage lock: {path}"),
            EkkoError::Storage(e) => write!(f, "{e}"),
            EkkoError::Directory(e) => write!(f, "{e}"),
            EkkoError::Config(e) => write!(f, "{e}"),
            EkkoError::Clipboard(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for EkkoError {}

impl From<StorageError> for EkkoError {
    fn from(error: StorageError) -> Self {
        match error {
            StorageError::LockTimeout(path) => EkkoError::LockTimeout(path.display().to_string()),
            other => EkkoError::Storage(other),
        }
    }
}

impl From<DirectoryError> for EkkoError {
    fn from(error: DirectoryError) -> Self {
        match error {
            DirectoryError::MissingEkkoDirFlagValue => EkkoError::MissingEkkoDirFlagValue,
            DirectoryError::InvalidCustomAppDir(path) => EkkoError::InvalidCustomAppDir(path),
            other => EkkoError::Directory(other),
        }
    }
}

impl From<config::ConfigError> for EkkoError {
    fn from(error: config::ConfigError) -> Self {
        EkkoError::Config(error)
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteResult {
    pub storage_id: u32,
    pub archive_id: u32,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreResult {
    pub archive_id: u32,
    pub storage_id: u32,
}

/// What a command actually did -- rich enough for `--json` to serialize
/// directly and for the pretty path to hand straight to a `Renderer`
/// method. `Board`-shaped and `Date`-shaped groupings are kept as distinct
/// variants (rather than one generic "Groups" case) purely so `--json`
/// output can name the field `boards` or `dates` correctly later; this
/// module doesn't otherwise treat them differently.
#[derive(Debug)]
pub enum Outcome {
    Task(Item),
    Note(Item),
    /// `overridden` is what `--force` pushed past: each task completed while
    /// still blocked, with the blockers that were open. Empty whenever
    /// nothing needed forcing, which is nearly always.
    ///
    /// `reopened` is the other half: each task reopened while completed work
    /// was blocked by it, with those completed dependents.
    Check {
        checked: Vec<u32>,
        unchecked: Vec<u32>,
        overridden: Vec<(u32, Vec<u32>)>,
        reopened: Vec<(u32, Vec<u32>)>,
    },
    Begin { started: Vec<u32>, paused: Vec<u32> },
    Star { starred: Vec<u32>, unstarred: Vec<u32> },
    /// Idempotent form of Check/Begin/Star: the states asked for, not
    /// the flips performed.
    Set {
        ids: Vec<u32>,
        states: Vec<String>,
        overridden: Vec<(u32, Vec<u32>)>,
        reopened: Vec<(u32, Vec<u32>)>,
    },
    Delete(Vec<DeleteResult>),
    Restore(Vec<RestoreResult>),
    Edit(Item),
    Move(Item),
    Priority(Item),
    Copy { ids: Vec<u32>, descriptions: Vec<String> },
    Board(Vec<(String, Vec<Item>)>),
    Timeline(Vec<(String, Vec<Item>)>),
    Archive(Vec<(String, Vec<Item>)>),
    Find(Vec<(String, Vec<Item>)>),
    List(Vec<(String, Vec<Item>)>),
    Projects(Vec<ProjectSummary>),
    Destroyed { name: String, tasks: u32, notes: u32, trash: std::path::PathBuf },
    Init(Box<crate::project::Initialized>),
    Phases(Vec<String>),
    Blocked { item: Item, blockers: Vec<u32> },
    Attached { item: Item, target: Option<u32> },
    Calendar(CalendarMonth),
    Stashed { ids: Vec<u32>, away: bool },
    Trashed { ids: Vec<u32>, away: bool },
    Stash(Vec<(String, Vec<Item>)>),
    Trash(Vec<Item>),
    Roadmap { steps: Vec<RoadmapStep>, rootless: u32, inversions: Vec<Inversion> },
    Stats(Stats),
    /// The agent views -- see `agent`. Boxed because they are much larger
    /// than every other outcome and would otherwise size the whole enum.
    Prime(Box<crate::agent::Prime>),
    /// What the SessionStart hook puts in context, already worded.
    Hook(String),
    Next(Vec<crate::agent::Entry>),
    Context(Box<crate::agent::Context>),
}


impl Outcome {
    /// Also doubles as the `--json` response's `"command"` field -- these
    /// variants exist one-to-one with CLI commands specifically so this
    /// never needs a separate name passed in alongside the data.
    pub fn command_name(&self) -> &'static str {
        match self {
            Outcome::Task(_) => "task",
            Outcome::Note(_) => "note",
            Outcome::Set { .. } => "set",
            Outcome::Check { .. } => "check",
            Outcome::Begin { .. } => "begin",
            Outcome::Star { .. } => "star",
            Outcome::Delete(_) => "delete",
            Outcome::Restore(_) => "restore",
            Outcome::Edit(_) => "edit",
            Outcome::Move(_) => "move",
            Outcome::Priority(_) => "priority",
            Outcome::Copy { .. } => "copy",
            Outcome::Board(_) => "board",
            Outcome::Timeline(_) => "timeline",
            Outcome::Archive(_) => "archive",
            Outcome::Find(_) => "find",
            Outcome::List(_) => "list",
            Outcome::Projects(_) => "projects",
            Outcome::Destroyed { .. } => "destroy",
            Outcome::Init(_) => "init",
            Outcome::Phases(_) => "phases",
            Outcome::Blocked { .. } => "blocked",
            Outcome::Attached { .. } => "attached",
            Outcome::Calendar(_) => "calendar",
            Outcome::Stashed { away, .. } => if *away { "stash" } else { "unstash" },
            Outcome::Trashed { away, .. } => if *away { "trash" } else { "untrash" },
            Outcome::Stash(_) => "stash",
            Outcome::Trash(_) => "trash",
            Outcome::Roadmap { .. } => "roadmap",
            Outcome::Stats(_) => "stats",
            Outcome::Prime(_) | Outcome::Hook(_) => "prime",
            Outcome::Next(_) => "next",
            Outcome::Context(_) => "context",
        }
    }

    pub fn render(&self, out: &mut Renderer) {
        match self {
            Outcome::Task(item) | Outcome::Note(item) => out.success_create(item),
            Outcome::Check { checked, unchecked, overridden, reopened } => {
                out.mark_complete_overriding(checked, overridden);
                out.mark_incomplete(unchecked, reopened);
            }
            Outcome::Set { ids, states, overridden, reopened } => {
                // Reuses the toggles' own messages where one exists, rather
                // than inventing a parallel vocabulary: the same transition
                // should read the same way however it was requested. The two
                // states with no toggle behind them get their own verbs.
                //
                // Every canonical state `canonical_state` can return must
                // appear here. The catch-all below cannot be removed (the
                // match is on `&str`), so it will not fail a build -- it
                // will silently print nothing, which is how `cancelled` and
                // `unstarted` shipped mute. Add the arm when adding a state.
                for state in states {
                    match state.as_str() {
                        "done" => out.mark_complete_overriding(ids, overridden),
                        "undone" => out.mark_incomplete(ids, reopened),
                        "progress" => out.mark_started(ids, reopened),
                        "paused" => out.mark_paused(ids, reopened),
                        "cancelled" => out.mark_cancelled(ids),
                        "unstarted" => out.mark_reset(ids, reopened),
                        "starred" => out.mark_starred(ids),
                        "unstarred" => out.mark_unstarred(ids),
                        _ => {}
                    }
                }
            }
            Outcome::Begin { started, paused } => {
                // --begin never forces, so it never reopens over anything.
                out.mark_started(started, &[]);
                out.mark_paused(paused, &[]);
            }
            Outcome::Star { starred, unstarred } => {
                out.mark_starred(starred);
                out.mark_unstarred(unstarred);
            }
            Outcome::Delete(items) => {
                let ids: Vec<u32> = items.iter().map(|r| r.storage_id).collect();
                out.success_delete(&ids);
            }
            Outcome::Restore(items) => {
                let ids: Vec<u32> = items.iter().map(|r| r.archive_id).collect();
                out.success_restore(&ids);
            }
            Outcome::Edit(item) => out.success_edit(item.id),
            Outcome::Move(item) => out.success_move(item.id, &item.boards),
            Outcome::Priority(item) => out.success_priority(item.id, item.priority.unwrap_or(1)),
            Outcome::Copy { ids, .. } => out.success_copy_to_clipboard(ids),
            Outcome::Board(groups) | Outcome::Find(groups) | Outcome::List(groups) => out.display_by_board(groups),
            Outcome::Timeline(groups) | Outcome::Archive(groups) => out.display_by_date(groups),
            Outcome::Projects(projects) => out.display_projects(projects),
            Outcome::Init(init) => out.success_init(init),
            Outcome::Destroyed { name, tasks, notes, trash } => {
                out.success_destroy(name, *tasks, *notes, trash)
            }
            Outcome::Phases(names) => out.display_phases(names),
            Outcome::Blocked { item, blockers } => out.success_blocked(item.id, blockers),
            Outcome::Attached { item, target } => out.success_attached(item.id, *target),
            Outcome::Calendar(month) => out.display_calendar(month),
            Outcome::Stashed { ids, away } => out.success_stashed(ids, *away),
            Outcome::Trashed { ids, away } => out.success_trashed(ids, *away),
            Outcome::Stash(groups) => out.display_stash(groups),
            Outcome::Trash(items) => out.display_trash(items),
            Outcome::Roadmap { steps, rootless, inversions } => {
                out.display_roadmap(steps, *rootless);
                out.display_inversions(inversions);
            }
            Outcome::Stats(stats) => out.display_stats(stats),
            Outcome::Prime(prime) => out.raw(&prime.text()),
            Outcome::Hook(text) => out.raw(text),
            Outcome::Next(entries) => out.raw(&crate::agent::list_text(entries, "Nothing is in progress or ready.")),
            Outcome::Context(context) => out.raw(&context.text()),
        }
    }
}

pub struct Ekko {
    pub(crate) storage: Storage,
}

impl Ekko {
    pub fn new(storage: Storage) -> Self {
        Ekko { storage }
    }

    /// Opens the board in `dir` -- wherever `directory::locate` said this
    /// invocation's board lives.
    pub fn at(dir: &std::path::Path) -> Result<Self, EkkoError> {
        Ok(Self::new(Storage::new(dir)?))
    }

    // ---- id / option parsing -------------------------------------------

    /// The next display id in storage: past every id `data` holds and every id
    /// storage has ever held, so a number someone kept never comes to name a
    /// different item once its own has left for the archive or out of the trash.
    pub(crate) fn generate_id(&self, data: &ItemMap) -> u32 {
        let highest = self.storage.get_counters().map(|counters| counters.highest_id).unwrap_or(0);
        data.keys().max().copied().unwrap_or(0).max(highest) + 1
    }

    /// Resolves what a caller typed into display ids, accepting either.
    ///
    /// A display id used to be recycled -- `max + 1` handed a deleted item's
    /// number back out -- and a restore still renumbers, so anything holding
    /// a reference across time is told to hold the `uid` instead. That advice
    /// was unfollowable until this accepted one: every mutation took display
    /// ids only, so an agent could carry a uid and then had nothing to do
    /// with it but re-read the board to translate.
    ///
    /// The two never collide. A uid is `{nanos:x}-{pid:x}`, so it always
    /// carries a hyphen and never parses as a `u32`; anything numeric is a
    /// display id and anything else is looked up as a uid. Both miss the
    /// same way, as `INVALID_ID`, because a caller branching on the code
    /// should not have to care which spelling it used.
    pub(crate) fn validate_ids(&self, raw_ids: &[String], existing: &ItemMap) -> Result<Vec<u32>, EkkoError> {
        if raw_ids.is_empty() {
            return Err(EkkoError::MissingId);
        }

        let mut ids = Vec::new();
        for raw in raw_ids {
            let id = match raw.parse::<u32>() {
                Ok(id) if existing.contains_key(&id) => id,
                Ok(_) => return Err(EkkoError::InvalidId(raw.clone())),
                Err(_) => {
                    find_by_uid(existing, raw).ok_or_else(|| EkkoError::InvalidId(raw.clone()))?
                }
            };
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        Ok(ids)
    }

    fn extract_single_id_target<'a>(&self, input: &'a [String]) -> Result<(String, Vec<&'a String>), EkkoError> {
        let targets: Vec<&String> = input.iter().filter(|x| x.starts_with('@')).collect();
        if targets.is_empty() {
            return Err(EkkoError::MissingId);
        }
        if targets.len() > 1 {
            return Err(EkkoError::InvalidIdsNumber);
        }

        let target = targets[0];
        let id = target.strip_prefix('@').unwrap_or(target).to_string();
        let rest: Vec<&String> = input.iter().filter(|x| *x != target).collect();
        Ok((id, rest))
    }

    fn parse_create_options(
        &self,
        input: &[String],
    ) -> Result<(Vec<String>, String, u8, Option<String>), EkkoError> {
        if input.is_empty() {
            return Err(EkkoError::MissingDesc);
        }

        let priority = get_priority(input);

        let mut due_date = None;
        for token in input.iter().filter(|t| is_due_opt(t)) {
            match parse_due_date(token) {
                Some(date) => due_date = Some(date),
                None => return Err(EkkoError::InvalidDueDate(token.clone())),
            }
        }
        let mut boards = Vec::new();
        let mut words = Vec::new();
        for token in input {
            if is_priority_opt(token) || is_due_opt(token) {
                continue;
            }
            if token.starts_with('@') && token.len() > 1 {
                boards.push(token.clone());
            } else {
                words.push(token.clone());
            }
        }

        let description = words.join(" ");
        if description.is_empty() {
            return Err(EkkoError::MissingDesc);
        }
        if boards.is_empty() {
            boards.push("My Board".to_string());
        }

        Ok((boards, description, priority, due_date))
    }

    // ---- grouping / stats / search -------------------------------------

    fn get_boards(&self, data: &ItemMap) -> Vec<String> {
        let mut boards = vec!["My Board".to_string()];
        for item in data.values() {
            for board in &item.boards {
                if !boards.contains(board) {
                    boards.push(board.clone());
                }
            }
        }
        boards
    }

    fn get_dates(&self, data: &ItemMap) -> Vec<String> {
        let mut dates = Vec::new();
        for item in data.values() {
            if !dates.contains(&item.date) {
                dates.push(item.date.clone());
            }
        }
        dates
    }

    fn group_by_board(&self, data: &ItemMap, boards: &[String]) -> Vec<(String, Vec<Item>)> {
        let boards: Vec<String> =
            if boards.is_empty() { self.get_boards(data) } else { boards.to_vec() };
        let mut grouped: Vec<(String, Vec<Item>)> = Vec::new();
        for item in data.values() {
            for board in &boards {
                if item.boards.contains(board) {
                    match grouped.iter_mut().find(|(b, _)| b == board) {
                        Some(entry) => entry.1.push(item.clone()),
                        None => grouped.push((board.clone(), vec![item.clone()])),
                    }
                }
            }
        }
        for (_, items) in &mut grouped {
            reorder_attached(items);
        }
        grouped
    }

    fn group_by_date(&self, data: &ItemMap, dates: &[String]) -> Vec<(String, Vec<Item>)> {
        let mut grouped: Vec<(String, Vec<Item>)> = Vec::new();
        for item in data.values() {
            for date in dates {
                if &item.date == date {
                    match grouped.iter_mut().find(|(d, _)| d == date) {
                        Some(entry) => entry.1.push(item.clone()),
                        None => grouped.push((date.clone(), vec![item.clone()])),
                    }
                }
            }
        }
        grouped
    }

    /// The stats line, counting every item exactly once.
    ///
    /// Stashed and trashed are counted *instead of* whatever they were,
    /// not as well as: a stashed done task shows up under `in-stash` and
    /// not under `done`. Disjoint counts are what let the line still sum
    /// to the board, and they answer the question the line is for -- what
    /// is in front of me right now -- rather than what exists.
    ///
    /// The item keeps its real state underneath, so unstashing puts it
    /// back where it belongs. Only the counting hides it.
    pub(crate) fn compute_stats(&self, data: &ItemMap) -> Stats {
        let (mut complete, mut in_progress, mut paused, mut cancelled, mut pending, mut notes) =
            (0u32, 0u32, 0u32, 0u32, 0u32, 0u32);
        let (mut stashed, mut trashed) = (0u32, 0u32);
        for item in data.values() {
            if item.trashed.is_some() {
                trashed += 1;
                continue;
            }
            if item.stashed.is_some() {
                stashed += 1;
                continue;
            }
            match State::of(item) {
                Some(State::Cancelled) => cancelled += 1,
                Some(State::Done) => complete += 1,
                Some(State::Progress) => in_progress += 1,
                // Counted apart from pending on purpose: lumping them back
                // together is exactly the conflation this state exists to
                // undo, and "0 pending" while two tasks sit half-done was the
                // original lie.
                Some(State::Paused) => paused += 1,
                Some(State::Pending) => pending += 1,
                None => notes += 1,
            }
        }
        // `cancelled` is absent from the total on purpose: counting it would
        // mean a board can never reach 100% once anything is dropped, which
        // reads as unfinished work rather than as work that went away.
        let total = complete + pending + in_progress + paused;
        let percent = (complete * 100).checked_div(total).unwrap_or(0);
        Stats { percent, complete, in_progress, paused, cancelled, pending, notes, stashed, trashed }
    }

    /// `all` is the whole of storage, stashed and trashed included, and is
    /// only consulted to resolve blockers -- see `unmet_blockers` for why a
    /// blocker has to be judged against everything rather than the view.
    fn filter_by_attributes(&self, attributes: &[String], mut data: ItemMap, all: &ItemMap) -> ItemMap {
        if data.is_empty() {
            return data;
        }
        for attribute in attributes {
            match attribute.as_str() {
                "star" | "starred" => data.retain(|_, item| item.is_starred),
                // Every state term reads through `State::of`, the same function
                // the board and the stats line use, so no combination of flags
                // can be done here and cancelled there.
                "done" | "checked" | "complete" => {
                    data.retain(|_, item| State::of(item) == Some(State::Done));
                }
                "progress" | "started" | "begun" => {
                    data.retain(|_, item| State::of(item) == Some(State::Progress));
                }
                "paused" => data.retain(|_, item| State::of(item) == Some(State::Paused)),
                // Matches the JS version exactly: "pending" only checks
                // `!isComplete`, so an in-progress task passes this filter
                // too. Not something this port introduced or should
                // silently change.
                "pending" | "unchecked" | "incomplete" => {
                    // Cancelled excluded, unlike the JS version, which had no such
                    // state to exclude. A dropped task is not waiting to be done,
                    // and listing it as pending is the same conflation the
                    // paused state was added to undo.
                    data.retain(|_, item| State::of(item).is_some_and(State::is_open));
                }
                "todo" | "task" | "tasks" => data.retain(|_, item| item.is_task),
                "note" | "notes" => data.retain(|_, item| !item.is_task),
                // Resolved against `all`, not the retained subset: a blocker
                // can sit outside whatever else is being filtered, and outside
                // the view altogether. Judging it against the view is how a
                // stashed blocker stopped holding here while the board still
                // drew it -- two surfaces answering one question differently.
                "ready" => {
                    let index = uid_index(all);
                    data.retain(|_, item| {
                        State::of(item).is_some_and(State::is_open)
                            && Self::unmet_blockers_indexed(&index, all, item).is_empty()
                    });
                }
                "blocked" => {
                    let index = uid_index(all);
                    data.retain(|_, item| !Self::unmet_blockers_indexed(&index, all, item).is_empty());
                }
                "cancelled" | "canceled" => {
                    data.retain(|_, item| State::of(item) == Some(State::Cancelled));
                }
                "due" => data.retain(|_, item| item.due_date.is_some()),
                // Only tasks that are still open: a finished task is not
                // late, however long its deadline has been past.
                "overdue" => {
                    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
                    // Open, not merely unfinished: a cancelled task is not late
                    // either, and used to be listed here as if it were.
                    data.retain(|_, item| {
                        State::of(item).is_some_and(State::is_open)
                            && item.due_date.as_deref().is_some_and(|d| d < today.as_str())
                    });
                }
                _ => {}
            }
        }
        data
    }

    // ---- archive/restore plumbing ---------------------------------------

    fn move_to_archive(&self, mut item: Item, archive: &mut ItemMap) -> u32 {
        // The archive numbers its own items; storage's high-water mark is not its.
        let archive_id = archive.keys().max().copied().unwrap_or(0) + 1;
        item.id = archive_id;
        archive.insert(archive_id, item);
        archive_id
    }

    fn move_to_storage(&self, mut item: Item, data: &mut ItemMap) -> u32 {
        let storage_id = self.generate_id(data);
        item.id = storage_id;
        data.insert(storage_id, item);
        storage_id
    }

    // ---- public: mutating commands ---------------------------------------

    /// Writes `data` back, stamping `updated_at` and the next board revision
    /// on whatever actually changed, and keeping the board's counters and
    /// journal. Returns the tasks the write set free and those it left waiting.
    ///
    /// Works by diffing against what is currently on disk rather than
    /// asking each command to remember which ids it touched. That is
    /// deliberate: there are eleven mutating commands, a twelfth is always
    /// possible, and a hand-stamped field is one someone eventually forgets
    /// to stamp. Comparing catches every field, including ones added later.
    ///
    /// Re-reading here is safe because every caller already holds the lock.
    pub(crate) fn save_touching(&self, data: &mut ItemMap) -> Result<(Vec<u32>, Vec<u32>), EkkoError> {
        let before = self.storage.get()?;
        self.save_against(&before, data)
    }

    /// `save_touching` against the board as the caller read it, under the
    /// lock it still holds: a structured write already has that copy, and
    /// reading storage.json a second time only parses the whole board again.
    pub(crate) fn save_against(&self, before: &ItemMap, data: &mut ItemMap) -> Result<(Vec<u32>, Vec<u32>), EkkoError> {
        let now = chrono::Local::now().timestamp_millis();

        // The trash empties here, on the way past, and only here.
        //
        // Ekko has no daemon, so expiry has to happen on some invocation --
        // and it must not be a read. `list_projects` already had to avoid
        // creating directories for the same reason: a command that only
        // looks at the board cannot be allowed to change it, or `--json`
        // stops being safe to call. Every write already holds the lock, so
        // this rides along on one that was happening anyway.
        data.retain(|_, item| match item.trashed {
            Some(at) => now - at < TRASH_DAYS * 86_400_000,
            None => true,
        });

        let changed: Vec<u32> = data.iter().filter(|(id, item)| before.get(*id) != Some(*item)).map(|(id, _)| *id).collect();
        let gone: Vec<&Item> = before.iter().filter(|(id, _)| !data.contains_key(*id)).map(|(_, item)| item).collect();

        let kept = self.storage.get_counters()?;
        let mut counters = kept;
        counters.highest_id = counters.highest_id.max(before.keys().chain(data.keys()).max().copied().unwrap_or(0));
        // A write that changes nothing leaves the revision where it was, so a
        // retried command does not tell every reader that the board moved.
        if !changed.is_empty() || !gone.is_empty() {
            counters.revision += 1;
        }
        for id in &changed {
            if let Some(item) = data.get_mut(id) {
                item.updated_at = Some(now);
                item.rev = Some(counters.revision);
            }
        }

        // Counters first. A crash between the two files leaves a revision no
        // item carries, which no cursor can miss; the other order would leave
        // items stamped with a revision the next write hands out again.
        if counters != kept {
            self.storage.set_counters(counters)?;
        }
        self.storage.set(data)?;

        // What the write did that the items it left cannot show: work it set
        // free or left waiting, and what it took out of storage. Journaled
        // after storage, so a crash in between loses a line about a write that
        // landed rather than recording one about a write that did not.
        let (released, blocked) = Self::readiness_changes(before, data);
        if !released.is_empty() || !blocked.is_empty() || !gone.is_empty() {
            let named = |ids: &[u32]| -> Vec<serde_json::Value> {
                ids.iter().filter_map(|id| data.get(id)).map(|item| serde_json::json!({"id": item.id, "uid": item.uid})).collect()
            };
            let removed: Vec<serde_json::Value> = gone
                .iter()
                .map(|item| serde_json::json!({"id": item.id, "uid": item.uid, "text": crate::agent::clip(&item.description, 80)}))
                .collect();
            self.storage.append_journal(&serde_json::json!({
                "rev": counters.revision,
                "at": now,
                "released": named(&released),
                "blocked": named(&blocked),
                "removed": removed,
            }))?;
        }
        Ok((released, blocked))
    }

    /// `phase` is the scope the CLI was invoked with. Items created without
    /// one land at the project root, outside the roadmap -- never in a guessed
    /// current phase, because putting work somewhere nobody chose is exactly
    /// the plausible-wrong-answer this codebase keeps refusing.
    pub fn create_task_in(
        &self,
        input: &[String],
        phase: Option<&str>,
    ) -> Result<Outcome, EkkoError> {
        let outcome = self.create_task(input)?;
        self.assign_phase(&outcome, phase)
    }

    pub fn create_note_in(
        &self,
        input: &[String],
        phase: Option<&str>,
    ) -> Result<Outcome, EkkoError> {
        let outcome = self.create_note(input)?;
        self.assign_phase(&outcome, phase)
    }

    fn assign_phase(&self, outcome: &Outcome, phase: Option<&str>) -> Result<Outcome, EkkoError> {
        let (Some(name), Outcome::Task(item) | Outcome::Note(item)) = (phase, outcome) else {
            return Ok(match outcome {
                Outcome::Task(i) => Outcome::Task(i.clone()),
                other => Outcome::Note(match other {
                    Outcome::Note(i) => i.clone(),
                    _ => unreachable!("assign_phase only ever sees a created item"),
                }),
            });
        };

        let _lock = self.storage.acquire_lock()?;
        let mut data = self.storage.get()?;
        let mut updated = item.clone();
        updated.phase = Some(name.to_string());
        data.insert(updated.id, updated.clone());
        self.save_touching(&mut data)?;

        Ok(if updated.is_task { Outcome::Task(updated) } else { Outcome::Note(updated) })
    }

    pub fn create_task(&self, input: &[String]) -> Result<Outcome, EkkoError> {
        let _lock = self.storage.acquire_lock()?;
        let (boards, description, priority, due_date) = self.parse_create_options(input)?;
        let mut data = self.storage.get()?;
        let id = self.generate_id(&data);
        let mut item = Item::new_task(id, description, boards, priority);
        item.due_date = due_date;
        data.insert(id, item.clone());
        self.save_touching(&mut data)?;
        Ok(Outcome::Task(item))
    }

    pub fn create_note(&self, input: &[String]) -> Result<Outcome, EkkoError> {
        let _lock = self.storage.acquire_lock()?;
        // Notes carry no deadline, same as they carry no priority: a `d:`
        // token on a note is parsed (so a malformed one still errors) and
        // then dropped.
        let (boards, description, _priority, _due) = self.parse_create_options(input)?;
        let mut data = self.storage.get()?;
        let id = self.generate_id(&data);
        let item = Item::new_note(id, description, boards);
        data.insert(id, item.clone());
        self.save_touching(&mut data)?;
        Ok(Outcome::Note(item))
    }

    /// Toggles completion. A toggle that would complete a task still
    /// blocked, or reopen one completed work depends on, is refused unless
    /// `force` -- see `refuse_broken_dependencies`.
    pub fn check_tasks(&self, ids: &[String], force: bool) -> Result<Outcome, EkkoError> {
        let _lock = self.storage.acquire_lock()?;
        let mut data = self.storage.get()?;
        let ids = self.validate_ids(ids, &data)?;
        let before = data.clone();
        let (mut checked, mut unchecked) = (Vec::new(), Vec::new());
        for id in ids {
            if let Some(item) = data.get_mut(&id) {
                if let Some(state) = State::of(item) {
                    let next = state.after(Change::ToggleDone);
                    next.write(item);
                    if next == State::Done { checked.push(id) } else { unchecked.push(id) }
                }
            }
        }
        let (overridden, reopened) = Self::refuse_broken_dependencies(&before, &data, force)?;
        self.save_touching(&mut data)?;
        Ok(Outcome::Check { checked, unchecked, overridden, reopened })
    }

    pub fn begin_tasks(&self, ids: &[String]) -> Result<Outcome, EkkoError> {
        let _lock = self.storage.acquire_lock()?;
        let mut data = self.storage.get()?;
        let ids = self.validate_ids(ids, &data)?;
        let before = data.clone();
        let (mut started, mut paused) = (Vec::new(), Vec::new());
        for id in ids {
            if let Some(item) = data.get_mut(&id) {
                if let Some(state) = State::of(item) {
                    let next = state.after(Change::ToggleProgress);
                    next.write(item);
                    if next == State::Progress { started.push(id) } else { paused.push(id) }
                }
            }
        }
        // Starting a done or cancelled task reopens it. No force here:
        // `--force` belongs to --check and --set, and the refusal says so.
        Self::refuse_broken_dependencies(&before, &data, false)?;
        self.save_touching(&mut data)?;
        Ok(Outcome::Begin { started, paused })
    }

    pub fn star_items(&self, ids: &[String]) -> Result<Outcome, EkkoError> {
        let _lock = self.storage.acquire_lock()?;
        let mut data = self.storage.get()?;
        let ids = self.validate_ids(ids, &data)?;
        let (mut starred, mut unstarred) = (Vec::new(), Vec::new());
        for id in ids {
            if let Some(item) = data.get_mut(&id) {
                item.is_starred = !item.is_starred;
                if item.is_starred { starred.push(id) } else { unstarred.push(id) }
            }
        }
        self.save_touching(&mut data)?;
        Ok(Outcome::Star { starred, unstarred })
    }

    /// Removes items -- to the trash, not to the archive.
    ///
    /// It used to archive them, which put a task you finished on purpose
    /// and a task you deleted by mistake in the same place. The archive is
    /// the record of what got done; a mistake in it is noise in the one
    /// history you might trust. The trash is where removals go, and it
    /// expires, which the archive must never do.
    ///
    /// `--clear` still archives, because completed work reaching the end
    /// is exactly what the archive is for.
    pub fn delete_items(&self, ids: &[String]) -> Result<Outcome, EkkoError> {
        self.set_trashed(ids, true)
    }

    /// Assumes the caller already holds the storage lock -- only called
    /// from `delete_items` (which acquires it) and `clear` (same), never
    /// on its own, so acquiring is never attempted twice for one logical
    /// operation.
    fn delete_items_locked(&self, ids: &[String]) -> Result<Outcome, EkkoError> {
        let mut data = self.storage.get()?;
        let ids = self.validate_ids(ids, &data)?;
        let mut archive = self.storage.get_archive()?;

        let mut results = Vec::new();
        for id in ids {
            if let Some(item) = data.remove(&id) {
                let archive_id = self.move_to_archive(item, &mut archive);
                results.push(DeleteResult { storage_id: id, archive_id });
            }
        }

        self.save_touching(&mut data)?;
        self.storage.set_archive(&archive)?;
        Ok(Outcome::Delete(results))
    }

    pub fn restore_items(&self, ids: &[String]) -> Result<Outcome, EkkoError> {
        let _lock = self.storage.acquire_lock()?;
        let mut archive = self.storage.get_archive()?;
        let archive_ids = self.validate_ids(ids, &archive)?;
        let mut data = self.storage.get()?;

        let mut results = Vec::new();
        for archive_id in archive_ids {
            if let Some(item) = archive.remove(&archive_id) {
                let storage_id = self.move_to_storage(item, &mut data);
                results.push(RestoreResult { archive_id, storage_id });
            }
        }

        self.storage.set_archive(&archive)?;
        self.save_touching(&mut data)?;
        Ok(Outcome::Restore(results))
    }

    pub fn edit_description(&self, input: &[String]) -> Result<Outcome, EkkoError> {
        let _lock = self.storage.acquire_lock()?;
        let (id_str, rest) = self.extract_single_id_target(input)?;
        let mut data = self.storage.get()?;
        let id = self.validate_ids(&[id_str], &data)?[0];

        let new_description = rest.into_iter().cloned().collect::<Vec<_>>().join(" ");
        if new_description.is_empty() {
            return Err(EkkoError::MissingDesc);
        }

        data.get_mut(&id).expect("id just validated against data").description = new_description;
        self.save_touching(&mut data)?;
        Ok(Outcome::Edit(data[&id].clone()))
    }

    pub fn move_boards(&self, input: &[String]) -> Result<Outcome, EkkoError> {
        let _lock = self.storage.acquire_lock()?;
        let (id_str, rest) = self.extract_single_id_target(input)?;
        let mut data = self.storage.get()?;
        let id = self.validate_ids(&[id_str], &data)?[0];

        let mut boards: Vec<String> = rest
            .into_iter()
            .map(|x| if x == "myboard" { "My Board".to_string() } else { format!("@{x}") })
            .collect();
        if boards.is_empty() {
            return Err(EkkoError::MissingBoards);
        }
        boards = remove_duplicates(boards);

        data.get_mut(&id).expect("id just validated against data").boards = boards;
        self.save_touching(&mut data)?;
        Ok(Outcome::Move(data[&id].clone()))
    }

    pub fn update_priority(&self, input: &[String]) -> Result<Outcome, EkkoError> {
        let _lock = self.storage.acquire_lock()?;
        let level: u8 = input
            .iter()
            .find(|x| matches!(x.as_str(), "1" | "2" | "3"))
            .ok_or(EkkoError::InvalidPriority)?
            .parse()
            .expect("matched against \"1\"|\"2\"|\"3\" above");

        let (id_str, _rest) = self.extract_single_id_target(input)?;
        let mut data = self.storage.get()?;
        let id = self.validate_ids(&[id_str], &data)?[0];

        data.get_mut(&id).expect("id just validated against data").priority = Some(level);
        self.save_touching(&mut data)?;
        Ok(Outcome::Priority(data[&id].clone()))
    }

    pub fn clear(&self) -> Result<Outcome, EkkoError> {
        let _lock = self.storage.acquire_lock()?;
        let data = self.storage.get()?;
        // Only what is actually on the board. Something already stashed has
        // been put away on purpose, and sweeping it into the archive would
        // both undo that and change its id on the way back -- a bulk
        // command reaching past the board into the things hidden from it is
        // the opposite of what hiding them was for.
        let ids: Vec<String> = data
            .iter()
            .filter(|(_, item)| {
                State::of(item) == Some(State::Done)
                    && item.stashed.is_none()
                    && item.trashed.is_none()
            })
            .map(|(id, _)| id.to_string())
            .collect();
        if ids.is_empty() {
            return Ok(Outcome::Delete(vec![]));
        }
        self.delete_items_locked(&ids)
    }

    /// Clipboard writing is injected rather than this module depending on
    /// a clipboard crate directly -- keeps the platform-specific part
    /// isolated to where the CLI actually wires it up, and this method
    /// (and its data-gathering logic) testable without a real clipboard.
    pub fn copy_to_clipboard<F>(&self, ids: &[String], write_clipboard: F) -> Result<Outcome, EkkoError>
    where
        F: FnOnce(&str) -> Result<(), String>,
    {
        let data = self.storage.get()?;
        let ids = self.validate_ids(ids, &data)?;
        let descriptions: Vec<String> = ids.iter().map(|id| data[id].description.clone()).collect();
        write_clipboard(&descriptions.join("\n")).map_err(EkkoError::Clipboard)?;
        Ok(Outcome::Copy { ids, descriptions })
    }

    // ---- public: read-only commands --------------------------------------

    /// Storage with what has been put away or thrown away taken out.
    ///
    /// Every *view* reads through this; every *mutation* reads raw, so a
    /// stashed item can still be addressed by id -- otherwise unstashing
    /// something would require it to be visible, which is the one thing it
    /// is not.
    fn visible(&self) -> Result<ItemMap, EkkoError> {
        let mut data = self.storage.get()?;
        data.retain(|_, item| item.stashed.is_none() && item.trashed.is_none());
        Ok(data)
    }

    pub fn display_by_board(&self) -> Result<Outcome, EkkoError> {
        let data = self.visible()?;
        let boards = self.get_boards(&data);
        Ok(Outcome::Board(self.group_by_board(&data, &boards)))
    }

    pub fn display_by_date(&self) -> Result<Outcome, EkkoError> {
        let data = self.visible()?;
        let dates = self.get_dates(&data);
        Ok(Outcome::Timeline(self.group_by_date(&data, &dates)))
    }

    pub fn display_archive(&self) -> Result<Outcome, EkkoError> {
        let archive = self.storage.get_archive()?;
        let dates = self.get_dates(&archive);
        Ok(Outcome::Archive(self.group_by_date(&archive, &dates)))
    }

    pub fn display_stats(&self) -> Result<Outcome, EkkoError> {
        let data = self.storage.get()?;
        Ok(Outcome::Stats(self.compute_stats(&data)))
    }

    pub fn find_items(&self, terms: &[String]) -> Result<Outcome, EkkoError> {
        let data = self.visible()?;
        let mut result: ItemMap = BTreeMap::new();
        for (id, item) in &data {
            if has_terms(&item.description, terms) {
                result.insert(*id, item.clone());
            }
        }
        // Board order/precedence comes from the *full* dataset, matching
        // the JS version's `_groupByBoard(result)` -- its `boards`
        // parameter defaults to `this._getBoards()`, evaluated against
        // `this._data` (everything), not the already-filtered `result`.
        let boards = self.get_boards(&data);
        Ok(Outcome::Find(self.group_by_board(&result, &boards)))
    }


    /// Idempotent counterpart to `check_tasks`/`begin_tasks`/`star_items`.
    ///
    /// Those three toggle, which is right for a person at a terminal and
    /// wrong for anything that might retry: run `--check 3` twice after a
    /// timeout and the task ends up unchecked. This takes the states the
    /// item should be *in*, so running it twice is the same as running it
    /// once.
    ///
    /// Ids are marked with `@`, matching `--priority`/`--move`, which
    /// leaves the bare words free to be state names.
    /// Puts items away, or brings them back.
    ///
    /// Hiding, not changing. A stashed done task is still done and comes
    /// back done -- which is why this is its own field and not another
    /// state: a state would overwrite whatever it found, and unstashing
    /// would then have to guess what to restore.
    ///
    /// A `@board` argument stashes what is on that board *now*. It does
    /// not close the board: an item created there tomorrow shows up
    /// normally. Closing a board so later items are born hidden is a
    /// different feature, and it is what item 31 wants for phases.
    pub fn set_stashed(&self, input: &[String], away: bool) -> Result<Outcome, EkkoError> {
        let _lock = self.storage.acquire_lock()?;
        let mut data = self.storage.get()?;

        let ids = self.resolve_targets(input, &data)?;
        let now = chrono::Local::now().timestamp_millis();

        for id in &ids {
            if let Some(item) = data.get_mut(id) {
                item.stashed = if away { Some(now) } else { None };
            }
        }

        self.save_touching(&mut data)?;
        Ok(Outcome::Stashed { ids, away })
    }

    /// Same shape for the trash, and the same reasoning about not
    /// overwriting state. The difference is that this one expires.
    pub fn set_trashed(&self, input: &[String], away: bool) -> Result<Outcome, EkkoError> {
        let _lock = self.storage.acquire_lock()?;
        let mut data = self.storage.get()?;

        let ids = self.resolve_targets(input, &data)?;
        let now = chrono::Local::now().timestamp_millis();

        for id in &ids {
            if let Some(item) = data.get_mut(id) {
                item.trashed = if away { Some(now) } else { None };
            }
        }

        self.save_touching(&mut data)?;
        Ok(Outcome::Trashed { ids, away })
    }

    /// Ids, or every item on a `@board`.
    ///
    /// Both spellings exist because stashing one note and stashing a
    /// finished area are the same intention at different sizes, and
    /// making the second one mean typing five ids would guarantee nobody
    /// does it.
    fn resolve_targets(&self, input: &[String], data: &ItemMap) -> Result<Vec<u32>, EkkoError> {
        if input.is_empty() {
            return Err(EkkoError::MissingId);
        }

        let mut ids = Vec::new();
        let mut raw = Vec::new();
        let boards = self.get_boards(data);

        for token in input {
            let as_board =
                if token.starts_with('@') { token.clone() } else { format!("@{token}") };
            if boards.contains(&as_board) {
                for (id, item) in data.iter() {
                    if item.boards.contains(&as_board) && !ids.contains(id) {
                        ids.push(*id);
                    }
                }
            } else {
                raw.push(token.trim_start_matches('@').to_string());
            }
        }

        if !raw.is_empty() {
            for id in self.validate_ids(&raw, data)? {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }

        ids.sort_unstable();
        Ok(ids)
    }

    /// What is in the stash, grouped by the board each item came from.
    ///
    /// Grouped rather than flat because the grouping is the context: a
    /// note explaining four tasks is only useful next to them, and taking
    /// a finished area out of the way should not shred it on the way.
    pub fn display_stash(&self) -> Result<Outcome, EkkoError> {
        let mut data = self.storage.get()?;
        data.retain(|_, item| item.stashed.is_some());
        let boards = self.get_boards(&data);
        Ok(Outcome::Stash(self.group_by_board(&data, &boards)))
    }

    /// What is in the trash, and how long it has left.
    pub fn display_trash(&self) -> Result<Outcome, EkkoError> {
        let mut data = self.storage.get()?;
        data.retain(|_, item| item.trashed.is_some());
        let mut items: Vec<Item> = data.into_values().collect();
        items.sort_by_key(|item| item.trashed);
        Ok(Outcome::Trash(items))
    }

    /// The current month, drawn.
    ///
    /// Reads nothing: no lock, no storage, no board. That is the whole of
    /// this first cut on purpose -- drawing a month and deciding what a day
    /// should show are separate questions, and the second one is not
    /// answered yet. See the board for which way it goes.
    pub fn display_calendar(&self) -> Result<Outcome, EkkoError> {
        use chrono::Datelike;

        let now = chrono::Local::now();
        let month = CalendarMonth::of(now.year(), now.month(), Some(now.day()))
            .expect("today is always a real date in a real month");

        Ok(Outcome::Calendar(month))
    }

    /// Attaches a note to the task it explains.
    ///
    /// Reasons and work were siblings on the board, which is how a long
    /// note about item 12 ended up either crammed into 12's description or
    /// floating beside it with nothing connecting the two. Neither is a
    /// good option and both were the only ones available.
    ///
    /// Only a note can be attached, and only to a task. A task under a task
    /// is a subtask -- a different feature, with real questions about whose
    /// total it counts toward -- and a note under a note would allow chains
    /// and so cycles. One level, always, and cycles impossible by shape.
    ///
    /// Passing no task detaches, designed in from the start rather than
    /// discovered missing: `--blocked-by` shipped without it and left a
    /// wrong dependency with no way back.
    pub fn set_attached_to(&self, input: &[String]) -> Result<Outcome, EkkoError> {
        let _lock = self.storage.acquire_lock()?;
        let mut data = self.storage.get()?;

        let (id_str, rest) = self.extract_single_id_target(input)?;
        let id = self.validate_ids(&[id_str], &data)?[0];

        if data.get(&id).is_some_and(|item| item.is_task) {
            return Err(EkkoError::AttachNotANote(id));
        }

        let raw: Vec<String> = rest.into_iter().cloned().collect();
        if raw.len() > 1 {
            return Err(EkkoError::InvalidIdsNumber);
        }
        let target_id = if raw.is_empty() { None } else { Some(self.validate_ids(&raw, &data)?[0]) };
        let target = Self::attach_target(&data, id, target_id)?;

        let item = data.get_mut(&id).expect("id just validated against data");
        item.attached_to = target.as_ref().map(|(_, uid)| uid.clone());
        let updated = item.clone();

        self.save_touching(&mut data)?;
        Ok(Outcome::Attached { item: updated, target: target.map(|(id, _)| id) })
    }

    /// The task a note is to be attached to, with the uid to store, or `None`
    /// to detach: refuses anything but a note, attached to anything but a
    /// task that has a uid. Shared by --attached-to and the structured writes.
    pub(crate) fn attach_target(
        data: &ItemMap,
        note: u32,
        target: Option<u32>,
    ) -> Result<Option<(u32, String)>, EkkoError> {
        if data.get(&note).is_some_and(|item| item.is_task) {
            return Err(EkkoError::AttachNotANote(note));
        }
        let Some(target_id) = target else { return Ok(None) };
        let task = data.get(&target_id).ok_or_else(|| EkkoError::InvalidId(target_id.to_string()))?;
        if !task.is_task {
            return Err(EkkoError::AttachTargetNotATask(target_id));
        }
        let uid = task.uid.clone().ok_or(EkkoError::AttachTargetHasNoUid(target_id))?;
        Ok(Some((target_id, uid)))
    }

    /// Moves a whole project to the trash, reporting what went with it.
    ///
    /// The only irreversible-looking operation in Ekko, and the reason for
    /// each of its choices: it takes the project's own lock first, so a
    /// concurrent write finishes rather than being torn out from under
    /// itself; it counts before moving, because nothing else would ever be
    /// able to say how big the thing was; and it moves rather than deletes,
    /// because every other removal here has somewhere to come back from and
    /// this one had nothing.
    ///
    /// It does not ask. No other command in Ekko prompts, and a prompt here
    /// would break every script and agent that drives it. The confirmation
    /// is the count in the reply, and the safety net is the trash.
    ///
    /// One race is left, deliberately: a second process already blocked on
    /// the lock acquires it after the move and writes into the trashed copy
    /// rather than a live project. Nothing is lost, the writes simply land
    /// somewhere that is no longer listed. Closing it would need a tombstone
    /// protocol for a case that takes two processes racing on one project at
    /// the moment it is destroyed.
    pub fn destroy_project(
        &self,
        home_dir: &std::path::Path,
        project: &crate::project::Project,
        now_millis: i64,
    ) -> Result<Outcome, EkkoError> {
        let _lock = self.storage.acquire_lock()?;
        let data = self.storage.get()?;

        let tasks = data.values().filter(|item| item.is_task).count() as u32;
        let notes = data.len() as u32 - tasks;

        let trash = crate::project::destroy(home_dir, project, now_millis)?;

        Ok(Outcome::Destroyed { name: project.name.clone(), tasks, notes, trash })
    }

    pub fn set_state(&self, input: &[String], force: bool) -> Result<Outcome, EkkoError> {
        let _lock = self.storage.acquire_lock()?;
        let mut data = self.storage.get()?;

        let (id_tokens, state_tokens): (Vec<&String>, Vec<&String>) =
            input.iter().partition(|token| token.starts_with('@'));

        if id_tokens.is_empty() {
            return Err(EkkoError::MissingId);
        }
        if state_tokens.is_empty() {
            return Err(EkkoError::MissingState);
        }

        let raw_ids: Vec<String> =
            id_tokens.iter().map(|token| token.trim_start_matches('@').to_string()).collect();
        let ids = self.validate_ids(&raw_ids, &data)?;

        let mut states = Vec::new();
        for token in &state_tokens {
            match canonical_state(token) {
                Some(state) => states.push(state.to_string()),
                None => return Err(EkkoError::UnknownState((*token).clone())),
            }
        }
        let states = remove_duplicates(states);

        let before = data.clone();
        for id in &ids {
            if let Some(item) = data.get_mut(id) {
                for state in &states {
                    apply_state(item, state);
                }
            }
        }
        let (overridden, reopened) = Self::refuse_broken_dependencies(&before, &data, force)?;

        self.save_touching(&mut data)?;
        Ok(Outcome::Set { ids, states, overridden, reopened })
    }

    /// The board, restricted to items changed at or after `since` (epoch
    /// millis). Grouped by board like the default view, so the shape a
    /// caller parses does not change with the filter.
    ///
    /// Only reports items that exist. A deletion leaves nothing behind to
    /// carry a timestamp, so a caller that must notice removals has to
    /// compare id sets, not just read this.
    pub fn display_since(&self, since: i64) -> Result<Outcome, EkkoError> {
        let mut data = self.visible()?;
        // Items written before `updatedAt` existed fall back to their
        // creation time. Otherwise they would be invisible to every
        // `--since`, including `--since 0` on a first sync, which is worse
        // than reporting the one instant we do know about them.
        data.retain(|_, item| item.updated_at.unwrap_or(item.timestamp) >= since);
        let boards = self.get_boards(&data);
        Ok(Outcome::Board(self.group_by_board(&data, &boards)))
    }

    /// Unmet blockers for every item that has any, keyed by display id.
    pub fn blocker_map(&self) -> Result<std::collections::HashMap<u32, Vec<u32>>, EkkoError> {
        let data = self.storage.get()?;
        let index = uid_index(&data);
        let mut map = std::collections::HashMap::new();
        for (id, item) in &data {
            let unmet = Self::unmet_blockers_indexed(&index, &data, item);
            if !unmet.is_empty() {
                map.insert(*id, unmet);
            }
        }
        Ok(map)
    }

    /// Records what one item is blocked by, replacing whatever blocked it
    /// before -- the same contract `--move` and `--phases` use.
    ///
    /// Refuses to create a cycle. Without one, A waiting on B while B waits
    /// on A is a pair nothing can ever make ready, and the board would state
    /// it as calmly as any other fact.
    pub fn set_blocked_by(&self, input: &[String]) -> Result<Outcome, EkkoError> {
        let _lock = self.storage.acquire_lock()?;
        let mut data = self.storage.get()?;

        let (id_str, rest) = self.extract_single_id_target(input)?;
        let id = self.validate_ids(&[id_str], &data)?[0];

        // No blockers given means clear, not "you forgot the blockers".
        // This shipped erroring instead, which left `blocked_by = None`
        // below unreachable and a wrongly blocked task with no way back
        // except delete and recreate. Real use found it first: an agent
        // had to write "this dependency is wrong and ekko will not let me
        // clear it" into a task description, which is a false statement
        // the board then carries as if it were true.
        let raw: Vec<String> = rest.into_iter().cloned().collect();
        let blocker_ids =
            if raw.is_empty() { Vec::new() } else { self.validate_ids(&raw, &data)? };

        let phases = self.storage.get_phases()?;
        let uids = self.blocker_uids(&data, &phases, id, &blocker_ids)?;

        let before = data.clone();
        let item = data.get_mut(&id).expect("id just validated against data");
        item.blocked_by = if uids.is_empty() { None } else { Some(uids) };
        let updated = item.clone();
        // Completed work cannot be declared to wait on open work either.
        Self::refuse_broken_dependencies(&before, &data, false)?;

        self.save_touching(&mut data)?;
        Ok(Outcome::Blocked { item: updated, blockers: blocker_ids })
    }

    /// The uids `id` may be recorded as blocked by: refuses a cycle, and a
    /// dependency against the declared phase order. Shared by --blocked-by
    /// and the structured writes, so a dependency is judged the same way
    /// however it is made.
    pub(crate) fn blocker_uids(
        &self,
        data: &ItemMap,
        phases: &[String],
        id: u32,
        blocker_ids: &[u32],
    ) -> Result<Vec<String>, EkkoError> {
        // A note has no state to finish, so a dependency on one would block
        // nothing while the board showed it as a dependency.
        if let Some(note) = blocker_ids.iter().find(|blocker| data.get(blocker).is_some_and(|item| !item.is_task)) {
            return Err(EkkoError::InvalidInput(format!(
                "{note} is a note, and only a task can block: a note has no state to finish, so it would block nothing"
            )));
        }
        for blocker in blocker_ids {
            if *blocker == id || self.reaches(data, *blocker, id) {
                return Err(EkkoError::BlockingCycle(id, *blocker));
            }
        }

        // Refused where the dependency is made rather than discovered later:
        // a phase cannot wait on one that comes after it. Reordering phases
        // afterwards is not refused -- the roadmap names what it leaves behind.
        let order = phase_order(phases);
        for blocker in blocker_ids {
            if let Some(inversion) = phase_inversion(&order, &data[&id], &data[blocker]) {
                return Err(EkkoError::PhaseOrder(inversion));
            }
        }

        Ok(blocker_ids.iter().filter_map(|b| data.get(b)?.uid.clone()).collect())
    }

    /// Whether `from` is already blocked, directly or through others, by `target`.
    ///
    /// A depth-first walk that visits each item once. It used to recurse along
    /// every path with no record of where it had been, so a chain of 26
    /// diamonds -- 80 tasks -- took 12.5 s to accept one dependency, and the
    /// time doubled with each diamond. Uids resolve through one index built up
    /// front instead of a scan of the whole board at every step.
    fn reaches(&self, data: &ItemMap, from: u32, target: u32) -> bool {
        let index = uid_index(data);
        let mut seen = HashSet::new();
        let mut stack = vec![from];
        while let Some(id) = stack.pop() {
            if id == target {
                return true;
            }
            if !seen.insert(id) {
                continue;
            }
            if let Some(blockers) = data.get(&id).and_then(|item| item.blocked_by.as_ref()) {
                stack.extend(blockers.iter().filter_map(|uid| index.get(uid.as_str()).copied()));
            }
        }
        false
    }

    /// `unmet_blockers_indexed` with the index built on the spot, for tests
    /// asking about a single item.
    #[cfg(test)]
    pub fn unmet_blockers(data: &ItemMap, item: &Item) -> Vec<u32> {
        Self::unmet_blockers_indexed(&uid_index(data), data, item)
    }

    /// The blockers of `item` that are still outstanding, as current display
    /// ids. A finished, cancelled or trashed blocker is not one -- which is
    /// why nothing ever has to be unblocked by hand -- and neither is a note,
    /// which has no state and so could never be finished.
    ///
    /// `data` has to be the whole of storage, and every caller passes it.
    /// Trashed is checked here rather than filtered out beforehand because
    /// `--delete` stopped removing items when the trash arrived: a deleted
    /// blocker stayed in storage, still open, and went on blocking on the
    /// board while `--list ready` -- reading the view, where it was gone --
    /// called the same task ready. A stashed blocker, on the other hand,
    /// still holds: stashing hides an item without finishing it.
    ///
    /// `index` is `uid_index(data)`, built once by the caller: every caller
    /// asks about many items, and a scan of the board per blocker made each
    /// of them quadratic.
    fn unmet_blockers_indexed(index: &HashMap<&str, u32>, data: &ItemMap, item: &Item) -> Vec<u32> {
        let Some(uids) = item.blocked_by.as_ref() else { return Vec::new() };

        let mut ids: Vec<u32> = uids
            .iter()
            .filter_map(|uid| index.get(uid.as_str()).copied())
            .filter(|id| data.get(id).is_some_and(holds))
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    /// Refuses a command that would break the rule a dependency states --
    /// completed work never waits on open work -- or, with `force`, lets it
    /// and returns what it pushed past, so the reply can say so.
    ///
    /// One rule, checked in one place, whichever command runs. `after` is the
    /// board as the command would leave it, and only a pair it breaks that
    /// `before` did not already break counts. Three things follow:
    ///
    /// - A blocker and what it blocks can be completed together, or reopened
    ///   together, in one command: the board they leave keeps the rule.
    /// - A pair an earlier `--force` broke does not make later commands fail,
    ///   so `--set @3 done` stays safe to retry after a forced completion.
    /// - Starting, pausing and cancelling an open task never break it. Only
    ///   completing, reopening -- reviving a cancelled task included -- and
    ///   declaring a dependency can.
    ///
    /// Each broken pair is reported from the side the command moved: a task
    /// it completed (`BLOCKED`), a blocker it reopened
    /// (`COMPLETED_DEPENDENTS`), or, when neither moved, the dependency it
    /// declared (`ALREADY_DONE`, which only `--blocked-by` reaches, and which
    /// nothing forces). All or nothing, like an invalid id: nothing is
    /// written, rather than leaving the caller to work out which half landed.
    ///
    /// Recovering items -- `--restore`, `--untrash` -- brings them back as
    /// they were, and is not checked: refusing to give back what was removed
    /// would be the worse surprise.
    pub(crate) fn refuse_broken_dependencies(
        before: &ItemMap,
        after: &ItemMap,
        force: bool,
    ) -> Result<(Linked, Linked), EkkoError> {
        let mut completed_over: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
        let mut reopened_under: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
        let mut declared: BTreeMap<u32, Vec<u32>> = BTreeMap::new();

        let already = broken_dependencies(before);
        for (task, blocker) in broken_dependencies(after) {
            if already.contains(&(task, blocker)) {
                continue;
            }
            if !before.get(&task).is_some_and(completed) {
                completed_over.entry(task).or_default().push(blocker);
            } else if !before.get(&blocker).is_some_and(holds) {
                reopened_under.entry(blocker).or_default().push(task);
            } else {
                declared.entry(task).or_default().push(blocker);
            }
        }

        let completed_over: Linked = completed_over.into_iter().collect();
        let reopened_under: Linked = reopened_under.into_iter().collect();
        let declared: Linked = declared.into_iter().collect();
        if !declared.is_empty() {
            return Err(EkkoError::AlreadyDone(declared));
        }
        if !force && !completed_over.is_empty() {
            return Err(EkkoError::Blocked(completed_over));
        }
        if !force && !reopened_under.is_empty() {
            return Err(EkkoError::CompletedDependents(reopened_under));
        }
        Ok((completed_over, reopened_under))
    }

    /// The tasks a write set free, and the ones it left waiting: open and
    /// listed on both sides of the write, waiting on open work on one side
    /// and not on the other, as display ids in order.
    ///
    /// This is what a reply names so that its caller does not read the board
    /// again to learn what its write moved -- the call an agent otherwise
    /// makes after every completion. Only tasks that existed before count: a
    /// task the write created, or reopened, is the caller's own doing, not
    /// news about the board. A stashed task is not listed, so it is not news
    /// either.
    pub(crate) fn readiness_changes(before: &ItemMap, after: &ItemMap) -> (Vec<u32>, Vec<u32>) {
        let (before_index, after_index) = (uid_index(before), uid_index(after));
        let listed = |item: &Item| holds(item) && item.stashed.is_none();
        let mut released = Vec::new();
        let mut blocked = Vec::new();
        for (id, item) in after {
            let Some(was) = before.get(id) else { continue };
            if !listed(was) || !listed(item) {
                continue;
            }
            let waited = !Self::unmet_blockers_indexed(&before_index, before, was).is_empty();
            let waits = !Self::unmet_blockers_indexed(&after_index, after, item).is_empty();
            match (waited, waits) {
                (true, false) => released.push(*id),
                (false, true) => blocked.push(*id),
                _ => {}
            }
        }
        (released, blocked)
    }

    /// Replaces the project's phase sequence.
    pub fn set_phases(&self, names: &[String]) -> Result<Outcome, EkkoError> {
        let _lock = self.storage.acquire_lock()?;
        let cleaned = remove_duplicates(
            names.iter().map(|n| n.trim_start_matches('@').to_string()).collect(),
        );
        if self.storage.get_phases()? != cleaned {
            // The order work is taken up in moved though no item did: the
            // revision says so, and `changes` tells its reader to prime again.
            let mut counters = self.storage.get_counters()?;
            counters.revision += 1;
            self.storage.set_counters(counters)?;
        }
        self.storage.set_phases(&cleaned)?;
        Ok(Outcome::Phases(cleaned))
    }

    /// The roadmap: declared phases in order, each with how far it has got,
    /// and which one holds work in progress.
    ///
    /// Nothing here is stored beyond the sequence itself -- the counts and
    /// the cursor are read off the items every time, so the view cannot
    /// drift from the board it describes.
    pub fn display_roadmap(&self) -> Result<Outcome, EkkoError> {
        let data = self.visible()?;
        let phases = self.storage.get_phases()?;

        let mut steps = Vec::new();
        for name in &phases {
            let items: Vec<&Item> =
                data.values().filter(|i| i.phase.as_deref() == Some(name.as_str())).collect();

            // Counted by `tally`, the one definition of a total: cancelled work
            // is not work, here as in the percentage and the board title.
            let (complete, total) = tally(items.iter().copied());
            let current = items.iter().any(|i| State::of(i) == Some(State::Progress));

            steps.push(RoadmapStep {
                name: name.clone(),
                complete,
                total,
                notes: items.iter().filter(|i| !i.is_task).count() as u32,
                current,
            });
        }

        // Anything inside the project but outside every phase. Counted rather
        // than hidden: the root is a deliberate exception, not a hole.
        let rootless = data.values().filter(|i| i.phase.is_none()).count() as u32;

        // Dependencies that run against the phase order -- possible only when
        // phases were reordered underneath them, since --blocked-by refuses
        // making one. Named rather than hidden.
        let order = phase_order(&phases);
        let index = uid_index(&data);
        let inversions: Vec<Inversion> = data
            .values()
            .flat_map(|item| {
                item.blocked_by.iter().flatten().filter_map(|uid| {
                    let blocker = data.get(index.get(uid.as_str())?)?;
                    phase_inversion(&order, item, blocker)
                })
            })
            .collect();

        Ok(Outcome::Roadmap { steps, rootless, inversions })
    }

    pub fn list_by_attributes(&self, terms: &[String]) -> Result<Outcome, EkkoError> {
        let data = self.visible()?;
        let stored_boards = self.get_boards(&data);

        let (mut boards, mut attributes) = (Vec::new(), Vec::new());
        for term in terms {
            // Two deliberate departures from the JS version here, both aimed
            // at the same failure: it accepted anything and quietly listed
            // *everything* when a term matched nothing, which reads as a
            // successful filter returning the whole board.
            //
            // First, `@board` is accepted alongside the bare `board` the JS
            // version wanted. The board view prints names in their `@name`
            // form, so feeding one straight back is the obvious move -- and
            // it was the one that silently did nothing.
            let at_board =
                if term.starts_with('@') { term.clone() } else { format!("@{term}") };

            if stored_boards.contains(&at_board) {
                boards.push(at_board);
            } else if term == "myboard" {
                boards.push("My Board".to_string());
            } else if is_known_attribute(term) {
                attributes.push(term.clone());
            } else {
                // Second: a term that names neither a board nor a known
                // attribute is a typo or a board that does not exist, and
                // saying so beats handing back a plausible-looking answer.
                return Err(EkkoError::UnknownListTerm(term.clone()));
            }
        }
        let boards = remove_duplicates(boards);
        let attributes = remove_duplicates(attributes);

        let all = self.storage.get()?;
        let filtered = self.filter_by_attributes(&attributes, data, &all);
        Ok(Outcome::List(self.group_by_board(&filtered, &boards)))
    }
}

/// Moves each attached note to sit directly after the task it explains.
///
/// Only within one group, and only when the task is in it. A note whose
/// task lives on another board stays exactly where it was rather than
/// jumping between boards -- surprising placement is worse than an
/// un-nested reason, and the note is still findable where it was filed.
///
/// A group with nothing attached comes back untouched, which is what keeps
/// every existing board -- goldens included -- rendering in id order.
fn reorder_attached(items: &mut Vec<Item>) {
    if !items.iter().any(|item| item.attached_to.is_some()) {
        return;
    }

    let (attached, mut rest): (Vec<Item>, Vec<Item>) =
        items.drain(..).partition(|item| item.attached_to.is_some());

    let mut orphans = Vec::new();
    for note in attached {
        let target = note.attached_to.as_deref().expect("partitioned on attached_to being set");
        match rest.iter().position(|item| item.uid.as_deref() == Some(target)) {
            Some(at) => rest.insert(at + 1, note),
            None => orphans.push(note),
        }
    }

    // Orphans keep their id order among themselves, appended rather than
    // dropped: a note whose task is elsewhere is still this board's note.
    orphans.sort_by_key(|item| item.id);
    rest.extend(orphans);
    *items = rest;
}

/// The display id of the item carrying `uid`, if any.
///
/// Shared by `validate_ids` and `reaches` rather than written twice: they
/// are asking the same question, and a dependency stored by uid has to
/// resolve the same way whether a cycle check or a caller is asking.
fn find_by_uid(items: &ItemMap, uid: &str) -> Option<u32> {
    items.iter().find(|(_, item)| item.uid.as_deref() == Some(uid)).map(|(id, _)| *id)
}

/// Ids, each with the ids on the other side of a dependency: what blocks it,
/// or what it blocks, as the name holding it says.
pub(crate) type Linked = Vec<(u32, Vec<u32>)>;

/// Whether `item` is completed work as dependencies see it: a done task, not
/// in the trash.
fn completed(item: &Item) -> bool {
    item.trashed.is_none() && State::of(item) == Some(State::Done)
}

/// Whether `item` holds up what it blocks: an open task, not in the trash.
/// Done and cancelled are closed, and a note has no state to finish. A
/// stashed task still holds -- stashing hides an item without finishing it.
pub(crate) fn holds(item: &Item) -> bool {
    item.trashed.is_none() && State::of(item).is_some_and(State::is_open)
}

/// Every place a board breaks the rule its dependencies state -- completed
/// work waiting on open work -- as (task, blocker) display ids, in order.
///
/// Empty unless `--force` overrode the rule or a recovery brought back
/// something that breaks it; every other write refuses to. Display ids are
/// safe to compare across one command, which never renumbers.
pub(crate) fn broken_dependencies(data: &ItemMap) -> BTreeSet<(u32, u32)> {
    let index = uid_index(data);
    let mut broken = BTreeSet::new();
    for (id, item) in data.iter().filter(|(_, item)| completed(item)) {
        for uid in item.blocked_by.iter().flatten() {
            let Some(&blocker) = index.get(uid.as_str()) else { continue };
            if data.get(&blocker).is_some_and(holds) {
                broken.insert((*id, blocker));
            }
        }
    }
    broken
}

/// Every uid on a board, resolved to its display id in one pass.
pub(crate) fn uid_index(items: &ItemMap) -> HashMap<&str, u32> {
    items.iter().filter_map(|(id, item)| Some((item.uid.as_deref()?, *id))).collect()
}

/// Each declared phase with its position in the sequence.
pub(crate) fn phase_order(phases: &[String]) -> HashMap<&str, usize> {
    phases.iter().enumerate().map(|(at, name)| (name.as_str(), at)).collect()
}

/// The inversion `blocked` waiting on `blocker` would be, if `blocker` sits in
/// a declared phase later than `blocked`'s. Items at the project root, or in a
/// phase no longer declared, are outside the order and never inverted.
///
/// The one definition, shared by the refusal in --blocked-by and the report
/// in --roadmap, so the two cannot disagree about what counts.
pub(crate) fn phase_inversion(order: &HashMap<&str, usize>, blocked: &Item, blocker: &Item) -> Option<Inversion> {
    let blocked_phase = blocked.phase.as_deref()?;
    let blocker_phase = blocker.phase.as_deref()?;
    (order.get(blocker_phase)? > order.get(blocked_phase)?).then(|| Inversion {
        blocked: blocked.id,
        blocked_phase: blocked_phase.to_string(),
        blocker: blocker.id,
        blocker_phase: blocker_phase.to_string(),
    })
}

fn is_priority_opt(token: &str) -> bool {
    matches!(token, "p:1" | "p:2" | "p:3")
}

fn get_priority(input: &[String]) -> u8 {
    input
        .iter()
        .find(|t| is_priority_opt(t))
        .and_then(|t| t.chars().last())
        .and_then(|c| c.to_digit(10))
        .map(|d| d as u8)
        .unwrap_or(1)
}

/// `d:YYYY-MM-DD`, mirroring how `p:N` marks priority. Validated here
/// rather than at render time so a typo is rejected at the point the user
/// can still see what they typed, instead of silently becoming a task with
/// no deadline.
fn is_due_opt(token: &str) -> bool {
    token.starts_with("d:")
}

pub(crate) fn parse_due_date(token: &str) -> Option<String> {
    let value = token.strip_prefix("d:")?;
    let parsed = chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").ok()?;
    // Round-tripped through chrono so the stored form is always canonical:
    // `d:2026-9-1` and `d:2026-09-01` land as the same string, which keeps
    // the plain string comparisons in `due_state` honest.
    Some(parsed.format("%Y-%m-%d").to_string())
}

fn has_terms(text: &str, terms: &[String]) -> bool {
    let lower = text.to_lowercase();
    terms.iter().any(|term| lower.contains(&term.to_lowercase()))
}

fn join_ids(ids: &[u32]) -> String {
    ids.iter().map(u32::to_string).collect::<Vec<_>>().join(", ")
}

pub(crate) fn remove_duplicates(items: Vec<String>) -> Vec<String> {
    let mut seen = Vec::new();
    for item in items {
        if !seen.contains(&item) {
            seen.push(item);
        }
    }
    seen
}
/// The state vocabulary `--set` accepts, mapped to its canonical spelling.
/// Deliberately the same words `--list` filters on, so there is one set of
/// names to learn rather than two.
pub(crate) fn canonical_state(term: &str) -> Option<&'static str> {
    match term {
        "done" | "checked" | "complete" => Some("done"),
        "undone" | "unchecked" | "incomplete" | "pending" => Some("undone"),
        "progress" | "started" | "begun" => Some("progress"),
        "paused" => Some("paused"),
        // Repointed: with a real paused state these became opposites.
        // Also the way back from a mistyped `--set progress`.
        "unstarted" | "unstart" => Some("unstarted"),
        "cancel" | "cancelled" | "canceled" => Some("cancelled"),
        "star" | "starred" => Some("starred"),
        "unstar" | "unstarred" => Some("unstarred"),
        _ => None,
    }
}

/// Applies one canonical state. Task states go through `State::after` and
/// `State::write`, so the result is always one of the five encodings; notes
/// have no state and skip them, matching how `--check` and `--begin` ignore
/// notes. Starring is the one that applies to both.
pub(crate) fn apply_state(item: &mut Item, state: &str) {
    match state {
        "starred" => item.is_starred = true,
        "unstarred" => item.is_starred = false,
        other => {
            if let (Some(current), Some(change)) = (State::of(item), change_for(other)) {
                current.after(change).write(item);
            }
        }
    }
}

/// The state change a canonical `--set` word asks for. `unstarted` is the
/// way back to never-started from anywhere, done and cancelled included.
fn change_for(state: &str) -> Option<Change> {
    Some(match state {
        "done" => Change::Become(State::Done),
        "undone" => Change::Undo,
        "progress" => Change::Become(State::Progress),
        "paused" => Change::Become(State::Paused),
        "cancelled" => Change::Become(State::Cancelled),
        "unstarted" => Change::Become(State::Pending),
        _ => return None,
    })
}
/// The attribute terms `--list` filters on. Kept beside
/// `Ekko::filter_by_attributes`, which is the code that acts on them --
/// the two must agree, or `list_by_attributes` would reject a term the
/// filter would happily have handled.
fn is_known_attribute(term: &str) -> bool {
    matches!(
        term,
        "star"
            | "starred"
            | "done"
            | "checked"
            | "complete"
            | "progress"
            | "started"
            | "begun"
            | "pending"
            | "unchecked"
            | "incomplete"
            | "todo"
            | "task"
            | "tasks"
            | "note"
            | "notes"
            | "cancelled"
            | "canceled"
            | "paused"
            | "ready"
            | "blocked"
            | "due"
            | "overdue"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::render::Painter;
    use std::fs;
    use std::path::PathBuf;
    use std::process;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn words(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn fresh_ekko() -> (Ekko, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "ekko-core-test-{}-{}",
            process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let ekko = Ekko::new(Storage::new(&dir).unwrap());
        (ekko, dir)
    }

    fn cleanup(dir: &std::path::Path) {
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn create_task_assigns_sequential_ids_and_defaults_to_my_board() {
        let (ekko, dir) = fresh_ekko();

        let Outcome::Task(first) = ekko.create_task(&words(&["first task"])).unwrap() else { panic!() };
        let Outcome::Task(second) = ekko.create_task(&words(&["@coding", "second task"])).unwrap() else { panic!() };

        assert_eq!(first.id, 1);
        assert_eq!(first.boards, vec!["My Board".to_string()]);
        assert_eq!(second.id, 2);
        assert_eq!(second.boards, vec!["@coding".to_string()]);

        cleanup(&dir);
    }

    #[test]
    fn create_task_rejects_a_priority_marker_with_no_real_description() {
        let (ekko, dir) = fresh_ekko();

        let result = ekko.create_task(&words(&["p:2"]));
        assert!(matches!(result, Err(EkkoError::MissingDesc)));

        let Outcome::Board(groups) = ekko.display_by_board().unwrap() else { panic!() };
        assert!(groups.is_empty(), "no task should have been created");

        cleanup(&dir);
    }

    #[test]
    fn create_note_rejects_a_board_marker_with_no_real_description() {
        let (ekko, dir) = fresh_ekko();

        let result = ekko.create_note(&words(&["@onlyaboard"]));

        assert!(matches!(result, Err(EkkoError::MissingDesc)));

        cleanup(&dir);
    }

    #[test]
    fn inline_priority_marker_is_parsed_and_stripped_from_the_description() {
        let (ekko, dir) = fresh_ekko();

        let Outcome::Task(item) = ekko.create_task(&words(&["@coding", "Fix", "the", "bug", "p:3"])).unwrap() else {
            panic!()
        };

        assert_eq!(item.description, "Fix the bug");
        assert_eq!(item.priority, Some(3));

        cleanup(&dir);
    }

    #[test]
    fn check_toggles_completion_and_ignores_notes() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["a task"])).unwrap();
        ekko.create_note(&words(&["a note"])).unwrap();

        let Outcome::Check { checked, unchecked, .. } = ekko.check_tasks(&words(&["1", "2"]), false).unwrap() else { panic!() };
        assert_eq!(checked, vec![1]);
        assert_eq!(unchecked, Vec::<u32>::new());

        let Outcome::Check { checked, unchecked, .. } = ekko.check_tasks(&words(&["1"]), false).unwrap() else { panic!() };
        assert_eq!(checked, Vec::<u32>::new());
        assert_eq!(unchecked, vec![1]);

        cleanup(&dir);
    }

    #[test]
    fn star_applies_to_notes_too_unlike_check_and_begin() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_note(&words(&["a note"])).unwrap();

        let Outcome::Star { starred, .. } = ekko.star_items(&words(&["1"])).unwrap() else { panic!() };

        assert_eq!(starred, vec![1]);

        cleanup(&dir);
    }

    /// Was about --delete archiving, which it no longer does. Kept,
    /// pointed at --clear, because the archive id being unrelated to the
    /// storage id is still true and still the trap it always was.
    #[test]
    fn clearing_reports_both_the_storage_id_and_the_new_unrelated_archive_id() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["first"])).unwrap();
        ekko.set_state(&words(&["@1", "done"]), false).unwrap();

        let Outcome::Delete(results) = ekko.clear().unwrap() else { panic!() };

        assert_eq!(results, vec![DeleteResult { storage_id: 1, archive_id: 1 }]);

        cleanup(&dir);
    }

    #[test]
    fn restore_gives_the_item_a_fresh_storage_id_not_the_old_one() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["a"])).unwrap();
        ekko.create_task(&words(&["b"])).unwrap();
        ekko.set_state(&words(&["@1", "done"]), false).unwrap();
        ekko.clear().unwrap(); // archive id 1

        let Outcome::Restore(results) = ekko.restore_items(&words(&["1"])).unwrap() else { panic!() };

        // Next storage id is 3 (1 and 2 already used, 1 left storage but
        // ids aren't reused across a *different* map the way they are
        // within the same one).
        assert_eq!(results, vec![RestoreResult { archive_id: 1, storage_id: 3 }]);

        cleanup(&dir);
    }

    /// An id is never handed out twice, even once its item has left storage.
    /// `--clear` archiving the highest-numbered item used to give its number
    /// to the next task, and an agent still holding that number then acted on
    /// the wrong item with no error (evals/agent/semantics.py, item 3.4).
    /// Storage keeps the highest id it has held in counters.json instead.
    #[test]
    fn an_id_is_never_handed_out_again_once_its_item_leaves_storage() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["a"])).unwrap();
        ekko.create_task(&words(&["b"])).unwrap(); // id 2
        ekko.set_state(&words(&["@2", "done"]), false).unwrap();
        ekko.clear().unwrap();

        let Outcome::Task(item) = ekko.create_task(&words(&["c"])).unwrap() else { panic!() };

        assert_eq!(item.id, 3, "id 2 was handed out again after its item was archived");

        cleanup(&dir);
    }

    /// The other half of the same rule, and the reason it has to hold:
    /// something in the trash keeps its number, so recovering it by that
    /// number is unambiguous.
    #[test]
    fn a_trashed_item_keeps_its_id_so_nothing_can_take_it() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["a"])).unwrap();
        ekko.create_task(&words(&["b"])).unwrap();
        ekko.delete_items(&words(&["2"])).unwrap();

        let Outcome::Task(item) = ekko.create_task(&words(&["c"])).unwrap() else { panic!() };

        assert_eq!(item.id, 3, "a new item took the trashed item's number");
        ekko.set_trashed(&words(&["2"]), false).unwrap();
        let Outcome::Board(groups) = ekko.display_by_board().unwrap() else { panic!() };
        assert_eq!(groups[0].1.len(), 3, "recovering it collided with something");

        cleanup(&dir);
    }

    #[test]
    fn move_replaces_the_board_list_rather_than_appending_to_it() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["@bugs", "x"])).unwrap();

        let Outcome::Move(item) = ekko.move_boards(&words(&["@1", "tests"])).unwrap() else { panic!() };

        assert_eq!(item.boards, vec!["@tests".to_string()], "the original @bugs tag should be gone, not kept alongside @tests");

        cleanup(&dir);
    }

    #[test]
    fn priority_check_runs_before_the_missing_id_check() {
        let (ekko, dir) = fresh_ekko();

        // Neither a valid priority digit nor an `@id` target is present;
        // JS checks for the priority digit first, so that's the error
        // that should come back, not MissingId.
        let result = ekko.update_priority(&words(&["nonsense"]));

        assert!(matches!(result, Err(EkkoError::InvalidPriority)));

        cleanup(&dir);
    }

    #[test]
    fn clear_deletes_only_complete_tasks_and_is_a_noop_with_none() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["done"])).unwrap();
        ekko.create_task(&words(&["not done"])).unwrap();
        ekko.check_tasks(&words(&["1"]), false).unwrap();

        let Outcome::Delete(results) = ekko.clear().unwrap() else { panic!() };
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].storage_id, 1);

        let Outcome::Delete(results) = ekko.clear().unwrap() else { panic!() };
        assert!(results.is_empty(), "nothing left complete, clear should be a no-op");

        cleanup(&dir);
    }

    #[test]
    fn pending_filter_matches_the_js_version_exactly_including_in_progress_tasks() {
        // Ported as-observed, not as one might assume: JS's "pending"
        // filter only checks `!isComplete`, so an in-progress task passes
        // it too. Locking that in deliberately rather than "fixing" it
        // during the port.
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["pending one"])).unwrap();
        ekko.create_task(&words(&["in progress one"])).unwrap();
        ekko.begin_tasks(&words(&["2"])).unwrap();

        let Outcome::List(groups) = ekko.list_by_attributes(&words(&["pending"])).unwrap() else { panic!() };
        let ids: Vec<u32> = groups.iter().flat_map(|(_, items)| items.iter().map(|i| i.id)).collect();

        assert!(ids.contains(&1));
        assert!(ids.contains(&2), "an in-progress task should still show up under the pending filter, matching JS");

        cleanup(&dir);
    }

    #[test]
    fn a_blocker_that_finishes_stops_blocking_without_anyone_saying_so() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["first"])).unwrap();
        ekko.create_task(&words(&["second"])).unwrap();
        ekko.set_blocked_by(&words(&["@2", "1"])).unwrap();

        let data = ekko.storage.get().unwrap();
        assert_eq!(Ekko::unmet_blockers(&data, &data[&2]), vec![1]);

        ekko.set_state(&words(&["@1", "done"]), false).unwrap();

        let data = ekko.storage.get().unwrap();
        assert!(Ekko::unmet_blockers(&data, &data[&2]).is_empty(), "nothing to unblock by hand");

        cleanup(&dir);
    }

    #[test]
    fn blockers_are_stored_by_uid_so_a_recycled_id_cannot_repoint_them() {
        // Ids are max + 1, so deleting the highest and creating another hands
        // the number back. A dependency stored as a number would follow it.
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["blocker"])).unwrap();
        ekko.create_task(&words(&["waiter"])).unwrap();
        ekko.create_task(&words(&["doomed"])).unwrap();
        ekko.set_blocked_by(&words(&["@2", "1"])).unwrap();

        let stored = ekko.storage.get().unwrap()[&2].blocked_by.clone().unwrap();
        let blocker_uid = ekko.storage.get().unwrap()[&1].uid.clone().unwrap();
        assert_eq!(stored, vec![blocker_uid], "stored by uid, not by 1");

        cleanup(&dir);
    }

    #[test]
    fn a_cycle_is_refused_rather_than_recorded() {
        // Without it, two items wait on each other and neither can ever be
        // ready -- a fact the board would state as calmly as any other.
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["a"])).unwrap();
        ekko.create_task(&words(&["b"])).unwrap();
        ekko.create_task(&words(&["c"])).unwrap();
        ekko.set_blocked_by(&words(&["@2", "1"])).unwrap();
        ekko.set_blocked_by(&words(&["@3", "2"])).unwrap();

        // Direct, and through the chain a -> b -> c.
        assert!(matches!(
            ekko.set_blocked_by(&words(&["@1", "2"])),
            Err(EkkoError::BlockingCycle(_, _))
        ));
        assert!(matches!(
            ekko.set_blocked_by(&words(&["@1", "3"])),
            Err(EkkoError::BlockingCycle(_, _))
        ));

        cleanup(&dir);
    }

    #[test]
    fn ready_excludes_blocked_work_and_blocked_finds_it() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["free"])).unwrap();
        ekko.create_task(&words(&["waiting"])).unwrap();
        ekko.set_blocked_by(&words(&["@2", "1"])).unwrap();

        let ids = |o: Outcome| -> Vec<u32> {
            let Outcome::List(groups) = o else { panic!() };
            groups.iter().flat_map(|(_, i)| i.iter().map(|x| x.id)).collect()
        };

        assert_eq!(ids(ekko.list_by_attributes(&words(&["ready"])).unwrap()), vec![1]);
        assert_eq!(ids(ekko.list_by_attributes(&words(&["blocked"])).unwrap()), vec![2]);

        cleanup(&dir);
    }

    #[test]
    fn deleting_a_blocker_unblocks_what_waited_on_it() {
        // A blocker that no longer exists cannot be finished, so treating it
        // as still blocking would strand the waiter forever.
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["blocker"])).unwrap();
        ekko.create_task(&words(&["waiter"])).unwrap();
        ekko.set_blocked_by(&words(&["@2", "1"])).unwrap();

        ekko.delete_items(&words(&["1"])).unwrap();

        // By id, not `values().next()`. Written when deleting removed the
        // item, that picked the waiter; once deleting started trashing, the
        // blocker stayed in storage and came first -- so this went on
        // passing by inspecting the blocker, which blocks nothing.
        let data = ekko.storage.get().unwrap();
        assert!(Ekko::unmet_blockers(&data, &data[&2]).is_empty());

        // And every surface agrees: the marker, and the filters.
        assert_eq!(ekko.blocker_map().unwrap().get(&2), None, "the board still draws the marker");
        let listed = |term: &str| -> Vec<u32> {
            let Outcome::List(groups) = ekko.list_by_attributes(&words(&[term])).unwrap() else {
                panic!()
            };
            groups.iter().flat_map(|(_, items)| items.iter().map(|x| x.id)).collect()
        };
        assert_eq!(listed("ready"), vec![2]);
        assert!(listed("blocked").is_empty());

        cleanup(&dir);
    }

    /// Stashing hides an item without finishing it, so a pending blocker put
    /// away still holds -- on the board and in the filters alike. The view
    /// dropping it was how `--list ready` came to disagree with the marker.
    #[test]
    fn a_stashed_blocker_is_hidden_not_finished_so_it_still_holds() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["blocker"])).unwrap();
        ekko.create_task(&words(&["waiter"])).unwrap();
        ekko.set_blocked_by(&words(&["@2", "1"])).unwrap();

        ekko.set_stashed(&words(&["1"]), true).unwrap();

        assert_eq!(ekko.blocker_map().unwrap().get(&2), Some(&vec![1]));
        let listed = |term: &str| -> Vec<u32> {
            let Outcome::List(groups) = ekko.list_by_attributes(&words(&[term])).unwrap() else {
                panic!()
            };
            groups.iter().flat_map(|(_, items)| items.iter().map(|x| x.id)).collect()
        };
        assert!(listed("ready").is_empty(), "a hidden blocker stopped holding");
        assert_eq!(listed("blocked"), vec![2]);

        cleanup(&dir);
    }

    #[test]
    fn phases_are_replaced_wholesale_so_reordering_and_inserting_are_one_operation() {
        // Appending could not express "put testing between the last two"
        // without three more commands to fix the ordering afterwards.
        let (ekko, dir) = fresh_ekko();
        ekko.set_phases(&words(&["setup", "ship"])).unwrap();

        ekko.set_phases(&words(&["setup", "testing", "ship"])).unwrap();

        assert_eq!(ekko.storage.get_phases().unwrap(), words(&["setup", "testing", "ship"]));

        cleanup(&dir);
    }

    #[test]
    fn an_item_created_without_a_phase_lands_outside_the_roadmap_and_is_counted() {
        let (ekko, dir) = fresh_ekko();
        ekko.set_phases(&words(&["setup"])).unwrap();
        ekko.create_task_in(&words(&["@a", "in a phase"]), Some("setup")).unwrap();
        ekko.create_task_in(&words(&["@b", "at the root"]), None).unwrap();

        let Outcome::Roadmap { steps, rootless, .. } = ekko.display_roadmap().unwrap() else { panic!() };

        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].total, 1, "only the phased task belongs to the step");
        assert_eq!(rootless, 1, "the root item is counted, not hidden");

        cleanup(&dir);
    }

    #[test]
    fn the_same_area_name_in_two_phases_is_two_areas() {
        // Each phase is its own world; scoping is what keeps `@render` under
        // setup distinct from `@render` under build.
        let (ekko, dir) = fresh_ekko();
        ekko.set_phases(&words(&["setup", "build"])).unwrap();
        ekko.create_task_in(&words(&["@render", "early"]), Some("setup")).unwrap();
        ekko.create_task_in(&words(&["@render", "later"]), Some("build")).unwrap();

        let Outcome::Roadmap { steps, .. } = ekko.display_roadmap().unwrap() else { panic!() };

        assert_eq!(steps[0].total, 1);
        assert_eq!(steps[1].total, 1, "the second @render did not join the first");

        cleanup(&dir);
    }

    #[test]
    fn the_cursor_marks_the_phase_holding_work_and_nothing_when_there_is_none() {
        let (ekko, dir) = fresh_ekko();
        ekko.set_phases(&words(&["setup", "build"])).unwrap();
        ekko.create_task_in(&words(&["@a", "one"]), Some("setup")).unwrap();
        ekko.create_task_in(&words(&["@b", "two"]), Some("build")).unwrap();

        let Outcome::Roadmap { steps, .. } = ekko.display_roadmap().unwrap() else { panic!() };
        assert!(steps.iter().all(|s| !s.current), "nothing in progress means no cursor");

        ekko.set_state(&words(&["@2", "progress"]), false).unwrap();
        let Outcome::Roadmap { steps, .. } = ekko.display_roadmap().unwrap() else { panic!() };
        assert!(!steps[0].current && steps[1].current);

        cleanup(&dir);
    }

    #[test]
    fn cancelled_work_leaves_a_phase_total_rather_than_holding_it_short() {
        // Same reasoning as the percentage: cancelled work is not work, so a
        // phase that drops something can still read as finished.
        let (ekko, dir) = fresh_ekko();
        ekko.set_phases(&words(&["setup"])).unwrap();
        ekko.create_task_in(&words(&["@a", "done"]), Some("setup")).unwrap();
        ekko.create_task_in(&words(&["@a", "dropped"]), Some("setup")).unwrap();
        ekko.set_state(&words(&["@1", "done"]), false).unwrap();
        ekko.set_state(&words(&["@2", "cancelled"]), false).unwrap();

        let Outcome::Roadmap { steps, .. } = ekko.display_roadmap().unwrap() else { panic!() };

        assert_eq!((steps[0].complete, steps[0].total), (1, 1), "reads as finished");

        cleanup(&dir);
    }

    #[test]
    fn cancelling_is_terminal_and_mutually_exclusive_with_the_other_states() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["dropped"])).unwrap();
        ekko.set_state(&words(&["@1", "progress"]), false).unwrap();

        ekko.set_state(&words(&["@1", "cancelled"]), false).unwrap();

        let item = &ekko.storage.get().unwrap()[&1];
        assert_eq!(item.cancelled, Some(true));
        assert_eq!(item.in_progress, Some(false));
        assert_eq!(item.is_complete, Some(false));
        assert_eq!(item.paused, None);

        cleanup(&dir);
    }

    #[test]
    fn any_other_state_revives_a_cancelled_task() {
        let (ekko, dir) = fresh_ekko();
        for (id, revive) in [("@1", "progress"), ("@2", "done"), ("@3", "unstarted")] {
            ekko.create_task(&words(&["dropped"])).unwrap();
            ekko.set_state(&words(&[id, "cancelled"]), false).unwrap();
            ekko.set_state(&words(&[id, revive]), false).unwrap();
        }

        let data = ekko.storage.get().unwrap();
        for id in 1..=3 {
            assert_eq!(data[&id].cancelled, None, "item {id} should no longer be cancelled");
        }

        cleanup(&dir);
    }

    #[test]
    fn a_cancelled_task_is_not_pending_and_is_not_counted_in_the_percentage() {
        // Two conflations avoided at once: a dropped task is not waiting to be
        // done, and it is not unfinished work dragging the board down forever.
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["real work"])).unwrap();
        ekko.create_task(&words(&["dropped"])).unwrap();
        ekko.set_state(&words(&["@1", "done"]), false).unwrap();
        ekko.set_state(&words(&["@2", "cancelled"]), false).unwrap();

        let Outcome::List(groups) = ekko.list_by_attributes(&words(&["pending"])).unwrap() else {
            panic!()
        };
        assert!(groups.is_empty(), "the cancelled task must not show up as pending");

        let Outcome::Stats(stats) = ekko.display_stats().unwrap() else { panic!() };
        assert_eq!(stats.cancelled, 1);
        assert_eq!(stats.percent, 100, "one of one real task is done");

        cleanup(&dir);
    }

    #[test]
    fn paused_is_distinct_from_never_started() {
        // The whole point: taskbook collapsed these into one empty box.
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["set aside"])).unwrap();
        ekko.create_task(&words(&["never touched"])).unwrap();

        ekko.set_state(&words(&["@1", "progress"]), false).unwrap();
        ekko.set_state(&words(&["@1", "paused"]), false).unwrap();

        let data = ekko.storage.get().unwrap();
        assert_eq!(data[&1].paused, Some(true));
        assert_eq!(data[&1].in_progress, Some(false));
        assert_eq!(data[&2].paused, None, "an untouched task is not paused, it is unstarted");

        cleanup(&dir);
    }

    #[test]
    fn unstarted_undoes_a_progress_aimed_at_the_wrong_id() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["typo victim"])).unwrap();
        ekko.set_state(&words(&["@1", "progress"]), false).unwrap();

        ekko.set_state(&words(&["@1", "unstarted"]), false).unwrap();

        let item = &ekko.storage.get().unwrap()[&1];
        assert_eq!(item.in_progress, Some(false));
        assert_eq!(item.paused, None, "back to never-started, not paused");

        cleanup(&dir);
    }

    #[test]
    fn resuming_or_finishing_clears_the_paused_flag() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["one"])).unwrap();
        ekko.create_task(&words(&["two"])).unwrap();
        for id in ["@1", "@2"] {
            ekko.set_state(&words(&[id, "progress"]), false).unwrap();
            ekko.set_state(&words(&[id, "paused"]), false).unwrap();
        }

        ekko.set_state(&words(&["@1", "progress"]), false).unwrap();
        ekko.set_state(&words(&["@2", "done"]), false).unwrap();

        let data = ekko.storage.get().unwrap();
        assert_eq!(data[&1].paused, None, "resuming un-pauses");
        assert_eq!(data[&2].paused, None, "finishing settles it");
        assert_eq!(data[&2].is_complete, Some(true));

        cleanup(&dir);
    }

    #[test]
    fn stats_count_paused_apart_from_pending() {
        // "0 pending" while two tasks sat half-done was the original lie.
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["paused one"])).unwrap();
        ekko.create_task(&words(&["never started"])).unwrap();
        ekko.set_state(&words(&["@1", "progress"]), false).unwrap();
        ekko.set_state(&words(&["@1", "paused"]), false).unwrap();

        let Outcome::Stats(stats) = ekko.display_stats().unwrap() else { panic!() };

        assert_eq!(stats.paused, 1);
        assert_eq!(stats.pending, 1);
        assert_eq!(stats.in_progress, 0);

        cleanup(&dir);
    }

    #[test]
    fn since_reports_modified_items_not_only_newly_created_ones() {
        // The reason `updatedAt` had to exist at all: `_timestamp` is
        // creation time and never moves, so filtering on it would miss
        // every edit.
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["untouched"])).unwrap();
        ekko.create_task(&words(&["will be edited"])).unwrap();

        let mark = chrono::Local::now().timestamp_millis() + 1;
        std::thread::sleep(std::time::Duration::from_millis(5));
        ekko.set_state(&words(&["@2", "done"]), false).unwrap();

        let Outcome::Board(groups) = ekko.display_since(mark).unwrap() else { panic!() };
        let ids: Vec<u32> = groups.iter().flat_map(|(_, i)| i.iter().map(|x| x.id)).collect();

        assert_eq!(ids, vec![2], "only the item that actually changed");

        cleanup(&dir);
    }

    #[test]
    fn since_zero_returns_everything_including_items_with_no_updated_at() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["current"])).unwrap();
        // A legacy item: no `updatedAt`, as taskbook would have written it.
        let raw = r#"{"1":{"_id":1,"_date":"Mon Aug 24 2026","_timestamp":1787600000000,"description":"legacy","isStarred":false,"boards":["@old"],"_isTask":true,"isComplete":false,"inProgress":false,"priority":1}}"#;
        fs::write(dir.join("storage").join("storage.json"), raw).unwrap();

        let Outcome::Board(groups) = ekko.display_since(0).unwrap() else { panic!() };
        let ids: Vec<u32> = groups.iter().flat_map(|(_, i)| i.iter().map(|x| x.id)).collect();

        assert_eq!(ids, vec![1], "falls back to creation time rather than vanishing");

        cleanup(&dir);
    }

    #[test]
    fn a_write_that_changes_nothing_does_not_bump_updated_at() {
        // `save_touching` diffs rather than stamping blindly, so a command
        // that turns out to be a no-op must not make an item look modified.
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["a task"])).unwrap();
        ekko.set_state(&words(&["@1", "done"]), false).unwrap();
        let after_first = ekko.storage.get().unwrap()[&1].updated_at;

        std::thread::sleep(std::time::Duration::from_millis(5));
        ekko.set_state(&words(&["@1", "done"]), false).unwrap(); // idempotent: no change

        assert_eq!(ekko.storage.get().unwrap()[&1].updated_at, after_first);

        cleanup(&dir);
    }

    #[test]
    fn a_new_item_never_takes_the_id_of_one_that_left_storage() {
        // Ids used to be `max + 1`, so an item leaving storage and another
        // being created handed the new one the same id -- exactly the case a
        // caller holding an id across time got wrong. Storage now remembers
        // the highest id it has held, and the uid still tells two items apart
        // wherever they meet. `--clear` rather than `--delete`, because
        // deleting trashes, and a trashed item keeps its number anyway.
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["first"])).unwrap();
        ekko.create_task(&words(&["second"])).unwrap();
        let old_uid = ekko.storage.get().unwrap()[&2].uid.clone();

        ekko.set_state(&words(&["@2", "done"]), false).unwrap();
        ekko.clear().unwrap();
        ekko.create_task(&words(&["created after the clear"])).unwrap();

        let data = ekko.storage.get().unwrap();
        assert!(!data.contains_key(&2), "the cleared item's id came back");
        assert_eq!(data[&3].description, "created after the clear");
        assert!(old_uid.is_some());
        assert_ne!(data[&3].uid, old_uid, "a different item, a different uid");

        cleanup(&dir);
    }

    #[test]
    fn uid_survives_the_fresh_id_a_restore_hands_out() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["archived then restored"])).unwrap();
        let before = ekko.storage.get().unwrap()[&1].uid.clone();

        ekko.check_tasks(&words(&["1"]), false).unwrap();
        ekko.clear().unwrap();
        ekko.restore_items(&words(&["1"])).unwrap();

        let after = ekko.storage.get().unwrap().values().next().unwrap().uid.clone();
        assert_eq!(after, before, "the uid is what stays put when the id does not");

        cleanup(&dir);
    }

    #[test]
    fn items_without_a_uid_load_and_are_not_backfilled() {
        // What taskbook wrote, and what Ekko wrote before uids existed.
        // Backfilling would rewrite files that are otherwise untouched.
        let (ekko, dir) = fresh_ekko();
        let raw = r#"{"1":{"_id":1,"_date":"Mon Aug 24 2026","_timestamp":1787600000000,"description":"legacy","isStarred":false,"boards":["@old"],"_isTask":true,"isComplete":false,"inProgress":false,"priority":1}}"#;
        fs::write(dir.join("storage").join("storage.json"), raw).unwrap();

        let data = ekko.storage.get().unwrap();
        assert_eq!(data[&1].uid, None, "absent means legacy, not unknown");

        ekko.star_items(&words(&["1"])).unwrap();
        let written = fs::read_to_string(dir.join("storage").join("storage.json")).unwrap();
        assert!(!written.contains("uid"), "a write must not invent one either");

        cleanup(&dir);
    }

    #[test]
    fn set_is_idempotent_where_the_toggles_are_not() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["a task"])).unwrap();

        // The whole point: a retried command must not undo itself.
        ekko.set_state(&words(&["@1", "done"]), false).unwrap();
        ekko.set_state(&words(&["@1", "done"]), false).unwrap();
        assert_eq!(ekko.storage.get().unwrap()[&1].is_complete, Some(true));

        // Contrast, on the same data: the toggle flips back.
        ekko.check_tasks(&words(&["1"]), false).unwrap();
        assert_eq!(ekko.storage.get().unwrap()[&1].is_complete, Some(false));

        cleanup(&dir);
    }

    #[test]
    fn set_applies_several_states_to_several_items_at_once() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["one"])).unwrap();
        ekko.create_task(&words(&["two"])).unwrap();

        ekko.set_state(&words(&["@1", "@2", "progress", "starred"]), false).unwrap();

        let data = ekko.storage.get().unwrap();
        for id in [1, 2] {
            assert_eq!(data[&id].in_progress, Some(true), "item {id}");
            assert!(data[&id].is_starred, "item {id}");
            assert_eq!(data[&id].is_complete, Some(false), "starting work un-completes it");
        }

        cleanup(&dir);
    }

    #[test]
    fn set_skips_task_only_states_on_notes_but_still_stars_them() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_note(&words(&["a note"])).unwrap();

        ekko.set_state(&words(&["@1", "done", "starred"]), false).unwrap();

        let data = ekko.storage.get().unwrap();
        assert_eq!(data[&1].is_complete, None, "a note never gains task fields");
        assert!(data[&1].is_starred, "starring works on notes, matching --star");

        cleanup(&dir);
    }

    #[test]
    fn set_rejects_missing_or_unknown_states_rather_than_doing_nothing() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["a task"])).unwrap();

        assert!(matches!(ekko.set_state(&words(&["@1"]), false), Err(EkkoError::MissingState)));
        assert!(matches!(
            ekko.set_state(&words(&["@1", "finished"]), false),
            Err(EkkoError::UnknownState(ref t)) if t == "finished"
        ));
        assert_eq!(ekko.storage.get().unwrap()[&1].is_complete, Some(false), "nothing applied");

        cleanup(&dir);
    }

    #[test]
    fn due_dates_are_parsed_canonicalised_and_kept_off_notes() {
        let (ekko, dir) = fresh_ekko();
        // Deliberately non-canonical: single-digit month and day.
        ekko.create_task(&words(&["with a deadline", "d:2026-9-1"])).unwrap();
        ekko.create_note(&words(&["a note", "d:2026-09-01"])).unwrap();

        let data = ekko.storage.get().unwrap();
        assert_eq!(data[&1].due_date.as_deref(), Some("2026-09-01"), "stored form should be canonical");
        assert_eq!(data[&2].due_date, None, "notes carry no deadline, same as they carry no priority");

        cleanup(&dir);
    }

    #[test]
    fn a_malformed_due_date_is_rejected_rather_than_silently_dropped() {
        let (ekko, dir) = fresh_ekko();

        let result = ekko.create_task(&words(&["tomorrow please", "d:tomorrow"]));

        assert!(matches!(result, Err(EkkoError::InvalidDueDate(ref t)) if t == "d:tomorrow"));
        assert!(ekko.storage.get().unwrap().is_empty(), "nothing should have been created");

        cleanup(&dir);
    }

    #[test]
    fn overdue_excludes_completed_tasks_and_future_deadlines() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["late", "d:2000-01-01"])).unwrap();
        ekko.create_task(&words(&["also late but done", "d:2000-01-01"])).unwrap();
        ekko.create_task(&words(&["ages away", "d:3000-01-01"])).unwrap();
        ekko.create_task(&words(&["no deadline at all"])).unwrap();
        ekko.check_tasks(&words(&["2"]), false).unwrap();

        let Outcome::List(groups) = ekko.list_by_attributes(&words(&["overdue"])).unwrap() else {
            panic!()
        };
        let ids: Vec<u32> = groups.iter().flat_map(|(_, i)| i.iter().map(|x| x.id)).collect();

        assert_eq!(ids, vec![1], "only the open, past-due task counts as overdue");

        cleanup(&dir);
    }

    #[test]
    fn list_accepts_a_board_name_in_the_at_form_the_board_view_prints() {
        // The JS version only accepted the bare name, and quietly listed
        // every board when given the `@name` form it had just printed --
        // a filter that looks like it worked and did not.
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["@alpha", "in alpha"])).unwrap();
        ekko.create_task(&words(&["@beta", "in beta"])).unwrap();

        for term in ["alpha", "@alpha"] {
            let Outcome::List(groups) = ekko.list_by_attributes(&words(&[term])).unwrap() else {
                panic!()
            };
            let boards: Vec<&str> = groups.iter().map(|(board, _)| board.as_str()).collect();
            assert_eq!(boards, vec!["@alpha"], "--list {term} should list only @alpha");
        }

        cleanup(&dir);
    }

    #[test]
    fn list_rejects_a_term_that_is_neither_a_board_nor_an_attribute() {
        // Silently returning everything is the worst possible answer here:
        // it is indistinguishable from a filter that matched all items.
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["@alpha", "in alpha"])).unwrap();

        let result = ekko.list_by_attributes(&words(&["nonexistent"]));

        assert!(matches!(result, Err(EkkoError::UnknownListTerm(ref t)) if t == "nonexistent"));

        cleanup(&dir);
    }

    #[test]
    fn find_boards_order_comes_from_the_full_dataset_not_just_the_matches() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["@a", "alpha task, unrelated to the search"])).unwrap();
        ekko.create_task(&words(&["@b", "matches search term xyz"])).unwrap();

        let Outcome::Find(groups) = ekko.find_items(&words(&["xyz"])).unwrap() else { panic!() };

        // Only @b's item matches, so only @b should appear -- but board
        // *order* still follows full-dataset discovery order (@a before
        // @b), matching the JS version's `_groupByBoard(result)` using the
        // default `boards` parameter (which is `_getBoards()` over the
        // *full* `_data`, not the filtered `result`).
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].0, "@b");

        cleanup(&dir);
    }

    #[test]
    fn copy_to_clipboard_gathers_descriptions_and_hands_them_to_the_injected_writer() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["first"])).unwrap();
        ekko.create_task(&words(&["second"])).unwrap();

        let mut written = None;
        let outcome =
            ekko.copy_to_clipboard(&words(&["1", "2"]), |text| {
                written = Some(text.to_string());
                Ok(())
            }).unwrap();

        assert_eq!(written, Some("first\nsecond".to_string()));
        assert!(matches!(outcome, Outcome::Copy { .. }));

        cleanup(&dir);
    }

    #[test]
    fn validation_errors_carry_the_right_codes() {
        let (ekko, dir) = fresh_ekko();

        assert_eq!(ekko.check_tasks(&[], false).unwrap_err().code(), "MISSING_ID");
        assert_eq!(ekko.check_tasks(&words(&["999"]), false).unwrap_err().code(), "INVALID_ID");
        assert_eq!(ekko.create_task(&[]).unwrap_err().code(), "MISSING_DESC");
        assert_eq!(ekko.edit_description(&words(&["@1", "@2", "x"])).unwrap_err().code(), "INVALID_IDS_NUMBER");
        assert_eq!(ekko.move_boards(&words(&["@1"])).unwrap_err().code(), "INVALID_ID"); // no item 1 exists yet -> caught before boards are even checked

        cleanup(&dir);
    }

    /// Every state `--set` accepts has to say so. `cancelled` and
    /// `unstarted` shipped mute: they reached `apply_state` but not the
    /// match that renders the confirmation, so the write landed and the
    /// terminal stayed silent -- indistinguishable from a failure, and
    /// worst on `unstarted`, whose whole job is undoing a `--set` aimed at
    /// the wrong id. Drives the real `set_state` rather than building an
    /// `Outcome` by hand, so an arm can never be reachable only in a test.
    #[test]
    fn every_settable_state_reports_what_it_did() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["a task to move through every state"])).unwrap();

        // Every canonical state `canonical_state` can return.
        let states = [
            "done",
            "undone",
            "progress",
            "paused",
            "cancelled",
            "unstarted",
            "starred",
            "unstarred",
        ];

        for state in states {
            let outcome = ekko.set_state(&words(&["@1", state]), false).unwrap();

            let mut buffer: Vec<u8> = Vec::new();
            {
                let mut renderer =
                    Renderer::new(Painter::forced(false), Config::default(), &mut buffer);
                outcome.render(&mut renderer);
            }
            let rendered = String::from_utf8(buffer).unwrap();

            assert!(
                rendered.contains('1'),
                "--set @1 {state} rendered no confirmation naming the id: {rendered:?}"
            );
        }

        cleanup(&dir);
    }

    /// Shipped with no way to unset: passing no blockers errored, which
    /// left the `None` branch in `set_blocked_by` unreachable and a wrongly
    /// blocked task with no way back but delete and recreate. Real use hit
    /// it before we did -- an agent wrote "this dependency is wrong and
    /// ekko will not let me clear it" into a task description, so the board
    /// ended up carrying a false statement as if it were data.
    #[test]
    fn a_dependency_can_be_cleared() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["first"])).unwrap();
        ekko.create_task(&words(&["second"])).unwrap();

        ekko.set_blocked_by(&words(&["@2", "1"])).unwrap();
        assert_eq!(ekko.blocker_map().unwrap().get(&2), Some(&vec![1]));

        let Outcome::Blocked { blockers, .. } = ekko.set_blocked_by(&words(&["@2"])).unwrap()
        else {
            panic!("expected a Blocked outcome")
        };

        assert!(blockers.is_empty(), "clearing reported blockers: {blockers:?}");
        assert_eq!(ekko.blocker_map().unwrap().get(&2), None, "the marker survived");

        cleanup(&dir);
    }

    /// Display ids are recycled, so anything holding a reference across
    /// time is told to hold the uid instead -- advice that was impossible
    /// to follow while every mutation took display ids only. An agent could
    /// carry a uid and then had nothing to do with it but re-read the board
    /// to translate it back.
    #[test]
    fn a_uid_works_wherever_a_display_id_does() {
        let (ekko, dir) = fresh_ekko();
        let Outcome::Task(item) = ekko.create_task(&words(&["carry me"])).unwrap() else {
            panic!("expected a Task outcome")
        };
        let uid = item.uid.clone().expect("a created task carries a uid");
        let marked = format!("@{uid}");

        ekko.set_state(&[marked.clone(), "done".to_string()], false).unwrap();
        ekko.update_priority(&[marked.clone(), "2".to_string()]).unwrap();
        ekko.edit_description(&[marked, "renamed by uid".to_string()]).unwrap();
        // A toggle takes the id bare, with no `@`, so it exercises the
        // other spelling on the same value.
        ekko.star_items(std::slice::from_ref(&uid)).unwrap();

        let stored = ekko.display_by_board().unwrap();
        let Outcome::Board(groups) = stored else { panic!("expected a Board outcome") };
        let item = &groups[0].1[0];

        assert!(item.is_complete.unwrap_or(false), "--set by uid did not land");
        assert_eq!(item.priority, Some(2), "--priority by uid did not land");
        assert_eq!(item.description, "renamed by uid", "--edit by uid did not land");
        assert!(item.is_starred, "--star by a bare uid did not land");

        cleanup(&dir);
    }

    /// A uid can never be mistaken for a display id: it is
    /// `{nanos:x}-{pid:x}`, so it always carries a hyphen and never parses
    /// as a u32. Both spellings miss the same way, because a caller
    /// branching on the error code should not have to care which it used.
    #[test]
    fn an_unknown_uid_misses_the_same_way_an_unknown_id_does() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["only one"])).unwrap();

        let by_uid = ekko.set_state(&words(&["@18cfdfefd310bd35-10468b", "done"]), false);
        let by_id = ekko.set_state(&words(&["@999", "done"]), false);

        assert!(matches!(by_uid, Err(EkkoError::InvalidId(_))));
        assert!(matches!(by_id, Err(EkkoError::InvalidId(_))));

        cleanup(&dir);
    }

    /// A reason and the work it explains were siblings, so a long note
    /// about item 2 either got crammed into 2's description or floated
    /// beside it with nothing connecting them. Attaching moves it under
    /// its task and indents it, which is the whole feature.
    #[test]
    fn an_attached_note_sits_under_the_task_it_explains() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["@a", "first"])).unwrap();
        ekko.create_task(&words(&["@a", "second"])).unwrap();
        ekko.create_note(&words(&["@a", "why second is hard"])).unwrap();
        ekko.create_task(&words(&["@a", "third"])).unwrap();

        ekko.set_attached_to(&words(&["@3", "1"])).unwrap();

        let Outcome::Board(groups) = ekko.display_by_board().unwrap() else { panic!() };
        let order: Vec<u32> = groups[0].1.iter().map(|item| item.id).collect();

        assert_eq!(order, vec![1, 3, 2, 4], "the note did not move under task 1");

        cleanup(&dir);
    }

    /// Designed in from the start rather than discovered missing:
    /// `--blocked-by` shipped with no way to unset and left a wrong
    /// dependency with no way back.
    #[test]
    fn a_note_can_be_detached() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["@a", "first"])).unwrap();
        ekko.create_note(&words(&["@a", "why"])).unwrap();

        ekko.set_attached_to(&words(&["@2", "1"])).unwrap();
        let Outcome::Attached { target, .. } = ekko.set_attached_to(&words(&["@2"])).unwrap() else {
            panic!("expected an Attached outcome")
        };

        assert_eq!(target, None);
        let Outcome::Board(groups) = ekko.display_by_board().unwrap() else { panic!() };
        assert!(groups[0].1.iter().all(|item| item.attached_to.is_none()), "the attachment survived");

        cleanup(&dir);
    }

    /// One level, always. A task under a task is a subtask, with real
    /// questions about whose total it counts toward; a note under a note
    /// would allow chains and therefore cycles. Both are refused by shape
    /// rather than by a check that could be forgotten later.
    #[test]
    fn attaching_is_a_note_pointing_at_a_task_and_nothing_else() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["@a", "a task"])).unwrap();
        ekko.create_note(&words(&["@a", "a note"])).unwrap();
        ekko.create_note(&words(&["@a", "another note"])).unwrap();

        assert!(matches!(
            ekko.set_attached_to(&words(&["@1", "2"])),
            Err(EkkoError::AttachNotANote(1))
        ));
        assert!(matches!(
            ekko.set_attached_to(&words(&["@2", "3"])),
            Err(EkkoError::AttachTargetNotATask(3))
        ));

        cleanup(&dir);
    }

    /// A note whose task lives on another board stays where it was filed
    /// rather than jumping boards. Surprising placement is worse than an
    /// un-nested reason, and the note is still findable where it was put.
    #[test]
    fn a_note_whose_task_is_elsewhere_stays_put() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["@here", "the task"])).unwrap();
        ekko.create_note(&words(&["@there", "the reason"])).unwrap();
        ekko.create_note(&words(&["@there", "another"])).unwrap();

        ekko.set_attached_to(&words(&["@2", "1"])).unwrap();

        let Outcome::Board(groups) = ekko.display_by_board().unwrap() else { panic!() };
        let there = groups.iter().find(|(b, _)| b == "@there").expect("@there exists");
        let order: Vec<u32> = there.1.iter().map(|item| item.id).collect();

        assert_eq!(order, vec![3, 2], "the orphan is appended, in id order, never dropped");

        cleanup(&dir);
    }

    /// Stashing hides, it does not change. A stashed done task is still
    /// done underneath and comes back done -- which is why this is its own
    /// field and not another state: a state would overwrite what it found,
    /// and unstashing would then have to guess.
    #[test]
    fn stashing_hides_an_item_without_changing_what_it_is() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["done and put away"])).unwrap();
        ekko.set_state(&words(&["@1", "done"]), false).unwrap();

        ekko.set_stashed(&words(&["1"]), true).unwrap();

        let Outcome::Board(groups) = ekko.display_by_board().unwrap() else { panic!() };
        assert!(groups.is_empty(), "a stashed item is still on the board");

        let Outcome::Stash(stashed) = ekko.display_stash().unwrap() else { panic!() };
        assert!(stashed[0].1[0].is_complete.unwrap_or(false), "it stopped being done");

        ekko.set_stashed(&words(&["1"]), false).unwrap();
        let Outcome::Board(groups) = ekko.display_by_board().unwrap() else { panic!() };
        assert!(groups[0].1[0].is_complete.unwrap_or(false), "it came back as something else");

        cleanup(&dir);
    }

    /// The counts have to be disjoint or the line stops summing to the
    /// board. A stashed done task belongs under `in-stash` and nowhere
    /// else -- it is not in front of you, and the line answers what is.
    #[test]
    fn the_stats_line_counts_every_item_exactly_once() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["done and stashed"])).unwrap();
        ekko.create_task(&words(&["plain pending"])).unwrap();
        ekko.create_note(&words(&["a note, trashed"])).unwrap();
        ekko.set_state(&words(&["@1", "done"]), false).unwrap();
        ekko.set_stashed(&words(&["1"]), true).unwrap();
        ekko.set_trashed(&words(&["3"]), true).unwrap();

        let Outcome::Stats(stats) = ekko.display_stats().unwrap() else { panic!() };

        assert_eq!(stats.complete, 0, "the stashed one is still counted as done");
        assert_eq!(stats.notes, 0, "the trashed one is still counted as a note");
        assert_eq!((stats.stashed, stats.trashed, stats.pending), (1, 1, 1));

        cleanup(&dir);
    }

    /// A board argument stashes what is on it now. It does not close the
    /// board: something created there tomorrow shows up normally, which is
    /// a different feature and the one item 31 wants for phases.
    #[test]
    fn stashing_a_board_takes_what_is_on_it_and_does_not_close_it() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["@due", "first"])).unwrap();
        ekko.create_note(&words(&["@due", "the reason"])).unwrap();
        ekko.create_task(&words(&["@other", "untouched"])).unwrap();

        ekko.set_stashed(&words(&["@due"]), true).unwrap();
        ekko.create_task(&words(&["@due", "created after"])).unwrap();

        let Outcome::Board(groups) = ekko.display_by_board().unwrap() else { panic!() };
        let due = groups.iter().find(|(b, _)| b == "@due").expect("@due is back");

        assert_eq!(due.1.len(), 1, "the new item was born hidden");
        assert_eq!(due.1[0].description, "created after");

        cleanup(&dir);
    }

    /// `--clear` sweeps the board, and something stashed is not on it.
    /// Archiving it would undo the stash and change its id on the way
    /// back, which is the opposite of what putting it away was for.
    #[test]
    fn clearing_does_not_reach_into_the_stash() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["stashed and done"])).unwrap();
        ekko.create_task(&words(&["visible and done"])).unwrap();
        ekko.set_state(&words(&["@1", "@2", "done"]), false).unwrap();
        ekko.set_stashed(&words(&["1"]), true).unwrap();

        ekko.clear().unwrap();

        let Outcome::Stash(stashed) = ekko.display_stash().unwrap() else { panic!() };
        assert_eq!(stashed[0].1.len(), 1, "clear swept the stash");
        assert_eq!(stashed[0].1[0].description, "stashed and done");

        cleanup(&dir);
    }

    /// Removing goes to the trash, not the archive. The archive is the
    /// record of what got done; a task deleted by mistake sitting in it is
    /// noise in the one history worth trusting.
    #[test]
    fn deleting_trashes_rather_than_archiving() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["a mistake"])).unwrap();

        ekko.delete_items(&words(&["1"])).unwrap();

        let Outcome::Archive(archived) = ekko.display_archive().unwrap() else { panic!() };
        assert!(archived.is_empty(), "a deletion landed in the archive");

        let Outcome::Trash(trashed) = ekko.display_trash().unwrap() else { panic!() };
        assert_eq!(trashed.len(), 1);
        assert!(trashed[0].trashed.is_some(), "no timestamp, so nothing can expire");

        cleanup(&dir);
    }

    /// Ekko has no daemon, so expiry rides along on a write -- and only on
    /// a write. A read that changed the board would make `--json` unsafe to
    /// call, which is the rule `list_projects` already follows.
    #[test]
    fn the_trash_expires_on_a_write_and_never_on_a_read() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["long gone"])).unwrap();
        ekko.delete_items(&words(&["1"])).unwrap();

        // Backdate past the retention window.
        let mut data = ekko.storage.get().unwrap();
        let old = chrono::Local::now().timestamp_millis() - (TRASH_DAYS + 1) * 86_400_000;
        data.get_mut(&1).unwrap().trashed = Some(old);
        ekko.storage.set(&data).unwrap();

        ekko.display_by_board().unwrap();
        ekko.display_trash().unwrap();
        assert!(ekko.storage.get().unwrap().contains_key(&1), "a read swept the trash");

        ekko.create_task(&words(&["anything at all"])).unwrap();
        assert!(!ekko.storage.get().unwrap().contains_key(&1), "a write did not sweep it");

        cleanup(&dir);
    }

    /// A task blocked by something still open cannot be completed. The
    /// dependency used to be advice only: `--list ready` left the task out
    /// and `--check` closed it anyway, leaving a board that showed a tick
    /// beside the marker naming what the task was still waiting for.
    #[test]
    fn a_blocked_task_cannot_be_completed_and_nothing_is_written() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["blocker"])).unwrap();
        ekko.create_task(&words(&["blocked"])).unwrap();
        ekko.set_blocked_by(&words(&["@2", "1"])).unwrap();

        let by_check = ekko.check_tasks(&words(&["2"]), false);
        let by_set = ekko.set_state(&words(&["@2", "done"]), false);

        for result in [by_check, by_set] {
            let Err(EkkoError::Blocked(blocked)) = &result else {
                panic!("expected BLOCKED, got {result:?}")
            };
            assert_eq!(blocked, &vec![(2, vec![1])]);
            assert_eq!(result.unwrap_err().code(), "BLOCKED");
        }
        assert!(!ekko.storage.get().unwrap()[&2].is_complete.unwrap_or(false), "it closed anyway");

        cleanup(&dir);
    }

    /// `--force` completes it anyway and says what it pushed past. The
    /// dependency stays recorded, so the board keeps naming what the task
    /// was waiting on for as long as that stays open: the override leaves a
    /// trace instead of erasing the reason it was needed.
    #[test]
    fn force_completes_a_blocked_task_and_reports_what_it_overrode() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["blocker"])).unwrap();
        ekko.create_task(&words(&["blocked"])).unwrap();
        ekko.set_blocked_by(&words(&["@2", "1"])).unwrap();

        let Outcome::Check { checked, overridden, .. } =
            ekko.check_tasks(&words(&["2"]), true).unwrap()
        else {
            panic!("expected a Check outcome")
        };

        assert_eq!(checked, vec![2]);
        assert_eq!(overridden, vec![(2, vec![1])]);
        assert!(ekko.storage.get().unwrap()[&2].is_complete.unwrap_or(false));
        assert_eq!(ekko.blocker_map().unwrap().get(&2), Some(&vec![1]), "the trace was erased");

        cleanup(&dir);
    }

    /// One blocked task stops the whole command, the way one invalid id
    /// already does. Completing the rest and refusing one would leave the
    /// caller to work out which half landed.
    #[test]
    fn one_blocked_task_stops_the_whole_command() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["free"])).unwrap();
        ekko.create_task(&words(&["blocker"])).unwrap();
        ekko.create_task(&words(&["blocked"])).unwrap();
        ekko.set_blocked_by(&words(&["@3", "2"])).unwrap();

        let result = ekko.set_state(&words(&["@1", "@3", "done"]), false);

        assert!(matches!(result, Err(EkkoError::Blocked(_))), "{result:?}");
        assert!(!ekko.storage.get().unwrap()[&1].is_complete.unwrap_or(false), "half the command landed");

        cleanup(&dir);
    }

    /// Only completing is refused. Starting, pausing, cancelling and
    /// reopening claim nothing about the work being finished. And a task
    /// already done stays retry-safe: `--set done` again after a forced
    /// completion is a no-op, not a fresh refusal.
    #[test]
    fn only_completing_is_refused() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["blocker"])).unwrap();
        ekko.create_task(&words(&["blocked"])).unwrap();
        ekko.set_blocked_by(&words(&["@2", "1"])).unwrap();

        for state in ["progress", "paused", "cancelled", "unstarted"] {
            ekko.set_state(&words(&["@2", state]), false)
                .unwrap_or_else(|e| panic!("--set {state} was refused: {e}"));
        }

        ekko.set_state(&words(&["@2", "done"]), true).unwrap();
        ekko.set_state(&words(&["@2", "done"]), false).expect("a retry after --force was refused");
        ekko.check_tasks(&words(&["2"]), false).expect("reopening was refused");

        cleanup(&dir);
    }

    /// What `--force` overrode is said on the same line as the completion,
    /// because it is part of what happened to that task.
    #[test]
    fn a_forced_completion_says_so() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["blocker"])).unwrap();
        ekko.create_task(&words(&["blocked"])).unwrap();
        ekko.set_blocked_by(&words(&["@2", "1"])).unwrap();

        let outcome = ekko.set_state(&words(&["@2", "done"]), true).unwrap();
        let mut buffer: Vec<u8> = Vec::new();
        {
            let mut renderer = Renderer::new(Painter::forced(false), Config::default(), &mut buffer);
            outcome.render(&mut renderer);
        }
        let rendered = String::from_utf8(buffer).unwrap();

        assert!(rendered.contains("Checked task: 2 (blockers overridden: 1)"), "{rendered:?}");

        cleanup(&dir);
    }

    /// Ids a `--list <term>` returns, sorted and without the repeats an item
    /// on several boards would add.
    fn listed(ekko: &Ekko, term: &str) -> Vec<u32> {
        let Outcome::List(groups) = ekko.list_by_attributes(&words(&[term])).unwrap() else {
            panic!("expected a List outcome")
        };
        let mut ids: Vec<u32> =
            groups.iter().flat_map(|(_, items)| items.iter().map(|item| item.id)).collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    /// The case the rigor audit measured: cancelled, then --check. The flags
    /// ended up both done and cancelled, so the board drew the task cancelled,
    /// the stats counted it cancelled and --list done listed it as done. The
    /// check is now a transition like any other, and every surface agrees.
    #[test]
    fn cancelled_then_checked_is_done_on_every_surface() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["dropped, then checked"])).unwrap();
        ekko.set_state(&words(&["@1", "cancelled"]), false).unwrap();

        ekko.check_tasks(&words(&["1"]), false).unwrap();

        let item = &ekko.storage.get().unwrap()[&1];
        assert_eq!(item.cancelled, None, "the cancellation survived the check");
        assert!(matches!(crate::render::Level::of(item), crate::render::Level::Success));
        let Outcome::Stats(stats) = ekko.display_stats().unwrap() else { panic!() };
        assert_eq!((stats.complete, stats.cancelled), (1, 0));
        assert_eq!(listed(&ekko, "done"), vec![1]);
        assert!(listed(&ekko, "cancelled").is_empty());

        cleanup(&dir);
    }

    /// All sixteen flag combinations written straight into storage, as old
    /// data or a hand edit could leave them. The board's icon, the stats line
    /// and every state filter must agree on each one -- including paused,
    /// which --set accepted and --list used to reject.
    #[test]
    fn every_surface_agrees_on_every_flag_combination() {
        let (ekko, dir) = fresh_ekko();
        for _ in 0..16 {
            ekko.create_task(&words(&["combination"])).unwrap();
        }
        let mut data = ekko.storage.get().unwrap();
        for bits in 0u8..16 {
            let item = data.get_mut(&(u32::from(bits) + 1)).unwrap();
            item.is_complete = Some(bits & 1 != 0);
            item.in_progress = Some(bits & 2 != 0);
            item.paused = (bits & 4 != 0).then_some(true);
            item.cancelled = (bits & 8 != 0).then_some(true);
        }
        ekko.storage.set(&data).unwrap();

        let data = ekko.storage.get().unwrap();
        let with = |wanted: State| -> Vec<u32> {
            data.values().filter(|item| State::of(item) == Some(wanted)).map(|item| item.id).collect()
        };

        assert_eq!(listed(&ekko, "done"), with(State::Done));
        assert_eq!(listed(&ekko, "progress"), with(State::Progress));
        assert_eq!(listed(&ekko, "paused"), with(State::Paused));
        assert_eq!(listed(&ekko, "cancelled"), with(State::Cancelled));
        let mut open: Vec<u32> =
            [State::Pending, State::Progress, State::Paused].into_iter().flat_map(&with).collect();
        open.sort_unstable();
        assert_eq!(listed(&ekko, "pending"), open);

        let Outcome::Stats(stats) = ekko.display_stats().unwrap() else { panic!() };
        let count = |state: State| with(state).len() as u32;
        assert_eq!(
            (stats.complete, stats.in_progress, stats.paused, stats.cancelled, stats.pending),
            (count(State::Done), count(State::Progress), count(State::Paused), count(State::Cancelled), count(State::Pending))
        );

        for item in data.values() {
            use crate::render::Level;
            let level = Level::of(item);
            let agrees = match State::of(item).expect("every item here is a task") {
                State::Done => matches!(level, Level::Success),
                State::Progress => matches!(level, Level::Wait),
                State::Paused => matches!(level, Level::Paused),
                State::Cancelled => matches!(level, Level::Cancelled),
                State::Pending => matches!(level, Level::Pending),
            };
            assert!(agrees, "item {} is drawn as a different state than it has", item.id);
        }

        cleanup(&dir);
    }

    /// `--projects` counts what the project's own stats line counts: stashed
    /// and trashed items are away, and a cancelled task is not in the total.
    /// It used to count everything, and showed winwayland at [46/54] while
    /// the project itself said 88%.
    #[test]
    fn the_project_listing_counts_the_way_the_project_does() {
        let home = std::env::temp_dir().join(format!(
            "ekko-core-listing-{}-{}",
            process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
        let folder = home.join("work").join("p");
        fs::create_dir_all(&folder).unwrap();
        crate::project::init(&home, &folder, None, None, 0).unwrap();
        let storage = folder.join(".ekko").join("storage");
        fs::create_dir_all(&storage).unwrap();

        let mut done = Item::new_task(1, "done".into(), vec![], 1);
        State::Done.write(&mut done);
        let mut dropped = Item::new_task(2, "dropped".into(), vec![], 1);
        State::Cancelled.write(&mut dropped);
        let mut away = Item::new_task(3, "stashed".into(), vec![], 1);
        away.stashed = Some(1);
        let mut gone = Item::new_task(4, "trashed".into(), vec![], 1);
        gone.trashed = Some(1);
        let why = Item::new_note(5, "why".into(), vec![]);
        let items: ItemMap = [done, dropped, away, gone, why].into_iter().map(|i| (i.id, i)).collect();
        fs::write(storage.join("storage.json"), serde_json::to_string(&items).unwrap()).unwrap();

        let listing = crate::project::list(&home);
        assert_eq!((listing[0].complete, listing[0].tasks, listing[0].notes), (1, 1, 1));

        fs::remove_dir_all(&home).ok();
    }

    /// The completion rule from the other side: a blocker that completed work
    /// rests on cannot be reopened, whichever command tries -- the check
    /// toggle, --set undone, --set progress or --begin -- and nothing is
    /// written.
    #[test]
    fn a_blocker_that_completed_work_depends_on_cannot_be_reopened() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["blocker"])).unwrap();
        ekko.create_task(&words(&["dependent"])).unwrap();
        ekko.set_blocked_by(&words(&["@2", "1"])).unwrap();
        ekko.set_state(&words(&["@1", "done"]), false).unwrap();
        ekko.set_state(&words(&["@2", "done"]), false).unwrap();

        let attempts = [
            ekko.check_tasks(&words(&["1"]), false),
            ekko.set_state(&words(&["@1", "undone"]), false),
            ekko.set_state(&words(&["@1", "progress"]), false),
            ekko.begin_tasks(&words(&["1"])),
        ];
        for result in &attempts {
            let Err(EkkoError::CompletedDependents(found)) = result else {
                panic!("expected COMPLETED_DEPENDENTS, got {result:?}")
            };
            assert_eq!(found, &vec![(1, vec![2])]);
        }
        assert_eq!(State::of(&ekko.storage.get().unwrap()[&1]), Some(State::Done), "it reopened anyway");

        cleanup(&dir);
    }

    /// Reviving a cancelled blocker is a reopening too: cancelled did not hold
    /// its dependents up, so they could complete, and open again it would.
    /// Cancelling a done blocker is not one -- it stays closed.
    #[test]
    fn reviving_a_cancelled_blocker_counts_and_cancelling_a_done_one_does_not() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["blocker"])).unwrap();
        ekko.create_task(&words(&["dependent"])).unwrap();
        ekko.set_blocked_by(&words(&["@2", "1"])).unwrap();
        ekko.set_state(&words(&["@1", "done"]), false).unwrap();
        ekko.set_state(&words(&["@2", "done"]), false).unwrap();

        ekko.set_state(&words(&["@1", "cancelled"]), false).expect("cancelling a done blocker was refused");
        assert!(matches!(
            ekko.set_state(&words(&["@1", "unstarted"]), false),
            Err(EkkoError::CompletedDependents(_))
        ));

        cleanup(&dir);
    }

    /// --force reopens anyway and says what it pushed past, and the dependents
    /// stay completed. A blocker whose dependents are still open reopens
    /// freely, because live evaluation simply blocks them again.
    #[test]
    fn force_reopens_and_open_dependents_never_stop_a_reopening() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["blocker"])).unwrap();
        ekko.create_task(&words(&["completed dependent"])).unwrap();
        ekko.create_task(&words(&["another blocker"])).unwrap();
        ekko.create_task(&words(&["open dependent"])).unwrap();
        ekko.set_blocked_by(&words(&["@2", "1"])).unwrap();
        ekko.set_blocked_by(&words(&["@4", "3"])).unwrap();
        ekko.set_state(&words(&["@1", "@3", "done"]), false).unwrap();
        ekko.set_state(&words(&["@2", "done"]), false).unwrap();

        let Outcome::Set { reopened, .. } = ekko.set_state(&words(&["@1", "undone"]), true).unwrap() else {
            panic!("expected a Set outcome")
        };
        assert_eq!(reopened, vec![(1, vec![2])]);
        assert_eq!(State::of(&ekko.storage.get().unwrap()[&2]), Some(State::Done));

        ekko.set_state(&words(&["@3", "undone"]), false).expect("an open dependent stopped a reopening");
        assert_eq!(ekko.blocker_map().unwrap().get(&4), Some(&vec![3]), "live evaluation did not block again");

        cleanup(&dir);
    }

    /// A dependency against the declared phase order is refused where it is
    /// made: a task in an earlier phase cannot wait on one in a later phase.
    /// The forward direction, the same phase and the project root are free.
    #[test]
    fn a_dependency_against_the_phase_order_is_refused() {
        let (ekko, dir) = fresh_ekko();
        ekko.set_phases(&words(&["early", "late"])).unwrap();
        ekko.create_task_in(&words(&["@a", "early work"]), Some("early")).unwrap();
        ekko.create_task_in(&words(&["@a", "late work"]), Some("late")).unwrap();
        ekko.create_task_in(&words(&["@a", "more early work"]), Some("early")).unwrap();
        ekko.create_task_in(&words(&["@a", "at the root"]), None).unwrap();

        let Err(EkkoError::PhaseOrder(inversion)) = ekko.set_blocked_by(&words(&["@1", "2"])) else {
            panic!("expected PHASE_ORDER")
        };
        assert_eq!((inversion.blocked, inversion.blocker), (1, 2));

        ekko.set_blocked_by(&words(&["@2", "1"])).expect("a later phase waiting on an earlier one was refused");
        ekko.set_blocked_by(&words(&["@3", "1"])).expect("the same phase was refused");
        ekko.set_blocked_by(&words(&["@4", "2"])).expect("the root was held to the order");

        cleanup(&dir);
    }

    /// Phases can still be reordered underneath existing dependencies -- that
    /// is not refused -- and the roadmap then names each inversion it finds.
    #[test]
    fn the_roadmap_names_the_inversions_a_reordering_left_behind() {
        let (ekko, dir) = fresh_ekko();
        ekko.set_phases(&words(&["early", "late"])).unwrap();
        ekko.create_task_in(&words(&["@a", "early work"]), Some("early")).unwrap();
        ekko.create_task_in(&words(&["@a", "late work"]), Some("late")).unwrap();
        ekko.set_blocked_by(&words(&["@2", "1"])).unwrap();

        ekko.set_phases(&words(&["late", "early"])).unwrap();

        let Outcome::Roadmap { inversions, .. } = ekko.display_roadmap().unwrap() else { panic!() };
        assert_eq!(inversions.len(), 1);
        assert_eq!((inversions[0].blocked, inversions[0].blocker), (2, 1));

        cleanup(&dir);
    }

    /// The cycle check visits each item once. It used to walk every path, and
    /// a chain of 26 diamonds took 12.5 s to accept one dependency, doubling
    /// with each diamond -- 40 would have run for days. A real cycle through
    /// the chain is still refused.
    #[test]
    fn the_cycle_check_is_linear_on_a_chain_of_diamonds() {
        let (ekko, dir) = fresh_ekko();
        let layers = 40u32;
        let uid = |id: u32| format!("d{id}-0");
        let mut data = ItemMap::new();
        let mut add = |id: u32, blockers: Vec<u32>| {
            let mut item = Item::new_task(id, format!("n{id}"), vec!["@g".into()], 1);
            item.uid = Some(uid(id));
            item.blocked_by = (!blockers.is_empty()).then(|| blockers.into_iter().map(uid).collect());
            data.insert(id, item);
        };
        add(1, vec![]);
        for k in 1..=layers {
            add(3 * k - 1, vec![3 * (k - 1) + 1]);
            add(3 * k, vec![3 * (k - 1) + 1]);
            add(3 * k + 1, vec![3 * k - 1, 3 * k]);
        }
        let bottom = 3 * layers + 1;
        let outside = bottom + 1;
        add(outside, vec![]);
        ekko.storage.set(&data).unwrap();

        let blocked = format!("@{outside}");
        let blocker = bottom.to_string();
        let started = std::time::Instant::now();
        ekko.set_blocked_by(&words(&[blocked.as_str(), blocker.as_str()])).unwrap();
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "took {:?}", started.elapsed());

        assert!(matches!(
            ekko.set_blocked_by(&words(&["@1", blocker.as_str()])),
            Err(EkkoError::BlockingCycle(_, _))
        ));

        cleanup(&dir);
    }

    /// The rule is checked on the board a command would leave, not task by
    /// task: a blocker and what it blocks complete together in one command,
    /// and reopen together, because neither board breaks anything.
    #[test]
    fn a_blocker_and_what_it_blocks_close_and_reopen_together() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["blocker"])).unwrap();
        ekko.create_task(&words(&["blocked"])).unwrap();
        ekko.set_blocked_by(&words(&["@2", "1"])).unwrap();

        ekko.set_state(&words(&["@1", "@2", "done"]), false).expect("completing both at once was refused");
        ekko.set_state(&words(&["@1", "@2", "undone"]), false).expect("reopening both at once was refused");
        ekko.check_tasks(&words(&["1", "2"]), false).expect("toggling both at once was refused");
        assert!(broken_dependencies(&ekko.storage.get().unwrap()).is_empty());

        cleanup(&dir);
    }

    /// --blocked-by cannot state the contradiction outright either: a task
    /// already done cannot be given a blocker that is still open. A closed
    /// blocker is fine, and clearing is always allowed.
    #[test]
    fn completed_work_cannot_be_declared_to_wait_on_open_work() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["open"])).unwrap();
        ekko.create_task(&words(&["finished"])).unwrap();
        ekko.create_task(&words(&["done already"])).unwrap();
        ekko.set_state(&words(&["@2", "@3", "done"]), false).unwrap();

        let result = ekko.set_blocked_by(&words(&["@3", "1"]));
        let Err(EkkoError::AlreadyDone(found)) = &result else {
            panic!("expected ALREADY_DONE, got {result:?}")
        };
        assert_eq!(found, &vec![(3, vec![1])]);
        assert!(ekko.storage.get().unwrap()[&3].blocked_by.is_none(), "it was recorded anyway");

        ekko.set_blocked_by(&words(&["@3", "2"])).expect("a closed blocker was refused");
        ekko.set_blocked_by(&words(&["@3"])).expect("clearing was refused");

        cleanup(&dir);
    }

    /// What --force reopened over is said on the same line, the way a forced
    /// completion says what it was blocked by.
    #[test]
    fn a_forced_reopening_says_so() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["blocker"])).unwrap();
        ekko.create_task(&words(&["dependent"])).unwrap();
        ekko.set_blocked_by(&words(&["@2", "1"])).unwrap();
        ekko.set_state(&words(&["@1", "@2", "done"]), false).unwrap();

        let outcome = ekko.set_state(&words(&["@1", "undone"]), true).unwrap();
        let mut buffer: Vec<u8> = Vec::new();
        {
            let mut renderer = Renderer::new(Painter::forced(false), Config::default(), &mut buffer);
            outcome.render(&mut renderer);
        }
        let rendered = String::from_utf8(buffer).unwrap();

        assert!(rendered.contains("Unchecked task: 1 (completed dependents overridden: 2)"), "{rendered:?}");

        cleanup(&dir);
    }

    /// The rule as a property rather than a list of cases: from a fresh
    /// board, no sequence of commands short of --force leaves completed work
    /// waiting on open work -- whatever each command was, and whether it
    /// landed or was refused. The sequence has to reach every refusal, or it
    /// proved nothing about that side of the rule.
    #[test]
    fn no_sequence_of_commands_short_of_force_breaks_a_dependency() {
        let (ekko, dir) = fresh_ekko();
        for n in 1..=6 {
            ekko.create_task(&words(&[format!("task {n}").as_str()])).unwrap();
        }
        // Xorshift: the same sequence every run, so a failure replays, and no
        // crate to pull in for it.
        let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
        let mut roll = |bound: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % bound
        };
        let states = ["done", "undone", "progress", "paused", "cancelled", "unstarted"];
        let mut refused: HashSet<String> = HashSet::new();

        for step in 0..500 {
            let (a, b) = ((roll(6) + 1).to_string(), (roll(6) + 1).to_string());
            let (at_a, at_b) = (format!("@{a}"), format!("@{b}"));
            let (tried, result) = match roll(6) {
                0 => (format!("--check {a} {b}"), ekko.check_tasks(&words(&[a.as_str(), b.as_str()]), false)),
                1 => (format!("--begin {a}"), ekko.begin_tasks(&words(&[a.as_str()]))),
                2 => {
                    let state = states[roll(6) as usize];
                    (format!("--set {at_a} {state}"), ekko.set_state(&words(&[at_a.as_str(), state]), false))
                }
                3 => {
                    let state = states[roll(6) as usize];
                    let input = words(&[at_a.as_str(), at_b.as_str(), state]);
                    (format!("--set {at_a} {at_b} {state}"), ekko.set_state(&input, false))
                }
                4 => (format!("--blocked-by {at_a}"), ekko.set_blocked_by(&words(&[at_a.as_str()]))),
                _ => (format!("--blocked-by {at_a} {b}"), ekko.set_blocked_by(&words(&[at_a.as_str(), b.as_str()]))),
            };
            if let Err(error) = &result {
                refused.insert(error.code().to_string());
            }
            let broken = broken_dependencies(&ekko.storage.get().unwrap());
            assert!(broken.is_empty(), "step {step}, after {tried}: {broken:?}");
        }

        for code in ["BLOCKED", "COMPLETED_DEPENDENTS", "ALREADY_DONE"] {
            assert!(refused.contains(code), "the sequence never reached {code}: {refused:?}");
        }

        cleanup(&dir);
    }
}
