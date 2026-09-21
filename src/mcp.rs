//! `ekko --mcp`: the board served to an agent over the Model Context Protocol.
//!
//! The CLI made an agent pay twice: once to learn the command shapes from a
//! skill loaded into every conversation, and again on every call, re-reading
//! pretty output meant for a person. Over MCP the tools describe themselves,
//! a client loads their definitions only when it needs them, and every answer
//! is the agent view rather than the board view.
//!
//! Newline-delimited JSON-RPC over stdin and stdout, answered one request at a
//! time. Dual-era, per the 2026-07-28 revision: a request whose `_meta` names
//! a protocol version is served statelessly under that revision, and an
//! `initialize` handshake is served under the legacy revision it negotiates.
//! Written by hand rather than through an SDK because the whole protocol a
//! tools-only server needs is five methods, and the binary stays one file on
//! disk with no runtime.
//!
//! Logs, if any, go to stderr: stdout carries protocol messages and nothing
//! else. The server exits when stdin closes.

use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use serde_json::{json, Map, Value};

use crate::agent;
use crate::config;
use crate::directory;
use crate::ekko::{Ekko, EkkoError, Outcome};
use crate::ops::{self, Committed, Draft, Op};
use crate::render::{Painter, Renderer};
use crate::storage::ItemMap;

const MODERN: &[&str] = &["2026-07-28"];
/// Newest first: an `initialize` asking for a version not listed here is
/// answered with the first, which the client may accept or disconnect over.
const LEGACY: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

const TOOLS: &[&str] = &[
    "prime", "next", "context", "search", "changes", "roadmap", "projects", "create", "set_state",
    "force_state", "edit", "update", "link", "batch", "stash", "trash", "away", "phases",
];

const INSTRUCTIONS: &str = "\
Ekko is a task board shared with the user: they read and change the same board from their own terminal, between your calls. Do not treat it as yours.

Each session starts with the board's prime already in context -- in progress, ready in order, blocked, and the notes that explain them. Call prime again after a long pause or when the user may have changed things -- with if_rev set to the cursor you hold, it answers in one line when nothing moved -- and changes with that cursor lists what did.

- next is the order to take work up. context gives items, several per call: blockers and the roots free to start, what they block, notes clipped unless detail is full.
- Display ids are never reused, but a restore from the archive renumbers an item: hold the uid a write returns to follow it.
- set_state is idempotent. A task blocked by open work cannot be completed (BLOCKED), and a task that completed work depends on cannot be reopened (COMPLETED_DEPENDENTS): finish the other side, or clear a wrong dependency with link. force_state overrides the rule and is only for when the user has said so.
- Leave reasoning on the board: create a note with attached_to set to the task it explains. Change text with edit's replace or append instead of resending it, with if_updated_at from your last read when the user may have edited it.
- A write's reply names the tasks it set free (nowReady) or left waiting (nowBlocked): no next or prime is needed to find them.
- batch applies several operations in one write, all or nothing; $1, $2 name the items created by the batch's first and second operations.
- trash is recoverable for 30 days and still needs the user's consent. For work decided against, set_state cancelled keeps the record.
- Refusals come back as CODE: message. Branch on the code; nothing was written.";

/// Where calls find their board: the folder the server was started in, and
/// its environment -- the same way `ekko` typed in that folder would.
pub struct Server {
    home: PathBuf,
    cwd: PathBuf,
    ekko_dir_env: Option<String>,
    project_env: Option<String>,
}

struct RpcError {
    code: i64,
    message: String,
    data: Option<Value>,
}

impl RpcError {
    fn new(code: i64, message: impl Into<String>) -> Self {
        RpcError { code, message: message.into(), data: None }
    }
}

/// A tool call that did not succeed, as the model reads it: the ekko code it
/// can branch on, and the message saying what to do about it.
struct ToolError {
    code: &'static str,
    message: String,
}

impl From<EkkoError> for ToolError {
    fn from(error: EkkoError) -> Self {
        ToolError { code: error.code(), message: agent_message(&error, &|id| id.to_string()) }
    }
}

/// A refusal of a draft, with its items named the way `ops::Names` names them.
fn refusal(error: &EkkoError, names: &ops::Names) -> ToolError {
    ToolError { code: error.code(), message: agent_message(error, &|id| names.name(id)) }
}

fn invalid(message: impl Into<String>) -> ToolError {
    ToolError { code: "INVALID_INPUT", message: message.into() }
}

pub fn run(home: PathBuf, cwd: PathBuf, ekko_dir_env: Option<String>, project_env: Option<String>) -> ExitCode {
    let server = Server::new(home, cwd, ekko_dir_env, project_env);
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        if let Some(reply) = server.handle_line(&line) {
            if writeln!(stdout, "{reply}").and_then(|()| stdout.flush()).is_err() {
                break;
            }
        }
    }
    ExitCode::SUCCESS
}

impl Server {
    pub fn new(home: PathBuf, cwd: PathBuf, ekko_dir_env: Option<String>, project_env: Option<String>) -> Self {
        Server { home, cwd, ekko_dir_env, project_env }
    }

    /// One incoming line to at most one reply: notifications and stray
    /// responses get none.
    pub fn handle_line(&self, line: &str) -> Option<Value> {
        let message: Value = match serde_json::from_str(line) {
            Ok(message) => message,
            Err(error) => return Some(error_reply(Value::Null, RpcError::new(-32700, format!("Parse error: {error}")))),
        };
        let id = message.get("id").cloned();
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            return id.map(|id| error_reply(id, RpcError::new(-32600, "Invalid request: no method")));
        };
        let id = id?;
        let params = message.get("params").cloned().unwrap_or_else(|| json!({}));
        Some(match self.handle(method, &params) {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err(error) => error_reply(id, error),
        })
    }

    fn handle(&self, method: &str, params: &Value) -> Result<Value, RpcError> {
        let meta = params.get("_meta");
        let version = meta.and_then(|m| m.get("io.modelcontextprotocol/protocolVersion")).and_then(Value::as_str);
        if let Some(requested) = version {
            if !MODERN.contains(&requested) {
                return Err(RpcError {
                    code: -32022,
                    message: "Unsupported protocol version".to_string(),
                    data: Some(json!({"supported": supported(), "requested": requested})),
                });
            }
            if meta.and_then(|m| m.get("io.modelcontextprotocol/clientCapabilities")).is_none() {
                return Err(RpcError::new(-32602, "Invalid params: _meta lacks io.modelcontextprotocol/clientCapabilities"));
            }
        }

        let result = match method {
            "initialize" => return Ok(self.initialize(params)),
            "server/discover" => json!({
                "supportedVersions": supported(),
                "capabilities": {"tools": {}},
                "instructions": INSTRUCTIONS,
            }),
            "ping" => json!({}),
            "tools/list" => json!({"tools": tool_definitions()}),
            "tools/call" => self.call(params)?,
            _ => return Err(RpcError::new(-32601, format!("Method not found: {method}"))),
        };
        Ok(if version.is_some() || method == "server/discover" { complete(result) } else { result })
    }

    fn initialize(&self, params: &Value) -> Value {
        let requested = params.get("protocolVersion").and_then(Value::as_str).unwrap_or_default();
        let version = LEGACY.iter().find(|v| **v == requested).copied().unwrap_or(LEGACY[0]);
        json!({
            "protocolVersion": version,
            "capabilities": {"tools": {}},
            "serverInfo": server_info(),
            "instructions": INSTRUCTIONS,
        })
    }

    fn call(&self, params: &Value) -> Result<Value, RpcError> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError::new(-32602, "Invalid params: tools/call needs a tool name"))?;
        if !TOOLS.contains(&name) {
            return Err(RpcError::new(-32602, format!("Unknown tool: {name}")));
        }
        let mut args = match params.get("arguments") {
            None | Some(Value::Null) => Map::new(),
            Some(Value::Object(map)) => map.clone(),
            Some(_) => return Err(RpcError::new(-32602, "Invalid params: arguments must be an object")),
        };
        Ok(match self.tool(name, &mut args) {
            Ok(text) => json!({"content": [{"type": "text", "text": text}], "isError": false}),
            Err(error) => json!({
                "content": [{"type": "text", "text": format!("{}: {}", error.code, error.message)}],
                "isError": true,
            }),
        })
    }

    /// The board one call works on, resolved afresh each time: the same
    /// session start `--prime` ran from, and a project made with `ekko init`
    /// mid-session is the one the next call sees.
    fn open(&self, project: Option<&str>) -> Result<(Ekko, directory::Location), EkkoError> {
        let name = project.or(self.project_env.as_deref());
        let location = directory::locate(&self.home, &self.cwd, None, self.ekko_dir_env.as_deref(), name)?;
        Ok((Ekko::at(&location.dir)?, location))
    }

    fn label(location: &directory::Location) -> String {
        match (&location.project, location.discovered) {
            (Some(project), true) => format!("project {}, found from this folder", project.name),
            (Some(project), false) => format!("project {}", project.name),
            (None, _) => "default board".to_string(),
        }
    }

    fn tool(&self, name: &str, args: &mut Map<String, Value>) -> Result<String, ToolError> {
        if name == "projects" {
            finish(args)?;
            return Ok(agent::projects_text(&crate::project::list(&self.home)));
        }
        let project = match args.remove("project") {
            None | Some(Value::Null) => None,
            Some(Value::String(name)) => Some(name),
            Some(_) => return Err(invalid("project must be a string")),
        };
        let (ekko, location) = self.open(project.as_deref())?;

        match name {
            "prime" => {
                let if_rev = take(args, "if_rev", Value::as_i64, "an integer")?;
                finish(args)?;
                if let Some(line) = unchanged(&ekko, if_rev)? {
                    return Ok(line);
                }
                Ok(agent::prime(&ekko, &Self::label(&location))?.text())
            }
            "next" => {
                let limit = positive(take(args, "limit", Value::as_u64, "a positive integer")?)?;
                let if_rev = take(args, "if_rev", Value::as_i64, "an integer")?;
                finish(args)?;
                if let Some(line) = unchanged(&ekko, if_rev)? {
                    return Ok(line);
                }
                let (entries, total) = agent::next_listed(&ekko, Some(limit.unwrap_or(agent::SEARCH_LIMIT)))?;
                let mut text = agent::list_text(&entries, "Nothing is in progress or ready.");
                if total > entries.len() {
                    text.push_str(&format!("{} of {total} shown: raise limit for more.\n", entries.len()));
                }
                Ok(text)
            }
            "context" => {
                let item = take_item(args, "item")?;
                let items = take_item_list(args, "items")?;
                let detail = match take(args, "detail", |v| v.as_str().map(str::to_string), "concise or full")?.as_deref() {
                    None | Some("concise") => agent::Detail::Concise,
                    Some("full") => agent::Detail::Full,
                    Some(other) => return Err(invalid(format!("detail must be concise or full, got {other}"))),
                };
                finish(args)?;
                let targets = match (item, items) {
                    (Some(item), None) => vec![item],
                    (None, Some(items)) if items.len() <= CONTEXTS_AT_ONCE => items,
                    (None, Some(_)) => {
                        return Err(invalid(format!("context reads at most {CONTEXTS_AT_ONCE} items at once")))
                    }
                    _ => return Err(invalid("context takes exactly one of item or items")),
                };
                let read = agent::contexts(&ekko, &targets)?;
                Ok(read.iter().map(|context| context.text_with(detail)).collect::<Vec<_>>().join("\n"))
            }
            "search" => {
                let text = take(args, "text", |v| v.as_str().map(str::to_string), "a string")?;
                let filters = take_strings(args, "filters")?;
                let limit = positive(take(args, "limit", Value::as_u64, "a positive integer")?)?;
                finish(args)?;
                Ok(agent::search(&ekko, text.as_deref(), &filters, limit.unwrap_or(agent::SEARCH_LIMIT))?.text())
            }
            "changes" => {
                let since = take(args, "since", Value::as_i64, "an integer")?.ok_or_else(|| invalid("changes needs since"))?;
                finish(args)?;
                Ok(agent::changes(&ekko, since)?.text())
            }
            "roadmap" => {
                finish(args)?;
                Ok(agent::roadmap_text(&ekko.display_roadmap()?))
            }
            "create" => {
                let spec: ops::Create = parse(args)?;
                write(&ekko, false, |draft| draft.create(&spec).map(|id| vec![id]))
            }
            "set_state" | "force_state" => {
                let spec: ops::SetState = parse(args)?;
                write(&ekko, name == "force_state", |draft| draft.set_state(&spec.items, &spec.state))
            }
            "edit" => {
                let spec: ops::Edit = parse(args)?;
                write(&ekko, false, |draft| draft.edit(&spec))
            }
            "update" => {
                let spec: ops::Update = parse(args)?;
                write(&ekko, false, |draft| draft.update(&spec))
            }
            "link" => {
                let spec: ops::Link = parse(args)?;
                write(&ekko, false, |draft| draft.link(&spec))
            }
            "batch" => {
                let ops = args.remove("ops").ok_or_else(|| invalid("batch needs ops"))?;
                finish(args)?;
                batch(&ekko, ops)
            }
            "away" => {
                let which = take(args, "which", |v| v.as_str().map(str::to_string), "stash or trash")?;
                let limit = positive(take(args, "limit", Value::as_u64, "a positive integer")?)?;
                finish(args)?;
                let (stash, trash) = match which.as_deref() {
                    None => (true, true),
                    Some("stash") => (true, false),
                    Some("trash") => (false, true),
                    Some(other) => return Err(invalid(format!("which is stash or trash, not {other}"))),
                };
                Ok(agent::away(&ekko, stash, trash, limit.unwrap_or(agent::SEARCH_LIMIT))?)
            }
            "phases" => {
                let sequence = take_strings(args, "sequence")?;
                finish(args)?;
                if sequence.is_empty() {
                    return Err(invalid("sequence is required: every phase, in the order work goes through them"));
                }
                ekko.set_phases(&sequence)?;
                Ok(agent::roadmap_text(&ekko.display_roadmap()?))
            }
            "stash" | "trash" => {
                let items = take_items(args)?;
                let away = take(args, "away", Value::as_bool, "true or false")?.unwrap_or(true);
                finish(args)?;
                let outcome =
                    if name == "stash" { ekko.set_stashed(&items, away)? } else { ekko.set_trashed(&items, away)? };
                Ok(render(&self.home, &outcome))
            }
            _ => unreachable!("tool names are checked against TOOLS"),
        }
    }
}

/// Applies operations in order to one draft and writes it once. A refusal
/// names the operation it came from; nothing of the batch is written.
fn batch(ekko: &Ekko, ops: Value) -> Result<String, ToolError> {
    let Value::Array(values) = ops else { return Err(invalid("ops must be an array of operations")) };
    if values.is_empty() {
        return Err(invalid("ops is empty"));
    }
    let mut parsed = Vec::with_capacity(values.len());
    for (at, value) in values.into_iter().enumerate() {
        let op: Op = serde_json::from_value(value)
            .map_err(|e| invalid(format!("operation {}: {e}; nothing in the batch was written", at + 1)))?;
        parsed.push(op);
    }

    let mut draft = Draft::open(ekko)?;
    let mut touched = Vec::new();
    for (at, op) in parsed.iter().enumerate() {
        match draft.apply(op) {
            Ok(ids) => touched.push(ids),
            Err(error) => {
                let refused = refusal(&error, &draft.names());
                return Err(ToolError {
                    code: refused.code,
                    message: format!(
                        "operation {} was refused, and nothing in the batch was written: {}",
                        at + 1,
                        refused.message
                    ),
                });
            }
        }
    }
    let names = draft.names();
    let committed = draft.commit(false).map_err(|error| {
        let refused = refusal(&error, &names);
        ToolError {
            code: refused.code,
            message: format!("the batch as a whole was refused, and nothing was written: {}", refused.message),
        }
    })?;
    let results: Vec<Value> = touched
        .iter()
        .map(|ids| Value::Array(ids.iter().map(|id| ops::written(&committed.data, *id)).collect()))
        .collect();
    let mut reply = json!({"ok": true, "results": results});
    name_readiness(&mut reply, &committed);
    Ok(reply.to_string())
}

/// How much of a released or blocked task's text a write reply quotes.
const READINESS_CLIP: usize = 80;

/// How many items one context call reads.
const CONTEXTS_AT_ONCE: usize = 20;

/// Puts the tasks a commit set free or left waiting on its reply, when it
/// changed any, each named well enough to act on without reading the board
/// again: the id, the uid to hold, and the start of the text.
fn name_readiness(reply: &mut Value, committed: &Committed) {
    let named = |data: &ItemMap, ids: &[u32]| {
        Value::Array(
            ids.iter()
                .filter_map(|id| data.get(id))
                .map(|item| {
                    json!({"id": item.id, "uid": item.uid, "text": agent::clip(&item.description, READINESS_CLIP)})
                })
                .collect(),
        )
    };
    if !committed.released.is_empty() {
        reply["nowReady"] = named(&committed.data, &committed.released);
    }
    if !committed.blocked.is_empty() {
        reply["nowBlocked"] = named(&committed.data, &committed.blocked);
    }
}

fn write(
    ekko: &Ekko,
    force: bool,
    apply: impl FnOnce(&mut Draft) -> Result<Vec<u32>, EkkoError>,
) -> Result<String, ToolError> {
    let mut draft = Draft::open(ekko)?;
    let ids = match apply(&mut draft) {
        Ok(ids) => ids,
        Err(error) => return Err(refusal(&error, &draft.names())),
    };
    let names = draft.names();
    let committed = draft.commit(force).map_err(|error| refusal(&error, &names))?;
    let items: Vec<Value> = ids.iter().map(|id| ops::written(&committed.data, *id)).collect();
    let mut reply = json!({"ok": true, "items": items});
    let pairs = |pairs: &[(u32, Vec<u32>)], key: &str| {
        Value::Array(pairs.iter().map(|(id, others)| json!({"id": id, key: others})).collect())
    };
    if !committed.overridden.is_empty() {
        reply["overridden"] = pairs(&committed.overridden, "blockers");
    }
    if !committed.reopened.is_empty() {
        reply["reopenedOver"] = pairs(&committed.reopened, "dependents");
    }
    name_readiness(&mut reply, &committed);
    Ok(reply.to_string())
}

/// A refusal worded for an agent.
///
/// The terminal message names the CLI flags that resolve it -- `--blocked-by`,
/// `--force` -- which an agent over MCP does not have; this names the tools
/// instead, and leaves a command only where the step is the user's to take.
/// Every item is named through `name`, so one a refused write would have
/// created is spoken of by the operation that created it: the id it held in
/// the draft names nothing, and would name whatever takes that number next.
fn agent_message(error: &EkkoError, name: &dyn Fn(u32) -> String) -> String {
    let list = |ids: &[u32]| ids.iter().map(|id| name(*id)).collect::<Vec<_>>().join(", ");
    let verb = |ids: &[u32]| if ids.len() == 1 { "is" } else { "are" };
    match error {
        EkkoError::Blocked(found) => format!(
            "Cannot complete: {}, still open. Finish or cancel what blocks it, clear a dependency that is wrong with link, or use force_state if the user has said the dependency was dealt with",
            found
                .iter()
                .map(|(task, blockers)| format!("{} is blocked by {}", name(*task), list(blockers)))
                .collect::<Vec<_>>()
                .join("; "),
        ),
        EkkoError::CompletedDependents(found) => format!(
            "Cannot reopen: {}. Open again, it would be holding up work already done. Reopen that work first, clear a dependency that is wrong with link, or use force_state if the user has said so",
            found
                .iter()
                .map(|(task, done)| format!("completed {} {} blocked by {}", list(done), verb(done), name(*task)))
                .collect::<Vec<_>>()
                .join("; "),
        ),
        EkkoError::AlreadyDone(found) => format!(
            "Cannot record that dependency: {}, and completed work cannot wait on open work. Reopen it with set_state first, or finish or cancel what would block it",
            found
                .iter()
                .map(|(task, blockers)| {
                    format!("{} is already done while {} {} still open", name(*task), list(blockers), verb(blockers))
                })
                .collect::<Vec<_>>()
                .join("; "),
        ),
        EkkoError::PhaseOrder(inversion) => format!(
            "{} is in phase {} and cannot be blocked by {} in {}, which comes after it: a phase cannot wait on a later one. If the phase order itself is wrong, reordering it is for the user (ekko --phases)",
            name(inversion.blocked),
            inversion.blocked_phase,
            name(inversion.blocker),
            inversion.blocker_phase
        ),
        EkkoError::BlockingCycle(waiter, blocker) if waiter == blocker => {
            format!("{} cannot be blocked by itself", name(*waiter))
        }
        EkkoError::BlockingCycle(waiter, blocker) => format!(
            "{} cannot be blocked by {}: {} is already blocked by {}, directly or through other items, and the dependency would close a cycle",
            name(*waiter),
            name(*blocker),
            name(*blocker),
            name(*waiter)
        ),
        EkkoError::AttachNotANote(id) => {
            format!("Only a note can be attached, and {} is a task", name(*id))
        }
        EkkoError::AttachTargetNotATask(id) => {
            format!("A note is attached to a task, and {} is a note", name(*id))
        }
        EkkoError::AttachTargetHasNoUid(id) => {
            format!("{} predates uids, so nothing can point at it reliably", name(*id))
        }
        EkkoError::Stale { id, current } => format!(
            "{} changed since it was read (its updatedAt is now {current}), so the edit was not made. Read it again with context and redo the edit against what it says now",
            name(*id)
        ),
        EkkoError::EditMatch { id, found: 0 } => format!(
            "{} does not contain that text, so nothing was replaced. Read it with context and quote the text exactly",
            name(*id)
        ),
        EkkoError::EditMatch { id, found } => format!(
            "That text occurs {found} times in {}, so which one to replace is ambiguous and nothing was replaced. Quote enough of the surrounding text to make it unique",
            name(*id)
        ),
        EkkoError::InvalidDueDate(value) => {
            format!("due must be a date written YYYY-MM-DD, got: {}", value.trim_start_matches("d:"))
        }
        EkkoError::Directory(directory::DirectoryError::UnknownProject(project)) => format!(
            "No project named {project}; projects lists the ones that exist. A project is made by the user, with ekko init in its folder"
        ),
        EkkoError::Directory(directory::DirectoryError::MissingProjectName) => {
            "project is empty; leave it out to work on this session's board".to_string()
        }
        other => other.to_string(),
    }
}

fn parse<T: serde::de::DeserializeOwned>(args: &mut Map<String, Value>) -> Result<T, ToolError> {
    serde_json::from_value(Value::Object(std::mem::take(args))).map_err(|e| invalid(e.to_string()))
}

fn take<T>(
    args: &mut Map<String, Value>,
    key: &str,
    read: impl Fn(&Value) -> Option<T>,
    expected: &str,
) -> Result<Option<T>, ToolError> {
    match args.remove(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => read(&value).map(Some).ok_or_else(|| invalid(format!("{key} must be {expected}"))),
    }
}

fn take_item(args: &mut Map<String, Value>, key: &str) -> Result<Option<String>, ToolError> {
    take(args, key, item_text, "an id or a uid")
}

fn item_text(value: &Value) -> Option<String> {
    match value {
        Value::Number(n) => n.as_u64().map(|n| n.to_string()),
        Value::String(s) => Some(s.trim_start_matches('@').to_string()),
        _ => None,
    }
}

fn take_items(args: &mut Map<String, Value>) -> Result<Vec<String>, ToolError> {
    take_item_list(args, "items")?.ok_or_else(|| invalid("items is required"))
}

/// A non-empty array of ids or uids under `key`, or `None` when it is absent.
fn take_item_list(args: &mut Map<String, Value>, key: &str) -> Result<Option<Vec<String>>, ToolError> {
    let Some(values) = take(args, key, |v| v.as_array().cloned(), "an array of ids or uids")? else {
        return Ok(None);
    };
    match values.iter().map(item_text).collect::<Option<Vec<String>>>() {
        Some(items) if !items.is_empty() => Ok(Some(items)),
        _ => Err(invalid(format!("{key} must be a non-empty array of ids or uids"))),
    }
}

fn take_strings(args: &mut Map<String, Value>, key: &str) -> Result<Vec<String>, ToolError> {
    let values = take(args, key, |v| v.as_array().cloned(), "an array of strings")?.unwrap_or_default();
    values.iter().map(|v| v.as_str().map(str::to_string)).collect::<Option<_>>().ok_or_else(|| invalid(format!("{key} must be an array of strings")))
}

/// A read conditioned on the cursor a caller already holds: one line saying
/// nothing moved, when nothing did, in place of the whole answer again -- the
/// way an HTTP 304 answers a request that names the version it has.
fn unchanged(ekko: &Ekko, if_rev: Option<i64>) -> Result<Option<String>, ToolError> {
    let Some(held) = if_rev else { return Ok(None) };
    let revision = ekko.storage.get_counters().map_err(EkkoError::from)?.revision as i64;
    Ok((held == revision).then(|| format!("unchanged since cursor {revision}\n")))
}

/// A limit, refused at zero: asking for no items would otherwise be answered
/// as if the board held none.
fn positive(limit: Option<u64>) -> Result<Option<usize>, ToolError> {
    match limit {
        Some(0) => Err(invalid("limit must be a positive integer")),
        limit => Ok(limit.map(|n| n as usize)),
    }
}

/// Refuses whatever arguments are left: a misspelled one would otherwise do
/// nothing, silently.
fn finish(args: &Map<String, Value>) -> Result<(), ToolError> {
    match args.keys().next() {
        None => Ok(()),
        Some(key) => Err(invalid(format!("unknown argument: {key}"))),
    }
}

/// The CLI's own rendering of an outcome, without colour.
fn render(home: &std::path::Path, outcome: &Outcome) -> String {
    let mut buffer = Vec::new();
    {
        let mut renderer = Renderer::new(Painter::forced(false), config::get(home).unwrap_or_default(), &mut buffer);
        outcome.render(&mut renderer);
    }
    String::from_utf8_lossy(&buffer).trim_start_matches('\n').to_string()
}

fn supported() -> Vec<&'static str> {
    MODERN.iter().chain(LEGACY).copied().collect()
}

fn server_info() -> Value {
    json!({"name": "ekko", "version": env!("CARGO_PKG_VERSION")})
}

fn complete(mut result: Value) -> Value {
    if let Value::Object(map) = &mut result {
        map.insert("resultType".to_string(), json!("complete"));
        map.insert("_meta".to_string(), json!({"io.modelcontextprotocol/serverInfo": server_info()}));
    }
    result
}

fn error_reply(id: Value, error: RpcError) -> Value {
    let mut body = json!({"code": error.code, "message": error.message});
    if let Some(data) = error.data {
        body["data"] = data;
    }
    json!({"jsonrpc": "2.0", "id": id, "error": body})
}

fn tool_definitions() -> Value {
    let project = json!({"type": "string", "description": "Work on this project instead of the session's board, which prime names on its first line."});
    let if_rev = json!({"type": "integer", "description": "The cursor from an earlier read: if the board has not moved since, the answer is one line saying so."});
    let item = json!({"type": ["integer", "string"], "description": "A display id, or a uid -- which never changes."});
    let items = json!({"type": "array", "items": item, "minItems": 1});
    let state = json!({"type": "string", "enum": ["done", "undone", "progress", "paused", "cancelled", "unstarted", "starred", "unstarred"]});
    let read = json!({"readOnlyHint": true, "openWorldHint": false});
    let write = json!({"readOnlyHint": false, "destructiveHint": false, "openWorldHint": false});
    let object = |properties: Value, required: &[&str]| {
        json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false})
    };

    json!([
        {
            "name": "prime",
            "description": "The resume view of the board: in progress, ready in the order to take it up, blocked, recent notes, what needs attention, and a cursor for changes. Already in context at session start.",
            "inputSchema": object(json!({"project": project, "if_rev": if_rev}), &[]),
            "annotations": read,
        },
        {
            "name": "next",
            "description": "What to take up next, best first: work in progress, earlier phase, higher priority, nearer deadline, more work waiting on it, older.",
            "inputSchema": object(json!({"project": project, "limit": {"type": "integer", "minimum": 1}, "if_rev": if_rev}), &[]),
            "annotations": read,
        },
        {
            "name": "context",
            "description": "Items, one with item or up to 20 with items: text, state and fields, what blocks it and the roots free to start, what it blocks, how much open work waits on it, and its notes -- clipped to 300 characters each unless detail is full -- or the task a note explains.",
            "inputSchema": object(json!({"project": project, "item": item, "items": {"type": "array", "items": item, "minItems": 1, "maxItems": 20}, "detail": {"type": "string", "enum": ["concise", "full"], "default": "concise"}}), &[]),
            "annotations": read,
        },
        {
            "name": "search",
            "description": "Items holding the words of text -- any order, accents ignored, a word also matching longer words it starts -- ranked by relevance and shown where they matched, and/or passing filters: pending, progress, paused, done, cancelled, ready, blocked, due, overdue, star, task, note, or a board name. Up to limit (default 20), with the total. Neither text nor filters gives counts.",
            "inputSchema": object(json!({"project": project, "text": {"type": "string"}, "filters": {"type": "array", "items": {"type": "string"}}, "limit": {"type": "integer", "minimum": 1}}), &[]),
            "annotations": read,
        },
        {
            "name": "changes",
            "description": "What moved after a cursor -- the board revision from prime or an earlier changes, so nothing repeats or ties: items written, stashed or trashed, items removed from storage, and work set free (ready now) or left waiting (blocked now). Returns the next cursor.",
            "inputSchema": object(json!({"project": project, "since": {"type": "integer"}}), &["since"]),
            "annotations": read,
        },
        {
            "name": "roadmap",
            "description": "A project's declared phases in order, with progress and where work sits.",
            "inputSchema": object(json!({"project": project}), &[]),
            "annotations": read,
        },
        {
            "name": "projects",
            "description": "The projects that exist, with what each holds.",
            "inputSchema": object(json!({}), &[]),
            "annotations": read,
        },
        {
            "name": "create",
            "description": "Create a task or a note. The text is kept exactly as given. A note explaining a task should be attached_to it.",
            "inputSchema": object(json!({
                "project": project,
                "kind": {"type": "string", "enum": ["task", "note"], "default": "task"},
                "text": {"type": "string"},
                "boards": {"type": "array", "items": {"type": "string"}},
                "priority": {"type": "integer", "minimum": 1, "maximum": 3, "description": "Tasks only."},
                "due": {"type": "string", "description": "YYYY-MM-DD. Tasks only."},
                "phase": {"type": "string", "description": "A declared phase of the project."},
                "blocked_by": {"type": "array", "items": item, "description": "Tasks only."},
                "attached_to": {"type": ["integer", "string"], "description": "Notes only: the task this note explains."},
                "starred": {"type": "boolean"}
            }), &["text"]),
            "annotations": write,
        },
        {
            "name": "set_state",
            "description": "Set the state of items; safe to retry. Completing a task blocked by open work is refused (BLOCKED), and so is reopening a task completed work depends on (COMPLETED_DEPENDENTS).",
            "inputSchema": object(json!({"project": project, "items": items, "state": state}), &["items", "state"]),
            "annotations": json!({"readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}),
        },
        {
            "name": "force_state",
            "description": "set_state that overrides the dependency rule and reports what it overrode. Only when the user has explicitly said the dependency was dealt with.",
            "inputSchema": object(json!({"project": project, "items": items, "state": state}), &["items", "state"]),
            "annotations": json!({"readOnlyHint": false, "destructiveHint": true, "idempotentHint": true, "openWorldHint": false}),
        },
        {
            "name": "edit",
            "description": "Change an item's text: exactly one of text (all of it), replace (one exact occurrence of old) or append. if_updated_at refuses the edit (STALE) if the item changed since that read.",
            "inputSchema": object(json!({
                "project": project,
                "item": item,
                "text": {"type": "string"},
                "replace": {"type": "object", "properties": {"old": {"type": "string"}, "new": {"type": "string"}}, "required": ["old", "new"], "additionalProperties": false},
                "append": {"type": "string"},
                "if_updated_at": {"type": "integer"}
            }), &["item"]),
            "annotations": write,
        },
        {
            "name": "update",
            "description": "Change an item's boards (replaced), priority, due date (null clears), phase (null moves it to the project root) or star.",
            "inputSchema": object(json!({
                "project": project,
                "item": item,
                "boards": {"type": "array", "items": {"type": "string"}},
                "priority": {"type": "integer", "minimum": 1, "maximum": 3},
                "due": {"type": ["string", "null"]},
                "phase": {"type": ["string", "null"]},
                "starred": {"type": "boolean"}
            }), &["item"]),
            "annotations": write,
        },
        {
            "name": "link",
            "description": "Exactly one of: blocked_by, replacing what the item is blocked by (empty clears it); or attached_to, attaching a note to the task it explains (null detaches).",
            "inputSchema": object(json!({
                "project": project,
                "item": item,
                "blocked_by": {"type": "array", "items": item},
                "attached_to": {"type": ["integer", "string", "null"]}
            }), &["item"]),
            "annotations": write,
        },
        {
            "name": "batch",
            "description": "Several operations in one write, all or nothing. Each is an object with op (create, set_state, edit, update or link) and that tool's arguments, without project. $1, $2 refer to the items created by the first and second operations.",
            "inputSchema": object(json!({
                "project": project,
                "ops": {"type": "array", "minItems": 1, "items": {"type": "object", "properties": {"op": {"type": "string", "enum": ["create", "set_state", "edit", "update", "link"]}}, "required": ["op"]}}
            }), &["ops"]),
            "annotations": write,
        },
        {
            "name": "stash",
            "description": "Put items (or every item on a board, given as @board) away without changing them, or bring them back with away false.",
            "inputSchema": object(json!({"project": project, "items": items, "away": {"type": "boolean", "default": true}}), &["items"]),
            "annotations": write,
        },
        {
            "name": "trash",
            "description": "Move items to the trash, kept 30 days, or bring them back with away false. Ask the user first; prefer set_state cancelled for work decided against.",
            "inputSchema": object(json!({"project": project, "items": items, "away": {"type": "boolean", "default": true}}), &["items"]),
            "annotations": json!({"readOnlyHint": false, "destructiveHint": true, "openWorldHint": false}),
        },
        {
            "name": "away",
            "description": "What is put away: the stash, and the trash with the days each item has left there, one line per item with its state, at most limit (default 20) of each. which narrows it to one of them.",
            "inputSchema": object(json!({"project": project, "which": {"type": "string", "enum": ["stash", "trash"]}, "limit": {"type": "integer", "minimum": 1}}), &[]),
            "annotations": read,
        },
        {
            "name": "phases",
            "description": "Declare the project's phases in the order work goes through them, replacing the sequence: reordering is declaring it again. Answers with the roadmap.",
            "inputSchema": object(json!({"project": project, "sequence": {"type": "array", "items": {"type": "string"}, "minItems": 1}}), &["sequence"]),
            "annotations": write,
        }
    ])
}
