//! Resolves which directory `storage.rs` should actually use, in priority
//! order: `--ekko-dir` flag > `EKKO_DIR` env var > the config file's
//! `ekkoDirectory` > `home_dir/.ekko`.
//!
//! `home_dir`/`cwd`/`flag`/`env_var` are all explicit parameters rather
//! than read from `std::env`/`std::env::home_dir()` internally -- partly to
//! match the rest of this crate's style (inject what a function needs
//! rather than reach for ambient state inside it), but mainly because it
//! makes every branch here deterministically testable without mutating
//! real process-global state. `cargo test` runs tests in parallel threads
//! of one process; env vars and cwd are exactly that kind of global state,
//! and later Rust editions mark mutating them `unsafe` for precisely this
//! reason.

use std::path::{Path, PathBuf};

use crate::config;
use crate::paths::resolve_path;

const EKKO_DIR_NAME: &str = ".ekko";

#[derive(Debug)]
pub enum DirectoryError {
    MissingEkkoDirFlagValue,
    MissingProjectName,
    InvalidProjectName(String),
    UnknownProject(String),
    ProjectAndEkkoDirTogether,
    InvalidCustomAppDir(String),
    Config(config::ConfigError),
}

impl std::fmt::Display for DirectoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DirectoryError::MissingEkkoDirFlagValue => {
                write!(f, "Please provide a value for --ekko-dir or remove the flag.")
            }
            DirectoryError::MissingProjectName => {
                write!(f, "Please provide a name for --project or remove the flag.")
            }
            DirectoryError::InvalidProjectName(name) => {
                write!(f, "A project name cannot contain a path separator: {name}")
            }
            DirectoryError::UnknownProject(name) => write!(
                f,
                "No such project: {name}. Create it with: ekko --project {name} --create"
            ),
            DirectoryError::ProjectAndEkkoDirTogether => write!(
                f,
                "--project and --ekko-dir both say where data lives; pass only one"
            ),
            DirectoryError::InvalidCustomAppDir(candidate) => write!(
                f,
                "Custom app directory was not found on your system: {candidate}"
            ),
            DirectoryError::Config(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for DirectoryError {}

impl From<config::ConfigError> for DirectoryError {
    fn from(error: config::ConfigError) -> Self {
        DirectoryError::Config(error)
    }
}


/// Where named projects live, under the ekko directory.
pub const PROJECTS_DIR_NAME: &str = "projects";

/// Resolves a project name to its directory, creating it only when asked.
///
/// The filesystem is the registry: there is no list of projects to keep in
/// step with what exists, because the directories *are* the list. A name
/// that does not resolve is an error rather than a fresh empty project --
/// the same reasoning as `--list` refusing terms it does not recognise,
/// since a typo that silently succeeds is the worst kind of success.
pub fn retrieve_project_directory(
    home_dir: &Path,
    name: &str,
    create: bool,
) -> Result<PathBuf, DirectoryError> {
    if !is_present(name) {
        return Err(DirectoryError::MissingProjectName);
    }
    if name.contains('/') || name.contains(std::path::MAIN_SEPARATOR) || name == ".." {
        return Err(DirectoryError::InvalidProjectName(name.to_string()));
    }

    let root = home_dir.join(EKKO_DIR_NAME).join(PROJECTS_DIR_NAME);
    let dir = root.join(name);

    if dir.is_dir() {
        return Ok(dir.join(EKKO_DIR_NAME));
    }
    if !create {
        return Err(DirectoryError::UnknownProject(name.to_string()));
    }

    std::fs::create_dir_all(&dir)
        .map_err(|e| DirectoryError::InvalidCustomAppDir(format!("{}: {e}", dir.display())))?;
    Ok(dir.join(EKKO_DIR_NAME))
}

/// Every project that exists, in name order.
pub fn list_projects(home_dir: &Path) -> Vec<String> {
    let root = home_dir.join(EKKO_DIR_NAME).join(PROJECTS_DIR_NAME);
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Vec::new();
    };

    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    names
}

pub fn retrieve_ekko_directory(
    home_dir: &Path,
    cwd: &Path,
    flag: Option<&str>,
    env_var: Option<&str>,
) -> Result<PathBuf, DirectoryError> {
    if let Some(custom) = resolve_custom_ekko_directory(home_dir, cwd, flag, env_var)? {
        return Ok(custom);
    }

    Ok(home_dir.join(EKKO_DIR_NAME))
}

fn resolve_custom_ekko_directory(
    home_dir: &Path,
    cwd: &Path,
    flag: Option<&str>,
    env_var: Option<&str>,
) -> Result<Option<PathBuf>, DirectoryError> {
    let Some(candidate) = select_custom_directory_candidate(home_dir, flag, env_var)? else {
        return Ok(None);
    };

    let resolved = resolve_path(home_dir, cwd, &candidate);

    if resolved.file_name().and_then(|n| n.to_str()) == Some(EKKO_DIR_NAME) {
        // The candidate already names the ekko dir itself (e.g.
        // `--ekko-dir ~/work/.ekko`) -- use it directly, only its parent
        // needs to exist.
        let parent = resolved.parent().unwrap_or(&resolved);
        assert_directory_exists(parent, &candidate)?;
        return Ok(Some(resolved));
    }

    // Otherwise the candidate names the *parent* the ekko dir should live
    // under, and that parent must already exist.
    assert_directory_exists(&resolved, &candidate)?;
    Ok(Some(resolved.join(EKKO_DIR_NAME)))
}

fn select_custom_directory_candidate(
    home_dir: &Path,
    flag: Option<&str>,
    env_var: Option<&str>,
) -> Result<Option<String>, DirectoryError> {
    if let Some(value) = flag {
        return if is_present(value) {
            Ok(Some(value.to_string()))
        } else {
            Err(DirectoryError::MissingEkkoDirFlagValue)
        };
    }

    if let Some(value) = env_var {
        if is_present(value) {
            return Ok(Some(value.to_string()));
        }
    }

    let config = config::get(home_dir)?;
    if is_present(&config.ekko_directory) {
        return Ok(Some(config.ekko_directory));
    }

    Ok(None)
}

fn is_present(value: &str) -> bool {
    !value.trim().is_empty()
}

fn assert_directory_exists(dir: &Path, display: &str) -> Result<(), DirectoryError> {
    if dir.is_dir() {
        return Ok(());
    }

    let shown = if is_present(display) { display.to_string() } else { "\"\"".to_string() };
    Err(DirectoryError::InvalidCustomAppDir(shown))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ekko-directory-test-{}-{}",
            process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn an_unknown_project_is_an_error_carrying_the_way_to_create_it() {
        // Creating on first use would turn a typo into a new empty project,
        // which is the `--list` failure in another costume.
        let home = temp_dir();

        let result = retrieve_project_directory(&home, "nope", false);

        let Err(DirectoryError::UnknownProject(name)) = result else { panic!("{result:?}") };
        assert_eq!(name, "nope");
        assert!(
            DirectoryError::UnknownProject("nope".into()).to_string().contains("--create"),
            "the message should say how to fix it"
        );

        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn creating_a_project_puts_it_where_listing_will_find_it() {
        // The filesystem is the registry, so these two have to agree by
        // construction rather than by being kept in step.
        let home = temp_dir();

        retrieve_project_directory(&home, "winwayland", true).unwrap();

        assert_eq!(list_projects(&home), vec!["winwayland".to_string()]);
        assert!(retrieve_project_directory(&home, "winwayland", false).is_ok(), "now resolvable");

        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn a_project_name_cannot_escape_the_projects_directory() {
        let home = temp_dir();

        for name in ["../elsewhere", "a/b"] {
            let result = retrieve_project_directory(&home, name, true);
            assert!(
                matches!(result, Err(DirectoryError::InvalidProjectName(_))),
                "{name} should be rejected, got {result:?}"
            );
        }

        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn projects_are_isolated_from_each_other_and_from_the_default_board() {
        let home = temp_dir();
        let one = retrieve_project_directory(&home, "one", true).unwrap();
        let two = retrieve_project_directory(&home, "two", true).unwrap();
        let default = retrieve_ekko_directory(&home, &home, None, None).unwrap();

        assert_ne!(one, two);
        assert_ne!(one, default);
        assert_ne!(two, default);

        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn defaults_to_home_dot_ekko_with_nothing_configured() {
        let home = temp_dir();
        let cwd = temp_dir();

        let resolved = retrieve_ekko_directory(&home, &cwd, None, None).unwrap();

        assert_eq!(resolved, home.join(".ekko"));

        fs::remove_dir_all(&home).ok();
        fs::remove_dir_all(&cwd).ok();
    }

    #[test]
    fn flag_wins_over_everything_and_composes_dot_ekko_under_it() {
        let home = temp_dir();
        let cwd = temp_dir();
        let custom = temp_dir();

        let resolved =
            retrieve_ekko_directory(&home, &cwd, Some(custom.to_str().unwrap()), None).unwrap();

        assert_eq!(resolved, custom.join(".ekko"));

        fs::remove_dir_all(&home).ok();
        fs::remove_dir_all(&cwd).ok();
        fs::remove_dir_all(&custom).ok();
    }

    #[test]
    fn flag_naming_the_ekko_dir_itself_is_used_as_is() {
        let home = temp_dir();
        let cwd = temp_dir();
        let parent = temp_dir();
        let explicit = parent.join(".ekko");

        let resolved =
            retrieve_ekko_directory(&home, &cwd, Some(explicit.to_str().unwrap()), None).unwrap();

        assert_eq!(resolved, explicit);

        fs::remove_dir_all(&home).ok();
        fs::remove_dir_all(&cwd).ok();
        fs::remove_dir_all(&parent).ok();
    }

    #[test]
    fn empty_flag_value_is_an_error_not_a_fallthrough() {
        let home = temp_dir();
        let cwd = temp_dir();

        let result = retrieve_ekko_directory(&home, &cwd, Some("   "), None);

        assert!(matches!(result, Err(DirectoryError::MissingEkkoDirFlagValue)));

        fs::remove_dir_all(&home).ok();
        fs::remove_dir_all(&cwd).ok();
    }

    #[test]
    fn flag_pointing_at_a_directory_that_does_not_exist_is_an_error() {
        let home = temp_dir();
        let cwd = temp_dir();
        let nonexistent = home.join("nope-does-not-exist");

        let result = retrieve_ekko_directory(&home, &cwd, Some(nonexistent.to_str().unwrap()), None);

        assert!(matches!(result, Err(DirectoryError::InvalidCustomAppDir(_))));

        fs::remove_dir_all(&home).ok();
        fs::remove_dir_all(&cwd).ok();
    }

    #[test]
    fn env_var_is_used_when_no_flag_is_given() {
        let home = temp_dir();
        let cwd = temp_dir();
        let custom = temp_dir();

        let resolved =
            retrieve_ekko_directory(&home, &cwd, None, Some(custom.to_str().unwrap())).unwrap();

        assert_eq!(resolved, custom.join(".ekko"));

        fs::remove_dir_all(&home).ok();
        fs::remove_dir_all(&cwd).ok();
        fs::remove_dir_all(&custom).ok();
    }

    #[test]
    fn flag_takes_priority_over_env_var() {
        let home = temp_dir();
        let cwd = temp_dir();
        let from_flag = temp_dir();
        let from_env = temp_dir();

        let resolved = retrieve_ekko_directory(
            &home,
            &cwd,
            Some(from_flag.to_str().unwrap()),
            Some(from_env.to_str().unwrap()),
        )
        .unwrap();

        assert_eq!(resolved, from_flag.join(".ekko"));

        fs::remove_dir_all(&home).ok();
        fs::remove_dir_all(&cwd).ok();
        fs::remove_dir_all(&from_flag).ok();
        fs::remove_dir_all(&from_env).ok();
    }

    #[test]
    fn config_file_directory_is_used_when_no_flag_or_env_is_given() {
        let home = temp_dir();
        let cwd = temp_dir();
        let custom = temp_dir();
        fs::write(
            config::config_file_path(&home),
            format!(r#"{{"ekkoDirectory":"{}"}}"#, custom.to_str().unwrap().replace('\\', "\\\\")),
        )
        .unwrap();

        let resolved = retrieve_ekko_directory(&home, &cwd, None, None).unwrap();

        assert_eq!(resolved, custom.join(".ekko"));

        fs::remove_dir_all(&home).ok();
        fs::remove_dir_all(&cwd).ok();
        fs::remove_dir_all(&custom).ok();
    }

    #[test]
    fn relative_flag_value_resolves_against_cwd() {
        let home = temp_dir();
        let cwd = temp_dir();
        fs::create_dir_all(cwd.join("sub")).unwrap();

        let resolved = retrieve_ekko_directory(&home, &cwd, Some("sub"), None).unwrap();

        assert_eq!(resolved, cwd.join("sub").join(".ekko"));

        fs::remove_dir_all(&home).ok();
        fs::remove_dir_all(&cwd).ok();
    }
}
