//! The graph view: Obsidian's graph, in a terminal.
//!
//! Items are nodes and the relations between them are links (`model`); a force
//! simulation lays them out (`sim`); a camera frames them (`camera`); braille
//! dots draw them (`raster`); and the settings are the ones Obsidian's graph
//! has (`settings`). A `GraphView` holds one of each for a pane and answers
//! the pointer and the keys; the frame draws it.

pub mod camera;
pub mod model;
pub mod raster;
pub mod settings;
pub mod sim;

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use ratatui::layout::Rect;

use self::camera::Camera;
use self::model::Graph;
use self::settings::{Group, Section, Settings};
use self::sim::Sim;
use super::board::Snapshot;
use super::list::Cursor;

/// Steps of the layout per frame while it moves.
const STEPS_PER_FRAME: usize = 2;
/// Steps taken at once when a graph is laid out from nothing, so it opens
/// already taking shape rather than as a burst out of the middle.
const WARM_START: usize = 60;
/// How long a time-lapse takes to bring every node in.
const TIMELAPSE: Duration = Duration::from_secs(8);
/// How far the pointer moves, in dots, before a press on a node is a drag.
const DRAG_THRESHOLD: f64 = 2.0;
/// The scale a default graph is framed at, which node sizes are relative to.
const REFERENCE_SCALE: f64 = 0.05;

/// Which graph: the whole board's in the editor, or the local one beside it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    Global,
    Local,
}

/// The buttons drawn over a graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    ZoomIn,
    ZoomOut,
    Fit,
    Settings,
    Deeper,
    Shallower,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Drag {
    /// A press on a node: held under the pointer once it moves.
    Node { index: usize, moved: bool, from: (f64, f64) },
    /// A press on empty space: the view follows the pointer.
    View { last: (f64, f64) },
}

/// The settings a graph is built from, as opposed to drawn or moved by.
#[derive(Clone, Debug, PartialEq)]
struct Filters {
    search: String,
    tags: bool,
    existing_only: bool,
    orphans: bool,
    groups: Vec<Group>,
}

impl Filters {
    fn of(settings: &Settings) -> Filters {
        Filters {
            search: settings.search.clone(),
            tags: settings.tags,
            existing_only: settings.existing_only,
            orphans: settings.orphans,
            groups: settings.groups.clone(),
        }
    }
}

/// What a graph was last built from.
#[derive(Clone, Debug, PartialEq)]
struct Built {
    generation: u64,
    filters: Filters,
    centre: Option<(String, usize)>,
}

pub struct GraphView {
    pub graph: Graph,
    pub sim: Sim,
    pub camera: Camera,
    /// The node under the pointer.
    pub hover: Option<usize>,
    /// The node the keyboard is on.
    pub selected: Option<usize>,
    /// Where the view was last drawn, in cells.
    pub area: Rect,
    drag: Option<Drag>,
    built: Option<Built>,
    /// When a time-lapse began, and the order its nodes come in.
    timelapse: Option<(Instant, Vec<usize>)>,
}

impl Default for GraphView {
    fn default() -> Self {
        let graph = Graph::default();
        let sim = Sim::arrange(&graph, &HashMap::new());
        GraphView {
            graph,
            sim,
            camera: Camera::default(),
            hover: None,
            selected: None,
            area: Rect::default(),
            drag: None,
            built: None,
            timelapse: None,
        }
    }
}

impl GraphView {
    /// Builds the graph again when the board, the filters or the centre
    /// changed since it was last built -- and only then -- keeping every
    /// node's place, and the keyboard on the node it was on.
    pub fn refresh(&mut self, snapshot: &Snapshot, settings: &Settings, generation: u64, centre: Option<(String, usize)>) {
        let wanted = Built { generation, filters: Filters::of(settings), centre };
        if self.built.as_ref() == Some(&wanted) {
            return;
        }
        let places = self.sim.places(&self.graph);
        let selected = self.selected.and_then(|at| self.graph.nodes.get(at)).map(|node| node.key.clone());
        let links = |graph: &Graph| -> HashSet<(String, String)> {
            graph.edges.iter().map(|edge| (graph.nodes[edge.from].key.clone(), graph.nodes[edge.to].key.clone())).collect()
        };
        let before = links(&self.graph);

        let local = wanted.centre.as_ref().map(|(key, depth)| (key.as_str(), *depth));
        let graph = model::build(snapshot, settings, local);
        let mut sim = Sim::arrange(&graph, &places);
        if sim.alpha >= 1.0 {
            for _ in 0..WARM_START {
                sim.tick(&graph, settings);
            }
        } else if links(&graph) != before {
            sim.reheat(0.3);
        }

        self.selected = selected.and_then(|key| graph.index(&key));
        self.hover = None;
        self.drag = None;
        if self.timelapse.is_some() {
            self.timelapse = Some((Instant::now(), creation_order(&graph)));
        }
        self.graph = graph;
        self.sim = sim;
        self.built = Some(wanted);
    }

    /// Whether anything is still moving, so the frame keeps being drawn.
    pub fn moving(&self) -> bool {
        self.sim.running() || self.timelapse.is_some()
    }

    /// Advances the layout by a frame's worth of steps.
    pub fn animate(&mut self, settings: &Settings) {
        if self.timelapse.as_ref().is_some_and(|(started, _)| started.elapsed() >= TIMELAPSE) {
            self.timelapse = None;
        }
        if self.sim.running() {
            for _ in 0..STEPS_PER_FRAME {
                self.sim.tick(&self.graph, settings);
            }
        }
    }

    /// Starts Obsidian's time-lapse: the nodes come in the order their items
    /// were created.
    pub fn start_timelapse(&mut self) {
        self.timelapse = Some((Instant::now(), creation_order(&self.graph)));
        self.sim.reheat(0.3);
    }

    /// The nodes the time-lapse has brought in so far, or `None` when every
    /// node is shown.
    pub fn born(&self) -> Option<HashSet<usize>> {
        let (started, order) = self.timelapse.as_ref()?;
        let share = started.elapsed().as_secs_f64() / TIMELAPSE.as_secs_f64();
        let shown = ((share * order.len() as f64).ceil() as usize).max(1);
        Some(order.iter().take(shown).copied().collect())
    }

    /// The view's width and height in dots.
    pub fn size(&self) -> (f64, f64) {
        (f64::from(self.area.width) * 2.0, f64::from(self.area.height) * 4.0)
    }

    /// The middle of a cell, in the view's dots.
    fn dots(&self, cell: (u16, u16)) -> (f64, f64) {
        (
            f64::from(cell.0.saturating_sub(self.area.x)) * 2.0 + 1.0,
            f64::from(cell.1.saturating_sub(self.area.y)) * 4.0 + 2.0,
        )
    }

    /// Where a node is drawn, in the view's dots.
    pub fn point(&self, index: usize) -> (f64, f64) {
        let body = self.sim.bodies[index];
        self.camera.to_view((body.x, body.y), self.size())
    }

    /// A node's radius in dots: bigger for more links pointing at it, and
    /// growing a little as the view zooms in.
    pub fn radius(&self, index: usize, settings: &Settings) -> f64 {
        let weight = self.graph.nodes[index].weight as f64;
        let zoom = (self.camera.scale / REFERENCE_SCALE).sqrt().clamp(0.6, 3.0);
        (settings.node_size * (1.1 + 0.45 * weight.sqrt()) * zoom).clamp(0.6, 9.0)
    }

    /// The node under a point of the view, if any: the nearest whose drawn
    /// circle, with a little slack, holds it.
    fn node_at(&self, at: (f64, f64), settings: &Settings) -> Option<usize> {
        let born = self.born();
        (0..self.graph.nodes.len())
            .filter(|index| born.as_ref().is_none_or(|born| born.contains(index)))
            .filter_map(|index| {
                let point = self.point(index);
                let distance = (point.0 - at.0).hypot(point.1 - at.1);
                (distance <= self.radius(index, settings) + 2.5).then_some((distance, index))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, index)| index)
    }

    /// Frames the whole graph from now on, until it is moved by hand again.
    pub fn fit(&mut self) {
        self.camera.fitting = true;
    }

    /// Keeps the camera on the whole graph while it is framing it.
    pub fn frame(&mut self) {
        if self.camera.fitting {
            let size = self.size();
            let points: Vec<(f64, f64)> = self.sim.bodies.iter().map(|body| (body.x, body.y)).collect();
            self.camera.fit(points.into_iter(), size);
        }
    }

    /// Moves the view by a tenth of its size in each direction asked, as the
    /// arrow keys do in Obsidian.
    pub fn pan_by(&mut self, across: f64, down: f64) {
        let size = self.size();
        self.camera.pan((-across * size.0 * 0.1, -down * size.1 * 0.1));
    }

    pub fn zoom_by(&mut self, factor: f64) {
        let size = self.size();
        self.camera.zoom_at(factor, (size.0 / 2.0, size.1 / 2.0), size);
    }

    /// Moves the keyboard to the nearest node lying in the direction asked:
    /// the less off to the side, the nearer it counts.
    pub fn select_toward(&mut self, across: f64, down: f64) {
        if self.graph.nodes.is_empty() {
            return;
        }
        let size = self.size();
        let from = self.selected.map_or((size.0 / 2.0, size.1 / 2.0), |index| self.point(index));
        let best = (0..self.graph.nodes.len())
            .filter(|index| Some(*index) != self.selected)
            .filter_map(|index| {
                let point = self.point(index);
                let (dx, dy) = (point.0 - from.0, point.1 - from.1);
                let along = dx * across + dy * down;
                (along > 0.0).then(|| (along + 2.0 * (dx * down - dy * across).abs(), index))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, index)| index);
        self.selected = best.or(self.selected).or_else(|| {
            (0..self.graph.nodes.len()).max_by_key(|index| self.graph.nodes[*index].weight)
        });
    }

    /// The pointer moved over the view.
    pub fn hover_at(&mut self, cell: (u16, u16), settings: &Settings) {
        self.hover = self.node_at(self.dots(cell), settings);
    }

    /// A press: on a node, the start of a click or a drag; elsewhere, the
    /// start of dragging the view.
    pub fn press(&mut self, cell: (u16, u16), settings: &Settings) {
        let at = self.dots(cell);
        self.drag = Some(match self.node_at(at, settings) {
            Some(index) => Drag::Node { index, moved: false, from: at },
            None => Drag::View { last: at },
        });
    }

    /// The pointer moved with the button down.
    pub fn drag_to(&mut self, cell: (u16, u16)) {
        let at = self.dots(cell);
        match self.drag {
            Some(Drag::Node { index, moved, from }) => {
                let moved = moved || (at.0 - from.0).hypot(at.1 - from.1) >= DRAG_THRESHOLD;
                if moved {
                    let (x, y) = self.camera.to_layout(at, self.size());
                    self.sim.held = Some((index, x, y));
                    self.sim.target = 0.3;
                    self.sim.reheat(0.3);
                    self.camera.fitting = false;
                }
                self.drag = Some(Drag::Node { index, moved, from });
            }
            Some(Drag::View { last }) => {
                self.camera.pan((at.0 - last.0, at.1 - last.1));
                self.drag = Some(Drag::View { last: at });
            }
            None => {}
        }
    }

    /// The button was let go. Returns the node pressed and let go without
    /// being dragged: a click on it.
    pub fn release(&mut self) -> Option<usize> {
        let clicked = match self.drag.take() {
            Some(Drag::Node { index, moved: false, .. }) => Some(index),
            _ => None,
        };
        if self.sim.held.take().is_some() {
            self.sim.target = 0.0;
        }
        clicked
    }

    /// The wheel zooms around the pointer, in on the way up.
    pub fn wheel(&mut self, cell: (u16, u16), by: isize) {
        let at = self.dots(cell);
        let factor = if by < 0 { 1.25 } else { 0.8 };
        self.camera.zoom_at(factor, at, self.size());
    }
}

/// Nodes in the order their items were created, oldest first.
fn creation_order(graph: &Graph) -> Vec<usize> {
    let mut order: Vec<usize> = (0..graph.nodes.len()).collect();
    order.sort_by_key(|&index| (graph.nodes[index].created, index));
    order
}

/// What the settings panel is doing: which sections are open, the row the
/// keyboard is on, and the text being typed, if any.
#[derive(Default)]
pub struct Panel {
    pub open: bool,
    pub sections: HashSet<Section>,
    pub cursor: Cursor,
    pub editing: Option<Editing>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Editing {
    Search,
    Group(usize),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::board::tests::{board, snapshot, words};

    fn view(area: Rect) -> GraphView {
        GraphView { area, ..GraphView::default() }
    }

    /// A graph is built again only when what it is built from changes, and
    /// the keyboard stays on its node across a rebuild.
    #[test]
    fn a_graph_rebuilds_only_when_its_inputs_change() {
        let (dir, ekko) = board("view");
        ekko.create_task(&words(&["@a", "base"])).unwrap();
        ekko.create_task(&words(&["@a", "on top"])).unwrap();
        ekko.set_blocked_by(&words(&["@2", "1"])).unwrap();
        let snapshot = snapshot(&dir, &ekko);
        let settings = Settings::default();
        let mut view = view(Rect::new(0, 0, 60, 20));

        view.refresh(&snapshot, &settings, 1, None);
        assert_eq!(view.graph.nodes.len(), 2);
        view.selected = Some(1);
        let key = view.graph.nodes[1].key.clone();
        view.sim.bodies[0].x = 12345.0;
        view.refresh(&snapshot, &settings, 1, None);
        assert_eq!(view.sim.bodies[0].x, 12345.0, "an unchanged graph was laid out again");

        let tagged = Settings { tags: true, ..Settings::default() };
        view.refresh(&snapshot, &tagged, 1, None);
        assert_eq!(view.graph.nodes.len(), 3);
        assert_eq!(view.selected.map(|at| view.graph.nodes[at].key.clone()), Some(key));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A press and release on a node is a click; a press that moves drags the
    /// node, which the layout then holds under the pointer.
    #[test]
    fn a_press_on_a_node_is_a_click_until_it_moves() {
        let (dir, ekko) = board("pointer");
        ekko.create_task(&words(&["@a", "only"])).unwrap();
        let snapshot = snapshot(&dir, &ekko);
        let settings = Settings::default();
        let mut view = view(Rect::new(0, 0, 40, 20));
        view.refresh(&snapshot, &settings, 1, None);
        view.frame();
        let (x, y) = view.point(0);
        let cell = ((x / 2.0) as u16, (y / 4.0) as u16);

        view.hover_at(cell, &settings);
        assert_eq!(view.hover, Some(0));
        view.press(cell, &settings);
        assert_eq!(view.release(), Some(0), "a click on a node was not a click");

        view.press(cell, &settings);
        view.drag_to((cell.0 + 6, cell.1 + 2));
        assert!(view.sim.held.is_some() && view.sim.running());
        assert_eq!(view.release(), None, "a drag was taken for a click");
        assert!(view.sim.held.is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Dragging empty space moves the view, and the wheel zooms.
    #[test]
    fn dragging_space_pans_and_the_wheel_zooms() {
        let mut view = view(Rect::new(10, 5, 40, 20));
        let before = view.camera;
        view.press((20, 10), &Settings::default());
        view.drag_to((25, 10));
        assert!(view.camera.x < before.x, "the view did not follow the pointer");
        view.release();
        let scale = view.camera.scale;
        view.wheel((20, 10), -1);
        assert!(view.camera.scale > scale);
        assert!(!view.camera.fitting);
    }

    /// Ctrl and an arrow move the keyboard to the node that way.
    #[test]
    fn the_keyboard_moves_toward_the_node_in_its_direction() {
        let (dir, ekko) = board("toward");
        for description in ["left", "middle", "right"] {
            ekko.create_task(&words(&["@a", description])).unwrap();
        }
        let snapshot = snapshot(&dir, &ekko);
        let mut view = view(Rect::new(0, 0, 60, 20));
        view.refresh(&snapshot, &Settings::default(), 1, None);
        for (at, x) in [(0, -300.0), (1, 0.0), (2, 300.0)] {
            view.sim.bodies[at] = sim::Body { x, y: 0.0, vx: 0.0, vy: 0.0 };
        }
        view.frame();
        view.selected = Some(1);
        view.select_toward(1.0, 0.0);
        assert_eq!(view.selected, Some(2));
        view.select_toward(-1.0, 0.0);
        assert_eq!(view.selected, Some(1));
        std::fs::remove_dir_all(&dir).ok();
    }
}
