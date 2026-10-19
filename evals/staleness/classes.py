"""Who started each session: the user, ekko agents (a born session), or a
probe that a harness or a test typed."""

import re

PROBE = re.compile(
    r"^(Reply with|First call the EnterWorktree|Do not enter any worktree|Without using any tool|Call the |Qual é o nome do pacote|teste$|"
    r"▎ Use a ferramenta ask|Responda apenas)"
)
RESUME = re.compile(r"(?i)^\s*(onde paramos|continu|pode seguir|ta,? pode seguir|seguindo|retom|prossig|voltando|where did we|resume)")


def kind(prompt):
    text = (prompt or "").strip()
    if not text:
        return "none"
    if text.startswith("ekko agents started this session"):
        return "born"
    if PROBE.search(text):
        return "probe"
    return "user"


def resume_ask(prompt):
    return bool(RESUME.search((prompt or "").strip()))


def born_tasks(prompt):
    head = (prompt or "").split(" on this project", 1)[0]
    return {int(n) for n in re.findall(r"\d+", head)}
