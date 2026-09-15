//! Drawing a frame: every box in VS Code's shape and colours.
//!
//! Each thing a click can land on is recorded in `App::hits` as it is drawn,
//! so a click finds exactly what was on screen under the pointer, and no
//! second copy of the geometry can drift from the first.

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Widget};
use ratatui::Frame;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::app::{App, Kind, Part, Tab, Target};
use super::board::{Row, Severity};
use super::layout::{self, Region, Regions, Visibility};
use super::theme::{self, palette, Glyphs};
use crate::item::{Item, State};
use crate::render::Stats;

type Hits = Vec<(Rect, Target)>;

/// Draws the whole frame for `app`, and records what a click can land on.
pub fn frame(frame: &mut Frame, app: &mut App, glyphs: Glyphs) {
    let area = frame.area();
    let Some(regions) = layout::regions(area, app.wanted) else {
        app.hits.clear();
        too_small(frame.buffer_mut(), area);
        return;
    };
    app.shown = Visibility {
        next: regions.next.is_some(),
        explorer: regions.explorer.is_some(),
        panel: regions.panel.is_some(),
    };
    let focus_shown = match app.focus {
        Region::Next => app.shown.next,
        Region::Explorer => app.shown.explorer,
        Region::Panel => app.shown.panel,
        Region::Command | Region::Editor => true,
    };
    if !focus_shown {
        app.focus = Region::Editor;
    }
    follow(app, glyphs, &regions);

    let mut hits = Hits::new();
    let cursor = {
        let view: &App = app;
        let buf = frame.buffer_mut();
        buf.set_style(area, Style::new().bg(palette::WINDOW).fg(palette::TEXT));
        let cursor = command(buf, view, glyphs, regions.command, &mut hits);
        if let Some(rect) = regions.next {
            next(buf, view, glyphs, rect, &mut hits);
        }
        editor(buf, view, glyphs, regions.editor, &mut hits);
        if let Some(rect) = regions.explorer {
            explorer(buf, view, glyphs, rect, &mut hits);
        }
        activity(buf, view, glyphs, regions.activity, regions.explorer, &mut hits);
        if let Some(rect) = regions.panel {
            panel(buf, view, glyphs, rect, &mut hits);
        }
        status(buf, view, glyphs, regions.status, &mut hits);
        cursor
    };
    app.hits = hits;
    if let Some(position) = cursor {
        frame.set_cursor_position(position);
    }
}

/// Scrolls every list to its selection, before anything is drawn from them.
fn follow(app: &mut App, glyphs: Glyphs, regions: &Regions) {
    let board = list_area(regions.editor, 3);
    app.page = board.height as usize;
    app.board.follow(board.height as usize);
    if let Some(rect) = regions.next {
        app.next.follow(list_area(rect, 3).height as usize);
    }
    if let Some(rect) = regions.explorer {
        let (tree, _) = explorer_split(app, glyphs, rect);
        app.explorer.follow(tree.height as usize);
    }
    if let Some(rect) = regions.panel {
        let height = list_area(rect, 2).height as usize;
        match app.tab {
            Tab::Problems => app.problems.follow(height),
            Tab::Output => app.scroll = app.scroll.min(app.log.len().saturating_sub(height)),
            Tab::Agent => {
                let lines = app.snapshot.prime.text().lines().count();
                app.scroll = app.scroll.min(lines.saturating_sub(height));
            }
        }
    }
}

// ---- the boxes ------------------------------------------------------------

/// The command centre: the filter, as a rounded field, and the layout toggles
/// beside it.
fn command(buf: &mut Buffer, app: &App, glyphs: Glyphs, rect: Rect, hits: &mut Hits) -> Option<Position> {
    let icons = glyphs.icons();
    let focused = app.focus == Region::Command;
    let fill = if focused { palette::PILL } else { palette::IDLE };
    hits.push((rect, Target::Command));

    let (body_x, body_width) = match glyphs {
        Glyphs::Nerd => {
            let ends = Style::new().fg(fill).bg(palette::WINDOW);
            buf.set_string(rect.x, rect.y, "\u{e0b6}", ends);
            buf.set_string(rect.right() - 1, rect.y, "\u{e0b4}", ends);
            (rect.x + 1, rect.width - 2)
        }
        Glyphs::Plain => (rect.x, rect.width),
    };
    buf.set_style(Rect::new(body_x, rect.y, body_width, 1), Style::new().bg(fill));
    let lit = if focused { palette::BRIGHT } else { palette::MUTED };
    buf.set_string(body_x + 1, rect.y, icons.search, Style::new().fg(lit));

    let text_x = body_x + 3;
    let mut limit = body_x + body_width - 1;
    if !app.filter.is_empty() {
        let found = app.rows.iter().filter(|row| matches!(row, Row::Item { .. })).count();
        let count = format!("{found} found");
        limit = limit.saturating_sub(count.width() as u16 + 1);
        buf.set_string(limit + 1, rect.y, count, muted());
    }
    let room = limit.saturating_sub(text_x) as usize;
    let cursor = if app.filter.is_empty() {
        let hint = if focused { "Filter by words, #id or @board" } else { app.workspace.name.as_str() };
        buf.set_stringn(text_x, rect.y, hint, room, muted());
        Position::new(text_x, rect.y)
    } else {
        let shown = tail(&app.filter, room.saturating_sub(1));
        let colour = if focused { palette::BRIGHT } else { palette::TEXT };
        let (end, _) = buf.set_stringn(text_x, rect.y, shown, room, Style::new().fg(colour));
        Position::new(end, rect.y)
    };

    let mut x = rect.right() + 2;
    let toggles = [
        (Part::Next, app.wanted.next, icons.left_on, icons.left_off),
        (Part::Panel, app.wanted.panel, icons.panel_on, icons.panel_off),
        (Part::Explorer, app.wanted.explorer, icons.right_on, icons.right_off),
    ];
    for (part, open, on, off) in toggles {
        if x + 1 >= buf.area.right() {
            break;
        }
        let colour = if open { palette::TEXT } else { palette::MUTED };
        buf.set_string(x, rect.y, if open { on } else { off }, Style::new().fg(colour));
        hits.push((Rect::new(x, rect.y, 1, 1), Target::Toggle(part)));
        x += 2;
    }
    focused.then_some(cursor)
}

/// The box left of the editor: what to take up next, best first.
fn next(buf: &mut Buffer, app: &App, glyphs: Glyphs, rect: Rect, hits: &mut Hits) {
    card(buf, rect, palette::WINDOW, Borders::ALL);
    hits.push((rect, Target::Region(Region::Next)));
    let inner = inner(rect);
    let focused = app.focus == Region::Next;

    put(buf, inner.x + 1, inner.y, inner.width.saturating_sub(2), title_pill(glyphs, "Next", palette::WINDOW));
    let (doing, ready) = (app.snapshot.prime.doing.len(), app.snapshot.prime.ready.len());
    let summary = format!("{doing} in progress · {ready} ready");
    buf.set_stringn(inner.x + 1, inner.y + 1, summary, inner.width.saturating_sub(2) as usize, muted());
    // Edge to edge and joined to the border, as the line under VS Code's view
    // title meets the box.
    divider(buf, inner.x, inner.y + 2, inner.width);
    let joint = Style::new().fg(palette::BORDER).bg(palette::WINDOW);
    buf.set_string(rect.x, inner.y + 2, "\u{251c}", joint);
    buf.set_string(rect.right() - 1, inner.y + 2, "\u{2524}", joint);

    let list = list_area(rect, 3);
    let entries = app.snapshot.next();
    if entries.is_empty() {
        buf.set_stringn(list.x + 1, list.y, "Nothing is in progress or ready.", list.width as usize - 2, muted());
        return;
    }
    let id_width = entries.iter().map(|entry| entry.id.to_string().len()).max().unwrap_or(1);
    for (screen, (at, entry)) in entries.iter().enumerate().skip(app.next.offset).take(list.height as usize).enumerate() {
        let y = list.y + screen as u16;
        let line = Rect::new(list.x, y, list.width, 1);
        let selected = at == app.next.selected;
        if selected {
            buf.set_style(line, Style::new().bg(selection(focused)));
        }
        let (glyph, look) = entry_look(glyphs, entry.state);
        let prefix = 1 + 1 + 1 + id_width + 1;
        let room = (list.width as usize).saturating_sub(prefix + 1);
        let spans = vec![
            Span::raw(" "),
            Span::styled(glyph, look),
            Span::styled(format!(" {:>id_width$} ", entry.id), muted()),
            Span::styled(clip(&entry.description, room), text(selected && focused)),
        ];
        put(buf, list.x, y, list.width, spans);
        hits.push((line, Target::NextRow(at)));
    }
}

/// The editor: the board, grouped by board, where the writes are.
fn editor(buf: &mut Buffer, app: &App, glyphs: Glyphs, rect: Rect, hits: &mut Hits) {
    card(buf, rect, palette::EDITOR, Borders::ALL);
    hits.push((rect, Target::Region(Region::Editor)));
    let inner = inner(rect);
    let icons = glyphs.icons();
    let focused = app.focus == Region::Editor;

    let tab = format!("{} Board", icons.board);
    put(buf, inner.x + 1, inner.y, inner.width.saturating_sub(2), title_pill(glyphs, &tab, palette::EDITOR));
    let crumbs = vec![
        Span::styled(app.workspace.name.clone(), muted()),
        Span::styled(format!(" {} ", icons.collapsed), muted()),
        Span::styled("Board", muted()),
    ];
    let crumbs_end = put(buf, inner.x + 2, inner.y + 1, inner.width.saturating_sub(4), crumbs);
    let stats = stats_spans(&app.snapshot.prime.stats);
    let stats_width = Line::from(stats.clone()).width() as u16;
    if crumbs_end + 4 + stats_width < inner.right() {
        put(buf, inner.right() - 2 - stats_width, inner.y + 1, stats_width, stats);
    }

    let list = list_area(rect, 3);
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
                let meta_width = Line::from(meta.clone()).width() as u16;
                let indent: u16 = if item.attached_to.is_some() { 6 } else { 4 };
                let (glyph, look) = theme::item_look(glyphs, item);
                let prefix = id_width as u16 + 2 + 2;
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
        hits.push((line, Target::BoardRow(at)));
    }
    scrollbar(buf, Rect::new(list.right() - 1, list.y, 1, list.height), app.rows.len(), app.board.offset);
}

/// The explorer: the project, its boards, and the selected item in full.
fn explorer(buf: &mut Buffer, app: &App, glyphs: Glyphs, rect: Rect, hits: &mut Hits) {
    // No right border of its own: the activity bar's left border is its edge.
    card(buf, rect, palette::WINDOW, Borders::TOP | Borders::LEFT | Borders::BOTTOM);
    hits.push((rect, Target::Region(Region::Explorer)));
    let inner = explorer_inner(rect);
    let icons = glyphs.icons();
    let focused = app.focus == Region::Explorer;
    let bold = Style::new().fg(palette::TEXT).add_modifier(Modifier::BOLD);

    buf.set_string(inner.x + 1, inner.y, "Explorer", Style::new().fg(palette::TEXT));
    buf.set_string(inner.right().saturating_sub(3), inner.y, icons.ellipsis, muted());
    let project = vec![Span::styled(format!("{} ", icons.expanded), muted()), Span::styled(app.workspace.name.clone(), bold)];
    put(buf, inner.x + 1, inner.y + 1, inner.width.saturating_sub(8), project);
    let percent = format!("{}%", app.snapshot.prime.stats.percent);
    buf.set_string(inner.right().saturating_sub(2 + percent.len() as u16), inner.y + 1, percent, muted());

    let (tree, item) = explorer_split(app, glyphs, rect);
    let width = tree.width.saturating_sub(1);
    for (screen, (at, group)) in
        app.snapshot.groups.iter().enumerate().skip(app.explorer.offset).take(tree.height as usize).enumerate()
    {
        let y = tree.y + screen as u16;
        let line = Rect::new(tree.x, y, width, 1);
        let selected = at == app.explorer.selected;
        if selected {
            buf.set_style(line, Style::new().bg(selection(focused)));
        }
        let count = format!("{}/{}", group.complete, group.tasks);
        let room = width.saturating_sub(count.len() as u16 + 8) as usize;
        let spans = vec![
            Span::styled(format!("{} ", icons.collapsed), muted()),
            Span::styled(clip(&group.name, room), text(selected && focused)),
        ];
        put(buf, tree.x + 3, y, width.saturating_sub(3), spans);
        buf.set_string(line.right().saturating_sub(count.len() as u16 + 1), y, count, muted());
        hits.push((line, Target::ExplorerRow(at)));
    }

    if item.height >= 3 {
        divider(buf, item.x + 1, item.y, item.width.saturating_sub(3));
        let header = vec![Span::styled(format!("{} ", icons.expanded), muted()), Span::styled("Item", bold)];
        put(buf, item.x + 1, item.y + 1, item.width.saturating_sub(2), header);
        let width = item.width.saturating_sub(5);
        for (at, line) in details(app, glyphs, width as usize).iter().take(item.height as usize - 2).enumerate() {
            buf.set_line(item.x + 3, item.y + 2 + at as u16, line, width);
        }
    }
}

/// The activity bar, down the right edge: one icon per view, the active one lit.
fn activity(buf: &mut Buffer, app: &App, glyphs: Glyphs, rect: Rect, explorer: Option<Rect>, hits: &mut Hits) {
    card(buf, rect, palette::WINDOW, Borders::ALL);
    if let Some(explorer) = explorer {
        let joint = Style::new().fg(palette::BORDER).bg(palette::WINDOW);
        buf.set_string(rect.x, rect.y, "\u{252c}", joint);
        buf.set_string(rect.x, explorer.bottom() - 1, "\u{2524}", joint);
    }
    let icons = glyphs.icons();
    // One view lit at a time, as in VS Code. Search is not a view of its own
    // yet: it takes the focus to the command centre.
    let views = [
        (icons.explorer, app.shown.explorer, Target::Toggle(Part::Explorer)),
        (icons.search, false, Target::Command),
    ];
    let x = rect.x + 1;
    for (at, (icon, active, target)) in views.into_iter().enumerate() {
        let y = rect.y + 1 + at as u16 * 2;
        if y + 1 >= rect.bottom() {
            break;
        }
        if active {
            match glyphs {
                Glyphs::Nerd => {
                    let ends = Style::new().fg(palette::ACTIVE).bg(palette::WINDOW);
                    buf.set_string(x, y, "\u{e0b6}", ends);
                    buf.set_string(x + 1, y, icon, Style::new().fg(palette::BRIGHT).bg(palette::ACTIVE));
                    buf.set_string(x + 2, y, "\u{e0b4}", ends);
                }
                Glyphs::Plain => {
                    buf.set_string(x, y, format!(" {icon} "), Style::new().fg(palette::BRIGHT).bg(palette::ACTIVE));
                }
            }
        } else {
            buf.set_string(x + 1, y, icon, muted());
        }
        hits.push((Rect::new(x, y, 3, 1), target));
    }
}

/// The bottom panel: Problems, Output and Agent.
fn panel(buf: &mut Buffer, app: &App, glyphs: Glyphs, rect: Rect, hits: &mut Hits) {
    card(buf, rect, palette::WINDOW, Borders::ALL);
    hits.push((rect, Target::Region(Region::Panel)));
    let inner = inner(rect);
    let icons = glyphs.icons();

    let close = inner.right().saturating_sub(2);
    let mut x = inner.x + 1;
    for tab in Tab::ALL {
        let badge = match tab {
            Tab::Problems => app.snapshot.problems.len(),
            Tab::Output => app.unseen,
            Tab::Agent => 0,
        };
        let title = if badge > 0 { format!("{} {badge}", tab.title()) } else { tab.title().to_string() };
        let spans = if tab == app.tab {
            theme::pill(glyphs, title, palette::BRIGHT, palette::PILL, palette::WINDOW)
        } else {
            vec![Span::styled(format!(" {title} "), muted())]
        };
        let width = Line::from(spans.clone()).width() as u16;
        if x + width + 2 >= close {
            break;
        }
        put(buf, x, inner.y, width, spans);
        hits.push((Rect::new(x, inner.y, width, 1), Target::PanelTab(tab)));
        x += width + 1;
    }
    buf.set_string(close, inner.y, icons.close, muted());
    hits.push((Rect::new(close, inner.y, 1, 1), Target::Toggle(Part::Panel)));

    let body = list_area(rect, 2);
    match app.tab {
        Tab::Problems => problems(buf, app, glyphs, body, hits),
        Tab::Output => output(buf, app, body),
        Tab::Agent => {
            let text = app.snapshot.prime.text();
            for (at, line) in text.lines().skip(app.scroll).take(body.height as usize).enumerate() {
                let width = body.width.saturating_sub(3) as usize;
                buf.set_stringn(body.x + 2, body.y + at as u16, line, width, Style::new().fg(palette::TEXT));
            }
        }
    }
}

fn problems(buf: &mut Buffer, app: &App, glyphs: Glyphs, body: Rect, hits: &mut Hits) {
    let icons = glyphs.icons();
    let problems = &app.snapshot.problems;
    if problems.is_empty() {
        let said = "No problems have been detected on the board.";
        buf.set_stringn(body.x + 2, body.y, said, body.width.saturating_sub(3) as usize, muted());
        return;
    }
    let focused = app.focus == Region::Panel;
    for (screen, (at, problem)) in problems.iter().enumerate().skip(app.problems.offset).take(body.height as usize).enumerate() {
        let y = body.y + screen as u16;
        let line = Rect::new(body.x, y, body.width, 1);
        let selected = at == app.problems.selected;
        if selected {
            buf.set_style(line, Style::new().bg(selection(focused)));
        }
        let (glyph, colour) = match problem.severity {
            Severity::Blocked => (icons.blocked, palette::RED),
            Severity::Warning => (icons.warning, palette::YELLOW),
        };
        let detail = format!("  {}  #{}", problem.detail, problem.id);
        let room = (body.width as usize).saturating_sub(detail.width() + 6).max(12);
        let spans = vec![
            Span::styled(glyph, Style::new().fg(colour)),
            Span::raw(" "),
            Span::styled(clip(&problem.description, room), text(selected && focused)),
            Span::styled(detail, muted()),
        ];
        put(buf, body.x + 2, y, body.width.saturating_sub(3), spans);
        hits.push((line, Target::ProblemRow(at)));
    }
}

/// What this session did and saw, newest at the bottom, as a log reads.
fn output(buf: &mut Buffer, app: &App, body: Rect) {
    if app.log.is_empty() {
        let said = "Nothing has happened in this session yet.";
        buf.set_stringn(body.x + 2, body.y, said, body.width.saturating_sub(3) as usize, muted());
        return;
    }
    let end = app.log.len().saturating_sub(app.scroll);
    let start = end.saturating_sub(body.height as usize);
    for (at, logged) in app.log[start..end].iter().enumerate() {
        let colour = match logged.kind {
            Kind::Did => palette::TEXT,
            Kind::Refused => palette::YELLOW,
            Kind::Outside => palette::LINK,
        };
        let spans = vec![Span::styled(format!("{}  ", logged.at), muted()), Span::styled(logged.text.clone(), Style::new().fg(colour))];
        put(buf, body.x + 2, body.y + at as u16, body.width.saturating_sub(3), spans);
    }
}

/// The status bar: where this is, whether it is live, what is wrong, what is next.
fn status(buf: &mut Buffer, app: &App, glyphs: Glyphs, rect: Rect, hits: &mut Hits) {
    let icons = glyphs.icons();
    let y = rect.y;

    let bell_x = rect.right().saturating_sub(3);
    let (bell, bell_colour) = if app.unseen > 0 { (icons.bell_dot, palette::LINK) } else { (icons.bell, palette::MUTED) };
    buf.set_string(bell_x, y, bell, Style::new().fg(bell_colour));
    hits.push((Rect::new(bell_x.saturating_sub(1), y, 3, 1), Target::Bell));

    let mut limit = bell_x.saturating_sub(2);
    if let Some((said, kind)) = app.flash() {
        let colour = if kind == Kind::Refused { palette::YELLOW } else { palette::TEXT };
        let shown = clip(said, (limit.saturating_sub(rect.x) / 2) as usize);
        let x = limit.saturating_sub(shown.width() as u16);
        buf.set_string(x, y, &shown, Style::new().fg(colour));
        limit = x.saturating_sub(2);
    }

    let mut segments: Vec<(Vec<Span<'static>>, Option<Target>)> = Vec::new();
    if let Some(branch) = &app.workspace.branch {
        segments.push((vec![Span::styled(format!("{} {branch}", icons.branch), muted())], None));
    }
    let live = if app.pulsing() { palette::LINK } else { palette::MUTED };
    segments.push((vec![Span::styled(icons.sync, Style::new().fg(live))], None));
    let counters = format!(
        "{} {}  {} {}",
        icons.blocked,
        app.snapshot.count(Severity::Blocked),
        icons.warning,
        app.snapshot.count(Severity::Warning)
    );
    segments.push((vec![Span::styled(counters, muted())], Some(Target::PanelTab(Tab::Problems))));
    if let Some(entry) = app.snapshot.next().first() {
        let said = format!("{} {} {}", icons.rocket, entry.id, clip(&entry.description, 36));
        segments.push((vec![Span::styled(said, muted())], Some(Target::NextRow(0))));
    }
    segments.push((vec![Span::styled(format!("{} {}", icons.folder, app.workspace.folder), muted())], None));

    let mut x = rect.x + 2;
    for (spans, target) in segments {
        let width = Line::from(spans.clone()).width() as u16;
        if x + width > limit {
            break;
        }
        put(buf, x, y, width, spans);
        if let Some(target) = target {
            hits.push((Rect::new(x, y, width, 1), target));
        }
        x += width + 2;
    }
}

fn too_small(buf: &mut Buffer, area: Rect) {
    buf.set_style(area, Style::new().bg(palette::WINDOW));
    let said = format!(
        "ekko needs {}\u{d7}{} or more; this terminal is {}\u{d7}{}",
        layout::MIN_WIDTH,
        layout::MIN_HEIGHT,
        area.width,
        area.height
    );
    let x = area.x + area.width.saturating_sub(said.width() as u16) / 2;
    buf.set_stringn(x, area.y + area.height / 2, said, area.width as usize, muted());
}

// ---- pieces -----------------------------------------------------------------

fn card(buf: &mut Buffer, rect: Rect, fill: Color, borders: Borders) {
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

fn inner(rect: Rect) -> Rect {
    Rect::new(rect.x + 1, rect.y + 1, rect.width.saturating_sub(2), rect.height.saturating_sub(2))
}

fn explorer_inner(rect: Rect) -> Rect {
    Rect::new(rect.x + 1, rect.y + 1, rect.width.saturating_sub(1), rect.height.saturating_sub(2))
}

/// A box's rows below its `header` rows, inside the border.
fn list_area(rect: Rect, header: u16) -> Rect {
    let inner = inner(rect);
    let header = header.min(inner.height);
    Rect::new(inner.x, inner.y + header, inner.width, inner.height - header)
}

/// The explorer's board tree, and the item section under it -- as much room
/// as the item needs, up to half the box.
fn explorer_split(app: &App, glyphs: Glyphs, rect: Rect) -> (Rect, Rect) {
    let inner = explorer_inner(rect);
    let body = inner.height.saturating_sub(2);
    let wanted = details(app, glyphs, inner.width.saturating_sub(5) as usize).len() as u16 + 2;
    let item_height = wanted.min(body / 2);
    let tree = Rect::new(inner.x, inner.y + 2, inner.width, body - item_height);
    let item = Rect::new(inner.x, tree.bottom(), inner.width, item_height);
    (tree, item)
}

fn title_pill(glyphs: Glyphs, title: &str, under: Color) -> Vec<Span<'static>> {
    theme::pill(glyphs, title.to_string(), palette::BRIGHT, palette::PILL, under)
}

/// Writes `spans` from `x`, at most `width` cells; returns where they ended.
fn put(buf: &mut Buffer, x: u16, y: u16, width: u16, spans: Vec<Span<'_>>) -> u16 {
    buf.set_line(x, y, &Line::from(spans), width).0
}

fn divider(buf: &mut Buffer, x: u16, y: u16, width: u16) {
    buf.set_string(x, y, "\u{2500}".repeat(width as usize), Style::new().fg(palette::BORDER));
}

fn scrollbar(buf: &mut Buffer, track: Rect, total: usize, offset: usize) {
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

fn muted() -> Style {
    Style::new().fg(palette::MUTED)
}

fn text(bright: bool) -> Style {
    Style::new().fg(if bright { palette::BRIGHT } else { palette::TEXT })
}

fn selection(focused: bool) -> Color {
    if focused {
        palette::PILL
    } else {
        palette::IDLE
    }
}

/// The marks after an item's description: what it waits on, its date, its
/// priority and its star.
fn meta(app: &App, glyphs: Glyphs, item: &Item, today: &str) -> Vec<Span<'static>> {
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

/// The selected item in full, for the explorer's item section.
fn details(app: &App, glyphs: Glyphs, width: usize) -> Vec<Line<'static>> {
    let Some(at) = app.selected() else {
        return vec![Line::styled("Nothing selected", muted())];
    };
    let item = &app.snapshot.items[at];
    let (glyph, look) = theme::item_look(glyphs, item);
    let mut lines =
        vec![Line::from(vec![Span::styled(format!("{glyph} {}", state_word(item)), look), Span::styled(format!("  #{}", item.id), muted())])];
    for said in wrap(&item.description, width.max(1)) {
        lines.push(Line::styled(said, Style::new().fg(palette::TEXT)));
    }

    let mut facts = vec![(item.boards.join(" "), palette::MUTED)];
    if let Some(phase) = &item.phase {
        facts.push((format!("phase {phase}"), palette::MUTED));
    }
    if let Some(due) = &item.due_date {
        facts.push((format!("due {due}"), palette::MUTED));
    }
    if let Some(priority) = item.priority.filter(|priority| *priority > 1 && item.is_task) {
        facts.push((format!("priority {priority}"), palette::MUTED));
    }
    if item.is_starred {
        facts.push(("starred".to_string(), palette::YELLOW));
    }
    if let Some(blockers) = app.snapshot.blockers.get(&item.id) {
        facts.push((format!("blocked by {}", join(blockers)), palette::RED));
    }
    if let Some(count) = item.uid.as_ref().and_then(|uid| app.snapshot.attached.get(uid)) {
        let noun = if *count == 1 { "note" } else { "notes" };
        facts.push((format!("{count} {noun} attached"), palette::LINK));
    }
    if item.attached_to.is_some() {
        facts.push(("attached to a task".to_string(), palette::LINK));
    }
    facts.push((format!("created {}", item.date), palette::MUTED));
    lines.extend(facts.into_iter().map(|(said, colour)| Line::styled(clip(&said, width), Style::new().fg(colour))));
    lines
}

/// The totals the CLI prints under the board, in the same order and colours.
fn stats_spans(stats: &Stats) -> Vec<Span<'static>> {
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

fn state_word(item: &Item) -> &'static str {
    match State::of(item) {
        None => "note",
        Some(State::Pending) => "pending",
        Some(State::Progress) => "in progress",
        Some(State::Paused) => "paused",
        Some(State::Done) => "done",
        Some(State::Cancelled) => "cancelled",
    }
}

/// The glyph and style of a queue entry, from the state word `agent` gives it.
fn entry_look(glyphs: Glyphs, state: &str) -> (&'static str, Style) {
    let icons = glyphs.icons();
    match state {
        "in progress" => (icons.progress, Style::new().fg(palette::ACCENT)),
        "paused" => (icons.paused, Style::new().fg(palette::YELLOW)),
        _ => (icons.pending, muted()),
    }
}

fn join(ids: &[u32]) -> String {
    ids.iter().map(u32::to_string).collect::<Vec<_>>().join(", ")
}

/// `text` on one line, cut to `width` cells with an ellipsis when it does not fit.
fn clip(text: &str, width: usize) -> String {
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

/// The end of `text` that fits in `width` cells, so the cursor stays in view.
fn tail(text: &str, width: usize) -> String {
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

/// Wrapped on whitespace, not cut: the explorer's item section is where a
/// long description is read whole.
fn wrap(text: &str, width: usize) -> Vec<String> {
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
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    use super::*;
    use crate::tui::app::Workspace;
    use crate::tui::board::tests::{board, words};
    use crate::tui::board::Snapshot;

    fn drawn(app: &mut App, glyphs: Glyphs, width: u16, height: u16) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| frame(f, app, glyphs)).unwrap();
        terminal.backend().buffer().clone()
    }

    fn row(buf: &Buffer, y: u16) -> String {
        (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()
    }

    fn screen(buf: &Buffer) -> String {
        (0..buf.area.height).map(|y| row(buf, y)).collect::<Vec<_>>().join("\n")
    }

    fn sample(tag: &str) -> (std::path::PathBuf, App) {
        let (dir, ekko) = board(tag);
        ekko.create_task(&words(&["@render", "wire the watcher", "p:2"])).unwrap();
        ekko.create_task(&words(&["@render", "draw the frame", "d:2020-01-01"])).unwrap();
        ekko.create_task(&words(&["@docs", "rewrite the interactive mode section"])).unwrap();
        ekko.set_blocked_by(&words(&["@3", "1"])).unwrap();
        let workspace = Workspace {
            label: "project demo".into(),
            name: "demo".into(),
            folder: "~/Projects/demo".into(),
            cwd: dir.clone(),
            branch: Some("main".into()),
        };
        let app = App::new(workspace, Snapshot::load(&ekko, "project demo").unwrap());
        (dir, app)
    }

    /// Rounded boxes, the explorer meeting the activity bar in a joint rather
    /// than a corner, and the board, the queue and the status bar all drawn.
    #[test]
    fn a_wide_frame_draws_every_box_and_its_joints() {
        let (dir, mut app) = sample("wide");
        let buf = drawn(&mut app, Glyphs::Nerd, 170, 46);
        let regions = layout::regions(buf.area, app.wanted).unwrap();

        let editor = regions.editor;
        assert_eq!(buf[(editor.x, editor.y)].symbol(), "\u{256d}");
        assert_eq!(buf[(editor.right() - 1, editor.bottom() - 1)].symbol(), "\u{256f}");
        let explorer = regions.explorer.unwrap();
        assert_eq!(buf[(regions.activity.x, regions.activity.y)].symbol(), "\u{252c}");
        assert_eq!(buf[(regions.activity.x, explorer.bottom() - 1)].symbol(), "\u{2524}");
        assert_eq!(buf[(regions.activity.x, regions.activity.bottom() - 1)].symbol(), "\u{2570}");
        assert_eq!(buf[(editor.x + 1, editor.y + 1)].bg, palette::EDITOR);

        let all = screen(&buf);
        for expected in ["wire the watcher", "@render", "[0/2]", "Explorer", "Problems 2", "~/Projects/demo", "main"] {
            assert!(all.contains(expected), "{expected:?} is not on screen:\n{all}");
        }
        assert!(row(&buf, 45).contains("main"), "the branch is not in the status bar");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Every click target lies on the screen, and the board's rows are where
    /// their items were drawn.
    #[test]
    fn click_targets_are_where_their_things_were_drawn() {
        let (dir, mut app) = sample("targets");
        let buf = drawn(&mut app, Glyphs::Nerd, 170, 46);
        for (rect, target) in &app.hits {
            assert!(rect.right() <= buf.area.width && rect.bottom() <= buf.area.height, "{target:?} at {rect:?}");
        }
        let (rect, _) = app
            .hits
            .iter()
            .find(|(_, target)| *target == Target::BoardRow(app.board.selected))
            .expect("the selected row is not clickable");
        assert!(row(&buf, rect.y).contains("wire the watcher"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_small_terminal_says_so_instead_of_drawing_boxes() {
        let (dir, mut app) = sample("small");
        let buf = drawn(&mut app, Glyphs::Nerd, 50, 12);
        assert!(screen(&buf).contains("ekko needs 60\u{d7}16 or more"));
        assert!(app.hits.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Without a Nerd Font nothing from the private use area reaches the screen.
    #[test]
    fn plain_glyphs_draw_nothing_a_plain_font_lacks() {
        let (dir, mut app) = sample("plain");
        let buf = drawn(&mut app, Glyphs::Plain, 170, 46);
        let private = screen(&buf).chars().filter(|c| ('\u{e000}'..='\u{f8ff}').contains(c)).count();
        assert_eq!(private, 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn clipping_counts_cells_and_wrapping_keeps_every_word() {
        let clipped = clip("日本語のテキスト", 7);
        assert!(clipped.width() <= 7 && clipped.ends_with('\u{2026}'), "{clipped:?}");
        assert_eq!(clip("short", 10), "short");

        let lines = wrap("damage is in surface coordinates not output coordinates", 20);
        assert!(lines.iter().all(|line| line.width() <= 20), "{lines:?}");
        assert_eq!(lines.join(" "), "damage is in surface coordinates not output coordinates");
        assert_eq!(tail("a long filter", 6), "filter");
    }
}
