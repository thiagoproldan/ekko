<!-- Written by ekko docs from the board of ekko: change the board and run it again, since edits here are overwritten. -->

# Procedures

Steps that work: 18 in force, newest first.

- [994](#994) To know which CLAUDE.md a Claude Code session was given, read the instructions attachments in its transcript, not the…
- [953](#953) Test ctx's 5-hour cap without waiting for a real crossing: give a scratch session its own CTX_STATE, whose…
- [876](#876) Show that a new test can say no: take the rule out of one place at a time, and demand a failure at the expected check…
- [875](#875) Time ekko MCP calls, build A against build B, over stdio from bash (task [859](tasks/859.md), decision [872](decisions.md#872)).
- [793](#793) Read a session's auto-compact window and threshold from inside, without writing settings:
- [782](#782) Open a scratch Claude Code window from an agent session, for a live test of typing into Claude Code (task [747](tasks/747.md)), without…
- [755](#755) Wait on a long job with one background Bash until-loop, not Monitor re-arms: a command run with run_in_background…
- [733](#733) Show an MCP change live in Claude Code, beyond procedure [716](#716)'s headless run: put a recorder between Claude Code and…
- [729](#729) Test a Claude Code permission rule headless, A/B, before a switch and again after it. Before: Claude Code's real binary…
- [716](#716) Check an ekko build against Claude Code itself, not only tests/mcp.rs: one headless run with the build as an MCP server…
- [665](#665) Show that an ekko build costs no tokens and no speed against the installed one. Copy the board once per build into the…
- [625](#625) Test ekko's wake hook end to end in an interactive Claude Code, driven through tmux, with a scratch board.
- [602](#602) Drive ekko's menu end to end in a real Konsole window, keys typed through D-Bus. On a scratch folder B: ask a question…
- [570](#570) Read an account's 5-hour and weekly usage without the status line: run `claude -p 'Responda apenas: ok' --output-format…
- [547](#547) Read Claude Code's behaviour off its binary: the executable is bin/.claude-unwrapped in the claude-code package, a \~237…
- [523](#523) Re-run the recall and context studies of task [507](tasks/507.md) (evals/recall/ on main since 86772cd, first committed as f1f6b99 on…
- [458](#458) To try a build against this board without writing to it, run the binary from a copy of the project folder. mkdir -p…
- [382](#382) Several sessions on one repo and one board (task [352](tasks/352.md), approved by the user 2026-09-22; in force from v0.14.0). 1) No…

## <a id="994"></a>994. To know which CLAUDE.md a Claude Code session was given, read the instructions attachments in its transcript, not the…

To know which CLAUDE.md a Claude Code session was given, read the instructions attachments in its transcript, not the file's date.

Each one is a row of type attachment, with attachment.type 'instructions' and files[] holding path and content. It is written at the session's start and can come again mid-session (fa325757 had a second one 3 hours in, on 2.1.285). evals/claude-code/silence.py classifies turns this way (task [983](tasks/983.md)).

*Procedure · 2026-09-30 · on task [983](tasks/983.md)*

## <a id="953"></a>953. Test ctx's 5-hour cap without waiting for a real crossing: give a scratch session its own CTX_STATE, whose…

Test ctx's 5-hour cap without waiting for a real crossing: give a scratch session its own CTX_STATE, whose window/&lt;account> reads '95 &lt;the real window's end> &lt;seven>'.

- Never write the shared \~/.local/state/ctx/window/&lt;account>. Every session of the profile reads it, and the status line's merge keeps the highest reading of a window, so a fake 95 there stands until the window ends.
- The account is CLAUDE_CONFIG_DIR's folder name without the dot: 'claude' by default.
- Open the scratch window per procedure [782](#782), also unsetting KONSOLE_DBUS\_\*, so that the scratch session can never be pointed at the user's tab.
- For the auto-reset part: CTX_HANDOFF_TOKENS=16000, CTX_AUTO_RESET_IDLE=1, CTX_AUTO_RESET_WAIT=60 and an inert CTX_AUTO_RESET_PROMPT. Have the session start a background sleep 70 and end its turn. While it sleeps, write $CTX_STATE/handoff/&lt;sid>.written with ctx_context_tokens of its transcript.
- The no side of the refusal needs no window: feed the installed hooks/guard-window the same Agent input with CTX_STATE at 3% and at 95%.

Worked on 2026-09-29 (note [952](tasks/880.md#952)): both checks in under 2 minutes.

*Procedure · 2026-09-29 · on task [880](tasks/880.md)*

## <a id="876"></a>876. Show that a new test can say no: take the rule out of one place at a time, and demand a failure at the expected check…

Show that a new test can say no: take the rule out of one place at a time, and demand a failure at the expected check each time. Used on 862 (7 places), 859 (3) and 871 (2), on 2026-09-27.

1. Copy the files to a backup directory.
2. For each place, make a literal substitution with the strings passed through the environment, so nothing needs escaping:

   `FROM='<exact text>' TO='<mutant>' perl -0pi -e 'BEGIN{$f=$ENV{FROM}; $t=$ENV{TO}} s/\Q$f\E/$t/ or die "no match\n"' <file>`

   Perl's q{} broke on the unbalanced braces of Rust code.

3. Run only that test: `nix develop --command cargo test --locked --bin ekko <name>`. Restore the file from the backup right away, then print the first panicked line, which names the check that failed.
4. A mutation that passes means the test does not see that place: extend the test, then run the mutation again.
5. At the end, cmp every file against its backup.

*Procedure · 2026-09-27*

## <a id="875"></a>875. Time ekko MCP calls, build A against build B, over stdio from bash (task [859](tasks/859.md), decision [872](decisions.md#872)).

1. Build both with `nix develop --command cargo build --release --locked`, each copied out of target/:
   - B from the tree.
   - A after `git stash push <files>`, then `git stash pop`.
2. Copy the board once: `cp -a <repo>/.ekko <scratch>/pristine/proj/`. Writes change it, so every run gets a fresh copy of that.
3. Per run, in bash:

   ```
   export HOME=$run/home XDG_STATE_HOME=$run/state EKKO_DIR=$run/proj EKKO_TERMINAL=none
   unset EKKO_PROJECT CLAUDECODE
   coproc SRV { cd "$EKKO_DIR" && exec "$bin" --mcp 2>/dev/null; }
   exec 3>&"${SRV[1]}" 4<&"${SRV[0]}"
   ```

   - Send initialize (protocolVersion 2025-06-18) and read its reply; send notifications/initialized; make one read call to warm up.
   - Then per call: `t0=$EPOCHREALTIME; printf '%s\n' "$req" >&3; IFS= read -r line <&4; t1=$EPOCHREALTIME; echo $(( ${t1/./} - ${t0/./} ))` (microseconds, with no fork).
4. End with `exec 3>&- 4<&-; eval "exec ${SRV[1]}>&- ${SRV[0]}<&-"; wait`.

   GOTCHA: the coproc keeps its own copies of the pipes. Closed only through 3 and 4, the server never sees EOF and `wait` hangs; the first run on 2026-09-27 did.

5. Run 3 rounds of 40 calls, with A and B in alternating order per round. Drop each run's first call and compare medians per round.
6. To see from inside what the server read, run it under `strace -f -y -e trace=read -o $run/trace.txt` and sum the bytes of the reads on `storage/storage.json>`.

*Procedure · 2026-09-27*

## <a id="793"></a>793. Read a session's auto-compact window and threshold from inside, without writing settings:

- claude -p '/autocompact' --output-format stream-json --verbose, then read the result event. It prints 'Auto-compact window: &lt;N> tokens (&lt;source>)' (from settings, default for this model, or from CLAUDE_CODE_AUTO_COMPACT_WINDOW), and 'Auto-compact is currently disabled (see /config)' when off.
- claude -p '/context', read the same way. It prints 'Tokens: &lt;used> / &lt;window>', 'Free space' and 'Autocompact buffer 33k'. Compaction fires at used + free space.
- Pass the profile the same way as for a session: CLAUDE_CONFIG_DIR=\~/.claude-trabalho for trabalho, unset for default. Unset CLAUDECODE, CLAUDE_CODE_CHILD_SESSION, CLAUDE_CODE_SESSION_ID and CLAUDE_CODE_ENTRYPOINT when running it from inside a session.
- With no argument, /autocompact only reports. With one, it saves autoCompactWindow to the user's settings.json.
- Seen saying both yes and no on 2026-09-26: default read 'disabled' before task [789](tasks/789.md) and not after, and '--autocompact auto' brought back '1m tokens (default for this model)'.

*Procedure · 2026-09-26 · on task [789](tasks/789.md)*

## <a id="782"></a>782. Open a scratch Claude Code window from an agent session, for a live test of typing into Claude Code (task [747](tasks/747.md)), without…

Open a scratch Claude Code window from an agent session, for a live test of typing into Claude Code (task [747](tasks/747.md)), without touching the user's tab.

1. From /tmp/ctx-747-scratch, a folder already trusted, run: setsid -f env -u CLAUDE_CODE_CHILD_SESSION -u CLAUDE_CODE_SESSION_ID -u CLAUDE_PID -u CLAUDECODE -u CLAUDE_CODE_MESSAGING_SOCKET -u CLAUDE_CODE_MESSAGING_TOKEN -u CLAUDE_CODE_SESSION_ATTENDED -u CLAUDE_CODE_ENTRYPOINT -u CLAUDE_CODE_EXECPATH -u CLAUDE_EFFORT [test vars] konsole --separate --workdir /tmp/ctx-747-scratch -e claude --permission-mode default [--allowedTools Bash]
   - Without the unsets, the child inherits CLAUDE_CODE_CHILD_SESSION and saves no transcript.
   - --separate makes it its own process, with its own D-Bus service.
2. Its service is the org.kde.konsole-&lt;pid> in busctl --user list other than the user's; its tab is /Sessions/1.
3. Check that its foregroundProcessId is a claude whose cwd is /tmp/ctx-747-scratch.
4. Drive it with /tmp/ctx-747-kt (send/screen/tail; set S= to the service first), and read the state with ctx's ctx_screen_idle.
   - Copies of the driver kt, the idle-check prototype and the six captured screens are in \~/.local/state/ctx/747-live-test.
5. Each Enter goes in its own sendText, 0.5 s after the text. Never send Enter into a permission dialog: Esc dismisses it.
6. Close the window by killing that konsole pid.

Traps:

- The window takes focus when it opens: the user's keystrokes can land in it. The user pressed Enter in one on 2026-09-26.
- The scratch claude runs the user's hooks and plugins.

Learned 2026-09-26 \~18:10-18:30 (-03), in the end-to-end tests of ctx 0.4.0 and 0.4.1:

- A foreground sleep is refused by Claude Code (gotcha [785](gotchas.md#785)). To have the scratch session stop with the user away, have it start the sleep in the background and answer when it ends: the completion notification opens a turn with no human row.
- The scratch folder resumes the default board's stale handoff on 'continuando' (gotcha [787](gotchas.md#787)). Set CTX_AUTO_RESET_PROMPT to something inert, and allow only the test's commands.
- To trip the threshold in the first session but not in the fresh one after the reset (\~15k), set CTX_HANDOFF_TOKENS=16000 and pad the first prompt with \~6000 chars. A long sendText becomes '[Pasted text #1]', and the Enter sent apart submits it.
- The kt driver's S= line must be pointed at the new window's service each time.

*Procedure · 2026-09-26*

## <a id="755"></a>755. Wait on a long job with one background Bash until-loop, not Monitor re-arms: a command run with run_in_background…

Wait on a long job with one background Bash until-loop, not Monitor re-arms: a command run with run_in_background outlives the Bash tool's 10-minute timeout (one ran 86 min on 2026-09-26, Claude Code 2.1.283) and wakes the session once, when it exits. Read only the lines written after the loop starts -- from=$(( $(wc -l &lt; LOG) + 1 )); until tail -n +$from LOG | grep -qE 'done|Traceback' || ! systemctl --user is-active -q UNIT; do sleep 30; done -- since a grep over the whole log matches an older run's lines and exits at once. Include every terminal state in the pattern, and the unit dying.

*Procedure · 2026-09-26*

## <a id="733"></a>733. Show an MCP change live in Claude Code, beyond procedure [716](#716)'s headless run: put a recorder between Claude Code and…

Show an MCP change live in Claude Code, beyond procedure [716](#716)'s headless run: put a recorder between Claude Code and ekko (\~/.cache/ekko-paired/mcp708/recorder.sh logs both directions to $REC_LOG; REC_PASS=1 passes everything through, REC_REFUSE=1 refuses server/discover so Claude Code falls back to the handshake, the legacy control). Headless claude -p shows tools, ask through a fake terminal that answers the menu (mcp708/fake-terminal.sh, set as EKKO_TERMINAL), and the MRTR round trip. What -p does not show, an interactive claude in a detached tmux does (tmux new-session -d -s NAME -x 200 -y 50 "cd DIR && env CTX_DISABLE=1 EKKO_DIR=... claude --strict-mcp-config --mcp-config FILE --model claude-haiku-4-5-20251001"; read it with tmux capture-pane -pt NAME): -p re-reads the resource list on list_changed in neither era, and dismisses every dialog at once. A resources check needs no prompt at all -- write from the CLI and watch the recorder's log. In a dialog, Right expands a choice, Space picks, Down reaches Accept, Enter confirms; a new folder first asks for trust, 'No, exit' preselected (Down, Enter). Kill the tmux session after. Measured 2026-09-26 -03 on Claude Code 2.1.283 (task 708, note 732).

*Procedure · 2026-09-26*

## <a id="729"></a>729. Test a Claude Code permission rule headless, A/B, before a switch and again after it. Before: Claude Code's real binary…

Test a Claude Code permission rule headless, A/B, before a switch and again after it. Before: Claude Code's real binary (the .claude-wrapped the claude wrapper execs) with --settings set to a copy of the wrapper's settings file, A as is and B with the rule; after: the claude wrapper itself. For which commands a rule holds, pass --permission-mode manual --allowedTools Bash with Haiku 4.5: every command runs unless an ask rule holds it, with no classifier in the way, about US$0.02 a run (task [730](tasks/730.md)). For auto mode itself, --permission-mode auto --model claude-sonnet-5: Haiku 4.5 gets permissionMode default headless (the init event says so), where without --allowedTools every command asks. -p cannot prompt: an ask comes back as the tool result 'Claude requested permissions to use Bash, but you haven't granted it yet' and in the result's permission_denials; a classifier refusal says 'denied by the Claude Code auto mode classifier'. Put a fake program first in PATH so the commands read nothing real; the plugin starts its server and hooks as bare ekko, so the fake hands --mcp and --hook to the real binary (\~/.cache/ekko-paired/ask697/shim/ekko, run.sh, run-post.sh). Also CTX_DISABLE=1, a scratch EKKO_DIR, --no-session-persistence and &lt; /dev/null (else a 3 s wait on stdin), from a folder under \~/.cache/ekko-paired. US$0.07-0.16 a Sonnet run (2026-09-25 -03, Claude Code 2.1.283; task 697, notes 722 and 726). Measured in task [730](tasks/730.md), where the docs say only 'matches past any leading assignment': an ask rule also matches a command with its leading assignment, so Bash(EKKO_PROJECT=\*) holds EKKO_PROJECT=x ekko --list, but not env EKKO_PROJECT=x ekko nor export EKKO_PROJECT=x && ekko, which Bash(\*EKKO_PROJECT=\*) holds.

*Procedure · 2026-09-25*

## <a id="716"></a>716. Check an ekko build against Claude Code itself, not only tests/mcp.rs: one headless run with the build as an MCP server…

Check an ekko build against Claude Code itself, not only tests/mcp.rs: one headless run with the build as an MCP server and the installed one beside it as the control. From a folder under \~/.cache/ekko-paired (the transcript studies skip it), on a scratch board: env CTX_DISABLE=1 EKKO_DIR=&lt;scratch> claude -p 'Responda apenas: ok' --output-format stream-json --verbose --model claude-haiku-4-5-20251001 --no-session-persistence --strict-mcp-config --mcp-config &lt;a file naming each binary with args ["--mcp"] and env HOME and EKKO_DIR on the scratch board>. Read the init event (.mcp_servers, and the mcp\_\_&lt;name>\_\_ tools of each) and each \~/.cache/claude-cli-nodejs/&lt;folder>/mcp-logs-&lt;name>/\*.jsonl for protocolEra and 'failed'. The control has to fail where the build passes, or the run shows nothing. The wrapper adds the installed resources server to every run. About US$0.04 a run (2026-09-25 -03, Claude Code 2.1.282). Without --strict-mcp-config the run loads the real plugin and wrapper servers: the check after a switch (note 712).

*Procedure · 2026-09-25*

## <a id="665"></a>665. Show that an ekko build costs no tokens and no speed against the installed one. Copy the board once per build into the…

Show that an ekko build costs no tokens and no speed against the installed one. Copy the board once per build into the session's scratchpad (outside any repository, gotcha [646](gotchas.md#646)): mkdir -p S/&lt;v>/proj/.ekko S/&lt;v>/home; cp -r .ekko/storage .ekko/archive .ekko/project.json S/&lt;v>/proj/.ekko/. Then, from S/&lt;v>/proj with HOME=S/&lt;v>/home: (1) --prime of each build, compared with cmp; (2) initialize and tools/list piped into --mcp, their .result.instructions and .result.tools compared with jq and cmp -- that is the prefix every session pays; (3) 40 runs of --prime and 40 of --task per build, two rounds, timed with date +%s%N. Writes vary \~26-33 ms between rounds of one build, so read only differences past that; strace -T -e trace=linkat,copy_file_range,fsync isolates what one write adds. Used for v0.22.0 on 2026-09-25: all three identical, the copy \~0.2 ms a write.

*Procedure · 2026-09-25*

## <a id="625"></a>625. Test ekko's wake hook end to end in an interactive Claude Code, driven through tmux, with a scratch board.

1) In a scratch folder S, make boarddir/, state/ and cwd/.
2) Write S/settings.json. SessionStart runs &lt;bin> --prime --hook and &lt;bin> --tasklist --hook; the tasklist hook returns the watchPaths. FileChanged has matcher storage.json and runs a wrapper, asyncRewake true. The wrapper logs its input and exit code, then pipes the input to &lt;bin> --wake --hook.
3) Write S/mcp.json, serving &lt;bin> --mcp under another name (ekkonew), so it does not clash with the plugin's server.
4) export EKKO_DIR=S/boarddir XDG_STATE_HOME=S/state. Make task 1 and --begin it with CLAUDECODE unset, so the user holds it.
5) tmux -L ekkotest new-session -d -s wake "cd S/cwd && env -u CLAUDECODE EKKO_DIR=... XDG_STATE_HOME=... claude --model haiku --settings S/settings.json --mcp-config S/mcp.json --allowedTools mcp\_\_ekkonew\_\_wait".
6) Send the prompt with send-keys -l, then a separate Enter, asking the session to call wait on 1 with a text to act on. Watch capture-pane until it replies.
7) Run --check 1 from the terminal. Within about a second the pane shows 'ekko woke this session', then the reply. The wrapper's log shows exit=2 and the line it told.
8) tmux -L ekkotest kill-server.

Done on 2026-09-24 for branch wait (d8cedbf): the session woke 0.6 s after the write.

*Procedure · 2026-09-24 · on task [389](tasks/389.md)*

## <a id="602"></a>602. Drive ekko's menu end to end in a real Konsole window, keys typed through D-Bus. On a scratch folder B: ask a question…

Drive ekko's menu end to end in a real Konsole window, keys typed through D-Bus. On a scratch folder B: ask a question through `ekko --mcp` with HOME=B EKKO_DIR=B EKKO_TERMINAL=none (the reply names its uid); write a Spec {questions: [{uid, id, text, options, multiple}]} to B/menu.json; launch `konsole --separate --hide-menubar --hide-tabbar --hide-toolbars -e env EKKO_DIR=B <ekko> --menu B/menu.json` in the background, with XDG_CONFIG_HOME=B/.config and a B/.config/konsolerc holding EnableSecuritySensitiveDBusAPI=true under [General] and [KonsoleWindow]: without it sendText is refused (AccessDenied, 'Security sensitive DBus API is disabled'), and the scratch config keeps the user's own untouched. The menu is up once B/menu.pid exists; type with `qdbus org.kde.konsole-<konsole pid> /Sessions/1 sendText <text>` ($'\\r' for Enter); read the answer from B/.ekko/storage/storage.json (.question.answer.text of the item with that uid). The window takes the focus on the user's screen for a few seconds: say so first.

*Procedure · 2026-09-24 · on task [593](tasks/593.md)*

## <a id="570"></a>570. Read an account's 5-hour and weekly usage without the status line: run `claude -p 'Responda apenas: ok' --output-format…

Read an account's 5-hour and weekly usage without the status line: run `claude -p 'Responda apenas: ok' --output-format stream-json --verbose --effort low` under that profile (CLAUDE_CONFIG_DIR) in a folder of no project, and take the last line of type rate_limit_event: rate_limit_info.unifiedWindows.five_hour and .seven_day each hold utilization (0 to 1) and resetsAt (epoch seconds). One call at a \~17.5k floor, \~35k units. Claude Code 2.1.281; used by evals/paired/harness.py (ping) to pace the paired test. Found 2026-09-24, session 8c68c63f.

*Procedure · 2026-09-24*

## <a id="547"></a>547. Read Claude Code's behaviour off its binary: the executable is bin/.claude-unwrapped in the claude-code package, a \~237…

Read Claude Code's behaviour off its binary: the executable is bin/.claude-unwrapped in the claude-code package, a \~237 MB bundle of minified JS (bin/claude is a wrapper of a wrapper: readlink -f "$(command -v claude)", then the /nix/store path inside it). `strings` is not installed, and plain grep here ran as ugrep, which refused a window like .{0,200}; what worked: LC_ALL=C command grep -a -o -E '.{0,90}needle.{0,90}' "$U", and for a wider view, the offset from grep -a -b -o -F 'needle' "$U" with dd if="$U" bs=1 skip=$((off-600)) count=1400 | tr -c '[:print:]\\n' '.'. Search for user-facing strings first (a hook's error text), then for the function names around them. 2026-09-27: ripgrep is on PATH and reads the bundle as is.

- `rg -a -o -m 20 '.{0,300}needle.{0,300}' "$U"` shows a window around each match.
- `rg -a -U -o 'function NAME\(e\)\{(?s:.){0,900}' "$U"` reads on past the newlines inside template strings.
- A minified name is reused across modules; two unrelated `_c` functions were found. Read a definition beside its call site: `rg -a -o '.{0,300}[^a-zA-Z0-9_$]NAME\(.{0,300}' "$U"`.

*Procedure · 2026-09-24*

## <a id="523"></a>523. Re-run the recall and context studies of task [507](tasks/507.md) (evals/recall/ on main since 86772cd, first committed as f1f6b99 on…

Re-run the recall and context studies of task [507](tasks/507.md) (evals/recall/ on main since 86772cd, first committed as f1f6b99 on branch playground): from the repo root, nix develop -c python3 evals/recall/replay.py --out target/evals/recall. Add --embed with a local Ollama serving bge-m3 (ollama serve in the background, ollama pull bge-m3, stop the server after); its vectors are cached in target/evals/recall/embeddings.json, so a second run takes seconds instead of \~10 minutes. composition.py, thinking.py, effort.py, cold.py and quality.py read \~/.claude\*/projects/ directly; composition.py takes --since. nix develop resets the shell's folder to the repo, so give scripts absolute paths.

*Procedure · 2026-09-24*

## <a id="458"></a>458. To try a build against this board without writing to it, run the binary from a copy of the project folder. mkdir -p…

To try a build against this board without writing to it, run the binary from a copy of the project folder. mkdir -p &lt;scratch>/proj/.ekko &lt;scratch>/home; cp -r .ekko/storage .ekko/project.json &lt;scratch>/proj/.ekko/; then, from &lt;scratch>/proj, pipe an initialize and a tools/call line into HOME=&lt;scratch>/home target/debug/ekko --mcp and read the reply with jq (select(.id==2) | .result.content[0].text). EKKO_DIR pointed at a copy of storage/ reads an empty board (0 items), since a project's board is found from the folder. Used in session 55445341, 2026-09-23, to check 397's search against the real stash.

*Procedure · 2026-09-23*

## <a id="382"></a>382. Several sessions on one repo and one board (task [352](tasks/352.md), approved by the user 2026-09-22; in force from v0.14.0). 1) No…

Several sessions on one repo and one board (task [352](tasks/352.md), approved by the user 2026-09-22; in force from v0.14.0). 1) No session develops in main's working tree: each works in its own worktree on its own branch (git worktree add), and main's tree only follows origin (git pull --ff-only). 2) Work lands by fast-forward: rebase onto origin/main, cargo test and clippy, then fast-forward main and push. A rejected push means main moved, so rebase and push again. Never force a push to main, and never rewrite what is pushed. 3) One release at a time: a session claims it by setting a task 'Release vX.Y.Z' in progress, which makes any other session's change HELD. 4) Tasks are claimed with set_state progress, which records the session and its conversation, not with 'Held by' text; ekko --sessions shows who holds what. 5) A question for the user goes through ask before it is put to them, so it outlives a /clear or restart; answer records the reply, from whichever session hears it, and the answer says how far the board moved in between.

*Procedure · 2026-09-22*
