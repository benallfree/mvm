//! The optimised interpreter (round five). It builds on [`crate::pd`] and changes
//! what round three measured as expensive:
//!
//! - **pc is a table index, not a guest address.** Branch and jal targets are
//!   stored as indices, so fetch is a single indexed load: no subtract, no shift,
//!   no bounds check. The decoder proves every successor index is in range, so
//!   the hot path can use `get_unchecked`; only `jalr` targets are checked at run time.
//! - **The entry is read in place**, with no 12-byte copy to the stack.
//! - **"Not decoded yet" is its own op.** The table starts full of `Undecoded`,
//!   whose arm decodes and loops back, so the warm path never tests for it.
//! - **No exit test per instruction.** Only `jalr` can leave, so it checks for the exit address.
//! - **Operands are read inside the arms that need them**, not up front for every op.
//! - **Specialised forms:** `li`, `mv`, branch-against-zero (`beqz`/`bnez`), and `lui`/`auipc` fold to `li`.
//! - Writes to x0 are removed at decode time, as in `pd`.
//! - Round 7b: code may sit in several regions (scattered flash). A jump out
//!   of the current region stops with [`Stop::Jump`]; in-region code is
//!   unchanged (decode still proves in-region targets). A `jal` to another
//!   region decodes as `jalr` from x0 with the absolute target, so it needs no
//!   extra arm (an extra arm changed LLVM's block layout and cost 10%).
//!   `start` takes `gp` and no longer clears the register file.
//! - Round seven: `ecall` stops the loop ([`Stop::Ecall`]) so the host can
//!   service a mesh op and resume; `start`/`run` replace the one-shot `call`
//!   for hosts that offer ops (`call` remains, and treats `ecall` as a fault).

use crate::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Op {
    Undecoded,
    Illegal,
    Nop,
    Li,
    Mv,
    Jal,
    Jalr,
    Beq,
    Bne,
    Blt,
    Bge,
    Bltu,
    Bgeu,
    Beqz,
    Bnez,
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
    /// `ecall`: stop and hand the call to the host (round seven, mesh ops).
    Ecall,
}

/// One decoded instruction, 8 bytes. The table has one per 2 bytes of image.
///
/// `w` packs the fields so one `ldrd` fetches the whole entry and bit-field
/// extracts give already-masked register indices: `op` bits 0-7, `rd` bits
/// 8-13 (0..=32), `step - 1` bit 15, `rs1` bits 16-20, `rs2` bits 24-28.
#[derive(Clone, Copy, Debug)]
#[repr(C, align(8))]
pub struct E {
    // Private: `op()` relies on `w` only ever being built here.
    w: u32,
    /// Immediate; for branches and jal, the target slot index.
    imm: i32,
}

impl E {
    pub const EMPTY: E = E {
        w: Op::Undecoded as u32,
        imm: 0,
    };

    pub fn op(&self) -> Op {
        // SAFETY: `w` is only built by `EMPTY` or `pack`, whose low byte is an `Op`.
        unsafe { core::mem::transmute::<u8, Op>(self.w as u8) }
    }

    fn pack(d: &Dec, step: usize) -> E {
        let w = d.op as u32
            | (d.rd as u32 & 63) << 8
            | ((step as u32 - 1) & 1) << 15
            | (d.rs1 as u32 & 31) << 16
            | (d.rs2 as u32 & 31) << 24;
        E { w, imm: d.imm }
    }
}

/// The decoder's working form of an entry.
struct Dec {
    op: Op,
    rd: u8,
    rs1: u8,
    rs2: u8,
    imm: i32,
}

// @phase: decode
#[cold]
#[inline(never)]
fn decode(mem: &Mem, table: &mut [E], ip: usize) -> Result<(), Fault> {
    let n = table.len();
    let pc = mem.image_base.wrapping_add(2 * ip as u32);
    let lo = mem.fetch16(pc)?;
    let (ins, step) = if lo & 3 != 3 {
        (expand(lo).ok_or(Fault::Illegal(pc))?, 1usize)
    } else {
        (lo | mem.fetch16(pc.wrapping_add(2))? << 16, 2)
    };
    let rd = ((ins >> 7) & 31) as u8;
    let rs1 = ((ins >> 15) & 31) as u8;
    let rs2 = ((ins >> 20) & 31) as u8;
    let f3 = (ins >> 12) & 7;
    let f7 = ins >> 25;
    // A target slot, or None if it lies outside the image.
    let slot = |t: u32| -> Option<i32> {
        let off = t.wrapping_sub(mem.image_base);
        (off & 1 == 0 && ((off >> 1) as usize) < n).then_some((off >> 1) as i32)
    };
    use Op::*;
    let mut e = Dec {
        op: Illegal,
        rd,
        rs1,
        rs2,
        imm: 0,
    };
    match ins & 0x7f {
        0x37 => (e.op, e.imm) = (Li, (ins & 0xffff_f000) as i32),
        0x17 => (e.op, e.imm) = (Li, pc.wrapping_add(ins & 0xffff_f000) as i32),
        0x6f => {
            let target = pc.wrapping_add(imm_j(ins) as u32);
            match slot(target) {
                Some(t) => (e.op, e.imm) = (Jal, t),
                // Code in another region: `jalr rd, target(x0)`; the jalr arm
                // finds it out of range and stops with Stop::Jump.
                None => {
                    (e.op, e.imm) = (Jalr, target as i32);
                    e.rs1 = 0;
                }
            }
        }
        0x67 => (e.op, e.imm) = (Jalr, imm_i(ins)),
        0x63 => {
            if let Some(t) = slot(pc.wrapping_add(imm_b(ins) as u32)) {
                e.imm = t;
                e.op = match (f3, rs2) {
                    (0, 0) => Beqz,
                    (1, 0) => Bnez,
                    (0, _) => Beq,
                    (1, _) => Bne,
                    (4, _) => Blt,
                    (5, _) => Bge,
                    (6, _) => Bltu,
                    (7, _) => Bgeu,
                    _ => Illegal,
                };
            }
        }
        0x03 => {
            e.imm = imm_i(ins);
            e.op = match f3 {
                0 => Lb,
                1 => Lh,
                2 => Lw,
                4 => Lbu,
                5 => Lhu,
                _ => Illegal,
            };
        }
        0x23 => {
            e.imm = imm_s(ins);
            e.op = match f3 {
                0 => Sb,
                1 => Sh,
                2 => Sw,
                _ => Illegal,
            };
        }
        0x13 => {
            e.imm = imm_i(ins);
            e.op = match f3 {
                0 if rs1 == 0 => Li,
                0 if e.imm == 0 => Mv,
                0 => Addi,
                1 => Slli,
                2 => Slti,
                3 => Sltiu,
                4 => Xori,
                5 if e.imm & 0x400 != 0 => Srai,
                5 => Srli,
                6 => Ori,
                _ => Andi,
            };
            if matches!(e.op, Slli | Srli | Srai) {
                e.imm &= 31;
            }
        }
        0x33 => {
            e.op = match (f7, f3) {
                (0, 0) if rs1 == 0 => Mv, // add rd, x0, rs2
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
            if e.op == Mv {
                e.rs1 = rs2;
            }
        }
        0x0f => e.op = Nop,
        0x73 if ins == 0x0000_0073 => e.op = Ecall,
        _ => {}
    }
    // Writes to x0: ALU results are dropped; links and loads go to scratch x[32].
    if rd == 0 {
        match e.op {
            Jal | Jalr | Lb | Lh | Lw | Lbu | Lhu => e.rd = 32,
            Li | Mv | Addi | Slti | Sltiu | Xori | Ori | Andi | Slli | Srli | Srai | Add | Sub
            | Sll | Slt | Sltu | Xor | Srl | Sra | Or | And | Mul | Mulh | Mulhsu | Mulhu | Div
            | Divu | Rem | Remu => e.op = Nop,
            _ => {}
        }
    }
    // Everything that can fall through needs an in-range successor. This (with
    // the target checks above) is the invariant that makes `get_unchecked` sound.
    if !matches!(e.op, Jal | Jalr | Illegal) && ip + step >= n {
        e.op = Illegal;
    }
    table[ip] = E::pack(&e, step);
    Ok(())
}

/// Why [`Cpu::run`] stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    /// The guest returned to the exit address; this is `a0`.
    Returned(u32),
    /// The guest executed `ecall`. The host reads the request from the
    /// registers, writes the result to `a0`/`a1`, and calls `run` again to
    /// continue after the `ecall`.
    Ecall,
    /// Control left this code region for `target` (a jal or jalr to code
    /// elsewhere; the link register is already written). The host finds the
    /// region holding `target` and calls [`Cpu::resume_at`], then `run`.
    Jump(u32),
    /// Round 7d: the instruction budget ran out (see [`Cpu::budget`]). The
    /// call is abandoned, not resumable.
    Budget,
}

pub struct Cpu {
    /// x0..x31, then x[32] as a sink for writes to x0 (64 slots: `& 63` indexing).
    pub x: [u32; 64],
    /// Where `run` continues (a table slot index).
    ip: usize,
    /// Round 7d: control transfers (jumps and branches, taken or not) left
    /// before `run` stops with [`Stop::Budget`]. Every loop and every
    /// recursion executes one per iteration, so a runaway guest is always
    /// caught; straight-line code between two is bounded by the image size.
    /// Counting these instead of every instruction keeps the cost off the
    /// dispatch path. `u32::MAX` is effectively unlimited.
    pub budget: u32,
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
            ip: 0,
            budget: u32::MAX,
            #[cfg(feature = "count")]
            steps: 0,
        }
    }

    /// Call the guest function at `entry` with the stack at the top of RAM.
    /// `table` must have exactly `image.len() / 2` entries, all `EMPTY` or
    /// filled by earlier calls. An `ecall` is a fault here; hosts that offer
    /// calls use [`Cpu::start`] and [`Cpu::run`].
    #[inline(never)]
    pub fn call(
        &mut self,
        mem: &mut Mem,
        table: &mut [E],
        entry: u32,
        args: &[u32],
    ) -> Result<u32, Fault> {
        let sp = (mem.ram_base.wrapping_add(mem.ram.len() as u32)) & !15;
        self.start(mem, table.len(), entry, args, sp, 0)?;
        match self.run(mem, table)? {
            Stop::Returned(v) => Ok(v),
            Stop::Ecall => Err(Fault::Illegal(
                mem.image_base.wrapping_add(2 * self.ip as u32),
            )),
            Stop::Jump(t) => Err(Fault::Fetch(t)),
            Stop::Budget => Err(Fault::Fetch(0)),
        }
    }

    /// Set up a call to `entry` with up to 8 arguments, stack pointer `sp`
    /// and global pointer `gp`.
    ///
    /// The register file is reused, not cleared: only `ra`, `sp`, `gp` and the
    /// argument registers are set. A callee can't rely on other registers'
    /// values anyway, and clearing 256 bytes per call was most of the cost of
    /// an applet-to-applet call (round seven).
    pub fn start(
        &mut self,
        mem: &Mem,
        table_len: usize,
        entry: u32,
        args: &[u32],
        sp: u32,
        gp: u32,
    ) -> Result<(), Fault> {
        self.resume_at(mem, table_len, entry)?;
        self.x[1] = EXIT;
        self.x[2] = sp & !15;
        self.x[3] = gp;
        for (i, a) in args.iter().take(8).enumerate() {
            self.x[10 + i] = *a;
        }
        Ok(())
    }

    /// Continue at `target` in the code region `mem.image` (after
    /// [`Stop::Jump`]): registers are untouched.
    pub fn resume_at(&mut self, mem: &Mem, table_len: usize, target: u32) -> Result<(), Fault> {
        if table_len != mem.image.len() / 2 {
            return Err(Fault::Fetch(target));
        }
        let off = target.wrapping_sub(mem.image_base);
        if off & 1 != 0 || (off >> 1) as usize >= table_len {
            return Err(Fault::Fetch(target));
        }
        self.ip = (off >> 1) as usize;
        Ok(())
    }

    /// Run from where `start` or the last `ecall` left off until the guest
    /// returns or executes `ecall`.
    #[inline(never)]
    pub fn run(&mut self, mem: &mut Mem, table: &mut [E]) -> Result<Stop, Fault> {
        let n = table.len();
        let base = mem.image_base;
        if n != mem.image.len() / 2 || self.ip >= n {
            return Err(Fault::Fetch(base.wrapping_add(2 * self.ip as u32)));
        }
        let to_slot = |t: u32| -> Result<usize, Fault> {
            let off = t.wrapping_sub(base);
            if off & 1 == 0 && ((off >> 1) as usize) < n {
                Ok((off >> 1) as usize)
            } else {
                Err(Fault::Fetch(t))
            }
        };
        let mut ip = self.ip;
        let mut budget = self.budget;
        let x = &mut self.x;
        // One control transfer: count it, stop when the budget is spent.
        macro_rules! tick {
            () => {{
                budget = budget.wrapping_sub(1);
                if budget == 0 {
                    self.budget = 0;
                    return Ok(Stop::Budget);
                }
            }};
        }
        loop {
            #[cfg(feature = "count")]
            {
                self.steps += 1;
            }
            // @phase: fetch
            // SAFETY: ip < n. The entry and jalr targets go through `to_slot`;
            // decode only stores in-range targets and fall-through successors.
            let e = unsafe { *table.get_unchecked(ip) };
            let w = e.w as usize;
            let rd = (w >> 8) & 63;
            let r1 = (w >> 16) & 31;
            let r2 = (w >> 24) & 31;
            let next = ip + 1 + ((w >> 15) & 1);
            // @phase: execute
            // One arm per op (a nested match would be a second dispatch).
            macro_rules! br {
                ($f:expr) => {{
                    tick!();
                    ip = if $f(x[r1], x[r2]) {
                        e.imm as usize
                    } else {
                        next
                    }
                }};
            }
            macro_rules! ld {
                ($f3:expr) => {{
                    x[rd] = mem.load(x[r1].wrapping_add(e.imm as u32), $f3)?;
                    ip = next;
                }};
            }
            macro_rules! st {
                ($f3:expr) => {{
                    mem.store(x[r1].wrapping_add(e.imm as u32), $f3, x[r2])?;
                    ip = next;
                }};
            }
            macro_rules! alui {
                ($f:expr) => {{
                    x[rd] = $f(x[r1], e.imm as u32);
                    ip = next;
                }};
            }
            macro_rules! alur {
                ($f:expr) => {{
                    x[rd] = $f(x[r1], x[r2]);
                    ip = next;
                }};
            }
            // @phase: dispatch
            match e.op() {
                // @phase: execute
                Op::Undecoded => {
                    #[cfg(feature = "count")]
                    {
                        self.steps -= 1;
                    }
                    decode(mem, table, ip)?;
                    continue;
                }
                Op::Nop => ip = next,
                Op::Li => {
                    x[rd] = e.imm as u32;
                    ip = next;
                }
                Op::Mv => {
                    x[rd] = x[r1];
                    ip = next;
                }
                Op::Jal => {
                    tick!();
                    x[rd] = base.wrapping_add(2 * next as u32);
                    ip = e.imm as usize;
                }
                Op::Jalr => {
                    tick!();
                    let t = x[r1].wrapping_add(e.imm as u32) & !1;
                    x[rd] = base.wrapping_add(2 * next as u32);
                    if t == EXIT {
                        self.budget = budget;
                        return Ok(Stop::Returned(x[10]));
                    }
                    match to_slot(t) {
                        Ok(s) => ip = s,
                        // Code in another region (a far jal, or a return across regions).
                        Err(_) => {
                            self.budget = budget;
                            return Ok(Stop::Jump(t));
                        }
                    }
                }
                Op::Beqz => {
                    tick!();
                    ip = if x[r1] == 0 { e.imm as usize } else { next }
                }
                Op::Bnez => {
                    tick!();
                    ip = if x[r1] != 0 { e.imm as usize } else { next }
                }
                Op::Beq => br!(|a: u32, b: u32| a == b),
                Op::Bne => br!(|a: u32, b: u32| a != b),
                Op::Blt => br!(|a: u32, b: u32| (a as i32) < (b as i32)),
                Op::Bge => br!(|a: u32, b: u32| (a as i32) >= (b as i32)),
                Op::Bltu => br!(|a: u32, b: u32| a < b),
                Op::Bgeu => br!(|a: u32, b: u32| a >= b),
                Op::Lb => ld!(0),
                Op::Lh => ld!(1),
                Op::Lw => ld!(2),
                Op::Lbu => ld!(4),
                Op::Lhu => ld!(5),
                Op::Sb => st!(0),
                Op::Sh => st!(1),
                Op::Sw => st!(2),
                Op::Addi => alui!(|a: u32, i: u32| a.wrapping_add(i)),
                Op::Slti => alui!(|a: u32, i: u32| ((a as i32) < (i as i32)) as u32),
                Op::Sltiu => alui!(|a: u32, i: u32| (a < i) as u32),
                Op::Xori => alui!(|a: u32, i: u32| a ^ i),
                Op::Ori => alui!(|a: u32, i: u32| a | i),
                Op::Andi => alui!(|a: u32, i: u32| a & i),
                Op::Slli => alui!(|a: u32, i: u32| a << i),
                Op::Srli => alui!(|a: u32, i: u32| a >> i),
                Op::Srai => alui!(|a: u32, i: u32| ((a as i32) >> i) as u32),
                Op::Add => alur!(|a: u32, b: u32| a.wrapping_add(b)),
                Op::Sub => alur!(|a: u32, b: u32| a.wrapping_sub(b)),
                Op::Sll => alur!(|a: u32, b: u32| a << (b & 31)),
                Op::Slt => alur!(|a: u32, b: u32| ((a as i32) < (b as i32)) as u32),
                Op::Sltu => alur!(|a: u32, b: u32| (a < b) as u32),
                Op::Xor => alur!(|a: u32, b: u32| a ^ b),
                Op::Srl => alur!(|a: u32, b: u32| a >> (b & 31)),
                Op::Sra => alur!(|a: u32, b: u32| ((a as i32) >> (b & 31)) as u32),
                Op::Or => alur!(|a: u32, b: u32| a | b),
                Op::And => alur!(|a: u32, b: u32| a & b),
                Op::Mul => alur!(|a, b| alu(0, false, true, a, b)),
                Op::Mulh => alur!(|a, b| alu(1, false, true, a, b)),
                Op::Mulhsu => alur!(|a, b| alu(2, false, true, a, b)),
                Op::Mulhu => alur!(|a, b| alu(3, false, true, a, b)),
                Op::Div => alur!(|a, b| alu(4, false, true, a, b)),
                Op::Divu => alur!(|a, b| alu(5, false, true, a, b)),
                Op::Rem => alur!(|a, b| alu(6, false, true, a, b)),
                Op::Remu => alur!(|a, b| alu(7, false, true, a, b)),
                Op::Ecall => {
                    // Decode proved `next` is in range (ecall can fall through).
                    self.ip = next;
                    self.budget = budget;
                    return Ok(Stop::Ecall);
                }
                Op::Illegal => return Err(Fault::Illegal(base.wrapping_add(2 * ip as u32))),
            }
        }
    }
}
