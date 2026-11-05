#![no_std]

/// Mixes a value. Kept out of line so testcrate has a real cross-crate call.
#[inline(never)]
pub fn mix(x: u32) -> u32 {
    x.rotate_left(5) ^ 0x9e37_79b9
}

/// Uses the same literal as testcrate::greeting_len, in a different crate.
#[inline(never)]
pub fn shared_greeting() -> &'static str {
    "hello, mesh"
}
