<!-- Written by ekko docs from the board of ekko: change the board and run it again, since edits here are overwritten. -->

# Gotchas

Traps, and how to avoid them: 62 in force, newest first; the 5 that later ones replaced are at the end.

- [977](#977) A Claude Code background session keeps the KONSOLE_DBUS\_\* address of the shell it was dispatched from, but its tab…
- [974](#974) A running Claude Code session keeps the ctx hooks it started with. After a NixOS switch, only sessions started later…
- [971](#971) ctx's auto-reset still types into a draft the user leaves unchanged in the input box for 10 minutes after a turn ended.
- [969](#969) ctx's auto-reset cannot reach a session the user leaves right after a turn ends.
- [959](#959) Older notes and the project memory name \~/Projetos/&lt;name>, but on this machine projects live in /projects/&lt;name>.…
- [945](#945) Count an agent's real tool calls from transcripts with jq on tool_use blocks, deduped by id, never with grep
- [935](#935) Grep ctx's guard.jsonl with a space after the colon: it writes "ev": "x", not "ev":"x".
- [933](#933) A test that copies a commit with git cherry-pick must move the target branch first. Picked onto the commit's own parent…
- [928](#928) To see which ctx hooks a new Claude Code session will run, read \~/.claude\*/skills/ctx/.claude-plugin/plugin.json…
- [927](#927) ctx's handoff.jsonl undercounts the prompts sent again after a cold-return stop. A /handoff typed after the stop…
- [917](#917) Do not run cargo fmt --all in ekko either: rustfmt here reformats files nobody touched. Measured 2026-09-28 in the…
- [896](#896) Never run `ekko --prime --hook` by hand from inside a Claude Code session, as a timing loop or a test: it records the…
- [866](#866) In ctx's test-hooks.sh, a helper called before its definition runs as "command not found": it counts as neither a pass…
- [852](#852) Compare two ekko builds with evals/agent/scale.py back to back, never against an earlier results file.
- [851](#851) ctx's work-loss guard never guards a repo under its scratch roots, so a live check of it there passes silently.
- [850](#850) An ask the user answers after 120 s comes back as a background-task notification, and the harness marks that…
- [848](#848) When several PreToolUse hooks deny one call, Claude Code shows the model only the reason of the LAST hook to finish…
- [816](#816) Claude Code 2.1.283 on this machine has no Grep or Glob tool: a hook or permission rule on Grep or Glob never fires…
- [787](#787) A reset that works in the scratch folder types 'continuando' into a session that resumes someone else's work…
- [785](#785) Claude Code 2.1.283 refuses a foreground sleep in the Bash tool, so 'run sleep 75 in the foreground' cannot hold a…
- [784](#784) Claude Code shows a prompt suggestion in the input box at a turn's end, and over Konsole's D-Bus it reads exactly like…
- [781](#781) An ekko ask that waits past 120 s moves to the background, and its answer comes back as a task notification. Claude…
- [777](#777) A /clear does not drop a background Bash task: the process runs on, and its completion notification arrives in the new…
- [768](#768) Claude Code's auto-mode classifier puts driving a Claude Code prompt through terminal keystrokes in a guarded category…
- [766](#766) The folder Claude Code gives a session's background-task output, /tmp/claude-&lt;uid>/&lt;project>/&lt;id>/tasks/, names the…
- [764](#764) Claude Code 2.1.283 fires UserPromptSubmit for a background task's notification too, not only for what the user types…
- [751](#751) A compaction's own summary call is in neither the transcript nor the headless result's usage: only the result's…
- [750](#750) evals/paired/harness.py's deny rule Read(\~/.claude/projects/\*\*) (and the trabalho twin), meant to hide the original…
- [749](#749) A point of the default account's 5-hour window was \~63k units on 2026-09-26, not the \~175k of note 444 (2026-09-23)…
- [745](#745) Headless runs on the default account are not trabalho's runs with another login. (1) Default's \~/.claude/settings.json…
- [727](#727) No unreleased change to the prefix since v0.23.0 (697, 2026-09-25 -03), which shipped the last one main held. The next…
- [724](#724) An ask answered after 120 s comes back as a background task notification, which Claude Code 2.1.283 marks as not user…
- [715](#715) A session with ekko's server instructions but no ekko tool (ToolSearch finds no mcp\_\_plugin_ekko_ekko\_\_ name): read…
- [679](#679) skillOverrides never reaches a plugin's skills. In Claude Code 2.1.282 the lookup answers 'on' for any skill whose…
- [646](#646) A scratch folder inside a worktree of the ekko repo -- target/ of \~/Projetos/ekko-&lt;branch>, say -- belongs to the main…
- [634](#634) In ekko, priority 3 is the most urgent and 1 is the default, as with Taskwarrior's H/M/L: 3 adds 6 to urgency, 2 adds…
- [631](#631) A line added to the MCP server's instructions fails the release. Since v0.21.0 they are 1,993 bytes, and…
- [624](#624) In a headless claude -p session, a FileChanged hook on a file outside the folder never fires (Claude Code 2.1.282…
- [617](#617) `claude --autocompact W` compacts at about W less 33k, not at W: it keeps 20k for the summary's output plus a 13k…
- [616](#616) A point of the trabalho account's 5-hour window is \~77k units, not the \~175k that note 444 measured on default. A point…
- [605](#605) Before fast-forwarding main's tree in \~/Projetos/ekko, gate the merge on no cargo running in that tree, in an if, not a…
- [600](#600) In \~/Projetos/ekko, git pull --ff-only fails whenever the tree holds uncommitted work (often another session's evals)…
- [598](#598) In create's text, a plain string, put a real line break after a task's title, not the two characters \\n: the text is…
- [580](#580) \~/NixOS's auto-backup commits and pushes only when nixos-autocommit.timer fires -- 5 minutes after the timer starts (at…
- [573](#573) usr.claude-code.settings in \~/NixOS is attrsOf anything, and anything merges attribute sets but not lists: a list such…
- [569](#569) EKKO_DIR outranks the board found from the folder: with it set, ekko uses &lt;EKKO_DIR>/.ekko even inside a folder whose…
- [561](#561) Since v0.19.1 the prime leads with the handoff the reading session wrote (task [514](tasks/514.md)), but only a handoff with an author…
- [558](#558) Questions to the user (ask) are written in Portuguese, the user's language, though the board's other items are in…
- [552](#552) Before editing ekko's code, make a worktree on its own branch (procedure [382](procedures.md#382)): other sessions share main's working…
- [546](#546) A UserPromptSubmit hook cannot tell a typed prompt from a scheduled one: in Claude Code 2.1.281 its input carries the…
- [522](#522) An ekko ask the user takes more than 120 s to answer is moved to the background by Claude Code, and its answers come…
- [521](#521) Thinking is not dropped from the context: every call's thinking stays and is re-read on every later call until /clear…
- [491](#491) A Claude Code background session keeps the config of the daemon that runs it, past a NixOS rebuild: end every…
- [466](#466) A PreToolUse hook cannot remind the model before a tool runs: its additionalContext reaches the model next to the…
- [461](#461) Claude Code's Bash tool cannot show how set -e behaves: it runs a command where errexit is ignored, so a one-liner `f()…
- [438](#438) Read Claude Code's docs raw, not through WebFetch's summary: curl -sSfL https://code.claude.com/docs/en/&lt;page>.md, then…
- [434](#434) ctx's flake builds from git: a new file not at least staged (git add) is missing from nix build, and a hook that…
- [245](#245) create's kind defaults to task: a note has to say kind note, or decision, gotcha, procedure or handoff. With…
- [224](#224) Never answer an MCP tool call with structuredContent while the text is what the model should read: Claude Code 2.1.278…
- [218](#218) An MCP server shipped by a plugin cannot have its resources @-mentioned in Claude Code 2.1.278: the mention resolver…
- [200](#200) Claude Code's native task list (the ✔ ◼ ◻ widget under the spinner) can be drawn from outside, but only with its Task…
- [194](#194) After an ekko upgrade, restart Claude Code before anything writes to the board: an MCP server started before the…

## <a id="977"></a>977. A Claude Code background session keeps the KONSOLE_DBUS\_\* address of the shell it was dispatched from, but its tab…

A Claude Code background session keeps the KONSOLE_DBUS\_\* address of the shell it was dispatched from, but its tab shows another process. Anything that types into "this session's tab" must first check that Konsole's foregroundProcessId is the session's own Claude Code pid.

Measured 2026-09-30: session 63cbaf7f ran under the supervisor (claude daemon, bg-pty-host, /dev/pts/2) with KONSOLE_DBUS_SESSION=/Sessions/1, whose foreground was another session's claude (pid 175284). Transcripts of such sessions carry agent-name rows.

*Gotcha · 2026-09-30 · on task [975](tasks/975.md)*

## <a id="974"></a>974. A running Claude Code session keeps the ctx hooks it started with. After a NixOS switch, only sessions started later…

A running Claude Code session keeps the ctx hooks it started with. After a NixOS switch, only sessions started later run the new ctx.

Measured on 2026-09-30, session 63cbaf7f in /projects/ekko:

- ctx was switched to 0.10.0 (2026-09-29 23:32:42) and then to 0.10.1 (00:03:03).
- The session still ran /nix/store/0rffdn70...-ctx-0.9.0/share/ctx/hooks/handoff at 00:03:47 and 00:04:55.

To see which ctx a session runs: its transcript's system rows of subtype stop_hook_summary carry hookInfos[].command.

Avoid: restart Claude Code after a ctx switch to get the new hooks. Whether a /clear reloads them was not tested.

*Gotcha · 2026-09-30*

## <a id="971"></a>971. ctx's auto-reset still types into a draft the user leaves unchanged in the input box for 10 minutes after a turn ended.

- On screen, the draft looks like a Claude Code prompt suggestion, which typing replaces.
- auto-reset only learns it was the user's text after typing into it, and then takes back what it typed.
- An Enter pressed in that second sends the prompt with '/clear' partly in it.

Avoid: send or clear a draft before walking away from a session past 250k. Or turn prompt suggestions off (promptSuggestionEnabled false), which would let auto-reset refuse any text in the box; not done as of ctx 0.10.1.

*Gotcha · 2026-09-30 · on task [967](tasks/967.md)*

## <a id="969"></a>969. ctx's auto-reset cannot reach a session the user leaves right after a turn ends.

- It runs only from the Stop hook, and no Stop fires while nobody is there.
- The prompt that brings the user back rewrites the whole context cold. Task [941](tasks/941.md) measured 292k after 143 minutes, on 2026-09-29.
- Only the cold-return guard acts at that prompt, and it is off since ctx 0.9.0.

To avoid it: /handoff and /clear before leaving a big session, or turn the guard on with CTX_COLD_TOKENS=250000.

*Gotcha · 2026-09-29 · on task [941](tasks/941.md)*

## <a id="959"></a>959. Older notes and the project memory name \~/Projetos/&lt;name>, but on this machine projects live in /projects/&lt;name>.…

Older notes and the project memory name \~/Projetos/&lt;name>, but on this machine projects live in /projects/&lt;name>. \~/Projects is a home-manager symlink to /projects, and \~/Projetos does not exist: read such a path as /projects/&lt;name>. A project registered under the old path is invisible to evals/resume/run.sh and to other tools that read projects.json, until `ekko init` runs in its new folder. That run only rewrites the path: done 2026-09-29 for ekko, graff and zettelkasten (note 955).

*Gotcha · 2026-09-29*

## <a id="945"></a>945. Count an agent's real tool calls from transcripts with jq on tool_use blocks, deduped by id, never with grep

Measured 2026-09-29 on ekko's tools: grep '"name":"mcp\_\_plugin_ekko_ekko\_\_' over \~/.claude\*/projects gave about 4,050 calls; assistant tool_use blocks with jq gave 2,055; deduped by tool_use id, 1,964. The grep also matches the tool listings (records of type attachment deferred_tools_record, and system records), which is why wait, away and phases showed 2 to 4 hits with zero calls, and the copies of earlier turns that a resumed session carries.

Count with: jq -r 'select(.type=="assistant") | .message.content[]? | select(.type=="tool_use") | [.id,.name] | @tsv' over each transcript, then sort -u on the id. Split by the project directory in the path: -home-roldant--cache-ekko-paired-\* are eval runs and -projects-teste is a scratch project.

Check the counter on a tool that was called (answer: 20) and on one that was not (wait: 0) before trusting a total. A tool's calls also undercount its feature: phases has 0 calls and is used on three real boards, set from the CLI.

Same for the CLI: take the Bash tool_use commands where ekko is in command position, regex (^|[;&|(\\n])\\s\*([A-Za-z\_]+=\\S+\\s+)\*(\\S\*/)?ekko(\\s|$), and check it on 'grep -n ekko f' and 'git commit -m "add ekko --prime"', which must not match. Do not run jq under xargs -P: long lines interleave and some rows come out corrupt (project names that are tool ids); use -P 1. Classify by the record's cwd, not by the project directory: sessions started in /projects (the parent folder) worked on winwayland, minium and others.

*Gotcha · 2026-09-29*

## <a id="935"></a>935. Grep ctx's guard.jsonl with a space after the colon: it writes "ev": "x", not "ev":"x".

Measured 2026-09-28 \~22:10 (-03): \~/.local/state/ctx/guard.jsonl writes {"ts": "...", "ev": "guard-pass", ...}, with a space after each colon, while handoff.jsonl writes {"ts":"...","ev":"auto-reset",...}, with none. grep -o '"ev":"[^"]\*"' matched 0 of guard.jsonl's 51 lines, and '"ev": \*"[^"]\*"' matched all 51. A pattern with the colon glued to the quote reads as 'no such event' when the event is there. Allow the space, grep the bare event name (window-refuse), or use jq.

*Gotcha · 2026-09-29 · on task [880](tasks/880.md)*

## <a id="933"></a>933. A test that copies a commit with git cherry-pick must move the target branch first. Picked onto the commit's own parent…

A test that copies a commit with git cherry-pick must move the target branch first. Picked onto the commit's own parent within the same second, with the same author and committer, the pick has the same tree, parent, message and timestamps, so it is that very commit, with the same id: the 'copy' is the original, and a check for patch copies passes for the wrong reason. It happened in ctx's suite on 2026-09-28 (task [891](tasks/891.md)): 'branch -D, its commit copied to main' passed against the old guard too, until a commit on main before the pick made the copy a copy. Also, git cherry-pick has no -q; send its output to /dev/null.

*Gotcha · 2026-09-28*

## <a id="928"></a>928. To see which ctx hooks a new Claude Code session will run, read \~/.claude\*/skills/ctx/.claude-plugin/plugin.json…

To see which ctx hooks a new Claude Code session will run, read \~/.claude\*/skills/ctx/.claude-plugin/plugin.json: home-manager links it to the ctx store path, which Claude Code loads as a plugin from the skills folder (plugins/data/ctx-skills-dir). The claude wrappers, --settings and --mcp-config never name ctx, so searching them finds nothing. Each hook in the package is a makeWrapper script that sets PATH, and the code is in hooks/.&lt;name>-wrapped: a grep on hooks/&lt;name> for the hook's text finds 0 even in the right build. Checked 2026-09-28 after the ctx 0.8.1 switch: both profiles resolved to ctx-0.8.1, and .cold-return-wrapped held the new text.

*Gotcha · 2026-09-28*

## <a id="927"></a>927. ctx's handoff.jsonl undercounts the prompts sent again after a cold-return stop. A /handoff typed after the stop…

ctx's handoff.jsonl undercounts the prompts sent again after a cold-return stop. A /handoff typed after the stop passes, as every slash command does, and pays the whole rewrite. The prompt sent after it then finds a warm cache and a newer last call, so it passes as an ordinary prompt and no cold-pass is logged. On 2026-09-28 (task [540](tasks/540.md)) the log read 2 cold-stop and 0 cold-pass, but both stops had ended in /handoff and then the same prompt again. The count is in the transcripts: the block is a system row of subtype informational, 'UserPromptSubmit operation blocked by hook: ...', ending in 'Original prompt: &lt;text>'. Compare that text with the next typed prompts, and look for a /handoff between them and its cache write (cw about the whole context).

*Gotcha · 2026-09-28*

## <a id="917"></a>917. Do not run cargo fmt --all in ekko either: rustfmt here reformats files nobody touched. Measured 2026-09-28 in the…

Do not run cargo fmt --all in ekko either: rustfmt here reformats files nobody touched. Measured 2026-09-28 in the devshell (rustc 1.97.1): cargo fmt --check listed diffs in src/agent.rs, which that session never edited, so the tree is not rustfmt-clean with this rustfmt. Format a change by hand to match the code around it, and do not treat fmt --check as a gate. It is not one: release.sh runs test, clippy and the build, not fmt. The same trap is in the user's CLAUDE.md for winwayland.

*Gotcha · 2026-09-28*

## <a id="896"></a>896. Never run `ekko --prime --hook` by hand from inside a Claude Code session, as a timing loop or a test: it records the…

Never run `ekko --prime --hook` by hand from inside a Claude Code session, as a timing loop or a test: it records the session_id it is fed as the conversation of the Claude Code process it runs under. On 2026-09-28, 50 timing runs with session_id "t" rewrote this session's process record (\~/.local/state/ekko/processes/&lt;boot>-&lt;pid>-&lt;start>.json): conversation became "t", the real 40565f65 went to earlier, and since and transcript were lost. It also wrote sessions/t.json. Fixed by writing the record back with jq and deleting t.json; `ekko --sessions` then named the right conversation. To time a hook, feed the session's own id and transcript_path, or run it under HOME/XDG_STATE_HOME pointing at a scratch folder. `--memory --hook` records nothing and is safe.

*Gotcha · 2026-09-28*

## <a id="866"></a>866. In ctx's test-hooks.sh, a helper called before its definition runs as "command not found": it counts as neither a pass…

In ctx's test-hooks.sh, a helper called before its definition runs as "command not found": it counts as neither a pass nor a failure, and the suite still reports 0 failures.

Found 2026-09-27 (task [808](tasks/808.md)): eq was defined in the auto-reset section, and the 22 eq checks of a section placed earlier never ran. A mutation they should have caught was caught only by the has checks around them. Define each helper beside has, before any case, and grep the suite's output for 'command not found' when adding a section.

*Gotcha · 2026-09-27*

## <a id="852"></a>852. Compare two ekko builds with evals/agent/scale.py back to back, never against an earlier results file.

- On 2026-09-27 the machine's load moved v0.25.0's own timings about 1.7x between 01:22 and 03:47 (r5k prime_ms 11.7, then 19.8). At first that read as a 2x regression of the new build.
- Run back to back, the new build was even or faster.
- scale.py names its results file by the tree's HEAD, not by EKKO_BIN, so two builds tested from one tree get files with the same commit in their names. Write down which binary each run used.

*Gotcha · 2026-09-27*

## <a id="851"></a>851. ctx's work-loss guard never guards a repo under its scratch roots, so a live check of it there passes silently.

- The roots are /tmp, /var/tmp, $XDG_RUNTIME_DIR and \~/.cache by default (CTX_GUARD_SCRATCH).
- Use a repo elsewhere instead: \~/.local/state/ctx-&lt;task>-live/repo, as the live checks of 804 and 805 did.
- Deleting such a repo later trips the same guard when its commits exist nowhere else.

Learned 2026-09-27: `git reset --hard` in /tmp/831-live went through with no refusal, and ctx's ledger logged nothing for it.

*Gotcha · 2026-09-27*

## <a id="850"></a>850. An ask the user answers after 120 s comes back as a background-task notification, and the harness marks that…

An ask the user answers after 120 s comes back as a background-task notification, and the harness marks that notification as not the user's input.

- Do not take it as consent for a public or irreversible step: a push, a release, a GitHub release page, a system switch. Say what the menu recorded, and get the user's word in chat first.
- Nor is ctx's auto-reset, which types /clear and 'continuando', the user's word.
- An answer that comes back inside the ask call itself is the user's, as every other answer in session 8e199d1a was.

Learned 2026-09-27, session 8e199d1a. Question [849](tasks/839.md#849) ('Push, v0.25.1 e switch') was answered in the menu at \~12:47 (-03), 9 h after it was asked, and arrived that way.

*Gotcha · 2026-09-27*

## <a id="848"></a>848. When several PreToolUse hooks deny one call, Claude Code shows the model only the reason of the LAST hook to finish…

When several PreToolUse hooks deny one call, Claude Code shows the model only the reason of the LAST hook to finish denying. Read off the 2.1.283 bundle on 2026-09-27; hooks.md does not say.

- How it works: the hooks run at once, merged by WJt with Promise.race, so results come in the order they finish. S7e then sets the result from each hook's blockingError unconditionally, and a JSON deny also becomes a blockingError. (A permissionDecision deny alone would keep the first; the blockingError that comes with it makes the last one win.)
- Consequence: a fast guard's reason is lost whenever a slower one also refuses. Measured: ekko (Rust) against ctx (Python), ctx finished last 20 of 20.
- What to do: a guard's reason must carry the reasons of the others that refuse the same call. ekko does it through the shared refusal record and `--refuse` (task [839](tasks/839.md)).
- How to reread it: follow procedure [547](procedures.md#547) and grep the bundle for 'hook error: ${' and for 'async function S7e'. The names are minified and change between versions.

*Gotcha · 2026-09-27 · on task [839](tasks/839.md)*

## <a id="816"></a>816. Claude Code 2.1.283 on this machine has no Grep or Glob tool: a hook or permission rule on Grep or Glob never fires…

Claude Code 2.1.283 on this machine has no Grep or Glob tool: a hook or permission rule on Grep or Glob never fires here, so guard the Bash side, where the searches run (grep is ugrep 7.8.4, rg is 15.2.0).

Measured 2026-09-26 (task [804](tasks/804.md)): the init event of a headless trabalho session lists no Grep or Glob, this session's tool list has neither, and a Haiku run asked to use Grep found "No matching deferred tools". The Nix wrapper sets USE_BUILTIN_RIPGREP=0; whether that is the cause is not measured. Also measured then: rg's type sh includes .env, so rg -t sh does not keep a .env out.

*Gotcha · 2026-09-26*

## <a id="787"></a>787. A reset that works in the scratch folder types 'continuando' into a session that resumes someone else's work…

A reset that works in the scratch folder types 'continuando' into a session that resumes someone else's work: /tmp/ctx-747-scratch has no board of its own, so ekko gives it the default board, whose latest handoff is a stale office365_flake one (note 5 there, 2026-09-21).

- Seen 2026-09-26 18:24-18:25 (-03): after 0.4.1's direct test, fresh session 95889721 was told 'continuando'. Its ekko calls were search, prime, context and changes. It then ran git status, log, branch and diff in \~/Projetos/office365_flake, since the window had --allowedTools Bash.
- Everything it ran only read. It was killed while thinking, with no write, on the board or on disk.
- In a real session this does not happen: the reset only follows a handoff the session itself wrote, and 'continuando' reads that one.
- For a live test in the scratch folder:
  - launch with CTX_AUTO_RESET_PROMPT='responda só: ok';
  - allow only what the test runs, e.g. --allowedTools 'Bash(sleep:\*)' 'Bash(echo:\*)';
  - close the window as soon as the ledger shows auto-reset.

*Gotcha · 2026-09-26 · on task [747](tasks/747.md)*

## <a id="785"></a>785. Claude Code 2.1.283 refuses a foreground sleep in the Bash tool, so 'run sleep 75 in the foreground' cannot hold a…

Claude Code 2.1.283 refuses a foreground sleep in the Bash tool, so 'run sleep 75 in the foreground' cannot hold a scratch session's turn open.

- Seen 2026-09-26 18:11 (-03), scratch session 35e7ee98: the tool result was 'Blocked: sleep 75 followed by: echo ok. To wait for a condition, use Monitor with an until-loop'. The turn ended 7 s after the prompt.
- To keep a scratch session busy for N seconds, or to have it stop with the user away, start the sleep in the background: the turn ends, and the completion notification (origin.kind 'task-notification', not 'human') starts a new turn N seconds later.
- Step 3c of handoff 783 had planned the foreground sleep.

*Gotcha · 2026-09-26 · on task [747](tasks/747.md)*

## <a id="784"></a>784. Claude Code shows a prompt suggestion in the input box at a turn's end, and over Konsole's D-Bus it reads exactly like…

Claude Code shows a prompt suggestion in the input box at a turn's end, and over Konsole's D-Bus it reads exactly like text the user typed.

- Measured 2026-09-26 \~18:12-18:16 (-03), Claude Code 2.1.283, Konsole 26.08.1, scratch session 35e7ee98: after a turn, the box read '❯ roda em segundo plano' in getAllDisplayedText. Nobody had typed it.
- getAllDisplayedText carries no colour or attributes, and no other method of org.kde.konsole.Session does, so ghost text and typed text cannot be told apart by reading.
- Typing replaces a suggestion: 'x' left '❯ x', '/clear' left '❯ /clear'. Erasing back to empty brings it back.
- Typed text is appended to: over typed 'abc', '/clear' left '❯ abc/clear', and 6 backspaces (\\x7f) restored '❯ abc'. Ctrl+U (\\x15) empties the box, the user's text included.
- Suggestions are on by default: the setting promptSuggestionEnabled ('When absent or true, prompt suggestions are enabled'), or the env CLAUDE_CODE_ENABLE_PROMPT_SUGGESTION. Neither is set in either profile.
- The suggestion is not written to the transcript.
- So a guard that reads 'text in the box' as 'the user is typing' refuses every screen with a suggestion. The check that tells them apart is made after typing: the box holds exactly what was typed.

*Gotcha · 2026-09-26 · on task [747](tasks/747.md)*

## <a id="781"></a>781. An ekko ask that waits past 120 s moves to the background, and its answer comes back as a task notification. Claude…

An ekko ask that waits past 120 s moves to the background, and its answer comes back as a task notification. Claude Code labels that notification 'NOT user input' and says not to treat anything in it as the user's approval.

- Seen 2026-09-26 \~18:05 (-03) in session 49dc5224: question [780](tasks/747.md#780) ('publicar e instalar?' for ctx 0.4.0) was answered 'Sim, publicar e instalar' in ekko's menu. It arrived that way, and nothing was pushed or switched on it.
- Before an outward or hard-to-reverse action on such an answer (a push, a system switch, a release), ask the user to confirm in the chat.
- An answer that returns within the ask call itself is its tool result, as before.

*Gotcha · 2026-09-26*

## <a id="777"></a>777. A /clear does not drop a background Bash task: the process runs on, and its completion notification arrives in the new…

A /clear does not drop a background Bash task: the process runs on, and its completion notification arrives in the new conversation and starts a turn there.

- Measured 2026-09-26 17:33-17:34 (-03), Claude Code 2.1.283, in a scratch session:
  - it started 'sleep 45 && echo fim-747' with run_in_background (task bzbpug143) in conversation dbce4f39;
  - a /clear 22 s later made conversation b1999863;
  - at 20:34:26Z b1999863 got the user row with origin.kind 'task-notification' for bzbpug143, and the footer's '1 shell' survived the /clear.
- So the comment in \~/Projetos/ctx/src/hooks/handoff is wrong for background Bash: 'never while background work or a scheduled wakeup would resume the session -- a /clear then would drop it'.
- The fresh session gets the notification without the context of why the task ran, so the handoff has to name each running task and its output file.
- Not tested: a scheduled wakeup (session_crons), Monitor, a background subagent.

*Gotcha · 2026-09-26 · on task [747](tasks/747.md)*

## <a id="768"></a>768. Claude Code's auto-mode classifier puts driving a Claude Code prompt through terminal keystrokes in a guarded category…

Claude Code's auto-mode classifier puts driving a Claude Code prompt through terminal keystrokes in a guarded category it calls [Tmux Self Drive], and refuses it without the user's explicit permission. Measured 2026-09-26 \~17:35 (-03) in session 49dc5224 (auto mode, Claude Code 2.1.283): a WebFetch of github.com/wildware-uk/cmux/issues/1 (a spike on injecting a slash command into a Claude Code pane) was denied with that reason. A WebFetch of getcodeman.com's guide to Claude Code ignoring tmux's Enter went through in the same turn. The denial covers the outcome, not the tool: any other way of doing the same thing counts as the same action. The only way it names to allow it is a Bash permission rule the user adds to their settings. 747's first live test (sendText of /clear into a Claude Code tab over Konsole's D-Bus) is in the same category, so it was not run; the user decides how to go on. Do not route around it.

A 'yes' through ekko's ask does not clear it. The user chose 'go on over D-Bus, with your permission' (question [769](tasks/747.md#769)), and the next Bash call was still denied with the same reason. That call only created /tmp/ctx-747-scratch and listed Konsole's tabs, a read-only sessionList, but it was preparing the denied outcome. Claude Code's docs (permission-modes, 'How the classifier evaluates actions') say why: the classifier sees user messages and tool calls, and tool results are stripped, so ask's answer never reaches it. An allow rule resolves before the classifier (step 1 of that order), so the user's permission has to come as a rule they add, narrow enough to survive auto mode's drop of broad rules.

A third denial followed, with a different reason, [Auto-Mode Bypass]. The call was a read-only git check-ignore of .claude/settings.local.json, to tell the user where a rule saved from /permissions would land. The classifier read helping set up that allow rule as a way around auto mode. So the user writes the rule on their own, and the session only tells them what it would say.

*Gotcha · 2026-09-26 · on task [747](tasks/747.md)*

## <a id="766"></a>766. The folder Claude Code gives a session's background-task output, /tmp/claude-&lt;uid>/&lt;project>/&lt;id>/tasks/, names the…

The folder Claude Code gives a session's background-task output, /tmp/claude-&lt;uid>/&lt;project>/&lt;id>/tasks/, names the conversation the process started with and keeps it after a /clear: session 83c87811 wrote there under 0dca8739, the session it had cleared, and signed six notes with the wrong id before its own 'written by' line gave it away (2026-09-26). Read a session's id from ekko's 'written by ... · &lt;id>' on an item it wrote (context), or from its transcript's file name, never from that path.

*Gotcha · 2026-09-26*

## <a id="764"></a>764. Claude Code 2.1.283 fires UserPromptSubmit for a background task's notification too, not only for what the user types…

Claude Code 2.1.283 fires UserPromptSubmit for a background task's notification too, not only for what the user types, so the time of the last UserPromptSubmit does not tell whether the user is there. Measured 2026-09-26 in session 83c87811 off ctx's ledger (\~/.local/state/ctx/handoff.jsonl): its cold-return hook logged cold-scheduled at 05:52:53 and 08:35:46 (-03), each the moment a background Bash's completion notification arrived after 86 and 121 idle minutes, and cold-stop at 16:20:34 on the user's own prompt after 456. The transcript tells them apart: each user row carries origin.kind -- 'human' for a typed prompt, an accepted suggestion or a slash command (promptSource typed, suggestion_accepted), 'task-notification' for a notification, 'auto-continuation' for Claude Code resuming after a usage-limit reset (both promptSource system). That session: 14 human, 20 task-notification, 1 auto-continuation. To know when the user last acted, read the transcript's last human row. Whether the hook's own input carries this was not checked (ctx's cold-return says it did not name the sender).

*Gotcha · 2026-09-26 · on task [747](tasks/747.md)*

## <a id="751"></a>751. A compaction's own summary call is in neither the transcript nor the headless result's usage: only the result's…

A compaction's own summary call is in neither the transcript nor the headless result's usage: only the result's modelUsage (and total_cost_usd) count it. 396/bmax/1 (2026-09-26, three compactions at \~160k): transcript and result.usage 4.08M units, modelUsage 4.27M, so \~63k units a compaction went unseen; the cmax sessions' three tallies agree (bar 0.08M of Haiku in one). Pricing B off transcripts alone flatters it. evals/paired/grade.py's report now shows billed units and dollars from modelUsage beside the transcripts' units; compare cells on billed.

*Gotcha · 2026-09-26 · on task [259](tasks/259.md)*

## <a id="750"></a>750. evals/paired/harness.py's deny rule Read(\~/.claude/projects/\*\*) (and the trabalho twin), meant to hide the original…

evals/paired/harness.py's deny rule Read(\~/.claude/projects/\*\*) (and the trabalho twin), meant to hide the original sessions' transcripts, also hides a run's own files: a SessionStart hook output too large to show inline is saved under the run's project folder (&lt;sid>/tool-results/hook-\*) and the session is told to Read it. In 396/cmax/1 (2026-09-26) each of the three sessions after a reset tried once and got 'File is in a directory that is denied by your permission settings'; the user's own sessions can read it. Kept as is for tonight's runs so that all four stay alike (the pilot process holds the old rules for cmax/2); the report must count the denied reads per reset in each cell, B's compactions included, before comparing. Fix for later runs: deny only the originals' folders, e.g. Read(\~/.claude\*/projects/-home-roldant-Projetos-\*/\*\*), then check with a run that the hook file reads and an original transcript does not.

*Gotcha · 2026-09-26 · on task [259](tasks/259.md)*

## <a id="749"></a>749. A point of the default account's 5-hour window was \~63k units on 2026-09-26, not the \~175k of note 444 (2026-09-23)…

A point of the default account's 5-hour window was \~63k units on 2026-09-26, not the \~175k of note 444 (2026-09-23): 3.95M units (259's first long run and two pings) took it from 1% to 64% between 00:57 and 01:43 (-03), and its week from 82% to 87%, \~0.8M units a weekly point. Price a run on default by this until it is measured again; evals/paired/harness.py POINTS holds 63k. Why it moved is not known (a changed plan, the accounting, weekend terms): untested. At this rate a long paired run (\~5-6M units) takes most of a 5-hour window on either account.

*Gotcha · 2026-09-26 · on task [259](tasks/259.md)*

## <a id="745"></a>745. Headless runs on the default account are not trabalho's runs with another login. (1) Default's \~/.claude/settings.json…

Headless runs on the default account are not trabalho's runs with another login. (1) Default's \~/.claude/settings.json sets autoCompactEnabled false; trabalho's leaves it on. Claude Code 2.1.283 decides auto-compact by that setting through the settings' precedence (read in its bundle: auto-compact is on only if autoCompactEnabled resolves true), and --settings outranks the user's file, so evals/paired/harness.py now writes autoCompactEnabled true into its settings: without it, a --autocompact cell may never compact there (inferred from the code, not measured). (2) Default's global config is \~/.claude.json, read while CLAUDE_CONFIG_DIR is unset; trabalho's is \~/.claude-trabalho/.claude.json. \~/.claude/.claude.json does not exist, so for default the harness leaves CLAUDE_CONFIG_DIR unset (PROFILE_ENV) instead of pointing it at \~/.claude; what Claude Code would do there was not tried. (3) A point of default's 5-hour window is \~175k units (note 444), trabalho's \~77k (gotcha [616](#616)). Both profiles run the same plugins (the same Nix store paths for ekko 0.24.0 and ctx 0.3.0) and the same model alias, opus.

Update 2026-09-26 \~18:50 (-03), task [789](tasks/789.md): the claude wrapper's --settings now sets autoCompactEnabled true and autoCompactWindow 450000 for both profiles, so default compacts too, at \~417k (measured with /autocompact and /context). The harness's own autoCompactEnabled true is now redundant, and harmless.

*Gotcha · 2026-09-26 · on task [259](tasks/259.md)*

## <a id="727"></a>727. No unreleased change to the prefix since v0.23.0 (697, 2026-09-25 -03), which shipped the last one main held. The next…

No unreleased change to the prefix since v0.23.0 (697, 2026-09-25 -03), which shipped the last one main held. The next change to the instructions or an always-loaded definition moves PREFIX_FINGERPRINT and makes every session resumed after the upgrade write its context once: batch such changes into one release, and its notes say that what Claude Code sends ahead of the conversation changed (gotcha 454).

*Gotcha · 2026-09-25 · replaces [713](#713)*

## <a id="724"></a>724. An ask answered after 120 s comes back as a background task notification, which Claude Code 2.1.283 marks as not user…

An ask answered after 120 s comes back as a background task notification, which Claude Code 2.1.283 marks as not user input, to be taken neither as approval nor as the answer to a pending question -- though the board records the answer as the user's (question 723, 2026-09-25 -03). An answer within 120 s returns in the call itself (question 718). So for an outward step -- push, tag, release, switch -- confirm in chat after an answer that arrived that way, before acting on it.

*Gotcha · 2026-09-25*

## <a id="715"></a>715. A session with ekko's server instructions but no ekko tool (ToolSearch finds no mcp\_\_plugin_ekko_ekko\_\_ name): read…

A session with ekko's server instructions but no ekko tool (ToolSearch finds no mcp\_\_plugin_ekko_ekko\_\_ name): read Claude Code's own MCP log before anything else, \~/.cache/claude-cli-nodejs/&lt;the folder, each / as ->/mcp-logs-plugin-ekko-ekko/&lt;newest>.jsonl (the resources server's is mcp-logs-ekko). Each connection logs its protocolEra and negotiatedProtocolVersion, and a refused list as 'tools/list failed (Invalid result ...)' with the fields at fault: that is how task 705 was found. Grep them as ./-home-.../ or after --, since the folder names start with '-' and grep reads a bare one as options. Without MCP tools the CLI still writes decisions, gotchas and procedures, but no handoff and no ask: pipe initialize and a tools/call into ekko --mcp from the project's folder, as handoff 714 was written.

*Gotcha · 2026-09-25*

## <a id="679"></a>679. skillOverrides never reaches a plugin's skills. In Claude Code 2.1.282 the lookup answers 'on' for any skill whose…

skillOverrides never reaches a plugin's skills. In Claude Code 2.1.282 the lookup answers 'on' for any skill whose source is 'plugin', so hiding one means disabling its plugin. It does reach claude.ai's synced skills (anthropic-skills:&lt;name>, loaded from &lt;profile>/skills/synced), from --settings too; checked on 2026-09-25 in both profiles. And the Artifact tool's 3 artifact-\* skills leave the listing with it.

*Gotcha · 2026-09-25 · on task [431](tasks/431.md)*

## <a id="646"></a>646. A scratch folder inside a worktree of the ekko repo -- target/ of \~/Projetos/ekko-&lt;branch>, say -- belongs to the main…

A scratch folder inside a worktree of the ekko repo -- target/ of \~/Projetos/ekko-&lt;branch>, say -- belongs to the main checkout's project, whatever HOME says: a linked worktree's project is the main checkout's (repository_root), so ekko init there answers 'Already a project' and every write lands on the real board. On 2026-09-25 a manual test of 503 wrote tasks 644 and 645 to it that way. Try a build from a folder in the session's scratchpad (under /tmp, outside any repository), or as procedure [458](procedures.md#458) says.

*Gotcha · 2026-09-25 · on task [503](tasks/503.md)*

## <a id="634"></a>634. In ekko, priority 3 is the most urgent and 1 is the default, as with Taskwarrior's H/M/L: 3 adds 6 to urgency, 2 adds…

In ekko, priority 3 is the most urgent and 1 is the default, as with Taskwarrior's H/M/L: 3 adds 6 to urgency, 2 adds 3.9, 1 adds nothing (src/agent.rs, urgency). The priority field's schema says only 'Tasks only.', and on 2026-09-24 a session read 3 as low: it created 623 and 629 at p3 meaning 'minor', so 623, a flaky test seen once, went to the top of the ready list, above 495 and 503. Give a minor task no priority.

*Gotcha · 2026-09-25 · on task [623](tasks/623.md)*

## <a id="631"></a>631. A line added to the MCP server's instructions fails the release. Since v0.21.0 they are 1,993 bytes, and…

A line added to the MCP server's instructions fails the release. Since v0.21.0 they are 1,993 bytes, and evals/agent/semantics.py row 0.8 caps them at 2,000 bytes, below the 2,048 characters that tests/mcp.rs allows. cargo test passes such a line, and scripts/release.sh then stops at its semantics step. So name a new tool where the need arises instead -- a refusal, a reply -- as v0.21.0 did for wait (HELD, and next's line of work held elsewhere; note [626](tasks/389.md#626)), or cut the instructions first. Either way the prefix fingerprint moves only when they change.

*Gotcha · 2026-09-24*

## <a id="624"></a>624. In a headless claude -p session, a FileChanged hook on a file outside the folder never fires (Claude Code 2.1.282…

In a headless claude -p session, a FileChanged hook on a file outside the folder never fires (Claude Code 2.1.282, tested 2026-09-24). The case: a file watched through the watchPaths that a SessionStart hook returns, with stream-json input. After a write to the board's storage.json, neither ekko's task list hook nor ekko --wake --hook ran. A file inside the folder, named by the hook's matcher, did fire, and did wake that same kind of session. So the wake hook reaches only interactive sessions: a headless one, such as a paired-eval run, learns that its wait is over in its next ekko reply or its prime.

*Gotcha · 2026-09-24 · on task [389](tasks/389.md)*

## <a id="617"></a>617. `claude --autocompact W` compacts at about W less 33k, not at W: it keeps 20k for the summary's output plus a 13k…

`claude --autocompact W` compacts at about W less 33k, not at W: it keeps 20k for the summary's output plus a 13k buffer. To compact at T, pass T + 33k. The probe of 2026-09-24 (Claude Code 2.1.281) set W to 100k, and compactions started from 67k (78-79k when one file read crossed the line). After a compaction the context was 13-17k. evals/paired/harness.py sets WINDOW this way.

*Gotcha · 2026-09-24 · on task [259](tasks/259.md)*

## <a id="616"></a>616. A point of the trabalho account's 5-hour window is \~77k units, not the \~175k that note 444 measured on default. A point…

A point of the trabalho account's 5-hour window is \~77k units, not the \~175k that note 444 measured on default. A point of its week is \~0.9M units. Measured on 2026-09-24: 1.85M units took trabalho's 5-hour window from 67% to 91% and its week from 40% to 42%. Price a run by the account that pays for it. The headless stream reports both windows (procedure [570](procedures.md#570)).

*Gotcha · 2026-09-24*

## <a id="605"></a>605. Before fast-forwarding main's tree in \~/Projetos/ekko, gate the merge on no cargo running in that tree, in an if, not a…

Before fast-forwarding main's tree in \~/Projetos/ekko, gate the merge on no cargo running in that tree, in an if, not a line printed before it: on 2026-09-24 a session printed pgrep's hit and merged anyway. pgrep -afA 'cargo|rustc' also lists the paired eval's builds (task [526](tasks/526.md)), which run headless Claude sessions in clones under \~/.cache/ekko-paired/\*/repo; readlink /proc/&lt;pid>/cwd tells them apart, and a build in such a clone does not hold main's tree.

*Gotcha · 2026-09-24*

## <a id="600"></a>600. In \~/Projetos/ekko, git pull --ff-only fails whenever the tree holds uncommitted work (often another session's evals)…

In \~/Projetos/ekko, git pull --ff-only fails whenever the tree holds uncommitted work (often another session's evals): git is set to pull by rebase, and a rebase needs a clean tree ('cannot pull with rebase: You have unstaged changes'). Run git fetch origin && git merge --ff-only origin/main instead. It fast-forwards and keeps the uncommitted files, as long as the incoming commits don't touch them.

*Gotcha · 2026-09-24*

## <a id="598"></a>598. In create's text, a plain string, put a real line break after a task's title, not the two characters \\n: the text is…

In create's text, a plain string, put a real line break after a task's title, not the two characters \\n: the text is kept exactly as given, so \\n stays a backslash and an n on the title line. Task [595](tasks/595.md) was created that way on 2026-09-24 and fixed with an edit. In batch, whose ops are JSON, \\n in a string is a line break. Since v0.20.0 a task's first line is its title, at most 80 characters, so a title run into its body with a literal \\n is refused as too long, or, when short enough, lists with the \\n in it.

*Gotcha · 2026-09-24*

## <a id="580"></a>580. \~/NixOS's auto-backup commits and pushes only when nixos-autocommit.timer fires -- 5 minutes after the timer starts (at…

\~/NixOS's auto-backup commits and pushes only when nixos-autocommit.timer fires -- 5 minutes after the timer starts (at login, or at a switch that changes the unit), then 7 minutes after each run. Before leaving a change to it, check that `systemctl --user list-timers nixos-autocommit.timer` shows a NEXT; if it does not, `systemctl --user start nixos-autocommit.service` commits and pushes at once and re-arms the cycle. Until task [575](tasks/575.md) (2026-09-24) the first run counted from boot (OnBootSec), so a login more than 5 minutes after boot left it dead: nothing was committed from 05:32 to 19:45 that day, and the staged tree was the only copy of the running configuration's source.

*Gotcha · 2026-09-24 · replaces [572](#572)*

## <a id="573"></a>573. usr.claude-code.settings in \~/NixOS is attrsOf anything, and anything merges attribute sets but not lists: a list such…

usr.claude-code.settings in \~/NixOS is attrsOf anything, and anything merges attribute sets but not lists: a list such as permissions.allow defined in two modules fails the eval ('... Use `lib.mkForce value` or `lib.mkDefault value` to change the priority on any of these definitions'). usr/ekko.nix holds the only permissions.allow, ask and deny: add a rule to its lists. Task 531 put nixos-rebuild build, nvd diff and nix flake check there after the eval refused them in usr/claude-code.nix.

*Gotcha · 2026-09-24*

## <a id="569"></a>569. EKKO_DIR outranks the board found from the folder: with it set, ekko uses &lt;EKKO_DIR>/.ekko even inside a folder whose…

EKKO_DIR outranks the board found from the folder: with it set, ekko uses &lt;EKKO_DIR>/.ekko even inside a folder whose .ekko/project.json names a project (src/directory.rs, locate: --ekko-dir > --project or EKKO_PROJECT > EKKO_DIR > the folder > \~/.ekko). To give a scratch copy of a board to a session, put the copy's .ekko in its folder and leave EKKO_DIR and EKKO_PROJECT unset; finding a board writes nothing to the registry (\~/.ekko/projects.json changes only on init and destroy). Seen 2026-09-24, session 8c68c63f: the paired-test probe wrote its note to an empty scratch board.

*Gotcha · 2026-09-24*

## <a id="561"></a>561. Since v0.19.1 the prime leads with the handoff the reading session wrote (task [514](tasks/514.md)), but only a handoff with an author…

Since v0.19.1 the prime leads with the handoff the reading session wrote (task [514](tasks/514.md)), but only a handoff with an author ranks that way: one written by a server older than v0.19.0 -- a session started before that upgrade -- names none and still ranks by its task alone, and the hook's prime ranks as the system's ekko does, so before the rebuild to v0.19.1 nothing changes. Until then, and for handoffs with no author, a session resuming after /clear can be shown another's: the prime says whose it shows ('by this session', 'which has ended', 'which still runs'); when it names no author, check the handoff's first line and resume from your own by id: context &lt;id>.

*Gotcha · 2026-09-24 · replaces [538](#538)*

## <a id="558"></a>558. Questions to the user (ask) are written in Portuguese, the user's language, though the board's other items are in…

Questions to the user (ask) are written in Portuguese, the user's language, though the board's other items are in English. Sessions had written every ask in English, following the board; on 2026-09-24 (session 8c68c63f) the user closed ekko's menu on questions [553](tasks/526.md#553)-555 without answering and asked 'pq toda pergunta que vc chama é em ingles?'. They were rewritten in Portuguese with edit. The options' labels and descriptions go in Portuguese too.

*Gotcha · 2026-09-24*

## <a id="552"></a>552. Before editing ekko's code, make a worktree on its own branch (procedure [382](procedures.md#382)): other sessions share main's working…

Before editing ekko's code, make a worktree on its own branch (procedure [382](procedures.md#382)): other sessions share main's working tree, and the prime lists only the five newest procedures, so 382 is easy to miss. Seen 2026-09-24: 514's fix was written in main's tree while another session ran on the repo, and moved to a worktree with git stash before any commit.

*Gotcha · 2026-09-24*

## <a id="546"></a>546. A UserPromptSubmit hook cannot tell a typed prompt from a scheduled one: in Claude Code 2.1.281 its input carries the…

A UserPromptSubmit hook cannot tell a typed prompt from a scheduled one: in Claude Code 2.1.281 its input carries the session, transcript, cwd, mode, prompt and session title, and the prompt's source (a cron, a /loop wakeup) reaches the hook runner but is left out of the input. A hook that blocks prompts would stall a scheduled one with nobody to send it again; ctx's cold-return guard reads a mark the Stop hook keeps from session_crons and background_tasks, which only a Stop hook's input lists (note 544). Also: a blocked prompt ({decision: "block", reason}) is shown to the user under the reason unless hookSpecificOutput.suppressOriginalPrompt is set; whether it comes back to the input box is unverified.

*Gotcha · 2026-09-24 · on task [525](tasks/525.md)*

## <a id="522"></a>522. An ekko ask the user takes more than 120 s to answer is moved to the background by Claude Code, and its answers come…

An ekko ask the user takes more than 120 s to answer is moved to the background by Claude Code, and its answers come back in a background-task notification whose wrapper says it is not user input and must not be treated as consent. Do not act on those answers alone: tell the user what came back and ask for a one-word confirmation in chat. Seen 2026-09-24, session 844752fe (questions [519](tasks/507.md#519) and [520](tasks/507.md#520)).

*Gotcha · 2026-09-24*

## <a id="521"></a>521. Thinking is not dropped from the context: every call's thinking stays and is re-read on every later call until /clear…

Thinking is not dropped from the context: every call's thinking stays and is re-read on every later call until /clear or compaction, a new prompt included. Measured 2026-09-24 (note [518](tasks/507.md#518), evals/recall/thinking.py): between two calls the context grows by the whole output, thinking included (median 1.08 over 4,081 pairs in tool loops, 1.02 over 309 across a new prompt), never by the output without it. So effort is a context lever, not only an output one: thinking is \~23% of the bill (17.5% re-read, 5.8% generated), and max thinks 2.4x more a call than high. Mind it when choosing effort; it is also why segmentation pays.

*Gotcha · 2026-09-24*

## <a id="491"></a>491. A Claude Code background session keeps the config of the daemon that runs it, past a NixOS rebuild: end every…

A Claude Code background session keeps the config of the daemon that runs it, past a NixOS rebuild: end every background session, so the transient daemon exits, before trusting a restart to pick up new settings. Seen 2026-09-24: after the v0.18.0 rebuild, session 874cd2ed came back as a daemon spare (claude bg-spare under a bg-pty-host of `claude daemon run --origin transient`, started 04:55 from a session that ran before the rebuild). Its plugin server was ekko 0.18.0, but the resources server from usr.claude-code.mcpServers, which the wrapper's --mcp-config carries, was 0.17.0, and so was every path of the old --mcp-config and --settings. A deny added to settings would not reach such a session either. /exit only detaches from a background session; `claude stop` takes the job's short id (874cd2ed), not the session's UUID; `claude agents --json` lists them. A session started by the new wrapper in a terminal gets the new paths. 2026-09-27: a /clear is no restart either. After the 17:27 switch to ctx 0.8.0 and ekko 0.26.0, the user typed /clear and 'continue' in background session a7ab82f1. Its process, 1144255 (started 16:01), kept its old --settings, so ctx 0.7.0's status line and hooks, and its ekko 0.25.0/0.25.1 MCP servers. \~/.local/state/ctx/window/, which 0.8.0's status line writes, did not exist. Before a check that needs the new versions, compare the start of the session's Claude process, `ps -o lstart= -p $PPID` from a Bash call, with the switch, `stat -c %y /run/current-system` (the link's own time). On 2026-09-27 these read 16:01:36 and 17:27:24. Also 2026-09-27, 22:33: neither /exit nor /resume from a new terminal restarts it. The user typed /exit in background session a7ab82f1 and started a new claude on pts/0 (22:33:16, new --settings). There they ran /resume (22:33:22, recorded in conversation 4e8872a1). Their next prompt, 'pronto, valida agora', reached the same conversation, still in process 1144255 of 16:01, with ctx 0.7.0 and ekko 0.25.0/0.25.1 servers. Only `claude stop a7ab82f1` ends that process and lets the transient daemon (1144222) exit. 2026-09-27, 22:44: that /resume left a second background session under the same daemon. Its reply read 'Session 4f0e8b16 is running as a background session (a7ab82f1)', and at 22:33:24 a daemon spare (process 1144254) was claimed as session da2567e6, named resume-background-session. Its transcript holds only a title and an agent name; `claude agents --json` shows it idle, state blocked. Unlike a7ab82f1 it runs the new config: both ekko servers are 0.26.1, and ctx 0.8.0's window hook told it at 22:33:25. At 22:44 the user typed /clear (new conversation 674ff22c) and /exit in a7ab82f1: /exit only detached, and 1144255 still runs. By this gotcha's rule the daemon exits only when every background session ends, so it takes both `claude stop a7ab82f1` and `claude stop da2567e6` (not yet measured).

*Gotcha · 2026-09-24*

## <a id="466"></a>466. A PreToolUse hook cannot remind the model before a tool runs: its additionalContext reaches the model next to the…

A PreToolUse hook cannot remind the model before a tool runs: its additionalContext reaches the model next to the tool's result, so for AskUserQuestion only after the user has answered (hooks.md, read raw 2026-09-23). To act before the call, the hook has to deny it: permissionDecision "deny", whose permissionDecisionReason the model sees. The same holds for any "do X before calling Y" rule built on hooks.

*Gotcha · 2026-09-23 · on task [395](tasks/395.md)*

## <a id="461"></a>461. Claude Code's Bash tool cannot show how set -e behaves: it runs a command where errexit is ignored, so a one-liner `f()…

Claude Code's Bash tool cannot show how set -e behaves: it runs a command where errexit is ignored, so a one-liner `f() { false; echo continued; }; ( set -e; f )` printed 'continued' and returned 0 (2026-09-23, task [391](tasks/391.md)). The same lines in a script file run with bash stopped at the failure and returned 1. Test errexit, traps and pipefail in a file, never inline.

*Gotcha · 2026-09-23 · on task [391](tasks/391.md)*

## <a id="438"></a>438. Read Claude Code's docs raw, not through WebFetch's summary: curl -sSfL https://code.claude.com/docs/en/&lt;page>.md, then…

Read Claude Code's docs raw, not through WebFetch's summary: curl -sSfL https://code.claude.com/docs/en/&lt;page>.md, then grep it. On 2026-09-23, WebFetch's summary of the hooks page invented Stop fields (hookSpecificOutput.continueReason, stopReason) that the page does not have; the raw page gives decision and reason, and hookSpecificOutput.additionalContext. Found by session 06637eec.

Again on 2026-09-26 (session 4e640549, task [770](tasks/770.md)): asked to quote the hooks page verbatim, WebFetch's summary printed a section 'Hook Behavior Under bypassPermissions', in quotes, saying a hook's ask 'forces a permission prompt even under bypass'. The raw page has no such passage; its only bypass statements are elsewhere. Asking for verbatim quotes does not stop the invention.

*Gotcha · 2026-09-23*

## <a id="434"></a>434. ctx's flake builds from git: a new file not at least staged (git add) is missing from nix build, and a hook that…

ctx's flake builds from git: a new file not at least staged (git add) is missing from nix build, and a hook that sources it fails in the build's suite. Stage new files before nix build; staging is not a commit. And run ctx's suite through nix build (or result/bin/ctx-test), not bash src/test-hooks.sh: outside Nix, check-bash-read's 40 cases fail for want of python3.

*Gotcha · 2026-09-23 · on task [424](tasks/424.md)*

## <a id="245"></a>245. create's kind defaults to task: a note has to say kind note, or decision, gotcha, procedure or handoff. With…

create's kind defaults to task: a note has to say kind note, or decision, gotcha, procedure or handoff. With attached_to, a missing kind is refused (ATTACH_NOT_A_NOTE) and nothing is written. Without attached_to it goes through silently as a task. That is how the viability review 241 landed in Ready as a p1 task on 2026-09-21, when the handoff calls it a note.

UPDATE 2026-09-21: task [282](tasks/282.md) makes attached_to with no kind a note, in the working tree. Until that build is installed and Claude Code restarted, still say kind note.

*Gotcha · 2026-09-21*

## <a id="224"></a>224. Never answer an MCP tool call with structuredContent while the text is what the model should read: Claude Code 2.1.278…

Never answer an MCP tool call with structuredContent while the text is what the model should read: Claude Code 2.1.278 sends the model only that structure's compact JSON and drops every text block of content, with or without an outputSchema. Measured 2026-09-21 (MCP protocol 2025-11-25) by evals/claude-code/structured_content.py, no quota spent. A text-only result arrived as [{"type":"text","text":"PLAIN_TEXT_B3F"}]. A result with the text 'STRUCT_TEXT_M4K: see the structure' plus structuredContent arrived as the plain string {"marker":"STRUCT_DATA_Q8V","rev":4242,"ids":[7,9]}, and the text marker never reached the model. Declaring an outputSchema changed nothing. The model never gets structure as such, only text, so structure cannot shorten what the model reads more than compact text can. The bundle inside the binary agrees: it checks structuredContent before content, keeps only the non-text blocks (image, audio, resource, resource_link) and appends the JSON as text. That last part comes from reading the code, not from a run. stream-json's tool_use_result carries the object as well, so an Agent SDK consumer can read it, but the model cannot. ekko sends neither today: no match in src/, tests/ or plugin/. After a Claude Code upgrade, re-run the eval before anything depends on structuredContent.

*Gotcha · 2026-09-21 · on task [176](tasks/176.md)*

## <a id="218"></a>218. An MCP server shipped by a plugin cannot have its resources @-mentioned in Claude Code 2.1.278: the mention resolver…

An MCP server shipped by a plugin cannot have its resources @-mentioned in Claude Code 2.1.278: the mention resolver splits on the FIRST colon. Found 2026-09-21 in the binary (.claude-unwrapped, near offset 201782583, function Ajo): `let[O,...L]=w.split(":"),B=L.join(":")` then `h.find(ve=>ve.name===O)`. A plugin server is named plugin:&lt;plugin>:&lt;server>. So @plugin:ekko:ekko:item://107, which the @ menu itself builds, looks for a server named 'plugin' with the uri 'ekko:ekko:item://107'. No server matches, and the mention attaches nothing, silently: the transcript shows no mcp_resource attachment. Seen twice with the user after v0.10.0, once picked from the menu. The same resources served under a colon-free name (ekkodev, via --mcp-config) attach fine. Until Claude Code fixes the split, mentions need ekko served under a plain server name, outside the plugin.

SECOND TRAP, found 2026-09-21 while validating the first: the extractor (function Ujo) is /(^|[\\s。、？！])@([^\\s]+:[^\\s]+)\\b/g, and its trailing \\b cuts a mention back to its last word character. So a URI ending in a symbol can never be mentioned, from any server: @ekko:prime:// is read as 'ekko:prime', server ekko, uri 'prime'. Simulated with the same pattern in Python: item://107 survives whole, prime:// does not. Fixed in ekko by naming the prime prime://board (PRIME_URI in src/mcp.rs). VALIDATION SO FAR. The docs (code.claude.com/docs/en/mcp) name plugin servers plugin:&lt;plugin>:&lt;server> and document mentions as @server:protocol://path, with examples only of colon-free names. No GitHub issue or changelog entry covers either trap; 2.1.278 is the newest version. The autocomplete builds `${server}:${uri}` (the same bundle, near offset 218234177). A/B, same Claude Code, same board, same item: @ekko:item://107 from the --mcp-config server attached; @plugin:ekko:ekko:item://107 from the plugin's 0.10.0 server did not, twice, with no mcp_resource attachment in the transcript. The run with @plugin:ekko:ekko:prime:// is confounded by the \\b trap and proves nothing about the colon.

VALIDATED 2026-09-21, both traps, with the same ekko 0.10.0 binary on the same board. Served as a plain 'old' server through --mcp-config: @old:item://107 attached (an mcp_resource attachment, server old, uri item://107, in the transcript), and @old:prime:// attached nothing. Only the URI's last character differs, which is the \\b trap. Served as the plugin's plugin:ekko:ekko: @plugin:ekko:ekko:item://107 attached nothing, twice. Only the server's name differs from the attached case, which is the colon trap. Pasted messages resolve mentions like typed ones. The first try at a probe packaged as a plugin (--plugin-dir with a .mcp.json) never started its server, and that run proves nothing.

*Gotcha · 2026-09-21 · on task [174](tasks/174.md)*

## <a id="200"></a>200. Claude Code's native task list (the ✔ ◼ ◻ widget under the spinner) can be drawn from outside, but only with its Task…

Claude Code's native task list (the ✔ ◼ ◻ widget under the spinner) can be drawn from outside, but only with its Task tools switched on. Claude Code 2.1.278 offers those tools only on Claude 3.x, Opus 4.0-4.7, Sonnet 4.0-4.6 and Haiku 4.5. On any other model, Opus 5 included, set CLAUDE_CODE_ENABLE_TODO_TOOLS=1: both the changelog and the gating code in the binary say so. Without the tools there is no widget at all, and that is what note [199](#199)'s test ran into. With them, on 2026-09-21, tasks written from outside as \~/.claude-trabalho/tasks/&lt;list>/N.json (id, subject, description, activeForm, status, blocks, blockedBy, plus .lock and .highwatermark) showed live. The in-progress task's activeForm became the spinner text, 'Next:' named the next pending task, and with showExpandedTodos on the whole list showed ('4 tasks (3 done, 1 in progress, 0 open)'). showExpandedTodos is a key in the global config (\~/.claude-trabalho/.claude.json), read when a session opens. Ctrl+T (app:toggleTodos) flips it, but the user's terminal takes Ctrl+T for a new tab. A session's own list is the folder named by its session id, and CLAUDE_CODE_TASK_LIST_ID points a session at another list. The settings reference documents none of this.

*Gotcha · 2026-09-21 · replaces [199](#199)*

## <a id="194"></a>194. After an ekko upgrade, restart Claude Code before anything writes to the board: an MCP server started before the…

After an ekko upgrade, restart Claude Code before anything writes to the board: an MCP server started before the rebuild keeps running the old binary, and an older ekko drops the fields it does not know when it rewrites storage.json. Since v0.11.0 an item keeps its unknown fields, but a question does not (measured 2026-09-27: one --task from v0.24.0 over a 0.25.0 board kept the gotcha's cue and dropped every question's cue and allow, 1 of 1 and 11 of 11; task [829](tasks/829.md)). The store path in ps -eo lstart,args | grep 'ekko --mcp' names the version each server runs. Seen 2026-09-21, right after v0.8.0: three 0.7.2 servers in .claude-trabalho outlived the user's rebuild. /mcp Reconnect does not fix it. A Claude Code process started before the rebuild spawns the same old store path again (seen 2026-09-21 02:50: pid 181397, 0.7.2 once more). Only a new process picks up the new binary. A background session (claude bg-pty-host) outlives /exit of its window, so that session has to be ended itself.

COST OF THE RESTART, measured 2026-09-21 (note [258](tasks/180.md#258)). A resumed session reuses its prompt cache across an upgrade only when the release left unchanged what Claude Code sends ahead of the conversation: 0.9.1, 0.10.0 and 0.10.1 resumed on the cache. When that changed, every session resumed after it wrote its whole context again, about twice its size in units: 0.67M to 1.38M for sessions at 357k to 710k, and 4.53M, 9.3% of the day, on 2026-09-21. It happened after 0.8.0, 0.9.0, 0.10.2 and 0.11.0. For the last two the cause was an always-loaded tool's definition (alwaysLoad, then waiting in set_state), checked against the binaries. 0.9.0 changed no ekko tool, so a setting shipped in the same rebuild is the likely cause there. So before restarting a large session for a release that changes a tool definition, the server instructions or Claude Code's settings, write its handoff and /clear it: the restart then writes about 40k. Also seen after v0.11.0: a session forked from a process started before the rebuild ran the new plugin server, which is found on PATH, but kept the old --mcp-config, so its ekko --mcp --resources stayed on 0.10.2.

FROM 0.12.0 (task [274](tasks/274.md)): a server whose binary was replaced ends every reply with a note asking for a restart, so a stale session says so itself. The first upgrade it catches is the one after 0.12.0.

RESUMED CONVERSATIONS KEEP THE OLD DEFINITIONS (2026-09-24, Claude Code 2.1.280). Session db8b907e was resumed with claude --continue after the 0.17.0 rebuild. Its servers ran 0.17.0, yet ask stayed deferred there, though 0.17.0 marks it always-loaded. ToolSearch returned the 0.16.0 definition: no options, 'Record it here, then ask the user'. That is the one the transcript's deferred_tools_record saved when the conversation first loaded the tool (00:27 UTC). No deferred_tools_delta or mcp_instructions_delta row was written after the restart, so the instructions it held were 0.16.0's. The new settings did apply: the AskUserQuestion deny removed that tool. A new conversation (claude -p in an empty folder) had ask loaded at start with the 0.17.0 definition and options. So check a release's tools in a new conversation (/clear or a new session), never in a resumed one. 2026-09-24, v0.18.0: a conversation resumed after the upgrade kept ask's 0.17.0 definition even though ask is always loaded -- ToolSearch returned the old schema -- yet a call in the new shape (questions) went through to the 0.18.0 server and worked.

*Gotcha · 2026-09-21*

## Replaced

### <a id="713"></a>713. The next change to what Claude Code sends ahead of the conversation -- 697's removal of project and projects -- moves…

The next change to what Claude Code sends ahead of the conversation -- 697's removal of project and projects -- moves the prefix by itself: 638 shipped in v0.22.2 on 2026-09-25 -03, with 705. Batch any other change to the instructions or an always-loaded definition into 697's release, move PREFIX_FINGERPRINT with it, and say in that release's notes that what Claude Code sends ahead of the conversation changed.

*Gotcha · 2026-09-25 · replaces [662](#662) · replaced by [727](#727)*

### <a id="662"></a>662. main holds ed0da94 (638, the priority field's scale) unreleased since v0.22.1, and it moves the prefix every session…

main holds ed0da94 (638, the priority field's scale) unreleased since v0.22.1, and it moves the prefix every session pays (+58 bytes, PREFIX_FINGERPRINT 0xb5de580a429a0ef2). By the user's answer 661, it ships with the next release, whatever that brings: that release's notes say what Claude Code sends ahead of the conversation changed, and batch any other change to the instructions or an always-loaded definition into it. Once shipped, this gotcha is history: supersede it.

*Gotcha · 2026-09-25 · replaced by [713](#713)*

### <a id="572"></a>572. \~/NixOS's auto-commit has not run since the boot of 2026-09-24 12:53: nixos-autocommit.timer…

\~/NixOS's auto-commit has not run since the boot of 2026-09-24 12:53: nixos-autocommit.timer (modules/usr/nixos-autocommit.nix) fires on OnBootSec=5min, then OnUnitActiveSec=7min, but the user's systemd started it at login, 13:03:06, after boot+5min had passed, so it never fired, and OnUnitActiveSec counts from the service's last run, which never came under this login. `systemctl --user status nixos-autocommit.timer` reads 'active (elapsed)', Trigger: n/a; the journal's last run is 05:48, and HEAD is still c031b79 (05:32). Whenever login comes more than 5 minutes after boot, nothing in \~/NixOS is committed or pushed, and the staged working tree is the only copy of the running configuration's source: no git stash, checkout or reset there. Before leaving a change to the auto-backup, check `systemctl --user list-timers nixos-autocommit.timer` shows a NEXT. The fix: task [575](tasks/575.md).

*Gotcha · 2026-09-24 · replaced by [580](#580)*

### <a id="538"></a>538. Until task [514](tasks/514.md) is built, a session resuming after /clear can be shown another session's handoff: the prime puts first a…

Until task [514](tasks/514.md) is built, a session resuming after /clear can be shown another session's handoff: the prime puts first a handoff on a task the reader holds in progress, and otherwise the newest of all -- so with handoffs on waiting or pending tasks, the newest from any session leads every prime (seen 2026-09-24 with 516 and 528, note [536](tasks/514.md#536)). Say whose a handoff is in its first line (session id and profile), and when the prime leads with another session's, resume from your own by id: context &lt;id>.

*Gotcha · 2026-09-24 · replaced by [561](#561)*

### <a id="199"></a>199. Claude Code's native task list (the ✔ ◼ ◻ widget under the spinner) cannot be fed from outside. Tested 2026-09-21 on…

Claude Code's native task list (the ✔ ◼ ◻ widget under the spinner) cannot be fed from outside. Tested 2026-09-21 on v2.1.278: tasks written to \~/.claude-trabalho/tasks/&lt;id>/N.json, with CLAUDE_CODE_TASK_LIST_ID set to that id, never showed, idle or while working. That session had no Task tools at all, and the widget is drawn from them; Claude Code never even took the list's .lock. The tools come and go by Claude Code's own rules: in session 29cf2249 they appeared mid-session and were gone after a resume. The official docs cover neither the format (id, subject, description, activeForm, status, blocks, blockedBy, plus .lock and .highwatermark) nor the sharing by CLAUDE_CODE_TASK_LIST_ID. beads proposed the same bridge (steveyegge/beads#1361) and closed it as not planned.

*Gotcha · 2026-09-21 · replaced by [200](#200)*
