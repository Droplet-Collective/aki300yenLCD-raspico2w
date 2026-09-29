//! ビットマップフォント
//!
//! - [`shinonome`]: 東雲フォント (14 ドット日本語ビットマップ、/efont/ プロジェクト、Public Domain)。
//!   `fonts/shinonome/` のテーブルをフラッシュに埋め込み、Unicode から二分探索で引く。`ticker` の
//!   日本語表示に使う (等倍。v0.3.0 までは美咲フォント 8×8 の 2 倍だった)。

pub mod shinonome;
