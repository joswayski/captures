"""Generate the original, deliberately asymmetric L glyph used by raster tests.

Regenerate with: uv run --with fonttools python crates/captures-image/tests/make_test_font.py
No system font, downloaded artwork, or production font fallback is involved.
"""

from pathlib import Path

from fontTools.fontBuilder import FontBuilder
from fontTools.pens.ttGlyphPen import TTGlyphPen

font = FontBuilder(1000, isTTF=True)
font.setupGlyphOrder([".notdef", "space", "L"])
font.setupCharacterMap({32: "space", 76: "L"})
pen = TTGlyphPen(None)
empty = pen.glyph()
pen.moveTo((0, 0))
for position in [(600, 0), (600, 150), (150, 150), (150, 800), (0, 800)]:
    pen.lineTo(position)
pen.closePath()
font.setupGlyf({".notdef": empty, "space": empty, "L": pen.glyph()})
font.setupHorizontalMetrics({".notdef": (700, 0), "space": (300, 0), "L": (700, 0)})
font.setupHorizontalHeader(ascent=800, descent=-200)
font.setupNameTable({"familyName": "Captures Raster Test", "styleName": "Regular"})
font.setupOS2(sTypoAscender=800, sTypoDescender=-200, usWinAscent=800, usWinDescent=200)
font.setupPost()
font.setupMaxp()
font.font["head"].created = font.font["head"].modified = 2082844800
font.font.recalcTimestamp = False
font.save(Path(__file__).with_name("test-font.ttf"))
