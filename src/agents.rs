//! `ekko agents start` (task 1642, artifact 1640): a session of Claude Code
//! born to do board tasks, opened in a window of its own in a terminal
//! multiplexer, with the model and effort chosen for those tasks, in a
//! worktree of its own. It lives for its tasks; the user sees its window,
//! and may step into it and talk to it. Its hook (task 1643) closes it once
//! its tasks are finished, unless the user talks to it after that, and says
//! on the board and in its window's name when it waits on the user (task
//! 1645). What it is for and what it does now are kept on its pane, for
//! the views to draw (task 1675).

use std::collections::{BTreeMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::directory::Location;
use crate::ekko::{Ekko, EkkoError};
use crate::holder::Process;
use crate::item::{Item, State};
use crate::storage::ItemMap;

/// The multiplexer's session holding a window for each born session.
pub const SESSION: &str = "ekko";

/// Names, by uid and comma-separated, the tasks a born session was started
/// for, for the hooks that run in it.
pub const TASKS_VAR: &str = "EKKO_AGENT_TASK";

/// Name the worktree a born session works in and its branch, for the guard
/// that keeps its writes there (task 1800).
pub const WORKTREE_VAR: &str = "EKKO_AGENT_WORKTREE";
pub const BRANCH_VAR: &str = "EKKO_AGENT_BRANCH";

/// The levels Claude Code's `--effort` takes (its CLI reference, read
/// 2026-10-09); which of them a model allows is Claude Code's to say.
pub const EFFORTS: &[&str] = &["low", "medium", "high", "xhigh", "max", "ultracode"];

/// How long a born session must still run after its window opens for the
/// start to count: a command that is not there, or a Claude Code that
/// refuses to start, ends well within it.
const SETTLE: Duration = Duration::from_secs(1);

/// The user options a born session's pane holds for the views (task 1675,
/// decision 1674), which every view reads and none writes: its tasks' ids,
/// the first one's title, its model and effort, set as it starts; its state
/// -- `working`, `waiting` on the user or `idle` -- and since when, in
/// seconds since the epoch, set by its hook as it changes; and what it
/// waits on, while it does. On the pane, not its window, so they go with it
/// into another window. And the project whose tasks they are (task 1727),
/// as one session of the multiplexer holds every project's.
pub const TASKS_OPTION: &str = "@ekko_tasks";
pub const TITLE_OPTION: &str = "@ekko_title";
pub const MODEL_OPTION: &str = "@ekko_model";
pub const EFFORT_OPTION: &str = "@ekko_effort";
pub const STATE_OPTION: &str = "@ekko_state";
pub const SINCE_OPTION: &str = "@ekko_since";
pub const WAITS_OPTION: &str = "@ekko_waits";
pub const PROJECT_OPTION: &str = "@ekko_project";

/// The most sessions `start` runs at once for a project where the user set
/// no other with `ekko agents limit` (task 1727), nor a default for the
/// machine with `--default` (task 1902): two, as the user decided (decision
/// 1717).
pub const LIMIT: u32 = 2;

impl crate::storage::AgentsSettings {
    /// The most sessions that run at once for the project: its own limit,
    /// else the machine's `default`, else `LIMIT`.
    pub fn at_once(&self, default: Option<u32>) -> u32 {
        self.limit.or(default).unwrap_or(LIMIT)
    }
}

/// The machine's default limit, as `~/.ekko.json` under `home` holds it.
pub fn default_limit(home: &Path) -> Option<u32> {
    crate::config::get(home).ok().and_then(|config| config.agents_limit)
}

/// The states `STATE_OPTION` takes.
pub const WORKING: &str = "working";
pub const WAITING: &str = "waiting";
pub const IDLE: &str = "idle";

/// A tmux format, true when a pane of the window it is drawn for is in
/// `state` (task 1695): the window's own option would be its active pane's,
/// which may be a shell split beside the session or the view in a floating
/// pane over it.
macro_rules! any_pane {
    ($state:literal) => {
        concat!("#{P:#{?#{==:#{@ekko_state},", $state, "},1,}}")
    };
}

/// A window's tab while ztmux's tab bar is on (task 1678): `ztmux tabs on`
/// sets the global user option @ztmux-tab-bar and draws each window as
/// ` #I #W ` in grey, the current one in green -- colours as ztmux 3.7.47's
/// tabs.rs sets them -- but a born window's own format is the one tmux
/// draws it with. So this draws the bar's tab around `$name`, in black on
/// yellow while a pane of the window waits on the user.
macro_rules! tab {
    ($($name:tt)+) => {
        concat!(
            "#{?",
            any_pane!("waiting"),
            ",#[fg=black#,bg=yellow#,bold],#{?window_active,#[fg=colour235#,bg=colour108#,bold],#[fg=colour245#,bg=colour237]}} #I ",
            $($name)+,
            " #[default]"
        )
    };
}

/// How a born session's window reads in the status line (task 1675): its
/// index, then its name after a symbol for its session's state -- ⚙
/// working, ● waiting on you, black on yellow, ○ idle, dimmed -- and tmux's
/// flags, as tmux's own format ends. No colour of a status line hides it:
/// tmux's own is green, which a green symbol would vanish into. The state
/// is the panes' of the window, waiting first, whichever is active. With
/// ztmux's tab bar on, a tab as the bar draws its own (`tab!`).
pub const STATUS_FORMAT: &str = concat!(
    "#{?#{@ztmux-tab-bar},",
    tab!(
        "#{?",
        any_pane!("waiting"),
        ",\u{25cf} ,#{?",
        any_pane!("working"),
        ",\u{2699} ,#{?",
        any_pane!("idle"),
        ",\u{25cb} ,}}}#W"
    ),
    ",#I:#{?",
    any_pane!("waiting"),
    ",#[fg=black#,bg=yellow#,bold]\u{25cf} #W#[default],",
    "#{?",
    any_pane!("working"),
    ",\u{2699} #W,",
    "#{?",
    any_pane!("idle"),
    ",#[dim]\u{25cb} #W#[default],#W}}}",
    "#{?window_flags,#{window_flags}, }}"
);

/// What `start` opened.
pub struct Started {
    pub tasks: Vec<u32>,
    pub window: String,
    pub pane: String,
    pub worktree: PathBuf,
    pub branch: String,
    /// Whether the worktree was made by this start, rather than left by an
    /// earlier one.
    pub made: bool,
    /// The command that shows the session's window.
    pub attach: String,
    /// Why the pane's options and its window's status line could not be
    /// set, if they could not: the session runs all the same.
    pub undrawn: Option<String>,
    /// The note on each task saying how its session was started (task
    /// 1647): the model, the effort, the window and the branch.
    pub notes: Vec<u32>,
    /// Why those notes could not be written, if they could not: the session
    /// runs all the same.
    pub unnoted: Option<String>,
    /// The tasks other sessions work beside these, as a group (task 1648).
    pub beside: Vec<u32>,
}

/// Opens a session of Claude Code for the tasks `raw_ids` of the board at
/// `location`, with `model` and `effort`: in a worktree of the project's
/// repository, on a branch named for the tasks, in a window named for them
/// and the model in the multiplexer's session `ekko`, told by its first
/// prompt to claim them before it changes anything. Refused, opening
/// nothing, for a task the page's Start would refuse -- done, in progress,
/// in the trash, with someone, waiting on open work -- for one whose done
/// blocker's branch is not merged yet (task 1728), and for one that has a
/// window already; and while as many sessions run for the project as its
/// limit lets run at once (task 1727). With `raw_group`, the session is one
/// of a group (task 1648): those tasks are worked beside its own by sessions
/// of their own, which its first prompt names, for them to talk through
/// notes on each other's tasks. Once the session runs, a note on each task
/// records how it was started.
pub fn start(
    ekko: &Ekko,
    location: &Location,
    home: &Path,
    raw_ids: &[String],
    raw_group: &[String],
    model: &str,
    effort: Option<&str>,
) -> Result<Started, EkkoError> {
    let refused = EkkoError::InvalidInput;
    let Some(project) = location.project.as_ref() else {
        return Err(refused(
            "ekko agents works on a project's board, whose sessions work in worktrees of the project's repository; this is the default board".into(),
        ));
    };
    let Some(root) = project.root.as_deref() else {
        return Err(refused(format!("project {} has no folder of its own, so no repository for a session to work in", project.name)));
    };
    // The born session finds the board by the project's name: walking up
    // from a worktree leads to the repository's top, and misses a board in
    // a folder under it.
    if !crate::project::resolve_named(home, &project.name).is_ok_and(|found| same(&found.dir, &location.dir)) {
        return Err(refused(format!(
            "project {} is not registered under its name, which a session needs to reach its board: ekko init {} registers it",
            project.name,
            root.display()
        )));
    }
    ekko.storage.writable()?;
    let all = ekko.storage.get_shared()?;
    let ids = ekko.validate_ids(raw_ids, &all)?;
    for id in &ids {
        refusal(&all[id], &all).map_err(refused)?;
    }
    let mut beside: Vec<u32> = if raw_group.is_empty() { Vec::new() } else { ekko.validate_ids(raw_group, &all)? };
    beside.retain(|id| !ids.contains(id));
    beside.sort_unstable();
    for id in &beside {
        grouped(&all[id]).map_err(|why| refused(format!("{why}: a group names the tasks other sessions work beside these; leave it out of --group")))?;
    }
    // Its blockers' work first (task 1728): the worktree starts from HEAD.
    let top = PathBuf::from(
        git(root, &["rev-parse", "--show-toplevel"])
            .map_err(|why| refused(format!("{} is in no git repository, and a born session works in a worktree of its own: {why}", root.display())))?,
    );
    for id in &ids {
        if let Some((blocker, branch, made_for)) = unmerged_blocker(&top, "HEAD", &all[id], &all, &ids).map_err(refused)? {
            let on = git(&top, &["symbolic-ref", "--quiet", "--short", "HEAD"]).unwrap_or_else(|_| "the checkout's HEAD".to_string());
            return Err(refused(format!(
                "task {id} waits on task {blocker}, whose work is on branch {branch}, not in {on} yet: merge it first with ekko agents merge {}, \
                 as task {id}'s session starts from {on}",
                joined(&made_for, " ")
            )));
        }
    }

    let environment = starting_environment().map_err(refused)?;
    trusted(&environment, home, &top).map_err(refused)?;
    let mux = Mux::new(environment.clone());
    let windows = mux.windows().map_err(refused)?;
    let born = mux.born_panes().map_err(refused)?;
    if let Some((id, name)) = window_of(&project.name, &ids, &windows, &born) {
        return Err(refused(format!("task {id} has a window already, {name} in session {SESSION}: step into it there")));
    }
    // A session on the wall has no window of its own (task 1677): its pane
    // says what it is for.
    if let Some((id, pane)) = pane_of(&project.name, &ids, &born) {
        return Err(refused(format!("task {id} has a session already, in pane {} of window {} in session {SESSION}: step into it there", pane.pane, pane.window)));
    }
    let limit = ekko.storage.get_agents()?.at_once(default_limit(home));
    let running = running_for(&project.name, &windows, &born);
    if running.len() >= limit as usize {
        return Err(refused(format!(
            "{} {} for project {} already, as many as its limit lets run at once ({limit}, set with ekko agents limit): {}. \
             Start {} once one has gone -- a session closes itself once its tasks are finished; the limit is the user's to raise",
            running.len(),
            if running.len() == 1 { "session ekko agents started runs" } else { "sessions ekko agents started run" },
            project.name,
            running.join("; "),
            match ids.as_slice() {
                [id] => format!("task {id}"),
                _ => format!("tasks {}", listed(&ids)),
            },
        )));
    }
    let branch = branch_for(&top, &ids).map_err(refused)?;
    let worktree = worktree(root, &branch).map_err(refused)?;
    let real_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let cwd = match real_root.strip_prefix(&worktree.top) {
        Ok(under) if !under.as_os_str().is_empty() => worktree.path.join(under),
        _ => worktree.path.clone(),
    };
    let window = format!("{} \u{b7} {model}", joined(&ids, ","));
    let uids: Vec<String> = ids.iter().map(|id| all[id].uid.clone().unwrap_or_else(|| id.to_string())).collect();
    let uids = uids.join(",");
    // Its own folder for what it keeps, which a reboot does not empty, and the
    // ignored inputs the project names (task 1828).
    let folder = session_folder(home, &project.name, &branch);
    let tmp = folder.join("tmp");
    std::fs::create_dir_all(&tmp).map_err(|error| refused(format!("{} could not be made: {error}", tmp.display())))?;
    let settings = ekko.storage.get_agents()?;
    copy_ignored(&settings.copy, &worktree.top, &worktree.path).map_err(refused)?;
    let declared: Vec<(&str, OsString)> =
        settings.env.iter().map(|(name, value)| (name.as_str(), OsString::from(value.replace("{dir}", &folder.to_string_lossy())))).collect();
    let mut born: Vec<(&str, &OsStr)> = vec![
        (TASKS_VAR, uids.as_ref()),
        ("EKKO_PROJECT", project.name.as_ref()),
        (WORKTREE_VAR, worktree.path.as_os_str()),
        (BRANCH_VAR, branch.as_ref()),
        ("TMPDIR", tmp.as_os_str()),
    ];
    let brief = ekko.storage.get_agents()?.brief;
    // After TMPDIR: the project may set its own.
    born.extend(declared.iter().map(|(name, value)| (*name, value.as_os_str())));
    let argv = command_line(&environment, &mux.binary, &born, model, effort, &first_prompt(&ids, &beside, &branch, &worktree.path, brief.as_deref()));
    let pane = mux.open(&window, &cwd, &argv).map_err(refused)?;
    let title = match ids.as_slice() {
        [] => String::new(),
        [first] => crate::ekko::title(&all[first].description).to_string(),
        [first, rest @ ..] => format!("{} (+{})", crate::ekko::title(&all[first].description), rest.len()),
    };
    let mut drawn = vec![
        set_pane(&pane, TASKS_OPTION, &joined(&ids, ",")),
        set_pane(&pane, TITLE_OPTION, &title),
        set_pane(&pane, MODEL_OPTION, model),
        effort.map_or_else(|| unset_pane(&pane, EFFORT_OPTION), |effort| set_pane(&pane, EFFORT_OPTION, effort)),
    ];
    drawn.extend(state_commands(&pane, WORKING, None));
    for option in ["window-status-format", "window-status-current-format"] {
        drawn.push(["set-option", "-w", "-t", &pane, option, STATUS_FORMAT].map(str::to_string).to_vec());
    }
    drawn.push(set_pane(&pane, PROJECT_OPTION, &project.name));
    let undrawn = mux.batch(&drawn).err();
    std::thread::sleep(SETTLE);
    if !mux.alive(&pane) {
        return Err(refused(format!(
            "the session opened in window {window} ended within {} s: {} did not start there; run it in {} to see why",
            SETTLE.as_secs(),
            claude().to_string_lossy(),
            cwd.display()
        )));
    }
    // What the orchestrator chose for the tasks, on each, to learn from
    // (design item 5): the pane's options go with the pane.
    let text = format!(
        "Started by ekko agents start: a session on {model}{}, in window \"{window}\" of session {SESSION}, on branch {branch} in {} ({}).{}",
        effort.map(|effort| format!(" at effort {effort}")).unwrap_or_else(|| " at its default effort".to_string()),
        worktree.path.display(),
        if worktree.made { "made for it" } else { "an earlier start's, taken up again" },
        match beside.as_slice() {
            [] => String::new(),
            [id] => format!(" In a group with task {id}, worked by a session of its own: a note on either task reaches the session holding it."),
            _ => format!(" In a group with tasks {}, each worked by a session of its own: a note on a task reaches the session holding it.", listed(&beside)),
        },
    );
    let (notes, unnoted) = match crate::agents_merge::noted(ekko, &ids, &text) {
        Ok(notes) => (notes, None),
        Err(why) => (Vec::new(), Some(why.to_string())),
    };
    Ok(Started { tasks: ids, window, pane, worktree: worktree.path, branch, made: worktree.made, attach: mux.attach(), undrawn, notes, unnoted, beside })
}

/// Whether the repository whose top is `top` is trusted on the Claude Code
/// profile a born session will run under (task 1755): `environment`'s
/// CLAUDE_CONFIG_DIR, else the default profile, whose global config is
/// `~/.claude.json`. A repository's root not trusted there has every born
/// session stop at Claude Code's trust dialog before any hook runs, unseen:
/// its task stays unclaimed and no wait on it ends. A trusted parent folder
/// does not count, and trusting the root covers its worktrees (note 1835).
/// ekko does not write that config: trust is the user's to give.
fn trusted(environment: &[(OsString, OsString)], home: &Path, top: &Path) -> Result<(), String> {
    let config = match environment.iter().find(|(name, dir)| name == "CLAUDE_CONFIG_DIR" && !dir.is_empty()) {
        Some((_, dir)) => Path::new(dir).join(".claude.json"),
        None => home.join(".claude.json"),
    };
    let root = std::fs::canonicalize(top).unwrap_or_else(|_| top.to_path_buf());
    let accepted = std::fs::read(&config)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .is_some_and(|json| json["projects"][root.to_string_lossy().as_ref()]["hasTrustDialogAccepted"] == true);
    if accepted {
        return Ok(());
    }
    Err(format!(
        "{root} is not trusted on the Claude Code profile the born sessions use: projects[\"{root}\"].hasTrustDialogAccepted is not true in {}. \
         Each session would stop at Claude Code's trust dialog (\"Is this a project you created or one you trust?\"), where no hook runs: \
         its task stays unclaimed and no wait on it ends. A trusted parent folder does not count. Trust is the user's to give: open Claude Code \
         in {root} once, on that profile, and accept the dialog; ekko does not write Claude Code's config",
        config.display(),
        root = root.display(),
    ))
}

/// The branch a session started for `ids` works on (task 1851): the one
/// named for exactly those tasks, `task-12-13`; or, where none is, the one
/// an earlier start made for a group holding them all and more, if its work
/// is not in the checkout's HEAD yet -- a group handed back with one of its
/// tasks left to do goes on there, not on a new branch from HEAD that leaves
/// the group's work behind. More than one such branch is refused, naming them.
fn branch_for(top: &Path, ids: &[u32]) -> Result<String, String> {
    let own = format!("task-{}", joined(ids, "-"));
    if git(top, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{own}")]).is_ok() {
        return Ok(own);
    }
    let mut groups = Vec::new();
    for branch in git(top, &["for-each-ref", "--format=%(refname:short)", "refs/heads/task-*"])?.lines() {
        let made_for = branch_tasks(branch);
        if made_for.len() > ids.len() && ids.iter().all(|id| made_for.contains(id)) && !has(top, "HEAD", branch)? {
            groups.push(branch.to_string());
        }
    }
    match groups.as_slice() {
        [] => Ok(own),
        [group] => Ok(group.clone()),
        _ => Err(format!(
            "{} started in groups before, on branches {} that HEAD does not have yet: a start takes up one branch, so merge or drop all but one first",
            if ids.len() == 1 { format!("task {} was", ids[0]) } else { format!("tasks {} were", listed(ids)) },
            groups.join(", ")
        )),
    }
}

/// Why `task` may not be named in a group, if it may not (task 1648): a
/// group names tasks other sessions work, or will, so not a note, nor a
/// task in the trash or finished.
fn grouped(task: &Item) -> Result<(), String> {
    if task.trashed.is_some() {
        return Err(format!("task {} is in the trash", task.id));
    }
    match State::of(task) {
        Some(state @ (State::Done | State::Cancelled)) => Err(format!("task {} is {} already", task.id, state.word())),
        Some(_) => Ok(()),
        None => Err(format!("{} is no task", task.id)),
    }
}

/// The command that sets the user option `option` of `pane` to `value`, as
/// a status line or a border shows it (`shown`).
fn set_pane(pane: &str, option: &str, value: &str) -> Vec<String> {
    ["set-option", "-p", "-t", pane, option].map(str::to_string).into_iter().chain([shown(value)]).collect()
}

/// The command that unsets the user option `option` of `pane`.
fn unset_pane(pane: &str, option: &str) -> Vec<String> {
    ["set-option", "-p", "-u", "-t", pane, option].map(str::to_string).to_vec()
}

/// The commands that set `pane`'s state, since now, and what it waits on --
/// or that it waits on nothing.
fn state_commands(pane: &str, state: &str, waits: Option<&str>) -> Vec<Vec<String>> {
    let since = chrono::Local::now().timestamp().to_string();
    vec![
        set_pane(pane, STATE_OPTION, state),
        set_pane(pane, SINCE_OPTION, &since),
        match waits {
            Some(what) => set_pane(pane, WAITS_OPTION, what),
            None => unset_pane(pane, WAITS_OPTION),
        },
    ]
}

/// `value` as a status line or a pane's border shows it, kept as given: on
/// one line, its `#` doubled, which would start a style or a variable there,
/// and a `;` at its end escaped, which tmux would take for the end of the
/// command (both measured on tmux 3.7c and ztmux 3.7.47, task 1675).
fn shown(value: &str) -> String {
    let line = value.replace(['\n', '\r', '\t'], " ").replace('#', "##");
    match line.strip_suffix(';') {
        Some(rest) => format!("{rest}\\;"),
        None => line,
    }
}

/// Whether the window `name` is another project's: every pane of the session
/// that sits in a window of that name names another project. One session of
/// the multiplexer holds every project's windows, named by display id alone
/// (task 1731); a window whose panes name none, or this project, or that has
/// no pane listed, stays any project's.
fn window_is_foreign(name: &str, project: &str, born: &[BornPane]) -> bool {
    let mut panes = born.iter().filter(|pane| pane.window == name).peekable();
    panes.peek().is_some() && panes.all(|pane| pane.project.as_deref().is_some_and(|other| other != project))
}

/// The first of `ids`, and the window, among `windows` whose name gives
/// that task and that is not another project's (task 1731).
pub(crate) fn window_of<'a>(project: &str, ids: &[u32], windows: &'a [String], born: &[BornPane]) -> Option<(u32, &'a String)> {
    windows.iter().filter(|name| !window_is_foreign(name, project, born)).find_map(|name| ids.iter().find(|id| tasks_of(name).contains(id)).map(|id| (*id, name)))
}

/// The first of `ids`, and the pane that still lives, among `born` whose
/// tasks hold it and that is not another project's (task 1731): a pane that
/// names no project is any project's.
pub(crate) fn pane_of<'a>(project: &str, ids: &[u32], born: &'a [BornPane]) -> Option<(u32, &'a BornPane)> {
    born.iter()
        .filter(|pane| !pane.dead && pane.project.as_deref().is_none_or(|name| name == project))
        .find_map(|pane| ids.iter().find(|id| pane.tasks.contains(id)).map(|id| (*id, pane)))
}

/// The sessions `start` opened that count toward `project`'s limit (task
/// 1727), each as its refusal names it: the live panes whose options name
/// the project, or name none, as an ekko before that task left them; and,
/// of whichever project, the windows named for tasks where no pane has the
/// options, which the multiplexer would not set (`Started::undrawn`).
fn running_for(project: &str, windows: &[String], born: &[BornPane]) -> Vec<String> {
    let drawn: HashSet<&str> = born.iter().map(|pane| pane.window.as_str()).collect();
    let tasks = |ids: &[u32]| match ids {
        [id] => format!("task {id}"),
        _ => format!("tasks {}", listed(ids)),
    };
    born.iter()
        .filter(|pane| !pane.dead && pane.project.as_deref().is_none_or(|name| name == project))
        .map(|pane| format!("{} in window \"{}\"", tasks(&pane.tasks), pane.window))
        .chain(windows.iter().filter(|name| !tasks_of(name).is_empty() && !drawn.contains(name.as_str())).map(|name| format!("window \"{name}\"")))
        .collect()
}

/// Why `task` may not go to a born session now, if it may not: what the
/// page's Start refuses (`artifact::startable`), and for a task in progress,
/// who holds it and whether that one still runs.
fn refusal(task: &Item, all: &ItemMap) -> Result<(), String> {
    crate::artifact::startable(task, all).map_err(|why| match (&task.held_by, State::of(task)) {
        (Some(holder), Some(State::Progress)) => {
            let still = match holder.process() {
                None => "",
                Some(_) if holder.alive() => ", which still runs",
                Some(_) => ", which has ended: pause the task to hand it over",
            };
            format!("{why}, held by {}{still}", holder.label())
        }
        _ => why,
    })
}

/// The environment a born session starts from. Under a Claude Code session
/// -- the orchestrator, running this command -- the one that session's
/// process started with, what the user's launcher gave it, read from /proc:
/// without what Claude Code adds to each command it runs, which ties the
/// command to it (CLAUDECODE, CLAUDE_CODE_SESSION_ID, CLAUDE_PID and more,
/// which a version may add to) and has a Claude Code started there refuse to
/// run, nested in another. This command's own otherwise: a person's.
pub(crate) fn starting_environment() -> Result<Vec<(OsString, OsString)>, String> {
    if std::env::var_os("CLAUDECODE").is_none() {
        return Ok(std::env::vars_os().collect());
    }
    let pid = crate::holder::claude_among(std::os::unix::process::parent_id()).ok_or_else(|| {
        "this runs under a Claude Code session whose process is not among its ancestors, so the environment that session started with is not known".to_string()
    })?;
    let read = std::fs::read(format!("/proc/{pid}/environ"))
        .map_err(|error| format!("the environment Claude Code (pid {pid}) started with could not be read: {error}"))?;
    let environment: Vec<(OsString, OsString)> = read
        .split(|byte| *byte == 0)
        .filter_map(|entry| {
            let at = entry.iter().position(|byte| *byte == b'=').filter(|at| *at > 0)?;
            Some((OsStr::from_bytes(&entry[..at]).to_os_string(), OsStr::from_bytes(&entry[at + 1..]).to_os_string()))
        })
        .collect();
    if environment.is_empty() {
        return Err(format!("the environment Claude Code (pid {pid}) started with reads empty"));
    }
    Ok(environment)
}

/// The variables of a terminal's tab or window that name where the user is
/// typing (task 1803): ctx types `/clear` into "the session's Konsole tab"
/// through KONSOLE_DBUS_*, and the others are the same kind of address in
/// the terminals ekko knows; Black Box, which the menu opens in first, sets
/// none (task 2156). A born session lives in a multiplexer pane, not in the
/// user's tab, and a test the gate runs has no tab at all.
pub(crate) const TERMINAL_ADDRESSES: &[&str] = &[
    "KONSOLE_DBUS_SERVICE",
    "KONSOLE_DBUS_SESSION",
    "KONSOLE_DBUS_WINDOW",
    "KITTY_WINDOW_ID",
    "KITTY_LISTEN_ON",
    "WEZTERM_PANE",
    "WEZTERM_UNIX_SOCKET",
    "ITERM_SESSION_ID",
    "TERM_SESSION_ID",
    "WT_SESSION",
    "WINDOWID",
];

/// The variables a command started from this process must not inherit: one
/// list for a born session's window (task 1803) and for the test that gates
/// a merge (task 1942). Every variable this process has that `environment`
/// -- what the launcher gave the Claude Code session, as
/// `starting_environment` reads it -- has not, which are those Claude Code
/// adds to each command it runs; CLAUDECODE, whatever a version adds; the
/// Claude Code profile, where the launcher named none; and the addresses of
/// the user's terminal tab, which the launcher did give.
pub(crate) fn session_variables(environment: &[(OsString, OsString)]) -> Vec<OsString> {
    let started: HashSet<&OsStr> = environment.iter().map(|(name, _)| name.as_os_str()).collect();
    let mut unset: Vec<OsString> = std::env::vars_os().map(|(name, _)| name).filter(|name| !started.contains(name.as_os_str())).collect();
    unset.push("CLAUDECODE".into());
    if !started.contains(OsStr::new("CLAUDE_CONFIG_DIR")) {
        unset.push("CLAUDE_CONFIG_DIR".into());
    }
    unset.extend(TERMINAL_ADDRESSES.iter().map(OsString::from));
    unset.sort();
    unset.dedup();
    unset
}

/// What a born session's window runs: Claude Code through env(1), which
/// unsets `session_variables` -- CLAUDECODE, every other variable this
/// command has and `environment` has not (a multiplexer's server started by
/// a session's command holds them all), the user's terminal tab -- sets the
/// Claude Code profile `environment` names, CLAUDE_CONFIG_DIR, or unsets it
/// for the default one, and sets `born` -- the tasks, the project, the
/// worktree and its branch, the session's TMPDIR and what the project
/// declares (task 1828) -- and the multiplexer's binary, for the hooks that
/// guard the session and close its window. The rest is the multiplexer's, as
/// in any window the user opens there: no value but these goes on a command
/// line, which any process on the machine may read.
fn command_line(
    environment: &[(OsString, OsString)],
    mux: &OsStr,
    born: &[(&str, &OsStr)],
    model: &str,
    effort: Option<&str>,
    prompt: &str,
) -> Vec<OsString> {
    let unset = session_variables(environment);
    let profile = environment.iter().find(|(name, _)| name == "CLAUDE_CONFIG_DIR").map(|(_, dir)| dir);
    // env(1) reads its options up to the first assignment: every -u first.
    let mut argv: Vec<OsString> = vec!["env".into()];
    for name in unset {
        argv.push("-u".into());
        argv.push(name);
    }
    if let Some(dir) = profile {
        let mut set = OsString::from("CLAUDE_CONFIG_DIR=");
        set.push(dir);
        argv.push(set);
    }
    for (name, value) in born {
        let mut set = OsString::from(format!("{name}="));
        set.push(value);
        argv.push(set);
    }
    let mut binary = OsString::from("EKKO_MUX=");
    binary.push(mux);
    argv.push(binary);
    argv.push(claude());
    // No one answers its questions (task 1868): Claude Code's own tool for
    // them is not in its context. ekko's ask is left out by ekko's MCP
    // server, which sees EKKO_AGENT_TASK, whatever name it is registered
    // under. Before --model, where the list of tools ends.
    argv.extend(["--disallowedTools".into(), DISALLOWED.into()]);
    argv.extend(["--model".into(), model.into()]);
    if let Some(effort) = effort {
        argv.extend(["--effort".into(), effort.into()]);
    }
    argv.push(prompt.into());
    argv
}

/// The folder of a born session's own, under ekko's state directory, which
/// a reboot does not empty as it does /tmp (task 1828): the session's
/// TMPDIR is its `tmp`, and `{dir}` in a variable the project declares is the
/// folder itself.
pub(crate) fn session_folder(home: &Path, project: &str, branch: &str) -> PathBuf {
    let clean = |text: &str| -> String {
        let clean: String = text.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_') { c } else { '_' }).collect();
        if clean.chars().all(|c| c == '.') { "_".to_string() } else { clean }
    };
    crate::agent::state_dir(home).join("agents").join(clean(project)).join(clean(branch))
}

/// `path` as a copied path is kept (task 1828): relative to the repository's
/// top, under it, and not git's own folder.
pub(crate) fn copy_path(path: &str) -> Result<String, String> {
    let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty() && *part != ".").collect();
    if path.starts_with('/') || parts.is_empty() || parts.iter().any(|part| *part == ".." || *part == ".git") {
        return Err(format!("`{path}` is no path under the repository's top: write it relative to the top, as `evals/private`, without .. or .git"));
    }
    Ok(parts.join("/"))
}

/// Copies each of `paths` from the repository's top `from` into the worktree
/// `to`, where it is not already: what git ignores exists only in the
/// checkout, and a worktree is made without it (task 1828). A path the
/// checkout does not have is skipped.
fn copy_ignored(paths: &[String], from: &Path, to: &Path) -> Result<(), String> {
    for path in paths {
        let (source, target) = (from.join(path), to.join(path));
        if std::fs::symlink_metadata(&source).is_err() || std::fs::symlink_metadata(&target).is_ok() {
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|error| format!("{} could not be made: {error}", parent.display()))?;
        }
        copy_tree(&source, &target).map_err(|error| format!("{path} could not be copied from {} into {}: {error}", from.display(), to.display()))?;
    }
    Ok(())
}

fn copy_tree(source: &Path, target: &Path) -> std::io::Result<()> {
    let kind = std::fs::symlink_metadata(source)?.file_type();
    if kind.is_symlink() {
        std::os::unix::fs::symlink(std::fs::read_link(source)?, target)
    } else if kind.is_dir() {
        std::fs::create_dir(target)?;
        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            copy_tree(&entry.path(), &target.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(source, target).map(|_| ())
    }
}

/// The tools a born session runs without (task 1868): AskUserQuestion,
/// which puts a question to a user who is not at its window.
pub const DISALLOWED: &str = "AskUserQuestion";

/// The Claude Code a born session runs: `EKKO_CLAUDE`, else `claude` as the
/// window's PATH finds it -- the user's own launcher, with their settings.
fn claude() -> OsString {
    set("EKKO_CLAUDE").unwrap_or_else(|| "claude".into())
}

/// A born session's first prompt: what it was started for, that it claims
/// its tasks before it changes anything (design item 1), and how it ends --
/// a closing note on each task and the task done, or else a handoff, the
/// task still held, and the turn ended (design item 3, as task 1647 left
/// it): its hook then marks it waiting on the user, which wakes the
/// orchestrator waiting on the task (task 1715), as a task set back to
/// pending would not. Each note is named by the tool and argument that
/// write it: told only to attach a closing note, both sessions of one live
/// run appended it to the task's own text with edit (task 1644), where the
/// four sessions before them had attached one.
///
/// And how a note reaches it while it works (task 1648): ekko's wake hook
/// tells it of each note another attaches to its tasks, in a line that may
/// come inside a tool's result, which a session not told to expect one read
/// and would not act on (note 1741). With `beside`, the tasks of its group,
/// worked by sessions of their own, and how to write to them.
///
/// And that it says nothing to the user (decision 1870): it decides routine
/// choices and records them, for the review at merge (task 1868); its
/// closing notes are its report, with no text after the last (task 1866);
/// and what only the user can settle goes in a handoff, which the
/// orchestrator, woken by the session's wait, brings to them.
///
/// And, with `brief`, what the project tells its sessions (task 1927), after
/// how they decide and before how they end: ekko's own words say nothing of
/// it, so a project with none gets the prompt as it was.
///
/// And how it waits on its own work: by the work's own notice or its pid,
/// never a pgrep that matches the waiter, which kept done sessions open
/// (task 1829); and that a measurement it keeps names the tree and the
/// binaries it ran on, which the review then compares instead of file
/// times (task 1831).
fn first_prompt(ids: &[u32], beside: &[u32], branch: &str, worktree: &Path, brief: Option<&str>) -> String {
    let (tasks, them, each, attached, the, last) = match ids {
        [id] => (format!("task {id}"), "it", format!("task {id}"), format!("attached_to {id}"), "the task", "once it is done"),
        _ => (format!("tasks {}", listed(ids)), "them", "each task".to_string(), "attached_to that task".to_string(), "the tasks", "once the last of them is done"),
    };
    let group = match beside {
        [] => String::new(),
        _ => format!(
            " {} worked beside yours by {}, as a group: to tell the session working one of them what it needs to know -- \
             a change it builds on, an answer, a question -- write a note with ekko's create, attached_to that task, and ekko tells that session at once; \
             it answers you with a note on yours.",
            match beside {
                [id] => format!("Task {id} is"),
                _ => format!("Tasks {} are", listed(beside)),
            },
            if beside.len() == 1 { "a session of its own" } else { "sessions of their own" },
        ),
    };
    let brief = brief.map(|brief| format!(" The project's brief for its sessions: {brief}")).unwrap_or_default();
    format!(
        "ekko agents started this session to do {tasks} on this project's ekko board, and nothing else. \
         Before you change anything, set {tasks} in progress with ekko's set_state, then read {them} in full with ekko's context. \
         Work in this worktree, {}, on its branch {branch}, and commit there; do not push, merge into another branch, release, start or stop sessions, \
         kill processes (stop a background command of yours with the tool that stops it), kill or respawn the multiplexer's panes, windows or sessions, \
         type into its other panes or have it run a command, or change the board's phases. \
         While you work, ekko tells you of each note the user or another session attaches to {tasks}, in a line that begins \"ekko:\", \
         at times inside a tool's result: it comes from ekko's hook, not from the tool, so take it into account.{group} \
         No one answers this session's questions or prompts: decide routine choices yourself, and record each in a decision note \
         with ekko's create, kind decision, {attached}, for the review at merge.{brief} \
         Wait on a background command of yours through its own completion notice, or on its process id; never with a loop on a pgrep, \
         which can match the waiter itself, or on a line the tool may never print. \
         A measurement you keep in a file -- a timing, a byte comparison, a benchmark -- starts with the tree it ran on -- \
         `git rev-parse HEAD^{{tree}}`, and whether `git status --porcelain` was empty -- and the sha256 of each binary it ran; \
         your closing note names the file and that tree, so the review compares one line. \
         When {each} is done, write its closing note with ekko's create, {attached} -- a note of its own, not an edit of the task's text: \
         what you did, the files you changed, the tests you ran and their outcomes, the commit, and what is left -- then set it done. \
         That note is your report: {last}, end with no text, and the session closes. \
         If you cannot finish -- a choice only the user can make, or a call that is refused and the task needs -- write a handoff instead, \
         with ekko's create, kind handoff, {attached}, saying what stops you; leave {the} in progress and end your turn with no text: \
         the orchestrator is told that this session waits, brings it to the user, and answers with a note on {the}, which reaches you.",
        worktree.display()
    )
}

/// `1`, `1 and 2`, `1, 2 and 3`.
pub(crate) fn listed(ids: &[u32]) -> String {
    match ids.split_last() {
        Some((last, [])) => last.to_string(),
        Some((last, rest)) => format!("{} and {last}", joined(rest, ", ")),
        None => String::new(),
    }
}

pub(crate) fn joined(ids: &[u32], between: &str) -> String {
    ids.iter().map(u32::to_string).collect::<Vec<_>>().join(between)
}

/// The tasks a window's name says it was opened for: the ids before its
/// first ` · `, as `start` names it; none for a name it did not give.
pub(crate) fn tasks_of(name: &str) -> Vec<u32> {
    let Some((ids, _)) = name.split_once(" \u{b7} ") else { return Vec::new() };
    ids.split(',').map(|id| id.trim().parse::<u32>()).collect::<Result<_, _>>().unwrap_or_default()
}

/// The multiplexer, spoken to in tmux's command language (design item 8):
/// the binary `EKKO_MUX` names, `tmux` otherwise, on the server some
/// arguments name -- `-L <name>`, `-S <path>` -- or else the one it finds
/// itself: the server this command runs inside, or the user's default.
pub(crate) struct Mux {
    binary: OsString,
    server: Vec<OsString>,
    /// The environment it runs with; this process's own when `None`.
    environment: Option<Vec<(OsString, OsString)>>,
}

impl Mux {
    /// The one `start` speaks to, on the socket `EKKO_MUX_SOCKET` names, as
    /// `-L` takes it. It runs with the environment born sessions start
    /// from, so a server it starts hands that one to every window.
    fn new(environment: Vec<(OsString, OsString)>) -> Mux {
        let server = set("EKKO_MUX_SOCKET").map(|socket| vec!["-L".into(), socket]).unwrap_or_default();
        Mux { binary: set("EKKO_MUX").unwrap_or_else(|| "tmux".into()), server, environment: Some(environment) }
    }

    /// The one `start` speaks to, as this process's environment has it:
    /// inside a pane, without `EKKO_MUX_SOCKET`, the server of that pane, as
    /// tmux finds it from TMUX (task 1676).
    pub(crate) fn here() -> Mux {
        let server = set("EKKO_MUX_SOCKET").map(|socket| vec!["-L".into(), socket]).unwrap_or_default();
        Mux { binary: set("EKKO_MUX").unwrap_or_else(|| "tmux".into()), server, environment: None }
    }

    /// The arguments that run it on its server: the binary, then `-L` or
    /// `-S` and the socket, if one is named.
    pub(crate) fn argv(&self) -> Vec<OsString> {
        std::iter::once(self.binary.clone()).chain(self.server.iter().cloned()).collect()
    }

    /// The panes of the session `ekko` that `start` opened, each with what it
    /// keeps there for the views (task 1675); none without the session.
    pub(crate) fn born_panes(&self) -> Result<Vec<BornPane>, String> {
        if !self.has_session()? {
            return Ok(Vec::new());
        }
        let fields = ["#{pane_id}", "#{pane_dead}", "#{window_id}", "#{window_name}"]
            .into_iter()
            .map(str::to_string)
            .chain([TASKS_OPTION, TITLE_OPTION, MODEL_OPTION, EFFORT_OPTION, STATE_OPTION, SINCE_OPTION, WAITS_OPTION, PROJECT_OPTION].map(|option| format!("#{{{option}}}")))
            .collect::<Vec<_>>()
            .join("\t");
        let output = self.run(&["list-panes", "-s", "-t", &format!("={SESSION}"), "-F", &fields])?;
        if !output.status.success() {
            return Err(self.failed("list-panes", &output));
        }
        Ok(String::from_utf8_lossy(&output.stdout).lines().filter_map(BornPane::parse).collect())
    }

    /// The one whose pane this process runs in, as the server tells its
    /// panes: the socket before the first comma of TMUX.
    fn of_this_pane() -> Option<Mux> {
        let tmux = std::env::var_os("TMUX")?;
        let socket = tmux.as_bytes().split(|byte| *byte == b',').next().filter(|socket| !socket.is_empty())?;
        let server = vec!["-S".into(), OsStr::from_bytes(socket).to_os_string()];
        Some(Mux { binary: set("EKKO_MUX").unwrap_or_else(|| "tmux".into()), server, environment: None })
    }

    pub(crate) fn run<S: AsRef<OsStr>>(&self, args: &[S]) -> Result<Output, String> {
        let mut command = Command::new(&self.binary);
        command.args(&self.server).args(args).stdin(Stdio::null());
        if let Some(environment) = &self.environment {
            command.env_clear().envs(environment.iter().map(|(name, value)| (name, value)));
        }
        command.output().map_err(|error| format!("{} could not be run: {error}", self.binary.to_string_lossy()))
    }

    pub(crate) fn failed(&self, what: &str, output: &Output) -> String {
        format!("{} {what} failed: {}", self.binary.to_string_lossy(), String::from_utf8_lossy(&output.stderr).trim())
    }

    /// Runs `commands` in one call, as tmux runs a sequence: each separated
    /// from the next by a `;` of its own, in order, up to the first that
    /// fails.
    fn batch(&self, commands: &[Vec<String>]) -> Result<(), String> {
        let mut args: Vec<&str> = Vec::new();
        for command in commands {
            if !args.is_empty() {
                args.push(";");
            }
            args.extend(command.iter().map(String::as_str));
        }
        let output = self.run(&args)?;
        if !output.status.success() {
            return Err(self.failed(args.first().copied().unwrap_or_default(), &output));
        }
        Ok(())
    }

    fn has_session(&self) -> Result<bool, String> {
        Ok(self.run(&["has-session", "-t", &format!("={SESSION}")])?.status.success())
    }

    /// The window of the session `ekko` that WALL_OPTION marks, if one does
    /// (task 1677).
    fn wall_window(&self) -> Result<Option<String>, String> {
        if !self.has_session()? {
            return Ok(None);
        }
        let output = self.run(&["list-windows", "-t", &format!("={SESSION}"), "-F", &format!("#{{window_id}} #{{{WALL_OPTION}}}")])?;
        if !output.status.success() {
            return Err(self.failed("list-windows", &output));
        }
        Ok(String::from_utf8_lossy(&output.stdout).lines().find_map(|line| line.strip_suffix(" 1").map(str::to_string)))
    }

    /// Whether `pane` is on the wall, in a window whose name is not its own.
    fn on_wall(&self, pane: &str) -> bool {
        self.run(&["display-message", "-p", "-t", pane, &format!("#{{{WALL_OPTION}}}")]).is_ok_and(|output| output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "1")
    }

    /// Whether ztmux frames every pane, its zellij mode on, as it reads
    /// @ztmux-zellij-mode, or @ztmux-pane-names for an older setting; never
    /// for tmux, which has neither.
    fn framed(&self) -> bool {
        let format = "#{?#{@ztmux-zellij-mode},#{@ztmux-zellij-mode},#{@ztmux-pane-names}}";
        self.run(&["display-message", "-p", "-t", &format!("={SESSION}:"), format])
            .is_ok_and(|output| output.status.success() && matches!(String::from_utf8_lossy(&output.stdout).trim(), "on" | "1" | "true" | "yes"))
    }

    /// The names of the windows in the session `ekko`; none without it.
    pub(crate) fn windows(&self) -> Result<Vec<String>, String> {
        if !self.has_session()? {
            return Ok(Vec::new());
        }
        let output = self.run(&["list-windows", "-t", &format!("={SESSION}"), "-F", "#{window_name}"])?;
        if !output.status.success() {
            return Err(self.failed("list-windows", &output));
        }
        Ok(String::from_utf8_lossy(&output.stdout).lines().map(str::to_string).collect())
    }

    /// Opens the window `name`, in the folder `cwd`, running `argv` without
    /// a shell, in the session `ekko`, which it starts if it is not there;
    /// not made the session's current window, so the user's view stays.
    /// Its pane's id.
    fn open(&self, name: &str, cwd: &Path, argv: &[OsString]) -> Result<String, String> {
        let mut args: Vec<OsString> = if self.has_session()? {
            ["new-window", "-d", "-t", &format!("={SESSION}:")].map(OsString::from).to_vec()
        } else {
            ["new-session", "-d", "-s", SESSION].map(OsString::from).to_vec()
        };
        args.extend(["-n", name, "-c"].map(OsString::from));
        args.push(cwd.into());
        args.extend(["-P", "-F", "#{pane_id}"].map(OsString::from));
        args.extend(argv.iter().cloned());
        let output = self.run(&args)?;
        let pane = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !output.status.success() || pane.is_empty() {
            return Err(self.failed("could not open a window, and", &output));
        }
        Ok(pane)
    }

    /// The panes on its server, each as its id and whether it is dead: `%3
    /// 0` runs, `%3 1` is one a remain-on-exit keeps. tmux's
    /// display-message answers for a pane that is gone as for one that runs
    /// (gotcha 1637); this list tells them apart.
    fn panes(&self) -> Result<Vec<String>, String> {
        let output = self.run(&["list-panes", "-a", "-F", "#{pane_id} #{pane_dead}"])?;
        if !output.status.success() {
            return Err(self.failed("list-panes", &output));
        }
        Ok(String::from_utf8_lossy(&output.stdout).lines().map(str::to_string).collect())
    }

    /// Whether `pane` still runs; not when no server answers.
    fn alive(&self, pane: &str) -> bool {
        let running = format!("{pane} 0");
        self.panes().is_ok_and(|panes| panes.contains(&running))
    }

    /// Whether `pane` is on its server, running or dead; `Err` with what
    /// the server said when none answers, which takes its panes with it.
    fn listed(&self, pane: &str) -> Result<bool, String> {
        Ok(self.panes()?.iter().any(|line| line.split(' ').next() == Some(pane)))
    }

    /// The name of the window `pane` is in.
    fn window_name(&self, pane: &str) -> Result<String, String> {
        let output = self.run(&["display-message", "-p", "-t", pane, "#{window_name}"])?;
        if !output.status.success() {
            return Err(self.failed("display-message", &output));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim_end_matches('\n').to_string())
    }

    /// Names the window `pane` is in `name`.
    fn rename(&self, pane: &str, name: &str) -> Result<(), String> {
        let output = self.run(&["rename-window", "-t", pane, name])?;
        if !output.status.success() {
            return Err(self.failed("rename-window", &output));
        }
        Ok(())
    }

    fn attach(&self) -> String {
        let binary = Path::new(&self.binary).file_name().unwrap_or(self.binary.as_os_str()).to_string_lossy().into_owned();
        let server: String = self.server.iter().map(|arg| format!(" {}", arg.to_string_lossy())).collect();
        format!("{binary}{server} attach -t {SESSION}")
    }
}

/// A pane of the session `ekko` that `start` opened, as `born_panes` reads
/// it: what its options say, `#` no longer doubled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BornPane {
    pub pane: String,
    pub dead: bool,
    pub window_id: String,
    pub window: String,
    pub tasks: Vec<u32>,
    pub title: String,
    pub model: String,
    pub effort: Option<String>,
    pub state: String,
    /// Since when it is in its state, in seconds since the epoch.
    pub since: Option<i64>,
    pub waits: Option<String>,
    /// The project whose tasks they are; none for a pane an ekko before
    /// task 1727 opened.
    pub project: Option<String>,
}

impl BornPane {
    /// A line of `born_panes`' list, its fields apart by tabs, which no
    /// value holds (`shown`); none for a pane `start` did not open.
    fn parse(line: &str) -> Option<BornPane> {
        let fields: Vec<&str> = line.split('\t').collect();
        let [pane, dead, window_id, window, tasks, title, model, effort, state, since, waits, project] = fields.as_slice() else { return None };
        let tasks: Vec<u32> = tasks.split(',').map(|id| id.trim().parse::<u32>()).collect::<Result<_, _>>().unwrap_or_default();
        if tasks.is_empty() {
            return None;
        }
        let given = |value: &str| Some(value.replace("##", "#")).filter(|value| !value.is_empty());
        Some(BornPane {
            pane: pane.to_string(),
            dead: *dead == "1",
            window_id: window_id.to_string(),
            window: window.to_string(),
            tasks,
            title: given(title).unwrap_or_default(),
            model: model.to_string(),
            effort: given(effort),
            state: state.to_string(),
            since: since.parse().ok(),
            waits: given(waits),
            project: given(project),
        })
    }
}

/// The window option that marks the session's wall (task 1677).
pub const WALL_OPTION: &str = "@ekko_wall";

/// The wall's name, as its window is named in the status line.
pub const WALL: &str = "wall";

/// How each pane on the wall is titled on its top border: its state as
/// the status line draws it (STATUS_FORMAT), then its tasks, model and
/// title, from the options `start` set on it.
pub const WALL_BORDER: &str = concat!(
    " #{?#{==:#{@ekko_state},waiting},#[fg=black#,bg=yellow#,bold]\u{25cf} waiting on you#[default],",
    "#{?#{==:#{@ekko_state},working},\u{2699} working,",
    "#{?#{==:#{@ekko_state},idle},#[dim]\u{25cb} idle#[default],}}}",
    " #{@ekko_tasks} \u{b7} #{@ekko_model} \u{b7} #{@ekko_title} "
);

/// How the wall is drawn in the status line: its name, then each pane's
/// state, one symbol a pane as STATUS_FORMAT draws it, so that a session
/// that waits on the user shows when another pane is the active one; with
/// ztmux's tab bar on, in a tab as STATUS_FORMAT draws one.
pub const WALL_STATUS: &str = concat!(
    "#{?#{@ztmux-tab-bar},",
    tab!(
        "#W ",
        "#{P:#{?#{==:#{@ekko_state},waiting},\u{25cf},#{?#{==:#{@ekko_state},working},\u{2699},#{?#{==:#{@ekko_state},idle},\u{25cb},}}}}"
    ),
    ",#I:#W ",
    "#{P:#{?#{==:#{@ekko_state},waiting},#[fg=black#,bg=yellow#,bold]\u{25cf}#[default],",
    "#{?#{==:#{@ekko_state},working},\u{2699},",
    "#{?#{==:#{@ekko_state},idle},#[dim]\u{25cb}#[default],}}}}",
    "#{?window_flags,#{window_flags}, }}"
);

/// The options `wall` sets on the wall's window, each with its value there
/// and its value on a window of a session's own as `start` leaves it, none
/// for an option it leaves unset.
const WALL_OPTIONS: [(&str, &str, Option<&str>); 5] = [
    (WALL_OPTION, "1", None),
    ("pane-border-status", "top", None),
    ("pane-border-format", WALL_BORDER, None),
    ("window-status-format", WALL_STATUS, Some(STATUS_FORMAT)),
    ("window-status-current-format", WALL_STATUS, Some(STATUS_FORMAT)),
];

/// What `wall` and `unwall` moved: each session's pane and the tasks it is
/// for.
pub struct Moved {
    pub panes: Vec<(String, Vec<u32>)>,
    pub wall: Option<String>,
}

/// Whether a pane is `project`'s to move: one that names another project is
/// not, one that names none stays any project's (task 1806).
fn is_of(pane: &BornPane, project: Option<&str>) -> bool {
    pane.project.is_none() || pane.project.as_deref() == project
}

/// `ekko agents wall` (task 1677): every born session of `project` that runs,
/// side by side in one window of the session ekko, tiled, each pane titled on
/// its border; the wall window is the one WALL_OPTION marks, made now if
/// there is none. Each session goes on running in its pane, and typing there
/// still talks to it. One session holds every project's windows, and another
/// project's panes stay where they are (task 1806).
pub fn wall(project: Option<&str>) -> Result<Moved, String> {
    let mux = Mux::here();
    let every: Vec<BornPane> = mux.born_panes()?.into_iter().filter(|pane| !pane.dead).collect();
    let panes: Vec<&BornPane> = every.iter().filter(|pane| is_of(pane, project)).collect();
    if panes.is_empty() {
        return Err(match project {
            Some(project) => format!("no session ekko agents started for the project {project} runs in the session {SESSION}"),
            None => format!("no session ekko agents started runs in the session {SESSION}"),
        });
    }
    let (wall, placeholder) = match mux.wall_window()? {
        Some(wall) => (wall, None),
        None => {
            let output = mux.run(&["new-window", "-d", "-t", &format!("={SESSION}:"), "-n", WALL, "-P", "-F", "#{window_id} #{pane_id}"])?;
            let made = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let Some((window, pane)) = made.split_once(' ').filter(|_| output.status.success()) else {
                return Err(mux.failed("new-window", &output));
            };
            (window.to_string(), Some(pane.to_string()))
        }
    };
    // Each joins after the last pane on the wall, where join-pane puts it
    // in the window's list of panes, which the tiled layout follows: the
    // wall keeps the sessions in the order of their windows.
    let mut last = placeholder.clone().or_else(|| every.iter().rfind(|pane| pane.window_id == wall).map(|pane| pane.pane.clone())).unwrap_or_else(|| wall.clone());
    let mut moved = Vec::new();
    for pane in panes.iter().filter(|pane| pane.window_id != wall) {
        // Tiled after each, so that the next has room to split.
        mux.batch(&[
            ["join-pane", "-d", "-s", &pane.pane, "-t", &last].map(str::to_string).to_vec(),
            ["select-layout", "-t", &wall, "tiled"].map(str::to_string).to_vec(),
        ])?;
        last = pane.pane.clone();
        moved.push((pane.pane.clone(), pane.tasks.clone()));
    }
    let mut dressed: Vec<Vec<String>> = Vec::new();
    if let Some(placeholder) = placeholder {
        dressed.push(["kill-pane", "-t", &placeholder].map(str::to_string).to_vec());
        dressed.push(["select-layout", "-t", &wall, "tiled"].map(str::to_string).to_vec());
    }
    // ztmux's frames title each pane already (task 1678), and a border's
    // status line there takes the place of a pane's frame, which is then
    // not drawn (measured on ztmux 3.7.47).
    let framed = mux.framed();
    for (option, value, _) in WALL_OPTIONS {
        let value = if framed && option == "pane-border-status" { "off" } else { value };
        dressed.push(["set-option", "-w", "-t", &wall, option, value].map(str::to_string).to_vec());
    }
    // The wall is what one attached to the session ekko sees next.
    dressed.push(["select-window", "-t", &wall].map(str::to_string).to_vec());
    mux.batch(&dressed)?;
    Ok(Moved { panes: moved, wall: Some(wall) })
}

/// `ekko agents unwall`: each born session of `project` on the wall back in a
/// window of its own, named as `start` named it, its status line drawn as
/// `start` draws it. A pane on the wall that no session `start` opened, or
/// that is another project's, stays there (task 1806).
pub fn unwall(project: Option<&str>) -> Result<Moved, String> {
    let mux = Mux::here();
    let Some(wall) = mux.wall_window()? else {
        return Err(format!("there is no wall in the session {SESSION}"));
    };
    let mut moved = Vec::new();
    for pane in mux.born_panes()?.into_iter().filter(|pane| pane.window_id == wall && is_of(pane, project)) {
        let name = format!("{} \u{b7} {}", joined(&pane.tasks, ","), pane.model);
        let mut commands = vec![["break-pane", "-d", "-s", &pane.pane, "-n", &name].map(str::to_string).to_vec()];
        // break-pane gives the last pane on the wall the wall's own window,
        // renamed: each window given back sheds what the wall set on it.
        for (option, _, own) in WALL_OPTIONS {
            commands.push(match own {
                Some(value) => ["set-option", "-w", "-t", &pane.pane, option, value].map(str::to_string).to_vec(),
                None => ["set-option", "-wu", "-t", &pane.pane, option].map(str::to_string).to_vec(),
            });
        }
        mux.batch(&commands)?;
        moved.push((pane.pane, pane.tasks));
    }
    Ok(Moved { panes: moved, wall: None })
}

/// What born sessions did today, by `born.log`: each line written since
/// midnight, local time.
pub(crate) fn born_today(home: &Path) -> Vec<String> {
    let today = chrono::Local::now().format("%Y-%m-%dT").to_string();
    std::fs::read_to_string(born_dir(home).join("born.log")).unwrap_or_default().lines().filter(|line| line.starts_with(&today)).map(str::to_string).collect()
}

/// Whether a Claude Code process ekko recorded is a session `ekko agents
/// start` opened (task 1861): the tasks it was born for are in its
/// environment, as `starting_environment` gave them, or its hook has kept a
/// file for its conversation.
pub(crate) fn is_born(home: &Path, running: &crate::holder::Running) -> bool {
    let environment = std::fs::read(format!("/proc/{}/environ", running.pid)).unwrap_or_default();
    let named = format!("{TASKS_VAR}=");
    environment.split(|byte| *byte == 0).any(|entry| entry.starts_with(named.as_bytes()))
        || std::iter::once(&running.conversation).chain(&running.earlier).any(|conversation| born_file(home, conversation).exists())
}

/// The value of the variable `name`, unless it is unset or empty.
fn set(name: &str) -> Option<OsString> {
    std::env::var_os(name).filter(|value| !value.is_empty())
}

/// A born session's worktree: where it is, the top of its repository, and
/// whether this start made it.
pub(crate) struct Worktree {
    pub(crate) path: PathBuf,
    top: PathBuf,
    made: bool,
}

/// The worktree a born session works in: `.claude/worktrees/<branch>` at the
/// top of the repository holding `root`, where Claude Code's own
/// `--worktree` puts its own, on the branch `<branch>`. Made from the HEAD
/// of that checkout, unpushed commits and all -- Claude Code's default
/// branches from the remote's -- or, where an earlier start left one for
/// the branch, that one: the work of a session that stopped short goes on.
pub(crate) fn worktree(root: &Path, branch: &str) -> Result<Worktree, String> {
    let top = git(root, &["rev-parse", "--show-toplevel"])
        .map_err(|why| format!("{} is in no git repository, and a born session works in a worktree of its own: {why}", root.display()))?;
    let top = PathBuf::from(top);
    let path = top.join(".claude").join("worktrees").join(branch);
    let listed = git(&top, &["worktree", "list", "--porcelain"])?;
    for entry in listed.split("\n\n") {
        let field = |key: &str| entry.lines().find_map(|line| line.strip_prefix(key));
        if !field("worktree ").is_some_and(|at| same(Path::new(at), &path)) {
            continue;
        }
        let on = field("branch ").map(|on| on.trim_start_matches("refs/heads/"));
        return match on {
            Some(on) if on == branch && path.is_dir() => Ok(Worktree { path, top, made: false }),
            Some(on) if on == branch => Err(format!("{} is a worktree git still lists, and is gone: git worktree prune forgets it", path.display())),
            Some(on) => Err(format!("{} is a worktree on branch {on}, not {branch}", path.display())),
            None => Err(format!("{} is a worktree on no branch, not {branch}", path.display())),
        };
    }
    if path.exists() {
        return Err(format!("{} is there already, and is no worktree of {}", path.display(), top.display()));
    }
    let made = if git(&top, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")]).is_ok() {
        git(&top, &[OsStr::new("worktree"), OsStr::new("add"), path.as_os_str(), OsStr::new(branch)])
    } else {
        git(&top, &[OsStr::new("worktree"), OsStr::new("add"), OsStr::new("-b"), OsStr::new(branch), path.as_os_str(), OsStr::new("HEAD")])
    };
    made.map_err(|why| format!("no worktree was made at {}: {why}", path.display()))?;
    Ok(Worktree { path, top, made: true })
}

/// A done blocker of `task` whose work a born session left on a branch that
/// `into` does not have yet (task 1728): the blocker, the branch -- `task-N`,
/// or `task-N-M` for a group -- and the tasks the branch was made for. A
/// task's session starts from main, and its branch goes in after its
/// blockers', as Zuul and Mergify hold a change whose Depends-On has not
/// merged. A cancelled blocker holds nothing back, nor does one done
/// without a branch, nor one of `beside`, whose work goes in with the
/// task's, on one branch.
pub(crate) fn unmerged_blocker(top: &Path, into: &str, task: &Item, all: &ItemMap, beside: &[u32]) -> Result<Option<(u32, String, Vec<u32>)>, String> {
    let index = crate::ekko::uid_index(all);
    let blockers: Vec<u32> = task
        .blocked_by
        .iter()
        .flatten()
        .filter_map(|uid| index.get(uid.as_str()).copied())
        .filter(|id| !beside.contains(id) && all.get(id).is_some_and(|blocker| State::of(blocker) == Some(State::Done)))
        .collect();
    if blockers.is_empty() {
        return Ok(None);
    }
    for branch in git(top, &["for-each-ref", "--format=%(refname:short)", "refs/heads/task-*"])?.lines() {
        let made_for = branch_tasks(branch);
        let Some(blocker) = blockers.iter().find(|id| made_for.contains(id)) else { continue };
        if has(top, into, branch)? {
            continue;
        }
        return Ok(Some((*blocker, branch.to_string(), made_for)));
    }
    Ok(None)
}

/// The tasks a branch `start` made was made for, as its name says them:
/// `task-12` and `task-12-13`; none for a name not wholly that.
pub(crate) fn branch_tasks(branch: &str) -> Vec<u32> {
    let Some(ids) = branch.strip_prefix("task-") else { return Vec::new() };
    ids.split('-').map(str::parse::<u32>).collect::<Result<_, _>>().unwrap_or_default()
}

/// Whether `into` has every commit of `branch`.
fn has(top: &Path, into: &str, branch: &str) -> Result<bool, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(top)
        .args(["merge-base", "--is-ancestor", branch, into])
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("git could not be run: {error}"))?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(format!("git merge-base --is-ancestor {branch} {into} failed: {}", String::from_utf8_lossy(&output.stderr).trim())),
    }
}

/// `git -C dir args`: what it printed, trimmed, or else what it said.
pub(crate) fn git<S: AsRef<OsStr>>(dir: &Path, args: &[S]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("git could not be run: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// Whether two paths name the same place, through symlinks.
pub(crate) fn same(one: &Path, other: &Path) -> bool {
    let real = |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    real(one) == real(other)
}

/// The tasks this process acts for when a session ekko agents started runs
/// it -- EKKO_AGENT_TASK's uids, or display ids where a task has none --
/// and `None` anywhere else, the orchestrator included. Read where a
/// command, the MCP server or a hook starts, never by the board itself, so
/// a test run in a born session writes its own boards as before.
pub fn born_tasks() -> Option<Vec<String>> {
    let tasks: Vec<String> = std::env::var(TASKS_VAR).ok()?.split(',').map(str::trim).filter(|task| !task.is_empty()).map(str::to_string).collect();
    (!tasks.is_empty()).then_some(tasks)
}

/// What a session ekko agents started leaves to the orchestrator or the
/// user (task 1646, design item 7 of artifact 1640): the guard refuses its
/// Bash calls that do it, each as `limit_of` reads it, and its board refuses
/// it a write to another's task (`EkkoError::NotBornFor`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit {
    /// What sends its branch out: `git push`, `gh pr create` or `merge`, and
    /// `ekko agents merge` (task 1714).
    Push,
    /// What releases or deploys: a `gh release` that writes, a `publish` of
    /// `cargo`, `npm`, `pnpm` or `yarn`, `nixos-rebuild switch`, `boot` or
    /// `test`, and a program whose name starts with `release`, run itself
    /// or by a shell, as `scripts/release.sh` is.
    Release,
    /// What starts or opens a Claude Code session: `ekko agents start`, and
    /// `claude` but for `--version`, `--help` and the commands that start
    /// none (`NO_SESSION`).
    Session,
    /// What stops, restarts or removes Claude Code's sessions (task 1716):
    /// `claude stop`, `kill`, `rm` and `respawn`, and `purge`, which deletes
    /// a project's state (task 1742) (`ON_SESSIONS`).
    Stop,
    /// What kills or respawns the multiplexer's panes, windows, sessions or
    /// server (task 1716): a `kill-*` or `respawn-*` command (`KILLS`) of
    /// tmux, kmux, ztmux, or the binary `EKKO_MUX` names, and the `-k` that
    /// kills a window `new-window`, `link-window`, `move-window` or
    /// `unlink-window` replaces or leaves (task 1742), as `mux_limit` reads
    /// them.
    Kill,
    /// What signals processes (task 1742): `kill`, `pkill`, `killall` and
    /// `skill`, whatever they are given, but to list the signals or ask for
    /// their help or version (`signals`), and a `kill` that signals none but
    /// the session's own (task 1849, `own_signal`).
    Signal,
    /// What a multiplexer reads as no tmux command (task 1746): kmux's own
    /// verbs, which it runs before tmux's commands -- `revive`, `bcast`,
    /// `prune`, `resurrect` and the rest `kmux verbs` lists as extensions --
    /// and any other word in their place (`mux_verb`).
    Verb,
    /// What has the multiplexer run a command it is given (task 1742): a
    /// command of `REACHES` given one, a hook or another option of
    /// `RUN_OPTIONS` set, a copy mode pipe, a format's `#()`, or its own
    /// `-c`.
    Runs,
    /// What types into another pane than the session's own (task 1742):
    /// `send-keys`, `paste-buffer` and `send-prefix` with a `-t` that names
    /// another, or on a server another `-L` or `-S` names.
    Keys,
    /// What writes what every checkout of its repository shares, or leaves
    /// its own branch (task 1800): a git command that writes a branch other
    /// than the session's, a tag, notes, replacements, the worktrees, the
    /// remotes or the repository's config, or that puts its worktree on
    /// another branch or none (`git_touches`).
    Shared,
    /// What writes the stash, which every checkout of its repository shares
    /// (tasks 1800 and 1833): its refusal says how to set work aside instead.
    Stash,
    /// What writes in another checkout of its repository than its own
    /// worktree -- the project's own, another session's worktree (task
    /// 1800): a git command that writes there, wherever `-C`, a `cd`,
    /// `--git-dir` or `--work-tree` puts it, and an Edit or Write of a file
    /// there, or a Bash call's (task 1925, `write_limit`).
    Checkout,
}

impl Limit {
    /// Why a born session leaves it to another, as its refusal says.
    fn why(self) -> &'static str {
        match self {
            Limit::Push => "does not push or open a pull request: its branch is the orchestrator's to merge, and what reaches a remote is the user's to send",
            Limit::Release => "does not release or deploy: that waits on the user's word",
            Limit::Session => "does not start sessions: the orchestrator starts them, each with a context and a cost of its own",
            Limit::Stop => "does not stop, respawn or remove Claude Code's sessions, nor purge their state: they are the orchestrator's and the user's to end",
            Limit::Kill => "does not kill or respawn the multiplexer's panes, windows, sessions or server: the orchestrator, the user and the other born sessions run there",
            Limit::Signal => {
                "does not signal processes with kill, pkill or killall: the orchestrator's, the user's and the other born sessions' run on this machine too. \
                 A kill -0, which signals none, goes through, and so does a kill of its own: $!, $$, a job as %1, a variable holding one of those, \
                 or a pid below its Claude Code process"
            }
            Limit::Verb => {
                "does not run what the multiplexer reads as no tmux command -- kmux's own verbs, as revive, bcast, prune and resurrect, \
                 which `kmux verbs` lists as extensions: several respawn, broadcast to or remove what the orchestrator, the user and the other \
                 born sessions run, and the guard does not read them"
            }
            Limit::Runs => {
                "does not have the multiplexer run a command for it -- run-shell, if-shell, a key binding, a hook, a popup, a new window's command, a file of commands, \
                 a format's #(): the guard does not see what that command does"
            }
            Limit::Keys => "does not type into another pane than its own: the others are the orchestrator's, the user's and the other born sessions'",
            Limit::Shared => {
                "does not leave its own branch, nor write what every checkout of its repository shares -- another branch, a tag, notes, \
                 the worktrees, the remotes, the config: the orchestrator merges its branch into main, and the rest is the user's and the other sessions'"
            }
            Limit::Stash => {
                "does not write the stash: it is one list every checkout of its repository shares, where another session may pop or drop its entry, \
                 or it theirs. To set work aside, make it a WIP commit on its own branch -- git add -A && git commit -m wip -- and take it back \
                 with git reset HEAD~1; git stash list, show and apply stay free"
            }
            Limit::Checkout => {
                "does not write in another checkout of its repository -- the project's own, another session's worktree: it works in its own worktree, \
                 on its own branch, and reads anywhere"
            }
        }
    }
}

/// The options `claude` takes a value after, as `claude --help` lists them
/// (Claude Code 2.1.295, and 2.1.296 the same): never its command, nor a
/// prompt.
const CLAUDE_VALUED: &[&str] = &[
    "--add-dir", "--agent", "--agents", "--allowedTools", "--allowed-tools", "--append-system-prompt", "--autocompact", "--betas",
    "--debug-file", "--disallowedTools", "--disallowed-tools", "--effort", "--environment", "--fallback-model", "--file",
    "--input-format", "--json-schema", "--max-budget-usd", "--mcp-config", "--model", "-n", "--name", "--output-format",
    "--permission-mode", "--permission-prompts", "--plugin-dir", "--plugin-url", "--remote-control-session-name-prefix",
    "--session-id", "--setting-sources", "--settings", "--system-prompt", "--system-prompt-snapshot", "--tools",
];

/// Claude Code's commands that start or open no session, nor end one, as
/// `claude --help` lists them (2.1.295, and 2.1.296 the same). Those that
/// end one are `ON_SESSIONS`; its others -- `agents`, `attach`,
/// `ultrareview` -- start or open one, and any other first operand is a
/// prompt.
const NO_SESSION: &[&str] =
    &["auth", "auto-mode", "doctor", "gateway", "import", "install", "logs", "mcp", "plugin", "plugins", "setup-token", "update", "upgrade"];

/// Claude Code's commands that stop, restart or remove its background
/// sessions, as `claude --help` lists them (2.1.296): `stop` and its alias
/// `kill`, `rm`, and `respawn`, which restarts one or, with `--all`, all;
/// and `purge`, which deletes a project's transcripts, tasks, file history
/// and config entry, or every project's (task 1742), but with `--dry-run`.
const ON_SESSIONS: &[&str] = &["stop", "kill", "rm", "respawn", "purge"];

/// The multiplexers ekko agents speaks to by name; the binary `EKKO_MUX`
/// names is one too.
const MUXES: &[&str] = &["tmux", "kmux", "ztmux"];

/// tmux's commands that kill or respawn, each with its alias, as `tmux
/// list-commands` lists them (3.7c; kmux 3.7.47 the same).
const KILLS: &[(&str, Option<&str>)] = &[
    ("kill-pane", Some("killp")),
    ("kill-server", None),
    ("kill-session", None),
    ("kill-window", Some("killw")),
    ("respawn-pane", Some("respawnp")),
    ("respawn-window", Some("respawnw")),
];

/// What tmux's other commands do that a born session leaves to others
/// (task 1742, the user's answer 1740), as their usage lines in `list-commands`
/// read: tmux 3.7c's, and kmux 3.7.47's, which takes a value after more
/// options and has two commands more.
struct Reach {
    name: &'static str,
    /// The options it takes a value after, in tmux or kmux: either refuses
    /// an option it does not know, and runs nothing.
    valued: &'static str,
    /// Kills, given `-k`, the window it would replace or leave.
    kills_with_k: bool,
    /// Runs a command it is given (`Limit::Runs`): the operand past this
    /// many, or the value of this option.
    runs_past: Option<usize>,
    runs_by: Option<char>,
    /// Sets the option its first operand names, which may hold a command
    /// (`RUN_OPTIONS`).
    sets: bool,
    /// Types into the pane its `-t` names (`Limit::Keys`).
    types: bool,
}

impl Reach {
    const fn of(name: &'static str, valued: &'static str) -> Reach {
        Reach { name, valued, kills_with_k: false, runs_past: None, runs_by: None, sets: false, types: false }
    }

    const fn runs(name: &'static str, valued: &'static str, past: usize) -> Reach {
        Reach { runs_past: Some(past), ..Reach::of(name, valued) }
    }
}

/// Each command whose usage takes a command to run -- a `command`, a
/// `shell-command`, a `template` -- or, for `source-file`, a file of them;
/// those whose `-k` kills a window; those that set an option; and those that
/// type into a pane.
const REACHES: &[Reach] = &[
    Reach::runs("bind-key", "TN", 1),
    Reach::runs("choose-buffer", "FfKOt", 0),
    Reach::runs("choose-client", "FfKOt", 0),
    Reach::runs("choose-tree", "FfKOt", 0),
    Reach::runs("command-prompt", "IptT", 0),
    Reach::runs("confirm-before", "cpt", 0),
    Reach { runs_by: Some('E'), ..Reach::of("detach-client", "Est") },
    Reach::runs("display-menu", "bcCHsStTxy", 2),
    Reach::runs("display-panes", "dt", 0),
    Reach::runs("display-popup", "bcdehsStTwxy", 0),
    Reach::runs("if-shell", "t", 0),
    Reach { kills_with_k: true, ..Reach::of("link-window", "st") },
    Reach { kills_with_k: true, ..Reach::of("move-window", "st") },
    Reach::runs("new-pane", "BceFlmpsSRTxyXYt", 0),
    Reach::runs("new-session", "ceFfnstxy", 0),
    Reach { kills_with_k: true, ..Reach::runs("new-window", "ceFnt", 0) },
    Reach { types: true, ..Reach::of("paste-buffer", "sbt") },
    Reach::runs("pipe-pane", "t", 0),
    Reach::runs("run-shell", "cdt", 0),
    Reach { types: true, ..Reach::of("send-keys", "cNt") },
    Reach { types: true, ..Reach::of("send-prefix", "t") },
    Reach::runs("set-hook", "t", 1),
    Reach { sets: true, ..Reach::of("set-option", "t") },
    Reach { sets: true, ..Reach::of("set-window-option", "t") },
    Reach::runs("source-file", "t", 0),
    Reach::runs("split-window", "ceFlmpsSRTt", 0),
    Reach::runs("switch-mode", "Ft", 0),
    Reach { kills_with_k: true, ..Reach::of("unlink-window", "t") },
    Reach::runs("znative", "", 0),
];

/// The options whose value tmux runs, as `show-hooks -g`, `-gw`, `show -s`
/// and `show -g` list them (3.7c; kmux 3.7.47 the same): its hooks, which
/// `set-option` sets as `set-hook` does, and those that hold a command or a
/// shell to run. A format's `#()` runs one too, in any option or argument.
const RUN_OPTIONS: &str = "after-bind-key after-capture-pane after-copy-mode after-display-message after-display-panes \
    after-kill-pane after-list-buffers after-list-clients after-list-keys after-list-panes after-list-sessions \
    after-list-windows after-load-buffer after-lock-server after-new-session after-new-window after-paste-buffer \
    after-pipe-pane after-queue after-refresh-client after-rename-session after-rename-window after-resize-pane \
    after-resize-window after-save-buffer after-select-layout after-select-pane after-select-window after-send-keys \
    after-set-buffer after-set-environment after-set-hook after-set-option after-show-environment after-show-messages \
    after-show-options after-split-window after-unbind-key alert-activity alert-bell alert-silence client-active \
    client-attached client-detached client-focus-in client-focus-out client-resized client-session-changed \
    client-light-theme client-dark-theme command-error session-closed session-created session-renamed \
    session-window-changed window-linked window-unlinked pane-died pane-exited pane-focus-in pane-focus-out \
    pane-mode-changed pane-set-clipboard pane-title-changed window-layout-changed window-pane-changed window-renamed \
    window-resized \
    command-alias copy-command default-client-command default-command default-shell editor lock-command";

/// tmux's commands, each with its alias, as `tmux list-commands -F
/// '#{command_list_name}:#{command_list_alias}'` lists them (3.7c), and the
/// two kmux 3.7.47 has more, switch-mode and znative: what a word of their
/// command line may name.
const TMUX_COMMANDS: &str = "attach-session:attach bind-key:bind break-pane:breakp capture-pane:capturep choose-buffer: \
    choose-client: choose-tree: clear-history:clearhist clear-prompt-history:clearphist clock-mode: command-prompt: \
    confirm-before:confirm copy-mode: customize-mode: delete-buffer:deleteb detach-client:detach display-menu:menu \
    display-message:display display-popup:popup display-panes:displayp find-window:findw has-session:has if-shell:if \
    join-pane:joinp kill-pane:killp kill-server: kill-session: kill-window:killw last-pane:lastp last-window:last \
    link-window:linkw list-buffers:lsb list-clients:lsc list-commands:lscm list-keys:lsk list-panes:lsp list-sessions:ls \
    list-windows:lsw load-buffer:loadb lock-client:lockc lock-server:lock lock-session:locks move-pane:movep \
    move-window:movew new-pane:newp new-session:new new-window:neww next-layout:nextl next-window:next \
    paste-buffer:pasteb pipe-pane:pipep previous-layout:prevl previous-window:prev refresh-client:refresh \
    rename-session:rename rename-window:renamew resize-pane:resizep resize-window:resizew respawn-pane:respawnp \
    respawn-window:respawnw rotate-window:rotatew run-shell:run save-buffer:saveb select-layout:selectl \
    select-pane:selectp select-window:selectw send-keys:send send-prefix: server-access: set-buffer:setb \
    set-environment:setenv set-hook: set-option:set set-window-option:setw show-buffer:showb show-environment:showenv \
    show-hooks: show-messages:showmsgs show-options:show show-prompt-history:showphist show-window-options:showw \
    source-file:source split-window:splitw start-server:start suspend-client:suspendc swap-pane:swapp \
    swap-window:swapw switch-client:switchc switch-mode: unbind-key:unbind unlink-window:unlinkw wait-for:wait znative:";

/// tmux's command aliases as it starts (3.7c's command-alias option), which
/// it reads before its commands' names: each, and the words it stands for.
/// One the user adds is not known here.
const TMUX_ALIASES: &[(&str, &[&str])] = &[
    ("split-pane", &["split-window"]),
    ("splitp", &["split-window"]),
    ("server-info", &["show-messages", "-JT"]),
    ("info", &["show-messages", "-JT"]),
    ("choose-window", &["choose-tree", "-w"]),
    ("choose-session", &["choose-tree", "-s"]),
];

/// The options tmux takes a value after, before its command: tmux.c's
/// getopt string is "2c:CDdf:hlL:NqS:T:uUvV". Its `-c` runs a shell command.
const MUX_VALUED: &str = "cfLST";

/// The command tmux runs for `word`, as its `cmd_find` finds one: by its
/// alias, by its name, or by a start of its name that no other name has.
/// None for a start several names share, which tmux refuses as ambiguous.
fn tmux_command(word: &str) -> Option<&'static str> {
    let table = || TMUX_COMMANDS.split_whitespace().filter_map(|entry| entry.split_once(':'));
    if let Some((name, _)) = table().find(|(_, alias)| *alias == word) {
        return Some(name);
    }
    let starting: Vec<&'static str> = table().map(|(name, _)| name).filter(|name| name.starts_with(word)).collect();
    match starting.as_slice() {
        [one] => Some(one),
        _ => starting.iter().find(|name| **name == word).copied(),
    }
}

/// A command's arguments as tmux's getopt reads them, `valued` the options
/// it takes a value after: its flags, its options' values, and its operands,
/// from the first word that is no option, or after `--`.
fn getopt<'a>(args: &[&'a str], valued: &str) -> (Vec<char>, Vec<(char, &'a str)>, Vec<&'a str>) {
    let (mut flags, mut values) = (Vec::new(), Vec::new());
    let mut at = 0;
    while let Some(arg) = args.get(at) {
        at += 1;
        if *arg == "--" {
            break;
        }
        let Some(cluster) = arg.strip_prefix('-').filter(|cluster| !cluster.is_empty()) else {
            at -= 1;
            break;
        };
        for (index, flag) in cluster.char_indices() {
            if valued.contains(flag) {
                // The rest of its word, or else the next word.
                let rest = &cluster[index + flag.len_utf8()..];
                let value = if rest.is_empty() {
                    at += 1;
                    args.get(at - 1).copied().unwrap_or_default()
                } else {
                    rest
                };
                values.push((flag, value));
                break;
            }
            flags.push(flag);
        }
    }
    (flags, values, args.get(at..).unwrap_or_default().to_vec())
}

/// What a born session leaves to others that a multiplexer given `args`
/// would do, from the pane `own` (TMUX_PANE), read as tmux reads its
/// command line: its options as getopt reads them, then a command and its
/// arguments, and another command after each argument that ends in a `;`
/// not escaped -- `\;` in the shell. A word names the command `tmux_command`
/// finds for it, after tmux's own aliases. Each command is read by what
/// its usage says it takes (`REACHES`): a command it kills (`KILLS`), a
/// window its `-k` kills, a command it is given to run -- as an operand, an
/// option's value, an option it sets, a copy mode pipe, a format's `#()` --
/// a pane other than `own` it types into -- any pane, on a server another
/// `-L` or `-S` names.
fn mux_limit(args: &[String], own: Option<&str>) -> Option<Limit> {
    let mut at = 0;
    let mut elsewhere = false;
    while let Some(arg) = args.get(at) {
        at += 1;
        if arg == "--" {
            break;
        }
        let Some(flags) = arg.strip_prefix('-').filter(|flags| !flags.is_empty()) else {
            at -= 1;
            break;
        };
        // An option that takes a value takes the rest of its word, or else
        // the next word.
        if let Some(valued) = flags.find(|flag| MUX_VALUED.contains(flag)) {
            match flags.as_bytes()[valued] {
                b'c' => return Some(Limit::Runs),
                b'L' | b'S' => elsewhere = true,
                _ => {}
            }
            if valued == flags.len() - 1 {
                at += 1;
            }
        }
    }
    if let Some(first) = args.get(at) {
        let word = if first.ends_with("\\;") { first.as_str() } else { first.trim_end_matches(';') };
        if !word.is_empty() && mux_verb(word) {
            return Some(Limit::Verb);
        }
    }
    let mut command: Vec<&str> = Vec::new();
    for arg in args.iter().skip(at) {
        let ends = arg.ends_with(';') && !arg.ends_with("\\;");
        let word = if ends { &arg[..arg.len() - 1] } else { arg.as_str() };
        if !word.is_empty() || !ends {
            command.push(word);
        }
        if ends {
            if let Some(limit) = command_limit(&command, elsewhere, own) {
                return Some(limit);
            }
            command.clear();
        }
    }
    command_limit(&command, elsewhere, own)
}

/// kmux's own verbs that tmux reads as a command of its, as `kmux verbs`
/// lists them (3.7.47): `info`, one of tmux's aliases, which kmux runs as
/// its verb. Every other verb of kmux is no tmux command at all.
const SHADOWING_VERBS: &[&str] = &["info"];

/// Whether a multiplexer given `word` first after its options runs no tmux
/// command (task 1746, the user's decision 1910): one of kmux's own verbs,
/// which it runs on that word exactly before tmux reads its command line,
/// or a word tmux reads as none -- which tmux refuses, and a kmux newer than
/// this guard may run as a verb of its own.
fn mux_verb(word: &str) -> bool {
    SHADOWING_VERBS.contains(&word) || (!TMUX_ALIASES.iter().any(|(alias, _)| *alias == word) && tmux_command(word).is_none())
}

/// What one tmux command, `words` its name and arguments, would do that a
/// born session leaves to others; see `mux_limit`.
fn command_limit(words: &[&str], elsewhere: bool, own: Option<&str>) -> Option<Limit> {
    let (first, rest) = words.split_first()?;
    let expanded: Vec<&str>;
    let (name, rest) = match TMUX_ALIASES.iter().find(|(alias, _)| alias == first) {
        Some((_, stands)) => {
            expanded = stands[1..].iter().copied().chain(rest.iter().copied()).collect();
            (stands[0], expanded.as_slice())
        }
        None => (tmux_command(first)?, rest),
    };
    if KILLS.iter().any(|(kill, _)| *kill == name) {
        return Some(Limit::Kill);
    }
    // A format's `#()` runs its shell command wherever the format is
    // expanded.
    if rest.iter().any(|word| word.contains("#(")) {
        return Some(Limit::Runs);
    }
    let reach = REACHES.iter().find(|reach| reach.name == name)?;
    let (flags, values, operands) = getopt(rest, reach.valued);
    if reach.kills_with_k && flags.contains(&'k') {
        return Some(Limit::Kill);
    }
    let option = operands.first().filter(|_| reach.sets).map(|option| option.split('[').next().unwrap_or_default());
    // send-keys -X runs a copy mode command, and copy-pipe and its kin pipe
    // to the command they are given.
    let pipes = reach.types && flags.contains(&'X') && operands.first().is_some_and(|command| command.contains("pipe")) && operands.len() > 1;
    if reach.runs_past.is_some_and(|past| operands.len() > past)
        || reach.runs_by.is_some_and(|by| values.iter().any(|(flag, _)| *flag == by))
        || option.is_some_and(|option| RUN_OPTIONS.split_whitespace().any(|runs| runs == option))
        || pipes
    {
        return Some(Limit::Runs);
    }
    let target = values.iter().rev().find(|(flag, _)| *flag == 't').map(|(_, target)| *target);
    (reach.types && (elsewhere || target.is_some_and(|target| Some(target) != own))).then_some(Limit::Keys)
}

/// Whether a `kill`, `pkill`, `killall` or `skill` given `args` signals a
/// process (`Limit::Signal`): any but one that lists the signals -- `kill`
/// whose first argument is `-l`, `killall -l` -- or asks for its help or its
/// version. One with no argument is refused too, as `xargs kill` is: xargs
/// gives it the pids or names the guard does not see. pkill's `-l` lists
/// what it kills.
fn signals(name: &str, args: &[String]) -> bool {
    let only = |harmless: &[&str]| !args.is_empty() && args.iter().all(|arg| harmless.contains(&arg.as_str()));
    match name {
        "kill" => !args.first().is_some_and(|first| matches!(first.as_str(), "-l" | "-L" | "--list" | "--table" | "-h" | "--help" | "-V" | "--version")),
        "killall" => !only(&["-l", "--list", "-h", "--help", "-V", "--version"]),
        _ => !only(&["-h", "--help", "-V", "--version"]),
    }
}

/// Whether a `kill` given `args`, where `place` says, signals no process but
/// the session's own (task 1849): it sends signal 0, which signals none and
/// asks whether a process is there, or every pid it is given is the
/// session's -- `$!`, the last the command put in the background; `$$` or
/// `$BASHPID`, its shell; a job, as `%1`; a variable that holds one of
/// those; or a process below `claude`, the Claude Code process its tools
/// run under. A negative pid, a process group, is no one's own, and nor is
/// what pkill, killall and skill are given, a pattern or names.
fn own_signal(name: &str, args: &[String], place: &crate::shell::Located, claude: Option<u32>) -> bool {
    if name != "kill" {
        return false;
    }
    // The signal its first argument names, and where its pids start.
    let (signal, mut at) = match args.first().map(String::as_str) {
        Some("-s" | "-n" | "--signal") => (args.get(1).map(String::as_str), 2),
        Some(first) if first.starts_with("--signal=") => (first.strip_prefix("--signal="), 1),
        Some("--") | None => (None, 0),
        Some(first) if first.starts_with('-') && !first.starts_with("--") => (Some(&first[1..]), 1),
        Some(first) if first.starts_with('-') => return false,
        Some(_) => (None, 0),
    };
    if signal == Some("0") {
        return true;
    }
    if args.get(at).is_some_and(|arg| arg == "--") {
        at += 1;
    }
    let pids = &args[at.min(args.len())..];
    !pids.is_empty() && pids.iter().all(|pid| own_pid(pid, &place.variables, claude, 0))
}

/// Whether `pid`, as a kill is given it, names a process of the session's
/// own; see `own_signal`.
fn own_pid(pid: &str, variables: &std::collections::HashMap<String, String>, claude: Option<u32>, depth: usize) -> bool {
    if matches!(pid, "$!" | "${!}" | "$$" | "${$}" | "$BASHPID" | "${BASHPID}") || pid.starts_with('%') {
        return true;
    }
    let name = pid.strip_prefix("${").and_then(|name| name.strip_suffix('}')).or_else(|| pid.strip_prefix('$'));
    if let Some(name) = name.filter(|name| !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')) {
        return depth < 4 && variables.get(name).is_some_and(|value| own_pid(value, variables, claude, depth + 1));
    }
    match (pid.parse::<u32>(), claude) {
        (Ok(pid), Some(claude)) => below(pid, claude),
        _ => false,
    }
}

/// Whether the process `pid` runs below `ancestor`, by /proc: one of its
/// children, theirs, and so on -- `ancestor` itself is not.
fn below(mut pid: u32, ancestor: u32) -> bool {
    for _ in 0..64 {
        match crate::holder::stat(pid) {
            Some((parent, _)) if parent == ancestor => return true,
            Some((parent, _)) if parent > 1 => pid = parent,
            _ => return false,
        }
    }
    false
}

/// Who makes the calls the limits judge: a session ekko agents started, as
/// its environment says (`Caller::of_this_process`).
#[derive(Debug, Clone, Default)]
pub struct Caller {
    /// The name of the binary `EKKO_MUX` names, as a call names it: `kmux`.
    pub mux: Option<String>,
    /// The multiplexer's pane it runs in, TMUX_PANE.
    pub pane: Option<String>,
    /// The worktree and branch `start` named, where it named them.
    pub checkout: Option<Own>,
    /// The Claude Code process it runs in: what runs below it is the
    /// session's own to signal (task 1849).
    pub claude: Option<u32>,
}

impl Caller {
    /// The session this process -- the guard's hook -- runs in.
    fn of_this_process() -> Caller {
        Caller {
            mux: mux_name(),
            pane: set("TMUX_PANE").map(|pane| pane.to_string_lossy().into_owned()),
            checkout: Own::of_this_process(),
            claude: crate::holder::claude_among(std::os::unix::process::parent_id()),
        }
    }
}

/// The limit the call `call` reaches, if any, run where `place` says, by
/// the session `caller`.
pub fn limit_of(call: &crate::shell::Call, place: &crate::shell::Located, caller: &Caller) -> Option<Limit> {
    let operands = |valued: &[&str]| crate::shell::operands(&call.args, valued);
    let (mux, own) = (caller.mux.as_deref(), caller.pane.as_deref());
    let found = match call.name.as_str() {
        "git" => git_limit(&call.args, place, caller, 0),
        "gh" => match operands(&["-R", "--repo"]).as_slice() {
            ["pr", "create" | "merge", ..] => Some(Limit::Push),
            ["release", "create" | "upload" | "edit" | "delete" | "delete-asset", ..] => Some(Limit::Release),
            _ => None,
        },
        "cargo" => {
            let operands = operands(&["--manifest-path", "-Z", "--config", "-C", "--color"]);
            (operands.iter().find(|operand| !operand.starts_with('+')) == Some(&"publish")).then_some(Limit::Release)
        }
        // `yarn npm publish` and `npm run publish` too.
        "npm" | "pnpm" | "yarn" => operands(&[]).iter().take(2).any(|operand| *operand == "publish").then_some(Limit::Release),
        "nixos-rebuild" => {
            let operands = operands(&["--flake", "-I", "--target-host", "--build-host", "--profile-name", "-p", "--specialisation", "-c"]);
            operands.iter().any(|operand| matches!(*operand, "switch" | "boot" | "test")).then_some(Limit::Release)
        }
        "ekko" => match operands(&[]).as_slice() {
            ["agents", "start", ..] => Some(Limit::Session),
            ["agents", "merge", ..] => Some(Limit::Push),
            _ => None,
        },
        "claude" => {
            let asks = call.args.iter().any(|arg| matches!(arg.as_str(), "--version" | "-v" | "--help" | "-h"));
            match operands(CLAUDE_VALUED).first() {
                _ if asks => None,
                Some(&"purge") if call.args.iter().any(|arg| arg == "--dry-run") => None,
                Some(command) if ON_SESSIONS.contains(command) => Some(Limit::Stop),
                Some(command) if NO_SESSION.contains(command) => None,
                _ => Some(Limit::Session),
            }
        }
        "bash" | "sh" | "zsh" | "dash" => {
            let script = operands(&[]).first().map(|script| Path::new(script).file_name().unwrap_or_default().to_string_lossy().into_owned());
            script.filter(|script| script.starts_with("release")).map(|_| Limit::Release)
        }
        name if name.starts_with("release") => Some(Limit::Release),
        name @ ("kill" | "pkill" | "killall" | "skill") => {
            (signals(name, &call.args) && !own_signal(name, &call.args, place, caller.claude)).then_some(Limit::Signal)
        }
        name if MUXES.contains(&name) || mux == Some(name) => mux_limit(&call.args, own),
        _ => None,
    };
    found.or_else(|| caller.checkout.as_ref().and_then(|checkout| write_limit(call, place, checkout)))
}

/// The programs that write files a Bash call names (task 1925), each with
/// the options it takes a value after, as its `--help` lists them (GNU
/// coreutils 9.7 and sed 4.9): those whose operands are each a file they
/// write -- or, as `mv` does, take away -- and those that write the last,
/// or the folder `-t` names; `sed` with `-i`, and `dd` its `of=`.
const WRITERS: &[(&str, &[&str])] = &[
    ("sed", &["-e", "--expression", "-f", "--file", "-l", "--line-length"]),
    ("dd", &[]),
    ("cp", &["-S", "--suffix", "-t", "--target-directory"]),
    ("install", &["-m", "--mode", "-o", "--owner", "-g", "--group", "-S", "--suffix", "-t", "--target-directory", "--strip-program"]),
    ("ln", &["-S", "--suffix", "-t", "--target-directory"]),
    ("mv", &["-S", "--suffix", "-t", "--target-directory"]),
    ("rm", &[]),
    ("rmdir", &[]),
    ("unlink", &[]),
    ("shred", &["-n", "--iterations", "-s", "--size", "--random-source"]),
    ("touch", &["-d", "--date", "-r", "--reference", "-t"]),
    ("mkdir", &["-m", "--mode"]),
    ("truncate", &["-s", "--size", "-r", "--reference"]),
    ("tee", &[]),
];

/// The files the call `call` writes by what it is given, as written: see
/// `WRITERS`.
fn written_files(call: &crate::shell::Call) -> Vec<&str> {
    let args = &call.args;
    let Some((name, valued)) = WRITERS.iter().find(|(name, _)| *name == call.name) else { return Vec::new() };
    let operands = || crate::shell::operands(args, valued);
    let target = || crate::claims::given(args, &["-t", "--target-directory"]);
    match *name {
        "sed" => {
            let in_place = args.iter().take_while(|arg| *arg != "--").any(|arg| {
                arg.starts_with("--in-place")
                    || arg.strip_prefix('-').filter(|short| !short.starts_with('-')).is_some_and(|short| short.split_once('i').is_some_and(|(before, _)| before.chars().all(|c| "nErsuz".contains(c))))
            });
            let scripted = flagged(args, &["-e", "--expression", "-f", "--file"]);
            if in_place { operands().into_iter().skip(usize::from(!scripted)).collect() } else { Vec::new() }
        }
        "dd" => args.iter().filter_map(|arg| arg.strip_prefix("of=")).collect(),
        "cp" | "install" | "ln" if target().is_some() => target().into_iter().collect(),
        "install" if flagged(args, &["-d", "--directory"]) || clustered(args, &['d']) => operands(),
        "cp" | "install" => operands().last().copied().into_iter().collect(),
        // `ln -s target` makes its link here.
        "ln" => match operands().as_slice() {
            [_] => vec!["."],
            operands => operands.last().copied().into_iter().collect(),
        },
        "mv" => operands().into_iter().chain(target()).collect(),
        _ => operands(),
    }
}

/// The limit a born session working in `own` reaches with the files the
/// call `call`, run where `place` says, writes (task 1925): by its
/// redirections, or by what it is given (`written_files`), each judged as
/// an Edit of it is (`file_limit`). A path no variable the command or the
/// environment sets resolves goes unjudged, and a device is no file.
fn write_limit(call: &crate::shell::Call, place: &crate::shell::Located, own: &Own) -> Option<Limit> {
    let mut words: Vec<&str> = call.writes.iter().map(String::as_str).collect();
    words.extend(written_files(call));
    words
        .into_iter()
        .filter(|word| !word.starts_with("/dev/"))
        .flat_map(|word| crate::shell::expand(word, &place.variables, &place.folder))
        .find_map(|path| file_limit(path.to_str()?, &place.folder, own))
}

/// The worktree and branch `start` gave a born session (task 1800), and
/// the GIT_DIR and GIT_WORK_TREE a command sets before a git call of its,
/// which move where that call writes.
#[derive(Debug, Clone, Default)]
pub struct Own {
    pub worktree: PathBuf,
    pub branch: String,
    pub git_env: Vec<(String, String)>,
}

impl Own {
    /// What WORKTREE_VAR and BRANCH_VAR name, both set: none in the
    /// orchestrator, nor in a session an ekko that set neither started, which
    /// the limits on its checkout then leave free.
    pub fn of_this_process() -> Option<Own> {
        let worktree = PathBuf::from(set(WORKTREE_VAR)?);
        let branch = set(BRANCH_VAR)?.to_string_lossy().into_owned();
        Some(Own { worktree, branch, git_env: Vec::new() })
    }

    /// Whether the ref `name` is this session's branch, as a command names it.
    fn owns(&self, name: &str) -> bool {
        name == self.branch || name.strip_prefix("refs/heads/") == Some(self.branch.as_str())
    }
}

/// What a git command writes, as `git_touches` reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Touches {
    /// Nothing of the repository: it reads, or makes another one.
    Nothing,
    /// The checkout it runs in -- its files, index, HEAD and the branch
    /// checked out there -- and what is only that worktree's.
    Checkout,
    /// What every checkout of the repository shares, or another branch
    /// than the session's, or which branch the checkout is on.
    Shared,
    /// The stash, which every checkout shares too.
    Stash,
}

/// Git's commands that write nothing of the repository they run in: they
/// read it, or make another (`clone`, `init`).
const GIT_READS: &[&str] = &[
    "status", "log", "show", "diff", "rev-parse", "rev-list", "ls-files", "ls-tree", "ls-remote", "blame", "annotate", "grep",
    "describe", "merge-base", "for-each-ref", "show-ref", "show-branch", "cat-file", "shortlog", "whatchanged", "diff-tree",
    "diff-index", "diff-files", "name-rev", "range-diff", "cherry", "var", "help", "version", "count-objects", "fsck",
    "verify-commit", "verify-tag", "check-ignore", "check-attr", "check-ref-format", "archive", "format-patch", "bugreport",
    "clone", "init",
];

/// The ref names only one worktree has: pseudorefs as ORIG_HEAD, and
/// those under refs/bisect, refs/worktree and refs/rewritten (gitglossary,
/// "per-worktree ref").
fn per_worktree(name: &str) -> bool {
    let pseudo = !name.contains('/') && name.ends_with("HEAD") && name.chars().all(|c| c.is_ascii_uppercase() || c == '_');
    pseudo || ["refs/bisect/", "refs/worktree/", "refs/rewritten/"].iter().any(|prefix| name.starts_with(prefix))
}

/// Whether `rest` holds one of `flags` before a `--`, alone or, for a long
/// one, with `=value`.
fn flagged(rest: &[String], flags: &[&str]) -> bool {
    rest.iter().take_while(|arg| *arg != "--").any(|arg| {
        flags.iter().any(|flag| arg == flag || (flag.starts_with("--") && arg.strip_prefix(flag).is_some_and(|value| value.starts_with('='))))
    })
}

/// Whether `rest` holds, before a `--`, a cluster of short options -- `-df`
/// -- with one of `letters`.
fn clustered(rest: &[String], letters: &[char]) -> bool {
    rest.iter().take_while(|arg| *arg != "--").any(|arg| {
        arg.strip_prefix('-').filter(|short| !short.starts_with('-')).is_some_and(|short| short.chars().all(|c| c.is_ascii_alphabetic()) && short.contains(letters))
    })
}

/// What the git command `git`, given `rest` and run in `folder`, writes, in
/// a session whose branch `own` names (task 1800). A command it does not
/// know writes the checkout: in the session's own worktree, it is free.
fn git_touches(git: &str, rest: &[String], folder: &Path, own: &Own) -> Touches {
    use Touches::{Checkout, Nothing, Shared};
    let operands = |valued: &[&str]| crate::shell::operands(rest, valued);
    let shared_unless = |name: Option<&str>| if name.is_some_and(|name| own.owns(name)) { Checkout } else { Shared };
    if GIT_READS.contains(&git) {
        return Nothing;
    }
    match git {
        "branch" => {
            if flagged(rest, &["-d", "-D", "--delete", "-m", "-M", "--move", "-c", "-C", "--copy"]) || clustered(rest, &['d', 'D', 'm', 'M', 'c', 'C']) {
                Shared
            } else if let Some(name) = crate::claims::created_branch("branch", rest) {
                shared_unless(Some(name))
            } else if flagged(rest, &["-u", "--set-upstream-to", "--unset-upstream", "--edit-description"]) {
                Checkout
            } else {
                Nothing
            }
        }
        "tag" => {
            let lists = ["-l", "--list", "-v", "--verify", "--contains", "--no-contains", "--merged", "--no-merged", "--points-at"];
            if flagged(rest, &["-d", "--delete"]) || clustered(rest, &['d']) {
                Shared
            } else if flagged(rest, &lists)
                || rest.iter().any(|arg| arg.starts_with("-n"))
                || operands(&["-m", "-F", "-u", "--local-user", "--cleanup", "--sort", "--format", "--color", "--trailer"]).is_empty()
            {
                Nothing
            } else {
                Shared
            }
        }
        "update-ref" => {
            if flagged(rest, &["--stdin"]) {
                return Shared;
            }
            match operands(&["-m"]).first() {
                None => Nothing,
                Some(&"HEAD") if flagged(rest, &["--no-deref"]) => Shared,
                Some(&"HEAD") => Checkout,
                Some(name) if own.owns(name) || per_worktree(name) => Checkout,
                Some(_) => Shared,
            }
        }
        "symbolic-ref" => {
            if flagged(rest, &["-d", "--delete"]) {
                return Shared;
            }
            match operands(&["-m"]).as_slice() {
                [] | [_] => Nothing,
                ["HEAD", target] if own.owns(target) => Checkout,
                _ => Shared,
            }
        }
        "checkout" => {
            if let Some(name) = crate::claims::given(rest, &["-b", "-B", "--orphan"]) {
                return shared_unless(Some(name));
            }
            if flagged(rest, &["--detach"]) {
                return Shared;
            }
            if flagged(rest, &["-p", "--patch", "--pathspec-from-file"]) || rest.iter().any(|arg| arg == "--") {
                return Checkout;
            }
            match operands(&[]).first() {
                None => Checkout,
                Some(first) if own.owns(first) || *first == "HEAD" => Checkout,
                // A path, whose files it restores.
                Some(first) if *first != "-" && folder.join(first).exists() => Checkout,
                Some(_) => Shared,
            }
        }
        "switch" => {
            if let Some(name) = crate::claims::given(rest, &["-c", "-C", "--create", "--force-create"]) {
                return shared_unless(Some(name));
            }
            if flagged(rest, &["-d", "--detach", "--orphan"]) {
                return Shared;
            }
            match operands(&[]).first() {
                None => Checkout,
                first => shared_unless(first.copied()),
            }
        }
        "rebase" => {
            if flagged(rest, &["--update-refs"]) {
                return Shared;
            }
            let given = operands(&["--onto", "-s", "--strategy", "-X", "--strategy-option", "-x", "--exec", "-C", "--whitespace"]);
            let branch = if flagged(rest, &["--root"]) { given.first() } else { given.get(1) };
            match branch {
                Some(branch) if *branch != "HEAD" => shared_unless(Some(branch)),
                _ => Checkout,
            }
        }
        "fetch" | "pull" => {
            let valued = [
                "--depth", "--deepen", "--shallow-since", "--shallow-exclude", "-j", "--jobs", "--negotiation-tip", "--refmap", "-o",
                "--server-option", "--upload-pack", "-s", "--strategy", "-X", "--strategy-option",
            ];
            let given = operands(&valued);
            let writes = given.iter().skip(1).filter_map(|spec| spec.trim_start_matches('+').split_once(':').map(|(_, to)| to));
            if writes.into_iter().any(|to| !to.is_empty() && !to.starts_with("refs/remotes/") && !own.owns(to)) {
                Shared
            } else if git == "pull" {
                Checkout
            } else {
                Nothing
            }
        }
        "stash" => match operands(&[]).first() {
            Some(&("list" | "show")) => Nothing,
            Some(&("apply" | "create")) => Checkout,
            // refs/stash is every worktree's.
            _ => Touches::Stash,
        },
        "notes" => match operands(&["--ref"]).first() {
            None | Some(&("list" | "show" | "get-ref")) => Nothing,
            Some(_) => Shared,
        },
        "replace" => {
            if flagged(rest, &["-l", "--list"]) || operands(&["--format"]).is_empty() {
                Nothing
            } else {
                Shared
            }
        }
        "worktree" => match operands(&[]).first() {
            Some(&"list") => Nothing,
            _ => Shared,
        },
        "remote" => match operands(&[]).first() {
            None | Some(&("show" | "get-url" | "update")) => Nothing,
            Some(_) => Shared,
        },
        "reflog" => match operands(&[]).first() {
            Some(&("expire" | "delete" | "drop")) => Shared,
            _ => Nothing,
        },
        "config" => {
            if flagged(rest, &["--global", "--system", "--file", "-f", "--blob"]) {
                return Nothing;
            }
            if flagged(rest, &["--worktree"]) {
                return Checkout;
            }
            let given = operands(&["--type", "--default", "--comment", "--value", "--file", "-f", "--blob"]);
            match given.first() {
                Some(&("get" | "list")) => return Nothing,
                Some(&("set" | "unset" | "rename-section" | "remove-section" | "edit")) => return Shared,
                _ => {}
            }
            let writes = ["--unset", "--unset-all", "--add", "--replace-all", "--rename-section", "--remove-section", "-e", "--edit"];
            if flagged(rest, &writes) {
                Shared
            } else if flagged(rest, &["--get", "--get-all", "--get-regexp", "--get-urlmatch", "-l", "--list", "--get-color", "--get-colorbool"]) || given.len() <= 1 {
                Nothing
            } else {
                Shared
            }
        }
        "filter-branch" | "filter-repo" => Shared,
        _ => Checkout,
    }
}

/// The `--git-dir` and `--work-tree` the git call `args` gives before its
/// command, each as one argument, which `repository` hands on.
fn git_globals(args: &[String]) -> Vec<String> {
    let mut globals = Vec::new();
    let mut at = 0;
    while let Some(arg) = args.get(at) {
        if matches!(arg.as_str(), "--git-dir" | "--work-tree") {
            if let Some(value) = args.get(at + 1) {
                globals.push(format!("{arg}={value}"));
            }
            at += 2;
        } else if arg.starts_with("--git-dir=") || arg.starts_with("--work-tree=") {
            globals.push(arg.clone());
            at += 1;
        } else if crate::claims::GIT_VALUED.contains(&arg.as_str()) {
            at += 2;
        } else if arg.starts_with('-') {
            at += 1;
        } else {
            break;
        }
    }
    globals
}

/// Where git, run in `folder` with `globals` and `env`, writes: the
/// repository's common directory, which all its worktrees share, and the
/// top of the checkout, which there is none of inside a `.git`. `None`
/// outside a repository, and where `folder` is none.
fn repository(folder: &Path, globals: &[String], env: &[(String, String)]) -> Option<(PathBuf, Option<PathBuf>)> {
    let ask = |what: &str| -> Option<PathBuf> {
        let mut git = Command::new("git");
        git.current_dir(folder).args(globals).args(["rev-parse", "--path-format=absolute", what]);
        git.envs(env.iter().map(|(name, value)| (name, folder.join(value)))).stdin(Stdio::null()).stderr(Stdio::null());
        let output = git.output().ok().filter(|output| output.status.success())?;
        let path = String::from_utf8(output.stdout).ok()?.trim_end_matches('\n').to_string();
        (!path.is_empty()).then(|| PathBuf::from(path))
    };
    let common = ask("--git-common-dir")?;
    Some((common, ask("--show-toplevel")))
}

/// The limit a write in `folder` reaches, by what `touches` says of it and
/// where git writes from there (`repository`): none in another repository
/// than the session's own -- a scratch one in /tmp, a test's --, the shared
/// one for what every checkout shares, the checkout one for a write in
/// another checkout than its worktree, or inside a `.git`.
fn written(touches: Touches, folder: &Path, globals: &[String], own: &Own) -> Option<Limit> {
    if touches == Touches::Nothing {
        return None;
    }
    let (common, top) = repository(folder, globals, &own.git_env)?;
    let (own_common, _) = repository(&own.worktree, &[], &[])?;
    if !same(&common, &own_common) {
        return None;
    }
    match touches {
        Touches::Shared => Some(Limit::Shared),
        Touches::Stash => Some(Limit::Stash),
        _ => (!top.is_some_and(|top| same(&top, &own.worktree))).then_some(Limit::Checkout),
    }
}

/// The limit the git call `args`, run where `place` says, reaches: a push
/// in any born session; in one `start` named its worktree and branch
/// (`caller.checkout`), a write of what every checkout shares, or in
/// another checkout. An alias is read as the command it stands for (task
/// 1925), `depth` the aliases expanded on the way.
fn git_limit(args: &[String], place: &crate::shell::Located, caller: &Caller, depth: usize) -> Option<Limit> {
    let (git, rest, folder) = crate::claims::git_command(args, &place.folder)?;
    let before = &args[..args.len() - rest.len() - 1];
    if let Some(alias) = git_alias(before, git, &folder).filter(|_| depth < 8) {
        return match alias.strip_prefix('!') {
            // A shell command, which git runs at the top of the checkout.
            Some(shell) => {
                let top = repository(&folder, &git_globals(args), &[]).and_then(|(_, top)| top).unwrap_or(folder);
                reached(shell, &top, caller).into_iter().next()
            }
            None => {
                let words = crate::shell::calls(&format!("git {alias}")).into_iter().next().map(|call| call.args).unwrap_or_default();
                let expanded: Vec<String> = before.iter().chain(&words).chain(rest).cloned().collect();
                git_limit(&expanded, place, caller, depth + 1)
            }
        };
    }
    if git == "push" {
        return Some(Limit::Push);
    }
    let own = caller.checkout.as_ref()?;
    written(git_touches(git, rest, &folder, own), &folder, &git_globals(args), own)
}

/// Git's commands the guard reads by name, beside those of `GIT_READS`:
/// each is git's own, whose name an alias cannot take.
const GIT_OWN: &[&str] = &[
    "push", "branch", "tag", "update-ref", "symbolic-ref", "checkout", "switch", "rebase", "fetch", "pull", "stash", "notes", "replace",
    "worktree", "remote", "reflog", "config", "filter-branch", "commit", "add", "reset", "merge", "cherry-pick", "revert", "restore", "rm",
    "mv", "am", "apply", "clean", "gc", "submodule", "bisect", "sparse-checkout", "maintenance",
];

/// What the alias `git` stands for, where it is one: as a `-c alias.NAME=`
/// before the command gives it -- the last one --, or else the repository's
/// or the user's config, which is asked only for a name git has no command
/// of (`GIT_READS`, `GIT_OWN`).
fn git_alias(before: &[String], git: &str, folder: &Path) -> Option<String> {
    let key = format!("alias.{git}=");
    let given = before.windows(2).filter(|pair| pair[0] == "-c").filter_map(|pair| pair[1].strip_prefix(&key).map(str::to_string)).next_back();
    if given.is_some() || GIT_READS.contains(&git) || GIT_OWN.contains(&git) {
        return given;
    }
    let output = Command::new("git").current_dir(folder).args(["config", "--get", &format!("alias.{git}")]).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    let alias = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (output.status.success() && !alias.is_empty()).then_some(alias)
}

/// Why the guard refuses a born session the Edit or Write of the file
/// `path`, from `cwd` (task 1800): a file in another checkout of its
/// repository than its own worktree. Nothing for a file anywhere else --
/// another repository, a folder that is none, as /tmp is.
pub fn file_limits(path: &str, cwd: &Path) -> Vec<String> {
    Own::of_this_process().and_then(|own| file_limit(path, cwd, &own)).map(reason).into_iter().collect()
}

/// The limit a born session working in `own` reaches when it writes the
/// file `path`, from `cwd`: judged from the nearest folder there is on its
/// way, as the file may not be yet.
fn file_limit(path: &str, cwd: &Path, own: &Own) -> Option<Limit> {
    let path = cwd.join(path);
    let mut folder = path.as_path();
    while !folder.is_dir() {
        folder = folder.parent()?;
    }
    written(Touches::Checkout, folder, &[], own)
}

/// The reason the guard gives for `limit`.
fn reason(limit: Limit) -> String {
    format!("{LIMIT_REASON}: this session, started for its tasks, {}.", limit.why())
}

/// The name of the binary `EKKO_MUX` names, which `start` hands every born
/// session: `kmux` for `/usr/local/bin/kmux`, as a call names it.
fn mux_name() -> Option<String> {
    set("EKKO_MUX").and_then(|mux| Path::new(&mux).file_name().map(|name| name.to_string_lossy().into_owned()))
}

/// Why the guard refuses the Bash call `command`, run from `cwd`, in a
/// session ekko agents started: a reason for each limit its calls reach,
/// read as the guard reads calls (`crate::shell`); none for a call that
/// reaches none, or that only names one -- in an echo, a commit message, a
/// here-document.
pub fn limits(command: &str, cwd: &Path) -> Vec<String> {
    reached(command, cwd, &Caller::of_this_process()).into_iter().map(reason).collect()
}

/// The limits the calls of `command`, run from `cwd`, reach, each once, made
/// by the session `caller`: see `limits`.
fn reached(command: &str, cwd: &Path, caller: &Caller) -> Vec<Limit> {
    let (calls, placed) = crate::shell::located(command, cwd, crate::shell::Reading::GUARD);
    let mut found: Vec<Limit> = Vec::new();
    for place in &placed {
        // The GIT_DIR and GIT_WORK_TREE set before it, which move a git call.
        let mut caller = caller.clone();
        caller.checkout = caller.checkout.map(|mut checkout| {
            for call in &calls[..place.at] {
                let set: Vec<(&str, &str)> = match call.name.as_str() {
                    "=" => call.args.split_first().map(|(name, value)| (name.as_str(), value.first().map_or("", String::as_str))).into_iter().collect(),
                    "export" => call.args.iter().filter_map(|arg| arg.split_once('=')).collect(),
                    _ => Vec::new(),
                };
                for (name, value) in set.into_iter().filter(|(name, _)| matches!(*name, "GIT_DIR" | "GIT_WORK_TREE")) {
                    checkout.git_env.push((name.to_string(), value.to_string()));
                }
            }
            checkout
        });
        if let Some(limit) = limit_of(&calls[place.at], place, &caller).filter(|limit| !found.contains(limit)) {
            found.push(limit);
        }
    }
    found
}

/// How the reason a born session's limit refuses with opens.
pub const LIMIT_REASON: &str = "ekko agents";

/// The argument that runs ekko as a born session's closer, which its hook
/// starts: never typed, as `__ekko_clipboard_daemon` is not.
pub const CLOSER_ARG: &str = "__ekko_close";

/// How long the closer waits after a turn ends before it ends the session:
/// a prompt that reaches it meanwhile keeps it.
const GRACE: Duration = Duration::from_secs(1);

/// The variable that sets GRACE in milliseconds: a test sets it so that a
/// loaded machine does not outrun the window it acts within (task 2022).
const GRACE_VAR: &str = "EKKO_BORN_GRACE_MS";

/// How long the closer waits for the session's process, then its pane, to
/// be gone.
pub(crate) const GONE_WITHIN: Duration = Duration::from_secs(10);

/// The argument that runs ekko as the watcher of a born session's
/// permission prompt, which its hook starts (task 1679): never typed.
pub const ANSWER_ARG: &str = "__ekko_answer";

/// How often the watcher reads Claude Code's record of the session.
const ANSWER_EVERY: Duration = Duration::from_millis(100);

/// What the hook keeps of a born session between its events, in ekko's
/// state directory: how many prompts reached it, whether one reached it
/// once its tasks were finished, what it waits on the user for, what the
/// permission it asks for is for, and the state its pane was last given.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Born {
    prompts: u64,
    talked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    waiting: Option<Waiting>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    asked: Option<String>,
    /// `STATE_OPTION`'s value as the hook last set it (task 1675), so the
    /// multiplexer is called only as it changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    state: Option<String>,
    /// Whether the last turn's end found background work in flight (task
    /// 1686): a turn over a minute ago is then no wait on the user, since
    /// the work's end wakes the session itself.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    background: bool,
    /// The uid of the last handoff of its own the hook stopped the session
    /// at (task 2028), so a later write does not stop it again for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    handoff: Option<String>,
    /// What a later version keeps here that this one does not know, written
    /// back as read; see `Item::unknown`.
    #[serde(flatten)]
    unknown: BTreeMap<String, serde_json::Value>,
}

/// A born session waiting on the user (task 1645): since when, for what,
/// and the name its window was given then, if it was.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Waiting {
    since: i64,
    what: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    window: Option<String>,
    /// What marked it: the notification's type, `api_error`, or `handoff`
    /// (task 2028), which the turn's end that follows does not clear.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kind: Option<String>,
    /// What a later version keeps here that this one does not know, written
    /// back as read; see `Item::unknown`.
    #[serde(flatten)]
    unknown: BTreeMap<String, serde_json::Value>,
}

/// The notifications that say a born session waits on the user, as Claude
/// Code 2.1.295 names them (its hooks page): a permission prompt, or an MCP
/// server's dialog -- ekko's ask among them -- unanswered for about six
/// seconds, and a turn over a minute ago with nothing typed since.
const WAITS_ON_USER: &[&str] = &["permission_prompt", "elicitation_dialog", "elicitation_url_dialog", "idle_prompt"];

/// What a born session's window name ends with while it waits on the user.
pub const WAITING_ON_YOU: &str = " \u{b7} waiting on you";

/// The events of Claude Code the born hook acts on, each of which the
/// plugin runs it on: the prompts and the turn's end (task 1643), the
/// permission asked for, the notifications that the session waits on the
/// user, and the events that say it moved on (task 1645).
pub const EVENTS: &[&str] = &["UserPromptSubmit", "Stop", "StopFailure", "PermissionRequest", "Notification", "PostToolUse", "PostToolUseFailure", "ElicitationResult"];

/// The events that end a turn, as Claude Code's hooks page names them (read
/// 2026-10-18, Claude Code 2.1.302): Stop, and StopFailure, which "runs
/// instead of Stop when the turn ends due to an API error" (task 1798). The
/// born hook acts on each, so a turn that ends either way closes the session
/// or marks it waiting.
pub const TURN_ENDS: &[&str] = &["Stop", "StopFailure"];

/// What the PermissionRequest hook of a born session answers (task 1869,
/// decision 1909): deny, at once, since no one answers its prompts -- the
/// user is not at its window, and the orchestrator's path to them does not
/// fit a prompt's two-minute countdown. The message says what to do instead,
/// by what the prompt was for: Claude Code's hooks page has `message` tell
/// Claude why the permission was denied.
fn denial(tool: &str, input: &serde_json::Value) -> String {
    let command = input["command"].as_str().unwrap_or_default();
    let words: Vec<&str> = command.split(|c: char| c.is_whitespace() || matches!(c, ';' | '&' | '|' | '(' | ')' | '`' | '"' | '\'')).collect();
    let worktree = set(WORKTREE_VAR).map_or_else(|| "your worktree".to_string(), |worktree| worktree.to_string_lossy().into_owned());
    let instead = if tool == "AskUserQuestion" || tool.ends_with("__ask") {
        "Decide a routine choice yourself and record it in a decision note on your task, with ekko's create, kind decision, for the review at merge. \
         A choice only the user can make goes in a handoff on your task"
            .to_string()
    } else if tool == "SendMessage" {
        "To tell another session something, write a note on its task with ekko's create, attached_to that task: ekko tells the session holding it".to_string()
    } else if tool == "Bash" && words.iter().any(|word| matches!(*word, "rm" | "rmdir") || word.ends_with("/rm") || word.ends_with("/rmdir")) {
        format!(
            "Claude Code asks before a removal it reads as of a critical path, or cannot check: remove a literal path inside your worktree, {worktree}, \
             with no variable, glob or substitution in it, as its own command, not inside a bash -c script"
        )
    } else if tool == "Bash" && words.iter().any(|word| word.starts_with("--project")) {
        "Reach this project's board through ekko without --project: a session ekko agents started works on its own board".to_string()
    } else {
        format!("Do it another way that needs no prompt, inside your worktree, {worktree}")
    };
    format!(
        "No one answers this session's prompts: ekko agents started it, and its prompts are denied at once. {instead}. \
         If the task needs this exact call, write it in a handoff on your task, with ekko's create, kind handoff, and end your turn with no text: \
         the orchestrator brings it to the user, and their answer comes back as a note on your task."
    )
}

/// The tools a born session finishes its work with on the board: ekko's
/// writes, through its MCP server -- `mcp__<server>__<tool>`, whatever the
/// server is named where it is registered -- or its command line, in Bash.
fn writes_the_board(tool: &str, input: &serde_json::Value) -> bool {
    match tool.strip_prefix("mcp__").and_then(|rest| rest.rsplit_once("__")) {
        Some((server, name)) => server.contains("ekko") && matches!(name, "create" | "set_state" | "force_state" | "batch" | "edit" | "update"),
        None => tool == "Bash" && input["command"].as_str().is_some_and(|command| command.contains("ekko")),
    }
}

/// The hook of a born session (tasks 1643 and 1645), on each event of
/// `EVENTS`; nothing in a session `start` did not open, which has no
/// EKKO_AGENT_TASK. One run at a time for a session, as Claude Code fires
/// PostToolUse for tools that ran side by side at once.
///
/// A notification that the session waits on the user marks it so (design
/// item 4), with what it waits for -- for a permission, the tool and what
/// it was to do, as the request for it said: on the board, on each of its
/// tasks it holds, and in its window's name. Whichever event comes next --
/// a tool that ran once the user let it, the user's prompt, the turn's
/// end, a dialog answered -- says it moved on, and clears both; for a
/// permission prompt, the watcher it starts says so first, as the user
/// answers (task 1679).
///
/// Each prompt is counted, and whether the session's tasks were all
/// finished -- done or cancelled -- when it came: the user talking to a
/// session whose work is over makes it theirs (design item 2). A prompt
/// typed while a turn runs counts when it is typed, which is when Claude
/// Code 2.1.295 fires the event for it, into the turn that runs. At the end
/// of a turn, a session whose tasks are all finished, that no prompt
/// reached since, is closed by a process of its own, which outlives the
/// hook and stops the session's background work first (task 1829).
///
/// A born session says nothing to the user (decision 1870). Its permission
/// prompts are denied at once, with what to do instead (task 1869). The
/// write that finishes the last of its tasks, each with a closing note from
/// it, ends its processing there -- `continue: false`, so no closing text
/// follows in the pane -- and starts the closer, whether or not Claude Code
/// fires Stop after such a stop (task 1866). A turn an API error ends,
/// StopFailure, closes it as Stop would, or else marks it waiting on the
/// user with the error (task 1798). A write that leaves a handoff of its own
/// on a task it holds marks it waiting, with the handoff, and ends its turn
/// there, starting no closer: it waits for the answer (task 2028).
///
/// Each run makes sure the session's Claude Code process has its watcher
/// (`watch`), which speaks for it once it has ended, and tells the user of
/// a wait gone long (tasks 1827, 1729 and 1832).
pub fn hook(home: &Path, cwd: &Path, input: &str) -> std::process::ExitCode {
    let Some(tasks) = std::env::var(TASKS_VAR).ok().filter(|tasks| !tasks.trim().is_empty()) else {
        return std::process::ExitCode::SUCCESS;
    };
    let input: serde_json::Value = serde_json::from_str(input).unwrap_or_default();
    let session = input["session_id"].as_str().unwrap_or_default();
    let event = input["hook_event_name"].as_str().unwrap_or_default();
    if !EVENTS.contains(&event) {
        return std::process::ExitCode::SUCCESS;
    }
    let _alone = lock_born(home, session);
    let mut born = read_born(home, session);
    // Its watcher, which outlives the session's process (tasks 1827, 1729
    // and 1832): started once for each Claude Code process.
    if let Some(process) = crate::holder::claude_among(std::os::unix::process::parent_id()).and_then(Process::of) {
        if let Err(why) = spawn_watch(home, session, &process) {
            log(home, session, &tasks, &format!("not watched: {why}"));
        }
    }
    match event {
        // Kept for the notification that may follow, whose message names
        // no tool: "Claude needs your permission", in Claude Code 2.1.295.
        // Denied at once (task 1869): no one answers it. Kept all the same,
        // for a version that would show the prompt anyway.
        "PermissionRequest" => {
            let asked = asked_for(&input["tool_name"], &input["tool_input"]);
            born.asked = Some(asked.clone());
            write_born(home, session, &born);
            let tool = input["tool_name"].as_str().unwrap_or_default();
            let message = denial(tool, &input["tool_input"]);
            log(home, session, &tasks, &format!("its permission prompt denied at once: {asked}"));
            println!(
                "{}",
                serde_json::json!({"hookSpecificOutput": {"hookEventName": "PermissionRequest", "decision": {"behavior": "deny", "message": message}}})
            );
            return std::process::ExitCode::SUCCESS;
        }
        "Notification" => {
            let kind = input["notification_type"].as_str().unwrap_or_default();
            if kind == "idle_prompt" && born.background && born.waiting.is_none() {
                log(home, session, &tasks, "does not wait on you (idle_prompt): background work was in flight at its turn's end");
                return std::process::ExitCode::SUCCESS;
            }
            if WAITS_ON_USER.contains(&kind) && born.waiting.is_none() {
                let mut what = input["message"].as_str().map(str::trim).filter(|what| !what.is_empty()).unwrap_or(kind).to_string();
                if let Some(asked) = born.asked.as_ref().filter(|_| kind == "permission_prompt") {
                    what = format!("{what} -- {asked}");
                }
                let what = crate::agent::clip(&what, 200);
                born.waiting = wait_on_user(home, cwd, &tasks, session, kind, &what);
                if born.waiting.is_some() {
                    show_state(home, session, &tasks, &mut born, WAITING, Some(&what));
                }
                write_born(home, session, &born);
                // No event comes as the user answers a permission prompt,
                // until the tool they let run ends (task 1679). Started once
                // the wait is written, which is what it watches.
                if born.waiting.is_some() && kind == "permission_prompt" {
                    let process = crate::holder::claude_among(std::os::unix::process::parent_id()).and_then(Process::of);
                    let watching = process.ok_or_else(|| "no Claude Code process is among its ancestors".to_string()).and_then(|process| spawn_watcher(session, &process));
                    if let Err(why) = watching {
                        log(home, session, &tasks, &format!("its answer is not watched: {why}"));
                    }
                }
            }
            return std::process::ExitCode::SUCCESS;
        }
        _ => {}
    }
    // A handoff's wait outlasts the turn its stop ends (task 2028): the
    // session waits for the answer still.
    let handing_over = born.waiting.as_ref().is_some_and(|waiting| waiting.kind.as_deref() == Some(HANDOFF)) && TURN_ENDS.contains(&event);
    if (born.waiting.is_some() || born.asked.is_some()) && !handing_over {
        born.asked = None;
        if let Some(waiting) = born.waiting.take() {
            let process = crate::holder::claude_among(std::os::unix::process::parent_id()).and_then(Process::of);
            answered(home, cwd, &tasks, session, event, &waiting, process.as_ref());
        }
        write_born(home, session, &born);
    }
    match event {
        "UserPromptSubmit" => {
            born.prompts += 1;
            // Claude Code tells a session that its background work ended
            // with a prompt of its own, `<task-notification>` and the task
            // (2.1.295): no one talking to it (task 1688).
            let notice = input["prompt"].as_str().is_some_and(|prompt| prompt.trim_start().starts_with("<task-notification>"));
            if !notice && finished(home, cwd, &tasks).is_ok() && !born.talked {
                born.talked = true;
                log(home, session, &tasks, "a prompt reached it after its tasks were finished: it stays, the user's now");
            }
            show_state(home, session, &tasks, &mut born, WORKING, None);
            write_born(home, session, &born);
        }
        _ if TURN_ENDS.contains(&event) => {
            let busy = |key: &str| input[key].as_array().is_some_and(|work| !work.is_empty());
            let background = busy("background_tasks") || busy("session_crons");
            let stays = match finished(home, cwd, &tasks) {
                Err(why) => Some(why),
                Ok(()) if born.talked => Some("a prompt reached it after its tasks were finished".to_string()),
                // Background work in flight no longer keeps it (task 1829):
                // its tasks are finished, so what still runs is a waiter no
                // one needs, which the closer stops first.
                Ok(()) => None,
            };
            if let Some(why) = stays {
                log(home, session, &tasks, &format!("stays: {why}"));
                if handing_over {
                    return std::process::ExitCode::SUCCESS;
                }
                // A turn an API error ended (task 1798) waits on the user:
                // nothing else starts it again, and no idle_prompt came in
                // 101 s for 1564's session. Claude Code ignores this event's
                // output, so the mark is what tells anyone.
                if event == "StopFailure" {
                    let error = input["error"].as_str().filter(|error| !error.is_empty()).unwrap_or("unknown");
                    let said = input["last_assistant_message"].as_str().map(str::trim).filter(|said| !said.is_empty()).unwrap_or("API Error");
                    let what = crate::agent::clip(&format!("its turn ended on an error -- {said} ({error})"), 200);
                    born.waiting = wait_on_user(home, cwd, &tasks, session, "api_error", &what);
                    if born.waiting.is_some() {
                        show_state(home, session, &tasks, &mut born, WAITING, Some(&what));
                    }
                    born.background = background;
                    write_born(home, session, &born);
                    return std::process::ExitCode::SUCCESS;
                }
                let shown = show_state(home, session, &tasks, &mut born, if background { WORKING } else { IDLE }, None);
                if shown || born.background != background {
                    born.background = background;
                    write_born(home, session, &born);
                }
                return std::process::ExitCode::SUCCESS;
            }
            if closing(home, session, &tasks, born.prompts) {
                println!("{}", serde_json::json!({"systemMessage": "ekko: this session's tasks are finished, so it closes now"}));
            }
        }
        // A tool ran, or a dialog was answered: the turn goes on -- unless
        // the tool was the write that finished the last of the session's
        // tasks, each with its closing note (task 1866).
        _ => {
            if event == "PostToolUse" && !born.talked && writes_the_board(input["tool_name"].as_str().unwrap_or_default(), &input["tool_input"]) {
                let process = crate::holder::claude_among(std::os::unix::process::parent_id()).and_then(Process::of);
                let noted = process.as_ref().is_some_and(|process| finished(home, cwd, &tasks).is_ok() && closed(home, cwd, &tasks, process).is_ok());
                if noted && closing(home, session, &tasks, born.prompts) {
                    log(home, session, &tasks, "its last task is finished, each with its closing note: stopped after the write that finished it");
                    println!(
                        "{}",
                        serde_json::json!({"continue": false, "stopReason": "ekko: this session's tasks are finished, each with its closing note, so it closes now"})
                    );
                } else if let Some(handoff) = process.as_ref().and_then(|process| handed_over(home, cwd, &tasks, process, born.handoff.as_deref())) {
                    // A handoff of its own on a task it holds (task 2028): it
                    // waits for the answer from here, marked at once, and its
                    // turn ends with no text. It stays open for the note that
                    // answers it.
                    let what = crate::agent::clip(&format!("its handoff, note {}, on task {}: {}", handoff.note, handoff.task, handoff.headline), 200);
                    born.handoff = Some(handoff.uid);
                    born.waiting = wait_on_user(home, cwd, &tasks, session, HANDOFF, &what);
                    if born.waiting.is_some() {
                        show_state(home, session, &tasks, &mut born, WAITING, Some(&what));
                    }
                    write_born(home, session, &born);
                    log(home, session, &tasks, &format!("stopped after its handoff, note {}: it waits for the answer", handoff.note));
                    println!(
                        "{}",
                        serde_json::json!({"continue": false, "stopReason": format!("ekko: this session wrote its handoff on task {} and waits; the answer comes as a note on the task", handoff.task)})
                    );
                    return std::process::ExitCode::SUCCESS;
                }
            }
            if show_state(home, session, &tasks, &mut born, WORKING, None) {
                write_born(home, session, &born);
            }
        }
    }
    std::process::ExitCode::SUCCESS
}

/// Sets the state of the session's pane for the views (task 1675) --
/// `state`, since now, and what it waits on, if it does -- unless the hook
/// set it so last; in no multiplexer's pane, nothing. Whether it set it.
fn show_state(home: &Path, session: &str, tasks: &str, born: &mut Born, state: &str, waits: Option<&str>) -> bool {
    if born.state.as_deref() == Some(state) && waits.is_none() {
        return false;
    }
    let (Some(mux), Some(pane)) = (Mux::of_this_pane(), set("TMUX_PANE").map(|pane| pane.to_string_lossy().into_owned())) else {
        return false;
    };
    match mux.batch(&state_commands(&pane, state, waits)) {
        Ok(()) => {
            born.state = Some(state.to_string());
            true
        }
        Err(why) => {
            log(home, session, tasks, &format!("its pane's state, {state}, not set: {why}"));
            false
        }
    }
}

/// What a permission request is for, as a person reads it: the tool, and
/// the command, the file, the address or the pattern it was given, else
/// its input whole, on one line, clipped.
fn asked_for(tool: &serde_json::Value, input: &serde_json::Value) -> String {
    let tool = tool.as_str().unwrap_or("a tool");
    let given = ["command", "file_path", "notebook_path", "url", "path", "pattern"].iter().find_map(|key| input[key].as_str().map(str::to_string));
    let given = given.unwrap_or_else(|| if input.is_null() { String::new() } else { input.to_string() });
    let asked = if given.is_empty() { tool.to_string() } else { format!("{tool}: {given}") };
    crate::agent::clip(&asked, 100)
}

/// Marks that the session waits on the user, for `what`: on the board, on
/// each of its tasks its process holds in progress, and at the end of its
/// window's name. A turn over with nothing typed since is no wait on the
/// user while the session waits on the board, through ekko's wait: then
/// nothing is marked. What it marked, to clear.
fn wait_on_user(home: &Path, cwd: &Path, tasks: &str, session: &str, kind: &str, what: &str) -> Option<Waiting> {
    let since = chrono::Local::now().timestamp_millis();
    let process = crate::holder::claude_among(std::os::unix::process::parent_id()).and_then(Process::of);
    let ekko = process.as_ref().map(|_| board(home, cwd));
    if kind == "idle_prompt" {
        if let Some(note) = process.as_ref().zip(ekko.as_ref().and_then(|ekko| ekko.as_ref().ok())).and_then(|(process, ekko)| waits_on_board(ekko, process)) {
            log(home, session, tasks, &format!("does not wait on you ({kind}): it waits on the board, through note {note}"));
            return None;
        }
    }
    // Its window, and whether it is on the wall, whose name is every
    // session's on it (task 1677): there its border says this one waits.
    let pane = Mux::of_this_pane().zip(set("TMUX_PANE").map(|pane| pane.to_string_lossy().into_owned()));
    let found = pane.ok_or_else(|| "in no multiplexer's pane".to_string()).and_then(|(mux, pane)| {
        let wall = mux.on_wall(&pane);
        let name = mux.window_name(&pane)?;
        Ok((mux, pane, name, wall))
    });
    // Where the user finds it, which the board says to whoever waits on its
    // task (task 1715).
    let window = found.as_ref().ok().map(|(_, _, name, _)| name.strip_suffix(WAITING_ON_YOU).unwrap_or(name).to_string());
    let board = match process.zip(ekko) {
        None => "not marked on the board: no Claude Code process is among its ancestors".to_string(),
        Some((process, ekko)) => {
            // A session that waits before it took its tasks up (task 1729)
            // takes them up now, so the mark has a claim to go on and the
            // orchestrator's wait on them ends.
            let claimed = match ekko.as_ref().map(|_| claim_unheld(home, cwd, tasks, &process)) {
                Ok(Ok(ids)) if !ids.is_empty() => format!("claimed task {}, which it had not taken up; ", listed(&ids)),
                Ok(Err(why)) => format!("its tasks not claimed: {why}; "),
                _ => String::new(),
            };
            let on = crate::holder::OnUser { since, what: what.to_string(), window, unknown: BTreeMap::new() };
            let marked = match ekko.and_then(|ekko| mark_holds(&ekko, tasks, &process, Some(on))) {
                Ok(ids) if ids.is_empty() => "no task of its own in progress to mark on the board".to_string(),
                Ok(ids) => format!("marked on task {}", listed(&ids)),
                Err(why) => format!("not marked on the board: {why}"),
            };
            format!("{claimed}{marked}")
        }
    };
    let renamed = found.and_then(|(mux, pane, name, wall)| {
        if wall {
            return Err("it is on the wall, whose name is not its own".to_string());
        }
        if name.ends_with(WAITING_ON_YOU) {
            return Ok(name);
        }
        let renamed = format!("{name}{WAITING_ON_YOU}");
        mux.rename(&pane, &renamed).map(|()| renamed)
    });
    let window = match &renamed {
        Ok(name) => format!("its window named \"{name}\""),
        Err(why) => format!("its window not renamed: {why}"),
    };
    log(home, session, tasks, &format!("waits on you ({kind}): {what}; {board}; {window}"));
    Some(Waiting { since, what: what.to_string(), window: renamed.ok(), kind: Some(kind.to_string()), unknown: BTreeMap::new() })
}

/// Clears what `wait_on_user` marked, now that `event` says the session
/// moved on: on the board, for the tasks `process`, the session's Claude
/// Code, holds. A window the user renamed meanwhile keeps their name.
fn answered(home: &Path, cwd: &Path, tasks: &str, session: &str, event: &str, waiting: &Waiting, process: Option<&Process>) {
    let board = match process {
        None => "left on the board: no Claude Code process is among its ancestors".to_string(),
        Some(process) => match board(home, cwd).and_then(|ekko| mark_holds(&ekko, tasks, process, None)) {
            Ok(ids) if ids.is_empty() => "nothing to clear on the board".to_string(),
            Ok(ids) => format!("cleared on task {}", listed(&ids)),
            Err(why) => format!("left on the board: {why}"),
        },
    };
    let window = match (&waiting.window, Mux::of_this_pane(), set("TMUX_PANE")) {
        (None, _, _) => "its window was not renamed".to_string(),
        (Some(renamed), Some(mux), Some(pane)) => {
            let pane = pane.to_string_lossy();
            let named = renamed.strip_suffix(WAITING_ON_YOU).unwrap_or(renamed);
            match mux.window_name(&pane) {
                Ok(now) if now == *renamed => match mux.rename(&pane, named) {
                    Ok(()) => format!("its window named \"{named}\" again"),
                    Err(why) => format!("its window not renamed back: {why}"),
                },
                Ok(now) => format!("its window kept the name it was given meanwhile, \"{now}\""),
                Err(why) => format!("its window not renamed back: {why}"),
            }
        }
        (Some(_), _, _) => "its window not renamed back: in no multiplexer's pane".to_string(),
    };
    log(home, session, tasks, &format!("waits on you no more ({event}): {board}; {window}"));
}

/// The board this session works on: the project EKKO_PROJECT names, as
/// `start` gave it.
fn board(home: &Path, cwd: &Path) -> Result<Ekko, String> {
    let (ekko_dir, project) = (std::env::var("EKKO_DIR").ok(), std::env::var("EKKO_PROJECT").ok());
    let location = crate::directory::locate(home, cwd, None, ekko_dir.as_deref(), project.as_deref()).map_err(|error| EkkoError::from(error).to_string())?;
    Ekko::at(&location).map_err(|error| error.to_string())
}

/// The note through which `process` waits on an item, with ekko's wait,
/// while it does.
fn waits_on_board(ekko: &Ekko, process: &Process) -> Option<u32> {
    let all = ekko.storage.get_shared().ok()?;
    let mut waits: Vec<u32> = all
        .values()
        .filter(|note| note.trashed.is_none() && note.stashed.is_none())
        .filter(|note| note.wait.as_ref().is_some_and(|wait| wait.over.is_none() && wait.by.process().as_ref() == Some(process)))
        .map(|note| note.id)
        .collect();
    waits.sort_unstable();
    waits.first().copied()
}

/// Sets `on` -- what the session waits on the user for, or `None` to clear
/// it -- on each of the tasks `named` that `process` holds in progress, in
/// one write. A task marked already keeps the mark it has. The ids it
/// changed.
fn mark_holds(ekko: &Ekko, named: &str, process: &Process, on: Option<crate::holder::OnUser>) -> Result<Vec<u32>, String> {
    let _lock = ekko.storage.acquire_lock().map_err(|error| EkkoError::from(error).to_string())?;
    let before = ekko.storage.get_shared().map_err(|error| EkkoError::from(error).to_string())?;
    let mut data = ItemMap::clone(&before);
    let index = crate::ekko::uid_index(&before);
    let mut changed = Vec::new();
    for name in named.split(',').map(str::trim).filter(|name| !name.is_empty()) {
        let Some(task) = index.get(name).copied().or_else(|| name.parse::<u32>().ok()).and_then(|id| data.get_mut(&id)) else { continue };
        if State::of(task) != Some(State::Progress) {
            continue;
        }
        let Some(holder) = task.held_by.as_mut().filter(|holder| holder.process().as_ref() == Some(process)) else { continue };
        if holder.waits_on_user.is_some() == on.is_some() {
            continue;
        }
        holder.waits_on_user = on.clone();
        changed.push(task.id);
    }
    if !changed.is_empty() {
        ekko.save_against(&before, &mut data).map_err(|error| error.to_string())?;
    }
    changed.sort_unstable();
    Ok(changed)
}

/// Whether the tasks `named` -- EKKO_AGENT_TASK's uids, or display ids where
/// a task has none -- are all finished, done or cancelled, on the board this
/// session works on; why not, if not.
fn finished(home: &Path, cwd: &Path, named: &str) -> Result<(), String> {
    let all = board(home, cwd)?.storage.get_shared().map_err(|error| EkkoError::from(error).to_string())?;
    let index = crate::ekko::uid_index(&all);
    for name in named.split(',').map(str::trim).filter(|name| !name.is_empty()) {
        let id = index.get(name).copied().or_else(|| name.parse::<u32>().ok());
        let Some(task) = id.and_then(|id| all.get(&id)) else {
            return Err(format!("task {name} is not on its board"));
        };
        match State::of(task) {
            Some(State::Done | State::Cancelled) => {}
            Some(state) => return Err(format!("task {} is {}", task.id, state.word())),
            None => return Err(format!("{} is no task", task.id)),
        }
    }
    Ok(())
}

/// Whether each of the tasks `named` has a closing note from `process`,
/// the session's Claude Code (task 1866): a note of its own attached to the
/// task -- not a handoff, a decision, gotcha or procedure, a question or a
/// wait -- that this process wrote.
/// Why not, if not: a task set done before its note is written would lose
/// the note if the session stopped there.
fn closed(home: &Path, cwd: &Path, named: &str, process: &Process) -> Result<(), String> {
    let all = board(home, cwd)?.storage.get_shared().map_err(|error| EkkoError::from(error).to_string())?;
    let index = crate::ekko::uid_index(&all);
    for name in named.split(',').map(str::trim).filter(|name| !name.is_empty()) {
        let Some(task) = index.get(name).copied().or_else(|| name.parse::<u32>().ok()).and_then(|id| all.get(&id)) else {
            return Err(format!("task {name} is not on its board"));
        };
        let keys = [task.uid.clone(), Some(task.id.to_string())];
        let noted = all.values().any(|note| {
            !note.is_task
                && !note.handoff
                && note.knowledge.is_none()
                && note.question.is_none()
                && note.wait.is_none()
                && note.trashed.is_none()
                && note.stashed.is_none()
                && note.attached_to.as_ref().is_some_and(|on| keys.iter().flatten().any(|key| key == on))
                && note.created_by.as_ref().and_then(crate::holder::Holder::process).as_ref() == Some(process)
        });
        if !noted {
            return Err(format!("task {} has no closing note from this session", task.id));
        }
    }
    Ok(())
}

/// Starts the closer for the end of the session's work, and says so in
/// born.log; whether it started.
fn closing(home: &Path, session: &str, tasks: &str, prompts: u64) -> bool {
    let process = crate::holder::claude_among(std::os::unix::process::parent_id()).and_then(Process::of);
    match process.ok_or_else(|| "no Claude Code process is among its ancestors".to_string()).and_then(|process| spawn_closer(session, prompts, &process)) {
        Ok(()) => {
            log(home, session, tasks, "its tasks are finished: closing");
            true
        }
        Err(why) => {
            log(home, session, tasks, &format!("not closed: {why}"));
            false
        }
    }
}

/// A handoff of the session's own on a task it holds (task 2028): the
/// task, the note, its uid and its first line.
struct Handoff {
    task: u32,
    note: u32,
    uid: String,
    headline: String,
}

/// The newest handoff `process` wrote on one of the tasks `named` that it
/// holds in progress, unless it is `last`, which the hook stopped it at
/// already.
fn handed_over(home: &Path, cwd: &Path, named: &str, process: &Process, last: Option<&str>) -> Option<Handoff> {
    let all = board(home, cwd).ok()?.storage.get_shared().ok()?;
    let index = crate::ekko::uid_index(&all);
    let mut found: Option<Handoff> = None;
    for name in named.split(',').map(str::trim).filter(|name| !name.is_empty()) {
        let Some(task) = index.get(name).copied().or_else(|| name.parse::<u32>().ok()).and_then(|id| all.get(&id)) else { continue };
        let held = State::of(task) == Some(State::Progress) && task.held_by.as_ref().and_then(crate::holder::Holder::process).as_ref() == Some(process);
        if !held {
            continue;
        }
        let keys = [task.uid.clone(), Some(task.id.to_string())];
        let newest = all
            .values()
            .filter(|note| !note.is_task && note.handoff && note.trashed.is_none() && note.stashed.is_none())
            .filter(|note| note.attached_to.as_ref().is_some_and(|on| keys.iter().flatten().any(|key| key == on)))
            .filter(|note| note.created_by.as_ref().and_then(crate::holder::Holder::process).as_ref() == Some(process))
            .max_by_key(|note| note.id);
        let Some(note) = newest else { continue };
        let Some(uid) = note.uid.clone().filter(|uid| Some(uid.as_str()) != last) else { continue };
        if found.as_ref().is_none_or(|found| found.note < note.id) {
            found = Some(Handoff { task: task.id, note: note.id, uid, headline: crate::agent::headline(&note.description, false, 120) });
        }
    }
    found
}

/// Whom a write for `process` is by: that Claude Code process, named as
/// `like`, its claim on a task, names it, else as this one names its own.
fn actor_of(home: &Path, process: &Process, like: Option<&crate::holder::Holder>) -> crate::holder::Actor {
    crate::holder::Actor {
        process: Some(process.clone()),
        profile: like.and_then(|holder| holder.profile.clone()).or_else(crate::holder::profile),
        tty: like.and_then(|holder| holder.tty.clone()).or_else(|| crate::holder::tty(process.pid)),
        registry: Some(crate::holder::Registry::at(crate::agent::processes_dir(home))),
    }
}

/// Claims for `process` the tasks `named` that are open and that no one
/// still running holds, when it holds none of them in progress (task 1729):
/// a session that waits on the user before its model took its tasks up. The
/// ids it claimed.
fn claim_unheld(home: &Path, cwd: &Path, named: &str, process: &Process) -> Result<Vec<u32>, String> {
    let ekko = board(home, cwd)?.acting_as(actor_of(home, process, None));
    let all = ekko.storage.get_shared().map_err(|error| EkkoError::from(error).to_string())?;
    let index = crate::ekko::uid_index(&all);
    let tasks: Vec<&Item> = named
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .filter_map(|name| index.get(name).copied().or_else(|| name.parse::<u32>().ok()).and_then(|id| all.get(&id)))
        .collect();
    let progress = |task: &Item| State::of(task) == Some(State::Progress);
    if tasks.iter().any(|task| progress(task) && task.held_by.as_ref().and_then(crate::holder::Holder::process).as_ref() == Some(process)) {
        return Ok(Vec::new());
    }
    let free: Vec<u32> = tasks
        .iter()
        .filter(|task| State::of(task).is_some_and(State::is_open))
        .filter(|task| !(progress(task) && task.held_by.as_ref().is_some_and(crate::holder::Holder::alive)))
        .map(|task| task.id)
        .collect();
    if free.is_empty() {
        return Ok(Vec::new());
    }
    let mut draft = crate::ops::Draft::open(&ekko).map_err(|error| error.to_string())?;
    let refs: Vec<crate::ops::Ref> = free.iter().map(|id| crate::ops::Ref::Id(*id)).collect();
    draft.set_state(&refs, "progress").map_err(|error| error.to_string())?;
    draft.commit(false).map_err(|error| error.to_string())?;
    Ok(free)
}

/// What marks a born session's wait for the answer to its handoff.
const HANDOFF: &str = "handoff";

/// The argument that runs ekko as a born session's watcher (tasks 1827,
/// 1729 and 1832), which its hook starts: never typed.
pub const WATCH_ARG: &str = "__ekko_watch";

/// How often the watcher looks at the session's process and its wait.
const WATCH_EVERY: Duration = Duration::from_millis(500);

/// How long a born session waits on the user before the watcher tells them
/// directly (task 1832): ten minutes, or the seconds TELL_AFTER_VAR gives.
const TELL_AFTER: Duration = Duration::from_secs(600);

/// The variable that gives the watcher, in seconds, how long a wait goes
/// before the user is told.
pub const TELL_AFTER_VAR: &str = "EKKO_AGENTS_TELL_AFTER";

/// The lock a watcher of `process` holds while it runs, beside born.log.
fn watch_file(home: &Path, process: &Process) -> PathBuf {
    born_dir(home).join(format!("{}-{}.watch", process.pid, process.start))
}

/// The lock at `path`, taken at once, or `None` where another holds it or
/// it cannot be made.
fn try_lock(path: &Path) -> Option<std::fs::File> {
    use std::os::fd::AsRawFd as _;
    let file = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(path).ok()?;
    // SAFETY: flock(2) on a descriptor this function owns.
    let locked = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
    locked.then_some(file)
}

/// Starts the watcher of `process`, unless one watches it already.
fn spawn_watch(home: &Path, session: &str, process: &Process) -> Result<(), String> {
    std::fs::create_dir_all(born_dir(home)).map_err(|error| format!("its watcher's lock has nowhere to go: {error}"))?;
    let Some(free) = try_lock(&watch_file(home, process)) else { return Ok(()) };
    drop(free);
    detached(&[WATCH_ARG, session, &process.pid.to_string(), &process.start.to_string()]).map_err(|why| format!("its watcher did not start: {why}"))
}

/// The watcher (WATCH_ARG `<session> <pid> <start>`): runs as long as the
/// session's Claude Code, which nothing else outlives to speak for it.
///
/// Once its process is gone -- the window closed, a crash, a kill -- with a
/// task of its own unfinished, the watcher says so on the board (tasks 1827
/// and 1729): see `ended`. While the session waits on the user, after
/// TELL_AFTER it tells them with a desktop notification, once a wait (task
/// 1832): the orchestrator is told at once, but may be busy, or not this
/// board's, and on 2026-10-12 a born session waited all day.
pub fn watch(home: &Path, cwd: &Path, args: &[String]) -> std::process::ExitCode {
    use std::process::ExitCode;
    let tasks = std::env::var(TASKS_VAR).unwrap_or_default();
    let [session, pid, start] = args else { return ExitCode::FAILURE };
    let (Ok(pid), Ok(start), Some(boot)) = (pid.parse::<u32>(), start.parse::<u64>(), crate::holder::boot()) else {
        return ExitCode::FAILURE;
    };
    let process = Process { pid, start, boot };
    let lock = watch_file(home, &process);
    let Some(_held) = try_lock(&lock) else { return ExitCode::SUCCESS };
    let after = std::env::var(TELL_AFTER_VAR).ok().and_then(|seconds| seconds.trim().parse::<u64>().ok()).map_or(TELL_AFTER, Duration::from_secs);
    let after = i64::try_from(after.as_millis()).unwrap_or(i64::MAX);
    let mut told: Option<i64> = None;
    loop {
        if !process.alive() {
            // Where ekko's state is gone with its home, as a test's is once
            // it has run, nothing is left to tell, nor made again.
            if born_dir(home).is_dir() {
                ended(home, cwd, session, &tasks, &process);
                let _ = std::fs::remove_file(&lock);
            }
            return ExitCode::SUCCESS;
        }
        if let Some(waiting) = read_born(home, session).waiting {
            let now = chrono::Local::now().timestamp_millis();
            if told != Some(waiting.since) && now - waiting.since >= after {
                told = Some(waiting.since);
                tell_user(home, cwd, session, &tasks, &waiting, now);
            }
        }
        std::thread::sleep(WATCH_EVERY);
    }
}

/// Tells the user, with a desktop notification, that the session has
/// waited on them since `waiting.since`: for what, on which tasks, and in
/// which window of which session, where they find it.
fn tell_user(home: &Path, cwd: &Path, session: &str, tasks: &str, waiting: &Waiting, now: i64) {
    let waited = (now - waiting.since).max(0) / 1000;
    let waited = if waited < 60 { format!("{waited} s") } else { format!("{} min", waited / 60) };
    let all = board(home, cwd).ok().and_then(|ekko| ekko.storage.get_shared().ok());
    let ids: Vec<u32> = all.as_ref().map_or_else(Vec::new, |all| {
        let index = crate::ekko::uid_index(all);
        tasks.split(',').map(str::trim).filter_map(|name| index.get(name).copied().or_else(|| name.parse::<u32>().ok())).collect()
    });
    let named = match ids.as_slice() {
        [] => "a session ekko agents started".to_string(),
        [id] => format!("task {id}'s session"),
        _ => format!("the session of tasks {}", listed(&ids)),
    };
    let summary = format!("ekko: {named} has waited on you for {waited}");
    let window = waiting.window.as_deref().map(|name| name.strip_suffix(WAITING_ON_YOU).unwrap_or(name));
    let project = std::env::var("EKKO_PROJECT").ok().filter(|name| !name.is_empty()).map(|name| format!(", project {name}")).unwrap_or_default();
    let place = match window {
        Some(window) => format!("Window \"{window}\" of session {SESSION}{project}."),
        None => format!("A window of session {SESSION}{project}."),
    };
    let body = crate::agent::clip(&format!("{}. {place}", waiting.what.trim_end_matches('.')), 300);
    let shown = crate::menu::notifier(&summary, &body).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).output();
    match shown {
        Ok(output) if output.status.success() => log(home, session, tasks, &format!("told the user it has waited on them for {waited}: {body}")),
        Ok(output) => log(home, session, tasks, &format!("not told the user it waits: busctl said {}", String::from_utf8_lossy(&output.stderr).trim())),
        Err(error) => log(home, session, tasks, &format!("not told the user it waits: busctl did not run: {error}")),
    }
}

/// What the watcher writes once the session's process `process` is gone
/// (tasks 1827 and 1729), in one write, for each of the tasks `named` that
/// is unfinished and that no one still running holds: a task it held in
/// progress goes to paused, which lets it go, every wait on such a task
/// ends, saying its session ended, and a note on it says where the work
/// stands -- the worktree, the branch, its uncommitted files and last
/// commit -- and how to go on. A task it never took up, that nothing waits
/// on, is left as it was. The claim was a lease, as a job queue's visibility
/// timeout is: its holder gone, the work goes back with a record, and
/// whoever waits is told.
fn ended(home: &Path, cwd: &Path, session: &str, named: &str, process: &Process) {
    // Only a board whose file is there: opening one makes its folders.
    let (ekko_dir, project) = (std::env::var("EKKO_DIR").ok(), std::env::var("EKKO_PROJECT").ok());
    let located = crate::directory::locate(home, cwd, None, ekko_dir.as_deref(), project.as_deref());
    if !located.is_ok_and(|location| crate::storage::storage_file(&location.dir).is_file()) {
        return;
    }
    let Ok(ekko) = board(home, cwd) else { return };
    let Ok(all) = ekko.storage.get_shared() else { return };
    let index = crate::ekko::uid_index(&all);
    let tasks: Vec<&Item> = named
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .filter_map(|name| index.get(name).copied().or_else(|| name.parse::<u32>().ok()).and_then(|id| all.get(&id)))
        .collect();
    let waited = |task: &Item| {
        all.values().any(|note| {
            note.trashed.is_none() && note.stashed.is_none() && note.wait.as_ref().is_some_and(|wait| wait.over.is_none() && Some(&wait.on) == task.uid.as_ref())
        })
    };
    let held_here = |task: &Item| State::of(task) == Some(State::Progress) && task.held_by.as_ref().and_then(crate::holder::Holder::process).as_ref() == Some(process);
    let held_by_another = |task: &Item| State::of(task) == Some(State::Progress) && task.held_by.as_ref().is_some_and(crate::holder::Holder::alive);
    let left: Vec<(u32, bool)> = tasks
        .iter()
        .filter(|task| State::of(task).is_some_and(State::is_open))
        .filter(|task| held_here(task) || (!held_by_another(task) && waited(task)))
        .map(|task| (task.id, held_here(task)))
        .collect();
    if left.is_empty() {
        return;
    }
    let like = tasks.iter().find(|task| held_here(task)).and_then(|task| task.held_by.as_ref());
    let ekko = ekko.acting_as(actor_of(home, process, like));
    let stands = where_it_stands(cwd);
    let at = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let written = (|| -> Result<Vec<String>, EkkoError> {
        let mut draft = crate::ops::Draft::open(&ekko)?;
        let mut said = Vec::new();
        for (id, held) in &left {
            if *held {
                draft.set_state(&[crate::ops::Ref::Id(*id)], "paused")?;
            }
            let waits = draft.end_waits_as_ended(*id);
            let how = if *held { "with the task in progress: ekko paused it, which lets it go" } else { "before it took the task up" };
            let told = match waits.as_slice() {
                [] => String::new(),
                [one] => format!(" The wait of note {one} on it is over, and its session told."),
                _ => format!(" The waits of notes {} on it are over, and their sessions told.", listed(&waits)),
            };
            let text = format!(
                "The session ekko agents started for task {id} ended at {at}, its Claude Code process (pid {}) gone {how}.{told}\n{stands}\nTo go on: ekko agents start {id}, in the project's folder, takes this worktree up again as it is.",
                process.pid
            );
            let spec = crate::ops::Create {
                kind: Some(crate::ops::Kind::Note),
                text,
                boards: Vec::new(),
                priority: None,
                due: None,
                with: None,
                phase: None,
                blocked_by: Vec::new(),
                attached_to: Some(crate::ops::Ref::Id(*id)),
                supersedes: None,
                starred: false,
            };
            let note = draft.create(&spec)?;
            said.push(format!("task {id} {}, note {note}{}", if *held { "paused" } else { "left as it was" }, if waits.is_empty() { String::new() } else { format!(", waits {} ended", listed(&waits)) }));
        }
        draft.commit(false)?;
        Ok(said)
    })();
    match written {
        Ok(said) => log(home, session, named, &format!("its Claude Code, pid {}, ended with its work unfinished: {}", process.pid, said.join("; "))),
        Err(error) => log(home, session, named, &format!("its Claude Code, pid {}, ended with its work unfinished, and the board was not written: {error}", process.pid)),
    }
}

/// Where a born session's work stands in its worktree, for the note the
/// watcher writes as it ends: the worktree and branch, the files with
/// uncommitted changes, and the last commit.
fn where_it_stands(cwd: &Path) -> String {
    let worktree = set(WORKTREE_VAR).map_or_else(|| git(cwd, &["rev-parse", "--show-toplevel"]).map(PathBuf::from).unwrap_or_else(|_| cwd.to_path_buf()), PathBuf::from);
    let branch = set(BRANCH_VAR).map(|branch| branch.to_string_lossy().into_owned()).or_else(|| git(&worktree, &["rev-parse", "--abbrev-ref", "HEAD"]).ok());
    let on = branch.map(|branch| format!(", on branch {branch}")).unwrap_or_default();
    let changed = match git(&worktree, &["status", "--porcelain", "--untracked-files=all"]) {
        Ok(status) if status.is_empty() => "Nothing uncommitted.".to_string(),
        Ok(status) => {
            let files: Vec<&str> = status.lines().map(str::trim).collect();
            let shown = files.iter().take(20).copied().collect::<Vec<_>>().join(", ");
            let more = files.len().saturating_sub(20);
            let more = if more > 0 { format!(", and {more} more") } else { String::new() };
            format!("Uncommitted, {} file{}: {shown}{more}.", files.len(), if files.len() == 1 { "" } else { "s" })
        }
        Err(why) => format!("Its git status could not be read: {why}."),
    };
    let last = git(&worktree, &["log", "-1", "--format=%h %s"]).map_or_else(|_| String::new(), |last| format!(" Last commit: {last}."));
    format!("Its worktree: {}{on}. {changed}{last}", worktree.display())
}

/// Starts the closer: ekko again, in a session of its own with none of the
/// hook's pipes, so that the hook returns at once and the closer outlives
/// it.
fn spawn_closer(session: &str, prompts: u64, process: &Process) -> Result<(), String> {
    detached(&[CLOSER_ARG, session, &prompts.to_string(), &process.pid.to_string(), &process.start.to_string()]).map_err(|why| format!("its closer did not start: {why}"))
}

/// Starts the watcher of a permission prompt, as `spawn_closer` starts the
/// closer.
fn spawn_watcher(session: &str, process: &Process) -> Result<(), String> {
    detached(&[ANSWER_ARG, session, &process.pid.to_string(), &process.start.to_string()]).map_err(|why| format!("its watcher did not start: {why}"))
}

/// Runs ekko with `args` in a session of its own, with none of this
/// process's pipes, so that it outlives the hook that starts it.
fn detached(args: &[&str]) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    let exe = std::env::current_exe().map_err(|error| format!("ekko's own binary is not found: {error}"))?;
    let mut command = Command::new(exe);
    command.args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    // SAFETY: the closure runs between fork and exec, and calls only setsid,
    // which is async-signal-safe.
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    command.spawn().map(drop).map_err(|error| error.to_string())
}

/// The watcher (ANSWER_ARG `<session> <pid> <start>`, task 1679): Claude
/// Code fires no hook as the user answers a permission prompt, and the
/// tool they let run may take minutes before PostToolUse says the session
/// moved on. Its own record of the session does say it at once:
/// `sessions/<pid>.json` in its config folder, which `claude agents --json`
/// reports, whose `status` read `waiting` while the prompt showed and
/// `busy` 17 to 26 ms after Enter answered it, in three runs of Claude
/// Code 2.1.295. Every 100 ms the watcher reads it; once it has said
/// `waiting` and then `busy` or `idle`, it clears the mark as the hook
/// would, and the pane's state. It ends then, or once the session waits no
/// more, the hook having cleared it, or its Claude Code has ended. A record
/// that does not say `waiting` first, as one of another version might,
/// leaves the mark to the hook.
pub fn answer(home: &Path, cwd: &Path, args: &[String]) -> std::process::ExitCode {
    use std::process::ExitCode;
    let tasks = std::env::var(TASKS_VAR).unwrap_or_default();
    let [session, pid, start] = args else { return ExitCode::FAILURE };
    let (Ok(pid), Ok(start), Some(boot)) = (pid.parse::<u32>(), start.parse::<u64>(), crate::holder::boot()) else {
        return ExitCode::FAILURE;
    };
    let process = Process { pid, start, boot };
    let mut seen_waiting = false;
    loop {
        if !process.alive() || read_born(home, session).waiting.is_none() {
            return ExitCode::SUCCESS;
        }
        let status = claude_status(home, session, pid);
        match status.as_deref() {
            Some("waiting") => seen_waiting = true,
            Some(now @ ("busy" | "idle")) if seen_waiting => {
                let _alone = lock_born(home, session);
                let mut born = read_born(home, session);
                if let Some(waiting) = born.waiting.take() {
                    born.asked = None;
                    answered(home, cwd, &tasks, session, &format!("answered: Claude Code's record says {now}"), &waiting, Some(&process));
                    show_state(home, session, &tasks, &mut born, if now == "idle" { IDLE } else { WORKING }, None);
                    write_born(home, session, &born);
                }
                return ExitCode::SUCCESS;
            }
            _ => {}
        }
        std::thread::sleep(ANSWER_EVERY);
    }
}

/// The `status` Claude Code's record of the session gives, if the record
/// at `sessions/<pid>.json`, under `CLAUDE_CONFIG_DIR` or else `~/.claude`,
/// is this session's.
fn claude_status(home: &Path, session: &str, pid: u32) -> Option<String> {
    let dir = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|dir| !dir.is_empty()).map_or_else(|| home.join(".claude"), PathBuf::from);
    let record: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("sessions").join(format!("{pid}.json"))).ok()?).ok()?;
    if record["sessionId"].as_str() != Some(session) || record["pid"].as_u64() != Some(u64::from(pid)) {
        return None;
    }
    record["status"].as_str().map(str::to_string)
}

/// The closer (CLOSER_ARG `<session> <prompts> <pid> <start>`): a second
/// after the turn ended, unless a prompt reached the session meanwhile or
/// its tasks are no longer all finished, ends its Claude Code with SIGTERM,
/// which ends it at once -- 5 ms, measured with Claude Code 2.1.295, and
/// its SessionEnd hooks do not run -- where /exit typed into its pane would
/// land on whatever the user has half typed there. Before that it stops the
/// session's background work (`stop_background`), which would otherwise
/// outlive it, or keep it open (task 1829). Then it makes sure the pane is
/// gone, by the list of panes, and closes one a remain-on-exit keeps.
pub fn close(home: &Path, cwd: &Path, args: &[String]) -> std::process::ExitCode {
    use std::process::ExitCode;
    let tasks = std::env::var(TASKS_VAR).unwrap_or_default();
    let [session, prompts, pid, start] = args else { return ExitCode::FAILURE };
    let (Ok(prompts), Ok(pid), Ok(start), Some(boot)) = (prompts.parse::<u64>(), pid.parse::<u32>(), start.parse::<u64>(), crate::holder::boot()) else {
        return ExitCode::FAILURE;
    };
    std::thread::sleep(set(GRACE_VAR).and_then(|ms| ms.to_str()?.parse().ok()).map_or(GRACE, Duration::from_millis));
    if read_born(home, session).prompts != prompts {
        log(home, session, &tasks, "stays: a prompt reached it as it was closing");
        return ExitCode::SUCCESS;
    }
    if let Err(why) = finished(home, cwd, &tasks) {
        log(home, session, &tasks, &format!("stays: {why}"));
        return ExitCode::SUCCESS;
    }
    let process = Process { pid, start, boot };
    if process.alive() {
        stop_background(home, session, &tasks, &process);
        // SAFETY: a plain kill(2) of the process this pid names now, which
        // `alive` has just found to be the session's own.
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
        if !within(GONE_WITHIN, || !process.alive()) {
            log(home, session, &tasks, &format!("not closed: its Claude Code, pid {pid}, still runs {} s after SIGTERM", GONE_WITHIN.as_secs()));
            return ExitCode::FAILURE;
        }
    }
    let (Some(mux), Some(pane)) = (Mux::of_this_pane(), set("TMUX_PANE").map(|pane| pane.to_string_lossy().into_owned())) else {
        log(home, session, &tasks, "closed: its Claude Code ended, in no multiplexer's pane");
        return ExitCode::SUCCESS;
    };
    if mux.listed(&pane) == Ok(true) {
        let _ = mux.run(&["kill-pane", "-t", &pane]);
    }
    let gone = within(GONE_WITHIN, || mux.listed(&pane) != Ok(true));
    match (gone, mux.listed(&pane)) {
        (true, Ok(_)) => log(home, session, &tasks, &format!("closed: its Claude Code ended, and its pane {pane} is gone")),
        (true, Err(why)) => log(home, session, &tasks, &format!("closed: its Claude Code ended, and its pane {pane} is gone with its server ({why})")),
        (false, _) => {
            log(home, session, &tasks, &format!("not closed: its Claude Code ended, and its pane {pane} is still listed"));
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}

/// What the command line of a shell Claude Code runs a Bash or a Monitor
/// call in holds: it sources the shell snapshot of its config folder, then
/// evals the call's command -- `bash -c 'source
/// <config>/shell-snapshots/snapshot-bash-….sh … && eval '<command>' <
/// /dev/null && pwd -P >| /tmp/claude-…-cwd'`, as Claude Code 2.1.302
/// runs both, seen in /proc on 2026-10-18. An MCP server or a hook holds
/// no snapshot.
const TOOL_SHELL: &str = "/shell-snapshots/snapshot-";

/// How long the session's background work is given to end on SIGTERM
/// before SIGKILL.
const BACKGROUND_GONE_WITHIN: Duration = Duration::from_secs(2);

/// A process as /proc/<pid>/stat gives it: its parent, the session it is
/// in, and when it started.
struct Stat {
    pid: u32,
    parent: u32,
    session: u32,
    start: u64,
}

/// Every process /proc lists now, as `Stat`. Fields counted from the end
/// of the name, which may hold spaces and parentheses: the parent is field
/// 4, the session 6, the start 22.
fn processes() -> Vec<Stat> {
    let Ok(entries) = std::fs::read_dir("/proc") else { return Vec::new() };
    entries
        .filter_map(|entry| {
            let pid: u32 = entry.ok()?.file_name().to_str()?.parse().ok()?;
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
            let fields: Vec<&str> = stat.get(stat.rfind(')')? + 1..)?.split_whitespace().collect();
            Some(Stat { pid, parent: fields.get(1)?.parse().ok()?, session: fields.get(3)?.parse().ok()?, start: fields.get(19)?.parse().ok()? })
        })
        .collect()
}

/// The command a tool shell's command line evals, as the call gave it;
/// its whole command line where it evals none.
fn tool_command(cmdline: &str) -> String {
    let command = cmdline
        .split_once("eval '")
        .and_then(|(_, rest)| rest.rsplit_once("' < /dev/null").or_else(|| rest.rsplit_once("' && pwd")))
        .map(|(command, _)| command.replace("'\"'\"'", "'").replace("'\\''", "'"));
    command.unwrap_or_else(|| cmdline.to_string())
}

/// The background work of the session whose Claude Code is `claude`: the
/// shells its Bash and Monitor calls run in (`TOOL_SHELL`) that are its
/// children and lead a session of their own, as Claude Code starts each,
/// with every process in each such session -- what the shell started, and
/// what that started, which all inherit its session unless they leave it
/// -- and the command each shell runs. Nothing else: not its MCP servers,
/// in its own session, not a process outside the session's tree, however
/// its command line reads.
fn background_of(claude: u32) -> Vec<(String, Vec<Process>)> {
    let Some(boot) = crate::holder::boot() else { return Vec::new() };
    let all = processes();
    all.iter()
        .filter(|shell| shell.parent == claude && shell.session == shell.pid)
        .filter_map(|shell| {
            let cmdline = String::from_utf8_lossy(&std::fs::read(format!("/proc/{}/cmdline", shell.pid)).ok()?).replace('\0', " ");
            if !cmdline.contains(TOOL_SHELL) {
                return None;
            }
            let members =
                all.iter().filter(|member| member.session == shell.pid).map(|member| Process { pid: member.pid, start: member.start, boot: boot.clone() }).collect();
            Some((tool_command(cmdline.trim_end()), members))
        })
        .collect()
}

/// Stops the session's background work (`background_of`) once its tasks are
/// finished (task 1829): its Monitors and background Bash calls, which no
/// one needs then, and which kept a done session open when it waited for
/// them -- a `tail -F`, an `until ! pgrep -f …` that matched itself. SIGTERM,
/// then SIGKILL for what still runs after `BACKGROUND_GONE_WITHIN`; each
/// process signalled only while it is the one that was found. Says what it
/// stopped in born.log.
fn stop_background(home: &Path, session: &str, tasks: &str, claude: &Process) {
    let work = background_of(claude.pid);
    if work.is_empty() {
        return;
    }
    let signal = |process: &Process, signal: libc::c_int| {
        if process.alive() {
            // SAFETY: a plain kill(2) of the process this pid names now,
            // which `alive` has just found to be the one listed.
            unsafe { libc::kill(process.pid as libc::pid_t, signal) };
        }
    };
    let members: Vec<&Process> = work.iter().flat_map(|(_, members)| members).collect();
    for process in &members {
        signal(process, libc::SIGTERM);
    }
    if !within(BACKGROUND_GONE_WITHIN, || members.iter().all(|process| !process.alive())) {
        for process in &members {
            signal(process, libc::SIGKILL);
        }
    }
    let left: Vec<String> = members.iter().filter(|process| process.alive()).map(|process| process.pid.to_string()).collect();
    let commands: Vec<String> = work.iter().map(|(command, _)| format!("`{}`", crate::agent::clip(command, 200))).collect();
    let mut said = format!(
        "its background work stopped, its tasks being finished: {} shell{} ({} process{}): {}",
        work.len(),
        if work.len() == 1 { "" } else { "s" },
        members.len(),
        if members.len() == 1 { "" } else { "es" },
        commands.join("; ")
    );
    if !left.is_empty() {
        said.push_str(&format!("; still running after SIGKILL: pid {}", left.join(", ")));
    }
    log(home, session, tasks, &said);
}

/// Whether `done` holds within `limit`, asked every 50 ms.
fn within(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let until = std::time::Instant::now() + limit;
    loop {
        if done() {
            return true;
        }
        if std::time::Instant::now() >= until {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Where the hook keeps born sessions: `born/` in ekko's state directory, a
/// file for each session and `born.log`, what was decided and why.
fn born_dir(home: &Path) -> PathBuf {
    crate::agent::state_dir(home).join("born")
}

pub(crate) fn born_file(home: &Path, session: &str) -> PathBuf {
    let name: String = session.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
    born_dir(home).join(format!("{}.json", if name.is_empty() { "unknown" } else { &name }))
}

/// The lock that has the hook's runs for `session` take turns, held until
/// it is dropped; none where it cannot be taken, which leaves the runs as
/// they were before there was one.
fn lock_born(home: &Path, session: &str) -> Option<std::fs::File> {
    std::fs::create_dir_all(born_dir(home)).ok()?;
    crate::storage::lock_path(&born_file(home, session).with_extension("lock")).ok()
}

fn read_born(home: &Path, session: &str) -> Born {
    std::fs::read_to_string(born_file(home, session)).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

/// Written whole and put in place by a rename, for the closer, which reads
/// it from another process.
fn write_born(home: &Path, session: &str, born: &Born) {
    let file = born_file(home, session);
    let written = std::fs::create_dir_all(born_dir(home)).and_then(|()| {
        let fresh = file.with_extension(format!("json.{}", std::process::id()));
        std::fs::write(&fresh, serde_json::to_string(born).unwrap_or_default())?;
        std::fs::rename(&fresh, &file)
    });
    if let Err(error) = written {
        eprintln!("ekko: what this session's prompts were could not be kept: {error}");
    }
}

/// What born.log last said of the session born for the tasks `uids`, in
/// any order, as `HH:MM:SS: what`: why it stays, where it does (task 1809).
pub(crate) fn born_said(home: &Path, uids: &[String]) -> Option<String> {
    let mut wanted: Vec<&str> = uids.iter().map(String::as_str).collect();
    wanted.sort_unstable();
    let log = std::fs::read_to_string(born_dir(home).join("born.log")).ok()?;
    log.lines().rev().find_map(|line| {
        let mut fields = line.splitn(4, ' ');
        let (at, _session, tasks, what) = (fields.next()?, fields.next()?, fields.next()?, fields.next()?);
        let mut named: Vec<&str> = tasks.split(',').collect();
        named.sort_unstable();
        (named == wanted).then(|| format!("{}: {what}", at.split_once('T').map_or(at, |(_, time)| time.split('.').next().unwrap_or(time))))
    })
}

/// A line of `born.log`: when, the session, its tasks, and what was decided.
fn log(home: &Path, session: &str, tasks: &str, what: &str) {
    use std::io::Write as _;
    let line = format!("{} {} {tasks} {what}\n", chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.3f"), session.get(..8).unwrap_or(session));
    let _ = std::fs::create_dir_all(born_dir(home))
        .and_then(|()| std::fs::OpenOptions::new().create(true).append(true).open(born_dir(home).join("born.log")))
        .and_then(|mut file| file.write_all(line.as_bytes()));
}


#[cfg(test)]
mod tests {
    use super::*;

    fn pane_in(window: &str, tasks: &[u32], project: Option<&str>, dead: bool) -> BornPane {
        BornPane {
            pane: "%1".into(),
            dead,
            window_id: "@1".into(),
            window: window.into(),
            tasks: tasks.to_vec(),
            title: String::new(),
            model: "haiku".into(),
            effort: None,
            state: "working".into(),
            since: None,
            waits: None,
            project: project.map(str::to_string),
        }
    }

    #[test]
    fn another_projects_session_of_the_same_task_is_not_this_projects_window_or_pane() {
        let windows = vec!["3 \u{b7} haiku".to_string(), "wall".to_string()];
        let there = [pane_in("3 \u{b7} haiku", &[3], Some("site"), false), pane_in("wall", &[4], Some("site"), false)];
        assert_eq!(window_of("app", &[3], &windows, &there), None, "its window names another project");
        assert_eq!(pane_of("app", &[3, 4], &there).map(|(id, _)| id), None);
        // This project's, or one that names none (before task 1727), or a window no pane lists.
        for project in [Some("app"), None] {
            let here = [pane_in("3 \u{b7} haiku", &[3], project, false), pane_in("wall", &[4], project, false)];
            assert_eq!(window_of("app", &[3], &windows, &here).map(|(id, _)| id), Some(3));
            assert_eq!(pane_of("app", &[4], &here).map(|(id, _)| id), Some(4));
        }
        assert_eq!(window_of("app", &[3], &windows, &[]).map(|(id, _)| id), Some(3));
        // Two windows of one name, one of each project: this one's still holds it.
        let both = [pane_in("3 \u{b7} haiku", &[3], Some("site"), false), pane_in("3 \u{b7} haiku", &[3], Some("app"), false)];
        assert_eq!(window_of("app", &[3], &windows, &both).map(|(id, _)| id), Some(3));
        // A dead pane holds nothing, whoever's.
        assert_eq!(pane_of("app", &[3], &[pane_in("x", &[3], Some("app"), true)]).map(|(id, _)| id), None);
    }
    #[test]
    fn a_window_names_its_tasks_before_its_first_dot() {
        assert_eq!(tasks_of("12 \u{b7} haiku"), vec![12]);
        assert_eq!(tasks_of("3,4 \u{b7} sonnet \u{b7} waiting on you"), vec![3, 4]);
        assert_eq!(tasks_of("12"), Vec::<u32>::new(), "a name start did not give");
        assert_eq!(tasks_of("notes \u{b7} haiku"), Vec::<u32>::new());
    }

    /// Each option `claude` takes a value after hides no command: what
    /// follows the value is, a command that starts no session, one that ends
    /// one, or a prompt (tasks 1646 and 1716). Each command is one of the
    /// three, and asking it for its help ends none.
    #[test]
    fn a_value_of_claude_s_options_is_never_taken_for_its_command() {
        let limit = |args: &[&str]| {
            let (calls, placed) = crate::shell::located(&format!("claude {}", args.join(" ")), Path::new("/"), crate::shell::Reading::GUARD);
            limit_of(&calls[placed[0].at], &placed[0], &Caller::default())
        };
        for option in CLAUDE_VALUED {
            assert_eq!(limit(&[option, "value", "mcp", "list"]), None, "{option}");
            assert_eq!(limit(&[option, "value", "stop", "4f2a"]), Some(Limit::Stop), "{option}");
            assert_eq!(limit(&[option, "value", "'write it'"]), Some(Limit::Session), "{option}");
        }
        for command in NO_SESSION {
            assert_eq!(limit(&[command]), None, "{command}");
        }
        for command in ON_SESSIONS {
            assert_eq!(limit(&[command, "4f2a"]), Some(Limit::Stop), "{command}");
            assert_eq!(limit(&[command, "--help"]), None, "{command}");
        }
        // Purging deletes what it names, but for a dry run (task 1742).
        assert_eq!(limit(&["purge"]), Some(Limit::Stop));
        assert_eq!(limit(&["purge", "--all", "-y"]), Some(Limit::Stop));
        assert_eq!(limit(&["purge", "--dry-run", "/projects/x"]), None);
        for command in ["agents", "attach", "ultrareview"] {
            assert_eq!(limit(&[command]), Some(Limit::Session), "{command}");
        }
    }

    /// Every word tmux would run as a command that kills or respawns is
    /// refused as a kill (task 1716), and none it would run as another but
    /// one that types, here into a pane of the server another `-L` names
    /// (task 1742): each command's name, its alias and every start of its
    /// name, found as tmux's `cmd_find` finds one, for each multiplexer and
    /// for the binary `EKKO_MUX` names; a start more commands share runs
    /// none, either way.
    #[test]
    fn a_multiplexer_command_that_kills_or_respawns_is_refused_as_tmux_finds_it() {
        let table: Vec<(&str, &str)> = TMUX_COMMANDS.split_whitespace().map(|entry| entry.split_once(':').unwrap()).collect();
        assert_eq!(table.len(), 93);
        let kill = |name: &str| name.starts_with("kill-") || name.starts_with("respawn-");
        assert_eq!(KILLS.len(), table.iter().filter(|(name, _)| kill(name)).count());
        for (name, alias) in KILLS {
            assert!(table.contains(&(*name, alias.unwrap_or_default())), "{name} is tmux's, with its alias");
        }
        // What tmux runs for `word`: `None` where it runs nothing.
        let found = |word: &str| -> Option<&str> {
            if let Some((name, _)) = table.iter().find(|(_, alias)| *alias == word) {
                return Some(*name);
            }
            let starting: Vec<&str> = table.iter().map(|(name, _)| *name).filter(|name| name.starts_with(word)).collect();
            starting.iter().find(|name| **name == word).or(match starting.as_slice() {
                [one] => Some(one),
                _ => None,
            }).copied()
        };
        let limit = |command: &str, mux: Option<&str>| {
            let (calls, placed) = crate::shell::located(command, Path::new("/"), crate::shell::Reading::GUARD);
            limit_of(&calls[placed[0].at], &placed[0], &Caller { mux: mux.map(str::to_string), ..Caller::default() })
        };
        let mut words: Vec<String> = table.iter().filter(|(_, alias)| !alias.is_empty()).map(|(_, alias)| alias.to_string()).collect();
        for (name, _) in &table {
            words.extend((1..=name.len()).map(|end| name[..end].to_string()));
        }
        let mut refused = 0;
        for word in &words {
            let Some(runs) = found(word) else { continue };
            for binary in MUXES.iter().copied().chain(["/opt/mux/bin/mymux"]) {
                let mux = binary.contains('/').then_some("mymux");
                // Typing names a pane on the server -L names: another's.
                let expected = if kill(runs) {
                    Some(Limit::Kill)
                } else {
                    ["send-keys", "paste-buffer", "send-prefix"].contains(&runs).then_some(Limit::Keys)
                };
                assert_eq!(limit(&format!("{binary} -L ekko {word} -t%3"), mux), expected, "{binary} {word}, which runs {runs}");
                refused += usize::from(expected.is_some());
            }
        }
        assert!(refused > 4 * 6 * 3, "each kill-* and respawn-* by its name, its alias and starts of its name: {refused}");
        assert_eq!(limit("mymux kill-server", None), None, "a binary EKKO_MUX does not name");
    }

    /// tmux's command line, as `mux_limit` reads it: getopt's options, each
    /// value taken whole or from the next word, then commands, one after
    /// each argument ending in a `;` that is not escaped.
    #[test]
    fn a_kill_is_found_past_tmux_s_options_and_in_each_command_of_a_sequence() {
        let reached = |args: &[&str]| mux_limit(&args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>(), None);
        let read: &[(&[&str], Option<Limit>)] = &[
            (&["kill-server"], Some(Limit::Kill)),
            (&["-L", "ekko", "kill-session", "-t", "ekko"], Some(Limit::Kill)),
            (&["-Lekko", "kill-pane"], Some(Limit::Kill)),
            (&["-uL", "ekko", "-f", "/dev/null", "kill-window"], Some(Limit::Kill)),
            (&["-2", "-S", "/tmp/s", "-T", "RGB", "killp"], Some(Limit::Kill)),
            (&["--", "kill-server"], Some(Limit::Kill)),
            (&["new-window", ";", "kill-pane", "-t", "%3"], Some(Limit::Kill)),
            (&["new-window", "-n", "x;", "respawn-pane", "-k"], Some(Limit::Kill)),
            (&["display", "-p", "hi", ";", ";", "killw"], Some(Limit::Kill)),
            (&["kill-server;"], Some(Limit::Kill)),
            // tmux's own -c runs a shell command, as a new window's does.
            (&["-c", "kill-server"], Some(Limit::Runs)),
            (&["-uc", "kill-server"], Some(Limit::Runs)),
            (&["new-window", "kill-server"], Some(Limit::Runs)),
            (&[], None),
            (&["-L", "kill-server", "ls"], None),
            (&["-f", "kill-pane", "new-session"], None),
            (&["display", "-p", "kill-pane"], None),
            (&["display", "-p", r"a\;", "kill-pane"], None),
            (&["list-panes", "-a"], None),
            (&["-V"], None),
            // kmux's own verbs, which it runs before tmux's commands (task
            // 1746): first after the options only.
            (&["revive"], Some(Limit::Verb)),
            (&["-L", "ekko", "bcast", "kill-server"], Some(Limit::Verb)),
            (&["display", "-p", "x", ";", "revive"], None),
        ];
        for (args, limit) in read {
            assert_eq!(reached(args), *limit, "{args:?}");
        }
    }

    /// Every verb of kmux's own, as `kmux verbs` lists its extensions
    /// (3.7.47), is refused in a born session, whatever it is given and
    /// past any option -- `info` too, which tmux reads as its alias -- and
    /// so is a word tmux reads as no command, as a newer kmux's verb would
    /// be, while none of tmux's commands or aliases is (task 1746, the
    /// user's decision 1910).
    #[test]
    fn each_of_kmux_s_own_verbs_is_refused_as_kmux_verbs_lists_them() {
        let listed = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/kmux-3.7.47-verbs.txt")).unwrap();
        let section = |title: &str| -> Vec<String> {
            let lines = listed.lines().skip_while(|line| !line.starts_with(title)).skip(1);
            lines.take_while(|line| !line.trim().is_empty()).filter_map(|line| line.split_whitespace().next().map(str::to_string)).collect()
        };
        let (verbs, commands, aliases) = (section("EXTENSIONS"), section("COMMANDS"), section("ALIASES"));
        assert_eq!((verbs.len(), commands.len(), aliases.len()), (114, 93, 78));
        for word in ["revive", "bcast", "prune", "resurrect", "solo", "sync", "stash", "dead", "info"] {
            assert!(verbs.contains(&word.to_string()), "{word}");
        }
        let limit = |command: &str, mux: Option<&str>| -> Vec<Limit> {
            let (calls, placed) = crate::shell::located(command, Path::new("/"), crate::shell::Reading::GUARD);
            let caller = Caller { mux: mux.map(str::to_string), pane: Some("%7".into()), ..Caller::default() };
            placed.iter().filter_map(|place| limit_of(&calls[place.at], place, &caller)).collect()
        };
        for verb in &verbs {
            for command in [format!("kmux {verb}"), format!("kmux -L ekko {verb} --json"), format!("timeout 5 kmux -uS /tmp/s {verb};")] {
                assert_eq!(limit(&command, None), [Limit::Verb], "{command}");
            }
            assert_eq!(limit(&format!("/opt/mux/bin/mymux {verb}"), Some("mymux")), [Limit::Verb], "{verb}, by the binary EKKO_MUX names");
        }
        // The verbs tmux reads as one of its commands, which only the table names.
        let tmux_reads = |word: &str| TMUX_ALIASES.iter().any(|(alias, _)| *alias == word) || tmux_command(word).is_some();
        let shadowing: Vec<&str> = verbs.iter().map(String::as_str).filter(|verb| tmux_reads(verb)).collect();
        assert_eq!(shadowing, SHADOWING_VERBS);
        assert_eq!(limit("kmux zzverb", None), [Limit::Verb], "a word tmux reads as no command");
        for word in commands.iter().chain(&aliases) {
            assert!(limit(&format!("kmux {word} -t %7"), None).iter().all(|limit| *limit != Limit::Verb), "{word}, tmux's");
        }
        for command in ["kmux", "kmux -V", "kmux ls", "kmux list-panes -a", "tmux display -p '#{pane_id}'", "echo kmux revive", "kmux display -p revive"] {
            assert_eq!(limit(command, None), [], "{command}");
        }
    }

    /// The commands that run one they are given, read from their usage
    /// lines as `list-commands` prints them (tmux 3.7c and kmux 3.7.47):
    /// each whose operands take a `command`, a `shell-command` or a
    /// `template` -- but list-commands, whose names one -- or whose option
    /// takes one, and source-file, whose path is a file of them. `REACHES`
    /// holds each, past as many operands or by that option, and none other;
    /// each of its commands takes a value after the options either binary
    /// takes one after. Read: run past them, given a value after each such
    /// option, and not short of them.
    #[test]
    fn each_mux_command_that_runs_one_it_is_given_is_read_from_its_usage() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let runs = ["command", "shell-command", "template"];
        // A command's options with a value, and the operand past which, or
        // the option by which, it takes one to run.
        type Read = (std::collections::BTreeSet<char>, Option<usize>, Option<char>);
        let mut usage: std::collections::BTreeMap<String, Read> = Default::default();
        for (file, commands) in [("tmux-3.7c-list-commands.txt", 91), ("kmux-3.7.47-list-commands.txt", 93)] {
            let listed = std::fs::read_to_string(fixtures.join(file)).unwrap();
            assert_eq!(listed.lines().count(), commands, "{file}");
            for line in listed.lines() {
                let (name, mut rest) = line.split_once(' ').unwrap_or((line, ""));
                if let Some(aliased) = rest.strip_prefix('(') {
                    rest = aliased.split_once(") ").map_or("", |(_, rest)| rest);
                }
                let (valued, past, by) = usage.entry(name.to_string()).or_default();
                let mut operands: Vec<&str> = Vec::new();
                while let Some(word) = Some(rest.trim_start()).filter(|word| !word.is_empty()) {
                    if let Some(group) = word.strip_prefix("[-") {
                        let (inner, after) = group.split_once(']').unwrap();
                        if let Some((flag, placeholder)) = inner.split_once(' ') {
                            let flag = flag.chars().next().unwrap();
                            valued.insert(flag);
                            if runs.contains(&placeholder) {
                                *by = Some(flag);
                            }
                        }
                        rest = after;
                    } else {
                        let (operand, after) = word.split_once(' ').unwrap_or((word, ""));
                        operands.push(operand.trim_matches(['[', ']']));
                        rest = after;
                    }
                }
                *past = match name {
                    "list-commands" => None,
                    "source-file" => Some(0),
                    _ => operands.iter().position(|operand| runs.contains(operand)),
                };
            }
        }
        let mut taken = 0;
        for (name, (valued, past, by)) in &usage {
            let reach = REACHES.iter().find(|reach| reach.name == name);
            if KILLS.iter().any(|(kill, _)| kill == name) || (past.is_none() && by.is_none()) {
                assert!(reach.is_none_or(|reach| reach.runs_past.is_none() && reach.runs_by.is_none()), "{name} runs none it is given");
                if let Some(reach) = reach {
                    assert_eq!(reach.valued.chars().collect::<std::collections::BTreeSet<char>>(), *valued, "{name}'s options with a value");
                }
                continue;
            }
            let reach = reach.unwrap_or_else(|| panic!("{name} runs one it is given"));
            assert_eq!((reach.runs_past, reach.runs_by), (*past, *by), "{name}");
            assert_eq!(reach.valued.chars().collect::<std::collections::BTreeSet<char>>(), *valued, "{name}'s options with a value");
            taken += 1;
            let words = |count: usize| (0..count).map(|at| format!("w{at}"));
            for named in std::iter::once(name.as_str()).chain(TMUX_COMMANDS.split_whitespace().filter_map(|entry| entry.split_once(':')).filter(|(listed, alias)| listed == name && !alias.is_empty()).map(|(_, alias)| alias)) {
                let short = past.unwrap_or_default();
                let call = |options: Vec<String>, count: usize| mux_limit(&std::iter::once(named.to_string()).chain(options).chain(words(count)).collect::<Vec<_>>(), None);
                assert_eq!(call(Vec::new(), short), None, "{named} short of a command");
                if past.is_some() {
                    assert_eq!(call(Vec::new(), short + 1), Some(Limit::Runs), "{named} given a command");
                }
                for flag in valued.iter().filter(|flag| Some(**flag) != *by) {
                    assert_eq!(call(vec![format!("-{flag}"), "value".into()], short), None, "{named} -{flag} value, short of a command");
                }
                if let Some(by) = by {
                    assert_eq!(call(vec![format!("-{by}"), "true".into()], 0), Some(Limit::Runs), "{named} -{by}");
                }
            }
        }
        assert_eq!(REACHES.iter().filter(|reach| reach.runs_past.is_some() || reach.runs_by.is_some()).count(), taken);
        assert_eq!(taken, 21, "the commands that run one they are given");
    }

    /// The options whose value tmux runs: each hook, as `show-hooks` lists
    /// them, and each option that holds a command or a shell, set by any
    /// name of set-option, its window form or an item of the array; none
    /// other but by its format's `#()`, which runs anywhere.
    #[test]
    fn an_option_whose_value_tmux_runs_is_refused_and_a_format_s_shell_command() {
        let hooks = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tmux-3.7c-hooks.txt")).unwrap();
        let mut hooks: Vec<&str> = hooks.lines().collect();
        let mut held: Vec<&str> = RUN_OPTIONS.split_whitespace().collect();
        let holding = held.split_off(held.len() - 7);
        assert_eq!(holding, ["command-alias", "copy-command", "default-client-command", "default-command", "default-shell", "editor", "lock-command"]);
        hooks.sort_unstable();
        held.sort_unstable();
        assert_eq!(held, hooks, "every hook, and none other");
        let reached = |args: &[&str]| mux_limit(&args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>(), None);
        for option in RUN_OPTIONS.split_whitespace() {
            for set in ["set-option", "set", "set-window-option", "setw"] {
                assert_eq!(reached(&[set, "-g", option, "kill-server"]), Some(Limit::Runs), "{set} {option}");
                assert_eq!(reached(&[set, "-t", "ekko", &format!("{option}[2]"), "x"]), Some(Limit::Runs), "{set} {option}[2]");
            }
        }
        for (option, value) in [("status-style", "bg=red"), ("mouse", "on"), ("@ekko_tasks", "kill-server"), ("pane-border-format", "#{pane_id}"), ("status-right", "#(date)")] {
            let expected = value.contains("#(").then_some(Limit::Runs);
            assert_eq!(reached(&["set", "-g", option, value]), expected, "{option} {value}");
        }
        assert_eq!(reached(&["display", "-p", "#(pkill claude)"]), Some(Limit::Runs));
        assert_eq!(reached(&["neww", "-n", "#(date)"]), Some(Limit::Runs));
        assert_eq!(reached(&["display", "-p", "#{pane_id} (#S)"]), None);
        assert_eq!(reached(&["set-hook", "-gu", "pane-died"]), None, "a hook unset runs nothing");
    }

    /// The `-k` of the four commands that kill the window it replaces or
    /// leaves, and typing into a pane: refused into another than the
    /// session's own, the last `-t` named, or into any on another server;
    /// and a copy mode command piping to one it is given.
    #[test]
    fn a_window_s_k_and_keys_typed_into_another_s_pane_are_refused() {
        let reached = |args: &[&str], own: Option<&str>| mux_limit(&args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>(), own);
        for command in ["new-window", "neww", "link-window", "linkw", "move-window", "movew", "unlink-window", "unlinkw"] {
            assert_eq!(reached(&[command, "-k", "-t", "ekko:3"], None), Some(Limit::Kill), "{command} -k");
            assert_eq!(reached(&[command, "-dk"], None), Some(Limit::Kill), "{command} -dk");
            assert_eq!(reached(&[command, "-d", "-t", "ekko:3"], None), None, "{command} -d");
        }
        assert_eq!(reached(&["neww", "-n", "k"], None), None, "a value k");
        assert_eq!(reached(&["neww", "-nk"], None), None, "a value k");
        assert_eq!(reached(&["neww", "--", "-k"], None), Some(Limit::Runs), "a command after --");
        assert_eq!(reached(&["break-pane", "-k"], None), None, "a command whose -k kills no window");
        let own = Some("%7");
        for command in ["send-keys", "send", "paste-buffer", "pasteb", "send-prefix"] {
            assert_eq!(reached(&[command, "-t", "%9"], own), Some(Limit::Keys), "{command} -t %9");
            assert_eq!(reached(&[command, "-t%9"], own), Some(Limit::Keys), "{command} -t%9");
            assert_eq!(reached(&[command, "-t", "ekko:3"], own), Some(Limit::Keys), "{command} -t ekko:3");
            assert_eq!(reached(&[command, "-t", "%9", "-t", "%7"], own), None, "{command}, the last -t its own");
            assert_eq!(reached(&[command, "-t", "%7"], own), None, "{command} -t its own");
            assert_eq!(reached(&[command], own), None, "{command} with no -t");
            assert_eq!(reached(&["-L", "other", command], own), Some(Limit::Keys), "{command} on another server");
            assert_eq!(reached(&["-S", "/tmp/other", command, "-t", "%7"], own), Some(Limit::Keys), "{command} on another server");
            assert_eq!(reached(&[command, "-t", "%7"], None), Some(Limit::Keys), "{command} out of a pane");
        }
        assert_eq!(reached(&["send-keys", "-X", "copy-pipe", "sh -c 'pkill claude'"], own), Some(Limit::Runs));
        assert_eq!(reached(&["send", "-X", "-t", "%7", "copy-pipe-and-cancel", "x"], own), Some(Limit::Runs));
        assert_eq!(reached(&["send-keys", "-X", "copy-pipe"], own), None, "the user's copy-command");
        assert_eq!(reached(&["send-keys", "-X", "cancel"], own), None);
        assert_eq!(reached(&["send-keys", "copy-pipe", "x"], own), None, "keys typed");
    }

    /// kill(1), pkill, killall and skill, as the guard reads calls: refused
    /// whatever they are given -- a pid, a name, nothing, as `xargs` gives
    /// them theirs -- under any wrapper or shell, but to list the signals or
    /// ask for their help or version; named only, none. A kill of signal 0,
    /// which signals none, and one whose every pid is the session's own --
    /// `$!`, `$$`, a job, a variable holding one, a process below its Claude
    /// Code process -- go through (task 1849), each form in the transcripts
    /// of 2026-10-11 to 14 among them; a pkill pattern does not.
    #[test]
    fn a_program_that_signals_processes_is_refused_but_to_list_the_signals_or_its_own() {
        // This test's process stands for the session's Claude Code: a child
        // of its is the session's, and so is that child's child.
        let mut child = Command::new("sh").args(["-c", "sleep 30 & wait"]).spawn().unwrap();
        let mut grandchild = String::new();
        for _ in 0..250 {
            let children = std::fs::read_to_string(format!("/proc/{0}/task/{0}/children", child.id())).unwrap_or_default();
            if let Some(pid) = children.split_whitespace().next() {
                grandchild = pid.to_string();
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!grandchild.is_empty(), "sh started its sleep");
        let caller = Caller { claude: Some(std::process::id()), ..Caller::default() };
        let reached = |command: &str| -> Vec<Limit> {
            let (calls, placed) = crate::shell::located(command, Path::new("/"), crate::shell::Reading::GUARD);
            placed.iter().filter_map(|place| limit_of(&calls[place.at], place, &caller)).collect()
        };
        let (own, parent) = (child.id(), std::os::unix::process::parent_id());
        for command in [
            "kill 4242".to_string(),
            "kill -9 4242".into(),
            "kill -s HUP 4242".into(),
            "kill".into(),
            "kill -9".into(),
            "kill -9 4242 -l".into(),
            "/bin/kill -TERM 4242".into(),
            "command kill 4242".into(),
            "timeout 5 kill 4242".into(),
            "pgrep claude | xargs kill".into(),
            "pgrep claude | xargs -r kill -9".into(),
            "sh -c 'pkill -f claude'".into(),
            "pkill -f claude".into(),
            "pkill".into(),
            "/usr/bin/pkill -u roldant".into(),
            "killall claude".into(),
            "killall".into(),
            "skill -9 claude".into(),
            "echo \"`kill 1`\"".into(),
            "echo \"$(kill 1)\"".into(),
            // Not the session's: its Claude Code itself, what runs above it,
            // a process group, a pid a variable or a file holds, a pkill
            // pattern however narrow.
            format!("kill {}", std::process::id()),
            format!("kill {parent}"),
            "kill 1".into(),
            format!("kill {own} 4242"),
            format!("kill -9 -{own}"),
            format!("kill -- -{own}"),
            "kill $PID".into(),
            "x=4242; kill $x".into(),
            "kill $(cat /tmp/x.pid)".into(),
            "kill -0 4242 && kill 4242".into(),
            "kill --verbose 4242".into(),
            "pkill -A -x -f /tmp/nixdk-415/bin/nixdk".into(),
            "pkill -fA 'timeout 3600 Xvfb -displayfd 3'".into(),
        ] {
            assert_eq!(reached(&command), [Limit::Signal], "{command}");
        }
        for command in [
            "kill -l".to_string(),
            "kill -l 9".into(),
            "kill -L".into(),
            "kill --help".into(),
            "killall -l".into(),
            "killall --version".into(),
            "pkill --help".into(),
            "pkill -V".into(),
            "pgrep -f claude".into(),
            "echo kill 4242".into(),
            "git commit -m 'kill 4242'".into(),
            // Single-quoted, a substitution is text (task 1848).
            "echo '`kill 1`'".into(),
            "echo '$(kill 1)'".into(),
            "sed -i '16s#upstream `kill(-1, 0)` asked#x#' docs/PORTING.md".into(),
            // Signal 0 asks whether a process is there (seek 102, 103, 107).
            "kill -0 4242".into(),
            "kill -0 \"$(cat /tmp/kwin.pid)\"".into(),
            "kill -s 0 4242".into(),
            "kill -n 0 4242".into(),
            "kill --signal=0 4242".into(),
            // What the command itself started (blackbox 4, 5, 14 and 423).
            "nix develop -c bash -c 'Xvfb :99 & x=$!; sleep 1; kill $x'".into(),
            "sleep 9 & kill $!".into(),
            "sleep 9 & pid=$!; kill -TERM \"${pid}\"".into(),
            "sh -c 'kill -ABRT $$'".into(),
            "kill -9 $BASHPID".into(),
            "sleep 9 & kill %1".into(),
            "builtin kill %1".into(),
            "kill -- $!".into(),
            // A process below its Claude Code.
            format!("kill {own}"),
            format!("kill -9 {own} {grandchild}"),
            format!("pid={own}; kill $pid"),
        ] {
            assert_eq!(reached(&command), [], "{command}");
        }
        assert_eq!(limit_of(&crate::shell::calls(&format!("kill {own}"))[0], &crate::shell::located("x", Path::new("/"), crate::shell::Reading::GUARD).1[0], &Caller::default()), Some(Limit::Signal), "with no Claude Code process known, a pid is no one's own");
        child.kill().ok();
        child.wait().ok();
    }

    /// A Claude Code process is a born session when its environment names
    /// the tasks it was started for, or its conversation has a born file;
    /// the check reads a process, not the environment of this one.
    #[test]
    fn a_process_is_born_by_its_environment_or_by_its_conversation_s_born_file() {
        let home = std::env::temp_dir().join(format!("ekko-is-born-{}", std::process::id()));
        let spawn = |born: bool| {
            let mut command = std::process::Command::new("sleep");
            command.arg("30").env_remove(TASKS_VAR);
            if born {
                command.env(TASKS_VAR, "uid-1");
            }
            command.spawn().unwrap()
        };
        let running = |child: &std::process::Child, conversation: &str| {
            let process = Process::of(child.id()).unwrap();
            crate::holder::Running {
                pid: process.pid,
                start: process.start,
                boot: process.boot,
                profile: None,
                tty: None,
                config_dir: None,
                board: None,
                transcript: None,
                conversation: conversation.to_string(),
                earlier: vec!["before".to_string()],
                since: 0,
            }
        };
        let (mut born, mut plain) = (spawn(true), spawn(false));
        assert!(is_born(&home, &running(&born, "c-1")));
        assert!(!is_born(&home, &running(&plain, "c-2")));
        write_born(&home, "before", &Born::default());
        assert!(is_born(&home, &running(&plain, "c-2")), "its hook kept a file for a conversation it ran before");
        born.kill().ok();
        plain.kill().ok();
        born.wait().ok();
        plain.wait().ok();
        std::fs::remove_dir_all(&home).ok();
    }

    /// A value a pane holds for the views reads back as given in a status
    /// line or a border (task 1675): on one line, a `#` doubled, which
    /// would start a style there, and a `;` that ends it escaped, which
    /// tmux would take for the end of the command; a `;` inside is tmux's
    /// to keep.
    #[test]
    fn a_value_for_the_views_is_shown_as_given() {
        assert_eq!(shown("Fix #3 #[fg=red]"), "Fix ##3 ##[fg=red]");
        assert_eq!(shown("then ship;"), "then ship\\;");
        assert_eq!(shown(";"), "\\;");
        assert_eq!(shown("a ; b;c"), "a ; b;c");
        assert_eq!(shown("two\nlines\tand a tab"), "two lines and a tab");
        assert_eq!(shown("plain"), "plain");
    }

    #[test]
    fn ids_are_listed_as_a_sentence_says_them() {
        assert_eq!(listed(&[1]), "1");
        assert_eq!(listed(&[1, 2]), "1 and 2");
        assert_eq!(listed(&[1, 2, 3]), "1, 2 and 3");
    }

    /// What a permission is asked for reads as the tool and what it was
    /// given -- its command, file, address or pattern, else its input --
    /// on one line, and short.
    #[test]
    fn a_permission_is_asked_for_a_tool_and_what_it_was_given() {
        let asked = |tool: &str, input: serde_json::Value| asked_for(&serde_json::json!(tool), &input);
        assert_eq!(asked("Bash", serde_json::json!({"command": "date > stamp.txt", "description": "Write the date"})), "Bash: date > stamp.txt");
        assert_eq!(asked("Write", serde_json::json!({"file_path": "/a/b.rs", "content": "x"})), "Write: /a/b.rs");
        assert_eq!(asked("WebFetch", serde_json::json!({"url": "https://example.com", "prompt": "p"})), "WebFetch: https://example.com");
        assert_eq!(asked("mcp__ekko__trash", serde_json::json!({"items": [3]})), "mcp__ekko__trash: {\"items\":[3]}");
        assert_eq!(asked_for(&serde_json::Value::Null, &serde_json::Value::Null), "a tool");
        let long = asked("Bash", serde_json::json!({"command": format!("cat <<'EOF'\n{}\nEOF", "word ".repeat(40))}));
        assert!(!long.contains('\n') && long.ends_with(" chars)") && long.chars().count() < 120, "{long}");
    }

    /// The plugin runs the born hook on every event it acts on, for every
    /// tool where the event has tools, and on Notification for each kind
    /// that says the session waits on the user: an event or a kind left out
    /// of plugin.json is one the hook never hears of (task 1645).
    #[test]
    fn the_plugin_runs_the_born_hook_on_every_event_it_acts_on() {
        let plugin = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("plugin/.claude-plugin/plugin.json")).unwrap();
        let plugin: serde_json::Value = serde_json::from_str(&plugin).unwrap();
        let born = |entry: &&serde_json::Value| entry["hooks"].as_array().is_some_and(|hooks| hooks.iter().any(|hook| hook["command"] == "ekko --born --hook"));
        let mut events = Vec::new();
        for (event, entries) in plugin["hooks"].as_object().unwrap() {
            for entry in entries.as_array().unwrap().iter().filter(born) {
                events.push(event.as_str());
                let matcher = entry["matcher"].as_str();
                if event == "Notification" {
                    let mut kinds: Vec<&str> = matcher.unwrap_or_default().split('|').collect();
                    let mut waits = WAITS_ON_USER.to_vec();
                    kinds.sort_unstable();
                    waits.sort_unstable();
                    assert_eq!(kinds, waits, "the kinds of Notification the plugin sends the born hook");
                } else {
                    assert_eq!(matcher, None, "{event} reaches the born hook for some tools only");
                }
            }
        }
        let mut expected = EVENTS.to_vec();
        events.sort_unstable();
        expected.sort_unstable();
        assert_eq!(events, expected, "the events the plugin runs the born hook on");
        // Every event that ends a turn, an API error's included (task 1798).
        for end in TURN_ENDS {
            assert!(events.contains(end), "a turn that ends on {end} is one the born hook never hears of");
        }
    }

    /// The tools that finish a born session's work on the board are ekko's
    /// writes, under whatever name its MCP server is registered, and its
    /// command line in Bash; its reads and other tools are not.
    #[test]
    fn the_write_that_may_finish_a_born_session_s_work_is_ekko_s() {
        let none = serde_json::Value::Null;
        for tool in ["mcp__plugin_ekko_ekko__set_state", "mcp__ekko__set_state", "mcp__plugin_ekko_ekko__create", "mcp__ekko__batch", "mcp__plugin_ekko_ekko__force_state"] {
            assert!(writes_the_board(tool, &none), "{tool}");
        }
        for tool in ["mcp__plugin_ekko_ekko__context", "mcp__plugin_graff_graff__def", "mcp__other__set_state", "Read", "Edit"] {
            assert!(!writes_the_board(tool, &none), "{tool}");
        }
        assert!(writes_the_board("Bash", &serde_json::json!({"command": "ekko --check 3"})));
        assert!(!writes_the_board("Bash", &serde_json::json!({"command": "cargo test"})));
    }

    /// The command a tool shell runs, as born.log names it (task 1829): what
    /// it evals, its quotes as the call gave them, in the form Claude Code
    /// 2.1.302 wraps a Bash call and a Monitor in; the whole line otherwise.
    #[test]
    fn a_tool_shell_s_command_is_what_it_evals() {
        let wrapped = |eval: &str, end: &str| {
            format!(
                "/run/current-system/sw/bin/bash -c source /home/u/.claude/shell-snapshots/snapshot-bash-1-k.sh 2>/dev/null || true && shopt -u extglob 2>/dev/null || true && eval '{eval}'{end}"
            )
        };
        let quoted = "until ! pgrep -f '\"'\"'cargo test'\"'\"'; do sleep 5; done";
        assert_eq!(tool_command(&wrapped(quoted, " < /dev/null && pwd -P >| /tmp/claude-720e-cwd")), "until ! pgrep -f 'cargo test'; do sleep 5; done");
        assert_eq!(tool_command(&wrapped("tail -F x.log", " && pwd -P >| /tmp/claude-1-cwd")), "tail -F x.log");
        assert_eq!(tool_command("sleep 300"), "sleep 300");
    }

    /// What born.log last said of a session, found by its tasks in any order
    /// (task 1809): not another session's line, though it names one of them.
    #[test]
    fn born_log_s_last_word_on_a_session_is_found_by_its_tasks() {
        let home = crate::paths::test_dir("ekko-born-said");
        let uids = ["u-1".to_string(), "u-2".to_string()];
        assert_eq!(born_said(&home, &uids), None, "no born.log");
        std::fs::create_dir_all(born_dir(&home)).unwrap();
        let lines = [
            "2026-10-18T01:20:00.000 aaaaaaaa u-1,u-2 its tasks are finished: closing",
            "2026-10-18T01:21:45.123 aaaaaaaa u-2,u-1 stays: background work is in flight",
            "2026-10-18T01:22:00.000 bbbbbbbb u-1 closed: its Claude Code ended",
            "2026-10-18T01:23:00.000 cccccccc u-1,u-2,u-3 stays: task 3 is pending",
        ];
        std::fs::write(born_dir(&home).join("born.log"), lines.join("\n") + "\n").unwrap();
        assert_eq!(born_said(&home, &uids).as_deref(), Some("01:21:45: stays: background work is in flight"));
        assert_eq!(born_said(&home, &["u-1".to_string()]).as_deref(), Some("01:22:00: closed: its Claude Code ended"));
        assert_eq!(born_said(&home, &["u-9".to_string()]), None);
        std::fs::remove_dir_all(&home).ok();
    }

    /// A repository with a commit, made in a fresh folder under `tag`.
    fn repository_at(tag: &str) -> PathBuf {
        let dir = crate::paths::test_dir(&format!("ekko-agents-{tag}"));
        std::fs::create_dir_all(&dir).unwrap();
        let dir = std::fs::canonicalize(&dir).unwrap();
        for args in [&["init", "-q", "-b", "main"][..], &["-c", "user.name=e", "-c", "user.email=e@x", "commit", "-q", "--allow-empty", "-m", "first"]] {
            assert!(Command::new("git").current_dir(&dir).args(args).status().unwrap().success(), "{args:?}");
        }
        dir
    }

    /// A born session's checkout (task 1800): its worktree on `task-1`,
    /// beside the project's own checkout, `main`, and a scratch repository
    /// elsewhere. Each git command that writes a ref the checkouts share --
    /// another branch, a tag, the stash, notes, replacements, worktrees,
    /// remotes, reflogs, config -- or puts the worktree on another branch,
    /// is refused there and free in the scratch one; each that writes the
    /// checkout only is free in its worktree and refused in `main`, however
    /// `-C`, `cd`, `--git-dir`, `--work-tree` or GIT_DIR take it there; what
    /// reads is free anywhere; and the orchestrator is refused none, a push
    /// aside. So is a file's write: refused in `main`, its `.git` included.
    #[test]
    fn a_born_session_writes_only_its_own_worktree_and_branch() {
        let main = repository_at("main");
        let worktree = main.join(".claude/worktrees/task-1");
        let add = ["worktree", "add", "-q", "-b", "task-1"];
        assert!(Command::new("git").current_dir(&main).args(add).arg(&worktree).status().unwrap().success());
        let scratch = repository_at("scratch");
        let own = Own { worktree: worktree.clone(), branch: "task-1".into(), git_env: Vec::new() };
        let reach = |command: &str, cwd: &Path, own: Option<&Own>| reached(command, cwd, &Caller { checkout: own.cloned(), ..Caller::default() });
        let (main_dir, main_git) = (main.display().to_string(), main.join(".git").display().to_string());

        let shared = [
            "git branch other",
            "git branch -f main HEAD",
            "git branch -d main",
            "git branch -D other",
            "git branch -m main old",
            "git branch -M x",
            "git branch --copy main c",
            "git branch -df x",
            "git checkout -b other",
            "git checkout -B main",
            "git checkout --orphan o",
            "git checkout main",
            "git checkout --detach",
            "git checkout -",
            "git switch main",
            "git switch -c other",
            "git switch --create=other",
            "git switch -d HEAD~1",
            "git switch --orphan o",
            "git update-ref refs/heads/main HEAD",
            "git update-ref -d refs/heads/main",
            "git update-ref --stdin",
            "git update-ref --no-deref HEAD HEAD",
            "git update-ref refs/tags/v1 HEAD",
            "git symbolic-ref HEAD refs/heads/main",
            "git symbolic-ref -d HEAD",
            "git tag v1",
            "git tag -a v1 -m 'a tag'",
            "git tag -d v1",
            "git tag -f v1 HEAD",
            "git rebase main other",
            "git rebase --update-refs main",
            "git rebase --root other",
            "git rebase --onto main x other",
            "git fetch origin main:main",
            "git fetch origin +refs/heads/*:refs/heads/*",
            "git pull origin main:main",
            "git notes add -m x",
            "git notes --ref r remove HEAD",
            "git replace HEAD HEAD~1",
            "git replace -d HEAD",
            "git worktree add ../x",
            "git worktree remove ../x",
            "git worktree prune",
            "git remote add up /x",
            "git remote set-url origin /x",
            "git remote remove origin",
            "git reflog expire --all",
            "git reflog delete HEAD@{1}",
            "git config user.name x",
            "git config --unset core.x",
            "git config set core.x y",
            "git config --add a.b c",
            "git filter-branch --all",
        ];
        for command in shared {
            assert_eq!(reach(command, &worktree, Some(&own)), [Limit::Shared], "{command}, in its worktree");
            assert_eq!(reach(command, &scratch, Some(&own)), [], "{command}, in another repository");
            assert_eq!(reach(command, &worktree, None), [], "the orchestrator's {command}");
        }
        // The stash, refused with what to do instead (task 1833).
        for command in ["git stash", "git stash push -u -m x", "git stash pop", "git stash drop", "git stash clear", "git stash store x", "git stash branch b"] {
            assert_eq!(reach(command, &worktree, Some(&own)), [Limit::Stash], "{command}, in its worktree");
            assert_eq!(reach(command, &main, Some(&own)), [Limit::Stash], "{command}, in main");
            assert_eq!(reach(command, &scratch, Some(&own)), [], "{command}, in another repository");
            assert_eq!(reach(command, &worktree, None), [], "the orchestrator's {command}");
        }
        let instead = reason(Limit::Stash);
        assert!(instead.contains("make it a WIP commit on its own branch -- git add -A && git commit -m wip --"), "{instead}");
        for set_aside in ["git add -A && git commit -m wip", "git reset HEAD~1"] {
            assert_eq!(reach(set_aside, &worktree, Some(&own)), [], "{set_aside}, which the refusal names");
        }

        let checkout = [
            "git commit -am x",
            "git merge task-2",
            "git reset --hard",
            "git add .",
            "git cherry-pick HEAD",
            "git checkout -- file",
            "git checkout task-1",
            "git checkout .",
            "git switch task-1",
            "git branch -u origin/main",
            "git update-ref HEAD HEAD",
            "git update-ref refs/heads/task-1 HEAD",
            "git update-ref ORIG_HEAD HEAD",
            "git symbolic-ref HEAD refs/heads/task-1",
            "git rebase main",
            "git rebase main task-1",
            "git pull",
            "git stash apply",
            "git config --worktree a.b c",
        ];
        for command in checkout {
            assert_eq!(reach(command, &worktree, Some(&own)), [], "{command}, in its worktree");
            let elsewhere = command.replacen("git ", &format!("git -C {main_dir} "), 1);
            assert_eq!(reach(&elsewhere, &worktree, Some(&own)), [Limit::Checkout], "{elsewhere}");
            assert_eq!(reach(&format!("cd {main_dir} && {command}"), &worktree, Some(&own)), [Limit::Checkout], "cd, then {command}");
            assert_eq!(reach(command, &main, Some(&own)), [Limit::Checkout], "{command}, in main");
            assert_eq!(reach(command, &scratch, Some(&own)), [], "{command}, in another repository");
            assert_eq!(reach(&elsewhere, &worktree, None), [], "the orchestrator's {elsewhere}");
        }
        let moved = [
            format!("git --git-dir={main_git} --work-tree={main_dir} commit -am x"),
            format!("git --git-dir {main_git} --work-tree {main_dir} commit -am x"),
            format!("GIT_DIR={main_git} GIT_WORK_TREE={main_dir} git commit -am x"),
            format!("export GIT_DIR={main_git} GIT_WORK_TREE={main_dir}; git commit -am x"),
            format!("git -C {main_dir}/.git commit -am x"),
        ];
        for command in &moved {
            assert_eq!(reach(command, &worktree, Some(&own)), [Limit::Checkout], "{command}");
            assert_eq!(reach(command, &worktree, None), [], "the orchestrator's {command}");
        }

        let reads = [
            "git status",
            "git log --oneline",
            "git diff main...task-1",
            "git branch",
            "git branch --list",
            "git branch --show-current",
            "git tag",
            "git tag -l",
            "git tag --contains HEAD",
            "git stash list",
            "git config user.name",
            "git config --get user.name",
            "git config get user.name",
            "git config --global a.b c",
            "git remote -v",
            "git worktree list",
            "git notes",
            "git fetch",
            "git fetch origin main",
            "git symbolic-ref HEAD",
            "git reflog",
            "git replace -l",
            "git clone /x y",
            "git init y",
        ];
        for command in reads {
            assert_eq!(reach(&format!("cd {main_dir} && {command}"), &worktree, Some(&own)), [], "{command}, in main");
        }
        for (born, orchestrator) in [(Some(&own), "its"), (None, "the orchestrator's")] {
            assert_eq!(reach("git push origin task-1", &worktree, born), [Limit::Push], "{orchestrator} push");
        }

        for (path, refused) in [
            (worktree.join("src/new.rs"), None),
            (worktree.join("readme.md"), None),
            (main.join("src/agents.rs"), Some(Limit::Checkout)),
            (main.join("readme.md"), Some(Limit::Checkout)),
            (main.join(".git/refs/heads/main"), Some(Limit::Checkout)),
            (scratch.join("readme.md"), None),
            (PathBuf::from("/nowhere/at/all"), None),
        ] {
            assert_eq!(file_limit(path.to_str().unwrap(), &worktree, &own), refused, "{}", path.display());
        }
        assert_eq!(file_limit("../../../readme.md", &worktree, &own), Some(Limit::Checkout), "a path relative to its worktree");
        let _ = std::fs::remove_dir_all(&main);
        let _ = std::fs::remove_dir_all(&scratch);
    }

    /// A born session's Bash call that writes a file without git (task
    /// 1925): each program of `WRITERS` and each redirection, writing into
    /// the project's own checkout from its worktree -- by a path, a `cd`, a
    /// variable, a `bash -c` -- is refused as an Edit there is, and let
    /// through into its worktree, into a folder no repository holds, and for
    /// the orchestrator. A git alias is read as the command it stands for,
    /// given with `-c` or in the repository's config.
    #[test]
    fn a_born_session_s_shell_writes_into_another_checkout_are_refused_as_its_edits_are() {
        let main = repository_at("shell-main");
        let worktree = main.join(".claude/worktrees/task-1");
        let add = ["worktree", "add", "-q", "-b", "task-1"];
        assert!(Command::new("git").current_dir(&main).args(add).arg(&worktree).status().unwrap().success());
        let loose = crate::paths::test_dir("ekko-agents-loose");
        std::fs::create_dir_all(loose.join("d")).unwrap();
        for dir in [&main, &worktree, &loose] {
            std::fs::create_dir_all(dir.join("src")).unwrap();
        }
        let own = Own { worktree: worktree.clone(), branch: "task-1".into(), git_env: Vec::new() };
        let reach = |command: &str, own: Option<&Own>| reached(command, &worktree, &Caller { checkout: own.cloned(), ..Caller::default() });
        // Each program, and the forms it writes by.
        let by_program: &[(&str, &[&str])] = &[
            ("sed", &["sed -i s/a/b/ {}/src/x.rs", "sed -Ei.bak 's/a/b/' {}/x", "sed --in-place -e s/a/b/ -e s/c/d/ {}/x", "cd {} && sed -i s/a/b/ src/x.rs"]),
            ("dd", &["dd if=/dev/zero of={}/x bs=1 count=1"]),
            ("cp", &["cp x {}/", "cp -r src {}/src", "cp -t {} x", "cp --target-directory={} x y"]),
            ("install", &["install -m 644 x {}/x", "install -d {}/d"]),
            ("ln", &["ln -sf x {}/l", "cd {} && ln -s /x"]),
            ("mv", &["mv x {}/x", "mv {}/readme.md x", "mv -t {} x"]),
            ("rm", &["rm {}/readme.md", "rm -f {}/readme.md", "rm -rf {}/src", "rm -d {}/d"]),
            ("rmdir", &["rmdir {}/d"]),
            ("unlink", &["unlink {}/x"]),
            ("shred", &["shred -u -n 3 {}/x"]),
            ("touch", &["touch {}/x", "touch -d yesterday {}/x"]),
            ("mkdir", &["mkdir -p {}/d/e", "mkdir -m 700 {}/d"]),
            ("truncate", &["truncate -s 0 {}/x"]),
            ("tee", &["echo y | tee {}/readme.md", "tee -a {}/a < x"]),
        ];
        let names: Vec<&str> = by_program.iter().map(|(name, _)| *name).collect();
        assert_eq!(names, WRITERS.iter().map(|(name, _)| *name).collect::<Vec<_>>(), "a case for each program");
        let redirected = [
            "echo y > {}/readme.md",
            "echo y >> {}/readme.md",
            "cargo test 2> {}/log",
            "cargo test &> {}/log",
            "git diff > {}/x.patch",
            "cat > {}/x <<'EOF'\nbody\nEOF",
            "cd {} && echo y > readme.md",
            "M={}; echo y > $M/readme.md",
            "bash -c 'echo y > {}/x'",
            "nix develop -c sh -c 'touch {}/x'",
        ];
        let commands = by_program.iter().flat_map(|(_, forms)| forms.iter()).chain(&redirected);
        for template in commands {
            let at = |dir: &Path| template.replace("{}", &dir.display().to_string());
            assert_eq!(reach(&at(&main), Some(&own)), [Limit::Checkout], "{}", at(&main));
            assert_eq!(reach(&at(&main), None), [], "the orchestrator's {}", at(&main));
            assert_eq!(reach(&at(&worktree), Some(&own)), [], "{}", at(&worktree));
            assert_eq!(reach(&at(&loose), Some(&own)), [], "{}", at(&loose));
        }
        for command in ["sed s/a/b/ {}/x", "sed -n p {}/x", "cat {}/readme.md", "cp {}/readme.md x", "echo y 2>&1 >/dev/null", "grep -r x {} > /dev/null", "ls {} >&2"] {
            let command = command.replace("{}", &main.display().to_string());
            assert_eq!(reach(&command, Some(&own)), [], "{command}, which writes nothing there");
        }

        // A git alias, read as what it stands for.
        assert!(Command::new("git").current_dir(&main).args(["config", "alias.pp", "push"]).status().unwrap().success());
        for (command, limit) in [
            ("git -c alias.p=push p origin task-1", vec![Limit::Push]),
            ("git -c alias.p='!git push origin task-1' p", vec![Limit::Push]),
            ("git -c alias.b='branch -f main HEAD' b", vec![Limit::Shared]),
            ("git -c alias.s='stash push' s", vec![Limit::Stash]),
            ("git -c alias.c=commit -C ../../.. c -am x", vec![Limit::Checkout]),
            ("git pp origin task-1", vec![Limit::Push]),
            ("git -c alias.st=status st", vec![]),
            ("git -c alias.c=commit c -am x", vec![]),
        ] {
            assert_eq!(reach(command, Some(&own)), limit, "{command}");
        }
        let _ = std::fs::remove_dir_all(&main);
        let _ = std::fs::remove_dir_all(&loose);
    }
}
