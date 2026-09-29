//! ビットマップフォント
//!
//! - [`misaki`]: 美咲フォント (8×8 日本語ビットマップ、Num Kadoma 作)。`fonts/misaki/` のテーブルを
//!   フラッシュに埋め込み、Unicode から二分探索で引く。`ticker` の日本語表示に使う。

pub mod misaki;
