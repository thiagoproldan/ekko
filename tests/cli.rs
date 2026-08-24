//! CLI-level behaviours that only show up when the real binary meets a real
//! shell: signals, pipes, exit codes. None of it is reachable from the unit
//! tests, which drive the library side and never touch a file descriptor
//! they did not create.

use std::fs;
use std::io::Read as _;
use std::path::PathBuf;
use std::process::{self, Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_ekko_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ekko-e2e-cli-{}-{}",
        process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
    ));
    fs::create_dir_all(dir.join(".ekko").join("storage")).unwrap();
    dir
}

/// Writes a board big enough that its `--json` output cannot fit in a pipe
/// buffer, so the child is still writing when the reader goes away. Built
/// directly rather than by spawning the binary a few hundred times.
fn seed_large_board(dir: &std::path::Path, count: u32) {
    let items: Vec<String> = (1..=count)
        .map(|id| {
            format!(
                r#""{id}":{{"_id":{id},"_date":"Mon Aug 24 2026","_timestamp":1787600000000,"description":"item number {id}, padded out so the whole board comfortably exceeds a pipe buffer","isStarred":false,"boards":["@bench"],"_isTask":true,"isComplete":false,"inProgress":false,"priority":1}}"#
            )
        })
        .collect();
    fs::write(
        dir.join(".ekko").join("storage").join("storage.json"),
        format!("{{{}}}", items.join(",")),
    )
    .unwrap();
}

/// Rust disables SIGPIPE at startup, which turns the most ordinary shell
/// idiom there is -- `ekko --json | head -1` -- into a panic. Restoring the
/// default disposition is what every other Unix tool does.
#[test]
fn a_reader_that_goes_away_does_not_produce_a_panic() {
    let dir = temp_ekko_dir();
    seed_large_board(&dir, 600);

    let mut child = Command::new(env!("CARGO_BIN_EXE_ekko"))
        .args(["--ekko-dir", dir.to_str().unwrap(), "--json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn ekko");

    // Read a token amount, then drop the pipe while the child is still
    // writing -- exactly what `head -1` does.
    let mut stdout = child.stdout.take().expect("stdout was piped");
    let mut head = [0u8; 32];
    stdout.read_exact(&mut head).expect("expected some output before the pipe closed");
    drop(stdout);

    let output = child.wait_with_output().expect("failed to wait on ekko");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!stderr.contains("panicked"), "broken pipe produced a panic:\n{stderr}");
    assert!(
        !stderr.contains("Broken pipe"),
        "broken pipe was reported to the user:\n{stderr}"
    );

    fs::remove_dir_all(&dir).ok();
}

/// The same run, uninterrupted, still has to behave normally -- it would be
/// easy to "fix" the pipe case by breaking the ordinary one.
#[test]
fn output_that_is_read_to_the_end_still_succeeds() {
    let dir = temp_ekko_dir();
    seed_large_board(&dir, 20);

    let output = Command::new(env!("CARGO_BIN_EXE_ekko"))
        .args(["--ekko-dir", dir.to_str().unwrap(), "--json"])
        .output()
        .expect("failed to run ekko");

    assert!(output.status.success(), "exit status was {:?}", output.status);
    let stdout = String::from_utf8(output.stdout).expect("--json output should be UTF-8");
    assert_eq!(stdout.lines().count(), 2, "board line plus stats line");

    fs::remove_dir_all(&dir).ok();
}
