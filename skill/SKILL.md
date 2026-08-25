---
name: ekko
description: Track work on a shared ekko board (tasks, notes, boards, due dates, archive) that both the user and Claude read and write. Use when work spans sessions, when the user wants to see or edit the task list themselves, or when they ask to put something "on the board". Not a replacement for TodoWrite, which stays the right tool for a plan that only lives inside one conversation.
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

Before `--check`, `--delete`, `--edit`, `--move` or `--priority` on an id you
did not just create, re-read and confirm the description matches what you
expect. This costs one command and prevents silently editing the user's work.

`--json` also gives every item a `uid`: never recycled, and unchanged by
`--restore`. When you need to carry a reference across turns, carry that
rather than the display id.

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
  `LOCK_TIMEOUT`, …). Branch on `code`; the message text is not an API. Exit
  status is `1`.
- `--restore` especially: the pretty output reports the *archive* id, while the
  item comes back with a fresh storage id. Only `--json` gives you both
  (`archiveId`, `storageId`).

## Prefer `--set` over the toggles

`--check`, `--begin` and `--star` **toggle**. If a command times out after the
write landed and you retry it, you undo yourself. Use `--set`, which takes the
state the item should end up in and is therefore safe to repeat:

    ekko --set @3 done
    ekko --set @1 @2 progress starred

States: `done`, `undone`, `progress`, `paused`, `starred`, `unstarred` (plus
the `--list` aliases). Ids take `@`, states do not. Unknown states error rather
than doing nothing.

Leave the toggles to the user — they are the shorter thing to type by hand.

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

Filter attributes: `pending`, `progress`, `done`, `star`, `task`, `note`,
`due`, `overdue`, `myboard`, plus any board name.

## Destructive commands need consent

`--clear` archives every completed item and `--delete` removes items — on a
board the user also owns, including items you never saw them create. Ask first,
every time, even if the user asked you to "clean up". Their idea of which items
are finished with may not match the board's `isComplete` flags.

Concurrency itself is safe: the storage lock is `flock(2)`, so parallel
invocations queue rather than clobbering each other. It is your *judgement*
about which items to remove that needs checking, not the write.

## Leave reasoning behind, not just state

`--note` is the cheapest way to make a decision durable. When you settle
something the next session would otherwise re-derive — a design trade-off, a
dead end, a constraint discovered the hard way — put it on the board next to
the work it constrains. A note costs one short command and survives the
conversation.
