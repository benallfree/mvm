//! RISC-V immediate field encoders, shared by the linker (patching) and the
//! emulator (expanding compressed instructions).

pub fn sext(v: u32, bits: u32) -> i32 {
    ((v << (32 - bits)) as i32) >> (32 - bits)
}

/// I-type imm[11:0] -> bits 31:20.
pub fn i_imm(imm: i32) -> u32 {
    ((imm as u32) & 0xfff) << 20
}

/// S-type imm[11:5] -> 31:25, imm[4:0] -> 11:7.
pub fn s_imm(imm: i32) -> u32 {
    let v = imm as u32;
    ((v >> 5) & 0x7f) << 25 | (v & 0x1f) << 7
}

/// B-type imm[12|10:5] -> 31:25, imm[4:1|11] -> 11:7.
pub fn b_imm(imm: i32) -> u32 {
    let v = imm as u32;
    ((v >> 12) & 1) << 31 | ((v >> 5) & 0x3f) << 25 | ((v >> 1) & 0xf) << 8 | ((v >> 11) & 1) << 7
}

/// U-type imm[31:12] -> 31:12.
pub fn u_imm(imm: i32) -> u32 {
    (imm as u32) & 0xffff_f000
}

/// J-type imm[20|10:1|11|19:12] -> 31:12.
pub fn j_imm(imm: i32) -> u32 {
    let v = imm as u32;
    ((v >> 20) & 1) << 31
        | ((v >> 1) & 0x3ff) << 21
        | ((v >> 11) & 1) << 20
        | ((v >> 12) & 0xff) << 12
}

/// CJ-format offset[11|4|9:8|10|6|7|3:1|5] -> bits 12:2.
pub fn cj_imm(imm: i32) -> u16 {
    let o = imm as u32;
    (((o >> 11) & 1) << 12
        | ((o >> 4) & 1) << 11
        | ((o >> 8) & 3) << 9
        | ((o >> 10) & 1) << 8
        | ((o >> 6) & 1) << 7
        | ((o >> 7) & 1) << 6
        | ((o >> 1) & 7) << 3
        | ((o >> 5) & 1) << 2) as u16
}

/// CB-format offset[8|4:3] -> 12:10, offset[7:6|2:1|5] -> 6:2.
pub fn cb_imm(imm: i32) -> u16 {
    let o = imm as u32;
    (((o >> 8) & 1) << 12
        | ((o >> 3) & 3) << 10
        | ((o >> 6) & 3) << 5
        | ((o >> 1) & 3) << 3
        | ((o >> 5) & 1) << 2) as u16
}

pub fn enc_i(op: u32, rd: u32, f3: u32, rs1: u32, imm: i32) -> u32 {
    i_imm(imm) | rs1 << 15 | f3 << 12 | rd << 7 | op
}
pub fn enc_s(op: u32, f3: u32, rs1: u32, rs2: u32, imm: i32) -> u32 {
    s_imm(imm) | rs2 << 20 | rs1 << 15 | f3 << 12 | op
}
pub fn enc_r(op: u32, rd: u32, f3: u32, rs1: u32, rs2: u32, f7: u32) -> u32 {
    f7 << 25 | rs2 << 20 | rs1 << 15 | f3 << 12 | rd << 7 | op
}
pub fn enc_b(f3: u32, rs1: u32, rs2: u32, imm: i32) -> u32 {
    b_imm(imm) | rs2 << 20 | rs1 << 15 | f3 << 12 | 0x63
}
pub fn enc_u(op: u32, rd: u32, imm: i32) -> u32 {
    u_imm(imm) | rd << 7 | op
}
pub fn enc_j(rd: u32, imm: i32) -> u32 {
    j_imm(imm) | rd << 7 | 0x6f
}

#[cfg(test)]
mod tests {
    use super::*;

    // Decoders written independently of the encoders above.
    fn dec_cj(i: u32) -> i32 {
        let o = ((i >> 1) & 0x800)
            | ((i >> 7) & 0x10)
            | ((i >> 1) & 0x300)
            | ((i << 2) & 0x400)
            | ((i >> 1) & 0x40)
            | ((i << 1) & 0x80)
            | ((i >> 2) & 0xe)
            | ((i << 3) & 0x20);
        sext(o, 12)
    }
    fn dec_cb(i: u32) -> i32 {
        let o = ((i >> 4) & 0x100)
            | ((i >> 7) & 0x18)
            | ((i << 1) & 0xc0)
            | ((i >> 2) & 6)
            | ((i << 3) & 0x20);
        sext(o, 9)
    }
    fn dec_b(i: u32) -> i32 {
        let o = ((i >> 19) & 0x1000) | ((i << 4) & 0x800) | ((i >> 20) & 0x7e0) | ((i >> 7) & 0x1e);
        sext(o, 13)
    }
    fn dec_j(i: u32) -> i32 {
        let o = ((i >> 11) & 0x10_0000) | (i & 0xf_f000) | ((i >> 9) & 0x800) | ((i >> 20) & 0x7fe);
        sext(o, 21)
    }

    #[test]
    fn roundtrip_offsets() {
        for o in (-2048..2048).step_by(2) {
            assert_eq!(dec_cj(cj_imm(o) as u32), o, "cj {o}");
        }
        for o in (-256..256).step_by(2) {
            assert_eq!(dec_cb(cb_imm(o) as u32), o, "cb {o}");
        }
        for o in (-4096..4096).step_by(2) {
            assert_eq!(dec_b(b_imm(o)), o, "b {o}");
        }
        for o in (-(1 << 20)..(1 << 20)).step_by(2002) {
            assert_eq!(dec_j(j_imm(o)), o, "j {o}");
        }
    }
}
