//! Wi-Fi OTA (第 2 段階): `wifi_status` の表示 + GitHub Release からの自己更新
//!
//! - SD カードの `WIFI.TXT` で Wi-Fi に接続し、周辺 AP の RSSI を LCD に表示する (wifi_status と同じ)。
//! - DHCP 完了後、および `OTA_CHECK_INTERVAL` ごとに
//!   `https://github.com/<repo>/releases/latest/download/manifest.json` を取得し、
//!   自分より新しい版があれば `.bin` を **他方の A/B 区画** へストリーミング書き込みする
//!   (`ota::slot`)。SHA-256 と読み戻しで検証した後、`reboot(FLASH_UPDATE)` で新版を起動する。
//! - 新版は TBYB (Try Before You Buy) 付きなので bootrom のウォッチドッグ (16.7 s) 下で起動する。
//!   本 bin は自己診断 = 「LCD 走査中 + Wi-Fi join + DHCP で IP 取得」が成立したら
//!   `explicit_buy` で確定する。join + DHCP は 16.7 s に収まらないことがある (v0.2.2 の実機で
//!   DHCP 待ち中に巻き戻った) ので、buy 待ちの間は `tbyb_watchdog_task` が 2 s ごとに
//!   WATCHDOG.LOAD を再ロードして延長する (データシート §5.1.17 が認める方法)。ただし起動から
//!   `TBYB_SELFTEST_DEADLINE_SECS` 経っても成立しなければ延長をやめ、bootrom の設定どおり
//!   ウォッチドッグで旧版に戻る。`explicit_buy` は bootrom 側が最初にウォッチドッグを止める。
//! - 失敗 (DNS/TLS/HTTP/フラッシュ/ハッシュ) は LCD に表示し、60 s → 最大 10 min のバックオフで再試行。
//!   検証を通らないイメージで再起動することはない。
//! - 対象区画に manifest と同じイメージが既にある (= 前回 TBYB 起動で buy されず巻き戻った) ときは
//!   `Rejected` とし、最初にそう判定した時刻 + `REJECTED_RETRY_DELAY` に FLASH_UPDATE 起動を再試行する。
//!   60 s ごとの確認で同じ判定が続いても再試行時刻は動かさない (`OtaState::rejected`。v0.2.3 までは
//!   確認ごとに 10 分後へ延びて永遠に再試行しなかった)。
//! - 起動時は WL_REG_ON を `wifi::CYW43_POWER_OFF_MS` (500 ms) 落として CYW43439 をコールドスタートさせ、
//!   FLASH_UPDATE 再起動の前にも `leave()` + WL_REG_ON Low (`wifi::power_off_for_reboot`) で電源を切る。
//!   DHCP が 20 s で通らなければ AP から離脱して再 join する (v0.2.5 の TBYB 起動は温かい再起動で
//!   join は通るが DHCP が一度も通らず、再 join もしないまま 120 s で巻き戻った)。
//! - 巻き戻りの原因が分かるように、buy 待ちの新版は進行段階と稼働時間を WATCHDOG.SCRATCH5〜7 に
//!   書き続ける (`boot_trace`)。panic / HardFault も記録する。巻き戻り後に起動した旧版は起動時に
//!   それを読み、`WATCHDOG.REASON` と BOOT_INFO の診断ワードと共に LCD の下段に出す。
//!
//! - 表示位置の確認用に、画面の外周 1 px に暗い灰色の枠を常に描く (四辺が写真で見えれば 400×96 全体が
//!   表示されている。`lcd::display::VISIBLE_X_OFFSET` = 106 は v0.2.7 の目盛り表示で実機確認し、
//!   v0.2.8 で目盛りを削除して正式版にした)。
//!
//! TLS は `TlsVerify::None` (証明書検証なし)。理由と影響は docs/wifi-ota.md「セキュリティ」。
//!
//! v0.3.0: TBYB / 接続管理 / OTA の本体は `ota::app` に移し (`ticker` と共用)、この bin には LCD の描画と
//! メインループだけが残っている。振る舞いは v0.2.8 と同じ。
//!
//! LCD (400×96 = FONT_6X10 で 66 桁 × 9 行。各行は 66 桁に収める: 末尾の残り秒 / WDT が切れないように):
//! ```text
//! <SSID> 192.168.1.23 -52dBm  scan #12                                   ← 行 0 (wifi_status と同じ)
//! wifi_ota v0.2.4 via OTA slot B TBYB:pending 37/120s WDT 15.1s
//! ^^^^^^^^^^^^^^^^^^^^^^^ 版数はマゼンタ (0.2.2〜)。via OTA は FLASH_UPDATE 起動 (= OTA で届いたイメージ) のときだけ出る
//! OTA: 0.2.0 -> 0.2.1 downloading 45%  196608/435200 B
//! [=================                       ]                                 ← 進捗バー (ダウンロード中のみ)
//!  SSID ... RSSI 棒グラフ (上位 5 件。下の診断行が出るときは 3〜4 件)
//! TBYB 0.2.3: dhcp-wait @121.3s join3 fail2 dhcpto1              ← 前回の TBYB 起動の記録 (あるときだけ)
//! NORMAL P0 A:4C4D launched B:000D imgdef reset:wdt              ← 起動種別 / 診断ワード / リセット理由 (ウォッチドッグ起動のとき)
//! ```

#![no_std]
#![no_main]

use core::fmt::Write as _;
use core::mem::MaybeUninit;
use core::ptr::addr_of_mut;

use cyw43::{PowerManagementMode, ScanOptions};
use embassy_executor::Spawner;
use embassy_rp::bind_interrupts;
use embassy_rp::flash::Flash;
use embassy_rp::peripherals::*;
use embassy_rp::pio::{InterruptHandler, Pio};
use embassy_rp::usb::{Driver as UsbDriver, InterruptHandler as UsbInterruptHandler};
use embassy_time::{Duration, Instant, Timer};
use embassy_usb::UsbDevice;
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::mono_font::ascii::FONT_6X10;
use embedded_graphics::pixelcolor::Rgb666;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{PrimitiveStyle, Rectangle};
use embedded_graphics::text::{Baseline, Text};
use heapless::{String, Vec};
use pico2w_300yen_lcd::boot_trace::{self, Stage};
use pico2w_300yen_lcd::image_def::{FIRMWARE_VERSION, TBYB};
use pico2w_300yen_lcd::lcd::display::{BACK_HEIGHT, BACK_WIDTH, BackBuffer, Display, DisplayPins, FrameIrqHandler};
use pico2w_300yen_lcd::lcd::framebuffer::BLACK;
use pico2w_300yen_lcd::lcd::timing::H_ACTIVE;
use pico2w_300yen_lcd::ota::app::{
    self, BootStatus, Link, LinkManager, LinkUi, Net, NetBuffers, OtaPhase, OtaState, OtaUi, TcpState, Tone,
    secs_until,
};
use pico2w_300yen_lcd::ota::slot::{OtaFlash, SectorBuffers, Slots, slot_label};
use pico2w_300yen_lcd::sdcard::init_sd;
use pico2w_300yen_lcd::usb_reset::build_usb_device;
use pico2w_300yen_lcd::wifi::{
    self, ApEntry, Cyw43Pins, MAX_SCAN_APS, WifiCredentials, ascii_label, merge_ap, read_credentials,
};
use defmt_rtt as _;

// RP2350 bootrom 用 IMAGE_DEF (版数付き、--features tbyb で TBYB フラグ) と picotool 用 binary_info
pico2w_300yen_lcd::firmware_image_def!();

/// panic: 段階と行番号を SCRATCH に記録 → defmt に出力 → `udf` で HardFault (panic-probe と同じ止まり方)。
/// TBYB 起動中なら延長タスクも止まるので最長 16.7 s 後にウォッチドッグで旧版へ戻り、旧版の LCD に
/// `PANIC @12.3s line=N` が出る。通常起動では panic-probe と同様に止まったまま (LCD は最後の画面のまま)。
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    boot_trace::fault(Stage::Panic, info.location().map_or(0, |l| l.line()));
    defmt::error!("{}", defmt::Display2Format(info));
    cortex_m::asm::udf()
}

/// HardFault (スタック溢れ、バスフォールト、panic からの `udf` など): 段階と PC を記録して止まる。
/// panic 経由のときは `fault` が最初の PANIC 記録を残す。
#[cortex_m_rt::exception]
unsafe fn HardFault(frame: &cortex_m_rt::ExceptionFrame) -> ! {
    boot_trace::fault(Stage::HardFault, frame.pc());
    loop {
        cortex_m::asm::nop();
    }
}

bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => InterruptHandler<PIO0>;
    PIO1_IRQ_0 => InterruptHandler<PIO1>;
    DMA_IRQ_1 => FrameIrqHandler;
    USBCTRL_IRQ => UsbInterruptHandler<USB>;
});

#[embassy_executor::task]
async fn usb_task(mut device: UsbDevice<'static, UsbDriver<'static, USB>>) -> ! {
    device.run().await
}

// ============================================================
// ヒープ (embedded-tls の rsa feature → rsa / num-bigint-dig が alloc を要求する。
// アロケータ本体は lib の `heap` モジュール。TlsVerify::None では実際には使われない見込み)
// ============================================================

const HEAP_SIZE: usize = 8 * 1024;
static mut HEAP_MEM: [MaybeUninit<u8>; HEAP_SIZE] = [MaybeUninit::uninit(); HEAP_SIZE];

// ============================================================
// 動作パラメータ (OTA / TBYB / 接続のパラメータは ota::app)
// ============================================================

/// wifi_status と同じスキャン周期
const SCAN_PERIOD: Duration = Duration::from_secs(10);
/// メインループの周期 (画面更新)
const TICK: Duration = Duration::from_millis(500);
/// 表示する AP 数 (上位 RSSI)。OTA 行 2 本 + 進捗バーの分だけ wifi_status より少ない
const MAX_DISPLAY_APS: usize = 5;

// ============================================================
// static 配置のバッファ (BSS。大きな配列を future / スタックに置かない)
// ============================================================

static mut NET_BUFFERS: NetBuffers = NetBuffers::new();
static mut TCP_STATE: TcpState = TcpState::new();
static mut SECTOR_BUFFERS: SectorBuffers = SectorBuffers::new();

// ============================================================
// 状態
// ============================================================

/// 画面に出す情報一式 (描画関数はこれだけを見る)
struct Model {
    status: String<80>,
    joined_ssid: Option<String<32>>,
    aps: Vec<ApEntry, MAX_SCAN_APS>,
    boot: BootStatus,
    ota: OtaState,
}

struct Ui {
    display: Display,
    model: Model,
}

impl Ui {
    async fn present(&mut self) {
        draw_screen(self.display.back(), &self.model);
        self.display.present().await;
    }
}

/// OTA の途中経過 (Checking / Downloading / Verifying) をそのまま描く
impl OtaUi for Ui {
    async fn ota_phase(&mut self, phase: OtaPhase) {
        self.model.ota.phase = phase;
        self.present().await;
    }
}

/// 接続中の文言 (connecting / waiting for DHCP / DHCP timeout) をステータス行に描く
impl LinkUi for Ui {
    async fn link_status(&mut self, text: &str, joined_ssid: Option<&String<32>>) {
        self.model.status.clear();
        let _ = self.model.status.push_str(text);
        self.model.joined_ssid = joined_ssid.cloned();
        self.present().await;
    }
}

// ============================================================
// 描画
// ============================================================

const ROW_HEIGHT: i32 = 10;
const TEXT_X: i32 = 2;
const STATUS_Y: i32 = 1;
const OTA_ID_Y: i32 = 12;
const OTA_STATE_Y: i32 = 22;
const PROGRESS_Y: i32 = 33;
const LIST_TOP: i32 = 38;
const SSID_CHARS: usize = 21; // 6px × 21 = 126px
const BAR_X: i32 = 134;
const BAR_WIDTH: i32 = 186;
const RSSI_X: i32 = 326;
const RSSI_MIN: i32 = -100;
const RSSI_MAX: i32 = -30;

const WHITE: Rgb666 = Rgb666::new(63, 63, 63);
const GRAY: Rgb666 = Rgb666::new(32, 32, 32);
const DIM: Rgb666 = Rgb666::new(16, 16, 16);
const CYAN: Rgb666 = Rgb666::new(0, 63, 63);
const GREEN: Rgb666 = Rgb666::new(0, 63, 0);
const YELLOW: Rgb666 = Rgb666::new(63, 63, 0);
const RED: Rgb666 = Rgb666::new(63, 0, 0);
/// 行 1 の版数 (`wifi_ota vX.Y.Z [via OTA]`) の色。0.2.1 以前は TBYB の状態色と同じだった。
const VERSION_COLOR: Rgb666 = Rgb666::new(63, 0, 63); // マゼンタ
/// 画面の外周 1 px の枠の色 (暗い灰色)。四辺が写真で見えれば 400×96 の全体が表示されている
const FRAME_COLOR: Rgb666 = Rgb666::new(24, 24, 24);

/// ota::app の色調 → この画面の色 (v0.2.8 までの色と同じ)
fn tone_color(tone: Tone) -> Rgb666 {
    match tone {
        Tone::Muted => GRAY,
        Tone::Normal => WHITE,
        Tone::Ok => GREEN,
        Tone::Busy => YELLOW,
        Tone::Error => RED,
    }
}

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

fn rssi_color(rssi: i16) -> Rgb666 {
    if rssi >= -60 {
        GREEN
    } else if rssi >= -75 {
        YELLOW
    } else {
        Rgb666::new(63, 16, 0)
    }
}

/// 画面の外周 1 px の枠 (x=0 / x=399 / y=0 / y=95)。毎フレーム最初に描く。
/// 写真で四辺が見えれば、バックバッファの 400×96 全体が LCD に表示されている
/// (`lcd::display::VISIBLE_X_OFFSET` が正しい)。
fn draw_frame_border(frame: &mut BackBuffer) {
    let w = BACK_WIDTH as u32;
    let h = BACK_HEIGHT as u32;
    fill_rect(frame, 0, 0, w, 1, FRAME_COLOR);
    fill_rect(frame, 0, BACK_HEIGHT as i32 - 1, w, 1, FRAME_COLOR);
    fill_rect(frame, 0, 0, 1, h, FRAME_COLOR);
    fill_rect(frame, BACK_WIDTH as i32 - 1, 0, 1, h, FRAME_COLOR);
}

fn draw_screen(frame: &mut BackBuffer, model: &Model) {
    frame.clear(BLACK);
    draw_frame_border(frame);

    // 行 0: Wi-Fi ステータス (wifi_status と同じ)
    let status_color = if model.joined_ssid.is_some() { CYAN } else { WHITE };
    draw_text(frame, &model.status, TEXT_X, STATUS_Y, status_color);

    // 行 1: 自分の版数 / 区画 / TBYB
    // 版数の部分だけ VERSION_COLOR で描く (0.2.2 から。OTA 更新の前後を色でも見分けるため)。
    let mut head: String<32> = String::new();
    let _ = write!(head, "wifi_ota v{}", FIRMWARE_VERSION);
    // OTA で届いたイメージ (FLASH_UPDATE 起動) だと版数の隣に印を出す。TBYB を buy した後も
    // BOOT_INFO の boot_type は変わらないので、電源を切るまで見える。
    if model.boot.is_ota_boot() {
        let _ = head.push_str(" via OTA");
    }
    draw_text(frame, &head, TEXT_X, OTA_ID_Y, VERSION_COLOR);
    // 残り (区画 / TBYB) は版数の右に続けて TBYB の状態色で描く。head は ASCII のみなので 6px/文字。
    // 行全体で 66 桁 (400 px) に収める: "wifi_ota v0.2.4 via OTA" (23) + " slot B" (7) +
    // " TBYB:timeout->rollback" (23) + " WDT 16.7s" (10) = 63。v0.2.3 までは 100 桁を超え、WDT が画面外だった。
    let rest_x = TEXT_X + head.len() as i32 * FONT_6X10.character_size.width as i32;
    let mut line: String<96> = String::new();
    // TBYB フラグ無しのビルド (USB で入れる wifi_ota-plain) だけ印を出す。OTA で届くイメージは常に TBYB 付き
    let tbyb_tone = model.boot.write_tbyb_line(&mut line, !TBYB);
    draw_text(frame, &line, rest_x, OTA_ID_Y, tone_color(tbyb_tone));

    // 行 2: OTA の状態
    line.clear();
    let (ota_tone, progress) = model.ota.write_line(&mut line, &model.boot.slots);
    draw_text(frame, &line, TEXT_X, OTA_STATE_Y, tone_color(ota_tone));

    // 行 3: 進捗バー、または区切り線
    match progress {
        Some((done, total)) => {
            let width = (H_ACTIVE as i32 - 2 * TEXT_X) as u32;
            let filled = if total > 0 {
                (done as u64 * width as u64 / total as u64) as u32
            } else {
                0
            };
            fill_rect(frame, TEXT_X, PROGRESS_Y, width, 3, DIM);
            fill_rect(frame, TEXT_X, PROGRESS_Y, filled, 3, YELLOW);
        }
        None => fill_rect(frame, 0, PROGRESS_Y + 1, H_ACTIVE, 1, DIM),
    }

    // 行 4〜: 上段に AP 一覧、下段に起動診断 (前回の TBYB 記録 / 起動種別)。診断行の分だけ AP を減らす
    let mut diag_rows = 0;
    if model.boot.show_boot_line() {
        diag_rows += 1;
    }
    if model.boot.prev_trace.is_some() {
        diag_rows += 1;
    }
    let ap_rows = MAX_DISPLAY_APS - diag_rows;
    let mut next_diag_y = LIST_TOP + ap_rows as i32 * ROW_HEIGHT;
    line.clear();
    if let Some(tone) = model.boot.write_prev_trace_line(&mut line) {
        draw_text(frame, &line, TEXT_X, next_diag_y, tone_color(tone));
        next_diag_y += ROW_HEIGHT;
    }
    if model.boot.show_boot_line() {
        line.clear();
        model.boot.write_boot_line(&mut line);
        draw_text(frame, &line, TEXT_X, next_diag_y, GRAY);
    }
    if model.aps.is_empty() {
        draw_text(frame, "(no scan results yet)", TEXT_X, LIST_TOP, GRAY);
    }
    for (index, ap) in model.aps.iter().take(ap_rows).enumerate() {
        let y = LIST_TOP + index as i32 * ROW_HEIGHT;
        let label: String<SSID_CHARS> = ascii_label(ap.ssid());
        let is_joined = model
            .joined_ssid
            .as_ref()
            .is_some_and(|ssid| ssid.as_bytes() == ap.ssid());
        draw_text(frame, &label, TEXT_X, y, if is_joined { CYAN } else { WHITE });

        let rssi = i32::from(ap.rssi).clamp(RSSI_MIN, RSSI_MAX);
        let filled = ((rssi - RSSI_MIN) * BAR_WIDTH / (RSSI_MAX - RSSI_MIN)).max(1) as u32;
        fill_rect(frame, BAR_X, y + 1, BAR_WIDTH as u32, 7, Rgb666::new(6, 6, 6));
        fill_rect(frame, BAR_X, y + 1, filled, 7, rssi_color(ap.rssi));

        let mut rssi_text: String<16> = String::new();
        let _ = write!(rssi_text, "{}dBm ch{}", ap.rssi, ap.channel);
        draw_text(frame, &rssi_text, RSSI_X, y, Rgb666::new(48, 48, 48));
    }
}

// ============================================================
// main
// ============================================================

/// 対象区画への FLASH_UPDATE 再起動 (`ota::app::reboot_into_slot`)。先に表示を更新する。戻らない。
async fn reboot_into_slot(ui: &mut Ui, control: &mut cyw43::Control<'static>, slots: Slots) -> ! {
    ui.model.status.clear();
    let _ = write!(
        ui.model.status,
        "rebooting into slot {} (P{})... wifi off",
        slot_label(&slots.target),
        slots.target.index
    );
    ui.model.joined_ssid = None;
    ui.present().await;
    app::reboot_into_slot(control, slots).await
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());
    // Safety: HEAP_MEM は他から参照されない。init は 1 回だけ。
    unsafe { pico2w_300yen_lcd::heap::init(&mut *addr_of_mut!(HEAP_MEM)) };

    let boot = BootStatus::collect();
    defmt::info!(
        "wifi_ota v{} tbyb={} boot={:?} slots={:?}",
        FIRMWARE_VERSION,
        TBYB,
        boot.boot,
        boot.slots.as_ref().map(|s| (s.own.index, s.target.index)).map_err(|e| *e)
    );
    defmt::info!("reset reason: {:?}", boot.reset_reason);
    if let Some(trace) = &boot.prev_trace {
        defmt::warn!("previous TBYB boot left a trace: {:?}", trace);
    }
    // TBYB 起動なら bootrom のウォッチドッグの延長を始める (ota::app)
    boot.start_tbyb_feeding(&spawner);

    // --- SD カードから wifi.txt (GPIO SPI は同期処理なので走査開始前に済ませる) ---
    let credentials: Result<WifiCredentials, &'static str> = match init_sd(p.PIN_0, p.PIN_26, p.PIN_27, p.PIN_28) {
        Ok(volume_mgr) => read_credentials(&volume_mgr),
        Err(message) => Err(message),
    };
    match &credentials {
        Ok(c) => defmt::info!("wifi.txt: SSID={}", c.ssid.as_str()),
        Err(message) => defmt::warn!("wifi.txt: {}", message),
    }
    boot_trace::stage(Stage::SdRead);

    // --- 初期画面をバックバッファに描いてから LCD 走査を開始 ---
    let display = Display::new(DisplayPins {
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
    let mut ui = Ui {
        display,
        model: Model {
            status: String::new(),
            joined_ssid: None,
            aps: Vec::new(),
            boot,
            ota: OtaState::new(),
        },
    };
    match &credentials {
        Ok(c) => {
            let _ = write!(
                ui.model.status,
                "Wi-Fi: starting... ({})",
                ascii_label::<32>(c.ssid.as_bytes())
            );
        }
        Err(message) => {
            let _ = write!(ui.model.status, "Wi-Fi: {} (scan only)", message);
            ui.model.ota.phase = OtaPhase::Disabled(message);
        }
    }
    if let Err(e) = &ui.model.boot.slots {
        ui.model.ota.phase = OtaPhase::Disabled(e.label());
    }
    draw_screen(ui.display.back(), &ui.model);
    ui.display.start(Irqs);
    boot_trace::stage(Stage::DisplayStarted);

    // --- picotool 用 USB reset interface ---
    let usb = build_usb_device(UsbDriver::new(p.USB, Irqs), "Wi-Fi OTA");
    spawner.spawn(usb_task(usb)).unwrap();

    // --- CYW43439 + embassy-net (DHCP)。ダウンロード速度のため Performance ---
    // 最初に WL_REG_ON を CYW43_POWER_OFF_MS 落とす (FLASH_UPDATE の温かい再起動でも CYW43 をコールドスタート
    // させる。v0.2.5 の TBYB 起動で join 成功・DHCP 不通のまま巻き戻った原因の対策)。
    boot_trace::stage(Stage::WifiPowerCycle);
    ui.model.status.clear();
    let _ = write!(
        ui.model.status,
        "Wi-Fi: power cycle ({} ms) + init...",
        wifi::CYW43_POWER_OFF_MS
    );
    ui.present().await;
    let pio1 = Pio::new(p.PIO1, Irqs);
    let wifi::Network {
        stack,
        mut control,
        ..
    } = wifi::start(
        &spawner,
        pio1,
        Cyw43Pins {
            pwr: p.PIN_23,
            cs: p.PIN_25,
            dio: p.PIN_24,
            clk: p.PIN_29,
            dma: p.DMA_CH4,
        },
        PowerManagementMode::Performance,
        || boot_trace::stage(Stage::WifiInit),
    )
    .await;
    boot_trace::stage(Stage::WifiReady);

    // --- フラッシュと HTTP クライアントの資源 ---
    let mut flash: OtaFlash = Flash::new_blocking(p.FLASH);
    // Safety: これらの static は main からしか触らず、main は 1 回しか走らない。
    let bufs = unsafe { &mut *addr_of_mut!(NET_BUFFERS) };
    let sectors = unsafe { &mut *addr_of_mut!(SECTOR_BUFFERS) };
    let tcp_state = unsafe { &*addr_of_mut!(TCP_STATE) };
    let net = Net::new(stack, tcp_state);

    let mut link = LinkManager::new(&credentials);
    let mut next_scan = Instant::now();
    let mut scan_count: u32 = 0;
    let ota_possible = credentials.is_ok() && ui.model.boot.slots.is_ok();

    loop {

        // --- 接続管理 (join → DHCP → 通らなければ離脱して再 join。ota::app::LinkManager) ---
        let network_up = link.step(&mut control, stack, &credentials, &mut ui).await;
        if network_up {
            ui.model.ota.schedule_first_check();
        }

        // --- TBYB: 自己診断 = LCD 走査中 + Wi-Fi join + DHCP で IP 取得 → explicit_buy (ota::app) ---
        if ui.model.boot.selftest_tick(network_up, ui.display.is_running()) {
            ui.present().await;
        }

        // --- OTA (buy 待ち / 巻き戻し待ちの間は行わない) ---
        if ota_possible
            && network_up
            && ui.model.boot.ota_allowed()
            && ui.model.ota.is_due()
            && let Ok(slots) = ui.model.boot.slots
        {
            ui.model.ota.begin_check();
            let result = app::run_ota_check(&net, bufs, &mut flash, sectors, slots, &mut ui).await;
            ui.model.ota.apply(result);
            ui.present().await;
        }

        // --- 検証済みイメージへの FLASH_UPDATE 再起動 / 巻き戻されたイメージの再試行 ---
        if let Some(version) = ui.model.ota.reboot_due()
            && let Ok(slots) = ui.model.boot.slots
        {
            defmt::info!("reboot(FLASH_UPDATE) into P{} for {}", slots.target.index, version);
            reboot_into_slot(&mut ui, &mut control, slots).await;
        }

        // --- 周辺 AP のパッシブスキャン (10 s ごと。OTA 中は走らない) ---
        if Instant::now() >= next_scan {
            ui.model.aps.clear();
            {
                let mut scanner = control.scan(ScanOptions::default()).await;
                while let Some(bss) = scanner.next().await {
                    merge_ap(&mut ui.model.aps, &bss);
                }
            }
            scan_count = scan_count.wrapping_add(1);
            ui.model.aps.sort_unstable_by_key(|ap| core::cmp::Reverse(ap.rssi));
            defmt::info!("scan #{}: {} APs", scan_count, ui.model.aps.len());
            next_scan = Instant::now() + SCAN_PERIOD;
        }

        // --- ステータス行 ---
        ui.model.status.clear();
        match (&credentials, &link.link) {
            (Ok(creds), Link::Joined) => {
                let ssid = ascii_label::<32>(creds.ssid.as_bytes());
                let own_rssi = ui
                    .model
                    .aps
                    .iter()
                    .find(|ap| ap.ssid() == creds.ssid.as_bytes())
                    .map(|ap| ap.rssi);
                match stack.config_v4() {
                    Some(config) => {
                        let ip = config.address.address().octets();
                        let _ = write!(ui.model.status, "{} {}.{}.{}.{}", ssid, ip[0], ip[1], ip[2], ip[3]);
                    }
                    None => {
                        let _ = write!(ui.model.status, "{} connected, no IP yet", ssid);
                    }
                }
                if let Some(rssi) = own_rssi {
                    let _ = write!(ui.model.status, " {}dBm", rssi);
                }
                ui.model.joined_ssid = Some(creds.ssid.clone());
            }
            (
                Ok(_),
                Link::Disconnected {
                    last_error,
                    next_attempt,
                    ..
                },
            ) => {
                match last_error {
                    Some(code) => {
                        let _ = write!(
                            ui.model.status,
                            "join failed (status {}), retry in {}s",
                            code,
                            secs_until(*next_attempt)
                        );
                    }
                    None => {
                        let _ = ui.model.status.push_str("not connected");
                    }
                }
                ui.model.joined_ssid = None;
            }
            (Err(message), _) | (_, Link::ScanOnly(message)) => {
                let _ = write!(ui.model.status, "{} (scan only)", message);
                ui.model.joined_ssid = None;
            }
        }
        let _ = write!(ui.model.status, "  scan #{}", scan_count);

        ui.present().await;
        Timer::after(TICK).await;
    }
}
