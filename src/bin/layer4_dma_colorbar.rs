//! Layer 4: DMA スキャンライン転送 カラーバーテスト
//!
//! DMA を使って SM0 (ピクセルデータ) を転送し、
//! SM1 (HSYNC/VSYNC) は CPU push で駆動するハイブリッド方式。
//! カラーバーテストパターンを表示する。
//!
//! # 合格基準
//! - [ ] LCD 画面にカラーバー (8色帯) が表示される
//! - [ ] DMA 転送中に CPU が空いている (defmt ログ出力で確認)
//! - [ ] NCLK 停止リスクが実質的に解消されている
//! - [ ] 長時間 (10分以上) 安定動作する

#![no_std]
#![no_main]

use defmt::*;
use embassy_executor::Spawner;
use embassy_rp::peripherals::PIO0;
use embassy_rp::pio::{
    Config, Direction, FifoJoin, InterruptHandler, Pio, ShiftConfig, ShiftDirection,
};
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
const fn reverse6(v: u32) -> u32 {
    let v = v & 0x3F;
    ((v & 0x20) >> 5)
        | ((v & 0x10) >> 3)
        | ((v & 0x08) >> 1)
        | ((v & 0x04) << 1)
        | ((v & 0x02) << 3)
        | ((v & 0x01) << 5)
}

/// RGB666 ピクセルワード生成 (ビット順反転込み)
const fn rgb666(r: u32, g: u32, b: u32) -> u32 {
    (reverse6(r) << 12) | (reverse6(g) << 6) | reverse6(b)
}

const WHITE: u32 = rgb666(63, 63, 63);
const YELLOW: u32 = rgb666(63, 63, 0);
const CYAN: u32 = rgb666(0, 63, 63);
const GREEN: u32 = rgb666(0, 63, 0);
const MAGENTA: u32 = rgb666(63, 0, 63);
const RED: u32 = rgb666(63, 0, 0);
const BLUE: u32 = rgb666(0, 0, 63);
const BLACK: u32 = 0;

/// カラーバーの色テーブル (8バンド)
const COLORBAR_COLORS: [u32; 8] = [WHITE, YELLOW, CYAN, GREEN, MAGENTA, RED, BLUE, BLACK];

/// SM0 TX FIFO エントリ数 (FifoJoin::TxOnly 使用時)
const SM0_FIFO_DEPTH: usize = 8;

/// カラーバー1バンドあたりのピクセル数
const BAND_WIDTH: usize = H_ACTIVE as usize / 8; // 400 / 8 = 50, 端数なし

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let p = embassy_rp::init(Default::default());
    info!("Layer 4: DMA colorbar output test");

    // === DMA バッファ構築 ===
    // VSYNC ライン: プレフィル分を除いた残りを DMA で供給
    let vsync_remaining = [BLACK; H_TOTAL as usize - SM0_FIFO_DEPTH]; // 512 - 8 = 504 words

    // カラーバーライン: 107 BLACK + 400 カラー + 5 BLACK
    let mut colorbar_line = [BLACK; H_TOTAL as usize]; // 512 words
    {
        let active_start = H_BLANK_BEFORE_ACTIVE as usize; // 107
        for (band_idx, &color) in COLORBAR_COLORS.iter().enumerate() {
            let start = active_start + band_idx * BAND_WIDTH;
            for px in 0..BAND_WIDTH {
                colorbar_line[start + px] = color;
            }
        }
        // H_FRONT_PORCH (末尾5ピクセル) は初期値 BLACK のまま
    }

    // === SM0: ピクセル出力 + NCLK (2命令) ===
    let prg_pixel = pio_asm!(
        ".side_set 1",
        ".wrap_target",
        "    out pins, 18  side 0",
        "    nop           side 1",
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

    // DMA チャネル
    let mut dma_ch0 = p.DMA_CH0;

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
    cfg0.use_program(&loaded_pixel, &[&nclk_pin]);
    cfg0.set_out_pins(&[
        &pin2, &pin3, &pin4, &pin5, &pin6, &pin7,
        &pin8, &pin9, &pin10, &pin11, &pin12, &pin13,
        &pin14, &pin15, &pin16, &pin17, &pin18, &pin19,
    ]);
    cfg0.shift_out = ShiftConfig {
        auto_fill: true,
        threshold: 18,
        direction: ShiftDirection::Right,
    };
    cfg0.clock_divider = FixedU32::<U8>::from_bits(
        (PIO_CLK_DIV_INT as u32) << 8 | PIO_CLK_DIV_FRAC as u32,
    );
    cfg0.fifo_join = FifoJoin::TxOnly;

    // --- SM1 設定 ---
    let loaded_timing = common.load_program(&prg_timing.program);
    let mut cfg1 = Config::default();
    cfg1.use_program(&loaded_timing, &[]);
    cfg1.set_set_pins(&[&hsync_pin, &vsync_pin]);
    cfg1.clock_divider = FixedU32::<U8>::from_bits(SM1_CLK_DIV_BITS);
    cfg1.fifo_join = FifoJoin::TxOnly;

    sm0.set_config(&cfg0);
    sm1.set_config(&cfg1);

    // --- FIFO 事前充填 ---
    sm1.tx().push(SM1_HSYNC_COUNT);
    sm1.tx().push(SM1_VSYNC_REST_COUNT);
    sm1.tx().push(SM1_NORMAL_LINES_Y);
    sm1.tx().push(SM1_HSYNC_COUNT);
    sm1.tx().push(SM1_REST_COUNT);

    for _ in 0..SM0_FIFO_DEPTH {
        sm0.tx().push(BLACK);
    }

    info!("Starting PIO with DMA colorbar");
    info!(
        "SM1 counts: HSYNC={}, REST={}, LINES_Y={}",
        SM1_HSYNC_COUNT, SM1_REST_COUNT, SM1_NORMAL_LINES_Y
    );
    info!(
        "DMA buffer sizes: vsync_remaining={} words, colorbar_line={} words",
        vsync_remaining.len(),
        colorbar_line.len()
    );

    // 両 SM を同時に開始
    common.apply_sm_batch(|batch| {
        batch.set_enable(&mut sm0, true);
        batch.set_enable(&mut sm1, true);
    });

    // === メインフレームループ ===
    let mut frame_count: u32 = 0;
    loop {
        // VSYNC ライン: DMA で黒ピクセルを転送
        sm0.tx().dma_push(dma_ch0.reborrow(), &vsync_remaining, false).await;

        // 通常ライン #1 (SM1 データはプレフィル/前ループ末尾で供給済み)
        // SM0 ピクセルデータのみ DMA 転送
        sm0.tx().dma_push(dma_ch0.reborrow(), &colorbar_line, false).await;

        // 通常ライン #2〜#111 (110 ライン)
        for _ in 1..V_NORMAL_LINES {
            // SM1 タイミングデータ
            sm1.tx().wait_push(SM1_HSYNC_COUNT).await;
            sm1.tx().wait_push(SM1_REST_COUNT).await;

            // SM0 ピクセルデータ
            sm0.tx().dma_push(dma_ch0.reborrow(), &colorbar_line, false).await;
        }

        // 次フレームの SM1 VSYNC + 通常ライン #1 データ (5値)
        sm1.tx().wait_push(SM1_HSYNC_COUNT).await;
        sm1.tx().wait_push(SM1_VSYNC_REST_COUNT).await;
        sm1.tx().wait_push(SM1_NORMAL_LINES_Y).await;
        sm1.tx().wait_push(SM1_HSYNC_COUNT).await;
        sm1.tx().wait_push(SM1_REST_COUNT).await;

        frame_count = frame_count.wrapping_add(1);
        if frame_count % (TARGET_FPS * 3) == 0 {
            info!("DMA colorbar running (frame {})", frame_count);
        }
    }
}
