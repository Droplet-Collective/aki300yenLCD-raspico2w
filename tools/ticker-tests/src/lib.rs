//! 本体クレートの純粋なモジュールを取り込んでホストでテストする (`cargo test`)
//!
//! `boot_sim` (0.4.2〜) は起動の流れ (bootrom の A/B 選択 + TBYB、ファームウェアの通常 / 回復 / 他方区画への
//! 切り替え) を模擬し、本物の `boot_policy` で故障を注入して「必ず OTA 確認まで進む」「壊れた版を buy しない」を確かめる。

#[path = "../../../src/boot_policy.rs"]
pub mod boot_policy;
#[cfg(test)]
mod boot_sim;
#[path = "../../../src/ticker/civil.rs"]
pub mod civil;
#[path = "../../../src/ticker/config.rs"]
pub mod config;
#[path = "../../../src/ticker/digits.rs"]
pub mod digits;
#[path = "../../../src/ticker/health.rs"]
pub mod health;
#[path = "../../../src/ticker/sntp.rs"]
pub mod sntp;
#[path = "../../../src/ticker/weather.rs"]
pub mod weather;
#[path = "../../../src/font/shinonome.rs"]
pub mod shinonome;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_known_epochs() {
        // 1970-01-01 (木)
        let dt = civil::from_unix(0, 0);
        assert_eq!((dt.year, dt.month, dt.day, dt.weekday), (1970, 1, 1, 4));
        assert_eq!((dt.hour, dt.minute, dt.second), (0, 0, 0));
        // 2000-02-29 (火) 12:34:56 UTC = 951827696
        let dt = civil::from_unix(951_827_696, 0);
        assert_eq!((dt.year, dt.month, dt.day, dt.weekday), (2000, 2, 29, 2));
        assert_eq!((dt.hour, dt.minute, dt.second), (12, 34, 56));
        // 2026-09-29 12:55:48 UTC = 1790686548 → JST 21:55:48 (火)
        let dt = civil::from_unix(1_790_686_548, 9 * 3600);
        assert_eq!((dt.year, dt.month, dt.day), (2026, 9, 29));
        assert_eq!((dt.hour, dt.minute, dt.second), (21, 55, 48));
        assert_eq!(dt.weekday_ja(), "火");
        assert_eq!(dt.weekday_en(), "Tue");
        // 日付をまたぐオフセット: 2026-12-31 20:00 UTC + 9h = 2027-01-01 05:00 (金)
        let dt = civil::from_unix(1_798_747_200, 9 * 3600);
        assert_eq!((dt.year, dt.month, dt.day, dt.hour), (2027, 1, 1, 5));
        assert_eq!(dt.weekday_ja(), "金");
        // 負のオフセット: 1970-01-01 00:00 UTC − 5h = 1969-12-31 19:00 (水)
        let dt = civil::from_unix(0, -5 * 3600);
        assert_eq!((dt.year, dt.month, dt.day, dt.hour, dt.weekday), (1969, 12, 31, 19, 3));
        // 2038-01-19 03:14:08 (32 bit を超える)
        let dt = civil::from_unix(2_147_483_648, 0);
        assert_eq!((dt.year, dt.month, dt.day, dt.hour, dt.minute, dt.second), (2038, 1, 19, 3, 14, 8));
    }

    #[test]
    fn civil_from_days_matches_python() {
        // python: datetime.date.fromordinal(719163 + d) で確認済みの値
        assert_eq!(civil::civil_from_days(19_999), (2024, 10, 3));
        assert_eq!(civil::civil_from_days(-1), (1969, 12, 31));
        assert_eq!(civil::civil_from_days(-719_468), (0, 3, 1));
        assert_eq!(civil::civil_from_days(11_016), (2000, 2, 29));
    }

    #[test]
    fn config_defaults_and_parse() {
        let (c, any) = config::TickerConfig::parse(b"");
        assert!(!any);
        assert_eq!(c, config::TickerConfig::default());
        assert_eq!(c.place.as_str(), "東京");
        assert_eq!(c.tz_offset_secs, 32_400);
        assert_eq!(c.scroll_px, 1);
        assert_eq!(c.message_url.as_str(), config::DEFAULT_MESSAGE_URL);

        let text = "\u{feff}# comment\r\nLAT = 43.0621 # sapporo\r\nlon=141.3544\r\ntz=+09:00\r\nplace=札幌\r\nscroll=2\r\nmessage_url=https://example.com/m.txt\r\nbogus=1\r\n";
        let (c, any) = config::TickerConfig::parse(text.as_bytes());
        assert!(any);
        assert!((c.lat - 43.0621).abs() < 1e-4);
        assert!((c.lon - 141.3544).abs() < 1e-4);
        assert_eq!(c.tz_offset_secs, 32_400);
        assert_eq!(c.place.as_str(), "札幌");
        assert_eq!(c.scroll_px, 2);
        assert_eq!(c.message_url.as_str(), "https://example.com/m.txt");

        // 不正な値は既定のまま
        let (c, any) = config::TickerConfig::parse(b"lat=999\nlon=abc\ntz=+30\nscroll=0\nmessage_url=ftp://x\nplace=\n");
        assert!(!any);
        assert_eq!(c, config::TickerConfig::default());
    }

    #[test]
    fn config_slideshow_keys() {
        use config::{LayoutName, StatusMode};
        let d = config::TickerConfig::default();
        assert_eq!((d.slide_secs, d.layout, d.status, d.sd_fast), (30, LayoutName::Glass, StatusMode::Auto, true));
        assert!(d.images.is_empty());
        let text = "slide=0\nimages= IMAGE.BMP , image2.bmp,bad name.bmp,TOOLONGNAME.BMP\nlayout=Dock\nstatus=full\nsdfast=0\n";
        let (c, any) = config::TickerConfig::parse(text.as_bytes());
        assert!(any);
        assert_eq!(c.slide_secs, 0);
        assert_eq!(config::image_names(&c.images).collect::<Vec<_>>(), ["IMAGE.BMP", "image2.bmp"]);
        assert_eq!((c.layout, c.status, c.sd_fast), (LayoutName::Dock, StatusMode::Full, false));
        // 範囲外 / 不正は既定のまま
        let (c, any) = config::TickerConfig::parse(b"slide=3\nimages=,,\nlayout=fancy\nstatus=on\nsdfast=yes\n");
        assert!(!any);
        assert_eq!(c, d);
        assert!(config::is_short_name("A.BMP") && config::is_short_name("PHOTO_01.BMP"));
        assert!(!config::is_short_name("PHOTO.JPEG") && !config::is_short_name(".BMP") && !config::is_short_name("NOEXT"));
    }

    #[test]
    fn tz_forms() {
        assert_eq!(config::parse_tz("+9"), Some(32_400));
        assert_eq!(config::parse_tz("9"), Some(32_400));
        assert_eq!(config::parse_tz("-5.5"), Some(-19_800));
        assert_eq!(config::parse_tz("+05:30"), Some(19_800));
        assert_eq!(config::parse_tz("0"), Some(0));
        assert_eq!(config::parse_tz("+15"), None);
        assert_eq!(config::parse_tz("x"), None);
    }

    const OPEN_METEO_BODY: &str = r#"{"latitude":35.7,"longitude":139.75,"generationtime_ms":15.41,"utc_offset_seconds":32400,"timezone":"Asia/Tokyo","timezone_abbreviation":"GMT+9","elevation":10.0,"current_units":{"time":"iso8601","interval":"seconds","temperature_2m":"°C","weather_code":"wmo code"},"current":{"time":"2026-09-29T22:00","interval":900,"temperature_2m":19.1,"weather_code":2},"daily_units":{"time":"iso8601","temperature_2m_max":"°C","temperature_2m_min":"°C","precipitation_probability_max":"%"},"daily":{"time":["2026-09-29"],"temperature_2m_max":[21.9],"temperature_2m_min":[19.1],"precipitation_probability_max":[100]}}"#;

    #[test]
    fn weather_parse_real_body() {
        let w = weather::Weather::parse(OPEN_METEO_BODY.as_bytes()).expect("parse");
        assert!((w.temperature - 19.1).abs() < 1e-5);
        assert_eq!(w.code, 2);
        assert_eq!(w.condition_ja(), "晴れ時々くもり");
        assert!((w.max.unwrap() - 21.9).abs() < 1e-5);
        assert!((w.min.unwrap() - 19.1).abs() < 1e-5);
        assert_eq!(w.rain_pct, Some(100));
        // null も受ける
        let body = r#"{"current":{"temperature_2m":-3.5,"weather_code":71},"daily":{"temperature_2m_max":[null],"temperature_2m_min":[-8.0],"precipitation_probability_max":[null]}}"#;
        let w = weather::Weather::parse(body.as_bytes()).expect("parse null");
        assert_eq!(w.max, None);
        assert_eq!(w.min, Some(-8.0));
        assert_eq!(w.rain_pct, None);
        assert_eq!(w.condition_ja(), "小雪");
        assert!(weather::Weather::parse(b"{\"error\":true}").is_none());
        let mut s: heapless::String<16> = heapless::String::new();
        weather::format_temp(&mut s, -3.46);
        assert_eq!(s.as_str(), "-3.5");
    }

    #[test]
    fn weather_url_and_codes() {
        let url = weather::request_url("https", 35.6812, 139.7671);
        assert!(url.starts_with("https://api.open-meteo.com/v1/forecast?latitude=35.6812&longitude=139.7671&current="));
        assert!(url.contains("&timezone=auto&forecast_days=1"));
        assert_eq!(weather::condition_ja(0), "快晴");
        assert_eq!(weather::condition_ja(95), "雷雨");
        assert_eq!(weather::condition_ja(42), "不明");
    }

    #[test]
    fn sntp_packets() {
        let req = sntp::request_packet();
        assert_eq!(req.len(), 48);
        assert_eq!(req[0], 0x23);
        assert!(req[1..].iter().all(|&b| b == 0));
        // 応答: mode 4, stratum 1, transmit = 2026-09-29T12:55:48Z = unix 1790686548 → ntp 3999675348
        let mut reply = [0u8; 48];
        reply[0] = 0x24;
        reply[1] = 1;
        reply[40..44].copy_from_slice(&(1_790_686_548u32 + 2_208_988_800u32).to_be_bytes());
        reply[44..48].copy_from_slice(&0x8000_0000u32.to_be_bytes());
        let r = sntp::parse_reply(&reply).unwrap();
        assert_eq!(r.unix_secs, 1_790_686_548);
        assert_eq!(r.millis, 500);
        assert_eq!(r.stratum, 1);
        // kiss-o'-death / 短い / 時刻 0 は拒否
        reply[1] = 0;
        assert_eq!(sntp::parse_reply(&reply), Err(sntp::SntpError::BadReply));
        reply[1] = 2;
        assert_eq!(sntp::parse_reply(&reply[..40]), Err(sntp::SntpError::BadReply));
        reply[40..44].copy_from_slice(&0u32.to_be_bytes());
        assert_eq!(sntp::parse_reply(&reply), Err(sntp::SntpError::BadReply));
        // 2036 年以降 (MSB=0) は era 1 として扱う: ntp 秒 100 → unix 2^32 + 100 − 2208988800
        reply[40..44].copy_from_slice(&100u32.to_be_bytes());
        assert_eq!(sntp::parse_reply(&reply).unwrap().unix_secs, (1i64 << 32) + 100 - 2_208_988_800);
    }

    #[test]
    fn shinonome_lookup() {
        assert_eq!(shinonome::glyph_count(), 7047);
        // 半角 'A' (shnm7x14r): 送り幅 7、上位バイトだけ。行 2〜11 に字体、行 0・1・12・13 は空
        let a = shinonome::glyph('A').unwrap();
        assert_eq!(a.advance, 7);
        assert_eq!(a.rows[0], 0);
        assert_eq!(a.rows[2], 0b0011_0000 << 8);
        assert_eq!(a.rows[8], 0b1000_0100 << 8);
        assert_eq!(a.rows[13], 0);
        // 全角 '東' (shnmk14、JIS 0x456C): 送り幅 14、行 2 が横棒、行 13 に縦棒の下端
        let east = shinonome::glyph('東').unwrap();
        assert_eq!(east.advance, 14);
        assert_eq!(east.rows[0], 0b0000_0001_0000_0000);
        assert_eq!(east.rows[2], 0b0111_1111_1111_1100);
        assert_eq!(east.rows[13], 0b0000_0001_0000_0000);
        for ch in ['あ', '℃', '火', '、', '。', '…', 'ｱ', '□'] {
            assert!(shinonome::glyph(ch).is_some(), "{ch}");
        }
        // 半角カナは 7 px、全角は 14 px。～ (U+FF5E) と 〜 (U+301C) は同じグリフ
        assert_eq!(shinonome::advance_of('ｱ'), 7);
        assert_eq!(shinonome::advance_of('あ'), 14);
        assert_eq!(shinonome::glyph('～'), shinonome::glyph('〜'));
        assert_eq!(shinonome::glyph('－'), shinonome::glyph('−'));
        assert!(shinonome::glyph('\u{1F600}').is_none()); // 絵文字は無い → 代替
        assert_eq!(shinonome::fallback_glyph().advance, 14);
        assert_eq!(shinonome::advance_of('\u{1F600}'), 14);
        // 幅: "OTA 0.3.1" は半角 9 文字 = 63 px、"東京" は 28 px、混在は足し算
        assert_eq!(shinonome::text_width("OTA 0.3.1"), 63);
        assert_eq!(shinonome::text_width("東京"), 28);
        assert_eq!(shinonome::text_width("最高 21.9℃"), 28 + 5 * 7 + 14);
        assert_eq!(shinonome::text_width("2026/09/29 (火)"), 13 * 7 + 14);
        // 描画: 'A' を (10, 20) に描くと最初の点は行 2 の列 2・3 → (12, 22), (13, 22)。列 0 行 0 は無い
        let mut pts = Vec::new();
        let adv = shinonome::draw_text("A", 10, 20, 400, |x, y| pts.push((x, y)));
        assert_eq!(adv, 17);
        assert!(pts.contains(&(12, 22)) && pts.contains(&(13, 22)));
        assert!(!pts.contains(&(10, 20)));
        assert!(pts.iter().all(|&(x, y)| (10..17).contains(&x) && (20..34).contains(&y)));
        // 全角は 14 列すべて閉じる (bit15 → 列 0、bit2 → 列 13)。半角は 7 列より右に描かない
        let mut pts = Vec::new();
        shinonome::draw_text("東", 0, 0, 400, |x, y| pts.push((x, y)));
        assert!(pts.iter().all(|&(x, y)| (0..14).contains(&x) && (0..14).contains(&y)));
        assert!(pts.contains(&(1, 2)) && pts.contains(&(13, 2)));
        // 右端クリップ
        let mut n = 0;
        shinonome::draw_text("東京", 390, 0, 400, |x, _| {
            assert!(x < 400);
            n += 1;
        });
        assert!(n > 0);
    }

    #[test]
    fn digits_layout() {
        assert_eq!(digits::text_width("21:53:44", 4), 4 * (6 * 6 + 2 * 4) - 4);
        let mut n = 0;
        digits::draw_text("8", 0, 0, 1, |x, y| {
            assert!((0..5).contains(&x) && (0..7).contains(&y));
            n += 1;
        });
        assert_eq!(n, 17);
    }

    // ---- 0.4.1: ticker.txt が無い / 空 / 読めない / SD が無い、はどれも東京の既定値で続ける ----

    #[test]
    fn config_load_missing_empty_unreadable_nocard() {
        let default = config::TickerConfig::default();

        let (c, note) = config::load(config::ConfigSource::NotFound);
        assert_eq!(c, default);
        assert!(note.starts_with("ticker.txt not found"), "{note}");

        let (c, note) = config::load(config::ConfigSource::Read(b""));
        assert_eq!(c, default);
        assert_eq!(note.as_str(), "ticker.txt is empty, using Tokyo defaults");

        let (c, note) = config::load(config::ConfigSource::Read(b"  \r\n\n"));
        assert_eq!(c, default);
        assert!(note.contains("empty"));

        let (c, note) = config::load(config::ConfigSource::Read(b"# only a comment\nfoo=bar\n"));
        assert_eq!(c, default);
        assert!(note.contains("no valid keys"));

        let (c, note) = config::load(config::ConfigSource::Read(&[0xff, 0xfe, 0x00, 0x80]));
        assert_eq!(c, default);
        assert!(note.contains("no valid keys"));

        let (c, note) = config::load(config::ConfigSource::ReadFailed("read error"));
        assert_eq!(c, default);
        assert_eq!(note.as_str(), "ticker.txt: read error, using Tokyo defaults");

        let (c, note) = config::load(config::ConfigSource::NoCard("SD INIT FAILED"));
        assert_eq!(c, default);
        assert_eq!(note.as_str(), "SD: SD INIT FAILED (ticker.txt skipped, using Tokyo)");

        // 試験用 debug_crash (0.4.2〜)
        let (c, _) = config::load(config::ConfigSource::Read(b"debug_crash=ota\n"));
        assert_eq!(c.debug_crash, config::DebugCrash::Ota);
        let (c, _) = config::load(config::ConfigSource::Read(b"debug_crash = Slideshow # test\n"));
        assert_eq!(c.debug_crash, config::DebugCrash::Slideshow);
        let (c, _) = config::load(config::ConfigSource::Read(b"debug_crash=boot\n"));
        assert_eq!(c.debug_crash, config::DebugCrash::Boot);
        let (c, _) = config::load(config::ConfigSource::Read(b"debug_crash=bogus\nplace=x\n"));
        assert_eq!(c.debug_crash, config::DebugCrash::None);
        assert_eq!(default.debug_crash, config::DebugCrash::None);

        // 正常なファイルは注意なし
        let (c, note) = config::load(config::ConfigSource::Read("place=大阪\nlat=34.7\n".as_bytes()));
        assert_eq!(c.place.as_str(), "大阪");
        assert!(note.is_empty());
    }

    // ---- 0.4.1: 生存確認 (ウォッチドッグの再ロード条件) ----

    #[test]
    fn health_stalled_detection() {
        use health::{Limits, Who, stalled};
        let l = Limits::TICKER;
        // 全員さっき知らせた
        assert_eq!(stalled(10_000, &[9_900, 9_000, 9_990], &l), None);
        // 描画が 5 s を超えて止まった
        assert_eq!(stalled(20_000, &[19_900, 19_000, 14_000], &l), Some((Who::Render, 6_000)));
        // main が 90 s を超えて止まった (取得 / 描画は生きている)
        assert_eq!(stalled(200_000, &[100_000, 199_000, 199_990], &l), Some((Who::Main, 100_000)));
        // 取得タスク
        assert_eq!(stalled(200_000, &[199_000, 100_000, 199_990], &l), Some((Who::Jobs, 100_000)));
        // ちょうど上限は許す
        assert_eq!(stalled(95_000, &[5_000, 5_000, 90_000], &l), None);
        // 監視開始直後に書かれた「未来の」値 (割り込みとの競合) は 0 扱い
        assert_eq!(stalled(1_000, &[1_004, 1_000, 1_002], &l), None);
        // 49.7 日で u32 の ms が一周しても判定できる
        let now = 5u32;
        assert_eq!(stalled(now, &[u32::MAX - 100, u32::MAX - 100, u32::MAX - 10], &l), None);
        assert_eq!(stalled(now, &[u32::MAX - 100, u32::MAX - 100, u32::MAX - 6_000], &l), Some((Who::Render, 6_006)));
    }

    /// 監視 (supervisor::on_frame と同じ判定) を 0.5 s ごとに回す簡単なシミュレーション。
    /// ふだんの動き (描画 60 Hz、main 250 ms、取得は 20 s の TLS 待ちや 0.4 s のフラッシュ消去を含む) では
    /// 一度もリセットせず、描画が止まったら 5〜5.5 s で、main が止まったら 90〜90.5 s でリセットする。
    #[test]
    fn health_supervisor_simulation() {
        use health::{Limits, Who, stalled};
        fn run(stop: Option<(Who, u32)>, until_ms: u32) -> Option<(u32, Who)> {
            let limits = Limits::TICKER;
            let mut last = [0u32; 3];
            let mut t = 0u32;
            while t <= until_ms {
                let stopped = |who: Who| stop.is_some_and(|(w, at)| w == who && t >= at);
                // 描画: 16.7 ms ごと。ただしフラッシュ消去 (割り込みも止まる 0.4 s) を 10 s ごとに
                let erasing = (t % 10_000) < 400;
                if !stopped(Who::Render) && !erasing && t.is_multiple_of(17) {
                    last[Who::Render as usize] = t;
                }
                // main: 250 ms ごと。ただし join + DHCP 待ち (最長 25 s) を 60〜85 s に
                if !stopped(Who::Main) && !(60_000..85_000).contains(&t) && t.is_multiple_of(250) {
                    last[Who::Main as usize] = t;
                }
                // 取得: 250 ms ごと。ただし TLS の取得待ち (20 s で打ち切り) を 100〜120 s に
                if !stopped(Who::Jobs) && !(100_000..120_000).contains(&t) && t.is_multiple_of(250) {
                    last[Who::Jobs as usize] = t;
                }
                // 監視は割り込みなので、フラッシュ消去中は遅れる
                if t.is_multiple_of(500) && !erasing
                    && let Some((who, _)) = stalled(t, &last, &limits) {
                        return Some((t, who));
                    }
                t += 1;
            }
            None
        }
        assert_eq!(run(None, 300_000), None);
        let (at, who) = run(Some((Who::Render, 30_000)), 300_000).unwrap();
        assert_eq!(who, Who::Render);
        assert!((35_000..=35_600).contains(&at), "{at}");
        let (at, who) = run(Some((Who::Main, 130_000)), 400_000).unwrap();
        assert_eq!(who, Who::Main);
        assert!((220_000..=220_600).contains(&at), "{at}");
        let (at, who) = run(Some((Who::Jobs, 130_000)), 400_000).unwrap();
        assert_eq!(who, Who::Jobs);
        assert!((220_000..=220_600).contains(&at), "{at}");
    }

    // ---- 0.4.1: 前回のリセット理由の表示 ----

    #[test]
    fn health_last_reset_lines() {
        use health::*;
        let line = |r: LastReset<'_>, up: u32, streak: u32| {
            let mut s: heapless::String<80> = heapless::String::new();
            write_last_reset(&mut s, &r, up, streak);
            assert!(s.len() <= 66, "too long for the status line: {s}");
            s
        };
        let panic = decode(STAGE_PANIC, 123, 0x1004_0000, Some("src/ui/slide.rs")).unwrap();
        assert_eq!(line(panic, 1234, 1).as_str(), "last reset: panic src/ui/slide.rs:123 @123s");
        let long = "/home/runner/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/embedded-tls-0.18.0/src/connection.rs";
        let panic = decode(STAGE_PANIC, 77, 0, Some(long)).unwrap();
        assert_eq!(line(panic, 50, 2).as_str(), "last reset: panic embedded-tls-0.18.0/connection.rs:77 @5s #2");
        assert_eq!(
            health::split_crate_path(long),
            (Some("embedded-tls-0.18.0"), "connection.rs")
        );
        // 長い行番号 / 稼働時間でも 66 桁に収まる
        let panic = decode(STAGE_PANIC, 12345, 0, Some(long)).unwrap();
        let s = line(panic, 3_600_000, 12);
        assert!(s.ends_with(":12345 @360000s #12"), "{s}");
        let panic = decode(STAGE_PANIC, 77, 0, None).unwrap();
        assert_eq!(line(panic, 50, 0).as_str(), "last reset: panic ?:77 @5s");
        let hf = decode(STAGE_HARDFAULT, 0x1000_abcd, 0x1000_1235, None).unwrap();
        assert_eq!(line(hf, 312, 1).as_str(), "last reset: HardFault pc=1000abcd lr=10001235 @31s");
        let so = decode(STAGE_STACK_OVERFLOW, 0x1002_0000, 0, None).unwrap();
        assert_eq!(line(so, 40, 3).as_str(), "last reset: STACK OVERFLOW pc=10020000 @4s #3");
        let wdt = decode(STAGE_WDT_RENDER, 5_500, 0, None).unwrap();
        assert_eq!(line(wdt, 3600, 1).as_str(), "last reset: wdt: render stalled 5s @360s");
        let wdt = decode(STAGE_WDT_MAIN, 90_250, 0, None).unwrap();
        assert_eq!(line(wdt, 3600, 1).as_str(), "last reset: wdt: main stalled 90s @360s");
        let irq = decode(STAGE_UNHANDLED_IRQ, 17, 0, None).unwrap();
        assert_eq!(line(irq, 10, 1).as_str(), "last reset: unhandled IRQ 17 @1s");
        let none = LastReset::WatchdogNoRecord { stage: "running" };
        assert_eq!(line(none, 36000, 1).as_str(), "last reset: wdt timeout (no record, running) @3600s");
        // 異常終了以外の段階 (TBYB の進行など) は None
        assert_eq!(decode(12, 0, 0, None), None);
        assert_eq!(decode(0, 0, 0, None), None);
        assert_eq!(wdt_stage(Who::Render), STAGE_WDT_RENDER);
    }

    #[test]
    fn health_file_tail() {
        assert_eq!(health::file_tail("src/ui/slide.rs", 30), "src/ui/slide.rs");
        assert_eq!(health::file_tail("./src/bin/ticker.rs", 30), "src/bin/ticker.rs");
        assert_eq!(health::file_tail("/a/very/long/path/to/some/crate-1.2.3/src/lib.rs", 20), "src/lib.rs");
        let t = health::file_tail("/x/日本語のとても長いディレクトリ名/ファイル.rs", 20);
        assert!(t.len() <= 20 && t.ends_with(".rs"), "{t}");
    }

    // ---- 0.4.2: 起動の方針 (boot_policy) ----

    #[test]
    fn policy_state_word_roundtrip_and_legacy() {
        use boot_policy::*;
        let s = BootState {
            crash_streak: 2,
            recovery_streak: 1,
            in_recovery: true,
            fell_back: true,
        };
        assert_eq!(BootState::decode(s.encode()), s);
        // 0.4.1 の SCRATCH1 (0xC0DE_nnnn) は連続回数として読む
        assert_eq!(BootState::decode(0xC0DE_0003).crash_streak, 3);
        // ごみ / 電源投入直後は 0
        assert_eq!(BootState::decode(0), BootState::default());
        assert_eq!(BootState::decode(0x1234_5678), BootState::default());
        let retry = s.for_normal_retry();
        assert_eq!((retry.crash_streak, retry.recovery_streak, retry.in_recovery, retry.fell_back), (RECOVERY_AFTER - 1, 0, false, true));
    }

    #[test]
    fn policy_versions_and_marker() {
        use boot_policy::*;
        let v = version_word(0, 4, 2);
        assert_eq!(v, 0x0000_0192); // 4 * 100 + 2 = 402
        assert_eq!(version_parts(v), (0, 4, 2));
        assert!(version_word(0, 4, 10) > version_word(0, 4, 9));
        assert!(version_word(1, 0, 0) > version_word(0, 99, 99));
        let m = fallback_marker(v);
        assert_eq!(decode_fallback_marker(m), Some(v));
        assert_eq!(decode_fallback_marker(fallback_marker(version_word(3, 12, 7))), Some(version_word(3, 12, 7)));
        // SCRATCH0 の他の使い方とは区別できる
        for other in [0u32, 0x5355_5056, 0x1000_1234, 0x2008_1fe0, 0xFFFF_FFF9, 0xFFFF_FFFE] {
            assert_eq!(decode_fallback_marker(other), None, "{other:08x}");
        }
    }

    #[test]
    fn policy_decide_transitions() {
        use boot_policy::*;
        let own = version_word(0, 4, 2);
        let crash = PrevBoot {
            trace_version: Some(own),
            fault_recorded: true,
            watchdog_timeout: false,
        };
        let base = BootInputs {
            own_version: own,
            other_slot_known: true,
            ..BootInputs::default()
        };
        // 電源投入: 通常
        let p = decide(&BootInputs { hw_reset: true, ..base });
        assert_eq!((p.mode, p.fault), (Mode::Normal, false));
        // 1 回目の異常終了: 通常のまま、2 回目: 回復モード
        let p1 = decide(&BootInputs { prev: crash, ..base });
        assert_eq!((p1.mode, p1.fault, p1.state.crash_streak), (Mode::Normal, true, 1));
        let p2 = decide(&BootInputs { prev: crash, state_word: p1.state.encode(), ..base });
        assert_eq!((p2.mode, p2.state.crash_streak), (Mode::Recovery, 2));
        assert!(p2.state.in_recovery);
        // 記録の無いウォッチドッグの時間切れ (この版の記録あり) も異常終了
        let wdt = PrevBoot {
            trace_version: Some(own),
            fault_recorded: false,
            watchdog_timeout: true,
        };
        assert!(decide(&BootInputs { prev: wdt, ..base }).fault);
        // 他の版の記録 (TBYB で試した版が巻き戻った) は数えない
        let other = PrevBoot {
            trace_version: Some(version_word(0, 4, 3)),
            fault_recorded: true,
            watchdog_timeout: true,
        };
        let p = decide(&BootInputs { prev: other, state_word: p1.state.encode(), ..base });
        assert_eq!((p.fault, p.state.crash_streak, p.mode), (false, 1, Mode::Normal));
        // 回復モードで 3 回落ちたら他方区画へ (1 回だけ)
        let mut word = p2.state.encode();
        let mut modes = Vec::new();
        for _ in 0..4 {
            let p = decide(&BootInputs { prev: crash, state_word: word, ..base });
            modes.push(p.mode);
            word = p.state.encode();
        }
        assert_eq!(modes, [Mode::Recovery, Mode::Recovery, Mode::Fallback, Mode::Recovery]);
        // 他方区画が分からなければ回復モードのまま
        let p = decide(&BootInputs {
            prev: crash,
            state_word: BootState { crash_streak: 2, recovery_streak: 2, in_recovery: true, fell_back: false }.encode(),
            other_slot_known: false,
            ..base
        });
        assert_eq!(p.mode, Mode::Recovery);
        // TBYB の buy 待ちは常に通常、SCRATCH1 を書かない
        let p = decide(&BootInputs { tbyb_pending: true, flash_update_boot: true, prev: crash, state_word: word, ..base });
        assert_eq!((p.mode, p.write_state), (Mode::Normal, false));
        // 他の版が戻してきた: その版を覚える、回数は 0 から
        let p = decide(&BootInputs {
            flash_update_boot: true,
            marker_word: fallback_marker(version_word(0, 4, 3)),
            state_word: word,
            ..base
        });
        assert_eq!((p.mode, p.fell_back_from, p.state.crash_streak), (Mode::Normal, Some(version_word(0, 4, 3)), 0));
        // 自分の印で戻ってきた (他方区画に起動できるものが無い): 回復モードのまま、二度と戻そうとしない
        let p = decide(&BootInputs {
            flash_update_boot: true,
            marker_word: fallback_marker(own),
            state_word: word,
            ..base
        });
        assert_eq!((p.mode, p.fallback_failed, p.state.fell_back), (Mode::Recovery, true, true));
        // OTA で入った版 (FLASH_UPDATE、印なし) は 0 から
        let p = decide(&BootInputs { flash_update_boot: true, state_word: word, prev: crash, ..base });
        assert_eq!((p.mode, p.fault, p.state.crash_streak), (Mode::Normal, false, 0));
        // 回復モードから通常モードを試す (clean reset): もう 1 回落ちたらすぐ回復モード
        let retry = BootState { crash_streak: 2, in_recovery: true, ..BootState::default() }.for_normal_retry();
        let p = decide(&BootInputs { state_word: retry.encode(), ..base });
        assert_eq!(p.mode, Mode::Normal);
        let p = decide(&BootInputs { state_word: p.state.encode(), prev: crash, ..base });
        assert_eq!(p.mode, Mode::Recovery);
    }

    #[test]
    fn policy_buy_gate() {
        use boot_policy::*;
        let mut g = BuyGate::new(BUY_DEADLINE_MS, BUY_SETTLE_MS);
        let all = BuyInputs {
            now_ms: 0,
            network_up: true,
            ota_proved: true,
            round: Round::DONE,
            healthy: true,
        };
        // 条件の順に待つ
        assert_eq!(g.tick(&BuyInputs { network_up: false, ..all }), BuyStep::Waiting { missing: "wifi" });
        assert_eq!(g.tick(&BuyInputs { ota_proved: false, ..all }), BuyStep::Waiting { missing: "ota" });
        let partial = Round { sd_config: true, ntp: true, ..Round::default() };
        assert_eq!(g.tick(&BuyInputs { round: partial, ..all }), BuyStep::Waiting { missing: "weather" });
        assert_eq!(g.tick(&BuyInputs { healthy: false, ..all }), BuyStep::Waiting { missing: "health" });
        // 揃ってから 25 s
        assert_eq!(g.tick(&BuyInputs { now_ms: 40_000, ..all }), BuyStep::Settling { left_ms: 25_000 });
        assert_eq!(g.tick(&BuyInputs { now_ms: 50_000, ..all }), BuyStep::Settling { left_ms: 15_000 });
        // 途中で Wi-Fi が落ちたら待ち直し
        assert!(matches!(g.tick(&BuyInputs { now_ms: 55_000, network_up: false, ..all }), BuyStep::Waiting { .. }));
        assert_eq!(g.tick(&BuyInputs { now_ms: 60_000, ..all }), BuyStep::Settling { left_ms: 25_000 });
        assert_eq!(g.tick(&BuyInputs { now_ms: 85_000, ..all }), BuyStep::Buy);
        // 1 度だけ
        assert!(matches!(g.tick(&BuyInputs { now_ms: 86_000, ..all }), BuyStep::Waiting { .. }));
        // 締め切り
        let mut g = BuyGate::new(BUY_DEADLINE_MS, BUY_SETTLE_MS);
        assert_eq!(g.tick(&BuyInputs { now_ms: 170_000, ..all }), BuyStep::Settling { left_ms: 25_000 });
        assert_eq!(g.tick(&BuyInputs { now_ms: 180_000, ..all }), BuyStep::TimedOut);
    }

    #[test]
    fn policy_record_roundtrip() {
        use boot_policy::*;
        let r = Record::default().with_credentials(b"MyHome-2G", b"secret-pass");
        let r = Record { blocked: version_word(0, 4, 3), ..r };
        let bytes = r.encode();
        assert_eq!(Record::decode(&bytes), Some(r));
        assert_eq!(r.ssid(), b"MyHome-2G");
        assert_eq!(r.password(), b"secret-pass");
        // 1 バイト壊れたら読まない、消去状態 (0xFF) も
        let mut bad = bytes;
        bad[20] ^= 1;
        assert_eq!(Record::decode(&bad), None);
        assert_eq!(Record::decode(&[0xff; RECORD_LEN]), None);
        // 入れない版
        assert!(!r.allows(version_word(0, 4, 3)));
        assert!(!r.allows(version_word(0, 4, 2)));
        assert!(r.allows(version_word(0, 4, 4)));
        assert!(Record::default().allows(version_word(0, 4, 3)));
        // CRC-32 (IEEE) の既知の値
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn health_stack_paint_and_kib() {
        use health::*;
        let mut stack = [STACK_PAINT; 100];
        assert_eq!(untouched_words(&stack), 100);
        stack[60] = 0; // 最も深く使った位置 (下位側から 60 語目)
        stack[99] = 1;
        assert_eq!(untouched_words(&stack), 60);
        let kib = |b: u32| {
            let mut s: heapless::String<16> = heapless::String::new();
            write_kib(&mut s, b);
            s
        };
        assert_eq!(kib(40_452).as_str(), "39.5K");
        assert_eq!(kib(23_654).as_str(), "23.1K");
        assert_eq!(kib(0).as_str(), "0.0K");
    }
}
