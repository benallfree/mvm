//! The cheap optimisation: a lazily filled pre-decode table.
//!
//! Each 2-byte slot of the image has one [`D`] entry (12 bytes). The first
//! time an instruction runs it is fetched, expanded and decoded into its
//! entry; later passes dispatch straight from the entry. Extra tricks that
//! fall out of decoding once:
//! - branch and jal targets are stored as absolute addresses;
//! - an ALU op whose rd is x0 becomes `Nop`; jal/jalr links and loads into x0
//!   go to a scratch register `x[32]`, so execution never has to re-zero x0.
//!   The register file has 64 slots so `rd & 63` needs no bounds check.

use crate::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Op {
    Undecoded,
    Illegal,
    Nop,
    Lui,
    Auipc,
    Jal,
    Jalr,
    Beq,
    Bne,
    Blt,
    Bge,
    Bltu,
    Bgeu,
    Lb,
    Lh,
    Lw,
    Lbu,
    Lhu,
    Sb,
    Sh,
    Sw,
    Addi,
    Slti,
    Sltiu,
    Xori,
    Ori,
    Andi,
    Slli,
    Srli,
    Srai,
    Add,
    Sub,
    Sll,
    Slt,
    Sltu,
    Xor,
    Srl,
    Sra,
    Or,
    And,
    Mul,
    Mulh,
    Mulhsu,
    Mulhu,
    Div,
    Divu,
    Rem,
    Remu,
}

#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct D {
    pub op: Op,
    pub rd: u8,
    pub rs1: u8,
    pub rs2: u8,
    /// Instruction length in bytes (2 or 4).
    pub len: u8,
    /// Immediate; for branches and jal, the absolute target.
    pub imm: i32,
}

impl D {
    pub const EMPTY: D = D {
        op: Op::Undecoded,
        rd: 0,
        rs1: 0,
        rs2: 0,
        len: 0,
        imm: 0,
    };
}

// @phase: decode
fn decode(ins: u32, len: u32, pc: u32) -> D {
    let rd = ((ins >> 7) & 31) as u8;
    let rs1 = ((ins >> 15) & 31) as u8;
    let rs2 = ((ins >> 20) & 31) as u8;
    let f3 = (ins >> 12) & 7;
    let f7 = ins >> 25;
    let mut d = D {
        op: Op::Illegal,
        rd,
        rs1,
        rs2,
        len: len as u8,
        imm: 0,
    };
    use Op::*;
    match ins & 0x7f {
        0x37 => (d.op, d.imm) = (Lui, (ins & 0xffff_f000) as i32),
        0x17 => (d.op, d.imm) = (Auipc, pc.wrapping_add(ins & 0xffff_f000) as i32),
        0x6f => (d.op, d.imm) = (Jal, pc.wrapping_add(imm_j(ins) as u32) as i32),
        0x67 => (d.op, d.imm) = (Jalr, imm_i(ins)),
        0x63 => {
            d.imm = pc.wrapping_add(imm_b(ins) as u32) as i32;
            d.op = match f3 {
                0 => Beq,
                1 => Bne,
                4 => Blt,
                5 => Bge,
                6 => Bltu,
                7 => Bgeu,
                _ => Illegal,
            };
        }
        0x03 => {
            d.imm = imm_i(ins);
            d.op = match f3 {
                0 => Lb,
                1 => Lh,
                2 => Lw,
                4 => Lbu,
                5 => Lhu,
                _ => Illegal,
            };
        }
        0x23 => {
            d.imm = imm_s(ins);
            d.op = match f3 {
                0 => Sb,
                1 => Sh,
                2 => Sw,
                _ => Illegal,
            };
        }
        0x13 => {
            d.imm = imm_i(ins);
            d.op = match f3 {
                0 => Addi,
                1 => Slli,
                2 => Slti,
                3 => Sltiu,
                4 => Xori,
                5 if d.imm & 0x400 != 0 => Srai,
                5 => Srli,
                6 => Ori,
                _ => Andi,
            };
            if matches!(d.op, Slli | Srli | Srai) {
                d.imm &= 31;
            }
        }
        0x33 => {
            d.op = match (f7, f3) {
                (0, 0) => Add,
                (0x20, 0) => Sub,
                (0, 1) => Sll,
                (0, 2) => Slt,
                (0, 3) => Sltu,
                (0, 4) => Xor,
                (0, 5) => Srl,
                (0x20, 5) => Sra,
                (0, 6) => Or,
                (0, 7) => And,
                (1, 0) => Mul,
                (1, 1) => Mulh,
                (1, 2) => Mulhsu,
                (1, 3) => Mulhu,
                (1, 4) => Div,
                (1, 5) => Divu,
                (1, 6) => Rem,
                (1, 7) => Remu,
                _ => Illegal,
            };
        }
        0x0f => d.op = Nop,
        _ => {}
    }
    // Writes to x0: ALU/load results are discarded; links go to scratch x[32].
    if rd == 0 {
        match d.op {
            Jal | Jalr | Lb | Lh | Lw | Lbu | Lhu => d.rd = 32,
            Lui | Auipc | Addi | Slti | Sltiu | Xori | Ori | Andi | Slli | Srli | Srai | Add
            | Sub | Sll | Slt | Sltu | Xor | Srl | Sra | Or | And | Mul | Mulh | Mulhsu | Mulhu
            | Div | Divu | Rem | Remu => d.op = Nop,
            _ => {}
        }
    }
    d
}

pub struct Cpu {
    /// x0..x31, then x[32] as a sink for writes to x0 (64 slots: `& 63` indexing).
    pub x: [u32; 64],
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
            x: [0; 64],
            pc: 0,
            #[cfg(feature = "count")]
            steps: 0,
        }
    }

    /// Call the guest function at `entry`. `table` has one entry per 2 bytes of image.
    #[inline(never)]
    pub fn call(
        &mut self,
        mem: &mut Mem,
        table: &mut [D],
        entry: u32,
        args: &[u32],
    ) -> Result<u32, Fault> {
        self.x = [0; 64];
        self.x[1] = EXIT;
        self.x[2] = (mem.ram_base.wrapping_add(mem.ram.len() as u32)) & !15;
        for (i, a) in args.iter().take(8).enumerate() {
            self.x[10 + i] = *a;
        }
        let mut pc = entry;
        loop {
            // @phase: loop
            if pc == EXIT {
                self.pc = pc;
                return Ok(self.x[10]);
            }
            #[cfg(feature = "count")]
            {
                self.steps += 1;
            }
            // @phase: fetch
            let slot = (pc.wrapping_sub(mem.image_base) >> 1) as usize;
            let mut d = *table.get(slot).ok_or(Fault::Fetch(pc))?;
            if d.op == Op::Undecoded {
                // @phase: decode
                let lo = mem.fetch16(pc)?;
                d = if lo & 3 != 3 {
                    decode(expand(lo).ok_or(Fault::Illegal(pc))?, 2, pc)
                } else {
                    decode(lo | mem.fetch16(pc.wrapping_add(2))? << 16, 4, pc)
                };
                table[slot] = d;
            }
            // @phase: decode
            let a = self.x[d.rs1 as usize & 31];
            let b = self.x[d.rs2 as usize & 31];
            let rd = d.rd as usize;
            let imm = d.imm as u32;
            let mut next = pc.wrapping_add(d.len as u32);
            use Op::*;
            // @phase: dispatch
            match d.op {
                // @phase: execute
                Nop => {}
                Lui | Auipc => self.x[rd & 63] = imm,
                Jal => {
                    self.x[rd & 63] = next;
                    next = imm;
                }
                Jalr => {
                    let t = a.wrapping_add(imm) & !1;
                    self.x[rd & 63] = next;
                    next = t;
                }
                Beq => next = if a == b { imm } else { next },
                Bne => next = if a != b { imm } else { next },
                Blt => next = if (a as i32) < (b as i32) { imm } else { next },
                Bge => next = if (a as i32) >= (b as i32) { imm } else { next },
                Bltu => next = if a < b { imm } else { next },
                Bgeu => next = if a >= b { imm } else { next },
                Lb => self.x[rd & 63] = mem.load(a.wrapping_add(imm), 0)?,
                Lh => self.x[rd & 63] = mem.load(a.wrapping_add(imm), 1)?,
                Lw => self.x[rd & 63] = mem.load(a.wrapping_add(imm), 2)?,
                Lbu => self.x[rd & 63] = mem.load(a.wrapping_add(imm), 4)?,
                Lhu => self.x[rd & 63] = mem.load(a.wrapping_add(imm), 5)?,
                Sb => mem.store(a.wrapping_add(imm), 0, b)?,
                Sh => mem.store(a.wrapping_add(imm), 1, b)?,
                Sw => mem.store(a.wrapping_add(imm), 2, b)?,
                Addi => self.x[rd & 63] = a.wrapping_add(imm),
                Slti => self.x[rd & 63] = ((a as i32) < (imm as i32)) as u32,
                Sltiu => self.x[rd & 63] = (a < imm) as u32,
                Xori => self.x[rd & 63] = a ^ imm,
                Ori => self.x[rd & 63] = a | imm,
                Andi => self.x[rd & 63] = a & imm,
                Slli => self.x[rd & 63] = a << imm,
                Srli => self.x[rd & 63] = a >> imm,
                Srai => self.x[rd & 63] = ((a as i32) >> imm) as u32,
                Add => self.x[rd & 63] = a.wrapping_add(b),
                Sub => self.x[rd & 63] = a.wrapping_sub(b),
                Sll => self.x[rd & 63] = a << (b & 31),
                Slt => self.x[rd & 63] = ((a as i32) < (b as i32)) as u32,
                Sltu => self.x[rd & 63] = (a < b) as u32,
                Xor => self.x[rd & 63] = a ^ b,
                Srl => self.x[rd & 63] = a >> (b & 31),
                Sra => self.x[rd & 63] = ((a as i32) >> (b & 31)) as u32,
                Or => self.x[rd & 63] = a | b,
                And => self.x[rd & 63] = a & b,
                Mul => self.x[rd & 63] = alu(0, false, true, a, b),
                Mulh => self.x[rd & 63] = alu(1, false, true, a, b),
                Mulhsu => self.x[rd & 63] = alu(2, false, true, a, b),
                Mulhu => self.x[rd & 63] = alu(3, false, true, a, b),
                Div => self.x[rd & 63] = alu(4, false, true, a, b),
                Divu => self.x[rd & 63] = alu(5, false, true, a, b),
                Rem => self.x[rd & 63] = alu(6, false, true, a, b),
                Remu => self.x[rd & 63] = alu(7, false, true, a, b),
                Undecoded | Illegal => return Err(Fault::Illegal(pc)),
            }
            // @phase: loop
            pc = next;
        }
    }
}
