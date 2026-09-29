//! reqwless での HTTPS GET (リダイレクト追従、ストリーミング受信)
//!
//! GitHub の `releases/latest/download/<asset>` は
//! `302 → https://github.com/<o>/<r>/releases/download/<tag>/<asset>` →
//! `302 → https://objects.githubusercontent.com/...?X-Amz-...` (または
//! `release-assets.githubusercontent.com/...?...&jwt=...`) と 2 段のリダイレクトになる。
//! reqwless はリダイレクトを追わないので、`Location` を読んで新しい接続を開き直す
//! (ホストはどこでもよい)。TLS 設定 (`TlsVerify::None`、docs/wifi-ota.md の注意) は
//! `HttpClient` を作る側が持つ。

use embedded_io_async::Read;
use embedded_nal_async::{Dns, TcpConnect};
use heapless::String;
use reqwless::client::HttpClient;
use reqwless::request::{Method, RequestBuilder};

use super::{OtaError, URL_MAX};

/// 追従するリダイレクトの上限 (GitHub は 2 段)
pub const MAX_REDIRECTS: u8 = 3;

const HEADERS: &[(&str, &str)] = &[
    ("User-Agent", "pico2w-300yen-lcd-ota"),
    ("Accept", "*/*"),
    ("Connection", "close"),
];

/// 受信した本文を受け取る先 (フラッシュ書き込みや進捗表示のため async)
#[allow(async_fn_in_trait)] // 本クレート内でしか実装しない (Send 境界は不要)
pub trait BodySink {
    async fn push(&mut self, data: &[u8]) -> Result<(), OtaError>;
}

/// 200 以外も含む取得結果
#[derive(Clone, Copy, Debug, defmt::Format)]
pub struct Fetched {
    /// 最終応答の HTTP ステータス (200 なら本文を sink へ流した)
    pub status: u16,
    pub content_length: Option<usize>,
    /// sink へ渡したバイト数
    pub received: usize,
    pub redirects: u8,
}

/// `url` を GET し、3xx なら `Location` へ追従、200 なら本文を `chunk` 単位で `sink` へ流す。
/// 4xx / 5xx は本文を読まずに `Fetched { status, .. }` で返す (404 = Release 無しは呼び出し側で判断)。
/// `url` はリダイレクトで書き換わる。`rx_buf` は応答ヘッダ用 (GitHub は 2〜3 kB 出す)。
pub async fn fetch<T: TcpConnect, D: Dns, S: BodySink>(
    client: &mut HttpClient<'_, T, D>,
    url: &mut String<URL_MAX>,
    rx_buf: &mut [u8],
    chunk: &mut [u8],
    sink: &mut S,
) -> Result<Fetched, OtaError> {
    let mut redirects: u8 = 0;
    loop {
        defmt::info!("GET {} (redirect {})", url.as_str(), redirects);
        let mut handle = client
            .request(Method::GET, url.as_str())
            .await
            .map_err(map_error)?
            .headers(HEADERS);
        let response = handle.send(rx_buf).await.map_err(map_error)?;
        let status = response.status.0;
        defmt::info!("HTTP {} content-length {:?}", status, response.content_length);

        if (300..400).contains(&status) {
            let mut next: String<URL_MAX> = String::new();
            let mut found = false;
            for (name, value) in response.headers() {
                if name.eq_ignore_ascii_case("location") {
                    let text = core::str::from_utf8(value).map_err(|_| OtaError::HttpProtocol)?;
                    next.push_str(text.trim()).map_err(|_| OtaError::LocationTooLong)?;
                    found = true;
                    break;
                }
            }
            drop(response);
            drop(handle);
            if !found || !(next.starts_with("https://") || next.starts_with("http://")) {
                return Err(OtaError::HttpProtocol);
            }
            if redirects >= MAX_REDIRECTS {
                return Err(OtaError::TooManyRedirects);
            }
            redirects += 1;
            *url = next;
            continue;
        }

        if status != 200 {
            return Ok(Fetched {
                status,
                content_length: response.content_length,
                received: 0,
                redirects,
            });
        }

        let content_length = response.content_length;
        let mut reader = response.body().reader();
        let mut received = 0usize;
        loop {
            let n = reader.read(chunk).await.map_err(map_error)?;
            if n == 0 {
                break;
            }
            sink.push(&chunk[..n]).await?;
            received += n;
        }
        return Ok(Fetched {
            status,
            content_length,
            received,
            redirects,
        });
    }
}

fn map_error(error: reqwless::Error) -> OtaError {
    defmt::warn!("http error: {:?}", defmt::Debug2Format(&error));
    match error {
        reqwless::Error::Dns => OtaError::Dns,
        reqwless::Error::Network(_) | reqwless::Error::ConnectionAborted => OtaError::Network,
        reqwless::Error::Tls(_) => OtaError::Tls,
        _ => OtaError::HttpProtocol,
    }
}
