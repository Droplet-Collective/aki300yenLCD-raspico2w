//! CYW43439 Wi-Fi の立ち上げと SD カード `wifi.txt` の読み込み (`wifi_status` / `wifi_ota` 共用)
//!
//! - `WIFI.TXT` (1 行目 SSID、2 行目パスワード) の読み込みと検証
//! - CYW43439 (PWR=GP23, CS=GP25, DIO=GP24, CLK=GP29) を PIO1 SM0 + DMA CH4 で駆動し、
//!   cyw43 ドライバと embassy-net (DHCP) のタスクを起動する
//! - パッシブスキャン結果の集約 (`ApEntry`)
//!
//! LCD (PIO0 / DMA CH0〜CH3) とは独立。`Pio<PIO1>` は呼び出し側が `bind_interrupts!` した
//! 型で作って渡す。PIO の `Common` / `StateMachine` は drop すると参照カウントが減り、最後の
//! drop で PIO が使っていた GPIO の FUNCSEL が全て NULL に戻される (embassy-rp 0.9
//! `pio/mod.rs` `on_pio_drop`) ため、ここでは **一切 drop しない** (`mem::forget`)。

use cyw43::{JoinAuth, JoinOptions, PowerManagementMode};
use cyw43_pio::{DEFAULT_CLOCK_DIVIDER, PioSpi};
use embassy_executor::Spawner;
use embassy_net::{Stack, StackResources};
use embassy_rp::Peri;
use embassy_rp::clocks::RoscRng;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::{DMA_CH4, PIN_23, PIN_24, PIN_25, PIN_29, PIO1};
use embassy_rp::pio::Pio;
use embassy_time::{Duration, Timer, with_timeout};
use embedded_sdmmc::{Mode, VolumeIdx};
use heapless::{String, Vec};
use static_cell::StaticCell;

use crate::sdcard::{SdVolumeManager, volume_error};

// ============================================================
// CYW43439 ファームウェア (Infineon Permissive Binary License)
// ============================================================

pub static CYW43_FW: &[u8] = include_bytes!("../firmware/cyw43/43439A0.bin");
pub static CYW43_CLM: &[u8] = include_bytes!("../firmware/cyw43/43439A0_clm.bin");

/// embassy-net のソケット数 (DHCP + DNS + TCP 1 本 + 予備)
pub const STACK_SOCKETS: usize = 6;

/// 起動時に WL_REG_ON (GP23) を Low に保つ時間。
///
/// cyw43 0.6 の `Bus::init` は WL_REG_ON を **20 ms** 落としてから上げる (その後 250 ms 待つ) だけで、
/// 内部で `WLAN` / `SOCSRAM` コアをリセットしてファームウェアを再ロードはするものの、電源断としては短い。
/// 電源投入や BOOTSEL 起動では元々チップが無電源だったので問題にならないが、Wi-Fi に接続して DHCP まで
/// 済ませた状態のファームウェアから `reboot(FLASH_UPDATE)` で温かい再起動をすると、CYW43439 は直前まで
/// 通電・接続中で、20 ms では内部状態が残ることがある (v0.2.5 の TBYB 起動: join は 1 回で成功したのに
/// DHCP のブロードキャストが一度も通らず 120 s で巻き戻った)。ここで十分な時間 Low に保ってから
/// cyw43 にピンを渡し、起動経路にかかわらず毎回コールドスタートにする。
pub const CYW43_POWER_OFF_MS: u64 = 500;
/// 再起動直前に WL_REG_ON を落としてから `reboot()` を呼ぶまでの時間 (`power_off_for_reboot`)
pub const CYW43_REBOOT_POWER_OFF_MS: u64 = 100;
/// `power_off_for_reboot` の `leave()` に許す時間 (cyw43 ランナーが応答しなくても再起動は進める)
const LEAVE_TIMEOUT: Duration = Duration::from_secs(2);

pub type Cyw43Spi = PioSpi<'static, PIO1, 0, DMA_CH4>;
pub type Cyw43Runner = cyw43::Runner<'static, Output<'static>, Cyw43Spi>;

#[embassy_executor::task]
async fn cyw43_task(runner: Cyw43Runner) -> ! {
    runner.run().await
}

#[embassy_executor::task]
async fn net_task(mut runner: embassy_net::Runner<'static, cyw43::NetDriver<'static>>) -> ! {
    runner.run().await
}

// ============================================================
// wifi.txt
// ============================================================

pub struct WifiCredentials {
    pub ssid: String<32>,
    /// 空文字列ならオープンネットワークとして接続する
    pub password: String<64>,
}

impl WifiCredentials {
    /// cyw43 の join オプション (パスワード無しはオープン、有りは WPA2)
    pub fn join_options(&self) -> JoinOptions<'_> {
        if self.password.is_empty() {
            JoinOptions::new_open()
        } else {
            let mut options = JoinOptions::new(self.password.as_bytes());
            options.auth = JoinAuth::Wpa2;
            options
        }
    }
}

/// SD ルートの `WIFI.TXT` を読む
pub fn read_credentials(volume_mgr: &SdVolumeManager) -> Result<WifiCredentials, &'static str> {
    let volume = volume_mgr.open_volume(VolumeIdx(0)).map_err(volume_error)?;
    let root = volume.open_root_dir().map_err(|_| "ROOT DIR ERROR")?;
    let file = root
        .open_file_in_dir("WIFI.TXT", Mode::ReadOnly)
        .map_err(|_| "wifi.txt not found")?;
    let mut buf = [0u8; 256];
    let mut len = 0;
    while len < buf.len() {
        let count = file
            .read(&mut buf[len..])
            .map_err(|_| "wifi.txt read error")?;
        if count == 0 {
            break;
        }
        len += count;
    }
    parse_credentials(&buf[..len])
}

/// data 区画の写し (`persist`、0.4.2〜の回復モード) から資格情報を作る (`parse_credentials` と同じ検査)
pub fn credentials_from_bytes(ssid: &[u8], password: &[u8]) -> Result<WifiCredentials, &'static str> {
    let ssid = core::str::from_utf8(ssid).map_err(|_| "wifi copy: not UTF-8")?;
    let password = core::str::from_utf8(password).map_err(|_| "wifi copy: not UTF-8")?;
    if ssid.is_empty() || ssid.len() > 32 {
        return Err("wifi copy: bad SSID");
    }
    if !password.is_empty() && !(8..=63).contains(&password.len()) {
        return Err("wifi copy: bad password");
    }
    let mut credentials = WifiCredentials {
        ssid: String::new(),
        password: String::new(),
    };
    let _ = credentials.ssid.push_str(ssid);
    let _ = credentials.password.push_str(password);
    Ok(credentials)
}

/// 1 行目 SSID、2 行目パスワード。CR/LF のみ除去し、空行は読み飛ばす。
pub fn parse_credentials(bytes: &[u8]) -> Result<WifiCredentials, &'static str> {
    let text = core::str::from_utf8(bytes).map_err(|_| "wifi.txt: not UTF-8")?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.lines().filter(|line| !line.is_empty());
    let ssid = lines.next().ok_or("wifi.txt: SSID missing")?;
    let password = lines.next().unwrap_or("");
    if ssid.len() > 32 {
        return Err("wifi.txt: SSID too long");
    }
    if !password.is_empty() && !(8..=63).contains(&password.len()) {
        return Err("wifi.txt: password 8-63 chars");
    }
    let mut credentials = WifiCredentials {
        ssid: String::new(),
        password: String::new(),
    };
    credentials
        .ssid
        .push_str(ssid)
        .map_err(|_| "wifi.txt: SSID too long")?;
    credentials
        .password
        .push_str(password)
        .map_err(|_| "wifi.txt: password too long")?;
    Ok(credentials)
}

// ============================================================
// CYW43439 + embassy-net の起動
// ============================================================

/// CYW43439 に使うピンと DMA チャネル
pub struct Cyw43Pins {
    pub pwr: Peri<'static, PIN_23>,
    pub cs: Peri<'static, PIN_25>,
    pub dio: Peri<'static, PIN_24>,
    pub clk: Peri<'static, PIN_29>,
    pub dma: Peri<'static, DMA_CH4>,
}

/// 起動済みのネットワーク一式
pub struct Network {
    /// embassy-net スタック (Copy)。DHCP 完了は `stack.wait_config_up()` / `is_config_up()`
    pub stack: Stack<'static>,
    /// cyw43 の制御ハンドル (join / scan)
    pub control: cyw43::Control<'static>,
    pub mac: [u8; 6],
}

/// CYW43439 のファームウェアをロードし、cyw43 ドライバと embassy-net (DHCP) のタスクを起動する。
///
/// `pio1` は `Pio::new(p.PIO1, Irqs)` で作ったもの。SM0 と IRQ0 を PIO SPI に使い、
/// 残り (Common / SM1〜3) は drop せずに保持する (モジュール冒頭の注記)。
///
/// 最初に WL_REG_ON (GP23) を `CYW43_POWER_OFF_MS` の間 Low に保ち、CYW43439 を確実に電源断してから
/// cyw43 にピンを渡す (温かい再起動でも毎回コールドスタートにする)。`after_power_cycle` はその直後、
/// ファームウェア転送の前に呼ばれる (起動診断の段階記録用)。
pub async fn start(
    spawner: &Spawner,
    pio1: Pio<'static, PIO1>,
    pins: Cyw43Pins,
    power: PowerManagementMode,
    after_power_cycle: impl FnOnce(),
) -> Network {
    // WL_REG_ON を落として保持。cyw43 の `Bus::init` はこの後さらに 20 ms Low → High → 250 ms 待つ。
    let pwr = Output::new(pins.pwr, Level::Low);
    Timer::after_millis(CYW43_POWER_OFF_MS).await;
    after_power_cycle();
    let cs = Output::new(pins.cs, Level::High);

    let Pio {
        mut common,
        irq0,
        sm0,
        sm1,
        sm2,
        sm3,
        ..
    } = pio1;
    let spi: Cyw43Spi = PioSpi::new(
        &mut common,
        sm0,
        DEFAULT_CLOCK_DIVIDER,
        irq0,
        cs,
        pins.dio,
        pins.clk,
        pins.dma,
    );
    // PIO1 の Common / 未使用 SM は電源を切るまで解放しない (`on_pio_drop` で GPIO が切り離されるのを防ぐ)。
    core::mem::forget((common, sm1, sm2, sm3));

    static STATE: StaticCell<cyw43::State> = StaticCell::new();
    let state = STATE.init(cyw43::State::new());
    let (net_device, mut control, runner) = cyw43::new(state, pwr, spi, CYW43_FW).await;
    spawner.spawn(cyw43_task(runner)).unwrap();

    control.init(CYW43_CLM).await;
    control.set_power_management(power).await;
    let mac = control.address().await;
    defmt::info!("CYW43 MAC: {:02x}", mac);

    let seed = RoscRng.next_u64();
    static RESOURCES: StaticCell<StackResources<STACK_SOCKETS>> = StaticCell::new();
    let (stack, net_runner) = embassy_net::new(
        net_device,
        embassy_net::Config::dhcpv4(Default::default()),
        RESOURCES.init(StackResources::new()),
        seed,
    );
    spawner.spawn(net_task(net_runner)).unwrap();

    Network { stack, control, mac }
}

/// 再起動の直前に呼ぶ: AP から離脱し、WL_REG_ON (GP23) を Low に駆動して CYW43439 の電源を切る。
///
/// `reboot(FLASH_UPDATE)` などの温かい再起動は RP2350 だけをリセットし、CYW43439 は通電・接続したまま
/// 次のファームウェアに引き継がれる。次の版の `start` も `CYW43_POWER_OFF_MS` の間 Low に保つが、
/// こちらでも落としておき、合計の電源断時間を確実に確保する。
///
/// ピンの所有権: `start` は GP23 の `Output` を cyw43 の `Bus` に渡しており取り戻せない。`Bus` は
/// `init` (20 ms Low → High) の後は一切このピンに触らないので、ここでは `PIN_23::steal()` で
/// 同じピンの `Output` を作り直して Low に駆動する。作った `Output` は drop すると FUNCSEL が NULL に
/// 戻って駆動が外れる (embassy-rp `gpio` の `Drop`) ため `mem::forget` し、Low のまま再起動する。
/// 再起動後は IO バンクがリセットされ、次の版の `start` が改めて Low から始める。
pub async fn power_off_for_reboot(control: &mut cyw43::Control<'static>) {
    // 離脱 (失敗・応答なしは無視)。AP 側の接続状態も片付けておく
    if with_timeout(LEAVE_TIMEOUT, control.leave()).await.is_err() {
        defmt::warn!("cyw43 leave() timed out before reboot");
    }
    // Safety: PIN_23 は `start` で cyw43 の Bus に渡したが、Bus は init 後に触らない。再起動直前で、
    // 以後この関数の呼び出し元は戻ってこない (reboot する) 前提。
    let pwr = Output::new(unsafe { PIN_23::steal() }, Level::Low);
    Timer::after_millis(CYW43_REBOOT_POWER_OFF_MS).await;
    core::mem::forget(pwr);
    defmt::info!("CYW43 powered off (WL_REG_ON low) for reboot");
}

// ============================================================
// スキャン結果
// ============================================================

/// スキャン結果の保持上限 (SSID ごとに集約)
pub const MAX_SCAN_APS: usize = 32;

#[derive(Clone, Copy)]
pub struct ApEntry {
    pub ssid: [u8; 32],
    pub ssid_len: u8,
    pub rssi: i16,
    pub channel: u8,
}

impl ApEntry {
    pub fn ssid(&self) -> &[u8] {
        &self.ssid[..usize::from(self.ssid_len).min(32)]
    }
}

/// SSID ごとに最大 RSSI を保持しつつ結果を集約する (ステルス AP は捨てる)
pub fn merge_ap(aps: &mut Vec<ApEntry, MAX_SCAN_APS>, bss: &cyw43::BssInfo) {
    let ssid_len = usize::from(bss.ssid_len).min(32);
    if ssid_len == 0 {
        return;
    }
    let ssid = &bss.ssid[..ssid_len];
    if let Some(existing) = aps.iter_mut().find(|ap| ap.ssid() == ssid) {
        if bss.rssi > existing.rssi {
            existing.rssi = bss.rssi;
            existing.channel = (bss.chanspec & 0xff) as u8;
        }
        return;
    }
    let entry = ApEntry {
        ssid: bss.ssid,
        ssid_len: ssid_len as u8,
        rssi: bss.rssi,
        channel: (bss.chanspec & 0xff) as u8,
    };
    if aps.push(entry).is_err() {
        // 満杯なら最も弱い AP を置き換える
        let weakest = aps
            .iter()
            .enumerate()
            .min_by_key(|(_, ap)| ap.rssi)
            .map(|(index, ap)| (index, ap.rssi));
        if let Some((index, weakest_rssi)) = weakest
            && bss.rssi > weakest_rssi
        {
            aps[index] = entry;
        }
    }
}

/// 表示不能なバイトは '?' に置き換えて ASCII に丸める
pub fn ascii_label<const N: usize>(bytes: &[u8]) -> String<N> {
    let mut label = String::new();
    for &byte in bytes.iter().take(N) {
        let ch = if (0x20..=0x7e).contains(&byte) {
            byte as char
        } else {
            '?'
        };
        if label.push(ch).is_err() {
            break;
        }
    }
    label
}
