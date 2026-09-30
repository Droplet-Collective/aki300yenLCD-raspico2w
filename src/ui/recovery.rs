//! 回復モードの画面 (0.4.2〜、docs/ticker.md §8)
//!
//! 回復モードは「Wi-Fi につないで OTA を確認する」だけの最小構成なので、画面も黒地に `FONT_6X10` の
//! 文字だけ (写真 / ガラス / AA 数字 / 東雲フォントは使わない)。400×96 に 66 桁 × 8 行。
//!
//! ```text
//! y  0〜11  RECOVERY MODE  ticker v0.4.2 slot B          (赤い帯)
//! y 13〜    last reset: panic src/ui/slide.rs:123 @61s #2 (赤)
//!           Wi-Fi: MyHome 192.168.1.23                     (緑 / 黄)
//!           OTA: up to date (latest 0.4.2), next check in 42s
//!           ...
//! ```

use super::canvas::Canvas;
use super::color::{Color, palette, rgb};
use super::screen::{Tone, tone_color};
use super::{HEIGHT, WIDTH};

/// 本文の行数 (帯の下)
pub const ROWS: usize = 8;
/// 1 行の桁数 (6 px)
pub const COLUMNS: usize = WIDTH / 6;

const BAND_H: i32 = 12;
const ROW_TOP: i32 = 13;
const ROW_PITCH: i32 = 10;
const BAND: Color = rgb(150, 24, 24);

pub struct RecoveryView<'a> {
    /// 帯の左 (`RECOVERY MODE` など)
    pub title: &'a str,
    /// 帯の右 (版数 / 区画)
    pub ident: &'a str,
    /// 本文 (空の行は描かない)
    pub rows: [(&'a str, Tone); ROWS],
}

pub fn render(canvas: &mut Canvas, view: &RecoveryView<'_>) {
    canvas.fill_rect(0, 0, WIDTH as i32, HEIGHT as i32, palette::BLACK);
    canvas.fill_rect(0, 0, WIDTH as i32, BAND_H, BAND);
    canvas.small(view.title, 3, 1, palette::WHITE);
    let ident_x = WIDTH as i32 - 3 - view.ident.len() as i32 * 6;
    canvas.small(view.ident, ident_x.max(3 + view.title.len() as i32 * 6 + 6), 1, palette::OFF_WHITE);
    for (i, (text, tone)) in view.rows.iter().enumerate() {
        if text.is_empty() {
            continue;
        }
        canvas.small(text, 2, ROW_TOP + i as i32 * ROW_PITCH, tone_color(*tone));
    }
}
