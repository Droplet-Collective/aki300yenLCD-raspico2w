//! microSD (GPIO ビットバング SPI) と読み取り専用 FAT ボリューム
//!
//! 基板配線 (CS=GP26, CMD/MOSI=GP27, CLK=GP28, DAT0/MISO=GP0) は
//! ハードウェア SPI のピン組み合わせではないため GPIO で SPI を生成する。
//! `sd_bmp_viewer` と同じ実装を bin 間で共有できるよう切り出したもの。

use core::convert::Infallible;
use embassy_rp::Peri;
use embassy_rp::gpio::{Input, Level, Output, Pull};
use embassy_rp::peripherals::{PIN_0, PIN_26, PIN_27, PIN_28};
use embassy_time::Delay;
use embedded_hal::delay::DelayNs;
use embedded_hal::spi::{ErrorType, Operation, SpiDevice};
use embedded_sdmmc::{
    Block, BlockCount, BlockDevice, BlockIdx, Error as FsError, File, SdCard, SdCardError,
    TimeSource, Timestamp, VolumeManager,
};

// SD の SPI 配線は KiCad 基板の SD_DAT0/SD_CS/SD_CMD/SD_CLK と Pico 2W の
// パッド 1/31/32/34 を参照。ハードウェア SPI のピン組み合わせではない。
pub struct BitBangSd {
    miso: Input<'static>,
    cs: Output<'static>,
    mosi: Output<'static>,
    clk: Output<'static>,
    slow: bool,
}

impl BitBangSd {
    pub fn new(
        miso: Peri<'static, PIN_0>,
        cs: Peri<'static, PIN_26>,
        mosi: Peri<'static, PIN_27>,
        clk: Peri<'static, PIN_28>,
    ) -> Self {
        let mut bus = Self {
            miso: Input::new(miso, Pull::Up),
            cs: Output::new(cs, Level::High),
            mosi: Output::new(mosi, Level::High),
            clk: Output::new(clk, Level::Low),
            slow: true,
        };
        // SD SPI モードへ入る前に CS=High, CMD=High で 80 クロック。
        for _ in 0..10 {
            bus.transfer_byte(0xff);
        }
        // embedded-sdmmc はコマンドと応答を別々の SpiDevice 呼び出しで
        // 送受信する。共有バスはないので、読み終わるまで CS を保持する。
        bus.cs.set_low();
        bus.transfer_byte(0xff);
        bus
    }

    #[inline]
    fn half_clock_delay(&self) {
        if self.slow {
            let mut delay = Delay;
            delay.delay_us(2); // 初期化中は 400 kHz 以下
        } else {
            // 速い読み出し (写真の背景、v0.4.0〜): 半周期 ≈ 0.2 µs + GPIO 操作 → SCK ≈ 1.5〜2 MHz。
            // SD の SPI モードの上限 25 MHz より十分遅い。読み誤りは CRC (embedded-sdmmc の既定で有効) で分かる。
            cortex_m::asm::delay(FAST_HALF_CLOCK_CYCLES);
        }
    }

    fn transfer_byte(&mut self, out: u8) -> u8 {
        let mut input = 0u8;
        for bit in (0..8).rev() {
            self.clk.set_low();
            if out & (1 << bit) != 0 {
                self.mosi.set_high();
            } else {
                self.mosi.set_low();
            }
            self.half_clock_delay();
            self.clk.set_high();
            self.half_clock_delay();
            input = (input << 1) | u8::from(self.miso.is_high());
        }
        self.clk.set_low();
        input
    }
}

impl Drop for BitBangSd {
    fn drop(&mut self) {
        self.cs.set_high();
        self.clk.set_low();
    }
}

impl ErrorType for BitBangSd {
    type Error = Infallible;
}

impl SpiDevice<u8> for BitBangSd {
    fn transaction(&mut self, operations: &mut [Operation<'_, u8>]) -> Result<(), Self::Error> {
        for operation in operations {
            match operation {
                Operation::Read(read) => {
                    for byte in read.iter_mut() {
                        *byte = self.transfer_byte(0xff);
                    }
                }
                Operation::Write(write) => {
                    for &byte in write.iter() {
                        self.transfer_byte(byte);
                    }
                }
                Operation::Transfer(read, write) => {
                    for i in 0..read.len().max(write.len()) {
                        let byte = self.transfer_byte(write.get(i).copied().unwrap_or(0xff));
                        if let Some(slot) = read.get_mut(i) {
                            *slot = byte;
                        }
                    }
                }
                Operation::TransferInPlace(data) => {
                    for byte in data.iter_mut() {
                        *byte = self.transfer_byte(*byte);
                    }
                }
                Operation::DelayNs(ns) => {
                    let mut delay = Delay;
                    delay.delay_ns(*ns);
                }
            }
        }
        Ok(())
    }
}

/// 速い読み出しの半周期 (CPU サイクル、150 MHz で ≈ 0.2 µs)
const FAST_HALF_CLOCK_CYCLES: u32 = 30;

pub struct FixedTime;

impl TimeSource for FixedTime {
    fn get_timestamp(&self) -> Timestamp {
        // 読み取り専用なのでタイムスタンプは使わない。
        Timestamp {
            year_since_1970: 10,
            zero_indexed_month: 0,
            zero_indexed_day: 0,
            hours: 0,
            minutes: 0,
            seconds: 0,
        }
    }
}

// embedded-sdmmc は MBR のあるカードだけを受け付ける。先頭セクタが
// FAT ブートセクタのカードでは、読み取り専用の仮想 MBR を 1 セクタ挿入する。
pub struct ReadOnlyVolumeDevice {
    card: SdCard<BitBangSd, Delay>,
    card_blocks: u32,
    superfloppy: bool,
    fat32: bool,
}

impl BlockDevice for ReadOnlyVolumeDevice {
    type Error = SdCardError;

    fn read(&self, blocks: &mut [Block], start: BlockIdx) -> Result<(), Self::Error> {
        if blocks.is_empty() {
            return Ok(());
        }
        if !self.superfloppy {
            return self.card.read(blocks, start);
        }
        if start.0 == 0 {
            let (mbr, rest) = blocks.split_first_mut().unwrap();
            mbr.contents.fill(0);
            mbr[450] = if self.fat32 { 0x0c } else { 0x0e };
            mbr[454..458].copy_from_slice(&1u32.to_le_bytes());
            mbr[458..462].copy_from_slice(&self.card_blocks.to_le_bytes());
            mbr[510] = 0x55;
            mbr[511] = 0xaa;
            if !rest.is_empty() {
                self.card.read(rest, BlockIdx(0))?;
            }
            Ok(())
        } else {
            self.card.read(blocks, BlockIdx(start.0 - 1))
        }
    }

    fn write(&self, _blocks: &[Block], _start: BlockIdx) -> Result<(), Self::Error> {
        Err(SdCardError::WriteError)
    }

    fn num_blocks(&self) -> Result<BlockCount, Self::Error> {
        self.card_blocks
            .checked_add(u32::from(self.superfloppy))
            .map(BlockCount)
            .ok_or(SdCardError::BadState)
    }
}

pub fn is_fat_boot_sector(sector: &[u8; 512]) -> bool {
    let bytes_per_sector = u16::from_le_bytes([sector[11], sector[12]]);
    let reserved = u16::from_le_bytes([sector[14], sector[15]]);
    let total16 = u16::from_le_bytes([sector[19], sector[20]]);
    let total32 = u32::from_le_bytes([sector[32], sector[33], sector[34], sector[35]]);
    sector[510..512] == [0x55, 0xaa]
        && matches!(sector[0], 0xeb | 0xe9)
        && bytes_per_sector == 512
        && sector[13].is_power_of_two()
        && reserved > 0
        && sector[16] > 0
        && (total16 != 0 || total32 != 0)
}

pub fn volume_error(error: FsError<SdCardError>) -> &'static str {
    match error {
        FsError::FormatError(message) => message,
        FsError::DeviceError(SdCardError::TimeoutReadBuffer) => "SD SECTOR TIMEOUT",
        FsError::DeviceError(SdCardError::CrcError(_, _)) => "SD SECTOR CRC ERROR",
        FsError::DeviceError(_) => "SD SECTOR READ ERROR",
        _ => "FAT VOLUME ERROR",
    }
}

pub type SdFile<'a> = File<'a, ReadOnlyVolumeDevice, FixedTime, 4, 4, 1>;
pub type SdVolumeManager = VolumeManager<ReadOnlyVolumeDevice, FixedTime>;

pub fn init_sd(
    miso: Peri<'static, PIN_0>,
    cs: Peri<'static, PIN_26>,
    mosi: Peri<'static, PIN_27>,
    clk: Peri<'static, PIN_28>,
) -> Result<SdVolumeManager, &'static str> {
    let sdcard = SdCard::new(BitBangSd::new(miso, cs, mosi, clk), Delay);
    let card_bytes = sdcard.num_bytes().map_err(|_| "SD INIT FAILED")?;
    let card_blocks = u32::try_from(card_bytes / 512).map_err(|_| "SD CARD TOO LARGE")?;
    let mut sector0 = [Block::new()];
    sdcard
        .read(&mut sector0, BlockIdx(0))
        .map_err(|_| "SD SECTOR 0 ERROR")?;
    let superfloppy = is_fat_boot_sector(&sector0[0].contents);
    let fat32 = u16::from_le_bytes([sector0[0][22], sector0[0][23]]) == 0;
    defmt::info!(
        "SD card: {} blocks, direct FAT={}",
        card_blocks,
        superfloppy
    );

    // GPIO SPI はカードの読み取りが安定していることを実機で確認するまで
    // 初期化時と同じ低速設定に保つ。
    let device = ReadOnlyVolumeDevice {
        card: sdcard,
        card_blocks,
        superfloppy,
        fat32,
    };
    Ok(VolumeManager::new(device, FixedTime))
}

/// SD ルートの `name` (8.3 形式、例 `TICKER.TXT`) を `buf` の長さまで読む。戻り値は読んだバイト数。
/// ファイルが無ければ `Err("<name> not found")` ではなく呼び出し側が区別できるよう [`ReadError::NotFound`]。
pub fn read_root_file(volume_mgr: &SdVolumeManager, name: &str, buf: &mut [u8]) -> Result<usize, ReadError> {
    let volume = volume_mgr
        .open_volume(embedded_sdmmc::VolumeIdx(0))
        .map_err(|e| ReadError::Other(volume_error(e)))?;
    let root = volume.open_root_dir().map_err(|_| ReadError::Other("ROOT DIR ERROR"))?;
    let file = root
        .open_file_in_dir(name, embedded_sdmmc::Mode::ReadOnly)
        .map_err(|_| ReadError::NotFound)?;
    let mut len = 0;
    while len < buf.len() {
        let count = file.read(&mut buf[len..]).map_err(|_| ReadError::Other("read error"))?;
        if count == 0 {
            break;
        }
        len += count;
    }
    Ok(len)
}

/// [`read_root_file`] の失敗理由
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadError {
    /// ファイルが無い (任意ファイルなら既定値で続ける)
    NotFound,
    /// ボリューム / ディレクトリ / 読み取りの失敗
    Other(&'static str),
}

/// SD の読み出し速度を切り替える (`fast = false` で初期化時と同じ低速 ≈ 200 kHz)。
/// `ticker` は wifi.txt / ticker.txt を低速で読んだあと、写真の読み込みだけを速くする。
pub fn set_fast(volume_mgr: &SdVolumeManager, fast: bool) {
    volume_mgr.device(|dev| dev.card.spi(|bus| bus.slow = !fast));
}

/// SD ルートの `*.BMP` (8.3 形式の名前、ディレクトリ / 隠し / システム / `_` で始まる名前は除く) を
/// 名前順に最大 `N` 個。macOS が作る `._IMAGE.BMP` (短い名前は `_IMAGE~1.BMP`) を拾わないよう `_` 始まりは除く。
pub fn list_root_bmps<const N: usize>(volume_mgr: &SdVolumeManager) -> Result<heapless::Vec<heapless::String<12>, N>, &'static str> {
    let volume = volume_mgr
        .open_volume(embedded_sdmmc::VolumeIdx(0))
        .map_err(volume_error)?;
    let root = volume.open_root_dir().map_err(|_| "ROOT DIR ERROR")?;
    let mut names: heapless::Vec<heapless::String<12>, N> = heapless::Vec::new();
    root.iterate_dir(|entry| {
        let attr = entry.attributes;
        let ext = entry.name.extension();
        let base = entry.name.base_name();
        if !attr.is_directory()
            && !attr.is_hidden()
            && !attr.is_system()
            && !attr.is_volume()
            && ext.eq_ignore_ascii_case(b"BMP")
            && !base.is_empty()
            && base[0] != b'_'
            && entry.size > 54
        {
            let mut name: heapless::String<12> = heapless::String::new();
            for &b in base.iter().chain(b".").chain(ext.iter()) {
                let _ = name.push(b as char);
            }
            if names.push(name).is_err() {
                return core::ops::ControlFlow::Break(());
            }
        }
        core::ops::ControlFlow::Continue(())
    })
    .map_err(|_| "ROOT DIR READ ERROR")?;
    names.sort_unstable();
    Ok(names)
}
