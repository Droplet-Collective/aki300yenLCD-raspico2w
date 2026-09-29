//! Layer 5: フレームバッファ + ダブルバッファリング
//!
//! FrameBuffer を使った DMA 転送テスト。
//! Channel ベースのダブルバッファリングでティアリングフリーな
//! スクロールカラーグラデーションを表示する。
//!
//! # アーキテクチャ
//!
//! ```text
//! main (描画タスク)                display_task (DMA転送)
//! ┌──────────────────────┐        ┌──────────────────────┐
//! │ back に描画            │        │ front を DMA 転送      │
//! │ SWAP_CH.send(back)    │───────▶│ try_receive()         │
//! │ back = RETURN_CH      │◀───────│ RETURN_CH.send(front) │
//! │       .receive()      │        │ front = new_front     │
//! └──────────────────────┘        └──────────────────────┘
//! ```
//!
//! - main: back バッファにアニメーション描画 → SWAP_CH で送信
//! - display_task: front バッファを DMA 転送 → VSYNC 境界で swap
//! - Channel による所有権移動で safe なダブルバッファリングを実現
//!
//! # 合格基準
//! - [ ] LCD にアニメーションが表示される
//! - [ ] ティアリング (画面の裂け) がない
//! - [ ] 60fps を維持 (defmt でフレームレート表示)
//! - [ ] 長時間安定動作

#![no_std]
#![no_main]

use core::ptr::addr_of_mut;
use defmt::*;
use embassy_executor::Spawner;
use embassy_rp::bind_interrupts;
use embassy_rp::peripherals::*;
use embassy_rp::pio::program::pio_asm;
use embassy_rp::pio::{
    Config, Direction, FifoJoin, InterruptHandler, Pio, ShiftConfig, ShiftDirection,
};
use embassy_rp::Peri;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use fixed::FixedU32;
use fixed::types::extra::U8;
use pico2w_300yen_lcd::lcd::framebuffer::*;
use pico2w_300yen_lcd::lcd::timing::*;
use {defmt_rtt as _, panic_probe as _};

// RP2350 bootrom 用 IMAGE_DEF (版数付き) と picotool 用 binary_info を埋め込む
pico2w_300yen_lcd::firmware_image_def!();

bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => InterruptHandler<PIO0>;
});

// ============================================================
// ペリフェラル構造体 (TaskFn 16引数制限の回避)
// ============================================================

/// display_task に渡すペリフェラル一式
struct DisplayPeripherals {
    pio0: Peri<'static, PIO0>,
    pin2: Peri<'static, PIN_2>,
    pin3: Peri<'static, PIN_3>,
    pin4: Peri<'static, PIN_4>,
    pin5: Peri<'static, PIN_5>,
    pin6: Peri<'static, PIN_6>,
    pin7: Peri<'static, PIN_7>,
    pin8: Peri<'static, PIN_8>,
    pin9: Peri<'static, PIN_9>,
    pin10: Peri<'static, PIN_10>,
    pin11: Peri<'static, PIN_11>,
    pin12: Peri<'static, PIN_12>,
    pin13: Peri<'static, PIN_13>,
    pin14: Peri<'static, PIN_14>,
    pin15: Peri<'static, PIN_15>,
    pin16: Peri<'static, PIN_16>,
    pin17: Peri<'static, PIN_17>,
    pin18: Peri<'static, PIN_18>,
    pin19: Peri<'static, PIN_19>,
    pin20: Peri<'static, PIN_20>,
    pin21: Peri<'static, PIN_21>,
    pin22: Peri<'static, PIN_22>,
    dma_ch0: Peri<'static, DMA_CH0>,
}

// ============================================================
// ダブルバッファ管理
// ============================================================

/// フレームバッファ A (BSS 配置、ゼロ初期化)
///
/// 192 KB × 2 = 384 KB (RP2350 SRAM 520 KB の約 74%)
static mut FB_A_DATA: FrameBuffer = FrameBuffer::new();

/// フレームバッファ B (BSS 配置、ゼロ初期化)
static mut FB_B_DATA: FrameBuffer = FrameBuffer::new();

/// main → display_task: 描画完了した back バッファの所有権を送信
static SWAP_CH: Channel<CriticalSectionRawMutex, &'static mut FrameBuffer, 1> = Channel::new();

/// display_task → main: 使用済み front バッファの所有権を返却
static RETURN_CH: Channel<CriticalSectionRawMutex, &'static mut FrameBuffer, 1> = Channel::new();

// ============================================================
// 定数
// ============================================================

/// SM0 TX FIFO エントリ数 (FifoJoin::TxOnly)
const SM0_FIFO_DEPTH: usize = 8;

/// VSYNC 後のブランキング通常ライン数
///
/// V_BACK_PORCH (16) のうち 1 本は VSYNC ラインなので残り 15 本が通常ブランキング。
/// - line index 0〜14 (15本): ブランキング (BLACK_LINE)
/// - line index 15〜110 (96本): アクティブ行 0〜95 (FrameBuffer)
const V_BLANK_LINES: u32 = V_BACK_PORCH - 1; // 15

// ============================================================
// DMA バッファ (static 配置でタスクスタック節約)
// ============================================================

/// VSYNC ライン残りデータ (プレフィル 8 ワード分を除く)
static VSYNC_REMAINING: [u32; H_TOTAL as usize - SM0_FIFO_DEPTH] =
    [BLACK; H_TOTAL as usize - SM0_FIFO_DEPTH]; // 504 words

/// ブランキングライン (全 BLACK, 512 ワード)
static BLACK_LINE_BUF: [u32; LINE_WIDTH] = [BLACK; LINE_WIDTH];

// ============================================================
// display_task: DMA 転送 + VSYNC swap
// ============================================================

#[embassy_executor::task]
async fn display_task(
    res: DisplayPeripherals,
    mut front: &'static mut FrameBuffer,
) {
    let mut dma_ch0 = res.dma_ch0;

    // === SM0: ピクセル出力 + NCLK (sideset) ===
    let prg_pixel = pio_asm!(
        ".side_set 1",
        ".wrap_target",
        "    out pins, 18  side 0",
        "    nop           side 1",
        ".wrap",
    );

    // === SM1: HSYNC/VSYNC タイミング ===
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
    } = Pio::new(res.pio0, Irqs);

    // --- ピン設定 ---
    let pin2 = common.make_pio_pin(res.pin2);
    let pin3 = common.make_pio_pin(res.pin3);
    let pin4 = common.make_pio_pin(res.pin4);
    let pin5 = common.make_pio_pin(res.pin5);
    let pin6 = common.make_pio_pin(res.pin6);
    let pin7 = common.make_pio_pin(res.pin7);
    let pin8 = common.make_pio_pin(res.pin8);
    let pin9 = common.make_pio_pin(res.pin9);
    let pin10 = common.make_pio_pin(res.pin10);
    let pin11 = common.make_pio_pin(res.pin11);
    let pin12 = common.make_pio_pin(res.pin12);
    let pin13 = common.make_pio_pin(res.pin13);
    let pin14 = common.make_pio_pin(res.pin14);
    let pin15 = common.make_pio_pin(res.pin15);
    let pin16 = common.make_pio_pin(res.pin16);
    let pin17 = common.make_pio_pin(res.pin17);
    let pin18 = common.make_pio_pin(res.pin18);
    let pin19 = common.make_pio_pin(res.pin19);
    let nclk_pin = common.make_pio_pin(res.pin20);
    let hsync_pin = common.make_pio_pin(res.pin21);
    let vsync_pin = common.make_pio_pin(res.pin22);

    // ピン方向を出力に設定
    sm0.set_pin_dirs(Direction::Out, &[
        &pin2, &pin3, &pin4, &pin5, &pin6, &pin7, &pin8, &pin9, &pin10, &pin11, &pin12, &pin13,
        &pin14, &pin15, &pin16, &pin17, &pin18, &pin19, &nclk_pin,
    ]);
    sm1.set_pin_dirs(Direction::Out, &[&hsync_pin, &vsync_pin]);

    // --- SM0 設定 ---
    let loaded_pixel = common.load_program(&prg_pixel.program);
    let mut cfg0 = Config::default();
    cfg0.use_program(&loaded_pixel, &[&nclk_pin]);
    cfg0.set_out_pins(&[
        &pin2, &pin3, &pin4, &pin5, &pin6, &pin7, &pin8, &pin9, &pin10, &pin11, &pin12, &pin13,
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

    info!("Display task: starting PIO");

    // 両 SM を同時に開始
    common.apply_sm_batch(|batch| {
        batch.set_enable(&mut sm0, true);
        batch.set_enable(&mut sm1, true);
    });

    // === フレームループ ===
    let mut frame_count: u32 = 0;

    loop {
        // --- VSYNC ライン: DMA で黒ピクセルを転送 ---
        sm0.tx()
            .dma_push(dma_ch0.reborrow(), &VSYNC_REMAINING, false)
            .await;

        // --- 通常ライン #1 (line index 0): ブランキング ---
        sm0.tx()
            .dma_push(dma_ch0.reborrow(), &BLACK_LINE_BUF, false)
            .await;

        // --- 通常ライン #2〜#111 (line index 1〜110) ---
        for line in 1..V_NORMAL_LINES {
            // SM1 タイミングデータ
            sm1.tx().wait_push(SM1_HSYNC_COUNT).await;
            sm1.tx().wait_push(SM1_REST_COUNT).await;

            if line < V_BLANK_LINES {
                // ブランキング期間: BLACK ライン送信
                sm0.tx()
                    .dma_push(dma_ch0.reborrow(), &BLACK_LINE_BUF, false)
                    .await;
            } else {
                // アクティブ期間: フレームバッファの行を送信
                let fb_row = (line - V_BLANK_LINES) as usize;
                sm0.tx()
                    .dma_push(dma_ch0.reborrow(), front.row_slice(fb_row), false)
                    .await;
            }
        }

        // --- 次フレームの SM1 VSYNC + 通常ライン #1 データ (5値) ---
        sm1.tx().wait_push(SM1_HSYNC_COUNT).await;
        sm1.tx().wait_push(SM1_VSYNC_REST_COUNT).await;
        sm1.tx().wait_push(SM1_NORMAL_LINES_Y).await;
        sm1.tx().wait_push(SM1_HSYNC_COUNT).await;
        sm1.tx().wait_push(SM1_REST_COUNT).await;

        frame_count = frame_count.wrapping_add(1);
        if frame_count % (TARGET_FPS * 3) == 0 {
            info!("Display running (frame {})", frame_count);
        }

        // --- VSYNC 境界: swap チェック ---
        // main が描画完了した back バッファがあれば swap する
        if let Ok(new_front) = SWAP_CH.try_receive() {
            RETURN_CH.send(front).await;
            front = new_front;
        }
    }
}

// ============================================================
// main: 描画タスク
// ============================================================

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());
    info!("Layer 5: Framebuffer + double buffering test");

    // フレームバッファ初期化 (BSS 領域から &'static mut を取得)
    // Safety: 各バッファは初期化後、一方が display_task に、他方が main に
    // 排他的に渡される。以降は Channel を通じて所有権が移動するため安全。
    let fb_a: &'static mut FrameBuffer = unsafe { &mut *addr_of_mut!(FB_A_DATA) };
    let fb_b: &'static mut FrameBuffer = unsafe { &mut *addr_of_mut!(FB_B_DATA) };

    // display_task 起動 (fb_a を初期 front として渡す)
    spawner
        .spawn(display_task(
            DisplayPeripherals {
                pio0: p.PIO0,
                pin2: p.PIN_2,
                pin3: p.PIN_3,
                pin4: p.PIN_4,
                pin5: p.PIN_5,
                pin6: p.PIN_6,
                pin7: p.PIN_7,
                pin8: p.PIN_8,
                pin9: p.PIN_9,
                pin10: p.PIN_10,
                pin11: p.PIN_11,
                pin12: p.PIN_12,
                pin13: p.PIN_13,
                pin14: p.PIN_14,
                pin15: p.PIN_15,
                pin16: p.PIN_16,
                pin17: p.PIN_17,
                pin18: p.PIN_18,
                pin19: p.PIN_19,
                pin20: p.PIN_20,
                pin21: p.PIN_21,
                pin22: p.PIN_22,
                dma_ch0: p.DMA_CH0,
            },
            fb_a,
        ))
        .unwrap();

    // メインタスク: back バッファにアニメーション描画
    let mut back: &'static mut FrameBuffer = fb_b;
    let mut offset: u32 = 0;

    loop {
        // カラーグラデーション描画
        for y in 0..ACTIVE_HEIGHT {
            for x in 0..H_ACTIVE as usize {
                let r = ((x as u32 + offset) * 63 / H_ACTIVE) & 63;
                let g = ((y as u32 + offset) * 63 / ACTIVE_HEIGHT as u32) & 63;
                let b = (63 - r) & 63;
                back.set_pixel(x, y, rgb666(r, g, b));
            }
        }

        // 描画完了した back バッファを display_task に送信
        SWAP_CH.send(back).await;
        // 使い終わった front バッファを受け取って次の back として使用
        back = RETURN_CH.receive().await;

        offset = offset.wrapping_add(1);
    }
}
