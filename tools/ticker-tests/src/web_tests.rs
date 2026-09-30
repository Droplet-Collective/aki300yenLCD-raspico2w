//! 設定ページ (0.5.0〜) の純粋な部品のテスト: 要求ヘッダ、フォーム、ticker.txt の書き換え、アクセスコード、
//! 送り元の確認、BMP の検査と 8.3 の名前、JSON、生存確認 (Who::Web)

use crate::config::{self, RewriteError, TickerConfig};
use crate::health;
use crate::web::auth::{self, AuthError, Guard};
use crate::web::form;
use crate::web::http::{self, HeadError, Method};
use crate::web::json::Json;
use crate::web::upload;

// ============================================================
// HTTP の要求ヘッダ
// ============================================================

#[test]
fn http_parses_get_and_post() {
    let req = b"GET /api/status?x=1 HTTP/1.1\r\nHost: 192.168.1.23\r\nAccept: */*\r\n\r\n";
    let h = http::parse_head(req).unwrap();
    assert_eq!(h.method, Method::Get);
    assert_eq!(h.path, "/api/status");
    assert_eq!(h.query, "x=1");
    assert_eq!(h.host, Some("192.168.1.23"));
    assert_eq!(h.content_length, None);
    assert!(!h.close);
    assert_eq!(h.head_len, req.len());

    let req = b"POST /api/settings HTTP/1.1\r\nhost: 10.0.0.5:80\r\nContent-Type: application/x-www-form-urlencoded\r\n\
content-length: 11\r\nX-Ticker-Code: 123456\r\nOrigin: http://10.0.0.5\r\nSec-Fetch-Site: same-origin\r\nConnection: close\r\n\r\nplace=Osaka";
    let h = http::parse_head(req).unwrap();
    assert_eq!(h.method, Method::Post);
    assert_eq!(h.host, Some("10.0.0.5:80"));
    assert_eq!(h.content_length, Some(11));
    assert_eq!(h.code, Some("123456"));
    assert_eq!(h.origin, Some("http://10.0.0.5"));
    assert_eq!(h.fetch_site, Some("same-origin"));
    assert_eq!(h.content_type, Some("application/x-www-form-urlencoded"));
    assert!(h.close);
    // 本文はヘッダの後ろ
    assert_eq!(&req[h.head_len..], b"place=Osaka");
}

#[test]
fn http_rejects_bad_requests() {
    // 空行がまだ
    assert_eq!(http::parse_head(b"GET / HTTP/1.1\r\nHost: a\r\n"), Err(HeadError::Incomplete));
    // 長すぎる (空行が来ないまま上限)
    let mut long = b"GET / HTTP/1.1\r\nX: ".to_vec();
    long.resize(http::HEAD_MAX, b'a');
    assert_eq!(http::parse_head(&long), Err(HeadError::TooLarge));
    // 空行はあるが上限を超える
    let mut long = b"GET / HTTP/1.1\r\nX: ".to_vec();
    long.resize(http::HEAD_MAX + 10, b'a');
    long.extend_from_slice(b"\r\n\r\n");
    assert_eq!(http::parse_head(&long), Err(HeadError::TooLarge));
    // chunked は受けない
    assert_eq!(
        http::parse_head(b"POST /api/upload HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\n"),
        Err(HeadError::Unsupported)
    );
    // 要求行の形
    for bad in [
        &b"GET /\r\n\r\n"[..],
        b"GET / HTTP/2\r\n\r\n",
        b"get / HTTP/1.1\r\n\r\n",
        b"GET http://x/ HTTP/1.1\r\n\r\n",
        b"GET / HTTP/1.1 extra\r\n\r\n",
        b"GET / HTTP/1.1\r\nNoColon\r\n\r\n",
        b"GET / HTTP/1.1\r\nHost: a\r\nHost: b\r\n\r\n",
        b"POST / HTTP/1.1\r\nContent-Length: 5\r\nContent-Length: 6\r\n\r\n",
        b"POST / HTTP/1.1\r\nContent-Length: -1\r\n\r\n",
        b"POST / HTTP/1.1\r\nContent-Length: 99999999999\r\n\r\n",
        b"GET / HTTP/1.1\r\nBad Name: x\r\n\r\n",
        b"GET / HTTP/1.1\r\nX: \xff\r\n\r\n",
    ] {
        assert_eq!(http::parse_head(bad), Err(HeadError::Bad), "{:?}", String::from_utf8_lossy(bad));
    }
    // 知らないメソッドは Other (405 にする)
    assert_eq!(http::parse_head(b"DELETE / HTTP/1.1\r\n\r\n").unwrap().method, Method::Other);
    // HTTP/1.0 は keep-alive を頼まない限り閉じる
    assert!(http::parse_head(b"GET / HTTP/1.0\r\n\r\n").unwrap().close);
    assert!(!http::parse_head(b"GET / HTTP/1.0\r\nConnection: keep-alive\r\n\r\n").unwrap().close);
    // Expect
    assert!(http::parse_head(b"POST / HTTP/1.1\r\nExpect: 100-continue\r\n\r\n").unwrap().expect_continue);
    assert_eq!(http::parse_head(b"POST / HTTP/1.1\r\nExpect: other\r\n\r\n"), Err(HeadError::Unsupported));
}

#[test]
fn http_query_and_numbers() {
    assert_eq!(http::query_param("name=IMG.BMP&x=1", "name"), Some("IMG.BMP"));
    assert_eq!(http::query_param("a&name=", "name"), Some(""));
    assert_eq!(http::query_param("names=1", "name"), None);
    assert_eq!(http::parse_u32("0115254"), Some(115_254));
    assert_eq!(http::parse_u32("+1"), None);
    assert_eq!(http::parse_u32(""), None);
    assert_eq!(http::parse_u32("4294967296"), None);
    assert_eq!(http::reason(429), "Too Many Requests");
}

// ============================================================
// フォーム
// ============================================================

#[test]
fn form_decodes_urlsearchparams() {
    let body = "place=%E5%A4%A7%E9%98%AA&lat=34.69&message=%E3%81%93%E3%82%93%E3%81%AB%E3%81%A1%E3%81%AF+%23tag&&empty=";
    let pairs: Vec<_> = form::pairs(body).collect();
    assert_eq!(pairs.len(), 4);
    let mut buf = [0u8; 64];
    assert_eq!(form::decode(pairs[0].1, &mut buf), Some("大阪"));
    assert_eq!(form::decode(pairs[2].1, &mut buf), Some("こんにちは #tag"));
    assert_eq!(pairs[3], ("empty", ""));
    // 壊れた % / UTF-8 でない / 領域が足りない
    assert_eq!(form::decode("%E5%A4", &mut buf), None);
    assert_eq!(form::decode("%G0", &mut buf), None);
    assert_eq!(form::decode("abc%", &mut buf), None);
    let mut small = [0u8; 2];
    assert_eq!(form::decode("abc", &mut small), None);
    assert_eq!(form::decode("", &mut small), Some(""));
}

// ============================================================
// ticker.txt の書き換え (知らないキー / コメントを残す)
// ============================================================

fn rewrite(old: &str, updates: &[config::Update<'_>]) -> String {
    let mut out = [0u8; config::CONFIG_MAX];
    let n = config::rewrite(old.as_bytes(), updates, &mut out).unwrap();
    String::from_utf8(out[..n].to_vec()).unwrap()
}

#[test]
fn rewrite_keeps_unknown_keys_and_comments() {
    let old = "# 自分用のメモ\nlat=35.6812\nlon=139.7671   # 東京駅\n\nfoo=bar\n  place = 東京\nsdfast=0\n# 末尾のコメント\n";
    let new = rewrite(old, &[("place", Some("大阪")), ("lon", Some("135.5")), ("layout", Some("dock"))]);
    assert_eq!(
        new,
        "# 自分用のメモ\nlat=35.6812\nlon=135.5   # 東京駅\n\nfoo=bar\n  place=大阪\nsdfast=0\n# 末尾のコメント\nlayout=dock\n"
    );
    // 読み直すと新しい値、知らないキーの影響なし
    let (c, any) = TickerConfig::parse(new.as_bytes());
    assert!(any);
    assert_eq!(c.place.as_str(), "大阪");
    assert_eq!(c.lon, 135.5);
    assert_eq!(c.lat, 35.6812);
    assert!(!c.sd_fast);
    assert_eq!(c.layout, config::LayoutName::Dock);
    // 同じ内容をもう一度当てても変わらない (往復で安定)
    assert_eq!(rewrite(&new, &[("place", Some("大阪")), ("lon", Some("135.5")), ("layout", Some("dock"))]), new);
}

#[test]
fn rewrite_crlf_bom_duplicates_and_removal() {
    let old = "\u{feff}LAT=1\r\nimages=A.BMP,B.BMP\r\nlat=2\r\nmessage=古い # 文字\r\nscroll=2";
    let new = rewrite(old, &[("lat", Some("3")), ("images", None), ("message", Some("新しい #1")), ("tz", Some("+9"))]);
    // BOM と CRLF を保つ。同じキーは全部 (最後が勝つので)、キーの書き方 (LAT) も残す。None は行ごと消す。
    // message= は行末まで値なので、古い「# 文字」はコメントとして残さない。最後の行に改行が無ければ足してから追記
    assert_eq!(new, "\u{feff}LAT=3\r\nlat=3\r\nmessage=新しい #1\r\nscroll=2\r\ntz=+9\r\n");
    let (c, _) = TickerConfig::parse(new.as_bytes());
    assert_eq!(c.lat, 3.0);
    assert!(c.images.is_empty());
    assert!(c.local_message);
    assert_eq!(config::message_text(new.as_bytes()), Some("新しい #1"));
    // 空のファイル / 改行で終わらないファイルへ追記
    assert_eq!(rewrite("", &[("slide", Some("60"))]), "slide=60\n");
    assert_eq!(rewrite("# x", &[("slide", Some("60"))]), "# x\nslide=60\n");
    // 行を消すだけ (無ければ何もしない)
    assert_eq!(rewrite("a=1\n", &[("images", None)]), "a=1\n");
    // コメントの中のキーは触らない
    assert_eq!(rewrite("#place=昔\n", &[("place", Some("京都"))]), "#place=昔\nplace=京都\n");
}

#[test]
fn rewrite_limits() {
    let long = "x".repeat(config::CONFIG_MAX - 5);
    let mut out = [0u8; config::CONFIG_MAX];
    assert_eq!(
        config::rewrite(long.as_bytes(), &[("place", Some("大阪"))], &mut out),
        Err(RewriteError::TooLong)
    );
    assert_eq!(config::rewrite(b"\xff\xfe", &[], &mut out), Err(RewriteError::NotUtf8));
    let mut tiny = [0u8; 4];
    assert_eq!(config::rewrite(b"lat=1\n", &[], &mut tiny), Err(RewriteError::TooLong));
}

#[test]
fn settings_values_are_validated_by_the_ticker_txt_rules() {
    for (k, v) in [
        ("lat", "35.1"),
        ("lon", "-122.4"),
        ("tz", "+5:30"),
        ("tz", "-8"),
        ("place", "東京"),
        ("layout", "classic"),
        ("status", "compact"),
        ("slide", "0"),
        ("slide", "3600"),
        ("scroll", "8"),
        ("images", "A.BMP,IMG00001.BMP"),
        ("message_url", "https://example.com/m.txt"),
        ("message", "今日は #晴れ"),
    ] {
        assert!(config::valid_value(k, v), "{k}={v}");
    }
    for (k, v) in [
        ("lat", "91"),
        ("lon", "abc"),
        ("tz", "+15"),
        ("layout", "grid"),
        ("slide", "3"),
        ("scroll", "9"),
        ("images", "LONGNAME1.BMP"),
        ("message_url", "ftp://x"),
        ("place", "東#京"),
        ("place", "改\n行"),
        ("message", "改\r行"),
        ("unknown", "1"),
        ("", "1"),
        ("la t", "1"),
    ] {
        assert!(!config::valid_value(k, v), "{k}={v}");
    }
    assert!(!config::valid_value("message", &"あ".repeat(200)));
}

#[test]
fn local_message_key() {
    let (c, any) = TickerConfig::parse("message=".as_bytes());
    assert!(any && !c.local_message);
    assert_eq!(config::message_text(b"message=   "), None);
    let (c, _) = TickerConfig::parse("message=a#b\nmessage=".as_bytes());
    assert!(!c.local_message, "the last line wins");
    assert_eq!(config::message_text("message=a#b\n#message=x".as_bytes()), Some("a#b"));
}

// ============================================================
// アクセスコードと送り元
// ============================================================

#[test]
fn access_code_and_lockout() {
    assert_eq!(auth::code_from_random(0), 100_000);
    assert_eq!(auth::code_from_random(u32::MAX), 100_000 + u32::MAX % 900_000);
    assert!((100_000..1_000_000).contains(&auth::code_from_random(123_456_789)));
    let mut g = Guard::new(482_913);
    assert_eq!(g.check(0, None), Err(AuthError::Missing));
    assert_eq!(g.check(0, Some("")), Err(AuthError::Missing));
    assert_eq!(g.check(0, Some("482913")), Ok(()));
    assert_eq!(g.check(0, Some(" 482913 ")), Ok(()));
    // 桁が違う / 前後に何か付いている / 途中まで一致
    assert_eq!(g.check(1, Some("48291")), Err(AuthError::Wrong { left: 4 }));
    assert_eq!(g.check(2, Some("4829130")), Err(AuthError::Wrong { left: 3 }));
    assert_eq!(g.check(3, Some("482914")), Err(AuthError::Wrong { left: 2 }));
    assert_eq!(g.check(4, Some("000000")), Err(AuthError::Wrong { left: 1 }));
    // 5 回目で 30 s 締め出し。締め出し中は正しいコードも断る
    assert_eq!(g.check(5, Some("111111")), Err(AuthError::Locked { secs: 30 }));
    assert_eq!(g.check(1_000, Some("482913")), Err(AuthError::Locked { secs: 30 }));
    assert_eq!(g.locked(29_500), Some(1));
    assert_eq!(g.locked(30_005), None);
    // 明けて また 5 回間違えると 60 s (倍々)
    for i in 0..4 {
        assert!(matches!(g.check(31_000 + i, Some("1")), Err(AuthError::Wrong { .. })));
    }
    assert_eq!(g.check(31_010, Some("1")), Err(AuthError::Locked { secs: 60 }));
    // 正しいコードで数え直し
    assert_eq!(g.check(100_000, Some("482913")), Ok(()));
    for i in 0..4 {
        assert!(matches!(g.check(100_001 + i, Some("1")), Err(AuthError::Wrong { .. })));
    }
    assert_eq!(g.check(100_010, Some("1")), Err(AuthError::Locked { secs: 30 }));
    // 上限 15 分
    let mut g = Guard::new(123_456);
    let mut t = 0u32;
    let mut last = 0;
    for _ in 0..12 {
        for _ in 0..auth::MAX_FAILURES {
            if let Err(AuthError::Locked { secs }) = g.check(t, Some("0")) {
                last = secs;
            }
            t += 1;
        }
        t += last * 1000 + 1;
    }
    assert_eq!(last, auth::LOCK_MAX_MS / 1000);
    // 6 桁 → 総当たりは 30 s に 5 回から始まって倍々に遅くなる: 1 日で試せるのは 1,000 回に満たない
    let mut g = Guard::new(999_999);
    let mut t = 0u32;
    let mut tries = 0;
    while t < 86_400_000 {
        match g.check(t, Some("000000")) {
            Err(AuthError::Wrong { .. }) => tries += 1,
            Err(AuthError::Locked { secs }) if secs > 0 => {
                tries += 1;
                t = t.saturating_add(secs * 1000);
            }
            _ => {}
        }
        t += 1;
    }
    assert!(tries < 1_000, "{tries} guesses a day");
}

#[test]
fn host_and_origin_checks() {
    let ip = [192, 168, 200, 130];
    assert!(auth::host_is_board(Some("192.168.200.130"), ip));
    assert!(auth::host_is_board(Some("192.168.200.130:80"), ip));
    for bad in [
        None,
        Some("evil.example"),
        Some("192.168.200.13"),
        Some("192.168.200.130.nip.io"),
        Some("192.168.200.130:8080"),
        Some("0192.168.200.130"),
        Some("192.168.200"),
        Some(""),
    ] {
        assert!(!auth::host_is_board(bad, ip), "{bad:?}");
    }
    let host = Some("192.168.200.130");
    assert!(auth::same_origin(host, None, None), "curl など (Origin なし)");
    assert!(auth::same_origin(host, Some("http://192.168.200.130"), Some("same-origin")));
    assert!(auth::same_origin(Some("192.168.200.130:80"), Some("http://192.168.200.130"), None));
    assert!(!auth::same_origin(host, Some("https://evil.example"), None));
    assert!(!auth::same_origin(host, Some("http://192.168.200.130.evil"), None));
    assert!(!auth::same_origin(host, Some("null"), None));
    assert!(!auth::same_origin(host, None, Some("cross-site")));
    assert!(!auth::same_origin(host, Some("http://192.168.200.130"), Some("same-site")));
    assert!(auth::same_origin(host, None, Some("none")), "アドレス欄から直接");
}

// ============================================================
// 写真の追加: BMP の検査と名前
// ============================================================

/// ページの JS (encodeBmp) と同じ形の 400×96 24 bit BMP のヘッダ
fn page_bmp_header() -> Vec<u8> {
    let mut h = vec![0u8; 54];
    h[0..2].copy_from_slice(b"BM");
    h[2..6].copy_from_slice(&upload::UPLOAD_SIZE.to_le_bytes());
    h[10..14].copy_from_slice(&54u32.to_le_bytes());
    h[14..18].copy_from_slice(&40u32.to_le_bytes());
    h[18..22].copy_from_slice(&400i32.to_le_bytes());
    h[22..26].copy_from_slice(&96i32.to_le_bytes());
    h[26..28].copy_from_slice(&1u16.to_le_bytes());
    h[28..30].copy_from_slice(&24u16.to_le_bytes());
    h[34..38].copy_from_slice(&(400u32 * 96 * 3).to_le_bytes());
    h[38..42].copy_from_slice(&2835u32.to_le_bytes());
    h[42..46].copy_from_slice(&2835u32.to_le_bytes());
    h
}

#[test]
fn upload_bmp_header_is_checked() {
    assert_eq!(upload::UPLOAD_SIZE, 115_254);
    let h = page_bmp_header();
    assert_eq!(upload::check_header(&h, upload::UPLOAD_SIZE), Ok(()));
    // 上から下 (高さが負) も可、画像の大きさ欄 0 も可
    let mut td = h.clone();
    td[22..26].copy_from_slice(&(-96i32).to_le_bytes());
    td[34..38].copy_from_slice(&0u32.to_le_bytes());
    assert_eq!(upload::check_header(&td, upload::UPLOAD_SIZE), Ok(()));
    // 送られてくる長さが違う
    assert!(upload::check_header(&h, upload::UPLOAD_SIZE + 1).is_err());
    assert!(upload::check_header(&h[..53], upload::UPLOAD_SIZE).is_err());
    let bad = |i: usize, bytes: &[u8]| {
        let mut b = h.clone();
        b[i..i + bytes.len()].copy_from_slice(bytes);
        upload::check_header(&b, upload::UPLOAD_SIZE)
    };
    assert!(bad(0, b"BA").is_err());
    assert!(bad(2, &1u32.to_le_bytes()).is_err());
    assert!(bad(10, &138u32.to_le_bytes()).is_err());
    assert!(bad(14, &124u32.to_le_bytes()).is_err());
    assert!(bad(18, &401i32.to_le_bytes()).is_err());
    assert!(bad(22, &95i32.to_le_bytes()).is_err());
    assert!(bad(26, &2u16.to_le_bytes()).is_err());
    assert!(bad(28, &32u16.to_le_bytes()).is_err());
    assert!(bad(30, &3u32.to_le_bytes()).is_err());
    assert!(bad(34, &5u32.to_le_bytes()).is_err());
    // 本物の 400×96 のサンプル (convert_image_to_bmp.py で作ったもの) も通る
    let sample = std::fs::read("../../IMAGE.BMP").unwrap();
    assert_eq!(upload::check_header(&sample[..54], sample.len() as u32), Ok(()));
}

#[test]
fn upload_names() {
    assert_eq!(upload::short_name_from("sunset.jpg").as_deref(), Some("SUNSET.BMP"));
    assert_eq!(upload::short_name_from("My Photo 2024-05.heic").as_deref(), Some("MYPHOTO2.BMP"));
    assert_eq!(upload::short_name_from("a_b-c.png").as_deref(), Some("A_B-C.BMP"));
    assert_eq!(upload::short_name_from("_hidden.png").as_deref(), Some("HIDDEN.BMP"));
    assert_eq!(upload::short_name_from("夕焼け.jpg"), None);
    assert_eq!(upload::short_name_from(""), None);
    assert_eq!(upload::short_name_from(".bmp").as_deref(), Some("BMP.BMP"));
    assert_eq!(upload::numbered(1).as_str(), "IMG00001.BMP");
    assert_eq!(upload::variant("SUNSET.BMP", 2).as_str(), "SUNSET~2.BMP");
    assert_eq!(upload::variant("ABCDEFGH.BMP", 9).as_str(), "ABCDEF~9.BMP");
    for n in [
        upload::numbered(99_999),
        upload::variant("ABCDEFGH.BMP", 12),
        upload::short_name_from("abcdefghijkl.png").unwrap(),
    ] {
        assert!(upload::is_bmp_name(&n), "{n}");
        assert!(config::is_short_name(&n), "{n}");
    }
    assert!(upload::is_bmp_name("image.bmp"));
    for bad in ["TICKER.TXT", "WIFI.TXT", "_X.BMP", "../A.BMP", "A.BMPX", "TOOLONGNAME.BMP", "A B.BMP", ".BMP"] {
        assert!(!upload::is_bmp_name(bad), "{bad}");
    }
}

// ============================================================
// JSON
// ============================================================

#[test]
fn json_writer_escapes_and_overflows() {
    let mut buf = [0u8; 256];
    let mut j = Json::new(&mut buf);
    j.begin_object();
    j.field_str("s", "a\"b\\c\nd</script>\u{1}東京");
    j.field_int("n", -5);
    j.key("f");
    j.float(35.68123, 4);
    j.key("nan");
    j.float(f32::NAN, 1);
    j.key("arr");
    j.begin_array();
    j.str("x");
    j.int(1);
    j.begin_object();
    j.field_bool("ok", true);
    j.end_object();
    j.end_array();
    j.key("z");
    j.null();
    j.end_object();
    assert!(!j.overflow);
    let text = std::str::from_utf8(j.as_bytes()).unwrap().to_string();
    assert_eq!(
        text,
        r#"{"s":"a\"b\\c\nd\u003c/script>\u0001東京","n":-5,"f":35.6812,"nan":null,"arr":["x",1,{"ok":true}],"z":null}"#
    );
    let mut small = [0u8; 8];
    let mut j = Json::new(&mut small);
    j.begin_object();
    j.field_str("long", "value");
    assert!(j.overflow);
}

// ============================================================
// 生存確認 (Who::Web は処理中だけ監視する)
// ============================================================

#[test]
fn web_heartbeat_is_parked_while_idle() {
    use health::{Limits, PARKED, Who, stalled};
    let l = Limits::TICKER;
    assert!(Who::Web.starts_parked());
    assert!(!Who::Main.starts_parked());
    // 待ち受け中 (PARKED) は何時間でも止まっていない
    assert_eq!(stalled(10_000_000, &[9_999_900, 9_999_000, 9_999_990, PARKED], &l), None);
    // 処理を始めたら 20 s で止まったと見なす
    assert_eq!(stalled(100_000, &[99_900, 99_000, 99_990, 85_000], &l), None);
    assert_eq!(stalled(100_000, &[99_900, 99_000, 99_990, 79_000], &l), Some((Who::Web, 21_000)));
    assert_eq!(health::wdt_stage(Who::Web), health::STAGE_WDT_WEB);
    let mut s: heapless::String<80> = heapless::String::new();
    let r = health::decode(health::STAGE_WDT_WEB, 21_000, 0, None).unwrap();
    health::write_last_reset(&mut s, &r, 600, 1);
    assert_eq!(s.as_str(), "last reset: wdt: web stalled 21s @60s");
}
