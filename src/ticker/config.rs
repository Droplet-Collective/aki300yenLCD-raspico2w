//! SD カードの `ticker.txt` (任意)。無ければ東京の既定値。
//!
//! ```text
//! # 行頭 # はコメント。key=value、大文字小文字は区別しない。空白は前後とも無視
//! lat=35.6812
//! lon=139.7671
//! tz=+9            # UTC からの時差 (時間。+9 / 9 / -5.5 / +05:30 の形も可)
//! place=東京        # 天気の行の先頭に出す地名 (UTF-8、東雲フォントにある文字)
//! message_url=https://raw.githubusercontent.com/<owner>/<repo>/main/ticker/message.txt
//! scroll=1         # スクロール速度 (px / フレーム、1〜8)
//! ```

use heapless::String;

/// 地名の最大長 (バイト)
pub const PLACE_MAX: usize = 32;
/// message_url の最大長
pub const URL_MAX: usize = 256;

/// 既定の流れる文字の URL (このリポジトリの `ticker/message.txt`)
pub const DEFAULT_MESSAGE_URL: &str =
    "https://raw.githubusercontent.com/Droplet-Collective/aki300yenLCD-raspico2w/main/ticker/message.txt";

#[derive(Clone, Debug, PartialEq)]
pub struct TickerConfig {
    pub lat: f32,
    pub lon: f32,
    /// UTC からのオフセット (秒)
    pub tz_offset_secs: i32,
    pub place: String<PLACE_MAX>,
    pub message_url: String<URL_MAX>,
    /// 1 フレームあたりのスクロール量 (px)
    pub scroll_px: u8,
}

impl Default for TickerConfig {
    /// 東京駅 (35.6812, 139.7671)、JST
    fn default() -> Self {
        let mut place = String::new();
        let _ = place.push_str("東京");
        let mut message_url = String::new();
        let _ = message_url.push_str(DEFAULT_MESSAGE_URL);
        Self {
            lat: 35.6812,
            lon: 139.7671,
            tz_offset_secs: 9 * 3600,
            place,
            message_url,
            scroll_px: 1,
        }
    }
}

impl TickerConfig {
    /// `ticker.txt` の内容を解釈する。不明なキー / 不正な値は無視して既定値のまま。
    /// 戻り値の第 2 要素は「1 つでも有効なキーを読んだか」。
    pub fn parse(bytes: &[u8]) -> (Self, bool) {
        let mut config = Self::default();
        let mut any = false;
        let Ok(text) = core::str::from_utf8(bytes) else {
            return (config, false);
        };
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.split('#').next().unwrap_or("").trim();
            let key = key.trim();
            let ok = if key.eq_ignore_ascii_case("lat") {
                parse_f32(value).filter(|v| (-90.0..=90.0).contains(v)).map(|v| config.lat = v).is_some()
            } else if key.eq_ignore_ascii_case("lon") {
                parse_f32(value).filter(|v| (-180.0..=180.0).contains(v)).map(|v| config.lon = v).is_some()
            } else if key.eq_ignore_ascii_case("tz") {
                parse_tz(value).map(|v| config.tz_offset_secs = v).is_some()
            } else if key.eq_ignore_ascii_case("place") {
                let mut place = String::new();
                place.push_str(value).is_ok() && !value.is_empty() && {
                    config.place = place;
                    true
                }
            } else if key.eq_ignore_ascii_case("message_url") {
                let mut url = String::new();
                (value.starts_with("http://") || value.starts_with("https://")) && url.push_str(value).is_ok() && {
                    config.message_url = url;
                    true
                }
            } else if key.eq_ignore_ascii_case("scroll") {
                value.parse::<u8>().ok().filter(|v| (1..=8).contains(v)).map(|v| config.scroll_px = v).is_some()
            } else {
                false
            };
            any |= ok;
        }
        (config, any)
    }
}

fn parse_f32(text: &str) -> Option<f32> {
    text.parse::<f32>().ok().filter(|v| v.is_finite())
}

/// `+9` / `9` / `-5.5` / `+05:30` / `9:00` → 秒。範囲は ±14 時間
pub fn parse_tz(text: &str) -> Option<i32> {
    let text = text.trim();
    let (sign, body) = match text.as_bytes().first()? {
        b'-' => (-1, &text[1..]),
        b'+' => (1, &text[1..]),
        _ => (1, text),
    };
    let secs = if let Some((h, m)) = body.split_once(':') {
        h.parse::<i32>().ok()? * 3600 + m.parse::<i32>().ok()? * 60
    } else {
        let hours = parse_f32(body)?;
        (hours * 3600.0) as i32
    };
    if secs > 14 * 3600 {
        return None;
    }
    Some(sign * secs)
}
