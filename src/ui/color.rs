//! 色: 描画は RGB565 (`u16`)、LCD へは RGB666 (18 bit)、シミュレータの PNG は RGB888。
//!
//! バックバッファ / 背景は 1 画素 2 バイトの RGB565 (RAM の都合、docs/ticker.md の「RAM」)。
//! 垂直ブランキングのコピー (`lcd::display`) が [`to_666`] で 18 bit に広げて走査用ワードにする。
//! シミュレータ (`tools/ui-sim`) は同じ [`to_666`] のあと [`expand6`] で 8 bit にするので、
//! PNG は LCD が受け取る値そのもの (6 bit を 8 bit に引き延ばしただけ) になる。

/// RGB565 の色
pub type Color = u16;

/// 8 bit の R/G/B → RGB565 (切り捨て)
pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
    ((r as u16 >> 3) << 11) | ((g as u16 >> 2) << 5) | (b as u16 >> 3)
}

/// RGB565 → RGB666 (各 0..=63)。R/B の 5 bit は上位ビットを下へ複製して 6 bit にする
/// (31 → 63、0 → 0 で端が揃う)。
#[inline]
pub const fn to_666(c: Color) -> (u8, u8, u8) {
    let r5 = (c >> 11) as u8 & 0x1f;
    let g6 = (c >> 5) as u8 & 0x3f;
    let b5 = c as u8 & 0x1f;
    ((r5 << 1) | (r5 >> 4), g6, (b5 << 1) | (b5 >> 4))
}

/// 6 bit → 8 bit (上位ビットの複製。シミュレータの PNG 用)
#[inline]
pub const fn expand6(v: u8) -> u8 {
    (v << 2) | (v >> 4)
}

/// RGB565 → RGB888 (LCD が表示する値。[`to_666`] → [`expand6`])
pub const fn to_888(c: Color) -> (u8, u8, u8) {
    let (r, g, b) = to_666(c);
    (expand6(r), expand6(g), expand6(b))
}

const SPREAD_MASK: u32 = 0x07e0_f81f;

/// RGB565 を「G を上位 16 bit へ離した」32 bit 形式にする (R/G/B の間に 5〜6 bit の空きができ、
/// 0..=32 倍しても隣へ桁あふれしない)
#[inline(always)]
const fn spread(c: Color) -> u32 {
    let c = c as u32;
    (c | (c << 16)) & SPREAD_MASK
}

#[inline(always)]
const fn unspread(x: u32) -> Color {
    let x = x & SPREAD_MASK;
    (x | (x >> 16)) as u16
}

/// `dst` の上に `src` を不透明度 `alpha` (0..=32、32 で `src` そのもの) で重ねる
#[inline]
pub const fn blend(dst: Color, src: Color, alpha: u8) -> Color {
    let a = if alpha > 32 { 32 } else { alpha as u32 };
    let mixed = (spread(src) * a + spread(dst) * (32 - a)) >> 5;
    unspread(mixed)
}

/// 明るさを `level` / 32 倍する (0 で黒、32 でそのまま)
#[inline]
pub const fn dim(c: Color, level: u8) -> Color {
    let a = if level > 32 { 32 } else { level as u32 };
    unspread((spread(c) * a) >> 5)
}

/// 4×4 の Bayer 行列 (0..16)。背景の 8 bit → 5/6 bit の組織的ディザに使う
pub const BAYER4: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

/// 8 bit の R/G/B → RGB565 を位置 (x, y) の Bayer ディザ付きで (写真の空などの縞を目立たなくする)
#[inline]
pub const fn rgb_dithered(r: u8, g: u8, b: u8, x: usize, y: usize) -> Color {
    let t = BAYER4[y & 3][x & 3] as u32 * 16 + 8; // 8..248
    let r5 = (r as u32 * 31 + t) / 255;
    let g6 = (g as u32 * 63 + t) / 255;
    let b5 = (b as u32 * 31 + t) / 255;
    ((r5 << 11) | (g6 << 5) | b5) as u16
}

/// よく使う色
pub mod palette {
    use super::{Color, rgb};

    pub const BLACK: Color = 0;
    pub const WHITE: Color = rgb(255, 255, 255);
    pub const OFF_WHITE: Color = rgb(236, 240, 248);
    pub const SOFT: Color = rgb(196, 206, 222);
    pub const MUTED: Color = rgb(130, 140, 158);
    pub const FAINT: Color = rgb(88, 96, 112);
    pub const ACCENT: Color = rgb(255, 176, 72);
    pub const WARM: Color = rgb(255, 128, 104);
    pub const COOL: Color = rgb(112, 184, 255);
    pub const AQUA: Color = rgb(96, 224, 232);
    pub const GREEN: Color = rgb(96, 232, 128);
    pub const YELLOW: Color = rgb(255, 216, 64);
    pub const RED: Color = rgb(255, 88, 88);
    pub const MAGENTA: Color = rgb(240, 112, 240);
    pub const CYAN: Color = rgb(64, 232, 255);
    /// ガラスの板の地 (ほぼ黒、わずかに青)
    pub const GLASS: Color = rgb(8, 12, 22);
}
