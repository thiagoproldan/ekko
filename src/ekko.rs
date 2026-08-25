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

use std::collections::BTreeMap;

use crate::config;
use crate::directory::{self, DirectoryError};
use crate::item::Item;
use crate::render::{PathStep, Renderer, Stats};
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
            EkkoError::BlockingCycle(_, _) => out.generic_error(&self.to_string()),
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
                "Item {waiter} cannot wait on {blocker}: {blocker} already waits on {waiter}"
            ),
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
    Check { checked: Vec<u32>, unchecked: Vec<u32> },
    Begin { started: Vec<u32>, paused: Vec<u32> },
    Star { starred: Vec<u32>, unstarred: Vec<u32> },
    /// Idempotent form of Check/Begin/Star: the states asked for, not
    /// the flips performed.
    Set { ids: Vec<u32>, states: Vec<String> },
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
    Projects(Vec<String>),
    Phases(Vec<String>),
    Blocked { item: Item, blockers: Vec<u32> },
    Path { steps: Vec<PathStep>, rootless: u32 },
    Stats(Stats),
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
            Outcome::Phases(_) => "phases",
            Outcome::Blocked { .. } => "blocked",
            Outcome::Path { .. } => "path",
            Outcome::Stats(_) => "stats",
        }
    }

    pub fn render(&self, out: &mut Renderer) {
        match self {
            Outcome::Task(item) | Outcome::Note(item) => out.success_create(item),
            Outcome::Check { checked, unchecked } => {
                out.mark_complete(checked);
                out.mark_incomplete(unchecked);
            }
            Outcome::Set { ids, states } => {
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
                        "done" => out.mark_complete(ids),
                        "undone" => out.mark_incomplete(ids),
                        "progress" => out.mark_started(ids),
                        "paused" => out.mark_paused(ids),
                        "cancelled" => out.mark_cancelled(ids),
                        "unstarted" => out.mark_reset(ids),
                        "starred" => out.mark_starred(ids),
                        "unstarred" => out.mark_unstarred(ids),
                        _ => {}
                    }
                }
            }
            Outcome::Begin { started, paused } => {
                out.mark_started(started);
                out.mark_paused(paused);
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
            Outcome::Projects(names) => out.display_projects(names),
            Outcome::Phases(names) => out.display_projects(names),
            Outcome::Blocked { item, blockers } => out.success_blocked(item.id, blockers),
            Outcome::Path { steps, rootless } => out.display_path(steps, *rootless),
            Outcome::Stats(stats) => out.display_stats(stats),
        }
    }
}

pub struct Ekko {
    storage: Storage,
}

impl Ekko {
    pub fn new(storage: Storage) -> Self {
        Ekko { storage }
    }

    /// Resolves the ekko directory and opens storage in it.
    ///
    /// Precedence: `--ekko-dir` > `--project` > `EKKO_DIR` > config >
    /// default. `--ekko-dir` and `--project` together is an error rather
    /// than a silent winner: both say where data lives, and guessing which
    /// one someone meant is how you end up writing to the wrong board.
    ///
    /// Everything is a parameter here for the same reason `directory` and
    /// `config` take them explicitly -- fully deterministic, no hidden reach
    /// into `std::env` inside business logic.
    pub fn open(
        home_dir: &std::path::Path,
        cwd: &std::path::Path,
        ekko_dir_flag: Option<&str>,
        ekko_dir_env: Option<&str>,
        project: Option<&str>,
        create_project: bool,
    ) -> Result<Self, EkkoError> {
        let dir = match project {
            Some(name) if ekko_dir_flag.is_some() => {
                let _ = name;
                return Err(directory::DirectoryError::ProjectAndEkkoDirTogether.into());
            }
            Some(name) => directory::retrieve_project_directory(home_dir, name, create_project)?,
            None => {
                directory::retrieve_ekko_directory(home_dir, cwd, ekko_dir_flag, ekko_dir_env)?
            }
        };
        Ok(Self::new(Storage::new(&dir)?))
    }

    // ---- id / option parsing -------------------------------------------

    fn generate_id(&self, data: &ItemMap) -> u32 {
        data.keys().max().copied().unwrap_or(0) + 1
    }

    fn validate_ids(&self, raw_ids: &[String], existing: &ItemMap) -> Result<Vec<u32>, EkkoError> {
        if raw_ids.is_empty() {
            return Err(EkkoError::MissingId);
        }

        let mut ids = Vec::new();
        for raw in raw_ids {
            let id: u32 = raw.parse().map_err(|_| EkkoError::InvalidId(raw.clone()))?;
            if !existing.contains_key(&id) {
                return Err(EkkoError::InvalidId(raw.clone()));
            }
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

    fn compute_stats(&self, data: &ItemMap) -> Stats {
        let (mut complete, mut in_progress, mut paused, mut cancelled, mut pending, mut notes) =
            (0u32, 0u32, 0u32, 0u32, 0u32, 0u32);
        for item in data.values() {
            if item.is_task {
                if item.cancelled.unwrap_or(false) {
                    cancelled += 1;
                } else if item.is_complete.unwrap_or(false) {
                    complete += 1;
                } else if item.in_progress.unwrap_or(false) {
                    in_progress += 1;
                } else if item.paused.unwrap_or(false) {
                    // Counted apart from pending on purpose: lumping them back
                    // together is exactly the conflation this state exists to
                    // undo, and "0 pending" while two tasks sit half-done was
                    // the original lie.
                    paused += 1;
                } else {
                    pending += 1;
                }
            } else {
                notes += 1;
            }
        }
        // `cancelled` is absent from the total on purpose: counting it would
        // mean a board can never reach 100% once anything is dropped, which
        // reads as unfinished work rather than as work that went away.
        let total = complete + pending + in_progress + paused;
        let percent = (complete * 100).checked_div(total).unwrap_or(0);
        Stats { percent, complete, in_progress, paused, cancelled, pending, notes }
    }

    fn filter_by_attributes(&self, attributes: &[String], mut data: ItemMap) -> ItemMap {
        if data.is_empty() {
            return data;
        }
        for attribute in attributes {
            match attribute.as_str() {
                "star" | "starred" => data.retain(|_, item| item.is_starred),
                "done" | "checked" | "complete" => {
                    data.retain(|_, item| item.is_task && item.is_complete.unwrap_or(false));
                }
                "progress" | "started" | "begun" => {
                    data.retain(|_, item| item.is_task && item.in_progress.unwrap_or(false));
                }
                // Matches the JS version exactly: "pending" only checks
                // `!isComplete`, so an in-progress task passes this filter
                // too. Not something this port introduced or should
                // silently change.
                "pending" | "unchecked" | "incomplete" => {
                    // Cancelled excluded, unlike the JS version, which had no such
                    // state to exclude. A dropped task is not waiting to be done,
                    // and listing it as pending is the same conflation the
                    // paused state was added to undo.
                    data.retain(|_, item| {
                        item.is_task
                            && !item.is_complete.unwrap_or(false)
                            && !item.cancelled.unwrap_or(false)
                    });
                }
                "todo" | "task" | "tasks" => data.retain(|_, item| item.is_task),
                "note" | "notes" => data.retain(|_, item| !item.is_task),
                // Needs the whole map, not just the retained subset: a
                // blocker can sit outside whatever else is being filtered.
                "ready" => {
                    let all = data.clone();
                    data.retain(|_, item| {
                        item.is_task
                            && !item.is_complete.unwrap_or(false)
                            && !item.cancelled.unwrap_or(false)
                            && Self::unmet_blockers(&all, item).is_empty()
                    });
                }
                "blocked" => {
                    let all = data.clone();
                    data.retain(|_, item| !Self::unmet_blockers(&all, item).is_empty());
                }
                "cancelled" | "canceled" => {
                    data.retain(|_, item| item.cancelled.unwrap_or(false));
                }
                "due" => data.retain(|_, item| item.due_date.is_some()),
                // Only tasks that are still open: a finished task is not
                // late, however long its deadline has been past.
                "overdue" => {
                    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
                    data.retain(|_, item| {
                        item.is_task
                            && !item.is_complete.unwrap_or(false)
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
        let archive_id = self.generate_id(archive);
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

    /// Writes `data` back, stamping `updated_at` on whatever actually
    /// changed.
    ///
    /// Works by diffing against what is currently on disk rather than
    /// asking each command to remember which ids it touched. That is
    /// deliberate: there are eleven mutating commands, a twelfth is always
    /// possible, and a hand-stamped field is one someone eventually forgets
    /// to stamp. Comparing catches every field, including ones added later.
    ///
    /// Re-reading here is safe because every caller already holds the lock.
    fn save_touching(&self, data: &mut ItemMap) -> Result<(), EkkoError> {
        let before = self.storage.get()?;
        let now = chrono::Local::now().timestamp_millis();

        for (id, item) in data.iter_mut() {
            if before.get(id) != Some(&*item) {
                item.updated_at = Some(now);
            }
        }

        self.storage.set(data)?;
        Ok(())
    }

    /// `phase` is the scope the CLI was invoked with. Items created without
    /// one land at the project root, outside the path -- never in a guessed
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

    pub fn check_tasks(&self, ids: &[String]) -> Result<Outcome, EkkoError> {
        let _lock = self.storage.acquire_lock()?;
        let mut data = self.storage.get()?;
        let ids = self.validate_ids(ids, &data)?;
        let (mut checked, mut unchecked) = (Vec::new(), Vec::new());
        for id in ids {
            if let Some(item) = data.get_mut(&id) {
                if item.is_task {
                    item.in_progress = Some(false);
                    let now_complete = !item.is_complete.unwrap_or(false);
                    item.is_complete = Some(now_complete);
                    if now_complete { checked.push(id) } else { unchecked.push(id) }
                }
            }
        }
        self.save_touching(&mut data)?;
        Ok(Outcome::Check { checked, unchecked })
    }

    pub fn begin_tasks(&self, ids: &[String]) -> Result<Outcome, EkkoError> {
        let _lock = self.storage.acquire_lock()?;
        let mut data = self.storage.get()?;
        let ids = self.validate_ids(ids, &data)?;
        let (mut started, mut paused) = (Vec::new(), Vec::new());
        for id in ids {
            if let Some(item) = data.get_mut(&id) {
                if item.is_task {
                    item.is_complete = Some(false);
                    let now_in_progress = !item.in_progress.unwrap_or(false);
                    item.in_progress = Some(now_in_progress);
                    if now_in_progress { started.push(id) } else { paused.push(id) }
                }
            }
        }
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

    pub fn delete_items(&self, ids: &[String]) -> Result<Outcome, EkkoError> {
        let _lock = self.storage.acquire_lock()?;
        self.delete_items_locked(ids)
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
        let ids: Vec<String> =
            data.iter().filter(|(_, item)| item.is_complete.unwrap_or(false)).map(|(id, _)| id.to_string()).collect();
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

    pub fn display_by_board(&self) -> Result<Outcome, EkkoError> {
        let data = self.storage.get()?;
        let boards = self.get_boards(&data);
        Ok(Outcome::Board(self.group_by_board(&data, &boards)))
    }

    pub fn display_by_date(&self) -> Result<Outcome, EkkoError> {
        let data = self.storage.get()?;
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
        let data = self.storage.get()?;
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
    pub fn set_state(&self, input: &[String]) -> Result<Outcome, EkkoError> {
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

        for id in &ids {
            if let Some(item) = data.get_mut(id) {
                for state in &states {
                    apply_state(item, state);
                }
            }
        }

        self.save_touching(&mut data)?;
        Ok(Outcome::Set { ids, states })
    }

    /// The board, restricted to items changed at or after `since` (epoch
    /// millis). Grouped by board like the default view, so the shape a
    /// caller parses does not change with the filter.
    ///
    /// Only reports items that exist. A deletion leaves nothing behind to
    /// carry a timestamp, so a caller that must notice removals has to
    /// compare id sets, not just read this.
    pub fn display_since(&self, since: i64) -> Result<Outcome, EkkoError> {
        let mut data = self.storage.get()?;
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
        let mut map = std::collections::HashMap::new();
        for (id, item) in &data {
            let unmet = Self::unmet_blockers(&data, item);
            if !unmet.is_empty() {
                map.insert(*id, unmet);
            }
        }
        Ok(map)
    }

    /// Records that one item waits on others, replacing whatever it waited
    /// on before -- the same contract `--move` and `--phases` use.
    ///
    /// Refuses to create a cycle. Without one, A waiting on B while B waits
    /// on A is a pair nothing can ever make ready, and the board would state
    /// it as calmly as any other fact.
    pub fn set_blocked_by(&self, input: &[String]) -> Result<Outcome, EkkoError> {
        let _lock = self.storage.acquire_lock()?;
        let mut data = self.storage.get()?;

        let (id_str, rest) = self.extract_single_id_target(input)?;
        let id = self.validate_ids(&[id_str], &data)?[0];

        let blocker_ids = self.validate_ids(
            &rest.into_iter().cloned().collect::<Vec<String>>(),
            &data,
        )?;

        for blocker in &blocker_ids {
            if *blocker == id {
                return Err(EkkoError::BlockingCycle(id, *blocker));
            }
            if self.reaches(&data, *blocker, id) {
                return Err(EkkoError::BlockingCycle(id, *blocker));
            }
        }

        let uids: Vec<String> =
            blocker_ids.iter().filter_map(|b| data.get(b)?.uid.clone()).collect();

        let item = data.get_mut(&id).expect("id just validated against data");
        item.blocked_by = if uids.is_empty() { None } else { Some(uids) };
        let updated = item.clone();

        self.save_touching(&mut data)?;
        Ok(Outcome::Blocked { item: updated, blockers: blocker_ids })
    }

    /// Whether `from` already waits, directly or through others, on `target`.
    fn reaches(&self, data: &ItemMap, from: u32, target: u32) -> bool {
        let Some(item) = data.get(&from) else { return false };
        let Some(blockers) = item.blocked_by.as_ref() else { return false };

        for uid in blockers {
            let Some((id, _)) = data.iter().find(|(_, i)| i.uid.as_deref() == Some(uid)) else {
                continue;
            };
            if *id == target || self.reaches(data, *id, target) {
                return true;
            }
        }
        false
    }

    /// The blockers of `item` that are still outstanding, as current display
    /// ids. A finished, cancelled or deleted blocker is not one -- which is
    /// why nothing ever has to be unblocked by hand.
    pub fn unmet_blockers(data: &ItemMap, item: &Item) -> Vec<u32> {
        let Some(uids) = item.blocked_by.as_ref() else { return Vec::new() };

        let mut ids: Vec<u32> = uids
            .iter()
            .filter_map(|uid| data.iter().find(|(_, i)| i.uid.as_deref() == Some(uid)))
            .filter(|(_, blocker)| {
                !blocker.is_complete.unwrap_or(false) && !blocker.cancelled.unwrap_or(false)
            })
            .map(|(id, _)| *id)
            .collect();
        ids.sort_unstable();
        ids
    }
    /// Replaces the project's phase sequence.
    pub fn set_phases(&self, names: &[String]) -> Result<Outcome, EkkoError> {
        let _lock = self.storage.acquire_lock()?;
        let cleaned = remove_duplicates(
            names.iter().map(|n| n.trim_start_matches('@').to_string()).collect(),
        );
        self.storage.set_phases(&cleaned)?;
        Ok(Outcome::Phases(cleaned))
    }

    /// The journey: declared phases in order, each with how far it has got,
    /// and which one holds work in progress.
    ///
    /// Nothing here is stored beyond the sequence itself -- the counts and
    /// the cursor are read off the items every time, so the view cannot
    /// drift from the board it describes.
    pub fn display_path(&self) -> Result<Outcome, EkkoError> {
        let data = self.storage.get()?;
        let phases = self.storage.get_phases()?;

        let mut steps = Vec::new();
        for name in &phases {
            let items: Vec<&Item> =
                data.values().filter(|i| i.phase.as_deref() == Some(name.as_str())).collect();

            let tasks: Vec<&&Item> = items.iter().filter(|i| i.is_task).collect();
            let complete =
                tasks.iter().filter(|i| i.is_complete.unwrap_or(false)).count() as u32;
            let cancelled = tasks.iter().filter(|i| i.cancelled.unwrap_or(false)).count() as u32;
            let current = tasks.iter().any(|i| i.in_progress.unwrap_or(false));

            steps.push(PathStep {
                name: name.clone(),
                complete,
                // Cancelled work is not work, the same way it is left out of
                // the percentage.
                total: tasks.len() as u32 - cancelled,
                notes: items.iter().filter(|i| !i.is_task).count() as u32,
                current,
            });
        }

        // Anything inside the project but outside every phase. Counted rather
        // than hidden: the root is a deliberate exception, not a hole.
        let rootless = data.values().filter(|i| i.phase.is_none()).count() as u32;

        Ok(Outcome::Path { steps, rootless })
    }

    pub fn list_by_attributes(&self, terms: &[String]) -> Result<Outcome, EkkoError> {
        let data = self.storage.get()?;
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

        let filtered = self.filter_by_attributes(&attributes, data);
        Ok(Outcome::List(self.group_by_board(&filtered, &boards)))
    }
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

fn parse_due_date(token: &str) -> Option<String> {
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

fn remove_duplicates(items: Vec<String>) -> Vec<String> {
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
fn canonical_state(term: &str) -> Option<&'static str> {
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

/// Applies one canonical state. Task-only states are skipped on notes,
/// matching how `--check` and `--begin` already ignore them; starring is
/// the one that applies to both.
fn apply_state(item: &mut Item, state: &str) {
    match state {
        "done" if item.is_task => {
            item.is_complete = Some(true);
            item.in_progress = Some(false);
            // Finishing something settles it: there is nothing left paused.
            item.paused = None;
            item.cancelled = None;
        }
        "undone" if item.is_task => item.is_complete = Some(false),
        "progress" if item.is_task => {
            item.in_progress = Some(true);
            item.is_complete = Some(false);
            item.paused = None;
            item.cancelled = None;
        }
        "paused" if item.is_task => {
            item.in_progress = Some(false);
            item.paused = Some(true);
            item.cancelled = None;
        }
        // Terminal, like done, and mutually exclusive with it. Reviving a
        // cancelled task goes through `unstarted`.
        "cancelled" if item.is_task => {
            item.cancelled = Some(true);
            item.is_complete = Some(false);
            item.in_progress = Some(false);
            item.paused = None;
        }
        // Back to never-started: clears both flags, which is what undoes a
        // `--set progress` aimed at the wrong id.
        "unstarted" if item.is_task => {
            item.in_progress = Some(false);
            item.paused = None;
            item.cancelled = None;
        }
        "starred" => item.is_starred = true,
        "unstarred" => item.is_starred = false,
        _ => {}
    }
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

        let Outcome::Check { checked, unchecked } = ekko.check_tasks(&words(&["1", "2"])).unwrap() else { panic!() };
        assert_eq!(checked, vec![1]);
        assert_eq!(unchecked, Vec::<u32>::new());

        let Outcome::Check { checked, unchecked } = ekko.check_tasks(&words(&["1"])).unwrap() else { panic!() };
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

    #[test]
    fn delete_reports_both_the_storage_id_and_the_new_unrelated_archive_id() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["first"])).unwrap();

        let Outcome::Delete(results) = ekko.delete_items(&words(&["1"])).unwrap() else { panic!() };

        assert_eq!(results, vec![DeleteResult { storage_id: 1, archive_id: 1 }]);

        cleanup(&dir);
    }

    #[test]
    fn restore_gives_the_item_a_fresh_storage_id_not_the_old_one() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["a"])).unwrap();
        ekko.create_task(&words(&["b"])).unwrap();
        ekko.delete_items(&words(&["1"])).unwrap(); // archive id 1

        let Outcome::Restore(results) = ekko.restore_items(&words(&["1"])).unwrap() else { panic!() };

        // Next storage id is 3 (1 and 2 already used, 1 was deleted but
        // ids aren't reused across a *different* map the way they are
        // within the same one).
        assert_eq!(results, vec![RestoreResult { archive_id: 1, storage_id: 3 }]);

        cleanup(&dir);
    }

    #[test]
    fn deleted_ids_are_reused_within_storage_max_plus_one_not_a_counter() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["a"])).unwrap();
        ekko.create_task(&words(&["b"])).unwrap(); // id 2
        ekko.delete_items(&words(&["2"])).unwrap();

        let Outcome::Task(item) = ekko.create_task(&words(&["c"])).unwrap() else { panic!() };

        assert_eq!(item.id, 2, "id 2 should be reused now that the max id is 1 again");

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
        ekko.check_tasks(&words(&["1"])).unwrap();

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

        ekko.set_state(&words(&["@1", "done"])).unwrap();

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

        let data = ekko.storage.get().unwrap();
        let waiter = data.values().next().unwrap();
        assert!(Ekko::unmet_blockers(&data, waiter).is_empty());

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
    fn an_item_created_without_a_phase_lands_outside_the_path_and_is_counted() {
        let (ekko, dir) = fresh_ekko();
        ekko.set_phases(&words(&["setup"])).unwrap();
        ekko.create_task_in(&words(&["@a", "in a phase"]), Some("setup")).unwrap();
        ekko.create_task_in(&words(&["@b", "at the root"]), None).unwrap();

        let Outcome::Path { steps, rootless } = ekko.display_path().unwrap() else { panic!() };

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

        let Outcome::Path { steps, .. } = ekko.display_path().unwrap() else { panic!() };

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

        let Outcome::Path { steps, .. } = ekko.display_path().unwrap() else { panic!() };
        assert!(steps.iter().all(|s| !s.current), "nothing in progress means no cursor");

        ekko.set_state(&words(&["@2", "progress"])).unwrap();
        let Outcome::Path { steps, .. } = ekko.display_path().unwrap() else { panic!() };
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
        ekko.set_state(&words(&["@1", "done"])).unwrap();
        ekko.set_state(&words(&["@2", "cancelled"])).unwrap();

        let Outcome::Path { steps, .. } = ekko.display_path().unwrap() else { panic!() };

        assert_eq!((steps[0].complete, steps[0].total), (1, 1), "reads as finished");

        cleanup(&dir);
    }

    #[test]
    fn cancelling_is_terminal_and_mutually_exclusive_with_the_other_states() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["dropped"])).unwrap();
        ekko.set_state(&words(&["@1", "progress"])).unwrap();

        ekko.set_state(&words(&["@1", "cancelled"])).unwrap();

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
            ekko.set_state(&words(&[id, "cancelled"])).unwrap();
            ekko.set_state(&words(&[id, revive])).unwrap();
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
        ekko.set_state(&words(&["@1", "done"])).unwrap();
        ekko.set_state(&words(&["@2", "cancelled"])).unwrap();

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

        ekko.set_state(&words(&["@1", "progress"])).unwrap();
        ekko.set_state(&words(&["@1", "paused"])).unwrap();

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
        ekko.set_state(&words(&["@1", "progress"])).unwrap();

        ekko.set_state(&words(&["@1", "unstarted"])).unwrap();

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
            ekko.set_state(&words(&[id, "progress"])).unwrap();
            ekko.set_state(&words(&[id, "paused"])).unwrap();
        }

        ekko.set_state(&words(&["@1", "progress"])).unwrap();
        ekko.set_state(&words(&["@2", "done"])).unwrap();

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
        ekko.set_state(&words(&["@1", "progress"])).unwrap();
        ekko.set_state(&words(&["@1", "paused"])).unwrap();

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
        ekko.set_state(&words(&["@2", "done"])).unwrap();

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
        ekko.set_state(&words(&["@1", "done"])).unwrap();
        let after_first = ekko.storage.get().unwrap()[&1].updated_at;

        std::thread::sleep(std::time::Duration::from_millis(5));
        ekko.set_state(&words(&["@1", "done"])).unwrap(); // idempotent: no change

        assert_eq!(ekko.storage.get().unwrap()[&1].updated_at, after_first);

        cleanup(&dir);
    }

    #[test]
    fn uid_distinguishes_items_that_share_a_recycled_id() {
        // Ids are `max + 1`, so deleting the highest-numbered item and
        // creating another hands the new one the same id. This is exactly
        // the case a caller holding an id across time gets wrong.
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["first"])).unwrap();
        ekko.create_task(&words(&["second"])).unwrap();
        let old_uid = ekko.storage.get().unwrap()[&2].uid.clone();

        ekko.delete_items(&words(&["2"])).unwrap();
        ekko.create_task(&words(&["reuses id 2"])).unwrap();

        let data = ekko.storage.get().unwrap();
        assert_eq!(data[&2].description, "reuses id 2", "the id really was recycled");
        assert!(old_uid.is_some());
        assert_ne!(data[&2].uid, old_uid, "same id, different item, different uid");

        cleanup(&dir);
    }

    #[test]
    fn uid_survives_the_fresh_id_a_restore_hands_out() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["archived then restored"])).unwrap();
        let before = ekko.storage.get().unwrap()[&1].uid.clone();

        ekko.check_tasks(&words(&["1"])).unwrap();
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
        ekko.set_state(&words(&["@1", "done"])).unwrap();
        ekko.set_state(&words(&["@1", "done"])).unwrap();
        assert_eq!(ekko.storage.get().unwrap()[&1].is_complete, Some(true));

        // Contrast, on the same data: the toggle flips back.
        ekko.check_tasks(&words(&["1"])).unwrap();
        assert_eq!(ekko.storage.get().unwrap()[&1].is_complete, Some(false));

        cleanup(&dir);
    }

    #[test]
    fn set_applies_several_states_to_several_items_at_once() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["one"])).unwrap();
        ekko.create_task(&words(&["two"])).unwrap();

        ekko.set_state(&words(&["@1", "@2", "progress", "starred"])).unwrap();

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

        ekko.set_state(&words(&["@1", "done", "starred"])).unwrap();

        let data = ekko.storage.get().unwrap();
        assert_eq!(data[&1].is_complete, None, "a note never gains task fields");
        assert!(data[&1].is_starred, "starring works on notes, matching --star");

        cleanup(&dir);
    }

    #[test]
    fn set_rejects_missing_or_unknown_states_rather_than_doing_nothing() {
        let (ekko, dir) = fresh_ekko();
        ekko.create_task(&words(&["a task"])).unwrap();

        assert!(matches!(ekko.set_state(&words(&["@1"])), Err(EkkoError::MissingState)));
        assert!(matches!(
            ekko.set_state(&words(&["@1", "finished"])),
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
        ekko.check_tasks(&words(&["2"])).unwrap();

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

        assert_eq!(ekko.check_tasks(&[]).unwrap_err().code(), "MISSING_ID");
        assert_eq!(ekko.check_tasks(&words(&["999"])).unwrap_err().code(), "INVALID_ID");
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
            let outcome = ekko.set_state(&words(&["@1", state])).unwrap();

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
}
