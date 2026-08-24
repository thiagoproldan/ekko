//! `~/.taskbook.json` -- the one piece of state that's never relocatable,
//! unlike the task/note data itself. Read fresh on every access rather
//! than cached, same as the JS version.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::json;
use crate::paths::expand_tilde;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    pub taskbook_directory: String,
    pub display_complete_tasks: bool,
    pub display_progress_overview: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            taskbook_directory: "~".to_string(),
            display_complete_tasks: true,
            display_progress_overview: true,
        }
    }
}

#[derive(Debug)]
pub enum ConfigError {
    Io(io::Error),
    Json(serde_json::Error),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Io(e) => write!(f, "{e}"),
            ConfigError::Json(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ConfigError {}

impl From<io::Error> for ConfigError {
    fn from(error: io::Error) -> Self {
        ConfigError::Io(error)
    }
}

impl From<serde_json::Error> for ConfigError {
    fn from(error: serde_json::Error) -> Self {
        ConfigError::Json(error)
    }
}

pub fn config_file_path(home_dir: &Path) -> PathBuf {
    home_dir.join(".taskbook.json")
}

/// Ensures the config file exists (writing the defaults if not), then
/// returns the effective config: any key missing from the file falls back
/// to its default (via `#[serde(default)]` on the whole struct), and a
/// `taskbookDirectory` starting with `~` comes back already expanded to an
/// absolute path under `home_dir`.
pub fn get(home_dir: &Path) -> Result<Config, ConfigError> {
    let path = config_file_path(home_dir);
    ensure_config_file(&path)?;

    let content = fs::read_to_string(&path)?;
    let mut config: Config = serde_json::from_str(&content)?;
    config.taskbook_directory =
        expand_tilde(home_dir, &config.taskbook_directory).to_string_lossy().into_owned();

    Ok(config)
}

fn ensure_config_file(path: &Path) -> Result<(), ConfigError> {
    if path.exists() {
        return Ok(());
    }

    fs::write(path, json::to_pretty_string(&Config::default())?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_home_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ekko-config-test-{}-{}",
            process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn creates_and_returns_defaults_when_no_file_exists() {
        let home = temp_home_dir();

        let config = get(&home).unwrap();

        assert!(config_file_path(&home).exists());
        assert_eq!(config.taskbook_directory, home.to_string_lossy());
        assert!(config.display_complete_tasks);
        assert!(config.display_progress_overview);

        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn a_fully_specified_file_is_read_back_as_is_aside_from_tilde_expansion() {
        let home = temp_home_dir();
        fs::write(
            config_file_path(&home),
            r#"{"taskbookDirectory":"~/work","displayCompleteTasks":false,"displayProgressOverview":false}"#,
        )
        .unwrap();

        let config = get(&home).unwrap();

        assert_eq!(config.taskbook_directory, home.join("work").to_string_lossy());
        assert!(!config.display_complete_tasks);
        assert!(!config.display_progress_overview);

        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn a_partial_file_falls_back_to_defaults_for_missing_keys() {
        let home = temp_home_dir();
        fs::write(config_file_path(&home), r#"{"displayCompleteTasks":false}"#).unwrap();

        let config = get(&home).unwrap();

        assert!(!config.display_complete_tasks); // from the file
        assert!(config.display_progress_overview); // default, key was absent
        assert_eq!(config.taskbook_directory, home.to_string_lossy()); // default "~", expanded

        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn a_non_tilde_taskbook_directory_is_left_exactly_as_written() {
        let home = temp_home_dir();
        fs::write(config_file_path(&home), r#"{"taskbookDirectory":"some/relative/path"}"#)
            .unwrap();

        let config = get(&home).unwrap();

        // Not expanded or resolved here -- that's `directory::resolve_path`'s
        // job, once this value is picked as a candidate.
        assert_eq!(config.taskbook_directory, "some/relative/path");

        fs::remove_dir_all(&home).ok();
    }
}
