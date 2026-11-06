#![no_std]
//! The MVM guest/kernel contract (see MESHOPS.md). Guest code and the kernel
//! both build against this crate, so the two sides can't drift apart.

/// FNV-1a 64 over the full versioned name, e.g. `kernel.uart.write.v1`.
///
/// The hash is only an index: the kernel always checks the full name too, so
/// a crafted collision can't shadow an op.
pub const fn fnv1a64(s: &str) -> u64 {
    fnv1a64_bytes(s.as_bytes())
}

pub const fn fnv1a64_bytes(b: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut i = 0;
    while i < b.len() {
        h ^= b[i] as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
        i += 1;
    }
    h
}

/// Names of the ops the kernel provides.
pub mod names {
    pub const UART_WRITE: &str = "kernel.uart.write.v1";
    pub const TIMER_NOW: &str = "kernel.timer.now.v1";
    pub const NOP: &str = "kernel.nop.v1";
    /// `(ptr) -> applet id + 1` of the applet whose static block holds `ptr`, else 0.
    pub const MEM_OWNER: &str = "kernel.mem.owner.v1";
    /// Round 7c: the kernel sums `len` bytes at a guest pointer (proves the
    /// pointer is a real address the kernel can read).
    pub const MEM_SUM: &str = "kernel.mem.sum.v1";
}

/// Only the kernel may register names starting with this.
pub const KERNEL_NAMESPACE: &[u8] = b"kernel.";

/// `gp` points this far into an applet's static block, so the signed 12-bit
/// offsets of rewritten accesses reach the whole block (up to 4 KB). See
/// MEMMODEL.md.
pub const GP_BIAS: u32 = 2048;

/// Round 7c: bulk statics (see BULK.md). The gp window is at most
/// `GP_WINDOW` bytes. Its first words are the **bulk slots**: slot `k` (at gp
/// window offset `4k`) holds the real address `bulk + BULK_PAGE * k +
/// BULK_BIAS`, written by the kernel at load. An access to a bulk static at
/// offset `o` loads slot `o / BULK_PAGE` in place of the `auipc`, and its
/// paired instruction uses `o - BULK_PAGE * k - BULK_BIAS` (always a 12-bit
/// value), so any offset in a bulk segment of any size is reachable without
/// adding instructions.
pub const GP_WINDOW: u32 = 2 * GP_BIAS;
pub const BULK_PAGE: u32 = 4096;
pub const BULK_BIAS: u32 = 2048;
/// Statics this size or larger go to bulk (the chunker's default threshold).
pub const BULK_THRESHOLD: u32 = 256;

/// Number of bulk slots for a bulk segment of `size` bytes (covers every
/// offset in `0..=size`, including one-past-the-end addresses).
pub const fn bulk_slots(size: u32) -> u32 {
    if size == 0 {
        0
    } else {
        size / BULK_PAGE + 1
    }
}

/// The mesh-op call ABI: `ecall` with the import slot in a7, arguments in
/// a0..a5, result in a0 (and a1). Register numbers, x0 = 0.
pub mod call {
    pub const REG_SLOT: usize = 17;
    pub const REG_ARG0: usize = 10;
    pub const NUM_ARGS: usize = 6;
    pub const REG_RET0: usize = 10;
    pub const REG_RET1: usize = 11;
}

/// Manifest tables in an applet image: an 8-byte header (`count: u32`, then
/// 4 reserved bytes) followed by `count` little-endian entries. Name
/// addresses point at the name bytes inside the same image.
pub mod manifest {
    pub const HEADER_SIZE: usize = 8;

    /// Import entry: `{hash: u64, name: u32, len: u32}`. Slot `i` is entry `i`.
    pub mod import {
        pub const SIZE: usize = 16;
        pub const HASH: usize = 0;
        pub const NAME: usize = 8;
        pub const LEN: usize = 12;
    }

    /// Export entry: `{hash: u64, func: u32, name: u32, len: u32, reserved: u32}`.
    pub mod export {
        pub const SIZE: usize = 24;
        pub const HASH: usize = 0;
        pub const FUNC: usize = 8;
        pub const NAME: usize = 12;
        pub const LEN: usize = 16;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv_known_values() {
        // Reference values for FNV-1a 64.
        assert_eq!(fnv1a64(""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64("a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a64("foobar"), 0x8594_4171_f739_67e8);
    }
}
