//! Failures that repeat across sessions, found without a model (task 1442).
//!
//! Claude Code writes each session's transcript under its config folder's
//! `projects/`, in a folder named after the one the session started in. A
//! failed tool call is a `tool_result` marked `is_error` there: a command's
//! exit, a hook's refusal -- ekko's cues, ctx's blocks -- or ekko's own.
//! Each failure gets a fingerprint, as Sentry groups errors: the tool, the
//! command's first word for Bash, and the message with what varies between
//! two runs taken out. A fingerprint seen in 2 or more sessions on 2 or more
//! days recurs, ai-memory's two-session rule. The session at work, or the
//! user, writes the note from that evidence: nothing here writes the board.
//!
//! What was read is kept outside the board, in ekko's state directory, as
//! the hooks' records are: each transcript's offset, so a read takes only
//! the bytes written since the last, and the failures found, each once by
//! its tool_use_id -- a session begun from another's history repeats its
//! rows, and a copied failure would count as a second session.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::io::{BufRead as _, BufReader, Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ekko::EkkoError;

/// The fingerprint's rules, by number: what was kept under other rules is
/// read again from the start, since its fingerprints no longer compare.
const RULES: u32 = 1;
/// A fingerprint recurs once seen in this many sessions, on this many days.
const SESSIONS: usize = 2;
const DAYS: usize = 2;
/// What a fingerprint keeps of its message, as the prototype did.
const MESSAGE_CHARS: usize = 140;
/// What a failure keeps of the lines it was fingerprinted from, to show.
const EXAMPLE_CHARS: usize = 240;
/// The tool calls a read keeps from a transcript's end: a call written just
/// before a read has its result after it.
const PENDING_USES: usize = 16;
/// How far into a new transcript to look for the folder it began in.
const HEAD_BYTES: u64 = 4 << 20;
/// How long the SessionStart hook spends reading; the rest waits for the
/// next session's start, or for `ekko --repeats`.
const HOOK_BUDGET: Duration = Duration::from_millis(400);
/// What the prime's line names before it counts the rest.
const NAMED: usize = 2;
/// Calls that only set the shell up: the command is the next one.
const SETUP: &[&str] = &["=", "cd", "pushd", "popd", "export", "set", "source", ".", "for"];

// --- the fingerprint -----------------------------------------------------------

/// A failure as its fingerprint sees it: the tool, with the command's first
/// word for Bash, and the lines of its message it is known by, as written.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Shape {
    pub head: String,
    pub message: String,
}

impl Shape {
    /// Task 1442's step 3: the message's first two lines -- but for Bash,
    /// whose result opens with `Exit code N` and then the command's output,
    /// that line and the output's last. What fails last prints last, while
    /// the first line is often what the commands before it printed. Measured
    /// 2026-10-08 on the month's 934 interactive failures (/tmp/1442-ab):
    /// 46 recurring fingerprints either way, holding 310 failures against
    /// 282; "python3: command not found" grew from 28 sessions to 36, and
    /// groups named by an unrelated first line ("total 0", a passing test's
    /// summary) were gone.
    pub(crate) fn of(tool: &str, word: Option<&str>, text: &str) -> Shape {
        let lines: Vec<&str> = text.lines().filter(|line| !line.trim().is_empty()).collect();
        let bash = tool == "Bash";
        let message = match lines.as_slice() {
            [exit, .., last] if bash && is_exit(exit) => format!("{exit} | {last}"),
            _ => lines.iter().take(2).copied().collect::<Vec<_>>().join(" | "),
        };
        let word = word.filter(|_| bash && !names_its_command(&message));
        let head = match word {
            Some(word) => format!("{tool} {word}"),
            None => tool.to_string(),
        };
        Shape { head, message }
    }

    /// What the failure is known by: its head, and its message normalized.
    pub(crate) fn key(&self) -> String {
        format!("{}\n{}", self.head, normalized(&self.message))
    }
}

/// Bash's first line for a command that exits non-zero, as Claude Code
/// writes it.
fn is_exit(line: &str) -> bool {
    line.trim().strip_prefix("Exit code ").is_some_and(|code| !code.is_empty() && code.bytes().all(|b| b.is_ascii_digit()))
}

/// A message that names the command that failed, as bash's `X: command not
/// found` and env's `'X': No such file or directory` do: the command's first
/// word would only split one failure by how it was reached. Measured
/// 2026-10-07: "python3: command not found" fell into three fingerprints by
/// that word, in 8, 6 and 5 sessions.
fn names_its_command(message: &str) -> bool {
    let lower = message.to_lowercase();
    lower.contains("command not found") || lower.contains("no such file or directory")
}

/// The first word of what a Bash call ran: its first call, as ekko's shell
/// lexer reads the command, past the calls that only set the shell up, the
/// wrappers that run their arguments, a `bash -c` whose string holds the
/// calls, and `nix develop -c`. The prototype took the first word of the
/// text, which made `cd /projects/x && cargo test` a failure of `projects`.
pub(crate) fn command_word(command: &str) -> Option<String> {
    let calls = crate::shell::calls(command);
    let shells: HashSet<usize> = calls.iter().filter_map(|call| call.parent).collect();
    let (_, call) = calls.iter().enumerate().find(|(at, call)| {
        !(call.name.is_empty()
            || SETUP.contains(&call.name.as_str())
            || crate::shell::is_wrapper(&call.name)
            || shells.contains(at))
    })?;
    if call.name == "nix" && call.args.first().is_some_and(|arg| arg == "develop") {
        if let Some(word) = call.args.iter().skip_while(|arg| *arg != "-c" && *arg != "--command").nth(1) {
            return Some(word.rsplit('/').next().unwrap_or(word).to_string());
        }
    }
    Some(call.name.clone())
}

/// A message with what varies between two runs of one failure taken out, as
/// the prototype's `norm` did: lower case; quoted text, then paths, then
/// hashes, then numbers replaced; blanks collapsed; the first 140 characters.
pub(crate) fn normalized(text: &str) -> String {
    let text = numberless(&hashless(&pathless(&unquoted(&text.to_lowercase()))));
    text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(MESSAGE_CHARS).collect()
}

/// Text between double quotes, single quotes or backticks on one line, as `S`.
fn unquoted(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while at < chars.len() {
        let quote = chars[at];
        if matches!(quote, '"' | '\'' | '`') {
            let close = chars[at + 1..].iter().position(|&c| c == quote || c == '\n');
            if let Some(close) = close.filter(|&close| chars[at + 1 + close] == quote) {
                out.push('S');
                at += close + 2;
                continue;
            }
        }
        out.push(quote);
        at += 1;
    }
    out
}

/// Paths -- from `/`, `~/`, `./` or `../` to a blank, a colon, a comma or a
/// closing parenthesis -- as `/P`. A relative path keeps its first part:
/// `src/main.rs` is `src/P`.
fn pathless(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while at < chars.len() {
        let next = |n: usize| chars.get(at + n).copied();
        let slash = match chars[at] {
            '/' => Some(at),
            '~' if next(1) == Some('/') => Some(at + 1),
            '.' if next(1) == Some('.') && next(2) == Some('/') => Some(at + 2),
            '.' if next(1) == Some('/') => Some(at + 1),
            _ => None,
        };
        if let Some(slash) = slash {
            let end = chars[slash + 1..]
                .iter()
                .position(|&c| c.is_whitespace() || matches!(c, ':' | ',' | ')'))
                .map_or(chars.len(), |len| slash + 1 + len);
            out.push_str("/P");
            at = end;
            continue;
        }
        out.push(chars[at]);
        at += 1;
    }
    out
}

/// A word of 7 or more hexadecimal digits, a commit's or a uid's, as `H`.
fn hashless(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        let hash = word.chars().count() >= 7 && word.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c));
        out.push_str(if hash { "H" } else { word });
        word.clear();
    };
    for c in text.chars() {
        if c.is_alphanumeric() || c == '_' {
            word.push(c);
        } else {
            flush(&mut word, &mut out);
            out.push(c);
        }
    }
    flush(&mut word, &mut out);
    out
}

/// Each run of digits as `0`.
fn numberless(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut digits = false;
    for c in text.chars() {
        if c.is_ascii_digit() {
            if !digits {
                out.push('0');
            }
            digits = true;
        } else {
            out.push(c);
            digits = false;
        }
    }
    out
}

/// A fingerprint's name, short enough to type: six characters of its FNV-1a
/// hash, in the alphabet of the guard's codes.
pub(crate) fn id(key: &str) -> String {
    const ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";
    let hash = crate::artifact::fnv(key.as_bytes());
    (0..6).map(|i| ALPHABET[((hash >> (i * 5)) & 31) as usize] as char).collect()
}

// --- reading the transcripts -----------------------------------------------------

/// What was read of a project's transcripts, kept between reads. Not a
/// cache: Claude Code does not keep transcripts for ever -- on 2026-10-09
/// none on this machine was older than 30 days -- so the failures kept here
/// may be all that is left of them, and the dismissals are someone's word.
#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    rules: u32,
    root: PathBuf,
    #[serde(default)]
    transcripts: BTreeMap<PathBuf, Seen>,
    #[serde(default)]
    failures: Vec<Failure>,
    /// Fingerprints set aside, by id, with when: each comes back only if it
    /// recurs again after.
    #[serde(default)]
    dismissed: BTreeMap<String, String>,
    /// The recurring fingerprints a prime or a listing already showed.
    #[serde(default)]
    shown: BTreeSet<String>,
    /// What a later version keeps here that this one does not know, written
    /// back as read; see `Item::unknown`.
    #[serde(flatten)]
    unknown: BTreeMap<String, serde_json::Value>,
}

/// One transcript as read so far.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct Seen {
    /// Bytes read, to the end of the last whole line.
    offset: u64,
    /// Whose it is, by a `Whose`'s word, once a line has said where it began.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    whose: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pending: Vec<Use>,
    #[serde(flatten)]
    unknown: BTreeMap<String, serde_json::Value>,
}

/// Whose a transcript is. Kept by its word, so a word only a later version
/// knows is taken for none, and found again.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Whose {
    /// Begun in the project's folder, by a person.
    Ours,
    /// Begun in the project's folder, headless through the SDK: left out, as
    /// study 1462 counted (note 1468).
    Headless,
    /// Begun in another folder whose name reads the same.
    Elsewhere,
}

impl Whose {
    const ALL: [Whose; 3] = [Whose::Ours, Whose::Headless, Whose::Elsewhere];

    fn word(self) -> &'static str {
        match self {
            Whose::Ours => "ours",
            Whose::Headless => "headless",
            Whose::Elsewhere => "elsewhere",
        }
    }

    fn of(seen: &Seen) -> Option<Whose> {
        Whose::ALL.into_iter().find(|whose| seen.whose.as_deref() == Some(whose.word()))
    }
}

/// A tool call, as its failure will need it.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Use {
    id: String,
    tool: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    word: Option<String>,
    #[serde(flatten)]
    unknown: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Failure {
    /// The call's tool_use_id: a failure counts once, however many
    /// transcripts hold it.
    id: String,
    session: String,
    /// When, as the transcript says: RFC 3339 in UTC, which sorts as text.
    at: String,
    key: String,
    /// The lines it was fingerprinted from, as written, cut short.
    example: String,
    #[serde(flatten)]
    unknown: BTreeMap<String, serde_json::Value>,
}

impl Failure {
    /// Its day, in UTC.
    fn day(&self) -> &str {
        self.at.get(..10).unwrap_or(&self.at)
    }
}

/// What a read left out, and why.
#[derive(Debug, Default, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Left {
    /// The project's transcripts of headless runs.
    pub headless: usize,
    /// Lines that could not be read, or that mark a result in a way this
    /// reader does not know: their failures would go unseen.
    pub unreadable: usize,
    /// Transcripts the read did not reach before its time ran out.
    pub unread: usize,
}

impl State {
    fn load(file: &Path, root: &Path) -> State {
        let kept: Option<State> = fs::read(file).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok());
        match kept {
            Some(state) if state.rules == RULES && state.root == root => state,
            _ => State { rules: RULES, root: root.to_path_buf(), ..State::default() },
        }
    }

    /// Reads what is new in each of `transcripts` until `deadline`, and
    /// forgets the ones no longer on disk; their failures stay.
    fn read(&mut self, transcripts: &[PathBuf], deadline: Option<Instant>) -> Left {
        let mut known: HashSet<String> = self.failures.iter().map(|failure| failure.id.clone()).collect();
        let mut left = Left::default();
        for path in transcripts {
            let seen = self.transcripts.entry(path.clone()).or_default();
            let late = deadline.is_some_and(|deadline| Instant::now() >= deadline);
            if Whose::of(seen).is_none() && !late {
                seen.whose = whose(path, &self.root).map(|whose| whose.word().to_string());
            }
            match Whose::of(seen) {
                Some(Whose::Ours) if late => {
                    left.unread += 1;
                    continue;
                }
                Some(Whose::Ours) => {}
                Some(Whose::Headless) => {
                    left.headless += 1;
                    continue;
                }
                Some(Whose::Elsewhere) => continue,
                None => {
                    left.unread += usize::from(late);
                    continue;
                }
            }
            match read_new(path, seen, &mut known) {
                Ok((found, unreadable)) => {
                    self.failures.extend(found);
                    left.unreadable += unreadable;
                }
                Err(_) => left.unreadable += 1,
            }
        }
        let present: HashSet<&PathBuf> = transcripts.iter().collect();
        self.transcripts.retain(|path, _| present.contains(path));
        left
    }
}

/// Whose a transcript is, from the first of its lines that names the folder
/// it began in; `None` while none does yet.
fn whose(path: &Path, root: &Path) -> Option<Whose> {
    let mut reader = BufReader::new(fs::File::open(path).ok()?.take(HEAD_BYTES));
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line).ok()? == 0 || line.last() != Some(&b'\n') {
            return None;
        }
        let Ok(line) = std::str::from_utf8(&line) else { continue };
        if !line.contains("\"cwd\"") {
            continue;
        }
        let Ok(row) = serde_json::from_str::<Value>(line) else { continue };
        let Some(cwd) = row["cwd"].as_str() else { continue };
        if !Path::new(cwd).starts_with(root) {
            return Some(Whose::Elsewhere);
        }
        let headless = row["entrypoint"].as_str().is_some_and(|entrypoint| entrypoint.starts_with("sdk"));
        return Some(if headless { Whose::Headless } else { Whose::Ours });
    }
}

/// The failures in what `path` gained since `seen`'s offset, each new to
/// `known`, and how many lines could not be read. A transcript shorter than
/// its offset was written anew, and is read again from its start.
fn read_new(path: &Path, seen: &mut Seen, known: &mut HashSet<String>) -> std::io::Result<(Vec<Failure>, usize)> {
    let mut file = fs::File::open(path)?;
    let length = file.metadata()?.len();
    if length < seen.offset {
        seen.offset = 0;
        seen.pending.clear();
    }
    if length == seen.offset {
        return Ok((Vec::new(), 0));
    }
    file.seek(SeekFrom::Start(seen.offset))?;
    let mut reader = BufReader::new(file);
    let named = path.file_stem().and_then(|stem| stem.to_str()).unwrap_or_default().to_string();
    let mut uses: HashMap<String, Use> = seen.pending.iter().map(|used| (used.id.clone(), used.clone())).collect();
    let mut order = Vec::new();
    let mut found = Vec::new();
    let mut unreadable = 0;
    let mut offset = seen.offset;
    let mut bytes = Vec::new();
    loop {
        bytes.clear();
        let read = reader.read_until(b'\n', &mut bytes)?;
        // The end, or a line still being written: read whole, next time.
        if read == 0 || bytes.last() != Some(&b'\n') {
            break;
        }
        offset += read as u64;
        let Ok(line) = std::str::from_utf8(&bytes) else {
            unreadable += 1;
            continue;
        };
        let calls = line.contains("\"type\":\"tool_use\"");
        let failed = line.contains("\"is_error\":true");
        if !calls && !failed {
            // A result marked another way than these two: the format moved,
            // and its failures would go unseen, so it is counted.
            if line.contains("\"is_error\"") && !line.contains("\"is_error\":false") {
                unreadable += 1;
            }
            continue;
        }
        let Ok(row) = serde_json::from_str::<Value>(line) else {
            unreadable += 1;
            continue;
        };
        // A subagent's, written into its session's transcript by older
        // versions: the main thread only, as the study counted.
        if row["isSidechain"].as_bool() == Some(true) {
            continue;
        }
        let Some(blocks) = row["message"]["content"].as_array() else { continue };
        for block in blocks {
            match block["type"].as_str() {
                Some("tool_use") => {
                    let (Some(id), Some(tool)) = (block["id"].as_str(), block["name"].as_str()) else { continue };
                    let word = if tool == "Bash" { block["input"]["command"].as_str().and_then(command_word) } else { None };
                    uses.insert(id.to_string(), Use { id: id.to_string(), tool: tool.to_string(), word, unknown: BTreeMap::new() });
                    order.push(id.to_string());
                }
                Some("tool_result") if block["is_error"].as_bool() == Some(true) => {
                    let Some(id) = block["tool_use_id"].as_str() else { continue };
                    if !known.insert(id.to_string()) {
                        continue;
                    }
                    let used = uses.get(id);
                    let tool = used.map_or("?", |used| used.tool.as_str());
                    let shape = Shape::of(tool, used.and_then(|used| used.word.as_deref()), &text_of(&block["content"]));
                    found.push(Failure {
                        id: id.to_string(),
                        session: row["sessionId"].as_str().unwrap_or(&named).to_string(),
                        at: row["timestamp"].as_str().unwrap_or_default().to_string(),
                        key: shape.key(),
                        example: cut(&format!("{}: {}", shape.head, shape.message), EXAMPLE_CHARS),
                        unknown: BTreeMap::new(),
                    });
                }
                _ => {}
            }
        }
    }
    seen.offset = offset;
    let mut pending: Vec<Use> = order.iter().rev().filter_map(|id| uses.remove(id)).take(PENDING_USES).collect();
    pending.reverse();
    seen.pending = pending;
    Ok((found, unreadable))
}

/// A result's text: a string, or the text of its blocks.
fn text_of(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks.iter().filter_map(|block| block["text"].as_str()).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

fn cut(text: &str, chars: usize) -> String {
    if text.chars().count() <= chars {
        return text.to_string();
    }
    let kept: String = text.chars().take(chars.saturating_sub(1)).collect();
    format!("{}\u{2026}", kept.trim_end())
}

// --- where things are ------------------------------------------------------------

/// Where a project's reads happen: the Claude Code config folders whose
/// transcripts are read, and the file that keeps what was read.
struct Place {
    dirs: Vec<PathBuf>,
    file: PathBuf,
}

impl Place {
    fn of(home: &Path, root: &Path) -> Place {
        Place {
            dirs: config_dirs(home, std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from)),
            file: crate::agent::state_dir(home).join("repeats").join(format!("{}.json", folder_name(root))),
        }
    }
}

/// The folders Claude Code keeps sessions under: the one `CLAUDE_CONFIG_DIR`
/// names, `~/.claude`, and each `~/.claude-<profile>` -- the profiles the
/// holder names sessions by.
fn config_dirs(home: &Path, named: Option<PathBuf>) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = named.filter(|dir| !dir.as_os_str().is_empty()).into_iter().collect();
    dirs.push(home.join(".claude"));
    if let Ok(entries) = fs::read_dir(home) {
        let mut profiles: Vec<PathBuf> = entries
            .flatten()
            .filter(|entry| entry.file_name().to_str().is_some_and(|name| name.starts_with(".claude-")))
            .map(|entry| entry.path())
            .collect();
        profiles.sort();
        dirs.extend(profiles);
    }
    let mut seen = HashSet::new();
    dirs.retain(|dir| dir.join("projects").is_dir() && seen.insert(fs::canonicalize(dir).unwrap_or_else(|_| dir.clone())));
    dirs
}

/// The folder name Claude Code gives the sessions started in `path`: each
/// character but an ASCII letter or digit turned to `-`. Measured 2026-10-08:
/// 50 of the 51 folders under ~/.claude/projects are named so after their
/// first transcript's cwd; the other is a worktree's, entered by a session
/// that started in the project.
fn folder_name(path: &Path) -> String {
    path.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

/// The project's transcripts: the main thread's, in each config folder's
/// folder named after the project's or after one inside it -- a worktree's
/// under `.claude/worktrees`, a subfolder's.
fn transcripts(config_dirs: &[PathBuf], root: &Path) -> Vec<PathBuf> {
    let name = folder_name(root);
    let mut found = Vec::new();
    for dir in config_dirs {
        let Ok(folders) = fs::read_dir(dir.join("projects")) else { continue };
        for folder in folders.flatten() {
            let Some(folder_name) = folder.file_name().to_str().map(str::to_string) else { continue };
            if folder_name != name && !folder_name.strip_prefix(name.as_str()).is_some_and(|rest| rest.starts_with('-')) {
                continue;
            }
            let Ok(files) = fs::read_dir(folder.path()) else { continue };
            found.extend(files.flatten().map(|file| file.path()).filter(|path| path.extension().is_some_and(|ext| ext == "jsonl")));
        }
    }
    found.sort();
    found
}

/// Runs `act` on the project's state with what is new read in, under a lock
/// beside it, and keeps what it leaves.
fn with_state<T>(place: &Place, root: &Path, deadline: Option<Instant>, act: impl FnOnce(&mut State, &Left) -> T) -> Result<T, EkkoError> {
    let file = &place.file;
    let dir = file.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(dir).map_err(|error| EkkoError::InvalidInput(format!("{}: {error}", dir.display())))?;
    let _lock = crate::storage::lock_path(&file.with_extension("lock")).map_err(EkkoError::from)?;
    let mut state = State::load(file, root);
    let left = state.read(&transcripts(&place.dirs, root), deadline);
    let answer = act(&mut state, &left);
    let bytes = serde_json::to_vec(&state).map_err(|error| EkkoError::InvalidInput(error.to_string()))?;
    crate::guard::write_atomically(file, &bytes).map_err(|error| EkkoError::InvalidInput(format!("{}: {error}", file.display())))?;
    Ok(answer)
}

// --- what recurs ---------------------------------------------------------------

/// What `ekko --repeats` shows.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub board: String,
    /// The project's transcripts read, headless ones left out.
    pub transcripts: usize,
    pub failures: usize,
    pub fingerprints: usize,
    pub recurring: Vec<Repeat>,
    /// Recurring fingerprints set aside that have not recurred since.
    pub dismissed: usize,
    /// What this call set aside.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub set_aside: Vec<String>,
    pub left: Left,
}

/// A recurring fingerprint.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Repeat {
    pub id: String,
    /// Its latest failure, as written.
    pub example: String,
    pub sessions: usize,
    pub days: usize,
    pub times: usize,
    pub first: String,
    pub last: String,
    /// When it was set aside, for one that recurred again since: its counts
    /// are since then.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dismissed: Option<String>,
    /// Not shown as recurring before.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub new: bool,
}

/// Whether `failures` recur: in enough sessions, on enough days.
fn recurs(failures: &[&Failure]) -> bool {
    let sessions: HashSet<&str> = failures.iter().map(|failure| failure.session.as_str()).collect();
    let days: HashSet<&str> = failures.iter().map(|failure| failure.day()).collect();
    sessions.len() >= SESSIONS && days.len() >= DAYS
}

impl State {
    /// The fingerprints by key, with their failures in the order read.
    fn groups(&self) -> BTreeMap<&str, Vec<&Failure>> {
        let mut groups: BTreeMap<&str, Vec<&Failure>> = BTreeMap::new();
        for failure in &self.failures {
            groups.entry(failure.key.as_str()).or_default().push(failure);
        }
        groups
    }

    /// The recurring fingerprints, most sessions first, and how many set
    /// aside stay quiet. One set aside counts only its failures since.
    fn recurring(&self) -> (Vec<Repeat>, usize) {
        let mut found = Vec::new();
        let mut quiet = 0;
        for (key, group) in self.groups() {
            let id = id(key);
            let since = self.dismissed.get(&id);
            let counted: Vec<&Failure> =
                group.iter().copied().filter(|failure| since.is_none_or(|since| failure.at.as_str() > since.as_str())).collect();
            if !recurs(&counted) {
                quiet += usize::from(since.is_some() && recurs(&group));
                continue;
            }
            let latest = counted.iter().max_by(|a, b| a.at.cmp(&b.at)).expect("a recurring group has failures");
            let earliest = counted.iter().min_by(|a, b| a.at.cmp(&b.at)).expect("a recurring group has failures");
            found.push(Repeat {
                new: !self.shown.contains(&id),
                id,
                example: latest.example.clone(),
                sessions: counted.iter().map(|failure| failure.session.as_str()).collect::<HashSet<_>>().len(),
                days: counted.iter().map(|failure| failure.day()).collect::<HashSet<_>>().len(),
                times: counted.len(),
                first: earliest.at.clone(),
                last: latest.at.clone(),
                dismissed: since.cloned(),
            });
        }
        found.sort_by(|a, b| b.sessions.cmp(&a.sessions).then(b.times.cmp(&a.times)).then(b.last.cmp(&a.last)));
        (found, quiet)
    }

    fn report(&mut self, board: &str, left: &Left, set_aside: Vec<String>) -> Report {
        let (recurring, dismissed) = self.recurring();
        self.shown.extend(recurring.iter().map(|repeat| repeat.id.clone()));
        Report {
            board: board.to_string(),
            transcripts: self.transcripts.values().filter(|seen| Whose::of(seen) == Some(Whose::Ours)).count(),
            failures: self.failures.len(),
            fingerprints: self.groups().len(),
            recurring,
            dismissed,
            set_aside,
            left: left.clone(),
        }
    }
}

/// `ekko --repeats`: what is new read in, and what recurs listed -- which
/// counts as shown, so the prime does not tell it again.
pub fn report(home: &Path, root: Option<&Path>, board: &str) -> Result<Report, EkkoError> {
    let root = project_root(root)?;
    report_at(&Place::of(home, root), root, board)
}

fn report_at(place: &Place, root: &Path, board: &str) -> Result<Report, EkkoError> {
    with_state(place, root, None, |state, left| state.report(board, left, Vec::new()))
}

/// `ekko --repeats --dismiss <ids>`: sets fingerprints aside until they
/// recur again, and lists what still recurs.
pub fn dismiss(home: &Path, root: Option<&Path>, board: &str, ids: &[String]) -> Result<Report, EkkoError> {
    let root = project_root(root)?;
    dismiss_at(&Place::of(home, root), root, board, ids)
}

fn dismiss_at(place: &Place, root: &Path, board: &str, ids: &[String]) -> Result<Report, EkkoError> {
    with_state(place, root, None, |state, left| {
        let known: HashSet<String> = state.groups().keys().map(|key| id(key)).collect();
        if let Some(unknown) = ids.iter().find(|wanted| !known.contains(wanted.as_str())) {
            return Err(EkkoError::InvalidInput(format!("no failure's fingerprint is {unknown}: ekko --repeats lists them by id")));
        }
        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        for wanted in ids {
            state.dismissed.insert(wanted.clone(), now.clone());
            // Its return is news again.
            state.shown.remove(wanted);
        }
        Ok(state.report(board, left, ids.to_vec()))
    })?
}

/// The prime's line, for the SessionStart hook: what is new read in, within
/// `HOOK_BUDGET`, and a line only when a fingerprint recurs that no prime or
/// listing showed before. Silent on any failure: a start must not wait on it.
pub fn announce(home: &Path, root: &Path) -> Option<String> {
    announce_at(&Place::of(home, root), root, Instant::now() + HOOK_BUDGET)
}

fn announce_at(place: &Place, root: &Path, deadline: Instant) -> Option<String> {
    with_state(place, root, Some(deadline), |state, _| {
        let (recurring, _) = state.recurring();
        let new: Vec<&Repeat> = recurring.iter().filter(|repeat| repeat.new).collect();
        state.shown.extend(new.iter().map(|repeat| repeat.id.clone()));
        (!new.is_empty()).then(|| line(&new))
    })
    .ok()
    .flatten()
}

/// One line for the prime's Needs attention: the first new recurrences named,
/// the rest counted.
fn line(new: &[&Repeat]) -> String {
    let named: Vec<String> =
        new.iter().take(NAMED).map(|repeat| format!("{} ({} sessions, {})", cut(&repeat.example, 90), repeat.sessions, repeat.id)).collect();
    let more = new.len().saturating_sub(NAMED);
    let more = if more > 0 { format!("; +{more} more") } else { String::new() };
    format!("failures that newly recur across sessions: {}{more} -- ekko --repeats lists them; a gotcha may stop one", named.join("; "))
}

fn project_root(root: Option<&Path>) -> Result<&Path, EkkoError> {
    root.ok_or_else(|| {
        EkkoError::InvalidInput(
            "--repeats reads the Claude Code sessions begun in a project's folder, and the default board has none: run it in a project's folder, or name one with --project".into(),
        )
    })
}

/// `at`, an RFC 3339 time, as the board shows times.
fn when(at: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(at).map_or_else(|_| at.to_string(), |at| crate::holder::when(at.timestamp_millis()))
}

impl Report {
    pub fn text(&self) -> String {
        let mut out = String::new();
        for id in &self.set_aside {
            let _ = writeln!(out, "Dismissed {id}: it comes back only if it recurs again.");
        }
        let held: usize = self.recurring.iter().map(|repeat| repeat.times).sum();
        let _ = writeln!(
            out,
            "Failures that repeat across sessions \u{b7} {} \u{b7} {} of {} fingerprints, holding {held} of {} failures, in {} transcripts",
            self.board,
            self.recurring.len(),
            self.fingerprints,
            self.failures,
            self.transcripts
        );
        if self.recurring.is_empty() {
            let _ = writeln!(out, "None recurs yet: one does once seen in {SESSIONS} sessions on {DAYS} days.");
        }
        for repeat in &self.recurring {
            let mut facts = vec![
                format!("{} sessions", repeat.sessions),
                format!("{} days", repeat.days),
                format!("{} times", repeat.times),
                format!("last {}", when(&repeat.last)),
            ];
            if let Some(dismissed) = &repeat.dismissed {
                facts.push(format!("back since dismissed {}", when(dismissed)));
            }
            if repeat.new {
                facts.push("new".to_string());
            }
            let _ = writeln!(out, "{}  {}", repeat.id, facts.join(" \u{b7} "));
            let _ = writeln!(out, "        {}", repeat.example);
        }
        if self.dismissed > 0 {
            let _ = writeln!(out, "{} dismissed, quiet since.", self.dismissed);
        }
        let mut left = Vec::new();
        if self.left.headless > 0 {
            left.push(format!("{} headless transcripts", self.left.headless));
        }
        if self.left.unreadable > 0 {
            left.push(format!("{} lines that could not be read", self.left.unreadable));
        }
        if self.left.unread > 0 {
            left.push(format!("{} transcripts not reached in time", self.left.unread));
        }
        if !left.is_empty() {
            let _ = writeln!(out, "Left out: {}.", left.join(", "));
        }
        if !self.recurring.is_empty() {
            let _ = writeln!(out, "A gotcha the next session reads may stop one; ekko --repeats --dismiss <id> sets one aside until it recurs again.");
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const PYTHON: &str = "Exit code 127\n/run/current-system/sw/bin/bash: line 1: python3: command not found";

    /// A home with a project folder and one Claude Code config folder, and a
    /// state file of its own: nothing here reads the machine's sessions.
    struct Fixture {
        root: PathBuf,
        place: Place,
    }

    impl Fixture {
        fn new(tag: &str) -> Fixture {
            let home = crate::paths::test_dir(&format!("ekko-repeats-{tag}"));
            let root = home.join("work").join("app");
            fs::create_dir_all(&root).unwrap();
            let place = Place { dirs: vec![home.join(".claude")], file: home.join("state").join("repeats.json") };
            Fixture { root, place }
        }

        /// The transcript of `session`, in the folder Claude Code names after `cwd`.
        fn transcript(&self, cwd: &Path, session: &str) -> PathBuf {
            let folder = self.place.dirs[0].join("projects").join(folder_name(cwd));
            fs::create_dir_all(&folder).unwrap();
            folder.join(format!("{session}.jsonl"))
        }

        /// Appends `text` to `session`'s transcript, begun in the project.
        fn append(&self, session: &str, text: &str) {
            use std::io::Write as _;
            let path = self.transcript(&self.root, session);
            fs::OpenOptions::new().create(true).append(true).open(path).unwrap().write_all(text.as_bytes()).unwrap();
        }

        /// A Bash call in `session` that fails with `text`.
        fn fail(&self, session: &str, at: &str, id: &str, command: &str, text: &str) {
            self.append(session, &format!("{}\n{}\n", call(session, at, &self.root, id, command), failed(session, at, &self.root, id, text)));
        }

        fn report(&self) -> Report {
            report_at(&self.place, &self.root, "project app").unwrap()
        }

        fn announce(&self) -> Option<String> {
            announce_at(&self.place, &self.root, Instant::now() + Duration::from_secs(60))
        }
    }

    fn row(kind: &str, session: &str, at: &str, cwd: &Path, block: Value) -> String {
        json!({"type": kind, "sessionId": session, "timestamp": at, "cwd": cwd, "entrypoint": "cli", "isSidechain": false,
               "message": {"role": kind, "content": [block]}})
        .to_string()
    }

    fn call(session: &str, at: &str, cwd: &Path, id: &str, command: &str) -> String {
        row("assistant", session, at, cwd, json!({"type": "tool_use", "id": id, "name": "Bash", "input": {"command": command}}))
    }

    fn failed(session: &str, at: &str, cwd: &Path, id: &str, text: &str) -> String {
        row("user", session, at, cwd, json!({"type": "tool_result", "content": text, "is_error": true, "tool_use_id": id}))
    }

    fn examples(report: &Report) -> Vec<&str> {
        report.recurring.iter().map(|repeat| repeat.example.as_str()).collect()
    }

    /// Task 1442's step 4: 2 or more sessions on 2 or more days. One session
    /// on two days is a single session's trouble, and two sessions on one day
    /// may be one piece of work.
    #[test]
    fn a_failure_recurs_in_two_sessions_on_two_days_and_not_in_one_of_either() {
        let fixture = Fixture::new("rule");
        fixture.fail("s1", "2026-10-01T10:00:00.000Z", "t1", "python3 a.py", PYTHON);
        fixture.fail("s2", "2026-10-02T10:00:00.000Z", "t2", "python3 b.py", PYTHON);
        fixture.fail("s1", "2026-10-01T11:00:00.000Z", "t3", "cargo test", "Exit code 101\nerror: test failed, one session");
        fixture.fail("s1", "2026-10-03T11:00:00.000Z", "t4", "cargo test", "Exit code 101\nerror: test failed, one session");
        fixture.fail("s3", "2026-10-04T11:00:00.000Z", "t5", "make", "Exit code 2\nmake: *** one day. Stop.");
        fixture.fail("s4", "2026-10-04T12:00:00.000Z", "t6", "make", "Exit code 2\nmake: *** one day. Stop.");

        let report = fixture.report();
        assert_eq!(examples(&report), [format!("Bash: {}", PYTHON.replace('\n', " | "))]);
        let repeat = &report.recurring[0];
        assert_eq!((repeat.sessions, repeat.days, repeat.times), (2, 2, 2));
        assert_eq!((report.failures, report.fingerprints, report.transcripts), (6, 3, 4));
    }

    /// A session begun from another's history repeats its rows, the same
    /// tool_use_id under its own sessionId: counted twice, the copy would make
    /// one session's failure look like a second session's.
    #[test]
    fn a_transcript_that_repeats_anothers_rows_counts_them_once() {
        let fixture = Fixture::new("copies");
        fixture.fail("s1", "2026-10-01T10:00:00.000Z", "t1", "cargo test", "Exit code 101\nerror: test failed");
        fixture.fail("s1", "2026-10-02T10:00:00.000Z", "t2", "cargo test", "Exit code 101\nerror: test failed");
        // s2 begins from s1's history: its first rows are s1's first failure.
        fixture.fail("s2", "2026-10-01T10:00:00.000Z", "t1", "cargo test", "Exit code 101\nerror: test failed");

        let report = fixture.report();
        assert!(report.recurring.is_empty(), "{:?}", examples(&report));
        assert_eq!(report.failures, 2);
    }

    /// Bash's "X: command not found" names the command, so the command's
    /// first word would only split one failure by how it was reached: the
    /// prototype's python3 fell into three fingerprints.
    #[test]
    fn a_message_that_names_its_command_drops_the_commands_word() {
        let by_name = Shape::of("Bash", Some("python3"), PYTHON);
        let by_path = Shape::of("Bash", Some("check.py"), PYTHON);
        assert_eq!(by_name.key(), by_path.key());
        assert_eq!(by_name.head, "Bash");
        let missing = "Exit code 1\n/usr/bin/env: 'python3': No such file or directory";
        assert_eq!(Shape::of("Bash", Some("run.sh"), missing).head, "Bash");

        // A message that does not name it keeps the word: cargo's failure
        // is not make's.
        let other = "Exit code 2\nerror: could not compile";
        assert_ne!(Shape::of("Bash", Some("cargo"), other).key(), Shape::of("Bash", Some("make"), other).key());
        assert_eq!(Shape::of("Bash", Some("cargo"), other).head, "Bash cargo");
    }

    /// Bash's result is `Exit code N` and then the output: the failure is
    /// the output's last line, not its first, which earlier commands print.
    #[test]
    fn a_bash_failure_is_known_by_its_exit_and_its_last_line() {
        let after_a_listing = "Exit code 128\n?? .claude/\nsrc/main.rs\nfatal: Needed a single revision";
        let after_a_log = "Exit code 128\n7a6eeba test(mcp): a budget\nfatal: Needed a single revision";
        let shape = Shape::of("Bash", Some("git"), after_a_listing);
        assert_eq!(shape.message, "Exit code 128 | fatal: Needed a single revision");
        assert_eq!(shape.key(), Shape::of("Bash", Some("git"), after_a_log).key());

        // Any other tool's message is its first two lines.
        let refused = "INVALID_INPUT: a title runs 81\n\nNote: ekko was rebuilt\nmore";
        assert_eq!(Shape::of("mcp__plugin_ekko_ekko__create", None, refused).message, "INVALID_INPUT: a title runs 81 | Note: ekko was rebuilt");
        // And a Bash result that is not an exit's.
        assert_eq!(Shape::of("Bash", Some("sleep"), "<tool_use_error>Blocked: sleep 60\nsecond\nthird").message, "<tool_use_error>Blocked: sleep 60 | second");
    }

    /// What varies between two runs of one failure, as the prototype took it
    /// out: quoted text, paths, hashes and numbers, in that order.
    #[test]
    fn a_message_loses_what_varies_between_runs() {
        assert_eq!(
            normalized("Error at /tmp/x.rs:12: 'foo bar' failed  after 3 tries,\ncommit abcdef1234"),
            "error at /P:0: S failed after 0 tries, commit H"
        );
        assert_eq!(normalized("cannot read src/main.rs, ~/notes or ../up)"), "cannot read src/P, /P or /P)");
        assert_eq!(normalized("\"a\" `b` 'c'"), "S S S");
        // Seven hexadecimal digits make a hash; six, or a word with others, do not.
        assert_eq!(normalized("deadbee cafe12 deadbeefz"), "H cafe0 deadbeefz");
        assert_eq!(normalized(&"x".repeat(200)).chars().count(), MESSAGE_CHARS);
    }

    /// The command's first word is what ran, as the shell lexer reads it.
    #[test]
    fn a_commands_first_word_is_the_command_that_ran() {
        for (command, word) in [
            ("cd /projects/x && cargo test 2>&1 | tail -5", "cargo"),
            ("timeout 5 git status", "git"),
            ("S=1 make all", "make"),
            ("cd /x && nix develop -c cargo build --locked", "cargo"),
            ("bash -c 'ls /nowhere'", "ls"),
            ("export A=1; env python3 x.py", "python3"),
            ("for f in *.rs; do wc -l $f; done", "wc"),
        ] {
            assert_eq!(command_word(command).as_deref(), Some(word), "{command}");
        }
        assert_eq!(command_word("nix build .#ekko").as_deref(), Some("nix"));
        assert_eq!(command_word(""), None);
    }

    /// The prime says a recurrence once: the next prime, and a prime after
    /// `ekko --repeats` listed it, say nothing until another appears.
    #[test]
    fn the_prime_tells_a_new_recurrence_once() {
        let fixture = Fixture::new("prime");
        assert_eq!(fixture.announce(), None);
        fixture.fail("s1", "2026-10-01T10:00:00.000Z", "t1", "python3 a.py", PYTHON);
        assert_eq!(fixture.announce(), None, "one session is no recurrence");
        fixture.fail("s2", "2026-10-02T10:00:00.000Z", "t2", "python3 a.py", PYTHON);
        let line = fixture.announce().expect("a new recurrence is told");
        assert!(line.contains("python3: command not found (2 sessions, "), "{line}");
        assert_eq!(fixture.announce(), None, "told once");

        fixture.fail("s3", "2026-10-03T10:00:00.000Z", "t3", "python3 a.py", PYTHON);
        assert_eq!(fixture.announce(), None, "a third session is the same recurrence");

        fixture.fail("s3", "2026-10-03T11:00:00.000Z", "t4", "make", "Exit code 2\nmake: *** No rule to make target");
        fixture.fail("s4", "2026-10-04T11:00:00.000Z", "t5", "make", "Exit code 2\nmake: *** No rule to make target");
        let report = fixture.report();
        assert_eq!(report.recurring.iter().filter(|repeat| repeat.new).count(), 1, "the listing shows what is new");
        assert_eq!(fixture.announce(), None, "a recurrence the listing showed is not told again");
    }

    /// A fingerprint set aside stays out of the list while quiet, and comes
    /// back once it recurs again: 2 sessions on 2 days after the dismissal.
    #[test]
    fn a_dismissed_fingerprint_comes_back_only_if_it_recurs_again() {
        let fixture = Fixture::new("dismiss");
        fixture.fail("s1", "2026-10-01T10:00:00.000Z", "t1", "python3 a.py", PYTHON);
        fixture.fail("s2", "2026-10-02T10:00:00.000Z", "t2", "python3 a.py", PYTHON);
        let id = fixture.report().recurring[0].id.clone();

        let report = dismiss_at(&fixture.place, &fixture.root, "project app", std::slice::from_ref(&id)).unwrap();
        assert!(report.recurring.is_empty());
        assert_eq!((report.dismissed, report.set_aside.as_slice()), (1, std::slice::from_ref(&id)));
        assert!(dismiss_at(&fixture.place, &fixture.root, "project app", &["nosuch".to_string()]).is_err());

        // Once more, in one session: still quiet.
        fixture.fail("s3", "2099-01-01T10:00:00.000Z", "t3", "python3 a.py", PYTHON);
        assert!(fixture.report().recurring.is_empty());
        fixture.fail("s4", "2099-01-02T10:00:00.000Z", "t4", "python3 a.py", PYTHON);
        let report = fixture.report();
        assert_eq!(report.recurring.len(), 1);
        let back = &report.recurring[0];
        assert_eq!((back.id.as_str(), back.sessions, back.times, back.new), (id.as_str(), 2, 2, true));
        assert!(back.dismissed.is_some());
    }

    /// A read takes only the whole lines written since the last: a line
    /// still being written waits, and nothing is read twice.
    #[test]
    fn a_read_takes_only_the_whole_lines_written_since_the_last() {
        let fixture = Fixture::new("offsets");
        let at = "2026-10-01T10:00:00.000Z";
        fixture.append("s1", "{not json, \"is_error\":true}\n");
        fixture.fail("s1", at, "t1", "cargo test", "Exit code 101\nerror: one");
        let second = format!("{}\n{}\n", call("s1", at, &fixture.root, "t2", "cargo test"), failed("s1", at, &fixture.root, "t2", "Exit code 101\nerror: two"));
        let (head, tail) = second.split_at(second.len() - 40);
        fixture.append("s1", head);

        let report = fixture.report();
        assert_eq!((report.failures, report.left.unreadable), (1, 1));
        fixture.append("s1", tail);
        let report = fixture.report();
        assert_eq!((report.failures, report.left.unreadable), (2, 0));
    }

    /// A call whose result comes after a read keeps its tool and command.
    #[test]
    fn a_call_read_before_its_result_keeps_its_command() {
        let fixture = Fixture::new("pending");
        let at = "2026-10-01T10:00:00.000Z";
        fixture.append("s1", &format!("{}\n", call("s1", at, &fixture.root, "t1", "cd /x && cargo test")));
        assert_eq!(fixture.report().failures, 0);
        fixture.append("s1", &format!("{}\n", failed("s1", at, &fixture.root, "t1", "Exit code 101\nerror: test failed")));
        let state = State::load(&fixture.place.file, &fixture.root);
        assert_eq!(state.failures.len(), 0, "read only by the next report");
        fixture.report();
        let state = State::load(&fixture.place.file, &fixture.root);
        assert_eq!(state.failures[0].key.lines().next(), Some("Bash cargo"));
    }

    /// The transcripts of a folder whose name only reads like the project's,
    /// and of headless runs, are not the project's sessions.
    #[test]
    fn another_folders_and_headless_transcripts_are_left_out() {
        let fixture = Fixture::new("whose");
        fixture.fail("s1", "2026-10-01T10:00:00.000Z", "t1", "python3 a.py", PYTHON);
        // /work/app-old reads as /work/app's folder plus "-old".
        let elsewhere = fixture.root.with_file_name("app-old");
        let path = fixture.transcript(&elsewhere, "s2");
        fs::write(&path, format!("{}\n{}\n", call("s2", "2026-10-02T10:00:00.000Z", &elsewhere, "t2", "python3 a.py"), failed("s2", "2026-10-02T10:00:00.000Z", &elsewhere, "t2", PYTHON))).unwrap();
        let headless = fixture.transcript(&fixture.root, "s3");
        let rows = format!("{}\n{}\n", call("s3", "2026-10-03T10:00:00.000Z", &fixture.root, "t3", "python3 a.py"), failed("s3", "2026-10-03T10:00:00.000Z", &fixture.root, "t3", PYTHON));
        fs::write(&headless, rows.replace("\"entrypoint\":\"cli\"", "\"entrypoint\":\"sdk-cli\"")).unwrap();
        // A subfolder's sessions are the project's.
        let inside = fixture.root.join("src");
        let path = fixture.transcript(&inside, "s4");
        fs::write(&path, format!("{}\n{}\n", call("s4", "2026-10-04T10:00:00.000Z", &inside, "t4", "make"), failed("s4", "2026-10-04T10:00:00.000Z", &inside, "t4", "Exit code 2\nmake: nothing"))).unwrap();

        let report = fixture.report();
        assert!(report.recurring.is_empty(), "{:?}", examples(&report));
        assert_eq!((report.transcripts, report.failures, report.left.headless), (2, 2, 1));
    }
}
