//! The search view: words, `@board`, `#id`, and `is:` filters.
//!
//! The filters are `--list`'s own, applied by the same function, so a search
//! here and `--list` in a terminal cannot disagree about what `ready` or
//! `overdue` means -- the rigor audit found three surfaces deciding that each
//! by itself, and this is not going to be a fourth. The graph view's search
//! filter and its groups read queries through here too.

use std::collections::HashSet;

use super::board::Snapshot;
use crate::ekko::{is_known_attribute, Ekko};
use crate::storage::ItemMap;

/// The filters offered as chips under the search field.
pub const CHIPS: [&str; 13] = [
    "pending", "progress", "paused", "done", "blocked", "ready", "starred", "due", "overdue", "notes", "decision", "gotcha", "procedure",
];

#[derive(Debug, Default, PartialEq)]
pub struct Query {
    pub words: Vec<String>,
    pub attributes: Vec<String>,
    pub boards: Vec<String>,
    pub ids: Vec<u32>,
    /// `is:` words no filter knows, reported rather than ignored.
    pub unknown: Vec<String>,
}

impl Query {
    pub fn is_empty(&self) -> bool {
        self.words.is_empty() && self.attributes.is_empty() && self.boards.is_empty() && self.ids.is_empty()
    }
}

pub fn parse(text: &str) -> Query {
    let mut query = Query::default();
    for token in text.split_whitespace() {
        if let Some(attribute) = token.strip_prefix("is:") {
            let attribute = attribute.to_lowercase();
            if is_known_attribute(&attribute) {
                query.attributes.push(attribute);
            } else {
                query.unknown.push(token.to_string());
            }
        } else if token.len() > 1 && token.starts_with('@') {
            query.boards.push(token.to_lowercase());
        } else if let Some(id) = token.strip_prefix('#').and_then(|id| id.parse().ok()) {
            query.ids.push(id);
        } else {
            query.words.push(token.to_lowercase());
        }
    }
    query
}

/// The visible items `query` finds, by id: every visible item for an empty
/// query, and otherwise those the core's filters keep that carry every word,
/// one of the ids when any are given, and one of the boards when any are.
pub fn matching(snapshot: &Snapshot, query: &Query) -> HashSet<u32> {
    let visible: ItemMap = snapshot
        .all
        .iter()
        .filter(|(_, item)| item.stashed.is_none() && item.trashed.is_none())
        .map(|(id, item)| (*id, item.clone()))
        .collect();
    let kept = Ekko::filter_by_attributes(&query.attributes, visible, &snapshot.all);
    kept.values()
        .filter(|item| {
            let description = item.description.to_lowercase();
            (query.ids.is_empty() || query.ids.contains(&item.id))
                && (query.boards.is_empty() || item.boards.iter().any(|board| query.boards.contains(&board.to_lowercase())))
                && query.words.iter().all(|word| description.contains(word))
        })
        .map(|item| item.id)
        .collect()
}

/// A board and its matching items, by index into `Snapshot::items`.
#[derive(Debug, PartialEq)]
pub struct Hit {
    pub group: usize,
    pub items: Vec<usize>,
}

/// What `query` finds among the visible items, board by board. An empty query
/// finds nothing, as VS Code's search does before anything is typed.
pub fn results(snapshot: &Snapshot, query: &Query) -> Vec<Hit> {
    if query.is_empty() {
        return Vec::new();
    }
    let found = matching(snapshot, query);
    let mut hits = Vec::new();
    for (at, group) in snapshot.groups.iter().enumerate() {
        if !query.boards.is_empty() && !query.boards.contains(&group.name.to_lowercase()) {
            continue;
        }
        let items: Vec<usize> = group.items.clone().filter(|&index| found.contains(&snapshot.items[index].id)).collect();
        if !items.is_empty() {
            hits.push(Hit { group: at, items });
        }
    }
    hits
}

/// Adds `is:<chip>` to the query text, or takes it out when it is there.
pub fn toggle_chip(text: &str, chip: &str) -> String {
    let token = format!("is:{chip}");
    let tokens: Vec<&str> = text.split_whitespace().collect();
    if tokens.iter().any(|t| t.eq_ignore_ascii_case(&token)) {
        tokens.into_iter().filter(|t| !t.eq_ignore_ascii_case(&token)).collect::<Vec<_>>().join(" ")
    } else {
        tokens.into_iter().chain(std::iter::once(token.as_str())).collect::<Vec<_>>().join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::board::tests::{board, snapshot, words};

    #[test]
    fn a_query_splits_into_words_boards_ids_and_filters() {
        let query = parse("Wire is:done @Render #12 is:bogus");
        assert_eq!(query.words, vec!["wire"]);
        assert_eq!(query.attributes, vec!["done"]);
        assert_eq!(query.boards, vec!["@render"]);
        assert_eq!(query.ids, vec![12]);
        assert_eq!(query.unknown, vec!["is:bogus"]);
        assert!(parse("   ").is_empty());
    }

    /// The filters are the core's: `pending` is open work, `blocked` resolves
    /// blockers against everything, and words narrow what the filters keep.
    #[test]
    fn results_are_filtered_by_the_core() {
        let (dir, ekko) = board("search");
        ekko.create_task(&words(&["@a", "wire the watcher"])).unwrap();
        ekko.create_task(&words(&["@a", "wire the frame"])).unwrap();
        ekko.create_task(&words(&["@b", "document the wire format"])).unwrap();
        ekko.check_tasks(&words(&["1"]), false).unwrap();
        ekko.set_blocked_by(&words(&["@3", "2"])).unwrap();
        let snapshot = snapshot(&dir, &ekko);
        let found = |text: &str| -> Vec<u32> {
            results(&snapshot, &parse(text))
                .iter()
                .flat_map(|hit| hit.items.iter().map(|&index| snapshot.items[index].id))
                .collect()
        };

        assert_eq!(found("wire is:pending"), vec![2, 3]);
        assert_eq!(found("is:done"), vec![1]);
        assert_eq!(found("is:blocked"), vec![3]);
        assert_eq!(found("@b wire"), vec![3]);
        assert_eq!(found("#2"), vec![2]);
        assert!(found("").is_empty());
        assert_eq!(matching(&snapshot, &parse("")).len(), 3, "an empty query does not match everything visible");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_chip_adds_its_filter_and_takes_it_out_again() {
        let added = toggle_chip("wire", "done");
        assert_eq!(added, "wire is:done");
        assert_eq!(toggle_chip(&added, "done"), "wire");
    }
}
