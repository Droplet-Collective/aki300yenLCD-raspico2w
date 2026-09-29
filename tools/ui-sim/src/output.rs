//! 描画結果を PNG / GIF に書く。背景 BMP はファームウェアと同じ `ui::bmp::Resampler` で読む

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use crate::scenario::Scenario;
use crate::ui::bmp::{BmpInfo, HEADER_LEN, Resampler};
use crate::ui::canvas::Canvas;
use crate::ui::color;
use crate::ui::screen::{self, Layout};
use crate::ui::{HEIGHT, PIXELS, WIDTH, background, slide};

/// 画面 1 枚 (RGB565)
pub type Frame = Vec<u16>;

/// BMP の読み込み。ファームウェアと同じく 512 バイトずつ `push` する (分割の境界の確認を兼ねる)。
/// `rows_limit` 行 (ファイルの並び順) で止めると、読み込み途中の背景になる
pub struct BmpLoader {
    data: Vec<u8>,
    resampler: Box<Resampler>,
}

impl BmpLoader {
    pub fn open(path: &Path) -> Result<Self, String> {
        let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let head = &data[..data.len().min(HEADER_LEN)];
        let info = BmpInfo::parse(head, data.len() as u32).map_err(|e| format!("{}: {e}", path.display()))?;
        let resampler = Box::new(Resampler::new(info));
        let (cx, cy, cw, ch) = resampler.crop();
        eprintln!(
            "{}: {}x{} {:?} {} → crop ({cx},{cy}) {cw}x{ch} → 400x96",
            path.display(),
            info.width,
            info.height,
            info.format,
            if info.top_down { "top-down" } else { "bottom-up" }
        );
        Ok(Self { data, resampler })
    }

    /// 次の 1 行を読んで `dst` へ。全部済んでいれば false
    pub fn step(&mut self, dst: &mut [u16]) -> bool {
        let Some((offset, len)) = self.resampler.next_row() else {
            return false;
        };
        let row = &self.data[offset as usize..(offset + len) as usize];
        for chunk in row.chunks(512) {
            self.resampler.push(chunk);
        }
        self.resampler.end_row(dst);
        true
    }

    pub fn progress(&self) -> u32 {
        self.resampler.progress()
    }

    pub fn done(&self) -> bool {
        self.resampler.done()
    }

    pub fn load_all(path: &Path, dst: &mut [u16]) -> Result<(), String> {
        let mut l = Self::open(path)?;
        while l.step(dst) {}
        Ok(())
    }
}

fn resolve(base: &Path, p: &str) -> PathBuf {
    let path = Path::new(p);
    if path.is_absolute() || path.exists() { path.to_path_buf() } else { base.join(path) }
}

/// シナリオの背景 (無ければグラデーション)
pub fn background_for(sc: &Scenario, base: &Path) -> Result<Frame, String> {
    let mut bg = vec![0u16; PIXELS];
    match &sc.background {
        Some(p) => BmpLoader::load_all(&resolve(base, p), &mut bg)?,
        None => background::fill_gradient(&mut bg),
    }
    screen::prepare_background(&mut bg, sc.layout());
    Ok(bg)
}

/// 1 枚描く
pub fn render_frame(sc: &Scenario, bg: &[u16], bg_level: u8, layout: Layout, extra_secs: u32, scroll_x: i32) -> Frame {
    let mut px = vec![0u16; PIXELS];
    let mut canvas = Canvas::new(&mut px);
    let view = sc.view(extra_secs, scroll_x);
    screen::render(&mut canvas, bg, bg_level, &view, layout);
    px
}

/// RGB565 → LCD が表示する RGB888 (拡大率 `scale`、最近傍)
pub fn to_rgb(frame: &[u16], scale: u32) -> (u32, u32, Vec<u8>) {
    let s = scale.max(1) as usize;
    let (w, h) = (WIDTH * s, HEIGHT * s);
    let mut out = vec![0u8; w * h * 3];
    for y in 0..h {
        for x in 0..w {
            let (r, g, b) = color::to_888(frame[(y / s) * WIDTH + x / s]);
            let i = (y * w + x) * 3;
            out[i] = r;
            out[i + 1] = g;
            out[i + 2] = b;
        }
    }
    (w as u32, h as u32, out)
}

pub fn write_png(path: &Path, w: u32, h: u32, rgb: &[u8]) -> Result<(), String> {
    let file = File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut enc = png::Encoder::new(BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::Best);
    let mut writer = enc.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(rgb).map_err(|e| e.to_string())?;
    eprintln!("wrote {} ({}x{})", path.display(), w, h);
    Ok(())
}

/// PNG (1 倍 / 3 倍) と GIF
pub fn render_scenario(sc: &Scenario, base: &Path, out: &Path, name: &str, gif: bool) -> Result<(), String> {
    let bg = background_for(sc, base)?;
    let layout = sc.layout();
    let frame = render_frame(sc, &bg, sc.bg_level, layout, 0, sc.scroll_x);
    for scale in [1, 3] {
        let (w, h, rgb) = to_rgb(&frame, scale);
        let file = if scale == 1 { format!("{name}.png") } else { format!("{name}@{scale}x.png") };
        write_png(&out.join(file), w, h, &rgb)?;
    }
    if gif {
        write_animation(sc, base, bg, &out.join(format!("{name}.gif")))?;
    }
    Ok(())
}

/// 流れる文字とスライドの切り替え (次の背景があれば) を GIF に
fn write_animation(sc: &Scenario, base: &Path, mut bg: Frame, path: &Path) -> Result<(), String> {
    let a = &sc.animation;
    let layout = sc.layout();
    let frame_ms = a.frame_ms.max(20) / 10 * 10;
    let frames = a.duration_ms / frame_ms;
    let text_w = crate::font::shinonome::text_width(&sc.message) as i32;
    let (_, scroll_w) = layout.scroll_area(sc.status.expanded);
    let mut next = match &sc.next_background {
        Some(p) => Some(BmpLoader::open(&resolve(base, p))?),
        None => None,
    };
    let fade_out_end = a.transition_at_ms + slide::FADE_OUT_MS;
    let load_end = fade_out_end + a.load_ms;
    let mut rows_total = 0u32;
    let mut frames_rgb: Vec<Frame> = Vec::new();
    // LCD のフレーム番号で流れる文字を進める (ファームウェアと同じ: 帯の右端から左へ、出切ったら右端へ)
    let mut scroll_x = sc.scroll_x;
    let mut lcd_frame_prev = 0u64;
    for i in 0..frames {
        let t = i * frame_ms;
        let lcd_frame = t as u64 * a.lcd_hz as u64 / 1000;
        for _ in lcd_frame_prev..lcd_frame {
            if text_w > 0 {
                scroll_x -= a.scroll_px.max(1);
                if scroll_x + text_w < 0 {
                    scroll_x = scroll_w;
                }
            }
        }
        lcd_frame_prev = lcd_frame;
        let level = match &mut next {
            Some(loader) if t >= a.transition_at_ms => {
                if t < fade_out_end {
                    slide::fade_out_level(t - a.transition_at_ms)
                } else if t < load_end {
                    // 読み込みの進みに合わせて行を足す (実機は SD から 1 ブロックずつ)
                    if rows_total == 0 {
                        rows_total = loader_rows(loader);
                    }
                    let want = (t - fade_out_end + frame_ms) as u64 * rows_total as u64 / a.load_ms.max(1) as u64;
                    while (loader.progress() as u64 * rows_total as u64 / 256) < want && loader.step(&mut bg) {}
                    slide::LOADING_LEVEL
                } else {
                    if !loader.done() {
                        while loader.step(&mut bg) {}
                        screen::prepare_background(&mut bg, layout);
                    }
                    slide::fade_in_level(t - load_end)
                }
            }
            _ => 32.min(sc.bg_level),
        };
        frames_rgb.push(render_frame(sc, &bg, level, layout, t / 1000, scroll_x));
    }
    write_gif(path, &frames_rgb, a.scale, frame_ms)
}

fn loader_rows(loader: &BmpLoader) -> u32 {
    let (_, _, _, h) = loader.resampler.crop();
    h
}

/// 差分だけの GIF (変わった矩形だけを各コマに書く。色はコマごとに 256 色へ減色)
fn write_gif(path: &Path, frames: &[Frame], scale: u32, frame_ms: u32) -> Result<(), String> {
    let s = scale.max(1);
    let (w, h) = (WIDTH as u32 * s, HEIGHT as u32 * s);
    let file = File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut enc = gif::Encoder::new(BufWriter::new(file), w as u16, h as u16, &[]).map_err(|e| e.to_string())?;
    enc.set_repeat(gif::Repeat::Infinite).map_err(|e| e.to_string())?;
    let mut prev: Option<&Frame> = None;
    let mut pending_delay = 0u16;
    for (i, f) in frames.iter().enumerate() {
        // 前のコマと違う範囲
        let (mut x0, mut y0, mut x1, mut y1) = (WIDTH, HEIGHT, 0usize, 0usize);
        match prev {
            None => (x0, y0, x1, y1) = (0, 0, WIDTH, HEIGHT),
            Some(p) => {
                for y in 0..HEIGHT {
                    for x in 0..WIDTH {
                        if p[y * WIDTH + x] != f[y * WIDTH + x] {
                            x0 = x0.min(x);
                            y0 = y0.min(y);
                            x1 = x1.max(x + 1);
                            y1 = y1.max(y + 1);
                        }
                    }
                }
            }
        }
        pending_delay += (frame_ms / 10) as u16;
        if x1 <= x0 && i + 1 < frames.len() {
            continue; // 変化なし: 次のコマに時間を足す
        }
        if x1 <= x0 {
            (x0, y0, x1, y1) = (0, 0, 1, 1);
        }
        let (rw, rh) = ((x1 - x0) as u32 * s, (y1 - y0) as u32 * s);
        let mut rgba = Vec::with_capacity((rw * rh * 4) as usize);
        for y in 0..rh {
            for x in 0..rw {
                let sx = x0 + (x / s) as usize;
                let sy = y0 + (y / s) as usize;
                let (r, g, b) = color::to_888(f[sy * WIDTH + sx]);
                rgba.extend_from_slice(&[r, g, b, 255]);
            }
        }
        let mut frame = gif::Frame::from_rgba_speed(rw as u16, rh as u16, &mut rgba, 10);
        frame.left = (x0 as u32 * s) as u16;
        frame.top = (y0 as u32 * s) as u16;
        frame.delay = pending_delay;
        frame.dispose = gif::DisposalMethod::Keep;
        pending_delay = 0;
        enc.write_frame(&frame).map_err(|e| e.to_string())?;
        prev = Some(f);
    }
    drop(enc);
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    eprintln!("wrote {} ({} frames, {} KB)", path.display(), frames.len(), size / 1024);
    Ok(())
}

/// レイアウト 3 種 × 背景 (--bg、無ければシナリオの背景) の比較表 (各 2 倍、見出し付き)
pub fn render_sheet(sc: &Scenario, base: &Path, bgs: &[PathBuf], out: &Path, name: &str) -> Result<(), String> {
    let layouts = [Layout::Glass, Layout::Dock, Layout::Classic];
    let mut backgrounds: Vec<(String, Frame)> = Vec::new();
    if bgs.is_empty() {
        let mut bg = vec![0u16; PIXELS];
        match &sc.background {
            Some(p) => BmpLoader::load_all(&resolve(base, p), &mut bg)?,
            None => background::fill_gradient(&mut bg),
        }
        backgrounds.push(("scenario".into(), bg));
    }
    for p in bgs {
        let mut bg = vec![0u16; PIXELS];
        if p.as_os_str() == "gradient" {
            background::fill_gradient(&mut bg);
        } else {
            BmpLoader::load_all(p, &mut bg)?;
        }
        let label = p.file_name().map_or("?".into(), |n| n.to_string_lossy().into_owned());
        backgrounds.push((label, bg));
    }
    let scale = 2usize;
    let (cw, ch) = (WIDTH * scale, HEIGHT * scale);
    let gap = 12usize;
    let label_h = 18usize;
    let sheet_w = backgrounds.len() * cw + (backgrounds.len() + 1) * gap;
    let sheet_h = layouts.len() * (ch + label_h) + (layouts.len() + 1) * gap;
    let mut sheet = vec![32u8; sheet_w * sheet_h * 3];
    for (li, layout) in layouts.iter().enumerate() {
        for (bi, (label, raw)) in backgrounds.iter().enumerate() {
            let mut bg = raw.clone();
            screen::prepare_background(&mut bg, *layout);
            let frame = render_frame(sc, &bg, sc.bg_level, *layout, 0, sc.scroll_x);
            let (_, _, rgb) = to_rgb(&frame, scale as u32);
            let ox = gap + bi * (cw + gap);
            let oy = gap + li * (ch + label_h + gap) + label_h;
            for y in 0..ch {
                let dst = ((oy + y) * sheet_w + ox) * 3;
                sheet[dst..dst + cw * 3].copy_from_slice(&rgb[y * cw * 3..(y + 1) * cw * 3]);
            }
            draw_label(&mut sheet, sheet_w, ox, oy - label_h + 3, &format!("{} / {}", layout.name(), label));
        }
    }
    write_png(&out.join(format!("{name}.png")), sheet_w as u32, sheet_h as u32, &sheet)
}

/// 見出しの文字 (FONT_6X10 を 1 倍で。比較表の余白に書く)
fn draw_label(sheet: &mut [u8], sheet_w: usize, x: usize, y: usize, text: &str) {
    use embedded_graphics::mono_font::MonoTextStyle;
    use embedded_graphics::mono_font::ascii::FONT_6X10;
    use embedded_graphics::pixelcolor::Rgb888;
    use embedded_graphics::prelude::*;
    use embedded_graphics::text::{Baseline, Text};

    struct Target<'a> {
        buf: &'a mut [u8],
        w: usize,
        h: usize,
    }
    impl OriginDimensions for Target<'_> {
        fn size(&self) -> Size {
            Size::new(self.w as u32, self.h as u32)
        }
    }
    impl DrawTarget for Target<'_> {
        type Color = Rgb888;
        type Error = core::convert::Infallible;
        fn draw_iter<I: IntoIterator<Item = Pixel<Rgb888>>>(&mut self, pixels: I) -> Result<(), Self::Error> {
            for Pixel(p, c) in pixels {
                if p.x >= 0 && p.y >= 0 && (p.x as usize) < self.w && (p.y as usize) < self.h {
                    let i = (p.y as usize * self.w + p.x as usize) * 3;
                    self.buf[i..i + 3].copy_from_slice(&[c.r(), c.g(), c.b()]);
                }
            }
            Ok(())
        }
    }
    let h = sheet.len() / 3 / sheet_w;
    let mut t = Target { buf: sheet, w: sheet_w, h };
    let style = MonoTextStyle::new(&FONT_6X10, Rgb888::new(220, 220, 220));
    let _ = Text::with_baseline(text, Point::new(x as i32, y as i32), style, Baseline::Top).draw(&mut t);
}
