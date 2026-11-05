//! The simple interpreter: decode every instruction on every pass.

use crate::*;

pub struct Cpu {
    pub x: [u32; 32],
    pub pc: u32,
    #[cfg(feature = "count")]
    pub steps: u64,
}

impl Default for Cpu {
    fn default() -> Self {
        Self::new()
    }
}

impl Cpu {
    pub const fn new() -> Self {
        Cpu {
            x: [0; 32],
            pc: 0,
            #[cfg(feature = "count")]
            steps: 0,
        }
    }

    /// Call the guest function at `entry` with up to 8 arguments; return a0.
    #[inline(never)]
    pub fn call(&mut self, mem: &mut Mem, entry: u32, args: &[u32]) -> Result<u32, Fault> {
        self.x = [0; 32];
        self.x[1] = EXIT;
        self.x[2] = (mem.ram_base.wrapping_add(mem.ram.len() as u32)) & !15;
        for (i, a) in args.iter().take(8).enumerate() {
            self.x[10 + i] = *a;
        }
        self.pc = entry;
        loop {
            // @phase: loop
            if self.pc == EXIT {
                return Ok(self.x[10]);
            }
            #[cfg(feature = "count")]
            {
                self.steps += 1;
            }
            // @phase: fetch
            let pc = self.pc;
            let lo = mem.fetch16(pc)?;
            let (ins, len) = if lo & 3 != 3 {
                // @phase: decode
                (expand(lo).ok_or(Fault::Illegal(pc))?, 2)
            } else {
                // @phase: fetch
                (lo | mem.fetch16(pc.wrapping_add(2))? << 16, 4)
            };
            // @phase: decode
            let op = ins & 0x7f;
            let rd = ((ins >> 7) & 31) as usize;
            let f3 = (ins >> 12) & 7;
            let a = self.x[((ins >> 15) & 31) as usize];
            let b = self.x[((ins >> 20) & 31) as usize];
            let mut next = pc.wrapping_add(len);
            // @phase: dispatch
            match op {
                // @phase: execute
                0x37 => self.x[rd] = ins & 0xffff_f000,
                0x17 => self.x[rd] = pc.wrapping_add(ins & 0xffff_f000),
                0x6f => {
                    self.x[rd] = next;
                    next = pc.wrapping_add(imm_j(ins) as u32);
                }
                0x67 => {
                    let t = a.wrapping_add(imm_i(ins) as u32) & !1;
                    self.x[rd] = next;
                    next = t;
                }
                0x63 => {
                    let take = match f3 {
                        0 => a == b,
                        1 => a != b,
                        4 => (a as i32) < (b as i32),
                        5 => (a as i32) >= (b as i32),
                        6 => a < b,
                        7 => a >= b,
                        _ => return Err(Fault::Illegal(pc)),
                    };
                    if take {
                        next = pc.wrapping_add(imm_b(ins) as u32);
                    }
                }
                0x03 => self.x[rd] = mem.load(a.wrapping_add(imm_i(ins) as u32), f3)?,
                0x23 => mem.store(a.wrapping_add(imm_s(ins) as u32), f3, b)?,
                0x13 => {
                    let imm = imm_i(ins) as u32;
                    // Only shifts use the alt bit; for them imm[10] carries it.
                    let alt = f3 == 5 && imm & 0x400 != 0;
                    self.x[rd] = alu(f3, alt, false, a, imm);
                }
                0x33 => self.x[rd] = alu(f3, ins & 0x4000_0000 != 0, ins & 0x0200_0000 != 0, a, b),
                0x0f => {}
                _ => return Err(Fault::Illegal(pc)),
            }
            // @phase: loop
            self.x[0] = 0;
            self.pc = next;
        }
    }
}
