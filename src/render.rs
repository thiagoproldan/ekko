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
        if self.enabled {
            format!("\x1b[{open}m{text}\x1b[{close}m")
        } else {
            text.to_string()
        }
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub percent: u32,
    pub complete: u32,
    pub in_progress: u32,
    pub pending: u32,
    pub notes: u32,
}

/// Board or date groupings, in the order they should display -- callers
/// (ekko.rs) own how that order is produced; this module just walks it.
pub type Groups = [(String, Vec<Item>)];

enum Level {
    Success, // complete task
    Pending, // pending task
    Wait,    // in-progress task
    Note,
    Error,
}

impl Level {
    fn icon(&self) -> &'static str {
        match self {
            Level::Success => "\u{2714}", // ✔
            Level::Pending => "\u{2610}", // ☐
            Level::Wait => "\u{2026}",    // …
            Level::Note => "\u{25cf}",    // ●
            Level::Error => "\u{2716}",   // ✖
        }
    }

    fn paint(&self, painter: &Painter, text: &str) -> String {
        match self {
            Level::Success => painter.green(text),
            Level::Pending => painter.magenta(text),
            Level::Wait => painter.blue(text),
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
}

impl<'a> Renderer<'a> {
    pub fn new(painter: Painter, config: Config, out: &'a mut dyn Write) -> Self {
        Self::at(painter, config, out, Local::now())
    }

    /// `new` with the clock pinned. The golden `.ans` references embed the
    /// date they were captured on (the timeline and archive headers print
    /// it, and add `[Today]` when it matches), so the tests that diff
    /// against them have to render as of that same instant -- otherwise
    /// they would pass only on the day of capture.
    fn at(painter: Painter, config: Config, out: &'a mut dyn Write, now: DateTime<Local>) -> Self {
        Renderer { painter, config, out, now }
    }

    fn today(&self) -> String {
        self.now.format("%a %b %d %Y").to_string()
    }

    fn now_millis(&self) -> i64 {
        self.now.timestamp_millis()
    }

    // ---- layout building blocks -----------------------------------

    fn item_level(&self, item: &Item) -> Level {
        if !item.is_task {
            return Level::Note;
        }
        if item.is_complete.unwrap_or(false) {
            Level::Success
        } else if item.in_progress.unwrap_or(false) {
            Level::Wait
        } else {
            Level::Pending
        }
    }

    fn build_prefix(&self, item: &Item) -> String {
        let id = item.id.to_string();
        let padding = " ".repeat(4usize.saturating_sub(id.len()));
        format!("{padding} {}", self.painter.grey(&format!("{id}.")))
    }

    fn build_message(&self, item: &Item) -> String {
        let is_complete = item.is_complete.unwrap_or(false);
        let priority = item.priority.unwrap_or(0);

        let mut parts = Vec::new();

        if !is_complete && priority > PRIORITY_NORMAL {
            let colored = match priority {
                PRIORITY_MEDIUM => self.painter.yellow(&item.description),
                _ => self.painter.red(&item.description),
            };
            parts.push(self.painter.underline(&colored));
        } else if is_complete {
            parts.push(self.painter.grey(&item.description));
        } else {
            parts.push(item.description.clone());
        }

        if !is_complete && priority > PRIORITY_NORMAL {
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
        let suffix = if age.is_empty() { star } else { format!("{age} {star}") };
        let prefix = self.build_prefix(item);
        let message = self.build_message(item);
        self.emit(&prefix, Some(&level), &message, &suffix);
    }

    fn display_item_by_date(&mut self, item: &Item) {
        let level = self.item_level(item);
        let boards: Vec<String> =
            item.boards.iter().filter(|b| b.as_str() != "My Board").cloned().collect();
        let suffix = format!("{} {}", self.color_boards(&boards), self.get_star(item));
        let prefix = self.build_prefix(item);
        let message = self.build_message(item);
        self.emit(&prefix, Some(&level), &message, &suffix);
    }

    // ---- public: views ------------------------------------------------

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

        let status = [
            format!("{} {}", self.painter.green(&stats.complete.to_string()), self.painter.grey("done")),
            format!("{} {}", self.painter.blue(&stats.in_progress.to_string()), self.painter.grey("in-progress")),
            format!("{} {}", self.painter.magenta(&stats.pending.to_string()), self.painter.grey("pending")),
            format!(
                "{} {}",
                self.painter.blue(&stats.notes.to_string()),
                self.painter.grey(if stats.notes == 1 { "note" } else { "notes" })
            ),
        ];

        let total = stats.pending + stats.in_progress + stats.complete + stats.notes;
        if total == 0 {
            self.emit("\n ", None, "Type `ekko --help` to get started", "");
        }

        let complete_line = self.painter.grey(&format!("{percent} of all tasks complete."));
        self.emit("\n ", None, &complete_line, "");
        let joined = status.join(&self.painter.grey(" \u{b7} "));
        self.emit(" ", None, &joined, "\n");
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
        Stats { percent: 25, complete: 1, in_progress: 1, pending: 2, notes: 0 }
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

    #[test]
    fn stats_are_hidden_when_config_disables_the_progress_overview() {
        let config = Config { display_progress_overview: false, ..Config::default() };

        let output = render_with(config, |r| {
            r.display_stats(&Stats { percent: 50, complete: 1, in_progress: 1, pending: 1, notes: 0 });
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
}
