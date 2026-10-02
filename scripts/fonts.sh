#!/usr/bin/env bash
# The fonts of an artifact's page (task 1152), written to assets/fonts/ from
# the Inter 4.1 and Source Serif 4.005 that flake.lock's nixpkgs holds, both
# under the SIL Open Font License 1.1 (assets/fonts/OFL.txt).
#
#   scripts/fonts.sh
#
# Each variable font is cut to Latin, to the weights the page draws (400 to
# 700) and to the OpenType features a browser applies by default, and saved
# as WOFF2; the optical size axis stays. That is 212 kB for the three, where
# the fonts as nixpkgs ships them are 4 MB.
#
# A cut font is a Modified Version under the OFL, which may not carry a
# Reserved Font Name: Source Serif 4 reserves 'Source', so its cuts are
# renamed Ekko Serif, as the OFL FAQ asks of a subset for the web
# (https://openfontlicense.org/webfonts-and-reserved-font-names/). Inter
# reserves no name and keeps its own. Each file keeps its copyright,
# trademark and licence records, and the run fails if a name of Ekko Serif
# still says Source anywhere else.
set -euo pipefail

root=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
nixpkgs="(builtins.getFlake \"git+file://$root\").inputs.nixpkgs.legacyPackages.\${builtins.currentSystem}"
built() { nix build --no-link --print-out-paths --impure --expr "$1"; }
python=$(built "($nixpkgs).python3.withPackages (p: [ p.fonttools p.brotli ])")/bin/python3
inter=$(built "($nixpkgs).inter")/share/fonts/truetype/InterVariable.ttf
serif=$(built "($nixpkgs).source-serif")/share/fonts/variable

"$python" - "$inter" "$serif" "$root/assets/fonts" <<'EOF'
import os
import sys
import tempfile

from fontTools import subset
from fontTools.ttLib import TTFont
from fontTools.varLib import instancer

inter, serif, out = sys.argv[1:]
# Google Fonts' latin range, with the arrows and the check mark the page draws.
LATIN = "U+0000-00FF,U+0131,U+0152-0153,U+02BB-02BC,U+02C6,U+02DA,U+02DC,U+0304,U+0308,U+0329,U+2000-206F,U+20AC,U+2122,U+2190-2193,U+2212,U+2215,U+2713,U+FEFF,U+FFFD"
# Copyright, trademark, licence and its address: notices, kept as they are.
NOTICES = {0, 7, 13, 14}
RENAMED = [("Source Serif 4", "Ekko Serif"), ("SourceSerif4", "EkkoSerif")]


def cut(source, target, rename):
    # The source's own timestamp, not the hour of the run: the same input makes
    # the same file.
    font = instancer.instantiateVariableFont(TTFont(source, recalcTimestamp=False), {"wght": (400, 700)})
    if rename:
        for record in font["name"].names:
            if record.nameID not in NOTICES:
                text = record.toUnicode()
                for old, new in RENAMED:
                    text = text.replace(old, new)
                record.string = text
        left = [(record.nameID, record.toUnicode()) for record in font["name"].names if record.nameID not in NOTICES and "Source" in record.toUnicode()]
        if left:
            sys.exit(f"{target}: a name still says Source: {left}")
    with tempfile.TemporaryDirectory() as work:
        instance = os.path.join(work, "font")
        font.save(instance)
        subset.main([instance, f"--unicodes={LATIN}", "--name-IDs=*", "--name-languages=*", "--flavor=woff2", f"--output-file={target}"])
    made = TTFont(target)
    axes = ", ".join(f"{axis.axisTag} {axis.minValue:g}-{axis.maxValue:g}" for axis in made["fvar"].axes)
    print(f"{os.path.basename(target)}: {os.path.getsize(target)} bytes, {made['name'].getDebugName(1)}, {axes}, {len(made.getGlyphOrder())} glyphs")


cut(inter, os.path.join(out, "inter.woff2"), rename=False)
cut(os.path.join(serif, "SourceSerif4Variable-Roman.otf"), os.path.join(out, "ekko-serif.woff2"), rename=True)
cut(os.path.join(serif, "SourceSerif4Variable-Italic.otf"), os.path.join(out, "ekko-serif-italic.woff2"), rename=True)
EOF
