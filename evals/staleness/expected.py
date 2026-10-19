"""Task 2197: the expected share of stale starts, where a named task's close
lies in a window no record settles: the chance it fell between the handoff
and the start is that interval's share of the window [claim, last update],
under a uniform close time within it."""

import datetime
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import config as C  # noqa: E402
import starts as S  # noqa: E402
from classes import kind, resume_ask  # noqa: E402

rows = [json.loads(line) for line in open(os.path.join(C.DATA, "starts.jsonl"))]
user = [r for r in rows if r["source"] in ("startup", "clear", "resume") and kind(r.get("first_prompt")) == "user" and "named_changed" in r]


def p_stale(r):
    if (r.get("commits_since") or 0) > 0 or r.get("named_changed"):
        return 1.0
    b = S.board(r["board"])
    lo_t = datetime.datetime.fromisoformat(r["written"]).timestamp()
    hi_t = datetime.datetime.fromisoformat(r["at"]).timestamp()
    p_none = 1.0
    for i in r.get("named_unknown") or []:
        item = b["items"].get(i)
        if not item:
            continue
        lower = ((item.get("doneBy") or {}).get("since") or item.get("_timestamp") or 0) / 1000
        upper = (item.get("updatedAt") or 0) / 1000
        if upper <= lower:
            continue
        overlap = max(0.0, min(hi_t, upper) - max(lo_t, lower))
        p_none *= 1 - overlap / (upper - lower)
    return 1 - p_none


for label, g in (("all", user), ("startup", [r for r in user if r["source"] == "startup"]), ("clear", [r for r in user if r["source"] == "clear"]),
                 ("resume", [r for r in user if r["source"] == "resume"]), ("resume asks", [r for r in user if resume_ask(r.get("first_prompt"))]),
                 ("other prompts", [r for r in user if not resume_ask(r.get("first_prompt"))])):
    e = sum(p_stale(r) for r in g)
    print(f"  {label:14} n {len(g):3}: expected stale {e:.1f} ({e / len(g):.0%})")
