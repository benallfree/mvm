//! Ahead-of-time translator prototype: RV32IMC flat image -> Thumb-2 assembly.
//!
//! Usage: aot <image.bin> <base hex> <out.s> <entry=hexaddr>...
//!
//! - Code is found by following control flow from the entries, so data in the
//!   image (the CRC table) is never translated as code.
//! - Mostly 1:N, one guest instruction to one or more Thumb instructions. No
//!   optimisation passes.
//! - Register mapping (see `Map`): `ra` -> `lr`, `sp` -> `sp`, `a0..a3` -> `r0..r3`
//!   (so host arguments and the result are already in place), then the
//!   statically hottest guest registers -> `r4..r10`. The rest are spilled to a
//!   register file in RAM, addressed through `r10`, which is then no longer mapped.
//!   `r11` and `r12` are scratch.
//! - Position independent: every branch is PC-relative, and guest addresses of image
//!   data (built by `lui`/`addi`) become `adr` to a copy of the image embedded
//!   after the code. Assembling leaves no relocations; `run.sh` checks that.
//! - Memory model: guest pointers are host pointers, as v1 trusts applets.
//!   Loads and stores are plain Thumb loads and stores with no checks.
//! - Host imports: the interpreters have none (`ecall` faults); neither does this.
//!
//! Each emitted instruction is tagged with a category label (`aotc_<cat>_<n>`)
//! so the harness can say where the cycles go.

use interp::{expand, imm_b, imm_i, imm_j, imm_s};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write;

#[derive(Clone, Copy, Debug)]
enum I {
    Lui {
        rd: u32,
        imm: u32,
    },
    Auipc {
        rd: u32,
    },
    Jal {
        rd: u32,
        t: u32,
    },
    Jalr {
        rd: u32,
        rs1: u32,
        imm: i32,
    },
    Br {
        f3: u32,
        rs1: u32,
        rs2: u32,
        t: u32,
    },
    Ld {
        f3: u32,
        rd: u32,
        rs1: u32,
        imm: i32,
    },
    St {
        f3: u32,
        rs1: u32,
        rs2: u32,
        imm: i32,
    },
    OpImm {
        f3: u32,
        alt: bool,
        rd: u32,
        rs1: u32,
        imm: i32,
    },
    Op {
        f3: u32,
        alt: bool,
        m: bool,
        rd: u32,
        rs1: u32,
        rs2: u32,
    },
    Fence,
    Sys,
}

fn decode(ins: u32, pc: u32) -> Result<I, String> {
    let rd = (ins >> 7) & 31;
    let f3 = (ins >> 12) & 7;
    let rs1 = (ins >> 15) & 31;
    let rs2 = (ins >> 20) & 31;
    Ok(match ins & 0x7f {
        0x37 => I::Lui {
            rd,
            imm: ins & 0xffff_f000,
        },
        0x17 => I::Auipc { rd },
        0x6f => I::Jal {
            rd,
            t: pc.wrapping_add(imm_j(ins) as u32),
        },
        0x67 => I::Jalr {
            rd,
            rs1,
            imm: imm_i(ins),
        },
        0x63 => I::Br {
            f3,
            rs1,
            rs2,
            t: pc.wrapping_add(imm_b(ins) as u32),
        },
        0x03 => I::Ld {
            f3,
            rd,
            rs1,
            imm: imm_i(ins),
        },
        0x23 => I::St {
            f3,
            rs1,
            rs2,
            imm: imm_s(ins),
        },
        0x13 => I::OpImm {
            f3,
            alt: f3 == 5 && ins & 0x4000_0000 != 0,
            rd,
            rs1,
            imm: imm_i(ins),
        },
        0x33 => I::Op {
            f3,
            alt: ins & 0x4000_0000 != 0,
            m: ins & 0x0200_0000 != 0,
            rd,
            rs1,
            rs2,
        },
        0x0f => I::Fence,
        0x73 => I::Sys,
        _ => return Err(format!("illegal instruction {ins:#010x} at {pc:#x}")),
    })
}

struct Guest {
    base: u32,
    img: Vec<u8>,
    /// pc -> (instruction, length)
    code: BTreeMap<u32, (I, u32)>,
    /// branch / jump targets (block starts)
    targets: BTreeSet<u32>,
}

impl Guest {
    fn fetch(&self, pc: u32) -> Result<(u32, u32), String> {
        let o = pc.wrapping_sub(self.base) as usize;
        let h = |o: usize| -> Result<u32, String> {
            self.img
                .get(o..o + 2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]) as u32)
                .ok_or(format!("pc {pc:#x} outside image"))
        };
        let lo = h(o)?;
        if lo & 3 != 3 {
            Ok((
                expand(lo).ok_or(format!("illegal compressed at {pc:#x}"))?,
                2,
            ))
        } else {
            Ok((lo | h(o + 2)? << 16, 4))
        }
    }

    /// Follow control flow from the entries.
    fn discover(&mut self, entries: &[u32]) -> Result<(), String> {
        let mut work: Vec<u32> = entries.to_vec();
        self.targets.extend(entries);
        while let Some(pc) = work.pop() {
            if self.code.contains_key(&pc) {
                continue;
            }
            let (raw, len) = self.fetch(pc)?;
            let i = decode(raw, pc)?;
            self.code.insert(pc, (i, len));
            let next = pc + len;
            match i {
                I::Jal { rd: 0, t } => {
                    self.targets.insert(t);
                    work.push(t);
                }
                I::Jal { t, .. } => {
                    self.targets.insert(t);
                    work.extend([t, next]);
                }
                I::Jalr { rd: 0, .. } => {}
                I::Jalr { .. } => work.push(next),
                I::Br { t, .. } => {
                    self.targets.insert(t);
                    work.extend([t, next]);
                }
                _ => work.push(next),
            }
        }
        Ok(())
    }
}

/// Where a guest register lives on the host.
#[derive(Clone, Debug)]
enum Loc {
    Zero,
    Reg(&'static str),
    Spill(u32),
}

struct Map {
    loc: Vec<Loc>,
    spills: bool,
}

fn regname(r: u32) -> String {
    const N: [&str; 32] = [
        "zero", "ra", "sp", "gp", "tp", "t0", "t1", "t2", "s0", "s1", "a0", "a1", "a2", "a3", "a4",
        "a5", "a6", "a7", "s2", "s3", "s4", "s5", "s6", "s7", "s8", "s9", "s10", "s11", "t3", "t4",
        "t5", "t6",
    ];
    format!("x{r}/{}", N[r as usize])
}

impl Map {
    /// Fixed ABI registers first, then the statically hottest guest registers.
    fn build(g: &Guest) -> (Map, Vec<(u32, usize)>) {
        let mut uses = [0usize; 32];
        for (i, _) in g.code.values() {
            for r in regs(i) {
                uses[r as usize] += 1;
            }
        }
        let mut loc = vec![Loc::Spill(0); 32];
        loc[0] = Loc::Zero;
        loc[1] = Loc::Reg("lr");
        loc[2] = Loc::Reg("sp");
        for (k, h) in ["r0", "r1", "r2", "r3"].into_iter().enumerate() {
            loc[10 + k] = Loc::Reg(h);
        }
        let mut rest: Vec<u32> = (3..32)
            .filter(|r| !(10..14).contains(r) && uses[*r as usize] > 0)
            .collect();
        rest.sort_by_key(|r| std::cmp::Reverse(uses[*r as usize]));
        let pool = ["r4", "r5", "r6", "r7", "r8", "r9", "r10"];
        let spills = rest.len() > pool.len();
        // With spills, r10 holds the register-file base, so one fewer register maps.
        let avail = if spills { &pool[..6] } else { &pool[..] };
        let mut slot = 0;
        for (k, r) in rest.iter().enumerate() {
            loc[*r as usize] = match avail.get(k) {
                Some(h) => Loc::Reg(h),
                None => {
                    slot += 1;
                    Loc::Spill((slot - 1) * 4)
                }
            };
        }
        let usage = (0..32)
            .filter(|r| uses[*r as usize] > 0)
            .map(|r| (r, uses[r as usize]))
            .collect();
        (Map { loc, spills }, usage)
    }
}

fn regs(i: &I) -> Vec<u32> {
    match *i {
        I::Lui { rd, .. } | I::Auipc { rd } | I::Jal { rd, .. } => vec![rd],
        I::Jalr { rd, rs1, .. } | I::Ld { rd, rs1, .. } | I::OpImm { rd, rs1, .. } => vec![rd, rs1],
        I::Br { rs1, rs2, .. } | I::St { rs1, rs2, .. } => vec![rs1, rs2],
        I::Op { rd, rs1, rs2, .. } => vec![rd, rs1, rs2],
        I::Fence | I::Sys => vec![],
    }
}

/// Is `v` a Thumb-2 modified immediate?
fn thumb_imm(v: u32) -> bool {
    let b = v & 0xff;
    v <= 0xff
        || v == (b << 16 | b)
        || v == (b << 24 | b << 8) && b != 0
        || v == b * 0x0101_0101
        || (8..32).any(|r| {
            let x = v.rotate_left(r);
            x <= 0xff && x & 0x80 != 0
        })
}

struct Em<'a> {
    g: &'a Guest,
    m: &'a Map,
    out: String,
    n: usize,
    cat: &'static str,
    /// Guest registers known to hold a constant (from lui) in the current block.
    k: HashMap<u32, u32>,
}

impl Em<'_> {
    fn emit(&mut self, cat: &'static str, line: &str) {
        if cat != self.cat {
            self.n += 1;
            let _ = writeln!(self.out, "aotc_{cat}_{}:", self.n);
            self.cat = cat;
        }
        let _ = writeln!(self.out, "    {line}");
    }

    fn in_img(&self, a: u32) -> bool {
        a.wrapping_sub(self.g.base) < self.g.img.len() as u32
    }

    /// Materialise a constant into `reg`.
    fn konst(&mut self, cat: &'static str, reg: &str, v: u32) {
        if v <= 0xffff {
            self.emit(cat, &format!("movw {reg}, #{v}"));
        } else if thumb_imm(v) {
            self.emit(cat, &format!("mov.w {reg}, #{v}"));
        } else if thumb_imm(!v) {
            self.emit(cat, &format!("mvn {reg}, #{}", !v));
        } else {
            self.emit(cat, &format!("movw {reg}, #{}", v & 0xffff));
            self.emit(cat, &format!("movt {reg}, #{}", v >> 16));
        }
    }

    /// Host register holding guest `r` for reading; `scratch` is used if needed.
    fn src(&mut self, r: u32, scratch: &'static str) -> Result<String, String> {
        if let Some(&c) = self.k.get(&r) {
            if self.in_img(c) {
                return Err(format!(
                    "image address {c:#x} in x{r} used outside an addi/load/store"
                ));
            }
        }
        Ok(match self.m.loc[r as usize].clone() {
            Loc::Zero => {
                self.emit("const", &format!("movs {scratch}, #0"));
                scratch.to_string()
            }
            Loc::Reg(h) => h.to_string(),
            Loc::Spill(o) => {
                self.emit("spill", &format!("ldr {scratch}, [r10, #{o}]"));
                scratch.to_string()
            }
        })
    }

    /// Host register to write guest `rd` into (None for x0), plus a store-back if spilled.
    fn dst(&self, rd: u32) -> Option<(String, Option<u32>)> {
        match self.m.loc[rd as usize] {
            Loc::Zero => None,
            Loc::Reg(h) => Some((h.to_string(), None)),
            Loc::Spill(o) => Some(("r11".to_string(), Some(o))),
        }
    }

    fn finish(&mut self, st: Option<u32>) {
        if let Some(o) = st {
            self.emit("spill", &format!("str r11, [r10, #{o}]"));
        }
    }

    fn set_flag(&mut self, rd: u32, cc: &str, ncc: &str) {
        if let Some((d, st)) = self.dst(rd) {
            self.emit("flags", &format!("ite {cc}"));
            self.emit("flags", &format!("mov{cc} {d}, #1"));
            self.emit("flags", &format!("mov{ncc} {d}, #0"));
            self.finish(st);
        }
    }

    fn insn(&mut self, pc: u32, i: I) -> Result<(), String> {
        let lbl = |t: u32| format!("g_{t:x}");
        let img_ref = |a: u32, base: u32| format!(".Limg+{}", a - base);
        match i {
            I::Lui { rd, imm } => {
                if let Some((d, st)) = self.dst(rd) {
                    self.konst("op", &d, imm);
                    self.finish(st);
                }
                self.k.insert(rd, imm);
                return Ok(());
            }
            I::Auipc { .. } => {
                return Err(format!(
                    "auipc at {pc:#x}: build the guest with the small code model"
                ))
            }
            I::Jal { rd: 0, t } => self.emit("op", &format!("b.w {}", lbl(t))),
            I::Jal { rd: 1, t } => {
                self.emit("op", &format!("bl {}", lbl(t)));
                self.k.clear(); // the callee may change anything
            }
            I::Jalr {
                rd: 0,
                rs1: 1,
                imm: 0,
            } => self.emit("op", "bx lr"),
            I::Jal { .. } | I::Jalr { .. } => {
                return Err(format!(
                    "{i:?} at {pc:#x}: only direct calls and returns are supported"
                ))
            }
            I::Br { f3, rs1, rs2, t } => {
                let a = self.src(rs1, "r12")?;
                if rs2 == 0 {
                    self.emit("op", &format!("cmp {a}, #0"));
                } else {
                    let b = self.src(rs2, "r11")?;
                    self.emit("op", &format!("cmp {a}, {b}"));
                }
                let cc = ["eq", "ne", "", "", "lt", "ge", "lo", "hs"][f3 as usize];
                self.emit("op", &format!("b{cc}.w {}", lbl(t)));
            }
            I::Ld { f3, rd, rs1, imm } => {
                let mn = match f3 {
                    0 => "ldrsb",
                    1 => "ldrsh",
                    2 => "ldr",
                    4 => "ldrb",
                    5 => "ldrh",
                    _ => return Err(format!("bad load at {pc:#x}")),
                };
                let Some((d, st)) = self.dst(rd) else {
                    return Ok(());
                };
                let base = match self.k.get(&rs1).copied() {
                    Some(c) if self.in_img(c.wrapping_add(imm as u32)) => {
                        self.emit(
                            "addr",
                            &format!(
                                "adr.w r12, {}",
                                img_ref(c.wrapping_add(imm as u32), self.g.base)
                            ),
                        );
                        self.emit("op", &format!("{mn} {d}, [r12]"));
                        self.k.remove(&rd);
                        self.finish(st);
                        return Ok(());
                    }
                    _ => self.src(rs1, "r12")?,
                };
                self.mem_op(mn, &d, &base, imm)?;
                self.k.remove(&rd);
                self.finish(st);
                return Ok(());
            }
            I::St { f3, rs1, rs2, imm } => {
                let mn = ["strb", "strh", "str"]
                    .get(f3 as usize)
                    .ok_or(format!("bad store at {pc:#x}"))?;
                let v = self.src(rs2, "r11")?;
                let base = self.src(rs1, "r12")?;
                self.mem_op(mn, &v, &base, imm)?;
            }
            I::OpImm {
                f3,
                alt,
                rd,
                rs1,
                imm,
            } => {
                // An image address built by lui/addi becomes a PC-relative adr.
                if f3 == 0 {
                    if let Some(c) = self.k.get(&rs1).copied() {
                        let v = c.wrapping_add(imm as u32);
                        if let Some((d, st)) = self.dst(rd) {
                            if self.in_img(v) {
                                self.emit(
                                    "addr",
                                    &format!("adr.w {d}, {}", img_ref(v, self.g.base)),
                                );
                            } else {
                                self.konst("op", &d, v);
                            }
                            self.finish(st);
                        }
                        self.k.insert(rd, v);
                        if self.in_img(v) {
                            self.k.remove(&rd); // now a host pointer, not a guest constant
                        }
                        return Ok(());
                    }
                }
                let Some((d, st)) = self.dst(rd) else {
                    return Ok(());
                };
                if rs1 == 0 {
                    self.konst("op", &d, imm as u32);
                    self.k.insert(rd, imm as u32);
                    self.finish(st);
                    return Ok(());
                }
                let a = self.src(rs1, "r12")?;
                let u = imm as u32;
                match f3 {
                    0 if imm == 0 => self.emit("op", &format!("mov {d}, {a}")),
                    0 if imm > 0 => self.emit("op", &format!("add {d}, {a}, #{imm}")),
                    0 => self.emit("op", &format!("sub {d}, {a}, #{}", -imm)),
                    1 => self.emit("op", &format!("lsl {d}, {a}, #{}", imm & 31)),
                    5 if imm & 31 == 0 => self.emit("op", &format!("mov {d}, {a}")),
                    5 if alt => self.emit("op", &format!("asr {d}, {a}, #{}", imm & 31)),
                    5 => self.emit("op", &format!("lsr {d}, {a}, #{}", imm & 31)),
                    2 | 3 => {
                        if thumb_imm(u) {
                            self.emit("op", &format!("cmp {a}, #{u}"));
                        } else {
                            self.konst("const", "r11", u);
                            self.emit("op", &format!("cmp {a}, r11"));
                        }
                        if f3 == 2 {
                            self.set_flag(rd, "lt", "ge");
                        } else {
                            self.set_flag(rd, "lo", "hs");
                        }
                        self.k.remove(&rd);
                        return Ok(());
                    }
                    4 if imm == -1 => self.emit("op", &format!("mvn {d}, {a}")),
                    7 if !thumb_imm(u) && thumb_imm(!u) => {
                        self.emit("op", &format!("bic {d}, {a}, #{}", !u))
                    }
                    4 | 6 | 7 => {
                        let mn = ["", "", "", "", "eor", "", "orr", "and"][f3 as usize];
                        if thumb_imm(u) {
                            self.emit("op", &format!("{mn} {d}, {a}, #{u}"));
                        } else {
                            self.konst("const", "r11", u);
                            self.emit("op", &format!("{mn} {d}, {a}, r11"));
                        }
                    }
                    _ => return Err(format!("bad op-imm at {pc:#x}")),
                }
                self.k.remove(&rd);
                self.finish(st);
                return Ok(());
            }
            I::Op {
                f3,
                alt,
                m,
                rd,
                rs1,
                rs2,
            } => {
                let Some((d, st)) = self.dst(rd) else {
                    return Ok(());
                };
                let a = self.src(rs1, "r12")?;
                let b = self.src(rs2, "r11")?;
                match (m, f3) {
                    (false, 0) => self.emit(
                        "op",
                        &format!("{} {d}, {a}, {b}", if alt { "sub" } else { "add" }),
                    ),
                    (false, 4) => self.emit("op", &format!("eor {d}, {a}, {b}")),
                    (false, 6) => self.emit("op", &format!("orr {d}, {a}, {b}")),
                    (false, 7) => self.emit("op", &format!("and {d}, {a}, {b}")),
                    (false, 1 | 5) => {
                        // RISC-V shifts use the low 5 bits; Thumb uses the low 8.
                        self.emit("shiftmask", &format!("and r11, {b}, #31"));
                        let mn = if f3 == 1 {
                            "lsl"
                        } else if alt {
                            "asr"
                        } else {
                            "lsr"
                        };
                        self.emit("op", &format!("{mn} {d}, {a}, r11"));
                    }
                    (false, 2) => {
                        self.emit("op", &format!("cmp {a}, {b}"));
                        self.set_flag(rd, "lt", "ge");
                        self.k.remove(&rd);
                        return Ok(());
                    }
                    (false, 3) => {
                        self.emit("op", &format!("cmp {a}, {b}"));
                        self.set_flag(rd, "lo", "hs");
                        self.k.remove(&rd);
                        return Ok(());
                    }
                    (true, 0) => self.emit("op", &format!("mul {d}, {a}, {b}")),
                    (true, 1 | 3) => {
                        let lo = if d == "r12" { "r11" } else { "r12" };
                        let mn = if f3 == 1 { "smull" } else { "umull" };
                        self.emit("op", &format!("{mn} {lo}, {d}, {a}, {b}"));
                    }
                    (true, 4..=7) => {
                        // RISC-V: x/0 = -1 (all ones), x%0 = x. Thumb: x/0 = 0.
                        let mn = if f3 & 1 == 0 { "sdiv" } else { "udiv" };
                        let q = if b == "r12" { "r11" } else { "r12" };
                        let q = if q == a { "r11" } else { q };
                        self.emit("op", &format!("{mn} {q}, {a}, {b}"));
                        if f3 >= 6 {
                            self.emit("op", &format!("mls {d}, {q}, {b}, {a}"));
                        } else {
                            self.emit("flags", &format!("cmp {b}, #0"));
                            self.emit("flags", "it eq");
                            self.emit("flags", &format!("mvneq {q}, #0"));
                            self.emit("op", &format!("mov {d}, {q}"));
                        }
                    }
                    _ => return Err(format!("{i:?} at {pc:#x} not supported (mulhsu)")),
                }
                self.k.remove(&rd);
                self.finish(st);
                return Ok(());
            }
            I::Fence => {}
            I::Sys => {
                return Err(format!(
                "ecall/ebreak at {pc:#x}: host imports are not supported (nor in the interpreters)"
            ))
            }
        }
        Ok(())
    }

    fn mem_op(&mut self, mn: &str, r: &str, base: &str, imm: i32) -> Result<(), String> {
        if (-255..=4095).contains(&imm) {
            self.emit("op", &format!("{mn} {r}, [{base}, #{imm}]"));
        } else {
            let s = if base == "r12" || r == "r12" {
                "r11"
            } else {
                "r12"
            };
            self.konst("const", s, imm as u32);
            self.emit("op", &format!("{mn} {r}, [{base}, {s}]"));
        }
        Ok(())
    }
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 4 {
        return Err("usage: aot <image.bin> <base hex> <out.s> <name=hexaddr>...".into());
    }
    let img = std::fs::read(&args[0]).map_err(|e| e.to_string())?;
    let base =
        u32::from_str_radix(args[1].trim_start_matches("0x"), 16).map_err(|e| e.to_string())?;
    let entries: Vec<(String, u32)> = args[3..]
        .iter()
        .map(|e| {
            let (n, a) = e.split_once('=').expect("entry is name=hexaddr");
            (
                n.to_string(),
                u32::from_str_radix(a.trim_start_matches("0x"), 16).unwrap(),
            )
        })
        .collect();
    let mut g = Guest {
        base,
        img,
        code: BTreeMap::new(),
        targets: BTreeSet::new(),
    };
    g.discover(&entries.iter().map(|e| e.1).collect::<Vec<_>>())?;
    let (map, usage) = Map::build(&g);

    let mut em = Em {
        g: &g,
        m: &map,
        out: String::new(),
        n: 0,
        cat: "",
        k: HashMap::new(),
    };
    let _ = writeln!(em.out, "@ Generated by bench/aot. Do not edit.\n    .syntax unified\n    .cpu cortex-m4\n    .thumb\n    .text\n    .p2align 2");
    let _ = writeln!(em.out, "@ register map:");
    for (r, n) in &usage {
        let _ = writeln!(
            em.out,
            "@   {:<9} {:>3} static uses -> {:?}",
            regname(*r),
            n,
            map.loc[*r as usize]
        );
    }
    // Entry stubs: host AAPCS in, guest code (ra = lr) out.
    for (name, a) in &entries {
        let _ = writeln!(
            em.out,
            "    .globl aot_{name}\n    .thumb_func\naot_{name}:"
        );
        em.cat = "";
        em.emit("stub", "push {r4-r11, lr}");
        if map.spills {
            em.emit("stub", "sub sp, #128");
            em.emit("stub", "mov r10, sp");
        }
        em.emit("stub", &format!("bl g_{a:x}"));
        if map.spills {
            em.emit("stub", "add sp, #128");
        }
        em.emit("stub", "pop {r4-r11, pc}");
    }
    let mut prev_end = None;
    let code: Vec<(u32, (I, u32))> = g.code.iter().map(|(a, b)| (*a, *b)).collect();
    for (pc, (i, len)) in code {
        if g.targets.contains(&pc) || prev_end != Some(pc) {
            em.k.clear(); // block start: no known constants
        }
        let _ = writeln!(em.out, "g_{pc:x}:");
        em.cat = "";
        em.insn(pc, i)?;
        prev_end = Some(pc + len);
    }
    let _ = writeln!(
        em.out,
        "    .p2align 4\n    .globl aot_image\naot_image:\n.Limg:\n    .incbin \"{}\"",
        std::fs::canonicalize(&args[0]).unwrap().display()
    );
    std::fs::write(&args[2], &em.out).map_err(|e| e.to_string())?;
    let guest_bytes: u32 = g.code.values().map(|(_, l)| l).sum();
    eprintln!("guest code bytes: {guest_bytes}");
    eprintln!(
        "translated {} guest instructions from {} entries; {} guest registers used, spills: {}",
        g.code.len(),
        entries.len(),
        usage.len(),
        map.spills
    );
    Ok(())
}
