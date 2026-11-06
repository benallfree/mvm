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

/// Where a chunk lives in a position-independent link.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Seg {
    /// The flash image (code + rodata).
    Flash,
    /// The gp window: small statics, reached `gp`-relative.
    Gp,
    /// The bulk segment: large statics, reached through a bulk slot.
    Bulk,
}

impl Seg {
    pub fn name(self) -> &'static str {
        match self {
            Seg::Flash => "flash",
            Seg::Gp => "gp",
            Seg::Bulk => "bulk",
        }
    }
}

/// A RAM segment's layout: size, alignment and its initial bytes (a prefix:
/// `.data` is laid out before `.bss`, so the zeros after it aren't stored).
#[derive(Default)]
pub struct SegImage {
    pub size: u32,
    pub align: u32,
    pub init: Vec<u8>,
}

/// One static's placement, for the report.
pub struct Placed {
    pub origin: String,
    pub size: u32,
    pub seg: Seg,
    pub off: u32,
    /// Why it went to bulk ("threshold" or "spill"); empty for gp.
    pub why: &'static str,
}

/// Rounds 7b and 7c: a position-independent link. Nothing is linked at an
/// address.
///
/// - Code and read-only data (Text, ROData) go into one flash image laid out
///   from offset 0. References between them must be PC-relative (build with
///   `-C code-model=medium`), so the image runs unchanged at any address.
/// - Writable statics (Data, Bss) go into one of two RAM segments the kernel
///   places at load (round 7c):
///   - the **gp window** (at most 4 KB): `gp` = window + `GP_BIAS`. The
///     `auipc` of an access becomes `addi rd, gp, 0`; the paired
///     instruction gets the offset from `gp`.
///   - the **bulk segment** (any size): the `auipc` becomes
///     `lw rd, slot_k(gp)`, a load of bulk slot `k` at the start of the gp
///     window (the kernel writes `bulk + 4096k + 2048` there), and the paired
///     instruction gets the offset from that address.
///
///   No instruction is added or removed, and the code bytes depend only on
///   the applet's own layout, never on where the segments are placed.
/// - Placement is automatic: statics of `threshold` bytes or more go to
///   bulk; if the rest (plus the slots) still exceed the window, the largest
///   remaining move to bulk until they fit.
/// - Pointers stored *in* statics are listed in `data_relocs` for the kernel
///   to apply in RAM at load. Pointers stored in flash data can't be made
///   position independent this way; they are written as image offsets and
///   listed in `abs_flash`.
pub struct PicImage {
    pub flash: Vec<u8>,
    pub off: HashMap<usize, (Seg, u32)>,
    pub gp: SegImage,
    pub bulk: SegImage,
    /// Bulk slots at the start of the gp window.
    pub bulk_slots: u32,
    /// (segment holding the pointer, offset in it, target segment, target offset)
    pub data_relocs: Vec<(Seg, u32, Seg, u32)>,
    /// Absolute pointers in flash data: (chunk origin, offset in chunk).
    pub abs_flash: Vec<(String, u32)>,
    /// Code accesses rewritten to be gp-relative / through a bulk slot.
    pub gp_rewrites: usize,
    pub bulk_rewrites: usize,
    pub placed: Vec<Placed>,
    /// Read-only chunks moved to RAM because they hold pointers to RAM.
    pub promoted: Vec<String>,
}

/// The chunks that live in RAM: writable statics (Data, Bss), plus read-only
/// data holding a pointer to something in RAM (round 7c). A read-only static
/// whose initialiser takes a static's address, or a `static mut` LLVM found
/// is never written, lands in `.rodata` with an absolute pointer to RAM; that
/// address is only known at load, so the chunk moves to a RAM segment and the
/// pointer becomes a load-time reloc. Repeated until nothing more moves (a
/// pointer to a promoted chunk is a pointer to RAM too).
fn ram_chunks(store: &Store, order: &[usize]) -> std::collections::HashSet<usize> {
    use crate::chunk::Kind;
    let mut ram: std::collections::HashSet<usize> = order
        .iter()
        .copied()
        .filter(|&ci| matches!(store.chunks[ci].kind, Kind::Data | Kind::Bss))
        .collect();
    loop {
        let more: Vec<usize> = order
            .iter()
            .copied()
            .filter(|ci| !ram.contains(ci) && store.chunks[*ci].kind == Kind::ROData)
            .filter(|&ci| {
                store.chunks[ci].slots.iter().any(|s| {
                    s.kind == SlotKind::Abs32
                        && matches!(s.target, Target::Chunk(j) if ram.contains(&store.canonical(j)))
                })
            })
            .collect();
        if more.is_empty() {
            return ram;
        }
        ram.extend(more);
    }
}

/// Lay out `chunks` from `start`: `.data` first, then `.bss`. Returns the end
/// of the last `.data` byte (the initialised prefix) and the total size.
fn lay_out(store: &Store, chunks: &[usize], start: u64, out: &mut HashMap<usize, (Seg, u32)>, seg: Seg) -> (u64, u64, u64) {
    let (mut pc, mut init_end, mut align) = (start, start, 4u64);
    for bss in [false, true] {
        for &ci in chunks {
            let c = &store.chunks[ci];
            if (c.kind == crate::chunk::Kind::Bss) != bss {
                continue;
            }
            let a = c.align.max(1);
            pc = pc.div_ceil(a) * a;
            out.insert(ci, (seg, pc as u32));
            pc += c.bytes.len() as u64;
            align = align.max(a);
            if !bss {
                init_end = pc;
            }
        }
    }
    (init_end, pc, align)
}

/// Upper bound on the gp window size for `gp` statics with `slots` bulk slots
/// (worst-case alignment padding included), used by the placement loop.
fn gp_need(store: &Store, gp: &[usize], slots: u32) -> u64 {
    // (Read-only chunks promoted to RAM are laid out as initialised data.)
    let mut m = HashMap::new();
    lay_out(store, gp, 4 * slots as u64, &mut m, Seg::Gp).1
}

pub fn link_pic(store: &Store, roots: &[usize], threshold: u32) -> Result<PicImage> {
    use mvm_abi::{bulk_slots, BULK_BIAS, BULK_PAGE, GP_BIAS, GP_WINDOW};
    let order = store.closure_many(roots);
    let ram = ram_chunks(store, &order);
    let is_static = |_: &Store, ci: usize| ram.contains(&ci);
    let mut off: HashMap<usize, (Seg, u32)> = HashMap::new();
    let mut fpc = 0u64;
    let (mut gp, mut bulk) = (Vec::new(), Vec::new());
    let mut why: HashMap<usize, &'static str> = HashMap::new();
    for &ci in &order {
        let c = &store.chunks[ci];
        if is_static(store, ci) {
            if c.bytes.len() as u64 >= threshold as u64 {
                bulk.push(ci);
                why.insert(ci, "threshold");
            } else {
                gp.push(ci);
            }
        } else {
            let a = c.align.max(1);
            fpc = fpc.div_ceil(a) * a;
            off.insert(ci, (Seg::Flash, fpc as u32));
            fpc += c.bytes.len() as u64;
        }
    }
    // Spill: move the largest gp statics to bulk until the window fits.
    let bulk_size_of = |b: &[usize]| {
        let mut m = HashMap::new();
        lay_out(store, b, 0, &mut m, Seg::Bulk).1 as u32
    };
    loop {
        let slots = bulk_slots(bulk_size_of(&bulk));
        if gp_need(store, &gp, slots) <= GP_WINDOW as u64 {
            break;
        }
        let (i, _) = gp
            .iter()
            .enumerate()
            .max_by_key(|&(i, &ci)| (store.chunks[ci].bytes.len(), core::cmp::Reverse(i)))
            .ok_or("the bulk slots alone overflow the gp window")?;
        let ci = gp.remove(i);
        why.insert(ci, "spill");
        bulk.push(ci);
    }
    // Keep bulk in link order (placement doesn't depend on spill order).
    bulk.sort_by_key(|ci| order.iter().position(|o| o == ci));
    let (b_init, b_size, b_align) = lay_out(store, &bulk, 0, &mut off, Seg::Bulk);
    let slots = bulk_slots(b_size as u32);
    let (g_init, g_size, g_align) = lay_out(store, &gp, 4 * slots as u64, &mut off, Seg::Gp);
    assert!(g_size <= GP_WINDOW as u64);
    // Pad the image to whole words (flash images are fetched in halfwords
    // and placed at word-aligned addresses).
    fpc = fpc.div_ceil(4) * 4;
    let mut img = PicImage {
        flash: vec![0u8; fpc as usize],
        off,
        gp: SegImage { size: g_size as u32, align: g_align as u32, init: vec![0u8; g_init as usize] },
        bulk: SegImage { size: b_size as u32, align: b_align as u32, init: vec![0u8; b_init as usize] },
        bulk_slots: slots,
        data_relocs: Vec::new(),
        abs_flash: Vec::new(),
        gp_rewrites: 0,
        bulk_rewrites: 0,
        placed: Vec::new(),
        promoted: order
            .iter()
            .filter(|&&ci| ram.contains(&ci) && store.chunks[ci].kind == crate::chunk::Kind::ROData)
            .map(|&ci| store.chunks[ci].origin.clone())
            .collect(),
    };
    for &ci in gp.iter().chain(&bulk) {
        let (seg, o) = img.off[&ci];
        img.placed.push(Placed {
            origin: store.chunks[ci].origin.clone(),
            size: store.chunks[ci].bytes.len() as u32,
            seg,
            off: o,
            why: why.get(&ci).copied().unwrap_or(""),
        });
    }
    let off_of = |img: &PicImage, j: usize| -> (Seg, u32) { img.off[&store.canonical(j)] };
    let chunk_of = |t: &Target, origin: &str| -> Result<usize> {
        match t {
            Target::Chunk(j) => Ok(*j),
            other => Err(format!("{origin}: unresolved target {other:?}")),
        }
    };
    let rd32 = |b: &[u8], at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
    let wr32 = |b: &mut [u8], at: usize, v: u32| b[at..at + 4].copy_from_slice(&v.to_le_bytes());
    let imm12 = |v: i64, what: &str| -> Result<i32> {
        if (-2048..2048).contains(&v) {
            Ok(v as i32)
        } else {
            Err(format!("{what} offset {v} out of 12-bit range"))
        }
    };
    // The 12-bit immediate of the instruction paired with an access to (seg, v).
    let lo_imm = |seg: Seg, v: i64| -> Result<i32> {
        match seg {
            Seg::Gp => imm12(v - GP_BIAS as i64, "gp"),
            Seg::Bulk => {
                let k = page(v, slots)?;
                imm12(v - (BULK_PAGE * k + BULK_BIAS) as i64, "bulk")
            }
            Seg::Flash => unreachable!(),
        }
    };

    for &ci in &order {
        let c = &store.chunks[ci];
        if is_static(store, ci) {
            // Static data: copy the initial bytes; pointers inside become load-time relocs.
            let (seg, o) = img.off[&ci];
            let init = if seg == Seg::Gp { &mut img.gp.init } else { &mut img.bulk.init };
            if c.kind != crate::chunk::Kind::Bss {
                init[o as usize..o as usize + c.bytes.len()].copy_from_slice(&c.bytes);
            }
            for s in &c.slots {
                if s.kind != SlotKind::Abs32 {
                    return Err(format!("{}: {:?} in static data", c.origin, s.kind));
                }
                let (ts, t) = off_of(&img, chunk_of(&s.target, &c.origin)?);
                img.data_relocs.push((seg, o + s.at, ts, (t as i64 + s.addend) as u32));
            }
            continue;
        }
        let (_, a) = img.off[&ci];
        let o = a as usize;
        let mut buf = c.bytes.clone();
        for s in &c.slots {
            let at = s.at as usize;
            let p = (a + s.at) as i64;
            let target = |t: &Target, addend: i64| -> Result<(Seg, i64)> {
                let (seg, off) = off_of(&img, chunk_of(t, &c.origin)?);
                Ok((seg, off as i64 + addend))
            };
            let or32 = |buf: &mut [u8], at: usize, v: u32| {
                let w = rd32(buf, at) | v;
                wr32(buf, at, w)
            };
            let hi = |v: i64| u_imm((v as i32).wrapping_add(0x800));
            // `auipc`/`lui rd` -> `addi rd, gp, 0` (gp window) or
            // `lw rd, slot(gp)` (bulk). Same size; rd is kept.
            let rewrite_hi = |buf: &mut [u8], at: usize, seg: Seg, v: i64| -> Result<()> {
                let w = rd32(buf, at);
                if !matches!(w & 0x7f, 0x17 | 0x37) {
                    return Err(format!("{}: expected auipc/lui at {at:#x}", c.origin));
                }
                let rd = w & (31 << 7);
                let new = match seg {
                    Seg::Gp => rd | (3 << 15) | 0x13,
                    _ => {
                        let slot = 4 * page(v, slots)? as i32 - GP_BIAS as i32;
                        i_imm(slot) | (3 << 15) | (2 << 12) | rd | 0x03
                    }
                };
                wr32(buf, at, new);
                Ok(())
            };
            match s.kind {
                SlotKind::PcrelHi20 | SlotKind::Hi20 => {
                    let (seg, v) = target(&s.target, s.addend)?;
                    match seg {
                        Seg::Flash if s.kind == SlotKind::Hi20 => {
                            return Err(format!(
                                "{}: absolute address of flash data at {at:#x}; build with -C code-model=medium",
                                c.origin
                            ))
                        }
                        Seg::Flash => or32(&mut buf, at, hi(v - p)),
                        Seg::Gp => {
                            rewrite_hi(&mut buf, at, seg, v)?;
                            img.gp_rewrites += 1;
                        }
                        Seg::Bulk => {
                            rewrite_hi(&mut buf, at, seg, v)?;
                            img.bulk_rewrites += 1;
                        }
                    }
                }
                SlotKind::Lo12I | SlotKind::Lo12S => {
                    let (seg, v) = target(&s.target, s.addend)?;
                    if seg == Seg::Flash {
                        return Err(format!("{}: absolute flash address at {at:#x}", c.origin));
                    }
                    let i = lo_imm(seg, v)?;
                    or32(&mut buf, at, if s.kind == SlotKind::Lo12I { i_imm(i) } else { s_imm(i) });
                }
                SlotKind::PcrelLo12I | SlotKind::PcrelLo12S => {
                    let Target::PcrelHi(h) = s.target else { unreachable!() };
                    let hs = c.slots.iter().find(|t| t.at == h && t.kind == SlotKind::PcrelHi20).unwrap();
                    let (seg, v) = target(&hs.target, hs.addend)?;
                    let imm = if seg == Seg::Flash { (v - (a + h) as i64) as i32 } else { lo_imm(seg, v)? };
                    or32(&mut buf, at, if s.kind == SlotKind::PcrelLo12I { i_imm(imm) } else { s_imm(imm) });
                }
                SlotKind::Abs32 => {
                    let (seg, v) = target(&s.target, s.addend)?;
                    if seg != Seg::Flash {
                        return Err(format!("{}: pointer to RAM stored in flash at {at:#x}", c.origin));
                    }
                    // A code address in flash data would be jumped to as an
                    // image offset (a jump table, a vtable). Only export
                    // tables may hold one: the build tool reads them, the
                    // guest never does.
                    let to_code = matches!(s.target, Target::Chunk(j)
                        if store.chunks[store.canonical(j)].kind == crate::chunk::Kind::Text);
                    if to_code && !c.origin.ends_with("_EXPORTS") {
                        return Err(format!(
                            "{}: code address stored in flash data at {at:#x} (a jump table or function-pointer table?) can't be position independent",
                            c.origin
                        ));
                    }
                    or32(&mut buf, at, v as u32);
                    img.abs_flash.push((c.origin.clone(), s.at));
                }
                SlotKind::Call | SlotKind::Jal | SlotKind::Branch | SlotKind::RvcJump | SlotKind::RvcBranch => {
                    let (seg, v) = target(&s.target, s.addend)?;
                    if seg != Seg::Flash {
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

/// The bulk slot covering bulk offset `v`.
fn page(v: i64, slots: u32) -> Result<u32> {
    let k = v.div_euclid(mvm_abi::BULK_PAGE as i64);
    if (0..slots as i64).contains(&k) {
        Ok(k as u32)
    } else {
        Err(format!("bulk offset {v} outside the bulk segment"))
    }
}
