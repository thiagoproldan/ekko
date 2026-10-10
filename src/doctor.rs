//! `ekko --doctor`: checks, from the side that consumes them, that ekko's
//! links with Claude Code work (plan 1279, task 1282). They break without an
//! error anywhere: a session's MCP server keeps the binary it started with
//! after an upgrade (gotcha 194), a session whose plugin hooks did not run
//! has no prime, no wake and no guard, and one whose FileChanged hook stopped
//! hearing the board's file is never woken (task 1248, 1283). The checks
//! read /proc and ekko's state directory, and write nothing -- `--probe`
//! asks for one write of the board, which they judge (task 1286); finding the
//! board of a session no hook recorded reads the config file, as every ekko
//! command does, which writes the defaults where it is missing. Each gives
//! ok, fail, or skipped when it cannot judge, in one line, about one
//! session; a fail makes the exit 1. No check gives a warning yet: the
//! plan's warn comes with the first that has one to give. The prime runs
//! those that read no transcript, and names in one line the sessions on its
//! board that fail one (task 1287).

use std::ffi::{OsStr, OsString};
use std::fmt::Write as _;
use std::fs;
use std::os::unix::ffi::OsStringExt as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde::Serialize;

use crate::ekko::EkkoError;
use crate::holder::{when, Process, Registry, Running};
use crate::mcp::Binary;
use crate::storage::Storage;
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
    /// A fail about a session as the prime tells it, after the session's
    /// name (task 1287).
    #[serde(skip)]
    pub brief: Option<String>,
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
    /// The board it runs on, by its storage file: as its SessionStart hook
    /// recorded it, or else as its ekko finds its own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub board: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub checks: Vec<Check>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probe: Option<Probe>,
}

/// What `--probe` did (plan 1279, Design 3): the board it wrote again, when
/// that version was put in place, in milliseconds, and how many of the
/// sessions on the board whose hooks record what they hear heard it, within
/// `MARGIN` of the write.
#[derive(Debug, Serialize)]
pub struct Probe {
    pub board: PathBuf,
    pub written: i64,
    pub sessions: usize,
    pub heard: usize,
}

const OLD_BINARY: &str = "Old binary";
const HOOKS_LOADED: &str = "Hooks loaded";
const WAKE_HEARD: &str = "Wake heard";
const GUARD_RAN: &str = "Guard ran";
const SERVED_PAGE: &str = "Served page";

/// How much of a transcript's end the guard's check reads for the session's
/// last Bash call.
const TAIL: u64 = 1 << 20;

/// How old the board's last write must be before a session's wake hook is
/// judged on it, in milliseconds. The hook ran about 0.6 s after a write in
/// the measurements (notes 1264, 1265); a loaded machine may take longer,
/// and a margin too short raises false alarms (plan 1279's first risk).
/// A session's server, too, is this old before its hooks are judged: the
/// SessionStart hook recorded the session 0.13 and 0.16 s after its server
/// started, in two sessions measured (task 1555).
const MARGIN: i64 = 5_000;

/// What the checks read beside /proc: the SessionStart hook's records, the
/// wake hook's, this boot's id, where the store is, the time now, how long
/// this boot has run, in milliseconds, when /proc/uptime says, and the home
/// a board is found from. `transcripts` says whether Guard ran reads the
/// sessions' transcripts, Claude Code's files: the prime's checks read none
/// (plan 1279, Design 4).
struct Records<'a> {
    registry: &'a Registry,
    told: &'a Path,
    boot: &'a str,
    store: &'a Path,
    now: i64,
    up: Option<i64>,
    home: &'a Path,
    transcripts: bool,
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
    /// Its EKKO_DIR and EKKO_PROJECT, by which an ekko finds its board.
    ekko_dir: Option<String>,
    ekko_project: Option<String>,
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
        let environ = fs::read(format!("/proc/{pid}/environ")).unwrap_or_default();
        let var = |name: &str| environ.split(|&b| b == 0).find_map(|var| var.strip_prefix(name.as_bytes())?.strip_prefix(b"=").map(<[u8]>::to_vec));
        let text = |name: &str| var(name).map(|value| String::from_utf8_lossy(&value).into_owned());
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
            path_var: var("PATH").map(OsString::from_vec),
            ekko_dir: text("EKKO_DIR"),
            ekko_project: text("EKKO_PROJECT"),
        })
    }

    /// Every process /proc lets this user read that the checks are about,
    /// by its name, read first so that each other process costs one read.
    fn all() -> Vec<Proc> {
        let Ok(entries) = fs::read_dir("/proc") else { return Vec::new() };
        let named = |pid: &u32| checked(&fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default());
        entries.flatten().filter_map(|entry| entry.file_name().to_str()?.parse().ok()).filter(named).filter_map(Proc::read).collect()
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

/// Whether a process named `comm` is one the checks are about: an ekko or
/// a Claude Code, `.claude-unwrapped` behind a nix wrapper. Reading every
/// process whole added 26 ms to the prime over 453 processes, and reading
/// the name first 6.9 ms (task 1287).
fn checked(comm: &str) -> bool {
    ["ekko", "claude"].iter().any(|name| comm.trim().trim_start_matches('.').starts_with(name))
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
/// bare name `ekko` on its PATH too, when that leads into the store, as a
/// switch moves it. A dev build started by its path is judged by that path
/// alone: the ekko on PATH was never its. Nor is a store build judged by a
/// dev build on PATH, which a restart would not start in its place: the
/// NixOS wrapper's resources server stays pinned to the store (task 1572).
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
        if let Some(on_path) = Binary::resolve(OsStr::new("ekko"), proc.path_var.as_deref()).filter(|on_path| on_path.path.starts_with(store)) {
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

/// How long this boot has run, in milliseconds: the clock in which /proc
/// counts a process's start.
fn uptime() -> Option<i64> {
    let seconds: f64 = fs::read_to_string("/proc/uptime").ok()?.split_whitespace().next()?.parse().ok()?;
    Some((seconds * 1000.0) as i64)
}

/// The clock ticks in a second, in which /proc gives a process's start.
fn hertz() -> u64 {
    // SAFETY: sysconf reads a constant of the system and touches no memory.
    u64::try_from(unsafe { libc::sysconf(libc::_SC_CLK_TCK) }).ok().filter(|&hertz| hertz > 0).unwrap_or(100)
}

/// A process's start, given in clock ticks after boot, in milliseconds.
fn since_boot(start: u64) -> i64 {
    i64::try_from(start.saturating_mul(1000) / hertz()).unwrap_or(i64::MAX)
}

/// A time to the second, as the margin needs: the hour today, the day and
/// hour before that.
fn clock(millis: i64) -> String {
    use chrono::TimeZone as _;
    let Some(at) = chrono::Local.timestamp_millis_opt(millis).single() else { return millis.to_string() };
    if at.date_naive() == chrono::Local::now().date_naive() { at.format("%H:%M:%S").to_string() } else { at.format("%Y-%m-%d %H:%M:%S").to_string() }
}

/// Whether a wake hook's run heard `version` of the board's file: it read
/// that version, or ran after it was written, which proves its watch alive.
fn heard_it(heard: &Heard, version: (u64, i64, u64)) -> bool {
    (heard.inode, heard.mtime_ns, heard.size) == version || heard.at >= version.1.div_euclid(1_000_000)
}

/// Whether a session's wake hook heard the last write to its board: given
/// the board's version now, when its SessionStart hook last ran, what its
/// wake hook last heard, and the version of its ekko where that records
/// nothing. A write before the SessionStart is not judged, nor one newer
/// than `MARGIN` that the hook has not heard yet.
fn wake_heard(board: Option<(u64, i64, u64)>, started: Option<i64>, heard: Option<&Heard>, before: Option<&str>, now: i64) -> (Verdict, String) {
    let Some((inode, mtime_ns, size)) = board else {
        return (Verdict::Skipped, "its board's file cannot be read".to_string());
    };
    let Some(started) = started else {
        return (Verdict::Skipped, "when its SessionStart hook ran cannot be read".to_string());
    };
    let written = mtime_ns.div_euclid(1_000_000);
    if written <= started {
        return (Verdict::Skipped, format!("nothing was written to its board since its SessionStart at {}: nothing to hear yet", clock(started)));
    }
    // Any run after the write proves the watch alive; one that read the
    // version written says how long the write took to reach it.
    if let Some(heard) = heard.filter(|heard| heard_it(heard, (inode, mtime_ns, size))) {
        let after = if (heard.inode, heard.mtime_ns, heard.size) == (inode, mtime_ns, size) { format!(", {} ms after", (heard.at - written).max(0)) } else { String::new() };
        return (Verdict::Ok, format!("its wake hook heard the last write to its board, at {}{after}", clock(written)));
    }
    if let (None, Some(version)) = (heard, before) {
        return (Verdict::Skipped, format!("its {version} records nothing of what its wake hook heard, which began after 0.39.1"));
    }
    if now - written < MARGIN {
        return (Verdict::Skipped, format!("its board's last write, {} ms ago, is too new to judge: its hook may still be on its way", (now - written).max(0)));
    }
    let missed = format!("its board was written at {}, after its SessionStart at {}", clock(written), clock(started));
    match heard {
        Some(heard) => (
            Verdict::Fail,
            format!(
                "{missed}, and its wake hook last heard the version of {}: its FileChanged hook missed the write, as in task 1248; restart the session to watch the file again",
                clock(heard.mtime_ns.div_euclid(1_000_000))
            ),
        ),
        None => (
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
    // A record of the guard's run for that very call passes at once, however
    // new the call; an ekko that records nothing is no call to wait on.
    let ran = format!("its guard ran for its last Bash call, at {}", clock(at));
    if guarded.is_some_and(|guarded| guarded.tool_use_id == id) {
        return (Verdict::Ok, ran);
    }
    if let (None, Some(version)) = (guarded, before) {
        return (Verdict::Skipped, format!("its {version} records nothing of its guard's runs, which began after 0.39.1"));
    }
    if now - at < MARGIN {
        return (Verdict::Skipped, format!("its last Bash call, {} ms ago, is too new to judge", (now - at).max(0)));
    }
    match guarded {
        Some(guarded) if guarded.at + MARGIN >= at => (Verdict::Ok, ran),
        Some(guarded) => (
            Verdict::Fail,
            format!(
                "its last Bash call, at {}, came after the guard's last run there, at {}: its PreToolUse hook did not run; restart the session",
                clock(at),
                clock(guarded.at)
            ),
        ),
        None => (
            Verdict::Fail,
            format!("its last Bash call, at {}, ran with no record of the guard: its PreToolUse hook did not run there; restart the session, and if this stays, check that the plugin is enabled", clock(at)),
        ),
    }
}

/// What `ekko serve` shows of itself, from `dir`, ekko's state directory:
/// whether the server `serve.json` names answers `version`, and whether the
/// files that hold its token, `serve.json` and the `serve-open.html` beside
/// it, are the user's alone. Nothing where no server ever ran.
fn served_page(dir: &Path, version: &str) -> Vec<(Verdict, String)> {
    let runtime = dir.join(crate::serve::RUNTIME);
    let Ok(text) = fs::read(&runtime) else { return Vec::new() };
    let mut found = Vec::new();
    for file in [runtime, dir.join(crate::serve::OPEN)] {
        let Ok(meta) = fs::metadata(&file) else { continue };
        let mode = meta.mode() & 0o777;
        let name = file.file_name().unwrap_or_default().to_string_lossy();
        found.push(if mode & 0o077 == 0 {
            (Verdict::Ok, format!("{name} is readable by you alone (mode {mode:o})"))
        } else {
            (
                Verdict::Fail,
                format!("{name} is open to others (mode {mode:o}), and holds the token that stops the server and lets a page write as you: chmod 600 {}", file.display()),
            )
        });
    }
    found.push(match serde_json::from_slice::<crate::serve::Runtime>(&text) {
        Err(_) => (Verdict::Skipped, "serve.json cannot be read as a server's".to_string()),
        Ok(runtime) => match crate::serve::status(runtime.port) {
            None => (
                Verdict::Skipped,
                format!("nothing answers on 127.0.0.1:{}, where serve.json names the last server: none runs now, and the next ekko artifact starts one", runtime.port),
            ),
            Some(answered) if answered == version => (Verdict::Ok, format!("ekko serve on 127.0.0.1:{} answers this version, {version}", runtime.port)),
            Some(answered) => (
                Verdict::Fail,
                format!(
                    "ekko serve on 127.0.0.1:{} answers version {answered}, and this ekko is {version}: its pages follow {answered} until ekko serve --stop, or the next ekko artifact, replaces it",
                    runtime.port
                ),
            ),
        },
    });
    found
}

/// The Claude Code process `claude` as a check names it: as a claim names
/// its holder where its SessionStart hook recorded it, for a person; and for
/// machines, with `board`, the one it runs on.
fn about(claude: &Proc, record: Option<&Running>, board: Option<PathBuf>) -> (String, Option<Session>) {
    let name = match record {
        Some(running) => running.label(),
        None => format!("Claude Code pid {}{}", claude.pid, claude.tty.as_deref().map(|tty| format!(" on {tty}")).unwrap_or_default()),
    };
    let conversation = record.map(|running| running.conversation.clone());
    (name, Some(Session { pid: claude.pid, tty: claude.tty.clone(), folder: claude.cwd.clone(), conversation, board }))
}

/// The board a session runs on, by its storage file: the one its
/// SessionStart hook recorded, or else the one `server`, its ekko, finds
/// for itself from its folder, EKKO_DIR and EKKO_PROJECT, in the order
/// `directory::locate` gives them.
fn board_of(record: Option<&Running>, server: &Proc, home: &Path) -> Option<PathBuf> {
    if let Some(board) = record.and_then(|running| running.board.clone()) {
        return Some(board);
    }
    let location = crate::directory::locate(home, server.cwd.as_deref()?, None, server.ekko_dir.as_deref(), server.ekko_project.as_deref()).ok()?;
    Some(location.dir.join("storage").join("storage.json"))
}

/// Whether two paths name one board's file, however each is reached -- a
/// folder may be mounted at two paths: by the folder that holds the file,
/// which a write keeps while it replaces the file.
fn same_board(a: &Path, b: &Path) -> bool {
    let folder = |file: &Path| fs::metadata(file.parent()?).ok().map(|meta| (meta.dev(), meta.ino()));
    folder(a).is_some_and(|a| folder(b) == Some(a))
}

/// The Claude Code process `proc` runs under, if its parent is one.
fn claude_of<'a>(proc: &Proc, all: &'a [Proc]) -> Option<&'a Proc> {
    all.iter().find(|parent| parent.pid == proc.parent && parent.is_claude())
}

/// The Claude Code sessions among `all`, each with its server: the
/// plugin's, not the resources' registered beside it. A session that runs
/// it loaded the plugin, whose SessionStart hook records the session as it
/// starts.
fn sessions(all: &[Proc]) -> Vec<(&Proc, &Proc)> {
    let mut sessions: Vec<(&Proc, &Proc)> =
        all.iter().filter(|proc| proc.serves() == Some("ekko --mcp")).filter_map(|server| Some((claude_of(server, all)?, server))).collect();
    sessions.sort_by_key(|(claude, _)| claude.pid);
    sessions.dedup_by_key(|(claude, _)| claude.pid);
    sessions
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
            (Some(claude), _) => {
                let record = records.registry.of(&process(claude, records));
                about(claude, record.as_ref(), board_of(record.as_ref(), proc, records.home))
            }
            // `ekko serve` runs in a session of its own, for every board.
            (None, "ekko serve") => ("this machine".to_string(), None),
            (None, _) => (format!("no Claude Code process, under pid {}", proc.parent), None),
        };
        let runs = proc.exe.as_deref().map_or_else(|| "an ekko".to_string(), |exe| named(exe, records.store));
        let brief = (verdict == Verdict::Fail).then(|| format!("it runs {runs}, replaced since"));
        found.push(Check { check: OLD_BINARY, verdict, about, session, says: format!("{what} (pid {}): {says}{mend}", proc.pid), brief });
    }
    for (claude, server) in sessions(all) {
        let process = process(claude, records);
        let record = records.registry.of(&process);
        // A server just started is ahead of the hook that records it.
        let young = records.up.map(|up| up - since_boot(server.start)).filter(|&age| age < MARGIN);
        let (verdict, says) = match (&record, young) {
            (Some(running), _) => (Verdict::Ok, format!("its SessionStart hook recorded it, on its conversation since {}", when(running.since))),
            (None, Some(age)) => {
                (Verdict::Skipped, format!("its ekko --mcp started {} ms ago, too new to judge: its SessionStart hook may still be on its way", age.max(0)))
            }
            (None, None) => (
                Verdict::Fail,
                "it runs ekko's MCP server, and no SessionStart hook recorded it: the plugin's hooks did not run there, so it has no prime, no wake and no guard; restart it, and if this stays, check that the plugin is enabled".to_string(),
            ),
        };
        let brief = |verdict: Verdict, what: &str| (verdict == Verdict::Fail).then(|| what.to_string());
        let (named, session) = about(claude, record.as_ref(), board_of(record.as_ref(), server, records.home));
        let check = Check { check: HOOKS_LOADED, verdict, about: named.clone(), session: session.clone(), says, brief: brief(verdict, "its hooks did not run") };
        found.push(check);
        // Where the hook recorded the session, the board whose file its
        // FileChanged hook watches.
        let Some(board) = record.as_ref().and_then(|running| running.board.as_deref()) else { continue };
        let heard = Told::within(records.told, &process).last_heard();
        let before = server.exe.as_deref().and_then(|exe| before_records(exe, records.store));
        let started = records.registry.recorded_at(&process);
        let (verdict, says) = wake_heard(version(board), started, heard.as_ref(), before.as_deref(), records.now);
        let brief_wake = brief(verdict, "its FileChanged hook missed the board's last write");
        found.push(Check { check: WAKE_HEARD, verdict, about: named.clone(), session: session.clone(), says, brief: brief_wake });
        // The conversation it runs, whose transcript holds its Bash calls.
        let Some(transcript) = record.as_ref().and_then(|running| running.transcript.as_deref()).filter(|_| records.transcripts) else { continue };
        let guarded = Told::within(records.told, &process).last_guarded();
        let (verdict, says) = guard_ran(last_bash(transcript, TAIL), started, guarded.as_ref(), before.as_deref(), records.now);
        found.push(Check { check: GUARD_RAN, verdict, about: named, session, says, brief: brief(verdict, "its guard missed its last Bash call") });
    }
    found
}

/// How many sessions the prime's line names; it counts the rest.
const NAMED: usize = 2;

/// The prime's line from `found`, on the sessions on the board whose file
/// is `board`: each that fails a check, once, by its first fail, with `me`
/// -- the Claude Code process the prime is for -- first and as "this
/// session"; the first `NAMED` named and the rest counted. `None` when none
/// fails.
fn line(found: &[Check], board: &Path, me: Option<u32>) -> Option<String> {
    let mut sessions: Vec<(u32, String)> = Vec::new();
    for check in found.iter().filter(|check| check.verdict == Verdict::Fail) {
        let (Some(session), Some(brief)) = (&check.session, &check.brief) else { continue };
        if !session.board.as_deref().is_some_and(|theirs| same_board(theirs, board)) || sessions.iter().any(|(pid, _)| *pid == session.pid) {
            continue;
        }
        let who = if me == Some(session.pid) { "this session" } else { &check.about };
        sessions.push((session.pid, format!("{who} ({brief})")));
    }
    if sessions.is_empty() {
        return None;
    }
    sessions.sort_by_key(|(pid, _)| me != Some(*pid));
    let named: Vec<&str> = sessions.iter().take(NAMED).map(|(_, said)| said.as_str()).collect();
    let more = sessions.len().saturating_sub(NAMED);
    let more = if more > 0 { format!("; +{more} more") } else { String::new() };
    Some(format!("sessions on this board to restart: {}{more} -- ekko --doctor says why", named.join("; ")))
}

/// The prime's line on the sessions on the board whose file is `board`
/// (task 1287): those that fail a check reading /proc and ekko's state
/// directory, never the transcripts nor the served page, with `me`, the
/// Claude Code process the prime is for, named "this session". `None` when
/// none fails.
pub fn attention(home: &Path, board: &Path, me: Option<u32>) -> Option<String> {
    line(&check_all(home, &crate::holder::boot()?, false), board, me)
}

/// The checks over every process running now, with the records ekko's
/// hooks keep under `home` and `boot`, this boot's id; `transcripts` as in
/// `Records`.
fn check_all(home: &Path, boot: &str, transcripts: bool) -> Vec<Check> {
    let records = Records {
        registry: &Registry::at(crate::agent::processes_dir(home)),
        told: &crate::wake::told_dir(home),
        boot,
        store: Path::new(STORE),
        now: chrono::Local::now().timestamp_millis(),
        up: uptime(),
        home,
        transcripts,
    };
    checks(&Proc::all(), &records)
}

/// `ekko --doctor --probe` (task 1286): writes the board `storage` holds
/// again, under its lock, as every write does -- the same items, a new
/// version put in place by rename and kept in history -- then waits until
/// every session on it that records what its wake hook hears, and began
/// before the write, has heard that version, or `MARGIN` has passed. The
/// checks after it judge each session on the probe's write.
pub fn probe(storage: &Storage, home: &Path) -> Result<Probe, EkkoError> {
    let board = storage.storage_path();
    let version = {
        let _lock = storage.acquire_lock()?;
        let items = storage.get_shared()?;
        storage.set(&items)?;
        version(board).ok_or_else(|| EkkoError::InvalidInput(format!("{} cannot be read after the probe's write", board.display())))?
    };
    let written = version.1.div_euclid(1_000_000);
    let (registry, told) = (Registry::at(crate::agent::processes_dir(home)), crate::wake::told_dir(home));
    let boot = crate::holder::boot().unwrap_or_default();
    loop {
        let now = chrono::Local::now().timestamp_millis();
        let records = Records { registry: &registry, told: &told, boot: &boot, store: Path::new(STORE), now, up: None, home, transcripts: false };
        let (sessions, heard) = listening(&Proc::all(), &records, board, version);
        if heard == sessions || now - written >= MARGIN {
            return Ok(Probe { board: board.to_path_buf(), written, sessions, heard });
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// The sessions among `all` that the probe of `board` waits for, and how
/// many of them heard `version`, the one it wrote: each whose SessionStart
/// hook recorded it on that board before the write, and whose ekko records
/// what its wake hook hears -- not 0.39.1 or before.
fn listening(all: &[Proc], records: &Records, board: &Path, version: (u64, i64, u64)) -> (usize, usize) {
    let written = version.1.div_euclid(1_000_000);
    let waited: Vec<Process> = sessions(all)
        .into_iter()
        .filter(|(_, server)| server.exe.as_deref().and_then(|exe| before_records(exe, records.store)).is_none())
        .map(|(claude, _)| process(claude, records))
        .filter(|process| records.registry.of(process).and_then(|running| running.board).is_some_and(|theirs| same_board(&theirs, board)))
        .filter(|process| records.registry.recorded_at(process).is_some_and(|started| started < written))
        .collect();
    let heard = waited.iter().filter(|process| Told::within(records.told, process).last_heard().is_some_and(|heard| heard_it(&heard, version))).count();
    (waited.len(), heard)
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
        if let Some(probe) = &self.probe {
            let heard = match (probe.sessions, probe.heard) {
                (0, _) => "no session on it records what its wake hook hears: nothing to wait for".to_string(),
                (sessions, heard) if heard == sessions => format!("every session on it heard the write ({heard} of {sessions})"),
                (sessions, heard) => format!("{heard} of {sessions} sessions on it heard the write within {} s", MARGIN / 1000),
            };
            let _ = writeln!(out, "Probe: wrote {} again at {}; {heard}", probe.board.display(), clock(probe.written));
        }
        for name in [OLD_BINARY, HOOKS_LOADED, WAKE_HEARD, GUARD_RAN, SERVED_PAGE] {
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

/// `ekko --doctor`: the report on every session on this machine, after
/// `probe` where `--probe` wrote, as text or JSON, exiting 1 when a check
/// fails.
pub fn run(home: &Path, json: bool, probe: Option<Probe>) -> ExitCode {
    let mut report = Report { checks: check_all(home, &crate::holder::boot().unwrap_or_default(), true), probe };
    let served = served_page(&crate::agent::state_dir(home), env!("CARGO_PKG_VERSION"));
    let page = |(verdict, says)| Check { check: SERVED_PAGE, verdict, about: "this machine".to_string(), session: None, says, brief: None };
    report.checks.extend(served.into_iter().map(page));
    if json {
        // The envelope of every --json answer: `ok` says the command ran,
        // and the exit says whether a check failed, as for the text.
        let failed = report.checks.iter().filter(|check| check.verdict == Verdict::Fail).count();
        let mut reply = serde_json::json!({"ok": true, "command": "doctor", "failed": failed, "checks": report.checks});
        if let Some(probe) = &report.probe {
            reply["probe"] = serde_json::json!(probe);
        }
        println!("{reply}");
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

    /// What the checks read beside /proc, in a test's folders, 1,000 s into
    /// the boot, with `told`'s folder as the home.
    fn records<'a>(registry: &'a Registry, told: &'a Path, now: i64) -> Records<'a> {
        let home = told.parent().unwrap();
        Records { registry, told, boot: "boot-1", store: Path::new(STORE), now, up: Some(1_000_000), home, transcripts: true }
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
    /// started by, and, started by its store path, by ekko on its PATH too,
    /// when that is a store build.
    #[test]
    fn a_server_whose_names_lead_to_another_binary_now_fails() {
        let dir = crate::paths::test_dir("ekko-doctor-binary");
        let (old, new, dev) = (binary(&dir, "old"), binary(&dir, "new"), binary(&dir, "dev"));
        // Canonical, as the path ekko on PATH resolves to is.
        let store = fs::canonicalize(&dir).unwrap().join("store");
        let (pinned, switched) = (binary(&store, "hash-ekko-0.38.1"), binary(&store, "hash-ekko-0.39.1"));
        let on_path = new.parent().unwrap();
        let verdict = |proc: &Proc| old_binary(proc, &store).0;

        assert_eq!(verdict(&server(1, 0, Path::new("ekko"), &new, on_path)), Verdict::Ok, "started by name, still where it leads");
        assert_eq!(verdict(&server(2, 0, Path::new("ekko"), &old, on_path)), Verdict::Fail, "the name moved to another binary");
        assert_eq!(verdict(&server(3, 0, &pinned, &pinned, switched.parent().unwrap())), Verdict::Fail, "pinned in the store, and a switch moved ekko on PATH");
        assert_eq!(verdict(&server(4, 0, &pinned, &pinned, pinned.parent().unwrap())), Verdict::Ok, "pinned, and PATH still leads to it");
        assert_eq!(verdict(&server(10, 0, &pinned, &pinned, on_path)), Verdict::Ok, "pinned, beside a build outside the store on PATH, which a restart would not start");
        assert_eq!(verdict(&server(5, 0, &dev, &dev, on_path)), Verdict::Ok, "a dev build is judged by its path alone");
        assert_eq!(verdict(&server(6, 0, &dev, &old, on_path)), Verdict::Fail, "a dev build rebuilt in place");
        let relative = Proc { cwd: Some(dir.clone()), ..server(7, 0, Path::new("dev/ekko"), &dev, on_path) };
        assert_eq!(verdict(&relative), Verdict::Ok, "a relative path is looked up from the server's folder");
        assert_eq!(verdict(&Proc { cwd: Some(dir.clone()), ..server(8, 0, Path::new("dev/ekko"), &old, on_path) }), Verdict::Fail);
        fs::remove_file(&dev).unwrap();
        assert_eq!(verdict(&server(9, 0, &dev, &old, on_path)), Verdict::Skipped, "a name that leads nowhere now is no upgrade");
        let (_, says) = old_binary(&server(3, 0, &pinned, &pinned, switched.parent().unwrap()), &store);
        assert!(says.starts_with("it runs ekko-0.38.1, and ekko on its PATH leads to ekko-0.39.1 now"), "{says}");
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
        // A server started 1 s ago is ahead of the hook that records it; one
        // of 10 s ago is not, nor one whose age /proc/uptime does not tell.
        let started = |ago: u64| (1_000_000 - ago) * hertz() / 1000;
        let young = |ago: u64| vec![claude(100), Proc { start: started(ago), ..server(20, 100, Path::new("ekko"), &new, on_path) }];
        let found = checks(&young(1_000), &records(&registry, &told, 0));
        let check = found.iter().find(|check| check.check == HOOKS_LOADED).unwrap();
        assert_eq!(check.verdict, Verdict::Skipped, "{}", check.says);
        assert!(check.says.ends_with(" ms ago, too new to judge: its SessionStart hook may still be on its way"), "{}", check.says);
        assert_eq!(hooks(&young(10_000)), [(Verdict::Fail, "Claude Code pid 100".to_string())]);
        let unknown = checks(&young(1_000), &Records { up: None, ..records(&registry, &told, 0) });
        assert_eq!(unknown.iter().find(|check| check.check == HOOKS_LOADED).unwrap().verdict, Verdict::Fail);
        let report = Report { checks: checks(&all, &records(&registry, &told, 0)), probe: None };
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
        let report = Report { checks: checks(&all, &records(&registry, &told, 0)), probe: None };
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
        let (verdict, says) = wake(2_004_000);
        assert_eq!((verdict, says.as_str()), (Verdict::Skipped, "its board's last write, 3750 ms ago, is too new to judge: its hook may still be on its way"));
        heard(&Heard { inode, mtime_ns, size, revision: 2, at: 2_000_600 });
        assert_eq!(wake(minute_after).0, Verdict::Ok, "a hook that heard the last write");
        let (verdict, says) = wake(2_004_000);
        assert_eq!(verdict, Verdict::Ok, "a hook that heard a write too new to judge otherwise: {says}");
        assert!(says.ends_with(", 350 ms after"), "{says}");
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

    /// The prime's line names each session on its board that fails a check,
    /// once, by its first fail, this session first, and counts those past
    /// `NAMED`; a session on another board, or one that passes, is not in
    /// it. A session's board is the one its hook recorded, or else the one
    /// its ekko finds. Every fail about a session has the words the line
    /// gives it, and the prime's checks read no transcript.
    #[test]
    fn the_primes_line_names_each_failing_session_on_its_board_once() {
        use std::time::{Duration, UNIX_EPOCH};
        let dir = crate::paths::test_dir("ekko-doctor-prime");
        let (old, new) = (binary(&dir, "old"), binary(&dir, "new"));
        let on_path = new.parent().unwrap();
        let registry = Registry::at(dir.join("processes"));
        let told = dir.join("told");
        let file = |project: &str| dir.join(project).join(".ekko").join("storage").join("storage.json");
        let (board, elsewhere, quiet) = (file("app"), file("other"), file("quiet"));
        for board in [&board, &elsewhere, &quiet] {
            fs::create_dir_all(board.parent().unwrap()).unwrap();
            fs::write(board, "{}").unwrap();
        }
        let at = |path: &Path, millis: u64| {
            fs::File::options().write(true).open(path).unwrap().set_modified(UNIX_EPOCH + Duration::from_millis(millis)).unwrap();
        };
        let claude = |pid: u32| Proc { pid, parent: 1, start: 11, comm: "claude".into(), ..Proc::default() };
        let hookless = |pid: u32, parent: u32| Proc {
            ekko_dir: Some(dir.join("app").join(".ekko").to_string_lossy().into_owned()),
            ..server(pid, parent, Path::new("ekko"), &new, on_path)
        };
        // 100 runs a replaced ekko, missed the board's last write and ran a
        // Bash call past its guard; 200 and 300 have no hooks, and their ekko
        // finds the board by its EKKO_DIR; 400 runs a replaced ekko on
        // another board; 500 passes.
        let all = vec![
            claude(100),
            server(10, 100, Path::new("ekko"), &old, on_path),
            claude(200),
            hookless(20, 200),
            claude(300),
            hookless(30, 300),
            claude(400),
            server(40, 400, Path::new("ekko"), &old, on_path),
            claude(500),
            server(50, 500, Path::new("ekko"), &new, on_path),
        ];
        let record = |pid: u32, board: &Path| Running {
            pid,
            start: 11,
            boot: "boot-1".into(),
            profile: None,
            tty: Some(format!("pts/{}", pid / 100)),
            config_dir: None,
            board: Some(board.to_path_buf()),
            transcript: Some(dir.join(format!("c-{pid}.jsonl"))),
            conversation: format!("c-{pid}"),
            earlier: Vec::new(),
            since: 0,
        };
        // All recorded before any is dated: a record forgets the records of
        // ended processes as old as these.
        let sessions = [(100, &board, 1_000_000), (400, &elsewhere, 1_000_000), (500, &board, 3_000_000)];
        for (pid, board, _) in sessions {
            registry.record(record(pid, board)).unwrap();
        }
        for (pid, _, started) in sessions {
            at(&dir.join("processes").join(format!("boot1-{pid}-11.json")), started);
        }
        at(&board, 2_000_000);
        let call = serde_json::json!([{"type": "tool_use", "id": "toolu_1", "name": "Bash", "input": {"command": "ls"}}]);
        fs::write(dir.join("c-100.jsonl"), row("assistant", "1970-01-01T00:33:20Z", false, call) + "\n").unwrap();
        let found = checks(&all, &records(&registry, &told, 2_060_000));
        let failed = |name: &str| -> Vec<u32> {
            found.iter().filter(|check| check.check == name && check.verdict == Verdict::Fail).filter_map(|check| Some(check.session.as_ref()?.pid)).collect()
        };
        assert_eq!((failed(OLD_BINARY), failed(HOOKS_LOADED), failed(WAKE_HEARD), failed(GUARD_RAN)), (vec![100, 400], vec![200, 300], vec![100], vec![100]));
        for check in found.iter().filter(|check| check.verdict == Verdict::Fail) {
            assert!(check.brief.is_some(), "a fail with no words for the prime: {check:?}");
        }

        let old = old.display();
        assert_eq!(
            line(&found, &board, None).as_deref(),
            Some(format!("sessions on this board to restart: default on pts/1 \u{b7} c-100 (it runs {old}, replaced since); Claude Code pid 200 (its hooks did not run); +1 more -- ekko --doctor says why").as_str())
        );
        assert_eq!(
            line(&found, &board, Some(300)).as_deref(),
            Some(format!("sessions on this board to restart: this session (its hooks did not run); default on pts/1 \u{b7} c-100 (it runs {old}, replaced since); +1 more -- ekko --doctor says why").as_str())
        );
        assert_eq!(
            line(&found, &elsewhere, None).as_deref(),
            Some(format!("sessions on this board to restart: default on pts/4 \u{b7} c-400 (it runs {old}, replaced since) -- ekko --doctor says why").as_str())
        );
        assert_eq!(line(&found, &quiet, None), None, "a board whose sessions all pass");
        let unread = checks(&all, &Records { transcripts: false, ..records(&registry, &told, 2_060_000) });
        assert!(unread.iter().all(|check| check.check != GUARD_RAN), "the prime's checks read a transcript");
        fs::remove_dir_all(&dir).ok();
    }

    /// The probe writes the board again as every write does: the same items,
    /// a new version put in place by rename and kept in history, the
    /// revision as it was; with no session on the board it waits for none.
    #[test]
    fn the_probe_writes_the_same_board_as_a_new_version() {
        let dir = crate::paths::test_dir("ekko-doctor-probe");
        let ekko = crate::ekko::Ekko::new(Storage::new(&dir.join(".ekko")).unwrap());
        ekko.create_task(&["probe me".to_string()]).unwrap();
        let storage = &ekko.storage;
        let (items, before, revision) = (fs::read(storage.storage_path()).unwrap(), id(storage.storage_path()), storage.get_counters().unwrap().revision);
        let started = std::time::Instant::now();
        let probe = probe(storage, &dir.join("home")).unwrap();
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "it waited with no session to wait for");
        assert_eq!((probe.sessions, probe.heard), (0, 0));
        let after = id(storage.storage_path());
        assert_ne!(after, before, "no new version: a write in place, which a watch on the file may not follow");
        assert_eq!(probe.written, version(storage.storage_path()).unwrap().1.div_euclid(1_000_000));
        assert_eq!(fs::read(storage.storage_path()).unwrap(), items, "the probe changed the items");
        assert_eq!(storage.get_counters().unwrap().revision, revision, "the probe moved the board's revision");
        let kept = fs::read_dir(dir.join(".ekko").join("history")).unwrap().flatten().any(|entry| id(&entry.path()) == after);
        assert!(kept, "the probe's version is not in history");
        fs::remove_dir_all(&dir).ok();
    }

    /// The probe reads and writes the board under its lock, as every write
    /// does: another's write between its read and its own would be lost.
    #[test]
    fn the_probe_writes_under_the_boards_lock() {
        let dir = crate::paths::test_dir("ekko-doctor-probe-lock");
        let ekko = crate::ekko::Ekko::new(Storage::new(&dir.join(".ekko")).unwrap());
        ekko.create_task(&["probe me".to_string()]).unwrap();
        let storage = &ekko.storage;
        let before = id(storage.storage_path());
        let held = storage.acquire_lock().unwrap();
        std::thread::scope(|scope| {
            let probing = scope.spawn(|| probe(storage, &dir.join("home")).unwrap());
            std::thread::sleep(std::time::Duration::from_millis(300));
            assert_eq!(id(storage.storage_path()), before, "the probe wrote while another held the board's lock");
            drop(held);
            probing.join().unwrap();
        });
        assert_ne!(id(storage.storage_path()), before, "the probe did not write once the lock was free");
        fs::remove_dir_all(&dir).ok();
    }

    /// The probe waits for each session its SessionStart hook recorded on
    /// the board before the write, whose ekko records what its wake hook
    /// hears; not one on another board, one recorded after the write, nor
    /// one on ekko 0.39.1 or before.
    #[test]
    fn the_probe_waits_for_the_sessions_on_its_board_that_record_what_they_hear() {
        use std::time::{Duration, UNIX_EPOCH};
        let dir = crate::paths::test_dir("ekko-doctor-listening");
        let new = binary(&dir, "new");
        let on_path = new.parent().unwrap();
        let registry = Registry::at(dir.join("processes"));
        let told = dir.join("told");
        let file = |project: &str| dir.join(project).join(".ekko").join("storage").join("storage.json");
        let (board, elsewhere) = (file("app"), file("other"));
        for board in [&board, &elsewhere] {
            fs::create_dir_all(board.parent().unwrap()).unwrap();
            fs::write(board, "{}").unwrap();
        }
        let old = Path::new("/nix/store/h-ekko-0.39.1/bin/ekko");
        // 100 heard the probe, 200 not yet; 300 began after it; 400 is on
        // another board; 500 runs an ekko that records nothing.
        let mut all = Vec::new();
        for pid in [100, 200, 300, 400, 500] {
            all.push(Proc { pid, parent: 1, start: 11, comm: "claude".into(), ..Proc::default() });
            let server = server(pid / 10, pid, Path::new("ekko"), &new, on_path);
            all.push(if pid == 500 { Proc { exe: Some(old.to_path_buf()), ..server } } else { server });
        }
        let record = |pid: u32| Running {
            pid,
            start: 11,
            boot: "boot-1".into(),
            profile: None,
            tty: None,
            config_dir: None,
            board: Some(if pid == 400 { elsewhere.clone() } else { board.clone() }),
            transcript: None,
            conversation: format!("c-{pid}"),
            earlier: Vec::new(),
            since: 0,
        };
        for pid in [100, 200, 300, 400, 500] {
            registry.record(record(pid)).unwrap();
        }
        for pid in [100, 200, 300, 400, 500] {
            let started = if pid == 300 { 3_000_000 } else { 1_000_000 };
            fs::File::options()
                .write(true)
                .open(dir.join("processes").join(format!("boot1-{pid}-11.json")))
                .unwrap()
                .set_modified(UNIX_EPOCH + Duration::from_millis(started))
                .unwrap();
        }
        let version = (7, 2_000_000_000_000, 2);
        fs::create_dir_all(told.join("boot1-100-11")).unwrap();
        let heard = Heard { inode: 7, mtime_ns: 2_000_000_000_000, size: 2, revision: 1, at: 2_000_600 };
        fs::write(told.join("boot1-100-11").join("heard"), serde_json::to_string(&heard).unwrap()).unwrap();
        assert_eq!(listening(&all, &records(&registry, &told, 2_001_000), &board, version), (2, 1));
        fs::remove_dir_all(&dir).ok();
    }

    /// The report says what the probe wrote and how many sessions heard it.
    #[test]
    fn the_report_says_how_many_sessions_heard_the_probe() {
        let said = |sessions: usize, heard: usize| {
            let probe = Probe { board: PathBuf::from("/b/storage.json"), written: 0, sessions, heard };
            Report { checks: Vec::new(), probe: Some(probe) }.text().lines().nth(1).unwrap_or_default().to_string()
        };
        let wrote = format!("Probe: wrote /b/storage.json again at {}; ", clock(0));
        assert_eq!(said(2, 1), format!("{wrote}1 of 2 sessions on it heard the write within 5 s"));
        assert_eq!(said(2, 2), format!("{wrote}every session on it heard the write (2 of 2)"));
        assert_eq!(said(0, 0), format!("{wrote}no session on it records what its wake hook hears: nothing to wait for"));
    }

    /// One board's file reached by two paths is one board, as a project's
    /// folder mounted at two paths is.
    #[test]
    fn a_board_is_known_by_its_folder_however_it_is_reached() {
        let dir = crate::paths::test_dir("ekko-doctor-same-board");
        let file = dir.join("a").join("storage").join("storage.json");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "{}").unwrap();
        std::os::unix::fs::symlink(dir.join("a"), dir.join("b")).unwrap();
        assert!(same_board(&file, &dir.join("b").join("storage").join("storage.json")));
        let other = dir.join("c").join("storage").join("storage.json");
        fs::create_dir_all(other.parent().unwrap()).unwrap();
        assert!(!same_board(&file, &other));
        assert!(!same_board(&file, &dir.join("gone").join("storage").join("storage.json")), "a board whose folder is gone");
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
        let (verdict, says) = wake_heard(board, started, None, Some("ekko-0.39.1"), 2_001_000);
        assert_eq!(verdict, Verdict::Skipped);
        assert!(says.starts_with("its ekko-0.39.1 records nothing"), "said too new of an ekko that records nothing: {says}");
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
        assert_eq!(verdict(call("toolu_1", 2_000_000), Some(&guarded("toolu_1", 2_000_100)), None, 2_004_000), Verdict::Ok, "too new, and the guard ran for it");
        assert_eq!(verdict(call("toolu_1", 2_000_000), Some(&guarded("toolu_0", 1_999_000)), None, 2_004_000), Verdict::Skipped, "too new, and the guard ran for another");
        let (_, says) = guard_ran(call("toolu_1", 2_000_000), started, None, Some("ekko-0.39.1"), 2_004_000);
        assert!(says.starts_with("its ekko-0.39.1 records nothing"), "said too new of an ekko that records nothing: {says}");
        assert_eq!(verdict(call("toolu_1", 900_000), None, None, now), Verdict::Skipped, "before the SessionStart");
        assert_eq!(verdict(Ok(None), None, None, now), Verdict::Skipped, "no Bash call");
        assert_eq!(verdict(Err("unreadable".into()), None, None, now), Verdict::Skipped);
        let (_, says) = guard_ran(call("toolu_1", 2_000_000), started, Some(&guarded("toolu_0", 1_990_000)), None, now);
        assert!(says.ends_with("its PreToolUse hook did not run; restart the session"), "{says}");
    }

    /// A server that answers on a port of its own, as `ekko serve` answers
    /// `/status`, with `version`, for `answers` requests.
    fn serving(version: &'static str, answers: usize) -> u16 {
        use std::io::{Read as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming().take(answers) {
                let mut stream = stream.unwrap();
                // The whole request, which the client may write in pieces.
                let (mut request, mut buffer) = (Vec::new(), [0; 1024]);
                while !request.windows(4).any(|end| end == b"\r\n\r\n") {
                    match stream.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(read) => request.extend_from_slice(&buffer[..read]),
                    }
                }
                let body = serde_json::json!({"ekko": version, "pid": 1}).to_string();
                let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            }
        });
        port
    }

    /// The served page passes when its server answers this version and the
    /// files holding its token are the user's alone; it fails on a server of
    /// another version, or a file others can read.
    #[test]
    fn a_server_of_another_version_or_a_token_others_read_fails() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = crate::paths::test_dir("ekko-doctor-serve");
        fs::create_dir_all(&dir).unwrap();
        assert!(served_page(&dir, "0.39.1").is_empty(), "no server ever ran");
        let runtime = |port: u16, mode: u32| {
            let file = dir.join("serve.json");
            fs::write(&file, serde_json::json!({"pid": 1, "port": port, "version": "x", "token": "t"}).to_string()).unwrap();
            fs::set_permissions(&file, fs::Permissions::from_mode(mode)).unwrap();
        };
        let verdicts = |found: Vec<(Verdict, String)>| found.into_iter().map(|(verdict, _)| verdict).collect::<Vec<_>>();

        runtime(serving("0.39.1", 1), 0o600);
        assert_eq!(verdicts(served_page(&dir, "0.39.1")), [Verdict::Ok, Verdict::Ok], "the right setup");
        runtime(serving("0.38.1", 1), 0o600);
        let found = served_page(&dir, "0.39.1");
        assert_eq!(found[1].0, Verdict::Fail, "a server of another version: {found:?}");
        assert!(found[1].1.contains("answers version 0.38.1, and this ekko is 0.39.1"), "{found:?}");
        runtime(serving("0.39.1", 1), 0o644);
        let found = served_page(&dir, "0.39.1");
        assert_eq!(verdicts(found.clone()), [Verdict::Fail, Verdict::Ok], "serve.json others can read: {found:?}");
        assert!(found[0].1.starts_with("serve.json is open to others (mode 644)"), "{found:?}");
        runtime(serving("0.39.1", 1), 0o600);
        fs::write(dir.join("serve-open.html"), "token").unwrap();
        fs::set_permissions(dir.join("serve-open.html"), fs::Permissions::from_mode(0o640)).unwrap();
        assert_eq!(verdicts(served_page(&dir, "0.39.1")), [Verdict::Ok, Verdict::Fail, Verdict::Ok], "serve-open.html others can read");
        fs::remove_file(dir.join("serve-open.html")).unwrap();
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        runtime(closed, 0o600);
        assert_eq!(verdicts(served_page(&dir, "0.39.1")), [Verdict::Ok, Verdict::Skipped], "no server runs");
        fs::remove_dir_all(&dir).ok();
    }

    /// The doctor reads a process as /proc shows it: this test's own, and a
    /// child's environment, by which an ekko finds its board.
    #[test]
    fn a_process_is_read_from_proc() {
        let me = Proc::read(std::process::id()).expect("/proc reads this process");
        assert_eq!(me.running, id(&std::env::current_exe().unwrap()));
        assert_eq!(me.path_var, std::env::var_os("PATH"));
        assert_eq!(me.parent, std::os::unix::process::parent_id());
        assert!(!me.args.is_empty());
        assert_eq!(me.cwd, std::env::current_dir().ok());
        assert_eq!(me.serves(), None);
        let mut child = std::process::Command::new("cat")
            .env("EKKO_DIR", "/work/.ekko")
            .env("EKKO_PROJECT", "site")
            .env("EKKO_DIRECTORY", "not this one")
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        // The parent of a spawn may run again inside the child's exec, before
        // the kernel sets where its arguments and environment lie: /proc
        // shows them empty until then (8 runs in 30 of the suite). It sets
        // the arguments' first, so a read between the two finds arguments
        // and no environment: 215 of 30000 reads polled with no pause did,
        // and none of 30000 that waited for PATH too, which the child
        // inherits (task 1680).
        let read = (0..200)
            .find_map(|_| {
                let read = Proc::read(child.id()).filter(|read| !read.args.is_empty() && read.path_var.is_some());
                if read.is_none() {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                read
            })
            .expect("/proc reads the child");
        drop(child.stdin.take());
        child.wait().unwrap();
        assert_eq!((read.ekko_dir.as_deref(), read.ekko_project.as_deref()), (Some("/work/.ekko"), Some("site")));
    }

    /// Only an ekko or a Claude Code is read past its name, wrapped too: this
    /// test's own binary, named `ekko-<hash>` by cargo, among them.
    #[test]
    fn only_an_ekko_or_a_claude_code_is_read_whole() {
        for comm in ["ekko", ".ekko-wrapped", "claude", ".claude-unwrapp\n"] {
            assert!(checked(comm), "{comm}");
        }
        for comm in ["bash", "node", "my-ekko", "", "sh\n"] {
            assert!(!checked(comm), "{comm}");
        }
        assert!(Proc::all().iter().any(|proc| proc.pid == std::process::id()), "this test's process was not read");
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
