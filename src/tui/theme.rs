//! The look: VS Code's current dark theme, measured, and the codicon glyphs.
//!
//! Every colour below was read pixel by pixel off the user's own VS Code on
//! 2026-09-15, not approximated from memory -- the point of the interactive
//! mode's style is to read as the same family, and "roughly that grey" is
//! how a copy starts to look like a copy.
//!
//! The icons are VS Code's own codicons as Nerd Fonts carry them, with a plain
//! set for a terminal without a Nerd Font: `EKKO_ICONS=plain`.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

use crate::item::{Item, State};

/// Measured from VS Code, except where a comment says otherwise.
pub mod palette {
    use ratatui::style::Color;

    /// The window: the gaps between boxes, the side panels, the status bar.
    pub const WINDOW: Color = Color::Rgb(0x1F, 0x20, 0x21);
    /// The editor's own fill, a step darker, so the working area stands out.
    pub const EDITOR: Color = Color::Rgb(0x19, 0x1A, 0x1B);
    /// One-cell rounded borders, and the lines between sections.
    pub const BORDER: Color = Color::Rgb(0x2E, 0x2F, 0x30);
    /// A selected tab or title, and the selected row of the focused box.
    pub const PILL: Color = Color::Rgb(0x30, 0x31, 0x32);
    /// Behind the active activity-bar icon.
    pub const ACTIVE: Color = Color::Rgb(0x34, 0x35, 0x36);
    /// Not measured: the selected row of a box without focus, between the
    /// window and the pill, the way VS Code dims an inactive selection.
    pub const IDLE: Color = Color::Rgb(0x27, 0x28, 0x29);
    /// Not measured: a scrollbar's thumb, lighter than any surface it slides on.
    pub const SLIDER: Color = Color::Rgb(0x45, 0x46, 0x47);
    pub const TEXT: Color = Color::Rgb(0xBE, 0xBE, 0xBE);
    pub const BRIGHT: Color = Color::Rgb(0xEC, 0xEC, 0xEC);
    pub const MUTED: Color = Color::Rgb(0x8A, 0x8A, 0x8A);
    pub const LINK: Color = Color::Rgb(0x4A, 0x9F, 0xC6);
    pub const ACCENT: Color = Color::Rgb(0x00, 0x77, 0xD3);
    pub const GREEN: Color = Color::Rgb(0x28, 0xD0, 0x8A);
    /// Not on the measured screen: VS Code's error and warning foregrounds.
    pub const RED: Color = Color::Rgb(0xF1, 0x4C, 0x4C);
    pub const YELLOW: Color = Color::Rgb(0xCC, 0xA7, 0x00);
    /// Not on the measured screen: VS Code's purple and orange -- purple for
    /// tags, orange for a task waiting on something outside the board.
    pub const PURPLE: Color = Color::Rgb(0xB1, 0x80, 0xD7);
    pub const ORANGE: Color = Color::Rgb(0xD1, 0x86, 0x16);

    /// The graph's own greys, stepped between the surfaces and the text the
    /// way Obsidian's graph steps its links and nodes against its background.
    pub const NODE: Color = Color::Rgb(0xA6, 0xA6, 0xA6);
    pub const EDGE: Color = Color::Rgb(0x4E, 0x4F, 0x50);
    pub const FADED: Color = Color::Rgb(0x36, 0x37, 0x38);
    pub const UNRESOLVED: Color = Color::Rgb(0x60, 0x61, 0x62);

    /// The colours a group can be drawn in, by the index its settings keep.
    pub const GROUPS: [Color; 7] = [GREEN, ACCENT, YELLOW, LINK, RED, PURPLE, ORANGE];
}

/// Which glyph set the terminal can draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyphs {
    /// Codicons from a Nerd Font, and rounded pill ends.
    Nerd,
    /// Characters any font has, and square pills.
    Plain,
}

impl Glyphs {
    /// Nerd Font glyphs unless `EKKO_ICONS` asks for `plain` -- a terminal
    /// cannot say which font it draws with, so this is the person's call.
    pub fn from_env() -> Self {
        match std::env::var("EKKO_ICONS").as_deref() {
            Ok("plain") | Ok("ascii") => Glyphs::Plain,
            _ => Glyphs::Nerd,
        }
    }

    pub fn icons(self) -> &'static Icons {
        match self {
            Glyphs::Nerd => &NERD,
            Glyphs::Plain => &PLAIN,
        }
    }
}

/// One glyph per thing the frame draws, each one cell wide.
pub struct Icons {
    // The activity bar.
    pub explorer: &'static str,
    pub search: &'static str,
    pub graph: &'static str,
    pub projects: &'static str,
    pub changes: &'static str,
    // The toggles beside the command centre.
    pub left_on: &'static str,
    pub left_off: &'static str,
    pub panel_on: &'static str,
    pub panel_off: &'static str,
    pub right_on: &'static str,
    pub right_off: &'static str,
    // Tabs and the pages they show.
    pub board: &'static str,
    pub welcome: &'static str,
    pub roadmap: &'static str,
    pub calendar: &'static str,
    pub pinned: &'static str,
    pub close: &'static str,
    pub ellipsis: &'static str,
    // Trees, lists and the calendar's arrows.
    pub collapsed: &'static str,
    pub expanded: &'static str,
    pub back: &'static str,
    // Marks after an item.
    pub star: &'static str,
    pub due: &'static str,
    pub waits: &'static str,
    // Problems and the status bar.
    pub blocked: &'static str,
    pub warning: &'static str,
    pub branch: &'static str,
    pub sync: &'static str,
    pub rocket: &'static str,
    pub folder: &'static str,
    pub bell: &'static str,
    pub bell_dot: &'static str,
    // An item's state.
    pub done: &'static str,
    pub progress: &'static str,
    pub paused: &'static str,
    pub waiting: &'static str,
    pub pending: &'static str,
    pub cancelled: &'static str,
    pub note: &'static str,
    // The roadmap's nodes, as `--roadmap` draws them.
    pub behind: &'static str,
    pub here: &'static str,
    pub ahead: &'static str,
    // The graph's buttons.
    pub settings: &'static str,
    pub zoom_in: &'static str,
    pub zoom_out: &'static str,
    pub fit: &'static str,
    pub add: &'static str,
    pub discard: &'static str,
    pub play: &'static str,
}

/// Codepoints from the Nerd Fonts glyph table (`nf-cod-*`). Pending keeps a
/// plain circle, which every Nerd Font has and codicons have no outline for;
/// a blocker keeps the arrow the board view already draws, and the roadmap
/// keeps the board view's own nodes.
const NERD: Icons = Icons {
    explorer: "\u{eaf0}",
    search: "\u{ea6d}",
    graph: "\u{eb03}",
    projects: "\u{eb30}",
    changes: "\u{ea82}",
    left_on: "\u{ebf3}",
    left_off: "\u{ec02}",
    panel_on: "\u{ebf2}",
    panel_off: "\u{ec01}",
    right_on: "\u{ebf4}",
    right_off: "\u{ec00}",
    board: "\u{eab3}",
    welcome: "\u{eb06}",
    roadmap: "\u{eb20}",
    calendar: "\u{eab0}",
    pinned: "\u{eba0}",
    close: "\u{ea76}",
    ellipsis: "\u{ea7c}",
    collapsed: "\u{eab6}",
    expanded: "\u{eab4}",
    back: "\u{eab5}",
    star: "\u{eb59}",
    due: "\u{eab0}",
    waits: "\u{21e0}",
    blocked: "\u{ea87}",
    warning: "\u{ea6c}",
    branch: "\u{ec6f}",
    sync: "\u{ea77}",
    rocket: "\u{eb44}",
    folder: "\u{ea83}",
    bell: "\u{eaa2}",
    bell_dot: "\u{eb9a}",
    done: "\u{eba4}",
    progress: "\u{eb2c}",
    paused: "\u{ead1}",
    waiting: "\u{eb7b}",
    pending: "\u{25cb}",
    cancelled: "\u{eabd}",
    note: "\u{eb26}",
    behind: "\u{25cf}",
    here: "\u{25c9}",
    ahead: "\u{25cb}",
    settings: "\u{eb51}",
    zoom_in: "\u{eb81}",
    zoom_out: "\u{eb82}",
    fit: "\u{eb4c}",
    add: "\u{ea60}",
    discard: "\u{eae2}",
    play: "\u{eb2c}",
};

/// The board view's own glyphs where it has one, so a task reads the same in
/// both frontends.
const PLAIN: Icons = Icons {
    explorer: "≡",
    search: "/",
    graph: "◈",
    projects: "▦",
    changes: "↺",
    left_on: "[",
    left_off: "[",
    panel_on: "_",
    panel_off: "_",
    right_on: "]",
    right_off: "]",
    board: "▤",
    welcome: "☆",
    roadmap: "◔",
    calendar: "◷",
    pinned: "•",
    close: "×",
    ellipsis: "…",
    collapsed: "▸",
    expanded: "▾",
    back: "◂",
    star: "★",
    due: "◷",
    waits: "⇠",
    blocked: "⊗",
    warning: "!",
    branch: "⎇",
    sync: "↻",
    rocket: "»",
    folder: "⌂",
    bell: "○",
    bell_dot: "◉",
    done: "✔",
    progress: "…",
    paused: "⏸",
    waiting: "◔",
    pending: "☐",
    cancelled: "⊘",
    note: "●",
    behind: "●",
    here: "◉",
    ahead: "○",
    settings: "⚙",
    zoom_in: "+",
    zoom_out: "−",
    fit: "□",
    add: "+",
    discard: "↶",
    play: "▶",
};

/// A pill: text on the pill colour, with rounded ends drawn as half circles
/// in that colour over whatever is `under` it -- how a terminal draws VS
/// Code's tab and title pills. Square-ended with plain glyphs.
pub fn pill(glyphs: Glyphs, text: String, fg: Color, bg: Color, under: Color) -> Vec<Span<'static>> {
    match glyphs {
        Glyphs::Nerd => vec![
            Span::styled("\u{e0b6}", Style::new().fg(bg).bg(under)),
            Span::styled(text, Style::new().fg(fg).bg(bg)),
            Span::styled("\u{e0b4}", Style::new().fg(bg).bg(under)),
        ],
        Glyphs::Plain => vec![Span::styled(format!(" {text} "), Style::new().fg(fg).bg(bg))],
    }
}

/// The glyph and the style an item is drawn with, from its one state.
pub fn item_look(glyphs: Glyphs, item: &Item) -> (&'static str, Style) {
    state_look(glyphs, State::of(item))
}

/// The glyph and the style of a state, or of a note for `None`.
pub fn state_look(glyphs: Glyphs, state: Option<State>) -> (&'static str, Style) {
    let icons = glyphs.icons();
    let base = Style::new().fg(palette::TEXT);
    match state {
        None => (icons.note, base.fg(palette::LINK)),
        Some(State::Pending) => (icons.pending, base.fg(palette::MUTED)),
        Some(State::Progress) => (icons.progress, base.fg(palette::ACCENT)),
        Some(State::Paused) => (icons.paused, base.fg(palette::YELLOW)),
        Some(State::Waiting) => (icons.waiting, base.fg(palette::ORANGE)),
        Some(State::Done) => (icons.done, base.fg(palette::GREEN)),
        Some(State::Cancelled) => (icons.cancelled, base.fg(palette::MUTED)),
    }
}

/// How an item's description is drawn: finished work steps back, and
/// cancelled work is struck through, as the board view does it.
/// What goes before a handoff's or a typed note's text in a list: the word,
/// in the notes' colour, then a space. Empty for anything else.
pub fn mark(item: &Item) -> String {
    item.mark().map(|mark| format!("{mark} ")).unwrap_or_default()
}

/// The style of `mark`.
pub fn mark_style() -> Style {
    Style::new().fg(palette::LINK).add_modifier(Modifier::ITALIC)
}

pub fn description_style(item: &Item) -> Style {
    match State::of(item) {
        Some(State::Done) => Style::new().fg(palette::MUTED),
        Some(State::Cancelled) => Style::new().fg(palette::MUTED).add_modifier(Modifier::CROSSED_OUT),
        _ => Style::new().fg(palette::TEXT),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(icons: &Icons) -> Vec<&'static str> {
        vec![
            icons.explorer, icons.search, icons.graph, icons.projects, icons.changes, icons.left_on,
            icons.left_off, icons.panel_on, icons.panel_off, icons.right_on, icons.right_off, icons.board,
            icons.welcome, icons.roadmap, icons.calendar, icons.pinned, icons.close, icons.ellipsis,
            icons.collapsed, icons.expanded, icons.back, icons.star, icons.due, icons.waits, icons.blocked,
            icons.warning, icons.branch, icons.sync, icons.rocket, icons.folder, icons.bell, icons.bell_dot,
            icons.done, icons.progress, icons.paused, icons.pending, icons.cancelled, icons.note, icons.behind,
            icons.here, icons.ahead, icons.settings, icons.zoom_in, icons.zoom_out, icons.fit, icons.add,
            icons.discard, icons.play,
        ]
    }

    /// Every glyph is one cell wide by the measure ratatui lays out with -- a
    /// wider one would push the rest of its line over by a cell.
    #[test]
    fn every_glyph_is_one_cell_wide() {
        use unicode_width::UnicodeWidthStr;
        for icons in [&NERD, &PLAIN] {
            for glyph in all(icons) {
                assert_eq!(glyph.width(), 1, "{glyph:?} is {} cells", glyph.width());
            }
        }
    }

    /// Without a Nerd Font nothing from the private use area is drawn.
    #[test]
    fn the_plain_set_has_nothing_from_the_private_use_area() {
        for glyph in all(&PLAIN) {
            assert!(!glyph.chars().any(|c| ('\u{e000}'..='\u{f8ff}').contains(&c)), "{glyph:?}");
        }
    }

    /// A rounded pill is its text between two half circles drawn in the pill
    /// colour over the surface under it; a plain one pads with the pill.
    #[test]
    fn a_pill_rounds_its_ends_in_its_own_colour() {
        let spans = pill(Glyphs::Nerd, "Board".into(), palette::BRIGHT, palette::PILL, palette::EDITOR);
        assert_eq!(spans.len(), 3);
        assert_eq!((spans[0].style.fg, spans[0].style.bg), (Some(palette::PILL), Some(palette::EDITOR)));
        assert_eq!(spans[1].style.bg, Some(palette::PILL));

        let plain = pill(Glyphs::Plain, "Board".into(), palette::BRIGHT, palette::PILL, palette::EDITOR);
        assert_eq!(plain[0].content, " Board ");
    }
}
