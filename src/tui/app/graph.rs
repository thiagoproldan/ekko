//! What keys and the pointer do in a graph -- the whole board's, in its tab,
//! and the local one in the sidebar -- and in the graph's settings panel.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;

use super::{App, Kind};
use crate::tui::graph::model::Kind as NodeKind;
use crate::tui::graph::settings::{controls, Control, Group, Settings, Slider};
use crate::tui::graph::{Editing, GraphView, Pane, Tool};
use crate::tui::layout::Region;
use crate::tui::tabs::Doc;
use crate::tui::theme::palette;

/// Whether a slider is one of the forces, which move the layout when changed.
fn is_force(slider: Slider) -> bool {
    matches!(slider, Slider::Center | Slider::Repel | Slider::LinkForce | Slider::LinkDistance)
}

impl App {
    /// Runs `act` on a pane's view with the settings beside it.
    pub(super) fn with_view<R>(&mut self, pane: Pane, act: impl FnOnce(&mut GraphView, &Settings) -> R) -> R {
        match pane {
            Pane::Global => act(&mut self.graph, &self.settings),
            Pane::Local => act(&mut self.local, &self.settings),
        }
    }

    fn graph_shown(&self) -> bool {
        self.tabs.active() == &Doc::Graph
    }

    fn local_shown(&self) -> bool {
        self.shown.sidebar && self.view == super::View::Graph
    }

    /// Whether a graph on screen is still moving, so the frames keep coming.
    pub fn animating(&self) -> bool {
        (self.graph_shown() && self.graph.moving()) || (self.local_shown() && self.local.moving())
    }

    /// A frame's worth of movement for every graph on screen.
    pub fn animate(&mut self) {
        if self.graph_shown() {
            self.graph.animate(&self.settings);
        }
        if self.local_shown() {
            self.local.animate(&self.settings);
        }
    }

    /// The settings changed: they are saved beside the board, and a change to
    /// a force sets the layout moving again.
    pub(super) fn settings_changed(&mut self, forces: bool) {
        if let Err(error) = self.settings.save(&self.workspace.dir) {
            self.say(format!("The graph's settings could not be saved: {error}"), Kind::Refused);
        }
        if forces {
            self.graph.sim.reheat(0.3);
            self.local.sim.reheat(0.3);
        }
    }

    /// Opens what a node stands for: an item's tab, or a board on the board
    /// tab. An unresolved node has nothing to open, and says why.
    pub(super) fn open_node(&mut self, pane: Pane, index: usize, preview: bool) {
        let node = match pane {
            Pane::Global => self.graph.graph.nodes.get(index).cloned(),
            Pane::Local => self.local.graph.nodes.get(index).cloned(),
        };
        let Some(node) = node else { return };
        match node.kind {
            NodeKind::Item { .. } => self.open_item(node.key, preview),
            NodeKind::Board => {
                self.tabs.open(Doc::Board, false);
                self.set_filter(node.key);
                self.focus = Region::Editor;
            }
            NodeKind::Unresolved => self.say(
                format!("\u{201c}{}\u{201d} is not on the board: it is stashed, in the trash, or gone", node.label),
                Kind::Refused,
            ),
        }
    }

    pub(super) fn graph_key(&mut self, key: KeyEvent) {
        if self.graph_panel.open {
            self.settings_key(key);
        } else {
            self.canvas_key(Pane::Global, key);
        }
    }

    /// The local graph takes the canvas keys, and `[` and `]` for its depth.
    pub(super) fn local_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('[') => self.tool(Pane::Local, Tool::Shallower),
            KeyCode::Char(']') => self.tool(Pane::Local, Tool::Deeper),
            _ => self.canvas_key(Pane::Local, key),
        }
    }

    /// Obsidian's keys for a graph: the arrows move it, faster with Shift,
    /// and `+` and `-` zoom. Beside them, Ctrl and an arrow moves between
    /// nodes, Enter opens the one the keyboard is on, and `0` frames it all.
    fn canvas_key(&mut self, pane: Pane, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let fast = if key.modifiers.contains(KeyModifiers::SHIFT) { 3.0 } else { 1.0 };
        let direction = match key.code {
            KeyCode::Left => Some((-1.0, 0.0)),
            KeyCode::Right => Some((1.0, 0.0)),
            KeyCode::Up => Some((0.0, -1.0)),
            KeyCode::Down => Some((0.0, 1.0)),
            _ => None,
        };
        if let Some((across, down)) = direction {
            if ctrl {
                self.with_view(pane, |view, _| view.select_toward(across, down));
            } else {
                self.with_view(pane, |view, _| view.pan_by(across * fast, down * fast));
            }
            return;
        }
        match key.code {
            KeyCode::Char('+' | '=') => self.tool(pane, Tool::ZoomIn),
            KeyCode::Char('-' | '_') => self.tool(pane, Tool::ZoomOut),
            KeyCode::Char('0') => self.tool(pane, Tool::Fit),
            KeyCode::Esc => self.with_view(pane, |view, _| view.selected = None),
            KeyCode::Enter => {
                let selected = self.with_view(pane, |view, _| view.selected);
                if let Some(index) = selected {
                    self.open_node(pane, index, false);
                }
            }
            _ => {}
        }
    }

    /// A button over a graph, clicked or keyed.
    pub(super) fn tool(&mut self, pane: Pane, tool: Tool) {
        match tool {
            Tool::ZoomIn => self.with_view(pane, |view, _| view.zoom_by(1.25)),
            Tool::ZoomOut => self.with_view(pane, |view, _| view.zoom_by(0.8)),
            Tool::Fit => self.with_view(pane, |view, _| view.fit()),
            Tool::Settings => {
                self.graph_panel.open = !self.graph_panel.open;
                self.graph_panel.editing = None;
                self.focus = Region::Editor;
            }
            Tool::Deeper | Tool::Shallower => {
                let by = if tool == Tool::Deeper { 1.0 } else { -1.0 };
                Slider::Depth.nudge(&mut self.settings, by);
                self.settings_changed(false);
            }
        }
    }

    /// The settings panel's keys: the arrows move between rows and set a
    /// slider or a group's colour, Enter uses a row, Delete removes a group,
    /// and Escape closes the panel.
    fn settings_key(&mut self, key: KeyEvent) {
        if let Some(editing) = self.graph_panel.editing {
            self.edit_key(editing, key);
            return;
        }
        let rows = controls(&self.settings, &self.graph_panel.sections, false);
        let row = rows.get(self.graph_panel.cursor.selected).copied();
        let steps = if key.modifiers.contains(KeyModifiers::SHIFT) { 5.0 } else { 1.0 };
        match (key.code, row) {
            (KeyCode::Esc, _) => self.graph_panel.open = false,
            (KeyCode::Up, _) => self.graph_panel.cursor.step(-1, rows.len()),
            (KeyCode::Down, _) => self.graph_panel.cursor.step(1, rows.len()),
            (KeyCode::Left | KeyCode::Right, Some(Control::Slider(slider))) => {
                slider.nudge(&mut self.settings, if key.code == KeyCode::Left { -steps } else { steps });
                self.settings_changed(is_force(slider));
            }
            (KeyCode::Left | KeyCode::Right, Some(Control::Group(index))) => {
                if let Some(group) = self.settings.groups.get_mut(index) {
                    let count = palette::GROUPS.len();
                    group.colour = if key.code == KeyCode::Left { (group.colour + count - 1) % count } else { (group.colour + 1) % count };
                    self.settings_changed(false);
                }
            }
            (KeyCode::Right, Some(Control::Section(section))) => {
                self.graph_panel.sections.insert(section);
            }
            (KeyCode::Left, Some(Control::Section(section))) => {
                self.graph_panel.sections.remove(&section);
            }
            (KeyCode::Delete | KeyCode::Backspace, Some(Control::Group(index))) => {
                self.settings.groups.remove(index);
                self.settings_changed(false);
            }
            (KeyCode::Enter | KeyCode::Char(' '), Some(control)) => self.use_control(control),
            _ => {}
        }
    }

    /// What using a row does: a section opens or closes, a toggle flips, a
    /// text row takes typing, and a button does what it says.
    pub(super) fn use_control(&mut self, control: Control) {
        match control {
            Control::Section(section) => {
                if !self.graph_panel.sections.remove(&section) {
                    self.graph_panel.sections.insert(section);
                }
            }
            Control::Search => self.graph_panel.editing = Some(Editing::Search),
            Control::Toggle(toggle) => {
                toggle.flip(&mut self.settings);
                self.settings_changed(false);
            }
            Control::Group(index) => self.graph_panel.editing = Some(Editing::Group(index)),
            Control::NewGroup => {
                let colour = self.settings.groups.len() % palette::GROUPS.len();
                self.settings.groups.push(Group { query: String::new(), colour });
                self.graph_panel.editing = Some(Editing::Group(self.settings.groups.len() - 1));
                self.settings_changed(false);
            }
            Control::Slider(_) => {}
            Control::Animate => {
                self.graph.start_timelapse();
                self.graph_panel.open = false;
            }
            Control::Restore => {
                self.settings = Settings::default();
                self.graph_panel.editing = None;
                self.settings_changed(true);
            }
        }
    }

    /// Typing into the search field or a group's query; Enter or Escape stops.
    fn edit_key(&mut self, editing: Editing, key: KeyEvent) {
        let typing = !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        let text = match editing {
            Editing::Search => &mut self.settings.search,
            Editing::Group(index) => match self.settings.groups.get_mut(index) {
                Some(group) => &mut group.query,
                None => {
                    self.graph_panel.editing = None;
                    return;
                }
            },
        };
        match key.code {
            KeyCode::Enter | KeyCode::Esc => {
                self.graph_panel.editing = None;
                return;
            }
            KeyCode::Backspace => {
                text.pop();
            }
            KeyCode::Char(c) if typing => text.push(c),
            _ => return,
        }
        self.settings_changed(false);
    }

    /// A click on a slider's track sets it to where the click landed.
    pub(super) fn slide(&mut self, row: usize, track: Rect, column: u16) {
        let rows = controls(&self.settings, &self.graph_panel.sections, false);
        if let Some(Control::Slider(slider)) = rows.get(row).copied() {
            let (least, most, step) = slider.range();
            let share = f64::from(column.saturating_sub(track.x)) / f64::from(track.width.saturating_sub(1).max(1));
            let value = least + share.clamp(0.0, 1.0) * (most - least);
            slider.set(&mut self.settings, (value / step).round() * step);
            self.graph_panel.cursor.selected = row;
            self.settings_changed(is_force(slider));
        }
    }
}
