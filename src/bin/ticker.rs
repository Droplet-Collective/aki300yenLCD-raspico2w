//! ネットワーク・ティッカー (v0.3.0〜): NTP 時計 + Open-Meteo 天気 + GitHub の `message.txt` を流す + OTA
//!
//! `wifi_ota` の後継として Release の OTA イメージになる bin。TBYB / 接続管理 / OTA は `ota::app`
//! (wifi_ota と共用) で、この bin は表示と 3 つの取得 (NTP / 天気 / 流れる文字) を持つ。
//! 使い方と `ticker.txt` の書き方は docs/ticker.md。
//!
//! # タスク構成
//!
//! - `main`: SD (`WIFI.TXT` / `TICKER.TXT`) → LCD 開始 → USB → CYW43 → 以後 250 ms 周期のループで
//!   join / DHCP (`LinkManager`)、TBYB の自己診断と buy、状態行の更新、OTA の再起動を行う。
//! - `jobs_task` (0.4.1〜): OTA 確認、NTP、天気、流れる文字の取得を **順番に** 行う (HTTPS の TLS バッファは
//!   1 組しか無いので、同時に 2 本は張らない)。接続後の最初の仕事は必ず OTA 確認 (新しい版で直せるように)。
//!   0.4.0 までは main ループの中で行っていたが、main の poll のスタックフレーム (8〜14 KB) の上に
//!   TLS ハンドシェイクが積まれてスタックが溢れたので、別のタスクに分けた (docs/ticker.md「スタック」)。
//! - `render_task`: LCD の垂直同期 (`Display::present`、≈60 Hz) ごとに画面全体をバックバッファへ描き直し
//!   (背景の写真のコピー → ガラス板 → 文字)、流れる文字を `scroll` px / フレームで左へ動かす。表示する内容は
//!   `MODEL` (共有モデル) と `slideshow::BG` (背景) から読む。
//!   フラッシュ書き込み中 (数百 ms、割り込み禁止) は描画が止まるが、走査は SRAM の DMA リングで続く。
//! - `slideshow_task`: SD の BMP を読み、背景を切り替える (`ticker::slideshow`)。OTA の確認〜検証中は止まる。
//! - 設定ページの HTTP サーバ (0.5.0〜、`web::server`、docs/settings-server.md): 取得タスクの中で 1 要求ずつ動く
//!   (OTA 確認の後、NTP / 天気 / 文字より先)。最初の OTA 確認が通ってから待ち受け、回復モードでは動かない。
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
//! # OTA 到達保証 (0.4.2〜、docs/ticker.md §8、`boot_policy` / `supervisor`)
//!
//! - ウォッチドッグ (8 s) は main の最初 (`embassy_rp::init` の直後) から、どの起動でも動かす。初期化の間は
//!   main が明示的に再ロードし、描画が始まったら main / 取得 / 描画の 3 タスクの生存確認が揃っている間だけ
//!   LCD のフレーム割り込みが再ロードする。panic / HardFault / スタック溢れ (MSPLIM) / タスクの停止は理由を
//!   WATCHDOG の SCRATCH に残してすぐリセットし、次の起動が状態行 1 に `last reset: ...` と 5 分出す。
//! - TBYB の buy 条件: Wi-Fi + DHCP、OTA の manifest 確認が TLS + HTTP を最後まで通った、NTP / 天気 / 文字 /
//!   SD の設定 / 最初の写真を 1 回ずつ試した、その後 25 s 健全に動いた (`boot_policy::BuyGate`、締め切り 180 s)。
//!   buy 待ちの間の OTA 確認は manifest を読むだけ (ダウンロードは buy の後)。
//! - 通常モードで 2 回続けて異常終了したら **回復モード** (`recovery_main`、取得タスクの中で動く): 黒地の文字だけの画面、SD に触れない
//!   (Wi-Fi の資格情報は data 区画の写し `persist`)、Wi-Fi + OTA 確認 (60 s ごと) だけ。回復モードでも 3 回
//!   続けて落ちたら、他方区画 (前に buy した版) を FLASH_UPDATE 起動する。

#![no_std]
#![no_main]

use core::cell::RefCell;
use core::fmt::Write as _;
use core::mem::MaybeUninit;
use core::ptr::addr_of_mut;
use core::sync::atomic::{AtomicBool, Ordering};

use cyw43::PowerManagementMode;
use embassy_executor::Spawner;
use embassy_rp::bind_interrupts;
use embassy_rp::clocks::RoscRng;
use embassy_rp::flash::Flash;
use embassy_rp::peripherals::*;
use embassy_rp::pio::{InterruptHandler, Pio};
use embassy_rp::usb::{Driver as UsbDriver, InterruptHandler as UsbInterruptHandler};
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer, with_timeout};
use embassy_usb::UsbDevice;
use heapless::String;
use embassy_rp::Peri;
use pico2w_300yen_lcd::ab_boot;
use pico2w_300yen_lcd::boot_policy::{
    self, BootInputs, BootPlan, BootState, CheckOutcome, Mode, PENDING_OTA_RETRY_MS, PrevBoot, RECOVERY_NORMAL_RETRY_MS,
    RECOVERY_OTA_INTERVAL_MS, Round,
};
use pico2w_300yen_lcd::boot_trace::{self, ResetReason, Stage, Trace};
use pico2w_300yen_lcd::image_def::{FIRMWARE_VERSION, IMAGE_DEF_MAJOR, IMAGE_DEF_MINOR, IMAGE_DEF_VERSION_WORD, TBYB};
use pico2w_300yen_lcd::lcd::display::{BackBuffer, Display, DisplayPins, FrameIrqHandler};
use pico2w_300yen_lcd::ota::app::{
    self, BootStatus, BuyState, CheckMode, LinkManager, LinkUi, Net, NetBuffers, OtaPhase, OtaState, OtaUi, TcpState,
    Tone,
};
use pico2w_300yen_lcd::ota::http::{self, BodySink};
use pico2w_300yen_lcd::ota::slot::{OtaFlash, SectorBuffers, Slots, slot_label};
use pico2w_300yen_lcd::ota::OtaError;
use pico2w_300yen_lcd::noinline::noinline;
use pico2w_300yen_lcd::persist;
use pico2w_300yen_lcd::sdcard::{self, ReadError, SdVolumeManager, init_sd, read_root_file};
use pico2w_300yen_lcd::supervisor;
use pico2w_300yen_lcd::ticker::civil;
use pico2w_300yen_lcd::ticker::config::{self, ConfigSource, DebugCrash, LayoutName, StatusMode, TickerConfig};
use pico2w_300yen_lcd::ticker::health::{self, LastReset, Limits, Who};
use pico2w_300yen_lcd::ticker::slideshow::{self, SlideConfig};
use pico2w_300yen_lcd::ticker::sntp::{self, SntpError};
use pico2w_300yen_lcd::ticker::sntp_net::{self, SntpBuffers, Sync};
use pico2w_300yen_lcd::ticker::weather::{self, Weather};
use pico2w_300yen_lcd::ui::canvas::Canvas;
use pico2w_300yen_lcd::ui::recovery::{self as recovery_ui, RecoveryView};
use pico2w_300yen_lcd::ui::screen::{self, Banner, Clock, Layout, StatusView, Tone as UiTone, View, WeatherView};
use pico2w_300yen_lcd::ui::scroll::{self, ScrollText, Settings};
use pico2w_300yen_lcd::usb_reset::build_usb_device;
use pico2w_300yen_lcd::web::auth;
use pico2w_300yen_lcd::web::json::Json;
use pico2w_300yen_lcd::web::server::{self as web_server, Server};
use pico2w_300yen_lcd::wifi::{self, Cyw43Pins, WifiCredentials, ascii_label, read_credentials};
use defmt_rtt as _;

// RP2350 bootrom 用 IMAGE_DEF (版数付き、--features tbyb で TBYB フラグ) と picotool 用 binary_info
pico2w_300yen_lcd::firmware_image_def!();

/// panic: 行番号と `&Location` のアドレスを SCRATCH に記録 → defmt → リセット (0.4.1〜。0.4.0 までは
/// `udf` → HardFault の `loop {}` で止まったままだった)。次の起動 (同じ版) が `last reset: panic <file>:<line>`
/// と出す。TBYB の buy 待ちなら buy されていないので旧版へ戻る。
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    cortex_m::interrupt::disable();
    let (line, location) = info
        .location()
        .map_or((0, 0), |l| (l.line(), l as *const core::panic::Location<'_> as u32));
    boot_trace::fault_with(Stage::Panic, line, location);
    defmt::error!("{}", defmt::Display2Format(info));
    supervisor::reset_now()
}

/// HardFault: PC / LR と、スタック溢れ (MSPLIM を越えた = CFSR.STKOF) かどうかを記録してリセット (0.4.1〜)
#[cortex_m_rt::exception]
unsafe fn HardFault(frame: &cortex_m_rt::ExceptionFrame) -> ! {
    // Safety: SCB の CFSR (0xE000_ED28) を読むだけ
    let cfsr = unsafe { core::ptr::read_volatile(0xE000_ED28 as *const u32) };
    let stage = if cfsr & (1 << 20) != 0 { Stage::StackOverflow } else { Stage::HardFault };
    boot_trace::fault_with(stage, frame.pc(), frame.lr());
    supervisor::reset_now()
}

/// 登録していない割り込み: 番号を記録してリセット (cortex-m-rt の既定は `loop {}`)
#[cortex_m_rt::exception]
unsafe fn DefaultHandler(irqn: i16) -> ! {
    boot_trace::fault_with(Stage::UnhandledIrq, irqn as i32 as u32, 0);
    supervisor::reset_now()
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

/// ヒープ (embedded-tls の `rsa` feature が alloc を要求するため)。ticker は `TlsVerify::None` なので RSA の
/// 検証コードは呼ばれず、実際には確保は起きない (0.4.1 で逆アセンブルの呼び出しグラフから確認: どのタスクの
/// poll からも、証明書検証用の reqwless `Provider` を通らずに embedded-alloc / `alloc::raw_vec` へ届く経路は無い)。
/// 8 KB → 1 KB にしてスタックに回した。万一足りなければ panic → 記録してリセット。
const HEAP_SIZE: usize = 1024;
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
/// 前回の異常終了 (`last reset: ...`) を状態行 1 に出す時間
const FAULT_DIAG_SHOW: Duration = Duration::from_secs(300);
/// 接続後の最初の OTA 確認が済むまで写真の読み込みを待つ。ただしネットワークが無くてもこの時間で始める
const SLIDESHOW_START_MAX: Duration = Duration::from_secs(45);
/// 起動時に SD (wifi.txt / ticker.txt) を読む期限 (0.4.2〜、`sdcard::set_deadline`。カードが無いと ≈ 25 s かかっていた)
const SD_BOOT_DEADLINE: Duration = Duration::from_secs(5);
/// 流れる文字の最大長 (バイト。UTF-8 で日本語 ≈ 170 文字)
const MESSAGE_MAX: usize = config::MESSAGE_MAX;
/// 状態 3 行の行 1 に設定ページの案内 (URL とコード) を出す時間 (待ち受けの開始時 / ページの「LCD にコードを表示」)。
/// 「LCD にコードを表示」は `show_settings=0` でもこの間だけ流れる文字に設定の部分を入れる
const BANNER_SHOW: Duration = Duration::from_secs(60);
/// 「LCD にコードを表示」の後、流れる文字の設定の部分を目立たせる時間 (0.5.1〜)
const HIGHLIGHT_SHOW: Duration = Duration::from_secs(12);
// 流れる文字 (最大 MESSAGE_MAX) + 設定の部分が組み立ての領域に必ず入る
const _: () = assert!(scroll::TEXT_MAX >= MESSAGE_MAX + scroll::SETTINGS_MAX);

// ============================================================
// static 配置のバッファ (BSS)
// ============================================================

static mut NET_BUFFERS: NetBuffers = NetBuffers::new();
static mut TCP_STATE: TcpState = TcpState::new();
static mut SECTOR_BUFFERS: SectorBuffers = SectorBuffers::new();
static mut SNTP_BUFFERS: SntpBuffers = SntpBuffers::new();
/// 天気の JSON / 文字の本文の受信先 (どちらも同時には使わない)。起動時は ticker.txt の読み込みにも使う
static mut BODY: [u8; weather::BODY_MAX] = [0; weather::BODY_MAX];
const _: () = assert!(weather::BODY_MAX >= config::CONFIG_MAX);
/// SD (スライドショーと設定ページのサーバが `slideshow::SD_LOCK` で分け合う。0.5.0〜)
static SD_CARD: static_cell::StaticCell<SdVolumeManager> = static_cell::StaticCell::new();

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
    /// スタックの最大使用量 / 大きさ (バイト、状態行 2 の `stk 21.3/35.4K`。0.4.1〜)
    stack_used: u32,
    stack_total: u32,
    /// 接続先の SSID (設定ページの状態表示。0.5.0〜)
    ssid: String<32>,
    /// 前回のリセット理由 (設定ページの状態表示)
    last_reset: String<80>,
    /// 設定ページの URL (IP が無ければ空) / アクセスコード / 状態行 1 に案内を出す期限 (0.5.0〜)
    web_url: String<24>,
    web_code: String<8>,
    banner_until: Option<Instant>,
    /// 設定ページが待ち受けている (最初の OTA 確認の後。回復モードでは立たない)。URL / コードが変わるたびに `web_gen` を進める
    web_listening: bool,
    web_gen: u32,
    /// 流れる文字に設定の部分を入れる (`show_settings=`、0.5.1〜)
    show_settings: bool,
    /// 「LCD にコードを表示」: この時刻まで設定の部分を入れる (`show_settings=0` でも) / 目立たせる期限 /
    /// 設定の部分へ飛ぶ要求の世代
    settings_forced_until: Option<Instant>,
    highlight_until: Option<Instant>,
    scroll_jump_gen: u32,
    /// 組み立てた流れる文字 (render_task だけが書く。static に置くのでスタックを使わない)
    scroll: ScrollText,
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
            stack_used: 0,
            stack_total: 0,
            ssid: String::new(),
            last_reset: String::new(),
            web_url: String::new(),
            web_code: String::new(),
            banner_until: None,
            web_listening: false,
            web_gen: 0,
            show_settings: true,
            settings_forced_until: None,
            highlight_until: None,
            scroll_jump_gen: 0,
            scroll: ScrollText::new(),
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

// ============================================================
// main と取得タスクの受け渡し (0.4.1〜)
// ============================================================

/// main → 取得タスク: IP があり、buy も済んだ (または TBYB でない) ので取得してよい
static NET_READY: AtomicBool = AtomicBool::new(false);
/// 取得タスク → main: 検証済みイメージへ FLASH_UPDATE 再起動してほしい (Wi-Fi の電源断に `Control` が要る)
static REBOOT_REQUEST: Signal<ThreadModeRawMutex, Slots> = Signal::new();

/// ota::app への表示口 (取得タスク): OTA の途中経過を共有モデルへ書く (描画は render_task が毎フレーム行う)
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
        // ダウンロード中は 250 ms ごとに呼ばれる (数分かかっても取得タスクの生存確認が途切れない)
        supervisor::beat(Who::Jobs);
        self.ota.phase = phase;
        self.publish_ota();
    }
}

/// ota::app への表示口 (main): 接続状況を共有モデルへ書く
struct LinkModelUi;

impl LinkUi for LinkModelUi {
    async fn link_status(&mut self, text: &str, _joined_ssid: Option<&String<32>>) {
        supervisor::beat(Who::Main);
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
        scroll: m.scroll.line(m.highlight_until.is_some_and(|t| now < t)),
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
        banner: banner_of(m, now),
    };
    let mut canvas = Canvas::new(&mut frame.data);
    screen::render(&mut canvas, bg, slideshow::LEVEL.load(Ordering::Relaxed), &view, m.layout);
}

/// 流れる文字の設定の部分の状態 (render_task が変化を見て組み立て直す)
#[derive(Clone, Copy, PartialEq, Eq)]
enum SettingsPart {
    Hidden,
    Waiting,
    Ready,
}

fn settings_part(m: &Shared, now: Instant) -> SettingsPart {
    let forced = m.settings_forced_until.is_some_and(|t| now < t);
    if !m.web_listening || m.web_code.is_empty() || !(m.show_settings || forced) {
        SettingsPart::Hidden
    } else if m.web_url.is_empty() {
        SettingsPart::Waiting
    } else {
        SettingsPart::Ready
    }
}

/// 流れる文字を組み立て直す (共有モデルの static な領域の中で。スタックに文字列を置かない)
fn compose_scroll(m: &mut Shared, part: SettingsPart) {
    let Shared {
        scroll,
        message,
        web_url,
        web_code,
        ..
    } = m;
    let settings = match part {
        SettingsPart::Hidden => Settings::Hidden,
        SettingsPart::Waiting => Settings::Waiting,
        SettingsPart::Ready => Settings::Ready { url: web_url, code: web_code },
    };
    scroll.compose(message, settings);
}

/// 設定ページの案内を状態行 1 に出すか
fn banner_of(m: &Shared, now: Instant) -> Option<Banner<'_>> {
    (m.banner_until.is_some_and(|t| now < t) && !m.web_url.is_empty()).then_some(Banner {
        url: &m.web_url,
        code: &m.web_code,
    })
}

/// 毎フレーム (LCD の垂直同期ごと) 画面を描き直し、流れる文字を動かす。
///
/// 流れる文字の位置は LCD のフレーム番号で進める (SD の読み込みなどで描画が 1 フレーム遅れても、
/// 速さは変わらず 2 px 飛ぶだけ)。描き終えるたびに `slideshow::FRAME_SLOT` で SD の読み込みに番を渡す。
///
/// 流れる文字 (0.5.1〜) は 文字 + 設定ページの URL とコード。入力 (文字の世代、URL / コードの世代、設定の部分の
/// 状態) が変わったフレームだけ組み立て直す。文字が変わったら右端から入り直し、URL / コードだけが変わったときは
/// 位置をそのままにする (流れている途中で飛ばない)。
#[embassy_executor::task]
async fn render_task(mut display: Display) {
    let mut message_gen = u32::MAX;
    let mut scroll_key = (u32::MAX, u32::MAX, SettingsPart::Hidden);
    let mut jump_gen = 0u32;
    let mut scroll_x = 0;
    let mut last_frame = Display::frame_count();
    let mut render_us_max: u64 = 0;
    let mut stats_at = Instant::now();
    loop {
        supervisor::beat(Who::Render);
        let started = Instant::now();
        let frame_now = Display::frame_count();
        let elapsed_frames = frame_now.wrapping_sub(last_frame).clamp(1, 8) as i32;
        last_frame = frame_now;
        MODEL.lock(|cell| {
            let mut guard = cell.borrow_mut();
            let m = &mut *guard;
            let part = settings_part(m, Instant::now());
            let key = (m.message_gen, m.web_gen, part);
            if key != scroll_key {
                scroll_key = key;
                compose_scroll(m, part);
            }
            let (_, scroll_w) = m.layout.scroll_area(false);
            if m.message_gen != message_gen {
                message_gen = m.message_gen;
                scroll_x = scroll_w;
            } else {
                let step = i32::from(m.scroll_px.max(1)) * elapsed_frames;
                scroll_x = scroll::advance(scroll_x, step, m.scroll.width(), scroll_w, m.scroll.looped());
            }
            if m.scroll_jump_gen != jump_gen {
                // 「LCD にコードを表示」: 設定の部分を範囲の左端 (+ 目立たせる板の余白 4 px) へ
                jump_gen = m.scroll_jump_gen;
                if let Some(x) = m.scroll.settings_scroll_x() {
                    scroll_x = x + 4;
                }
            }
            let m = &*m;
            slideshow::BG.lock(|bg| draw_screen(display.back(), &bg.borrow()[..], m, scroll_x));
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
                // SD に書くファイルの日時 (設定ページの保存 / 写真の追加)
                sdcard::set_wall_clock((sync.now_unix() + i64::from(m.tz_offset_secs)).max(0) as u64);
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
// 起動診断
// ============================================================

/// 状態行 2 (版数 / 区画 / TBYB / スタックの最大使用量) を共有モデルへ
fn publish_ident(boot: &BootStatus) {
    let mut head: String<32> = String::new();
    let _ = write!(head, "ticker v{}", FIRMWARE_VERSION);
    if boot.is_ota_boot() {
        let _ = head.push_str(" via OTA");
    }
    let mut rest: String<64> = String::new();
    let tone = boot.write_tbyb_line(&mut rest, !TBYB);
    with_model(|m| {
        // buy 待ちの間は `wait:...` を出すので、スタックの表示は省く (66 桁に収めるため)
        if m.stack_total > 0 && boot.buy != BuyState::Pending {
            let _ = rest.push_str(" stk ");
            let tenths = (m.stack_used as u64 * 10 + 512) / 1024;
            let _ = write!(rest, "{}.{}/", tenths / 10, tenths % 10);
            health::write_kib(&mut rest, m.stack_total);
        }
        m.ident_head = head;
        m.ident_rest = rest;
        m.ident_tone = tone;
    });
}

unsafe extern "C" {
    static __srodata: u8;
    static __erodata: u8;
}

/// panic の記録 (SCRATCH0 = `&Location` のアドレス) からファイル名を取り出す。同じ版のイメージで、アドレスが
/// .rodata の中にあり、行番号 (SCRATCH7) が一致するときだけ (違うイメージのアドレスは解釈しない)
fn panic_file(trace: &Trace) -> Option<&'static str> {
    if trace.stage != Some(Stage::Panic) || u16::from(trace.major) != IMAGE_DEF_MAJOR || trace.minor != IMAGE_DEF_MINOR {
        return None;
    }
    let lo = &raw const __srodata as usize;
    let hi = &raw const __erodata as usize;
    let p = trace.extra as usize;
    let size = core::mem::size_of::<core::panic::Location<'static>>();
    if p < lo || p + size > hi || !p.is_multiple_of(core::mem::align_of::<core::panic::Location<'static>>()) {
        return None;
    }
    // Safety: 同じイメージの .rodata 内の、panic ハンドラが記録した `Location` のアドレス (上で範囲と整列を確認)
    let location = unsafe { &*(p as *const core::panic::Location<'static>) };
    if location.line() != trace.info {
        return None;
    }
    let file = location.file();
    let fp = file.as_ptr() as usize;
    (fp >= lo && fp + file.len() <= hi && file.len() <= 256).then_some(file)
}

/// 前回の記録を書いたのがこの版か
fn trace_is_own(trace: &Trace) -> bool {
    u16::from(trace.major) == IMAGE_DEF_MAJOR && trace.minor == IMAGE_DEF_MINOR
}

/// 起動の方針の入力 (`boot_policy::decide`)
fn boot_inputs(boot: &BootStatus) -> BootInputs {
    BootInputs {
        own_version: IMAGE_DEF_VERSION_WORD,
        hw_reset: boot.reset_reason == ResetReason::Hardware,
        flash_update_boot: boot.is_flash_update_boot(),
        tbyb_pending: boot.buy == BuyState::Pending,
        prev: PrevBoot {
            trace_version: boot
                .prev_trace
                .as_ref()
                .map(|t| u32::from(t.major) << 16 | u32::from(t.minor)),
            fault_recorded: boot.prev_trace.as_ref().is_some_and(|t| t.stage.is_some_and(Stage::is_fault)),
            watchdog_timeout: boot.reset_reason == ResetReason::WatchdogTimer,
        },
        state_word: boot_trace::read_streak(),
        marker_word: boot.scratch0,
        other_slot_known: boot.slots.is_ok(),
    }
}

/// 前回の起動 (この版) の異常終了 (panic / HardFault / スタック溢れ / 停止 / 記録の無いウォッチドッグ) を復号する。
/// 他の版の記録 (TBYB で試した版が巻き戻った) は `BootStatus::write_prev_trace_line` が別に出す
fn last_reset(boot: &BootStatus, plan: &BootPlan) -> Option<(LastReset<'static>, u32)> {
    if !plan.fault {
        return None;
    }
    let trace = boot.prev_trace.as_ref().filter(|t| trace_is_own(t))?;
    let reset = health::decode(trace.stage_code, trace.info, trace.extra, panic_file(trace)).unwrap_or(
        // ウォッチドッグの時間切れだが記録が無い: 割り込みも止まった (ロックアップ等) か、初期化の途中で止まった
        LastReset::WatchdogNoRecord {
            stage: trace.stage_label(),
        },
    );
    Some((reset, trace.uptime_ds))
}

/// 版数語 → `0.4.2`
fn write_version_word<const N: usize>(out: &mut String<N>, word: u32) {
    let (major, minor, patch) = boot_policy::version_parts(word);
    let _ = write!(out, "{}.{}.{}", major, minor, patch);
}

/// 起動診断を状態行 1 に出す: 他方区画から戻ってきた / 前回の異常終了 (5 分、赤) → 前回の TBYB 記録 →
/// 起動種別 (60 s)
fn publish_diag(boot: &BootStatus, plan: &BootPlan) {
    let mut line: String<80> = String::new();
    let (tone, show) = if let Some(from) = plan.fell_back_from {
        let _ = line.push_str("FALLBACK: v");
        write_version_word(&mut line, from);
        let _ = write!(line, " kept crashing, back on v{} (blocked)", FIRMWARE_VERSION);
        (Tone::Error, FAULT_DIAG_SHOW)
    } else if let Some((reset, uptime_ds)) = last_reset(boot, plan) {
        health::write_last_reset(&mut line, &reset, uptime_ds, u32::from(plan.state.crash_streak));
        (Tone::Error, FAULT_DIAG_SHOW)
    } else if let Some(tone) = boot.write_prev_trace_line(&mut line) {
        (tone, DIAG_SHOW)
    } else if boot.show_boot_line() {
        boot.write_boot_line(&mut line);
        (Tone::Muted, DIAG_SHOW)
    } else {
        return;
    };
    defmt::warn!("boot diag: {}", line.as_str());
    with_model(|m| {
        if tone == Tone::Error {
            m.last_reset = line.clone();
        }
        m.diag = line;
        m.diag_tone = tone;
        m.diag_until = Some(Instant::now() + show);
    });
}

// ============================================================
// 取得タスク (OTA / NTP / 天気 / 文字)
// ============================================================

/// main → 取得タスク: OTA のダウンロードをしてよい (buy 済み、または TBYB でない起動)。buy 待ちは manifest の確認だけ
static DOWNLOAD_OK: AtomicBool = AtomicBool::new(false);
/// main → 取得タスク: すぐに OTA を確認してほしい (buy の直後。buy 待ちの間に新しい版を見つけていてもよいように)
static OTA_NOW: AtomicBool = AtomicBool::new(false);
/// 取得タスク → main: OTA の manifest 確認が TLS + HTTP を最後まで通った (buy 条件 (b))
static OTA_PROVED: AtomicBool = AtomicBool::new(false);
/// 取得タスク → main: 一巡 (buy 条件 (c)) のうち NTP / 天気 / 文字を 1 回ずつ試した
static ROUND_NTP: AtomicBool = AtomicBool::new(false);
static ROUND_WEATHER: AtomicBool = AtomicBool::new(false);
static ROUND_MESSAGE: AtomicBool = AtomicBool::new(false);
/// 取得タスク → main: 設定ページのサーバが待ち受けを始めた (一巡の 1 つ、0.5.0〜)
static ROUND_WEB: AtomicBool = AtomicBool::new(false);
/// 取得タスク (設定ページ) → main: 再起動してほしい (Wi-Fi の電源断に `Control` が要る)
static WEB_REBOOT: AtomicBool = AtomicBool::new(false);

/// 取得タスクが持つもの (main が作って渡す)
struct Jobs {
    net: Net<'static>,
    stack: embassy_net::Stack<'static>,
    bufs: &'static mut NetBuffers,
    flash: OtaFlash,
    sectors: &'static mut SectorBuffers,
    sntp_bufs: &'static mut SntpBuffers,
    body: &'static mut [u8; weather::BODY_MAX],
    config: TickerConfig,
    ota: OtaState,
    slots: Result<Slots, OtaError>,
    /// wifi.txt があり、A/B 区画も分かる
    ota_possible: bool,
    /// この版数語以下は OTA で入れない (他方区画へ戻してきた版、`persist`)
    blocked: u32,
    /// 試験用 `debug_crash=ota` (buy 済みの通常起動だけ)
    crash_before_ota: bool,
    /// SD (設定ページの保存 / 写真。無ければ None)
    sd: Option<&'static SdVolumeManager>,
}

// ============================================================
// 設定ページ (0.5.0〜、web::server が頼むこと)
// ============================================================

/// 設定ページの URL を共有モデルへ (IP が変わったら直し、無くなったら空にする。流れる文字はすぐ組み直される)
fn publish_web_url(stack: embassy_net::Stack<'static>) {
    let mut url: String<24> = String::new();
    if let Some(cfg) = stack.config_v4() {
        let ip = cfg.address.address().octets();
        let _ = write!(url, "http://{}.{}.{}.{}/", ip[0], ip[1], ip[2], ip[3]);
    }
    with_model(|m| {
        if m.web_url != url {
            m.web_url = url;
            m.web_gen = m.web_gen.wrapping_add(1);
        }
    });
}

/// 設定ページのサーバから見た ticker (取得タスクの持ち物を借りる)
struct WebHost<'a> {
    stack: embassy_net::Stack<'static>,
    config: &'a mut TickerConfig,
    weather_job: &'a mut Job,
    message_job: &'a mut Job,
    ota: &'a OtaState,
    slots: &'a Result<Slots, OtaError>,
    last_ota_check: Option<Instant>,
}

fn layout_of(name: LayoutName) -> Layout {
    match name {
        LayoutName::Glass => Layout::Glass,
        LayoutName::Dock => Layout::Dock,
        LayoutName::Classic => Layout::Classic,
    }
}

/// 流れる文字として出せる形 (改行 / タブは空白、前後の空白を除く)
fn message_from(text: &str) -> String<MESSAGE_MAX> {
    let mut out: String<MESSAGE_MAX> = String::new();
    for ch in text.chars() {
        let ch = if ch == '\n' || ch == '\r' || ch == '\t' { ' ' } else { ch };
        if out.push(ch).is_err() {
            break;
        }
    }
    let trimmed = out.trim();
    let mut message: String<MESSAGE_MAX> = String::new();
    let _ = message.push_str(trimmed);
    message
}

impl web_server::App for WebHost<'_> {
    fn ip(&self) -> Option<[u8; 4]> {
        self.stack.config_v4().map(|c| c.address.address().octets())
    }

    fn write_status(&mut self, j: &mut Json<'_>) {
        let uptime = Instant::now().as_secs();
        j.field_str("version", FIRMWARE_VERSION);
        j.field_int("uptime_s", uptime as i64);
        let mut ota_line: String<80> = String::new();
        let (ota_tone, _) = self.ota.write_line(&mut ota_line, self.slots);
        with_model(|m| {
            j.field_str("ssid", &m.ssid);
            j.key("ip");
            match self.stack.config_v4() {
                Some(cfg) => {
                    let ip = cfg.address.address().octets();
                    let mut s: String<16> = String::new();
                    let _ = write!(s, "{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
                    j.str(&s);
                }
                None => j.null(),
            }
            // cyw43 0.6 は接続中の RSSI を読む API を公開していない (docs/settings-server.md「制限」)
            j.key("rssi");
            j.null();
            j.field_str("wifi", &m.wifi);
            j.field_str("ntp", &m.ntp);
            j.field_str("weather", &m.wx);
            j.field_str("message_state", &m.msg);
            j.field_str("ota", &ota_line);
            j.field_str("ota_tone", tone_name(ota_tone));
            j.field_str("ident", &m.ident_head);
            j.field_str("tbyb", m.ident_rest.trim());
            j.field_int("stack_used", i64::from(m.stack_used));
            j.field_int("stack_total", i64::from(m.stack_total));
            j.field_str("last_reset", &m.last_reset);
            j.field_str("layout", m.layout.name());
            j.key("weather_now");
            match m.weather {
                Some(w) => {
                    j.begin_object();
                    j.key("temperature");
                    j.float(w.temperature, 1);
                    j.field_int("code", i64::from(w.code));
                    j.field_str("condition", w.condition_ja());
                    j.end_object();
                }
                None => j.null(),
            }
        });
        j.key("last_ota_check_s");
        match self.last_ota_check {
            Some(t) => j.int(t.elapsed().as_secs() as i64),
            None => j.null(),
        }
        j.field_int("ota_checks", i64::from(self.ota.checks));
        j.field_bool("pending", !DOWNLOAD_OK.load(Ordering::Relaxed));
    }

    fn config(&self) -> &TickerConfig {
        self.config
    }

    fn write_local_message(&self, j: &mut Json<'_>) {
        if self.config.local_message {
            with_model(|m| j.str(&m.message));
        } else {
            j.str("");
        }
    }

    fn apply(&mut self, new: &TickerConfig, message: Option<&str>) {
        let old = core::mem::replace(self.config, new.clone());
        if old.lat != new.lat || old.lon != new.lon {
            // 地域が変わった: すぐ天気を取り直す
            self.weather_job.next = Instant::now();
            self.weather_job.failures = 0;
            with_model(|m| m.weather = None);
        }
        if old.message_url != new.message_url || (old.local_message && !new.local_message) {
            self.message_job.next = Instant::now();
            self.message_job.failures = 0;
        }
        let layout = layout_of(new.layout);
        with_model(|m| {
            m.tz_offset_secs = new.tz_offset_secs;
            m.place = new.place.clone();
            m.scroll_px = new.scroll_px;
            m.layout = layout;
            m.status_mode = new.status;
            m.show_settings = new.show_settings;
            if let Some(text) = message.filter(|_| new.local_message) {
                let text = message_from(text);
                if m.message != text {
                    m.message = text;
                    m.message_gen = m.message_gen.wrapping_add(1);
                }
                m.msg.clear();
                let _ = m.msg.push_str("ok(local)");
            } else if old.local_message {
                m.msg.clear();
                let _ = m.msg.push_str("---");
            }
        });
        let reload = old.images != new.images || old.layout != new.layout;
        slideshow::LIVE.lock(|l| {
            let mut l = l.borrow_mut();
            l.interval_secs = new.slide_secs;
            l.images = new.images.clone();
            l.layout = layout;
        });
        if reload {
            slideshow::RELOAD.store(true, Ordering::Relaxed);
        }
        defmt::info!("web: settings applied (layout {}, slide {} s, reload {})", layout.name(), new.slide_secs, reload);
    }

    /// 「LCD にコードを表示」: 流れる文字を設定の部分へ進めて目立たせ、状態行 1 にも 1 分出す
    /// (`show_settings=0` でもその 1 分は流れる文字に入れる)
    fn show_code(&mut self) {
        with_model(|m| {
            let now = Instant::now();
            m.banner_until = Some(now + BANNER_SHOW);
            m.settings_forced_until = Some(now + BANNER_SHOW);
            m.highlight_until = Some(now + HIGHLIGHT_SHOW);
            m.scroll_jump_gen = m.scroll_jump_gen.wrapping_add(1);
        });
    }

    fn reboot(&mut self) -> Result<(), &'static str> {
        if !DOWNLOAD_OK.load(Ordering::Relaxed) {
            // buy 待ちで再起動すると、buy していないこの版は旧版へ戻ってしまう
            return Err("更新の確認中 (TBYB の buy 待ち) なので再起動できません。数分後にもう一度");
        }
        WEB_REBOOT.store(true, Ordering::Relaxed);
        Ok(())
    }

    fn ota_check(&mut self) {
        OTA_NOW.store(true, Ordering::Relaxed);
    }
}

fn tone_name(tone: Tone) -> &'static str {
    match tone {
        Tone::Muted => "muted",
        Tone::Normal => "normal",
        Tone::Ok => "ok",
        Tone::Busy => "busy",
        Tone::Error => "error",
    }
}

/// OTA 確認が TLS + HTTP を最後まで通った
fn note_ota_proved(pending: bool) {
    if !OTA_PROVED.swap(true, Ordering::Relaxed) {
        defmt::info!("OTA check proved the TLS + HTTP path");
    }
    supervisor::ota_proved();
    if pending {
        boot_trace::stage(Stage::OtaProved);
    }
}

/// 取得タスクの仕事: 通常モードの取得、または回復モード全体 (0.4.2〜)。
/// 1 つのタスクにまとめるのは RAM のため: OTA 確認 (TLS) を含む future は ≈ 15 kB あり、別々のタスクにすると
/// static なタスク領域が 2 つ分要ってスタックが 15 kB 減る。1 つのタスクなら大きい方の分だけで済む。
#[allow(clippy::large_enum_variant)] // タスクの引数として static なタスク領域に 1 回置くだけ (ヒープは使わない)
enum NetWork {
    Normal(Jobs),
    Recovery(Spawner, RecoveryParts),
}

/// 通常モード: 250 ms ごとに 1 つずつ: OTA 確認 (接続後の最初の仕事、以後 60 s ごと) > NTP > 天気 > 文字。
/// 最初の OTA 確認が済むまでは他の取得も写真の読み込みも始めない (0.4.1〜。新しい版が壊れていても、
/// 起動するたびに先に OTA 確認まで進めば、次の版で直せる)。
/// TBYB の buy 待ちの間 (0.4.2〜) は manifest を読むだけで、通信の失敗なら 10 s 後に試し直す。
///
/// どの取得も内側で打ち切るので、OTA 確認の予定は他の取得が詰まっても最長 ≈ 20 s しか遅れない:
/// NTP は DNS / 送信 / 受信それぞれ 5 s × 2 ホスト、天気 / 文字は `fetch_small` の 20 s (DNS + TCP + TLS +
/// 本文まで全部)、OTA は manifest 30 s / ダウンロード 300 s。外側にもう 1 段 `with_timeout` を重ねると、
/// 包んだ future (数 kB) を一旦スタックに作ってから移すので poll のフレームが 25 kB 増えた (0.4.2 の試作で
/// stack-report が検出) ため、重ねない。OTA 専用のタスクを分けないのは TLS のバッファ (≈ 30 kB) が 1 組しか
/// 置けないから。
///
/// 回復モード: `recovery` の節 (このタスクの後半)。
#[embassy_executor::task]
async fn jobs_task(work: NetWork) {
    let (spawner, parts) = match work {
        NetWork::Normal(mut j) => {
            let mut ntp_job = Job::new();
            let mut weather_job = Job::new();
            let mut message_job = Job::new();
            let mut weather_plain_http = false;
            let mut first_ota_done = !j.ota_possible;
            let mut reboot_requested = false;
            // 設定ページのサーバ (最初の OTA 確認が通ってから作る。0.5.0〜)
            let mut web: Option<Server> = None;
            let mut last_ota_check: Option<Instant> = None;
            if first_ota_done {
                slideshow::START.store(true, Ordering::Relaxed);
            }
            if j.config.local_message {
                // 流れる文字は ticker.txt の message= (取得するものが無い)
                ROUND_MESSAGE.store(true, Ordering::Relaxed);
            }
            loop {
                supervisor::beat(Who::Jobs);
                let ready = NET_READY.load(Ordering::Relaxed);
                let mut served = false;
                // --- 設定ページ: 最初の OTA 確認が通ったら待ち受けを始める。以後は毎周片付けだけ ---
                if web.is_none() && ready && OTA_PROVED.load(Ordering::Relaxed) {
                    web = Some(Server::new(j.stack, j.sd, auth::code_from_random(RoscRng.next_u32())));
                }
                if let Some(server) = web.as_mut() {
                    server.maintain();
                    if server.is_listening() && !ROUND_WEB.load(Ordering::Relaxed) {
                        ROUND_WEB.store(true, Ordering::Relaxed);
                        boot_trace::stage(Stage::WebListening);
                        let code = server.code();
                        with_model(|m| {
                            m.web_code.clear();
                            let _ = write!(m.web_code, "{:06}", code);
                            m.web_listening = true;
                            m.web_gen = m.web_gen.wrapping_add(1);
                            // 状態 3 行の表示中 (起動 60 s など) は行 1 にも 1 分出す
                            m.banner_until = Some(Instant::now() + BANNER_SHOW);
                        });
                    }
                    publish_web_url(j.stack);
                }
                if ready && j.ota_possible {
                    j.ota.schedule_first_check_in(Duration::from_secs(0));
                }
                if OTA_NOW.swap(false, Ordering::Relaxed) && j.ota_possible && !reboot_requested {
                    j.ota.next_check = Some(Instant::now());
                }
                if reboot_requested {
                    // main が Wi-Fi を切って再起動するのを待つ
                } else if j.ota_possible
                    && ready
                    && j.ota.is_due()
                    && let Ok(slots) = j.slots
                {
                    if j.crash_before_ota {
                        panic!("debug_crash=ota");
                    }
                    let download_ok = DOWNLOAD_OK.load(Ordering::Relaxed);
                    j.ota.begin_check();
                    let mut ui = ModelUi {
                        ota: &mut j.ota,
                        slots: j.slots,
                    };
                    let mode = CheckMode {
                        check_only: !download_ok,
                        blocked: j.blocked,
                    };
                    let mut parsed = false;
                    let result = noinline(app::run_ota_check(&j.net, j.bufs, &mut j.flash, j.sectors, slots, mode, &mut ui, &mut parsed)).await;
                    let proved = app::check_outcome(&result, parsed) == CheckOutcome::Proved;
                    ui.ota.apply(result);
                    last_ota_check = Some(Instant::now());
                    if proved {
                        note_ota_proved(!download_ok);
                    } else if !download_ok {
                        ui.ota.next_check = Some(Instant::now() + Duration::from_millis(u64::from(PENDING_OTA_RETRY_MS)));
                    }
                    ui.publish_ota();
                    if !first_ota_done {
                        first_ota_done = true;
                        slideshow::START.store(true, Ordering::Relaxed);
                        defmt::info!("first OTA check done; starting NTP / weather / message / slideshow");
                    }
                } else if ready
                    && let Some(server) = web.as_mut()
                    && server.has_request()
                {
                    // --- 設定ページの要求 1 つ (OTA 確認の後、他の取得より先) ---
                    let mut host = WebHost {
                        stack: j.stack,
                        config: &mut j.config,
                        weather_job: &mut weather_job,
                        message_job: &mut message_job,
                        ota: &j.ota,
                        slots: &j.slots,
                        last_ota_check,
                    };
                    noinline(server.serve(&mut *j.bufs, &mut host)).await;
                    served = true;
                } else if ready && first_ota_done && ntp_job.due() {
                    // --- NTP (UDP。TLS バッファは使わない) ---
                    do_ntp(j.stack, j.sntp_bufs, &mut ntp_job).await;
                    ROUND_NTP.store(true, Ordering::Relaxed);
                } else if ready && first_ota_done && weather_job.due() {
                    do_weather(&j.net, j.bufs, &mut j.body[..], &j.config, &mut weather_plain_http, &mut weather_job).await;
                    ROUND_WEATHER.store(true, Ordering::Relaxed);
                } else if ready && first_ota_done && !j.config.local_message && message_job.due() {
                    do_message(&j.net, j.bufs, &mut j.body[..], &j.config, &mut message_job).await;
                    ROUND_MESSAGE.store(true, Ordering::Relaxed);
                }

                // --- 検証済みイメージへの FLASH_UPDATE 再起動 / 巻き戻されたイメージの再試行は main が行う ---
                if !reboot_requested
                    && let Some(version) = j.ota.reboot_due()
                    && let Ok(slots) = j.slots
                {
                    defmt::info!("reboot(FLASH_UPDATE) into P{} for {}", slots.target.index, version);
                    REBOOT_REQUEST.signal(slots);
                    reboot_requested = true;
                }

                ModelUi {
                    ota: &mut j.ota,
                    slots: j.slots,
                }
                .publish_ota();
                if served {
                    // 続けて次の要求 (ページの JS は順に送る)。OTA 確認は毎周最初に見る
                    embassy_futures::yield_now().await;
                } else if let Some(server) = web.as_ref() {
                    server.wait(TICK).await;
                } else {
                    Timer::after(TICK).await;
                }
            }
        }
        NetWork::Recovery(spawner, parts) => (spawner, parts),
    };

    // ---- 回復モード: 最小限の画面 → Wi-Fi → OTA 確認 (60 s ごと)。新しい版があれば入れて FLASH_UPDATE 起動する。
    // 確認が通って新しい版が無ければ、RECOVERY_NORMAL_RETRY_MS (10 分) 後に通常モードをもう一度試す (もう 1 回
    // 落ちたらすぐ回復モードに戻る)。SD の写真、ticker.txt、NTP、天気、文字、USB は使わない ----
    let RecoveryParts {
        mut display,
        sd_miso,
        sd_cs,
        sd_mosi,
        sd_clk,
        pio1,
        cyw43,
        flash,
        boot,
        plan,
    } = parts;
    boot_trace::stage(Stage::Recovery);
    defmt::error!(
        "RECOVERY MODE: crash streak {}, recovery streak {}, fell back {}",
        plan.state.crash_streak,
        plan.state.recovery_streak,
        plan.state.fell_back
    );

    // --- 画面の文字 (起動時に決まるもの) ---
    RECOVERY_MODEL.lock(|cell| {
        let mut m = cell.borrow_mut();
        let _ = write!(m.ident, "ticker v{}", FIRMWARE_VERSION);
        if let Ok(slots) = &boot.slots {
            let _ = write!(m.ident, " slot {}", slot_label(&slots.own));
        }
    });
    let mut line: String<80> = String::new();
    if plan.fallback_failed {
        let _ = line.push_str("fallback to the other slot failed (no bootable image there)");
    } else if let Some((reset, uptime_ds)) = last_reset(&boot, &plan) {
        health::write_last_reset(&mut line, &reset, uptime_ds, u32::from(plan.state.crash_streak));
    } else {
        let _ = write!(line, "last reset: (not recorded) crashes {}", plan.state.crash_streak);
    }
    set_row(Row::LastReset, UiTone::Error, &line);
    line.clear();
    let _ = write!(
        line,
        "crashed {}x in normal mode -> Wi-Fi + OTA only (rec #{})",
        plan.state.crash_streak,
        plan.state.recovery_streak
    );
    set_row(Row::Why, UiTone::Busy, &line);
    line.clear();
    boot.write_boot_line(&mut line);
    set_row(Row::Boot, UiTone::Muted, &line);
    set_row(Row::Hint, UiTone::Muted, "fix: publish a newer release (checked every 60 s) or USB");

    // --- Wi-Fi の資格情報: data 区画の写し → 無ければ SD の wifi.txt だけ (期限 4 s) ---
    supervisor::feed_init();
    let mut flash: OtaFlash = Flash::new_blocking(flash);
    let record = persist::data_offset().and_then(persist::read);
    let blocked = record.as_ref().map_or(0, |r| r.blocked);
    let credentials: Result<WifiCredentials, &'static str> = match record.as_ref().filter(|r| r.has_credentials()) {
        Some(r) => {
            set_row(Row::Creds, UiTone::Muted, "Wi-Fi credentials: flash copy (SD not used)");
            wifi::credentials_from_bytes(r.ssid(), r.password())
        }
        None => {
            let creds = read_wifi_txt_only(sd_miso, sd_cs, sd_mosi, sd_clk, Duration::from_secs(4));
            set_row(Row::Creds, UiTone::Muted, "Wi-Fi credentials: SD wifi.txt (no flash copy yet)");
            creds
        }
    };
    let mut ota = OtaState::new();
    match &credentials {
        Ok(c) => {
            line.clear();
            let _ = write!(line, "Wi-Fi: starting... ({})", ascii_label::<32>(c.ssid.as_bytes()));
            set_row(Row::Wifi, UiTone::Busy, &line);
        }
        Err(message) => {
            line.clear();
            let _ = write!(line, "Wi-Fi: {}", message);
            set_row(Row::Wifi, UiTone::Error, &line);
            ota.phase = OtaPhase::Disabled(message);
        }
    }
    if let Err(e) = &boot.slots {
        ota.phase = OtaPhase::Disabled(e.label());
    }
    RecoveryOtaUi {
        ota: &mut ota,
        slots: boot.slots,
    }
    .publish();

    // --- LCD (黒地に文字だけ) ---
    supervisor::feed_init();
    RECOVERY_MODEL.lock(|cell| draw_recovery(display.back(), &cell.borrow()));
    display.start(Irqs);
    boot_trace::stage(Stage::DisplayStarted);
    spawner.spawn(recovery_render_task(display)).unwrap();
    supervisor::start(Limits::TICKER);
    // 回復モードでは回数を 0 に戻さない (通常モードを試して 10 分動いたときだけ)
    supervisor::allow_streak_clear(false);

    // --- CYW43439 + embassy-net ---
    boot_trace::stage(Stage::WifiPowerCycle);
    let pio1 = Pio::new(pio1, Irqs);
    let wifi::Network { stack, mut control, .. } = noinline(wifi::start(
        &spawner,
        pio1,
        cyw43,
        PowerManagementMode::Performance,
        || boot_trace::stage(Stage::WifiInit),
    ))
    .await;
    boot_trace::stage(Stage::WifiReady);
    // Safety: 回復モードでは取得タスクを起動しないので、これらの static はこのタスクだけが触る
    let tcp_state = unsafe { &*addr_of_mut!(TCP_STATE) };
    let bufs = unsafe { &mut *addr_of_mut!(NET_BUFFERS) };
    let sectors = unsafe { &mut *addr_of_mut!(SECTOR_BUFFERS) };
    let net = Net::new(stack, tcp_state);
    let ota_possible = credentials.is_ok() && boot.slots.is_ok();
    let mut link = LinkManager::new(&credentials);
    let recovery_interval = Duration::from_millis(u64::from(RECOVERY_OTA_INTERVAL_MS));
    let normal_retry = Duration::from_millis(u64::from(RECOVERY_NORMAL_RETRY_MS));
    // OTA ができない (資格情報が無い / 区画が分からない) なら、回復モードに居ても直せないので、時間が来たら
    // 通常モードを試す (電源の入れ直しと同じ)
    let mut normal_retry_at: Option<Instant> = (!ota_possible).then(|| Instant::now() + normal_retry);

    loop {
        supervisor::beat(Who::Main);
        supervisor::beat(Who::Jobs);
        let network_up = noinline(link.step(&mut control, stack, &credentials, &mut RecoveryLinkUi)).await;
        supervisor::beat(Who::Main);
        supervisor::beat(Who::Jobs);
        if network_up && ota_possible {
            ota.schedule_first_check_in(Duration::from_secs(0));
        }
        if ota_possible
            && network_up
            && ota.is_due()
            && let Ok(slots) = boot.slots
        {
            ota.begin_check();
            let mut ui = RecoveryOtaUi {
                ota: &mut ota,
                slots: boot.slots,
            };
            let mode = CheckMode { check_only: false, blocked };
            let mut parsed = false;
            let result = noinline(app::run_ota_check(&net, bufs, &mut flash, sectors, slots, mode, &mut ui, &mut parsed)).await;
            let proved = app::check_outcome(&result, parsed) == CheckOutcome::Proved;
            ui.ota.apply(result);
            // 回復モードは失敗しても 60 s ごとに試す (通常モードのバックオフ 10 分までは待たない)
            let soon = Instant::now() + recovery_interval;
            ui.ota.next_check = Some(ui.ota.next_check.map_or(soon, |n| n.min(soon)));
            ui.publish();
            if proved {
                boot_trace::stage(Stage::OtaProved);
                if normal_retry_at.is_none() && matches!(ui.ota.phase, OtaPhase::UpToDate { .. } | OtaPhase::NoRelease | OtaPhase::Blocked { .. }) {
                    normal_retry_at = Some(Instant::now() + normal_retry);
                }
            }
        }

        // --- 新しい版 (検証済み) / 巻き戻された版の再試行へ FLASH_UPDATE 起動 ---
        if ota.reboot_due().is_some()
            && let Ok(slots) = boot.slots
        {
            line.clear();
            let _ = write!(line, "rebooting into slot {} (P{})... wifi off", slot_label(&slots.target), slots.target.index);
            set_row(Row::Next, UiTone::Ok, &line);
            Timer::after(Duration::from_millis(50)).await;
            boot_trace::clear();
            app::reboot_into_slot(&mut control, slots).await;
        }

        // --- 新しい版が無く、確認が通って 10 分動いた: 通常モードをもう一度試す ---
        line.clear();
        match normal_retry_at {
            Some(at) if Instant::now() >= at => {
                set_row(Row::Next, UiTone::Busy, "retrying normal mode now...");
                boot_trace::write_streak(plan.state.for_normal_retry().encode());
                Timer::after(Duration::from_millis(50)).await;
                let _ = with_timeout(Duration::from_secs(2), control.leave()).await;
                boot_trace::clear();
                supervisor::reset_now();
            }
            Some(at) => {
                let left = app::secs_until(at);
                let _ = write!(line, "no newer release: retry normal mode in {}:{:02}", left / 60, left % 60);
                set_row(Row::Next, UiTone::Muted, &line);
            }
            None => {
                let _ = line.push_str("waiting for a successful OTA check (every 60 s)");
                set_row(Row::Next, UiTone::Muted, &line);
            }
        }
        if link.is_joined()
            && let Ok(creds) = &credentials
        {
            line.clear();
            let _ = write!(line, "Wi-Fi: {}", ascii_label::<32>(creds.ssid.as_bytes()));
            let tone = match stack.config_v4() {
                Some(cfg) => {
                    let ip = cfg.address.address().octets();
                    let _ = write!(line, " {}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
                    UiTone::Ok
                }
                None => {
                    let _ = line.push_str(" no IP yet");
                    UiTone::Busy
                }
            };
            set_row(Row::Wifi, tone, &line);
        } else if let app::Link::Disconnected {
            last_error: Some(code),
            next_attempt,
            ..
        } = &link.link
        {
            line.clear();
            let _ = write!(line, "Wi-Fi: join failed ({}), retry in {}s", code, app::secs_until(*next_attempt));
            set_row(Row::Wifi, UiTone::Error, &line);
        }
        RecoveryOtaUi {
            ota: &mut ota,
            slots: boot.slots,
        }
        .publish();
        Timer::after(TICK).await;
    }
}


// ============================================================
// 回復モード (0.4.2〜): Wi-Fi + OTA だけ
// ============================================================

/// 回復モードの画面の内容 (recovery_task が書き、recovery_render_task が毎フレーム読む)
struct RecoveryModel {
    ident: String<40>,
    rows: [(String<{ recovery_ui::COLUMNS }>, UiTone); recovery_ui::ROWS],
}

impl RecoveryModel {
    const fn new() -> Self {
        Self {
            ident: String::new(),
            rows: [const { (String::new(), UiTone::Muted) }; recovery_ui::ROWS],
        }
    }
}

static RECOVERY_MODEL: Mutex<ThreadModeRawMutex, RefCell<RecoveryModel>> = Mutex::new(RefCell::new(RecoveryModel::new()));

/// 回復モードの画面の行 (上から)
#[derive(Clone, Copy)]
enum Row {
    LastReset = 0,
    Why = 1,
    Wifi = 2,
    Ota = 3,
    Next = 4,
    Creds = 5,
    Boot = 6,
    Hint = 7,
}

fn set_row(row: Row, tone: UiTone, text: &str) {
    RECOVERY_MODEL.lock(|cell| {
        let mut m = cell.borrow_mut();
        let slot = &mut m.rows[row as usize];
        slot.0.clear();
        for ch in text.chars() {
            if slot.0.push(ch).is_err() {
                break;
            }
        }
        slot.1 = tone;
    });
}

fn draw_recovery(frame: &mut BackBuffer, m: &RecoveryModel) {
    let view = RecoveryView {
        title: "RECOVERY MODE",
        ident: &m.ident,
        rows: core::array::from_fn(|i| (m.rows[i].0.as_str(), m.rows[i].1)),
    };
    let mut canvas = Canvas::new(&mut frame.data);
    recovery_ui::render(&mut canvas, &view);
}

/// 回復モードの描画 (毎フレーム。写真も AA 文字も使わない)
#[embassy_executor::task]
async fn recovery_render_task(mut display: Display) {
    loop {
        supervisor::beat(Who::Render);
        RECOVERY_MODEL.lock(|cell| draw_recovery(display.back(), &cell.borrow()));
        display.present().await;
    }
}

/// 回復モードの OTA の途中経過を画面へ
struct RecoveryOtaUi<'a> {
    ota: &'a mut OtaState,
    slots: Result<Slots, OtaError>,
}

impl RecoveryOtaUi<'_> {
    fn publish(&self) {
        let mut line: String<80> = String::new();
        let (tone, _) = self.ota.write_line(&mut line, &self.slots);
        set_row(Row::Ota, ui_tone(tone), &line);
    }
}

impl OtaUi for RecoveryOtaUi<'_> {
    async fn ota_phase(&mut self, phase: OtaPhase) {
        supervisor::beat(Who::Jobs);
        supervisor::beat(Who::Main);
        self.ota.phase = phase;
        self.publish();
    }
}

struct RecoveryLinkUi;

impl LinkUi for RecoveryLinkUi {
    async fn link_status(&mut self, text: &str, _joined_ssid: Option<&String<32>>) {
        supervisor::beat(Who::Main);
        supervisor::beat(Who::Jobs);
        let mut line: String<80> = String::new();
        let _ = write!(line, "Wi-Fi: {}", text);
        set_row(Row::Wifi, UiTone::Busy, &line);
    }
}

/// 回復モードに渡すもの (main が `embassy_rp::init` の Peripherals から取り出す)
struct RecoveryParts {
    display: Display,
    sd_miso: Peri<'static, PIN_0>,
    sd_cs: Peri<'static, PIN_26>,
    sd_mosi: Peri<'static, PIN_27>,
    sd_clk: Peri<'static, PIN_28>,
    pio1: Peri<'static, PIO1>,
    cyw43: Cyw43Pins,
    flash: Peri<'static, FLASH>,
    boot: BootStatus,
    plan: BootPlan,
}

/// SD の `wifi.txt` だけを、期限付きで読む (回復モードで data 区画に写しが無いとき)
fn read_wifi_txt_only(
    miso: Peri<'static, PIN_0>,
    cs: Peri<'static, PIN_26>,
    mosi: Peri<'static, PIN_27>,
    clk: Peri<'static, PIN_28>,
    deadline: Duration,
) -> Result<WifiCredentials, &'static str> {
    supervisor::feed_init();
    boot_trace::stage(Stage::SdInit);
    sdcard::set_deadline(Some(Instant::now() + deadline));
    let result = match init_sd(miso, cs, mosi, clk) {
        Ok(volume_mgr) => read_credentials(&volume_mgr),
        Err(message) => Err(message),
    };
    sdcard::set_deadline(None);
    supervisor::feed_init();
    result
}

/// 他方区画へ FLASH_UPDATE 起動する (回復モードでも落ち続けた)。印を SCRATCH0 に置き、画面も Wi-Fi も
/// 使わずにすぐ再起動する (壊れている箇所に触れないため)。戻らない。
///
/// bootrom (pico-bootrom-rp2350 `varm_flash_boot.c` / `varm_launch_image.c`) は FLASH_UPDATE の対象区画に
/// 正しいイメージがあれば版数によらずそれを選び、他方 (この版) の方が新しければ、対象が TBYB でない (buy 済み)
/// なら起動時に、この版の区画の先頭セクタを消す (版数の巻き戻し)。以後はその版だけが起動する。
fn fallback_now(boot: &BootStatus) -> ! {
    let Ok(slots) = boot.slots else {
        supervisor::reset_now();
    };
    defmt::error!(
        "recovery kept crashing: FLASH_UPDATE into the other slot P{} (marker for v{})",
        slots.target.index,
        FIRMWARE_VERSION
    );
    boot_trace::stage(Stage::FallbackReboot);
    boot_trace::write_fallback_marker(boot_policy::fallback_marker(IMAGE_DEF_VERSION_WORD));
    ab_boot::reboot_flash_update(slots.target.start_offset(), 100)
}

// ============================================================
// main
// ============================================================

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    // スタック溢れを HardFault にする (MSPLIM) + 最大使用量の計測用に空きスタックを塗る (0.4.1〜)
    supervisor::set_stack_limit();
    supervisor::paint_stack();
    let p = embassy_rp::init(Default::default());
    // 0.4.2〜: ウォッチドッグ (8 s) を何より先に動かす (TBYB 起動では bootrom の 16.7 s を縮めるだけ)。
    // 以後、描画が始まるまでは各段階の前に明示的に再ロードし、どこかで止まれば 8 s でリセットする
    supervisor::start_early();
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
        defmt::warn!("previous boot left a trace: {:?}", trace);
    }
    // --- 起動の方針 (通常 / 回復 / 他方区画へ、boot_policy) ---
    let plan = boot_policy::decide(&boot_inputs(&boot));
    if plan.write_state {
        boot_trace::write_streak(plan.state.encode());
    }
    defmt::info!(
        "boot plan: mode {} fault {} crash streak {} recovery streak {} fell back from {:?}",
        match plan.mode {
            Mode::Normal => "normal",
            Mode::Recovery => "RECOVERY",
            Mode::Fallback => "FALLBACK",
        },
        plan.fault,
        plan.state.crash_streak,
        plan.state.recovery_streak,
        plan.fell_back_from
    );
    // 記録を始める (前回の記録と SCRATCH0 は BootStatus::collect で読んだ)。TBYB の buy 待ちも、buy 済みの版も
    boot_trace::arm();
    boot_trace::stage(Stage::MainEntered);
    let pending = boot.buy == BuyState::Pending;
    if plan.mode == Mode::Fallback {
        fallback_now(&boot);
    }

    // --- LCD (走査はまだ始めない) ---
    supervisor::feed_init();
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
    let cyw43_pins = Cyw43Pins {
        pwr: p.PIN_23,
        cs: p.PIN_25,
        dio: p.PIN_24,
        clk: p.PIN_29,
        dma: p.DMA_CH4,
    };

    if plan.mode == Mode::Recovery {
        spawner
            .spawn(jobs_task(NetWork::Recovery(spawner, RecoveryParts {
                display,
                sd_miso: p.PIN_0,
                sd_cs: p.PIN_26,
                sd_mosi: p.PIN_27,
                sd_clk: p.PIN_28,
                pio1: p.PIO1,
                cyw43: cyw43_pins,
                flash: p.FLASH,
                boot,
                plan,
            })))
            .unwrap();
        return;
    }

    // --- SD カード: wifi.txt と ticker.txt (GPIO SPI は同期処理なので走査開始前に済ませる。期限 5 s) ---
    // ticker.txt は任意: 無い / 空 / 読めない / SD が無い、のどれでも東京の既定値で続ける (ticker::config::load)
    supervisor::feed_init();
    boot_trace::stage(Stage::SdInit);
    sdcard::set_deadline(Some(Instant::now() + SD_BOOT_DEADLINE));
    let mut sd: Option<SdVolumeManager> = None;
    // ticker.txt の中身は BODY (天気 / 文字の受信先、取得タスクを起動する前なので空いている) に読む
    // Safety: 取得タスクを起動する前 (この後 Jobs に渡すまで) は main だけが触る
    let config_buf = unsafe { &mut *addr_of_mut!(BODY) };
    let mut local_message: String<MESSAGE_MAX> = String::new();
    let (config, config_note, credentials): (TickerConfig, String<80>, Result<WifiCredentials, &'static str>) =
        match init_sd(p.PIN_0, p.PIN_26, p.PIN_27, p.PIN_28) {
            Ok(volume_mgr) => {
                let creds = read_credentials(&volume_mgr);
                let buf = &mut config_buf[..config::CONFIG_MAX];
                let mut read = read_root_file(&volume_mgr, "TICKER.TXT", buf);
                // 0.5.0〜: 設定ページの保存が TICKER.TXT の書き換えで失敗すると、新しい内容は TICKER.NEW に残る
                if matches!(read, Ok(0) | Err(ReadError::NotFound))
                    && let Ok(n) = read_root_file(&volume_mgr, "TICKER.NEW", buf)
                    && n > 0
                {
                    defmt::warn!("ticker.txt missing / empty: using TICKER.NEW ({} bytes)", n);
                    read = Ok(n);
                }
                let source = match read {
                    Ok(len) => ConfigSource::Read(&buf[..len]),
                    Err(ReadError::NotFound) => ConfigSource::NotFound,
                    Err(ReadError::Other(e)) => ConfigSource::ReadFailed(e),
                };
                let (config, note) = config::load(source);
                if let Ok(len) = read
                    && let Some(text) = config::message_text(&buf[..len])
                {
                    local_message = message_from(text);
                }
                // 背景の写真 (スライドショー) は走査開始後に別タスクが読む
                sd = Some(volume_mgr);
                (config, note, creds)
            }
            Err(message) => {
                let (config, note) = config::load(ConfigSource::NoCard(message));
                (config, note, Err(message))
            }
        };
    sdcard::set_deadline(None);
    supervisor::feed_init();
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
    boot_trace::stage(Stage::SdRead);
    // 試験用 (debug_crash=): buy 済みの版の通常起動でだけ効く。TBYB の buy 待ちでは無視する
    let debug_crash = if pending { DebugCrash::None } else { config.debug_crash };
    if debug_crash == DebugCrash::Boot {
        panic!("debug_crash=boot");
    }
    let layout = layout_of(config.layout);

    // --- data 区画: Wi-Fi の資格情報の写し (回復モード用) と、入れない版。変わったときだけ書く ---
    let mut flash: OtaFlash = Flash::new_blocking(p.FLASH);
    let mut blocked = 0;
    if let Some(offset) = persist::data_offset() {
        let stored = persist::read(offset);
        let mut record = stored.unwrap_or_default();
        if let Ok(c) = &credentials {
            record = record.with_credentials(c.ssid.as_bytes(), c.password.as_bytes());
        }
        if let Some(from) = plan.fell_back_from {
            record.blocked = record.blocked.max(from);
        }
        blocked = record.blocked;
        if stored != Some(record) && (record.has_credentials() || record.blocked != 0) {
            supervisor::feed_init();
            match persist::write_if_changed(&mut flash, offset, &record) {
                Ok(true) => defmt::info!("persist: record written (blocked {})", record.blocked),
                Ok(false) => {}
                Err(e) => defmt::error!("persist: write failed: {}", e),
            }
            supervisor::feed_init();
        }
    }

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
        m.show_settings = config.show_settings;
        m.layout = layout;
        m.status_mode = config.status;
        m.boot_at = Instant::now();
        m.note_until = Some(Instant::now() + NOTE_SHOW);
        if debug_crash != DebugCrash::None {
            let _ = m.note.push_str("ticker.txt: debug_crash is set (test crash on purpose)");
        } else if !config_note.is_empty() {
            m.note = config_note.clone();
        }
        m.stack_total = supervisor::stack_size();
        let _ = m.ntp.push_str("---");
        let _ = m.wx.push_str("---");
        if config.local_message {
            // ticker.txt の message= (取得しない)
            m.message = local_message.clone();
            m.message_gen = m.message_gen.wrapping_add(1);
            let _ = m.msg.push_str("ok(local)");
        } else {
            let _ = m.msg.push_str("---");
        }
        if let Ok(c) = &credentials {
            m.ssid = ascii_label::<32>(c.ssid.as_bytes());
        }
    });
    slideshow::LIVE.lock(|l| {
        let mut l = l.borrow_mut();
        l.interval_secs = config.slide_secs;
        l.images = config.images.clone();
        l.layout = layout;
    });
    with_model(|m| {
        let _ = m.last_reset.push_str(match boot.reset_reason {
            ResetReason::Hardware => "power-on / reset pin",
            ResetReason::WatchdogTimer => "watchdog timer (update reboot or timeout)",
            ResetReason::WatchdogForce => "forced reset (reboot or recorded fault)",
        });
    });
    publish_ident(&boot);
    publish_diag(&boot, &plan);
    ModelUi {
        ota: &mut ota,
        slots: boot.slots,
    }
    .publish_ota();

    // --- LCD: 初期画面を描いてから走査を開始し、以後は render_task が毎フレーム描く ---
    supervisor::feed_init();
    // 背景は写真を読むまで既定のグラデーション
    slideshow::fill_default(layout);
    MODEL.lock(|cell| {
        let m = cell.borrow();
        slideshow::BG.lock(|bg| draw_screen(display.back(), &bg.borrow()[..], &m, layout.scroll_area(false).1));
    });
    display.start(Irqs);
    boot_trace::stage(Stage::DisplayStarted);
    spawner.spawn(render_task(display)).unwrap();
    // 生存確認つきの監視 (TBYB の buy 待ちも。締め切りを過ぎたら再ロードをやめて旧版へ戻る)
    supervisor::start(Limits::TICKER);
    if pending {
        supervisor::set_buy_deadline(Some(boot_policy::BUY_DEADLINE_MS));
    } else {
        boot_trace::stage(Stage::Running);
        supervisor::allow_streak_clear(true);
        DOWNLOAD_OK.store(true, Ordering::Relaxed);
    }
    let sd: Option<&'static SdVolumeManager> = sd.map(|volume_mgr| &*SD_CARD.init(volume_mgr));
    match sd {
        Some(volume_mgr) => spawner
            .spawn(slideshow::slideshow_task(
                volume_mgr,
                SlideConfig {
                    sd_fast: config.sd_fast,
                    start_by: Instant::now() + SLIDESHOW_START_MAX,
                    crash_on_first: debug_crash == DebugCrash::Slideshow,
                },
            ))
            .unwrap(),
        // SD が無ければ写真の一巡は済み (試すものが無い)
        None => slideshow::FIRST_DONE.store(true, Ordering::Relaxed),
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
        cyw43_pins,
        PowerManagementMode::Performance,
        || boot_trace::stage(Stage::WifiInit),
    )
    .await;
    boot_trace::stage(Stage::WifiReady);
    supervisor::beat(Who::Main);

    // --- フラッシュと HTTP クライアントの資源 (取得タスクへ渡す) ---
    // Safety: これらの static は取得タスクからしか触らず、取得タスクは 1 つだけ。
    let tcp_state = unsafe { &*addr_of_mut!(TCP_STATE) };
    let ota_possible = credentials.is_ok() && boot.slots.is_ok();
    spawner
        .spawn(jobs_task(NetWork::Normal(Jobs {
            net: Net::new(stack, tcp_state),
            stack,
            bufs: unsafe { &mut *addr_of_mut!(NET_BUFFERS) },
            flash,
            sectors: unsafe { &mut *addr_of_mut!(SECTOR_BUFFERS) },
            sntp_bufs: unsafe { &mut *addr_of_mut!(SNTP_BUFFERS) },
            body: unsafe { &mut *addr_of_mut!(BODY) },
            config,
            ota,
            slots: boot.slots,
            ota_possible,
            blocked,
            crash_before_ota: debug_crash == DebugCrash::Ota,
            sd,
        })))
        .unwrap();

    let mut link = LinkManager::new(&credentials);
    let mut ticks: u32 = 0;
    let mut stack_logged: u32 = 0;

    loop {
        supervisor::beat(Who::Main);
        // --- 接続管理 (join → DHCP → 通らなければ離脱して再 join。ota::app::LinkManager) ---
        let network_up = link.step(&mut control, stack, &credentials, &mut LinkModelUi).await;
        supervisor::beat(Who::Main);

        // --- TBYB: buy 条件 (boot_policy::BuyGate) = Wi-Fi + DHCP、OTA 確認が TLS + HTTP を通った、
        //     NTP / 天気 / 文字 / SD の設定 / 最初の写真を 1 回ずつ試した、生存確認が揃って 25 s ---
        if pending && boot.buy == BuyState::Pending {
            let round = Round {
                sd_config: true,
                ntp: ROUND_NTP.load(Ordering::Relaxed),
                weather: ROUND_WEATHER.load(Ordering::Relaxed),
                message: ROUND_MESSAGE.load(Ordering::Relaxed),
                slideshow: slideshow::FIRST_DONE.load(Ordering::Relaxed),
                web: ROUND_WEB.load(Ordering::Relaxed),
            };
            let healthy = supervisor::all_alive() && supervisor::alive(Who::Render, 2_000);
            if boot.buy_tick(network_up, OTA_PROVED.load(Ordering::Relaxed), round, healthy)
                && matches!(boot.buy, BuyState::Bought | BuyState::Failed(_))
            {
                // explicit_buy は bootrom がウォッチドッグを止めるので、すぐ動かし直す
                supervisor::rearm();
                if boot.buy == BuyState::Bought {
                    boot_trace::stage(Stage::Running);
                    // buy した版の回数は 0 から (旧版の SCRATCH1 は buy 待ちの間は触らない)
                    boot_trace::write_streak(BootState::default().encode());
                    supervisor::allow_streak_clear(true);
                    DOWNLOAD_OK.store(true, Ordering::Relaxed);
                    // buy 待ちの間に新しい版を見つけていてもよいように、すぐ確認し直す (今度はダウンロードまで)
                    OTA_NOW.store(true, Ordering::Relaxed);
                }
            }
        }

        // buy 待ちの間も OTA 確認 (manifest だけ) と取得は行う (buy 条件の一巡)。締め切り後は行わない
        NET_READY.store(network_up && boot.ota_check_allowed(), Ordering::Relaxed);

        // --- 検証済みイメージへの FLASH_UPDATE 再起動 / 巻き戻されたイメージの再試行 (取得タスクの依頼) ---
        if let Some(slots) = REBOOT_REQUEST.try_take() {
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
            boot_trace::clear();
            app::reboot_into_slot(&mut control, slots).await;
        }

        // --- 設定ページの「再起動」(buy 済みのときだけ受け付けている)。意図した再起動なので記録は消す ---
        if WEB_REBOOT.load(Ordering::Relaxed) {
            with_model(|m| {
                m.wifi.clear();
                let _ = m.wifi.push_str("rebooting (settings page)... wifi off");
                m.wifi_tone = Tone::Busy;
            });
            Timer::after(Duration::from_millis(50)).await;
            wifi::power_off_for_reboot(&mut control).await;
            boot_trace::clear();
            supervisor::reset_now();
        }

        // --- 状態行 (Wi-Fi) を更新 ---
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
        } else if credentials.is_ok()
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

        // --- スタックの最大使用量 (1 s ごと。増えたら defmt にも出す) ---
        if ticks.is_multiple_of(4) {
            let used = supervisor::stack_used();
            if used >= stack_logged + 256 {
                stack_logged = used;
                defmt::info!("stack high-water: {} of {} B", used, supervisor::stack_size());
            }
            with_model(|m| m.stack_used = used);
        }
        ticks = ticks.wrapping_add(1);
        publish_ident(&boot);

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
