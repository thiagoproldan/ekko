//! Who holds a task in progress: the Claude Code process working it, or the
//! person at the terminal.
//!
//! A process and not a session id. The MCP server's `CLAUDE_CODE_SESSION_ID`
//! is fixed when Claude Code starts it, so after a resume or a /clear it no
//! longer names the conversation, while the process it runs under stays the
//! same one terminal, from its first message to its last /clear. That process
//! is the server's parent, and an ancestor of every hook and Bash command the
//! session runs, so each of them can tell a claim is its own.
//!
//! Whether a holder is still there is read off /proc, with no clock and no
//! lease to expire: a process is the one that claimed the task while its pid
//! is running with the start time and the boot it had then, which a pid used
//! again by another process never matches.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// A process, told apart from every other that ever had its pid: the pid,
/// when it started, in clock ticks after boot, and the boot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Process {
    pub pid: u32,
    pub start: u64,
    pub boot: String,
}

impl Process {
    /// The process running as `pid` now.
    pub fn of(pid: u32) -> Option<Process> {
        let (_, start) = stat(pid)?;
        Some(Process { pid, start, boot: boot()? })
    }

    /// Whether it is still running: the same pid, started at the same tick,
    /// in the same boot.
    pub fn alive(&self) -> bool {
        boot().is_some_and(|boot| boot == self.boot) && stat(self.pid).is_some_and(|(_, start)| start == self.start)
    }
}

/// Who acts through this process: a Claude Code process, or a person.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Actor {
    /// The Claude Code process; `None` for a person at the terminal.
    pub process: Option<Process>,
    /// The Claude Code profile, from `CLAUDE_CONFIG_DIR`: `trabalho` for
    /// `~/.claude-trabalho`, `default` for `~/.claude`.
    pub profile: Option<String>,
    /// The terminal the process runs in, such as `pts/1`.
    pub tty: Option<String>,
}

impl Actor {
    /// A person at the terminal.
    pub fn person() -> Actor {
        Actor::default()
    }

    /// The client of this MCP server: its parent, which is the Claude Code
    /// process that started it.
    pub fn client_of_this_server() -> Actor {
        let parent = std::os::unix::process::parent_id();
        Actor { process: Process::of(parent), profile: profile(), tty: tty(parent) }
    }

    /// Whoever runs this command: the Claude Code session it runs under,
    /// when there is one -- a hook, or a Bash command the agent ran -- and
    /// otherwise the person at the terminal.
    pub fn of_this_command() -> Actor {
        if std::env::var_os("CLAUDECODE").is_none() {
            return Actor::person();
        }
        match claude_ancestor() {
            Some(pid) => Actor { process: Process::of(pid), profile: profile(), tty: tty(pid) },
            None => Actor::person(),
        }
    }

    pub fn is_person(&self) -> bool {
        self.process.is_none()
    }

    /// The claim this actor makes on a task, from `since`.
    pub fn holder(&self, since: i64) -> Holder {
        let process = self.process.as_ref();
        Holder {
            pid: process.map(|p| p.pid),
            start: process.map(|p| p.start),
            boot: process.map(|p| p.boot.clone()),
            profile: self.profile.clone(),
            tty: self.tty.clone(),
            since,
        }
    }

    /// Whether `holder` is this actor: the same process, or both a person.
    pub fn is(&self, holder: &Holder) -> bool {
        self.process == holder.process()
    }
}

/// Who holds a task in progress, as stored on it (`heldBy`): the process and
/// how to name it, or, with no process, a person.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Holder {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tty: Option<String>,
    /// When the claim was made, in milliseconds.
    pub since: i64,
}

impl Holder {
    pub fn process(&self) -> Option<Process> {
        Some(Process { pid: self.pid?, start: self.start?, boot: self.boot.clone()? })
    }

    /// Whether the claim still stands: a person's lasts until the task
    /// leaves progress, a process's while it runs.
    pub fn alive(&self) -> bool {
        self.process().is_none_or(|process| process.alive())
    }

    /// How a person reads it: `trabalho on pts/1`, `the user`.
    pub fn label(&self) -> String {
        let Some(pid) = self.pid else { return "the user".to_string() };
        let profile = self.profile.as_deref().unwrap_or("default");
        match &self.tty {
            Some(tty) => format!("{profile} on {tty}"),
            None => format!("{profile}, pid {pid}"),
        }
    }
}

/// A claim's time as a person reads it: the hour today, the day and hour
/// before that.
pub fn when(millis: i64) -> String {
    use chrono::TimeZone as _;
    let Some(at) = chrono::Local.timestamp_millis_opt(millis).single() else { return millis.to_string() };
    if at.date_naive() == chrono::Local::now().date_naive() {
        at.format("%H:%M").to_string()
    } else {
        at.format("%Y-%m-%d %H:%M").to_string()
    }
}

/// `pid`'s parent and its start, from /proc/<pid>/stat. The name in field 2
/// can hold spaces and parentheses, so the fields are counted from its end:
/// the parent is field 4 and the start field 22.
fn stat(pid: u32) -> Option<(u32, u64)> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let fields: Vec<&str> = stat.get(stat.rfind(')')? + 1..)?.split_whitespace().collect();
    Some((fields.get(1)?.parse().ok()?, fields.get(19)?.parse().ok()?))
}

fn boot() -> Option<String> {
    Some(fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?.trim().to_string())
}

/// The profile Claude Code runs with, named after its config directory.
fn profile() -> Option<String> {
    let dir = std::env::var_os("CLAUDE_CONFIG_DIR");
    let name = dir.as_deref().and_then(|dir| Path::new(dir).file_name()).map(|name| name.to_string_lossy().into_owned());
    Some(match name.as_deref().map(|name| name.trim_start_matches('.')) {
        None | Some("claude") => "default".to_string(),
        Some(name) => name.strip_prefix("claude-").unwrap_or(name).to_string(),
    })
}

/// The terminal `pid` reads from, as `pts/1` or `tty2`.
fn tty(pid: u32) -> Option<String> {
    let path = fs::read_link(format!("/proc/{pid}/fd/0")).ok()?;
    let name = path.to_str()?.strip_prefix("/dev/")?;
    (name.starts_with("pts/") || name.starts_with("tty")).then(|| name.to_string())
}

/// The nearest ancestor that is Claude Code, by its process name: `claude`,
/// or `.claude-unwrapped` behind a nix wrapper.
fn claude_ancestor() -> Option<u32> {
    let mut pid = std::os::unix::process::parent_id();
    for _ in 0..16 {
        let name = fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
        if name.trim().trim_start_matches('.').starts_with("claude") {
            return Some(pid);
        }
        pid = stat(pid)?.0;
        if pid <= 1 {
            return None;
        }
    }
    None
}

/// Claude Code sessions for tests: this test process as one, on pts/1; its
/// parent, alive while the test runs, as another, on pts/2; and one that is
/// gone, on pts/3.
#[cfg(test)]
pub(crate) fn test_sessions() -> (Actor, Actor, Actor) {
    let session = |process: Process, tty: &str| Actor { process: Some(process), profile: Some("default".into()), tty: Some(tty.into()) };
    let me = Process::of(std::process::id()).expect("/proc reads this process");
    let other = Process::of(std::os::unix::process::parent_id()).expect("/proc reads the parent");
    let gone = Process { start: me.start + 1, ..me.clone() };
    (session(me, "pts/1"), session(other, "pts/2"), session(gone, "pts/3"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_process_is_alive_and_a_changed_start_is_not() {
        let me = Process::of(std::process::id()).expect("/proc reads this process");
        assert!(me.alive());
        assert!(!Process { start: me.start + 1, ..me.clone() }.alive(), "another process that got the pid");
        assert!(!Process { boot: "another boot".into(), ..me.clone() }.alive());
    }

    #[test]
    fn a_holder_is_named_by_profile_and_terminal_or_as_the_user() {
        let actor = Actor {
            process: Some(Process { pid: 42, start: 7, boot: "b".into() }),
            profile: Some("trabalho".into()),
            tty: Some("pts/1".into()),
        };
        let holder = actor.holder(1);
        assert_eq!(holder.label(), "trabalho on pts/1");
        assert!(actor.is(&holder));
        assert!(!Actor::person().is(&holder));
        assert_eq!(Actor::person().holder(1).label(), "the user");
        assert!(Actor::person().holder(1).alive(), "a person's claim lasts until the task leaves progress");
        assert!(!holder.alive(), "no process 42 started at tick 7 in boot b");
    }
}
