//! Pretty terminal output. Ported from `render.js` (chalk + signale) by
//! reproducing the *observed* byte output exactly rather than re-deriving
//! it from signale's source -- this session captured real, colored output
//! from the JS build and decoded every icon/color code from the raw bytes
//! before writing a line of this file.
//!
//! Deliberately hand-rolled ANSI wrapping instead of a color crate: the
//! nesting order matters byte-for-byte here (underline+color opens as
//! `\x1b[4m\x1b[33m`, closes as `\x1b[39m\x1b[24m` -- color resets before
//! underline does, mirroring how they were opened), and plain string
//! composition gives exact control over that without fighting a crate's
//! own idea of how to compose styles.
//!
//! Writes go through an injected `dyn Write` rather than straight to
//! `println!`, specifically so tests can capture the exact bytes produced
//! and diff them against golden output captured from the real JS build,
//! not just eyeball similarity.
//!
//! This module only renders for a human -- it doesn't know `--json`
//! exists. Deciding pretty vs. JSON output happens one layer up, where the
//! richer structured data a JSON response needs is available anyway.

use std::io::{IsTerminal, Write};

use chrono::{DateTime, Local};

use crate::config::Config;
use crate::item::Item;

const PRIORITY_NORMAL: u8 = 1;
const PRIORITY_MEDIUM: u8 = 2;
const PRIORITY_HIGH: u8 = 3;

/// Below this many columns of usable space, folding stops helping: the
/// "(+N lines)" marker would eat most of what is left, so leave the note
/// whole and let the terminal wrap it.
const MIN_FOLD_WIDTH: usize = 24;

/// How long the trash keeps something before dropping it.
///
/// Long enough that "I deleted the wrong thing" is recoverable days
/// later, short enough that the trash does not become a second board.
pub const TRASH_DAYS: i64 = 30;

/// Decides whether ANSI codes get emitted at all. Real usage auto-detects
/// (colors for a real terminal, plain text once piped/redirected, same as
/// chalk); `NO_COLOR`/`FORCE_COLOR` override that, and tests construct one
/// directly instead of touching real env vars or a real terminal.
pub struct Painter {
    enabled: bool,
}

impl Painter {
    pub fn auto() -> Self {
        let no_color = std::env::var_os("NO_COLOR").is_some();
        let force_color = std::env::var_os("FORCE_COLOR").is_some();
        Painter { enabled: !no_color && (force_color || std::io::stdout().is_terminal()) }
    }

    /// Colour on/off regardless of TTY or environment. Only the tests
    /// need this -- production always goes through `auto`, which is what
    /// decides colour from `NO_COLOR`/`FORCE_COLOR`/isatty.
    #[cfg(test)]
    pub fn forced(enabled: bool) -> Self {
        Painter { enabled }
    }

    fn wrap(&self, open: &str, close: &str, text: &str) -> String {
        if !self.enabled {
            return text.to_string();
        }

        let open_seq = format!("\x1b[{open}m");
        let close_seq = format!("\x1b[{close}m");

        // Reproduces how chalk closes a *nested* style: rather than emitting
        // a plain reset, which would drop back to the terminal default and
        // silently lose the enclosing style, it rewrites any inner close of
        // this same style into a re-open of it. `grey(green(x) + rest)` has
        // to come out as `90 32 x 90 rest 39`, not `90 32 x 39 rest 39` --
        // otherwise `rest` renders in the default colour instead of grey.
        let inner = text.replace(&close_seq, &open_seq);

        format!("{open_seq}{inner}{close_seq}")
    }

    fn grey(&self, text: &str) -> String {
        self.wrap("90", "39", text)
    }

    fn green(&self, text: &str) -> String {
        self.wrap("32", "39", text)
    }

    fn blue(&self, text: &str) -> String {
        self.wrap("34", "39", text)
    }

    fn magenta(&self, text: &str) -> String {
        self.wrap("35", "39", text)
    }

    fn yellow(&self, text: &str) -> String {
        self.wrap("33", "39", text)
    }

    fn red(&self, text: &str) -> String {
        self.wrap("31", "39", text)
    }

    fn underline(&self, text: &str) -> String {
        self.wrap("4", "24", text)
    }

    /// SGR 9. Same nesting rules as every other style here, so a struck
    /// description that already carries colour closes correctly.
    fn strike(&self, text: &str) -> String {
        self.wrap("9", "29", text)
    }
}

/// A month laid out as weeks, ready to draw.
///
/// Computed rather than derived at draw time so it can be tested against a
/// fixed date -- a calendar built from `Local::now()` inside the renderer
/// would only be checkable on the day it was written.
///
/// Weeks start on Sunday, which is what `cal(1)` does on this system and
/// what a Brazilian wall calendar does. `None` is padding before the first
/// and after the last day.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CalendarMonth {
    pub year: i32,
    pub month: u32,
    pub label: String,
    pub weeks: Vec<Vec<Option<u32>>>,
    /// Day of the month, when today falls inside this month.
    pub today: Option<u32>,
}

impl CalendarMonth {
    pub fn of(year: i32, month: u32, today: Option<u32>) -> Option<CalendarMonth> {
        use chrono::{Datelike, NaiveDate};

        let first = NaiveDate::from_ymd_opt(year, month, 1)?;
        let next = match month {
            12 => NaiveDate::from_ymd_opt(year + 1, 1, 1)?,
            _ => NaiveDate::from_ymd_opt(year, month + 1, 1)?,
        };
        let days = next.signed_duration_since(first).num_days() as u32;
        let lead = first.weekday().num_days_from_sunday() as usize;

        let mut weeks: Vec<Vec<Option<u32>>> = Vec::new();
        let mut week: Vec<Option<u32>> = vec![None; lead];
        for day in 1..=days {
            week.push(Some(day));
            if week.len() == 7 {
                weeks.push(std::mem::take(&mut week));
            }
        }
        if !week.is_empty() {
            week.resize(7, None);
            weeks.push(week);
        }

        Some(CalendarMonth {
            year,
            month,
            label: format!("{} {year}", MONTHS[month as usize - 1]),
            weeks,
            today,
        })
    }
}

const MONTHS: [&str; 12] = [
    "January", "February", "March", "April", "May", "June", "July", "August", "September",
    "October", "November", "December",
];

/// One project in `--projects`: its name and what it holds. Same counts a
/// board title carries, so the listing and the board agree on what `[1/6]`
/// means -- tasks only, notes reported separately.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProjectSummary {
    pub name: String,
    pub complete: u32,
    pub tasks: u32,
    pub notes: u32,
}

/// One node of the path: a declared phase, how far it has got, and whether
/// work currently sits in it. Lives here beside `Stats` for the same reason
/// -- a shape the renderer is given, computed elsewhere.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PathStep {
    pub name: String,
    pub complete: u32,
    pub total: u32,
    pub notes: u32,
    pub current: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub percent: u32,
    pub complete: u32,
    pub in_progress: u32,
    /// Started, then set aside. Only surfaces in the stats line when it is
    /// above zero, so a board nobody pauses prints exactly what it always
    /// did -- goldens included.
    pub paused: u32,
    /// Abandoned on purpose. Counted, but deliberately kept out of the
    /// percentage denominator: cancelled work is not work, so a board that
    /// drops something can still reach 100%.
    pub cancelled: u32,
    pub pending: u32,
    pub notes: u32,
    /// Put away, and thrown away. Counted *instead of* whatever the item
    /// was rather than as well, so the line still sums to the board -- a
    /// stashed done task appears here and not under `done`. The item keeps
    /// its real state underneath; only the counting hides it.
    ///
    /// Shown only above zero, the same rule paused and cancelled follow,
    /// which is what keeps a board that uses neither byte-identical.
    pub stashed: u32,
    pub trashed: u32,
}

/// Board or date groupings, in the order they should display -- callers
/// (ekko.rs) own how that order is produced; this module just walks it.
pub type Groups = [(String, Vec<Item>)];

/// Which of the six item appearances applies. Public so a second
/// frontend can ask the same question rather than re-deriving the
/// precedence between cancelled, complete, in-progress and paused --
/// getting that order wrong in one place and not the other is exactly
/// how two surfaces drift apart.
pub enum Level {
    Success, // complete task
    Pending, // pending task
    Wait,    // in-progress task
    Paused,  // started, then set aside
    Cancelled, // abandoned on purpose, kept for the record
    Note,
    Error,
}

impl Level {
    /// Which appearance an item has, decided in one place.
    ///
    /// The precedence matters and is easy to get subtly different on a
    /// second reading: cancelled wins over complete because both are
    /// terminal, and in-progress wins over paused because if stale data
    /// ever claims both, "being worked on" is the more useful lie to
    /// believe.
    pub fn of(item: &Item) -> Level {
        if !item.is_task {
            return Level::Note;
        }
        if item.cancelled.unwrap_or(false) {
            Level::Cancelled
        } else if item.is_complete.unwrap_or(false) {
            Level::Success
        } else if item.in_progress.unwrap_or(false) {
            Level::Wait
        } else if item.paused.unwrap_or(false) {
            Level::Paused
        } else {
            Level::Pending
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
            Level::Success => "\u{2714}", // ✔
            Level::Pending => "\u{2610}", // ☐
            Level::Wait => "\u{2026}",    // …
            Level::Paused => "\u{23f8}",  // ⏸ -- no variation selector, so it stays one column wide like the rest
            Level::Cancelled => "\u{2298}", // ⊘ -- covered by the mono fonts, unlike the pause glyph
            Level::Note => "\u{25cf}",    // ●
            Level::Error => "\u{2716}",   // ✖
        }
    }

    fn paint(&self, painter: &Painter, text: &str) -> String {
        match self {
            Level::Success => painter.green(text),
            Level::Pending => painter.magenta(text),
            Level::Wait => painter.blue(text),
            Level::Paused => painter.yellow(text),
            Level::Cancelled => painter.grey(text),
            Level::Note => painter.blue(text),
            Level::Error => painter.red(text),
        }
    }
}

pub struct Renderer<'a> {
    painter: Painter,
    config: Config,
    out: &'a mut dyn Write,
    /// Captured once, at construction, rather than read per group: one
    /// render pass has to be internally consistent -- a run that straddles
    /// midnight must not tag one group `[Today]` and the next one not --
    /// and pinning it is what lets the golden tests reproduce output that
    /// was captured on a specific date.
    now: DateTime<Local>,
    /// Width to fold long notes to, or `None` to leave them whole.
    ///
    /// Deliberately not derived from `Painter`: that folds colour in with
    /// `NO_COLOR`/`FORCE_COLOR`, and `FORCE_COLOR=1 ekko > file` must still
    /// write every note in full. Folding is about a screen, so it asks the
    /// screen directly.
    fold_width: Option<usize>,
    /// Unmet blockers per item id, computed where the whole map is in scope
    /// and handed in, since blockers can point at items outside the current
    /// view and resolving them from the view alone would report the wrong
    /// answer.
    blockers: std::collections::HashMap<u32, Vec<u32>>,
}

impl<'a> Renderer<'a> {
    pub fn new(painter: Painter, config: Config, out: &'a mut dyn Write) -> Self {
        let mut renderer = Self::at(painter, config, out, Local::now());
        renderer.fold_width = terminal_width();
        renderer
    }

    /// `new` with the clock pinned. The golden `.ans` references embed the
    /// date they were captured on (the timeline and archive headers print
    /// it, and add `[Today]` when it matches), so the tests that diff
    /// against them have to render as of that same instant -- otherwise
    /// they would pass only on the day of capture.
    fn at(painter: Painter, config: Config, out: &'a mut dyn Write, now: DateTime<Local>) -> Self {
        // Folding off by default, so every test -- the byte-for-byte golden
        // ones above all -- renders exactly what it always did.
        Renderer {
            painter,
            config,
            out,
            now,
            fold_width: None,
            blockers: std::collections::HashMap::new(),
        }
    }

    /// Hands the renderer the unmet blockers it cannot work out for itself.
    pub fn with_blockers(&mut self, blockers: std::collections::HashMap<u32, Vec<u32>>) {
        self.blockers = blockers;
    }

    fn today(&self) -> String {
        self.now.format("%a %b %d %Y").to_string()
    }

    fn now_millis(&self) -> i64 {
        self.now.timestamp_millis()
    }

    // ---- layout building blocks -----------------------------------

    fn item_level(&self, item: &Item) -> Level {
        Level::of(item)
    }

    /// The id column, indented two further for a note that explains a task.
    ///
    /// The indent is the whole point of anchoring: a reason sitting at the
    /// same depth as the work reads as another item competing for
    /// attention, which is how a board of long notes became a wall. One
    /// step in and it reads as belonging to the line above it.
    ///
    /// An unanchored note is untouched, so every board that does not use
    /// this renders exactly as it did -- goldens included.
    fn build_prefix(&self, item: &Item) -> String {
        let id = item.id.to_string();
        let padding = " ".repeat(4usize.saturating_sub(id.len()));
        let indent = if item.anchor.is_some() { "  " } else { "" };
        format!("{indent}{padding} {}", self.painter.grey(&format!("{id}.")))
    }

    /// Shortens a note that would wrap, to one line plus a count of what was
    /// hidden. Returns the description untouched when there is nothing to
    /// gain, or when folding is off.
    ///
    /// Notes only. They are prose and hold the reasoning worth keeping, which
    /// is exactly why they run long -- on a real board they took 56 of 94
    /// rendered lines while every task, open and closed, took 28. Tasks stay
    /// whole because a truncated task hides something you are meant to act
    /// on, where a truncated note hides something you can go and read.
    fn fold_note(&self, item: &Item) -> String {
        let Some(width) = self.fold_width else {
            return item.description.clone();
        };
        if item.is_task {
            return item.description.clone();
        }

        // Everything the line spends before the description: the id column,
        // the icon and their separators.
        let overhead = 12;
        let available = width.saturating_sub(overhead);
        let chars = item.description.chars().count();
        if available < MIN_FOLD_WIDTH || chars <= available {
            return item.description.clone();
        }

        let hidden = chars.div_ceil(available) - 1;
        let plural = if hidden == 1 { "line" } else { "lines" };
        let suffix = format!("\u{2026} (+{hidden} {plural})");
        let keep = available.saturating_sub(suffix.chars().count());

        let head: String = item.description.chars().take(keep).collect();
        format!("{head}{suffix}")
    }

    fn build_message(&self, item: &Item) -> String {
        let is_complete = item.is_complete.unwrap_or(false);
        let priority = item.priority.unwrap_or(0);
        let description = self.fold_note(item);

        let mut parts = Vec::new();

        if item.cancelled.unwrap_or(false) {
            // Struck through in the same grey the icon uses: the strike carries
            // "dropped", the grey carries "no longer live". Priority markers
            // are left off -- an abandoned task has no urgency left.
            parts.push(self.painter.strike(&self.painter.grey(&description)));
        } else if !is_complete && priority > PRIORITY_NORMAL {
            let colored = match priority {
                PRIORITY_MEDIUM => self.painter.yellow(&description),
                _ => self.painter.red(&description),
            };
            parts.push(self.painter.underline(&colored));
        } else if is_complete {
            parts.push(self.painter.grey(&description));
        } else {
            parts.push(description);
        }

        // Cancelled excluded alongside complete: an abandoned task has no
        // urgency left, and a struck-through line still shouting "(!!)" reads
        // as a contradiction.
        if !is_complete && !item.cancelled.unwrap_or(false) && priority > PRIORITY_NORMAL {
            parts.push(if priority == PRIORITY_MEDIUM {
                self.painter.yellow("(!)")
            } else {
                self.painter.red("(!!)")
            });
        }

        parts.join(" ")
    }

    fn get_star(&self, item: &Item) -> String {
        if item.is_starred { self.painter.yellow("\u{2605}") } else { String::new() }
    }

    fn get_age_days(&self, timestamp: i64, now_millis: i64) -> String {
        let day_ms: i64 = 24 * 60 * 60 * 1000;
        let age = ((now_millis - timestamp).abs() as f64 / day_ms as f64).round() as i64;
        if age == 0 { String::new() } else { self.painter.grey(&format!("{age}d")) }
    }

/// The due date, coloured by how it stands against today: overdue red,
/// due today yellow, still ahead grey. Empty when the item has no due
/// date, which is what keeps output for every pre-existing item -- and
    /// The `⇠ ids` marker, listing only the blockers still outstanding.
    ///
    /// A satisfied blocker vanishes on its own, so the marker always names
    /// what is holding the item up *now* -- nothing ever has to be unblocked
    /// by hand. Empty when there is nothing outstanding, which is what keeps
    /// every pre-existing board byte-identical.
    fn get_blocked(&self, item: &Item) -> String {
        let Some(ids) = self.blockers.get(&item.id) else { return String::new() };
        if ids.is_empty() {
            return String::new();
        }
        let list = ids.iter().map(u32::to_string).collect::<Vec<_>>().join(", ");
        self.painter.grey(&format!("\u{21e0} {list}"))
    }

/// every golden reference -- byte-identical.
///
/// Compared as plain strings: `parse_due_date` canonicalises to
/// `YYYY-MM-DD`, which sorts lexicographically the same way it sorts
/// chronologically.
    fn get_due(&self, item: &Item) -> String {
        let Some(due) = item.due_date.as_deref() else { return String::new() };

        // A finished task's deadline is history, not a warning.
        if item.is_complete.unwrap_or(false) {
            return self.painter.grey(due);
        }

        let today = self.now.format("%Y-%m-%d").to_string();
        match due.cmp(today.as_str()) {
            std::cmp::Ordering::Less => self.painter.red(due),
            std::cmp::Ordering::Equal => self.painter.yellow(due),
            std::cmp::Ordering::Greater => self.painter.grey(due),
        }
    }

    fn color_boards(&self, boards: &[String]) -> String {
        boards.iter().map(|b| self.painter.grey(b)).collect::<Vec<_>>().join(" ")
    }

    fn item_stats(&self, items: &[Item]) -> (u32, u32, u32) {
        let mut tasks = 0;
        let mut complete = 0;
        let mut notes = 0;
        for item in items {
            if item.is_task {
                tasks += 1;
                if item.is_complete.unwrap_or(false) {
                    complete += 1;
                }
            } else {
                notes += 1;
            }
        }
        (tasks, complete, notes)
    }

    fn is_group_complete(&self, items: &[Item]) -> bool {
        let (tasks, complete, notes) = self.item_stats(items);
        tasks == complete && notes == 0
    }

    fn correlation(&self, items: &[Item]) -> String {
        let (tasks, complete, _) = self.item_stats(items);
        self.painter.grey(&format!("[{complete}/{tasks}]"))
    }

    // ---- line emission ----------------------------------------------

    /// `prefix + ' ' + icon + "  " + message + (suffix, if any, as ' ' + suffix)`
    /// -- the exact layout signale produced, reverse-engineered from real
    /// captured output rather than assumed.
    fn emit(&mut self, prefix: &str, icon: Option<&Level>, message: &str, suffix: &str) {
        let mut line = String::from(prefix);
        line.push(' ');
        if let Some(level) = icon {
            // The trailing space is colored *with* the icon (signale bakes
            // it into the icon glyph itself) -- only one more, uncolored,
            // space separates it from the message. Confirmed against raw
            // captured bytes, not assumed: naively coloring just the glyph
            // and adding two plain spaces after produced a visible diff.
            line.push_str(&level.paint(&self.painter, &format!("{} ", level.icon())));
            line.push(' ');
        }
        line.push_str(message);
        if !suffix.is_empty() {
            line.push(' ');
            line.push_str(suffix);
        }
        // A CLI writing a few dozen lines to stdout/a buffer essentially
        // never fails; not worth a `Result` on every public method here
        // for it (same posture the JS version took with plain console
        // writes -- it didn't handle this either).
        let _ = writeln!(self.out, "{line}");
    }

    fn display_title(&mut self, key: &str, items: &[Item], today: &str) {
        let title = if key == today {
            format!("{} {}", self.painter.underline(key), self.painter.grey("[Today]"))
        } else {
            self.painter.underline(key)
        };
        let suffix = self.correlation(items);
        self.emit("\n ", None, &title, &suffix);
    }

    fn display_item_by_board(&mut self, item: &Item, now_millis: i64) {

        let level = self.item_level(item);
        let age = self.get_age_days(item.timestamp, now_millis);
        let star = self.get_star(item);
        let due = self.get_due(item);
        let blocked = self.get_blocked(item);
        // Spelled out rather than filtered-and-joined so the two pre-existing
        // cases keep their exact spacing, trailing space included.
        let suffix = match (age.is_empty(), due.is_empty()) {
            (true, true) => star,
            (false, true) => format!("{age} {star}"),
            (true, false) => format!("{due} {star}"),
            (false, false) => format!("{age} {due} {star}"),
        };
        let suffix = if blocked.is_empty() { suffix } else { format!("{blocked} {suffix}") };
        let prefix = self.build_prefix(item);
        let message = self.build_message(item);
        self.emit(&prefix, Some(&level), &message, &suffix);
    }

    fn display_item_by_date(&mut self, item: &Item) {
        let level = self.item_level(item);
        let boards: Vec<String> =
            item.boards.iter().filter(|b| b.as_str() != "My Board").cloned().collect();
        let due = self.get_due(item);
        let suffix = if due.is_empty() {
            format!("{} {}", self.color_boards(&boards), self.get_star(item))
        } else {
            format!("{} {} {}", self.color_boards(&boards), due, self.get_star(item))
        };
        // Same shape as the board view: prepended, and empty when nothing is
        // outstanding, which is what keeps the timeline goldens byte-identical.
        // The board drew this from the day dependencies landed and this view
        // did not -- one surface got the feature and its sibling did not.
        let blocked = self.get_blocked(item);
        let suffix = if blocked.is_empty() { suffix } else { format!("{blocked} {suffix}") };
        let prefix = self.build_prefix(item);
        let message = self.build_message(item);
        self.emit(&prefix, Some(&level), &message, &suffix);
    }

    // ---- public: views ------------------------------------------------

    /// Names the project a board belongs to, when one is active.
    ///
    /// Without this, `EKKO_PROJECT` set and forgotten would show a different
    /// board with nothing on screen saying so -- invisible state changing
    /// what you see, which is the failure mode this whole file keeps trying
    /// to avoid. Printed only when a project is active, so the default board
    /// is untouched.
    pub fn display_project(&mut self, name: &str) {
        let line = format!("{} {}", self.painter.grey("project:"), self.painter.underline(name));
        self.emit("\n ", None, &line, "");
    }

    /// The projects that exist, or a nudge when there are none yet.
    /// The journey through a project's phases: what is behind, where work
    /// sits now, and what is still ahead.
    ///
    /// Filled for phases that are done, marked for the one holding work,
    /// hollow for the ones nobody has started -- so the same picture reads
    /// backwards as history and forwards as a plan.
    pub fn display_path(&mut self, steps: &[PathStep], rootless: u32) {
        if steps.is_empty() {
            self.emit(
                "\n ",
                None,
                "No phases yet -- declare them with `ekko --phases <first> <second> ...`",
                "",
            );
            return;
        }

        let mut nodes = Vec::new();
        let mut counts = Vec::new();
        for step in steps {
            let done = step.total > 0 && step.complete == step.total;
            // Name and node painted together. Painting only the name left
            // every dot in the default colour, so behind/here/ahead read as
            // one weight at a glance -- the shape carried the distinction
            // alone and the colour that should reinforce it was absent.
            let icon = if step.current {
                "\u{25c9}" // ◉ here
            } else if done {
                "\u{25cf}" // ● behind
            } else {
                "\u{25cb}" // ○ ahead
            };
            let label = format!("{} {icon}", step.name);
            let head = if step.current {
                self.painter.blue(&label)
            } else if done {
                self.painter.green(&label)
            } else {
                self.painter.grey(&label)
            };

            // Widths come from the plain strings and the padding is appended
            // outside the colour: an escape sequence has no width on screen
            // but plenty of chars, so padding a painted string lines nothing
            // up. `{head:width$}` did exactly that and silently added no
            // spaces whenever a name was shorter than its own tally.
            let plain = if step.current {
                format!("{}/{} HERE", step.complete, step.total)
            } else {
                format!("{}/{}", step.complete, step.total)
            };
            let width = step.name.chars().count().max(plain.chars().count()) + 2;
            let tally = if step.current {
                self.painter.blue(&format!("{plain:width$}"))
            } else {
                self.painter.grey(&format!("{plain:width$}"))
            };

            let pad = width.saturating_sub(label.chars().count());
            nodes.push(format!("{head}{:pad$}", ""));
            counts.push(tally);
        }

        let joined = nodes.join(&self.painter.grey("\u{2500}\u{2500}\u{2500}"));
        self.emit("\n ", None, &joined, "");
        self.emit(" ", None, &counts.join("   "), "");

        let mut tail = Vec::new();
        let notes: u32 = steps.iter().map(|s| s.notes).sum();
        if notes > 0 {
            tail.push(format!("{notes} {}", if notes == 1 { "note" } else { "notes" }));
        }
        if rootless > 0 {
            // Named, not hidden: the project root is a deliberate exception
            // to "areas live inside phases", so it has to be visible.
            tail.push(format!("{rootless} outside any phase"));
        }
        if !tail.is_empty() {
            let line = self.painter.grey(&tail.join(" \u{b7} "));
            self.emit("\n ", None, &line, "\n");
        }
    }


    /// Reports what an item now waits on, or that it waits on nothing.
    pub fn success_stashed(&mut self, ids: &[u32], away: bool) {
        let verb = if away { "Stashed" } else { "Unstashed" };
        self.mark(verb, "items", "item", ids);
    }

    pub fn success_trashed(&mut self, ids: &[u32], away: bool) {
        let verb = if away { "Trashed" } else { "Recovered" };
        self.mark(verb, "items", "item", ids);
    }

    /// What is put away, grouped by the board it came from.
    ///
    /// Grouped rather than flat because the grouping *is* the context: a
    /// note explaining four tasks is only worth anything beside them, and
    /// putting a finished area out of the way should not shred it on the
    /// way out.
    pub fn display_stash(&mut self, groups: &Groups) {
        if groups.is_empty() {
            self.emit("\n ", None, "Nothing stashed", "");
            return;
        }
        let now_millis = self.now_millis();
        for (board, items) in groups {
            let oldest = items.iter().filter_map(|item| item.stashed).min();
            let since = oldest
                .map(|at| self.painter.grey(&format!("(stashed {})", self.ago(at, now_millis))))
                .unwrap_or_default();
            self.emit("\n ", None, &self.painter.underline(board), &since);
            for item in items {
                self.display_item_by_board(item, now_millis);
            }
        }
    }

    /// What is in the trash, and how long each thing has left.
    ///
    /// The countdown is the point. Without it "expires" is a promise
    /// nobody can see coming, and the first time anyone learns the trash
    /// empties is when they go looking for something that is gone.
    pub fn display_trash(&mut self, items: &[Item]) {
        if items.is_empty() {
            self.emit("\n ", None, "Nothing in the trash", "");
            return;
        }
        let now_millis = self.now_millis();
        self.emit("\n ", None, &self.painter.underline("Trash"), "");
        for item in items {
            let left = item
                .trashed
                .map(|at| self.expires_in(at, now_millis))
                .unwrap_or_default();
            let level = Level::of(item);
            let prefix = self.build_prefix(item);
            let message = self.build_message(item);
            self.emit(&prefix, Some(&level), &message, &left);
        }
    }

    /// Whole days since an instant, worded for a person.
    fn ago(&self, at: i64, now_millis: i64) -> String {
        let days = (now_millis - at) / 86_400_000;
        match days {
            d if d <= 0 => "today".to_string(),
            1 => "yesterday".to_string(),
            d => format!("{d}d ago"),
        }
    }

    /// Days left before the trash drops it, coloured as it runs out.
    fn expires_in(&self, trashed_at: i64, now_millis: i64) -> String {
        let elapsed = (now_millis - trashed_at) / 86_400_000;
        let left = TRASH_DAYS - elapsed;
        let text = match left {
            d if d <= 0 => "expires today".to_string(),
            1 => "expires tomorrow".to_string(),
            d => format!("expires in {d}d"),
        };
        // Red at the end, the same urgency vocabulary due dates already
        // use, so the last week reads as a deadline rather than a fact.
        if left <= 7 {
            self.painter.red(&text)
        } else {
            self.painter.grey(&text)
        }
    }

    pub fn success_anchored(&mut self, id: u32, target: Option<u32>) {
        match target {
            Some(target) => {
                let suffix = self.painter.grey(&target.to_string());
                self.success(" ", &format!("Note {id} now explains:"), &suffix);
            }
            None => self.success(" ", &format!("Note {id} explains nothing in particular"), ""),
        }
    }

    pub fn success_blocked(&mut self, id: u32, blockers: &[u32]) {
        if blockers.is_empty() {
            self.success(" ", &format!("Item {id} waits on nothing"), "");
            return;
        }
        let suffix = self.painter.grey(&join_ids(blockers));
        self.success(" ", &format!("Item {id} now waits on:"), &suffix);
    }
    /// A month, with today picked out.
    ///
    /// Nothing from the board is on it yet. That is deliberate for a first
    /// cut: the drawing and the question of what a day should show are two
    /// decisions, and the second one -- whether a day means "due" or
    /// "created" -- has not been made.
    pub fn display_calendar(&mut self, month: &CalendarMonth) {
        let width: usize = 20; // seven columns of two, six single spaces between
        let pad = width.saturating_sub(month.label.chars().count()) / 2;
        self.emit("\n ", None, &format!("{}{}", " ".repeat(pad), self.painter.underline(&month.label)), "");

        let head = ["Su", "Mo", "Tu", "We", "Th", "Fr", "Sa"].join(" ");
        self.emit(" ", None, &self.painter.grey(&head), "");

        for week in &month.weeks {
            let cells: Vec<String> = week
                .iter()
                .map(|day| match day {
                    None => "  ".to_string(),
                    Some(day) if Some(*day) == month.today => {
                        // Today is the one thing a calendar with no items on
                        // it can still tell you, so it is the one thing
                        // painted.
                        self.painter.yellow(&format!("{day:>2}"))
                    }
                    Some(day) => format!("{day:>2}"),
                })
                .collect();
            self.emit(" ", None, cells.join(" ").trim_end(), "");
        }
    }

    /// A project's declared phase sequence, in order.
    ///
    /// Split from `display_projects` when that grew counts: the two were
    /// sharing a renderer only because a list of names and a list of names
    /// looked alike, and they stopped looking alike.
    pub fn display_phases(&mut self, names: &[String]) {
        for name in names {
            self.emit("\n ", None, &self.painter.underline(name), "");
        }
    }

    /// The projects that exist, each with what it holds.
    ///
    /// The counts are the point, not decoration: a project is the one thing
    /// in Ekko with no archive behind it, so knowing it holds fifteen tasks
    /// has to be possible *before* acting on it rather than after. Notes are
    /// reported separately and only when there are any, the same way the
    /// stats line treats paused and cancelled.
    pub fn display_projects(&mut self, projects: &[ProjectSummary]) {
        if projects.is_empty() {
            self.emit("\n ", None, "No projects yet -- create one with `ekko --project <name> --create`", "");
            return;
        }
        for project in projects {
            let title = self.painter.underline(&project.name);
            let mut suffix =
                self.painter.grey(&format!("[{}/{}]", project.complete, project.tasks));
            if project.notes > 0 {
                let word = if project.notes == 1 { "note" } else { "notes" };
                suffix.push_str(&self.painter.grey(&format!(" · {} {word}", project.notes)));
            }
            self.emit("\n ", None, &title, &suffix);
        }
    }

    pub fn display_by_board(&mut self, groups: &Groups) {
        let today = self.today();
        let now_millis = self.now_millis();
        for (board, items) in groups {
            if self.is_group_complete(items) && !self.config.display_complete_tasks {
                continue;
            }
            self.display_title(board, items, &today);
            for item in items {
                if item.is_task && item.is_complete.unwrap_or(false) && !self.config.display_complete_tasks {
                    continue;
                }
                self.display_item_by_board(item, now_millis);
            }
        }
    }

    pub fn display_by_date(&mut self, groups: &Groups) {
        let today = self.today();
        for (date, items) in groups {
            if self.is_group_complete(items) && !self.config.display_complete_tasks {
                continue;
            }
            self.display_title(date, items, &today);
            for item in items {
                if item.is_task && item.is_complete.unwrap_or(false) && !self.config.display_complete_tasks {
                    continue;
                }
                self.display_item_by_date(item);
            }
        }
    }

    pub fn display_stats(&mut self, stats: &Stats) {
        if !self.config.display_progress_overview {
            return;
        }

        let percent_text = format!("{}%", stats.percent);
        let percent = if stats.percent >= 75 {
            self.painter.green(&percent_text)
        } else if stats.percent >= 50 {
            self.painter.yellow(&percent_text)
        } else {
            percent_text
        };

        let mut status = vec![
            format!("{} {}", self.painter.green(&stats.complete.to_string()), self.painter.grey("done")),
            format!("{} {}", self.painter.blue(&stats.in_progress.to_string()), self.painter.grey("in-progress")),
        ];
        if stats.paused > 0 {
            status.push(format!(
                "{} {}",
                self.painter.yellow(&stats.paused.to_string()),
                self.painter.grey("paused")
            ));
        }
        if stats.cancelled > 0 {
            status.push(format!(
                "{} {}",
                self.painter.grey(&stats.cancelled.to_string()),
                self.painter.grey("cancelled")
            ));
        }
        status.extend([
            format!("{} {}", self.painter.magenta(&stats.pending.to_string()), self.painter.grey("pending")),
            format!(
                "{} {}",
                self.painter.blue(&stats.notes.to_string()),
                self.painter.grey(if stats.notes == 1 { "note" } else { "notes" })
            ),
        ]);
        // At the end, and only when they exist: these count what is NOT in
        // front of you, so they read as a footnote to the line rather than
        // as part of it. A board that stashes nothing prints exactly what it
        // always did.
        if stats.stashed > 0 {
            status.push(format!(
                "{} {}",
                self.painter.grey(&stats.stashed.to_string()),
                self.painter.grey("in-stash")
            ));
        }
        if stats.trashed > 0 {
            status.push(format!(
                "{} {}",
                self.painter.grey(&stats.trashed.to_string()),
                self.painter.grey("in-trash")
            ));
        }

        let total =
            stats.pending + stats.in_progress + stats.paused + stats.cancelled + stats.complete + stats.notes;
        if total == 0 {
            self.emit("\n ", None, "Type `ekko --help` to get started", "");
        }

        let complete_line = self.painter.grey(&format!("{percent} of all tasks complete."));
        self.emit("\n ", None, &complete_line, "");
        let joined = status.join(&self.painter.grey(" \u{b7} "));
        self.emit(" ", None, &joined, "\n");

        // A nudge, not an error. More than one thing in progress is how the
        // mark stops meaning "where I am" and starts meaning "things I once
        // started" -- and it accumulates from decisions that each looked
        // reasonable at the time, so it is worth surfacing on the day it
        // happens rather than on the day someone comes back.
        //
        // Only ever printed when the situation exists, so a board that keeps
        // to one cursor prints exactly what it always did.
        if stats.in_progress > 1 {
            let warning = self.painter.yellow(&format!(
                "{} tasks in progress -- pause the ones you are not on: ekko --set @id paused",
                stats.in_progress
            ));
            self.emit(" ", None, &warning, "\n");
        }
    }

    // ---- public: messages ----------------------------------------------

    pub fn invalid_custom_app_dir(&mut self, path: &str) {
        let shown = if path.trim().is_empty() { "\"\"".to_string() } else { path.to_string() };
        let suffix = self.painter.red(&shown);
        self.error("\n", "Custom app directory was not found on your system:", &suffix);
    }

    pub fn missing_ekko_dir_flag_value(&mut self) {
        self.error("\n ", "Please provide a value for --ekko-dir or remove the flag.", "");
    }

    pub fn invalid_id(&mut self, id: &str) {
        let suffix = self.painter.grey(id);
        self.error("\n", "Unable to find item with id:", &suffix);
    }

    pub fn invalid_ids_number(&mut self) {
        self.error("\n", "More than one ids were given as input", "");
    }

    pub fn invalid_priority(&mut self) {
        self.error("\n", "Priority can only be 1, 2 or 3", "");
    }

    pub fn lock_timeout(&mut self, path: &str) {
        let suffix = self.painter.red(path);
        self.error(
            "\n",
            "Timed out waiting for the ekko storage lock. If no other ekko process is running, delete this file and try again:",
            &suffix,
        );
    }

    pub fn missing_boards(&mut self) {
        self.error("\n", "No boards were given as input", "");
    }

    pub fn missing_desc(&mut self) {
        self.error("\n", "No description was given as input", "");
    }

    pub fn missing_id(&mut self) {
        self.error("\n", "No id was given as input", "");
    }

    /// Fallback for the wrapped-IO/JSON error variants in `EkkoError`,
    /// which are unexpected enough (disk full, permission denied, a
    /// corrupt JSON file) that a bespoke message per case isn't worth it.
    pub fn generic_error(&mut self, message: &str) {
        self.error("\n", message, "");
    }

    pub fn mark_complete(&mut self, ids: &[u32]) {
        self.mark("Checked", "tasks", "task", ids);
    }

    pub fn mark_incomplete(&mut self, ids: &[u32]) {
        self.mark("Unchecked", "tasks", "task", ids);
    }

    pub fn mark_started(&mut self, ids: &[u32]) {
        self.mark("Started", "tasks", "task", ids);
    }

    pub fn mark_paused(&mut self, ids: &[u32]) {
        self.mark("Paused", "tasks", "task", ids);
    }

    pub fn mark_cancelled(&mut self, ids: &[u32]) {
        self.mark("Cancelled", "tasks", "task", ids);
    }

    /// `unstarted` clears progress, pause and cancellation at once, so no
    /// single past participle names it. "Reset" describes what the caller
    /// asked for, which is most often undoing a `--set` aimed at a wrong id.
    pub fn mark_reset(&mut self, ids: &[u32]) {
        self.mark("Reset", "tasks", "task", ids);
    }

    pub fn mark_starred(&mut self, ids: &[u32]) {
        self.mark("Starred", "items", "item", ids);
    }

    pub fn mark_unstarred(&mut self, ids: &[u32]) {
        self.mark("Unstarred", "items", "item", ids);
    }

    pub fn success_create(&mut self, item: &Item) {
        let kind = if item.is_task { "task:" } else { "note:" };
        let message = format!("Created {kind}");
        let suffix = self.painter.grey(&item.id.to_string());
        self.success("\n", &message, &suffix);
    }

    /// Says what went, then where it went.
    ///
    /// The count is the confirmation Ekko never asks for out loud, and the
    /// path is the way back -- both on screen because a destroyed project
    /// is the one thing `--restore` cannot reach.
    pub fn success_destroy(
        &mut self,
        name: &str,
        tasks: u32,
        notes: u32,
        trash: &std::path::Path,
    ) {
        let mut held = format!("{tasks} {}", if tasks == 1 { "task" } else { "tasks" });
        if notes > 0 {
            held.push_str(&format!(" · {notes} {}", if notes == 1 { "note" } else { "notes" }));
        }
        let suffix = format!("{} {}", self.painter.grey(name), self.painter.grey(&format!("({held})")));
        self.success("\n", "Destroyed project:", &suffix);
        self.emit(" ", None, &self.painter.grey(&format!("moved to {}", trash.display())), "");
    }

    pub fn success_edit(&mut self, id: u32) {
        let suffix = self.painter.grey(&id.to_string());
        self.success("\n", "Updated description of item:", &suffix);
    }

    pub fn success_delete(&mut self, ids: &[u32]) {
        let word = if ids.len() > 1 { "items" } else { "item" };
        let message = format!("Deleted {word}:");
        let suffix = self.painter.grey(&join_ids(ids));
        self.success("\n", &message, &suffix);
    }

    pub fn success_move(&mut self, id: u32, boards: &[String]) {
        let message = format!("Move item: {} to", self.painter.grey(&id.to_string()));
        let suffix = self.painter.grey(&boards.join(", "));
        self.success("\n", &message, &suffix);
    }

    pub fn success_priority(&mut self, id: u32, level: u8) {
        let label = match level {
            PRIORITY_HIGH => self.painter.red("high"),
            PRIORITY_MEDIUM => self.painter.yellow("medium"),
            _ => self.painter.green("normal"),
        };
        let message = format!("Updated priority of task: {} to", self.painter.grey(&id.to_string()));
        self.success("\n", &message, &label);
    }

    pub fn success_restore(&mut self, ids: &[u32]) {
        let word = if ids.len() > 1 { "items" } else { "item" };
        let message = format!("Restored {word}:");
        let suffix = self.painter.grey(&join_ids(ids));
        self.success("\n", &message, &suffix);
    }

    pub fn success_copy_to_clipboard(&mut self, ids: &[u32]) {
        let phrase = if ids.len() > 1 { "descriptions of items" } else { "description of item" };
        let message = format!("Copied the {phrase}:");
        let suffix = self.painter.grey(&join_ids(ids));
        self.success("\n", &message, &suffix);
    }

    // ---- shared plumbing for the mark_*/success_*/error_* families ----

    fn mark(&mut self, verb: &str, plural: &str, singular: &str, ids: &[u32]) {
        if ids.is_empty() {
            return;
        }
        let word = if ids.len() > 1 { plural } else { singular };
        let message = format!("{verb} {word}:");
        let suffix = self.painter.grey(&join_ids(ids));
        self.success("\n", &message, &suffix);
    }

    fn success(&mut self, prefix: &str, message: &str, suffix: &str) {
        self.emit(prefix, Some(&Level::Success), message, suffix);
    }

    fn error(&mut self, prefix: &str, message: &str, suffix: &str) {
        self.emit(prefix, Some(&Level::Error), message, suffix);
    }
}

fn join_ids(ids: &[u32]) -> String {
    ids.iter().map(u32::to_string).collect::<Vec<_>>().join(", ")
}


/// Columns of the controlling terminal, or `None` when stdout is not one.
///
/// `ioctl(TIOCGWINSZ)` rather than a crate: `libc` is already a dependency
/// for the storage lock, and this is the whole of what a terminal-size
/// crate would do. A terminal that reports zero columns is treated as
/// unknown rather than as a very narrow screen.
fn terminal_width() -> Option<usize> {
    use std::os::fd::AsRawFd;

    if !std::io::stdout().is_terminal() {
        return None;
    }

    // SAFETY: `winsize` is plain data, and the fd is stdout's, alive for the
    // duration of the call.
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    let result =
        unsafe { libc::ioctl(std::io::stdout().as_raw_fd(), libc::TIOCGWINSZ, &mut size) };

    (result == 0 && size.ws_col > 0).then_some(size.ws_col as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::fs;
    use std::path::Path;

    /// The date the golden `.ans` files were captured on. Their
    /// timeline and archive headers print it literally and tag it
    /// `[Today]`, so every render under test is pinned to this instant --
    /// without that these tests would only have passed on the capture day.
    const GOLDEN_DAY: &str = "Mon Aug 24 2026";

    fn golden_now() -> DateTime<Local> {
        Local
            .with_ymd_and_hms(2026, 8, 24, 12, 0, 0)
            .single()
            .expect("2026-08-24 12:00 local is a real, unambiguous instant")
    }

    fn render_with<F: FnOnce(&mut Renderer)>(config: Config, f: F) -> String {
        let mut buffer: Vec<u8> = Vec::new();
        {
            let mut renderer =
                Renderer::at(Painter::forced(true), config, &mut buffer, golden_now());
            f(&mut renderer);
        }
        String::from_utf8(buffer).unwrap()
    }

    fn golden(name: &str) -> String {
        // Repo-relative, resolved at compile time -- these reference files
        // are committed under tests/golden/, not pulled from anywhere
        // ephemeral, so `cargo test` works on a fresh clone and in CI.
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden").join(name);
        fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
    }

    /// Reconstructs the exact same items, in the exact same order, as the
    /// script that captured the golden `.ans` references from the real JS
    /// build -- ids and timestamps included, since both are echoed in the
    /// output.
    fn golden_items() -> Vec<Item> {
        #[allow(clippy::too_many_arguments)] // test-only fixture builder; a flat arg list reads fine here
        fn item(
            id: u32,
            description: &str,
            boards: &[&str],
            is_task: bool,
            is_complete: bool,
            in_progress: bool,
            priority: u8,
            is_starred: bool,
        ) -> Item {
            let mut item = if is_task {
                Item::new_task(id, description.to_string(), boards.iter().map(|s| s.to_string()).collect(), priority)
            } else {
                Item::new_note(id, description.to_string(), boards.iter().map(|s| s.to_string()).collect())
            };
            item.date = GOLDEN_DAY.to_string();
            // Same instant the renderer is pinned to, so the age suffix is
            // always 0d -- exactly as it was in the golden capture.
            item.timestamp = golden_now().timestamp_millis();
            if is_task {
                item.is_complete = Some(is_complete);
                item.in_progress = Some(in_progress);
            }
            item.is_starred = is_starred;
            item
        }

        vec![
            item(1, "Normal priority task", &["@coding"], true, true, false, 1, false),
            item(2, "Medium priority task", &["@coding"], true, false, true, 2, false),
            item(3, "High priority task", &["@coding"], true, false, false, 3, true),
            item(5, "Another board entirely", &["@writing"], true, false, false, 1, false),
        ]
    }

    fn golden_groups_by_board() -> Vec<(String, Vec<Item>)> {
        let items = golden_items();
        vec![
            ("@coding".to_string(), items[0..3].to_vec()),
            ("@writing".to_string(), items[3..4].to_vec()),
        ]
    }

    // Matches the golden `.ans` files' own stats line: 1 done, 1
    // in-progress, 2 pending, 0 notes -> 25% (1 of 4 tasks complete).
    fn golden_stats() -> Stats {
        Stats { percent: 25, complete: 1, in_progress: 1, paused: 0, cancelled: 0, pending: 2, notes: 0, stashed: 0, trashed: 0 }
    }

    #[test]
    fn board_view_matches_the_real_js_output_byte_for_byte() {
        // `tb` with no flags -- what actually produced board.ans -- runs
        // displayByBoard() *and* displayStats() in sequence, so the test
        // has to reproduce both to compare against the whole file.
        let output = render_with(Config::default(), |r| {
            r.display_by_board(&golden_groups_by_board());
            r.display_stats(&golden_stats());
        });

        assert_eq!(output, golden("board.ans"));
    }

    #[test]
    fn timeline_view_matches_the_real_js_output_byte_for_byte() {
        let date_groups = vec![(GOLDEN_DAY.to_string(), golden_items())];

        let output = render_with(Config::default(), |r| {
            r.display_by_date(&date_groups);
            r.display_stats(&golden_stats());
        });

        assert_eq!(output, golden("timeline.ans"));
    }

    #[test]
    fn error_message_matches_the_real_js_output_byte_for_byte() {
        let output = render_with(Config::default(), |r| {
            r.invalid_id("999999");
        });

        assert_eq!(output, golden("error.ans"));
    }

    #[test]
    fn success_create_message_matches_the_real_js_output_byte_for_byte() {
        let output = render_with(Config::default(), |r| {
            let item = Item::new_task(6, "created ok".to_string(), vec!["@coding".to_string()], 1);
            r.success_create(&item);
        });

        assert_eq!(output, golden("success-msg.ans"));
    }

    #[test]
    fn archive_view_with_a_note_matches_the_real_js_output_byte_for_byte() {
        // `--archive` calls displayByDate() only, no displayStats() --
        // unlike the default board view and --timeline.
        let mut note = Item::new_note(1, "A reference note".to_string(), vec!["@coding".to_string()]);
        note.date = GOLDEN_DAY.to_string();
        note.timestamp = golden_now().timestamp_millis();
        let date_groups = vec![(GOLDEN_DAY.to_string(), vec![note])];

        let output = render_with(Config::default(), |r| {
            r.display_by_date(&date_groups);
        });

        assert_eq!(output, golden("archive.ans"));
    }

    /// Renders with folding on at a fixed width, and without colour, so the
    /// assertions are about the text rather than the escape codes.
    fn render_folded_at<F: FnOnce(&mut Renderer)>(width: usize, f: F) -> String {
        let mut buffer: Vec<u8> = Vec::new();
        {
            let mut renderer =
                Renderer::at(Painter::forced(false), Config::default(), &mut buffer, golden_now());
            renderer.fold_width = Some(width);
            f(&mut renderer);
        }
        String::from_utf8(buffer).unwrap()
    }

    fn long_note(text: &str) -> Item {
        let mut note = Item::new_note(1, text.to_string(), vec!["@b".to_string()]);
        note.date = GOLDEN_DAY.to_string();
        note.timestamp = golden_now().timestamp_millis();
        note
    }

    #[test]
    fn a_long_note_folds_to_one_line_and_says_how_much_it_hid() {
        let text = "x".repeat(300);
        let output = render_folded_at(100, |r| {
            r.display_by_board(&[("@b".to_string(), vec![long_note(&text)])]);
        });

        let body = output.lines().find(|l| l.contains('x')).expect("the note should render");
        assert!(body.chars().count() <= 100, "folded line still overflows: {}", body.chars().count());
        assert!(body.contains("(+"), "no hidden-line count: {body}");
        assert!(!body.contains(&text), "the full text leaked through");
    }

    #[test]
    fn a_note_that_already_fits_is_left_exactly_alone() {
        let output = render_folded_at(100, |r| {
            r.display_by_board(&[("@b".to_string(), vec![long_note("short enough")])]);
        });

        assert!(output.contains("short enough"));
        assert!(!output.contains("(+"), "nothing was hidden, so nothing should be claimed");
    }

    #[test]
    fn tasks_are_never_folded_however_long_they_get() {
        // A truncated task hides something you are meant to act on.
        let text = "y".repeat(300);
        let mut task = Item::new_task(1, text.clone(), vec!["@b".to_string()], 1);
        task.date = GOLDEN_DAY.to_string();
        task.timestamp = golden_now().timestamp_millis();

        let output = render_folded_at(100, |r| {
            r.display_by_board(&[("@b".to_string(), vec![task])]);
        });

        assert!(output.contains(&text), "a task must survive whole");
    }

    #[test]
    fn folding_off_leaves_everything_whole() {
        // This is what keeps every golden test passing: the renderer only
        // folds when something told it a width, and nothing does in tests or
        // when stdout is a pipe.
        let text = "z".repeat(300);
        let output = render_with(Config::default(), |r| {
            r.display_by_board(&[("@b".to_string(), vec![long_note(&text)])]);
        });

        assert!(output.contains(&text));
    }

    #[test]
    fn a_terminal_too_narrow_to_fold_usefully_does_not_try() {
        let text = "w".repeat(300);
        let output = render_folded_at(20, |r| {
            r.display_by_board(&[("@b".to_string(), vec![long_note(&text)])]);
        });

        assert!(output.contains(&text), "below MIN_FOLD_WIDTH the marker would eat the line");
    }

    #[test]
    fn the_stats_line_is_unchanged_when_nothing_is_paused() {
        // The compatibility promise: boards that never pause anything print
        // exactly what they always did, goldens included.
        let output = render_with(Config::default(), |r| {
            r.display_stats(&Stats { percent: 25, complete: 1, in_progress: 1, paused: 0, cancelled: 0, pending: 2, notes: 0, stashed: 0, trashed: 0 });
        });

        assert!(!output.contains("paused"), "a zero count must not appear:\n{output}");
    }

    #[test]
    fn the_stats_line_reports_paused_once_there_is_any() {
        let output = render_with(Config::default(), |r| {
            r.display_stats(&Stats { percent: 0, complete: 0, in_progress: 1, paused: 2, cancelled: 0, pending: 1, notes: 0, stashed: 0, trashed: 0 });
        });

        assert!(output.contains("2") && output.contains("paused"), "{output}");
    }

    #[test]
    fn more_than_one_in_progress_earns_a_warning_and_one_does_not() {
        let one = render_with(Config::default(), |r| {
            r.display_stats(&Stats { percent: 0, complete: 0, in_progress: 1, paused: 0, cancelled: 0, pending: 0, notes: 0, stashed: 0, trashed: 0 });
        });
        let several = render_with(Config::default(), |r| {
            r.display_stats(&Stats { percent: 0, complete: 0, in_progress: 3, paused: 0, cancelled: 0, pending: 0, notes: 0, stashed: 0, trashed: 0 });
        });

        assert!(!one.contains("in progress --"), "a single cursor is the healthy case:\n{one}");
        assert!(several.contains("3 tasks in progress"), "{several}");
    }

    #[test]
    fn a_paused_task_renders_with_its_own_icon() {
        let mut item = Item::new_task(1, "set aside".to_string(), vec!["@b".to_string()], 1);
        item.date = GOLDEN_DAY.to_string();
        item.timestamp = golden_now().timestamp_millis();
        item.paused = Some(true);

        let output = render_with(Config::default(), |r| {
            r.display_by_board(&[("@b".to_string(), vec![item])]);
        });

        assert!(output.contains('\u{23f8}'), "expected the pause icon:\n{output}");
        assert!(!output.contains('\u{2610}'), "must not fall back to the empty box:\n{output}");
    }

    #[test]
    fn stats_are_hidden_when_config_disables_the_progress_overview() {
        let config = Config { display_progress_overview: false, ..Config::default() };

        let output = render_with(config, |r| {
            r.display_stats(&Stats { percent: 50, complete: 1, in_progress: 1, paused: 0, cancelled: 0, pending: 1, notes: 0, stashed: 0, trashed: 0 });
        });

        assert_eq!(output, "");
    }

    #[test]
    fn complete_tasks_are_hidden_when_config_disables_them() {
        let config = Config { display_complete_tasks: false, ..Config::default() };

        let output = render_with(config, |r| {
            r.display_by_board(&golden_groups_by_board());
        });

        assert!(!output.contains("Normal priority task"));
        assert!(output.contains("Medium priority task"));
    }

    /// The two-task `@ekko` board that `stats-complete.ans` and
    /// `stats-half.ans` were captured from.
    fn stats_board(second_complete: bool) -> Vec<(String, Vec<Item>)> {
        let items = [("One", true), ("Two", second_complete)]
            .iter()
            .enumerate()
            .map(|(index, (description, is_complete))| {
                let mut item = Item::new_task(
                    index as u32 + 1,
                    (*description).to_string(),
                    vec!["@ekko".to_string()],
                    1,
                );
                item.date = GOLDEN_DAY.to_string();
                item.timestamp = golden_now().timestamp_millis();
                item.is_complete = Some(*is_complete);
                item.in_progress = Some(false);
                item
            })
            .collect();

        vec![("@ekko".to_string(), items)]
    }

    // A coloured percentage is the one place the renderer nests one style
    // inside another, and chalk closes a nested style by re-opening the
    // enclosing one instead of resetting to the terminal default. Neither
    // board.ans nor timeline.ans exercises it -- both sit at 25%, where the
    // percentage is left uncoloured -- which is exactly how the port came to
    // emit a plain reset here and render the rest of the line in the default
    // colour rather than grey. These pin both coloured branches.
    #[test]
    fn fully_complete_stats_line_matches_the_real_js_output_byte_for_byte() {
        let output = render_with(Config::default(), |r| {
            r.display_by_board(&stats_board(true));
            r.display_stats(&Stats {
                percent: 100,
                complete: 2,
                in_progress: 0,
                paused: 0,
                cancelled: 0,
                pending: 0,
                notes: 0,
                stashed: 0,
                trashed: 0,
            });
        });

        assert_eq!(output, golden("stats-complete.ans"));
    }

    #[test]
    fn half_complete_stats_line_matches_the_real_js_output_byte_for_byte() {
        let output = render_with(Config::default(), |r| {
            r.display_by_board(&stats_board(false));
            r.display_stats(&Stats {
                percent: 50,
                complete: 1,
                in_progress: 0,
                paused: 0,
                cancelled: 0,
                pending: 1,
                notes: 0,
                stashed: 0,
                trashed: 0,
            });
        });

        assert_eq!(output, golden("stats-half.ans"));
    }

    /// The board view drew `⇠` from the day dependencies landed; the
    /// timeline never did, because `display_item_by_date` was not updated
    /// alongside `display_item_by_board`. One surface got the feature and
    /// its sibling did not, which is invisible until you happen to run the
    /// other view.
    #[test]
    fn the_timeline_draws_the_blocked_marker_too() {
        let mut blocked = Item::new_task(2, "ship it".to_string(), vec!["@coding".to_string()], 1);
        blocked.date = GOLDEN_DAY.to_string();
        blocked.timestamp = golden_now().timestamp_millis();
        let groups = vec![(GOLDEN_DAY.to_string(), vec![blocked])];

        let mut map = std::collections::HashMap::new();
        map.insert(2, vec![1]);

        let output = render_with(Config::default(), |r| {
            r.with_blockers(map);
            r.display_by_date(&groups);
        });

        assert!(output.contains('\u{21e0}'), "timeline dropped the marker: {output:?}");
        assert!(output.contains("ship it"), "timeline lost the description: {output:?}");
    }

    /// The path's three node glyphs carry the state, and the colour has to
    /// carry it with them. Painting only the name left every dot in the
    /// terminal default, so behind, here and ahead read as one weight.
    #[test]
    fn path_nodes_are_painted_with_their_phase() {
        let steps = vec![
            PathStep { name: "setup".into(), complete: 2, total: 2, notes: 0, current: false },
            PathStep { name: "build".into(), complete: 0, total: 3, notes: 0, current: true },
            PathStep { name: "ship".into(), complete: 0, total: 1, notes: 0, current: false },
        ];

        let output = render_with(Config::default(), |r| r.display_path(&steps, 0));

        // Each glyph inside its colour's span, not after the close.
        assert!(output.contains("\u{1b}[32msetup \u{25cf}"), "done node unpainted: {output:?}");
        assert!(output.contains("\u{1b}[34mbuild \u{25c9}"), "current node unpainted: {output:?}");
        assert!(output.contains("\u{1b}[90mship \u{25cb}"), "ahead node unpainted: {output:?}");
    }

    /// Column widths are counted on the plain text and the padding is
    /// appended outside the colour. Padding the *painted* string counted the
    /// escape sequences as width, so whenever a phase name was shorter than
    /// its own tally the node row silently got no padding at all and the two
    /// rows drifted apart -- only with colour on, which is to say only in a
    /// real terminal and never in a pipe.
    #[test]
    fn path_rows_line_up_when_the_tally_is_wider_than_the_name() {
        let steps = vec![
            PathStep { name: "ci".into(), complete: 0, total: 12, notes: 0, current: true },
            PathStep { name: "build".into(), complete: 0, total: 0, notes: 0, current: false },
        ];

        let output = render_with(Config::default(), |r| r.display_path(&steps, 0));

        let plain: String = strip_ansi(&output);
        let rows: Vec<&str> = plain.lines().filter(|l| !l.trim().is_empty()).collect();
        let nodes = rows[0];
        let counts = rows[1];

        // Columns, not byte offsets: `───` and `◉` are three bytes each, so
        // `str::find` would report the two rows as misaligned when they line
        // up perfectly on screen.
        fn column(haystack: &str, needle: &str) -> Option<usize> {
            haystack.find(needle).map(|b| haystack[..b].chars().count())
        }

        // "build" on the node row starts in the column "0/0" does below it.
        assert_eq!(
            column(nodes, "build"),
            column(counts, "0/0"),
            "node and tally rows drifted:\n{nodes}\n{counts}"
        );
    }

    fn strip_ansi(s: &str) -> String {
        let mut out = String::new();
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            if c == '\u{1b}' {
                for c in chars.by_ref() {
                    if c == 'm' {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    /// `--projects` promised item counts in `cli.rs` and printed bare names,
    /// so the one listing that could have said how big a project was said
    /// nothing -- and a project is the only thing in Ekko with no archive
    /// behind it. Notes appear only when there are any, the same rule the
    /// stats line uses for paused and cancelled.
    #[test]
    fn the_project_listing_says_what_each_one_holds() {
        let projects = vec![
            ProjectSummary { name: "plan".into(), complete: 0, tasks: 15, notes: 4 },
            ProjectSummary { name: "solo".into(), complete: 1, tasks: 2, notes: 0 },
            ProjectSummary { name: "empty".into(), complete: 0, tasks: 0, notes: 0 },
        ];

        let output = render_with(Config::default(), |r| r.display_projects(&projects));
        let plain = strip_ansi(&output);

        assert!(plain.contains("plan [0/15] · 4 notes"), "{plain:?}");
        assert!(plain.contains("solo [1/2]"), "{plain:?}");
        assert!(!plain.contains("solo [1/2] ·"), "no note tail when there are none: {plain:?}");
        assert!(plain.contains("empty [0/0]"), "{plain:?}");
    }

    /// Phases lost their renderer when projects grew counts. They were only
    /// sharing one because two lists of names looked alike, and `--phases`
    /// still has to print exactly what it always printed.
    #[test]
    fn phases_still_print_as_bare_names() {
        let names = vec!["setup".to_string(), "build".to_string()];

        let output = render_with(Config::default(), |r| r.display_phases(&names));
        let plain = strip_ansi(&output);

        assert!(!plain.contains('['), "a phase list carries no counts: {plain:?}");
        assert_eq!(
            plain.lines().filter(|l| !l.trim().is_empty()).count(),
            2,
            "one line per phase: {plain:?}"
        );
    }

    /// The indent is the visible half of anchoring. A reason sitting at the
    /// same depth as the work reads as another item competing for
    /// attention, which is how a board of long notes became a wall; one
    /// step in and it reads as belonging to the line above.
    #[test]
    fn an_anchored_note_is_indented_and_an_ordinary_one_is_not() {
        let mut anchored =
            Item::new_note(2, "the reason".to_string(), vec!["@a".to_string()]);
        anchored.date = GOLDEN_DAY.to_string();
        anchored.timestamp = golden_now().timestamp_millis();
        anchored.anchor = Some("some-task-uid".to_string());

        let mut plain = Item::new_note(3, "unattached".to_string(), vec!["@a".to_string()]);
        plain.date = GOLDEN_DAY.to_string();
        plain.timestamp = golden_now().timestamp_millis();

        let groups = vec![("@a".to_string(), vec![anchored, plain])];
        let output = render_with(Config::default(), |r| r.display_by_board(&groups));
        let plain_text = strip_ansi(&output);

        let anchored_line =
            plain_text.lines().find(|l| l.contains("the reason")).expect("rendered");
        let plain_line =
            plain_text.lines().find(|l| l.contains("unattached")).expect("rendered");

        let depth = |line: &str| line.len() - line.trim_start().len();
        assert_eq!(
            depth(anchored_line),
            depth(plain_line) + 2,
            "anchored note not indented:\n{anchored_line}\n{plain_line}"
        );
    }

    /// August 2026 starts on a Saturday and has 31 days, so it needs six
    /// leading blanks and spills into a sixth week -- the two shapes most
    /// likely to be got wrong.
    #[test]
    fn a_month_is_padded_at_both_ends_to_whole_weeks() {
        let month = CalendarMonth::of(2026, 8, Some(28)).expect("August 2026 is real");

        assert_eq!(month.label, "August 2026");
        assert!(month.weeks.iter().all(|week| week.len() == 7), "every week is seven cells");
        assert_eq!(month.weeks[0], vec![None, None, None, None, None, None, Some(1)]);
        assert_eq!(month.weeks.last().unwrap()[2..], [None, None, None, None, None]);

        let days: Vec<u32> = month.weeks.iter().flatten().flatten().copied().collect();
        assert_eq!(days, (1..=31).collect::<Vec<u32>>(), "every day appears once, in order");
    }

    /// February in a leap year, the case a hand-rolled day count gets
    /// wrong. Derived from the next month's first day rather than a table,
    /// so the calendar cannot disagree with the calendar.
    #[test]
    fn february_lengths_come_from_the_dates_themselves() {
        let leap = CalendarMonth::of(2028, 2, None).expect("February 2028 is real");
        let plain = CalendarMonth::of(2026, 2, None).expect("February 2026 is real");

        let count = |m: &CalendarMonth| m.weeks.iter().flatten().flatten().count();

        assert_eq!(count(&leap), 29);
        assert_eq!(count(&plain), 28);
    }

    /// December has to look at the next January, not month 13.
    #[test]
    fn december_rolls_into_the_next_year_rather_than_failing() {
        let month = CalendarMonth::of(2026, 12, None).expect("December 2026 is real");

        assert_eq!(month.weeks.iter().flatten().flatten().count(), 31);
        assert_eq!(month.label, "December 2026");
    }

    #[test]
    fn an_impossible_month_is_none_rather_than_a_panic() {
        assert!(CalendarMonth::of(2026, 13, None).is_none());
        assert!(CalendarMonth::of(2026, 0, None).is_none());
    }

    /// With nothing from the board on it, today is the only thing the
    /// drawing can tell you -- so it is the one thing painted.
    #[test]
    fn today_is_the_only_painted_cell() {
        let month = CalendarMonth::of(2026, 8, Some(28)).expect("August 2026 is real");

        let output = render_with(Config::default(), |r| r.display_calendar(&month));

        assert!(output.contains("\u{1b}[33m28\u{1b}[39m"), "today not painted: {output:?}");
        assert!(!output.contains("\u{1b}[33m27"), "a plain day was painted: {output:?}");
    }
}
