//! SD カードの IMAGE.BMP と IMAGE2.BMP を交互に表示する。
//!
//! SD は基板配線に合わせた GPIO SPI（CS=GP26, CMD=GP27, CLK=GP28,
//! DAT0=GP0）。FAT ボリュームのルートにある非圧縮 24-bit BMP を読む。
//! 次の BMP を `lcd::display` のバックバッファ (表示領域 400×96 のみ) へ先読みし、
//! 走査を続けたまま垂直ブランキングで切り替える (`Display::present`)。

#![no_std]
#![no_main]

use core::convert::Infallible;
use embassy_executor::Spawner;
use embassy_rp::Peri;
use embassy_rp::bind_interrupts;
use embassy_rp::gpio::{Input, Level, Output, Pull};
use embassy_rp::peripherals::*;
use embassy_rp::pio::InterruptHandler;
use embassy_rp::usb::{Driver as UsbDriver, InterruptHandler as UsbInterruptHandler};
use embassy_time::{Delay, Duration, Instant, Timer};
use embassy_usb::UsbDevice;
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::mono_font::ascii::FONT_6X10;
use embedded_graphics::pixelcolor::Rgb666;
use embedded_graphics::prelude::*;
use embedded_graphics::text::Text;
use embedded_hal::delay::DelayNs;
use embedded_hal::spi::{ErrorType, Operation, SpiDevice};
use embedded_sdmmc::{
    Block, BlockCount, BlockDevice, BlockIdx, Error as FsError, File, Mode, SdCard, SdCardError,
    TimeSource, Timestamp, VolumeIdx, VolumeManager,
};
use pico2w_300yen_lcd::lcd::display::{BACK_WIDTH, BackBuffer, Display, DisplayPins, FrameIrqHandler};
use pico2w_300yen_lcd::lcd::framebuffer::{ACTIVE_HEIGHT, BLACK, rgb666};
use pico2w_300yen_lcd::lcd::timing::H_ACTIVE;
use pico2w_300yen_lcd::usb_reset::build_usb_device;
use {defmt_rtt as _, panic_probe as _};

// RP2350 bootrom 用 IMAGE_DEF (版数付き) と picotool 用 binary_info を埋め込む
pico2w_300yen_lcd::firmware_image_def!();

bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => InterruptHandler<PIO0>;
    DMA_IRQ_1 => FrameIrqHandler;
    USBCTRL_IRQ => UsbInterruptHandler<USB>;
});

#[embassy_executor::task]
async fn usb_task(mut device: UsbDevice<'static, UsbDriver<'static, USB>>) -> ! {
    device.run().await
}

const SLIDES: [&str; 2] = ["IMAGE.BMP", "IMAGE2.BMP"];
const SLIDE_SECONDS: u64 = 10;

// SD の SPI 配線は KiCad 基板の SD_DAT0/SD_CS/SD_CMD/SD_CLK と Pico 2W の
// パッド 1/31/32/34 を参照。ハードウェア SPI のピン組み合わせではない。
struct BitBangSd {
    miso: Input<'static>,
    cs: Output<'static>,
    mosi: Output<'static>,
    clk: Output<'static>,
    slow: bool,
}

impl BitBangSd {
    fn new(
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

struct FixedTime;

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
struct ReadOnlyVolumeDevice {
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

fn is_fat_boot_sector(sector: &[u8; 512]) -> bool {
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

fn volume_error(error: FsError<SdCardError>) -> &'static str {
    match error {
        FsError::FormatError(message) => message,
        FsError::DeviceError(SdCardError::TimeoutReadBuffer) => "SD SECTOR TIMEOUT",
        FsError::DeviceError(SdCardError::CrcError(_, _)) => "SD SECTOR CRC ERROR",
        FsError::DeviceError(_) => "SD SECTOR READ ERROR",
        _ => "FAT VOLUME ERROR",
    }
}

type SdFile<'a> = File<'a, ReadOnlyVolumeDevice, FixedTime, 4, 4, 1>;
type SdVolumeManager = VolumeManager<ReadOnlyVolumeDevice, FixedTime>;

fn init_sd(
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

async fn load_image(
    staged: &mut BackBuffer,
    volume_mgr: &SdVolumeManager,
    filename: &'static str,
) -> Result<(u32, u32), &'static str> {
    let volume = volume_mgr.open_volume(VolumeIdx(0)).map_err(volume_error)?;
    let root = volume.open_root_dir().map_err(|_| "ROOT DIR ERROR")?;
    let file = root
        .open_file_in_dir(filename, Mode::ReadOnly)
        .map_err(|_| match filename {
            "IMAGE.BMP" => "IMAGE.BMP MISSING",
            _ => "IMAGE2.BMP MISSING",
        })?;
    draw_bmp(staged, &file).await
}

fn read_exact(file: &SdFile<'_>, mut bytes: &mut [u8]) -> Result<(), &'static str> {
    while !bytes.is_empty() {
        let count = file.read(bytes).map_err(|_| "SD READ ERROR")?;
        if count == 0 {
            return Err("TRUNCATED BMP");
        }
        bytes = &mut bytes[count..];
    }
    Ok(())
}

async fn draw_bmp(
    staged: &mut BackBuffer,
    file: &SdFile<'_>,
) -> Result<(u32, u32), &'static str> {
    let file_len = file.length();
    if file_len < 54 {
        return Err("BAD BMP HEADER");
    }
    let mut header = [0u8; 54];
    read_exact(file, &mut header)?;
    if &header[0..2] != b"BM" {
        return Err("NOT A BMP FILE");
    }
    let pixel_start = u32::from_le_bytes(header[10..14].try_into().unwrap());
    let dib_size = u32::from_le_bytes(header[14..18].try_into().unwrap());
    let width = i32::from_le_bytes(header[18..22].try_into().unwrap());
    let signed_height = i32::from_le_bytes(header[22..26].try_into().unwrap());
    let planes = u16::from_le_bytes(header[26..28].try_into().unwrap());
    let bits_per_pixel = u16::from_le_bytes(header[28..30].try_into().unwrap());
    let compression = u32::from_le_bytes(header[30..34].try_into().unwrap());
    if dib_size < 40
        || pixel_start < 14u32.saturating_add(dib_size)
        || planes != 1
        || bits_per_pixel != 24
        || compression != 0
        || width <= 0
        || signed_height == 0
        || signed_height == i32::MIN
    {
        return Err("UNSUPPORTED BMP");
    }

    let width = width as u32;
    let height = signed_height.unsigned_abs();
    let row_bytes = width
        .checked_mul(3)
        .and_then(|v| v.checked_add(3))
        .map(|v| v & !3)
        .ok_or("BMP TOO LARGE")?;
    let image_end = row_bytes
        .checked_mul(height)
        .and_then(|v| v.checked_add(pixel_start))
        .ok_or("BMP TOO LARGE")?;
    if image_end > file_len {
        return Err("TRUNCATED BMP");
    }

    let draw_width = width.min(H_ACTIVE) as usize;
    let draw_height = height.min(ACTIVE_HEIGHT as u32) as usize;
    let src_x = (width - draw_width as u32) / 2;
    let src_y = (height - draw_height as u32) / 2;
    let dst_x = (H_ACTIVE as usize - draw_width) / 2;
    let dst_y = (ACTIVE_HEIGHT - draw_height) / 2;
    let mut row = [0u8; 64 * 3];
    staged.clear(BLACK);

    for dy in 0..draw_height {
        let source_y = src_y + dy as u32;
        let file_y = if signed_height > 0 {
            height - 1 - source_y // BMP の通常の下から上への格納
        } else {
            source_y // top-down BMP
        };
        let offset = pixel_start + file_y * row_bytes + src_x * 3;
        file.seek_from_start(offset).map_err(|_| "BMP SEEK ERROR")?;

        for x in (0..draw_width).step_by(64) {
            let count = (draw_width - x).min(64);
            read_exact(file, &mut row[..count * 3])?;
            for (dx, bgr) in row[..count * 3].chunks_exact(3).enumerate() {
                staged.data[(dst_y + dy) * BACK_WIDTH + dst_x + x + dx] = rgb666(
                    (bgr[2] >> 2) as u32,
                    (bgr[1] >> 2) as u32,
                    (bgr[0] >> 2) as u32,
                );
            }
        }
        // GPIO SPI は同期処理。各行の後で USB reset task に実行機会を渡す。
        Timer::after_millis(1).await;
    }
    Ok((width, height))
}

// ============================================================
// main: 走査中に次の BMP を先読みし、10秒ごとに表示を切り替える
// ============================================================

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());
    let volume_mgr = init_sd(p.PIN_0, p.PIN_26, p.PIN_27, p.PIN_28);

    let mut display = Display::new(DisplayPins {
        pio0: p.PIO0,
        pin2: p.PIN_2,
        pin3: p.PIN_3,
        pin4: p.PIN_4,
        pin5: p.PIN_5,
        pin6: p.PIN_6,
        pin7: p.PIN_7,
        pin8: p.PIN_8,
        pin9: p.PIN_9,
        pin10: p.PIN_10,
        pin11: p.PIN_11,
        pin12: p.PIN_12,
        pin13: p.PIN_13,
        pin14: p.PIN_14,
        pin15: p.PIN_15,
        pin16: p.PIN_16,
        pin17: p.PIN_17,
        pin18: p.PIN_18,
        pin19: p.PIN_19,
        pin20: p.PIN_20,
        pin21: p.PIN_21,
        pin22: p.PIN_22,
        dma_ch0: p.DMA_CH0,
        dma_ch1: p.DMA_CH1,
        dma_ch2: p.DMA_CH2,
        dma_ch3: p.DMA_CH3,
    });

    // 最初の BMP (または エラー表示) をバックバッファに描いてから走査を開始する。
    match &volume_mgr {
        Ok(manager) => match load_image(display.back(), manager, SLIDES[0]).await {
            Ok((width, height)) => {
                defmt::info!("{} loaded: {}x{}", SLIDES[0], width, height);
            }
            Err(message) => {
                defmt::error!("{}: {}", SLIDES[0], message);
                draw_error(display.back(), message);
            }
        },
        Err(message) => {
            defmt::error!("BMP viewer: {}", message);
            draw_error(display.back(), message);
        }
    }
    display.start(Irqs);

    let usb = build_usb_device(UsbDriver::new(p.USB, Irqs), "SD BMP Viewer");
    spawner.spawn(usb_task(usb)).unwrap();

    if let Ok(manager) = volume_mgr {
        // USB task を起動させてから次の BMP を読む。
        Timer::after_millis(1).await;
        let mut slide = 0;
        let mut displayed_at = Instant::now();
        loop {
            let next_slide = (slide + 1) % SLIDES.len();
            // DMA がフロントの現在画像を走査している間に、バックバッファへ先読みする。
            let loaded = load_image(display.back(), &manager, SLIDES[next_slide]).await;
            Timer::at(displayed_at + Duration::from_secs(SLIDE_SECONDS)).await;

            match loaded {
                Ok((width, height)) => {
                    defmt::info!("{} loaded: {}x{}", SLIDES[next_slide], width, height);
                }
                Err(message) => {
                    defmt::error!("{}: {}", SLIDES[next_slide], message);
                    draw_error(display.back(), message);
                }
            }
            // 垂直ブランキングで一括反映 (新旧の行が混ざらない)
            display.present().await;
            slide = next_slide;
            displayed_at = Instant::now();
        }
    } else {
        core::future::pending::<()>().await;
    }
}

fn draw_error(frame: &mut BackBuffer, message: &'static str) {
    frame.clear(BLACK);
    let style = MonoTextStyle::new(&FONT_6X10, Rgb666::WHITE);
    Text::new(
        "SD BMP viewer",
        embedded_graphics::geometry::Point::new(10, 20),
        style,
    )
    .draw(frame)
    .unwrap();
    Text::new(
        message,
        embedded_graphics::geometry::Point::new(10, 40),
        style,
    )
    .draw(frame)
    .unwrap();
}
