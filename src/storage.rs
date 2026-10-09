//! File storage: reading/writing `storage.json`/`archive.json`, and the
//! cross-process lock that protects both.
//!
//! Ported from the JS version's `storage.js` with two structural changes.
//!
//! The first is made natural by Rust rather than bolted on: instead of a
//! module-level `Set` of held lock paths plus a manually-registered
//! `process.on('exit')` cleanup hook (needed there because
//! `process.exit()` skips `finally` blocks), `acquire_lock` returns a
//! [`LockGuard`] that releases on drop. As long as call sites propagate
//! errors with `?` instead of exiting mid-function (see the crate's
//! error-handling design), normal Rust unwinding guarantees the release.
//!
//! The second is the lock primitive itself: `flock(2)` on a held
//! descriptor, rather than the JS version's pid-in-a-lock-file scheme.
//! The kernel releases a `flock` whenever the descriptor closes, so a
//! holder that exits, panics or is killed outright leaves nothing to clean
//! up, and no other process ever needs to judge whether a lock is
//! abandoned. A port of the original scheme was tried first and lost
//! updates under real contention; `acquire_lock` documents exactly how. Nesting (e.g. one operation built out of two others that
//! each separately need the lock) is handled by having only the outermost,
//! public entry point acquire it, with private helpers that assume it's
//! already held -- not by making acquisition itself re-entrant.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read as _};
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::item::Item;
use crate::json;

const LOCK_ACQUIRE_TIMEOUT: Duration = Duration::from_millis(5000);

/// How old a leftover file in the temp directory has to be before
/// `clean_temp_dir` treats it as debris from a crashed write rather than
/// an in-flight one. Nothing to do with the lock. A write creates its temp
/// file, fsyncs it and renames it, and the fsync alone can stall past a
/// second on a busy btrfs: at one second, the next process to open the
/// board -- a hook, or the resources server every two seconds -- could
/// delete a live writer's file and fail its rename. Debris costs nothing
/// while it waits, so the margin is generous.
const TEMP_FILE_ABANDONED: Duration = Duration::from_secs(600);

/// The versions of storage.json `history/` keeps: the newest this many, and,
/// of the older ones, the newest of each day for `HISTORY_DAYS` days -- the
/// last hour of work to undo a bad write, and a few weeks to go back to.
/// About 64 versions, 21 MB on a board of 330 KB.
const HISTORY_RECENT: usize = 50;
const HISTORY_DAYS: u64 = 14;

/// Boards/timeline grouping iterates this in id order; a `BTreeMap` gives
/// that for free and matches the JS version's behavior, where plain objects
/// with integer-like string keys (`{"1": ..., "2": ...}`) iterate in
/// ascending numeric order regardless of insertion order.
pub type ItemMap = BTreeMap<u32, Item>;

#[derive(Debug)]
pub enum StorageError {
    Io(io::Error),
    Json(serde_json::Error),
    /// The lock, and who held it as /proc/locks told when the wait gave up.
    LockTimeout(PathBuf, Option<String>),
}

impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StorageError::Io(e) => write!(f, "{e}"),
            StorageError::Json(e) => write!(f, "{e}"),
            StorageError::LockTimeout(path, holder) => {
                write!(f, "{} {}", lock_timeout_advice(holder.as_deref()), path.display())
            }
        }
    }
}

impl std::error::Error for StorageError {}

impl From<io::Error> for StorageError {
    fn from(error: io::Error) -> Self {
        StorageError::Io(error)
    }
}

impl From<serde_json::Error> for StorageError {
    fn from(error: serde_json::Error) -> Self {
        StorageError::Json(error)
    }
}

pub struct Storage {
    /// The board's directory, `.ekko/`.
    dir: PathBuf,
    storage_file: PathBuf,
    archive_file: PathBuf,
    temp_dir: PathBuf,
    lock_file: PathBuf,
    /// Where the versions of storage.json a write replaced are kept.
    history_dir: PathBuf,
    /// Where every write copies the board's files, outside the project's
    /// folder -- `~/.ekko/copies/<project id>/` -- or `None` for a board
    /// that is no project's (task 503).
    copy_dir: Option<PathBuf>,
}

/// Proof of a held lock. Releases it on drop, including when a caller
/// returns early via `?` -- unlike the JS version, no explicit
/// "did I remember to unlock on every exit path" bookkeeping is possible to
/// forget.
///
/// There is no `Drop` impl: the lock *is* the open descriptor, so closing
/// the file is the release, and `File` already does that on drop. The file
/// is deliberately never unlinked -- deleting it would let the next process
/// create a fresh inode and lock that one while this guard still holds the
/// old one, which is exactly the kind of hole the pid-file scheme had.
pub struct LockGuard<'a> {
    _storage: &'a Storage,
    _file: File,
}

/// The file of the board in `ekko_dir`, as `Storage::storage_path` names it,
/// found without opening the board.
pub fn storage_file(ekko_dir: &Path) -> PathBuf {
    ekko_dir.join("storage").join("storage.json")
}

impl Storage {
    pub fn new(ekko_dir: &Path) -> Result<Self, StorageError> {
        let storage_dir = ekko_dir.join("storage");
        let archive_dir = ekko_dir.join("archive");
        let temp_dir = ekko_dir.join(".temp");

        fs::create_dir_all(&storage_dir)?;
        fs::create_dir_all(&archive_dir)?;
        fs::create_dir_all(&temp_dir)?;

        let storage = Storage {
            dir: ekko_dir.to_path_buf(),
            storage_file: storage_file(ekko_dir),
            archive_file: archive_dir.join("archive.json"),
            temp_dir,
            lock_file: ekko_dir.join(".lock"),
            history_dir: ekko_dir.join("history"),
            copy_dir: None,
        };

        storage.clean_temp_dir()?;
        Ok(storage)
    }

    /// This storage, copying the board's files to `copy_dir` at every write
    /// (see `copy_out`).
    pub fn copied_to(mut self, copy_dir: Option<PathBuf>) -> Self {
        self.copy_dir = copy_dir;
        self
    }

    /// Sweeps up temp files abandoned by a crashed write (create-then-
    /// rename is atomic, but only once the write in between has actually
    /// finished). This runs unconditionally at startup, for every
    /// process, *not* under the lock -- constructing `Storage` at all
    /// needs to stay lock-free for read-only commands. That means it can
    /// run concurrently with another live process's in-flight
    /// `write_atomic`, so it only removes temp files old enough that they
    /// cannot plausibly still be someone's in-progress write (one fresh,
    /// uniquely-named file, written, fsynced and renamed -- gone in well
    /// under this margin under any real load); a fresh temp file is left
    /// alone rather than risk deleting live work.
    fn clean_temp_dir(&self) -> Result<(), StorageError> {
        for entry in fs::read_dir(&self.temp_dir)? {
            let entry = entry?;
            let age = entry
                .metadata()
                .and_then(|m| m.modified())
                .and_then(|m| m.elapsed().map_err(io::Error::other))
                .unwrap_or(Duration::ZERO);
            if age >= TEMP_FILE_ABANDONED {
                fs::remove_file(entry.path())?;
            }
        }
        Ok(())
    }

    /// The board, to change: a copy of the version `get_shared` keeps, so a
    /// command parses each version once however often it reads it. A command
    /// reads more than once -- before it changes the board and again under
    /// its lock, then for the view after -- and not keeping the first read
    /// made `--list pending` parse the board five times and `--task` four
    /// (task 844). A copy costs about a quarter of a parse; a read that
    /// changes nothing takes `get_shared` and no copy at all.
    pub fn get(&self) -> Result<ItemMap, StorageError> {
        Ok(ItemMap::clone(&*self.get_shared()?))
    }

    /// The board, to read, shared with every other read of the same version
    /// in this process. The MCP server answers many calls from one process,
    /// and parsing is most of what a read costs: 25 ms of a 41 ms `context`
    /// on a board of 20,000 items. A version is the file's identity as its
    /// open descriptor reports it, so the check costs one `fstat`, and a write
    /// -- always a rename, so always a new inode -- is never answered from
    /// the version it replaced.
    pub fn get_shared(&self) -> Result<Arc<ItemMap>, StorageError> {
        let Some((file, version)) = open_versioned(&self.storage_file)? else {
            return Ok(Arc::default());
        };
        if let Some(board) = kept_board(&self.storage_file, version) {
            return Ok(board);
        }
        let board = Arc::new(parse_map(file)?);
        keep_board(&self.storage_file, version, Arc::clone(&board));
        Ok(board)
    }

    pub fn get_archive(&self) -> Result<ItemMap, StorageError> {
        read_map(&self.archive_file)
    }

    pub fn set(&self, data: &ItemMap) -> Result<(), StorageError> {
        // Nothing before the rename touches the version it replaces. Claude
        // Code's FileChanged follows storage.json by its inode, and an event
        // on it before the rename could leave that watch on the old inode for
        // good (task 1248). So history takes the new version before it is put
        // in place, and the old one only by copy, when no write kept it.
        self.keep_unkept_version();
        let content = json::to_pretty_string(data)?;
        let version = replace_durably_keeping(&self.storage_file, &self.temp_dir, content.as_bytes(), Some(&self.history_dir))?;
        self.prune_history();
        // The version this write made is the map it wrote, so the read after
        // it -- the MCP server reads the board after every write -- parses
        // nothing (task 859). A later write by any process is a new inode.
        keep_board(&self.storage_file, version, Arc::new(data.clone()));
        self.copy_out(&self.storage_file);
        // The files a write seldom touches are copied the first time too, so
        // a copy is whole from its first write; after that, each refreshes
        // its own copy whenever it is written.
        for file in self.board_files() {
            if self.copy_of(&file).is_some_and(|copy| !copy.exists()) {
                self.copy_out(&file);
            }
        }
        Ok(())
    }

    /// Every file that makes up the board, and that a copy must hold to bring
    /// it back: a file added to the board is added here.
    fn board_files(&self) -> [PathBuf; 6] {
        [
            self.storage_file.clone(),
            self.counters_file(),
            self.journal_file(),
            self.phases_file(),
            self.archive_file.clone(),
            self.dir.join(crate::project::MARKER),
        ]
    }

    /// Where `file`, one of the board's, is copied: the same place under
    /// `copy_dir` as it has under the board's directory.
    fn copy_of(&self, file: &Path) -> Option<PathBuf> {
        Some(self.copy_dir.as_ref()?.join(file.strip_prefix(&self.dir).ok()?))
    }

    /// Brings the copy of `file` up to date, outside the project's folder, so
    /// that `git clean -fdx` or removing the folder leaves the board behind
    /// (task 503). A hard link where one can be made: it costs nothing, and
    /// since a write replaces a file by rename, never in place, the version
    /// linked never changes. A copy where it cannot, across filesystems --
    /// or across btrfs subvolumes, where `fs::copy` clones instead. Put in
    /// place by rename, so a copy is always a whole file. Best effort, like
    /// history: a copy that cannot be made never stops the write.
    fn copy_out(&self, file: &Path) {
        let Some(copy) = self.copy_of(file) else { return };
        let (Ok(source), Some(parent)) = (fs::metadata(file), copy.parent()) else { return };
        // Already this very file: a journal appended in place, or a file no
        // write has replaced since.
        if fs::metadata(&copy).is_ok_and(|kept| (kept.dev(), kept.ino()) == (source.dev(), source.ino())) {
            return;
        }
        if fs::create_dir_all(parent).is_err() {
            return;
        }
        let mut temp = copy.clone().into_os_string();
        temp.push(".new");
        let temp = PathBuf::from(temp);
        let _ = fs::remove_file(&temp);
        let placed = fs::hard_link(file, &temp).is_ok() || fs::copy(file, &temp).is_ok();
        if !placed || fs::rename(&temp, &copy).is_err() {
            let _ = fs::remove_file(&temp);
        }
    }

    /// Copies into `history/` the version of storage.json this write is about
    /// to replace, when no write kept it there: one a hand or an older ekko
    /// wrote. Every version a write makes is kept already, linked before the
    /// rename that put it in place (see `set`). A copy only reads the file,
    /// where a link would be an event on it. Best effort: a version that
    /// cannot be kept never stops the write.
    fn keep_unkept_version(&self) {
        let Ok(meta) = fs::metadata(&self.storage_file) else { return };
        let Some(name) = history_name(&meta) else { return };
        let kept = self.history_dir.join(name);
        if kept.exists() || fs::create_dir_all(&self.history_dir).is_err() {
            return;
        }
        // Whole or not at all: copied beside the board's other temp files,
        // then put in place by rename.
        let temp = temp_file_path(&kept, &self.temp_dir);
        if fs::copy(&self.storage_file, &temp).is_err() || fs::rename(&temp, &kept).is_err() {
            let _ = fs::remove_file(&temp);
        }
    }

    /// Lets go of the versions `history/` no longer keeps, on the way past
    /// each write (see `HISTORY_RECENT`). The board is kept out of git and
    /// every write replaces storage.json whole, so history is what undoes a
    /// bad write.
    fn prune_history(&self) {
        let Ok(entries) = fs::read_dir(&self.history_dir) else { return };
        let kept: Vec<u128> = entries
            .flatten()
            .filter_map(|entry| entry.file_name().to_str()?.strip_suffix(".json")?.parse().ok())
            .collect();
        for version in stale_versions(kept, SystemTime::now()) {
            let _ = fs::remove_file(self.history_dir.join(format!("{version}.json")));
        }
    }

    pub fn set_archive(&self, data: &ItemMap) -> Result<(), StorageError> {
        write_atomic(&self.archive_file, &self.temp_dir, data)?;
        self.copy_out(&self.archive_file);
        Ok(())
    }

    /// Blocks (thread::sleep between polls, not a busy spin) until the lock
    /// is acquired, or `LOCK_ACQUIRE_TIMEOUT` elapses against a holder that
    /// keeps it that long.
    ///
    /// The exclusion is `flock(2)`, held on an open descriptor for the whole
    /// critical section. That is what makes crash recovery free: the kernel
    /// drops a `flock` when the descriptor closes, and that covers every way
    /// a process can end -- normal exit, panic, SIGKILL, an unreaped zombie
    /// -- so a lock file left behind by a dead holder is already unlocked by
    /// the time anyone else looks at it. Nothing has to *detect* staleness,
    /// and nothing ever removes a lock file it doesn't hold.
    ///
    /// That last part is load-bearing. The previous design wrote the
    /// holder's pid into the file and let a waiter delete the file when that
    /// pid looked dead. Reading the pid, judging it dead, and unlinking are
    /// three separate steps, and under real contention the lock changed
    /// hands in between -- so a waiter could delete a *live* holder's lock,
    /// admitting a second writer into the critical section. Both then read
    /// the same state, derived the same next id, and one silently
    /// overwrote the other. `tests/concurrency.rs` is what caught it.
    pub fn acquire_lock(&self) -> Result<LockGuard<'_>, StorageError> {
        Ok(LockGuard { _storage: self, _file: lock_path(&self.lock_file)? })
    }

    /// The board's file, for a watcher to follow: every write replaces it.
    pub fn storage_path(&self) -> &Path {
        &self.storage_file
    }

    /// The board's directory, `.ekko/`, where files written from the board
    /// beside it go: an artifact's page (`crate::artifact`).
    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

/// What a timed-out wait tells the person or agent waiting. Never to delete
/// the lock file: the holder keeps its lock on the old inode, the next process
/// locks a new one, and two writers are in at once -- the lost update
/// `tests/concurrency.rs` exists for.
pub fn lock_timeout_advice(holder: Option<&str>) -> String {
    format!(
        "Timed out after {} s waiting for the ekko storage lock, held by {}. Let it finish, or end it if it is stuck; do not delete the lock file, which lets a second writer in while the first still holds it:",
        LOCK_ACQUIRE_TIMEOUT.as_secs(),
        holder.unwrap_or("another ekko process")
    )
}

/// Who holds the `flock` on `path`, as /proc/locks says: the process and its
/// command, the program by its file name. `None` once nothing holds it, or
/// where /proc does not say.
///
/// /proc/locks names the file by inode and by its superblock's device, which
/// on btrfs is not the device `stat` reports for a subvolume, so the device
/// is not compared: a process is named only if it also has the file open.
fn lock_holder(path: &Path) -> Option<String> {
    let meta = fs::metadata(path).ok()?;
    let same_file = |other: &fs::Metadata| other.dev() == meta.dev() && other.ino() == meta.ino();
    let inode = format!(":{}", meta.ino());
    // "1: FLOCK  ADVISORY  WRITE 4242 00:23:81 0 EOF"; a waiter's line has
    // "->" after the number, so its fields do not line up and it is skipped.
    let pid = fs::read_to_string("/proc/locks").ok()?.lines().find_map(|line| {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.get(1) != Some(&"FLOCK") || !fields.get(5).is_some_and(|file| file.ends_with(&inode)) {
            return None;
        }
        let pid = fields[4];
        let open = fs::read_dir(format!("/proc/{pid}/fd")).ok()?.flatten().any(|fd| fs::metadata(fd.path()).is_ok_and(|m| same_file(&m)));
        open.then(|| pid.to_string())
    })?;
    let args: Vec<String> = fs::read(format!("/proc/{pid}/cmdline"))
        .unwrap_or_default()
        .split(|&b| b == 0)
        .filter(|arg| !arg.is_empty())
        .map(|arg| String::from_utf8_lossy(arg).into_owned())
        .collect();
    Some(match args.split_first() {
        Some((program, rest)) => {
            let program = Path::new(program).file_name().map_or(program.clone(), |name| name.to_string_lossy().into_owned());
            format!("process {pid} ({})", std::iter::once(program).chain(rest.iter().cloned()).collect::<Vec<_>>().join(" "))
        }
        None => format!("process {pid}"),
    })
}

/// Takes an exclusive `flock` on `path`, creating the file if needed, and
/// returns the open file that holds it -- closing it is the release. The
/// storage lock is one use; the project registry's is the other, and they
/// must behave alike, which is why this is the one place the loop lives.
pub fn lock_path(path: &Path) -> Result<File, StorageError> {
    // `truncate(false)` because the file's *contents* are irrelevant now
    // -- it exists purely as something to lock.
    let file = OpenOptions::new().write(true).create(true).truncate(false).open(path)?;
    // SAFETY: `file` owns this descriptor and outlives the call.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(file);
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() != Some(libc::EWOULDBLOCK) {
        return Err(error.into());
    }
    // Held: wait in the kernel's queue rather than polling, so the lock goes
    // to a waiter the moment it is released instead of to whoever happens to
    // ask first. The wait runs on a thread that owns the descriptor and hands
    // it back; if the timeout passes first, the thread still takes the lock
    // when it comes free, finds nobody to hand it to, and drops it.
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        // SAFETY: the thread owns `file`, which outlives the call.
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
        let _ = sender.send(if result == 0 { Ok(file) } else { Err(io::Error::last_os_error()) });
    });
    match receiver.recv_timeout(LOCK_ACQUIRE_TIMEOUT) {
        Ok(Ok(file)) => Ok(file),
        Ok(Err(error)) => Err(error.into()),
        Err(_) => Err(StorageError::LockTimeout(path.to_path_buf(), lock_holder(path))),
    }
}


/// The ordered phase sequence of a project.
///
/// Kept in its own file rather than inside `storage.json`, which is a flat
/// map of numeric item ids that taskbook parses by iterating keys -- a
/// non-numeric key there would be a foreign object in someone else's
/// format. This one is Ekko's alone.
///
/// Order is the whole point and cannot be derived: "setup comes before
/// build" is knowledge, not a timestamp.
impl Storage {
    fn phases_file(&self) -> PathBuf {
        self.storage_file.with_file_name("phases.json")
    }

    /// The declared sequence, or empty when a project has none.
    pub fn get_phases(&self) -> Result<Vec<String>, StorageError> {
        let path = self.phases_file();
        if !path.exists() {
            return Ok(Vec::new());
        }
        let content = fs::read_to_string(&path)?;
        Ok(serde_json::from_str(&content)?)
    }

    /// Replaces the sequence wholesale. Written through the same temp-file
    /// and rename dance as everything else, so a reader never sees half a
    /// list.
    pub fn set_phases(&self, phases: &[String]) -> Result<(), StorageError> {
        replace_durably(&self.phases_file(), &self.temp_dir, serde_json::to_string_pretty(phases)?.as_bytes())?;
        self.copy_out(&self.phases_file());
        Ok(())
    }
}

/// Where an item that left this board for another went (task 897), kept in
/// `moved.json` beside `storage.json` for the reason `phases.json` is. The
/// display id it had here is never handed out again, so the old number keeps
/// naming it: a lookup of it, or of its uid, says where it is now, the way an
/// issue moved in Jira or transferred on GitHub redirects.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Moved {
    /// Its display id here.
    pub id: u32,
    pub uid: String,
    /// The project it went to; `None`, the default board.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Its display id there, as it arrived.
    #[serde(rename = "as")]
    pub as_id: u32,
    pub at: i64,
    /// Whatever a later version keeps here, written back as read; see
    /// `Item::unknown`.
    #[serde(flatten)]
    pub unknown: BTreeMap<String, serde_json::Value>,
}

impl Storage {
    fn moved_file(&self) -> PathBuf {
        self.storage_file.with_file_name("moved.json")
    }

    /// Every item that left, in the order they left; empty when none has.
    pub fn get_moved(&self) -> Result<Vec<Moved>, StorageError> {
        let path = self.moved_file();
        if !path.exists() {
            return Ok(Vec::new());
        }
        Ok(serde_json::from_str(&fs::read_to_string(&path)?)?)
    }

    /// Written through the same temp-file and rename dance as everything else.
    pub fn set_moved(&self, moved: &[Moved]) -> Result<(), StorageError> {
        replace_durably(&self.moved_file(), &self.temp_dir, serde_json::to_string_pretty(moved)?.as_bytes())?;
        self.copy_out(&self.moved_file());
        Ok(())
    }
}

/// The board's counters, kept in `counters.json` beside `storage.json` for
/// the reason `phases.json` is: storage is a flat map of numeric item ids that
/// taskbook iterates, with no room for board-level fields.
///
/// - `revision` rises by one on every write that changes the board, and that
///   write stamps it on the items it changed: a cursor that cannot tie, repeat
///   an item, or depend on a clock.
/// - `highest_id` is the largest display id storage has held, so a number is
///   never handed out again once its item has left.
///
/// A board with no file yet reads as both at zero, and its next write sets
/// them right.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Counters {
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub highest_id: u32,
    /// Whatever a later version keeps here that this one does not know,
    /// written back as read -- every write rewrites this file, and would
    /// otherwise drop it, for the reason `Item::unknown` exists.
    #[serde(flatten)]
    pub unknown: BTreeMap<String, serde_json::Value>,
}

impl Storage {
    fn counters_file(&self) -> PathBuf {
        self.storage_file.with_file_name("counters.json")
    }

    pub fn get_counters(&self) -> Result<Counters, StorageError> {
        let path = self.counters_file();
        if !path.exists() {
            return Ok(Counters::default());
        }
        Ok(serde_json::from_str(&fs::read_to_string(&path)?)?)
    }

    /// Written through the same temp-file and rename dance as everything else.
    pub fn set_counters(&self, counters: &Counters) -> Result<(), StorageError> {
        replace_durably(&self.counters_file(), &self.temp_dir, serde_json::to_string_pretty(counters)?.as_bytes())?;
        self.copy_out(&self.counters_file());
        Ok(())
    }
}

/// How much journal a board keeps: past twice this, the oldest entries go.
const JOURNAL_BYTES: u64 = 256 * 1024;

/// What a write did that the items it leaves cannot show, one JSON object per
/// line in `journal.jsonl` beside `storage.json`: the tasks it set free or left
/// waiting, whose own fields did not change, and the items it took out of
/// storage, which are gone. `changes` reads it to tell a cursor about both.
///
/// When the journal is trimmed its first line becomes `{"from": rev}`, the
/// oldest revision still described, so a cursor older than that is told the
/// journal no longer reaches it instead of being given a partial answer.
impl Storage {
    fn journal_file(&self) -> PathBuf {
        self.storage_file.with_file_name("journal.jsonl")
    }

    /// Every entry, oldest first; a line that does not parse is skipped.
    pub fn read_journal(&self) -> Result<Vec<serde_json::Value>, StorageError> {
        let path = self.journal_file();
        if !path.exists() {
            return Ok(Vec::new());
        }
        Ok(fs::read_to_string(&path)?.lines().filter_map(|line| serde_json::from_str(line).ok()).collect())
    }

    /// The revision on the journal's last line, the newest it records, or 0
    /// with no journal. Read from the end, a few kilobytes at a time, so a
    /// write pays for one line and not for the whole journal.
    pub fn last_journal_rev(&self) -> Result<u64, StorageError> {
        use std::io::{Read as _, Seek as _, SeekFrom};
        let mut file = match File::open(self.journal_file()) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
            Err(error) => return Err(error.into()),
        };
        let len = file.metadata()?.len();
        let mut window = 4096;
        loop {
            let start = len.saturating_sub(window);
            file.seek(SeekFrom::Start(start))?;
            let mut tail = Vec::new();
            file.read_to_end(&mut tail)?;
            // A line is only whole past the first newline, unless the window
            // reaches back to the start of the file.
            let whole = if start == 0 { &tail[..] } else { tail.splitn(2, |&b| b == b'\n').nth(1).unwrap_or(&[]) };
            let rev = whole
                .split(|&b| b == b'\n')
                .rev()
                .filter_map(|line| serde_json::from_slice::<serde_json::Value>(line).ok())
                .find_map(|entry| entry["rev"].as_u64());
            if let Some(rev) = rev {
                return Ok(rev);
            }
            if start == 0 {
                return Ok(0);
            }
            window *= 4;
        }
    }

    /// Appends one entry. Callers hold the lock.
    pub fn append_journal(&self, entry: &serde_json::Value) -> Result<(), StorageError> {
        self.append_journal_within(entry, JOURNAL_BYTES)?;
        self.copy_out(&self.journal_file());
        Ok(())
    }

    fn append_journal_within(&self, entry: &serde_json::Value, bytes: u64) -> Result<(), StorageError> {
        use std::io::Write as _;
        let path = self.journal_file();
        let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
        writeln!(file, "{entry}")?;
        if file.metadata()?.len() <= bytes * 2 {
            return Ok(());
        }
        drop(file);

        let entries: Vec<serde_json::Value> =
            self.read_journal()?.into_iter().filter(|entry| entry.get("from").is_none()).collect();
        let mut kept: Vec<String> = Vec::new();
        let mut size = 0;
        for entry in entries.iter().rev() {
            let line = entry.to_string();
            size += line.len() as u64 + 1;
            if size > bytes {
                break;
            }
            kept.push(line);
        }
        kept.reverse();
        let from = kept.first().and_then(|line| serde_json::from_str::<serde_json::Value>(line).ok()?["rev"].as_i64());
        let mut content = format!("{}\n", serde_json::json!({"from": from.unwrap_or(0)}));
        for line in kept {
            content.push_str(&line);
            content.push('\n');
        }
        replace_durably(&path, &self.temp_dir, content.as_bytes()).map(|_| ())
    }
}

/// Of the versions in history, by modification time in nanoseconds, the ones
/// to let go of at `now`: past the newest `HISTORY_RECENT`, all but the
/// newest of each day, and every one from before the last `HISTORY_DAYS`
/// days. A day is the calendar's, in UTC. Counted back from `now` instead, a
/// version shared its day with every newer one written within 24 hours of
/// it, so a board written every day pruned each version before it could
/// become a day of its own, and kept nothing past the newest 50 (task 640).
fn stale_versions(mut versions: Vec<u128>, now: SystemTime) -> Vec<u128> {
    const DAY: u128 = 86_400_000_000_000;
    let today = now.duration_since(UNIX_EPOCH).map(|at| at.as_nanos()).unwrap_or(0) / DAY;
    versions.sort_unstable_by(|a, b| b.cmp(a));
    let mut days_kept = Vec::new();
    let mut stale = Vec::new();
    for version in versions.into_iter().skip(HISTORY_RECENT) {
        let day = version / DAY;
        if today.saturating_sub(day) < u128::from(HISTORY_DAYS) && !days_kept.contains(&day) {
            days_kept.push(day);
        } else {
            stale.push(version);
        }
    }
    stale
}

fn read_map(path: &Path) -> Result<ItemMap, StorageError> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }

    let content = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&content)?)
}

/// Which version of a file an open descriptor holds. Every write replaces the
/// file by rename, so a new version is a new inode; the modification time and
/// the size catch an edit made in place by something else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Version {
    dev: u64,
    ino: u64,
    mtime: i64,
    mtime_nsec: i64,
    len: u64,
}

impl Version {
    fn of(meta: &fs::Metadata) -> Version {
        Version { dev: meta.dev(), ino: meta.ino(), mtime: meta.mtime(), mtime_nsec: meta.mtime_nsec(), len: meta.len() }
    }
}

/// The file open, with the version the descriptor holds -- read from the same
/// descriptor the content will be, so the two cannot disagree -- or `None`
/// when the board has no file yet.
fn open_versioned(path: &Path) -> Result<Option<(File, Version)>, StorageError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let version = Version::of(&file.metadata()?);
    Ok(Some((file, version)))
}

#[cfg(test)]
thread_local! {
    /// The boards this thread parsed: a test's own reads, whatever the tests
    /// running beside it read.
    static PARSED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn parse_map(mut file: File) -> Result<ItemMap, StorageError> {
    #[cfg(test)]
    PARSED.with(|parsed| parsed.set(parsed.get() + 1));
    let mut content = String::new();
    file.read_to_string(&mut content)?;
    Ok(serde_json::from_str(&content)?)
}

/// Boards this process has parsed, by storage file: the latest version of
/// each, and only a few files, since a process reads one board or a handful.
const BOARDS_KEPT: usize = 8;
static BOARDS: Mutex<Vec<(PathBuf, Version, Arc<ItemMap>)>> = Mutex::new(Vec::new());

fn kept_board(path: &Path, version: Version) -> Option<Arc<ItemMap>> {
    let boards = BOARDS.lock().unwrap_or_else(PoisonError::into_inner);
    boards.iter().find(|(file, kept, _)| file == path && *kept == version).map(|(_, _, board)| Arc::clone(board))
}

fn keep_board(path: &Path, version: Version, board: Arc<ItemMap>) {
    let mut boards = BOARDS.lock().unwrap_or_else(PoisonError::into_inner);
    boards.retain(|(file, _, _)| file != path);
    if boards.len() >= BOARDS_KEPT {
        boards.remove(0);
    }
    boards.push((path.to_path_buf(), version, board));
}

fn write_atomic(path: &Path, temp_dir: &Path, data: &ItemMap) -> Result<Version, StorageError> {
    replace_durably(path, temp_dir, json::to_pretty_string(data)?.as_bytes())
}

/// Replaces `path` with `content` so that a reader sees the old file or the
/// new one, never half of one, and so the new one survives a crash: the temp
/// file is synced before the rename and its directory after. Without the
/// first sync, a crash just after the rename can leave an empty file on a
/// filesystem that allocates lazily, which for storage.json is the whole
/// board. Answers with the version written: the temp file's, read before the
/// rename, which carries its inode, times and length over to `path`.
fn replace_durably(path: &Path, temp_dir: &Path, content: &[u8]) -> Result<Version, StorageError> {
    replace_durably_keeping(path, temp_dir, content, None)
}

/// `replace_durably`, linking the new version into `history` too, while it
/// is still the temp file: once it is `path`, the link would be an event on
/// the file a watcher follows (see `Storage::set`). A link the rename never
/// followed is taken back.
fn replace_durably_keeping(path: &Path, temp_dir: &Path, content: &[u8], history: Option<&Path>) -> Result<Version, StorageError> {
    use std::io::Write as _;
    let temp_file = temp_file_path(path, temp_dir);
    let mut file = File::create(&temp_file)?;
    file.write_all(content)?;
    file.sync_all()?;
    let meta = file.metadata()?;
    let version = Version::of(&meta);
    drop(file);
    let kept = history.and_then(|history| link_into(history, &temp_file, &meta));
    if let Err(error) = fs::rename(&temp_file, path) {
        if let Some(kept) = kept {
            let _ = fs::remove_file(kept);
        }
        return Err(error.into());
    }
    // Best effort: some filesystems refuse to sync a directory, and the
    // rename itself has already happened.
    if let Some(dir) = path.parent().and_then(|parent| File::open(parent).ok()) {
        let _ = dir.sync_all();
    }
    Ok(version)
}

/// Links `file`, a version of storage.json whose metadata is `meta`, into
/// `history`. The link, when one was made.
fn link_into(history: &Path, file: &Path, meta: &fs::Metadata) -> Option<PathBuf> {
    let kept = history.join(history_name(meta)?);
    (fs::create_dir_all(history).is_ok() && fs::hard_link(file, &kept).is_ok()).then_some(kept)
}

/// The name `history/` keeps a version under: its modification time in
/// nanoseconds, which orders the versions.
fn history_name(meta: &fs::Metadata) -> Option<String> {
    Some(format!("{}.json", meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_nanos()))
}

/// pid + nanosecond timestamp instead of the JS version's random hex --
/// unique enough for a temp filename without pulling in a `rand` dependency.
fn temp_file_path(target: &Path, temp_dir: &Path) -> PathBuf {
    let stem = target.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
    let ext = target.extension().and_then(|s| s.to_str());
    let unique = format!(
        "{}-{}",
        process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
    );

    let filename = match ext {
        Some(ext) => format!("{stem}.TEMP-{unique}.{ext}"),
        None => format!("{stem}.TEMP-{unique}"),
    };

    temp_dir.join(filename)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::time::Instant;

    fn temp_ekko_dir() -> PathBuf {
        let dir = crate::paths::test_dir("ekko-test");
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample_item(id: u32) -> Item {
        Item::new_task(id, format!("item {id}"), vec!["@x".into()], 1)
    }

    #[test]
    fn get_on_a_fresh_dir_is_an_empty_map() {
        let dir = temp_ekko_dir();
        let storage = Storage::new(&dir).unwrap();

        assert_eq!(storage.get().unwrap(), BTreeMap::new());
        assert_eq!(storage.get_archive().unwrap(), BTreeMap::new());

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn set_then_get_round_trips() {
        let dir = temp_ekko_dir();
        let storage = Storage::new(&dir).unwrap();

        let mut data = BTreeMap::new();
        data.insert(1, sample_item(1));
        data.insert(2, sample_item(2));
        storage.set(&data).unwrap();

        assert_eq!(storage.get().unwrap(), data);
        let file = dir.join("storage").join("storage.json");
        assert_eq!(read_map(&file).unwrap(), data, "the file itself, which the read above no longer parses");

        fs::remove_dir_all(&dir).ok();
    }

    /// The version a write made is the map it wrote, so the read after it
    /// parses nothing (task 859); and a version another process wrote is
    /// parsed, though this one read the board just before.
    #[test]
    fn a_write_is_read_back_without_a_parse() {
        let dir = temp_ekko_dir();
        let storage = Storage::new(&dir).unwrap();
        let file = dir.join("storage").join("storage.json");
        let parsed = || PARSED.with(std::cell::Cell::get);
        let mut data = BTreeMap::from([(1, sample_item(1))]);
        storage.set(&data).unwrap();

        let before = parsed();
        assert_eq!(*storage.get_shared().unwrap(), data);
        assert_eq!(parsed(), before, "its own write");

        data.insert(2, sample_item(2));
        let theirs = dir.join("theirs.json");
        fs::write(&theirs, json::to_pretty_string(&data).unwrap()).unwrap();
        fs::rename(&theirs, &file).unwrap();
        assert_eq!(*storage.get_shared().unwrap(), data);
        assert_eq!(parsed(), before + 1, "another's write");

        fs::remove_dir_all(&dir).ok();
    }

    /// What a write keeps must be what a parse of its file gives back, or a
    /// session would read its own write one way and every other process
    /// another. Each kind of item a session writes, through the operations
    /// that write it, and a field from a later version.
    #[test]
    fn a_parse_of_the_written_board_gives_back_the_map_written() {
        let (me, _, _) = crate::holder::test_sessions();
        let dir = temp_ekko_dir();
        let storage = Storage::new(&dir).unwrap();
        let mut later = sample_item(1);
        later.unknown.insert("fromALaterVersion".into(), serde_json::json!({"kept": [1, "two", null, true]}));
        storage.set(&BTreeMap::from([(1, later)])).unwrap();
        storage.set_counters(&Counters { revision: 1, highest_id: 1, ..Counters::default() }).unwrap();
        storage.set_phases(&["alpha".to_string()]).unwrap();

        let ekko = crate::ekko::Ekko::new(Storage::new(&dir).unwrap()).acting_as(me);
        let mut draft = crate::ops::Draft::open(&ekko).unwrap();
        let ops = [
            serde_json::json!({"op": "create", "text": "a task", "boards": ["@coding"], "priority": 3, "due": "2026-10-08",
                "with": "rodrigo", "phase": "alpha", "starred": true}),
            serde_json::json!({"op": "create", "text": "after it", "blocked_by": [2]}),
            serde_json::json!({"op": "create", "kind": "decision", "text": "why", "attached_to": 2}),
            serde_json::json!({"op": "create", "kind": "gotcha", "text": "a trap"}),
            serde_json::json!({"op": "create", "kind": "procedure", "text": "the steps"}),
            serde_json::json!({"op": "create", "kind": "handoff", "text": "where it stopped", "attached_to": 3}),
            serde_json::json!({"op": "create", "text": "put away"}),
            serde_json::json!({"op": "create", "text": "thrown away"}),
            serde_json::json!({"op": "set_state", "items": [2], "state": "progress"}),
            serde_json::json!({"op": "set_state", "items": [1], "state": "done"}),
        ];
        for op in ops {
            draft.apply(&serde_json::from_value(op).unwrap()).unwrap();
        }
        let asked = draft.ask("Merge now?", Some(&crate::ops::Ref::Id(2))).unwrap();
        let spec = crate::ops::WaitOn { item: crate::ops::Ref::Id(3), until: None, text: Some("then merge".into()), cancel: false };
        draft.wait(&spec).unwrap();
        draft.commit(false).unwrap();
        let person = crate::ekko::Ekko::new(Storage::new(&dir).unwrap()).acting_as(crate::holder::Actor::person());
        person.answer_question(&[asked.to_string(), "yes".to_string()]).unwrap();
        person.set_stashed(&["8".to_string()], true).unwrap();
        person.set_trashed(&["9".to_string()], true).unwrap();

        let mut draft = crate::ops::Draft::open(&ekko).unwrap();
        draft.apply(&serde_json::from_value(serde_json::json!({"op": "set_state", "items": [3], "state": "paused"})).unwrap()).unwrap();
        let written = draft.commit(false).unwrap().data;
        let fields = |data: &ItemMap| serde_json::to_value(data).unwrap().to_string();
        for field in ["fromALaterVersion", "heldBy", "doneBy", "createdBy", "question", "answer", "wait", "stashed", "trashed", "phase"] {
            assert!(fields(&written).contains(&format!("\"{field}\"")), "the board holds {field}");
        }
        assert_eq!(read_map(&dir.join("storage").join("storage.json")).unwrap(), written);

        fs::remove_dir_all(&dir).ok();
    }

    /// A version already read is handed out again without parsing, and a write
    /// -- a rename, so a new inode -- is never answered from the version it
    /// replaced, by this process's reads or another `Storage` on the same file.
    #[test]
    fn get_shared_reuses_a_version_until_a_write_replaces_it() {
        let dir = temp_ekko_dir();
        let storage = Storage::new(&dir).unwrap();
        let mut data = BTreeMap::new();
        data.insert(1, sample_item(1));
        storage.set(&data).unwrap();

        let first = storage.get_shared().unwrap();
        let again = Storage::new(&dir).unwrap().get_shared().unwrap();
        assert!(Arc::ptr_eq(&first, &again), "the same version is parsed once");
        assert_eq!(storage.get().unwrap(), *first, "a copy to change is the same board");

        data.insert(2, sample_item(2));
        storage.set(&data).unwrap();
        let after = storage.get_shared().unwrap();
        assert!(!Arc::ptr_eq(&first, &after));
        assert_eq!(*after, data);
        assert_eq!(storage.get().unwrap(), data);

        fs::remove_dir_all(&dir).ok();
    }

    /// An edit made in place by something else -- same inode -- still shows.
    #[test]
    fn get_shared_sees_an_edit_made_in_place() {
        let dir = temp_ekko_dir();
        let storage = Storage::new(&dir).unwrap();
        let mut data = BTreeMap::new();
        data.insert(1, sample_item(1));
        storage.set(&data).unwrap();
        let first = storage.get_shared().unwrap();

        data.insert(2, sample_item(2));
        let file = dir.join("storage").join("storage.json");
        let mut handle = OpenOptions::new().write(true).truncate(true).open(&file).unwrap();
        use std::io::Write as _;
        handle.write_all(json::to_pretty_string(&data).unwrap().as_bytes()).unwrap();
        drop(handle);

        let after = storage.get_shared().unwrap();
        assert!(!Arc::ptr_eq(&first, &after));
        assert_eq!(*after, data);

        fs::remove_dir_all(&dir).ok();
    }
    #[test]
    fn counters_read_as_zero_until_written_and_round_trip() {
        let dir = temp_ekko_dir();
        let storage = Storage::new(&dir).unwrap();

        assert_eq!(storage.get_counters().unwrap(), Counters::default());
        storage.set_counters(&Counters { revision: 7, highest_id: 42, ..Counters::default() }).unwrap();
        assert_eq!(storage.get_counters().unwrap(), Counters { revision: 7, highest_id: 42, ..Counters::default() });
        assert!(dir.join("storage").join("counters.json").exists());

        fs::remove_dir_all(&dir).ok();
    }

    /// A counter a later version keeps survives this version's rewrite of
    /// counters.json, which every write makes.
    #[test]
    fn counters_keep_what_a_later_version_added() {
        let dir = temp_ekko_dir();
        let storage = Storage::new(&dir).unwrap();
        let file = dir.join("storage").join("counters.json");
        fs::write(&file, r#"{"revision": 3, "highestId": 5, "journalFrom": 2}"#).unwrap();

        let mut counters = storage.get_counters().unwrap();
        counters.revision += 1;
        storage.set_counters(&counters).unwrap();

        let written: serde_json::Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(written, serde_json::json!({"revision": 4, "highestId": 5, "journalFrom": 2}));

        fs::remove_dir_all(&dir).ok();
    }

    /// Where an item went reads as none until an item leaves, and a later
    /// version's fields on it survive this version's rewrite of moved.json.
    #[test]
    fn moved_reads_as_none_until_written_and_keeps_what_a_later_version_added() {
        let dir = temp_ekko_dir();
        let storage = Storage::new(&dir).unwrap();
        assert!(storage.get_moved().unwrap().is_empty());
        crate::json::assert_keeps_what_it_does_not_know::<Moved>(serde_json::json!({
            "id": 3, "uid": "18d99814e1b8c099-1a6885", "project": "notes", "as": 7, "at": 1790629542310_i64
        }));
        let file = dir.join("storage").join("moved.json");
        fs::write(&file, r#"[{"id": 3, "uid": "u", "as": 7, "at": 1, "reason": "later"}]"#).unwrap();
        let moved = storage.get_moved().unwrap();
        assert_eq!((moved[0].project.as_deref(), moved[0].as_id), (None, 7), "no project: the default board");
        storage.set_moved(&moved).unwrap();
        let written: serde_json::Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(written, serde_json::json!([{"id": 3, "uid": "u", "as": 7, "at": 1, "reason": "later"}]));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_journal_appends_in_order_and_trims_to_what_it_keeps() {
        let dir = temp_ekko_dir();
        let storage = Storage::new(&dir).unwrap();
        assert!(storage.read_journal().unwrap().is_empty());

        for rev in 1..=40 {
            storage.append_journal_within(&serde_json::json!({"rev": rev}), 100).unwrap();
        }
        let kept = storage.read_journal().unwrap();
        let revs: Vec<i64> = kept.iter().filter_map(|entry| entry["rev"].as_i64()).collect();
        assert!(revs.len() < 40 && revs.last() == Some(&40), "{revs:?}");
        assert!(revs.windows(2).all(|pair| pair[1] == pair[0] + 1), "{revs:?}");
        assert_eq!(kept[0]["from"].as_i64(), Some(revs[0]), "a trimmed journal says where it starts");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_lock_file_is_never_unlinked_and_the_lock_is_retakeable() {
        let dir = temp_ekko_dir();
        let storage = Storage::new(&dir).unwrap();
        let lock_file = dir.join(".lock");

        {
            let _guard = storage.acquire_lock().unwrap();
            assert!(lock_file.exists());
        }

        // Deliberately still on disk. The lock lives in the kernel, attached
        // to the open descriptor -- not in the file existing or in anything
        // written inside it. Unlinking on release is what would reintroduce
        // the old hole: the next process would create a fresh inode and lock
        // *that* while an existing holder still had the old one.
        assert!(lock_file.exists(), "the lock file must outlive the guard");

        // And releasing really did release.
        let _again = storage.acquire_lock().unwrap();

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn two_guards_in_one_process_still_exclude_each_other() {
        // `flock` is held per open file description, not per process, so
        // this is a real exclusion test and not a tautology -- it would
        // fail if `acquire_lock` ever started reusing one descriptor.
        let dir = temp_ekko_dir();
        let storage = Storage::new(&dir).unwrap();

        let _held = storage.acquire_lock().unwrap();
        let second = storage.acquire_lock();

        assert!(matches!(second, Err(StorageError::LockTimeout(..))));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_lock_file_left_behind_by_a_dead_holder_is_acquired_immediately() {
        let dir = temp_ekko_dir();
        let storage = Storage::new(&dir).unwrap();
        // Exactly what a crashed holder leaves: the file, with whatever it
        // happened to contain. The kernel dropped its flock when it died, so
        // the leftover file means nothing on its own -- and unlike the old
        // pid-file scheme, nothing here has to work that out.
        fs::write(dir.join(".lock"), "999999999").unwrap();

        let start = Instant::now();
        let _guard = storage.acquire_lock().unwrap();
        let elapsed = start.elapsed();

        assert!(elapsed < Duration::from_millis(500), "expected near-instant recovery, took {elapsed:?}");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn waits_out_a_real_external_process_then_succeeds() {
        let dir = temp_ekko_dir();
        let storage = Storage::new(&dir).unwrap();

        // A real, separate OS process -- not a simulation -- standing in for
        // another `ekko` invocation holding the lock.
        let mut holder = spawn_lock_holder(&dir.join(".lock"), "1");

        let start = Instant::now();
        let _guard = storage.acquire_lock().unwrap();
        let elapsed = start.elapsed();

        assert!(elapsed >= Duration::from_millis(300), "should have waited for the real holder, only waited {elapsed:?}");
        holder.wait().ok();
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_unreaped_zombie_holder_does_not_keep_the_lock() {
        let dir = temp_ekko_dir();
        let storage = Storage::new(&dir).unwrap();

        // Deliberately not reaped. A zombie still has a pid and still
        // answers `kill(pid, 0)`, which is what made it a hazard for the old
        // pid-based scheme -- it looked alive. Its descriptors are gone
        // though, so its flock went with them.
        let mut holder = spawn_lock_holder(&dir.join(".lock"), "0.2");
        std::thread::sleep(Duration::from_millis(500)); // let it exit and zombify

        let start = Instant::now();
        let _guard = storage.acquire_lock().unwrap();
        let elapsed = start.elapsed();

        assert!(elapsed < Duration::from_millis(500), "expected near-instant recovery from a zombie holder, took {elapsed:?}");
        holder.wait().ok();
        fs::remove_dir_all(&dir).ok();
    }
    #[test]
    fn fresh_temp_files_survive_a_new_storage_construction() {
        // Same root cause class as the lock-file test below: cleanup that
        // runs unconditionally (here, on every `Storage::new`, unlocked,
        // since read-only commands must stay lock-free) must not delete
        // something another live process is still in the middle of
        // writing.
        let dir = temp_ekko_dir();
        Storage::new(&dir).unwrap();
        let fresh = dir.join(".temp").join("storage.TEMP-fake.json");
        fs::write(&fresh, "in-progress-write").unwrap();

        Storage::new(&dir).unwrap(); // re-runs clean_temp_dir()

        assert!(fresh.exists(), "a fresh temp file must not be swept up as abandoned");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn old_abandoned_temp_files_are_swept_up() {
        let dir = temp_ekko_dir();
        Storage::new(&dir).unwrap();
        let old = dir.join(".temp").join("storage.TEMP-fake.json");
        fs::write(&old, "abandoned").unwrap();
        let old_time = std::time::SystemTime::now() - TEMP_FILE_ABANDONED - Duration::from_secs(1);
        filetime_set(&old, old_time);

        Storage::new(&dir).unwrap();

        assert!(!old.exists(), "an old abandoned temp file should be cleaned up");
        fs::remove_dir_all(&dir).ok();
    }

    /// Backdates a file's mtime. No `filetime` dependency needed just for
    /// this one test -- `std::fs::File::set_modified` already does it.
    fn filetime_set(path: &Path, time: std::time::SystemTime) {
        let file = fs::File::options().write(true).open(path).unwrap();
        file.set_modified(time).unwrap();
    }

    /// Spawns a real, separate process that holds the lock via `flock(1)`
    /// for `secs`, and returns once the lock is observably taken -- so a
    /// caller timing an `acquire_lock` isn't racing the child's startup.
    fn spawn_lock_holder(lock_file: &Path, secs: &str) -> process::Child {
        let mut child = Command::new("flock")
            .arg("-x")
            .arg(lock_file)
            .arg("sleep")
            .arg(secs)
            .spawn()
            .expect("flock(1) from util-linux is required by these tests");

        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            let probe = fs::OpenOptions::new().write(true).create(true).truncate(false).open(lock_file).unwrap();
            let taken = unsafe { libc::flock(probe.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0;
            if taken {
                return child;
            }
            drop(probe); // releases our probe lock before retrying
            std::thread::sleep(Duration::from_millis(10));
        }

        child.kill().ok();
        child.wait().ok();
        panic!("the flock(1) holder never took the lock");
    }

    #[test]
    fn times_out_against_a_holder_that_outlives_the_timeout() {
        let dir = temp_ekko_dir();
        let storage = Storage::new(&dir).unwrap();

        let mut holder = spawn_lock_holder(&dir.join(".lock"), "8");

        let result = storage.acquire_lock();

        // The message names the flock(1) process, never "delete this file".
        let Err(StorageError::LockTimeout(_, named)) = &result else { panic!("expected a timeout") };
        assert_eq!(named.as_deref(), Some(format!("process {} (flock -x {} sleep 8)", holder.id(), dir.join(".lock").display()).as_str()));
        assert!(!result.err().unwrap().to_string().contains("delete this file"));
        holder.kill().ok();
        holder.wait().ok();
        fs::remove_dir_all(&dir).ok();
    }

    /// The newest revision comes off the journal's last line, even one longer
    /// than the first window read from the end.
    #[test]
    fn the_last_journal_revision_is_read_past_a_long_last_line() {
        let dir = temp_ekko_dir();
        let storage = Storage::new(&dir).unwrap();
        assert_eq!(storage.last_journal_rev().unwrap(), 0, "no journal yet");

        storage.append_journal(&serde_json::json!({"rev": 3})).unwrap();
        storage.append_journal(&serde_json::json!({"rev": 4, "removed": [{"text": "x".repeat(20_000)}]})).unwrap();

        assert_eq!(storage.last_journal_rev().unwrap(), 4);
        fs::remove_dir_all(&dir).ok();
    }

    /// History keeps each version a write makes, the board's current one
    /// among them, as a link to the very file; and a version no ekko wrote,
    /// a hand's or an older ekko's, byte for byte once a write replaces it.
    #[test]
    fn history_keeps_each_version_written_and_any_other_it_replaces() {
        let dir = temp_ekko_dir();
        let storage = Storage::new(&dir).unwrap();
        let file = dir.join("storage").join("storage.json");
        let history = || -> Vec<PathBuf> { fs::read_dir(dir.join("history")).unwrap().map(|entry| entry.unwrap().path()).collect() };
        let linked = |kept: &[PathBuf]| kept.iter().any(|path| fs::metadata(path).unwrap().ino() == fs::metadata(&file).unwrap().ino());
        storage.set(&BTreeMap::new()).unwrap();
        let first = fs::read(&file).unwrap();
        assert_eq!(history().len(), 1, "the first version");
        assert!(linked(&history()), "the version written, linked");

        let hand = dir.join("storage").join("hand.json");
        fs::write(&hand, b"{\"by\": \"hand\"}\n").unwrap();
        fs::rename(&hand, &file).unwrap();
        let mut data = BTreeMap::new();
        data.insert(1, Item::new_task(1, "kept".into(), vec!["My Board".into()], 1));
        storage.set(&data).unwrap();

        let kept = history();
        assert_eq!(kept.len(), 3, "the first version, the hand's and the new one");
        let texts: Vec<Vec<u8>> = kept.iter().map(|path| fs::read(path).unwrap()).collect();
        assert!(texts.contains(&first), "the first version, whole");
        assert!(texts.contains(&b"{\"by\": \"hand\"}\n".to_vec()), "the hand's version, whole");
        assert!(linked(&kept), "the version written, linked");
        fs::remove_dir_all(&dir).ok();
    }

    /// Claude Code's FileChanged follows storage.json by its inode, through
    /// chokidar: at an event on it, the watch stats the path and moves to the
    /// inode the path names now, but it drops an event that comes within 5 ms
    /// of the one before. So the first event a write gives the version it
    /// replaces must come from the rename that replaces it. History's link
    /// to that version came about 10 ms before the rename, and the watch went
    /// deaf whenever the rename's event was dropped (task 1248). Read here
    /// as the watch reads it, for all a write does: history kept and pruned,
    /// the board copied out, and a version no ekko wrote.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_write_touches_the_version_it_replaces_only_by_replacing_it() {
        let dir = temp_ekko_dir();
        fs::write(dir.join("project.json"), r#"{"name": "site", "id": "abc"}"#).unwrap();
        let storage = Storage::new(&dir).unwrap().copied_to(Some(temp_ekko_dir().join("copy")));
        let file = dir.join("storage").join("storage.json");
        let mut data = BTreeMap::new();
        storage.set(&data).unwrap();
        // Old enough to be pruned, so the writes below prune them.
        fs::create_dir_all(dir.join("history")).unwrap();
        for old in 0..60u128 {
            fs::write(dir.join("history").join(format!("{}.json", 1_000_000_000_000 + old)), "{}").unwrap();
        }
        for write in 1..=5 {
            if write == 5 {
                let hand = dir.join("storage").join("hand.json");
                fs::write(&hand, fs::read(&file).unwrap()).unwrap();
                fs::rename(&hand, &file).unwrap();
            }
            let replaced = fs::metadata(&file).unwrap().ino();
            let first = inode_at_first_event(&file);
            data.insert(write, sample_item(write));
            storage.set(&data).unwrap();
            let named = first.join().unwrap();
            assert!(named.is_some(), "write {write}: no event on the version it replaced");
            assert_ne!(named, Some(replaced), "write {write}: an event on the version it replaces came while storage.json still named it");
        }
        assert!(fs::read_dir(dir.join("history")).unwrap().count() < 60, "the writes pruned history");
        fs::remove_dir_all(&dir).ok();
    }

    /// The inode `file` names when the first event comes on the inode it
    /// names now, as a watch that stats the path at each event sees it: with
    /// the events Claude Code's watch asks for (mask c06 in its fdinfo).
    /// `None` when no event comes within 5 s. Armed when it returns.
    #[cfg(target_os = "linux")]
    fn inode_at_first_event(file: &Path) -> std::thread::JoinHandle<Option<u64>> {
        use std::os::unix::ffi::OsStrExt as _;
        let path = std::ffi::CString::new(file.as_os_str().as_bytes()).unwrap();
        let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC) };
        assert!(fd >= 0, "inotify_init1");
        let mask = libc::IN_MODIFY | libc::IN_ATTRIB | libc::IN_DELETE_SELF | libc::IN_MOVE_SELF;
        assert!(unsafe { libc::inotify_add_watch(fd, path.as_ptr(), mask) } >= 0, "inotify_add_watch");
        let file = file.to_path_buf();
        std::thread::spawn(move || {
            let mut ready = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
            let named = (unsafe { libc::poll(&mut ready, 1, 5_000) } == 1).then(|| fs::metadata(&file).unwrap().ino());
            unsafe { libc::close(fd) };
            named
        })
    }

    /// History keeps the newest 50 versions, then the newest of each day for
    /// two weeks, and nothing older.
    #[test]
    fn history_keeps_recent_versions_and_one_a_day() {
        const MINUTE: u128 = 60_000_000_000;
        const DAY: u128 = 1_440 * MINUTE;
        let now = UNIX_EPOCH + Duration::from_secs(1_790_000_000);
        let at = now.duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let recent: Vec<u128> = (0..60).map(|k| at - k * MINUTE).collect();
        let older = vec![at - 2 * DAY, at - 2 * DAY - MINUTE, at - 3 * DAY, at - 20 * DAY];
        let versions: Vec<u128> = recent.iter().chain(&older).copied().collect();

        let mut stale = stale_versions(versions, now);
        stale.sort_unstable();
        let mut expected: Vec<u128> = recent[51..].to_vec();
        expected.extend([at - 2 * DAY - MINUTE, at - 20 * DAY]);
        expected.sort_unstable();
        assert_eq!(stale, expected, "past the newest 50, one a day for 14 days");
    }

    /// Pruned at every write, as `prune_history` prunes it, history still ends
    /// up with each earlier day's last version: nothing newer of that day is
    /// ever written to replace it. Days counted back from the write lost
    /// them all (task 640).
    #[test]
    fn history_pruned_write_by_write_keeps_each_days_last_version() {
        const MINUTE: u128 = 60_000_000_000;
        const DAY: u128 = 1_440 * MINUTE;
        let midnight = 20_000 * DAY;
        let mut kept: Vec<u128> = Vec::new();
        for write in 0..5 * 144 {
            let at = midnight + write * 10 * MINUTE;
            kept.push(at);
            let now = UNIX_EPOCH + Duration::from_nanos(u64::try_from(at).unwrap());
            let stale = stale_versions(kept.clone(), now);
            kept.retain(|version| !stale.contains(version));
        }

        let last_of_each_day: Vec<u128> = (1..5).map(|day| midnight + day * DAY - 10 * MINUTE).collect();
        for last in &last_of_each_day {
            assert!(kept.contains(last), "day {} kept its last version", (last - midnight) / DAY);
        }
        assert_eq!(kept.len(), HISTORY_RECENT + 1 + last_of_each_day.len(), "the newest 50, today's newest past them, one per earlier day");
    }

    /// Every write copies the file it wrote outside the board's directory, in
    /// the board's own layout, so the board outlives its folder (task 503).
    #[test]
    fn every_write_copies_the_board_outside_its_folder() {
        let dir = temp_ekko_dir();
        let copy = temp_ekko_dir().join("copy");
        fs::write(dir.join("project.json"), r#"{"name": "site", "id": "abc"}"#).unwrap();
        let storage = Storage::new(&dir).unwrap().copied_to(Some(copy.clone()));
        storage.set(&BTreeMap::from([(1, sample_item(1))])).unwrap();
        storage.set_counters(&Counters { revision: 1, highest_id: 1, ..Counters::default() }).unwrap();
        storage.append_journal(&serde_json::json!({"rev": 1})).unwrap();
        storage.append_journal(&serde_json::json!({"rev": 2})).unwrap();
        storage.set_archive(&BTreeMap::from([(2, sample_item(2))])).unwrap();
        storage.set_phases(&["one".to_string()]).unwrap();
        storage.set(&BTreeMap::from([(1, sample_item(1)), (3, sample_item(3))])).unwrap();

        fs::remove_dir_all(&dir).unwrap();
        let copied = Storage::new(&copy).unwrap();
        assert_eq!(copied.get().unwrap().keys().copied().collect::<Vec<u32>>(), vec![1, 3], "the latest version");
        assert_eq!(copied.get_counters().unwrap().highest_id, 1);
        assert_eq!(copied.read_journal().unwrap().len(), 2, "an append in place reaches the copy");
        assert_eq!(copied.get_archive().unwrap()[&2].description, "item 2");
        assert_eq!(copied.get_phases().unwrap(), vec!["one".to_string()]);
        assert!(copy.join("project.json").is_file(), "the marker, which names the project");
        assert!(!copy.join("history").exists(), "the history stays with the board");
        fs::remove_dir_all(copy.parent().unwrap()).ok();
    }

    /// A board written before it had a copy comes out whole from the first
    /// write that copies it: the files that write leaves alone are copied too.
    #[test]
    fn the_first_copy_of_a_board_holds_every_file() {
        let dir = temp_ekko_dir();
        let copy = temp_ekko_dir();
        let before = Storage::new(&dir).unwrap();
        before.set_archive(&BTreeMap::from([(2, sample_item(2))])).unwrap();
        before.set_phases(&["one".to_string()]).unwrap();
        before.append_journal(&serde_json::json!({"rev": 1})).unwrap();
        before.set_counters(&Counters { revision: 1, highest_id: 2, ..Counters::default() }).unwrap();

        let storage = Storage::new(&dir).unwrap().copied_to(Some(copy.clone()));
        storage.set(&BTreeMap::from([(1, sample_item(1))])).unwrap();

        let copied = Storage::new(&copy).unwrap();
        assert_eq!(copied.get().unwrap().len(), 1);
        assert_eq!(copied.get_archive().unwrap().len(), 1);
        assert_eq!(copied.get_phases().unwrap(), vec!["one".to_string()]);
        assert_eq!(copied.read_journal().unwrap().len(), 1);
        assert_eq!(copied.get_counters().unwrap().highest_id, 2);
        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&copy).ok();
    }

    /// A copy that cannot be made never stops the write it would have kept.
    #[test]
    fn a_copy_that_cannot_be_made_does_not_fail_the_write() {
        let dir = temp_ekko_dir();
        let not_a_directory = dir.join("copy");
        fs::write(&not_a_directory, "").unwrap();
        let storage = Storage::new(&dir).unwrap().copied_to(Some(not_a_directory));
        storage.set(&BTreeMap::from([(1, sample_item(1))])).unwrap();
        assert_eq!(storage.get().unwrap().len(), 1);
        fs::remove_dir_all(&dir).ok();
    }

    /// The rule `get` and `get_shared` split between them (task 844): a read
    /// that changes nothing takes the version every read in the process
    /// shares, and `get` copies it only for a caller that changes the copy.
    /// So every `get` in the source binds a working copy, `let mut`; a read
    /// calling it would pay a copy for nothing, as the CLI's views and a
    /// write's `before` did -- on top of the parse each paid before.
    #[test]
    fn only_a_copy_to_change_is_taken_with_get() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut reads = Vec::new();
        let mut copies = 0;
        for entry in fs::read_dir(&src).unwrap() {
            let path = entry.unwrap().path();
            let text = fs::read_to_string(&path).unwrap();
            for (at, line) in text.lines().take_while(|line| *line != "mod tests {").enumerate() {
                if !line.contains("storage.get()") {
                    continue;
                }
                if line.trim_start().strip_prefix("let mut ").is_some_and(|rest| rest.ends_with("storage.get()?;")) {
                    copies += 1;
                } else {
                    reads.push(format!("{}:{}: {}", path.display(), at + 1, line.trim()));
                }
            }
        }
        assert!(reads.is_empty(), "a read that changes nothing takes get_shared: {reads:#?}");
        assert!(copies > 10, "the scan finds the working copies it should, not {copies}");
    }
}
