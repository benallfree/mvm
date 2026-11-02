//! The ESP32-S3 kernel image. A stub until 0.4.

#![no_std]
#![no_main]

esp_bootloader_esp_idf::esp_app_desc!();

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}

#[esp_hal::main]
fn main() -> ! {
    let _peripherals = esp_hal::init(esp_hal::Config::default());
    loop {}
}
