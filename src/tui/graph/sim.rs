//! The forces that lay a graph out, computed the way d3-force computes them
//! -- the model graph views of Obsidian's kind are built on. Every link pulls
//! its two ends toward the link distance, every node pushes every other away,
//! a pull toward the middle keeps the whole together, and a cooling `alpha`
//! lets it all come to rest.

use std::collections::HashMap;

use super::model::Graph;
use super::settings::Settings;

/// How fast the layout cools, as d3's default: to rest in about 300 steps.
const ALPHA_DECAY: f64 = 0.0228;
/// Below this the layout is at rest and nothing moves.
pub const ALPHA_MIN: f64 = 0.001;
/// How much of its speed a node keeps from one step to the next.
const VELOCITY_KEEP: f64 = 0.6;
/// The settings' repel force, per unit of link distance: at the defaults a
/// linked pair comes to rest a little past the length of its link.
const REPEL_SCALE: f64 = 1.4;
/// The settings' center force, in the units the layout pulls in: enough to
/// keep the unlinked nodes gathered round the linked ones instead of drifting.
const CENTER_SCALE: f64 = 0.2;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Body {
    pub x: f64,
    pub y: f64,
    pub vx: f64,
    pub vy: f64,
}

pub struct Sim {
    pub bodies: Vec<Body>,
    pub alpha: f64,
    /// Where alpha heads: zero to come to rest, higher while a node is dragged.
    pub target: f64,
    /// A node held where the pointer is while it is dragged.
    pub held: Option<(usize, f64, f64)>,
}

/// A number from a key, the same on every run, for choosing a direction.
fn spin(key: &str) -> f64 {
    let hash = key.bytes().fold(2_166_136_261u32, |hash, byte| (hash ^ u32::from(byte)).wrapping_mul(16_777_619));
    f64::from(hash) / f64::from(u32::MAX) * std::f64::consts::TAU
}

impl Sim {
    /// Lays `graph` out, keeping every node that had a place in `before`
    /// where it was. A new node starts beside a neighbour that already had a
    /// place, and otherwise on d3's phyllotaxis spiral.
    pub fn arrange(graph: &Graph, before: &HashMap<String, Body>) -> Sim {
        let count = graph.nodes.len();
        let mut bodies = vec![Body::default(); count];
        let mut placed = vec![false; count];
        for (at, node) in graph.nodes.iter().enumerate() {
            if let Some(body) = before.get(&node.key) {
                bodies[at] = *body;
                placed[at] = true;
            }
        }
        let fresh = placed.iter().filter(|placed| !**placed).count();
        for at in 0..count {
            if placed[at] {
                continue;
            }
            let neighbour = graph.neighbours[at].iter().find(|&&next| placed[next]).copied();
            bodies[at] = match neighbour {
                Some(next) => {
                    let angle = spin(&graph.nodes[at].key);
                    Body { x: bodies[next].x + 40.0 * angle.cos(), y: bodies[next].y + 40.0 * angle.sin(), vx: 0.0, vy: 0.0 }
                }
                None => {
                    let index = at as f64;
                    let radius = 40.0 * (0.5 + index).sqrt();
                    let angle = index * std::f64::consts::PI * (3.0 - 5f64.sqrt());
                    Body { x: radius * angle.cos(), y: radius * angle.sin(), vx: 0.0, vy: 0.0 }
                }
            };
            placed[at] = true;
        }
        let alpha = if fresh == 0 {
            0.0
        } else if fresh == count {
            1.0
        } else {
            0.3
        };
        Sim { bodies, alpha, target: 0.0, held: None }
    }

    pub fn running(&self) -> bool {
        self.alpha >= ALPHA_MIN || self.target > 0.0
    }

    /// Warms the layout back up, as a change to the graph does.
    pub fn reheat(&mut self, alpha: f64) {
        self.alpha = self.alpha.max(alpha);
    }

    /// Where every node is, by key, to carry into the next arrangement.
    pub fn places(&self, graph: &Graph) -> HashMap<String, Body> {
        graph.nodes.iter().zip(&self.bodies).map(|(node, body)| (node.key.clone(), *body)).collect()
    }

    /// One step of every force, at the strengths `settings` gives them.
    pub fn tick(&mut self, graph: &Graph, settings: &Settings) {
        let count = self.bodies.len();
        if count != graph.nodes.len() {
            return;
        }
        self.alpha += (self.target - self.alpha) * ALPHA_DECAY;
        let alpha = self.alpha;

        // Links pull their ends toward the link distance, and the end with
        // fewer links moves more, as d3's link force moves them.
        let mut degree = vec![0usize; count];
        for edge in &graph.edges {
            degree[edge.from] += 1;
            degree[edge.to] += 1;
        }
        for edge in &graph.edges {
            let (from, to) = (edge.from, edge.to);
            if from == to {
                continue;
            }
            let (a, b) = (self.bodies[from], self.bodies[to]);
            let mut dx = b.x + b.vx - a.x - a.vx;
            let mut dy = b.y + b.vy - a.y - a.vy;
            if dx == 0.0 && dy == 0.0 {
                dx = spin(&graph.nodes[to].key).cos() * 1e-3;
                dy = spin(&graph.nodes[to].key).sin() * 1e-3;
            }
            let length = (dx * dx + dy * dy).sqrt();
            let strength = settings.link_force / degree[from].min(degree[to]).max(1) as f64;
            let pull = (length - settings.link_distance) / length * alpha * strength;
            let (px, py) = (dx * pull, dy * pull);
            let bias = degree[from] as f64 / (degree[from] + degree[to]) as f64;
            self.bodies[to].vx -= px * bias;
            self.bodies[to].vy -= py * bias;
            self.bodies[from].vx += px * (1.0 - bias);
            self.bodies[from].vy += py * (1.0 - bias);
        }

        // Every node pushes every other away, harder the closer they are.
        let strength = -settings.repel * REPEL_SCALE * settings.link_distance * alpha;
        for i in 0..count {
            for j in i + 1..count {
                let mut dx = self.bodies[j].x - self.bodies[i].x;
                let mut dy = self.bodies[j].y - self.bodies[i].y;
                if dx == 0.0 && dy == 0.0 {
                    let angle = spin(&graph.nodes[j].key);
                    dx = angle.cos() * 1e-3;
                    dy = angle.sin() * 1e-3;
                }
                let squared = (dx * dx + dy * dy).max(1.0);
                let push = strength / squared;
                self.bodies[i].vx += dx * push;
                self.bodies[i].vy += dy * push;
                self.bodies[j].vx -= dx * push;
                self.bodies[j].vy -= dy * push;
            }
        }

        // A pull toward the middle, stronger the farther out a node is.
        let pull = settings.center * CENTER_SCALE * alpha;
        let held = self.held;
        for (at, body) in self.bodies.iter_mut().enumerate() {
            if let Some((_, x, y)) = held.filter(|(index, _, _)| *index == at) {
                *body = Body { x, y, vx: 0.0, vy: 0.0 };
                continue;
            }
            body.vx -= body.x * pull;
            body.vy -= body.y * pull;
            body.vx *= VELOCITY_KEEP;
            body.vy *= VELOCITY_KEEP;
            body.x += body.vx;
            body.y += body.vy;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::graph::model::{Edge, Kind, Link, Node};

    fn node(key: &str) -> Node {
        Node { key: key.to_string(), kind: Kind::Board, label: key.to_string(), created: 0, weight: 0, group: None }
    }

    fn graph(keys: &[&str], links: &[(usize, usize)]) -> Graph {
        let nodes = keys.iter().map(|key| node(key)).collect();
        let edges = links.iter().map(|&(from, to)| Edge { from, to, link: Link::WaitsOn }).collect();
        Graph::with(nodes, edges)
    }

    fn distance(sim: &Sim, a: usize, b: usize) -> f64 {
        let (a, b) = (sim.bodies[a], sim.bodies[b]);
        ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
    }

    fn settle(sim: &mut Sim, graph: &Graph, settings: &Settings) {
        for _ in 0..400 {
            sim.tick(graph, settings);
        }
    }

    /// A pair of linked nodes comes to rest about the link distance apart,
    /// and a node linked to neither sits farther off than the pair's length.
    #[test]
    fn linked_nodes_settle_near_the_link_distance() {
        let graph = graph(&["a", "b", "c"], &[(0, 1)]);
        let settings = Settings::default();
        let mut sim = Sim::arrange(&graph, &HashMap::new());
        settle(&mut sim, &graph, &settings);

        let linked = distance(&sim, 0, 1);
        assert!((settings.link_distance * 0.8..settings.link_distance * 1.6).contains(&linked), "{linked}");
        assert!(!sim.running(), "the layout did not come to rest");
        assert!(sim.bodies.iter().all(|body| body.x.is_finite() && body.y.is_finite()));
    }

    /// Nodes that start on top of each other part, and never become NaN.
    #[test]
    fn nodes_on_top_of_each_other_part() {
        let graph = graph(&["a", "b", "c", "d"], &[]);
        let mut sim = Sim::arrange(&graph, &HashMap::new());
        for body in &mut sim.bodies {
            *body = Body::default();
        }
        sim.reheat(1.0);
        settle(&mut sim, &graph, &Settings::default());
        assert!(sim.bodies.iter().all(|body| body.x.is_finite() && body.y.is_finite()));
        assert!(distance(&sim, 0, 1) > 1.0, "coincident nodes stayed together");
    }

    /// A dragged node stays under the pointer while the rest moves around it.
    #[test]
    fn a_held_node_stays_where_it_is_held() {
        let graph = graph(&["a", "b"], &[(0, 1)]);
        let mut sim = Sim::arrange(&graph, &HashMap::new());
        sim.held = Some((0, 500.0, -300.0));
        sim.target = 0.3;
        settle(&mut sim, &graph, &Settings::default());
        assert_eq!((sim.bodies[0].x, sim.bodies[0].y), (500.0, -300.0));
        assert!(sim.running(), "a layout being dragged came to rest");
    }

    /// Rearranging keeps every known node where it was, and only a graph
    /// with new nodes warms up.
    #[test]
    fn rearranging_keeps_known_places_by_key() {
        let first = graph(&["a", "b"], &[(0, 1)]);
        let mut sim = Sim::arrange(&first, &HashMap::new());
        assert_eq!(sim.alpha, 1.0);
        settle(&mut sim, &first, &Settings::default());
        let places = sim.places(&first);

        let same = Sim::arrange(&first, &places);
        assert_eq!((same.alpha, same.bodies[1]), (0.0, sim.bodies[1]));

        let grown = graph(&["b", "c", "a"], &[(2, 0), (1, 0)]);
        let grown_sim = Sim::arrange(&grown, &places);
        assert_eq!(grown_sim.bodies[0], sim.bodies[1]);
        assert_eq!(grown_sim.alpha, 0.3);
        assert!(distance(&grown_sim, 1, 0) < 100.0, "a new node did not start beside its neighbour");
    }
}
