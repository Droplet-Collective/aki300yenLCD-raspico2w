//! 時計用の大きな数字: 5×7 ドットの数字と記号を `scale` 倍で描く (日本語フォントの半角数字は
//! 時計には小さすぎるため)。`0`〜`9`、`:`、`-`、` ` だけ。描画先は閉包で受ける (フォント層と同じ)。

/// 1 文字の幅 (等倍)
pub const WIDTH: i32 = 5;
/// 1 文字の高さ (等倍)
pub const HEIGHT: i32 = 7;
/// 文字間 (等倍)
pub const GAP: i32 = 1;

/// 5×7 のビットマップ (各行 bit4 が左)
const fn rows(ch: char) -> [u8; 7] {
    match ch {
        '0' => [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
        '1' => [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        '2' => [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111],
        '3' => [0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110],
        '4' => [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
        '5' => [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110],
        '6' => [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
        '7' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
        '8' => [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
        '9' => [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100],
        ':' => [0b00000, 0b00100, 0b00100, 0b00000, 0b00100, 0b00100, 0b00000],
        '-' => [0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000],
        _ => [0; 7],
    }
}

/// 文字の送り幅 (等倍)。`:` は詰める
pub const fn advance(ch: char) -> i32 {
    match ch {
        ':' => 3 + GAP,
        _ => WIDTH + GAP,
    }
}

/// 文字列の描画幅 (px)
pub fn text_width(text: &str, scale: i32) -> i32 {
    text.chars().map(|c| advance(c) * scale).sum::<i32>() - GAP * scale
}

/// 文字列を `(x, y)` から `scale` 倍で描き、次の x を返す
pub fn draw_text(text: &str, x: i32, y: i32, scale: i32, mut put: impl FnMut(i32, i32)) -> i32 {
    let mut cursor = x;
    for ch in text.chars() {
        let bitmap = rows(ch);
        // `:` は 5 桁のうち中央 3 桁だけ使うので、左に 1 桁詰めて描く
        let shift = if ch == ':' { 1 } else { 0 };
        for (row, bits) in bitmap.iter().enumerate() {
            for col in 0..WIDTH {
                if bits & (0x10 >> col) == 0 {
                    continue;
                }
                let px = cursor + (col - shift) * scale;
                let py = y + row as i32 * scale;
                for dy in 0..scale {
                    for dx in 0..scale {
                        put(px + dx, py + dy);
                    }
                }
            }
        }
        cursor += advance(ch) * scale;
    }
    cursor
}
