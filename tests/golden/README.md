# Golden reference output

Raw terminal output (ANSI colors included) captured from the **original
taskbook v0.4.0 JavaScript build** — the authoritative reference Ekko's
`render` module is diffed against, byte for byte, by the tests in
`src/render.rs`.

Regenerate (should produce byte-identical files) by extracting the JS
source from the `v0.4.0` git tag and replaying this exact command sequence
against a fresh, pre-created data directory, with `FORCE_COLOR=1`:

    --task @coding 'Normal priority task'
    --task @coding 'Medium priority task' p:2
    --task @coding 'High priority task' p:3
    --begin 2
    --check 1
    --star 3
    --note @coding 'A reference note'
    --task @writing 'Another board entirely'
    --delete 4

then capture: no-flags (board.ans), --timeline, --archive,
--check 999999 (error.ans), --task @coding 'created ok' (success-msg.ans).

These are frozen artifacts: their provenance is the JS build, so the tests
prove Ekko matches taskbook's real output, not merely that it matches
itself.

## These files embed the date they were captured on

`timeline.ans` and `archive.ans` print a date header, and tag it `[Today]`
when it matches the day of the run — the current files say
`Mon Aug 24 2026 [Today]`, because that is when they were captured.

So the tests do **not** call the system clock. `render_with` pins the
renderer to `GOLDEN_DAY` (`src/render.rs`), and the item fixtures take
both their `date` and their `timestamp` from that same pinned instant.
That is what keeps them passing on any day, in any timezone, rather than
only on the day of capture.

**If you regenerate these files, update `GOLDEN_DAY` to the new capture
date in the same commit** — otherwise the timeline and archive tests will
fail on the date header alone, while every other byte still matches.
