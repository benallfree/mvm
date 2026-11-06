#![no_std]
//! Chip-independent benchmark body. A firmware supplies a cycle counter and a
//! `fmt::Write` sink; this runs the guest image (CRC16 + parse, RV32IMC) in
//! the `fast` interpreter and the same kernels natively, on the same 5 inputs
//! as the host tests, and prints one line per input:
//!
//! `MVM <chip> <case> interp=0x.. native=0x.. match=<0|1> interp_cycles=N native_cycles=N`
//!
//! then `MVM <chip> done pass=<0|1>`.

use core::fmt::Write;
use interp::{fast, Mem};
use kernels::inputs::{CRC_DATA, PACKET};

mod img {
    include!("../../../bench/guest-image/kernels.rs");
}

const GUEST_RAM: u32 = 0x8000_0000;

pub struct Case {
    pub name: &'static str,
    pub crc: bool,
    pub len: usize,
    pub bad_sum: bool,
}

pub const CASES: [Case; 5] = [
    Case { name: "crc16", crc: true, len: 256, bad_sum: false },
    Case { name: "crc16-check", crc: true, len: 9, bad_sum: false },
    Case { name: "parse", crc: false, len: 64, bad_sum: false },
    Case { name: "parse-bad-sum", crc: false, len: 64, bad_sum: true },
    Case { name: "parse-short", crc: false, len: 20, bad_sum: false },
];

/// The input bytes for a case (also used on the host for the expected values).
pub fn input(c: &Case, buf: &mut [u8; 256]) {
    if c.crc {
        if c.len == 9 {
            buf[..9].copy_from_slice(b"123456789");
        } else {
            buf.copy_from_slice(&CRC_DATA);
        }
    } else {
        buf[..64].copy_from_slice(&PACKET);
        if c.bad_sum {
            buf[63] ^= 1;
        }
    }
}

/// Run every case; returns true if all matched native.
pub fn run_all(chip: &str, cycles: impl Fn() -> u32, out: &mut impl Write) -> bool {
    let mut gram = [0u8; 4096];
    let mut table = [fast::E::EMPTY; img::IMAGE_LEN / 2];
    let mut all = true;
    for c in &CASES {
        let mut buf = [0u8; 256];
        input(c, &mut buf);
        gram[..256].copy_from_slice(&buf);
        let entry = if c.crc { img::CRC16 } else { img::PARSE };

        // Warm the decode table once, then time a warm run (as in round five).
        let mut cpu = fast::Cpu::new();
        let mut mem = Mem { image: &img::IMAGE, image_base: img::BASE, ram: &mut gram, ram_base: GUEST_RAM };
        let _ = cpu.call(&mut mem, &mut table, entry, &[GUEST_RAM, c.len as u32]);
        let t0 = cycles();
        let got = cpu.call(&mut mem, &mut table, entry, &[GUEST_RAM, c.len as u32]);
        let t1 = cycles();

        let n0 = cycles();
        let want = unsafe {
            if c.crc {
                kernels::crc16(core::hint::black_box(buf.as_ptr()), c.len)
            } else {
                kernels::parse(core::hint::black_box(buf.as_ptr()), c.len)
            }
        };
        let n1 = cycles();

        let got = got.unwrap_or(0xdead_0000);
        let ok = got == want;
        all &= ok;
        let _ = writeln!(
            out,
            "MVM {chip} {} interp={got:#x} native={want:#x} match={} interp_cycles={} native_cycles={}",
            c.name,
            ok as u8,
            t1.wrapping_sub(t0),
            n1.wrapping_sub(n0)
        );
    }
    let _ = writeln!(out, "MVM {chip} done pass={}", all as u8);
    all
}
