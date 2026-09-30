//! 流れる文字の組み立て (0.5.1〜、`src/ui/scroll.rs`): 文字 + 区切り + 設定ページの URL とアクセスコード

use crate::config::{MESSAGE_MAX, TickerConfig};
use crate::scroll::{self, Ink, LABEL_CODE, LABEL_SETTINGS, SEP, ScrollText, Settings, TEXT_MAX, WAITING};
use crate::shinonome;

const URL: &str = "http://192.168.200.130/";

fn ready<'a>(url: &'a str, code: &'a str) -> Settings<'a> {
    Settings::Ready { url, code }
}

/// 部分ごとの文字と色 (組み立ての結果を読みやすく比べる)
fn parts(t: &ScrollText) -> Vec<(&str, Ink)> {
    t.spans().iter().map(|s| (&t.text()[s.start as usize..s.end as usize], s.ink)).collect()
}

/// 部分の位置 / 幅が文字列と東雲の幅に合っていて、隙間なく並ぶ
fn check_geometry(t: &ScrollText) {
    let mut x = 0;
    let mut end = 0;
    for s in t.spans() {
        assert_eq!(s.start as usize, end, "spans are contiguous");
        assert!(s.end > s.start);
        assert!(t.text().is_char_boundary(s.start as usize) && t.text().is_char_boundary(s.end as usize));
        assert_eq!(s.x, x);
        assert_eq!(s.w, shinonome::text_width(&t.text()[s.start as usize..s.end as usize]) as i32);
        x += s.w;
        end = s.end as usize;
    }
    assert_eq!(end, t.text().len());
    assert_eq!(x, t.width());
    assert_eq!(t.width(), shinonome::text_width(t.text()) as i32);
    assert!(t.text().len() <= TEXT_MAX);
}

#[test]
fn settings_part_follows_the_message_with_separators() {
    let mut t = ScrollText::new();
    t.compose("こんにちは", ready(URL, "482913"));
    assert_eq!(t.text(), "こんにちは\u{3000}◆\u{3000}設定 http://192.168.200.130/ コード 482913\u{3000}◆\u{3000}");
    assert_eq!(
        parts(&t),
        vec![
            ("こんにちは", Ink::Message),
            (SEP, Ink::Sep),
            (LABEL_SETTINGS, Ink::Label),
            (URL, Ink::Value),
            (LABEL_CODE, Ink::Label),
            ("482913", Ink::Value),
            (SEP, Ink::Sep),
        ]
    );
    assert!(t.looped());
    check_geometry(&t);
    // 設定の部分の位置 = 文字 + 区切りの幅、幅 = 見出しからコードの末尾まで
    let (sx, sw) = t.settings().unwrap();
    assert_eq!(sx, shinonome::text_width("こんにちは\u{3000}◆\u{3000}") as i32);
    assert_eq!(sw, shinonome::text_width("設定 http://192.168.200.130/ コード 482913") as i32);
    assert_eq!(t.settings_scroll_x(), Some(-sx));
    // 区切りと見出しは東雲にある文字 (□ にならない)
    for ch in "\u{3000}◆設定コード:接続待ち".chars() {
        assert!(shinonome::glyph(ch).is_some(), "{ch}");
    }
    // Glass の帯 (264 px) に URL の先頭からコードの末尾までが収まる (一度に読める)
    assert!(shinonome::text_width("http://192.168.200.130/ コード 482913") <= 264);
}

#[test]
fn empty_message_is_only_the_settings_part() {
    let mut t = ScrollText::new();
    t.compose("", ready(URL, "000123"));
    assert_eq!(t.text(), "設定 http://192.168.200.130/ コード 000123\u{3000}◆\u{3000}");
    assert_eq!(t.settings().unwrap().0, 0);
    assert!(t.looped());
    check_geometry(&t);
}

#[test]
fn hidden_is_the_plain_message_as_before() {
    // show_settings=0 / 待ち受け前 / 回復モード: 0.4.x と同じ (繰り返さず、右端から入り直す)
    let mut t = ScrollText::new();
    t.compose("こんにちは", Settings::Hidden);
    assert_eq!(t.text(), "こんにちは");
    assert_eq!(parts(&t), vec![("こんにちは", Ink::Message)]);
    assert!(!t.looped());
    assert_eq!(t.settings(), None);
    assert_eq!(t.settings_scroll_x(), None);
    check_geometry(&t);
    t.compose("", Settings::Hidden);
    assert!(t.text().is_empty() && t.spans().is_empty() && t.width() == 0);
    assert!(t.line(true).is_empty());
    assert_eq!(t.line(true).highlight, None);
}

#[test]
fn no_ip_shows_waiting() {
    let mut t = ScrollText::new();
    t.compose("こんにちは", Settings::Waiting);
    assert_eq!(t.text(), format!("こんにちは{SEP}{WAITING}{SEP}"));
    assert_eq!(parts(&t)[2], (WAITING, Ink::Note));
    assert!(t.looped());
    assert!(!t.text().contains("http"));
    check_geometry(&t);
}

#[test]
fn code_or_ip_change_recomposes_in_place() {
    let mut t = ScrollText::new();
    t.compose("文字", ready(URL, "111111"));
    let w1 = t.width();
    t.compose("文字", ready(URL, "222222"));
    assert!(t.text().contains("コード 222222") && !t.text().contains("111111"));
    assert_eq!(t.width(), w1, "same length code, same width");
    t.compose("文字", ready("http://10.0.0.5/", "222222"));
    assert!(t.text().contains("http://10.0.0.5/") && !t.text().contains("192.168"));
    assert!(t.width() < w1);
    check_geometry(&t);
    // 目立たせる範囲は設定の部分
    let line = t.line(true);
    assert_eq!(line.highlight, t.settings());
    assert!(line.looped);
    assert_eq!(t.line(false).highlight, None);
}

#[test]
fn long_message_is_cut_on_a_char_boundary_and_settings_always_fit() {
    let mut t = ScrollText::new();
    // 1..=4 バイトの文字を混ぜて、どの位置で切れても文字の境目になるように
    for lead in ["", "a", "ab", "abc"] {
        let long = format!("{lead}{}", "あé😀x".repeat(400));
        assert!(long.len() > TEXT_MAX * 2);
        for settings in [ready("http://255.255.255.255/", "999999"), Settings::Waiting, Settings::Hidden] {
            t.compose(&long, settings);
            assert!(t.text().len() <= TEXT_MAX);
            check_geometry(&t);
            match settings {
                Settings::Ready { .. } => {
                    assert!(t.text().ends_with(&format!("{LABEL_SETTINGS}http://255.255.255.255/{LABEL_CODE}999999{SEP}")));
                }
                Settings::Waiting => assert!(t.text().ends_with(&format!("{WAITING}{SEP}"))),
                Settings::Hidden => assert!(t.text().len() > TEXT_MAX - 4),
            }
            // 文字の部分は元の文字の先頭そのまま (境目で切れている)
            let msg = parts(&t)[0].0;
            assert!(long.starts_with(msg) && !msg.is_empty());
        }
    }
    // 上限 (MESSAGE_MAX) までの文字なら切らずに全部入る
    let full = "あ".repeat(MESSAGE_MAX / 3);
    t.compose(&full, ready("http://255.255.255.255/", "99999999"));
    assert_eq!(parts(&t)[0].0, full);
    assert!(TEXT_MAX >= MESSAGE_MAX + scroll::SETTINGS_MAX);
    // 極端に長い URL / コード (起こらないが) でも溢れず落ちない
    let url = "h".repeat(TEXT_MAX * 2);
    t.compose("文字", ready(&url, "1"));
    assert!(t.text().len() <= TEXT_MAX);
    check_geometry(&t);
}

#[test]
fn advance_wraps_seamlessly_when_looped() {
    // 繰り返す: 1 周 (width) 進んだら 1 周分戻す (範囲の右端へは戻らない)
    let w = 500;
    let mut x = 0;
    for _ in 0..(w * 3) {
        let nx = scroll::advance(x, 1, w, 264, true);
        assert!(nx == x - 1 || nx == x - 1 + w, "{x} -> {nx}");
        assert!(nx > -w && nx <= 264);
        x = nx;
    }
    // 1 フレームに何 px 飛んでも (描画の遅れ) 範囲に収まる
    assert_eq!(scroll::advance(-495, 8, w, 264, true), -3);
    assert_eq!(scroll::advance(-10, 3 * w, w, 264, true), -10);
    // 繰り返さない: 0.4.x と同じく、出切ったら右端から
    assert_eq!(scroll::advance(-499, 1, w, 264, false), -500);
    assert_eq!(scroll::advance(-500, 1, w, 264, false), 264);
    // 空なら動かない
    assert_eq!(scroll::advance(42, 1, 0, 264, true), 42);
}

#[test]
fn copies_cover_the_area_without_gaps() {
    let mut t = ScrollText::new();
    t.compose("", ready(URL, "482913"));
    let line = t.line(false);
    let w = line.width;
    for x in [-w + 1, -w / 2, 0, 10, 263] {
        let c: Vec<i32> = scroll::copies(&line, x, 264).collect();
        assert!(!c.is_empty());
        assert_eq!(c[0], x, "the first copy is the scroll position");
        for pair in c.windows(2) {
            assert_eq!(pair[1] - pair[0], w);
        }
        // 最後の写しが範囲の右端を越えて終わる (右に空白が出ない)
        assert!(*c.last().unwrap() + w >= 264 || x > 0);
    }
    t.compose("文字", Settings::Hidden);
    let line = t.line(false);
    assert_eq!(scroll::copies(&line, 100, 264).collect::<Vec<_>>(), vec![100]);
    assert_eq!(scroll::copies(&line, -1000, 264).count(), 0);
}

#[test]
fn show_settings_key() {
    assert!(TickerConfig::default().show_settings);
    let (c, any) = TickerConfig::parse(b"show_settings=0\n");
    assert!(any && !c.show_settings);
    let (c, any) = TickerConfig::parse(b"SHOW_SETTINGS = 1 # comment\n");
    assert!(any && c.show_settings);
    let (c, any) = TickerConfig::parse(b"show_settings=off\n");
    assert!(!any && c.show_settings, "invalid values keep the default");
}
