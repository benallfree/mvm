#![no_std]
//! Round-seven mesh-op tests, shared by every target. A target supplies an
//! output sink, a monotonic tick source (for kernel.timer.now.v1) and a cycle
//! counter; this loads the applets and runs tests 1-7 plus the cost sections.
//!
//! Lines: `MO <chip> <test> pass=<0|1> <details>` and
//! `MO <chip> cost <section> n=<N> cycles=<C> [steps=<S>]`, then `MO <chip> done pass=<0|1>`.
//! The Unicorn harness ignores `cycles=` and times each section itself from
//! the two counter reads around it (the order is fixed: see `SECTIONS`).

use core::fmt::Write;
use core::ptr::addr_of_mut;
use interp::fast;
use mvm_kernel::{fnv1a64, Console, Error, Kernel, MAX_DEPTH};

#[allow(dead_code)]
mod img {
    include!("../../guest-image/applets.rs");
}

/// The measured sections, in the order they run (each brackets one counter pair).
pub const SECTIONS: [&str; 11] = [
    "load_all",
    "empty_loop",
    "nop_loop",
    "uart_loop_muted",
    "local_loop_len0",
    "echo_loop_len0",
    "local_loop_len64",
    "echo_loop_len64",
    "recurse_depth1",
    "recurse_depth6",
    "call_overhead",
];

/// Dev-side name table: the kernel only knows hashes.
pub const NAMES: [&str; 7] = [
    "kernel.uart.write.v1",
    "kernel.timer.now.v1",
    "kernel.nop.v1",
    "test.echo.v1",
    "test.echo_loud.v1",
    "test.recurse.v1",
    "test.nobody.v1",
];

/// `Kind:name` for a load error (names from the dev table).
fn why(e: Option<Error>) -> Line {
    let mut l = Line::default();
    let _ = match e {
        Some(Error::ReservedNamespace(h)) => write!(l, "ReservedNamespace:{}", name_of(h)),
        Some(Error::NameHashMismatch(h)) => write!(l, "NameHashMismatch:{}", name_of(h)),
        Some(Error::HashCollision(h)) => write!(l, "HashCollision:{}", name_of(h)),
        Some(Error::NameMismatch(h)) => write!(l, "NameMismatch:{}", name_of(h)),
        Some(e) => write!(l, "{e:?}"),
        None => write!(l, "accepted"),
    };
    l
}

pub fn name_of(h: u64) -> &'static str {
    NAMES
        .iter()
        .copied()
        .find(|n| fnv1a64(n) == h)
        .unwrap_or("?")
}

/// Same checksum as the applets' test.echo.v1.
pub fn checksum(p: &[u8]) -> u32 {
    let mut s = 0x811c_9dc5u32;
    for &b in p {
        s = (s ^ b as u32).wrapping_mul(0x0100_0193);
    }
    s
}

fn b_buf() -> [u8; 64] {
    let mut buf = [0u8; 64];
    for (i, v) in buf.iter_mut().enumerate() {
        *v = (i as u8).wrapping_mul(7).wrapping_add(3);
    }
    buf
}

/// Forwards guest output to the target and keeps a copy for checking. The
/// copy lives in statics because the kernel holds the console as `dyn Console`.
struct Tee<'a, W: Write> {
    out: &'a mut W,
}

static mut CAP: [u8; 512] = [0; 512];
static mut CAP_LEN: usize = 0;
static mut MUTED: bool = false;

/// Take (and clear) what the guest printed since the last take, as a copy.
fn cap_take() -> Line {
    let mut l = Line::default();
    unsafe {
        let cap: &[u8; 512] = &*addr_of_mut!(CAP);
        let n = CAP_LEN.min(l.buf.len());
        l.buf[..n].copy_from_slice(&cap[..n]);
        l.len = n;
        CAP_LEN = 0;
    }
    l
}
fn mute(m: bool) {
    unsafe { MUTED = m }
}

impl<W: Write> Console for Tee<'_, W> {
    fn write(&mut self, bytes: &[u8]) {
        unsafe {
            if MUTED {
                return;
            }
            if let Ok(s) = core::str::from_utf8(bytes) {
                let _ = self.out.write_str(s);
            }
            let cap = &mut *addr_of_mut!(CAP);
            let n = bytes.len().min(cap.len() - CAP_LEN);
            cap[CAP_LEN..CAP_LEN + n].copy_from_slice(&bytes[..n]);
            CAP_LEN += n;
        }
    }
}

const RAM_BASE: u32 = 0x8000_0000;
const RAM_SIZE: usize = 24 * 1024;
static mut RAM: [u8; RAM_SIZE] = [0; RAM_SIZE];
static mut TABLE: [fast::E; img::BLOB_LEN / 2] = [fast::E::EMPTY; img::BLOB_LEN / 2];

const H: u8 = 0;
const A: u8 = 1;
const B: u8 = 2;
const A2: u8 = 3;
const B_AGAIN: u8 = 4;
const M: u8 = 5;

/// Default `n` for the call-cost loops (Unicorn uses less: it hooks every instruction).
pub const N_CALLS: u32 = 10_000;
const RECURSE_REPS: u32 = 200;

/// Run everything. `now` backs kernel.timer.now.v1; `cycles` times sections.
pub fn run_all<W: Write>(
    chip: &str,
    out: &mut W,
    now: fn() -> u32,
    cycles: fn() -> u32,
    n_calls: u32,
) -> bool {
    #[allow(non_snake_case)]
    let N = n_calls;
    let mut all = true;
    macro_rules! line {
        ($($t:tt)*) => {{ let _ = writeln!(out, $($t)*); }};
    }
    // The kernel borrows `out` through the console, so results go to a
    // buffer of lines and are flushed at the end.
    let mut report = Lines::default();
    let mut tee = Tee { out };
    cap_take();
    mute(false);
    let (ram, table) = unsafe { (&mut *addr_of_mut!(RAM), &mut *addr_of_mut!(TABLE)) };

    let mut k = match Kernel::new(
        &img::BLOB,
        img::BLOB_BASE,
        ram,
        RAM_BASE,
        table,
        &mut tee,
        now,
    ) {
        Ok(k) => k,
        Err(e) => {
            let _ = writeln!(tee.out, "MO {chip} init pass=0 {e:?}");
            return false;
        }
    };
    #[cfg(feature = "count")]
    let steps = |k: &mut Kernel| core::mem::replace(&mut k.steps, 0);
    #[cfg(not(feature = "count"))]
    let steps = |_: &mut Kernel| 0u64;
    let cost = |r: &mut Lines, name: &str, n: u32, c: u32, s: u64| {
        r.push(format_args!(
            "MO {chip} cost {name} n={n} cycles={c} steps={s}"
        ));
    };

    // Load H, A, B (the order matters only for exports-before-imports).
    let c0 = cycles();
    let loads = [
        k.load(H, img::h::IMPORTS, img::h::EXPORTS),
        k.load(A, img::a::IMPORTS, img::a::EXPORTS),
        k.load(B, img::b::IMPORTS, img::b::EXPORTS),
    ];
    let c1 = cycles();
    let loaded = loads.iter().all(|l| l.is_ok());
    all &= loaded;
    report.push(format_args!(
        "MO {chip} load pass={} applets=3 registry_entries={} entry_bytes={} applet_bytes={} registry_bytes={}",
        loaded as u8,
        k.registry.len(),
        core::mem::size_of::<mvm_kernel::Entry>(),
        core::mem::size_of::<mvm_kernel::Applet>(),
        core::mem::size_of::<mvm_kernel::Registry>()
    ));
    cost(&mut report, "load_all", 3, c1.wrapping_sub(c0), 0);

    // 1. Hello.
    let r = k.call(H, img::h::HELLO, &[]);
    let want = b"hello from guest applet H\n";
    let got = cap_take();
    let ok = r == Ok(want.len() as u32) && got.as_str().as_bytes() == want;
    all &= ok;
    report.push(format_args!(
        "MO {chip} 1-hello pass={} ret={r:?}",
        ok as u8
    ));

    // 2. Timer.
    let r = k.call(H, img::h::TIMER, &[2000]);
    let printed = cap_take();
    let ok = match r {
        Ok(d) => d > 0 && printed.as_str().trim_end() == fmt_delta(d).as_str(),
        Err(_) => false,
    };
    all &= ok;
    report.push(format_args!(
        "MO {chip} 2-timer pass={} delta={r:?}",
        ok as u8
    ));

    // 3. Bare call cost (and baselines). Guest output muted for the uart loop.
    let _ = steps(&mut k);
    let c0 = cycles();
    let e = k.call(H, img::h::EMPTY_LOOP, &[N]);
    let c1 = cycles();
    cost(
        &mut report,
        "empty_loop",
        N,
        c1.wrapping_sub(c0),
        steps(&mut k),
    );
    let c0 = cycles();
    let n = k.call(H, img::h::NOP_LOOP, &[N]);
    let c1 = cycles();
    cost(
        &mut report,
        "nop_loop",
        N,
        c1.wrapping_sub(c0),
        steps(&mut k),
    );
    mute(true);
    let c0 = cycles();
    let u = k.call(H, img::h::UART_LOOP, &[N]);
    let c1 = cycles();
    mute(false);
    cost(
        &mut report,
        "uart_loop_muted",
        N,
        c1.wrapping_sub(c0),
        steps(&mut k),
    );
    let ok = e == Ok(0) && n == Ok(0) && u == Ok(16 * N);
    all &= ok;
    report.push(format_args!(
        "MO {chip} 3-nop-cost pass={} calls={N}",
        ok as u8
    ));

    // 4. Applet-provided op: B calls A's test.echo.v1 with its own buffer.
    let buf = b_buf();
    let cs = checksum(&buf);
    let r = k.call(B, img::b::ECHO_ONCE, &[]);
    let mut ok = r == Ok(cs);
    for len in [0u32, 64] {
        let name_l = if len == 0 {
            "local_loop_len0"
        } else {
            "local_loop_len64"
        };
        let name_e = if len == 0 {
            "echo_loop_len0"
        } else {
            "echo_loop_len64"
        };
        let c0 = cycles();
        let l = k.call(B, img::b::LOCAL_LOOP, &[N, len]);
        let c1 = cycles();
        cost(&mut report, name_l, N, c1.wrapping_sub(c0), steps(&mut k));
        let c0 = cycles();
        let x = k.call(B, img::b::ECHO_LOOP, &[N, len]);
        let c1 = cycles();
        cost(&mut report, name_e, N, c1.wrapping_sub(c0), steps(&mut k));
        ok &= l.is_ok() && l == x;
    }
    all &= ok;
    report.push(format_args!(
        "MO {chip} 4-applet-op pass={} echo={r:?} want={cs:#x}",
        ok as u8
    ));

    // 5. Nested: A's op prints through the kernel; recursion through A's op.
    let r = k.call(B, img::b::LOUD, &[]);
    let printed = cap_take();
    let loud_ok = r == Ok(cs) && printed.as_str() == "A echo_loud: 64 bytes\n";
    k.max_depth_seen = 0;
    let deepest_ok = MAX_DEPTH as u32 - 2; // B is level 0; A(n) .. A(0) take n+1 more
    let r_ok = k.call(B, img::b::RECURSE, &[deepest_ok]);
    let seen = k.max_depth_seen;
    let r_bad = k.call(B, img::b::RECURSE, &[deepest_ok + 1]);
    let after = k.call(B, img::b::ECHO_ONCE, &[]);
    let ok = loud_ok
        && r_ok == Ok(deepest_ok)
        && seen == MAX_DEPTH - 1
        && r_bad == Err(Error::DepthExceeded(MAX_DEPTH))
        && after == Ok(cs);
    all &= ok;
    report.push(format_args!(
        "MO {chip} 5-nested pass={} loud={r:?} recurse({deepest_ok})={r_ok:?} max_depth={seen} recurse({})={r_bad:?} after={after:?}",
        ok as u8,
        deepest_ok + 1
    ));
    let _ = steps(&mut k);
    for (name, d) in [("recurse_depth1", 1u32), ("recurse_depth6", 6)] {
        let c0 = cycles();
        for _ in 0..RECURSE_REPS {
            let _ = k.call(B, img::b::RECURSE, &[d]);
        }
        let c1 = cycles();
        cost(
            &mut report,
            name,
            RECURSE_REPS,
            c1.wrapping_sub(c0),
            steps(&mut k),
        );
    }
    // Host -> guest call overhead (Kernel::call of a trivial entry), for context.
    let c0 = cycles();
    for _ in 0..RECURSE_REPS {
        let _ = k.call(H, img::h::EMPTY_LOOP, &[0]);
    }
    let c1 = cycles();
    cost(
        &mut report,
        "call_overhead",
        RECURSE_REPS,
        c1.wrapping_sub(c0),
        steps(&mut k),
    );

    // 6. Missing import: the load fails, names the import, and changes nothing.
    let before = k.registry.len();
    let r = k.load(M, img::m::IMPORTS, img::m::EXPORTS);
    let ok = matches!(r, Err(Error::Unresolved(h)) if h == fnv1a64("test.nobody.v1"))
        && k.registry.len() == before
        && k.applets[M as usize].is_none();
    all &= ok;
    let missing = match r {
        Err(Error::Unresolved(h)) => name_of(h),
        _ => "-",
    };
    report.push(format_args!(
        "MO {chip} 6-missing-import pass={} error=unresolved:{missing}",
        ok as u8
    ));

    // 7. Swap provider: A2 registers test.echo.v1 after B resolved it.
    let before = k.call(B, img::b::ECHO_ONCE, &[]);
    let l = k.load(A2, img::a2::IMPORTS, img::a2::EXPORTS);
    let old_binding = k.call(B, img::b::ECHO_ONCE, &[]);
    let l2 = k.load(B_AGAIN, img::b::IMPORTS, img::b::EXPORTS);
    let new_binding = k.call(B_AGAIN, img::b::ECHO_ONCE, &[]);
    let tagged = cs ^ 0xA2A2_0000;
    let ok = before == Ok(cs)
        && l.is_ok()
        && l2.is_ok()
        && old_binding == Ok(tagged)
        && new_binding == Ok(tagged);
    all &= ok;
    report.push(format_args!(
        "MO {chip} 7-swap pass={} before={before:?} loaded-before-swap={old_binding:?} loaded-after-swap={new_binding:?}",
        ok as u8
    ));

    // 8. Spoofing: a kernel name, and a forged hash/name pair on each side.
    let before = k.registry.len();
    let s1 = k.load(M, img::s1::IMPORTS, img::s1::EXPORTS);
    let s2 = k.load(M, img::s2::IMPORTS, img::s2::EXPORTS);
    let s3 = k.load(M, img::s3::IMPORTS, img::s3::EXPORTS);
    let echo = fnv1a64("test.echo.v1");
    let still = k.call(B, img::b::ECHO_ONCE, &[]);
    let ok = matches!(s1, Err(Error::ReservedNamespace(h)) if h == fnv1a64("kernel.nop.v1"))
        && matches!(s2, Err(Error::NameHashMismatch(h)) if h == echo)
        && matches!(s3, Err(Error::NameHashMismatch(h)) if h == echo)
        && k.registry.len() == before
        && k.applets[M as usize].is_none()
        && still == Ok(cs ^ 0xA2A2_0000);
    all &= ok;
    report.push(format_args!(
        "MO {chip} 8-spoof pass={} kernel-name={} forged-export={} forged-import={}",
        ok as u8,
        why(s1.err()).as_str(),
        why(s2.err()).as_str(),
        why(s3.err()).as_str()
    ));

    for l in report.lines() {
        let _ = writeln!(tee.out, "{l}");
    }
    line!("MO {chip} done pass={}", all as u8);
    all
}

fn fmt_delta(d: u32) -> Line {
    let mut l = Line::default();
    let _ = write!(l, "timer delta: {d}");
    l
}

/// A fixed-capacity text line.
pub struct Line {
    buf: [u8; 192],
    len: usize,
}

impl Default for Line {
    fn default() -> Self {
        Line {
            buf: [0; 192],
            len: 0,
        }
    }
}

impl Line {
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
}

impl Write for Line {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let n = s.len().min(self.buf.len() - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}

/// Report lines, kept until the kernel releases the output sink.
pub struct Lines {
    l: [Line; 24],
    n: usize,
}

impl Default for Lines {
    fn default() -> Self {
        Lines {
            l: core::array::from_fn(|_| Line::default()),
            n: 0,
        }
    }
}

impl Lines {
    fn push(&mut self, a: core::fmt::Arguments) {
        if self.n < self.l.len() {
            let _ = self.l[self.n].write_fmt(a);
            self.n += 1;
        }
    }
    fn lines(&self) -> impl Iterator<Item = &str> {
        self.l[..self.n].iter().map(|l| l.as_str())
    }
}
