# Capturing the readme screenshots

`shot.sh` renders one real ekko command to a PNG:

```bash
$ media/capture/shot.sh media/header-boards.png ./target/debug/ekko --ekko-dir /tmp/demo
$ media/capture/shot.sh media/path.png ./target/debug/ekko --project compositor --path
```

It runs the command with `FORCE_COLOR=1`, converts the ANSI it emits to HTML
(`ansi2html.awk`), and screenshots the result with headless Chrome. Nothing
here re-implements the colours -- if a screenshot is wrong, the renderer is
wrong, which is the point.

Needs `google-chrome-stable` and `gawk`.

## Things learned the hard way, kept here so they are not rediscovered

**The pause glyph is not in any mono font.** U+23F8 comes from Noto Sans
Symbols 2, whose advance is narrower than the mono cell, so the column drifts
by a fraction unless it is forced to `1ch`. `shot.sh` wraps it. Every other
glyph Ekko uses -- U+2610 ☐, U+2714 ✔, U+2298 ⊘, U+25CF ●, U+21E0 ⇠ -- is in
DejaVu Sans Mono, which is why that face is second in the stack.

**Chrome screenshots the window, not the content**, so the window has to be
sized to the text or the PNG carries a field of empty canvas. The size is
computed from the plain text: JetBrains Mono advances 0.6em, the line box is
1.62em, and the rest is the paddings declared in the CSS. Change one and the
other has to follow.

**The style stack has to track state, not nest.** Ekko closes a nested style
by re-opening the outer one (chalk's rule), so a stack would misread it.
`ansi2html.awk` carries "current colour, current flags" instead.

**A capture run gives every item the same date**, which makes the timeline
view look identical to the board view. Backdate a few `_timestamp`/`_date`
pairs in `storage.json` before shooting `--timeline`, or the picture argues
against the feature it is meant to show.
