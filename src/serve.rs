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
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
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

/// The longest body a request may carry: a comment and what it is about.
const BODY_MOST: usize = 64 * 1024;

/// How often a stream of `/events` looks for a page's new version
/// (decision 1165).
const LOOK: Duration = Duration::from_millis(200);

/// How long a stream of `/events` stays silent before a comment, whose
/// failed write finds a browser gone.
const KEEP: Duration = Duration::from_secs(15);

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
    private(&dir(home).join(RUNTIME), &serde_json::to_vec_pretty(runtime)?)
}

/// Replaces `path` with `content` by rename, readable by the user alone.
fn private(path: &Path, content: &[u8]) -> io::Result<()> {
    let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let temp = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    let _ = fs::remove_file(&temp);
    let mut file = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&temp)?;
    file.write_all(content)?;
    fs::rename(&temp, path)
}

/// The file `ekko artifact` opens in place of the page's `address`: it sends
/// the browser on to the page with the token, which so stays off the
/// browser's command line, where other users read it, as Jupyter's redirect
/// file keeps its own (decision 1132).
pub fn redirect_file(home: &Path, address: &str) -> Result<PathBuf, String> {
    let runtime = read(home).ok_or("no server runs")?;
    let target = format!("{address}?token={}", runtime.token);
    let html = format!(
        "<!doctype html>\n<meta charset=\"utf-8\">\n<meta http-equiv=\"refresh\" content=\"0;url={target}\">\n<title>ekko</title>\n<a href=\"{target}\">{address}</a>\n"
    );
    let path = dir(home).join("serve-open.html");
    private(&path, html.as_bytes()).map_err(|error| format!("{} cannot be written: {error}", path.display()))?;
    Ok(path)
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
    /// The pages served, by the path of their html, each with its board's
    /// file, which a stream of `/events` watches.
    pages: Mutex<BTreeMap<String, PathBuf>>,
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
    // The last server's token, so a tab that opened its page still writes
    // after a replacement.
    let token = match read(home).map(|runtime| runtime.token).filter(|token| token.len() == 64) {
        Some(token) => token,
        None => token().map_err(|error| format!("no token from /dev/urandom: {error}"))?,
    };
    let runtime = Runtime { pid: std::process::id(), port, version: VERSION.to_string(), token };
    save(home, &runtime).map_err(|error| format!("{} cannot be written: {error}", dir.join(RUNTIME).display()))?;
    log(home, &format!("started: version {VERSION}, 127.0.0.1:{port}, pid {}, stops after {} s idle", runtime.pid, idle.as_secs()));
    let shared = Arc::new(Shared { home: home.to_path_buf(), runtime, began: Instant::now(), last: AtomicU64::new(0), open: AtomicUsize::new(0), pages: Mutex::default() });
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
    append(&dir(home), &line);
    line
}

/// Appends `line` to the log in `dir` as one write, which O_APPEND lands
/// whole: writeln! makes two, the text and the newline, and two requests
/// logging at once joined their lines (task 1241).
fn append(dir: &Path, line: &str) {
    let _ = fs::create_dir_all(dir);
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(dir.join(LOG)) {
        let _ = file.write_all(format!("{line}\n").as_bytes());
    }
}

/// Answers the one request on `stream`, then closes it.
fn handle(shared: &Shared, mut stream: TcpStream) {
    shared.touch();
    let _ = stream.set_read_timeout(Some(TIMEOUT));
    let _ = stream.set_write_timeout(Some(TIMEOUT));
    let answer = match read_request(&mut stream) {
        Ok(request) => {
            let answer = match caller(shared.runtime.port, &stream, &request) {
                Ok(_) if request.method == "GET" && request.path.split('?').next() == Some("/events") => {
                    events(shared, &mut stream);
                    shared.touch();
                    return;
                }
                Ok(socket) => route(shared, &request, socket),
                Err(why) => Answer::refuse(why, false),
            };
            if let Some(why) = &answer.refusal {
                log(&shared.home, &format!("refused {} {}: {why}", request.method, request.path));
            }
            answer
        }
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
    body: Vec<u8>,
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
            Ok(httparse::Status::Complete(head)) => {
                let mut request = Request {
                    method: parsed.method.unwrap_or_default().to_string(),
                    path: parsed.path.unwrap_or_default().to_string(),
                    headers: parsed.headers.iter().map(|header| (header.name.to_string(), String::from_utf8_lossy(header.value).into_owned())).collect(),
                    body: buffer[head..filled].to_vec(),
                };
                let length = request.header("content-length").and_then(|length| length.trim().parse::<usize>().ok()).unwrap_or(0);
                if length > BODY_MOST {
                    return Err(Some(Answer::text(413, "the request's body is too long")));
                }
                while request.body.len() < length {
                    let mut more = vec![0; length - request.body.len()];
                    let read = stream.read(&mut more).map_err(|_| None)?;
                    if read == 0 {
                        return Err(None);
                    }
                    request.body.extend_from_slice(&more[..read]);
                }
                request.body.truncate(length);
                return Ok(request);
            }
            Ok(httparse::Status::Partial) if filled < buffer.len() => {}
            Ok(httparse::Status::Partial) | Err(httparse::Error::TooManyHeaders) => {
                return Err(Some(Answer::text(431, "the request's head is too long")));
            }
            Err(error) => return Err(Some(Answer::text(400, &format!("not an HTTP request: {error}")))),
        }
    }
}

/// The socket at the other end of `stream`, which this user owns: its inode,
/// or none where there is no /proc/net/tcp to read it from, in which case
/// another user's processes are served too. Why not, for a Host other than
/// this server's own, or another user's socket.
fn caller(port: u16, stream: &TcpStream, request: &Request) -> Result<Option<u64>, String> {
    if !local_host(request.header("host"), port) {
        return Err(format!("Host {:?} is not 127.0.0.1:{port} or localhost:{port}", request.header("host").unwrap_or_default()));
    }
    if !cfg!(target_os = "linux") {
        return Ok(None);
    }
    let (Ok(SocketAddr::V4(local)), Ok(SocketAddr::V4(peer))) = (stream.local_addr(), stream.peer_addr()) else {
        return Err("not a connection to 127.0.0.1".to_string());
    };
    let table = fs::read_to_string("/proc/net/tcp").unwrap_or_default();
    // SAFETY: geteuid only reads this process's credentials.
    let mine = unsafe { libc::geteuid() };
    match socket(&table, peer, local) {
        Some((uid, inode)) if uid == mine => Ok(Some(inode)),
        Some((uid, _)) => Err(format!("the connecting socket is uid {uid}'s, not this user's")),
        None => Err(format!("the connecting socket, {peer}, is not in /proc/net/tcp")),
    }
}

/// Whether `host` names this server: 127.0.0.1 or localhost, at its port. A
/// page that rebinds its own name to 127.0.0.1 still sends its name.
fn local_host(host: Option<&str>, port: u16) -> bool {
    host.is_some_and(|host| host == format!("127.0.0.1:{port}") || host.eq_ignore_ascii_case(&format!("localhost:{port}")))
}

/// The socket at `from` connected to `to`, in a table laid out as
/// /proc/net/tcp is: the uid owning it, in the eighth field, and its inode,
/// in the tenth. Each address is in hex, the IPv4 one as the kernel holds it,
/// in network order.
fn socket(table: &str, from: SocketAddrV4, to: SocketAddrV4) -> Option<(u32, u64)> {
    table.lines().skip(1).find_map(|line| {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let ends = (address(fields.get(1)?)?, address(fields.get(2)?)?);
        (ends == (from, to)).then(|| Some((fields.get(7)?.parse().ok()?, fields.get(9)?.parse().ok()?)))?
    })
}

/// The processes holding socket `inode`: each one with a descriptor linked to
/// socket:[inode], which a process that handed it down to a child shares.
fn holders(inode: u64) -> Vec<u32> {
    let link = format!("socket:[{inode}]");
    let Ok(entries) = fs::read_dir("/proc") else { return Vec::new() };
    entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
        .filter(|pid| {
            let descriptors = fs::read_dir(format!("/proc/{pid}/fd")).into_iter().flatten().flatten();
            descriptors.into_iter().any(|fd| fs::read_link(fd.path()).is_ok_and(|target| target.as_os_str() == link.as_str()))
        })
        .collect()
}

/// The person behind a write, as the processes holding its socket, each
/// with its name; why not, when a Claude Code process is among the ancestors
/// of one of them, judged as `ekko --answer` judges a command (decision 1132).
fn session(socket: Option<u64>) -> Result<String, String> {
    let Some(inode) = socket else {
        return Err("only Linux tells the user from a session, by /proc; elsewhere a write is ekko's menu's".to_string());
    };
    let holders = holders(inode);
    if holders.is_empty() {
        return Err("no process holds the connecting socket".to_string());
    }
    match holders.iter().find_map(|pid| crate::holder::claude_among(*pid).map(|claude| (*pid, claude))) {
        Some((pid, claude)) => Err(format!("process {pid} runs under Claude Code, process {claude}: a session writes with ekko's MCP tools")),
        None => Ok(holders
            .iter()
            .map(|pid| format!("process {pid} ({})", fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default().trim()))
            .collect::<Vec<_>>()
            .join(", ")),
    }
}

/// The person behind a write, as `session` names them; why the write is
/// refused, when it comes from another page than the server's own, from a
/// browser that has not opened the page with the token, or from a Claude
/// Code session (decision 1132).
fn writer(shared: &Shared, request: &Request, socket: Option<u64>) -> Result<String, String> {
    let port = shared.runtime.port;
    let origin = request.header("origin").unwrap_or_default();
    if origin != format!("http://127.0.0.1:{port}") && !origin.eq_ignore_ascii_case(&format!("http://localhost:{port}")) {
        return Err(format!("a write comes from the page itself, and this one's Origin is {origin:?}"));
    }
    if !cookie(request, &cookie_name(port)).is_some_and(|token| same(token, &shared.runtime.token)) {
        return Err("this browser has not opened the page through ekko artifact, which hands it the token".to_string());
    }
    session(socket)
}

/// The cookie a browser that opened a page with the token holds: one per
/// port, since cookies are not kept apart by port.
fn cookie_name(port: u16) -> String {
    format!("ekko_{port}")
}

/// The value of the cookie `name` a request carries.
fn cookie<'a>(request: &'a Request, name: &str) -> Option<&'a str> {
    request.header("cookie")?.split(';').filter_map(|pair| pair.trim().split_once('=')).find(|(key, _)| *key == name).map(|(_, value)| value)
}

/// An address as /proc/net/tcp writes it, `0100007F:A1B2` for 127.0.0.1:41394.
fn address(text: &str) -> Option<SocketAddrV4> {
    let (ip, port) = text.split_once(':')?;
    let ip = u32::from_str_radix(ip, 16).ok()?;
    Some(SocketAddrV4::new(Ipv4Addr::from(ip.to_ne_bytes()), u16::from_str_radix(port, 16).ok()?))
}

/// What the server answers a request from this user's `socket`.
fn route(shared: &Shared, request: &Request, socket: Option<u64>) -> Answer {
    let (path, query) = request.path.split_once('?').unwrap_or((request.path.as_str(), ""));
    match (request.method.as_str(), path) {
        ("GET", "/status") => Answer::new(200, "application/json", serde_json::json!({"ekko": VERSION, "pid": shared.runtime.pid}).to_string()),
        ("POST", "/stop") => match request.header(TOKEN) {
            Some(token) if same(token, &shared.runtime.token) => Answer { stops: true, ..Answer::text(200, "stopping") },
            _ => Answer::refuse(format!("stopping takes the token serve.json holds, in {TOKEN}"), false),
        },
        // Writes nothing: says whom the server takes the caller for, which
        // the page shows, and a test asks.
        ("POST", "/api/who") => match writer(shared, request, socket) {
            Ok(person) => {
                log(&shared.home, &format!("POST /api/who: the user, {person}"));
                Answer::new(200, "application/json", serde_json::json!({"person": true}).to_string())
            }
            Err(why) => Answer::refuse(why, true),
        },
        ("POST", "/api/comment") => comment(shared, request, socket, "new"),
        ("POST", "/api/comment/edit") => comment(shared, request, socket, "edit"),
        ("POST", "/api/comment/delete") => comment(shared, request, socket, "delete"),
        ("GET", path) => match query.split('&').find_map(|pair| pair.strip_prefix("token=")) {
            Some(token) if path.starts_with("/default/") || path.starts_with("/project/") => opened(shared, path, token),
            _ => font(path).or_else(|| page(shared, path)).unwrap_or_else(|| Answer::text(404, "no artifact's page is here")),
        },
        _ => Answer::text(405, "the server answers GET, and POST to /stop, /api/who and /api/comment"),
    }
}

/// `POST /api/comment`, `/api/comment/edit` and `/api/comment/delete`: the
/// person's comments on an artifact's page (tasks 1105 and 1213), as JSON
/// naming the page's path. A new comment carries its text and the comment
/// itself, with the plan's version it was made on and what it is about; an
/// edit names a comment by uid with the text, theme or color it takes; a
/// deletion names one alone, and puts it in the trash. Each is written as
/// the person on the artifact's board, and refused when the plan moved on,
/// no longer holds the words, or the comment is not the person's.
fn comment(shared: &Shared, request: &Request, socket: Option<u64>, how: &str) -> Answer {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Posted {
        page: String,
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        comment: Option<crate::item::Comment>,
        #[serde(default)]
        uid: Option<String>,
        #[serde(default)]
        theme: Option<String>,
        #[serde(default)]
        color: Option<String>,
    }
    let json = |code: u16, value: serde_json::Value| Answer::new(code, "application/json", value.to_string());
    let person = match writer(shared, request, socket) {
        Ok(person) => person,
        Err(why) => return Answer::refuse(why, true),
    };
    let posted: Posted = match serde_json::from_slice(&request.body) {
        Ok(posted) => posted,
        Err(error) => return json(400, serde_json::json!({"why": format!("not a comment: {error}")})),
    };
    let Some((project, uid)) = board_of(&posted.page) else {
        return json(404, serde_json::json!({"why": format!("{} is no artifact's page", posted.page)}));
    };
    let fields = match how {
        "new" => posted.comment.is_some() && posted.uid.is_none() && posted.theme.is_none() && posted.color.is_none(),
        "edit" => posted.uid.is_some() && posted.comment.is_none(),
        _ => posted.uid.is_some() && posted.comment.is_none() && posted.text.is_none() && posted.theme.is_none() && posted.color.is_none(),
    };
    if !fields {
        let wants = match how {
            "new" => "a new comment takes page, text and comment",
            "edit" => "an edit takes page, uid, and any of text, theme and color",
            _ => "a deletion takes page and uid",
        };
        return json(400, serde_json::json!({ "why": wants }));
    }
    let written = crate::directory::locate(&shared.home, &shared.home, None, None, project.as_deref())
        .map_err(|error| error.to_string())
        .and_then(|location| crate::ekko::Ekko::at(&location).map_err(|error| error.to_string()))
        .and_then(|ekko| {
            let ekko = ekko.acting_as(crate::holder::Actor::person());
            let mut draft = crate::ops::Draft::open(&ekko).map_err(|error| error.to_string())?;
            let on = crate::ops::Ref::Text(uid.clone());
            let id = match (how, posted.comment, posted.uid.as_deref()) {
                ("new", Some(comment), _) => draft.comment(&on, posted.text.as_deref().unwrap_or_default(), comment),
                ("edit", _, Some(note)) => draft.edit_comment(&on, note, posted.text.as_deref(), posted.theme.as_deref(), posted.color.as_deref()),
                (_, _, Some(note)) => draft.trash_comment(&on, note),
                _ => unreachable!("the fields were checked"),
            }
            .map_err(|error| error.to_string())?;
            let committed = draft.commit(false).map_err(|error| error.to_string())?;
            Ok((id, committed.data.get(&id).and_then(|note| note.uid.clone()).unwrap_or_default()))
        });
    match written {
        Ok((id, note)) => {
            let did = match how {
                "new" => "note",
                "edit" => "edited note",
                _ => "trashed note",
            };
            log(&shared.home, &format!("POST /api/comment: {did} {id} on {uid}, by the user, {person}"));
            json(200, serde_json::json!({"id": id, "uid": note}))
        }
        Err(why) => json(409, serde_json::json!({"why": why})),
    }
}

/// The board and the artifact a page's path names: the project's name, or
/// none for the default board, and the artifact's uid.
fn board_of(path: &str) -> Option<(Option<String>, String)> {
    let segments: Vec<String> = path.strip_prefix('/')?.split('/').map(decode).collect::<Option<_>>()?;
    let (file, board) = segments.split_last()?;
    let uid = file.strip_suffix(".html")?.to_string();
    match board {
        [default] if default == "default" => Some((None, uid)),
        [project, name] if project == "project" => Some((Some(name.clone()), uid)),
        _ => None,
    }
}

/// A page opened through the redirect file: the token traded for the cookie
/// a write needs, and the browser sent on to the address without it, which
/// keeps the token out of its history. A token not this server's gets no
/// cookie, and the page then reads only.
fn opened(shared: &Shared, path: &str, token: &str) -> Answer {
    let answer = Answer::text(303, "the page").with("Location", path.to_string());
    if !same(token, &shared.runtime.token) {
        return answer;
    }
    answer.with("Set-Cookie", format!("{}={token}; HttpOnly; SameSite=Strict; Path=/", cookie_name(shared.runtime.port)))
}

/// A page as the server sends it, under a Content-Security-Policy that runs
/// only its own scripts, each given this answer's nonce, and fetches from
/// nowhere but the server (decision 1132), its fonts included (task 1152).
/// A plan's text cannot carry a script tag of its own: its raw HTML is
/// escaped.
fn secured(html: &str) -> Answer {
    let nonce = token().unwrap_or_default();
    let policy = format!(
        "default-src 'none'; script-src 'self' 'nonce-{nonce}'; style-src 'unsafe-inline'; font-src 'self'; img-src 'self' data:; connect-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'"
    );
    Answer::new(200, "text/html; charset=utf-8", html.replace("<script>", &format!("<script nonce=\"{nonce}\">"))).with("Content-Security-Policy", policy)
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
fn page(shared: &Shared, path: &str) -> Option<Answer> {
    let (built, kind) = build(&shared.home, path)?;
    let answer = match kind.as_str() {
        "html" => secured(&built.html),
        "js" => Answer::new(200, "text/javascript; charset=utf-8", crate::artifact::script(&built.version)),
        _ => return None,
    };
    if let Ok(mut pages) = shared.pages.lock() {
        pages.insert(built.path, built.board);
    }
    Some(answer)
}

/// A page as the board holds it now.
struct Built {
    html: String,
    version: String,
    /// The path of its html, the one a browser shows.
    path: String,
    /// The board's file, which every write replaces.
    board: PathBuf,
}

/// The page a path names, built from its board, and the kind the path asks
/// for: html, or js for the script holding its version.
fn build(home: &Path, path: &str) -> Option<(Built, String)> {
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
    let page = format!("{}.html", path.rsplit_once('.')?.0);
    Some((Built { html, version, path: page, board: dir.join("storage").join("storage.json") }, kind.to_string()))
}

/// `GET /events`: the version of every page this server has served, as
/// Server-Sent Events on one stream a browser shares among its tabs
/// (decision 1165). It opens with each page's version, so a tab that missed
/// a write while it connected still learns of it; then it sends a page's
/// version whenever a write changes it, looking every `LOOK`, and a comment
/// after `KEEP` of silence. It ends once a write to the browser fails.
fn events(shared: &Shared, stream: &mut TcpStream) {
    let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\n\r\nretry: 1000\n\n";
    if stream.write_all(head.as_bytes()).is_err() {
        return;
    }
    // What was sent of each page: its board's file then, and its version,
    // empty while the board holds no such page.
    let mut sent: BTreeMap<String, (Option<(u64, u64)>, String)> = BTreeMap::new();
    let mut quiet = Instant::now();
    loop {
        let pages: Vec<(String, PathBuf)> = shared.pages.lock().map(|pages| pages.clone().into_iter().collect()).unwrap_or_default();
        let mut out = String::new();
        for (path, board) in pages {
            // A write replaces the file by rename, so a new inode is a write.
            let stamp = fs::metadata(&board).ok().map(|meta| (meta.dev(), meta.ino()));
            if sent.get(&path).is_some_and(|(seen, _)| *seen == stamp) {
                continue;
            }
            let version = build(&shared.home, &path).map(|(built, _)| built.version).unwrap_or_default();
            if !version.is_empty() && sent.get(&path).is_none_or(|(_, last)| *last != version) {
                out.push_str(&format!("event: version\ndata: {path} {version}\n\n"));
            }
            sent.insert(path, (stamp, version));
        }
        if out.is_empty() && quiet.elapsed() >= KEEP {
            out.push_str(": keep\n\n");
        }
        if !out.is_empty() {
            if stream.write_all(out.as_bytes()).and_then(|()| stream.flush()).is_err() {
                return;
            }
            quiet = Instant::now();
        }
        shared.touch();
        std::thread::sleep(LOOK);
    }
}

/// A font of the pages, or their licence, which a page's style names beside
/// it: `fonts/<file>` under the path of a board, any board, since a font is
/// no board's. A font's name holds its hash, so a browser may keep it for
/// good, and a page reloaded on a new version draws at once.
fn font(path: &str) -> Option<Answer> {
    let (board, file) = path.rsplit_once(&format!("/{}/", crate::artifact::FONTS_DIR))?;
    let named = board.strip_prefix("/project/").is_some_and(|name| !name.is_empty() && !name.contains('/'));
    if board != "/default" && !named {
        return None;
    }
    let (license, text) = crate::artifact::FONT_LICENSE;
    if file == license {
        return Some(Answer::new(200, "text/plain; charset=utf-8", text));
    }
    let (_, bytes) = crate::artifact::font_files().iter().find(|(name, _)| name == file)?;
    Some(Answer { cache: "max-age=31536000, immutable", ..Answer::new(200, "font/woff2", *bytes) })
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
    /// The headers past those every answer has.
    headers: Vec<(&'static str, String)>,
    /// Why it refuses, which the log keeps.
    refusal: Option<String>,
    /// Whether the server stops once it is sent.
    stops: bool,
    /// How long a browser may keep it: nothing but a font, by default.
    cache: &'static str,
}

impl Answer {
    fn new(code: u16, kind: &'static str, body: impl Into<Vec<u8>>) -> Answer {
        Answer { code, kind, body: body.into(), headers: Vec::new(), refusal: None, stops: false, cache: "no-store" }
    }

    fn text(code: u16, text: &str) -> Answer {
        Answer::new(code, "text/plain; charset=utf-8", format!("{text}\n"))
    }

    /// A 403 saying `why`: in JSON to the page's own API, which shows it.
    fn refuse(why: String, json: bool) -> Answer {
        let mut answer = match json {
            true => Answer::new(403, "application/json", serde_json::json!({"person": false, "why": why}).to_string()),
            false => Answer::text(403, &why),
        };
        answer.refusal = Some(why);
        answer
    }

    fn with(mut self, name: &'static str, value: String) -> Answer {
        self.headers.push((name, value));
        self
    }

    fn bytes(&self) -> Vec<u8> {
        let reason = match self.code {
            200 => "OK",
            303 => "See Other",
            400 => "Bad Request",
            403 => "Forbidden",
            404 => "Not Found",
            405 => "Method Not Allowed",
            409 => "Conflict",
            413 => "Content Too Large",
            431 => "Request Header Fields Too Large",
            _ => "Service Unavailable",
        };
        let mut head = format!(
            "HTTP/1.1 {} {reason}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: {}\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\n",
            self.code,
            self.kind,
            self.body.len(),
            self.cache
        );
        for (name, value) in &self.headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        let mut bytes = format!("{head}\r\n").into_bytes();
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

    /// A server on port 9 of no home, whose token is `secret`.
    fn shared() -> Shared {
        Shared {
            home: std::env::temp_dir().join("ekko-serve-nowhere"),
            runtime: Runtime { pid: 7, port: 9, version: VERSION.to_string(), token: "secret".to_string() },
            began: Instant::now(),
            last: AtomicU64::new(0),
            open: AtomicUsize::new(0),
            pages: Mutex::default(),
        }
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
        assert_eq!(socket(&table, browser, server), Some((1001, 1002)), "the connecting socket is the browser's, and its inode: {table}");
        assert_eq!(socket(&table, server, browser), Some((1000, 1001)), "the accepted one is the server's own");
        assert_eq!(socket(&table, other, server), Some((1002, 1003)), "the address counts, not the port alone");
        assert_eq!(socket(&table, SocketAddrV4::new(Ipv4Addr::LOCALHOST, 50001), server), None);
        assert_eq!(socket(&table, browser, SocketAddrV4::new(Ipv4Addr::LOCALHOST, 41395)), None);
    }

    /// A connection this user makes is this user's, read off the machine's
    /// own /proc/net/tcp.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_connection_from_this_user_is_served_and_held_by_its_process() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (accepted, _) = listener.accept().unwrap();
        let request = |host: &str| Request { method: "GET".to_string(), path: "/".to_string(), headers: vec![("Host".to_string(), host.to_string())], body: Vec::new() };
        let inode = caller(port, &accepted, &request(&format!("127.0.0.1:{port}"))).unwrap().expect("Linux reads the socket");
        assert_eq!(holders(inode), vec![std::process::id()], "the connecting socket is this process's");
        assert!(caller(port, &accepted, &request("evil.example")).is_err());
        // This process is a session when Claude Code runs the tests, and the
        // person on CI: the judgment follows its ancestors either way.
        assert_eq!(session(Some(inode)).is_err(), crate::holder::claude_among(std::process::id()).is_some());
        // A socket its process closed once the request was sent, as bash's
        // `printf ... > /dev/tcp/...` does, is nobody's, and nobody writes.
        drop(client);
        assert_eq!(holders(inode), Vec::<u32>::new());
        assert!(session(Some(inode)).unwrap_err().contains("no process holds"));
    }

    #[test]
    fn a_write_needs_the_page_own_origin_and_the_cookie_the_token_bought() {
        let shared = shared();
        let write = |origin: Option<&str>, cookie: Option<&str>| {
            let mut headers = vec![("Host".to_string(), "127.0.0.1:9".to_string())];
            headers.extend(origin.map(|origin| ("Origin".to_string(), origin.to_string())));
            headers.extend(cookie.map(|cookie| ("Cookie".to_string(), cookie.to_string())));
            writer(&shared, &Request { method: "POST".to_string(), path: "/api/who".to_string(), headers, body: Vec::new() }, None).unwrap_err()
        };
        let mine = "a=1; ekko_9=secret; b=2";
        let foreign = ["null", "http://127.0.0.1:8", "http://127.0.0.1:90", "http://localhost:8", "http://localhost:90", "http://evil.example", "https://127.0.0.1:9"];
        for origin in std::iter::once(None).chain(foreign.map(Some)) {
            assert!(write(origin, Some(mine)).contains("Origin"), "{origin:?}");
        }
        for cookie in [None, Some("ekko_9=secrex"), Some("ekko_8=secret"), Some("xekko_9=secret")] {
            assert!(write(Some("http://127.0.0.1:9"), cookie).contains("ekko artifact"), "{cookie:?}");
        }
        assert!(write(Some("http://localhost:9"), Some(mine)).contains("only Linux"), "past the Origin and the cookie, the person check");

        let opened = opened(&shared, "/default/u.html", "secret");
        assert_eq!(opened.code, 303);
        let headers: Vec<(&str, &str)> = opened.headers.iter().map(|(name, value)| (*name, value.as_str())).collect();
        assert_eq!(headers, [("Location", "/default/u.html"), ("Set-Cookie", "ekko_9=secret; HttpOnly; SameSite=Strict; Path=/")]);
        let wrong = opened_headers(&super::opened(&shared, "/default/u.html", "secrex"));
        assert_eq!(wrong, ["Location"], "a token not the server's buys no cookie");
    }

    fn opened_headers(answer: &Answer) -> Vec<&'static str> {
        answer.headers.iter().map(|(name, _)| *name).collect()
    }

    #[test]
    fn a_served_page_runs_only_its_own_scripts() {
        let answer = secured("<html><script>a()</script><p>&lt;script&gt;</p><script>b()</script></html>");
        let policy = &answer.headers.iter().find(|(name, _)| *name == "Content-Security-Policy").unwrap().1;
        let nonce = policy.split("'nonce-").nth(1).and_then(|rest| rest.split('\'').next()).unwrap();
        assert_eq!(nonce.len(), 64, "{policy}");
        let html = String::from_utf8(answer.body).unwrap();
        assert_eq!(html.matches(&format!("<script nonce=\"{nonce}\">")).count(), 2, "{html}");
        assert!(!html.contains("<script>") && html.contains("&lt;script&gt;"), "{html}");
        for part in ["default-src 'none'", "script-src 'self' 'nonce-", "font-src 'self';", "connect-src 'self'", "base-uri 'none'", "frame-ancestors 'none'"] {
            assert!(policy.contains(part), "{part}: {policy}");
        }
        assert_ne!(secured("<script>").headers, answer.headers, "a nonce per answer");
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
        let shared = shared();
        let request = |method: &str, path: &str, token: Option<&str>| Request {
            method: method.to_string(),
            path: path.to_string(),
            headers: token.map(|token| vec![(TOKEN.to_ascii_lowercase(), token.to_string())]).unwrap_or_default(),
            body: Vec::new(),
        };
        let status = route(&shared, &request("GET", "/status", None), None);
        assert_eq!(status.code, 200);
        assert_eq!(serde_json::from_slice::<serde_json::Value>(&status.body).unwrap(), serde_json::json!({"ekko": VERSION, "pid": 7}));
        let stopping = route(&shared, &request("POST", "/stop", Some("secret")), None);
        assert_eq!((stopping.code, stopping.stops), (200, true));
        for token in [None, Some("secrex"), Some("secret2"), Some("")] {
            let refused = route(&shared, &request("POST", "/stop", token), None);
            assert_eq!((refused.code, refused.stops), (403, false), "{token:?}");
        }
        assert_eq!(route(&shared, &request("GET", "/stop", Some("secret")), None).code, 404);
        assert_eq!(route(&shared, &request("GET", "/default/nothing.html", None), None).code, 404);
        assert_eq!(route(&shared, &request("GET", "/elsewhere/x/y.html", None), None).code, 404);
        assert_eq!(route(&shared, &request("DELETE", "/status", None), None).code, 405);
    }

    /// The fonts a page names beside it, under any board's path, kept by
    /// the browser for good; their licence beside them; nothing else there.
    #[test]
    fn the_fonts_of_the_pages_are_served_beside_them_and_kept() {
        let shared = shared();
        let get = |path: &str| route(&shared, &Request { method: "GET".to_string(), path: path.to_string(), headers: Vec::new(), body: Vec::new() }, None);
        let (name, bytes) = &crate::artifact::font_files()[1];
        for board in ["/default", "/project/site", "/project/a%20b"] {
            let font = get(&format!("{board}/fonts/{name}"));
            assert_eq!((font.code, font.kind, font.cache), (200, "font/woff2", "max-age=31536000, immutable"), "{board}");
            assert_eq!(&font.body, bytes);
            assert!(String::from_utf8_lossy(&font.bytes()).contains("\r\nCache-Control: max-age=31536000, immutable\r\n"));
        }
        let license = get("/default/fonts/OFL.txt");
        assert_eq!((license.code, license.kind), (200, "text/plain; charset=utf-8"));
        assert!(String::from_utf8_lossy(&license.body).contains("SIL OPEN FONT LICENSE"));
        for path in [format!("/default/fonts/{}", name.replace(".woff2", "0.woff2")), format!("/elsewhere/fonts/{name}"), format!("/project/fonts/{name}"), format!("/project/a/b/fonts/{name}"), format!("/fonts/{name}")] {
            assert_eq!(get(&path).code, 404, "{path}");
        }
        let page = get("/default/nothing.html");
        assert_eq!((page.code, page.cache), (404, "no-store"), "anything but a font is kept by none");
        assert!(String::from_utf8_lossy(&page.bytes()).contains("\r\nCache-Control: no-store\r\n"));
    }

    /// Lines logged at once, as two tabs reloading together log theirs,
    /// come out each whole and on its own line (task 1241).
    #[test]
    fn lines_logged_at_once_stay_whole() {
        let dir = std::env::temp_dir().join(format!("ekko-serve-log-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        std::thread::scope(|scope| {
            for thread in 0..8 {
                let dir = &dir;
                scope.spawn(move || (0..250).for_each(|n| append(dir, &format!("thread {thread}, line {n}"))));
            }
        });
        let log = fs::read_to_string(dir.join(LOG)).unwrap();
        let mut lines: Vec<&str> = log.lines().collect();
        let broken: Vec<&&str> = lines.iter().filter(|line| line.matches("thread").count() != 1).take(3).collect();
        assert!(broken.is_empty(), "lines joined or empty: {broken:?}");
        lines.sort_unstable();
        lines.dedup();
        assert_eq!(lines.len(), 2000);
        fs::remove_dir_all(&dir).ok();
    }
}
