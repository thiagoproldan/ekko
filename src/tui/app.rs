//! The interactive mode's state, and what each key and click does to it.
//!
//! Nothing here draws or touches the terminal, so every rule -- which key
//! moves, which one writes, what a refusal says -- is tested without one.
//! Moving and writing are kept apart by type: a key or a click changes what is
//! selected, shown, opened or focused, and only an `Action` handed back by
//! `key` writes, through `act`. What happens in a graph is in `graph`.

mod graph;

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use chrono::Datelike;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

use super::board::{self, Reading, Row, Snapshot};
use super::graph::settings::{controls, Settings};
use super::graph::{GraphView, Panel, Pane, Tool};
use super::layout::{Region, Visibility};
use super::list::Cursor;
use super::search;
use super::tabs::{Doc, Page, Tabs};
use super::tree::{self, Tree};
use crate::ekko::{Ekko, EkkoError};
use crate::item::State;

/// How long a message stays in the status bar.
const FLASH: Duration = Duration::from_secs(6);
/// How long the live indicator stays lit after the board changed elsewhere.
const PULSE: Duration = Duration::from_secs(2);
/// Lines Output keeps.
const LOG_LINES: usize = 500;
/// Two clicks on one cell closer together than this are a double click.
const DOUBLE_CLICK: Duration = Duration::from_millis(450);
/// How long Ctrl+K waits for the second key of its chord.
const CHORD: Duration = Duration::from_secs(3);

/// Where the interactive mode was opened.
pub struct Workspace {
    /// How `prime` names the board, as the CLI does.
    pub label: String,
    /// The project's name, or what the board is when it is not a project's.
    pub name: String,
    /// The folder, with home shortened to `~`.
    pub folder: String,
    pub cwd: PathBuf,
    pub home: PathBuf,
    /// The board's `.ekko/` directory, where the graph's settings are kept.
    pub dir: PathBuf,
    pub branch: Option<String>,
}

/// The bottom panel's tabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelTab {
    Problems,
    Output,
    Agent,
}

impl PanelTab {
    pub const ALL: [PanelTab; 3] = [PanelTab::Problems, PanelTab::Output, PanelTab::Agent];

    pub fn title(self) -> &'static str {
        match self {
            PanelTab::Problems => "Problems",
            PanelTab::Output => "Output",
            PanelTab::Agent => "Agent",
        }
    }
}

/// What the activity bar puts in the sidebar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Explorer,
    Search,
    /// The local graph: the neighbourhood of the active item.
    Graph,
    Projects,
    Changes,
}

impl View {
    pub fn title(self) -> &'static str {
        match self {
            View::Explorer => "Explorer",
            View::Search => "Search",
            View::Graph => "Graph",
            View::Projects => "Projects",
            View::Changes => "Changes",
        }
    }
}

/// A box that can be closed and opened again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Next,
    Sidebar,
    Panel,
}

/// What a click can land on, recorded as each thing is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Region(Region),
    Command,
    Toggle(Part),
    Activity(View),
    BoardRow(usize),
    NextRow(usize),
    ProblemRow(usize),
    PanelTab(PanelTab),
    Bell,
    Tab(usize),
    CloseTab(usize),
    Page(Page),
    TreeRow(usize),
    SearchField,
    Chip(usize),
    SearchRow(usize),
    ProjectRow(usize),
    ChangeRow(usize),
    /// An item named in a document, by its place in `App::hit_keys`.
    Key(usize),
    RoadmapRow(usize),
    Day(u32),
    /// The calendar's month arrows, and 0 for today.
    Month(i32),
    Welcome(usize),
    /// A graph's canvas.
    Canvas(Pane),
    Tool(Pane, Tool),
    /// A row of the graph's settings panel.
    Control(usize),
    /// A slider's track in the settings panel, by row.
    Track(usize),
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

/// One line of the search results: a board heading, or an item under it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchLine {
    Board { group: usize, count: usize },
    Item(usize),
}

/// One line of the roadmap: a phase, or an item in a phase that is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoadmapLine {
    Phase(usize),
    Item(u32),
}

/// What a link on the Welcome page does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Welcome {
    Page(Page),
    View(View),
    Problems,
    Project(usize),
}

/// Where each document is: its cursors, and the calendar's month.
pub struct DocState {
    /// The relation under the cursor in an item's tab, and how far it scrolled.
    pub links: Cursor,
    pub scroll: usize,
    /// The link the tab last scrolled to, so scrolling by hand is not undone.
    pub followed: Option<usize>,
    pub roadmap: Cursor,
    pub open_phases: HashSet<String>,
    pub welcome: Cursor,
    pub month: (i32, u32),
    pub day: u32,
    /// The document these cursors were last used for.
    pub seen: Doc,
}

pub struct App {
    pub workspace: Workspace,
    pub snapshot: Snapshot,
    /// When the session began, in epoch millis: what Changes counts from.
    pub since: i64,
    pub rows: Vec<Row>,
    pub filter: String,
    pub focus: Region,
    /// The boxes the person has open.
    pub wanted: Visibility,
    /// The boxes the last frame drew: `wanted`, less what the terminal had no
    /// room for.
    pub shown: Visibility,
    pub view: View,
    pub panel: PanelTab,
    pub tabs: Tabs,
    pub doc: DocState,
    pub board: Cursor,
    pub next: Cursor,
    pub problems: Cursor,
    pub tree: Tree,
    pub search: String,
    pub found: Cursor,
    pub projects: Cursor,
    pub changes: Cursor,
    /// The graph view's settings, and its panel.
    pub settings: Settings,
    pub graph_panel: Panel,
    /// The whole board's graph, in its tab, and the local one in the sidebar.
    pub graph: GraphView,
    pub local: GraphView,
    /// Counts the times the board was read, so a graph knows when to rebuild.
    pub generation: u64,
    /// The graph a press began in, and whether it was a double click, until
    /// the button is let go.
    dragging: Option<(Pane, bool)>,
    /// Lines scrolled past in Output or Agent.
    pub scroll: usize,
    pub log: Vec<Logged>,
    flash: Option<(String, Kind, Instant)>,
    /// Changes made elsewhere since Output was last in view.
    pub unseen: usize,
    pulse: Option<Instant>,
    chord: Option<Instant>,
    last_click: Option<(Instant, u16, u16)>,
    pub hits: Vec<(Rect, Target)>,
    pub hit_keys: Vec<String>,
    /// Rows the board list had room for in the last frame, for Page Up/Down.
    pub page: usize,
    /// A project chosen to switch to, by name, for the loop to open.
    pub switch: Option<String>,
    pub quit: bool,
}

/// An item's relations in the order its tab lists them.
pub fn relations(snapshot: &Snapshot, id: u32) -> Vec<(&'static str, Vec<u32>)> {
    let links = snapshot.links.get(&id).cloned().unwrap_or_default();
    vec![
        ("Blocked by", links.blockers),
        ("Blocks", links.dependents),
        ("Attached to", links.attached_to.into_iter().collect()),
        ("Notes", links.notes),
    ]
}

fn days_in(year: i32, month: u32) -> u32 {
    let (next_year, next_month) = if month == 12 { (year + 1, 1) } else { (year, month + 1) };
    chrono::NaiveDate::from_ymd_opt(next_year, next_month, 1)
        .and_then(|first| first.pred_opt())
        .map_or(28, |last| last.day())
}

impl App {
    pub fn new(workspace: Workspace, snapshot: Snapshot, since: i64) -> Self {
        let today = chrono::Local::now().date_naive();
        let open_phases = snapshot.roadmap.iter().filter(|step| step.current).map(|step| step.name.clone()).collect();
        let first_run = snapshot.all.is_empty();
        let settings = Settings::load(&workspace.dir);
        let mut app = App {
            workspace,
            snapshot,
            since,
            rows: Vec::new(),
            filter: String::new(),
            focus: Region::Editor,
            wanted: Visibility::default(),
            shown: Visibility::default(),
            view: View::Explorer,
            panel: PanelTab::Problems,
            tabs: Tabs::default(),
            doc: DocState {
                links: Cursor::default(),
                scroll: 0,
                followed: Some(0),
                roadmap: Cursor::default(),
                open_phases,
                welcome: Cursor::default(),
                month: (today.year(), today.month()),
                day: today.day(),
                seen: Doc::Board,
            },
            board: Cursor::default(),
            next: Cursor::default(),
            problems: Cursor::default(),
            tree: Tree::default(),
            search: String::new(),
            found: Cursor::default(),
            projects: Cursor::default(),
            changes: Cursor::default(),
            settings,
            graph_panel: Panel::default(),
            graph: GraphView::default(),
            local: GraphView::default(),
            generation: 0,
            dragging: None,
            scroll: 0,
            log: Vec::new(),
            flash: None,
            unseen: 0,
            pulse: None,
            chord: None,
            last_click: None,
            hits: Vec::new(),
            hit_keys: Vec::new(),
            page: 10,
            switch: None,
            quit: false,
        };
        // A board with nothing on it yet opens on the Welcome page, as a fresh
        // VS Code window does.
        if first_run {
            app.tabs.open(Doc::Welcome, false);
        }
        app.restore(None, true);
        app
    }

    /// The selected board row's item, by its index in `snapshot.items`.
    pub fn selected(&self) -> Option<usize> {
        match self.rows.get(self.board.selected) {
            Some(Row::Item { item, .. }) => Some(*item),
            _ => None,
        }
    }

    /// The item the outline, the item tabs and the local graph are about: the
    /// open item tab's, or else the one selected on the board.
    pub fn active_item(&self) -> Option<u32> {
        match self.tabs.active() {
            Doc::Item(key) => self.snapshot.find(key).filter(|(_, archived)| !archived).map(|(item, _)| item.id),
            _ => self.selected().map(|at| self.snapshot.items[at].id),
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

    /// Whether Ctrl+K is waiting for the rest of its chord.
    pub fn chording(&self) -> bool {
        self.chord.is_some_and(|at| at.elapsed() < CHORD)
    }

    pub fn tree_rows(&self) -> Vec<tree::Row> {
        self.tree.rows(&self.snapshot, &self.tabs, &self.workspace.name, self.active_item())
    }

    pub fn search_lines(&self) -> Vec<SearchLine> {
        let query = search::parse(&self.search);
        let mut lines = Vec::new();
        for hit in search::results(&self.snapshot, &query) {
            lines.push(SearchLine::Board { group: hit.group, count: hit.items.len() });
            lines.extend(hit.items.into_iter().map(SearchLine::Item));
        }
        lines
    }

    pub fn roadmap_lines(&self) -> Vec<RoadmapLine> {
        let mut lines = Vec::new();
        for (at, step) in self.snapshot.roadmap.iter().enumerate() {
            lines.push(RoadmapLine::Phase(at));
            if self.doc.open_phases.contains(&step.name) {
                lines.extend(self.snapshot.in_phase(&step.name).iter().map(|item| RoadmapLine::Item(item.id)));
            }
        }
        lines
    }

    pub fn welcome_links(&self) -> Vec<Welcome> {
        let mut links = vec![
            Welcome::Page(Page::Board),
            Welcome::Page(Page::Graph),
            Welcome::Page(Page::Roadmap),
            Welcome::Page(Page::Calendar),
            Welcome::View(View::Search),
            Welcome::Problems,
        ];
        links.extend((0..self.snapshot.projects.len()).map(Welcome::Project));
        links
    }

    /// The items an item's tab links to, in the order it lists them.
    pub fn item_links(&self, key: &str) -> Vec<u32> {
        match self.snapshot.find(key) {
            Some((item, false)) => relations(&self.snapshot, item.id).into_iter().flat_map(|(_, ids)| ids).collect(),
            _ => Vec::new(),
        }
    }

    // ---- reading the board ----------------------------------------------

    /// Reads the board again, keeping the same item selected.
    pub fn reload(&mut self, ekko: &Ekko) -> Result<(), EkkoError> {
        let kept = self.kept();
        let reading = Reading { label: &self.workspace.label, home: &self.workspace.home, since: self.since };
        self.snapshot = Snapshot::load(ekko, &reading)?;
        self.generation += 1;
        self.restore(kept, false);
        Ok(())
    }

    /// The board was written by someone else: another terminal, or an agent.
    pub fn outside_change(&mut self, ekko: &Ekko) -> Result<(), EkkoError> {
        self.reload(ekko)?;
        self.log("The board changed elsewhere, and is shown as it is now", Kind::Outside);
        if !(self.shown.panel && self.panel == PanelTab::Output) {
            self.unseen += 1;
        }
        self.pulse = Some(Instant::now());
        Ok(())
    }

    /// The selected item's board and key, to find it again after the rows change.
    fn kept(&self) -> Option<(String, String)> {
        let Some(Row::Item { group, item }) = self.rows.get(self.board.selected) else { return None };
        Some((self.snapshot.groups[*group].name.clone(), board::key(&self.snapshot.items[*item])))
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
                        board::key(&self.snapshot.items[*item]) == key
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
        self.problems.clamp(self.snapshot.problems.len());
        self.projects.clamp(self.snapshot.projects.len());
        self.changes.clamp(self.snapshot.changes.len());
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

    // ---- moving and opening ------------------------------------------------

    /// Moves the board selection by `by` rows, stepping over headings and
    /// stopping at either end.
    fn move_board(&mut self, by: isize) {
        if self.rows.is_empty() {
            return;
        }
        let target = (self.board.selected as isize).saturating_add(by).clamp(0, self.rows.len() as isize - 1) as usize;
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

    fn open_page(&mut self, page: Page) {
        self.tabs.open(page.doc(), false);
        self.focus = Region::Editor;
    }

    /// Opens an item's tab: in passing as a preview, or on purpose, which
    /// pins it and moves to the editor.
    fn open_item(&mut self, key: String, preview: bool) {
        self.tabs.open(Doc::Item(key), preview);
        if !preview {
            self.focus = Region::Editor;
        }
    }

    fn open_selected(&mut self) {
        if let Some(at) = self.selected() {
            let key = board::key(&self.snapshot.items[at]);
            self.open_item(key, false);
        }
    }

    fn show_view(&mut self, view: View) {
        self.view = view;
        self.wanted.sidebar = true;
        self.focus = Region::Sidebar;
    }

    fn show_panel(&mut self, tab: PanelTab) {
        self.panel = tab;
        self.scroll = 0;
        if tab == PanelTab::Output {
            self.unseen = 0;
        }
    }

    fn toggle(&mut self, part: Part) {
        let (open, region) = match part {
            Part::Next => (&mut self.wanted.next, Region::Next),
            Part::Sidebar => (&mut self.wanted.sidebar, Region::Sidebar),
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
            (Region::Sidebar, self.shown.sidebar),
            (Region::Panel, self.shown.panel),
            (Region::Next, self.shown.next),
        ]
        .into_iter()
        .filter_map(|(region, shown)| shown.then_some(region))
        .collect();
        let at = order.iter().position(|region| *region == self.focus).unwrap_or(0);
        self.focus = order[(at as isize + by).rem_euclid(order.len() as isize) as usize];
    }

    fn choose_project(&mut self, at: usize) {
        let Some(project) = self.snapshot.projects.get(at) else { return };
        if project.name == self.workspace.name {
            self.say(format!("{} is the project already open", project.name), Kind::Refused);
            return;
        }
        match project.status {
            "here" => self.switch = Some(project.name.clone()),
            "missing" => self.say(
                format!("{}'s folder no longer holds it; ekko init in the folder it moved to finds it again", project.name),
                Kind::Refused,
            ),
            _ => self.say(format!("{} is not in a folder yet; ekko init in its folder adopts it", project.name), Kind::Refused),
        }
    }

    fn welcome(&mut self, link: Welcome) {
        match link {
            Welcome::Page(page) => self.open_page(page),
            Welcome::View(view) => self.show_view(view),
            Welcome::Problems => {
                self.wanted.panel = true;
                self.focus = Region::Panel;
                self.show_panel(PanelTab::Problems);
            }
            Welcome::Project(at) => self.choose_project(at),
        }
    }

    fn move_day(&mut self, by: i64) {
        let (year, month) = self.doc.month;
        let Some(date) = chrono::NaiveDate::from_ymd_opt(year, month, self.doc.day) else { return };
        let moved = date + chrono::Duration::days(by);
        self.doc.month = (moved.year(), moved.month());
        self.doc.day = moved.day();
    }

    fn move_month(&mut self, by: i32) {
        let (year, month) = self.doc.month;
        let months = year * 12 + month as i32 - 1 + by;
        self.doc.month = (months.div_euclid(12), months.rem_euclid(12) as u32 + 1);
        self.doc.day = self.doc.day.min(days_in(self.doc.month.0, self.doc.month.1));
    }

    fn today(&mut self) {
        let today = chrono::Local::now().date_naive();
        self.doc.month = (today.year(), today.month());
        self.doc.day = today.day();
    }

    /// The first item due on the calendar's selected day, opened on purpose.
    fn open_day(&mut self) {
        let (year, month) = self.doc.month;
        let key = self.snapshot.due_in(year, month).get(&self.doc.day).and_then(|items| items.first()).map(|item| board::key(item));
        if let Some(key) = key {
            self.open_item(key, false);
        }
    }

    fn change_key(&self, at: usize) -> Option<String> {
        self.snapshot.changes.get(at).map(|entry| entry.uid.clone().unwrap_or_else(|| format!("#{}", entry.id)))
    }

    /// The nearest item line of the search results from `at` towards `by`.
    fn found_item(lines: &[SearchLine], at: usize, by: isize) -> Option<usize> {
        let mut line = at as isize;
        while line >= 0 && (line as usize) < lines.len() {
            if matches!(lines[line as usize], SearchLine::Item(_)) {
                return Some(line as usize);
            }
            line += by;
        }
        None
    }

    fn reset_found(&mut self) {
        let lines = self.search_lines();
        self.found = Cursor { selected: Self::found_item(&lines, 0, 1).unwrap_or(0), offset: 0 };
    }

    fn choose_found(&mut self, at: usize, preview: bool) {
        if let Some(SearchLine::Item(index)) = self.search_lines().get(at).copied() {
            let key = board::key(&self.snapshot.items[index]);
            self.open_item(key, preview);
        }
    }

    /// Chooses a row of the tree: an item opens, an open editor comes to the
    /// front, and anything else with rows under it opens or closes.
    fn choose_row(&mut self, at: usize, preview: bool) {
        let rows = self.tree_rows();
        let Some(row) = rows.get(at) else { return };
        match &row.node {
            tree::Node::Item { key, .. } => self.open_item(key.clone(), preview),
            tree::Node::Tab(tab) => {
                self.tabs.activate(*tab);
                if !preview {
                    self.focus = Region::Editor;
                }
            }
            node if row.expandable => self.tree.toggle(node),
            _ => {}
        }
    }

    // ---- keys -------------------------------------------------------------

    /// What a key does: moves or opens, and hands back the write it asks for.
    pub fn key(&mut self, key: KeyEvent) -> Option<Action> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        if let Some(at) = self.chord.take() {
            if at.elapsed() < CHORD {
                self.chord(key);
                return None;
            }
        }
        match key.code {
            KeyCode::Char('q' | 'c') if ctrl => self.quit = true,
            KeyCode::Char('k') if ctrl => self.chord = Some(Instant::now()),
            KeyCode::Char('p') if ctrl => {
                self.tabs.open(Doc::Board, false);
                self.focus = Region::Command;
            }
            KeyCode::Char('b') if ctrl && alt => self.toggle(Part::Next),
            KeyCode::Char('b') if ctrl => self.toggle(Part::Sidebar),
            KeyCode::Char('j') if ctrl => self.toggle(Part::Panel),
            KeyCode::Char('w') if ctrl => {
                if !self.tabs.close(self.tabs.active) {
                    self.say("The board stays open: it is where the writes are".to_string(), Kind::Refused);
                }
            }
            KeyCode::PageUp if ctrl => self.tabs.step(-1),
            KeyCode::PageDown if ctrl => self.tabs.step(1),
            KeyCode::F(6) => self.cycle(if key.modifiers.contains(KeyModifiers::SHIFT) { -1 } else { 1 }),
            KeyCode::Tab => self.cycle(1),
            KeyCode::BackTab => self.cycle(-1),
            _ => match self.focus {
                Region::Command => self.command_key(key),
                Region::Next => self.next_key(key),
                Region::Sidebar => self.sidebar_key(key),
                Region::Panel => self.panel_key(key),
                Region::Editor => return self.editor_key(key),
            },
        }
        None
    }

    /// The second key of a Ctrl+K chord, with or without Ctrl held, as VS Code
    /// takes either.
    fn chord(&mut self, key: KeyEvent) {
        let KeyCode::Char(c) = key.code else {
            if key.code != KeyCode::Esc {
                self.say("The key combination (Ctrl+K, …) is not a command".to_string(), Kind::Refused);
            }
            return;
        };
        match c.to_ascii_lowercase() {
            'b' => self.open_page(Page::Board),
            'g' => self.open_page(Page::Graph),
            'w' => self.open_page(Page::Welcome),
            'r' => self.open_page(Page::Roadmap),
            'c' => self.open_page(Page::Calendar),
            'e' => self.show_view(View::Explorer),
            'f' => self.show_view(View::Search),
            'l' => self.show_view(View::Graph),
            'p' => self.show_view(View::Projects),
            'h' => self.show_view(View::Changes),
            ',' => {
                self.open_page(Page::Graph);
                self.graph_panel.open = true;
            }
            other => self.say(
                format!("The key combination (Ctrl+K, {}) is not a command", other.to_ascii_uppercase()),
                Kind::Refused,
            ),
        }
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

    fn editor_key(&mut self, key: KeyEvent) -> Option<Action> {
        match self.tabs.active().clone() {
            Doc::Board => return self.board_key(key),
            Doc::Item(item) => self.item_key(&item, key),
            Doc::Roadmap => self.roadmap_key(key),
            Doc::Calendar => self.calendar_key(key),
            Doc::Welcome => self.welcome_key(key),
            Doc::Graph => self.graph_key(key),
        }
        None
    }

    /// The board is where the writes are, on the keys the picker had: Enter
    /// completes, Space starts or pauses, Ctrl+S stashes. Anything printable
    /// starts a filter, as it did there.
    ///
    /// Opening the selected item takes Alt+Enter, or Ctrl+Enter where the
    /// terminal reports Ctrl on Enter at all. Most terminals send Ctrl+Enter as
    /// a plain Enter, and a plain Enter here completes the task -- so Ctrl only
    /// opens when it arrives, and never falls through to a write when it does.
    fn board_key(&mut self, key: KeyEvent) -> Option<Action> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Up => self.move_board(-1),
            KeyCode::Down => self.move_board(1),
            KeyCode::PageUp => self.move_board(-(self.page.max(1) as isize)),
            KeyCode::PageDown => self.move_board(self.page.max(1) as isize),
            KeyCode::Home => self.board.selected = self.item_row(0, 1).unwrap_or(0),
            KeyCode::End => {
                self.board.selected = self.item_row(self.rows.len().saturating_sub(1), -1).unwrap_or(0)
            }
            KeyCode::Enter if ctrl || alt => self.open_selected(),
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

    fn item_key(&mut self, item: &str, key: KeyEvent) {
        let links = self.item_links(item);
        match key.code {
            KeyCode::Up if links.is_empty() => self.doc.scroll = self.doc.scroll.saturating_sub(1),
            KeyCode::Down if links.is_empty() => self.doc.scroll += 1,
            KeyCode::Up => self.doc.links.step(-1, links.len()),
            KeyCode::Down => self.doc.links.step(1, links.len()),
            KeyCode::Home => self.doc.links.selected = 0,
            KeyCode::End => self.doc.links.selected = links.len().saturating_sub(1),
            KeyCode::PageUp => self.doc.scroll = self.doc.scroll.saturating_sub(self.page.max(1)),
            KeyCode::PageDown => self.doc.scroll += self.page.max(1),
            KeyCode::Enter => {
                let key = links.get(self.doc.links.selected).and_then(|id| self.snapshot.all.get(id)).map(board::key);
                if let Some(key) = key {
                    self.open_item(key, false);
                }
            }
            _ => {}
        }
    }

    fn roadmap_key(&mut self, key: KeyEvent) {
        let lines = self.roadmap_lines();
        match key.code {
            KeyCode::Up => self.doc.roadmap.step(-1, lines.len()),
            KeyCode::Down => self.doc.roadmap.step(1, lines.len()),
            KeyCode::Home => self.doc.roadmap.selected = 0,
            KeyCode::End => self.doc.roadmap.selected = lines.len().saturating_sub(1),
            KeyCode::Right | KeyCode::Left | KeyCode::Enter => self.choose_roadmap(key.code, false),
            _ => {}
        }
    }

    fn choose_roadmap(&mut self, code: KeyCode, preview: bool) {
        let lines = self.roadmap_lines();
        match lines.get(self.doc.roadmap.selected).copied() {
            Some(RoadmapLine::Phase(at)) => {
                let name = self.snapshot.roadmap[at].name.clone();
                let open = self.doc.open_phases.contains(&name);
                match code {
                    KeyCode::Right if !open => {
                        self.doc.open_phases.insert(name);
                    }
                    KeyCode::Left if open => {
                        self.doc.open_phases.remove(&name);
                    }
                    KeyCode::Enter if open => {
                        self.doc.open_phases.remove(&name);
                    }
                    KeyCode::Enter => {
                        self.doc.open_phases.insert(name);
                    }
                    _ => {}
                }
            }
            Some(RoadmapLine::Item(id)) => match code {
                KeyCode::Enter => {
                    if let Some(key) = self.snapshot.all.get(&id).map(board::key) {
                        self.open_item(key, preview);
                    }
                }
                KeyCode::Left => {
                    let at = self.doc.roadmap.selected;
                    if let Some(phase) = lines[..at].iter().rposition(|line| matches!(line, RoadmapLine::Phase(_))) {
                        self.doc.roadmap.selected = phase;
                    }
                }
                _ => {}
            },
            None => {}
        }
    }

    fn calendar_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Left => self.move_day(-1),
            KeyCode::Right => self.move_day(1),
            KeyCode::Up => self.move_day(-7),
            KeyCode::Down => self.move_day(7),
            KeyCode::PageUp => self.move_month(-1),
            KeyCode::PageDown => self.move_month(1),
            KeyCode::Home => self.today(),
            KeyCode::Enter => self.open_day(),
            _ => {}
        }
    }

    fn welcome_key(&mut self, key: KeyEvent) {
        let links = self.welcome_links();
        match key.code {
            KeyCode::Up => self.doc.welcome.step(-1, links.len()),
            KeyCode::Down => self.doc.welcome.step(1, links.len()),
            KeyCode::Home => self.doc.welcome.selected = 0,
            KeyCode::End => self.doc.welcome.selected = links.len().saturating_sub(1),
            KeyCode::Enter => {
                if let Some(link) = links.get(self.doc.welcome.selected).copied() {
                    self.welcome(link);
                }
            }
            _ => {}
        }
    }

    fn sidebar_key(&mut self, key: KeyEvent) {
        match self.view {
            View::Explorer => self.tree_key(key),
            View::Search => self.search_key(key),
            View::Graph => self.local_key(key),
            View::Projects => match key.code {
                KeyCode::Up => self.projects.step(-1, self.snapshot.projects.len()),
                KeyCode::Down => self.projects.step(1, self.snapshot.projects.len()),
                KeyCode::Enter => self.choose_project(self.projects.selected),
                _ => {}
            },
            View::Changes => match key.code {
                KeyCode::Up => self.changes.step(-1, self.snapshot.changes.len()),
                KeyCode::Down => self.changes.step(1, self.snapshot.changes.len()),
                KeyCode::Enter => {
                    if let Some(key) = self.change_key(self.changes.selected) {
                        self.open_item(key, false);
                    }
                }
                _ => {}
            },
        }
    }

    /// The tree takes VS Code's keys: arrows move, Right opens or steps in,
    /// Left closes or steps out, Enter opens an item on purpose and Space opens
    /// it in passing.
    fn tree_key(&mut self, key: KeyEvent) {
        let rows = self.tree_rows();
        self.tree.settle(&rows);
        match key.code {
            KeyCode::Up => self.tree.step(-1, &rows),
            KeyCode::Down => self.tree.step(1, &rows),
            KeyCode::PageUp => self.tree.step(-(self.page.max(1) as isize), &rows),
            KeyCode::PageDown => self.tree.step(self.page.max(1) as isize, &rows),
            KeyCode::Home => self.tree.select(0, &rows),
            KeyCode::End => self.tree.select(rows.len().saturating_sub(1), &rows),
            KeyCode::Right => self.tree.right(&rows),
            KeyCode::Left => self.tree.left(&rows),
            KeyCode::Enter => self.choose_row(self.tree.cursor.selected, false),
            KeyCode::Char(' ') => self.choose_row(self.tree.cursor.selected, true),
            _ => {}
        }
    }

    fn search_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.search.clear();
                self.reset_found();
            }
            KeyCode::Backspace => {
                self.search.pop();
                self.reset_found();
            }
            KeyCode::Up | KeyCode::Down => {
                let lines = self.search_lines();
                let by = if key.code == KeyCode::Up { -1 } else { 1 };
                let from = (self.found.selected as isize + by).max(0) as usize;
                if let Some(line) = Self::found_item(&lines, from, by) {
                    self.found.selected = line;
                }
            }
            KeyCode::Enter => self.choose_found(self.found.selected, false),
            _ => {
                if let Some(c) = Self::typed(&key) {
                    self.search.push(c);
                    self.reset_found();
                }
            }
        }
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
                    self.tabs.open(Doc::Board, false);
                    self.reveal(id);
                    self.focus = Region::Editor;
                }
            }
            _ => {}
        }
    }

    fn panel_key(&mut self, key: KeyEvent) {
        let step = match key.code {
            KeyCode::Left | KeyCode::Right => {
                let at = PanelTab::ALL.iter().position(|tab| *tab == self.panel).unwrap_or(0) as isize;
                let by = if key.code == KeyCode::Left { -1 } else { 1 };
                self.show_panel(PanelTab::ALL[(at + by).rem_euclid(PanelTab::ALL.len() as isize) as usize]);
                return;
            }
            KeyCode::Enter => {
                if self.panel == PanelTab::Problems {
                    if let Some(id) = self.snapshot.problems.get(self.problems.selected).map(|p| p.id) {
                        self.tabs.open(Doc::Board, false);
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
        match self.panel {
            PanelTab::Problems => self.problems.step(by, self.snapshot.problems.len()),
            _ => self.scroll = (self.scroll as isize).saturating_add(by).max(0) as usize,
        }
    }

    // ---- the mouse ---------------------------------------------------------

    pub fn mouse(&mut self, mouse: MouseEvent) {
        let cell = (mouse.column, mouse.row);
        // A press that began in a graph belongs to that graph until the button
        // is let go, wherever the pointer wanders meanwhile.
        if let Some((pane, double)) = self.dragging {
            match mouse.kind {
                MouseEventKind::Drag(MouseButton::Left) => {
                    self.with_view(pane, |view, _| view.drag_to(cell));
                    return;
                }
                MouseEventKind::Up(MouseButton::Left) => {
                    self.dragging = None;
                    if let Some(index) = self.with_view(pane, |view, _| view.release()) {
                        self.with_view(pane, |view, _| view.selected = Some(index));
                        self.open_node(pane, index, !double);
                    }
                    return;
                }
                _ => {}
            }
        }

        let hit = self
            .hits
            .iter()
            .rev()
            .find(|(rect, _)| {
                mouse.column >= rect.x
                    && mouse.column < rect.x + rect.width
                    && mouse.row >= rect.y
                    && mouse.row < rect.y + rect.height
            })
            .copied();
        let target = hit.map(|(_, target)| target);
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let double = self.last_click.is_some_and(|(at, column, row)| {
                    at.elapsed() < DOUBLE_CLICK && column == mouse.column && row == mouse.row
                });
                self.last_click = if double { None } else { Some((Instant::now(), mouse.column, mouse.row)) };
                if let Some((rect, target)) = hit {
                    self.click(target, double, rect, cell);
                }
            }
            // The pointer lights the node under it in a graph, and nothing
            // stays lit once it leaves.
            MouseEventKind::Moved => {
                let over = match target {
                    Some(Target::Canvas(pane)) => Some(pane),
                    _ => None,
                };
                for pane in [Pane::Global, Pane::Local] {
                    if over == Some(pane) {
                        self.with_view(pane, |view, settings| view.hover_at(cell, settings));
                    } else {
                        self.with_view(pane, |view, _| view.hover = None);
                    }
                }
            }
            MouseEventKind::ScrollDown => self.wheel(target, 1, cell),
            MouseEventKind::ScrollUp => self.wheel(target, -1, cell),
            _ => {}
        }
    }

    /// A click, and what a second click on the same place makes of it: things
    /// a single click opens in passing, a double click opens on purpose.
    fn click(&mut self, target: Target, double: bool, rect: Rect, cell: (u16, u16)) {
        match target {
            Target::Region(region) => self.focus = region,
            Target::Command => {
                self.tabs.open(Doc::Board, false);
                self.focus = Region::Command;
            }
            Target::Toggle(part) => self.toggle(part),
            Target::Activity(view) => {
                if self.view == view && self.shown.sidebar {
                    self.wanted.sidebar = false;
                    if self.focus == Region::Sidebar {
                        self.focus = Region::Editor;
                    }
                } else {
                    self.show_view(view);
                }
            }
            Target::BoardRow(row) => {
                self.focus = Region::Editor;
                if matches!(self.rows.get(row), Some(Row::Item { .. })) {
                    self.board.selected = row;
                    if double {
                        self.open_selected();
                    }
                }
            }
            Target::NextRow(at) => {
                self.focus = Region::Next;
                self.next.selected = at;
                if let Some(id) = self.snapshot.next().get(at).map(|entry| entry.id) {
                    self.reveal(id);
                }
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
                self.show_panel(tab);
            }
            Target::Bell => {
                self.wanted.panel = true;
                self.focus = Region::Panel;
                self.show_panel(PanelTab::Output);
            }
            Target::Tab(at) => {
                self.tabs.activate(at);
                if double {
                    self.tabs.pin(at);
                }
                self.focus = Region::Editor;
            }
            Target::CloseTab(at) => {
                self.tabs.close(at);
            }
            Target::Page(page) => self.open_page(page),
            Target::TreeRow(at) => {
                self.focus = Region::Sidebar;
                let rows = self.tree_rows();
                self.tree.select(at, &rows);
                self.choose_row(at, !double);
            }
            Target::SearchField => {
                self.view = View::Search;
                self.focus = Region::Sidebar;
            }
            Target::Chip(at) => {
                self.search = search::toggle_chip(&self.search, search::CHIPS[at]);
                self.reset_found();
                self.focus = Region::Sidebar;
            }
            Target::SearchRow(at) => {
                self.focus = Region::Sidebar;
                self.found.selected = at;
                self.choose_found(at, !double);
            }
            Target::ProjectRow(at) => {
                self.focus = Region::Sidebar;
                self.projects.selected = at;
                if double {
                    self.choose_project(at);
                }
            }
            Target::ChangeRow(at) => {
                self.focus = Region::Sidebar;
                self.changes.selected = at;
                if let Some(key) = self.change_key(at) {
                    self.open_item(key, !double);
                }
            }
            Target::Key(at) => {
                if let Some(key) = self.hit_keys.get(at).cloned() {
                    self.open_item(key, false);
                }
            }
            Target::RoadmapRow(at) => {
                self.focus = Region::Editor;
                self.doc.roadmap.selected = at;
                self.choose_roadmap(KeyCode::Enter, !double);
            }
            Target::Day(day) => {
                self.focus = Region::Editor;
                self.doc.day = day;
                if double {
                    self.open_day();
                }
            }
            Target::Month(0) => self.today(),
            Target::Month(by) => self.move_month(by),
            Target::Welcome(at) => {
                if let Some(link) = self.welcome_links().get(at).copied() {
                    self.welcome(link);
                }
            }
            Target::Canvas(pane) => {
                self.focus = if pane == Pane::Global { Region::Editor } else { Region::Sidebar };
                self.with_view(pane, |view, settings| view.press(cell, settings));
                self.dragging = Some((pane, double));
            }
            Target::Tool(pane, tool) => self.tool(pane, tool),
            Target::Control(row) => {
                self.focus = Region::Editor;
                self.graph_panel.cursor.selected = row;
                let rows = controls(&self.settings, &self.graph_panel.sections, false);
                if let Some(control) = rows.get(row).copied() {
                    self.use_control(control);
                }
            }
            Target::Track(row) => {
                self.focus = Region::Editor;
                self.slide(row, rect, cell.0);
            }
        }
    }

    /// The wheel scrolls whatever is under the pointer, focused or not, and
    /// zooms a graph around the pointer.
    fn wheel(&mut self, target: Option<Target>, by: isize, cell: (u16, u16)) {
        let region = match target {
            Some(Target::Canvas(pane)) => {
                self.with_view(pane, |view, _| view.wheel(cell, by));
                return;
            }
            Some(Target::Region(region)) => region,
            Some(
                Target::BoardRow(_)
                | Target::Key(_)
                | Target::RoadmapRow(_)
                | Target::Day(_)
                | Target::Welcome(_)
                | Target::Control(_)
                | Target::Track(_),
            ) => Region::Editor,
            Some(Target::NextRow(_)) => Region::Next,
            Some(Target::TreeRow(_) | Target::SearchRow(_) | Target::ProjectRow(_) | Target::ChangeRow(_) | Target::Chip(_)) => {
                Region::Sidebar
            }
            Some(Target::ProblemRow(_) | Target::PanelTab(_)) => Region::Panel,
            _ => return,
        };
        match region {
            Region::Editor => match self.tabs.active().clone() {
                Doc::Board => self.move_board(by),
                Doc::Item(_) => self.doc.scroll = (self.doc.scroll as isize).saturating_add(by * 3).max(0) as usize,
                Doc::Roadmap => self.doc.roadmap.step(by, self.roadmap_lines().len()),
                Doc::Calendar => self.move_day(by as i64 * 7),
                Doc::Welcome => self.doc.welcome.step(by, self.welcome_links().len()),
                Doc::Graph => {
                    let rows = controls(&self.settings, &self.graph_panel.sections, false).len();
                    self.graph_panel.cursor.step(by, rows);
                }
            },
            Region::Next => self.next.step(by, self.snapshot.next().len()),
            Region::Sidebar => match self.view {
                View::Explorer => {
                    let rows = self.tree_rows();
                    self.tree.settle(&rows);
                    self.tree.step(by, &rows);
                }
                View::Search => {
                    let lines = self.search_lines();
                    let from = (self.found.selected as isize + by).max(0) as usize;
                    if let Some(line) = Self::found_item(&lines, from, by) {
                        self.found.selected = line;
                    }
                }
                View::Graph => {}
                View::Projects => self.projects.step(by, self.snapshot.projects.len()),
                View::Changes => self.changes.step(by, self.snapshot.changes.len()),
            },
            Region::Panel => self.scroll_panel(by * if self.panel == PanelTab::Problems { 1 } else { 3 }),
            Region::Command => {}
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

    pub fn say(&mut self, text: String, kind: Kind) {
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
    use crate::render::ProjectSummary;
    use crate::tui::board::tests::{board, snapshot, words};

    fn open(dir: &Path, ekko: &Ekko) -> App {
        let workspace = Workspace {
            label: "test".into(),
            name: "test".into(),
            folder: "~/test".into(),
            cwd: dir.to_path_buf(),
            home: dir.to_path_buf(),
            dir: dir.to_path_buf(),
            branch: None,
        };
        App::new(workspace, snapshot(dir, ekko), 0)
    }

    fn press(app: &mut App, code: KeyCode) -> Option<Action> {
        app.key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn with(app: &mut App, code: KeyCode, modifiers: KeyModifiers) -> Option<Action> {
        app.key(KeyEvent::new(code, modifiers))
    }

    fn ctrl(app: &mut App, c: char) -> Option<Action> {
        with(app, KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn state(ekko: &Ekko, id: u32) -> Option<State> {
        State::of(&ekko.storage.get().unwrap()[&id])
    }

    fn selected_description(app: &App) -> &str {
        &app.snapshot.items[app.selected().expect("nothing is selected")].description
    }

    fn pointer(app: &mut App, kind: MouseEventKind, column: u16, row: u16) {
        app.mouse(MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE });
    }

    fn click(app: &mut App, column: u16, row: u16) {
        pointer(app, MouseEventKind::Down(MouseButton::Left), column, row);
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
        assert_eq!(app.generation, 1, "a reload did not count");
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
        app.tabs.open(Doc::Board, false);

        ctrl(&mut app, 'b');
        assert!(!app.wanted.sidebar);
        app.shown = app.wanted;
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.focus, Region::Panel);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.focus, Region::Next);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.focus, Region::Editor);

        press(&mut app, KeyCode::BackTab);
        assert_eq!(app.focus, Region::Next);
        with(&mut app, KeyCode::Char('b'), KeyModifiers::CONTROL | KeyModifiers::ALT);
        assert!(!app.wanted.next);
        assert_eq!(app.focus, Region::Editor, "focus stayed on a closed box");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Alt+Enter, and Ctrl+Enter where it arrives as such, open the selected
    /// item; a plain Enter still completes it.
    #[test]
    fn enter_with_a_modifier_opens_the_item_and_plain_enter_completes_it() {
        let (dir, ekko) = board("open");
        ekko.create_task(&words(&["@a", "one"])).unwrap();
        let mut app = open(&dir, &ekko);
        let key = board::key(&app.snapshot.all[&1]);

        assert_eq!(with(&mut app, KeyCode::Enter, KeyModifiers::ALT), None);
        assert_eq!(app.tabs.active(), &Doc::Item(key.clone()));
        assert!(!app.tabs.open[app.tabs.active].preview);

        ctrl(&mut app, 'w');
        assert_eq!(app.tabs.active(), &Doc::Board);
        assert_eq!(with(&mut app, KeyCode::Enter, KeyModifiers::CONTROL), None);
        assert_eq!(app.tabs.active(), &Doc::Item(key));

        ctrl(&mut app, 'w');
        assert_eq!(press(&mut app, KeyCode::Enter), Some(Action::Done));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Ctrl+K chords open the pages and the views; an unknown second key says so.
    #[test]
    fn ctrl_k_chords_open_pages_and_views() {
        let (dir, ekko) = board("chords");
        let mut app = open(&dir, &ekko);

        ctrl(&mut app, 'k');
        assert!(app.chording());
        press(&mut app, KeyCode::Char('r'));
        assert_eq!((app.tabs.active(), app.focus), (&Doc::Roadmap, Region::Editor));

        ctrl(&mut app, 'k');
        ctrl(&mut app, 'f');
        assert_eq!((app.view, app.focus), (View::Search, Region::Sidebar));

        ctrl(&mut app, 'k');
        press(&mut app, KeyCode::Char('z'));
        assert!(app.flash().unwrap().0.contains("(Ctrl+K, Z) is not a command"));
        assert!(!app.chording());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ctrl_w_closes_a_tab_but_never_the_board() {
        let (dir, ekko) = board("close");
        ekko.create_task(&words(&["@a", "one"])).unwrap();
        let mut app = open(&dir, &ekko);
        app.tabs.open(Doc::Calendar, false);

        ctrl(&mut app, 'w');
        assert_eq!(app.tabs.open.len(), 1);
        ctrl(&mut app, 'w');
        assert_eq!(app.tabs.open.len(), 1);
        assert_eq!(app.flash().unwrap().1, Kind::Refused);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A click lands on whatever the last frame recorded under it, the most
    /// specific thing first, and a second click on the same place opens.
    #[test]
    fn a_click_lands_on_what_was_drawn_and_a_double_click_opens() {
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

        click(&mut app, 10, 6);
        assert_eq!((app.focus, app.board.selected), (Region::Editor, 2));
        assert_eq!(app.tabs.open.len(), 1, "a single click opened a tab");
        click(&mut app, 10, 6);
        assert_eq!(app.tabs.active(), &Doc::Item(board::key(&app.snapshot.all[&2])));

        click(&mut app, 99, 39);
        assert_eq!((app.panel, app.unseen, app.focus), (PanelTab::Output, 0, Region::Panel));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// In the explorer Space opens an item in passing and Enter on purpose.
    #[test]
    fn the_explorer_previews_with_space_and_opens_with_enter() {
        let (dir, ekko) = board("explorer");
        ekko.create_task(&words(&["@a", "one"])).unwrap();
        ekko.create_task(&words(&["@a", "two"])).unwrap();
        let mut app = open(&dir, &ekko);
        app.focus = Region::Sidebar;
        app.tree.toggle(&tree::Node::Board("@a".to_string()));
        let rows = app.tree_rows();
        let one = rows.iter().position(|row| row.label == "one").unwrap();
        app.tree.select(one, &rows);

        press(&mut app, KeyCode::Char(' '));
        assert!(app.tabs.open[app.tabs.active].preview);
        assert_eq!(app.focus, Region::Sidebar, "a preview took the focus");
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        // As in VS Code, opening on purpose leaves the preview beside it; only
        // another preview takes a preview's place.
        assert_eq!(app.tabs.open.len(), 3);
        assert!(app.tabs.open.iter().any(|tab| tab.preview), "the preview was closed");
        assert!(!app.tabs.open[app.tabs.active].preview);
        assert_eq!(app.focus, Region::Editor);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn search_finds_through_the_core_and_opens_what_it_finds() {
        let (dir, ekko) = board("finding");
        ekko.create_task(&words(&["@a", "wire the watcher"])).unwrap();
        ekko.create_task(&words(&["@a", "draw the frame"])).unwrap();
        ekko.check_tasks(&words(&["2"]), false).unwrap();
        let mut app = open(&dir, &ekko);
        app.show_view(View::Search);

        for c in "is:done".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        let lines = app.search_lines();
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert_eq!(app.found.selected, 1, "the first result is not selected");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.tabs.active(), &Doc::Item(board::key(&app.snapshot.all[&2])));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Only a project that lives in its folder can be switched to.
    #[test]
    fn switching_is_asked_for_only_a_project_in_its_folder() {
        let (dir, ekko) = board("projects");
        let mut app = open(&dir, &ekko);
        let summary = |name: &str, status: &'static str| ProjectSummary {
            name: name.to_string(),
            complete: 0,
            tasks: 0,
            notes: 0,
            path: Some("/somewhere".to_string()),
            status,
        };
        app.snapshot.projects = vec![summary("elsewhere", "here"), summary("gone", "missing"), summary("test", "here")];
        app.show_view(View::Projects);

        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert!(app.switch.is_none());
        assert!(app.flash().unwrap().0.contains("no longer holds it"));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert!(app.flash().unwrap().0.contains("already open"));
        app.projects.selected = 0;
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.switch.as_deref(), Some("elsewhere"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn calendar_keys_move_through_days_and_months() {
        let (dir, ekko) = board("calendar");
        let mut app = open(&dir, &ekko);
        app.tabs.open(Doc::Calendar, false);
        app.focus = Region::Editor;
        app.doc.month = (2026, 1);
        app.doc.day = 31;

        press(&mut app, KeyCode::Right);
        assert_eq!((app.doc.month, app.doc.day), ((2026, 2), 1));
        press(&mut app, KeyCode::Up);
        assert_eq!((app.doc.month, app.doc.day), ((2026, 1), 25));
        app.doc.day = 31;
        press(&mut app, KeyCode::PageDown);
        assert_eq!((app.doc.month, app.doc.day), ((2026, 2), 28), "February kept a 31st");
        press(&mut app, KeyCode::PageUp);
        press(&mut app, KeyCode::PageUp);
        assert_eq!(app.doc.month, (2025, 12));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Enter opens a phase onto its items and opens an item from there.
    #[test]
    fn the_roadmap_opens_a_phase_onto_its_items() {
        let (dir, ekko) = board("roadmap");
        ekko.set_phases(&words(&["setup", "release"])).unwrap();
        ekko.create_task_in(&words(&["@a", "wire it"]), Some("release")).unwrap();
        let mut app = open(&dir, &ekko);
        app.tabs.open(Doc::Roadmap, false);
        app.focus = Region::Editor;
        app.doc.open_phases.clear();

        assert_eq!(app.roadmap_lines().len(), 2);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.roadmap_lines(), vec![RoadmapLine::Phase(0), RoadmapLine::Phase(1), RoadmapLine::Item(1)]);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.tabs.active(), &Doc::Item(board::key(&app.snapshot.all[&1])));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The graph opens from its chord and its settings from `Ctrl+K ,`; a
    /// toggle flipped in the panel is saved beside the board, and Escape
    /// closes the panel.
    #[test]
    fn the_graph_panel_changes_settings_and_saves_them() {
        let (dir, ekko) = board("panel");
        ekko.create_task(&words(&["@a", "one"])).unwrap();
        let mut app = open(&dir, &ekko);

        ctrl(&mut app, 'k');
        press(&mut app, KeyCode::Char('g'));
        assert_eq!(app.tabs.active(), &Doc::Graph);
        ctrl(&mut app, 'k');
        press(&mut app, KeyCode::Char(','));
        assert!(app.graph_panel.open);

        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert!(app.settings.tags, "Enter on Tags did not flip it");
        assert!(Settings::load(&dir).tags, "the change was not saved");

        press(&mut app, KeyCode::Up);
        press(&mut app, KeyCode::Enter);
        for c in "one".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.settings.search, "one");
        press(&mut app, KeyCode::Esc);
        assert!(!app.graph_panel.open);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A click on a node opens its item, and the local graph's depth keys stay
    /// inside Obsidian's range.
    #[test]
    fn a_click_on_a_node_opens_its_item_and_depth_stays_in_range() {
        let (dir, ekko) = board("node");
        ekko.create_task(&words(&["@a", "only"])).unwrap();
        let mut app = open(&dir, &ekko);
        app.tabs.open(Doc::Graph, false);
        let area = Rect::new(0, 0, 60, 20);
        app.graph.area = area;
        app.graph.refresh(&app.snapshot, &app.settings, app.generation, None);
        app.graph.frame();
        app.hits = vec![(area, Target::Canvas(Pane::Global))];
        let (x, y) = app.graph.point(0);
        let (column, row) = ((x / 2.0) as u16, (y / 4.0) as u16);

        click(&mut app, column, row);
        pointer(&mut app, MouseEventKind::Up(MouseButton::Left), column, row);
        assert_eq!(app.tabs.active(), &Doc::Item(board::key(&app.snapshot.all[&1])));

        app.show_view(View::Graph);
        for _ in 0..9 {
            press(&mut app, KeyCode::Char(']'));
        }
        assert_eq!(app.settings.depth, 5);
        for _ in 0..9 {
            press(&mut app, KeyCode::Char('['));
        }
        assert_eq!(app.settings.depth, 1);
        std::fs::remove_dir_all(&dir).ok();
    }
}
