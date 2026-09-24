//! The commits that name an item (task 396): a line `Ekko: 469` in a
//! commit's message -- a trailer, as git calls it -- written with the commit
//! by whoever makes it. Read from the project's git history each time an
//! item is read, and never stored: a rebase, a cherry-pick or a squash that
//! keeps the message keeps the line, so the link outlives the SHA a note
//! would have cited.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::{Command, Stdio};

use serde::Serialize;

/// The trailer's key, matched ignoring case, as git matches trailer keys.
pub const KEY: &str = "Ekko";

/// A commit that names an item.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Commit {
    /// Abbreviated, as `git log --oneline` prints it.
    pub sha: String,
    /// The committer's date, which a rebase or a cherry-pick moves.
    pub date: String,
    pub subject: String,
    /// Whether the branch checked out holds it, rather than only another.
    pub landed: bool,
}

/// What the `Ekko:` lines in `message` name: `Ekko: 469`, `Ekko: 125, 396`,
/// `Ekko: #469`, or a uid. A line naming anything else is no trailer but
/// prose that starts with the word, or a path such as `Ekko::new` -- this
/// repository's history holds one -- and names nothing.
pub fn named(message: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in message.lines() {
        let Some((key, value)) = line.split_once(':') else { continue };
        if !key.trim().eq_ignore_ascii_case(KEY) {
            continue;
        }
        let tokens: Vec<String> = value
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|token| !token.is_empty())
            .map(|token| token.trim_start_matches('#').to_lowercase())
            .collect();
        if !tokens.is_empty() && tokens.iter().all(|token| reference(token)) {
            names.extend(tokens);
        }
    }
    names
}

/// A display id, all digits, or a uid: two runs of hex digits and a dash.
fn reference(token: &str) -> bool {
    let hex = |run: &str| !run.is_empty() && run.chars().all(|c| c.is_ascii_hexdigit());
    (!token.is_empty() && token.chars().all(|c| c.is_ascii_digit())) || token.split_once('-').is_some_and(|(a, b)| hex(a) && hex(b))
}

/// The commits of the repository `folder` is in that name each of `items`,
/// given by display id and uid, newest first. Only commits made since
/// `since`, in epoch milliseconds, less a day for clocks that disagree: none
/// can name an item before it existed, and the bound keeps a long history
/// from being read whole. Nothing, quietly, where git or a repository is
/// missing.
pub fn naming(folder: &Path, since: i64, items: &[(u32, Option<&str>)]) -> HashMap<u32, Vec<Commit>> {
    let mut found: HashMap<u32, Vec<Commit>> = HashMap::new();
    if items.is_empty() {
        return found;
    }
    let day = chrono::DateTime::from_timestamp_millis(since - 86_400_000)
        .map_or_else(|| "1970-01-01".to_string(), |at| at.format("%Y-%m-%d").to_string());
    let since = format!("--since={day}");
    let grep = format!("--grep=^[[:space:]]*{KEY}:");
    let Some(log) = git(folder, &["log", "--all", &since, "-i", &grep, "--format=%H%x1f%h%x1f%cs%x1f%s%x1f%B%x1e"])
    else {
        return found;
    };
    let landed: HashSet<String> = git(folder, &["log", "HEAD", &since, "-i", &grep, "--format=%H"])
        .map(|out| out.lines().map(str::to_string).collect())
        .unwrap_or_default();
    for record in log.split('\x1e') {
        let fields: Vec<&str> = record.trim_start_matches('\n').splitn(5, '\x1f').collect();
        let [full, sha, date, subject, message] = fields[..] else { continue };
        let names = named(message);
        for (id, uid) in items {
            let id_text = id.to_string();
            if names.iter().any(|name| *name == id_text || uid.is_some_and(|uid| name == uid)) {
                found.entry(*id).or_default().push(Commit {
                    sha: sha.to_string(),
                    date: date.to_string(),
                    subject: subject.to_string(),
                    landed: landed.contains(full),
                });
            }
        }
    }
    found
}

/// Whether `folder` is in a git repository, where commits can name items:
/// it or a folder above holds `.git`, a folder or, in a worktree, a file.
pub fn in_repository(folder: &Path) -> bool {
    folder.ancestors().any(|dir| dir.join(".git").exists())
}

/// The branch checked out in `folder`'s repository, or `None` where there is
/// no repository or HEAD is detached.
pub fn branch(folder: &Path) -> Option<String> {
    let name = git(folder, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    let name = name.trim();
    (!name.is_empty() && name != "HEAD").then(|| name.to_string())
}

/// `git -C folder <args>`'s output, or `None` when it could not run or
/// failed. A signature check a user's config asks of `git log` would print
/// its own lines among the commits, so it is turned off.
fn git(folder: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(folder)
        .args(["-c", "log.showSignature=false"])
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_trailer_names_ids_and_uids_and_nothing_else() {
        let message = "fix: a thing (task 469)\n\nThe body mentions Ekko: in prose.\n\nEkko: 125, #396\nekko: 18d81a4d97e916db-ac6d5\nCo-Authored-By: someone <a@b>\n";
        let names = named(message);
        for expected in ["125", "396", "18d81a4d97e916db-ac6d5"] {
            assert!(names.contains(&expected.to_string()), "{expected} in {names:?}");
        }
        assert!(!names.contains(&"469".to_string()), "a subject's '(task 469)' is no trailer: {names:?}");
        assert!(named("Ekkos: 1\nNot Ekko: 2").is_empty(), "another key names nothing");
        for prose in ["Ekko::new takes 2 arguments", "Ekko: the board has 3 views", "Ekko:"] {
            assert!(named(prose).is_empty(), "{prose:?} named {:?}", named(prose));
        }
    }
}
