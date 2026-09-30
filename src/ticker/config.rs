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
//! debug_crash=ota  # 試験用 (0.4.2〜): boot / ota / slideshow の場所でわざと panic する。既定は無し
//! message=こんにちは  # 流れる文字をこの端末で決める (0.5.0〜)。行末までそのまま (# もコメントにしない)。
//!                     # 空、または行が無ければ message_url から取得する
//! show_settings=1  # 流れる文字に設定ページの URL とアクセスコードを入れる (0.5.1〜、既定 1)。0 で入れない
//! ```
//!
//! 設定ページ (0.5.0〜、docs/settings-server.md) は [`rewrite`] でこのファイルを書き換える: 変えたキーの行だけを
//! 置き換え、知らないキー / コメント / 空行 / 行の順番はそのまま残す (手で書いた内容と画面の設定を 1 つに保つ)。
//!
//! `debug_crash` は回復モード (docs/ticker.md §8) を確かめるためのもの。buy 済みの版の通常起動でだけ効き、
//! TBYB の buy 待ち (OTA で届いたばかりの版) と回復モードでは無視する (回復モードは ticker.txt を読まない)。

use heapless::String;

/// 地名の最大長 (バイト)
pub const PLACE_MAX: usize = 32;
/// `TICKER.TXT` の最大長 (バイト。0.5.0 で 512 → 1536、設定ページがコメントを残したまま書き足すため)
pub const CONFIG_MAX: usize = 1536;
/// `message=` (この端末で決める流れる文字) の最大長 (バイト。LCD の流れる文字の上限と同じ)
pub const MESSAGE_MAX: usize = 512;
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

/// 試験用にわざと落ちる場所 (`debug_crash=`)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DebugCrash {
    None,
    /// ticker.txt を読んだ直後 (LCD の前)
    Boot,
    /// 接続後、最初の OTA 確認の直前 (取得タスク)
    Ota,
    /// 最初の写真を読み始めたとき
    Slideshow,
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
    /// 試験用にわざと落ちる場所 (既定 None)
    pub debug_crash: DebugCrash,
    /// `message=` に文字がある (流れる文字を取得せず、その文字を出す。本文は [`message_text`] で取り出す)
    pub local_message: bool,
    /// 流れる文字に設定ページの URL とアクセスコードを入れる (`show_settings=`、0.5.1〜、既定 true)
    pub show_settings: bool,
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
            debug_crash: DebugCrash::None,
            local_message: false,
            show_settings: true,
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
            let Some((key, raw_value)) = line.split_once('=') else {
                continue;
            };
            let value = raw_value.split('#').next().unwrap_or("").trim();
            let key = key.trim();
            let ok = if key.eq_ignore_ascii_case("message") {
                // 行末まで (# もそのまま)。空なら取得に戻す
                let text = raw_value.trim();
                config.local_message = !text.is_empty() && text.len() <= MESSAGE_MAX;
                text.len() <= MESSAGE_MAX
            } else if key.eq_ignore_ascii_case("lat") {
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
            } else if key.eq_ignore_ascii_case("debug_crash") {
                let crash = if value.eq_ignore_ascii_case("boot") {
                    Some(DebugCrash::Boot)
                } else if value.eq_ignore_ascii_case("ota") {
                    Some(DebugCrash::Ota)
                } else if value.eq_ignore_ascii_case("slideshow") {
                    Some(DebugCrash::Slideshow)
                } else if value.is_empty() || value.eq_ignore_ascii_case("none") || value == "0" {
                    Some(DebugCrash::None)
                } else {
                    None
                };
                crash.map(|c| config.debug_crash = c).is_some()
            } else if key.eq_ignore_ascii_case("show_settings") {
                match value {
                    "1" => {
                        config.show_settings = true;
                        true
                    }
                    "0" => {
                        config.show_settings = false;
                        true
                    }
                    _ => false,
                }
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

/// `message=` の文字 (最後の行。空 / 長すぎる / 無ければ None)。[`TickerConfig::parse`] と同じ規則
pub fn message_text(bytes: &[u8]) -> Option<&str> {
    let text = core::str::from_utf8(bytes).ok()?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut found = None;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=')
            && key.trim().eq_ignore_ascii_case("message")
        {
            let value = value.trim();
            found = (!value.is_empty() && value.len() <= MESSAGE_MAX).then_some(value);
        }
    }
    found
}

/// 設定ページが送った 1 つの値が `ticker.txt` の規則で正しいか (0.5.0〜)。`key=value` の 1 行として
/// [`TickerConfig::parse`] に読ませ、受け付けられたら正しい (規則を 2 か所に書かない)。
/// 改行と、`message` 以外の `#` (コメントの始まりとして値が切れてしまう) は受け付けない。
pub fn valid_value(key: &str, value: &str) -> bool {
    if value.contains(['\n', '\r']) || key.is_empty() || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return false;
    }
    let is_message = key.eq_ignore_ascii_case("message");
    if !is_message && value.contains('#') {
        return false;
    }
    if is_message {
        return value.trim().len() <= MESSAGE_MAX;
    }
    let mut line: String<{ URL_MAX + 32 }> = String::new();
    if line.push_str(key).is_err() || line.push('=').is_err() || line.push_str(value).is_err() {
        return false;
    }
    TickerConfig::parse(line.as_bytes()).1
}

/// [`rewrite`] に渡す変更: `Some(値)` で置き換え (無ければ末尾に足す)、`None` で行を消す
pub type Update<'a> = (&'a str, Option<&'a str>);

/// [`rewrite`] の失敗
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RewriteError {
    /// 書き換えた結果が `out` (または [`CONFIG_MAX`]) に収まらない
    TooLong,
    /// 元のファイルが UTF-8 でない
    NotUtf8,
}

/// `ticker.txt` の `old` に `updates` を当てた内容を `out` へ書き、長さを返す (0.5.0〜、設定ページの保存)。
///
/// - `key=value` の行のうち、キー (大文字小文字は区別しない) が `updates` にあるものだけを書き換える。
///   同じキーの行が複数あればすべて (読むときは最後の行が勝つので、1 つだけ直すと効かない)。
///   キーの書き方、行頭の空白、値の後ろのコメント (`  # ...`、`message` 以外) は残す。
/// - `None` の更新はその行を消す。ファイルに無いキーは末尾に `key=value` を足す。
/// - コメント行、空行、知らないキー、行の順番、改行の種類 (CRLF / LF)、先頭の BOM はそのまま。
pub fn rewrite(old: &[u8], updates: &[Update<'_>], out: &mut [u8]) -> Result<usize, RewriteError> {
    let text = core::str::from_utf8(old).map_err(|_| RewriteError::NotUtf8)?;
    let limit = out.len().min(CONFIG_MAX);
    let mut w = Out { buf: &mut out[..limit], len: 0 };
    let (bom, body) = match text.strip_prefix('\u{feff}') {
        Some(rest) => ("\u{feff}", rest),
        None => ("", text),
    };
    let newline = if body.contains("\r\n") { "\r\n" } else { "\n" };
    w.push(bom)?;
    let mut seen = [false; 16];
    let find = |key: &str| updates.iter().position(|(k, _)| k.eq_ignore_ascii_case(key.trim()));
    let mut rest = body;
    while !rest.is_empty() {
        let (line, ending, next) = match rest.find('\n') {
            Some(i) => {
                let line = &rest[..i];
                match line.strip_suffix('\r') {
                    Some(l) => (l, "\r\n", &rest[i + 1..]),
                    None => (line, "\n", &rest[i + 1..]),
                }
            }
            None => (rest, "", ""),
        };
        rest = next;
        let trimmed = line.trim_start();
        let hit = if trimmed.starts_with('#') {
            None
        } else {
            trimmed.split_once('=').and_then(|(key, value)| find(key).map(|i| (i, key, value)))
        };
        match hit {
            None => {
                w.push(line)?;
                w.push(ending)?;
            }
            Some((i, key, value)) => {
                if i < seen.len() {
                    seen[i] = true;
                }
                let Some(new_value) = updates[i].1 else {
                    continue; // 行を消す
                };
                let indent = &line[..line.len() - trimmed.len()];
                w.push(indent)?;
                w.push(key.trim_end())?;
                w.push("=")?;
                w.push(new_value)?;
                // 値の後ろのコメントは残す (message は行末まで値なので、コメントは無い)
                if !key.trim().eq_ignore_ascii_case("message")
                    && let Some(pos) = value.find('#')
                {
                    let before = &value[..pos];
                    let gap = &before[before.trim_end().len()..];
                    w.push(if gap.is_empty() { " " } else { gap })?;
                    w.push(&value[pos..])?;
                }
                w.push(ending)?;
            }
        }
    }
    for (i, (key, value)) in updates.iter().enumerate() {
        if i < seen.len() && seen[i] {
            continue;
        }
        let Some(value) = value else {
            continue;
        };
        if w.len > bom.len() && !w.ends_with_newline() {
            w.push(newline)?;
        }
        w.push(key)?;
        w.push("=")?;
        w.push(value)?;
        w.push(newline)?;
    }
    Ok(w.len)
}

struct Out<'a> {
    buf: &'a mut [u8],
    len: usize,
}

impl Out<'_> {
    fn push(&mut self, s: &str) -> Result<(), RewriteError> {
        let end = self.len + s.len();
        if end > self.buf.len() {
            return Err(RewriteError::TooLong);
        }
        self.buf[self.len..end].copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }

    fn ends_with_newline(&self) -> bool {
        self.len == 0 || self.buf[self.len - 1] == b'\n'
    }
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
