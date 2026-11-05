//! Rejoin chunks into a flat image: lay out the closure of an entry chunk,
//! then fill every slot from the final addresses.

use crate::chunk::{Result, SlotKind, Store, Target};
use crate::rv::*;
use std::collections::HashMap;

pub struct Image {
    pub bytes: Vec<u8>,
    pub addr: HashMap<usize, u32>,
}

fn fits(v: i64, bits: u32) -> bool {
    let lim = 1i64 << (bits - 1);
    (-lim..lim).contains(&v)
}

pub fn link(store: &Store, entry: usize, base: u32) -> Result<Image> {
    link_many(store, &[entry], base)
}

pub fn link_many(store: &Store, roots: &[usize], base: u32) -> Result<Image> {
    let order = store.closure_many(roots);
    let mut addr = HashMap::new();
    let mut pc = base as u64;
    for &ci in &order {
        let c = &store.chunks[ci];
        let a = c.align.max(1);
        pc = pc.div_ceil(a) * a;
        addr.insert(ci, pc as u32);
        pc += c.bytes.len() as u64;
    }
    let mut bytes = vec![0u8; (pc - base as u64) as usize];
    for &ci in &order {
        let c = &store.chunks[ci];
        let a = addr[&ci];
        let off = (a - base) as usize;
        let buf = &mut bytes[off..off + c.bytes.len()];
        buf.copy_from_slice(&c.bytes);
        let target_of = |t: &Target, addend: i64| -> Result<i64> {
            match t {
                Target::Chunk(j) => Ok(addr[&store.canonical(*j)] as i64 + addend),
                other => Err(format!("{}: unexpected target {other:?}", c.origin)),
            }
        };
        for s in &c.slots {
            let at = s.at as usize;
            let p = (a + s.at) as i64;
            let or32 = |buf: &mut [u8], at: usize, v: u32| {
                let w = u32::from_le_bytes(buf[at..at + 4].try_into().unwrap()) | v;
                buf[at..at + 4].copy_from_slice(&w.to_le_bytes());
            };
            let or16 = |buf: &mut [u8], at: usize, v: u16| {
                let w = u16::from_le_bytes(buf[at..at + 2].try_into().unwrap()) | v;
                buf[at..at + 2].copy_from_slice(&w.to_le_bytes());
            };
            let hi = |v: i64| u_imm((v as i32).wrapping_add(0x800));
            let range = |ok: bool| {
                if ok {
                    Ok(())
                } else {
                    Err(format!(
                        "{}: {:?} at {:#x} out of range",
                        c.origin, s.kind, s.at
                    ))
                }
            };
            match s.kind {
                SlotKind::Abs32 => or32(buf, at, target_of(&s.target, s.addend)? as u32),
                SlotKind::Hi20 => or32(buf, at, hi(target_of(&s.target, s.addend)?)),
                SlotKind::Lo12I => or32(buf, at, i_imm(target_of(&s.target, s.addend)? as i32)),
                SlotKind::Lo12S => or32(buf, at, s_imm(target_of(&s.target, s.addend)? as i32)),
                SlotKind::PcrelHi20 => or32(buf, at, hi(target_of(&s.target, s.addend)? - p)),
                SlotKind::PcrelLo12I | SlotKind::PcrelLo12S => {
                    let Target::PcrelHi(h) = s.target else {
                        unreachable!()
                    };
                    let hs = c
                        .slots
                        .iter()
                        .find(|t| t.at == h && t.kind == SlotKind::PcrelHi20)
                        .unwrap();
                    let v = (target_of(&hs.target, hs.addend)? - (a + h) as i64) as i32;
                    or32(
                        buf,
                        at,
                        if s.kind == SlotKind::PcrelLo12I {
                            i_imm(v)
                        } else {
                            s_imm(v)
                        },
                    );
                }
                SlotKind::Call => {
                    let v = target_of(&s.target, s.addend)? - p;
                    range(fits(v, 32))?;
                    or32(buf, at, hi(v));
                    or32(buf, at + 4, i_imm(v as i32));
                }
                SlotKind::Jal => {
                    let v = target_of(&s.target, s.addend)? - p;
                    range(fits(v, 21))?;
                    or32(buf, at, j_imm(v as i32));
                }
                SlotKind::Branch => {
                    let v = target_of(&s.target, s.addend)? - p;
                    range(fits(v, 13))?;
                    or32(buf, at, b_imm(v as i32));
                }
                SlotKind::RvcJump => {
                    let v = target_of(&s.target, s.addend)? - p;
                    range(fits(v, 12))?;
                    or16(buf, at, cj_imm(v as i32));
                }
                SlotKind::RvcBranch => {
                    let v = target_of(&s.target, s.addend)? - p;
                    range(fits(v, 9))?;
                    or16(buf, at, cb_imm(v as i32));
                }
            }
        }
    }
    Ok(Image { bytes, addr })
}
