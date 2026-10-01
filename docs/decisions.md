<!-- Written by ekko docs from the board of ekko: change the board and run it again, since edits here are overwritten. -->

# Decisions

What was settled, and why: 69 in force, newest first; the 2 that later ones replaced are at the end.

- [1030](#1030) The prime's five lasting-note lines are the 3 decisions, gotchas or procedures the most visible items cite by number…
- [1009](#1009) ekko docs writes a project's documentation from its board by code, with no model, so it costs no tokens (task [915](tasks/915.md)…
- [993](#993) Narration between tool calls stays off through the 'Work silently' rule in the user's CLAUDE.md, not through an output…
- [989](#989) Nothing is cut after the audit of 2026-09-29: the user keeps the tasklist hook, the gotcha guard, and wait, away and…
- [966](#966) ekko, ctx and graff should each work alone. The handoff is ekko's; ctx only helps tell when. graff is a separate thing.…
- [962](#962) With the user there, ctx's Stop hook asks for no handoff: neither past CTX_HANDOFF_TOKENS nor past the 5-hour cap (ctx…
- [951](#951) No cue for cargo fmt --all in winwayland: cancelled 2026-09-29 at the user's word ('pode cancelar'). winwayland is an…
- [943](#943) ctx's cold-return guard is off by default since ctx 0.9.0 (2026-09-29), the user's call on task [929](tasks/929.md) (ask 938), amending…
- [916](#916) The MCP tool move_to moves at once only where a session already writes; anywhere else it moves once the user allows it…
- [910](#910) A moved cue that names no folder guards the board it went to; no folder is written out (c46f685, 2026-09-28).
- [901](#901) --move-to (task [897](tasks/897.md), ddb73f4): a move keeps the item whole and never drops a link silently.
- [898](#898) The project's memory is memory.md in the board's folder, read by its own SessionStart hook (`ekko --memory --hook`) and…
- [888](#888) No 'not modified' answer for context or search: sessions do not re-read. Measured 2026-09-28 with…
- [881](#881) 808 is live since 2026-09-27 22:33:16 (-03), as designed in decision [863](#863). The go-live is recorded on 540, note [864](tasks/540.md#864).…
- [872](#872) A write keeps the map it wrote as the version it made: measured first, as 859 asked, and kept (commit 35de338, local…
- [870](#870) A conversation Claude Code moves to a background session keeps its work: ekko reads the move off the `continued-in`…
- [863](#863) The 5-hour cap, v1: 808's four open points and the design, settled by the agent under decision [856](#856) (automatic mode)…
- [858](#858) A read that changes nothing takes Storage::get_shared; get() is only for a working copy to change, and copies the…
- [857](#857) The enums a board stores are open: Knowledge, Until and How read a word they do not know into Unknown(&'static str)…
- [856](#856) Automatic mode for the chain 845 -> 844 -> 811 -> 808, at the user's word in chat, 2026-09-27 \~13:25 (-03): 'encadeia a…
- [847](#847) A cue lives only on a board the guard reads: the default board or a registered project's (task [827](tasks/827.md), commit 1a437e1).…
- [846](#846) Every struct ekko reads from a file and writes back keeps the fields it does not know, in a `#[serde(flatten)] unknown…
- [823](#823) 805's design settled by the user on 2026-09-26 (questions 820 to 822, research in note 819).
- [810](#810) ekko, ctx and graff are built for Claude Code and nothing else. The user, 2026-09-26, in session 01215f07: 'já deixando…
- [801](#801) Bypass guards settled by the user on 2026-09-26 (questions [798](tasks/770.md#798) to 800, task [770](tasks/770.md)): bypass stays on, and the guards are…
- [792](#792) Claude Code's compaction, the safety net under ctx's handoff, fires at \~417k: autoCompactWindow 450000 with…
- [765](#765) Decided by the user, 2026-09-26 \~17:15 (-03): a session left alone goes through the same handoff ritual, made automatic…
- [762](#762) The handoff practice stays, on the measure of its first 5.6 days (note [761](tasks/757.md#761), 2026-09-26): against the baseline week, the…
- [748](#748) Decided by session 83c87811 (first written as 0dca8739, the session it cleared), 2026-09-26 \~01:10 (-03), on the user's…
- [744](#744) Decided by the user, 2026-09-26 \~01:00 (-03), question [743](tasks/259.md#743): 259's three long runs go on the default account at once…
- [741](#741) Decided by the user, 2026-09-26 -03, question [740](tasks/259.md#740) (the recommended option): 259's three long runs -- 396 in C/max twice…
- [676](#676) Decided by the user, 2026-09-25, questions [670](tasks/431.md#670)-675 (every recommended option), after the re-measure on 2.1.282…
- [663](#663) Decided by the user, 2026-09-25, in chat ('sim, fecha a 496 como ferramenta pessoal'), after decision [637](#637): ekko is the…
- [637](#637) Decided by the user, 2026-09-25, question [635](notes.md#635) ('ekko não precisa rodar em outros agentes, somente no claude code, pode…
- [614](#614) The default effort stays max, in \~/NixOS for both profiles. Decided by the user on 2026-09-24 (question [611](tasks/526.md#611)), from the…
- [590](#590) The question menu in a window of its own is a feature to keep, not a host limit to work around. The user, 2026-09-24…
- [587](#587) Memory types settled by the user, 2026-09-24 (questions [583](tasks/337.md#583) to 585, closing 404). Of note [338](tasks/337.md#338)'s seven ideas, three are…
- [563](#563) Decided by the user, 2026-09-24, session 8c68c63f (questions [553](tasks/526.md#553)-555; 'agora ficou claro, aprovado, pode seguir…
- [537](#537) Decided by the user, 2026-09-24, questions [532](tasks/525.md#532)-535 ('siga os 4 recomendados, aprovado'): (1) 86772cd, evals/recall/, is…
- [527](#527) Decided by the user, 2026-09-24 (task [507](tasks/507.md) cancelled at 'pode continuar', session 844752fe): ekko does not inject board…
- [511](#511) Provenance, as built for 125 and 396 (the user chose each part in questions [504](tasks/396.md#504), [505](tasks/396.md#505) and [506](tasks/396.md#506), 2026-09-24). WHO WROTE AN…
- [486](#486) The menu's extras stay, and its look: the user's answers in ekko's own menu on its real check (2026-09-24, session…
- [484](#484) ekko's menu covers what AskUserQuestion had, in one release, before AskUserQuestion goes back into the deny (the user…
- [482](#482) Questions reach the user through ekko's own menu, not Claude Code's (the user's choice through AskUserQuestion…
- [452](#452) Who a task is with, as built for 87 (the user approved it on 2026-09-23 in question 451: 'ta, beleza pode aplicar').…
- [449](#449) A handoff that a later one replaces is recorded in supersedes, the field decisions use: ekko sets it on the new handoff…
- [430](#430) Decided by the user, 2026-09-23, question [422](tasks/422.md) (from task [416](tasks/416.md), note [417](tasks/416.md#417)): option (a). Measure what each part of every…
- [428](#428) Decided by the user, 2026-09-23, question [421](tasks/421.md) (from task [416](tasks/416.md), note [417](tasks/416.md#417)): option (a). What must outlive a handoff goes…
- [427](#427) Decided by the user, 2026-09-23, question [420](tasks/420.md) (from task [416](tasks/416.md), note [417](tasks/416.md#417)): option (a). ctx's handoff text, the Stop…
- [425](#425) Decided by the user, 2026-09-23, question [419](tasks/419.md) (from task [416](tasks/416.md), note [417](tasks/416.md#417)): option (a). A replaced handoff shows in…
- [423](#423) Decided by the user, 2026-09-23, question [418](tasks/418.md) (from task [416](tasks/416.md), note [417](tasks/416.md#417)): option (a) with (b). ctx's status line shows…
- [410](#410) Decided by the user, 2026-09-23, replacing decision [372](#372), after session 06637eec explained how auto memory works (per…
- [343](#343) Owners, as built (decided 2026-09-22, session 9e449e1b, the user having left it to the session). WHO: the Claude Code…
- [327](#327) The handoff trigger belongs to ctx, not ekko (the user, 2026-09-22). ctx (github.com/thiagoproldan/ctx, created…
- [323](#323) --copy and --since stay (the user, 2026-09-22: 'o resto como --copy e --since sao importantes'), although neither shows…
- [317](#317) rustfmt stays off: the repo is not reformatted and CI does not check formatting (decided 2026-09-22, session 9e449e1b).…
- [314](#314) A stale server warns, it does not refuse, and it tells by its binary rather than by a version fence (decided…
- [313](#313) The blocking weight in next's urgency is 1, not Taskwarrior's 8 and not 0 (decided 2026-09-22, session 9e449e1b, the…
- [250](#250) Decided by the user on 2026-09-21, answering three questions after task [239](tasks/239.md). (1) Commit the removal of --ui: done…
- [243](#243) Cancelled 2026-09-21 along with task [239](tasks/239.md), which removes --ui. WHY IT DIED. The user barely used the interactive mode…
- [240](#240) Approved by the user on 2026-09-21: 'adiciona isso ao board e faz o handoff, vamos fazer isso na nova session, mas…
- [236](#236) Validated and approved by the user on 2026-09-21 ('valide a opção 1 antes', then 'sim, pode fazer'). RULE: when the…
- [232](#232) Waiting state, approved by the user on 2026-09-21 ('sim, pode seguir') as proposed. (1) A sixth state, waiting, beside…
- [229](#229) Every ekko tool is allowed in Claude Code's permissions, and trash and force_state ask. This is set in \~/NixOS usr.ekko…
- [227](#227) A bare 'continue' after /clear means the handoff's next step at its planned size, and stopping where it says to ask. A…
- [204](#204) Approved by the user on 2026-09-21 for task [202](tasks/202.md). (1) CLAUDE_CODE_ENABLE_TODO_TOOLS=1 and…
- [163](#163) Second pass on the model behind this task, 2026-09-15 -- it corrects the framing, not the direction.
- [162](#162) Study 2026-09-15: token proxies and compressors (rtk, headroom, caveman) against a critique (Rakuen, 'Token compression…
- [161](#161) Why this sits above everything in the rigor plan. Measured 2026-09-15 on the Claude Code transcripts of every profile…

## <a id="1030"></a>1030. The prime's five lasting-note lines are the 3 decisions, gotchas or procedures the most visible items cite by number…

The prime's five lasting-note lines are the 3 decisions, gotchas or procedures the most visible items cite by number, then the newest gotchas and procedures by creation, and never anything ranked by updatedAt. This is the user's pick in question [1024](tasks/1021.md#1024), shipped in v0.32.0.

- On the sessions of 2026-09-15 to 10-01 it recalled 46% of the lasting notes they fetched, against 24% for the newest five.
- A citation of a superseded note counts for the note at the end of its line. A rewritten procedure keeps its place this way.
- Code: lasting_shown and cited_ids in src/agent.rs.
- Revisit by replaying turns with target/evals/slots/arms.py; do not change it by feel.

*Decision · 2026-10-01 · on task [1020](tasks/1020.md)*

## <a id="1009"></a>1009. ekko docs writes a project's documentation from its board by code, with no model, so it costs no tokens (task [915](tasks/915.md)…

ekko docs writes a project's documentation from its board by code, with no model, so it costs no tokens (task [915](tasks/915.md), v0.30.0).

- The pages: decisions.md, gotchas.md and procedures.md, a page per kind; history.md; a page per task under tasks/; notes.md for the notes on no task; and index.md, which opens with the memory page. This layout was the user's choice in question [997](tasks/915.md#997).
- Where they go: the project's docs/, or a folder named, which the default board needs (question [998](tasks/915.md#998)).
- What it overwrites: it overwrites or removes only files whose first line carries its mark. A run that finds another file in its way writes nothing (NOT_GENERATED).
- How text is handled: the text of tasks and notes is escaped for GitHub's markdown, outside code spans and fenced blocks. A cited id that is on the board becomes a link to where that item is written.
- Stashed and trashed items are left out.

Why: research notes [995](tasks/915.md#995) and [996](tasks/915.md#996). A board's links are already explicit, so no model is needed to build them (unlike graphify on prose), and note 173 keeps models out of ekko. The record of the tasks, about 830k characters on ekko's board, does not fit one page. A project's docs/ may hold pages written by hand.

2026-10-01 (task [1004](tasks/1004.md), the user's answer to question 1010): an item on the board @private, in any case, is left out, and so is every note on a task there. It stays on the board, in the prime and in search, and nothing in the docs links to it. To publish a board's docs, put on @private what must not go public, then read the diff before the commit.

*Decision · 2026-10-01 · on task [915](tasks/915.md)*

## <a id="993"></a>993. Narration between tool calls stays off through the 'Work silently' rule in the user's CLAUDE.md, not through an output…

Narration between tool calls stays off through the 'Work silently' rule in the user's CLAUDE.md, not through an output style or a setting (task [983](tasks/983.md)).

Measured in note 992: narrated nudges fell from 48% to 0 of 23, and interim texts from 2.53 to 0.08 per turn. No setting turns Claude Code's nudge off (note [990](tasks/983.md#990)), so the rule must keep naming it.

*Decision · 2026-09-30 · on task [983](tasks/983.md)*

## <a id="989"></a>989. Nothing is cut after the audit of 2026-09-29: the user keeps the tasklist hook, the gotcha guard, and wait, away and…

Nothing is cut after the audit of 2026-09-29: the user keeps the tasklist hook, the gotcha guard, and wait, away and the wake hook.

Answered by the user in ekko's menu on 2026-09-30 (questions [985](tasks/944.md#985)-988, the first asked with explain, recommended, why and example, task [979](tasks/979.md)). The session recommended removing the tasklist hook, replacing the guard with permissions.deny and cutting wait/away/wake now; the user chose Manter on all three. On the readme: trim it together with the cuts, so a section leaves in the same commit as the feature it describes, when a feature is cut. The audit's numbers (note 947) and recommendation (note 950) stay as the record, not as a plan.

WHY wait, away and wake stay (the user, 2026-09-30): they exist for working with more than one Claude account. Zero MCP calls in the audit does not make them unused: the audit counted calls, not the case they cover. Do not propose cutting them again from call counts alone.

*Decision · 2026-09-30 · on task [944](tasks/944.md)*

## <a id="966"></a>966. ekko, ctx and graff should each work alone. The handoff is ekko's; ctx only helps tell when. graff is a separate thing.…

ekko, ctx and graff should each work alone. The handoff is ekko's; ctx only helps tell when. graff is a separate thing. Not now: the focus is closing ekko's board.

The user's word on 2026-09-29 ('handoff é do ekko, ctx só ajuda a saber quando, graff tbm é outra coisa a parte, o ideal aqui é cada um dos três ser independente, mas por hora não vamos fazer isso').

ctx helps with the handoff's timing: the threshold, the status line's flag, and the reset of a session left alone.

State measured that day, from the code:

- ekko needs nothing from ctx: its flake takes only nixpkgs and flake-utils, and it reads no CTX\_\* variable nor ctx's state.
- ctx needs ekko for three things. A guard's refusal lifted once goes through 'ekko --guard --refuse' (src/lib/guards.py). The handoff's age and auto-reset need hooks/handoff-written, which sees only ekko's create and batch.

Where they are coupled today:

- /handoff is a skill of ctx's (src/skills/handoff), not of ekko's.
- NixOS's usr.ctx module also carries Claude Code's auto-compaction (autoCompactWindow 450000; both profiles' settings.json have it off) and graphify, which nothing else installs.

*Decision · 2026-09-29*

## <a id="962"></a>962. With the user there, ctx's Stop hook asks for no handoff: neither past CTX_HANDOFF_TOKENS nor past the 5-hour cap (ctx…

With the user there, ctx's Stop hook asks for no handoff: neither past CTX_HANDOFF_TOKENS nor past the 5-hour cap (ctx 0.10.0, a6e6aff, task [961](tasks/961.md)). The user's word on 2026-09-29 ('pode fazer a sua proposta'), after 'esses handoffs do nada atrapalhando'.

- The status line flags '⚑ handoff' (and '5h N% ⚑ handoff'), and the user runs /handoff and /clear between tasks, where Claude Code's docs put a /clear.
- Only a session left alone (Konsole, cli, nothing typed for CTX_AUTO_RESET_IDLE minutes, past the threshold) is asked, and auto-reset clears it.
- No ctx text tells the model to write a handoff by itself: past the cap, the rule is to finish the step at hand and start nothing new.
- Claude Code's compaction (\~417k) stays the net (decision [792](#792)).
- This extends decision [423](#423), which kept the first ask and rejected only the re-asks.

Why: of 30 asks with the user there over 2026-09-23..29, 18 were followed by a prompt that went on with the work, and 12 ended the session. Each spared ask is logged as 'handoff-quiet' in \~/.local/state/ctx/handoff.jsonl, once a band, to keep the count.

*Decision · 2026-09-29 · on task [961](tasks/961.md)*

## <a id="951"></a>951. No cue for cargo fmt --all in winwayland: cancelled 2026-09-29 at the user's word ('pode cancelar'). winwayland is an…

No cue for cargo fmt --all in winwayland: cancelled 2026-09-29 at the user's word ('pode cancelar'). winwayland is an old project that will not come back to this machine, so the cue has no folder to guard. Nothing in ekko depended on it: the cue mechanism itself is done (805). Cancelling it drops 841 from 915's blockers; 915 still waits on 880.

*Decision · 2026-09-29 · on task [841](tasks/841.md)*

## <a id="943"></a>943. ctx's cold-return guard is off by default since ctx 0.9.0 (2026-09-29), the user's call on task [929](tasks/929.md) (ask 938), amending…

ctx's cold-return guard is off by default since ctx 0.9.0 (2026-09-29), the user's call on task [929](tasks/929.md) (ask 938), amending decision [537](#537)'s 250k default. Why: since 0.8.0's auto-reset there were 0 cold rewrites of 100k+ (note [937](tasks/929.md#937)), and where the guard stopped a prompt it saved nothing: 0 of 2 ended in /clear (note [922](tasks/540.md#922)). The code stays; CTX_COLD_TOKENS=250000 turns it back on. Task [941](tasks/941.md) re-counts from 2026-10-13 and reopens it if cold rewrites of 100k+ with a typed prompt come back from sessions auto-reset cannot reach.

*Decision · 2026-09-29 · on task [929](tasks/929.md)*

## <a id="916"></a>916. The MCP tool move_to moves at once only where a session already writes; anywhere else it moves once the user allows it…

The MCP tool move_to moves at once only where a session already writes; anywhere else it moves once the user allows it (858455f, v0.28.0; the user's choice, 907).

- Like GitHub (write access to both repositories) and Jira (permission on both projects): a session's permission on another board is the link. So a move goes at once to the session's own board or to a linked one.
- Any other destination, including the default board, which links to none, answers NOT_LINKED with a code. The code comes from guard::gate, which records the call as a guard's refusal. The session asks with allow, and the user's "Allow once" in ekko's menu lets that exact call through once: same folder, same session, within 24 hours.
- The server sees no tool use id, so each call gets its own ("mcp:&lt;uid>"). A fixed id would let the same call through again: that mutation passed the "once" check.
- There is no force over MCP. A refusal for what a running session watches names --force as the user's, in a terminal. A gotcha whose cue is on stays the user's to move (CUE_IS_USERS).

*Decision · 2026-09-28 · on task [909](tasks/909.md)*

## <a id="910"></a>910. A moved cue that names no folder guards the board it went to; no folder is written out (c46f685, 2026-09-28).

- The user first kept the pinned folder (908), then left the rule to the data, for fear of what removing boards and projects would do to it.
- The data: the one cue that is on, on any board here, is 832, which names / of its own, so a move keeps it either way.
- Why not pin: a path written out outlives its board. The guard reads only the default board and registered projects (guard.rs, boards), and --destroy forgets a project, so a cue pinned to / on a project stops guarding the machine when that project goes. One pinned to a project's folder guards a place no board stands for once the folder moves.
- So the cue goes as it is, and the reply names each cue that is on and now guards another place: "3's cue now guards &lt;folder>, not the whole machine", and "cue": {guarded, guards} in --json. A cue with a folder of its own keeps it.

*Decision · 2026-09-28 · on task [897](tasks/897.md)*

## <a id="901"></a>901. --move-to (task [897](tasks/897.md), ddb73f4): a move keeps the item whole and never drops a link silently.

- The item keeps its uid, dates, author, state, holders and text. Only its display id and its revisions (rev, question.rev, answer.rev, wait.rev, over.rev) belong to the destination and are restamped there. A task takes every note attached to it.
- A uid link between an item that moves and one that stays is refused as SPLIT_LINKS, never dropped. The board would drop it without a word: a missing blocker blocks nothing, and an older decision becomes current again.
- The source keeps moved.json: one redirect per id that left, never pruned. An item that comes back keeps its stacked redirects, as Jira stacks keys; pruning them lost history in both lookups and changes.
- move_to.rs destructures every field of Item and the structs it holds (arriving, references), so a new field does not compile until it is placed there.

Research and the full design: note [900](tasks/897.md#900).

*Decision · 2026-09-28 · on task [897](tasks/897.md)*

## <a id="898"></a>898. The project's memory is memory.md in the board's folder, read by its own SessionStart hook (`ekko --memory --hook`) and…

The project's memory is memory.md in the board's folder, read by its own SessionStart hook (`ekko --memory --hook`) and shipped in v0.27.0 (2026-09-28). The user keeps the page, and a session proposes changes to it in chat. It is capped at 6,000 characters, cut at a line with a notice. It is a hook of its own because Claude Code's 10,000-character cap counts per hook. It replaced the trial's CLAUDE.local.md import, now deleted. Live since the 17:49 switch: a new session in this project gets the page through the installed plugin, and one on a board without a page does not. Its gain, one orientation call or more saved per session, is an estimate. After some days of use, the orientation of sessions can be compared in the transcripts (evals/claude-code/transcripts.py) before and after 17:49. Not built: proposals through ask; a task of its own if the user wants it.

*Decision · 2026-09-28 · on task [885](tasks/885.md)*

## <a id="888"></a>888. No 'not modified' answer for context or search: sessions do not re-read. Measured 2026-09-28 with…

No 'not modified' answer for context or search: sessions do not re-read. Measured 2026-09-28 with evals/claude-code/rereads.py over the 433 transcripts of both profiles (paired runs skipped):

- ekko's reads came to 2,185,658 chars, 5.9% of every tool result;
- a call repeated inside one conversation, since its last compaction, got the same answer 3 times, 460 chars in all;
- context calls naming an item already given, partial overlaps included: 11 calls, at most 24,555 chars, 1.1% of ekko's reads.

The in-memory side already exists: storage.rs keeps each parsed board by file version (859). So 884 builds nothing. ekko's own speed was not measured here; if a read is ever slow, procedure [875](procedures.md#875) times it, and the task can be reopened.

SPEED, measured the same day (20 runs each, board 1,529,368 bytes): process start 2 ms; `ekko --guard --hook` on a Bash call 9 ms, since it reads its own index (\~/.ekko/guard/index.json, 1.2 KB) instead of the board; a cold read of the whole board (`ekko --json`) 38 ms. The hooks that start cold and read the whole board (prime and tasklist at session start, tasklist and wake after each write) cost about that each: an index like the guard's would save tens of ms that nobody feels. Worth it only if the board grows several-fold (the parse grows with the file).

*Decision · 2026-09-28 · on task [884](tasks/884.md)*

## <a id="881"></a>881. 808 is live since 2026-09-27 22:33:16 (-03), as designed in decision [863](#863). The go-live is recorded on 540, note [864](tasks/540.md#864).…

808 is live since 2026-09-27 22:33:16 (-03), as designed in decision [863](#863). The go-live is recorded on 540, note [864](tasks/540.md#864). Checked from the side that consumes it, at the user's word 'pronto, valida agora' (\~22:33), in the sessions started after the 22:14 switch:

a) The reading. \~/.local/state/ctx/window/claude-trabalho was written by the first 0.8.0 status line at 22:33:22 and read '45 1790559600 50'. It read '48 1790559600 50' at 22:35:17. resets_at 1790559600 is 22:40:00, in the future when read. At 22:36:06 the status line of a session still on 0.7.0 recorded 48% for the same account, the same five.

b) The model is told. Session 691db1f4 (zettelkasten, pts/6) got 'ctx: 5h 45% until 22:40, cap 85% · 7d 50%.' as additional context:

   - from SessionStart at 22:33:55;
   - again from UserPromptSubmit with the user's prompt at 22:34:43.

   Band 0, so no rule followed it. \~/.local/state/ctx/window/told/ holds band 0 for 8afebca4, da2567e6 and 691db1f4.

c) The cap. Not reachable live: the window was at 45-48%. The installed ctx-0.8.0's ctx-test passes, 471 ok and 0 failures. Its window guard cases:

   - allow a subagent and claude -p at 84%;
   - at 85%, deny a subagent, Task, a new cron, a wakeup, and claude -p or --print, also after cd, under timeout and inside bash -c;
   - allow the call the user lets through in ekko's menu;
   - log refusals.

   The prompt hook holds a loop_wakeup or schedule_wakeup prompt past the cap and lets user, system, sdk and absent sources through. Auto-reset types /clear without 'continuando' past the cap. Task [880](tasks/880.md) watches for the first live case.

Also found:

- The new sessions run ekko 0.26.1 (both servers).
- This conversation's own process, 1144255 (background session a7ab82f1, 16:01), survived the user's /exit and /resume. It kept ctx 0.7.0 and ekko 0.25.0/0.25.1, as did the default profile's 1170029 (16:25).
- The old terminal, 1047920, exited at 22:33:14 and appended only last-prompt and cost-state after 33fb3219's continued-in record: no message, so the continuation stands under 862's rule.

*Decision · 2026-09-27 · on task [808](tasks/808.md)*

## <a id="872"></a>872. A write keeps the map it wrote as the version it made: measured first, as 859 asked, and kept (commit 35de338, local…

A write keeps the map it wrote as the version it made: measured first, as 859 asked, and kept (commit 35de338, local, not pushed).

The version is taken from the temp file's fstat after sync and before the rename. The rename carries the inode, mtime and length over, and a write by any other process is a new inode, so it is parsed. The map is cloned, since save_against uses it after the write.

Measured on a copy of this board (865 items in storage.json, 1.48 MB), A = a98119a and B = this change, both release builds, driven over stdio by a bash coproc:

- MCP set_state: 3 alternating rounds of 40 calls. Medians A 30.6 / 31.4 / 31.8 ms, B 29.6 / 29.0 / 29.9 ms; overall 31.4 -> 29.3.
- storage.json read by the server over 10 writes, by strace -y: A 11 times (16.3 MB), B once (1.5 MB), the first read.
- A CLI write reads nothing after it, so it only pays for the clone. No difference: A 15.1-15.3 ms, B 14.8-15.3 ms, 3 rounds of 30.
- The unit tests of the binary ran in 8.0 s instead of 15.7 s.

The condition 859 named -- a parse of the file gives back the map written -- is now stated by a test, a_parse_of_the_written_board_gives_back_the_map_written. It covers each kind of item a session writes, a question with its answer, a wait, stash, trash, a phase, and a field from a later version. On this board, re-serializing the parse was byte-identical to the file and the map equal (a throwaway test, not committed).

set_then_get_round_trips now parses the file itself: its get() is answered from what the write kept and no longer tests the parse. Three mutations each fail a test: keeping nothing, taking the version before write_all, and a priority read back as n+1.

*Decision · 2026-09-27 · on task [859](tasks/859.md)*

## <a id="870"></a>870. A conversation Claude Code moves to a background session keeps its work: ekko reads the move off the `continued-in`…

A conversation Claude Code moves to a background session keeps its work: ekko reads the move off the `continued-in` record in the old transcript (task [862](tasks/862.md), commit a98119a, local, not pushed).

Checked first, as 862 asked:

- SessionStart's input under source fork does not name the parent. Read off the Claude Code 2.1.283 bundle: session_id, transcript_path and the common fields, source, agent_type, model, session_title, plus the resume fields (seconds_since_last_response, context_tokens, prompt_cache_likely_expired, estimated_cache_write_usd).
- Process 1047920 only launched the continuation. It still runs, as the terminal on pts/0, with its v0.25.0 MCP servers. Its registry record still says 33fb3219, since no SessionStart has fired in it since 12:49. The conversation runs in 1144255, under the daemon's pty host.
- Claude Code writes the record after its spawn of the new session succeeds, and only for a move (not with keepParent, a fork that stays). It reads the record back from the end, and a completed message after it cancels it. Measured once: the record at 19:01:37.219Z, the fork's SessionStart hook at \~19:01:38.9Z. The order is not guaranteed, and a warm spare session (--bg-spare) may start faster.

The rule, Actor::continues: this session's conversation carries on the one the holder's process ran last, or the one the claim was made in. The transcript sought is the one beside this session's own, as Claude Code itself looks; the SessionStart hook now records transcript_path in $XDG_STATE_HOME/ekko/processes. It applies wherever the resume rule already did, and at the two places a SessionStart that ran first needs it:

- take_back at SessionStart (the claims rewritten);
- Holder::whose (handoffs);
- the prime's and wake's `mine` (waits, and answers to questions);
- held_elsewhere (the HELD refusal);
- the claim notice of a structured write.

Evidence:

- One test runs the whole class against two controls: a fork with no record, and a record followed by a message. Removing the rule from any of the seven places fails it at the expected check.
- A/B on a copy of this board with the real transcripts, 808's claim rewritten to 1047920/33fb3219. For conversation a7ab82f1, 0.26.0 refuses `--set @808 paused` ("still running") and this build pauses it, and the fork's SessionStart hook now says '808 was held by this conversation until Claude Code moved it out of its process (trabalho on pts/0 · 33fb3219): yours again'. Conversation 4f0e8b16, which did not continue it, is refused by both builds and takes nothing.
- Cost: the prime opens 31 transcript ends more, at 16 KB each; 21 ms -> 22 ms per prime, in 3 rounds of 20, with the same output.

*Decision · 2026-09-27 · on task [862](tasks/862.md)*

## <a id="863"></a>863. The 5-hour cap, v1: 808's four open points and the design, settled by the agent under decision [856](#856) (automatic mode)…

The 5-hour cap, v1: 808's four open points and the design, settled by the agent under decision [856](#856) (automatic mode), 2026-09-27.

1. Refusal or warning past the cap. Both, by what is at stake.
   - Refused at PreToolUse, with ekko's allow-once as in ctx 0.7.0 (decision [801](#801)):
     - the work a session starts on its own: subagents (Agent, Task);
     - new schedules (CronCreate, ScheduleWakeup);
     - Bash calls that run headless claude (claude -p or --print).
   - Blocked at UserPromptSubmit: a scheduled prompt (source loop_wakeup or schedule_wakeup). A recurring one fires again after the window resets.
   - Auto-reset types /clear and not 'continuando': a cleared session idles for free, while a big one idling costs a cold rewrite on the user's return.
   - Warned, not refused: the user's own prompt goes through, with the reading and the rule that the reserve is theirs.

   Why: a warning alone depends on the model complying, and the ledger holds 4 handoff asks at 87-98% out of 19. Refusing everything would take the reserve from the user too.

2. One variable, CTX_HANDOFF_5H (85). The handoff ask and the cap are the same moment, and a second variable would reopen the gap 808 closes: a handoff asked while work goes on.
3. The week gets no cap. Its percentage goes in the line the model reads. A weekly cap at 85% would stop work for days, and the user asked about the 5-hour window.
4. A scheduled prompt tells itself from the user's by the source field in UserPromptSubmit's input. Claude Code 2.1.283 sends user, sdk, system, loop_wakeup, schedule_wakeup or poll_event (read off its bundle). It says payloads may omit the field while it rolls out; a prompt without it counts as the user's.

   cold-return's .scheduled mark stays as it is until 540 has counted, and so do 809's prices.

Design:

- The reading moves from each session to each account: $CTX_STATE/window/&lt;profile dir>, merged per window (resets_at), keeping the higher percentage.
  - Usage never falls within a window, and an idle session's status line repeats the last number its process saw.
  - A reading at or past the cap stands until its window ends, whatever its age. Session start has a reading before the session's own status line runs.
- The model is told:
  - at session start and with each prompt, one line: '5h 62% until 21:40, cap 85% · 7d 48%';
  - mid-turn, at PostToolUse, once, when the window crosses 70% or the cap.
- Not seen: a script that starts claude by itself, such as ekko's evals/paired/harness.py.

*Decision · 2026-09-27 · on task [808](tasks/808.md)*

## <a id="858"></a>858. A read that changes nothing takes Storage::get_shared; get() is only for a working copy to change, and copies the…

A read that changes nothing takes Storage::get_shared; get() is only for a working copy to change, and copies the version get_shared keeps (task [844](tasks/844.md), commit 0b8a299, 2026-09-27).

- Why: get() parsed whenever its version was not kept, and only get_shared kept one, so a CLI command parsed the board once per read: --list pending 5 times, the board view 4, --find 3, --task 4 (strace, on a 1,325-item copy). Now each view parses once and a write twice, once on either side of it.
- Views copy only what they show (Ekko::board_where); a write's `before` (save_touching_forced, take_back, the MCP Draft) is the shared Arc itself, not a copy.
- The rule is kept by storage::tests::only_a_copy_to_change_is_taken_with_get: every storage.get() in the source binds `let mut`; anything else is named as a read.
- Measured against 2f4f292, interleaved: --list pending 129.8M -> 39.7M instructions, 51.9 -> 30.7 ms; --task [122](tasks/122.md).6M -> 80.0M, 70.1 -> 60.5 ms; board view 54.6 -> 44.5 ms.
- Tried and dropped: get() copying from the cache with every read still on get(). Reads gained as much, a write nothing (user 25 -> 22 ms, sys 26 -> 29 ms), since each view after the write copied the whole board.
- The MCP server's reads were already cached (get_shared); each write still parses once after it: task created alongside.

*Decision · 2026-09-27 · on task [844](tasks/844.md)*

## <a id="857"></a>857. The enums a board stores are open: Knowledge, Until and How read a word they do not know into Unknown(&'static str)…

The enums a board stores are open: Knowledge, Until and How read a word they do not know into Unknown(&'static str), and write it back as read (task [845](tasks/845.md), commit 2f4f292, 2026-09-27).

- Why: serde's derived reader refused a later version's variant, and with it the whole board, for every command and the MCP server. The alternative in the task, shipping each new variant with a reader one version ahead, leaves an MCP server started before the upgrade just as stuck (gotcha [194](gotchas.md#194)). Open enums are how proto3 and the AWS and Stripe SDKs handle it.
- What Unknown means, only what this version can vouch for: a note of an unknown kind shows its word ('note, a lesson') and counts as none of the known kinds (no cue, not in the prime's lists); a wait for an unknown condition ends only when its item leaves the board (How::Removed); a wait ended in an unknown way is over, and its session is told ('ended the wait, in a way only a later version of ekko names').
- The word is interned (leaked once per distinct word, fn kept), so the enums stay Copy and word() stays &'static str, which every view takes.
- Rule over the class, kept by tests in item.rs: no enum outside ops.rs derives Deserialize (a source scan); the stored ones go through stored_as_words!(Until, How, Knowledge); every known value reads back as itself under the word the derived writer used; an unknown word in each of the three places comes back where it was. A new stored enum: add it to the macro, give it word(), parse() and Unknown, and list it in those tests.
- Measured: v0.25.1 refuses a board with one note's kind set to 'lesson'; this build reads, shows and rewrites it. --list pending 33.6-34.9 ms with either build, back to back.

*Decision · 2026-09-27 · on task [845](tasks/845.md)*

## <a id="856"></a>856. Automatic mode for the chain 845 -> 844 -> 811 -> 808, at the user's word in chat, 2026-09-27 \~13:25 (-03): 'encadeia a…

Automatic mode for the chain 845 -> 844 -> 811 -> 808, at the user's word in chat, 2026-09-27 \~13:25 (-03): 'encadeia a sua ordem sugerida e ataca todas em modo automatico'.

- Take the tasks in that order without asking between them. A session that starts from a handoff of this chain with 'continuando' (ctx's auto-reset) carries on with it.
- The design points each task leaves open (845's two options, 811's six, 808's four) are settled by the agent: read what the board and the code already hold, research how others solve it, record a decision on the task, and list those decisions in the final report so the user can overturn them.
- Not covered: git push, a release, its GitHub page and the NixOS switch still wait on the user's word in chat (procedure 234, gotcha [850](gotchas.md#850)). Commit locally, keep each commit green, and ask at the end.
- 540 keeps its date (2026-10-08) instead of a link behind 808: it is a measurement over time, and 809 stays behind it.

*Decision · 2026-09-27*

## <a id="847"></a>847. A cue lives only on a board the guard reads: the default board or a registered project's (task [827](tasks/827.md), commit 1a437e1).…

A cue lives only on a board the guard reads: the default board or a registered project's (task [827](tasks/827.md), commit 1a437e1). ask with cue refuses, with INVALID_INPUT, to propose one on a board opened elsewhere through EKKO_DIR or --ekko-dir, since it would never refuse a call. Turning a cue off stays allowed everywhere.

Reading EKKO_DIR boards was set aside, for two reasons:

- Nothing registers such a board, so the hook would see it only in sessions started with the variable.
- The one index every session shares would change with whichever session ran last. A cue's effect would then depend on history.

If the user ever keeps a real global board through EKKO_DIR (the readme's "home and work"), the way back is to register such boards persistently, as projects are, and have boards() read the list.

*Decision · 2026-09-27 · on task [827](tasks/827.md)*

## <a id="846"></a>846. Every struct ekko reads from a file and writes back keeps the fields it does not know, in a `#[serde(flatten)] unknown…

Every struct ekko reads from a file and writes back keeps the fields it does not know, in a `#[serde(flatten)] unknown: BTreeMap<String, serde_json::Value>` (task [829](tasks/829.md), commit 718f0b5).

- The rule is enforced in item.rs by a test that scans src/ and requires the field on each struct deriving Deserialize, unless the struct is on its exempt list with a reason. The exempt ones are CueOn (its flattened Cue holds the map), the guard's index (a cache), the process registry, Marker, Config, the tasklist's Session and Task, the menu's Spec and Posed, and everything in ops.rs, which is a call's input.
- A new struct that is stored gets the field, and goes into the scan's expected list and into a fixture of json::assert_keeps_what_it_does_not_know.
- Never flatten a second map into an object that already has one, as CueOn over Cue would: each unknown field is then written twice. The round-trip test catches it with "a field is written twice".
- What an Item holds inline costs every item, because items are moved by value in the board's BTreeMap. Box a large field that few items hold, as question and wait now are: Item went from 2,096 to 952 bytes, and `--list pending` over 838 items from 35.5 to 30.5 ms (33.2 before 829).
- It helps only from this version on. A version older than the fix still drops a question's new fields, so a new question field is safe once no session runs an ekko older than this one (gotcha [194](gotchas.md#194)).
- Enum variants are a separate gap, measured in task [845](tasks/845.md).

*Decision · 2026-09-27 · on task [829](tasks/829.md)*

## <a id="823"></a>823. 805's design settled by the user on 2026-09-26 (questions 820 to 822, research in note 819).

- Scope: a cue on a project's board guards the calls that run inside the project's folder. A cue on the default board guards the whole machine. A cue may name a folder of its own, which wins. A call is judged by the folder it runs in, after the command's own cd and pushd, not by the session's board. The hook reads one small index of the cues that are on, rebuilt when a board changes.
- Turning a cue on: a session proposes it through ask, and the menu shows the gotcha, the command, the words and the folder. A person's 'Ligar' turns it on. Changing or dropping a cue that is on also goes through the user's answer. The gotcha holds only the cue a person approved; a proposal lives on its question until then.
- Exception: a refusal, by ekko's cue or by a ctx guard, records the exact call and its folder under a short code, and its reason tells the agent to ask with that code. The menu shows the command and folder from the record, not the agent's text. A person's 'Liberar uma vez' lets the next identical call through once: same command, same folder, the session that asked, within 24 hours. The use is recorded on the question. This is dcg's allow-once, reached through ekko's menu.
- Mine, from the research: port ctx's shell_calls.py to Rust, with its tests' shapes, and prove parity by running both lexers over the replay's commands. dcg uses tree-sitter-bash, but the replayed evidence (note 797) is on ctx's lexer.

*Decision · 2026-09-26 · on task [805](tasks/805.md)*

## <a id="810"></a>810. ekko, ctx and graff are built for Claude Code and nothing else. The user, 2026-09-26, in session 01215f07: 'já deixando…

ekko, ctx and graff are built for Claude Code and nothing else. The user, 2026-09-26, in session 01215f07: 'já deixando claro, tudo isso: ekko, ctx e graff é exclusivamente para o ambiente claude code e nada mais'.

- No other agent harness (Codex, Gemini CLI, Cursor...) is a target, and the three may lean on Claude Code's own features: hooks, plugins, the status line, claude -p.
- Each project stands on its own ('uma coisa é uma coisa e outra coisa é outra coisa', the same day). graff's rule on models (only local ones or Claude's own, and only once the rest of graff is done: decision 33 on graff's board) is graff's alone. Read from that reply, and stated back to the user in chat: it does not touch ctx's bulk-reader and code-writer, which keep delegating to Gemini through agy.

*Decision · 2026-09-26*

## <a id="801"></a>801. Bypass guards settled by the user on 2026-09-26 (questions [798](tasks/770.md#798) to 800, task [770](tasks/770.md)): bypass stays on, and the guards are…

Bypass guards settled by the user on 2026-09-26 (questions [798](tasks/770.md#798) to 800, task [770](tasks/770.md)): bypass stays on, and the guards are keyed on state, not on the command's name.

- ctx refuses a command only when it would destroy something that exists nowhere else, or bring secret material into the context, and says what. That covers uncommitted changes, a stash, a branch with no copy on a remote, and a force-push over the remote's main.
- ekko turns the board's gotchas that carry a command cue into blocks, with the gotcha's text as the reason. This revives note [402](tasks/337.md#402)'s item (1), which decision [587](#587) had deferred until there were more cued lessons; 770 is the new cause.
- A guard refuses, with the reason. The agent then fixes the cause (commit, stash, push), or asks the user through ekko's ask; the user's answer lets that exact command through once. Only an answer recorded by a person counts. ekko tells a session from a person by the process tree, so an answer the agent writes through MCP or through ekko in Bash is the session's.

Rejected:

- A static pack keyed on names (dcg, cc-safety-net): on the replay it would fire on the user's routine (note 797).
- Isolation (bubblewrap with no escape, or a container): the only guard that holds against a hijacked agent, but it blocks nix, git push and \~/NixOS (decision 582).

Threat model: accidents, not a hijacked agent, since a hook sees only the literal call (note 796). All four parts were chosen: work loss (ctx), secrets (ctx), board rules (ekko), and bypass declared in Nix.

*Decision · 2026-09-26 · on task [770](tasks/770.md)*

## <a id="792"></a>792. Claude Code's compaction, the safety net under ctx's handoff, fires at \~417k: autoCompactWindow 450000 with…

Claude Code's compaction, the safety net under ctx's handoff, fires at \~417k: autoCompactWindow 450000 with autoCompactEnabled true, in the claude wrapper's --settings (\~/NixOS modules/usr/ctx.nix), for both profiles. The user picked it on 2026-09-26 (question [790](tasks/789.md#790)).

- It sits above where the handoff ask actually lands (250-379k over 2026-09-23..26), so it replaces the ritual only when no /clear came after the ask: outside Konsole, after an auto-reset-stop, or when the user goes on.
- It follows CTX_HANDOFF_TOKENS (250k): move the two together, keeping the window above the ask's landing plus the handoff write.
- --settings outranks settings.json, so /autocompact and /config's toggle no longer change it; claude --autocompact &lt;N|auto> still does, for one launch.

*Decision · 2026-09-26 · on task [789](tasks/789.md)*

## <a id="765"></a>765. Decided by the user, 2026-09-26 \~17:15 (-03): a session left alone goes through the same handoff ritual, made automatic…

Decided by the user, 2026-09-26 \~17:15 (-03): a session left alone goes through the same handoff ritual, made automatic -- ctx asks for the handoff, then types /clear and 'continuando' into the session's own Konsole tab over D-Bus -- with no switch and no command to remember, and the handoff kept on the board. Claude Code's compaction is only the safety net where that cannot run (a tab that is not Konsole, a failed injection). 259's paired test (notes [752](tasks/259.md#752), [753](tasks/259.md#753)) backs either road on cost and quality: the ritual and the compaction cost the same per task within the noise, and the handoff itself is cheap (\~0.11M a reset); the user keeps the ritual because the handoff is what carries context across sessions. It replaces decision [759](#759), which had made the compaction the mechanism for a session alone.

*Decision · 2026-09-26 · on task [747](tasks/747.md) · replaces [759](#759)*

## <a id="762"></a>762. The handoff practice stays, on the measure of its first 5.6 days (note [761](tasks/757.md#761), 2026-09-26): against the baseline week, the…

The handoff practice stays, on the measure of its first 5.6 days (note [761](tasks/757.md#761), 2026-09-26): against the baseline week, the cost per call fell 45% (70.0k -> 38.2k units) and the context per call halved (mean 490k -> 251k), and the 25 handoff clears saved 23% net of what the era would have cost, as note [407](tasks/337.md#407)'s model said (22-27%). A clear pays only if the new session goes on: the 3 of 24 that lost money were followed by 32-34 calls, the winners by 44-427. The expensive part of a clear is the new session's orientation, over note 164's 2(b+R) in 18 of 24, not the handoff itself; 259 found Claude Code's compaction no worse within a run (decision [759](#759)).

*Decision · 2026-09-26*

## <a id="748"></a>748. Decided by session 83c87811 (first written as 0dca8739, the session it cleared), 2026-09-26 \~01:10 (-03), on the user's…

Decided by session 83c87811 (first written as 0dca8739, the session it cleared), 2026-09-26 \~01:10 (-03), on the user's word before going to sleep: trabalho's week (84%) resets at 09:00, 'pode utilizar o restante ... assume aí'. That rest, lost otherwise, buys a fourth long run, 396/bmax/2 on trabalho, after the three on default: two samples of B against two of C, so B's spread shows beside C's. Estimates: \~3.8M units, \~49 points of trabalho's 5-hour window that opens at 03:10, \~4 points of its week. Unit ekko-paired-after (log target/evals/paired/grade.log) waits on the default runs, starts bmax/2 only between 03:15 and 07:00 (to end before the week resets), then runs grade.py checks, judge and report. The fourth run's account differs: the same model, plugins and harness settings, but default also loads the serena (failing) and hm:nixos MCP servers; weigh that when reading B's two runs.

*Decision · 2026-09-26 · on task [259](tasks/259.md)*

## <a id="744"></a>744. Decided by the user, 2026-09-26 \~01:00 (-03), question [743](tasks/259.md#743): 259's three long runs go on the default account at once…

Decided by the user, 2026-09-26 \~01:00 (-03), question [743](tasks/259.md#743): 259's three long runs go on the default account at once, past the week's guard. The first launch (00:57) stopped in 5 s having spent nothing but its ping: default stood at 1% of its 5-hour window and 82% of its week (resets Mon 2026-09-28 23:00); trabalho at 88% and 83% (its week resets Sat 2026-09-26 09:00), so neither passed the harness's 70% week guard. The user: 'pode utilizar, eu tenho um reset que a anthropic me deu de presente, nao irei utilizar o default claude até segunda mesmo'. So evals/paired/harness.py pilot --long --week 0.95: a run starts while the week is under 95% and one in progress stops at 99%. The week's cost on default was never measured; \~6 points is an estimate from trabalho's ratio. It amends decision [741](#741) in the week guard only: the account, the night and the one-at-a-time unit stand.

*Decision · 2026-09-26 · on task [259](tasks/259.md)*

## <a id="741"></a>741. Decided by the user, 2026-09-26 -03, question [740](tasks/259.md#740) (the recommended option): 259's three long runs -- 396 in C/max twice…

Decided by the user, 2026-09-26 -03, question [740](tasks/259.md#740) (the recommended option): 259's three long runs -- 396 in C/max twice and in B/max once, \~11M units -- run on the default account (\~175k units a point of its 5-hour window, \~65 points; note 444), at night, one at a time as a systemd --user unit, prepared and launched from a fresh session rather than the one past 400k that asked. The trabalho account is spared: its week was at 81%, and a point there is \~77k units (gotcha [616](gotchas.md#616)).

*Decision · 2026-09-26 · on task [259](tasks/259.md)*

## <a id="676"></a>676. Decided by the user, 2026-09-25, questions [670](tasks/431.md#670)-675 (every recommended option), after the re-measure on 2.1.282…

Decided by the user, 2026-09-25, questions [670](tasks/431.md#670)-675 (every recommended option), after the re-measure on 2.1.282: trabalho's floor 38.0k (session 25ed7db1; target/evals/memoria/floor/now.rep). The cuts, for both profiles, in \~/NixOS:

(1) Artifact, 11.5k: off by default. The wrapper exports CLAUDE_CODE_DISABLE_ARTIFACT=1 when the caller left it unset, so CLAUDE_CODE_DISABLE_ARTIFACT=0 claude opens one session with it.

(2) SendFeedback, 1.9k: permissions.deny.

(3) ScheduleWakeup, 1.6k, and ReportFindings, 0.75k: permissions.deny. /loop without an interval and /code-review lose them.

(4) claude.ai's connectors, today only Claude Docs (\~1.3k): ENABLE_CLAUDEAI_MCP_SERVERS=false when unset, so ENABLE_CLAUDEAI_MCP_SERVERS=true claude brings them back for one session. claude.ai itself is untouched.

(5) The 17 skills unused in 30 days, 3.2k: skillOverrides user-invocable-only, so the agent's listing drops them and each /name still works. The 9 bundled ones: dataviz, code-review, schedule, run, loop, fewer-permission-prompts, simplify, security-review, init. The 8 from claude.ai: docs, pptx, xlsx, docx, pdf, morning, skill-creator, import-memory.

(6) Workflow, default only: kept (7 uses).

Expected: trabalho's floor \~18k, about 5% of usage, of which the Artifact is \~3% (note [459](tasks/431.md#459)'s model).

(7) Question [677](tasks/431.md#677), after the check in default: 4 more synced skills, default's account only and 0 uses, are hidden the same way. They are anthropic-skills:computer-use, built-in-browser, chrome-browser and deep-research (\~1.1k).

*Decision · 2026-09-25 · on task [431](tasks/431.md)*

## <a id="663"></a>663. Decided by the user, 2026-09-25, in chat ('sim, fecha a 496 como ferramenta pessoal'), after decision [637](#637): ekko is the…

Decided by the user, 2026-09-25, in chat ('sim, fecha a 496 como ferramenta pessoal'), after decision [637](#637): ekko is the user's own tool, for now. It is built for this user's setup -- NixOS, Konsole, the default and trabalho profiles, Claude Code -- and generality is not a goal: other terminals for the menu, other setups, docs written for others, other MCP clients. Any of these waits until the user starts wanting it.

*Decision · 2026-09-25 · on task [496](tasks/496.md)*

## <a id="637"></a>637. Decided by the user, 2026-09-25, question [635](notes.md#635) ('ekko não precisa rodar em outros agentes, somente no claude code, pode…

Decided by the user, 2026-09-25, question [635](notes.md#635) ('ekko não precisa rodar em outros agentes, somente no claude code, pode fechar essa, pq no momento só utilizo claude code, quando eu começar a pensar em utilizar outros tbm, ai sim iremos dar suporte'): ekko targets Claude Code only, the one agent the user runs. Support for another MCP client (Codex CLI, Gemini CLI, OpenCode) waits until the user starts using one, so 495 is cancelled until then. Note 494 had recommended the run.

*Decision · 2026-09-25 · on task [495](tasks/495.md)*

## <a id="614"></a>614. The default effort stays max, in \~/NixOS for both profiles. Decided by the user on 2026-09-24 (question [611](tasks/526.md#611)), from the…

The default effort stays max, in \~/NixOS for both profiles. Decided by the user on 2026-09-24 (question [611](tasks/526.md#611)), from the paired test's pilot (note [610](tasks/526.md#610)). High cost 0.24 of max per task. But the blind judge preferred max in 4 of 4 verdicts, and high's answer to 397 has a real bug. By note [549](tasks/526.md#549)'s rule, cost down with quality down keeps max. Eight more pairs could hardly have met the bar for switching, which is the judge preferring max in at most 3 of 10. Not tested: xhigh. The harness and the grader are in the commit that context 526 lists.

*Decision · 2026-09-24 · on task [526](tasks/526.md)*

## <a id="590"></a>590. The question menu in a window of its own is a feature to keep, not a host limit to work around. The user, 2026-09-24…

The question menu in a window of its own is a feature to keep, not a host limit to work around. The user, 2026-09-24: 'o lance da pergunta em uma nova janela abriu oportunidade para interagir com o claude enquanto a pergunta está no aguardo, isso na minha visão é extremamente positivo'. Seen that day: while question [589](tasks/260.md#589) waited in the menu, the user asked the session 'mas qual o ganho do titulo?', read the answer, and then chose in the menu. Claude Code 2.1.282 moves a running MCP call to the background, at once when a user message arrives and on its own after 120 s, so the ask's answer came later as a task notification. That is host behaviour, and a Claude Code release could change it. This corrects tension (3) of note 494, which counted the menu's own window only as a cost. Keep asks non-blocking in any redesign of the menu.

*Decision · 2026-09-24*

## <a id="587"></a>587. Memory types settled by the user, 2026-09-24 (questions [583](tasks/337.md#583) to 585, closing 404). Of note [338](tasks/337.md#338)'s seven ideas, three are…

Memory types settled by the user, 2026-09-24 (questions [583](tasks/337.md#583) to 585, closing 404). Of note [338](tasks/337.md#338)'s seven ideas, three are built. (1) became the fixed PreToolUse guard for pgrep/pkill -f in \~/NixOS (note [445](tasks/337.md#445)), with no board mechanism until consolidation yields more cued lessons. (2) is 398's hint when a task completes. (5) is owners (264), authorship (125) and the 'Ekko:' trailer (396). Three are dropped. (3) verifiedAt: supersedes covers the corrections seen. (4) waiting triggers: every wait seen was on an outside event, and due covers dates. (6) --run: a stored command would be a persistent vector for injected instructions, and a procedure names a script in the repo instead. (7) structural salience is deferred. Recommendation 497 (retrieval and forgetting) is answered by 507: retrieval by the prompt cost more than it saved, and the static prime already names 56% of what sessions fetch (note 517). Forgetting runs on supersedes, on notes leaving the prime with their task, and on the stash. Not a task for now: search across projects (note 408's gap), seen once in a month; revisit if it repeats. The old feedback rules' migration is task [586](tasks/586.md), at the user's word. With decision [410](#410) (auto memory off, ekko the memory per project), this closes 337.

*Decision · 2026-09-24 · on task [337](tasks/337.md)*

## <a id="563"></a>563. Decided by the user, 2026-09-24, session 8c68c63f (questions [553](tasks/526.md#553)-555; 'agora ficou claro, aprovado, pode seguir…

Decided by the user, 2026-09-24, session 8c68c63f (questions [553](tasks/526.md#553)-555; 'agora ficou claro, aprovado, pode seguir, escolha o perfil de trabalho'): build the paired-test harness of note [549](tasks/526.md#549) in evals/paired/ and run stage 1 (the probe, then the pilot on 125+396, 394 and 397) on the trabalho profile's account; drop 259's arm A. Stage 2 (\~41M units, \~240 points) waits on the user's word after the pilot's numbers. The user first read the plan as merging the two profiles' quota; it is not: each profile is its own account (checked: different accounts and subscriptions), the runs log in with one, and the harness copies each run's transcripts out so deleting a profile loses no data. Lesson for how to put such a plan: say first what is being found out and why, then how, then what it costs; cells, arms and units only after that.

*Decision · 2026-09-24 · on task [526](tasks/526.md)*

## <a id="537"></a>537. Decided by the user, 2026-09-24, questions [532](tasks/525.md#532)-535 ('siga os 4 recomendados, aprovado'): (1) 86772cd, evals/recall/, is…

Decided by the user, 2026-09-24, questions [532](tasks/525.md#532)-535 ('siga os 4 recomendados, aprovado'): (1) 86772cd, evals/recall/, is pushed to origin. (2) 525's guard lives in ctx, a UserPromptSubmit hook beside the Stop hook, the handoff marks and the status line, not in the public ekko plugin. (3) No new way to write the handoff while the cache is warm: the Stop hook's bands, /handoff and the status line's age stay as they are; the guard shows how old the handoff is and the user chooses between /clear from it and sending the prompt again. The cold returns are re-counted in two weeks: note [518](tasks/507.md#518)'s 4 of 65 is from before the Stop hook went live (2026-09-23), and since 2026-09-22 noon 3 of 6 cold rewrites had a handoff in the 3 hours before. (4) 526 is designed after 525 is built, in a new session, together with 259 on one shared harness. Not taken for now: a delta read off the transcript since the last handoff, and a desktop notification before the cache goes cold. AMENDED the same day, at the user's word ('sim, aplica as duas e segue com o commit'), after the question of what the guard gains day to day: its default threshold is 250k, the Stop hook's, since under \~210k of context going on costs less than starting over; and its reason gives both prices, going on (2 x the context) against starting over (\~2 points: a fresh \~45k prefix plus \~331k units of orientation). Built and pushed as ctx 0.3.0, f109877 (note 544).

*Decision · 2026-09-24 · on task [525](tasks/525.md)*

## <a id="527"></a>527. Decided by the user, 2026-09-24 (task [507](tasks/507.md) cancelled at 'pode continuar', session 844752fe): ekko does not inject board…

Decided by the user, 2026-09-24 (task [507](tasks/507.md) cancelled at 'pode continuar', session 844752fe): ekko does not inject board items by the user's prompt (a UserPromptSubmit retrieval). Measured offline in note 517: it costs more than it saves in every configuration (up to 0.9% of the bill lost), the static prime already names 56% of what sessions fetch, and the quality gain is thin (of 30 new items judged, 3 clearly useful and 20 noise). The board's 'attention' stays deterministic and position-like: the prime (recency and state), handoffs, and command cues (note [402](tasks/337.md#402)). Revisit only with new evidence, such as a board far larger than a prime can cover, re-measured with evals/recall/replay.py (procedure [523](procedures.md#523)).

*Decision · 2026-09-24*

## <a id="511"></a>511. Provenance, as built for 125 and 396 (the user chose each part in questions [504](tasks/396.md#504), [505](tasks/396.md#505) and [506](tasks/396.md#506), 2026-09-24). WHO WROTE AN…

Provenance, as built for 125 and 396 (the user chose each part in questions [504](tasks/396.md#504), [505](tasks/396.md#505) and [506](tasks/396.md#506), 2026-09-24). WHO WROTE AN ITEM: createdBy, the holder record heldBy uses, set as an item is created and never after -- the session (profile, terminal, the conversation it ran then) or the person; absent on items from before, which name no author. context shows it on a line of its own; by:NAME finds it in --list and search (user, a profile, or a conversation's start of 4+ characters). WHERE A TASK LANDED: a commit names its tasks with the trailer 'Ekko: &lt;display id>', several on one line, a uid read too. The agent writes it, told by a notice when it puts a task in progress on a project in a git repository -- no git hook, no PreToolUse block. context reads the commits from git on each read, nothing stored, so a rebase keeps the link; a line counts only if everything after 'Ekko:' is ids or uids. A note says the commit is listed by context rather than citing a SHA.

*Decision · 2026-09-24 · on task [396](tasks/396.md)*

## <a id="486"></a>486. The menu's extras stay, and its look: the user's answers in ekko's own menu on its real check (2026-09-24, session…

The menu's extras stay, and its look: the user's answers in ekko's own menu on its real check (2026-09-24, session 874cd2ed, a scratch board; three questions in one call, answered together after the review). Kept, all three marked: the desktop notification after 20 s without an answer, the Konsole window sized to the menu, and the review screen before recording several answers. The focused option stays an arrow and bold cyan, chosen over a reversed line with the preview beside each. The check itself: a Konsole window of 100x19 opened, the three answers came back in one reply as answers [{id, answer}], recorded by the user, and no elicitation was sent to a client that shows forms.

*Decision · 2026-09-24 · on task [395](tasks/395.md)*

## <a id="484"></a>484. ekko's menu covers what AskUserQuestion had, in one release, before AskUserQuestion goes back into the deny (the user…

ekko's menu covers what AskUserQuestion had, in one release, before AskUserQuestion goes back into the deny (the user, 2026-09-24: 'pode seguir assim'). The user's reason to own it: depending on Anthropic's architecture, which can change at any time, is a flaw ('se mudar lá, muda aqui'); code to maintain is not a cost to them. The plan: (1) no new window where it can be avoided: in Konsole, a split beside the conversation through Konsole's D-Bus (createSplit, runCommand), closing itself once answered and handing focus back, with a desktop notification when the user is elsewhere; a new window only outside Konsole; (2) without a display: a tmux popup inside tmux; otherwise the question stays open and the session hands over `ekko --answer <id>`, the same menu in any terminal; the elicitation form only as the last resort; (3) the features: several questions in one call (tabs, ←/→), several options picked (space), a preview beside the focused option, a note with the choice; all of ask's schema changes in the same release, one prefix change; (4) then the deny, after the release and the rebuild.

*Decision · 2026-09-24 · on task [395](tasks/395.md)*

## <a id="482"></a>482. Questions reach the user through ekko's own menu, not Claude Code's (the user's choice through AskUserQuestion…

Questions reach the user through ekko's own menu, not Claude Code's (the user's choice through AskUserQuestion, 2026-09-24, session 7ed2ad71, over the recommended 'native menu + ekko records by hooks' and 'both'). Why not the form: MCP elicitation is the only UI an MCP server gets inside Claude Code's screen, and Claude Code draws it as a form whose option is a dropdown (note [480](tasks/395.md#480)). The user saw AskUserQuestion's menu after the deny came off and wants that kind of menu, drawn by ekko. So: ask records the question and waits for its answer to land on the board; ekko draws a menu outside Claude Code (options with arrows and Enter, and a free-text answer), which writes the answer. The user runs Konsole on Wayland with no tmux, so the menu cannot be a popup over Claude Code's pane. AskUserQuestion stays allowed for now; whether it goes back into the deny is open. WHERE: a new window each time (the user's choice, recommended, over a standing pane or both): ask opens a Konsole window holding only the question, and it closes once answered.

*Decision · 2026-09-24 · on task [395](tasks/395.md)*

## <a id="452"></a>452. Who a task is with, as built for 87 (the user approved it on 2026-09-23 in question 451: 'ta, beleza pode aplicar').…

Who a task is with, as built for 87 (the user approved it on 2026-09-23 in question 451: 'ta, beleza pode aplicar'). The field is with: one word, stored in lower case, on tasks only, and absent unless set. THE DEFAULT IS THE OPPOSITE OF NOTES [90](notes.md#90)/91. A task with no with is anyone's, and the agent takes it up as before. A task with someone leaves the agent's queue. Reasons: a board that names nobody stays byte-identical and behaves as before; the agent never has to mark the work it gives itself; a forgotten mark falls back to today's behaviour. The August default (no mark means the user's, and the agent works only with:agent) would have emptied the agent's queue on every board that marks nothing. No name is special: there is no 'mine', no 'agent' and no reserved name for the user, and the prime groups by name. Each task has one name, whoever has the ball now. Names match ignoring case and accents (lexical fold), and storage keeps the accents. A ready task with someone is still ready: --list ready and nowReady keep it. Only next, the prime's Ready and the task list leave it out. 'The agent marks what it claims' (note [90](notes.md#90)) is heldBy from 264. Authorship stays 125.

*Decision · 2026-09-23 · on task [87](tasks/87.md)*

## <a id="449"></a>449. A handoff that a later one replaces is recorded in supersedes, the field decisions use: ekko sets it on the new handoff…

A handoff that a later one replaces is recorded in supersedes, the field decisions use: ekko sets it on the new handoff when it demotes the old one. It is not a new storage field (426 suggested 'e.g. the new handoff's uid') because an MCP server still on an older binary rewrites a note keeping only the fields it knows (gotcha [194](gotchas.md#194)), and every binary knows supersedes. It follows that a note without a kind supersedes only as a handoff: such a note takes no kind, and a handoff moved to another task drops its supersedes. Chosen by session 81ba8737 while building 426 (55e506b), 2026-09-23, without asking the user: revisit if they object.

*Decision · 2026-09-23 · on task [426](tasks/426.md)*

## <a id="430"></a>430. Decided by the user, 2026-09-23, question [422](tasks/422.md) (from task [416](tasks/416.md), note [417](tasks/416.md#417)): option (a). Measure what each part of every…

Decided by the user, 2026-09-23, question [422](tasks/422.md) (from task [416](tasks/416.md), note [417](tasks/416.md#417)): option (a). Measure what each part of every session's floor weighs, then decide with the user what to unload and where. The session estimates each part from its text; the user checks with /context in a fresh session of each profile. Rejected: leaving the floor at \~38.6k unexamined. The measurement is its own task on @memoria.

*Decision · 2026-09-23 · on task [422](tasks/422.md)*

## <a id="428"></a>428. Decided by the user, 2026-09-23, question [421](tasks/421.md) (from task [416](tasks/416.md), note [417](tasks/416.md#417)): option (a). What must outlive a handoff goes…

Decided by the user, 2026-09-23, question [421](tasks/421.md) (from task [416](tasks/416.md), note [417](tasks/416.md#417)): option (a). What must outlive a handoff goes into a typed note (decision, gotcha, procedure); the handoff carries the state and the next step. Prompted at both moments: ctx's handoff text asks for the typed notes first (built with 424), and completing a task lists its attached untyped notes and asks whether one holds a lesson, with nothing written on its own (398). Rejected: either moment alone (the handoff alone misses tasks closed without one; completion alone lets a long task's chain of handoffs decay until it closes), and nothing. This settles item (2) of question [404](tasks/337.md#404): build it small, with 398.

*Decision · 2026-09-23 · on task [421](tasks/421.md)*

## <a id="427"></a>427. Decided by the user, 2026-09-23, question [420](tasks/420.md) (from task [416](tasks/416.md), note [417](tasks/416.md#417)): option (a). ctx's handoff text, the Stop…

Decided by the user, 2026-09-23, question [420](tasks/420.md) (from task [416](tasks/416.md), note [417](tasks/416.md#417)): option (a). ctx's handoff text, the Stop hook's ask and 424's /handoff command alike, asks the session to name by id the notes the next session must read in full, and what it can skip; the next session reads concise first. Rejected: also changing ekko's description of the handoff kind (it sits in create's always-loaded schema, which every session pays for and 275's test guards), and nothing. Built with 424.

*Decision · 2026-09-23 · on task [420](tasks/420.md)*

## <a id="425"></a>425. Decided by the user, 2026-09-23, question [419](tasks/419.md) (from task [416](tasks/416.md), note [417](tasks/416.md#417)): option (a). A replaced handoff shows in…

Decided by the user, 2026-09-23, question [419](tasks/419.md) (from task [416](tasks/416.md), note [417](tasks/416.md#417)): option (a). A replaced handoff shows in context marked as history ('[handoff, replaced by N]') and clipped as concise clips, even under detail full, the rule task [235](tasks/235.md) set for the prime; its full text stays reachable by its own id. Rejected: marking only (it saves nothing), leaving it out of detail full (a fact only it holds, like 401's 2,000-byte constraint, would drop out of reach), and nothing. The build is its own task on @memoria.

*Decision · 2026-09-23 · on task [419](tasks/419.md)*

## <a id="423"></a>423. Decided by the user, 2026-09-23, question [418](tasks/418.md) (from task [416](tasks/416.md), note [417](tasks/416.md#417)): option (a) with (b). ctx's status line shows…

Decided by the user, 2026-09-23, question [418](tasks/418.md) (from task [416](tasks/416.md), note [417](tasks/416.md#417)): option (a) with (b). ctx's status line shows how old the handoff is once one was asked for or written (e.g. 'handoff 80k ago'), at no token cost, and a /handoff command asks for a fresh handoff before a /clear; the user chooses when, and nothing interrupts the session. Rejected: the hook asking again every +50k, since each ask holds a turn (\~47 s and about four calls at high context) and interrupts most in the long, important sessions; and leaving it as is. The build is its own task on @memoria.

*Decision · 2026-09-23 · on task [418](tasks/418.md)*

## <a id="410"></a>410. Decided by the user, 2026-09-23, replacing decision [372](#372), after session 06637eec explained how auto memory works (per…

Decided by the user, 2026-09-23, replacing decision [372](#372), after session 06637eec explained how auto memory works (per repository and per profile, its MEMORY.md index loaded into every session) and memory as context poisoning. Claude Code's auto memory stays off in every case and every profile (\~/NixOS/modules/usr/ekko.nix sets CLAUDE_CODE_DISABLE_AUTO_MEMORY=1, note [353](tasks/337.md#353)). Claude Code sessions are impermanent: 'vamos trabalhar com Impermanência e algo mais efêmero no que tange sessoes de claude code'. What must outlive a session goes on the project's ekko board, which takes over memory per project; quick, direct sessions need no memory at all. Risk accepted as the user put it: ekko can do what auto memory did and become a source of context poisoning, 'mas ai depende da ocasião e como o usuário está agindo'. Rejected, as in 372: splitting by subject, which would turn auto memory back on outside projects with a board. Still open on this task: 404 (verdicts per memory type), 405 (order), 406 (the NixOS hook for pkill -f and pgrep -f; the feedback rules).

*Decision · 2026-09-23 · on task [337](tasks/337.md) · replaces [372](#372)*

## <a id="343"></a>343. Owners, as built (decided 2026-09-22, session 9e449e1b, the user having left it to the session). WHO: the Claude Code…

Owners, as built (decided 2026-09-22, session 9e449e1b, the user having left it to the session). WHO: the Claude Code process, not the session id: the MCP server's CLAUDE_CODE_SESSION_ID is fixed when Claude Code starts it and names another conversation after a resume or a /clear (this session's server read 4ca5b91c while its conversation was 9e449e1b), while the process is the one terminal across /clears, the server's parent, and an ancestor of every hook and Bash command. Named for people by profile and terminal ('trabalho on pts/1'); a command typed outside Claude Code is the user. LIVENESS: /proc, by pid, start tick and boot, so a claim ends when its process does; no clock, no lease, no lock files. START: setting a task in progress, by any path. END: the task leaving progress, or its process ending; a handoff keeps the claim, since the same process goes on after /clear. RULE: another running session is refused as HELD on any change of the task's state, force_state takes it over at the user's word, and a person at the terminal is never refused. A person can hold a task. Stored as heldBy on the task, kept by 0.11 and later as an unknown field.

*Decision · 2026-09-22 · on task [264](tasks/264.md)*

## <a id="327"></a>327. The handoff trigger belongs to ctx, not ekko (the user, 2026-09-22). ctx (github.com/thiagoproldan/ctx, created…

The handoff trigger belongs to ctx, not ekko (the user, 2026-09-22). ctx (github.com/thiagoproldan/ctx, created 2026-09-21, empty so far) is a fork of shunt, the context tool whose status line runs today, and works in step with ekko. Both halves go there: the status line, and the Stop hook that past the threshold keeps the turn open once to ask for the handoff (note [320](tasks/267.md#320)). ekko builds neither. What ekko must offer ctx -- whether the session's task has a fresh handoff, which task the session holds (264) -- becomes an ekko task when ctx asks for it. This task waits on ctx.

*Decision · 2026-09-22 · on task [267](tasks/267.md)*

## <a id="323"></a>323. --copy and --since stay (the user, 2026-09-22: 'o resto como --copy e --since sao importantes'), although neither shows…

--copy and --since stay (the user, 2026-09-22: 'o resto como --copy e --since sao importantes'), although neither shows in the shell history or the transcripts since 2026-09-15. Absence from those two sources is not evidence of disuse: do not propose removing a flag on that basis alone. arboard's weight (49 of the 74 crates) is the price of --copy.

*Decision · 2026-09-22 · on task [321](tasks/321.md)*

## <a id="317"></a>317. rustfmt stays off: the repo is not reformatted and CI does not check formatting (decided 2026-09-22, session 9e449e1b).…

rustfmt stays off: the repo is not reformatted and CI does not check formatting (decided 2026-09-22, session 9e449e1b). cargo fmt --check differs in 734 hunks, or 168 with max_width 120 and use_small_heuristics Max; reformatting would rewrite most files for no change in behaviour, bury every line's history under one commit and conflict with every patch in flight, and note 184 already set the rule never to reformat. New code keeps the surrounding style by hand.

*Decision · 2026-09-22 · on task [278](tasks/278.md)*

## <a id="314"></a>314. A stale server warns, it does not refuse, and it tells by its binary rather than by a version fence (decided…

A stale server warns, it does not refuse, and it tells by its binary rather than by a version fence (decided 2026-09-22, session 9e449e1b). Refusing writes would stop the session that made a release from writing its handoff after the rebuild. The binary check needs no storage change and also catches a rebuild at the same version; the fence needs a newer server to have written the board first. Both guard only the upgrades after the release that ships them.

*Decision · 2026-09-22 · on task [274](tasks/274.md)*

## <a id="313"></a>313. The blocking weight in next's urgency is 1, not Taskwarrior's 8 and not 0 (decided 2026-09-22, session 9e449e1b, the…

The blocking weight in next's urgency is 1, not Taskwarrior's 8 and not 0 (decided 2026-09-22, session 9e449e1b, the user having left it to the session). 8 outweighed the whole gap between p3 and p2 (2.1), so a p2 task holding up p2 work ranked above every p3 task; man taskrc advises 0 under inheritance, and ekko already inherits the highest priority and the latest finish of the work that waits. 1 keeps 'more work waiting on it' as the order within one priority, as next's description says, and never above priority. Accepted cost: within one priority, a task with any date, however far, ranks above one that holds up work, since the due ramp starts at 0.2 x 12 = 2.4. Built in f61dc17, with semantics.py's 1.5 target moved to [1, 3, 4, 5].

*Decision · 2026-09-22 · on task [284](tasks/284.md)*

## <a id="250"></a>250. Decided by the user on 2026-09-21, answering three questions after task [239](tasks/239.md). (1) Commit the removal of --ui: done…

Decided by the user on 2026-09-21, answering three questions after task [239](tasks/239.md). (1) Commit the removal of --ui: done, ffbbf19, signed, local. (2) Take up review 241's recommendations 1 and 2 before releasing v0.11.0, as 246, 247 and 248, which block this release. (3) Push the tag ui-v1 with the release. Nothing is pushed before then: main is four commits ahead of origin.

*Decision · 2026-09-21 · on task [249](tasks/249.md)*

## <a id="243"></a>243. Cancelled 2026-09-21 along with task [239](tasks/239.md), which removes --ui. WHY IT DIED. The user barely used the interactive mode…

Cancelled 2026-09-21 along with task [239](tasks/239.md), which removes --ui. WHY IT DIED. The user barely used the interactive mode, and it did not come out as they wanted. Even frozen, it taxed every change to the core: the waiting state touched four TUI files and needed a TUI test. src/tui was 8,518 of about 24k lines. WHAT WAS TRIED. Decision 158's direction: a dashboard of panels in the shape of VS Code, full parity with the CLI, ratatui over crossterm. Phase 1 landed (note 159), and nothing after it. THE CODE is kept at the signed tag ui-v1 on 62e45d6, the last commit with src/tui. The tag stays local until the user says to push it: the repo is public. FOR THE REDO. A UI gets redone from scratch later, not from this code. Start from the moments the user would open a UI instead of the CLI or the agent, not from a layout. It may not be a TUI (decision [240](#240)).

*Decision · 2026-09-21*

## <a id="240"></a>240. Approved by the user on 2026-09-21: 'adiciona isso ao board e faz o handoff, vamos fazer isso na nova session, mas…

Approved by the user on 2026-09-21: 'adiciona isso ao board e faz o handoff, vamos fazer isso na nova session, mas seguindo suas recomendações'. WHY REMOVE, NOT FREEZE. The user barely uses --ui, and it did not come out the way they wanted. They will redo a UI later from scratch, not from this code. src/tui is 8,518 of about 24k lines (35%). ratatui, crossterm and unicode-width are used nowhere else; arboard stays for --copy. Frozen, it still taxes every core change: the waiting state touched four TUI files and needed a TUI test. THE CUT IS CLEAN. Outside src/tui, only main.rs refers to it: mod tui, the cli.ui dispatch near main.rs:223 and two HELP lines. Comments name it in tasklist.rs:161 and cli.rs:66. \~/NixOS has no reference to --ui. --calendar is CLI and stays.

STEPS. 1) git tag ui-v1 on 62e45d6, the last commit with the TUI. Ask before pushing it: the repo is public. 2) Cancel 108 with a decision note: why it died, decision 158 as the direction tried, and the tag. 3) Edit 117 to drop its UI-panel half and keep the CLI part. 4) Remove src/tui, mod tui, the ui flag in src/cli.rs and its dispatch, the HELP lines, and the three dependencies (Cargo.lock too, since builds use --locked). Remove readme's Interactive Mode section, its 7 --ui mentions, and the comments that name --ui. Answer --ui with a stable refusal the way the renamed flags are answered (tests/cli.rs an_old_flag_name_is_answered_with_the_new_one), so a typed --ui says it was removed. 5) Checks: cargo test, clippy -D warnings, a release build, semantics.py and scale.py. Measure the binary size (5,710,296 bytes before) and the build time. 6) Ask the user before committing. It ships in v0.11.0 with the three commits already on main.

FOR THE REDO, later: start from the moments the user would open a UI instead of the CLI or the agent, not from a layout. It may not be a TUI.

*Decision · 2026-09-21 · on task [239](tasks/239.md)*

## <a id="236"></a>236. Validated and approved by the user on 2026-09-21 ('valide a opção 1 antes', then 'sim, pode fazer'). RULE: when the…

Validated and approved by the user on 2026-09-21 ('valide a opção 1 antes', then 'sim, pode fazer'). RULE: when the prime shows a handoff, 'Recent notes, not attached to a task' lists only loose notes whose updatedAt is newer than that handoff's. With no handoff shown, nothing changes. EVIDENCE: all 63 loose notes on this board were older than handoff 230, so the section, 1,646 of 7,477 characters (22%), would vanish from every session start. In today's transcripts, six sessions read loose notes in full: 184 three times, then 170, 169 and 167. All came mid-session, through a reference in another note or a search, never from the prime's section, so none would have changed. The one time the section drove a decision at session start was harmful: in session bb04549e, 184's stale clip ('1) 172 typed knowledge notes...') led to recommending 172, which was already done. RISK: a loose note written just before its session's own handoff is hidden at the next start. That happened once: 184 was created 5 minutes before handoff 185, and 185 cited it by id. So the handoff prompt should ask for the notes the next session needs to be named by id. REJECTED: first lines only (saves \~750 characters, and its 160-character cuts land mid-sentence in notes not written headline-first); hiding the section whenever a handoff exists (the same saving here, but it would also hide a note the user wrote after the handoff); attaching the 63 legacy notes (one large read, and new loose notes would still pile up).

*Decision · 2026-09-21 · on task [235](tasks/235.md)*

## <a id="232"></a>232. Waiting state, approved by the user on 2026-09-21 ('sim, pode seguir') as proposed. (1) A sixth state, waiting, beside…

Waiting state, approved by the user on 2026-09-21 ('sim, pode seguir') as proposed. (1) A sixth state, waiting, beside paused. Paused is set aside by choice and stays ready. Waiting cannot move until something outside the board happens, so it leaves ready, next and the Claude Code task list. It is still open work: it counts in the total and the percentage, holds its dependents and passes --list pending. It is stored as waiting: true only when true, so a board without it stays byte-identical, and taskbook reads it as pending. (2) What it waits on goes in an attached note. There is no new field; 87's with:NAME composes with it later. (3) No check-back date for now. (4) Prime gets a 'Waiting (N)' section after Blocked: first lines, at most 5, then '+N more'. A task that is both waiting and blocked is shown only there. (5) CLI --set waiting and --list waiting. MCP set_state waiting and a search filter waiting, with no new sentence in the instructions. The board view gets its own icon and 'N waiting' in the stats line. --ui gets the icon, a colour and a search chip, but no key. (6) The name is waiting; @waiting stays a board (task [116](tasks/116.md)). (7) After the release, with the user's consent, set 180 and 220 to waiting. It ships as v0.11.0, only at the user's word.

*Decision · 2026-09-21 · on task [88](tasks/88.md)*

## <a id="229"></a>229. Every ekko tool is allowed in Claude Code's permissions, and trash and force_state ask. This is set in \~/NixOS usr.ekko…

Every ekko tool is allowed in Claude Code's permissions, and trash and force_state ask. This is set in \~/NixOS usr.ekko (usr.claude-code.settings.permissions), for all four profiles. Why: in auto mode, a tool the rules do not allow waits for the safety classifier before it runs. Measured on 2026-09-21 in one session: the same set_state took 1,326 ms before the rule and 37 ms after. Every ekko write had cost about 1.4 s all day, in every version, before and after the task-list hook; the ekko server takes about 7 ms per write and its hooks about 4 ms. Allowing is safe: the writes are recoverable through the journal, the stash and the trash, and the two that are not ordinary edits still ask. Only .claude-trabalho's settings.json allowed even the reads before. Until the rebuild, \~/Projetos/ekko/.claude/settings.local.json (kept out of git by .git/info/exclude) allows the writes in this project; delete it after the rebuild. ADDED the same day: 35 shell-command prefixes that only read, test or build are allowed too (cargo test/clippy/build and python3 evals/ through nix develop, git status/diff/log/show, gh run list/view/watch, gh release view, nix build/path-info/eval, ekko --prime/--next/--context/--version, jq, ls, cat, head, tail, wc, sort, uniq, cut, tr, diff, pgrep, ps, readlink, stat, file). Measured before: 578 non-plain shell calls had a median of 1.7 s, against 0.13 s for plain reads. Left out on purpose: sed, find, awk, rm, mkdir, git commit/push/tag, gh release create, gh api, nix flake update. Loops, heredocs and $(...) still go to the classifier. Measure again after the rebuild. MEASURED after the rebuild, with v0.10.2 and only the NixOS rules, one call at a time: set_state 62 ms (1,326 before); `cd <repo> && git log --oneline -1 | cat` 107 ms (about 1.4 s before); `git -C <repo> status` still 1,611 ms, because the rule matches the command's start and git -C is not allowed on purpose (it would let commit and push through), so prefer `cd <repo> && git status`. Parallel calls in one response are logged together and take the slowest one's time, so measure one call per response. The stopgap .claude/settings.local.json was deleted.

*Decision · 2026-09-21*

## <a id="227"></a>227. A bare 'continue' after /clear means the handoff's next step at its planned size, and stopping where it says to ask. A…

A bare 'continue' after /clear means the handoff's next step at its planned size, and stopping where it says to ask. A better but bigger route (new tooling, reading Claude Code's binary, a new eval) is offered inside that question, not done. Why: on 2026-09-21 a 'continue' on task [176](tasks/176.md) ran for 10 minutes and used 129k context and 58.8k output tokens. It reverse-engineered Claude Code and built a stand-in API to avoid two short paid messages, the step the plan said to ask about, and spent more than it saved. The user watches context, time and output tokens. Moved here from Claude Code's auto memory, which is now off (the board is the one memory).

*Decision · 2026-09-21*

## <a id="204"></a>204. Approved by the user on 2026-09-21 for task [202](tasks/202.md). (1) CLAUDE_CODE_ENABLE_TODO_TOOLS=1 and…

Approved by the user on 2026-09-21 for task [202](tasks/202.md). (1) CLAUDE_CODE_ENABLE_TODO_TOOLS=1 and CLAUDE_CODE_TODO_REMINDER_MODE=off go in the NixOS module that installs the ekko plugin in the four profiles: it is part of installing ekko, not a new dependency. (2) Never the native task tools, only ekko: the agent does not use TaskCreate, TaskUpdate, TaskList or TaskGet, and the native list shows only what ekko writes. The plan is to deny those tools in the same module's permissions, if a denied tool still leaves the widget drawn. (3) The list shows the next 5 ready tasks, besides work in progress and what this session finished.

*Decision · 2026-09-21 · on task [202](tasks/202.md)*

## <a id="163"></a>163. Second pass on the model behind this task, 2026-09-15 -- it corrects the framing, not the direction.

Measured parameters (week 2026-09-08..15, all profiles): context growth d = 2.28k tokens per call (mean; median 1.12k), output o = 1,439 per call, system floor b = 37k, orientation to first edit in fresh sessions R = 60k tokens over 10 calls (median, only n = 6, p75 70k). Auto-compaction fires at 989k (median) and leaves 53k, so today sessions are segmented every \~410 calls. 82% of cost comes after call 100 of a session, 70% after call 200. 10.1% of cost is full-prefix cache rewrites, and 56% of those follow a gap under 60 minutes -- invalidations, not TTL expiry; causes not identified.

Per-call cost with segment length L and reset overhead W on base B: c(L) = 2d + 5o + 0.1B + 0.05dL + W/L. Optimum L\* = sqrt(20W/d), c\* = 2d + 5o + 0.1B + 2 sqrt(0.05dW). Status quo modelled at 64.5k units per call against 66k observed with rewrites (+8%). 2d + 5o is 18% and no policy touches it.

Results. Handoff with B = b + R: R = 60k gives -52% (x = 0 extra calls per reset), -47% (x = 10), -33% (x = 50), break-even at 230 extra calls; R = 120k gives -39/-33/-17%; R = 200k gives -22/-16/-2%. By logical session length it loses below \~150 calls (never reset short work), +17% at 186 calls, +38% at 300, and flattens near 52% from \~410 on, because auto-compaction already bounds the monolithic case.

The finding that changes this task: compacting earlier with no ekko at all gives the same math -- -57% at a 200k cap, -51% at 300k -- and Claude Code exposes an auto-compact window setting (--autocompact &lt;auto|tokens>, CLAUDE_AUTOCOMPACT_PCT_OVERRIDE). The potential belongs to segmentation, not to ekko. The handoff earns its place only if, at the same cap, it beats compaction on R (what the resume has to re-read), x (turns lost to missing nuance) and task quality; none is measured. Anthropic's context engineering post supports the direction (context rot; compaction risks 'loss of subtle but critical context'; structured note-taking) but gives no coding numbers.

So acceptance needs a paired test on the same tasks: (A) status quo, (B) a smaller auto-compact window, (C) handoff plus /clear at the same cap -- cost per completed task, extra calls, quality. The rigor plan items that shrink R now carry the real weight: 0.2 prime budget, 2.2 concise context, 2.3 several items per call.

*Decision · 2026-09-15 · on task [160](tasks/160.md)*

## <a id="162"></a>162. Study 2026-09-15: token proxies and compressors (rtk, headroom, caveman) against a critique (Rakuen, 'Token compression…

Study 2026-09-15: token proxies and compressors (rtk, headroom, caveman) against a critique (Rakuen, 'Token compression tools measure the wrong thing', rev. 2026-08-02), done with our own tools: the three READMEs (26-34 KB) went through shunt-bulk-read on Gemini and every cited line was checked on disk; graphify did not apply (prose, not code), which is its correct-scenario boundary.

Verdict: they can lower or raise the bill, and none publishes the number that decides it, cost per successful task with cache writes, reads and extra turns. Paired evidence that exists: rtk 0.43.0 on Claude Code (JetBrains, 80 pairs) +7.6% median cost at low effort (turns +13.8%, cache reads +14.3%), +0.1% at high effort, quality tied; caveman skill only (JetBrains, 86 tasks) -8.5% output tokens, \~-10% cost, quality flat. Vendor-only: caveman proxy -33.2% provider input tokens on a 54-run haystack suite (not public, input tokens not cost; HTML case +9.9%; reports headroom at 6.7% saved and 3/18 checks failed -- competitor claim); headroom 21-57% offline compress() on payloads, no end-to-end cost.

Applied to our week (2026-09-08..15, Opus 5 weights): tool results are 10.2% of cost (Bash 8.63%, Read 0.97%). Ceilings: rtk 7.8% but \~0 at the high/xhigh effort we run; any proxy on all tool results 3.4-5.8%, wiped by 3.6 extra turns per 100 calls (one extra turn at the 471k average context costs \~54k input-equivalent tokens); caveman skill +0.75% net but makes answers terse, which the user does not want. Concrete cost-raising mechanism: headroom's effort routing changes effort per turn, and per Anthropic's caching docs a top-level effort change invalidates the messages cache -- about 0.33% of the week's cost per toggle at our context size, unless it uses the Opus 5 mid-conversation system-message hatch, which its README does not mention. Also: caveman and headroom send telemetry by default; caveman's proxy is BSL-1.1; both would sit on the subscription's traffic.

Why this points back here: the critique's own model is T(n) = nb + d n(n+1)/2 -- the quadratic term, turns in a growing context, dominates. Compressors trim d; the handoff resets n. Simulated saving 37-57% versus compressors' 0-6% ceiling. Not adding any of them now. Gap this exposed in our own rigor: shunt and graphify also have no paired cost-per-successful-task test yet (shunt-report section 3 measures sessions, not tasks); a SHUNT_DISABLE=1 paired run on the same tasks would close it.

*Decision · 2026-09-15 · on task [160](tasks/160.md)*

## <a id="161"></a>161. Why this sits above everything in the rigor plan. Measured 2026-09-15 on the Claude Code transcripts of every profile…

Why this sits above everything in the rigor plan. Measured 2026-09-15 on the Claude Code transcripts of every profile, week 2026-09-08..09-15, with Opus 5 billing weights (1h cache write 2x, cache read 0.1x, output 5x).

Where the cost is: cache reads are 70% of it, because the whole context is re-sent on every call. Median context per call is 444k tokens (p90 892k), and 3 sessions made 82% of the week. Against that, ekko's own footprint since it became an MCP server is small: the hook's prime is 0.15-0.24% of cost (9 injections in 7 sessions, avg 3,960 chars, \~35 later calls each) and tool results 0.25-0.40% (search is the heaviest, avg 3.8k chars). The rigor plan's 57% cut on what the agent reads is therefore worth about 0.2-0.4%.

The lever: a simulated policy that clears the session when context passes a cap and resumes from the system floor plus prime plus re-reads, paying a full cache write and 2 re-orientation calls per reset. Cap 250k with a 40k resume saves 49% (one reset per \~88 calls); even a heavy 80k resume at cap 400k saves 37%. The model reproduces 90% of the billed cost. The saving only holds if what the conversation knew survives the clear -- that is this task.

Shape to decide: (1) where it lives -- a note attached to the in-progress task, marked as a handoff so prime shows only the latest one per task; (2) who writes it -- an MCP prompt, the same surface as the plan's 4.2 resume/plan prompts, plus server instructions telling the agent to hand off before /clear or once context is large; (3) what it holds, under a budget -- where it stopped, decisions and their reasons, files and lines touched, open questions, the next concrete step; (4) prime on startup and clear shows it under the in-progress task, inside the plan's 0.2 budget: real primes are already 5,934 chars (ekko) and 6,147 (winwayland) of the hook's 10,000 cap, and a truncated prime breaks exactly this resume.

Acceptance: in a fresh session after /clear, the agent continues the task from prime plus the handoff without re-reading more than the files the handoff names; measured on the transcripts before and after (context per call, cost, resets).

Pairs with rigor plan items 0.2 (prime budget) and 0.7 (hook that knows how the session started), neither on the board yet. Plan: artifact 'Plano de rigor do ekko'.

*Decision · 2026-09-15 · on task [160](tasks/160.md)*

## Replaced

### <a id="759"></a>759. Settled by 259's paired test, closed at the user's word on 2026-09-26: the handoff stays, and Claude Code's own…

Settled by 259's paired test, closed at the user's word on 2026-09-26: the handoff stays, and Claude Code's own compaction at \~160k is the mechanism for a session that runs alone. Measured (notes [752](tasks/259.md#752), [753](tasks/259.md#753)): per task the handoff ritual and the compaction cost the same within the noise (B -3% of C's mean, the two C runs 37% apart), the compaction's answer was at least as good (the blind judge slightly for it; one ritual run broke nix build), and a compaction reset cost \~0.31M against \~0.55M for a handoff reset. The handoff is kept because it alone carries context across sessions, profiles and days, and writing it is cheap (\~0.11M): the user's point. To compact at T pass --autocompact T+33k (gotcha [617](gotchas.md#617)), and on default turn autoCompactEnabled on (gotcha [745](gotchas.md#745)).

*Decision · 2026-09-26 · on task [747](tasks/747.md) · replaced by [765](#765)*

### <a id="372"></a>372. Decided by the user, 2026-09-22: ekko is Claude Code's long-term memory, and Claude Code's auto memory stays off in…

Decided by the user, 2026-09-22: ekko is Claude Code's long-term memory, and Claude Code's auto memory stays off in every profile, as \~/NixOS/modules/usr/ekko.nix already sets (CLAUDE_CODE_DISABLE_AUTO_MEMORY=1, note [353](tasks/337.md#353)). Rejected: splitting by subject, which would turn auto memory back on outside projects with a board. Still open on this task: which of the seven proposals to build, in what order (note [340](tasks/337.md#340)'s suggestion), and whether to spend quota on the planted-gotcha eval.

*Decision · 2026-09-22 · on task [337](tasks/337.md) · replaced by [410](#410)*
