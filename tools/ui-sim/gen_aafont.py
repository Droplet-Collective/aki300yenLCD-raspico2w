"""DejaVu Sans から時計 / 気温用の 4 bit アンチエイリアス数字フォントを作り、src/ui/aafont_data.rs に書く。

    python3 tools/ui-sim/gen_aafont.py [DejaVuSans.ttf のパス]

既定のフォントは Debian / Ubuntu の fonts-dejavu-core (/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf)。
ライセンスは fonts/dejavu/LICENSE (Bitstream Vera + DejaVu の変更はパブリックドメイン)。
出力はグリフごとに「送り幅 / 左端のずれ / 幅 / 4 bit のアルファ (1 バイトに 2 画素、上位 4 bit が左)」。
高さは字種全体で揃え (上端 = 字種で一番高いインクの行、下端 = 一番低い行)、ベースラインからの位置も保つ。
"""
import sys
from pathlib import Path
from PIL import Image, ImageDraw, ImageFont

REPO = Path(__file__).resolve().parents[2]
FONT = sys.argv[1] if len(sys.argv) > 1 else "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"

# (Rust の名前, 字の大きさ (px), 字種, 送り幅の追加 (px), 数字を等幅にするか)
FONTS = [
    ("CLOCK", 42, "0123456789:-", 0, True),
    ("MEDIUM", 20, "0123456789:.-°%", 0, True),
    ("SMALL", 13, "0123456789:.-°%/", 0, True),
]


def render(font, ch):
    """文字 ch を大きめのキャンバスに描き、(アルファ画像, 原点 x, ベースライン y) を返す"""
    size = font.size * 3
    img = Image.new("L", (size, size), 0)
    d = ImageDraw.Draw(img)
    ox, base = font.size, font.size * 2
    d.text((ox, base), ch, font=font, fill=255, anchor="ls")
    return img, ox, base


def build(name, px, chars, extra, tabular):
    font = ImageFont.truetype(FONT, px)
    glyphs = []
    top, bottom = 10**9, -1
    for ch in chars:
        img, ox, base = render(font, ch)
        bbox = img.getbbox()
        adv = round(font.getlength(ch)) + extra
        glyphs.append((ch, img, ox, base, bbox, adv))
        if bbox:
            top = min(top, bbox[1] - base)
            bottom = max(bottom, bbox[3] - base)
    height = bottom - top
    if tabular:
        dig = max(g[5] for g in glyphs if g[0].isdigit())
        glyphs = [(c, i, o, b, bb, dig if c.isdigit() else a) for (c, i, o, b, bb, a) in glyphs]
    out = []
    for ch, img, ox, base, bbox, adv in glyphs:
        if bbox is None:
            x0, x1 = ox, ox
        else:
            x0, x1 = bbox[0], bbox[2]
        w = x1 - x0
        stride = (w + 1) // 2
        data = bytearray()
        for row in range(height):
            y = base + top + row
            for bx in range(stride):
                hi = img.getpixel((x0 + bx * 2, y)) if bx * 2 < w else 0
                lo = img.getpixel((x0 + bx * 2 + 1, y)) if bx * 2 + 1 < w else 0
                data.append(((hi * 15 + 127) // 255) << 4 | ((lo * 15 + 127) // 255))
        out.append((ch, adv, x0 - ox, w, bytes(data)))
    return height, -top, out


def rust_char(ch):
    return "'\\''" if ch == "'" else f"'{ch}'"


lines = [
    "//! 自動生成 (tools/ui-sim/gen_aafont.py)。手で編集しない。",
    "//!",
    "//! DejaVu Sans (Bitstream Vera 由来、fonts/dejavu/LICENSE) の数字と記号を 4 bit アンチエイリアスで",
    "//! ラスタライズしたもの。",
    "",
    "use super::aafont::{AaFont, AaGlyph};",
    "",
]
for name, px, chars, extra, tabular in FONTS:
    height, baseline, glyphs = build(name, px, chars, extra, tabular)
    lines.append(f"/// DejaVu Sans {px} px ({chars})、高さ {height} px")
    lines.append(f"pub static {name}: AaFont = AaFont {{")
    lines.append(f"    height: {height},")
    lines.append(f"    baseline: {baseline},")
    lines.append("    glyphs: &[")
    for ch, adv, xoff, w, data in glyphs:
        hexes = ", ".join(f"0x{b:02x}" for b in data)
        lines.append(f"        AaGlyph {{ ch: {rust_char(ch)}, advance: {adv}, x_offset: {xoff}, width: {w}, alpha: &[{hexes}] }},")
    lines.append("    ],")
    lines.append("};")
    lines.append("")
    print(f"{name}: {px}px height {height} baseline {baseline} glyphs {len(glyphs)} bytes {sum(len(g[4]) for g in glyphs)}")

(REPO / "src/ui/aafont_data.rs").write_text("\n".join(lines))
