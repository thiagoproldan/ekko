//! The interactive mode: a third frontend on the same `Ekko` core.
//!
//! It never goes through `render.rs`. That module reproduces taskbook's
//! output byte for byte and is pinned by golden tests; nothing here can
//! disturb it, which is what makes "changes nothing about the CLI" a
//! property of the shape rather than a promise to be careful.
//!
//! What it does borrow is `Level`, so which icon a cancelled task gets --
//! and the precedence between cancelled, complete, in-progress and paused
//! -- is decided in exactly one place.
//!
//! The layout is the picker idiom: a list, a preview of whatever is
//! selected, a calendar, a prompt that filters as you type, and a status
//! line. The split earns its place -- notes took 43 of 85 item lines on a
//! real board, and folding can only truncate them to fit. A picker does
//! not have to truncate anything, because the list holds one line per item
//! and the full text lives in the preview.

use std::io::{self, Write};
use std::time::Duration;

use chrono::Datelike;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEventKind,
};
use crossterm::style::{
    Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor,
};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::{cursor, execute, queue};

use crate::ekko::{Ekko, EkkoError, Outcome};
use crate::item::Item;
use crate::render::{CalendarMonth, Level, Stats};

/// Restores the terminal when it goes out of scope, however that happens.
///
/// A CLI that dies leaves a mess in the scrollback. A raw-mode program
/// that dies leaves the shell itself unusable -- no echo, no line editing,
/// still on the alternate screen -- and the person has to know to type
/// `reset` blind. Tying teardown to a value's lifetime means the unwind
/// restores it, and the hook covers the message printed before it.
struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture, cursor::Hide)?;

        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ = restore();
            previous(info);
        }));

        Ok(TerminalGuard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = restore();
    }
}

fn restore() -> io::Result<()> {
    execute!(io::stdout(), cursor::Show, DisableMouseCapture, LeaveAlternateScreen)?;
    disable_raw_mode()
}

/// One line of the list: either a board heading or an item under it.
///
/// Headings are in the list rather than floating above it because the
/// grouping has to survive filtering and scrolling -- a board name that
/// only appears when you happen to be scrolled to the top tells you
/// nothing about the row you are looking at.
enum Row {
    Board { name: String, complete: u32, tasks: u32 },
    Item(usize),
}

struct State {
    /// Items grouped the way the board view groups them, flattened with
    /// their headings so one index addresses one screen line.
    items: Vec<Item>,
    rows: Vec<Row>,
    filter: String,
    /// Index into `rows`, always pointing at an `Item`.
    selected: usize,
    /// First visible row. Without this the list simply stops at the fold
    /// and everything past it is unreachable -- which on a 96-item board
    /// meant 71 items you could not get to.
    offset: usize,
    stats: Stats,
    month: CalendarMonth,
    /// What the last write did, shown briefly. A picker puts navigation
    /// and mutation on neighbouring keys, so a change that leaves no
    /// trace is a change you cannot notice you made.
    flash: Option<String>,
}

impl State {
    fn load(ekko: &Ekko, filter: &str) -> Result<(Vec<Item>, Vec<Row>, Stats), EkkoError> {
        let Outcome::Board(groups) = ekko.display_by_board()? else {
            return Err(EkkoError::MissingId);
        };
        let Outcome::Stats(stats) = ekko.display_stats()? else {
            return Err(EkkoError::MissingId);
        };

        let needle = filter.to_lowercase();
        let mut items = Vec::new();
        let mut rows = Vec::new();

        for (board, group) in groups {
            let matching: Vec<&Item> = group
                .iter()
                .filter(|item| {
                    needle.is_empty() || item.description.to_lowercase().contains(&needle)
                })
                .collect();
            if matching.is_empty() {
                continue;
            }

            // The counts come from the whole group, not the filtered one:
            // `[18/28]` is a fact about the board, and recomputing it per
            // filter would make the same board report different totals
            // depending on what you had typed.
            let tasks = group.iter().filter(|i| i.is_task).count() as u32;
            let complete =
                group.iter().filter(|i| i.is_complete.unwrap_or(false)).count() as u32;
            rows.push(Row::Board { name: board.clone(), complete, tasks });

            for item in matching {
                rows.push(Row::Item(items.len()));
                items.push(item.clone());
            }
        }

        Ok((items, rows, stats))
    }

    fn selected_item(&self) -> Option<usize> {
        match self.rows.get(self.selected) {
            Some(Row::Item(at)) => Some(*at),
            _ => None,
        }
    }

    /// Moves to the next selectable row, skipping headings.
    fn step(&mut self, by: isize) {
        if self.rows.is_empty() {
            return;
        }
        let len = self.rows.len() as isize;
        let mut at = self.selected as isize;
        for _ in 0..len {
            at = (at + by).rem_euclid(len);
            if matches!(self.rows[at as usize], Row::Item(_)) {
                self.selected = at as usize;
                return;
            }
        }
    }

    /// Pulls the viewport so the selection is on screen, scrolling by the
    /// smallest amount that achieves it.
    fn scroll_into_view(&mut self, height: usize) {
        if height == 0 {
            return;
        }
        if self.selected < self.offset {
            self.offset = self.selected;
        } else if self.selected >= self.offset + height {
            self.offset = self.selected + 1 - height;
        }
    }
}

pub fn run(ekko: &Ekko) -> Result<(), EkkoError> {
    let (items, rows, stats) = State::load(ekko, "")?;
    let now = chrono::Local::now();
    let mut state = State {
        items,
        rows,
        filter: String::new(),
        selected: 0,
        offset: 0,
        stats,
        month: CalendarMonth::of(now.year(), now.month(), Some(now.day()))
            .expect("today is a real date"),
        flash: None,
    };
    state.step(1);

    let _guard = TerminalGuard::enter().map_err(io_error)?;

    loop {
        draw(&mut state).map_err(io_error)?;

        if !event::poll(Duration::from_millis(250)).map_err(io_error)? {
            continue;
        }

        match event::read().map_err(io_error)? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                if !handle_key(ekko, &mut state, key)? {
                    return Ok(());
                }
            }
            Event::Mouse(mouse) => handle_mouse(&mut state, mouse),
            _ => {}
        }
    }
}

fn io_error(e: io::Error) -> EkkoError {
    EkkoError::Storage(crate::storage::StorageError::Io(e))
}

fn reload(ekko: &Ekko, state: &mut State) -> Result<(), EkkoError> {
    let (items, rows, stats) = State::load(ekko, &state.filter)?;
    state.items = items;
    state.rows = rows;
    state.stats = stats;
    if state.selected >= state.rows.len() || state.selected_item().is_none() {
        state.selected = 0;
        state.step(1);
    }
    Ok(())
}

fn handle_key(ekko: &Ekko, state: &mut State, key: KeyEvent) -> Result<bool, EkkoError> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

    match key.code {
        KeyCode::Esc => return Ok(false),
        KeyCode::Char('c') if ctrl => return Ok(false),

        KeyCode::Down => state.step(1),
        KeyCode::Up => state.step(-1),
        KeyCode::Char('n') if ctrl => state.step(1),
        KeyCode::Char('p') if ctrl => state.step(-1),

        KeyCode::Backspace => {
            state.filter.pop();
            state.selected = 0;
            state.offset = 0;
            reload(ekko, state)?;
        }
        KeyCode::Char(c) if !ctrl => {
            state.filter.push(c);
            state.selected = 0;
            state.offset = 0;
            reload(ekko, state)?;
        }

        KeyCode::Enter => act(ekko, state, Action::Done)?,
        KeyCode::Tab => act(ekko, state, Action::Progress)?,
        KeyCode::Char('s') if ctrl => act(ekko, state, Action::Stash)?,

        _ => {}
    }

    Ok(true)
}

fn handle_mouse(state: &mut State, mouse: event::MouseEvent) {
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            let row = mouse.row as usize;
            if row < LIST_TOP {
                return;
            }
            let at = state.offset + (row - LIST_TOP);
            // Clicking a heading selects nothing rather than selecting
            // the wrong thing.
            if matches!(state.rows.get(at), Some(Row::Item(_))) {
                state.selected = at;
            }
        }
        MouseEventKind::ScrollDown => state.step(1),
        MouseEventKind::ScrollUp => state.step(-1),
        _ => {}
    }
}

enum Action {
    Done,
    Progress,
    Stash,
}

/// Applies one action, then reloads.
///
/// The lock is taken by the operation and released the moment it returns,
/// never held while the loop sits idle. A UI parked on the flock would
/// block the user's other terminal and every agent, which is the exact
/// failure the lock exists to prevent, caused by the thing meant to help.
fn act(ekko: &Ekko, state: &mut State, action: Action) -> Result<(), EkkoError> {
    let Some(at) = state.selected_item() else { return Ok(()) };
    let item = &state.items[at];
    let id = item.id;

    // Done and cancelled are terminal, and a navigation key must not undo
    // one. Setting `progress` clears `isComplete` by definition, so a
    // stray Tab on a finished task used to destroy the fact that it was
    // finished -- which is exactly how two items got un-completed the hour
    // this mode shipped.
    let terminal = item.is_complete.unwrap_or(false) || item.cancelled.unwrap_or(false);

    let result = match action {
        Action::Stash => ekko.set_stashed(&[id.to_string()], true).map(|_| format!("{id} stashed")),
        _ if !item.is_task => {
            state.flash = Some("notes have no state".to_string());
            return Ok(());
        }
        Action::Done | Action::Progress if terminal => {
            state.flash = Some(format!("{id} is finished -- use --set to change it"));
            return Ok(());
        }
        Action::Done => ekko
            .set_state(&[format!("@{id}"), "done".to_string()])
            .map(|_| format!("{id} → done")),
        Action::Progress => {
            let next = if item.in_progress.unwrap_or(false) { "paused" } else { "progress" };
            ekko.set_state(&[format!("@{id}"), next.to_string()])
                .map(|_| format!("{id} → {next}"))
        }
    };

    match result {
        Ok(said) => {
            state.flash = Some(said);
            reload(ekko, state)?;
        }
        Err(e) => state.flash = Some(e.to_string()),
    }
    Ok(())
}

// ---- drawing ---------------------------------------------------------

const LIST_TOP: usize = 1;
const MIN_COLS: usize = 60;
const MIN_ROWS: usize = 12;
/// Border, label, weekday row, six weeks, border.
const CALENDAR_H: usize = 11;

const BORDER: Color = Color::AnsiValue(240);
const DIM: Color = Color::AnsiValue(245);

fn draw(state: &mut State) -> io::Result<()> {
    let (cols, rows) = crossterm::terminal::size()?;
    let (cols, rows) = (cols as usize, rows as usize);
    let mut out = io::stdout();

    queue!(out, Clear(ClearType::All))?;

    if cols < MIN_COLS || rows < MIN_ROWS {
        queue!(out, cursor::MoveTo(0, 0), Print("terminal too small"))?;
        return out.flush();
    }

    // One row at the bottom for the status line, outside every panel --
    // the vim statusline idiom, and the only place a full-width fact fits
    // without stealing from the list.
    let body = rows - 1;
    let prompt_h = 3;
    let results_h = body - prompt_h;
    let list_rows = results_h - 2;

    let list_width = cols.saturating_sub(4) / 2;
    let right = list_width + 2;
    let right_width = cols - right - 1;
    let calendar_h = if body > CALENDAR_H + 6 { CALENDAR_H } else { 0 };
    let preview_h = body - calendar_h;

    state.scroll_into_view(list_rows);

    panel(&mut out, 0, 0, list_width, results_h, "Results")?;
    panel(&mut out, 0, results_h, list_width, prompt_h, "Prompt")?;
    panel(&mut out, right, 0, right_width, preview_h, "Preview")?;
    if calendar_h > 0 {
        panel(&mut out, right, preview_h, right_width, calendar_h, &state.month.label)?;
        draw_calendar(&mut out, state, right + 2, preview_h + 1)?;
    }

    for screen in 0..list_rows {
        let Some(row) = state.rows.get(state.offset + screen) else { break };
        draw_row(&mut out, state, row, screen, state.offset + screen == state.selected, list_width)?;
    }

    if let Some(at) = state.selected_item() {
        draw_preview(&mut out, &state.items[at], right + 2, right_width, preview_h)?;
    }

    draw_prompt(&mut out, state, results_h + 1, list_width)?;
    draw_status(&mut out, state, rows - 1, cols)?;

    out.flush()
}

fn panel(out: &mut impl Write, x: usize, y: usize, w: usize, h: usize, title: &str) -> io::Result<()> {
    if w < 4 || h < 2 {
        return Ok(());
    }
    let inner = w - 2;
    let label = format!(" {title} ");
    let left = inner.saturating_sub(label.chars().count()) / 2;
    let right = inner.saturating_sub(left + label.chars().count());

    queue!(out, SetForegroundColor(BORDER))?;
    queue!(
        out,
        cursor::MoveTo(x as u16, y as u16),
        Print(format!("┌{}{}{}┐", "─".repeat(left), label, "─".repeat(right)))
    )?;
    for row in 1..h.saturating_sub(1) {
        queue!(out, cursor::MoveTo(x as u16, (y + row) as u16), Print("│"))?;
        queue!(out, cursor::MoveTo((x + w - 1) as u16, (y + row) as u16), Print("│"))?;
    }
    queue!(
        out,
        cursor::MoveTo(x as u16, (y + h - 1) as u16),
        Print(format!("└{}┘", "─".repeat(inner)))
    )?;
    queue!(out, ResetColor)
}

fn draw_row(
    out: &mut impl Write,
    state: &State,
    row: &Row,
    screen: usize,
    selected: bool,
    width: usize,
) -> io::Result<()> {
    let inner = width.saturating_sub(2);
    queue!(out, cursor::MoveTo(1, (LIST_TOP + screen) as u16))?;

    let (body, colour) = match row {
        Row::Board { name, complete, tasks } => {
            (format!("{name} [{complete}/{tasks}]"), Color::Reset)
        }
        Row::Item(at) => {
            let item = &state.items[*at];
            let level = Level::of(item);
            let indent = if item.anchor.is_some() { "  " } else { "" };
            let marker = if selected { ">" } else { " " };
            (
                format!(
                    "{marker} {indent}{:>3}. {} {}",
                    item.id,
                    level.icon(),
                    item.description.replace('\n', " ")
                ),
                colour(&level),
            )
        }
    };

    if selected {
        queue!(out, SetBackgroundColor(Color::AnsiValue(237)))?;
    }
    if matches!(row, Row::Board { .. }) {
        queue!(out, SetAttribute(Attribute::Bold))?;
    }

    let body: String = body.chars().take(inner).collect();
    let pad = inner.saturating_sub(body.chars().count());
    queue!(out, SetForegroundColor(colour), Print(body), Print(" ".repeat(pad)))?;
    queue!(out, SetAttribute(Attribute::Reset), ResetColor)
}

fn draw_preview(
    out: &mut impl Write,
    item: &Item,
    x: usize,
    width: usize,
    height: usize,
) -> io::Result<()> {
    let inner = width.saturating_sub(4);
    if inner == 0 {
        return Ok(());
    }
    let mut row = 2usize;
    let last = height.saturating_sub(2);

    let level = Level::of(item);
    queue!(out, cursor::MoveTo(x as u16, row as u16), SetForegroundColor(colour(&level)))?;
    queue!(out, Print(format!("{} {}", level.icon(), label(item))), ResetColor)?;
    row += 2;

    // Wrapped, not folded. Folding exists because the board has one line
    // per item; the whole point of this pane is that it does not.
    for line in wrap(&item.description, inner) {
        if row >= last {
            break;
        }
        queue!(out, cursor::MoveTo(x as u16, row as u16), Print(line))?;
        row += 1;
    }
    row += 1;

    let mut facts = vec![format!("boards   {}", item.boards.join(", "))];
    if let Some(due) = &item.due_date {
        facts.push(format!("due      {due}"));
    }
    if let Some(priority) = item.priority.filter(|p| *p > 1) {
        facts.push(format!("priority {priority}"));
    }
    if let Some(blockers) = &item.blocked_by {
        facts.push(format!("waits on {}", blockers.len()));
    }
    if item.anchor.is_some() {
        facts.push("explains a task".to_string());
    }
    if item.is_starred {
        facts.push("starred".to_string());
    }
    facts.push(format!("created  {}", item.date));

    queue!(out, SetForegroundColor(DIM))?;
    for fact in facts {
        if row >= last {
            break;
        }
        queue!(out, cursor::MoveTo(x as u16, row as u16), Print(fact))?;
        row += 1;
    }
    queue!(out, ResetColor)
}

fn draw_calendar(out: &mut impl Write, state: &State, x: usize, y: usize) -> io::Result<()> {
    queue!(
        out,
        cursor::MoveTo(x as u16, (y + 1) as u16),
        SetForegroundColor(DIM),
        Print("Su Mo Tu We Th Fr Sa"),
        ResetColor
    )?;
    for (week, days) in state.month.weeks.iter().enumerate() {
        queue!(out, cursor::MoveTo(x as u16, (y + 2 + week) as u16))?;
        for (at, day) in days.iter().enumerate() {
            if at > 0 {
                queue!(out, Print(" "))?;
            }
            match day {
                None => queue!(out, Print("  "))?,
                Some(day) if Some(*day) == state.month.today => queue!(
                    out,
                    SetBackgroundColor(Color::Yellow),
                    SetForegroundColor(Color::Black),
                    Print(format!("{day:>2}")),
                    ResetColor
                )?,
                Some(day) => queue!(out, Print(format!("{day:>2}")))?,
            }
        }
    }
    Ok(())
}

fn draw_prompt(out: &mut impl Write, state: &State, row: usize, width: usize) -> io::Result<()> {
    queue!(out, cursor::MoveTo(2, row as u16))?;
    queue!(out, Print(format!("> {}", state.filter)))?;
    queue!(out, SetAttribute(Attribute::Reverse), Print(" "), SetAttribute(Attribute::Reset))?;

    let shown = state.rows.iter().filter(|r| matches!(r, Row::Item(_))).count();
    let tally = format!("{shown} / {}", state.stats_total());
    let at = width.saturating_sub(tally.chars().count() + 2);
    queue!(out, cursor::MoveTo(at as u16, row as u16), SetForegroundColor(DIM), Print(tally))?;
    queue!(out, ResetColor)
}

/// The stats line, the same facts the CLI prints under the board.
///
/// Full width and outside every panel: it is a fact about the whole board
/// rather than about any one pane, and squeezing it into the prompt would
/// have made both unreadable.
fn draw_status(out: &mut impl Write, state: &State, row: usize, cols: usize) -> io::Result<()> {
    let s = &state.stats;
    let mut parts = vec![
        (format!("{} done", s.complete), Color::Green),
        (format!("{} in-progress", s.in_progress), Color::Blue),
    ];
    if s.paused > 0 {
        parts.push((format!("{} paused", s.paused), Color::Yellow));
    }
    if s.cancelled > 0 {
        parts.push((format!("{} cancelled", s.cancelled), DIM));
    }
    parts.push((format!("{} pending", s.pending), Color::Magenta));
    parts.push((
        format!("{} {}", s.notes, if s.notes == 1 { "note" } else { "notes" }),
        Color::Blue,
    ));
    if s.stashed > 0 {
        parts.push((format!("{} in-stash", s.stashed), DIM));
    }
    if s.trashed > 0 {
        parts.push((format!("{} in-trash", s.trashed), DIM));
    }

    queue!(out, cursor::MoveTo(1, row as u16))?;
    // The flash wins the line while it is there: what just changed matters
    // more for a moment than what the totals are, and a picker where a
    // keystroke changes data silently is how two done tasks got undone.
    if let Some(flash) = &state.flash {
        queue!(out, SetForegroundColor(Color::Yellow), Print(flash.clone()), ResetColor)?;
        return Ok(());
    }

    let mut used = 1usize;
    for (at, (text, colour)) in parts.iter().enumerate() {
        if used + text.chars().count() + 3 > cols {
            break;
        }
        if at > 0 {
            queue!(out, SetForegroundColor(DIM), Print(" · "))?;
            used += 3;
        }
        queue!(out, SetForegroundColor(*colour), Print(text.clone()))?;
        used += text.chars().count();
    }
    queue!(out, ResetColor)
}

impl State {
    fn stats_total(&self) -> u32 {
        let s = &self.stats;
        s.complete + s.in_progress + s.paused + s.cancelled + s.pending + s.notes
    }
}

fn label(item: &Item) -> &'static str {
    if !item.is_task {
        return "note";
    }
    match Level::of(item) {
        Level::Cancelled => "cancelled",
        Level::Success => "done",
        Level::Wait => "in progress",
        Level::Paused => "paused",
        _ => "pending",
    }
}

fn colour(level: &Level) -> Color {
    match level {
        Level::Success => Color::Green,
        Level::Pending => Color::Magenta,
        Level::Wait => Color::Blue,
        Level::Paused => Color::Yellow,
        Level::Cancelled => DIM,
        Level::Note => Color::Blue,
        Level::Error => Color::Red,
    }
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
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

    fn state_with(rows: Vec<Row>, items: Vec<Item>) -> State {
        State {
            items,
            rows,
            filter: String::new(),
            selected: 0,
            offset: 0,
            stats: Stats {
                percent: 0,
                complete: 0,
                in_progress: 0,
                paused: 0,
                cancelled: 0,
                pending: 0,
                notes: 0,
                stashed: 0,
                trashed: 0,
            },
            month: CalendarMonth::of(2026, 8, Some(28)).unwrap(),
            flash: None,
        }
    }

    fn note(id: u32) -> Item {
        Item::new_note(id, format!("item {id}"), vec!["@a".to_string()])
    }

    /// A heading is a row like any other, so moving has to step over it.
    /// Landing on one would select nothing and then act on nothing, which
    /// reads as the key being broken.
    #[test]
    fn moving_skips_board_headings() {
        let rows = vec![
            Row::Board { name: "@a".into(), complete: 0, tasks: 1 },
            Row::Item(0),
            Row::Board { name: "@b".into(), complete: 0, tasks: 1 },
            Row::Item(1),
        ];
        let mut state = state_with(rows, vec![note(1), note(2)]);

        state.step(1);
        assert_eq!(state.selected, 1, "did not skip the first heading");
        state.step(1);
        assert_eq!(state.selected, 3, "did not skip the second heading");
        state.step(1);
        assert_eq!(state.selected, 1, "did not wrap back past the heading");
    }

    /// A list with headings and nothing else must not spin forever looking
    /// for something to select.
    #[test]
    fn moving_through_headings_alone_terminates() {
        let rows = vec![Row::Board { name: "@a".into(), complete: 0, tasks: 0 }];
        let mut state = state_with(rows, Vec::new());

        state.step(1);

        assert_eq!(state.selected, 0);
        assert!(state.selected_item().is_none());
    }

    /// Without a viewport the list stops at the fold and everything past
    /// it is unreachable -- on a 96-item board that was 71 items you could
    /// not get to, which is most of why this mode was not useful.
    #[test]
    fn the_viewport_follows_the_selection_in_both_directions() {
        let rows: Vec<Row> = (0..50).map(Row::Item).collect();
        let items: Vec<Item> = (0..50).map(|i| note(i as u32)).collect();
        let mut state = state_with(rows, items);

        state.selected = 40;
        state.scroll_into_view(10);
        assert_eq!(state.offset, 31, "scrolled further than needed");

        state.selected = 5;
        state.scroll_into_view(10);
        assert_eq!(state.offset, 5, "did not follow the selection back up");
    }

    #[test]
    fn a_selection_already_on_screen_does_not_scroll() {
        let rows: Vec<Row> = (0..50).map(Row::Item).collect();
        let items: Vec<Item> = (0..50).map(|i| note(i as u32)).collect();
        let mut state = state_with(rows, items);
        state.offset = 10;

        state.selected = 15;
        state.scroll_into_view(10);

        assert_eq!(state.offset, 10);
    }

    #[test]
    fn wrapping_breaks_on_whitespace_and_keeps_every_word() {
        let lines = wrap("damage is in surface coordinates not output coordinates", 20);

        assert!(lines.iter().all(|line| line.chars().count() <= 20), "{lines:?}");
        assert_eq!(lines.join(" "), "damage is in surface coordinates not output coordinates");
    }
}
