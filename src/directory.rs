//! Resolves which directory `storage.rs` should actually use, in priority
//! order: `--ekko-dir` flag > `--project` or `EKKO_PROJECT` > `EKKO_DIR` env
//! var > the project found from the current folder > the config file's
//! `ekkoDirectory` > `home_dir/.ekko`. Projects themselves live in
//! `project.rs`.
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
use crate::project::{self, Project};

pub(crate) const EKKO_DIR_NAME: &str = ".ekko";

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
    /// `ekko init` aimed at home, whose `.ekko/` is the default board.
    InitInHome,
    /// `ekko init` in a folder inside another project, with no repository
    /// boundary between them.
    NestedProject { name: String, root: PathBuf },
    NameTaken { name: String, path: PathBuf },
    /// A registered project whose folder no longer holds it.
    ProjectMoved { name: String, path: PathBuf },
    /// A legacy project and the folder `ekko init` adopts it into both hold
    /// a board.
    AdoptConflict { name: String, legacy: PathBuf, root: PathBuf },
    /// `ekko init --name` on a folder that is already a project by another name.
    AlreadyProject { name: String, root: PathBuf },
    NotAFolder(String),
    Io(String),
}

impl From<std::io::Error> for DirectoryError {
    fn from(error: std::io::Error) -> Self {
        DirectoryError::Io(error.to_string())
    }
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
                "No such project: {name}. A project lives in its folder: run ekko init there, and ekko --projects lists the ones that exist"
            ),
            DirectoryError::DestroyNeedsProject => write!(
                f,
                "--destroy needs a project: run it inside the project's folder, or name one with --project <name>"
            ),
            DirectoryError::InitInHome => write!(
                f,
                "Home already holds the default board at ~/.ekko; run ekko init in a project's folder or repository instead"
            ),
            DirectoryError::NestedProject { name, root } => write!(
                f,
                "This folder is inside the project {name} at {}; a project inside another needs a repository of its own",
                root.display()
            ),
            DirectoryError::NameTaken { name, path } => write!(
                f,
                "The name {name} is taken by the project at {}; choose another with --name",
                path.display()
            ),
            DirectoryError::ProjectMoved { name, path } => write!(
                f,
                "The project {name} was registered at {}, which no longer holds it; run ekko init in the folder it lives in now",
                path.display()
            ),
            DirectoryError::AdoptConflict { name, legacy, root } => write!(
                f,
                "Both {} and {} hold a board for {name}, and ekko will not merge two boards; move one of them away and run ekko init again",
                legacy.display(),
                root.join(EKKO_DIR_NAME).display()
            ),
            DirectoryError::AlreadyProject { name, root } => write!(
                f,
                "{} is already the project {name}; renaming a project is not supported",
                root.display()
            ),
            DirectoryError::NotAFolder(path) => write!(f, "No such folder: {path}"),
            DirectoryError::Io(message) => write!(f, "{message}"),
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


/// Where projects lived before they moved into their folders:
/// `~/.ekko/projects/<name>/.ekko/`. Read only to adopt what is still there.
pub const PROJECTS_DIR_NAME: &str = "projects";

/// Where destroyed projects go, and where an adopted legacy project's old
/// copy is parked: `~/.ekko/.trash/<name>-<millis>`.
///
/// Beside `projects/` rather than inside it, which would have put it in the
/// listing's way and made `.trash` a name nobody could give a project.
pub const TRASH_DIR_NAME: &str = ".trash";

/// Where each project's board is copied at every write, outside its folder:
/// `~/.ekko/copies/<project id>/`, holding the board's files as its `.ekko/`
/// holds them, without the history (task 503).
pub const COPIES_DIR_NAME: &str = "copies";

/// Where one invocation's board lives, and the project it belongs to.
#[derive(Debug)]
pub struct Location {
    pub dir: PathBuf,
    pub project: Option<Project>,
    /// Found from the folder, rather than named with `--project` or
    /// `EKKO_PROJECT`.
    pub discovered: bool,
    /// Where every write copies the board, when the board is a project's.
    pub copy: Option<PathBuf>,
    /// On the default board, the project registered for this folder when the
    /// folder no longer holds its board.
    pub lost: Option<project::Lost>,
}

impl Location {
    fn new(home_dir: &Path, dir: PathBuf, project: Option<Project>, discovered: bool) -> Self {
        let copy = project::copy_dir(home_dir, &dir);
        Location { dir, project, discovered, copy, lost: None }
    }

    /// How the prime, the sessions and the hooks name this board. On the
    /// default board in a folder whose project's board is gone, it says so
    /// and how to bring it back: nothing else would, and the session's
    /// writes would land on the default board meanwhile.
    pub fn label(&self) -> String {
        match (&self.project, self.discovered, &self.lost) {
            (Some(project), true, _) => format!("project {}, found from this folder", project.name),
            (Some(project), false, _) => format!("project {}", project.name),
            (None, _, Some(lost)) => format!("default board -- {}", lost.warning()),
            (None, _, None) => "default board".to_string(),
        }
    }
}

/// Resolves the board for one invocation.
///
/// Precedence: `--ekko-dir` > `--project` or `EKKO_PROJECT` > `EKKO_DIR` > the
/// project found from `cwd` > the config file's `ekkoDirectory` > `~/.ekko`.
/// Whatever the command line or the environment names beats what the folder
/// implies, and what the folder implies beats the default -- the order git's
/// own `--git-dir`, `GIT_DIR` and discovery follow. `--ekko-dir` together
/// with a project is an error rather than a silent winner: both say where
/// data lives, and guessing which was meant is how you write to the wrong
/// board.
pub fn locate(
    home_dir: &Path,
    cwd: &Path,
    flag: Option<&str>,
    env_var: Option<&str>,
    project_name: Option<&str>,
) -> Result<Location, DirectoryError> {
    let plain = |dir: PathBuf| Location::new(home_dir, dir, None, false);

    if flag.is_some() {
        if project_name.is_some() {
            return Err(DirectoryError::ProjectAndEkkoDirTogether);
        }
        return Ok(plain(retrieve_ekko_directory(home_dir, cwd, flag, None)?));
    }
    if let Some(name) = project_name {
        let found = project::resolve_named(home_dir, name)?;
        return Ok(Location::new(home_dir, found.dir.clone(), Some(found), false));
    }
    if env_var.is_some_and(is_present) {
        return Ok(plain(retrieve_ekko_directory(home_dir, cwd, None, env_var)?));
    }
    if let Some(found) = project::discover(home_dir, cwd) {
        return Ok(Location::new(home_dir, found.dir.clone(), Some(found), true));
    }
    let mut location = plain(retrieve_ekko_directory(home_dir, cwd, None, None)?);
    location.lost = project::lost_at(home_dir, cwd);
    Ok(location)
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

    fn temp_dir() -> PathBuf {
        let dir = crate::paths::test_dir("ekko-directory-test");
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Whatever the command line or the environment names beats the folder,
    /// and the folder beats the default -- the order git follows too -- and a
    /// directory together with a project is refused rather than guessed.
    #[test]
    fn a_named_board_beats_the_folder_and_the_folder_beats_the_default() {
        let home = temp_dir();
        let folder = home.join("work").join("site");
        fs::create_dir_all(&folder).unwrap();
        project::init(&home, &folder, None, None, 0).unwrap();
        let inside = fs::canonicalize(&folder).unwrap().join("sub");
        fs::create_dir_all(&inside).unwrap();

        let found = locate(&home, &inside, None, None, None).unwrap();
        assert!(found.discovered);
        assert_eq!(found.project.as_ref().map(|p| p.name.as_str()), Some("site"));

        let named = locate(&home, &home, None, None, Some("site")).unwrap();
        assert!(!named.discovered);
        assert_eq!(named.dir, found.dir);

        let custom = temp_dir();
        let by_env = locate(&home, &inside, None, Some(custom.to_str().unwrap()), None).unwrap();
        assert_eq!((by_env.dir, by_env.project), (custom.join(".ekko"), None));

        let outside = temp_dir();
        let default = locate(&home, &outside, None, None, None).unwrap();
        assert_eq!((default.dir, default.project), (home.join(".ekko"), None));

        assert!(matches!(
            locate(&home, &inside, Some(custom.to_str().unwrap()), None, Some("site")),
            Err(DirectoryError::ProjectAndEkkoDirTogether)
        ));

        for dir in [&home, &custom, &outside] {
            fs::remove_dir_all(dir).ok();
        }
    }

    /// A project's board is copied at every write, and when its folder loses
    /// it, the default board in that folder says so, and how to bring it
    /// back, in the label every prime starts with (task 503).
    #[test]
    fn a_projects_board_is_copied_and_its_loss_named_in_the_label() {
        let home = fs::canonicalize(temp_dir()).unwrap();
        let folder = home.join("work").join("site");
        fs::create_dir_all(&folder).unwrap();
        let id = project::init(&home, &folder, None, None, 0).unwrap().id;
        let found = locate(&home, &folder, None, None, None).unwrap();
        assert_eq!(found.copy, Some(home.join(".ekko").join(COPIES_DIR_NAME).join(&id)));
        assert_eq!(found.label(), "project site, found from this folder");
        crate::ekko::Ekko::at(&found).unwrap().storage.set(&crate::storage::ItemMap::new()).unwrap();

        fs::remove_dir_all(folder.join(EKKO_DIR_NAME)).unwrap();
        let fallen = locate(&home, &folder, None, None, None).unwrap();
        assert_eq!((fallen.dir.clone(), fallen.copy.clone()), (home.join(".ekko"), None));
        let label = fallen.label();
        assert!(label.starts_with("default board -- "), "{label}");
        assert!(label.contains(&format!("{} was project site, whose board is gone: ekko init", folder.display())), "{label}");

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
