#!/usr/bin/env python3
"""BDF ビットマップフォントを、ファームウェアが include_bytes! するコンパクトなテーブルに変換する。

    tools/bdf2bin.py --height 14 --out fonts/shinonome shnmk14.bdf:jisx0208 shnm7x14r.bdf:jisx0201

引数の BDF は `ファイル:符号化` の形で複数与える (`:符号化` を省くと unicode)。

    unicode   ENCODING が Unicode スカラー値 (美咲フォントなど)
    jisx0208  ENCODING が JIS X 0208 の区点 ((区+0x20)<<8 | 点+0x20)。EUC-JP として Unicode に直す
    jisx0201  ENCODING が JIS X 0201 (0x20〜0x7E は ASCII、ただし 0x5C = ¥ U+00A5 / 0x7E = ‾ U+203E、
              0xA1〜0xDF は半角カナ U+FF61〜U+FF9F)。それ以外の符号は捨てる

出力 (すべてグリフ番号順 = Unicode 昇順)。`--height H` はフォントの高さ (行数):
    codes.bin    u16 LE × N       Unicode スカラー値 (昇順。ファームウェアは二分探索する)
    glyphs.bin   (2 × H) B × N    1 行 2 バイト (ビッグエンディアン u16、bit15 が左端) × H 行。BDF の BBX
                                  オフセットを FONT_ASCENT / FONT_DESCENT の H 行枠へ展開済み。
                                  半角グリフも同じ幅で持つ (右バイトは 0)
    widths.bin   1 B × N          送り幅 (DWIDTH。東雲 14 なら半角 7 / 全角 14)

JIS にあって Unicode の別の符号位置でよく書かれる文字は、同じグリフを複数の符号位置に載せる
(ALIASES: 〜 U+301C と ～ U+FF5E、− U+2212 と － U+FF0D、¥ U+00A5 と \\ U+005C など)。

読み手は src/font/shinonome.rs (と tools/ticker-tests の同モジュール)。
"""
import argparse
import os
import sys

# 出力先の Unicode ← 元 (BDF から得た Unicode)。元が無ければ何もしない
ALIASES = {
    0x005C: 0x00A5,  # \  ← ¥ (JIS X 0201 の 0x5C)
    0x007E: 0x301C,  # ~  ← 〜 (全角の波ダッシュで描く。JIS X 0201 の 0x7E は ‾)
    0xFF5E: 0x301C,  # ～ ← 〜 (Windows 系の入力は U+FF5E になる)
    0xFF0D: 0x2212,  # － ← − (同上)
    0x2225: 0x2016,  # ∥ ← ‖
    0x2014: 0x2015,  # — ← ―
    0xFFE0: 0x00A2,  # ￠ ← ¢
    0xFFE1: 0x00A3,  # ￡ ← £
    0xFFE2: 0x00AC,  # ￢ ← ¬
    0x00A0: 0x0020,  # NBSP ← 空白
}


def to_unicode(code, encoding):
    """BDF の ENCODING を Unicode スカラー値にする (収録しないものは None)"""
    if encoding == "unicode":
        return code if 0 <= code <= 0xFFFF else None
    if encoding == "jisx0208":
        hi, lo = code >> 8, code & 0xFF
        if not (0x21 <= hi <= 0x7E and 0x21 <= lo <= 0x7E):
            return None
        try:
            return ord(bytes([hi | 0x80, lo | 0x80]).decode("euc_jp"))
        except (UnicodeDecodeError, TypeError):
            return None
    if encoding == "jisx0201":
        if code == 0x5C:
            return 0x00A5
        if code == 0x7E:
            return 0x203E
        if 0x20 <= code <= 0x7E:
            return code
        if 0xA1 <= code <= 0xDF:
            return 0xFF61 + (code - 0xA1)
        return None
    raise ValueError(encoding)


def parse_bdf(path, encoding, height):
    """BDF を読み、{Unicode: (送り幅, [行 u16 × height])} を返す"""
    glyphs = {}
    ascent = None
    with open(path, encoding="latin-1") as f:
        code = None
        dwidth = None
        bbx = None
        in_bitmap = False
        rows = []
        for raw in f:
            line = raw.strip()
            if line.startswith("FONT_ASCENT"):
                ascent = int(line.split()[1])
            elif line.startswith("STARTCHAR"):
                code, dwidth, bbx, rows, in_bitmap = None, None, None, [], False
            elif line.startswith("ENCODING"):
                code = int(line.split()[1])
            elif line.startswith("DWIDTH"):
                dwidth = int(line.split()[1])
            elif line.startswith("BBX"):
                bbx = tuple(int(v) for v in line.split()[1:5])
            elif line == "BITMAP":
                in_bitmap = True
            elif line == "ENDCHAR":
                in_bitmap = False
                if code is None or dwidth is None or bbx is None:
                    continue
                uni = to_unicode(code, encoding)
                if uni is None or uni in glyphs:
                    continue
                if ascent is None:
                    sys.exit(f"{path}: FONT_ASCENT がありません")
                glyphs[uni] = (dwidth, expand(bbx, rows, ascent, height))
            elif in_bitmap:
                rows.append(line)
    return glyphs


def expand(bbx, rows, ascent, height):
    """BBX (w h xoff yoff) と行データを、幅 16 bit (bit15 が左端) × height 行に展開する"""
    w, h, xo, yo = bbx
    top = ascent - yo - h
    nbits = ((w + 7) // 8) * 8
    out = [0] * height
    for i, hexrow in enumerate(rows[:h]):
        value = int(hexrow, 16) if hexrow else 0
        bits = value << (16 - nbits) if nbits <= 16 else value >> (nbits - 16)
        bits = bits >> xo if xo >= 0 else bits << -xo
        y = top + i
        if 0 <= y < height:
            out[y] = bits & 0xFFFF
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--height", type=int, required=True, help="フォントの高さ (行数)")
    ap.add_argument("--out", required=True, help="出力ディレクトリ")
    ap.add_argument("bdf", nargs="+", help="BDF ファイル (`path:encoding`、encoding は unicode / jisx0208 / jisx0201)")
    args = ap.parse_args()

    glyphs = {}
    for spec in args.bdf:
        path, _, encoding = spec.partition(":")
        encoding = encoding or "unicode"
        got = parse_bdf(path, encoding, args.height)
        added = 0
        for uni, g in got.items():
            if uni not in glyphs:
                glyphs[uni] = g
                added += 1
        print(f"{path} ({encoding}): {len(got)} glyphs, {added} new")
    for dst, src in ALIASES.items():
        if dst not in glyphs and src in glyphs:
            glyphs[dst] = glyphs[src]

    codes = sorted(glyphs)
    os.makedirs(args.out, exist_ok=True)
    with open(os.path.join(args.out, "codes.bin"), "wb") as f:
        for c in codes:
            f.write(c.to_bytes(2, "little"))
    with open(os.path.join(args.out, "glyphs.bin"), "wb") as f:
        for c in codes:
            for row in glyphs[c][1]:
                f.write(row.to_bytes(2, "big"))
    with open(os.path.join(args.out, "widths.bin"), "wb") as f:
        f.write(bytes(glyphs[c][0] for c in codes))
    widths = sorted({glyphs[c][0] for c in codes})
    total = len(codes) * (2 + 2 * args.height + 1)
    print(f"{len(codes)} glyphs, widths {widths}, codes U+{codes[0]:04X}..U+{codes[-1]:04X}, "
          f"{total} bytes total -> {args.out}")


if __name__ == "__main__":
    main()
