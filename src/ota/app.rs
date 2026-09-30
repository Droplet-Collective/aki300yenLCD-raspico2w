//! OTA + TBYB の実行部 (`wifi_ota` と `ticker` が共用)
//!
//! v0.2.8 までは `src/bin/wifi_ota.rs` にあったものを、表示 (LCD の描画) だけを bin 側に残して
//! ここへ移した。振る舞いは変えていない (docs/wifi-ota.md §1, §5)。
//!
//! - **TBYB**: 0.4.2〜の buy 条件は `boot_policy::BuyGate` (Wi-Fi + DHCP、OTA の manifest 確認が TLS + HTTP を
//!   最後まで通った、bin ごとの機能の一巡、その後 25 s の健全な稼働)。[`BootStatus::buy_tick`] が判定して
//!   `explicit_buy` する。起動から [`TBYB_SELFTEST_DEADLINE_SECS`] (180 s) 経っても揃わなければ buy せず旧版へ
//!   戻る。buy 待ちの間の OTA 確認は manifest を読むだけで、ダウンロードは buy の後 ([`CheckMode`]。
//!   buy 待ちの間の書き込み先は、戻り先になる旧版の区画だから)。
//!   ウォッチドッグは `wifi_ota` なら [`tbyb_watchdog_task`] が bootrom の 16.7 s を 2 s ごとに延ばし、
//!   `ticker` は `supervisor` が生存確認つきで再ロードする (止まったタスクがあれば旧版へ戻る)。
//! - **接続管理**: [`LinkManager`] が join → DHCP (20 s) → 通らなければ離脱して再 join、を回す。
//!   進行は `boot_trace` に記録する (巻き戻ったときに旧版が読む)。
//! - **OTA**: [`run_ota_check`] が manifest → 新版なら他方区画へストリーミング書き込み → 検証、
//!   [`OtaState::apply`] が次回確認時刻とバックオフを決め、[`OtaState::write_line`] が LCD の
//!   `OTA: ...` 行の文字列を作る。
//!
//! 表示への通知は [`OtaUi`] (OTA の進行) と [`LinkUi`] (接続状況の文字列) の 2 つの trait で受ける。
//! `wifi_ota` はどちらも「モデルを更新して present」、`ticker` は「共有モデルを更新するだけ」で実装する。

use core::fmt::Write as _;
use core::sync::atomic::{AtomicBool, Ordering};

use embassy_net::Stack;
use embassy_net::dns::DnsSocket;
use embassy_net::tcp::client::{TcpClient, TcpClientState};
use embassy_rp::clocks::RoscRng;
use embassy_rp::pac::WATCHDOG;
use embassy_time::{Duration, Instant, Timer, with_timeout};
use heapless::String;
use reqwless::client::{HttpClient, TlsConfig, TlsVerify};

use super::http::{self, BodySink};
use super::manifest::{Manifest, Version};
use super::slot::{self, OtaFlash, SectorBuffers, SlotWriter, Slots, slot_label};
use super::{MANIFEST_NAME, OtaError, URL_MAX, set_latest_asset_url};
use crate::ab_boot::{self, BootInfo};
use crate::boot_policy::{BUY_SETTLE_MS, BuyGate, BuyInputs, BuyStep, CheckOutcome, Round};
use crate::boot_trace::{self, ResetReason, SelftestCounters, Stage, Trace};
use crate::wifi::{self, WifiCredentials, ascii_label};

// ============================================================
// 動作パラメータ
// ============================================================

/// manifest を確認する周期 (DHCP 完了後の初回は `OTA_FIRST_CHECK_DELAY` 後)
pub const OTA_CHECK_INTERVAL: Duration = Duration::from_secs(60);
pub const OTA_FIRST_CHECK_DELAY: Duration = Duration::from_secs(5);
/// 失敗時のバックオフ (倍々、上限 10 分)
pub const OTA_BACKOFF_MIN: Duration = Duration::from_secs(60);
pub const OTA_BACKOFF_MAX: Duration = Duration::from_secs(600);
/// manifest 取得 / bin ダウンロードの全体タイムアウト
pub const MANIFEST_TIMEOUT: Duration = Duration::from_secs(30);
pub const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);
/// TCP ソケットの無通信タイムアウト
pub const SOCKET_TIMEOUT: Duration = Duration::from_secs(20);
/// 対象区画に manifest と同一のイメージが既にある (= 前回 TBYB 起動で buy されず戻ってきた) とき、
/// もう一度 FLASH_UPDATE 起動を試すまでの待ち時間
pub const REJECTED_RETRY_DELAY: Duration = Duration::from_secs(600);
/// 検証完了から再起動までの表示時間
pub const REBOOT_AFTER: Duration = Duration::from_secs(2);
/// ダウンロード中の進捗通知間隔
pub const PROGRESS_REDRAW: Duration = Duration::from_millis(250);

/// TBYB 自己診断の締め切り (起動からの秒数、`boot_policy::BUY_DEADLINE_MS`)。この間はウォッチドッグを
/// 再ロードし続ける。過ぎたら再ロードをやめて buy もしない (ウォッチドッグで旧版へ戻る)。
/// 0.4.1 までは 120 s (join + DHCP だけ)。0.4.2〜は OTA 確認と機能の一巡、25 s の様子見が入るので 180 s。
pub const TBYB_SELFTEST_DEADLINE_SECS: u64 = (crate::boot_policy::BUY_DEADLINE_MS / 1000) as u64;
/// ウォッチドッグを再ロードする周期 (16.7 s に対して十分短く、フラッシュ操作や scan の待ちより長い)
pub const TBYB_WATCHDOG_FEED_INTERVAL: Duration = Duration::from_secs(2);
/// WATCHDOG.LOAD に書く値。24 bit × 1 µs = 16.7 s で、bootrom が TBYB 起動時に設定するのと同じ最大値。
/// LOAD は書き込み専用でカウンタを再ロードするだけ (CTRL の ENABLE / PAUSE_* や、reboot パラメータが
/// 入っている SCRATCH2〜7 には触れない)。
pub const WATCHDOG_LOAD_MAX: u32 = 0x00ff_ffff;

/// 接続パラメータ (wifi_status と同じ)
pub const DHCP_TIMEOUT: Duration = Duration::from_secs(20);
/// DHCP タイムアウト後の `leave()` に許す時間と、離脱してから再 join するまでの間
pub const DHCP_REJOIN_LEAVE_TIMEOUT: Duration = Duration::from_secs(2);
pub const DHCP_REJOIN_DELAY: Duration = Duration::from_millis(500);
pub const JOIN_RETRY_MIN: Duration = Duration::from_secs(5);
pub const JOIN_RETRY_MAX: Duration = Duration::from_secs(60);

/// TLS レコードバッファ。受信側は 16 kB のレコード + 128 B のオーバーヘッドが必要 (embedded-tls)。
/// 送信側はリクエスト行 (URL 最大 2 kB) + ヘッダ + オーバーヘッドが入ればよい (超えれば複数レコードに分割される)。
pub const TLS_RX_SIZE: usize = 16384 + 256;
pub const TLS_TX_SIZE: usize = 3072;
/// TCP ソケットバッファ (受信ウィンドウがダウンロード速度を決める。RAM 節約のため 4 kB)
pub const TCP_RX_SIZE: usize = 4096;
pub const TCP_TX_SIZE: usize = 2048;
/// HTTP 応答ヘッダ用。github.com の 302 は Content-Security-Policy (約 3.7 kB) と
/// Set-Cookie 3 本を含めて 5.0〜5.9 kB (2026-09 実測)。reqwless はヘッダ終端がこのバッファに
/// 収まらないと `BufferTooSmall` を返す (v0.2.0 の 4 kB では "bad HTTP response" になった)。
pub const HTTP_HEADER_SIZE: usize = 8192;
/// 本文の受信単位
pub const CHUNK_SIZE: usize = 2048;
// `http::fetch` はリダイレクト先 (Location) を一旦 `chunk` に写す
const _: () = assert!(CHUNK_SIZE >= URL_MAX);
/// manifest.json の上限
pub const MANIFEST_MAX: usize = 512;

// ============================================================
// static 配置のバッファ (bin 側で `static mut` に置く。大きな配列を future / スタックに置かない)
// ============================================================

pub struct NetBuffers {
    pub tls_rx: [u8; TLS_RX_SIZE],
    pub tls_tx: [u8; TLS_TX_SIZE],
    pub http_rx: [u8; HTTP_HEADER_SIZE],
    pub chunk: [u8; CHUNK_SIZE],
    pub manifest: [u8; MANIFEST_MAX],
    pub url: String<URL_MAX>,
}

impl NetBuffers {
    pub const fn new() -> Self {
        Self {
            tls_rx: [0; TLS_RX_SIZE],
            tls_tx: [0; TLS_TX_SIZE],
            http_rx: [0; HTTP_HEADER_SIZE],
            chunk: [0; CHUNK_SIZE],
            manifest: [0; MANIFEST_MAX],
            url: String::new(),
        }
    }
}

impl Default for NetBuffers {
    fn default() -> Self {
        Self::new()
    }
}

pub type TcpState = TcpClientState<1, TCP_TX_SIZE, TCP_RX_SIZE>;
pub type Tcp<'a> = TcpClient<'a, 1, TCP_TX_SIZE, TCP_RX_SIZE>;

/// HTTP クライアントの土台 (TCP 1 本 + DNS)
pub struct Net<'a> {
    pub tcp: Tcp<'a>,
    pub dns: DnsSocket<'a>,
}

impl<'a> Net<'a> {
    pub fn new(stack: Stack<'a>, tcp_state: &'a TcpState) -> Self {
        let mut tcp: Tcp<'a> = TcpClient::new(stack, tcp_state);
        tcp.set_timeout(Some(SOCKET_TIMEOUT));
        Self {
            tcp,
            dns: DnsSocket::new(stack),
        }
    }

    /// 接続ごとに乱数シードを変えた TLS 付き HTTP クライアント (reqwless は seed から ChaCha8 を毎回作り直す)。
    /// `http://` の URL にはそのまま平文で接続する。
    pub fn client<'c>(&'c self, tls_rx: &'c mut [u8], tls_tx: &'c mut [u8]) -> HttpClient<'c, Tcp<'a>, DnsSocket<'a>> {
        let seed = RoscRng.next_u64();
        let tls = TlsConfig::new(seed, tls_rx, tls_tx, TlsVerify::None);
        HttpClient::new_with_tls(&self.tcp, &self.dns, tls)
    }
}

// ============================================================
// TBYB: bootrom のウォッチドッグの延長
// ============================================================

/// true の間 `tbyb_watchdog_task` がウォッチドッグを再ロードする。main が buy 待ちの開始時に立て、
/// explicit_buy の後 (成否によらず) に落とす。締め切りを過ぎたらタスク自身が落とす。
pub static TBYB_FEEDING: AtomicBool = AtomicBool::new(false);

/// ウォッチドッグのカウンタを最大値 (16.7 s) に再ロードする。embassy の `Watchdog` は使わない
/// (`Watchdog::start` は CTRL / PAUSE / SCRATCH を書き換え、bootrom が TBYB 用に設定した状態を壊す)。
pub fn feed_watchdog() {
    WATCHDOG.load().write(|w| w.set_load(WATCHDOG_LOAD_MAX));
}

/// buy 待ちの間、`TBYB_WATCHDOG_FEED_INTERVAL` ごとに bootrom のウォッチドッグを再ロードする。
/// main ループは join / DHCP / scan で数秒〜20 s 待つので、独立したタスクで回す。
/// `deadline` (起動 + `TBYB_SELFTEST_DEADLINE_SECS`) を過ぎたら再ロードをやめて終わる。以後は
/// 最長 16.7 s でウォッチドッグが発火し、旧版で通常起動する (データシート §5.1.17)。
#[embassy_executor::task]
pub async fn tbyb_watchdog_task(deadline: Instant) {
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

/// ウォッチドッグの残り時間 (0.1 秒単位)。無効なら None。
pub fn watchdog_remaining_tenths() -> Option<u32> {
    let ctrl = WATCHDOG.ctrl().read();
    if ctrl.enable() { Some(ctrl.time() / 100_000) } else { None }
}

// ============================================================
// 起動状態 / TBYB
// ============================================================

/// TBYB の進行状態 (ota_selftest と同じ)
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BuyState {
    NotTbyb,
    /// TBYB 起動。自己診断 (Wi-Fi + DHCP) が通ったら buy する。この間はウォッチドッグを延長している
    Pending,
    /// 締め切り (`TBYB_SELFTEST_DEADLINE_SECS`) までに自己診断が通らなかった。延長をやめ、buy もしない。
    /// 最長 16.7 s 後にウォッチドッグで旧版へ戻る
    TimedOut,
    Bought,
    Failed(i32),
}

pub struct BootStatus {
    pub boot: Option<BootInfo>,
    pub slots: Result<Slots, OtaError>,
    pub buy: BuyState,
    pub boot_at: Instant,
    /// 直前のリセットがウォッチドッグ由来か (FLASH_UPDATE 再起動も TBYB の巻き戻りもこれ)
    pub reset_reason: ResetReason,
    /// 前回の TBYB 起動が SCRATCH5〜7 に残した記録 (巻き戻り後の旧版で見える)
    pub prev_trace: Option<Trace>,
    /// SCRATCH0 の生の値 (`boot_trace::arm` の前に読む。他方区画へ戻した印 `boot_policy::fallback_marker`)
    pub scratch0: u32,
    /// buy 条件の判定 (0.4.2〜)
    pub gate: BuyGate,
    /// 直近の判定結果 (LCD の `wait:ota` / `settle 12s`)
    pub last_step: BuyStep,
}

impl BootStatus {
    /// `boot_trace::arm()` より前に呼ぶ (前回の記録を読んでから上書きする)
    pub fn collect() -> Self {
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
            scratch0: boot_trace::read_scratch0(),
            gate: BuyGate::new(crate::boot_policy::BUY_DEADLINE_MS, BUY_SETTLE_MS),
            last_step: BuyStep::Waiting { missing: "wifi" },
        }
    }

    /// `reboot(FLASH_UPDATE)` で起動した (OTA で書いた版、または他方区画へ戻した版)。buy 済みかどうかは問わない
    pub fn is_flash_update_boot(&self) -> bool {
        self.is_ota_boot()
    }

    /// 起動診断行 (起動種別 / 診断ワード / リセット理由) を出すか。電源投入直後の通常起動では出さない
    pub fn show_boot_line(&self) -> bool {
        self.reset_reason != ResetReason::Hardware || self.prev_trace.is_some()
    }

    /// TBYB 自己診断の締め切り時刻
    pub fn selftest_deadline(&self) -> Instant {
        self.boot_at + Duration::from_secs(TBYB_SELFTEST_DEADLINE_SECS)
    }

    /// 今の起動が `reboot(FLASH_UPDATE)` 由来か (= OTA で書いたイメージが動いている)
    pub fn is_ota_boot(&self) -> bool {
        self.boot
            .as_ref()
            .is_some_and(|b| b.boot_type & !ab_boot::BOOT_TYPE_CHAINED_FLAG == ab_boot::BOOT_TYPE_FLASH_UPDATE)
    }

    /// TBYB 起動なら bootrom のウォッチドッグ (16.7 s) が既に走っている。SD / LCD / Wi-Fi の初期化が
    /// 先に来るので、まず一度再ロードし、以後は `tbyb_watchdog_task` に任せる。進行は boot_trace に
    /// 記録する (巻き戻ったときに旧版が読む。prev_trace を読んだ後に arm する)。
    /// main の最初 (ペリフェラル初期化の直後) に呼ぶ。TBYB 起動でなければ何もしない。
    pub fn start_tbyb_feeding(&self, spawner: &embassy_executor::Spawner) {
        if self.buy != BuyState::Pending {
            return;
        }
        feed_watchdog();
        boot_trace::arm();
        boot_trace::stage(Stage::MainEntered);
        TBYB_FEEDING.store(true, Ordering::Relaxed);
        spawner.spawn(tbyb_watchdog_task(self.selftest_deadline())).unwrap();
        boot_trace::stage(Stage::FeedStarted);
        defmt::info!(
            "TBYB buy pending: extending the watchdog every {} s until self-test passes (deadline {} s)",
            TBYB_WATCHDOG_FEED_INTERVAL.as_secs(),
            TBYB_SELFTEST_DEADLINE_SECS
        );
    }

    /// TBYB の buy 条件 (`boot_policy::BuyGate`、0.4.2〜) を判定し、揃えば `explicit_buy` する。
    /// `ota_proved` = OTA の manifest 確認が一度でも TLS + HTTP を最後まで通った ([`check_outcome`])、
    /// `round` = bin ごとの機能の一巡 (`wifi_ota` は `Round::DONE`)、`healthy` = 生存確認が揃っている
    /// (`wifi_ota` は LCD 走査中)。締め切りを過ぎたら buy せず、ウォッチドッグで旧版へ戻るのを待つ。
    /// メインループで毎回呼ぶ。状態が変わったら true (表示を更新する)。
    pub fn buy_tick(&mut self, network_up: bool, ota_proved: bool, round: Round, healthy: bool) -> bool {
        if self.buy != BuyState::Pending {
            return false;
        }
        let step = self.gate.tick(&BuyInputs {
            now_ms: self.boot_at.elapsed().as_millis() as u32,
            network_up,
            ota_proved,
            round,
            healthy,
        });
        self.last_step = step;
        if step == BuyStep::TimedOut {
            TBYB_FEEDING.store(false, Ordering::Relaxed);
            self.buy = BuyState::TimedOut;
            boot_trace::stage(Stage::SelftestTimedOut);
            defmt::warn!("TBYB self-test timed out; not buying, waiting for the watchdog to roll back");
            return true;
        }
        if step == BuyStep::Buy {
            defmt::info!("self-test passed (Wi-Fi + OTA check + first round + settle), explicit_buy ...");
            // bootrom の explicit_buy は最初に WATCHDOG.CTRL.ENABLE を落とす (成否によらず) ので、
            // 以後の再ロードは不要。先にフラグを落としてタスクを終わらせる。
            TBYB_FEEDING.store(false, Ordering::Relaxed);
            boot_trace::stage(Stage::BuyCalled);
            self.buy = match ab_boot::explicit_buy() {
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
            return true;
        }
        false
    }

    /// OTA のダウンロード / 書き込みを行ってよいか (buy 待ち / 巻き戻し待ちの間は行わない。
    /// 書き込み先の他方区画は、buy されなかったときの戻り先だから)
    pub fn ota_allowed(&self) -> bool {
        !matches!(self.buy, BuyState::Pending | BuyState::TimedOut)
    }

    /// OTA の manifest 確認を行ってよいか (0.4.2〜: buy 待ちの間も行う。buy 条件の 1 つ)
    pub fn ota_check_allowed(&self) -> bool {
        self.buy != BuyState::TimedOut
    }

    /// LCD 用: ` slot B TBYB:pending 37/120s WDT 15.1s` の形の文字列と、その色調。
    /// (`plain_mark` = TBYB フラグ無しのビルドなら ` plain` を挟む)
    pub fn write_tbyb_line<const N: usize>(&self, line: &mut String<N>, plain_mark: bool) -> Tone {
        match &self.slots {
            Ok(slots) => {
                let _ = write!(line, " slot {}", slot_label(&slots.own));
            }
            Err(e) => {
                let _ = write!(line, " slot ?({})", e.label());
            }
        }
        if plain_mark {
            let _ = line.push_str(" plain");
        }
        let tone = match self.buy {
            BuyState::NotTbyb => {
                let _ = line.push_str(" TBYB:no");
                Tone::Muted
            }
            BuyState::Pending => {
                // ウォッチドッグを再ロードしながら自己診断中。締め切りまでの経過秒と、待っている条件を出す
                let _ = write!(
                    line,
                    " TBYB:pending {}/{}s",
                    self.boot_at.elapsed().as_secs().min(TBYB_SELFTEST_DEADLINE_SECS),
                    TBYB_SELFTEST_DEADLINE_SECS
                );
                match self.last_step {
                    BuyStep::Waiting { missing } => {
                        let _ = write!(line, " wait:{}", missing);
                    }
                    BuyStep::Settling { left_ms } => {
                        let _ = write!(line, " settle {}s", left_ms.div_ceil(1000));
                    }
                    _ => {}
                }
                Tone::Busy
            }
            BuyState::TimedOut => {
                let _ = line.push_str(" TBYB:timeout->rollback");
                Tone::Error
            }
            BuyState::Bought => {
                let _ = line.push_str(" TBYB:bought OK");
                Tone::Ok
            }
            BuyState::Failed(rc) => {
                let _ = write!(line, " TBYB:buy FAILED rc={}", rc);
                Tone::Error
            }
        };
        // 巻き戻るまでの残り (0.4.2〜: buy 待ちの間は 0.5 s ごとに再ロードされるので、締め切り後だけ出す)
        if self.buy == BuyState::TimedOut
            && let Some(t) = watchdog_remaining_tenths()
        {
            let _ = write!(line, " WDT {}.{}s", t / 10, t % 10);
        }
        tone
    }

    /// LCD 用: 前回の TBYB 起動の記録 (`TBYB 0.2.3: dhcp-wait @121.3s join3 fail2 dhcpto1`)。記録が無ければ None
    pub fn write_prev_trace_line<const N: usize>(&self, line: &mut String<N>) -> Option<Tone> {
        let trace = self.prev_trace.as_ref()?;
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
        let tone = match trace.stage {
            Some(Stage::Panic) => {
                let _ = write!(line, " line={}", trace.info);
                Tone::Error
            }
            Some(Stage::HardFault) => {
                let _ = write!(line, " pc={:#010x}", trace.info);
                Tone::Error
            }
            Some(Stage::BuyFailed) => {
                let _ = write!(line, " rc={}", trace.info as i32);
                Tone::Error
            }
            stage => {
                let c = trace.counters();
                let _ = write!(line, " join{} fail{} dhcpto{}", c.join_attempts, c.join_failures, c.dhcp_timeouts);
                if c.last_join_status != 0 {
                    let _ = write!(line, " st{}", c.last_join_status);
                }
                if stage == Some(Stage::SelftestTimedOut) { Tone::Error } else { Tone::Busy }
            }
        };
        Some(tone)
    }

    /// LCD 用: 起動種別 / 診断ワード / リセット理由 (`NORMAL P0 A:4C4D launched B:000D imgdef reset:wdt`)
    pub fn write_boot_line<const N: usize>(&self, line: &mut String<N>) {
        match &self.boot {
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
        let _ = write!(line, " reset:{}", self.reset_reason.label());
    }
}

/// 文字列の色調 (bin 側が実際の色に写す)
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tone {
    /// 灰色 (待機、無効)
    Muted,
    /// 白 (進行中の通常表示)
    Normal,
    /// 緑 (成功、最新)
    Ok,
    /// 黄 (ダウンロード中、buy 待ち)
    Busy,
    /// 赤 (失敗、巻き戻り)
    Error,
}

// ============================================================
// 接続管理 (join → DHCP → 通らなければ離脱して再 join)
// ============================================================

/// 接続状況の表示先。`text` はステータス行 (66 桁以内)、`joined_ssid` は接続中 (DHCP 待ち含む) の SSID
#[allow(async_fn_in_trait)] // 本クレート内の bin でしか実装しない
pub trait LinkUi {
    async fn link_status(&mut self, text: &str, joined_ssid: Option<&String<32>>);
}

pub enum Link {
    ScanOnly(&'static str),
    Disconnected {
        next_attempt: Instant,
        retry: Duration,
        last_error: Option<u32>,
    },
    Joined,
}

/// join / DHCP の状態機械。`step` をメインループで毎回呼ぶ
pub struct LinkManager {
    pub link: Link,
    /// TBYB 自己診断の進み具合 (boot_trace の SCRATCH7 に書く。巻き戻ったとき旧版に見える)
    pub counters: SelftestCounters,
    network_was_up: bool,
}

impl LinkManager {
    pub fn new(credentials: &Result<WifiCredentials, &'static str>) -> Self {
        let link = match credentials {
            Ok(_) => Link::Disconnected {
                next_attempt: Instant::now(),
                retry: JOIN_RETRY_MIN,
                last_error: None,
            },
            Err(message) => Link::ScanOnly(message),
        };
        Self {
            link,
            counters: SelftestCounters::default(),
            network_was_up: false,
        }
    }

    /// リンク断の検出 → 再 join、再試行時刻が来ていれば join → DHCP 待ち (20 s) → 通らなければ離脱。
    /// 戻り値は「IP を持って接続中か」。
    pub async fn step(
        &mut self,
        control: &mut cyw43::Control<'static>,
        stack: Stack<'static>,
        credentials: &Result<WifiCredentials, &'static str>,
        ui: &mut impl LinkUi,
    ) -> bool {
        let now = Instant::now();
        if let (Ok(creds), Link::Joined) = (credentials, &self.link)
            && !stack.is_link_up()
        {
            defmt::warn!("link down, will rejoin {}", creds.ssid.as_str());
            self.link = Link::Disconnected {
                next_attempt: now,
                retry: JOIN_RETRY_MIN,
                last_error: None,
            };
        }
        if let (Ok(creds), Link::Disconnected { next_attempt, retry, .. }) = (credentials, &self.link)
            && now >= *next_attempt
        {
            let retry = *retry;
            let mut status: String<80> = String::new();
            let _ = write!(status, "connecting to {}...", ascii_label::<32>(creds.ssid.as_bytes()));
            ui.link_status(&status, None).await;
            self.counters.join_attempts = self.counters.join_attempts.saturating_add(1);
            boot_trace::stage(Stage::Joining);
            boot_trace::info(self.counters.pack());
            match control.join(creds.ssid.as_str(), creds.join_options()).await {
                Ok(()) => {
                    defmt::info!("joined {}", creds.ssid.as_str());
                    boot_trace::stage(Stage::Joined);
                    status.clear();
                    let _ = write!(status, "{}: waiting for DHCP...", ascii_label::<32>(creds.ssid.as_bytes()));
                    ui.link_status(&status, Some(&creds.ssid)).await;
                    boot_trace::stage(Stage::DhcpWait);
                    if with_timeout(DHCP_TIMEOUT, stack.wait_config_up()).await.is_err() {
                        // DHCP が通らない。association はあるのにデータが流れない状態 (v0.2.5 の TBYB 起動で
                        // 観測: join1 dhcpto1 のまま 120 s) から抜けるため、AP から離脱して次のループで
                        // 再 join する (v0.2.5 までは Joined のまま DHCP クライアントに任せ、リンクが落ちない
                        // 限り再 join しなかった)。TBYB の締め切り判定と延長タスクはこのループの外で回り続ける。
                        defmt::warn!("DHCP timeout; leaving and rejoining");
                        self.counters.dhcp_timeouts = self.counters.dhcp_timeouts.saturating_add(1);
                        boot_trace::stage(Stage::DhcpTimeout);
                        boot_trace::info(self.counters.pack());
                        status.clear();
                        let _ = write!(
                            status,
                            "{}: DHCP timeout ({}), rejoining...",
                            ascii_label::<32>(creds.ssid.as_bytes()),
                            self.counters.dhcp_timeouts
                        );
                        ui.link_status(&status, None).await;
                        if with_timeout(DHCP_REJOIN_LEAVE_TIMEOUT, control.leave()).await.is_err() {
                            defmt::warn!("leave() timed out");
                        }
                        boot_trace::stage(Stage::DhcpRetry);
                        self.link = Link::Disconnected {
                            next_attempt: Instant::now() + DHCP_REJOIN_DELAY,
                            retry: JOIN_RETRY_MIN,
                            last_error: None,
                        };
                    } else {
                        self.link = Link::Joined;
                    }
                }
                Err(error) => {
                    defmt::error!("join failed: status {}", error.status);
                    self.counters.join_failures = self.counters.join_failures.saturating_add(1);
                    self.counters.last_join_status = (error.status & 0xff) as u8;
                    boot_trace::stage(Stage::JoinFailed);
                    boot_trace::info(self.counters.pack());
                    self.link = Link::Disconnected {
                        next_attempt: Instant::now() + retry,
                        retry: (retry * 2).min(JOIN_RETRY_MAX),
                        last_error: Some(error.status),
                    };
                }
            }
        }
        let network_up = matches!(self.link, Link::Joined) && stack.is_config_up();
        if network_up && !self.network_was_up {
            self.network_was_up = true;
            boot_trace::stage(Stage::NetworkUp);
        }
        network_up
    }

    pub fn is_joined(&self) -> bool {
        matches!(self.link, Link::Joined)
    }
}

// ============================================================
// OTA の状態
// ============================================================

/// OTA の進行状態 (LCD の OTA 行に出す)
#[derive(Clone, Copy, Debug)]
pub enum OtaPhase {
    /// まだ確認していない
    Idle,
    /// wifi.txt が無い等で確認できない
    Disabled(&'static str),
    Checking,
    /// manifest が 404 (Release 無し)
    NoRelease,
    UpToDate { latest: Version },
    /// 新しい版がある (buy 待ちなので、ダウンロードは buy の後。0.4.2〜)
    NewerAvailable { latest: Version },
    /// 新しい版があるが、他方区画へ戻す原因になった版 (以下) なので入れない (0.4.2〜、`persist`)
    Blocked { latest: Version },
    Downloading { version: Version, received: u32, total: u32 },
    Verifying { version: Version },
    /// 検証済み。`REBOOT_AFTER` 後に FLASH_UPDATE 再起動
    Rebooting { version: Version, at: Instant },
    /// 対象区画に同じイメージが既にある (前回 buy されなかった)。`retry_at` に再起動を試す
    Rejected { version: Version, retry_at: Instant },
    Failed { error: OtaError, retry_at: Instant },
}

pub struct OtaState {
    pub phase: OtaPhase,
    pub next_check: Option<Instant>,
    pub backoff: Duration,
    pub checks: u32,
    /// 最初に `Rejected` と判定した版とその再試行時刻。`run_ota_check` は開始時に phase を `Checking` に
    /// するので、phase からは「前回も同じ版で Rejected だった」ことが分からない。ここに保ち、同じ版なら
    /// 再試行時刻を動かさない。別の結果 (最新 / 新版あり / Release 無し) が出たら消す。
    pub rejected: Option<(Version, Instant)>,
}

impl OtaState {
    pub const fn new() -> Self {
        Self {
            phase: OtaPhase::Idle,
            next_check: None,
            backoff: OTA_BACKOFF_MIN,
            checks: 0,
            rejected: None,
        }
    }

    /// DHCP が通ったら初回確認の時刻を決める (未定のときだけ)
    pub fn schedule_first_check(&mut self) {
        self.schedule_first_check_in(OTA_FIRST_CHECK_DELAY);
    }

    /// [`schedule_first_check`](Self::schedule_first_check) の待ち時間を指定する版。`ticker` (0.4.1〜) は
    /// 0 s で呼び、接続後の最初の仕事を OTA 確認にする (新しい版で直せるように、他の取得より先に)。
    /// 一度でも確認を始めたら (`checks > 0`) 何もしない。
    pub fn schedule_first_check_in(&mut self, delay: Duration) {
        if self.next_check.is_none() && self.checks == 0 && matches!(self.phase, OtaPhase::Idle) {
            self.next_check = Some(Instant::now() + delay);
        }
    }

    /// 確認時刻が来たか
    pub fn is_due(&self) -> bool {
        self.next_check.is_some_and(|due| Instant::now() >= due)
    }

    /// 確認を始める (回数を数え、次回時刻を消す)
    pub fn begin_check(&mut self) {
        self.checks += 1;
        self.next_check = None;
    }

    /// `run_ota_check` の結果から phase / 次回確認 / バックオフを更新する
    pub fn apply(&mut self, result: Result<OtaPhase, OtaError>) {
        let now = Instant::now();
        match result {
            Ok(phase @ (OtaPhase::NoRelease | OtaPhase::UpToDate { .. } | OtaPhase::Blocked { .. })) => {
                self.rejected = None;
                self.backoff = OTA_BACKOFF_MIN;
                self.phase = phase;
                self.next_check = Some(now + OTA_CHECK_INTERVAL);
            }
            Ok(OtaPhase::Rejected { version, retry_at }) => {
                // 同じ版の Rejected が続いているなら最初の retry_at を保つ (phase は Checking になっているので
                // ota.rejected で判定する。v0.2.3 までは phase を見ていたため毎回 10 分後へ延び、再試行しなかった)
                let retry_at = match self.rejected {
                    Some((v, first)) if v == version => first,
                    _ => {
                        defmt::warn!(
                            "{} rejected before; FLASH_UPDATE retry in {} s",
                            version,
                            REJECTED_RETRY_DELAY.as_secs()
                        );
                        self.rejected = Some((version, retry_at));
                        retry_at
                    }
                };
                self.backoff = OTA_BACKOFF_MIN;
                self.phase = OtaPhase::Rejected { version, retry_at };
                self.next_check = Some(now + OTA_CHECK_INTERVAL);
            }
            Ok(phase @ OtaPhase::Rebooting { .. }) => {
                self.rejected = None;
                self.phase = phase;
            }
            Ok(other) => {
                self.rejected = None;
                self.phase = other;
                self.next_check = Some(now + OTA_CHECK_INTERVAL);
            }
            Err(error) => {
                defmt::error!("OTA failed: {:?}", error);
                let backoff = self.backoff;
                self.phase = OtaPhase::Failed {
                    error,
                    retry_at: now + backoff,
                };
                self.next_check = Some(now + backoff);
                self.backoff = (backoff * 2).min(OTA_BACKOFF_MAX);
            }
        }
    }

    /// 検証済みイメージへの FLASH_UPDATE 再起動 / 巻き戻されたイメージの再試行の時刻が来ていれば、その版
    pub fn reboot_due(&self) -> Option<Version> {
        match self.phase {
            OtaPhase::Rebooting { version, at } if Instant::now() >= at => Some(version),
            OtaPhase::Rejected { version, retry_at } if Instant::now() >= retry_at => Some(version),
            _ => None,
        }
    }

    /// LCD 用: `OTA: ...` 行。戻り値は色調と進捗 (ダウンロード中 = (受信, 全体)、検証中 / 再起動待ち = (1, 1))
    pub fn write_line<const N: usize>(&self, line: &mut String<N>, slots: &Result<Slots, OtaError>) -> (Tone, Option<(u32, u32)>) {
        let mut progress: Option<(u32, u32)> = None;
        let tone = match self.phase {
            OtaPhase::Idle => {
                let _ = line.push_str("OTA: waiting for network");
                if let Some(next) = self.next_check {
                    let _ = write!(line, ", first check in {}s", secs_until(next));
                }
                Tone::Muted
            }
            OtaPhase::Disabled(reason) => {
                let _ = write!(line, "OTA: disabled ({})", reason);
                Tone::Muted
            }
            OtaPhase::Checking => {
                let _ = write!(line, "OTA: checking {} (#{})...", MANIFEST_NAME, self.checks);
                Tone::Normal
            }
            OtaPhase::NoRelease => {
                let _ = line.push_str("OTA: no release yet (404)");
                if let Some(next) = self.next_check {
                    let _ = write!(line, ", next check in {}s", secs_until(next));
                }
                Tone::Muted
            }
            OtaPhase::UpToDate { latest } => {
                let _ = write!(line, "OTA: up to date (latest {})", latest);
                if let Some(next) = self.next_check {
                    let _ = write!(line, ", next check in {}s", secs_until(next));
                }
                Tone::Ok
            }
            OtaPhase::NewerAvailable { latest } => {
                let _ = write!(line, "OTA: {} available, download after TBYB buy", latest);
                Tone::Busy
            }
            OtaPhase::Blocked { latest } => {
                let _ = write!(line, "OTA: latest {} blocked (fell back from it), waiting", latest);
                if let Some(next) = self.next_check {
                    let _ = write!(line, " {}s", secs_until(next));
                }
                Tone::Error
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
                Tone::Busy
            }
            OtaPhase::Verifying { version } => {
                let _ = write!(line, "OTA: {} downloaded, verifying (sha256 + readback)...", version);
                progress = Some((1, 1));
                Tone::Busy
            }
            OtaPhase::Rebooting { version, at } => {
                let slot = slots.as_ref().map(|s| slot_label(&s.target)).unwrap_or("?");
                let _ = write!(
                    line,
                    "OTA: {} verified -> reboot into slot {} in {}s (TBYB)",
                    version,
                    slot,
                    secs_until(at)
                );
                progress = Some((1, 1));
                Tone::Ok
            }
            OtaPhase::Rejected { version, retry_at } => {
                let slot = slots.as_ref().map(|s| slot_label(&s.target)).unwrap_or("?");
                let _ = write!(
                    line,
                    "OTA: {} in slot {} was rolled back; retry boot in {}s",
                    version,
                    slot,
                    secs_until(retry_at)
                );
                Tone::Error
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
                Tone::Error
            }
        };
        (tone, progress)
    }
}

impl Default for OtaState {
    fn default() -> Self {
        Self::new()
    }
}

pub fn secs_until(at: Instant) -> u64 {
    at.saturating_duration_since(Instant::now()).as_secs()
}

// ============================================================
// OTA 本体
// ============================================================

/// OTA の進行の表示先 (ダウンロード進捗など、`run_ota_check` の途中経過)
#[allow(async_fn_in_trait)] // 本クレート内の bin でしか実装しない
pub trait OtaUi {
    async fn ota_phase(&mut self, phase: OtaPhase);
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

/// bin を他方区画へ書きながら進捗を通知する
struct DownloadSink<'a, 'f, U: OtaUi> {
    writer: &'a mut SlotWriter<'f>,
    ui: &'a mut U,
    version: Version,
    last_draw: Instant,
}

impl<U: OtaUi> BodySink for DownloadSink<'_, '_, U> {
    async fn push(&mut self, data: &[u8]) -> Result<(), OtaError> {
        // セクタがたまるごとに消去 (45〜400 ms) + 書き込み。割り込み禁止中も LCD の DMA リングは
        // SRAM だけを読むので走査は乱れない (docs/ota-design.md §4.1)。
        self.writer.push(data)?;
        if self.last_draw.elapsed() >= PROGRESS_REDRAW {
            self.ui
                .ota_phase(OtaPhase::Downloading {
                    version: self.version,
                    received: self.writer.received(),
                    total: self.writer.expected(),
                })
                .await;
            self.last_draw = Instant::now();
        }
        Ok(())
    }
}

/// 1 回の確認で何をするか (0.4.2〜)
#[derive(Clone, Copy, Debug, Default)]
pub struct CheckMode {
    /// 新しい版があってもダウンロードしない (TBYB の buy 待ち。`NewerAvailable` を返す)
    pub check_only: bool,
    /// この版数語以下は入れない (`boot_policy::Record::blocked`、0 = 無し)
    pub blocked: u32,
}

/// 確認の結果の分類 (`boot_policy::CheckOutcome`)。HTTP の応答を受けた (manifest を解釈した / 確定した
/// ステータス / 本文の検証まで進んだ) なら `Proved` = この版の TLS + HTTP の経路が最後まで動いた。
/// DNS / TCP / TLS / 時間切れ / 区画の問題は `Transport` (経路を試し切れていない)。
pub fn check_outcome(result: &Result<OtaPhase, OtaError>) -> CheckOutcome {
    match result {
        Ok(_) => CheckOutcome::Proved,
        Err(
            OtaError::HttpStatus(_)
            | OtaError::HttpHeaderTooLong
            | OtaError::HttpCodec
            | OtaError::HttpRedirect
            | OtaError::TooManyRedirects
            | OtaError::LocationTooLong
            | OtaError::Manifest
            | OtaError::BadSize
            | OtaError::SizeMismatch
            | OtaError::ShaMismatch
            | OtaError::ReadbackMismatch
            | OtaError::Flash,
        ) => CheckOutcome::Proved,
        Err(
            OtaError::Dns
            | OtaError::Network
            | OtaError::Tls
            | OtaError::HttpProtocol
            | OtaError::Timeout
            | OtaError::NoPartitionTable
            | OtaError::NoTarget,
        ) => CheckOutcome::Transport,
    }
}

/// 1 回の更新確認。戻り値の `OtaPhase` は NoRelease / UpToDate / NewerAvailable / Blocked / Rejected /
/// Rebooting のいずれか。
pub async fn run_ota_check(
    net: &Net<'_>,
    bufs: &mut NetBuffers,
    flash: &mut OtaFlash,
    sectors: &mut SectorBuffers,
    slots: Slots,
    mode: CheckMode,
    ui: &mut impl OtaUi,
) -> Result<OtaPhase, OtaError> {
    ui.ota_phase(OtaPhase::Checking).await;

    let mut client = net.client(&mut bufs.tls_rx, &mut bufs.tls_tx);

    // --- [1] manifest ---
    set_latest_asset_url(&mut bufs.url, MANIFEST_NAME);
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
    let latest_word = crate::boot_policy::version_word(manifest.version.major, manifest.version.minor, manifest.version.patch);
    if mode.blocked != 0 && latest_word <= mode.blocked {
        defmt::warn!("manifest {} is blocked (fell back from it); not installing", manifest.version);
        return Ok(OtaPhase::Blocked {
            latest: manifest.version,
        });
    }
    if mode.check_only {
        return Ok(OtaPhase::NewerAvailable {
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
    ui.ota_phase(OtaPhase::Downloading {
        version: manifest.version,
        received: 0,
        total: manifest.size,
    })
    .await;
    set_latest_asset_url(&mut bufs.url, &manifest.bin);
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
    ui.ota_phase(OtaPhase::Verifying {
        version: manifest.version,
    })
    .await;
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

/// 対象区画への FLASH_UPDATE 再起動。先に AP から離脱して CYW43 の電源 (WL_REG_ON) を落とし、
/// 次の版が接続中のチップを引き継がないようにする (`wifi::power_off_for_reboot`)。戻らない。
/// 呼び出し側は先に「rebooting into slot ...」を表示しておく。
pub async fn reboot_into_slot(control: &mut cyw43::Control<'static>, slots: Slots) -> ! {
    wifi::power_off_for_reboot(control).await;
    ab_boot::reboot_flash_update(slots.target.start_offset(), 100)
}
