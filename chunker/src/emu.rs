//! A minimal RV32IMC interpreter: just enough to run a linked image and
//! return from one call.

use crate::rv::*;

pub struct Cpu {
    pub x: [u32; 32],
    pub pc: u32,
    pub mem: Vec<u8>,
    pub steps: u64,
}

/// Returning to this address ends the run.
const EXIT: u32 = 0;

impl Cpu {
    pub fn new(mem_size: usize) -> Self {
        Cpu {
            x: [0; 32],
            pc: 0,
            mem: vec![0; mem_size],
            steps: 0,
        }
    }

    fn chk(&self, a: u32, n: u32) -> Result<usize, String> {
        let a = a as usize;
        if a < 0x100 || a + n as usize > self.mem.len() {
            return Err(format!("bad access {a:#x} at pc {:#x}", self.pc));
        }
        Ok(a)
    }
    fn ld(&self, a: u32, n: u32) -> Result<u32, String> {
        let a = self.chk(a, n)?;
        let mut v = 0u32;
        for i in 0..n as usize {
            v |= (self.mem[a + i] as u32) << (8 * i);
        }
        Ok(v)
    }
    fn st(&mut self, a: u32, n: u32, v: u32) -> Result<(), String> {
        let a = self.chk(a, n)?;
        for i in 0..n as usize {
            self.mem[a + i] = (v >> (8 * i)) as u8;
        }
        Ok(())
    }

    /// Call `pc` with `args`, run until it returns, give back a0.
    pub fn call(&mut self, pc: u32, args: &[u32], max_steps: u64) -> Result<u32, String> {
        self.x = [0; 32];
        self.x[1] = EXIT;
        self.x[2] = (self.mem.len() as u32) & !15;
        for (i, a) in args.iter().enumerate() {
            self.x[10 + i] = *a;
        }
        self.pc = pc;
        while self.pc != EXIT {
            if self.steps >= max_steps {
                return Err("step limit".into());
            }
            self.step()?;
            self.steps += 1;
        }
        Ok(self.x[10])
    }

    fn step(&mut self) -> Result<(), String> {
        let lo = self.ld(self.pc, 2)? as u16;
        let (ins, len) = if lo & 3 != 3 {
            (
                expand(lo)
                    .ok_or_else(|| format!("illegal compressed {lo:#06x} at {:#x}", self.pc))?,
                2,
            )
        } else {
            (self.ld(self.pc, 4)?, 4)
        };
        self.exec(ins, len)
    }

    fn exec(&mut self, ins: u32, len: u32) -> Result<(), String> {
        let op = ins & 0x7f;
        let rd = ((ins >> 7) & 31) as usize;
        let f3 = (ins >> 12) & 7;
        let r1 = self.x[((ins >> 15) & 31) as usize];
        let r2 = self.x[((ins >> 20) & 31) as usize];
        let f7 = ins >> 25;
        let imm_i = (ins as i32) >> 20;
        let imm_s = (((ins as i32) >> 25) << 5) | ((ins >> 7) & 31) as i32;
        let imm_b = sext(
            ((ins >> 19) & 0x1000)
                | ((ins << 4) & 0x800)
                | ((ins >> 20) & 0x7e0)
                | ((ins >> 7) & 0x1e),
            13,
        );
        let imm_j = sext(
            ((ins >> 11) & 0x10_0000)
                | (ins & 0xf_f000)
                | ((ins >> 9) & 0x800)
                | ((ins >> 20) & 0x7fe),
            21,
        );
        let pc = self.pc;
        let mut next = pc.wrapping_add(len);
        let ill = || Err(format!("illegal instruction {ins:#010x} at {pc:#x}"));
        let w = |cpu: &mut Cpu, v: u32| {
            if rd != 0 {
                cpu.x[rd] = v
            }
        };
        match op {
            0x37 => w(self, ins & 0xffff_f000),
            0x17 => w(self, pc.wrapping_add(ins & 0xffff_f000)),
            0x6f => {
                w(self, next);
                next = pc.wrapping_add(imm_j as u32);
            }
            0x67 => {
                let t = r1.wrapping_add(imm_i as u32) & !1;
                w(self, next);
                next = t;
            }
            0x63 => {
                let take = match f3 {
                    0 => r1 == r2,
                    1 => r1 != r2,
                    4 => (r1 as i32) < (r2 as i32),
                    5 => (r1 as i32) >= (r2 as i32),
                    6 => r1 < r2,
                    7 => r1 >= r2,
                    _ => return ill(),
                };
                if take {
                    next = pc.wrapping_add(imm_b as u32);
                }
            }
            0x03 => {
                let a = r1.wrapping_add(imm_i as u32);
                let v = match f3 {
                    0 => self.ld(a, 1)? as i8 as i32 as u32,
                    1 => self.ld(a, 2)? as i16 as i32 as u32,
                    2 => self.ld(a, 4)?,
                    4 => self.ld(a, 1)?,
                    5 => self.ld(a, 2)?,
                    _ => return ill(),
                };
                w(self, v);
            }
            0x23 => {
                let a = r1.wrapping_add(imm_s as u32);
                match f3 {
                    0 => self.st(a, 1, r2)?,
                    1 => self.st(a, 2, r2)?,
                    2 => self.st(a, 4, r2)?,
                    _ => return ill(),
                }
            }
            0x13 => {
                let sh = (imm_i as u32) & 31;
                let v = match f3 {
                    0 => r1.wrapping_add(imm_i as u32),
                    1 => r1 << sh,
                    2 => ((r1 as i32) < imm_i) as u32,
                    3 => (r1 < imm_i as u32) as u32,
                    4 => r1 ^ imm_i as u32,
                    5 if f7 == 0x20 => ((r1 as i32) >> sh) as u32,
                    5 => r1 >> sh,
                    6 => r1 | imm_i as u32,
                    _ => r1 & imm_i as u32,
                };
                w(self, v);
            }
            0x33 if f7 == 1 => {
                let v = match f3 {
                    0 => r1.wrapping_mul(r2),
                    1 => ((r1 as i32 as i64 * r2 as i32 as i64) >> 32) as u32,
                    2 => ((r1 as i32 as i64 * r2 as u64 as i64) >> 32) as u32,
                    3 => ((r1 as u64 * r2 as u64) >> 32) as u32,
                    4 => match (r1 as i32, r2 as i32) {
                        (_, 0) => u32::MAX,
                        (a, b) => a.wrapping_div(b) as u32,
                    },
                    5 => r1.checked_div(r2).unwrap_or(u32::MAX),
                    6 => match (r1 as i32, r2 as i32) {
                        (a, 0) => a as u32,
                        (a, b) => a.wrapping_rem(b) as u32,
                    },
                    _ => r1.checked_rem(r2).unwrap_or(r1),
                };
                w(self, v);
            }
            0x33 => {
                let sh = r2 & 31;
                let v = match (f3, f7) {
                    (0, 0) => r1.wrapping_add(r2),
                    (0, 0x20) => r1.wrapping_sub(r2),
                    (1, 0) => r1 << sh,
                    (2, 0) => ((r1 as i32) < (r2 as i32)) as u32,
                    (3, 0) => (r1 < r2) as u32,
                    (4, 0) => r1 ^ r2,
                    (5, 0) => r1 >> sh,
                    (5, 0x20) => ((r1 as i32) >> sh) as u32,
                    (6, 0) => r1 | r2,
                    (7, 0) => r1 & r2,
                    _ => return ill(),
                };
                w(self, v);
            }
            0x0f => {}
            _ => return ill(),
        }
        self.pc = next;
        Ok(())
    }
}

/// Expand an RV32C instruction into its 32-bit equivalent.
pub fn expand(c: u16) -> Option<u32> {
    let i = c as u32;
    let f3 = (i >> 13) & 7;
    let rd = (i >> 7) & 31;
    let rs2 = (i >> 2) & 31;
    let rdp = 8 + ((i >> 2) & 7); // rd'/rs2' in bits 4:2
    let rs1p = 8 + ((i >> 7) & 7); // rs1'/rd' in bits 9:7
    let imm6 = sext(((i >> 7) & 0x20) | ((i >> 2) & 0x1f), 6);
    let cj = sext(
        ((i >> 1) & 0x800)
            | ((i >> 7) & 0x10)
            | ((i >> 1) & 0x300)
            | ((i << 2) & 0x400)
            | ((i >> 1) & 0x40)
            | ((i << 1) & 0x80)
            | ((i >> 2) & 0xe)
            | ((i << 3) & 0x20),
        12,
    );
    let cb = sext(
        ((i >> 4) & 0x100)
            | ((i >> 7) & 0x18)
            | ((i << 1) & 0xc0)
            | ((i >> 2) & 6)
            | ((i << 3) & 0x20),
        9,
    );
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
        (1, 1) => enc_j(1, cj),
        (1, 2) => enc_i(0x13, rd, 0, 0, imm6),
        (1, 3) if rd == 2 => {
            let imm = ((i >> 3) & 0x200)
                | ((i >> 2) & 0x10)
                | ((i << 1) & 0x40)
                | ((i << 4) & 0x180)
                | ((i << 3) & 0x20);
            enc_i(0x13, 2, 0, 2, sext(imm, 10))
        }
        (1, 3) => enc_u(0x37, rd, imm6 << 12),
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
        (1, 5) => enc_j(0, cj),
        (1, 6) => enc_b(0, rs1p, 0, cb),
        (1, 7) => enc_b(1, rs1p, 0, cb),
        (2, 0) => enc_i(0x13, rd, 1, rd, imm6 & 31),
        (2, 2) => {
            let imm = ((i >> 7) & 0x20) | ((i >> 2) & 0x1c) | ((i << 4) & 0xc0);
            enc_i(0x03, rd, 2, 2, imm as i32)
        }
        (2, 4) => match (i & 0x1000 != 0, rd, rs2) {
            (false, _, 0) => enc_i(0x67, 0, 0, rd, 0),
            (false, _, _) => enc_r(0x33, rd, 0, 0, rs2, 0),
            (true, 0, 0) => return None, // ebreak
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
