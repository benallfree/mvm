//! nRF52840 firmware (Embassy): runs the `fast` interpreter on the chunked
//! guest image and the same kernels natively, timed with the DWT cycle
//! counter, and prints the results on UARTE0 (TX on P0.06, as on the DK).
#![no_std]
#![no_main]

use core::fmt::Write;
use cortex_m::peripheral::DWT;
use embassy_executor::Spawner;
use embassy_nrf::{bind_interrupts, peripherals, uarte};
use panic_halt as _;

bind_interrupts!(struct Irqs {
    UARTE0 => uarte::InterruptHandler<peripherals::UARTE0>;
});

struct Out<'a>(uarte::UarteTx<'a>);

impl Write for Out<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        // blocking_write copies flash-resident data to RAM for EasyDMA.
        self.0
            .blocking_write(s.as_bytes())
            .map_err(|_| core::fmt::Error)
    }
}

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let p = embassy_nrf::init(Default::default());
    let mut config = uarte::Config::default();
    config.baudrate = uarte::Baudrate::Baud115200;
    let mut out = Out(uarte::UarteTx::new(p.UARTE0, Irqs, p.P0_06, config));

    let mut cp = cortex_m::Peripherals::take().unwrap();
    cp.DCB.enable_trace();
    cp.DWT.enable_cycle_counter();

    let _ = writeln!(out, "MVM nrf52840 start");
    bench_core::run_all("nrf52840", DWT::cycle_count, &mut out);
    // Round seven: mesh ops. kernel.timer.now.v1 and section timing both read DWT.
    mvm_meshops_tests::run_all(
        "nrf52840",
        &mut out,
        DWT::cycle_count,
        DWT::cycle_count,
        mvm_meshops_tests::N_CALLS,
    );
    loop {
        cortex_m::asm::wfi();
    }
}
