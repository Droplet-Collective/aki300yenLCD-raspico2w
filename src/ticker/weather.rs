//! Open-Meteo (<https://open-meteo.com/>、API キー不要、非商用無料) の現在天気と今日の予報
//!
//! 要求 (1 日分だけ、本文 ≈ 620 B、chunked):
//! ```text
//! https://api.open-meteo.com/v1/forecast?latitude=35.6812&longitude=139.7671
//!   &current=temperature_2m,weather_code
//!   &daily=temperature_2m_max,temperature_2m_min,precipitation_probability_max
//!   &timezone=Asia%2FTokyo&forecast_days=1
//! ```
//! 応答のうち `current` と `daily` だけを serde-json-core で読む (他の項目は読み飛ばす)。

use core::fmt::Write as _;

use heapless::String;
use serde::Deserialize;

/// 応答本文の上限 (実測 ≈ 620 B。余裕を持たせる)
pub const BODY_MAX: usize = 1536;
/// 要求 URL の長さ上限
pub const URL_MAX: usize = 320;

#[derive(Deserialize)]
struct Current {
    temperature_2m: f32,
    weather_code: u8,
}

#[derive(Deserialize)]
struct Daily {
    temperature_2m_max: [Option<f32>; 1],
    temperature_2m_min: [Option<f32>; 1],
    precipitation_probability_max: [Option<u8>; 1],
}

#[derive(Deserialize)]
struct Raw {
    current: Current,
    daily: Daily,
}

/// LCD に出す天気
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Weather {
    /// 現在気温 (℃)
    pub temperature: f32,
    /// WMO 天気コード
    pub code: u8,
    pub max: Option<f32>,
    pub min: Option<f32>,
    /// 今日の最大降水確率 (%)
    pub rain_pct: Option<u8>,
}

impl Weather {
    /// Open-Meteo の JSON 本文を解釈する
    pub fn parse(body: &[u8]) -> Option<Weather> {
        let (raw, _) = serde_json_core::from_slice::<Raw>(body).ok()?;
        Some(Weather {
            temperature: raw.current.temperature_2m,
            code: raw.current.weather_code,
            max: raw.daily.temperature_2m_max[0],
            min: raw.daily.temperature_2m_min[0],
            rain_pct: raw.daily.precipitation_probability_max[0],
        })
    }

    /// 天気コードの日本語 (東雲フォントにある文字だけ)
    pub fn condition_ja(&self) -> &'static str {
        condition_ja(self.code)
    }
}

/// WMO 4677 の天気コード → 短い日本語
pub fn condition_ja(code: u8) -> &'static str {
    match code {
        0 => "快晴",
        1 => "晴れ",
        2 => "晴れ時々くもり",
        3 => "くもり",
        45 | 48 => "霧",
        51 | 53 | 55 => "霧雨",
        56 | 57 => "着氷性の霧雨",
        61 => "小雨",
        63 => "雨",
        65 => "大雨",
        66 | 67 => "着氷性の雨",
        71 => "小雪",
        73 => "雪",
        75 => "大雪",
        77 => "霧雪",
        80 => "にわか雨",
        81 => "強いにわか雨",
        82 => "激しいにわか雨",
        85 | 86 => "にわか雪",
        95 => "雷雨",
        96 | 99 => "雷雨とひょう",
        _ => "不明",
    }
}

/// 要求 URL を組み立てる。`scheme` は "https" か "http" (TLS が通らないときの予備)。
/// 緯度経度は小数 4 桁。`tz_offset_secs` はそのまま Open-Meteo に渡さず、`timezone=auto` で座標から
/// 現地時刻の 1 日を切ってもらう (`daily` の最高 / 最低は現地の今日)。
pub fn request_url(scheme: &str, lat: f32, lon: f32) -> String<URL_MAX> {
    let mut url = String::new();
    let _ = write!(
        url,
        "{}://api.open-meteo.com/v1/forecast?latitude={:.4}&longitude={:.4}\
         &current=temperature_2m,weather_code\
         &daily=temperature_2m_max,temperature_2m_min,precipitation_probability_max\
         &timezone=auto&forecast_days=1",
        scheme, lat, lon
    );
    url
}

/// 気温を `19.1` の形に (小数 1 桁、負なら `-3.2`)。`f32` の `{:.1}` は core::fmt で使える
pub fn format_temp<const N: usize>(out: &mut String<N>, value: f32) {
    let _ = write!(out, "{:.1}", value);
}
