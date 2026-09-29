//! SNTP の問い合わせ (embassy-net の DNS + UDP)。パケットの形式は [`super::sntp`]。
//!
//! 往復時間の半分を加えて補正する (数十 ms 程度なので秒表示には影響しない)。

use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_net::{IpAddress, IpEndpoint, Stack, dns::DnsQueryType};
use embassy_time::{Duration, Instant, with_timeout};

use super::sntp::{NTP_PORT, PACKET_LEN, SntpError, parse_reply, request_packet};


/// UDP ソケット用の小さなバッファ (bin 側で static に置く)
pub struct SntpBuffers {
    pub rx_meta: [PacketMetadata; 2],
    pub rx: [u8; 128],
    pub tx_meta: [PacketMetadata; 2],
    pub tx: [u8; 128],
}

impl SntpBuffers {
    pub const fn new() -> Self {
        Self {
            rx_meta: [PacketMetadata::EMPTY; 2],
            rx: [0; 128],
            tx_meta: [PacketMetadata::EMPTY; 2],
            tx: [0; 128],
        }
    }
}

impl Default for SntpBuffers {
    fn default() -> Self {
        Self::new()
    }
}

/// 時刻同期の結果: 「`at` の瞬間に UNIX 時刻は `unix_millis` だった」
#[derive(Clone, Copy, Debug)]
pub struct Sync {
    pub unix_millis: i64,
    pub at: Instant,
    pub stratum: u8,
    pub host: &'static str,
}

impl Sync {
    /// 今の UNIX 秒 (embassy の単調時計で進める)
    pub fn now_unix(&self) -> i64 {
        (self.unix_millis + self.at.elapsed().as_millis() as i64).div_euclid(1000)
    }
}

/// `host` を DNS で引いて SNTP 要求を送り、応答を待つ。
pub async fn query(
    stack: Stack<'_>,
    bufs: &mut SntpBuffers,
    host: &'static str,
    timeout: Duration,
) -> Result<Sync, SntpError> {
    let addrs = with_timeout(timeout, stack.dns_query(host, DnsQueryType::A))
        .await
        .map_err(|_| SntpError::Timeout)?
        .map_err(|_| SntpError::Dns)?;
    let Some(IpAddress::Ipv4(addr)) = addrs.first().copied() else {
        return Err(SntpError::Dns);
    };
    let mut socket = UdpSocket::new(stack, &mut bufs.rx_meta, &mut bufs.rx, &mut bufs.tx_meta, &mut bufs.tx);
    socket.bind(0).map_err(|_| SntpError::Socket)?;
    let endpoint = IpEndpoint::new(IpAddress::Ipv4(addr), NTP_PORT);
    let request = request_packet();
    let sent_at = Instant::now();
    socket.send_to(&request, endpoint).await.map_err(|_| SntpError::Send)?;
    let mut reply = [0u8; PACKET_LEN];
    let (n, _meta) = with_timeout(timeout, socket.recv_from(&mut reply))
        .await
        .map_err(|_| SntpError::Timeout)?
        .map_err(|_| SntpError::Socket)?;
    let received_at = Instant::now();
    let parsed = parse_reply(&reply[..n])?;
    // 応答はサーバ送信からおよそ往復の半分だけ遅れて届く
    let half_rtt_ms = (received_at - sent_at).as_millis() as i64 / 2;
    defmt::info!(
        "SNTP {} ({}): unix {} stratum {} rtt/2 {} ms",
        host,
        defmt::Debug2Format(&addr),
        parsed.unix_secs,
        parsed.stratum,
        half_rtt_ms
    );
    Ok(Sync {
        unix_millis: parsed.unix_secs * 1000 + i64::from(parsed.millis) + half_rtt_ms,
        at: received_at,
        stratum: parsed.stratum,
        host,
    })
}
