//! Layer 7: 全フレーム一括DMA転送方式
//!
//! SM0（ピクセル+NCLK）とSM1（HSYNC/VSYNC）をそれぞれ別DMAチャネルで
//! フレーム全体を一括転送し、行間のCPU介入を排除してジッターを解消する。
//!
//! # 合格基準
//! - [ ] SM0/SM1 両DMAを MULTI_CHAN_TRIGGER で完全同時起動
//! - [ ] 行間の CPU 介入がない (ジッタフリー)
//! - [ ] テキスト・図形が正しく表示される
//! - [ ] ティアリングがない

#![no_std]
#![no_main]

use core::ptr::addr_of_mut;
use core::sync::atomic::{AtomicU32, Ordering};
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
use embassy_time::Instant;
use embedded_graphics::geometry::Size;
use embedded_graphics::mono_font::ascii::FONT_6X10;
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::pixelcolor::Rgb666;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{Circle, Line, PrimitiveStyle, Rectangle};
use embedded_graphics::text::Text;
use fixed::FixedU32;
use fixed::types::extra::U8;
use pico2w_300yen_lcd::lcd::framebuffer::*;
use pico2w_300yen_lcd::lcd::timing::*;
use {defmt_rtt as _, panic_probe as _};

bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => InterruptHandler<PIO0>;
});

// ============================================================
// SM1 フレームデータ (static 配置)
// ============================================================

/// SM1 の 1 フレーム分タイミングデータ (225 ワード)
static SM1_FRAME_DATA: [u32; SM1_FRAME_SIZE] = sm1_frame_data();

// CH2/CH3 が各フレーム終端で読み、CH0/CH1 の読み出し先を再設定する。
static DMA_PIXEL_FRAME_ADDR: AtomicU32 = AtomicU32::new(0);
static DMA_TIMING_FRAME_ADDR: AtomicU32 = AtomicU32::new(0);

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
    dma_ch1: Peri<'static, DMA_CH1>,
    dma_ch2: Peri<'static, DMA_CH2>,
    dma_ch3: Peri<'static, DMA_CH3>,
}

// ============================================================
// ダブルバッファ管理
// ============================================================

/// フレームバッファ A (BSS 配置、ゼロ初期化)
static mut FB_A_DATA: FrameBuffer = FrameBuffer::new();

/// フレームバッファ B (BSS 配置、ゼロ初期化)
static mut FB_B_DATA: FrameBuffer = FrameBuffer::new();

/// main → display_task: 描画完了した back バッファの所有権を送信
static SWAP_CH: Channel<CriticalSectionRawMutex, &'static mut FrameBuffer, 1> = Channel::new();

/// display_task → main: 使用済み front バッファの所有権を返却
static RETURN_CH: Channel<CriticalSectionRawMutex, &'static mut FrameBuffer, 1> = Channel::new();

// ============================================================
// 計測統計 (lock-free)
// ============================================================

/// main 描画処理時間（CPUのみ）[ms] 最新値
static DRAW_CPU_MS_LATEST: AtomicU32 = AtomicU32::new(0);

/// main 描画処理時間（CPUのみ）[ms] 最大値
static DRAW_CPU_MS_MAX: AtomicU32 = AtomicU32::new(0);

/// main フレームループ全体時間（描画+待機）[ms] 最新値
static FRAME_LOOP_MS_LATEST: AtomicU32 = AtomicU32::new(0);

/// main フレームループ全体時間（描画+待機）[ms] 最大値
static FRAME_LOOP_MS_MAX: AtomicU32 = AtomicU32::new(0);

/// display_task フレーム境界処理時間 [us] 最新値
static DISPLAY_BOUNDARY_SECTION_US_LATEST: AtomicU32 = AtomicU32::new(0);

/// display_task フレーム境界処理時間 [us] 最大値
static DISPLAY_BOUNDARY_SECTION_US_MAX: AtomicU32 = AtomicU32::new(0);
/// PIO が空の TX FIFO を待って停止したフレーム数（起動時の停止は除外）
static SM0_TX_STALL_FRAMES: AtomicU32 = AtomicU32::new(0);
static SM1_TX_STALL_FRAMES: AtomicU32 = AtomicU32::new(0);

const HUD_X: i32 = 230;
const HUD_LINE1_Y: i32 = 12;
const HUD_LINE2_Y: i32 = 24;
const HUD_LINE3_Y: i32 = 36;
const HUD_LINE4_Y: i32 = 48;
const HUD_UPDATE_INTERVAL_FRAMES: u32 = 8;

fn saturating_u64_to_u32(v: u64) -> u32 {
    if v > u32::MAX as u64 {
        u32::MAX
    } else {
        v as u32
    }
}

fn update_max_atomic(max: &AtomicU32, value: u32) {
    let mut observed = max.load(Ordering::Relaxed);
    while value > observed {
        match max.compare_exchange_weak(observed, value, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(next_observed) => observed = next_observed,
        }
    }
}

fn write_u32_decimal<const N: usize>(mut value: u32, buf: &mut [u8; N]) -> &str {
    let mut idx = N;
    if value == 0 {
        idx -= 1;
        buf[idx] = b'0';
    } else {
        while value > 0 {
            idx -= 1;
            buf[idx] = b'0' + (value % 10) as u8;
            value /= 10;
        }
    }

    // Safety: buf[idx..] はASCII数字のみで構成される
    unsafe { core::str::from_utf8_unchecked(&buf[idx..]) }
}

// ============================================================
// display_task: 全フレーム一括 DMA 転送
// ============================================================

#[embassy_executor::task]
async fn display_task(
    res: DisplayPeripherals,
    mut front: &'static mut FrameBuffer,
) {
    // DMA CH0/CH1 の所有権を保持（embassy による二重使用を防止）
    // PAC 直接操作で初回同時起動し、その後は DMA チェインで再起動するため
    // embassy API では使用しない
    let _dma_ch0 = res.dma_ch0;
    let _dma_ch1 = res.dma_ch1;
    let _dma_ch2 = res.dma_ch2;
    let _dma_ch3 = res.dma_ch3;

    // === SM0: ピクセル出力 + NCLK (sideset) ===
    // 2命令反転版: side 1 でデータセットアップ、side 0 の立ち下がりでLCDサンプル
    let prg_pixel = pio_asm!(
        ".side_set 1",
        ".wrap_target",
        "    out pins, 18  side 1",   // データ出力 + NCLK HIGH（セットアップ期間）
        "    nop           side 0",   // NCLK LOW（立ち下がりでLCDサンプル）
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

    // 両 SM を同時に開始
    common.apply_sm_batch(|batch| {
        batch.set_enable(&mut sm0, true);
        batch.set_enable(&mut sm1, true);
    });

    // DMA 書き込み先アドレス (PIO0 TX FIFO) — ループ中不変
    let sm0_txf_addr = embassy_rp::pac::PIO0.txf(0).as_ptr() as u32;
    let sm1_txf_addr = embassy_rp::pac::PIO0.txf(1).as_ptr() as u32;
    DMA_PIXEL_FRAME_ADDR.store(front.frame_data().as_ptr() as u32, Ordering::SeqCst);
    DMA_TIMING_FRAME_ADDR.store(SM1_FRAME_DATA.as_ptr() as u32, Ordering::SeqCst);

    // PAC で直接管理する4チャネルの完了フラグは display_task がポーリングする。
    let dma = embassy_rp::pac::DMA;
    dma.inte(0).write_value(dma.inte(0).read() & !0b1111);
    dma.intr(0).write_value(0b1111);

    // === 初回 DMA 設定: WRITE_ADDR と CTRL は固定のため一度だけ設定 ===
    {
        let dma = embassy_rp::pac::DMA;

        // --- CH0 (SM0: ピクセルデータ) WRITE_ADDR + CTRL ---
        let ch0 = dma.ch(0);
        ch0.write_addr().write_value(sm0_txf_addr);
        // al1_ctrl: 非トリガーエイリアス — 書き込んでもDMA起動しない
        {
            let mut ctrl = embassy_rp::pac::dma::regs::CtrlTrig(0);
            ctrl.set_en(true);
            ctrl.set_data_size(embassy_rp::pac::dma::vals::DataSize::SIZE_WORD);
            ctrl.set_incr_read(true);
            ctrl.set_incr_write(false);
            ctrl.set_treq_sel(embassy_rp::pac::dma::vals::TreqSel::PIO0_TX0);
            ctrl.set_chain_to(2); // CH0 完了後、CH2 が次フレームを起動
            ch0.al1_ctrl().write_value(ctrl.0);
        }

        // --- CH1 (SM1: HSYNC/VSYNC タイミング) WRITE_ADDR + CTRL ---
        let ch1 = dma.ch(1);
        ch1.write_addr().write_value(sm1_txf_addr);
        {
            let mut ctrl = embassy_rp::pac::dma::regs::CtrlTrig(0);
            ctrl.set_en(true);
            ctrl.set_data_size(embassy_rp::pac::dma::vals::DataSize::SIZE_WORD);
            ctrl.set_incr_read(true);
            ctrl.set_incr_write(false);
            ctrl.set_treq_sel(embassy_rp::pac::dma::vals::TreqSel::PIO0_TX1);
            ctrl.set_chain_to(3); // CH1 完了後、CH3 が次フレームを起動
            ch1.al1_ctrl().write_value(ctrl.0);
        }

        // CH2: ピクセルフレーム先頭アドレスを CH0 のトリガー別名へ1ワード転送。
        // 転送数はハードウェアの RELOAD 値から毎回復元される。
        let ch2 = dma.ch(2);
        ch2.read_addr()
            .write_value(DMA_PIXEL_FRAME_ADDR.as_ptr() as u32);
        ch2.write_addr()
            .write_value(ch0.al3_read_addr_trig().as_ptr() as u32);
        ch2.trans_count().write(|w| w.set_count(1));
        {
            let mut ctrl = embassy_rp::pac::dma::regs::CtrlTrig(0);
            ctrl.set_en(true);
            ctrl.set_data_size(embassy_rp::pac::dma::vals::DataSize::SIZE_WORD);
            ctrl.set_incr_read(false);
            ctrl.set_incr_write(false);
            ctrl.set_treq_sel(embassy_rp::pac::dma::vals::TreqSel::PERMANENT);
            ctrl.set_chain_to(2); // 自CHへのチェインは無効
            ch2.al1_ctrl().write_value(ctrl.0);
        }

        // CH3: 同期フレーム先頭アドレスを CH1 のトリガー別名へ1ワード転送。
        let ch3 = dma.ch(3);
        ch3.read_addr()
            .write_value(DMA_TIMING_FRAME_ADDR.as_ptr() as u32);
        ch3.write_addr()
            .write_value(ch1.al3_read_addr_trig().as_ptr() as u32);
        ch3.trans_count().write(|w| w.set_count(1));
        {
            let mut ctrl = embassy_rp::pac::dma::regs::CtrlTrig(0);
            ctrl.set_en(true);
            ctrl.set_data_size(embassy_rp::pac::dma::vals::DataSize::SIZE_WORD);
            ctrl.set_incr_read(false);
            ctrl.set_incr_write(false);
            ctrl.set_treq_sel(embassy_rp::pac::dma::vals::TreqSel::PERMANENT);
            ctrl.set_chain_to(3); // 自CHへのチェインは無効
            ch3.al1_ctrl().write_value(ctrl.0);
        }
    }

    // === フレームループ: 初回のみ同時起動し、以後は DMA チェインで連続供給 ===
    // CH0→CH2→CH0、CH1→CH3→CH1 とハードウェアで再起動する。
    // CPU はフレーム境界の転送再設定に関与しない。
    // 初回フレーム起動（クリティカルセクション内）
    cortex_m::interrupt::free(|_| {
        let dma = embassy_rp::pac::DMA;

        let ch0 = dma.ch(0);
        ch0.read_addr()
            .write_value(front.frame_data().as_ptr() as u32);
        ch0.trans_count().write(|w| {
            w.set_count(front.frame_data().len() as u32);
        });

        let ch1 = dma.ch(1);
        ch1.read_addr()
            .write_value(SM1_FRAME_DATA.as_ptr() as u32);
        ch1.trans_count().write(|w| {
            w.set_count(SM1_FRAME_SIZE as u32);
        });

        dma.multi_chan_trigger().write(|w| {
            w.set_multi_chan_trigger(0b11);
        });
    });

    let mut first_stall_sample = true;
    let mut staged_front: Option<&'static mut FrameBuffer> = None;
    loop {
        // CH0 の完了フラグを待つ。CH2 は既に次フレームを起動している。
        let dma = embassy_rp::pac::DMA;
        loop {
            if dma.intr(0).read() & 0b1 != 0 {
                dma.intr(0).write_value(0b1);
                break;
            }
            embassy_futures::yield_now().await;
        }

        let boundary_start = Instant::now();

        // 前の境界で予約したバッファは、今回の CH2 再起動で使用開始済み。
        if let Some(new_front) = staged_front.take() {
            let old_front = core::mem::replace(&mut front, new_front);
            RETURN_CH.send(old_front).await;
        }

        // 次のフレーム境界で CH2 が使うポインタを公開する。
        if let Ok(new_front) = SWAP_CH.try_receive() {
            DMA_PIXEL_FRAME_ADDR.store(new_front.frame_data().as_ptr() as u32, Ordering::SeqCst);
            staged_front = Some(new_front);
        }

        // FDEBUG.TXSTALL は write-one-to-clear。初回は SM 起動時の空 FIFO による
        // 停止を含むため集計せず、以後はフレームごとに停止の有無を数える。
        let fdebug = embassy_rp::pac::PIO0.fdebug();
        let stalled = fdebug.read().txstall() & 0b11;
        if stalled != 0 {
            fdebug.write(|w| w.set_txstall(stalled));
        }
        if !first_stall_sample {
            if stalled & 0b01 != 0 {
                SM0_TX_STALL_FRAMES.fetch_add(1, Ordering::Relaxed);
            }
            if stalled & 0b10 != 0 {
                SM1_TX_STALL_FRAMES.fetch_add(1, Ordering::Relaxed);
            }
        }
        first_stall_sample = false;
        let boundary_us = saturating_u64_to_u32(boundary_start.elapsed().as_micros());
        DISPLAY_BOUNDARY_SECTION_US_LATEST.store(boundary_us, Ordering::Relaxed);
        update_max_atomic(&DISPLAY_BOUNDARY_SECTION_US_MAX, boundary_us);
    }
}

// ============================================================
// main: embedded-graphics 描画タスク
// ============================================================

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());

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
                dma_ch1: p.DMA_CH1,
                dma_ch2: p.DMA_CH2,
                dma_ch3: p.DMA_CH3,
            },
            fb_a,
        ))
        .unwrap();

    // メインタスク: embedded-graphics で描画
    let mut back: &'static mut FrameBuffer = fb_b;
    let mut hud_div: u32 = 0;

    let text_style = MonoTextStyle::new(&FONT_6X10, Rgb666::WHITE);

    let mut draw_cpu_latest_buf = [0u8; 10];
    let mut draw_cpu_max_buf = [0u8; 10];
    let mut frame_loop_latest_buf = [0u8; 10];
    let mut frame_loop_max_buf = [0u8; 10];
    let mut sm0_stall_buf = [0u8; 10];
    let mut sm1_stall_buf = [0u8; 10];

    let mut draw_cpu_latest_text: &str = "0";
    let mut draw_cpu_max_text: &str = "0";
    let mut frame_loop_latest_text: &str = "0";
    let mut frame_loop_max_text: &str = "0";
    let mut sm0_stall_text: &str = "0";
    let mut sm1_stall_text: &str = "0";

    loop {
        let frame_loop_start = Instant::now();

        if hud_div == 0 {
            let draw_cpu_latest = DRAW_CPU_MS_LATEST.load(Ordering::Relaxed);
            let draw_cpu_max = DRAW_CPU_MS_MAX.load(Ordering::Relaxed);
            let frame_loop_latest = FRAME_LOOP_MS_LATEST.load(Ordering::Relaxed);
            let frame_loop_max = FRAME_LOOP_MS_MAX.load(Ordering::Relaxed);

            draw_cpu_latest_text = write_u32_decimal(draw_cpu_latest, &mut draw_cpu_latest_buf);
            draw_cpu_max_text = write_u32_decimal(draw_cpu_max, &mut draw_cpu_max_buf);
            frame_loop_latest_text = write_u32_decimal(frame_loop_latest, &mut frame_loop_latest_buf);
            frame_loop_max_text = write_u32_decimal(frame_loop_max, &mut frame_loop_max_buf);
            sm0_stall_text = write_u32_decimal(
                SM0_TX_STALL_FRAMES.load(Ordering::Relaxed),
                &mut sm0_stall_buf,
            );
            sm1_stall_text = write_u32_decimal(
                SM1_TX_STALL_FRAMES.load(Ordering::Relaxed),
                &mut sm1_stall_buf,
            );
        }

        let draw_cpu_start = Instant::now();

        DrawTarget::clear(back, Rgb666::BLACK).unwrap();

        // テキスト描画
        Text::new(
            "Hello, 300yen LCD!",
            embedded_graphics::geometry::Point::new(10, 12),
            text_style,
        )
        .draw(back)
        .unwrap();

        Text::new(
            "Full-frame DMA",
            embedded_graphics::geometry::Point::new(10, 26),
            text_style,
        )
        .draw(back)
        .unwrap();

        // 赤い矩形
        Rectangle::new(
            embedded_graphics::geometry::Point::new(10, 35),
            Size::new(60, 30),
        )
        .into_styled(PrimitiveStyle::with_fill(Rgb666::RED))
        .draw(back)
        .unwrap();

        // 緑の円
        Circle::new(embedded_graphics::geometry::Point::new(90, 35), 30)
            .into_styled(PrimitiveStyle::with_fill(Rgb666::GREEN))
            .draw(back)
            .unwrap();

        // 青い矩形
        Rectangle::new(
            embedded_graphics::geometry::Point::new(140, 35),
            Size::new(60, 30),
        )
        .into_styled(PrimitiveStyle::with_fill(Rgb666::BLUE))
        .draw(back)
        .unwrap();

        // 白い線
        Line::new(
            embedded_graphics::geometry::Point::new(10, 75),
            embedded_graphics::geometry::Point::new(390, 75),
        )
        .into_styled(PrimitiveStyle::with_stroke(Rgb666::WHITE, 1))
        .draw(back)
        .unwrap();

        // カラーバー
        let colors = [
            Rgb666::WHITE,
            Rgb666::new(63, 63, 0),
            Rgb666::new(0, 63, 63),
            Rgb666::GREEN,
            Rgb666::new(63, 0, 63),
            Rgb666::RED,
            Rgb666::BLUE,
            Rgb666::BLACK,
        ];
        for (i, &color) in colors.iter().enumerate() {
            let x = i as i32 * 50;
            Rectangle::new(
                embedded_graphics::geometry::Point::new(x, 80),
                Size::new(50, 16),
            )
            .into_styled(PrimitiveStyle::with_fill(color))
            .draw(back)
            .unwrap();
        }

        // 計測 HUD (右上固定、表示内容は間引き更新)
        Text::new(
            "CPUms L:",
            embedded_graphics::geometry::Point::new(HUD_X, HUD_LINE1_Y),
            text_style,
        )
        .draw(back)
        .unwrap();
        Text::new(
            draw_cpu_latest_text,
            embedded_graphics::geometry::Point::new(HUD_X + 45, HUD_LINE1_Y),
            text_style,
        )
        .draw(back)
        .unwrap();
        Text::new(
            " M:",
            embedded_graphics::geometry::Point::new(HUD_X + 75, HUD_LINE1_Y),
            text_style,
        )
        .draw(back)
        .unwrap();
        Text::new(
            draw_cpu_max_text,
            embedded_graphics::geometry::Point::new(HUD_X + 93, HUD_LINE1_Y),
            text_style,
        )
        .draw(back)
        .unwrap();

        Text::new(
            "FRMms L:",
            embedded_graphics::geometry::Point::new(HUD_X, HUD_LINE2_Y),
            text_style,
        )
        .draw(back)
        .unwrap();
        Text::new(
            frame_loop_latest_text,
            embedded_graphics::geometry::Point::new(HUD_X + 45, HUD_LINE2_Y),
            text_style,
        )
        .draw(back)
        .unwrap();
        Text::new(
            " M:",
            embedded_graphics::geometry::Point::new(HUD_X + 75, HUD_LINE2_Y),
            text_style,
        )
        .draw(back)
        .unwrap();
        Text::new(
            frame_loop_max_text,
            embedded_graphics::geometry::Point::new(HUD_X + 93, HUD_LINE2_Y),
            text_style,
        )
        .draw(back)
        .unwrap();

        Text::new(
            "SM0 stalls:",
            embedded_graphics::geometry::Point::new(HUD_X, HUD_LINE3_Y),
            text_style,
        )
        .draw(back)
        .unwrap();
        Text::new(
            sm0_stall_text,
            embedded_graphics::geometry::Point::new(HUD_X + 66, HUD_LINE3_Y),
            text_style,
        )
        .draw(back)
        .unwrap();
        Text::new(
            "SM1 stalls:",
            embedded_graphics::geometry::Point::new(HUD_X, HUD_LINE4_Y),
            text_style,
        )
        .draw(back)
        .unwrap();
        Text::new(
            sm1_stall_text,
            embedded_graphics::geometry::Point::new(HUD_X + 66, HUD_LINE4_Y),
            text_style,
        )
        .draw(back)
        .unwrap();

        let draw_cpu_ms = saturating_u64_to_u32(draw_cpu_start.elapsed().as_millis());
        DRAW_CPU_MS_LATEST.store(draw_cpu_ms, Ordering::Relaxed);
        update_max_atomic(&DRAW_CPU_MS_MAX, draw_cpu_ms);

        SWAP_CH.send(back).await;
        back = RETURN_CH.receive().await;

        let frame_loop_ms = saturating_u64_to_u32(frame_loop_start.elapsed().as_millis());
        FRAME_LOOP_MS_LATEST.store(frame_loop_ms, Ordering::Relaxed);
        update_max_atomic(&FRAME_LOOP_MS_MAX, frame_loop_ms);

        hud_div = (hud_div + 1) % HUD_UPDATE_INTERVAL_FRAMES;
    }
}
