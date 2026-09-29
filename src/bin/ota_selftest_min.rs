//! **診断用** OTA 自己診断 bin (`ota_selftest_min`) — 白画面問題の実機二分探索用
//!
//! 内容は 9e9d7f1 時点の `ota_selftest` (正常に表示できていた最後の版、
//! 単一フレームバッファ + bin 内 `display_task`) そのままで、変更は
//! **SM1 タイミングテーブルを SRAM (`.data`) に置く** 1 点だけ。
//! `lcd::display` (ダブルバッファ / `present()` / DMA_IRQ_1) は使わない。
//!
//! - これが映って `ota_selftest` が映らなければ、原因は `lcd::display` 側。
//! - これも白画面なら、原因はテーブル配置以外 (ハード・パーティション等)。
//!
//! 画面 1 行目に `OTA selftest-MIN` と表示して区別する。TBYB 起動なら
//! 3 秒後に `explicit_buy` する点も元と同じ (SM1 テーブルが SRAM にあるので
//! buy 後に砂嵐にならないことも確認できる)。
//!
//! 通常運用では `ota_selftest` を使うこと。問題解決後に削除して構わない。

#![no_std]
#![no_main]

use core::fmt::Write as _;
use core::ptr::addr_of_mut;
use core::sync::atomic::{AtomicU32, Ordering};
use embassy_executor::Spawner;
use embassy_rp::Peri;
use embassy_rp::bind_interrupts;
use embassy_rp::peripherals::*;
use embassy_rp::pio::program::pio_asm;
use embassy_rp::pio::{
    Config, Direction, FifoJoin, InterruptHandler, Pio, ShiftConfig, ShiftDirection,
};
use embassy_rp::usb::{Driver as UsbDriver, InterruptHandler as UsbInterruptHandler};
use embassy_time::{Duration, Instant, Timer};
use embassy_usb::UsbDevice;
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::mono_font::ascii::FONT_6X10;
use embedded_graphics::pixelcolor::Rgb666;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{PrimitiveStyle, Rectangle};
use embedded_graphics::text::{Baseline, Text};
use fixed::FixedU32;
use fixed::types::extra::U8;
use heapless::String;
use pico2w_300yen_lcd::ab_boot::{
    self, BootInfo, FLAGS_ACCEPTS_FAMILY_DATA, FLAGS_ACCEPTS_FAMILY_RP2350_ARM_S,
    FLAGS_NOT_BOOTABLE_ARM, Link, PartitionTable,
};
use pico2w_300yen_lcd::image_def::{FIRMWARE_VERSION, IMAGE_DEF_MAJOR, IMAGE_DEF_MINOR, TBYB};
use pico2w_300yen_lcd::lcd::framebuffer::*;
use pico2w_300yen_lcd::lcd::timing::*;
use pico2w_300yen_lcd::usb_reset::build_usb_device;
use {defmt_rtt as _, panic_probe as _};

// RP2350 bootrom 用 IMAGE_DEF (版数付き) と picotool 用 binary_info を埋め込む
pico2w_300yen_lcd::firmware_image_def!();

bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => InterruptHandler<PIO0>;
    USBCTRL_IRQ => UsbInterruptHandler<USB>;
});

#[embassy_executor::task]
async fn usb_task(mut device: UsbDevice<'static, UsbDriver<'static, USB>>) -> ! {
    device.run().await
}

// ============================================================
// 動作パラメータ
// ============================================================

/// TBYB 起動時、explicit_buy を呼ぶまでの安定動作時間
const BUY_AFTER: Duration = Duration::from_secs(3);
/// 画面更新周期
const REDRAW_PERIOD: Duration = Duration::from_millis(500);

// ============================================================
// SM1 フレームデータ (static 配置) — wifi_status と同じ
// ============================================================

const VIEWER_SM1_FRAME_SIZE: usize = 2 + V_NORMAL_LINES as usize;

const fn viewer_sm1_frame_data() -> [u32; VIEWER_SM1_FRAME_SIZE] {
    let mut data = [0; VIEWER_SM1_FRAME_SIZE];
    data[0] = H_TOTAL - 7; // VSYNC 行: set×2 + pull/mov×2 + loop(X+1)
    data[1] = V_NORMAL_LINES - 1;
    let mut line = 0;
    while line < V_NORMAL_LINES as usize {
        data[2 + line] = H_TOTAL - 6; // 通常行: set×2 + pull/mov + loop(X+1) + jmp
        line += 1;
    }
    data
}

/// SM1 (HSYNC/VSYNC) 用タイミングデータ。9e9d7f1 との唯一の差分:
/// `.rodata` (フラッシュ XIP) ではなく `.data` (SRAM) に置き、フラッシュ
/// 消去・書き込み中に DMA CH1 がバスフォールトで止まらないようにする。
#[unsafe(link_section = ".data.lcd_sm1_frame_min")]
static SM1_FRAME_DATA: [u32; VIEWER_SM1_FRAME_SIZE] = viewer_sm1_frame_data();

// CH2/CH3 が各フレーム終端で読み、CH0/CH1 の読み出し先を再設定する。
static DMA_PIXEL_FRAME_ADDR: AtomicU32 = AtomicU32::new(0);
static DMA_TIMING_FRAME_ADDR: AtomicU32 = AtomicU32::new(0);

// ============================================================
// ペリフェラル構造体 (TaskFn 16引数制限の回避)
// ============================================================

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

/// 実機の可視開始位置。`lcd::display::VISIBLE_X_OFFSET` (106、実機確認済み 2026-09-29) と同じ値
const VIEWER_VISIBLE_X: usize = 106;

fn align_image_to_visible_area(frame: &mut FrameBuffer) {
    let nominal_x = H_BLANK_BEFORE_ACTIVE as usize;
    let width = H_ACTIVE as usize;
    for y in 0..ACTIVE_HEIGHT {
        let row_start = (ACTIVE_Y_OFFSET + y) * LINE_WIDTH;
        let source = row_start + nominal_x;
        let target = row_start + VIEWER_VISIBLE_X;
        frame.data.copy_within(source..source + width, target);
        let first = frame.data[target];
        let last = frame.data[target + width - 1];
        frame.data[row_start..target].fill(first);
        frame.data[target + width..row_start + LINE_WIDTH].fill(last);
    }
}

// ============================================================
// display_task: 全フレーム一括 DMA 転送 (wifi_status と同一)
// ============================================================

#[embassy_executor::task]
async fn display_task(res: DisplayPeripherals, frame_addr: u32) {
    // DMA CH0〜CH3 の所有権を保持 (PAC 直接操作のため embassy API では使わない)
    let _dma_ch0 = res.dma_ch0;
    let _dma_ch1 = res.dma_ch1;
    let _dma_ch2 = res.dma_ch2;
    let _dma_ch3 = res.dma_ch3;

    // === SM0: ピクセル出力 + NCLK (sideset) ===
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
        "    set pins, 3",
        "    set pins, 2",
        "    pull block",
        "    mov x, osr",
        "rest_v0:",
        "    jmp x-- rest_v0",
        "    pull block",
        "    mov y, osr",
        "normal_line:",
        "    set pins, 1",
        "    set pins, 0",
        "    pull block",
        "    mov x, osr",
        "rest_v1:",
        "    jmp x-- rest_v1",
        "    jmp y-- normal_line",
        ".wrap",
    );

    let Pio {
        mut common,
        mut sm0,
        mut sm1,
        ..
    } = Pio::new(res.pio0, Irqs);

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

    sm0.set_pin_dirs(
        Direction::Out,
        &[
            &pin2, &pin3, &pin4, &pin5, &pin6, &pin7, &pin8, &pin9, &pin10, &pin11, &pin12, &pin13,
            &pin14, &pin15, &pin16, &pin17, &pin18, &pin19, &nclk_pin,
        ],
    );
    sm1.set_pin_dirs(Direction::Out, &[&hsync_pin, &vsync_pin]);

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

    let loaded_timing = common.load_program(&prg_timing.program);
    let mut cfg1 = Config::default();
    cfg1.use_program(&loaded_timing, &[]);
    cfg1.set_set_pins(&[&hsync_pin, &vsync_pin]);
    cfg1.clock_divider = FixedU32::<U8>::from_bits(SM1_CLK_DIV_BITS);
    cfg1.fifo_join = FifoJoin::TxOnly;

    sm0.set_config(&cfg0);
    sm1.set_config(&cfg1);

    common.apply_sm_batch(|batch| {
        batch.set_enable(&mut sm0, true);
        batch.set_enable(&mut sm1, true);
    });

    let sm0_txf_addr = embassy_rp::pac::PIO0.txf(0).as_ptr() as u32;
    let sm1_txf_addr = embassy_rp::pac::PIO0.txf(1).as_ptr() as u32;

    DMA_PIXEL_FRAME_ADDR.store(frame_addr, Ordering::SeqCst);
    DMA_TIMING_FRAME_ADDR.store(SM1_FRAME_DATA.as_ptr() as u32, Ordering::SeqCst);

    let dma = embassy_rp::pac::DMA;
    dma.inte(0).write_value(dma.inte(0).read() & !0b1111);
    dma.intr(0).write_value(0b1111);

    // --- CH0 (SM0: ピクセルデータ) ---
    let ch0 = dma.ch(0);
    ch0.write_addr().write_value(sm0_txf_addr);
    {
        let mut ctrl = embassy_rp::pac::dma::regs::CtrlTrig(0);
        ctrl.set_en(true);
        ctrl.set_data_size(embassy_rp::pac::dma::vals::DataSize::SIZE_WORD);
        ctrl.set_incr_read(true);
        ctrl.set_incr_write(false);
        ctrl.set_treq_sel(embassy_rp::pac::dma::vals::TreqSel::PIO0_TX0);
        ctrl.set_chain_to(2);
        ch0.al1_ctrl().write_value(ctrl.0);
    }

    // --- CH1 (SM1: HSYNC/VSYNC タイミング) ---
    let ch1 = dma.ch(1);
    ch1.write_addr().write_value(sm1_txf_addr);
    {
        let mut ctrl = embassy_rp::pac::dma::regs::CtrlTrig(0);
        ctrl.set_en(true);
        ctrl.set_data_size(embassy_rp::pac::dma::vals::DataSize::SIZE_WORD);
        ctrl.set_incr_read(true);
        ctrl.set_incr_write(false);
        ctrl.set_treq_sel(embassy_rp::pac::dma::vals::TreqSel::PIO0_TX1);
        ctrl.set_chain_to(3);
        ch1.al1_ctrl().write_value(ctrl.0);
    }

    // CH2: ピクセルフレーム先頭アドレスを CH0 のトリガー別名へ 1 ワード転送
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
        ctrl.set_chain_to(2);
        ch2.al1_ctrl().write_value(ctrl.0);
    }

    // CH3: 同期フレーム先頭アドレスを CH1 のトリガー別名へ 1 ワード転送
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
        ctrl.set_chain_to(3);
        ch3.al1_ctrl().write_value(ctrl.0);
    }

    // 初回フレームは CH0/CH1 を同時起動。以後は CH2/CH3 チェインで連続供給。
    cortex_m::interrupt::free(|_| {
        let ch0 = dma.ch(0);
        ch0.read_addr().write_value(frame_addr);
        ch0.trans_count().write(|w| w.set_count(FB_SIZE as u32));

        let ch1 = dma.ch(1);
        ch1.read_addr().write_value(SM1_FRAME_DATA.as_ptr() as u32);
        ch1.trans_count()
            .write(|w| w.set_count(VIEWER_SM1_FRAME_SIZE as u32));

        dma.multi_chan_trigger().write(|w| w.set_multi_chan_trigger(0b11));
    });

    core::future::pending::<()>().await;
}

// ============================================================
// 起動情報の収集
// ============================================================

/// TBYB の進行状態
#[derive(Clone, Copy, PartialEq, Eq)]
enum BuyState {
    /// TBYB ではない通常起動 (buy 不要)
    NotTbyb,
    /// TBYB 起動。`BUY_AFTER` 経過後に buy する
    Pending,
    /// `skip-buy` ビルド: buy せず、ウォッチドッグで旧イメージへ戻る
    Skipped,
    /// explicit_buy 成功
    Bought,
    /// explicit_buy 失敗 (bootrom エラーコード)
    Failed(i32),
}

struct Status {
    boot: Option<BootInfo>,
    /// 0x1000_0000 が対応するストレージオフセット (= 起動パーティションの先頭)
    storage_offset: Result<u32, i32>,
    table: Result<PartitionTable, i32>,
    buy: BuyState,
    boot_at: Instant,
}

impl Status {
    fn collect() -> Self {
        let boot = BootInfo::read();
        let storage_offset = ab_boot::runtime_to_storage_offset(ab_boot::XIP_BASE);
        let table = PartitionTable::read();
        let buy = match boot {
            Some(b) if b.buy_pending() => {
                if cfg!(feature = "skip-buy") {
                    BuyState::Skipped
                } else {
                    BuyState::Pending
                }
            }
            _ => BuyState::NotTbyb,
        };
        Self {
            boot,
            storage_offset,
            table,
            buy,
            boot_at: Instant::now(),
        }
    }

    /// 起動パーティションの表示名 ("A"/"B"/名前)。テーブルとオフセットから判定。
    fn booted_slot_label(&self) -> String<24> {
        let mut s: String<24> = String::new();
        let Ok(table) = &self.table else {
            let _ = s.push_str("no table");
            return s;
        };
        let by_offset = self
            .storage_offset
            .ok()
            .and_then(|offset| table.find_by_offset(offset));
        let by_boot_info = self
            .boot
            .filter(|b| b.partition >= 0)
            .and_then(|b| table.get(b.partition as u8));
        match by_offset.or(by_boot_info) {
            Some(p) => {
                let ab = match p.link() {
                    Link::BOf(_) => "B",
                    _ => "A",
                };
                let _ = write!(s, "slot {} (P{} {})", ab, p.index, p.name());
            }
            None => {
                let _ = s.push_str("unpartitioned");
            }
        }
        s
    }
}

/// ウォッチドッグの残り時間 (秒、小数 1 桁)。無効なら None。
fn watchdog_remaining() -> Option<u32> {
    let ctrl = embassy_rp::pac::WATCHDOG.ctrl().read();
    if ctrl.enable() {
        // CTRL.TIME は 1 µs 刻みの残り時間 (bootrom は TBYB 用に 16.7 s = 0xffffff を設定)
        Some(ctrl.time() / 100_000)
    } else {
        None
    }
}

// ============================================================
// 描画
// ============================================================

const TEXT_X: i32 = 2;
const ROW_HEIGHT: i32 = 10;

fn draw_text(frame: &mut FrameBuffer, text: &str, x: i32, y: i32, color: Rgb666) {
    let style = MonoTextStyle::new(&FONT_6X10, color);
    Text::with_baseline(text, Point::new(x, y), style, Baseline::Top)
        .draw(frame)
        .unwrap();
}

fn fill_rect(frame: &mut FrameBuffer, x: i32, y: i32, w: u32, h: u32, color: Rgb666) {
    Rectangle::new(Point::new(x, y), Size::new(w, h))
        .into_styled(PrimitiveStyle::with_fill(color))
        .draw(frame)
        .unwrap();
}

const WHITE: Rgb666 = Rgb666::new(63, 63, 63);
const GRAY: Rgb666 = Rgb666::new(32, 32, 32);
const CYAN: Rgb666 = Rgb666::new(0, 63, 63);
const GREEN: Rgb666 = Rgb666::new(0, 63, 0);
const YELLOW: Rgb666 = Rgb666::new(63, 63, 0);
const RED: Rgb666 = Rgb666::new(63, 0, 0);

/// 9 行 × 66 文字 (FONT_6X10) に収める。
///
/// ```text
/// OTA selftest v0.1.0  IMAGE_DEF 0.100  tbyb-build:no      up 12s
/// booted: slot A (P0 app-a)  storage 0x00002000  type FLASH_UPDATE(4)
/// boot_info: part=0 tbyb=0x01 diag=0x00004c4d p0=0x10002000
/// TBYB: pending -> buy in 3s        WDT: 14.2s left
/// partition table: 3 partitions
///  P0 app-a  0x002000-0x1E1FFF 1920K  arm-s  A
///  P1 app-b  0x1E2000-0x3C1FFF 1920K  arm-s  B of P0
///  P2 data   0x3C2000-0x3FFFFF  248K  data   (not bootable)
/// ```
fn draw_screen(frame: &mut FrameBuffer, status: &Status) {
    frame.clear(BLACK);
    let mut y = 1;
    let mut line: String<80> = String::new();

    // 1 行目: 版数
    let uptime = status.boot_at.elapsed().as_secs();
    let _ = write!(
        line,
        "OTA selftest-MIN v{}  IMAGE_DEF {}.{}  tbyb-build:{}",
        FIRMWARE_VERSION,
        IMAGE_DEF_MAJOR,
        IMAGE_DEF_MINOR,
        if TBYB { "yes" } else { "no" }
    );
    draw_text(frame, &line, TEXT_X, y, CYAN);
    line.clear();
    let _ = write!(line, "up {}s", uptime);
    draw_text(frame, &line, 400 - 6 * line.len() as i32 - 2, y, GRAY);
    y += ROW_HEIGHT;
    fill_rect(frame, 0, y, H_ACTIVE, 1, Rgb666::new(16, 16, 16));
    y += 2;

    // 2 行目: 起動スロット
    line.clear();
    let _ = write!(line, "booted: {}", status.booted_slot_label());
    match status.storage_offset {
        Ok(offset) => {
            let _ = write!(line, "  storage 0x{:08X}", offset);
        }
        Err(e) => {
            let _ = write!(line, "  storage err {}", e);
        }
    }
    if let Some(b) = &status.boot {
        let _ = write!(line, "  type {}({})", b.boot_type_name(), b.boot_type);
    }
    draw_text(frame, &line, TEXT_X, y, WHITE);
    y += ROW_HEIGHT;

    // 3 行目: BOOT_INFO 生値
    line.clear();
    match &status.boot {
        Some(b) => {
            let _ = write!(
                line,
                "boot_info: part={} tbyb=0x{:02X} diag=0x{:08X} p0=0x{:08X}",
                b.partition, b.tbyb_and_update_info, b.boot_diagnostic, b.reboot_params[0]
            );
        }
        None => {
            let _ = line.push_str("boot_info: get_sys_info failed");
        }
    }
    draw_text(frame, &line, TEXT_X, y, GRAY);
    y += ROW_HEIGHT;

    // 4 行目: TBYB 状態とウォッチドッグ
    line.clear();
    let (color, remaining_to_buy) = match status.buy {
        BuyState::NotTbyb => {
            let _ = line.push_str("TBYB: not a TBYB boot (nothing to buy)");
            (WHITE, None)
        }
        BuyState::Pending => {
            let left = BUY_AFTER
                .as_millis()
                .saturating_sub(status.boot_at.elapsed().as_millis());
            (YELLOW, Some(left))
        }
        BuyState::Skipped => {
            let _ = line.push_str("TBYB: pending, skip-buy build -> rollback by WDT");
            (RED, None)
        }
        BuyState::Bought => {
            let _ = line.push_str("TBYB: bought OK (explicit_buy rc=0)");
            (GREEN, None)
        }
        BuyState::Failed(rc) => {
            let _ = write!(line, "TBYB: explicit_buy FAILED rc={} {}", rc, ab_boot::error_name(rc));
            (RED, None)
        }
    };
    if let Some(left) = remaining_to_buy {
        let _ = write!(line, "TBYB: pending -> buy in {}.{}s", left / 1000, (left % 1000) / 100);
    }
    match watchdog_remaining() {
        Some(tenths) => {
            let _ = write!(line, "   WDT: {}.{}s left", tenths / 10, tenths % 10);
        }
        None => {
            let _ = line.push_str("   WDT: off");
        }
    }
    draw_text(frame, &line, TEXT_X, y, color);
    y += ROW_HEIGHT;

    // 5 行目以降: パーティションテーブル
    line.clear();
    match &status.table {
        Ok(table) => {
            let _ = write!(
                line,
                "partition table: {} partitions{}",
                table.count(),
                if table.present { "" } else { " (not present)" }
            );
            draw_text(frame, &line, TEXT_X, y, CYAN);
            y += ROW_HEIGHT;
            for p in table.partitions().take(4) {
                line.clear();
                let families = if p.has_flag(FLAGS_ACCEPTS_FAMILY_RP2350_ARM_S) {
                    "arm-s"
                } else if p.has_flag(FLAGS_ACCEPTS_FAMILY_DATA) {
                    "data "
                } else {
                    "-    "
                };
                let _ = write!(
                    line,
                    " P{} {:<6} 0x{:06X}-0x{:06X} {:>5}K  {} ",
                    p.index,
                    p.name(),
                    p.start_offset(),
                    p.end_offset() - 1,
                    p.size() / 1024,
                    families
                );
                match p.link() {
                    Link::None => {
                        let _ = line.push_str("A");
                    }
                    Link::BOf(a) => {
                        let _ = write!(line, "B of P{}", a);
                    }
                    Link::OwnedBy(o) => {
                        let _ = write!(line, "owned by P{}", o);
                    }
                }
                if p.has_flag(FLAGS_NOT_BOOTABLE_ARM) {
                    let _ = line.push_str(" (not bootable)");
                }
                let booted = status
                    .storage_offset
                    .is_ok_and(|offset| p.contains_offset(offset));
                draw_text(frame, &line, TEXT_X, y, if booted { GREEN } else { WHITE });
                y += ROW_HEIGHT;
            }
        }
        Err(e) => {
            let _ = write!(
                line,
                "partition table: error {} ({})",
                e,
                ab_boot::error_name(*e)
            );
            draw_text(frame, &line, TEXT_X, y, RED);
        }
    }

    align_image_to_visible_area(frame);
}

// ============================================================
// main
// ============================================================

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());

    let mut status = Status::collect();
    defmt::info!(
        "ota_selftest_min v{} IMAGE_DEF {}.{} boot={:?} storage={:?}",
        FIRMWARE_VERSION,
        IMAGE_DEF_MAJOR,
        IMAGE_DEF_MINOR,
        status.boot,
        status.storage_offset
    );

    // --- 初期画面を描いてから LCD 走査を開始 ---
    let frame_addr = {
        // Safety: この時点では DMA は未起動。
        let frame = unsafe { &mut *addr_of_mut!(FB_DATA) };
        draw_screen(frame, &status);
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

    // --- picotool 用 USB reset interface ---
    let usb = build_usb_device(UsbDriver::new(p.USB, Irqs), "OTA Selftest MIN");
    spawner.spawn(usb_task(usb)).unwrap();

    // --- メインループ: 画面更新と、TBYB なら一定時間後の explicit_buy ---
    loop {
        Timer::after(REDRAW_PERIOD).await;

        if status.buy == BuyState::Pending && status.boot_at.elapsed() >= BUY_AFTER {
            defmt::info!("explicit_buy ...");
            status.buy = match ab_boot::explicit_buy() {
                Ok(()) => {
                    defmt::info!("explicit_buy OK");
                    BuyState::Bought
                }
                Err(rc) => {
                    defmt::error!("explicit_buy failed: {}", rc);
                    BuyState::Failed(rc)
                }
            };
        }

        // DMA が走査中のフレームバッファへ直接描く (テキストなので許容)
        // Safety: フレームバッファは main タスクだけが書く。
        let frame = unsafe { &mut *addr_of_mut!(FB_DATA) };
        draw_screen(frame, &status);
    }
}
