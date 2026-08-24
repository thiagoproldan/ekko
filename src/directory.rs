//! Resolves which directory `storage.rs` should actually use, in priority
//! order: `--ekko-dir` flag > `EKKO_DIR` env var > the config file's
//! `ekkoDirectory` > `home_dir/.ekko`.
//!
//! `home_dir`/`cwd`/`flag`/`env_var` are all explicit parameters rather
//! than read from `std::env`/`home::home_dir()` internally -- partly to
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
    InvalidCustomAppDir(String),
    Config(config::ConfigError),
}

impl std::fmt::Display for DirectoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DirectoryError::MissingEkkoDirFlagValue => {
                write!(f, "Please provide a value for --ekko-dir or remove the flag.")
            }
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
