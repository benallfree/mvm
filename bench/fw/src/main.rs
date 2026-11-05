//! Benchmark firmware for the Unicorn harness (not a real nRF52840 image: no
//! vector table or startup code; the harness calls the `bench_*` functions).
#![no_std]
#![no_main]

use core::ptr::addr_of_mut;
use interp::{pd, simple, Mem};
use kernels::inputs::{CRC_DATA, PACKET};

mod img {
    include!("../../guest-image/kernels.rs");
}

/// Native inputs live in flash, like the guest image.
static CRC_IN: [u8; 256] = CRC_DATA;
static PKT_IN: [u8; 64] = PACKET;

/// Guest RAM window: inputs at the bottom, guest stack at the top.
const GUEST_RAM: u32 = 0x8000_0000;
const CRC_AT: u32 = GUEST_RAM;
const PKT_AT: u32 = GUEST_RAM + 256;
static mut GRAM: [u8; 4096] = [0; 4096];
static mut TABLE: [pd::D; img::IMAGE_LEN / 2] = [pd::D::EMPTY; img::IMAGE_LEN / 2];

fn mem() -> Mem<'static> {
    Mem {
        image: &img::IMAGE,
        image_base: img::BASE,
        ram: unsafe { &mut *addr_of_mut!(GRAM) },
        ram_base: GUEST_RAM,
    }
}

fn table() -> &'static mut [pd::D] {
    unsafe { &mut *addr_of_mut!(TABLE) }
}

/// Copy inputs into guest RAM and clear the pre-decode table. Not timed.
#[no_mangle]
pub extern "C" fn bench_setup() {
    let m = mem();
    m.ram[..256].copy_from_slice(&CRC_IN);
    m.ram[256..320].copy_from_slice(&PKT_IN);
    table().fill(pd::D::EMPTY);
}

#[no_mangle]
pub extern "C" fn bench_native_crc16() -> u32 {
    unsafe { kernels::crc16(CRC_IN.as_ptr(), CRC_IN.len()) }
}

#[no_mangle]
pub extern "C" fn bench_native_parse() -> u32 {
    unsafe { kernels::parse(PKT_IN.as_ptr(), PKT_IN.len()) }
}

#[no_mangle]
pub extern "C" fn bench_simple_crc16() -> u32 {
    simple::Cpu::new()
        .call(&mut mem(), img::CRC16, &[CRC_AT, 256])
        .unwrap_or(0xdead_0000)
}

#[no_mangle]
pub extern "C" fn bench_simple_parse() -> u32 {
    simple::Cpu::new()
        .call(&mut mem(), img::PARSE, &[PKT_AT, 64])
        .unwrap_or(0xdead_0000)
}

#[no_mangle]
pub extern "C" fn bench_pd_crc16() -> u32 {
    pd::Cpu::new()
        .call(&mut mem(), table(), img::CRC16, &[CRC_AT, 256])
        .unwrap_or(0xdead_0000)
}

#[no_mangle]
pub extern "C" fn bench_pd_parse() -> u32 {
    pd::Cpu::new()
        .call(&mut mem(), table(), img::PARSE, &[PKT_AT, 64])
        .unwrap_or(0xdead_0000)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
