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
//!
//! The conversation is kept beside the process, in a `Registry` the
//! SessionStart hook writes each time one starts in it: /clear begins a new
//! conversation in the same process, and a resume carries a conversation
//! into a new process. So a claim names the conversation to resume, and a
//! conversation resumed after a restart can tell the claims it made before.

use std::fs;
use std::path::{Path, PathBuf};

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
    /// Where the conversation each process runs is recorded, and read back;
    /// `None` where nothing does, as in most tests.
    pub registry: Option<Registry>,
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
        Actor { process: Process::of(parent), profile: profile(), tty: tty(parent), registry: None }
    }

    /// Whoever runs this command: the Claude Code session it runs under,
    /// when there is one -- a hook, or a Bash command the agent ran -- and
    /// otherwise the person at the terminal.
    pub fn of_this_command() -> Actor {
        if std::env::var_os("CLAUDECODE").is_none() {
            return Actor::person();
        }
        match claude_ancestor() {
            Some(pid) => Actor { process: Process::of(pid), profile: profile(), tty: tty(pid), registry: None },
            None => Actor::person(),
        }
    }

    /// This actor, with the conversations recorded in `registry`.
    pub fn with_registry(mut self, registry: Registry) -> Actor {
        self.registry = Some(registry);
        self
    }

    pub fn is_person(&self) -> bool {
        self.process.is_none()
    }

    /// The conversation this actor's process runs now, as last recorded.
    pub fn conversation(&self) -> Option<String> {
        self.registry.as_ref()?.conversation_of(self.process.as_ref()?)
    }

    /// Records that this actor's process now runs `conversation`, started on
    /// the board whose storage is `board`. Best effort: the hook that
    /// records it answers whether or not it could.
    pub fn record(&self, conversation: &str, board: &Path) {
        let (Some(registry), Some(process)) = (&self.registry, &self.process) else { return };
        let now = Running {
            pid: process.pid,
            start: process.start,
            boot: process.boot.clone(),
            profile: self.profile.clone(),
            tty: self.tty.clone(),
            config_dir: std::env::var("CLAUDE_CONFIG_DIR").ok().filter(|dir| !dir.is_empty()),
            board: Some(board.to_path_buf()),
            conversation: conversation.to_string(),
            earlier: Vec::new(),
            since: chrono::Local::now().timestamp_millis(),
        };
        if let Err(error) = registry.record(now) {
            eprintln!("ekko: the conversation this session runs was not recorded: {error}");
        }
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
            conversation: self.conversation(),
            since,
        }
    }

    /// How this actor names `holder`: with the conversation its process runs
    /// now, when the registry knows it (see `Holder::label_in`).
    pub fn name(&self, holder: &Holder) -> String {
        holder.label_in(self.registry.as_ref())
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
    /// The conversation the process ran when the claim was made. A /clear
    /// since leaves it behind: `conversation_in` reads the newer one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation: Option<String>,
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

    /// The conversation the holding process runs now, or ran last, as
    /// `registry` has it; else the one the claim was made in.
    pub fn conversation_in(&self, registry: Option<&Registry>) -> Option<String> {
        let recorded = registry.zip(self.process()).and_then(|(registry, process)| registry.conversation_of(&process));
        recorded.or_else(|| self.conversation.clone())
    }

    /// How a person reads it: `trabalho on pts/1 · a6b026e6`, `the user`. The
    /// conversation is the one to resume, so the newest `registry` knows of:
    /// after a /clear the claim's own is the conversation before it, and
    /// resuming that one resumes the wrong work.
    pub fn label_in(&self, registry: Option<&Registry>) -> String {
        let Some(pid) = self.pid else { return "the user".to_string() };
        let profile = self.profile.as_deref().unwrap_or("default");
        let mut label = match &self.tty {
            Some(tty) => format!("{profile} on {tty}"),
            None => format!("{profile}, pid {pid}"),
        };
        if let Some(conversation) = self.conversation_in(registry) {
            label.push_str(&format!(" \u{b7} {}", conversation.get(..8).unwrap_or(&conversation)));
        }
        label
    }

    /// `label_in` with no registry: the conversation the claim recorded.
    pub fn label(&self) -> String {
        self.label_in(None)
    }
}

/// What each Claude Code process on this machine runs: a small file per
/// process in the state directory, outside every board, which the
/// SessionStart hook writes each time a conversation starts in it -- at
/// startup, resume, /clear and compaction.
#[derive(Debug, Clone, PartialEq)]
pub struct Registry {
    dir: PathBuf,
}

/// A process as the registry has it: which one, how to name it, and the
/// conversation it runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Running {
    pub pid: u32,
    pub start: u64,
    pub boot: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tty: Option<String>,
    /// Its `CLAUDE_CONFIG_DIR`, which resuming its conversation needs.
    #[serde(rename = "configDir", default, skip_serializing_if = "Option::is_none")]
    pub config_dir: Option<String>,
    /// The board its session started on, by its storage file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board: Option<PathBuf>,
    /// The conversation it runs now, or ran last.
    pub conversation: String,
    /// The ones it ran before, oldest first: each /clear adds one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub earlier: Vec<String>,
    /// When `conversation` began in it, in milliseconds.
    pub since: i64,
}

impl Running {
    pub fn process(&self) -> Process {
        Process { pid: self.pid, start: self.start, boot: self.boot.clone() }
    }

    /// As a claim would name it: see `Holder::label_in`.
    pub fn label(&self) -> String {
        let holder = Holder {
            pid: Some(self.pid),
            start: Some(self.start),
            boot: Some(self.boot.clone()),
            profile: self.profile.clone(),
            tty: self.tty.clone(),
            conversation: Some(self.conversation.clone()),
            since: self.since,
        };
        holder.label()
    }

    /// The command that resumes its conversation, under its profile.
    pub fn resume(&self) -> String {
        let profile = self.config_dir.as_deref().map(|dir| format!("CLAUDE_CONFIG_DIR={dir} ")).unwrap_or_default();
        format!("{profile}claude --resume {}", self.conversation)
    }
}

/// How long the record of a process that ended is kept: long enough to
/// resume its conversation after a restart, or a reboot.
const ENDED_KEPT: std::time::Duration = std::time::Duration::from_secs(30 * 86_400);

impl Registry {
    pub fn at(dir: PathBuf) -> Registry {
        Registry { dir }
    }

    fn file(&self, process: &Process) -> PathBuf {
        let boot: String = process.boot.chars().filter(char::is_ascii_alphanumeric).take(8).collect();
        self.dir.join(format!("{boot}-{}-{}.json", process.pid, process.start))
    }

    /// The record of `process`, if it has one.
    pub fn of(&self, process: &Process) -> Option<Running> {
        serde_json::from_str(&fs::read_to_string(self.file(process)).ok()?).ok()
    }

    /// The conversation `process` runs now, or ran last.
    pub fn conversation_of(&self, process: &Process) -> Option<String> {
        self.of(process).map(|running| running.conversation)
    }

    /// Every process recorded, running or ended.
    pub fn all(&self) -> Vec<Running> {
        let Ok(entries) = fs::read_dir(&self.dir) else { return Vec::new() };
        entries.flatten().filter_map(|entry| serde_json::from_str(&fs::read_to_string(entry.path()).ok()?).ok()).collect()
    }

    /// Records `now`, what a process runs and where, keeping the
    /// conversations it ran before and when the one it runs began; and
    /// forgets on the way the processes that ended more than `ENDED_KEPT` ago.
    pub fn record(&self, mut now: Running) -> std::io::Result<()> {
        let process = now.process();
        if let Some(before) = self.of(&process) {
            now.earlier = before.earlier;
            if before.conversation == now.conversation {
                now.since = before.since;
            } else {
                now.earlier.push(before.conversation);
            }
        }
        fs::create_dir_all(&self.dir)?;
        self.forget_ended();
        fs::write(self.file(&process), serde_json::to_string(&now)?)
    }

    fn forget_ended(&self) {
        let Ok(entries) = fs::read_dir(&self.dir) else { return };
        for entry in entries.flatten() {
            let old = entry
                .metadata()
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|modified| modified.elapsed().ok())
                .is_some_and(|age| age > ENDED_KEPT);
            let running = fs::read_to_string(entry.path())
                .ok()
                .and_then(|text| serde_json::from_str::<Running>(&text).ok())
                .is_some_and(|running| running.process().alive());
            if old && !running {
                let _ = fs::remove_file(entry.path());
            }
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
    let session = |process: Process, tty: &str| Actor { process: Some(process), profile: Some("default".into()), tty: Some(tty.into()), registry: None };
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
            registry: None,
        };
        let holder = actor.holder(1);
        assert_eq!(holder.label(), "trabalho on pts/1");
        assert!(actor.is(&holder));
        assert!(!Actor::person().is(&holder));
        assert_eq!(Actor::person().holder(1).label(), "the user");
        assert!(Actor::person().holder(1).alive(), "a person's claim lasts until the task leaves progress");
        assert!(!holder.alive(), "no process 42 started at tick 7 in boot b");
    }

    /// A claim names the conversation to resume: the one its process runs
    /// now, which a /clear moves on from the one the claim was made in.
    /// Seen on 2026-09-22, when a session named by its conversation before a
    /// /clear was resumed instead of the one after it.
    #[test]
    fn a_claim_names_the_conversation_its_process_runs() {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("ekko-registry-{}-{nanos}", std::process::id()));
        let registry = Registry::at(dir.clone());
        let me = Process::of(std::process::id()).expect("/proc reads this process");
        let actor = Actor {
            process: Some(me.clone()),
            profile: Some("trabalho".into()),
            tty: Some("pts/5".into()),
            registry: Some(registry.clone()),
        };
        assert_eq!(actor.holder(1).conversation, None, "nothing recorded yet");

        actor.record("a6b026e6-85d5-4e5c-ae3b-e5a4a676878a", Path::new("/board"));
        let claim = actor.holder(1);
        assert_eq!(claim.conversation.as_deref(), Some("a6b026e6-85d5-4e5c-ae3b-e5a4a676878a"));
        assert_eq!(claim.label(), "trabalho on pts/5 \u{b7} a6b026e6");

        actor.record("e3011485-0b75-46ea-b411-2cdf3193e1d1", Path::new("/board"));
        assert_eq!(actor.name(&claim), "trabalho on pts/5 \u{b7} e3011485", "after a /clear, the one to resume");
        assert_eq!(claim.label(), "trabalho on pts/5 \u{b7} a6b026e6", "the claim keeps the one it was made in");
        let running = registry.of(&me).unwrap();
        assert_eq!(running.earlier, ["a6b026e6-85d5-4e5c-ae3b-e5a4a676878a"]);
        assert_eq!((running.profile.as_deref(), running.tty.as_deref()), (Some("trabalho"), Some("pts/5")));

        std::fs::remove_dir_all(&dir).ok();
    }
}
