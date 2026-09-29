//! 本体クレートの純粋なモジュールを取り込んでホストでテストする (`cargo test`)

#[path = "../../../src/ticker/civil.rs"]
pub mod civil;
#[path = "../../../src/ticker/config.rs"]
pub mod config;
#[path = "../../../src/ticker/digits.rs"]
pub mod digits;
#[path = "../../../src/ticker/sntp.rs"]
pub mod sntp;
#[path = "../../../src/ticker/weather.rs"]
pub mod weather;
#[path = "../../../src/font/misaki.rs"]
pub mod misaki;

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
    fn misaki_lookup() {
        assert_eq!(misaki::glyph_count(), 7171);
        let a = misaki::glyph('A').unwrap();
        assert_eq!(a.advance, 4);
        assert_eq!(a.rows, [0x40, 0xA0, 0xA0, 0xE0, 0xA0, 0xA0, 0x00, 0x00]);
        let hi = misaki::glyph('火').unwrap();
        assert_eq!(hi.advance, 8);
        assert_eq!(hi.rows[0], 0b0001_0000);
        assert_eq!(hi.rows[6], 0b1100_0110);
        assert!(misaki::glyph('あ').is_some());
        assert!(misaki::glyph('℃').is_some());
        assert!(misaki::glyph('\u{1F600}').is_none()); // 絵文字は無い → 代替
        assert_eq!(misaki::fallback_glyph().advance, 8);
        // 幅: "OTA 0.3.0" は半角 9 文字 = 36 px、"東京" は 16 px、×2 で倍
        assert_eq!(misaki::text_width("OTA 0.3.0", 1), 36);
        assert_eq!(misaki::text_width("東京", 2), 32);
        // 描画: 'A' を (10, 20) に ×2 で描くと左上の点は (10..12, 20..22)
        let mut pts = Vec::new();
        let adv = misaki::draw_text("A", 10, 20, 2, 400, |x, y| pts.push((x, y)));
        assert_eq!(adv, 18);
        assert!(pts.contains(&(12, 20)) && pts.contains(&(13, 21)));
        assert!(!pts.contains(&(10, 20)));
        // 右端クリップ
        let mut n = 0;
        misaki::draw_text("東京", 390, 0, 2, 400, |x, _| {
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
}
