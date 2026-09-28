//! SD カードの IMAGE.BMP を単一フレームバッファへ読み込み、LCD に表示する。
//!
//! SD は基板配線に合わせた GPIO SPI（CS=GP26, CMD=GP27, CLK=GP28,
//! DAT0=GP0）。FAT ボリュームのルートにある非圧縮 24-bit BMP を読む。
//! 読み込みを終えてから全フレーム DMA を起動するため、表示中のバッファは
//! CPU から書き換えない。

#![no_std]
#![no_main]

use core::convert::Infallible;
use core::ptr::addr_of_mut;
use core::sync::atomic::{AtomicU32, Ordering};
use embassy_executor::Spawner;
use embassy_rp::Peri;
use embassy_rp::bind_interrupts;
use embassy_rp::gpio::{Input, Level, Output, Pull};
use embassy_rp::peripherals::*;
use embassy_rp::pio::program::pio_asm;
use embassy_rp::pio::{
    Config, Direction, FifoJoin, InterruptHandler, Pio, ShiftConfig, ShiftDirection,
};
use embassy_time::Delay;
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::mono_font::ascii::FONT_6X10;
use embedded_graphics::pixelcolor::Rgb666;
use embedded_graphics::prelude::*;
use embedded_graphics::text::Text;
use embedded_hal::delay::DelayNs;
use embedded_hal::spi::{ErrorType, Operation, SpiDevice};
use embedded_sdmmc::{File, Mode, SdCard, TimeSource, Timestamp, VolumeIdx, VolumeManager};
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

// SD の SPI 配線は KiCad 基板の SD_DAT0/SD_CS/SD_CMD/SD_CLK と Pico 2W の
// パッド 1/31/32/34 を参照。ハードウェア SPI のピン組み合わせではない。
struct BitBangSd {
    miso: Input<'static>,
    cs: Output<'static>,
    mosi: Output<'static>,
    clk: Output<'static>,
    slow: bool,
}

impl BitBangSd {
    fn new(
        miso: Peri<'static, PIN_0>,
        cs: Peri<'static, PIN_26>,
        mosi: Peri<'static, PIN_27>,
        clk: Peri<'static, PIN_28>,
    ) -> Self {
        let mut bus = Self {
            miso: Input::new(miso, Pull::Up),
            cs: Output::new(cs, Level::High),
            mosi: Output::new(mosi, Level::High),
            clk: Output::new(clk, Level::Low),
            slow: true,
        };
        // SD SPI モードへ入る前に CS=High, CMD=High で 80 クロック。
        for _ in 0..10 {
            bus.transfer_byte(0xff);
        }
        // embedded-sdmmc はコマンドと応答を別々の SpiDevice 呼び出しで
        // 送受信する。共有バスはないので、読み終わるまで CS を保持する。
        bus.cs.set_low();
        bus.transfer_byte(0xff);
        bus
    }

    #[inline]
    fn half_clock_delay(&self) {
        if self.slow {
            let mut delay = Delay;
            delay.delay_us(2); // 初期化中は 400 kHz 以下
        }
    }

    fn transfer_byte(&mut self, out: u8) -> u8 {
        let mut input = 0u8;
        for bit in (0..8).rev() {
            self.clk.set_low();
            if out & (1 << bit) != 0 {
                self.mosi.set_high();
            } else {
                self.mosi.set_low();
            }
            self.half_clock_delay();
            self.clk.set_high();
            self.half_clock_delay();
            input = (input << 1) | u8::from(self.miso.is_high());
        }
        self.clk.set_low();
        input
    }
}

impl Drop for BitBangSd {
    fn drop(&mut self) {
        self.cs.set_high();
        self.clk.set_low();
    }
}

impl ErrorType for BitBangSd {
    type Error = Infallible;
}

impl SpiDevice<u8> for BitBangSd {
    fn transaction(&mut self, operations: &mut [Operation<'_, u8>]) -> Result<(), Self::Error> {
        for operation in operations {
            match operation {
                Operation::Read(read) => {
                    for byte in read.iter_mut() {
                        *byte = self.transfer_byte(0xff);
                    }
                }
                Operation::Write(write) => {
                    for &byte in write.iter() {
                        self.transfer_byte(byte);
                    }
                }
                Operation::Transfer(read, write) => {
                    for i in 0..read.len().max(write.len()) {
                        let byte = self.transfer_byte(write.get(i).copied().unwrap_or(0xff));
                        if let Some(slot) = read.get_mut(i) {
                            *slot = byte;
                        }
                    }
                }
                Operation::TransferInPlace(data) => {
                    for byte in data.iter_mut() {
                        *byte = self.transfer_byte(*byte);
                    }
                }
                Operation::DelayNs(ns) => {
                    let mut delay = Delay;
                    delay.delay_ns(*ns);
                }
            }
        }
        Ok(())
    }
}

struct FixedTime;

impl TimeSource for FixedTime {
    fn get_timestamp(&self) -> Timestamp {
        // 読み取り専用なのでタイムスタンプは使わない。
        Timestamp {
            year_since_1970: 10,
            zero_indexed_month: 0,
            zero_indexed_day: 0,
            hours: 0,
            minutes: 0,
            seconds: 0,
        }
    }
}

type SdFile<'a> = File<'a, SdCard<BitBangSd, Delay>, FixedTime, 4, 4, 1>;

fn load_image(
    frame: &mut FrameBuffer,
    miso: Peri<'static, PIN_0>,
    cs: Peri<'static, PIN_26>,
    mosi: Peri<'static, PIN_27>,
    clk: Peri<'static, PIN_28>,
) -> Result<(u32, u32), &'static str> {
    let sdcard = SdCard::new(BitBangSd::new(miso, cs, mosi, clk), Delay);
    sdcard.num_bytes().map_err(|_| "SD INIT FAILED")?;
    sdcard.spi(|bus| bus.slow = false);

    let volume_mgr = VolumeManager::new(sdcard, FixedTime);
    let volume = volume_mgr
        .open_volume(VolumeIdx(0))
        .map_err(|_| "FAT VOLUME ERROR")?;
    let root = volume.open_root_dir().map_err(|_| "ROOT DIR ERROR")?;
    let file = root
        .open_file_in_dir("IMAGE.BMP", Mode::ReadOnly)
        .map_err(|_| "IMAGE.BMP MISSING")?;
    draw_bmp(frame, &file)
}

fn read_exact(file: &SdFile<'_>, mut bytes: &mut [u8]) -> Result<(), &'static str> {
    while !bytes.is_empty() {
        let count = file.read(bytes).map_err(|_| "SD READ ERROR")?;
        if count == 0 {
            return Err("TRUNCATED BMP");
        }
        bytes = &mut bytes[count..];
    }
    Ok(())
}

fn draw_bmp(frame: &mut FrameBuffer, file: &SdFile<'_>) -> Result<(u32, u32), &'static str> {
    let file_len = file.length();
    if file_len < 54 {
        return Err("BAD BMP HEADER");
    }
    let mut header = [0u8; 54];
    read_exact(file, &mut header)?;
    if &header[0..2] != b"BM" {
        return Err("NOT A BMP FILE");
    }
    let pixel_start = u32::from_le_bytes(header[10..14].try_into().unwrap());
    let dib_size = u32::from_le_bytes(header[14..18].try_into().unwrap());
    let width = i32::from_le_bytes(header[18..22].try_into().unwrap());
    let signed_height = i32::from_le_bytes(header[22..26].try_into().unwrap());
    let planes = u16::from_le_bytes(header[26..28].try_into().unwrap());
    let bits_per_pixel = u16::from_le_bytes(header[28..30].try_into().unwrap());
    let compression = u32::from_le_bytes(header[30..34].try_into().unwrap());
    if dib_size < 40
        || pixel_start < 14u32.saturating_add(dib_size)
        || planes != 1
        || bits_per_pixel != 24
        || compression != 0
        || width <= 0
        || signed_height == 0
        || signed_height == i32::MIN
    {
        return Err("UNSUPPORTED BMP");
    }

    let width = width as u32;
    let height = signed_height.unsigned_abs();
    let row_bytes = width
        .checked_mul(3)
        .and_then(|v| v.checked_add(3))
        .map(|v| v & !3)
        .ok_or("BMP TOO LARGE")?;
    let image_end = row_bytes
        .checked_mul(height)
        .and_then(|v| v.checked_add(pixel_start))
        .ok_or("BMP TOO LARGE")?;
    if image_end > file_len {
        return Err("TRUNCATED BMP");
    }

    let draw_width = width.min(H_ACTIVE) as usize;
    let draw_height = height.min(ACTIVE_HEIGHT as u32) as usize;
    let src_x = (width - draw_width as u32) / 2;
    let src_y = (height - draw_height as u32) / 2;
    let dst_x = (H_ACTIVE as usize - draw_width) / 2;
    let dst_y = (ACTIVE_HEIGHT - draw_height) / 2;
    let mut row = [0u8; 64 * 3];

    for dy in 0..draw_height {
        let source_y = src_y + dy as u32;
        let file_y = if signed_height > 0 {
            height - 1 - source_y // BMP の通常の下から上への格納
        } else {
            source_y // top-down BMP
        };
        let offset = pixel_start + file_y * row_bytes + src_x * 3;
        file.seek_from_start(offset).map_err(|_| "BMP SEEK ERROR")?;

        for x in (0..draw_width).step_by(64) {
            let count = (draw_width - x).min(64);
            read_exact(file, &mut row[..count * 3])?;
            for (dx, bgr) in row[..count * 3].chunks_exact(3).enumerate() {
                frame.set_pixel(
                    dst_x + x + dx,
                    dst_y + dy,
                    rgb666(
                        (bgr[2] >> 2) as u32,
                        (bgr[1] >> 2) as u32,
                        (bgr[0] >> 2) as u32,
                    ),
                );
            }
        }
    }
    Ok((width, height))
}

// ============================================================
// display_task: 全フレーム一括 DMA 転送
// ============================================================

#[embassy_executor::task]
async fn display_task(res: DisplayPeripherals, frame_addr: u32) {
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
        "    out pins, 18  side 1", // データ出力 + NCLK HIGH（セットアップ期間）
        "    nop           side 0", // NCLK LOW（立ち下がりでLCDサンプル）
        ".wrap",
    );

    // === SM1: HSYNC/VSYNC タイミング ===
    let prg_timing = pio_asm!(
        ".wrap_target",
        // VSYNC active line (1 line)
        "    set pins, 0", // HSYNC=0, VSYNC=0
        "    pull block",
        "    mov x, osr",
        "hsync_v0:",
        "    jmp x-- hsync_v0",
        //
        "    set pins, 1", // HSYNC=1, VSYNC=0
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
        "    set pins, 2", // HSYNC=0, VSYNC=1
        "    pull block",
        "    mov x, osr",
        "hsync_v1:",
        "    jmp x-- hsync_v1",
        //
        "    set pins, 3", // HSYNC=1, VSYNC=1
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
    sm0.set_pin_dirs(
        Direction::Out,
        &[
            &pin2, &pin3, &pin4, &pin5, &pin6, &pin7, &pin8, &pin9, &pin10, &pin11, &pin12, &pin13,
            &pin14, &pin15, &pin16, &pin17, &pin18, &pin19, &nclk_pin,
        ],
    );
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
    cfg0.clock_divider =
        FixedU32::<U8>::from_bits((PIO_CLK_DIV_INT as u32) << 8 | PIO_CLK_DIV_FRAC as u32);
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
    DMA_PIXEL_FRAME_ADDR.store(frame_addr, Ordering::SeqCst);
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
        ch0.read_addr().write_value(frame_addr);
        ch0.trans_count().write(|w| {
            w.set_count(FB_SIZE as u32);
        });

        let ch1 = dma.ch(1);
        ch1.read_addr().write_value(SM1_FRAME_DATA.as_ptr() as u32);
        ch1.trans_count().write(|w| {
            w.set_count(SM1_FRAME_SIZE as u32);
        });

        dma.multi_chan_trigger().write(|w| {
            w.set_multi_chan_trigger(0b11);
        });
    });

    // 静止画像なので、DMA 開始後はフレームバッファへ一切書き込まない。
    core::future::pending::<()>().await;
}

// ============================================================
// main: SD 読み込みが完了してからスキャンアウト開始
// ============================================================

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());

    let frame_addr = {
        // Safety: DMA 起動前は main だけが FB_DATA を読み書きする。
        // DMA 起動後はこのバッファへ一切書き込まない。
        let frame = unsafe { &mut *addr_of_mut!(FB_DATA) };
        match load_image(frame, p.PIN_0, p.PIN_26, p.PIN_27, p.PIN_28) {
            Ok((width, height)) => {
                defmt::info!("IMAGE.BMP loaded: {}x{}", width, height);
            }
            Err(message) => {
                defmt::error!("BMP viewer: {}", message);
                draw_error(frame, message);
            }
        }
        frame.frame_data().as_ptr() as u32
    };

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
            frame_addr,
        ))
        .unwrap();

    core::future::pending::<()>().await;
}

fn draw_error(frame: &mut FrameBuffer, message: &'static str) {
    frame.clear(BLACK);
    let style = MonoTextStyle::new(&FONT_6X10, Rgb666::WHITE);
    Text::new(
        "SD BMP viewer",
        embedded_graphics::geometry::Point::new(10, 20),
        style,
    )
    .draw(frame)
    .unwrap();
    Text::new(
        message,
        embedded_graphics::geometry::Point::new(10, 40),
        style,
    )
    .draw(frame)
    .unwrap();
}
