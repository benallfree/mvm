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

/// Round 7b: a position-independent link. Nothing is linked at an address.
///
/// - Code and read-only data (Text, ROData) go into one flash image laid out
///   from offset 0. References between them must be PC-relative (build with
///   `-C code-model=medium`), so the image runs unchanged at any address.
/// - Writable statics (Data, Bss) go into a separate static block that the
///   kernel places in RAM at load. Every code reference to a static is
///   rewritten to be relative to `gp`, which the kernel points at
///   `block + mvm_abi::GP_BIAS`:
///   - the `auipc`/`lui` of the pair becomes `addi rd, gp, 0` (`mv rd, gp`);
///   - each paired `addi`/load/store gets the static's offset from `gp`.
///
///   No instruction is added or removed, and the code bytes depend only on
///   the applet's own static layout, never on where it is loaded.
/// - Pointers stored *in* statics (`.data` relocations) are listed in
///   `data_relocs` for the kernel to apply in RAM at load. Pointers stored in
///   flash data (absolute words in rodata) can't be made position
///   independent this way; they are written as image offsets and listed in
///   `abs_flash` so callers can see (and reject) them.
pub struct PicImage {
    pub flash: Vec<u8>,
    pub flash_off: HashMap<usize, u32>,
    pub static_size: u32,
    pub static_align: u32,
    /// Initial contents of the static block (`.data` bytes, zeros for `.bss`).
    pub static_init: Vec<u8>,
    pub static_off: HashMap<usize, u32>,
    /// (offset in the static block, target is static?, target offset)
    pub data_relocs: Vec<(u32, bool, u32)>,
    /// Absolute pointers in flash data: (chunk origin, offset in chunk).
    pub abs_flash: Vec<(String, u32)>,
    /// Code accesses rewritten to be gp-relative.
    pub gp_rewrites: usize,
}

fn is_static(store: &Store, ci: usize) -> bool {
    matches!(store.chunks[ci].kind, crate::chunk::Kind::Data | crate::chunk::Kind::Bss)
}

pub fn link_pic(store: &Store, roots: &[usize]) -> Result<PicImage> {
    use mvm_abi::GP_BIAS;
    let order = store.closure_many(roots);
    let (mut flash_off, mut static_off) = (HashMap::new(), HashMap::new());
    let (mut fpc, mut spc, mut salign) = (0u64, 0u64, 4u64);
    for &ci in &order {
        let c = &store.chunks[ci];
        let a = c.align.max(1);
        if is_static(store, ci) {
            spc = spc.div_ceil(a) * a;
            static_off.insert(ci, spc as u32);
            spc += c.bytes.len() as u64;
            salign = salign.max(a);
        } else {
            fpc = fpc.div_ceil(a) * a;
            flash_off.insert(ci, fpc as u32);
            fpc += c.bytes.len() as u64;
        }
    }
    // Pad the image to whole words (flash images are fetched in halfwords
    // and placed at word-aligned addresses).
    fpc = fpc.div_ceil(4) * 4;
    if spc > 2 * GP_BIAS as u64 {
        return Err(format!(
            "statics are {spc} bytes; gp-relative 12-bit offsets reach {} (see MEMMODEL.md)",
            2 * GP_BIAS
        ));
    }
    let mut img = PicImage {
        flash: vec![0u8; fpc as usize],
        flash_off,
        static_size: spc as u32,
        static_align: salign as u32,
        static_init: vec![0u8; spc as usize],
        static_off,
        data_relocs: Vec::new(),
        abs_flash: Vec::new(),
        gp_rewrites: 0,
    };
    // Where a chunk is: (static?, offset in its area).
    let off_of = |img: &PicImage, j: usize| -> (bool, u32) {
        let j = store.canonical(j);
        if is_static(store, j) {
            (true, img.static_off[&j])
        } else {
            (false, img.flash_off[&j])
        }
    };
    let chunk_of = |t: &Target, origin: &str| -> Result<usize> {
        match t {
            Target::Chunk(j) => Ok(*j),
            other => Err(format!("{origin}: unresolved target {other:?}")),
        }
    };
    let rd32 = |b: &[u8], at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
    let wr32 = |b: &mut [u8], at: usize, v: u32| b[at..at + 4].copy_from_slice(&v.to_le_bytes());
    let gp_imm = |off: i64| -> Result<i32> {
        let v = off - GP_BIAS as i64;
        if (-2048..2048).contains(&v) {
            Ok(v as i32)
        } else {
            Err(format!("gp offset {v} out of 12-bit range"))
        }
    };

    for &ci in &order {
        let c = &store.chunks[ci];
        if is_static(store, ci) {
            // Static data: copy the initial bytes; pointers inside become load-time relocs.
            let o = img.static_off[&ci] as usize;
            img.static_init[o..o + c.bytes.len()].copy_from_slice(&c.bytes);
            for s in &c.slots {
                if s.kind != SlotKind::Abs32 {
                    return Err(format!("{}: {:?} in static data", c.origin, s.kind));
                }
                let (to_static, t) = off_of(&img, chunk_of(&s.target, &c.origin)?);
                img.data_relocs.push((o as u32 + s.at, to_static, (t as i64 + s.addend) as u32));
            }
            continue;
        }
        let a = img.flash_off[&ci];
        let o = a as usize;
        let mut buf = c.bytes.clone();
        for s in &c.slots {
            let at = s.at as usize;
            let p = (a + s.at) as i64;
            let target = |t: &Target, addend: i64| -> Result<(bool, i64)> {
                let (st, off) = off_of(&img, chunk_of(t, &c.origin)?);
                Ok((st, off as i64 + addend))
            };
            let or32 = |buf: &mut [u8], at: usize, v: u32| {
                let w = rd32(buf, at) | v;
                wr32(buf, at, w)
            };
            let hi = |v: i64| u_imm((v as i32).wrapping_add(0x800));
            // `auipc`/`lui rd` -> `addi rd, gp, 0`.
            let to_mv_gp = |buf: &mut [u8], at: usize| -> Result<()> {
                let w = rd32(buf, at);
                if !matches!(w & 0x7f, 0x17 | 0x37) {
                    return Err(format!("{}: expected auipc/lui at {at:#x}", c.origin));
                }
                wr32(buf, at, (w & (31 << 7)) | (3 << 15) | 0x13);
                Ok(())
            };
            match s.kind {
                SlotKind::PcrelHi20 | SlotKind::Hi20 => {
                    let (st, v) = target(&s.target, s.addend)?;
                    if st {
                        to_mv_gp(&mut buf, at)?;
                        img.gp_rewrites += 1;
                    } else if s.kind == SlotKind::Hi20 {
                        return Err(format!(
                            "{}: absolute address of flash data at {at:#x}; build with -C code-model=medium",
                            c.origin
                        ));
                    } else {
                        or32(&mut buf, at, hi(v - p));
                    }
                }
                SlotKind::Lo12I | SlotKind::Lo12S => {
                    let (st, v) = target(&s.target, s.addend)?;
                    if !st {
                        return Err(format!("{}: absolute flash address at {at:#x}", c.origin));
                    }
                    let i = gp_imm(v)?;
                    or32(&mut buf, at, if s.kind == SlotKind::Lo12I { i_imm(i) } else { s_imm(i) });
                }
                SlotKind::PcrelLo12I | SlotKind::PcrelLo12S => {
                    let Target::PcrelHi(h) = s.target else { unreachable!() };
                    let hs = c.slots.iter().find(|t| t.at == h && t.kind == SlotKind::PcrelHi20).unwrap();
                    let (st, v) = target(&hs.target, hs.addend)?;
                    let imm = if st { gp_imm(v)? } else { (v - (a + h) as i64) as i32 };
                    or32(&mut buf, at, if s.kind == SlotKind::PcrelLo12I { i_imm(imm) } else { s_imm(imm) });
                }
                SlotKind::Abs32 => {
                    let (st, v) = target(&s.target, s.addend)?;
                    if st {
                        return Err(format!("{}: pointer to RAM stored in flash at {at:#x}", c.origin));
                    }
                    or32(&mut buf, at, v as u32);
                    img.abs_flash.push((c.origin.clone(), s.at));
                }
                SlotKind::Call | SlotKind::Jal | SlotKind::Branch | SlotKind::RvcJump | SlotKind::RvcBranch => {
                    let (st, v) = target(&s.target, s.addend)?;
                    if st {
                        return Err(format!("{}: jump into static data at {at:#x}", c.origin));
                    }
                    let d = v - p;
                    match s.kind {
                        SlotKind::Call => {
                            or32(&mut buf, at, hi(d));
                            or32(&mut buf, at + 4, i_imm(d as i32));
                        }
                        SlotKind::Jal => or32(&mut buf, at, j_imm(d as i32)),
                        SlotKind::Branch => or32(&mut buf, at, b_imm(d as i32)),
                        SlotKind::RvcJump => {
                            let w = u16::from_le_bytes([buf[at], buf[at + 1]]) | cj_imm(d as i32);
                            buf[at..at + 2].copy_from_slice(&w.to_le_bytes());
                        }
                        _ => {
                            let w = u16::from_le_bytes([buf[at], buf[at + 1]]) | cb_imm(d as i32);
                            buf[at..at + 2].copy_from_slice(&w.to_le_bytes());
                        }
                    }
                }
            }
        }
        img.flash[o..o + buf.len()].copy_from_slice(&buf);
    }
    Ok(img)
}
