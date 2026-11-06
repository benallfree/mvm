//! ESP32-S3 firmware (plain esp-hal): runs the `fast` interpreter on the
//! chunked guest image and the same kernels natively, timed with the Xtensa
//! CCOUNT cycle register, then the mesh-op tests (rounds 7-7d). Output on
//! UART0 (TX GPIO43, RX GPIO44).
//!
//! Round 7d, and why there is no interrupt here: under Espressif's QEMU,
//! every interrupt source tried hung the CPU as soon as it was enabled
//! (esp-rtos's scheduler tick, a TIMG0 periodic timer interrupt, and the
//! Xtensa core timer CCOMPARE0 with a handler that only re-arms it). So on
//! this target the fake drivers tick *cooperatively*: the interpreter's wait
//! loop runs one driver tick per 24,000 CCOUNT (a tenth of a millisecond at 240 MHz)
//! and drains the UART for the test-30 script. Every async test runs except
//! 28 (drivers preempting a long guest call), which needs a preemptive
//! driver context and reports itself as skipped. See ASYNC.md.
//!
//! QEMU's ESP32-S3 GPIO can't be driven from outside, so the script's button
//! press arrives as a typed line ("E3") instead of a pin edge.
#![no_std]
#![no_main]

use core::fmt::Write;
use esp_hal::uart::{Config, Uart, UartRx};
use esp_hal::xtensa_lx::timer::get_cycle_count;
use esp_hal::Blocking;
use mvm_meshops_tests::asynct::{driver_tick, script_line, Platform};

esp_bootloader_esp_idf::esp_app_desc!();

/// One driver millisecond: 24,000 CCOUNT, a tenth of a real millisecond at
/// the nominal 240 MHz. The wait loop busy-waits (no interrupts here), and
/// under `-icount` QEMU each real millisecond costs about a million emulated
/// instructions, so a full-speed clock made the async tests take over ten
/// minutes. Every test measures time in driver ms, so they're unaffected;
/// guest code just looks 10x slower against this clock.
const TICK: u32 = 24_000;

/// Drivers ticked from the wait loop (no preemption on this target).
struct Plat {
    rx: UartRx<'static, Blocking>,
    next: u32,
    line: [u8; 40],
    n: usize,
}

impl Plat {
    fn uart(&mut self) {
        let mut b = [0u8; 16];
        while self.rx.read_ready() {
            let Ok(k) = self.rx.read_buffered(&mut b) else { break };
            if k == 0 {
                break;
            }
            for &c in &b[..k] {
                if c == b'\n' {
                    script_line(&self.line[..self.n]);
                    self.n = 0;
                } else if c != b'\r' && self.n < self.line.len() {
                    self.line[self.n] = c;
                    self.n += 1;
                }
            }
        }
    }
}

impl Platform for Plat {
    async fn wait(&mut self) {
        while (get_cycle_count().wrapping_sub(self.next) as i32) < 0 {}
        self.next = self.next.wrapping_add(TICK);
        self.uart();
        driver_tick();

    }
    fn preemptive(&self) -> bool {
        false
    }
}

#[esp_hal::main]
fn main() -> ! {
    let peripherals = esp_hal::init(esp_hal::Config::default());
    let (rx, mut uart) = Uart::new(peripherals.UART0, Config::default())
        .unwrap()
        .with_tx(peripherals.GPIO43)
        .with_rx(peripherals.GPIO44)
        .split();
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
    let mut plat = Plat {
        rx,
        next: get_cycle_count().wrapping_add(TICK),
        line: [0; 40],
        n: 0,
    };
    mvm_meshops_tests::asynct::block_on(mvm_meshops_tests::asynct::run_async(
        "esp32s3",
        &mut uart,
        &mut plat,
        get_cycle_count,
        get_cycle_count,
        mvm_meshops_tests::N_CALLS,
        true,
    ));
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
