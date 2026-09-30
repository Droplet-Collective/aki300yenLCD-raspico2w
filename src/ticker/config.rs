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
//! slide=30         # 写真の切り替え間隔 (秒、0 で切り替えない。5〜3600)          (v0.4.0〜)
//! images=A.BMP,B.BMP  # 背景に使う BMP (SD のルート、8.3 形式、最大 16)。無ければルートの *.BMP 全部
//! layout=glass     # 画面構成 glass / dock / classic (docs/ticker.md「画面」)
//! status=auto      # 状態 3 行の表示 auto (必要なときだけ) / full (常に) / compact (常に 1 行)
//! sdfast=1         # 写真を読むときの SD の速さ 1 = 速い (読み誤りがあれば自動で 0 に戻す) / 0 = 起動時と同じ低速
//! ```

use heapless::String;

/// 地名の最大長 (バイト)
pub const PLACE_MAX: usize = 32;
/// message_url の最大長
pub const URL_MAX: usize = 256;

/// `images=` の最大長 (バイト)
pub const IMAGES_MAX: usize = 208;
/// `images=` / ルートの走査で使う BMP の最大数
pub const MAX_IMAGES: usize = 16;
/// 写真の切り替え間隔の既定値 (秒)
pub const DEFAULT_SLIDE_SECS: u16 = 30;

/// 画面構成 (`ui::screen::Layout` と同じ名前。config はホストのテストのため ui に依存しない)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutName {
    Glass,
    Dock,
    Classic,
}

/// 状態 3 行の出し方
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusMode {
    /// 起動直後 / 異常時 / OTA 中だけ 3 行、ふだんは小さな 1 行
    Auto,
    /// 常に 3 行
    Full,
    /// 常に小さな 1 行
    Compact,
}

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
    /// 写真の切り替え間隔 (秒、0 = 切り替えない)
    pub slide_secs: u16,
    /// `images=` の値 (カンマ区切り、空ならルートの *.BMP)
    pub images: String<IMAGES_MAX>,
    pub layout: LayoutName,
    pub status: StatusMode,
    /// 写真の読み込みで SD を速く読むか
    pub sd_fast: bool,
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
            slide_secs: DEFAULT_SLIDE_SECS,
            images: String::new(),
            layout: LayoutName::Glass,
            status: StatusMode::Auto,
            sd_fast: true,
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
            } else if key.eq_ignore_ascii_case("slide") {
                value
                    .parse::<u16>()
                    .ok()
                    .filter(|v| *v == 0 || (5..=3600).contains(v))
                    .map(|v| config.slide_secs = v)
                    .is_some()
            } else if key.eq_ignore_ascii_case("images") {
                let mut images = String::new();
                images.push_str(value).is_ok() && image_names(value).next().is_some() && {
                    config.images = images;
                    true
                }
            } else if key.eq_ignore_ascii_case("layout") {
                let layout = if value.eq_ignore_ascii_case("glass") {
                    Some(LayoutName::Glass)
                } else if value.eq_ignore_ascii_case("dock") {
                    Some(LayoutName::Dock)
                } else if value.eq_ignore_ascii_case("classic") {
                    Some(LayoutName::Classic)
                } else {
                    None
                };
                layout.map(|l| config.layout = l).is_some()
            } else if key.eq_ignore_ascii_case("status") {
                let mode = if value.eq_ignore_ascii_case("auto") {
                    Some(StatusMode::Auto)
                } else if value.eq_ignore_ascii_case("full") {
                    Some(StatusMode::Full)
                } else if value.eq_ignore_ascii_case("compact") {
                    Some(StatusMode::Compact)
                } else {
                    None
                };
                mode.map(|m| config.status = m).is_some()
            } else if key.eq_ignore_ascii_case("sdfast") {
                match value {
                    "1" => {
                        config.sd_fast = true;
                        true
                    }
                    "0" => {
                        config.sd_fast = false;
                        true
                    }
                    _ => false,
                }
            } else {
                false
            };
            any |= ok;
        }
        (config, any)
    }
}

/// SD から `TICKER.TXT` を読んだ結果 (ファームウェアの `sdcard` の結果をこれに写して [`load`] に渡す)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigSource<'a> {
    /// SD カードが無い / 初期化できない (メッセージは `sdcard::init_sd` の失敗理由)
    NoCard(&'a str),
    /// ファイルが無い (既定の使い方。東京の既定値で動く)
    NotFound,
    /// ボリューム / ディレクトリ / 読み取りの失敗
    ReadFailed(&'a str),
    /// 読めた内容 (空でもよい)
    Read(&'a [u8]),
}

/// `ticker.txt` の設定と、状態行 1 に出す注意 (問題が無ければ空)。どの場合も既定値 (東京) で起動を続ける。
/// 0.4.1: ticker.txt が無いと止まるのでは、という報告を受けて、4 つの場合をホストのテストで確かめている
/// (`tools/ticker-tests`。0.4.0 の停止の原因は TLS のスタック溢れで、ticker.txt とは無関係)。
pub fn load(source: ConfigSource<'_>) -> (TickerConfig, String<80>) {
    use core::fmt::Write as _;
    let mut note: String<80> = String::new();
    let config = match source {
        ConfigSource::NoCard(message) => {
            let _ = write!(note, "SD: {} (ticker.txt skipped, using Tokyo)", message);
            TickerConfig::default()
        }
        ConfigSource::NotFound => {
            let _ = note.push_str("ticker.txt not found, using Tokyo (35.6812,139.7671 UTC+9)");
            TickerConfig::default()
        }
        ConfigSource::ReadFailed(message) => {
            let _ = write!(note, "ticker.txt: {}, using Tokyo defaults", message);
            TickerConfig::default()
        }
        ConfigSource::Read(bytes) => {
            let (parsed, any) = TickerConfig::parse(bytes);
            if bytes.iter().all(u8::is_ascii_whitespace) {
                let _ = note.push_str("ticker.txt is empty, using Tokyo defaults");
            } else if !any {
                let _ = note.push_str("ticker.txt: no valid keys, using Tokyo defaults");
            }
            parsed
        }
    };
    (config, note)
}

/// `images=` の値から 8.3 形式として正しい名前だけを順に返す (前後の空白は除く。大文字小文字はそのまま)
pub fn image_names(list: &str) -> impl Iterator<Item = &str> {
    list.split(',').map(str::trim).filter(|n| is_short_name(n)).take(MAX_IMAGES)
}

/// 8.3 形式 (本体 1〜8 文字 + `.` + 拡張子 1〜3 文字、ASCII の英数字と `_-~!#$%&'()@^{}` だけ)
pub fn is_short_name(name: &str) -> bool {
    let Some((base, ext)) = name.split_once('.') else {
        return false;
    };
    let ok = |s: &str, max: usize| {
        !s.is_empty() && s.len() <= max && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"_-~!#$%&'()@^{}".contains(&b))
    };
    ok(base, 8) && ok(ext, 3)
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
