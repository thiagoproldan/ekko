//! What a Bash call says of the task it is for (task 1560, decision 1615).
//! A branch `task-N` it creates claims task N for the session, the way
//! Linear's one shortcut names the branch, assigns the issue and moves it to
//! In Progress: the claim happens in the gesture that starts the work. A
//! commit whose `Ekko:` trailer names a task the session does not hold is
//! told so. Both reach the session through the guard's PreToolUse reply, as
//! context beside the call's result; neither refuses the call.
//!
//! In the six-session run of 2026-10-09 a session worked 22 minutes on a task
//! it had not set in progress, and the others, finding nothing in progress,
//! saw none of that work: set_state progress is the only claim they see, and
//! the only one that makes a second session's change HELD.

use std::path::{Path, PathBuf};

use crate::ekko::Ekko;
use crate::holder::{Actor, Registry};
use crate::item::{Item, State};
use crate::ops::{Draft, Ref};
use crate::shell::{self, Call, Reading};

/// Where a guarded call finds its board: the variables `ekko` itself reads
/// there, as the hook was started with them.
#[derive(Debug, Default)]
pub struct Env {
    pub ekko_dir: Option<String>,
    pub project: Option<String>,
}

impl Env {
    pub fn of_this_process() -> Env {
        Env { ekko_dir: std::env::var("EKKO_DIR").ok(), project: std::env::var("EKKO_PROJECT").ok() }
    }
}

/// Git's options before its command that take a value of their own.
const GIT_VALUED: &[&str] = &["-c", "-C", "--git-dir", "--work-tree", "--namespace", "--exec-path", "--config-env"];

/// The options of `git branch` that list, delete, move, copy or describe
/// branches: with any of them, `git branch <name>` creates none.
const NOT_CREATING: &[&str] = &[
    "-d", "-D", "--delete", "-m", "-M", "--move", "-c", "-C", "--copy", "-l", "--list", "-a", "--all", "-r", "--remotes",
    "--show-current", "--contains", "--no-contains", "--merged", "--no-merged", "--points-at", "-u", "--set-upstream-to",
    "--unset-upstream", "--edit-description", "-v", "-vv", "--verbose", "--format", "--sort", "--column",
];

/// What the guard tells the session `actor` of the Bash call `command`, run
/// from `cwd`, about the tasks it works on: each branch `task-N` it creates,
/// which claims task N, or why it did not; and each task a commit's `Ekko:`
/// trailer names that the session does not hold. Only on a project's board:
/// a trailer names an item of the project the repository is. Nothing for the
/// user, and nothing for a call that neither names a branch `task-N` nor
/// an `Ekko:` trailer, which nearly every call does not, at the cost of a
/// look at its text.
pub fn told(home: &Path, command: &str, cwd: &Path, actor: &Actor, env: &Env) -> Vec<String> {
    if actor.is_person() || !command.contains("git") {
        return Vec::new();
    }
    let lower = command.to_ascii_lowercase();
    let from_file = lower.contains("commit") && (command.contains("-F") || command.contains("--file"));
    if !lower.contains("task-") && !lower.contains("ekko:") && !from_file {
        return Vec::new();
    }
    let (calls, placed) = shell::located(command, cwd, Reading::GUARD);
    let mut said = Vec::new();
    for place in &placed {
        let call = &calls[place.at];
        if call.name != "git" {
            continue;
        }
        let Some((git, rest, folder)) = git_command(&call.args, &place.folder) else { continue };
        if let Some(branch) = created_branch(git, rest) {
            let Some(id) = branch.strip_prefix("task-").filter(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())) else {
                continue;
            };
            let Ok(id) = id.parse::<u32>() else { continue };
            if let Some(ekko) = board(home, &folder, actor, env) {
                said.extend(claim(&ekko, id, branch));
            }
        } else if git == "commit" {
            let names = crate::commits::named(&message(&calls, place.at, rest, &folder));
            if names.is_empty() {
                continue;
            }
            if let Some(ekko) = board(home, &folder, actor, env) {
                said.extend(unheld(&ekko, &names));
            }
        }
    }
    said.dedup();
    said
}

/// The command git runs with `args`, the arguments after it, and the folder
/// it runs in: `folder`, or where `-C` moves it.
fn git_command<'a>(args: &'a [String], folder: &Path) -> Option<(&'a str, &'a [String], PathBuf)> {
    let mut folder = folder.to_path_buf();
    let mut at = 0;
    while let Some(arg) = args.get(at).map(String::as_str) {
        if arg == "-C" {
            if let Some(dir) = args.get(at + 1) {
                folder = folder.join(dir);
            }
            at += 2;
        } else if GIT_VALUED.contains(&arg) {
            at += 2;
        } else if arg.starts_with('-') {
            at += 1;
        } else {
            return Some((arg, &args[at + 1..], folder));
        }
    }
    None
}

/// The branch the git command `git`, given `rest`, creates: `worktree add`
/// with `-b` or `-B`, the only one of its commands that takes them,
/// `checkout` with `-b` or `-B`, `switch` with `-c`, `-C`,
/// `--create` or `--force-create`, and `branch <name>` with no option that
/// lists, deletes, moves or copies branches.
fn created_branch<'a>(git: &str, rest: &'a [String]) -> Option<&'a str> {
    let given = |flags: &[&str]| -> Option<&'a str> {
        for (at, arg) in rest.iter().enumerate() {
            if arg == "--" {
                break;
            }
            for flag in flags {
                if arg == flag {
                    return rest.get(at + 1).map(String::as_str);
                }
                let attached = match flag.strip_prefix("--") {
                    Some(_) => arg.strip_prefix(flag).and_then(|value| value.strip_prefix('=')),
                    None => arg.strip_prefix(flag).filter(|value| !value.is_empty() && !arg.starts_with("--")),
                };
                if attached.is_some() {
                    return attached;
                }
            }
        }
        None
    };
    match git {
        "worktree" => given(&["-b", "-B"]),
        "checkout" => given(&["-b", "-B"]),
        "switch" => given(&["-c", "-C", "--create", "--force-create"]),
        "branch" => {
            let lists = |arg: &String| NOT_CREATING.iter().any(|flag| arg == flag || arg.strip_prefix(flag).is_some_and(|rest| rest.starts_with('=')));
            if rest.iter().any(lists) {
                return None;
            }
            shell::operands(rest, &[]).first().copied()
        }
        _ => None,
    }
}

/// The message of the commit `calls[at]` makes, as far as the call shows it:
/// its arguments, all that is fed to it -- a here-document, what a
/// substitution in them prints -- and the file `-F` or `--file` names.
fn message(calls: &[Call], at: usize, rest: &[String], folder: &Path) -> String {
    let mut texts = shell::heard(calls, at, Reading { unwrap: true, messages: false });
    let mut args = rest.iter();
    while let Some(arg) = args.next() {
        let file = match arg.as_str() {
            "-F" | "--file" => args.next().map(String::as_str),
            other => other.strip_prefix("--file=").or_else(|| other.strip_prefix("-F").filter(|file| !file.is_empty())),
        };
        if let Some(file) = file.filter(|file| *file != "-") {
            if let Ok(text) = std::fs::read_to_string(folder.join(file)) {
                texts.push(text.chars().take(64 * 1024).collect());
            }
        }
    }
    texts.join("\n")
}

/// The project's board `ekko` finds from `folder`, read as `actor` with the
/// processes the SessionStart hook recorded, so a claim names the
/// conversation to resume. `None` off a project, and where it cannot open.
fn board(home: &Path, folder: &Path, actor: &Actor, env: &Env) -> Option<Ekko> {
    let location = crate::directory::locate(home, folder, None, env.ekko_dir.as_deref(), env.project.as_deref()).ok()?;
    location.project.as_ref()?;
    let actor = actor.clone().with_registry(Registry::at(crate::agent::processes_dir(home)));
    Some(Ekko::at(&location).ok()?.acting_as(actor))
}

/// Sets task `id` in progress for the session `ekko` acts as, which created
/// `branch`, and says so; or why it did not. Nothing when the session holds
/// it already.
fn claim(ekko: &Ekko, id: u32, branch: &str) -> Option<String> {
    let actor = ekko.actor.as_ref()?;
    let data = ekko.storage.get_shared().ok()?;
    let Some(task) = data.get(&id).filter(|item| item.is_task) else {
        return Some(format!("ekko: branch {branch} names no task on this board, so it claimed nothing"));
    };
    match State::of(task) {
        Some(State::Progress) if task.held_by.as_ref().is_some_and(|holder| actor.is(holder) || actor.continues(holder)) => return None,
        Some(state @ (State::Done | State::Cancelled)) => {
            return Some(format!(
                "ekko: task {id} is {}, so branch {branch} claimed nothing; set_state progress reopens it, if this is more of its work",
                state.word()
            ));
        }
        _ => {}
    }
    let mut draft = Draft::open(ekko).ok()?;
    draft.set_state(&[Ref::Id(id)], "progress").ok()?;
    Some(match draft.commit(false) {
        Ok(_) => format!("ekko: task {id} is in progress now, held by this session: creating branch {branch} claimed it (task 1560)"),
        Err(error) => format!("ekko: branch {branch} did not claim task {id}: {error}"),
    })
}

/// What a commit naming `names` is told of each task among them that the
/// session `ekko` acts as does not hold in progress. A done or cancelled
/// task is left alone: a commit after it is often its own follow-up.
fn unheld(ekko: &Ekko, names: &[String]) -> Vec<String> {
    let (Some(actor), Ok(data)) = (ekko.actor.as_ref(), ekko.storage.get_shared()) else { return Vec::new() };
    let mut said = Vec::new();
    for name in names {
        let found = match name.parse::<u32>() {
            Ok(id) => data.get(&id),
            Err(_) => data.values().find(|item| item.uid.as_deref() == Some(name.as_str())),
        };
        let Some(task) = found.filter(|item: &&Item| item.is_task) else { continue };
        let id = task.id;
        let line = match (State::of(task), &task.held_by) {
            (Some(State::Done | State::Cancelled), _) => continue,
            (Some(State::Progress), Some(holder)) if actor.is(holder) || actor.continues(holder) => continue,
            (Some(State::Progress), Some(holder)) if holder.alive() => {
                format!("ekko: this commit names task {id}, which {} holds, not this session", actor.name(holder))
            }
            (Some(State::Progress), Some(holder)) => {
                format!("ekko: this commit names task {id}, held by {}, which has ended: set_state progress takes it over", actor.name(holder))
            }
            (state, _) => format!(
                "ekko: this commit names task {id}, which this session does not hold: it is {}. Set it in progress before you change anything for it: that claim is what other sessions see",
                state.map_or("pending", State::word)
            ),
        };
        if !said.contains(&line) {
            said.push(line);
        }
    }
    said
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::holder::test_sessions;
    use crate::storage::Storage;
    use serde_json::json;

    /// A home holding project `site`, whose board has the tasks `texts`.
    fn project(tag: &str, texts: &[&str]) -> (PathBuf, PathBuf) {
        let home = crate::paths::test_dir(&format!("ekko-claims-{tag}"));
        let site = home.join("site");
        std::fs::create_dir_all(&site).unwrap();
        crate::project::init(&home, &site, None, None, 0).unwrap();
        for text in texts {
            write(&site, None, json!({"op": "create", "text": text}));
        }
        (home, site)
    }

    fn write(site: &Path, actor: Option<&Actor>, op: serde_json::Value) {
        let ekko = Ekko::new(Storage::new(&site.join(".ekko")).unwrap());
        let ekko = match actor {
            Some(actor) => ekko.acting_as(actor.clone()),
            None => ekko,
        };
        let mut draft = Draft::open(&ekko).unwrap();
        draft.apply(&serde_json::from_value(op).unwrap()).unwrap();
        draft.commit(false).unwrap();
    }

    fn task(site: &Path, id: u32) -> Item {
        Storage::new(&site.join(".ekko")).unwrap().get().unwrap()[&id].clone()
    }

    /// A branch `task-N` the session creates claims task N for it, in each
    /// way git creates one, and says so once; a task another running session
    /// holds, a done one and one not on the board stay as they were, and the
    /// reply says why (task 1560).
    #[test]
    fn creating_a_branch_task_n_claims_task_n() {
        let (me, other, _) = test_sessions();
        let (home, site) = project("branch", &["one", "two", "three", "four", "five", "six", "seven"]);
        let told = |command: &str| told(&home, command, &site, &me, &Env::default());
        let mine = |id: u32| {
            let task = task(&site, id);
            State::of(&task) == Some(State::Progress) && task.held_by.as_ref().is_some_and(|holder| me.is(holder))
        };

        let said = told("git worktree add ../wt -b task-1");
        assert_eq!(said, vec!["ekko: task 1 is in progress now, held by this session: creating branch task-1 claimed it (task 1560)"]);
        assert!(mine(1));
        assert_eq!(told("git worktree add ../wt -b task-1"), Vec::<String>::new(), "held already");
        for (command, id) in [("cd ../ && cd site && git checkout -b task-2 main", 2), ("git switch --create=task-3", 3), ("git -c x=y branch --track task-4 origin/main", 4)] {
            assert_eq!(told(command).len(), 1, "{command}");
            assert!(mine(id), "{command}");
        }

        write(&site, Some(&other), json!({"op": "set_state", "items": [5], "state": "progress"}));
        let said = told("git checkout -B task-5").join("\n");
        assert!(said.starts_with("ekko: branch task-5 did not claim task 5: ") && said.contains("still running"), "{said}");
        assert!(task(&site, 5).held_by.as_ref().is_some_and(|holder| other.is(holder)));
        write(&site, None, json!({"op": "set_state", "items": [6], "state": "done"}));
        assert_eq!(told("git switch -c task-6"), vec!["ekko: task 6 is done, so branch task-6 claimed nothing; set_state progress reopens it, if this is more of its work"]);
        assert_eq!(told("git branch task-99"), vec!["ekko: branch task-99 names no task on this board, so it claimed nothing"]);

        for command in [
            "git branch -D task-6",
            "git branch --list task-6",
            "git checkout task-6",
            "git worktree add ../wt task-6",
            "git worktree add ../wt -b feature",
            "git checkout -b task-6x",
            "git checkout -b task-+6",
            "echo git checkout -b task-6",
            "git log --oneline task-6",
        ] {
            assert_eq!(told(command), Vec::<String>::new(), "{command}");
        }
        let plain = home.join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        assert_eq!(super::told(&home, "git checkout -b task-6", &plain, &me, &Env::default()), Vec::<String>::new(), "off a project");
        assert_eq!(super::told(&home, "git -C ../site switch -c task-7", &plain, &me, &Env::default()).len(), 1, "git -C names the project");
        assert!(mine(7));
        assert_eq!(super::told(&home, "git checkout -b task-6", &site, &Actor::person(), &Env::default()), Vec::<String>::new(), "the user");
        assert_eq!(State::of(&task(&site, 6)), Some(State::Done));
        std::fs::remove_dir_all(&home).ok();
    }

    /// A commit whose `Ekko:` trailer names a task the session does not hold
    /// is told so -- written in an argument, a here-document, a substitution
    /// or a file -- and one naming a task it holds, a done one, or nothing,
    /// is not (task 1560).
    #[test]
    fn a_commit_naming_a_task_the_session_does_not_hold_is_told() {
        let (me, other, gone) = test_sessions();
        let (home, site) = project("commit", &["one", "two", "three", "four", "five"]);
        let told = |command: &str| told(&home, command, &site, &me, &Env::default()).join("\n");
        let pending = "ekko: this commit names task 1, which this session does not hold: it is pending. Set it in progress before you change anything for it: that claim is what other sessions see";

        std::fs::write(site.join("message.txt"), "fix: a thing\n\nEkko: 1\n").unwrap();
        for command in [
            "git commit -m 'fix: a thing\n\nEkko: 1'",
            "git commit -q -F - <<'EOF'\nfix: a thing\n\nEkko: 1\nEOF",
            "git add -A && git commit -m \"$(cat <<'EOF'\nfix: a thing\n\nEkko: #1\nEOF\n)\"",
            "git commit -F message.txt",
            "git -C ../site commit --trailer 'Ekko: 1' -m fix",
        ] {
            assert_eq!(told(command), pending, "{command}");
        }
        let uid = task(&site, 1).uid.unwrap();
        assert_eq!(told(&format!("git commit -m 'fix\n\nEkko: {uid}'")), pending, "by its uid");
        write(&site, Some(&me), json!({"op": "set_state", "items": [2], "state": "progress"}));
        write(&site, Some(&other), json!({"op": "set_state", "items": [3], "state": "progress"}));
        write(&site, Some(&gone), json!({"op": "set_state", "items": [4], "state": "progress"}));
        write(&site, None, json!({"op": "set_state", "items": [5], "state": "done"}));
        let named = told("git commit -m 'fix\n\nEkko: 2, 3, 4, 5'");
        assert!(!named.contains("task 2") && !named.contains("task 5"), "{named}");
        assert!(named.contains("names task 3, which default on pts/2 holds, not this session"), "{named}");
        assert!(named.contains("names task 4, held by default on pts/3, which has ended: set_state progress takes it over"), "{named}");
        for command in ["git commit -m 'Ekko: the board has three views'", "git log --grep 'Ekko: 1'", "git commit -m 'fix'", "echo 'Ekko: 1'"] {
            assert_eq!(told(command), "", "{command}");
        }
        std::fs::remove_dir_all(&home).ok();
    }
}
