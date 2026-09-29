//! Layer 0: GPIO 基本動作確認
//!
//! GP2-GP22 を出力モードに設定し、全ピンをトグルする。
//! オシロスコープまたはロジックアナライザで信号を確認する。
//!
//! # 合格基準
//! - [ ] defmt ログが RTT で表示される
//! - [ ] GP2-GP22 の全ピンでトグル信号が確認できる
//! - [ ] FPC 経由で LCD の対応ピンに信号が到達している

#![no_std]
#![no_main]

use defmt::*;
use embassy_executor::Spawner;
use embassy_rp::gpio::{Level, Output};
use embassy_time::Timer;
use {defmt_rtt as _, panic_probe as _};

// RP2350 bootrom 用 IMAGE_DEF (版数付き) と picotool 用 binary_info を埋め込む
pico2w_300yen_lcd::firmware_image_def!();

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let p = embassy_rp::init(Default::default());
    info!("Layer 0: GPIO toggle test started");

    // LCD 接続ピン (GP2-GP22) を出力に設定
    let mut pins: [Output; 21] = [
        Output::new(p.PIN_2, Level::Low),   // B5
        Output::new(p.PIN_3, Level::Low),   // B4
        Output::new(p.PIN_4, Level::Low),   // B3
        Output::new(p.PIN_5, Level::Low),   // B2
        Output::new(p.PIN_6, Level::Low),   // B1
        Output::new(p.PIN_7, Level::Low),   // B0
        Output::new(p.PIN_8, Level::Low),   // G5
        Output::new(p.PIN_9, Level::Low),   // G4
        Output::new(p.PIN_10, Level::Low),  // G3
        Output::new(p.PIN_11, Level::Low),  // G2
        Output::new(p.PIN_12, Level::Low),  // G1
        Output::new(p.PIN_13, Level::Low),  // G0
        Output::new(p.PIN_14, Level::Low),  // R5
        Output::new(p.PIN_15, Level::Low),  // R4
        Output::new(p.PIN_16, Level::Low),  // R3
        Output::new(p.PIN_17, Level::Low),  // R2
        Output::new(p.PIN_18, Level::Low),  // R1
        Output::new(p.PIN_19, Level::Low),  // R0
        Output::new(p.PIN_20, Level::Low),  // NCLK
        Output::new(p.PIN_21, Level::Low),  // HSYNC
        Output::new(p.PIN_22, Level::Low),  // VSYNC
    ];

    info!("All LCD pins (GP2-GP22) configured as output");

    // テスト1: 全ピン同時トグル
    info!("Test 1: Toggle all pins simultaneously");
    for cycle in 0..10u32 {
        for pin in pins.iter_mut() {
            pin.set_high();
        }
        Timer::after_millis(500).await;

        for pin in pins.iter_mut() {
            pin.set_low();
        }
        Timer::after_millis(500).await;

        info!("Cycle {} complete", cycle);
    }

    // テスト2: ピンを1本ずつトグル（配線確認用）
    info!("Test 2: Toggle pins one by one");
    let pin_names = [
        "B5(GP2)", "B4(GP3)", "B3(GP4)", "B2(GP5)", "B1(GP6)", "B0(GP7)",
        "G5(GP8)", "G4(GP9)", "G3(GP10)", "G2(GP11)", "G1(GP12)", "G0(GP13)",
        "R5(GP14)", "R4(GP15)", "R3(GP16)", "R2(GP17)", "R1(GP18)", "R0(GP19)",
        "NCLK(GP20)", "HSYNC(GP21)", "VSYNC(GP22)",
    ];

    for (i, pin) in pins.iter_mut().enumerate() {
        info!("Toggling pin: {}", pin_names[i]);
        for _ in 0..5 {
            pin.set_high();
            Timer::after_millis(200).await;
            pin.set_low();
            Timer::after_millis(200).await;
        }
    }

    info!("Layer 0: GPIO toggle test complete!");
    info!("Verify all signals with oscilloscope or logic analyzer");

    loop {
        Timer::after_secs(10).await;
    }
}
