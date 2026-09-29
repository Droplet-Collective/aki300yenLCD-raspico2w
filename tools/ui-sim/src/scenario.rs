//! シナリオ (JSON): 画面に出す値と、アニメーションの条件。省略した項目は既定値 (docs/ui-sim.md)

use serde::Deserialize;

use crate::ui::screen::{Clock, StatusView, Tone, View, WeatherView};

#[derive(Deserialize, Clone)]
#[serde(default)]
pub struct Scenario {
    /// glass / dock / classic
    pub layout: String,
    /// 背景の BMP (シナリオのファイルからの相対パス)。null なら既定のグラデーション
    pub background: Option<String>,
    /// GIF のスライド切り替えで次に出す BMP (null なら切り替えを描かない)
    pub next_background: Option<String>,
    /// 背景の明るさ (0..=32)
    pub bg_level: u8,
    /// null なら「時刻同期中」
    pub clock: Option<ClockJson>,
    pub place: String,
    /// null なら「天気取得中」
    pub weather: Option<WeatherJson>,
    pub message: String,
    /// 流れる文字の位置 (帯の左端からの px)。PNG の静止画に使う
    pub scroll_x: i32,
    pub status: StatusJson,
    pub animation: AnimationJson,
}

#[derive(Deserialize, Clone, Copy)]
pub struct ClockJson {
    pub year: i32,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

#[derive(Deserialize, Clone, Copy)]
pub struct WeatherJson {
    pub temperature: f32,
    pub code: u8,
    pub max: Option<f32>,
    pub min: Option<f32>,
    pub rain_pct: Option<u8>,
}

#[derive(Deserialize, Clone)]
#[serde(default)]
pub struct StatusJson {
    pub expanded: bool,
    pub line1: String,
    pub line1_tone: String,
    pub ident_head: String,
    pub ident_rest: String,
    pub ident_tone: String,
    pub ota: String,
    pub ota_tone: String,
    pub progress: Option<(u32, u32)>,
    pub wifi: String,
    pub ntp: String,
    pub wx: String,
    pub msg: String,
    pub version: String,
}

#[derive(Deserialize, Clone)]
#[serde(default)]
pub struct AnimationJson {
    /// GIF の長さ (ms)
    pub duration_ms: u32,
    /// GIF の 1 コマ (ms、10 の倍数。GIF の遅延は 1/100 s 単位)
    pub frame_ms: u32,
    /// 流れる文字の速さ (px / LCD の 1 フレーム。ticker.txt の scroll)
    pub scroll_px: i32,
    /// LCD のフレーム周波数 (Hz)
    pub lcd_hz: u32,
    /// スライドの切り替えを始める時刻 (ms)。next_background が null なら無視
    pub transition_at_ms: u32,
    /// 次の写真の読み込みにかかる時間 (ms、SD の速さの見積り)
    pub load_ms: u32,
    /// GIF の拡大率
    pub scale: u32,
}

impl Default for Scenario {
    fn default() -> Self {
        Self {
            layout: "glass".into(),
            background: None,
            next_background: None,
            bg_level: 32,
            clock: Some(ClockJson {
                year: 2026,
                month: 9,
                day: 29,
                hour: 21,
                minute: 53,
                second: 44,
            }),
            place: "東京".into(),
            weather: Some(WeatherJson {
                temperature: 19.1,
                code: 2,
                max: Some(21.9),
                min: Some(18.6),
                rain_pct: Some(40),
            }),
            message: "こんにちは、おちょこさん。ネットワーク・ティッカー 0.4.0 が SD の写真の上に時計と天気を表示しています。".into(),
            scroll_x: 0,
            status: StatusJson::default(),
            animation: AnimationJson::default(),
        }
    }
}

impl Default for StatusJson {
    fn default() -> Self {
        Self {
            expanded: false,
            line1: "aterm-abff4a-g 192.168.200.130 | NTP ok s1 | WX ok | MSG ok".into(),
            line1_tone: "ok".into(),
            ident_head: "ticker v0.4.0 via OTA".into(),
            ident_rest: " slot B TBYB:bought OK".into(),
            ident_tone: "ok".into(),
            ota: "OTA: up to date (latest 0.4.0), next check in 45s".into(),
            ota_tone: "ok".into(),
            progress: None,
            wifi: "ok".into(),
            ntp: "ok".into(),
            wx: "ok".into(),
            msg: "ok".into(),
            version: "v0.4.0".into(),
        }
    }
}

impl Default for AnimationJson {
    fn default() -> Self {
        Self {
            duration_ms: 5000,
            frame_ms: 40,
            scroll_px: 1,
            lcd_hz: 60,
            transition_at_ms: 1200,
            load_ms: 900,
            scale: 2,
        }
    }
}

pub fn tone(name: &str) -> Tone {
    match name.to_ascii_lowercase().as_str() {
        "ok" | "green" => Tone::Ok,
        "busy" | "yellow" | "fetching" => Tone::Busy,
        "error" | "red" | "fail" => Tone::Error,
        "normal" | "white" => Tone::Normal,
        _ => Tone::Muted,
    }
}

/// (年, 月, 日) の曜日 (0 = 日曜)。Zeller の変形 (Sakamoto)
fn weekday(y: i32, m: u8, d: u8) -> u8 {
    const T: [i32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let y = if m < 3 { y - 1 } else { y };
    ((y + y / 4 - y / 100 + y / 400 + T[(m as usize + 11) % 12] + d as i32).rem_euclid(7)) as u8
}

impl ClockJson {
    /// `extra_secs` 秒進めた時刻 (GIF で秒を進める。日付の繰り上がりは省略)
    pub fn to_clock(self, extra_secs: u32) -> Clock {
        let total = self.hour as u32 * 3600 + self.minute as u32 * 60 + self.second as u32 + extra_secs;
        Clock {
            year: self.year,
            month: self.month,
            day: self.day,
            weekday: weekday(self.year, self.month, self.day),
            hour: ((total / 3600) % 24) as u8,
            minute: ((total / 60) % 60) as u8,
            second: (total % 60) as u8,
        }
    }
}

/// 天気コードの日本語はファームウェアと同じ表 (src/ticker/weather.rs) を使う
#[path = "../../../src/ticker/weather.rs"]
#[allow(dead_code)]
mod weather;

impl Scenario {
    pub fn layout(&self) -> Layout {
        Layout::parse(&self.layout).unwrap_or(Layout::Glass)
    }

    pub fn view(&self, extra_secs: u32, scroll_x: i32) -> View<'_> {
        let s = &self.status;
        View {
            clock: self.clock.map(|c| c.to_clock(extra_secs)),
            place: &self.place,
            weather: self.weather.map(|w| WeatherView {
                temperature: w.temperature,
                code: w.code,
                condition: weather::condition_ja(w.code),
                max: w.max,
                min: w.min,
                rain_pct: w.rain_pct,
            }),
            message: &self.message,
            scroll_x,
            status: StatusView {
                expanded: s.expanded,
                line1: &s.line1,
                line1_tone: tone(&s.line1_tone),
                ident_head: &s.ident_head,
                ident_rest: &s.ident_rest,
                ident_tone: tone(&s.ident_tone),
                ota: &s.ota,
                ota_tone: tone(&s.ota_tone),
                progress: s.progress,
                wifi: tone(&s.wifi),
                ntp: tone(&s.ntp),
                wx: tone(&s.wx),
                msg: tone(&s.msg),
                version: &s.version,
            },
        }
    }
}

use crate::ui::screen::Layout;
