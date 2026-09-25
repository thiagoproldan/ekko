//! `ekko --mcp` spoken to the way a client speaks to it: the real binary,
//! newline-delimited JSON-RPC in on stdin, replies read back from stdout.

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{self, Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

/// A counter beside the clock: tests run in parallel, and two can read the
/// same clock value (task 393).
fn temp_home() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let next = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("ekko-e2e-mcp-{}-{nanos}-{next}", process::id()));
    fs::create_dir_all(dir.join(".ekko").join("storage")).unwrap();
    dir
}

/// Sends every line, closes stdin -- which is what ends the server -- and
/// returns the replies by id, with the ones that could carry no id under
/// `null`.
fn session(home: &PathBuf, lines: &[String]) -> HashMap<String, Value> {
    transcript(home, lines).into_iter().map(|reply| (reply["id"].to_string(), reply)).collect()
}

/// Every message the server wrote, in order, notifications included.
fn transcript(home: &PathBuf, lines: &[String]) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_ekko"))
        .arg("--mcp")
        .env("HOME", home)
        .env("EKKO_DIR", home)
        .env("EKKO_TERMINAL", "none")
        .env_remove("EKKO_PROJECT")
        .current_dir(home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn ekko --mcp");

    let mut stdin = child.stdin.take().unwrap();
    for line in lines {
        writeln!(stdin, "{line}").unwrap();
    }
    drop(stdin);

    let mut out = String::new();
    child.stdout.take().unwrap().read_to_string(&mut out).unwrap();
    let status = child.wait().unwrap();
    assert!(status.success(), "the server did not exit cleanly when stdin closed");

    out.lines()
        .map(|line| serde_json::from_str(line).unwrap_or_else(|e| panic!("not one JSON message per line: {e}: {line}")))
        .collect()
}

fn request(id: u64, method: &str, params: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string()
}

fn call(id: u64, tool: &str, arguments: Value) -> String {
    request(id, "tools/call", json!({"name": tool, "arguments": arguments}))
}

fn text(reply: &Value) -> &str {
    reply["result"]["content"][0]["text"].as_str().unwrap_or_else(|| panic!("no text content: {reply}"))
}

#[test]
fn a_legacy_client_initializes_lists_tools_and_writes_through_them() {
    let home = temp_home();
    let replies = session(
        &home,
        &[
            request(1, "initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}})),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string(),
            request(2, "tools/list", json!({})),
            call(3, "create", json!({"text": "write the server for @agent, p:3 is just text"})),
            call(4, "batch", json!({"ops": [
                {"op": "create", "text": "a"},
                {"op": "create", "text": "b", "blocked_by": ["$1"]},
                {"op": "set_state", "items": ["$2"], "state": "done"}
            ]})),
            call(5, "prime", json!({})),
            call(6, "create", json!({"text": "x", "blockedBy": [1]})),
            request(7, "ping", json!({})),
            call(8, "create", json!({"text": "waits on the first", "blocked_by": [1]})),
            call(9, "set_state", json!({"items": [2], "state": "done"})),
        ],
    );

    assert_eq!(replies.len(), 9, "a notification was answered, or a request was not: {replies:?}");

    let init = &replies["1"]["result"];
    assert_eq!(init["protocolVersion"], "2025-06-18");
    assert_eq!(init["serverInfo"]["name"], "ekko");
    assert!(init["instructions"].as_str().is_some_and(|s| s.contains("shared with the user")));

    let tools = replies["2"]["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 21);
    // The five nearly every session calls, and ask, load at session start; the rest stay behind ToolSearch.
    let loaded: Vec<&str> =
        tools.iter().filter(|tool| tool["_meta"]["anthropic/alwaysLoad"] == true).map(|tool| tool["name"].as_str().unwrap()).collect();
    assert_eq!(loaded, ["context", "search", "create", "set_state", "edit", "ask"]);
    assert!(tools.iter().all(|tool| tool["inputSchema"]["type"] == "object" && tool["description"].is_string()));

    let created: Value = serde_json::from_str(text(&replies["3"])).unwrap();
    assert_eq!(created["items"][0]["id"], 1);
    assert!(created["items"][0]["uid"].is_string());

    // The dependency rule is checked once, on the board the whole batch would
    // leave -- which is what lets a batch complete a blocker and its dependent
    // together -- so the refusal names the batch, not one operation.
    assert_eq!(replies["4"]["result"]["isError"], true);
    assert!(text(&replies["4"]).starts_with("BLOCKED: the batch as a whole was refused"), "{}", text(&replies["4"]));
    // The items it would have created exist nowhere, so they are named by
    // operation rather than by the ids they held in the draft -- and the way
    // out is named in tools, not in CLI flags the agent does not have.
    let refusal = text(&replies["4"]);
    assert!(refusal.contains("$2 (from operation 2) is blocked by $1 (from operation 1)"), "{refusal}");
    assert!(!refusal.contains("--"), "{refusal}");

    let blocked = text(&replies["9"]);
    assert!(blocked.starts_with("BLOCKED: Cannot complete: 2 is blocked by 1, still open."), "{blocked}");
    assert!(blocked.contains("with link") && blocked.contains("force_state"), "{blocked}");
    assert!(!blocked.contains("--"), "{blocked}");

    let prime = text(&replies["5"]);
    assert!(prime.contains("   1. write the server for @agent, p:3 is just text"), "{prime}");
    assert!(!prime.contains("2. a"), "the refused batch wrote something:\n{prime}");

    assert_eq!(replies["6"]["result"]["isError"], true);
    assert!(text(&replies["6"]).starts_with("INVALID_INPUT: unknown field `blockedBy`"), "{}", text(&replies["6"]));

    assert_eq!(replies["7"]["result"], json!({}));

    fs::remove_dir_all(&home).ok();
}

/// A write's reply names the work it set free, so an agent that completes a
/// blocker learns what moved without reading the board again -- and a retry,
/// which frees nothing, says nothing.
#[test]
fn a_write_names_the_work_it_set_free() {
    let home = temp_home();
    let replies = session(
        &home,
        &[
            call(1, "batch", json!({"ops": [
                {"op": "create", "text": "blocker"},
                {"op": "create", "text": "waits on the blocker", "blocked_by": ["$1"]}
            ]})),
            call(2, "set_state", json!({"items": [1], "state": "done"})),
            call(3, "set_state", json!({"items": [1], "state": "done"})),
            call(4, "batch", json!({"ops": [{"op": "set_state", "items": [1], "state": "undone"}]})),
        ],
    );

    let done: Value = serde_json::from_str(text(&replies["2"])).unwrap();
    assert_eq!(done["nowReady"][0]["id"], 2, "{done}");
    assert_eq!(done["nowReady"][0]["text"], "waits on the blocker");
    assert!(done["nowReady"][0]["uid"].is_string(), "{done}");
    assert!(done.get("nowBlocked").is_none(), "{done}");

    let again: Value = serde_json::from_str(text(&replies["3"])).unwrap();
    assert!(again.get("nowReady").is_none(), "a retry set something free: {again}");

    let reopened: Value = serde_json::from_str(text(&replies["4"])).unwrap();
    assert_eq!(reopened["nowBlocked"][0]["id"], 2, "{reopened}");

    fs::remove_dir_all(&home).ok();
}

/// A read that names the cursor it holds is answered in one line while the
/// board has not moved, and in full once a write has moved it.
#[test]
fn a_read_conditioned_on_the_cursor_answers_in_one_line_when_nothing_moved() {
    let home = temp_home();
    let replies = session(
        &home,
        &[
            call(1, "create", json!({"text": "one"})),
            call(2, "prime", json!({"if_rev": 1})),
            call(3, "next", json!({"if_rev": 1})),
            call(4, "create", json!({"text": "two"})),
            call(5, "prime", json!({"if_rev": 1})),
            call(6, "changes", json!({"since": 1})),
        ],
    );

    assert_eq!(text(&replies["2"]), "unchanged since cursor 1\n");
    assert_eq!(text(&replies["3"]), "unchanged since cursor 1\n");
    assert!(text(&replies["5"]).starts_with("ekko \u{b7} default board \u{b7} cursor 2\n"), "{}", text(&replies["5"]));
    assert!(text(&replies["6"]).starts_with("cursor 2 \u{b7} 1 changed since 1\n   2. [pending] two\n"), "{}", text(&replies["6"]));

    fs::remove_dir_all(&home).ok();
}

/// Several items in one call, in the order asked; item and items together,
/// or more than one call reads, are refused rather than half-answered.
#[test]
fn context_reads_several_items_in_one_call() {
    let home = temp_home();
    let many: Vec<u32> = (1..=21).collect();
    let replies = session(
        &home,
        &[
            call(1, "batch", json!({"ops": [{"op": "create", "text": "first"}, {"op": "create", "text": "second"}]})),
            call(2, "context", json!({"items": [2, 1]})),
            call(3, "context", json!({"item": 1, "items": [2]})),
            call(4, "context", json!({"items": many})),
        ],
    );

    let both = text(&replies["2"]);
    let second = both.find("   2. second").unwrap_or_else(|| panic!("{both}"));
    let first = both.find("   1. first").unwrap_or_else(|| panic!("{both}"));
    assert!(second < first, "not in the order asked:\n{both}");

    for id in ["3", "4"] {
        assert_eq!(replies[id]["result"]["isError"], true, "{}", replies[id]);
        assert!(text(&replies[id]).starts_with("INVALID_INPUT"), "{}", text(&replies[id]));
    }

    fs::remove_dir_all(&home).ok();
}

#[test]
fn a_modern_client_is_served_per_request_and_refused_a_version_it_does_not_share() {
    let home = temp_home();
    let meta = |version: &str| json!({"io.modelcontextprotocol/protocolVersion": version, "io.modelcontextprotocol/clientCapabilities": {}});
    let replies = session(
        &home,
        &[
            request(1, "server/discover", json!({"_meta": meta("2026-07-28")})),
            request(2, "tools/call", json!({"name": "next", "arguments": {}, "_meta": meta("2026-07-28")})),
            request(3, "tools/list", json!({"_meta": meta("1900-01-01")})),
            request(4, "tools/list", json!({"_meta": {"io.modelcontextprotocol/protocolVersion": "2026-07-28"}})),
            request(5, "no/such/method", json!({})),
            request(6, "tools/call", json!({"name": "destroy", "arguments": {}})),
            "this is not json".to_string(),
        ],
    );

    let discover = &replies["1"]["result"];
    assert_eq!(discover["resultType"], "complete");
    assert_eq!(discover["supportedVersions"][0], "2026-07-28");
    assert!(discover["supportedVersions"].as_array().unwrap().contains(&json!("2025-11-25")));
    assert_eq!(discover["_meta"]["io.modelcontextprotocol/serverInfo"]["name"], "ekko");
    assert_eq!(discover["capabilities"], json!({"tools": {}, "prompts": {}}));

    assert_eq!(replies["2"]["result"]["resultType"], "complete");
    assert_eq!(text(&replies["2"]), "Nothing is in progress or ready.\n");

    assert_eq!(replies["3"]["error"]["code"], -32022);
    assert_eq!(replies["3"]["error"]["data"]["requested"], "1900-01-01");
    assert_eq!(replies["4"]["error"]["code"], -32602, "clientCapabilities is required on a modern request");
    assert_eq!(replies["5"]["error"]["code"], -32601);
    assert_eq!(replies["6"]["error"]["code"], -32602);
    assert_eq!(replies["null"]["error"]["code"], -32700);

    fs::remove_dir_all(&home).ok();
}

/// What is put away reads through a read-only tool, and the phase sequence is
/// declared over MCP: no question a session asks sends the agent to the files.
#[test]
fn away_lists_the_stash_and_the_trash_and_phases_declares_the_sequence() {
    let home = temp_home();
    let replies = session(
        &home,
        &[
            request(1, "initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}})),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string(),
            call(2, "create", json!({"text": "kept for later"})),
            call(3, "create", json!({"text": "made by mistake"})),
            call(4, "stash", json!({"items": [1]})),
            call(5, "trash", json!({"items": [2]})),
            call(6, "away", json!({})),
            call(7, "away", json!({"which": "trash"})),
            call(8, "phases", json!({"sequence": ["setup", "build"]})),
            call(9, "away", json!({"which": "archive"})),
            call(10, "phases", json!({"sequence": []})),
        ],
    );

    let both = text(&replies["6"]);
    assert!(both.contains("kept for later") && both.contains("made by mistake"), "{both}");
    let trash = text(&replies["7"]);
    assert!(trash.contains("made by mistake") && !trash.contains("kept for later"), "{trash}");
    let roadmap = text(&replies["8"]);
    assert!(roadmap.contains("setup") && roadmap.contains("build"), "{roadmap}");
    assert_eq!(replies["9"]["result"]["isError"], true, "{}", replies["9"]);
    assert_eq!(replies["10"]["result"]["isError"], true, "{}", replies["10"]);

    fs::remove_dir_all(&home).ok();
}

/// The server offers `handoff` as a prompt -- what Claude Code turns into a
/// slash command -- and fills it in from the board: the task in progress, by
/// id and uid. A handoff written through create then leads the next prime.
#[test]
fn the_handoff_prompt_is_offered_and_a_handoff_leads_the_prime() {
    let home = temp_home();
    let replies = session(
        &home,
        &[
            request(1, "initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}})),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string(),
            call(2, "create", json!({"text": "port the parser"})),
            call(3, "set_state", json!({"items": [1], "state": "progress"})),
            request(4, "prompts/list", json!({})),
            request(5, "prompts/get", json!({"name": "handoff"})),
            call(6, "create", json!({"kind": "handoff", "text": "Stopped after the lexer.\nNext: the parser's error paths.", "attached_to": 1})),
            call(7, "prime", json!({})),
            request(8, "prompts/get", json!({"name": "nothing"})),
        ],
    );

    assert!(replies["1"]["result"]["capabilities"]["prompts"].is_object(), "{}", replies["1"]);
    assert_eq!(replies["4"]["result"]["prompts"][0]["name"], "handoff");
    let prompt = replies["5"]["result"]["messages"][0]["content"]["text"].as_str().unwrap();
    assert!(prompt.contains("Write the handoff for task 1 now"), "{prompt}");
    let prime = text(&replies["7"]);
    assert!(prime.contains("Where the last session stopped: handoff 2 on task 1 [in progress]"), "{prime}");
    assert!(prime.contains("    > Next: the parser's error paths."), "{prime}");
    assert!(replies["8"]["error"].is_object(), "{}", replies["8"]);

    fs::remove_dir_all(&home).ok();
}

/// Typed notes end to end over stdio: create takes the kinds and supersedes,
/// prime lists what is in force, search filters by kind, and the schema an
/// agent reads offers all of it.
#[test]
fn typed_notes_are_written_listed_and_searched_over_stdio() {
    let home = temp_home();
    let replies = session(
        &home,
        &[
            request(1, "initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}})),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string(),
            call(2, "create", json!({"kind": "decision", "text": "ship weekly"})),
            call(3, "create", json!({"kind": "decision", "text": "ship on demand", "supersedes": 1})),
            call(4, "create", json!({"kind": "procedure", "text": "Release: bump, tag, push"})),
            call(5, "prime", json!({})),
            call(6, "search", json!({"filters": ["decision"]})),
            call(7, "create", json!({"kind": "gotcha", "text": "wrong kind", "supersedes": 3})),
            request(8, "tools/list", json!({})),
        ],
    );

    let prime = text(&replies["5"]);
    assert!(prime.contains("Gotchas and procedures (1)\n   3. [procedure] Release: bump, tag, push"), "{prime}");
    assert!(prime.contains("Decisions (1): search with the decision filter"), "{prime}");
    assert!(!prime.contains("ship weekly"), "{prime}");
    let found = text(&replies["6"]);
    assert!(found.contains("1. [decision, superseded by 2] ship weekly"), "{found}");
    assert!(text(&replies["7"]).starts_with("INVALID_INPUT: 3 is a procedure, and a gotcha supersedes only a gotcha"), "{}", replies["7"]);
    let tools = replies["8"]["result"]["tools"].as_array().unwrap();
    let create = tools.iter().find(|tool| tool["name"] == "create").unwrap();
    assert!(create["inputSchema"]["properties"]["kind"]["enum"].as_array().unwrap().contains(&json!("gotcha")));
    assert!(create["inputSchema"]["properties"]["supersedes"].is_object());

    fs::remove_dir_all(&home).ok();
}

/// The waiting state over stdio: set_state takes it, next and a write's reply
/// leave the task out as work to take up, prime and search list it, and the
/// schema an agent reads offers it.
#[test]
fn a_waiting_task_is_set_listed_and_never_offered_as_ready() {
    let home = temp_home();
    let replies = session(
        &home,
        &[
            call(1, "batch", json!({"ops": [
                {"op": "create", "text": "blocker"},
                {"op": "create", "text": "needs the vendor's reply", "blocked_by": ["$1"]},
                {"op": "set_state", "items": ["$2"], "state": "waiting"}
            ]})),
            call(2, "set_state", json!({"items": [1], "state": "done"})),
            call(3, "next", json!({})),
            call(4, "prime", json!({})),
            call(5, "search", json!({"filters": ["waiting"]})),
            request(6, "tools/list", json!({})),
        ],
    );

    let done: Value = serde_json::from_str(text(&replies["2"])).unwrap();
    assert!(done.get("nowReady").is_none(), "a waiting task was reported ready: {done}");
    assert!(!text(&replies["3"]).contains("vendor"), "{}", text(&replies["3"]));
    assert!(text(&replies["4"]).contains("\nWaiting (1)\n   2. needs the vendor's reply"), "{}", text(&replies["4"]));
    assert!(text(&replies["5"]).contains("2. [waiting] needs the vendor's reply"), "{}", text(&replies["5"]));
    let tools = replies["6"]["result"]["tools"].as_array().unwrap();
    let set_state = tools.iter().find(|tool| tool["name"] == "set_state").unwrap();
    assert!(set_state["inputSchema"]["properties"]["state"]["enum"].as_array().unwrap().contains(&json!("waiting")));

    fs::remove_dir_all(&home).ok();
}

/// Who a task is with, over stdio: create takes it, next names the task
/// apart instead of offering it, the prime lists it by name, search finds it
/// with with:NAME, and update with null makes it work to take up again.
#[test]
fn a_task_with_someone_is_named_apart_from_the_work_to_take_up() {
    let home = temp_home();
    let replies = session(
        &home,
        &[
            call(1, "batch", json!({"ops": [
                {"op": "create", "text": "mine to take"},
                {"op": "create", "text": "call the vendor", "with": "Rodrigo"}
            ]})),
            call(2, "next", json!({})),
            call(3, "prime", json!({})),
            call(4, "search", json!({"filters": ["with:rodrigo"]})),
            call(5, "update", json!({"item": 2, "with": null})),
            call(6, "next", json!({})),
        ],
    );

    let next = text(&replies["2"]);
    assert!(next.contains("   1. mine to take\n") && !next.contains("   2. call the vendor"), "{next}");
    assert!(next.contains("With someone, not to take up: 2 (rodrigo)\n"), "{next}");
    assert!(
        text(&replies["3"]).contains("\nWith someone, not to take up (1)\n   2. call the vendor \u{b7} with rodrigo\n"),
        "{}",
        text(&replies["3"])
    );
    assert!(text(&replies["4"]).contains("2. [pending] call the vendor \u{b7} with rodrigo"), "{}", text(&replies["4"]));
    assert!(text(&replies["5"]).contains("\"ok\":true"), "{}", replies["5"]);
    assert!(text(&replies["6"]).contains("   2. call the vendor\n"), "{}", text(&replies["6"]));
    assert!(!text(&replies["6"]).contains("With someone"), "{}", text(&replies["6"]));

    fs::remove_dir_all(&home).ok();
}

/// A server kept running while a test talks to it, a message at a time: what
/// the resources server does on its own, between requests, shows only here.
struct Live {
    child: std::process::Child,
    stdin: Option<std::process::ChildStdin>,
    messages: std::sync::mpsc::Receiver<Value>,
}

impl Live {
    fn start(home: &PathBuf, args: &[&str]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_ekko"))
            .args(args)
            .env("HOME", home)
            .env("EKKO_DIR", home)
            .env("EKKO_TERMINAL", "none")
            .env_remove("EKKO_PROJECT")
            .current_dir(home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn ekko");
        let stdout = child.stdout.take().unwrap();
        let (sender, messages) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            use std::io::BufRead as _;
            for line in std::io::BufReader::new(stdout).lines().map_while(Result::ok) {
                let message = serde_json::from_str(&line).unwrap_or_else(|e| panic!("not one JSON message per line: {e}: {line}"));
                if sender.send(message).is_err() {
                    break;
                }
            }
        });
        Live { stdin: child.stdin.take(), child, messages }
    }

    /// Sends a request and returns its reply, failing on anything else first.
    fn ask(&mut self, line: String) -> Value {
        writeln!(self.stdin.as_mut().unwrap(), "{line}").unwrap();
        self.next(std::time::Duration::from_secs(5)).expect("no reply")
    }

    fn next(&self, within: std::time::Duration) -> Option<Value> {
        self.messages.recv_timeout(within).ok()
    }

    fn stop(mut self) {
        drop(self.stdin.take());
        assert!(self.child.wait().unwrap().success(), "the server did not exit cleanly when stdin closed");
    }
}

/// The board as resources a person mentions with @, from their own server:
/// the plugin's server keeps the tools and offers no resources, because
/// Claude Code cannot resolve a mention of a server named plugin:ekko:ekko.
/// The resources server lists the prime and the items worth picking, reads
/// any item as context does, and when the board changes -- through the other
/// server or a terminal, which it never hears of -- tells the client within
/// seconds, so a new item can be mentioned.
#[test]
fn the_board_is_served_as_resources_by_a_server_of_their_own() {
    let home = temp_home();
    let board = session(
        &home,
        &[
            call(1, "create", json!({"text": "an open task"})),
            call(2, "create", json!({"text": "a finished task"})),
            call(3, "set_state", json!({"items": [2], "state": "done"})),
            call(4, "create", json!({"kind": "note", "text": "a plain note"})),
            call(5, "create", json!({"kind": "gotcha", "text": "a trap to remember"})),
            request(6, "resources/list", json!({})),
        ],
    );
    assert_eq!(board["6"]["error"]["code"], -32601, "the plugin's server offers no resources: {}", board["6"]);

    let mut live = Live::start(&home, &["--mcp", "--resources"]);
    let init = live.ask(request(1, "initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}})));
    assert_eq!(init["result"]["capabilities"], json!({"resources": {"listChanged": true}}));
    assert!(init["result"].get("instructions").is_none(), "the plugin's server carries the one copy: {init}");
    assert_eq!(live.ask(request(2, "tools/list", json!({})))["error"]["code"], -32601);

    let uris = |reply: &Value| -> Vec<String> {
        reply["result"]["resources"].as_array().unwrap().iter().map(|r| r["uri"].as_str().unwrap().to_string()).collect()
    };
    let listed = live.ask(request(3, "resources/list", json!({})));
    assert_eq!(uris(&listed), ["prime://board", "item://1", "item://4"], "closed work and plain notes are left out");
    assert_eq!(listed["result"]["resources"][1]["name"], "1 \u{b7} an open task");

    let text_of = |reply: &Value| reply["result"]["contents"][0]["text"].as_str().unwrap_or_else(|| panic!("{reply}")).to_string();
    assert!(text_of(&live.ask(request(4, "resources/read", json!({"uri": "item://1"})))).contains("an open task"));
    let unlisted = text_of(&live.ask(request(5, "resources/read", json!({"uri": "item://2"}))));
    assert!(unlisted.contains("a finished task"), "an unlisted item still reads: {unlisted}");
    assert!(text_of(&live.ask(request(6, "resources/read", json!({"uri": "prime://board"})))).starts_with("ekko \u{b7} "));
    assert_eq!(live.ask(request(7, "resources/read", json!({"uri": "item://99"})))["error"]["code"], -32002);
    assert_eq!(live.ask(request(8, "resources/templates/list", json!({})))["result"]["resourceTemplates"], json!([]));

    // A write from a terminal, which this server takes no part in.
    let written = Command::new(env!("CARGO_BIN_EXE_ekko"))
        .args(["--task", "made", "from", "a", "terminal"])
        .env("HOME", &home)
        .env("EKKO_DIR", &home)
        .env_remove("EKKO_PROJECT")
        .output()
        .unwrap();
    assert!(written.status.success(), "{}", String::from_utf8_lossy(&written.stderr));
    let notice = live.next(std::time::Duration::from_secs(10)).expect("no list_changed within 10 seconds of the write");
    assert_eq!(notice, json!({"jsonrpc": "2.0", "method": "notifications/resources/list_changed"}));
    assert_eq!(uris(&live.ask(request(9, "resources/list", json!({})))), ["prime://board", "item://1", "item://4", "item://5"]);

    // A write that leaves the list as it was is not announced.
    let starred = Command::new(env!("CARGO_BIN_EXE_ekko")).args(["--star", "1"]).env("HOME", &home).env("EKKO_DIR", &home).output().unwrap();
    assert!(starred.status.success(), "{}", String::from_utf8_lossy(&starred.stderr));
    assert_eq!(live.next(std::time::Duration::from_secs(5)), None, "the list did not change, yet the client was told it did");

    live.stop();
    fs::remove_dir_all(&home).ok();
}

/// ask records a question on the board, and the prime lists it under
/// 'Waiting on you' until answer records the reply; a second answer is
/// refused, and context reads the one given.
#[test]
fn a_question_is_asked_and_answered_through_the_tools() {
    let home = temp_home();
    let replies = session(
        &home,
        &[
            call(1, "create", json!({"text": "the merge"})),
            call(2, "ask", json!({"questions": [{"text": "Merge auditoria now?"}], "about": 1})),
            call(3, "prime", json!({})),
            call(4, "answer", json!({"question": 2, "text": "yes, after the rebase"})),
            call(5, "answer", json!({"question": 2, "text": "no"})),
            call(6, "context", json!({"item": 2})),
            call(7, "prime", json!({})),
        ],
    );
    assert!(text(&replies["2"]).contains("\"id\":2"), "{}", replies["2"]);
    assert!(text(&replies["3"]).contains("\nWaiting on you (1)\n   2. [asked by "), "{}", text(&replies["3"]));
    assert!(text(&replies["3"]).contains(", about 1] Merge auditoria now?\n"), "{}", text(&replies["3"]));
    assert!(text(&replies["4"]).contains("\"ok\":true"), "{}", replies["4"]);
    assert_eq!(replies["5"]["result"]["isError"], true, "{}", replies["5"]);
    assert!(text(&replies["5"]).contains("2 was already answered: yes, after the rebase"), "{}", replies["5"]);
    assert!(text(&replies["6"]).contains(": yes, after the rebase\n"), "{}", text(&replies["6"]));
    assert!(!text(&replies["7"]).contains("Waiting on you"), "{}", text(&replies["7"]));

    fs::remove_dir_all(&home).ok();
}

fn elicitation_client() -> String {
    request(1, "initialize", json!({"protocolVersion": "2025-11-25", "capabilities": {"elicitation": {}}, "clientInfo": {"name": "test", "version": "0"}}))
}

fn dialog_answer(id: &str, result: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string()
}

/// ask in a client that shows forms: the question is recorded, put in a
/// dialog, and the answer that comes back is recorded against it -- a label
/// chosen, or the text of "Other answer…" in a second dialog. Other requests
/// are answered while a dialog is open.
#[test]
fn a_question_is_put_to_the_user_in_a_dialog_and_the_answer_recorded() {
    let home = temp_home();
    let options = json!([{"label": "Yes", "description": "all four"}, {"label": "No"}]);
    let messages = transcript(
        &home,
        &[
            elicitation_client(),
            call(2, "ask", json!({"questions": [{"text": "Mark them?", "options": options}]})),
            request(3, "ping", json!({})),
            dialog_answer("ekko-ask-1", json!({"action": "accept", "content": {"answer": "Yes"}})),
            call(4, "ask", json!({"questions": [{"text": "Why?", "options": options}]})),
            dialog_answer("ekko-ask-2", json!({"action": "accept", "content": {"answer": "ekko:other"}})),
            dialog_answer("ekko-ask-3", json!({"action": "accept", "content": {"answer": "only the ninth"}})),
            call(5, "context", json!({"items": [1, 2]})),
        ],
    );
    let by_id = |id: Value| messages.iter().find(|m| m["id"] == id && m.get("method").is_none()).unwrap_or_else(|| panic!("no reply {id}: {messages:?}"));
    let opened: Vec<&Value> = messages.iter().filter(|m| m["method"] == "elicitation/create").collect();
    assert_eq!(opened.len(), 3, "{messages:?}");
    assert_eq!(opened[0]["id"], "ekko-ask-1");
    assert_eq!(opened[0]["params"]["message"], "Mark them?");
    let choices = &opened[0]["params"]["requestedSchema"]["properties"]["answer"]["oneOf"];
    assert_eq!(choices[0], json!({"const": "Yes", "title": "Yes: all four"}));
    assert_eq!(choices[2]["title"], "Other answer…");
    assert_eq!(opened[2]["params"]["requestedSchema"]["properties"]["answer"], json!({"type": "string", "title": "Answer"}));
    // The ping is answered before the dialog comes back.
    let at = |id: Value| messages.iter().position(|m| m["id"] == id && m.get("method").is_none()).unwrap();
    assert!(at(json!(3)) < at(json!(2)), "{messages:?}");

    let first: Value = serde_json::from_str(text(by_id(json!(2)))).unwrap();
    assert_eq!(first["answers"], json!([{"id": 1, "answer": "Yes"}]), "{first}");
    assert_eq!(first["items"][0]["id"], 1);
    let second: Value = serde_json::from_str(text(by_id(json!(4)))).unwrap();
    assert_eq!(second["answers"][0]["answer"], "only the ninth", "{second}");
    let read = text(by_id(json!(5)));
    assert!(read.contains("Mark them?\nOptions:\n- Yes: all four\n- No"), "{read}");
    assert!(read.contains(": Yes\n") && read.contains(": only the ninth\n"), "{read}");

    fs::remove_dir_all(&home).ok();
}

/// What leaves a question open: a dialog declined or dismissed -- Claude Code
/// under -p dismisses every one at once -- a call the client cancels, whose
/// dialog is then closed, and a client that shows no forms at all.
#[test]
fn a_question_left_unanswered_stays_open_on_the_board() {
    let home = temp_home();
    let messages = transcript(
        &home,
        &[
            elicitation_client(),
            call(2, "ask", json!({"questions": [{"text": "Declined?"}]})),
            dialog_answer("ekko-ask-1", json!({"action": "decline"})),
            call(3, "ask", json!({"questions": [{"text": "Dismissed?"}]})),
            dialog_answer("ekko-ask-2", json!({"action": "cancel"})),
            call(4, "ask", json!({"questions": [{"text": "Cancelled?"}]})),
            json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 4}}).to_string(),
            call(5, "ask", json!({"questions": [{"text": "One option?", "options": [{"label": "Only"}]}]})),
            call(6, "prime", json!({})),
        ],
    );
    let reply = |id: u64| messages.iter().find(|m| m["id"] == id).unwrap_or_else(|| panic!("no reply {id}: {messages:?}"));
    assert!(text(reply(2)).contains("\"unanswered\":\"the user declined the dialog; the questions without an answer stay open"), "{}", text(reply(2)));
    assert!(text(reply(3)).contains("\"unanswered\":\"the user dismissed the dialog; the questions without an answer stay open"), "{}", text(reply(3)));
    assert!(!messages.iter().any(|m| m["id"] == 4), "a cancelled call is not answered: {messages:?}");
    assert!(messages.iter().any(|m| m["method"] == "notifications/cancelled" && m["params"]["requestId"] == "ekko-ask-3"), "{messages:?}");
    assert_eq!(reply(5)["result"]["isError"], true);
    assert!(text(reply(5)).starts_with("INVALID_INPUT: options offers 2 to 6 answers"), "{}", text(reply(5)));
    assert!(text(reply(6)).contains("\nWaiting on you (3)\n"), "{}", text(reply(6)));

    let without = session(&home, &[request(1, "initialize", json!({"protocolVersion": "2025-11-25", "capabilities": {}})), call(2, "ask", json!({"questions": [{"text": "No forms?"}]}))]);
    assert!(text(&without["2"]).contains("\"unanswered\":\"ekko's menu has nowhere to open here: no tmux, and no display; the questions"), "{}", without["2"]);

    fs::remove_dir_all(&home).ok();
}

/// A server kept running while a test talks to it, as a client does while a
/// window of ekko's menu is up: the answer comes back from a thread of the
/// server, after stdin has gone quiet.
struct Held {
    child: process::Child,
    stdin: Option<process::ChildStdin>,
    messages: std::sync::mpsc::Receiver<Value>,
    seen: Vec<Value>,
}

impl Held {
    /// `ekko --mcp` whose menu opens with `terminal`, a script standing in
    /// for the terminal: it gets ekko's command line as its arguments.
    fn start(home: &PathBuf, terminal: &str) -> Held {
        let script = home.join("terminal.sh");
        // The arguments end in `<ekko> --menu <file>`: the script finds both,
        // and the pid file beside the questions, as $exe, $spec and $pid.
        let prelude = "spec=; exe=; prev=\nfor a in \"$@\"; do\n  [ \"$prev\" = --menu ] && spec=$a\n  [ \"$a\" = --menu ] && exe=$prev\n  prev=$a\ndone\npid=\"${spec%.json}.pid\"\n";
        let log = home.join("terminal.log");
        fs::write(&script, format!("#!/bin/sh\necho \"$@\" >> \"{}\"\n{prelude}{terminal}\n", log.display())).unwrap();
        fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_ekko"))
            .arg("--mcp")
            .env("HOME", home)
            .env("EKKO_DIR", home)
            .env("EKKO_TERMINAL", &script)
            // As in a session of its own: the command line is what removes it
            // from the menu, and the fake terminal skips the command line.
            .env_remove("CLAUDECODE")
            .env("XDG_RUNTIME_DIR", home)
            .env_remove("EKKO_PROJECT")
            .current_dir(home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn ekko --mcp");
        let stdout = child.stdout.take().unwrap();
        let (tx, messages) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(serde_json::from_str(&line).unwrap_or_else(|e| panic!("not JSON: {e}: {line}"))).is_err() {
                    break;
                }
            }
        });
        let stdin = child.stdin.take();
        let mut live = Held { child, stdin, messages, seen: Vec::new() };
        live.send(&request(1, "initialize", json!({"protocolVersion": "2025-11-25", "capabilities": {}})));
        live.reply(1);
        live
    }

    fn send(&mut self, line: &str) {
        writeln!(self.stdin.as_mut().unwrap(), "{line}").unwrap();
    }

    /// The reply to request `id`, waiting up to 20 seconds for it.
    fn reply(&mut self, id: u64) -> Value {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            if let Some(found) = self.seen.iter().find(|m| m["id"] == id && m.get("method").is_none()) {
                return found.clone();
            }
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match self.messages.recv_timeout(left) {
                Ok(message) => self.seen.push(message),
                Err(_) => panic!("no reply {id} in 20 seconds: {:?}", self.seen),
            }
        }
    }

    fn stop(mut self) {
        drop(self.stdin.take());
        assert!(self.child.wait().unwrap().success(), "the server did not exit cleanly when stdin closed");
    }
}

/// ask where a menu can open: the questions go to ekko's own menu, a
/// terminal running `ekko --menu <file>`, and the answers recorded there come
/// back as ask's reply. No elicitation is sent, even to a client that shows
/// forms, and the file -- with the previews the board does not keep -- is
/// gone once the call is answered.
#[test]
fn questions_go_to_ekkos_menu_and_the_answers_come_back() {
    let home = temp_home();
    // What the menu does, less the keys: the pid first, then every answer.
    let answer = format!(
        "cp \"$spec\" \"{}\"\necho $$ > \"$pid\"\nfor uid in $(grep -o '\"uid\":\"[^\"]*\"' \"$spec\" | cut -d'\"' -f4); do \"$exe\" --answer \"$uid\" Yes; done",
        home.join("spec.json").display()
    );
    let mut live = Held::start(&home, &answer);
    let options = json!([{"label": "Yes", "description": "all four", "preview": "fn yes() {}"}, {"label": "No"}]);
    live.send(&call(2, "ask", json!({"questions": [{"text": "Mark them?", "options": options}, {"text": "And these?", "options": options, "multiple": true}]})));
    let answered: Value = serde_json::from_str(text(&live.reply(2))).unwrap();
    assert_eq!(answered["answers"], json!([{"id": 1, "answer": "Yes"}, {"id": 2, "answer": "Yes"}]), "{answered}");
    assert!(answered.get("unanswered").is_none(), "{answered}");
    live.send(&call(3, "context", json!({"items": [1, 2]})));
    let read = text(&live.reply(3)).to_string();
    assert!(read.contains("Mark them?\nOptions:\n- Yes: all four\n- No"), "{read}");
    assert!(read.contains("And these?\nOptions, any number of them:\n- Yes: all four"), "{read}");
    assert!(read.contains("recorded by the user: Yes\n"), "the answers are the user's, not the session's: {read}");
    assert!(!live.seen.iter().any(|m| m["method"] == "elicitation/create"), "{:?}", live.seen);
    live.stop();
    let log = fs::read_to_string(home.join("terminal.log")).unwrap();
    assert!(log.starts_with("env -u CLAUDECODE ") && log.contains(" --menu "), "{log}");
    let spec: Value = serde_json::from_str(&fs::read_to_string(home.join("spec.json")).unwrap()).unwrap();
    assert_eq!(spec["questions"][0]["options"][0]["preview"], "fn yes() {}");
    assert_eq!(spec["questions"][1]["multiple"], true);
    let left = fs::read_dir(&home).unwrap().filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().starts_with("ekko-menu-")).count();
    assert_eq!(left, 0, "the menu's files are cleaned up");

    fs::remove_dir_all(&home).ok();
}

/// What leaves questions in ekko's menu open: a menu closed without the
/// answers -- the ones given meanwhile still come back -- a terminal that
/// cannot open one, and a call the client cancels, which closes its menu.
#[test]
fn questions_left_in_ekkos_menu_stay_open() {
    let home = temp_home();
    // The first question answered, then the menu closed.
    let mut closed = Held::start(&home, "echo $$ > \"$pid\"\nuid=$(grep -o '\"uid\":\"[^\"]*\"' \"$spec\" | head -1 | cut -d'\"' -f4)\n\"$exe\" --answer \"$uid\" first");
    closed.send(&call(2, "ask", json!({"questions": [{"text": "One?"}, {"text": "Two?"}]})));
    let reply: Value = serde_json::from_str(text(&closed.reply(2))).unwrap();
    assert_eq!(reply["answers"], json!([{"id": 1, "answer": "first"}]), "{reply}");
    assert!(reply["unanswered"].as_str().unwrap().starts_with("the user closed ekko's menu without answering; the questions without an answer stay open"), "{reply}");
    closed.stop();

    let mut failed = Held::start(&home, "exit 3");
    failed.send(&call(2, "ask", json!({"questions": [{"text": "Failed?"}]})));
    assert!(text(&failed.reply(2)).contains("ekko's menu could not open: the terminal exited with exit status: 3; the questions"), "{:?}", failed.seen);
    failed.stop();

    let menu_pid = home.join("menu.pid");
    let mut cancelled = Held::start(&home, &format!("echo $$ > \"$pid\"\necho $$ > \"{}\"\nexec sleep 30", menu_pid.display()));
    cancelled.send(&call(2, "ask", json!({"questions": [{"text": "Cancelled?"}]})));
    let up = std::time::Instant::now();
    while fs::read_to_string(&menu_pid).map_or(true, |p| p.trim().is_empty()) {
        assert!(up.elapsed().as_secs() < 10, "the menu never came up");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    cancelled.send(&json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 2}}).to_string());
    cancelled.send(&call(3, "prime", json!({})));
    assert!(text(&cancelled.reply(3)).contains("\nWaiting on you (3)\n"), "{:?}", cancelled.seen);
    let menu = fs::read_to_string(&menu_pid).unwrap().trim().to_string();
    let gone = (0..40).any(|_| {
        std::thread::sleep(std::time::Duration::from_millis(50));
        !Command::new("kill").args(["-0", &menu]).stderr(Stdio::null()).status().unwrap().success()
    });
    assert!(gone, "the cancelled call's menu is closed");
    assert!(!cancelled.seen.iter().any(|m| m["id"] == 2), "a cancelled call is not answered: {:?}", cancelled.seen);
    cancelled.stop();

    fs::remove_dir_all(&home).ok();
}

/// What Claude Code puts ahead of every conversation from this server: its
/// instructions and the always-loaded tool definitions. A byte changed there
/// makes every session resumed after an upgrade write its whole context again
/// -- six such rewrites cost 9.6% of the handoff era of 2026-09-21 (note 258)
/// -- so it changes on purpose, batched into a release that changes it anyway,
/// with this fingerprint moved alongside.
const PREFIX_FINGERPRINT: u64 = 0xaead77f39854ac15;

#[test]
fn the_prefix_every_session_pays_for_changes_only_on_purpose() {
    let home = temp_home();
    let replies = session(
        &home,
        &[
            request(1, "initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}})),
            request(2, "tools/list", json!({})),
        ],
    );
    let instructions = replies["1"]["result"]["instructions"].as_str().unwrap();
    // Claude Code keeps the first 2,048 characters of a server's instructions (note 167).
    assert!(instructions.chars().count() <= 2048, "{} characters of instructions", instructions.chars().count());
    let mut prefix = instructions.to_string();
    for tool in replies["2"]["result"]["tools"].as_array().unwrap() {
        if tool["_meta"]["anthropic/alwaysLoad"] == true {
            prefix.push_str(&tool.to_string());
        }
    }
    // FNV-1a: stable across Rust versions, which std's hasher is not.
    let fingerprint = prefix.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3));
    assert_eq!(
        fingerprint, PREFIX_FINGERPRINT,
        "the instructions or an always-loaded definition changed: if that is meant, batch it with the other changes to them in one release, and set PREFIX_FINGERPRINT to {fingerprint:#018x}"
    );

    fs::remove_dir_all(&home).ok();
}

/// A server kept open between calls, under a shell of its own that stays its
/// parent: another client process, the way each Claude Code session is.
struct Session {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
    sent: u64,
}

impl Session {
    fn start(home: &PathBuf) -> Session {
        Session::spawn(home, None)
    }

    /// A session started in `folder`, as Claude Code starts one in a
    /// project's folder: its board is the project found from there.
    fn in_folder(home: &PathBuf, folder: &Path) -> Session {
        Session::spawn(home, Some(folder))
    }

    fn spawn(home: &PathBuf, folder: Option<&Path>) -> Session {
        // Not exec'd: the shell stays, as the server's parent, until the
        // server exits.
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg(format!("'{}' --mcp; true", env!("CARGO_BIN_EXE_ekko")))
            .env("HOME", home)
            .env("EKKO_TERMINAL", "none")
            .env_remove("EKKO_PROJECT")
            .env_remove("XDG_STATE_HOME");
        match folder {
            Some(folder) => command.env_remove("EKKO_DIR").current_dir(folder),
            None => command.env("EKKO_DIR", home).current_dir(home),
        };
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn sh");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut session = Session { child, stdin, stdout, sent: 0 };
        session.request("initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}}));
        session
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.sent += 1;
        writeln!(self.stdin, "{}", request(self.sent, method, params)).unwrap();
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }

    fn call(&mut self, tool: &str, arguments: Value) -> String {
        text(&self.request("tools/call", json!({"name": tool, "arguments": arguments}))).to_string()
    }

    fn close(self) {
        drop(self.stdin);
        let mut child = self.child;
        child.wait().unwrap();
    }
}

/// Two sessions on one board: the second cannot take the task the first
/// holds while the first runs, is not steered into it by next, reads in the
/// prime who holds it, and takes it over once the first is gone.
#[test]
fn a_second_session_does_not_take_the_first_ones_task() {
    let home = temp_home();
    let mut first = Session::start(&home);
    let mut second = Session::start(&home);
    first.call("create", json!({"text": "the first session's work"}));
    first.call("set_state", json!({"items": [1], "state": "progress"}));
    second.call("create", json!({"text": "other work"}));

    let refused = second.call("set_state", json!({"items": [1], "state": "done"}));
    assert!(refused.starts_with("HELD: 1 is in progress in another Claude Code session"), "{refused}");
    let next = second.call("next", json!({}));
    assert!(next.starts_with("   2. other work\n"), "{next}");
    assert!(next.contains("In progress in other sessions, not to take up: 1 ("), "{next}");
    let prime = second.call("prime", json!({}));
    assert!(prime.contains("   1. the first session's work \u{b7} in progress \u{b7} held by "), "{prime}");
    let again = first.call("set_state", json!({"items": [1], "state": "progress"}));
    assert!(again.contains("\"notices\":[\"1 was already in progress, yours since "), "{again}");

    first.close();
    let taken = second.call("set_state", json!({"items": [1], "state": "progress"}));
    assert!(taken.contains("which is gone: now yours"), "{taken}");
    assert!(second.call("prime", json!({})).contains("   1. the first session's work \u{b7} in progress \u{b7} yours\n"));

    second.close();
    fs::remove_dir_all(&home).ok();
}

/// A session refused a task another holds waits on it instead (task 389).
/// The holder is told once, in its next reply; the write that completes the
/// task says whose wait it ended; and the session waiting is told once, in
/// its next reply, what happened and what it meant to do then.
#[test]
fn a_session_waiting_on_another_is_told_once_it_is_over() {
    let home = temp_home();
    let mut holder = Session::start(&home);
    let mut waiter = Session::start(&home);
    holder.call("create", json!({"text": "Release v0.15.0"}));
    holder.call("set_state", json!({"items": [1], "state": "progress"}));

    let refused = waiter.call("set_state", json!({"items": [1], "state": "done"}));
    assert!(refused.starts_with("HELD: ") && refused.ends_with("To be told once it is free, wait on it with until free"), "{refused}");
    let waited = waiter.call("wait", json!({"item": 1, "text": "land 380: rebase, test, push"}));
    assert!(waited.starts_with("{\"ok\":true,\"items\":[{\"id\":2,") && waited.contains("\"told\":"), "{waited}");
    let again = waiter.call("wait", json!({"item": 1, "until": "done", "text": "again"}));
    assert!(again.starts_with("This session already waits on it for that, in note 2"), "{again}");

    let told = holder.call("next", json!({}));
    let line = "waits on 1 (Release v0.15.0), which this session holds, until done (note 2): land 380: rebase, test, push\n";
    assert!(told.contains("\n\nekko: ") && told.ends_with(line), "{told}");
    assert!(!holder.call("next", json!({})).contains("ekko: "), "the holder is told once");

    let done = holder.call("set_state", json!({"items": [1], "state": "done"}));
    assert!(done.contains("\"notices\":[\"1 is done: ") && done.contains("waited on it (note 2) and is told"), "{done}");
    let over = waiter.call("next", json!({}));
    assert!(over.contains("\n\nekko: 1 (Release v0.15.0) is done, by "), "{over}");
    assert!(over.ends_with("This session waited on it (note 2) to: land 380: rebase, test, push\n"), "{over}");
    assert!(!waiter.call("next", json!({})).contains("ekko: "), "the session waiting is told once");
    let late = waiter.call("wait", json!({"item": 1, "text": "late"}));
    assert_eq!(late, "No wait was recorded: 1 is done already.\n");

    holder.close();
    waiter.close();
    fs::remove_dir_all(&home).ok();
}

/// A commit names the tasks it carries with an 'Ekko:' trailer (task 396).
/// The session taking a task up in a git repository is told the trailer, and
/// context lists the commits naming a task, read from git each time: those
/// on the branch checked out first, then the rest, named apart. A trailer
/// naming 12 is no commit of 1's, and the terminal's --context lists them
/// too.
#[test]
fn commits_name_their_tasks_and_context_lists_them() {
    let home = temp_home();
    let repo = home.join("repo");
    fs::create_dir_all(&repo).unwrap();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["-c", "user.name=ekko", "-c", "user.email=ekko@example.com", "-c", "commit.gpgsign=false", "-c", "init.defaultBranch=main"])
            .args(args)
            .env("HOME", &home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .expect("git runs");
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };
    let ekko = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_ekko"))
            .args(args)
            .current_dir(&repo)
            .env("HOME", &home)
            .env_remove("EKKO_DIR")
            .env_remove("EKKO_PROJECT")
            .output()
            .unwrap();
        assert!(out.status.success(), "ekko {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap()
    };
    git(&["init", "-q"]);
    ekko(&["init"]);

    let mut session = Session::in_folder(&home, &repo);
    session.call("create", json!({"text": "ship it"}));
    session.call("create", json!({"text": "and this"}));
    let started = session.call("set_state", json!({"items": [1], "state": "progress"}));
    assert!(started.contains("Commits for 1: end the message with the trailer 'Ekko: 1', by which context lists them"), "{started}");
    let again = session.call("set_state", json!({"items": [1], "state": "progress"}));
    assert!(!again.contains("trailer"), "told again for a task already in progress: {again}");

    let commit = |message: &str| {
        git(&["commit", "-q", "--allow-empty", "-m", message]);
        git(&["rev-parse", "--short", "HEAD"])
    };
    let first = commit("feat: the first half\n\nEkko: 1");
    let both = commit("fix: both at once\n\nEkko: 1, 2\nCo-Authored-By: someone <a@b.c>");
    let other = commit("chore: another task's\n\nEkko: 12");
    git(&["checkout", "-q", "-b", "side"]);
    let side = commit("wip: on a branch\n\nekko: #2");
    git(&["checkout", "-q", "main"]);

    let read = session.call("context", json!({"items": [1, 2]}));
    let (one, two) = read.split_once("\n   2. and this").expect("both items read");
    let line = |sha: &str| format!("\n {sha} ");
    assert!(one.contains("\nCommits\n") && one.contains(&line(&first)) && one.contains(&line(&both)), "{one}");
    assert!(one.contains("fix: both at once\n") && !one.contains(&line(&other)) && !one.contains(&line(&side)), "{one}");
    assert!(two.contains(&line(&both)) && two.contains("wip: on a branch (not on main)\n"), "{two}");
    assert!(two.find(&line(&both)) < two.find(&line(&side)), "the commits on main come first: {two}");
    session.close();

    let terminal = ekko(&["--context", "1"]);
    assert!(terminal.contains(&line(&first)) && terminal.contains(&line(&both)), "{terminal}");

    fs::remove_dir_all(&home).ok();
}
