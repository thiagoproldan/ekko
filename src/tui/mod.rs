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
mod layout;
mod theme;
mod watch;

use std::io;
use std::path::Path;
use std::time::Duration;

use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind};
use crossterm::execute;

use crate::directory::Location;
use crate::ekko::{Ekko, EkkoError};

/// How long to wait for input before looking at the board's files again.
const TICK: Duration = Duration::from_millis(200);

/// Restores the terminal however the interactive mode ends: returning, an
/// error, or a panic. A raw-mode program that dies without this leaves the
/// shell with no echo, on the alternate screen, typing out every mouse move.
struct Session {
    terminal: ratatui::DefaultTerminal,
}

impl Session {
    fn start() -> io::Result<Self> {
        let terminal = ratatui::try_init()?;
        execute!(io::stdout(), EnableMouseCapture)?;
        // ratatui's own hook puts the screen back; this one releases the
        // mouse, which it does not know was captured.
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ = execute!(io::stdout(), DisableMouseCapture);
            previous(info);
        }));
        Ok(Session { terminal })
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), DisableMouseCapture);
        ratatui::restore();
    }
}

fn io_error(error: io::Error) -> EkkoError {
    EkkoError::Storage(crate::storage::StorageError::Io(error))
}

/// Takes the terminal and shows the board at `location` until the person
/// leaves. `label` is how the board is named, as the CLI names it.
pub fn run(ekko: &Ekko, location: &Location, label: &str, home: &Path, cwd: &Path) -> Result<(), EkkoError> {
    let root = location.project.as_ref().and_then(|project| project.root.clone()).unwrap_or_else(|| location.dir.clone());
    let workspace = app::Workspace {
        label: label.to_string(),
        name: location.project.as_ref().map_or_else(|| "default board".to_string(), |project| project.name.clone()),
        folder: shorten(&root, home),
        cwd: cwd.to_path_buf(),
        branch: git::branch(cwd),
    };
    let mut app = app::App::new(workspace, board::Snapshot::load(ekko, label)?);
    let mut watch = watch::Watch::new(&location.dir);
    let glyphs = theme::Glyphs::from_env();

    let mut session = Session::start().map_err(io_error)?;
    loop {
        session.terminal.draw(|frame| draw::frame(frame, &mut app, glyphs)).map_err(io_error)?;

        if event::poll(TICK).map_err(io_error)? {
            match event::read().map_err(io_error)? {
                Event::Key(key) if key.kind != KeyEventKind::Release => {
                    if let Some(action) = app.key(key) {
                        if app.act(ekko, action)? {
                            watch.settle();
                        }
                    }
                }
                Event::Mouse(mouse) => app.mouse(mouse),
                _ => {}
            }
        }
        if app.quit {
            return Ok(());
        }
        if watch.changed() {
            app.outside_change(ekko)?;
        }
        app.workspace.branch = git::branch(&app.workspace.cwd);
    }
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
