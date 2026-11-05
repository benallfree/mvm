//! Minimal RV32IMC interpreters for the speed spike.
//!
//! - [`simple`]: a fetch / decode / match loop that decodes every instruction
//!   on every pass. No caching.
//! - [`pd`]: the one cheap optimisation. Each instruction is decoded once into
//!   a small table (lazily, on first execution) and later passes dispatch
//!   straight from the table.
//!
//! `// @phase: <name>` comments mark source regions. The Unicorn harness maps
//! each host instruction to a source line via DWARF line tables, then to the
//! phase whose marker is the closest one above that line.
#![no_std]

pub mod pd;
pub mod simple;

/// A call returns when the guest jumps here (it is the initial `ra`).
pub const EXIT: u32 = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    Fetch(u32),
    Load(u32),
    Store(u32),
    Illegal(u32),
}

/// Guest memory: a read-only image (code + rodata) and a read-write RAM window.
pub struct Mem<'a> {
    pub image: &'a [u8],
    pub image_base: u32,
    pub ram: &'a mut [u8],
    pub ram_base: u32,
}

impl Mem<'_> {
    // @phase: fetch
    #[inline(always)]
    pub fn fetch16(&self, pc: u32) -> Result<u32, Fault> {
        let off = pc.wrapping_sub(self.image_base) as usize;
        match self.image.get(off..off + 2) {
            Some(b) => Ok(u16::from_le_bytes([b[0], b[1]]) as u32),
            None => Err(Fault::Fetch(pc)),
        }
    }

    // @phase: execute
    #[inline(always)]
    fn bytes(&self, a: u32, n: usize) -> Option<&[u8]> {
        let r = a.wrapping_sub(self.ram_base) as usize;
        if r.wrapping_add(n) <= self.ram.len() {
            return self.ram.get(r..r + n);
        }
        let i = a.wrapping_sub(self.image_base) as usize;
        self.image.get(i..i + n)
    }

    #[inline(always)]
    pub fn load(&self, a: u32, f3: u32) -> Result<u32, Fault> {
        let n = 1usize << (f3 & 3);
        let b = self.bytes(a, n).ok_or(Fault::Load(a))?;
        Ok(match f3 {
            0 => b[0] as i8 as i32 as u32,
            1 => u16::from_le_bytes([b[0], b[1]]) as i16 as i32 as u32,
            2 => u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            4 => b[0] as u32,
            5 => u16::from_le_bytes([b[0], b[1]]) as u32,
            _ => return Err(Fault::Load(a)),
        })
    }

    #[inline(always)]
    pub fn store(&mut self, a: u32, f3: u32, v: u32) -> Result<(), Fault> {
        let n = 1usize << (f3 & 3);
        let r = a.wrapping_sub(self.ram_base) as usize;
        let b = self
            .ram
            .get_mut(r..r.wrapping_add(n))
            .ok_or(Fault::Store(a))?;
        b.copy_from_slice(&v.to_le_bytes()[..n]);
        Ok(())
    }
}

// @phase: decode
#[inline(always)]
pub fn sext(v: u32, bits: u32) -> i32 {
    ((v << (32 - bits)) as i32) >> (32 - bits)
}
#[inline(always)]
pub fn imm_i(ins: u32) -> i32 {
    (ins as i32) >> 20
}
#[inline(always)]
pub fn imm_s(ins: u32) -> i32 {
    (((ins as i32) >> 25) << 5) | ((ins >> 7) & 31) as i32
}
#[inline(always)]
pub fn imm_b(ins: u32) -> i32 {
    sext(
        ((ins >> 19) & 0x1000) | ((ins << 4) & 0x800) | ((ins >> 20) & 0x7e0) | ((ins >> 7) & 0x1e),
        13,
    )
}
#[inline(always)]
pub fn imm_j(ins: u32) -> i32 {
    sext(
        ((ins >> 11) & 0x10_0000) | (ins & 0xf_f000) | ((ins >> 9) & 0x800) | ((ins >> 20) & 0x7fe),
        21,
    )
}

fn enc_i(op: u32, rd: u32, f3: u32, rs1: u32, imm: i32) -> u32 {
    ((imm as u32) & 0xfff) << 20 | rs1 << 15 | f3 << 12 | rd << 7 | op
}
fn enc_s(op: u32, f3: u32, rs1: u32, rs2: u32, imm: i32) -> u32 {
    let v = imm as u32;
    ((v >> 5) & 0x7f) << 25 | rs2 << 20 | rs1 << 15 | f3 << 12 | (v & 0x1f) << 7 | op
}
fn enc_r(op: u32, rd: u32, f3: u32, rs1: u32, rs2: u32, f7: u32) -> u32 {
    f7 << 25 | rs2 << 20 | rs1 << 15 | f3 << 12 | rd << 7 | op
}
fn enc_b(f3: u32, rs1: u32, rs2: u32, imm: i32) -> u32 {
    let v = imm as u32;
    ((v >> 12) & 1) << 31
        | ((v >> 5) & 0x3f) << 25
        | rs2 << 20
        | rs1 << 15
        | f3 << 12
        | ((v >> 1) & 0xf) << 8
        | ((v >> 11) & 1) << 7
        | 0x63
}
fn enc_j(rd: u32, imm: i32) -> u32 {
    let v = imm as u32;
    ((v >> 20) & 1) << 31
        | ((v >> 1) & 0x3ff) << 21
        | ((v >> 11) & 1) << 20
        | ((v >> 12) & 0xff) << 12
        | rd << 7
        | 0x6f
}

/// Expand an RV32C instruction into its 32-bit equivalent.
///
/// `inline(always)`: left out of line, the call and the `Option` returned through
/// the stack made the simple interpreter 1.2-1.3x slower (measured: crc16 192x vs
/// 148x native, parse 111x vs 90x).
#[inline(always)]
pub fn expand(c: u32) -> Option<u32> {
    let i = c;
    let f3 = (i >> 13) & 7;
    let rd = (i >> 7) & 31;
    let rs2 = (i >> 2) & 31;
    let rdp = 8 + ((i >> 2) & 7);
    let rs1p = 8 + ((i >> 7) & 7);
    let imm6 = sext(((i >> 7) & 0x20) | ((i >> 2) & 0x1f), 6);
    let cj = || {
        sext(
            ((i >> 1) & 0x800)
                | ((i >> 7) & 0x10)
                | ((i >> 1) & 0x300)
                | ((i << 2) & 0x400)
                | ((i >> 1) & 0x40)
                | ((i << 1) & 0x80)
                | ((i >> 2) & 0xe)
                | ((i << 3) & 0x20),
            12,
        )
    };
    let cb = || {
        sext(
            ((i >> 4) & 0x100)
                | ((i >> 7) & 0x18)
                | ((i << 1) & 0xc0)
                | ((i >> 2) & 6)
                | ((i << 3) & 0x20),
            9,
        )
    };
    let lw_imm = (((i >> 7) & 0x38) | ((i >> 4) & 4) | ((i << 1) & 0x40)) as i32;
    Some(match (i & 3, f3) {
        (0, 0) => {
            let imm = ((i >> 7) & 0x30) | ((i >> 1) & 0x3c0) | ((i >> 4) & 4) | ((i >> 2) & 8);
            if imm == 0 {
                return None;
            }
            enc_i(0x13, rdp, 0, 2, imm as i32)
        }
        (0, 2) => enc_i(0x03, rdp, 2, rs1p, lw_imm),
        (0, 6) => enc_s(0x23, 2, rs1p, rdp, lw_imm),
        (1, 0) => enc_i(0x13, rd, 0, rd, imm6),
        (1, 1) => enc_j(1, cj()),
        (1, 2) => enc_i(0x13, rd, 0, 0, imm6),
        (1, 3) if rd == 2 => {
            let imm = ((i >> 3) & 0x200)
                | ((i >> 2) & 0x10)
                | ((i << 1) & 0x40)
                | ((i << 4) & 0x180)
                | ((i << 3) & 0x20);
            enc_i(0x13, 2, 0, 2, sext(imm, 10))
        }
        (1, 3) => ((imm6 << 12) as u32 & 0xffff_f000) | rd << 7 | 0x37,
        (1, 4) => match (i >> 10) & 3 {
            0 => enc_i(0x13, rs1p, 5, rs1p, imm6 & 31),
            1 => enc_i(0x13, rs1p, 5, rs1p, (imm6 & 31) | 0x400),
            2 => enc_i(0x13, rs1p, 7, rs1p, imm6),
            _ => {
                if i & 0x1000 != 0 {
                    return None;
                }
                let (f3, f7) = match (i >> 5) & 3 {
                    0 => (0, 0x20),
                    1 => (4, 0),
                    2 => (6, 0),
                    _ => (7, 0),
                };
                enc_r(0x33, rs1p, f3, rs1p, rdp, f7)
            }
        },
        (1, 5) => enc_j(0, cj()),
        (1, 6) => enc_b(0, rs1p, 0, cb()),
        (1, 7) => enc_b(1, rs1p, 0, cb()),
        (2, 0) => enc_i(0x13, rd, 1, rd, imm6 & 31),
        (2, 2) => {
            let imm = ((i >> 7) & 0x20) | ((i >> 2) & 0x1c) | ((i << 4) & 0xc0);
            enc_i(0x03, rd, 2, 2, imm as i32)
        }
        (2, 4) => match (i & 0x1000 != 0, rd, rs2) {
            (false, _, 0) => enc_i(0x67, 0, 0, rd, 0),
            (false, _, _) => enc_r(0x33, rd, 0, 0, rs2, 0),
            (true, 0, 0) => return None,
            (true, _, 0) => enc_i(0x67, 1, 0, rd, 0),
            (true, _, _) => enc_r(0x33, rd, 0, rd, rs2, 0),
        },
        (2, 6) => {
            let imm = ((i >> 7) & 0x3c) | ((i >> 1) & 0xc0);
            enc_s(0x23, 2, 2, rs2, imm as i32)
        }
        _ => return None,
    })
}

// @phase: execute
/// M-extension and base ALU ops shared by both interpreters.
#[inline(always)]
pub fn alu(f3: u32, alt: bool, m: bool, a: u32, b: u32) -> u32 {
    if m {
        return match f3 {
            0 => a.wrapping_mul(b),
            1 => ((a as i32 as i64 * b as i32 as i64) >> 32) as u32,
            2 => ((a as i32 as i64 * b as u64 as i64) >> 32) as u32,
            3 => ((a as u64 * b as u64) >> 32) as u32,
            4 => match b as i32 {
                0 => u32::MAX,
                d => (a as i32).wrapping_div(d) as u32,
            },
            5 => a.checked_div(b).unwrap_or(u32::MAX),
            6 => match b as i32 {
                0 => a,
                d => (a as i32).wrapping_rem(d) as u32,
            },
            _ => a.checked_rem(b).unwrap_or(a),
        };
    }
    let sh = b & 31;
    match f3 {
        0 if alt => a.wrapping_sub(b),
        0 => a.wrapping_add(b),
        1 => a << sh,
        2 => ((a as i32) < (b as i32)) as u32,
        3 => (a < b) as u32,
        4 => a ^ b,
        5 if alt => ((a as i32) >> sh) as u32,
        5 => a >> sh,
        6 => a | b,
        _ => a & b,
    }
}

#[cfg(test)]
mod tests;
