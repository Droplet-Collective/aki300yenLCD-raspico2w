//! Pico 2W + 300円LCD (LTA042B010F) ドライバ
//!
//! PIO を使用して 400x96 RGB666 TFT LCD を駆動する

#![no_std]
#![no_main]

use defmt::*;
use embassy_executor::Spawner;
use embassy_rp::gpio::{Level, Output};
use {defmt_rtt as _, panic_probe as _};

// RP2350 bootrom 用 IMAGE_DEF (版数付き) と picotool 用 binary_info を埋め込む
pico2w_300yen_lcd::firmware_image_def!();

// LCD ドライバモジュールはライブラリクレート (lib.rs) で定義
// Layer 3 以降で使用: use pico2w_300yen_lcd::{lcd, pins};

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let p = embassy_rp::init(Default::default());
    info!("Pico 2W + 300yen LCD initialized");

    // TODO: LCD ドライバ初期化 (Layer 3 以降で実装)

    loop {
        embassy_time::Timer::after_secs(1).await;
        info!("running...");
    }
}
