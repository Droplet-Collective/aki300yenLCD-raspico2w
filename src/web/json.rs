//! 応答の JSON を固定長バッファへ書く小さな道具 (0.5.0〜、ホストでテストする)
//!
//! 溢れたら以後は書かず `overflow` を立てる (呼び出し側が 500 にする)。文字列は JSON の規則で逃がす。

use core::fmt::Write as _;

pub struct Json<'a> {
    buf: &'a mut [u8],
    len: usize,
    pub overflow: bool,
    /// 次の値の前に `,` が要るか
    comma: bool,
}

impl<'a> Json<'a> {
    pub fn new(buf: &'a mut [u8]) -> Self {
        Self {
            buf,
            len: 0,
            overflow: false,
            comma: false,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }

    pub fn raw(&mut self, s: &str) {
        let end = self.len + s.len();
        if self.overflow || end > self.buf.len() {
            self.overflow = true;
            return;
        }
        self.buf[self.len..end].copy_from_slice(s.as_bytes());
        self.len = end;
    }

    fn sep(&mut self) {
        if self.comma {
            self.raw(",");
        }
        self.comma = true;
    }

    pub fn begin_object(&mut self) {
        self.sep();
        self.raw("{");
        self.comma = false;
    }

    pub fn end_object(&mut self) {
        self.raw("}");
        self.comma = true;
    }

    pub fn begin_array(&mut self) {
        self.sep();
        self.raw("[");
        self.comma = false;
    }

    pub fn end_array(&mut self) {
        self.raw("]");
        self.comma = true;
    }

    /// オブジェクトのキー (次に値を 1 つ書く)
    pub fn key(&mut self, key: &str) {
        self.sep();
        self.string_body(key);
        self.raw(":");
        self.comma = false;
    }

    pub fn str(&mut self, value: &str) {
        self.sep();
        self.string_body(value);
    }

    pub fn null(&mut self) {
        self.sep();
        self.raw("null");
    }

    pub fn bool(&mut self, value: bool) {
        self.sep();
        self.raw(if value { "true" } else { "false" });
    }

    pub fn int(&mut self, value: i64) {
        self.sep();
        let mut s: heapless::String<24> = heapless::String::new();
        let _ = write!(s, "{}", value);
        self.raw(&s);
    }

    /// 小数 (`digits` 桁、有限でなければ null)
    pub fn float(&mut self, value: f32, digits: usize) {
        if !value.is_finite() {
            self.null();
            return;
        }
        self.sep();
        let mut s: heapless::String<32> = heapless::String::new();
        let _ = write!(s, "{:.*}", digits, value);
        self.raw(&s);
    }

    pub fn field_str(&mut self, key: &str, value: &str) {
        self.key(key);
        self.str(value);
    }

    pub fn field_int(&mut self, key: &str, value: i64) {
        self.key(key);
        self.int(value);
    }

    pub fn field_bool(&mut self, key: &str, value: bool) {
        self.key(key);
        self.bool(value);
    }

    fn string_body(&mut self, s: &str) {
        self.raw("\"");
        let mut start = 0;
        for (i, ch) in s.char_indices() {
            let esc: Option<&str> = match ch {
                '"' => Some("\\\""),
                '\\' => Some("\\\\"),
                '\n' => Some("\\n"),
                '\r' => Some("\\r"),
                '\t' => Some("\\t"),
                c if (c as u32) < 0x20 => Some(""),
                // `</script>` などを HTML に埋めても安全なように
                '<' => Some("\\u003c"),
                _ => None,
            };
            if let Some(esc) = esc {
                self.raw(&s[start..i]);
                if esc.is_empty() {
                    let mut u: heapless::String<8> = heapless::String::new();
                    let _ = write!(u, "\\u{:04x}", ch as u32);
                    self.raw(&u);
                } else {
                    self.raw(esc);
                }
                start = i + ch.len_utf8();
            }
        }
        self.raw(&s[start..]);
        self.raw("\"");
    }
}
