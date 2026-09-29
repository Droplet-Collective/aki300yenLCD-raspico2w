//! 400×96 の RGB565 画面への描画 (塗り / 半透明 / 角丸のガラス板 / 文字 3 種 / アイコン)
//!
//! ファームウェアは `lcd::display::BackBuffer` の画素配列を、シミュレータは `Vec<u16>` を渡す。
//! どちらも同じこのコードで描くので、シミュレータの PNG は実機の画面と画素単位で一致する。

use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::mono_font::ascii::{FONT_5X7, FONT_6X10};
use embedded_graphics::pixelcolor::{Rgb565, raw::RawU16};
use embedded_graphics::prelude::*;
use embedded_graphics::text::{Baseline, Text};

use super::aafont::AaFont;
use super::color::{self, Color};
use super::{HEIGHT, WIDTH};
use crate::font::shinonome;

/// 描画を許す矩形 (x0..x1, y0..y1、右と下は含まない)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Clip {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl Clip {
    pub const FULL: Clip = Clip {
        x0: 0,
        y0: 0,
        x1: WIDTH as i32,
        y1: HEIGHT as i32,
    };

    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self {
            x0: x,
            y0: y,
            x1: x + w,
            y1: y + h,
        }
    }

    #[inline]
    fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x0 && x < self.x1 && y >= self.y0 && y < self.y1
    }
}

/// ガラス板 (半透明の暗い地 + 上辺の明るい 1 px + 角丸)
#[derive(Clone, Copy, Debug)]
pub struct Panel {
    /// 地の色 ([`color::palette::GLASS`] など)
    pub tint: Color,
    /// 地の不透明度 (0..=32)
    pub alpha: u8,
    /// 上辺 1 px のハイライトの不透明度 (白、0 で無し)
    pub edge: u8,
    /// 角の半径 (0..=6)
    pub radius: u8,
}

/// 角丸の被覆率 (半径 r、角の r×r 画素、0..=16)。4×4 の副標本で円の内側を数える
const fn corner_coverage(r: usize, i: usize, j: usize) -> u8 {
    // 角の画素 (i, j) (i: 角からの列、j: 角からの行)。円の中心は (r, r)
    let mut count = 0;
    let mut sy = 0;
    while sy < 4 {
        let mut sx = 0;
        while sx < 4 {
            // 副標本の位置 ×8 (整数で): (i + (sx + 0.5) / 4) * 8
            let px = (i * 8 + sx * 2 + 1) as i32;
            let py = (j * 8 + sy * 2 + 1) as i32;
            let c = (r * 8) as i32;
            let dx = c - px;
            let dy = c - py;
            if dx * dx + dy * dy <= c * c {
                count += 1;
            }
            sx += 1;
        }
        sy += 1;
    }
    count
}

/// 画面の 1 枚分への描画口
pub struct Canvas<'a> {
    px: &'a mut [Color],
    clip: Clip,
}

impl<'a> Canvas<'a> {
    /// `px` は 400×96 の行優先 (`[y * 400 + x]`)
    pub fn new(px: &'a mut [Color]) -> Self {
        assert!(px.len() >= WIDTH * HEIGHT);
        Self { px, clip: Clip::FULL }
    }

    pub fn pixels(&self) -> &[Color] {
        self.px
    }

    pub fn pixels_mut(&mut self) -> &mut [Color] {
        self.px
    }

    /// 以後の描画を `clip` (画面と交わる部分) に限る
    pub fn set_clip(&mut self, clip: Clip) {
        self.clip = Clip {
            x0: clip.x0.max(0),
            y0: clip.y0.max(0),
            x1: clip.x1.min(WIDTH as i32),
            y1: clip.y1.min(HEIGHT as i32),
        };
    }

    pub fn reset_clip(&mut self) {
        self.clip = Clip::FULL;
    }

    #[inline]
    pub fn put(&mut self, x: i32, y: i32, c: Color) {
        if self.clip.contains(x, y) {
            self.px[y as usize * WIDTH + x as usize] = c;
        }
    }

    #[inline]
    pub fn get(&self, x: i32, y: i32) -> Color {
        if (0..WIDTH as i32).contains(&x) && (0..HEIGHT as i32).contains(&y) {
            self.px[y as usize * WIDTH + x as usize]
        } else {
            0
        }
    }

    /// (x, y) に `c` を不透明度 `alpha` (0..=32) で重ねる
    #[inline]
    pub fn blend_px(&mut self, x: i32, y: i32, c: Color, alpha: u8) {
        if alpha == 0 || !self.clip.contains(x, y) {
            return;
        }
        let i = y as usize * WIDTH + x as usize;
        self.px[i] = if alpha >= 32 { c } else { color::blend(self.px[i], c, alpha) };
    }

    /// 矩形を切り抜き範囲で切って返す (空なら None)
    fn clip_rect(&self, x: i32, y: i32, w: i32, h: i32) -> Option<(usize, usize, usize, usize)> {
        let x0 = x.max(self.clip.x0);
        let y0 = y.max(self.clip.y0);
        let x1 = (x + w).min(self.clip.x1);
        let y1 = (y + h).min(self.clip.y1);
        if x0 >= x1 || y0 >= y1 {
            None
        } else {
            Some((x0 as usize, y0 as usize, x1 as usize, y1 as usize))
        }
    }

    pub fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32, c: Color) {
        if let Some((x0, y0, x1, y1)) = self.clip_rect(x, y, w, h) {
            for row in y0..y1 {
                self.px[row * WIDTH + x0..row * WIDTH + x1].fill(c);
            }
        }
    }

    /// 矩形に `c` を不透明度 `alpha` (0..=32) で重ねる
    pub fn blend_rect(&mut self, x: i32, y: i32, w: i32, h: i32, c: Color, alpha: u8) {
        if alpha >= 32 {
            self.fill_rect(x, y, w, h, c);
            return;
        }
        if alpha == 0 {
            return;
        }
        if let Some((x0, y0, x1, y1)) = self.clip_rect(x, y, w, h) {
            for row in y0..y1 {
                for p in &mut self.px[row * WIDTH + x0..row * WIDTH + x1] {
                    *p = color::blend(*p, c, alpha);
                }
            }
        }
    }

    /// ガラス板。角は半径 `radius` の 1/4 円で、縁の画素は被覆率に応じて薄くする
    pub fn panel(&mut self, x: i32, y: i32, w: i32, h: i32, style: &Panel) {
        let r = (style.radius as i32).min(6).min(w / 2).min(h / 2).max(0);
        for row in 0..h {
            // この行の角の部分 (左右 r 画素) を除いた中央
            let j = if row < r {
                Some(row)
            } else if row >= h - r {
                Some(h - 1 - row)
            } else {
                None
            };
            let py = y + row;
            match j {
                None => self.blend_rect(x, py, w, 1, style.tint, style.alpha),
                Some(j) => {
                    self.blend_rect(x + r, py, w - 2 * r, 1, style.tint, style.alpha);
                    for i in 0..r {
                        let cov = corner_coverage(r as usize, i as usize, j as usize);
                        let a = (style.alpha as u32 * cov as u32 / 16) as u8;
                        self.blend_px(x + i, py, style.tint, a);
                        self.blend_px(x + w - 1 - i, py, style.tint, a);
                    }
                }
            }
        }
        if style.edge > 0 {
            // 上辺のハイライト (角の丸みの内側だけ。両端は弱く)
            let inset = r.max(1);
            self.blend_rect(x + inset, y, w - 2 * inset, 1, color::palette::WHITE, style.edge);
            if r >= 2 {
                self.blend_px(x + inset - 1, y + 1, color::palette::WHITE, style.edge / 2);
                self.blend_px(x + w - inset, y + 1, color::palette::WHITE, style.edge / 2);
            }
        }
    }

    // ------------------------------------------------------------
    // 文字
    // ------------------------------------------------------------

    /// 東雲 14 px (等倍) を上端 `y` に描き、次の x を返す
    pub fn jp(&mut self, text: &str, x: i32, y: i32, c: Color) -> i32 {
        let clip_right = self.clip.x1;
        shinonome::draw_text(text, x, y, clip_right, |px, py| self.put(px, py, c))
    }

    /// 東雲 14 px + 右下 1 px の影 (写真の上に直接置く文字用)
    pub fn jp_shadow(&mut self, text: &str, x: i32, y: i32, c: Color, shadow_alpha: u8) -> i32 {
        let clip_right = self.clip.x1;
        shinonome::draw_text(text, x + 1, y + 1, clip_right, |px, py| {
            self.blend_px(px, py, color::palette::BLACK, shadow_alpha)
        });
        self.jp(text, x, y, c)
    }

    /// `FONT_6X10` (embedded-graphics) を上端 `y` に描き、次の x を返す
    pub fn small(&mut self, text: &str, x: i32, y: i32, c: Color) -> i32 {
        let style = MonoTextStyle::new(&FONT_6X10, Rgb565::from(RawU16::new(c)));
        let _ = Text::with_baseline(text, Point::new(x, y), style, Baseline::Top).draw(self);
        x + text.len() as i32 * FONT_6X10.character_size.width as i32
    }

    /// `FONT_5X7` (embedded-graphics、状態の小さな表示用) を上端 `y` に描き、次の x を返す
    pub fn tiny(&mut self, text: &str, x: i32, y: i32, c: Color) -> i32 {
        let style = MonoTextStyle::new(&FONT_5X7, Rgb565::from(RawU16::new(c)));
        let _ = Text::with_baseline(text, Point::new(x, y), style, Baseline::Top).draw(self);
        x + text.len() as i32 * FONT_5X7.character_size.width as i32
    }

    /// アンチエイリアスの数字フォントを上端 `y` に描き、次の x を返す
    pub fn aa_text(&mut self, font: &AaFont, text: &str, x: i32, y: i32, c: Color) -> i32 {
        let mut pen = x;
        for ch in text.chars() {
            if let Some(g) = font.glyph(ch) {
                let gx = pen + i32::from(g.x_offset);
                for row in 0..font.height as usize {
                    for col in 0..g.width as usize {
                        let a = g.alpha_at(col, row);
                        if a != 0 {
                            // 0..=15 → 0..=32
                            let a32 = (a as u32 * 32 + 7) / 15;
                            self.blend_px(gx + col as i32, y + row as i32, c, a32 as u8);
                        }
                    }
                }
            }
            pen += font.advance(ch);
        }
        pen
    }

    /// アンチエイリアス文字 + 影 (右下へ `offset` px、ぼかし無し)。写真の上の大きな数字用
    #[allow(clippy::too_many_arguments)]
    pub fn aa_text_shadow(&mut self, font: &AaFont, text: &str, x: i32, y: i32, c: Color, offset: i32, shadow_alpha: u8) -> i32 {
        let mut pen = x + offset;
        for ch in text.chars() {
            if let Some(g) = font.glyph(ch) {
                let gx = pen + i32::from(g.x_offset);
                for row in 0..font.height as usize {
                    for col in 0..g.width as usize {
                        let a = g.alpha_at(col, row);
                        if a != 0 {
                            let a32 = a as u32 * shadow_alpha as u32 / 15;
                            self.blend_px(gx + col as i32, y + offset + row as i32, color::palette::BLACK, a32 as u8);
                        }
                    }
                }
            }
            pen += font.advance(ch);
        }
        self.aa_text(font, text, x, y, c)
    }

    /// パレット文字列のアイコン ([`super::icons`]) を (x, y) に描く
    pub fn icon(&mut self, icon: &super::icons::Icon, x: i32, y: i32) {
        for (row, line) in icon.rows.iter().enumerate() {
            for (col, ch) in line.bytes().enumerate() {
                if let Some((c, a)) = super::icons::ink(ch) {
                    self.blend_px(x + col as i32, y + row as i32, c, a);
                }
            }
        }
    }
}

impl Canvas<'_> {
    /// 1 色のアイコン (`w` の画素を `c` で、他のパレット文字はそのまま)
    pub fn icon_tinted(&mut self, icon: &super::icons::Icon, x: i32, y: i32, c: Color) {
        for (row, line) in icon.rows.iter().enumerate() {
            for (col, ch) in line.bytes().enumerate() {
                if ch == b'w' {
                    self.put(x + col as i32, y + row as i32, c);
                } else if let Some((ink, a)) = super::icons::ink(ch) {
                    self.blend_px(x + col as i32, y + row as i32, ink, a);
                }
            }
        }
    }

    /// 両端が半円の「錠剤」形 (高さ `h`、半径 h/2)
    pub fn pill(&mut self, x: i32, y: i32, w: i32, h: i32, c: Color, alpha: u8) {
        self.panel(
            x,
            y,
            w,
            h,
            &Panel {
                tint: c,
                alpha,
                edge: 0,
                radius: (h / 2).min(6) as u8,
            },
        );
    }
}

impl OriginDimensions for Canvas<'_> {
    fn size(&self) -> Size {
        Size::new(WIDTH as u32, HEIGHT as u32)
    }
}

impl DrawTarget for Canvas<'_> {
    type Color = Rgb565;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(p, c) in pixels {
            self.put(p.x, p.y, RawU16::from(c).into_inner());
        }
        Ok(())
    }
}
