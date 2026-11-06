#![no_std]
//! Round-seven mesh-op tests, shared by every target. A target supplies an
//! output sink, a monotonic tick source (for kernel.timer.now.v1) and a cycle
//! counter; this loads the applets and runs tests 1-8 plus the cost sections.
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

/// A2's test.echo.v1 tags its answer so callers can tell which provider ran.
const A2_TAG: u32 = 0xA2A2_0000;

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
    let mut tee = Tee { out };
    cap_take();
    mute(false);
    let (ram, table) = unsafe { (&mut *addr_of_mut!(RAM), &mut *addr_of_mut!(TABLE)) };
    let k = match Kernel::new(
        &img::BLOB.0,
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
    let mut r = Run {
        chip,
        k,
        report: Lines::default(),
        cycles,
        n: n_calls,
        all: true,
        cs: checksum(&b_buf()),
    };
    r.load();
    r.hello();
    r.timer();
    r.kernel_call_costs();
    r.applet_op();
    r.nested();
    r.missing_import();
    r.swap();
    r.spoof();

    // The kernel held `out` (through the console) until now.
    let Run { report, all, .. } = r;
    for l in report.lines() {
        let _ = writeln!(tee.out, "{l}");
    }
    let _ = writeln!(tee.out, "MO {chip} done pass={}", all as u8);
    all
}

/// One test run: the kernel, the report so far, and the overall result.
struct Run<'c, 'a> {
    chip: &'c str,
    k: Kernel<'a>,
    /// Report lines, printed once the kernel releases the output sink.
    report: Lines,
    cycles: fn() -> u32,
    /// Calls per cost loop.
    n: u32,
    all: bool,
    /// test.echo.v1's answer for B's buffer (A's provider).
    cs: u32,
}

impl<'a> Run<'_, 'a> {
    /// Record one test: `MO <chip> <test> pass=<0|1> <detail>`.
    fn check(&mut self, test: &str, ok: bool, detail: core::fmt::Arguments) {
        self.all &= ok;
        self.report.push(format_args!(
            "MO {} {test} pass={} {detail}",
            self.chip, ok as u8
        ));
    }

    /// Guest instructions since the last call (0 without the `count` feature).
    fn take_steps(&mut self) -> u64 {
        #[cfg(feature = "count")]
        return core::mem::replace(&mut self.k.steps, 0);
        #[cfg(not(feature = "count"))]
        0
    }

    /// Time one section: exactly two counter reads around `f`, in
    /// `SECTIONS` order (the Unicorn harness relies on both).
    fn timed<R>(&mut self, name: &str, n: u32, f: impl FnOnce(&mut Kernel<'a>) -> R) -> R {
        self.take_steps();
        let c0 = (self.cycles)();
        let r = f(&mut self.k);
        let c1 = (self.cycles)();
        let s = self.take_steps();
        let chip = self.chip;
        self.report.push(format_args!(
            "MO {chip} cost {name} n={n} cycles={} steps={s}",
            c1.wrapping_sub(c0)
        ));
        r
    }

    /// Load H, A, B (the order matters only for exports-before-imports).
    fn load(&mut self) {
        let loads = self.timed("load_all", 3, |k| {
            [
                k.load(H, img::h::IMPORTS, img::h::EXPORTS),
                k.load(A, img::a::IMPORTS, img::a::EXPORTS),
                k.load(B, img::b::IMPORTS, img::b::EXPORTS),
            ]
        });
        let ok = loads.iter().all(|l| l.is_ok());
        let entries = self.k.registry.len();
        self.check(
            "load",
            ok,
            format_args!(
                "applets=3 registry_entries={entries} entry_bytes={} applet_bytes={} registry_bytes={}",
                core::mem::size_of::<mvm_kernel::Entry>(),
                core::mem::size_of::<mvm_kernel::Applet>(),
                core::mem::size_of::<mvm_kernel::Registry>()
            ),
        );
    }

    /// 1. A guest prints through kernel.uart.write.v1.
    fn hello(&mut self) {
        let r = self.k.call(H, img::h::HELLO, &[]);
        let want = b"hello from guest applet H\n";
        let ok = r == Ok(want.len() as u32) && cap_take().as_str().as_bytes() == want;
        self.check("1-hello", ok, format_args!("ret={r:?}"));
    }

    /// 2. A guest reads the timer around a loop and prints the delta.
    fn timer(&mut self) {
        let r = self.k.call(H, img::h::TIMER, &[2000]);
        let printed = cap_take();
        let ok =
            matches!(r, Ok(d) if d > 0 && printed.as_str().trim_end() == fmt_delta(d).as_str());
        self.check("2-timer", ok, format_args!("delta={r:?}"));
    }

    /// 3. Bare call cost: kernel.nop.v1 and a muted uart write, against an empty loop.
    fn kernel_call_costs(&mut self) {
        let n = self.n;
        let e = self.timed("empty_loop", n, |k| k.call(H, img::h::EMPTY_LOOP, &[n]));
        let p = self.timed("nop_loop", n, |k| k.call(H, img::h::NOP_LOOP, &[n]));
        mute(true);
        let u = self.timed("uart_loop_muted", n, |k| k.call(H, img::h::UART_LOOP, &[n]));
        mute(false);
        let ok = e == Ok(0) && p == Ok(0) && u == Ok(16 * n);
        self.check("3-nop-cost", ok, format_args!("calls={n}"));
    }

    /// 4. B calls A's test.echo.v1 with a buffer B owns (and the call costs).
    fn applet_op(&mut self) {
        let (n, cs) = (self.n, self.cs);
        let r = self.k.call(B, img::b::ECHO_ONCE, &[]);
        let mut ok = r == Ok(cs);
        for (len, local, echo) in [
            (0u32, "local_loop_len0", "echo_loop_len0"),
            (64, "local_loop_len64", "echo_loop_len64"),
        ] {
            let l = self.timed(local, n, |k| k.call(B, img::b::LOCAL_LOOP, &[n, len]));
            let x = self.timed(echo, n, |k| k.call(B, img::b::ECHO_LOOP, &[n, len]));
            ok &= l.is_ok() && l == x;
        }
        self.check("4-applet-op", ok, format_args!("echo={r:?} want={cs:#x}"));
    }

    /// 5. Nested re-entry: A's op prints through the kernel; recursion hits the depth cap.
    fn nested(&mut self) {
        let cs = self.cs;
        let r = self.k.call(B, img::b::LOUD, &[]);
        let loud_ok = r == Ok(cs) && cap_take().as_str() == "A echo_loud: 64 bytes\n";
        self.k.max_depth_seen = 0;
        let deepest_ok = MAX_DEPTH as u32 - 2; // B is level 0; A(n) .. A(0) take n+1 more
        let r_ok = self.k.call(B, img::b::RECURSE, &[deepest_ok]);
        let seen = self.k.max_depth_seen;
        let r_bad = self.k.call(B, img::b::RECURSE, &[deepest_ok + 1]);
        let after = self.k.call(B, img::b::ECHO_ONCE, &[]);
        let ok = loud_ok
            && r_ok == Ok(deepest_ok)
            && seen == MAX_DEPTH - 1
            && r_bad == Err(Error::DepthExceeded(MAX_DEPTH))
            && after == Ok(cs);
        self.check(
            "5-nested",
            ok,
            format_args!(
                "loud={r:?} recurse({deepest_ok})={r_ok:?} max_depth={seen} recurse({})={r_bad:?} after={after:?}",
                deepest_ok + 1
            ),
        );
        for (name, d) in [("recurse_depth1", 1u32), ("recurse_depth6", 6)] {
            self.timed(name, RECURSE_REPS, |k| {
                for _ in 0..RECURSE_REPS {
                    let _ = k.call(B, img::b::RECURSE, &[d]);
                }
            });
        }
        // Host -> guest call overhead (Kernel::call of a trivial entry), for context.
        self.timed("call_overhead", RECURSE_REPS, |k| {
            for _ in 0..RECURSE_REPS {
                let _ = k.call(H, img::h::EMPTY_LOOP, &[0]);
            }
        });
    }

    /// 6. A missing import fails the load, names the import, and changes nothing.
    fn missing_import(&mut self) {
        let before = self.k.registry.len();
        let r = self.k.load(M, img::m::IMPORTS, img::m::EXPORTS);
        let ok = matches!(r, Err(Error::Unresolved(h)) if h == fnv1a64("test.nobody.v1"))
            && self.k.registry.len() == before
            && self.k.applets[M as usize].is_none();
        let missing = match r {
            Err(Error::Unresolved(h)) => name_of(h),
            _ => "-",
        };
        self.check(
            "6-missing-import",
            ok,
            format_args!("error=unresolved:{missing}"),
        );
    }

    /// 7. A2 registers test.echo.v1 after B resolved it: old and new callers reach A2.
    fn swap(&mut self) {
        let cs = self.cs;
        let before = self.k.call(B, img::b::ECHO_ONCE, &[]);
        let l = self.k.load(A2, img::a2::IMPORTS, img::a2::EXPORTS);
        let old_binding = self.k.call(B, img::b::ECHO_ONCE, &[]);
        let l2 = self.k.load(B_AGAIN, img::b::IMPORTS, img::b::EXPORTS);
        let new_binding = self.k.call(B_AGAIN, img::b::ECHO_ONCE, &[]);
        let tagged = cs ^ A2_TAG;
        let ok = before == Ok(cs)
            && l.is_ok()
            && l2.is_ok()
            && old_binding == Ok(tagged)
            && new_binding == Ok(tagged);
        self.check(
            "7-swap",
            ok,
            format_args!(
                "before={before:?} loaded-before-swap={old_binding:?} loaded-after-swap={new_binding:?}"
            ),
        );
    }

    /// 8. Spoofing: a kernel name, and a forged hash/name pair on each side.
    fn spoof(&mut self) {
        let before = self.k.registry.len();
        let s1 = self.k.load(M, img::s1::IMPORTS, img::s1::EXPORTS);
        let s2 = self.k.load(M, img::s2::IMPORTS, img::s2::EXPORTS);
        let s3 = self.k.load(M, img::s3::IMPORTS, img::s3::EXPORTS);
        let echo = fnv1a64("test.echo.v1");
        let still = self.k.call(B, img::b::ECHO_ONCE, &[]);
        let ok = matches!(s1, Err(Error::ReservedNamespace(h)) if h == fnv1a64("kernel.nop.v1"))
            && matches!(s2, Err(Error::NameHashMismatch(h)) if h == echo)
            && matches!(s3, Err(Error::NameHashMismatch(h)) if h == echo)
            && self.k.registry.len() == before
            && self.k.applets[M as usize].is_none()
            && still == Ok(self.cs ^ A2_TAG);
        self.check(
            "8-spoof",
            ok,
            format_args!(
                "kernel-name={} forged-export={} forged-import={}",
                why(s1.err()).as_str(),
                why(s2.err()).as_str(),
                why(s3.err()).as_str()
            ),
        );
    }
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
