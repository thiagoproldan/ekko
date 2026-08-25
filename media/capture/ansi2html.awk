#!/usr/bin/gawk -f
# Turns the ANSI ekko writes into HTML spans, preserving exactly the
# attributes the renderer emits: 8 colours plus their bright forms, the
# underline pair (4/24) and the strike pair (9/29).
#
# Tracks state rather than nesting: ekko opens styles inside styles and
# closes the inner one by RE-OPENING the outer (chalk's rule), so a stack
# would misread it. Carrying "current colour, current flags" and wrapping
# each literal run reproduces what a terminal shows.

function esc(s) {
    gsub(/&/, "\\&amp;", s)
    gsub(/</, "\\&lt;", s)
    gsub(/>/, "\\&gt;", s)
    return s
}

function flush(text,   cls) {
    if (text == "") return
    cls = ""
    if (fg != "") cls = cls " c" fg
    if (ul) cls = cls " ul"
    if (st) cls = cls " st"
    if (cls == "") printf "%s", esc(text)
    else printf "<span class=\"%s\">%s</span>", substr(cls, 2), esc(text)
}

function apply(codes,   n, i, parts, c) {
    n = split(codes, parts, ";")
    for (i = 1; i <= n; i++) {
        c = parts[i] + 0
        if (c == 0)                     { fg = ""; ul = 0; st = 0 }
        else if (c == 4)                { ul = 1 }
        else if (c == 24)               { ul = 0 }
        else if (c == 9)                { st = 1 }
        else if (c == 29)               { st = 0 }
        else if (c == 39)               { fg = "" }
        else if (c >= 30 && c <= 37)    { fg = c "" }
        else if (c >= 90 && c <= 97)    { fg = c "" }
    }
}

BEGIN { fg = ""; ul = 0; st = 0; ESC = sprintf("%c", 27) }

{
    line = $0
    while (match(line, ESC "\\[[0-9;]*m")) {
        flush(substr(line, 1, RSTART - 1))
        seq = substr(line, RSTART, RLENGTH)
        apply(substr(seq, 3, length(seq) - 3))
        line = substr(line, RSTART + RLENGTH)
    }
    flush(line)
    printf "\n"
}
