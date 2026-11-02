//! The nRF52840 kernel image. A stub until 0.4.

#![no_std]
#![no_main]

use panic_halt as _;

#[cortex_m_rt::entry]
fn main() -> ! {
    loop {}
}
