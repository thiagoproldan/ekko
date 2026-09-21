//! Structured writes: the operations an agent sends, applied to a draft of
//! the board and written once.
//!
//! The CLI reads words out of the description it is given -- `@board`, `p:2`,
//! `d:2026-09-01` -- which is right at a prompt and wrong for a caller writing
//! prose: a note that mentions `@mentions` or `p:3` would lose those words to
//! board names and priorities. These take every field separately and never
//! look inside the text.
//!
//! Every operation lands on a `Draft`, a copy of the board taken under the
//! storage lock, and nothing reaches the disk until `commit`. So a batch of
//! twenty operations is one write, the dependency rule is checked once on the
//! board the whole batch would leave, and any refusal leaves the file exactly
//! as it was.

use serde::{Deserialize, Deserializer};
use serde_json::{json, Value};

use crate::ekko::{
    apply_state, canonical_state, holds, parse_due_date, phase_inversion, phase_order, remove_duplicates,
    uid_index, Ekko, EkkoError, Linked,
};
use crate::item::Item;
use crate::storage::{ItemMap, LockGuard};

/// An item as a caller names it: a display id, a uid, or `$N` for the item
/// the N-th operation of the same batch created. Numbers and strings both.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Ref {
    Id(u32),
    Text(String),
}

impl Ref {
    fn text(&self) -> String {
        match self {
            Ref::Id(id) => id.to_string(),
            Ref::Text(text) => text.trim_start_matches('@').to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Task,
    Note,
    /// A note that hands a task over to the next session; see `Item::handoff`.
    Handoff,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Create {
    #[serde(default)]
    pub kind: Kind,
    pub text: String,
    #[serde(default)]
    pub boards: Vec<String>,
    pub priority: Option<u8>,
    pub due: Option<String>,
    pub phase: Option<String>,
    #[serde(default)]
    pub blocked_by: Vec<Ref>,
    pub attached_to: Option<Ref>,
    #[serde(default)]
    pub starred: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Edit {
    pub item: Ref,
    /// The whole new description.
    pub text: Option<String>,
    /// Replace one exact occurrence of `old` with `new`.
    pub replace: Option<Replace>,
    /// Add to the end of the description.
    pub append: Option<String>,
    /// Refuse the edit (STALE) unless the item still has this `updatedAt`.
    pub if_updated_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Replace {
    pub old: String,
    pub new: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Update {
    pub item: Ref,
    pub boards: Option<Vec<String>>,
    pub priority: Option<u8>,
    /// A date sets it, `null` clears it, absent leaves it.
    #[serde(default, deserialize_with = "present")]
    pub due: Option<Option<String>>,
    /// A declared phase moves the item there, `null` to the project root.
    #[serde(default, deserialize_with = "present")]
    pub phase: Option<Option<String>>,
    pub starred: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Link {
    pub item: Ref,
    /// Replaces what the item is blocked by; empty clears it.
    pub blocked_by: Option<Vec<Ref>>,
    /// The task a note explains; `null` detaches it.
    #[serde(default, deserialize_with = "present")]
    pub attached_to: Option<Option<Ref>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetState {
    pub items: Vec<Ref>,
    pub state: String,
}

/// One operation of a batch, tagged by `op`.
#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    Create(Create),
    SetState(SetState),
    Edit(Edit),
    Update(Update),
    Link(Link),
}

/// Tells a field given as `null` from a field left out: `Some(None)` for the
/// first, and `None`, through `default`, for the second.
fn present<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

fn invalid(message: impl Into<String>) -> EkkoError {
    EkkoError::InvalidInput(message.into())
}

/// `["coding", "@reviews"]` to `["@coding", "@reviews"]`, the default board
/// by either of its names, and the default board when there are none.
fn boards(names: &[String]) -> Vec<String> {
    let named: Vec<String> = names
        .iter()
        .map(|name| name.trim())
        .filter(|name| !name.is_empty())
        .map(|name| match name.trim_start_matches('@') {
            "myboard" | "My Board" => "My Board".to_string(),
            bare => format!("@{bare}"),
        })
        .collect();
    if named.is_empty() { vec!["My Board".to_string()] } else { remove_duplicates(named) }
}

/// How a refusal names the items it involves.
///
/// An item already on the board is its display id. One this draft created
/// exists nowhere -- a refusal writes nothing -- so the id it held in the
/// draft would name nothing, or worse, whatever takes that number next. It is
/// named by the operation that created it instead, the way `$N` refers to it.
pub struct Names {
    existing: std::collections::HashSet<u32>,
    created: Vec<Option<u32>>,
}

impl Names {
    pub fn name(&self, id: u32) -> String {
        if self.existing.contains(&id) {
            return id.to_string();
        }
        match self.created.iter().position(|created| *created == Some(id)) {
            Some(at) => format!("${} (from operation {})", at + 1, at + 1),
            None => "the new item".to_string(),
        }
    }
}

/// A board being written: the lock, what storage held when it was taken, and
/// the copy every operation changes.
pub struct Draft<'a> {
    ekko: &'a Ekko,
    _lock: LockGuard<'a>,
    before: ItemMap,
    data: ItemMap,
    phases: Vec<String>,
    /// For each operation applied so far, the item it created, if any --
    /// what `$N` resolves through.
    created: Vec<Option<u32>>,
}

/// What a commit wrote, what `force` pushed past to write it, and the tasks
/// whose readiness it changed.
pub struct Committed {
    pub data: ItemMap,
    pub overridden: Linked,
    pub reopened: Linked,
    /// Tasks that waited on open work before the write and no longer do.
    pub released: Vec<u32>,
    /// Tasks that were free to start before the write and now wait.
    pub blocked: Vec<u32>,
}

impl<'a> Draft<'a> {
    pub fn open(ekko: &'a Ekko) -> Result<Self, EkkoError> {
        let lock = ekko.storage.acquire_lock()?;
        let data = ekko.storage.get()?;
        let phases = ekko.storage.get_phases()?;
        Ok(Draft { ekko, _lock: lock, before: data.clone(), data, phases, created: Vec::new() })
    }

    /// How a refusal of this draft should name its items; see `Names`.
    pub fn names(&self) -> Names {
        Names { existing: self.before.keys().copied().collect(), created: self.created.clone() }
    }

    fn resolve(&self, reference: &Ref) -> Result<u32, EkkoError> {
        let raw = reference.text();
        if let Some(position) = raw.strip_prefix('$') {
            let op: usize = position
                .parse()
                .map_err(|_| invalid(format!("{raw} is not a reference: $1 is the item the first operation created")))?;
            return op
                .checked_sub(1)
                .and_then(|at| self.created.get(at).copied().flatten())
                .ok_or_else(|| invalid(format!("{raw} does not name an item created earlier in this batch")));
        }
        Ok(self.ekko.validate_ids(&[raw], &self.data)?[0])
    }

    fn resolve_all(&self, references: &[Ref]) -> Result<Vec<u32>, EkkoError> {
        let mut ids = Vec::new();
        for reference in references {
            let id = self.resolve(reference)?;
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        Ok(ids)
    }

    fn item(&mut self, id: u32) -> &mut Item {
        self.data.get_mut(&id).expect("ids are resolved against the draft")
    }

    /// Applies one operation to the draft, returning the ids it touched.
    pub fn apply(&mut self, op: &Op) -> Result<Vec<u32>, EkkoError> {
        let result = match op {
            Op::Create(spec) => self.create(spec).map(|id| vec![id]),
            Op::SetState(spec) => self.set_state(&spec.items, &spec.state),
            Op::Edit(spec) => self.edit(spec),
            Op::Update(spec) => self.update(spec),
            Op::Link(spec) => self.link(spec),
        };
        let created = match (op, &result) {
            (Op::Create(_), Ok(ids)) => ids.first().copied(),
            _ => None,
        };
        self.created.push(created);
        result
    }

    pub fn create(&mut self, spec: &Create) -> Result<u32, EkkoError> {
        let text = spec.text.trim();
        if text.is_empty() {
            return Err(EkkoError::MissingDesc);
        }
        if spec.kind != Kind::Task {
            for (field, given) in [
                ("priority", spec.priority.is_some()),
                ("due date", spec.due.is_some()),
                ("blocked_by", !spec.blocked_by.is_empty()),
            ] {
                if given {
                    return Err(invalid(format!("A note has no {field}; only a task does")));
                }
            }
        }
        if spec.kind == Kind::Handoff && spec.attached_to.is_none() {
            return Err(invalid("A handoff is attached_to the task it hands over"));
        }
        let priority = spec.priority.unwrap_or(1);
        if !(1..=3).contains(&priority) {
            return Err(EkkoError::InvalidPriority);
        }
        let due = match &spec.due {
            Some(date) => Some(parse_due_date(&format!("d:{date}")).ok_or_else(|| EkkoError::InvalidDueDate(date.clone()))?),
            None => None,
        };
        let phase = self.declared_phase(spec.phase.as_deref())?;

        let id = self.ekko.generate_id(&self.data);
        let mut item = match spec.kind {
            Kind::Task => Item::new_task(id, text.to_string(), boards(&spec.boards), priority),
            Kind::Note | Kind::Handoff => Item::new_note(id, text.to_string(), boards(&spec.boards)),
        };
        item.due_date = due;
        item.phase = phase;
        item.is_starred = spec.starred;
        self.data.insert(id, item);

        // Relations after the item is in the draft: both checks read it there.
        if !spec.blocked_by.is_empty() {
            let blockers = self.resolve_all(&spec.blocked_by)?;
            let uids = self.ekko.blocker_uids(&self.data, &self.phases, id, &blockers)?;
            self.item(id).blocked_by = Some(uids);
        }
        if let Some(target) = &spec.attached_to {
            let target = self.resolve(target)?;
            let attached = Ekko::attach_target(&self.data, id, Some(target))?;
            self.item(id).attached_to = attached.map(|(_, uid)| uid);
            if spec.kind == Kind::Handoff {
                self.hand_over(id, target)?;
            }
        }
        Ok(id)
    }

    /// Makes note `id` the handoff of `task`: an open task only, since a
    /// finished one has nothing left to hand over, and the only one there --
    /// the handoff it replaces stays on the task as an ordinary note.
    fn hand_over(&mut self, id: u32, task: u32) -> Result<(), EkkoError> {
        if !holds(&self.data[&task]) {
            return Err(invalid(format!("{task} is finished, and a finished task has nothing to hand over")));
        }
        let uid = self.data[&task].uid.clone();
        let earlier: Vec<u32> = self
            .data
            .values()
            .filter(|note| note.handoff && note.id != id && note.attached_to.is_some() && note.attached_to == uid)
            .map(|note| note.id)
            .collect();
        for note in earlier {
            self.item(note).handoff = false;
        }
        self.item(id).handoff = true;
        Ok(())
    }

    fn declared_phase(&self, phase: Option<&str>) -> Result<Option<String>, EkkoError> {
        match phase {
            None => Ok(None),
            Some(name) if self.phases.iter().any(|declared| declared == name) => Ok(Some(name.to_string())),
            Some(name) => Err(invalid(if self.phases.is_empty() {
                format!("Unknown phase {name}: this board declares no phases")
            } else {
                format!("Unknown phase {name}: the declared phases are {}", self.phases.join(", "))
            })),
        }
    }

    pub fn set_state(&mut self, items: &[Ref], state: &str) -> Result<Vec<u32>, EkkoError> {
        let canonical = canonical_state(state).ok_or_else(|| EkkoError::UnknownState(state.to_string()))?;
        if items.is_empty() {
            return Err(EkkoError::MissingId);
        }
        let ids = self.resolve_all(items)?;
        // The terminal skips task states on notes quietly, as taskbook did; a
        // structured write that changed nothing would still answer ok.
        if !matches!(canonical, "starred" | "unstarred") {
            if let Some(note) = ids.iter().find(|id| !self.data[*id].is_task) {
                return Err(invalid(format!(
                    "{note} is a note, and a note has no state; of the states only starred and unstarred apply to it"
                )));
            }
        }
        for id in &ids {
            apply_state(self.item(*id), canonical);
        }
        Ok(ids)
    }

    pub fn edit(&mut self, spec: &Edit) -> Result<Vec<u32>, EkkoError> {
        let id = self.resolve(&spec.item)?;
        let item = self.item(id);

        // Against the version the caller read, which is the version before
        // this batch: the stamp only moves when the batch is written.
        if let Some(expected) = spec.if_updated_at {
            let current = item.updated_at.unwrap_or(item.timestamp);
            if current != expected {
                return Err(EkkoError::Stale { id, current });
            }
        }

        let description = match (&spec.text, &spec.replace, &spec.append) {
            (Some(text), None, None) => text.clone(),
            (None, Some(replace), None) => {
                if replace.old.is_empty() {
                    return Err(invalid("replace.old is empty, so there is nothing to find"));
                }
                let found = item.description.matches(replace.old.as_str()).count();
                if found != 1 {
                    return Err(EkkoError::EditMatch { id, found });
                }
                item.description.replacen(&replace.old, &replace.new, 1)
            }
            (None, None, Some(more)) => {
                let joins = item.description.ends_with(char::is_whitespace) || more.starts_with(char::is_whitespace);
                if joins { format!("{}{more}", item.description) } else { format!("{} {more}", item.description) }
            }
            _ => return Err(invalid("An edit takes exactly one of text, replace or append")),
        };
        if description.trim().is_empty() {
            return Err(EkkoError::MissingDesc);
        }
        item.description = description;
        Ok(vec![id])
    }

    pub fn update(&mut self, spec: &Update) -> Result<Vec<u32>, EkkoError> {
        let id = self.resolve(&spec.item)?;
        if spec.boards.is_none() && spec.priority.is_none() && spec.due.is_none() && spec.phase.is_none() && spec.starred.is_none() {
            return Err(invalid("An update needs at least one of boards, priority, due, phase or starred"));
        }
        let is_task = self.data[&id].is_task;

        if let Some(names) = &spec.boards {
            if names.iter().all(|name| name.trim().is_empty()) {
                return Err(EkkoError::MissingBoards);
            }
            self.item(id).boards = boards(names);
        }
        if let Some(priority) = spec.priority {
            if !is_task {
                return Err(invalid("A note has no priority; only a task does"));
            }
            if !(1..=3).contains(&priority) {
                return Err(EkkoError::InvalidPriority);
            }
            self.item(id).priority = Some(priority);
        }
        if let Some(due) = &spec.due {
            let parsed = match due {
                Some(_) if !is_task => return Err(invalid("A note has no due date; only a task does")),
                Some(date) => Some(parse_due_date(&format!("d:{date}")).ok_or_else(|| EkkoError::InvalidDueDate(date.clone()))?),
                None => None,
            };
            self.item(id).due_date = parsed;
        }
        if let Some(phase) = &spec.phase {
            let phase = self.declared_phase(phase.as_deref())?;
            self.item(id).phase = phase;
            self.refuse_phase_inversions(id)?;
        }
        if let Some(starred) = spec.starred {
            self.item(id).is_starred = starred;
        }
        Ok(vec![id])
    }

    /// Moving an item between phases can turn its dependencies, in either
    /// direction, against the phase order -- refused the way `--blocked-by`
    /// refuses making one.
    fn refuse_phase_inversions(&self, id: u32) -> Result<(), EkkoError> {
        let order = phase_order(&self.phases);
        let index = uid_index(&self.data);
        let item = &self.data[&id];
        for uid in item.blocked_by.iter().flatten() {
            if let Some(blocker) = index.get(uid.as_str()).and_then(|b| self.data.get(b)) {
                if let Some(inversion) = phase_inversion(&order, item, blocker) {
                    return Err(EkkoError::PhaseOrder(inversion));
                }
            }
        }
        let Some(uid) = item.uid.as_deref() else { return Ok(()) };
        for dependent in self.data.values() {
            if dependent.blocked_by.iter().flatten().any(|b| b == uid) {
                if let Some(inversion) = phase_inversion(&order, dependent, item) {
                    return Err(EkkoError::PhaseOrder(inversion));
                }
            }
        }
        Ok(())
    }

    pub fn link(&mut self, spec: &Link) -> Result<Vec<u32>, EkkoError> {
        let id = self.resolve(&spec.item)?;
        match (&spec.blocked_by, &spec.attached_to) {
            (Some(blockers), None) => {
                let blockers = self.resolve_all(blockers)?;
                let uids = self.ekko.blocker_uids(&self.data, &self.phases, id, &blockers)?;
                self.item(id).blocked_by = if uids.is_empty() { None } else { Some(uids) };
            }
            (None, Some(target)) => {
                let target = target.as_ref().map(|t| self.resolve(t)).transpose()?;
                let attached = Ekko::attach_target(&self.data, id, target)?;
                let moved = self.data[&id].attached_to != attached.as_ref().map(|(_, uid)| uid.clone());
                self.item(id).attached_to = attached.map(|(_, uid)| uid);
                // A handoff hands over the task it was written on; moved or
                // detached, it is an ordinary note about that work.
                if moved {
                    self.item(id).handoff = false;
                }
            }
            _ => return Err(invalid("A link takes exactly one of blocked_by or attached_to")),
        }
        Ok(vec![id])
    }

    /// Writes the draft, once, if the board it leaves keeps the dependency
    /// rule -- or, with `force`, anyway, saying what it pushed past.
    pub fn commit(mut self, force: bool) -> Result<Committed, EkkoError> {
        let (overridden, reopened) = Ekko::refuse_broken_dependencies(&self.before, &self.data, force)?;
        let (released, blocked) = self.ekko.save_against(&self.before, &mut self.data)?;
        Ok(Committed { data: self.data, overridden, reopened, released, blocked })
    }
}

/// How a written item is reported back: enough to act on it again safely --
/// the uid to hold, and the `updatedAt` a later edit can be conditioned on.
pub fn written(data: &ItemMap, id: u32) -> Value {
    match data.get(&id) {
        Some(item) => json!({"id": id, "uid": item.uid, "updatedAt": item.updated_at}),
        None => json!({"id": id}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::State;
    use crate::storage::Storage;
    use std::path::PathBuf;

    fn board(tag: &str) -> (Ekko, PathBuf) {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("ekko-ops-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        (Ekko::new(Storage::new(&dir).unwrap()), dir)
    }

    fn op(value: Value) -> Op {
        serde_json::from_value(value).expect("a valid operation")
    }

    /// Run `ops` as one batch and commit it, the way a caller would.
    fn batch(ekko: &Ekko, ops: &[Value]) -> Result<Committed, EkkoError> {
        let mut draft = Draft::open(ekko)?;
        for value in ops {
            draft.apply(&op(value.clone()))?;
        }
        draft.commit(false)
    }

    /// Nothing in the text is read as a board, a priority or a date: the words
    /// the CLI would have taken out are exactly the ones prose uses.
    #[test]
    fn the_text_is_kept_whole_and_every_field_comes_separately() {
        let (ekko, dir) = board("create");
        let text = "mention @someone, rate it p:3, and d:2026-01-01 is just a date";
        let written = batch(
            &ekko,
            &[json!({"op": "create", "text": text, "boards": ["coding", "@reviews"], "priority": 2, "due": "2026-9-1"})],
        )
        .unwrap();

        let item = &written.data[&1];
        assert_eq!(item.description, text);
        assert_eq!(item.boards, vec!["@coding", "@reviews"]);
        assert_eq!((item.priority, item.due_date.as_deref()), (Some(2), Some("2026-09-01")));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A batch can build a small plan in one write: later operations name the
    /// items earlier ones created, by position.
    #[test]
    fn a_batch_refers_to_what_it_created_and_writes_once() {
        let (ekko, dir) = board("refs");
        let written = batch(
            &ekko,
            &[
                json!({"op": "create", "text": "design"}),
                json!({"op": "create", "text": "build", "blocked_by": ["$1"]}),
                json!({"op": "create", "kind": "note", "text": "why build waits", "attached_to": "$2"}),
                json!({"op": "set_state", "items": ["$1"], "state": "done"}),
            ],
        )
        .unwrap();

        let data = &written.data;
        assert_eq!(data[&2].blocked_by, Some(vec![data[&1].uid.clone().unwrap()]));
        assert_eq!(data[&3].attached_to, data[&2].uid);
        assert_eq!(State::of(&data[&1]), Some(State::Done));
        assert_eq!(ekko.storage.get().unwrap(), *data, "what was reported is not what was written");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A handoff is a note on an open task, and the newest one is the only
    /// handoff there: the one it replaces stays on the task as a plain note.
    #[test]
    fn a_handoff_replaces_the_one_before_on_its_task() {
        let (ekko, dir) = board("handoff");
        batch(
            &ekko,
            &[
                json!({"op": "create", "text": "the work"}),
                json!({"op": "create", "kind": "handoff", "text": "stopped at the parser", "attached_to": "$1"}),
            ],
        )
        .unwrap();
        let written = batch(&ekko, &[json!({"op": "create", "kind": "handoff", "text": "parser done, tests next", "attached_to": 1})]).unwrap();

        let data = &written.data;
        assert!(!data[&3].is_task && data[&3].handoff);
        assert_eq!(data[&3].attached_to, data[&1].uid);
        assert!(!data[&2].handoff, "the earlier handoff is demoted");
        assert_eq!(data[&2].attached_to, data[&1].uid, "and kept on the task");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Nothing to hand over without a task, or on one already finished.
    #[test]
    fn a_handoff_needs_an_open_task() {
        let (ekko, dir) = board("handoff-refused");
        batch(&ekko, &[json!({"op": "create", "text": "finished work"}), json!({"op": "set_state", "items": [1], "state": "done"})])
            .unwrap();

        let loose = batch(&ekko, &[json!({"op": "create", "kind": "handoff", "text": "where I stopped"})]);
        assert!(matches!(loose, Err(EkkoError::InvalidInput(ref m)) if m.contains("attached_to")), "{:?}", loose.as_ref().err());
        let finished = batch(&ekko, &[json!({"op": "create", "kind": "handoff", "text": "where I stopped", "attached_to": 1})]);
        assert!(matches!(finished, Err(EkkoError::InvalidInput(ref m)) if m.contains("finished")), "{:?}", finished.as_ref().err());
        assert_eq!(ekko.storage.get().unwrap().len(), 1, "a refused handoff wrote nothing");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A handoff hands over the task it was written on: moved to another task
    /// or detached, it is an ordinary note.
    #[test]
    fn a_moved_handoff_is_an_ordinary_note() {
        let (ekko, dir) = board("handoff-moved");
        batch(
            &ekko,
            &[
                json!({"op": "create", "text": "first"}),
                json!({"op": "create", "text": "second"}),
                json!({"op": "create", "kind": "handoff", "text": "where I stopped", "attached_to": "$1"}),
            ],
        )
        .unwrap();
        let moved = batch(&ekko, &[json!({"op": "link", "item": 3, "attached_to": 2})]).unwrap();
        assert!(!moved.data[&3].handoff);
        assert_eq!(moved.data[&3].attached_to, moved.data[&2].uid);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// All or nothing: a refusal anywhere in the batch, including the
    /// dependency rule checked on the board the batch would leave, writes
    /// nothing at all.
    #[test]
    fn a_refused_batch_writes_nothing() {
        let (ekko, dir) = board("refused");
        let cycle = batch(
            &ekko,
            &[
                json!({"op": "create", "text": "a"}),
                json!({"op": "create", "text": "b", "blocked_by": ["$1"]}),
                json!({"op": "link", "item": "$1", "blocked_by": ["$2"]}),
            ],
        );
        assert!(matches!(cycle, Err(EkkoError::BlockingCycle(_, _))), "{:?}", cycle.err());

        let rule = batch(
            &ekko,
            &[
                json!({"op": "create", "text": "a"}),
                json!({"op": "create", "text": "b", "blocked_by": ["$1"]}),
                json!({"op": "set_state", "items": ["$2"], "state": "done"}),
            ],
        );
        assert!(matches!(rule, Err(EkkoError::Blocked(_))), "{:?}", rule.err());

        let bad_ref = batch(&ekko, &[json!({"op": "set_state", "items": ["$1"], "state": "done"})]);
        assert!(matches!(bad_ref, Err(EkkoError::InvalidInput(_))));

        assert!(ekko.storage.get().unwrap().is_empty(), "a refused batch wrote something");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A refusal names an item on the board by its id, and one the refused
    /// write would have created -- which exists nowhere -- by the operation
    /// that created it, never by the number it held in the draft.
    #[test]
    fn a_refusal_names_what_it_would_have_created_by_operation() {
        let (ekko, dir) = board("names");
        batch(&ekko, &[json!({"op": "create", "text": "on the board"})]).unwrap();

        let mut draft = Draft::open(&ekko).unwrap();
        draft.apply(&op(json!({"op": "create", "text": "drafted"}))).unwrap();
        let names = draft.names();
        assert_eq!(names.name(1), "1");
        assert_eq!(names.name(2), "$1 (from operation 1)");
        assert_eq!(names.name(3), "the new item");
        drop(draft);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A commit names the tasks its write set free and the ones it left
    /// waiting, from either side of a dependency -- and only tasks that
    /// existed before it: work the write created is the caller's own doing.
    #[test]
    fn a_commit_names_what_it_released_and_what_it_blocked() {
        let (ekko, dir) = board("readiness");
        batch(
            &ekko,
            &[
                json!({"op": "create", "text": "blocker"}),
                json!({"op": "create", "text": "one", "blocked_by": ["$1"]}),
                json!({"op": "create", "text": "two", "blocked_by": ["$1"]}),
                json!({"op": "create", "text": "free"}),
            ],
        )
        .unwrap();

        let done = batch(&ekko, &[json!({"op": "set_state", "items": [1], "state": "done"})]).unwrap();
        assert_eq!((done.released, done.blocked), (vec![2, 3], vec![]));

        let again = batch(&ekko, &[json!({"op": "set_state", "items": [1], "state": "done"})]).unwrap();
        assert!(again.released.is_empty() && again.blocked.is_empty(), "a retry released something new");

        let reopened = batch(&ekko, &[json!({"op": "set_state", "items": [1], "state": "undone"})]).unwrap();
        assert_eq!((reopened.released, reopened.blocked), (vec![], vec![2, 3]));

        let linked = batch(&ekko, &[json!({"op": "link", "item": 4, "blocked_by": [1]})]).unwrap();
        assert_eq!((linked.released, linked.blocked), (vec![], vec![4]));

        let created = batch(&ekko, &[json!({"op": "create", "text": "new and waiting", "blocked_by": [1]})]).unwrap();
        assert!(created.released.is_empty() && created.blocked.is_empty(), "a task the write created was reported");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A note is neither a blocker nor something with a state: blocking on one
    /// would block nothing, and a task state set on one would change nothing
    /// while the reply said ok. Both are refused; a star still applies.
    #[test]
    fn a_note_can_neither_block_nor_take_a_task_state() {
        let (ekko, dir) = board("notes");
        batch(&ekko, &[json!({"op": "create", "kind": "note", "text": "a note"})]).unwrap();

        let blocked = batch(&ekko, &[json!({"op": "create", "text": "waits on a note", "blocked_by": [1]})]);
        assert!(matches!(blocked, Err(EkkoError::InvalidInput(_))), "{:?}", blocked.err());
        let stated = batch(&ekko, &[json!({"op": "set_state", "items": [1], "state": "done"})]);
        assert!(matches!(stated, Err(EkkoError::InvalidInput(_))), "{:?}", stated.err());
        assert!(batch(&ekko, &[json!({"op": "set_state", "items": [1], "state": "starred"})]).is_ok());

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A misspelled field is refused, not ignored: an ignored `blockedBy`
    /// would create the task and silently drop the dependency.
    #[test]
    fn an_unknown_field_is_refused() {
        let wrong = serde_json::from_value::<Op>(json!({"op": "create", "text": "x", "blockedBy": [1]}));
        assert!(wrong.is_err());
        let unknown_op = serde_json::from_value::<Op>(json!({"op": "destroy"}));
        assert!(unknown_op.is_err());
    }

    /// A replacement names one exact occurrence, and an edit conditioned on a
    /// version refuses once the item has moved on.
    #[test]
    fn an_edit_replaces_one_exact_occurrence_against_the_version_it_read() {
        let (ekko, dir) = board("edit");
        let written = batch(&ekko, &[json!({"op": "create", "kind": "note", "text": "alpha beta alpha"})]).unwrap();
        let version = written.data[&1].updated_at.unwrap();

        let twice = batch(&ekko, &[json!({"op": "edit", "item": 1, "replace": {"old": "alpha", "new": "x"}})]);
        assert!(matches!(twice, Err(EkkoError::EditMatch { found: 2, .. })));
        let nowhere = batch(&ekko, &[json!({"op": "edit", "item": 1, "replace": {"old": "gamma", "new": "x"}})]);
        assert!(matches!(nowhere, Err(EkkoError::EditMatch { found: 0, .. })));

        std::thread::sleep(std::time::Duration::from_millis(5));
        let replaced = batch(
            &ekko,
            &[json!({"op": "edit", "item": 1, "replace": {"old": "beta", "new": "gamma"}, "if_updated_at": version})],
        )
        .unwrap();
        assert_eq!(replaced.data[&1].description, "alpha gamma alpha");

        let stale = batch(&ekko, &[json!({"op": "edit", "item": 1, "append": "late", "if_updated_at": version})]);
        assert!(matches!(stale, Err(EkkoError::Stale { .. })), "{:?}", stale.err());

        let appended = batch(&ekko, &[json!({"op": "edit", "item": 1, "append": "omega"})]).unwrap();
        assert_eq!(appended.data[&1].description, "alpha gamma alpha omega");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Moving an item between phases is held to the phase order its
    /// dependencies already follow, and `null` clears what it names.
    #[test]
    fn an_update_keeps_dependencies_along_the_phase_order() {
        let (ekko, dir) = board("update");
        ekko.set_phases(&["early".to_string(), "late".to_string()]).unwrap();
        batch(
            &ekko,
            &[
                json!({"op": "create", "text": "first", "phase": "early", "due": "2026-01-01"}),
                json!({"op": "create", "text": "second", "phase": "late", "blocked_by": ["$1"]}),
            ],
        )
        .unwrap();

        let same = batch(&ekko, &[json!({"op": "update", "item": 2, "phase": "early"})]);
        assert!(same.is_ok(), "moving into its blocker's phase was refused: {:?}", same.err());
        let against = batch(&ekko, &[json!({"op": "update", "item": 1, "phase": "late"})]);
        assert!(
            matches!(against, Err(EkkoError::PhaseOrder(_))),
            "a blocker moved to a phase after what it blocks: {:?}",
            against.err()
        );

        let unknown = batch(&ekko, &[json!({"op": "update", "item": 1, "phase": "nowhere"})]);
        assert!(matches!(unknown, Err(EkkoError::InvalidInput(_))));

        let cleared = batch(&ekko, &[json!({"op": "update", "item": 1, "due": null})]).unwrap();
        assert_eq!(cleared.data[&1].due_date, None);

        std::fs::remove_dir_all(&dir).ok();
    }
}
