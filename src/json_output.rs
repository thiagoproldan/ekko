//! Builds the exact `--json` envelope this project settled on while still
//! JavaScript: `{"ok":true,"command":...,...fields}` on success,
//! `{"ok":false,"error":...,"code":...,...extra}` on failure. One line of
//! compact JSON per `print_*` call -- callers that need to emit more than
//! one (the default board view + stats, `--timeline`, `--list`) just call
//! it more than once, same newline-delimited-JSON contract documented for
//! the JS version.

use serde_json::{json, Value};

use crate::ekko::{EkkoError, Outcome};
use crate::item::Item;

pub fn print_success(outcome: &Outcome) {
    println!("{}", success_value(outcome));
}

pub fn print_error(error: &EkkoError) {
    println!("{}", error_value(error));
}

fn success_value(outcome: &Outcome) -> Value {
    let command = outcome.command_name();
    match outcome {
        Outcome::Task(item) | Outcome::Note(item) => json!({"ok": true, "command": command, "item": item}),
        Outcome::Check { checked, unchecked } => {
            json!({"ok": true, "command": command, "checked": checked, "unchecked": unchecked})
        }
        Outcome::Begin { started, paused } => {
            json!({"ok": true, "command": command, "started": started, "paused": paused})
        }
        Outcome::Star { starred, unstarred } => {
            json!({"ok": true, "command": command, "starred": starred, "unstarred": unstarred})
        }
        Outcome::Set { ids, states } => {
            json!({"ok": true, "command": command, "ids": ids, "states": states})
        }
        Outcome::Delete(items) => json!({"ok": true, "command": command, "items": items}),
        Outcome::Restore(items) => json!({"ok": true, "command": command, "items": items}),
        Outcome::Edit(item) | Outcome::Move(item) | Outcome::Priority(item) => {
            json!({"ok": true, "command": command, "item": item})
        }
        Outcome::Copy { ids, descriptions } => {
            json!({"ok": true, "command": command, "ids": ids, "descriptions": descriptions})
        }
        Outcome::Board(groups) | Outcome::Find(groups) | Outcome::List(groups) => {
            json!({"ok": true, "command": command, "boards": groups_to_value(groups)})
        }
        Outcome::Timeline(groups) | Outcome::Archive(groups) => {
            json!({"ok": true, "command": command, "dates": groups_to_value(groups)})
        }
        Outcome::Projects(names) => json!({"ok": true, "command": command, "projects": names}),
        Outcome::Phases(names) => json!({"ok": true, "command": command, "phases": names}),
        Outcome::Path { steps, rootless } => {
            json!({"ok": true, "command": command, "steps": steps, "rootless": rootless})
        }
        Outcome::Stats(stats) => json!({"ok": true, "command": command, "stats": stats}),
    }
}

fn error_value(error: &EkkoError) -> Value {
    let mut value = json!({"ok": false, "error": error.to_string(), "code": error.code()});
    let extra = match error {
        EkkoError::InvalidId(id) => Some(("id", json!(id))),
        EkkoError::InvalidCustomAppDir(path) => Some(("path", json!(path))),
        EkkoError::LockTimeout(path) => Some(("path", json!(path))),
        _ => None,
    };
    if let (Some((key, val)), Value::Object(map)) = (extra, &mut value) {
        map.insert(key.to_string(), val);
    }
    value
}

/// `[(name, items), ...]` -> `{"name": [items], ...}`, preserving the
/// exact order `groups` was built in (board/date discovery order) --
/// requires serde_json's `preserve_order` feature, since its `Map`
/// defaults to alphabetical (`BTreeMap`-backed) otherwise.
fn groups_to_value(groups: &[(String, Vec<Item>)]) -> Value {
    let mut map = serde_json::Map::new();
    for (key, items) in groups {
        map.insert(key.clone(), serde_json::to_value(items).expect("Item serialization cannot fail"));
    }
    Value::Object(map)
}
