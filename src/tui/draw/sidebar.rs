//! The sidebar: whichever view the activity bar chose -- the explorer's tree,
//! search, the projects, or what changed this session.

use std::path::Path;

use chrono::TimeZone;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::Borders;
use unicode_width::UnicodeWidthStr;

use super::pieces::{card, clip, doc_icon, entry_look, field, muted, plural, put, scrollbar, selection, sidebar_inner, text, width_of};
use super::Hits;
use crate::tui::app::{App, SearchLine, Target, View};
use crate::tui::layout::Region;
use crate::tui::search;
use crate::tui::theme::{self, palette, Glyphs};
use crate::tui::tree::{Look, Mark, Node};

/// The rows under the view's title.
fn body(rect: Rect) -> Rect {
    let inner = sidebar_inner(rect);
    Rect::new(inner.x, inner.y + 1, inner.width, inner.height.saturating_sub(1))
}

/// Where each chip goes, row by row, as offsets from the left of `width`.
fn chip_layout(width: u16) -> Vec<Vec<(usize, u16)>> {
    let mut rows: Vec<Vec<(usize, u16)>> = vec![Vec::new()];
    let mut x = 0;
    for (at, chip) in search::CHIPS.iter().enumerate() {
        let chip_width = chip.width() as u16 + 2;
        if x > 0 && x + chip_width > width {
            rows.push(Vec::new());
            x = 0;
        }
        rows.last_mut().expect("a row was pushed").push((at, x));
        x += chip_width + 1;
    }
    rows
}

/// The search results' rows: below the field, the chips and the summary.
fn results_area(body: Rect) -> Rect {
    let top = body.y + 2 + chip_layout(body.width.saturating_sub(2)).len() as u16;
    Rect::new(body.x, top, body.width, body.bottom().saturating_sub(top))
}

fn local_time(millis: i64, format: &str) -> String {
    chrono::Local.timestamp_millis_opt(millis).single().map(|at| at.format(format).to_string()).unwrap_or_default()
}

/// Scrolls the view's list to its selection, before the frame is drawn.
pub(super) fn follow(app: &mut App, rect: Rect) {
    let body = body(rect);
    match app.view {
        View::Explorer => {
            let rows = app.tree_rows();
            app.tree.settle(&rows);
            app.tree.cursor.follow(body.height as usize);
        }
        View::Search => {
            let lines = app.search_lines().len();
            app.found.clamp(lines);
            app.found.follow(results_area(body).height as usize);
        }
        View::Projects => app.projects.follow(body.height as usize),
        View::Changes => app.changes.follow(body.height.saturating_sub(1) as usize),
        View::Graph => super::graph::follow_local(app, super::graph::local_canvas(body)),
    }
}

/// Draws the sidebar; returns where the cursor goes when the search box is focused.
pub(super) fn draw(buf: &mut Buffer, app: &App, glyphs: Glyphs, rect: Rect, hits: &mut Hits) -> Option<Position> {
    // No right border of its own: the activity bar's left border is its edge.
    card(buf, rect, palette::WINDOW, Borders::TOP | Borders::LEFT | Borders::BOTTOM);
    hits.push(rect, Target::Region(Region::Sidebar));
    let inner = sidebar_inner(rect);
    let icons = glyphs.icons();
    buf.set_string(inner.x + 1, inner.y, app.view.title(), Style::new().fg(palette::TEXT));
    buf.set_string(inner.right().saturating_sub(3), inner.y, icons.ellipsis, muted());

    let body = body(rect);
    let focused = app.focus == Region::Sidebar;
    match app.view {
        View::Explorer => {
            explorer(buf, app, glyphs, body, focused, hits);
            None
        }
        View::Search => search_view(buf, app, glyphs, body, focused, hits),
        View::Projects => {
            projects(buf, app, glyphs, body, focused, hits);
            None
        }
        View::Changes => {
            changes(buf, app, glyphs, body, focused, hits);
            None
        }
        View::Graph => {
            super::graph::local_view(buf, app, glyphs, body, hits);
            None
        }
    }
}

fn explorer(buf: &mut Buffer, app: &App, glyphs: Glyphs, body: Rect, focused: bool, hits: &mut Hits) {
    let icons = glyphs.icons();
    let rows = app.tree_rows();
    let width = body.width.saturating_sub(1);
    for (screen, (at, row)) in rows.iter().enumerate().skip(app.tree.cursor.offset).take(body.height as usize).enumerate() {
        let y = body.y + screen as u16;
        let line = Rect::new(body.x, y, width, 1);
        let selected = at == app.tree.cursor.selected;
        if selected {
            buf.set_style(line, Style::new().bg(selection(focused)));
        }

        let indent = 1 + row.depth * 2;
        let chevron = match (row.expandable, row.expanded) {
            (true, true) => icons.expanded,
            (true, false) => icons.collapsed,
            _ => " ",
        };
        let mut spans = vec![Span::styled(chevron, muted()), Span::raw(" ")];
        let mut label = text(selected && focused);
        match row.look {
            Look::Section => label = Style::new().fg(palette::TEXT).add_modifier(Modifier::BOLD),
            Look::Heading => label = muted(),
            Look::Item { id, archived } => {
                let item = if archived { app.snapshot.archived.get(&id) } else { app.snapshot.all.get(&id) };
                if let Some(item) = item {
                    let (glyph, look) = theme::item_look(glyphs, item);
                    spans.push(Span::styled(glyph, look));
                    spans.push(Span::raw(" "));
                    label = theme::description_style(item);
                    if selected && focused {
                        label = label.fg(palette::BRIGHT);
                    }
                }
            }
            Look::Phase(mark) => {
                let (glyph, colour) = match mark {
                    Mark::Behind => (icons.behind, palette::GREEN),
                    Mark::Here => (icons.here, palette::ACCENT),
                    Mark::Ahead => (icons.ahead, palette::MUTED),
                };
                spans.push(Span::styled(glyph, Style::new().fg(colour)));
                spans.push(Span::raw(" "));
            }
            Look::Tab => {
                if let Node::Tab(tab) = row.node {
                    if let Some(open) = app.tabs.open.get(tab) {
                        let (glyph, look) = doc_icon(app, glyphs, &open.doc);
                        spans.push(Span::styled(glyph, look));
                        spans.push(Span::raw(" "));
                    }
                }
            }
            Look::Folder => {}
        }
        let detail = row.detail.width() as u16;
        let used = indent + width_of(&spans);
        let room = width.saturating_sub(used + detail + 2) as usize;
        spans.push(Span::styled(clip(&row.label, room), label));
        put(buf, body.x + indent, y, width.saturating_sub(indent), spans);
        if detail > 0 {
            buf.set_string(line.right().saturating_sub(detail + 1), y, &row.detail, muted());
        }
        hits.push(line, Target::TreeRow(at));
    }
    scrollbar(buf, Rect::new(body.right().saturating_sub(1), body.y, 1, body.height), rows.len(), app.tree.cursor.offset);
}

fn search_view(buf: &mut Buffer, app: &App, glyphs: Glyphs, body: Rect, focused: bool, hits: &mut Hits) -> Option<Position> {
    let icons = glyphs.icons();
    let field_rect = Rect::new(body.x + 1, body.y, body.width.saturating_sub(3), 1);
    hits.push(field_rect, Target::SearchField);
    let cursor = field(buf, glyphs, field_rect, focused, icons.search, &app.search, "Search", None);

    let query = search::parse(&app.search);
    let layout = chip_layout(body.width.saturating_sub(2));
    for (row, chips) in layout.iter().enumerate() {
        let y = body.y + 1 + row as u16;
        for &(chip, offset) in chips {
            let name = search::CHIPS[chip];
            let on = query.attributes.iter().any(|attribute| attribute == name);
            let spans = if on {
                theme::pill(glyphs, name.to_string(), palette::BRIGHT, palette::PILL, palette::WINDOW)
            } else {
                vec![Span::styled(format!(" {name} "), muted())]
            };
            let chip_width = width_of(&spans);
            let x = body.x + 1 + offset;
            put(buf, x, y, chip_width, spans);
            hits.push(Rect::new(x, y, chip_width, 1), Target::Chip(chip));
        }
    }

    let lines = app.search_lines();
    let summary_y = body.y + 1 + layout.len() as u16;
    let (said, colour) = if !query.unknown.is_empty() {
        (format!("{} is not a filter", query.unknown.join(", ")), palette::YELLOW)
    } else if query.is_empty() {
        ("Words, #id, @board or is: filters".to_string(), palette::MUTED)
    } else {
        let boards = lines.iter().filter(|line| matches!(line, SearchLine::Board { .. })).count();
        let items = lines.len() - boards;
        (format!("{} in {}", plural(items, "result"), plural(boards, "board")), palette::MUTED)
    };
    let width = body.width.saturating_sub(2) as usize;
    buf.set_stringn(body.x + 1, summary_y, clip(&said, width), width, Style::new().fg(colour));

    let results = results_area(body);
    let line_width = body.width.saturating_sub(1);
    for (screen, (at, line)) in lines.iter().enumerate().skip(app.found.offset).take(results.height as usize).enumerate() {
        let y = results.y + screen as u16;
        let row = Rect::new(body.x, y, line_width, 1);
        match *line {
            SearchLine::Board { group, count } => {
                let name = &app.snapshot.groups[group].name;
                let spans = vec![
                    Span::styled(format!("{} ", icons.expanded), muted()),
                    Span::styled(name.clone(), Style::new().fg(palette::TEXT).add_modifier(Modifier::BOLD)),
                ];
                put(buf, body.x + 1, y, line_width.saturating_sub(1), spans);
                let count = count.to_string();
                buf.set_string(row.right().saturating_sub(count.len() as u16 + 1), y, count, muted());
            }
            SearchLine::Item(index) => {
                let selected = at == app.found.selected;
                if selected {
                    buf.set_style(row, Style::new().bg(selection(focused)));
                }
                let item = &app.snapshot.items[index];
                let (glyph, look) = theme::item_look(glyphs, item);
                let id = format!(" {} ", item.id);
                let mark = theme::mark(item);
                let room = (line_width as usize).saturating_sub(id.len() + 5 + mark.len());
                let mut description = theme::description_style(item);
                if selected && focused {
                    description = description.fg(palette::BRIGHT);
                }
                let spans = vec![
                    Span::styled(glyph, look),
                    Span::styled(id, muted()),
                    Span::styled(mark, theme::mark_style()),
                    Span::styled(clip(&item.description, room), description),
                ];
                put(buf, body.x + 3, y, line_width.saturating_sub(3), spans);
            }
        }
        hits.push(row, Target::SearchRow(at));
    }
    focused.then_some(cursor)
}

fn projects(buf: &mut Buffer, app: &App, glyphs: Glyphs, body: Rect, focused: bool, hits: &mut Hits) {
    let icons = glyphs.icons();
    if app.snapshot.projects.is_empty() {
        let said = "No projects yet: ekko init in a folder makes one.";
        for (at, line) in super::pieces::wrap(said, body.width.saturating_sub(2) as usize).iter().enumerate() {
            buf.set_string(body.x + 1, body.y + at as u16, line, muted());
        }
        return;
    }
    let width = body.width.saturating_sub(1);
    for (screen, (at, project)) in app.snapshot.projects.iter().enumerate().skip(app.projects.offset).take(body.height as usize).enumerate() {
        let y = body.y + screen as u16;
        let line = Rect::new(body.x, y, width, 1);
        let selected = at == app.projects.selected;
        if selected {
            buf.set_style(line, Style::new().bg(selection(focused)));
        }
        let current = project.name == app.workspace.name;
        let (detail, detail_colour) = match project.status {
            "here" => (format!("{}/{}", project.complete, project.tasks), palette::MUTED),
            other => (other.to_string(), palette::YELLOW),
        };
        let mut name = text(selected && focused);
        if current {
            name = name.add_modifier(Modifier::BOLD);
        }
        let room = width.saturating_sub(detail.len() as u16 + 6) as usize;
        let name_text = clip(&project.name, room);
        let path_room = room.saturating_sub(name_text.width() + 2);
        let path = project
            .path
            .as_deref()
            .map(|path| clip(&crate::tui::shorten(Path::new(path), &app.workspace.home), path_room))
            .unwrap_or_default();
        let marker = if current { Span::styled(icons.here, Style::new().fg(palette::ACCENT)) } else { Span::raw(" ") };
        let spans = vec![Span::raw(" "), marker, Span::raw(" "), Span::styled(name_text, name), Span::styled(format!("  {path}"), muted())];
        put(buf, body.x, y, width, spans);
        buf.set_string(line.right().saturating_sub(detail.len() as u16 + 1), y, detail, Style::new().fg(detail_colour));
        hits.push(line, Target::ProjectRow(at));
    }
}

fn changes(buf: &mut Buffer, app: &App, glyphs: Glyphs, body: Rect, focused: bool, hits: &mut Hits) {
    let since = local_time(app.since, "%H:%M");
    if app.snapshot.changes.is_empty() {
        buf.set_stringn(body.x + 1, body.y, format!("Nothing changed since {since}."), body.width.saturating_sub(2) as usize, muted());
        return;
    }
    buf.set_stringn(body.x + 1, body.y, format!("Since {since}"), body.width.saturating_sub(2) as usize, muted());
    let list = Rect::new(body.x, body.y + 1, body.width, body.height.saturating_sub(1));
    let width = list.width.saturating_sub(1);
    for (screen, (at, entry)) in app.snapshot.changes.iter().enumerate().skip(app.changes.offset).take(list.height as usize).enumerate() {
        let y = list.y + screen as u16;
        let line = Rect::new(list.x, y, width, 1);
        let selected = at == app.changes.selected;
        if selected {
            buf.set_style(line, Style::new().bg(selection(focused)));
        }
        let (glyph, look) = entry_look(glyphs, entry.state);
        let id = format!(" {} ", entry.id);
        let away = entry.away.map(|away| format!(" {away}")).unwrap_or_default();
        let room = (width as usize).saturating_sub(id.len() + away.len() + 9);
        let spans = vec![
            Span::styled(format!(" {} ", local_time(entry.updated_at, "%H:%M")), muted()),
            Span::styled(glyph, look),
            Span::styled(id, muted()),
            Span::styled(clip(&entry.description, room), text(selected && focused)),
            Span::styled(away, Style::new().fg(palette::YELLOW)),
        ];
        put(buf, list.x, y, width, spans);
        hits.push(line, Target::ChangeRow(at));
    }
}
