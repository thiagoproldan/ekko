"""Task 983: does the 'Work silently' rule hold? It went into the user's
CLAUDE.md on 2026-09-30 at 01:04:29 (-03), and names Claude Code's nudge on
purpose: a silent_turn_reminder attachment, 'The user hasn't heard from you in
a while -- say in a few words what you're doing, then continue'.

Over every Claude Code transcript of every profile, main thread only, a turn
runs from one prompt to the next. A text block the model writes in it is
interim when a tool call follows it in the same turn, and final otherwise. A
nudge is narrated when the model's next block after it is interim text,
answered by the final reply when that block ends the turn, and silent when it
is a tool call. A turn is under the rule when the last instructions attachment
before its first answer quotes the rule -- what the session was given, not
what the file held then; a transcript without one is placed by the clock.

    nix develop -c python3 evals/claude-code/silence.py [--since 2026-09-23] [--until ...] [--list]

Times are local (UTC-3). Only reads ~/.claude*/projects/, skipping the paired
runs' folder; the numbers land in target/evals/claude-code/silence/report.json.
"""

import argparse
import collections
import datetime
import glob
import json
import os

HOME = os.path.expanduser("~")
LOCAL = datetime.timezone(datetime.timedelta(hours=-3))
OUT = os.path.normpath(
    os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "target", "evals", "claude-code", "silence")
)
# The runs of the paired test (evals/paired/harness.py) live under a folder of
# this name: they are experiments, not the user's sessions.
SKIP = "ekko-paired"
RULE = "Work silently: write no text between tool calls"
RULE_AT = datetime.datetime(2026, 9, 30, 1, 4, 29, tzinfo=LOCAL)


def when(stamp):
    return datetime.datetime.fromisoformat(stamp.replace("Z", "+00:00")).astimezone(LOCAL)


def local(text):
    return datetime.datetime.fromisoformat(text).replace(tzinfo=LOCAL)


def profiles():
    return sorted(path for path in glob.glob(os.path.join(HOME, ".claude*")) if os.path.isdir(os.path.join(path, "projects")))


def is_prompt(row):
    """Whether a row starts a turn: a tool's result, a slash command and its
    output, and what Claude Code adds as meta do not."""
    if row.get("type") != "user" or row.get("isMeta"):
        return False
    content = (row.get("message") or {}).get("content")
    if isinstance(content, str):
        text = content
    else:
        blocks = [block for block in content or [] if isinstance(block, dict)]
        if any(block.get("type") == "tool_result" for block in blocks):
            return False
        text = " ".join(block.get("text", "") for block in blocks if block.get("type") == "text")
    text = text.strip()
    return bool(text) and not text.startswith(("<command-", "<local-command"))


class Turn:
    __slots__ = ("profile", "folder", "session", "at", "rule", "by", "model", "effort", "version", "blocks", "nudges", "interim")

    def tally(self):
        """The turn's counts. blocks holds each text block, and None for each
        tool call; nudges, the index of the block after each."""
        tools = [i for i, block in enumerate(self.blocks) if block is None]
        last = tools[-1] if tools else -1
        texts = [i for i, block in enumerate(self.blocks) if block is not None]
        interim = [i for i in texts if i < last]
        after = [self.blocks[i] if i < len(self.blocks) else "none" for i in self.nudges]
        self.interim = [self.blocks[i] for i in interim]
        return {
            "turns": 1,
            "with_tools": int(bool(tools)),
            "texts": len(texts),
            "interim": len(interim),
            "interim_chars": sum(len(self.blocks[i]) for i in interim),
            "final": len(texts) - len(interim),
            "nudges": len(self.nudges),
            "narrated": sum(1 for i in self.nudges if i < last and self.blocks[i] is not None),
            "answered": sum(1 for i in self.nudges if i < len(self.blocks) and i > last),
            "silent": sum(1 for kind in after if kind is None),
        }


def turns_of(profile, path, seen):
    """Each turn of one transcript that the model answered, in order."""
    rule = None  # what the last instructions attachment said; None: none yet
    turn = None
    with open(path, errors="replace") as handle:
        for raw in handle:
            try:
                row = json.loads(raw)
            except ValueError:
                continue
            # a resumed conversation carries rows already counted elsewhere
            uuid = row.get("uuid")
            if uuid in seen:
                continue
            if uuid:
                seen.add(uuid)
            if row.get("isSidechain") or not row.get("timestamp"):
                continue
            kind = row.get("type")
            if kind == "attachment":
                attachment = row.get("attachment") or {}
                if attachment.get("type") == "instructions":
                    rule = RULE in json.dumps(attachment, ensure_ascii=False)
                elif attachment.get("type") == "silent_turn_reminder" and turn is not None:
                    turn.nudges.append(len(turn.blocks))
            elif is_prompt(row):
                if turn is not None and turn.blocks:
                    yield turn
                turn = Turn()
                turn.profile, turn.folder, turn.session = profile, os.path.basename(os.path.dirname(path)), os.path.basename(path)[:8]
                turn.at = when(row["timestamp"])
                turn.rule, turn.by, turn.model, turn.effort, turn.version = None, None, "", "", ""
                turn.blocks, turn.nudges = [], []
            elif kind == "assistant" and turn is not None:
                message = row.get("message") or {}
                if message.get("model") == "<synthetic>":
                    continue
                if turn.rule is None:
                    turn.rule, turn.by = (rule, "attachment") if rule is not None else (turn.at >= RULE_AT, "clock")
                    turn.model, turn.effort, turn.version = message.get("model") or "", row.get("effort") or "", row.get("version") or ""
                for block in message.get("content") or []:
                    if not isinstance(block, dict):
                        continue
                    if block.get("type") == "text" and block.get("text", "").strip():
                        turn.blocks.append(block["text"])
                    elif block.get("type") == "tool_use":
                        turn.blocks.append(None)
    if turn is not None and turn.blocks:
        yield turn


def scan(since, until):
    paths = []
    for base in profiles():
        pattern = os.path.join(base, "projects", "**", "*.jsonl")
        paths += [
            (os.path.basename(base), path)
            for path in glob.glob(pattern, recursive=True)
            if SKIP not in path and "/subagents/" not in path and os.path.getmtime(path) >= since.timestamp()
        ]
    # the conversation a resume copied from is counted where it was held first
    paths.sort(key=lambda pair: os.path.getmtime(pair[1]))
    seen = set()
    return [turn for profile, path in paths for turn in turns_of(profile, path, seen) if since <= turn.at < until]


def add(total, counts):
    for key, value in counts.items():
        total[key] = total.get(key, 0) + value


def rates(total):
    turns, nudges = total.get("turns", 0), total.get("nudges", 0)
    return {
        "interim_per_turn": round(total.get("interim", 0) / turns, 2) if turns else None,
        "interim_per_tool_turn": round(total.get("interim", 0) / total["with_tools"], 2) if total.get("with_tools") else None,
        "narrated_share": round(total.get("narrated", 0) / nudges, 2) if nudges else None,
    }


def phase(turn):
    return "rule" if turn.rule else "before"


def report(turns):
    groups = collections.defaultdict(dict)
    folders = collections.defaultdict(dict)
    sessions = collections.OrderedDict()
    for turn in sorted(turns, key=lambda turn: turn.at):
        counts = turn.tally()
        add(groups[phase(turn)], counts)
        add(folders[(turn.folder, phase(turn))], counts)
        key = (turn.profile, turn.folder, turn.session, phase(turn))
        if key not in sessions:
            sessions[key] = {
                "first": turn.at, "profile": turn.profile, "folder": turn.folder, "session": turn.session, "phase": phase(turn),
                "by": collections.Counter(), "model": collections.Counter(), "effort": collections.Counter(), "version": turn.version,
            }
        entry = sessions[key]
        entry["last"] = turn.at
        entry["by"][turn.by] += 1
        entry["model"][turn.model] += 1
        entry["effort"][turn.effort or "-"] += 1
        add(entry.setdefault("counts", {}), counts)
        # under the rule, what still slipped through, to read
        if turn.rule:
            entry.setdefault("interim_texts", []).extend(f"{turn.at:%H:%M} {' '.join(text.split())[:200]}" for text in turn.interim)
    for entry in sessions.values():
        for field in ("by", "model", "effort"):
            entry[field] = dict(entry[field])
    return {
        "phases": {name: {**total, **rates(total)} for name, total in groups.items()},
        "folders": [{"folder": folder, "phase": name, **total, **rates(total)} for (folder, name), total in sorted(folders.items())],
        "sessions": [{**entry, **rates(entry["counts"])} for entry in sessions.values()],
    }


def show(results, listing):
    def line(label, total, rate):
        print(
            f"  {label:<34} turns {total['turns']:>4} (tools {total['with_tools']:>4})  interim {total['interim']:>4} "
            f"({rate['interim_per_turn']}/turn, {rate['interim_per_tool_turn']}/tool turn, {total['interim_chars']/1e3:.1f}k chars)  "
            f"nudges {total['nudges']:>3}: narrated {total['narrated']:>3} ({rate['narrated_share']}), "
            f"reply {total['answered']:>2}, silent {total['silent']:>3}"
        )

    for name in ("before", "rule"):
        if name in results["phases"]:
            line(name, results["phases"][name], results["phases"][name])
    print("by folder:")
    for entry in results["folders"]:
        line(f"{entry['folder'][:26]} {entry['phase']}", entry, entry)
    if listing:
        print("by session:")
        for entry in results["sessions"]:
            model = max(entry["model"], key=entry["model"].get)
            effort = max(entry["effort"], key=entry["effort"].get)
            print(
                f"  {entry['first']:%m-%d %H:%M}-{entry['last']:%H:%M} {entry['profile']:<16} {entry['folder'][:24]:<24} {entry['session']} "
                f"{entry['phase']:<6} by {'+'.join(entry['by'])} {model} {effort} v{entry['version']}"
            )
            line("", entry["counts"], entry)
            for text in entry.get("interim_texts", []):
                print(f"      interim {text}")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--since", default="2026-09-23", help="local time")
    parser.add_argument("--until", help="local time")
    parser.add_argument("--list", action="store_true", help="each session, with its model, effort, Claude Code version and, under the rule, its interim texts")
    opts = parser.parse_args()
    since = local(opts.since)
    until = local(opts.until) if opts.until else datetime.datetime.now(LOCAL)
    print(f"rule {RULE!r}, live {RULE_AT:%Y-%m-%d %H:%M:%S}; turns from {since:%Y-%m-%d %H:%M} to {until:%Y-%m-%d %H:%M}; profiles {[os.path.basename(p) for p in profiles()]}")
    results = report(scan(since, until))
    results["config"] = {"rule": RULE, "rule_at": RULE_AT, "since": since, "until": until, "profiles": profiles()}
    show(results, opts.list)
    os.makedirs(OUT, exist_ok=True)
    with open(os.path.join(OUT, "report.json"), "w") as handle:
        json.dump(results, handle, indent=1, default=str)
    print(f"\nreport: {os.path.join(OUT, 'report.json')}")


if __name__ == "__main__":
    main()
