//! End-to-end concurrency regression tests: they spawn real, separate
//! `ekko` processes (the compiled binary, via `CARGO_BIN_EXE_ekko` -- not
//! calls into the library, an actual subprocess per invocation) racing on
//! the same ekko directory at once.
//!
//! This file exists because of what it caught. Every lower-level piece of
//! the lock had its own unit tests and they all passed -- acquire/release,
//! stale detection, pid liveness, zombie holders, temp-file cleanup, seven
//! tests' worth -- while the lock was still losing updates under real
//! contention. None of those tests ever had two real processes contending
//! for the same lock through the real `acquire_lock` on both sides, and
//! that was exactly the gap the bug lived in: a waiter deleting a *live*
//! holder's lock file, letting a second writer into the critical section,
//! where both derived the same next id and one silently overwrote the
//! other. See `storage::acquire_lock` for the mechanism and the fix.
//!
//! The lesson generalises past that one bug, so these tests deliberately
//! cover the properties rather than the implementation: no lost updates,
//! no torn reads, and no wedging when a holder dies outright. Any future
//! rework of the locking has to keep passing these even if every unit test
//! around it is rewritten.

use std::collections::HashSet;
use std::os::fd::AsRawFd;
use std::fs;
use std::path::PathBuf;
use std::process::{self, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn temp_ekko_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ekko-e2e-concurrency-{}-{}",
        process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn run_task_create(exe: &str, dir: &std::path::Path, description: &str) -> process::Child {
    Command::new(exe)
        .args(["--ekko-dir", dir.to_str().unwrap(), "--task", description])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn ekko")
}

#[test]
fn concurrent_writers_neither_collide_on_an_id_nor_lose_an_update() {
    let dir = temp_ekko_dir();
    let exe = env!("CARGO_BIN_EXE_ekko");
    let total = 25;

    let children: Vec<process::Child> =
        (0..total).map(|i| run_task_create(exe, &dir, &format!("concurrent task {i}"))).collect();

    for mut child in children {
        let status = child.wait().expect("failed to wait on child");
        assert!(status.success(), "an `ekko --task` invocation exited non-zero: {status:?}");
    }

    let storage_path = dir.join(".ekko").join("storage").join("storage.json");
    let content = fs::read_to_string(&storage_path).expect("storage.json should exist");
    let data: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&content).expect("storage.json should be valid JSON");

    assert_eq!(data.len(), total, "expected {total} items, found {} (lost update)", data.len());

    let ids: HashSet<i64> = data.keys().map(|k| k.parse().unwrap()).collect();
    assert_eq!(ids.len(), total, "expected every id to be unique");

    let descriptions: HashSet<&str> =
        data.values().map(|v| v["description"].as_str().unwrap()).collect();
    assert_eq!(descriptions.len(), total, "expected every description to be unique");

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn fifty_concurrent_writers_still_neither_collide_nor_lose_updates() {
    let dir = temp_ekko_dir();
    let exe = env!("CARGO_BIN_EXE_ekko");
    let total = 50;

    let children: Vec<process::Child> =
        (0..total).map(|i| run_task_create(exe, &dir, &format!("task {i}"))).collect();

    for mut child in children {
        assert!(child.wait().expect("failed to wait on child").success());
    }

    let storage_path = dir.join(".ekko").join("storage").join("storage.json");
    let content = fs::read_to_string(&storage_path).unwrap();
    let data: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&content).unwrap();

    assert_eq!(data.len(), total, "expected {total} items, found {} (lost update)", data.len());

    fs::remove_dir_all(&dir).ok();
}

fn spawn(exe: &str, dir: &std::path::Path, args: &[&str]) -> process::Child {
    Command::new(exe)
        .args(["--ekko-dir", dir.to_str().unwrap()])
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn ekko")
}

fn read_storage(dir: &std::path::Path) -> serde_json::Map<String, serde_json::Value> {
    let path = dir.join(".ekko").join("storage").join("storage.json");
    let content = fs::read_to_string(&path).expect("storage.json should exist");
    serde_json::from_str(&content).expect("storage.json should be valid JSON")
}

fn seed_tasks(exe: &str, dir: &std::path::Path, count: u32) {
    let children: Vec<_> =
        (0..count).map(|i| run_task_create(exe, dir, &format!("seed {i}"))).collect();
    for mut child in children {
        assert!(child.wait().expect("failed to wait on child").success());
    }
}

/// Creates alone only ever *append*. These race the read-modify-write
/// commands -- which load every item, mutate one, and write the whole map
/// back -- against each other, so a dropped update destroys a neighbouring
/// item's state rather than just costing an id.
#[test]
fn concurrent_mixed_mutations_neither_lose_writes_nor_corrupt_state() {
    let dir = temp_ekko_dir();
    let exe = env!("CARGO_BIN_EXE_ekko");
    let seeded = 20;

    seed_tasks(exe, &dir, seeded);

    // `--check` and `--star` both toggle, so exactly one of each per id
    // leaves a state that is deterministic no matter what order they land
    // in -- but only if every single one of them survives.
    let mut children = Vec::new();
    for id in 1..=seeded {
        children.push(spawn(exe, &dir, &["--check", &id.to_string()]));
        children.push(spawn(exe, &dir, &["--star", &id.to_string()]));
        children.push(run_task_create(exe, &dir, &format!("extra {id}")));
    }
    for mut child in children {
        assert!(child.wait().expect("failed to wait on child").success());
    }

    let data = read_storage(&dir);
    assert_eq!(data.len(), (seeded * 2) as usize, "expected every create to survive");

    for id in 1..=seeded {
        let item = &data[&id.to_string()];
        assert_eq!(item["isComplete"], true, "item {id} lost its --check");
        assert_eq!(item["isStarred"], true, "item {id} lost its --star");
    }

    fs::remove_dir_all(&dir).ok();
}

/// Readers deliberately don't take the lock, so this is really a test of
/// `write_atomic`: writes land by rename, and a reader must therefore see
/// either the whole previous file or the whole next one -- never a prefix.
#[test]
fn readers_never_observe_a_torn_storage_file() {
    let dir = temp_ekko_dir();
    let exe = env!("CARGO_BIN_EXE_ekko");

    seed_tasks(exe, &dir, 5);

    let mut writers = Vec::new();
    let mut readers = Vec::new();
    for i in 0..25 {
        writers.push(run_task_create(exe, &dir, &format!("writer {i}")));
        readers.push(
            Command::new(exe)
                .args(["--ekko-dir", dir.to_str().unwrap(), "--json"])
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .expect("failed to spawn ekko"),
        );
    }

    for mut writer in writers {
        assert!(writer.wait().expect("failed to wait on child").success());
    }

    for reader in readers {
        let output = reader.wait_with_output().expect("failed to wait on reader");
        assert!(output.status.success(), "a concurrent reader failed outright");

        let stdout = String::from_utf8(output.stdout).expect("--json output should be UTF-8");
        for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
            let parsed: serde_json::Value =
                serde_json::from_str(line).unwrap_or_else(|e| panic!("torn read: {e}\n{line}"));
            assert_eq!(parsed["ok"], true, "reader reported an error: {line}");
        }
    }

    fs::remove_dir_all(&dir).ok();
}

/// The failure the pid-file scheme was built to handle, done for real: a
/// holder that dies without ever getting to clean up. Nothing in ekko
/// notices -- the kernel drops the `flock` with the process's descriptors
/// -- which is the whole reason the fix removed the detection logic rather
/// than tightening it.
#[test]
fn a_lock_holder_killed_outright_does_not_wedge_later_writers() {
    let dir = temp_ekko_dir();
    let exe = env!("CARGO_BIN_EXE_ekko");

    seed_tasks(exe, &dir, 1); // creates the directory layout and the lock file
    let lock_file = dir.join(".ekko").join(".lock");
    assert!(lock_file.exists(), "expected the lock file at {}", lock_file.display());

    // `-o` closes the locked descriptor before running the command, so the
    // `sleep` never inherits it. Without that the lock survives killing
    // flock itself -- a `flock` is held by the open file description, which
    // fork duplicates -- and this test would be asserting the opposite of
    // what it means to.
    let mut holder = Command::new("flock")
        .args(["-x", "-o", lock_file.to_str().unwrap(), "sleep", "10"])
        .spawn()
        .expect("flock(1) from util-linux is required by this test");

    // Wait until the lock is observably taken, so killing it proves
    // something rather than racing the child's startup.
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if !can_lock(&lock_file) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!can_lock(&lock_file), "the flock(1) holder never took the lock");

    holder.kill().expect("failed to kill the holder");
    holder.wait().ok();

    let start = Instant::now();
    let mut child = run_task_create(exe, &dir, "after the holder was killed");
    assert!(child.wait().expect("failed to wait on child").success());
    let elapsed = start.elapsed();

    assert!(elapsed < Duration::from_secs(2), "a killed holder wedged the lock for {elapsed:?}");
    assert_eq!(read_storage(&dir).len(), 2);

    fs::remove_dir_all(&dir).ok();
}

/// `true` if the lock is free right now. Takes and immediately drops the
/// lock, so it must never be called while this process wants to hold it.
fn can_lock(lock_file: &std::path::Path) -> bool {
    let file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_file)
        .expect("failed to open the lock file");
    // SAFETY: `file` owns this descriptor and outlives the call.
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) == 0 }
}
