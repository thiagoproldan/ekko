//! The editor: its tabs, and the document each one shows -- the board, an
//! item, the roadmap, the calendar, and the Welcome page.

use std::path::Path;

use chrono::{Datelike, TimeZone};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Borders;
use unicode_width::UnicodeWidthStr;

use super::pieces::{
    card, clip, doc_icon, inner, list_area, meta, muted, plural, put, scrollbar, selection, state_word, stats_spans, text,
    width_of, wrap,
};
use super::Hits;
use crate::item::State;
use crate::render::{CalendarMonth, RoadmapStep};
use crate::tui::app::{relations, App, RoadmapLine, Target, Welcome};
use crate::tui::board::{self, Row, Severity};
use crate::tui::layout::Region;
use crate::tui::tabs::{Doc, Page};
use crate::tui::theme::{self, palette, Glyphs, Icons};

/// The document's rows, below the tabs and the breadcrumbs.
pub(super) fn body(rect: Rect) -> Rect {
    list_area(rect, 3)
}

fn item_width(body: Rect) -> usize {
    body.width.saturating_sub(4) as usize
}

/// Scrolls the active document to its cursor, before the frame is drawn.
pub(super) fn follow(app: &mut App, glyphs: Glyphs, rect: Rect) {
    let body = body(rect);
    let height = body.height as usize;
    app.page = height;
    match app.tabs.active().clone() {
        Doc::Board => app.board.follow(height),
        Doc::Item(key) => {
            let links = app.item_links(&key).len();
            app.doc.links.clamp(links);
            let lines = item_lines(app, glyphs, &key, item_width(body));
            // Only when the link cursor moved: scrolling by hand is not undone
            // on the next frame.
            if app.doc.followed != Some(app.doc.links.selected) {
                if let Some(at) = lines.iter().position(|line| line.link == Some(app.doc.links.selected)) {
                    if at < app.doc.scroll {
                        app.doc.scroll = at;
                    } else if at >= app.doc.scroll + height {
                        app.doc.scroll = at + 1 - height;
                    }
                }
                app.doc.followed = Some(app.doc.links.selected);
            }
            app.doc.scroll = app.doc.scroll.min(lines.len().saturating_sub(height));
        }
        Doc::Roadmap => {
            let lines = app.roadmap_lines().len();
            app.doc.roadmap.clamp(lines);
            app.doc.roadmap.follow(roadmap_list(app, body).height as usize);
        }
        Doc::Welcome => app.doc.welcome.clamp(app.welcome_links().len()),
        Doc::Calendar => {}
    }
}

pub(super) fn draw(buf: &mut Buffer, app: &App, glyphs: Glyphs, rect: Rect, hits: &mut Hits) {
    card(buf, rect, palette::EDITOR, Borders::ALL);
    hits.push(rect, Target::Region(Region::Editor));
    let inner = inner(rect);
    tab_strip(buf, app, glyphs, inner, hits);
    breadcrumbs(buf, app, glyphs, inner);
    let body = body(rect);
    match app.tabs.active() {
        Doc::Board => board_doc(buf, app, glyphs, body, hits),
        Doc::Item(key) => item_doc(buf, app, glyphs, key, body, hits),
        Doc::Roadmap => roadmap_doc(buf, app, glyphs, body, hits),
        Doc::Calendar => calendar_doc(buf, app, glyphs, body, hits),
        Doc::Welcome => welcome_doc(buf, app, glyphs, body, hits),
    }
}

/// VS Code's tabs: the active one a pill, a preview's title in italics, the
/// board pinned, and the editor's actions at the right end.
fn tab_strip(buf: &mut Buffer, app: &App, glyphs: Glyphs, inner: Rect, hits: &mut Hits) {
    let icons = glyphs.icons();
    let y = inner.y;
    let actions = [(icons.roadmap, Page::Roadmap), (icons.calendar, Page::Calendar), (icons.welcome, Page::Welcome)];
    let mut x = inner.right().saturating_sub(2);
    for (icon, page) in actions.iter().rev() {
        buf.set_string(x, y, *icon, muted());
        hits.push(Rect::new(x, y, 1, 1), Target::Page(*page));
        x = x.saturating_sub(2);
    }
    let limit = x.saturating_sub(1);

    let titles: Vec<(String, u16)> = app
        .tabs
        .open
        .iter()
        .map(|tab| {
            let title = clip(&app.snapshot.title(&tab.doc), 28);
            let width = title.width() as u16 + 6;
            (title, width)
        })
        .collect();
    // Start far enough along that the active tab fits.
    let available = limit.saturating_sub(inner.x + 1);
    let active = app.tabs.active;
    let mut start = 0;
    while start < active && titles[start..=active].iter().map(|(_, width)| width + 1).sum::<u16>() > available {
        start += 1;
    }

    let mut x = inner.x + 1;
    for (at, tab) in app.tabs.open.iter().enumerate().skip(start) {
        let (title, tab_width) = &titles[at];
        if x + tab_width > limit {
            break;
        }
        let is_active = at == active;
        let fill = |style: Style| if is_active { style.bg(palette::PILL) } else { style };
        let (glyph, look) = doc_icon(app, glyphs, &tab.doc);
        let mut title_style = if is_active { Style::new().fg(palette::BRIGHT) } else { muted() };
        if tab.preview {
            title_style = title_style.add_modifier(Modifier::ITALIC);
        }
        if matches!(&tab.doc, Doc::Item(key) if app.snapshot.find(key).is_none()) {
            title_style = title_style.add_modifier(Modifier::CROSSED_OUT);
        }
        let close = if tab.doc == Doc::Board {
            icons.pinned
        } else if is_active {
            icons.close
        } else {
            " "
        };
        let (left, right) = match (glyphs, is_active) {
            (Glyphs::Nerd, true) => {
                let ends = Style::new().fg(palette::PILL).bg(palette::EDITOR);
                (Span::styled("\u{e0b6}", ends), Span::styled("\u{e0b4}", ends))
            }
            _ => (Span::styled(" ", fill(Style::new())), Span::styled(" ", fill(Style::new()))),
        };
        let spans = vec![
            left,
            Span::styled(glyph, fill(look)),
            Span::styled(" ", fill(Style::new())),
            Span::styled(title.clone(), fill(title_style)),
            Span::styled(" ", fill(Style::new())),
            Span::styled(close, fill(muted())),
            right,
        ];
        put(buf, x, y, *tab_width, spans);
        hits.push(Rect::new(x, y, *tab_width, 1), Target::Tab(at));
        if is_active && tab.doc != Doc::Board {
            hits.push(Rect::new(x + tab_width - 2, y, 1, 1), Target::CloseTab(at));
        }
        x += tab_width + 1;
    }
}

fn breadcrumbs(buf: &mut Buffer, app: &App, glyphs: Glyphs, inner: Rect) {
    let icons = glyphs.icons();
    let separator = Span::styled(format!(" {} ", icons.collapsed), muted());
    let mut crumbs = vec![Span::styled(app.workspace.name.clone(), muted())];
    let mut right: Vec<Span<'static>> = Vec::new();
    let mut crumb = |said: String| {
        crumbs.push(separator.clone());
        crumbs.push(Span::styled(said, muted()));
    };
    match app.tabs.active() {
        // VS Code's Welcome page has no breadcrumbs.
        Doc::Welcome => return,
        Doc::Board => {
            crumb("Board".to_string());
            right = stats_spans(&app.snapshot.prime.stats);
        }
        Doc::Item(key) => match app.snapshot.find(key) {
            Some((item, archived)) => {
                let place = if archived { "Archive".to_string() } else { item.boards.first().cloned().unwrap_or_default() };
                crumb(place);
                crumb(format!("#{}", item.id));
            }
            None => crumb("Deleted item".to_string()),
        },
        Doc::Roadmap => {
            crumb("Roadmap".to_string());
            if !app.snapshot.roadmap.is_empty() {
                right = vec![Span::styled(plural(app.snapshot.roadmap.len(), "phase"), muted())];
            }
        }
        Doc::Calendar => crumb("Calendar".to_string()),
    }
    let y = inner.y + 1;
    let end = put(buf, inner.x + 2, y, inner.width.saturating_sub(4), crumbs);
    let right_width = width_of(&right);
    if right_width > 0 && end + 4 + right_width < inner.right() {
        put(buf, inner.right() - 2 - right_width, y, right_width, right);
    }
}

/// The board, grouped by board, where the writes are.
fn board_doc(buf: &mut Buffer, app: &App, glyphs: Glyphs, list: Rect, hits: &mut Hits) {
    let focused = app.focus == Region::Editor;
    if app.rows.is_empty() {
        let said = if app.filter.is_empty() {
            "The board is empty. Add to it from the CLI: ekko --task @board what needs doing".to_string()
        } else {
            format!("Nothing on the board matches \u{201c}{}\u{201d}.", app.filter)
        };
        buf.set_stringn(list.x + 2, list.y, said, list.width.saturating_sub(4) as usize, muted());
        return;
    }

    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let id_width = app.snapshot.items.iter().map(|item| item.id.to_string().len()).max().unwrap_or(1);
    let width = list.width.saturating_sub(1);
    for (screen, at) in (app.board.offset..app.rows.len()).take(list.height as usize).enumerate() {
        let y = list.y + screen as u16;
        let line = Rect::new(list.x, y, width, 1);
        match app.rows[at] {
            Row::Board(group) => {
                let group = &app.snapshot.groups[group];
                let spans = vec![
                    Span::styled(group.name.clone(), Style::new().fg(palette::TEXT).add_modifier(Modifier::BOLD)),
                    Span::styled(format!("  [{}/{}]", group.complete, group.tasks), muted()),
                ];
                put(buf, list.x + 2, y, width.saturating_sub(2), spans);
            }
            Row::Item { item, .. } => {
                let item = &app.snapshot.items[item];
                let selected = at == app.board.selected;
                if selected {
                    buf.set_style(line, Style::new().bg(selection(focused)));
                }
                let meta = meta(app, glyphs, item, &today);
                let meta_width = width_of(&meta);
                let indent: u16 = if item.attached_to.is_some() { 6 } else { 4 };
                let (glyph, look) = theme::item_look(glyphs, item);
                let prefix = id_width as u16 + 4;
                let room = width.saturating_sub(indent + prefix + meta_width + 3) as usize;
                let mut description = theme::description_style(item);
                if selected && focused {
                    description = description.fg(palette::BRIGHT);
                }
                let spans = vec![
                    Span::styled(format!("{:>id_width$}. ", item.id), muted()),
                    Span::styled(glyph, look),
                    Span::raw(" "),
                    Span::styled(clip(&item.description, room), description),
                ];
                put(buf, list.x + indent, y, width.saturating_sub(indent), spans);
                if meta_width > 0 {
                    put(buf, line.right().saturating_sub(meta_width + 1), y, meta_width, meta);
                }
            }
        }
        hits.push(line, Target::BoardRow(at));
    }
    scrollbar(buf, Rect::new(list.right().saturating_sub(1), list.y, 1, list.height), app.rows.len(), app.board.offset);
}

/// One line of an item's tab, and the relation it links to, if any.
pub(super) struct DocLine {
    pub line: Line<'static>,
    /// Its place among the item's links, for the keyboard's cursor.
    pub link: Option<usize>,
    pub key: Option<String>,
}

fn plain(line: Line<'static>) -> DocLine {
    DocLine { line, link: None, key: None }
}

fn local_time(millis: i64, format: &str) -> String {
    chrono::Local.timestamp_millis_opt(millis).single().map(|at| at.format(format).to_string()).unwrap_or_default()
}

/// How far off a date is, for a task still open.
fn relative(due: &str) -> String {
    let (Ok(due), today) = (chrono::NaiveDate::parse_from_str(due, "%Y-%m-%d"), chrono::Local::now().date_naive()) else {
        return String::new();
    };
    match (due - today).num_days() {
        0 => "  today".to_string(),
        1 => "  tomorrow".to_string(),
        days if days > 1 => format!("  in {days} days"),
        -1 => "  a day late".to_string(),
        days => format!("  {} days late", -days),
    }
}

/// An item's tab, line by line: its state, its text whole, its facts, and its
/// relations -- each one a link to that item's own tab.
pub(super) fn item_lines(app: &App, glyphs: Glyphs, key: &str, width: usize) -> Vec<DocLine> {
    let icons = glyphs.icons();
    let Some((item, archived)) = app.snapshot.find(key) else {
        return vec![plain(Line::styled("This item is no longer on the board, or in the archive.", muted()))];
    };
    let mut lines = Vec::new();

    let (glyph, look) = theme::item_look(glyphs, item);
    let mut header = vec![Span::styled(format!("{glyph} {}", state_word(item)), look), Span::styled(format!("   #{}", item.id), muted())];
    if archived {
        header.push(Span::styled("   archived", Style::new().fg(palette::YELLOW)));
    }
    if item.stashed.is_some() {
        header.push(Span::styled("   stashed", Style::new().fg(palette::YELLOW)));
    }
    if item.trashed.is_some() {
        header.push(Span::styled("   in the trash", Style::new().fg(palette::RED)));
    }
    if item.is_starred {
        header.push(Span::styled(format!("   {}", icons.star), Style::new().fg(palette::YELLOW)));
    }
    lines.push(plain(Line::from(header)));
    lines.push(plain(Line::default()));
    for said in wrap(&item.description, width.max(1)) {
        lines.push(plain(Line::styled(said, Style::new().fg(palette::BRIGHT))));
    }
    lines.push(plain(Line::default()));

    let fact = |name: &str, value: String, colour: Color| {
        plain(Line::from(vec![Span::styled(format!("{name:<10}"), muted()), Span::styled(value, Style::new().fg(colour))]))
    };
    lines.push(fact("Boards", item.boards.join("  "), palette::TEXT));
    if let Some(phase) = &item.phase {
        lines.push(fact("Phase", phase.clone(), palette::TEXT));
    }
    if let Some(priority) = item.priority.filter(|priority| *priority > 1 && item.is_task) {
        lines.push(fact("Priority", priority.to_string(), if priority > 2 { palette::RED } else { palette::YELLOW }));
    }
    if let Some(due) = &item.due_date {
        let open = State::of(item).is_some_and(State::is_open);
        let when = if open { relative(due) } else { String::new() };
        let late = open && when.ends_with("late");
        lines.push(fact("Due", format!("{due}{when}"), if late { palette::YELLOW } else { palette::TEXT }));
    }
    lines.push(fact("Created", item.date.clone(), palette::TEXT));
    if let Some(updated) = item.updated_at {
        lines.push(fact("Updated", local_time(updated, "%Y-%m-%d %H:%M"), palette::TEXT));
    }
    if let Some(uid) = &item.uid {
        lines.push(fact("uid", uid.clone(), palette::MUTED));
    }
    if archived {
        return lines;
    }

    let open_blockers = app.snapshot.blockers.get(&item.id);
    let mut link = 0;
    for (heading, ids) in relations(&app.snapshot, item.id) {
        if ids.is_empty() {
            continue;
        }
        lines.push(plain(Line::default()));
        lines.push(plain(Line::from(vec![
            Span::styled(heading, Style::new().fg(palette::TEXT).add_modifier(Modifier::BOLD)),
            Span::styled(format!("  {}", ids.len()), muted()),
        ])));
        for id in ids {
            let Some(other) = app.snapshot.all.get(&id) else { continue };
            let (glyph, look) = theme::item_look(glyphs, other);
            let key = board::key(other);
            let id_label = Span::styled(format!(" {:<5}", other.id), muted());
            if heading == "Notes" {
                // A note is read whole where it is attached.
                for (at, said) in wrap(&other.description, width.saturating_sub(9).max(1)).into_iter().enumerate() {
                    let spans = if at == 0 {
                        vec![Span::raw("  "), Span::styled(glyph, look), id_label.clone(), Span::styled(said, Style::new().fg(palette::TEXT))]
                    } else {
                        vec![Span::raw("         "), Span::styled(said, Style::new().fg(palette::TEXT))]
                    };
                    let first = at == 0;
                    lines.push(DocLine { line: Line::from(spans), link: first.then_some(link), key: first.then(|| key.clone()) });
                }
            } else {
                let mut spans = vec![
                    Span::raw("  "),
                    Span::styled(glyph, look),
                    id_label,
                    Span::styled(clip(&other.description, width.saturating_sub(22)), theme::description_style(other)),
                ];
                if heading == "Blocked by" && open_blockers.is_some_and(|open| open.contains(&id)) {
                    spans.push(Span::styled("  still open", Style::new().fg(palette::RED)));
                }
                lines.push(DocLine { line: Line::from(spans), link: Some(link), key: Some(key) });
            }
            link += 1;
        }
    }
    lines
}

fn item_doc(buf: &mut Buffer, app: &App, glyphs: Glyphs, key: &str, body: Rect, hits: &mut Hits) {
    let focused = app.focus == Region::Editor;
    let lines = item_lines(app, glyphs, key, item_width(body));
    for (screen, doc_line) in lines.iter().skip(app.doc.scroll).take(body.height as usize).enumerate() {
        let y = body.y + screen as u16;
        if let (Some(link), Some(key)) = (doc_line.link, &doc_line.key) {
            let row = Rect::new(body.x + 1, y, body.width.saturating_sub(3), 1);
            if link == app.doc.links.selected {
                buf.set_style(row, Style::new().bg(selection(focused)));
            }
            hits.key(row, key.clone());
        }
        buf.set_line(body.x + 2, y, &doc_line.line, body.width.saturating_sub(4));
    }
    scrollbar(buf, Rect::new(body.right().saturating_sub(1), body.y, 1, body.height), lines.len(), app.doc.scroll);
}

/// Where a phase stands, drawn as `--roadmap` draws it.
fn mark(icons: &Icons, step: &RoadmapStep) -> (&'static str, Color) {
    if step.current {
        (icons.here, palette::ACCENT)
    } else if step.total > 0 && step.complete == step.total {
        (icons.behind, palette::GREEN)
    } else {
        (icons.ahead, palette::MUTED)
    }
}

/// The lines under the roadmap: its notes and root items, and each dependency
/// that runs against the phase order.
fn roadmap_footer(app: &App) -> Vec<(String, Color)> {
    let mut lines = Vec::new();
    let notes: u32 = app.snapshot.roadmap.iter().map(|step| step.notes).sum();
    let mut tail = Vec::new();
    if notes > 0 {
        tail.push(plural(notes as usize, "note"));
    }
    if app.snapshot.rootless > 0 {
        tail.push(format!("{} outside any phase", app.snapshot.rootless));
    }
    if !tail.is_empty() {
        lines.push((tail.join(" · "), palette::MUTED));
    }
    for inversion in &app.snapshot.inversions {
        lines.push((
            format!(
                "{} in {} waits on {} in the later {}",
                inversion.blocked, inversion.blocked_phase, inversion.blocker, inversion.blocker_phase
            ),
            palette::YELLOW,
        ));
    }
    lines.truncate(4);
    lines
}

/// The roadmap's list, between its chain and its footer.
fn roadmap_list(app: &App, body: Rect) -> Rect {
    let top = 3.min(body.height);
    let footer = roadmap_footer(app).len() as u16;
    Rect::new(body.x, body.y + top, body.width, body.height.saturating_sub(top + footer))
}

fn roadmap_doc(buf: &mut Buffer, app: &App, glyphs: Glyphs, body: Rect, hits: &mut Hits) {
    let icons = glyphs.icons();
    if app.snapshot.roadmap.is_empty() {
        let said = "No phases yet -- declare them with ekko --phases <first> <second> ...";
        buf.set_stringn(body.x + 2, body.y, said, body.width.saturating_sub(4) as usize, muted());
        return;
    }

    // The chain, as `--roadmap` draws it.
    let mut chain = Vec::new();
    let mut counts = Vec::new();
    for (at, step) in app.snapshot.roadmap.iter().enumerate() {
        let (glyph, colour) = mark(icons, step);
        let label = format!("{} {glyph}", step.name);
        let tally = if step.current {
            format!("{}/{} HERE", step.complete, step.total)
        } else {
            format!("{}/{}", step.complete, step.total)
        };
        let width = label.width().max(tally.width()) + 2;
        if at > 0 {
            chain.push(Span::styled("\u{2500}\u{2500}\u{2500}", muted()));
            counts.push(Span::raw("   "));
        }
        let pad = width.saturating_sub(label.width());
        chain.push(Span::styled(format!("{label}{:pad$}", ""), Style::new().fg(colour)));
        let pad = width.saturating_sub(tally.width());
        counts.push(Span::styled(format!("{tally}{:pad$}", ""), if step.current { Style::new().fg(palette::ACCENT) } else { muted() }));
    }
    put(buf, body.x + 2, body.y, body.width.saturating_sub(4), chain);
    put(buf, body.x + 2, body.y + 1, body.width.saturating_sub(4), counts);

    let list = roadmap_list(app, body);
    let lines = app.roadmap_lines();
    let focused = app.focus == Region::Editor;
    let bar = 20usize;
    for (screen, (at, line)) in lines.iter().enumerate().skip(app.doc.roadmap.offset).take(list.height as usize).enumerate() {
        let y = list.y + screen as u16;
        let row = Rect::new(body.x + 1, y, body.width.saturating_sub(3), 1);
        let selected = at == app.doc.roadmap.selected;
        if selected {
            buf.set_style(row, Style::new().bg(selection(focused)));
        }
        match *line {
            RoadmapLine::Phase(index) => {
                let step = &app.snapshot.roadmap[index];
                let open = app.doc.open_phases.contains(&step.name);
                let (glyph, colour) = mark(icons, step);
                let filled = if step.total == 0 {
                    0
                } else {
                    (step.complete as usize * bar + step.total as usize / 2) / step.total as usize
                };
                let name = clip(&step.name, 18);
                let pad = 20usize.saturating_sub(name.width());
                let mut spans = vec![
                    Span::styled(if open { icons.expanded } else { icons.collapsed }, muted()),
                    Span::raw(" "),
                    Span::styled(glyph, Style::new().fg(colour)),
                    Span::raw(" "),
                    Span::styled(format!("{name}{:pad$}", ""), Style::new().fg(palette::TEXT).add_modifier(Modifier::BOLD)),
                    Span::styled("\u{2501}".repeat(filled), Style::new().fg(colour)),
                    Span::styled("\u{2501}".repeat(bar - filled), Style::new().fg(palette::BORDER)),
                    Span::styled(format!("  {}/{}", step.complete, step.total), muted()),
                ];
                if step.notes > 0 {
                    spans.push(Span::styled(format!(" · {}", plural(step.notes as usize, "note")), muted()));
                }
                put(buf, body.x + 2, y, body.width.saturating_sub(4), spans);
            }
            RoadmapLine::Item(id) => {
                if let Some(item) = app.snapshot.all.get(&id) {
                    let (glyph, look) = theme::item_look(glyphs, item);
                    let room = body.width.saturating_sub(18) as usize;
                    let mut description = theme::description_style(item);
                    if selected && focused {
                        description = description.fg(palette::BRIGHT);
                    }
                    let spans = vec![
                        Span::styled(glyph, look),
                        Span::styled(format!(" {:<5}", item.id), muted()),
                        Span::styled(clip(&item.description, room), description),
                    ];
                    put(buf, body.x + 8, y, body.width.saturating_sub(10), spans);
                }
            }
        }
        hits.push(row, Target::RoadmapRow(at));
    }

    for (at, (said, colour)) in roadmap_footer(app).iter().enumerate() {
        let width = body.width.saturating_sub(4) as usize;
        buf.set_stringn(body.x + 2, list.bottom() + at as u16, clip(said, width), width, Style::new().fg(*colour));
    }
}

/// The month as a grid, with what is due on each day in its cell.
fn calendar_doc(buf: &mut Buffer, app: &App, glyphs: Glyphs, body: Rect, hits: &mut Hits) {
    let icons = glyphs.icons();
    let (year, month) = app.doc.month;
    let today = chrono::Local::now().date_naive();
    let today_day = (today.year() == year && today.month() == month).then(|| today.day());
    let Some(grid) = CalendarMonth::of(year, month, today_day) else { return };
    let focused = app.focus == Region::Editor;

    let y = body.y;
    let mut x = body.x + 2;
    buf.set_string(x, y, icons.back, text(false));
    hits.push(Rect::new(x, y, 1, 1), Target::Month(-1));
    x += 2;
    buf.set_string(x, y, &grid.label, Style::new().fg(palette::BRIGHT).add_modifier(Modifier::BOLD));
    x += grid.label.width() as u16 + 1;
    buf.set_string(x, y, icons.collapsed, text(false));
    hits.push(Rect::new(x, y, 1, 1), Target::Month(1));
    let today_x = body.right().saturating_sub(8);
    buf.set_string(today_x, y, "Today", Style::new().fg(palette::LINK));
    hits.push(Rect::new(today_x, y, 5, 1), Target::Month(0));

    let left = body.x + 2;
    let cell_width = (body.width.saturating_sub(4) / 7).max(4);
    for (at, name) in ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"].iter().enumerate() {
        buf.set_stringn(left + at as u16 * cell_width + 1, body.y + 2, name, cell_width.saturating_sub(1) as usize, muted());
    }

    let top = body.y + 3;
    let weeks = grid.weeks.len().max(1) as u16;
    let cell_height = (body.bottom().saturating_sub(top) / weeks).max(1);
    let due = app.snapshot.due_in(year, month);
    let today_text = today.format("%Y-%m-%d").to_string();
    for (week, days) in grid.weeks.iter().enumerate() {
        for (column, day) in days.iter().enumerate() {
            let Some(day) = *day else { continue };
            let cell = Rect::new(left + column as u16 * cell_width, top + week as u16 * cell_height, cell_width.saturating_sub(1), cell_height);
            if cell.bottom() > body.bottom() {
                continue;
            }
            let selected = day == app.doc.day;
            if selected {
                buf.set_style(cell, Style::new().bg(selection(focused)));
            }
            hits.push(cell, Target::Day(day));

            let number = format!("{day:>2}");
            let number_style = if Some(day) == grid.today {
                Style::new().fg(palette::BRIGHT).bg(palette::ACCENT).add_modifier(Modifier::BOLD)
            } else if column == 0 || column == 6 {
                muted()
            } else {
                text(selected)
            };
            buf.set_string(cell.x + 1, cell.y, number, number_style);

            let Some(items) = due.get(&day) else { continue };
            let rows = cell.height.saturating_sub(1) as usize;
            let overflow = items.len() > rows;
            let shown = if overflow { rows.saturating_sub(1) } else { rows };
            for (at, item) in items.iter().take(shown).enumerate() {
                let y = cell.y + 1 + at as u16;
                let (glyph, look) = theme::item_look(glyphs, item);
                let late = State::of(item).is_some_and(State::is_open)
                    && item.due_date.as_deref().is_some_and(|due| due < today_text.as_str());
                let style = if late { Style::new().fg(palette::YELLOW) } else { theme::description_style(item) };
                let id = format!(" {} ", item.id);
                let room = cell.width.saturating_sub(2 + id.len() as u16) as usize;
                let spans = vec![Span::styled(glyph, look), Span::styled(id, muted()), Span::styled(clip(&item.description, room), style)];
                let row = Rect::new(cell.x + 1, y, cell.width.saturating_sub(1), 1);
                put(buf, row.x, y, row.width, spans);
                hits.key(row, board::key(item));
            }
            if overflow && rows > 0 {
                buf.set_string(cell.x + 1, cell.y + rows as u16, format!("+{} more", items.len() - shown), muted());
            }
        }
    }
}

/// VS Code's Welcome page, in ekko's terms: where to start, the projects on
/// this machine, and how far the board has come.
fn welcome_doc(buf: &mut Buffer, app: &App, glyphs: Glyphs, body: Rect, hits: &mut Hits) {
    let icons = glyphs.icons();
    let focused = app.focus == Region::Editor;
    let heading = Style::new().fg(palette::BRIGHT).add_modifier(Modifier::BOLD);
    let left = body.x + (body.width / 8).max(2);
    let right = body.x + body.width / 2 + 2;
    let column = right.saturating_sub(left + 2);
    let links = app.welcome_links();
    let link_style = |at: usize| {
        let style = Style::new().fg(palette::LINK);
        if focused && at == app.doc.welcome.selected {
            style.add_modifier(Modifier::UNDERLINED)
        } else {
            style
        }
    };

    let mut y = body.y + 1;
    buf.set_string(left, y, "Start", heading);
    y += 2;
    for (at, link) in links.iter().enumerate() {
        let (icon, said) = match link {
            Welcome::Page(Page::Board) => (icons.board, "Open the board"),
            Welcome::Page(Page::Roadmap) => (icons.roadmap, "Open the roadmap"),
            Welcome::Page(Page::Calendar) => (icons.calendar, "Open the calendar"),
            Welcome::Page(Page::Welcome) => (icons.welcome, "Welcome"),
            Welcome::View(_) => (icons.search, "Search the board"),
            Welcome::Problems => (icons.warning, "Show the problems"),
            Welcome::Project(_) => continue,
        };
        if y >= body.bottom() {
            break;
        }
        let spans = vec![Span::styled(icon, Style::new().fg(palette::LINK)), Span::raw(" "), Span::styled(said, link_style(at))];
        let width = width_of(&spans).min(column);
        put(buf, left, y, column, spans);
        hits.push(Rect::new(left, y, width, 1), Target::Welcome(at));
        y += 1;
    }

    y += 1;
    if y < body.bottom() {
        buf.set_string(left, y, "Recent", heading);
    }
    y += 2;
    if app.snapshot.projects.is_empty() && y < body.bottom() {
        buf.set_stringn(left, y, "No projects yet: ekko init in a folder makes one.", column as usize, muted());
    }
    for (at, link) in links.iter().enumerate() {
        let Welcome::Project(index) = link else { continue };
        if y >= body.bottom() {
            break;
        }
        let project = &app.snapshot.projects[*index];
        let path = project
            .path
            .as_deref()
            .map(|path| crate::tui::shorten(Path::new(path), &app.workspace.home))
            .unwrap_or_else(|| project.status.to_string());
        let spans = vec![Span::styled(project.name.clone(), link_style(at)), Span::styled(format!("   {path}"), muted())];
        put(buf, left, y, column, spans);
        hits.push(Rect::new(left, y, column, 1), Target::Welcome(at));
        y += 1;
    }

    if right + 16 >= body.right() {
        return;
    }
    let stats = &app.snapshot.prime.stats;
    let mut y = body.y + 1;
    buf.set_string(right, y, "Progress", heading);
    y += 2;
    let width = body.right().saturating_sub(right + 3).min(52);
    let inner_width = (width as usize).saturating_sub(2);
    let said = clip(&format!(" {} {}% of the work on this board is done", icons.done, stats.percent), inner_width);
    let pad = inner_width.saturating_sub(said.width());
    put(buf, right, y, width, theme::pill(glyphs, format!("{said}{:pad$}", ""), palette::TEXT, palette::PILL, palette::EDITOR));
    // The thin line under the pill, as VS Code draws a walkthrough's progress.
    let filled = (u32::from(width.saturating_sub(2)) * stats.percent / 100) as usize;
    buf.set_string(right + 1, y + 1, "\u{2594}".repeat(filled), Style::new().fg(palette::ACCENT));
    y += 3;
    let facts = [
        (format!("{} in progress", stats.in_progress), palette::ACCENT),
        (format!("{} ready to take up", app.snapshot.prime.ready.len()), palette::TEXT),
        (format!("{} blocked", app.snapshot.count(Severity::Blocked)), palette::RED),
        (plural(app.snapshot.problems.len(), "problem"), palette::YELLOW),
    ];
    for (said, colour) in facts {
        if y >= body.bottom() {
            break;
        }
        buf.set_stringn(right + 1, y, said, width as usize, Style::new().fg(colour));
        y += 1;
    }
}
