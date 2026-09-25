//! ekko's own menu: questions asked on the board, put to the user in a
//! terminal of their own.
//!
//! Claude Code draws an MCP server's elicitation as a form, where choosing an
//! option means opening a dropdown, which the user found confusing next to
//! Claude Code's own menu (2026-09-24, note 480). An MCP server cannot draw
//! inside Claude Code's screen, so ask opens a terminal of its own: a popup
//! over the pane inside tmux, or else a window. It runs `ekko --menu <file>`,
//! which lists each question's options to pick with the arrows or a number,
//! several with the space bar where the question allows, a preview beside
//! the focused option, "Other answer…" to write one, and Tab for a note. The
//! answers go on the board, where the server that opened the menu reads them
//! and answers ask's call. Esc leaves every question open. A key counts only
//! once the menu has been up, or back in focus, for a second and the keyboard
//! has been quiet, so typing meant for another window never answers.
//!
//! The file holds the questions, previews included, which the board does not
//! keep. The menu writes its pid beside it: a pid gone with the answers not
//! on the board means the user closed the menu, and no pid at all, that the
//! menu never opened. `ekko --answer <id>` alone opens the same menu on one
//! question, read back from the board, in whatever terminal it runs.

use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::dialog;
use crate::ekko::{Ekko, EkkoError, Outcome};
use crate::ops::{Choice, Draft, Ref};

/// The terminal to open the menu's window with: a command the menu's
/// command line is appended to, such as `konsole -e`. `none` opens nothing.
pub const TERMINAL: &str = "EKKO_TERMINAL";

/// How long the menu has to say it is up before it counts as never opened.
const OPENING: Duration = Duration::from_secs(15);

/// How long an open menu waits before a desktop notification says a
/// question is waiting: long enough that a user at the screen has answered.
const REMIND: Duration = Duration::from_secs(20);

/// How long the menu is up, or back in focus, before a key counts: keys typed
/// for another window as this one took the focus are dropped. A space typed
/// as the window opened once chose the recommended option (task 593).
const SETTLE: Duration = Duration::from_secs(1);

/// How long the keyboard must then be quiet after a key dropped: typing that
/// runs on, still meant for another window, is dropped as a whole.
const QUIET: Duration = Duration::from_millis(400);

/// Terminals looked for on PATH when `EKKO_TERMINAL` is unset, each with
/// what runs a command in it. Konsole in a process of its own, so that the
/// environment and folder are this server's, and without its bars.
const KNOWN: &[(&str, &[&str])] = &[
    ("konsole", &["--separate", "--hide-menubar", "--hide-tabbar", "--hide-toolbars", "-e"]),
    ("gnome-terminal", &["--"]),
    ("kitty", &[]),
    ("alacritty", &["-e"]),
    ("foot", &[]),
    ("wezterm", &["start", "--"]),
    ("ghostty", &["-e"]),
    ("xfce4-terminal", &["-x"]),
    ("xterm", &["-e"]),
];

/// The questions one menu puts to the user, as ask recorded them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Spec {
    pub questions: Vec<Posed>,
}

/// One question: where the board keeps it, and what the menu shows of it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Posed {
    pub uid: String,
    pub id: u32,
    pub text: String,
    #[serde(default)]
    pub options: Vec<Choice>,
    #[serde(default)]
    pub multiple: bool,
}

/// Where the menu opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Place {
    /// A popup over the pane, inside tmux: no display needed, over SSH too.
    Tmux,
    /// A window, opened with this command.
    Window(Vec<String>),
}

/// Where a menu can open, if anywhere: `EKKO_TERMINAL`, or else a tmux popup
/// inside tmux, or else a window of a known terminal on PATH, `$TERMINAL`
/// first, when there is a display.
pub fn place() -> Option<Place> {
    if let Ok(chosen) = std::env::var(TERMINAL) {
        let words: Vec<String> = chosen.split_whitespace().map(str::to_string).collect();
        return (!words.is_empty() && words != ["none"]).then_some(Place::Window(words));
    }
    let path = std::env::var_os("PATH")?;
    let found = |name: &str| std::env::split_paths(&path).any(|dir| dir.join(name).is_file());
    if std::env::var_os("TMUX").is_some() && found("tmux") {
        return Some(Place::Tmux);
    }
    if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return None;
    }
    let preferred = std::env::var("TERMINAL").ok();
    KNOWN
        .iter()
        .filter(|(name, _)| preferred.as_deref() == Some(*name))
        .chain(KNOWN.iter())
        .find(|(name, _)| found(name))
        .map(|(name, run)| Place::Window(std::iter::once(*name).chain(run.iter().copied()).map(str::to_string).collect()))
}

/// A menu put up for one call of ask.
pub struct Window {
    file: PathBuf,
    opened: Instant,
    child: Child,
    reminded: bool,
}

impl Window {
    /// Opens a menu on `spec` at `place`, for the board the server finds from
    /// `cwd` and `project`.
    pub fn open(place: &Place, spec: &Spec, project: Option<&str>, cwd: &Path) -> io::Result<Window> {
        static OPENED: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let file = dir.join(format!("ekko-menu-{}-{}.json", std::process::id(), OPENED.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_file(pid_file(&file));
        std::fs::write(&file, serde_json::to_vec(spec)?)?;
        let line = command_line(&std::env::current_exe()?, &file, project, cwd);
        let mut command = match place {
            Place::Window(terminal) => {
                let mut command = Command::new(&terminal[0]);
                command.args(sized(terminal, spec)).args(&line);
                command
            }
            Place::Tmux => {
                let mut command = Command::new("tmux");
                command.args(["display-popup", "-E", "-w", "90%", "-h", "80%", "-T", " ekko "]);
                if let Ok(pane) = std::env::var("TMUX_PANE") {
                    command.args(["-t", &pane]);
                }
                command.arg(shell_line(&line));
                command
            }
        };
        let child = command.current_dir(cwd).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
        let child = child.inspect_err(|_| {
            let _ = std::fs::remove_file(&file);
        })?;
        Ok(Window { file, opened: Instant::now(), child, reminded: false })
    }

    fn pid(&self) -> Option<i32> {
        std::fs::read_to_string(pid_file(&self.file)).ok()?.trim().parse().ok()
    }

    /// Why the menu is gone without the answers, once it is.
    pub fn gone(&mut self) -> Option<String> {
        // A terminal that hands the window to a running instance exits at
        // once, so the menu's own pid is what counts.
        let exited = self.child.try_wait().ok().flatten();
        match self.pid() {
            Some(pid) if !alive(pid) => Some("the user closed ekko's menu without answering".to_string()),
            Some(_) => None,
            None => match exited {
                Some(status) if !status.success() => Some(format!("ekko's menu could not open: the terminal exited with {status}")),
                _ if self.opened.elapsed() > OPENING => Some("ekko's menu did not open".to_string()),
                _ => None,
            },
        }
    }

    /// A desktop notification, once, when the menu has waited `REMIND`
    /// with no answer: the user may be looking elsewhere.
    pub fn remind(&mut self, question: &str) {
        if self.reminded || self.opened.elapsed() < REMIND || !self.pid().is_some_and(alive) {
            return;
        }
        self.reminded = true;
        let text: String = question.lines().next().unwrap_or_default().chars().take(200).collect();
        let _ = Command::new("busctl")
            .args(["--user", "call", "org.freedesktop.Notifications", "/org/freedesktop/Notifications", "org.freedesktop.Notifications", "Notify"])
            .args(["susssasa{sv}i", "ekko", "0", "dialog-question", "ekko: a question is waiting for you", &text, "0", "0", "15000"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(|mut child| std::thread::spawn(move || child.wait()));
    }

    /// Closes the menu, if it is still up, and forgets it.
    pub fn close(self) {
        if let Some(pid) = self.pid() {
            // SAFETY: kill only sends a signal; a pid gone by now fails harmlessly.
            unsafe { libc::kill(pid, libc::SIGTERM) };
        }
        let _ = std::fs::remove_file(pid_file(&self.file));
        let _ = std::fs::remove_file(&self.file);
        // Reaped once it exits, so no zombie is left behind: the terminal may
        // take a moment to close after the menu is gone.
        let mut child = self.child;
        std::thread::spawn(move || child.wait());
    }
}

/// Where the menu started on `file` writes its pid.
fn pid_file(file: &Path) -> PathBuf {
    file.with_extension("pid")
}

fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 checks the process exists and sends nothing.
    unsafe { libc::kill(pid, 0) == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM) }
}

/// The menu's command line: in `cwd`, where the board is found from, with the
/// variables that pick the board, and without CLAUDECODE, so the answers are
/// recorded as the user's -- which they are -- and not the asking session's.
/// Through `env` and `sh`, since a tmux popup runs it in tmux's environment,
/// and a terminal may start it in a folder of its own.
fn command_line(exe: &Path, file: &Path, project: Option<&str>, cwd: &Path) -> Vec<String> {
    let mut line: Vec<String> = ["env", "-u", "CLAUDECODE"].map(str::to_string).to_vec();
    for name in ["EKKO_DIR", "EKKO_PROJECT"] {
        if let Ok(value) = std::env::var(name) {
            line.push(format!("{name}={value}"));
        }
    }
    line.extend(["sh", "-c", "cd \"$0\" && exec \"$@\""].map(str::to_string));
    line.extend([cwd, exe].map(|path| path.to_string_lossy().into_owned()));
    line.extend(["--menu".to_string(), file.to_string_lossy().into_owned()]);
    if let Some(project) = project {
        line.extend(["--project".to_string(), project.to_string()]);
    }
    line
}

/// `line` as one shell command, each word quoted.
fn shell_line(line: &[String]) -> String {
    line.iter().map(|word| format!("'{}'", word.replace('\'', "'\\''"))).collect::<Vec<_>>().join(" ")
}

/// The terminal's command, sized to the menu where the terminal takes a size:
/// Konsole opens at its profile's size, most of it empty around a question.
fn sized(terminal: &[String], spec: &Spec) -> Vec<String> {
    let mut words = terminal[1..].to_vec();
    let konsole = Path::new(&terminal[0]).file_name().is_some_and(|name| name == "konsole");
    if let (true, Some(at)) = (konsole, words.iter().position(|word| word == "-e")) {
        let (columns, rows) = size(spec);
        let props = ["-p".to_string(), format!("TerminalColumns={columns}"), "-p".to_string(), format!("TerminalRows={rows}")];
        words.splice(at..at, props);
    }
    words
}

/// Columns and rows that hold the menu's longest page.
fn size(spec: &Spec) -> (usize, usize) {
    let previews: Vec<&str> = spec.questions.iter().flat_map(|q| &q.options).filter_map(|o| o.preview.as_deref()).collect();
    let preview_width = previews.iter().flat_map(|p| p.lines()).map(|l| l.chars().count()).max().unwrap_or(0);
    let widest = spec
        .questions
        .iter()
        .flat_map(|q| q.text.lines().map(|l| l.chars().count()).chain(q.options.iter().map(|o| o.label.chars().count().max(o.description.as_deref().map_or(0, |d| d.chars().count())) + 5)))
        .max()
        .unwrap_or(0);
    let columns = if previews.is_empty() { (widest + 4).clamp(72, 110) } else { (LEFT + 3 + preview_width + 4).clamp(100, 160) };
    let rows = spec
        .questions
        .iter()
        .map(|q| {
            let text: usize = q.text.lines().map(|l| l.chars().count() / columns + 1).sum();
            let options: usize = q.options.iter().map(|o| 1 + usize::from(o.description.is_some())).sum::<usize>() + 1;
            let preview = q.options.iter().filter_map(|o| o.preview.as_deref()).map(|p| p.lines().count() + 2).max().unwrap_or(0);
            text + options.max(preview) + 9
        })
        .max()
        .unwrap_or(0);
    (columns, rows.clamp(14, 45))
}

/// A key the menu acts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    Enter,
    Tab,
    Back,
    Esc,
    Char(char),
    /// The terminal took the focus (xterm's focus reporting, mode 1004): no
    /// key pressed, but what keys wait after (SETTLE).
    Focus,
}

/// Where the menu stands after a key.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    Stay,
    /// Every question has its answer: record them.
    Done,
    /// Leave every question open.
    Left,
}

/// Text being written: the answer "Other answer…" opened, or a note.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Editing {
    Other(String),
    Note(String),
}

/// One question in the menu, and where the user stands on it.
pub struct Page {
    pub posed: Posed,
    pub cursor: usize,
    /// Which options are marked, where several may be chosen.
    pub marked: Vec<bool>,
    /// The text written for "Other answer…".
    pub other: Option<String>,
    pub note: Option<String>,
    /// The answer chosen, before its note.
    pub answer: Option<String>,
}

impl Page {
    fn new(posed: Posed) -> Page {
        let marked = vec![false; posed.options.len()];
        Page { posed, cursor: 0, marked, other: None, note: None, answer: None }
    }

    /// The answer as the board records it: with the note, if there is one.
    fn recorded(&self) -> Option<String> {
        let answer = self.answer.as_ref()?;
        Some(match &self.note {
            Some(note) => format!("{answer} — note: {note}"),
            None => answer.clone(),
        })
    }
}

/// The menu's state: a page per question, and after them the review of every
/// answer, when there is more than one question.
pub struct Menu {
    pub pages: Vec<Page>,
    /// The page shown; `pages.len()` is the review.
    pub current: usize,
    editing: Option<Editing>,
}

impl Menu {
    pub fn new(questions: Vec<Posed>) -> Menu {
        let mut menu = Menu { pages: questions.into_iter().map(Page::new).collect(), current: 0, editing: None };
        menu.show(0);
        menu
    }

    /// The answers chosen, by the question's uid.
    pub fn answers(&self) -> Vec<(String, String)> {
        self.pages.iter().filter_map(|page| Some((page.posed.uid.clone(), page.recorded()?))).collect()
    }

    fn reviewing(&self) -> bool {
        self.current == self.pages.len()
    }

    /// Shows page `n`, or the review: a question without options opens at its text.
    fn show(&mut self, n: usize) {
        self.current = n;
        self.editing = self.pages.get(n).filter(|page| page.posed.options.is_empty()).map(|page| Editing::Other(page.other.clone().unwrap_or_default()));
    }

    /// Settles the current page's answer and moves on: to the next question
    /// still without one, or the review, or done when there is one question.
    fn settle(&mut self, answer: String) -> Step {
        self.pages[self.current].answer = Some(answer);
        if self.pages.len() == 1 {
            return Step::Done;
        }
        let count = self.pages.len();
        let next = (1..count).map(|k| (self.current + k) % count).find(|&n| self.pages[n].answer.is_none());
        self.show(next.unwrap_or(count));
        Step::Stay
    }

    pub fn press(&mut self, key: Key) -> Step {
        if let Some(editing) = self.editing.clone() {
            return self.write(editing, key);
        }
        if self.reviewing() {
            return self.review(key);
        }
        let several = self.pages.len() > 1;
        let page = &mut self.pages[self.current];
        let other = page.posed.options.len();
        match key {
            Key::Up | Key::Char('k') => page.cursor = page.cursor.checked_sub(1).unwrap_or(other),
            Key::Down | Key::Char('j') => page.cursor = if page.cursor == other { 0 } else { page.cursor + 1 },
            Key::Left | Key::Char('h') if several => self.show(self.current.saturating_sub(1)),
            Key::Right | Key::Char('l') if several => self.show(self.current + 1),
            Key::Tab => self.editing = Some(Editing::Note(page.note.clone().unwrap_or_default())),
            Key::Char(c @ '1'..='9') => {
                let n = c as usize - '1' as usize;
                if n <= other {
                    page.cursor = n;
                    return if page.posed.multiple { self.toggle() } else { self.choose() };
                }
            }
            // Space marks and never answers: of the keys the menu acts on, it
            // is the one most often typed for another window (task 593).
            Key::Char(' ') if page.posed.multiple => return self.toggle(),
            Key::Enter if !page.posed.multiple => return self.choose(),
            Key::Enter => return self.confirm(),
            Key::Esc | Key::Char('q') => return Step::Left,
            _ => {}
        }
        Step::Stay
    }

    /// One choice: the focused option, or the text field of "Other answer…".
    fn choose(&mut self) -> Step {
        let page = &self.pages[self.current];
        match page.posed.options.get(page.cursor) {
            Some(option) => self.settle(option.label.trim().to_string()),
            None => {
                self.editing = Some(Editing::Other(page.other.clone().unwrap_or_default()));
                Step::Stay
            }
        }
    }

    /// Several choices: marks or unmarks the focused option; "Other answer…"
    /// opens its text field, or drops the text written there.
    fn toggle(&mut self) -> Step {
        let page = &mut self.pages[self.current];
        match page.marked.get_mut(page.cursor) {
            Some(marked) => *marked = !*marked,
            None if page.other.is_some() => page.other = None,
            None => self.editing = Some(Editing::Other(String::new())),
        }
        Step::Stay
    }

    /// Several choices, confirmed: the ones marked, and the other answer. With
    /// none, Enter takes the focused option, as with a single choice.
    fn confirm(&mut self) -> Step {
        let page = &mut self.pages[self.current];
        if !page.marked.contains(&true) && page.other.is_none() {
            if page.cursor == page.posed.options.len() {
                return self.toggle();
            }
            page.marked[page.cursor] = true;
        }
        let labels = page.posed.options.iter().zip(&page.marked).filter(|(_, marked)| **marked).map(|(option, _)| option.label.trim().to_string());
        let answer = labels.chain(page.other.clone()).collect::<Vec<_>>().join(", ");
        self.settle(answer)
    }

    fn write(&mut self, editing: Editing, key: Key) -> Step {
        let several = self.pages.len() > 1;
        let page = &mut self.pages[self.current];
        let free = page.posed.options.is_empty();
        let (Editing::Other(mut text) | Editing::Note(mut text)) = editing.clone();
        match key {
            Key::Char(c) if !c.is_control() => text.push(c),
            Key::Back => {
                text.pop();
            }
            Key::Enter => {
                let written = text.trim().to_string();
                return match editing {
                    Editing::Note(_) => {
                        page.note = (!written.is_empty()).then_some(written);
                        self.show(self.current);
                        Step::Stay
                    }
                    Editing::Other(_) if written.is_empty() => Step::Stay,
                    Editing::Other(_) if page.posed.multiple => {
                        page.other = Some(written);
                        self.editing = None;
                        Step::Stay
                    }
                    Editing::Other(_) => {
                        page.other = Some(written.clone());
                        self.settle(written)
                    }
                };
            }
            // A question without options: its text is the page, so Tab
            // writes a note and the arrows move between questions.
            Key::Tab if free && matches!(editing, Editing::Other(_)) => {
                page.other = Some(text).filter(|t| !t.trim().is_empty());
                self.editing = Some(Editing::Note(page.note.clone().unwrap_or_default()));
                return Step::Stay;
            }
            Key::Left | Key::Right if free && several && matches!(editing, Editing::Other(_)) => {
                page.other = Some(text).filter(|t| !t.trim().is_empty());
                let n = if key == Key::Left { self.current.saturating_sub(1) } else { self.current + 1 };
                self.show(n);
                return Step::Stay;
            }
            Key::Esc if free && matches!(editing, Editing::Other(_)) => return Step::Left,
            Key::Esc => {
                self.show(self.current);
                return Step::Stay;
            }
            _ => return Step::Stay,
        }
        self.editing = Some(match editing {
            Editing::Other(_) => Editing::Other(text),
            Editing::Note(_) => Editing::Note(text),
        });
        Step::Stay
    }

    fn review(&mut self, key: Key) -> Step {
        match key {
            Key::Enter => match self.pages.iter().position(|page| page.answer.is_none()) {
                Some(n) => self.show(n),
                None => return Step::Done,
            },
            Key::Left | Key::Char('h') => self.show(self.pages.len() - 1),
            Key::Char(c @ '1'..='9') if (c as usize - '1' as usize) < self.pages.len() => self.show(c as usize - '1' as usize),
            Key::Esc | Key::Char('q') => return Step::Left,
            _ => {}
        }
        Step::Stay
    }

    /// The whole screen, drawn from the top, for a terminal `width` columns wide.
    pub fn draw(&self, width: usize) -> String {
        let first = self.pages[0].posed.id;
        let mut out = String::from("\x1b[H\x1b[2J");
        if self.pages.len() == 1 {
            out.push_str(&format!("\x1b]0;ekko: question {first}\x07\x1b[2mekko · question {first}\x1b[0m\r\n\r\n"));
        } else {
            out.push_str(&format!("\x1b]0;ekko: {} questions\x07\x1b[2mekko · {} questions\x1b[0m  ", self.pages.len(), self.pages.len()));
            for (n, page) in self.pages.iter().enumerate() {
                let tab = format!(" {}{} ", n + 1, if page.answer.is_some() { " ✓" } else { "" });
                out.push_str(&if n == self.current { format!("\x1b[7m{tab}\x1b[0m ") } else { format!("{tab} ") });
            }
            out.push_str(&if self.reviewing() { "\x1b[7m Review \x1b[0m".to_string() } else { " Review ".to_string() });
            out.push_str("\r\n\r\n");
        }
        if self.reviewing() {
            out.push_str("\x1b[1mYour answers\x1b[0m\r\n\r\n");
            for (n, page) in self.pages.iter().enumerate() {
                out.push_str(&format!("  {}. {}\r\n", n + 1, fit(page.posed.text.lines().next().unwrap_or_default(), width.saturating_sub(5))));
                out.push_str(&match page.recorded() {
                    Some(answer) => format!("     \x1b[36m→ {}\x1b[0m\r\n", fit(&answer, width.saturating_sub(7))),
                    None => "     \x1b[2m→ no answer yet\x1b[0m\r\n".to_string(),
                });
            }
            out.push_str("\r\n\x1b[?25l\x1b[2mEnter to record them · ← or a number to change one · Esc to leave them open\x1b[0m");
            return out;
        }
        let page = &self.pages[self.current];
        out.push_str(&format!("\x1b[1m{}\x1b[0m\r\n", page.posed.text.trim().replace('\n', "\r\n")));
        if page.posed.multiple {
            out.push_str("\x1b[2mAny number: Space marks, Enter confirms\x1b[0m\r\n");
        }
        out.push_str("\r\n");
        if !page.posed.options.is_empty() {
            out.push_str(&self.options(page, width));
        }
        if let Some(note) = &page.note {
            out.push_str(&format!("\r\n\x1b[2mNote: {note}\x1b[0m\r\n"));
        }
        let several = if self.pages.len() > 1 { " · ←/→ between questions" } else { "" };
        match &self.editing {
            Some(Editing::Other(text)) => {
                let back = if page.posed.options.is_empty() { "Esc to leave it open" } else { "Esc to go back to the options" };
                let note = if page.posed.options.is_empty() { " · Tab for a note" } else { "" };
                let hint = format!("Enter to record it{note}{} · {back}", if page.posed.options.is_empty() { several } else { "" });
                out.push_str(&format!("\r\nYour answer: {text}\x1b7\r\n\r\n\x1b[2m{hint}\x1b[0m\x1b8\x1b[?25h"));
            }
            Some(Editing::Note(text)) => {
                out.push_str(&format!("\r\nNote: {text}\x1b7\r\n\r\n\x1b[2mEnter to keep it · Esc to drop the change\x1b[0m\x1b8\x1b[?25h"));
            }
            None => {
                let pick = if page.posed.multiple { "Space to mark · Enter to confirm" } else { "Enter or a number to choose" };
                out.push_str(&format!("\r\n\x1b[?25l\x1b[2m↑/↓ to move · {pick} · Tab for a note{several} · Esc to leave it open\x1b[0m"));
            }
        }
        out
    }

    /// The options, and beside them -- or under them, in a narrow terminal --
    /// the preview of the focused one, when any option has one.
    fn options(&self, page: &Page, width: usize) -> String {
        let previews = page.posed.options.iter().any(|option| option.preview.is_some());
        let beside = previews && width >= 90;
        let room = if beside { LEFT } else { width };
        let mut lines: Vec<(String, String)> = Vec::new();
        let other = Choice { label: "Other answer…".to_string(), description: None, preview: None };
        for (n, option) in page.posed.options.iter().chain(std::iter::once(&other)).enumerate() {
            let focused = n == page.cursor && self.editing.is_none();
            let label = match (n == page.posed.options.len(), &page.other) {
                (true, Some(text)) => format!("Other answer: {text}"),
                _ => option.label.trim().to_string(),
            };
            let mark = match page.marked.get(n) {
                _ if !page.posed.multiple => String::new(),
                Some(true) => "[✓] ".to_string(),
                None if page.other.is_some() => "[✓] ".to_string(),
                _ => "[ ] ".to_string(),
            };
            let chosen = page.answer.as_deref() == Some(option.label.trim()) && !page.posed.multiple;
            let plain = fit(&format!("{}. {mark}{label}{}", n + 1, if chosen { " ✓" } else { "" }), room.saturating_sub(2));
            let styled = if focused { format!("\x1b[36m❯ \x1b[1m{plain}\x1b[0m") } else { format!("  {plain}") };
            lines.push((format!("  {plain}"), styled));
            if let Some(description) = option.description.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
                let plain = fit(description, room.saturating_sub(5));
                lines.push((format!("     {plain}"), format!("     \x1b[2m{plain}\x1b[0m")));
            }
        }
        let preview = page.posed.options.get(page.cursor).and_then(|option| option.preview.as_deref());
        let boxed = previews.then(|| framed(preview.unwrap_or("(no preview)"), if beside { width - LEFT - 3 } else { width.min(100) }));
        let mut out = String::new();
        if beside {
            let boxed = boxed.unwrap_or_default();
            for n in 0..lines.len().max(boxed.len()) {
                let (plain, styled) = lines.get(n).cloned().unwrap_or_default();
                let pad = " ".repeat(LEFT.saturating_sub(plain.chars().count()));
                out.push_str(&format!("{styled}{pad}   {}\r\n", boxed.get(n).map_or("", String::as_str)));
            }
        } else {
            for (_, styled) in &lines {
                out.push_str(&format!("{styled}\r\n"));
            }
            for line in boxed.unwrap_or_default() {
                out.push_str(&format!("{line}\r\n"));
            }
        }
        out
    }
}

/// How wide the options run when a preview sits beside them.
const LEFT: usize = 46;

/// `text` cut to `width` characters, an ellipsis marking the cut.
fn fit(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    text.chars().take(width.saturating_sub(1)).chain(std::iter::once('…')).collect()
}

/// `text` in a box `width` characters wide.
fn framed(text: &str, width: usize) -> Vec<String> {
    let inner = width.saturating_sub(4).max(1);
    let mut lines = vec![format!("┌{}┐", "─".repeat(inner + 2))];
    for line in text.lines() {
        let line = fit(line, inner);
        lines.push(format!("│ {line}{} │", " ".repeat(inner - line.chars().count())));
    }
    lines.push(format!("└{}┘", "─".repeat(inner + 2)));
    lines
}

/// The question's text, the options it offers and whether several may be
/// chosen, read back from the note `dialog::noted` recorded: an options list
/// last, one "- label: description" per line.
pub fn parse(noted: &str) -> (String, Vec<Choice>, bool) {
    for (header, multiple) in [(dialog::MULTIPLE, true), ("\nOptions:", false)] {
        let Some((text, list)) = noted.rsplit_once(header) else { continue };
        let lines: Vec<&str> = list.lines().filter(|line| !line.is_empty()).collect();
        let Some(options) = lines.iter().map(|line| line.strip_prefix("- ")).collect::<Option<Vec<_>>>() else { continue };
        let options = options
            .into_iter()
            .map(|option| match option.split_once(": ") {
                Some((label, description)) => Choice { label: label.to_string(), description: Some(description.to_string()), preview: None },
                None => Choice { label: option.to_string(), description: None, preview: None },
            })
            .collect();
        return (text.to_string(), options, multiple);
    }
    (noted.to_string(), Vec::new(), false)
}

/// `ekko --menu <file>`: the menu ask opened, on the questions in `file`.
pub fn run_file(ekko: &Ekko, file: &Path) -> Result<Vec<Outcome>, EkkoError> {
    let _ = std::fs::write(pid_file(file), std::process::id().to_string());
    let spec: Spec = std::fs::read(file)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .ok_or_else(|| EkkoError::InvalidInput(format!("no questions in {}", file.display())))?;
    if spec.questions.is_empty() {
        return Err(EkkoError::InvalidInput(format!("no questions in {}", file.display())));
    }
    run(ekko, spec.questions)
}

/// `ekko --answer <question>` in a terminal: the menu on one question, read
/// back from the board -- one left open when its session ended, say.
pub fn run_one(ekko: &Ekko, reference: &str) -> Result<Vec<Outcome>, EkkoError> {
    // Read without the board's lock: the user takes their time, and every
    // other session's writes go on meanwhile.
    let data = ekko.storage.get()?;
    let found = match reference.parse::<u32>() {
        Ok(id) => data.get(&id),
        Err(_) => data.values().find(|item| item.uid.as_deref() == Some(reference)),
    };
    let item = found.ok_or_else(|| EkkoError::InvalidInput(format!("no question {reference} on this board")))?;
    match &item.question {
        None => return Err(EkkoError::InvalidInput(format!("{} is not a question", item.id))),
        Some(asked) if asked.answer.is_some() => return Err(EkkoError::InvalidInput(format!("{} was already answered", item.id))),
        Some(_) => {}
    }
    let (text, options, multiple) = parse(&item.description);
    let uid = item.uid.clone().unwrap_or_else(|| item.id.to_string());
    run(ekko, vec![Posed { uid, id: item.id, text, options, multiple }])
}

fn run(ekko: &Ekko, questions: Vec<Posed>) -> Result<Vec<Outcome>, EkkoError> {
    let mut menu = Menu::new(questions);
    let done = {
        let _raw = Raw::enter()?;
        let mut out = io::stdout();
        // Mode 1004: the terminal reports taking the focus, and keys wait
        // SETTLE after it as after the first draw.
        let _ = write!(out, "\x1b[?1004h");
        let mut settle = Settle::new(Instant::now());
        loop {
            let _ = write!(out, "{}", menu.draw(columns()));
            let _ = out.flush();
            let mut keys = std::iter::from_fn(|| read_key(&mut Keys, follows));
            let Some(key) = keys.find(|key| settle.counts(key, Instant::now())) else { break false };
            match menu.press(key) {
                Step::Stay => {}
                Step::Done => break true,
                Step::Left => break false,
            }
        }
    };
    let mut out = io::stdout();
    let _ = write!(out, "\x1b[?1004l\x1b[?25h\x1b[H\x1b[2J");
    let _ = out.flush();
    if !done {
        let ids: Vec<String> = menu.pages.iter().map(|page| page.posed.id.to_string()).collect();
        let _ = writeln!(out, "Left open: {}", ids.join(", "));
        return Ok(Vec::new());
    }
    let mut draft = Draft::open(ekko)?;
    let mut answered = Vec::new();
    for (uid, text) in menu.answers() {
        // One answered meanwhile, from elsewhere, keeps that answer; the
        // others are still recorded.
        match draft.answer(&Ref::Text(uid), &text) {
            Ok(id) => answered.push(id),
            Err(error) => {
                let _ = writeln!(io::stderr(), "{error}");
            }
        }
    }
    let committed = draft.commit(false)?;
    Ok(answered.into_iter().map(|id| Outcome::Answered(committed.data[&id].clone())).collect())
}

/// When keys start to count: SETTLE after the menu is drawn.
struct Settle(Instant);

impl Settle {
    fn new(drawn: Instant) -> Settle {
        Settle(drawn + SETTLE)
    }

    /// Whether `key`, read at `now`, counts. The start moves, where that is
    /// later, to SETTLE after the terminal takes the focus, and to QUIET
    /// after a key dropped.
    fn counts(&mut self, key: &Key, now: Instant) -> bool {
        let wait = match key {
            Key::Focus => SETTLE,
            _ if now < self.0 => QUIET,
            _ => return true,
        };
        self.0 = self.0.max(now + wait);
        false
    }
}

/// The terminal's width, or a guess where it cannot say.
fn columns() -> usize {
    // SAFETY: winsize is plain data, filled in by the ioctl when it succeeds.
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    if unsafe { libc::ioctl(1, libc::TIOCGWINSZ, &mut size) } == 0 && size.ws_col > 0 {
        usize::from(size.ws_col)
    } else {
        100
    }
}

/// Set by SIGTERM -- the server closing the menu of a call the client gave
/// up on -- or SIGHUP, the window closed: the menu then ends as Esc ends it.
static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn stop(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

/// The terminal in raw mode, for keys one at a time, until dropped.
struct Raw(libc::termios);

impl Raw {
    fn enter() -> Result<Raw, EkkoError> {
        if !io::stdin().is_terminal() {
            return Err(EkkoError::InvalidInput("the menu needs a terminal; give the answer's text after the id".into()));
        }
        // SAFETY: a handler that only stores to an atomic, installed without
        // SA_RESTART so that a read waiting for a key returns.
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = stop as extern "C" fn(libc::c_int) as libc::sighandler_t;
            libc::sigemptyset(&mut action.sa_mask);
            libc::sigaction(libc::SIGTERM, &action, std::ptr::null_mut());
            libc::sigaction(libc::SIGHUP, &action, std::ptr::null_mut());
        }
        // SAFETY: termios is plain data, filled in by tcgetattr before use.
        let mut saved: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(0, &mut saved) } != 0 {
            return Err(EkkoError::InvalidInput(format!("could not read the terminal's mode: {}", io::Error::last_os_error())));
        }
        let mut raw = saved;
        raw.c_lflag &= !(libc::ICANON | libc::ECHO | libc::ISIG | libc::IEXTEN);
        raw.c_iflag &= !(libc::IXON | libc::ICRNL);
        raw.c_cc[libc::VMIN] = 1;
        raw.c_cc[libc::VTIME] = 0;
        // SAFETY: raw is a valid termios, a copy of the one just read.
        unsafe { libc::tcsetattr(0, libc::TCSANOW, &raw) };
        Ok(Raw(saved))
    }
}

impl Drop for Raw {
    fn drop(&mut self) {
        // SAFETY: restores the mode read in enter.
        unsafe { libc::tcsetattr(0, libc::TCSANOW, &self.0) };
    }
}

/// Standard input a byte at a time, with no buffer in between: a buffer would
/// hold the rest of an arrow's escape sequence where `follows` cannot see
/// it, and the arrow would read as Esc. It reads as ended once STOP is set.
struct Keys;

impl Read for Keys {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            if STOP.load(Ordering::Relaxed) {
                return Ok(0);
            }
            // SAFETY: buf is valid for writes of its length.
            let read = unsafe { libc::read(0, buf.as_mut_ptr().cast(), buf.len()) };
            if let Ok(read) = usize::try_from(read) {
                return Ok(read);
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }
}

/// Whether another byte arrives within a few milliseconds: what tells the Esc
/// key from the start of an arrow's escape sequence.
fn follows() -> bool {
    let mut poll = libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 };
    // SAFETY: one valid pollfd, for the length given.
    unsafe { libc::poll(&mut poll, 1, 30) > 0 }
}

/// The next key, or None when the terminal is gone.
fn read_key(keys: &mut impl Read, follows: impl Fn() -> bool) -> Option<Key> {
    let mut byte = [0u8; 1];
    loop {
        keys.read_exact(&mut byte).ok()?;
        return Some(match byte[0] {
            b'\r' | b'\n' => Key::Enter,
            b'\t' => Key::Tab,
            0x7f | 0x08 => Key::Back,
            // Ctrl-C and Ctrl-D: leave, as Esc does.
            0x03 | 0x04 => Key::Esc,
            0x1b if !follows() => Key::Esc,
            0x1b => {
                keys.read_exact(&mut byte).ok()?;
                let intro = byte[0];
                if intro != b'[' && intro != b'O' {
                    continue;
                }
                // The sequence runs to its final letter: ESC [ 1 ; 5 A too.
                loop {
                    keys.read_exact(&mut byte).ok()?;
                    if byte[0].is_ascii_alphabetic() || byte[0] == b'~' {
                        break;
                    }
                }
                match byte[0] {
                    b'A' => Key::Up,
                    b'B' => Key::Down,
                    b'C' => Key::Right,
                    b'D' => Key::Left,
                    // ESC [ I: the focus came; ESC [ O, it left, is skipped.
                    b'I' if intro == b'[' => Key::Focus,
                    _ => continue,
                }
            }
            first if first < 0x80 => Key::Char(first as char),
            first => {
                let len = if first >= 0xf0 { 4 } else if first >= 0xe0 { 3 } else { 2 };
                let mut bytes = vec![first];
                let mut rest = vec![0u8; len - 1];
                keys.read_exact(&mut rest).ok()?;
                bytes.extend(rest);
                match std::str::from_utf8(&bytes).ok().and_then(|s| s.chars().next()) {
                    Some(c) => Key::Char(c),
                    None => continue,
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choice(label: &str, description: Option<&str>) -> Choice {
        Choice { label: label.to_string(), description: description.map(str::to_string), preview: None }
    }

    fn posed(id: u32, text: &str, options: Vec<Choice>, multiple: bool) -> Posed {
        Posed { uid: format!("uid-{id}"), id, text: text.to_string(), options, multiple }
    }

    fn yes_no() -> Vec<Choice> {
        vec![choice("Yes", None), choice("No", None)]
    }

    fn press(menu: &mut Menu, keys: &[Key]) -> Step {
        keys.iter().map(|key| menu.press(key.clone())).last().unwrap_or(Step::Stay)
    }

    fn typed(text: &str) -> Vec<Key> {
        text.chars().map(Key::Char).collect()
    }

    #[test]
    fn the_options_are_read_back_from_the_recorded_note() {
        let options = vec![choice("469 (recommended)", Some("the replaced-binary note: under Nix")), choice("396", None)];
        let (text, read, multiple) = parse(&dialog::noted("Which next?\nOptions: none of these, in the text", &options, false));
        assert_eq!(text, "Which next?\nOptions: none of these, in the text");
        assert!(!multiple);
        assert_eq!(read.len(), 2);
        assert_eq!((read[0].label.as_str(), read[0].description.as_deref()), ("469 (recommended)", Some("the replaced-binary note: under Nix")));
        assert_eq!((read[1].label.as_str(), read[1].description.as_deref()), ("396", None));
        let (_, read, multiple) = parse(&dialog::noted("Which ones?", &options, true));
        assert!(multiple);
        assert_eq!(read.len(), 2);
        let (text, options, multiple) = parse("Just a question?");
        assert_eq!((text.as_str(), options.len(), multiple), ("Just a question?", 0, false));
    }

    #[test]
    fn the_arrows_move_and_enter_or_a_number_chooses() {
        let mut menu = Menu::new(vec![posed(1, "Q?", yes_no(), false)]);
        assert_eq!(menu.press(Key::Up), Step::Stay);
        assert_eq!(menu.pages[0].cursor, 2, "up from the first wraps to Other answer…");
        assert_eq!(press(&mut menu, &[Key::Down, Key::Down]), Step::Stay);
        assert_eq!(menu.press(Key::Enter), Step::Done);
        assert_eq!(menu.answers(), [("uid-1".to_string(), "No".to_string())]);
        let mut menu = Menu::new(vec![posed(1, "Q?", yes_no(), false)]);
        assert_eq!(menu.press(Key::Char('9')), Step::Stay, "a number past the options does nothing");
        assert_eq!(menu.press(Key::Char('1')), Step::Done);
        assert_eq!(menu.answers()[0].1, "Yes");
        assert_eq!(Menu::new(vec![posed(1, "Q?", yes_no(), false)]).press(Key::Esc), Step::Left);
        assert_eq!(Menu::new(vec![posed(1, "Q?", yes_no(), false)]).press(Key::Char(' ')), Step::Stay, "space does not choose");
    }

    #[test]
    fn keys_count_once_the_menu_has_been_up_a_second_and_the_keyboard_is_quiet() {
        let drawn = Instant::now();
        let at = |ms: u64| drawn + Duration::from_millis(ms);
        let mut settle = Settle::new(drawn);
        assert!(!settle.counts(&Key::Char(' '), at(300)), "typed for another window as the menu opened");
        assert!(!settle.counts(&Key::Char('e'), at(900)));
        assert!(!settle.counts(&Key::Enter, at(1200)), "typing that runs on past the second is dropped too");
        assert!(settle.counts(&Key::Enter, at(1700)), "the keyboard was quiet for {QUIET:?}");
        assert!(!settle.counts(&Key::Focus, at(5000)), "the focus is no key");
        assert!(!settle.counts(&Key::Char('1'), at(5500)), "the menu back in focus waits again");
        assert!(settle.counts(&Key::Char('1'), at(6000)));
    }

    #[test]
    fn other_answer_opens_a_text_and_esc_goes_back() {
        let mut menu = Menu::new(vec![posed(1, "Q?", yes_no(), false)]);
        menu.press(Key::Char('3'));
        press(&mut menu, &typed("só às"));
        menu.press(Key::Back);
        assert_eq!(menu.editing, Some(Editing::Other("só à".into())));
        assert_eq!(menu.press(Key::Esc), Step::Stay);
        assert!(menu.editing.is_none(), "Esc goes back to the options");
        menu.press(Key::Char('3'));
        assert_eq!(menu.press(Key::Enter), Step::Stay, "an empty answer is not recorded");
        press(&mut menu, &typed("x"));
        assert_eq!(menu.press(Key::Enter), Step::Done);
        assert_eq!(menu.answers()[0].1, "x");
    }

    #[test]
    fn a_question_without_options_is_written_at_once() {
        let mut menu = Menu::new(vec![posed(1, "Why?", Vec::new(), false)]);
        assert_eq!(press(&mut menu, &typed("kj")), Step::Stay, "letters are text here, not moves");
        assert_eq!(menu.press(Key::Enter), Step::Done);
        assert_eq!(menu.answers()[0].1, "kj");
        assert_eq!(Menu::new(vec![posed(1, "Why?", Vec::new(), false)]).press(Key::Esc), Step::Left);
    }

    #[test]
    fn several_options_are_marked_with_space_and_confirmed_with_enter() {
        let options = vec![choice("A", None), choice("B", None), choice("C", None)];
        let mut menu = Menu::new(vec![posed(1, "Which?", options.clone(), true)]);
        press(&mut menu, &[Key::Char(' '), Key::Down, Key::Down, Key::Char(' '), Key::Char('2'), Key::Char('2')]);
        assert_eq!(menu.pages[0].marked, [true, false, true], "a number toggles too");
        press(&mut menu, &[Key::Char('4')]);
        press(&mut menu, &typed("D too"));
        assert_eq!(menu.press(Key::Enter), Step::Stay, "the other answer joins the marks");
        assert_eq!(menu.press(Key::Enter), Step::Done);
        assert_eq!(menu.answers()[0].1, "A, C, D too");

        let mut menu = Menu::new(vec![posed(1, "Which?", options, true)]);
        menu.press(Key::Down);
        assert_eq!(menu.press(Key::Enter), Step::Done, "with nothing marked, Enter takes the focused option");
        assert_eq!(menu.answers()[0].1, "B");
    }

    #[test]
    fn a_note_goes_with_the_answer() {
        let mut menu = Menu::new(vec![posed(1, "Q?", yes_no(), false)]);
        menu.press(Key::Tab);
        press(&mut menu, &typed("but ask again in May"));
        menu.press(Key::Enter);
        assert_eq!(menu.pages[0].note.as_deref(), Some("but ask again in May"));
        assert_eq!(menu.press(Key::Enter), Step::Done);
        assert_eq!(menu.answers()[0].1, "Yes — note: but ask again in May");
        let mut menu = Menu::new(vec![posed(1, "Q?", yes_no(), false)]);
        press(&mut menu, &[Key::Tab, Key::Char('x'), Key::Esc]);
        assert!(menu.pages[0].note.is_none(), "Esc drops the note");
    }

    #[test]
    fn several_questions_move_on_and_end_in_a_review() {
        let mut menu = Menu::new(vec![posed(1, "First?", yes_no(), false), posed(2, "Second?", yes_no(), false), posed(3, "Why?", Vec::new(), false)]);
        assert_eq!(menu.press(Key::Char('2')), Step::Stay, "one answer of three is not the end");
        assert_eq!(menu.current, 1, "the next question comes up");
        menu.press(Key::Right);
        assert_eq!(menu.current, 2);
        press(&mut menu, &typed("because"));
        menu.press(Key::Left);
        assert_eq!(menu.current, 1, "the arrows leave a text question, keeping its draft");
        assert_eq!(menu.pages[2].other.as_deref(), Some("because"));
        menu.press(Key::Char('1'));
        assert_eq!(menu.current, 2, "the question still without an answer comes up");
        menu.press(Key::Enter);
        assert!(menu.reviewing());
        menu.press(Key::Char('1'));
        assert_eq!(menu.current, 0, "a number in the review goes back to that question");
        menu.press(Key::Char('1'));
        assert!(menu.reviewing(), "every question answered: back to the review");
        assert_eq!(menu.press(Key::Enter), Step::Done);
        let answers: Vec<String> = menu.answers().into_iter().map(|(_, answer)| answer).collect();
        assert_eq!(answers, ["Yes", "Yes", "because"]);
    }

    #[test]
    fn the_review_sends_back_to_a_question_without_an_answer() {
        let mut menu = Menu::new(vec![posed(1, "First?", yes_no(), false), posed(2, "Second?", yes_no(), false)]);
        press(&mut menu, &[Key::Right, Key::Right]);
        assert!(menu.reviewing());
        assert_eq!(menu.press(Key::Enter), Step::Stay);
        assert_eq!(menu.current, 0);
        assert_eq!(menu.press(Key::Esc), Step::Left);
    }

    #[test]
    fn a_preview_sits_beside_the_options_or_under_them_when_narrow() {
        let mut options = yes_no();
        options[0].preview = Some("fn main() {}\n// yes".into());
        let menu = Menu::new(vec![posed(7, "Which?", options, false)]);
        let wide = menu.draw(120);
        let line = wide.split("\r\n").find(|line| line.contains("1. Yes")).unwrap();
        assert!(line.contains("┌"), "the preview's box starts on the first option's line: {line:?}");
        assert!(wide.contains("│ fn main() {}"), "{wide}");
        let narrow = menu.draw(60);
        let at = |text: &str| narrow.find(text).unwrap_or_else(|| panic!("{text} not in {narrow}"));
        assert!(at("Other answer…") < at("│ fn main() {}"), "under the options when narrow");
        assert!(wide.contains("ekko · question 7"));
    }

    #[test]
    fn keys_are_read_from_their_bytes() {
        let mut keys: &[u8] = b"\x1b[I\x1b[A\x1b[1;5B\x1b[O\x1b[C\x1bOD\t\rx\x7f\xc3\xa0";
        let read: Vec<Key> = std::iter::from_fn(|| read_key(&mut keys, || true)).collect();
        assert_eq!(read, [Key::Focus, Key::Up, Key::Down, Key::Right, Key::Left, Key::Tab, Key::Enter, Key::Char('x'), Key::Back, Key::Char('à')]);
    }

    #[test]
    fn the_menu_runs_in_the_servers_folder_as_the_user_quoted_for_tmux() {
        let line = command_line(Path::new("/nix/store/x/bin/ekko"), Path::new("/run/ekko-menu-1-0.json"), Some("it's"), Path::new("/home/a b"));
        let at = line.iter().position(|word| word == "sh").unwrap();
        assert_eq!(&line[..3], ["env", "-u", "CLAUDECODE"]);
        assert_eq!(&line[at..], ["sh", "-c", "cd \"$0\" && exec \"$@\"", "/home/a b", "/nix/store/x/bin/ekko", "--menu", "/run/ekko-menu-1-0.json", "--project", "it's"]);
        assert!(shell_line(&line).ends_with("'--project' 'it'\\''s'"), "{}", shell_line(&line));
        let spec = Spec { questions: vec![posed(1, "Q?", yes_no(), false)] };
        let words = sized(&["konsole".to_string(), "--separate".to_string(), "-e".to_string()], &spec);
        assert_eq!(words[0], "--separate");
        assert!(words[1] == "-p" && words[2].starts_with("TerminalColumns=") && words[4].starts_with("TerminalRows="), "{words:?}");
        assert_eq!(words.last().unwrap(), "-e");
        assert_eq!(sized(&["xterm".to_string(), "-e".to_string()], &spec), ["-e"]);
    }
}
