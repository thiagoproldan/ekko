//! `ekko agents view` (task 1676): on one screen, the sessions `ekko agents
//! start` opened in the multiplexer's session `ekko`, grouped by the state
//! their hooks keep on their panes (task 1675) -- waiting on you, working,
//! idle -- and the tasks born sessions finished today, each with the first
//! line of its closing note. In a terminal, outside the multiplexer or in a
//! popup of it, the arrows choose a session, Enter steps into its window,
//! Space peeks at its screen and q leaves; the panes and the board are read
//! again every second. Printed once with --once, or with no terminal; as one
//! JSON object with --json (task 1861), for a status line. It shows the
//! sessions of every project, each read on the board of its own.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, IsTerminal as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::TimeZone as _;

use crate::agents::{self, BornPane, Mux};
use crate::ekko::{Ekko, EkkoError};
use crate::item::{Item, State};
use crate::menu::{self, Key};
use crate::storage::ItemMap;

/// How often the view reads the panes and the board again.
const EVERY: Duration = Duration::from_secs(1);

/// The widest a title is drawn before it is cut.
const TITLE: usize = 48;

/// The groups of the view, in the order it draws them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Group {
    Waiting,
    #[default]
    Working,
    Idle,
    Finished,
}

impl Group {
    /// The symbol every view draws it with (decision 1674).
    pub fn symbol(self) -> char {
        match self {
            Group::Waiting => '\u{25cf}',
            Group::Working => '\u{2699}',
            Group::Idle => '\u{25cb}',
            Group::Finished => '\u{2713}',
        }
    }

    fn heading(self) -> &'static str {
        match self {
            Group::Waiting => "Waiting on you",
            Group::Working => "Working",
            Group::Idle => "Idle",
            Group::Finished => "Finished today",
        }
    }

    /// A pane's state, as its hooks keep it; working where none is kept,
    /// as `start` leaves a new one.
    fn of(state: &str) -> Group {
        match state {
            agents::WAITING => Group::Waiting,
            agents::IDLE => Group::Idle,
            _ => Group::Working,
        }
    }

    /// The word `--json` says it with: the state of a session that runs, or
    /// `done` for a task finished today.
    fn word(self) -> &'static str {
        match self {
            Group::Waiting => "waiting",
            Group::Working => "working",
            Group::Idle => "idle",
            Group::Finished => "done",
        }
    }

    /// How its heading is drawn in colour: as the status line draws the
    /// same state (task 1675).
    fn style(self) -> &'static str {
        match self {
            Group::Waiting => "\x1b[1;30;43m",
            Group::Working | Group::Finished => "\x1b[1m",
            Group::Idle => "\x1b[2m",
        }
    }
}

/// A line of the view: a session that runs, or a task finished today.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Row {
    pub group: Group,
    /// The project whose tasks `ids` are; none on the default board.
    pub project: Option<String>,
    /// The session's pane and window; none for a finished task.
    pub pane: Option<String>,
    pub window: Option<String>,
    pub ids: Vec<u32>,
    pub title: String,
    /// Its model and effort; for a finished task, how and when it ended.
    pub how: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// Since when it is in its state, `HH:MM`.
    pub since: Option<String>,
    /// The same in seconds since the epoch; for a finished task, when it
    /// ended.
    pub at: Option<i64>,
    /// For a finished task, how it ended: `done` or `cancelled`.
    pub ended: Option<&'static str>,
    /// What it waits on; for a finished task, its closing note's first line.
    pub detail: Option<String>,
}

/// Every project's board the view reads, by the project's name; none for the
/// default board.
pub type Boards = BTreeMap<Option<String>, Arc<ItemMap>>;

/// The rows of the view: each pane `start` opened that still runs, by its
/// state, then each task named in today's `born.log` that is now done or
/// cancelled and has no such pane, with the newest note attached to it. A
/// pane is read on the board of its project, `current` where it names none;
/// a task of `born.log` is found by its uid on any board, or by its id on
/// `current`.
pub fn rows(panes: &[BornPane], boards: &Boards, current: &Option<String>, today: &[String]) -> Vec<Row> {
    let none = ItemMap::new();
    let mut rows = Vec::new();
    let mut running = BTreeSet::new();
    for pane in panes.iter().filter(|pane| !pane.dead) {
        let project = pane.project.clone().or_else(|| current.clone());
        let all = boards.get(&project).map_or(&none, |all| &**all);
        running.extend(pane.tasks.iter().map(|id| (project.clone(), *id)));
        let title = match (pane.tasks.first().and_then(|id| all.get(id)), pane.tasks.len()) {
            (Some(first), 1) => crate::ekko::title(&first.description).to_string(),
            (Some(first), n) => format!("{} (+{})", crate::ekko::title(&first.description), n - 1),
            (None, _) => pane.title.clone(),
        };
        let how = match &pane.effort {
            Some(effort) => format!("{} \u{b7} {effort}", pane.model),
            None => pane.model.clone(),
        };
        let group = Group::of(&pane.state);
        let detail = pane.waits.clone().filter(|_| group == Group::Waiting);
        rows.push(Row {
            group,
            project,
            pane: Some(pane.pane.clone()),
            window: Some(pane.window.clone()),
            ids: pane.tasks.clone(),
            title,
            how,
            model: Some(pane.model.clone()),
            effort: pane.effort.clone(),
            since: pane.since.map(|at| clock(at * 1000)),
            at: pane.since,
            detail,
            ended: None,
        });
    }
    let indexes: Vec<(&Option<String>, &ItemMap, _)> = boards.iter().map(|(project, all)| (project, &**all, crate::ekko::uid_index(all))).collect();
    let mut listed = BTreeSet::new();
    for line in today {
        let Some(tasks) = line.split(' ').nth(2) else { continue };
        for name in tasks.split(',').filter(|name| !name.is_empty()) {
            let found = indexes.iter().find_map(|(project, all, index)| Some((*project, *all, *index.get(name)?))).or_else(|| {
                let (project, all, _) = indexes.iter().find(|(project, _, _)| *project == current)?;
                Some((*project, *all, name.parse().ok()?))
            });
            let Some((project, all, id)) = found else { continue };
            let Some(task) = all.get(&id) else { continue };
            if running.contains(&(project.clone(), task.id)) || !listed.insert((project.clone(), task.id)) {
                continue;
            }
            let ended = match State::of(task) {
                Some(State::Done) => "done",
                Some(State::Cancelled) => "cancelled",
                _ => continue,
            };
            let how = match task.updated_at {
                Some(at) => format!("{ended} {}", clock(at)),
                None => ended.to_string(),
            };
            let detail = closing_note(all, task).map(|note| crate::ekko::title(&note.description).to_string());
            rows.push(Row {
                group: Group::Finished,
                project: project.clone(),
                ids: vec![task.id],
                title: crate::ekko::title(&task.description).to_string(),
                how,
                at: task.updated_at.map(|at| at / 1000),
                ended: Some(ended),
                detail,
                ..Row::default()
            });
        }
    }
    rows.sort_by_key(|row| (row.group, row.project.clone(), row.ids.first().copied()));
    rows
}

/// The newest note attached to `task`, out of the trash and the stash.
fn closing_note<'a>(all: &'a ItemMap, task: &Item) -> Option<&'a Item> {
    let uid = task.uid.as_deref()?;
    all.values().filter(|note| !note.is_task && note.attached_to.as_deref() == Some(uid) && note.trashed.is_none() && note.stashed.is_none()).max_by_key(|note| (note.timestamp, note.id))
}

/// `HH:MM` of a time in epoch milliseconds, local, with the day before it
/// if that is not today.
fn clock(millis: i64) -> String {
    let Some(at) = chrono::Local.timestamp_millis_opt(millis).single() else { return String::new() };
    if at.date_naive() == chrono::Local::now().date_naive() {
        at.format("%H:%M").to_string()
    } else {
        at.format("%b %d %H:%M").to_string()
    }
}

/// The screen: a heading line, then each group with its rows, `selected`
/// marked among the rows of sessions that run, each line cut to `width`.
pub fn draw(rows: &[Row], selected: Option<usize>, width: usize, colour: bool, heading: &str) -> Vec<String> {
    let styled = |style: &str, text: String| if colour { format!("{style}{text}\x1b[0m") } else { text };
    let mut lines = vec![styled("\x1b[1m", heading.to_string()), String::new()];
    if rows.is_empty() {
        lines.push("No session ekko agents opened runs in the session ekko, and none finished today.".to_string());
    }
    // Where the tasks are of more than one project, each says whose.
    let several = rows.iter().map(|row| &row.project).collect::<BTreeSet<_>>().len() > 1;
    let ids = |row: &Row| {
        let ids = row.ids.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
        match (&row.project, several) {
            (Some(project), true) => format!("{project}:{ids}"),
            _ => ids,
        }
    };
    let id_width = rows.iter().map(|row| ids(row).chars().count()).max().unwrap_or(0);
    let title_width = rows.iter().map(|row| row.title.chars().count().min(TITLE)).max().unwrap_or(0);
    let how_width = rows.iter().map(|row| row.how.chars().count()).max().unwrap_or(0);
    let mut live = 0;
    let mut group = None;
    for row in rows {
        if group != Some(row.group) {
            if group.is_some() {
                lines.push(String::new());
            }
            group = Some(row.group);
            lines.push(styled(row.group.style(), format!("{} {}", row.group.symbol(), row.group.heading())));
        }
        let chosen = row.pane.is_some() && selected == Some(live);
        live += usize::from(row.pane.is_some());
        let since = row.since.as_ref().map(|at| format!("since {at}")).unwrap_or_default();
        let text = format!(
            "{} {:>id_width$}  {:<title_width$}  {:<how_width$}  {:<11}  {}",
            if chosen { '\u{276f}' } else { ' ' },
            ids(row),
            cut(&row.title, TITLE),
            row.how,
            since,
            row.detail.as_deref().unwrap_or_default(),
        );
        let text = cut(text.trim_end(), width);
        lines.push(if chosen { styled("\x1b[7m", text) } else { text });
    }
    lines
}

/// `text` cut to `width` characters, the last one an ellipsis where it ran
/// past.
fn cut(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let mut kept: String = text.chars().take(width.saturating_sub(1)).collect();
    kept.push('\u{2026}');
    kept
}

/// What a key asks of the view.
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    Stay,
    StepIn(String),
    Peek(String),
    Leave,
}

/// Where the user stands: which session that runs is chosen.
#[derive(Debug, Default)]
pub struct View {
    pub selected: usize,
}

impl View {
    pub fn press(&mut self, key: &Key, rows: &[Row]) -> Action {
        let panes: Vec<&String> = rows.iter().filter_map(|row| row.pane.as_ref()).collect();
        self.selected = self.selected.min(panes.len().saturating_sub(1));
        match key {
            Key::Up | Key::Char('k') => self.selected = self.selected.saturating_sub(1),
            Key::Down | Key::Char('j') => self.selected = (self.selected + 1).min(panes.len().saturating_sub(1)),
            Key::Enter => return panes.get(self.selected).map_or(Action::Stay, |pane| Action::StepIn(pane.to_string())),
            Key::Char(' ') => return panes.get(self.selected).map_or(Action::Stay, |pane| Action::Peek(pane.to_string())),
            Key::Char('q') | Key::Esc => return Action::Leave,
            _ => {}
        }
        Action::Stay
    }
}

/// The boards the view reads, opened once: the one it was run on, and every
/// other registered project's (task 1861), as one multiplexer session holds
/// the sessions of them all.
pub struct Opened<'a> {
    current: Option<String>,
    ekko: &'a Ekko,
    others: Vec<(String, Ekko)>,
}

impl<'a> Opened<'a> {
    pub fn new(ekko: &'a Ekko, home: &Path, current: Option<String>) -> Opened<'a> {
        let others = crate::project::registered(home)
            .into_iter()
            .filter(|name| Some(name) != current.as_ref())
            .filter_map(|name| {
                let location = crate::directory::locate(home, home, None, None, Some(&name)).ok()?;
                Some((name, Ekko::at(&location).ok()?))
            })
            .collect();
        Opened { current, ekko, others }
    }

    /// The boards the panes are of, and those a task finished today is on,
    /// as they are now: a board's file is read whole only to look for the
    /// uids `born.log` names, and parsed only where one is, so the view
    /// stays within a status line's time however many projects there are.
    /// An unreadable board is left out.
    fn boards(&self, panes: &[BornPane], today: &[String]) -> Boards {
        let names = logged(today);
        let wanted: BTreeSet<Option<String>> = panes.iter().filter(|pane| !pane.dead).map(|pane| pane.project.clone().or_else(|| self.current.clone())).collect();
        let current = std::iter::once((self.current.clone(), self.ekko));
        current
            .chain(self.others.iter().map(|(name, ekko)| (Some(name.clone()), ekko)))
            .filter(|(project, ekko)| reads(project, &self.current, &wanted, &names, || ekko.storage.text().ok()))
            .filter_map(|(project, ekko)| Some((project, ekko.storage.get_shared().ok()?)))
            .collect()
    }

    /// Where each board's file is, by its project.
    fn files(&self) -> Vec<(Option<String>, PathBuf)> {
        let current = std::iter::once((self.current.clone(), self.ekko));
        current.chain(self.others.iter().map(|(name, ekko)| (Some(name.clone()), ekko))).map(|(project, ekko)| (project, ekko.storage.storage_path().to_path_buf())).collect()
    }
}

/// The tasks today's `born.log` lines name, by uid or, in a log of before
/// the uids, by id.
fn logged(today: &[String]) -> BTreeSet<&str> {
    today.iter().filter_map(|line| line.split(' ').nth(2)).flat_map(|tasks| tasks.split(',')).filter(|name| !name.is_empty()).collect()
}

/// Whether the view parses the board of `project`: a pane is of it; it is
/// the current project's and a log names a task by id; or its file, which
/// `text` reads only when there is a task to look for, holds the uid of one.
fn reads(project: &Option<String>, current: &Option<String>, wanted: &BTreeSet<Option<String>>, names: &BTreeSet<&str>, text: impl FnOnce() -> Option<String>) -> bool {
    if wanted.contains(project) || (project == current && names.iter().any(|name| name.bytes().all(|byte| byte.is_ascii_digit()))) {
        return true;
    }
    !names.is_empty() && text().is_some_and(|board| names.iter().any(|name| board.contains(name)))
}

/// The Claude Code sessions on the boards that `ekko agents start` did not
/// open, which `ekko --sessions` lists as running (task 1861).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Others {
    pub total: usize,
    /// By the project whose board each started on; `(no project)` where the
    /// board is none of the registered ones.
    pub by_project: BTreeMap<String, usize>,
}

/// Counted from the processes ekko recorded: each that runs and started on
/// a board, but for one whose conversation Claude Code moved to another
/// process, and `born` ones.
pub fn others(recorded: &[crate::holder::Running], born: impl Fn(&crate::holder::Running) -> bool, files: &[(Option<String>, PathBuf)]) -> Others {
    let moved = crate::agent::moved_out(recorded);
    let mut found = Others::default();
    for running in recorded.iter().filter(|running| running.process().alive() && running.board.is_some()) {
        if moved.iter().any(|(from, _)| *from == running.process()) || born(running) {
            continue;
        }
        let project = files.iter().find(|(_, file)| Some(file) == running.board.as_ref()).and_then(|(project, _)| project.clone());
        *found.by_project.entry(project.unwrap_or_else(|| "(no project)".to_string())).or_default() += 1;
        found.total += 1;
    }
    found
}

/// What the view shows now, read from the multiplexer, the boards and
/// `born.log`; what the multiplexer said, if it could not list the panes.
fn read(mux: &Mux, opened: &Opened, home: &Path) -> (Vec<Row>, Option<String>) {
    let (panes, said) = match mux.born_panes() {
        Ok(panes) => (panes, None),
        Err(why) => (Vec::new(), Some(why)),
    };
    let today = agents::born_today(home);
    (rows(&panes, &opened.boards(&panes, &today), &opened.current, &today), said)
}

/// `ekko agents view --json`: every session that runs in any project, and
/// each task finished today, each with its project, tasks, model, effort,
/// state -- `working`, `waiting` on the user, `idle` or `done` -- since when
/// in seconds since the epoch, and pane; then the count of each state, and
/// how many other Claude Code sessions run on the boards (task 1861).
pub fn json(rows: &[Row], others: &Others, said: Option<&str>) -> serde_json::Value {
    let sessions: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| {
            let mut session = serde_json::json!({
                "project": row.project,
                "tasks": row.ids,
                "title": row.title,
                "model": row.model,
                "effort": row.effort,
                "state": row.group.word(),
                "since": row.at,
                "pane": row.pane,
                "window": row.window,
            });
            match row.group {
                Group::Waiting => session["waitingOn"] = serde_json::json!(row.detail),
                Group::Finished => {
                    session["ended"] = serde_json::json!(row.ended);
                    session["closingNote"] = serde_json::json!(row.detail);
                }
                Group::Working | Group::Idle => {}
            }
            session
        })
        .collect();
    let count = |group: Group| rows.iter().filter(|row| row.group == group).count();
    let mut out = serde_json::json!({
        "ok": true,
        "command": "agents view",
        "session": agents::SESSION,
        "sessions": sessions,
        "counts": {
            "working": count(Group::Working),
            "waiting": count(Group::Waiting),
            "idle": count(Group::Idle),
            "done": count(Group::Finished),
        },
        "others": { "total": others.total, "byProject": others.by_project },
    });
    if let Some(said) = said {
        out["unread"] = serde_json::json!(said);
    }
    out
}

/// The heading line: where the sessions are, and how many.
fn heading(mux: &Mux, rows: &[Row]) -> String {
    let server: Vec<String> = mux.argv().iter().map(|arg| arg.to_string_lossy().into_owned()).collect();
    let running = rows.iter().filter(|row| row.pane.is_some()).count();
    let finished = rows.len() - running;
    format!("ekko agents \u{b7} {} session {} \u{b7} {running} running, {finished} finished today", server.join(" "), agents::SESSION)
}

/// `ekko agents view`, on the board of `ekko`, the project `current`'s; the
/// sessions of every project are shown.
pub fn run(ekko: &Ekko, home: &Path, current: Option<String>, once: bool, as_json: bool) -> Result<(), EkkoError> {
    let mux = Mux::here();
    let mut out = io::stdout();
    let opened = Opened::new(ekko, home, current);
    if as_json {
        let (rows, said) = read(&mux, &opened, home);
        let recorded = crate::holder::Registry::at(crate::agent::processes_dir(home)).running();
        let others = others(&recorded, |running| agents::is_born(home, running), &opened.files());
        let _ = writeln!(out, "{}", json(&rows, &others, said.as_deref()));
        return Ok(());
    }
    if once || !io::stdin().is_terminal() || !out.is_terminal() {
        let (rows, said) = read(&mux, &opened, home);
        let colour = out.is_terminal() && std::env::var_os("NO_COLOR").is_none();
        for line in draw(&rows, None, usize::MAX, colour, &heading(&mux, &rows)) {
            let _ = writeln!(out, "{line}");
        }
        if let Some(said) = said {
            let _ = writeln!(out, "\n{said}");
        }
        return Ok(());
    }
    let mut view = View::default();
    let stepping = {
        let _raw = menu::Raw::enter()?;
        let _ = write!(out, "\x1b[?1049h\x1b[?25l");
        let mut said: Option<String> = None;
        let stepping = loop {
            let (rows, unread) = read(&mux, &opened, home);
            let mut lines = draw(&rows, Some(view.selected), menu::columns(), true, &heading(&mux, &rows));
            lines.push(String::new());
            lines.extend(said.iter().chain(unread.iter()).cloned());
            lines.push("\x1b[2m\u{2191}\u{2193} choose \u{b7} Enter step in \u{b7} Space peek \u{b7} q leave\x1b[0m".to_string());
            paint(&mut out, &lines);
            if !key_within(EVERY) {
                continue;
            }
            let Some(key) = menu::read_key(&mut menu::Keys, menu::follows) else { break None };
            said = None;
            match view.press(&key, &rows) {
                Action::Stay => {}
                Action::Leave => break None,
                Action::Peek(pane) => {
                    peek(&mux, &mut out, &pane);
                    if menu::read_key(&mut menu::Keys, menu::follows).is_none() {
                        break None;
                    }
                }
                // Inside the multiplexer the view's own client moves there,
                // and the view ends, as a popup over it must.
                Action::StepIn(pane) if std::env::var_os("TMUX").is_some() => match mux.run(&["switch-client", "-t", &pane]) {
                    Ok(output) if output.status.success() => break None,
                    Ok(output) => said = Some(mux.failed("switch-client", &output)),
                    Err(why) => said = Some(why),
                },
                Action::StepIn(pane) => break Some(pane),
            }
        };
        let _ = write!(out, "\x1b[?25h\x1b[?1049l");
        let _ = out.flush();
        stepping
    };
    // Outside the multiplexer, the view becomes a client of it, attached
    // to that session's window.
    if let Some(pane) = stepping {
        use std::os::unix::process::CommandExt as _;
        let argv = mux.argv();
        let error = std::process::Command::new(&argv[0]).args(&argv[1..]).args(["attach-session", "-t", &pane]).exec();
        return Err(EkkoError::InvalidInput(format!("{} could not be run: {error}", argv[0].to_string_lossy())));
    }
    Ok(())
}

/// Draws `lines` from the top of the screen, each erasing what was left of
/// its line, and the rest of the screen below them.
fn paint(out: &mut impl io::Write, lines: &[String]) {
    let mut screen = String::from("\x1b[H");
    for line in lines {
        screen.push_str(line);
        screen.push_str("\x1b[K\r\n");
    }
    screen.push_str("\x1b[J");
    let _ = out.write_all(screen.as_bytes());
    let _ = out.flush();
}

/// The screen of `pane` as its multiplexer holds it, colours and all, under
/// a line saying whose it is.
fn peek(mux: &Mux, out: &mut impl io::Write, pane: &str) {
    let screen = match mux.run(&["capture-pane", "-p", "-e", "-t", pane]) {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout).into_owned(),
        Ok(output) => mux.failed("capture-pane", &output),
        Err(why) => why,
    };
    let mut lines = vec![format!("\x1b[1mPeek at {pane}\x1b[0m \u{b7} \x1b[2many key returns\x1b[0m")];
    lines.extend(screen.trim_end().lines().map(|line| format!("{line}\x1b[0m")));
    paint(out, &lines);
}

/// Whether a key waits to be read within `limit`; not when a signal cut
/// the wait short, which redraws the screen -- a resize -- unless it was
/// SIGTERM or SIGHUP, which the next read reads as the end.
fn key_within(limit: Duration) -> bool {
    let mut poll = libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 };
    let millis = libc::c_int::try_from(limit.as_millis()).unwrap_or(libc::c_int::MAX);
    // SAFETY: one valid pollfd, for the length given.
    let ready = unsafe { libc::poll(&mut poll, 1, millis) };
    ready > 0 || menu::stopped()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(id: &str, tasks: &[u32], state: &str, waits: Option<&str>) -> BornPane {
        BornPane {
            pane: id.to_string(),
            dead: false,
            window_id: format!("@{}", tasks[0]),
            window: format!("{} \u{b7} haiku", tasks[0]),
            tasks: tasks.to_vec(),
            title: "from the pane".to_string(),
            model: "haiku".to_string(),
            effort: Some("low".to_string()),
            state: state.to_string(),
            since: Some(chrono::Local::now().timestamp()),
            waits: waits.map(str::to_string),
            project: Some("app".to_string()),
        }
    }

    fn board() -> ItemMap {
        let mut all = ItemMap::new();
        let mut add = |id: u32, text: &str, task: bool, edit: &dyn Fn(&mut Item)| {
            let mut item: Item = serde_json::from_value(serde_json::json!({
                "_id": id, "_date": "Sat Oct 10 2026", "_timestamp": i64::from(id), "description": text,
                "isStarred": false, "boards": ["My Board"], "_isTask": task, "uid": format!("u{id}"),
            }))
            .unwrap();
            edit(&mut item);
            all.insert(id, item);
        };
        let now = chrono::Local::now().timestamp_millis();
        add(1, "Fix the parser\nwhere it reads dates", true, &|item| item.in_progress = Some(true));
        add(2, "Write the docs", true, &|item| item.in_progress = Some(true));
        add(3, "Second of two", true, &|_| {});
        add(4, "Review the plan", true, &|item| item.is_complete = Some(true));
        add(5, "Add the flag", true, &|item| {
            item.is_complete = Some(true);
            item.updated_at = Some(now);
        });
        add(6, "An older note", false, &|item| item.attached_to = Some("u5".into()));
        add(7, "Added --json to agents start\ntests pass", false, &|item| item.attached_to = Some("u5".into()));
        add(8, "Dropped", true, &|item| item.cancelled = Some(true));
        add(9, "Still pending", true, &|_| {});
        all
    }

    /// Sessions by their pane's state, a session's tasks titled from the
    /// board, and today's log naming tasks finished -- by uid, as
    /// EKKO_AGENT_TASK names them -- each once, with its newest note; a task
    /// still open, or one a running session holds though it is done, is no
    /// finished row, and a dead pane no session.
    #[test]
    fn the_rows_group_sessions_by_state_and_list_what_finished_today() {
        let mut dead = pane("%5", &[9], "working", None);
        dead.dead = true;
        let panes = [pane("%2", &[2, 3], "working", None), pane("%1", &[1], "waiting", Some("Claude needs your permission -- Bash: date")), pane("%4", &[4], "idle", None), dead];
        let today = [
            "2026-10-10T01:00:00.000 aaaaaaaa u5 its tasks are finished: closing",
            "2026-10-10T01:00:01.000 aaaaaaaa u5 closed: its Claude Code ended",
            "2026-10-10T01:00:02.000 bbbbbbbb u8 closed: its Claude Code ended",
            "2026-10-10T01:00:03.000 cccccccc u9 stays: task 9 is pending",
            "2026-10-10T01:00:04.000 dddddddd u1 stays: task 1 is in progress",
            "2026-10-10T01:00:05.000 eeeeeeee u4 stays: a prompt reached it after its tasks were finished",
        ]
        .map(str::to_string);
        let rows = rows(&panes, &Boards::from([(Some("app".to_string()), Arc::new(board()))]), &Some("app".to_string()), &today);
        type Shown<'a> = (Group, Option<&'a str>, Vec<u32>, &'a str, &'a str, Option<&'a str>);
        let shown: Vec<Shown> =
            rows.iter().map(|row| (row.group, row.pane.as_deref(), row.ids.clone(), row.title.as_str(), row.how.as_str(), row.detail.as_deref())).collect();
        let done = rows[3].how.clone();
        assert!(done.starts_with("done ") && rows[4].how == "cancelled", "{rows:?}");
        assert_eq!(
            shown,
            [
                (Group::Waiting, Some("%1"), vec![1], "Fix the parser", "haiku \u{b7} low", Some("Claude needs your permission -- Bash: date")),
                (Group::Working, Some("%2"), vec![2, 3], "Write the docs (+1)", "haiku \u{b7} low", None),
                (Group::Idle, Some("%4"), vec![4], "Review the plan", "haiku \u{b7} low", None),
                (Group::Finished, None, vec![5], "Add the flag", done.as_str(), Some("Added --json to agents start")),
                (Group::Finished, None, vec![8], "Dropped", "cancelled", None),
            ]
        );
    }

    /// The screen: the groups in order under their symbols, the chosen
    /// session marked, columns aligned, every line cut to the width; no
    /// colour unless asked for.
    #[test]
    fn the_screen_draws_each_group_under_its_symbol_with_the_chosen_session_marked() {
        let row = |group, pane: Option<&str>, id, title: &str, detail: Option<&str>| Row {
            group,
            pane: pane.map(str::to_string),
            ids: vec![id],
            title: title.to_string(),
            how: "haiku".to_string(),
            since: pane.map(|_| "02:31".to_string()),
            detail: detail.map(str::to_string),
            ..Row::default()
        };
        let rows = [
            row(Group::Waiting, Some("%1"), 12, "Fix the parser", Some("Claude needs your permission")),
            row(Group::Working, Some("%2"), 3, "Write the docs", None),
            row(Group::Finished, None, 11, "Add the flag", Some("Added --json")),
        ];
        let lines = draw(&rows, Some(1), 200, false, "ekko agents");
        assert_eq!(
            lines,
            [
                "ekko agents",
                "",
                "\u{25cf} Waiting on you",
                "  12  Fix the parser  haiku  since 02:31  Claude needs your permission",
                "",
                "\u{2699} Working",
                "\u{276f}  3  Write the docs  haiku  since 02:31",
                "",
                "\u{2713} Finished today",
                "  11  Add the flag    haiku               Added --json",
            ]
        );
        assert!(draw(&rows, Some(0), 20, false, "h").iter().all(|line| line.chars().count() <= 20));
        assert_eq!(draw(&rows, Some(0), 20, false, "h")[3], "\u{276f} 12  Fix the parse\u{2026}");
        let coloured = draw(&rows, Some(0), 200, true, "h");
        assert!(coloured[2].starts_with("\x1b[1;30;43m\u{25cf} Waiting") && coloured[3].starts_with("\x1b[7m\u{276f}"), "{coloured:?}");
        assert_eq!(draw(&[], None, 200, false, "h")[2], "No session ekko agents opened runs in the session ekko, and none finished today.");
    }


    /// A board of `tasks`, each `(id, text, uid, done)`, as the view reads it.
    fn lone(tasks: &[(u32, &str, &str, bool)]) -> Arc<ItemMap> {
        let now = chrono::Local::now().timestamp_millis();
        Arc::new(
            tasks
                .iter()
                .map(|(id, text, uid, done)| {
                    let item: Item = serde_json::from_value(serde_json::json!({
                        "_id": id, "_date": "Sat Oct 10 2026", "_timestamp": i64::from(*id), "description": text,
                        "isStarred": false, "boards": ["My Board"], "_isTask": true, "uid": uid,
                        "isComplete": done, "inProgress": !done, "updatedAt": now,
                    }))
                    .unwrap();
                    (*id, item)
                })
                .collect(),
        )
    }

    fn named(name: &str) -> Option<String> {
        Some(name.to_string())
    }

    /// One multiplexer session holds every project's sessions (task 1861):
    /// a pane is read on the board of its project, however the same id is
    /// numbered on another's, one that names none on the current project's;
    /// a task finished today is found by its uid on whichever board holds
    /// it, and the same id on two boards is two rows.
    #[test]
    fn sessions_of_two_projects_are_each_read_on_their_own_board() {
        let boards = Boards::from([
            (named("app"), lone(&[(1, "Fix the parser", "a1", false)])),
            (named("lib"), lone(&[(1, "Cache the index", "l1", false), (2, "Ship the flag", "l2", true)])),
        ]);
        let mut in_lib = pane("%2", &[1], "waiting", Some("a permission"));
        in_lib.project = named("lib");
        let mut unnamed = pane("%3", &[1], "idle", None);
        unnamed.project = None;
        let panes = [pane("%1", &[1], "working", None), in_lib, unnamed];
        let today = ["2026-10-10T01:00:00.000 aaaaaaaa l2 closed: its Claude Code ended".to_string()];
        let rows = rows(&panes, &boards, &named("app"), &today);
        let shown: Vec<_> = rows.iter().map(|row| (row.group, row.project.as_deref(), row.ids.clone(), row.title.as_str())).collect();
        assert_eq!(
            shown,
            [
                (Group::Waiting, Some("lib"), vec![1], "Cache the index"),
                (Group::Working, Some("app"), vec![1], "Fix the parser"),
                (Group::Idle, Some("app"), vec![1], "Fix the parser"),
                (Group::Finished, Some("lib"), vec![2], "Ship the flag"),
            ]
        );
        let lines = draw(&rows, None, 200, false, "h");
        assert!(lines[3].contains("lib:1") && lines[6].contains("app:1") && lines[12].contains("lib:2"), "{lines:?}");
        assert!(draw(&rows[1..2], None, 200, false, "h")[3].starts_with("  1 "), "one project's tasks carry no project");
    }

    /// `--json`: per session its project, tasks, model, effort, state,
    /// since, pane; per finished task how it ended and its closing note;
    /// the count of each state and of the other sessions.
    #[test]
    fn the_json_says_each_session_s_project_tasks_model_effort_state_since_and_pane() {
        let boards = Boards::from([(named("app"), lone(&[(1, "Fix the parser", "a1", false), (2, "Ship the flag", "a2", true)]))]);
        let mut waiting = pane("%1", &[1], "waiting", Some("a permission"));
        waiting.since = Some(1_700_000_000);
        let today = ["2026-10-10T01:00:00.000 aaaaaaaa a2 closed: its Claude Code ended".to_string()];
        let rows = rows(&[waiting, pane("%2", &[1], "working", None)], &boards, &named("app"), &today);
        let others = Others { total: 3, by_project: BTreeMap::from([("app".to_string(), 2), ("lib".to_string(), 1)]) };
        let said = json(&rows, &others, Some("no server"));
        let session = &said["sessions"][0];
        assert_eq!(session["project"], "app");
        assert_eq!(session["tasks"], serde_json::json!([1]));
        assert_eq!((session["model"].as_str(), session["effort"].as_str()), (Some("haiku"), Some("low")));
        assert_eq!((session["state"].as_str(), session["since"].as_i64(), session["pane"].as_str()), (Some("waiting"), Some(1_700_000_000), Some("%1")));
        assert_eq!(session["waitingOn"], "a permission");
        assert_eq!(said["sessions"][1]["state"], "working");
        let done = &said["sessions"][2];
        assert_eq!((done["state"].as_str(), done["ended"].as_str(), done["pane"].is_null()), (Some("done"), Some("done"), true));
        assert!(done["since"].as_i64().is_some_and(|at| at > 1_700_000_000), "{done}");
        assert_eq!(said["counts"], serde_json::json!({"working": 1, "waiting": 1, "idle": 0, "done": 1}));
        assert_eq!(said["others"], serde_json::json!({"total": 3, "byProject": {"app": 2, "lib": 1}}));
        assert_eq!(said["unread"], "no server");
        assert!(json(&rows, &others, None).get("unread").is_none());
    }

    /// Which boards the view parses: a pane's, or one whose file holds a
    /// uid `born.log` names; a file is not read when nothing is looked for.
    #[test]
    fn a_board_is_parsed_only_where_a_pane_or_a_finished_task_is() {
        let wanted = BTreeSet::from([named("app")]);
        let lines = ["2026-10-10T01:00:00.000 aaaaaaaa u5,u6 closed".to_string()];
        let names = logged(&lines);
        assert_eq!(names, BTreeSet::from(["u5", "u6"]));
        let never = || -> Option<String> { panic!("read a file nothing looked for") };
        assert!(reads(&named("app"), &named("app"), &wanted, &BTreeSet::new(), never));
        assert!(!reads(&named("lib"), &named("app"), &wanted, &BTreeSet::new(), never));
        assert!(reads(&named("lib"), &named("app"), &wanted, &names, || Some("{\"uid\": \"u6\"}".to_string())));
        assert!(!reads(&named("lib"), &named("app"), &wanted, &names, || Some("{\"uid\": \"u7\"}".to_string())));
        assert!(!reads(&named("lib"), &named("app"), &wanted, &names, || None));
        let by_id = ["x y 12 closed".to_string()];
        assert!(reads(&named("lib"), &named("lib"), &BTreeSet::new(), &logged(&by_id), || None), "a log of ids is the current project's");
        assert!(!reads(&named("lib"), &named("app"), &BTreeSet::new(), &logged(&by_id), || None));
    }

    /// The other sessions: those that run and started on a board, but not
    /// the ones ekko agents started, those that ended, or one whose
    /// conversation went on in another process; by the project of the board.
    #[test]
    fn the_other_sessions_leave_out_the_born_the_ended_and_those_without_a_board() {
        use crate::holder::{Process, Running};
        let app = PathBuf::from("/app/.ekko/storage/storage.json");
        let files = [(named("app"), app.clone()), (named("lib"), PathBuf::from("/lib/.ekko/storage/storage.json"))];
        let running = |process: Process, conversation: &str, board: Option<&PathBuf>| Running {
            pid: process.pid,
            start: process.start,
            boot: process.boot,
            profile: None,
            tty: None,
            config_dir: None,
            board: board.cloned(),
            transcript: None,
            conversation: conversation.to_string(),
            earlier: Vec::new(),
            since: 0,
        };
        let me = Process::of(std::process::id()).unwrap();
        let parent = Process::of(std::os::unix::process::parent_id()).unwrap();
        let init = Process::of(1).unwrap();
        let recorded = [
            running(me.clone(), "mine", Some(&app)),
            running(parent, "elsewhere", Some(&PathBuf::from("/scratch/storage.json"))),
            running(init, "born", Some(&app)),
            running(Process { start: me.start + 1, ..me.clone() }, "ended", Some(&app)),
            running(Process { pid: me.pid + 100_000_000, ..me }, "no process", Some(&app)),
        ];
        let found = others(&recorded, |running| running.conversation == "born", &files);
        assert_eq!(found, Others { total: 2, by_project: BTreeMap::from([("app".to_string(), 1), ("(no project)".to_string(), 1)]) });
        let nowhere = recorded.iter().map(|running| Running { board: None, ..running.clone() }).collect::<Vec<_>>();
        assert_eq!(others(&nowhere, |_| false, &files), Others::default(), "a session on no board is on no board's list");
    }
    /// The arrows move among the sessions that run, Enter and Space act on
    /// the chosen one, q and Esc leave; a finished task is never chosen.
    #[test]
    fn keys_choose_a_running_session_step_into_it_peek_or_leave() {
        let row = |pane: Option<&str>| Row { pane: pane.map(str::to_string), ids: vec![1], ..Row::default() };
        let rows = [row(Some("%1")), row(None), row(Some("%3"))];
        let mut view = View::default();
        assert_eq!(view.press(&Key::Up, &rows), Action::Stay);
        assert_eq!(view.press(&Key::Enter, &rows), Action::StepIn("%1".into()));
        view.press(&Key::Down, &rows);
        view.press(&Key::Char('j'), &rows);
        assert_eq!(view.press(&Key::Char(' '), &rows), Action::Peek("%3".into()));
        view.press(&Key::Char('k'), &rows);
        assert_eq!(view.press(&Key::Enter, &rows), Action::StepIn("%1".into()));
        assert_eq!(view.press(&Key::Char('q'), &rows), Action::Leave);
        assert_eq!(view.press(&Key::Esc, &rows), Action::Leave);
        assert_eq!(View::default().press(&Key::Enter, &rows[1..2]), Action::Stay);
    }
}
