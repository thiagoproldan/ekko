//! The board's database (task 2233): one SQLite file, `board.sqlite`, in the
//! board's `storage/` folder beside `storage.json`, which is now only the
//! stamp a watcher follows. It holds the items, the counters, the journal and
//! every change of the last two weeks, and one write is one transaction of
//! all four, so a writer killed at any point leaves the board as it was
//! before the write or as it is after it (task 2226). Only `Storage` opens
//! it.
//!
//! Each item is kept whole, as the JSON `storage.json` held it, by its
//! display id: every read in this stage still loads the whole board, and
//! what stage 2 queries is built beside these rows, not instead of them.

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{params, Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use serde_json::Value;

use crate::item::Item;
use crate::storage::{Counters, ItemMap, StorageError};

/// The database's name, in the board's `storage/` folder.
pub const FILE: &str = "board.sqlite";

/// The layout this build reads and writes, kept in the file's
/// `user_version`. A build writes no database of a later layout, whose
/// writes may keep more than this one knows to keep up to date; it reads
/// one as long as the items are where it looks for them.
pub const LAYOUT: i64 = 1;

const SCHEMA: &str = "
CREATE TABLE items (id INTEGER PRIMARY KEY, body TEXT NOT NULL);
CREATE TABLE meta (key TEXT PRIMARY KEY, value);
CREATE TABLE journal (seq INTEGER PRIMARY KEY, rev INTEGER NOT NULL, entry TEXT NOT NULL);
CREATE TABLE changes (seq INTEGER PRIMARY KEY, rev INTEGER NOT NULL, at INTEGER NOT NULL, id INTEGER NOT NULL, before TEXT, after TEXT);
CREATE INDEX changes_rev ON changes(rev);
CREATE INDEX changes_at ON changes(at);
";

/// How long a change is kept: the two weeks `history/` kept versions of
/// storage.json for, so any revision of that window can be had back exactly.
pub const CHANGES_KEPT_MS: i64 = 14 * 86_400_000;

/// How long a write waits on another connection's write before failing.
/// Writers already take the board's lock, so this covers only a reader
/// checkpointing the log, which takes milliseconds.
const BUSY: Duration = Duration::from_secs(5);

/// One item a write changes: what it is now, or `None` where it left the
/// board.
pub struct Change<'a> {
    pub id: u32,
    pub after: Option<&'a Item>,
}

/// What one write puts in the database, in one transaction.
pub struct Write<'a> {
    pub changes: Vec<Change<'a>>,
    /// The revision the write is stamped with, which its changes carry.
    pub rev: u64,
    /// When, in epoch milliseconds.
    pub at: i64,
    /// A line of the journal, when the write has one.
    pub journal: Option<&'a Value>,
    /// The counters as they are after it, when they moved.
    pub counters: Option<&'a Counters>,
    /// How much journal to keep: past twice this, the oldest lines go.
    pub journal_bytes: u64,
}

/// Where a database stands after a write: how many writes have changed its
/// items, what a process's kept board is checked against, and how many
/// transactions it has taken, what a copy is checked against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Written {
    pub writes: i64,
    pub commits: i64,
}

pub struct Database {
    conn: Connection,
}

impl Database {
    /// The database at `path`, or `None` where there is no file there.
    pub fn open(path: &Path) -> Result<Option<Database>, StorageError> {
        if !path.exists() {
            return Ok(None);
        }
        let conn = match Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX) {
            Ok(conn) => conn,
            // A board whose folder this user may only read is still read.
            Err(error) => Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(|_| error)?,
        };
        Ok(Some(Database::configured(conn)?))
    }

    /// A new database at `path`, which must not exist, holding no item yet.
    /// Built with a rollback journal, so the file alone is the whole
    /// database until `seal` turns on the write-ahead log.
    pub fn create(path: &Path) -> Result<Database, StorageError> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.execute_batch(SCHEMA)?;
        conn.pragma_update(None, "user_version", LAYOUT)?;
        Database::configured(conn)
    }

    fn configured(conn: Connection) -> Result<Database, StorageError> {
        conn.busy_timeout(BUSY)?;
        // FULL: a write is on disk when its transaction returns, as
        // storage.json was once renamed into place and its folder synced.
        conn.execute_batch("PRAGMA synchronous = FULL; PRAGMA mmap_size = 268435456;")?;
        Ok(Database { conn })
    }

    /// Turns on the write-ahead log, after the rows a new database was
    /// built with: readers then never wait on a writer, nor a writer on
    /// them. Kept in the file, so every later connection has it.
    pub fn seal(&self) -> Result<(), StorageError> {
        let mode: String = self.conn.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(StorageError::Refused(format!("SQLite kept the journal mode {mode}, not WAL")));
        }
        Ok(())
    }

    /// Whether the write-ahead log is on: a database copied by VACUUM INTO,
    /// as the copy outside the folder is, or put back by hand, has it off.
    pub fn logged(&self) -> Result<bool, StorageError> {
        let mode: String = self.conn.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
        Ok(mode.eq_ignore_ascii_case("wal"))
    }

    /// The layout the file says it has.
    pub fn layout(&self) -> Result<i64, StorageError> {
        Ok(self.conn.pragma_query_value(None, "user_version", |row| row.get(0))?)
    }

    /// For the copy of the board outside its folder, which guards against
    /// losing the folder, not the machine: its writes are not synced, as
    /// the hard links that were its copies before cost no sync either. A
    /// killed write still rolls back, by the journal; one the system lost
    /// in a crash leaves the copy out of step, which the next write finds
    /// by its count of commits and makes again whole.
    pub fn relax(&self) -> Result<(), StorageError> {
        self.conn.execute_batch("PRAGMA synchronous = OFF;")?;
        Ok(())
    }

    /// How many writes have changed the items: what a process's kept board is
    /// checked against.
    pub fn writes(&self) -> Result<i64, StorageError> {
        meta_i64(&self.conn, "writes")
    }

    /// How many transactions have written to it.
    pub fn commits(&self) -> Result<i64, StorageError> {
        meta_i64(&self.conn, "commits")
    }

    /// Every item, with the count of writes they are as of, read in one
    /// snapshot.
    pub fn load(&self) -> Result<(i64, ItemMap), StorageError> {
        let tx = self.conn.unchecked_transaction()?;
        let writes = meta_i64(&tx, "writes")?;
        let mut rows = Vec::new();
        {
            let mut statement = tx.prepare_cached("SELECT id, body FROM items ORDER BY id")?;
            let mut found = statement.query([])?;
            while let Some(row) = found.next()? {
                rows.push((row.get::<_, u32>(0)?, row.get::<_, String>(1)?));
            }
        }
        tx.finish()?;
        Ok((writes, parse(rows)?))
    }

    /// Every item's JSON as stored, one a line, unparsed.
    pub fn text(&self) -> Result<String, StorageError> {
        Ok(self.conn.query_row("SELECT COALESCE(group_concat(body, char(10)), '') FROM (SELECT body FROM items ORDER BY id)", [], |row| row.get(0))?)
    }

    /// The counters, or both at zero before any write has set them.
    pub fn counters(&self) -> Result<Counters, StorageError> {
        let value: Option<String> = self.conn.query_row("SELECT value FROM meta WHERE key = 'counters'", [], |row| row.get(0)).optional()?;
        Ok(match value {
            Some(json) => serde_json::from_str(&json)?,
            None => Counters::default(),
        })
    }

    /// Every line of the journal, oldest first, behind `{"from": rev}` once
    /// it has been trimmed, as journal.jsonl held them.
    pub fn journal(&self) -> Result<Vec<Value>, StorageError> {
        let mut entries = Vec::new();
        let from: Option<i64> = self.conn.query_row("SELECT value FROM meta WHERE key = 'journal_from'", [], |row| row.get(0)).optional()?;
        if let Some(from) = from {
            entries.push(serde_json::json!({"from": from}));
        }
        let mut statement = self.conn.prepare_cached("SELECT entry FROM journal ORDER BY seq")?;
        let mut found = statement.query([])?;
        while let Some(row) = found.next()? {
            if let Ok(entry) = serde_json::from_str(&row.get::<_, String>(0)?) {
                entries.push(entry);
            }
        }
        Ok(entries)
    }

    /// The revision of the journal's last line, or 0 with none.
    pub fn last_journal_rev(&self) -> Result<u64, StorageError> {
        let rev: Option<i64> = self.conn.query_row("SELECT rev FROM journal ORDER BY seq DESC LIMIT 1", [], |row| row.get(0)).optional()?;
        Ok(rev.map_or(0, |rev| rev.max(0) as u64))
    }

    /// Puts one write in place, all of it or none: each item it changed, each
    /// change with the item before and after, the journal's line and the
    /// counters.
    pub fn write(&mut self, write: &Write) -> Result<Written, StorageError> {
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let layout: i64 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if layout > LAYOUT {
            return Err(StorageError::Refused(newer(layout)));
        }
        let commits = meta_i64(&tx, "commits")? + 1;
        set_meta(&tx, "commits", commits)?;
        let mut writes = meta_i64(&tx, "writes")?;
        let mut changed = false;
        for change in &write.changes {
            let before: Option<String> =
                tx.prepare_cached("SELECT body FROM items WHERE id = ?1")?.query_row([change.id], |row| row.get(0)).optional()?;
            let after = change.after.map(serde_json::to_string).transpose()?;
            if before == after {
                continue;
            }
            match &after {
                Some(body) => tx.prepare_cached("INSERT OR REPLACE INTO items (id, body) VALUES (?1, ?2)")?.execute(params![change.id, body])?,
                None => tx.prepare_cached("DELETE FROM items WHERE id = ?1")?.execute([change.id])?,
            };
            tx.prepare_cached("INSERT INTO changes (rev, at, id, before, after) VALUES (?1, ?2, ?3, ?4, ?5)")?
                .execute(params![write.rev as i64, write.at, change.id, before, after])?;
            changed = true;
        }
        if changed {
            writes += 1;
            set_meta(&tx, "writes", writes)?;
            prune_changes(&tx, write.at - CHANGES_KEPT_MS)?;
        }
        if let Some(entry) = write.journal {
            let rev = entry["rev"].as_i64().unwrap_or(write.rev as i64);
            tx.prepare_cached("INSERT INTO journal (rev, entry) VALUES (?1, ?2)")?.execute(params![rev, entry.to_string()])?;
            trim_journal(&tx, write.journal_bytes)?;
        }
        if let Some(counters) = write.counters {
            tx.prepare_cached("INSERT OR REPLACE INTO meta (key, value) VALUES ('counters', ?1)")?.execute([serde_json::to_string(counters)?])?;
        }
        tx.commit()?;
        Ok(Written { writes, commits })
    }

    /// Fills a new database from a board as storage.json, counters.json and
    /// journal.jsonl held it, in one transaction: the migration's rows.
    pub fn fill(&mut self, board: &ItemMap, counters: &Counters, journal: &[Value], revision: u64) -> Result<(), StorageError> {
        let tx = self.conn.transaction()?;
        {
            let mut insert = tx.prepare_cached("INSERT INTO items (id, body) VALUES (?1, ?2)")?;
            for (id, item) in board {
                insert.execute(params![id, serde_json::to_string(item)?])?;
            }
        }
        for entry in journal {
            match entry.get("from").and_then(Value::as_i64) {
                Some(from) => set_meta(&tx, "journal_from", from)?,
                None => {
                    let rev = entry["rev"].as_i64().unwrap_or(0);
                    tx.prepare_cached("INSERT INTO journal (rev, entry) VALUES (?1, ?2)")?.execute(params![rev, entry.to_string()])?;
                }
            }
        }
        tx.prepare_cached("INSERT OR REPLACE INTO meta (key, value) VALUES ('counters', ?1)")?.execute([serde_json::to_string(counters)?])?;
        // The board is whole from this revision on: the changes table starts
        // here, and a revision before it cannot be had back from it.
        set_meta(&tx, "changes_from", revision as i64)?;
        set_meta(&tx, "writes", 1)?;
        set_meta(&tx, "commits", 1)?;
        tx.commit()?;
        Ok(())
    }

    /// The board as it stood at revision `rev`: the items now, with every
    /// change made after `rev` undone, newest first. `None` where `rev` is
    /// older than the oldest revision the changes kept reach back to, or
    /// newer than the board.
    pub fn board_at(&self, rev: u64) -> Result<Option<ItemMap>, StorageError> {
        let tx = self.conn.unchecked_transaction()?;
        let from = meta_i64(&tx, "changes_from")?;
        let now: Counters = match tx.query_row("SELECT value FROM meta WHERE key = 'counters'", [], |row| row.get::<_, String>(0)).optional()? {
            Some(json) => serde_json::from_str(&json)?,
            None => Counters::default(),
        };
        if (rev as i64) < from || rev > now.revision {
            return Ok(None);
        }
        let mut rows = Vec::new();
        {
            let mut statement = tx.prepare_cached("SELECT id, body FROM items ORDER BY id")?;
            let mut found = statement.query([])?;
            while let Some(row) = found.next()? {
                rows.push((row.get::<_, u32>(0)?, row.get::<_, String>(1)?));
            }
        }
        let mut bodies: std::collections::BTreeMap<u32, String> = rows.into_iter().collect();
        {
            let mut statement = tx.prepare_cached("SELECT id, before FROM changes WHERE rev > ?1 ORDER BY seq DESC")?;
            let mut found = statement.query([rev as i64])?;
            while let Some(row) = found.next()? {
                let id: u32 = row.get(0)?;
                match row.get::<_, Option<String>>(1)? {
                    Some(before) => bodies.insert(id, before),
                    None => bodies.remove(&id),
                };
            }
        }
        tx.finish()?;
        Ok(Some(parse(bodies.into_iter().collect())?))
    }

    /// Checkpoints the write-ahead log into the file, so that the file
    /// alone is the whole database: before it is copied whole.
    pub fn checkpoint(&self) -> Result<(), StorageError> {
        self.conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
        Ok(())
    }

    /// Writes a whole copy of the database to `path`, which must not exist:
    /// one file, consistent as of one snapshot, whatever writes go on.
    pub fn copy_to(&self, path: &Path) -> Result<(), StorageError> {
        let target = path.to_str().ok_or_else(|| StorageError::Refused(format!("{} is no UTF-8 path", path.display())))?;
        self.conn.execute("VACUUM INTO ?1", [target])?;
        Ok(())
    }
}

/// What SQLite keeps beside a database in use, by suffix: the write-ahead
/// log, the log's index and the rollback journal.
pub const LOGS: [&str; 3] = ["-wal", "-shm", "-journal"];

/// `path` with `suffix` after it, as SQLite names the files beside a
/// database.
pub fn beside(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Puts the database at `temp`, which no connection has open, at `path` by
/// rename. Every database ekko puts at a path gets there this way: a board's
/// at its move out of JSON, the copy outside its folder, a board brought back
/// from that copy or moved (task 2233). The log and journal at `path` go
/// first: they belong to a file no longer there, and SQLite would apply them
/// to the one put in its place, one of the ways its documentation lists to
/// corrupt a database (sqlite.org/howtocorrupt.html, 1.4). A process still
/// holding the file that was at `path` keeps the log it has open, and its
/// close leaves the new file's log alone, as
/// `a_database_put_back_where_its_files_were_removed_is_read_from_the_new_one`
/// shows. A database some process has open must never be replaced, this way
/// or any other (ibid., 2.5): `path` holds none, or a copy no other process
/// opens.
pub fn put_in_place(temp: &Path, path: &Path) -> Result<(), StorageError> {
    File::open(temp)?.sync_all()?;
    for suffix in LOGS {
        match fs::remove_file(beside(path, suffix)) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error.into()),
            _ => {}
        }
    }
    fs::rename(temp, path)?;
    for suffix in LOGS {
        let _ = fs::remove_file(beside(temp, suffix));
    }
    if let Some(dir) = path.parent().and_then(|parent| File::open(parent).ok()) {
        let _ = dir.sync_all();
    }
    Ok(())
}

/// Copies the database at `from` to `to` through SQLite, never as a file: a
/// file copied while its log holds writes not yet checkpointed into it leaves
/// them behind, where VACUUM INTO reads them (sqlite.org/howtocorrupt.html,
/// 1.2). The copy has the write-ahead log on, as a board's database has, and
/// goes in by `put_in_place`.
pub fn copy(from: &Path, to: &Path) -> Result<(), StorageError> {
    let temp = beside(to, ".copying");
    for suffix in [""].into_iter().chain(LOGS) {
        let _ = fs::remove_file(beside(&temp, suffix));
    }
    let source = Database::open(from)?.ok_or_else(|| StorageError::Refused(format!("{} is gone", from.display())))?;
    source.copy_to(&temp)?;
    drop(source);
    let made = Database::open(&temp)?.ok_or_else(|| StorageError::Refused(format!("{} was not made", temp.display())))?;
    made.seal()?;
    drop(made);
    put_in_place(&temp, to)
}

/// Whether `name` is one of the files SQLite keeps beside the board's
/// database, which a copy of the board leaves out: `copy` reads what they
/// hold.
pub fn is_log(name: &std::ffi::OsStr) -> bool {
    LOGS.iter().any(|suffix| name.to_str() == Some(&format!("{FILE}{suffix}")))
}

/// What a build says of a database a later ekko laid out.
pub fn newer(layout: i64) -> String {
    format!(
        "This board's database has layout {layout}, from an ekko later than this one, which knows layout {LAYOUT} at most and writes nothing to it: upgrade ekko, then restart every Claude Code session"
    )
}

/// Parses each row's body into its item. Several threads share the rows on
/// a large board: a whole-board read is what every command does in this
/// stage, and one thread parsing row by row took 12-28% longer than the
/// single parse of storage.json (study 2207, note 2227).
fn parse(rows: Vec<(u32, String)>) -> Result<ItemMap, StorageError> {
    #[cfg(test)]
    crate::storage::count_parse();
    const ALONE: usize = 512;
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).clamp(1, 8);
    if rows.len() < ALONE || threads == 1 {
        let mut board = ItemMap::new();
        for (id, body) in rows {
            board.insert(id, serde_json::from_str::<Item>(&body)?);
        }
        return Ok(board);
    }
    let share = rows.len().div_ceil(threads);
    let parts: Vec<Result<Vec<(u32, Item)>, serde_json::Error>> = std::thread::scope(|scope| {
        let handles: Vec<_> = rows
            .chunks(share)
            .map(|part| scope.spawn(move || part.iter().map(|(id, body)| Ok((*id, serde_json::from_str::<Item>(body)?))).collect()))
            .collect();
        handles.into_iter().map(|handle| handle.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic))).collect()
    });
    let mut board = ItemMap::new();
    for part in parts {
        board.extend(part?);
    }
    Ok(board)
}

fn meta_i64(conn: &Connection, key: &str) -> Result<i64, StorageError> {
    Ok(conn.query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| row.get::<_, i64>(0)).optional()?.unwrap_or(0))
}

fn set_meta(conn: &Connection, key: &str, value: i64) -> Result<(), StorageError> {
    conn.prepare_cached("INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)")?.execute(params![key, value])?;
    Ok(())
}

/// Lets go of the changes made before `before`, and moves the oldest
/// revision the board can be had back at past them.
fn prune_changes(conn: &Connection, before: i64) -> Result<(), StorageError> {
    let pruned: Option<i64> = conn.query_row("SELECT MAX(rev) FROM changes WHERE at < ?1", [before], |row| row.get(0))?;
    if let Some(rev) = pruned {
        conn.execute("DELETE FROM changes WHERE at < ?1", [before])?;
        if rev > meta_i64(conn, "changes_from")? {
            set_meta(conn, "changes_from", rev)?;
        }
    }
    Ok(())
}

/// Past twice `bytes` of journal, keeps the newest lines within `bytes` and
/// records the oldest revision still described, as journal.jsonl's
/// `{"from": rev}` line did.
fn trim_journal(conn: &Connection, bytes: u64) -> Result<(), StorageError> {
    let total: i64 = conn.query_row("SELECT COALESCE(SUM(LENGTH(CAST(entry AS BLOB)) + 1), 0) FROM journal", [], |row| row.get(0))?;
    if total as u64 <= bytes * 2 {
        return Ok(());
    }
    let mut kept: Option<(i64, i64)> = None;
    {
        let mut statement = conn.prepare("SELECT seq, rev, LENGTH(CAST(entry AS BLOB)) + 1 FROM journal ORDER BY seq DESC")?;
        let mut found = statement.query([])?;
        let mut size = 0u64;
        while let Some(row) = found.next()? {
            size += row.get::<_, i64>(2)? as u64;
            if size > bytes {
                break;
            }
            kept = Some((row.get(0)?, row.get(1)?));
        }
    }
    match kept {
        Some((seq, rev)) => {
            conn.execute("DELETE FROM journal WHERE seq < ?1", [seq])?;
            set_meta(conn, "journal_from", rev)?;
        }
        None => {
            conn.execute("DELETE FROM journal", [])?;
            set_meta(conn, "journal_from", 0)?;
        }
    }
    Ok(())
}
