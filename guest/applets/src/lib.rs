#![no_std]
//! Mesh-op test applets (round seven). See MESHOPS.md for the ABI.
//!
//! Each applet is a set of `#[no_mangle]` symbols with a prefix:
//! - `<X>_IMPORTS`: the names it calls, by FNV-1a 64 hash. Slot = index.
//! - `<X>_EXPORTS`: the ops it provides (name hash + function).
//! - functions the tests call directly.
//!
//! `chunker link` builds one image per applet from those symbols.

/// FNV-1a 64 over the full versioned name, e.g. `kernel.uart.write.v1`.
pub const fn fnv1a64(s: &str) -> u64 {
    let b = s.as_bytes();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut i = 0;
    while i < b.len() {
        h ^= b[i] as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
        i += 1;
    }
    h
}

/// One import: 16 bytes (name hash, then the full name's address and length,
/// so the kernel can check the hash and the name it resolves to).
#[repr(C)]
pub struct Import {
    pub hash: u64,
    pub name: *const u8,
    pub len: u32,
}

/// Import table: `count`, then the imports. Slot `i` is `entries[i]`.
#[repr(C)]
pub struct Imports<const N: usize> {
    pub count: u32,
    pub _reserved: u32,
    pub entries: [Import; N],
}

// The tables are read-only data; the raw name pointers point at string literals.
unsafe impl<const N: usize> Sync for Imports<N> {}
unsafe impl<const N: usize> Sync for Exports<N> {}

impl<const N: usize> Imports<N> {
    pub const fn new(entries: [Import; N]) -> Self {
        Imports {
            count: N as u32,
            _reserved: 0,
            entries,
        }
    }
}

/// An import of `name` (hash computed at compile time).
pub const fn import(name: &'static str) -> Import {
    Import {
        hash: fnv1a64(name),
        name: name.as_ptr(),
        len: name.len() as u32,
    }
}

/// An op provided by an applet: called with a0..a5, returns a0.
pub type OpFn = extern "C" fn(u32, u32, u32, u32, u32, u32) -> u32;

/// One export: 24 bytes (hash, function address, name address, name length, reserved).
#[repr(C)]
pub struct Export {
    pub hash: u64,
    pub func: OpFn,
    pub name: *const u8,
    pub len: u32,
    pub _reserved: u32,
}

/// An export of `func` under `name`.
pub const fn export(name: &'static str, func: OpFn) -> Export {
    Export {
        hash: fnv1a64(name),
        func,
        name: name.as_ptr(),
        len: name.len() as u32,
        _reserved: 0,
    }
}

#[repr(C)]
pub struct Exports<const N: usize> {
    pub count: u32,
    pub _reserved: u32,
    pub entries: [Export; N],
}

impl<const N: usize> Exports<N> {
    pub const fn new(entries: [Export; N]) -> Self {
        Exports {
            count: N as u32,
            _reserved: 0,
            entries,
        }
    }
}

/// Call the op in import slot `slot` (the ABI: `ecall`, slot in a7,
/// arguments in a0..a5, result in a0 and a1).
#[inline(always)]
pub fn op(slot: u32, a0: u32, a1: u32, a2: u32) -> u32 {
    #[cfg(target_arch = "riscv32")]
    unsafe {
        let r: u32;
        core::arch::asm!(
            "ecall",
            inlateout("a0") a0 => r,
            inlateout("a1") a1 => _,
            in("a2") a2,
            in("a7") slot,
        );
        r
    }
    #[cfg(not(target_arch = "riscv32"))]
    {
        let _ = (slot, a0, a1, a2);
        unreachable!("mesh ops only exist in the guest")
    }
}

pub mod names {
    pub const UART_WRITE: &str = "kernel.uart.write.v1";
    pub const TIMER_NOW: &str = "kernel.timer.now.v1";
    pub const NOP: &str = "kernel.nop.v1";
    pub const ECHO: &str = "test.echo.v1";
    pub const ECHO_LOUD: &str = "test.echo_loud.v1";
    pub const RECURSE: &str = "test.recurse.v1";
    pub const NOBODY: &str = "test.nobody.v1";
}

fn print(slot: u32, s: &[u8]) -> u32 {
    op(slot, s.as_ptr() as u32, s.len() as u32, 0)
}

/// Decimal digits of `v` into `buf`; returns the used tail.
fn dec(mut v: u32, buf: &mut [u8; 10]) -> &[u8] {
    let mut i = 10;
    loop {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    &buf[i..]
}

/// The checksum `test.echo.v1` returns.
#[inline(always)]
pub fn checksum(p: &[u8]) -> u32 {
    let mut s = 0x811c_9dc5u32;
    for &b in p {
        s = (s ^ b as u32).wrapping_mul(0x0100_0193);
    }
    s
}

// ---- H: hello / timer / nop (kernel ops only) ----
mod h {
    pub const UART: u32 = 0;
    pub const NOW: u32 = 1;
    pub const NOP: u32 = 2;
}

#[no_mangle]
pub static H_IMPORTS: Imports<3> = Imports::new([
    import(names::UART_WRITE),
    import(names::TIMER_NOW),
    import(names::NOP),
]);

/// Test 1: print a greeting; returns the op's result (bytes written).
#[no_mangle]
pub extern "C" fn h_hello() -> u32 {
    print(h::UART, b"hello from guest applet H\n")
}

/// Test 2: read the timer around a loop of `n` iterations, print and return the delta.
#[no_mangle]
pub extern "C" fn h_timer(n: u32) -> u32 {
    let t0 = op(h::NOW, 0, 0, 0);
    let mut acc = 0u32;
    for i in 0..n {
        acc = core::hint::black_box(acc.wrapping_mul(31).wrapping_add(i));
    }
    let t1 = op(h::NOW, 0, 0, 0);
    let d = t1.wrapping_sub(t0);
    let mut b = [0u8; 10];
    print(h::UART, b"timer delta: ");
    print(h::UART, dec(d, &mut b));
    print(h::UART, b"\n");
    core::hint::black_box(acc);
    d
}

/// Test 3: `n` calls of kernel.nop.v1.
#[no_mangle]
pub extern "C" fn h_nop_loop(n: u32) -> u32 {
    let mut acc = 0u32;
    for _ in 0..n {
        acc = acc.wrapping_add(op(h::NOP, 0, 0, 0));
    }
    acc
}

/// Baseline for test 3: the same loop without the call.
#[no_mangle]
pub extern "C" fn h_empty_loop(n: u32) -> u32 {
    let mut acc = 0u32;
    for _ in 0..n {
        acc = acc.wrapping_add(core::hint::black_box(0));
    }
    acc
}

/// `n` calls of kernel.uart.write.v1 with a fixed 16-byte message.
#[no_mangle]
pub extern "C" fn h_uart_loop(n: u32) -> u32 {
    let mut acc = 0u32;
    for _ in 0..n {
        acc = acc.wrapping_add(print(h::UART, b"0123456789abcdef"));
    }
    acc
}

// ---- A: provides test.echo.v1, test.echo_loud.v1, test.recurse.v1 ----
mod a {
    pub const UART: u32 = 0;
    pub const RECURSE: u32 = 1;
}

#[no_mangle]
pub static A_IMPORTS: Imports<2> =
    Imports::new([import(names::UART_WRITE), import(names::RECURSE)]);

#[no_mangle]
pub static A_EXPORTS: Exports<3> = Exports::new([
    export(names::ECHO, a_echo),
    export(names::ECHO_LOUD, a_echo_loud),
    export(names::RECURSE, a_recurse),
]);

/// test.echo.v1(ptr, len) -> checksum of the caller's buffer.
#[no_mangle]
pub extern "C" fn a_echo(ptr: u32, len: u32, _: u32, _: u32, _: u32, _: u32) -> u32 {
    let p = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
    checksum(p)
}

/// test.echo_loud.v1(ptr, len): prints through a kernel op (nested), then echoes.
#[no_mangle]
pub extern "C" fn a_echo_loud(ptr: u32, len: u32, _: u32, _: u32, _: u32, _: u32) -> u32 {
    let mut b = [0u8; 10];
    print(a::UART, b"A echo_loud: ");
    print(a::UART, dec(len, &mut b));
    print(a::UART, b" bytes\n");
    a_echo(ptr, len, 0, 0, 0, 0)
}

/// test.recurse.v1(n): calls itself through the kernel n times; returns n.
#[no_mangle]
pub extern "C" fn a_recurse(n: u32, _: u32, _: u32, _: u32, _: u32, _: u32) -> u32 {
    if n == 0 {
        0
    } else {
        1 + op(a::RECURSE, n - 1, 0, 0)
    }
}

// ---- A2: a second provider of test.echo.v1 (for the swap test) ----
#[no_mangle]
pub static A2_IMPORTS: Imports<0> = Imports::new([]);

#[no_mangle]
pub static A2_EXPORTS: Exports<1> = Exports::new([export(names::ECHO, a2_echo)]);

/// Same checksum, tagged so callers can tell which provider answered.
#[no_mangle]
pub extern "C" fn a2_echo(ptr: u32, len: u32, _: u32, _: u32, _: u32, _: u32) -> u32 {
    let p = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
    checksum(p) ^ 0xA2A2_0000
}

// ---- B: calls A's ops with buffers it owns (on its stack) ----
mod b {
    pub const ECHO: u32 = 0;
    pub const ECHO_LOUD: u32 = 1;
    pub const RECURSE: u32 = 2;
}

#[no_mangle]
pub static B_IMPORTS: Imports<3> = Imports::new([
    import(names::ECHO),
    import(names::ECHO_LOUD),
    import(names::RECURSE),
]);

fn b_buf() -> [u8; 64] {
    let mut buf = [0u8; 64];
    for (i, v) in buf.iter_mut().enumerate() {
        *v = (i as u8).wrapping_mul(7).wrapping_add(3);
    }
    buf
}

/// Test 4: one echo of a 64-byte buffer B owns.
#[no_mangle]
pub extern "C" fn b_echo_once() -> u32 {
    let buf = core::hint::black_box(b_buf());
    op(b::ECHO, buf.as_ptr() as u32, buf.len() as u32, 0)
}

/// Test 4 cost: `n` echo calls over the first `len` bytes of B's buffer.
#[no_mangle]
pub extern "C" fn b_echo_loop(n: u32, len: u32) -> u32 {
    let buf = core::hint::black_box(b_buf());
    let mut acc = 0u32;
    for _ in 0..n {
        acc ^= op(b::ECHO, buf.as_ptr() as u32, len, 0);
    }
    acc
}

/// Baseline for the echo cost: the same loop doing the checksum locally.
#[no_mangle]
pub extern "C" fn b_local_loop(n: u32, len: u32) -> u32 {
    let buf = core::hint::black_box(b_buf());
    let mut acc = 0u32;
    for _ in 0..n {
        acc ^= checksum(core::hint::black_box(&buf[..len as usize]));
    }
    acc
}

/// Test 5: nested (A's op prints through the kernel).
#[no_mangle]
pub extern "C" fn b_loud() -> u32 {
    let buf = core::hint::black_box(b_buf());
    op(b::ECHO_LOUD, buf.as_ptr() as u32, buf.len() as u32, 0)
}

/// Test 5: recursion depth `n` through A's op.
#[no_mangle]
pub extern "C" fn b_recurse(n: u32) -> u32 {
    op(b::RECURSE, n, 0, 0)
}

// ---- M: imports a name nobody provides ----
#[no_mangle]
pub static M_IMPORTS: Imports<2> = Imports::new([import(names::UART_WRITE), import(names::NOBODY)]);

#[no_mangle]
pub extern "C" fn m_main() -> u32 {
    op(1, 0, 0, 0)
}

// ---- S1-S3: spoofing attempts the kernel must reject at load ----

/// S1 tries to provide a kernel op. Only the kernel may register `kernel.*`.
#[no_mangle]
pub static S1_IMPORTS: Imports<0> = Imports::new([]);
#[no_mangle]
pub static S1_EXPORTS: Exports<1> = Exports::new([export(names::NOP, a2_echo)]);

/// S2 claims test.echo.v1's hash under another name: a forged collision.
/// (A crafted real FNV collision is caught too: the registry keeps the full
/// name and rejects a second name for the same hash.)
#[no_mangle]
pub static S2_IMPORTS: Imports<0> = Imports::new([]);
#[no_mangle]
pub static S2_EXPORTS: Exports<1> = Exports::new([Export {
    hash: fnv1a64(names::ECHO),
    func: a2_echo,
    name: "evil.echo.v1".as_ptr(),
    len: 12,
    _reserved: 0,
}]);

/// S3 imports with the same forged pair.
#[no_mangle]
pub static S3_IMPORTS: Imports<1> = Imports::new([Import {
    hash: fnv1a64(names::ECHO),
    name: "evil.echo.v1".as_ptr(),
    len: 12,
}]);

#[cfg(target_arch = "riscv32")]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
