"""Build `assets/fonts/OndinIcons.ttf` from `glyphs.json`.

Each glyph is drawn on a 16-unit grid: `stroke` is a centreline stroked at 1 unit with
round caps and joins, `fill` is filled as is, `cut` is stroked the same way and taken
out of the result (the gaps between a filled hand's fingers), and `transform` (one SVG
`translate`, `rotate` or `matrix`) applies to all three. The grid maps onto a 1024-unit em with an
ascent of 960, so an icon fills the em box the way the rest of the chrome expects.

Needs fontTools and skia-pathops. On this repository's Windows machines, run it in WSL:

    python3 -m venv ~/iconfont-venv
    ~/iconfont-venv/bin/pip install fonttools skia-pathops
    ~/iconfont-venv/bin/python -I crates/ondin-app/assets/icons/build_font.py
"""

import json
import math
import pathlib
import re

import pathops
from fontTools.fontBuilder import FontBuilder
from fontTools.misc.transform import Transform
from fontTools.pens.cu2quPen import Cu2QuPen
from fontTools.pens.transformPen import TransformPen
from fontTools.pens.ttGlyphPen import TTGlyphPen
from fontTools.svgLib.path import parse_path

HERE = pathlib.Path(__file__).resolve().parent
SOURCE = HERE / "glyphs.json"
OUTPUT = HERE.parent / "fonts" / "OndinIcons.ttf"

UPM = 1024
ASCENT = 960
DESCENT = -64
UNIT = 64  # font units per grid unit
GRID = Transform(UNIT, 0, 0, -UNIT, 0, ASCENT)


def svg_transform(text):
    if not text:
        return Transform()
    kind, args = re.fullmatch(r"(\w+)\(([^)]*)\)", text.strip()).groups()
    args = [float(a) for a in args.replace(",", " ").split()]
    if kind == "translate":
        return Transform().translate(*args)
    if kind == "matrix":
        return Transform(*args)
    if kind == "rotate":
        angle, cx, cy = args
        return Transform().translate(cx, cy).rotate(math.radians(angle)).translate(-cx, -cy)
    raise ValueError(f"unsupported transform {text!r}")


def outline(d, transform):
    path = pathops.Path()
    if d:
        parse_path(d, TransformPen(path.getPen(), GRID.transform(transform)))
    return path


def glyph(entry):
    transform = svg_transform(entry.get("transform"))
    stroked = outline(entry.get("stroke"), transform)
    stroked.stroke(UNIT, pathops.LineCap.ROUND_CAP, pathops.LineJoin.ROUND_JOIN, 4)
    stroked.convertConicsToQuads(0.25)
    shape = pathops.op(stroked, outline(entry.get("fill"), transform), pathops.PathOp.UNION)
    if entry.get("cut"):
        cut = outline(entry["cut"], transform)
        cut.stroke(UNIT, pathops.LineCap.ROUND_CAP, pathops.LineJoin.ROUND_JOIN, 4)
        cut.convertConicsToQuads(0.25)
        shape = pathops.op(shape, cut, pathops.PathOp.DIFFERENCE)
    shape = pathops.simplify(shape, clockwise=True)
    pen = TTGlyphPen(None)
    shape.draw(Cu2QuPen(pen, max_err=0.5, reverse_direction=False))
    return pen.glyph()


def source_stamp(text):
    """FNV-1a 64 of the source with `\\r` dropped, so a CRLF checkout stamps alike.
    `theme::tests::the_font_was_built_from_the_current_glyphs` recomputes it."""
    h = 0xCBF29CE484222325
    for b in text.replace("\r", "").encode("utf-8"):
        h = ((h ^ b) * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return f"glyphs {h:016x}"


def main():
    text = SOURCE.read_text(encoding="utf-8")
    entries = json.loads(text)
    order, cmap, glyphs = [".notdef"], {}, {".notdef": TTGlyphPen(None).glyph()}
    for entry in entries:
        name = entry["name"]
        order.append(name)
        cmap[int(entry["codepoint"][2:], 16)] = name
        glyphs[name] = glyph(entry)

    fb = FontBuilder(UPM, isTTF=True)
    fb.setupGlyphOrder(order)
    fb.setupCharacterMap(cmap)
    fb.setupGlyf(glyphs)
    # The left side bearing has to be the glyph's own left edge: rasterizers place
    # the outline from it, so a 0 here shifts every icon to the left of its box.
    glyf = fb.font["glyf"]
    metrics = {}
    for name in order:
        g = glyf[name]
        g.recalcBounds(glyf)
        metrics[name] = (UPM, getattr(g, "xMin", 0))
    fb.setupHorizontalMetrics(metrics)
    fb.setupHorizontalHeader(ascent=ASCENT, descent=DESCENT)
    fb.setupOS2(
        sTypoAscender=ASCENT,
        sTypoDescender=DESCENT,
        sTypoLineGap=0,
        usWinAscent=ASCENT,
        usWinDescent=-DESCENT,
    )
    # A face with no PostScript name is skipped by fontdb, which the font picker scans with.
    fb.setupNameTable(
        {
            "familyName": "Ondin Icons",
            "styleName": "Regular",
            "fullName": "Ondin Icons Regular",
            "psName": "OndinIcons-Regular",
            "version": source_stamp(text),
        }
    )
    fb.setupPost()
    fb.save(OUTPUT)
    print(f"{len(entries)} glyphs -> {OUTPUT}")


if __name__ == "__main__":
    main()
