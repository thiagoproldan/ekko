//! Drawing a graph: its links, then its nodes, then their names, in braille
//! over the pane -- and the buttons and the settings panel drawn over it.

use std::collections::HashSet;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::Borders;
use unicode_width::UnicodeWidthStr;

use super::pieces::{card, clip, field, inner, muted, put};
use super::Hits;
use crate::tui::app::{App, Target};
use crate::tui::board;
use crate::tui::graph::model::Kind;
use crate::tui::graph::raster::Raster;
use crate::tui::graph::settings::{controls, Control, Settings, Slider};
use crate::tui::graph::{Editing, GraphView, Pane, Tool};
use crate::tui::layout::Region;
use crate::tui::theme::{palette, Glyphs};

/// Names show from this zoom up, moved by the text fade threshold.
const LABEL_SCALE: f64 = 0.08;
const PANEL_WIDTH: u16 = 40;

/// The local graph's canvas, below its header row in the sidebar.
pub(super) fn local_canvas(body: Rect) -> Rect {
    Rect::new(body.x, body.y + 1, body.width.saturating_sub(1), body.height.saturating_sub(1))
}

/// Brings the whole board's graph up to date and frames it, before drawing.
pub(super) fn follow_global(app: &mut App, canvas: Rect) {
    app.graph.area = canvas;
    app.graph.refresh(&app.snapshot, &app.settings, app.generation, None);
    app.graph.frame();
    let rows = controls(&app.settings, &app.graph_panel.sections, false).len();
    app.graph_panel.cursor.clamp(rows);
    app.graph_panel.cursor.follow(canvas.height.saturating_sub(4) as usize);
}

/// Brings the local graph up to date around the active item, before drawing.
pub(super) fn follow_local(app: &mut App, canvas: Rect) {
    let centre = app
        .active_item()
        .and_then(|id| app.snapshot.all.get(&id))
        .map(|item| (board::key(item), app.settings.depth));
    app.local.area = canvas;
    app.local.refresh(&app.snapshot, &app.settings, app.generation, centre);
    app.local.frame();
}

/// The Graph tab: the whole board, its buttons, and the settings panel.
pub(super) fn graph_doc(buf: &mut Buffer, app: &App, glyphs: Glyphs, body: Rect, hits: &mut Hits) {
    hits.push(body, Target::Canvas(Pane::Global));
    let view = &app.graph;
    if view.graph.nodes.is_empty() {
        let said = if app.snapshot.all.is_empty() { "The board is empty." } else { "Nothing on the board passes the graph's filters." };
        buf.set_stringn(body.x + 2, body.y, said, body.width.saturating_sub(4) as usize, muted());
    } else {
        draw_graph(buf, view, &app.settings, body, app.focus == Region::Editor && !app.graph_panel.open);
        let said = format!("{} nodes · {} links", view.graph.nodes.len(), view.graph.edges.len());
        buf.set_stringn(body.x + 2, body.bottom().saturating_sub(1), said, body.width.saturating_sub(4) as usize, muted());
    }
    tools(buf, glyphs, body, Pane::Global, app.graph_panel.open, hits);
    if app.graph_panel.open {
        panel(buf, app, glyphs, body, hits);
    }
}

/// The sidebar's graph: the neighbourhood of the active item, as Obsidian's
/// local graph follows the active note.
pub(super) fn local_view(buf: &mut Buffer, app: &App, glyphs: Glyphs, body: Rect, hits: &mut Hits) {
    let icons = glyphs.icons();
    let depth = format!("depth {}", app.settings.depth);
    buf.set_stringn(body.x + 1, body.y, &depth, body.width as usize, muted());
    let minus = body.x + 2 + depth.len() as u16;
    buf.set_string(minus, body.y, icons.zoom_out, muted());
    hits.push(Rect::new(minus, body.y, 1, 1), Target::Tool(Pane::Local, Tool::Shallower));
    buf.set_string(minus + 2, body.y, icons.zoom_in, muted());
    hits.push(Rect::new(minus + 2, body.y, 1, 1), Target::Tool(Pane::Local, Tool::Deeper));
    let fit = body.right().saturating_sub(3);
    buf.set_string(fit, body.y, icons.fit, muted());
    hits.push(Rect::new(fit, body.y, 1, 1), Target::Tool(Pane::Local, Tool::Fit));

    let canvas = local_canvas(body);
    hits.push(canvas, Target::Canvas(Pane::Local));
    if app.local.graph.nodes.is_empty() {
        let said = "Open an item, or select one on the board, to see what it is linked to.";
        for (at, line) in super::pieces::wrap(said, canvas.width.saturating_sub(2) as usize).iter().enumerate() {
            buf.set_string(canvas.x + 1, canvas.y + at as u16, line, muted());
        }
        return;
    }
    draw_graph(buf, &app.local, &app.settings, canvas, app.focus == Region::Sidebar);
}

/// Zoom, frame and settings, over the top right of a graph.
fn tools(buf: &mut Buffer, glyphs: Glyphs, area: Rect, pane: Pane, panel_open: bool, hits: &mut Hits) {
    let icons = glyphs.icons();
    let buttons = [(icons.zoom_out, Tool::ZoomOut), (icons.zoom_in, Tool::ZoomIn), (icons.fit, Tool::Fit), (icons.settings, Tool::Settings)];
    let mut x = area.right().saturating_sub(2 + 2 * buttons.len() as u16);
    for (icon, tool) in buttons {
        let lit = tool == Tool::Settings && panel_open;
        buf.set_string(x, area.y, icon, Style::new().fg(if lit { palette::BRIGHT } else { palette::MUTED }));
        hits.push(Rect::new(x, area.y, 1, 1), Target::Tool(pane, tool));
        x += 2;
    }
}

/// The links, the nodes and their names. What the pointer is on, or else
/// what the keyboard is on, is lit with its links and neighbours, and the
/// rest of the graph steps back, as hovering does in Obsidian.
fn draw_graph(buf: &mut Buffer, view: &GraphView, settings: &Settings, area: Rect, focused: bool) {
    if area.width < 4 || area.height < 2 {
        return;
    }
    let mut raster = Raster::new(area.width, area.height);
    let born = view.born();
    let shown = |index: usize| born.as_ref().is_none_or(|born| born.contains(&index));
    let focus = view.hover.or(if focused { view.selected } else { None });
    let near: HashSet<usize> = focus
        .map(|at| view.graph.neighbours[at].iter().copied().chain(std::iter::once(at)).collect())
        .unwrap_or_default();
    let lines = settings.link_thickness.round().clamp(1.0, 3.0) as usize;

    for edge in &view.graph.edges {
        if !shown(edge.from) || !shown(edge.to) || edge.from == edge.to {
            continue;
        }
        let (from, to) = (view.point(edge.from), view.point(edge.to));
        let lit = focus.is_some_and(|at| edge.from == at || edge.to == at);
        let (colour, priority) = if lit {
            (palette::ACCENT, 3)
        } else if focus.is_some() {
            (palette::FADED, 1)
        } else {
            (palette::EDGE, 1)
        };
        let (dx, dy) = (to.0 - from.0, to.1 - from.1);
        let length = dx.hypot(dy).max(1e-9);
        let (px, py) = (-dy / length, dx / length);
        for line in 0..lines {
            let shift = line as f64 - (lines - 1) as f64 / 2.0;
            raster.line((from.0 + px * shift, from.1 + py * shift), (to.0 + px * shift, to.1 + py * shift), colour, priority);
        }
        if settings.arrows {
            let back = view.radius(edge.to, settings) + 1.0;
            if length > back + 3.0 {
                let (ux, uy) = (dx / length, dy / length);
                let tip = (to.0 - ux * back, to.1 - uy * back);
                for angle in [0.45f64, -0.45] {
                    let (cos, sin) = (angle.cos(), angle.sin());
                    let (rx, ry) = (-(ux * cos - uy * sin), -(ux * sin + uy * cos));
                    raster.line(tip, (tip.0 + rx * 4.0, tip.1 + ry * 4.0), colour, priority);
                }
            }
        }
    }

    for (index, node) in view.graph.nodes.iter().enumerate() {
        if !shown(index) {
            continue;
        }
        let base = match node.kind {
            Kind::Board => palette::PURPLE,
            Kind::Unresolved => palette::UNRESOLVED,
            Kind::Item { .. } => node
                .group
                .and_then(|group| settings.groups.get(group))
                .map_or(palette::NODE, |group| palette::GROUPS[group.colour % palette::GROUPS.len()]),
        };
        let (colour, priority) = if Some(index) == view.hover {
            (palette::ACCENT, 8)
        } else if focused && Some(index) == view.selected {
            (palette::BRIGHT, 8)
        } else if focus.is_some() && !near.contains(&index) {
            (palette::FADED, 4)
        } else if near.contains(&index) {
            (base, 6)
        } else {
            (base, 5)
        };
        raster.disk(view.point(index), view.radius(index, settings), colour, priority);
    }
    raster.blit(buf, area);

    // Names under the nodes: those in focus always, the rest once the view is
    // zoomed in far enough, fading in on the way, and never over each other.
    let threshold = LABEL_SCALE * 2f64.powf(settings.text_fade);
    let scale = view.camera.scale;
    let mut order: Vec<usize> = (0..view.graph.nodes.len()).filter(|index| shown(*index)).collect();
    order.sort_by_key(|&index| {
        (Some(index) != view.hover, !near.contains(&index), std::cmp::Reverse(view.graph.nodes[index].weight))
    });
    let mut taken: Vec<(u16, u16, u16)> = Vec::new();
    for index in order {
        let in_focus = near.contains(&index) || (focused && Some(index) == view.selected);
        if !in_focus && scale < threshold * 0.6 {
            continue;
        }
        let colour = if Some(index) == view.hover || (focused && Some(index) == view.selected) {
            palette::BRIGHT
        } else if in_focus {
            palette::TEXT
        } else if focus.is_some() || scale < threshold {
            palette::FADED
        } else {
            palette::MUTED
        };
        let (x, y) = view.point(index);
        let below = y + view.radius(index, settings) + 2.0;
        if x < 0.0 || below < 0.0 {
            continue;
        }
        let row = area.y + (below / 4.0).floor() as u16;
        if row >= area.bottom() {
            continue;
        }
        let label = clip(&view.graph.nodes[index].label, 24);
        let width = label.width() as u16;
        let centre = area.x + (x / 2.0) as u16;
        let left = centre.saturating_sub(width / 2).max(area.x).min(area.right().saturating_sub(width));
        if taken.iter().any(|&(taken_row, start, end)| taken_row == row && left <= end && start <= left + width) {
            continue;
        }
        buf.set_stringn(left, row, &label, width as usize, Style::new().fg(colour));
        taken.push((row, left, left + width));
    }
}

/// The settings panel, over the top right of the graph: Filters, Groups,
/// Display and Forces, each closed until opened, and Restore at the foot.
fn panel(buf: &mut Buffer, app: &App, glyphs: Glyphs, canvas: Rect, hits: &mut Hits) {
    let icons = glyphs.icons();
    let rows = controls(&app.settings, &app.graph_panel.sections, false);
    let width = PANEL_WIDTH.min(canvas.width.saturating_sub(4));
    let height = (rows.len() as u16 + 2).min(canvas.height.saturating_sub(2));
    if width < 20 || height < 3 {
        return;
    }
    let rect = Rect::new(canvas.right().saturating_sub(width + 1), canvas.y + 1, width, height);
    card(buf, rect, palette::WINDOW, Borders::ALL);
    hits.push(rect, Target::Region(Region::Editor));
    let inner = inner(rect);
    let focused = app.focus == Region::Editor;
    let settings = &app.settings;

    for (screen, (at, control)) in rows.iter().enumerate().skip(app.graph_panel.cursor.offset).take(inner.height as usize).enumerate() {
        let y = inner.y + screen as u16;
        let row = Rect::new(inner.x, y, inner.width, 1);
        let selected = focused && at == app.graph_panel.cursor.selected;
        if selected {
            buf.set_style(row, Style::new().bg(palette::PILL));
        }
        let left = inner.x + 1;
        let room = inner.width.saturating_sub(2);
        match *control {
            Control::Section(section) => {
                let open = app.graph_panel.sections.contains(&section);
                let spans = vec![
                    Span::styled(format!("{} ", if open { icons.expanded } else { icons.collapsed }), muted()),
                    Span::styled(section.label(), Style::new().fg(palette::TEXT).add_modifier(Modifier::BOLD)),
                ];
                put(buf, left, y, room, spans);
            }
            Control::Search => {
                let editing = app.graph_panel.editing == Some(Editing::Search);
                field(buf, glyphs, Rect::new(left, y, room, 1), editing, icons.search, &settings.search, "Search items", None);
            }
            Control::Toggle(toggle) => {
                buf.set_stringn(left + 2, y, toggle.label(), room.saturating_sub(6) as usize, Style::new().fg(palette::TEXT));
                let on = toggle.get(settings);
                let spans = if on {
                    vec![Span::styled("\u{2501}\u{2501}", Style::new().fg(palette::ACCENT)), Span::styled("\u{25cf}", Style::new().fg(palette::BRIGHT))]
                } else {
                    vec![Span::styled("\u{25cf}", Style::new().fg(palette::MUTED)), Span::styled("\u{2501}\u{2501}", Style::new().fg(palette::BORDER))]
                };
                put(buf, inner.right().saturating_sub(4), y, 3, spans);
            }
            Control::Slider(slider) => {
                let track = 10u16;
                let value = slider_value(slider, settings);
                let label_room = room.saturating_sub(track + value.len() as u16 + 4);
                buf.set_stringn(left + 2, y, clip(slider.label(), label_room as usize), label_room as usize, Style::new().fg(palette::TEXT));
                let start = inner.right().saturating_sub(track + value.len() as u16 + 2);
                let knob = (slider.fraction(settings) * f64::from(track - 1)).round() as u16;
                for step in 0..track {
                    let (symbol, colour) = match step.cmp(&knob) {
                        std::cmp::Ordering::Less => ("\u{2501}", palette::ACCENT),
                        std::cmp::Ordering::Equal => ("\u{25cf}", palette::BRIGHT),
                        std::cmp::Ordering::Greater => ("\u{2500}", palette::BORDER),
                    };
                    buf.set_string(start + step, y, symbol, Style::new().fg(colour));
                }
                buf.set_string(start + track + 1, y, &value, muted());
                hits.push(row, Target::Control(at));
                hits.push(Rect::new(start, y, track, 1), Target::Track(at));
                continue;
            }
            Control::Group(index) => {
                let group = &settings.groups[index];
                let colour = palette::GROUPS[group.colour % palette::GROUPS.len()];
                let editing = app.graph_panel.editing == Some(Editing::Group(index));
                let (said, style) = if group.query.is_empty() && !editing {
                    ("Enter a search to colour".to_string(), muted())
                } else {
                    (format!("{}{}", group.query, if editing { "\u{258f}" } else { "" }), Style::new().fg(if editing { palette::BRIGHT } else { palette::TEXT }))
                };
                let spans = vec![Span::raw("  "), Span::styled("\u{25cf} ", Style::new().fg(colour)), Span::styled(clip(&said, room.saturating_sub(4) as usize), style)];
                put(buf, left, y, room, spans);
            }
            Control::NewGroup | Control::Animate | Control::Restore => {
                let (icon, said) = match *control {
                    Control::NewGroup => (icons.add, "New group"),
                    Control::Animate => (icons.play, "Animate"),
                    _ => (icons.discard, "Restore default settings"),
                };
                let spans = vec![Span::raw("  "), Span::styled(format!("{icon} "), Style::new().fg(palette::LINK)), Span::styled(said, Style::new().fg(palette::LINK))];
                put(buf, left, y, room, spans);
            }
        }
        hits.push(row, Target::Control(at));
    }
}

fn slider_value(slider: Slider, settings: &Settings) -> String {
    let value = slider.get(settings);
    match slider {
        Slider::Depth | Slider::LinkDistance => format!("{value:.0}"),
        Slider::Repel => format!("{value:.1}"),
        _ => format!("{value:.2}"),
    }
}
