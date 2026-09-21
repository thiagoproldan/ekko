//! `ekko --mcp` spoken to the way a client speaks to it: the real binary,
//! newline-delimited JSON-RPC in on stdin, replies read back from stdout.

use std::collections::HashMap;
use std::fs;
use std::io::{Read as _, Write as _};
use std::path::PathBuf;
use std::process::{self, Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

fn temp_home() -> PathBuf {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let dir = std::env::temp_dir().join(format!("ekko-e2e-mcp-{}-{nanos}", process::id()));
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
    assert_eq!(tools.len(), 18);
    // The five nearly every session calls load at session start; the rest stay behind ToolSearch.
    let loaded: Vec<&str> =
        tools.iter().filter(|tool| tool["_meta"]["anthropic/alwaysLoad"] == true).map(|tool| tool["name"].as_str().unwrap()).collect();
    assert_eq!(loaded, ["context", "search", "create", "set_state", "edit"]);
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
