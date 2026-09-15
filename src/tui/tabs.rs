//! The editor's tabs, kept the way VS Code keeps them.
//!
//! A tab opened in passing is a preview: its title in italics, and replaced by
//! the next thing opened in passing, so looking through items does not bury
//! the editor under tabs. Opening something on purpose -- a double click, or
//! Enter -- pins it. Closing a tab goes back to the one used before it, not to
//! whichever happens to sit beside it.

/// What an editor tab shows.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Doc {
    Board,
    Welcome,
    Roadmap,
    Calendar,
    /// An item, by its uid, or `#id` for one written before uids existed.
    Item(String),
}

/// The documents there is one of, for things that open one by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Page {
    Board,
    Welcome,
    Roadmap,
    Calendar,
}

impl Page {
    pub fn doc(self) -> Doc {
        match self {
            Page::Board => Doc::Board,
            Page::Welcome => Doc::Welcome,
            Page::Roadmap => Doc::Roadmap,
            Page::Calendar => Doc::Calendar,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditorTab {
    pub doc: Doc,
    pub preview: bool,
}

pub struct Tabs {
    pub open: Vec<EditorTab>,
    pub active: usize,
    /// Open documents in the order they were last active, most recent last.
    used: Vec<Doc>,
}

impl Default for Tabs {
    fn default() -> Self {
        Tabs { open: vec![EditorTab { doc: Doc::Board, preview: false }], active: 0, used: vec![Doc::Board] }
    }
}

impl Tabs {
    pub fn active(&self) -> &Doc {
        &self.open[self.active].doc
    }

    pub fn activate(&mut self, at: usize) {
        if let Some(tab) = self.open.get(at) {
            let doc = tab.doc.clone();
            self.active = at;
            self.used.retain(|used| *used != doc);
            self.used.push(doc);
        }
    }

    /// Shows `doc`: the tab it already has, pinned if this opening is not a
    /// preview; the preview tab's place when this one is a preview too; and
    /// otherwise a new tab just after the active one.
    pub fn open(&mut self, doc: Doc, preview: bool) {
        if let Some(at) = self.open.iter().position(|tab| tab.doc == doc) {
            if !preview {
                self.open[at].preview = false;
            }
            self.activate(at);
            return;
        }
        let tab = EditorTab { doc, preview };
        if preview {
            if let Some(at) = self.open.iter().position(|tab| tab.preview) {
                let replaced = std::mem::replace(&mut self.open[at], tab);
                self.used.retain(|used| *used != replaced.doc);
                self.activate(at);
                return;
            }
        }
        let at = (self.active + 1).min(self.open.len());
        self.open.insert(at, tab);
        self.activate(at);
    }

    /// Keeps a preview tab open for good, as a double click on it does.
    pub fn pin(&mut self, at: usize) {
        if let Some(tab) = self.open.get_mut(at) {
            tab.preview = false;
        }
    }

    /// Closes the tab at `at` and returns to the tab used before it. The board
    /// is the one tab that never closes: it is where the writes are.
    pub fn close(&mut self, at: usize) -> bool {
        if self.open.get(at).is_none_or(|tab| tab.doc == Doc::Board) {
            return false;
        }
        let closed = self.open.remove(at);
        self.used.retain(|used| *used != closed.doc);
        let back = self
            .used
            .last()
            .and_then(|doc| self.open.iter().position(|tab| &tab.doc == doc))
            .unwrap_or(0);
        self.activate(back);
        true
    }

    /// The next or previous tab, wrapping around, as Ctrl+PageDown and Ctrl+PageUp.
    pub fn step(&mut self, by: isize) {
        let len = self.open.len() as isize;
        self.activate((self.active as isize + by).rem_euclid(len) as usize);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(key: &str) -> Doc {
        Doc::Item(key.to_string())
    }

    fn docs(tabs: &Tabs) -> Vec<(Doc, bool)> {
        tabs.open.iter().map(|tab| (tab.doc.clone(), tab.preview)).collect()
    }

    /// Looking through items keeps one preview tab, replaced each time.
    #[test]
    fn a_preview_replaces_the_previous_preview() {
        let mut tabs = Tabs::default();
        tabs.open(item("a"), true);
        tabs.open(item("b"), true);
        assert_eq!(docs(&tabs), vec![(Doc::Board, false), (item("b"), true)]);
        assert_eq!(tabs.active(), &item("b"));
    }

    /// Opening on purpose pins, whether the tab was a preview or not yet open,
    /// and a later preview then opens beside it instead of replacing it.
    #[test]
    fn opening_on_purpose_pins_and_keeps_the_tab() {
        let mut tabs = Tabs::default();
        tabs.open(item("a"), true);
        tabs.open(item("a"), false);
        tabs.open(item("b"), true);
        assert_eq!(docs(&tabs), vec![(Doc::Board, false), (item("a"), false), (item("b"), true)]);

        tabs.open(item("a"), true);
        assert_eq!(tabs.active(), &item("a"));
        assert!(!tabs.open[1].preview, "a preview opening unpinned a pinned tab");
    }

    /// Closing returns to the tab used before, and the board never closes.
    #[test]
    fn closing_goes_back_to_the_tab_used_before() {
        let mut tabs = Tabs::default();
        tabs.open(Doc::Roadmap, false);
        tabs.open(Doc::Calendar, false);
        tabs.activate(0);
        tabs.activate(2);
        assert_eq!(tabs.active(), &Doc::Calendar);

        assert!(tabs.close(2));
        assert_eq!(tabs.active(), &Doc::Board, "did not return to the tab used before");
        assert!(!tabs.close(0), "the board closed");
        assert_eq!(tabs.open.len(), 2);
    }

    #[test]
    fn stepping_through_tabs_wraps_around() {
        let mut tabs = Tabs::default();
        tabs.open(Doc::Welcome, false);
        tabs.step(1);
        assert_eq!(tabs.active(), &Doc::Board);
        tabs.step(-1);
        assert_eq!(tabs.active(), &Doc::Welcome);
    }
}
