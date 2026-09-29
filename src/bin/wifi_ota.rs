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
use core::sync::atomic::{AtomicBool, Ordering};

use cyw43::{PowerManagementMode, ScanOptions};
use embassy_executor::Spawner;
use embassy_net::dns::DnsSocket;
use embassy_net::tcp::client::{TcpClient, TcpClientState};
use embassy_rp::bind_interrupts;
use embassy_rp::clocks::RoscRng;
use embassy_rp::flash::Flash;
use embassy_rp::pac::WATCHDOG;
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
use heapless::{String, Vec};
use pico2w_300yen_lcd::ab_boot::{self, BootInfo};
use pico2w_300yen_lcd::boot_trace::{self, ResetReason, SelftestCounters, Stage, Trace};
use pico2w_300yen_lcd::image_def::{FIRMWARE_VERSION, TBYB};
use pico2w_300yen_lcd::lcd::display::{BACK_HEIGHT, BACK_WIDTH, BackBuffer, Display, DisplayPins, FrameIrqHandler};
use pico2w_300yen_lcd::lcd::framebuffer::BLACK;
use pico2w_300yen_lcd::lcd::timing::H_ACTIVE;
use pico2w_300yen_lcd::ota::http::{self, BodySink};
use pico2w_300yen_lcd::ota::manifest::{Manifest, Version};
use pico2w_300yen_lcd::ota::slot::{self, OtaFlash, SectorBuffers, SlotWriter, Slots, slot_label};
use pico2w_300yen_lcd::ota::{self, MANIFEST_NAME, OtaError, URL_MAX};
use pico2w_300yen_lcd::sdcard::init_sd;
use pico2w_300yen_lcd::usb_reset::build_usb_device;
use pico2w_300yen_lcd::wifi::{
    self, ApEntry, Cyw43Pins, MAX_SCAN_APS, WifiCredentials, ascii_label, merge_ap, read_credentials,
};
use reqwless::client::{HttpClient, TlsConfig, TlsVerify};
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
// 動作パラメータ
// ============================================================

/// manifest を確認する周期 (DHCP 完了後の初回は `OTA_FIRST_CHECK_DELAY` 後)
const OTA_CHECK_INTERVAL: Duration = Duration::from_secs(60);
const OTA_FIRST_CHECK_DELAY: Duration = Duration::from_secs(5);
/// 失敗時のバックオフ (倍々、上限 10 分)
const OTA_BACKOFF_MIN: Duration = Duration::from_secs(60);
const OTA_BACKOFF_MAX: Duration = Duration::from_secs(600);
/// manifest 取得 / bin ダウンロードの全体タイムアウト
const MANIFEST_TIMEOUT: Duration = Duration::from_secs(30);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);
/// TCP ソケットの無通信タイムアウト
const SOCKET_TIMEOUT: Duration = Duration::from_secs(20);
/// 対象区画に manifest と同一のイメージが既にある (= 前回 TBYB 起動で buy されず戻ってきた) とき、
/// もう一度 FLASH_UPDATE 起動を試すまでの待ち時間
const REJECTED_RETRY_DELAY: Duration = Duration::from_secs(600);
/// 検証完了から再起動までの表示時間
const REBOOT_AFTER: Duration = Duration::from_secs(2);
/// ダウンロード中の進捗再描画間隔
const PROGRESS_REDRAW: Duration = Duration::from_millis(250);

/// TBYB 起動時、explicit_buy を許す最短稼働時間 (LCD 走査が回っていることの確認)
const BUY_MIN_UPTIME: Duration = Duration::from_secs(2);
/// TBYB 自己診断の締め切り (起動からの秒数)。この間は `tbyb_watchdog_task` が bootrom のウォッチドッグを
/// 延長し続ける。過ぎたら延長をやめて buy もしない (最長 16.7 s 後にウォッチドッグで旧版へ戻る)。
/// join 再試行 (5〜60 s) + DHCP (最大 20 s) を数回やり直せる長さ。
const TBYB_SELFTEST_DEADLINE_SECS: u64 = 120;
/// ウォッチドッグを再ロードする周期 (16.7 s に対して十分短く、フラッシュ操作や scan の待ちより長い)
const TBYB_WATCHDOG_FEED_INTERVAL: Duration = Duration::from_secs(2);
/// WATCHDOG.LOAD に書く値。24 bit × 1 µs = 16.7 s で、bootrom が TBYB 起動時に設定するのと同じ最大値。
/// LOAD は書き込み専用でカウンタを再ロードするだけ (CTRL の ENABLE / PAUSE_* や、reboot パラメータが
/// 入っている SCRATCH2〜7 には触れない)。
const WATCHDOG_LOAD_MAX: u32 = 0x00ff_ffff;

/// wifi_status と同じスキャン / 接続パラメータ
const SCAN_PERIOD: Duration = Duration::from_secs(10);
const DHCP_TIMEOUT: Duration = Duration::from_secs(20);
/// DHCP タイムアウト後の `leave()` に許す時間と、離脱してから再 join するまでの間
const DHCP_REJOIN_LEAVE_TIMEOUT: Duration = Duration::from_secs(2);
const DHCP_REJOIN_DELAY: Duration = Duration::from_millis(500);
const JOIN_RETRY_MIN: Duration = Duration::from_secs(5);
const JOIN_RETRY_MAX: Duration = Duration::from_secs(60);
/// メインループの周期 (画面更新)
const TICK: Duration = Duration::from_millis(500);
/// 表示する AP 数 (上位 RSSI)。OTA 行 2 本 + 進捗バーの分だけ wifi_status より少ない
const MAX_DISPLAY_APS: usize = 5;

/// TLS レコードバッファ。受信側は 16 kB のレコード + 128 B のオーバーヘッドが必要 (embedded-tls)。
/// 送信側はリクエスト行 (URL 最大 2 kB) + ヘッダ + オーバーヘッドが入ればよい (超えれば複数レコードに分割される)。
const TLS_RX_SIZE: usize = 16384 + 256;
const TLS_TX_SIZE: usize = 3072;
/// TCP ソケットバッファ (受信ウィンドウがダウンロード速度を決める。RAM 節約のため 4 kB)
const TCP_RX_SIZE: usize = 4096;
const TCP_TX_SIZE: usize = 2048;
/// HTTP 応答ヘッダ用。github.com の 302 は Content-Security-Policy (約 3.7 kB) と
/// Set-Cookie 3 本を含めて 5.0〜5.9 kB (2026-09 実測)。reqwless はヘッダ終端がこのバッファに
/// 収まらないと `BufferTooSmall` を返す (v0.2.0 の 4 kB では "bad HTTP response" になった)。
const HTTP_HEADER_SIZE: usize = 8192;
/// 本文の受信単位
const CHUNK_SIZE: usize = 2048;
/// manifest.json の上限
const MANIFEST_MAX: usize = 512;

// ============================================================
// static 配置のバッファ (BSS。大きな配列を future / スタックに置かない)
// ============================================================

struct NetBuffers {
    tls_rx: [u8; TLS_RX_SIZE],
    tls_tx: [u8; TLS_TX_SIZE],
    http_rx: [u8; HTTP_HEADER_SIZE],
    chunk: [u8; CHUNK_SIZE],
    manifest: [u8; MANIFEST_MAX],
    url: String<URL_MAX>,
}

static mut NET_BUFFERS: NetBuffers = NetBuffers {
    tls_rx: [0; TLS_RX_SIZE],
    tls_tx: [0; TLS_TX_SIZE],
    http_rx: [0; HTTP_HEADER_SIZE],
    chunk: [0; CHUNK_SIZE],
    manifest: [0; MANIFEST_MAX],
    url: String::new(),
};
static mut TCP_STATE: TcpClientState<1, TCP_TX_SIZE, TCP_RX_SIZE> = TcpClientState::new();
static mut SECTOR_BUFFERS: SectorBuffers = SectorBuffers::new();

// ============================================================
// TBYB: bootrom のウォッチドッグの延長
// ============================================================

/// true の間 `tbyb_watchdog_task` がウォッチドッグを再ロードする。main が buy 待ちの開始時に立て、
/// explicit_buy の後 (成否によらず) に落とす。締め切りを過ぎたらタスク自身が落とす。
static TBYB_FEEDING: AtomicBool = AtomicBool::new(false);

/// ウォッチドッグのカウンタを最大値 (16.7 s) に再ロードする。embassy の `Watchdog` は使わない
/// (`Watchdog::start` は CTRL / PAUSE / SCRATCH を書き換え、bootrom が TBYB 用に設定した状態を壊す)。
fn feed_watchdog() {
    WATCHDOG.load().write(|w| w.set_load(WATCHDOG_LOAD_MAX));
}

/// buy 待ちの間、`TBYB_WATCHDOG_FEED_INTERVAL` ごとに bootrom のウォッチドッグを再ロードする。
/// main ループは join / DHCP / scan で数秒〜20 s 待つので、独立したタスクで回す。
/// `deadline` (起動 + `TBYB_SELFTEST_DEADLINE_SECS`) を過ぎたら再ロードをやめて終わる。以後は
/// 最長 16.7 s でウォッチドッグが発火し、旧版で通常起動する (データシート §5.1.17)。
#[embassy_executor::task]
async fn tbyb_watchdog_task(deadline: Instant) {
    while TBYB_FEEDING.load(Ordering::Relaxed) {
        if Instant::now() >= deadline {
            defmt::warn!(
                "TBYB self-test deadline ({} s) passed without explicit_buy; stop feeding, watchdog will roll back",
                TBYB_SELFTEST_DEADLINE_SECS
            );
            TBYB_FEEDING.store(false, Ordering::Relaxed);
            break;
        }
        feed_watchdog();
        boot_trace::heartbeat();
        Timer::after(TBYB_WATCHDOG_FEED_INTERVAL).await;
    }
    defmt::info!("tbyb_watchdog_task done");
}

// ============================================================
// 状態
// ============================================================

/// TBYB の進行状態 (ota_selftest と同じ)
#[derive(Clone, Copy, PartialEq, Eq)]
enum BuyState {
    NotTbyb,
    /// TBYB 起動。自己診断 (Wi-Fi + DHCP) が通ったら buy する。この間はウォッチドッグを延長している
    Pending,
    /// 締め切り (`TBYB_SELFTEST_DEADLINE_SECS`) までに自己診断が通らなかった。延長をやめ、buy もしない。
    /// 最長 16.7 s 後にウォッチドッグで旧版へ戻る
    TimedOut,
    Bought,
    Failed(i32),
}

struct BootStatus {
    boot: Option<BootInfo>,
    slots: Result<Slots, OtaError>,
    buy: BuyState,
    boot_at: Instant,
    /// 直前のリセットがウォッチドッグ由来か (FLASH_UPDATE 再起動も TBYB の巻き戻りもこれ)
    reset_reason: ResetReason,
    /// 前回の TBYB 起動が SCRATCH5〜7 に残した記録 (巻き戻り後の旧版で見える)
    prev_trace: Option<Trace>,
}

impl BootStatus {
    /// `boot_trace::arm()` より前に呼ぶ (前回の記録を読んでから上書きする)
    fn collect() -> Self {
        let boot = BootInfo::read();
        let buy = match boot {
            Some(b) if b.buy_pending() => BuyState::Pending,
            _ => BuyState::NotTbyb,
        };
        Self {
            boot,
            slots: slot::find_slots(),
            buy,
            boot_at: Instant::now(),
            reset_reason: ResetReason::read(),
            prev_trace: boot_trace::read(),
        }
    }

    /// 起動診断行 (起動種別 / 診断ワード / リセット理由) を出すか。電源投入直後の通常起動では出さない
    fn show_boot_line(&self) -> bool {
        self.reset_reason != ResetReason::Hardware || self.prev_trace.is_some()
    }

    /// TBYB 自己診断の締め切り時刻
    fn selftest_deadline(&self) -> Instant {
        self.boot_at + Duration::from_secs(TBYB_SELFTEST_DEADLINE_SECS)
    }

    /// 今の起動が `reboot(FLASH_UPDATE)` 由来か (= OTA で書いたイメージが動いている)
    fn is_ota_boot(&self) -> bool {
        self.boot
            .as_ref()
            .is_some_and(|b| b.boot_type & !ab_boot::BOOT_TYPE_CHAINED_FLAG == ab_boot::BOOT_TYPE_FLASH_UPDATE)
    }
}

/// OTA の進行状態 (LCD の OTA 行に出す)
#[derive(Clone, Copy)]
enum OtaPhase {
    /// まだ確認していない
    Idle,
    /// wifi.txt が無い等で確認できない
    Disabled(&'static str),
    Checking,
    /// manifest が 404 (Release 無し)
    NoRelease,
    UpToDate { latest: Version },
    Downloading { version: Version, received: u32, total: u32 },
    Verifying { version: Version },
    /// 検証済み。`REBOOT_AFTER` 後に FLASH_UPDATE 再起動
    Rebooting { version: Version, at: Instant },
    /// 対象区画に同じイメージが既にある (前回 buy されなかった)。`retry_at` に再起動を試す
    Rejected { version: Version, retry_at: Instant },
    Failed { error: OtaError, retry_at: Instant },
}

struct OtaState {
    phase: OtaPhase,
    next_check: Option<Instant>,
    backoff: Duration,
    checks: u32,
    /// 最初に `Rejected` と判定した版とその再試行時刻。`run_ota_check` は開始時に phase を `Checking` に
    /// するので、phase からは「前回も同じ版で Rejected だった」ことが分からない。ここに保ち、同じ版なら
    /// 再試行時刻を動かさない。別の結果 (最新 / 新版あり / Release 無し) が出たら消す。
    rejected: Option<(Version, Instant)>,
}

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

/// ウォッチドッグの残り時間 (0.1 秒単位)。無効なら None。
fn watchdog_remaining_tenths() -> Option<u32> {
    let ctrl = embassy_rp::pac::WATCHDOG.ctrl().read();
    if ctrl.enable() { Some(ctrl.time() / 100_000) } else { None }
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

fn secs_until(at: Instant) -> u64 {
    at.saturating_duration_since(Instant::now()).as_secs()
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
    match &model.boot.slots {
        Ok(slots) => {
            let _ = write!(line, " slot {}", slot_label(&slots.own));
        }
        Err(e) => {
            let _ = write!(line, " slot ?({})", e.label());
        }
    }
    // TBYB フラグ無しのビルド (USB で入れる wifi_ota-plain) だけ印を出す。OTA で届くイメージは常に TBYB 付き
    if !TBYB {
        let _ = line.push_str(" plain");
    }
    let tbyb_color = match model.boot.buy {
        BuyState::NotTbyb => {
            let _ = line.push_str(" TBYB:no");
            GRAY
        }
        BuyState::Pending => {
            // ウォッチドッグを延長しながら自己診断中。締め切りまでの経過秒を出す
            let _ = write!(
                line,
                " TBYB:pending {}/{}s",
                model.boot.boot_at.elapsed().as_secs().min(TBYB_SELFTEST_DEADLINE_SECS),
                TBYB_SELFTEST_DEADLINE_SECS
            );
            YELLOW
        }
        BuyState::TimedOut => {
            let _ = line.push_str(" TBYB:timeout->rollback");
            RED
        }
        BuyState::Bought => {
            let _ = line.push_str(" TBYB:bought OK");
            GREEN
        }
        BuyState::Failed(rc) => {
            let _ = write!(line, " TBYB:buy FAILED rc={}", rc);
            RED
        }
    };
    if let Some(t) = watchdog_remaining_tenths() {
        let _ = write!(line, " WDT {}.{}s", t / 10, t % 10);
    }
    draw_text(frame, &line, rest_x, OTA_ID_Y, tbyb_color);

    // 行 2: OTA の状態
    line.clear();
    let mut progress: Option<(u32, u32)> = None;
    let color = match model.ota.phase {
        OtaPhase::Idle => {
            let _ = line.push_str("OTA: waiting for network");
            if let Some(next) = model.ota.next_check {
                let _ = write!(line, ", first check in {}s", secs_until(next));
            }
            GRAY
        }
        OtaPhase::Disabled(reason) => {
            let _ = write!(line, "OTA: disabled ({})", reason);
            GRAY
        }
        OtaPhase::Checking => {
            let _ = write!(line, "OTA: checking {} (#{})...", MANIFEST_NAME, model.ota.checks);
            WHITE
        }
        OtaPhase::NoRelease => {
            let _ = line.push_str("OTA: no release yet (404)");
            if let Some(next) = model.ota.next_check {
                let _ = write!(line, ", next check in {}s", secs_until(next));
            }
            GRAY
        }
        OtaPhase::UpToDate { latest } => {
            let _ = write!(line, "OTA: up to date (latest {})", latest);
            if let Some(next) = model.ota.next_check {
                let _ = write!(line, ", next check in {}s", secs_until(next));
            }
            GREEN
        }
        OtaPhase::Downloading {
            version,
            received,
            total,
        } => {
            let pct = if total > 0 { (received as u64 * 100 / total as u64) as u32 } else { 0 };
            let _ = write!(
                line,
                "OTA: {} -> {} downloading {}%  {}/{} B",
                Version::CURRENT,
                version,
                pct,
                received,
                total
            );
            progress = Some((received, total));
            YELLOW
        }
        OtaPhase::Verifying { version } => {
            let _ = write!(line, "OTA: {} downloaded, verifying (sha256 + readback)...", version);
            progress = Some((1, 1));
            YELLOW
        }
        OtaPhase::Rebooting { version, at } => {
            let slot = model
                .boot
                .slots
                .as_ref()
                .map(|s| slot_label(&s.target))
                .unwrap_or("?");
            let _ = write!(
                line,
                "OTA: {} verified -> reboot into slot {} in {}s (TBYB)",
                version,
                slot,
                secs_until(at)
            );
            progress = Some((1, 1));
            GREEN
        }
        OtaPhase::Rejected { version, retry_at } => {
            let slot = model
                .boot
                .slots
                .as_ref()
                .map(|s| slot_label(&s.target))
                .unwrap_or("?");
            let _ = write!(
                line,
                "OTA: {} in slot {} was rolled back; retry boot in {}s",
                version,
                slot,
                secs_until(retry_at)
            );
            RED
        }
        OtaPhase::Failed { error, retry_at } => {
            match error {
                OtaError::HttpStatus(code) => {
                    let _ = write!(line, "OTA: HTTP {}", code);
                }
                other => {
                    let _ = write!(line, "OTA: {}", other.label());
                }
            }
            let _ = write!(line, ", retry in {}s", secs_until(retry_at));
            RED
        }
    };
    draw_text(frame, &line, TEXT_X, OTA_STATE_Y, color);

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
    if let Some(trace) = &model.boot.prev_trace {
        line.clear();
        let _ = write!(
            line,
            "TBYB {}.{}.{}: {} @{}.{}s",
            trace.major,
            trace.minor / 100,
            trace.minor % 100,
            trace.stage_label(),
            trace.uptime_ds / 10,
            trace.uptime_ds % 10
        );
        let color = match trace.stage {
            Some(Stage::Panic) => {
                let _ = write!(line, " line={}", trace.info);
                RED
            }
            Some(Stage::HardFault) => {
                let _ = write!(line, " pc={:#010x}", trace.info);
                RED
            }
            Some(Stage::BuyFailed) => {
                let _ = write!(line, " rc={}", trace.info as i32);
                RED
            }
            stage => {
                let c = trace.counters();
                let _ = write!(line, " join{} fail{} dhcpto{}", c.join_attempts, c.join_failures, c.dhcp_timeouts);
                if c.last_join_status != 0 {
                    let _ = write!(line, " st{}", c.last_join_status);
                }
                if stage == Some(Stage::SelftestTimedOut) { RED } else { YELLOW }
            }
        };
        draw_text(frame, &line, TEXT_X, next_diag_y, color);
        next_diag_y += ROW_HEIGHT;
    }
    if model.boot.show_boot_line() {
        line.clear();
        match &model.boot.boot {
            Some(b) => {
                let (a, bh) = b.diagnostic_halves();
                let _ = write!(
                    line,
                    "{} P{} A:{:04X} {} B:{:04X} {}",
                    b.boot_type_name(),
                    b.partition,
                    a,
                    ab_boot::diagnostic_summary(a),
                    bh,
                    ab_boot::diagnostic_summary(bh)
                );
            }
            None => {
                let _ = line.push_str("boot ? (BOOT_INFO n/a)");
            }
        }
        let _ = write!(line, " reset:{}", model.boot.reset_reason.label());
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
// OTA 本体
// ============================================================

type Tcp<'a> = TcpClient<'a, 1, TCP_TX_SIZE, TCP_RX_SIZE>;

struct Net<'a> {
    tcp: Tcp<'a>,
    dns: DnsSocket<'a>,
}

/// manifest.json を `MANIFEST_MAX` まで受ける
struct ManifestSink<'a> {
    buf: &'a mut [u8; MANIFEST_MAX],
    len: usize,
}

impl BodySink for ManifestSink<'_> {
    async fn push(&mut self, data: &[u8]) -> Result<(), OtaError> {
        if self.len + data.len() > self.buf.len() {
            return Err(OtaError::Manifest);
        }
        self.buf[self.len..self.len + data.len()].copy_from_slice(data);
        self.len += data.len();
        Ok(())
    }
}

/// bin を他方区画へ書きながら進捗を描く
struct DownloadSink<'a, 'f> {
    writer: &'a mut SlotWriter<'f>,
    ui: &'a mut Ui,
    version: Version,
    last_draw: Instant,
}

impl BodySink for DownloadSink<'_, '_> {
    async fn push(&mut self, data: &[u8]) -> Result<(), OtaError> {
        // セクタがたまるごとに消去 (45〜400 ms) + 書き込み。割り込み禁止中も LCD の DMA リングは
        // SRAM だけを読むので走査は乱れない (docs/ota-design.md §4.1)。
        self.writer.push(data)?;
        if self.last_draw.elapsed() >= PROGRESS_REDRAW {
            self.ui.model.ota.phase = OtaPhase::Downloading {
                version: self.version,
                received: self.writer.received(),
                total: self.writer.expected(),
            };
            self.ui.present().await;
            self.last_draw = Instant::now();
        }
        Ok(())
    }
}

/// 1 回の更新確認。戻り値の `OtaPhase` は NoRelease / UpToDate / Rejected / Rebooting のいずれか。
async fn run_ota_check(
    net: &Net<'_>,
    bufs: &mut NetBuffers,
    flash: &mut OtaFlash,
    sectors: &mut SectorBuffers,
    slots: Slots,
    ui: &mut Ui,
) -> Result<OtaPhase, OtaError> {
    ui.model.ota.phase = OtaPhase::Checking;
    ui.present().await;

    // 接続ごとに乱数シードを変える (reqwless は seed から ChaCha8 を毎回作り直す)
    let seed = RoscRng.next_u64();
    let tls = TlsConfig::new(seed, &mut bufs.tls_rx, &mut bufs.tls_tx, TlsVerify::None);
    let mut client = HttpClient::new_with_tls(&net.tcp, &net.dns, tls);

    // --- [1] manifest ---
    bufs.url = ota::latest_asset_url(MANIFEST_NAME);
    let manifest_len = {
        let mut sink = ManifestSink {
            buf: &mut bufs.manifest,
            len: 0,
        };
        let fetched = with_timeout(
            MANIFEST_TIMEOUT,
            http::fetch(&mut client, &mut bufs.url, &mut bufs.http_rx, &mut bufs.chunk, &mut sink),
        )
        .await
        .map_err(|_| OtaError::Timeout)??;
        match fetched.status {
            200 => sink.len,
            404 => return Ok(OtaPhase::NoRelease),
            code => return Err(OtaError::HttpStatus(code)),
        }
    };
    let manifest = Manifest::parse(&bufs.manifest[..manifest_len])?;
    defmt::info!(
        "manifest: version {} bin {} size {} (current {})",
        manifest.version,
        manifest.bin.as_str(),
        manifest.size,
        Version::CURRENT
    );
    if !manifest.is_newer_than_current() {
        return Ok(OtaPhase::UpToDate {
            latest: manifest.version,
        });
    }
    if manifest.size == 0 || manifest.size > slots.target.size() {
        return Err(OtaError::BadSize);
    }

    // --- [2] 対象区画に同じイメージが既にあるなら、前回 TBYB で起動して buy されなかったもの ---
    if slot::hash_storage(slots.target.start_offset(), manifest.size) == manifest.sha256 {
        defmt::warn!("target slot already holds this image (rolled back before?)");
        return Ok(OtaPhase::Rejected {
            version: manifest.version,
            retry_at: Instant::now() + REJECTED_RETRY_DELAY,
        });
    }

    // --- [3] ダウンロードしながら書き込み ---
    ui.model.ota.phase = OtaPhase::Downloading {
        version: manifest.version,
        received: 0,
        total: manifest.size,
    };
    ui.present().await;
    bufs.url = ota::latest_asset_url(&manifest.bin);
    let mut writer = SlotWriter::new(flash, sectors, &slots.target, manifest.size)?;
    writer.begin()?; // 先頭セクタを消して無効化

    let fetched = {
        let mut sink = DownloadSink {
            writer: &mut writer,
            ui,
            version: manifest.version,
            last_draw: Instant::now(),
        };
        let result = with_timeout(
            DOWNLOAD_TIMEOUT,
            http::fetch(&mut client, &mut bufs.url, &mut bufs.http_rx, &mut bufs.chunk, &mut sink),
        )
        .await;
        match result {
            Ok(Ok(fetched)) => fetched,
            Ok(Err(e)) => {
                let _ = writer.invalidate();
                return Err(e);
            }
            Err(_) => {
                let _ = writer.invalidate();
                return Err(OtaError::Timeout);
            }
        }
    };
    if fetched.status != 200 {
        let _ = writer.invalidate();
        return Err(OtaError::HttpStatus(fetched.status));
    }

    // --- [4] 検証: 受信サイズ / SHA-256 → 先頭セクタ書き込み → 読み戻し SHA-256 ---
    let (digest, received) = writer.finish()?;
    if received != manifest.size {
        let _ = writer.invalidate();
        return Err(OtaError::SizeMismatch);
    }
    if digest != manifest.sha256 {
        let _ = writer.invalidate();
        return Err(OtaError::ShaMismatch);
    }
    ui.model.ota.phase = OtaPhase::Verifying {
        version: manifest.version,
    };
    ui.present().await;
    writer.commit_first_sector()?;
    let readback = slot::hash_storage(slots.target.start_offset(), manifest.size);
    if readback != manifest.sha256 {
        let _ = writer.invalidate();
        return Err(OtaError::ReadbackMismatch);
    }
    defmt::info!(
        "image {} written to slot {} (P{}) and verified, {} sectors",
        manifest.version,
        slot_label(&slots.target),
        slots.target.index,
        writer.sectors_written()
    );
    Ok(OtaPhase::Rebooting {
        version: manifest.version,
        at: Instant::now() + REBOOT_AFTER,
    })
}

// ============================================================
// main
// ============================================================

enum Link {
    ScanOnly(&'static str),
    Disconnected {
        next_attempt: Instant,
        retry: Duration,
        last_error: Option<u32>,
    },
    Joined,
}

/// 対象区画への FLASH_UPDATE 再起動。先に AP から離脱して CYW43 の電源 (WL_REG_ON) を落とし、
/// 次の版が接続中のチップを引き継がないようにする (`wifi::power_off_for_reboot`)。戻らない。
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
    wifi::power_off_for_reboot(control).await;
    ab_boot::reboot_flash_update(slots.target.start_offset(), 100)
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
    // TBYB 起動なら bootrom のウォッチドッグ (16.7 s) が既に走っている。SD / LCD / Wi-Fi の初期化が
    // 先に来るので、まず一度再ロードし、以後は tbyb_watchdog_task に任せる。進行は boot_trace に記録する
    // (巻き戻ったときに旧版が読む。prev_trace を読んだ後に arm する)。
    if boot.buy == BuyState::Pending {
        feed_watchdog();
        boot_trace::arm();
        boot_trace::stage(Stage::MainEntered);
        TBYB_FEEDING.store(true, Ordering::Relaxed);
        spawner.spawn(tbyb_watchdog_task(boot.selftest_deadline())).unwrap();
        boot_trace::stage(Stage::FeedStarted);
        defmt::info!(
            "TBYB buy pending: extending the watchdog every {} s until self-test passes (deadline {} s)",
            TBYB_WATCHDOG_FEED_INTERVAL.as_secs(),
            TBYB_SELFTEST_DEADLINE_SECS
        );
    }

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
            ota: OtaState {
                phase: OtaPhase::Idle,
                next_check: None,
                backoff: OTA_BACKOFF_MIN,
                checks: 0,
                rejected: None,
            },
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
    let mut tcp: Tcp<'_> = TcpClient::new(stack, tcp_state);
    tcp.set_timeout(Some(SOCKET_TIMEOUT));
    let net = Net {
        tcp,
        dns: DnsSocket::new(stack),
    };

    let mut link = match &credentials {
        Ok(_) => Link::Disconnected {
            next_attempt: Instant::now(),
            retry: JOIN_RETRY_MIN,
            last_error: None,
        },
        Err(message) => Link::ScanOnly(message),
    };
    let mut next_scan = Instant::now();
    let mut scan_count: u32 = 0;
    let ota_possible = credentials.is_ok() && ui.model.boot.slots.is_ok();
    // TBYB 自己診断の進み具合 (boot_trace の SCRATCH7 に書く。巻き戻ったとき旧版に見える)
    let mut counters = SelftestCounters::default();
    let mut network_was_up = false;

    loop {
        let now = Instant::now();

        // --- 接続管理 (wifi_status と同じ) ---
        if let (Ok(creds), Link::Joined) = (&credentials, &link)
            && !stack.is_link_up()
        {
            defmt::warn!("link down, will rejoin {}", creds.ssid.as_str());
            link = Link::Disconnected {
                next_attempt: now,
                retry: JOIN_RETRY_MIN,
                last_error: None,
            };
            ui.model.joined_ssid = None;
        }
        if let (Ok(creds), Link::Disconnected { next_attempt, retry, .. }) = (&credentials, &link)
            && now >= *next_attempt
        {
            let retry = *retry;
            ui.model.status.clear();
            let _ = write!(
                ui.model.status,
                "connecting to {}...",
                ascii_label::<32>(creds.ssid.as_bytes())
            );
            ui.present().await;
            counters.join_attempts = counters.join_attempts.saturating_add(1);
            boot_trace::stage(Stage::Joining);
            boot_trace::info(counters.pack());
            match control.join(creds.ssid.as_str(), creds.join_options()).await {
                Ok(()) => {
                    defmt::info!("joined {}", creds.ssid.as_str());
                    boot_trace::stage(Stage::Joined);
                    ui.model.status.clear();
                    let _ = write!(
                        ui.model.status,
                        "{}: waiting for DHCP...",
                        ascii_label::<32>(creds.ssid.as_bytes())
                    );
                    ui.model.joined_ssid = Some(creds.ssid.clone());
                    ui.present().await;
                    boot_trace::stage(Stage::DhcpWait);
                    if with_timeout(DHCP_TIMEOUT, stack.wait_config_up()).await.is_err() {
                        // DHCP が通らない。association はあるのにデータが流れない状態 (v0.2.5 の TBYB 起動で
                        // 観測: join1 dhcpto1 のまま 120 s) から抜けるため、AP から離脱して次のループで
                        // 再 join する (v0.2.5 までは Joined のまま DHCP クライアントに任せ、リンクが落ちない
                        // 限り再 join しなかった)。TBYB の締め切り判定と延長タスクはこのループの外で回り続ける。
                        defmt::warn!("DHCP timeout; leaving and rejoining");
                        counters.dhcp_timeouts = counters.dhcp_timeouts.saturating_add(1);
                        boot_trace::stage(Stage::DhcpTimeout);
                        boot_trace::info(counters.pack());
                        ui.model.status.clear();
                        let _ = write!(
                            ui.model.status,
                            "{}: DHCP timeout ({}), rejoining...",
                            ascii_label::<32>(creds.ssid.as_bytes()),
                            counters.dhcp_timeouts
                        );
                        ui.model.joined_ssid = None;
                        ui.present().await;
                        if with_timeout(DHCP_REJOIN_LEAVE_TIMEOUT, control.leave()).await.is_err() {
                            defmt::warn!("leave() timed out");
                        }
                        boot_trace::stage(Stage::DhcpRetry);
                        link = Link::Disconnected {
                            next_attempt: Instant::now() + DHCP_REJOIN_DELAY,
                            retry: JOIN_RETRY_MIN,
                            last_error: None,
                        };
                    } else {
                        if ui.model.ota.next_check.is_none() {
                            ui.model.ota.next_check = Some(Instant::now() + OTA_FIRST_CHECK_DELAY);
                        }
                        link = Link::Joined;
                    }
                }
                Err(error) => {
                    defmt::error!("join failed: status {}", error.status);
                    counters.join_failures = counters.join_failures.saturating_add(1);
                    counters.last_join_status = (error.status & 0xff) as u8;
                    boot_trace::stage(Stage::JoinFailed);
                    boot_trace::info(counters.pack());
                    link = Link::Disconnected {
                        next_attempt: Instant::now() + retry,
                        retry: (retry * 2).min(JOIN_RETRY_MAX),
                        last_error: Some(error.status),
                    };
                }
            }
        }
        let network_up = matches!(link, Link::Joined) && stack.is_config_up();
        if network_up && !network_was_up {
            network_was_up = true;
            boot_trace::stage(Stage::NetworkUp);
        }
        if network_up && ui.model.ota.next_check.is_none() {
            ui.model.ota.next_check = Some(Instant::now() + OTA_FIRST_CHECK_DELAY);
        }

        // --- TBYB: 自己診断 = LCD 走査中 (BUY_MIN_UPTIME) + Wi-Fi join + DHCP で IP 取得 → explicit_buy ---
        // 成立するまでは tbyb_watchdog_task がウォッチドッグを延長する。締め切りを過ぎたら延長も buy も
        // やめ、最長 16.7 s 後にウォッチドッグで旧版へ戻る。
        if ui.model.boot.buy == BuyState::Pending && Instant::now() >= ui.model.boot.selftest_deadline() {
            TBYB_FEEDING.store(false, Ordering::Relaxed);
            ui.model.boot.buy = BuyState::TimedOut;
            boot_trace::stage(Stage::SelftestTimedOut);
            defmt::warn!("TBYB self-test timed out; not buying, waiting for the watchdog to roll back");
        }
        if ui.model.boot.buy == BuyState::Pending
            && network_up
            && ui.model.boot.boot_at.elapsed() >= BUY_MIN_UPTIME
            && ui.display.is_running()
        {
            defmt::info!("self-test passed (Wi-Fi + DHCP up), explicit_buy ...");
            // bootrom の explicit_buy は最初に WATCHDOG.CTRL.ENABLE を落とす (成否によらず) ので、
            // 以後の再ロードは不要。先にフラグを落としてタスクを終わらせる。
            TBYB_FEEDING.store(false, Ordering::Relaxed);
            boot_trace::stage(Stage::BuyCalled);
            ui.model.boot.buy = match ab_boot::explicit_buy() {
                Ok(()) => {
                    defmt::info!("explicit_buy OK (watchdog enabled: {})", WATCHDOG.ctrl().read().enable());
                    boot_trace::stage(Stage::Bought);
                    BuyState::Bought
                }
                Err(rc) => {
                    defmt::error!("explicit_buy failed: {}", rc);
                    boot_trace::stage(Stage::BuyFailed);
                    boot_trace::info(rc as u32);
                    BuyState::Failed(rc)
                }
            };
            ui.present().await;
        }

        // --- OTA (buy 待ち / 巻き戻し待ちの間は行わない) ---
        if ota_possible
            && network_up
            && !matches!(ui.model.boot.buy, BuyState::Pending | BuyState::TimedOut)
            && let Some(due) = ui.model.ota.next_check
            && Instant::now() >= due
            && let Ok(slots) = ui.model.boot.slots
        {
            ui.model.ota.checks += 1;
            ui.model.ota.next_check = None;
            let result = run_ota_check(&net, bufs, &mut flash, sectors, slots, &mut ui).await;
            let now = Instant::now();
            match result {
                Ok(phase @ (OtaPhase::NoRelease | OtaPhase::UpToDate { .. })) => {
                    ui.model.ota.rejected = None;
                    ui.model.ota.backoff = OTA_BACKOFF_MIN;
                    ui.model.ota.phase = phase;
                    ui.model.ota.next_check = Some(now + OTA_CHECK_INTERVAL);
                }
                Ok(OtaPhase::Rejected { version, retry_at }) => {
                    // 同じ版の Rejected が続いているなら最初の retry_at を保つ (phase は Checking になっているので
                    // ota.rejected で判定する。v0.2.3 までは phase を見ていたため毎回 10 分後へ延び、再試行しなかった)
                    let retry_at = match ui.model.ota.rejected {
                        Some((v, first)) if v == version => first,
                        _ => {
                            defmt::warn!("{} rejected before; FLASH_UPDATE retry in {} s", version, REJECTED_RETRY_DELAY.as_secs());
                            ui.model.ota.rejected = Some((version, retry_at));
                            retry_at
                        }
                    };
                    ui.model.ota.backoff = OTA_BACKOFF_MIN;
                    ui.model.ota.phase = OtaPhase::Rejected { version, retry_at };
                    ui.model.ota.next_check = Some(now + OTA_CHECK_INTERVAL);
                }
                Ok(phase @ OtaPhase::Rebooting { .. }) => {
                    ui.model.ota.rejected = None;
                    ui.model.ota.phase = phase;
                }
                Ok(other) => {
                    ui.model.ota.rejected = None;
                    ui.model.ota.phase = other;
                    ui.model.ota.next_check = Some(now + OTA_CHECK_INTERVAL);
                }
                Err(error) => {
                    defmt::error!("OTA failed: {:?}", error);
                    let backoff = ui.model.ota.backoff;
                    ui.model.ota.phase = OtaPhase::Failed {
                        error,
                        retry_at: now + backoff,
                    };
                    ui.model.ota.next_check = Some(now + backoff);
                    ui.model.ota.backoff = (backoff * 2).min(OTA_BACKOFF_MAX);
                }
            }
            ui.present().await;
        }

        // --- 検証済みイメージへの FLASH_UPDATE 再起動 / 巻き戻されたイメージの再試行 ---
        match ui.model.ota.phase {
            OtaPhase::Rebooting { version, at } if Instant::now() >= at => {
                if let Ok(slots) = ui.model.boot.slots {
                    defmt::info!("reboot(FLASH_UPDATE) into P{} for {}", slots.target.index, version);
                    reboot_into_slot(&mut ui, &mut control, slots).await;
                }
            }
            OtaPhase::Rejected { version, retry_at } if Instant::now() >= retry_at => {
                if let Ok(slots) = ui.model.boot.slots {
                    defmt::warn!("retrying FLASH_UPDATE boot into P{} for {}", slots.target.index, version);
                    reboot_into_slot(&mut ui, &mut control, slots).await;
                }
            }
            _ => {}
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
        match (&credentials, &link) {
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
