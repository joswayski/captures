"""Original geometric glyphs with known GSUB/GPOS metrics; no external font.

Regenerate with:
uv run --with fonttools python crates/captures-image/tests/make_shaping_font.py
"""

from pathlib import Path
import struct
import zlib

from fontTools.colorLib.builder import buildCOLR, buildCPAL
from fontTools.feaLib.builder import addOpenTypeFeaturesFromString
from fontTools.fontBuilder import FontBuilder
from fontTools.pens.ttGlyphPen import TTGlyphPen
from fontTools.ttLib import newTable
from fontTools.ttLib.tables.sbixGlyph import Glyph
from fontTools.ttLib.tables.sbixStrike import Strike


def glyph(points):
    pen = TTGlyphPen(None)
    if points:
        pen.moveTo(points[0])
        for point in points[1:]:
            pen.lineTo(point)
        pen.closePath()
    return pen.glyph()


for variant in ["Regular", "Bold", "Italic", "Color", "Bitmap"]:
    style = "Regular" if variant in ["Color", "Bitmap"] else variant
    font = FontBuilder(1000, isTTF=True)
    widths = {".notdef": 700, "space": 300, "L": 800 if style == "Bold" else 700,
              "f": 400, "i": 200, "fi": 450, "A": 700, "acute": 0,
              "aleph": 900, "bet": 500}
    font.setupGlyphOrder(list(widths))
    font.setupCharacterMap({32: "space", 76: "L", 102: "f", 105: "i", 65: "A",
                            0x301: "acute", 0x5D0: "aleph", 0x5D1: "bet"})
    shapes = {
        ".notdef": [], "space": [],
        "L": [(0, 0), (600, 0), (600, 150), (150, 150),
              (150, 800), (-100 if style == "Italic" else 0, 800)],
        "f": [(0, 0), (100, 0), (100, 700), (0, 700)],
        "i": [(0, 0), (100, 0), (100, 400), (0, 400)],
        "fi": [(0, 0), (450, 0), (450, 200), (0, 700)],
        "A": [(0, 0), (600, 0), (300, 700)],
        "acute": [(0, 0), (200, 100), (200, 200), (0, 100)],
        "aleph": [(0, 0), (200, 0), (200, 700), (0, 700)],
        "bet": [(0, 0), (450, 0), (450, 500), (0, 500)],
    }
    font.setupGlyf({name: glyph(points) for name, points in shapes.items()})
    font.setupHorizontalMetrics({name: (width, -100 if name == "L" and style == "Italic" else 0)
                                for name, width in widths.items()})
    font.setupHorizontalHeader(ascent=1000, descent=-200)
    font.setupNameTable({"familyName": "Captures Shaping Test", "styleName": style,
                        "psName": "CapturesShapingTest-" + style})
    font.setupOS2(sTypoAscender=1000, sTypoDescender=-200,
                  usWinAscent=1000, usWinDescent=200,
                  usWeightClass=700 if style == "Bold" else 400,
                  fsSelection={"Regular": 64, "Bold": 32, "Italic": 1}[style])
    font.setupPost(italicAngle=-12 if style == "Italic" else 0)
    font.setupMaxp()
    font.font["head"].macStyle = {"Regular": 0, "Bold": 1, "Italic": 2}[style]
    addOpenTypeFeaturesFromString(font.font, """
        languagesystem DFLT dflt;
        languagesystem latn dflt;
        languagesystem hebr dflt;
        feature liga { sub f i by fi; } liga;
        feature kern { pos A A -600; } kern;
        markClass acute <anchor 0 0> @TOP;
        feature mark { pos base A <anchor 200 800> mark @TOP; } mark;
    """)
    if variant == "Color":
        font.font["COLR"] = buildCOLR({"A": [("f", 0), ("i", 1)]})
        font.font["CPAL"] = buildCPAL([[(1., 0., 0., .5), (0., 1., 0., 1.)]])
    if variant == "Bitmap":
        def chunk(kind, data):
            return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

        # A 2×2 straight-alpha PNG. Muted channels catch erroneous unpremultiplication.
        png = (b"\x89PNG\r\n\x1a\n"
               + chunk(b"IHDR", struct.pack(">IIBBBBB", 2, 2, 8, 6, 0, 0, 0))
               + chunk(b"IDAT", zlib.compress((b"\x00" + bytes([90, 140, 190, 128]) * 2) * 2))
               + chunk(b"IEND", b""))
        strike = Strike(ppem=100, resolution=72)
        strike.glyphs["A"] = Glyph(glyphName="A", graphicType="png ", imageData=png)
        font.font["sbix"] = newTable("sbix")
        font.font["sbix"].strikes = {100: strike}
    font.font["head"].created = font.font["head"].modified = 2082844800
    font.font.recalcTimestamp = False
    font.save(Path(__file__).with_name(f"shaping-{variant.lower()}.ttf"))
