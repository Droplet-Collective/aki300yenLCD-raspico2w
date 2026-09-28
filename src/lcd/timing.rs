//! LTA042B010F タイミングパラメータ
//!
//! コミュニティ解析 (naruken, xcrosgs2wy) に基づく確定値

/// LCD 水平解像度
pub const H_ACTIVE: u32 = 400;

/// LCD 垂直解像度
pub const V_ACTIVE: u32 = 96;

/// HSYNC 開始から表示開始までのクロック数
/// **注意**: HSYNC パルス幅 (5 NCLK) を含む。
/// 標準用語の "back porch" とは異なり、HSYNC パルス + 実際のバックポーチ = 107 NCLK。
pub const H_BACK_PORCH: u32 = 107;

/// 水平フロントポーチ (表示終了 → 次 HSYNC)
pub const H_FRONT_PORCH: u32 = 5;

/// 水平合計クロック数 (1ライン)
pub const H_TOTAL: u32 = H_ACTIVE + H_BACK_PORCH + H_FRONT_PORCH; // 512

/// 垂直バックポーチ (VSYNC → 表示開始)
pub const V_BACK_PORCH: u32 = 16;

/// 垂直フロントポーチ (表示終了 → 次 VSYNC)
pub const V_FRONT_PORCH: u32 = 0;

/// 垂直合計ライン数 (1フレーム)
pub const V_TOTAL: u32 = V_ACTIVE + V_BACK_PORCH + V_FRONT_PORCH; // 112

/// VSYNC サンプリングオフセット (HSYNC 開始から N クロック後)
pub const VSYNC_SAMPLE_OFFSET: u32 = 98;

/// 目標フレームレート (Hz)
pub const TARGET_FPS: u32 = 60;

/// 目標ピクセルクロック周波数 (Hz)
/// H_TOTAL × V_TOTAL × FPS = 512 × 112 × 60 = 3,440,640 Hz
pub const PIXEL_CLOCK_HZ: u32 = H_TOTAL * V_TOTAL * TARGET_FPS;

/// システムクロック (Hz)
pub const SYS_CLOCK_HZ: u32 = 150_000_000;

/// PIO クロック分周比 (整数部)
/// NCLK は 2 PIO サイクルで 1 ピクセル (HIGH + LOW)
/// 分周比 = SYS_CLOCK / (PIXEL_CLOCK × 2) = 150_000_000 / (3_440_640 × 2) ≈ 21.8
pub const PIO_CLK_DIV_INT: u16 = (SYS_CLOCK_HZ / (PIXEL_CLOCK_HZ * 2)) as u16; // = 21

/// PIO クロック分周比 (小数部, 0-255)
pub const PIO_CLK_DIV_FRAC: u8 = {
    let remainder = SYS_CLOCK_HZ % (PIXEL_CLOCK_HZ * 2);
    ((remainder as u64 * 256) / (PIXEL_CLOCK_HZ as u64 * 2)) as u8
};

// ============================================================
// Layer 2: HSYNC/VSYNC PIO タイミング生成用定数
// ============================================================

/// HSYNC パルス幅 (NCLK cycles)
pub const HSYNC_PULSE_WIDTH: u32 = 5;

/// HSYNC 後の残りクロック (1 ライン - HSYNC パルス幅)
pub const H_REST: u32 = H_TOTAL - HSYNC_PULSE_WIDTH; // 507

/// VSYNC パルス幅 (ライン数)
pub const VSYNC_PULSE_LINES: u32 = 1;

/// 通常ライン数 (V_TOTAL - VSYNC_PULSE_LINES)
pub const V_NORMAL_LINES: u32 = V_TOTAL - VSYNC_PULSE_LINES; // 111

/// PIO ループカウント: HSYNC パルスフェーズ
///
/// PIO HSYNC フェーズの内訳:
/// - overhead: set + pull + mov + nop = 4 PIO (2 NCLK)
/// - ループ: nop + jmp = 2 PIO × (X+1) 回 = (X+1) NCLK
/// - 合計: X + 3 NCLK
///
/// X = HSYNC_PULSE_WIDTH - 3 = 2
pub const PIO_HSYNC_COUNT: u32 = HSYNC_PULSE_WIDTH - 3; // 2

/// PIO ループカウント: 残りフェーズ
///
/// PIO 残りフェーズの内訳:
/// - overhead: set + pull + mov + nop = 4 PIO (2 NCLK)
/// - ループ: nop + jmp = 2 PIO × (X+1) 回 = (X+1) NCLK
/// - 合計: X + 3 NCLK
/// - ライン末尾: nop + jmp = 2 PIO (1 NCLK)
///
/// HSYNC(X_h+3) + REST(X_r+3) + LINE_END(1) = 512
/// → X_r = H_REST - 4 = 503
pub const PIO_REST_COUNT: u32 = H_REST - 4; // 503

/// PIO Y レジスタ: 通常ライン数 (jmp y-- で Y+1 回ループ)
pub const PIO_NORMAL_LINES_COUNT: u32 = V_NORMAL_LINES - 1; // 110

// ============================================================
// Layer 3: デュアルSM 構成用定数
// ============================================================
// SM0: ピクセル出力 + NCLK (sideset), autopull
// SM1: HSYNC/VSYNC タイミング (sideset なし, SET 2 pins)
//
// SM1 は sideset を使わないためオーバーヘッドが異なる:
//   HSYNC フェーズ: set(1) + pull(1) + mov(1) + loop(X+1) = X + 4 NCLK
//   残りフェーズ:   set(1) + pull(1) + mov(1) + loop(X'+1) = X' + 4 NCLK
//   ライン末尾: jmp y--(1) = 1 NCLK
//   合計: X + X' + 9 = 512

/// SM1 HSYNC カウント (Layer3: overhead=4, HSYNC_PULSE=5, count=1)
pub const SM1_HSYNC_COUNT: u32 = HSYNC_PULSE_WIDTH - 4; // 1

/// SM1 残りカウント (Layer3: overhead=4, jmp=1, rest=507, count=502)
pub const SM1_REST_COUNT: u32 = H_REST - 4 - 1; // 502

/// SM1 VSYNC ライン用 REST カウント
/// 遷移命令 (pull+mov=2 NCLK) が通常ライン末尾 (jmp=1 NCLK) より 1 NCLK 多いため -1
pub const SM1_VSYNC_REST_COUNT: u32 = SM1_REST_COUNT - 1; // 501

/// SM1 通常ラインカウント (Y = V_NORMAL_LINES - 1)
pub const SM1_NORMAL_LINES_Y: u32 = V_NORMAL_LINES - 1; // 110

/// SM1 クロック分周比 raw bits (SM0 の 2倍: 1 PIO cycle = 1 NCLK)
pub const SM1_CLK_DIV_BITS: u32 = ((PIO_CLK_DIV_INT as u32) << 8 | PIO_CLK_DIV_FRAC as u32) * 2;

/// 表示前ブランキングピクセル数 (= H_BACK_PORCH, HSYNCパルス含む)
pub const H_BLANK_BEFORE_ACTIVE: u32 = H_BACK_PORCH; // 107

// ============================================================
// Layer 7: 全フレーム DMA 用定数・関数
// ============================================================

/// SM1 が 1 フレームで消費するワード数
///
/// - VSYNC 行: 3 ワード (`SM1_HSYNC_COUNT` + `SM1_VSYNC_REST_COUNT` + `SM1_NORMAL_LINES_Y`)
/// - 通常行: 2 ワード × (V_TOTAL − 1) = 2 × 111 = 222 ワード
/// - 合計: 225 ワード
pub const SM1_FRAME_SIZE: usize = 3 + 2 * (V_TOTAL as usize - 1);

/// SM1 の 1 フレーム分のタイミングデータを生成する
///
/// SM1 PIO プログラムの pull 順序:
///   1. `SM1_HSYNC_COUNT`      — VSYNC ライン HSYNC ループ
///   2. `SM1_VSYNC_REST_COUNT` — VSYNC ライン 残りループ
///   3. `SM1_NORMAL_LINES_Y`   — 通常ラインの Y カウンタ (jmp y--)
///   4–225. `SM1_HSYNC_COUNT`, `SM1_REST_COUNT` を 111 回繰り返し
pub const fn sm1_frame_data() -> [u32; SM1_FRAME_SIZE] {
    let mut buf = [0u32; SM1_FRAME_SIZE];
    buf[0] = SM1_HSYNC_COUNT;
    buf[1] = SM1_VSYNC_REST_COUNT;
    buf[2] = SM1_NORMAL_LINES_Y;
    let mut i: usize = 0;
    while i < (V_TOTAL as usize - 1) {
        buf[3 + i * 2] = SM1_HSYNC_COUNT;
        buf[3 + i * 2 + 1] = SM1_REST_COUNT;
        i += 1;
    }
    buf
}
