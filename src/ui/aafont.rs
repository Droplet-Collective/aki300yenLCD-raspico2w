//! 4 bit アンチエイリアスのビットマップフォント (時計 / 気温の数字)
//!
//! データは `aafont_data.rs` (`tools/ui-sim/gen_aafont.py` が DejaVu Sans から生成)。
//! 各グリフはフォント共通の高さ `height` 行 × `width` 列の 4 bit アルファ (1 バイト 2 画素、上位が左)。
//! 描画は [`crate::ui::canvas::Canvas::aa_text`] (写真の上でも縁が滑らかになるよう背景と混ぜる)。

/// 1 グリフ
pub struct AaGlyph {
    pub ch: char,
    /// 送り幅 (px)
    pub advance: u8,
    /// ペン位置からビットマップ左端までのずれ (px)
    pub x_offset: i8,
    /// ビットマップの幅 (px)
    pub width: u8,
    /// `height` 行 × `(width + 1) / 2` バイト
    pub alpha: &'static [u8],
}

/// フォント (字種は数字と少しの記号だけ)
pub struct AaFont {
    /// ビットマップの行数 (字種全体のインクの上端〜下端)
    pub height: u8,
    /// 上端からベースラインまでの行数
    pub baseline: u8,
    pub glyphs: &'static [AaGlyph],
}

impl AaGlyph {
    /// (col, row) のアルファ (0..=15)
    #[inline]
    pub fn alpha_at(&self, col: usize, row: usize) -> u8 {
        let stride = (self.width as usize).div_ceil(2);
        let byte = self.alpha[row * stride + col / 2];
        if col & 1 == 0 { byte >> 4 } else { byte & 0x0f }
    }
}

impl AaFont {
    pub fn glyph(&self, ch: char) -> Option<&AaGlyph> {
        self.glyphs.iter().find(|g| g.ch == ch)
    }

    /// 文字の送り幅 (収録外の文字は空白として数字の半分)
    pub fn advance(&self, ch: char) -> i32 {
        match self.glyph(ch) {
            Some(g) => i32::from(g.advance),
            None => self.glyph('0').map_or(4, |g| i32::from(g.advance) / 2),
        }
    }

    /// 文字列の描画幅 (px)
    pub fn text_width(&self, text: &str) -> i32 {
        text.chars().map(|c| self.advance(c)).sum()
    }
}
