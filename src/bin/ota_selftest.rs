//! OTA 第 1 段階の自己診断 bin (`ota_selftest`)
//!
//! Wi-Fi は使わず、RP2350 bootrom の A/B パーティション + TBYB
//! (Try Before You Buy) の挙動を LCD で確認するためのファームウェア。
//!
//! 起動時に以下を bootrom から読み、400×96 LCD に表示する:
//! - どのパーティション (slot A / B) から起動したか (`get_sys_info` BOOT_INFO と
//!   `flash_runtime_to_storage_addr` の両方)
//! - 自イメージの版数 (IMAGE_DEF の VERSION 項目に載せたものと同じ値)
//! - TBYB の状態 (pending / bought / not TBYB)、ウォッチドッグの残り時間
//! - パーティションテーブルの一覧
//!
//! TBYB で起動した場合は 3 秒間の安定動作 (LCD 走査が回っていること) の後に
//! `explicit_buy` を呼んで自イメージを確定させる。`skip-buy` feature 付きで
//! ビルドすると buy を行わず、bootrom のウォッチドッグ (16.7 s) で旧イメージへ
//! 戻ることを確認できる。
//!
//! picotool 用 USB reset interface を持つので `picotool load -f` で書き換えられる。
//! LCD 走査は `lcd::display` (PIO0 SM0/SM1 + DMA CH0〜CH3 の自走リング、ダブルバッファ)。
//! DMA が読むのは SRAM だけなので explicit_buy のフラッシュ書き換え中も同期は崩れない。

#![no_std]
#![no_main]

use core::fmt::Write as _;
use embassy_executor::Spawner;
use embassy_rp::bind_interrupts;
use embassy_rp::peripherals::*;
use embassy_rp::pio::InterruptHandler;
use embassy_rp::usb::{Driver as UsbDriver, InterruptHandler as UsbInterruptHandler};
use embassy_time::{Duration, Instant, Timer};
use embassy_usb::UsbDevice;
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::mono_font::ascii::FONT_6X10;
use embedded_graphics::pixelcolor::Rgb666;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{PrimitiveStyle, Rectangle};
use embedded_graphics::text::{Baseline, Text};
use heapless::String;
use pico2w_300yen_lcd::ab_boot::{
    self, BootInfo, FLAGS_ACCEPTS_FAMILY_DATA, FLAGS_ACCEPTS_FAMILY_RP2350_ARM_S,
    FLAGS_NOT_BOOTABLE_ARM, Link, PartitionTable,
};
use pico2w_300yen_lcd::image_def::{FIRMWARE_VERSION, IMAGE_DEF_MAJOR, IMAGE_DEF_MINOR, TBYB};
use pico2w_300yen_lcd::lcd::display::{BackBuffer, Display, DisplayPins, FrameIrqHandler};
use pico2w_300yen_lcd::lcd::display::BACK_BLACK;
use pico2w_300yen_lcd::lcd::timing::H_ACTIVE;
use pico2w_300yen_lcd::usb_reset::build_usb_device;
use {defmt_rtt as _, panic_probe as _};

// RP2350 bootrom 用 IMAGE_DEF (版数付き) と picotool 用 binary_info を埋め込む
pico2w_300yen_lcd::firmware_image_def!();

bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => InterruptHandler<PIO0>;
    DMA_IRQ_1 => FrameIrqHandler;
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

fn draw_text(frame: &mut BackBuffer, text: &str, x: i32, y: i32, color: Rgb666) {
    let style = MonoTextStyle::new(&FONT_6X10, color);
    Text::with_baseline(text, Point::new(x, y), style, Baseline::Top)
        .draw(frame)
        .unwrap();
}

fn fill_rect(frame: &mut BackBuffer, x: i32, y: i32, w: u32, h: u32, color: Rgb666) {
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
fn draw_screen(frame: &mut BackBuffer, status: &Status) {
    frame.clear(BACK_BLACK);
    let mut y = 1;
    let mut line: String<80> = String::new();

    // 1 行目: 版数
    let uptime = status.boot_at.elapsed().as_secs();
    let _ = write!(
        line,
        "OTA selftest v{}  IMAGE_DEF {}.{}  tbyb-build:{}",
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
}

// ============================================================
// main
// ============================================================

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());

    let mut status = Status::collect();
    defmt::info!(
        "ota_selftest v{} IMAGE_DEF {}.{} boot={:?} storage={:?}",
        FIRMWARE_VERSION,
        IMAGE_DEF_MAJOR,
        IMAGE_DEF_MINOR,
        status.boot,
        status.storage_offset
    );

    // --- 初期画面をバックバッファに描いてから LCD 走査を開始 ---
    let mut display = Display::new(DisplayPins {
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
    });
    draw_screen(display.back(), &status);
    display.start(Irqs);

    // --- picotool 用 USB reset interface ---
    let usb = build_usb_device(UsbDriver::new(p.USB, Irqs), "OTA Selftest");
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

        // バックバッファに描き、垂直ブランキングでフロントへ反映 (ティアリング無し)
        draw_screen(display.back(), &status);
        display.present().await;
    }
}
