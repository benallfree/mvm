//! nRF52840 firmware (Embassy): runs the `fast` interpreter on the chunked
//! guest image and the same kernels natively, timed with the DWT cycle
//! counter, then the mesh-op tests (rounds 7-7d). Output on UARTE0 (TX
//! P0.06, RX P0.08, as on the DK).
//!
//! Round 7d priorities:
//! - the interpreter (and the kernel's executor loop) runs in `main`, on the
//!   thread-mode executor: the lowest priority, preemptible by everything;
//! - the fake drivers run on an `InterruptExecutor` driven by SWI0 at P6:
//!   a 1 ms ticker (embassy-time, RTC1 at P5), the UART script reader and
//!   the button on P0.03 (its level, sampled every tick). They only push events
//!   into the inbox and signal `WAKE`; they never call guest code.
#![no_std]
#![no_main]

use core::fmt::Write;
use cortex_m::peripheral::DWT;
use embassy_executor::{InterruptExecutor, Spawner};
use embassy_nrf::gpio::{Input, Pull};
use embassy_nrf::interrupt;
use embassy_nrf::interrupt::{InterruptExt, Priority};
use embassy_nrf::{bind_interrupts, peripherals, uarte};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Ticker};
use mvm_meshops_tests::asynct::{driver_input, driver_tick, Inject, Platform};
use panic_halt as _;

bind_interrupts!(struct Irqs {
    UARTE0 => uarte::InterruptHandler<peripherals::UARTE0>;
});

/// The drivers' executor (SWI0 / EGU0 interrupt).
static DRIVERS: InterruptExecutor = InterruptExecutor::new();
/// Raised by the drivers after every tick: the interpreter task wakes.
static WAKE: Signal<CriticalSectionRawMutex, ()> = Signal::new();

#[interrupt]
unsafe fn EGU0_SWI0() {
    unsafe { DRIVERS.on_interrupt() }
}

struct Out<'a>(uarte::UarteTx<'a>);

impl Write for Out<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        // blocking_write copies flash-resident data to RAM for EasyDMA.
        self.0
            .blocking_write(s.as_bytes())
            .map_err(|_| core::fmt::Error)
    }
}

/// GPIO port 0's IN register (P0.03 is bit 3).
const P0_IN: *const u32 = 0x5000_0510 as *const u32;

/// The fake drivers' clock: one tick per millisecond. Each tick also
/// samples the button (P0.03) and reports rising edges. (Renode's GPIOTE
/// latches the edge in EVENTS_IN but never raised its interrupt here, and a
/// tick-time read of the event didn't see it either; the pin level itself
/// is reliable, so the driver samples it at 1 kHz.)
#[embassy_executor::task]
async fn ticker(_button: Input<'static>) {
    let mut t = Ticker::every(Duration::from_millis(1));
    let mut was_high = false;
    loop {
        t.next().await;
        // SAFETY: reading the port's input register has no side effects.
        let high = unsafe { core::ptr::read_volatile(P0_IN) } & (1 << 3) != 0;
        if high && !was_high {
            driver_input(Inject::Edge { pin: 3, edge: 1 });
        }
        was_high = high;
        driver_tick();
        WAKE.signal(());
    }
}

/// Inputs typed by the Renode script: "P<text>" a packet, "E<pin>" an edge.
#[embassy_executor::task]
async fn script(mut rx: uarte::UarteRx<'static>) {
    let mut line = [0u8; 40];
    let mut n = 0;
    let mut b = [0u8; 1];
    loop {
        if rx.read(&mut b).await.is_err() {
            continue;
        }
        if b[0] != b'\n' {
            if n < line.len() {
                line[n] = b[0];
                n += 1;
            }
            continue;
        }
        mvm_meshops_tests::asynct::script_line(&line[..n]);
        n = 0;
    }
}

/// The interpreter task sleeps on WAKE (WFE in the thread executor) until
/// the drivers have ticked.
struct Plat;
impl Platform for Plat {
    async fn wait(&mut self) {
        WAKE.wait().await;
    }
}

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let mut config = embassy_nrf::config::Config::default();
    config.time_interrupt_priority = Priority::P5;
    config.gpiote_interrupt_priority = Priority::P5;
    let p = embassy_nrf::init(config);
    let mut ucfg = uarte::Config::default();
    ucfg.baudrate = uarte::Baudrate::Baud115200;
    let (tx, rx) = uarte::Uarte::new(p.UARTE0, p.P0_08, p.P0_06, Irqs, ucfg).split();
    let mut out = Out(tx);

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
    // Round 7d: start the drivers' executor above thread mode, then the async tests.
    interrupt::EGU0_SWI0.set_priority(Priority::P6);
    let drivers = DRIVERS.start(interrupt::EGU0_SWI0);
    let button = Input::new(p.P0_03, Pull::Down);
    drivers.spawn(ticker(button).unwrap());
    drivers.spawn(script(rx).unwrap());
    let ok = mvm_meshops_tests::asynct::run_async(
        "nrf52840",
        &mut out,
        &mut Plat,
        DWT::cycle_count,
        DWT::cycle_count,
        mvm_meshops_tests::N_CALLS,
        true,
    )
    .await;
    let _ = ok;
    loop {
        cortex_m::asm::wfi();
    }
}
