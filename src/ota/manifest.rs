//! `manifest.json` の解釈と semver 比較
//!
//! ```json
//! {"version":"0.3.0","bin":"ticker.bin","size":1058560,"sha256":"<64 hex>"}
//! ```
//!
//! `bin` は Release のアセット名で、実機はこの名前をそのまま取りに行く (v0.2.x は `wifi_ota.bin`、
//! v0.3.0 からは `ticker.bin`。名前が変わっても古い版はそのまま新しい bin に切り替わる)。
//!
//! `scripts/make-manifest.sh` が生成し、CI が Release に添付する。

use core::fmt;

use heapless::String;
use serde::Deserialize;

use super::OtaError;
use crate::image_def::{VERSION_MAJOR, VERSION_MINOR, VERSION_PATCH};

/// semver の major.minor.patch (pre-release / build metadata は扱わない)。
/// `Ord` は宣言順 (major → minor → patch) の辞書順比較。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, defmt::Format)]
pub struct Version {
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
}

impl Version {
    /// 実行中ファームウェアの版数 (`CARGO_PKG_VERSION`)
    pub const CURRENT: Version = Version {
        major: VERSION_MAJOR,
        minor: VERSION_MINOR,
        patch: VERSION_PATCH,
    };

    /// `"1.2.3"` (先頭の `v` は許容) を解釈する
    pub fn parse(text: &str) -> Option<Version> {
        let text = text.strip_prefix('v').unwrap_or(text);
        let mut parts = text.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let patch = parts.next()?.parse().ok()?;
        if parts.next().is_some() {
            return None;
        }
        Some(Version { major, minor, patch })
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// JSON をそのまま借用する形
#[derive(Deserialize)]
struct RawManifest<'a> {
    version: &'a str,
    bin: &'a str,
    size: u32,
    sha256: &'a str,
}

/// 解釈済みの manifest
#[derive(Clone, Debug)]
pub struct Manifest {
    pub version: Version,
    /// Release アセット名 (例 `ticker.bin`)
    pub bin: String<64>,
    pub size: u32,
    pub sha256: [u8; 32],
}

impl Manifest {
    /// `manifest.json` の本文を解釈する。末尾の空白 / 改行は無視する。
    pub fn parse(body: &[u8]) -> Result<Manifest, OtaError> {
        let (raw, _) = serde_json_core::from_slice::<RawManifest>(body).map_err(|_| OtaError::Manifest)?;
        let version = Version::parse(raw.version).ok_or(OtaError::Manifest)?;
        let mut bin = String::new();
        bin.push_str(raw.bin).map_err(|_| OtaError::Manifest)?;
        if bin.is_empty() || bin.contains('/') {
            return Err(OtaError::Manifest);
        }
        let mut sha256 = [0u8; 32];
        decode_hex(raw.sha256, &mut sha256).ok_or(OtaError::Manifest)?;
        Ok(Manifest {
            version,
            bin,
            size: raw.size,
            sha256,
        })
    }

    /// 実行中の版数より厳密に新しいか
    pub fn is_newer_than_current(&self) -> bool {
        self.version > Version::CURRENT
    }
}

/// 64 文字の 16 進を 32 バイトへ
pub fn decode_hex(text: &str, out: &mut [u8; 32]) -> Option<()> {
    let bytes = text.as_bytes();
    if bytes.len() != 64 {
        return None;
    }
    for (i, pair) in bytes.as_chunks::<2>().0.iter().enumerate() {
        let hi = (pair[0] as char).to_digit(16)?;
        let lo = (pair[1] as char).to_digit(16)?;
        out[i] = ((hi << 4) | lo) as u8;
    }
    Some(())
}

/// 32 バイトを 16 進文字列へ (LCD / ログ表示用)
pub fn encode_hex(digest: &[u8; 32]) -> String<64> {
    let mut s = String::new();
    for byte in digest {
        let _ = core::fmt::Write::write_fmt(&mut s, format_args!("{:02x}", byte));
    }
    s
}
