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
use crate::render::{Renderer, Stats};
use crate::storage::{ItemMap, Storage, StorageError};

#[derive(Debug)]
pub enum EkkoError {
    MissingId,
    InvalidId(String),
    MissingDesc,
    InvalidIdsNumber,
    InvalidPriority,
    MissingBoards,
    InvalidCustomAppDir(String),
    MissingTaskbookDirFlagValue,
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
            EkkoError::InvalidCustomAppDir(_) => "INVALID_CUSTOM_APP_DIR",
            EkkoError::MissingTaskbookDirFlagValue => "MISSING_TASKBOOK_DIR_FLAG_VALUE",
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
            EkkoError::InvalidCustomAppDir(path) => out.invalid_custom_app_dir(path),
            EkkoError::MissingTaskbookDirFlagValue => out.missing_taskbook_dir_flag_value(),
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
            EkkoError::InvalidCustomAppDir(path) => {
                write!(f, "Custom app directory was not found on your system: {path}")
            }
            EkkoError::MissingTaskbookDirFlagValue => {
                write!(f, "Please provide a value for --taskbook-dir or remove the flag.")
            }
            EkkoError::LockTimeout(path) => write!(f, "Timed out waiting for the taskbook storage lock: {path}"),
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
            DirectoryError::MissingTaskbookDirFlagValue => EkkoError::MissingTaskbookDirFlagValue,
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

    /// Resolves the taskbook directory (flag > env > config > default,
    /// see `directory::retrieve_taskbook_directory`) and opens storage in
    /// it. `home_dir`/`cwd`/`taskbook_dir_flag`/`taskbook_dir_env` are the
    /// real values the CLI layer read; kept as parameters here for the
    /// same reason `directory`/`config` take them explicitly -- fully
    /// deterministic, no hidden reach into `std::env` inside business
    /// logic.
    pub fn open(
        home_dir: &std::path::Path,
        cwd: &std::path::Path,
        taskbook_dir_flag: Option<&str>,
        taskbook_dir_env: Option<&str>,
    ) -> Result<Self, EkkoError> {
        let dir = directory::retrieve_taskbook_directory(home_dir, cwd, taskbook_dir_flag, taskbook_dir_env)?;
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

    fn parse_create_options(&self, input: &[String]) -> Result<(Vec<String>, String, u8), EkkoError> {
        if input.is_empty() {
            return Err(EkkoError::MissingDesc);
        }

        let priority = get_priority(input);
        let mut boards = Vec::new();
        let mut words = Vec::new();
        for token in input {
            if is_priority_opt(token) {
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

        Ok((boards, description, priority))
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
        let (mut complete, mut in_progress, mut pending, mut notes) = (0u32, 0u32, 0u32, 0u32);
        for item in data.values() {
            if item.is_task {
                if item.is_complete.unwrap_or(false) {
                    complete += 1;
                } else if item.in_progress.unwrap_or(false) {
                    in_progress += 1;
                } else {
                    pending += 1;
                }
            } else {
                notes += 1;
            }
        }
        let total = complete + pending + in_progress;
        let percent = (complete * 100).checked_div(total).unwrap_or(0);
        Stats { percent, complete, in_progress, pending, notes }
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
                    data.retain(|_, item| item.is_task && !item.is_complete.unwrap_or(false));
                }
                "todo" | "task" | "tasks" => data.retain(|_, item| item.is_task),
                "note" | "notes" => data.retain(|_, item| !item.is_task),
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

    pub fn create_task(&self, input: &[String]) -> Result<Outcome, EkkoError> {
        let _lock = self.storage.acquire_lock()?;
        let (boards, description, priority) = self.parse_create_options(input)?;
        let mut data = self.storage.get()?;
        let id = self.generate_id(&data);
        let item = Item::new_task(id, description, boards, priority);
        data.insert(id, item.clone());
        self.storage.set(&data)?;
        Ok(Outcome::Task(item))
    }

    pub fn create_note(&self, input: &[String]) -> Result<Outcome, EkkoError> {
        let _lock = self.storage.acquire_lock()?;
        let (boards, description, _priority) = self.parse_create_options(input)?;
        let mut data = self.storage.get()?;
        let id = self.generate_id(&data);
        let item = Item::new_note(id, description, boards);
        data.insert(id, item.clone());
        self.storage.set(&data)?;
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
        self.storage.set(&data)?;
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
        self.storage.set(&data)?;
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
        self.storage.set(&data)?;
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

        self.storage.set(&data)?;
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
        self.storage.set(&data)?;
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
        self.storage.set(&data)?;
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
        self.storage.set(&data)?;
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
        self.storage.set(&data)?;
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

    pub fn list_by_attributes(&self, terms: &[String]) -> Result<Outcome, EkkoError> {
        let data = self.storage.get()?;
        let stored_boards = self.get_boards(&data);

        let (mut boards, mut attributes) = (Vec::new(), Vec::new());
        for term in terms {
            let at_board = format!("@{term}");
            if stored_boards.contains(&at_board) {
                boards.push(at_board);
            } else if term == "myboard" {
                boards.push("My Board".to_string());
            } else {
                attributes.push(term.clone());
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

#[cfg(test)]
mod tests {
    use super::*;
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
}
