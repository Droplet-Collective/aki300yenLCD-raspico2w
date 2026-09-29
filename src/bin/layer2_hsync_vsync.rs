//! Layer 2: HSYNC/VSYNC タイミング生成テスト
//!
//! PIO を使って NCLK + HSYNC + VSYNC を同時生成する。
//! 単一 SM 方式: sideset=NCLK, set=HSYNC+VSYNC
//!
//! # 合格基準
//! - [ ] HSYNC が 512 NCLK サイクル周期で生成される
//! - [ ] VSYNC が 112 ライン周期で生成される
//! - [ ] NCLK が約 3.44MHz で連続出力される（途切れない）
//! - [ ] HSYNC パルス幅が約 5 NCLK
//! - [ ] VSYNC パルス幅が 1 ライン (512 NCLK)
//! - [ ] defmt でフレームカウントが表示される

#![no_std]
#![no_main]

use defmt::*;
use embassy_executor::Spawner;
use embassy_rp::peripherals::PIO0;
use embassy_rp::pio::{Config, Direction, FifoJoin, InterruptHandler, Pio};
use embassy_rp::pio::program::pio_asm;
use embassy_rp::bind_interrupts;
use fixed::FixedU32;
use fixed::types::extra::U8;
use pico2w_300yen_lcd::lcd::timing::{
    PIO_CLK_DIV_FRAC, PIO_CLK_DIV_INT, PIO_HSYNC_COUNT, PIO_NORMAL_LINES_COUNT,
    PIO_REST_COUNT, V_NORMAL_LINES,
};
use {defmt_rtt as _, panic_probe as _};

// RP2350 bootrom 用 IMAGE_DEF (版数付き) と picotool 用 binary_info を埋め込む
pico2w_300yen_lcd::firmware_image_def!();

bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => InterruptHandler<PIO0>;
});

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let p = embassy_rp::init(Default::default());
    info!("Layer 2: HSYNC/VSYNC timing generation test");

    // PIO プログラム: NCLK(sideset) + HSYNC/VSYNC(set) 同時生成
    // 28 命令 / 32 スロット
    let prg = pio_asm!(
        ".side_set 1",
        "",
        ".wrap_target",
        // === VSYNC アクティブライン (1 ライン) ===
        "    set pins, 0      side 0",  // [0]  HSYNC=0, VSYNC=0
        "    pull block        side 1",  // [1]  FIFO→OSR: HSYNC カウント
        "    mov x, osr        side 0",  // [2]  X = カウント
        "    nop               side 1",  // [3]  アラインメント
        "hsync_v0:",
        "    nop               side 0",  // [4]
        "    jmp x-- hsync_v0  side 1",  // [5]  X+1 回ループ
        "",
        "    set pins, 1       side 0",  // [6]  HSYNC=1, VSYNC=0
        "    pull block        side 1",  // [7]  FIFO→OSR: 残りカウント
        "    mov x, osr        side 0",  // [8]
        "    nop               side 1",  // [9]
        "rest_v0:",
        "    nop               side 0",  // [10]
        "    jmp x-- rest_v0   side 1",  // [11]
        "",
        // === 通常ラインカウントロード ===
        "    pull block        side 0",  // [12] FIFO→OSR: 通常ライン数-1
        "    mov y, osr        side 1",  // [13] Y = ライン数-1
        "",
        // === 通常ラインループ ===
        "normal_line:",
        "    set pins, 2       side 0",  // [14] HSYNC=0, VSYNC=1
        "    pull block        side 1",  // [15] FIFO→OSR: HSYNC カウント
        "    mov x, osr        side 0",  // [16]
        "    nop               side 1",  // [17]
        "hsync_v1:",
        "    nop               side 0",  // [18]
        "    jmp x-- hsync_v1  side 1",  // [19]
        "",
        "    set pins, 3       side 0",  // [20] HSYNC=1, VSYNC=1
        "    pull block        side 1",  // [21] FIFO→OSR: 残りカウント
        "    mov x, osr        side 0",  // [22]
        "    nop               side 1",  // [23]
        "rest_v1:",
        "    nop               side 0",  // [24]
        "    jmp x-- rest_v1   side 1",  // [25]
        "",
        "    nop               side 0",  // [26] NCLK アラインメント
        "    jmp y-- normal_line side 1", // [27] 次のライン (Y=0 で wrap)
        ".wrap",
    );

    let Pio {
        mut common,
        sm0: mut sm,
        ..
    } = Pio::new(p.PIO0, Irqs);

    // ピン設定
    let nclk_pin = common.make_pio_pin(p.PIN_20);  // NCLK (sideset)
    let hsync_pin = common.make_pio_pin(p.PIN_21);  // HSYNC (set bit0)
    let vsync_pin = common.make_pio_pin(p.PIN_22);  // VSYNC (set bit1)

    // ピン方向を出力に設定
    sm.set_pin_dirs(Direction::Out, &[&nclk_pin, &hsync_pin, &vsync_pin]);

    let mut cfg = Config::default();
    // sideset = NCLK (1 pin)
    cfg.use_program(&common.load_program(&prg.program), &[&nclk_pin]);
    // set pins = HSYNC + VSYNC (2 pins, 連続 GP21-GP22)
    cfg.set_set_pins(&[&hsync_pin, &vsync_pin]);

    // クロック分周: 21.796875
    // 実際の NCLK = 150MHz / (21.796875 × 2) ≈ 3.44 MHz
    cfg.clock_divider = FixedU32::<U8>::from_bits(
        (PIO_CLK_DIV_INT as u32) << 8 | PIO_CLK_DIV_FRAC as u32,
    );

    cfg.fifo_join = FifoJoin::TxOnly;  // TX FIFO を 8 エントリに拡張

    sm.set_config(&cfg);

    // --- FIFO 事前充填（NCLKグリッチ防止） ---
    // プレフィル: VSYNC(2) + Y(1) + Normal1(2) = 5値 → TxOnly (8エントリ) で安全
    sm.tx().push(PIO_HSYNC_COUNT);        // VSYNCライン HSYNCパルス
    sm.tx().push(PIO_REST_COUNT);         // VSYNCライン 残り
    sm.tx().push(PIO_NORMAL_LINES_COUNT); // 通常ライン数 (Y register)
    sm.tx().push(PIO_HSYNC_COUNT);        // 通常ライン1行目 HSYNC
    sm.tx().push(PIO_REST_COUNT);         // 通常ライン1行目 残り

    sm.set_enable(true);

    info!("PIO started, feeding timing data...");
    info!("HSYNC count: {}, Rest count: {}, Normal lines Y: {}",
          PIO_HSYNC_COUNT, PIO_REST_COUNT, PIO_NORMAL_LINES_COUNT);
    info!("Expected: 512 NCLK/line, 112 lines/frame, ~60 fps");

    let mut frame_count: u32 = 0;

    loop {
        // === 残りの通常ライン (line 2 ~ V_NORMAL_LINES) ===
        for _ in 1..V_NORMAL_LINES {
            sm.tx().wait_push(PIO_HSYNC_COUNT).await;
            sm.tx().wait_push(PIO_REST_COUNT).await;
        }

        // === 次フレームの VSYNC データ ===
        sm.tx().wait_push(PIO_HSYNC_COUNT).await;
        sm.tx().wait_push(PIO_REST_COUNT).await;
        sm.tx().wait_push(PIO_NORMAL_LINES_COUNT).await;

        // === 次フレームの通常ライン1行目 ===
        sm.tx().wait_push(PIO_HSYNC_COUNT).await;
        sm.tx().wait_push(PIO_REST_COUNT).await;

        frame_count = frame_count.wrapping_add(1);
        if frame_count % (60 * 3) == 0 {
            info!("Frame {}", frame_count);
        }
    }
}
