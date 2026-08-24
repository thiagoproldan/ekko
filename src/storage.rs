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
use std::io;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::item::Item;
use crate::json;

const LOCK_ACQUIRE_TIMEOUT: Duration = Duration::from_millis(5000);
const LOCK_RETRY_DELAY: Duration = Duration::from_millis(50);

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
    LockTimeout(PathBuf),
}

impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StorageError::Io(e) => write!(f, "{e}"),
            StorageError::Json(e) => write!(f, "{e}"),
            StorageError::LockTimeout(path) => write!(
                f,
                "Timed out waiting for the ekko storage lock. If no other ekko process is running, delete this file and try again: {}",
                path.display()
            ),
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

    pub fn get(&self) -> Result<ItemMap, StorageError> {
        read_map(&self.storage_file)
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
        let deadline = Instant::now() + LOCK_ACQUIRE_TIMEOUT;

        // Opened once, outside the loop: a `flock` belongs to the open file
        // description, not to the path or the process, so re-opening per
        // attempt would be both wasteful and easy to get subtly wrong.
        // `truncate(false)` because the file's *contents* are irrelevant now
        // -- it exists purely as something to lock.
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&self.lock_file)?;

        loop {
            // SAFETY: `file` owns this descriptor and outlives the call.
            let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            if result == 0 {
                return Ok(LockGuard { _storage: self, _file: file });
            }

            let error = io::Error::last_os_error();
            // EWOULDBLOCK is the only "someone else is holding it" answer.
            // Anything else is a genuine failure and shouldn't be retried
            // silently until the timeout.
            if error.raw_os_error() != Some(libc::EWOULDBLOCK) {
                return Err(error.into());
            }

            if Instant::now() >= deadline {
                return Err(StorageError::LockTimeout(self.lock_file.clone()));
            }

            std::thread::sleep(LOCK_RETRY_DELAY);
        }
    }
}

fn read_map(path: &Path) -> Result<ItemMap, StorageError> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }

    let content = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&content)?)
}

fn write_atomic(path: &Path, temp_dir: &Path, data: &ItemMap) -> Result<(), StorageError> {
    let content = json::to_pretty_string(data)?;
    let temp_file = temp_file_path(path, temp_dir);
    fs::write(&temp_file, content)?;
    fs::rename(&temp_file, path)?;
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

        assert!(matches!(second, Err(StorageError::LockTimeout(_))));

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

        assert!(matches!(result, Err(StorageError::LockTimeout(_))));
        holder.kill().ok();
        holder.wait().ok();
        fs::remove_dir_all(&dir).ok();
    }
}
