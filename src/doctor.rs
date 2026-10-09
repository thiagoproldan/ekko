//! `ekko --doctor`: checks, from the side that consumes them, that ekko's
//! links with Claude Code work (plan 1279, task 1282). They break without an
//! error anywhere: a session's MCP server keeps the binary it started with
//! after an upgrade (gotcha 194), and a session whose plugin hooks did not
//! run has no prime, no wake and no guard. The checks read /proc and ekko's
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

use crate::holder::{Process, Registry, Running};
use crate::mcp::Binary;

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
#[derive(Debug, Serialize)]
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

/// What the SessionStart hook recorded of the Claude Code process `claude`,
/// in `registry`, if it ran there.
fn record(claude: &Proc, registry: &Registry, boot: &str) -> Option<Running> {
    registry.of(&Process { pid: claude.pid, start: claude.start, boot: boot.to_string() })
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
fn checks(all: &[Proc], registry: &Registry, boot: &str, store: &Path) -> Vec<Check> {
    let mut found = Vec::new();
    let served: Vec<&Proc> = all.iter().filter(|proc| proc.serves().is_some()).collect();
    for proc in &served {
        let (verdict, says) = old_binary(proc, store);
        let what = proc.serves().unwrap_or("ekko");
        let mend = match (verdict, what) {
            (Verdict::Fail, "ekko serve") => ": ekko serve --stop stops it, and the next ekko artifact starts the new one",
            (Verdict::Fail, _) => ": restart the session to load the new one",
            _ => "",
        };
        let (about, session) = match (claude_of(proc, all), what) {
            (Some(claude), _) => about(claude, record(claude, registry, boot).as_ref()),
            // `ekko serve` runs in a session of its own, for every board.
            (None, "ekko serve") => ("this machine".to_string(), None),
            (None, _) => (format!("no Claude Code process, under pid {}", proc.parent), None),
        };
        found.push(Check { check: OLD_BINARY, verdict, about, session, says: format!("{what} (pid {}): {says}{mend}", proc.pid) });
    }
    // The plugin's server, not the resources' registered beside it: a
    // session that runs it loaded the plugin, whose SessionStart hook
    // records the session as it starts.
    let mut sessions: Vec<&Proc> =
        served.iter().filter(|proc| proc.serves() == Some("ekko --mcp")).filter_map(|proc| claude_of(proc, all)).collect();
    sessions.sort_by_key(|claude| claude.pid);
    sessions.dedup_by_key(|claude| claude.pid);
    for claude in sessions {
        let record = record(claude, registry, boot);
        let (verdict, says) = match &record {
            Some(running) => (Verdict::Ok, format!("its SessionStart hook recorded it, on its conversation since {}", crate::holder::when(running.since))),
            None => (
                Verdict::Fail,
                "it runs ekko's MCP server, and no SessionStart hook recorded it: the plugin's hooks did not run there, so it has no prime, no wake and no guard; restart it, and if this stays, check that the plugin is enabled".to_string(),
            ),
        };
        let (about, session) = about(claude, record.as_ref());
        found.push(Check { check: HOOKS_LOADED, verdict, about, session, says });
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
        for name in [OLD_BINARY, HOOKS_LOADED] {
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
    let report = Report { checks: checks(&Proc::all(), &Registry::at(crate::agent::processes_dir(home)), &boot, Path::new(STORE)) };
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
        let claude = |pid: u32| Proc { pid, parent: 1, start: 11, comm: ".claude-unwrapp".into(), ..Proc::default() };
        let mut resources = server(21, 200, Path::new("ekko"), &new, on_path);
        resources.args.push("--resources".into());
        let alone = server(22, 1, Path::new("ekko"), &new, on_path);
        // Two of ekko's servers in one session, as when a config starts one beside the plugin's.
        let second = server(23, 100, Path::new("ekko"), &new, on_path);
        let all = vec![claude(100), server(20, 100, Path::new("ekko"), &new, on_path), claude(200), resources, alone, second];
        let hooks = |all: &[Proc]| -> Vec<(Verdict, String)> {
            checks(all, &registry, "boot-1", Path::new(STORE))
                .into_iter()
                .filter(|check| check.check == HOOKS_LOADED)
                .map(|check| (check.verdict, check.about))
                .collect()
        };

        assert_eq!(hooks(&all), [(Verdict::Fail, "Claude Code pid 100".to_string())]);
        let report = Report { checks: checks(&all, &registry, "boot-1", Path::new(STORE)) };
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
        let report = Report { checks: checks(&all, &registry, "boot-1", Path::new(STORE)) };
        assert!(!report.failed(), "{}", report.text());
        fs::remove_dir_all(&dir).ok();
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
