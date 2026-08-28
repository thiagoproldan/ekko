//! The interactive mode: a third frontend on the same `Ekko` core.
//!
//! It never goes through `render.rs`. That module reproduces taskbook's
//! output byte for byte and is pinned by golden tests; nothing here can
//! disturb it, which is what makes "changes nothing about the CLI" a
//! property of the shape rather than a promise to be careful.
//!
//! What it does borrow from there is `Level`, so which icon a cancelled
//! task gets -- and the precedence between cancelled, complete,
//! in-progress and paused -- is decided in exactly one place. Two
//! surfaces disagreeing about that is the drift this project keeps
//! finding.
//!
//! The layout is the picker idiom: a list, a preview of whatever is
//! selected, and a prompt that filters as you type. That shape is the
//! point rather than decoration. On a real board notes took 43 of 85 item
//! lines, and folding could only truncate them to fit; a picker does not
//! have to truncate anything, because the list holds one line per item
//! and the full text lives in the preview, on demand.

use std::io::{self, Write};
use std::time::Duration;

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEventKind,
};
use crossterm::style::{Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::{cursor, execute, queue};

use crate::ekko::{Ekko, EkkoError, Outcome};
use crate::item::Item;
use crate::render::Level;

/// Restores the terminal when it goes out of scope, however that happens.
///
/// A CLI that panics leaves a mess in the scrollback. A raw-mode program
/// that panics leaves the shell itself unusable -- no echo, no line
/// editing, still on the alternate screen -- and the person has to know to
/// type `reset` blind. Tying teardown to a value's lifetime means the
/// unwind restores it for us, and the hook below covers the abort path.
struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture, cursor::Hide)?;

        // The guard handles unwind; this handles a panic that prints
        // before unwinding, so the message lands on a sane terminal
        // rather than a raw-mode one that renders it as a staircase.
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

/// Everything the loop needs, rebuilt from storage after every write.
struct State {
    items: Vec<Item>,
    filter: String,
    selected: usize,
    /// Rows on screen, so a click can be turned back into an item. Rebuilt
    /// every draw because the filter changes which item each row holds.
    rows: Vec<usize>,
    status: Option<String>,
}

impl State {
    fn matching(&self) -> Vec<usize> {
        let needle = self.filter.to_lowercase();
        self.items
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                needle.is_empty() || item.description.to_lowercase().contains(&needle)
            })
            .map(|(at, _)| at)
            .collect()
    }
}

/// Runs the interactive mode until the user quits.
pub fn run(ekko: &Ekko) -> Result<(), EkkoError> {
    let mut state = State {
        items: load(ekko)?,
        filter: String::new(),
        selected: 0,
        rows: Vec::new(),
        status: None,
    };

    let _guard = TerminalGuard::enter().map_err(io_error)?;

    loop {
        draw(&mut state).map_err(io_error)?;

        // A poll rather than a blocking read so a redraw can be triggered
        // by something other than a keystroke later -- noticing that the
        // board changed underneath us is the obvious one, and it needs a
        // loop that wakes up on its own.
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
            Event::Resize(_, _) => {}
            _ => {}
        }
    }
}

/// Reads the board flat, in id order.
///
/// Deliberately not grouped by board: the list is filtered by typing, and
/// a grouping that shifts under a filter reads as noise. The board a thing
/// belongs to is shown in the preview instead.
fn load(ekko: &Ekko) -> Result<Vec<Item>, EkkoError> {
    let Outcome::Board(groups) = ekko.display_by_board()? else {
        return Ok(Vec::new());
    };

    let mut items: Vec<Item> = Vec::new();
    for (_, group) in groups {
        for item in group {
            if !items.iter().any(|seen: &Item| seen.id == item.id) {
                items.push(item);
            }
        }
    }
    items.sort_by_key(|item| item.id);
    Ok(items)
}

fn io_error(e: io::Error) -> EkkoError {
    EkkoError::Storage(crate::storage::StorageError::Io(e))
}

/// Returns false when the loop should end.
fn handle_key(ekko: &Ekko, state: &mut State, key: KeyEvent) -> Result<bool, EkkoError> {
    let matched = state.matching();

    match key.code {
        KeyCode::Esc => return Ok(false),
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return Ok(false),

        KeyCode::Down => move_selection(state, 1),
        KeyCode::Up => move_selection(state, -1),
        // Ctrl-n/p as well as the arrows: the picker idiom this borrows
        // from binds both, and a hand already on the prompt should not
        // have to leave the home row.
        KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            move_selection(state, 1)
        }
        KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            move_selection(state, -1)
        }

        KeyCode::Backspace => {
            state.filter.pop();
            state.selected = 0;
        }
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.filter.push(c);
            state.selected = 0;
        }

        KeyCode::Enter => {
            if let Some(&at) = matched.get(state.selected) {
                act(ekko, state, at, "done")?;
            }
        }
        KeyCode::Tab => {
            if let Some(&at) = matched.get(state.selected) {
                let next = if state.items[at].in_progress.unwrap_or(false) {
                    "paused"
                } else {
                    "progress"
                };
                act(ekko, state, at, next)?;
            }
        }
        _ => {}
    }

    Ok(true)
}

fn handle_mouse(state: &mut State, mouse: event::MouseEvent) {
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            let row = mouse.row as usize;
            if row >= LIST_TOP && row - LIST_TOP < state.rows.len() {
                state.selected = row - LIST_TOP;
            }
        }
        MouseEventKind::ScrollDown => move_selection(state, 1),
        MouseEventKind::ScrollUp => move_selection(state, -1),
        _ => {}
    }
}

fn move_selection(state: &mut State, by: i32) {
    let count = state.matching().len();
    if count == 0 {
        state.selected = 0;
        return;
    }
    let next = state.selected as i32 + by;
    state.selected = next.rem_euclid(count as i32) as usize;
}

/// Applies a state change, then reloads.
///
/// The lock is taken by `set_state` and released the moment it returns --
/// never held while the loop sits idle. A UI parked on the flock would
/// block the user's other terminal and every agent, which is the exact
/// failure the lock exists to prevent, caused by the thing meant to help.
///
/// Reloading afterwards costs under 10ms on a real board and is what keeps
/// the screen honest about what actually landed, rather than about what
/// was asked for.
fn act(ekko: &Ekko, state: &mut State, at: usize, want: &str) -> Result<(), EkkoError> {
    let item = &state.items[at];
    if !item.is_task {
        state.status = Some("notes have no state".to_string());
        return Ok(());
    }

    let id = format!("@{}", item.id);
    match ekko.set_state(&[id, want.to_string()]) {
        Ok(_) => {
            state.items = load(ekko)?;
            state.status = None;
        }
        Err(e) => state.status = Some(e.to_string()),
    }
    Ok(())
}

// ---- drawing ---------------------------------------------------------

/// First row of the results list, below the panel's top border.
const LIST_TOP: usize = 1;

/// Below this the three panels cannot be drawn without overlapping.
const MIN_COLS: usize = 40;
const MIN_ROWS: usize = 8;

const BORDER: Color = Color::AnsiValue(240);
const DIM: Color = Color::AnsiValue(245);

fn draw(state: &mut State) -> io::Result<()> {
    let (cols, rows) = crossterm::terminal::size()?;
    let (cols, rows) = (cols as usize, rows as usize);
    let mut out = io::stdout();

    queue!(out, Clear(ClearType::All))?;

    // Below this there is no room for three bordered panels and anything
    // inside them, and drawing anyway produces overlapping garbage rather
    // than a small version of the layout. Say so instead.
    if cols < MIN_COLS || rows < MIN_ROWS {
        queue!(out, cursor::MoveTo(0, 0), Print("terminal too small"))?;
        return out.flush();
    }

    // Results and preview share the width; the prompt sits under results
    // only, the way the picker this borrows from lays it out. Three rows
    // for the prompt because a panel needs a border, a line to type on,
    // and a border.
    let prompt_h = 3usize;
    let results_h = rows - prompt_h;
    let list_rows = results_h - 2;

    let list_width = cols.saturating_sub(4) / 2;
    let preview_left = list_width + 2;
    let preview_width = cols - preview_left - 1;

    let matched = state.matching();
    if state.selected >= matched.len() {
        state.selected = matched.len().saturating_sub(1);
    }

    panel(&mut out, 0, 0, list_width, results_h, "Results")?;
    panel(&mut out, preview_left, 0, preview_width, rows, "Preview")?;
    panel(&mut out, 0, results_h, list_width, prompt_h, "Prompt")?;

    state.rows.clear();
    for (row, &at) in matched.iter().take(list_rows).enumerate() {
        state.rows.push(at);
        draw_row(&mut out, state, at, row, row == state.selected, list_width)?;
    }

    if let Some(&at) = matched.get(state.selected) {
        draw_preview(&mut out, &state.items[at], preview_left + 2, preview_width)?;
    }

    draw_prompt(&mut out, state, matched.len(), results_h + 1, list_width)?;

    out.flush()
}

/// A bordered box with its title centred in the top edge.
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
    at: usize,
    row: usize,
    selected: bool,
    width: usize,
) -> io::Result<()> {
    let item = &state.items[at];
    let level = Level::of(item);
    let indent = if item.anchor.is_some() { "  " } else { "" };

    queue!(out, cursor::MoveTo(1, (LIST_TOP + row) as u16))?;
    if selected {
        queue!(out, SetBackgroundColor(Color::AnsiValue(237)))?;
    }

    let marker = if selected { ">" } else { " " };
    let body = format!(
        "{marker} {indent}{:>3}. {} {}",
        item.id,
        level.icon(),
        item.description.replace('\n', " ")
    );
    let body: String = body.chars().take(width.saturating_sub(2)).collect();
    let pad = width.saturating_sub(2).saturating_sub(body.chars().count());

    queue!(out, SetForegroundColor(colour(&level)), Print(body), Print(" ".repeat(pad)))?;
    queue!(out, ResetColor)
}

fn draw_preview(out: &mut impl Write, item: &Item, x: usize, width: usize) -> io::Result<()> {
    let inner = width.saturating_sub(4);
    if inner == 0 {
        return Ok(());
    }
    let mut row = 2usize;

    let level = Level::of(item);
    queue!(out, cursor::MoveTo(x as u16, row as u16), SetForegroundColor(colour(&level)))?;
    queue!(out, Print(format!("{} {}", level.icon(), label(item))), ResetColor)?;
    row += 2;

    // Wrapped rather than folded. Folding exists because the board has one
    // line per item and a long note has to fit in it; here the whole point
    // of the pane is that it does not.
    for line in wrap(&item.description, inner) {
        queue!(out, cursor::MoveTo(x as u16, row as u16), Print(line))?;
        row += 1;
    }
    row += 1;

    let mut facts: Vec<String> = Vec::new();
    facts.push(format!("boards   {}", item.boards.join(", ")));
    if let Some(due) = &item.due_date {
        facts.push(format!("due      {due}"));
    }
    if let Some(blockers) = &item.blocked_by {
        facts.push(format!("waits on {} item(s)", blockers.len()));
    }
    if item.anchor.is_some() {
        facts.push("explains a task".to_string());
    }
    facts.push(format!("created  {}", item.date));

    queue!(out, SetForegroundColor(DIM))?;
    for fact in facts {
        queue!(out, cursor::MoveTo(x as u16, row as u16), Print(fact))?;
        row += 1;
    }
    queue!(out, ResetColor)
}

fn draw_prompt(
    out: &mut impl Write,
    state: &State,
    matched: usize,
    row: usize,
    width: usize,
) -> io::Result<()> {
    queue!(out, cursor::MoveTo(2, row as u16), SetForegroundColor(Color::Reset))?;
    queue!(out, Print(format!("> {}", state.filter)), SetAttribute(Attribute::Reverse), Print(" "))?;
    queue!(out, SetAttribute(Attribute::Reset))?;

    let tally = match &state.status {
        Some(message) => message.clone(),
        None => format!("{matched} / {}", state.items.len()),
    };
    let at = width.saturating_sub(tally.chars().count() + 2);
    queue!(out, cursor::MoveTo(at as u16, row as u16), SetForegroundColor(DIM), Print(tally))?;
    queue!(out, ResetColor)
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

/// Greedy wrap on whitespace, wide enough for the pane.
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

    fn note(id: u32, description: &str) -> Item {
        Item::new_note(id, description.to_string(), vec!["@a".to_string()])
    }

    #[test]
    fn the_filter_matches_on_description_regardless_of_case() {
        let state = State {
            items: vec![note(1, "Vendor wlroots"), note(2, "damage tracking")],
            filter: "DAMAGE".to_string(),
            selected: 0,
            rows: Vec::new(),
            status: None,
        };

        assert_eq!(state.matching(), vec![1]);
    }

    /// Wrapping around the ends rather than stopping: a picker with a
    /// short list is scrolled past constantly, and stopping dead at the
    /// bottom is the more annoying of the two behaviours.
    #[test]
    fn moving_past_either_end_wraps() {
        let mut state = State {
            items: vec![note(1, "one"), note(2, "two")],
            filter: String::new(),
            selected: 0,
            rows: Vec::new(),
            status: None,
        };

        move_selection(&mut state, -1);
        assert_eq!(state.selected, 1);
        move_selection(&mut state, 1);
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn moving_in_an_empty_list_stays_put_rather_than_dividing_by_zero() {
        let mut state =
            State { items: Vec::new(), filter: String::new(), selected: 0, rows: Vec::new(), status: None };

        move_selection(&mut state, 1);

        assert_eq!(state.selected, 0);
    }

    #[test]
    fn wrapping_breaks_on_whitespace_and_keeps_every_word() {
        let lines = wrap("damage is in surface coordinates not output coordinates", 20);

        assert!(lines.iter().all(|line| line.chars().count() <= 20), "{lines:?}");
        assert_eq!(lines.join(" "), "damage is in surface coordinates not output coordinates");
    }
}
