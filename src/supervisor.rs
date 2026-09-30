//! 止まったら自分で戻る仕組み (0.4.1〜、`ticker` が使う): ウォッチドッグ、生存確認、スタックの監視、即時リセット
//!
//! 0.4.0 は最初の HTTPS の TLS ハンドシェイクでスタックが溢れて止まった (docs/ticker.md §8)。
//! buy の後は bootrom のウォッチドッグが無効になるので、画面は最後のフレームのまま何時間も固まっていた。
//!
//! # ウォッチドッグ (0.4.2〜: main の最初から、どの起動でも)
//!
//! 1. [`start_early`]: `embassy_rp::init` の直後 (SD / LCD / CYW43 の初期化より前) にハードウェアの
//!    ウォッチドッグを [`WATCHDOG_TIMEOUT_US`] (8 s) で動かす。TBYB 起動では bootrom が 16.7 s で動かして
//!    いたものを 8 s に縮めるだけ (CTRL の他のビットと、bootrom が SCRATCH2〜4 に置いた再起動の指定は触らない。
//!    時間切れでも強制リセットでも、bootrom は通常起動 = buy 済みの旧版を選ぶ)。
//! 2. 初期化の間は main が各段階の前に [`feed_init`] で明示的に再ロードする。どこかで止まれば 8 s でリセット。
//! 3. [`start`]: 描画タスクが動き出したら生存確認つきの再ロードに切り替える。各タスクは [`beat`] で生存を
//!    知らせ (main ループ、取得タスク、描画タスク)、LCD のフレーム割り込み (≈ 60 Hz) が 30 フレームごとに
//!    [`on_frame`] で確かめて、全員が `ticker::health::Limits` 以内ならウォッチドッグを再ロードする。止まった
//!    タスクがあれば boot_trace に記録してすぐリセットする。割り込みごと止まった場合 (ロックアップ等) は
//!    再ロードが止まり、8 s でウォッチドッグがリセットする (次の起動は「この版の記録 + 時間切れ」を異常終了と数える)。
//! 4. TBYB の buy 待ちも同じ仕組みで再ロードする (0.4.1 までは延長タスクが無条件に 16.7 s を延ばしていた)。
//!    buy 待ちで止まったタスクがあればリセット → 旧版へ戻る。[`set_buy_deadline`] の締め切り
//!    (`boot_policy::BUY_DEADLINE_MS`) を過ぎたら再ロードをやめ、8 s 後に旧版へ戻る。
//! 5. `explicit_buy` は bootrom がウォッチドッグを止める (CTRL.ENABLE = 0) ので、直後に [`rearm`] で動かし直す。
//!
//! - フラッシュの消去 / 書き込み中 (1 回 ≤ 0.4 s、割り込み禁止) も 8 s には十分遠い。
//! - 連続異常終了の回数 (`boot_policy::BootState`、SCRATCH1) は、通常モードで OTA 確認が通って
//!   ([`ota_proved`]) から `boot_policy::HEALTHY_CLEAR_MS` (10 分) 異常なく動いたら 0 に戻す。
//!
//! # スタック
//!
//! - [`set_stack_limit`]: ARMv8-M の MSPLIM を `.uninit` の上端 (= `_stack_end`) に設定する。越えると
//!   UsageFault (STKOF) → HardFault になり、`ticker` の HardFault ハンドラが「STACK OVERFLOW」を記録して
//!   リセットする。0.4.0 のように .bss の最上位 (`FRAME_WAKER` など) を黙って壊すことはもう無い。
//!   CCR.STKOFHFNMIGN を立て、HardFault ハンドラ自身は下限より下 (`.uninit` の defmt RTT バッファ) を使える。
//! - [`paint_stack`] / [`stack_used`]: 起動時に空きスタックへ模様を塗り、どこまで上書きされたかで最大使用量を測る。

use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};

use embassy_rp::pac::{PSM, WATCHDOG};
use embassy_time::Instant;

use crate::boot_policy::{BootState, HEALTHY_CLEAR_MS};
use crate::boot_trace::{self, Stage};
use crate::ticker::health::{self, Limits, WHO_COUNT, Who};

/// ウォッチドッグの時間 (µs)。RP2350 の上限は 24 bit = 16.7 s
pub const WATCHDOG_TIMEOUT_US: u32 = 8_000_000;
/// 何フレームごとに確かめるか (≈ 0.5 s)
const CHECK_EVERY_FRAMES: u32 = 30;
/// ウォッチドッグのリセット対象 (PSM WDSEL): ROSC (bit 2) と XOSC (bit 3) 以外の全部 (pico-sdk と同じ)。
/// embassy-rp 0.9 の `Watchdog` は RP2040 のビット配置の値を書くので使わない。
const WDSEL_ALL_BUT_OSC: u32 = 0x01ff_ffff & !(1 << 2 | 1 << 3);

const PHASE_OFF: u8 = 0;
/// 初期化中: main の [`feed_init`] だけが再ロードする
const PHASE_INIT: u8 = 1;
/// 生存確認つき: フレーム割り込みが再ロードする
const PHASE_SUPERVISED: u8 = 2;

static PHASE: AtomicU8 = AtomicU8::new(PHASE_OFF);
static STREAK_CLEARED: AtomicBool = AtomicBool::new(false);
static LAST_BEAT: [AtomicU32; WHO_COUNT] = [const { AtomicU32::new(0) }; WHO_COUNT];
static LIMIT_MS: [AtomicU32; WHO_COUNT] = [const { AtomicU32::new(0) }; WHO_COUNT];
/// buy 待ちの締め切り (起動からの ms、0 = 無し)
static BUY_DEADLINE_MS: AtomicU32 = AtomicU32::new(0);
static DEADLINE_LOGGED: AtomicBool = AtomicBool::new(false);
/// 最初に OTA 確認が通った時刻 (起動からの ms、0 = まだ)
static PROVED_AT_MS: AtomicU32 = AtomicU32::new(0);
/// 連続異常終了の回数を 0 に戻してよい (通常モードで、buy 待ちでない)
static CLEAR_ALLOWED: AtomicBool = AtomicBool::new(false);

fn now_ms() -> u32 {
    Instant::now().as_millis() as u32
}

/// 生存を知らせる (各タスクのループで呼ぶ。軽い: 時刻を 1 語書くだけ)
pub fn beat(who: Who) {
    // `health::PARKED` (u32::MAX) は「監視しない」の印なので、時刻がちょうどその値になる 1 ms は避ける
    LAST_BEAT[who as usize].store(now_ms().min(health::PARKED - 1), Ordering::Relaxed);
}

/// `who` の監視を止める (設定ページのサーバが要求を処理し終えて待ち受けに戻ったとき。0.5.0〜)。
/// 次の [`beat`] で監視が再び始まる
pub fn park(who: Who) {
    LAST_BEAT[who as usize].store(health::PARKED, Ordering::Relaxed);
}

/// `who` が `within_ms` 以内に生存を知らせたか (TBYB の自己診断で「描画が回っている」の確認に使う)
pub fn alive(who: Who, within_ms: u32) -> bool {
    let last = LAST_BEAT[who as usize].load(Ordering::Relaxed);
    if last == health::PARKED {
        return true;
    }
    let age = now_ms().wrapping_sub(last);
    age <= within_ms
}

/// 全タスクが期限内に生存を知らせている (生存確認つきの監視中のみ true。buy 条件の「健全」)
pub fn all_alive() -> bool {
    if PHASE.load(Ordering::Relaxed) != PHASE_SUPERVISED {
        return false;
    }
    health::stalled(now_ms(), &last_beats(), &limits()).is_none()
}

/// 生存確認つきの監視中か
pub fn is_active() -> bool {
    PHASE.load(Ordering::Relaxed) == PHASE_SUPERVISED
}

fn last_beats() -> [u32; WHO_COUNT] {
    core::array::from_fn(|i| LAST_BEAT[i].load(Ordering::Relaxed))
}

fn limits() -> Limits {
    Limits {
        ms: core::array::from_fn(|i| LIMIT_MS[i].load(Ordering::Relaxed)),
    }
}

fn enable() {
    PSM.wdsel().write_value(embassy_rp::pac::psm::regs::Wdsel(WDSEL_ALL_BUT_OSC));
    feed();
    WATCHDOG.ctrl().modify(|w| w.set_enable(true));
}

/// main の最初 (`embassy_rp::init` の直後) にウォッチドッグを 8 s で動かす (0.4.2〜)。
/// 以後 [`start`] までは [`feed_init`] で明示的に再ロードする。
pub fn start_early() {
    enable();
    PHASE.store(PHASE_INIT, Ordering::SeqCst);
}

/// 初期化中の明示的な再ロード (各段階の前に呼ぶ)。生存確認つきの監視が始まった後は何もしない
pub fn feed_init() {
    if PHASE.load(Ordering::Relaxed) == PHASE_INIT {
        feed();
    }
}

/// 生存確認つきの監視を始める (2 回目以降は何もしない)。描画タスクを起動した直後に呼ぶ。
/// 呼ぶまでに間があるタスク (取得タスクは CYW43 の初期化の後に起動する) も、ここから `limits` の猶予がある。
pub fn start(limits: Limits) {
    if PHASE.load(Ordering::Relaxed) == PHASE_SUPERVISED {
        return;
    }
    let now = now_ms();
    for who in Who::ALL {
        LIMIT_MS[who as usize].store(limits.ms[who as usize], Ordering::Relaxed);
        let start = if who.starts_parked() { health::PARKED } else { now };
        LAST_BEAT[who as usize].store(start, Ordering::Relaxed);
    }
    enable();
    PHASE.store(PHASE_SUPERVISED, Ordering::SeqCst);
    // 次の起動が「記録の無いウォッチドッグ・リセット」(割り込みごと止まった) を見分けるための印 (0.4.1 の表示用)
    boot_trace::set_extra(health::SUPERVISED_MARK);
    defmt::info!(
        "supervisor: watchdog {} ms, limits main {} ms jobs {} ms render {} ms web {} ms",
        WATCHDOG_TIMEOUT_US / 1000,
        limits.ms[0],
        limits.ms[1],
        limits.ms[2],
        limits.ms[3]
    );
}

/// TBYB の buy 待ちの締め切り (起動からの ms)。過ぎたら再ロードをやめる (8 s 後に旧版へ戻る)。
/// `None` で外す (buy の後)
pub fn set_buy_deadline(deadline_ms: Option<u32>) {
    BUY_DEADLINE_MS.store(deadline_ms.map_or(0, |d| d.max(1)), Ordering::SeqCst);
}

/// `explicit_buy` の直後に呼ぶ: bootrom が止めたウォッチドッグをすぐ動かし直し、締め切りを外す
pub fn rearm() {
    set_buy_deadline(None);
    enable();
}

/// OTA 確認が TLS + HTTP を最後まで通った (初回だけ時刻を覚える。10 分後に連続回数を 0 に戻す基準)
pub fn ota_proved() {
    let _ = PROVED_AT_MS.compare_exchange(0, now_ms().max(1), Ordering::SeqCst, Ordering::Relaxed);
}

/// 連続異常終了の回数を 0 に戻してよいか (通常モードで、buy 待ちでないときだけ true にする)
pub fn allow_streak_clear(allowed: bool) {
    CLEAR_ALLOWED.store(allowed, Ordering::SeqCst);
}

fn feed() {
    WATCHDOG.load().write(|w| w.set_load(WATCHDOG_TIMEOUT_US));
}

/// LCD のフレーム割り込みから毎フレーム呼ばれる (`frame` = 走査開始からのフレーム数)。
/// 生存確認つきの監視中でなければ何もしない。
pub fn on_frame(frame: u32) {
    if !frame.is_multiple_of(CHECK_EVERY_FRAMES) || PHASE.load(Ordering::Relaxed) != PHASE_SUPERVISED {
        return;
    }
    let now = now_ms();
    match health::stalled(now, &last_beats(), &limits()) {
        None => {
            let deadline = BUY_DEADLINE_MS.load(Ordering::Relaxed);
            if deadline != 0 && now >= deadline {
                // buy 待ちの締め切りを過ぎた: 再ロードをやめる (8 s 後に時間切れ → 通常起動 = 旧版)
                if !DEADLINE_LOGGED.swap(true, Ordering::Relaxed) {
                    boot_trace::stage(Stage::SelftestTimedOut);
                    defmt::warn!("supervisor: TBYB buy deadline passed, letting the watchdog roll back");
                }
                return;
            }
            feed();
            boot_trace::heartbeat();
            let proved_at = PROVED_AT_MS.load(Ordering::Relaxed);
            if CLEAR_ALLOWED.load(Ordering::Relaxed)
                && proved_at != 0
                && now.wrapping_sub(proved_at) >= HEALTHY_CLEAR_MS
                && !STREAK_CLEARED.swap(true, Ordering::Relaxed)
            {
                // OTA 確認が通ってから異常終了なしで 10 分動いた: 連続異常終了の回数を 0 に戻す
                boot_trace::write_streak(BootState::healthy().encode());
            }
        }
        Some((who, ms)) => {
            let stage = match who {
                Who::Main => Stage::WdtMain,
                Who::Jobs => Stage::WdtJobs,
                Who::Render => Stage::WdtRender,
                Who::Web => Stage::WdtWeb,
            };
            boot_trace::fault_with(stage, ms, 0);
            defmt::error!("supervisor: {} stalled for {} ms, resetting", who.label(), ms);
            reset_now();
        }
    }
}

/// ウォッチドッグの強制トリガでチップをリセットする (panic / HardFault / 停止の記録の後)。
/// SCRATCH0〜7 と WATCHDOG.REASON (`force`) は残る。bootrom は通常起動で同じ (buy 済みの) 区画を選ぶ。
/// TBYB の buy 待ち中なら buy されていないので旧版へ戻る。
pub fn reset_now() -> ! {
    PSM.wdsel().write_value(embassy_rp::pac::psm::regs::Wdsel(WDSEL_ALL_BUT_OSC));
    // 念のため 1 ms の時間切れも仕掛けてから強制トリガ (デバッガ接続中の一時停止も解除)
    WATCHDOG.load().write(|w| w.set_load(1_000));
    WATCHDOG.ctrl().modify(|w| {
        w.set_pause_dbg0(false);
        w.set_pause_dbg1(false);
        w.set_pause_jtag(false);
        w.set_enable(true);
        w.set_trigger(true);
    });
    loop {
        cortex_m::asm::nop();
    }
}

// ============================================================
// スタック
// ============================================================

unsafe extern "C" {
    static _stack_start: u32;
    static _stack_end: u32;
}

/// スタック領域 (下端 `_stack_end` = `.uninit` の上端, 上端 `_stack_start`) のアドレス
pub fn stack_bounds() -> (usize, usize) {
    (&raw const _stack_end as usize, &raw const _stack_start as usize)
}

/// スタックの大きさ (バイト)
pub fn stack_size() -> u32 {
    let (bottom, top) = stack_bounds();
    (top - bottom) as u32
}

/// MSPLIM をスタックの下端に設定し、HardFault / NMI ではスタック下限の検査をしないようにする
/// (CCR.STKOFHFNMIGN)。main の最初に 1 回呼ぶ。
pub fn set_stack_limit() {
    let (bottom, _) = stack_bounds();
    // Safety: MSPLIM を今の SP より下に設定するだけ (SP は上端付近)。CCR は SCB のレジスタ。
    unsafe {
        let ccr = 0xE000_ED14 as *mut u32;
        core::ptr::write_volatile(ccr, core::ptr::read_volatile(ccr) | 1 << 10);
        core::arch::asm!("msr MSPLIM, {}", in(reg) (bottom + 7) & !7, options(nomem, nostack, preserves_flags));
    }
}

/// 今の SP より下 (64 B の余裕を残す) の空きスタックに模様を塗る。main の最初に 1 回呼ぶ。
/// 割り込みが途中で下を使っても、戻った後に塗り直すだけなので害はない。
#[inline(never)]
pub fn paint_stack() {
    let (bottom, _) = stack_bounds();
    let sp = cortex_m::register::msp::read() as usize;
    let mut p = (bottom + 3) & !3;
    while p + 64 < sp {
        // Safety: [bottom, sp - 64) は誰も使っていないスタック領域
        unsafe { core::ptr::write_volatile(p as *mut u32, health::STACK_PAINT) };
        p += 4;
    }
}

/// 起動からのスタック最大使用量 (バイト)。下端から模様が残っている語を数える (数十 µs)
pub fn stack_used() -> u32 {
    let (bottom, top) = stack_bounds();
    let mut p = (bottom + 3) & !3;
    // Safety: スタック領域は常に読める。模様と比べるだけで、使用中の値を解釈はしない (volatile で読む)
    while p < top && unsafe { core::ptr::read_volatile(p as *const u32) } == health::STACK_PAINT {
        p += 4;
    }
    (top - p) as u32
}
