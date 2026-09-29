//! Wi-Fi ステータス表示: SD カードの `wifi.txt` で Wi-Fi に接続し、
//! 周辺 AP の RSSI を 400×96 LCD に棒グラフで表示する。
//!
//! - SD (GPIO SPI: CS=GP26, CMD=GP27, CLK=GP28, DAT0=GP0) のルートにある
//!   `WIFI.TXT` (1 行目 SSID、2 行目パスワード) を読む。
//! - CYW43439 (PWR=GP23, CS=GP25, DIO=GP24, CLK=GP29) を PIO1 SM0 + DMA CH4 で
//!   駆動し、embassy-net の DHCP で IP を取得する。
//! - LCD は `lcd::display` (PIO0 SM0/SM1 + DMA CH0-CH3 の自走リング、ダブルバッファ) で走査。
//! - 約 10 秒ごとにパッシブスキャンし、RSSI 上位 8 件を SSID 付きで描画する。
//! - picotool 用 USB reset interface を公開し、`picotool load -f` で書き換えられる。

#![no_std]
#![no_main]

use core::fmt::Write as _;
use cyw43::{JoinAuth, JoinOptions, PowerManagementMode, ScanOptions};
use cyw43_pio::{DEFAULT_CLOCK_DIVIDER, PioSpi};
use embassy_executor::Spawner;
use embassy_net::StackResources;
use embassy_rp::bind_interrupts;
use embassy_rp::clocks::RoscRng;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::*;
use embassy_rp::pio::{InterruptHandler, Pio};
use embassy_rp::usb::{Driver as UsbDriver, InterruptHandler as UsbInterruptHandler};
use embassy_time::{Duration, Instant, Timer, with_timeout};
use embassy_usb::UsbDevice;
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::mono_font::ascii::FONT_6X10;
use embedded_graphics::pixelcolor::Rgb666;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{PrimitiveStyle, Rectangle};
use embedded_graphics::text::{Baseline, Text};
use embedded_sdmmc::{Mode, VolumeIdx};
use heapless::{String, Vec};
use pico2w_300yen_lcd::lcd::display::{BackBuffer, Display, DisplayPins, FrameIrqHandler};
use pico2w_300yen_lcd::lcd::framebuffer::BLACK;
use pico2w_300yen_lcd::lcd::timing::H_ACTIVE;
use pico2w_300yen_lcd::sdcard::{SdVolumeManager, init_sd, volume_error};
use pico2w_300yen_lcd::usb_reset::build_usb_device;
use static_cell::StaticCell;
use {defmt_rtt as _, panic_probe as _};

// RP2350 bootrom 用 IMAGE_DEF (版数付き) と picotool 用 binary_info を埋め込む
pico2w_300yen_lcd::firmware_image_def!();

bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => InterruptHandler<PIO0>;
    PIO1_IRQ_0 => InterruptHandler<PIO1>;
    DMA_IRQ_1 => FrameIrqHandler;
    USBCTRL_IRQ => UsbInterruptHandler<USB>;
});

// ============================================================
// CYW43439 ファームウェア (Infineon Permissive Binary License)
// ============================================================

static CYW43_FW: &[u8] = include_bytes!("../../firmware/cyw43/43439A0.bin");
static CYW43_CLM: &[u8] = include_bytes!("../../firmware/cyw43/43439A0_clm.bin");

type Cyw43Spi = PioSpi<'static, PIO1, 0, DMA_CH4>;
type Cyw43Runner = cyw43::Runner<'static, Output<'static>, Cyw43Spi>;

#[embassy_executor::task]
async fn cyw43_task(runner: Cyw43Runner) -> ! {
    runner.run().await
}

#[embassy_executor::task]
async fn net_task(mut runner: embassy_net::Runner<'static, cyw43::NetDriver<'static>>) -> ! {
    runner.run().await
}

#[embassy_executor::task]
async fn usb_task(mut device: UsbDevice<'static, UsbDriver<'static, USB>>) -> ! {
    device.run().await
}

// ============================================================
// 動作パラメータ
// ============================================================

/// スキャン周期 (秒)
const SCAN_PERIOD_SECS: u64 = 10;
/// 表示する AP 数 (上位 RSSI)
const MAX_DISPLAY_APS: usize = 8;
/// スキャン結果の保持上限 (SSID ごとに集約)
const MAX_SCAN_APS: usize = 32;
/// DHCP 待ち時間
const DHCP_TIMEOUT: Duration = Duration::from_secs(20);
/// join 失敗時の初回リトライ間隔と上限
const JOIN_RETRY_MIN: Duration = Duration::from_secs(5);
const JOIN_RETRY_MAX: Duration = Duration::from_secs(60);

// ============================================================
// wifi.txt の読み込み
// ============================================================

struct WifiCredentials {
    ssid: String<32>,
    /// 空文字列ならオープンネットワークとして接続する
    password: String<64>,
}

fn read_credentials(volume_mgr: &SdVolumeManager) -> Result<WifiCredentials, &'static str> {
    let volume = volume_mgr.open_volume(VolumeIdx(0)).map_err(volume_error)?;
    let root = volume.open_root_dir().map_err(|_| "ROOT DIR ERROR")?;
    let file = root
        .open_file_in_dir("WIFI.TXT", Mode::ReadOnly)
        .map_err(|_| "wifi.txt not found")?;
    let mut buf = [0u8; 256];
    let mut len = 0;
    while len < buf.len() {
        let count = file
            .read(&mut buf[len..])
            .map_err(|_| "wifi.txt read error")?;
        if count == 0 {
            break;
        }
        len += count;
    }
    parse_credentials(&buf[..len])
}

/// 1 行目 SSID、2 行目パスワード。CR/LF のみ除去し、空行は読み飛ばす。
fn parse_credentials(bytes: &[u8]) -> Result<WifiCredentials, &'static str> {
    let text = core::str::from_utf8(bytes).map_err(|_| "wifi.txt: not UTF-8")?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.lines().filter(|line| !line.is_empty());
    let ssid = lines.next().ok_or("wifi.txt: SSID missing")?;
    let password = lines.next().unwrap_or("");
    if ssid.len() > 32 {
        return Err("wifi.txt: SSID too long");
    }
    if !password.is_empty() && !(8..=63).contains(&password.len()) {
        return Err("wifi.txt: password 8-63 chars");
    }
    let mut credentials = WifiCredentials {
        ssid: String::new(),
        password: String::new(),
    };
    credentials
        .ssid
        .push_str(ssid)
        .map_err(|_| "wifi.txt: SSID too long")?;
    credentials
        .password
        .push_str(password)
        .map_err(|_| "wifi.txt: password too long")?;
    Ok(credentials)
}

// ============================================================
// スキャン結果
// ============================================================

#[derive(Clone, Copy)]
struct ApEntry {
    ssid: [u8; 32],
    ssid_len: u8,
    rssi: i16,
    channel: u8,
}

impl ApEntry {
    fn ssid(&self) -> &[u8] {
        &self.ssid[..usize::from(self.ssid_len).min(32)]
    }
}

/// SSID ごとに最大 RSSI を保持しつつ結果を集約する
fn merge_ap(aps: &mut Vec<ApEntry, MAX_SCAN_APS>, bss: &cyw43::BssInfo) {
    let ssid_len = usize::from(bss.ssid_len).min(32);
    if ssid_len == 0 {
        return; // ステルス AP は表示しない
    }
    let ssid = &bss.ssid[..ssid_len];
    if let Some(existing) = aps.iter_mut().find(|ap| ap.ssid() == ssid) {
        if bss.rssi > existing.rssi {
            existing.rssi = bss.rssi;
            existing.channel = (bss.chanspec & 0xff) as u8;
        }
        return;
    }
    let entry = ApEntry {
        ssid: bss.ssid,
        ssid_len: ssid_len as u8,
        rssi: bss.rssi,
        channel: (bss.chanspec & 0xff) as u8,
    };
    if aps.push(entry).is_err() {
        // 満杯なら最も弱い AP を置き換える
        let weakest = aps
            .iter()
            .enumerate()
            .min_by_key(|(_, ap)| ap.rssi)
            .map(|(index, ap)| (index, ap.rssi));
        if let Some((index, weakest_rssi)) = weakest
            && bss.rssi > weakest_rssi
        {
            aps[index] = entry;
        }
    }
}

// ============================================================
// 描画
// ============================================================

const ROW_HEIGHT: i32 = 10;
const LIST_TOP: i32 = 14;
const SSID_X: i32 = 2;
const SSID_CHARS: usize = 21; // 6px × 21 = 126px
const BAR_X: i32 = 134;
const BAR_WIDTH: i32 = 186;
const RSSI_X: i32 = 326; // "-100dBm ch11" = 12 文字 = 72px → 398px
const RSSI_MIN: i32 = -100;
const RSSI_MAX: i32 = -30;

/// 表示不能なバイトは '?' に置き換えて ASCII に丸める
fn ascii_label<const N: usize>(bytes: &[u8]) -> String<N> {
    let mut label = String::new();
    for &byte in bytes.iter().take(N) {
        let ch = if (0x20..=0x7e).contains(&byte) {
            byte as char
        } else {
            '?'
        };
        if label.push(ch).is_err() {
            break;
        }
    }
    label
}

fn rssi_color(rssi: i16) -> Rgb666 {
    if rssi >= -60 {
        Rgb666::new(0, 63, 0)
    } else if rssi >= -75 {
        Rgb666::new(63, 63, 0)
    } else {
        Rgb666::new(63, 16, 0)
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

/// 1 行目にステータス、2 行目以降に AP 一覧をバックバッファへ描く。
/// 画面への反映は呼び出し側の `display.present().await`。
fn draw_screen(frame: &mut BackBuffer, status: &str, aps: &[ApEntry], joined_ssid: Option<&str>) {
    frame.clear(BLACK);
    let status_color = if joined_ssid.is_some() {
        Rgb666::new(0, 63, 63)
    } else {
        Rgb666::WHITE
    };
    draw_text(frame, status, SSID_X, 2, status_color);
    fill_rect(frame, 0, 12, H_ACTIVE, 1, Rgb666::new(16, 16, 16));

    if aps.is_empty() {
        draw_text(
            frame,
            "(no scan results yet)",
            SSID_X,
            LIST_TOP,
            Rgb666::new(32, 32, 32),
        );
    }

    for (index, ap) in aps.iter().take(MAX_DISPLAY_APS).enumerate() {
        let y = LIST_TOP + index as i32 * ROW_HEIGHT;
        let label: String<SSID_CHARS> = ascii_label(ap.ssid());
        let is_joined = joined_ssid.is_some_and(|ssid| ssid.as_bytes() == ap.ssid());
        let text_color = if is_joined {
            Rgb666::new(0, 63, 63)
        } else {
            Rgb666::WHITE
        };
        draw_text(frame, &label, SSID_X, y, text_color);

        // 棒グラフ: 背景 (暗) + RSSI に比例した長さ
        let rssi = i32::from(ap.rssi).clamp(RSSI_MIN, RSSI_MAX);
        let filled = ((rssi - RSSI_MIN) * BAR_WIDTH / (RSSI_MAX - RSSI_MIN)).max(1) as u32;
        fill_rect(
            frame,
            BAR_X,
            y + 1,
            BAR_WIDTH as u32,
            7,
            Rgb666::new(6, 6, 6),
        );
        fill_rect(frame, BAR_X, y + 1, filled, 7, rssi_color(ap.rssi));

        let mut rssi_text: String<16> = String::new();
        let _ = write!(rssi_text, "{}dBm ch{}", ap.rssi, ap.channel);
        draw_text(frame, &rssi_text, RSSI_X, y, Rgb666::new(48, 48, 48));
    }
}

// ============================================================
// main
// ============================================================

enum Link {
    /// wifi.txt が無い、または不正。スキャンのみ行う。
    ScanOnly(&'static str),
    /// 未接続 (次の join 試行時刻と直前のエラー)
    Disconnected {
        next_attempt: Instant,
        retry: Duration,
        last_error: Option<u32>,
    },
    /// join 済み (IP は stack.config_v4() で確認)
    Joined,
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());

    // --- SD カードから wifi.txt を読む (GPIO SPI は同期処理なので走査開始前に済ませる) ---
    let credentials = match init_sd(p.PIN_0, p.PIN_26, p.PIN_27, p.PIN_28) {
        Ok(volume_mgr) => read_credentials(&volume_mgr),
        Err(message) => Err(message),
    };
    match &credentials {
        Ok(c) => defmt::info!("wifi.txt: SSID={}", c.ssid.as_str()),
        Err(message) => defmt::warn!("wifi.txt: {}", message),
    }

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
    {
        let mut status: String<64> = String::new();
        match &credentials {
            Ok(c) => {
                let _ = write!(
                    status,
                    "Wi-Fi: starting... ({})",
                    ascii_label::<32>(c.ssid.as_bytes())
                );
            }
            Err(message) => {
                let _ = write!(status, "Wi-Fi: {} (scan only)", message);
            }
        }
        draw_screen(display.back(), &status, &[], None);
    }
    display.start(Irqs);

    // --- picotool 用 USB reset interface ---
    let usb = build_usb_device(UsbDriver::new(p.USB, Irqs), "Wi-Fi Status");
    spawner.spawn(usb_task(usb)).unwrap();

    // --- CYW43439 (PIO1 SM0 + DMA CH4; LCD の PIO0 / DMA CH0-3 とは独立) ---
    let pwr = Output::new(p.PIN_23, Level::Low);
    let cs = Output::new(p.PIN_25, Level::High);
    let mut pio1 = Pio::new(p.PIO1, Irqs);
    let spi: Cyw43Spi = PioSpi::new(
        &mut pio1.common,
        pio1.sm0,
        DEFAULT_CLOCK_DIVIDER,
        pio1.irq0,
        cs,
        p.PIN_24,
        p.PIN_29,
        p.DMA_CH4,
    );

    static STATE: StaticCell<cyw43::State> = StaticCell::new();
    let state = STATE.init(cyw43::State::new());
    let (net_device, mut control, runner) = cyw43::new(state, pwr, spi, CYW43_FW).await;
    spawner.spawn(cyw43_task(runner)).unwrap();

    control.init(CYW43_CLM).await;
    control
        .set_power_management(PowerManagementMode::PowerSave)
        .await;
    let mac = control.address().await;
    defmt::info!("CYW43 MAC: {:02x}", mac);

    // --- embassy-net (DHCP) ---
    let seed = RoscRng.next_u64();
    static RESOURCES: StaticCell<StackResources<3>> = StaticCell::new();
    let (stack, net_runner) = embassy_net::new(
        net_device,
        embassy_net::Config::dhcpv4(Default::default()),
        RESOURCES.init(StackResources::new()),
        seed,
    );
    spawner.spawn(net_task(net_runner)).unwrap();

    let mut link = match &credentials {
        Ok(_) => Link::Disconnected {
            next_attempt: Instant::now(),
            retry: JOIN_RETRY_MIN,
            last_error: None,
        },
        Err(message) => Link::ScanOnly(message),
    };
    let mut aps: Vec<ApEntry, MAX_SCAN_APS> = Vec::new();
    let mut scan_count: u32 = 0;
    let mut status: String<80> = String::new();

    loop {
        let cycle_start = Instant::now();

        // --- 接続管理 ---
        if let (Ok(creds), Link::Joined) = (&credentials, &link)
            && !stack.is_link_up()
        {
            defmt::warn!("link down, will rejoin {}", creds.ssid.as_str());
            link = Link::Disconnected {
                next_attempt: Instant::now(),
                retry: JOIN_RETRY_MIN,
                last_error: None,
            };
        }
        if let (
            Ok(creds),
            Link::Disconnected {
                next_attempt,
                retry,
                ..
            },
        ) = (&credentials, &link)
            && Instant::now() >= *next_attempt
        {
            let retry = *retry;
            status.clear();
            let _ = write!(
                status,
                "connecting to {}...",
                ascii_label::<32>(creds.ssid.as_bytes())
            );
            draw_screen(display.back(), &status, &aps, None);
            display.present().await;

            let options = if creds.password.is_empty() {
                JoinOptions::new_open()
            } else {
                let mut options = JoinOptions::new(creds.password.as_bytes());
                options.auth = JoinAuth::Wpa2;
                options
            };
            match control.join(creds.ssid.as_str(), options).await {
                Ok(()) => {
                    defmt::info!("joined {}", creds.ssid.as_str());
                    status.clear();
                    let _ = write!(
                        status,
                        "{}: waiting for DHCP...",
                        ascii_label::<32>(creds.ssid.as_bytes())
                    );
                    draw_screen(display.back(), &status, &aps, Some(creds.ssid.as_str()));
                    display.present().await;
                    if with_timeout(DHCP_TIMEOUT, stack.wait_config_up())
                        .await
                        .is_err()
                    {
                        defmt::warn!("DHCP timeout");
                    }
                    link = Link::Joined;
                }
                Err(error) => {
                    defmt::error!("join failed: status {}", error.status);
                    link = Link::Disconnected {
                        next_attempt: Instant::now() + retry,
                        retry: (retry * 2).min(JOIN_RETRY_MAX),
                        last_error: Some(error.status),
                    };
                }
            }
        }

        // --- 周辺 AP のパッシブスキャン ---
        aps.clear();
        {
            let mut scanner = control.scan(ScanOptions::default()).await;
            while let Some(bss) = scanner.next().await {
                merge_ap(&mut aps, &bss);
            }
        }
        scan_count = scan_count.wrapping_add(1);
        aps.sort_unstable_by_key(|ap| core::cmp::Reverse(ap.rssi));
        defmt::info!("scan #{}: {} APs", scan_count, aps.len());

        // --- ステータス行 ---
        status.clear();
        let joined_ssid = match (&credentials, &link) {
            (Ok(creds), Link::Joined) => {
                let ssid = ascii_label::<32>(creds.ssid.as_bytes());
                let own_rssi = aps
                    .iter()
                    .find(|ap| ap.ssid() == creds.ssid.as_bytes())
                    .map(|ap| ap.rssi);
                match stack.config_v4() {
                    Some(config) => {
                        let ip = config.address.address().octets();
                        let _ = write!(status, "{} {}.{}.{}.{}", ssid, ip[0], ip[1], ip[2], ip[3]);
                    }
                    None => {
                        let _ = write!(status, "{} connected, no IP yet", ssid);
                    }
                }
                if let Some(rssi) = own_rssi {
                    let _ = write!(status, " {}dBm", rssi);
                }
                Some(creds.ssid.as_str())
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
                        let wait = next_attempt
                            .saturating_duration_since(Instant::now())
                            .as_secs();
                        let _ = write!(status, "join failed (status {}), retry in {}s", code, wait);
                    }
                    None => {
                        let _ = write!(status, "not connected");
                    }
                }
                None
            }
            (Err(message), _) | (_, Link::ScanOnly(message)) => {
                let _ = write!(status, "{} (scan only)", message);
                None
            }
        };
        let _ = write!(status, "  scan #{}", scan_count);

        draw_screen(display.back(), &status, &aps, joined_ssid);
        display.present().await;

        Timer::at(cycle_start + Duration::from_secs(SCAN_PERIOD_SECS)).await;
    }
}
