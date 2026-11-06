#![no_std]
//! Speed-spike workloads. Built twice: for RV32IMC (chunked, run in the
//! interpreter) and natively for the Cortex-M4.

/// CRC-16/CCITT-FALSE (poly 0x1021, init 0xFFFF), bitwise. "123456789" -> 0x29B1.
///
/// # Safety
/// `ptr` must be valid for `len` bytes.
#[no_mangle]
#[inline(never)]
pub unsafe extern "C" fn crc16(ptr: *const u8, len: usize) -> u32 {
    let data = core::slice::from_raw_parts(ptr, len);
    let mut crc: u16 = 0xFFFF;
    for &b in data {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc as u32
}

/// CRC-16/CCITT-FALSE, table-driven: the algorithm LLVM derives from `crc16`
/// on Thumb and RV32. Used as the native baseline where the compiler doesn't
/// do that itself (the Xtensa toolchain builds `crc16` as the bitwise loop).
///
/// # Safety
/// `ptr` must be valid for `len` bytes.
#[no_mangle]
#[inline(never)]
pub unsafe extern "C" fn crc16_table(ptr: *const u8, len: usize) -> u32 {
    let data = core::slice::from_raw_parts(ptr, len);
    let mut crc: u16 = 0xFFFF;
    for &b in data {
        crc = (crc << 8) ^ CRC_TABLE[((crc >> 8) as u8 ^ b) as usize];
    }
    crc as u32
}

static CRC_TABLE: [u16; 256] = {
    let mut t = [0u16; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = (i as u16) << 8;
        let mut k = 0;
        while k < 8 {
            c = if c & 0x8000 != 0 {
                (c << 1) ^ 0x1021
            } else {
                c << 1
            };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
};

/// Packet parse. Header `ver:u8 type:u8 len:u16le seq:u32le`, then TLV records
/// `t:u8 l:u8 v[l]` filling `len` bytes, then a checksum byte (sum of the body).
/// Returns a digest of the fields, or `0x8000_0000 | error`.
///
/// # Safety
/// `ptr` must be valid for `len` bytes.
#[no_mangle]
#[inline(never)]
pub unsafe extern "C" fn parse(ptr: *const u8, len: usize) -> u32 {
    let p = core::slice::from_raw_parts(ptr, len);
    match parse_packet(p) {
        Ok(d) => d,
        Err(e) => 0x8000_0000 | e as u32,
    }
}

fn parse_packet(p: &[u8]) -> Result<u32, u8> {
    if p.len() < 8 {
        return Err(1);
    }
    if p[0] != 1 {
        return Err(2);
    }
    let ty = p[1];
    let body_len = u16::from_le_bytes([p[2], p[3]]) as usize;
    let seq = u32::from_le_bytes([p[4], p[5], p[6], p[7]]);
    if 8 + body_len + 1 > p.len() {
        return Err(3);
    }
    let body = &p[8..8 + body_len];
    let mut digest = seq ^ ((ty as u32) << 24);
    let mut count = 0u32;
    let mut i = 0;
    while i < body.len() {
        if i + 2 > body.len() {
            return Err(4);
        }
        let t = body[i];
        let l = body[i + 1] as usize;
        let v = body.get(i + 2..i + 2 + l).ok_or(5)?;
        let mut s = 0u32;
        for &b in v {
            s = s.wrapping_mul(31).wrapping_add(b as u32);
        }
        digest = digest.rotate_left(5) ^ s ^ t as u32;
        count += 1;
        i += 2 + l;
    }
    let sum = body.iter().fold(0u8, |a, &b| a.wrapping_add(b));
    if sum != p[8 + body_len] {
        return Err(6);
    }
    Ok((digest ^ (count << 16)) & 0x7fff_ffff)
}

/// Test inputs shared by every build, so all of them compute the same thing.
pub mod inputs {
    /// 256 bytes of a fixed pseudo-random pattern.
    pub const CRC_DATA: [u8; 256] = {
        let mut d = [0u8; 256];
        let mut x: u32 = 0x1234_5678;
        let mut i = 0;
        while i < 256 {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            d[i] = x as u8;
            i += 1;
        }
        d
    };

    /// A 64-byte packet: 8-byte header, 55-byte body of 6 TLVs, checksum.
    pub const PACKET: [u8; 64] = {
        let mut p = [0u8; 64];
        p[0] = 1; // ver
        p[1] = 0x42; // type
        p[2] = 55; // body len
        p[4] = 0x78;
        p[5] = 0x56;
        p[6] = 0x34;
        p[7] = 0x12; // seq
                     // TLVs: lengths 7, 9, 3, 12, 6, 6 -> 6*2 + 43 = 55 bytes
        let lens = [7u8, 9, 3, 12, 6, 6];
        let mut at = 8;
        let mut k = 0;
        let mut sum: u8 = 0;
        while k < 6 {
            p[at] = 0x10 + k as u8;
            p[at + 1] = lens[k];
            sum = sum.wrapping_add(p[at]).wrapping_add(p[at + 1]);
            let mut j = 0;
            while j < lens[k] as usize {
                let v = (k * 37 + j * 11 + 5) as u8;
                p[at + 2 + j] = v;
                sum = sum.wrapping_add(v);
                j += 1;
            }
            at += 2 + lens[k] as usize;
            k += 1;
        }
        p[at] = sum; // at == 63
        p
    };
}

#[cfg(target_arch = "riscv32")]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    #[test]
    fn crc_check_value() {
        assert_eq!(unsafe { crc16(b"123456789".as_ptr(), 9) }, 0x29B1);
        assert_eq!(unsafe { crc16_table(b"123456789".as_ptr(), 9) }, 0x29B1);
        let d = inputs::CRC_DATA;
        assert_eq!(unsafe { crc16_table(d.as_ptr(), 256) }, unsafe {
            crc16(d.as_ptr(), 256)
        });
    }

    #[test]
    fn packet_parses() {
        let r = unsafe { parse(inputs::PACKET.as_ptr(), 64) };
        assert_eq!(r & 0x8000_0000, 0, "error {r:#x}");
        let mut bad = inputs::PACKET;
        bad[63] ^= 1;
        assert_eq!(unsafe { parse(bad.as_ptr(), 64) }, 0x8000_0006);
        assert_eq!(unsafe { parse(bad.as_ptr(), 20) }, 0x8000_0003);
    }
}
