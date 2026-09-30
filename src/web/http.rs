//! HTTP/1.1 の要求ヘッダの解釈 (設定ページのサーバ、0.5.0〜。ハードウェアに依存しない、ホストでテストする)
//!
//! 受け付けるのは設定ページが使う範囲だけ: `GET` / `POST` / `OPTIONS`、`Content-Length` の本文 (chunked は 501)、
//! ヘッダ全体は [`HEAD_MAX`] バイトまで (超えたら 431)。値はバッファを指す `&str` のまま返す (複写しない)。

/// 要求ヘッダ (要求行 + ヘッダ + 空行) の上限
pub const HEAD_MAX: usize = 2048;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Options,
    Other,
}

/// 解釈した要求ヘッダ (値は元のバッファを指す)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Head<'a> {
    pub method: Method,
    /// `?` より前 (例 `/api/status`)
    pub path: &'a str,
    /// `?` より後 (無ければ空)
    pub query: &'a str,
    pub host: Option<&'a str>,
    pub origin: Option<&'a str>,
    /// `Sec-Fetch-Site` (ブラウザが付ける。`same-origin` / `cross-site` / `none` など)
    pub fetch_site: Option<&'a str>,
    pub content_type: Option<&'a str>,
    pub content_length: Option<u32>,
    /// `X-Ticker-Code` (状態を変える要求に必須のアクセスコード)
    pub code: Option<&'a str>,
    /// `Connection: close` か HTTP/1.0 (keep-alive なし)
    pub close: bool,
    /// `Expect: 100-continue`
    pub expect_continue: bool,
    /// ヘッダの長さ (空行まで含む。本文はここから始まる)
    pub head_len: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeadError {
    /// まだ空行まで届いていない (続きを読む)
    Incomplete,
    /// [`HEAD_MAX`] を超えた (431)
    TooLarge,
    /// 構文が正しくない (400)
    Bad,
    /// `Transfer-Encoding` (chunked) など、対応しない本文 (501)
    Unsupported,
}

/// `buf` の先頭から空行 (`\r\n\r\n`) までを探す。戻り値はヘッダの長さ
pub fn find_head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4)
}

/// 要求ヘッダを解釈する。`buf` は受け取った分 (本文の一部が続いていてもよい)
pub fn parse_head(buf: &[u8]) -> Result<Head<'_>, HeadError> {
    let Some(head_len) = find_head_end(buf) else {
        return Err(if buf.len() >= HEAD_MAX { HeadError::TooLarge } else { HeadError::Incomplete });
    };
    if head_len > HEAD_MAX {
        return Err(HeadError::TooLarge);
    }
    let text = core::str::from_utf8(&buf[..head_len - 4]).map_err(|_| HeadError::Bad)?;
    let mut lines = text.split("\r\n");
    let request = lines.next().ok_or(HeadError::Bad)?;
    let mut parts = request.split(' ');
    let (Some(method), Some(target), Some(version), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
        return Err(HeadError::Bad);
    };
    let method = match method {
        "GET" => Method::Get,
        "POST" => Method::Post,
        "OPTIONS" => Method::Options,
        m if !m.is_empty() && m.bytes().all(|b| b.is_ascii_uppercase()) => Method::Other,
        _ => return Err(HeadError::Bad),
    };
    let http10 = match version {
        "HTTP/1.1" => false,
        "HTTP/1.0" => true,
        _ => return Err(HeadError::Bad),
    };
    if !target.starts_with('/') {
        return Err(HeadError::Bad);
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let mut head = Head {
        method,
        path,
        query,
        host: None,
        origin: None,
        fetch_site: None,
        content_type: None,
        content_length: None,
        code: None,
        close: http10,
        expect_continue: false,
        head_len,
    };
    for line in lines {
        let (name, value) = line.split_once(':').ok_or(HeadError::Bad)?;
        if name.is_empty() || name.contains([' ', '\t']) {
            return Err(HeadError::Bad);
        }
        let value = value.trim_matches([' ', '\t']);
        let is = |n: &str| name.eq_ignore_ascii_case(n);
        if is("host") {
            if head.host.replace(value).is_some() {
                return Err(HeadError::Bad); // Host が 2 つ
            }
        } else if is("origin") {
            head.origin = Some(value);
        } else if is("sec-fetch-site") {
            head.fetch_site = Some(value);
        } else if is("content-type") {
            head.content_type = Some(value);
        } else if is("content-length") {
            let n = parse_u32(value).ok_or(HeadError::Bad)?;
            if head.content_length.is_some_and(|m| m != n) {
                return Err(HeadError::Bad);
            }
            head.content_length = Some(n);
        } else if is("transfer-encoding") {
            return Err(HeadError::Unsupported);
        } else if is("x-ticker-code") {
            head.code = Some(value);
        } else if is("connection") {
            if value.split(',').any(|t| t.trim().eq_ignore_ascii_case("close")) {
                head.close = true;
            } else if value.split(',').any(|t| t.trim().eq_ignore_ascii_case("keep-alive")) && http10 {
                head.close = false;
            }
        } else if is("expect") {
            if value.eq_ignore_ascii_case("100-continue") {
                head.expect_continue = true;
            } else {
                return Err(HeadError::Unsupported);
            }
        }
    }
    Ok(head)
}

/// ヘッダと一緒に読んだ `received` バイトのうち、この要求の本文に当たる範囲 (`head_len` から、`content_length`
/// を超えない)。後ろの余り (パイプライン化された次の要求など) は本文に含めない
pub fn body_prefix(received: usize, head_len: usize, content_length: u32) -> core::ops::Range<usize> {
    let start = head_len.min(received);
    let end = received.min(head_len.saturating_add(content_length as usize)).max(start);
    start..end
}

/// 10 進の u32 (先頭の 0 は可、符号や空白は不可)
pub fn parse_u32(text: &str) -> Option<u32> {
    if text.is_empty() || text.len() > 10 || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// クエリ文字列 (`a=1&b=2`) から `key` の生の値 (百分率符号化のまま) を探す
pub fn query_param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('=').or(Some((pair, ""))))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v)
}

/// 状態コードの理由句
pub fn reason(status: u16) -> &'static str {
    match status {
        100 => "Continue",
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        409 => "Conflict",
        411 => "Length Required",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        422 => "Unprocessable Content",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        503 => "Service Unavailable",
        _ => "Status",
    }
}
