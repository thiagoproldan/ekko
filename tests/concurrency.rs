//! End-to-end concurrency regression test: spawns real, separate `ekko`
//! processes (the compiled binary, via `CARGO_BIN_EXE_ekko` -- not calls
//! into the library, an actual subprocess per invocation) racing to write
//! the same ekko directory at once.
//!
//! This is the test that should have existed from the start and didn't:
//! every lower-level piece (the lock's acquire/release logic, temp-file
//! cleanup) had its own unit tests and they all passed, but none of them
//! ever had two real processes contending for the *same* lock file using
//! the real `acquire_lock` code on both sides. That gap let a real bug
//! through: `create_new` (atomically creating the lock file) and writing
//! the pid into it are two separate syscalls, and a concurrent reader
//! catching the file in between -- empty, unparseable -- treated it as
//! abandoned and deleted it out from under the process that had just
//! created it. Manual testing with the real binary caught it (duplicate
//! "Created task: N" ids across processes); this test locks the fix in.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::process::{self, Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

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
