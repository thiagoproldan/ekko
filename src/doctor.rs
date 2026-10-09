//! `ekko --doctor`: checks, from the side that consumes them, that ekko's
//! links with Claude Code work (plan 1279, task 1282). They break without an
//! error anywhere: a session's MCP server keeps the binary it started with
//! after an upgrade (gotcha 194), a session whose plugin hooks did not run
//! has no prime, no wake and no guard, and one whose FileChanged hook stopped
//! hearing the board's file is never woken (task 1248, 1283). The checks
//! read /proc and ekko's
//! state directory and write nothing. Each gives ok, fail, or skipped when
//! it cannot judge, in one line, about one session; a fail makes the exit 1.
//! No check gives a warning yet: the plan's warn comes with the first that
//! has one to give.

use std::ffi::{OsStr, OsString};
use std::fmt::Write as _;
use std::fs;
use std::os::unix::ffi::OsStringExt as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde::Serialize;

use crate::holder::{when, Process, Registry, Running};
use crate::mcp::Binary;
use crate::wake::{Guarded, Heard, Told};

/// Where NixOS keeps the binary a session started with after an upgrade:
/// a store path never changes, so the plugin, which starts ekko by its store
/// path, runs the old one while the switch moves the name `ekko` on PATH
/// (task 469).
const STORE: &str = "/nix/store/";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Ok,
    Fail,
    Skipped,
}

impl Verdict {
    fn word(self) -> &'static str {
        match self {
            Verdict::Ok => "ok",
            Verdict::Fail => "fail",
            Verdict::Skipped => "skipped",
        }
    }
}

/// One check's answer about one session.
#[derive(Debug, Serialize)]
pub struct Check {
    pub check: &'static str,
    pub verdict: Verdict,
    /// What it is about, as a person reads it: a Claude Code session, named
    /// as a claim names its holder, or `ekko serve`.
    pub about: String,
    /// The same Claude Code session, for machines.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<Session>,
    pub says: String,
}

/// A Claude Code process, as a check names it to machines.
#[derive(Debug, Clone, Serialize)]
pub struct Session {
    pub pid: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tty: Option<String>,
    /// The folder it runs in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder: Option<PathBuf>,
    /// The conversation it runs, as its SessionStart hook recorded it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub checks: Vec<Check>,
}

const OLD_BINARY: &str = "Old binary";
const HOOKS_LOADED: &str = "Hooks loaded";
const WAKE_HEARD: &str = "Wake heard";
const GUARD_RAN: &str = "Guard ran";

/// How much of a transcript's end the guard's check reads for the session's
/// last Bash call.
const TAIL: u64 = 1 << 20;

/// How old the board's last write must be before a session's wake hook is
/// judged on it, in milliseconds. The hook ran about 0.6 s after a write in
/// the measurements (notes 1264, 1265); a loaded machine may take longer,
/// and a margin too short raises false alarms (plan 1279's first risk).
const MARGIN: i64 = 5_000;

/// What the checks read beside /proc: the SessionStart hook's records, the
/// wake hook's, this boot's id, where the store is, and the time now.
struct Records<'a> {
    registry: &'a Registry,
    told: &'a Path,
    boot: &'a str,
    store: &'a Path,
    now: i64,
}

/// A process as /proc shows it, as far as the checks need.
#[derive(Debug, Clone, Default)]
struct Proc {
    pid: u32,
    parent: u32,
    start: u64,
    comm: String,
    args: Vec<String>,
    tty: Option<String>,
    /// The binary it runs, by device and inode, read through
    /// /proc/<pid>/exe, which leads to it even once it is replaced.
    running: Option<(u64, u64)>,
    exe: Option<PathBuf>,
    cwd: Option<PathBuf>,
    path_var: Option<OsString>,
}

impl Proc {
    fn read(pid: u32) -> Option<Proc> {
        let (parent, start) = crate::holder::stat(pid)?;
        let comm = fs::read_to_string(format!("/proc/{pid}/comm")).ok()?.trim().to_string();
        let args = fs::read(format!("/proc/{pid}/cmdline"))
            .ok()?
            .split(|&b| b == 0)
            .filter(|arg| !arg.is_empty())
            .map(|arg| String::from_utf8_lossy(arg).into_owned())
            .collect();
        let exe = format!("/proc/{pid}/exe");
        let path_var = fs::read(format!("/proc/{pid}/environ"))
            .ok()
            .and_then(|environ| environ.split(|&b| b == 0).find_map(|var| var.strip_prefix(b"PATH=").map(|path| OsString::from_vec(path.to_vec()))));
        Some(Proc {
            pid,
            parent,
            start,
            comm,
            args,
            tty: crate::holder::tty(pid),
            running: fs::metadata(&exe).ok().map(|file| (file.dev(), file.ino())),
            exe: fs::read_link(&exe).ok(),
            cwd: fs::read_link(format!("/proc/{pid}/cwd")).ok(),
            path_var,
        })
    }

    /// Every process /proc lets this user read.
    fn all() -> Vec<Proc> {
        let Ok(entries) = fs::read_dir("/proc") else { return Vec::new() };
        entries.flatten().filter_map(|entry| entry.file_name().to_str()?.parse().ok()).filter_map(Proc::read).collect()
    }

    /// What it serves, for an ekko the checks are about: a session's MCP
    /// server, the resources' beside it, or `ekko serve`.
    fn serves(&self) -> Option<&'static str> {
        if Path::new(self.args.first()?).file_name()? != "ekko" {
            return None;
        }
        if self.args.iter().any(|arg| arg == "--mcp") {
            return Some(if self.args.iter().any(|arg| arg == "--resources") { "ekko --mcp --resources" } else { "ekko --mcp" });
        }
        self.args.iter().skip(1).find(|arg| *arg != "--json" && *arg != "-j").filter(|arg| *arg == "serve").map(|_| "ekko serve")
    }

    /// Claude Code, as the holder tells it: by its process name.
    fn is_claude(&self) -> bool {
        self.comm.trim_start_matches('.').starts_with("claude")
    }
}

/// A binary's name as the report gives it: its folder in `store` without
/// the hash, as `ekko-0.39.1`, or else its path. /proc names a binary
/// deleted since it started with ` (deleted)` after its path.
fn named(path: &Path, store: &Path) -> String {
    let shown = path.to_string_lossy();
    let shown = Path::new(shown.strip_suffix(" (deleted)").unwrap_or(&shown));
    match shown.strip_prefix(store).ok().and_then(|rest| rest.iter().next()) {
        Some(folder) => {
            let folder = folder.to_string_lossy();
            folder.split_once('-').map_or(&*folder, |(_, name)| name).to_string()
        }
        None => shown.display().to_string(),
    }
}

/// Whether `proc`, an ekko that serves, runs the binary its names lead to
/// now, as the server's own `REPLACED` decides: its program name, looked up
/// on its own PATH when bare; and, for one started by its store path, the
/// bare name `ekko` on its PATH too. A dev build started by its path is
/// judged by that path alone: the ekko on PATH was never its.
fn old_binary(proc: &Proc, store: &Path) -> (Verdict, String) {
    let Some(running) = proc.running else {
        return (Verdict::Skipped, format!("/proc/{}/exe cannot be read", proc.pid));
    };
    let runs = proc.exe.as_deref().map_or_else(|| "a binary".to_string(), |exe| named(exe, store));
    let program = Path::new(proc.args.first().map(String::as_str).unwrap_or_default());
    // A relative path runs from the process's folder, not the doctor's.
    let lookup: OsString = match &proc.cwd {
        Some(cwd) if program.components().count() > 1 && program.is_relative() => cwd.join(program).into(),
        _ => program.as_os_str().to_os_string(),
    };
    let Some(now) = Binary::resolve(&lookup, proc.path_var.as_deref()) else {
        return (Verdict::Skipped, format!("it runs {runs}, and {} leads to no binary now", program.display()));
    };
    if (now.dev, now.ino) != running {
        return (Verdict::Fail, format!("it runs {runs}, and {} leads to {} now", program.display(), named(&now.path, store)));
    }
    if program.starts_with(store) {
        if let Some(on_path) = Binary::resolve(OsStr::new("ekko"), proc.path_var.as_deref()) {
            if (on_path.dev, on_path.ino) != running {
                return (Verdict::Fail, format!("it runs {runs}, and ekko on its PATH leads to {} now", named(&on_path.path, store)));
            }
        }
    }
    (Verdict::Ok, format!("it runs {runs}, where its names lead now"))
}

/// The Claude Code process `claude` as the hooks' records name it.
fn process(claude: &Proc, records: &Records) -> Process {
    Process { pid: claude.pid, start: claude.start, boot: records.boot.to_string() }
}

/// The board's file as the wake hook records a version of it: its inode,
/// its modification time in nanoseconds and its size.
fn version(board: &Path) -> Option<(u64, i64, u64)> {
    let file = fs::metadata(board).ok()?;
    Some((file.ino(), file.mtime().saturating_mul(1_000_000_000).saturating_add(file.mtime_nsec()), file.len()))
}

/// The version of a store build of ekko whose hooks record nothing of their
/// runs, `heard` and `guarded` -- 0.39.1 or before, by its folder's name --
/// or `None`. A build named by no version is taken to record them.
fn before_records(exe: &Path, store: &Path) -> Option<String> {
    let name = named(exe, store);
    let numbers: Vec<u64> = name.strip_prefix("ekko-")?.split('.').map(|number| number.parse().ok()).collect::<Option<_>>()?;
    (numbers.as_slice() <= [0, 39, 1].as_slice()).then_some(name)
}

/// A time to the second, as the margin needs: the hour today, the day and
/// hour before that.
fn clock(millis: i64) -> String {
    use chrono::TimeZone as _;
    let Some(at) = chrono::Local.timestamp_millis_opt(millis).single() else { return millis.to_string() };
    if at.date_naive() == chrono::Local::now().date_naive() { at.format("%H:%M:%S").to_string() } else { at.format("%Y-%m-%d %H:%M:%S").to_string() }
}

/// Whether a session's wake hook heard the last write to its board: given
/// the board's version now, when its SessionStart hook last ran, what its
/// wake hook last heard, and the version of its ekko where that records
/// nothing. A write before the SessionStart, or newer than `MARGIN`, is not
/// judged.
fn wake_heard(board: Option<(u64, i64, u64)>, started: Option<i64>, heard: Option<&Heard>, before: Option<&str>, now: i64) -> (Verdict, String) {
    let Some((inode, mtime_ns, size)) = board else {
        return (Verdict::Skipped, "its board's file cannot be read".to_string());
    };
    let Some(started) = started else {
        return (Verdict::Skipped, "when its SessionStart hook ran cannot be read".to_string());
    };
    let written = mtime_ns.div_euclid(1_000_000);
    if now - written < MARGIN {
        return (Verdict::Skipped, format!("its board's last write, {} ms ago, is too new to judge: its hook may still be on its way", (now - written).max(0)));
    }
    if written <= started {
        return (Verdict::Skipped, format!("nothing was written to its board since its SessionStart at {}: nothing to hear yet", clock(started)));
    }
    let missed = format!("its board was written at {}, after its SessionStart at {}", clock(written), clock(started));
    match (heard, before) {
        (Some(heard), _) if (heard.inode, heard.mtime_ns, heard.size) == (inode, mtime_ns, size) || heard.at >= written => {
            (Verdict::Ok, format!("its wake hook heard the last write to its board, at {}", clock(written)))
        }
        (Some(heard), _) => (
            Verdict::Fail,
            format!(
                "{missed}, and its wake hook last heard the version of {}: its FileChanged hook missed the write, as in task 1248; restart the session to watch the file again",
                clock(heard.mtime_ns.div_euclid(1_000_000))
            ),
        ),
        (None, Some(version)) => (Verdict::Skipped, format!("its {version} records nothing of what its wake hook heard, which began after 0.39.1")),
        (None, None) => (
            Verdict::Fail,
            format!("{missed}, and its wake hook recorded hearing nothing: its FileChanged hook did not run; restart the session to watch the file again"),
        ),
    }
}

/// The last Bash call in the end of `transcript`, the last `tail` bytes:
/// its tool_use id and when Claude Code wrote it, in milliseconds. A
/// subagent's calls, which its own transcripts hold, are left out. An end
/// with no row read as Claude Code writes them -- its format is not
/// documented, and can change -- is an error that says so.
fn last_bash(transcript: &Path, tail: u64) -> Result<Option<(String, i64)>, String> {
    use std::io::{Read as _, Seek as _, SeekFrom};
    let unread = |error: std::io::Error| format!("its transcript cannot be read: {error}");
    let mut file = fs::File::open(transcript).map_err(unread)?;
    let length = file.metadata().map_err(unread)?.len();
    file.seek(SeekFrom::Start(length.saturating_sub(tail))).map_err(unread)?;
    let mut end = Vec::new();
    file.read_to_end(&mut end).map_err(unread)?;
    // A read from inside the file starts within a row.
    let whole = if length > tail { end.iter().position(|&b| b == b'\n').map_or(&end[..0], |at| &end[at + 1..]) } else { &end[..] };
    let mut rows = 0;
    for line in whole.split(|&b| b == b'\n').rev().filter(|line| !line.is_empty()) {
        let Ok(row) = serde_json::from_slice::<serde_json::Value>(line) else { continue };
        let Some(kind) = row["type"].as_str() else { continue };
        rows += 1;
        if kind != "assistant" || row["isSidechain"] == true {
            continue;
        }
        let blocks = row["message"]["content"].as_array().into_iter().flatten();
        let Some(call) = blocks.rev().find(|block| block["type"] == "tool_use" && block["name"] == "Bash") else { continue };
        let at = row["timestamp"].as_str().and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok()).map(|at| at.timestamp_millis());
        let (Some(id), Some(at)) = (call["id"].as_str(), at) else { continue };
        return Ok(Some((id.to_string(), at)));
    }
    if rows == 0 && !whole.is_empty() {
        return Err("its transcript's end holds no row as Claude Code writes them".to_string());
    }
    Ok(None)
}

/// Whether the guard ran for a session's last Bash call: given that call,
/// when its SessionStart hook last ran, the guard's last run there, and the
/// version of its ekko where that records nothing. A call before the
/// SessionStart, or newer than `MARGIN`, is not judged.
fn guard_ran(last: Result<Option<(String, i64)>, String>, started: Option<i64>, guarded: Option<&Guarded>, before: Option<&str>, now: i64) -> (Verdict, String) {
    let (id, at) = match last {
        Err(why) => return (Verdict::Skipped, why),
        Ok(None) => return (Verdict::Skipped, "no Bash call in its transcript's end: nothing to judge".to_string()),
        Ok(Some(call)) => call,
    };
    let Some(started) = started else {
        return (Verdict::Skipped, "when its SessionStart hook ran cannot be read".to_string());
    };
    if at <= started {
        return (Verdict::Skipped, format!("no Bash call since its SessionStart at {}: nothing to judge", clock(started)));
    }
    if now - at < MARGIN {
        return (Verdict::Skipped, format!("its last Bash call, {} ms ago, is too new to judge", (now - at).max(0)));
    }
    match (guarded, before) {
        (Some(guarded), _) if guarded.tool_use_id == id || guarded.at + MARGIN >= at => (Verdict::Ok, format!("its guard ran for its last Bash call, at {}", clock(at))),
        (Some(guarded), _) => (
            Verdict::Fail,
            format!(
                "its last Bash call, at {}, came after the guard's last run there, at {}: its PreToolUse hook did not run; restart the session",
                clock(at),
                clock(guarded.at)
            ),
        ),
        (None, Some(version)) => (Verdict::Skipped, format!("its {version} records nothing of its guard's runs, which began after 0.39.1")),
        (None, None) => (
            Verdict::Fail,
            format!("its last Bash call, at {}, ran with no record of the guard: its PreToolUse hook did not run there; restart the session, and if this stays, check that the plugin is enabled", clock(at)),
        ),
    }
}

/// The Claude Code process `claude` as a check names it: as a claim names
/// its holder where its SessionStart hook recorded it, for a person; and for
/// machines.
fn about(claude: &Proc, record: Option<&Running>) -> (String, Option<Session>) {
    let name = match record {
        Some(running) => running.label(),
        None => format!("Claude Code pid {}{}", claude.pid, claude.tty.as_deref().map(|tty| format!(" on {tty}")).unwrap_or_default()),
    };
    let conversation = record.map(|running| running.conversation.clone());
    (name, Some(Session { pid: claude.pid, tty: claude.tty.clone(), folder: claude.cwd.clone(), conversation }))
}

/// The Claude Code process `proc` runs under, if its parent is one.
fn claude_of<'a>(proc: &Proc, all: &'a [Proc]) -> Option<&'a Proc> {
    all.iter().find(|parent| parent.pid == proc.parent && parent.is_claude())
}

/// The checks over `all`, the processes running, with `registry` the
/// SessionStart hook's records and `boot` this boot's id.
fn checks(all: &[Proc], records: &Records) -> Vec<Check> {
    let mut found = Vec::new();
    let served: Vec<&Proc> = all.iter().filter(|proc| proc.serves().is_some()).collect();
    for proc in &served {
        let (verdict, says) = old_binary(proc, records.store);
        let what = proc.serves().unwrap_or("ekko");
        let mend = match (verdict, what) {
            (Verdict::Fail, "ekko serve") => ": ekko serve --stop stops it, and the next ekko artifact starts the new one",
            (Verdict::Fail, _) => ": restart the session to load the new one",
            _ => "",
        };
        let (about, session) = match (claude_of(proc, all), what) {
            (Some(claude), _) => about(claude, records.registry.of(&process(claude, records)).as_ref()),
            // `ekko serve` runs in a session of its own, for every board.
            (None, "ekko serve") => ("this machine".to_string(), None),
            (None, _) => (format!("no Claude Code process, under pid {}", proc.parent), None),
        };
        found.push(Check { check: OLD_BINARY, verdict, about, session, says: format!("{what} (pid {}): {says}{mend}", proc.pid) });
    }
    // The plugin's server, not the resources' registered beside it: a
    // session that runs it loaded the plugin, whose SessionStart hook
    // records the session as it starts.
    let mut sessions: Vec<(&Proc, &Proc)> = served
        .iter()
        .filter(|proc| proc.serves() == Some("ekko --mcp"))
        .filter_map(|server| Some((claude_of(server, all)?, *server)))
        .collect();
    sessions.sort_by_key(|(claude, _)| claude.pid);
    sessions.dedup_by_key(|(claude, _)| claude.pid);
    for (claude, server) in sessions {
        let process = process(claude, records);
        let record = records.registry.of(&process);
        let (verdict, says) = match &record {
            Some(running) => (Verdict::Ok, format!("its SessionStart hook recorded it, on its conversation since {}", when(running.since))),
            None => (
                Verdict::Fail,
                "it runs ekko's MCP server, and no SessionStart hook recorded it: the plugin's hooks did not run there, so it has no prime, no wake and no guard; restart it, and if this stays, check that the plugin is enabled".to_string(),
            ),
        };
        let (named, session) = about(claude, record.as_ref());
        found.push(Check { check: HOOKS_LOADED, verdict, about: named.clone(), session: session.clone(), says });
        // Where the hook recorded the session, the board whose file its
        // FileChanged hook watches.
        let Some(board) = record.as_ref().and_then(|running| running.board.as_deref()) else { continue };
        let heard = Told::within(records.told, &process).last_heard();
        let before = server.exe.as_deref().and_then(|exe| before_records(exe, records.store));
        let started = records.registry.recorded_at(&process);
        let (verdict, says) = wake_heard(version(board), started, heard.as_ref(), before.as_deref(), records.now);
        found.push(Check { check: WAKE_HEARD, verdict, about: named.clone(), session: session.clone(), says });
        // The conversation it runs, whose transcript holds its Bash calls.
        let Some(transcript) = record.as_ref().and_then(|running| running.transcript.as_deref()) else { continue };
        let guarded = Told::within(records.told, &process).last_guarded();
        let (verdict, says) = guard_ran(last_bash(transcript, TAIL), started, guarded.as_ref(), before.as_deref(), records.now);
        found.push(Check { check: GUARD_RAN, verdict, about: named, session, says });
    }
    found
}

impl Report {
    pub fn failed(&self) -> bool {
        self.checks.iter().any(|check| check.verdict == Verdict::Fail)
    }

    pub fn text(&self) -> String {
        let mut out = String::new();
        let count = |verdict: Verdict| self.checks.iter().filter(|check| check.verdict == verdict).count();
        let counts: Vec<String> = [Verdict::Ok, Verdict::Fail, Verdict::Skipped]
            .into_iter()
            .filter(|&verdict| count(verdict) > 0)
            .map(|verdict| format!("{} {}", count(verdict), verdict.word()))
            .collect();
        let _ = writeln!(out, "ekko --doctor \u{b7} {}", if counts.is_empty() { "no session runs ekko's MCP server, and no ekko serve runs: nothing to check".to_string() } else { counts.join(" \u{b7} ") });
        for name in [OLD_BINARY, HOOKS_LOADED, WAKE_HEARD, GUARD_RAN] {
            let of: Vec<&Check> = self.checks.iter().filter(|check| check.check == name).collect();
            if of.is_empty() {
                continue;
            }
            let _ = writeln!(out, "{name}");
            for check in of {
                let _ = writeln!(out, "  {:<7} {}: {}", check.verdict.word(), check.about, check.says);
            }
        }
        out
    }
}

/// `ekko --doctor`: the report on every session on this machine, as text or
/// JSON, exiting 1 when a check fails.
pub fn run(home: &Path, json: bool) -> ExitCode {
    let boot = crate::holder::boot().unwrap_or_default();
    let records = Records {
        registry: &Registry::at(crate::agent::processes_dir(home)),
        told: &crate::wake::told_dir(home),
        boot: &boot,
        store: Path::new(STORE),
        now: chrono::Local::now().timestamp_millis(),
    };
    let report = Report { checks: checks(&Proc::all(), &records) };
    if json {
        // The envelope of every --json answer: `ok` says the command ran,
        // and the exit says whether a check failed, as for the text.
        let failed = report.checks.iter().filter(|check| check.verdict == Verdict::Fail).count();
        println!("{}", serde_json::json!({"ok": true, "command": "doctor", "failed": failed, "checks": report.checks}));
    } else {
        print!("{}", report.text());
    }
    if report.failed() { ExitCode::FAILURE } else { ExitCode::SUCCESS }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binary(dir: &Path, name: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt as _;
        let path = dir.join(name).join("ekko");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, name).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    /// What the checks read beside /proc, in a test's folders.
    fn records<'a>(registry: &'a Registry, told: &'a Path, now: i64) -> Records<'a> {
        Records { registry, told, boot: "boot-1", store: Path::new(STORE), now }
    }

    fn id(path: &Path) -> Option<(u64, u64)> {
        let file = fs::metadata(path).unwrap();
        Some((file.dev(), file.ino()))
    }

    fn server(pid: u32, parent: u32, program: &Path, running: &Path, path_var: &Path) -> Proc {
        Proc {
            pid,
            parent,
            start: 7,
            comm: "ekko".into(),
            args: vec![program.to_string_lossy().into_owned(), "--mcp".into()],
            running: id(running),
            exe: Some(running.to_path_buf()),
            cwd: Some(PathBuf::from("/")),
            path_var: Some(path_var.as_os_str().to_os_string()),
            ..Proc::default()
        }
    }

    /// A server is judged as its own `REPLACED` judges it: by the name it was
    /// started by, and, started by its store path, by ekko on its PATH too.
    #[test]
    fn a_server_whose_names_lead_to_another_binary_now_fails() {
        let dir = crate::paths::test_dir("ekko-doctor-binary");
        let (old, new, dev) = (binary(&dir, "old"), binary(&dir, "new"), binary(&dir, "dev"));
        let store = dir.join("store");
        let pinned = binary(&store, "hash-ekko-0.38.1");
        let on_path = new.parent().unwrap();
        let verdict = |proc: &Proc| old_binary(proc, &store).0;

        assert_eq!(verdict(&server(1, 0, Path::new("ekko"), &new, on_path)), Verdict::Ok, "started by name, still where it leads");
        assert_eq!(verdict(&server(2, 0, Path::new("ekko"), &old, on_path)), Verdict::Fail, "the name moved to another binary");
        assert_eq!(verdict(&server(3, 0, &pinned, &pinned, on_path)), Verdict::Fail, "pinned in the store, and ekko on PATH moved on");
        assert_eq!(verdict(&server(4, 0, &pinned, &pinned, pinned.parent().unwrap())), Verdict::Ok, "pinned, and PATH still leads to it");
        assert_eq!(verdict(&server(5, 0, &dev, &dev, on_path)), Verdict::Ok, "a dev build is judged by its path alone");
        assert_eq!(verdict(&server(6, 0, &dev, &old, on_path)), Verdict::Fail, "a dev build rebuilt in place");
        let relative = Proc { cwd: Some(dir.clone()), ..server(7, 0, Path::new("dev/ekko"), &dev, on_path) };
        assert_eq!(verdict(&relative), Verdict::Ok, "a relative path is looked up from the server's folder");
        assert_eq!(verdict(&Proc { cwd: Some(dir.clone()), ..server(8, 0, Path::new("dev/ekko"), &old, on_path) }), Verdict::Fail);
        fs::remove_file(&dev).unwrap();
        assert_eq!(verdict(&server(9, 0, &dev, &old, on_path)), Verdict::Skipped, "a name that leads nowhere now is no upgrade");
        let (_, says) = old_binary(&server(3, 0, &pinned, &pinned, on_path), &store);
        assert!(says.starts_with("it runs ekko-0.38.1, and ekko on its PATH leads to "), "{says}");
        fs::remove_dir_all(&dir).ok();
    }

    /// A Claude Code process running the plugin's server has a SessionStart
    /// record, or its hooks did not run; the resources' server alone does not
    /// say the plugin is there.
    #[test]
    fn a_session_with_the_plugins_server_and_no_session_start_record_fails() {
        let dir = crate::paths::test_dir("ekko-doctor-hooks");
        let new = binary(&dir, "new");
        let on_path = new.parent().unwrap();
        let registry = Registry::at(dir.join("processes"));
        let told = dir.join("told");
        let claude = |pid: u32| Proc { pid, parent: 1, start: 11, comm: ".claude-unwrapp".into(), ..Proc::default() };
        let mut resources = server(21, 200, Path::new("ekko"), &new, on_path);
        resources.args.push("--resources".into());
        let alone = server(22, 1, Path::new("ekko"), &new, on_path);
        // Two of ekko's servers in one session, as when a config starts one beside the plugin's.
        let second = server(23, 100, Path::new("ekko"), &new, on_path);
        let all = vec![claude(100), server(20, 100, Path::new("ekko"), &new, on_path), claude(200), resources, alone, second];
        let hooks = |all: &[Proc]| -> Vec<(Verdict, String)> {
            checks(all, &records(&registry, &told, 0))
                .into_iter()
                .filter(|check| check.check == HOOKS_LOADED)
                .map(|check| (check.verdict, check.about))
                .collect()
        };

        assert_eq!(hooks(&all), [(Verdict::Fail, "Claude Code pid 100".to_string())]);
        let report = Report { checks: checks(&all, &records(&registry, &told, 0)) };
        assert!(report.failed());
        assert!(report.text().starts_with("ekko --doctor \u{b7} 4 ok \u{b7} 1 fail\nOld binary\n"), "{}", report.text());
        let record = |start: u64, boot: &str| Running {
            pid: 100,
            start,
            boot: boot.into(),
            profile: Some("default".into()),
            tty: Some("pts/4".into()),
            config_dir: None,
            board: None,
            transcript: None,
            conversation: "4461f145-f1e0".into(),
            earlier: Vec::new(),
            since: 0,
        };
        // Another process that had the pid, before or in another boot.
        registry.record(record(10, "boot-1")).unwrap();
        registry.record(record(11, "boot-0")).unwrap();
        assert_eq!(hooks(&all), [(Verdict::Fail, "Claude Code pid 100".to_string())]);
        registry.record(record(11, "boot-1")).unwrap();
        assert_eq!(hooks(&all), [(Verdict::Ok, "default on pts/4 \u{b7} 4461f145".to_string())]);
        let report = Report { checks: checks(&all, &records(&registry, &told, 0)) };
        assert!(!report.failed(), "{}", report.text());
        fs::remove_dir_all(&dir).ok();
    }

    /// A session whose board was written after its SessionStart, and longer
    /// ago than the margin, heard that write, or its FileChanged hook is
    /// deaf; a newer write, or one before the SessionStart, is not judged.
    #[test]
    fn a_session_whose_wake_hook_did_not_hear_the_last_write_fails() {
        use std::time::{Duration, UNIX_EPOCH};
        let dir = crate::paths::test_dir("ekko-doctor-wake");
        let new = binary(&dir, "new");
        let registry = Registry::at(dir.join("processes"));
        let told = dir.join("told");
        let board = dir.join("storage.json");
        fs::write(&board, "{}").unwrap();
        let all = vec![
            Proc { pid: 100, parent: 1, start: 11, comm: "claude".into(), ..Proc::default() },
            server(20, 100, Path::new("ekko"), &new, new.parent().unwrap()),
        ];
        registry
            .record(Running {
                pid: 100,
                start: 11,
                boot: "boot-1".into(),
                profile: None,
                tty: None,
                config_dir: None,
                board: Some(board.clone()),
                transcript: Some(dir.join("c-1.jsonl")),
                conversation: "c-1".into(),
                earlier: Vec::new(),
                since: 0,
            })
            .unwrap();
        let at = |path: &Path, millis: u64| {
            fs::File::options().write(true).open(path).unwrap().set_modified(UNIX_EPOCH + Duration::from_millis(millis)).unwrap();
        };
        // The SessionStart at 1,000 s, the board's last write at 2,000.25 s.
        at(&dir.join("processes").join("boot1-100-11.json"), 1_000_000);
        at(&board, 2_000_250);
        let heard = |heard: &Heard| {
            fs::create_dir_all(told.join("boot1-100-11")).unwrap();
            fs::write(told.join("boot1-100-11").join("heard"), serde_json::to_string(heard).unwrap()).unwrap();
        };
        let wake = |now: i64| -> (Verdict, String) {
            let check = checks(&all, &records(&registry, &told, now)).into_iter().find(|check| check.check == WAKE_HEARD).unwrap();
            (check.verdict, check.says)
        };
        let minute_after = 2_060_000;

        let (verdict, says) = wake(minute_after);
        assert_eq!(verdict, Verdict::Fail, "{says}");
        assert!(says.ends_with("and its wake hook recorded hearing nothing: its FileChanged hook did not run; restart the session to watch the file again"), "{says}");
        let (inode, mtime_ns, size) = version(&board).unwrap();
        assert_eq!(mtime_ns, 2_000_250_000_000);
        heard(&Heard { inode: inode + 1, mtime_ns: 1_500_000_000_000, size, revision: 1, at: 1_500_600 });
        let (verdict, says) = wake(minute_after);
        assert_eq!(verdict, Verdict::Fail, "a hook that last heard an older version: {says}");
        heard(&Heard { inode, mtime_ns, size, revision: 2, at: 2_000_600 });
        assert_eq!(wake(minute_after).0, Verdict::Ok, "a hook that heard the last write");
        let (verdict, says) = wake(2_004_000);
        assert_eq!((verdict, says.as_str()), (Verdict::Skipped, "its board's last write, 3750 ms ago, is too new to judge: its hook may still be on its way"));
        at(&dir.join("processes").join("boot1-100-11.json"), 3_000_000);
        let (verdict, says) = wake(3_060_000);
        assert_eq!(verdict, Verdict::Skipped, "a write before the SessionStart: {says}");
        assert!(says.starts_with("nothing was written to its board since its SessionStart at "), "{says}");

        // The same session's Bash calls, by its transcript, and the guard's.
        let guard = |now: i64| checks(&all, &records(&registry, &told, now)).into_iter().find(|check| check.check == GUARD_RAN).unwrap().verdict;
        let call = serde_json::json!([{"type": "tool_use", "id": "toolu_1", "name": "Bash", "input": {"command": "ls"}}]);
        fs::write(dir.join("c-1.jsonl"), row("assistant", "1970-01-01T00:58:20Z", false, call) + "\n").unwrap();
        assert_eq!(guard(3_560_000), Verdict::Fail, "a call at 3,500 s, after the SessionStart, with no guard");
        fs::write(told.join("boot1-100-11").join("guarded"), r#"{"tool_use_id":"toolu_1","at":3500100}"#).unwrap();
        assert_eq!(guard(3_560_000), Verdict::Ok);
        fs::remove_dir_all(&dir).ok();
    }

    /// A hook that recorded nothing is judged by its ekko's version, since no
    /// hook recorded what it heard up to 0.39.1; and one that recorded the
    /// board's version now heard it, whatever the clocks say.
    #[test]
    fn a_wake_hook_that_recorded_nothing_is_judged_by_its_ekkos_version() {
        let board = Some((7, 2_000_000_000_000, 10));
        let (started, now) = (Some(1_000_000), 2_060_000);
        assert_eq!(wake_heard(board, started, None, Some("ekko-0.39.1"), now).0, Verdict::Skipped);
        assert_eq!(wake_heard(board, started, None, None, now).0, Verdict::Fail);
        let later = Heard { inode: 6, mtime_ns: 0, size: 0, revision: 0, at: 2_000_500 };
        assert_eq!(wake_heard(board, started, Some(&later), None, now).0, Verdict::Ok, "a hook that ran after the write heard it");
        let same = Heard { inode: 7, mtime_ns: 2_000_000_000_000, size: 10, revision: 0, at: 0 };
        assert_eq!(wake_heard(board, started, Some(&same), None, now).0, Verdict::Ok, "a hook that heard the version the board is at");
        assert_eq!(wake_heard(None, started, None, None, now).0, Verdict::Skipped, "a board that cannot be read");
        let store = Path::new(STORE);
        let before = |exe: &str| before_records(Path::new(exe), store);
        assert_eq!(before("/nix/store/h-ekko-0.39.1/bin/ekko (deleted)").as_deref(), Some("ekko-0.39.1"));
        assert_eq!(before("/nix/store/h-ekko-0.38.0/bin/ekko").as_deref(), Some("ekko-0.38.0"));
        assert_eq!(before("/nix/store/h-ekko-0.39.2/bin/ekko"), None);
        assert_eq!(before("/nix/store/h-ekko-0.40.0/bin/ekko"), None);
        assert_eq!(before("/projects/ekko/target/release/ekko"), None, "a dev build");
    }

    /// A transcript row as Claude Code writes one, with its content blocks.
    fn row(kind: &str, at: &str, sidechain: bool, blocks: serde_json::Value) -> String {
        serde_json::json!({"type": kind, "timestamp": at, "isSidechain": sidechain, "message": {"role": kind, "content": blocks}}).to_string()
    }

    /// The last Bash call is the main conversation's, read from the end of
    /// the transcript, and an end with no row Claude Code writes says so.
    #[test]
    fn the_last_bash_call_is_read_from_the_end_of_the_transcript() {
        let dir = crate::paths::test_dir("ekko-doctor-transcript");
        fs::create_dir_all(&dir).unwrap();
        let transcript = dir.join("t.jsonl");
        let bash = |id: &str| serde_json::json!([{"type": "tool_use", "id": id, "name": "Bash", "input": {"command": "ls"}}]);
        let rows = [
            row("assistant", "2026-10-09T03:00:00.000Z", false, bash("toolu_old")),
            row("assistant", "2026-10-09T03:00:10.000Z", false, bash("toolu_last")),
            row("user", "2026-10-09T03:00:11.000Z", false, serde_json::json!([{"type": "tool_result", "tool_use_id": "toolu_last", "content": "ok"}])),
            row("assistant", "2026-10-09T03:00:20.000Z", false, serde_json::json!([{"type": "tool_use", "id": "toolu_read", "name": "Read", "input": {}}])),
            row("assistant", "2026-10-09T03:00:30.000Z", true, bash("toolu_subagent")),
        ];
        fs::write(&transcript, rows.join("\n") + "\n").unwrap();
        let at = chrono::DateTime::parse_from_rfc3339("2026-10-09T03:00:10Z").unwrap().timestamp_millis();
        assert_eq!(last_bash(&transcript, TAIL), Ok(Some(("toolu_last".to_string(), at))));
        // An end that starts inside a row reads from the next whole one.
        let cut = rows[3].len() as u64 + rows[4].len() as u64 + 2 + 10;
        assert_eq!(last_bash(&transcript, cut), Ok(None), "no Bash call in the end read");
        assert_eq!(last_bash(&transcript, 40), Ok(None), "an end inside the last row holds no row whole, and is no error");
        fs::write(&transcript, "not a transcript\nat all\n").unwrap();
        assert!(last_bash(&transcript, TAIL).unwrap_err().starts_with("its transcript's end holds no row"));
        assert!(last_bash(&dir.join("gone.jsonl"), TAIL).unwrap_err().starts_with("its transcript cannot be read"));
        fs::remove_dir_all(&dir).ok();
    }

    /// A Bash call after the SessionStart, and longer ago than the margin,
    /// ran past the guard, or its PreToolUse hook does not run.
    #[test]
    fn a_bash_call_the_guard_did_not_see_fails() {
        let call = |id: &str, at: i64| Ok(Some((id.to_string(), at)));
        let (started, now) = (Some(1_000_000), 2_060_000);
        let guarded = |id: &str, at: i64| Guarded { tool_use_id: id.to_string(), at };
        let verdict = |last, guard: Option<&Guarded>, before: Option<&str>, now| guard_ran(last, started, guard, before, now).0;
        assert_eq!(verdict(call("toolu_1", 2_000_000), Some(&guarded("toolu_1", 1_000_100)), None, now), Verdict::Ok, "the guard ran for that call, whatever its clock");
        assert_eq!(verdict(call("toolu_1", 2_000_000), Some(&guarded("toolu_2", 2_003_000)), None, now), Verdict::Ok, "for another, about then");
        assert_eq!(verdict(call("toolu_1", 2_000_000), Some(&guarded("toolu_0", 1_990_000)), None, now), Verdict::Fail, "its last run was before");
        assert_eq!(verdict(call("toolu_1", 2_000_000), None, None, now), Verdict::Fail, "it never ran");
        assert_eq!(verdict(call("toolu_1", 2_000_000), None, Some("ekko-0.39.1"), now), Verdict::Skipped, "an ekko that records nothing");
        assert_eq!(verdict(call("toolu_1", 2_000_000), None, None, 2_004_000), Verdict::Skipped, "too new");
        assert_eq!(verdict(call("toolu_1", 900_000), None, None, now), Verdict::Skipped, "before the SessionStart");
        assert_eq!(verdict(Ok(None), None, None, now), Verdict::Skipped, "no Bash call");
        assert_eq!(verdict(Err("unreadable".into()), None, None, now), Verdict::Skipped);
        let (_, says) = guard_ran(call("toolu_1", 2_000_000), started, Some(&guarded("toolu_0", 1_990_000)), None, now);
        assert!(says.ends_with("its PreToolUse hook did not run; restart the session"), "{says}");
    }

    /// The doctor reads a process as /proc shows it: this test's own.
    #[test]
    fn a_process_is_read_from_proc() {
        let me = Proc::read(std::process::id()).expect("/proc reads this process");
        assert_eq!(me.running, id(&std::env::current_exe().unwrap()));
        assert_eq!(me.path_var, std::env::var_os("PATH"));
        assert_eq!(me.parent, std::os::unix::process::parent_id());
        assert!(!me.args.is_empty());
        assert_eq!(me.cwd, std::env::current_dir().ok());
        assert_eq!(me.serves(), None);
    }

    /// The ekkos checked are the servers, by their arguments: the MCP
    /// servers and `ekko serve`, not a hook or a command.
    #[test]
    fn the_ekkos_checked_are_the_servers() {
        let serves = |args: &[&str]| Proc { args: args.iter().map(|arg| arg.to_string()).collect(), ..Proc::default() }.serves();
        assert_eq!(serves(&["/nix/store/x-ekko-0.39.1/bin/ekko", "--mcp"]), Some("ekko --mcp"));
        assert_eq!(serves(&["ekko", "--mcp", "--resources"]), Some("ekko --mcp --resources"));
        assert_eq!(serves(&["/x/ekko", "serve"]), Some("ekko serve"));
        assert_eq!(serves(&["ekko", "--json", "serve"]), Some("ekko serve"));
        assert_eq!(serves(&["ekko", "--guard", "--hook"]), None, "a hook");
        assert_eq!(serves(&["ekko", "--task", "serve", "it"]), None, "serve as a word of a task");
        assert_eq!(serves(&["ekko", "--doctor"]), None);
        assert_eq!(serves(&["node", "server.js", "--mcp"]), None, "another program");
        assert_eq!(serves(&[]), None);
    }
}
