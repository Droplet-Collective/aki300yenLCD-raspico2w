//! LCD 走査 (PIO0 SM0/SM1 + DMA CH0〜CH3 の自走リング) とダブルバッファ描画
//!
//! `wifi_status` / `sd_bmp_viewer` / `ota_selftest` が共用する表示層。
//! 以前は各 bin に同じ `display_task` がコピーされていたが、フラッシュ操作
//! (TBYB の explicit_buy、第 2 段階の OTA 書き込み) と共存できるようにここへ集約した。
//!
//! # 走査 (フロントバッファ、CPU 不介入)
//!
//! ```text
//! FRONT (509×113 ワード, SRAM) ──CH0──▶ PIO0 SM0 TX (RGB + NCLK)
//!         ▲ CH2 が毎フレーム先頭アドレスを CH0 の READ_ADDR_TRIG へ書き戻す
//! SM1_FRAME_DATA (114 ワード, SRAM .data) ──CH1──▶ PIO0 SM1 TX (HSYNC/VSYNC)
//!         ▲ CH3 が毎フレーム先頭アドレスを CH1 の READ_ADDR_TRIG へ書き戻す
//! ```
//!
//! - 初回だけ `MULTI_CHAN_TRIGGER` で CH0/CH1 を同時起動し、以後は
//!   `CH0 → CH2 → CH0`、`CH1 → CH3 → CH1` の chain_to で永久に回る。
//!   割り込みハンドラや async タスクによるフレーム毎の再武装は無い。
//! - DMA が読むメモリは **全て SRAM** に置く。`SM1_FRAME_DATA` は
//!   `#[link_section = ".data…"]` で RAM に配置する (以前は `.rodata` = フラッシュ
//!   XIP 窓にあり、フラッシュ消去/書き込み中は QMI がダイレクトモードになって XIP
//!   窓が **バスフォールト** を返す (データシート §5.4.8.10, §12.14.5)。バスエラーを
//!   受けた DMA チャネルはエラーフラグを消すまで **停止したまま** (§12.6.7.1) なので、
//!   explicit_buy 1 回で HSYNC/VSYNC の供給が永久に止まり、NCLK と画素だけが
//!   出続ける「砂嵐」になっていた)。
//! - したがって CPU が数百 ms 止まっても (セクタ消去 + ページ書き込み、割り込み禁止)
//!   走査は影響を受けない。位相 (CH0 と CH1 の相対関係) も崩れない。
//!
//! # 描画 (バックバッファ、ティアリング無し)
//!
//! アプリは 400×96 の [`BackBuffer`] に描き、[`Display::present`] で垂直
//! ブランキング中にフロントへコピーする。フレーム先頭は CH2 完了割り込み
//! (`DMA_IRQ_1`、embassy が使う `DMA_IRQ_0` とは別) で検出する。コピーは
//! 上から下へ 1 行 ≈ 5 µs で進み、走査は 1 行 ≈ 147 µs なので、ブランキング期間
//! (16 行 ≈ 2.4 ms) の前半に始めれば走査に追い越されない。間に合わなければ次の
//! フレームまで待つ (数回連続で間に合わない場合だけ諦めて即時コピーする)。
//!
//! この割り込みは描画のペース合わせ専用で、走査の維持には関与しない。
//! (フラッシュ操作中は割り込み禁止なので、走査維持を割り込みに頼る設計は不可)。
//!
//! # 可視位置の補正
//!
//! LCD が最初の表示画素として取り込むのはフレーム行の x=[`VISIBLE_X_OFFSET`] (106) 番目の
//! ワードなので、コピー時にバックバッファをそこへ置き、行の残りは端の画素色で埋める。
//! 根拠は [`VISIBLE_X_OFFSET`] のコメント。
//!
//! # メモリ
//!
//! FRONT 230,068 B + BACK 76,800 B (RGB565、v0.3 までは RGB666 ワードで 153,600 B) + SM1 データ 456 B
//! + 走査ワードの表 2 KB。

use core::future::poll_fn;
use core::ptr::{addr_of, addr_of_mut};
use core::sync::atomic::{AtomicU32, Ordering, compiler_fence};
use core::task::Poll;

use embassy_rp::Peri;
use embassy_rp::interrupt::InterruptExt;
use embassy_rp::interrupt::typelevel::{Binding, DMA_IRQ_1, Handler, PIO0_IRQ_0};
use embassy_rp::pac;
use embassy_rp::pac::dma::regs::CtrlTrig;
use embassy_rp::pac::dma::vals::{DataSize, TreqSel};
use embassy_rp::peripherals::*;
use embassy_rp::pio::program::pio_asm;
use embassy_rp::pio::{
    Config, Direction, FifoJoin, InterruptHandler, Pio, ShiftConfig, ShiftDirection,
};
use embassy_sync::waitqueue::AtomicWaker;
use embedded_graphics::geometry::Size;
use embedded_graphics::pixelcolor::{Rgb666, RgbColor};
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;
use fixed::FixedU32;
use fixed::types::extra::U8;

use crate::lcd::framebuffer::{ACTIVE_HEIGHT, ACTIVE_Y_OFFSET, FB_SIZE, FrameBuffer, LINE_WIDTH, rgb666};
use crate::lcd::timing::{H_ACTIVE, H_TOTAL, PIO_CLK_DIV_FRAC, PIO_CLK_DIV_INT, SM1_CLK_DIV_BITS, V_NORMAL_LINES};

// ============================================================
// 定数
// ============================================================

/// バックバッファの幅 (LCD の表示幅)
pub const BACK_WIDTH: usize = H_ACTIVE as usize; // 400
/// バックバッファの高さ (LCD の表示高)
pub const BACK_HEIGHT: usize = ACTIVE_HEIGHT; // 96
/// バックバッファの画素数 (38,400 画素 = 76,800 バイト)
pub const BACK_SIZE: usize = BACK_WIDTH * BACK_HEIGHT;

/// LCD の最初の表示画素に対応するフレーム行内の x (バックバッファの x=0 を置く位置)。
///
/// 名目値は `H_BACK_PORCH` = 108 (HSYNC パルス 1 + バックポーチ 107、`timing.rs`) だが、
/// SM0 と SM1 の起動位相で 2 ワードずれる:
///
/// - SM1 (HSYNC/VSYNC) は有効化直後に `set pins, 3` / `set pins, 2` の 2 命令 (= 2 NCLK) を
///   実行してから `pull block` で止まる。SM0 (画素) は先頭の `out` で止まる。
/// - `MULTI_CHAN_TRIGGER` で CH0/CH1 が同時に FIFO を埋めると両 SM は同時に動き出すが、
///   SM1 は既に行の 2 サイクル目まで進んでいるので、以後すべての行で HSYNC の立ち下がり
///   (行の 2 サイクル目、`set pins, 0`) はフレーム行の **x=−1** (前行の最終ワード) の
///   NCLK 立ち下がりで LCD に取り込まれる。
/// - LTA042B010F は HSYNC 取り込みから 107 クロック後の画素を表示開始とする
///   (`docs/datasheet-LTA042B010F.md` の H back porch) ので、最初の表示画素は
///   フレーム行の x = −1 + 107 = **106**。
///
/// **正式値 (実機確認済み、2026-09-29)**: 以前の 98 では左端の 1 文字 (6 px) が欠けていた
/// (バックバッファの x=0..7 が表示開始より左に置かれていた)。106 にした v0.2.7 で、外周 1 px の枠線が
/// 四辺とも見え、起動直後の x 座標目盛りの左端 `0` と右端 `390` が両方読めることを実機の写真で確認した。
/// 目盛りは v0.2.8 で削除し、`wifi_ota` は外周 1 px の枠 (`draw_frame_border`) だけを常に描いて
/// 簡易確認に使う。
///
/// 106 + 400 = 506 ≤ 509 (`LINE_WIDTH`) なので右端も切れない (残り 3 ワードは
/// 右端の画素色で埋め、次行の HSYNC までのフロントポーチになる)。
pub const VISIBLE_X_OFFSET: usize = 106;

const _: () = assert!(
    VISIBLE_X_OFFSET + BACK_WIDTH <= LINE_WIDTH,
    "back buffer must fit in the frame line after VISIBLE_X_OFFSET"
);

/// `present()` がコピーを始めてよい最終ライン (0 起点のフレーム行)。
/// これより前 (垂直ブランキング 16 行のうち先頭 12 行以内) に起きられれば、
/// 残り 4 行 ≈ 0.6 ms + 走査より速いコピーで走査に追い越されない。
const PRESENT_DEADLINE_LINE: usize = ACTIVE_Y_OFFSET - 4;
/// 連続でこの回数フレーム先頭に間に合わなければ即時コピーする (1 フレームだけ乱れる)。
const PRESENT_MAX_MISSES: u32 = 4;

/// SM1 が 1 フレームで pull するワード数 (VSYNC 行の残りカウント + 通常行数 + 各通常行の残りカウント)
const SM1_FRAME_SIZE: usize = 2 + V_NORMAL_LINES as usize;

/// フレーム先頭 (CH2 完了) を通知する DMA チャネルのビット
const FRAME_START_CHANNEL_BIT: u32 = 1 << 2;

// ============================================================
// static 配置のバッファ
// ============================================================

/// DMA が走査するフロントバッファ (BSS、ゼロ = 黒で初期化)
static mut FRONT: FrameBuffer = FrameBuffer::new();

/// アプリが描くバックバッファ (BSS)
static mut BACK: BackBuffer = BackBuffer::new();

const fn sm1_frame_data() -> [u32; SM1_FRAME_SIZE] {
    let mut data = [0; SM1_FRAME_SIZE];
    data[0] = H_TOTAL - 7; // VSYNC 行: set×2 + pull/mov×2 + loop(X+1)
    data[1] = V_NORMAL_LINES - 1;
    let mut line = 0;
    while line < V_NORMAL_LINES as usize {
        data[2 + line] = H_TOTAL - 6; // 通常行: set×2 + pull/mov + loop(X+1) + jmp
        line += 1;
    }
    data
}

/// SM1 (HSYNC/VSYNC) 用タイミングデータ。CH1 が毎フレーム読む。
///
/// **必ず RAM に置く** (`.data` セクション)。`.rodata` (フラッシュ) に置くと
/// フラッシュ消去・書き込み中に DMA がバスフォールトを受けて CH1 が停止する。
#[unsafe(link_section = ".data.lcd_sm1_frame")]
static SM1_FRAME_DATA: [u32; SM1_FRAME_SIZE] = sm1_frame_data();

// CH2/CH3 が各フレーム終端で読み、CH0/CH1 の読み出し先を再設定する (BSS)。
static DMA_PIXEL_FRAME_ADDR: AtomicU32 = AtomicU32::new(0);
static DMA_TIMING_FRAME_ADDR: AtomicU32 = AtomicU32::new(0);

/// 走査開始からのフレーム数 (CH2 完了割り込みで加算)
static FRAME_COUNT: AtomicU32 = AtomicU32::new(0);
static FRAME_WAKER: AtomicWaker = AtomicWaker::new();

// ============================================================
// BackBuffer: アプリが描く 400×96 の画面
// ============================================================

/// アプリケーションが描画するバックバッファ (400×96、**RGB565** の `u16`)。
///
/// v0.4.0 で RGB666 ワード (`u32`、153,600 B) から RGB565 (76,800 B) にした。空いた 76.8 KB を
/// `ticker` の写真の背景 (同じく RGB565) に使うため (docs/ticker.md「RAM」)。LCD は 18 bit なので
/// 垂直ブランキングのコピー ([`Display::present`]) が表引きで RGB666 の走査ワードへ広げる
/// (R/B の 5 bit は上位ビットの複製で 6 bit に、`ui::color::to_666` と同じ)。
///
/// `embedded-graphics` の `DrawTarget<Color = Rgb666>` も実装する (R/B の最下位ビットは捨てる)。
pub struct BackBuffer {
    /// 行優先 `[y * 400 + x]`、RGB565
    pub data: [u16; BACK_SIZE],
}

/// バックバッファの黒
pub const BACK_BLACK: u16 = 0;

/// RGB666 (各 0..=63) → バックバッファの RGB565
#[inline]
pub const fn rgb666_to_565(r: u8, g: u8, b: u8) -> u16 {
    ((r as u16 >> 1) << 11) | ((g as u16 & 0x3f) << 5) | (b as u16 >> 1)
}

impl BackBuffer {
    pub const fn new() -> Self {
        Self { data: [BACK_BLACK; BACK_SIZE] }
    }

    #[inline]
    pub fn set_pixel(&mut self, x: usize, y: usize, color: u16) {
        if x < BACK_WIDTH && y < BACK_HEIGHT {
            self.data[y * BACK_WIDTH + x] = color;
        }
    }

    #[inline]
    pub fn get_pixel(&self, x: usize, y: usize) -> u16 {
        if x < BACK_WIDTH && y < BACK_HEIGHT {
            self.data[y * BACK_WIDTH + x]
        } else {
            BACK_BLACK
        }
    }

    /// 行 y (0..96) の 400 画素
    #[inline]
    pub fn row(&self, y: usize) -> &[u16] {
        &self.data[y * BACK_WIDTH..(y + 1) * BACK_WIDTH]
    }

    /// 行 y (0..96) の 400 画素 (可変)
    #[inline]
    pub fn row_mut(&mut self, y: usize) -> &mut [u16] {
        &mut self.data[y * BACK_WIDTH..(y + 1) * BACK_WIDTH]
    }

    /// 全画面を指定色 (RGB565) で塗る
    pub fn clear(&mut self, color: u16) {
        self.data.fill(color);
    }
}

impl Default for BackBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl OriginDimensions for BackBuffer {
    fn size(&self) -> Size {
        Size::new(BACK_WIDTH as u32, BACK_HEIGHT as u32)
    }
}

#[inline]
fn to_565(color: Rgb666) -> u16 {
    rgb666_to_565(color.r(), color.g(), color.b())
}

impl DrawTarget for BackBuffer {
    type Color = Rgb666;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(coord, color) in pixels {
            if let Ok((x, y)) = <(u32, u32)>::try_from(coord)
                && (x as usize) < BACK_WIDTH
                && (y as usize) < BACK_HEIGHT
            {
                self.data[y as usize * BACK_WIDTH + x as usize] = to_565(color);
            }
        }
        Ok(())
    }

    fn fill_solid(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        let word = to_565(color);
        let clipped = area.intersection(&self.bounding_box());
        if clipped.size.width > 0 && clipped.size.height > 0 {
            let x_start = clipped.top_left.x as usize;
            let y_start = clipped.top_left.y as usize;
            let width = clipped.size.width as usize;
            for y in y_start..y_start + clipped.size.height as usize {
                let start = y * BACK_WIDTH + x_start;
                self.data[start..start + width].fill(word);
            }
        }
        Ok(())
    }

    fn clear(&mut self, color: Self::Color) -> Result<(), Self::Error> {
        self.data.fill(to_565(color));
        Ok(())
    }
}

// ============================================================
// RGB565 → 走査ワード (RGB666、ビット順反転込み) の表
// ============================================================

/// RGB565 の上位バイト (RRRRRGGG) の寄与: R の 6 bit と G の上位 3 bit
const fn lut_hi() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let (r6, g6, _) = crate::ui::color::to_666((i as u16) << 8);
        t[i] = rgb666(r6 as u32, g6 as u32, 0);
        i += 1;
    }
    t
}

/// RGB565 の下位バイト (GGGBBBBB) の寄与: G の下位 3 bit と B の 6 bit
const fn lut_lo() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let (_, g6, b6) = crate::ui::color::to_666(i as u16);
        t[i] = rgb666(0, g6 as u32, b6 as u32);
        i += 1;
    }
    t
}

/// 垂直ブランキングのコピーで引く表 (各 1 KB)。CPU が読むだけ (DMA は読まない) だが、
/// 所要時間を XIP キャッシュの当たり外れに左右されないよう SRAM (.data) に置く。
#[unsafe(link_section = ".data.lcd_lut_hi")]
static LUT_HI: [u32; 256] = lut_hi();
#[unsafe(link_section = ".data.lcd_lut_lo")]
static LUT_LO: [u32; 256] = lut_lo();

// 上位 / 下位バイトの寄与を OR すると 1 画素の走査ワードになる (ビット反転は各ビット独立なので分けてよい)。
// 全 65,536 色でコンパイル時に確かめる。
const _: () = {
    let hi = lut_hi();
    let lo = lut_lo();
    let mut c: u32 = 0;
    while c < 65536 {
        let (r, g, b) = crate::ui::color::to_666(c as u16);
        assert!(hi[(c >> 8) as usize] | lo[(c & 0xff) as usize] == rgb666(r as u32, g as u32, b as u32));
        c += 1;
    }
};

/// RGB565 の 1 画素 → 走査ワード
#[inline(always)]
fn scan_word(c: u16) -> u32 {
    LUT_HI[(c >> 8) as usize] | LUT_LO[(c & 0xff) as usize]
}

// ============================================================
// ペリフェラル一式
// ============================================================

/// LCD に使うペリフェラル (GP2〜GP22、PIO0、DMA CH0〜CH3)
pub struct DisplayPins {
    pub pio0: Peri<'static, PIO0>,
    pub pin2: Peri<'static, PIN_2>,
    pub pin3: Peri<'static, PIN_3>,
    pub pin4: Peri<'static, PIN_4>,
    pub pin5: Peri<'static, PIN_5>,
    pub pin6: Peri<'static, PIN_6>,
    pub pin7: Peri<'static, PIN_7>,
    pub pin8: Peri<'static, PIN_8>,
    pub pin9: Peri<'static, PIN_9>,
    pub pin10: Peri<'static, PIN_10>,
    pub pin11: Peri<'static, PIN_11>,
    pub pin12: Peri<'static, PIN_12>,
    pub pin13: Peri<'static, PIN_13>,
    pub pin14: Peri<'static, PIN_14>,
    pub pin15: Peri<'static, PIN_15>,
    pub pin16: Peri<'static, PIN_16>,
    pub pin17: Peri<'static, PIN_17>,
    pub pin18: Peri<'static, PIN_18>,
    pub pin19: Peri<'static, PIN_19>,
    pub pin20: Peri<'static, PIN_20>,
    pub pin21: Peri<'static, PIN_21>,
    pub pin22: Peri<'static, PIN_22>,
    pub dma_ch0: Peri<'static, DMA_CH0>,
    pub dma_ch1: Peri<'static, DMA_CH1>,
    pub dma_ch2: Peri<'static, DMA_CH2>,
    pub dma_ch3: Peri<'static, DMA_CH3>,
}

/// フレーム先頭 (CH2 完了) を受ける `DMA_IRQ_1` ハンドラ。
///
/// bin 側で `bind_interrupts!(struct Irqs { DMA_IRQ_1 => FrameIrqHandler; … })` に
/// 登録し、[`Display::start`] に渡す。走査そのものはこの割り込みに依存しない。
pub struct FrameIrqHandler;

impl Handler<DMA_IRQ_1> for FrameIrqHandler {
    unsafe fn on_interrupt() {
        let ints = pac::DMA.ints(1).read();
        pac::DMA.ints(1).write_value(ints);
        if ints & FRAME_START_CHANNEL_BIT != 0 {
            let frame = FRAME_COUNT.fetch_add(1, Ordering::Release).wrapping_add(1);
            FRAME_WAKER.wake();
            // ウォッチドッグの監視 (0.4.1〜、`ticker` が `supervisor::start` した後だけ動く)
            crate::supervisor::on_frame(frame);
        }
    }
}

// ============================================================
// Display
// ============================================================

/// LCD 表示のハンドル。`DisplayPins` (PIO0 を含む) を消費するので同時に 1 つしか作れない。
///
/// 使い方:
/// ```ignore
/// let mut display = Display::new(DisplayPins { … });
/// draw(display.back());          // 初期画面
/// display.start(Irqs);           // 初期画面をフロントへ写してから走査開始
/// loop {
///     draw(display.back());
///     display.present().await;   // 垂直ブランキングでフロントへ反映
/// }
/// ```
pub struct Display {
    /// `start()` でペリフェラルを消費するまで保持する
    pins: Option<DisplayPins>,
}

impl Display {
    pub fn new(pins: DisplayPins) -> Self {
        Self { pins: Some(pins) }
    }

    /// 走査を開始済みか
    pub fn is_running(&self) -> bool {
        self.pins.is_none()
    }

    /// 描画先のバックバッファ。走査中でも自由に描いてよい (画面には `present()` で反映)。
    pub fn back(&mut self) -> &mut BackBuffer {
        // Safety: BACK を触るのは Display (単一インスタンス) の &mut self 経由のみ。
        // DMA はフロントしか読まない。
        unsafe { &mut *addr_of_mut!(BACK) }
    }

    /// 走査開始からのフレーム数
    pub fn frame_count() -> u32 {
        FRAME_COUNT.load(Ordering::Acquire)
    }

    /// 走査中の現在ライン (0..113、CH0 の残り転送数から推定。DMA は FIFO 8 ワード分だけ先行)
    pub fn current_line() -> usize {
        let remaining = pac::DMA.ch(0).trans_count().read().count() as usize;
        FB_SIZE.saturating_sub(remaining) / LINE_WIDTH
    }

    /// バックバッファをフロントへ写してから LCD 走査を開始する。
    ///
    /// 2 回目以降の呼び出しは何もしない。
    pub fn start(
        &mut self,
        irqs: impl Binding<PIO0_IRQ_0, InterruptHandler<PIO0>> + Binding<DMA_IRQ_1, FrameIrqHandler>,
    ) {
        let Some(pins) = self.pins.take() else {
            return;
        };
        // DMA 未起動なのでフロントへの同期コピーは安全。
        copy_back_to_front();
        start_scanout(pins, irqs);
    }

    /// バックバッファの内容を次の垂直ブランキングでフロントへ反映する。
    ///
    /// 走査開始前なら即時コピーする。
    pub async fn present(&mut self) {
        if !self.is_running() {
            copy_back_to_front();
            return;
        }
        let mut misses = 0;
        loop {
            wait_frame_start().await;
            let line = Self::current_line();
            if line < PRESENT_DEADLINE_LINE || misses >= PRESENT_MAX_MISSES {
                if misses >= PRESENT_MAX_MISSES {
                    defmt::warn!("present: missed vblank {} times, copying mid-frame", misses);
                }
                copy_back_to_front();
                return;
            }
            misses += 1;
        }
    }
}

/// 次のフレーム先頭 (CH2 完了) まで待つ。描画しないタスク (SD の読み込みなど) が 1 フレームに
/// 1 回だけ動くための待ち合わせにも使える (走査開始前に呼ぶと永久に待つ)
pub async fn next_frame() {
    wait_frame_start().await
}

/// 次の CH2 完了 (フレーム先頭) まで待つ
async fn wait_frame_start() {
    let seen = FRAME_COUNT.load(Ordering::Acquire);
    poll_fn(|cx| {
        FRAME_WAKER.register(cx.waker());
        if FRAME_COUNT.load(Ordering::Acquire) != seen {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    })
    .await
}

/// BACK の 96 行を FRONT の表示行へ写す。可視開始位置 `VISIBLE_X_OFFSET` に置き、
/// 行の残り (左 106 + 右 3 ワード) は端の画素色で埋める。
///
/// RGB565 → 走査ワードの変換は 2 つの 256 語の表の OR (1 画素 ≈ 7 サイクル、1 行 ≈ 20〜25 µs @150 MHz)。
/// 走査は 1 行 ≈ 147 µs なので、ブランキング中に始めれば v0.3 までの単純コピー (≈5 µs/行) と同じく
/// 走査に追い越されない (全 96 行 ≈ 2.4 ms、ブランキング 16 行 ≈ 2.4 ms のうち先頭 12 行以内に開始)。
fn copy_back_to_front() {
    // Safety: BACK は Display の &mut self 経由でしか書かれず、この関数も
    // Display の &mut self からしか呼ばれない。FRONT を書くのはここだけで、
    // DMA は読み出しのみ。
    let back = unsafe { &*addr_of!(BACK) };
    let front = unsafe { &mut *addr_of_mut!(FRONT) };
    for y in 0..BACK_HEIGHT {
        let src = back.row(y);
        let row_start = (ACTIVE_Y_OFFSET + y) * LINE_WIDTH;
        let dst = &mut front.data[row_start..row_start + LINE_WIDTH];
        for (d, &c) in dst[VISIBLE_X_OFFSET..VISIBLE_X_OFFSET + BACK_WIDTH].iter_mut().zip(src) {
            *d = scan_word(c);
        }
        let first = scan_word(src[0]);
        let last = scan_word(src[BACK_WIDTH - 1]);
        dst[..VISIBLE_X_OFFSET].fill(first);
        dst[VISIBLE_X_OFFSET + BACK_WIDTH..].fill(last);
    }
    compiler_fence(Ordering::SeqCst);
}

/// PIO0 SM0/SM1 と DMA CH0〜CH3 を設定し、走査を開始する。
fn start_scanout(
    pins: DisplayPins,
    irqs: impl Binding<PIO0_IRQ_0, InterruptHandler<PIO0>> + Binding<DMA_IRQ_1, FrameIrqHandler>,
) {
    // DMA CH0〜CH3 の所有権は PAC 直接操作のため embassy API では使わない。
    // Peri を drop しても他のコードから取得できないので、ここで消費して構わない。
    let _dma_ch0 = pins.dma_ch0;
    let _dma_ch1 = pins.dma_ch1;
    let _dma_ch2 = pins.dma_ch2;
    let _dma_ch3 = pins.dma_ch3;

    // === SM0: ピクセル出力 + NCLK (sideset) ===
    let prg_pixel = pio_asm!(
        ".side_set 1",
        ".wrap_target",
        "    out pins, 18  side 1", // データ出力 + NCLK HIGH（セットアップ期間）
        "    nop           side 0", // NCLK LOW（立ち下がりでLCDサンプル）
        ".wrap",
    );

    // === SM1: HSYNC/VSYNC タイミング ===
    let prg_timing = pio_asm!(
        ".wrap_target",
        "    set pins, 3",
        "    set pins, 2",
        "    pull block",
        "    mov x, osr",
        "rest_v0:",
        "    jmp x-- rest_v0",
        "    pull block",
        "    mov y, osr",
        "normal_line:",
        "    set pins, 1",
        "    set pins, 0",
        "    pull block",
        "    mov x, osr",
        "rest_v1:",
        "    jmp x-- rest_v1",
        "    jmp y-- normal_line",
        ".wrap",
    );

    // `Pio` は関数を抜けても **drop しない** (末尾の `mem::forget`)。
    // embassy-rp の `StateMachine::drop` は SM を無効化し、`Common`/`StateMachine`
    // の最後の drop で PIO が使っていた全 GPIO の FUNCSEL を NULL に戻す
    // (embassy-rp 0.9 `pio/mod.rs` `on_pio_drop`)。以前は display_task が
    // `pending().await` で永久に保持していたが、同期関数に移した際に関数末尾で
    // drop され、NCLK/HSYNC/VSYNC/RGB が全て切り離されて白画面になっていた。
    let mut pio = Pio::new(pins.pio0, irqs);
    let common = &mut pio.common;
    let sm0 = &mut pio.sm0;
    let sm1 = &mut pio.sm1;

    let pin2 = common.make_pio_pin(pins.pin2);
    let pin3 = common.make_pio_pin(pins.pin3);
    let pin4 = common.make_pio_pin(pins.pin4);
    let pin5 = common.make_pio_pin(pins.pin5);
    let pin6 = common.make_pio_pin(pins.pin6);
    let pin7 = common.make_pio_pin(pins.pin7);
    let pin8 = common.make_pio_pin(pins.pin8);
    let pin9 = common.make_pio_pin(pins.pin9);
    let pin10 = common.make_pio_pin(pins.pin10);
    let pin11 = common.make_pio_pin(pins.pin11);
    let pin12 = common.make_pio_pin(pins.pin12);
    let pin13 = common.make_pio_pin(pins.pin13);
    let pin14 = common.make_pio_pin(pins.pin14);
    let pin15 = common.make_pio_pin(pins.pin15);
    let pin16 = common.make_pio_pin(pins.pin16);
    let pin17 = common.make_pio_pin(pins.pin17);
    let pin18 = common.make_pio_pin(pins.pin18);
    let pin19 = common.make_pio_pin(pins.pin19);
    let nclk_pin = common.make_pio_pin(pins.pin20);
    let hsync_pin = common.make_pio_pin(pins.pin21);
    let vsync_pin = common.make_pio_pin(pins.pin22);

    sm0.set_pin_dirs(
        Direction::Out,
        &[
            &pin2, &pin3, &pin4, &pin5, &pin6, &pin7, &pin8, &pin9, &pin10, &pin11, &pin12, &pin13,
            &pin14, &pin15, &pin16, &pin17, &pin18, &pin19, &nclk_pin,
        ],
    );
    sm1.set_pin_dirs(Direction::Out, &[&hsync_pin, &vsync_pin]);

    let loaded_pixel = common.load_program(&prg_pixel.program);
    let mut cfg0 = Config::default();
    cfg0.use_program(&loaded_pixel, &[&nclk_pin]);
    cfg0.set_out_pins(&[
        &pin2, &pin3, &pin4, &pin5, &pin6, &pin7, &pin8, &pin9, &pin10, &pin11, &pin12, &pin13,
        &pin14, &pin15, &pin16, &pin17, &pin18, &pin19,
    ]);
    cfg0.shift_out = ShiftConfig {
        auto_fill: true,
        threshold: 18,
        direction: ShiftDirection::Right,
    };
    cfg0.clock_divider =
        FixedU32::<U8>::from_bits((PIO_CLK_DIV_INT as u32) << 8 | PIO_CLK_DIV_FRAC as u32);
    cfg0.fifo_join = FifoJoin::TxOnly;

    let loaded_timing = common.load_program(&prg_timing.program);
    let mut cfg1 = Config::default();
    cfg1.use_program(&loaded_timing, &[]);
    cfg1.set_set_pins(&[&hsync_pin, &vsync_pin]);
    cfg1.clock_divider = FixedU32::<U8>::from_bits(SM1_CLK_DIV_BITS);
    cfg1.fifo_join = FifoJoin::TxOnly;

    sm0.set_config(&cfg0);
    sm1.set_config(&cfg1);

    common.apply_sm_batch(|batch| {
        batch.set_enable(sm0, true);
        batch.set_enable(sm1, true);
    });

    let sm0_txf_addr = pac::PIO0.txf(0).as_ptr() as u32;
    let sm1_txf_addr = pac::PIO0.txf(1).as_ptr() as u32;

    let frame_addr = addr_of!(FRONT) as *const u32 as u32;
    let timing_addr = SM1_FRAME_DATA.as_ptr() as u32;
    debug_assert!(
        (0x2000_0000..0x2009_0000).contains(&timing_addr),
        "SM1_FRAME_DATA must live in SRAM"
    );
    DMA_PIXEL_FRAME_ADDR.store(frame_addr, Ordering::SeqCst);
    DMA_TIMING_FRAME_ADDR.store(timing_addr, Ordering::SeqCst);

    let dma = pac::DMA;
    // embassy の DMA_IRQ_0 ハンドラ (全チャネルの INTE0 を有効にしている) に
    // CH0〜CH3 の完了を通知しない。フレーム先頭 (CH2 完了) だけ DMA_IRQ_1 へ流す。
    dma.inte(0).write_value(dma.inte(0).read() & !0b1111);
    dma.intr(0).write_value(0b1111);
    dma.inte(1).write_value(dma.inte(1).read() | FRAME_START_CHANNEL_BIT);
    dma.ints(1).write_value(FRAME_START_CHANNEL_BIT);
    embassy_rp::interrupt::DMA_IRQ_1.set_priority(embassy_rp::interrupt::Priority::P3);
    // Safety: ハンドラは bind_interrupts! で登録済み (Binding 制約)。
    unsafe { embassy_rp::interrupt::DMA_IRQ_1.enable() };

    // --- CH0 (SM0: ピクセルデータ) ---
    let ch0 = dma.ch(0);
    ch0.write_addr().write_value(sm0_txf_addr);
    {
        let mut ctrl = CtrlTrig(0);
        ctrl.set_en(true);
        ctrl.set_high_priority(true);
        ctrl.set_data_size(DataSize::SIZE_WORD);
        ctrl.set_incr_read(true);
        ctrl.set_incr_write(false);
        ctrl.set_treq_sel(TreqSel::PIO0_TX0);
        ctrl.set_chain_to(2);
        ch0.al1_ctrl().write_value(ctrl.0);
    }

    // --- CH1 (SM1: HSYNC/VSYNC タイミング) ---
    let ch1 = dma.ch(1);
    ch1.write_addr().write_value(sm1_txf_addr);
    {
        let mut ctrl = CtrlTrig(0);
        ctrl.set_en(true);
        ctrl.set_high_priority(true);
        ctrl.set_data_size(DataSize::SIZE_WORD);
        ctrl.set_incr_read(true);
        ctrl.set_incr_write(false);
        ctrl.set_treq_sel(TreqSel::PIO0_TX1);
        ctrl.set_chain_to(3);
        ch1.al1_ctrl().write_value(ctrl.0);
    }

    // CH2: ピクセルフレーム先頭アドレスを CH0 のトリガー別名へ 1 ワード転送
    let ch2 = dma.ch(2);
    ch2.read_addr()
        .write_value(DMA_PIXEL_FRAME_ADDR.as_ptr() as u32);
    ch2.write_addr()
        .write_value(ch0.al3_read_addr_trig().as_ptr() as u32);
    ch2.trans_count().write(|w| w.set_count(1));
    {
        let mut ctrl = CtrlTrig(0);
        ctrl.set_en(true);
        ctrl.set_data_size(DataSize::SIZE_WORD);
        ctrl.set_incr_read(false);
        ctrl.set_incr_write(false);
        ctrl.set_treq_sel(TreqSel::PERMANENT);
        ctrl.set_chain_to(2);
        ch2.al1_ctrl().write_value(ctrl.0);
    }

    // CH3: 同期フレーム先頭アドレスを CH1 のトリガー別名へ 1 ワード転送
    let ch3 = dma.ch(3);
    ch3.read_addr()
        .write_value(DMA_TIMING_FRAME_ADDR.as_ptr() as u32);
    ch3.write_addr()
        .write_value(ch1.al3_read_addr_trig().as_ptr() as u32);
    ch3.trans_count().write(|w| w.set_count(1));
    {
        let mut ctrl = CtrlTrig(0);
        ctrl.set_en(true);
        ctrl.set_data_size(DataSize::SIZE_WORD);
        ctrl.set_incr_read(false);
        ctrl.set_incr_write(false);
        ctrl.set_treq_sel(TreqSel::PERMANENT);
        ctrl.set_chain_to(3);
        ch3.al1_ctrl().write_value(ctrl.0);
    }

    // 初回フレームは CH0/CH1 を同時起動。以後は CH2/CH3 チェインで連続供給。
    cortex_m::interrupt::free(|_| {
        ch0.read_addr().write_value(frame_addr);
        ch0.trans_count().write(|w| w.set_count(FB_SIZE as u32));

        ch1.read_addr().write_value(timing_addr);
        ch1.trans_count()
            .write(|w| w.set_count(SM1_FRAME_SIZE as u32));

        dma.multi_chan_trigger().write(|w| w.set_multi_chan_trigger(0b11));
    });

    // 走査は電源を切るまで続くので PIO0 の所有権を解放しない (上記コメント参照)。
    // (PIO 用 `Pin` と `LoadedProgram` には Drop 処理が無いので、そのまま解放してよい)
    core::mem::forget(pio);
}
