//! Where each box goes, for a terminal of a given size.
//!
//! The frame is VS Code's: a command centre on the top row, the editor in the
//! middle with a box of its own on either side, a bottom panel under all
//! three, the activity bar down the right edge and a status bar under
//! everything. Boxes sit edge to edge, and the gap between two of them is the
//! window showing between their rounded borders -- a border line runs through
//! the middle of its cell, so neighbouring borders already stand apart. The
//! sidebar is the exception, as it is in VS Code: its right edge is the
//! activity bar's left one.
//!
//! Pure geometry, so every size can be tested without a terminal.

use ratatui::layout::Rect;

/// Below this the frame cannot show a board meaningfully, and says so.
pub const MIN_WIDTH: u16 = 60;
pub const MIN_HEIGHT: u16 = 16;

const ACTIVITY_WIDTH: u16 = 5;
const NEXT_WIDTH: u16 = 32;
const SIDEBAR_WIDTH: u16 = 34;

/// Which optional boxes are open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Visibility {
    pub next: bool,
    pub sidebar: bool,
    pub panel: bool,
}

impl Default for Visibility {
    fn default() -> Self {
        Visibility { next: true, sidebar: true, panel: true }
    }
}

/// The boxes of one frame. An optional box is `None` when closed, or when
/// the terminal has no room for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Regions {
    pub command: Rect,
    pub next: Option<Rect>,
    pub editor: Rect,
    pub sidebar: Option<Rect>,
    pub activity: Rect,
    pub panel: Option<Rect>,
    pub status: Rect,
}

/// A box that can hold the focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    Command,
    Next,
    Editor,
    Sidebar,
    Panel,
}

/// The frame for `area`, or `None` when the terminal is too small.
///
/// As the terminal narrows the optional boxes give way in the order VS Code's
/// own layout drops them -- the box beside the editor first, then the
/// sidebar -- and the bottom panel goes when the height runs short. The
/// editor, the activity bar and the status bar always stay.
pub fn regions(area: Rect, wanted: Visibility) -> Option<Regions> {
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        return None;
    }

    let status = Rect::new(area.x, area.y + area.height - 1, area.width, 1);
    let command_width = (area.width / 2).clamp(30, 72);
    let command = Rect::new(area.x + (area.width - command_width) / 2, area.y, command_width, 1);

    let body_y = area.y + 1;
    let body_height = area.height - 2;
    let activity = Rect::new(area.x + area.width - ACTIVITY_WIDTH, body_y, ACTIVITY_WIDTH, body_height);
    let work_width = area.width - ACTIVITY_WIDTH;

    let next_shown = wanted.next && work_width >= 110;
    let sidebar_shown = wanted.sidebar && work_width >= 80;
    let panel_shown = wanted.panel && body_height >= 22;

    let panel_height = if panel_shown { (body_height * 3 / 10).clamp(6, 14) } else { 0 };
    let top_height = body_height - panel_height;

    let next = next_shown.then(|| Rect::new(area.x, body_y, NEXT_WIDTH, top_height));
    let editor_x = area.x + if next_shown { NEXT_WIDTH } else { 0 };
    let sidebar_width = if sidebar_shown { SIDEBAR_WIDTH } else { 0 };
    let editor = Rect::new(editor_x, body_y, work_width - (editor_x - area.x) - sidebar_width, top_height);
    let sidebar = sidebar_shown.then(|| Rect::new(editor.x + editor.width, body_y, SIDEBAR_WIDTH, top_height));
    let panel = panel_shown.then(|| Rect::new(area.x, body_y + top_height, work_width, panel_height));

    Some(Regions { command, next, editor, sidebar, activity, panel, status })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(frame: &Regions) -> Vec<Rect> {
        [Some(frame.command), frame.next, Some(frame.editor), frame.sidebar, Some(frame.activity), frame.panel, Some(frame.status)]
            .into_iter()
            .flatten()
            .collect()
    }

    fn overlap(a: Rect, b: Rect) -> bool {
        a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height
    }

    /// On a wide terminal every box is there, none overlaps another, and all
    /// of them stay inside the screen.
    #[test]
    fn a_wide_terminal_shows_every_box_without_overlap() {
        let area = Rect::new(0, 0, 180, 50);
        let frame = regions(area, Visibility::default()).unwrap();
        assert!(frame.next.is_some() && frame.sidebar.is_some() && frame.panel.is_some());

        let boxes = all(&frame);
        for (i, a) in boxes.iter().enumerate() {
            assert!(a.x + a.width <= area.width && a.y + a.height <= area.height, "{a:?} leaves the screen");
            for b in &boxes[i + 1..] {
                assert!(!overlap(*a, *b), "{a:?} overlaps {b:?}");
            }
        }
        assert_eq!(frame.activity.x + frame.activity.width, area.width, "the activity bar is not on the edge");
        assert_eq!(frame.sidebar.unwrap().right(), frame.activity.x, "the sidebar does not meet the activity bar");
        assert_eq!(frame.status.y, area.height - 1);
        let panel = frame.panel.unwrap();
        assert_eq!(panel.width + frame.activity.width, area.width, "the bottom panel does not span the work area");
    }

    /// The optional boxes give way as the terminal narrows or shortens, the
    /// side box first; the editor always keeps the room left over.
    #[test]
    fn boxes_give_way_as_the_terminal_shrinks() {
        let medium = regions(Rect::new(0, 0, 100, 40), Visibility::default()).unwrap();
        assert!(medium.next.is_none() && medium.sidebar.is_some());

        let narrow = regions(Rect::new(0, 0, 70, 40), Visibility::default()).unwrap();
        assert!(narrow.sidebar.is_none());
        assert_eq!(narrow.editor.width, 65);

        let short = regions(Rect::new(0, 0, 180, 20), Visibility::default()).unwrap();
        assert!(short.panel.is_none());
        assert_eq!(short.editor.height, 18);
    }

    #[test]
    fn a_terminal_too_small_has_no_frame() {
        assert!(regions(Rect::new(0, 0, MIN_WIDTH - 1, 40), Visibility::default()).is_none());
        assert!(regions(Rect::new(0, 0, 120, MIN_HEIGHT - 1), Visibility::default()).is_none());
    }

    /// A box that was closed stays closed, and the editor takes its room.
    #[test]
    fn a_closed_box_gives_its_room_to_the_editor() {
        let area = Rect::new(0, 0, 180, 50);
        let open = regions(area, Visibility::default()).unwrap();
        let closed = regions(area, Visibility { next: false, sidebar: false, panel: false }).unwrap();
        assert!(closed.next.is_none() && closed.sidebar.is_none() && closed.panel.is_none());
        assert!(closed.editor.width > open.editor.width && closed.editor.height > open.editor.height);
    }
}
