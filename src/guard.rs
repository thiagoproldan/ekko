//! Guards on a Claude Code session's calls (task 805, decision 801).
//!
//! A gotcha may carry a cue: a command, words what the call is given must
//! hold, and a folder. Once the user turns it on, `ekko --guard --hook`, a
//! PreToolUse hook on Bash, refuses each call that matches it, with the
//! gotcha's text as the reason. The match is against the calls the command
//! parses into (`crate::shell`), never its text, so a here-document or a
//! memory file that names the call passes.
//!
//! A cue on a project's board guards the calls that run inside the project's
//! folder, and one on the default board the whole machine; a cue may name a
//! folder of its own. Every board is read, not the session's alone: the
//! incident that asked for this ran in a folder with no board of its own
//! (note 797). What the boards hold is kept in one small index beside the
//! default board, rebuilt when a board's file changes, so a Bash call does
//! not parse every board.
//!
//! A refused call -- by a cue, or by another guard, such as ctx's, through
//! `ekko --guard --refuse` -- is recorded under a code, which the reason
//! gives. A session asks the user with that code (`ask` with `allow`), the
//! question quotes the call as recorded, and the user's "Allow once" in
//! ekko's menu lets the same call, from the same folder and the same
//! session, through once within a day: dcg's allow-once, answered where the
//! user already answers. The tool call that used it is recorded on the
//! question, so each guard that sees that call lets it through, and no other
//! call gets through on it.
//!
//! Only a person's answer counts. ekko tells a person from a session by the
//! process tree (`crate::holder`); the threat is an accident, not a hijacked
//! agent, which a hook seeing only the literal call cannot stop (note 796).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::holder::{Actor, Process};
use crate::item::{Allowance, Cue, Item, Knowledge, Proposal};
use crate::storage::{ItemMap, Storage};

/// The answers ekko offers under a question it composes; only the first of
/// each pair does anything.
pub const TURN_ON: &str = "Turn on";
pub const LEAVE_OFF: &str = "Leave off";
pub const TURN_OFF: &str = "Turn off";
pub const KEEP_ON: &str = "Keep on";
pub const ALLOW_ONCE: &str = "Allow once";
pub const KEEP_REFUSED: &str = "Keep refused";

/// How long a refusal can be asked about, and how long the user's answer
/// lets its call through.
const DAY: i64 = 86_400_000;

/// Past this, a call is quoted in a question by its start and its end.
const QUOTED: usize = 12_000;

/// The cues that are on in `data`, by the uid of their gotcha: a gotcha's
/// cue is on while it is a gotcha, on the board, in neither the trash nor
/// the stash.
pub fn cues_on(data: &ItemMap) -> BTreeMap<String, Cue> {
    data.values().filter(|item| on(item)).filter_map(|item| Some((item.uid.clone()?, item.cue.as_ref()?.cue.clone()))).collect()
}

/// `item`'s cue, while it is on.
pub fn cue_of(item: &Item) -> Option<&Cue> {
    item.cue.as_ref().filter(|_| on(item)).map(|on| &on.cue)
}

fn on(item: &Item) -> bool {
    !item.is_task
        && item.knowledge == Some(Knowledge::Gotcha)
        && item.cue.is_some()
        && item.trashed.is_none()
        && item.stashed.is_none()
}

// --- what the guard reads ------------------------------------------------------

/// A board the guard reads: where it lives, and the folder its cues guard
/// when they name none -- `None` on the default board: the whole machine.
struct Board {
    dir: PathBuf,
    folder: Option<PathBuf>,
}

fn boards(home: &Path) -> Vec<Board> {
    let mut found = Vec::new();
    if let Ok(dir) = crate::directory::retrieve_ekko_directory(home, home, None, None) {
        found.push(Board { dir, folder: None });
    }
    for (root, dir) in crate::project::boards(home) {
        found.push(Board { dir, folder: Some(root) });
    }
    found
}

/// Whether the guard reads the board whose storage file is `storage`: the
/// default board, or a registered project's. One opened through EKKO_DIR or
/// --ekko-dir is neither, so a cue on it would never refuse (task 827).
pub fn reads(home: &Path, storage: &Path) -> bool {
    let real = |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    boards(home).iter().any(|board| real(&storage_file(&board.dir)) == real(storage))
}

/// Where the guard keeps its index and the calls it refused: beside the
/// default board.
fn state(home: &Path) -> PathBuf {
    crate::directory::retrieve_ekko_directory(home, home, None, None)
        .unwrap_or_else(|_| home.join(crate::directory::EKKO_DIR_NAME))
        .join("guard")
}

fn storage_file(board: &Path) -> PathBuf {
    board.join("storage").join("storage.json")
}

/// A file's size and modification time, which change with each write.
type Stamp = Option<(u64, u64, u32)>;

fn stamp(path: &Path) -> Stamp {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    Some((meta.len(), modified.as_secs(), modified.subsec_nanos()))
}

/// What the boards hold for the guard, as of the stamps it was read at.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Index {
    /// The registry's, then each board's storage file's.
    stamps: Vec<(PathBuf, Stamp)>,
    cues: Vec<Indexed>,
    granted: Vec<Granted>,
}

/// A cue that is on.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Indexed {
    board: PathBuf,
    /// Its gotcha, by display id, and the gotcha's text: the reason.
    id: u32,
    text: String,
    #[serde(flatten)]
    cue: Cue,
    /// The folder it guards; `None`, the whole machine.
    guards: Option<PathBuf>,
}

/// A refused call the user answered "Allow once" for.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Granted {
    board: PathBuf,
    /// The question, by display id and by uid.
    id: u32,
    uid: String,
    code: String,
    /// The session that asked, which alone it lets through.
    session: Option<(u32, u64, String)>,
    answered: i64,
    /// The tool call that went through on it.
    used: Option<String>,
}

fn key(process: &Process) -> (u32, u64, String) {
    (process.pid, process.start, process.boot.clone())
}

/// The index, read back while no stamp has moved, else rebuilt and kept.
fn current(home: &Path, now: i64) -> Index {
    let boards = boards(home);
    let registry = crate::project::registry_file(home);
    let stamps: Vec<(PathBuf, Stamp)> = std::iter::once(registry)
        .chain(boards.iter().map(|board| storage_file(&board.dir)))
        .map(|path| {
            let stamped = stamp(&path);
            (path, stamped)
        })
        .collect();
    let file = state(home).join("index.json");
    let kept = std::fs::read(&file).ok().and_then(|bytes| serde_json::from_slice::<Index>(&bytes).ok());
    if let Some(index) = kept.filter(|index| index.stamps == stamps) {
        return index;
    }
    let index = build(&boards, stamps, now);
    // A lost cache costs the next call a rebuild, never a refusal.
    let _ = write_atomically(&file, &serde_json::to_vec(&index).unwrap_or_default());
    index
}

fn build(boards: &[Board], stamps: Vec<(PathBuf, Stamp)>, now: i64) -> Index {
    let mut index = Index { stamps, ..Index::default() };
    for board in boards {
        // Only a board already there: opening one creates its folders.
        if !storage_file(&board.dir).is_file() {
            continue;
        }
        let Ok(data) = Storage::new(&board.dir).and_then(|storage| storage.get_shared()) else { continue };
        for item in data.values() {
            if let (true, Some(cue)) = (on(item), &item.cue) {
                let guards = cue.cue.folder.as_deref().map(PathBuf::from).or_else(|| board.folder.clone());
                index.cues.push(Indexed {
                    board: board.dir.clone(),
                    id: item.id,
                    text: item.description.clone(),
                    cue: cue.cue.clone(),
                    guards,
                });
            }
            let Some(question) = item.question.as_ref().filter(|_| item.trashed.is_none()) else { continue };
            let (Some(allow), Some(answer), Some(uid)) = (&question.allow, &question.answer, &item.uid) else { continue };
            if answer.by_person() && answer.text == ALLOW_ONCE && now - answer.at < DAY {
                index.granted.push(Granted {
                    board: board.dir.clone(),
                    id: item.id,
                    uid: uid.clone(),
                    code: allow.code.clone(),
                    session: question.asked_by.as_ref().and_then(|by| by.process()).as_ref().map(key),
                    answered: answer.at,
                    used: allow.used.as_ref().map(|used| used.tool_use_id.clone()),
                });
            }
        }
    }
    index
}

fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let temp = dir.join(format!(".{}.{}", path.file_name().and_then(|n| n.to_str()).unwrap_or("guard"), std::process::id()));
    std::fs::write(&temp, bytes)?;
    std::fs::rename(&temp, path)
}

// --- matching ------------------------------------------------------------------

/// The cues the calls of `command`, run from `cwd`, hit: a call of the
/// cue's command, in its folder or under it, whose arguments and what is
/// fed to it hold every one of its words.
fn hits<'a>(cues: &'a [Indexed], command: &str, cwd: &Path) -> Vec<&'a Indexed> {
    let (calls, placed) = crate::shell::located(command, cwd);
    let mut found: Vec<&Indexed> = Vec::new();
    for place in &placed {
        let call = &calls[place.at];
        for cue in cues.iter().filter(|cue| cue.cue.command == call.name) {
            if cue.guards.as_deref().is_some_and(|folder| !under(&place.folder, folder)) {
                continue;
            }
            let fed = crate::shell::fed(&calls, place.at);
            let held = |word: &String| call.args.iter().chain(&fed).any(|text| holds(text, word));
            if cue.cue.words.iter().all(held) && !found.iter().any(|hit| hit.board == cue.board && hit.id == cue.id) {
                found.push(cue);
            }
        }
    }
    found
}

/// Whether `text` holds `word`: all of it, or as a whole word inside it,
/// bounded by what is neither a letter, a digit, `_` nor `-`.
fn holds(text: &str, word: &str) -> bool {
    let inner = |c: char| c.is_alphanumeric() || c == '_' || c == '-';
    !word.is_empty()
        && text.match_indices(word).any(|(at, _)| {
            let before = text[..at].chars().next_back();
            let after = text[at + word.len()..].chars().next();
            !before.is_some_and(inner) && !after.is_some_and(inner)
        })
}

fn under(path: &Path, folder: &Path) -> bool {
    let real = |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    real(path).starts_with(real(folder))
}

/// How a cue reads, as its gotcha's question and the refusal put it.
pub fn described(cue: &Cue, board_folder: Option<&Path>) -> String {
    let command = &cue.command;
    let calls = match cue.words.as_slice() {
        [] => format!("every `{command}` call"),
        words => format!("`{command}` calls holding {}", listed(&words.iter().map(|word| format!("`{word}`")).collect::<Vec<_>>())),
    };
    let place = match (&cue.folder, board_folder) {
        (Some(folder), _) => format!("run in {folder} or under it"),
        (None, Some(root)) => format!("run in {} or under it, this board's project", root.display()),
        (None, None) => "anywhere on this machine".to_string(),
    };
    format!("{calls}, {place}")
}

fn listed(words: &[String]) -> String {
    match words {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

// --- refusals ------------------------------------------------------------------

/// A call as a guard sees it, from the PreToolUse event.
struct Seen {
    tool: String,
    /// A Bash command, or another tool's input, its keys in order.
    call: String,
    cwd: String,
    tool_use_id: String,
}

fn seen(event: &Value) -> Option<Seen> {
    let tool = event.get("tool_name")?.as_str()?.to_string();
    let input = event.get("tool_input")?;
    let call = match input.get("command").and_then(Value::as_str) {
        Some(command) if tool == "Bash" => command.to_string(),
        _ => {
            // What a call says of itself is not what it does.
            let mut input = input.clone();
            if let Some(fields) = input.as_object_mut() {
                fields.remove("description");
            }
            canonical(&input).to_string()
        }
    };
    Some(Seen {
        tool,
        call,
        cwd: event.get("cwd").and_then(Value::as_str).unwrap_or_default().to_string(),
        tool_use_id: event.get("tool_use_id").and_then(Value::as_str).unwrap_or_default().to_string(),
    })
}

fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(fields) => {
            let mut sorted: Vec<(&String, &Value)> = fields.iter().collect();
            sorted.sort_by(|a, b| a.0.cmp(b.0));
            Value::Object(sorted.into_iter().map(|(key, value)| (key.clone(), canonical(value))).collect::<Map<_, _>>())
        }
        Value::Array(values) => Value::Array(values.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

/// The code a refused call goes by: the same for the same call from the same
/// folder and session, so each guard that refuses it gives the same one, and
/// a retry finds the answer given for it.
fn code(session: Option<&Process>, seen: &Seen) -> String {
    let session = session.map(|process| format!("{}:{}:{}", process.pid, process.start, process.boot)).unwrap_or_default();
    // FNV-1a, fixed across processes and versions, unlike std's hasher.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for part in [session.as_str(), &seen.cwd, &seen.tool, &seen.call] {
        for byte in part.bytes().chain(std::iter::once(0)) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    }
    const ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";
    (0..6).map(|i| ALPHABET[((hash >> (i * 5)) & 31) as usize] as char).collect()
}

/// A refused call, kept under its code for the question that asks about it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Refused {
    pub code: String,
    /// When a guard last refused it.
    pub at: i64,
    pub tool: String,
    pub call: String,
    pub cwd: String,
    pub session: Option<(u32, u64, String)>,
    pub reasons: Vec<String>,
    /// What a later version keeps here that this one does not know, written
    /// back as read; see `Item::unknown`.
    #[serde(flatten)]
    pub unknown: BTreeMap<String, serde_json::Value>,
}

fn refused_dir(home: &Path) -> PathBuf {
    state(home).join("refused")
}

/// Records that `seen` was refused, with `reasons` beside any another guard
/// gave for it, and gives back all of them; records older than a day go on
/// the way past.
fn record(home: &Path, seen: &Seen, session: Option<&Process>, code: &str, reasons: &[String], now: i64) -> std::io::Result<Vec<String>> {
    let dir = refused_dir(home);
    std::fs::create_dir_all(&dir)?;
    let _lock = crate::storage::lock_path(&dir.join(".lock")).map_err(std::io::Error::other)?;
    let path = dir.join(format!("{code}.json"));
    let session = session.map(key);
    let kept = std::fs::read(&path).ok().and_then(|bytes| serde_json::from_slice::<Refused>(&bytes).ok());
    let same = |kept: &Refused| kept.call == seen.call && kept.cwd == seen.cwd && kept.session == session && now - kept.at < DAY;
    let mut refused = kept.filter(same).unwrap_or_else(|| Refused {
        code: code.to_string(),
        at: now,
        tool: seen.tool.clone(),
        call: seen.call.clone(),
        cwd: seen.cwd.clone(),
        session,
        reasons: Vec::new(),
        unknown: BTreeMap::new(),
    });
    refused.at = now;
    for reason in reasons {
        if !refused.reasons.contains(reason) {
            refused.reasons.push(reason.clone());
        }
    }
    write_atomically(&path, &serde_json::to_vec_pretty(&refused).unwrap_or_default())?;
    // Taking the lock never touches its file's time, so it looks old too;
    // deleting it while held would let the next writer lock a new one.
    for entry in std::fs::read_dir(&dir)?.flatten().filter(|entry| entry.file_name() != ".lock") {
        let old = entry.metadata().and_then(|meta| meta.modified()).ok().and_then(|time| time.elapsed().ok());
        if old.is_some_and(|age| age.as_millis() > DAY as u128) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    Ok(refused.reasons)
}

/// The refusal `code` names, when a guard made it in the last day for
/// `session`: what a question asking about it quotes.
pub fn refusal(home: &Path, code: &str, session: Option<&Process>, now: i64) -> Result<Refused, String> {
    let plain = !code.is_empty() && code.chars().all(|c| c.is_ascii_alphanumeric());
    let found = plain
        .then(|| std::fs::read(refused_dir(home).join(format!("{code}.json"))).ok())
        .flatten()
        .and_then(|bytes| serde_json::from_slice::<Refused>(&bytes).ok());
    let Some(refused) = found.filter(|refused| now - refused.at < DAY) else {
        return Err(format!("allow {code:?} names no call a guard refused in the last day: ask with the code the refusal gave"));
    };
    if refused.session != session.map(key) {
        return Err(format!("allow {code:?} names a call another session made: only the session whose call was refused asks for it"));
    }
    Ok(refused)
}

/// What a guard's refusal of a call comes to.
enum Verdict {
    /// The user's answer to question `id` lets it through.
    Through(u32),
    /// Refused, and recorded under this code, with every reason a guard has
    /// given for it so far: this one's and any another recorded first.
    Refused(String, Vec<String>),
}

/// Whether `seen` goes through on the user's answer, or else is refused,
/// with `reasons`. `actor` is the session making the call.
fn decide(home: &Path, index: &Index, seen: &Seen, reasons: &[String], actor: &Actor, now: i64) -> Verdict {
    let session = actor.process.as_ref();
    let code = code(session, seen);
    let asked = |grant: &&Granted| grant.code == code && now - grant.answered < DAY && grant.session == session.map(key);
    for grant in index.granted.iter().filter(asked) {
        match &grant.used {
            Some(used) if *used == seen.tool_use_id && !used.is_empty() => return Verdict::Through(grant.id),
            Some(_) => continue,
            None if spend(home, grant, &seen.tool_use_id, actor) => return Verdict::Through(grant.id),
            None => continue,
        }
    }
    let all = record(home, seen, session, &code, reasons, now).unwrap_or_else(|_| reasons.to_vec());
    Verdict::Refused(code, all)
}

/// Records on `grant`'s question that this tool call used it, unless
/// another did first.
fn spend(home: &Path, grant: &Granted, tool_use_id: &str, actor: &Actor) -> bool {
    if tool_use_id.is_empty() {
        return false;
    }
    let Ok(storage) = Storage::new(&grant.board) else { return false };
    let ekko = crate::ekko::Ekko::new(storage.copied_to(crate::project::copy_dir(home, &grant.board))).acting_as(actor.clone());
    let Ok(mut draft) = crate::ops::Draft::open(&ekko) else { return false };
    match draft.spend_allowance(&grant.uid, tool_use_id) {
        Ok(true) => draft.commit(false).is_ok(),
        _ => false,
    }
}

/// The sentence a refusal ends with: how the user can let the call through.
fn how_to_ask(code: &str) -> String {
    format!(
        "If the user wants this exact call made anyway, ask them through ekko's ask with allow set to \"{code}\" on the \
         question. Only their answer in ekko's menu lets it through: once, from this folder and this session, within \
         24 hours. An answer a session records does not count."
    )
}

// --- the hooks -----------------------------------------------------------------

/// `ekko --guard --hook`: Claude Code's PreToolUse on Bash. Refuses a call
/// a cue that is on names, unless the user let it through; silent for
/// everything else, and on any error, since a broken guard must not break
/// the shell.
pub fn hook(home: &Path, input: &str) -> ExitCode {
    if let Some(reply) = hook_reply(home, input, &Actor::of_this_command()) {
        println!("{reply}");
    }
    ExitCode::SUCCESS
}

/// What the hook answers `input` with, for the session `actor`: nothing,
/// a refusal, or word that the user's answer let the call through.
fn hook_reply(home: &Path, input: &str, actor: &Actor) -> Option<Value> {
    let event = serde_json::from_str::<Value>(input).ok()?;
    let seen = seen(&event).filter(|seen| seen.tool == "Bash")?;
    let now = chrono::Local::now().timestamp_millis();
    let index = current(home, now);
    let reasons = cue_reasons(home, &index, &seen);
    if reasons.is_empty() {
        return None;
    }
    Some(match decide(home, &index, &seen, &reasons, actor, now) {
        Verdict::Through(question) => json!({"hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "additionalContext": format!("ekko: the user's answer to question {question} let this call through, once, past the cue that refuses it."),
        }}),
        Verdict::Refused(code, all) => json!({"hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": clipped(&format!("{}{}", reasons.join("\n\n"), ending(&code, &all, &reasons)), 9_000),
        }}),
    })
}

/// The reasons of the cues that are on and name the Bash call `seen`.
fn cue_reasons(home: &Path, index: &Index, seen: &Seen) -> Vec<String> {
    // Most calls name no cue's command at all, and need no parse.
    if seen.tool != "Bash" || !index.cues.iter().any(|cue| seen.call.contains(cue.cue.command.as_str())) {
        return Vec::new();
    }
    let found = hits(&index.cues, &seen.call, Path::new(if seen.cwd.is_empty() { "/" } else { &seen.cwd }));
    let folders = boards(home);
    found
        .iter()
        .map(|cue| {
            let folder = folders.iter().find(|board| board.dir == cue.board).and_then(|board| board.folder.as_deref());
            format!(
                "ekko gotcha {}: its cue, which the user turned on, refuses {}. The gotcha:\n{}",
                cue.id,
                described(&cue.cue, folder),
                clipped(&cue.text, 3_000)
            )
        })
        .collect()
}

/// What a guard's refusal ends with, after its own `reasons`: those of
/// `all` another guard gave, then how to ask. Claude Code runs the hooks at
/// once and shows the model only the reason of the last to finish refusing
/// (2.1.283, read off its bundle), so each guard's carries every reason
/// recorded by then (task 839).
fn ending(code: &str, all: &[String], reasons: &[String]) -> String {
    let others: String = all.iter().filter(|reason| !reasons.contains(reason)).map(|other| format!("\n\n{other}")).collect();
    format!("{others}\n\n{}", how_to_ask(code))
}

/// `ekko --guard --refuse REASON`, for another guard about to refuse the
/// call on stdin, a PreToolUse event: exits 0, printing nothing, when the
/// user let this call through; else records the refusal with `reason`,
/// prints what the refusal should end with -- the reasons of ekko's cues
/// and of any other guard that refused the call, then how to ask -- and
/// exits 1. Anything it cannot read exits 2, and the other guard refuses as
/// it would have.
pub fn refuse(home: &Path, input: &str, reason: &str) -> ExitCode {
    match refuse_reply(home, input, reason, &Actor::of_this_command()) {
        None => ExitCode::from(2),
        Some(None) => ExitCode::SUCCESS,
        Some(Some(sentence)) => {
            println!("{sentence}");
            ExitCode::FAILURE
        }
    }
}

/// `refuse`'s answer for the session `actor`: `None` for input it cannot
/// read, else what a refusal ends with, or `None` inside for a call the
/// user let through.
fn refuse_reply(home: &Path, input: &str, reason: &str, actor: &Actor) -> Option<Option<String>> {
    let seen = seen(&serde_json::from_str::<Value>(input).ok()?)?;
    let now = chrono::Local::now().timestamp_millis();
    let index = current(home, now);
    let given = vec![reason.trim().to_string()];
    let reasons = [given.clone(), cue_reasons(home, &index, &seen)].concat();
    Some(match decide(home, &index, &seen, &reasons, actor, now) {
        Verdict::Through(_) => None,
        Verdict::Refused(code, all) => Some(ending(&code, &all, &given).trim_start().to_string()),
    })
}

fn clipped(text: &str, most: usize) -> String {
    let length = text.chars().count();
    if length <= most {
        return text.to_string();
    }
    let head: String = text.chars().take(most).collect();
    format!("{head}… ({} more characters)", length - most)
}

// --- questions -----------------------------------------------------------------

/// The block under a question asking about a refused call, and the answers
/// offered: the call as the guard recorded it, not as the session put it.
pub fn allow_block(refused: &Refused) -> (String, Allowance) {
    let length = refused.call.chars().count();
    let call = if length <= QUOTED {
        refused.call.clone()
    } else {
        let head: String = refused.call.chars().take(QUOTED - 2_000).collect();
        let tail: String = refused.call.chars().skip(length - 2_000).collect();
        format!("{head}\n[… {} characters not shown …]\n{tail}", length - QUOTED)
    };
    let why: String = refused.reasons.iter().map(|reason| format!("\n- {}", clipped(reason, 1_500))).collect();
    let text = format!("\n\nThe call refused ({}), from {}:\n{call}\nWhy:{why}", refused.tool, refused.cwd);
    let allowance = Allowance {
        code: refused.code.clone(),
        tool: refused.tool.clone(),
        call: refused.call.clone(),
        cwd: refused.cwd.clone(),
        reasons: refused.reasons.clone(),
        used: None,
        unknown: BTreeMap::new(),
    };
    (text, allowance)
}

/// The block under a question proposing `proposal` for `gotcha`, on a board
/// whose project is `board_folder`.
pub fn proposal_block(gotcha: &Item, proposal: &Proposal, board_folder: Option<&Path>) -> String {
    let lesson = clipped(&gotcha.description, 600);
    match (&proposal.cue, &gotcha.cue) {
        (Some(cue), None) => format!(
            "\n\nThe cue proposed for gotcha {}: refuse {}, giving the gotcha as the reason:\n{lesson}",
            gotcha.id,
            described(cue, board_folder)
        ),
        (Some(cue), Some(now)) => format!(
            "\n\nThe cue proposed for gotcha {}: refuse {}, in place of its cue now, which refuses {}. The gotcha:\n{lesson}",
            gotcha.id,
            described(cue, board_folder),
            described(&now.cue, board_folder)
        ),
        (None, Some(now)) => format!(
            "\n\nProposed: turn off the cue of gotcha {}, which refuses {}. The gotcha:\n{lesson}",
            gotcha.id,
            described(&now.cue, board_folder)
        ),
        (None, None) => format!("\n\nGotcha {} has no cue to turn off.", gotcha.id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ekko::{Ekko, EkkoError};
    use crate::holder::test_sessions;
    use crate::item::CueOn;
    use crate::ops::{Draft, Inquiry, Op, Ref};

    fn home(tag: &str) -> PathBuf {
        let home = crate::paths::test_dir(&format!("ekko-guard-{tag}"));
        std::fs::create_dir_all(home.join(".ekko")).unwrap();
        home
    }

    fn ekko_at(dir: &Path, actor: Option<&Actor>) -> Ekko {
        let ekko = Ekko::new(Storage::new(dir).unwrap());
        match actor {
            Some(actor) => ekko.acting_as(actor.clone()),
            None => ekko,
        }
    }

    fn apply(ekko: &Ekko, op: Value) -> Result<Vec<u32>, EkkoError> {
        let mut draft = Draft::open(ekko)?;
        let ids = draft.apply(&serde_json::from_value::<Op>(op).unwrap())?;
        draft.commit(false)?;
        Ok(ids)
    }

    fn cue(command: &str, words: &[&str], folder: Option<&Path>) -> Cue {
        Cue {
            command: command.into(),
            words: words.iter().map(|word| word.to_string()).collect(),
            folder: folder.map(|folder| folder.to_string_lossy().into_owned()),
            unknown: BTreeMap::new(),
        }
    }

    /// A gotcha on the board at `dir`, its cue on as the user's answer
    /// would have left it: its id.
    fn cued(dir: &Path, text: &str, cue: Cue) -> u32 {
        let ekko = ekko_at(dir, None);
        let id = apply(&ekko, json!({"op": "create", "kind": "gotcha", "text": text})).unwrap()[0];
        let mut data = ekko.storage.get().unwrap();
        data.get_mut(&id).unwrap().cue = Some(CueOn { cue, question: "q".into(), at: 0 });
        ekko.storage.set(&data).unwrap();
        id
    }

    fn event(command: &str, cwd: &Path, tool_use_id: &str) -> String {
        json!({"tool_name": "Bash", "tool_input": {"command": command}, "cwd": cwd, "tool_use_id": tool_use_id}).to_string()
    }

    /// The refusal's reason, when the reply is a refusal.
    fn refused(reply: &Option<Value>) -> Option<String> {
        let output = &reply.as_ref()?["hookSpecificOutput"];
        (output["permissionDecision"] == "deny").then(|| output["permissionDecisionReason"].as_str().unwrap().to_string())
    }

    fn code_in(reason: &str) -> String {
        reason.split("allow set to \"").nth(1).unwrap()[..6].to_string()
    }

    fn ask(ekko: &Ekko, home: &Path, question: Value) -> Result<(u32, Inquiry), EkkoError> {
        let mut draft = Draft::open(ekko)?;
        let asked = draft.ask_inquiry(&serde_json::from_value(question).unwrap(), None, home)?;
        draft.commit(false)?;
        Ok(asked)
    }

    fn answer(dir: &Path, actor: &Actor, question: u32, text: &str) -> Result<(), EkkoError> {
        let ekko = ekko_at(dir, Some(actor));
        let mut draft = Draft::open(&ekko)?;
        draft.answer(&Ref::Id(question), text)?;
        draft.commit(false).map(|_| ())
    }

    #[test]
    fn a_word_is_held_whole_or_bounded_inside_a_longer_text() {
        assert!(holds("--all", "--all"));
        assert!(holds("mutation {\n  updateProjectV2Field(input: {", "updateProjectV2Field"));
        assert!(holds("query=updateProjectV2Field", "updateProjectV2Field"));
        assert!(!holds("--all-features", "--all"));
        assert!(!holds("rustfmt", "fmt"));
        assert!(!holds("updateProjectV2FieldX", "updateProjectV2Field"));
        assert!(!holds("anything", ""));
    }

    #[test]
    fn a_code_is_the_same_for_the_same_call_and_differs_for_any_other() {
        let session = Process { pid: 7, start: 11, boot: "b".to_string() };
        let at = |cwd: &str, call: &str| Seen { tool: "Bash".into(), call: call.into(), cwd: cwd.into(), tool_use_id: "x".into() };
        let one = code(Some(&session), &at("/r", "git reset --hard"));
        assert_eq!(one.len(), 6);
        assert_eq!(one, code(Some(&session), &Seen { tool_use_id: "another".into(), ..at("/r", "git reset --hard") }));
        assert_ne!(one, code(Some(&session), &at("/other", "git reset --hard")));
        assert_ne!(one, code(Some(&session), &at("/r", "git reset --hard ")));
        assert_ne!(one, code(Some(&Process { pid: 8, ..session.clone() }), &at("/r", "git reset --hard")));
    }

    #[test]
    fn another_tool_s_call_is_its_input_without_the_description() {
        let event = json!({"tool_name": "Read", "tool_input": {"limit": 5, "file_path": "/x"}, "cwd": "/", "tool_use_id": "t"});
        let described = json!({"tool_name": "Read", "tool_input": {"file_path": "/x", "description": "d", "limit": 5}, "cwd": "/"});
        assert_eq!(seen(&event).unwrap().call, seen(&described).unwrap().call);
        assert_eq!(seen(&event).unwrap().call, r#"{"file_path":"/x","limit":5}"#);
    }

    #[test]
    fn a_cue_refuses_the_calls_it_names_and_not_the_text_that_names_them() {
        let home = home("names");
        let (session, _, _) = test_sessions();
        let lesson = "Changing the Status field's options recreates them and clears every item's status.";
        let gotcha = cued(&home.join(".ekko"), lesson, cue("gh", &["api", "graphql", "updateProjectV2Field"], None));
        let at = |command: &str| refused(&hook_reply(&home, &event(command, Path::new("/"), "t1"), &session));
        let reason = at("gh api graphql -f query='mutation { updateProjectV2Field(input: {fieldId: \"x\"}) { x } }'").expect("refused");
        assert!(reason.starts_with(&format!("ekko gotcha {gotcha}: its cue, which the user turned on, refuses")), "{reason}");
        assert!(reason.contains(lesson) && reason.contains("allow set to \""), "{reason}");
        for refused_too in [
            "cd /tmp && timeout 30 bash -c \"gh api graphql -f query='mutation { updateProjectV2Field(input: {}) { x } }'\"",
            "gh api graphql -f query=\"$(cat <<'EOF'\nmutation { updateProjectV2Field(input: {}) { x } }\nEOF\n)\"",
            "gh api graphql -F query=@- <<'EOF'\nmutation { updateProjectV2Field(input: {}) { x } }\nEOF",
            "cat <<'EOF' | gh api graphql -F query=@-\nmutation { updateProjectV2Field }\nEOF",
        ] {
            assert!(at(refused_too).is_some(), "{refused_too}");
        }
        for passes in [
            "cat > notes.md <<'EOF'\nnever run gh api graphql with updateProjectV2Field\nEOF",
            "echo 'gh api graphql updateProjectV2Field'",
            "git commit -m 'gh api graphql updateProjectV2Field broke the board'",
            "gh api graphql -f query='query { viewer { login } }'",
            "gh issue list",
            "ls",
        ] {
            assert_eq!(at(passes), None, "{passes}");
        }
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn a_cue_guards_its_project_s_folder_or_its_own_and_the_default_board_s_the_whole_machine() {
        let home = home("scope");
        let (session, _, _) = test_sessions();
        let project = home.join("work/p");
        let other = home.join("other");
        std::fs::create_dir_all(project.join("sub")).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        crate::project::init(&home, &project, None, Some("p"), 0).unwrap();
        let (project, other) = (std::fs::canonicalize(&project).unwrap(), std::fs::canonicalize(&other).unwrap());
        cued(&home.join(".ekko"), "anywhere", cue("gh", &["alpha"], None));
        cued(&project.join(".ekko"), "in the project", cue("cargo", &["fmt", "--all"], None));
        cued(&project.join(".ekko"), "in another folder", cue("make", &["deploy"], Some(&other)));
        let at = |command: &str, cwd: &Path| refused(&hook_reply(&home, &event(command, cwd, "t"), &session)).is_some();
        assert!(at("gh alpha", Path::new("/")) && at("gh alpha", &project), "the default board's cue guards everywhere");
        assert!(at("cargo fmt --all", &project) && at("cargo fmt --all", &project.join("sub")));
        assert!(!at("cargo fmt --all", &other), "a project's cue stays in its folder");
        assert!(at(&format!("cd {} && cargo fmt --all", project.display()), &other), "the folder the call runs in counts");
        assert!(at("make deploy", &other) && !at("make deploy", &project), "a cue's own folder wins");
        let reason = refused(&hook_reply(&home, &event("cargo fmt --all", &project, "t"), &session)).unwrap();
        assert!(reason.contains(&format!("run in {} or under it, this board's project", project.display())), "{reason}");
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn a_refused_call_goes_through_once_on_the_user_s_answer_and_for_nobody_else() {
        let home = home("once");
        let dir = home.join(".ekko");
        let (session, other, _) = test_sessions();
        cued(&dir, "The Status field lesson.", cue("gh", &["updateProjectV2Field"], None));
        let command = "gh api graphql -f query='mutation { updateProjectV2Field }'";
        let call = |cwd: &str, tool_use_id: &str, actor: &Actor| hook_reply(&home, &event(command, Path::new(cwd), tool_use_id), actor);
        let code = code_in(&refused(&call("/r", "t1", &session)).unwrap());

        let (asked, put) = ask(&ekko_at(&dir, Some(&session)), &home, json!({"text": "Posso rodar a mutação?", "allow": code})).unwrap();
        assert!(put.text.starts_with("Posso rodar a mutação?\n\nThe call refused (Bash), from /r:\n"), "{}", put.text);
        assert!(put.text.contains(command) && put.text.contains("The Status field lesson."), "{}", put.text);
        assert_eq!(put.options.iter().map(|option| option.label.as_str()).collect::<Vec<_>>(), [ALLOW_ONCE, KEEP_REFUSED]);
        assert!(refused(&call("/r", "t2", &session)).is_some(), "unanswered, it stays refused");

        let by_session = answer(&dir, &session, asked, ALLOW_ONCE).unwrap_err().to_string();
        assert!(by_session.contains("is the user's to answer, in ekko's menu"), "{by_session}");
        answer(&dir, &Actor::person(), asked, ALLOW_ONCE).unwrap();

        assert!(refused(&call("/r", "t3", &other)).is_some(), "another session's same call");
        assert!(refused(&call("/elsewhere", "t3", &session)).is_some(), "the same call from another folder");
        let through = call("/r", "t4", &session).unwrap();
        let context = through["hookSpecificOutput"]["additionalContext"].as_str().unwrap();
        assert!(context.contains(&format!("question {asked}")) && through["hookSpecificOutput"].get("permissionDecision").is_none(), "{through}");
        assert_eq!(refuse_reply(&home, &event(command, Path::new("/r"), "t4"), "ctx: it would lose work", &session), Some(None), "another guard on the same call");
        assert!(refused(&call("/r", "t5", &session)).is_some(), "once: the next call is refused again");
        let allow = ekko_at(&dir, None).storage.get().unwrap()[&asked].question.clone().unwrap().allow.unwrap();
        assert_eq!(allow.used.unwrap().tool_use_id, "t4");
        std::fs::remove_dir_all(&home).ok();
    }

    /// When ekko's cue and another guard refuse one call, the model reads
    /// only the reason of the last to finish refusing (task 839), and each
    /// guard's text is made as it records. ctx's carries ekko's cues'
    /// reasons in either order, since `--refuse` works them out; ekko's
    /// carries ctx's once ctx has recorded it. Left open: ekko records
    /// first yet finishes last, in the moment between its record and its
    /// exit.
    #[test]
    fn two_guards_refusing_one_call_each_carry_both_reasons() {
        let (session, _, _) = test_sessions();
        let command = "gh api graphql -f query='mutation { updateProjectV2Field }'";
        let (lesson, ctx) = ("The Status field lesson.", "ctx: it would lose work");
        for ekko_first in [true, false] {
            let home = home(if ekko_first { "both-ekko-first" } else { "both-ctx-first" });
            cued(&home.join(".ekko"), lesson, cue("gh", &["updateProjectV2Field"], None));
            let input = event(command, Path::new("/r"), "t1");
            // As ctx refuses: its reason, then what ekko told it to end with.
            let from_ctx = || format!("{ctx}\n\n{}", refuse_reply(&home, &input, ctx, &session).unwrap().unwrap());
            let from_ekko = || refused(&hook_reply(&home, &input, &session)).unwrap();
            let (ekko_text, ctx_text) = if ekko_first {
                let ekko_text = from_ekko();
                (ekko_text, from_ctx())
            } else {
                let ctx_text = from_ctx();
                (from_ekko(), ctx_text)
            };
            let order = if ekko_first { "ekko records first" } else { "ctx records first" };
            let both = |text: &str| (text.matches(lesson).count(), text.matches(ctx).count());
            assert_eq!(both(&ctx_text), (1, 1), "{order}, ctx's: {ctx_text}");
            if !ekko_first {
                assert_eq!(both(&ekko_text), (1, 1), "{order}, ekko's: {ekko_text}");
            }
            assert_eq!(code_in(&ekko_text), code_in(&ctx_text), "{order}");
            std::fs::remove_dir_all(&home).ok();
        }
    }

    #[test]
    fn another_guard_s_refusal_is_asked_about_and_let_through_the_same_way() {
        let home = home("ctx");
        let dir = home.join(".ekko");
        let (session, _, _) = test_sessions();
        let read = |tool_use_id: &str| {
            json!({"tool_name": "Read", "tool_input": {"file_path": "/persist/secrets/key"}, "cwd": "/r", "tool_use_id": tool_use_id}).to_string()
        };
        let sentence = refuse_reply(&home, &read("t1"), "ctx: it would bring secret material into the context", &session).unwrap().unwrap();
        let code = code_in(&sentence);
        assert!(refuse_reply(&home, &read("t2"), "ctx: it would bring secret material into the context", &session).unwrap().is_some());
        let (asked, put) = ask(&ekko_at(&dir, Some(&session)), &home, json!({"text": "Leio a chave?", "allow": code})).unwrap();
        assert!(put.text.contains("The call refused (Read), from /r:\n{\"file_path\":\"/persist/secrets/key\"}"), "{}", put.text);
        assert!(put.text.contains("- ctx: it would bring secret material into the context"), "{}", put.text);
        answer(&dir, &Actor::person(), asked, ALLOW_ONCE).unwrap();
        assert_eq!(refuse_reply(&home, &read("t3"), "ctx: again", &session), Some(None));
        assert_eq!(refuse_reply(&home, "not json", "ctx", &session), None, "unreadable input: the other guard refuses as it would have");
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn both_guards_of_one_call_spend_its_allowance_and_no_other_call_can() {
        // Claude Code runs a call's hooks at once: each may find the
        // allowance unspent in the index and spend it, one after the other.
        let home = home("spend");
        let dir = home.join(".ekko");
        let (session, _, _) = test_sessions();
        let read = json!({"tool_name": "Read", "tool_input": {"file_path": "/k"}, "cwd": "/r", "tool_use_id": "t1"}).to_string();
        let code = code_in(&refuse_reply(&home, &read, "ctx: a secret", &session).unwrap().unwrap());
        let (asked, _) = ask(&ekko_at(&dir, Some(&session)), &home, json!({"text": "Leio?", "allow": code})).unwrap();
        answer(&dir, &Actor::person(), asked, ALLOW_ONCE).unwrap();
        let grant = current(&home, chrono::Local::now().timestamp_millis()).granted.into_iter().find(|grant| grant.id == asked).unwrap();
        assert!(grant.used.is_none());
        assert!(spend(&home, &grant, "t2", &session), "the first guard of the call");
        assert!(spend(&home, &grant, "t2", &session), "the second guard of the same call, from the same stale index");
        assert!(!spend(&home, &grant, "t3", &session), "another call");
        assert!(!spend(&home, &grant, "", &session), "a call with no id");
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn the_refusals_lock_outlives_the_old_records_pruned_around_it() {
        let home = home("prune");
        let dir = refused_dir(&home);
        std::fs::create_dir_all(&dir).unwrap();
        let two_days_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 86_400);
        for name in [".lock", "zzzzzz.json", ".zzzzzz.json.7"] {
            std::fs::write(dir.join(name), b"{}").unwrap();
            std::fs::File::options().write(true).open(dir.join(name)).unwrap().set_modified(two_days_ago).unwrap();
        }
        let seen = Seen { tool: "Bash".into(), call: "ls".into(), cwd: "/".into(), tool_use_id: "t".into() };
        record(&home, &seen, None, "abcdef", &["why".to_string()], chrono::Local::now().timestamp_millis()).unwrap();
        let mut left: Vec<String> =
            std::fs::read_dir(&dir).unwrap().flatten().map(|entry| entry.file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, [".lock", "abcdef.json"], "deleting a held lock would let the next writer take a second one");
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn only_the_user_s_answer_turns_a_cue_on_changes_it_or_drops_it() {
        let home = home("proposal");
        let dir = home.join(".ekko");
        let (session, _, _) = test_sessions();
        let as_session = ekko_at(&dir, Some(&session));
        let gotcha = apply(&as_session, json!({"op": "create", "kind": "gotcha", "text": "cargo fmt --all reformats unrelated files"})).unwrap()[0];
        let propose = |spec: Value| ask(&as_session, &home, json!({"text": "Ligo a trava?", "cue": spec}));
        let on = || cues_on(&ekko_at(&dir, None).storage.get().unwrap()).into_values().collect::<Vec<_>>();

        let (asked, put) = propose(json!({"gotcha": gotcha, "command": "cargo", "words": ["fmt", "--all"], "folder": "/projects/winwayland/"})).unwrap();
        assert!(put.text.contains(&format!("The cue proposed for gotcha {gotcha}: refuse `cargo` calls holding `fmt` and `--all`, run in /projects/winwayland or under it")), "{}", put.text);
        assert_eq!(put.options.iter().map(|option| option.label.as_str()).collect::<Vec<_>>(), [TURN_ON, LEAVE_OFF]);
        assert!(on().is_empty(), "a proposal stays off");
        assert!(answer(&dir, &session, asked, TURN_ON).is_err());
        answer(&dir, &Actor::person(), asked, TURN_ON).unwrap();
        let winwayland = cue("cargo", &["fmt", "--all"], Some(Path::new("/projects/winwayland")));
        assert_eq!(on(), vec![winwayland.clone()]);

        let (change, _) = propose(json!({"gotcha": gotcha, "command": "cargo", "words": ["fmt"]})).unwrap();
        answer(&dir, &Actor::person(), change, LEAVE_OFF).unwrap();
        assert_eq!(on(), vec![winwayland], "another answer changes nothing");
        let (change, _) = propose(json!({"gotcha": gotcha, "command": "cargo", "words": ["fmt"]})).unwrap();
        answer(&dir, &Actor::person(), change, TURN_ON).unwrap();
        assert_eq!(on(), vec![cue("cargo", &["fmt"], None)]);

        let (off, put) = propose(json!({"gotcha": gotcha, "off": true})).unwrap();
        assert_eq!(put.options.iter().map(|option| option.label.as_str()).collect::<Vec<_>>(), [TURN_OFF, KEEP_ON]);
        answer(&dir, &Actor::person(), off, TURN_OFF).unwrap();
        assert!(on().is_empty());
        std::fs::remove_dir_all(&home).ok();
    }

    /// The class: every write that would turn a cue off or back on -- the
    /// trash and back, the stash and back, another kind and back -- is
    /// refused to a session and left to the user, while a write that leaves
    /// the cues as they were goes through.
    #[test]
    fn a_session_s_write_never_changes_the_cues_that_are_on() {
        let home = home("class");
        let dir = home.join(".ekko");
        let (session, _, _) = test_sessions();
        let gotcha = cued(&dir, "a lesson", cue("gh", &["x"], None));
        let plain = apply(&ekko_at(&dir, None), json!({"op": "create", "kind": "note", "text": "plain"})).unwrap()[0];
        let (as_session, person) = (ekko_at(&dir, Some(&session)), ekko_at(&dir, Some(&Actor::person())));
        let id = [gotcha.to_string()];
        let count = || cues_on(&ekko_at(&dir, None).storage.get().unwrap()).len();
        let refused = |what: &str, result: Result<(), EkkoError>| {
            assert_eq!(result.map_err(|error| error.code()), Err("CUE_IS_USERS"), "{what}");
        };

        refused("trash", as_session.set_trashed(&id, true).map(|_| ()));
        refused("stash", as_session.set_stashed(&id, true).map(|_| ()));
        refused("another kind", apply(&as_session, json!({"op": "update", "item": gotcha, "kind": "decision"})).map(|_| ()));
        assert_eq!(count(), 1, "nothing was written");
        apply(&as_session, json!({"op": "edit", "item": gotcha, "append": " Seen again."})).unwrap();
        apply(&as_session, json!({"op": "update", "item": plain, "kind": "gotcha"})).unwrap();

        for (away, back) in [("trash", "untrash"), ("stash", "unstash"), ("another kind", "a gotcha again")] {
            match away {
                "trash" => person.set_trashed(&id, true).map(|_| ()),
                "stash" => person.set_stashed(&id, true).map(|_| ()),
                _ => apply(&person, json!({"op": "update", "item": gotcha, "kind": "decision"})).map(|_| ()),
            }
            .unwrap();
            assert_eq!(count(), 0, "the user took it off: {away}");
            let brought = match back {
                "untrash" => as_session.set_trashed(&id, false).map(|_| ()),
                "unstash" => as_session.set_stashed(&id, false).map(|_| ()),
                _ => apply(&as_session, json!({"op": "update", "item": gotcha, "kind": "gotcha"})).map(|_| ()),
            };
            refused(back, brought);
            match back {
                "untrash" => person.set_trashed(&id, false).map(|_| ()),
                "unstash" => person.set_stashed(&id, false).map(|_| ()),
                _ => apply(&person, json!({"op": "update", "item": gotcha, "kind": "gotcha"})).map(|_| ()),
            }
            .unwrap();
            assert_eq!(count(), 1, "the user brought it back: {back}");
        }
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn a_question_about_a_cue_or_a_refused_call_is_checked_before_it_is_asked() {
        let home = home("checked");
        let dir = home.join(".ekko");
        let (session, other, _) = test_sessions();
        let as_session = ekko_at(&dir, Some(&session));
        let gotcha = cued(&dir, "a lesson", cue("gh", &["x"], None));
        let note = apply(&as_session, json!({"op": "create", "kind": "note", "text": "not a gotcha"})).unwrap()[0];
        let code = code_in(&refused(&hook_reply(&home, &event("gh x", Path::new("/"), "t"), &session)).unwrap());
        let fails = |question: Value, why: &str| {
            let error = ask(&as_session, &home, question.clone()).unwrap_err().to_string();
            assert!(error.contains(why), "{question}: {error}");
        };
        fails(json!({"text": "?", "allow": "zzzzzz"}), "names no call a guard refused in the last day");
        fails(json!({"text": "?", "allow": code, "options": [{"label": "a"}, {"label": "b"}]}), "takes no options");
        fails(json!({"text": "?", "allow": code, "cue": {"gotcha": gotcha, "off": true}}), "cue or allow, not both");
        fails(json!({"text": "?", "cue": {"gotcha": note, "command": "gh"}}), "is not a gotcha");
        fails(json!({"text": "?", "cue": {"gotcha": gotcha, "command": "/usr/bin/gh"}}), "no directory");
        fails(json!({"text": "?", "cue": {"gotcha": gotcha, "command": "gh", "folder": "relative/path"}}), "absolute");
        fails(json!({"text": "?", "cue": {"gotcha": gotcha, "command": "gh", "words": ["x"]}}), "that one already");
        fails(json!({"text": "?", "cue": {"gotcha": gotcha, "off": true, "command": "gh"}}), "takes no command");
        let error = ask(&ekko_at(&dir, Some(&other)), &home, json!({"text": "?", "allow": code})).unwrap_err().to_string();
        assert!(error.contains("another session made"), "{error}");
        std::fs::remove_dir_all(&home).ok();
    }

    /// A cue is proposed only on a board the guard reads (task 827): on one
    /// opened through EKKO_DIR it would never refuse, and the user would turn
    /// on a guard that guards nothing.
    #[test]
    fn a_cue_is_proposed_only_on_a_board_the_guard_reads() {
        let home = home("unread");
        let (session, _, _) = test_sessions();
        let elsewhere = home.join("hometasks");
        std::fs::create_dir_all(&elsewhere).unwrap();
        for (dir, read) in [(home.join(".ekko"), true), (elsewhere, false)] {
            let as_session = ekko_at(&dir, Some(&session));
            let gotcha = apply(&as_session, json!({"op": "create", "kind": "gotcha", "text": "a lesson"})).unwrap()[0];
            let asked = ask(&as_session, &home, json!({"text": "?", "cue": {"gotcha": gotcha, "command": "gh"}}));
            if read {
                assert!(asked.is_ok(), "{}: {:?}", dir.display(), asked.err());
            } else {
                let error = asked.unwrap_err().to_string();
                assert!(error.contains("does not read this board") && error.contains("never refuse"), "{error}");
            }
        }
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn the_index_is_read_back_until_a_board_changes() {
        let home = home("index");
        let dir = home.join(".ekko");
        cued(&dir, "one", cue("gh", &["x"], None));
        let now = chrono::Local::now().timestamp_millis();
        assert_eq!(current(&home, now).cues.len(), 1);
        // What the next call reads is the index kept: altered, it shows.
        let file = state(&home).join("index.json");
        let mut kept: Index = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        kept.cues[0].text = "from the index".into();
        std::fs::write(&file, serde_json::to_vec(&kept).unwrap()).unwrap();
        assert_eq!(current(&home, now).cues[0].text, "from the index", "no board moved");
        cued(&dir, "two", cue("cargo", &[], None));
        let texts: Vec<String> = current(&home, now).cues.into_iter().map(|cue| cue.text).collect();
        assert_eq!(texts, ["one", "two"], "a board moved: read again");
        std::fs::remove_file(&file).unwrap();
        assert_eq!(current(&home, now).cues.len(), 2, "a lost index is rebuilt");
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn a_gotcha_whose_cue_is_on_says_so_in_the_prime_and_in_context() {
        let home = home("views");
        let dir = home.join(".ekko");
        let gotcha = cued(&dir, "Never run cargo fmt --all in winwayland.", cue("cargo", &["fmt", "--all"], Some(Path::new("/projects/winwayland"))));
        let plain = apply(&ekko_at(&dir, None), json!({"op": "create", "kind": "gotcha", "text": "A gotcha with no cue."})).unwrap()[0];
        let ekko = ekko_at(&dir, None);
        let prime = crate::agent::prime(&ekko, "default board").unwrap().text();
        assert!(prime.contains(&format!("{gotcha:>4}. [gotcha, cue on] Never run cargo fmt")), "{prime}");
        assert!(prime.contains(&format!("{plain:>4}. [gotcha] A gotcha with no cue.")), "{prime}");
        let read = crate::agent::context(&ekko, &gotcha.to_string()).unwrap().text();
        assert!(read.contains("cue on: refuses `cargo` calls holding `fmt` and `--all`, run in /projects/winwayland or under it"), "{read}");
        assert!(!crate::agent::context(&ekko, &plain.to_string()).unwrap().text().contains("cue on"));
        std::fs::remove_dir_all(&home).ok();
    }

    /// A refusal's record keeps a later version's fields (task 829): the
    /// hook a session started with and the ekko ctx runs can be two
    /// versions, and each rewrites the record to add its reason.
    #[test]
    fn a_refusal_s_record_keeps_what_it_does_not_know() {
        crate::json::assert_keeps_what_it_does_not_know::<Refused>(serde_json::json!({
            "code": "bv762s", "at": 1790488528484_i64, "tool": "Bash", "call": "git reset --hard", "cwd": "/tmp",
            "session": [1047920, 10288721, "016877cc"], "reasons": ["a gotcha", "ctx's work-loss guard"]
        }));
    }
}
