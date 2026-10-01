<!-- Written by ekko docs from the board of ekko: change the board and run it again, since edits here are overwritten. -->

# Other notes

Notes on no task, newest first: what was written about the project as a whole, and the questions asked about it.

## <a id="843"></a>843. A 805 e a 770 estão fechadas. O bypass fica sempre ligado, com as travas do ctx e do ekko verificadas ao vivo, e o…

A 805 e a 770 estão fechadas. O bypass fica sempre ligado, com as travas do ctx e do ekko verificadas ao vivo, e o primeiro gatilho real (campo Status) está ligado. O que eu pego agora? A 540 só pode rodar a partir de 08/10.

Options:

- 829, 827 e 839 (Recomendado): As três sobras da 805, todas no ekko e numa release só. A 829 primeiro: uma versão antiga apaga os campos novos de uma pergunta, e a correção tem de sair antes do próximo campo. A 827 faz o guard ler o quadro do EKKO_DIR. A 839 faz o modelo ler as duas razões quando ctx e ekko recusam juntos.
- 808, janela de 5h no ctx: O modelo passa a ver o uso da janela e para de começar trabalho novo em 85%. Antes do código, há quatro pontos para você decidir: recusa ou aviso, uma variável ou duas, limite semanal, e como distinguir um prompt agendado.
- 811, links entre projetos: O MCP passa a ler e escrever quadros vinculados, sem fundi-los. É a maior das três: muda os schemas das ferramentas, e há seis pontos de desenho para decidir.
- Parar por aqui: Deixo a passagem de sessão escrita e paro.

**Answer**, 2026-09-27: 829, 827 e 839 (Recomendado)

*Question · 2026-09-27*

## <a id="660"></a>660. v0.22.1 is live: both servers on pts/0 run it. What next?

Options:

- 638, the priority field's schema: Small: create's and update's schemas say 3 is the most urgent and 1, the default, the least. It changes the tool definitions, and so the prefix every session pays: on release, every session's cache is rebuilt once.
- Forget winwayland (recommended): ekko --project winwayland --destroy: it is registered, its folder /projects/winwayland is gone, and it has no copy to restore. One command, nothing else touched.
- Stop here for now: The board keeps where things stand; the next session picks up from it.

**Answer**, 2026-09-25: 638, the priority field's schema

*Question · 2026-09-25*

## <a id="654"></a>654. v0.22.0 is live: both servers run it, and ekko's board has its copy (a hard link). What next?

Options:

- 623, the flaky test (recommended): commits_name_their_tasks_and_context_lists_them failed once; the guess is git log --since cutting at the second the item was created. Small, and it touches no prefix.
- 638, the priority field's schema: Small, but it changes the tool definitions, and so the prefix every session pays: better batched with other prefix changes.
- Forget winwayland: ekko --project winwayland --destroy: it is registered, its folder is gone, and it has no copy.
- Stop here for now

**Answer**, 2026-09-25: 623, the flaky test (recommended)

*Question · 2026-09-25*

## <a id="639"></a>639. With 495 cancelled, three tasks are ready: 503, 638 and 623. Which next?

Options:

- 503, protect each board from its own folder (recommended): The one real risk on the list: git clean -fdx, or removing a project's folder, takes its board and history/ with it (gotcha 498). I would read gotcha 498 and question [502](#502), then bring you a design to approve: where the copy lives, when it is taken, how many are kept, how one is restored.
- 638 and 623, the two small ones: 638: the priority field's schema says which way it goes. 623: find why commits_name_their_tasks_and_context_lists_them failed once (the guess: git log --since cuts at the second the item was created). Both would ship in the next release.

**Answer**, 2026-09-25: 503, protect each board from its own folder (recommended)

*Question · 2026-09-25*

## <a id="636"></a>636. Task [623](tasks/623.md) is at priority 3, set by a session that meant 'minor'. In ekko, 3 is the most urgent (gotcha [634](gotchas.md#634)), so a flaky…

Task [623](tasks/623.md) is at priority 3, set by a session that meant 'minor'. In ekko, 3 is the most urgent (gotcha [634](gotchas.md#634)), so a flaky test seen once tops the ready list. What should I do with it?

Options:

- Lower 623 to 1 and open a task for the schema (recommended): The task: the priority field's schema says '3 is the most urgent; 1, the default, the least', a few bytes, so no session reads it backwards again.
- Only lower 623 to 1
- Leave it at 3

**Answer**, 2026-09-25: Lower 623 to 1 and open a task for the schema (recommended)

*Question · 2026-09-25*

## <a id="635"></a>635. Which task next? My order, best first: 495, 503, 623. 525, 526 and 259 belong to the trabalho session and stay out.

Options:

- 495, ekko under another MCP client (recommended): Run ekko under Codex CLI, Gemini CLI or OpenCode and list what works only in Claude Code: the SessionStart prime, the tasklist hooks, the plugin, alwaysLoad, the deny lists, form elicitation, the menu. Each gets a verdict: move it behind MCP, give it a stand-in, or keep it on purpose. It follows decision [484](decisions.md#484).
- 503, protect each board from its own folder: Keep a copy of .ekko/ outside the project's folder, since git clean -fdx or removing the folder takes the board and its history/ with it (gotcha 498); and --projects offers to drop a project whose folder is gone. Design first: where the copy lives, when it is taken, how many are kept, how one is restored.
- 623, the flaky test: commits_name_their_tasks_and_context_lists_them failed once in about 11 runs. The guess: git log --since cuts at the second the item was created. Small.

**Answer**, 2026-09-25: ekko não precisa rodar em outros agentes, somente no claude code, pode fechar essa, pq no momento só utilizo  claude code, quando eu começar a pensar em utilizar outros tbm, ai sim iremos dar suporte

*Question · 2026-09-25*

## <a id="609"></a>609. Qual tarefa pego agora? As três estão prontas e são todas p1. Recomendo a 503 porque ela evita uma perda que não tem…

Qual tarefa pego agora? As três estão prontas e são todas p1. Recomendo a 503 porque ela evita uma perda que não tem volta: um git clean -fdx na pasta apaga o .ekko/ junto com todo o histórico do board. Além disso, o escopo dela é fechado. Sugiro fazer a 495 antes da 389, porque a 389 depende justamente dos hooks do Claude Code que a 495 vai listar. O codex já está instalado para testar a 495.

Options:

- 503 - proteger o board (Recomendado): Guardar uma cópia do .ekko/ de cada projeto fora da pasta dele, e o --projects oferecer tirar da lista um projeto cuja pasta sumiu. Antes de construir, te proponho onde a cópia fica, quando é feita, quantas ficam guardadas e como restaurar.
- 495 - ekko em outro cliente MCP: Rodar o ekko no Codex CLI, que já está instalado, e listar o que só funciona no Claude Code, com um veredito para cada item: passar para o MCP, criar um substituto para outros clientes ou manter no Claude Code de propósito.
- 389 - espera entre sessões: É o desenho maior: como uma sessão registra que está esperando por outra e como fica sabendo quando pode seguir. Rende mais depois da 495.

**Answer**, 2026-09-24: 389 - espera entre sessões

*Question · 2026-09-24*

## <a id="502"></a>502. Record the board's protection as a task, to be weighed later (nothing is built now)? (1) A copy of each project's…

Record the board's protection as a task, to be weighed later (nothing is built now)? (1) A copy of each project's .ekko/ kept outside its folder: git clean -fdx or removing the folder takes the history with the board (gotcha 498). (2) ekko --projects offering to drop a project whose folder is gone (winwayland shows 'missing').

Options:

- Yes, one task for both (Recommended): One task holding (1) the copy outside the folder and (2) dropping a missing project.
- Only (1), the copy: Record just the copy outside the folder; leave the missing-project cleanup out.
- No: Record nothing; the question is not raised again.

**Answer**, 2026-09-24: Yes, one task for both (Recommended)

*Question · 2026-09-24*

## <a id="400"></a>400. ASSESSMENT, session e3011485, 2026-09-22, asked by the user: does ekko have more potential, for large projects, single…

ASSESSMENT, session e3011485, 2026-09-22, asked by the user: does ekko have more potential, for large projects, single sessions and several sessions with several agents, and did its token cost pay off? Measured on this session's transcript: 199 API calls, 5.34M units, weighting 2 per token written to cache, 0.1 per token read and 5 per output token.

TOKENS. ekko cost about 9% of the bill: tool replies 5.6% (38 results, 56k characters) and SessionStart hook output 3.8% (three full primes of about 8k characters: at the /clear and at two resumes). Bash was 15%, the session's own output 17%, the rest re-reading context. That is above note [319](tasks/312.md#319)'s 1 to 2%, because this session was about ekko itself: the audit, the release, and a retrieval check with about ten searches (note [399](#399)). What it saved weighs more. The previous session (dccf828e) ended at 390k tokens of context, and this one started at 42k after the /clear, from the handoff; without it, each of the 199 calls would have re-read about 350k more, some 7M units, more than this session's whole bill. The rigor that paid: segmentation, and a stable prefix (v0.15.0 left the instructions and tool definitions byte-identical, so resumes kept their cache; six rewrites once cost 9.6% of the handoff era, note [275](tasks/275.md)). Against it: this session reached 366k (mean 203k) without segmenting. The trigger still depends on someone remembering: decision 164's Stop hook was never built.

BY SCENARIO. One long session: delivered; 'continue' resumed a tangled state, and every restart gave the tasks back. Several sessions, one person, one repo: works, with friction; two sessions shared a release and a merge without colliding, but the user relayed messages five or six times, and a wait was a hand-written jq loop (389). Several agents in one session (subagents, workflows): not yet; the holder is the Claude Code process (decision [343](decisions.md#343)), so every subagent of a session is one holder, and the hook input's agent_id (note 339) is the way. Large projects: scale holds (prime within 6,000 characters at 5,000 items, create 15 to 100 ms), but a 5,000-step chain gives a 62.9 KB context, and one storage.json rewritten under one lock serializes writers (4 servers measured, not 20). The real limit is retrieval: the check in note [399](#399) found an orphaned note (398), stashed items search cannot see (397), and a session proposing before searching. It weighs more as a board grows.

WHERE THE POTENTIAL IS, by expected return: (1) the right memory at the right moment, a gotcha brought up by the file edited or the error seen (note [338](tasks/337.md#338) (1)), a quality gain nobody has measured: run the planted-gotcha eval (note [340](tasks/337.md#340)) first; (2) the segmentation trigger, the largest token lever; (3) coordination without the user in the middle: waiting on another session (389), and an identity per subagent; (4) the git link (396), and deterministic steps such as the release script (391). What ekko set out to be, a shared board and a memory across sessions, it delivers; the next step is a memory that arrives on its own and stays valid.

*Note · 2026-09-22*

## <a id="399"></a>399. RETRIEVAL CHECK, 2026-09-22, session e3011485, asked by the user before three pains from this session's use of ekko…

RETRIEVAL CHECK, 2026-09-22, session e3011485, asked by the user before three pains from this session's use of ekko went on the board, to see whether retrieval still fails. Each pain was searched on the whole board: search first, then jq over storage.json and archive.json, stashed items included. All three were already there: $N kept in a batch's text (notes [335](tasks/312.md#335), [392](#392) (2)); questions going around ask (357, 371, 374, 382 item 5); the 'Ekko:' commit trailer (338, 340, 341). search found every item the grep found for those terms. What failed: (1) the session proposed before searching -- 392 had been in its context since the resume, clipped to one line of what moved, and decision 374 it had never read; (2) search never returns stashed items (task [397](tasks/397.md)); (3) a note attached to a task left the prime when the task was completed, unaddressed (341 on 264: tasks [396](tasks/396.md) and [398](tasks/398.md)); (4) a query with one word too many hides the items holding only some of its words, since partial matches show only when no item holds them all ('ToolSearch deferred' hid 374). Precedent: note 266, where 'what to improve' was answered in chat and only part reached the board. Registered as tasks [394](tasks/394.md) ($N notice), 395 (ask through a hook), 396 (commit trailer), 397 (search and stashed items) and 398 (closing a task with notes that still propose).

*Note · 2026-09-22*

## <a id="392"></a>392. FROM SESSION a6b026e6's OWN USE of ekko, 2026-09-22, asked by the user at the end of the day. Beyond what is already on…

FROM SESSION a6b026e6's OWN USE of ekko, 2026-09-22, asked by the user at the end of the day. Beyond what is already on the board (389 waiting on another session, 380 answers on resume, 379 an answer typed with ! counts as the session's, the release script): (1) ekko --sessions lists the same conversation twice when it outlived a process: once running, and once 'ended' with what the old process finished. It should group by conversation. (2) batch's $N names the item operation N created, not the Nth item created; a batch whose first operation is a set_state had $1 refused. The message was clear, but it is easy to get wrong. (3) The server instructions stand at 1,984 of the eval's 2,000 bytes, so any new line must replace one; guidance should move to tool descriptions or the prime. (4) Tests built on an Ekko with no actor behave unlike real use, where every hook and server acts as a session: two tests failed only for that. The test helpers should default to a session actor. (5) In a worktree, CARGO_TARGET_DIR=\~/Projetos/ekko/target reuses main's build; procedure [382](procedures.md#382) could say so. (6) To check who holds what, the session read storage.json with jq more than once; ekko --sessions now answers most of it.

*Note · 2026-09-22*

## <a id="357"></a>357. IDEA, the user's, 2026-09-22, kept for some day: a question as a kind of item, or a state -- something asked of the…

IDEA, the user's, 2026-09-22, kept for some day: a question as a kind of item, or a state -- something asked of the user that waits for an answer. Today a question lives in a session's chat or its AskUserQuestion prompt, and is lost when that session clears or stops. On 2026-09-22 session dccf828e's four questions (the merge of auditoria, note [336](tasks/312.md#336)'s fixes and rules, task [337](tasks/337.md)'s memory split) existed only in its prompt, and one went stale before the user answered, because main moved in between. As an item it would say who asked (the session, with owners), about what (a task), against which state (the commit, the board revision), and hold the answer, recorded by whoever gets it, which closes it. The prime could list the open ones under 'Waiting on you', and an answer given in another session would still reach the one that asked. It belongs to the multi-agent work, which took over 352's item 5: questions are how several sessions and the user coordinate.

*Note · 2026-09-22*

## <a id="324"></a>324. Cancelled 2026-09-22 along with --calendar (321), at the user's word that the calendar no longer makes sense.

*Note · 2026-09-22*

## <a id="285"></a>285. FINAL ANALYSIS of ekko 0.11.0 (470edb7), 2026-09-21, session 2c09b5f1: what was tested, what holds, what breaks, and…

FINAL ANALYSIS of ekko 0.11.0 (470edb7), 2026-09-21, session 2c09b5f1: what was tested, what holds, what breaks, and what to do, by expected value. Tasks and notes on @analise-final-ekko; scripts, raw results, a README and the prototype patch in target/evals/analise-final/ (git-ignored). The published report is linked at the end of this note.

TESTS. cargo test 268 of 268, clippy -D warnings clean, rustfmt drift 734 hunks; semantics.py 27 of 27; scale.py --full within its targets (prime at most 100 ms at 20,000 items); hooks 2 to 3 ms each. Against independent implementations: next's order on 60 random boards, 1,292 positions, 0 mismatches; search's BM25 over 30 queries, 323 hits, 0 mismatches. 300 SIGKILLs mid-write: no torn file, no acknowledged write lost.

BROKEN, each reproduced: the cursor race (263: a session following changes misses 100% of another session's writes when polling back to back, 2.5% at 100 ms, and 48% of primes under concurrent writes hand out a cursor ahead of their board); the revision going back when counters.json is lost; prime past the hook's 10,000 characters from an unbounded Needs attention; changes with no limit; --clear losing the item when the archive write fails; the lock-timeout message saying to delete the lock; lock waiters starving (max 1.6 s); roots offering a stashed task (257); --phase creating in two writes; whole-value writes dropping another session's change; search missing inflections (13 of 18 plural or -ing queries find under half). A patch for the race, the healing and the starvation passes 269 tests and clippy, with a regression test that fails before it (missed 40 of 40) and passes after.

MONEY, Opus 5 weights, from the transcripts. ekko's own bytes are 0.1% to 3% of the cost and the requests reading its results back 1% to 12%, the high end on a day spent working on ekko itself: the formatting of replies is no longer the lever. The lever is when a session hands off: by the segmentation model, at \~160k of context instead of the median 356k the handoffs came at, about -20% per call, more for longer segments, less if resets lose turns (259 measures that). Restarts after releases cost 9.6% of the handoff era.

MULTI-AGENT. Four Claude Code sessions had ekko servers in this folder during the analysis. Storage holds under concurrent writers: no lost write, no id or uid collision, no torn file. What breaks sits above it: the cursor race, next ranking another session's in-progress task first, the newest handoff on the board shown as every session's own, and whole-value writes. Each MCP server already has CLAUDE_CODE_SESSION_ID in its environment, so owners (264) need no new protocol.

REPORT: https://claude.ai/artifact/MEdzU6tEQCMzU5F7LxqSyR ('Auditoria final do ekko', in Portuguese; private to the user until shared).

UPDATE 2026-09-21, session 9e449e1b: checked by experiment and, at the user's word, applied uncommitted: 263, 269, 279, 268, 276, 280, 281, 282, 270, 271, 257, 275, 278. Corrections to this analysis on 268, 269, 279 and 284 are in the notes there and in 296 and 297. Handoff on 265.

*Note · 2026-09-21*

## <a id="256"></a>256. Approved by the user on 2026-09-21 ('pode seguir'): set this task to waiting once no ekko 0.10.2 MCP server is running…

Approved by the user on 2026-09-21 ('pode seguir'): set this task to waiting once no ekko 0.10.2 MCP server is running any more. At the moment the session on pts/0 (09:34), which is working on 180, still runs one (note [255](tasks/180.md#255)). Check with ps -eo lstart,args | grep 'ekko --mcp' that every server is ekko-0.11.0, and only then set_state waiting (decision [232](decisions.md#232), item 7).

*Note · 2026-09-21*

## <a id="221"></a>221. WHEN. Only once Claude Code resolves a mention of a plugin server's resource: the colon trap in gotcha [218](gotchas.md#218). Check each…

WHEN. Only once Claude Code resolves a mention of a plugin server's resource: the colon trap in gotcha [218](gotchas.md#218). Check each new Claude Code version's changelog, or in about 2 minutes with evals/claude-code/probe_server.py packaged as a plugin (plugin.json with inline mcpServers, as plugin/.claude-plugin/plugin.json does; a separate .mcp.json under --plugin-dir never started). The test: @plugin:&lt;plugin>:&lt;server>:item://93 picked from the menu attaches (an mcp_resource attachment in the transcript). The \\b trap does not matter here: prime://board ends in a letter.

WHY. Decided with the user on 2026-09-21. Two servers are only the workaround while the bug lasts. Today the plugin's server (ekko --mcp) holds the tools and prompts, and ekko --mcp --resources, registered outside the plugin as 'ekko', holds the mentions. One server in the plugin is the better end state. Installing the plugin gives everything, with no claude mcp add. It is one process per session. And list_changed goes out right after the session's own writes. The longer mention, @plugin:ekko:ekko:item://93, costs little: typing @ekko filters the menu, and the item is picked from it.

THE CHANGE. src/mcp.rs: Mode::Board offers resources again; resources/list, resources/templates/list and resources/read are no longer gated by mode; offer() declares resources {listChanged: true} beside tools and prompts. Keep the POLL thread for the board server, since a change from a terminal or --ui reaches no call of the session. Drop --resources (src/cli.rs, src/main.rs usage) or keep it as an alias, and fold its test in tests/mcp.rs into one server. readme.md: the Mentions paragraph. \~/NixOS modules/usr/ekko.nix: remove usr.claude-code.mcpServers.ekko. The usr.claude-code.mcpServers option can stay for other servers. Close with the user: @ekko typed in a normal session, an item picked, and it attaches.

*Note · 2026-09-21*

## <a id="167"></a>167. Empirical checks of the Claude Code behaviour the rigor plan depends on, 2026-09-15, Claude Code 2.1.272 in print mode…

Empirical checks of the Claude Code behaviour the rigor plan depends on, 2026-09-15, Claude Code 2.1.272 in print mode (claude -p, --strict-mcp-config) against a throwaway stdio MCP server whose request log shows what the client actually called. At initialize the client asks for protocol 2025-11-25 and lists tools, prompts and resources before the first prompt.

(1) 0.8 HOLDS, WITH A NUMBER: server instructions and tool descriptions are both cut at exactly 2,048 characters and end in '… [truncated]'. A marker at character 1,881 arrived and one at 2,131 did not, in the instructions and in a description loaded through ToolSearch, and the model quoted text ending at character 2,048 of 2,589 (ASCII, so characters and bytes coincide here). ekko's instructions are 1,669 bytes after item 1.3.

(2) 4.2 HOLDS IN ONE FORM: an MCP prompt runs as /mcp\_\_&lt;server>\_\_&lt;prompt> -- the client called prompts/get and the model answered with the prompt's text. The /&lt;server>:&lt;prompt> form answered 'Unknown command' in print mode even though prompts/list had run; the docs describe that form as the label the interactive menu shows. For the plugin's server the runnable name would be /mcp\_\_plugin_ekko_ekko\_\_&lt;prompt>, not checked yet.

(3) 4.1 IS NOT CONFIRMED: a prompt carrying @probe:item://93 attached nothing in print mode. With the resource list and read tools disallowed the model answered NONE, and the client never called resources/read. Interactive @ mentions stay untested, so 4.1 must not count on resources removing a call until an interactive session shows it.

The hook cap on SessionStart output (10,000 characters, a 2 KB preview past it) was verified in note [165](#165) and is documented in hooks.md line 941.

*Note · 2026-09-15*

## <a id="166"></a>166. Rigor plan re-baseline at HEAD 5956589, 2026-09-15, measured with the checkout's own release build…

Rigor plan re-baseline at HEAD 5956589, 2026-09-15, measured with the checkout's own release build (target/release/ekko), as note [165](#165) asked. The measurement scripts moved out of /tmp into evals/agent/ (ekko_mcp.py, semantics.py, retrieval.py, scale.py, prototype.py, run.sh; python3 added to the dev shell), and evals/resume/run.sh now finds boards through \~/.ekko/projects.json and reads the default board with --ekko-dir, so a project found from the current folder cannot stand in for it. Work happens in the worktree .claude/worktrees/rigor on branch rigor, apart from the UI session; nothing committed yet.

SEMANTICS (evals/agent/semantics.py): 3 of 26 checks hold -- server instructions byte-identical across writes, instructions 1,541 bytes, every tool description at most 230 bytes, all under the 2 KB cut. The other 23 are open exactly as the plan measured them.

REAL BOARDS: the ekko prime is now 7,341 characters, 73% of the hook's 10,000 cap (5,775 when the plan was written); winwayland 5,951. The largest context response is task [160](tasks/160.md) at 10,359 bytes, its four attached notes printed in full. On a copy of the ekko board: learning why 33 cannot start takes 4 context calls and 1,868 bytes to reach root 97, which the prototype finds in one pass; 'dependency cycle' and 'check cycle' return nothing while 3 and 4 items match every word; 'cycle' returns 8 hits, 4 of them only inside other words such as recycled; search() with no arguments returns 21,870 bytes. The session model comes to 16 calls and 26,787 bytes. Priced with note [165](#165)'s call term (C = 471k, o = 1,439), its calls cost about 869k input-equivalent tokens against about 96k for the tokens it adds -- 9x, so the items that remove calls lead the order.

SCALE is unchanged from the plan within noise: prime 12.1 s and next(10) 3.2 s with 2,000 roots over a chain of 8,000, prime 2.2 s on a chain of 5,000, 4.8 s on ten layers. The Python prototype does every count and the propagation, parse included, in 72-242 ms on the same boards, and orders the priority inversion 1, 5, 3, 4.

DOCS, against note [165](#165): the current Claude Code docs do state the limits 165 lists as undocumented. hooks.md line 941 caps hook output at 10,000 characters; mcp.md line 1401 truncates tool descriptions and server instructions at 2KB each; mcp.md line 1354 gives @server:protocol://resource/path, and the same page lists MCP prompts as /servername:promptname. Downloaded 2026-09-14 and again 2026-09-15, byte-identical. They are tested empirically anyway before 0.8, 4.1 and 4.2 depend on them.

STALE SERVERS: a Claude Code session keeps the ekko MCP server it started with. This session's server still runs the binary from before 7bf8191, so it answers 'No projects yet' and cannot see item 165; only sessions started after the rebuild see projects by folder.

*Note · 2026-09-15*

## <a id="165"></a>165. Review of the rigor plan artifact ('Plano de rigor do ekko', validated 2026-09-15) before any code, by the session that…

Review of the rigor plan artifact ('Plano de rigor do ekko', validated 2026-09-15) before any code, by the session that measured token economics on the transcripts (notes [161](decisions.md#161)-164). Verdict: approve with the fixes below.

VERIFIED. SessionStart hook output limit, tested with claude -p and a throwaway plugin: 9,900 characters arrive whole; 10,100 become '&lt;persisted-output> Output too large ... Preview (first 2KB)'; 9,000 accented characters (17,981 bytes) arrive whole, so the limit counts characters, not bytes. The hooks docs state no limit, but the behaviour is real: 0.2's 9,500 hard cap and 0.7's premise hold, and the installed plugin's SessionStart hook still has no matcher. MCP docs confirm the 10,000-token warning and the 25,000 default (MAX_MCP_OUTPUT_TOKENS), with over-limit results saved to a file; a tool can declare its own limit with anthropic/maxResultSizeChars, which 0.3 should use. NOT in the docs: the 2 KB truncation of server instructions and tool descriptions, the @server:proto://path resource syntax and /server:prompt commands -- test them empirically before 0.8, 4.1 and 4.2 depend on them.

MUST FIX BEFORE CODE. (1) Re-baseline at HEAD 5956589: the plan measured c53e376, and 7bf8191 (projects by folder) plus the UI commit changed agent.rs, ekko.rs, main.rs, mcp.rs, render.rs and storage.rs, so line references are stale -- locate by symbol, and measure with the HEAD binary, not the installed one (note 157). (2) Move mcpclient.py and s1-s4 from /tmp/claude-1000/-projects-ekko/7f421024-ecaa-4628-8474-01a364f04a1c/scratchpad/ into evals/ first: /tmp does not survive a reboot and they are the only way to check the acceptance table. evals/resume/run.sh still globs \~/.ekko/projects/\*, the layout 7bf8191 retired. (3) 3.4 no longer waits on 94: 94 decided .ekko/ stays out of git, so a never-recycled counter is enough and hash ids are not needed.

REPRIORITISE WITH THE CALL TERM. The plan values tokens entering context at 2 + 0.1T + 2R each, but every avoided tool call is also an avoided request that re-reads the whole context: 0.1\*C + 5\*o, about 54k input-equivalent units at the measured 471k average context. In the plan's model session the 7 saved calls are worth \~380k units against \~53k for the 5.9k saved tokens, about 7x. Items that remove calls -- 1.3 nowReady in write responses, 1.6 roots in one call, 2.1 first-hit search, 2.3 several items per call, 3.3 conditional reads, 4.1 resources -- carry more than the table shows; add calls per question to the Metas.

OTHER. Skip the provisional 0.6 cursor and do 3.1 (rev) early, so 0.7 builds on rev rather than on a format that gets replaced. Keep server instructions and tool descriptions byte-stable within a session: in the transcripts a full cache rewrite followed an mcp_instructions_delta attachment, so nothing dynamic (counts, dates) goes in them. Defer pure-performance items (1.1 CSR cache, 1.4 for speed, 2.4 knapsack, 2.5 RRF) until an eval on real boards shows need -- today \~52 tasks answer in 1-4 ms -- and keep 1.2 (single parse, fsync) as correctness. The plan lacks the largest lever, task [160](tasks/160.md) (handoff), which builds on 0.2, 0.7, 2.2, 2.3 and 4.2; see notes [161](decisions.md#161), [163](decisions.md#163) and 164. Another session is working on the UI in the same repo: use separate worktrees, and coordinate the storage changes (1.2, 3.1, 3.2), since the UI writes the same storage and the 1.1 cache key assumes every writer renames.

*Note · 2026-09-15*

## <a id="156"></a>156. Agent frontend landed 2026-09-14, the second step of the new direction: ekko stopped being a skill and became an MCP…

Agent frontend landed 2026-09-14, the second step of the new direction: ekko stopped being a skill and became an MCP server. 7cfbfa8 added --prime (the resume view: in progress, ready in next order, blocked, reasons attached under their work, recent notes, what needs attention, a cursor), --next (a lexicographic order: in progress, phase, priority, due, open work waiting downstream, age) and --context (one item, one hop along every relation); prime reads the project named after the repository it runs in. The resume eval gave prime full coverage of doing, ready and attached reasons in 2 to 6 KB on all three real boards, against 77 to 99 KB for the board view. 35e36e5 added structured writes (create with every field apart from the text, edit by one exact replacement or an append with an updatedAt precondition, update, link, batch with $N references written once and all or nothing, changes since a cursor), ekko --mcp (dual-era: 2026-07-28 per-request \_meta and server/discover, legacy initialize back to 2024-11-05; 16 tools, no clear or destroy, refusals as CODE: message) and plugin/ (the MCP server plus a SessionStart hook running ekko --prime; the flake output packages.plugin pins the binary). The skill was removed. 122, 123 and 124 are answered by context, batch and edit; 121 only for agents, whose arguments never pass through a shell. The first real session found refusals over MCP naming CLI flags and ids that existed only in a draft; c53e376 words them for agents, naming tools instead of flags and the items a refused batch would have created by operation. NixOS: modules/usr/ekko.nix now links packages.plugin into the four profiles as a skills-directory plugin; it takes effect after nix flake update ekko and a rebuild, and /mcp should then list ekko.

*Note · 2026-09-14*

## <a id="155"></a>155. Rigor pass landed 2026-09-14. 56855fe: a task is exactly one of five states (State enum, one canonical write, a total…

Rigor pass landed 2026-09-14. 56855fe: a task is exactly one of five states (State enum, one canonical write, a total transition function, exhaustive tests); one total everywhere, leaving cancelled out (board counter, roadmap, --projects, which also stops counting stashed and trashed items); --list paused. 342cbd4: one dependency rule, completed work never waits on open work, checked on the board a command would leave against the board before it, refusing only the pairs the command breaks: BLOCKED when completing, COMPLETED_DEPENDENTS when reopening (reviving a cancelled blocker included; --force overrides and says so), ALREADY_DONE when --blocked-by gives done work an open blocker (never forced). A blocker and its dependents can be completed or reopened together in one command, and pairs left by an earlier --force do not fail later commands. --restore and --untrash are recovery and deliberately unchecked, so the coming prime view should surface any broken pair they bring back. Property test: 500 random commands, rule holds after each; disabling the check fails it and 9 others. PHASE_ORDER is refused at --blocked-by, and --roadmap lists the inversions a later --phases reordering leaves (reordering is never refused). The cycle check is a visited-set walk over a uid index: a 40-diamond chain answers at once (it was 12.5 s at 26, doubling per diamond). Not done: refusing notes as blockers or as blocked items at write time, harmless today because notes never hold.

*Note · 2026-09-14*

## <a id="152"></a>152. Rigor audit of the core model, 2026-09-14 -- the first step of the applied-mathematics direction, where the human side…

Rigor audit of the core model, 2026-09-14 -- the first step of the applied-mathematics direction, where the human side becomes the UI and TUI and the agent side gets an improved mathematical backend. Six findings, all reproduced on scratch boards the same day: the state space, the two totals, the dependency invariant after a reopen, phase order against dependency order, paused missing from --list, and HERE marking every phase with work in progress rather than one point -- that last one already lives in #137, with its measurements in #138 (two HEREs on winwayland, none on minium). The state, totals and paused findings share one cause: each surface decides by itself what the data means. The direction the audit recommends is to define the model once -- state as a type with five values, one transition function, one definition of a total -- and derive every view from it: the CLI, --json, the UI, and the graph view the UI is going to get.

*Note · 2026-09-14*

## <a id="139"></a>139. Vocabulary settled on 2026-09-14, replacing the parts of note [127](#127) it touches. ROADMAP is the project's timeline: its…

Vocabulary settled on 2026-09-14, replacing the parts of note [127](#127) it touches. ROADMAP is the project's timeline: its declared phases in order, what is behind, where work sits, what is ahead. The command is --roadmap, which was --path. BLOCKED BY is the dependency, said the way the industry says it: task A is blocked by task B. --blocked-by @A B records it, --list ready and --list blocked read it, and a blocked task cannot be completed while a blocker is open; --force overrides that, and only on a person's word. ATTACHED TO is a note placed under the task it explains: --attached-to @note task, which was --anchor. ANCHOR IS NO LONGER A WORD IN EKKO. It had meant binding a board to a repo (notes 62 to 66), then a note under a task, and was about to mean a dependency as well; the old flag now answers with RENAMED_FLAG. --timeline stays what taskbook made it, the view grouped by creation date, and has nothing to do with the roadmap. ITEM, TASK, NOTE, BOARD, STASH, TRASH and ARCHIVE keep their meanings from note [127](#127). PROJECT and BOX, the containers, are parked with the rest of the backlog (note 134).

*Note · 2026-09-14*

## <a id="127"></a>127. Glossary, settled with the user on 2026-08-27 and never written down until now -- which is exactly why task 97 spent a…

Glossary, settled with the user on 2026-08-27 and never written down until now -- which is exactly why task 97 spent a day saying "box" for the concept that became "stash". An ITEM is a line, and is either a TASK (work, has state, counts in the percentage) or a NOTE (not work, no state, outside the percentage). A BOARD is the @something grouping; an item can be on several. A PROJECT is a container bound to a repo; a BOX is the free-standing equivalent -- same shape, different binding, and that split is what note 92 decided. STASH is putting something away without changing it; TRASH is removal with an expiry; ARCHIVE is the permanent record of what got done. The one that keeps getting inverted is task versus board: "task" is the line, not the @thing. Both of us had it backwards at different points, and the user said so in as many words.

*Note · 2026-08-28*

## <a id="126"></a>126. What an agent lacks, from a full session of first-hand use rather than speculation. Ranked by evidence, and the first…

What an agent lacks, from a full session of first-hand use rather than speculation. Ranked by evidence, and the first one is not inconvenience, it is DAMAGE. (121) MEASURED: this board holds 120 items, roughly 55 of them notes I wrote in English, and it contains ZERO apostrophes. The proof is in the text -- "users own" appears four times, plus "users project", "users shell", "ekkos cli", "its own". I dropped every apostrophe deliberately to avoid fighting shell quoting, so the interface degraded the content. Reading a description from stdin fixes it and costs nothing. (122) MEASURED: looking at one item costs 83KB of whole board, or 1.2KB via --find using a substring you must already know, which is circular. The skill now tells agents to carry a uid across turns; there is no cheap way to look at what one points to. (123) Closing a task and writing the note that explains it are two processes and two calls. Not atomic: interrupted between them, the board carries a closed task with no reason, which is the one thing this tool exists to prevent. (124) --edit takes the whole new description, so fixing one word in a 900-character note costs the note again -- and it compounds with 121, since that long text also goes through argv. (125) is the axis with:NAME deliberately left out: who WROTE an item versus who it is with. Worth having when a human and an agent share a board and one comes back to it; noted during the 87 design as a separate question and then not registered, which is the pattern note [74](#74) keeps naming.

*Note · 2026-08-28*

## <a id="119"></a>119. Item 116, found while trying to measure due dates for the calendar and getting a wrong answer from ekko itself. `ekko…

Item 116, found while trying to measure due dates for the calendar and getting a wrong answer from ekko itself. `ekko --list due` returned five items and NONE of them had a due date -- because the board is named @due, and list_by_attributes checks stored_boards BEFORE is_known_attribute. Verified on a scratch board: with a board called @due present, `--list due` and `--list @due` both return the board, and there is NO spelling that reaches the attribute. It is not shadowing, it is unreachable. AFFECTED NAMES, any of which as a board kills its filter: due, overdue, done, pending, progress, ready, blocked, star, task, note, cancelled. THE SHAPE OF THE FAILURE is the one this codebase keeps rejecting by name: a plausible wrong answer instead of an error. --list already errors on a term it does not recognise, precisely so a typo cannot silently return everything; a term it recognises as TWO things and silently picks one of is the same defect wearing a different hat. FIXES WORTH WEIGHING: error on the ambiguity and make the caller disambiguate; or reserve attribute names so a board cannot take one, which breaks existing boards including this one; or give boards a spelling attributes cannot have. Note that the current code already treats bare and @-prefixed as identical, so the @ cannot be the disambiguator without changing what --list @board means today.

*Note · 2026-08-28*

## <a id="115"></a>115. Found by the user testing --ui within an hour of it shipping: two stray Tab presses while scrolling turned items 37 and…

Found by the user testing --ui within an hour of it shipping: two stray Tab presses while scrolling turned items 37 and 38 from done back into in-progress, two seconds apart. Restored with --set @37 @38 done. WHAT ACTUALLY WENT WRONG is worse than a wrong state -- setting "progress" clears isComplete by definition, so a navigation key DESTROYED a terminal state rather than cycling within the active ones. I wrote Tab thinking only about a pending task and never asked what it does to a finished one. THE FIX for 113: Tab and Enter leave done and cancelled alone. Those are terminal by design -- the whole argument for the cancelled state was that a decision to stop should survive -- and resurrecting one by keystroke is the opposite. THE FIX for 114 is separate but the same root: the UI reloads in silence, so the only evidence a write happened is the screen redrawing. The prompt line has room to say what just changed for a moment. WORTH NOTING ABOUT THE SHAPE: the CLI has never had this problem because every mutation there is a command you typed on purpose. A picker turns navigation and mutation into neighbouring keys on the same hand, which is a class of hazard the CLI half of this tool simply does not have, and the first cut shipped without anyone asking that question.

*Note · 2026-08-28*

## <a id="106"></a>106. Anchoring shipped in 29e093a. --anchor @&lt;note> &lt;task> renders the note indented under its task; --anchor @&lt;note> with…

Anchoring shipped in 29e093a. --anchor @&lt;note> &lt;task> renders the note indented under its task; --anchor @&lt;note> with no target clears, designed in rather than found missing, which is the whole lesson of the --blocked-by fix two commits earlier. FOUR RULES, each narrowing it on purpose: only a note can be anchored, because a task under a task is a subtask and answering "whose total does it count toward" by accident is worse than not having the feature; only to a task, because a note under a note allows chains and so cycles, refused by SHAPE rather than by a check somebody has to remember; stored by uid, since a reason pointing at a recycled number would end up explaining different work; and a note whose task is on another board stays where it was filed rather than jumping boards, because surprising placement beats an un-nested reason. Unanchored notes render exactly as before, so the goldens never moved -- the same invisible-until-used shape as due dates, paused and cancelled. WORTH KEEPING: the reorder and the indent needed SEPARATE tests. Removing the indent left all 133 other tests green, and it is the visible half of the feature. The reorder is what the data does and the indent is what the reader sees, and only one of them had anything asserting it.

*Note · 2026-08-28*

## <a id="105"></a>105. Both shipped in 686a963. Clearing: --blocked-by with no blockers now unsets, which made TWO layers of dead code…

Both shipped in 686a963. Clearing: --blocked-by with no blockers now unsets, which made TWO layers of dead code reachable at once -- the None branch in set_blocked_by and the "Item N waits on nothing" message that render.rs had been carrying unused. Nobody wrote either as dead code; they were both written correctly for an input path that refused to deliver. Uids: resolved inside validate_ids, so all twelve callers gained it in one change, --restore included -- which is where the risk was highest, since the archive renumbers and a uid survives it. reaches() had been doing the same lookup by hand and both now share find_by_uid. THE PART WORTH REMEMBERING: neither was a discovery. Note [91](#91) recorded the clearing gap a day early and it never became a task, and the uid advice was in a skill I wrote without ever checking the binary honoured it. Note [74](#74) named this pattern as a change reaching one surface and not its siblings; both of these went further, reaching the documentation and never the code. What caught them was somebody else using the thing under load.

*Note · 2026-08-28*

## <a id="104"></a>104. Where I read the report differently, which is item 102. It asked for a new FIELD for progress detail. The real gap is…

Where I read the report differently, which is item 102. It asked for a new FIELD for progress detail. The real gap is that notes have no ANCHOR: a note lives on a board, not on an item, so "the reason for task 12" is a loose note nearby and nothing connects them. That is why it got the two bad options it described -- a four-line description or a split pair. The mechanism already exists, since --blocked-by references items by uid and a note could do the same. Anchoring fixes THREE of its complaints at once: the anchored note IS the detail field it wanted; it renders nested under its task rather than as a sibling, which removes the wall of text the human sees; and the [x/y] counter stops reading as ambiguous because notes stop competing for sibling lines. WHAT I WOULD NOT TAKE: a mandatory short summary on every note -- folding already produces the one-line view automatically, and a required field nobody fills well is exactly what the readme rejects for cancellation reasons. And --path being useless to an agent is not a defect, it is the design: two audiences, different views, which is why --list ready exists and why that is the one it reached for. ITS BEST OBSERVATION, which is not a feature request: the board made it disagree with itself. Rereading a task before working it, it saw the premise had died and rewrote instead of executing. Without a durable place for the task to sit still, it would simply have done what the task said.

*Note · 2026-08-28*

## <a id="103"></a>103. First external evidence of ekko under load: an agent ran a whole winwayland session on it -- \~100 tasks and 103 notes…

First external evidence of ekko under load: an agent ran a whole winwayland session on it -- \~100 tasks and 103 notes -- and was asked for criticism. What it praised: notes were the biggest win and NOT the declared goal, because the board became where the REASON lives and that outvalues task tracking; live-evaluated dependencies, with --list ready as its most used command; idempotent --set, which saved it from real error because it repeated commands; cancelled outside the denominator, letting a board close at 100% without lying. What it hit, verified here before recording. (1) NO WAY TO CLEAR A DEPENDENCY -- its single biggest ask. It blocked an item wrongly, could not undo it, and had to write "this dependency is wrong and ekko will not let me clear it" INSIDE the task description, which it called a structural lie only a human will undo. Note [91](#91) found this a day earlier and it never became a task. (2) uid IS UNACTIONABLE: verified, --set and --edit both reject one with "Unable to find item with id". The skill instructs agents to carry uid across turns and no command accepts it, so the advice we wrote is impossible to follow and the agent paid an extra read every time instead. (3) The [x/y] counter excludes notes correctly but nothing on screen says so -- verified, a board with 2 tasks and 1 note prints [0/2] over three lines. (4) Description is the only field, so "what was measured" and "what is left" became four-line descriptions or split task+note pairs.

*Note · 2026-08-28*

## <a id="91"></a>91. Two things 87 must get right at birth, both learned the hard way here. (1) IT MUST BE CLEARABLE. --blocked-by shipped…

Two things 87 must get right at birth, both learned the hard way here. (1) IT MUST BE CLEARABLE. --blocked-by shipped with no way to unset: passing it with no ids errors, and the `if uids.is_empty() { None }` branch in set_blocked_by is dead code that can never be reached. A task can be blocked forever with no way back except delete and recreate. Whatever --with looks like, clearing has to be designed in and tested on day one, not discovered later. (2) TWO ENTRY POINTS, like priority: with:NAME at creation, and a command taking the id to change it afterwards, marked with @ the way --priority and --move already do. ORDERING against 88: the actor is the missing half of waiting -- "waiting" alone does not say when to check back, and "waiting on the vendor" does. So 87 first. Recorded as prose and NOT as --blocked-by, because 88 alone still fixes the concrete defect (--list ready calling an unstartable task ready), which makes this judgement rather than mechanism -- same call as notes 70 and 18.

*Note · 2026-08-26*

## <a id="90"></a>90. Why 87 matters more than "assignment" makes it sound, in the users own words: mark what is mine and what is the agents…

Why 87 matters more than "assignment" makes it sound, in the users own words: mark what is mine and what is the agents, wire a roadmap between them, then say "I demarcated your tasks and linked a roadmap, just go". That makes --list with:agent ready the agents ENTIRE work queue -- what is his, not blocked, in the order the dependency chain allows. It is the composition of three things that already shipped (dependencies, --list ready, and the live evaluation that means finishing one unblocks the next with nothing to run) plus this fourth. Both directions work: the human demarcates for the agent, and the agent marks which items it is claiming so the human can see what is covered. FILTER TERMS that fall out: --list mine (nothing set), --list delegated (anything set), --list with:NAME. Note that "mine" is first-person and there are two firsts -- it must mean "no actor set", so the agent never asks for mine, it asks for with:agent. The skill has to say that or an agent will read the humans queue as its own.

*Note · 2026-08-26*

## <a id="89"></a>89. Item 87 design, settled. SHAPE: an attribute, not a state. Delegated-and-blocked and delegated-but-not-blocked are both…

Item 87 design, settled. SHAPE: an attribute, not a state. Delegated-and-blocked and delegated-but-not-blocked are both real, so collapsing who into a state loses the second -- Maria is writing the migration and I am free to do other things. The waiting state (88) composes with it rather than containing it. SYNTAX: with:NAME, a fourth k:v marker beside p:2 and d:YYYY-MM-DD, stripped from the description the same way. Not @name, which is the universal convention everywhere else and which taskbook already spent on boards -- changing that would break byte parity, so this fights muscle memory and there is no way out. Rejected: assignee:/owner: carry team and permission baggage ekko does not have, by: reads as authorship which is a different axis, for: inverts the direction. VALUES ARE NAMES, NOT ROLE WORDS. "delegated" or "someone else" would name the category and hide the fact, which is exactly what the empty checkbox was doing before the paused state existed -- you cannot chase "someone else". ABSENCE IS THE BOARD OWNER, so you never mark your own work: marking it would give --list mine two spellings that do not match each other, and it keeps every existing board byte-identical. NO REGISTRY, like boards rather than like projects; a typo is caught on READ by the rule that already errors on --list zzznope, so detection costs nothing new. NOT ONLY PEOPLE: with:ci, with:legal, with:vendor -- required, not a concession, because waiting on CI needs the same slot and CI is not a person.

*Note · 2026-08-26*

## <a id="83"></a>83. Both halves of the project-deletion gap closed. --projects now says what each project holds, and --project X --destroy…

Both halves of the project-deletion gap closed. --projects now says what each project holds, and --project X --destroy moves it to \~/.ekko/.trash/&lt;name>-&lt;millis>. Design points worth not re-litigating: the word is destroy and not delete because --delete already means "remove items" and --project old --delete 3 removes item 3 INSIDE old -- one word with two meanings would make a command that lost its ids destroy the whole board while looking like it worked. The trash sits BESIDE projects/ rather than inside, because inside would put every destroyed project in list_projects way until a filter hid it, and that filter would reserve .trash as a name nobody could give a project. It renames rather than copies: atomic on one filesystem, so a project is either listed or trashed and never half of both. It takes the projects own lock first, which is the actual fix for what this note first reported -- flock protected writers from each other and never protected anything from the directory vanishing, because rm -rf never went through ekko. And it does not prompt, because nothing here does and one command that did would break every script driving it; the count in the reply is the confirmation. One race left on purpose: a process already blocked on the lock acquires it after the move and writes into the trashed copy. Nothing lost, it just lands somewhere unlisted.

*Note · 2026-08-25*

## <a id="82"></a>82. What deleting a project actually showed, 2026-08-25. The half that works, works well: the filesystem being the registry…

What deleting a project actually showed, 2026-08-25. The half that works, works well: the filesystem being the registry means --projects reflects reality the instant after rm -rf, with no index to fall out of step, and asking for the gone project errors correctly (exit 1, DIRECTORY_ERROR, message carrying the fix). The half that does not: an item deleted goes to the archive and comes back with --restore, a project deleted exists nowhere -- the asymmetry is not just a missing command, it is a missing recoverability. Nineteen items and five phases went with no count, no confirmation and no way back. And rm -rf takes no lock, so whatever flock protects, it does not protect a project from being removed underneath a writer, because whoever deletes never goes through ekko. Note [81](tasks/81.md) is the smaller half of the fix and stands on its own: showing counts in --projects is what tells you the size before you act, and cli.rs already claims it does.

*Note · 2026-08-25*

## <a id="74"></a>74. The recurring failure in this codebase has a shape, and today it appeared four times. A change lands on one surface and…

The recurring failure in this codebase has a shape, and today it appeared four times. A change lands on one surface and not on its siblings. --set learned cancelled and unstarted in apply_state but not in the match that renders the confirmation, so both wrote correctly and printed nothing. Dependencies landed in display_item_by_board and not display_item_by_date. Six features shipped and the skill that documents them was not touched. And the readme still described the pid-file liveness check that flock replaced -- documentation that was FALSE rather than merely stale, which is the worse kind, because the mechanism it named had been deliberately removed. Removal-driven rot beats addition-driven rot every time: adding a feature leaves docs incomplete, removing one leaves them lying. No mechanism catches this. The match arms cannot be made exhaustive (they are on &str), the two item renderers are separate functions by design, and nothing links a flag to its prose. So it is a habit: when a change lands, ask what ELSE renders, documents or asserts the thing just changed.

*Note · 2026-08-25*

## <a id="73"></a>73. A third bug fell out of fixing 72, in the same lines. display_path padded the already-COLOURED head with {head:width$}…

A third bug fell out of fixing 72, in the same lines. display_path padded the already-COLOURED head with {head:width$}, and Rust counts escape sequences as width, so whenever a phase name was shorter than its own tally the node row got no padding at all and the two rows drifted -- ten columns in the ci / 0/12 HERE case. Widths now come from the plain text with the padding appended outside the colour, which the comment two lines below had always prescribed for the tally. How it hid is the part worth keeping: with colour OFF the padding worked, so it was correct in a pipe and wrong in a terminal. Every habit of checking output by piping it somewhere would have confirmed it as fine. The test forces colour on, and compares COLUMNS rather than byte offsets -- the first version of it reported drift where the rows lined up perfectly, because U+2500 and U+25C9 are three bytes each.

*Note · 2026-08-25*

## <a id="44"></a>44. Safety branches deleted after verifying the premise rather than trusting it: backup-pre-reroot pointed at ae6d39f…

Safety branches deleted after verifying the premise rather than trusting it: backup-pre-reroot pointed at ae6d39f, whose tree is byte-identical to the rewritten 0d1383d, so the rewrite changed metadata and nothing else. The JS history stays reachable through the v0.1.0-v0.4.0 tags, which tests/golden/README.md depends on. Orphaned SHAs, still in the reflog for a while: ae6d39f4313fdaf9cbfae5fbb160e51cd1b5f7c4 and 81f31e566f3e01e611a30f0253f1733f4c6b99a2.

*Note · 2026-08-25*

## <a id="43"></a>43. PATH and skill both closed by the same change: the flake gained packages.default and packages.skill, and a home-manager…

PATH and skill both closed by the same change: the flake gained packages.default and packages.skill, and a home-manager module installs them together. The skill now ships pinned to the same revision as the binary, so an older input cannot describe commands that Ekko does not have. Two gotchas worth keeping: an untracked file is invisible to a git+file flake (the first build failed with "option does not exist" until git add), and home-manager aborts activation when a backup file already exists, so manual scaffolding has to be cleared before switching.

*Note · 2026-08-25*

## <a id="40"></a>40. Timeline shelved, and note folding chosen instead. Two things came out of printing the board and actually looking at…

Timeline shelved, and note folding chosen instead. Two things came out of printing the board and actually looking at it. First, a name clash nobody had noticed: --timeline is already taken by the date-grouped view, so the proposed feature could never have used that name. Second, and decisive: the noise was measured on the wrong side. Of 94 rendered lines, notes took 56 and every task -- open and closed -- took 28. Collapsing done tasks attacks the smaller half. Folding long notes to one line with a hidden-line count takes the board from 94 lines to 49, with no phase state, no closing rule, no new symbols. It also avoids the worst risk in note [28](#28): folding shows less and says how much it hid, whereas promoting notes would have amplified a stale one. The users own timeline idea is a separate future view for a different model, not this, and is parked deliberately rather than rejected.

*Note · 2026-08-24*

## <a id="39"></a>39. Cursor decision, settled. The root cause is that in_progress is a per-item boolean with nothing constraining how many…

Cursor decision, settled. The root cause is that in_progress is a per-item boolean with nothing constraining how many are true, so the mark drifts from "where I am" to "things I once started". Worse, pausing and never-starting collapse into the same empty box: taskbook names the concept (--begin is documented "Start/pause task") but models it as absence. So: add a real third state. (a) Symbols become never-started, in-progress, paused, done -- a paused item is a situation taskbook could not produce, so nothing byte-identical is at risk. (b) The invariant is AT MOST one in progress, never exactly one -- zero is honest when you are between things, and forcing a cursor would invent one. (c) The alias unstarted currently maps to paused and must be repointed, since with a real paused state the two become opposites; unstarted clears both flags back to the empty box, which also undoes a mistyped --set progress. (d) Neither --set progress nor --begin auto-pauses the others. That was the tempting design and it is wrong: --begin documents multiple ids on purpose, so making one command enforce and the other not leaves two commands for the same action with different rules -- a trap worse than the problem. Duplication is solved by visibility (warn when more than one is in progress) plus expressibility (somewhere to record "set aside"), not by enforcement. (e) The timeline does not need the guarantee anyway: with updatedAt it can point at the most recently touched in-progress item and degrade gracefully. (f) The stats line shows paused only when the count is above zero, so boards that never pause anything render exactly as before -- same trick that kept due dates from disturbing the goldens.

*Note · 2026-08-24*

## <a id="29"></a>29. Measured before building: collapsing 13 done tasks saves \~600 bytes (\~170 tokens) against a \~37k cold-start floor. Real…

Measured before building: collapsing 13 done tasks saves \~600 bytes (\~170 tokens) against a \~37k cold-start floor. Real and irrelevant. The value is signal, not bytes -- 13 lines of tick drown the one line that matters, which is exactly how this board came to read "100% complete" while the actual state was unclear. Also note --clear + --archive already cover most of the collapsing; part of the pain is an unused feature, not a missing one.

*Note · 2026-08-24*

## <a id="28"></a>28. Timeline idea, with the constraints that survived scrutiny. (1) A node is a closed PHASE, never a task -- a timeline of…

Timeline idea, with the constraints that survived scrutiny. (1) A node is a closed PHASE, never a task -- a timeline of tasks is just the archive with dashes. (2) Notes must not collapse with the ticks: they are 65% of the payload and the only part a new session needs, so collapse the check marks and promote the notes, perhaps carrying a count (@due 4/4 - 1 note). (3) "Closed" must be declared, never inferred from 100% -- a board at 4/4 today can take a fifth item tomorrow, and auto-collapsing live work is the same defect the feature is meant to cure. (4) ATUAL needs a cursor: the timeline can only point at where you are if something is in progress, so --timeline and the --set progress habit are one feature. Build the cursor first -- it works without the timeline, the timeline does not work without it. (5) Notes go stale and nothing on the board shows their age -- note 16 still reads "MSRV is 1.88", true when written and false since the home dependency was dropped. Promoting notes to the headline makes a wrong note worse than thirteen ignored ticks, so the design has to answer for age: show it, flag it, or accept it as a deliberate trade.

*Note · 2026-08-24*

## <a id="27"></a>27. Piso de partida a frio medido em 10 sessões: mediana 37.446 tokens (system prompt + ferramentas + CLAUDE.md + índice de…

Piso de partida a frio medido em 10 sessões: mediana 37.446 tokens (system prompt + ferramentas + CLAUDE.md + índice de memória). Logo: sessão nova + ler o board \~38k contra \~850k para retomar a sessão gorda. Trocar de sessão é 22x mais barato -- decidir sem re-medir

*Note · 2026-08-24*

## <a id="26"></a>26. Volta fria custa o TAMANHO da sessão, não o tempo fora: TTL do cache de prompt e 1h. Medido em 90f9ae74 (winwayland)…

Volta fria custa o TAMANHO da sessão, não o tempo fora: TTL do cache de prompt e 1h. Medido em 90f9ae74 (winwayland): ocioso de 69 min gerou 849.837 tokens de cache_write com input_tokens=1. Dois turnos ociosos em 1133 = 40% de todo o cache escrito da sessão

*Note · 2026-08-24*

## <a id="24"></a>24. These three came from comparing ekko against TodoWrite: it wins on features, loses on agent safety. Toggles are not…

These three came from comparing ekko against TodoWrite: it wins on features, loses on agent safety. Toggles are not retry-safe, ids get recycled, and the only read is the whole board

*Note · 2026-08-24*

## <a id="20"></a>20. Archive renumbers ids: --restore N takes the ARCHIVE id and the item returns with a fresh storage id. Pretty output…

Archive renumbers ids: --restore N takes the ARCHIVE id and the item returns with a fresh storage id. Pretty output reports only the archive id (JS-faithful); --json gives both, so agents must use it

*Note · 2026-08-24*

## <a id="15"></a>15. False lead: --move is not affected -- it marks the id with @, is documented that way, and errors loudly when misused

*Note · 2026-08-24*

## <a id="14"></a>14. Found while dogfooding in round 1/3: board prints @release, but --list @release silently returns everything

*Note · 2026-08-24*
