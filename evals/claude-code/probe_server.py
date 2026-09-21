"""Throwaway MCP server for empirical checks of Claude Code behaviour the
rigor plan wants to depend on:

  - server instructions and tool descriptions cut at 2 KB (plan 0.8)
  - resources referenced as @server:protocol://path (plan 4.1)
  - prompts run as /server:prompt commands (plan 4.2)

Every probe carries unique markers so the model's answer shows exactly what
reached it. Logs every request to probe.log next to this file, or to
$PROBE_LOG. Run it under Claude Code with an MCP config naming it alone:

  {"mcpServers": {"probe": {"command": "python3", "args": ["<this file>"],
                            "env": {"PROBE_LOG": "<a log path>"}}}}
  claude --strict-mcp-config --mcp-config <that file>

Findings so far are on the ekko board: note 167 (2026-09-15, print mode) and
task 174's notes (the interactive @ mention).
"""
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
LOG = os.environ.get("PROBE_LOG", os.path.join(HERE, "probe.log"))


def filler(prefix, until):
    out, k = [], 0
    while len(" ".join(out)) < until:
        k += 1
        out.append(f"{prefix} filler sentence number {k} adds bytes without meaning.")
    return " ".join(out)


def marked(head, before, after):
    text = head + " " + filler("a", 1850)
    text = text[:1880] + " " + before + " "
    text = text + filler("b", 2150 - len(text))
    text = text[:2130] + " " + after + " " + filler("c", 400)
    return text


INSTRUCTIONS = marked("INSTR_HEAD_7Q2", "INSTR_BEFORE_2K_K4M", "INSTR_AFTER_2K_Z9X")
DESCRIPTION = marked("TOOL_HEAD_P3L", "TOOL_BEFORE_2K_R8T", "TOOL_AFTER_2K_W5N")
# What resources/list returns; a tool call adds item://5.
LISTED = ["item://93"]


def log(entry):
    with open(LOG, "a") as f:
        f.write(json.dumps(entry) + "\n")


def reply(message_id, result):
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": message_id, "result": result}) + "\n")
    sys.stdout.flush()


def main():
    log({"started": True, "instructions_bytes": len(INSTRUCTIONS.encode()), "description_bytes": len(DESCRIPTION.encode()),
         "instr_after_offset": INSTRUCTIONS.index("INSTR_AFTER_2K_Z9X"), "tool_after_offset": DESCRIPTION.index("TOOL_AFTER_2K_W5N"),
         "instr_before_offset": INSTRUCTIONS.index("INSTR_BEFORE_2K_K4M"), "tool_before_offset": DESCRIPTION.index("TOOL_BEFORE_2K_R8T")})
    for line in sys.stdin:
        if not line.strip():
            continue
        message = json.loads(line)
        method, message_id = message.get("method"), message.get("id")
        log({"method": method, "params": message.get("params")})
        if message_id is None:
            continue
        if method == "initialize":
            version = (message.get("params") or {}).get("protocolVersion", "2025-06-18")
            reply(message_id, {
                "protocolVersion": version,
                "capabilities": {"tools": {}, "resources": {"listChanged": True}, "prompts": {}},
                "serverInfo": {"name": "probe", "version": "0"},
                "instructions": INSTRUCTIONS,
            })
        elif method == "tools/list":
            reply(message_id, {"tools": [{
                "name": "longtool",
                "description": DESCRIPTION,
                "inputSchema": {"type": "object", "properties": {}, "additionalProperties": False},
            }]})
        elif method == "tools/call":
            # The call stands for a write that adds an item: the list grows,
            # and the client is told so, to see whether it reads it again.
            reply(message_id, {"content": [{"type": "text", "text": "TOOLCALL_RESULT_G7D"}], "isError": False})
            if "item://5" not in LISTED:
                LISTED.append("item://5")
                sys.stdout.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/resources/list_changed"}) + "\n")
                sys.stdout.flush()
                log({"sent": "notifications/resources/list_changed"})
        elif method == "resources/list":
            reply(message_id, {"resources": [{"uri": uri, "name": uri.replace("://", " "), "mimeType": "text/plain"} for uri in LISTED]})
        elif method == "resources/templates/list":
            # A template reaches items the list does not name, as a board's
            # new items would be after the list was read.
            reply(message_id, {"resourceTemplates": [
                {"uriTemplate": "item://{id}", "name": "an item by id", "mimeType": "text/plain"}
            ]})
        elif method == "resources/read":
            uri = (message.get("params") or {}).get("uri")
            reply(message_id, {"contents": [{"uri": uri, "mimeType": "text/plain", "text": f"RESOURCE_BODY_J6H: the body of {uri}."}]})
        elif method == "prompts/list":
            reply(message_id, {"prompts": [{"name": "resume", "description": "Resume the probe board."}]})
        elif method == "prompts/get":
            reply(message_id, {"description": "Resume the probe board.", "messages": [
                {"role": "user", "content": {"type": "text", "text": "Reply with exactly this and nothing else: PROMPT_BODY_V2C"}}
            ]})
        elif method == "ping":
            reply(message_id, {})
        else:
            sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": message_id, "error": {"code": -32601, "message": f"Method not found: {method}"}}) + "\n")
            sys.stdout.flush()


if __name__ == "__main__":
    main()

