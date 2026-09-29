//! Layer 3: 固定パターン出力テスト
//!
//! デュアルSM構成でLCDに固定色を表示する。
//! SM0: ピクセル出力 + NCLK (autopull)
//! SM1: HSYNC/VSYNC タイミング
//!
//! # 合格基準
//! - [ ] LCD 画面に単色が表示される
//! - [ ] 表示色が期待通り（白/赤/緑/青の切替確認）
//! - [ ] 表示が安定している（ちらつきなし）
//! - [ ] HSYNC/VSYNC/NCLK のタイミングが正しい
//! - [ ] ブランキング期間にピクセルデータが表示されない

#![no_std]
#![no_main]

use defmt::*;
use embassy_executor::Spawner;
use embassy_rp::peripherals::PIO0;
use embassy_rp::pio::{Config, Direction, FifoJoin, InterruptHandler, Pio, ShiftConfig, ShiftDirection};
use embassy_rp::pio::program::pio_asm;
use embassy_rp::bind_interrupts;
use fixed::FixedU32;
use fixed::types::extra::U8;
use pico2w_300yen_lcd::lcd::timing::*;
use {defmt_rtt as _, panic_probe as _};

// RP2350 bootrom 用 IMAGE_DEF (版数付き) と picotool 用 binary_info を埋め込む
pico2w_300yen_lcd::firmware_image_def!();

bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => InterruptHandler<PIO0>;
});

/// 6ビット値のビット順を反転 (MSB↔LSB)
/// PIO OUT pins の bit 順が LCD ピン配列と逆順のため必要
//
// PIO OUT pins (right shift) のビットマッピング:
// bit 0 → GP2 (B5=青MSB) ← OUT shift の最下位ビットが GPIO の最低番号に接続
// bit 5 → GP7 (B0=青LSB)
// → ソフトウェアの bit 0 が LCD の MSB に対応するため、チャンネル内ビット反転が必要
const fn reverse6(v: u32) -> u32 {
    let v = v & 0x3F; // 6ビットにマスク
    ((v & 0x20) >> 5)
    | ((v & 0x10) >> 3)
    | ((v & 0x08) >> 1)
    | ((v & 0x04) << 1)
    | ((v & 0x02) << 3)
    | ((v & 0x01) << 5)
}

/// RGB666 ピクセルワード生成 (ビット順反転込み)
///
/// `pixel_word = (reverse6(R) << 12) | (reverse6(G) << 6) | reverse6(B)`
/// OUT base=GP2, right shift で bit0→GP2
const fn rgb666(r: u32, g: u32, b: u32) -> u32 {
    (reverse6(r) << 12) | (reverse6(g) << 6) | reverse6(b)
}

const WHITE: u32 = rgb666(63, 63, 63); // 0x3FFFF
const RED: u32 = rgb666(63, 0, 0);     // 0x3F000
const GREEN: u32 = rgb666(0, 63, 0);   // 0x00FC0
const BLUE: u32 = rgb666(0, 0, 63);    // 0x0003F
const BLACK: u32 = 0;

/// SM0 TX FIFO エントリ数 (FifoJoin::TxOnly 使用時)
const SM0_FIFO_DEPTH: u32 = 8;

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let p = embassy_rp::init(Default::default());
    info!("Layer 3: Solid color output test");

    // === SM0: ピクセル出力 + NCLK (2命令) ===
    let prg_pixel = pio_asm!(
        ".side_set 1",
        ".wrap_target",
        "    out pins, 18  side 0", // ピクセル出力 + NCLK LOW (LCD サンプル)
        "    nop           side 1", // NCLK HIGH
        ".wrap",
    );

    // === SM1: HSYNC/VSYNC タイミング (19命令) ===
    let prg_timing = pio_asm!(
        ".wrap_target",
        // VSYNC active line (1 line)
        "    set pins, 0",          // HSYNC=0, VSYNC=0
        "    pull block",
        "    mov x, osr",
        "hsync_v0:",
        "    jmp x-- hsync_v0",
        //
        "    set pins, 1",          // HSYNC=1, VSYNC=0
        "    pull block",
        "    mov x, osr",
        "rest_v0:",
        "    jmp x-- rest_v0",
        //
        // 通常ラインカウントロード
        "    pull block",
        "    mov y, osr",
        //
        // 通常ラインループ
        "normal_line:",
        "    set pins, 2",          // HSYNC=0, VSYNC=1
        "    pull block",
        "    mov x, osr",
        "hsync_v1:",
        "    jmp x-- hsync_v1",
        //
        "    set pins, 3",          // HSYNC=1, VSYNC=1
        "    pull block",
        "    mov x, osr",
        "rest_v1:",
        "    jmp x-- rest_v1",
        //
        "    jmp y-- normal_line",
        ".wrap",
    );

    let Pio {
        mut common,
        mut sm0,
        mut sm1,
        ..
    } = Pio::new(p.PIO0, Irqs);

    // --- ピン設定 ---
    // RGB ピン (GP2-GP19): SM0 OUT
    let pin2 = common.make_pio_pin(p.PIN_2);
    let pin3 = common.make_pio_pin(p.PIN_3);
    let pin4 = common.make_pio_pin(p.PIN_4);
    let pin5 = common.make_pio_pin(p.PIN_5);
    let pin6 = common.make_pio_pin(p.PIN_6);
    let pin7 = common.make_pio_pin(p.PIN_7);
    let pin8 = common.make_pio_pin(p.PIN_8);
    let pin9 = common.make_pio_pin(p.PIN_9);
    let pin10 = common.make_pio_pin(p.PIN_10);
    let pin11 = common.make_pio_pin(p.PIN_11);
    let pin12 = common.make_pio_pin(p.PIN_12);
    let pin13 = common.make_pio_pin(p.PIN_13);
    let pin14 = common.make_pio_pin(p.PIN_14);
    let pin15 = common.make_pio_pin(p.PIN_15);
    let pin16 = common.make_pio_pin(p.PIN_16);
    let pin17 = common.make_pio_pin(p.PIN_17);
    let pin18 = common.make_pio_pin(p.PIN_18);
    let pin19 = common.make_pio_pin(p.PIN_19);

    // NCLK (GP20): SM0 sideset
    let nclk_pin = common.make_pio_pin(p.PIN_20);

    // HSYNC (GP21), VSYNC (GP22): SM1 set
    let hsync_pin = common.make_pio_pin(p.PIN_21);
    let vsync_pin = common.make_pio_pin(p.PIN_22);

    // ピン方向を出力に設定
    sm0.set_pin_dirs(Direction::Out, &[
        &pin2, &pin3, &pin4, &pin5, &pin6, &pin7,
        &pin8, &pin9, &pin10, &pin11, &pin12, &pin13,
        &pin14, &pin15, &pin16, &pin17, &pin18, &pin19,
        &nclk_pin,
    ]);
    sm1.set_pin_dirs(Direction::Out, &[&hsync_pin, &vsync_pin]);

    // --- SM0 設定 ---
    let loaded_pixel = common.load_program(&prg_pixel.program);
    let mut cfg0 = Config::default();
    cfg0.use_program(&loaded_pixel, &[&nclk_pin]); // sideset = NCLK
    cfg0.set_out_pins(&[
        &pin2, &pin3, &pin4, &pin5, &pin6, &pin7,
        &pin8, &pin9, &pin10, &pin11, &pin12, &pin13,
        &pin14, &pin15, &pin16, &pin17, &pin18, &pin19,
    ]);
    cfg0.shift_out = ShiftConfig {
        auto_fill: true,        // autopull 有効
        threshold: 18,          // 18ビットで autopull
        direction: ShiftDirection::Right,
    };
    // SM0 clock: 2 PIO cycles = 1 NCLK
    cfg0.clock_divider = FixedU32::<U8>::from_bits(
        (PIO_CLK_DIV_INT as u32) << 8 | PIO_CLK_DIV_FRAC as u32,
    );
    cfg0.fifo_join = FifoJoin::TxOnly;  // TX FIFO を 8 エントリに

    // --- SM1 設定 ---
    let loaded_timing = common.load_program(&prg_timing.program);
    let mut cfg1 = Config::default();
    cfg1.use_program(&loaded_timing, &[]); // sideset なし
    cfg1.set_set_pins(&[&hsync_pin, &vsync_pin]);
    // SM1 clock: SM0 の 2倍 → 1 PIO cycle = 1 NCLK
    cfg1.clock_divider = FixedU32::<U8>::from_bits(SM1_CLK_DIV_BITS);
    cfg1.fifo_join = FifoJoin::TxOnly;  // TX FIFO を 8 エントリに

    sm0.set_config(&cfg0);
    sm1.set_config(&cfg1);

    // --- FIFO 事前充填 ---
    // SM1: VSYNC ライン開始データ + 通常ライン1行目
    sm1.tx().push(SM1_HSYNC_COUNT);
    sm1.tx().push(SM1_VSYNC_REST_COUNT);
    sm1.tx().push(SM1_NORMAL_LINES_Y);
    sm1.tx().push(SM1_HSYNC_COUNT);
    sm1.tx().push(SM1_REST_COUNT);

    // SM0: VSYNC ライン先頭のプレフィル (SM0_FIFO_DEPTH エントリ分)
    for _ in 0..SM0_FIFO_DEPTH {
        sm0.tx().push(BLACK);
    }

    info!("Starting PIO with solid WHITE");
    info!(
        "SM1 counts: HSYNC={}, REST={}, LINES_Y={}",
        SM1_HSYNC_COUNT, SM1_REST_COUNT, SM1_NORMAL_LINES_Y
    );
    info!(
        "SM0 clkdiv bits={}, SM1 clkdiv bits={}",
        (PIO_CLK_DIV_INT as u32) << 8 | PIO_CLK_DIV_FRAC as u32,
        SM1_CLK_DIV_BITS
    );

    // 両 SM を同時に開始 (PioBatch でアトミックに enable)
    common.apply_sm_batch(|batch| {
        batch.set_enable(&mut sm0, true);
        batch.set_enable(&mut sm1, true);
    });

    // 表示色を順次切り替え
    let colors = [WHITE, RED, GREEN, BLUE];
    let color_names = ["WHITE", "RED", "GREEN", "BLUE"];
    let mut color_idx = 0;
    let mut frame_count: u32 = 0;

    loop {
        let color = colors[color_idx];

        // === 1フレーム生成 ===

        // VSYNC ライン (512 - プレフィル8 = 504 ピクセル)
        for _ in 0..(H_TOTAL - SM0_FIFO_DEPTH) {
            sm0.tx().wait_push(BLACK).await;
        }

        // SM1 は VSYNC ラインを消化中。余裕時間を活用してログ出力
        frame_count = frame_count.wrapping_add(1);
        if frame_count % (TARGET_FPS * 3) == 0 {
            color_idx = (color_idx + 1) % colors.len();
            info!("Color: {} (frame {})", color_names[color_idx], frame_count);
        }

        // 通常ライン #1 (SM1 データはプレフィル/前ループ末尾で供給済み)
        // SM0 ピクセルデータのみ
        for _ in 0..H_BLANK_BEFORE_ACTIVE {
            sm0.tx().wait_push(BLACK).await;
        }
        for _ in 0..H_ACTIVE {
            sm0.tx().wait_push(color).await;
        }
        for _ in 0..H_FRONT_PORCH {
            sm0.tx().wait_push(BLACK).await;
        }

        // 通常ライン #2〜#111 (110 ライン)
        // SM1 タイミング + SM0 ピクセル
        for _ in 1..V_NORMAL_LINES {
            // SM1 タイミングデータ
            sm1.tx().wait_push(SM1_HSYNC_COUNT).await;
            sm1.tx().wait_push(SM1_REST_COUNT).await;

            // SM0 ピクセルデータ
            for _ in 0..H_BLANK_BEFORE_ACTIVE {
                sm0.tx().wait_push(BLACK).await;
            }
            for _ in 0..H_ACTIVE {
                sm0.tx().wait_push(color).await;
            }
            for _ in 0..H_FRONT_PORCH {
                sm0.tx().wait_push(BLACK).await;
            }
        }

        // 次フレームの SM1 VSYNC + 通常ライン #1 データ
        sm1.tx().wait_push(SM1_HSYNC_COUNT).await;
        sm1.tx().wait_push(SM1_VSYNC_REST_COUNT).await;
        sm1.tx().wait_push(SM1_NORMAL_LINES_Y).await;
        sm1.tx().wait_push(SM1_HSYNC_COUNT).await;
        sm1.tx().wait_push(SM1_REST_COUNT).await;
    }
}
