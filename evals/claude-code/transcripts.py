"""Task 180: what handoffs save, read off the Claude Code transcripts of every
profile, against the week the model behind them was measured on (2026-09-08..15,
decisions 161, 163 and 164). The 2026-09-15 pass was thrown away; this is its
rewrite, checked by reproducing that week's numbers.

Each API call is counted once, by message id, and priced in units of one
uncached input token with Opus 5's weights: a 1-hour cache write 2x, a 5-minute
one 1.25x, a cache read 0.1x, output 5x. Per period it reports the parameters
of the model (context per call, growth d, output o, floor b, orientation R),
where the cost went, compactions, and every expensive restart: a cold resume
(a gap over 60 minutes and the whole prefix written again, the baseline's
definition) and any start whose first call wrote a large prefix again, whatever
the gap. Then every session a /clear started with a handoff in its prime: what
it began with, how far it read before its first productive action, and what
continuing the cleared session would have cost instead.

    nix develop -c python3 evals/claude-code/transcripts.py [--handoffs-from 2026-09-21T01:50] [--until ...]

Times are local (UTC-3). Only reads ~/.claude*/projects/; the numbers land in
target/evals/claude-code/transcripts/report.json.
"""

import collections
import datetime
import glob
import json
import os
import re
import statistics
import sys

HOME = os.path.expanduser("~")
LOCAL = datetime.timezone(datetime.timedelta(hours=-3))
OUT = os.path.normpath(
    os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "target", "evals", "claude-code", "transcripts")
)

# Opus 5, in units of one uncached input token.
W_IN, W_CW1H, W_CW5M, W_CR, W_OUT = 1.0, 2.0, 1.25, 0.1, 5.0

COLD_GAP = datetime.timedelta(minutes=60)
WINDOW = datetime.timedelta(hours=5)
# A /clear leaves no mark in the session it cleared, and the new session names
# no parent: the cleared one is the last to call from the same folder and
# profile, this shortly before.
LINK_GAP = datetime.timedelta(minutes=15)
# A prefix large enough that writing it again is an event.
LARGE = 100_000
# What auto-compaction does to a session that keeps going (medians of the
# baseline week): it fires near 1M, the next call starts near 87k, and the
# summary costs about a read of the window, its output and the new base.
COMPACT_AT, COMPACT_LEAVES, COMPACT_COST = 1_000_000, 87_000, 350_000

EDIT_TOOLS = {"Edit", "Write", "MultiEdit", "NotebookEdit"}
# Board writes that carry content. Setting a state is bookkeeping: a resumed
# session sets its task in progress before it has done anything.
BOARD_WRITE = re.compile(r"^mcp__.*ekko.*__(create|edit|update|link|batch|stash|trash|phases)$")


def when(stamp):
    return datetime.datetime.fromisoformat(stamp.replace("Z", "+00:00")).astimezone(LOCAL)


def local(text):
    return datetime.datetime.fromisoformat(text).replace(tzinfo=LOCAL)


class Call:
    __slots__ = (
        "mid", "profile", "file", "line", "at", "side", "cwd", "model", "effort",
        "inp", "cw1h", "cw5m", "cr", "out", "think", "tools", "stop",
    )

    @property
    def cw(self):
        return self.cw1h + self.cw5m

    @property
    def ctx(self):
        return self.inp + self.cw + self.cr

    @property
    def cost(self):
        return W_IN * self.inp + W_CW1H * self.cw1h + W_CW5M * self.cw5m + W_CR * self.cr + W_OUT * self.out

    @property
    def write_cost(self):
        return W_CW1H * self.cw1h + W_CW5M * self.cw5m

    def rewrote(self, at_least):
        """Whether this call wrote (nearly) its whole prefix again."""
        return self.ctx >= at_least and self.cw >= 0.8 * self.ctx

    def productive(self):
        """What makes this call a productive action, or None: a file edited, a
        board write, a commit, or a turn handed back to the user."""
        for name, arg in self.tools:
            if name in EDIT_TOOLS:
                return "edit"
            if BOARD_WRITE.match(name):
                return "board"
            if name == "Bash" and "git commit" in arg:
                return "commit"
        return "answer" if self.stop == "end_turn" else None


def tool_of(block):
    name = block.get("name", "")
    args = block.get("input") or {}
    if name == "Bash":
        return (name, (args.get("command") or "")[:400])
    return (name, args.get("file_path") or args.get("notebook_path") or "")


# The runs of the paired test (evals/paired/harness.py) live under a folder of
# this name: they are experiments, not the user's sessions.
SKIP = "ekko-paired"


def profiles():
    """Every Claude Code config dir in the home folder: ~/.claude, and each
    ~/.claude-<name> beside it that a profile keeps apart."""
    return sorted(path for path in glob.glob(os.path.join(HOME, ".claude*")) if os.path.isdir(os.path.join(path, "projects")))


def scan():
    """Every call once, and each transcript's events in file order."""
    calls = {}
    files = collections.defaultdict(list)
    starts = {}
    paths = []
    for base in profiles():
        pattern = os.path.join(base, "projects", "**", "*.jsonl")
        paths += [(os.path.basename(base), path) for path in sorted(glob.glob(pattern, recursive=True)) if SKIP not in path]
    for profile, path in paths:
        with open(path, errors="replace") as handle:
            for line_no, raw in enumerate(handle):
                if not any(mark in raw for mark in ('"assistant"', "SessionStart", "compact_boundary", '"user"')):
                    continue
                try:
                    row = json.loads(raw)
                except ValueError:
                    continue
                stamp = row.get("timestamp")
                if not stamp:
                    continue
                kind = row.get("type")
                if kind == "assistant":
                    message = row.get("message") or {}
                    usage = message.get("usage")
                    mid = message.get("id") or row.get("requestId")
                    if not usage or not mid or message.get("model") == "<synthetic>":
                        continue
                    tools = [tool_of(b) for b in message.get("content") or [] if isinstance(b, dict) and b.get("type") == "tool_use"]
                    if mid in calls:
                        # one response is logged as one line per content block
                        if calls[mid].file == path:
                            calls[mid].tools += tools
                            calls[mid].stop = message.get("stop_reason") or calls[mid].stop
                        continue
                    call = Call()
                    call.mid, call.profile, call.file, call.line = mid, profile, path, line_no
                    call.at = when(stamp)
                    call.side = bool(row.get("isSidechain")) or "/subagents/" in path
                    call.cwd = row.get("cwd") or ""
                    call.model = message.get("model") or ""
                    call.effort = row.get("effort") or ""
                    split = usage.get("cache_creation") or {}
                    call.cw1h = split.get("ephemeral_1h_input_tokens")
                    call.cw5m = split.get("ephemeral_5m_input_tokens")
                    if call.cw1h is None and call.cw5m is None:
                        call.cw1h, call.cw5m = usage.get("cache_creation_input_tokens") or 0, 0
                    call.cw1h, call.cw5m = call.cw1h or 0, call.cw5m or 0
                    call.inp = usage.get("input_tokens") or 0
                    call.cr = usage.get("cache_read_input_tokens") or 0
                    call.out = usage.get("output_tokens") or 0
                    call.think = (usage.get("output_tokens_details") or {}).get("thinking_tokens") or 0
                    call.tools = tools
                    call.stop = message.get("stop_reason")
                    calls[mid] = call
                    files[path].append((line_no, "call", call))
                elif kind == "attachment":
                    attachment = row.get("attachment") or {}
                    name = attachment.get("hookName") or ""
                    if not name.startswith("SessionStart:"):
                        continue
                    key = (path, attachment.get("toolUseID"))
                    if key not in starts:
                        starts[key] = {"at": when(stamp), "source": name.split(":", 1)[1], "prime": 0, "handoff": False, "profile": profile}
                        files[path].append((line_no, "start", starts[key]))
                    if "--prime" in (attachment.get("command") or ""):
                        content = attachment.get("content") or attachment.get("stdout") or ""
                        starts[key]["prime"] = len(content)
                        starts[key]["handoff"] = "Where the last session stopped" in content
                elif kind == "system" and row.get("subtype") == "compact_boundary":
                    meta = row.get("compactMetadata") or {}
                    files[path].append((line_no, "compact", {
                        "at": when(stamp), "trigger": meta.get("trigger"), "pre": meta.get("preTokens"), "post": meta.get("postTokens"),
                    }))
                elif kind == "user" and not row.get("isMeta") and not row.get("isSidechain"):
                    content = (row.get("message") or {}).get("content")
                    if isinstance(content, str):
                        text = content
                    else:
                        text = " ".join(b.get("text", "") for b in content or [] if isinstance(b, dict) and b.get("type") == "text")
                    # a slash command and its output are not what the user asked
                    if text.strip() and not text.lstrip().startswith(("<command-", "<local-command")):
                        files[path].append((line_no, "prompt", {"at": when(stamp), "text": text[:300]}))
    for events in files.values():
        events.sort(key=lambda event: event[0])
    return calls, files


def segments(path, events):
    """One transcript cut at every start and compaction, each stretch with the
    main-thread calls in it and the first prompt typed in it."""
    out = [{"path": path, "how": "file", "event": {}, "calls": [], "prompt": None}]
    for _, kind, data in events:
        if kind in ("start", "compact"):
            out.append({"path": path, "how": kind, "event": data, "calls": [], "prompt": None})
        elif kind == "prompt" and out[-1]["prompt"] is None:
            out[-1]["prompt"] = data["text"]
        elif kind == "call" and not data.side:
            out[-1]["calls"].append(data)
    return out


def median(values):
    return statistics.median(values) if values else None


def quantile(values, q):
    values = sorted(values)
    return values[min(len(values) - 1, int(round(q * (len(values) - 1))))] if values else None


def mean(values):
    return statistics.fmean(values) if values else None


def orientation(calls):
    """(index, kind) of the first productive call, or None."""
    for index, call in enumerate(calls):
        kind = call.productive()
        if kind:
            return index, kind
    return None


def continuing(before, segment_calls, first_work):
    """What the calls from `first_work` on would have cost in the cleared
    session, which stood at `before` tokens: the same work on top of the old
    prefix instead of the new one, with auto-compaction where it would fire."""
    offset = before - segment_calls[first_work].ctx
    cost = 0.0
    for index, call in enumerate(segment_calls[first_work:]):
        grown = call.ctx + offset
        if grown > COMPACT_AT:
            cost += COMPACT_COST
            offset = COMPACT_LEAVES - call.ctx
            grown = COMPACT_LEAVES
        whole = call.cw >= 0.8 * call.ctx
        if index == 0:
            # the cleared session reads its old prefix warm where the new one
            # wrote a base; content that is new is written in both
            cost += W_IN * call.inp + W_CR * grown + (0 if whole else call.write_cost) + W_OUT * call.out
        elif whole and call.ctx >= LARGE:
            # an invalidation writes the whole prefix again, and the old one is larger
            cost += call.cost + W_CW1H * (grown - call.ctx)
        else:
            cost += call.cost + W_CR * (grown - call.ctx)
    return cost


def first_of(calls, kinds):
    """How far the context rose, and in how many calls, before the first call
    whose productive kind is one of `kinds`: (tokens, calls), or (None, None)."""
    for index, call in enumerate(calls):
        if call.productive() in kinds:
            return call.ctx - calls[0].ctx, index
    return None, None


def handoff_clear(segment, life, calls_by_place):
    """A session a /clear started with a handoff in its prime. `life` is every
    main-thread call it made, later resumes included: all of them ran on the
    smaller prefix the clear left."""
    calls = segment["calls"]
    first = calls[0]
    start = segment["event"]["at"]
    earlier = [c for c in calls_by_place[(first.profile, first.cwd)] if c.at < start and c.file != segment["path"]]
    cleared = max(earlier, key=lambda c: c.at) if earlier else None
    if cleared and start - cleared.at > LINK_GAP:
        cleared = None
    found = orientation(calls)
    index, kind = found if found else (len(calls), None)
    reads = sum(1 for c in calls[:index] for name, _ in c.tools if name == "Read")
    transcript_reads = sum(1 for c in calls for name, arg in c.tools if name in ("Bash", "Read") and "projects/" in arg and ".jsonl" in arg)
    r_edit, r_edit_calls = first_of(calls, {"edit", "commit"})
    r_answer, r_answer_calls = first_of(calls, {"answer"})
    record = {
        "session": os.path.basename(segment["path"])[:8], "profile": first.profile, "at": start.isoformat(timespec="minutes"),
        "prompt": (segment["prompt"] or "").strip()[:80], "prime_chars": segment["event"]["prime"],
        "b": first.ctx, "first_kind": kind, "R": (calls[index].ctx - first.ctx) if found else None, "R_calls": index if found else None,
        "R_edit": r_edit, "R_edit_calls": r_edit_calls, "R_answer": r_answer, "R_answer_calls": r_answer_calls,
        "reads_before": reads, "transcript_reads": transcript_reads,
        "calls": len(life), "cost": sum(c.cost for c in life), "orientation_cost": sum(c.cost for c in calls[:index]),
        "cleared": os.path.basename(cleared.file)[:8] if cleared else None,
        "C_stop": cleared.ctx if cleared else None, "gap_min": round((start - cleared.at).total_seconds() / 60, 1) if cleared else None,
    }
    if cleared and found:
        record["continue_cost"] = continuing(cleared.ctx, life, index)
        # net: continuing would need no orientation; gross: it makes the same calls
        record["saved_net"] = record["continue_cost"] - record["cost"]
        record["saved_gross"] = continuing(cleared.ctx, life, 0) - record["cost"]
        record["cold_resume_cost"] = W_CW1H * cleared.ctx
        record["bound_2bR"] = 2 * calls[index].ctx
    return record


def report(calls, files, periods):
    calls_by_place = collections.defaultdict(list)
    spending = collections.defaultdict(list)
    for call in calls.values():
        spending[call.profile].append((call.at, call.cost))
        if not call.side:
            calls_by_place[(call.profile, call.cwd)].append(call)
    for profile in spending:
        spending[profile].sort()

    def next_hours(profile, at):
        return sum(cost for moment, cost in spending[profile] if at <= moment < at + WINDOW)

    # a window that runs past the last call read is not over yet, and its share is too high
    last = max(call.at for call in calls.values())

    def share(call):
        window = next_hours(call.profile, call.at)
        return (call.cost / window if window else None), call.at + WINDOW <= last

    all_segments = [segment for path, events in files.items() for segment in segments(path, events)]
    file_calls = {path: [data for _, kind, data in events if kind == "call" and not data.side] for path, events in files.items()}
    results = {}
    for name, start, end in periods:
        inside = [c for c in calls.values() if start <= c.at < end]
        main = [c for c in inside if not c.side]
        if not inside:
            continue
        total = sum(c.cost for c in inside)
        sessions = collections.Counter()
        for c in inside:
            sessions[c.file.split("/subagents/")[0]] += c.cost

        growth, after_100, after_200, rewrites, cold = [], 0.0, 0.0, 0.0, []
        for path, fc in file_calls.items():
            for index, c in enumerate(fc):
                if not (start <= c.at < end):
                    continue
                after_100 += c.cost if index >= 100 else 0
                after_200 += c.cost if index >= 200 else 0
                if index and c.rewrote(50_000):
                    rewrites += c.write_cost
                if index and c.rewrote(LARGE) and c.at - fc[index - 1].at >= COLD_GAP:
                    portion, complete = share(c)
                    cold.append({"session": os.path.basename(path)[:8], "at": c.at.isoformat(timespec="minutes"), "ctx": c.ctx,
                                 "cost": c.cost, "share_5h": portion, "window_complete": complete})
        fresh_b, fresh_r_edit, fresh_r_prod, compactions, restarts, clears, abandoned = [], [], [], [], [], [], 0
        for segment in all_segments:
            seg_calls = segment["calls"]
            source = segment["event"].get("source") if segment["how"] == "start" else segment["how"]
            handed_off = segment["how"] == "start" and source == "clear" and segment["event"].get("handoff")
            if not seg_calls:
                # a /clear nobody typed after: the new session never called
                abandoned += bool(handed_off and start <= segment["event"]["at"] < end)
                continue
            for a, b in zip(seg_calls, seg_calls[1:]):
                if start <= b.at < end:
                    growth.append(b.ctx - a.ctx)
            began = segment["event"].get("at") or seg_calls[0].at
            if not (start <= began < end):
                continue
            first = seg_calls[0]
            if source in ("startup", "file"):
                fresh_b.append(first.ctx)
                r_edit, _ = first_of(seg_calls, {"edit"})
                if r_edit is not None:
                    fresh_r_edit.append(r_edit)
                found = orientation(seg_calls)
                if found:
                    fresh_r_prod.append(seg_calls[found[0]].ctx - first.ctx)
            if segment["how"] == "compact":
                r_edit, r_edit_calls = first_of(seg_calls, {"edit", "commit"})
                found = orientation(seg_calls)
                compactions.append({
                    **{key: value for key, value in segment["event"].items() if key != "at"},
                    "session": os.path.basename(segment["path"])[:8], "b": first.ctx, "R_edit": r_edit, "R_edit_calls": r_edit_calls,
                    "R": seg_calls[found[0]].ctx - first.ctx if found else None,
                })
            if segment["how"] == "start" and first.rewrote(LARGE):
                fc = file_calls[segment["path"]]
                position = fc.index(first)
                gap = (first.at - fc[position - 1].at).total_seconds() / 60 if position else None
                portion, complete = share(first)
                restarts.append({
                    "session": os.path.basename(segment["path"])[:8], "source": source, "at": first.at.isoformat(timespec="minutes"),
                    "ctx": first.ctx, "cost": first.cost, "gap_min": round(gap) if gap is not None else None,
                    "share_5h": portion, "window_complete": complete, "prompt": (segment["prompt"] or "").strip()[:60],
                })
            if handed_off:
                fc = file_calls[segment["path"]]
                clears.append(handoff_clear(segment, fc[fc.index(first):], calls_by_place))
        ekko = [c for c in main if os.path.basename(c.cwd) == "ekko"]
        results[name] = {
            "window": f"{start:%Y-%m-%d %H:%M}..{end:%Y-%m-%d %H:%M}",
            "calls": len(inside), "main_calls": len(main), "sessions": len(sessions), "cost": total,
            "cost_per_call": total / len(inside),
            "parts": {
                "input": sum(W_IN * c.inp for c in inside) / total,
                "cache_write": sum(c.write_cost for c in inside) / total,
                "cache_read": sum(W_CR * c.cr for c in inside) / total,
                "output": sum(W_OUT * c.out for c in inside) / total,
            },
            "top3": sum(cost for _, cost in sessions.most_common(3)) / total,
            "ctx_mean": mean([c.ctx for c in main]), "ctx_median": median([c.ctx for c in main]),
            "ctx_p90": quantile([c.ctx for c in main], 0.9), "ctx_max": max((c.ctx for c in main), default=None),
            "d_mean": mean(growth), "d_median": median(growth),
            "o_mean": mean([c.out for c in main]), "thinking_share": sum(c.think for c in main) / max(1, sum(c.out for c in main)),
            "effort": collections.Counter(c.effort or "?" for c in main).most_common(),
            "projects": collections.Counter(os.path.basename(c.cwd) or "?" for c in main).most_common(5),
            "ekko_project": {"calls": len(ekko), "cost_per_call": mean([c.cost for c in ekko]), "ctx_mean": mean([c.ctx for c in ekko])},
            "b_median": median(fresh_b), "fresh_starts": len(fresh_b),
            "R_edit_median": median(fresh_r_edit), "R_edit_n": len(fresh_r_edit),
            "R_prod_median": median(fresh_r_prod), "R_prod_n": len(fresh_r_prod),
            "after_100": after_100 / total, "after_200": after_200 / total, "rewrites": rewrites / total,
            "compactions": compactions, "cold_resumes": cold, "large_restarts": restarts, "handoff_clears": clears,
            "abandoned_clears": abandoned,
        }
    return results


def k(value):
    return "-" if value is None else f"{value / 1000:,.1f}k"


def show(results):
    for name, r in results.items():
        print(f"\n=== {name} {r['window']}")
        print(f"{r['calls']:,} calls ({r['main_calls']:,} main) in {r['sessions']} sessions; {r['cost'] / 1e6:.1f}M units, "
              f"{k(r['cost_per_call'])} per call; top 3 sessions {r['top3']:.0%}")
        print("  where it went: " + ", ".join(f"{part} {share:.1%}" for part, share in r["parts"].items()))
        print(f"  context per call: mean {k(r['ctx_mean'])}, median {k(r['ctx_median'])}, p90 {k(r['ctx_p90'])}, max {k(r['ctx_max'])}")
        print(f"  growth d mean {k(r['d_mean'])}, median {k(r['d_median'])}; output o {r['o_mean']:,.0f} per call, {r['thinking_share']:.0%} thinking")
        print(f"  effort {r['effort']}; projects {r['projects']}")
        e = r["ekko_project"]
        print(f"  ekko project alone: {e['calls']} calls, {k(e['cost_per_call'])} per call, context mean {k(e['ctx_mean'])}")
        print(f"  floor b median {k(r['b_median'])} over {r['fresh_starts']} fresh starts; R to first edit {k(r['R_edit_median'])} "
              f"(n {r['R_edit_n']}), to first productive action {k(r['R_prod_median'])} (n {r['R_prod_n']})")
        print(f"  cost after call 100 {r['after_100']:.0%}, after 200 {r['after_200']:.0%}; full-prefix rewrites {r['rewrites']:.1%} of cost")
        auto = [c for c in r["compactions"] if c.get("trigger") == "auto"]
        print(f"  compactions {len(r['compactions'])} ({len(auto)} auto): pre median {k(median([c['pre'] for c in r['compactions'] if c['pre']]))}, "
              f"next call median {k(median([c['b'] for c in r['compactions']]))}")
        for c in r["compactions"]:
            print(f"    after compaction {c['session']} ({c['trigger']}): b {k(c['b'])}, first productive at R {k(c['R'])}, "
                  f"first edit at R {k(c['R_edit'])} in {c['R_edit_calls']} calls")
        shares = [c["share_5h"] for c in r["cold_resumes"] if c["share_5h"] is not None]
        print(f"  cold resumes {len(r['cold_resumes'])}: call median {k(median([c['cost'] for c in r['cold_resumes']]))} units, "
              f"share of the next 5 h median {median(shares) or 0:.1%}" + (f", max {max(shares):.0%}" if shares else ""))
        big = r["large_restarts"]
        print(f"  starts that rewrote a prefix over {k(LARGE)}: {len(big)}, {sum(x['cost'] for x in big) / 1e6:.2f}M units "
              f"({sum(x['cost'] for x in big) / r['cost']:.1%} of the period)")
        for x in big:
            mark = "" if x["window_complete"] else " (window not over)"
            print(f"    {x['session']} {x['source']:<7} {x['at']} ctx {k(x['ctx'])} cost {k(x['cost'])} gap {x['gap_min']} min, "
                  f"5 h share {x['share_5h']:.0%}{mark}: {x['prompt']!r}")
        if r["abandoned_clears"]:
            print(f"  handoff clears that no message followed: {r['abandoned_clears']}")
        for h in r["handoff_clears"]:
            line = (f"    clear {h['session']} {h['at']} b {k(h['b'])}; first {h['first_kind']} at R {k(h['R'])} in {h['R_calls']} calls, "
                    f"first edit/commit at {k(h['R_edit'])} in {h['R_edit_calls']}, first answer at {k(h['R_answer'])} in {h['R_answer_calls']} "
                    f"({h['reads_before']} reads before, {h['transcript_reads']} transcript reads); its life {h['calls']} calls {k(h['cost'])}; "
                    f"cleared {h['cleared']} at {k(h['C_stop'])} {h['gap_min']} min before")
            if "saved_net" in h:
                line += (f"; orientation {k(h['orientation_cost'])} vs 2(b+R) {k(h['bound_2bR'])} vs cold resume {k(h['cold_resume_cost'])}; "
                         f"saved net {k(h['saved_net'])}, gross {k(h['saved_gross'])}")
            print(line + f": {h['prompt']!r}")
        linked = [h for h in r["handoff_clears"] if "saved_net" in h]
        if linked:
            net = sum(h["saved_net"] for h in linked)
            print(f"  handoff clears {len(r['handoff_clears'])}, {len(linked)} linked and productive: saved net {net / 1e6:.2f}M units "
                  f"({net / (r['cost'] + net):.0%} of what the period would have cost), gross {sum(h['saved_gross'] for h in linked) / 1e6:.2f}M")


def main():
    args = sys.argv[1:]
    handoffs_from = local("2026-09-21T01:50")
    until = datetime.datetime.now(LOCAL)
    while args:
        flag = args.pop(0)
        if flag == "--handoffs-from":
            handoffs_from = local(args.pop(0))
        elif flag == "--until":
            until = local(args.pop(0))
    periods = [
        ("baseline", local("2026-09-08T00:00"), local("2026-09-15T00:00")),
        ("interim", local("2026-09-15T00:00"), handoffs_from),
        ("handoffs", handoffs_from, until),
    ]
    calls, files = scan()
    results = report(calls, files, periods)
    show(results)
    os.makedirs(OUT, exist_ok=True)
    with open(os.path.join(OUT, "report.json"), "w") as handle:
        json.dump(results, handle, indent=1, default=str)
    print(f"\nreport: {os.path.join(OUT, 'report.json')}")


if __name__ == "__main__":
    main()
