//! File storage: reading/writing `storage.json`/`archive.json`, and the
//! cross-process lock that protects both.
//!
//! Ported from the JS version's `storage.js` with one structural change,
//! made natural by Rust rather than bolted on: instead of a module-level
//! `Set` of held lock paths plus a manually-registered `process.on('exit')`
//! cleanup hook (needed there because `process.exit()` skips `finally`
//! blocks), `acquire_lock` returns a [`LockGuard`] whose `Drop` impl
//! releases the lock. As long as call sites propagate errors with `?`
//! instead of exiting mid-function (see the crate's error-handling
//! design), normal Rust unwinding guarantees the release -- no exit hook
//! needed at all. Nesting (e.g. one operation built out of two others that
//! each separately need the lock) is handled by having only the outermost,
//! public entry point acquire it, with private helpers that assume it's
//! already held -- not by making acquisition itself re-entrant.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize as _;

use crate::item::Item;

const LOCK_ACQUIRE_TIMEOUT: Duration = Duration::from_millis(5000);
const LOCK_RETRY_DELAY: Duration = Duration::from_millis(50);
const LOCK_STALE: Duration = Duration::from_millis(30_000);

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
                "Timed out waiting for the taskbook storage lock. If no other taskbook process is running, delete this file and try again: {}",
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
pub struct LockGuard<'a> {
    storage: &'a Storage,
}

impl Drop for LockGuard<'_> {
    fn drop(&mut self) {
        self.storage.release_lock_file();
    }
}

impl Storage {
    pub fn new(taskbook_dir: &Path) -> Result<Self, StorageError> {
        let storage_dir = taskbook_dir.join("storage");
        let archive_dir = taskbook_dir.join("archive");
        let temp_dir = taskbook_dir.join(".temp");

        fs::create_dir_all(&storage_dir)?;
        fs::create_dir_all(&archive_dir)?;
        fs::create_dir_all(&temp_dir)?;

        let storage = Storage {
            storage_file: storage_dir.join("storage.json"),
            archive_file: archive_dir.join("archive.json"),
            temp_dir,
            lock_file: taskbook_dir.join(".lock"),
        };

        storage.clean_temp_dir()?;
        Ok(storage)
    }

    fn clean_temp_dir(&self) -> Result<(), StorageError> {
        for entry in fs::read_dir(&self.temp_dir)? {
            fs::remove_file(entry?.path())?;
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
    /// is acquired, a dead holder is detected and cleared, or
    /// `LOCK_ACQUIRE_TIMEOUT` elapses against a genuinely live holder.
    pub fn acquire_lock(&self) -> Result<LockGuard<'_>, StorageError> {
        let deadline = Instant::now() + LOCK_ACQUIRE_TIMEOUT;

        loop {
            match OpenOptions::new()
                .write(true)
                .create_new(true) // atomic exclusive create: POSIX O_CREAT|O_EXCL
                .open(&self.lock_file)
            {
                Ok(mut file) => {
                    file.write_all(process::id().to_string().as_bytes())?;
                    return Ok(LockGuard { storage: self });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    if self.clear_lock_if_stale() {
                        continue;
                    }

                    if Instant::now() >= deadline {
                        return Err(StorageError::LockTimeout(self.lock_file.clone()));
                    }

                    std::thread::sleep(LOCK_RETRY_DELAY);
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    /// `true` if the lock was missing, unreadable, or stale (and has now
    /// been cleared) -- i.e. the caller should retry acquiring immediately.
    /// `false` means it's genuinely held by a live process within the
    /// staleness window; the caller should keep waiting.
    fn clear_lock_if_stale(&self) -> bool {
        let (Ok(metadata), Ok(content)) =
            (fs::metadata(&self.lock_file), fs::read_to_string(&self.lock_file))
        else {
            return true;
        };

        let pid: Option<u32> = content.trim().parse().ok();
        let held_by_live_process = pid.is_some_and(is_process_alive);
        let age = metadata
            .modified()
            .and_then(|m| m.elapsed().map_err(io::Error::other))
            .unwrap_or(Duration::MAX);

        if held_by_live_process && age < LOCK_STALE {
            return false;
        }

        let _ = fs::remove_file(&self.lock_file);
        true
    }

    /// Never removes a lock file this process doesn't currently own -- it
    /// may have been cleared as stale and re-acquired by someone else in
    /// between.
    fn release_lock_file(&self) {
        if let Ok(content) = fs::read_to_string(&self.lock_file) {
            if content.trim() == process::id().to_string() {
                let _ = fs::remove_file(&self.lock_file);
            }
        }
    }
}

fn is_process_alive(pid: u32) -> bool {
    // Signal 0: sends nothing, just checks whether the pid could be
    // signaled at all. `ESRCH` means "no such process"; anything else
    // (typically EPERM, it exists but we can't signal it) still counts as
    // alive.
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if result != 0 && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
        return false;
    }

    // kill() succeeding also covers zombies: a process that has already
    // exited and is just waiting for its parent to collect the exit
    // status. It holds no real resources and isn't doing any work, so
    // treat it the same as "not alive" for staleness purposes. Found by a
    // test that spawned a child, didn't reap it promptly, and watched lock
    // recovery wait the full timeout instead of clearing near-instantly.
    !is_zombie(pid)
}

/// Linux-only (via procfs); harmlessly returns `false` (i.e. "trust
/// `kill()`") anywhere else, including if the process has already gone
/// entirely.
fn is_zombie(pid: u32) -> bool {
    let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };

    // Format: "pid (comm) state ...". `comm` (the executable name) can
    // itself contain spaces or parens, so find the *last* ')' rather than
    // splitting on the first one.
    stat.rfind(')')
        .and_then(|i| stat[i + 1..].trim_start().chars().next())
        .is_some_and(|state| state == 'Z')
}

fn read_map(path: &Path) -> Result<ItemMap, StorageError> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }

    let content = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&content)?)
}

fn write_atomic(path: &Path, temp_dir: &Path, data: &ItemMap) -> Result<(), StorageError> {
    let json = to_pretty_json(data)?;
    let temp_file = temp_file_path(path, temp_dir);
    fs::write(&temp_file, json)?;
    fs::rename(&temp_file, path)?;
    Ok(())
}

/// 4-space indent, matching the JS version's `JSON.stringify(data, null, 4)`.
fn to_pretty_json(data: &ItemMap) -> Result<String, serde_json::Error> {
    let mut buffer = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b"    ");
    let mut serializer = serde_json::Serializer::with_formatter(&mut buffer, formatter);
    data.serialize(&mut serializer)?;
    Ok(String::from_utf8(buffer).expect("serde_json only ever writes valid UTF-8"))
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

    fn temp_taskbook_dir() -> PathBuf {
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
        let dir = temp_taskbook_dir();
        let storage = Storage::new(&dir).unwrap();

        assert_eq!(storage.get().unwrap(), BTreeMap::new());
        assert_eq!(storage.get_archive().unwrap(), BTreeMap::new());

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn set_then_get_round_trips() {
        let dir = temp_taskbook_dir();
        let storage = Storage::new(&dir).unwrap();

        let mut data = BTreeMap::new();
        data.insert(1, sample_item(1));
        data.insert(2, sample_item(2));
        storage.set(&data).unwrap();

        assert_eq!(storage.get().unwrap(), data);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn acquire_then_drop_creates_and_removes_the_lock_file() {
        let dir = temp_taskbook_dir();
        let storage = Storage::new(&dir).unwrap();
        let lock_file = dir.join(".lock");

        {
            let _guard = storage.acquire_lock().unwrap();
            assert_eq!(fs::read_to_string(&lock_file).unwrap(), process::id().to_string());
        }

        assert!(!lock_file.exists(), "lock file should be gone once the guard drops");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_lock_left_by_a_dead_process_is_cleared_near_instantly() {
        let dir = temp_taskbook_dir();
        let storage = Storage::new(&dir).unwrap();
        // A pid essentially guaranteed not to exist, standing in for a
        // process that crashed while holding the lock.
        fs::write(dir.join(".lock"), "999999999").unwrap();

        let start = Instant::now();
        let _guard = storage.acquire_lock().unwrap();
        let elapsed = start.elapsed();

        assert!(elapsed < Duration::from_millis(500), "expected near-instant recovery, took {elapsed:?}");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn release_never_deletes_a_lock_file_this_process_does_not_own() {
        let dir = temp_taskbook_dir();
        let storage = Storage::new(&dir).unwrap();
        let lock_file = dir.join(".lock");

        let guard = storage.acquire_lock().unwrap();
        // Simulate the file having been recreated by someone else in the
        // meantime.
        fs::write(&lock_file, "1").unwrap();
        drop(guard);

        assert!(lock_file.exists());

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn waits_out_a_real_external_process_then_succeeds() {
        let dir = temp_taskbook_dir();
        let storage = Storage::new(&dir).unwrap();

        // A real, separate OS process -- not a simulation -- standing in
        // for another `ekko` invocation holding the lock.
        let mut holder = Command::new("sleep").arg("1").spawn().unwrap();
        fs::write(dir.join(".lock"), holder.id().to_string()).unwrap();

        // Reap it as soon as it exits, same as a real ekko invocation's
        // parent shell would -- otherwise it lingers as a zombie, which
        // `is_process_alive` now specifically accounts for but this test
        // shouldn't rely on that to still prove out the common case.
        let reaper = std::thread::spawn(move || {
            holder.wait().ok();
        });

        let start = Instant::now();
        let _guard = storage.acquire_lock().unwrap();
        let elapsed = start.elapsed();

        assert!(elapsed >= Duration::from_millis(900), "should have waited for the real holder, only waited {elapsed:?}");
        reaper.join().ok();
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_lock_left_by_an_unreaped_zombie_is_also_cleared_near_instantly() {
        let dir = temp_taskbook_dir();
        let storage = Storage::new(&dir).unwrap();

        // Deliberately do *not* reap this one -- it becomes a zombie the
        // moment it exits, which plain `kill(pid, 0)` alone would still
        // report as "alive".
        let mut holder = Command::new("sleep").arg("0.2").spawn().unwrap();
        fs::write(dir.join(".lock"), holder.id().to_string()).unwrap();
        std::thread::sleep(Duration::from_millis(500)); // let it exit and zombify

        let start = Instant::now();
        let _guard = storage.acquire_lock().unwrap();
        let elapsed = start.elapsed();

        assert!(elapsed < Duration::from_millis(500), "expected near-instant recovery from a zombie holder, took {elapsed:?}");
        holder.wait().ok();
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn times_out_against_a_holder_that_outlives_the_timeout() {
        let dir = temp_taskbook_dir();
        let storage = Storage::new(&dir).unwrap();

        let mut holder = Command::new("sleep").arg("8").spawn().unwrap();
        fs::write(dir.join(".lock"), holder.id().to_string()).unwrap();

        let result = storage.acquire_lock();

        assert!(matches!(result, Err(StorageError::LockTimeout(_))));
        holder.kill().ok();
        holder.wait().ok();
        fs::remove_dir_all(&dir).ok();
    }
}
