#![no_std]

static TABLE: [u32; 8] = [3, 1, 4, 1, 5, 9, 2, 6];

/// Constant table lookup. The mask keeps the index in range, so no bounds-check panic.
#[inline(never)]
pub fn table_lookup(i: u32) -> u32 {
    TABLE[(i & 7) as usize]
}

/// String literal: sums its bytes.
#[inline(never)]
pub fn greeting_sum() -> u32 {
    let s: &'static str = core::hint::black_box("hello, mesh");
    let mut acc = 0u32;
    for b in s.bytes() {
        acc = acc.wrapping_add(b as u32);
    }
    acc
}

/// Same literal as greeting_sum, from a second function.
#[inline(never)]
pub fn greeting_len() -> u32 {
    core::hint::black_box("hello, mesh").len() as u32
}

/// Calls another local function and the second crate.
#[inline(never)]
pub fn combine(x: u32) -> u32 {
    let t = table_lookup(x);
    helper::mix(t.wrapping_add(x))
}

/// Entry point for the round-trip test: touches every function.
#[no_mangle]
#[inline(never)]
pub extern "C" fn spike_entry(x: u32) -> u32 {
    let mut acc = 0u32;
    let mut i = 0;
    while i < 10 {
        acc = acc.wrapping_add(combine(x.wrapping_add(i)));
        i += 1;
    }
    acc ^ greeting_sum() ^ (greeting_len() << 16) ^ helper::shared_greeting().len() as u32
}

// ---- Extra cases so every relocation shape shows up ----

/// A static of &str: a data chunk whose words are relocated (R_RISCV_32).
static NAMES: [&str; 3] = ["zero", "one", "two"];

#[inline(never)]
pub fn name_len(i: u32) -> u32 {
    NAMES[(i % 3) as usize].len() as u32
}

/// Dense match with distinct arms, so LLVM emits a jump table.
#[inline(never)]
pub fn classify(x: u32) -> u32 {
    let y = core::hint::black_box(x);
    match x & 7 {
        0 => y.wrapping_mul(3),
        1 => y ^ 0x55,
        2 => y.rotate_left(3),
        3 => y.wrapping_sub(17),
        4 => y.count_ones(),
        5 => y >> 2,
        6 => y.wrapping_add(1000),
        _ => !y,
    }
}

/// C string literals: null-terminated, so they can land in a mergeable string pool.
#[inline(never)]
pub fn cstr_lens() -> u32 {
    let a = core::hint::black_box(c"hello, mesh").to_bytes().len() as u32;
    let b = core::hint::black_box(c"chunk").to_bytes().len() as u32;
    a * 100 + b
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn spike_entry2(x: u32) -> u32 {
    let mut acc = spike_entry(x);
    let mut i = 0;
    while i < 8 {
        acc = acc.wrapping_mul(31).wrapping_add(classify(x.wrapping_add(i)) ^ name_len(i));
        i += 1;
    }
    acc ^ cstr_lens()
}
