//! Noticing the board change underneath.
//!
//! Every other command reads, answers and exits, so nothing in ekko could
//! ever show stale state. The interactive mode is the first reader that stays
//! -- while the person works in another terminal and an agent writes from a
//! third -- and note 109 on the board named this as the hard part before
//! anyone built it. Storage is written by renaming a fresh file over the old
//! one, so a changed modification time or length is a changed board; polling
//! the three files is enough, and costs a few `stat` calls a second.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

type Stamp = Vec<Option<(SystemTime, u64)>>;

/// The files a board is made of, and how they looked at the last read.
pub struct Watch {
    files: Vec<PathBuf>,
    last: Stamp,
}

impl Watch {
    /// Watches the board in `dir` -- a `.ekko` directory -- from how it looks now.
    pub fn new(dir: &Path) -> Self {
        let files = vec![
            dir.join("storage").join("storage.json"),
            dir.join("storage").join("phases.json"),
            dir.join("archive").join("archive.json"),
        ];
        let last = stamp(&files);
        Watch { files, last }
    }

    /// Whether the board changed since the last time this was asked, or
    /// since `settle`.
    pub fn changed(&mut self) -> bool {
        let now = stamp(&self.files);
        if now == self.last {
            return false;
        }
        self.last = now;
        true
    }

    /// Takes the board as it is now as seen, so a write the interactive mode
    /// made itself is not reported back to it as a change from outside.
    pub fn settle(&mut self) {
        self.last = stamp(&self.files);
    }
}

fn stamp(files: &[PathBuf]) -> Stamp {
    files
        .iter()
        .map(|file| {
            let meta = std::fs::metadata(file).ok()?;
            Some((meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), meta.len()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{ItemMap, Storage};

    fn board(tag: &str) -> PathBuf {
        let nanos = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("ekko-watch-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(dir: &Path, description: &str) {
        let mut items = ItemMap::new();
        items.insert(1, crate::item::Item::new_task(1, description.to_string(), vec!["@a".into()], 1));
        Storage::new(dir).unwrap().set(&items).unwrap();
    }

    /// A write from anywhere is noticed once, and a quiet board is quiet.
    #[test]
    fn a_write_from_elsewhere_is_noticed_once() {
        let dir = board("outside");
        write(&dir, "before");
        let mut watch = Watch::new(&dir);
        assert!(!watch.changed(), "an untouched board reported a change");

        write(&dir, "after, and longer");
        assert!(watch.changed(), "a write went unnoticed");
        assert!(!watch.changed(), "one write was reported twice");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The interactive mode's own write is not news to it.
    #[test]
    fn its_own_write_is_settled_rather_than_reported() {
        let dir = board("own");
        write(&dir, "before");
        let mut watch = Watch::new(&dir);

        write(&dir, "written by the interactive mode itself");
        watch.settle();
        assert!(!watch.changed());

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A board with nothing written yet is watched too: its first write is a
    /// change like any other.
    #[test]
    fn a_board_with_no_files_yet_notices_its_first_write() {
        let dir = board("empty");
        let mut watch = Watch::new(&dir);
        assert!(!watch.changed());

        write(&dir, "first");
        assert!(watch.changed());

        std::fs::remove_dir_all(&dir).ok();
    }
}
