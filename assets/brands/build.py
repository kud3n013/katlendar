#!/usr/bin/env python3
"""Builds brands.ttf, the font of service logos katlendar shows as the
icon of a task list that comes from that service.

Each logo is a 24x24 SVG from Simple Icons (https://simpleicons.org, CC0).
Its one path becomes a glyph at a private-use code point, sized to sit
beside the Lucide icons. Run it again after adding a logo here:

    python3 assets/brands/build.py

and give the new code point a name in src/icons.rs.
"""

import re
from pathlib import Path

from fontTools.fontBuilder import FontBuilder
from fontTools.pens.cu2quPen import Cu2QuPen
from fontTools.pens.transformPen import TransformPen
from fontTools.pens.ttGlyphPen import TTGlyphPen
from fontTools.svgLib.path import parse_path

HERE = Path(__file__).parent
# the logos, by the code point they get; add new ones at the end
LOGOS = [(0xF8E0, "todoist"), (0xF8E1, "nextcloud")]
EM = 1000
# Lucide's strokes reach the edges of its em; a filled logo the same size
# looks heavier, so it is drawn a little smaller and centred
SIZE, LEFT, BOTTOM = 860, 70, 50


def glyph(name):
    d = re.search(r'<path d="([^"]+)"', (HERE / f"{name}.svg").read_text()).group(1)
    pen = TTGlyphPen(None)
    scale = SIZE / 24
    # SVG counts y downwards, fonts upwards
    flip = TransformPen(Cu2QuPen(pen, max_err=1.0, reverse_direction=True), (scale, 0, 0, -scale, LEFT, BOTTOM + SIZE))
    parse_path(d, flip)
    return pen.glyph()


def main():
    names = [".notdef"] + [n for _, n in LOGOS]
    fb = FontBuilder(EM, isTTF=True)
    fb.setupGlyphOrder(names)
    fb.setupCharacterMap({cp: n for cp, n in LOGOS})
    glyphs = {".notdef": TTGlyphPen(None).glyph()}
    glyphs.update({n: glyph(n) for _, n in LOGOS})
    fb.setupGlyf(glyphs)
    fb.setupHorizontalMetrics({n: (EM, getattr(glyphs[n], "xMin", 0)) for n in names})
    fb.setupHorizontalHeader(ascent=EM, descent=0)
    fb.setupNameTable({"familyName": "katlendar brands", "styleName": "Regular"})
    fb.setupOS2(sTypoAscender=EM, sTypoDescender=0, usWinAscent=EM, usWinDescent=0)
    fb.setupPost()
    fb.save(HERE / "brands.ttf")


if __name__ == "__main__":
    main()
