#!/usr/bin/env bash
# Renders one ekko command's real ANSI output to a PNG for the readme.
#
#   shot.sh <out.png> <command...>
#
# The command is run with FORCE_COLOR so the renderer emits the same bytes
# it would to a terminal; nothing here re-implements the colours.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUT="$1"; shift

RAW="$(FORCE_COLOR=1 "$@")"
BODY="$(printf '%s\n' "$RAW" | gawk -f "$HERE/ansi2html.awk")"

# Chrome screenshots the whole window, so the window has to be the size of
# the content or the PNG carries a field of empty canvas. Measured off the
# plain text: JetBrains Mono advances 0.6em (10.2px at 17px) and the line
# box is 1.62em. The constants below are the paddings declared in the CSS.
read -r COLS ROWS <<<"$(printf '%s\n' "$RAW" \
  | gawk 'BEGIN{ESC=sprintf("%c",27)}
          {gsub(ESC "\\[[0-9;]*m",""); if (length($0)>w) w=length($0); n++}
          END{print w, n}')"

read -r WIDTH HEIGHT <<<"$(gawk -v c="$COLS" -v r="$ROWS" \
  'BEGIN { printf "%d %d\n", 32+34+(c*10.2)+34+32, 28+41+6+(r*27.54)+26+40 }')"

# U+23F8 comes from Noto Sans Symbols 2 -- no mono face in the usual set
# carries it -- and its advance is narrower than the mono cell, so the
# column would drift by a fraction. Forcing one character cell puts it
# back where a terminal actually draws it.
BODY="${BODY//⏸/<span class=\"pause\">⏸</span>}"

PAGE="$(mktemp --suffix=.html)"
trap 'rm -f "$PAGE"' EXIT
cat > "$PAGE" <<HTML
<!doctype html>
<meta charset="utf-8">
<style>
  :root {
    --bg: #16161e;
    --fg: #c0caf5;
    --c30: #414868; --c31: #f7768e; --c32: #9ece6a; --c33: #e0af68;
    --c34: #7aa2f7; --c35: #bb9af7; --c36: #7dcfff; --c37: #a9b1d6;
    --c90: #565f89; --c91: #f7768e; --c92: #9ece6a; --c93: #e0af68;
    --c94: #7aa2f7; --c95: #bb9af7; --c96: #7dcfff; --c97: #c0caf5;
  }
  html, body { margin: 0; background: transparent; }
  body { padding: 28px 32px 40px; display: inline-block; }
  .window {
    background: var(--bg);
    border-radius: 12px;
    box-shadow: 0 18px 44px rgba(0,0,0,.34), 0 3px 10px rgba(0,0,0,.22);
    padding: 0 0 26px;
    display: inline-block;
    min-width: 640px;
  }
  .bar { padding: 20px 0 8px 22px; }
  .dot { display: inline-block; width: 13px; height: 13px; border-radius: 50%; margin-right: 8px; }
  .r { background: #ff5f57 } .y { background: #febc2e } .g { background: #28c840 }
  pre {
    margin: 0; padding: 6px 34px 0;
    font-family: "JetBrains Mono", "DejaVu Sans Mono", "Noto Sans Symbols 2", monospace;
    font-size: 17px; line-height: 1.62;
    color: var(--fg);
    font-variant-ligatures: none;
    white-space: pre;
  }
  .pause { display: inline-block; width: 1ch; }
  .ul { text-decoration: underline }
  .st { text-decoration: line-through }
  .ul.st { text-decoration: underline line-through }
  .c30{color:var(--c30)} .c31{color:var(--c31)} .c32{color:var(--c32)} .c33{color:var(--c33)}
  .c34{color:var(--c34)} .c35{color:var(--c35)} .c36{color:var(--c36)} .c37{color:var(--c37)}
  .c90{color:var(--c90)} .c91{color:var(--c91)} .c92{color:var(--c92)} .c93{color:var(--c93)}
  .c94{color:var(--c94)} .c95{color:var(--c95)} .c96{color:var(--c96)} .c97{color:var(--c97)}
</style>
<div class="window">
  <div class="bar"><span class="dot r"></span><span class="dot y"></span><span class="dot g"></span></div>
  <pre>$BODY</pre>
</div>
HTML

google-chrome-stable \
  --headless --disable-gpu --hide-scrollbars \
  --force-device-scale-factor=2 \
  --default-background-color=00000000 \
  --screenshot="$OUT" \
  --window-size="$WIDTH,$HEIGHT" \
  "file://$PAGE" 2>/dev/null

echo "wrote $OUT  (${COLS}x${ROWS} chars -> ${WIDTH}x${HEIGHT} css px)"
