//! `ekko serve` (task 1102), end to end against the built binary: the page
//! from a server `ekko artifact` started, the same server for a second call,
//! a server of another version stopped and replaced on its port, an idle one
//! gone, and the requests it refuses. Each test has a home of its own, so its
//! server, its serve.json and its board are its own, and the server ends with
//! the home once its serve.json is gone.

use std::fs;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{self, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn temp_home() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let next = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("ekko-e2e-serve-{}-{nanos}-{next}", process::id()));
    fs::create_dir_all(dir.join(".ekko").join("storage")).unwrap();
    dir
}

/// ekko, run in `home` as its user's whole world: its board the default one,
/// its state under it.
fn ekko(home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ekko"));
    command.env("HOME", home).env_remove("EKKO_DIR").env_remove("EKKO_PROJECT").env_remove("XDG_STATE_HOME").current_dir(home);
    command
}

fn state(home: &Path) -> PathBuf {
    home.join(".local").join("state").join("ekko")
}

fn runtime(home: &Path) -> Value {
    serde_json::from_slice(&fs::read(state(home).join("serve.json")).unwrap()).unwrap()
}

/// An artifact on the default board, written by the artifact tool: the
/// tool's reply.
fn artifact(home: &Path) -> Value {
    let plan = "Ship the page\n\n## Goal\nWhy.\n## What is known\nFacts, in `code` and **bold**.\n## Design\nHow.\n## Risks and open questions\nNone.";
    let lines = [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}}}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "artifact", "arguments": {"text": plan, "steps": [{"key": "one", "text": "The first step"}]}}}),
    ];
    let mut child = ekko(home).arg("--mcp").env("EKKO_TERMINAL", "none").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    for line in lines {
        writeln!(stdin, "{line}").unwrap();
    }
    drop(stdin);
    let mut out = String::new();
    child.stdout.take().unwrap().read_to_string(&mut out).unwrap();
    assert!(child.wait().unwrap().success());
    let reply: Value = out.lines().map(|line| serde_json::from_str::<Value>(line).unwrap()).find(|reply| reply["id"] == 2).unwrap();
    serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
}

/// `ekko artifact <id> --no-open`: what it printed, and whether it succeeded.
fn opened(home: &Path, id: &str) -> (String, String, bool) {
    let out = ekko(home).args(["artifact", id, "--no-open"]).output().unwrap();
    (String::from_utf8_lossy(&out.stdout).trim().to_string(), String::from_utf8_lossy(&out.stderr).into_owned(), out.status.success())
}

/// One request to 127.0.0.1:`port`, naming `host`: the status code and the body.
fn get(port: u16, path: &str, host: &str) -> (u16, String) {
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(stream, "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").unwrap();
    let mut answer = String::new();
    stream.read_to_string(&mut answer).unwrap();
    let (head, body) = answer.split_once("\r\n\r\n").unwrap();
    (head.split(' ').nth(1).unwrap().parse().unwrap(), body.to_string())
}

/// One request to 127.0.0.1:`port`, naming `host`: the head and the body,
/// as bytes.
fn fetch(port: u16, path: &str, host: &str) -> (String, Vec<u8>) {
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(stream, "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").unwrap();
    let mut answer = Vec::new();
    stream.read_to_end(&mut answer).unwrap();
    let end = answer.windows(4).position(|window| window == b"\r\n\r\n").unwrap();
    (String::from_utf8_lossy(&answer[..end]).into_owned(), answer[end + 4..].to_vec())
}

/// The port and path of an address the server gives.
fn split(address: &str) -> (u16, String) {
    let rest = address.strip_prefix("http://127.0.0.1:").unwrap_or_else(|| panic!("not the server's address: {address}"));
    let (port, path) = rest.split_once('/').unwrap();
    (port.parse().unwrap(), format!("/{path}"))
}

/// Whether `pid` runs, a zombie not counted.
fn alive(pid: u64) -> bool {
    fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| !stat.rsplit(')').next().unwrap_or_default().trim_start().starts_with('Z'))
}

/// Waits up to `seconds` for `done`.
fn until(seconds: u64, mut done: impl FnMut() -> bool) -> bool {
    let began = Instant::now();
    while began.elapsed() < Duration::from_secs(seconds) {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    done()
}

/// A home of its own for one test: when the test ends, failed or not, its
/// server is stopped and the folder taken away.
struct Home(PathBuf);

impl Home {
    fn new() -> Home {
        Home(temp_home())
    }
}

impl std::ops::Deref for Home {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = ekko(&self.0).args(["serve", "--stop"]).output();
        fs::remove_dir_all(&self.0).ok();
    }
}

/// The artifact tool's reply gives the page from a server it started; with
/// that one stopped, `ekko artifact` starts another, on the same port, and
/// its page and version script come from the board. A second call finds the
/// same server, and the file page is written all the same. With serve.json
/// gone, the server goes too.
#[test]
fn ekko_artifact_opens_the_page_from_a_server_it_started_and_a_second_call_reuses_it() {
    let home = Home::new();
    let written = artifact(&home);
    let (port, path) = split(written["page"].as_str().unwrap());
    let uid = path.strip_prefix("/default/").and_then(|file| file.strip_suffix(".html")).unwrap().to_string();
    assert!(written["unserved"].is_null(), "{written}");
    let first = runtime(&home);
    assert_eq!((first["port"].as_u64(), first["version"].as_str()), (Some(u64::from(port)), Some(VERSION)), "{first}");
    assert!(alive(first["pid"].as_u64().unwrap()));

    let stopped = ekko(&home).args(["serve", "--stop"]).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&stopped.stdout).trim(), format!("Stopped the server on 127.0.0.1:{port}"));
    assert!(until(5, || !alive(first["pid"].as_u64().unwrap())), "the server outlived --stop");

    let (address, stderr, ok) = opened(&home, "1");
    assert!(ok, "{stderr}");
    assert_eq!(address, format!("http://127.0.0.1:{port}/default/{uid}.html"), "the last port, taken again: {stderr}");
    let started = runtime(&home);
    assert_ne!(started["pid"], first["pid"]);
    assert_eq!(started["token"], first["token"], "the token outlives its server, so a tab open across the replacement still writes");
    assert!(alive(started["pid"].as_u64().unwrap()));
    let mode = fs::metadata(state(&home).join("serve.json")).map(|meta| std::os::unix::fs::PermissionsExt::mode(&meta.permissions()) & 0o777).unwrap();
    assert_eq!(mode, 0o600, "the token is the user's alone");

    let (code, page) = get(port, &path, &format!("127.0.0.1:{port}"));
    assert_eq!(code, 200, "{page}");
    assert!(page.contains("<h1>Ship the page</h1>"), "{page}");
    assert!(page.contains(&format!("script.src = \"{uid}.js?t=\"")), "the page polls the script beside it");
    let (code, script) = get(port, &format!("/default/{uid}.js?t=1"), &format!("localhost:{port}"));
    assert_eq!(code, 200);
    let file = home.join(".ekko").join("artifacts");
    assert_eq!(script, fs::read_to_string(file.join(format!("{uid}.js"))).unwrap(), "the version the file page has");
    assert!(fs::read_to_string(file.join(format!("{uid}.html"))).unwrap().contains("<h1>Ship the page</h1>"));
    // The fonts the page names come from the server, the very ones written
    // beside the file page (task 1152).
    let fonts: Vec<&str> = page.split("url(\"fonts/").skip(1).filter_map(|rest| rest.split('"').next()).collect();
    assert_eq!(fonts.len(), 3, "{page}");
    for font in fonts {
        let (head, body) = fetch(port, &format!("/default/fonts/{font}"), &format!("127.0.0.1:{port}"));
        assert!(head.starts_with("HTTP/1.1 200 ") && head.contains("Content-Type: font/woff2"), "{font}: {head}");
        assert_eq!(body, fs::read(file.join("fonts").join(font)).unwrap(), "{font}");
    }

    let (again, stderr, ok) = opened(&home, "1");
    assert!(ok, "{stderr}");
    assert_eq!(again, address);
    assert_eq!(runtime(&home), started, "the same server, untouched");
    let log = fs::read_to_string(state(&home).join("serve.log")).unwrap();
    assert_eq!(log.matches("asked to start a server").count(), 2, "by the tool, then by the first ekko artifact, and no more: {log}");

    fs::remove_file(state(&home).join("serve.json")).unwrap();
    assert!(until(10, || !alive(started["pid"].as_u64().unwrap())), "a server serve.json no longer names stops");
}

/// A server of another version on the port serve.json names is asked to stop,
/// with the token the file holds, and an ekko of this version serves the page
/// on that port once it is free.
#[test]
fn a_server_of_another_version_is_stopped_and_replaced_on_its_port() {
    let home = Home::new();
    artifact(&home);
    assert!(ekko(&home).args(["serve", "--stop"]).output().unwrap().status.success());

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let older = json!({"pid": 1, "port": port, "version": "0.0.1", "token": "the-older-token"});
    fs::write(state(&home).join("serve.json"), older.to_string()).unwrap();
    let (told, stopped) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut head = Vec::new();
            for line in BufReader::new(stream.try_clone().unwrap()).lines() {
                let line = line.unwrap();
                if line.is_empty() {
                    break;
                }
                head.push(line);
            }
            let answer = |stream: &mut TcpStream, body: &str| {
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            };
            if head[0].starts_with("GET /status ") {
                answer(&mut stream, &json!({"ekko": "0.0.1", "pid": 1}).to_string());
            } else if head[0].starts_with("POST /stop ") {
                answer(&mut stream, "stopping\n");
                // Its port is free once this returns.
                let _ = told.send(head.iter().find_map(|line| line.strip_prefix("X-Ekko-Token: ").map(str::to_string)));
                return;
            }
        }
    });

    let (address, stderr, ok) = opened(&home, "1");
    assert!(ok, "{stderr}");
    let token = stopped.recv_timeout(Duration::from_secs(10));
    assert_eq!(token, Ok(Some("the-older-token".to_string())), "the older server is asked to stop, with its token");
    assert_eq!(split(&address).0, port, "the older server's port, taken again: {address}");
    let (code, status) = get(port, "/status", &format!("127.0.0.1:{port}"));
    assert_eq!(code, 200);
    assert_eq!(serde_json::from_str::<Value>(&status).unwrap()["ekko"], VERSION);
    assert_eq!(runtime(&home)["version"], VERSION);
    let token = runtime(&home)["token"].as_str().unwrap().to_string();
    assert!(token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit()), "a token ekko did not draw is not kept: {token}");
}

/// A server with no request for its idle time stops, and says so in its log.
#[test]
fn an_idle_server_stops() {
    let home = Home::new();
    let mut server = ekko(&home).args(["serve", "--idle", "1"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    assert!(until(5, || state(&home).join("serve.json").exists()), "the server never wrote serve.json");
    let (code, _) = get(runtime(&home)["port"].as_u64().unwrap() as u16, "/status", &format!("127.0.0.1:{}", runtime(&home)["port"]));
    assert_eq!(code, 200);
    let mut exited = None;
    assert!(until(10, || {
        exited = server.try_wait().unwrap();
        exited.is_some()
    }), "the server is still up after 10 s with nothing asked of it");
    assert!(exited.unwrap().success());
    let log = fs::read_to_string(state(&home).join("serve.log")).unwrap();
    assert!(log.contains("stopped: no request for 1 s"), "{log}");
}

/// The server answers only a Host that names it, only what it serves, and
/// stops only with the token; and one server runs at a time.
#[test]
fn the_server_refuses_a_foreign_host_and_what_it_does_not_serve() {
    let home = Home::new();
    let written = artifact(&home);
    let (port, path) = split(written["page"].as_str().unwrap());
    for host in ["evil.example", "evil.example:80", "127.0.0.1:1", "[::1]"] {
        let (code, body) = get(port, &path, &host.replace("80", &port.to_string()));
        assert_eq!(code, 403, "{host}: {body}");
    }
    let host = format!("127.0.0.1:{port}");
    assert_eq!(get(port, "/default/18da0000000000-00000.html", &host).0, 404);
    assert_eq!(get(port, "/project/nothing/x.html", &host).0, 404);
    assert_eq!(get(port, &path.replace(".html", ".css"), &host).0, 404);
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
    write!(stream, "POST /stop HTTP/1.1\r\nHost: {host}\r\nX-Ekko-Token: not-it\r\nConnection: close\r\n\r\n").unwrap();
    let mut answer = String::new();
    stream.read_to_string(&mut answer).unwrap();
    assert!(answer.starts_with("HTTP/1.1 403 "), "{answer}");
    assert_eq!(get(port, "/status", &host).0, 200, "still up");

    let second = ekko(&home).args(["serve"]).output().unwrap();
    assert!(!second.status.success());
    assert!(String::from_utf8_lossy(&second.stderr).contains(&format!("another ekko serve runs, on 127.0.0.1:{port}")), "{}", String::from_utf8_lossy(&second.stderr));
    let log = fs::read_to_string(state(&home).join("serve.log")).unwrap();
    assert!(log.contains("refused GET ") && log.contains("Host \"evil.example\""), "{log}");

    // A task that is no artifact has no page, and an artifact in the trash has none left.
    let task: Value = serde_json::from_slice(&ekko(&home).args(["--json", "--task", "A plain task"]).output().unwrap().stdout).unwrap();
    assert_eq!(get(port, &format!("/default/{}.html", task["item"]["uid"].as_str().unwrap()), &host).0, 404);
    assert_eq!(get(port, &path, &host).0, 200);
    assert!(ekko(&home).args(["--delete", "1"]).output().unwrap().status.success());
    assert_eq!(get(port, &path, &host).0, 404);
}

/// A board the server would not find by name -- a copy EKKO_DIR points at --
/// keeps its file page, and is told why.
#[test]
fn a_board_the_server_cannot_name_keeps_its_file_page() {
    let home = Home::new();
    artifact(&home);
    let copy = home.join("copy");
    fs::create_dir_all(&copy).unwrap();
    let copied = Command::new("cp").arg("-r").arg(home.join(".ekko")).arg(&copy).status().unwrap();
    assert!(copied.success());
    let out = ekko(&home).args(["artifact", "1", "--no-open"]).env("EKKO_DIR", copy.join(".ekko")).output().unwrap();
    assert!(out.status.success());
    let page = String::from_utf8_lossy(&out.stdout).trim().to_string();
    assert!(page.starts_with(&copy.join(".ekko").join("artifacts").display().to_string()) && page.ends_with(".html"), "{page}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("this board is neither"), "{}", String::from_utf8_lossy(&out.stderr));
}

/// One request, written whole, to 127.0.0.1:`port`, from this process: the
/// whole answer.
fn exchange(port: u16, request: &str) -> String {
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    let mut answer = String::new();
    stream.read_to_string(&mut answer).unwrap();
    answer
}

/// The same request sent by bash through /dev/tcp: by a copy of bash named
/// claude, which is a Claude Code process as ekko tells one, or else by a
/// subshell its bash left behind, which so passed to init or a subreaper
/// before it connected, out of every session's process tree, this test's
/// included when Claude Code runs it. Its whole answer.
fn from_bash(home: &Path, port: u16, request: &str, as_claude: bool) -> String {
    let out = home.join(if as_claude { "answer-claude" } else { "answer-person" });
    let script = format!("exec 3<>/dev/tcp/127.0.0.1/{port}; printf '%s' \"$REQUEST\" >&3; cat <&3 > \"$OUT.part\"; mv \"$OUT.part\" \"$OUT\"");
    let mut command = if as_claude {
        let bash = String::from_utf8(Command::new("sh").args(["-c", "command -v bash"]).output().unwrap().stdout).unwrap();
        let claude = home.join("claude");
        // Copied once: the copy keeps the store's read-only mode, which
        // refuses a second copy over it.
        if !claude.exists() {
            fs::copy(bash.trim(), &claude).unwrap();
        }
        let mut command = Command::new(claude);
        command.arg("-c").arg(script);
        command
    } else {
        let mut command = Command::new("bash");
        command.arg("-c").arg(format!("(sleep 0.5; {script}) > /dev/null 2>&1 &"));
        command
    };
    command.env("REQUEST", request).env("OUT", &out);
    // A file just copied is busy to run while another test's fork, made
    // before its exec, still holds the copy open: run it a moment later.
    let mut tries = 0;
    let status = loop {
        match command.status() {
            Err(error) if error.kind() == std::io::ErrorKind::ExecutableFileBusy && tries < 100 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(20));
            }
            other => break other.unwrap(),
        }
    };
    assert!(status.success());
    assert!(until(10, || out.exists()), "bash never got its answer");
    fs::read_to_string(out).unwrap()
}

/// A write is the user's only from the page opened through the redirect
/// file, and from a process outside every Claude Code session (task 1103):
/// the file hands the browser the token, which it trades for the cookie;
/// the same write under Claude Code, from another Origin, with another Host
/// or without the cookie is refused; and the page runs only its own scripts.
#[test]
fn a_write_is_the_users_only_from_the_page_opened_with_the_token() {
    let home = Home::new();
    let written = artifact(&home);
    let (port, path) = split(written["page"].as_str().unwrap());
    let (address, stderr, ok) = opened(&home, "1");
    assert!(ok, "{stderr}");
    let token = runtime(&home)["token"].as_str().unwrap().to_string();
    let redirect = state(&home).join("serve-open.html");
    assert!(fs::read_to_string(&redirect).unwrap().contains(&format!("url={address}?token={token}")));
    let mode = fs::metadata(&redirect).map(|meta| std::os::unix::fs::PermissionsExt::mode(&meta.permissions()) & 0o777).unwrap();
    assert_eq!(mode, 0o600);

    let host = format!("127.0.0.1:{port}");
    let traded = exchange(port, &format!("GET {path}?token={token} HTTP/1.1\r\nHost: {host}\r\n\r\n"));
    assert!(traded.starts_with("HTTP/1.1 303 "), "{traded}");
    assert!(traded.contains(&format!("\r\nLocation: {path}\r\n")), "{traded}");
    let cookie = format!("ekko_{port}={token}");
    assert!(traded.contains(&format!("\r\nSet-Cookie: {cookie}; HttpOnly; SameSite=Strict; Path=/\r\n")), "{traded}");
    let page = exchange(port, &format!("GET {path} HTTP/1.1\r\nHost: {host}\r\n\r\n"));
    let policy = page.lines().find_map(|line| line.strip_prefix("Content-Security-Policy: ")).unwrap_or_else(|| panic!("no CSP: {page}"));
    let nonce = policy.split("'nonce-").nth(1).and_then(|rest| rest.split('\'').next()).unwrap();
    assert!(page.contains(&format!("<script nonce=\"{nonce}\">")) && !page.contains("<script>"), "every script of the page carries the nonce");
    assert!(page.contains("fetch(\"/api/who\""), "the page asks whom it writes as");

    let who = |origin: &str, host: &str, cookie: &str| format!("POST /api/who HTTP/1.1\r\nHost: {host}\r\nOrigin: {origin}\r\nCookie: {cookie}\r\nContent-Length: 0\r\n\r\n");
    let origin = format!("http://127.0.0.1:{port}");
    let person = from_bash(&home, port, &who(&origin, &host, &cookie), false);
    assert!(person.starts_with("HTTP/1.1 200 ") && person.ends_with("{\"person\":true}"), "{person}");
    let session = from_bash(&home, port, &who(&origin, &host, &cookie), true);
    assert!(session.starts_with("HTTP/1.1 403 ") && session.contains("runs under Claude Code"), "{session}");
    for (request, refusal) in [
        (who("http://127.0.0.1:1", &host, &cookie), "Origin"),
        (who("null", &host, &cookie), "Origin"),
        (who(&origin, "evil.example", &cookie), "Host"),
        (who(&origin, &host, "other=1"), "ekko artifact"),
        (who(&origin, &host, &format!("ekko_{port}=not-the-token")), "ekko artifact"),
    ] {
        let answer = exchange(port, &request);
        assert!(answer.starts_with("HTTP/1.1 403 ") && answer.contains(refusal), "{request}: {answer}");
    }
    let log = fs::read_to_string(state(&home).join("serve.log")).unwrap();
    assert!(log.contains("refused POST /api/who: process ") && log.contains("runs under Claude Code"), "{log}");
}

/// The next event on a stream of /events, comments and other fields passed
/// over: its name and data, or none when nothing came within `wait`.
fn next_event(reader: &mut BufReader<TcpStream>, wait: Duration) -> Option<(String, String)> {
    reader.get_ref().set_read_timeout(Some(wait)).unwrap();
    let (mut name, mut data) = (String::new(), String::new());
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() && !data.is_empty() {
            return Some((name, data));
        }
        if let Some(value) = line.strip_prefix("event: ") {
            name = value.to_string();
        } else if let Some(value) = line.strip_prefix("data: ") {
            data = value.to_string();
        }
    }
}

/// A stream of /events opens with the version of the page the server served,
/// sends nothing on a write the page does not show, and carries the page's
/// new version within a second of a write from the CLI that changes it (task
/// 1104).
#[test]
fn a_write_from_the_cli_reaches_an_open_stream_of_events_within_a_second() {
    let home = Home::new();
    let written = artifact(&home);
    let (port, path) = split(written["page"].as_str().unwrap());
    let host = format!("127.0.0.1:{port}");
    assert_eq!(get(port, &path, &host).0, 200, "the page, served once, which the stream then follows");

    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
    write!(stream, "GET /events HTTP/1.1\r\nHost: {host}\r\nAccept: text/event-stream\r\n\r\n").unwrap();
    let mut reader = BufReader::new(stream);
    reader.get_ref().set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut head = String::new();
    while !head.ends_with("\r\n\r\n") {
        assert!(reader.read_line(&mut head).unwrap() > 0, "{head}");
    }
    assert!(head.starts_with("HTTP/1.1 200 ") && head.contains("Content-Type: text/event-stream\r\n"), "{head}");
    let (name, data) = next_event(&mut reader, Duration::from_secs(5)).expect("the page's version, at once");
    let (page, before) = data.split_once(' ').unwrap();
    assert_eq!((name.as_str(), page), ("version", path.as_str()), "{data}");

    assert!(ekko(&home).args(["--task", "Not on the page"]).output().unwrap().status.success());
    assert_eq!(next_event(&mut reader, Duration::from_millis(800)), None, "a write the page does not show changes no version");

    let began = Instant::now();
    let checked = ekko(&home).args(["--check", "1"]).output().unwrap();
    assert!(checked.status.success(), "{}", String::from_utf8_lossy(&checked.stderr));
    let (name, data) = next_event(&mut reader, Duration::from_secs(1)).expect("the new version, within a second");
    let took = began.elapsed();
    assert!(took < Duration::from_secs(1), "{took:?}");
    let (page, after) = data.split_once(' ').unwrap();
    assert_eq!((name.as_str(), page), ("version", path.as_str()), "{data}");
    assert_ne!(after, before, "the artifact done is a new version");
}

/// A comment from the page is the person's note on the artifact, holding
/// what it is about (task 1105): made on the plan's version, on words that
/// version holds. A stale version, words the plan lacks, or a write under
/// Claude Code are refused, and write nothing.
#[test]
fn a_comment_from_the_page_is_the_persons_note_on_the_artifact() {
    let home = Home::new();
    let written = artifact(&home);
    let (port, path) = split(written["page"].as_str().unwrap());
    let token = runtime(&home)["token"].as_str().unwrap().to_string();
    let post = |body: &Value| {
        let body = body.to_string();
        format!(
            "POST /api/comment HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://127.0.0.1:{port}\r\nCookie: ekko_{port}={token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
    };
    let comment = |version: u32, exact: &str| json!({"page": path, "text": "Which facts?", "comment": {"version": version, "quote": {"exact": exact, "prefix": "## What is known\n", "suffix": "\n## Design", "section": "What is known"}}});
    let notes = || -> Vec<Value> {
        let board: Value = serde_json::from_slice(&fs::read(home.join(".ekko").join("storage").join("storage.json")).unwrap()).unwrap();
        board.as_object().unwrap().values().filter(|item| item.get("comment").is_some()).cloned().collect()
    };

    for (body, as_claude, refusal) in [
        (comment(2, "Facts"), false, "HTTP/1.1 409 "),
        (comment(1, "Fiction."), false, "HTTP/1.1 409 "),
        (comment(1, "Facts"), true, "HTTP/1.1 403 "),
    ] {
        let answer = from_bash(&home, port, &post(&body), as_claude);
        assert!(answer.starts_with(refusal), "{body}: {answer}");
        let _ = fs::remove_file(home.join(if as_claude { "answer-claude" } else { "answer-person" }));
    }
    assert!(notes().is_empty(), "a refused comment wrote nothing");

    let answer = from_bash(&home, port, &post(&comment(1, "Facts")), false);
    assert!(answer.starts_with("HTTP/1.1 200 "), "{answer}");
    let notes = notes();
    assert_eq!(notes.len(), 1, "{notes:?}");
    let note = &notes[0];
    assert_eq!(note["description"], "Which facts?");
    assert_eq!(note["attachedTo"], path.trim_start_matches("/default/").trim_end_matches(".html"));
    assert_eq!(note["comment"]["version"], 1);
    assert_eq!(note["comment"]["quote"]["exact"], "Facts");
    assert!(note["comment"].get("sent").is_none(), "pending until a review sends it");
    let by = note["createdBy"].as_object().map(|by| by.keys().cloned().collect::<Vec<_>>());
    assert!(by.is_none_or(|keys| !keys.iter().any(|key| key == "pid")), "written by the person, no process: {note}");

    // Words selected on the page come without the Markdown around them.
    let _ = fs::remove_file(home.join("answer-person"));
    let answer = from_bash(&home, port, &post(&comment(1, "Facts, in code and bold.")), false);
    assert!(answer.starts_with("HTTP/1.1 200 "), "the words as the page shows them: {answer}");
}

/// The person edits and deletes their comment from the page (task 1213):
/// its text, theme and color change, and a deletion puts it in the trash;
/// Claude is refused, as are an unknown color, a body that mixes a new
/// comment with an edit, and a comment already deleted, each writing
/// nothing.
#[test]
fn the_person_edits_and_deletes_their_comment_from_the_page() {
    let home = Home::new();
    let written = artifact(&home);
    let (port, path) = split(written["page"].as_str().unwrap());
    let token = runtime(&home)["token"].as_str().unwrap().to_string();
    let post = |to: &str, body: &Value| {
        let body = body.to_string();
        format!(
            "POST {to} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://127.0.0.1:{port}\r\nCookie: ekko_{port}={token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
    };
    let ask = |to: &str, body: Value, as_claude: bool| {
        let _ = fs::remove_file(home.join(if as_claude { "answer-claude" } else { "answer-person" }));
        from_bash(&home, port, &post(to, &body), as_claude)
    };
    let comment = || -> Value {
        let board: Value = serde_json::from_slice(&fs::read(home.join(".ekko").join("storage").join("storage.json")).unwrap()).unwrap();
        board.as_object().unwrap().values().find(|item| item.get("comment").is_some()).cloned().unwrap_or_default()
    };
    let quote = json!({"exact": "Facts", "prefix": "", "suffix": "", "section": "What is known"});
    let answer = ask("/api/comment", json!({"page": path, "text": "Which facts?", "comment": {"version": 1, "quote": quote, "theme": "Question", "color": "blue"}}), false);
    assert!(answer.starts_with("HTTP/1.1 200 "), "{answer}");
    let uid = comment()["uid"].as_str().unwrap().to_string();
    assert_eq!((&comment()["comment"]["theme"], &comment()["comment"]["color"]), (&json!("Question"), &json!("blue")));

    for (to, body, as_claude, refusal) in [
        ("/api/comment/edit", json!({"page": path, "uid": uid, "text": "Claude's words"}), true, "HTTP/1.1 403 "),
        ("/api/comment/edit", json!({"page": path, "uid": uid, "color": "teal"}), false, "HTTP/1.1 409 "),
        ("/api/comment/edit", json!({"page": path, "uid": uid, "comment": {"version": 1}}), false, "HTTP/1.1 400 "),
        ("/api/comment/delete", json!({"page": path, "uid": uid, "text": "and words"}), false, "HTTP/1.1 400 "),
        ("/api/comment/delete", json!({"page": path, "uid": uid}), true, "HTTP/1.1 403 "),
    ] {
        let answer = ask(to, body.clone(), as_claude);
        assert!(answer.starts_with(refusal), "{to} {body}: {answer}");
    }
    assert_eq!(comment()["description"], "Which facts?", "a refused edit wrote nothing");
    assert!(comment().get("trashed").is_none(), "a refused deletion wrote nothing");

    let answer = ask("/api/comment/edit", json!({"page": path, "uid": uid, "text": "Which facts, exactly?", "theme": "Dúvida", "color": "pink"}), false);
    assert!(answer.starts_with("HTTP/1.1 200 "), "{answer}");
    let edited = comment();
    assert_eq!((&edited["description"], &edited["comment"]["theme"], &edited["comment"]["color"]), (&json!("Which facts, exactly?"), &json!("Dúvida"), &json!("pink")));
    assert_eq!(edited["comment"]["quote"]["exact"], "Facts", "an edit keeps the words it is on");

    let answer = ask("/api/comment/delete", json!({"page": path, "uid": uid}), false);
    assert!(answer.starts_with("HTTP/1.1 200 "), "{answer}");
    assert!(comment().get("trashed").is_some(), "in the trash");
    let answer = ask("/api/comment/delete", json!({"page": path, "uid": uid}), false);
    assert!(answer.starts_with("HTTP/1.1 409 "), "a comment deleted already: {answer}");
}

/// `ekko --mcp` on the default board of `home`, kept running as a session's
/// is, whose menu opens with a script standing in for the terminal: it says
/// the menu is up and waits, so ask's call waits on the board for its
/// answers, as it does while the user has the menu open.
struct Session {
    child: process::Child,
    stdin: Option<process::ChildStdin>,
    messages: std::sync::mpsc::Receiver<Value>,
    seen: Vec<Value>,
}

impl Session {
    fn start(home: &Path) -> Session {
        let script = home.join("terminal.sh");
        // The arguments end in `<ekko> --menu <file>`; the pid file beside
        // the file is how the menu says it is up.
        let body = "spec=; prev=\nfor a in \"$@\"; do\n  [ \"$prev\" = --menu ] && spec=$a\n  prev=$a\ndone\necho $$ > \"${spec%.json}.pid\"\necho $$ >> \"$(dirname \"$0\")/menus\"\nexec sleep 60\n";
        fs::write(&script, format!("#!/bin/sh\n{body}")).unwrap();
        fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let mut child = ekko(home)
            .arg("--mcp")
            .env("EKKO_TERMINAL", &script)
            .env_remove("CLAUDECODE")
            .env("XDG_RUNTIME_DIR", home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, messages) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(serde_json::from_str(&line).unwrap()).is_err() {
                    break;
                }
            }
        });
        let stdin = child.stdin.take();
        let mut session = Session { child, stdin, messages, seen: Vec::new() };
        session.send(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-11-25", "capabilities": {}}}));
        session.reply(1, Duration::from_secs(10)).expect("initialized");
        session
    }

    fn send(&mut self, message: Value) {
        writeln!(self.stdin.as_mut().unwrap(), "{message}").unwrap();
    }

    /// The reply to request `id`, if it comes within `wait`.
    fn reply(&mut self, id: u64, wait: Duration) -> Option<Value> {
        let deadline = Instant::now() + wait;
        loop {
            if let Some(found) = self.seen.iter().find(|message| message["id"] == id && message.get("method").is_none()) {
                return Some(found.clone());
            }
            let left = deadline.saturating_duration_since(Instant::now());
            match self.messages.recv_timeout(left) {
                Ok(message) => self.seen.push(message),
                Err(_) => return None,
            }
        }
    }

    /// Asks the user, through ask, to approve the plan of artifact 1.
    fn ask_approval(&mut self, id: u64) {
        let options = json!([
            {"label": "Approve it", "recommended": true, "why": "its steps become tasks", "example": "one task a step"},
            {"label": "Not yet", "why": "nothing changes", "example": "the plan stays as it is"}
        ]);
        let question = json!({"text": "Approve the plan?", "explain": "What the approval does.", "approve": 1, "options": options});
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": "ask", "arguments": {"questions": [question]}}}));
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        drop(self.stdin.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A review from the page answers the question a session's ask waits on
/// (task 1106): Request changes returns the call with the other answer,
/// naming the review, and closes the menu; Approve returns it with the
/// answer that approves, which makes the plan's tasks. A review under
/// Claude Code is refused and writes nothing.
#[test]
fn a_review_from_the_page_returns_the_ask_that_waits_on_it() {
    let home = Home::new();
    let written = artifact(&home);
    let (port, path) = split(written["page"].as_str().unwrap());
    let token = runtime(&home)["token"].as_str().unwrap().to_string();
    let post = |body: &Value| {
        let body = body.to_string();
        format!(
            "POST /api/review HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://127.0.0.1:{port}\r\nCookie: ekko_{port}={token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
    };
    let send = |body: Value, as_claude: bool| {
        let _ = fs::remove_file(home.join(if as_claude { "answer-claude" } else { "answer-person" }));
        from_bash(&home, port, &post(&body), as_claude)
    };
    let board = || -> Value { serde_json::from_slice(&fs::read(home.join(".ekko").join("storage").join("storage.json")).unwrap()).unwrap() };
    let reviews = || board().as_object().unwrap().values().filter(|item| item.get("review").is_some()).count();
    let menus = || fs::read_to_string(home.join("menus")).unwrap_or_default().lines().map(str::to_string).collect::<Vec<_>>();
    let up = |n: usize| until(10, || menus().len() == n);
    let answer = |reply: &Value| -> Value {
        let text = reply["result"]["content"][0]["text"].as_str().unwrap();
        serde_json::from_str::<Value>(text).unwrap_or_else(|_| panic!("{text}"))["answers"][0]["answer"].clone()
    };

    let mut session = Session::start(&home);
    session.ask_approval(2);
    assert!(up(1), "the menu came up");
    assert_eq!(session.reply(2, Duration::from_millis(1500)), None, "ask waits on the menu");

    let refused = send(json!({"page": path, "verdict": "changes", "version": 1, "text": "Claude's words"}), true);
    assert!(refused.starts_with("HTTP/1.1 403 "), "{refused}");
    assert_eq!(reviews(), 0, "a refused review wrote nothing");

    let sent = send(json!({"page": path, "verdict": "changes", "version": 1, "text": "Split the step."}), false);
    assert!(sent.starts_with("HTTP/1.1 200 "), "{sent}");
    let review: Value = serde_json::from_str(sent.split_once("\r\n\r\n").unwrap().1).unwrap();
    let returned = session.reply(2, Duration::from_secs(5)).expect("ask returned once the page answered");
    assert_eq!(answer(&returned), json!(format!("Not yet \u{2014} note: changes requested on the artifact's page, in review {}: Split the step.", review["id"])));
    let menu = menus()[0].clone();
    assert!(until(5, || fs::metadata(format!("/proc/{menu}")).is_err()), "the menu closed");

    session.ask_approval(3);
    assert!(up(2), "the menu came up again");
    let sent = send(json!({"page": path, "verdict": "approve", "version": 1}), false);
    assert!(sent.starts_with("HTTP/1.1 200 "), "{sent}");
    let review: Value = serde_json::from_str(sent.split_once("\r\n\r\n").unwrap().1).unwrap();
    assert!(review["notices"].as_array().unwrap().iter().any(|notice| notice.as_str().unwrap().contains("plan is approved")), "{review}");
    let returned = session.reply(3, Duration::from_secs(5)).expect("ask returned once the page approved");
    assert_eq!(answer(&returned), json!(format!("Approve it \u{2014} note: approved on the artifact's page, in review {}", review["id"])));
    let step = board()["1"]["artifact"]["steps"][0].clone();
    assert!(step["task"].is_string(), "the approval made the step's task: {step}");
    let log = fs::read_to_string(state(&home).join("serve.log")).unwrap();
    assert!(log.contains("POST /api/review: approve in note ") && log.contains("refused POST /api/review: process "), "{log}");
}
