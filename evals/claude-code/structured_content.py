"""Plan 4.5: what reaches the model when an MCP tool answers with
structuredContent beside its text, with and without an outputSchema.

Claude Code runs in print mode against a stand-in for the Anthropic API, so no
model runs and no quota is spent. The stand-in answers the first request by
calling the structured tools of probe_server.py, and the request after that is
the evidence: the tool_result blocks exactly as Claude Code sends them to the
model. Each tool's text and structure carry different markers.

    nix develop -c python3 evals/claude-code/structured_content.py

Claude Code gets a throwaway config dir and a dummy API key, so the user's
login, settings, plugins and hooks stay out of the run. CLAUDE_BIN picks the
binary (default: claude on PATH). What the run saw lands in
target/evals/claude-code/structured/: the API requests, the MCP log and Claude
Code's stream-json output.
"""

import http.server
import json
import os
import shutil
import subprocess
import sys
import threading

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from probe_server import STRUCTURED  # noqa: E402

OUT = os.path.normpath(os.path.join(HERE, "..", "..", "target", "evals", "claude-code", "structured"))
TOOLS = {f"mcp__probe__{name}": name for name in STRUCTURED}
requests = []


def answer(body):
    """The stand-in model: call every structured tool at once, then stop."""
    # Not just the last message: Claude Code 2.1.278 ends a request with a
    # message of role system, after the tool results.
    contents = [message["content"] for message in body["messages"] if isinstance(message["content"], list)]
    if any(block.get("type") == "tool_result" for content in contents for block in content):
        return [{"type": "text", "text": "MOCK_DONE"}]
    offered = {tool.get("name") for tool in body.get("tools") or []}
    if not offered.issuperset(TOOLS):
        return [{"type": "text", "text": "MOCK_NO_TOOLS"}]
    return [{"type": "tool_use", "id": f"toolu_probe_{name}", "name": tool, "input": {}} for tool, name in TOOLS.items()]


def stream(message, blocks):
    """The Messages API's server-sent events for a message of these blocks."""
    events = [{"type": "message_start", "message": {**message, "content": [], "stop_reason": None}}]
    for index, block in enumerate(blocks):
        if block["type"] == "text":
            start, delta = {"type": "text", "text": ""}, {"type": "text_delta", "text": block["text"]}
        else:
            start, delta = {**block, "input": {}}, {"type": "input_json_delta", "partial_json": json.dumps(block["input"])}
        events += [
            {"type": "content_block_start", "index": index, "content_block": start},
            {"type": "content_block_delta", "index": index, "delta": delta},
            {"type": "content_block_stop", "index": index},
        ]
    events += [
        {"type": "message_delta", "delta": {"stop_reason": message["stop_reason"], "stop_sequence": None}, "usage": {"output_tokens": 1}},
        {"type": "message_stop"},
    ]
    return "".join(f"event: {event['type']}\ndata: {json.dumps(event)}\n\n" for event in events).encode()


class Api(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def send(self, status, content_type, payload):
        self.send_response(status)
        self.send_header("content-type", content_type)
        self.send_header("content-length", str(len(payload)))
        self.send_header("request-id", f"req_mock_{len(requests)}")
        self.end_headers()
        self.wfile.write(payload)

    def refuse(self):
        error = {"type": "error", "error": {"type": "not_found_error", "message": f"the stand-in has no {self.path}"}}
        self.send(404, "application/json", json.dumps(error).encode())

    def do_GET(self):
        requests.append({"method": "GET", "path": self.path})
        self.refuse()

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("content-length") or 0)) or b"{}")
        requests.append({"method": "POST", "path": self.path, "body": body})
        if self.path.startswith("/v1/messages/count_tokens"):
            return self.send(200, "application/json", b'{"input_tokens": 1}')
        if not self.path.startswith("/v1/messages"):
            return self.refuse()
        blocks = answer(body)
        message = {
            "id": f"msg_mock_{len(requests)}", "type": "message", "role": "assistant", "model": body.get("model", "mock"),
            "content": blocks, "stop_reason": "tool_use" if blocks[0]["type"] == "tool_use" else "end_turn",
            "stop_sequence": None, "usage": {"input_tokens": 1, "output_tokens": 1},
        }
        if body.get("stream"):
            self.send(200, "text/event-stream", stream(message, blocks))
        else:
            self.send(200, "application/json", json.dumps(message).encode())


def run(claude):
    shutil.rmtree(OUT, ignore_errors=True)
    config = os.path.join(OUT, "config")
    os.makedirs(config)
    mcp = os.path.join(OUT, "mcp.json")
    with open(mcp, "w") as f:
        server = {"command": sys.executable, "args": [os.path.join(HERE, "probe_server.py")],
                  "env": {"PROBE_LOG": os.path.join(OUT, "probe.log")}}
        json.dump({"mcpServers": {"probe": server}}, f)
    api = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Api)
    threading.Thread(target=api.serve_forever, daemon=True).start()
    # Nothing of the calling session or the user's login leaks in: no
    # CLAUDE* or ANTHROPIC_* variable but the ones set here.
    env = {key: value for key, value in os.environ.items() if not key.startswith(("CLAUDE", "ANTHROPIC_"))}
    env.update({
        "CLAUDE_CONFIG_DIR": config,
        "ANTHROPIC_BASE_URL": f"http://127.0.0.1:{api.server_address[1]}",
        "ANTHROPIC_API_KEY": "sk-ant-stand-in-not-a-key",
        "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",
        "DISABLE_AUTOUPDATER": "1",
    })
    command = [claude, "-p", "--output-format", "stream-json", "--verbose", "--strict-mcp-config", "--mcp-config", mcp,
               "--allowedTools", ",".join(TOOLS), "--max-turns", "3", "Call the probe tools."]
    done = subprocess.run(command, cwd=OUT, env=env, capture_output=True, text=True, timeout=180)
    api.shutdown()
    for name, content in [("stream.jsonl", done.stdout), ("stderr.txt", done.stderr), ("requests.json", json.dumps(requests, indent=1))]:
        with open(os.path.join(OUT, name), "w") as f:
            f.write(content)
    return done


def delivered():
    """The tool_result blocks of the first request that carries them, by tool."""
    ids = {f"toolu_probe_{name}": name for name in STRUCTURED}
    for request in requests:
        for message in (request.get("body") or {}).get("messages", []):
            if message["role"] == "user" and isinstance(message["content"], list):
                found = {ids[block["tool_use_id"]]: block for block in message["content"]
                         if block.get("type") == "tool_result" and block.get("tool_use_id") in ids}
                if found:
                    return found
    return {}


def text_of(content):
    if isinstance(content, str):
        return content
    return "\n".join(part.get("text", f"<{part.get('type')} block>") for part in content or [])


def main():
    claude = os.environ.get("CLAUDE_BIN", "claude")
    version = subprocess.run([claude, "--version"], capture_output=True, text=True).stdout.strip()
    done = run(claude)
    with open(os.path.join(OUT, "probe.log")) as f:
        log = [json.loads(line) for line in f]
    initialize = next((entry["params"] for entry in log if entry.get("method") == "initialize"), {})
    called = [entry["params"]["name"] for entry in log if entry.get("method") == "tools/call"]
    print(f"{version}, MCP protocol {initialize.get('protocolVersion')}, exit {done.returncode}, "
          f"{len(requests)} API requests, tools called: {', '.join(called) or 'none'}")
    found = delivered()
    for name, probe in STRUCTURED.items():
        block = found.get(name)
        if block is None:
            print(f"  {name:<18} no tool_result reached the API")
            continue
        text = text_of(block["content"])
        seen = [f"text {'arrived' if probe['text'].split(':')[0] in text else 'DROPPED'}"]
        if "structure" in probe:
            seen.append(f"structure {'arrived' if probe['structure']['marker'] in text else 'DROPPED'}")
        schema = ", with an outputSchema" if "schema" in probe else ""
        print(f"  {name:<18} {'; '.join(seen)}{schema}\n{'':<21}the model receives: {json.dumps(block['content'])}")
    print(f"evidence in {OUT}")
    sys.exit(0 if len(found) == len(STRUCTURED) else 1)


if __name__ == "__main__":
    main()
