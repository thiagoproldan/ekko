//! `ekko agents view` (task 1676): on one screen, the sessions `ekko agents
//! start` opened in the multiplexer's session `ekko`, grouped by the state
//! their hooks keep on their panes (task 1675) -- waiting on you, working,
//! idle -- and the tasks born sessions finished today, each with the first
//! line of its closing note. In a terminal, outside the multiplexer or in a
//! popup of it, the arrows choose a session, Enter steps into its window,
//! Space peeks at its screen and q leaves; the panes and the board are read
//! again every second. Printed once with --once, or with no terminal.

use std::collections::BTreeSet;
use std::io::{self, IsTerminal as _, Write as _};
use std::path::Path;
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Group {
    Waiting,
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub group: Group,
    /// The session's pane; none for a finished task.
    pub pane: Option<String>,
    pub ids: Vec<u32>,
    pub title: String,
    /// Its model and effort; for a finished task, how and when it ended.
    pub how: String,
    /// Since when it is in its state, `HH:MM`.
    pub since: Option<String>,
    /// What it waits on; for a finished task, its closing note's first line.
    pub detail: Option<String>,
}

/// The rows of the view: each pane `start` opened that still runs, by its
/// state, then each task named in today's `born.log` that is now done or
/// cancelled and has no such pane, with the newest note attached to it.
pub fn rows(panes: &[BornPane], all: &ItemMap, today: &[String]) -> Vec<Row> {
    let index = crate::ekko::uid_index(all);
    let mut rows = Vec::new();
    let mut running = BTreeSet::new();
    for pane in panes.iter().filter(|pane| !pane.dead) {
        running.extend(pane.tasks.iter().copied());
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
        rows.push(Row { group, pane: Some(pane.pane.clone()), ids: pane.tasks.clone(), title, how, since: pane.since.map(|at| clock(at * 1000)), detail });
    }
    let mut listed = BTreeSet::new();
    for line in today {
        let Some(tasks) = line.split(' ').nth(2) else { continue };
        for name in tasks.split(',').filter(|name| !name.is_empty()) {
            let Some(task) = index.get(name).copied().or_else(|| name.parse().ok()).and_then(|id| all.get(&id)) else { continue };
            if running.contains(&task.id) || !listed.insert(task.id) {
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
            rows.push(Row { group: Group::Finished, pane: None, ids: vec![task.id], title: crate::ekko::title(&task.description).to_string(), how, since: None, detail });
        }
    }
    rows.sort_by_key(|row| (row.group, row.ids.first().copied()));
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
    let ids = |row: &Row| row.ids.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
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

/// What the view shows now, read from the multiplexer, the board and
/// `born.log`; what the multiplexer said, if it could not list the panes.
fn read(mux: &Mux, ekko: &Ekko, home: &Path) -> (Vec<Row>, Option<String>) {
    let (panes, said) = match mux.born_panes() {
        Ok(panes) => (panes, None),
        Err(why) => (Vec::new(), Some(why)),
    };
    let all = ekko.storage.get_shared().map(|all| ItemMap::clone(&all)).unwrap_or_default();
    (rows(&panes, &all, &agents::born_today(home)), said)
}

/// The heading line: where the sessions are, and how many.
fn heading(mux: &Mux, rows: &[Row]) -> String {
    let server: Vec<String> = mux.argv().iter().map(|arg| arg.to_string_lossy().into_owned()).collect();
    let running = rows.iter().filter(|row| row.pane.is_some()).count();
    let finished = rows.len() - running;
    format!("ekko agents \u{b7} {} session {} \u{b7} {running} running, {finished} finished today", server.join(" "), agents::SESSION)
}

/// `ekko agents view`.
pub fn run(ekko: &Ekko, home: &Path, once: bool) -> Result<(), EkkoError> {
    let mux = Mux::here();
    let mut out = io::stdout();
    if once || !io::stdin().is_terminal() || !out.is_terminal() {
        let (rows, said) = read(&mux, ekko, home);
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
            let (rows, unread) = read(&mux, ekko, home);
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
        let rows = rows(&panes, &board(), &today);
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

    /// The arrows move among the sessions that run, Enter and Space act on
    /// the chosen one, q and Esc leave; a finished task is never chosen.
    #[test]
    fn keys_choose_a_running_session_step_into_it_peek_or_leave() {
        let row = |pane: Option<&str>| Row { group: Group::Working, pane: pane.map(str::to_string), ids: vec![1], title: String::new(), how: String::new(), since: None, detail: None };
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
