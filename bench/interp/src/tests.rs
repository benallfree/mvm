//! Both interpreters run the chunked kernels image and must match the native
//! (host) build of the same kernels. With `--features count` the guest
//! instruction counts are printed for the report.

extern crate std;
use crate::*;
use kernels::inputs::{CRC_DATA, PACKET};
use std::{println, vec, vec::Vec};

mod img {
    include!("../../guest-image/kernels.rs");
}

const RAM_BASE: u32 = 0x8000_0000;

fn cases() -> Vec<(&'static str, u32, Vec<u8>)> {
    let mut bad_sum = PACKET.to_vec();
    bad_sum[63] ^= 1;
    vec![
        ("crc16", img::CRC16, CRC_DATA.to_vec()),
        ("crc16 check", img::CRC16, b"123456789".to_vec()),
        ("parse", img::PARSE, PACKET.to_vec()),
        ("parse bad sum", img::PARSE, bad_sum),
        ("parse short", img::PARSE, PACKET[..20].to_vec()),
    ]
}

fn native(entry: u32, data: &[u8]) -> u32 {
    unsafe {
        if entry == img::CRC16 {
            kernels::crc16(data.as_ptr(), data.len())
        } else {
            kernels::parse(data.as_ptr(), data.len())
        }
    }
}

#[test]
fn simple_matches_native() {
    for (name, entry, data) in cases() {
        let mut ram = vec![0u8; 8192];
        ram[..data.len()].copy_from_slice(&data);
        let mut mem = Mem {
            image: &img::IMAGE,
            image_base: img::BASE,
            ram: &mut ram,
            ram_base: RAM_BASE,
        };
        let mut cpu = simple::Cpu::new();
        let got = cpu
            .call(&mut mem, entry, &[RAM_BASE, data.len() as u32])
            .unwrap();
        assert_eq!(got, native(entry, &data), "{name}");
        #[cfg(feature = "count")]
        println!("simple {name}: {got:#x}, {} guest instructions", cpu.steps);
        #[cfg(not(feature = "count"))]
        println!("simple {name}: {got:#x}");
    }
}

#[test]
fn predecoded_matches_native() {
    for (name, entry, data) in cases() {
        let mut ram = vec![0u8; 8192];
        ram[..data.len()].copy_from_slice(&data);
        let mut mem = Mem {
            image: &img::IMAGE,
            image_base: img::BASE,
            ram: &mut ram,
            ram_base: RAM_BASE,
        };
        let mut table = vec![pd::D::EMPTY; img::IMAGE_LEN / 2];
        // Twice: cold (decodes as it goes) and warm (table filled).
        for pass in ["cold", "warm"] {
            let mut cpu = pd::Cpu::new();
            let got = cpu
                .call(&mut mem, &mut table, entry, &[RAM_BASE, data.len() as u32])
                .unwrap();
            assert_eq!(got, native(entry, &data), "{name} {pass}");
            #[cfg(feature = "count")]
            println!(
                "pd {pass} {name}: {got:#x}, {} guest instructions",
                cpu.steps
            );
        }
        let used = table.iter().filter(|d| d.op != pd::Op::Undecoded).count();
        println!("pd {name}: {used} of {} table entries filled", table.len());
    }
    assert_eq!(native(img::CRC16, b"123456789"), 0x29B1);
}
