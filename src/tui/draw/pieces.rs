//! The small pieces every box is drawn from: rounded cards, pills, fields,
//! dividers, and text cut or wrapped to the cells it has.

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Widget};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::item::{Item, State};
use crate::render::Stats;
use crate::tui::app::App;
use crate::tui::tabs::Doc;
use crate::tui::theme::{self, palette, Glyphs};

pub(super) fn card(buf: &mut Buffer, rect: Rect, fill: Color, borders: Borders) {
    Block::new()
        .borders(borders)
        .border_type(BorderType::Rounded)
        // The border's cell stays the window's colour: a line runs through the
        // middle of its cell, and a darker fill behind it would square off the
        // rounded corners the whole look is built on.
        .border_style(Style::new().fg(palette::BORDER).bg(palette::WINDOW))
        .style(Style::new().bg(fill))
        .render(rect, buf);
}

pub(super) fn inner(rect: Rect) -> Rect {
    Rect::new(rect.x + 1, rect.y + 1, rect.width.saturating_sub(2), rect.height.saturating_sub(2))
}

/// Inside the sidebar, which has no right border of its own: the activity
/// bar's left border is its edge.
pub(super) fn sidebar_inner(rect: Rect) -> Rect {
    Rect::new(rect.x + 1, rect.y + 1, rect.width.saturating_sub(1), rect.height.saturating_sub(2))
}

/// A box's rows below its `header` rows, inside the border.
pub(super) fn list_area(rect: Rect, header: u16) -> Rect {
    let inner = inner(rect);
    let header = header.min(inner.height);
    Rect::new(inner.x, inner.y + header, inner.width, inner.height - header)
}

pub(super) fn title_pill(glyphs: Glyphs, title: &str, under: Color) -> Vec<Span<'static>> {
    theme::pill(glyphs, title.to_string(), palette::BRIGHT, palette::PILL, under)
}

/// Writes `spans` from `x`, at most `width` cells; returns where they ended.
pub(super) fn put(buf: &mut Buffer, x: u16, y: u16, width: u16, spans: Vec<Span<'_>>) -> u16 {
    buf.set_line(x, y, &Line::from(spans), width).0
}

pub(super) fn width_of(spans: &[Span<'_>]) -> u16 {
    spans.iter().map(Span::width).sum::<usize>() as u16
}

pub(super) fn divider(buf: &mut Buffer, x: u16, y: u16, width: u16) {
    buf.set_string(x, y, "\u{2500}".repeat(width as usize), Style::new().fg(palette::BORDER));
}

/// A border glyph where two borders meet.
pub(super) fn joint(buf: &mut Buffer, x: u16, y: u16, symbol: &str) {
    buf.set_string(x, y, symbol, Style::new().fg(palette::BORDER).bg(palette::WINDOW));
}

pub(super) fn scrollbar(buf: &mut Buffer, track: Rect, total: usize, offset: usize) {
    let height = track.height as usize;
    if total <= height || height == 0 {
        return;
    }
    let thumb = (height * height / total).max(1);
    let top = (offset * height / total).min(height - thumb);
    for at in top..top + thumb {
        buf.set_string(track.x, track.y + at as u16, "\u{2590}", Style::new().fg(palette::SLIDER));
    }
}

/// A rounded one-line field, as the command centre and the search box are:
/// an icon, then the text typed or a placeholder, and optionally a count at
/// the right end. Returns where the cursor goes while the field has the focus.
#[allow(clippy::too_many_arguments)]
pub(super) fn field(
    buf: &mut Buffer,
    glyphs: Glyphs,
    rect: Rect,
    focused: bool,
    icon: &str,
    typed: &str,
    placeholder: &str,
    count: Option<String>,
) -> Position {
    let fill = if focused { palette::PILL } else { palette::IDLE };
    let (body_x, body_width) = match glyphs {
        Glyphs::Nerd if rect.width > 4 => {
            let ends = Style::new().fg(fill).bg(palette::WINDOW);
            buf.set_string(rect.x, rect.y, "\u{e0b6}", ends);
            buf.set_string(rect.right() - 1, rect.y, "\u{e0b4}", ends);
            (rect.x + 1, rect.width - 2)
        }
        _ => (rect.x, rect.width),
    };
    buf.set_style(Rect::new(body_x, rect.y, body_width, 1), Style::new().bg(fill));
    let lit = if focused { palette::BRIGHT } else { palette::MUTED };
    buf.set_string(body_x + 1, rect.y, icon, Style::new().fg(lit));

    let text_x = body_x + 3;
    let mut limit = (body_x + body_width).saturating_sub(1);
    if let Some(count) = count {
        limit = limit.saturating_sub(count.width() as u16 + 1);
        buf.set_string(limit + 1, rect.y, count, muted());
    }
    let room = limit.saturating_sub(text_x) as usize;
    if typed.is_empty() {
        buf.set_stringn(text_x, rect.y, placeholder, room, muted());
        Position::new(text_x, rect.y)
    } else {
        let shown = tail(typed, room.saturating_sub(1));
        let colour = if focused { palette::BRIGHT } else { palette::TEXT };
        let (end, _) = buf.set_stringn(text_x, rect.y, shown, room, Style::new().fg(colour));
        Position::new(end, rect.y)
    }
}

pub(super) fn muted() -> Style {
    Style::new().fg(palette::MUTED)
}

pub(super) fn text(bright: bool) -> Style {
    Style::new().fg(if bright { palette::BRIGHT } else { palette::TEXT })
}

pub(super) fn selection(focused: bool) -> Color {
    if focused {
        palette::PILL
    } else {
        palette::IDLE
    }
}

/// The icon a tab shows for what it holds.
pub(super) fn doc_icon(app: &App, glyphs: Glyphs, doc: &Doc) -> (&'static str, Style) {
    let icons = glyphs.icons();
    match doc {
        Doc::Board => (icons.board, Style::new().fg(palette::LINK)),
        Doc::Welcome => (icons.welcome, Style::new().fg(palette::ACCENT)),
        Doc::Roadmap => (icons.roadmap, Style::new().fg(palette::GREEN)),
        Doc::Calendar => (icons.calendar, Style::new().fg(palette::YELLOW)),
        Doc::Item(key) => match app.snapshot.find(key) {
            Some((item, _)) => theme::item_look(glyphs, item),
            None => (icons.close, muted()),
        },
    }
}

/// The marks after an item's description: what it waits on, its date, its
/// priority and its star.
pub(super) fn meta(app: &App, glyphs: Glyphs, item: &Item, today: &str) -> Vec<Span<'static>> {
    let icons = glyphs.icons();
    let open = State::of(item).is_some_and(State::is_open);
    let mut spans = Vec::new();
    if let Some(blockers) = app.snapshot.blockers.get(&item.id) {
        spans.push(Span::styled(format!("{} {}", icons.waits, join(blockers)), Style::new().fg(palette::RED)));
    }
    if let Some(due) = &item.due_date {
        let late = open && due.as_str() < today;
        let colour = if late { palette::YELLOW } else { palette::MUTED };
        spans.push(Span::styled(format!("{} {}", icons.due, due.get(5..).unwrap_or(due)), Style::new().fg(colour)));
    }
    if let Some(priority) = item.priority.filter(|priority| *priority > 1 && item.is_task) {
        let colour = if priority > 2 { palette::RED } else { palette::YELLOW };
        spans.push(Span::styled(format!("p{priority}"), Style::new().fg(colour)));
    }
    if item.is_starred {
        spans.push(Span::styled(icons.star, Style::new().fg(palette::YELLOW)));
    }
    let mut spaced = Vec::new();
    for (at, span) in spans.into_iter().enumerate() {
        if at > 0 {
            spaced.push(Span::raw("  "));
        }
        spaced.push(span);
    }
    spaced
}

/// The totals the CLI prints under the board, in the same order and colours.
pub(super) fn stats_spans(stats: &Stats) -> Vec<Span<'static>> {
    let mut parts = vec![
        (format!("{} done", stats.complete), palette::GREEN),
        (format!("{} in progress", stats.in_progress), palette::ACCENT),
    ];
    if stats.paused > 0 {
        parts.push((format!("{} paused", stats.paused), palette::YELLOW));
    }
    parts.push((format!("{} pending", stats.pending), palette::TEXT));
    parts.push((format!("{} {}", stats.notes, if stats.notes == 1 { "note" } else { "notes" }), palette::LINK));
    let mut spans = Vec::new();
    for (at, (said, colour)) in parts.into_iter().enumerate() {
        if at > 0 {
            spans.push(Span::styled(" · ", muted()));
        }
        spans.push(Span::styled(said, Style::new().fg(colour)));
    }
    spans
}

pub(super) fn state_word(item: &Item) -> &'static str {
    match State::of(item) {
        None => "note",
        Some(State::Pending) => "pending",
        Some(State::Progress) => "in progress",
        Some(State::Paused) => "paused",
        Some(State::Done) => "done",
        Some(State::Cancelled) => "cancelled",
    }
}

/// The glyph and style of a listed entry, from the state word `agent` gives it.
pub(super) fn entry_look(glyphs: Glyphs, state: &str) -> (&'static str, Style) {
    let state = match state {
        "in progress" => Some(State::Progress),
        "paused" => Some(State::Paused),
        "done" => Some(State::Done),
        "cancelled" => Some(State::Cancelled),
        "note" => None,
        _ => Some(State::Pending),
    };
    theme::state_look(glyphs, state)
}

pub(super) fn plural(count: usize, one: &str) -> String {
    if count == 1 {
        format!("{count} {one}")
    } else {
        format!("{count} {one}s")
    }
}

pub(super) fn join(ids: &[u32]) -> String {
    ids.iter().map(u32::to_string).collect::<Vec<_>>().join(", ")
}

/// `text` on one line, cut to `width` cells with an ellipsis when it does not fit.
pub(super) fn clip(text: &str, width: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.width() <= width {
        return flat;
    }
    let mut clipped = String::new();
    let mut used = 0;
    for c in flat.chars() {
        let cells = c.width().unwrap_or(0);
        if used + cells + 1 > width {
            break;
        }
        clipped.push(c);
        used += cells;
    }
    if width > 0 {
        clipped.push('\u{2026}');
    }
    clipped
}

/// The end of `text` that fits in `width` cells, so a cursor stays in view.
pub(super) fn tail(text: &str, width: usize) -> String {
    let mut kept: Vec<char> = Vec::new();
    let mut used = 0;
    for c in text.chars().rev() {
        let cells = c.width().unwrap_or(0);
        if used + cells > width {
            break;
        }
        kept.push(c);
        used += cells;
    }
    kept.into_iter().rev().collect()
}

/// Wrapped on whitespace, not cut: where a long description is read whole.
pub(super) fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.width() + 1 + word.width() > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipping_counts_cells_and_wrapping_keeps_every_word() {
        let clipped = clip("日本語のテキスト", 7);
        assert!(clipped.width() <= 7 && clipped.ends_with('\u{2026}'), "{clipped:?}");
        assert_eq!(clip("short", 10), "short");

        let lines = wrap("damage is in surface coordinates not output coordinates", 20);
        assert!(lines.iter().all(|line| line.width() <= 20), "{lines:?}");
        assert_eq!(lines.join(" "), "damage is in surface coordinates not output coordinates");
        assert_eq!(tail("a long filter", 6), "filter");
        assert_eq!(plural(1, "result"), "1 result");
        assert_eq!(plural(3, "board"), "3 boards");
    }
}
