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
        Outcome::Check { checked, unchecked, overridden, reopened } => with_overrides(
            json!({"ok": true, "command": command, "checked": checked, "unchecked": unchecked}),
            overridden,
            reopened,
        ),
        Outcome::Begin { started, paused } => {
            json!({"ok": true, "command": command, "started": started, "paused": paused})
        }
        Outcome::Star { starred, unstarred } => {
            json!({"ok": true, "command": command, "starred": starred, "unstarred": unstarred})
        }
        Outcome::Set { ids, settings, overridden, reopened } => with_overrides(
            json!({"ok": true, "command": command, "ids": ids, "states": settings.iter().map(|s| s.word()).collect::<Vec<_>>()}),
            overridden,
            reopened,
        ),
        Outcome::Delete(items) => json!({"ok": true, "command": command, "items": items}),
        Outcome::Restore(items) => json!({"ok": true, "command": command, "items": items}),
        Outcome::Edit(item)
        | Outcome::Answered(item)
        | Outcome::Move(item)
        | Outcome::Priority(item)
        | Outcome::With(item) => {
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
        Outcome::Projects(projects) => json!({"ok": true, "command": command, "projects": projects}),
        Outcome::Destroyed { name, tasks, notes, trash } => json!({
            "ok": true,
            "command": command,
            "project": name,
            "tasks": tasks,
            "notes": notes,
            // Absolute, because it is the only way back and a caller has no
            // other way to work out where the project went.
            "trash": trash.display().to_string(),
        }),
        Outcome::Init(init) => json!({
            "ok": true,
            "command": command,
            "project": {"name": init.name, "id": init.id, "root": init.root.display().to_string()},
            "existing": init.existing,
            "claimed": init.claimed,
            "adopted": init.adopted.as_ref().map(|adopted| json!({
                "from": adopted.from.display().to_string(),
                "parked": adopted.parked.display().to_string(),
            })),
            "excluded": init.excluded,
        }),
        Outcome::Phases(names) => json!({"ok": true, "command": command, "phases": names}),
        Outcome::Stashed { ids, away } | Outcome::Trashed { ids, away } => {
            json!({"ok": true, "command": command, "ids": ids, "away": away})
        }
        Outcome::Stash(groups) => {
            json!({"ok": true, "command": command, "boards": groups_to_value(groups)})
        }
        Outcome::Trash(items) => json!({"ok": true, "command": command, "items": items}),
        Outcome::Attached { item, target } => {
            json!({"ok": true, "command": command, "item": item, "target": target})
        }
        Outcome::Blocked { item, blockers } => {
            json!({"ok": true, "command": command, "item": item, "blockers": blockers})
        }
        Outcome::Roadmap { steps, rootless, inversions } => {
            let mut value = json!({"ok": true, "command": command, "steps": steps, "rootless": rootless});
            if !inversions.is_empty() {
                if let Value::Object(map) = &mut value {
                    map.insert("inversions".to_string(), json!(inversions));
                }
            }
            value
        }
        Outcome::Stats(stats) => json!({"ok": true, "command": command, "stats": stats}),
        Outcome::Prime(prime) => json!({"ok": true, "command": command, "prime": prime}),
        Outcome::Sessions(sessions) => json!({"ok": true, "command": command, "sessions": sessions}),
        Outcome::Hook(text) => json!({"ok": true, "command": command, "text": text}),
        Outcome::Next(entries) => json!({"ok": true, "command": command, "items": entries}),
        Outcome::Context(context) => json!({"ok": true, "command": command, "context": context}),
    }
}

fn error_value(error: &EkkoError) -> Value {
    let mut value = json!({"ok": false, "error": error.to_string(), "code": error.code()});
    let extra = match error {
        EkkoError::InvalidId(id) => Some(("id", json!(id))),
        EkkoError::InvalidCustomAppDir(path) => Some(("path", json!(path))),
        EkkoError::LockTimeout(path, _) => Some(("path", json!(path))),
        EkkoError::RenamedFlag { new, .. } => Some(("renamedTo", json!(new))),
        EkkoError::Blocked(blocked) => Some(("blocked", pairs_value(blocked, "blockers"))),
        EkkoError::CompletedDependents(found) => Some(("dependents", pairs_value(found, "dependents"))),
        EkkoError::AlreadyDone(found) => Some(("blocked", pairs_value(found, "blockers"))),
        EkkoError::Stale { current, .. } => Some(("current", json!(current))),
        EkkoError::EditMatch { found, .. } => Some(("found", json!(found))),
        EkkoError::PhaseOrder(inversion) => Some(("inversion", json!(inversion))),
        _ => None,
    };
    if let (Some((key, val)), Value::Object(map)) = (extra, &mut value) {
        map.insert(key.to_string(), val);
    }
    value
}

/// Adds `overridden` to a completion's reply, and only when `--force`
/// actually overrode something -- so every reply that forced nothing stays
/// exactly what it was before the key existed.
fn with_overrides(mut value: Value, overridden: &[(u32, Vec<u32>)], reopened: &[(u32, Vec<u32>)]) -> Value {
    if let Value::Object(map) = &mut value {
        if !overridden.is_empty() {
            map.insert("overridden".to_string(), pairs_value(overridden, "blockers"));
        }
        if !reopened.is_empty() {
            map.insert("reopenedOver".to_string(), pairs_value(reopened, "dependents"));
        }
    }
    value
}

/// `[(id, others), ...]` -> `[{"id": 2, "<key>": [1]}, ...]`: named fields
/// rather than bare pairs, so a caller never has to know which half of a
/// tuple is which.
fn pairs_value(pairs: &[(u32, Vec<u32>)], key: &str) -> Value {
    Value::Array(pairs.iter().map(|(id, others)| json!({"id": id, key: others})).collect())
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
