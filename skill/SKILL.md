---
name: ekko
description: Track work on a shared ekko board (tasks, notes, boards, due dates, dependencies, projects and phases, archive) that both the user and Claude read and write. Use when work spans sessions, when the user wants to see or edit the task list themselves, or when they ask to put something "on the board". Not a replacement for TodoWrite, which stays the right tool for a plan that only lives inside one conversation.
---

# Ekko as a shared board

Ekko is a CLI task board. Its state lives on disk, not in the conversation, so
both you and the user drive the same board — they may add, edit, reorder or
delete items between two of your commands, from their own terminal.

That is the whole point, and it is also the main hazard. **Do not treat the
board as yours.**

## When to reach for it

Use ekko when the work outlives the conversation, when the user should be able
to see or edit the list themselves, or when they ask for something to go "on
the board".

Keep using TodoWrite for a plan that only matters inside the current
conversation. The two are not in competition: TodoWrite is scratch, ekko is
durable and shared. Do not migrate one to the other unasked.

## Being invoked as `/ekko`

`/ekko` with no arguments means "show me the board". Read it and present the
plain text as it comes — it is already laid out for reading, and reformatting
it into a markdown table loses the icons, the colours and the counts. Add a
line of your own only when something needs pointing at: overdue items, a board
that has grown, work that looks stalled.

`/ekko <flags>` means run exactly that. The arguments are ekko flags, passed
through verbatim — do not reinterpret them, do not "improve" them, and do not
substitute a different command you think fits better. Report what came back.

A destructive command the user typed themselves is authorised; that *is* the
asking. The consent rule below is about you deciding to clear or delete, not
about carrying out an explicit instruction. Do still say what it removed, and
do check first if the blast radius is larger than the command looks — `--clear`
on a board with thirty completed items, for instance.

The user can equally just ask in prose ("how's the list looking?"). Same thing:
read, then answer the question they actually asked rather than dumping the
whole board.

## Preflight

Run `ekko --version` once per session. If it is not on PATH, try
`~/.local/bin/ekko` and `~/.cargo/bin/ekko` before giving up — a cargo-built
binary often lands somewhere the shell does not look. If it is genuinely
missing, do not silently substitute TodoWrite — say so and offer to install it
(`cargo install --path <ekko checkout>`, or `--ekko-dir`/`EKKO_DIR` if the
binary exists but the data directory does not). `--ekko-dir` requires the
directory to already exist; it refuses to create one.

## Ids are not stable identifiers

The next id is `max(existing) + 1`, so **deleting the highest-numbered item and
creating another reuses that number**. An id you read a while ago may now point
at something else entirely.

- Safe to act on blind: an id you created *in this same turn*.
- Not safe: any id from an earlier turn, or one the user mentioned in passing.

**Carry the `uid`, and act on it.** `--json` gives every item one: never
recycled, unchanged by `--restore`, and accepted **anywhere a display id is**
— `--set`, `--edit`, `--move`, `--priority`, `--delete`, `--blocked-by`,
`--restore` and the toggles all take either.

    ekko --set @18cfa4987d5ce3-1043bc done      # `@` marks the id, as always
    ekko --star 18cfa4987d5ce3-1043bc           # toggles take it bare

The two can never be confused: a uid always carries a hyphen and never parses
as a number. An unknown one fails as `INVALID_ID`, the same as an unknown id.

So the re-read before acting is only needed when a **display** id is all you
have — one the user typed, or one you read a while ago. Carrying uids from the
start removes that cost entirely, and it is the one reference that survives an
item being archived and restored.

## Read sparingly, but read when it matters

A full board read costs roughly ten times what a mutation does, so do not
re-read after every change — you already know what you just did.

Do re-read when the answer actually depends on current state: before acting on
older ids, before reporting board status to the user, and after any pause where
the user may have been working. Correctness wins over the byte count every
time.

Narrow the read when you can. `ekko --list <board>` beats a full board, and
`ekko --json --since <millis>` beats both: it returns only items changed at or
after an instant. The sync loop is read with `--since <last>`, work, then keep
the highest `updatedAt` you saw as the next `<last>`. On a forty-item board
that is a few hundred bytes instead of eleven kilobytes.

Two limits: a write that changes nothing does not bump `updatedAt`, and
deletions leave nothing behind to report. If you must notice removals, compare
id sets.

## Parse `--json`, never the pretty output

Add `--json` to anything whose result you will branch on.

- View commands emit **newline-delimited JSON** — a data line, then a separate
  `{"command":"stats",...}` line. Parse line by line, not as one document.
- Errors are `{"ok":false,"error":...,"code":...}` with a stable `code`
  (`INVALID_ID`, `UNKNOWN_LIST_TERM`, `UNKNOWN_STATE`, `INVALID_DUE_DATE`,
  `ANCHOR_NOT_A_NOTE`, `ANCHOR_TARGET_NOT_A_TASK`,
  `BLOCKING_CYCLE`, `LOCK_TIMEOUT`, …). Branch on `code`; the message text is
  not an API. Exit status is `1`.
- `--restore` especially: the pretty output reports the *archive* id, while the
  item comes back with a fresh storage id. Only `--json` gives you both
  (`archiveId`, `storageId`).

## Prefer `--set` over the toggles

`--check`, `--begin` and `--star` **toggle**. If a command times out after the
write landed and you retry it, you undo yourself. Use `--set`, which takes the
state the item should end up in and is therefore safe to repeat:

    ekko --set @3 done
    ekko --set @1 @2 progress starred

States: `done`, `undone`, `progress`, `paused`, `cancelled`, `unstarted`,
`starred`, `unstarred` (plus the `--list` aliases). Ids take `@`, states do
not. Unknown states error rather than doing nothing.

Two of those are not toggles of anything and are worth knowing:

- `cancelled` is terminal, like `done`, and mutually exclusive with it —
  abandoned on purpose, struck through, kept as context rather than deleted.
  It is left out of the percentage denominator, so a board that drops
  something can still reach 100%. Prefer it to `--delete` when the user
  decides against a task: the record of having decided is usually the point.
- `unstarted` is the way back. It clears progress, pause and cancellation at
  once, which is what undoes a `--set` aimed at the wrong id.

Leave the toggles to the user — they are the shorter thing to type by hand.

## Dependencies, and the one filter to reach for

`ekko --blocked-by @3 1 2` records that item 3 waits on 1 and 2. The `@`
marks the item being blocked; the blockers are bare ids. Blocked items render
with `⇠ 1, 2` after the description.

Blockers are stored as `uid` and **evaluated live**, never latched. Reopening
a finished blocker re-blocks everything waiting on it, with no command to
run. Cycles are refused at write time (`BLOCKING_CYCLE`), so the graph cannot
be made inconsistent.

`ekko --list ready` is the filter this exists for: pending items with no
unmet blockers — what can actually be started right now. Reach for it instead
of reading the whole board and reasoning about order yourself, which is both
more expensive and easier to get wrong.

`--blocked-by @3` with no blockers **clears** them. Reach for it the moment a
dependency turns out to be wrong — a blocker you cannot undo becomes a false
statement the board then carries as if it were data.

## Projects and phases

The default board is flat: boards (areas) and items, quick capture, no
hierarchy. That is unchanged and is still the right shape for most work.

A **project** is a separate board with its own storage. `--project <name>`
scopes every command to it, `--projects` lists them with what each holds
(`plan [0/15] · 4 notes`), and `--project <name> --create` makes one. It is
sugar over `--ekko-dir` and the two are mutually exclusive — passing both
errors rather than picking one.

`--project <name> --destroy` removes one. It is **not** `--delete`, which
still means "remove items" — `--project old --delete 3` removes item 3 inside
`old`. Destroy takes the project's lock, reports what went (`15 tasks · 4
notes`) and moves the directory to `~/.ekko/.trash/<name>-<millis>` rather
than deleting it. `--restore` does not reach it; recovery is `mv`-ing that
directory back into `~/.ekko/projects/`.

It never prompts, so the consent rule below applies with full force: read
`--projects` first, put the count in front of the user, and let them say yes
before you destroy a board you did not build.

Inside a project the hierarchy is **project > phase > area**. Each phase is
its own world, so the same area name in two phases means two distinct areas.

Phases are declared, never inferred — "setup comes before build" is
knowledge, not a timestamp:

    ekko --project demo --phases setup build ship

That **replaces** the whole ordered list rather than appending, which is what
makes reordering and inserting the same operation. Same contract as `--move`.

Two things that will bite otherwise:

- `--phase <name>` scopes **creation only**. `--task` and `--note` honour it;
  the views ignore it silently, so `--project demo --phase build` prints the
  whole project, not that phase.
- A task created in a project *without* `--phase` lands at the project root,
  outside the path — never in a guessed current phase. `--path` counts those
  at its foot so they stay visible.

`ekko --project demo --path` is the phase-aware view:

    setup ●───build ◉   ───ship ○
    2/2       0/2 HERE     0/0

    1 note · 1 outside any phase


## Command shapes worth memorising

| intent | command | note |
|---|---|---|
| create task | `ekko --task @board 'text' p:2 d:2026-09-01` | `p:` and `d:` optional, stripped from the text |
| create note | `ekko --note @board 'text'` | notes take no priority or due date |
| set state | `ekko --set @3 done` | idempotent — prefer this |
| move boards | `ekko --move @3 backlog` | `@` marks the **id**; replaces the board list |
| filter | `ekko --list overdue backlog` | boards bare or `@backlog`; unknown terms error |
| changed only | `ekko --json --since 1787600000000` | epoch millis |
| history | `ekko --archive` | completed items, grouped by date |
| restore | `ekko --restore 2` | takes the **archive** id |
| what can start | `ekko --list ready` | pending, no unmet blockers |
| declare a blocker | `ekko --blocked-by @3 1 2` | `@` marks the **blocked** item |
| clear a blocker | `ekko --blocked-by @3` | no blockers means none |
| anchor a note | `ekko --anchor @16 12` | note first, then its task |
| scope to a project | `ekko --project demo --list ready` | mutually exclusive with `--ekko-dir` |
| the phase view | `ekko --project demo --path` | `--phase` itself only affects creation |
| destroy a project | `ekko --project old --destroy` | whole board to the trash; ask first |

Filter attributes: `pending`, `progress`, `done`, `star`, `task`, `note`,
`cancelled`, `ready`, `blocked`, `due`, `overdue`, `myboard`, plus any board
name. Several have aliases (`checked`/`complete`, `started`/`begun`,
`unchecked`/`incomplete`, `todo`). Boards work bare or as `@board`, but by
their `@` name — `--list 'My Board'` errors, `--list myboard` is the spelling
for the default one. Unknown terms error rather than quietly returning
everything.

## Destructive commands need consent

`--clear` archives every completed item, `--delete` removes items, and
`--destroy` takes a whole project — on a board the user also owns, including
items you never saw them create. Ask first, every time, even if the user asked
you to "clean up". Their idea of which items are finished with may not match
the board's `isComplete` flags.

`--destroy` is the one to be slowest about. It is the only command whose
blast radius is a board rather than a row, and Ekko will not stop you: it
takes the lock, reports the count and moves on. Read `--projects`, say the
number out loud, and wait.

Before proposing `--delete`, consider `--set @N cancelled` instead. It keeps
the item, struck through, and does not count against the percentage — so
"we decided not to do this" survives as a fact rather than as a hole where an
item used to be. Deleting is for things that should never have been on the
board; cancelling is for things that were, and then were not.

Concurrency itself is safe: the storage lock is `flock(2)`, so parallel
invocations queue rather than clobbering each other. It is your *judgement*
about which items to remove that needs checking, not the write.

## Leave reasoning behind, not just state

`--note` is the cheapest way to make a decision durable. When you settle
something the next session would otherwise re-derive — a design trade-off, a
dead end, a constraint discovered the hard way — put it on the board next to
the work it constrains. A note costs one short command and survives the
conversation.

**Anchor it to the task it explains.** `ekko --anchor @<note> <task>` renders
the note indented under that task instead of beside it, and `--anchor @<note>`
with no target clears it.

    ekko --note @wayland 'damage is in surface coords, not output coords'
    ekko --anchor @3 2

This matters more for you than for the person you share the board with. You
will write long notes — that is the point, the next session should not have to
re-derive them — and a column of 900-character reasons is a wall to whoever
opens the board next. Anchored, the reason sits under its work and the list
they scan is the tasks. Write the long note; just attach it.

Only a note can be anchored, and only to a task, so there is one level and no
chains.
