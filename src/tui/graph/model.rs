//! What a graph view draws: items as nodes, the relations between them as
//! links.
//!
//! Obsidian's graph shows notes and the links written between them; this
//! shows items and the relations the board records -- a task waiting on
//! another, a note attached to a task -- with boards standing in for tags, and
//! a relation to an item that is no longer on the board drawn the way Obsidian
//! draws a link to a note that does not exist: as an unresolved node.

use std::collections::{HashMap, HashSet, VecDeque};

use super::settings::Settings;
use crate::item::Item;
use crate::tui::board::{self, one_line, Snapshot};
use crate::tui::search;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Item { id: u32 },
    /// A board, shown the way Obsidian shows a tag.
    Board,
    /// A relation's other end that is not on the board: stashed, in the
    /// trash, or gone.
    Unresolved,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    /// What the node is across rebuilds, so it keeps its place: an item's
    /// key, a board's name, or `?` and the uid a relation names.
    pub key: String,
    pub kind: Kind,
    pub label: String,
    /// When it was created, for the time-lapse.
    pub created: i64,
    /// How many links point at it: the more, the bigger it is drawn.
    pub weight: usize,
    /// The first group it belongs to, by its place in the settings.
    pub group: Option<usize>,
}

/// What a link stands for, pointing from the item that records it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Link {
    WaitsOn,
    AttachedTo,
    Tagged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edge {
    pub from: usize,
    pub to: usize,
    pub link: Link,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Graph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    /// Each node's neighbours, either way along a link.
    pub neighbours: Vec<Vec<usize>>,
}

impl Graph {
    /// A graph made by hand, for the layout's tests.
    #[cfg(test)]
    pub fn with(nodes: Vec<Node>, edges: Vec<Edge>) -> Graph {
        Graph { nodes, edges, neighbours: Vec::new() }.finish()
    }

    pub fn index(&self, key: &str) -> Option<usize> {
        self.nodes.iter().position(|node| node.key == key)
    }

    /// Keeps the nodes `keep` chooses and the links between them.
    fn keep(self, keep: impl Fn(usize) -> bool) -> Graph {
        let mut moved = vec![None; self.nodes.len()];
        let mut nodes = Vec::new();
        for (at, node) in self.nodes.into_iter().enumerate() {
            if keep(at) {
                moved[at] = Some(nodes.len());
                nodes.push(node);
            }
        }
        let edges = self
            .edges
            .into_iter()
            .filter_map(|edge| Some(Edge { from: moved[edge.from]?, to: moved[edge.to]?, link: edge.link }))
            .collect();
        Graph { nodes, edges, neighbours: Vec::new() }.finish()
    }

    /// Counts each node's weight and gathers its neighbours.
    fn finish(mut self) -> Graph {
        let mut neighbours = vec![Vec::new(); self.nodes.len()];
        for node in &mut self.nodes {
            node.weight = 0;
        }
        for edge in &self.edges {
            self.nodes[edge.to].weight += 1;
            if edge.from != edge.to {
                neighbours[edge.from].push(edge.to);
                neighbours[edge.to].push(edge.from);
            }
        }
        for list in &mut neighbours {
            list.sort_unstable();
            list.dedup();
        }
        self.neighbours = neighbours;
        self
    }
}

fn visible(item: &Item) -> bool {
    item.stashed.is_none() && item.trashed.is_none()
}

/// The graph of the board in `snapshot` as `settings` filter it -- or, for a
/// local graph, only what lies within `depth` links of the item `centre` keys.
pub fn build(snapshot: &Snapshot, settings: &Settings, local: Option<(&str, usize)>) -> Graph {
    let visible_items: Vec<&Item> = snapshot.all.values().filter(|item| visible(item)).collect();
    // A local graph always shows its centre, so the search that filters the
    // whole graph does not filter a neighbourhood.
    let query = search::parse(&settings.search);
    let shown: HashSet<u32> = if local.is_some() || query.is_empty() {
        visible_items.iter().map(|item| item.id).collect()
    } else {
        search::matching(snapshot, &query)
    };

    let mut nodes = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    for item in visible_items.iter().filter(|item| shown.contains(&item.id)) {
        at.insert(board::key(item), nodes.len());
        nodes.push(Node {
            key: board::key(item),
            kind: Kind::Item { id: item.id },
            label: one_line(&item.description),
            created: item.timestamp,
            weight: 0,
            group: None,
        });
    }

    let by_uid: HashMap<&str, &Item> =
        snapshot.all.values().filter_map(|item| item.uid.as_deref().map(|uid| (uid, item))).collect();
    let items: Vec<(usize, &Item)> = nodes
        .iter()
        .enumerate()
        .filter_map(|(index, node)| match node.kind {
            Kind::Item { id } => snapshot.all.get(&id).map(|item| (index, item)),
            _ => None,
        })
        .collect();

    let mut edges = Vec::new();
    for (from, item) in items {
        let relations = item
            .blocked_by
            .iter()
            .flatten()
            .map(|uid| (uid.as_str(), Link::WaitsOn))
            .chain(item.attached_to.as_deref().map(|uid| (uid, Link::AttachedTo)));
        for (uid, link) in relations {
            let target = by_uid.get(uid).copied();
            let to = match target {
                Some(target) if at.contains_key(&board::key(target)) => at[&board::key(target)],
                // On the board, but left out by the search: no link, the way
                // Obsidian drops the links of the notes it filtered away.
                Some(target) if visible(target) => continue,
                _ if settings.existing_only => continue,
                _ => {
                    let key = format!("?{uid}");
                    *at.entry(key.clone()).or_insert_with(|| {
                        nodes.push(Node {
                            key,
                            kind: Kind::Unresolved,
                            label: target.map_or_else(|| "a deleted item".to_string(), |target| one_line(&target.description)),
                            created: target.map_or(item.timestamp, |target| target.timestamp),
                            weight: 0,
                            group: None,
                        });
                        nodes.len() - 1
                    })
                }
            };
            edges.push(Edge { from, to, link });
        }
        if settings.tags {
            for name in &item.boards {
                let to = *at.entry(name.clone()).or_insert_with(|| {
                    nodes.push(Node {
                        key: name.clone(),
                        kind: Kind::Board,
                        label: name.clone(),
                        created: item.timestamp,
                        weight: 0,
                        group: None,
                    });
                    nodes.len() - 1
                });
                edges.push(Edge { from, to, link: Link::Tagged });
            }
        }
    }

    let mut graph = Graph { nodes, edges, neighbours: Vec::new() }.finish();

    let mut centre = None;
    if let Some((key, depth)) = local {
        let Some(start) = graph.index(key) else { return Graph::default() };
        let mut reached = vec![false; graph.nodes.len()];
        let mut queue = VecDeque::from([(start, 0)]);
        reached[start] = true;
        while let Some((node, far)) = queue.pop_front() {
            if far == depth {
                continue;
            }
            for &next in &graph.neighbours[node] {
                if !reached[next] {
                    reached[next] = true;
                    queue.push_back((next, far + 1));
                }
            }
        }
        graph = graph.keep(|index| reached[index]);
        centre = graph.index(key);
    }

    if !settings.orphans {
        let lonely: Vec<bool> = graph.neighbours.iter().map(Vec::is_empty).collect();
        graph = graph.keep(|index| !lonely[index] || Some(index) == centre);
    }

    let groups: Vec<HashSet<u32>> = settings
        .groups
        .iter()
        .map(|group| {
            let query = search::parse(&group.query);
            if query.is_empty() {
                HashSet::new()
            } else {
                search::matching(snapshot, &query)
            }
        })
        .collect();
    for node in &mut graph.nodes {
        if let Kind::Item { id } = node.kind {
            node.group = groups.iter().position(|members| members.contains(&id));
        }
    }
    graph
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::board::tests::{board, snapshot, words};
    use crate::tui::graph::settings::Group;

    fn labels(graph: &Graph) -> Vec<&str> {
        let mut labels: Vec<&str> = graph.nodes.iter().map(|node| node.label.as_str()).collect();
        labels.sort_unstable();
        labels
    }

    fn link(graph: &Graph, from: &str, to: &str) -> Option<Link> {
        graph
            .edges
            .iter()
            .find(|edge| graph.nodes[edge.from].label == from && graph.nodes[edge.to].label == to)
            .map(|edge| edge.link)
    }

    /// Items are nodes, a task waits on its blocker, a note points at its task,
    /// and what is pointed at weighs more.
    #[test]
    fn items_are_nodes_and_relations_are_links() {
        let (dir, ekko) = board("model");
        ekko.create_task(&words(&["@a", "base"])).unwrap();
        ekko.create_task(&words(&["@a", "on top"])).unwrap();
        ekko.create_note(&words(&["@a", "why"])).unwrap();
        ekko.set_blocked_by(&words(&["@2", "1"])).unwrap();
        ekko.set_attached_to(&words(&["@3", "1"])).unwrap();
        let graph = build(&snapshot(&dir, &ekko), &Settings::default(), None);

        assert_eq!(labels(&graph), vec!["base", "on top", "why"]);
        assert_eq!(link(&graph, "on top", "base"), Some(Link::WaitsOn));
        assert_eq!(link(&graph, "why", "base"), Some(Link::AttachedTo));
        let base = graph.index(&board::key(&ekko.storage.get().unwrap()[&1])).unwrap();
        assert_eq!(graph.nodes[base].weight, 2);
        assert_eq!(graph.neighbours[base].len(), 2);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn boards_are_nodes_only_with_tags_on() {
        let (dir, ekko) = board("tags");
        ekko.create_task(&words(&["@a", "one"])).unwrap();
        ekko.create_task(&words(&["@a", "@b", "two"])).unwrap();
        let snapshot = snapshot(&dir, &ekko);

        assert_eq!(labels(&build(&snapshot, &Settings::default(), None)), vec!["one", "two"]);
        let tagged = build(&snapshot, &Settings { tags: true, ..Settings::default() }, None);
        assert_eq!(labels(&tagged), vec!["@a", "@b", "one", "two"]);
        assert_eq!(link(&tagged, "two", "@b"), Some(Link::Tagged));
        assert_eq!(tagged.nodes[tagged.index("@a").unwrap()].weight, 2);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A blocker put in the trash is an unresolved node, unless the graph
    /// shows existing items only -- and then the task waiting on it is an
    /// orphan, which the orphans filter can hide.
    #[test]
    fn a_relation_off_the_board_is_unresolved_unless_existing_only() {
        let (dir, ekko) = board("unresolved");
        ekko.create_task(&words(&["@a", "gone"])).unwrap();
        ekko.create_task(&words(&["@a", "waiting"])).unwrap();
        ekko.set_blocked_by(&words(&["@2", "1"])).unwrap();
        ekko.set_trashed(&words(&["1"]), true).unwrap();
        let snapshot = snapshot(&dir, &ekko);

        let graph = build(&snapshot, &Settings::default(), None);
        assert!(graph.nodes.iter().any(|node| node.kind == Kind::Unresolved && node.label == "gone"));
        let existing = Settings { existing_only: true, ..Settings::default() };
        assert_eq!(labels(&build(&snapshot, &existing, None)), vec!["waiting"]);
        let neither = Settings { existing_only: true, orphans: false, ..Settings::default() };
        assert!(build(&snapshot, &neither, None).nodes.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A local graph reaches as many links out from its centre as its depth.
    #[test]
    fn a_local_graph_reaches_depth_links_around_its_centre() {
        let (dir, ekko) = board("local");
        for description in ["one", "two", "three", "four"] {
            ekko.create_task(&words(&["@a", description])).unwrap();
        }
        ekko.set_blocked_by(&words(&["@2", "1"])).unwrap();
        ekko.set_blocked_by(&words(&["@3", "2"])).unwrap();
        ekko.set_blocked_by(&words(&["@4", "3"])).unwrap();
        let snapshot = snapshot(&dir, &ekko);
        let centre = board::key(&snapshot.all[&1]);

        assert_eq!(labels(&build(&snapshot, &Settings::default(), Some((&centre, 1)))), vec!["one", "two"]);
        assert_eq!(labels(&build(&snapshot, &Settings::default(), Some((&centre, 2)))), vec!["one", "three", "two"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A node takes the colour of the first group it belongs to, and a search
    /// keeps only what it finds.
    #[test]
    fn the_first_matching_group_colours_a_node_and_a_search_filters() {
        let (dir, ekko) = board("groups");
        ekko.create_task(&words(&["@a", "finished work"])).unwrap();
        ekko.create_task(&words(&["@a", "open work"])).unwrap();
        ekko.create_note(&words(&["@a", "a thought"])).unwrap();
        ekko.check_tasks(&words(&["1"]), false).unwrap();
        let snapshot = snapshot(&dir, &ekko);
        let settings = Settings {
            groups: vec![
                Group { query: "is:done".into(), colour: 0 },
                Group { query: "is:tasks".into(), colour: 1 },
            ],
            ..Settings::default()
        };

        let graph = build(&snapshot, &settings, None);
        let group = |label: &str| graph.nodes.iter().find(|node| node.label == label).and_then(|node| node.group);
        assert_eq!((group("finished work"), group("open work"), group("a thought")), (Some(0), Some(1), None));

        let searched = build(&snapshot, &Settings { search: "work".into(), ..Settings::default() }, None);
        assert_eq!(labels(&searched), vec!["finished work", "open work"]);
        std::fs::remove_dir_all(&dir).ok();
    }
}
