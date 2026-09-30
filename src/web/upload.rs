//! 写真の追加 (0.5.0〜、ホストでテストする): 受け付ける BMP の形と、SD に置く 8.3 形式の名前
//!
//! ブラウザが 400×96 に切り抜いて 24 bit の BMP (下から上、パディング無し) にしたものだけを受け付ける:
//! [`UPLOAD_SIZE`] = 54 + 400 × 96 × 3 = 115,254 バイト。スライドショーはもっと自由な BMP も読める
//! (`ui::bmp`) が、SD に書くものは形を 1 つに決めて、ヘッダを全部確かめる。

use heapless::String;

pub const WIDTH: u32 = 400;
pub const HEIGHT: u32 = 96;
/// BMP のヘッダ (BITMAPFILEHEADER 14 + BITMAPINFOHEADER 40)
pub const HEADER: usize = 54;
/// 受け付ける BMP の大きさ (バイト)
pub const UPLOAD_SIZE: u32 = HEADER as u32 + WIDTH * HEIGHT * 3;
/// SD のルートに置ける BMP の数 (スライドショーが使う上限と同じ)
pub const MAX_FILES: usize = 16;

fn u16_at(h: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([h[i], h[i + 1]])
}

fn u32_at(h: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([h[i], h[i + 1], h[i + 2], h[i + 3]])
}

/// BMP のヘッダ (先頭 54 バイト) と、送られてくる全体の長さを確かめる
pub fn check_header(h: &[u8], content_length: u32) -> Result<(), &'static str> {
    if content_length != UPLOAD_SIZE {
        return Err("size must be 115254 bytes (400x96 24-bit BMP)");
    }
    if h.len() < HEADER {
        return Err("short header");
    }
    if &h[0..2] != b"BM" {
        return Err("not a BMP");
    }
    if u32_at(h, 2) != UPLOAD_SIZE {
        return Err("BMP file size field mismatch");
    }
    if u32_at(h, 10) != HEADER as u32 {
        return Err("pixel data offset must be 54");
    }
    if u32_at(h, 14) != 40 {
        return Err("BITMAPINFOHEADER expected");
    }
    let width = u32_at(h, 18) as i32;
    let height = u32_at(h, 22) as i32;
    if width != WIDTH as i32 || height.unsigned_abs() != HEIGHT {
        return Err("image must be 400x96");
    }
    if u16_at(h, 26) != 1 || u16_at(h, 28) != 24 {
        return Err("24-bit BMP expected");
    }
    if u32_at(h, 30) != 0 {
        return Err("compressed BMP not supported");
    }
    let image_size = u32_at(h, 34);
    if image_size != 0 && image_size != WIDTH * HEIGHT * 3 {
        return Err("BMP image size field mismatch");
    }
    Ok(())
}

/// 受け取る 8.3 形式の BMP の名前か (大文字小文字は問わない。`_` で始まる名前は macOS の `._` などと
/// 紛れるのでスライドショーも使わない)
pub fn is_bmp_name(name: &str) -> bool {
    crate::ticker::config::is_short_name(name)
        && name.len() >= 5
        && name[name.len() - 4..].eq_ignore_ascii_case(".BMP")
        && !name.starts_with('_')
}

/// 元のファイル名 (`夕焼け 2.jpg` など) から、SD に置く 8.3 の名前を作る (英数字だけ、8 文字まで、大文字、`.BMP`)。
/// 使える文字が無ければ None (呼び出し側が [`numbered`] の名前にする)
pub fn short_name_from(original: &str) -> Option<String<12>> {
    let stem = match original.rfind('.') {
        Some(i) if i > 0 => &original[..i],
        _ => original,
    };
    let mut name: String<12> = String::new();
    for ch in stem.chars() {
        if name.len() >= 8 {
            break;
        }
        if ch.is_ascii_alphanumeric() {
            let _ = name.push(ch.to_ascii_uppercase());
        } else if (ch == '-' || ch == '_') && !name.is_empty() {
            let _ = name.push(ch);
        }
    }
    let trimmed = name.trim_end_matches(['-', '_']).len();
    name.truncate(trimmed);
    if name.is_empty() {
        return None;
    }
    let _ = name.push_str(".BMP");
    Some(name)
}

/// `IMG00001.BMP` の形の名前 (`n` は 1〜99999)
pub fn numbered(n: u32) -> String<12> {
    use core::fmt::Write as _;
    let mut name: String<12> = String::new();
    let _ = write!(name, "IMG{:05}.BMP", n % 100_000);
    name
}

/// 同じ名前が既にあるときの別名: `SUNSET.BMP` → `SUNSET~2.BMP` (本体を 6 文字に詰めて `~n`)
pub fn variant(name: &str, n: u32) -> String<12> {
    use core::fmt::Write as _;
    let stem = name.split('.').next().unwrap_or("IMG");
    let mut out: String<12> = String::new();
    let suffix_len = if n < 10 { 2 } else { 3 };
    for ch in stem.chars().take(8 - suffix_len) {
        let _ = out.push(ch);
    }
    let _ = write!(out, "~{}.BMP", n % 100);
    out
}
