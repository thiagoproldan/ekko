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
use crate::ops::{self, Draft, Op};
use crate::render::{Painter, Renderer};

const MODERN: &[&str] = &["2026-07-28"];
/// Newest first: an `initialize` asking for a version not listed here is
/// answered with the first, which the client may accept or disconnect over.
const LEGACY: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

const TOOLS: &[&str] = &[
    "prime", "next", "context", "search", "changes", "roadmap", "projects", "create", "set_state",
    "force_state", "edit", "update", "link", "batch", "stash", "trash",
];

const INSTRUCTIONS: &str = "\
Ekko is a task board shared with the user: they read and change the same board from their own terminal, between your calls. Do not treat it as yours.

Each session starts with the board's prime already in context -- in progress, ready in order, blocked, and the notes that explain them. Call prime again after a long pause or when the user may have changed things; changes with the cursor from your last read is the cheap way to hear what moved.

- next is the order to take work up. context gives one item in full: its blockers, what it blocks, its notes.
- Hold the uid a write returns. A display id can point at a different item after deletions.
- set_state is idempotent. A task blocked by open work cannot be completed (BLOCKED), and a task that completed work depends on cannot be reopened (COMPLETED_DEPENDENTS): finish the other side, or clear a wrong dependency with link. force_state overrides the rule and is only for when the user has said so.
- Leave reasoning on the board: create a note with attached_to set to the task it explains. Change text with edit's replace or append instead of resending it, with if_updated_at from your last read when the user may have edited it.
- batch applies several operations in one write, all or nothing; $1, $2 name the items created by the batch's first and second operations.
- trash is recoverable for 30 days and still needs the user's consent. For work decided against, set_state cancelled keeps the record.
- Refusals come back as CODE: message. Branch on the code; nothing was written.";

/// What a call without `project` works on, settled once when the server starts.
pub struct Server {
    home: PathBuf,
    ekko_dir_env: Option<String>,
    project: Option<String>,
    matched: bool,
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
        ToolError { code: error.code(), message: error.to_string() }
    }
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
        // The same choice `--prime` makes at a session start, so the prime in
        // context and the tools answer about the same board.
        let matched = if project_env.is_none() && ekko_dir_env.is_none() {
            directory::project_named_after(&home, &cwd)
        } else {
            None
        };
        let is_match = matched.is_some();
        Server { home, ekko_dir_env, project: project_env.or(matched), matched: is_match }
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

    fn open(&self, project: Option<&str>) -> Result<Ekko, EkkoError> {
        let project = project.or(self.project.as_deref());
        Ekko::open(&self.home, &self.home, None, self.ekko_dir_env.as_deref(), project, false)
    }

    fn label(&self, project: Option<&str>) -> String {
        match (project, self.project.as_deref()) {
            (Some(name), _) => format!("project {name}"),
            (None, Some(name)) if self.matched => format!("project {name}, named after this directory"),
            (None, Some(name)) => format!("project {name}"),
            (None, None) => "default board".to_string(),
        }
    }

    fn tool(&self, name: &str, args: &mut Map<String, Value>) -> Result<String, ToolError> {
        if name == "projects" {
            finish(args)?;
            return Ok(render(&self.home, &Outcome::Projects(directory::list_projects(&self.home))));
        }
        let project = match args.remove("project") {
            None | Some(Value::Null) => None,
            Some(Value::String(name)) => Some(name),
            Some(_) => return Err(invalid("project must be a string")),
        };
        let ekko = self.open(project.as_deref())?;

        match name {
            "prime" => {
                finish(args)?;
                Ok(agent::prime(&ekko, &self.label(project.as_deref()))?.text())
            }
            "next" => {
                let limit = take(args, "limit", Value::as_u64, "a positive integer")?;
                finish(args)?;
                let entries = agent::next(&ekko, limit.map(|n| n as usize))?;
                Ok(agent::list_text(&entries, "Nothing is in progress or ready."))
            }
            "context" => {
                let item = take_item(args, "item")?.ok_or_else(|| invalid("context needs an item"))?;
                finish(args)?;
                Ok(agent::context(&ekko, &item)?.text())
            }
            "search" => {
                let text = take(args, "text", |v| v.as_str().map(str::to_string), "a string")?;
                let filters = take_strings(args, "filters")?;
                finish(args)?;
                let entries = agent::search(&ekko, text.as_deref(), &filters)?;
                Ok(agent::list_text(&entries, "Nothing matches."))
            }
            "changes" => {
                let since = take(args, "since", Value::as_i64, "an integer")?.ok_or_else(|| invalid("changes needs since"))?;
                finish(args)?;
                Ok(agent::changes(&ekko, since)?.text())
            }
            "roadmap" => {
                finish(args)?;
                Ok(render(&self.home, &ekko.display_roadmap()?))
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
                return Err(ToolError {
                    code: error.code(),
                    message: format!("operation {} was refused, and nothing in the batch was written: {error}", at + 1),
                })
            }
        }
    }
    let committed = draft.commit(false).map_err(|error| ToolError {
        code: error.code(),
        message: format!("the batch as a whole was refused, and nothing was written: {error}"),
    })?;
    let results: Vec<Value> = touched
        .iter()
        .map(|ids| Value::Array(ids.iter().map(|id| ops::written(&committed.data, *id)).collect()))
        .collect();
    Ok(json!({"ok": true, "results": results}).to_string())
}

fn write(
    ekko: &Ekko,
    force: bool,
    apply: impl FnOnce(&mut Draft) -> Result<Vec<u32>, EkkoError>,
) -> Result<String, ToolError> {
    let mut draft = Draft::open(ekko)?;
    let ids = apply(&mut draft)?;
    let committed = draft.commit(force)?;
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
    Ok(reply.to_string())
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
    let items = take(args, "items", |v| v.as_array().cloned(), "an array of ids or uids")?
        .ok_or_else(|| invalid("items is required"))?;
    let items: Option<Vec<String>> = items.iter().map(item_text).collect();
    match items {
        Some(items) if !items.is_empty() => Ok(items),
        _ => Err(invalid("items must be a non-empty array of ids or uids")),
    }
}

fn take_strings(args: &mut Map<String, Value>, key: &str) -> Result<Vec<String>, ToolError> {
    let values = take(args, key, |v| v.as_array().cloned(), "an array of strings")?.unwrap_or_default();
    values.iter().map(|v| v.as_str().map(str::to_string)).collect::<Option<_>>().ok_or_else(|| invalid(format!("{key} must be an array of strings")))
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
            "inputSchema": object(json!({"project": project}), &[]),
            "annotations": read,
        },
        {
            "name": "next",
            "description": "What to take up next, best first: work in progress, earlier phase, higher priority, nearer deadline, more work waiting on it, older.",
            "inputSchema": object(json!({"project": project, "limit": {"type": "integer", "minimum": 1}}), &[]),
            "annotations": read,
        },
        {
            "name": "context",
            "description": "One item in full: its state and fields, what blocks it, what it blocks, how much open work waits on it, and the notes attached to it (or the task a note explains).",
            "inputSchema": object(json!({"project": project, "item": item}), &["item"]),
            "annotations": read,
        },
        {
            "name": "search",
            "description": "Items matching a text and/or filters. Filters: pending, progress, paused, done, cancelled, ready, blocked, due, overdue, star, task, note, or a board name.",
            "inputSchema": object(json!({"project": project, "text": {"type": "string"}, "filters": {"type": "array", "items": {"type": "string"}}}), &[]),
            "annotations": read,
        },
        {
            "name": "changes",
            "description": "Items written at or after a cursor (from prime or an earlier changes), including ones stashed or trashed since. Returns the next cursor.",
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
        }
    ])
}
