# The `/ekko` skill

`SKILL.md` teaches a Claude Code agent to drive an Ekko board: when to reach
for it over the in-conversation todo list, how to avoid the traps (ids get
recycled, the archive renumbers, the toggles are not retry-safe), and which
commands to prefer when something will parse the output.

It lives here rather than loose in `~/.claude/skills/` so it moves with the
code it documents. It has already gone stale once -- it recommended the
toggles for a while after `--set` existed -- and the version that ships with
a given Ekko can only describe commands that Ekko actually has.

Install it by pointing a skills directory at this file, for example through
home-manager:

    home.file.".claude/skills/ekko/SKILL.md".source =
      "${inputs.ekko}/skill/SKILL.md";
