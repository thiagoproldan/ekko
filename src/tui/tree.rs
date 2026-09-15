//! The explorer's tree: the project with its boards and their items, and the
//! sections VS Code's explorer keeps around the folder -- open editors, an
//! outline -- with ekko's own beside them: phases, the stash, the trash and
//! the archive.
//!
//! Rows are rebuilt from the snapshot every frame. What is expanded, and where
//! the cursor is, is remembered by node rather than by row, so a board that
//! changes underneath keeps the tree open where it was.

use std::collections::HashSet;

use super::board::{self, one_line, Snapshot};
use super::list::Cursor;
use super::tabs::Tabs;
use crate::item::Item;
use crate::render::TRASH_DAYS;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Section {
    OpenEditors,
    Project,
    Phases,
    Stash,
    Trash,
    Archive,
    Outline,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Node {
    Section(Section),
    Board(String),
    Phase(String),
    Date(String),
    Heading(&'static str),
    Tab(usize),
    /// An item, under whatever shows it: an item can sit on two boards, and in
    /// its phase as well, and each is a place of its own in the tree.
    Item { under: String, key: String },
}

/// Where a phase stands, as `--roadmap` marks it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Behind,
    Here,
    Ahead,
}

/// How a row is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
    Section,
    Folder,
    Heading,
    Tab,
    Phase(Mark),
    /// An item, by display id -- in the archive when `archived`.
    Item { id: u32, archived: bool },
}

pub struct Row {
    pub node: Node,
    pub depth: u16,
    pub label: String,
    /// Drawn against the right edge: a count, a percentage, days left.
    pub detail: String,
    pub look: Look,
    pub expandable: bool,
    pub expanded: bool,
}

pub struct Tree {
    pub expanded: HashSet<Node>,
    pub cursor: Cursor,
    /// The node under the cursor, to find it again after the rows change.
    at: Option<Node>,
}

impl Default for Tree {
    fn default() -> Self {
        Tree { expanded: HashSet::from([Node::Section(Section::Project)]), cursor: Cursor::default(), at: None }
    }
}

struct Rows<'a> {
    tree: &'a Tree,
    rows: Vec<Row>,
}

impl Rows<'_> {
    /// Adds a row, and says whether what is under it follows.
    fn push(&mut self, node: Node, depth: u16, label: String, detail: String, look: Look, expandable: bool) -> bool {
        let expanded = expandable && self.tree.expanded.contains(&node);
        self.rows.push(Row { node, depth, label, detail, look, expandable, expanded });
        expanded
    }

    fn item(&mut self, under: &str, depth: u16, item: &Item, detail: String, expandable: bool) -> bool {
        let node = Node::Item { under: under.to_string(), key: board::key(item) };
        let look = Look::Item { id: item.id, archived: under.starts_with("date:") };
        self.push(node, depth, one_line(&item.description), detail, look, expandable)
    }
}

impl Tree {
    /// The rows of the tree as it stands: `project` names the root, and
    /// `outline` is the item whose relations the outline shows.
    pub fn rows(&self, snapshot: &Snapshot, tabs: &Tabs, project: &str, outline: Option<u32>) -> Vec<Row> {
        let mut rows = Rows { tree: self, rows: Vec::new() };

        let open = tabs.open.len().to_string();
        if rows.push(Node::Section(Section::OpenEditors), 0, "Open Editors".into(), open, Look::Section, true) {
            for (at, tab) in tabs.open.iter().enumerate() {
                rows.push(Node::Tab(at), 1, snapshot.title(&tab.doc), String::new(), Look::Tab, false);
            }
        }

        let percent = format!("{}%", snapshot.prime.stats.percent);
        if rows.push(Node::Section(Section::Project), 0, project.to_string(), percent, Look::Section, true) {
            for group in &snapshot.groups {
                let items = &snapshot.items[group.items.clone()];
                let counts = format!("{}/{}", group.complete, group.tasks);
                let node = Node::Board(group.name.clone());
                if !rows.push(node, 1, group.name.clone(), counts, Look::Folder, !items.is_empty()) {
                    continue;
                }
                // A note attached to a task on the same board sits under the
                // task, one level down, the way a file sits in its folder.
                let tasks: HashSet<&str> = items.iter().filter(|item| item.is_task).filter_map(|item| item.uid.as_deref()).collect();
                for item in items {
                    if item.attached_to.as_deref().is_some_and(|task| tasks.contains(task)) {
                        continue;
                    }
                    let notes: Vec<&Item> = match item.uid.as_deref() {
                        Some(uid) => items.iter().filter(|note| note.attached_to.as_deref() == Some(uid)).collect(),
                        None => Vec::new(),
                    };
                    if rows.item(&group.name, 2, item, String::new(), !notes.is_empty()) {
                        for note in notes {
                            rows.item(&group.name, 3, note, String::new(), false);
                        }
                    }
                }
            }
        }

        if !snapshot.phases.is_empty() {
            let count = snapshot.roadmap.len().to_string();
            if rows.push(Node::Section(Section::Phases), 0, "Phases".into(), count, Look::Section, true) {
                for step in &snapshot.roadmap {
                    let mark = if step.current {
                        Mark::Here
                    } else if step.total > 0 && step.complete == step.total {
                        Mark::Behind
                    } else {
                        Mark::Ahead
                    };
                    let items = snapshot.in_phase(&step.name);
                    let counts = format!("{}/{}", step.complete, step.total);
                    let node = Node::Phase(step.name.clone());
                    if rows.push(node, 1, step.name.clone(), counts, Look::Phase(mark), !items.is_empty()) {
                        let under = format!("phase:{}", step.name);
                        for item in items {
                            rows.item(&under, 2, item, String::new(), false);
                        }
                    }
                }
            }
        }

        let mut seen = HashSet::new();
        let stashed: Vec<(&str, &Item)> = snapshot
            .stash
            .iter()
            .flat_map(|(board, items)| items.iter().map(move |item| (board.as_str(), item)))
            .filter(|(_, item)| seen.insert(item.id))
            .collect();
        let count = stashed.len().to_string();
        if rows.push(Node::Section(Section::Stash), 0, "Stash".into(), count, Look::Section, true) {
            for (board, item) in stashed {
                rows.item("stash", 1, item, board.to_string(), false);
            }
        }

        let count = snapshot.trash.len().to_string();
        if rows.push(Node::Section(Section::Trash), 0, "Trash".into(), count, Look::Section, true) {
            let now = chrono::Local::now().timestamp_millis();
            for item in &snapshot.trash {
                let left = TRASH_DAYS - (now - item.trashed.unwrap_or(now)) / 86_400_000;
                rows.item("trash", 1, item, format!("{}d left", left.max(0)), false);
            }
        }

        let count = snapshot.archive.iter().map(|(_, items)| items.len()).sum::<usize>().to_string();
        if rows.push(Node::Section(Section::Archive), 0, "Archive".into(), count, Look::Section, true) {
            for (date, items) in &snapshot.archive {
                let node = Node::Date(date.clone());
                if rows.push(node, 1, date.clone(), items.len().to_string(), Look::Folder, true) {
                    let under = format!("date:{date}");
                    for item in items {
                        rows.item(&under, 2, item, String::new(), false);
                    }
                }
            }
        }

        if rows.push(Node::Section(Section::Outline), 0, "Outline".into(), String::new(), Look::Section, true) {
            match outline.and_then(|id| snapshot.all.get(&id)) {
                Some(item) => {
                    let links = snapshot.links.get(&item.id).cloned().unwrap_or_default();
                    let relations = [
                        ("Blocked by", links.blockers),
                        ("Blocks", links.dependents),
                        ("Notes", links.notes),
                        ("Attached to", links.attached_to.into_iter().collect()),
                    ];
                    let mut any = false;
                    for (heading, ids) in relations {
                        if ids.is_empty() {
                            continue;
                        }
                        any = true;
                        let count = ids.len().to_string();
                        rows.push(Node::Heading(heading), 1, heading.to_string(), count, Look::Heading, false);
                        let under = format!("outline:{heading}");
                        for other in ids.iter().filter_map(|id| snapshot.all.get(id)) {
                            rows.item(&under, 2, other, String::new(), false);
                        }
                    }
                    if !any {
                        let said = format!("{} has no relations", item.id);
                        rows.push(Node::Heading("none"), 1, said, String::new(), Look::Heading, false);
                    }
                }
                None => {
                    let said = "No item is active".to_string();
                    rows.push(Node::Heading("none"), 1, said, String::new(), Look::Heading, false);
                }
            }
        }

        rows.rows
    }

    /// Opens a node that is closed, and closes one that is open.
    pub fn toggle(&mut self, node: &Node) {
        if !self.expanded.remove(node) {
            self.expanded.insert(node.clone());
        }
    }

    /// Puts the cursor on row `at`.
    pub fn select(&mut self, at: usize, rows: &[Row]) {
        if let Some(row) = rows.get(at) {
            self.cursor.selected = at;
            self.at = Some(row.node.clone());
        }
    }

    pub fn step(&mut self, by: isize, rows: &[Row]) {
        let mut cursor = self.cursor;
        cursor.step(by, rows.len());
        self.select(cursor.selected, rows);
    }

    /// Right: opens the row under the cursor, or steps into it when it is open.
    pub fn right(&mut self, rows: &[Row]) {
        let Some(row) = rows.get(self.cursor.selected) else { return };
        if row.expandable && !row.expanded {
            self.expanded.insert(row.node.clone());
        } else if row.expanded && rows.get(self.cursor.selected + 1).is_some_and(|next| next.depth > row.depth) {
            self.select(self.cursor.selected + 1, rows);
        }
    }

    /// Left: closes the row under the cursor, or steps out to its parent.
    pub fn left(&mut self, rows: &[Row]) {
        let Some(row) = rows.get(self.cursor.selected) else { return };
        if row.expanded {
            self.expanded.remove(&row.node);
        } else if let Some(parent) = rows[..self.cursor.selected].iter().rposition(|above| above.depth < row.depth) {
            self.select(parent, rows);
        }
    }

    /// Finds the cursor's node again in rows that were just rebuilt.
    pub fn settle(&mut self, rows: &[Row]) {
        if let Some(at) = self.at.as_ref().and_then(|node| rows.iter().position(|row| &row.node == node)) {
            self.cursor.selected = at;
        }
        self.cursor.clamp(rows.len());
        self.at = rows.get(self.cursor.selected).map(|row| row.node.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::board::tests::{board, snapshot, words};

    fn labels(rows: &[Row]) -> Vec<(u16, String)> {
        rows.iter().map(|row| (row.depth, row.label.clone())).collect()
    }

    fn position(rows: &[Row], label: &str) -> usize {
        rows.iter().position(|row| row.label == label).unwrap_or_else(|| panic!("{label:?} is not in {:?}", labels(rows)))
    }

    /// The project opens by default with its boards closed, and a board opens
    /// onto its items with each attached note one level under its task.
    #[test]
    fn a_board_opens_onto_its_items_with_notes_under_their_task() {
        let (dir, ekko) = board("tree");
        ekko.create_task(&words(&["@a", "wire the watcher"])).unwrap();
        ekko.create_note(&words(&["@a", "why it polls"])).unwrap();
        ekko.create_task(&words(&["@b", "write the docs"])).unwrap();
        ekko.set_attached_to(&words(&["@2", "1"])).unwrap();
        let snapshot = snapshot(&dir, &ekko);
        let tabs = Tabs::default();
        let mut tree = Tree::default();

        let rows = tree.rows(&snapshot, &tabs, "demo", None);
        assert_eq!(rows[0].label, "Open Editors");
        assert!(!rows[0].expanded);
        let at = position(&rows, "@a");
        assert!(rows[at].expandable && !rows[at].expanded);

        tree.toggle(&rows[at].node.clone());
        let rows = tree.rows(&snapshot, &tabs, "demo", None);
        let task = position(&rows, "wire the watcher");
        assert_eq!(rows[task].depth, 2);
        assert!(rows[task].expandable, "a task with a note under it cannot be opened");
        assert!(!rows.iter().any(|row| row.label == "why it polls"), "the note shows before its task is opened");

        tree.toggle(&rows[task].node.clone());
        let rows = tree.rows(&snapshot, &tabs, "demo", None);
        assert_eq!(rows[position(&rows, "why it polls")].depth, 3);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Left and Right move through the tree the way they do in VS Code.
    #[test]
    fn left_and_right_open_close_and_step_out() {
        let (dir, ekko) = board("keys");
        ekko.create_task(&words(&["@a", "one"])).unwrap();
        let snapshot = snapshot(&dir, &ekko);
        let tabs = Tabs::default();
        let mut tree = Tree::default();

        let rows = tree.rows(&snapshot, &tabs, "demo", None);
        tree.select(position(&rows, "@a"), &rows);
        tree.right(&rows);
        let rows = tree.rows(&snapshot, &tabs, "demo", None);
        tree.settle(&rows);
        assert!(rows[tree.cursor.selected].expanded, "Right did not open the board");

        tree.right(&rows);
        assert_eq!(rows[tree.cursor.selected].label, "one", "Right did not step into the open board");
        tree.left(&rows);
        assert_eq!(rows[tree.cursor.selected].label, "@a", "Left did not step out to the board");
        tree.left(&rows);
        let rows = tree.rows(&snapshot, &tabs, "demo", None);
        tree.settle(&rows);
        assert!(!rows[tree.cursor.selected].expanded, "Left did not close the board");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The cursor stays on its node when rows appear above it.
    #[test]
    fn the_cursor_keeps_its_node_when_rows_change_above_it() {
        let (dir, ekko) = board("settle");
        ekko.create_task(&words(&["@a", "one"])).unwrap();
        ekko.create_task(&words(&["@b", "two"])).unwrap();
        let snapshot = snapshot(&dir, &ekko);
        let tabs = Tabs::default();
        let mut tree = Tree::default();

        let rows = tree.rows(&snapshot, &tabs, "demo", None);
        tree.select(position(&rows, "@b"), &rows);
        tree.toggle(&Node::Board("@a".to_string()));
        let rows = tree.rows(&snapshot, &tabs, "demo", None);
        tree.settle(&rows);
        assert_eq!(rows[tree.cursor.selected].label, "@b");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The outline lists the active item's relations under their headings.
    #[test]
    fn the_outline_lists_the_active_items_relations() {
        let (dir, ekko) = board("outline");
        ekko.create_task(&words(&["@a", "base"])).unwrap();
        ekko.create_task(&words(&["@a", "on top"])).unwrap();
        ekko.set_blocked_by(&words(&["@2", "1"])).unwrap();
        let snapshot = snapshot(&dir, &ekko);
        let tabs = Tabs::default();
        let mut tree = Tree::default();
        tree.toggle(&Node::Section(Section::Outline));

        let rows = tree.rows(&snapshot, &tabs, "demo", Some(2));
        let heading = position(&rows, "Blocked by");
        assert_eq!((rows[heading + 1].depth, rows[heading + 1].label.as_str()), (2, "base"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
