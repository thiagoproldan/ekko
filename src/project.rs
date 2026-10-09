//! Projects: a board that belongs to a folder.
//!
//! A project lives in the folder it is about, at `<root>/.ekko/`, and is found
//! from inside that folder the way git finds a repository; the default board
//! stays at `~/.ekko/`. Note 92 on the board decided this for repositories on
//! 2026-08-26 -- `~/.ekko/projects/` goes away, and a board leaves with the
//! checkout that holds it -- and it was widened to any folder when `ekko init`
//! arrived.
//!
//! Three files carry it:
//!
//! - `<root>/.ekko/project.json`, the marker: the project's name and id. It is
//!   what makes a `.ekko/` a project. The default board has none, so walking up
//!   from anywhere under home can never mistake `~/.ekko/` for a project.
//! - `~/.ekko/projects.json`, the registry: every project `ekko init` made, and
//!   its folder -- what `--project <name>` and `--projects` read from anywhere.
//!   The filesystem stopped being the registry when projects stopped living in
//!   one directory.
//! - `~/.ekko/projects.lock`, which serialises writes to the registry.
//!
//! Only `ekko init` and `--destroy` write the registry. Reading never does,
//! the rule every view in Ekko keeps: a project whose folder moved is reported
//! as moved, and `ekko init` in its new folder records where it went.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::directory::{DirectoryError, COPIES_DIR_NAME, EKKO_DIR_NAME, PROJECTS_DIR_NAME, TRASH_DIR_NAME};
use crate::render::ProjectSummary;
use crate::storage::{ItemMap, Storage};

pub(crate) const MARKER: &str = "project.json";
const REGISTRY: &str = "projects.json";
const REGISTRY_LOCK: &str = "projects.lock";

/// What a project's own folder says about it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    pub name: String,
    /// Never changes, so a project moved to another folder is recognised as
    /// the same project rather than as a second one with a familiar name.
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Registered {
    name: String,
    id: String,
    path: PathBuf,
    /// What a later version keeps here that this one does not know, written
    /// back as read; see `Item::unknown`.
    #[serde(flatten)]
    unknown: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Registry {
    projects: Vec<Registered>,
    /// The pairs of projects the user linked (task 811), by id, which a
    /// rename or a move keeps: a session on either board reaches the other's
    /// through ekko's MCP. A pair links both ways. Absent when there are
    /// none, so a registry without links is written as before.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    links: Vec<[String; 2]>,
    /// What a later version keeps here that this one does not know, written
    /// back as read; see `Item::unknown`.
    #[serde(flatten)]
    unknown: BTreeMap<String, serde_json::Value>,
}

/// A project, resolved to where its board lives.
#[derive(Debug, Clone, PartialEq)]
pub struct Project {
    pub name: String,
    /// The folder it belongs to -- `None` for a legacy project still under
    /// `~/.ekko/projects/`, waiting for `ekko init` to adopt it.
    pub root: Option<PathBuf>,
    /// The `.ekko/` directory holding its board.
    pub dir: PathBuf,
    id: Option<String>,
}

/// What `ekko init` did.
#[derive(Debug, Clone, PartialEq)]
pub struct Initialized {
    pub name: String,
    pub id: String,
    pub root: PathBuf,
    /// The folder was already this project: init only recorded where it is.
    pub existing: bool,
    /// The folder already held a board with no marker -- what `--ekko-dir
    /// <folder>` leaves behind -- and init made it the project in place.
    pub claimed: bool,
    pub adopted: Option<Adopted>,
    /// The folder had no board, and init brought back the one of a project
    /// whose board was gone, from its copy.
    pub restored: Option<Restored>,
    /// Whether `.ekko/` is kept out of git through `.git/info/exclude`.
    pub excluded: bool,
}

/// A legacy project moved into its folder: where it lived, and where its
/// old copy was parked.
#[derive(Debug, Clone, PartialEq)]
pub struct Adopted {
    pub from: PathBuf,
    pub parked: PathBuf,
}

/// A project's board brought back from its copy: where the project was
/// registered, and when the copy was last written.
#[derive(Debug, Clone, PartialEq)]
pub struct Restored {
    pub from: PathBuf,
    pub copied: Option<String>,
}

/// A registered project whose folder no longer holds its board -- removed,
/// cleaned with `git clean -fdx`, or moved without it -- and when its copy
/// was last written, if it has one (task 503).
#[derive(Debug, Clone, PartialEq)]
pub struct Lost {
    pub name: String,
    pub path: PathBuf,
    pub copied: Option<String>,
}

impl Lost {
    /// What a session on the default board in its folder is told.
    pub fn warning(&self) -> String {
        let path = self.path.display();
        match &self.copied {
            Some(when) => format!(
                "{path} was project {}, whose board is gone: ekko init {path} restores it from its copy of {when}",
                self.name
            ),
            None => format!(
                "{path} was project {}, whose board is gone and has no copy: ekko --project {} --destroy forgets it",
                self.name, self.name
            ),
        }
    }
}

/// What `--destroy` did to a project whose folder no longer holds its board:
/// forgot it, and moved its copy, if it had one, to the trash.
#[derive(Debug, Clone, PartialEq)]
pub struct Forgotten {
    pub name: String,
    pub path: PathBuf,
    pub parked: Option<PathBuf>,
}

fn io(error: impl std::fmt::Display) -> DirectoryError {
    DirectoryError::Io(error.to_string())
}

fn read_marker(root: &Path) -> Option<Marker> {
    marker_in(&root.join(EKKO_DIR_NAME))
}

fn marker_in(ekko_dir: &Path) -> Option<Marker> {
    let text = fs::read_to_string(ekko_dir.join(MARKER)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Where the board in `ekko_dir` is copied at every write, when it is a
/// project's: `~/.ekko/copies/<id>/`, by the project's id, which neither a
/// move nor a new folder changes.
pub fn copy_dir(home: &Path, ekko_dir: &Path) -> Option<PathBuf> {
    copies(home, &marker_in(ekko_dir)?.id)
}

fn copies(home: &Path, id: &str) -> Option<PathBuf> {
    // An id is a uid; anything that could name another directory is not one.
    let plain = !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    plain.then(|| home.join(EKKO_DIR_NAME).join(COPIES_DIR_NAME).join(id))
}

/// When the copy of project `id` was last written, as the listing and the
/// prime print it -- `None` when it has no copy to restore.
fn copied_at(home: &Path, id: &str) -> Option<String> {
    let modified = fs::metadata(copies(home, id)?.join("storage").join("storage.json")).ok()?.modified().ok()?;
    Some(when(modified))
}

fn when(time: SystemTime) -> String {
    chrono::DateTime::<chrono::Local>::from(time).format("%Y-%m-%d %H:%M").to_string()
}

/// Whether a registered project's folder no longer holds its board.
fn missing(entry: &Registered) -> bool {
    read_marker(&entry.path).is_none_or(|found| found.id != entry.id)
}

fn lost(home: &Path, entry: &Registered) -> Lost {
    Lost { name: entry.name.clone(), path: entry.path.clone(), copied: copied_at(home, &entry.id) }
}

/// The project registered for the folder `cwd` is in, when that folder no
/// longer holds its board: looked for at or above `cwd`, no further up than
/// the top of its repository, where `discover` would have found it.
pub fn lost_at(home: &Path, cwd: &Path) -> Option<Lost> {
    let registry = read_registry(home).ok()?;
    let registered = |dir: &Path| registry.projects.iter().find(|entry| entry.path == dir);
    let cwd = fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    for dir in cwd.ancestors() {
        if let Some(entry) = registered(dir) {
            return missing(entry).then(|| lost(home, entry));
        }
        if let Some(main) = repository_root(dir) {
            return registered(&main).filter(|entry| missing(entry)).map(|entry| lost(home, entry));
        }
    }
    None
}

fn project_at(root: &Path, marker: Marker) -> Project {
    Project {
        name: marker.name,
        root: Some(root.to_path_buf()),
        dir: root.join(EKKO_DIR_NAME),
        id: Some(marker.id),
    }
}

/// The checkout `dir` is the top of, when it is the top of a git repository:
/// `dir` itself, or -- for a linked worktree, whose `.git` is a file pointing
/// into the main repository -- the main checkout, so every worktree of one
/// repository shares one project. A submodule's `.git` file points somewhere
/// else, and a submodule is a repository of its own.
fn repository_root(dir: &Path) -> Option<PathBuf> {
    let git = dir.join(".git");
    if git.is_dir() {
        return Some(dir.to_path_buf());
    }
    let pointer = fs::read_to_string(&git).ok()?;
    let gitdir = pointer.lines().find_map(|line| line.strip_prefix("gitdir:"))?.trim();
    let gitdir = if Path::new(gitdir).is_absolute() { PathBuf::from(gitdir) } else { dir.join(gitdir) };
    let text = gitdir.to_string_lossy().into_owned();
    Some(match text.find("/.git/worktrees/") {
        Some(at) => PathBuf::from(&text[..at]),
        None => dir.to_path_buf(),
    })
}

/// The project `cwd` is in: the nearest folder at or above it holding a
/// marker, looking no further than the top of the repository `cwd` is in --
/// a repository is its own world, the way git's own discovery treats it. Home
/// itself is never a project, since `~/.ekko/` is the default board.
pub fn discover(home: &Path, cwd: &Path) -> Option<Project> {
    for dir in cwd.ancestors() {
        if dir != home {
            if let Some(marker) = read_marker(dir) {
                return Some(project_at(dir, marker));
            }
        }
        if let Some(main) = repository_root(dir) {
            if main == dir {
                return None;
            }
            return read_marker(&main).map(|marker| project_at(&main, marker));
        }
    }
    None
}

fn validate_name(name: &str) -> Result<(), DirectoryError> {
    if name.trim().is_empty() {
        return Err(DirectoryError::MissingProjectName);
    }
    if name.contains('/') || name.contains(std::path::MAIN_SEPARATOR) || name == "." || name == ".." {
        return Err(DirectoryError::InvalidProjectName(name.to_string()));
    }
    Ok(())
}

fn legacy_folder(home: &Path, name: &str) -> PathBuf {
    home.join(EKKO_DIR_NAME).join(PROJECTS_DIR_NAME).join(name)
}

/// The project called `name`, from anywhere: a registered one whose folder
/// still holds it, or -- until `ekko init` adopts it -- a legacy one still
/// under `~/.ekko/projects/`. An unknown name is an error, never a new empty
/// project: a typo that silently succeeds is the worst kind of success.
pub fn resolve_named(home: &Path, name: &str) -> Result<Project, DirectoryError> {
    validate_name(name)?;
    if let Some(entry) = read_registry(home)?.projects.into_iter().find(|entry| entry.name == name) {
        return match read_marker(&entry.path) {
            Some(marker) if marker.id == entry.id => Ok(project_at(&entry.path, marker)),
            _ => Err(DirectoryError::ProjectMoved { name: name.to_string(), path: entry.path }),
        };
    }
    let legacy = legacy_folder(home, name);
    if legacy.is_dir() {
        return Ok(Project { name: name.to_string(), root: None, dir: legacy.join(EKKO_DIR_NAME), id: None });
    }
    Err(DirectoryError::UnknownProject(name.to_string()))
}

pub(crate) fn registry_file(home: &Path) -> PathBuf {
    home.join(EKKO_DIR_NAME).join(REGISTRY)
}

/// Every registered project whose folder still holds its board, as the
/// folder and the `.ekko/` holding the board: the boards whose cues the
/// guard reads (task 805). A moved or lost project is left out, since the
/// folder its cues guard is not where it was.
pub fn boards(home: &Path) -> Vec<(PathBuf, PathBuf)> {
    let Ok(registry) = read_registry(home) else { return Vec::new() };
    registry.projects.iter().filter(|entry| !missing(entry)).map(|entry| (entry.path.clone(), entry.path.join(EKKO_DIR_NAME))).collect()
}

/// The projects linked to `project` (task 811), in name order: each still
/// registered and in its folder, where a session reaches its board. None for
/// a project without an id, a legacy one `ekko init` has not adopted.
pub fn linked(home: &Path, project: &Project) -> Vec<Project> {
    let Some(id) = project.id.as_deref() else { return Vec::new() };
    let Ok(registry) = read_registry(home) else { return Vec::new() };
    let mut found: Vec<Project> = registry
        .links
        .iter()
        .filter_map(|[a, b]| if a == id { Some(b) } else if b == id { Some(a) } else { None })
        .filter_map(|other| registry.projects.iter().find(|entry| &entry.id == other))
        .filter_map(|entry| read_marker(&entry.path).filter(|marker| marker.id == entry.id).map(|marker| project_at(&entry.path, marker)))
        .collect();
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found.dedup_by(|a, b| a.id == b.id);
    found
}

/// The answer that made the link a question proposed (task 811), before
/// the asking session wrote the answers itself (task 1044): a question
/// asked then records no `applies`, and still takes it.
pub const LINK: &str = "Link";

/// Links `a` and `b`, both ways (task 811); false when they were linked
/// already. Only the user makes a link: the caller has made sure of that.
pub fn link(home: &Path, a: &Project, b: &Project) -> Result<bool, DirectoryError> {
    let [a_id, b_id] = linkable(a, b)?;
    join(home, &a_id, &b_id)
}

/// For tests: projects `names`, each made in a folder of `home` named after
/// it, with the pairs of them `links` names linked; where each one's board
/// is, in that order.
#[cfg(test)]
pub(crate) fn test_projects(home: &Path, names: &[&str], links: &[(usize, usize)]) -> Vec<crate::directory::Location> {
    let projects: Vec<Project> = names
        .iter()
        .map(|name| {
            let folder = home.join(name);
            fs::create_dir_all(&folder).unwrap();
            init(home, &folder, None, None, 0).unwrap();
            resolve_named(home, name).unwrap()
        })
        .collect();
    for (a, b) in links {
        link(home, &projects[*a], &projects[*b]).unwrap();
    }
    names.iter().map(|name| crate::directory::locate(home, home, None, None, Some(name)).unwrap()).collect()
}

/// Links the projects whose ids are `a` and `b`, as `link` does, for a
/// question that named them when it was asked: each must still be
/// registered, since one gone since would leave a link to nothing.
pub fn join(home: &Path, a: &str, b: &str) -> Result<bool, DirectoryError> {
    let _registry_lock = lock_registry(home)?;
    let mut registry = read_registry(home)?;
    if [a, b].iter().any(|id| !registry.projects.iter().any(|entry| entry.id == *id)) {
        return Err(DirectoryError::Unlinkable("a project the link names is no longer registered: nothing was linked".to_string()));
    }
    if registry.links.iter().any(|pair| joins(pair, a, b)) {
        return Ok(false);
    }
    registry.links.push([a.to_string(), b.to_string()]);
    write_registry(home, &registry)?;
    Ok(true)
}

/// Takes away the link between `a` and `b`; false when there was none.
pub fn unlink(home: &Path, a: &Project, b: &Project) -> Result<bool, DirectoryError> {
    let [a_id, b_id] = linkable(a, b)?;
    let _registry_lock = lock_registry(home)?;
    let mut registry = read_registry(home)?;
    let before = registry.links.len();
    registry.links.retain(|pair| !joins(pair, &a_id, &b_id));
    if registry.links.len() == before {
        return Ok(false);
    }
    write_registry(home, &registry)?;
    Ok(true)
}

/// The ids of two projects a link can join: two registered projects, and
/// two different ones.
pub fn linkable(a: &Project, b: &Project) -> Result<[String; 2], DirectoryError> {
    let id = |project: &Project| {
        project.id.clone().ok_or_else(|| {
            DirectoryError::Unlinkable(format!(
                "{} is a legacy project, which a link cannot name: adopt it with ekko init in its folder first",
                project.name
            ))
        })
    };
    let (a_id, b_id) = (id(a)?, id(b)?);
    if a_id == b_id {
        return Err(DirectoryError::Unlinkable(format!("{} is this project: a session reaches its own board already", b.name)));
    }
    Ok([a_id, b_id])
}

/// Whether `pair` is the link between the projects `a` and `b`, either way round.
fn joins(pair: &[String; 2], a: &str, b: &str) -> bool {
    (pair[0] == a && pair[1] == b) || (pair[0] == b && pair[1] == a)
}

fn read_registry(home: &Path) -> Result<Registry, DirectoryError> {
    let path = registry_file(home);
    match fs::read_to_string(&path) {
        Ok(text) => {
            serde_json::from_str(&text).map_err(|e| DirectoryError::Io(format!("{}: {e}", path.display())))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Registry::default()),
        Err(e) => Err(DirectoryError::Io(format!("{}: {e}", path.display()))),
    }
}

/// Written whole to a temporary file and renamed over the registry, so a
/// reader never sees half of it. Callers hold the registry lock.
fn write_registry(home: &Path, registry: &Registry) -> Result<(), DirectoryError> {
    let path = registry_file(home);
    let temp = path.with_extension(format!("json.{}", std::process::id()));
    fs::write(&temp, crate::json::to_pretty_string(registry).map_err(io)?)?;
    fs::rename(&temp, &path)?;
    Ok(())
}

fn lock_registry(home: &Path) -> Result<fs::File, DirectoryError> {
    let dir = home.join(EKKO_DIR_NAME);
    fs::create_dir_all(&dir)?;
    crate::storage::lock_path(&dir.join(REGISTRY_LOCK)).map_err(io)
}

/// Records `marker` at `root`, replacing the entry with the same id -- a
/// project run through `init` again after it moved -- and any entry holding
/// the same name whose folder no longer holds that project.
fn register(registry: &mut Registry, marker: &Marker, root: &Path) {
    registry.projects.retain(|entry| {
        entry.id != marker.id
            && !(entry.name == marker.name && read_marker(&entry.path).is_none_or(|found| found.id != entry.id))
    });
    registry.projects.push(Registered {
        name: marker.name.clone(),
        id: marker.id.clone(),
        path: root.to_path_buf(),
        unknown: BTreeMap::new(),
    });
    registry.projects.sort_by(|a, b| a.name.cmp(&b.name));
}

fn holds_board(dir: &Path) -> bool {
    dir.join("storage").join("storage.json").is_file()
}

/// Makes a folder a project: `ekko init [folder] [--name NAME]`.
///
/// The folder is `folder` resolved against `cwd`, or `cwd`; inside a git
/// repository it is the top of the repository, because a repository has one
/// project, wherever in it `init` was typed. The name is the folder's own,
/// unless `--name` gives another, and must not already belong to a project
/// that still exists.
///
/// Refused in home, whose `.ekko/` is the default board, and in a folder
/// inside another project with no repository boundary between them. Run in a
/// folder that is already a project, it only records where the project is --
/// which is how a moved project is found again.
pub fn init(
    home: &Path,
    cwd: &Path,
    folder: Option<&str>,
    name: Option<&str>,
    now_millis: i64,
) -> Result<Initialized, DirectoryError> {
    let asked = match folder {
        Some(folder) => crate::paths::resolve_path(home, cwd, folder),
        None => cwd.to_path_buf(),
    };
    if !asked.is_dir() {
        return Err(DirectoryError::NotAFolder(asked.display().to_string()));
    }
    // Canonical, so two spellings of one folder -- a symlink such as
    // ~/Projects pointing at /projects -- are one project and not two.
    let asked = fs::canonicalize(&asked)?;
    let root = asked.ancestors().find_map(repository_root).unwrap_or(asked);
    let home = fs::canonicalize(home).unwrap_or_else(|_| home.to_path_buf());
    if root == home {
        return Err(DirectoryError::InitInHome);
    }
    if repository_root(&root).is_none() {
        if let Some(outer) = root.parent().and_then(|parent| discover(&home, parent)) {
            return Err(DirectoryError::NestedProject { name: outer.name, root: outer.root.unwrap_or_default() });
        }
    }

    let _registry_lock = lock_registry(&home)?;
    let mut registry = read_registry(&home)?;
    let dir = root.join(EKKO_DIR_NAME);

    if let Some(marker) = read_marker(&root) {
        if name.is_some_and(|asked| asked.trim() != marker.name) {
            return Err(DirectoryError::AlreadyProject { name: marker.name, root });
        }
        register(&mut registry, &marker, &root);
        write_registry(&home, &registry)?;
        let excluded = exclude_from_git(&root)?;
        return Ok(Initialized {
            name: marker.name,
            id: marker.id,
            root,
            existing: true,
            claimed: false,
            adopted: None,
            restored: None,
            excluded,
        });
    }

    let named = name.map(|asked| asked.trim().to_string());
    let name = match &named {
        Some(name) => name.clone(),
        None => root.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string(),
    };
    validate_name(&name)?;
    let taken = registry
        .projects
        .iter()
        .find(|entry| entry.name == name && read_marker(&entry.path).is_some_and(|found| found.id == entry.id));
    if let Some(taken) = taken {
        return Err(DirectoryError::NameTaken { name, path: taken.path.clone() });
    }

    // A project whose board is gone -- its folder removed, cleaned with `git
    // clean -fdx`, or cloned again elsewhere -- comes back from its copy
    // instead of starting empty (task 503): the one registered for this
    // folder, else the one of this name. A board already in the folder, or a
    // legacy one waiting under that name, is kept instead.
    if !holds_board(&dir) && !legacy_folder(&home, &name).is_dir() {
        let restorable: Vec<&Registered> =
            registry.projects.iter().filter(|entry| missing(entry) && copied_at(&home, &entry.id).is_some()).collect();
        let here = restorable
            .iter()
            .find(|entry| entry.path == root && named.as_ref().is_none_or(|asked| *asked == entry.name));
        if let Some(entry) = here.or_else(|| restorable.iter().find(|entry| entry.name == name)).map(|entry| (*entry).clone()) {
            let restored = restore(&home, &entry, &dir)?;
            let marker = Marker { name: entry.name, id: entry.id };
            register(&mut registry, &marker, &root);
            write_registry(&home, &registry)?;
            let excluded = exclude_from_git(&root)?;
            return Ok(Initialized {
                name: marker.name,
                id: marker.id,
                root,
                existing: false,
                claimed: false,
                adopted: None,
                restored: Some(restored),
                excluded,
            });
        }
    }

    let legacy = legacy_folder(&home, &name);
    let mut adopted = None;
    let mut claimed = false;
    if legacy.is_dir() {
        if holds_board(&dir) {
            return Err(DirectoryError::AdoptConflict { name, legacy, root });
        }
        adopted = Some(adopt(&home, &legacy, &dir, &name, now_millis)?);
    } else if holds_board(&dir) {
        claimed = true;
    }

    fs::create_dir_all(&dir)?;
    let marker = Marker { name: name.clone(), id: crate::item::new_uid() };
    fs::write(dir.join(MARKER), crate::json::to_pretty_string(&marker).map_err(io)?)?;
    register(&mut registry, &marker, &root);
    write_registry(&home, &registry)?;
    let excluded = exclude_from_git(&root)?;
    Ok(Initialized { name, id: marker.id, root, existing: false, claimed, adopted, restored: None, excluded })
}

/// Brings a lost project's board back into `dir` from its copy. The marker
/// is written from the registry, whatever the copy holds, so the project
/// comes back as the one registered.
fn restore(home: &Path, entry: &Registered, dir: &Path) -> Result<Restored, DirectoryError> {
    let copy = copies(home, &entry.id).ok_or_else(|| io(format!("no copy of {}", entry.name)))?;
    let copied = copied_at(home, &entry.id);
    copy_tree(&copy, dir)?;
    let marker = Marker { name: entry.name.clone(), id: entry.id.clone() };
    fs::write(dir.join(MARKER), crate::json::to_pretty_string(&marker).map_err(io)?)?;
    Ok(Restored { from: entry.path.clone(), copied })
}

/// Moves a legacy project's board into its folder: copied under the board's
/// own lock, so a write racing the move lands before the copy or waits until
/// after it, then the old copy parked in the trash rather than deleted.
///
/// A copy and not a rename, because `~/.ekko/` and a project folder are often
/// on different filesystems, and a rename cannot cross between them.
fn adopt(home: &Path, legacy: &Path, dir: &Path, name: &str, now_millis: i64) -> Result<Adopted, DirectoryError> {
    let board = legacy.join(EKKO_DIR_NAME);
    let storage = Storage::new(&board).map_err(io)?;
    let _board_lock = storage.acquire_lock().map_err(io)?;
    copy_tree(&board, dir)?;

    let trash = home.join(EKKO_DIR_NAME).join(TRASH_DIR_NAME);
    fs::create_dir_all(&trash)?;
    let parked = trash.join(format!("{name}-{now_millis}"));
    fs::rename(legacy, &parked)?;
    // Gone once the last project has left it: the directory note 92 retired.
    let _ = fs::remove_dir(home.join(EKKO_DIR_NAME).join(PROJECTS_DIR_NAME));
    Ok(Adopted { from: legacy.to_path_buf(), parked })
}

/// Everything under `from` into `to`, leaving out the lock file and the
/// temporary-file directory: both belong to the copy being left behind.
fn copy_tree(from: &Path, to: &Path) -> Result<(), DirectoryError> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let entry_name = entry.file_name();
        if entry_name == ".lock" || entry_name == ".temp" {
            continue;
        }
        let target = to.join(&entry_name);
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// A rename where one can happen, and a copy then a removal across
/// filesystems, where it cannot.
fn move_tree(from: &Path, to: &Path) -> Result<(), DirectoryError> {
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(error) if error.raw_os_error() == Some(libc::EXDEV) => {
            copy_tree(from, to)?;
            fs::remove_dir_all(from)?;
            Ok(())
        }
        Err(error) => Err(DirectoryError::Trash(format!("{} -> {}: {error}", from.display(), to.display()))),
    }
}

/// Keeps `.ekko/` out of git without touching a tracked file: the entry goes
/// in `.git/info/exclude`, which belongs to the clone alone. That is the
/// answer the user gave on 2026-08-26 -- the board is ignored at first --
/// and removing the line is all it takes to share one later. Returns whether
/// `root` is a repository at all.
fn exclude_from_git(root: &Path) -> Result<bool, DirectoryError> {
    let git = root.join(".git");
    if !git.is_dir() {
        return Ok(false);
    }
    let info = git.join("info");
    fs::create_dir_all(&info)?;
    let exclude = info.join("exclude");
    let current = fs::read_to_string(&exclude).unwrap_or_default();
    if current.lines().any(|line| matches!(line.trim(), "/.ekko/" | ".ekko/" | "/.ekko" | ".ekko")) {
        return Ok(true);
    }
    let separator = if current.is_empty() || current.ends_with('\n') { "" } else { "\n" };
    fs::write(&exclude, format!("{current}{separator}/.ekko/\n"))?;
    Ok(true)
}

/// Every project: the registered ones -- with their folder, or marked missing
/// when the folder no longer holds them -- and the legacy ones still waiting
/// under `~/.ekko/projects/`, which would otherwise look lost until adopted.
/// In name order. Reads only; a listing never writes.
pub fn list(home: &Path) -> Vec<ProjectSummary> {
    let mut listed: Vec<ProjectSummary> = read_registry(home)
        .unwrap_or_default()
        .projects
        .into_iter()
        .map(|entry| {
            let here = read_marker(&entry.path).is_some_and(|found| found.id == entry.id);
            let (complete, tasks, notes) = if here {
                count_items(&entry.path.join(EKKO_DIR_NAME).join("storage").join("storage.json"))
            } else {
                (0, 0, 0)
            };
            ProjectSummary {
                copied: if here { None } else { copied_at(home, &entry.id) },
                name: entry.name,
                complete,
                tasks,
                notes,
                path: Some(entry.path.display().to_string()),
                status: if here { "here" } else { "missing" },
            }
        })
        .collect();

    if let Ok(entries) = fs::read_dir(home.join(EKKO_DIR_NAME).join(PROJECTS_DIR_NAME)) {
        for entry in entries.filter_map(Result::ok).filter(|entry| entry.path().is_dir()) {
            let Ok(name) = entry.file_name().into_string() else { continue };
            if listed.iter().any(|project| project.name == name) {
                continue;
            }
            let storage = entry.path().join(EKKO_DIR_NAME).join("storage").join("storage.json");
            let (complete, tasks, notes) = count_items(&storage);
            listed.push(ProjectSummary { name, complete, tasks, notes, path: None, status: "legacy", copied: None });
        }
    }

    listed.sort_by(|a, b| a.name.cmp(&b.name));
    listed
}

/// Moves a project's board to the trash and forgets the project, returning
/// where the board went. The folder itself stays; only its `.ekko/` goes.
///
/// The caller holds the project's own lock, so a concurrent write finishes
/// rather than having its directory taken from under it. The timestamp keeps
/// two projects of one name from replacing each other in the trash.
pub fn destroy(home: &Path, project: &Project, now_millis: i64) -> Result<PathBuf, DirectoryError> {
    let trash = home.join(EKKO_DIR_NAME).join(TRASH_DIR_NAME);
    fs::create_dir_all(&trash).map_err(|e| DirectoryError::Trash(format!("{}: {e}", trash.display())))?;
    let target = trash.join(format!("{}-{now_millis}", project.name));

    match &project.id {
        // A legacy project is its whole folder under ~/.ekko/projects/, on the
        // same filesystem as the trash.
        None => {
            let folder = legacy_folder(home, &project.name);
            fs::rename(&folder, &target)
                .map_err(|e| DirectoryError::Trash(format!("{} -> {}: {e}", folder.display(), target.display())))?;
        }
        Some(id) => {
            move_tree(&project.dir, &target)?;
            // The board is in the trash, and its copy holds nothing more.
            if let Some(copy) = copies(home, id) {
                let _ = fs::remove_dir_all(copy);
            }
            let _registry_lock = lock_registry(home)?;
            let mut registry = read_registry(home)?;
            registry.projects.retain(|entry| &entry.id != id);
            registry.links.retain(|pair| !pair.contains(id));
            write_registry(home, &registry)?;
        }
    }
    Ok(target)
}

/// `--destroy` on a registered project whose folder no longer holds its
/// board, which leaves no board to move: forgets the project, and moves its
/// copy, when it has one, to the trash, where `destroy` puts a board.
pub fn forget(home: &Path, name: &str, now_millis: i64) -> Result<Forgotten, DirectoryError> {
    let _registry_lock = lock_registry(home)?;
    let mut registry = read_registry(home)?;
    let Some(entry) = registry.projects.iter().find(|entry| entry.name == name && missing(entry)).cloned() else {
        return Err(DirectoryError::UnknownProject(name.to_string()));
    };
    let parked = match copies(home, &entry.id).filter(|copy| copy.is_dir()) {
        Some(copy) => {
            let trash = home.join(EKKO_DIR_NAME).join(TRASH_DIR_NAME);
            fs::create_dir_all(&trash).map_err(|e| DirectoryError::Trash(format!("{}: {e}", trash.display())))?;
            let target = trash.join(format!("{name}-{now_millis}"));
            move_tree(&copy, &target)?;
            Some(target)
        }
        None => None,
    };
    registry.projects.retain(|kept| kept.id != entry.id);
    registry.links.retain(|pair| !pair.contains(&entry.id));
    write_registry(home, &registry)?;
    Ok(Forgotten { name: entry.name, path: entry.path, parked })
}

/// Reads one project's storage directly rather than through `Storage`, which
/// creates the directories it expects -- a side effect no listing should
/// have. An absent file is an empty project, and so is one that cannot be
/// parsed: failing the whole listing over one bad project would hide the good
/// ones, and opening that project reports the real error.
fn count_items(storage_file: &Path) -> (u32, u32, u32) {
    let Ok(contents) = fs::read_to_string(storage_file) else {
        return (0, 0, 0);
    };
    let Ok(items) = serde_json::from_str::<ItemMap>(&contents) else {
        return (0, 0, 0);
    };

    // What is in front of you, counted the way the project's own stats line
    // counts it: stashed and trashed items are away, and `tally` keeps
    // cancelled tasks out of the total.
    let visible = items.values().filter(|item| item.stashed.is_none() && item.trashed.is_none());
    let (complete, tasks) = crate::item::tally(visible.clone());
    let notes = visible.filter(|item| !item.is_task).count() as u32;
    (complete, tasks, notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1787600000000;

    /// A scratch home, canonical so paths compare the way init records them.
    fn temp(tag: &str) -> PathBuf {
        let dir = crate::paths::test_dir(&format!("ekko-project-{tag}"));
        fs::create_dir_all(&dir).unwrap();
        fs::canonicalize(&dir).unwrap()
    }

    fn folder(home: &Path, path: &str) -> PathBuf {
        let dir = home.join(path);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A board holding one task, written straight into `dir`.
    fn seed(dir: &Path, description: &str) {
        let storage = Storage::new(dir).unwrap();
        let mut items = ItemMap::new();
        items.insert(1, crate::item::Item::new_task(1, description.to_string(), vec!["@a".into()], 1));
        storage.set(&items).unwrap();
    }

    /// A folder becomes a project, named after the folder and found from
    /// anywhere inside it; a folder elsewhere has none.
    #[test]
    fn a_folder_becomes_a_project_found_from_anywhere_inside_it() {
        let home = temp("plain");
        let site = folder(&home, "work/site");
        let deep = folder(&home, "work/site/a/b");
        let elsewhere = folder(&home, "work/other");

        let done = init(&home, &deep, Some(site.to_str().unwrap()), None, NOW).unwrap();
        assert_eq!((done.name.as_str(), done.existing, done.excluded), ("site", false, false));
        assert_eq!(discover(&home, &deep).map(|p| p.name), Some("site".to_string()));
        assert_eq!(discover(&home, &elsewhere), None);
        assert_eq!(resolve_named(&home, "site").unwrap().root, Some(site));

        fs::remove_dir_all(&home).ok();
    }

    /// Inside a git repository the project goes to the top of it, wherever
    /// init was typed, and `.ekko/` is kept out of git through the clone's own
    /// exclude file -- once, however many times init runs.
    #[test]
    fn inside_a_repository_the_project_goes_to_its_top_and_out_of_git() {
        let home = temp("repo");
        let repo = folder(&home, "src/app");
        fs::create_dir_all(repo.join(".git")).unwrap();
        let deep = folder(&home, "src/app/crates/core");

        let done = init(&home, &deep, None, None, NOW).unwrap();
        assert_eq!((done.root.clone(), done.excluded), (repo.clone(), true));
        assert!(init(&home, &deep, None, None, NOW).unwrap().existing);
        let exclude = fs::read_to_string(repo.join(".git").join("info").join("exclude")).unwrap();
        assert_eq!(exclude.matches("/.ekko/").count(), 1, "{exclude:?}");

        fs::remove_dir_all(&home).ok();
    }

    /// A repository is its own world: discovery does not look past its top,
    /// so a repository inside a folder project can be a project of its own --
    /// while a plain folder inside a project cannot, and neither can home.
    #[test]
    fn a_repository_bounds_discovery_and_nesting_needs_one() {
        let home = temp("nesting");
        let notes = folder(&home, "notes");
        init(&home, &notes, None, None, NOW).unwrap();

        let drafts = folder(&home, "notes/drafts");
        assert!(matches!(init(&home, &drafts, None, None, NOW), Err(DirectoryError::NestedProject { .. })));

        let tool = folder(&home, "notes/tool");
        fs::create_dir_all(tool.join(".git")).unwrap();
        assert_eq!(discover(&home, &tool), None, "discovery crossed the repository boundary");
        init(&home, &tool, None, None, NOW).expect("a repository inside a folder project was refused");
        assert_eq!(discover(&home, &tool).map(|p| p.name), Some("tool".to_string()));

        assert!(matches!(init(&home, &home, None, None, NOW), Err(DirectoryError::InitInHome)));

        fs::remove_dir_all(&home).ok();
    }

    /// The name is the folder's own unless --name gives another, and belongs
    /// to one living project: a taken name is refused, while the name of a
    /// project gone from its folder is free again.
    #[test]
    fn a_name_is_the_folders_and_belongs_to_one_living_project() {
        let home = temp("names");
        let first = folder(&home, "a/app");
        let second = folder(&home, "b/app");
        init(&home, &first, None, None, NOW).unwrap();

        let Err(DirectoryError::NameTaken { path, .. }) = init(&home, &second, None, None, NOW) else {
            panic!("a taken name was accepted")
        };
        assert_eq!(path, first);
        assert_eq!(init(&home, &second, None, Some("app-b"), NOW).unwrap().name, "app-b");

        fs::remove_dir_all(&first).unwrap();
        let third = folder(&home, "c/app");
        assert_eq!(init(&home, &third, None, None, NOW).unwrap().name, "app", "a vanished project kept its name");

        let odd = folder(&home, "d/odd");
        assert!(matches!(init(&home, &odd, None, Some("a/b"), NOW), Err(DirectoryError::InvalidProjectName(_))));

        fs::remove_dir_all(&home).ok();
    }

    /// A project moved to another folder is reported as moved, never silently
    /// lost or duplicated, and init in its new folder records where it went.
    #[test]
    fn a_moved_project_is_reported_and_init_finds_it_again() {
        let home = temp("moved");
        let before = folder(&home, "old/site");
        let id = init(&home, &before, None, None, NOW).unwrap().id;
        let after = folder(&home, "new").join("site");
        fs::rename(&before, &after).unwrap();

        assert!(matches!(resolve_named(&home, "site"), Err(DirectoryError::ProjectMoved { .. })));
        assert_eq!(discover(&home, &after).map(|p| p.name), Some("site".to_string()), "the marker stayed behind");

        let again = init(&home, &after, None, None, NOW).unwrap();
        assert_eq!((again.existing, again.id), (true, id));
        assert_eq!(resolve_named(&home, "site").unwrap().root, Some(after));

        fs::remove_dir_all(&home).ok();
    }

    /// A project from before projects lived in folders is moved into the
    /// folder of the same name: its board arrives intact, the old copy is
    /// parked in the trash, and the old directory goes once it is empty.
    #[test]
    fn init_adopts_the_legacy_project_of_the_same_name() {
        let home = temp("adopt");
        let legacy = home.join(".ekko").join("projects").join("minium");
        seed(&legacy.join(".ekko"), "made before projects moved");
        let repo = folder(&home, "src/minium");

        let adopted = init(&home, &repo, None, None, NOW).unwrap().adopted.expect("nothing was adopted");
        assert_eq!(adopted.from, legacy);
        assert!(adopted.parked.join(".ekko").join("storage").join("storage.json").is_file(), "the old copy is gone");
        assert!(!legacy.exists() && !home.join(".ekko").join("projects").exists());

        let board = Storage::new(&repo.join(".ekko")).unwrap().get().unwrap();
        assert_eq!(board[&1].description, "made before projects moved");
        assert_eq!(resolve_named(&home, "minium").unwrap().root, Some(repo));

        fs::remove_dir_all(&home).ok();
    }

    /// Two boards for one project are never merged: adoption is refused, and
    /// nothing moves.
    #[test]
    fn adoption_is_refused_when_the_folder_already_holds_a_board() {
        let home = temp("conflict");
        let legacy = home.join(".ekko").join("projects").join("site");
        seed(&legacy.join(".ekko"), "legacy");
        let site = folder(&home, "work/site");
        seed(&site.join(".ekko"), "local");

        assert!(matches!(init(&home, &site, None, None, NOW), Err(DirectoryError::AdoptConflict { .. })));
        assert!(legacy.join(".ekko").join("storage").join("storage.json").is_file());
        assert_eq!(discover(&home, &site), None);

        fs::remove_dir_all(&home).ok();
    }

    /// A board already in the folder with no marker -- what `--ekko-dir
    /// <folder>` leaves behind -- becomes the project in place.
    #[test]
    fn a_board_already_in_the_folder_is_claimed_in_place() {
        let home = temp("claim");
        let site = folder(&home, "work/site");
        seed(&site.join(".ekko"), "kept");

        assert!(init(&home, &site, None, None, NOW).unwrap().claimed);
        assert_eq!(Storage::new(&site.join(".ekko")).unwrap().get().unwrap()[&1].description, "kept");

        fs::remove_dir_all(&home).ok();
    }

    /// Every worktree of a repository shares its project: a linked worktree's
    /// `.git` file points into the main checkout, and that is where the
    /// project is found, and where init puts it.
    #[test]
    fn a_linked_worktree_shares_the_main_checkouts_project() {
        let home = temp("worktree");
        let main = folder(&home, "src/app");
        let pointer = main.join(".git").join("worktrees").join("feature");
        fs::create_dir_all(&pointer).unwrap();
        let worktree = folder(&home, "src/app-feature");
        fs::write(worktree.join(".git"), format!("gitdir: {}\n", pointer.display())).unwrap();
        let deep = folder(&home, "src/app-feature/src");

        assert_eq!(init(&home, &deep, None, None, NOW).unwrap().root, main);
        assert_eq!(discover(&home, &deep).and_then(|p| p.root), Some(main));

        fs::remove_dir_all(&home).ok();
    }

    /// The listing says where each project is, which ones their folder no
    /// longer holds, and which still wait under ~/.ekko/projects/ -- and it
    /// writes nothing while it looks.
    #[test]
    fn the_listing_shows_every_project_and_writes_nothing() {
        let home = temp("list");
        let here = folder(&home, "work/here");
        init(&home, &here, None, None, NOW).unwrap();
        let gone = folder(&home, "work/gone");
        init(&home, &gone, None, None, NOW).unwrap();
        fs::remove_dir_all(&gone).unwrap();
        seed(&home.join(".ekko").join("projects").join("old").join(".ekko"), "waiting");
        let registry = fs::read(registry_file(&home)).unwrap();

        let listed: Vec<(String, &str)> = list(&home).into_iter().map(|p| (p.name, p.status)).collect();
        assert_eq!(listed, vec![("gone".into(), "missing"), ("here".into(), "here"), ("old".into(), "legacy")]);
        assert_eq!(fs::read(registry_file(&home)).unwrap(), registry, "the listing wrote the registry");

        fs::remove_dir_all(&home).ok();
    }

    /// Destroying a project moves its board to the trash and forgets the
    /// project, leaving the folder; a legacy project goes to the trash whole.
    #[test]
    fn destroying_moves_the_board_to_the_trash_and_forgets_the_project() {
        let home = temp("destroy");
        let site = folder(&home, "work/site");
        init(&home, &site, None, None, NOW).unwrap();
        let project = resolve_named(&home, "site").unwrap();

        let trashed = destroy(&home, &project, NOW).unwrap();
        assert!(trashed.join(MARKER).is_file() && site.is_dir() && !site.join(".ekko").exists());
        assert!(matches!(resolve_named(&home, "site"), Err(DirectoryError::UnknownProject(_))));

        seed(&home.join(".ekko").join("projects").join("old").join(".ekko"), "old");
        let legacy = resolve_named(&home, "old").unwrap();
        let parked = destroy(&home, &legacy, NOW + 1).unwrap();
        assert!(parked.join(".ekko").is_dir() && !home.join(".ekko").join("projects").join("old").exists());

        fs::remove_dir_all(&home).ok();
    }

    /// A board of one task, written through the storage a project's writes
    /// go through, which copies it outside the folder.
    fn seed_copied(home: &Path, root: &Path, description: &str) {
        let dir = root.join(EKKO_DIR_NAME);
        let storage = Storage::new(&dir).unwrap().copied_to(copy_dir(home, &dir));
        let mut items = ItemMap::new();
        items.insert(1, crate::item::Item::new_task(1, description.to_string(), vec!["@a".into()], 1));
        storage.set(&items).unwrap();
    }

    /// A board cleaned out of its folder, as `git clean -fdx` does, is
    /// reported lost with its copy, and init there brings it back: the same
    /// project, the same items (task 503).
    #[test]
    fn a_board_cleaned_out_of_its_folder_comes_back_from_its_copy() {
        let home = temp("lost");
        let site = folder(&home, "work/site");
        fs::create_dir_all(site.join(".git")).unwrap();
        let inside = folder(&home, "work/site/src");
        let id = init(&home, &site, None, None, NOW).unwrap().id;
        seed_copied(&home, &site, "kept outside the folder");
        fs::remove_dir_all(site.join(EKKO_DIR_NAME)).unwrap();

        let lost = lost_at(&home, &inside).expect("the lost board went unreported");
        assert_eq!((lost.name.as_str(), lost.path.as_path(), lost.copied.is_some()), ("site", site.as_path(), true));
        let listed = list(&home);
        assert_eq!((listed[0].status, listed[0].copied.is_some()), ("missing", true));

        let again = init(&home, &inside, None, None, NOW).unwrap();
        assert_eq!((again.id, again.existing, again.restored.is_some()), (id, false, true));
        let board = Storage::new(&site.join(EKKO_DIR_NAME)).unwrap().get().unwrap();
        assert_eq!(board[&1].description, "kept outside the folder");
        assert_eq!(lost_at(&home, &inside), None);
        assert_eq!(discover(&home, &inside).map(|p| p.name), Some("site".to_string()));

        fs::remove_dir_all(&home).ok();
    }

    /// A project whose folder is gone comes back from its copy in a new
    /// folder of its name, as when a repository is cloned again elsewhere.
    #[test]
    fn a_lost_project_comes_back_in_a_new_folder_of_its_name() {
        let home = temp("reclone");
        let old = folder(&home, "old/site");
        let id = init(&home, &old, None, None, NOW).unwrap().id;
        seed_copied(&home, &old, "outlived its folder");
        fs::remove_dir_all(&old).unwrap();
        let new = folder(&home, "new/site");

        let again = init(&home, &new, None, None, NOW).unwrap();
        assert_eq!((again.id, again.restored.map(|restored| restored.from)), (id, Some(old)));
        assert_eq!(resolve_named(&home, "site").unwrap().root, Some(new.clone()));
        let board = Storage::new(&new.join(EKKO_DIR_NAME)).unwrap().get().unwrap();
        assert_eq!(board[&1].description, "outlived its folder");

        fs::remove_dir_all(&home).ok();
    }

    /// With no copy there is nothing to bring back, and init starts the
    /// folder afresh; nor does a copy ever replace a board in the folder.
    #[test]
    fn init_restores_only_a_copy_into_a_folder_with_no_board() {
        let home = temp("nocopy");
        let site = folder(&home, "work/site");
        let id = init(&home, &site, None, None, NOW).unwrap().id;
        seed(&site.join(EKKO_DIR_NAME), "never copied");
        fs::remove_dir_all(site.join(EKKO_DIR_NAME)).unwrap();
        assert_eq!(lost_at(&home, &site).map(|lost| lost.copied), Some(None));
        let fresh = init(&home, &site, None, None, NOW).unwrap();
        assert!(fresh.restored.is_none() && fresh.id != id);

        let other = folder(&home, "work/other");
        init(&home, &other, None, None, NOW).unwrap();
        seed_copied(&home, &other, "the copy");
        fs::remove_file(other.join(EKKO_DIR_NAME).join(MARKER)).unwrap();
        let kept = init(&home, &other, None, None, NOW).unwrap();
        assert!(kept.claimed && kept.restored.is_none(), "the board in the folder was not kept");

        fs::remove_dir_all(&home).ok();
    }

    /// --destroy forgets a lost project and parks its copy in the trash; on a
    /// project still in its folder, the copy goes with the board.
    #[test]
    fn destroy_forgets_a_lost_project_and_parks_its_copy() {
        let home = temp("forget");
        let site = folder(&home, "work/site");
        let id = init(&home, &site, None, None, NOW).unwrap().id;
        seed_copied(&home, &site, "gone");
        fs::remove_dir_all(&site).unwrap();

        assert!(matches!(forget(&home, "other", NOW), Err(DirectoryError::UnknownProject(_))));
        let forgotten = forget(&home, "site", NOW).unwrap();
        let parked = forgotten.parked.expect("the copy was not parked");
        assert!(parked.join("storage").join("storage.json").is_file());
        assert!(!copies(&home, &id).unwrap().exists() && list(&home).is_empty());

        let kept = folder(&home, "work/kept");
        let id = init(&home, &kept, None, None, NOW).unwrap().id;
        seed_copied(&home, &kept, "trashed");
        let trash = destroy(&home, &resolve_named(&home, "kept").unwrap(), NOW + 1).unwrap();
        assert!(trash.join("storage").join("storage.json").is_file());
        assert!(!copies(&home, &id).unwrap().exists(), "the copy outlived the destroyed board");

        fs::remove_dir_all(&home).ok();
    }

    /// The project registry keeps a later version's fields, on itself and
    /// on each project (task 829): every ekko that registers a project
    /// rewrites it whole.
    #[test]
    fn the_registry_keeps_what_it_does_not_know() {
        crate::json::assert_keeps_what_it_does_not_know::<Registry>(serde_json::json!({
            "projects": [{"name": "ekko", "id": "18d74dd807e6db5a-4c0c6", "path": "/home/roldant/Projetos/ekko"}]
        }));
    }

    /// A link joins two projects both ways (task 811): each lists the other
    /// -- and nothing further, since a link is not passed on -- until either
    /// side takes it away or either project leaves. A project links neither
    /// to itself nor to a legacy one, and a question naming a project gone
    /// since links nothing.
    #[test]
    fn a_link_joins_two_projects_both_ways_until_either_leaves() {
        let home = temp("links");
        for name in ["site", "blog", "shop"] {
            init(&home, &folder(&home, name), None, None, NOW).unwrap();
        }
        let project = |name: &str| resolve_named(&home, name).unwrap();
        let names = |name: &str| linked(&home, &project(name)).into_iter().map(|p| p.name).collect::<Vec<_>>();
        let refused = |result: Result<bool, DirectoryError>| result.unwrap_err().to_string();
        assert!(names("site").is_empty());

        assert!(link(&home, &project("site"), &project("shop")).unwrap());
        assert!(link(&home, &project("site"), &project("blog")).unwrap());
        assert!(!link(&home, &project("blog"), &project("site")).unwrap(), "linked already, either way round");
        assert_eq!(names("site"), ["blog", "shop"]);
        assert_eq!(names("blog"), ["site"]);
        assert_eq!(names("shop"), ["site"]);

        assert!(refused(link(&home, &project("site"), &project("site"))).contains("is this project"));
        let legacy = Project { name: "old".into(), root: None, dir: home.join("old"), id: None };
        assert!(refused(link(&home, &project("site"), &legacy)).contains("old is a legacy project"));
        let site_id = project("site").id.unwrap();
        assert!(refused(join(&home, &site_id, "18d0000000000000-00000")).contains("no longer registered"));

        assert!(unlink(&home, &project("blog"), &project("site")).unwrap());
        assert!(!unlink(&home, &project("site"), &project("blog")).unwrap());
        assert_eq!(names("site"), ["shop"]);

        destroy(&home, &project("shop"), NOW).unwrap();
        assert!(names("site").is_empty());
        link(&home, &project("site"), &project("blog")).unwrap();
        fs::remove_dir_all(home.join("blog")).unwrap();
        assert!(names("site").is_empty(), "a project whose folder lost its board is left out");
        forget(&home, "blog", NOW).unwrap();
        assert!(read_registry(&home).unwrap().links.is_empty(), "every link leaves with its project");

        fs::remove_dir_all(&home).ok();
    }

    /// Only the user makes a link (task 811): a session's --link-project is
    /// refused, and so is its answer to the question proposing one, which
    /// ask records with the two answers the session wrote (task 1044); the
    /// user's first answer makes it, with a note or without, and the second
    /// -- or ekko's own Link, which this question did not offer -- leaves the
    /// two as they were. A
    /// link that exists is not proposed again, and taking one away is
    /// anyone's. A question asked before, which records no answer of its
    /// own, still takes ekko's Link, and nothing else.
    #[test]
    fn only_the_user_makes_a_link_and_a_session_proposes_one_with_ask() {
        use crate::holder::{test_sessions, Actor};
        use crate::ops::{Draft, Ref};
        let home = temp("proposal");
        for name in ["site", "blog"] {
            init(&home, &folder(&home, name), None, None, NOW).unwrap();
        }
        let (session, _, _) = test_sessions();
        let open = |actor: &Actor| {
            let location = crate::directory::locate(&home, &home.join("site"), None, None, None).unwrap();
            crate::ekko::Ekko::at(&location).unwrap().acting_as(actor.clone())
        };
        let names = || linked(&home, &resolve_named(&home, "site").unwrap()).into_iter().map(|p| p.name).collect::<Vec<_>>();
        let propose = || {
            let ekko = open(&session);
            let mut draft = Draft::open(&ekko)?;
            let inquiry = serde_json::from_value(serde_json::json!({
                "text": "Ligo os quadros?",
                "link_project": "blog",
                "explain": "O que a ligação faz.",
                "options": [
                    {"label": "Ligar", "recommended": true, "why": "uma sessão alcança o outro quadro", "example": "project blog"},
                    {"label": "Não ligar", "why": "nada muda", "example": "cada quadro fica só"}
                ]
            }))
            .unwrap();
            let asked = draft.ask_inquiry(&inquiry, None, &home)?;
            draft.commit(false)?;
            Ok::<_, crate::ekko::EkkoError>(asked)
        };
        let answer = |actor: &Actor, question: u32, text: &str| {
            let ekko = open(actor);
            let mut draft = Draft::open(&ekko)?;
            draft.answer(&Ref::Id(question), text)?;
            draft.commit(false).map(|_| ())
        };

        let refused = open(&session).link_project("blog", true).unwrap_err().to_string();
        assert!(refused.contains("a link is the user's to make, and this runs inside a Claude Code session"), "{refused}");
        assert!(names().is_empty());

        let (question, put) = propose().unwrap();
        let explain = put.explain.clone().unwrap_or_default();
        assert!(explain.starts_with("O que a ligação faz.\n\nThe link proposed: the boards of projects site ("), "{explain}");
        assert_eq!(put.options.iter().map(|option| option.label.as_str()).collect::<Vec<_>>(), ["Ligar", "Não ligar"]);
        assert!(answer(&session, question, "Ligar").is_err(), "a session's answer never links");
        assert!(names().is_empty());
        let (declined, _) = propose().unwrap();
        answer(&Actor::person(), declined, "Não ligar").unwrap();
        assert!(names().is_empty());
        let (old_word, _) = propose().unwrap();
        answer(&Actor::person(), old_word, LINK).unwrap();
        assert!(names().is_empty(), "ekko's own Link applies only a question that offered it");
        answer(&Actor::person(), question, "Ligar — note: só o blog").unwrap();
        assert_eq!(names(), ["blog"]);
        assert!(propose().unwrap_err().to_string().contains("site and blog are linked already"));

        open(&session).link_project("blog", false).unwrap();
        assert!(names().is_empty());

        let asked_before = || {
            let (asked, _) = propose().unwrap();
            let ekko = open(&Actor::person());
            let mut data = ekko.storage.get().unwrap();
            data.get_mut(&asked).unwrap().question.as_mut().unwrap().applies = None;
            ekko.storage.set(&data).unwrap();
            asked
        };
        answer(&Actor::person(), asked_before(), "Ligar").unwrap();
        assert!(names().is_empty(), "the session's word, on a question that recorded none");
        answer(&Actor::person(), asked_before(), LINK).unwrap();
        assert_eq!(names(), ["blog"]);
        fs::remove_dir_all(&home).ok();
    }
}
