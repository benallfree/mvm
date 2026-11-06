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
}

/// Only the kernel may register names starting with this.
pub const KERNEL_NAMESPACE: &[u8] = b"kernel.";

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
