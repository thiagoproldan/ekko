"""Task 507: attention-style retrieval for ekko, replayed on the Claude Code
transcripts of every profile, at no quota. The user's prompt is the query and
the board's items the keys: would the few items a ranking puts first have been
the ones the turn went on to fetch, and what would injecting them with the
prompt (a UserPromptSubmit hook) have cost against the lookups it saves?

A turn is a typed prompt and the main-thread calls until the next one. Its
targets are the items it fetched with context, each sorted by where the
session could have known it from when it fetched it, first match wins:

  cited      the prompt names it ("task 431", "nota 402")
  prime      the session's SessionStart prime listed it
  earlier    an earlier turn's result or reply named it
  this-turn  a result earlier in the same turn named it (a search, mostly)
  none       nothing in the session named it before

Arms, each a ranking of the board as it stood when the prompt was typed:
items created after it, or stashed or trashed before it, left out. The text
is today's: the board keeps no history before 2026-09-24.

  prime      the ids the prime listed, as one unranked set
  cited      the ids the prompt names
  ekko       search's own ranking (src/lexical.rs) with the whole prompt as
             its text: most words matched first, then BM25
  bm25       the same folding, stemming and prefix matching, less Portuguese
             and English stopwords, ranked by BM25 alone
  hybrid     cited first, then bm25
  replied    the ids the reply before the prompt names, the last named first
  bm25ctx    bm25 over the prompt and the reply before it
  hop        cited first, then bm25 over the prompt and the cited items' text:
             a second layer, the first one's result as its query
  heads      cited, the reply's last two ids, then hop (or bm25ctx)
  embed      with --embed: cosine over a local multilingual embedding
             (bge-m3 through Ollama) of the prompt, and embedctx of the
             prompt and the reply's end; eheads is heads with embed last

Prices as evals/claude-code/transcripts.py, in units of one uncached input
token: a 1-hour cache write 2, a cache read 0.1, output 5. A lookup call
avoided saves one request, 0.1*C + 5*o, and its result entering the context,
r*(2 + 0.1*T) over the T calls left before the context is reset; injecting m
tokens with a prompt costs m*(2 + 0.1*T). A context call is avoided only when
every item it fetched was injected whole; a search call only when the items
the turn went on to fetch from its hits were all injected.

    nix develop -c python3 evals/recall/replay.py [--since 2026-09-15] [--out DIR]

Only reads ~/.claude*/projects/ and each registered board's storage.json.
"""

import argparse
import bisect
import collections
import datetime
import glob
import hashlib
import json
import math
import os
import re
import statistics
import subprocess
import sys
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "claude-code"))
from transcripts import LOCAL, SKIP, W_CR, W_CW1H, W_CW5M, W_IN, W_OUT, profiles, when  # noqa: E402

OUT = os.path.normpath(os.path.join(HERE, "..", "..", "target", "evals", "recall"))

# Note 441 calibrated 2.7 characters a token for a first turn, 2.9 for the prefix.
CHARS_PER_TOKEN = 2.8
# What an injected item costs beyond its text: its id, kind and a line break.
ITEM_HEADER = 40
READS = {"context", "search", "next", "prime", "changes", "roadmap", "projects"}
EKKO_TOOL = re.compile(r"^mcp__.*ekko.*__(\w+)$")
EDIT_TOOLS = {"Edit", "Write", "MultiEdit", "NotebookEdit"}
BOARD_WRITES = {"create", "edit", "update", "link", "batch", "stash", "trash", "phases", "set_state", "answer", "ask"}

# ---- search's ranking, ported from src/lexical.rs ---------------------------

K1, B, PREFIX_FROM = 1.2, 0.75, 3
FOLD = {}
for letters, base in (
    ("àáâãäåāăą", "a"), ("çćč", "c"), ("ďđ", "d"), ("èéêëēėęě", "e"), ("ğ", "g"), ("ìíîïīįı", "i"), ("ł", "l"),
    ("ñńň", "n"), ("òóôõöøōő", "o"), ("ř", "r"), ("śšş", "s"), ("ťţ", "t"), ("ùúûüūůű", "u"), ("ýÿ", "y"), ("źżž", "z"),
):
    for letter in letters:
        FOLD[letter] = base
FOLD.update({"ß": "ss", "æ": "ae", "œ": "oe"})
FOLD = str.maketrans(FOLD)
WORD = re.compile(r"[^\W_]+")


def words(text):
    """The runs of letters and digits in `text`, folded."""
    return [word.lower().translate(FOLD) for word in WORD.findall(text)]


def stem(word):
    def cut(ending, shortest):
        if word.endswith(ending) and len(word) - len(ending) >= shortest:
            return word[: -len(ending)]
        return None

    for ending, shortest in (("oes", 4), ("ies", 4), ("ing", 4), ("ed", 4)):
        if (stemmed := cut(ending, shortest)) is not None:
            return stemmed
    if any(word.endswith(ending) for ending in ("ches", "shes", "sses", "xes", "zes")):
        if (stemmed := cut("es", 3)) is not None:
            return stemmed
    if not any(word.endswith(kept) for kept in ("ss", "us", "is")):
        if (stemmed := cut("s", 3)) is not None:
            return stemmed
    return word


def terms_of(text, stop=frozenset()):
    """The query words of `text`, stemmed, each once, in order."""
    out = []
    for word in words(text):
        if word in stop:
            continue
        term = stem(word)
        if term not in out:
            out.append(term)
    return out


# Portuguese and English words that say nothing about a board, folded.
STOP = frozenset("""
a ao aos aquela aquelas aquele aqueles aquilo as ate com como da das de dela delas dele deles depois do dos e ela elas
ele eles em entre era eram essa essas esse esses esta estas este estes estou estamos estao eu foi fomos for foram ha
isso isto ja lhe lhes mais mas me mesmo mesma meu meus minha minhas muito muita na nao nas nem no nos nossa nossas
nosso nossos num numa o os ou para pela pelas pelo pelos por qual quais quando que quem se sem ser seu seus so sua
suas tambem te tem temos tenho ter teu tua um uma umas uns voce voces vc vcs vou vai vamos pra pro pq porque entao
agora aqui ai la cara ta tipo coisa coisas fazer faz feito pode podemos preciso precisa quero queria acho ver olha
sobre onde assim ainda bem sim ok beleza certo dar deu tb tbm alguma algum alguns algumas cada todo toda todos todas
outro outra outros outras qualquer antes sempre nunca vez vezes seja sao sera seria fosse estava estavam tinha tinham
teve tiver havia pois ne consegue conseguir faca fazendo feita seguinte isso dessa desse disso nessa nesse nisso
the an and any are at be because been before being but by can could did do does doing down during each few from
further had has have having he her here hers him his how i if in into is it its just more most my nor not now of
off on once only or other our out over own same she should some such than that their them then there these they
this those through to too under until up very was we were what when where which while who whom why will with would
you your yours let lets please also get got make use used using like want need
""".split())


class Board:
    """One board's items, indexed for search's ranking."""

    def __init__(self, name, path):
        self.name, self.path = name, path
        with open(os.path.join(path, ".ekko", "storage", "storage.json")) as handle:
            raw = json.load(handle)
        self.items, self.by_uid = {}, {}
        for item in raw.values():
            iid = int(item["_id"])
            if item.get("_isTask") in (True, "true"):
                kind = "task"
            elif item.get("handoff"):
                kind = "handoff"
            elif item.get("question"):
                kind = "question"
            elif item.get("knowledge"):
                kind = str(item["knowledge"])
            else:
                kind = "note"
            self.items[iid] = {
                "id": iid,
                "created": int(item.get("_timestamp") or 0),
                "stashed": stamp_ms(item.get("stashed")),
                "trashed": stamp_ms(item.get("trashed")),
                "desc": item.get("description") or "",
                "kind": kind,
            }
            if item.get("uid"):
                self.by_uid[item["uid"]] = iid
        self.ids = sorted(self.items)
        self.length = []
        self.postings = collections.defaultdict(collections.Counter)
        for doc, iid in enumerate(self.ids):
            found = words(self.items[iid]["desc"])
            self.length.append(len(found))
            for word in found:
                self.postings[word][doc] += 1
        self.vocab = sorted(self.postings)
        self.cache = {}
        self.vectors = None

    def resolve(self, ref):
        if isinstance(ref, int):
            return ref if ref in self.items else None
        if isinstance(ref, str):
            if ref.strip().isdigit():
                iid = int(ref)
                return iid if iid in self.items else None
            return self.by_uid.get(ref.strip())
        return None

    def visible(self, at_ms):
        """The doc indices of the items a search could see at `at_ms`."""
        out = []
        for doc, iid in enumerate(self.ids):
            item = self.items[iid]
            if item["created"] > at_ms:
                continue
            if item["stashed"] is not None and item["stashed"] <= at_ms:
                continue
            if item["trashed"] is not None and item["trashed"] <= at_ms:
                continue
            out.append(doc)
        return out

    def term_postings(self, term):
        if term not in self.cache:
            counts = collections.Counter()
            if len(term) >= PREFIX_FROM:
                at = bisect.bisect_left(self.vocab, term)
                while at < len(self.vocab) and self.vocab[at].startswith(term):
                    counts.update(self.postings[self.vocab[at]])
                    at += 1
            elif term in self.postings:
                counts.update(self.postings[term])
            self.cache[term] = counts
        return self.cache[term]

    def rank(self, terms, visible, mode):
        """[(item id, score)] best first. `mode` "ekko" is search's order,
        "bm25" BM25 alone."""
        if not terms or not visible:
            return []
        among = set(visible)
        documents = len(visible)
        average = max(1.0, sum(self.length[doc] for doc in visible) / documents)
        matched, score = collections.Counter(), collections.defaultdict(float)
        for term in terms:
            postings = {doc: tf for doc, tf in self.term_postings(term).items() if doc in among}
            if not postings:
                continue
            idf = math.log(1 + (documents - len(postings) + 0.5) / (len(postings) + 0.5))
            for doc, tf in postings.items():
                matched[doc] += 1
                score[doc] += idf * tf * (K1 + 1) / (tf + K1 * (1 - B + B * self.length[doc] / average))
        hits = list(matched)
        if mode == "ekko":
            if any(matched[doc] == len(terms) for doc in hits):
                hits = [doc for doc in hits if matched[doc] == len(terms)]
            hits.sort(key=lambda doc: (-matched[doc], -score[doc], doc))
        else:
            hits.sort(key=lambda doc: (-score[doc], doc))
        return [(self.ids[doc], score[doc]) for doc in hits]


EMBED_URL = "http://localhost:11434/api/embed"
EMBED_MODEL = "bge-m3"
EMBED_CHARS = 4000


class Embedder:
    """Unit vectors of texts from a local Ollama model, cached on disk."""

    def __init__(self, path):
        self.path = path
        try:
            with open(path) as handle:
                self.cache = json.load(handle)
        except (OSError, ValueError):
            self.cache = {}

    @staticmethod
    def key(text):
        return hashlib.sha1((EMBED_MODEL + "\0" + text[:EMBED_CHARS]).encode()).hexdigest()

    def vectors(self, texts):
        missing = list(dict.fromkeys(text for text in texts if self.key(text) not in self.cache))
        for at in range(0, len(missing), 16):
            batch = [text[:EMBED_CHARS] for text in missing[at : at + 16]]
            body = json.dumps({"model": EMBED_MODEL, "input": batch, "truncate": True}).encode()
            request = urllib.request.Request(EMBED_URL, data=body, headers={"Content-Type": "application/json"})
            with urllib.request.urlopen(request, timeout=900) as reply:
                found = json.load(reply)["embeddings"]
            for text, vector in zip(missing[at : at + 16], found):
                norm = math.sqrt(sum(x * x for x in vector)) or 1.0
                self.cache[self.key(text)] = [x / norm for x in vector]
        return [self.cache[self.key(text)] for text in texts]

    def save(self):
        with open(self.path, "w") as handle:
            json.dump(self.cache, handle)


def cosine_rank(board, vector, visible):
    scored = [(board.ids[doc], sum(a * b for a, b in zip(vector, board.vectors[board.ids[doc]]))) for doc in visible]
    scored.sort(key=lambda pair: (-pair[1], pair[0]))
    return scored


def stamp_ms(value):
    if value in (None, False, "false", ""):
        return None
    if isinstance(value, (int, float)):
        return int(value)
    text = str(value)
    return int(text) if text.isdigit() else 0


def boards():
    """Every registered board that is still on disk, by name, and the folders
    sessions ran it from."""
    listed = json.loads(subprocess.run(["ekko", "--projects", "--json"], capture_output=True, text=True).stdout)
    out, folders = {}, []
    for project in listed.get("projects", []):
        if project.get("status") == "missing":
            continue
        out[project["name"]] = Board(project["name"], project["path"])
        folders.append((project["path"], project["name"]))
    # Sessions of the ekko repo ran from /projects/ekko too, a mount of the same folder.
    if "ekko" in out:
        folders.append(("/projects/ekko", "ekko"))
    folders.sort(key=lambda pair: -len(pair[0]))
    return out, folders


def board_of(cwd, folders):
    for folder, name in folders:
        if cwd == folder or cwd.startswith(folder + "/"):
            return name
    return None


# ---- ids named in text ------------------------------------------------------

LISTED = re.compile(r"(?m)^\s{0,8}(\d{1,5})\.\s")
RUN = r"((?:\d{1,5}(?:\s*(?:,|\be\b|\band\b|\bou\b|\bor\b|/)\s*)?)+)"
NAMED = re.compile(
    r"(?i)\b(?:tasks?|notes?|items?|handoffs?|gotchas?|procedures?|decisions?|questions?|about|on task|tarefas?|notas?"
    r"|itens|perguntas?|decisao|decisoes|procedimentos?)\s+#?" + RUN
)
CITED = re.compile(
    r"(?i)(?:\b(?:tasks?|notes?|items?|handoffs?|gotchas?|procedures?|decisions?|questions?|tarefas?|notas?|itens"
    r"|perguntas?|decisao|decisoes|procedimentos?)\s+#?|#)" + RUN
)


def numbers(run):
    return {int(number) for number in re.findall(r"\d{1,5}", run)}


def named_ids(text):
    """Ids prose names: "task 431", "notes 402, 407 and 459"."""
    out = set()
    for run in NAMED.findall(text.translate(FOLD)):
        out |= numbers(run)
    return out


def listed_ids(text):
    """Ids ekko's own output names: its listing lines and the prose in it."""
    return {int(number) for number in LISTED.findall(text)} | named_ids(text)


# A number standing alone: not a version, a decimal, a time, a size or a path.
BARE = re.compile(r"(?<![\w.,/:#%-])(\d{1,4})(?![\w%]|[.,/:]\d)")
ARTICLE = re.compile(r"(?:\b(?:a|o|as|os|da|do|na|no|das|dos)\s+|#)$")


def cited_ids(text, exists):
    """Ids `text` names: after a keyword ("task 431", "notas 402 e 407"), or
    bare when the board holds them -- any of two digits or more, one digit
    only after an article ("a 5")."""
    folded = text.lower().translate(FOLD)
    out = set()
    for run in CITED.findall(folded):
        out |= numbers(run)
    for found in BARE.finditer(folded):
        number = int(found.group(1))
        if number in exists and (number >= 10 or ARTICLE.search(folded[: found.start()])):
            out.add(number)
    return out


def cited_in_order(text, exists):
    """The ids `text` names, the last named first."""
    folded = text.lower().translate(FOLD)
    spots = []
    for found in CITED.finditer(folded):
        spots += [(found.start(), n) for n in numbers(found.group(1))]
    for found in BARE.finditer(folded):
        number = int(found.group(1))
        if number in exists and (number >= 10 or ARTICLE.search(folded[: found.start()])):
            spots.append((found.start(), number))
    out = []
    for _, number in sorted(spots, reverse=True):
        if number in exists and number not in out:
            out.append(number)
    return out


def language(prompt):
    found = words(prompt)
    pt = sum(word in PT_MARKS for word in found)
    en = sum(word in EN_MARKS for word in found)
    return "pt" if pt > en else "en" if en > pt else "?"


PT_MARKS = frozenset("que nao para com uma isso voce esta mais como pra tem sao ele ela mas foi fazer vamos agora".split())
EN_MARKS = frozenset("the and that with this for you are not what have from was will can should would".split())

# ---- transcripts ------------------------------------------------------------


class Call:
    __slots__ = ("mid", "at", "inp", "cw1h", "cw5m", "cr", "out", "uses", "text", "stop", "left")

    @property
    def ctx(self):
        return self.inp + self.cw1h + self.cw5m + self.cr

    @property
    def cost(self):
        return W_IN * self.inp + W_CW1H * self.cw1h + W_CW5M * self.cw5m + W_CR * self.cr + W_OUT * self.out

    def productive(self):
        """A file edited, a board write, a commit, or the turn handed back."""
        for name, _, arguments in self.uses:
            if name in EDIT_TOOLS or ekko_op(name) in BOARD_WRITES:
                return True
            if name == "Bash" and "git commit" in (arguments.get("command") or ""):
                return True
        return self.stop == "end_turn"


def ekko_op(name):
    found = EKKO_TOOL.match(name or "")
    return found.group(1) if found else None


def text_of(content):
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "\n".join(block.get("text", "") for block in content if isinstance(block, dict) and block.get("type") == "text")
    return ""


def scan(since):
    """Each main-thread transcript as its events in file order, every row
    once however many files hold it."""
    seen_rows = set()
    for base in profiles():
        for path in sorted(glob.glob(os.path.join(base, "projects", "**", "*.jsonl"), recursive=True)):
            if "/subagents/" in path or SKIP in path:
                continue
            events, calls = [], {}
            with open(path, errors="replace") as handle:
                for raw in handle:
                    try:
                        row = json.loads(raw)
                    except ValueError:
                        continue
                    stamp = row.get("timestamp")
                    if not stamp or row.get("isSidechain"):
                        continue
                    at = when(stamp)
                    if at < since:
                        continue
                    kind = row.get("type")
                    uuid = row.get("uuid")
                    message = row.get("message") or {}
                    if kind == "assistant":
                        mid = message.get("id") or row.get("requestId")
                        usage = message.get("usage")
                        if not mid or not usage or message.get("model") == "<synthetic>":
                            continue
                        blocks = [block for block in message.get("content") or [] if isinstance(block, dict)]
                        uses = [(b.get("name", ""), b.get("id"), b.get("input") or {}) for b in blocks if b.get("type") == "tool_use"]
                        text = " ".join(b.get("text", "") for b in blocks if b.get("type") == "text")
                        if mid in calls:
                            calls[mid].uses += uses
                            calls[mid].text += " " + text
                            calls[mid].stop = message.get("stop_reason") or calls[mid].stop
                            continue
                        if mid in seen_rows:
                            continue
                        seen_rows.add(mid)
                        call = Call()
                        split = usage.get("cache_creation") or {}
                        call.cw1h = split.get("ephemeral_1h_input_tokens")
                        call.cw5m = split.get("ephemeral_5m_input_tokens")
                        if call.cw1h is None and call.cw5m is None:
                            call.cw1h, call.cw5m = usage.get("cache_creation_input_tokens") or 0, 0
                        call.cw1h, call.cw5m = call.cw1h or 0, call.cw5m or 0
                        call.mid, call.at = mid, at
                        call.inp = usage.get("input_tokens") or 0
                        call.cr = usage.get("cache_read_input_tokens") or 0
                        call.out = usage.get("output_tokens") or 0
                        call.uses, call.text, call.stop, call.left = uses, text, message.get("stop_reason"), 0
                        calls[mid] = call
                        events.append(("call", call))
                        continue
                    if uuid in seen_rows:
                        continue
                    if uuid:
                        seen_rows.add(uuid)
                    if kind == "user" and not row.get("isMeta"):
                        content = message.get("content")
                        if isinstance(content, list) and any(isinstance(b, dict) and b.get("type") == "tool_result" for b in content):
                            for block in content:
                                if isinstance(block, dict) and block.get("type") == "tool_result":
                                    events.append(("result", block.get("tool_use_id"), text_of(block.get("content"))))
                            continue
                        text = text_of(content)
                        if text.strip() and not text.lstrip().startswith(
                            ("<command-", "<local-command", "<system-reminder", "<task-notification", "[Request interrupted")
                        ):
                            events.append(("prompt", at, text, row.get("cwd") or ""))
                    elif kind == "attachment":
                        attachment = row.get("attachment") or {}
                        name = attachment.get("hookName") or ""
                        if name.startswith("SessionStart:") and "--prime" in (attachment.get("command") or ""):
                            prime = attachment.get("content") or attachment.get("stdout") or ""
                            events.append(("start", name.split(":", 1)[1], prime))
                    elif kind == "system" and row.get("subtype") == "compact_boundary":
                        events.append(("compact",))
            if events:
                yield path, events


def calls_left(events):
    """Set each call's `left`: the calls after it before the context resets."""
    stretch = []
    for event in events + [("start", "clear", "")]:
        if event[0] == "call":
            stretch.append(event[1])
        elif (event[0] == "start" and event[1] in ("startup", "clear")) or event[0] == "compact":
            for at, call in enumerate(stretch):
                call.left = len(stretch) - at - 1
            stretch = []


# ---- turns ------------------------------------------------------------------


def turns(path, events, folders, known):
    """Each typed prompt with the calls it led to and what they fetched."""
    calls_left(events)
    out, seen, prime_ids, turn, uses, segment, reply = [], set(), set(), None, {}, 0, ""
    for event in events:
        kind = event[0]
        if kind == "start":
            if event[1] in ("startup", "clear"):
                seen = set()
                segment += 1
            prime_ids = listed_ids(event[2])
            seen |= prime_ids
        elif kind == "compact":
            segment += 1
        elif kind == "prompt":
            _, at, text, cwd = event
            board = board_of(cwd, folders)
            at_ms = int(at.timestamp() * 1000)
            exists = {i for i, item in known[board].items.items() if item["created"] <= at_ms} if board in known else set()
            turn = {
                "at": at, "prompt": text, "cwd": cwd, "board": board, "prime": set(prime_ids), "reply": reply[-4000:],
                "exists": exists, "seen": set(seen), "cited": cited_ids(text, exists), "calls": [], "targets": [],
                "found": set(), "later": set(),
                "searches": [], "left": None, "session": (path, segment),
            }
            out.append(turn)
            seen |= turn["cited"]
            reply = ""
        elif kind == "call" and turn is not None:
            call = event[1]
            if turn["left"] is None:
                turn["left"] = call.left + 1
            turn["calls"].append(call)
            for name, use_id, arguments in call.uses:
                op = ekko_op(name)
                if op is None:
                    continue
                board = arguments.get("project") or turn["board"]
                uses[use_id] = (op, board, turn, arguments)
                if op == "context":
                    refs = arguments.get("items") or ([arguments["item"]] if "item" in arguments else [])
                    turn["targets"].append({
                        "call": call, "board": board, "refs": refs, "use": use_id, "result": None, "known": set(turn["found"]),
                    })
                elif op == "search":
                    turn["searches"].append({
                        "call": call, "use": use_id, "text": arguments.get("text") or "", "filters": arguments.get("filters") or [],
                        "hits": set(), "result": 0,
                    })
                for value in arguments.values():
                    if isinstance(value, (int, str)) and str(value).isdigit():
                        turn["later"].add(int(value))
                    elif isinstance(value, list):
                        turn["later"] |= {int(v) for v in value if isinstance(v, (int, str)) and str(v).isdigit()}
            mentioned = named_ids(call.text)
            turn["later"] |= mentioned
            seen |= mentioned
            reply += " " + call.text
        elif kind == "result" and event[1] in uses:
            op, board, owner, _ = uses[event[1]]
            ids = listed_ids(event[2])
            owner["found"] |= ids
            seen |= ids
            for search in owner["searches"]:
                if search["use"] == event[1]:
                    search["hits"] |= {int(n) for n in LISTED.findall(event[2])}
                    search["result"] = len(event[2])
            for target in owner["targets"]:
                if target["use"] == event[1]:
                    target["result"] = len(event[2])
    return out


def resolve_targets(turn, known):
    """Each item the turn fetched, once: (id, board name, where it was known
    from, the call that fetched it, its result size)."""
    out, found_so_far, done = [], set(), set()
    # a result's ids count as found in this turn only for fetches after it
    for target in turn["targets"]:
        board = known.get(target["board"]) if target["board"] else None
        if board is None:
            continue
        for ref in target["refs"]:
            iid = board.resolve(ref)
            if iid is None or (board.name, iid) in done:
                continue
            done.add((board.name, iid))
            if iid in turn["cited"]:
                source = "cited"
            elif iid in turn["prime"]:
                source = "prime"
            elif iid in turn["seen"]:
                source = "earlier"
            elif iid in target["known"]:
                source = "this-turn"
            else:
                source = "none"
            out.append({"id": iid, "board": board.name, "source": source, "call": target["call"], "result": target.get("result") or 0})
    return out


# ---- arms and prices --------------------------------------------------------


def rankings(turn, board):
    at_ms = int(turn["at"].timestamp() * 1000)
    visible = board.visible(at_ms)
    everyone = set(board.ids[doc] for doc in visible)
    cited = [iid for iid in cited_in_order(turn["prompt"], everyone)]
    replied = [iid for iid in cited_in_order(turn["reply"], everyone) if iid not in cited]
    prompt_terms = terms_of(turn["prompt"], STOP)
    ekko = board.rank(terms_of(turn["prompt"]), visible, "ekko")
    bm25 = board.rank(prompt_terms, visible, "bm25")
    context_terms = prompt_terms + [term for term in terms_of(turn["reply"], STOP) if term not in prompt_terms]
    bm25ctx = board.rank(context_terms, visible, "bm25")
    hop_text = " ".join(board.items[iid]["desc"][:1500] for iid in cited)
    hop = board.rank(prompt_terms + [t for t in terms_of(hop_text, STOP) if t not in prompt_terms], visible, "bm25") if cited else bm25

    def ordered(*heads):
        out, taken = [], set()
        for head in heads:
            for iid, score in head:
                if iid not in taken:
                    taken.add(iid)
                    out.append((iid, score))
        return out

    first = [(iid, math.inf) for iid in cited]
    embedded = {}
    if board.vectors is not None:
        prompt_vector, context_vector = EMBEDDER.vectors([turn["prompt"], turn["prompt"] + "\n" + turn["reply"][-1500:]])
        embedded["embed"] = cosine_rank(board, prompt_vector, visible)
        embedded["embedctx"] = cosine_rank(board, context_vector, visible)
        embedded["eheads"] = ordered(first, [(iid, math.inf) for iid in replied[:2]], embedded["embed"])
    return embedded | {
        "cited": first,
        "replied": [(iid, math.inf) for iid in replied],
        "ekko": ekko,
        "bm25": bm25,
        "hybrid": ordered(first, bm25),
        "bm25ctx": bm25ctx,
        "hop": ordered(first, hop),
        "heads": ordered(first, [(iid, math.inf) for iid in replied[:2]], hop if cited else bm25ctx),
    }, everyone


def content_terms(prompt):
    return [term for term in terms_of(prompt, STOP) if not term.isdigit()]


def wilson(hits, total, z=1.96):
    if total == 0:
        return (None, None)
    p = hits / total
    centre = (p + z * z / (2 * total)) / (1 + z * z / total)
    half = z * math.sqrt(p * (1 - p) / total + z * z / (4 * total * total)) / (1 + z * z / total)
    return (round(centre - half, 3), round(centre + half, 3))


def tokens(chars):
    return chars / CHARS_PER_TOKEN


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--since", default="2026-09-15")
    parser.add_argument("--out", default=OUT)
    parser.add_argument("--embed", action="store_true", help="add the embedding arms (a local Ollama with bge-m3)")
    args = parser.parse_args()
    since = datetime.datetime.fromisoformat(args.since).replace(tzinfo=LOCAL)
    known, folders = boards()
    BOARDS_CACHE.update(known)
    global EMBEDDER
    if args.embed:
        os.makedirs(args.out, exist_ok=True)
        EMBEDDER = Embedder(os.path.join(args.out, "embeddings.json"))
        for board in known.values():
            texts = [board.items[iid]["desc"] for iid in board.ids]
            board.vectors = dict(zip(board.ids, EMBEDDER.vectors(texts)))

    all_turns, bill, bill_boards = [], 0.0, 0.0
    for path, events in scan(since):
        for turn in turns(path, events, folders, known):
            all_turns.append(turn)
            cost = sum(call.cost for call in turn["calls"])
            bill += cost
            if turn["board"] in known:
                bill_boards += cost

    on_board = [turn for turn in all_turns if turn["board"] in known]
    rows = []
    for turn in on_board:
        board = known[turn["board"]]
        arms, everyone = rankings(turn, board)
        targets = [t for t in resolve_targets(turn, known) if t["board"] == board.name]
        rows.append({"turn": turn, "arms": arms, "targets": targets, "everyone": everyone})
    if EMBEDDER is not None:
        EMBEDDER.save()

    report = {"since": args.since, "turns": len(all_turns), "turns_on_boards": len(on_board), "bill": bill, "bill_on_boards": bill_boards}
    report["by_board"] = collections.Counter(turn["board"] for turn in on_board)
    report["language"] = collections.Counter(language(turn["prompt"]) for turn in on_board)

    needing = [row for row in rows if row["targets"]]
    report["turns_fetching"] = len(needing)
    report["targets"] = sum(len(row["targets"]) for row in needing)
    report["sources"] = collections.Counter(t["source"] for row in needing for t in row["targets"])

    # H1, H2: recall at k, per source, per board and per language
    recall = {}
    arms_measured = ["prime", "cited", "replied", "ekko", "bm25", "hybrid", "bm25ctx", "hop", "heads", "prime+heads"]
    if EMBEDDER is not None:
        arms_measured += ["embed", "embedctx", "eheads", "prime+eheads"]
    for arm in arms_measured:
        for k in (1, 3, 5, 10):
            if arm in ("prime", "cited", "replied") and k != 10:
                continue
            hit_items, total_items, hit_turns = 0, 0, 0
            split = collections.defaultdict(lambda: [0, 0])
            for row in needing:
                if arm == "prime":
                    chosen = row["turn"]["prime"]
                elif arm.startswith("prime+"):
                    chosen = row["turn"]["prime"] | {iid for iid, _ in row["arms"][arm[6:]][:k]}
                else:
                    chosen = {iid for iid, _ in row["arms"][arm][:k]}
                got = [t for t in row["targets"] if t["id"] in chosen]
                hit_items += len(got)
                total_items += len(row["targets"])
                hit_turns += bool(got)
                for t in row["targets"]:
                    shape = "topical" if len(content_terms(row["turn"]["prompt"])) >= 3 else "short"
                    for key in (f"source:{t['source']}", f"board:{t['board']}", f"lang:{language(row['turn']['prompt'])}", f"shape:{shape}"):
                        split[key][0] += t["id"] in chosen
                        split[key][1] += 1
            name = arm if arm in ("prime", "cited", "replied") else f"{arm}@{k}"
            recall[name] = {
                "items": [hit_items, total_items, round(hit_items / max(1, total_items), 3), wilson(hit_items, total_items)],
                "turns": [hit_turns, len(needing), round(hit_turns / max(1, len(needing)), 3)],
                "split": {key: [h, n, round(h / n, 3)] for key, (h, n) in sorted(split.items())},
            }
    report["recall"] = recall
    report["shapes"] = collections.Counter(
        "topical" if len(content_terms(row["turn"]["prompt"])) >= 3 else "short" for row in needing
    )

    # The most any injection could save: every lookup-only call on a board.
    oracle = oracle_calls = overhead = seconds = 0.0
    for row in rows:
        calls = row["turn"]["calls"]
        for at, call in enumerate(calls):
            if call.uses and all(ekko_op(n) in READS for n, _, _ in call.uses):
                results = [t["result"] for t in row["targets"] if t["call"] is call]
                results += [s["result"] for s in row["turn"]["searches"] if s["call"] is call]
                oracle += W_CR * call.ctx + W_OUT * call.out + tokens(sum(results) or 1500) * (W_CW1H + W_CR * call.left)
                overhead += W_CR * call.ctx + W_OUT * call.out
                oracle_calls += 1
                if at + 1 < len(calls):
                    seconds += (calls[at + 1].at - call.at).total_seconds()
    report["oracle"] = {
        "calls": oracle_calls, "units_with_content": round(oracle), "of_bill": round(oracle / bill_boards, 4),
        "units_calls_only": round(overhead), "calls_only_of_bill": round(overhead / bill_boards, 4), "seconds": round(seconds),
    }

    # Precision and prices, per arm and cut: what the hook would inject on
    # every prompt against the lookups it would save.
    prices = {}
    for arm in ["hybrid", "heads"] + (["eheads"] if EMBEDDER is not None else []):
        for k in (1, 3):
            for clip in (300, 800, 1500):
                for floor in (0.0, 8.0):
                    prices[f"{arm}@{k} clip {clip} floor {floor:g}"] = price(rows, arm, k, clip, floor)
    for k in (1, 3):
        prices[f"cited@{k} whole"] = price(rows, "cited", k, 100_000, 0.0)
    for arm in ["heads"] + (["eheads"] if EMBEDDER is not None else []):
        for k in (1, 2):
            prices[f"{arm}@{k} clip 800 tuned"] = price(rows, arm, k, 800, 0.0, tuned=True)
    report["prices"] = prices

    # H4: the warm-up, calls and seconds before the first productive action.
    warm = []
    for row in rows:
        calls = row["turn"]["calls"]
        first = next((at for at, call in enumerate(calls) if call.productive()), None)
        if first is None:
            continue
        lookups = sum(1 for call in calls[:first] if call.uses and all(ekko_op(n) in READS for n, _, _ in call.uses))
        seconds = (calls[first].at - row["turn"]["at"]).total_seconds()
        warm.append((first, lookups, seconds))
    report["warmup"] = {
        "turns": len(warm),
        "median_calls_before_first_action": statistics.median(w[0] for w in warm) if warm else None,
        "turns_with_lookups_before_it": sum(1 for w in warm if w[1]),
        "lookup_calls_before_it": sum(w[1] for w in warm),
        "median_seconds": statistics.median(w[2] for w in warm) if warm else None,
    }

    os.makedirs(args.out, exist_ok=True)
    with open(os.path.join(args.out, "report.json"), "w") as handle:
        json.dump(report, handle, indent=1, default=str)
    with open(os.path.join(args.out, "turns.jsonl"), "w") as handle:
        for row in needing:
            turn = row["turn"]
            handle.write(json.dumps({
                "at": turn["at"].isoformat(), "board": turn["board"], "lang": language(turn["prompt"]), "prompt": turn["prompt"][:600],
                "targets": [{k: v for k, v in t.items() if k != "call"} for t in row["targets"]],
                "bm25": [iid for iid, _ in row["arms"]["bm25"][:5]], "ekko": [iid for iid, _ in row["arms"]["ekko"][:5]],
                "prime": sorted(turn["prime"]), "cited": sorted(turn["cited"]),
                "searches": [{"text": s["text"], "filters": s["filters"], "hits": sorted(s["hits"])[:10]} for s in turn["searches"]],
            }, ensure_ascii=False) + "\n")
    with open(os.path.join(args.out, "injections.jsonl"), "w") as handle:
        for row in rows:
            turn = row["turn"]
            handle.write(json.dumps({
                "at": turn["at"].isoformat(), "board": turn["board"], "prompt": turn["prompt"][:800], "reply": turn["reply"][-600:],
                "shape": "topical" if len(content_terms(turn["prompt"])) >= 3 else "short",
                "prime": sorted(turn["prime"]), "seen": sorted(turn["seen"]), "later": sorted(turn["later"]),
                "targets": [t["id"] for t in row["targets"]],
                "arms": {arm: [iid for iid, _ in ranked[:3]] for arm, ranked in row["arms"].items()},
            }, ensure_ascii=False) + "\n")
    show(report)


PASTED = re.compile(r"^\s*<(bash-|pasted_content|command-)")


def price(rows, arm, k, clip, floor, tuned=False):
    """What injecting `arm`'s best `k` above `floor`, each clipped to `clip`
    characters, would have cost and saved, over every prompt on a board.
    Tuned, it injects only on a prompt with three content words or more that
    is not a paste or a command's output, and only items the session has not
    been shown: not in its prime, not named before."""
    injected_cost = saved = 0.0
    injected_items = useful = 0
    avoided_calls = avoided_seconds = 0
    session_injected = collections.defaultdict(set)
    for row in rows:
        turn, board = row["turn"], row["turn"]["board"]
        key = turn["session"]
        ranked = row["arms"][arm]
        if tuned:
            if len(content_terms(turn["prompt"])) < 3 or PASTED.match(turn["prompt"]):
                ranked = []
            ranked = [(iid, s) for iid, s in ranked if iid not in turn["prime"] and iid not in turn["seen"]]
        chosen = [(iid, s) for iid, s in ranked[:k] if s >= floor]
        fresh = [iid for iid, _ in chosen if iid not in session_injected[key]]
        left = turn["left"] or 0
        items = BOARDS_CACHE[board].items
        chars = sum(min(len(items[iid]["desc"]), clip) + ITEM_HEADER for iid in fresh)
        injected_cost += tokens(chars) * (W_CW1H + W_CR * left)
        injected_items += len(fresh)
        useful += sum(1 for iid in fresh if iid in turn["later"] or any(t["id"] == iid for t in row["targets"]))
        session_injected[key] |= set(fresh)
        have = session_injected[key]
        whole = {iid for iid in have if len(items[iid]["desc"]) <= clip}
        # context calls whose every item came whole with the prompt
        by_call = collections.defaultdict(list)
        for target in row["targets"]:
            by_call[id(target["call"])].append(target)
        calls = turn["calls"]
        for at, call in enumerate(calls):
            if not call.uses or not all(ekko_op(n) in READS for n, _, _ in call.uses):
                continue
            ops = {ekko_op(n) for n, _, _ in call.uses}
            fetched = by_call.get(id(call), [])
            if ops == {"context"} and fetched and all(t["id"] in whole for t in fetched):
                result = sum(t["result"] for t in fetched)
            elif ops == {"search"}:
                searches = [s for s in turn["searches"] if s["call"] is call]
                hits = set().union(*(s["hits"] for s in searches)) if searches else set()
                used = {t["id"] for t in row["targets"]} & hits
                if not used or not used <= have:
                    continue
                result = sum(s["result"] for s in searches)
            else:
                continue
            saved += W_CR * call.ctx + W_OUT * call.out + tokens(result) * (W_CW1H + W_CR * call.left)
            avoided_calls += 1
            if at + 1 < len(calls):
                avoided_seconds += (calls[at + 1].at - call.at).total_seconds()
    return {
        "injected_items": injected_items, "useful": useful, "precision": round(useful / max(1, injected_items), 3),
        "injected_units": round(injected_cost), "saved_units": round(saved), "net_units": round(saved - injected_cost),
        "avoided_calls": avoided_calls, "avoided_seconds": round(avoided_seconds),
    }


BOARDS_CACHE = {}
EMBEDDER = None


def show(report):
    print(f"since {report['since']}: {report['turns']} turns, {report['turns_on_boards']} on a board {dict(report['by_board'])}")
    print(f"languages of those prompts: {dict(report['language'])}")
    print(f"bill: {report['bill'] / 1e6:.1f}M units, {report['bill_on_boards'] / 1e6:.1f}M on boards")
    print(f"turns that fetched items: {report['turns_fetching']}, {report['targets']} items; known from: {dict(report['sources'])}")
    print("\nRECALL (items found / items fetched, Wilson 95%; turns with any)")
    for name, value in report["recall"].items():
        items, turns_ = value["items"], value["turns"]
        print(f"  {name:10s} {items[0]:4d}/{items[1]:<4d} {items[2]:.2f} {items[3]}  turns {turns_[0]}/{turns_[1]}")
    print("shapes of the turns that fetched:", dict(report["shapes"]))
    print("ORACLE (every lookup-only call on a board avoided):", report["oracle"])
    names = ["prime", "cited", "replied", "bm25@3", "bm25ctx@3", "hop@3", "heads@3", "heads@10", "prime+heads@3"]
    names += [n for n in ("embed@3", "embedctx@3", "eheads@3", "eheads@10", "prime+eheads@3") if n in report["recall"]]
    counts = {k: v[1] for k, v in report["recall"]["prime"]["split"].items()}
    print("  items per split:", counts)
    for name in names:
        print(f"  {name:15s}", {k.split(":")[1][:9]: v[2] for k, v in report["recall"][name]["split"].items()})
    print("\nPRICES (units; bill on boards %.1fM)" % (report["bill_on_boards"] / 1e6))
    for name, value in report["prices"].items():
        print(f"  {name:28s} inj {value['injected_items']:4d} prec {value['precision']:.2f} cost {value['injected_units'] / 1e3:8.0f}k"
              f" saved {value['saved_units'] / 1e3:7.0f}k net {value['net_units'] / 1e3:8.0f}k calls {value['avoided_calls']:3d} {value['avoided_seconds']}s")
    print("\nWARM-UP", report["warmup"])


if __name__ == "__main__":
    main()
