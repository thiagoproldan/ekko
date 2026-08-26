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
use crate::render::ProjectSummary;
use crate::storage::ItemMap;

const EKKO_DIR_NAME: &str = ".ekko";

#[derive(Debug)]
pub enum DirectoryError {
    MissingEkkoDirFlagValue,
    MissingProjectName,
    InvalidProjectName(String),
    UnknownProject(String),
    ProjectAndEkkoDirTogether,
    DestroyNeedsProject,
    Trash(String),
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
            DirectoryError::DestroyNeedsProject => write!(
                f,
                "--destroy needs a project to destroy: ekko --project <name> --destroy"
            ),
            DirectoryError::Trash(detail) => {
                write!(f, "Could not move the project to the trash: {detail}")
            }
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

/// Where destroyed projects go, beside `projects/` rather than inside it.
///
/// Inside would put it in `list_projects`' way and force a filter, which
/// would make `.trash` a name nobody could give a project -- a reserved
/// word invented to work around a layout choice. Beside costs nothing and
/// mirrors how `archive/` already sits next to `storage/`.
pub const TRASH_DIR_NAME: &str = ".trash";

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
pub fn list_projects(home_dir: &Path) -> Vec<ProjectSummary> {
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
        .into_iter()
        .map(|name| {
            let storage = root.join(&name).join(EKKO_DIR_NAME).join("storage").join("storage.json");
            let (complete, tasks, notes) = count_items(&storage);
            ProjectSummary { name, complete, tasks, notes }
        })
        .collect()
}

/// Moves a project out of `projects/` and into the trash, returning where
/// it went.
///
/// A rename rather than a delete, and a rename rather than a copy: it is
/// atomic on one filesystem, so the project is either listed or trashed and
/// never half of both, and it cannot run out of disk partway through. The
/// caller holds the project's lock while this runs.
///
/// The timestamp is epoch millis -- the same clock `_timestamp` and
/// `updatedAt` already use -- so two projects of the same name can be
/// trashed without one replacing the other, and the directory sorts by
/// when it was removed.
pub fn destroy_project(home_dir: &Path, name: &str, now_millis: i64) -> Result<PathBuf, DirectoryError> {
    let root = home_dir.join(EKKO_DIR_NAME).join(PROJECTS_DIR_NAME);
    let dir = root.join(name);
    if !dir.is_dir() {
        return Err(DirectoryError::UnknownProject(name.to_string()));
    }

    let trash = home_dir.join(EKKO_DIR_NAME).join(TRASH_DIR_NAME);
    std::fs::create_dir_all(&trash)
        .map_err(|e| DirectoryError::Trash(format!("{}: {e}", trash.display())))?;

    let target = trash.join(format!("{name}-{now_millis}"));
    std::fs::rename(&dir, &target)
        .map_err(|e| DirectoryError::Trash(format!("{} -> {}: {e}", dir.display(), target.display())))?;

    Ok(target)
}

/// Reads one project's storage directly rather than through `Storage`,
/// which creates the directories it expects -- a side effect no listing
/// should have, least of all one that might run while a project is being
/// removed.
///
/// An absent file is an empty project and counts as such: `--create` makes
/// the directories and writes nothing until the first item. A file that
/// cannot be parsed also reads as empty here, because failing the whole
/// listing over one bad project would hide the good ones; opening that
/// project reports the real error.
fn count_items(storage_file: &Path) -> (u32, u32, u32) {
    let Ok(contents) = std::fs::read_to_string(storage_file) else {
        return (0, 0, 0);
    };
    let Ok(items) = serde_json::from_str::<ItemMap>(&contents) else {
        return (0, 0, 0);
    };

    let mut complete = 0;
    let mut tasks = 0;
    let mut notes = 0;
    for item in items.values() {
        if item.is_task {
            tasks += 1;
            if item.is_complete.unwrap_or(false) {
                complete += 1;
            }
        } else {
            notes += 1;
        }
    }
    (complete, tasks, notes)
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

        let listed = list_projects(&home);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "winwayland");
        assert!(retrieve_project_directory(&home, "winwayland", false).is_ok(), "now resolvable");

        fs::remove_dir_all(&home).ok();
    }

    /// The trash lives beside `projects/`, not inside it. Inside would make
    /// every destroyed project show up as a project until a filter hid it,
    /// and that filter would reserve `.trash` as a name nobody could use.
    #[test]
    fn a_destroyed_project_leaves_the_listing_and_lands_in_the_trash() {
        let home = temp_dir();
        retrieve_project_directory(&home, "doomed", true).unwrap();
        retrieve_project_directory(&home, "keeper", true).unwrap();

        let trashed = destroy_project(&home, "doomed", 1787600000000).unwrap();

        let names: Vec<String> = list_projects(&home).into_iter().map(|p| p.name).collect();
        assert_eq!(names, vec!["keeper".to_string()], "destroyed project still listed");
        assert!(trashed.is_dir(), "nothing arrived at {}", trashed.display());
        assert!(trashed.ends_with("doomed-1787600000000"), "{}", trashed.display());
        assert!(
            !trashed.starts_with(home.join(EKKO_DIR_NAME).join(PROJECTS_DIR_NAME)),
            "the trash must not sit inside projects/: {}",
            trashed.display()
        );

        fs::remove_dir_all(&home).ok();
    }

    /// The timestamp is what keeps two projects of the same name from
    /// replacing one another in the trash -- destroy, recreate, destroy
    /// again is an ordinary sequence and neither copy may be lost.
    #[test]
    fn destroying_the_same_name_twice_keeps_both_copies() {
        let home = temp_dir();

        retrieve_project_directory(&home, "again", true).unwrap();
        let first = destroy_project(&home, "again", 1787600000000).unwrap();
        retrieve_project_directory(&home, "again", true).unwrap();
        let second = destroy_project(&home, "again", 1787600009999).unwrap();

        assert_ne!(first, second);
        assert!(first.is_dir() && second.is_dir(), "one copy replaced the other");

        fs::remove_dir_all(&home).ok();
    }

    /// Destroying something that is not there is an error, not a quiet
    /// success -- the same reasoning as `--list` refusing unknown terms.
    #[test]
    fn destroying_an_unknown_project_is_an_error() {
        let home = temp_dir();

        let result = destroy_project(&home, "never-existed", 1787600000000);

        assert!(matches!(result, Err(DirectoryError::UnknownProject(_))));

        fs::remove_dir_all(&home).ok();
    }

    /// A project created and never written to has no storage.json at all,
    /// which is an empty project and must count as one rather than as an
    /// error or a gap in the listing.
    #[test]
    fn a_project_with_nothing_written_yet_counts_as_empty() {
        let home = temp_dir();
        retrieve_project_directory(&home, "fresh", true).unwrap();

        let listed = list_projects(&home);

        assert_eq!((listed[0].complete, listed[0].tasks, listed[0].notes), (0, 0, 0));

        fs::remove_dir_all(&home).ok();
    }

    /// The counts are the whole point of the listing: a project is the one
    /// thing here with no archive behind it, so what it holds has to be
    /// visible before acting on it. Tasks and notes are counted separately,
    /// matching what a board title means by `[1/2]`.
    #[test]
    fn listing_reports_what_each_project_holds() {
        let home = temp_dir();
        let ekko_dir = retrieve_project_directory(&home, "work", true).unwrap();
        let storage = ekko_dir.join("storage");
        fs::create_dir_all(&storage).unwrap();
        fs::write(
            storage.join("storage.json"),
            r#"{
              "1":{"_id":1,"_date":"Tue Aug 25 2026","_timestamp":1787600000000,"description":"done one","isStarred":false,"boards":["@a"],"_isTask":true,"isComplete":true,"inProgress":false,"priority":1},
              "2":{"_id":2,"_date":"Tue Aug 25 2026","_timestamp":1787600000000,"description":"open one","isStarred":false,"boards":["@a"],"_isTask":true,"isComplete":false,"inProgress":false,"priority":1},
              "3":{"_id":3,"_date":"Tue Aug 25 2026","_timestamp":1787600000000,"description":"a note","isStarred":false,"boards":["@a"],"_isTask":false}
            }"#,
        )
        .unwrap();

        let listed = list_projects(&home);

        assert_eq!((listed[0].complete, listed[0].tasks, listed[0].notes), (1, 2, 1));

        fs::remove_dir_all(&home).ok();
    }

    /// Listing must not be a write. `Storage::new` creates the directories
    /// it expects, so counting through it would have the listing recreate
    /// storage for every project it walks -- including one being removed.
    #[test]
    fn listing_creates_nothing() {
        let home = temp_dir();
        let ekko_dir = retrieve_project_directory(&home, "solo", true).unwrap();
        let storage = ekko_dir.join("storage");
        fs::remove_dir_all(&storage).ok();

        list_projects(&home);

        assert!(!storage.exists(), "listing recreated {}", storage.display());

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
