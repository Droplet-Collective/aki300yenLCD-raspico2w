//! 流れる文字の組み立て (0.5.1〜): 文字 + 区切り + 設定ページの URL とアクセスコード。
//!
//! `…文字…　◆　設定 http://192.168.x.y/  コード 123456　◆　` を 1 本の文字列にして、色の違う部分
//! ([`Span`]) の位置と幅を一緒に持つ。設定の部分があるときは文字列を切れ目なく繰り返して流す
//! ([`advance`] の `looped`。末尾の区切りの後にすぐ先頭が続くので、つなぎ目で飛ばない)。
//!
//! ファームウェアは組み立てた結果を static な共有モデルに置き (スタックに置かない)、描画タスクは
//! 入力 (文字の世代、URL / コードの世代、設定の部分の状態) が変わったときだけ組み立て直す。
//! ホストのシミュレータ (`tools/ui-sim`) とテスト (`tools/ticker-tests`) も同じソースを使う。

use heapless::{String, Vec};

use crate::font::shinonome;

/// 部分の間の区切り (全角空白 + ◆ + 全角空白)
pub const SEP: &str = "\u{3000}◆\u{3000}";
/// 設定の部分の見出し
pub const LABEL_SETTINGS: &str = "設定 ";
/// URL とコードの間 (見出し。Glass の帯 264 px に URL の先頭からコードの末尾まで (最長の IP でなければ) 収まる幅)
pub const LABEL_CODE: &str = " コード ";
/// IP アドレスが無い (Wi-Fi が切れている) 間の設定の部分
pub const WAITING: &str = "設定: Wi-Fi 接続待ち";
/// 設定の部分の最大長 (バイト): 区切り 2 つ + 見出し 2 つ + URL (`http://255.255.255.255/` = 23) + コード (8)
pub const SETTINGS_MAX: usize = 2 * SEP.len() + LABEL_SETTINGS.len() + LABEL_CODE.len() + 23 + 8;
/// 組み立てた文字列の最大長 (バイト)。ファームウェアは `MESSAGE_MAX + SETTINGS_MAX` 以上あることを確かめる
pub const TEXT_MAX: usize = 512 + 96;
/// 色の違う部分の最大数 (文字 / 区切り / 見出し / URL / 見出し / コード / 区切り)
pub const SPANS_MAX: usize = 8;

const _: () = assert!(TEXT_MAX >= 512 + SETTINGS_MAX);

/// 部分の色の種類 (色は画面構成が決める)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ink {
    /// 流れる文字 (画面構成の文字の色)
    Message,
    /// 区切りの ◆
    Sep,
    /// `設定` / `コード` の見出し
    Label,
    /// URL とコード (目立つ色)
    Value,
    /// `設定: Wi-Fi 接続待ち`
    Note,
}

/// 色の違う 1 部分: 文字列の `start..end` (バイト、文字の境目)、先頭からの位置 `x` と幅 `w` (px)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: u16,
    pub end: u16,
    pub x: i32,
    pub w: i32,
    pub ink: Ink,
}

/// 設定の部分に何を出すか
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Settings<'a> {
    /// 出さない (`show_settings=0`、待ち受け前、回復モード)
    Hidden,
    /// 待ち受け中だが IP アドレスが無い
    Waiting,
    /// URL (`http://192.168.x.y/`) とアクセスコード
    Ready { url: &'a str, code: &'a str },
}

/// 組み立てた流れる文字 (static に置く。≈ 0.8 kB)
pub struct ScrollText {
    text: String<TEXT_MAX>,
    spans: Vec<Span, SPANS_MAX>,
    width: i32,
    /// 設定の部分 (先頭の区切りの後から末尾の区切りの前まで) の位置と幅 (px)
    settings: Option<(i32, i32)>,
}

impl Default for ScrollText {
    fn default() -> Self {
        Self::new()
    }
}

/// 描画に渡す形 (借りるだけ)
#[derive(Clone, Copy, Debug)]
pub struct Line<'a> {
    pub text: &'a str,
    pub spans: &'a [Span],
    /// 全体の幅 (px)。繰り返すときの周期
    pub width: i32,
    /// 切れ目なく繰り返して流すか (設定の部分があるとき)
    pub looped: bool,
    /// 設定の部分を目立たせる範囲 (先頭からの x, 幅)。「LCD にコードを表示」の直後だけ Some
    pub highlight: Option<(i32, i32)>,
}

impl Line<'_> {
    pub const EMPTY: Line<'static> = Line {
        text: "",
        spans: &[],
        width: 0,
        looped: false,
        highlight: None,
    };

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

/// `s` を入るだけ (文字の境目で) 足す。足したバイト数を返す
fn push_trunc<const N: usize>(out: &mut String<N>, s: &str) -> usize {
    let room = N - out.len();
    let mut end = s.len().min(room);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    let _ = out.push_str(&s[..end]);
    end
}

impl ScrollText {
    pub const fn new() -> Self {
        Self {
            text: String::new(),
            spans: Vec::new(),
            width: 0,
            settings: None,
        }
    }

    /// `message` (空でもよい) と設定の部分を組み立て直す。どんな長さでも溢れず、設定の部分は必ず全部入る
    /// (足りなければ文字の方を文字の境目で切る)
    pub fn compose(&mut self, message: &str, settings: Settings<'_>) {
        self.text.clear();
        self.spans.clear();
        self.width = 0;
        self.settings = None;
        let tail_len = match settings {
            Settings::Hidden => 0,
            Settings::Waiting => SEP.len() + WAITING.len() + SEP.len(),
            Settings::Ready { url, code } => SEP.len() + LABEL_SETTINGS.len() + url.len() + LABEL_CODE.len() + code.len() + SEP.len(),
        };
        // 文字に使えるのは設定の部分の残り (設定の部分が極端に長ければ 0)
        let budget = TEXT_MAX.saturating_sub(tail_len);
        let mut end = message.len().min(budget);
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        let message = &message[..end];
        self.push(message, Ink::Message);
        match settings {
            Settings::Hidden => {}
            Settings::Waiting => {
                if !message.is_empty() {
                    self.push(SEP, Ink::Sep);
                }
                let start = self.width;
                self.push(WAITING, Ink::Note);
                self.settings = Some((start, self.width - start));
                self.push(SEP, Ink::Sep);
            }
            Settings::Ready { url, code } => {
                if !message.is_empty() {
                    self.push(SEP, Ink::Sep);
                }
                let start = self.width;
                self.push(LABEL_SETTINGS, Ink::Label);
                self.push(url, Ink::Value);
                self.push(LABEL_CODE, Ink::Label);
                self.push(code, Ink::Value);
                self.settings = Some((start, self.width - start));
                self.push(SEP, Ink::Sep);
            }
        }
    }

    fn push(&mut self, s: &str, ink: Ink) {
        if s.is_empty() || self.spans.is_full() {
            return;
        }
        let start = self.text.len();
        let n = push_trunc(&mut self.text, s);
        if n == 0 {
            return;
        }
        let w = shinonome::text_width(&s[..n]) as i32;
        let _ = self.spans.push(Span {
            start: start as u16,
            end: (start + n) as u16,
            x: self.width,
            w,
            ink,
        });
        self.width += w;
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn spans(&self) -> &[Span] {
        &self.spans
    }

    pub fn width(&self) -> i32 {
        self.width
    }

    /// 設定の部分がある (切れ目なく繰り返す)
    pub fn looped(&self) -> bool {
        self.settings.is_some()
    }

    /// 設定の部分の位置と幅 (px)
    pub fn settings(&self) -> Option<(i32, i32)> {
        self.settings
    }

    /// 設定の部分が範囲の左端に来るスクロール位置 (「LCD にコードを表示」で飛ぶ先)
    pub fn settings_scroll_x(&self) -> Option<i32> {
        self.settings.map(|(x, _)| -x)
    }

    pub fn line(&self, highlight: bool) -> Line<'_> {
        Line {
            text: &self.text,
            spans: &self.spans,
            width: self.width,
            looped: self.looped(),
            highlight: if highlight { self.settings } else { None },
        }
    }
}

/// スクロール位置を `step` px 進める。`scroll_x` は範囲の左端から文字列の先頭までの px。
///
/// - `looped` (設定の部分あり): 1 周 (`width`) 進んだら 1 周分戻す。描画は `width` ごとに並べるので見た目は切れ目なく続く。
/// - そうでなければ 0.4.x と同じ: 文字列が左へ出切ったら範囲の右端 (`area_w`) から入り直す。
pub fn advance(scroll_x: i32, step: i32, width: i32, area_w: i32, looped: bool) -> i32 {
    if width <= 0 {
        return scroll_x;
    }
    let x = scroll_x - step;
    if looped {
        if x <= -width { x + width * ((-x) / width) } else { x }
    } else if x + width < 0 {
        area_w
    } else {
        x
    }
}

/// 範囲 (幅 `area_w`) に見える写しの先頭 (範囲の左端からの px)。繰り返さないなら `scroll_x` だけ
pub fn copies(line: &Line<'_>, scroll_x: i32, area_w: i32) -> impl Iterator<Item = i32> {
    let period = line.width.max(1);
    let n = if line.looped && line.width > 0 { (area_w - scroll_x).max(0) / period + 2 } else { 1 };
    (0..n).map(move |k| scroll_x + k * period).filter(move |x| *x < area_w && *x + period > 0)
}
