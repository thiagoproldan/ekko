# The `/ekko` skill

`SKILL.md` teaches a Claude Code agent to drive an Ekko board: when to reach
for it over the in-conversation todo list, how to avoid the traps (ids get
recycled, the archive renumbers, the toggles are not retry-safe), and which
commands to prefer when something will parse the output.

It lives here rather than loose in `~/.claude/skills/` so it moves with the
code it documents. It has already gone stale twice -- it recommended the
toggles for a while after `--set` existed, and it described the board as of
the paused state for six features after that -- and the version that ships
with a given Ekko can only describe commands that Ekko actually has.

Note which half of that the packaging solves. Pinning the skill to the same
revision as the binary guarantees it never describes a command the binary
lacks. Nothing guarantees the reverse: a feature can ship with the skill
still silent about it, which is exactly how it went stale the second time.
Adding a flag means editing this file in the same commit.

Install it by pointing a skills directory at this file, for example through
home-manager:

    home.file.".claude/skills/ekko/SKILL.md".source =
      "${inputs.ekko}/skill/SKILL.md";
