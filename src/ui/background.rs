//! 背景層: 写真が無いときの既定のグラデーションと、フェード付きの背景コピー

use super::color::{self, Color};
use super::{HEIGHT, WIDTH};

/// 写真が無いときの背景 (夜空のような紺 → 青紫の斜めグラデーション + 周辺減光、ディザ付き)
pub fn fill_gradient(dst: &mut [Color]) {
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            // t: 左上 0 → 右下 1 (横方向を重く)
            let t = (x as u32 * 3 * 256 / WIDTH as u32 + y as u32 * 256 / HEIGHT as u32) / 4; // 0..256
            let (r0, g0, b0) = (14u32, 22u32, 48u32);
            let (r1, g1, b1) = (58u32, 34u32, 92u32);
            let mut r = r0 + (r1 - r0) * t / 256;
            let mut g = g0 + (g1 - g0) * t / 256;
            let mut b = b0 + (b1 - b0) * t / 256;
            // 右上に淡い光
            let dx = x as i32 - 300;
            let dy = y as i32 + 10;
            let d2 = (dx * dx + 4 * dy * dy) as u32;
            let glow = 40u32.saturating_sub(d2 / 600);
            r += glow / 2;
            g += glow / 2;
            b += glow;
            // 周辺減光 (上下の端を少し暗く)
            let edge = (y as i32 - 48).unsigned_abs();
            let v = 256 - (edge * edge / 24).min(64);
            r = r * v / 256;
            g = g * v / 256;
            b = b * v / 256;
            dst[y * WIDTH + x] = color::rgb_dithered(r.min(255) as u8, g.min(255) as u8, b.min(255) as u8, x, y);
        }
    }
}

/// 背景 `src` を明るさ `level` (0..=32、32 でそのまま) で `dst` へ写す
pub fn copy_dimmed(src: &[Color], dst: &mut [Color], level: u8) {
    let n = WIDTH * HEIGHT;
    if level >= 32 {
        dst[..n].copy_from_slice(&src[..n]);
    } else if level == 0 {
        dst[..n].fill(0);
    } else {
        for (d, s) in dst[..n].iter_mut().zip(&src[..n]) {
            *d = color::dim(*s, level);
        }
    }
}
