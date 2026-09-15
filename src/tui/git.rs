//! The branch the status bar names, read the way git itself would find it.

use std::path::{Path, PathBuf};

/// The branch checked out where `dir` is, or the commit's first seven
/// characters when HEAD is detached; `None` outside a repository.
///
/// Read from the files rather than by running git: the status bar is drawn
/// many times a second, and a process per frame for one word is not a trade
/// worth making. A linked worktree's `.git` is a file naming its own git
/// directory, whose HEAD is that worktree's branch -- not the main checkout's.
pub fn branch(dir: &Path) -> Option<String> {
    let head = dir.ancestors().find_map(head_file)?;
    let text = std::fs::read_to_string(head).ok()?;
    let text = text.trim();
    Some(match text.strip_prefix("ref: ") {
        Some(reference) => reference.strip_prefix("refs/heads/").unwrap_or(reference).to_string(),
        None => text.chars().take(7).collect(),
    })
}

fn head_file(dir: &Path) -> Option<PathBuf> {
    let git = dir.join(".git");
    if git.is_dir() {
        return Some(git.join("HEAD"));
    }
    let pointer = std::fs::read_to_string(&git).ok()?;
    let gitdir = pointer.lines().find_map(|line| line.strip_prefix("gitdir:"))?.trim();
    let gitdir = if Path::new(gitdir).is_absolute() { PathBuf::from(gitdir) } else { dir.join(gitdir) };
    Some(gitdir.join("HEAD"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("ekko-git-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn names_the_branch_from_anywhere_inside_the_checkout() {
        let repo = folder("branch");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::create_dir_all(repo.join("src/deep")).unwrap();

        assert_eq!(branch(&repo).as_deref(), Some("main"));
        assert_eq!(branch(&repo.join("src/deep")).as_deref(), Some("main"));
        std::fs::remove_dir_all(&repo).ok();
    }

    /// A linked worktree has a branch of its own, and that is the one to name.
    #[test]
    fn a_linked_worktree_names_its_own_branch() {
        let main = folder("main");
        let gitdir = main.join(".git/worktrees/feature");
        std::fs::create_dir_all(&gitdir).unwrap();
        std::fs::write(main.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(gitdir.join("HEAD"), "ref: refs/heads/feature/tui\n").unwrap();
        let worktree = folder("worktree");
        std::fs::write(worktree.join(".git"), format!("gitdir: {}\n", gitdir.display())).unwrap();

        assert_eq!(branch(&worktree).as_deref(), Some("feature/tui"));
        std::fs::remove_dir_all(&main).ok();
        std::fs::remove_dir_all(&worktree).ok();
    }

    #[test]
    fn a_detached_head_is_its_short_commit_and_no_repository_is_nothing() {
        let repo = folder("detached");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::write(repo.join(".git/HEAD"), "342cbd4a1b2c3d4e5f60718293a4b5c6d7e8f901\n").unwrap();
        assert_eq!(branch(&repo).as_deref(), Some("342cbd4"));
        std::fs::remove_dir_all(&repo).ok();

        let plain = folder("plain");
        assert!(branch(&plain).is_none() || plain.ancestors().skip(1).any(|dir| dir.join(".git").exists()));
        std::fs::remove_dir_all(&plain).ok();
    }
}
