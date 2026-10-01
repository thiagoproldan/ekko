//! `ekko serve` (task 1102): one local HTTP server per user, started on
//! demand, serving each artifact's page from its board. `ekko artifact` and
//! the artifact tool start it when none runs and give its address; the page
//! written to a file stays wherever the server cannot start or cannot name
//! the board.
//!
//! The server keeps `serve.json` in ekko's state directory: its pid, its
//! port, its version and a token. A client trusts the file only as far as
//! `GET /status` answers there, and asks a server of another version to stop,
//! with the token, before it starts its own, as Bazel's client replaces a
//! server of another version. The file outlives the server, so the next one
//! takes the same port again and a tab left open finds it. The server stops
//! after an idle hour, and as soon as the file names another server or is
//! gone, which is how a test's server ends with its home.
//!
//! It listens on 127.0.0.1 only, answers only a Host naming that address or
//! localhost at its port, which stops DNS rebinding, and only sockets the
//! user owns, read from /proc/net/tcp (decision 1128). Each connection gets a
//! thread and one request, whose head httparse reads.

use std::fs;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::directory::Location;

/// How long a server waits for a request before it stops.
pub const IDLE: Duration = Duration::from_secs(60 * 60);

/// What runs: the version a server serves, and a client wants.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The runtime file, in ekko's state directory.
const RUNTIME: &str = "serve.json";

/// The file a running server holds a lock on, so that one runs.
const LOCK: &str = "serve.lock";

/// The server's log: when it started and stopped, and what it refused.
const LOG: &str = "serve.log";

/// Past this size the log starts over when a server starts.
const LOG_MOST: u64 = 256 * 1024;

/// The header that carries the token, which `POST /stop` needs.
const TOKEN: &str = "X-Ekko-Token";

/// How long a client waits on one answer from a server.
const ASKING: Duration = Duration::from_secs(2);

/// How long a client waits for a server it started to answer, or for one it
/// asked to stop to let go of its port.
const STARTING: Duration = Duration::from_secs(5);

/// How often a server checks it is still the one `serve.json` names, and
/// still wanted.
const WATCH: Duration = Duration::from_secs(2);

/// How long a connection may take to send its request, or to take the answer.
const TIMEOUT: Duration = Duration::from_secs(10);

/// The longest request head read.
const HEAD_MOST: usize = 16 * 1024;

/// The most connections answered at once; one more gets 503.
const CONNECTIONS_MOST: usize = 64;

/// What `serve.json` holds: the server that runs, or the last one that did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Runtime {
    pub pid: u32,
    pub port: u16,
    pub version: String,
    /// Asks the server to stop. Only the user's processes can read it: the
    /// file is theirs alone.
    pub token: String,
}

/// Where the server keeps its files: ekko's state directory.
fn dir(home: &Path) -> PathBuf {
    crate::agent::state_dir(home)
}

/// What `serve.json` holds, if it holds a server.
pub fn read(home: &Path) -> Option<Runtime> {
    serde_json::from_slice(&fs::read(dir(home).join(RUNTIME)).ok()?).ok()
}

/// Replaces `serve.json` by rename, readable by the user alone, as Jupyter
/// writes its own runtime file.
fn save(home: &Path, runtime: &Runtime) -> io::Result<()> {
    let dir = dir(home);
    let temp = dir.join(format!(".{RUNTIME}.{}.tmp", std::process::id()));
    let _ = fs::remove_file(&temp);
    let mut file = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&temp)?;
    file.write_all(&serde_json::to_vec_pretty(runtime)?)?;
    fs::rename(&temp, dir.join(RUNTIME))
}

// ---- the client: `ekko artifact` and the artifact tool ----------------

/// Where the page of artifact `uid` on the board at `location` is served,
/// starting a server when none of this version runs; why not, when it
/// cannot be.
pub fn page_address(home: &Path, location: &Location, uid: &str) -> Result<String, String> {
    let board = board_path(home, location)?;
    let runtime = ensure(home)?;
    Ok(format!("http://127.0.0.1:{}/{board}/{uid}.html", runtime.port))
}

/// How the server names the board at `location` in a path: `default`, or
/// `project/<name>`. A board the server would not find by that name, such as
/// a copy `EKKO_DIR` points at, has none.
fn board_path(home: &Path, location: &Location) -> Result<String, String> {
    let (path, dir) = match &location.project {
        Some(project) => (format!("project/{}", encode(&project.name)), crate::project::resolve_named(home, &project.name).ok().map(|found| found.dir)),
        None => ("default".to_string(), crate::directory::retrieve_ekko_directory(home, home, None, None).ok()),
    };
    match dir {
        Some(dir) if same_dir(&dir, &location.dir) => Ok(path),
        _ => Err("the server finds a board as the default one or by its project's name, and this board is neither".to_string()),
    }
}

/// Whether `a` and `b` are one directory, however each is reached: a project
/// registered at one path may be found from a mount of it at another.
fn same_dir(a: &Path, b: &Path) -> bool {
    match (fs::metadata(a), fs::metadata(b)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}

/// A server of this version, running: the one `serve.json` names, or one
/// started now. One of another version is asked to stop first.
pub fn ensure(home: &Path) -> Result<Runtime, String> {
    if let Some(runtime) = read(home) {
        match status(runtime.port) {
            Some(version) if version == VERSION => return Ok(runtime),
            Some(_) => {
                stop(&runtime);
            }
            None => {}
        }
    }
    record(home, &format!("asked to start a server, by pid {}", std::process::id()));
    start(home)?;
    let began = Instant::now();
    while began.elapsed() < STARTING {
        if let Some(runtime) = read(home).filter(|runtime| runtime.version == VERSION) {
            if status(runtime.port).as_deref() == Some(VERSION) {
                return Ok(runtime);
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Err(format!("ekko serve did not answer within {} s; its log is {}", STARTING.as_secs(), dir(home).join(LOG).display()))
}

/// `ekko serve --stop`: stops the server that runs, if one does: its port.
pub fn stop_running(home: &Path) -> Result<Option<u16>, String> {
    let Some(runtime) = read(home).filter(|runtime| status(runtime.port).is_some()) else { return Ok(None) };
    if stop(&runtime) {
        Ok(Some(runtime.port))
    } else {
        Err(format!("the server on 127.0.0.1:{} did not stop within {} s", runtime.port, STARTING.as_secs()))
    }
}

/// The version of the ekko serving on `port`, if one answers there.
fn status(port: u16) -> Option<String> {
    let (code, body) = ask(port, "GET", "/status", None).ok()?;
    let status: serde_json::Value = serde_json::from_slice(&body).ok().filter(|_| code == 200)?;
    status["ekko"].as_str().map(str::to_string)
}

/// Asks the server `runtime` names to stop, and waits until its port is
/// free: whether it is.
fn stop(runtime: &Runtime) -> bool {
    let _ = ask(runtime.port, "POST", "/stop", Some(&runtime.token));
    let began = Instant::now();
    while began.elapsed() < STARTING {
        if TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, runtime.port)), ASKING).is_err() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

/// Sends one request to the server on `port`, and reads the answer: its
/// status code and its body.
fn ask(port: u16, method: &str, path: &str, token: Option<&str>) -> io::Result<(u16, Vec<u8>)> {
    let mut stream = TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, port)), ASKING)?;
    stream.set_read_timeout(Some(ASKING))?;
    stream.set_write_timeout(Some(ASKING))?;
    let token = token.map(|token| format!("{TOKEN}: {token}\r\n")).unwrap_or_default();
    write!(stream, "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{token}Content-Length: 0\r\nConnection: close\r\n\r\n")?;
    let mut answer = Vec::new();
    stream.take(1 << 20).read_to_end(&mut answer)?;
    let mut headers = [httparse::EMPTY_HEADER; 32];
    let mut response = httparse::Response::new(&mut headers);
    match response.parse(&answer) {
        Ok(httparse::Status::Complete(head)) => Ok((response.code.unwrap_or_default(), answer[head..].to_vec())),
        _ => Err(io::Error::other("not an HTTP answer")),
    }
}

/// Starts `ekko serve` on its own: in a session of its own, with nothing of
/// this process's terminal or pipes, so it outlives the command or the
/// session that started it.
#[cfg(not(test))]
fn start(home: &Path) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    let exe = std::env::current_exe().map_err(|error| format!("ekko's own binary is not found: {error}"))?;
    let mut command = std::process::Command::new(exe);
    command.arg("serve").current_dir(home).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    // SAFETY: the closure runs between fork and exec, and calls only setsid,
    // which is async-signal-safe.
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = command.spawn().map_err(|error| format!("ekko serve could not start: {error}"))?;
    // Reaped when it exits, so a long-lived MCP server leaves no zombie.
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// A unit test's binary is the test harness, which `serve` would run.
#[cfg(test)]
fn start(_home: &Path) -> Result<(), String> {
    Err("a unit test starts no server".to_string())
}

// ---- the server --------------------------------------------------------

/// What every connection's thread shares.
struct Shared {
    home: PathBuf,
    runtime: Runtime,
    began: Instant,
    /// When the last request came, in milliseconds after `began`.
    last: AtomicU64,
    /// The connections being answered.
    open: AtomicUsize,
}

impl Shared {
    fn touch(&self) {
        self.last.store(u64::try_from(self.began.elapsed().as_millis()).unwrap_or(u64::MAX), Ordering::Relaxed);
    }

    fn idle_for(&self) -> Duration {
        self.began.elapsed().saturating_sub(Duration::from_millis(self.last.load(Ordering::Relaxed)))
    }
}

/// `ekko serve`: serves until it is idle for `idle`, is asked to stop, or
/// `serve.json` names another server. Refused while another one runs.
pub fn run(home: &Path, idle: Duration) -> Result<(), String> {
    let dir = dir(home);
    fs::create_dir_all(&dir).map_err(|error| format!("{} cannot be made: {error}", dir.display()))?;
    // Held until the process exits. A server replacing one that is still
    // on its way out waits for it here.
    let _held = crate::storage::lock_path(&dir.join(LOCK)).map_err(|_| match read(home) {
        Some(runtime) => format!("another ekko serve runs, on 127.0.0.1:{} as pid {}: ekko serve --stop stops it", runtime.port, runtime.pid),
        None => "another ekko serve runs".to_string(),
    })?;
    if fs::metadata(dir.join(LOG)).is_ok_and(|log| log.len() > LOG_MOST) {
        let _ = fs::write(dir.join(LOG), "");
    }
    let listener = bind(read(home).map(|runtime| runtime.port))?;
    let port = listener.local_addr().map_err(|error| format!("the port is not known: {error}"))?.port();
    let token = token().map_err(|error| format!("no token from /dev/urandom: {error}"))?;
    let runtime = Runtime { pid: std::process::id(), port, version: VERSION.to_string(), token };
    save(home, &runtime).map_err(|error| format!("{} cannot be written: {error}", dir.join(RUNTIME).display()))?;
    log(home, &format!("started: version {VERSION}, 127.0.0.1:{port}, pid {}, stops after {} s idle", runtime.pid, idle.as_secs()));
    let shared = Arc::new(Shared { home: home.to_path_buf(), runtime, began: Instant::now(), last: AtomicU64::new(0), open: AtomicUsize::new(0) });
    watch(Arc::clone(&shared), idle);
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };
        if shared.open.load(Ordering::SeqCst) >= CONNECTIONS_MOST {
            let _ = stream.write_all(&Answer::text(503, "too many connections at once").bytes());
            continue;
        }
        shared.open.fetch_add(1, Ordering::SeqCst);
        let shared = Arc::clone(&shared);
        std::thread::spawn(move || {
            handle(&shared, stream);
            shared.open.fetch_sub(1, Ordering::SeqCst);
        });
    }
    Ok(())
}

/// Listens on 127.0.0.1, at the `last` server's port when it is free, so a
/// tab open on that server finds this one.
fn bind(last: Option<u16>) -> Result<TcpListener, String> {
    last.and_then(|port| TcpListener::bind((Ipv4Addr::LOCALHOST, port)).ok())
        .map_or_else(|| TcpListener::bind((Ipv4Addr::LOCALHOST, 0)), Ok)
        .map_err(|error| format!("no port on 127.0.0.1: {error}"))
}

/// 32 random bytes, in hex.
fn token() -> io::Result<String> {
    let mut bytes = [0u8; 32];
    fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Stops the server once `serve.json` names another or is gone, or once it
/// had no request for `idle`.
fn watch(shared: Arc<Shared>, idle: Duration) {
    std::thread::spawn(move || loop {
        std::thread::sleep(WATCH.min(idle));
        if read(&shared.home).map(|runtime| runtime.pid) != Some(shared.runtime.pid) {
            leave(&shared.home, "serve.json names another server, or is gone");
        }
        if shared.open.load(Ordering::SeqCst) == 0 && shared.idle_for() >= idle {
            leave(&shared.home, &format!("no request for {} s", idle.as_secs()));
        }
    });
}

/// Stops the server, saying why in its log. `serve.json` stays, so the next
/// server takes the same port.
fn leave(home: &Path, why: &str) -> ! {
    log(home, &format!("stopped: {why}"));
    std::process::exit(0)
}

/// Appends `line` to the server's log, and to its standard error, which is a
/// terminal only when someone ran `ekko serve` there.
fn log(home: &Path, line: &str) {
    eprintln!("{}", record(home, line));
}

/// Appends `line` to the server's log, with the time: what a server did, or
/// what a client asked of it. The line as written.
fn record(home: &Path, line: &str) -> String {
    let line = format!("{} {line}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
    let _ = fs::create_dir_all(dir(home));
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(dir(home).join(LOG)) {
        let _ = writeln!(file, "{line}");
    }
    line
}

/// Answers the one request on `stream`, then closes it.
fn handle(shared: &Shared, mut stream: TcpStream) {
    shared.touch();
    let _ = stream.set_read_timeout(Some(TIMEOUT));
    let _ = stream.set_write_timeout(Some(TIMEOUT));
    let answer = match read_request(&mut stream) {
        Ok(request) => match refused(shared.runtime.port, &stream, &request) {
            Some(why) => {
                log(&shared.home, &format!("refused {} {}: {why}", request.method, request.path));
                Answer::text(403, &why)
            }
            None => route(shared, &request),
        },
        Err(Some(answer)) => answer,
        Err(None) => return,
    };
    let _ = stream.write_all(&answer.bytes());
    let _ = stream.flush();
    shared.touch();
    if answer.stops {
        leave(&shared.home, "asked to stop");
    }
}

/// A request's head, as the server reads it.
#[derive(Debug)]
struct Request {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, value)| value.as_str())
    }
}

/// Reads a request's head, up to `HEAD_MOST` bytes; its body is never read.
/// The answer to one that is not a request, or nothing when the connection
/// closed or stalled first.
fn read_request(stream: &mut impl Read) -> Result<Request, Option<Answer>> {
    let mut buffer = vec![0; HEAD_MOST];
    let mut filled = 0;
    loop {
        let read = stream.read(&mut buffer[filled..]).map_err(|_| None)?;
        if read == 0 {
            return Err(None);
        }
        filled += read;
        let mut headers = [httparse::EMPTY_HEADER; 64];
        let mut parsed = httparse::Request::new(&mut headers);
        match parsed.parse(&buffer[..filled]) {
            Ok(httparse::Status::Complete(_)) => {
                return Ok(Request {
                    method: parsed.method.unwrap_or_default().to_string(),
                    path: parsed.path.unwrap_or_default().to_string(),
                    headers: parsed.headers.iter().map(|header| (header.name.to_string(), String::from_utf8_lossy(header.value).into_owned())).collect(),
                })
            }
            Ok(httparse::Status::Partial) if filled < buffer.len() => {}
            Ok(httparse::Status::Partial) | Err(httparse::Error::TooManyHeaders) => {
                return Err(Some(Answer::text(431, "the request's head is too long")));
            }
            Err(error) => return Err(Some(Answer::text(400, &format!("not an HTTP request: {error}")))),
        }
    }
}

/// Why a request is refused, if it is: a Host other than this server's own,
/// or a socket another user owns.
fn refused(port: u16, stream: &TcpStream, request: &Request) -> Option<String> {
    if !local_host(request.header("host"), port) {
        return Some(format!("Host {:?} is not 127.0.0.1:{port} or localhost:{port}", request.header("host").unwrap_or_default()));
    }
    foreign(stream)
}

/// Whether `host` names this server: 127.0.0.1 or localhost, at its port. A
/// page that rebinds its own name to 127.0.0.1 still sends its name.
fn local_host(host: Option<&str>, port: u16) -> bool {
    host.is_some_and(|host| host == format!("127.0.0.1:{port}") || host.eq_ignore_ascii_case(&format!("localhost:{port}")))
}

/// Why the socket at the other end of `stream` is refused, if it is: owned by
/// another user, or not found. Only Linux has /proc/net/tcp to tell; elsewhere
/// another user's processes are served too.
fn foreign(stream: &TcpStream) -> Option<String> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let (Ok(SocketAddr::V4(local)), Ok(SocketAddr::V4(peer))) = (stream.local_addr(), stream.peer_addr()) else {
        return Some("not a connection to 127.0.0.1".to_string());
    };
    let table = fs::read_to_string("/proc/net/tcp").unwrap_or_default();
    // SAFETY: geteuid only reads this process's credentials.
    let mine = unsafe { libc::geteuid() };
    match owner(&table, peer, local) {
        Some(uid) if uid == mine => None,
        Some(uid) => Some(format!("the connecting socket is uid {uid}'s, not this user's")),
        None => Some(format!("the connecting socket, {peer}, is not in /proc/net/tcp")),
    }
}

/// The uid owning the socket at `from` connected to `to`, in a table laid out
/// as /proc/net/tcp is: each address in hex, the IPv4 one as the kernel holds
/// it, in network order, and the uid in the eighth field.
fn owner(table: &str, from: SocketAddrV4, to: SocketAddrV4) -> Option<u32> {
    table.lines().skip(1).find_map(|line| {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let ends = (address(fields.get(1)?)?, address(fields.get(2)?)?);
        (ends == (from, to)).then(|| fields.get(7)?.parse().ok())?
    })
}

/// An address as /proc/net/tcp writes it, `0100007F:A1B2` for 127.0.0.1:41394.
fn address(text: &str) -> Option<SocketAddrV4> {
    let (ip, port) = text.split_once(':')?;
    let ip = u32::from_str_radix(ip, 16).ok()?;
    Some(SocketAddrV4::new(Ipv4Addr::from(ip.to_ne_bytes()), u16::from_str_radix(port, 16).ok()?))
}

/// What the server answers a request it accepted.
fn route(shared: &Shared, request: &Request) -> Answer {
    let path = request.path.split('?').next().unwrap_or_default();
    match (request.method.as_str(), path) {
        ("GET", "/status") => Answer::new(200, "application/json", serde_json::json!({"ekko": VERSION, "pid": shared.runtime.pid}).to_string()),
        ("POST", "/stop") => match request.header(TOKEN) {
            Some(token) if same(token, &shared.runtime.token) => Answer { stops: true, ..Answer::text(200, "stopping") },
            _ => Answer::text(403, &format!("stopping takes the token serve.json holds, in {TOKEN}")),
        },
        ("GET", path) => page(&shared.home, path).unwrap_or_else(|| Answer::text(404, "no artifact's page is here")),
        _ => Answer::text(405, "the server answers GET, and POST /stop"),
    }
}

/// Whether two tokens are the same, in a time that does not tell how much
/// of one matched.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0, |differ, (x, y)| differ | (x ^ y)) == 0
}

/// The page, or the script holding its version, a path names:
/// `/default/<uid>.html` on the default board, `/project/<name>/<uid>.js` on
/// a project's, its name escaped as `encode` escapes it. Read from the board
/// as it is now, so it needs no write to be current.
fn page(home: &Path, path: &str) -> Option<Answer> {
    let segments: Vec<String> = path.strip_prefix('/')?.split('/').map(decode).collect::<Option<_>>()?;
    let (file, board) = segments.split_last()?;
    let (uid, kind) = file.rsplit_once('.')?;
    let (dir, folder) = match board {
        [default] if default == "default" => (crate::directory::retrieve_ekko_directory(home, home, None, None).ok()?, None),
        [project, name] if project == "project" => {
            let found = crate::project::resolve_named(home, name).ok()?;
            (found.dir, found.root)
        }
        _ => return None,
    };
    // Opening storage makes its folders, which a board that is not there
    // must not get from a read.
    if !dir.join("storage").is_dir() {
        return None;
    }
    let all = crate::storage::Storage::new(&dir).ok()?.get_shared().ok()?;
    let index = crate::ekko::uid_index(&all);
    let item = index.get(uid).and_then(|id| all.get(id)).filter(|item| item.artifact.is_some() && item.trashed.is_none())?;
    let (html, version) = crate::artifact::page(item, &all, folder.as_deref());
    match kind {
        "html" => Some(Answer::new(200, "text/html; charset=utf-8", html)),
        "js" => Some(Answer::new(200, "text/javascript; charset=utf-8", crate::artifact::script(&version))),
        _ => None,
    }
}

/// `text` as one segment of a path: every byte escaped but the unreserved
/// ones, so a project's name can hold anything a name can.
fn encode(text: &str) -> String {
    text.bytes()
        .map(|byte| if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) { char::from(byte).to_string() } else { format!("%{byte:02X}") })
        .collect()
}

/// A segment of a path with its escapes undone; none when an escape is not
/// two hex digits or the bytes are not UTF-8.
fn decode(segment: &str) -> Option<String> {
    let bytes = segment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            out.push(u8::from_str_radix(segment.get(at + 1..at + 3)?, 16).ok()?);
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// A response, sent whole, after which the connection closes.
#[derive(Debug)]
struct Answer {
    code: u16,
    kind: &'static str,
    body: Vec<u8>,
    /// Whether the server stops once it is sent.
    stops: bool,
}

impl Answer {
    fn new(code: u16, kind: &'static str, body: impl Into<Vec<u8>>) -> Answer {
        Answer { code, kind, body: body.into(), stops: false }
    }

    fn text(code: u16, text: &str) -> Answer {
        Answer::new(code, "text/plain; charset=utf-8", format!("{text}\n"))
    }

    fn bytes(&self) -> Vec<u8> {
        let reason = match self.code {
            200 => "OK",
            400 => "Bad Request",
            403 => "Forbidden",
            404 => "Not Found",
            405 => "Method Not Allowed",
            431 => "Request Header Fields Too Large",
            _ => "Service Unavailable",
        };
        let mut bytes = format!(
            "HTTP/1.1 {} {reason}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\n\r\n",
            self.code,
            self.kind,
            self.body.len()
        )
        .into_bytes();
        bytes.extend_from_slice(&self.body);
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A line of /proc/net/tcp for the socket at `local` connected to
    /// `remote`, owned by `uid`, its addresses written as this machine's
    /// kernel writes them.
    fn line(at: usize, local: SocketAddrV4, remote: SocketAddrV4, uid: u32) -> String {
        let hex = |address: SocketAddrV4| format!("{:08X}:{:04X}", u32::from_ne_bytes(address.ip().octets()), address.port());
        format!("{at:4}: {} {} 01 00000000:00000000 00:00000000 00000000 {uid:5}        0 {} 1 0000000000000000 20 4 30 10 -1", hex(local), hex(remote), 1000 + at)
    }

    #[test]
    fn the_owner_of_a_connection_is_read_off_proc_net_tcp() {
        let server = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 41394);
        let browser = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 50000);
        let other = SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, 7), 50000);
        let table = [
            "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode".to_string(),
            line(0, server, SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0), 1000),
            line(1, server, browser, 1000),
            line(2, browser, server, 1001),
            line(3, other, server, 1002),
        ]
        .join("\n");
        assert_eq!(address("0100007F:A1B2").filter(|_| cfg!(target_endian = "little")), cfg!(target_endian = "little").then_some(server));
        assert_eq!(owner(&table, browser, server), Some(1001), "the connecting socket is the browser's: {table}");
        assert_eq!(owner(&table, server, browser), Some(1000), "the accepted one is the server's own");
        assert_eq!(owner(&table, other, server), Some(1002), "the address counts, not the port alone");
        assert_eq!(owner(&table, SocketAddrV4::new(Ipv4Addr::LOCALHOST, 50001), server), None);
        assert_eq!(owner(&table, browser, SocketAddrV4::new(Ipv4Addr::LOCALHOST, 41395)), None);
    }

    /// A connection this user makes is this user's, read off the machine's
    /// own /proc/net/tcp.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_connection_from_this_user_is_served() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let _client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (accepted, _) = listener.accept().unwrap();
        assert_eq!(foreign(&accepted), None);
    }

    #[test]
    fn only_a_host_naming_this_server_is_local() {
        for host in ["127.0.0.1:4242", "localhost:4242", "LocalHost:4242"] {
            assert!(local_host(Some(host), 4242), "{host}");
        }
        for host in ["127.0.0.1:4243", "localhost:424", "evil.example:4242", "127.0.0.1", "[::1]:4242", "127.0.0.1:4242.evil.example", ""] {
            assert!(!local_host(Some(host), 4242), "{host}");
        }
        assert!(!local_host(None, 4242));
    }

    #[test]
    fn a_project_name_survives_the_path() {
        for name in ["ekko", "my project", "a%b", "ç/ã?#&", "~x.y-z_"] {
            let segment = encode(name);
            assert!(segment.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"-._~%".contains(&byte)), "{segment}");
            assert_eq!(decode(&segment).as_deref(), Some(name));
        }
        assert_eq!(encode("~x.y-z_"), "~x.y-z_");
        assert_eq!(decode("%zz"), None);
        assert_eq!(decode("ab%4"), None);
        assert_eq!(decode("%FF"), None, "not UTF-8");
    }

    #[test]
    fn a_request_head_is_read_and_anything_else_answered() {
        let head = b"GET /default/a.html?t=1 HTTP/1.1\r\nHost: 127.0.0.1:9\r\nX-Ekko-Token: t\r\n\r\nbody";
        let request = read_request(&mut &head[..]).unwrap();
        assert_eq!((request.method.as_str(), request.path.as_str()), ("GET", "/default/a.html?t=1"));
        assert_eq!((request.header("host"), request.header(TOKEN)), (Some("127.0.0.1:9"), Some("t")));
        let answer = |bytes: &[u8]| read_request(&mut &bytes[..]).map(|_| ()).unwrap_err().map(|answer| answer.code);
        assert_eq!(answer(b"hello there\r\n\r\n"), Some(400));
        assert_eq!(answer(format!("GET / HTTP/1.1\r\nX: {}\r\n\r\n", "y".repeat(HEAD_MOST)).as_bytes()), Some(431));
        assert_eq!(answer(b"GET / HTTP/1.1\r\nHost: unfinished"), None, "closed before its head ended");
    }

    #[test]
    fn the_server_tells_its_version_and_stops_only_with_its_token() {
        let shared = Shared {
            home: std::env::temp_dir().join("ekko-serve-nowhere"),
            runtime: Runtime { pid: 7, port: 9, version: VERSION.to_string(), token: "secret".to_string() },
            began: Instant::now(),
            last: AtomicU64::new(0),
            open: AtomicUsize::new(0),
        };
        let request = |method: &str, path: &str, token: Option<&str>| Request {
            method: method.to_string(),
            path: path.to_string(),
            headers: token.map(|token| vec![(TOKEN.to_ascii_lowercase(), token.to_string())]).unwrap_or_default(),
        };
        let status = route(&shared, &request("GET", "/status", None));
        assert_eq!(status.code, 200);
        assert_eq!(serde_json::from_slice::<serde_json::Value>(&status.body).unwrap(), serde_json::json!({"ekko": VERSION, "pid": 7}));
        let stopping = route(&shared, &request("POST", "/stop", Some("secret")));
        assert_eq!((stopping.code, stopping.stops), (200, true));
        for token in [None, Some("secrex"), Some("secret2"), Some("")] {
            let refused = route(&shared, &request("POST", "/stop", token));
            assert_eq!((refused.code, refused.stops), (403, false), "{token:?}");
        }
        assert_eq!(route(&shared, &request("GET", "/stop", Some("secret"))).code, 404);
        assert_eq!(route(&shared, &request("GET", "/default/nothing.html", None)).code, 404);
        assert_eq!(route(&shared, &request("GET", "/elsewhere/x/y.html", None)).code, 404);
        assert_eq!(route(&shared, &request("DELETE", "/status", None)).code, 405);
    }
}
