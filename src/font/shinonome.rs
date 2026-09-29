//! 東雲フォント (Shinonome、14 ドット日本語ビットマップフォント、/efont/ プロジェクト、Public Domain)
//!
//! `tools/bdf2bin.py` が BDF (`shnmk14.bdf` JIS X 0208 全角 14×14 + `shnm7x14r.bdf` JIS X 0201 半角 7×14、
//! 東雲 0.9.11) から作った 3 つのテーブルを `include_bytes!` でフラッシュに置く (計 218,457 B、7,047 グリフ):
//!
//! - `codes.bin`  … Unicode スカラー値 (u16 LE、昇順) → ここを二分探索してグリフ番号を得る
//! - `glyphs.bin` … グリフ番号 × 28 バイト (14 行 × 2 バイト、行 0 が上、ビッグエンディアン u16 の bit15 が左)
//! - `widths.bin` … 送り幅 (半角 7 / 全角 14)
//!
//! ライセンスは `fonts/shinonome/LICENSE` (Public Domain、無保証)。
//!
//! 描画は等倍 (1 px = 1 ドット) で [`draw_text`] が `BackBuffer` (RGB666 ワード) に直接書く。
//! 16 px の帯の中に上下 1 px の余白を置いて使う (`ticker` の `JP_PAD`)。
//! 収録外の文字は `□` (U+25A1) があればそれ、無ければ全角の空白幅で送る。
//! テーブル検索 ([`glyph`]) は `core` だけに依存し、ホストのテスト (`tools/ticker-tests`) でも動く。

/// フォントの高さ (行数)
pub const HEIGHT: usize = 14;
/// 全角の送り幅 (px)
pub const FULL_WIDTH: u8 = 14;
/// 半角の送り幅 (px)
pub const HALF_WIDTH: u8 = 7;
/// 1 グリフのバイト数 (`glyphs.bin` のストライド)
const GLYPH_BYTES: usize = HEIGHT * 2;

/// 1 グリフのビットマップ (14 行、bit15 が左端。半角は上位バイトだけを使う) と送り幅
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Glyph {
    pub rows: [u16; HEIGHT],
    /// 送り幅 (px)。半角 7、全角 14
    pub advance: u8,
}

static CODES: &[u8] = include_bytes!("../../fonts/shinonome/codes.bin");
static GLYPHS: &[u8] = include_bytes!("../../fonts/shinonome/glyphs.bin");
static WIDTHS: &[u8] = include_bytes!("../../fonts/shinonome/widths.bin");

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
    let base = index * GLYPH_BYTES;
    let mut rows = [0u16; HEIGHT];
    for (row, out) in rows.iter_mut().enumerate() {
        *out = u16::from_be_bytes([GLYPHS[base + row * 2], GLYPHS[base + row * 2 + 1]]);
    }
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
        rows: [0; HEIGHT],
        advance: FULL_WIDTH,
    })
}

/// 文字の送り幅 (px)。収録外は代替グリフの幅
pub fn advance_of(ch: char) -> u8 {
    glyph(ch).unwrap_or_else(fallback_glyph).advance
}

/// 文字列の描画幅 (px)
pub fn text_width(text: &str) -> u32 {
    text.chars().map(|c| u32::from(advance_of(c))).sum()
}

/// 1 グリフを `(x, y)` に等倍で描く。戻り値は送り幅 (px)。
///
/// `put` は `(x, y)` のピクセルを塗る閉包 (バックバッファへの書き込み。画面外の切り捨ても `put` 側)。
/// フォント層を LCD の型に依存させないため、描画先は閉包で受ける。
pub fn draw_glyph(glyph: &Glyph, x: i32, y: i32, mut put: impl FnMut(i32, i32)) -> i32 {
    let width = i32::from(glyph.advance);
    for (row, bits) in glyph.rows.iter().enumerate() {
        if *bits == 0 {
            continue;
        }
        let py = y + row as i32;
        for col in 0..width {
            if bits & (0x8000 >> col) != 0 {
                put(x + col, py);
            }
        }
    }
    width
}

/// 文字列を `(x, y)` から等倍で描き、次の x を返す。`clip_right` 以上には描かない
/// (スクロール表示の右端。左端は `put` 側で x<0 を捨てる)。
pub fn draw_text(text: &str, x: i32, y: i32, clip_right: i32, mut put: impl FnMut(i32, i32)) -> i32 {
    let mut cursor = x;
    for ch in text.chars() {
        let glyph = glyph(ch).unwrap_or_else(fallback_glyph);
        let advance = i32::from(glyph.advance);
        if cursor >= clip_right {
            break;
        }
        if cursor + advance > 0 {
            draw_glyph(&glyph, cursor, y, |px, py| {
                if px < clip_right {
                    put(px, py)
                }
            });
        }
        cursor += advance;
    }
    cursor
}
