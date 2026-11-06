//! ESP32-S3 firmware (plain esp-hal): runs the `fast` interpreter on the
//! chunked guest image and the same kernels natively, timed with the Xtensa
//! CCOUNT cycle register, and prints the results on UART0.
#![no_std]
#![no_main]

use core::fmt::Write;
use esp_hal::uart::{Config, Uart};
use esp_hal::xtensa_lx::timer::get_cycle_count;

esp_bootloader_esp_idf::esp_app_desc!();

#[esp_hal::main]
fn main() -> ! {
    let peripherals = esp_hal::init(esp_hal::Config::default());
    let mut uart = Uart::new(peripherals.UART0, Config::default())
        .unwrap()
        .with_tx(peripherals.GPIO43);
    let _ = writeln!(uart, "MVM esp32s3 start");
    bench_core::run_all("esp32s3", get_cycle_count, &mut uart);
    // Round seven: mesh ops. kernel.timer.now.v1 and section timing both read CCOUNT.
    mvm_meshops_tests::run_all(
        "esp32s3",
        &mut uart,
        get_cycle_count,
        get_cycle_count,
        mvm_meshops_tests::N_CALLS,
    );
    let _ = uart.flush();
    loop {}
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    // esp-println writes through the ROM UART routines, so this works even if
    // the panic happens before our UART driver exists.
    esp_println::println!("MVM esp32s3 panic: {info}");
    loop {}
}
