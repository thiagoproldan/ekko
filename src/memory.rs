//! The project's memory (task 885): one page on what the project is as a
//! whole, kept by the user as memory.md in the board's folder, and put in a
//! Claude Code session's context when it starts, clears or compacts.
//!
//! The board holds the work; the page holds the whole, which a session would
//! otherwise rebuild in tokens from the board and the code. It is a hook of
//! its own beside the prime's: Claude Code keeps at most 10,000 characters of
//! a hook's output (note 165), counted per hook and not per event -- two hooks
//! of 5,993 characters arrived whole while one of 10,103 was cut to a preview
//! (2026-09-28) -- and the prime with its handoff already uses up to 9,500 of
//! its own.

use std::io;
use std::path::{Path, PathBuf};

/// The page's name in the board's folder: `.ekko/memory.md` in a project.
pub const FILE: &str = "memory.md";

/// The most of the page a session is given, in characters. Every later call
/// in the session reads it again, so it stays one page: a longer one shows
/// its start, cut at a line, and a line saying how long it runs.
pub const BUDGET: usize = 6_000;

pub fn path(board_dir: &Path) -> PathBuf {
    board_dir.join(FILE)
}

/// What the SessionStart hook puts in context: the page for a session that
/// starts, clears or compacts, and nothing for one that resumes or forks,
/// which holds it in its transcript already. Nothing either when the board
/// has no page, so a board without one costs a session nothing.
pub fn hook(board_dir: &Path, board: &str, source: &str) -> String {
    if matches!(source, "resume" | "fork") {
        return String::new();
    }
    page(board_dir, board).unwrap_or_default()
}

/// The page under a line naming it, or `None` when there is none. A page
/// that cannot be read says so instead of passing for no page.
pub fn page(board_dir: &Path, board: &str) -> Option<String> {
    let file = path(board_dir);
    let head = format!(
        "ekko memory \u{b7} {board} \u{b7} {} \u{b7} the user keeps this page: a change to it is theirs to make or approve\n",
        file.display()
    );
    let bytes = match std::fs::read(&file) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return None,
        Err(error) => return Some(format!("{head}It could not be read: {error}\n")),
    };
    let text = String::from_utf8_lossy(&bytes);
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let length = text.chars().count();
    if length <= BUDGET {
        return Some(format!("{head}{text}\n"));
    }
    Some(format!(
        "{head}{}\n\u{2026} memory.md runs to {length} characters, past the {BUDGET} a session is given: the rest is left out, so the page wants shortening\n",
        cut_at_line(text, BUDGET)
    ))
}

/// The start of `text` within `budget` characters, ending at the last line
/// that fits whole; a first line longer than the budget is cut where the
/// budget ends.
fn cut_at_line(text: &str, budget: usize) -> &str {
    let end = text.char_indices().nth(budget).map_or(text.len(), |(at, _)| at);
    let head = &text[..end];
    match head.rfind('\n') {
        Some(at) if at > 0 => head[..at].trim_end(),
        _ => head,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn board_with(page: Option<&str>) -> PathBuf {
        let dir = crate::paths::test_dir("ekko-memory");
        fs::create_dir_all(&dir).unwrap();
        if let Some(page) = page {
            fs::write(path(&dir), page).unwrap();
        }
        dir
    }

    #[test]
    fn a_session_that_starts_clears_or_compacts_is_given_the_page() {
        let dir = board_with(Some("# what this project is\n\nA board for a person and an agent.\n"));
        for source in ["startup", "clear", "compact", ""] {
            let said = hook(&dir, "site", source);
            let head = said.lines().next().unwrap();
            assert!(head.starts_with("ekko memory \u{b7} site \u{b7} "), "{source}: {said}");
            assert!(head.contains(path(&dir).to_str().unwrap()), "{source}: {said}");
            assert!(said.ends_with("# what this project is\n\nA board for a person and an agent.\n"), "{source}: {said}");
        }
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_session_that_resumes_or_forks_is_given_nothing_it_holds_already() {
        let dir = board_with(Some("# what this project is\n"));
        for source in ["resume", "fork"] {
            assert_eq!(hook(&dir, "site", source), "", "{source}");
        }
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_board_without_a_page_costs_a_session_nothing() {
        let none = board_with(None);
        let blank = board_with(Some(" \n\n\t\n"));
        for dir in [&none, &blank] {
            assert_eq!(hook(dir, "site", "startup"), "");
            fs::remove_dir_all(dir).ok();
        }
    }

    #[test]
    fn a_page_past_the_budget_shows_its_start_cut_at_a_line_and_says_so() {
        let lines: Vec<String> = (0..120).map(|n| format!("{n:03} {}", "x".repeat(95))).collect();
        let dir = board_with(Some(&lines.join("\n")));
        let said = hook(&dir, "site", "startup");
        let body: Vec<&str> = said.lines().skip(1).collect();
        let (last, shown) = body.split_last().unwrap();
        assert!(last.contains("runs to 11999 characters, past the 6000"), "{last}");
        assert_eq!(shown.len(), BUDGET / 100, "whole lines only, within the budget: {}", shown.len());
        assert!(shown.iter().zip(&lines).all(|(shown, line)| shown == line), "a line was cut or changed");
        assert!(said.chars().count() < 10_000, "past what Claude Code keeps of a hook: {}", said.chars().count());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_first_line_longer_than_the_budget_is_cut_where_the_budget_ends() {
        assert_eq!(cut_at_line(&"é".repeat(10), 4), "éééé");
        assert_eq!(cut_at_line("abc\ndefgh", 6), "abc");
    }

    #[test]
    fn a_page_that_cannot_be_read_says_so() {
        let dir = board_with(None);
        fs::create_dir_all(path(&dir)).unwrap();
        let said = hook(&dir, "site", "startup");
        assert!(said.contains("could not be read"), "{said}");
        fs::remove_dir_all(&dir).ok();
    }
}
