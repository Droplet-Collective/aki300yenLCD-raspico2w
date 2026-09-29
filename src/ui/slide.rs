//! スライドショーの切り替え演出 (背景層だけを暗くして次の写真を読み、明るく戻す)
//!
//! ```text
//!  明るさ 32 ─────┐                              ┌────── 32
//!                 └─ FADE_OUT ─┐  読み込み中   ┌─ FADE_IN ┘
//!  LOADING_LEVEL               └──────────────┘
//! ```
//!
//! 背景用のバッファは 1 枚 (76.8 KB) しか無いので、読み込み中は暗いまま新しい写真の行が順に
//! 上書きされていく (下から上の BMP なら下の行から現れる)。重ね描き (時計や文字) は常に通常の明るさ。

/// 暗くするのにかける時間 (ms)
pub const FADE_OUT_MS: u32 = 500;
/// 明るく戻すのにかける時間 (ms)
pub const FADE_IN_MS: u32 = 700;
/// 読み込み中の背景の明るさ (0..=32)
pub const LOADING_LEVEL: u8 = 7;

/// なめらかな 0..=256 (smoothstep)
fn ease(t256: u32) -> u32 {
    let t = t256.min(256);
    (3 * t * t * 256 - 2 * t * t * t) / (256 * 256)
}

/// 暗くし始めてから `elapsed_ms` の明るさ
pub fn fade_out_level(elapsed_ms: u32) -> u8 {
    let t = elapsed_ms.min(FADE_OUT_MS) * 256 / FADE_OUT_MS;
    let span = 32 - LOADING_LEVEL as u32;
    (32 - span * ease(t) / 256) as u8
}

/// 明るくし始めてから `elapsed_ms` の明るさ
pub fn fade_in_level(elapsed_ms: u32) -> u8 {
    let t = elapsed_ms.min(FADE_IN_MS) * 256 / FADE_IN_MS;
    let span = 32 - LOADING_LEVEL as u32;
    (LOADING_LEVEL as u32 + span * ease(t) / 256) as u8
}
