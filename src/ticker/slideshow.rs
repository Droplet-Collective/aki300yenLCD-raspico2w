//! 写真のスライドショー (v0.4.0〜): SD ルートの BMP を背景に順番に表示する
//!
//! - 背景は [`BG`] (400×96 RGB565 = 76,800 B) の 1 枚だけ。`render_task` は毎フレームこれを
//!   明るさ [`LEVEL`] でバックバッファへ写し、その上に時計や天気を描く (`ui::screen::render`)。
//! - 切り替え: 背景だけを暗くする (`ui::slide::FADE_OUT_MS`) → 次の BMP を読みながら行を上書き
//!   (暗いまま) → 明るく戻す (`FADE_IN_MS`)。時計 / 流れる文字は止めない。
//! - SD は GPIO SPI の同期処理なので、1 回に読むのは 512 バイト境界までの 1 ブロック分だけにし、
//!   `render_task` が 1 フレーム描き終えた合図 ([`FRAME_SLOT`]) のあとに [`READ_BUDGET`] まで読む。
//!   速い読み出し (`sdcard::set_fast`) で 1 ブロック ≈ 3 ms、低速なら ≈ 15 ms (その間はフレームが落ちる)。
//! - OTA の確認 / ダウンロード / 検証中 ([`PAUSE`] を main が立てる) は切り替えを始めず、読み込み中なら止まって待つ。
//! - 読み誤り (CRC など) が出たら低速に戻して同じ写真を 1 回だけ読み直す。
//! - BMP が 1 枚も無ければ既定のグラデーション (`ui::background::fill_gradient`) のまま。
//! - 0.5.0〜: SD は設定ページのサーバ (取得タスク) と共有する。SD を使う間は [`SD_LOCK`] を持ち、サーバが SD を
//!   使った直後 ([`HOLD_UNTIL_MS`] まで) は次の写真を読み始めない。切り替え間隔 / `images=` / 画面構成は
//!   [`LIVE`] から毎回読み、[`RELOAD`] が立ったら一覧を作り直してすぐ読み直す (設定ページの保存、写真の追加 / 削除)。

use core::cell::RefCell;
use core::fmt::Write as _;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};

use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::mutex::Mutex as AsyncMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer};
use heapless::{String, Vec};

use crate::sdcard::{self, SdFile, SdVolumeManager};
use crate::ticker::config::{self, MAX_IMAGES};
use crate::ui::bmp::{BmpInfo, HEADER_LEN, Resampler};
use crate::ui::screen::{self, Layout};
use crate::ui::{PIXELS, background, slide};

/// 背景 (RGB565)。`render_task` が読み、スライドショーが書く (同じ thread-mode executor)
pub static BG: Mutex<ThreadModeRawMutex, RefCell<[u16; PIXELS]>> = Mutex::new(RefCell::new([0; PIXELS]));
/// 背景の明るさ (0..=32)
pub static LEVEL: AtomicU8 = AtomicU8::new(32);
/// `render_task` が 1 フレーム描き終えるたびに立てる
pub static FRAME_SLOT: Signal<ThreadModeRawMutex, ()> = Signal::new();
/// main が OTA の確認〜検証の間だけ立てる (切り替え / 読み込みを止める)
pub static PAUSE: AtomicBool = AtomicBool::new(false);
/// 取得タスクが最初の OTA 確認を終えたら立てる。これが立つか [`SlideConfig::start_by`] を過ぎるまで SD の
/// 写真には触れない (0.4.1〜: 起動したら何より先に OTA 確認まで進み、壊れた版でも次の版で直せるように)
pub static START: AtomicBool = AtomicBool::new(false);
/// 最初の写真の読み込みを試し終えた (成否を問わない。写真が 1 枚も無い / SD が無いときも立つ)。
/// TBYB の buy 条件「機能の一巡」の 1 つ (0.4.2〜、`boot_policy::Round::slideshow`)
pub static FIRST_DONE: AtomicBool = AtomicBool::new(false);
/// 直近の読み込み失敗 (main が状態行 1 に出して消す)
pub static LAST_ERROR: Mutex<ThreadModeRawMutex, RefCell<Option<String<64>>>> = Mutex::new(RefCell::new(None));

/// SD を使う権利 (スライドショーと設定ページのサーバ。0.5.0〜)。GPIO SPI の同期処理そのものは同じ executor の上で
/// 重ならないが、embedded-sdmmc は開けるボリュームが 1 つなので、写真 1 枚の読み込み (await をまたぐ) の間は持ち続ける
pub static SD_LOCK: AsyncMutex<ThreadModeRawMutex, ()> = AsyncMutex::new(());
/// 設定ページのサーバが SD を使った直後はこの時刻 (起動からの ms) まで次の写真を読み始めない
/// (写真の一覧のサムネイルを続けて読む間に、スライドショーが 1 枚 2〜4 s の読み込みを挟まないように)
pub static HOLD_UNTIL_MS: AtomicU32 = AtomicU32::new(0);
/// 一覧を作り直してすぐ読み直す (設定ページが `images=` / 画面構成を変えた、写真を足した / 消した)
pub static RELOAD: AtomicBool = AtomicBool::new(false);

/// 保存するたびに変わる設定 (設定ページがその場で変える。0.5.0〜)
pub struct LiveSlide {
    /// 切り替え間隔 (秒、0 = 最初の 1 枚を出したら切り替えない)
    pub interval_secs: u16,
    /// `images=` の値 (空ならルートの *.BMP)
    pub images: String<{ config::IMAGES_MAX }>,
    pub layout: Layout,
}

pub static LIVE: Mutex<ThreadModeRawMutex, RefCell<LiveSlide>> = Mutex::new(RefCell::new(LiveSlide {
    interval_secs: config::DEFAULT_SLIDE_SECS,
    images: String::new(),
    layout: Layout::Glass,
}));

/// SD の 1 回の同期処理の期限 (ms)。描画も止まるので短く (1 ブロック ≈ 3〜15 ms)
pub const SD_OP_DEADLINE_MS: u64 = 2_000;

/// 1 フレームの間に SD を読んでよい時間 (これを超えた時点で次のフレームまで待つ。最低 1 ブロックは読む)
pub const READ_BUDGET: Duration = Duration::from_millis(5);

/// スライドショーの設定 (起動時に ticker.txt から。切り替え間隔 / 一覧 / 画面構成は [`LIVE`] に入れる)
pub struct SlideConfig {
    pub sd_fast: bool,
    /// [`START`] が立たなくてもこの時刻には始める (Wi-Fi が無い / つながらない場合)
    pub start_by: Instant,
    /// 試験用 (`debug_crash=slideshow`): 最初の写真を読み始めたら panic する
    pub crash_on_first: bool,
}

/// 既定のグラデーションを背景にする (起動時 / 写真が 1 枚も読めないとき)
pub fn fill_default(layout: Layout) {
    BG.lock(|bg| {
        let mut bg = bg.borrow_mut();
        background::fill_gradient(&mut bg[..]);
        screen::prepare_background(&mut bg[..], layout);
    });
}

fn report(name: &str, message: &str) {
    defmt::warn!("slideshow: {}: {}", name, message);
    let mut text: String<64> = String::new();
    let _ = write!(text, "BG {}: {}", name, message);
    LAST_ERROR.lock(|e| *e.borrow_mut() = Some(text));
}

async fn wait_frame() {
    FRAME_SLOT.wait().await;
}

async fn wait_unpaused() {
    while PAUSE.load(Ordering::Relaxed) {
        Timer::after(Duration::from_millis(500)).await;
    }
}

/// 背景を明るさ `from` → `to` の向きに `slide` の曲線で動かす
async fn fade(out: bool) {
    let start = Instant::now();
    loop {
        wait_frame().await;
        let ms = start.elapsed().as_millis() as u32;
        let (level, done) = if out {
            (slide::fade_out_level(ms), ms >= slide::FADE_OUT_MS)
        } else {
            (slide::fade_in_level(ms), ms >= slide::FADE_IN_MS)
        };
        LEVEL.store(level, Ordering::Relaxed);
        if done {
            return;
        }
    }
}

/// 読み込みの失敗理由
enum LoadError {
    /// ファイルが無い / BMP として読めない (次の写真へ)
    Bad(&'static str),
    /// SD の読み誤り (低速にして読み直す価値がある)
    Io(&'static str),
}

fn read_exact(file: &SdFile<'_>, buf: &mut [u8]) -> Result<(), LoadError> {
    let mut done = 0;
    while done < buf.len() {
        let n = sdcard::with_deadline(SD_OP_DEADLINE_MS, || file.read(&mut buf[done..])).map_err(|_| LoadError::Io("SD read error"))?;
        if n == 0 {
            return Err(LoadError::Bad("truncated"));
        }
        done += n;
    }
    Ok(())
}

/// `name` を読みながら [`BG`] を上書きする (暗い間に呼ぶ)。成功したら元画像の (幅, 高さ)
async fn load(volume_mgr: &SdVolumeManager, name: &str, layout: Layout) -> Result<(u32, u32), LoadError> {
    let _sd = SD_LOCK.lock().await;
    let volume = sdcard::with_deadline(SD_OP_DEADLINE_MS, || volume_mgr.open_volume(embedded_sdmmc::VolumeIdx(0)))
        .map_err(|e| LoadError::Io(sdcard::volume_error(e)))?;
    let root = volume.open_root_dir().map_err(|_| LoadError::Io("root dir error"))?;
    let file = sdcard::with_deadline(SD_OP_DEADLINE_MS, || root.open_file_in_dir(name, embedded_sdmmc::Mode::ReadOnly))
        .map_err(|_| LoadError::Bad("not found"))?;
    let len = file.length();
    let mut header = [0u8; HEADER_LEN];
    let head_len = (len as usize).min(HEADER_LEN);
    read_exact(&file, &mut header[..head_len])?;
    let info = BmpInfo::parse(&header[..head_len], len).map_err(LoadError::Bad)?;
    let mut resampler = Resampler::new(info);
    let (cx, cy, cw, ch) = resampler.crop();
    defmt::info!(
        "slideshow: {} {}x{} {} crop ({},{}) {}x{}",
        name,
        info.width,
        info.height,
        if info.top_down { "top-down" } else { "bottom-up" },
        cx,
        cy,
        cw,
        ch
    );
    let mut buf = [0u8; 512];
    let mut slot_start = Instant::now();
    while let Some((offset, row_len)) = resampler.next_row() {
        sdcard::with_deadline(SD_OP_DEADLINE_MS, || file.seek_from_start(offset)).map_err(|_| LoadError::Bad("seek error"))?;
        let mut pos = offset;
        let end = offset + row_len;
        while pos < end {
            // 1 回は 512 バイト境界まで (SD の 1 ブロック)
            let n = ((end - pos) as usize).min(512 - (pos as usize % 512));
            if slot_start.elapsed() >= READ_BUDGET {
                wait_unpaused().await;
                wait_frame().await;
                slot_start = Instant::now();
            }
            read_exact(&file, &mut buf[..n])?;
            resampler.push(&buf[..n]);
            pos += n as u32;
        }
        BG.lock(|bg| resampler.end_row(&mut bg.borrow_mut()[..]));
    }
    BG.lock(|bg| screen::prepare_background(&mut bg.borrow_mut()[..], layout));
    Ok((info.width, info.height))
}

/// 使う BMP の一覧 (`images=` か、ルートの *.BMP)
async fn image_list(volume_mgr: &SdVolumeManager) -> Vec<String<12>, MAX_IMAGES> {
    let mut list: Vec<String<12>, MAX_IMAGES> = Vec::new();
    let images_set = LIVE.lock(|l| {
        let l = l.borrow();
        for name in config::image_names(&l.images) {
            let mut n: String<12> = String::new();
            let _ = n.push_str(name);
            let _ = list.push(n);
        }
        !l.images.is_empty()
    });
    if images_set {
        return list;
    }
    let _sd = SD_LOCK.lock().await;
    match sdcard::with_deadline(SD_OP_DEADLINE_MS * 2, || sdcard::list_root_bmps::<MAX_IMAGES>(volume_mgr)) {
        Ok(found) => list = found,
        Err(e) => report("*.BMP", e),
    }
    list
}

fn live_layout() -> Layout {
    LIVE.lock(|l| l.borrow().layout)
}

fn live_interval() -> Duration {
    Duration::from_secs(u64::from(LIVE.lock(|l| l.borrow().interval_secs)))
}

/// 設定ページが SD を使っている間 (と、その直後) は待つ
async fn wait_web_quiet() {
    while (Instant::now().as_millis() as u32) < HOLD_UNTIL_MS.load(Ordering::Relaxed) {
        Timer::after(Duration::from_millis(250)).await;
    }
}

/// `d` だけ待つ。[`RELOAD`] が立ったらすぐ戻る (true)
async fn sleep_or_reload(d: Duration) -> bool {
    let until = Instant::now() + d;
    loop {
        if RELOAD.load(Ordering::Relaxed) {
            return true;
        }
        let now = Instant::now();
        if now >= until {
            return false;
        }
        Timer::after((until - now).min(Duration::from_millis(250))).await;
    }
}

#[embassy_executor::task]
pub async fn slideshow_task(volume_mgr: &'static SdVolumeManager, cfg: SlideConfig) {
    // 起動直後は描画 / Wi-Fi の初期化と最初の OTA 確認を先に進める (ルートの走査もその後)
    Timer::after(Duration::from_millis(1500)).await;
    while !START.load(Ordering::Relaxed) && Instant::now() < cfg.start_by {
        Timer::after(Duration::from_millis(250)).await;
    }
    let mut fast = cfg.sd_fast;
    sdcard::set_fast(volume_mgr, fast);
    'relist: loop {
        RELOAD.store(false, Ordering::Relaxed);
        wait_web_quiet().await;
        let list = image_list(volume_mgr).await;
        defmt::info!("slideshow: {} image(s), interval {} s", list.len(), live_interval().as_secs());
        if list.is_empty() {
            // グラデーションのまま (画面構成が変わったら焼き込みを合わせる)。写真が足されるのを待つ
            fill_default(live_layout());
            FIRST_DONE.store(true, Ordering::Relaxed);
            while !RELOAD.load(Ordering::Relaxed) {
                Timer::after(Duration::from_millis(500)).await;
            }
            continue 'relist;
        }

        let mut index = 0usize;
        let mut shown: Option<usize> = None;
        let mut failures = 0usize;
        loop {
            wait_unpaused().await;
            wait_web_quiet().await;
            if RELOAD.load(Ordering::Relaxed) {
                continue 'relist;
            }
            let name = list[index].as_str();
            fade(true).await;
            LEVEL.store(slide::LOADING_LEVEL, Ordering::Relaxed);
            let started = Instant::now();
            if cfg.crash_on_first && !FIRST_DONE.load(Ordering::Relaxed) {
                panic!("debug_crash=slideshow");
            }
            let layout = live_layout();
            let result = loop {
                let result = load(volume_mgr, name, layout).await;
                if let Err(LoadError::Io(_)) = result
                    && fast
                {
                    // 速い読み出しで読み誤った: 以後は低速で読み直す
                    defmt::warn!("slideshow: {}: read error at fast SD clock, falling back to slow", name);
                    fast = false;
                    sdcard::set_fast(volume_mgr, false);
                    continue;
                }
                break result;
            };
            match result {
                Ok((w, h)) => {
                    defmt::info!("slideshow: {} ({}x{}) loaded in {} ms", name, w, h, started.elapsed().as_millis());
                    shown = Some(index);
                    failures = 0;
                }
                Err(LoadError::Bad(e)) | Err(LoadError::Io(e)) => {
                    report(name, e);
                    failures += 1;
                    // 途中まで上書きした背景は、前の写真には戻せないのでグラデーションにする
                    fill_default(layout);
                    if shown == Some(index) {
                        shown = None;
                    }
                }
            }
            FIRST_DONE.store(true, Ordering::Relaxed);
            fade(false).await;
            LEVEL.store(32, Ordering::Relaxed);

            index = (index + 1) % list.len();
            if failures >= list.len() {
                // 全部失敗: しばらく待ってからやり直す
                failures = 0;
                if sleep_or_reload(Duration::from_secs(60)).await {
                    continue 'relist;
                }
                continue;
            }
            let interval = live_interval();
            if shown.is_some() && (list.len() == 1 || interval.as_ticks() == 0) {
                // 1 枚だけ / slide=0: もう切り替えない (設定ページの変更は待つ)
                while !RELOAD.load(Ordering::Relaxed) {
                    Timer::after(Duration::from_millis(500)).await;
                }
                continue 'relist;
            }
            if shown.is_some() && sleep_or_reload(interval).await {
                continue 'relist;
            }
        }
    }
}
