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
        .map(|line| {
            let reply: Value = serde_json::from_str(line).unwrap_or_else(|e| panic!("not one JSON message per line: {e}: {line}"));
            (reply["id"].to_string(), reply)
        })
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
    assert_eq!(tools.len(), 16);
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
    assert_eq!(discover["capabilities"], json!({"tools": {}}));

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
