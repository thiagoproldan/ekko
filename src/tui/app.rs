//! The interactive mode's state, and what each key and click does to it.
//!
//! Nothing here draws or touches the terminal, so every rule -- which key
//! moves, which one writes, what a refusal says -- is tested without one.
//! Moving and writing are kept apart by type: a key or a click changes what is
//! selected, shown or focused, and only an `Action` handed back by `key`
//! writes, through `act`.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

use super::board::{Row, Snapshot};
use super::layout::{Region, Visibility};
use crate::ekko::{Ekko, EkkoError};
use crate::item::{Item, State};

/// How long a message stays in the status bar.
const FLASH: Duration = Duration::from_secs(6);
/// How long the live indicator stays lit after the board changed elsewhere.
const PULSE: Duration = Duration::from_secs(2);
/// Lines Output keeps.
const LOG_LINES: usize = 500;

/// Where the interactive mode was opened.
pub struct Workspace {
    /// How `prime` names the board, as the CLI does.
    pub label: String,
    /// The project's name, or what the board is when it is not a project's.
    pub name: String,
    /// The folder, with home shortened to `~`.
    pub folder: String,
    pub cwd: PathBuf,
    pub branch: Option<String>,
}

/// The bottom panel's tabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Problems,
    Output,
    Agent,
}

impl Tab {
    pub const ALL: [Tab; 3] = [Tab::Problems, Tab::Output, Tab::Agent];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Problems => "Problems",
            Tab::Output => "Output",
            Tab::Agent => "Agent",
        }
    }
}

/// A box that can be closed and opened again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Next,
    Explorer,
    Panel,
}

/// What a click can land on, recorded as each thing is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Region(Region),
    Command,
    Toggle(Part),
    BoardRow(usize),
    NextRow(usize),
    ExplorerRow(usize),
    ProblemRow(usize),
    PanelTab(Tab),
    Bell,
}

/// The writes a key can ask for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Done,
    Progress,
    Stash,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Did,
    Refused,
    Outside,
}

pub struct Logged {
    pub at: String,
    pub text: String,
    pub kind: Kind,
}

/// A selection in a list, and the first line of the list on screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cursor {
    pub selected: usize,
    pub offset: usize,
}

impl Cursor {
    /// Pulls the viewport to the selection by the least scrolling that does it.
    pub fn follow(&mut self, height: usize) {
        if height == 0 {
            return;
        }
        if self.selected < self.offset {
            self.offset = self.selected;
        } else if self.selected >= self.offset + height {
            self.offset = self.selected + 1 - height;
        }
    }

    fn step(&mut self, by: isize, len: usize) {
        if len > 0 {
            self.selected = (self.selected as isize + by).clamp(0, len as isize - 1) as usize;
        }
    }

    fn clamp(&mut self, len: usize) {
        self.selected = self.selected.min(len.saturating_sub(1));
    }
}

pub struct App {
    pub workspace: Workspace,
    pub snapshot: Snapshot,
    pub rows: Vec<Row>,
    pub filter: String,
    pub focus: Region,
    /// The boxes the person has open.
    pub wanted: Visibility,
    /// The boxes the last frame drew: `wanted`, less what the terminal had no
    /// room for.
    pub shown: Visibility,
    pub tab: Tab,
    pub board: Cursor,
    pub next: Cursor,
    pub explorer: Cursor,
    pub problems: Cursor,
    /// Lines scrolled past in Output or Agent.
    pub scroll: usize,
    pub log: Vec<Logged>,
    flash: Option<(String, Kind, Instant)>,
    /// Changes made elsewhere since Output was last in view.
    pub unseen: usize,
    pulse: Option<Instant>,
    pub hits: Vec<(Rect, Target)>,
    /// Rows the board list had room for in the last frame, for Page Up/Down.
    pub page: usize,
    pub quit: bool,
}

/// What names an item across reloads: its uid, which survives renumbering.
fn key_of(item: &Item) -> String {
    item.uid.clone().unwrap_or_else(|| format!("#{}", item.id))
}

impl App {
    pub fn new(workspace: Workspace, snapshot: Snapshot) -> Self {
        let mut app = App {
            workspace,
            snapshot,
            rows: Vec::new(),
            filter: String::new(),
            focus: Region::Editor,
            wanted: Visibility::default(),
            shown: Visibility::default(),
            tab: Tab::Problems,
            board: Cursor::default(),
            next: Cursor::default(),
            explorer: Cursor::default(),
            problems: Cursor::default(),
            scroll: 0,
            log: Vec::new(),
            flash: None,
            unseen: 0,
            pulse: None,
            hits: Vec::new(),
            page: 10,
            quit: false,
        };
        app.restore(None, true);
        app
    }

    /// The selected item, by its index in `snapshot.items`.
    pub fn selected(&self) -> Option<usize> {
        match self.rows.get(self.board.selected) {
            Some(Row::Item { item, .. }) => Some(*item),
            _ => None,
        }
    }

    pub fn flash(&self) -> Option<(&str, Kind)> {
        self.flash
            .as_ref()
            .filter(|(_, _, at)| at.elapsed() < FLASH)
            .map(|(text, kind, _)| (text.as_str(), *kind))
    }

    /// Whether the board changed elsewhere a moment ago.
    pub fn pulsing(&self) -> bool {
        self.pulse.is_some_and(|at| at.elapsed() < PULSE)
    }

    // ---- reading the board ----------------------------------------------

    /// Reads the board again, keeping the same item selected.
    pub fn reload(&mut self, ekko: &Ekko) -> Result<(), EkkoError> {
        let kept = self.kept();
        self.snapshot = Snapshot::load(ekko, &self.workspace.label)?;
        self.restore(kept, false);
        Ok(())
    }

    /// The board was written by someone else: another terminal, or an agent.
    pub fn outside_change(&mut self, ekko: &Ekko) -> Result<(), EkkoError> {
        self.reload(ekko)?;
        self.log("The board changed elsewhere, and is shown as it is now", Kind::Outside);
        if !(self.shown.panel && self.tab == Tab::Output) {
            self.unseen += 1;
        }
        self.pulse = Some(Instant::now());
        Ok(())
    }

    /// The selected item's board and key, to find it again after the rows change.
    fn kept(&self) -> Option<(String, String)> {
        let Some(Row::Item { group, item }) = self.rows.get(self.board.selected) else { return None };
        Some((self.snapshot.groups[*group].name.clone(), key_of(&self.snapshot.items[*item])))
    }

    /// Rebuilds the rows and selects `kept` again: under the same board when
    /// it is still there, under another when it moved, and otherwise the
    /// first item (`first`) or the nearest one to where the selection was.
    fn restore(&mut self, kept: Option<(String, String)>, first: bool) {
        self.rows = self.snapshot.rows(&self.filter);
        let found = kept.and_then(|(board, key)| {
            let at = |same_board: bool| {
                self.rows.iter().position(|row| match row {
                    Row::Item { group, item } => {
                        key_of(&self.snapshot.items[*item]) == key
                            && (!same_board || self.snapshot.groups[*group].name == board)
                    }
                    Row::Board(_) => false,
                })
            };
            at(true).or_else(|| at(false))
        });
        let from = if first { 0 } else { self.board.selected.min(self.rows.len().saturating_sub(1)) };
        self.board.selected = found
            .or_else(|| self.item_row(from, 1))
            .or_else(|| self.item_row(from, -1))
            .unwrap_or(0);

        self.next.clamp(self.snapshot.next().len());
        self.explorer.clamp(self.snapshot.groups.len());
        self.problems.clamp(self.snapshot.problems.len());
    }

    /// The nearest item row from `at` in direction `by`, `at` included.
    fn item_row(&self, at: usize, by: isize) -> Option<usize> {
        let mut row = at as isize;
        while row >= 0 && (row as usize) < self.rows.len() {
            if matches!(self.rows[row as usize], Row::Item { .. }) {
                return Some(row as usize);
            }
            row += by;
        }
        None
    }

    fn set_filter(&mut self, filter: String) {
        let kept = self.kept();
        self.filter = filter;
        self.restore(kept, true);
    }

    // ---- moving -----------------------------------------------------------

    /// Moves the board selection by `by` rows, stepping over headings and
    /// stopping at either end.
    fn move_board(&mut self, by: isize) {
        if self.rows.is_empty() {
            return;
        }
        let target = (self.board.selected as isize + by).clamp(0, self.rows.len() as isize - 1) as usize;
        let ahead = if by < 0 { -1 } else { 1 };
        if let Some(row) = self.item_row(target, ahead).or_else(|| self.item_row(target, -ahead)) {
            self.board.selected = row;
        }
    }

    /// Selects `id` in the board list, clearing a filter that hides it.
    fn reveal(&mut self, id: u32) {
        let position = |app: &App| {
            app.rows.iter().position(|row| matches!(row, Row::Item { item, .. } if app.snapshot.items[*item].id == id))
        };
        if position(self).is_none() && !self.filter.is_empty() {
            self.filter.clear();
            self.rows = self.snapshot.rows("");
        }
        if let Some(row) = position(self) {
            self.board.selected = row;
        }
    }

    /// Brings a board's heading to the top of the list, and selects its first item.
    fn reveal_board(&mut self, group: usize) {
        let position = |app: &App| {
            app.rows.iter().position(|row| matches!(row, Row::Item { group: g, .. } if *g == group))
        };
        if position(self).is_none() && !self.filter.is_empty() {
            self.filter.clear();
            self.rows = self.snapshot.rows("");
        }
        if let Some(row) = position(self) {
            self.board.selected = row;
            self.board.offset = row.saturating_sub(1);
        }
    }

    fn toggle(&mut self, part: Part) {
        let (open, region) = match part {
            Part::Next => (&mut self.wanted.next, Region::Next),
            Part::Explorer => (&mut self.wanted.explorer, Region::Explorer),
            Part::Panel => (&mut self.wanted.panel, Region::Panel),
        };
        *open = !*open;
        if *open {
            self.focus = region;
        } else if self.focus == region {
            self.focus = Region::Editor;
        }
    }

    /// Moves the focus to the next box on screen, as F6 does in VS Code.
    fn cycle(&mut self, by: isize) {
        let order: Vec<Region> = [
            (Region::Editor, true),
            (Region::Explorer, self.shown.explorer),
            (Region::Panel, self.shown.panel),
            (Region::Next, self.shown.next),
        ]
        .into_iter()
        .filter_map(|(region, shown)| shown.then_some(region))
        .collect();
        let at = order.iter().position(|region| *region == self.focus).unwrap_or(0);
        self.focus = order[(at as isize + by).rem_euclid(order.len() as isize) as usize];
    }

    fn show_tab(&mut self, tab: Tab) {
        self.tab = tab;
        self.scroll = 0;
        if tab == Tab::Output {
            self.unseen = 0;
        }
    }

    // ---- keys -------------------------------------------------------------

    /// What a key does: moves, and hands back the write it asks for, if any.
    pub fn key(&mut self, key: KeyEvent) -> Option<Action> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Char('q' | 'c') if ctrl => self.quit = true,
            KeyCode::Char('p') if ctrl => self.focus = Region::Command,
            KeyCode::Char('b') if ctrl && alt => self.toggle(Part::Next),
            KeyCode::Char('b') if ctrl => self.toggle(Part::Explorer),
            KeyCode::Char('j') if ctrl => self.toggle(Part::Panel),
            KeyCode::F(6) => self.cycle(if key.modifiers.contains(KeyModifiers::SHIFT) { -1 } else { 1 }),
            KeyCode::Tab => self.cycle(1),
            KeyCode::BackTab => self.cycle(-1),
            _ => {
                return match self.focus {
                    Region::Command => {
                        self.command_key(key);
                        None
                    }
                    Region::Next => {
                        self.next_key(key);
                        None
                    }
                    Region::Explorer => {
                        self.explorer_key(key);
                        None
                    }
                    Region::Panel => {
                        self.panel_key(key);
                        None
                    }
                    _ => self.board_key(key),
                };
            }
        }
        None
    }

    fn typed(key: &KeyEvent) -> Option<char> {
        match key.code {
            KeyCode::Char(c) if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => Some(c),
            _ => None,
        }
    }

    fn command_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                if !self.filter.is_empty() {
                    self.set_filter(String::new());
                }
                self.focus = Region::Editor;
            }
            KeyCode::Enter => self.focus = Region::Editor,
            KeyCode::Backspace => {
                let mut filter = self.filter.clone();
                filter.pop();
                self.set_filter(filter);
            }
            KeyCode::Up => self.move_board(-1),
            KeyCode::Down => self.move_board(1),
            _ => {
                if let Some(c) = Self::typed(&key) {
                    self.set_filter(format!("{}{c}", self.filter));
                }
            }
        }
    }

    /// The board list is where the writes are, on the keys the picker had:
    /// Enter completes, Space starts or pauses, Ctrl+S stashes. Anything else
    /// printable starts a filter, as it did there.
    fn board_key(&mut self, key: KeyEvent) -> Option<Action> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Up => self.move_board(-1),
            KeyCode::Down => self.move_board(1),
            KeyCode::PageUp => self.move_board(-(self.page.max(1) as isize)),
            KeyCode::PageDown => self.move_board(self.page.max(1) as isize),
            KeyCode::Home => self.board.selected = self.item_row(0, 1).unwrap_or(0),
            KeyCode::End => {
                self.board.selected = self.item_row(self.rows.len().saturating_sub(1), -1).unwrap_or(0)
            }
            KeyCode::Enter => return Some(Action::Done),
            KeyCode::Char(' ') if !ctrl => return Some(Action::Progress),
            KeyCode::Char('s') if ctrl => return Some(Action::Stash),
            KeyCode::Esc if !self.filter.is_empty() => self.set_filter(String::new()),
            KeyCode::Backspace if !self.filter.is_empty() => {
                let mut filter = self.filter.clone();
                filter.pop();
                self.set_filter(filter);
                self.focus = Region::Command;
            }
            _ => {
                if let Some(c) = Self::typed(&key) {
                    self.set_filter(format!("{}{c}", self.filter));
                    self.focus = Region::Command;
                }
            }
        }
        None
    }

    fn next_key(&mut self, key: KeyEvent) {
        let len = self.snapshot.next().len();
        match key.code {
            KeyCode::Up => self.next.step(-1, len),
            KeyCode::Down => self.next.step(1, len),
            KeyCode::Home => self.next.selected = 0,
            KeyCode::End => self.next.selected = len.saturating_sub(1),
            KeyCode::Enter => {
                if let Some(id) = self.snapshot.next().get(self.next.selected).map(|entry| entry.id) {
                    self.reveal(id);
                    self.focus = Region::Editor;
                }
            }
            _ => {}
        }
    }

    fn explorer_key(&mut self, key: KeyEvent) {
        let len = self.snapshot.groups.len();
        match key.code {
            KeyCode::Up => self.explorer.step(-1, len),
            KeyCode::Down => self.explorer.step(1, len),
            KeyCode::Home => self.explorer.selected = 0,
            KeyCode::End => self.explorer.selected = len.saturating_sub(1),
            KeyCode::Enter if len > 0 => {
                self.reveal_board(self.explorer.selected);
                self.focus = Region::Editor;
            }
            _ => {}
        }
    }

    fn panel_key(&mut self, key: KeyEvent) {
        let step = match key.code {
            KeyCode::Left | KeyCode::Right => {
                let at = Tab::ALL.iter().position(|tab| *tab == self.tab).unwrap_or(0) as isize;
                let by = if key.code == KeyCode::Left { -1 } else { 1 };
                self.show_tab(Tab::ALL[(at + by).rem_euclid(Tab::ALL.len() as isize) as usize]);
                return;
            }
            KeyCode::Enter => {
                if self.tab == Tab::Problems {
                    if let Some(id) = self.snapshot.problems.get(self.problems.selected).map(|p| p.id) {
                        self.reveal(id);
                        self.focus = Region::Editor;
                    }
                }
                return;
            }
            KeyCode::Up => -1,
            KeyCode::Down => 1,
            KeyCode::PageUp => -10,
            KeyCode::PageDown => 10,
            KeyCode::Home => isize::MIN / 2,
            KeyCode::End => isize::MAX / 2,
            _ => return,
        };
        self.scroll_panel(step);
    }

    fn scroll_panel(&mut self, by: isize) {
        match self.tab {
            Tab::Problems => self.problems.step(by, self.snapshot.problems.len()),
            _ => self.scroll = (self.scroll as isize).saturating_add(by).max(0) as usize,
        }
    }

    // ---- the mouse ---------------------------------------------------------

    pub fn mouse(&mut self, mouse: MouseEvent) {
        let target = self
            .hits
            .iter()
            .rev()
            .find(|(rect, _)| {
                mouse.column >= rect.x
                    && mouse.column < rect.x + rect.width
                    && mouse.row >= rect.y
                    && mouse.row < rect.y + rect.height
            })
            .map(|(_, target)| *target);
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(target) = target {
                    self.click(target);
                }
            }
            MouseEventKind::ScrollDown => self.wheel(target, 1),
            MouseEventKind::ScrollUp => self.wheel(target, -1),
            _ => {}
        }
    }

    fn click(&mut self, target: Target) {
        match target {
            Target::Region(region) => self.focus = region,
            Target::Command => self.focus = Region::Command,
            Target::Toggle(part) => self.toggle(part),
            Target::BoardRow(row) => {
                self.focus = Region::Editor;
                if matches!(self.rows.get(row), Some(Row::Item { .. })) {
                    self.board.selected = row;
                }
            }
            Target::NextRow(at) => {
                self.focus = Region::Next;
                self.next.selected = at;
                if let Some(id) = self.snapshot.next().get(at).map(|entry| entry.id) {
                    self.reveal(id);
                }
            }
            Target::ExplorerRow(group) => {
                self.focus = Region::Explorer;
                self.explorer.selected = group;
                self.reveal_board(group);
            }
            Target::ProblemRow(at) => {
                self.focus = Region::Panel;
                self.problems.selected = at;
                if let Some(id) = self.snapshot.problems.get(at).map(|problem| problem.id) {
                    self.reveal(id);
                }
            }
            // The status bar's counters name a tab too, and the panel may be closed.
            Target::PanelTab(tab) => {
                self.wanted.panel = true;
                self.focus = Region::Panel;
                self.show_tab(tab);
            }
            Target::Bell => {
                self.wanted.panel = true;
                self.focus = Region::Panel;
                self.show_tab(Tab::Output);
            }
        }
    }

    /// The wheel scrolls whatever is under the pointer, focused or not.
    fn wheel(&mut self, target: Option<Target>, by: isize) {
        let region = match target {
            Some(Target::Region(region)) => region,
            Some(Target::BoardRow(_)) => Region::Editor,
            Some(Target::NextRow(_)) => Region::Next,
            Some(Target::ExplorerRow(_)) => Region::Explorer,
            Some(Target::ProblemRow(_) | Target::PanelTab(_)) => Region::Panel,
            _ => return,
        };
        match region {
            Region::Editor => self.move_board(by),
            Region::Next => self.next.step(by, self.snapshot.next().len()),
            Region::Explorer => self.explorer.step(by, self.snapshot.groups.len()),
            Region::Panel => self.scroll_panel(by * if self.tab == Tab::Problems { 1 } else { 3 }),
            _ => {}
        }
    }

    // ---- writing ------------------------------------------------------------

    fn log(&mut self, text: impl Into<String>, kind: Kind) {
        let at = chrono::Local::now().format("%H:%M:%S").to_string();
        self.log.push(Logged { at, text: text.into(), kind });
        if self.log.len() > LOG_LINES {
            self.log.remove(0);
        }
    }

    fn say(&mut self, text: String, kind: Kind) {
        self.log(text.clone(), kind);
        self.flash = Some((text, kind, Instant::now()));
    }

    /// Applies one action to the selected item, then reads the board back.
    /// Returns whether anything was written.
    ///
    /// The lock is taken by the write and released the moment it returns --
    /// never held while the frame sits idle. The item is named by its uid, so
    /// a board renumbered by a write elsewhere since the last frame cannot
    /// turn the key into a write to some other item.
    pub fn act(&mut self, ekko: &Ekko, action: Action) -> Result<bool, EkkoError> {
        let Some(at) = self.selected() else { return Ok(false) };
        let item = &self.snapshot.items[at];
        let id = item.id;
        let target = item.uid.clone().unwrap_or_else(|| id.to_string());
        let state = State::of(item);

        let result = match action {
            Action::Stash => ekko.set_stashed(std::slice::from_ref(&target), true).map(|_| format!("{id} stashed")),
            _ if state.is_none() => {
                self.say(format!("{id} is a note, and notes have no state"), Kind::Refused);
                return Ok(false);
            }
            // Done and cancelled are terminal here. A stray key on a finished
            // task used to undo the fact that it was finished -- which is how
            // two items got un-completed the hour the picker shipped.
            _ if matches!(state, Some(State::Done | State::Cancelled)) => {
                self.say(format!("{id} is finished; change it from the CLI with --set"), Kind::Refused);
                return Ok(false);
            }
            // Never forced from here. `--force` is a deliberate override, and a
            // frame full of keys is exactly where one gets pressed without
            // deliberation.
            Action::Done => ekko.set_state(&[format!("@{target}"), "done".into()], false).map(|_| format!("{id} done")),
            Action::Progress => {
                let (next, said) =
                    if state == Some(State::Progress) { ("paused", "paused") } else { ("progress", "in progress") };
                ekko.set_state(&[format!("@{target}"), next.into()], false).map(|_| format!("{id} {said}"))
            }
        };

        match result {
            Ok(said) => {
                self.say(said, Kind::Did);
                self.reload(ekko)?;
                Ok(true)
            }
            Err(EkkoError::Blocked(blocked)) => {
                let by: Vec<String> = blocked.iter().flat_map(|(_, by)| by).map(u32::to_string).collect();
                self.say(
                    format!("{id} is blocked by {}; finish that first, or --force it from the CLI", by.join(", ")),
                    Kind::Refused,
                );
                Ok(false)
            }
            Err(error) => {
                self.say(error.to_string(), Kind::Refused);
                Ok(false)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::tui::board::tests::{board, words};

    fn open(dir: &Path, ekko: &Ekko) -> App {
        let workspace = Workspace {
            label: "test".into(),
            name: "test".into(),
            folder: "~/test".into(),
            cwd: dir.to_path_buf(),
            branch: None,
        };
        App::new(workspace, Snapshot::load(ekko, "test").unwrap())
    }

    fn press(app: &mut App, code: KeyCode) -> Option<Action> {
        app.key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn ctrl(app: &mut App, c: char) -> Option<Action> {
        app.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
    }

    fn state(ekko: &Ekko, id: u32) -> Option<State> {
        State::of(&ekko.storage.get().unwrap()[&id])
    }

    fn selected_description(app: &App) -> &str {
        &app.snapshot.items[app.selected().expect("nothing is selected")].description
    }

    /// Enter completes the selected task, and no key undoes a finished one.
    #[test]
    fn a_finished_task_stays_finished_whatever_is_pressed() {
        let (dir, ekko) = board("finished");
        ekko.create_task(&words(&["@a", "one"])).unwrap();
        let mut app = open(&dir, &ekko);

        let action = press(&mut app, KeyCode::Enter).unwrap();
        assert!(app.act(&ekko, action).unwrap());
        assert_eq!(state(&ekko, 1), Some(State::Done));

        for code in [KeyCode::Enter, KeyCode::Char(' ')] {
            let action = press(&mut app, code).unwrap();
            assert!(!app.act(&ekko, action).unwrap(), "{code:?} wrote to a finished task");
        }
        assert_eq!(state(&ekko, 1), Some(State::Done));
        assert!(app.flash().unwrap().0.contains("finished"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn space_starts_a_task_and_pauses_it_again() {
        let (dir, ekko) = board("space");
        ekko.create_task(&words(&["@a", "one"])).unwrap();
        let mut app = open(&dir, &ekko);

        let action = press(&mut app, KeyCode::Char(' ')).unwrap();
        app.act(&ekko, action).unwrap();
        assert_eq!(state(&ekko, 1), Some(State::Progress));
        let action = press(&mut app, KeyCode::Char(' ')).unwrap();
        app.act(&ekko, action).unwrap();
        assert_eq!(state(&ekko, 1), Some(State::Paused));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The dependency rule holds from a key as from anywhere, and the refusal
    /// says where the override lives instead of offering one.
    #[test]
    fn a_blocked_task_is_refused_and_never_forced() {
        let (dir, ekko) = board("blocked");
        ekko.create_task(&words(&["@a", "base"])).unwrap();
        ekko.create_task(&words(&["@a", "on top"])).unwrap();
        ekko.set_blocked_by(&words(&["@2", "1"])).unwrap();
        let mut app = open(&dir, &ekko);

        press(&mut app, KeyCode::Down);
        assert_eq!(selected_description(&app), "on top");
        let action = press(&mut app, KeyCode::Enter).unwrap();
        assert!(!app.act(&ekko, action).unwrap());
        assert_eq!(state(&ekko, 2), Some(State::Pending));
        let (said, kind) = app.flash().unwrap();
        assert!(said.contains("blocked by 1") && kind == Kind::Refused, "{said}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_note_has_no_state_but_can_be_stashed() {
        let (dir, ekko) = board("note");
        ekko.create_note(&words(&["@a", "a thought"])).unwrap();
        let mut app = open(&dir, &ekko);

        let action = press(&mut app, KeyCode::Enter).unwrap();
        assert!(!app.act(&ekko, action).unwrap());
        assert!(app.flash().unwrap().0.contains("note"));

        let action = ctrl(&mut app, 's').unwrap();
        assert!(app.act(&ekko, action).unwrap());
        assert!(ekko.storage.get().unwrap()[&1].stashed.is_some());
        assert!(app.selected().is_none(), "a stashed note is still on the board");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A write elsewhere renumbers nothing on screen out from under the
    /// person: the same item stays selected, and Output counts the change.
    #[test]
    fn the_selection_follows_its_item_across_a_change_elsewhere() {
        let (dir, ekko) = board("follow");
        for description in ["one", "two", "three"] {
            ekko.create_task(&words(&["@a", description])).unwrap();
        }
        let mut app = open(&dir, &ekko);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        assert_eq!(selected_description(&app), "three");

        Ekko::at(&dir).unwrap().set_trashed(&words(&["1"]), true).unwrap();
        app.outside_change(&ekko).unwrap();

        assert_eq!(selected_description(&app), "three");
        assert_eq!(app.unseen, 1);
        assert!(app.pulsing());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn typing_on_the_board_filters_and_escape_brings_everything_back() {
        let (dir, ekko) = board("typing");
        ekko.create_task(&words(&["@a", "wire the watcher"])).unwrap();
        ekko.create_task(&words(&["@a", "draw the frame"])).unwrap();
        ekko.create_task(&words(&["@b", "write the docs"])).unwrap();
        let mut app = open(&dir, &ekko);

        for c in "wir".chars() {
            assert_eq!(press(&mut app, KeyCode::Char(c)), None, "typing wrote something");
        }
        assert_eq!((app.focus, app.filter.as_str(), app.rows.len()), (Region::Command, "wir", 2));
        assert_eq!(selected_description(&app), "wire the watcher");

        press(&mut app, KeyCode::Esc);
        assert_eq!((app.focus, app.filter.as_str(), app.rows.len()), (Region::Editor, "", 5));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn moving_steps_over_headings_and_stops_at_the_ends() {
        let (dir, ekko) = board("moving");
        ekko.create_task(&words(&["@a", "one"])).unwrap();
        ekko.create_task(&words(&["@b", "two"])).unwrap();
        let mut app = open(&dir, &ekko);
        assert_eq!(app.board.selected, 1);

        press(&mut app, KeyCode::Down);
        assert_eq!(app.board.selected, 3, "did not step over the second heading");
        press(&mut app, KeyCode::Down);
        assert_eq!(app.board.selected, 3, "went past the end");
        press(&mut app, KeyCode::Up);
        press(&mut app, KeyCode::Up);
        assert_eq!(app.board.selected, 1, "landed on a heading or went past the top");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// VS Code's keys open and close the boxes, and focus only visits the
    /// boxes on screen.
    #[test]
    fn boxes_open_and_close_and_focus_visits_only_what_is_shown() {
        let (dir, ekko) = board("boxes");
        let mut app = open(&dir, &ekko);

        ctrl(&mut app, 'b');
        assert!(!app.wanted.explorer);
        app.shown = app.wanted;
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.focus, Region::Panel);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.focus, Region::Next);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.focus, Region::Editor);

        press(&mut app, KeyCode::BackTab);
        assert_eq!(app.focus, Region::Next);
        app.key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL | KeyModifiers::ALT));
        assert!(!app.wanted.next);
        assert_eq!(app.focus, Region::Editor, "focus stayed on a closed box");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A click lands on whatever the last frame recorded under it, the most
    /// specific thing first.
    #[test]
    fn a_click_lands_on_what_was_drawn_under_it() {
        let (dir, ekko) = board("click");
        ekko.create_task(&words(&["@a", "one"])).unwrap();
        ekko.create_task(&words(&["@a", "two"])).unwrap();
        let mut app = open(&dir, &ekko);
        app.focus = Region::Next;
        app.unseen = 3;
        app.hits = vec![
            (Rect::new(0, 0, 100, 40), Target::Region(Region::Editor)),
            (Rect::new(2, 6, 50, 1), Target::BoardRow(2)),
            (Rect::new(99, 39, 1, 1), Target::Bell),
        ];
        let click = |column, row| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        };

        app.mouse(click(10, 6));
        assert_eq!((app.focus, app.board.selected), (Region::Editor, 2));
        app.mouse(click(99, 39));
        assert_eq!((app.tab, app.unseen, app.focus), (Tab::Output, 0, Region::Panel));
        std::fs::remove_dir_all(&dir).ok();
    }
}
