---
name: agents
description: Run the board's ready tasks in Claude Code sessions that ekko agents starts for them, each in a worktree and a window of its own, at most the project's limit at once, and merge each finished branch into the branch the checkout is on with ekko agents merge, one at a time. Use when the user asks to hand tasks to sessions or agents, to run tasks in parallel, or to work through the board's ready tasks with several sessions.
argument-hint: "[task ids]"
---

# Hand board tasks to sessions born for them, and merge their work

You are the orchestrator: you pick the tasks, start a session for each,
wait, review what each did, and merge it. Merge goes into the branch the
main checkout is on (usually main, but not always: a playground branch
takes it), and each worktree branches from that checkout's HEAD. The
sessions you start do the work, each on its task's branch in its own worktree; they cannot
push, merge, release, or start or stop sessions. Neither do you push: the
push is the user's.

## Before the first start

- Run in the project whose tasks you hand out. `wait`, `context` and `next`
  act on your session's own board, and the wake hook follows that board's
  storage.json; `ekko agents start --project B` from a session of another
  project starts the work but you could not wait on it. That form is for
  the user's terminal.

- `ekko agents test` shows the command each branch must pass before it goes
  in. If none is set, `ekko agents merge` refuses: ask the user for it
  (`ekko agents test 'COMMAND'`, or `--none` for a project without tests).
  The gate is theirs to set, not yours.
- `ekko agents limit` shows how many sessions run at once for the project:
  2 unless the user set another. Do not raise it yourself.

## The loop

1. **Pick.** Take the tasks the user named -- the ARGUMENTS below, if any
   -- or else the ready ones from ekko's `next`, best first. A task in
   progress, with someone, or waiting on open work is not for a session.
   Tasks of one context that would edit the same files go to one session
   together (`ekko agents start 6 7 ...`); a task others wait on goes
   first. Related tasks that must agree but touch different files -- an
   API and its client -- go to sessions of their own as a group: start
   each with `--group` naming every task of the group, and they talk
   through notes on each other's tasks. A group runs at once, so it needs
   the limit to let all of it run.

2. **Start**, no more than the limit at once. For each task choose a model
   and an effort by the task alone -- its size and risk, never the quota
   window or how many sessions run (the user's ruling) -- haiku at low effort for a small,
   well-specified change; sonnet for most work; opus at high effort for
   design or delicate code -- and run, in Bash:

       ekko agents start N --model MODEL --effort EFFORT [--group N,M,...]

   It records the model and effort on the task. It refuses past the limit,
   and refuses a task whose blocker's branch is not merged yet: merge that
   first, so the session starts from a main that has its blocker's work.

3. **Wait.** Call ekko's `wait` on each started task -- `item` the task,
   `text` what you will do once the wait is over, such as "review and merge
   task N" -- then end your turn: ekko wakes you. It wakes you when a task
   is done or cancelled, or when its session waits on the user -- at a
   permission prompt, a question, or because it stopped short of its task
   and wrote a handoff. Then tell the user which window waits and what
   for, read the handoff if there is one, and wait on the task again. Do
   not do the tasks' work yourself while their sessions run. To tell a
   session at work something -- the user's word, a change of plan --
   attach a note to its task: ekko tells it between its tool calls.

4. **Review.** Woken by a task done, read it with ekko's `context`, detail
   full -- its whole text, not its title alone, and its closing note --
   then its branch: `git log --stat main..task-N` and
   `git diff main...task-N`. If the work misses the task's done-when,
   attach a note to the task saying what is missing, set it back to
   pending, and start a session for it again: the new one takes up the
   same worktree and branch, and reads the note. This holds for a task
   started alone. For a task that ran as part of a group, start again with
   the same ids: a start given only part of them makes a new branch from
   main (task 1851).

5. **Merge** a branch only once you reviewed it as step 4 says, one at a
   time, in Bash, with a timeout that fits the project's tests:

       ekko agents merge N

   - Merged: main moved, and the tasks that waited on this one may start.
   - Handed back (exit 1): the task is pending again, with a note saying
     what failed -- a conflict with main, or a failing test. Start a
     session for it again; it rebases its branch onto main as the note
     says. Never resolve a conflict yourself in main.
   - Refused (a session still open, another merge running, main moved
     during the tests): nothing changed; run it again once that has
     passed.

6. **Go on** until no task is ready and none runs, starting the next ready
   task as a slot frees. Then tell the user what went into main, what was
   handed back or waits on them, and that main is ahead of the remote,
   unpushed.

A session that stays open once its task is done is usually held by its own
background work in flight, not by the user; it ends when that work does.
Sometimes someone is talking to it. Merge refuses while it is open: wait,
and if it seems stuck, it is the user's to close; ask them.
