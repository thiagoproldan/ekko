//! Small path utilities shared by `config` and `directory`.

use std::path::{Path, PathBuf};

/// Expands a leading `~` to `home_dir`, but only when it's the whole
/// string or immediately followed by a path separator -- `~` and `~/foo`
/// expand, `~foo` does not (that's someone's literal directory name, not a
/// home-relative path).
///
/// The JS version had two separate implementations of this that disagreed
/// on exactly this point: `config.js` stripped *any* leading `~` including
/// from `~foo`, while `directory.js`'s was already this stricter, correct
/// form. Unified here since there was never a reason for it to differ by
/// call site.
pub fn expand_tilde(home_dir: &Path, input: &str) -> PathBuf {
    match input.strip_prefix('~') {
        Some("") => home_dir.to_path_buf(),
        Some(rest) if rest.starts_with(['/', '\\']) => {
            home_dir.join(rest.trim_start_matches(['/', '\\']))
        }
        _ => PathBuf::from(input),
    }
}

/// Rust's `Path::join` replaces `self` outright when the argument is
/// absolute (correct and by design -- unlike Node's `path.join`, which
/// naively concatenates every segment regardless). `resolve_path` below
/// needs Node's `path.resolve` behavior instead: make `input` absolute
/// (relative to `cwd` if it isn't already), then lexically normalize `.`
/// and `..` components -- without touching the filesystem or resolving
/// symlinks.
pub fn resolve_path(home_dir: &Path, cwd: &Path, input: &str) -> PathBuf {
    let expanded = expand_tilde(home_dir, input);
    let absolute = if expanded.is_absolute() { expanded } else { cwd.join(expanded) };
    normalize_lexically(&absolute)
}

fn normalize_lexically(path: &Path) -> PathBuf {
    use std::path::Component;

    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                result.pop();
            }
            Component::CurDir => {}
            other => result.push(other.as_os_str()),
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_tilde_expands_to_home() {
        assert_eq!(expand_tilde(Path::new("/home/x"), "~"), Path::new("/home/x"));
    }

    #[test]
    fn tilde_slash_expands_relative_to_home() {
        assert_eq!(expand_tilde(Path::new("/home/x"), "~/work"), Path::new("/home/x/work"));
    }

    #[test]
    fn tilde_immediately_followed_by_a_name_does_not_expand() {
        assert_eq!(expand_tilde(Path::new("/home/x"), "~work"), Path::new("~work"));
    }

    #[test]
    fn no_tilde_is_unchanged() {
        assert_eq!(expand_tilde(Path::new("/home/x"), "/elsewhere"), Path::new("/elsewhere"));
    }

    #[test]
    fn resolve_path_normalizes_dot_dot_lexically() {
        let resolved = resolve_path(Path::new("/home/x"), Path::new("/cwd"), "/a/b/../c");
        assert_eq!(resolved, Path::new("/a/c"));
    }

    #[test]
    fn resolve_path_makes_a_relative_path_absolute_against_cwd() {
        let resolved = resolve_path(Path::new("/home/x"), Path::new("/cwd"), "sub/dir");
        assert_eq!(resolved, Path::new("/cwd/sub/dir"));
    }

    #[test]
    fn resolve_path_expands_tilde_before_resolving() {
        let resolved = resolve_path(Path::new("/home/x"), Path::new("/cwd"), "~/work");
        assert_eq!(resolved, Path::new("/home/x/work"));
    }
}
