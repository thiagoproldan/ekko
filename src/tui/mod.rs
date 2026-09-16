//! The interactive mode: the board as a dashboard, in VS Code's current shape.
//!
//! A frontend on the same `Ekko` core as the CLI and the MCP server, and it
//! never goes through `render.rs` -- that module reproduces taskbook's output
//! byte for byte and is pinned by golden tests, which nothing here can disturb.
//!
//! ratatui lays out and draws the boxes; crossterm owns the terminal and its
//! events. The board is read without the lock and written through `Ekko`,
//! which holds the lock for one write at a time, and its files are watched so
//! a write from another terminal or an agent shows up without a keypress.

mod app;
mod board;
mod draw;
mod git;
mod graph;
mod layout;
mod list;
mod search;
mod tabs;
mod theme;
mod tree;
mod watch;

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;

use crate::directory::Location;
use crate::ekko::{Ekko, EkkoError};
use crate::project;

/// How long to wait for input before looking at the board's files again.
const TICK: Duration = Duration::from_millis(200);
/// How often a frame is drawn while a graph is moving.
const FRAME: Duration = Duration::from_millis(33);
/// Events handled before the next frame: the pointer reports every cell it
/// crosses, and a frame per report would fall behind the pointer.
const BURST: usize = 64;

/// Restores the terminal however the interactive mode ends: returning, an
/// error, or a panic. A raw-mode program that dies without this leaves the
/// shell with no echo, on the alternate screen, typing out every mouse move.
struct Session {
    terminal: ratatui::DefaultTerminal,
    enhanced: bool,
}

impl Session {
    fn start() -> io::Result<Self> {
        let terminal = ratatui::try_init()?;
        execute!(io::stdout(), EnableMouseCapture)?;
        // Where the terminal speaks the kitty keyboard protocol, it is asked to
        // tell Ctrl+Enter from Enter; anywhere else nothing is pushed, and
        // nothing has to be popped.
        let enhanced = matches!(crossterm::terminal::supports_keyboard_enhancement(), Ok(true));
        if enhanced {
            execute!(io::stdout(), PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES))?;
        }
        // ratatui's own hook puts the screen back; this one gives back what it
        // does not know was taken.
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if enhanced {
                let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
            }
            let _ = execute!(io::stdout(), DisableMouseCapture);
            previous(info);
        }));
        Ok(Session { terminal, enhanced })
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if self.enhanced {
            let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
        }
        let _ = execute!(io::stdout(), DisableMouseCapture);
        ratatui::restore();
    }
}

fn io_error(error: io::Error) -> EkkoError {
    EkkoError::Storage(crate::storage::StorageError::Io(error))
}

/// A board to open: what it is called, and where it and its folder are.
struct Place {
    label: String,
    name: String,
    root: PathBuf,
    dir: PathBuf,
    cwd: PathBuf,
}

/// Takes the terminal and shows the board at `location` until the person
/// leaves. `label` is how the board is named, as the CLI names it.
pub fn run(ekko: &Ekko, location: &Location, label: &str, home: &Path, cwd: &Path) -> Result<(), EkkoError> {
    let since = chrono::Local::now().timestamp_millis();
    let place = Place {
        label: label.to_string(),
        name: location.project.as_ref().map_or_else(|| "default board".to_string(), |project| project.name.clone()),
        root: location.project.as_ref().and_then(|project| project.root.clone()).unwrap_or_else(|| location.dir.clone()),
        dir: location.dir.clone(),
        cwd: cwd.to_path_buf(),
    };
    let mut app = open(ekko, place, home, since)?;
    let mut watch = watch::Watch::new(&location.dir);
    // A project switched to from the Projects view; the board main opened
    // until then.
    let mut switched: Option<Ekko> = None;
    let glyphs = theme::Glyphs::from_env();

    let mut session = Session::start().map_err(io_error)?;
    loop {
        app.animate();
        session.terminal.draw(|frame| draw::frame(frame, &mut app, glyphs)).map_err(io_error)?;

        let wait = if app.animating() { FRAME } else { TICK };
        let mut ready = event::poll(wait).map_err(io_error)?;
        let mut handled = 0;
        while ready && handled < BURST {
            let current = switched.as_ref().unwrap_or(ekko);
            match event::read().map_err(io_error)? {
                Event::Key(key) if key.kind != KeyEventKind::Release => {
                    if let Some(action) = app.key(key) {
                        if app.act(current, action)? {
                            watch.settle();
                        }
                    }
                }
                Event::Mouse(mouse) => app.mouse(mouse),
                _ => {}
            }
            handled += 1;
            if app.quit || app.switch.is_some() {
                break;
            }
            ready = event::poll(Duration::ZERO).map_err(io_error)?;
        }
        if app.quit {
            return Ok(());
        }
        if let Some(name) = app.switch.take() {
            let opened = project::resolve_named(home, &name)
                .map_err(EkkoError::from)
                .and_then(|project| Ok((Ekko::at(&project.dir)?, project)));
            match opened {
                Ok((next, project)) => {
                    let root = project.root.clone().unwrap_or_else(|| project.dir.clone());
                    let place = Place {
                        label: format!("project {}", project.name),
                        name: project.name.clone(),
                        root: root.clone(),
                        dir: project.dir.clone(),
                        cwd: root,
                    };
                    app = open(&next, place, home, since)?;
                    watch = watch::Watch::new(&project.dir);
                    switched = Some(next);
                    continue;
                }
                Err(error) => app.say(error.to_string(), app::Kind::Refused),
            }
        }
        if watch.changed() {
            app.outside_change(switched.as_ref().unwrap_or(ekko))?;
        }
        app.workspace.branch = git::branch(&app.workspace.cwd);
    }
}

/// The interactive mode's state for a board: read, and named.
fn open(ekko: &Ekko, place: Place, home: &Path, since: i64) -> Result<app::App, EkkoError> {
    let workspace = app::Workspace {
        label: place.label,
        name: place.name,
        folder: shorten(&place.root, home),
        branch: git::branch(&place.cwd),
        cwd: place.cwd,
        home: home.to_path_buf(),
        dir: place.dir,
    };
    let snapshot = board::Snapshot::load(ekko, &board::Reading { label: &workspace.label, home, since })?;
    Ok(app::App::new(workspace, snapshot, since))
}

/// `path` with the home directory written as `~`.
fn shorten(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_is_written_as_a_tilde() {
        let home = Path::new("/home/someone");
        assert_eq!(shorten(Path::new("/home/someone/Projects/ekko"), home), "~/Projects/ekko");
        assert_eq!(shorten(home, home), "~");
        assert_eq!(shorten(Path::new("/projects/ekko"), home), "/projects/ekko");
    }
}
