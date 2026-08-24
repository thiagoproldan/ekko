//! The task/note data model.
//!
//! JS taskbook modeled this as `Item` (base) with `Task`/`Note` subclasses.
//! Rust has no classical inheritance, and more importantly the two are
//! serialized to the *same* flat JSON shape on disk (a `Note` simply never
//! has `isComplete`/`inProgress`/`priority` at all, rather than having them
//! set to some "N/A" value) — so a single struct with `Option`al task-only
//! fields matches the wire format exactly, which matters: this needs to
//! read `storage.json`/`archive.json` files an existing JS install already
//! produced, unchanged.

use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::Local;
use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Item {
    #[serde(rename = "_id")]
    pub id: u32,
    #[serde(rename = "_date")]
    pub date: String,
    #[serde(rename = "_timestamp")]
    pub timestamp: i64,
    pub description: String,
    #[serde(rename = "isStarred", default)]
    pub is_starred: bool,
    #[serde(default)]
    pub boards: Vec<String>,
    #[serde(rename = "_isTask")]
    pub is_task: bool,

    // Task-only. `None` for notes, and omitted from the JSON entirely in
    // that case -- matching how the JS `Note` class never set these at all,
    // rather than setting them to some placeholder value.
    #[serde(rename = "isComplete", default, skip_serializing_if = "Option::is_none")]
    pub is_complete: Option<bool>,
    #[serde(rename = "inProgress", default, skip_serializing_if = "Option::is_none")]
    pub in_progress: Option<bool>,
    // Task-only, and absent from the JSON when unset -- so a file
    // written by taskbook, or by an Ekko that never saw a `d:` token,
    // round-trips byte-identically through this field.
    #[serde(rename = "dueDate", default, skip_serializing_if = "Option::is_none")]
    pub due_date: Option<String>,
    /// Stable across everything the display id is not: it survives
    /// `--restore` (which hands the item a fresh `_id`), and it is never
    /// recycled, whereas ids are `max + 1` and so get reused as soon as the
    /// highest-numbered item is deleted. Callers that hold a reference
    /// across time should hold this.
    ///
    /// `Option` because items written before this existed -- and any
    /// written by taskbook -- do not have one, and backfilling would
    /// rewrite files that are otherwise untouched. Absent means "legacy",
    /// not "unknown".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
    // Old data may have this stored as a JSON string (a bug in the JS
    // version's --priority path, fixed here rather than carried forward) --
    // still readable, but always written back out as a number now.
    #[serde(default, deserialize_with = "deserialize_priority", skip_serializing_if = "Option::is_none")]
    pub priority: Option<u8>,
}

impl Item {
    pub fn new_task(id: u32, description: String, boards: Vec<String>, priority: u8) -> Self {
        let (date, timestamp) = now();
        Item {
            id,
            date,
            timestamp,
            description,
            is_starred: false,
            boards,
            is_task: true,
            is_complete: Some(false),
            in_progress: Some(false),
            due_date: None,
            uid: Some(new_uid()),
            priority: Some(priority),
        }
    }

    pub fn new_note(id: u32, description: String, boards: Vec<String>) -> Self {
        let (date, timestamp) = now();
        Item {
            id,
            date,
            timestamp,
            description,
            is_starred: false,
            boards,
            is_task: false,
            is_complete: None,
            in_progress: None,
            priority: None,
            due_date: None,
            uid: Some(new_uid()),
        }
    }
}

/// `(_date, _timestamp)` for a freshly created item, computed *now* -- not
/// once at startup and reused. That was a real bug in the JS version: `now`
/// was a module-level `const`, so every item created within one long-lived
/// process shared a single frozen timestamp. Harmless for a CLI that starts
/// a new process per command, but wrong the moment this is used as a
/// library (as our own test suite does). Computing it fresh on every call
/// makes that bug structurally impossible here rather than merely fixed.
fn now() -> (String, i64) {
    let now = Local::now();
    // Matches JS `Date.prototype.toDateString()` exactly, e.g. "Sun Aug 23
    // 2026" -- zero-padded day, no leading zero suppression. Verified
    // against a real `new Date(...).toDateString()` call, not assumed.
    (now.format("%a %b %d %Y").to_string(), now.timestamp_millis())
}

fn deserialize_priority<'de, D>(deserializer: D) -> Result<Option<u8>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrNumber {
        Number(u8),
        String(String),
    }

    Ok(match Option::<StringOrNumber>::deserialize(deserializer)? {
        None => None,
        Some(StringOrNumber::Number(n)) => Some(n),
        Some(StringOrNumber::String(s)) => s.parse().ok(),
    })
}


/// Unique per item, and cheap: nanosecond clock plus pid, hex-encoded.
/// Same trick `storage::temp_file_path` already uses, and for the same
/// reason -- unique enough without pulling in a `rand` dependency. Two
/// items created back to back in one process get different nanos; two
/// processes racing get different pids.
fn new_uid() -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("{:x}-{:x}", nanos, process::id())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_for_clock_tick() {
        // `_timestamp` is millisecond-truncated, so waiting for *any*
        // nonzero `Instant` delta isn't enough -- wait for the millisecond
        // value itself to roll over, the same technique the JS regression
        // test used.
        let start = Local::now().timestamp_millis();
        while Local::now().timestamp_millis() == start {
            // Busy-wait for the millisecond to roll over.
        }
    }

    #[test]
    fn each_item_gets_its_own_timestamp() {
        let first = Item::new_task(1, "first".into(), vec![], 1);
        wait_for_clock_tick();
        let second = Item::new_task(2, "second".into(), vec![], 1);

        assert_ne!(first.timestamp, second.timestamp);
    }

    #[test]
    fn date_and_timestamp_agree() {
        let item = Item::new_task(1, "x".into(), vec![], 1);
        let from_timestamp = chrono::DateTime::from_timestamp_millis(item.timestamp)
            .unwrap()
            .with_timezone(&Local)
            .format("%a %b %d %Y")
            .to_string();

        assert_eq!(item.date, from_timestamp);
    }

    #[test]
    fn note_has_no_task_fields() {
        let note = Item::new_note(1, "a note".into(), vec!["@x".into()]);

        assert!(!note.is_task);
        assert_eq!(note.is_complete, None);
        assert_eq!(note.in_progress, None);
        assert_eq!(note.priority, None);

        let json = serde_json::to_value(&note).unwrap();
        assert!(json.get("isComplete").is_none());
        assert!(json.get("inProgress").is_none());
        assert!(json.get("priority").is_none());
    }

    #[test]
    fn task_serializes_with_the_exact_js_field_names() {
        let task = Item::new_task(7, "ship it".into(), vec!["@coding".into()], 2);
        let json = serde_json::to_value(&task).unwrap();

        assert_eq!(json["_id"], 7);
        assert_eq!(json["description"], "ship it");
        assert_eq!(json["isStarred"], false);
        assert_eq!(json["boards"], serde_json::json!(["@coding"]));
        assert_eq!(json["_isTask"], true);
        assert_eq!(json["isComplete"], false);
        assert_eq!(json["inProgress"], false);
        assert_eq!(json["priority"], 2);
    }

    #[test]
    fn reads_legacy_string_priority_and_legacy_number_priority_alike() {
        let string_priority = serde_json::json!({
            "_id": 1, "_date": "Sun Aug 23 2026", "_timestamp": 0,
            "description": "x", "isStarred": false, "boards": [],
            "_isTask": true, "isComplete": false, "inProgress": false,
            "priority": "3"
        });
        let number_priority = serde_json::json!({
            "_id": 2, "_date": "Sun Aug 23 2026", "_timestamp": 0,
            "description": "x", "isStarred": false, "boards": [],
            "_isTask": true, "isComplete": false, "inProgress": false,
            "priority": 1
        });

        let a: Item = serde_json::from_value(string_priority).unwrap();
        let b: Item = serde_json::from_value(number_priority).unwrap();

        assert_eq!(a.priority, Some(3));
        assert_eq!(b.priority, Some(1));
    }

    #[test]
    fn reads_a_real_note_produced_by_the_js_version_unchanged() {
        // A verbatim row copied from a JS-produced archive.json during
        // this project's own dogfooding session.
        let raw = serde_json::json!({
            "_id": 1,
            "_date": "Sun Aug 23 2026",
            "_timestamp": 1787531257957_i64,
            "description": "A reference note",
            "isStarred": false,
            "boards": ["@coding"],
            "_isTask": false
        });

        let note: Item = serde_json::from_value(raw).unwrap();

        assert_eq!(note.id, 1);
        assert!(!note.is_task);
        assert_eq!(note.is_complete, None);
    }
}
