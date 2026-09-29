//! TBYB 起動の進行記録 (WATCHDOG.SCRATCH5〜7) と、巻き戻り後にそれを読む側
//!
//! TBYB イメージがウォッチドッグで旧版へ巻き戻されると、新版の画面は消えてしまい「どこまで
//! 進んだか」が分からない。そこで buy 待ちの新版は進行段階 (`Stage`) と稼働時間、自己診断の
//! カウンタを WATCHDOG の SCRATCH レジスタに書き続け、巻き戻り後に起動した旧版がそれを LCD に出す。
//!
//! # SCRATCH5〜7 を使える根拠 (pico-bootrom-rp2350)
//!
//! - `s_varm_hx_reboot` (varm_apis.c) は FLASH_UPDATE など NORMAL 以外の再起動で SCRATCH2..7 を
//!   全て書く (2=p0, 3=p1, 4=magic, 5=pc^-magic (= 0xFFFFFFFE), 6=p1, 7=pc)。TBYB 起動時に bootrom が
//!   仕掛けるウォッチドッグは REBOOT_TYPE_NORMAL なので SCRATCH2..4 だけを書き、5..7 は触らない。
//! - 起動時の `try_vector` (varm_boot_path.c) は SCRATCH4 の magic が合った時に SCRATCH4 を 0 に
//!   するだけで、5..7 には書かない。ウォッチドッグによる巻き戻り (通常起動) でも同じ。
//! - SCRATCH はチップレベルリセットと RUN ピン / DVDD 断で消える (データシート §12.9.5) が、bootrom の
//!   ウォッチドッグは PSM リセット (`PSM_WDSEL`) なので残る。`WATCHDOG.REASON` も同様に残る。
//!
//! したがって TBYB イメージが起動した後の SCRATCH5..7 は、次に FLASH_UPDATE 再起動が行われるまで
//! ファームウェアが自由に使える。旧版は起動時に一度読んで RAM に取り、以後は表示するだけ。
//!
//! # レイアウト
//!
//! | レジスタ | 内容 |
//! |---|---|
//! | SCRATCH5 | `MAGIC` (上位 12 bit) \| IMAGE_DEF major (4 bit) \| IMAGE_DEF minor (16 bit)。書いた版の識別 |
//! | SCRATCH6 | 稼働時間 (100 ms 単位、上位 24 bit) \| `Stage` (下位 8 bit) |
//! | SCRATCH7 | 付加情報。PANIC なら行番号、HARDFAULT なら PC、それ以外は [`SelftestCounters`] |
//!
//! bootrom が FLASH_UPDATE 再起動で書く SCRATCH5 (0xFFFFFFFE) やリセット値 0 は `MAGIC` と一致しない
//! ので、新版起動直後や電源投入直後に「前回の記録」を誤検出することはない。

use core::sync::atomic::{AtomicBool, Ordering};

use embassy_rp::pac::WATCHDOG;
use embassy_time::Instant;

use crate::image_def::{IMAGE_DEF_MAJOR, IMAGE_DEF_MINOR};

/// SCRATCH5 の上位 12 bit。bootrom の値 (0xFFFFFFFE / 0) と重ならない任意の値
const MAGIC: u32 = 0x7B10_0000;
const MAGIC_MASK: u32 = 0xFFF0_0000;

/// 進行段階。値は SCRATCH6 の下位 8 bit に入る (0xE0 以上は異常終了)
#[derive(Clone, Copy, PartialEq, Eq, Debug, defmt::Format)]
#[repr(u8)]
pub enum Stage {
    None = 0,
    /// main に入り、BOOT_INFO を読んだ
    MainEntered = 1,
    /// ウォッチドッグ延長タスクを起動した
    FeedStarted = 2,
    /// SD から wifi.txt を読んだ (成否問わず)
    SdRead = 3,
    /// LCD 走査を開始した
    DisplayStarted = 4,
    /// CYW43 の初期化 (ファームウェア転送) を開始
    WifiInit = 5,
    /// CYW43 と embassy-net が起動した
    WifiReady = 6,
    /// join を開始
    Joining = 7,
    /// join 失敗 (再試行待ち)
    JoinFailed = 8,
    Joined = 9,
    /// DHCP 待ち
    DhcpWait = 10,
    /// DHCP が 20 s で完了しなかった (DHCP クライアントは継続)
    DhcpTimeout = 11,
    /// IP を取得した (自己診断の最後の条件)
    NetworkUp = 12,
    /// explicit_buy を呼ぶ直前
    BuyCalled = 13,
    Bought = 14,
    BuyFailed = 15,
    /// 自己診断の締め切りを過ぎ、延長をやめた
    SelftestTimedOut = 16,
    /// CYW43 の電源断 (WL_REG_ON Low を `wifi::CYW43_POWER_OFF_MS` 保持) 中 (0.2.6〜)
    WifiPowerCycle = 17,
    /// DHCP タイムアウト後に AP から離脱し、再 join する (0.2.6〜)。次のループで `Joining` に進む
    DhcpRetry = 18,
    /// panic ハンドラに入った (SCRATCH7 = 行番号)
    Panic = 0xE0,
    /// HardFault に入った (SCRATCH7 = PC)
    HardFault = 0xE1,
}

impl Stage {
    pub fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            0 => Self::None,
            1 => Self::MainEntered,
            2 => Self::FeedStarted,
            3 => Self::SdRead,
            4 => Self::DisplayStarted,
            5 => Self::WifiInit,
            6 => Self::WifiReady,
            7 => Self::Joining,
            8 => Self::JoinFailed,
            9 => Self::Joined,
            10 => Self::DhcpWait,
            11 => Self::DhcpTimeout,
            12 => Self::NetworkUp,
            13 => Self::BuyCalled,
            14 => Self::Bought,
            15 => Self::BuyFailed,
            16 => Self::SelftestTimedOut,
            17 => Self::WifiPowerCycle,
            18 => Self::DhcpRetry,
            0xE0 => Self::Panic,
            0xE1 => Self::HardFault,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::MainEntered => "main",
            Self::FeedStarted => "wdt-feed",
            Self::SdRead => "sd-read",
            Self::DisplayStarted => "lcd",
            Self::WifiInit => "cyw43-init",
            Self::WifiReady => "cyw43-ready",
            Self::Joining => "joining",
            Self::JoinFailed => "join-failed",
            Self::Joined => "joined",
            Self::DhcpWait => "dhcp-wait",
            Self::DhcpTimeout => "dhcp-timeout",
            Self::NetworkUp => "network-up",
            Self::BuyCalled => "buy-called",
            Self::Bought => "bought",
            Self::BuyFailed => "buy-failed",
            Self::SelftestTimedOut => "selftest-timeout",
            Self::WifiPowerCycle => "cyw43-pwr-cycle",
            Self::DhcpRetry => "dhcp-rejoin",
            Self::Panic => "PANIC",
            Self::HardFault => "HARDFAULT",
        }
    }

    /// 異常終了 (PANIC / HARDFAULT) か
    pub fn is_fault(self) -> bool {
        (self as u8) >= 0xE0
    }
}

/// 自己診断の進み具合 (SCRATCH7、`Stage` が PANIC / HARDFAULT 以外のとき)
///
/// 4 × u8 で 32 bit を使い切っている。0.2.6 の DHCP 再試行 (leave → 再 join) は `join_attempts` に
/// 含める (再 join 回数 = `join_attempts` − `join_failures` − 1)。旧版がこの語を復号して表示するので、
/// 配置は変えない。
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, defmt::Format)]
pub struct SelftestCounters {
    /// join を呼んだ回数 (失敗後の再試行と、DHCP タイムアウト後の再 join を含む)
    pub join_attempts: u8,
    pub join_failures: u8,
    pub dhcp_timeouts: u8,
    /// 直近の join 失敗 status (cyw43)。0 なら無し
    pub last_join_status: u8,
}

impl SelftestCounters {
    pub fn pack(self) -> u32 {
        u32::from(self.join_attempts)
            | u32::from(self.join_failures) << 8
            | u32::from(self.dhcp_timeouts) << 16
            | u32::from(self.last_join_status) << 24
    }

    pub fn unpack(word: u32) -> Self {
        Self {
            join_attempts: (word & 0xff) as u8,
            join_failures: ((word >> 8) & 0xff) as u8,
            dhcp_timeouts: ((word >> 16) & 0xff) as u8,
            last_join_status: ((word >> 24) & 0xff) as u8,
        }
    }
}

/// 前回の TBYB 起動が残した記録
#[derive(Clone, Copy, Debug, defmt::Format)]
pub struct Trace {
    /// 記録を書いた版 (IMAGE_DEF major / minor。minor = Cargo minor × 100 + patch)
    pub major: u8,
    pub minor: u16,
    /// 最後に記録した段階 (未知の値なら None)
    pub stage: Option<Stage>,
    pub stage_code: u8,
    /// 最後に記録した時点の稼働時間 (100 ms 単位)
    pub uptime_ds: u32,
    /// SCRATCH7 の生値 (意味は `stage` による)
    pub info: u32,
}

impl Trace {
    pub fn counters(&self) -> SelftestCounters {
        SelftestCounters::unpack(self.info)
    }

    pub fn stage_label(&self) -> &'static str {
        self.stage.map(Stage::label).unwrap_or("?")
    }
}

/// `WATCHDOG.REASON` (直前のリセットがウォッチドッグ由来か)
#[derive(Clone, Copy, PartialEq, Eq, Debug, defmt::Format)]
pub enum ResetReason {
    /// ハードウェアリセット (電源投入 / RUN ピン / デバッガ)
    Hardware,
    /// ウォッチドッグのタイマ満了。bootrom の `reboot()` (FLASH_UPDATE 含む) もこれになる
    WatchdogTimer,
    /// `CTRL.TRIGGER` による強制
    WatchdogForce,
}

impl ResetReason {
    pub fn read() -> Self {
        let reason = WATCHDOG.reason().read();
        if reason.force() {
            Self::WatchdogForce
        } else if reason.timer() {
            Self::WatchdogTimer
        } else {
            Self::Hardware
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Hardware => "hw",
            Self::WatchdogTimer => "wdt",
            Self::WatchdogForce => "force",
        }
    }
}

/// `arm()` 済み (= この起動は TBYB で、記録を書く) か
static ARMED: AtomicBool = AtomicBool::new(false);

fn uptime_ds() -> u32 {
    (Instant::now().as_millis() / 100).min(0x00ff_ffff) as u32
}

/// 前回の記録を読む。`arm()` より前に呼ぶこと (arm は上書きする)。
pub fn read() -> Option<Trace> {
    let s5 = WATCHDOG.scratch5().read();
    if s5 & MAGIC_MASK != MAGIC {
        return None;
    }
    let s6 = WATCHDOG.scratch6().read();
    let stage_code = (s6 & 0xff) as u8;
    Some(Trace {
        major: ((s5 >> 16) & 0xf) as u8,
        minor: (s5 & 0xffff) as u16,
        stage: Stage::from_code(stage_code),
        stage_code,
        uptime_ds: s6 >> 8,
        info: WATCHDOG.scratch7().read(),
    })
}

/// 記録を開始する (TBYB で起動した側が main の最初で呼ぶ)。SCRATCH5..7 を初期化する。
pub fn arm() {
    WATCHDOG
        .scratch5()
        .write_value(MAGIC | (u32::from(IMAGE_DEF_MAJOR) & 0xf) << 16 | u32::from(IMAGE_DEF_MINOR));
    WATCHDOG.scratch6().write_value(0);
    WATCHDOG.scratch7().write_value(0);
    ARMED.store(true, Ordering::SeqCst);
}

pub fn is_armed() -> bool {
    ARMED.load(Ordering::Relaxed)
}

/// 進行段階を記録する (稼働時間も更新)。`arm()` していなければ何もしない。
pub fn stage(stage: Stage) {
    if is_armed() {
        WATCHDOG.scratch6().write_value(uptime_ds() << 8 | u32::from(stage as u8));
    }
}

/// 段階は変えずに稼働時間だけ更新する (ウォッチドッグ延長のたびに呼ぶ)
pub fn heartbeat() {
    if is_armed() {
        let stage = WATCHDOG.scratch6().read() & 0xff;
        WATCHDOG.scratch6().write_value(uptime_ds() << 8 | stage);
    }
}

/// 付加情報 (SCRATCH7) を書く
pub fn info(word: u32) {
    if is_armed() {
        WATCHDOG.scratch7().write_value(word);
    }
}

/// 異常終了を記録する (panic / HardFault ハンドラから)。既に異常終了が記録されていれば
/// 上書きしない (panic → udf → HardFault の順で来るので、最初の PANIC を残す)。
pub fn fault(stage: Stage, info: u32) {
    if !is_armed() {
        return;
    }
    let current = (WATCHDOG.scratch6().read() & 0xff) as u8;
    if Stage::from_code(current).is_some_and(Stage::is_fault) {
        return;
    }
    WATCHDOG.scratch6().write_value(uptime_ds() << 8 | u32::from(stage as u8));
    WATCHDOG.scratch7().write_value(info);
}
