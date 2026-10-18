//! `~/.ekko.json` -- the one piece of state that's never relocatable,
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
    pub ekko_directory: String,
    pub display_complete_tasks: bool,
    pub display_progress_overview: bool,
    /// The most sessions ekko agents starts at once for a project that set
    /// no limit of its own (task 1902): the user's default for this
    /// machine, set with `ekko agents limit N --default`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agents_limit: Option<u32>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            ekko_directory: "~".to_string(),
            display_complete_tasks: true,
            display_progress_overview: true,
            agents_limit: None,
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
    home_dir.join(".ekko.json")
}

/// Ensures the config file exists (writing the defaults if not), then
/// returns the effective config: any key missing from the file falls back
/// to its default (via `#[serde(default)]` on the whole struct), and an
/// `ekkoDirectory` starting with `~` comes back already expanded to an
/// absolute path under `home_dir`.
pub fn get(home_dir: &Path) -> Result<Config, ConfigError> {
    let path = config_file_path(home_dir);
    ensure_config_file(&path)?;

    let content = fs::read_to_string(&path)?;
    let mut config: Config = serde_json::from_str(&content)?;
    config.ekko_directory = expand_tilde(home_dir, &config.ekko_directory).to_string_lossy().into_owned();

    Ok(config)
}

/// Sets the machine's default limit on ekko agents' sessions at once (task
/// 1902), keeping every other key of the file as it is, those this version
/// does not know among them; written whole, then renamed over the old.
pub fn set_agents_limit(home_dir: &Path, limit: u32) -> Result<Config, ConfigError> {
    let path = config_file_path(home_dir);
    ensure_config_file(&path)?;
    let mut file: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path)?)?;
    let Some(keys) = file.as_object_mut() else {
        return Err(ConfigError::Io(io::Error::new(io::ErrorKind::InvalidData, format!("{} holds no JSON object", path.display()))));
    };
    keys.insert("agentsLimit".to_string(), serde_json::json!(limit));
    let temp = path.with_file_name(format!(".ekko.json.{}.tmp", std::process::id()));
    fs::write(&temp, json::to_pretty_string(&file)?)?;
    fs::rename(&temp, &path)?;
    get(home_dir)
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

    fn temp_home_dir() -> PathBuf {
        let dir = crate::paths::test_dir("ekko-config-test");
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn creates_and_returns_defaults_when_no_file_exists() {
        let home = temp_home_dir();

        let config = get(&home).unwrap();

        assert!(config_file_path(&home).exists());
        assert_eq!(config.ekko_directory, home.to_string_lossy());
        assert!(config.display_complete_tasks);
        assert!(config.display_progress_overview);

        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn a_fully_specified_file_is_read_back_as_is_aside_from_tilde_expansion() {
        let home = temp_home_dir();
        fs::write(
            config_file_path(&home),
            r#"{"ekkoDirectory":"~/work","displayCompleteTasks":false,"displayProgressOverview":false}"#,
        )
        .unwrap();

        let config = get(&home).unwrap();

        assert_eq!(config.ekko_directory, home.join("work").to_string_lossy());
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
        assert_eq!(config.ekko_directory, home.to_string_lossy()); // default "~", expanded

        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn the_default_agents_limit_is_set_keeping_the_rest_of_the_file() {
        let home = temp_home_dir();
        assert_eq!(get(&home).unwrap().agents_limit, None);
        fs::write(config_file_path(&home), r#"{"displayCompleteTasks":false,"later":{"kept":1}}"#).unwrap();

        let config = set_agents_limit(&home, 5).unwrap();

        assert_eq!((config.agents_limit, config.display_complete_tasks), (Some(5), false));
        let file: serde_json::Value = serde_json::from_str(&fs::read_to_string(config_file_path(&home)).unwrap()).unwrap();
        assert_eq!(file, serde_json::json!({"displayCompleteTasks": false, "later": {"kept": 1}, "agentsLimit": 5}));
        // The defaults written for a new file leave it out.
        assert!(!json::to_pretty_string(&Config::default()).unwrap().contains("agentsLimit"));

        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn a_non_tilde_ekko_directory_is_left_exactly_as_written() {
        let home = temp_home_dir();
        fs::write(config_file_path(&home), r#"{"ekkoDirectory":"some/relative/path"}"#).unwrap();

        let config = get(&home).unwrap();

        // Not expanded or resolved here -- that's `directory::resolve_path`'s
        // job, once this value is picked as a candidate.
        assert_eq!(config.ekko_directory, "some/relative/path");

        fs::remove_dir_all(&home).ok();
    }
}
