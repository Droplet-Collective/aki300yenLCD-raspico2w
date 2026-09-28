//! Layer 7: シングルバッファ全フレームDMA転送方式
//!
//! SM0（ピクセル+NCLK）とSM1（HSYNC/VSYNC）をそれぞれ別DMAチャネルで
//! フレーム全体を一括転送する。描画はDMA起動前に一度だけ行い、
//! 以後は同じ読み取り専用バッファを60Hzで連続走査する。
//! 動的描画を行うダブルバッファ版は layer7_double_buffer_dma に保存している。

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
use embassy_time::Timer;
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
// シングルバッファ
// ============================================================

/// フレームバッファ (BSS 配置、ゼロ初期化)
static mut FB_DATA: FrameBuffer = FrameBuffer::new();

// ============================================================
// display_task: 全フレーム一括 DMA 転送
// ============================================================

#[embassy_executor::task]
async fn display_task(
    res: DisplayPeripherals,
    frame: &'static FrameBuffer,
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
    DMA_PIXEL_FRAME_ADDR.store(frame.frame_data().as_ptr() as u32, Ordering::SeqCst);
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
            .write_value(frame.frame_data().as_ptr() as u32);
        ch0.trans_count().write(|w| {
            w.set_count(frame.frame_data().len() as u32);
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

    // DMA連鎖が走査を続ける間、CPUは100msごとにTXSTALLだけを確認する。
    // 起動直後の空FIFOによる停止を含む最初の標本は除外する。
    let mut first_stall_sample = true;
    loop {
        Timer::after_millis(100).await;
        let fdebug = embassy_rp::pac::PIO0.fdebug();
        let stalled = fdebug.read().txstall() & 0b11;
        if stalled != 0 {
            fdebug.write(|w| w.set_txstall(stalled));
        }
        if !first_stall_sample && stalled != 0 {
            defmt::warn!("PIO TXSTALL: SM0={} SM1={}", stalled & 1, (stalled >> 1) & 1);
        }
        first_stall_sample = false;
    }
}

// ============================================================
// main: 起動時に1回だけ描画
// ============================================================

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());

    // Safety: display_task 起動前は main だけがこのバッファにアクセスする。
    // 描画後に不変参照として渡し、以後CPUは書き換えない。
    let frame: &'static mut FrameBuffer = unsafe { &mut *addr_of_mut!(FB_DATA) };
    draw_test_pattern(frame);
    let frame: &'static FrameBuffer = frame;

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
            frame,
        ))
        .unwrap();

    core::future::pending::<()>().await;
}

fn draw_test_pattern(frame: &mut FrameBuffer) {
    let text_style = MonoTextStyle::new(&FONT_6X10, Rgb666::WHITE);
    DrawTarget::clear(frame, Rgb666::BLACK).unwrap();

    Text::new(
        "Hello, 300yen LCD!",
        embedded_graphics::geometry::Point::new(10, 12),
        text_style,
    )
    .draw(frame)
    .unwrap();
    Text::new(
        "Single-buffer DMA",
        embedded_graphics::geometry::Point::new(10, 26),
        text_style,
    )
    .draw(frame)
    .unwrap();

    Rectangle::new(
        embedded_graphics::geometry::Point::new(10, 35),
        Size::new(60, 30),
    )
    .into_styled(PrimitiveStyle::with_fill(Rgb666::RED))
    .draw(frame)
    .unwrap();
    Circle::new(embedded_graphics::geometry::Point::new(90, 35), 30)
        .into_styled(PrimitiveStyle::with_fill(Rgb666::GREEN))
        .draw(frame)
        .unwrap();
    Rectangle::new(
        embedded_graphics::geometry::Point::new(140, 35),
        Size::new(60, 30),
    )
    .into_styled(PrimitiveStyle::with_fill(Rgb666::BLUE))
    .draw(frame)
    .unwrap();
    Line::new(
        embedded_graphics::geometry::Point::new(10, 75),
        embedded_graphics::geometry::Point::new(390, 75),
    )
    .into_styled(PrimitiveStyle::with_stroke(Rgb666::WHITE, 1))
    .draw(frame)
    .unwrap();

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
        Rectangle::new(
            embedded_graphics::geometry::Point::new(i as i32 * 50, 80),
            Size::new(50, 16),
        )
        .into_styled(PrimitiveStyle::with_fill(color))
        .draw(frame)
        .unwrap();
    }

    Text::new(
        "Single buffer",
        embedded_graphics::geometry::Point::new(230, 12),
        text_style,
    )
    .draw(frame)
    .unwrap();
    Text::new(
        "60Hz DMA scan",
        embedded_graphics::geometry::Point::new(230, 26),
        text_style,
    )
    .draw(frame)
    .unwrap();
    Text::new(
        "Static image",
        embedded_graphics::geometry::Point::new(230, 40),
        text_style,
    )
    .draw(frame)
    .unwrap();
}
