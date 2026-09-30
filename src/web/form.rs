//! `application/x-www-form-urlencoded` の本文 / クエリの解釈 (0.5.0〜、ホストでテストする)
//!
//! 設定ページは `URLSearchParams` で送る (`a=1&b=%E6%9D%B1`、空白は `+`)。値は呼び出し側のバッファへ復号する。

/// `a=1&b=2` の組を順に返す (値は符号化のまま。空の組は飛ばす)
pub fn pairs(body: &str) -> impl Iterator<Item = (&str, &str)> {
    body.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| p.split_once('=').unwrap_or((p, "")))
}

/// 百分率符号化 (`%XX`) と `+` (空白) を復号して `out` へ。UTF-8 として正しくなければ None
pub fn decode<'o>(raw: &str, out: &'o mut [u8]) -> Option<&'o str> {
    let bytes = raw.as_bytes();
    let mut i = 0;
    let mut n = 0;
    while i < bytes.len() {
        let b = match bytes[i] {
            b'+' => b' ',
            b'%' => {
                let hi = hex(*bytes.get(i + 1)?)?;
                let lo = hex(*bytes.get(i + 2)?)?;
                i += 2;
                hi << 4 | lo
            }
            b => b,
        };
        *out.get_mut(n)? = b;
        n += 1;
        i += 1;
    }
    core::str::from_utf8(&out[..n]).ok()
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
