//! Round 7c applets: bulk statics.
//!
//! Every writable static in this file is an ordinary Rust `static mut` with
//! no attribute: no `#[link_section]`, no `#[no_mangle]`, nothing that says
//! where it should live. The chunker places each one (gp window or bulk) by
//! size. `meshops/build_applets.py` checks this file stays that way (test 7).
//! The only attributes here are on functions (entry points) and on the import
//! tables (which are the applets' interface, read by the build tool, and live
//! in flash).

use crate::{import, op, Imports};
use core::ptr::{addr_of, addr_of_mut};
use mvm_abi::names;

/// FNV-1a 32, the same checksum as kernel.mem.sum.v1 and the tests.
fn fnv(p: &[u8]) -> u32 {
    let mut s = 0x811c_9dc5u32;
    for &b in p {
        s = (s ^ b as u32).wrapping_mul(0x0100_0193);
    }
    s
}

// ---- D: a 16 KB buffer plus small counters ----
mod d {
    pub const SUM: u32 = 0;
    pub const OWNER: u32 = 1;
}

#[no_mangle]
pub static D_IMPORTS: Imports<2> = Imports::new([import(names::MEM_SUM), import(names::MEM_OWNER)]);

/// 16 KB of zeros (.bss): above the threshold, so bulk.
static mut WORDS: [u32; 4096] = [0; 4096];
/// 384 B of initialised data: bulk too, so bulk init data is exercised.
static mut SQUARES: [u32; 96] = squares();
/// 256 B of zeros, laid out after `WORDS` (bulk offset > 16 KB), so its
/// accesses go through a later bulk slot.
static mut TAIL: [u32; 64] = [0; 64];
/// Small: gp window.
static mut FILLS: u32 = 0;
static mut SEED: u32 = 0x1234_5678;
/// A pointer into bulk, stored in the gp window (a load-time data reloc gp -> bulk).
static mut WORDS_PTR: *mut u32 = addr_of_mut!(WORDS) as *mut u32;
/// 64 pointers back into the gp window, stored in bulk (256 B: data relocs bulk -> gp).
static mut FILLS_PTRS: [*const u32; 64] = [addr_of!(FILLS); 64];
/// Bumped by the gp access-cost loop.
static mut TOUCHES: u32 = 0;

const fn squares() -> [u32; 96] {
    let mut t = [0u32; 96];
    let mut i = 0;
    while i < 96 {
        t[i] = (i as u32) * (i as u32) * 7 + 1;
        i += 1;
    }
    t
}

/// Fill the buffer from a xorshift32 stream seeded with `seed ^ SEED`;
/// returns how many fills this instance has done.
#[no_mangle]
pub extern "C" fn d_fill(seed: u32) -> u32 {
    unsafe {
        let mut x = seed ^ *addr_of!(SEED);
        for w in (*addr_of_mut!(WORDS)).iter_mut() {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            *w = x;
        }
        *addr_of_mut!(FILLS) += 1;
        *addr_of!(FILLS)
    }
}

/// FNV-1a 32 of the whole buffer, computed by the guest.
#[no_mangle]
pub extern "C" fn d_sum() -> u32 {
    unsafe {
        let w = &*addr_of!(WORDS);
        fnv(core::slice::from_raw_parts(w.as_ptr() as *const u8, 16384))
    }
}

/// The same checksum, computed by the kernel through a pointer to the buffer.
#[no_mangle]
pub extern "C" fn d_kernel_sum() -> u32 {
    op(d::SUM, addr_of!(WORDS) as u32, 16384, 0)
}

/// The guest address of `WORDS[i]`.
#[no_mangle]
pub extern "C" fn d_addr(i: u32) -> u32 {
    unsafe { (*addr_of!(WORDS)).as_ptr().add(i as usize) as u32 }
}

/// Which applet the kernel says owns `WORDS[i]`.
#[no_mangle]
pub extern "C" fn d_owner(i: u32) -> u32 {
    op(d::OWNER, d_addr(i), 0, 0)
}

/// Sum of `SQUARES` (checks bulk `.data` initialisation).
#[no_mangle]
pub extern "C" fn d_squares_sum() -> u32 {
    unsafe { (*addr_of!(SQUARES)).iter().fold(0u32, |a, &v| a.wrapping_add(v)) }
}

/// Add `v` to `SQUARES[i]` and set `SEED`. (Without a writer, LLVM turns a
/// never-written `static mut` into read-only data in flash.)
#[no_mangle]
pub extern "C" fn d_bump(i: u32, v: u32) -> u32 {
    unsafe {
        let s = &mut *addr_of_mut!(SQUARES);
        s[i as usize % 96] = s[i as usize % 96].wrapping_add(v);
        *addr_of_mut!(SEED) ^= v;
        s[i as usize % 96]
    }
}

/// Store `v` in `TAIL[i % 64]`, return the sum of `TAIL`.
#[no_mangle]
pub extern "C" fn d_tail(i: u32, v: u32) -> u32 {
    unsafe {
        let t = &mut *addr_of_mut!(TAIL);
        t[i as usize % 64] = v;
        t.iter().fold(0u32, |a, &x| a.wrapping_add(x))
    }
}

/// Pointers stored in statics: 1 if `WORDS_PTR` is `&WORDS[0]` and every
/// `FILLS_PTRS[i]` is `&FILLS`, and both read the right values.
#[no_mangle]
pub extern "C" fn d_ptrs_ok() -> u32 {
    unsafe {
        let w = *addr_of!(WORDS_PTR);
        let ok_w = w == addr_of_mut!(WORDS) as *mut u32 && *w == (*addr_of!(WORDS))[0];
        let ok_f = (*addr_of!(FILLS_PTRS))
            .iter()
            .all(|&p| p == addr_of!(FILLS) && *p == *addr_of!(FILLS));
        (ok_w && ok_f) as u32
    }
}

/// Point `WORDS_PTR` at `WORDS[i]` (its writer, so it stays a writable static)
/// and return what it points at.
#[no_mangle]
pub extern "C" fn d_ptr_set(i: u32) -> u32 {
    unsafe {
        let p = (addr_of_mut!(WORDS) as *mut u32).add(i as usize % 4096);
        *addr_of_mut!(WORDS_PTR) = p;
        *p
    }
}

/// Constant-index accesses far into the buffer (offsets that cross bulk
/// pages): store `v`, `v + 1`, `v + 2` at words 1023, 1024 and 4095.
#[no_mangle]
pub extern "C" fn d_corners_set(v: u32) {
    unsafe {
        let w = addr_of_mut!(WORDS);
        (*w)[1023] = v;
        (*w)[1024] = v + 1;
        (*w)[4095] = v + 2;
    }
}

/// Read them back (a separate function, so the compiler can't fold the stores).
#[no_mangle]
pub extern "C" fn d_corners_get() -> u32 {
    unsafe {
        let w = addr_of!(WORDS);
        (*w)[1023] ^ ((*w)[1024] << 8) ^ ((*w)[4095] << 16)
    }
}

// Access cost: each loop calls a one-access function `n` times. The
// functions are `inline(never)` so every call redoes the address sequence.
#[inline(never)]
fn touch_gp() {
    unsafe {
        let p = addr_of_mut!(TOUCHES);
        p.write_volatile(p.read_volatile().wrapping_add(1));
    }
}

#[inline(never)]
fn touch_bulk() {
    unsafe {
        let p = addr_of_mut!(WORDS) as *mut u32;
        p.write_volatile(p.read_volatile().wrapping_add(1));
    }
}

/// The baseline: the same call and return, no access. (An empty `asm!`
/// counts as a side effect, so the call isn't optimised away; it emits no
/// instruction.)
#[inline(never)]
fn touch_none() {
    #[cfg(target_arch = "riscv32")]
    unsafe {
        core::arch::asm!("", options(nostack))
    }
}

#[no_mangle]
pub extern "C" fn d_gp_loop(n: u32) -> u32 {
    for _ in 0..n {
        touch_gp();
    }
    n
}

#[no_mangle]
pub extern "C" fn d_bulk_loop(n: u32) -> u32 {
    for _ in 0..n {
        touch_bulk();
    }
    n
}

#[no_mangle]
pub extern "C" fn d_none_loop(n: u32) -> u32 {
    for _ in 0..n {
        touch_none();
    }
    n
}

// ---- E: forty 128-byte statics (5 KB in all), each below the threshold ----
// Together they overflow the 4 KB gp window, so the chunker spills the
// largest to bulk until the rest fit. Even ones are zeros (.bss), odd ones
// start at their index (.data).
#[no_mangle]
pub static E_IMPORTS: Imports<0> = Imports::new([]);

static mut S00: [u32; 32] = [0; 32];
static mut S01: [u32; 32] = [1; 32];
static mut S02: [u32; 32] = [0; 32];
static mut S03: [u32; 32] = [3; 32];
static mut S04: [u32; 32] = [0; 32];
static mut S05: [u32; 32] = [5; 32];
static mut S06: [u32; 32] = [0; 32];
static mut S07: [u32; 32] = [7; 32];
static mut S08: [u32; 32] = [0; 32];
static mut S09: [u32; 32] = [9; 32];
static mut S10: [u32; 32] = [0; 32];
static mut S11: [u32; 32] = [11; 32];
static mut S12: [u32; 32] = [0; 32];
static mut S13: [u32; 32] = [13; 32];
static mut S14: [u32; 32] = [0; 32];
static mut S15: [u32; 32] = [15; 32];
static mut S16: [u32; 32] = [0; 32];
static mut S17: [u32; 32] = [17; 32];
static mut S18: [u32; 32] = [0; 32];
static mut S19: [u32; 32] = [19; 32];
static mut S20: [u32; 32] = [0; 32];
static mut S21: [u32; 32] = [21; 32];
static mut S22: [u32; 32] = [0; 32];
static mut S23: [u32; 32] = [23; 32];
static mut S24: [u32; 32] = [0; 32];
static mut S25: [u32; 32] = [25; 32];
static mut S26: [u32; 32] = [0; 32];
static mut S27: [u32; 32] = [27; 32];
static mut S28: [u32; 32] = [0; 32];
static mut S29: [u32; 32] = [29; 32];
static mut S30: [u32; 32] = [0; 32];
static mut S31: [u32; 32] = [31; 32];
static mut S32: [u32; 32] = [0; 32];
static mut S33: [u32; 32] = [33; 32];
static mut S34: [u32; 32] = [0; 32];
static mut S35: [u32; 32] = [35; 32];
static mut S36: [u32; 32] = [0; 32];
static mut S37: [u32; 32] = [37; 32];
static mut S38: [u32; 32] = [0; 32];
static mut S39: [u32; 32] = [39; 32];
static mut RUNS: u32 = 0;

macro_rules! each_static {
    ($m:ident) => {
        $m!(S00, 0); $m!(S01, 1); $m!(S02, 2); $m!(S03, 3); $m!(S04, 4); $m!(S05, 5); $m!(S06, 6); $m!(S07, 7); $m!(S08, 8); $m!(S09, 9); $m!(S10, 10); $m!(S11, 11); $m!(S12, 12); $m!(S13, 13); $m!(S14, 14); $m!(S15, 15); $m!(S16, 16); $m!(S17, 17); $m!(S18, 18); $m!(S19, 19); $m!(S20, 20); $m!(S21, 21); $m!(S22, 22); $m!(S23, 23); $m!(S24, 24); $m!(S25, 25); $m!(S26, 26); $m!(S27, 27); $m!(S28, 28); $m!(S29, 29); $m!(S30, 30); $m!(S31, 31); $m!(S32, 32); $m!(S33, 33); $m!(S34, 34); $m!(S35, 35); $m!(S36, 36); $m!(S37, 37); $m!(S38, 38); $m!(S39, 39);
    };
}

#[inline(never)]
fn sum_one(v: &[u32; 32]) -> u32 {
    v.iter().fold(0u32, |a, &x| a.wrapping_add(x))
}

#[inline(never)]
fn fill_one(v: &mut [u32; 32], base: u32) {
    for (i, x) in v.iter_mut().enumerate() {
        *x = base.wrapping_add(i as u32);
    }
}

/// Sum of every word of every static (wrapping).
#[no_mangle]
pub extern "C" fn e_sum() -> u32 {
    let mut s = 0u32;
    macro_rules! add {
        ($v:ident, $k:expr) => {
            s = s.wrapping_add(sum_one(unsafe { &*addr_of!($v) }));
        };
    }
    each_static!(add);
    s
}

/// Write `seed + 32k + i` to word `i` of static `k`; returns the run count.
#[no_mangle]
pub extern "C" fn e_fill(seed: u32) -> u32 {
    macro_rules! fill {
        ($v:ident, $k:expr) => {
            fill_one(unsafe { &mut *addr_of_mut!($v) }, seed.wrapping_add($k * 32));
        };
    }
    each_static!(fill);
    unsafe {
        *addr_of_mut!(RUNS) += 1;
        *addr_of!(RUNS)
    }
}
