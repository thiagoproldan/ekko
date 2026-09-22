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
/// an in-flight one. Nothing to do with the lock -- `write_atomic` creates
/// its temp file and renames it within microseconds, so anything still
/// sitting there after a full second belongs to a process that died
/// between the two steps.
const TEMP_FILE_ABANDONED: Duration = Duration::from_millis(1000);

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
    storage_file: PathBuf,
    archive_file: PathBuf,
    temp_dir: PathBuf,
    lock_file: PathBuf,
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

impl Storage {
    pub fn new(ekko_dir: &Path) -> Result<Self, StorageError> {
        let storage_dir = ekko_dir.join("storage");
        let archive_dir = ekko_dir.join("archive");
        let temp_dir = ekko_dir.join(".temp");

        fs::create_dir_all(&storage_dir)?;
        fs::create_dir_all(&archive_dir)?;
        fs::create_dir_all(&temp_dir)?;

        let storage = Storage {
            storage_file: storage_dir.join("storage.json"),
            archive_file: archive_dir.join("archive.json"),
            temp_dir,
            lock_file: ekko_dir.join(".lock"),
        };

        storage.clean_temp_dir()?;
        Ok(storage)
    }

    /// Sweeps up temp files abandoned by a crashed write (create-then-
    /// rename is atomic, but only once the write in between has actually
    /// finished). This runs unconditionally at startup, for every
    /// process, *not* under the lock -- constructing `Storage` at all
    /// needs to stay lock-free for read-only commands. That means it can
    /// run concurrently with another live process's in-flight
    /// `write_atomic`, so it only removes temp files old enough that they
    /// cannot plausibly still be someone's in-progress write (that write
    /// is one `fs::write` call to a fresh, uniquely-named file -- there
    /// and gone in well under this margin under any real load); a fresh
    /// temp file is left alone rather than risk deleting live work.
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

    /// The board, to change. A version this process already parsed is cloned
    /// rather than parsed again; one it has not is parsed and not kept, so a
    /// one-shot command pays nothing for a cache it would never read twice.
    pub fn get(&self) -> Result<ItemMap, StorageError> {
        let Some((file, version)) = open_versioned(&self.storage_file)? else {
            return Ok(BTreeMap::new());
        };
        match kept_board(&self.storage_file, version) {
            Some(board) => Ok(ItemMap::clone(&board)),
            None => parse_map(file),
        }
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
        write_atomic(&self.storage_file, &self.temp_dir, data)
    }

    pub fn set_archive(&self, data: &ItemMap) -> Result<(), StorageError> {
        write_atomic(&self.archive_file, &self.temp_dir, data)
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
        replace_durably(&self.phases_file(), &self.temp_dir, serde_json::to_string_pretty(phases)?.as_bytes())
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
        replace_durably(&self.counters_file(), &self.temp_dir, serde_json::to_string_pretty(counters)?.as_bytes())
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
        self.append_journal_within(entry, JOURNAL_BYTES)
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
        replace_durably(&path, &self.temp_dir, content.as_bytes())
    }
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

/// The file open, with the version the descriptor holds -- read from the same
/// descriptor the content will be, so the two cannot disagree -- or `None`
/// when the board has no file yet.
fn open_versioned(path: &Path) -> Result<Option<(File, Version)>, StorageError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let meta = file.metadata()?;
    let version =
        Version { dev: meta.dev(), ino: meta.ino(), mtime: meta.mtime(), mtime_nsec: meta.mtime_nsec(), len: meta.len() };
    Ok(Some((file, version)))
}

fn parse_map(mut file: File) -> Result<ItemMap, StorageError> {
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

fn write_atomic(path: &Path, temp_dir: &Path, data: &ItemMap) -> Result<(), StorageError> {
    replace_durably(path, temp_dir, json::to_pretty_string(data)?.as_bytes())
}

/// Replaces `path` with `content` so that a reader sees the old file or the
/// new one, never half of one, and so the new one survives a crash: the temp
/// file is synced before the rename and its directory after. Without the
/// first sync, a crash just after the rename can leave an empty file on a
/// filesystem that allocates lazily, which for storage.json is the whole
/// board.
fn replace_durably(path: &Path, temp_dir: &Path, content: &[u8]) -> Result<(), StorageError> {
    use std::io::Write as _;
    let temp_file = temp_file_path(path, temp_dir);
    let mut file = File::create(&temp_file)?;
    file.write_all(content)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temp_file, path)?;
    // Best effort: some filesystems refuse to sync a directory, and the
    // rename itself has already happened.
    if let Some(dir) = path.parent().and_then(|parent| File::open(parent).ok()) {
        let _ = dir.sync_all();
    }
    Ok(())
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
        let dir = std::env::temp_dir().join(format!(
            "ekko-test-{}-{}",
            process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
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
}
