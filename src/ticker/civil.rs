//! UNIX 時刻 (秒) → 暦 (年月日、曜日、時分秒)
//!
//! Howard Hinnant の `civil_from_days` (proleptic Gregorian) をそのまま使う。閏秒は扱わない。

/// 暦上の日時
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DateTime {
    pub year: i32,
    /// 1..=12
    pub month: u8,
    /// 1..=31
    pub day: u8,
    /// 0 = 日曜 … 6 = 土曜
    pub weekday: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

/// 曜日の日本語 1 文字 (0 = 日)
pub const WEEKDAY_JA: [&str; 7] = ["日", "月", "火", "水", "木", "金", "土"];
/// 曜日の英語 3 文字
pub const WEEKDAY_EN: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/// 1970-01-01 からの日数 → (年, 月, 日)
pub fn civil_from_days(days: i64) -> (i32, u8, u8) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u8; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u8; // [1, 12]
    let year = if m <= 2 { y + 1 } else { y };
    (year as i32, m, d)
}

/// UNIX 時刻 (UTC 秒) と UTC からのオフセット (秒) から現地の日時を作る
pub fn from_unix(unix: i64, offset_secs: i32) -> DateTime {
    let local = unix + i64::from(offset_secs);
    let days = local.div_euclid(86_400);
    let secs = local.rem_euclid(86_400) as u32;
    let (year, month, day) = civil_from_days(days);
    // 1970-01-01 は木曜 (4)
    let weekday = (days + 4).rem_euclid(7) as u8;
    DateTime {
        year,
        month,
        day,
        weekday,
        hour: (secs / 3600) as u8,
        minute: (secs / 60 % 60) as u8,
        second: (secs % 60) as u8,
    }
}

impl DateTime {
    pub fn weekday_ja(&self) -> &'static str {
        WEEKDAY_JA[usize::from(self.weekday) % 7]
    }

    pub fn weekday_en(&self) -> &'static str {
        WEEKDAY_EN[usize::from(self.weekday) % 7]
    }
}
