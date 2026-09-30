//! picotool 用 USB reset interface
//!
//! Pico SDK の `pico_stdio_usb` と同じ vendor interface (FF/00/01,
//! 文字列 "rp2xxx-reset") を公開し、class request 0x01 で BOOTSEL へ
//! 再起動する。`picotool load -f` がこのインターフェースを検出して
//! 実行中のファームウェアを BOOTSEL モードに移行させる。
//!
//! 使い方 (bin 側):
//!
//! ```ignore
//! bind_interrupts!(struct Irqs { USBCTRL_IRQ => UsbInterruptHandler<USB>; });
//! let usb = build_usb_device(UsbDriver::new(p.USB, Irqs), "My Firmware");
//! spawner.spawn(usb_task(usb)).unwrap(); // usb_task は device.run().await
//! ```

use embassy_rp::peripherals::USB;
use embassy_rp::usb::Driver as UsbDriver;
use embassy_usb::control::{OutResponse, Recipient, Request, RequestType};
use embassy_usb::msos::{
    CompatibleIdFeatureDescriptor, PropertyData, RegistryPropertyFeatureDescriptor, windows_version,
};
use embassy_usb::types::{InterfaceNumber, StringIndex};
use embassy_usb::{Builder as UsbBuilder, Config as UsbConfig, Handler, UsbDevice};
use static_cell::StaticCell;

/// `build_usb_device` が返す USB デバイス型
pub type ResetUsbDevice = UsbDevice<'static, UsbDriver<'static, USB>>;

/*
Adapted from rp-usb-reset 0.0.2 (https://github.com/sunipkm/rp-usb-reset).
Copyright 2024 Sunip K. Mukherjee

Permission is hereby granted, free of charge, to any person obtaining a copy of
this software and associated documentation files (the "Software"), to deal in
the Software without restriction, including without limitation the rights to
use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies
of the Software, and to permit persons to whom the Software is furnished to do
so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
*/

// picotool が -f で検出する RP2 USB reset interface。SDK と同じ
// vendor function (FF/00/01) と class request 01 を公開する。
struct UsbResetHandler {
    interface: InterfaceNumber,
    description: StringIndex,
}

impl UsbResetHandler {
    fn new(builder: &mut UsbBuilder<'static, UsbDriver<'static, USB>>) -> Self {
        let mut function = builder.function(0xff, 0x00, 0x01);
        let mut interface = function.interface();
        let number = interface.interface_number();
        let description = interface.string();
        interface.alt_setting(0xff, 0x00, 0x01, Some(description));
        Self {
            interface: number,
            description,
        }
    }
}

impl Handler for UsbResetHandler {
    fn get_string(&mut self, index: StringIndex, _lang_id: u16) -> Option<&str> {
        (index == self.description).then_some("rp2xxx-reset")
    }

    fn control_out(&mut self, request: Request, _data: &[u8]) -> Option<OutResponse> {
        if request.request_type == RequestType::Class
            && request.recipient == Recipient::Interface
            && request.index == u8::from(self.interface) as u16
            && request.request == 0x01
        {
            // 意図したリセットなので、boot_trace の記録 (0.4.1 の ticker は通常運転中も記録する) を消す。
            // 消さないと次の起動が「記録の無いウォッチドッグ・リセット」と誤表示する。
            crate::boot_trace::clear();
            embassy_rp::rom_data::reset_to_usb_boot(0, 0);
            Some(OutResponse::Accepted)
        } else {
            None
        }
    }
}

pub fn usb_chip_serial() -> Option<&'static str> {
    // Pico SDK の pico_get_unique_board_id_string と同じ RP2350 CHIP_INFO。
    // ROM USB BOOTSEL の serial と一致させ、picotool --ser を維持する。
    let mut info = [0u32; 9];
    let words = unsafe { embassy_rp::rom_data::get_sys_info(info.as_mut_ptr(), info.len(), 1) };
    if words != 4 || info[0] != 1 {
        defmt::warn!("RP2350 CHIP_INFO unavailable: {}", words);
        return None;
    }
    let chip_id = (u64::from(info[3]) << 32) | u64::from(info[2]);
    static SERIAL: StaticCell<[u8; 16]> = StaticCell::new();
    let serial = SERIAL.init([0; 16]);
    for (index, digit) in serial.iter_mut().enumerate() {
        let nibble = ((chip_id >> (60 - index * 4)) & 0xf) as u8;
        *digit = b"0123456789ABCDEF"[nibble as usize];
    }
    core::str::from_utf8(serial).ok()
}

pub fn build_usb_device(
    driver: UsbDriver<'static, USB>,
    product: &'static str,
) -> UsbDevice<'static, UsbDriver<'static, USB>> {
    static CONFIG_DESCRIPTOR: StaticCell<[u8; 256]> = StaticCell::new();
    static BOS_DESCRIPTOR: StaticCell<[u8; 256]> = StaticCell::new();
    static MSOS_DESCRIPTOR: StaticCell<[u8; 256]> = StaticCell::new();
    static CONTROL_BUFFER: StaticCell<[u8; 64]> = StaticCell::new();
    static RESET_HANDLER: StaticCell<UsbResetHandler> = StaticCell::new();

    let mut config = UsbConfig::new(0x2e8a, 0x0009);
    // 単一の reset interface を WinUSB デバイスとしてバインドする。
    config.device_class = 0xff;
    config.device_sub_class = 0;
    config.device_protocol = 1;
    config.composite_with_iads = false;
    config.device_release = 0x0020;
    config.manufacturer = Some("Raspberry Pi");
    config.product = Some(product);
    config.serial_number = usb_chip_serial();
    config.max_packet_size_0 = 64;
    config.max_power = 100;

    let mut builder = UsbBuilder::new(
        driver,
        config,
        CONFIG_DESCRIPTOR.init([0; 256]),
        BOS_DESCRIPTOR.init([0; 256]),
        MSOS_DESCRIPTOR.init([0; 256]),
        CONTROL_BUFFER.init([0; 64]),
    );
    builder.msos_descriptor(windows_version::WIN8_1, 0x20);
    builder.msos_feature(CompatibleIdFeatureDescriptor::new("WINUSB", ""));
    builder.msos_feature(RegistryPropertyFeatureDescriptor::new(
        "DeviceInterfaceGUIDs",
        PropertyData::RegMultiSz(&["{CDB3B5AD-293B-4663-AA36-1AAE46463776}"]),
    ));
    let reset_handler = RESET_HANDLER.init(UsbResetHandler::new(&mut builder));
    builder.handler(reset_handler);
    builder.build()
}
