#![no_std]
//! Mesh-op tests (rounds 7, 7b, 7c), shared by every target. A target supplies an
//! output sink, a monotonic tick source (for kernel.timer.now.v1) and a cycle
//! counter; this loads the applets and runs tests 1-19 plus the cost sections.
//!
//! Lines: `MO <chip> <test> pass=<0|1> <details>` and
//! `MO <chip> cost <section> n=<N> cycles=<C> [steps=<S>]`, then `MO <chip> done pass=<0|1>`.
//! The Unicorn harness ignores `cycles=` and times each section itself from
//! the two counter reads around it (the order is fixed: see `SECTIONS`).

use core::fmt::Write;
use core::ptr::addr_of_mut;
use interp::fast;
use mvm_kernel::{bulk_slots, fnv1a64, fnv1a64_bytes, Console, Error, Kernel, Manifest, Segment, MAX_DEPTH};

#[allow(dead_code)]
mod img {
    include!("../../guest-image/applets.rs");
}

/// The measured sections, in the order they run (each brackets one counter pair).
pub const SECTIONS: [&str; 14] = [
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
    "none_access_loop",
    "gp_access_loop",
    "bulk_access_loop",
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

/// The kernel's RAM arena: applet static blocks at the bottom, the one shared stack on top.
const RAM_SIZE: usize = 56 * 1024;
const STACK_SIZE: u32 = 8 * 1024;
static mut RAM: [u8; RAM_SIZE] = [0; RAM_SIZE];
/// Decode tables for every loaded flash image (one entry per 2 bytes of image).
const TABLES: usize = 6144;
static mut TABLE: [fast::E; TABLES] = [fast::E::EMPTY; TABLES];

const H: u8 = 0;
const A: u8 = 1;
const B: u8 = 2;
const A2: u8 = 3;
const B_AGAIN: u8 = 4;
const M: u8 = 5;
/// Applet C: two instances of one flash image, and one from a second flash copy.
const C_A: u8 = 6;
const C_B: u8 = 7;
const C_COPY: u8 = 8;
const FIT: u8 = 9;
/// Applet D (16 KB bulk buffer): two instances, from two flash copies.
const D_A: u8 = 10;
const D_B: u8 = 11;
/// Applet E (40 small statics that overflow the gp window).
const E: u8 = 12;
const BULK_FIT: u8 = 13;
/// D's `SEED` at load.
const D_SEED: u32 = 0x1234_5678;

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
    let k = match Kernel::new(ram, STACK_SIZE, table, &mut tee, now) {
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
    r.position_independence();
    r.instances();
    r.real_pointers();
    r.shared_stack();
    r.fit();
    r.big_buffer();
    r.spill();
    r.bulk_position_independence();
    r.bulk_real_pointer();
    r.bulk_fit();
    r.no_annotations();

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
                k.load(H, &img::h::MANIFEST),
                k.load(A, &img::a::MANIFEST),
                k.load(B, &img::b::MANIFEST),
            ]
        });
        let ok = loads.iter().all(|l| l.is_ok());
        let entries = self.k.registry.len();
        self.check(
            "load",
            ok,
            format_args!(
                "applets=3 registry_entries={entries} entry_bytes={} applet_bytes={} registry_bytes={} real_addresses={}",
                core::mem::size_of::<mvm_kernel::Entry>(),
                core::mem::size_of::<mvm_kernel::Applet>(),
                core::mem::size_of::<mvm_kernel::Registry>(),
                self.k.real_addresses() as u8
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
        let r = self.k.load(M, &img::m::MANIFEST);
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
        let l = self.k.load(A2, &img::a2::MANIFEST);
        let old_binding = self.k.call(B, img::b::ECHO_ONCE, &[]);
        let l2 = self.k.load(B_AGAIN, &img::b::MANIFEST);
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
        let s1 = self.k.load(M, &img::s1::MANIFEST);
        let s2 = self.k.load(M, &img::s2::MANIFEST);
        let s3 = self.k.load(M, &img::s3::MANIFEST);
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

    /// The u32 at guest address `p`, read by the kernel.
    fn peek(&self, p: u32) -> Option<u32> {
        let b = self.k.guest_bytes(p, 4)?;
        Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// 9. One image, linked once, at two flash addresses and two RAM blocks.
    fn position_independence(&mut self) {
        let copy = Manifest {
            image: &img::c::IMAGE_COPY.0,
            ..img::c::MANIFEST
        };
        let la = self.k.load(C_A, &img::c::MANIFEST);
        let lc = self.k.load(C_COPY, &copy);
        let same_bytes = img::c::IMAGE.0 == img::c::IMAGE_COPY.0;
        let (fa, fc) = (self.k.code_addr(C_A, 0), self.k.code_addr(C_COPY, 0));
        let (ba, bc) = (self.block(C_A), self.block(C_COPY));
        let msg: &[u8] = b"C says hi from its .data\n";
        let mut ok = la.is_ok() && lc.is_ok() && same_bytes && fa != fc && ba != bc;
        for id in [C_A, C_COPY] {
            ok &= self.k.call(id, img::c::TABLE_SUM, &[]) == Ok(10);
            ok &= self.k.call(id, img::c::INC, &[]) == Ok(1);
            ok &= self.k.call(id, img::c::PRINT, &[]) == Ok(msg.len() as u32)
                && cap_take().as_str().as_bytes() == msg;
        }
        self.check(
            "9-position-independence",
            ok,
            format_args!(
                "flash={:#x},{:#x} ram={:#x},{:#x} image_fnv={:#018x} identical={} gp_accesses={} abs_ptrs={}",
                fa.unwrap_or(0),
                fc.unwrap_or(0),
                ba,
                bc,
                fnv1a64_bytes(&img::c::IMAGE.0),
                same_bytes as u8,
                img::c::GP_REWRITES,
                img::c::ABS_FLASH_POINTERS
            ),
        );
    }

    fn block(&self, id: u8) -> u32 {
        self.k.applets[id as usize].map(|a| a.block).unwrap_or(0)
    }

    /// 10. Two instances of C: same flash code, separate statics through their own gp.
    fn instances(&mut self) {
        let lb = self.k.load(C_B, &img::c::MANIFEST);
        let shared_code = self.k.code_addr(C_A, 0) == self.k.code_addr(C_B, 0);
        let separate = self.block(C_A) != self.block(C_B);
        // C_A is at 1 after test 9.
        let a2 = self.k.call(C_A, img::c::INC, &[]);
        let a3 = self.k.call(C_A, img::c::INC, &[]);
        let b1 = self.k.call(C_B, img::c::INC, &[]);
        let ok = lb.is_ok() && shared_code && separate && a2 == Ok(2) && a3 == Ok(3) && b1 == Ok(1);
        self.check(
            "10-instances",
            ok,
            format_args!(
                "same_flash={} blocks={:#x},{:#x} counts={a3:?},{b1:?}",
                shared_code as u8,
                self.block(C_A),
                self.block(C_B)
            ),
        );
    }

    /// 11. A guest pointer to a static is the real address in its block.
    fn real_pointers(&mut self) {
        let p = self.k.call(C_A, img::c::ADDR, &[]);
        let owner = self.k.call(C_A, img::c::OWNER, &[]);
        let want = self.block(C_A) + img::c::STATIC_COUNT;
        let value = p.ok().and_then(|p| self.peek(p));
        // On a 32-bit chip the guest address *is* the host address: read through it directly.
        let direct = match p {
            Ok(p) if self.k.real_addresses() => unsafe {
                Some(core::ptr::read_volatile(p as usize as *const u32))
            },
            _ => None,
        };
        let ok = p == Ok(want)
            && owner == Ok(C_A as u32 + 1)
            && value == Some(3)
            && (!self.k.real_addresses() || direct == Some(3));
        self.check(
            "11-real-pointers",
            ok,
            format_args!(
                "ptr={p:?} block+off={want:#x} owner={owner:?} real={} direct_read={direct:?}",
                self.k.real_addresses() as u8
            ),
        );
    }

    /// Bytes of the shared stack used by `f` (paint, run, find the deepest write).
    fn stack_peak(&mut self, f: impl FnOnce(&mut Kernel<'a>)) -> usize {
        self.k.stack().fill(0xA5);
        f(&mut self.k);
        let s = self.k.stack();
        s.len() - s.iter().position(|&b| b != 0xA5).unwrap_or(s.len())
    }

    /// 12. Nested calls share the one stack: peak use per nesting level.
    fn shared_stack(&mut self) {
        let mut r = [Ok(0); 2];
        let p1 = self.stack_peak(|k| r[0] = k.call(B, img::b::RECURSE, &[1]));
        let p6 = self.stack_peak(|k| r[1] = k.call(B, img::b::RECURSE, &[6]));
        let per_level = (p6.saturating_sub(p1)) / 5;
        let ok = r == [Ok(1), Ok(6)] && p6 > p1 && p6 < STACK_SIZE as usize;
        self.check(
            "12-shared-stack",
            ok,
            format_args!(
                "peak_depth2={p1} peak_depth7={p6} per_level={per_level} stack={STACK_SIZE}"
            ),
        );
    }

    /// 13. An applet whose statics don't fit is rejected before anything changes.
    ///     (7c: C's gp window plus a bulk segment that alone would just fit.)
    fn fit(&mut self) {
        let free = self.k.free_ram();
        let entries = self.k.registry.len();
        let bulk = free - 16;
        let big = Manifest {
            bulk: Segment {
                size: bulk,
                align: 4,
                init: &[],
            },
            bulk_slots: bulk_slots(bulk),
            gp: Segment {
                size: img::c::MANIFEST.gp.size.max(4 * bulk_slots(bulk)),
                ..img::c::MANIFEST.gp
            },
            ..img::c::MANIFEST
        };
        let r = self.k.load(FIT, &big);
        let ok = matches!(r, Err(Error::NoRoom { need, free: f }) if f == free && need > free)
            && self.k.free_ram() == free
            && self.k.registry.len() == entries
            && self.k.applets[FIT as usize].is_none();
        self.check("13-fit", ok, format_args!("free={free} result={r:?}"));
    }

    fn bulk_of(&self, id: u8) -> u32 {
        self.k.applets[id as usize].map(|a| a.bulk).unwrap_or(0)
    }

    /// 14. D: a 16 KB static buffer plus small counters. Fill, checksum,
    ///     bulk `.data` init, far constant offsets, a later bulk slot. Also the
    ///     per-access cost sections.
    fn big_buffer(&mut self) {
        let l = self.k.load(D_A, &img::d::MANIFEST);
        let seed = 0xC0FF_EE11;
        let fills = self.k.call(D_A, img::d::FILL, &[seed]);
        let sum = self.k.call(D_A, img::d::SUM, &[]);
        let want = d_checksum(seed ^ D_SEED);
        let squares = self.k.call(D_A, img::d::SQUARES_SUM, &[]);
        let want_sq = (0..96u32).fold(0u32, |a, i| a.wrapping_add(i * i * 7 + 1));
        let _ = self.k.call(D_A, img::d::CORNERS_SET, &[0x11]);
        let corners = self.k.call(D_A, img::d::CORNERS_GET, &[]);
        let want_c = 0x11 ^ (0x12 << 8) ^ (0x13 << 16);
        let tail = [
            self.k.call(D_A, img::d::TAIL, &[3, 5]),
            self.k.call(D_A, img::d::TAIL, &[63, 7]),
        ];
        let bump = self.k.call(D_A, img::d::BUMP, &[2, 100]);
        let ptrs = self.k.call(D_A, img::d::PTRS_OK, &[]);
        let _ = self.k.call(D_A, img::d::PTR_SET, &[0]);
        let ok = l.is_ok()
            && ptrs == Ok(1)
            && img::d::FILLS_PTRS_IN_BULK
            && img::d::PROMOTED == 1
            && !img::d::WORDS_PTR_IN_BULK
            && img::d::WORDS_IN_BULK
            && img::d::SQUARES_IN_BULK
            && img::d::TAIL_IN_BULK
            && !img::d::FILLS_IN_BULK
            && !img::d::SEED_IN_BULK
            && fills == Ok(1)
            && sum == Ok(want)
            && squares == Ok(want_sq)
            && corners == Ok(want_c)
            && tail == [Ok(5), Ok(12)]
            && bump == Ok(2 * 2 * 7 + 1 + 100);
        self.check(
            "14-big-buffer",
            ok,
            format_args!(
                "sum={sum:?} want={want:#x} squares={squares:?} corners={corners:?} tail={:?} ptrs={ptrs:?} relocs={} gp={} bulk={} slots={} gp_acc={} bulk_acc={}",
                tail[1],
                img::d::DATA_RELOCS.len(),
                img::d::MANIFEST.gp.size,
                img::d::MANIFEST.bulk.size,
                img::d::MANIFEST.bulk_slots,
                img::d::GP_REWRITES,
                img::d::BULK_REWRITES
            ),
        );
        let n = self.n;
        for (name, entry) in [
            ("none_access_loop", img::d::NONE_LOOP),
            ("gp_access_loop", img::d::GP_LOOP),
            ("bulk_access_loop", img::d::BULK_LOOP),
        ] {
            let _ = self.timed(name, n, |k| k.call(D_A, entry, &[n]));
        }
    }

    /// 15. E: 41 small statics (over 4 KB in all) build and run; the chunker
    ///     spilled some to bulk.
    fn spill(&mut self) {
        let l = self.k.load(E, &img::e::MANIFEST);
        let init = self.k.call(E, img::e::SUM, &[]);
        // Odd statics start at their index (32 words each), even ones at 0.
        let want_init = (1..40u32).step_by(2).map(|k| 32 * k).sum::<u32>();
        let runs = self.k.call(E, img::e::FILL, &[1000]);
        let after = self.k.call(E, img::e::SUM, &[]);
        let want = (0..40 * 32u32).fold(0u32, |a, i| a.wrapping_add(1000 + i));
        let m = &img::e::MANIFEST;
        let ok = l.is_ok()
            && init == Ok(want_init)
            && runs == Ok(1)
            && after == Ok(want)
            && img::e::STATIC_BYTES > mvm_kernel::GP_BIAS * 2
            && img::e::SPILLED > 0
            && m.gp.size <= mvm_kernel::GP_BIAS * 2;
        self.check(
            "15-overflow-spill",
            ok,
            format_args!(
                "statics={} bytes={} spilled={} gp={} bulk={} init={init:?} after={after:?}",
                img::e::STATICS,
                img::e::STATIC_BYTES,
                img::e::SPILLED,
                m.gp.size,
                m.bulk.size
            ),
        );
    }

    /// 16. D again, from a second flash copy: other flash, other gp window,
    ///     other bulk segment; identical code bytes, separate state.
    fn bulk_position_independence(&mut self) {
        let copy = Manifest {
            image: &img::d::IMAGE_COPY.0,
            ..img::d::MANIFEST
        };
        let a_before = self.k.call(D_A, img::d::SUM, &[]);
        let l = self.k.load(D_B, &copy);
        let seed = 0x5EED_0002;
        let fills = self.k.call(D_B, img::d::FILL, &[seed]);
        let sum_b = self.k.call(D_B, img::d::SUM, &[]);
        let a_after = self.k.call(D_A, img::d::SUM, &[]);
        let fills_a = self.k.call(D_A, img::d::FILL, &[0x77]);
        let same = img::d::IMAGE.0 == img::d::IMAGE_COPY.0;
        let (fa, fb) = (self.k.code_addr(D_A, 0), self.k.code_addr(D_B, 0));
        let ok = l.is_ok()
            && same
            && fa != fb
            && self.block(D_A) != self.block(D_B)
            && self.bulk_of(D_A) != self.bulk_of(D_B)
            && fills == Ok(1)
            && sum_b == Ok(d_checksum(seed ^ D_SEED))
            && a_before.is_ok()
            && a_after == a_before
            && fills_a == Ok(2);
        self.check(
            "16-bulk-position-independence",
            ok,
            format_args!(
                "flash={:#x},{:#x} gp={:#x},{:#x} bulk={:#x},{:#x} image_fnv={:#018x} identical={} sums={a_after:?},{sum_b:?} want_b={:#x} fills={fills:?},{fills_a:?}",
                fa.unwrap_or(0),
                fb.unwrap_or(0),
                self.block(D_A),
                self.block(D_B),
                self.bulk_of(D_A),
                self.bulk_of(D_B),
                fnv1a64_bytes(&img::d::IMAGE.0),
                same as u8,
                d_checksum(seed ^ D_SEED)
            ),
        );
    }

    /// 17. A pointer into D's bulk buffer is the real address: the kernel
    ///     reads 16 KB through it (kernel.mem.sum.v1) and names its owner.
    fn bulk_real_pointer(&mut self) {
        let _ = self.k.call(D_A, img::d::FILL, &[0xABCD]);
        let p = self.k.call(D_A, img::d::ADDR, &[5]);
        let want = self.bulk_of(D_A) + img::d::WORDS_OFF + 5 * 4;
        let owner = self.k.call(D_A, img::d::OWNER, &[5]);
        let guest_sum = self.k.call(D_A, img::d::SUM, &[]);
        let kernel_sum = self.k.call(D_A, img::d::KERNEL_SUM, &[]);
        let word = p.ok().and_then(|p| self.peek(p));
        let direct = match p {
            Ok(p) if self.k.real_addresses() => unsafe {
                Some(core::ptr::read_volatile(p as usize as *const u32))
            },
            _ => None,
        };
        let ok = p == Ok(want)
            && owner == Ok(D_A as u32 + 1)
            && guest_sum.is_ok()
            && kernel_sum == guest_sum
            && word.is_some()
            && (!self.k.real_addresses() || direct == word);
        self.check(
            "17-bulk-real-pointer",
            ok,
            format_args!(
                "ptr={p:?} bulk+off={want:#x} owner={owner:?} kernel_sum={kernel_sum:?} guest_sum={guest_sum:?} real={} direct_read={direct:?}",
                self.k.real_addresses() as u8
            ),
        );
    }

    /// 18. An applet whose bulk segment doesn't fit in free RAM is rejected
    ///     before anything changes (its gp window alone would fit).
    fn bulk_fit(&mut self) {
        let free = self.k.free_ram();
        let entries = self.k.registry.len();
        let size = free + 1;
        let big = Manifest {
            bulk: Segment {
                size,
                ..img::d::MANIFEST.bulk
            },
            bulk_slots: bulk_slots(size),
            gp: Segment {
                size: img::d::MANIFEST.gp.size.max(4 * bulk_slots(size)),
                ..img::d::MANIFEST.gp
            },
            ..img::d::MANIFEST
        };
        let r = self.k.load(BULK_FIT, &big);
        let ok = matches!(r, Err(Error::NoRoom { need, free: f }) if f == free && need > free)
            && self.k.free_ram() == free
            && self.k.registry.len() == entries
            && self.k.applets[BULK_FIT as usize].is_none();
        self.check("18-bulk-fit", ok, format_args!("free={free} bulk={size} result={r:?}"));
    }

    /// 19. The 7c applets' source uses only ordinary statics (checked at
    ///     build time by build_applets.py; the counts are in the generated image).
    fn no_annotations(&mut self) {
        let ok = img::PLAIN_STATICS > 0 && img::ANNOTATED_STATICS == 0;
        self.check(
            "19-no-annotations",
            ok,
            format_args!(
                "plain_statics={} annotated={}",
                img::PLAIN_STATICS,
                img::ANNOTATED_STATICS
            ),
        );
    }
}

/// FNV-1a 32 of D's buffer after `d_fill`: a xorshift32 stream from `x`.
fn d_checksum(mut x: u32) -> u32 {
    let mut s = 0x811c_9dc5u32;
    for _ in 0..4096 {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        for b in x.to_le_bytes() {
            s = (s ^ b as u32).wrapping_mul(0x0100_0193);
        }
    }
    s
}

fn fmt_delta(d: u32) -> Line {
    let mut l = Line::default();
    let _ = write!(l, "timer delta: {d}");
    l
}

/// A fixed-capacity text line.
pub struct Line {
    buf: [u8; 256],
    len: usize,
}

impl Default for Line {
    fn default() -> Self {
        Line {
            buf: [0; 256],
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
    l: [Line; 48],
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
