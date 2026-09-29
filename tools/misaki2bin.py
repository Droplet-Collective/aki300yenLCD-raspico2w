#!/usr/bin/env python3
"""美咲フォント (8×8 ビットマップ、Num Kadoma 作) の BDF を、ファームウェアが include_bytes! する
コンパクトなテーブルに変換する。

    tools/misaki2bin.py [misaki_gothic.bdf] [出力ディレクトリ]

    既定: BDF は https://littlelimit.net/arc/misaki/misaki_bdf_2021-05-05.zip をカレントに展開したもの、
    出力は fonts/misaki/。

出力 (すべてグリフ番号順 = Unicode 昇順):
    codes.bin    u16 LE × N   Unicode スカラー値 (昇順。ファームウェアは二分探索する)
    glyphs.bin   8 B × N      8×8 ビットマップ (行 0 が上、bit7 が左)。BDF の BBX オフセットを
                              FONT_ASCENT=6 / FONT_DESCENT=2 の 8 行枠へ展開済み
    widths.bin   1 B × N      送り幅 (DWIDTH)。半角 = 4、全角 = 8

src/font/misaki.rs がこの 3 ファイルを読む。ライセンスは fonts/misaki/misaki.txt (原文) を参照。
"""
import os
import sys

ASCENT = 6
HEIGHT = 8


def parse_bdf(path):
    glyphs = {}
    with open(path, encoding="utf-8", errors="replace") as f:
        code = None
        dwidth = 8
        bbx = (8, 8, 0, -2)
        in_bitmap = False
        rows = []
        for raw in f:
            line = raw.strip()
            if line.startswith("STARTCHAR"):
                code, dwidth, rows, in_bitmap = None, 8, [], False
            elif line.startswith("ENCODING"):
                code = int(line.split()[1])
            elif line.startswith("DWIDTH"):
                dwidth = int(line.split()[1])
            elif line.startswith("BBX"):
                w, h, xo, yo = (int(v) for v in line.split()[1:5])
                bbx = (w, h, xo, yo)
            elif line == "BITMAP":
                in_bitmap = True
            elif line == "ENDCHAR":
                in_bitmap = False
                if code is None or code < 0 or code > 0xFFFF:
                    continue
                glyphs[code] = (dwidth, expand(bbx, rows))
            elif in_bitmap:
                rows.append(line)
    return glyphs


def expand(bbx, rows):
    """BBX (w h xoff yoff) と行データを 8×8 (8 バイト、上から) に展開する"""
    w, h, xo, yo = bbx
    top = ASCENT - yo - h
    out = [0] * HEIGHT
    nbytes = (w + 7) // 8
    for i, hexrow in enumerate(rows[:h]):
        value = int(hexrow, 16) if hexrow else 0
        bits = value >> (nbytes * 8 - w) if nbytes * 8 > w else value
        y = top + i
        if 0 <= y < HEIGHT:
            shifted = (bits << (8 - w - xo)) if (8 - w - xo) >= 0 else (bits >> -(8 - w - xo))
            out[y] = shifted & 0xFF
    return bytes(out)


def main():
    bdf = sys.argv[1] if len(sys.argv) > 1 else "misaki_gothic.bdf"
    outdir = sys.argv[2] if len(sys.argv) > 2 else os.path.join("fonts", "misaki")
    glyphs = parse_bdf(bdf)
    codes = sorted(glyphs)
    os.makedirs(outdir, exist_ok=True)
    with open(os.path.join(outdir, "codes.bin"), "wb") as f:
        for c in codes:
            f.write(c.to_bytes(2, "little"))
    with open(os.path.join(outdir, "glyphs.bin"), "wb") as f:
        for c in codes:
            f.write(glyphs[c][1])
    with open(os.path.join(outdir, "widths.bin"), "wb") as f:
        f.write(bytes(4 if glyphs[c][0] <= 4 else 8 for c in codes))
    half = sum(1 for c in codes if glyphs[c][0] <= 4)
    print(f"{len(codes)} glyphs ({half} half-width), codes {codes[0]}..{codes[-1]}, "
          f"{len(codes) * 11} bytes total -> {outdir}")


if __name__ == "__main__":
    main()
