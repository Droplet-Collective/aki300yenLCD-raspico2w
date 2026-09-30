//! ティッカーの画面構成 (写真の背景 + 情報の重ね描き)。レイアウトは 3 種類 ([`Layout`])。
//!
//! 入力は表示する値だけを集めた [`View`] (文字列は呼び出し側が組み立てる)。ファームウェアは共有モデルから、
//! シミュレータはシナリオ (JSON) から作る。描画は毎フレーム全画面 (背景のコピー → 板 → 文字)。

use core::fmt::Write as _;

use heapless::String;

use super::aafont_data::{CLOCK, MEDIUM, SMALL};
use super::background;
use super::canvas::{Canvas, Clip, Panel};
use super::color::{Color, palette, rgb};
use super::icons;
use super::scroll::{self, Ink, Line};
use super::{HEIGHT, WIDTH};
use crate::font::shinonome;

/// 状態の色分け (`ota::app::Tone` と同じ段階)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Muted,
    Normal,
    Ok,
    Busy,
    Error,
}

/// 現地の日時
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Clock {
    pub year: i32,
    pub month: u8,
    pub day: u8,
    /// 0 = 日曜
    pub weekday: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WeatherView<'a> {
    pub temperature: f32,
    /// WMO 天気コード (アイコン)
    pub code: u8,
    /// 天気の日本語 (`weather::condition_ja`)
    pub condition: &'a str,
    pub max: Option<f32>,
    pub min: Option<f32>,
    pub rain_pct: Option<u8>,
}

/// 状態表示 (起動直後 / 異常時 / OTA 中は 3 行、平常時は小さな 1 行)
#[derive(Clone, Copy, Debug)]
pub struct StatusView<'a> {
    /// 3 行を出すか (規則は ticker 側。docs/ticker.md「状態表示」)
    pub expanded: bool,
    /// 行 1: 起動診断 / ticker.txt の注意 / `SSID IP | NTP .. | WX .. | MSG ..`
    pub line1: &'a str,
    pub line1_tone: Tone,
    /// 行 2: `ticker v0.4.0 via OTA` + ` slot B TBYB:...`
    pub ident_head: &'a str,
    pub ident_rest: &'a str,
    pub ident_tone: Tone,
    /// 行 3: `OTA: ...`
    pub ota: &'a str,
    pub ota_tone: Tone,
    pub progress: Option<(u32, u32)>,
    /// 平常時の小さな表示 (Wi-Fi の扇 / NTP / WX / MSG / 版数)
    pub wifi: Tone,
    pub ntp: Tone,
    pub wx: Tone,
    pub msg: Tone,
    /// `v0.4.0`
    pub version: &'a str,
}

/// 状態 3 行の行 1 に出す設定ページの案内 (0.5.0〜): 待ち受けを始めてから 1 分と、設定ページの
/// 「LCD にコードを表示」から 1 分。`url` は `http://192.168.x.y/`、`code` は 6 桁のアクセスコード。
/// ふだんの画面では流れる文字の中に出す (0.5.1〜、[`scroll`])
#[derive(Clone, Copy, Debug)]
pub struct Banner<'a> {
    pub url: &'a str,
    pub code: &'a str,
}

#[derive(Clone, Copy, Debug)]
pub struct View<'a> {
    /// NTP 同期前は None
    pub clock: Option<Clock>,
    pub place: &'a str,
    /// 取得前は None
    pub weather: Option<WeatherView<'a>>,
    /// 流れる文字 (文字 + 設定ページの URL とコード、[`scroll::ScrollText::line`])。空なら文字を描かない
    pub scroll: Line<'a>,
    /// 流れる文字の先頭の位置 (範囲の左端からの px、[`scroll::advance`])
    pub scroll_x: i32,
    pub status: StatusView<'a>,
    /// 状態 3 行の行 1 に出す設定ページの案内 (出す間だけ Some)
    pub banner: Option<Banner<'a>>,
}

/// 画面の構成
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// 左上に大きな時計 (板なし・影付き)、右上に天気のガラス板、下に流れる文字の帯 (右端に小さな状態)
    Glass,
    /// 上は写真と時計だけ、下 40 px のガラスの台に天気と流れる文字をまとめる
    Dock,
    /// 0.3.1 の配置のまま、写真を暗く敷く (比較用)
    Classic,
}

impl Layout {
    pub fn parse(name: &str) -> Option<Layout> {
        if name.eq_ignore_ascii_case("glass") {
            Some(Layout::Glass)
        } else if name.eq_ignore_ascii_case("dock") {
            Some(Layout::Dock)
        } else if name.eq_ignore_ascii_case("classic") {
            Some(Layout::Classic)
        } else {
            None
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Layout::Glass => "glass",
            Layout::Dock => "dock",
            Layout::Classic => "classic",
        }
    }

    /// 流れる文字の表示範囲 (x, 幅)。スクロール位置の折り返しに使う
    pub fn scroll_area(self, expanded: bool) -> (i32, i32) {
        match self {
            Layout::Glass => (GLASS_BAND_X + 5, GLASS_SCROLL_W),
            Layout::Dock => (DOCK_X + 6, WIDTH as i32 - 2 * DOCK_X - 12),
            Layout::Classic => {
                let _ = expanded;
                (1, WIDTH as i32 - 2)
            }
        }
    }
}

const WEEKDAYS: [&str; 7] = ["日", "月", "火", "水", "木", "金", "土"];

/// 写真の上に置く大きな文字の影の不透明度 (0..=32)
const SHADOW: u8 = 22;
const GLASS_PANEL: Panel = Panel {
    tint: palette::GLASS,
    alpha: 20,
    edge: 7,
    radius: 4,
};
const STATUS_PANEL: Panel = Panel {
    tint: palette::GLASS,
    alpha: 25,
    edge: 6,
    radius: 4,
};

pub fn tone_color(tone: Tone) -> Color {
    match tone {
        Tone::Muted => palette::MUTED,
        Tone::Normal => palette::OFF_WHITE,
        Tone::Ok => palette::GREEN,
        Tone::Busy => palette::YELLOW,
        Tone::Error => palette::RED,
    }
}

/// 小さな状態表示の色 (未取得は灰)
fn dot_color(tone: Tone) -> Color {
    match tone {
        Tone::Muted | Tone::Normal => palette::FAINT,
        other => tone_color(other),
    }
}

fn is_night(clock: Option<Clock>) -> bool {
    clock.is_some_and(|c| !(6..18).contains(&c.hour))
}

fn fmt_temp(out: &mut String<16>, v: Option<f32>) {
    match v {
        Some(v) => {
            let _ = write!(out, "{:.1}", v);
        }
        None => {
            let _ = out.push_str("--");
        }
    }
}

/// 背景 (明るさ `bg_level` 0..=32) と `view` を描く
pub fn render(canvas: &mut Canvas, bg: &[Color], bg_level: u8, view: &View, layout: Layout) {
    background::copy_dimmed(bg, canvas.pixels_mut(), bg_level);
    match layout {
        Layout::Glass => render_glass(canvas, view),
        Layout::Dock => render_dock(canvas, view),
        Layout::Classic => render_classic(canvas, view),
    }
    canvas.reset_clip();
}

// ============================================================
// 共通部品
// ============================================================

/// 大きな時計 `HH:MM` + 小さな `:SS`。戻り値は右端の x
fn draw_big_clock(c: &mut Canvas, clock: Option<Clock>, x: i32, y: i32) -> i32 {
    let mut text: String<16> = String::new();
    match clock {
        Some(t) => {
            let _ = write!(text, "{:02}:{:02}", t.hour, t.minute);
            let end = c.aa_text_shadow(&CLOCK, &text, x, y, palette::WHITE, 1, SHADOW);
            text.clear();
            let _ = write!(text, ":{:02}", t.second);
            // 秒はベースラインを揃える
            let sy = y + CLOCK.baseline as i32 - MEDIUM.baseline as i32;
            c.aa_text_shadow(&MEDIUM, &text, end + 1, sy, palette::SOFT, 1, SHADOW)
        }
        None => c.aa_text_shadow(&CLOCK, "--:--", x, y, palette::MUTED, 1, SHADOW),
    }
}

fn date_text(clock: Option<Clock>) -> String<32> {
    let mut text: String<32> = String::new();
    if let Some(t) = clock {
        let _ = write!(text, "{}月{}日({})", t.month, t.day, WEEKDAYS[t.weekday as usize % 7]);
    } else {
        let _ = text.push_str("時刻同期中…");
    }
    text
}

/// 最高 / 最低 / 降水確率の 1 行 (高さ 11 px)。戻り値は右端の x
fn draw_forecast(c: &mut Canvas, w: &WeatherView, x: i32, y: i32) -> i32 {
    let mut text: String<16> = String::new();
    let mut pen = x;
    c.icon_tinted(&icons::UP, pen, y + 4, palette::WARM);
    pen += 7;
    fmt_temp(&mut text, w.max);
    pen = c.aa_text(&SMALL, &text, pen, y, palette::WARM) + 6;
    c.icon_tinted(&icons::DOWN, pen, y + 4, palette::COOL);
    pen += 7;
    text.clear();
    fmt_temp(&mut text, w.min);
    pen = c.aa_text(&SMALL, &text, pen, y, palette::COOL) + 6;
    // 降水確率の錠剤
    text.clear();
    match w.rain_pct {
        Some(p) => {
            let _ = write!(text, "{}%", p);
        }
        None => {
            let _ = text.push_str("--%");
        }
    }
    let chip_w = 5 + 5 + 3 + SMALL.text_width(&text) + 5;
    c.pill(pen, y - 1, chip_w, 13, palette::AQUA, 7);
    c.icon_tinted(&icons::DROP, pen + 5, y + 1, palette::AQUA);
    c.aa_text(&SMALL, &text, pen + 5 + 5 + 3, y + 1, palette::AQUA);
    pen + chip_w
}

/// 平常時の小さな状態表示 (右端 `right`、上端 `y`、高さ 7 px): Wi-Fi の扇 NTP WX MSG v0.4.0。戻り値は左端の x
fn draw_status_chip(c: &mut Canvas, s: &StatusView, right: i32, y: i32) -> i32 {
    let labels = [("NTP", s.ntp), ("WX", s.wx), ("MSG", s.msg)];
    let mut width = 9 + 4;
    for (label, _) in labels {
        width += label.len() as i32 * 5 + 4;
    }
    width += s.version.len() as i32 * 5;
    let x0 = right - width;
    c.icon_tinted(&icons::WIFI, x0, y, dot_color(s.wifi));
    let mut pen = x0 + 9 + 4;
    for (label, tone) in labels {
        pen = c.tiny(label, pen, y, dot_color(tone)) + 4;
    }
    c.tiny(s.version, pen, y, palette::MUTED);
    x0
}

/// 状態 3 行の行 1 に出す案内 (ASCII、`FONT_6X10` の 66 桁以内)
pub fn banner_line(b: &Banner) -> String<80> {
    let mut line: String<80> = String::new();
    let _ = write!(line, "settings: {}  code {}", b.url, b.code);
    line
}

/// 3 行の状態表示 (板の中、上端 `y`、行の間隔 9 px、`FONT_6X10`)。案内があれば行 1 の代わりに出す
fn draw_status_lines(c: &mut Canvas, s: &StatusView, banner: Option<Banner>, x: i32, y: i32, width: i32) {
    match banner {
        Some(b) => {
            c.small(&banner_line(&b), x, y, palette::CYAN);
        }
        None => {
            c.small(s.line1, x, y, tone_color(s.line1_tone));
        }
    }
    let head_end = c.small(s.ident_head, x, y + 9, palette::MAGENTA);
    c.small(s.ident_rest, head_end, y + 9, tone_color(s.ident_tone));
    if let Some((done, total)) = s.progress {
        let filled = if total > 0 { (done as u64 * width as u64 / total as u64) as i32 } else { 0 };
        c.fill_rect(x, y + 18, width, 1, palette::FAINT);
        c.fill_rect(x, y + 18, filled, 1, palette::YELLOW);
    }
    c.small(s.ota, x, y + 18, tone_color(s.ota_tone));
}

/// 流れる文字の部分の色 (`text` は画面構成の文字の色)
fn ink_color(ink: Ink, text: Color) -> Color {
    match ink {
        Ink::Message => text,
        Ink::Sep => palette::ACCENT,
        Ink::Label => palette::SOFT,
        Ink::Value => palette::CYAN,
        Ink::Note => palette::MUTED,
    }
}

/// 流れる文字を範囲 (x, y, w) に描く (東雲 14 px)。設定の部分があれば `width` ごとに並べて切れ目なく流し、
/// 見えない部分は飛ばす (長い文字でも 1 フレームの手間を増やさない)。目立たせる間は設定の部分に薄い板を敷く
fn draw_scroll(c: &mut Canvas, view: &View, x: i32, y: i32, w: i32, color: Color) {
    let line = &view.scroll;
    if line.is_empty() {
        return;
    }
    c.set_clip(Clip::new(x, y - 1, w, shinonome::HEIGHT as i32 + 2));
    for origin in scroll::copies(line, view.scroll_x, w) {
        let base = x + origin;
        if let Some((hx, hw)) = line.highlight {
            c.pill(base + hx - 4, y - 1, hw + 8, shinonome::HEIGHT as i32 + 2, palette::CYAN, 9);
        }
        for span in line.spans {
            let sx = base + span.x;
            if sx >= x + w || sx + span.w <= x {
                continue;
            }
            let text = line.text.get(span.start as usize..span.end as usize).unwrap_or("");
            c.jp(text, sx, y, ink_color(span.ink, color));
        }
    }
    c.reset_clip();
}

// ============================================================
// Glass
// ============================================================

const GLASS_CARD_X: i32 = 234;
const GLASS_CARD_Y: i32 = 4;
const GLASS_CARD_W: i32 = WIDTH as i32 - GLASS_CARD_X - 4;
const GLASS_CARD_H: i32 = 54;
const GLASS_BAND_X: i32 = 4;
const GLASS_BAND_Y: i32 = 74;
const GLASS_BAND_W: i32 = WIDTH as i32 - 2 * GLASS_BAND_X;
const GLASS_BAND_H: i32 = 19;
/// 帯の右端に置く小さな状態表示の幅 (その左までが流れる文字)
const GLASS_CHIP_W: i32 = 118;
const GLASS_SCROLL_W: i32 = GLASS_BAND_W - 10 - GLASS_CHIP_W;

/// 左上から右下へ薄れていく暗がり (写真が明るくても板なしの時計と日付が読めるように)
fn scrim(c: &mut Canvas<'_>, w: i32, h: i32, max_alpha: u8) {
    for y in 0..h {
        let fy = (h - y) as u32;
        for x in 0..w {
            let a = max_alpha as u32 * (w - x) as u32 * fy / (w as u32 * h as u32);
            if a > 0 {
                c.blend_px(x, y, palette::BLACK, a as u8);
            }
        }
    }
}

/// 背景 (写真 / グラデーション) を読み終えたときに 1 回だけ焼き込む暗がり。
/// 毎フレームの合成を減らすため、レイアウトが写真の上に直接置く文字の下だけを暗くしておく
/// (Glass: 左上の時計と日付、Dock: 上の帯、Classic: 全面)。
pub fn prepare_background(bg: &mut [Color], layout: Layout) {
    let mut c = Canvas::new(bg);
    match layout {
        Layout::Glass => scrim(&mut c, 230, 70, 20),
        Layout::Dock => scrim(&mut c, 260, 52, 16),
        Layout::Classic => c.blend_rect(0, 0, WIDTH as i32, HEIGHT as i32, palette::BLACK, 20),
    }
}

fn render_glass(c: &mut Canvas, view: &View) {
    // --- 時計 (板なし) + 日付 ---
    draw_big_clock(c, view.clock, 8, 4);
    let date = date_text(view.clock);
    let date_color = if view.clock.is_some() { palette::OFF_WHITE } else { palette::MUTED };
    c.jp_shadow(&date, 10, 40, date_color, SHADOW);

    // --- 天気のガラス板 ---
    c.panel(GLASS_CARD_X, GLASS_CARD_Y, GLASS_CARD_W, GLASS_CARD_H, &GLASS_PANEL);
    let x = GLASS_CARD_X + 6;
    let right = GLASS_CARD_X + GLASS_CARD_W - 6;
    // 地名 (右上)
    let place_w = shinonome::text_width(view.place) as i32;
    c.icon_tinted(&icons::PIN, right - place_w - 8, GLASS_CARD_Y + 7, palette::MUTED);
    c.jp(view.place, right - place_w, GLASS_CARD_Y + 4, palette::SOFT);
    match view.weather {
        Some(w) => {
            c.icon(icons::weather_icon(w.code, is_night(view.clock)), x, GLASS_CARD_Y + 4);
            let mut text: String<16> = String::new();
            fmt_temp(&mut text, Some(w.temperature));
            let _ = text.push('°');
            c.aa_text(&MEDIUM, &text, x + 19, GLASS_CARD_Y + 4, palette::ACCENT);
            c.jp(w.condition, x, GLASS_CARD_Y + 22, palette::OFF_WHITE);
            draw_forecast(c, &w, x, GLASS_CARD_Y + 40);
        }
        None => {
            c.icon(&icons::UNKNOWN, x, GLASS_CARD_Y + 4);
            c.jp("天気取得中…", x, GLASS_CARD_Y + 22, palette::MUTED);
        }
    }

    // --- 下: 3 行の状態 (展開時) または 流れる文字の帯 + 小さな状態 ---
    if view.status.expanded {
        let y = GLASS_CARD_Y + GLASS_CARD_H + 3;
        c.panel(2, y, WIDTH as i32 - 4, HEIGHT as i32 - y - 1, &STATUS_PANEL);
        draw_status_lines(c, &view.status, view.banner, 6, y + 3, WIDTH as i32 - 12);
    } else {
        c.panel(GLASS_BAND_X, GLASS_BAND_Y, GLASS_BAND_W, GLASS_BAND_H, &GLASS_PANEL);
        let (sx, sw) = Layout::Glass.scroll_area(false);
        draw_scroll(c, view, sx, GLASS_BAND_Y + 3, sw, palette::OFF_WHITE);
        // 流れる文字と状態の間の仕切り
        let chip_x = GLASS_BAND_X + GLASS_BAND_W - GLASS_CHIP_W;
        c.blend_rect(chip_x - 1, GLASS_BAND_Y + 4, 1, GLASS_BAND_H - 8, palette::WHITE, 6);
        draw_status_chip(c, &view.status, GLASS_BAND_X + GLASS_BAND_W - 6, GLASS_BAND_Y + 6);
    }
}

// ============================================================
// Dock
// ============================================================

const DOCK_X: i32 = 3;
const DOCK_Y: i32 = 52;

fn render_dock(c: &mut Canvas, view: &View) {
    // --- 上: 時計 + 日付 / 地名 (写真の上、影付き) ---
    let end = draw_big_clock(c, view.clock, 8, 4);
    let date = date_text(view.clock);
    let date_color = if view.clock.is_some() { palette::OFF_WHITE } else { palette::MUTED };
    c.jp_shadow(&date, end + 10, 20, date_color, SHADOW);
    if !view.status.expanded {
        c.pill(WIDTH as i32 - 124, 2, 121, 13, palette::GLASS, 20);
        draw_status_chip(c, &view.status, WIDTH as i32 - 9, 5);
    }

    // --- 下のガラスの台 ---
    let h = HEIGHT as i32 - DOCK_Y - 3;
    if view.status.expanded {
        c.panel(DOCK_X, DOCK_Y + 4, WIDTH as i32 - 2 * DOCK_X, h - 4, &STATUS_PANEL);
        draw_status_lines(c, &view.status, view.banner, DOCK_X + 4, DOCK_Y + 7, WIDTH as i32 - 2 * DOCK_X - 8);
        return;
    }
    c.panel(DOCK_X, DOCK_Y, WIDTH as i32 - 2 * DOCK_X, h, &GLASS_PANEL);
    let x = DOCK_X + 6;
    let y = DOCK_Y + 3;
    match view.weather {
        Some(w) => {
            c.icon(icons::weather_icon(w.code, is_night(view.clock)), x, y);
            let mut text: String<16> = String::new();
            fmt_temp(&mut text, Some(w.temperature));
            let _ = text.push('°');
            let tx = c.aa_text(&MEDIUM, &text, x + 19, y, palette::ACCENT) + 6;
            let cx = c.jp(w.condition, tx, y + 1, palette::OFF_WHITE) + 10;
            let px = c.jp(view.place, cx, y + 1, palette::MUTED);
            // 予報は右寄せ
            let fw = forecast_width(&w);
            let fx = (WIDTH as i32 - DOCK_X - 6 - fw).max(px + 8);
            draw_forecast(c, &w, fx, y + 3);
        }
        None => {
            c.icon(&icons::UNKNOWN, x, y);
            let cx = c.jp("天気取得中…", x + 19, y + 1, palette::MUTED) + 10;
            c.jp(view.place, cx, y + 1, palette::MUTED);
        }
    }
    c.blend_rect(DOCK_X + 6, DOCK_Y + 20, WIDTH as i32 - 2 * DOCK_X - 12, 1, palette::WHITE, 5);
    let (sx, sw) = Layout::Dock.scroll_area(false);
    draw_scroll(c, view, sx, DOCK_Y + 23, sw, palette::OFF_WHITE);
}

fn forecast_width(w: &WeatherView) -> i32 {
    let mut text: String<16> = String::new();
    fmt_temp(&mut text, w.max);
    let mut width = 7 + SMALL.text_width(&text) + 6;
    text.clear();
    fmt_temp(&mut text, w.min);
    width += 7 + SMALL.text_width(&text) + 6;
    text.clear();
    match w.rain_pct {
        Some(p) => {
            let _ = write!(text, "{}%", p);
        }
        None => {
            let _ = text.push_str("--%");
        }
    }
    width + 5 + 5 + 3 + SMALL.text_width(&text) + 5
}

// ============================================================
// Classic (0.3.1 の配置 + 暗くした写真)
// ============================================================

fn render_classic(c: &mut Canvas, view: &View) {
    // 写真は prepare_background で全面を暗くしてある (文字は 0.3.1 と同じ色)
    let frame = rgb(96, 96, 96);
    c.fill_rect(0, 0, WIDTH as i32, 1, frame);
    c.fill_rect(0, HEIGHT as i32 - 1, WIDTH as i32, 1, frame);
    c.fill_rect(0, 0, 1, HEIGHT as i32, frame);
    c.fill_rect(WIDTH as i32 - 1, 0, 1, HEIGHT as i32, frame);

    draw_big_clock(c, view.clock, 6, 2);
    let date = date_text(view.clock);
    c.jp(&date, 196, 3, rgb(160, 224, 255));
    let mut x = c.jp(view.place, 196, 19, rgb(255, 200, 80)) + 8;
    match view.weather {
        Some(w) => {
            let mut text: String<16> = String::new();
            fmt_temp(&mut text, Some(w.temperature));
            let _ = text.push('℃');
            x = c.jp(&text, x, 19, rgb(255, 160, 0)) + 8;
            c.jp(w.condition, x, 19, palette::WHITE);
            let mut text: String<32> = String::new();
            let mut t: String<16> = String::new();
            fmt_temp(&mut t, w.max);
            let _ = write!(text, "最高 {}℃", t);
            let x = c.jp(&text, 6, 35, rgb(255, 112, 112)) + 12;
            text.clear();
            t.clear();
            fmt_temp(&mut t, w.min);
            let _ = write!(text, "最低 {}℃", t);
            let x = c.jp(&text, x, 35, rgb(112, 176, 255)) + 12;
            text.clear();
            match w.rain_pct {
                Some(p) => {
                    let _ = write!(text, "降水確率 {}%", p);
                }
                None => {
                    let _ = text.push_str("降水確率 --%");
                }
            }
            c.jp(&text, x, 35, rgb(80, 255, 255));
        }
        None => {
            c.jp("天気取得中…", x, 19, palette::MUTED);
        }
    }
    let dim = rgb(64, 64, 64);
    c.fill_rect(1, 50, WIDTH as i32 - 2, 1, dim);
    c.fill_rect(1, 67, WIDTH as i32 - 2, 1, dim);
    let (sx, sw) = Layout::Classic.scroll_area(false);
    draw_scroll(c, view, sx, 52, sw, rgb(255, 255, 192));
    draw_status_lines(c, &view.status, view.banner, 2, 68, WIDTH as i32 - 4);
}
