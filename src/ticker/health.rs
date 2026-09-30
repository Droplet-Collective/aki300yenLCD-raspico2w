//! 止まったら自分で戻るための判定と、前回のリセット理由の表示 (0.4.1〜、ハードウェアに依存しない部分)
//!
//! 0.4.0 は最初の HTTPS (天気) の TLS ハンドシェイクでスタックが溢れ、.bss の最上位にある
//! `FRAME_WAKER` などを壊して HardFault → `loop {}` で止まった (buy 後はウォッチドッグも無効)。
//! 0.4.1 はウォッチドッグを buy 後も動かし続け、panic / HardFault / 停止を記録してから自分でリセットする。
//! この module はその判定 (どのタスクが止まったか) と、次の起動で状態行に出す文字列を作る。
//! レジスタの読み書きは `crate::supervisor` と `crate::boot_trace` が行う。
//! `tools/ticker-tests` がホストでテストする (`core` + `heapless` だけに依存)。

use core::fmt::Write as _;

use heapless::String;

// ============================================================
// 生存確認 (heartbeat)
// ============================================================

/// 生存確認をするタスク
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Who {
    /// main ループ (接続管理 / TBYB / 状態行)。250 ms ごと。join + DHCP 待ちで最長 ≈ 25 s 止まる
    Main = 0,
    /// 取得タスク (OTA / NTP / 天気 / 文字)。1 回の取得は 20〜30 s で打ち切る。OTA のダウンロード中は
    /// 250 ms ごとの進捗通知で生存を知らせる
    Jobs = 1,
    /// 描画タスク。毎フレーム (≈ 60 Hz)
    Render = 2,
}

pub const WHO_COUNT: usize = 3;

impl Who {
    pub const ALL: [Who; WHO_COUNT] = [Who::Main, Who::Jobs, Who::Render];

    pub fn label(self) -> &'static str {
        match self {
            Who::Main => "main",
            Who::Jobs => "jobs",
            Who::Render => "render",
        }
    }
}

/// 各タスクが生存を知らせないまま許す最長時間 (ms)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub ms: [u32; WHO_COUNT],
}

impl Limits {
    /// ticker の既定値。描画は 5 s (フラッシュ消去で割り込みが止まるのは 1 回 0.4 s 以内、TLS の鍵交換も
    /// 1 s 以内)、main / 取得は 90 s (どの待ちも 30 s 以内で打ち切られる)。
    pub const TICKER: Limits = Limits {
        ms: [90_000, 90_000, 5_000],
    };
}

/// 生存確認の結果: 止まっているタスクと、最後に知らせてからの時間 (ms)。全員生きていれば None
pub fn stalled(now_ms: u32, last_ms: &[u32; WHO_COUNT], limits: &Limits) -> Option<(Who, u32)> {
    for who in Who::ALL {
        let age = now_ms.wrapping_sub(last_ms[who as usize]);
        // 未来の値 (起動直後に書き換わった直後など) は 0 とみなす
        let age = if age > u32::MAX / 2 { 0 } else { age };
        if age > limits.ms[who as usize] {
            return Some((who, age));
        }
    }
    None
}

// ============================================================
// 異常終了の記録 (boot_trace の SCRATCH6 下位 8 bit と同じ値)
// ============================================================

pub const STAGE_PANIC: u8 = 0xE0;
pub const STAGE_HARDFAULT: u8 = 0xE1;
/// MSPLIM (スタック下限) を越えた (CFSR.STKOF)。0.4.1〜
pub const STAGE_STACK_OVERFLOW: u8 = 0xE2;
/// ウォッチドッグ: main / 取得 / 描画タスクが止まった。0.4.1〜
pub const STAGE_WDT_MAIN: u8 = 0xE3;
pub const STAGE_WDT_JOBS: u8 = 0xE4;
pub const STAGE_WDT_RENDER: u8 = 0xE5;
/// 登録していない割り込み (DefaultHandler)。0.4.1〜
pub const STAGE_UNHANDLED_IRQ: u8 = 0xE6;

pub fn wdt_stage(who: Who) -> u8 {
    match who {
        Who::Main => STAGE_WDT_MAIN,
        Who::Jobs => STAGE_WDT_JOBS,
        Who::Render => STAGE_WDT_RENDER,
    }
}

/// 監視中 (ウォッチドッグを動かしている) の印。異常終了の記録が無いまま SCRATCH0 にこれが残っていて、
/// リセット理由がウォッチドッグの時間切れなら、割り込みごと止まった (ロックアップ等) と分かる
pub const SUPERVISED_MARK: u32 = 0x5355_5056; // "SUPV"

/// 前回の起動が残した異常終了の記録 (復号済み)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LastReset<'a> {
    /// panic。`file` は同じイメージのときだけ分かる (記録は `Location` へのポインタなので)
    Panic { file: Option<&'a str>, line: u32 },
    HardFault { pc: u32, lr: u32 },
    StackOverflow { pc: u32, lr: u32 },
    Stalled { who: Who, ms: u32 },
    UnhandledIrq { irqn: i32 },
    /// ウォッチドッグの時間切れだが記録が無い (割り込みも止まった = ロックアップ等)。最後に記録した段階
    WatchdogNoRecord { stage: &'a str },
}

/// SCRATCH の値から異常終了の種類を復号する (`stage` = SCRATCH6 下位 8 bit、`info` = SCRATCH7、
/// `extra` = SCRATCH0)。異常終了の記録でなければ None。
///
/// 記録の中身 (SCRATCH7 は 0.2.x からの旧版も読むので意味を変えない):
/// panic = 行番号 / `&Location` のアドレス、HardFault / スタック溢れ = PC / LR、停止 = 経過 ms / 0、
/// 未登録の割り込み = IRQ 番号 / 0。panic のファイル名は `file` (呼び出し側が SCRATCH0 の `Location`
/// ポインタを検証して解決する) を使う。
pub fn decode(stage: u8, info: u32, extra: u32, file: Option<&str>) -> Option<LastReset<'_>> {
    Some(match stage {
        STAGE_PANIC => LastReset::Panic { file, line: info },
        STAGE_HARDFAULT => LastReset::HardFault { pc: info, lr: extra },
        STAGE_STACK_OVERFLOW => LastReset::StackOverflow { pc: info, lr: extra },
        STAGE_WDT_MAIN => LastReset::Stalled { who: Who::Main, ms: info },
        STAGE_WDT_JOBS => LastReset::Stalled { who: Who::Jobs, ms: info },
        STAGE_WDT_RENDER => LastReset::Stalled { who: Who::Render, ms: info },
        STAGE_UNHANDLED_IRQ => LastReset::UnhandledIrq { irqn: info as i32 },
        _ => return None,
    })
}

/// パスの末尾 (最大 `max` バイト、`/` の直後から)。`src/ui/slide.rs` や
/// `embedded-tls-0.18.0/src/connection.rs` が読めるように
pub fn file_tail(path: &str, max: usize) -> &str {
    let path = path.trim_start_matches("./");
    if path.len() <= max {
        return path;
    }
    let mut start = path.len() - max;
    while !path.is_char_boundary(start) {
        start += 1;
    }
    let tail = &path[start..];
    match tail.find('/') {
        Some(i) if i + 1 < tail.len() => &tail[i + 1..],
        _ => tail,
    }
}

/// 状態行の桁数 (400 px / 6 px の `FONT_6X10`)
pub const STATUS_COLUMNS: usize = 66;

/// crates.io / git の依存クレートのパス (`.../registry/src/<index>/<crate>-<ver>/src/<file>`) なら
/// (`<crate>-<ver>`, `<file>`)、そうでなければ (None, パス)
pub fn split_crate_path(path: &str) -> (Option<&str>, &str) {
    for marker in ["/registry/src/", "/git/checkouts/"] {
        if let Some(i) = path.find(marker) {
            let after = &path[i + marker.len()..];
            // <index-dir>/<crate>-<ver>/... (git は <repo>-<hash>/<rev>/...)
            let mut parts = after.splitn(3, '/');
            let (_index, krate, rest) = (parts.next(), parts.next(), parts.next());
            if let (Some(krate), Some(rest)) = (krate, rest) {
                return (Some(krate), rest.strip_prefix("src/").unwrap_or(rest));
            }
        }
    }
    (None, path)
}

/// 状態行 1 に出す `last reset: ...` (`STATUS_COLUMNS` 桁以内に収める)。`uptime_ds` は記録時の稼働時間
/// (100 ms 単位)、`streak` は連続回数 (2 以上なら `#n` を付ける)
pub fn write_last_reset<const N: usize>(out: &mut String<N>, reset: &LastReset<'_>, uptime_ds: u32, streak: u32) {
    let mut suffix: String<24> = String::new();
    let _ = write!(suffix, " @{}s", uptime_ds / 10);
    if streak >= 2 {
        let _ = write!(suffix, " #{}", streak);
    }
    let start = out.len();
    let _ = out.push_str("last reset: ");
    match *reset {
        LastReset::Panic { file, line } => {
            let mut at: String<12> = String::new();
            let _ = write!(at, ":{}", line);
            let _ = out.push_str("panic ");
            let used = out.len() - start;
            let budget = STATUS_COLUMNS.saturating_sub(used + at.len() + suffix.len()).max(8);
            match split_crate_path(file.unwrap_or("?")) {
                (Some(krate), rest) if krate.len() + 1 + rest.len() <= budget => {
                    let _ = write!(out, "{}/{}", krate, rest);
                }
                (_, rest) => {
                    let _ = out.push_str(file_tail(rest, budget));
                }
            }
            let _ = out.push_str(&at);
        }
        LastReset::HardFault { pc, lr } => {
            let _ = write!(out, "HardFault pc={:08x} lr={:08x}", pc, lr);
        }
        LastReset::StackOverflow { pc, .. } => {
            let _ = write!(out, "STACK OVERFLOW pc={:08x}", pc);
        }
        LastReset::Stalled { who, ms } => {
            let _ = write!(out, "wdt: {} stalled {}s", who.label(), ms / 1000);
        }
        LastReset::UnhandledIrq { irqn } => {
            let _ = write!(out, "unhandled IRQ {}", irqn);
        }
        LastReset::WatchdogNoRecord { stage } => {
            let _ = write!(out, "wdt timeout (no record, {})", stage);
        }
    }
    let _ = out.push_str(&suffix);
}

// ============================================================
// 連続クラッシュの回数 (SCRATCH1) と安全モード
// ============================================================

/// SCRATCH1 の上位 16 bit (bootrom は SCRATCH0/1 に書かない)
pub const STREAK_MAGIC: u32 = 0xC0DE_0000;
/// この回数続けて異常終了したら、写真 / 天気 / 文字を止めて OTA と NTP だけにする
pub const SAFE_MODE_STREAK: u32 = 3;
/// 異常終了なしでこの時間動いたら回数を 0 に戻す
pub const STREAK_CLEAR_MS: u32 = 10 * 60 * 1000;

/// 起動時: 前回が異常終了なら回数 + 1、そうでなければ 0。戻り値は (今回の回数, SCRATCH1 に書く値)
pub fn streak_after_boot(prev_was_fault: bool, scratch1: u32) -> (u32, u32) {
    let stored = if scratch1 & 0xFFFF_0000 == STREAK_MAGIC { scratch1 & 0xFFFF } else { 0 };
    let streak = if prev_was_fault { (stored + 1).min(0xFFFF) } else { 0 };
    (streak, STREAK_MAGIC | streak)
}

// ============================================================
// スタックの使用量 (起動時に塗った模様がどこまで残っているか)
// ============================================================

/// スタック領域 (下位アドレス側から) に塗る模様
pub const STACK_PAINT: u32 = 0x5AC3_A53C;

/// 下位側から数えて、塗った模様のまま残っている語数
pub fn untouched_words(words: &[u32]) -> usize {
    words.iter().take_while(|&&w| w == STACK_PAINT).count()
}

/// `23.1K` の形 (1 KiB = 1024 B、小数 1 桁)
pub fn write_kib<const N: usize>(out: &mut String<N>, bytes: u32) {
    let tenths = (bytes as u64 * 10 + 512) / 1024;
    let _ = write!(out, "{}.{}K", tenths / 10, tenths % 10);
}
