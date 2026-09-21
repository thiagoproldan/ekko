//! Drawing a frame: every box in VS Code's shape and colours.
//!
//! Each thing a click can land on is recorded as it is drawn, so a click finds
//! exactly what was on screen under the pointer, and no second copy of the
//! geometry can drift from the first.

mod editor;
mod graph;
mod pieces;
mod sidebar;

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;
use ratatui::text::Span;
use ratatui::widgets::Borders;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use self::pieces::{card, clip, divider, entry_look, field, inner, joint, list_area, muted, put, selection, text, title_pill, width_of};
use super::app::{App, Kind, PanelTab, Part, Target, View};
use super::board::{Row, Severity};
use super::layout::{self, Region, Regions, Visibility};
use super::list::Cursor;
use super::theme::{self, palette, Glyphs};

/// What a click can land on, gathered while a frame is drawn.
#[derive(Default)]
struct Hits {
    targets: Vec<(Rect, Target)>,
    keys: Vec<String>,
}

impl Hits {
    fn push(&mut self, rect: Rect, target: Target) {
        self.targets.push((rect, target));
    }

    /// A click on `rect` opens the item `key` names.
    fn key(&mut self, rect: Rect, key: String) {
        self.targets.push((rect, Target::Key(self.keys.len())));
        self.keys.push(key);
    }
}

/// Draws the whole frame for `app`, and records what a click can land on.
pub fn frame(frame: &mut Frame, app: &mut App, glyphs: Glyphs) {
    let area = frame.area();
    let Some(regions) = layout::regions(area, app.wanted) else {
        app.hits.clear();
        app.hit_keys.clear();
        too_small(frame.buffer_mut(), area);
        return;
    };
    app.shown = Visibility {
        next: regions.next.is_some(),
        sidebar: regions.sidebar.is_some(),
        panel: regions.panel.is_some(),
    };
    let focus_shown = match app.focus {
        Region::Next => app.shown.next,
        Region::Sidebar => app.shown.sidebar,
        Region::Panel => app.shown.panel,
        Region::Command | Region::Editor => true,
    };
    if !focus_shown {
        app.focus = Region::Editor;
    }
    follow(app, glyphs, &regions);

    let mut hits = Hits::default();
    let cursor = {
        let view: &App = app;
        let buf = frame.buffer_mut();
        buf.set_style(area, Style::new().bg(palette::WINDOW).fg(palette::TEXT));
        let command = command(buf, view, glyphs, regions.command, &mut hits);
        if let Some(rect) = regions.next {
            next(buf, view, glyphs, rect, &mut hits);
        }
        editor::draw(buf, view, glyphs, regions.editor, &mut hits);
        let search = regions.sidebar.and_then(|rect| sidebar::draw(buf, view, glyphs, rect, &mut hits));
        activity(buf, view, glyphs, regions.activity, regions.sidebar, &mut hits);
        if let Some(rect) = regions.panel {
            panel(buf, view, glyphs, rect, &mut hits);
        }
        status(buf, view, glyphs, regions.status, &mut hits);
        command.or(search)
    };
    app.hits = hits.targets;
    app.hit_keys = hits.keys;
    if let Some(position) = cursor {
        frame.set_cursor_position(position);
    }
}

/// Scrolls every list to its selection, before anything is drawn from them.
fn follow(app: &mut App, glyphs: Glyphs, regions: &Regions) {
    // A document's cursors start over when another document comes forward.
    if app.tabs.active() != &app.doc.seen {
        app.doc.seen = app.tabs.active().clone();
        app.doc.links = Cursor::default();
        app.doc.scroll = 0;
        // A document opens at its top, not scrolled to its first link.
        app.doc.followed = Some(0);
    }
    editor::follow(app, glyphs, regions.editor);
    if let Some(rect) = regions.next {
        app.next.follow(list_area(rect, 3).height as usize);
    }
    if let Some(rect) = regions.sidebar {
        sidebar::follow(app, rect);
    }
    if let Some(rect) = regions.panel {
        let height = list_area(rect, 2).height as usize;
        match app.panel {
            PanelTab::Problems => app.problems.follow(height),
            PanelTab::Output => app.scroll = app.scroll.min(app.log.len().saturating_sub(height)),
            PanelTab::Agent => {
                let lines = app.snapshot.prime.text().lines().count();
                app.scroll = app.scroll.min(lines.saturating_sub(height));
            }
        }
    }
}

// ---- the boxes ------------------------------------------------------------

/// The command centre: the board's filter, as a rounded field, and the
/// layout toggles beside it.
fn command(buf: &mut Buffer, app: &App, glyphs: Glyphs, rect: Rect, hits: &mut Hits) -> Option<Position> {
    let icons = glyphs.icons();
    let focused = app.focus == Region::Command;
    hits.push(rect, Target::Command);
    let found = (!app.filter.is_empty())
        .then(|| format!("{} found", app.rows.iter().filter(|row| matches!(row, Row::Item { .. })).count()));
    let placeholder = if focused { "Filter the board by words, #id or @board" } else { app.workspace.name.as_str() };
    let cursor = field(buf, glyphs, rect, focused, icons.search, &app.filter, placeholder, found);

    let mut x = rect.right() + 2;
    let toggles = [
        (Part::Next, app.wanted.next, icons.left_on, icons.left_off),
        (Part::Panel, app.wanted.panel, icons.panel_on, icons.panel_off),
        (Part::Sidebar, app.wanted.sidebar, icons.right_on, icons.right_off),
    ];
    for (part, open, on, off) in toggles {
        if x + 1 >= buf.area.right() {
            break;
        }
        let colour = if open { palette::TEXT } else { palette::MUTED };
        buf.set_string(x, rect.y, if open { on } else { off }, Style::new().fg(colour));
        hits.push(Rect::new(x, rect.y, 1, 1), Target::Toggle(part));
        x += 2;
    }
    focused.then_some(cursor)
}

/// The box left of the editor: what to take up next, best first.
fn next(buf: &mut Buffer, app: &App, glyphs: Glyphs, rect: Rect, hits: &mut Hits) {
    card(buf, rect, palette::WINDOW, Borders::ALL);
    hits.push(rect, Target::Region(Region::Next));
    let inner = inner(rect);
    let focused = app.focus == Region::Next;

    put(buf, inner.x + 1, inner.y, inner.width.saturating_sub(2), title_pill(glyphs, "Next", palette::WINDOW));
    let (doing, ready) = (app.snapshot.prime.doing.len(), app.snapshot.prime.ready.len());
    let summary = format!("{doing} in progress · {ready} ready");
    buf.set_stringn(inner.x + 1, inner.y + 1, summary, inner.width.saturating_sub(2) as usize, muted());
    // Edge to edge and joined to the border, as the line under VS Code's view
    // title meets the box.
    divider(buf, inner.x, inner.y + 2, inner.width);
    joint(buf, rect.x, inner.y + 2, "\u{251c}");
    joint(buf, rect.right() - 1, inner.y + 2, "\u{2524}");

    let list = list_area(rect, 3);
    let entries = app.snapshot.next();
    if entries.is_empty() {
        let said = "Nothing is in progress or ready.";
        buf.set_stringn(list.x + 1, list.y, said, list.width.saturating_sub(2) as usize, muted());
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
        let room = (list.width as usize).saturating_sub(id_width + 5);
        let spans = vec![
            Span::raw(" "),
            Span::styled(glyph, look),
            Span::styled(format!(" {:>id_width$} ", entry.id), muted()),
            Span::styled(clip(&entry.description, room), text(selected && focused)),
        ];
        put(buf, list.x, y, list.width, spans);
        hits.push(line, Target::NextRow(at));
    }
}

/// The activity bar, down the right edge: one icon per view, the shown one lit.
fn activity(buf: &mut Buffer, app: &App, glyphs: Glyphs, rect: Rect, sidebar: Option<Rect>, hits: &mut Hits) {
    card(buf, rect, palette::WINDOW, Borders::ALL);
    if let Some(sidebar) = sidebar {
        joint(buf, rect.x, rect.y, "\u{252c}");
        joint(buf, rect.x, sidebar.bottom() - 1, "\u{2524}");
    }
    let icons = glyphs.icons();
    let views = [
        (View::Explorer, icons.explorer),
        (View::Search, icons.search),
        (View::Graph, icons.graph),
        (View::Projects, icons.projects),
        (View::Changes, icons.changes),
    ];
    let x = rect.x + 1;
    for (at, (view, icon)) in views.into_iter().enumerate() {
        let y = rect.y + 1 + at as u16 * 2;
        if y + 1 >= rect.bottom() {
            break;
        }
        // One view lit at a time, as in VS Code.
        if app.shown.sidebar && app.view == view {
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
        hits.push(Rect::new(x, y, 3, 1), Target::Activity(view));
    }
}

/// The bottom panel: Problems, Output and Agent.
fn panel(buf: &mut Buffer, app: &App, glyphs: Glyphs, rect: Rect, hits: &mut Hits) {
    card(buf, rect, palette::WINDOW, Borders::ALL);
    hits.push(rect, Target::Region(Region::Panel));
    let inner = inner(rect);
    let icons = glyphs.icons();

    let close = inner.right().saturating_sub(2);
    let mut x = inner.x + 1;
    for tab in PanelTab::ALL {
        let badge = match tab {
            PanelTab::Problems => app.snapshot.problems.len(),
            PanelTab::Output => app.unseen,
            PanelTab::Agent => 0,
        };
        let title = if badge > 0 { format!("{} {badge}", tab.title()) } else { tab.title().to_string() };
        let spans = if tab == app.panel {
            theme::pill(glyphs, title, palette::BRIGHT, palette::PILL, palette::WINDOW)
        } else {
            vec![Span::styled(format!(" {title} "), muted())]
        };
        let width = width_of(&spans);
        if x + width + 2 >= close {
            break;
        }
        put(buf, x, inner.y, width, spans);
        hits.push(Rect::new(x, inner.y, width, 1), Target::PanelTab(tab));
        x += width + 1;
    }
    buf.set_string(close, inner.y, icons.close, muted());
    hits.push(Rect::new(close, inner.y, 1, 1), Target::Toggle(Part::Panel));

    let body = list_area(rect, 2);
    match app.panel {
        PanelTab::Problems => problems(buf, app, glyphs, body, hits),
        PanelTab::Output => output(buf, app, body),
        PanelTab::Agent => {
            let said = app.snapshot.prime.text();
            for (at, line) in said.lines().skip(app.scroll).take(body.height as usize).enumerate() {
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
        hits.push(line, Target::ProblemRow(at));
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
    hits.push(Rect::new(bell_x.saturating_sub(1), y, 3, 1), Target::Bell);

    let mut limit = bell_x.saturating_sub(2);
    let message = if app.chording() {
        Some(("(Ctrl+K) was pressed. Waiting for second key of chord...", palette::TEXT))
    } else {
        app.flash().map(|(said, kind)| (said, if kind == Kind::Refused { palette::YELLOW } else { palette::TEXT }))
    };
    if let Some((said, colour)) = message {
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
    segments.push((vec![Span::styled(counters, muted())], Some(Target::PanelTab(PanelTab::Problems))));
    if let Some(entry) = app.snapshot.next().first() {
        let said = format!("{} {} {}", icons.rocket, entry.id, clip(&entry.description, 36));
        segments.push((vec![Span::styled(said, muted())], Some(Target::NextRow(0))));
    }
    segments.push((vec![Span::styled(format!("{} {}", icons.folder, app.workspace.folder), muted())], None));

    let mut x = rect.x + 2;
    for (spans, target) in segments {
        let width = width_of(&spans);
        if x + width > limit {
            break;
        }
        put(buf, x, y, width, spans);
        if let Some(target) = target {
            hits.push(Rect::new(x, y, width, 1), target);
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

#[cfg(test)]
mod tests {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    use super::*;
    use crate::tui::app::Workspace;
    use crate::tui::board::tests::{board, snapshot, words};
    use crate::tui::tabs::Doc;

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
        ekko.set_phases(&words(&["setup", "release"])).unwrap();
        ekko.create_task_in(&words(&["@render", "wire the watcher", "p:2"]), Some("setup")).unwrap();
        ekko.create_task_in(&words(&["@render", "draw the frame", "d:2020-01-01"]), Some("release")).unwrap();
        ekko.create_task(&words(&["@docs", "rewrite the interactive mode section"])).unwrap();
        ekko.create_note(&words(&["@render", "why the watcher polls"])).unwrap();
        ekko.set_blocked_by(&words(&["@3", "1"])).unwrap();
        ekko.set_attached_to(&words(&["@4", "1"])).unwrap();
        let workspace = Workspace {
            label: "project demo".into(),
            name: "demo".into(),
            folder: "~/Projects/demo".into(),
            cwd: dir.clone(),
            home: dir.clone(),
            dir: dir.clone(),
            branch: Some("main".into()),
        };
        let app = App::new(workspace, snapshot(&dir, &ekko), 0);
        (dir, app)
    }

    /// Rounded boxes, the sidebar meeting the activity bar in a joint rather
    /// than a corner, and the board, the queue and the status bar all drawn.
    #[test]
    fn a_wide_frame_draws_every_box_and_its_joints() {
        let (dir, mut app) = sample("wide");
        let buf = drawn(&mut app, Glyphs::Nerd, 170, 46);
        let regions = layout::regions(buf.area, app.wanted).unwrap();

        let editor = regions.editor;
        assert_eq!(buf[(editor.x, editor.y)].symbol(), "\u{256d}");
        assert_eq!(buf[(editor.right() - 1, editor.bottom() - 1)].symbol(), "\u{256f}");
        let sidebar = regions.sidebar.unwrap();
        assert_eq!(buf[(regions.activity.x, regions.activity.y)].symbol(), "\u{252c}");
        assert_eq!(buf[(regions.activity.x, sidebar.bottom() - 1)].symbol(), "\u{2524}");
        assert_eq!(buf[(regions.activity.x, regions.activity.bottom() - 1)].symbol(), "\u{2570}");
        assert_eq!(buf[(editor.x + 1, editor.y + 1)].bg, palette::EDITOR);

        let all = screen(&buf);
        for expected in ["wire the watcher", "@render", "[0/2]", "Explorer", "Open Editors", "Problems 2", "~/Projects/demo", "main"] {
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

    /// A typed note says its kind on the board's row and in its tab's header,
    /// and the search view offers the kinds as chips.
    #[test]
    fn a_typed_note_carries_its_mark_on_the_board_and_in_its_tab() {
        let (dir, mut app) = sample("marks");
        let at = app.snapshot.items.iter().position(|item| item.id == 4).unwrap();
        app.snapshot.items[at].knowledge = Some(crate::item::Knowledge::Gotcha);
        app.snapshot.all.get_mut(&4).unwrap().knowledge = Some(crate::item::Knowledge::Gotcha);

        let all = screen(&drawn(&mut app, Glyphs::Nerd, 170, 46));
        assert!(all.contains("gotcha why the watcher polls"), "{all}");

        let key = crate::tui::board::key(&app.snapshot.all[&4]);
        app.tabs.open(Doc::Item(key), false);
        app.view = View::Search;
        let all = screen(&drawn(&mut app, Glyphs::Nerd, 170, 46));
        let header = all.lines().find(|line| line.contains("#4") && !line.contains("@render")).expect("the note's tab has a header");
        assert!(header.contains("gotcha"), "{header}");
        assert!(all.contains(" procedure "), "the kinds are not among the chips:\n{all}");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A waiting task has its own glyph on the board in both glyph sets, the
    /// stats say how many wait, and the search view offers the state as a chip.
    #[test]
    fn a_waiting_task_is_drawn_as_waiting() {
        let (dir, mut app) = sample("waiting");
        let ekko = crate::ekko::Ekko::new(crate::storage::Storage::new(&dir).unwrap());
        ekko.set_state(&words(&["@2", "waiting"]), false).unwrap();
        app.reload(&ekko).unwrap();

        for (glyphs, glyph) in [(Glyphs::Nerd, "\u{eb7b}"), (Glyphs::Plain, "\u{25d4}")] {
            let all = screen(&drawn(&mut app, glyphs, 170, 46));
            let row = all.lines().find(|line| line.contains("draw the frame")).expect("the task's row");
            assert!(row.contains(glyph), "{row}");
            assert!(all.contains("1 waiting"), "{all}");
        }
        app.view = View::Search;
        let all = screen(&drawn(&mut app, Glyphs::Nerd, 170, 46));
        assert!(all.contains(" waiting "), "the state is not among the chips:\n{all}");
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

    /// Every page and every view draws something of its own, and none of them
    /// reaches for the private use area without a Nerd Font.
    #[test]
    fn every_page_and_view_draws_and_plain_glyphs_stay_plain() {
        let (dir, mut app) = sample("pages");
        let key = crate::tui::board::key(&app.snapshot.all[&1]);
        let pages = [
            (Doc::Board, View::Explorer, vec!["wire the watcher"]),
            (Doc::Item(key), View::Explorer, vec!["Blocks", "Notes", "why the watcher polls", "Phase"]),
            (Doc::Roadmap, View::Search, vec!["setup", "release", "Words, #id"]),
            (Doc::Calendar, View::Projects, vec!["Su", "No projects yet"]),
            (Doc::Welcome, View::Changes, vec!["Start", "Open the roadmap", "Since"]),
            (Doc::Graph, View::Graph, vec!["nodes", "links", "depth 1"]),
        ];
        for (doc, view, expected) in pages {
            app.tabs.open(doc.clone(), false);
            app.view = view;
            for glyphs in [Glyphs::Nerd, Glyphs::Plain] {
                let buf = drawn(&mut app, glyphs, 170, 46);
                let all = screen(&buf);
                for text in &expected {
                    assert!(all.contains(text), "{doc:?} with {view:?} lacks {text:?}:\n{all}");
                }
                if glyphs == Glyphs::Plain {
                    let private = all.chars().filter(|c| ('\u{e000}'..='\u{f8ff}').contains(c)).count();
                    assert_eq!(private, 0, "{doc:?} with {view:?} drew {private} private-use glyphs");
                }
            }
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
