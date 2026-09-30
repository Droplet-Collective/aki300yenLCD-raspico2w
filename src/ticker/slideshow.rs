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

use core::cell::RefCell;
use core::fmt::Write as _;
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
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

/// 1 フレームの間に SD を読んでよい時間 (これを超えた時点で次のフレームまで待つ。最低 1 ブロックは読む)
pub const READ_BUDGET: Duration = Duration::from_millis(5);

/// スライドショーの設定 (ticker.txt から)
pub struct SlideConfig {
    /// 切り替え間隔 (0 = 最初の 1 枚を出したら切り替えない)
    pub interval: Duration,
    /// `images=` の値 (空ならルートの *.BMP)
    pub images: String<{ config::IMAGES_MAX }>,
    pub layout: Layout,
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
        let n = file.read(&mut buf[done..]).map_err(|_| LoadError::Io("SD read error"))?;
        if n == 0 {
            return Err(LoadError::Bad("truncated"));
        }
        done += n;
    }
    Ok(())
}

/// `name` を読みながら [`BG`] を上書きする (暗い間に呼ぶ)。成功したら元画像の (幅, 高さ)
async fn load(volume_mgr: &SdVolumeManager, name: &str, layout: Layout) -> Result<(u32, u32), LoadError> {
    let volume = volume_mgr
        .open_volume(embedded_sdmmc::VolumeIdx(0))
        .map_err(|e| LoadError::Io(sdcard::volume_error(e)))?;
    let root = volume.open_root_dir().map_err(|_| LoadError::Io("root dir error"))?;
    let file = root
        .open_file_in_dir(name, embedded_sdmmc::Mode::ReadOnly)
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
        file.seek_from_start(offset).map_err(|_| LoadError::Bad("seek error"))?;
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
fn image_list(volume_mgr: &SdVolumeManager, cfg: &SlideConfig) -> Vec<String<12>, MAX_IMAGES> {
    let mut list: Vec<String<12>, MAX_IMAGES> = Vec::new();
    if !cfg.images.is_empty() {
        for name in config::image_names(&cfg.images) {
            let mut n: String<12> = String::new();
            let _ = n.push_str(name);
            let _ = list.push(n);
        }
        return list;
    }
    match sdcard::list_root_bmps::<MAX_IMAGES>(volume_mgr) {
        Ok(found) => list = found,
        Err(e) => report("*.BMP", e),
    }
    list
}

#[embassy_executor::task]
pub async fn slideshow_task(volume_mgr: SdVolumeManager, cfg: SlideConfig) {
    // 起動直後は描画 / Wi-Fi の初期化と最初の OTA 確認を先に進める (ルートの走査もその後)
    Timer::after(Duration::from_millis(1500)).await;
    while !START.load(Ordering::Relaxed) && Instant::now() < cfg.start_by {
        Timer::after(Duration::from_millis(250)).await;
    }
    let list = image_list(&volume_mgr, &cfg);
    defmt::info!("slideshow: {} image(s), interval {} s", list.len(), cfg.interval.as_secs());
    if list.is_empty() {
        FIRST_DONE.store(true, Ordering::Relaxed);
        return; // グラデーションのまま
    }
    let mut fast = cfg.sd_fast;
    sdcard::set_fast(&volume_mgr, fast);

    let mut index = 0usize;
    let mut shown: Option<usize> = None;
    let mut failures = 0usize;
    loop {
        wait_unpaused().await;
        let name = list[index].as_str();
        fade(true).await;
        LEVEL.store(slide::LOADING_LEVEL, Ordering::Relaxed);
        let started = Instant::now();
        if cfg.crash_on_first && !FIRST_DONE.load(Ordering::Relaxed) {
            panic!("debug_crash=slideshow");
        }
        let result = loop {
            let result = load(&volume_mgr, name, cfg.layout).await;
            if let Err(LoadError::Io(_)) = result
                && fast
            {
                // 速い読み出しで読み誤った: 以後は低速で読み直す
                defmt::warn!("slideshow: {}: read error at fast SD clock, falling back to slow", name);
                fast = false;
                sdcard::set_fast(&volume_mgr, false);
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
                fill_default(cfg.layout);
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
            Timer::after(Duration::from_secs(60)).await;
            continue;
        }
        if shown.is_some() && (list.len() == 1 || cfg.interval.as_ticks() == 0) {
            defmt::info!("slideshow: single image / slide=0, not changing any more");
            return;
        }
        if shown.is_some() {
            Timer::after(cfg.interval).await;
        }
    }
}
