//! 止まったら自分で戻る仕組み (0.4.1〜、`ticker` が使う): ウォッチドッグ、生存確認、スタックの監視、即時リセット
//!
//! 0.4.0 は最初の HTTPS の TLS ハンドシェイクでスタックが溢れて止まった (docs/ticker.md「止まったとき」)。
//! buy の後は bootrom のウォッチドッグが無効になるので、画面は最後のフレームのまま何時間も固まっていた。
//!
//! # ウォッチドッグと生存確認
//!
//! - [`start`] でハードウェアのウォッチドッグを [`WATCHDOG_TIMEOUT_US`] (8 s) で動かす。TBYB 起動なら
//!   explicit_buy の後 (buy 待ちの間は bootrom の 16.7 s を `ota::app::tbyb_watchdog_task` が延ばす)、
//!   TBYB でない起動なら描画が始まった直後に呼ぶ。
//! - 各タスクは [`beat`] で生存を知らせる (main ループ、取得タスク、描画タスク)。
//! - LCD のフレーム割り込み (`lcd::display::FrameIrqHandler`、≈ 60 Hz) が 30 フレームごとに [`on_frame`] で
//!   確かめ、全員が `ticker::health::Limits` 以内に知らせていればウォッチドッグを再ロードする。止まった
//!   タスクがあれば boot_trace に記録してすぐリセットする。割り込みで確かめるので、thread モードの
//!   無限ループでも記録できる。割り込みごと止まった場合 (ロックアップ等) は再ロードが止まり、8 s で
//!   ウォッチドッグがリセットする (記録は無いが `reset:wdt` で分かる)。
//! - フラッシュの消去 / 書き込み中 (1 回 ≤ 0.4 s、割り込み禁止) も 8 s には十分遠い。
//!
//! # スタック
//!
//! - [`set_stack_limit`]: ARMv8-M の MSPLIM を `.uninit` の上端 (= `_stack_end`) に設定する。越えると
//!   UsageFault (STKOF) → HardFault になり、`ticker` の HardFault ハンドラが「STACK OVERFLOW」を記録して
//!   リセットする。0.4.0 のように .bss の最上位 (`FRAME_WAKER` など) を黙って壊すことはもう無い。
//!   CCR.STKOFHFNMIGN を立て、HardFault ハンドラ自身は下限より下 (`.uninit` の defmt RTT バッファ) を使える。
//! - [`paint_stack`] / [`stack_used`]: 起動時に空きスタックへ模様を塗り、どこまで上書きされたかで最大使用量を測る。

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use embassy_rp::pac::{PSM, WATCHDOG};
use embassy_time::Instant;

use crate::boot_trace::{self, Stage};
use crate::ticker::health::{self, Limits, WHO_COUNT, Who};

/// ウォッチドッグの時間 (µs)。RP2350 の上限は 24 bit = 16.7 s
pub const WATCHDOG_TIMEOUT_US: u32 = 8_000_000;
/// 何フレームごとに確かめるか (≈ 0.5 s)
const CHECK_EVERY_FRAMES: u32 = 30;
/// ウォッチドッグのリセット対象 (PSM WDSEL): ROSC (bit 2) と XOSC (bit 3) 以外の全部 (pico-sdk と同じ)。
/// embassy-rp 0.9 の `Watchdog` は RP2040 のビット配置の値を書くので使わない。
const WDSEL_ALL_BUT_OSC: u32 = 0x01ff_ffff & !(1 << 2 | 1 << 3);

static ACTIVE: AtomicBool = AtomicBool::new(false);
static STREAK_CLEARED: AtomicBool = AtomicBool::new(false);
static LAST_BEAT: [AtomicU32; WHO_COUNT] = [AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0)];
static LIMIT_MS: [AtomicU32; WHO_COUNT] = [AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0)];

fn now_ms() -> u32 {
    Instant::now().as_millis() as u32
}

/// 生存を知らせる (各タスクのループで呼ぶ。軽い: 時刻を 1 語書くだけ)
pub fn beat(who: Who) {
    LAST_BEAT[who as usize].store(now_ms(), Ordering::Relaxed);
}

/// `who` が `within_ms` 以内に生存を知らせたか (TBYB の自己診断で「描画が回っている」の確認に使う)
pub fn alive(who: Who, within_ms: u32) -> bool {
    let age = now_ms().wrapping_sub(LAST_BEAT[who as usize].load(Ordering::Relaxed));
    age <= within_ms
}

/// 監視中か
pub fn is_active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

/// ウォッチドッグを動かして監視を始める (2 回目以降は何もしない)。
/// TBYB の buy 待ちの間は呼ばない (bootrom のウォッチドッグと延長タスクに任せる)。
pub fn start(limits: Limits) {
    if ACTIVE.load(Ordering::Relaxed) {
        return;
    }
    let now = now_ms();
    for who in Who::ALL {
        LIMIT_MS[who as usize].store(limits.ms[who as usize], Ordering::Relaxed);
        LAST_BEAT[who as usize].store(now, Ordering::Relaxed);
    }
    PSM.wdsel().write_value(embassy_rp::pac::psm::regs::Wdsel(WDSEL_ALL_BUT_OSC));
    feed();
    WATCHDOG.ctrl().modify(|w| w.set_enable(true));
    ACTIVE.store(true, Ordering::SeqCst);
    boot_trace::stage(Stage::Running);
    // 次の起動が「記録の無いウォッチドッグ・リセット」(割り込みごと止まった) を見分けるための印
    boot_trace::set_extra(health::SUPERVISED_MARK);
    defmt::info!(
        "supervisor: watchdog {} ms, limits main {} ms jobs {} ms render {} ms",
        WATCHDOG_TIMEOUT_US / 1000,
        limits.ms[0],
        limits.ms[1],
        limits.ms[2]
    );
}

fn feed() {
    WATCHDOG.load().write(|w| w.set_load(WATCHDOG_TIMEOUT_US));
}

/// LCD のフレーム割り込みから毎フレーム呼ばれる (`frame` = 走査開始からのフレーム数)。
/// 監視中でなければ何もしない。
pub fn on_frame(frame: u32) {
    if !frame.is_multiple_of(CHECK_EVERY_FRAMES) || !ACTIVE.load(Ordering::Relaxed) {
        return;
    }
    let now = now_ms();
    let last: [u32; WHO_COUNT] = core::array::from_fn(|i| LAST_BEAT[i].load(Ordering::Relaxed));
    let limits = Limits {
        ms: core::array::from_fn(|i| LIMIT_MS[i].load(Ordering::Relaxed)),
    };
    match health::stalled(now, &last, &limits) {
        None => {
            feed();
            boot_trace::heartbeat();
            if now >= health::STREAK_CLEAR_MS && !STREAK_CLEARED.swap(true, Ordering::Relaxed) {
                // 異常終了なしで 10 分動いた: 連続クラッシュの回数を 0 に戻す
                boot_trace::write_streak(health::STREAK_MAGIC);
            }
        }
        Some((who, ms)) => {
            let stage = match who {
                Who::Main => Stage::WdtMain,
                Who::Jobs => Stage::WdtJobs,
                Who::Render => Stage::WdtRender,
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
