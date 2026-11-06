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
            others: &[],
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
            others: &[],
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

#[test]
fn fast_matches_native() {
    for (name, entry, data) in cases() {
        let mut ram = vec![0u8; 8192];
        ram[..data.len()].copy_from_slice(&data);
        let mut mem = Mem {
            image: &img::IMAGE,
            image_base: img::BASE,
            ram: &mut ram,
            ram_base: RAM_BASE,
            others: &[],
        };
        let mut table = vec![fast::E::EMPTY; img::IMAGE_LEN / 2];
        for pass in ["cold", "warm"] {
            let mut cpu = fast::Cpu::new();
            let got = cpu
                .call(&mut mem, &mut table, entry, &[RAM_BASE, data.len() as u32])
                .unwrap();
            assert_eq!(got, native(entry, &data), "{name} {pass}");
            #[cfg(feature = "count")]
            println!(
                "fast {pass} {name}: {got:#x}, {} guest instructions",
                cpu.steps
            );
        }
    }
}

#[test]
fn fast_rejects_bad_targets() {
    let mut ram = vec![0u8; 64];
    let mut mem = Mem {
        image: &img::IMAGE,
        image_base: img::BASE,
        ram: &mut ram,
        ram_base: RAM_BASE,
        others: &[],
    };
    let mut table = vec![fast::E::EMPTY; img::IMAGE_LEN / 2];
    let mut cpu = fast::Cpu::new();
    // Outside the image, odd, and a wrong-sized table are all faults, not UB.
    assert!(cpu.call(&mut mem, &mut table, 0x10, &[]).is_err());
    assert!(cpu.call(&mut mem, &mut table, img::CRC16 + 1, &[]).is_err());
    assert!(cpu
        .call(&mut mem, &mut table[..10], img::CRC16, &[])
        .is_err());
}

#[test]
fn fast_jumps_between_code_regions() {
    // Region A (0x1000): `jal x0, +0x1000` (to 0x2000, outside A), then a nop.
    let a: [u8; 8] = [0x6f, 0x10, 0x00, 0x00, 0x13, 0x00, 0x00, 0x00];
    // Region B (0x2000): `addi a0, a0, 5`; `ret`.
    let b: [u8; 8] = [0x13, 0x05, 0x55, 0x00, 0x67, 0x80, 0x00, 0x00];
    let (mut ta, mut tb) = ([fast::E::EMPTY; 4], [fast::E::EMPTY; 4]);
    let mut ram = vec![0u8; 256];
    let mut cpu = fast::Cpu::new();
    let mut mem = Mem {
        image: &a,
        image_base: 0x1000,
        ram: &mut ram,
        ram_base: RAM_BASE,
        others: &[],
    };
    cpu.start(&mem, ta.len(), 0x1000, &[37], RAM_BASE + 256, 0)
        .unwrap();
    assert_eq!(cpu.run(&mut mem, &mut ta), Ok(fast::Stop::Jump(0x2000)));
    let mut mem = Mem {
        image: &b,
        image_base: 0x2000,
        ram: &mut ram,
        ram_base: RAM_BASE,
        others: &[],
    };
    cpu.resume_at(&mem, tb.len(), 0x2000).unwrap();
    assert_eq!(cpu.run(&mut mem, &mut tb), Ok(fast::Stop::Returned(42)));
}

#[test]
fn loads_reach_other_regions() {
    let code: [u8; 8] = [0x13, 0, 0, 0, 0x13, 0, 0, 0];
    let other: [u8; 4] = [0x78, 0x56, 0x34, 0x12];
    let mut ram = vec![0u8; 16];
    let regions = [Region {
        base: 0x9000,
        bytes: &other,
    }];
    let mem = Mem {
        image: &code,
        image_base: 0x1000,
        ram: &mut ram,
        ram_base: RAM_BASE,
        others: &regions,
    };
    assert_eq!(mem.load(0x9000, 2), Ok(0x1234_5678));
    assert!(mem.load(0x9004, 2).is_err());
}

#[test]
fn fast_budget_stops_a_long_run() {
    // Round 7d: a small budget stops crc16 (a loop) with Stop::Budget; a big
    // one lets it finish with the right answer.
    let data = CRC_DATA.to_vec();
    for (budget, want_stop) in [(50u32, true), (u32::MAX, false)] {
        let mut ram = vec![0u8; 8192];
        ram[..data.len()].copy_from_slice(&data);
        let mut mem = Mem {
            image: &img::IMAGE,
            image_base: img::BASE,
            ram: &mut ram,
            ram_base: RAM_BASE,
            others: &[],
        };
        let mut table = vec![fast::E::EMPTY; img::IMAGE_LEN / 2];
        let mut cpu = fast::Cpu::new();
        cpu.start(&mem, table.len(), img::CRC16, &[RAM_BASE, data.len() as u32], RAM_BASE + 8192, 0)
            .unwrap();
        cpu.budget = budget;
        let r = cpu.run(&mut mem, &mut table).unwrap();
        if want_stop {
            assert_eq!(r, fast::Stop::Budget);
            assert_eq!(cpu.budget, 0);
        } else {
            assert_eq!(r, fast::Stop::Returned(native(img::CRC16, &data)));
        }
    }
}
