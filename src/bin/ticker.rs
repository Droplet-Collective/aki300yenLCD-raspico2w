//! ネットワーク・ティッカー (v0.3.0〜): NTP 時計 + Open-Meteo 天気 + GitHub の `message.txt` を流す + OTA
//!
//! `wifi_ota` の後継として Release の OTA イメージになる bin。TBYB / 接続管理 / OTA は `ota::app`
//! (wifi_ota と共用) で、この bin は表示と 3 つの取得 (NTP / 天気 / 流れる文字) を持つ。
//! 使い方と `ticker.txt` の書き方は docs/ticker.md。
//!
//! # タスク構成
//!
//! - `main`: SD (`WIFI.TXT` / `TICKER.TXT`) → LCD 開始 → USB → CYW43 → 以後 250 ms 周期のループで
//!   join / DHCP (`LinkManager`)、TBYB の自己診断と buy、OTA 確認、NTP、天気、流れる文字の取得を
//!   **順番に** 行う (HTTPS の TLS バッファは 1 組しか無いので、同時に 2 本は張らない)。
//! - `render_task`: LCD の垂直同期 (`Display::present`、≈60 Hz) ごとに画面全体をバックバッファへ描き直し
//!   (背景の写真のコピー → ガラス板 → 文字)、流れる文字を `scroll` px / フレームで左へ動かす。表示する内容は
//!   `MODEL` (共有モデル) と `slideshow::BG` (背景) から読む。
//!   フラッシュ書き込み中 (数百 ms、割り込み禁止) は描画が止まるが、走査は SRAM の DMA リングで続く。
//! - `slideshow_task`: SD の BMP を読み、背景を切り替える (`ticker::slideshow`)。OTA の確認〜検証中は止まる。
//!
//! # 画面 (400×96、v0.4.0〜: SD の写真の上に重ね描き)
//!
//! 描画はハードウェアに依存しない `ui` モジュール (`src/ui/`) が行い、同じコードをホストの
//! シミュレータ `tools/ui-sim` が PNG / GIF にする (docs/ui-sim.md)。既定の構成 `layout=glass`:
//!
//! ```text
//!  y  4〜35  21:53:44 (DejaVu Sans の AA 数字、影付き)      ┌ 天気のガラス板 (x 234〜396, y 4〜57) ┐
//!  y 40〜53  9月29日(火) (東雲 14 px、影付き)               │ ☁ 19.1°             ⌖東京 │
//!            (左上は背景に薄い暗がりを焼き込む)             │ 晴れ時々くもり              │
//!                                                           │ ▲21.9 ▼18.6 (💧40%)        │
//!  y 74〜92  ┌ 流れる文字 (東雲 14 px) ─────────────────┬ Wi-Fi NTP WX MSG v0.4.0 ┐  ← ガラスの帯
//! ```
//!
//! 状態 3 行 (Wi-Fi / 版数・TBYB / OTA、`FONT_6X10`) は必要なときだけ下の帯の位置に出す
//! (起動 60 s、失敗から 30 s、Wi-Fi が IP を得ていない、OTA のダウンロード〜再起動待ち / 失敗、TBYB の
//! buy 待ち / 失敗。`status=full` で常に、`status=compact` で出さない)。それ以外は帯の右端の小さな表示だけ。
//! 背景の写真は `ticker::slideshow` が SD から読み、切り替えは背景だけを暗くして行う。
//!
//! TBYB の自己診断 (buy 条件) は wifi_ota と同じ「LCD 走査中 + join + DHCP」だけ。NTP / 天気 / 文字の
//! 取得の成否は buy に関係しない (それらは buy が済むまで始めない)。

#![no_std]
#![no_main]

use core::cell::RefCell;
use core::fmt::Write as _;
use core::mem::MaybeUninit;
use core::ptr::addr_of_mut;
use core::sync::atomic::Ordering;

use cyw43::PowerManagementMode;
use embassy_executor::Spawner;
use embassy_rp::bind_interrupts;
use embassy_rp::flash::Flash;
use embassy_rp::peripherals::*;
use embassy_rp::pio::{InterruptHandler, Pio};
use embassy_rp::usb::{Driver as UsbDriver, InterruptHandler as UsbInterruptHandler};
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_time::{Duration, Instant, Timer, with_timeout};
use embassy_usb::UsbDevice;
use heapless::String;
use pico2w_300yen_lcd::boot_trace::{self, Stage};
use pico2w_300yen_lcd::font::shinonome;
use pico2w_300yen_lcd::image_def::{FIRMWARE_VERSION, TBYB};
use pico2w_300yen_lcd::lcd::display::{BackBuffer, Display, DisplayPins, FrameIrqHandler};
use pico2w_300yen_lcd::ota::app::{
    self, BootStatus, LinkManager, LinkUi, Net, NetBuffers, OtaPhase, OtaState, OtaUi, TcpState, Tone,
};
use pico2w_300yen_lcd::ota::http::{self, BodySink};
use pico2w_300yen_lcd::ota::slot::{OtaFlash, SectorBuffers, Slots, slot_label};
use pico2w_300yen_lcd::ota::OtaError;
use pico2w_300yen_lcd::sdcard::{ReadError, SdVolumeManager, init_sd, read_root_file};
use pico2w_300yen_lcd::ticker::civil;
use pico2w_300yen_lcd::ticker::config::{LayoutName, StatusMode, TickerConfig};
use pico2w_300yen_lcd::ticker::slideshow::{self, SlideConfig};
use pico2w_300yen_lcd::ticker::sntp::{self, SntpError};
use pico2w_300yen_lcd::ticker::sntp_net::{self, SntpBuffers, Sync};
use pico2w_300yen_lcd::ticker::weather::{self, Weather};
use pico2w_300yen_lcd::ui::canvas::Canvas;
use pico2w_300yen_lcd::ui::screen::{self, Clock, Layout, StatusView, Tone as UiTone, View, WeatherView};
use pico2w_300yen_lcd::usb_reset::build_usb_device;
use pico2w_300yen_lcd::wifi::{self, Cyw43Pins, WifiCredentials, ascii_label, read_credentials};
use defmt_rtt as _;

// RP2350 bootrom 用 IMAGE_DEF (版数付き、--features tbyb で TBYB フラグ) と picotool 用 binary_info
pico2w_300yen_lcd::firmware_image_def!();

/// panic: 段階と行番号を SCRATCH に記録 → defmt → `udf` (wifi_ota と同じ)
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    boot_trace::fault(Stage::Panic, info.location().map_or(0, |l| l.line()));
    defmt::error!("{}", defmt::Display2Format(info));
    cortex_m::asm::udf()
}

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
// 動作パラメータ
// ============================================================

const HEAP_SIZE: usize = 8 * 1024;
static mut HEAP_MEM: [MaybeUninit<u8>; HEAP_SIZE] = [MaybeUninit::uninit(); HEAP_SIZE];

/// main ループの周期
const TICK: Duration = Duration::from_millis(250);
/// NTP の再同期周期と、失敗時の再試行 (倍々、上限 10 分)
const NTP_RESYNC: Duration = Duration::from_secs(6 * 3600);
const NTP_RETRY_MIN: Duration = Duration::from_secs(30);
const NTP_RETRY_MAX: Duration = Duration::from_secs(600);
const NTP_TIMEOUT: Duration = Duration::from_secs(5);
/// 天気の更新周期 / 失敗時の再試行
const WEATHER_REFRESH: Duration = Duration::from_secs(30 * 60);
const WEATHER_RETRY: Duration = Duration::from_secs(120);
/// 流れる文字の更新周期 / 失敗時の再試行
const MESSAGE_REFRESH: Duration = Duration::from_secs(5 * 60);
const MESSAGE_RETRY: Duration = Duration::from_secs(60);
/// 小さな HTTP(S) 取得 (天気 / 文字) の全体タイムアウト
const FETCH_TIMEOUT: Duration = Duration::from_secs(20);
/// 起動診断 (前回の TBYB 記録 / 起動種別) を状態行 1 に出す時間
const DIAG_SHOW: Duration = Duration::from_secs(60);
/// `ticker.txt` の注意を状態行 1 に出す時間 (起動診断の後)
const NOTE_SHOW: Duration = Duration::from_secs(90);
/// 流れる文字の最大長 (バイト。UTF-8 で日本語 ≈ 170 文字)
const MESSAGE_MAX: usize = 512;
/// `TICKER.TXT` の最大長
const CONFIG_MAX: usize = 512;

// ============================================================
// static 配置のバッファ (BSS)
// ============================================================

static mut NET_BUFFERS: NetBuffers = NetBuffers::new();
static mut TCP_STATE: TcpState = TcpState::new();
static mut SECTOR_BUFFERS: SectorBuffers = SectorBuffers::new();
static mut SNTP_BUFFERS: SntpBuffers = SntpBuffers::new();
/// 天気の JSON / 文字の本文の受信先 (どちらも同時には使わない)
static mut BODY: [u8; weather::BODY_MAX] = [0; weather::BODY_MAX];

// ============================================================
// 共有モデル (main が書き、render_task が毎フレーム読む)
// ============================================================

/// 表示する内容。文字列は main が組み立てて入れる (描画側は色と位置だけを決める)
struct Shared {
    /// 状態行 1: Wi-Fi (SSID / IP または接続中の文言)
    wifi: String<48>,
    wifi_tone: Tone,
    /// 状態行 1 の右側: NTP / 天気 / 文字の取得状況
    ntp: String<20>,
    wx: String<20>,
    msg: String<20>,
    /// 状態行 1 の代わりに出す起動診断 (前回の TBYB 記録 → 起動種別)。`diag_until` まで
    diag: String<80>,
    diag_tone: Tone,
    diag_until: Option<Instant>,
    /// `ticker.txt` の注意 (無い / 読めない)。`note_until` まで (起動診断の後)
    note: String<80>,
    note_until: Option<Instant>,
    /// 状態行 2: `ticker v0.3.0 [via OTA]` (マゼンタ) + ` slot B TBYB:...` (TBYB の色)
    ident_head: String<32>,
    ident_rest: String<64>,
    ident_tone: Tone,
    /// 状態行 3: `OTA: ...`
    ota: String<80>,
    ota_tone: Tone,
    progress: Option<(u32, u32)>,
    /// 時刻 (NTP 同期済みなら Some)
    clock: Option<Sync>,
    tz_offset_secs: i32,
    /// 天気
    weather: Option<Weather>,
    place: String<32>,
    /// 流れる文字と、その世代 (変わったらスクロール位置を右端に戻す)
    message: String<MESSAGE_MAX>,
    message_gen: u32,
    scroll_px: u8,
    /// 画面構成と状態 3 行の出し方 (ticker.txt)
    layout: Layout,
    status_mode: StatusMode,
    /// 起動時刻 (状態 3 行を 60 s 出す)
    boot_at: Instant,
    /// 失敗から 30 s は状態 3 行を出す
    alert_until: Option<Instant>,
}

impl Shared {
    const fn new() -> Self {
        Self {
            wifi: String::new(),
            wifi_tone: Tone::Normal,
            ntp: String::new(),
            wx: String::new(),
            msg: String::new(),
            diag: String::new(),
            diag_tone: Tone::Muted,
            diag_until: None,
            note: String::new(),
            note_until: None,
            ident_head: String::new(),
            ident_rest: String::new(),
            ident_tone: Tone::Muted,
            ota: String::new(),
            ota_tone: Tone::Muted,
            progress: None,
            clock: None,
            tz_offset_secs: 9 * 3600,
            weather: None,
            place: String::new(),
            message: String::new(),
            message_gen: 0,
            scroll_px: 1,
            layout: Layout::Glass,
            status_mode: StatusMode::Auto,
            boot_at: Instant::from_ticks(0),
            alert_until: None,
        }
    }
}

/// main と render_task は同じ thread-mode executor で動き、割り込みからは触らないので `ThreadModeRawMutex`
/// (ロック中も割り込みを止めない。描画 1 フレーム分 ≈ 1 ms の間 cyw43 / タイマ割り込みを遅らせないため)。
static MODEL: Mutex<ThreadModeRawMutex, RefCell<Shared>> = Mutex::new(RefCell::new(Shared::new()));

fn with_model<R>(f: impl FnOnce(&mut Shared) -> R) -> R {
    MODEL.lock(|m| f(&mut m.borrow_mut()))
}

/// 失敗を記録する (状態 3 行を `ALERT_SHOW` の間出す)
fn raise_alert(m: &mut Shared) {
    m.alert_until = Some(Instant::now() + ALERT_SHOW);
}

/// ota::app への表示口: 接続状況と OTA の途中経過を共有モデルへ書く (描画は render_task が毎フレーム行う)
struct ModelUi<'a> {
    ota: &'a mut OtaState,
    slots: Result<Slots, OtaError>,
}

impl ModelUi<'_> {
    fn publish_ota(&self) {
        let mut line: String<80> = String::new();
        let (tone, progress) = self.ota.write_line(&mut line, &self.slots);
        // 確認〜検証の間はスライドショーの SD 読み込みを止める (フラッシュ書き込みと重ねない)
        let active = matches!(
            self.ota.phase,
            OtaPhase::Checking | OtaPhase::Downloading { .. } | OtaPhase::Verifying { .. }
        );
        slideshow::PAUSE.store(active, Ordering::Relaxed);
        with_model(|m| {
            if tone == Tone::Error && m.ota_tone != Tone::Error {
                raise_alert(m);
            }
            m.ota = line;
            m.ota_tone = tone;
            m.progress = progress;
        });
    }
}

impl OtaUi for ModelUi<'_> {
    async fn ota_phase(&mut self, phase: OtaPhase) {
        self.ota.phase = phase;
        self.publish_ota();
    }
}

impl LinkUi for ModelUi<'_> {
    async fn link_status(&mut self, text: &str, _joined_ssid: Option<&String<32>>) {
        with_model(|m| {
            m.wifi.clear();
            let _ = m.wifi.push_str(text);
            m.wifi_tone = Tone::Normal;
        });
    }
}

// ============================================================
// 描画 (render_task)
// ============================================================

/// 起動から状態 3 行を出しておく時間
const STATUS_BOOT_SHOW: Duration = Duration::from_secs(60);
/// 取得 / 接続 / OTA の失敗から状態 3 行を出しておく時間
const ALERT_SHOW: Duration = Duration::from_secs(30);

fn ui_tone(tone: Tone) -> UiTone {
    match tone {
        Tone::Muted => UiTone::Muted,
        Tone::Normal => UiTone::Normal,
        Tone::Ok => UiTone::Ok,
        Tone::Busy => UiTone::Busy,
        Tone::Error => UiTone::Error,
    }
}

/// `ok` / `ok s1` / `ok(http)` → 良好、`---` → 未取得、`syncing` / `fetching` → 処理中、他は失敗
fn job_tone(state: &str) -> UiTone {
    if state.starts_with("ok") {
        UiTone::Ok
    } else if state == "---" || state.is_empty() {
        UiTone::Muted
    } else if state == "syncing" || state == "fetching" {
        UiTone::Busy
    } else {
        UiTone::Error
    }
}

/// 状態 3 行を出すか (docs/ticker.md「状態表示」): 起動 60 s、失敗から 30 s、起動診断 / 設定の注意の表示中、
/// Wi-Fi が IP を得ていない間、OTA のダウンロード〜再起動待ちと OTA 失敗、TBYB の buy 待ち / 失敗
fn status_expanded(m: &Shared, now: Instant) -> bool {
    match m.status_mode {
        StatusMode::Full => return true,
        StatusMode::Compact => return false,
        StatusMode::Auto => {}
    }
    now < m.boot_at + STATUS_BOOT_SHOW
        || m.alert_until.is_some_and(|t| now < t)
        || (m.diag_until.is_some_and(|t| now < t) && !m.diag.is_empty())
        || (m.note_until.is_some_and(|t| now < t) && !m.note.is_empty())
        || m.wifi_tone != Tone::Ok
        || m.progress.is_some()
        || m.ota_tone == Tone::Error
        || matches!(m.ident_tone, Tone::Busy | Tone::Error)
}

/// 共有モデル → 画面 (背景 + 重ね描き)。`line1` / `date` は呼び出し側の作業用
fn draw_screen(frame: &mut BackBuffer, bg: &[u16], m: &Shared, scroll_x: i32) {
    let now = Instant::now();
    let expanded = status_expanded(m, now);

    // --- 状態行 1: 起動診断 → ticker.txt の注意 → Wi-Fi / NTP / WX / MSG ---
    let mut line1: String<96> = String::new();
    let line1_tone = if m.diag_until.is_some_and(|until| now < until) && !m.diag.is_empty() {
        let _ = line1.push_str(&m.diag);
        ui_tone(m.diag_tone)
    } else if m.note_until.is_some_and(|until| now < until) && !m.note.is_empty() {
        let _ = line1.push_str(&m.note);
        UiTone::Busy
    } else {
        let _ = write!(line1, "{} | NTP {} | WX {} | MSG {}", m.wifi, m.ntp, m.wx, m.msg);
        ui_tone(m.wifi_tone)
    };
    let mut version: String<16> = String::new();
    let _ = write!(version, "v{}", FIRMWARE_VERSION);

    let clock = m.clock.map(|sync| {
        let dt = civil::from_unix(sync.now_unix(), m.tz_offset_secs);
        Clock {
            year: dt.year,
            month: dt.month,
            day: dt.day,
            weekday: dt.weekday,
            hour: dt.hour,
            minute: dt.minute,
            second: dt.second,
        }
    });
    let view = View {
        clock,
        place: &m.place,
        weather: m.weather.map(|w| WeatherView {
            temperature: w.temperature,
            code: w.code,
            condition: w.condition_ja(),
            max: w.max,
            min: w.min,
            rain_pct: w.rain_pct,
        }),
        message: &m.message,
        scroll_x,
        status: StatusView {
            expanded,
            line1: &line1,
            line1_tone,
            ident_head: &m.ident_head,
            ident_rest: &m.ident_rest,
            ident_tone: ui_tone(m.ident_tone),
            ota: &m.ota,
            ota_tone: ui_tone(m.ota_tone),
            progress: m.progress,
            wifi: match m.wifi_tone {
                Tone::Ok => UiTone::Ok,
                Tone::Error => UiTone::Error,
                _ => UiTone::Busy,
            },
            ntp: job_tone(&m.ntp),
            wx: job_tone(&m.wx),
            msg: job_tone(&m.msg),
            version: &version,
        },
    };
    let mut canvas = Canvas::new(&mut frame.data);
    screen::render(&mut canvas, bg, slideshow::LEVEL.load(Ordering::Relaxed), &view, m.layout);
}

/// 毎フレーム (LCD の垂直同期ごと) 画面を描き直し、流れる文字を動かす。
///
/// 流れる文字の位置は LCD のフレーム番号で進める (SD の読み込みなどで描画が 1 フレーム遅れても、
/// 速さは変わらず 2 px 飛ぶだけ)。描き終えるたびに `slideshow::FRAME_SLOT` で SD の読み込みに番を渡す。
#[embassy_executor::task]
async fn render_task(mut display: Display) {
    let mut message_gen = u32::MAX;
    let mut message_width: i32 = 0;
    let mut scroll_x = 0;
    let mut last_frame = Display::frame_count();
    let mut render_us_max: u64 = 0;
    let mut stats_at = Instant::now();
    loop {
        let started = Instant::now();
        let frame_now = Display::frame_count();
        let elapsed_frames = frame_now.wrapping_sub(last_frame).clamp(1, 8) as i32;
        last_frame = frame_now;
        MODEL.lock(|cell| {
            let m = cell.borrow();
            let (_, scroll_w) = m.layout.scroll_area(false);
            if m.message_gen != message_gen {
                message_gen = m.message_gen;
                message_width = shinonome::text_width(&m.message) as i32;
                scroll_x = scroll_w;
            } else if message_width > 0 {
                scroll_x -= i32::from(m.scroll_px.max(1)) * elapsed_frames;
                if scroll_x + message_width < 0 {
                    scroll_x = scroll_w;
                }
            }
            slideshow::BG.lock(|bg| draw_screen(display.back(), &bg.borrow()[..], &m, scroll_x));
        });
        let spent = started.elapsed().as_micros();
        render_us_max = render_us_max.max(spent);
        if stats_at.elapsed() >= Duration::from_secs(30) {
            defmt::info!("render: max {} us per frame (last 30 s), frame {}", render_us_max, frame_now);
            render_us_max = 0;
            stats_at = Instant::now();
        }
        slideshow::FRAME_SLOT.signal(());
        display.present().await;
    }
}

// ============================================================
// 取得 (NTP / 天気 / 文字)
// ============================================================

/// 本文を固定長バッファへ受ける (溢れた分は捨てて `overflow` を立てる)
struct BufSink<'a> {
    buf: &'a mut [u8],
    len: usize,
    overflow: bool,
}

impl BodySink for BufSink<'_> {
    async fn push(&mut self, data: &[u8]) -> Result<(), OtaError> {
        let room = self.buf.len() - self.len;
        let n = data.len().min(room);
        self.buf[self.len..self.len + n].copy_from_slice(&data[..n]);
        self.len += n;
        if n < data.len() {
            self.overflow = true;
        }
        Ok(())
    }
}

/// `url` を GET して本文を `out` へ。戻り値は (HTTP ステータス, 受信長, 溢れたか)
async fn fetch_small(net: &Net<'_>, bufs: &mut NetBuffers, url: &str, out: &mut [u8]) -> Result<(u16, usize, bool), OtaError> {
    bufs.url.clear();
    bufs.url.push_str(url).map_err(|_| OtaError::LocationTooLong)?;
    let mut client = net.client(&mut bufs.tls_rx, &mut bufs.tls_tx);
    let mut sink = BufSink {
        buf: out,
        len: 0,
        overflow: false,
    };
    let fetched = with_timeout(
        FETCH_TIMEOUT,
        http::fetch(&mut client, &mut bufs.url, &mut bufs.http_rx, &mut bufs.chunk, &mut sink),
    )
    .await
    .map_err(|_| OtaError::Timeout)??;
    Ok((fetched.status, sink.len, sink.overflow))
}

fn short_error(error: OtaError) -> String<20> {
    let mut s = String::new();
    match error {
        OtaError::HttpStatus(code) => {
            let _ = write!(s, "HTTP {}", code);
        }
        OtaError::Dns => {
            let _ = s.push_str("DNS fail");
        }
        OtaError::Tls => {
            let _ = s.push_str("TLS fail");
        }
        OtaError::Timeout => {
            let _ = s.push_str("timeout");
        }
        OtaError::Network => {
            let _ = s.push_str("net err");
        }
        OtaError::HttpHeaderTooLong | OtaError::HttpCodec | OtaError::HttpProtocol => {
            let _ = s.push_str("bad HTTP");
        }
        _ => {
            let _ = s.push_str("error");
        }
    }
    s
}

/// 取得 1 種類の予定 (次回時刻と失敗回数)
struct Job {
    next: Instant,
    failures: u32,
}

impl Job {
    fn new() -> Self {
        Self {
            next: Instant::now(),
            failures: 0,
        }
    }

    fn due(&self) -> bool {
        Instant::now() >= self.next
    }

    fn ok(&mut self, interval: Duration) {
        self.failures = 0;
        self.next = Instant::now() + interval;
    }

    fn failed(&mut self, retry: Duration) {
        self.failures = self.failures.saturating_add(1);
        self.next = Instant::now() + retry;
    }
}

/// NTP: nict → 予備 pool.ntp.org。成功したら 6 h 後、失敗なら 30 s → 最大 10 min 後に再試行
async fn do_ntp(stack: embassy_net::Stack<'static>, sntp_bufs: &mut SntpBuffers, job: &mut Job) {
    with_model(|m| {
        m.ntp.clear();
        let _ = m.ntp.push_str("syncing");
    });
    let mut result: Result<Sync, SntpError> = Err(SntpError::Timeout);
    for host in [sntp::PRIMARY_HOST, sntp::FALLBACK_HOST] {
        result = sntp_net::query(stack, sntp_bufs, host, NTP_TIMEOUT).await;
        if result.is_ok() {
            break;
        }
        defmt::warn!("SNTP {} failed: {}", host, result.as_ref().err().map(|e| e.label()));
    }
    match result {
        Ok(sync) => {
            job.ok(NTP_RESYNC);
            with_model(|m| {
                m.clock = Some(sync);
                m.ntp.clear();
                let _ = write!(m.ntp, "ok s{}", sync.stratum);
            });
        }
        Err(e) => {
            let retry = (NTP_RETRY_MIN * 2u32.pow(job.failures.min(5))).min(NTP_RETRY_MAX);
            job.failed(retry);
            with_model(|m| {
                m.ntp.clear();
                let _ = m.ntp.push_str(e.label());
                raise_alert(m);
            });
        }
    }
}

/// 天気: HTTPS (TLS 1.3)。TLS が通らなければ次回から平文 HTTP (Open-Meteo は http も受ける)
async fn do_weather(net: &Net<'_>, bufs: &mut NetBuffers, body: &mut [u8], config: &TickerConfig, plain_http: &mut bool, job: &mut Job) {
    with_model(|m| {
        m.wx.clear();
        let _ = m.wx.push_str("fetching");
    });
    let url = weather::request_url(if *plain_http { "http" } else { "https" }, config.lat, config.lon);
    let result = fetch_small(net, bufs, &url, body).await;
    let outcome: Result<Weather, String<20>> = match result {
        Ok((200, len, false)) => Weather::parse(&body[..len]).ok_or_else(|| {
            let mut s = String::new();
            let _ = s.push_str("bad JSON");
            s
        }),
        Ok((200, _, true)) => {
            let mut s = String::new();
            let _ = s.push_str("too long");
            Err(s)
        }
        Ok((code, _, _)) => Err(short_error(OtaError::HttpStatus(code))),
        Err(OtaError::Tls) if !*plain_http => {
            defmt::warn!("weather: TLS failed, falling back to plain http next time");
            *plain_http = true;
            Err(short_error(OtaError::Tls))
        }
        Err(e) => Err(short_error(e)),
    };
    match outcome {
        Ok(w) => {
            defmt::info!("weather: {} C code {} max {:?} min {:?} rain {:?}", w.temperature, w.code, w.max, w.min, w.rain_pct);
            job.ok(WEATHER_REFRESH);
            with_model(|m| {
                m.weather = Some(w);
                m.wx.clear();
                let _ = m.wx.push_str(if *plain_http { "ok(http)" } else { "ok" });
            });
        }
        Err(text) => {
            job.failed(WEATHER_RETRY);
            with_model(|m| {
                m.wx = text;
                raise_alert(m);
            });
        }
    }
}

/// 流れる文字: `message_url` (既定はこのリポジトリの ticker/message.txt) を取得。変わったときだけ世代を進める
async fn do_message(net: &Net<'_>, bufs: &mut NetBuffers, body: &mut [u8], config: &TickerConfig, job: &mut Job) {
    with_model(|m| {
        m.msg.clear();
        let _ = m.msg.push_str("fetching");
    });
    let result = fetch_small(net, bufs, &config.message_url, &mut body[..MESSAGE_MAX]).await;
    match result {
        Ok((200, len, overflow)) => {
            // UTF-8 として正しい範囲だけ使い、末尾の改行 / 空白を除く。改行は空白に置き換える (1 行に流す)
            let valid = match core::str::from_utf8(&body[..len]) {
                Ok(s) => s,
                Err(e) => core::str::from_utf8(&body[..e.valid_up_to()]).unwrap_or(""),
            };
            let mut text: String<MESSAGE_MAX> = String::new();
            for ch in valid.chars() {
                let ch = if ch == '\n' || ch == '\r' || ch == '\t' { ' ' } else { ch };
                if text.push(ch).is_err() {
                    break;
                }
            }
            let trimmed = text.trim();
            let mut message: String<MESSAGE_MAX> = String::new();
            let _ = message.push_str(trimmed);
            defmt::info!("message: {} bytes{}", message.len(), if overflow { " (truncated)" } else { "" });
            job.ok(MESSAGE_REFRESH);
            with_model(|m| {
                if m.message != message {
                    m.message = message;
                    m.message_gen = m.message_gen.wrapping_add(1);
                }
                m.msg.clear();
                let _ = m.msg.push_str(if overflow { "ok(cut)" } else { "ok" });
            });
        }
        Ok((code, _, _)) => {
            job.failed(MESSAGE_RETRY);
            with_model(|m| {
                m.msg = short_error(OtaError::HttpStatus(code));
                raise_alert(m);
            });
        }
        Err(e) => {
            job.failed(MESSAGE_RETRY);
            with_model(|m| {
                m.msg = short_error(e);
                raise_alert(m);
            });
        }
    }
}

// ============================================================
// main
// ============================================================

/// 状態行 2 (版数 / 区画 / TBYB) を共有モデルへ
fn publish_ident(boot: &BootStatus) {
    let mut head: String<32> = String::new();
    let _ = write!(head, "ticker v{}", FIRMWARE_VERSION);
    if boot.is_ota_boot() {
        let _ = head.push_str(" via OTA");
    }
    let mut rest: String<64> = String::new();
    let tone = boot.write_tbyb_line(&mut rest, !TBYB);
    with_model(|m| {
        m.ident_head = head;
        m.ident_rest = rest;
        m.ident_tone = tone;
    });
}

/// 起動診断 (前回の TBYB 記録があればそれ、無ければ起動種別) を状態行 1 に `DIAG_SHOW` の間出す
fn publish_diag(boot: &BootStatus) {
    let mut line: String<80> = String::new();
    let tone = if let Some(tone) = boot.write_prev_trace_line(&mut line) {
        tone
    } else if boot.show_boot_line() {
        boot.write_boot_line(&mut line);
        Tone::Muted
    } else {
        return;
    };
    with_model(|m| {
        m.diag = line;
        m.diag_tone = tone;
        m.diag_until = Some(Instant::now() + DIAG_SHOW);
    });
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());
    // Safety: HEAP_MEM は他から参照されない。init は 1 回だけ。
    unsafe { pico2w_300yen_lcd::heap::init(&mut *addr_of_mut!(HEAP_MEM)) };

    let mut boot = BootStatus::collect();
    defmt::info!(
        "ticker v{} tbyb={} boot={:?} slots={:?}",
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

    // --- SD カード: wifi.txt と ticker.txt (GPIO SPI は同期処理なので走査開始前に済ませる) ---
    let mut config = TickerConfig::default();
    let mut config_note: String<80> = String::new();
    let mut sd: Option<SdVolumeManager> = None;
    let credentials: Result<WifiCredentials, &'static str> = match init_sd(p.PIN_0, p.PIN_26, p.PIN_27, p.PIN_28) {
        Ok(volume_mgr) => {
            let creds = read_credentials(&volume_mgr);
            let mut buf = [0u8; CONFIG_MAX];
            match read_root_file(&volume_mgr, "TICKER.TXT", &mut buf) {
                Ok(len) => {
                    let (parsed, any) = TickerConfig::parse(&buf[..len]);
                    config = parsed;
                    if !any {
                        let _ = config_note.push_str("ticker.txt: no valid keys, using Tokyo defaults");
                    }
                }
                Err(ReadError::NotFound) => {
                    let _ = config_note.push_str("ticker.txt not found, using Tokyo (35.6812,139.7671 UTC+9)");
                }
                Err(ReadError::Other(e)) => {
                    let _ = write!(config_note, "ticker.txt: {}, using Tokyo defaults", e);
                }
            }
            // 背景の写真 (スライドショー) は走査開始後に別タスクが読む
            sd = Some(volume_mgr);
            creds
        }
        Err(message) => {
            let _ = write!(config_note, "SD: {} (ticker.txt skipped, using Tokyo)", message);
            Err(message)
        }
    };
    match &credentials {
        Ok(c) => defmt::info!("wifi.txt: SSID={}", c.ssid.as_str()),
        Err(message) => defmt::warn!("wifi.txt: {}", message),
    }
    defmt::info!(
        "ticker.txt: lat {} lon {} tz {} s place {} scroll {} slide {} s images '{}' sdfast {} note '{}'",
        config.lat,
        config.lon,
        config.tz_offset_secs,
        config.place.as_str(),
        config.scroll_px,
        config.slide_secs,
        config.images.as_str(),
        config.sd_fast,
        config_note.as_str()
    );
    let layout = match config.layout {
        LayoutName::Glass => Layout::Glass,
        LayoutName::Dock => Layout::Dock,
        LayoutName::Classic => Layout::Classic,
    };
    boot_trace::stage(Stage::SdRead);

    // --- 共有モデルの初期値 ---
    let mut ota = OtaState::new();
    match &credentials {
        Ok(c) => {
            with_model(|m| {
                let _ = write!(m.wifi, "Wi-Fi: starting... ({})", ascii_label::<32>(c.ssid.as_bytes()));
            });
        }
        Err(message) => {
            with_model(|m| {
                let _ = write!(m.wifi, "Wi-Fi: {}", message);
                m.wifi_tone = Tone::Error;
            });
            ota.phase = OtaPhase::Disabled(message);
        }
    }
    if let Err(e) = &boot.slots {
        ota.phase = OtaPhase::Disabled(e.label());
    }
    with_model(|m| {
        m.tz_offset_secs = config.tz_offset_secs;
        m.place = config.place.clone();
        m.scroll_px = config.scroll_px;
        m.layout = layout;
        m.status_mode = config.status;
        m.boot_at = Instant::now();
        if !config_note.is_empty() {
            m.note = config_note.clone();
            m.note_until = Some(Instant::now() + NOTE_SHOW);
        }
        let _ = m.ntp.push_str("---");
        let _ = m.wx.push_str("---");
        let _ = m.msg.push_str("---");
    });
    publish_ident(&boot);
    publish_diag(&boot);
    ModelUi {
        ota: &mut ota,
        slots: boot.slots,
    }
    .publish_ota();

    // --- LCD: 初期画面を描いてから走査を開始し、以後は render_task が毎フレーム描く ---
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
    // 背景は写真を読むまで既定のグラデーション
    slideshow::fill_default(layout);
    MODEL.lock(|cell| {
        let m = cell.borrow();
        slideshow::BG.lock(|bg| draw_screen(display.back(), &bg.borrow()[..], &m, layout.scroll_area(false).1));
    });
    display.start(Irqs);
    boot_trace::stage(Stage::DisplayStarted);
    spawner.spawn(render_task(display)).unwrap();
    if let Some(volume_mgr) = sd {
        spawner
            .spawn(slideshow::slideshow_task(
                volume_mgr,
                SlideConfig {
                    interval: Duration::from_secs(u64::from(config.slide_secs)),
                    images: config.images.clone(),
                    layout,
                    sd_fast: config.sd_fast,
                },
            ))
            .unwrap();
    }

    // --- picotool 用 USB reset interface ---
    let usb = build_usb_device(UsbDriver::new(p.USB, Irqs), "Network Ticker");
    spawner.spawn(usb_task(usb)).unwrap();

    // --- CYW43439 + embassy-net (DHCP) ---
    boot_trace::stage(Stage::WifiPowerCycle);
    with_model(|m| {
        m.wifi.clear();
        let _ = write!(m.wifi, "Wi-Fi: power cycle ({} ms) + init...", wifi::CYW43_POWER_OFF_MS);
    });
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
    let sntp_bufs = unsafe { &mut *addr_of_mut!(SNTP_BUFFERS) };
    let body = unsafe { &mut *addr_of_mut!(BODY) };
    let tcp_state = unsafe { &*addr_of_mut!(TCP_STATE) };
    let net = Net::new(stack, tcp_state);

    let mut link = LinkManager::new(&credentials);
    let ota_possible = credentials.is_ok() && boot.slots.is_ok();
    let mut ntp_job = Job::new();
    let mut weather_job = Job::new();
    let mut message_job = Job::new();
    let mut weather_plain_http = false;

    loop {
        // --- 接続管理 (join → DHCP → 通らなければ離脱して再 join。ota::app::LinkManager) ---
        let network_up = {
            let mut ui = ModelUi {
                ota: &mut ota,
                slots: boot.slots,
            };
            link.step(&mut control, stack, &credentials, &mut ui).await
        };
        if network_up {
            ota.schedule_first_check();
        }

        // --- TBYB: 自己診断 = LCD 走査中 + join + DHCP → explicit_buy (ota::app)。取得の成否は見ない ---
        boot.selftest_tick(network_up, true);

        // buy が済む (または TBYB でない) まで OTA も取得も始めない (buy を遅らせないため)
        let fetch_allowed = network_up && boot.ota_allowed();

        // --- OTA (wifi_ota と同じ手順) ---
        if ota_possible
            && fetch_allowed
            && ota.is_due()
            && let Ok(slots) = boot.slots
        {
            ota.begin_check();
            let mut ui = ModelUi {
                ota: &mut ota,
                slots: boot.slots,
            };
            let result = app::run_ota_check(&net, bufs, &mut flash, sectors, slots, &mut ui).await;
            ui.ota.apply(result);
            ui.publish_ota();
        } else if fetch_allowed && ntp_job.due() {
            // --- NTP (UDP。TLS バッファは使わない) ---
            do_ntp(stack, sntp_bufs, &mut ntp_job).await;
        } else if fetch_allowed && weather_job.due() {
            do_weather(&net, bufs, body, &config, &mut weather_plain_http, &mut weather_job).await;
        } else if fetch_allowed && message_job.due() {
            do_message(&net, bufs, body, &config, &mut message_job).await;
        }

        // --- 検証済みイメージへの FLASH_UPDATE 再起動 / 巻き戻されたイメージの再試行 ---
        if let Some(version) = ota.reboot_due()
            && let Ok(slots) = boot.slots
        {
            defmt::info!("reboot(FLASH_UPDATE) into P{} for {}", slots.target.index, version);
            with_model(|m| {
                m.wifi.clear();
                let _ = write!(
                    m.wifi,
                    "rebooting into slot {} (P{})... wifi off",
                    slot_label(&slots.target),
                    slots.target.index
                );
                m.wifi_tone = Tone::Busy;
            });
            Timer::after(Duration::from_millis(50)).await; // 1 フレーム描かせる
            app::reboot_into_slot(&mut control, slots).await;
        }

        // --- 状態行 (Wi-Fi / 版数・TBYB / OTA) を更新 ---
        if link.is_joined() {
            with_model(|m| {
                m.wifi.clear();
                if let Ok(creds) = &credentials {
                    let _ = write!(m.wifi, "{}", ascii_label::<32>(creds.ssid.as_bytes()));
                }
                match stack.config_v4() {
                    Some(cfg) => {
                        let ip = cfg.address.address().octets();
                        let _ = write!(m.wifi, " {}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
                        m.wifi_tone = Tone::Ok;
                    }
                    None => {
                        let _ = m.wifi.push_str(" no IP yet");
                        m.wifi_tone = Tone::Normal;
                    }
                }
            });
        } else if let Ok(_) = &credentials
            && let app::Link::Disconnected {
                last_error, next_attempt, ..
            } = &link.link
        {
            with_model(|m| {
                m.wifi.clear();
                match last_error {
                    Some(code) => {
                        let _ = write!(m.wifi, "join failed ({}), retry in {}s", code, app::secs_until(*next_attempt));
                        if m.wifi_tone != Tone::Error {
                            raise_alert(m);
                        }
                        m.wifi_tone = Tone::Error;
                    }
                    None => {
                        if !m.wifi.starts_with("Wi-Fi") && !m.wifi.contains("...") {
                            let _ = m.wifi.push_str("not connected");
                            m.wifi_tone = Tone::Normal;
                        }
                    }
                }
            });
        }
        publish_ident(&boot);
        ModelUi {
            ota: &mut ota,
            slots: boot.slots,
        }
        .publish_ota();

        // --- 背景の写真が読めなかったら状態行 1 に出す ---
        if let Some(error) = slideshow::LAST_ERROR.lock(|e| e.borrow_mut().take()) {
            with_model(|m| {
                m.note.clear();
                let _ = m.note.push_str(&error);
                m.note_until = Some(Instant::now() + ALERT_SHOW);
                raise_alert(m);
            });
        }

        Timer::after(TICK).await;
    }
}
