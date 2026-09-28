"""Task 884: would a 'not modified' answer pay off on context and search?

prime already answers in one line when nothing moved since the cursor the
caller holds (if_rev). This counts, over every Claude Code transcript of every
profile, how often an ekko read returned exactly what the same conversation
had already been given by the same call since its last compaction: the bytes a
'not modified' answer would have saved. A call with the same arguments whose
answer changed is counted apart, since it needed the full answer anyway.

    nix develop -c python3 evals/claude-code/rereads.py

Only reads ~/.claude*/projects/, skipping the paired runs' folder.
"""

import collections
import glob
import json
import os
import re

HOME = os.path.expanduser("~")
SKIP = "ekko-paired"
EKKO_READ = re.compile(r"^mcp__(?:plugin_ekko_)?ekko__(context|search|prime|next|changes)$")


def profiles():
    return sorted(path for path in glob.glob(os.path.join(HOME, ".claude*")) if os.path.isdir(os.path.join(path, "projects")))


def text_of(content):
    if isinstance(content, str):
        return content
    return "".join(part.get("text", "") for part in content or [] if isinstance(part, dict))


def key_of(tool, args):
    """What makes two calls the same read."""
    if tool == "context":
        items = args.get("items") or [args.get("item")]
        return (tool, tuple(sorted(str(item) for item in items)), args.get("detail", "concise"))
    return (tool, json.dumps(args, sort_keys=True))


def compaction(record):
    return (record.get("type") == "system" and record.get("subtype") == "compact_boundary") or record.get("isCompactSummary")


def main():
    stats = collections.defaultdict(lambda: collections.Counter())
    all_results = 0
    files = 0
    for base in profiles():
        for path in glob.glob(os.path.join(base, "projects", "**", "*.jsonl"), recursive=True):
            if SKIP in path:
                continue
            files += 1
            pending = {}
            held = {}
            seen_items = set()
            with open(path, encoding="utf-8", errors="replace") as handle:
                for line in handle:
                    try:
                        record = json.loads(line)
                    except ValueError:
                        continue
                    if compaction(record):
                        held = {}
                        seen_items = set()
                    content = (record.get("message") or {}).get("content")
                    if not isinstance(content, list):
                        continue
                    for block in content:
                        if not isinstance(block, dict):
                            continue
                        if block.get("type") == "tool_use":
                            match = EKKO_READ.match(block.get("name", ""))
                            if match:
                                pending[block.get("id")] = key_of(match.group(1), block.get("input") or {})
                        elif block.get("type") == "tool_result":
                            result = text_of(block.get("content"))
                            all_results += len(result)
                            key = pending.pop(block.get("tool_use_id"), None)
                            if key is None:
                                continue
                            counter = stats[key[0]]
                            counter["calls"] += 1
                            counter["chars"] += len(result)
                            if key[0] == "context":
                                # A call whose items were partly given before:
                                # their share of its answer, as an upper bound.
                                items = set(key[1])
                                again = items & seen_items
                                if again:
                                    counter["overlap calls"] += 1
                                    counter["overlap chars"] += len(result) * len(again) // len(items)
                                seen_items |= items
                            if key in held:
                                if held[key] == result:
                                    counter["repeat calls"] += 1
                                    counter["repeat chars"] += len(result)
                                else:
                                    counter["changed calls"] += 1
                            held[key] = result
    ekko = sum(counter["chars"] for counter in stats.values())
    repeat = sum(counter["repeat chars"] for counter in stats.values())
    print(f"transcripts {files}; every tool result {all_results:,} chars; ekko reads {ekko:,} chars ({ekko / max(all_results, 1):.1%} of them)")
    for tool, counter in sorted(stats.items()):
        print(
            f"  {tool:8} calls {counter['calls']:6}  chars {counter['chars']:>11,}  "
            f"repeats {counter['repeat calls']:5} = {counter['repeat chars']:>10,} chars  changed {counter['changed calls']:5}"
        )
    print(f"a 'not modified' answer on every exact repeat: {repeat:,} chars, {repeat / max(ekko, 1):.1%} of ekko's reads, {repeat / max(all_results, 1):.2%} of every tool result")
    overlap = stats["context"]
    print(f"context calls naming an item given before: {overlap['overlap calls']}, at most {overlap['overlap chars']:,} chars, {overlap['overlap chars'] / max(ekko, 1):.1%} of ekko's reads")


if __name__ == "__main__":
    main()
