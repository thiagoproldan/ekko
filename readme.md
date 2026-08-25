<h1 align="center">
  Ekko
</h1>

<h4 align="center">
  Tasks, boards & notes for the command-line habitat
</h4>

<div align="center">
  <img alt="Boards" width="70%" src="media/header-boards.png"/>
</div>

## Description

Ekko is a pure-Rust rewrite of [taskbook](https://github.com/klaudiosinani/taskbook), keeping its terminal look and simple, minimal usage syntax byte-for-byte and rebuilding everything underneath: machine-readable `--json` output, a real cross-process lock so two invocations writing at once can't silently clobber each other, and a `node:test`\-era test suite carried over as `cargo test`. Local and private by design -- data never leaves your machine, and now nothing about running it depends on Node being installed either.

Effectively a task manager built to be driven by a human and an LLM/coding agent working the same boards at the same time, which is exactly the property the rewrite exists to make solid.

## Highlights

- Organize tasks & notes into boards
- Board & timeline views
- Priority & favorite mechanisms
- Due dates via `d:YYYY-MM-DD`, coloured by urgency and filterable -- an Ekko addition
- A real paused state, so "set aside" stops looking like "never started"
- Long notes fold to one line on screen, and stay whole in pipes and `--json`
- Retry-safe state changes (`--set`), stable per-item `uid`s, and incremental reads (`--since`) -- built for scripts and agents
- Search & filter items
- Archive & restore deleted items
- Machine-readable `--json` output for every command, scripts and agents included
- Cross-process file lock: concurrent writers queue instead of losing each other's updates
- Data written atomically to storage (temp file + rename)
- Custom storage location, per-project or per-context
- Progress overview
- Configurable through `~/.ekko.json`
- Data stored in plain JSON at `~/.ekko/storage`
- A reproducible `nix develop` shell for the whole toolchain

<div align="center">
  <img alt="Highlights" width="66%" src="media/highlights.png"/>
</div>

## Contents

- [Description](#description)
- [Highlights](#highlights)
- [Install](#install)
- [Usage](#usage)
- [Views](#views)
- [Configuration](#configuration)
- [Flight Manual](#flight-manual)
- [Development](#development)
- [Credits](#credits)
- [License](#license)

## Install

Not published anywhere -- build it from this repository.

```bash
$ git clone <this repo> && cd ekko
$ nix develop           # Rust toolchain: cargo, rustc, clippy, rustfmt
$ cargo install --path . --locked
```

That installs the `ekko` binary to `~/.cargo/bin` (make sure it's on your `PATH`). To just build it without installing: `cargo build --release`, binary lands at `target/release/ekko`.

## Usage

```
$ ekko --help

  Usage
    $ ekko [<options> ...]

    Options
        none             Display board view
      --archive, -a      Display archived items
      --begin, -b        Start/pause task
      --check, -c        Check/uncheck task
      --clear            Delete all checked items
      --copy, -y         Copy item description
      --delete, -d       Delete item
      --edit, -e         Edit item description
      --find, -f         Search for items
      --help, -h         Display help message
      --json, -j         Output machine-readable JSON instead of formatted text
      --list, -l         List items by attributes
      --move, -m         Move item between boards
      --note, -n         Create note
      --priority, -p     Update priority of task
      --restore, -r      Restore items from archive
      --set              Set item state idempotently (retry-safe)
      --since <MILLIS>   Only items changed at or after a timestamp
      --star, -s         Star/unstar item
      --ekko-dir         Define a custom ekko directory
      --task, -t         Create task
      --timeline, -i     Display timeline view
      --version, -v      Display installed version

    Examples
      $ ekko
      $ ekko --archive
      $ ekko --begin 2 3
      $ ekko --check 1 2
      $ ekko --clear
      $ ekko --copy 1 2 3
      $ ekko --delete 4
      $ ekko --edit @3 Merge PR #42
      $ ekko --find documentation
      $ ekko --json --task @coding Review PR #42
      $ ekko --list pending coding
      $ ekko --move @1 cooking
      $ ekko --note @coding Mergesort worse-case O(nlogn)
      $ ekko --priority @3 2
      $ ekko --restore 4
      $ ekko --star 2
      $ ekko --task @coding @reviews Review PR #42
      $ ekko --task @coding Improve documentation
      $ ekko --task Make some buttercream
      $ ekko --timeline
```

## Views

### Board View

Invoking Ekko without any options will display all saved items grouped into their respective boards.

<div align="center">
  <img alt="Boards" width="60%" src="media/header-boards.png"/>
</div>

### Timeline View

In order to display all items in a timeline view, based on their creation date, the `--timeline`/`-i` option can be used.

<div align="center">
  <img alt="Timeline View" width="62%" src="media/timeline.png"/>
</div>

## Configuration

To configure Ekko navigate to the `~/.ekko.json` file and modify any of the options to match your own preference. To reset back to the default values, simply delete the config file from your home directory.

The following illustrates all the available options with their respective default values.

```json
{
  "ekkoDirectory": "~",
  "displayCompleteTasks": true,
  "displayProgressOverview": true
}
```

### In Detail

##### `ekkoDirectory`

- Type: `String`
- Default: `~`

Filesystem path where the storage will be initialized, i.e: `/home/username/the-cloud` or `~/the-cloud`

If left undefined the home directory `~` will be used and Ekko will be set up under `~/.ekko/`.

##### `displayCompleteTasks`

- Type: `Boolean`
- Default: `true`

Display tasks that are marked as complete.

##### `displayProgressOverview`

- Type: `Boolean`
- Default: `true`

Display progress overview below the timeline and board views.

## Flight Manual

The following is a minor walkthrough containing a set of examples on how to use Ekko.

### Create Task

To create a new task use the `--task`/`-t` option with your task's description following right after.

```
$ ekko -t Improve documentation
```

### Create Note

To create a new note use the `--note`/`-n` option with your note's body following right after.

```
$ ekko -n Mergesort worse-case O(nlogn)
```

### Create Board

Boards are automatically initialized when creating a new task or note. To create one or more boards, include their names, prefixed by the `@` symbol, in the description of the about-to-be created item. As a result the newly created item will belong to all of the given boards. By default, items that do not contain any board names in their description are automatically added to the general purpose; `My Board`.

```
$ ekko -t @coding @docs Update contributing guidelines
```

### Check Task

To mark a task as complete/incomplete, use the `--check`/`-c` option followed by the ids of the target tasks. Note that the option will update to its opposite the `complete` status of the given tasks, thus checking a complete task will render it as pending and a pending task as complete. Duplicate ids are automatically filtered out.

```
$ ekko -c 1 3
```

### Begin Task

To mark a task as started/paused, use the `--begin`/`-b` option followed by the ids of the target tasks. The functionality of this option is the same as the one of the above described `--check` option.

```
$ ekko -b 2 3
```

### Star Item

To mark one or more items as favorite, use the `--star`/`-s` option followed by the ids of the target items. The functionality of this option is the same as the one of the above described `--check` option.

```
$ ekko -s 1 2 3
```

### Copy Item Description

To copy to your system's clipboard the description of one or more items, use the `--copy`/`-y` option followed by the ids of the target items. Note that the option will also include the newline character as a separator to each pair of adjacent copied descriptions, thus resulting in a clear and readable stack of sentences on paste.

```
$ ekko -y 1 2 3
```

On Linux, copying spawns a short-lived detached process to keep serving the clipboard after `ekko` itself exits (X11/Wayland make the copying application responsible for answering paste requests; a process that exits immediately can't). It goes away once something else claims the clipboard.

### Display Boards

Invoking Ekko without any options will display all of saved items grouped into their respective boards.

```
$ ekko
```

### Display Timeline

In order to display all items in a timeline view, based on their creation date, the `--timeline`/`-i` option can be used.

```
$ ekko -i
```

### Set Priority

To set a priority level for a task while initializing it, include the `p:x` syntax in the task's description, where x can be an integer of value `1`, `2` or `3`. Note that all tasks by default are created with a normal priority - `1`.

- `1` - Normal priority
- `2` - Medium priority
- `3` - High priority

```
$ ekko -t @coding Fix issue `#42` p:3
```

To update the priority level of a specific task after its creation, use the `--priority`/`-p` option along with the id the target task, prefixed by the `@` symbol, and an integer of value `1`, `2` or `3`. Note that the order in which the target id and priority level are placed is not significant.

```
$ ekko -p @1 2
```

### Due Dates

To give a task a deadline while creating it, include a `d:YYYY-MM-DD` token in the description, alongside `p:x` if you want both. The token is stripped from the description, and a date that does not parse is an error (`INVALID_DUE_DATE`) rather than a task quietly created without one.

```
 -t @coding Ship the release notes d:2026-09-01 p:2
```

Due dates show up next to the item, coloured by where they stand: red once past, yellow on the day itself, grey while still ahead, and grey again once the task is checked off -- a finished task is not late. Notes take no deadline, the same way they take no priority.

Filter with `--list due` for everything carrying a deadline, or `--list overdue` for open tasks whose date has passed. Both compose with board names, so `ekko -l overdue coding` narrows to one board.

This is an Ekko addition; taskbook has no equivalent. Items without a due date are unaffected, on screen and in `storage.json` alike -- the field is omitted entirely when unset, so files stay readable by taskbook.





### Folded Notes

Notes hold the reasoning worth keeping, which is why they run long. On a real board they took 56 of 94 rendered lines while every task, open and closed, took 28 -- so a board becomes unscannable through its notes, not its tick marks.

When stdout is a terminal, a note too long for one line is shortened and told on itself:

```
   28. ●  Timeline idea, with the constraints that survived scrutiny. (1) A node is … (+12 lines)
```

The count is useful on its own: you can see which notes are dense before deciding to open one.

Three deliberate limits:

- **Only when stdout is a terminal.** `ekko | grep` and `ekko > file` get every note in full, so nothing that reads Ekko's output breaks. Folding asks the terminal for its width directly rather than reusing the colour decision, because `FORCE_COLOR=1 ekko > file` must still write everything.
- **Only notes.** A truncated task hides something you are meant to act on; a truncated note hides something you can go and read.
- **Only when it helps.** Below about two dozen usable columns the marker would eat most of the line, so the note is left whole for the terminal to wrap.

To read a folded note in full, pipe the output (`ekko | less`) or use `--json`, which never folds.

### Pausing

`--check`, `--begin` and `--star` toggle; setting a task back out of progress with any of them leaves it looking exactly like a task that was never started. Those are different situations, and conflating them is how a board comes to report `0 pending` while two tasks sit half-done.

A paused task keeps its own state and its own icon:

```
    1. ☐  never started
    2. …  in progress
    3. ⏸  paused
    4. ✔  done
```

`ekko --set @3 paused` sets a task aside; `--set @3 progress` resumes it; `--set @3 unstarted` clears both flags and returns it to never-started, which is also how to undo a `--set progress` aimed at the wrong id. Finishing a task settles it either way.

Nothing is paused automatically. Ekko instead points out when more than one task is in progress, since that is the state where the marker stops telling you where you are:

```
2 tasks in progress -- pause the ones you are not on: ekko --set @id paused
```

Both additions are conditional: the `paused` count joins the stats line only when it is above zero, and the warning only appears when it applies. A board that keeps to one task at a time prints exactly what it printed before.

This is an Ekko addition, though the concept is not: taskbook's own help calls `--begin` "Start/pause task". It named pausing without giving it anywhere to live.

### Incremental Reads

Reading the whole board to find out what changed gets expensive fast, and for a script or an agent syncing on every step it is nearly all waste. `--since` takes an epoch-millisecond timestamp and returns only items changed at or after it, grouped by board exactly like the default view.

```
$ ekko --json --since 1787600000000
```

Every item carries `updatedAt`, stamped whenever it actually changes -- distinct from `_timestamp`, which is creation time and never moves. The sync loop is therefore: read with `--since <last>`, do the work, and remember the highest `updatedAt` you saw as the next `<last>`.

On a forty-item board, syncing one changed item this way costs around 330 bytes against roughly 11.5 KB for the full board.

Two limits worth knowing. A write that changes nothing does not bump `updatedAt`, so an idempotent `--set` that was already satisfied will not resurface. And deletions leave nothing behind to carry a timestamp: a caller that must notice removals has to compare id sets, not just read `--since`. Items predating the field fall back to their creation time rather than disappearing, so `--since 0` still returns everything.

### Setting State Idempotently

`--check`, `--begin` and `--star` all **toggle**, which is right at a terminal and wrong for anything that might retry: run `ekko -c 3` twice after a timed-out first attempt and the task ends up unchecked again.

`--set` takes the states an item should end up *in*, so running it twice does the same thing as running it once. Ids are marked with `@`, exactly as in `--priority` and `--move`, which leaves bare words free to name states.

```
$ ekko --set @3 done
$ ekko --set @1 @2 progress starred
```

Accepted states, with their aliases: `done`/`checked`/`complete`, `undone`/`unchecked`/`incomplete`/`pending`, `progress`/`started`/`begun`, `paused`, `unstarted`/`unstart`, `starred`/`star`, `unstarred`/`unstar`. They are the same words `--list` filters on, so there is one vocabulary rather than two. An unrecognised state is an error (`UNKNOWN_STATE`), not a silent no-op.

Task-only states are ignored on notes, matching how `--check` already ignores them; starring applies to both.

This is an Ekko addition. The toggles are unchanged and remain the shorter thing to type by hand.

### Move Item

To move an item to one or more boards, use the `--move`/`-m` option, followed by the target item id, prefixed by the `@` symbol, and the name of the destination boards. The default `My board` can be accessed through the `myboard` keyword. The order in which the target id and board names are placed is not significant. Note that this **replaces** the item's board list; it does not add to it -- list every board you want the item to keep, not just the new one.

```
$ ekko -m @1 myboard reviews
```

### Delete Item

To delete one or more items, use the `--delete`/`-d` options followed by the ids of the target items. Note that deleted items are automatically archived, and can be inspected or restored at any moment. Duplicate ids are automatically filtered out.

```
$ ekko -d 1 2
```

### Delete Checked Tasks

To delete/clear all complete tasks at once across all boards, use the `--clear` option. Note that all deleted tasks are automatically archived, and can be inspected or restored at any moment. In order to discourage any possible accidental usage, the `--clear` option has no available shorter alias.

```
$ ekko --clear
```

### Display Archive

To display all archived items, use the `--archive`/`-a` option. Note that all archived items are displayed in timeline view, based on their creation date.

```
$ ekko -a
```

### Restore Items

To restore one or more items, use the `--restore`/`-r` option followed by the ids of the target items. Note that the ids of all archived items can be seen when invoking the `--archive`/`-a` option, **and are a separate id space from storage** -- a restored item gets a new id in `storage.json`, it does not get its old one back. `--delete`'s `--json` response reports both, precisely so a script/agent driving `--restore` afterward never has to guess.

```
$ ekko -r 1 2
```

### List Items

To list a group of items where each item complies with a specific set of attributes, use the `--list`/`-l` option followed by the desired attributes. Board names along with item traits can be considered valid listing attributes. For example to list all items that belong to the default `myboard` and are pending tasks, the following could be used;

```
$ ekko -l myboard pending
```

The by default supported listing attributes, together with their respective aliases, are the following;

- `myboard` - Items that belong to `My board`
- `task`, `tasks`, `todo` - Items that are tasks.
- `note`, `notes` - Items that are notes.
- `pending`, `unchecked`, `incomplete` - Items that are pending tasks (note: an in-progress task is not yet complete either, so it matches this too).
- `progress`, `started`, `begun` - Items that are in-progress tasks.
- `done`, `checked`, `complete` - Items that complete tasks.
- `star`, `starred` - Items that are starred.
- `due` - Tasks that have a due date.
- `overdue` - Tasks whose due date has passed and that are not yet complete.

A board can be named either bare or in the `@name` form the board view prints, so `--list release` and `--list @release` are equivalent. A term matching neither a board nor an attribute above is an error (`UNKNOWN_LIST_TERM`), not a silent no-op.

Both are deliberate departures from taskbook, which accepted the bare form only and listed *every* board when a term matched nothing -- indistinguishable, from the output alone, from a filter that legitimately matched everything. That is a bad answer for a person and a worse one for a script or an agent, which cannot tell the two apart at all.

### Search Items

To search for one of more items, use the `--find`/`-f` option, followed by your search terms.

```
$ ekko -f documentation
```

### Runtime ekko directory override

To override the configured storage location, use the `--ekko-dir` flag or `EKKO_DIR` environment variable. While Ekko is designed to provide a multiple board approach for all of your projects, these options enable alternative use cases. Setup per-project storage or run multiple global boards like home and work.

Note, if both the flag and environment variable are present Ekko will use the flag value.

```
$ ekko --ekko-dir .custom-ekko-dir
```

```
$ EKKO_DIR=~/hometasks ekko
```

### Locating the data files

All task/note data lives in one JSON file, written atomically (temp file + rename) so it is always safe to read directly, without going through the CLI: `<ekko-dir>/storage/storage.json`. Deleted/archived items live alongside it at `<ekko-dir>/archive/archive.json`. `<ekko-dir>` defaults to `~/.ekko` and follows the same resolution order as the `--ekko-dir` flag described above.

```
$ cat ~/.ekko/storage/storage.json
```

Every command that writes (`--task`, `--check`, `--delete`, ...) takes a lock at `<ekko-dir>/.lock` for its duration, so two `ekko` processes -- two terminals, two scripts, two agents -- touching the same directory at once queue up instead of silently clobbering each other's write. A second process waits up to 5 seconds for the lock to free up; if the process holding it has actually died, the stale lock is detected and cleared automatically, no waiting required. You should never need to touch this file by hand, but if `ekko` ever reports a timeout with no other ekko process actually running, it's safe to delete it.

### Machine-readable output

Add the `--json`/`-j` flag to any command to get a single-line JSON object on stdout instead of formatted text -- meant for scripts and agents that need to parse the result, rather than scrape colored terminal output. It composes with every other flag.

```
$ ekko --json --task @coding Review PR #42
{"ok":true,"command":"task","item":{"_id":7,"_date":"Mon Aug 24 2026","_timestamp":1787532527693,"description":"Review PR #42","isStarred":false,"boards":["@coding"],"_isTask":true,"isComplete":false,"inProgress":false,"priority":1}}
```

On success, the object always has `ok: true` and a `command` field naming what ran, plus whatever data that command produces (a `create`d/`edit`ed/`move`d/`priority`-updated item's full record, id lists for `check`/`begin`/`star`, board- or date-grouped items for the view commands, etc). On failure it's `ok: false` with an `error` message and a stable `code` (`MISSING_ID`, `INVALID_ID`, `MISSING_DESC`, `INVALID_IDS_NUMBER`, `INVALID_PRIORITY`, `MISSING_BOARDS`, `UNKNOWN_LIST_TERM`, `INVALID_DUE_DATE`, `MISSING_STATE`, `UNKNOWN_STATE`, `INVALID_CUSTOM_APP_DIR`, `MISSING_EKKO_DIR_FLAG_VALUE`, `LOCK_TIMEOUT`) to branch on instead of matching on the message text -- the process also exits `1`, same as without `--json`.

A couple of things worth knowing:

- **`--json` always returns complete data.** The `displayCompleteTasks`/`displayProgressOverview` preferences in `~/.ekko.json` only affect the human-readable views; JSON output never hides anything.

Every item created by Ekko also carries a `uid`: a stable identifier that, unlike `_id`, is never recycled and survives `--restore`. Ids are assigned as `max + 1`, so deleting the highest-numbered item and creating another hands the new one the same number -- which makes `_id` a poor thing to hold on to across time. Scripts and agents that keep a reference between invocations should key on `uid`.

Items written before uids existed, and any written by taskbook, have no `uid` field. Ekko does not backfill one, because that would rewrite files it otherwise leaves untouched; an absent `uid` means "legacy", not "unknown".

- **Output is [newline-delimited JSON](https://github.com/ndjson/ndjson-spec), not always a single object.** A few commands (the default board view, `--timeline`, `--list`) print a data line followed by a separate `{"command":"stats",...}` line. Parse stdout line by line, not as one JSON document.
- If you drive `ekko` through `nix develop --command`, the devShell's own banner goes to stderr, not stdout, specifically so it never lands in a `--json` response -- safe to invoke that way from a script.

### A literal hyphen at the start of a value

If a description or other value genuinely needs to start with `-`, separate it from the flags with `--`, the same convention most CLI tools use for this:

```
$ ekko --task -- --json in the description, not the flag
```

Without the `--`, a word that happens to match a real flag name (`--json`, `--task`, ...) gets parsed as that flag instead of kept as literal text; a word that doesn't match anything real errors clearly rather than being silently dropped.

## Development

- Fork the repository and clone it to your machine
- Navigate to your local fork: `cd ekko`
- Run `nix develop` for a shell with the full Rust toolchain (cargo, rustc, clippy, rustfmt, rust-analyzer) already set up
- Run the full check -- lint plus the test suite: `cargo clippy --all-targets && cargo test`
- `cargo test` includes integration tests in `tests/` that spawn the real compiled binary (including real concurrent processes, to actually exercise the storage lock) -- not just unit tests

## Credits

Ekko is a Rust rewrite of [taskbook](https://github.com/klaudiosinani/taskbook) by Klaudio Sinani and Mario Sinani. The terminal output -- icons, colors, layout -- is deliberately unchanged; that's what made the original worth rebuilding rather than replacing. See [license.md](license.md) for the original MIT copyright, preserved as required for a derivative work.

Versioning continues taskbook's rather than restarting: the `v0.1.0`-`v0.4.0` tags are the original project's, inherited along with its history and kept deliberately -- `tests/golden/` documents regenerating Ekko's reference output from the `v0.4.0` tree. Ekko's first release is therefore `v0.5.0`, a minor bump and not a patch, because the rename is a breaking change for anyone arriving from taskbook: the binary, the data directory (`~/.ekko`), the config file (`~/.ekko.json`) and the environment variable all changed name.

## License

[MIT](license.md)
