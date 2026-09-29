//! 美咲フォント (8×8 ドット日本語ビットマップフォント、Copyright (C) 2002-2021 Num Kadoma)
//!
//! `tools/misaki2bin.py` が BDF (misaki_gothic.bdf、2021-05-05 版) から作った 3 つのテーブルを
//! `include_bytes!` でフラッシュに置く (計 78,881 B、7,171 グリフ):
//!
//! - `codes.bin`  … Unicode スカラー値 (u16 LE、昇順) → ここを二分探索してグリフ番号を得る
//! - `glyphs.bin` … グリフ番号 × 8 バイト (行 0 が上、bit7 が左)
//! - `widths.bin` … 送り幅 (半角 4 / 全角 8)
//!
//! ライセンスは `fonts/misaki/misaki.txt` (改変・商用を問わず利用・複製・再配布自由、無保証)。
//!
//! 描画は [`draw_text`] で `BackBuffer` (RGB666 ワード) に直接書く。`scale` = 2 で 16×16 (半角 8×16)。
//! 収録外の文字は `□` (U+25A1) があればそれ、無ければ全角の空白幅で送る。
//! テーブル検索 ([`glyph`]) は `core` だけに依存し、ホストのテスト (`tools/ticker-tests`) でも動く。

/// 1 グリフのビットマップ (8 行、bit7 が左端) と送り幅
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Glyph {
    pub rows: [u8; 8],
    /// 送り幅 (px、等倍)。半角 4、全角 8
    pub advance: u8,
}

/// フォントの高さ (等倍)
pub const HEIGHT: u32 = 8;
/// 全角の送り幅 (等倍)
pub const FULL_WIDTH: u8 = 8;

static CODES: &[u8] = include_bytes!("../../fonts/misaki/codes.bin");
static GLYPHS: &[u8] = include_bytes!("../../fonts/misaki/glyphs.bin");
static WIDTHS: &[u8] = include_bytes!("../../fonts/misaki/widths.bin");

/// 収録グリフ数
pub fn glyph_count() -> usize {
    WIDTHS.len()
}

#[inline]
fn code_at(index: usize) -> u16 {
    u16::from_le_bytes([CODES[index * 2], CODES[index * 2 + 1]])
}

/// Unicode スカラー値からグリフ番号を引く (二分探索)
pub fn index_of(ch: char) -> Option<usize> {
    let code = u32::from(ch);
    if code > u32::from(u16::MAX) {
        return None;
    }
    let code = code as u16;
    let mut lo = 0usize;
    let mut hi = glyph_count();
    while lo < hi {
        let mid = (lo + hi) / 2;
        let at = code_at(mid);
        if at == code {
            return Some(mid);
        } else if at < code {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    None
}

/// グリフ番号 → ビットマップ
pub fn glyph_at(index: usize) -> Glyph {
    let mut rows = [0u8; 8];
    rows.copy_from_slice(&GLYPHS[index * 8..index * 8 + 8]);
    Glyph {
        rows,
        advance: WIDTHS[index],
    }
}

/// 文字 → グリフ (収録外なら None)
pub fn glyph(ch: char) -> Option<Glyph> {
    index_of(ch).map(glyph_at)
}

/// 収録外の文字の代わりに描くグリフ (`□`、それも無ければ空白の全角幅)
pub fn fallback_glyph() -> Glyph {
    glyph('\u{25a1}').unwrap_or(Glyph {
        rows: [0; 8],
        advance: FULL_WIDTH,
    })
}

/// 文字の送り幅 (等倍)。収録外は代替グリフの幅
pub fn advance_of(ch: char) -> u8 {
    glyph(ch).unwrap_or_else(fallback_glyph).advance
}

/// 文字列の描画幅 (px)
pub fn text_width(text: &str, scale: u32) -> u32 {
    text.chars().map(|c| u32::from(advance_of(c)) * scale).sum()
}

/// 1 グリフを `(x, y)` に `scale` 倍で描く。画面外の部分は捨てる。戻り値は送り幅 (px)。
///
/// `put` は `(x, y)` のピクセルを塗る閉包 (バックバッファへの書き込み)。フォント層を LCD の型に
/// 依存させないため、描画先は閉包で受ける。
pub fn draw_glyph(glyph: &Glyph, x: i32, y: i32, scale: u32, mut put: impl FnMut(i32, i32)) -> i32 {
    let scale_i = scale as i32;
    for (row, bits) in glyph.rows.iter().enumerate() {
        if *bits == 0 {
            continue;
        }
        for col in 0..8i32 {
            if bits & (0x80 >> col) == 0 {
                continue;
            }
            let px = x + col * scale_i;
            let py = y + row as i32 * scale_i;
            for dy in 0..scale_i {
                for dx in 0..scale_i {
                    put(px + dx, py + dy);
                }
            }
        }
    }
    i32::from(glyph.advance) * scale_i
}

/// 文字列を `(x, y)` から `scale` 倍で描き、次の x を返す。`clip_right` 以上には描かない
/// (スクロール表示の右端。左端は `put` 側で x<0 を捨てる)。
pub fn draw_text(text: &str, x: i32, y: i32, scale: u32, clip_right: i32, mut put: impl FnMut(i32, i32)) -> i32 {
    let mut cursor = x;
    let glyph_w = 8 * scale as i32;
    for ch in text.chars() {
        let glyph = glyph(ch).unwrap_or_else(fallback_glyph);
        let advance = i32::from(glyph.advance) * scale as i32;
        if cursor >= clip_right {
            break;
        }
        if cursor + glyph_w > 0 {
            draw_glyph(&glyph, cursor, y, scale, |px, py| {
                if px < clip_right {
                    put(px, py)
                }
            });
        }
        cursor += advance;
    }
    cursor
}
