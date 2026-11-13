"""Builds assets/selawik/flare-selawik-*.ttf from Microsoft's Selawik 1.01 release (rule 1232).

Selawik is Segoe UI's open stand-in, but its hhea line (ascent 2027, descent 431: 1.2 em) is shorter than
Segoe UI's (2210 and 514: 1.33 em), and Slint's text layout reads hhea. This sets hhea to Segoe UI's values
(Selawik's own usWinAscent and usWinDescent already carry them), so a Mac line of text is as tall as a
Windows one. The OFL reserves the name Selawik, so the changed faces are renamed "Flare Selawik". The glyphs
are not touched.

    pip install fonttools
    python tools/fonts/make-flare-selawik.py <unzipped Selawik_Release folder> assets/selawik
"""

import sys
from pathlib import Path

from fontTools.ttLib import TTFont

FACES = {"selawk.ttf": "Regular", "selawksb.ttf": "Semibold", "selawkb.ttf": "Bold"}
# Segoe UI's hhea, and Selawik's own usWinAscent and usWinDescent.
ASCENT, DESCENT = 2210, -514
NEW = "Flare Selawik"
NOTE = (
    "Modified by Flare (2026-11-13): hhea line metrics set to Segoe UI's, renamed. "
    "Based on Selawik 1.01, (c) 2015 Microsoft Corporation, SIL Open Font License 1.1."
)


def rename(name_table):
    for rec in name_table.names:
        text = rec.toUnicode()
        if rec.nameID in (1, 3, 4, 16):
            rec.string = text.replace("Selawik", NEW)
        elif rec.nameID == 6:
            rec.string = text.replace("Selawik", NEW.replace(" ", ""))
    name_table.setName(NOTE, 10, 3, 1, 0x409)


def main(src, dst):
    src, dst = Path(src), Path(dst)
    dst.mkdir(parents=True, exist_ok=True)
    for file, style in FACES.items():
        font = TTFont(src / file, recalcTimestamp=False)
        os2 = font["OS/2"]
        assert os2.usWinAscent == ASCENT and os2.usWinDescent == -DESCENT, file
        font["hhea"].ascent = ASCENT
        font["hhea"].descent = DESCENT
        font["hhea"].lineGap = 0
        rename(font["name"])
        out = dst / f"flare-selawik-{style.lower()}.ttf"
        font.save(out)
        print(out)


if __name__ == "__main__":
    main(*sys.argv[1:3])
