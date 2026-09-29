//! SNTP (RFC 4330) クライアント: 48 バイトの要求を UDP 123 へ送り、サーバの送信時刻を UNIX 秒にする
//!
//! パケットの組み立て / 解釈 ([`request_packet`] / [`parse_reply`]) は `core` だけに依存し、ホストで
//! テストする (このファイルは `core` だけに依存する)。問い合わせ本体は [`super::sntp_net::query`]
//! (embassy-net の DNS と UDP ソケット)。

/// NTP 時刻 (1900 起点) と UNIX 時刻 (1970 起点) の差 (秒)
pub const NTP_UNIX_OFFSET: u64 = 2_208_988_800;
/// パケット長
pub const PACKET_LEN: usize = 48;
/// 既定のサーバ (NICT 公開 NTP) と予備 (pool.ntp.org)
pub const PRIMARY_HOST: &str = "ntp.nict.jp";
pub const FALLBACK_HOST: &str = "pool.ntp.org";
pub const NTP_PORT: u16 = 123;

/// 応答の解釈結果
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reply {
    /// サーバの送信時刻 (UNIX 秒)
    pub unix_secs: i64,
    /// 同、秒未満 (ms)
    pub millis: u16,
    pub stratum: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SntpError {
    Dns,
    Socket,
    Send,
    Timeout,
    /// 長さ不足、モードが server でない、stratum 0 (kiss-o'-death)、時刻 0
    BadReply,
}

impl SntpError {
    pub fn label(self) -> &'static str {
        match self {
            SntpError::Dns => "DNS failed",
            SntpError::Socket => "socket error",
            SntpError::Send => "send failed",
            SntpError::Timeout => "timeout",
            SntpError::BadReply => "bad reply",
        }
    }
}

/// クライアント要求: LI=0, VN=4, Mode=3 (client)。他のフィールドは 0 でよい (RFC 4330 §5)。
pub fn request_packet() -> [u8; PACKET_LEN] {
    let mut packet = [0u8; PACKET_LEN];
    packet[0] = 0x23;
    packet
}

/// サーバ応答から送信時刻 (Transmit Timestamp、バイト 40〜47) を取り出す
pub fn parse_reply(packet: &[u8]) -> Result<Reply, SntpError> {
    if packet.len() < PACKET_LEN {
        return Err(SntpError::BadReply);
    }
    let mode = packet[0] & 0x07;
    let stratum = packet[1];
    if mode != 4 || stratum == 0 {
        return Err(SntpError::BadReply);
    }
    let secs = u32::from_be_bytes([packet[40], packet[41], packet[42], packet[43]]);
    let frac = u32::from_be_bytes([packet[44], packet[45], packet[46], packet[47]]);
    if secs == 0 {
        return Err(SntpError::BadReply);
    }
    // NTP 秒は 2036-02-07 で 32 bit を一周する。UNIX 起点より前 (MSB=0) の値は次の時代 (era 1) として扱う
    let ntp_secs = if secs & 0x8000_0000 != 0 {
        u64::from(secs)
    } else {
        u64::from(secs) + (1u64 << 32)
    };
    let unix_secs = ntp_secs as i64 - NTP_UNIX_OFFSET as i64;
    let millis = ((u64::from(frac) * 1000) >> 32) as u16;
    Ok(Reply {
        unix_secs,
        millis,
        stratum,
    })
}
