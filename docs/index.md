<!-- Written by ekko docs from the board of ekko: change the board and run it again, since edits here are overwritten. -->

# ekko -- what this project is

One page, kept by the user (task 885). A session may propose a change here;
the user approves it. The board holds the work; this page holds the whole.

## What it is

A task board driven by a person and a coding agent at the same time. It
began as a pure-Rust port of taskbook, whose terminal output it still
reproduces byte for byte (tests/golden), and grew what an agent needs:
stable uids, retry-safe state changes, dependencies, due dates, a waiting
state, typed notes, projects, incremental reads.

- The person uses the CLI (`ekko`) in a terminal.
- The agent uses `ekko --mcp`, an MCP server shipped as a Claude Code
  plugin (plugin/.claude-plugin/plugin.json) with hooks: SessionStart adds
  the board's resume view (prime), PreToolUse on Bash runs the gotcha
  guard, FileChanged on storage.json wakes a session whose wait ended.
- Data is plain JSON: a project's board in its folder's .ekko/, found the
  way git finds a repository; ~/.ekko otherwise. A flock lock and atomic
  writes let many processes share one file.
- Public: github.com/thiagoproldan/ekko, MIT. A release happens only at the
  user's word (procedure 234), then the NixOS switch (procedure 574).

On this machine the board is each project's memory: what must outlive a
session goes on it, and Claude Code's auto memory is off (decision 410).

## What it is not

- ctx (~/Projetos/ctx): keeps Claude Code's context small. Cheap-model
  reads, hooks that block big reads, the handoff to ekko and /clear, the
  5-hour window cap, the cold-return guard. ekko must not depend on ctx.
- graff (~/Projetos/graff): a planned code map for Claude Code, judged
  against graphify and codebase-memory-mcp by evals/verdict/PROTOCOL.md.
  No code yet. Not the graphify skill.

## Where things live

- src/: main.rs and cli.rs (the CLI), mcp.rs (the MCP server), agent.rs
  (prime, context, next: the board as an agent reads it), ops.rs (the
  operations), item.rs, storage.rs (the file, the lock, and each process's
  parsed board kept by file version), render.rs (taskbook's views),
  project.rs and directory.rs (which board), move_to.rs (--move-to:
  items to another board, leaving a redirect behind), holder.rs (which
  Claude Code process holds a task), wake.rs, dialog.rs and menu.rs (ask,
  answered in ekko's own menu), guard.rs and shell.rs (gotcha cues; the
  shell lexer is a port of ctx's), commits.rs (`Ekko: N` trailers),
  lexical.rs (search).
- tests/: cli.rs, mcp.rs, concurrency.rs, golden/.
- evals/ (Python): agent, claude-code, paired, recall, resume.

## How work is done here

- A commit that carries a task ends with a trailer `Ekko: N`.
- A new test must be shown able to say no (procedure 876).

## Documentation

- [Decisions](decisions.md): what was settled, and why. 69 in force, 2 replaced.
- [Gotchas](gotchas.md): traps, and how to avoid them. 62 in force, 5 replaced.
- [Procedures](procedures.md): steps that work. 18 in force.
- [History](history.md): every task, newest first, each on a page of its own. 186 done, 14 cancelled, 4 open.
- [Other notes](notes.md): notes on no task. 50.

*Written by ekko docs from the board of ekko, as of 2026-10-01. The items on the board @private, and the notes on a task there, are left out.*
