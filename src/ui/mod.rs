//! 画面の描画 (ハードウェアに依存しない純粋なコード)
//!
//! `ticker` の画面 (写真の背景 + 時計 / 天気 / 流れる文字 / 状態) を 400×96 の RGB565 配列に描く。
//! `core` と `embedded-graphics` (6×10 / 5×7 の英数字フォント)、`heapless`、`crate::font::shinonome`
//! だけに依存するので、同じソースをホストのシミュレータ `tools/ui-sim` が `#[path]` で取り込み、
//! 書き込む前に PNG / GIF で画面を確かめられる (docs/ui-sim.md)。
//!
//! - [`color`] — RGB565 / RGB666 / RGB888 の変換、半透明の合成、ディザ
//! - [`canvas`] — 描画口 (塗り、ガラス板、角丸、文字 3 種、アイコン)
//! - [`aafont`] — 時計 / 気温のアンチエイリアス数字 (DejaVu Sans から生成、`aafont_data`)
//! - [`icons`] — 天気のドット絵 (15×15) と状態表示の小物
//! - [`bmp`] — SD の BMP を行ごとに読み、画面いっぱいに拡大縮小して背景にする
//! - [`background`] — 写真が無いときのグラデーション、フェード
//! - [`screen`] — 画面構成 ([`screen::View`] を [`screen::Layout`] で描く)
//! - [`scroll`] — 流れる文字の組み立て (文字 + 設定ページの URL とコード、0.5.1〜)
//! - [`slide`] — スライドショーの切り替え (背景のフェード) の明るさ
//! - [`recovery`] — 回復モードの画面 (黒地に `FONT_6X10` だけ、0.4.2〜)

pub mod aafont;
pub mod aafont_data;
pub mod background;
pub mod bmp;
pub mod canvas;
pub mod color;
pub mod icons;
pub mod recovery;
pub mod screen;
pub mod scroll;
pub mod slide;

/// 画面の幅 (px)
pub const WIDTH: usize = 400;
/// 画面の高さ (px)
pub const HEIGHT: usize = 96;
/// 1 画面の画素数
pub const PIXELS: usize = WIDTH * HEIGHT;
