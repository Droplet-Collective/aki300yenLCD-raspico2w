//! 手描きのドット絵アイコン (天気 15×15、状態表示の小物)
//!
//! 1 文字 = 1 画素のパレット文字列。`.` は透明。色と不透明度は [`ink`]。
//! 縁取り `k` (半透明の黒) で明るい写真の上でも形が分かるようにしてある。

use super::color::{Color, palette, rgb};

pub struct Icon {
    pub rows: &'static [&'static str],
}

impl Icon {
    pub fn width(&self) -> i32 {
        self.rows.first().map_or(0, |r| r.len() as i32)
    }

    pub fn height(&self) -> i32 {
        self.rows.len() as i32
    }
}

/// パレット文字 → (色, 不透明度 0..=32)
pub fn ink(ch: u8) -> Option<(Color, u8)> {
    Some(match ch {
        b'Y' => (rgb(255, 214, 72), 32),  // 太陽
        b'O' => (rgb(255, 150, 40), 32),  // 太陽の縁
        b'W' => (rgb(244, 247, 252), 32), // 雲
        b'L' => (rgb(196, 206, 222), 32), // 雲の影
        b'G' => (rgb(140, 150, 168), 32), // 暗い雲
        b'D' => (rgb(96, 104, 122), 32),  // 雨雲
        b'B' => (rgb(88, 170, 255), 32),  // 雨
        b'C' => (rgb(200, 232, 255), 32), // 雪
        b'T' => (rgb(255, 232, 64), 32),  // 雷
        b'M' => (rgb(252, 238, 176), 32), // 月
        b'm' => (rgb(214, 196, 130), 32), // 月の影
        b'k' => (palette::BLACK, 14),     // 縁取り
        b'w' => (palette::WHITE, 32),
        b'g' => (rgb(120, 232, 140), 32), // 状態: 良好
        b'y' => (rgb(255, 208, 64), 32),  // 状態: 処理中
        b'r' => (rgb(255, 92, 92), 32),   // 状態: 異常
        b's' => (rgb(150, 158, 176), 32), // 状態: 未取得
        _ => return None,
    })
}

pub static SUN: Icon = Icon {
    rows: &[
        ".......Y.......",
        ".......Y.......",
        "..Y.........Y..",
        "...Y..OOO..Y...",
        ".....OYYYO.....",
        "....OYYYYYO....",
        "...OYYYYYYYO...",
        "YY.OYYYYYYYO.YY",
        "...OYYYYYYYO...",
        "....OYYYYYO....",
        ".....OYYYO.....",
        "...Y..OOO..Y...",
        "..Y.........Y..",
        ".......Y.......",
        ".......Y.......",
    ],
};

pub static MOON: Icon = Icon {
    rows: &[
        "...............",
        ".....kMMMk.....",
        "....kMMMk......",
        "...kMMMk.......",
        "..kMMMMk.......",
        "..kMMMk........",
        ".kMMMMk........",
        ".kMMMMk........",
        ".kMMMMMk.......",
        "..kMMMMMk......",
        "..kmMMMMMk...k.",
        "...kmMMMMMMMMk.",
        "....kmmMMMMMk..",
        ".....kkmmmmk...",
        ".......kkkk....",
    ],
};

pub static PARTLY_SUN: Icon = Icon {
    rows: &[
        ".........Y.....",
        "....Y....Y....Y",
        ".....Y.OOO..Y..",
        "......OYYYO....",
        ".....OYYYYYO.YY",
        "....kkkYYYYYO..",
        "...kWWWkYYYYO..",
        "..kWWWWWkOOO..Y",
        ".kWWWWWWWkk.Y..",
        "kWWWWWWWWWWk...",
        "kWWWWWWWWWWWk..",
        "kLWWWWWWWWWWWk.",
        "kLLWWWWWWWWWWk.",
        ".kLLLLLLLLLLk..",
        "..kkkkkkkkkk...",
    ],
};

pub static PARTLY_MOON: Icon = Icon {
    rows: &[
        "...............",
        "........kMMMk..",
        ".......kMMMk...",
        "......kMMMk....",
        "......kMMMk....",
        "....kkkMMMMk...",
        "...kWWWkMMMMMk.",
        "..kWWWWWkMMMk..",
        ".kWWWWWWWkkk...",
        "kWWWWWWWWWWk...",
        "kWWWWWWWWWWWk..",
        "kLWWWWWWWWWWWk.",
        "kLLWWWWWWWWWWk.",
        ".kLLLLLLLLLLk..",
        "..kkkkkkkkkk...",
    ],
};

pub static CLOUD: Icon = Icon {
    rows: &[
        "...............",
        "...............",
        "...............",
        "......kkkk.....",
        ".....kWWWWk....",
        "..kkkWWWWWWk...",
        ".kWWWWWWWWWWk..",
        ".kWWWWWWWWWWWk.",
        "kWWWWWWWWWWWWWk",
        "kWWWWWWWWWWWWWk",
        "kLWWWWWWWWWWWLk",
        "kLLLWWWWWWWLLLk",
        ".kLLLLLLLLLLLk.",
        "..kkkkkkkkkkk..",
        "...............",
    ],
};

pub static FOG: Icon = Icon {
    rows: &[
        "...............",
        "...............",
        "......kkkk.....",
        ".....kLLLLk....",
        "..kkkLLLLLLk...",
        ".kLLLLLLLLLLk..",
        "kLLLLLLLLLLLLk.",
        ".kkkkkkkkkkkk..",
        "...............",
        ".LLLLLLLLLLLL..",
        "...............",
        "...LLLLLLLLLLLL",
        "...............",
        ".LLLLLLLLLLL...",
        "...............",
    ],
};

pub static DRIZZLE: Icon = Icon {
    rows: &[
        "......kkkk.....",
        ".....kLLLLk....",
        "..kkkLLLLLLk...",
        ".kLLLLLLLLLLk..",
        "kLLLLLLLLLLLLk.",
        "kGLLLLLLLLLLGk.",
        ".kGGGGGGGGGGk..",
        "..kkkkkkkkkk...",
        "...............",
        "...B...B...B...",
        "...............",
        ".B...B...B.....",
        "...............",
        "...B...B...B...",
        "...............",
    ],
};

pub static RAIN: Icon = Icon {
    rows: &[
        "......kkkk.....",
        ".....kGGGGk....",
        "..kkkGGGGGGk...",
        ".kGGGGGGGGGGk..",
        "kGGGGGGGGGGGGk.",
        "kDGGGGGGGGGGDk.",
        ".kDDDDDDDDDDk..",
        "..kkkkkkkkkk...",
        "...B...B...B...",
        "..B...B...B....",
        "..B...B...B....",
        ".B...B...B.....",
        ".B...B...B.....",
        "B...B...B......",
        "...............",
    ],
};

pub static SNOW: Icon = Icon {
    rows: &[
        "......kkkk.....",
        ".....kLLLLk....",
        "..kkkLLLLLLk...",
        ".kLLLLLLLLLLk..",
        "kLLLLLLLLLLLLk.",
        "kGLLLLLLLLLLGk.",
        ".kGGGGGGGGGGk..",
        "..kkkkkkkkkk...",
        "...............",
        "..C.....C......",
        ".CCC...CCC..C..",
        "..C.....C..CCC.",
        ".....C......C..",
        "....CCC........",
        ".....C.........",
    ],
};

pub static THUNDER: Icon = Icon {
    rows: &[
        "......kkkk.....",
        ".....kGGGGk....",
        "..kkkGGGGGGk...",
        ".kGGGGGGGGGGk..",
        "kGGGGGGGGGGGGk.",
        "kDGGGGGGGGGGDk.",
        ".kDDDDDTTDDDk..",
        "..kkkkTTkkkk...",
        ".B...TT....B...",
        ".B..TTTTT..B...",
        "B......TT.B....",
        "B.....TT..B....",
        ".....TT........",
        "....T..........",
        "...............",
    ],
};

pub static UNKNOWN: Icon = Icon {
    rows: &[
        "...............",
        "...............",
        "....kkkkkk.....",
        "...kLLLLLLk....",
        "..kLLkkkkLLk...",
        "..kkk....kLk...",
        ".........kLk...",
        ".......kkLLk...",
        "......kLLLk....",
        "......kLLk.....",
        "......kkkk.....",
        "...............",
        "......kLLk.....",
        "......kLLk.....",
        "......kkkk.....",
    ],
};

/// WMO 4677 の天気コード → アイコン。`night` なら晴れ / 晴れ時々くもりを月にする
pub fn weather_icon(code: u8, night: bool) -> &'static Icon {
    match code {
        0 => {
            if night {
                &MOON
            } else {
                &SUN
            }
        }
        1 | 2 => {
            if night {
                &PARTLY_MOON
            } else {
                &PARTLY_SUN
            }
        }
        3 => &CLOUD,
        45 | 48 => &FOG,
        51..=57 => &DRIZZLE,
        61..=67 | 80..=82 => &RAIN,
        71..=77 | 85 | 86 => &SNOW,
        95..=99 => &THUNDER,
        _ => &UNKNOWN,
    }
}

// ------------------------------------------------------------
// 小物 (状態表示・予報)
// ------------------------------------------------------------

/// Wi-Fi の扇 (9×7)。色は描くときに差し替える (`w` の部分)
pub static WIFI: Icon = Icon {
    rows: &[
        "..wwwww..",
        ".w.....w.",
        "w..www..w",
        "..w...w..",
        "....w....",
        "...www...",
        "....w....",
    ],
};

/// 最高 (上向き三角、5×3)
pub static UP: Icon = Icon {
    rows: &["..w..", ".www.", "wwwww"],
};

/// 最低 (下向き三角、5×3)
pub static DOWN: Icon = Icon {
    rows: &["wwwww", ".www.", "..w.."],
};

/// 降水確率の雨粒 (5×8)
pub static DROP: Icon = Icon {
    rows: &["..w..", "..w..", ".www.", ".www.", "wwwww", "wwwww", "wwwww", ".www."],
};

/// 地名の位置マーク (5×7)
pub static PIN: Icon = Icon {
    rows: &[".www.", "ww.ww", "ww.ww", ".www.", ".www.", "..w..", "..w.."],
};
