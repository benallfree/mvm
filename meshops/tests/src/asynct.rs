//! Round 7d: the async event-model tests (20-29), shared by every target.
//!
//! The target supplies a [`Platform`]: a way to sleep until a driver
//! interrupt may have pushed something. The fake drivers run in the
//! target's interrupt context through [`driver_tick`] (1 kHz) and, on the
//! emulators, [`driver_input`] (inputs a script types on the UART). The tests
//! drive inputs through the drivers' request ring (the test hook), so they
//! arrive from interrupt context exactly like real ones.

use crate::{cap_take, checksum, img, mute, Line, Lines, Tee, RAM, STACK_SIZE, TABLE};
use core::fmt::Write;
use core::future::Future;
use core::pin::pin;
use core::ptr::addr_of_mut;
use core::sync::atomic::Ordering;
use core::task::{Context, Poll, Waker};
use mvm_abi::asyncabi::{DONE, INVALID, RISING};
use mvm_kernel::drivers::{Drivers, SEND_MS};
use mvm_kernel::io::{Event, Io, Packet, Request};
use mvm_kernel::{Kernel, TState, DEFAULT_BUDGET, MAX_TICKETS};

/// The rings shared with the drivers (one per program).
pub static IO: Io = Io::new();
/// The fake drivers: touched only from driver (interrupt) context.
static mut DRIVERS: Drivers = Drivers::new();

/// One driver tick (1 ms). Call from the target's driver interrupt / task
/// only. True if something was pushed into the inbox.
pub fn driver_tick() -> bool {
    // SAFETY: only ever called from the one driver context.
    unsafe { (*addr_of_mut!(DRIVERS)).tick(&IO) }
}

/// An input arriving now (driver context only: the emulators' UART script).
pub fn driver_input(what: Inject) -> bool {
    // SAFETY: as driver_tick.
    unsafe { (*addr_of_mut!(DRIVERS)).input(&IO, what) }
}

pub use mvm_kernel::io::Inject;

/// One line typed by an emulator script (driver context): `P<text>` delivers
/// a packet, `E<pin>` a rising edge on that pin.
pub fn script_line(line: &[u8]) {
    match line.split_first() {
        Some((b'P', rest)) => {
            driver_input(Inject::Packet(Packet::new(rest)));
        }
        Some((b'E', rest)) => {
            let pin = rest.iter().fold(0u8, |a, &d| a.wrapping_mul(10).wrapping_add(d.wrapping_sub(b'0')));
            driver_input(Inject::Edge { pin, edge: RISING as u8 });
        }
        _ => {}
    }
}

/// What a target provides for the async tests.
pub trait Platform {
    /// Sleep until a driver interrupt may have pushed an event (or about a
    /// millisecond passed), then return.
    fn wait(&mut self) -> impl Future<Output = ()>;
    /// Whether the drivers run in a context that preempts guest code (an
    /// interrupt). False when they only tick from `wait` (QEMU ESP32-S3).
    fn preemptive(&self) -> bool {
        true
    }
}

/// Run a future to completion with a no-op waker, for targets without an
/// executor (host, Unicorn): their `Platform::wait` sleeps and returns Ready.
pub fn block_on<F: Future>(f: F) -> F::Output {
    let mut f = pin!(f);
    let mut cx = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
    }
}

// Applet ids in the async kernel.
const A: u8 = 0;
const B: u8 = 1;
const F: u8 = 2;
const F2: u8 = 3;
const T1: u8 = 4;
const T2: u8 = 5;
const R: u8 = 6;
const T1B: u8 = 7;
const X: u8 = 8;
const F3: u8 = 9;
const L: u8 = 10;
const S: u8 = 11;
const S2: u8 = 12;
const L_FOR_BURN: u8 = L;

/// Measured sections of the async run (after `crate::SECTIONS`).
pub const SECTIONS: [&str; 9] = [
    "a_budget_burn",
    "a_long_call",
    "a_call_empty",
    "a_empty_loop",
    "a_check_loop",
    "a_start16",
    "a_cancel16",
    "a_select_poll",
    "a_wake_poll",
];

struct ARun<'c, 'a, P: Platform> {
    chip: &'c str,
    k: Kernel<'a>,
    plat: &'c mut P,
    report: Lines,
    all: bool,
    cycles: fn() -> u32,
    n: u32,
    /// Executor loop turns that found nothing to do (the chip could sleep).
    idle_waits: u32,
    /// Futures' sizes: hello, recv, ticker, select.
    fut: [u32; 4],
}

/// Run tests 20-29 (and, with `script`, 30: inputs typed by an external
/// script on the emulators). Prints its lines to `out` at the end.
pub async fn run_async<W: Write, P: Platform>(
    chip: &str,
    out: &mut W,
    plat: &mut P,
    now: fn() -> u32,
    cycles: fn() -> u32,
    n_calls: u32,
    script: bool,
) -> bool {
    let mut tee = Tee { out };
    cap_take();
    mute(false);
    let (ram, table) = unsafe { (&mut *addr_of_mut!(RAM), &mut *addr_of_mut!(TABLE)) };
    let k = match Kernel::new(ram, STACK_SIZE, table, &mut tee, now, &IO) {
        Ok(k) => k,
        Err(e) => {
            let _ = writeln!(tee.out, "MO {chip} async-init pass=0 {e:?}");
            return false;
        }
    };
    let mut r = ARun {
        chip,
        k,
        plat,
        report: Lines::default(),
        all: true,
        cycles,
        n: n_calls,
        idle_waits: 0,
        fut: [0; 4],
    };
    let ok = r.k.load(A, &img::a::MANIFEST).is_ok()
        && r.k.load(B, &img::b::MANIFEST).is_ok()
        && r.k.load(L, &img::l::MANIFEST).is_ok();
    r.check("async-load", ok, format_args!("budget={} tickets={MAX_TICKETS} inbox={}", DEFAULT_BUDGET, mvm_kernel::io::INBOX));
    r.hello_loop().await;
    r.two_tasks().await;
    r.two_applets().await;
    r.idle().await;
    r.no_deadlock().await;
    r.budget().await;
    r.stale_ticket().await;
    r.overflow().await;
    r.driver_priority().await;
    r.select().await;
    r.costs().await;
    r.sizes();
    if script {
        r.script().await;
    }
    let ARun { report, all, .. } = r;
    for l in report.lines() {
        let _ = writeln!(tee.out, "{l}");
    }
    let _ = writeln!(tee.out, "MO {chip} async-done pass={}", all as u8);
    all
}

fn hex4(p: &[u8]) -> u32 {
    let mut b = [0u8; 4];
    let n = p.len().min(4);
    b[..n].copy_from_slice(&p[..n]);
    u32::from_le_bytes(b)
}

impl<'a, P: Platform> ARun<'_, 'a, P> {
    fn check(&mut self, test: &str, ok: bool, detail: core::fmt::Arguments) {
        self.all &= ok;
        self.report.push(format_args!("MO {} {test} pass={} {detail}", self.chip, ok as u8));
    }

    fn now(&self) -> u32 {
        IO.now()
    }

    /// Ask the drivers to deliver `what` at `at` ms (the test hook).
    fn inject(&self, at: u32, what: Inject) {
        let _ = IO.requests.push(Request::Inject { at, what });
    }

    fn packet(&self, at: u32, b: &[u8]) {
        self.inject(at, Inject::Packet(Packet::new(b)));
    }

    fn edge(&self, at: u32, pin: u8) {
        self.inject(at, Inject::Edge { pin, edge: RISING as u8 });
    }

    /// Run the executor until `done` holds or `ms` milliseconds pass.
    async fn run_until(&mut self, ms: u32, mut done: impl FnMut(&mut Kernel<'a>) -> bool) -> bool {
        let deadline = self.now().wrapping_add(ms);
        loop {
            if done(&mut self.k) {
                return true;
            }
            if self.k.has_work() {
                self.k.run_once();
                continue;
            }
            if (self.now().wrapping_sub(deadline) as i32) >= 0 {
                return done(&mut self.k);
            }
            self.idle_waits += 1;
            self.plat.wait().await;
        }
    }

    async fn run_for(&mut self, ms: u32) {
        self.run_until(ms, |_| false).await;
    }

    /// Quiesce the drivers (for cost sections that push events themselves).
    async fn pause(&mut self, on: bool) {
        IO.pause.store(on, Ordering::Release);
        if on {
            while !IO.paused_ack.load(Ordering::Acquire) {
                self.plat.wait().await;
            }
        }
    }

    fn stat(&mut self, applet: u8, entry: u32, i: u32) -> u32 {
        self.k.call(applet, entry, &[i]).unwrap_or(u32::MAX)
    }

    fn polls(&self, applet: u8, task: usize) -> u32 {
        self.k.exec_stats().polls_by[applet as usize][task]
    }

    /// 20. One task: 5 x (timer 50 ms, send "helloN", await the send).
    async fn hello_loop(&mut self) {
        let t0 = self.now();
        while IO.sent.pop().is_some() {}
        let l = self.k.load(F, &img::f::MANIFEST);
        let done = self.run_until(600, |k| k.call(F, img::f::STAT, &[0]) == Ok(5)).await;
        let mut ok = l.is_ok() && done;
        let mut starts = [0u32; 5];
        let mut n = 0;
        let mut order_ok = true;
        while let Some(s) = IO.sent.pop() {
            if n < 5 {
                starts[n] = s.start_ms.wrapping_sub(t0);
                order_ok &= s.packet.bytes() == [b'h', b'e', b'l', b'l', b'o', b'0' + n as u8];
                order_ok &= s.done_ms.wrapping_sub(s.start_ms) == SEND_MS;
            }
            n += 1;
        }
        // Each send starts 50 ms after the previous one completed (+ wake latency).
        let mut timing_ok = (50..=53).contains(&starts[0]);
        for i in 1..5 {
            let gap = starts[i].wrapping_sub(starts[i - 1]);
            timing_ok &= (50 + SEND_MS..=53 + SEND_MS).contains(&gap);
        }
        // 1 first poll + 2 wakes per round (timer, send); the last one finishes.
        let polls = self.polls(F, 0);
        self.fut[0] = self.stat(F, img::f::STAT, 3);
        self.fut[1] = self.stat(F, img::f::STAT, 4);
        // Retire F: its receive loop would take later tests' packets.
        self.k.stop(F);
        ok &= n == 5 && order_ok && timing_ok && polls == 11;
        self.check(
            "20-hello-loop",
            ok,
            format_args!("sends={n} order={} start_ms={starts:?} timing={} polls={polls}", order_ok as u8, timing_ok as u8),
        );
    }

    /// 21. Two tasks in one applet: the hello loop plus a receive loop.
    async fn two_tasks(&mut self) {
        let t0 = self.now();
        while IO.sent.pop().is_some() {}
        let l = self.k.load(F2, &img::f::MANIFEST);
        for (k, at) in [25u32, 80, 130, 190].iter().enumerate() {
            self.packet(t0 + at, &[b'p', b'k', b't', b'0' + k as u8]);
        }
        let mid = self.run_until(205, |_| false).await;
        let _ = mid;
        let sent_mid = self.stat(F2, img::f::STAT, 0);
        let recv_mid = self.stat(F2, img::f::STAT, 1);
        let done = self.run_until(400, |k| k.call(F2, img::f::STAT, &[0]) == Ok(5)).await;
        let last = self.stat(F2, img::f::STAT, 2);
        let (p0, p1) = (self.polls(F2, 0), self.polls(F2, 1));
        let ok = l.is_ok()
            && done
            && recv_mid == 4
            && (1..5).contains(&sent_mid)
            && last == hex4(b"pkt3")
            && p0 == 11
            && p1 == 1 + 4;
        self.k.stop(F2);
        self.check(
            "21-two-tasks",
            ok,
            format_args!("at_205ms: sent={sent_mid} received={recv_mid} polls: hello={p0} recv={p1} last={last:#x}"),
        );
    }

    /// 22. Two applets with timers at 30 and 70 ms: each polled only for its own ticket.
    async fn two_applets(&mut self) {
        let l1 = self.k.load(T1, &img::t1::MANIFEST);
        let l2 = self.k.load(T2, &img::t2::MANIFEST);
        self.run_for(430).await;
        let (n1, n2) = (self.stat(T1, img::t1::STAT, 0), self.stat(T2, img::t2::STAT, 0));
        let (p1, p2) = (self.polls(T1, 0), self.polls(T2, 0));
        self.fut[2] = self.stat(T1, img::t1::STAT, 99);
        let mut spacing_ok = n1 >= 2 && n2 >= 2;
        for k in 1..n1.min(14) {
            let d = self.stat(T1, img::t1::STAT, 2 + k).wrapping_sub(self.stat(T1, img::t1::STAT, 1 + k));
            spacing_ok &= (30..=32).contains(&d);
        }
        for k in 1..n2.min(5) {
            let d = self.stat(T2, img::t2::STAT, 2 + k).wrapping_sub(self.stat(T2, img::t2::STAT, 1 + k));
            spacing_ok &= (70..=72).contains(&d);
        }
        let ok = l1.is_ok()
            && l2.is_ok()
            && (13..=15).contains(&n1)
            && (5..=7).contains(&n2)
            && p1 == n1 + 1
            && p2 == n2 + 1
            && spacing_ok;
        self.check(
            "22-two-applets",
            ok,
            format_args!("ticks t1={n1} t2={n2} polls t1={p1} t2={p2} spacing={}", spacing_ok as u8),
        );
    }

    /// 23. Idle: with every task waiting, no guest code runs between events.
    async fn idle(&mut self) {
        let s0 = *self.k.exec_stats();
        let (a0, b0) = (self.stat(T1, img::t1::STAT, 0), self.stat(T2, img::t2::STAT, 0));
        let w0 = self.idle_waits;
        let calls0 = self.k.exec_stats().guest_calls;
        self.run_for(210).await;
        let calls1 = self.k.exec_stats().guest_calls;
        let s1 = *self.k.exec_stats();
        let (a1, b1) = (self.stat(T1, img::t1::STAT, 0), self.stat(T2, img::t2::STAT, 0));
        let ticks = (a1 - a0) + (b1 - b0);
        let polls = s1.polls - s0.polls;
        let events = s1.events - s0.events;
        // Every guest call in the window was a poll or the echo it made.
        let guest = calls1 - calls0;
        let ok = polls == ticks && events == ticks && guest == 2 * polls && !self.k.has_work();
        self.check(
            "23-idle",
            ok,
            format_args!(
                "window=210ms events={events} polls={polls} guest_calls={guest} (poll+echo each) idle_waits={} has_work={}",
                self.idle_waits - w0,
                self.k.has_work() as u8
            ),
        );
    }

    /// 24. A mesh op into another applet's guest code (A's test.echo.v1),
    /// from inside polls and from the host, while async tasks are pending.
    async fn no_deadlock(&mut self) {
        let pending = self.k.live_tickets();
        let want = checksum(b"tick");
        let e1 = self.stat(T1, img::t1::STAT, 1);
        let e2 = self.stat(T2, img::t2::STAT, 1);
        let mut buf_cs = [0u8; 64];
        for (i, v) in buf_cs.iter_mut().enumerate() {
            *v = (i as u8).wrapping_mul(7).wrapping_add(3);
        }
        let b = self.k.call(B, img::b::ECHO_ONCE, &[]);
        let r = self.k.call(B, img::b::RECURSE, &[3]);
        let ok = pending >= 2 && e1 == want && e2 == want && b == Ok(checksum(&buf_cs)) && r == Ok(3);
        self.check(
            "24-no-deadlock",
            ok,
            format_args!("pending_tickets={pending} echo_in_poll={e1:#x},{e2:#x} want={want:#x} host_echo={b:?} recurse={r:?}"),
        );
        let _ = self.k.call(T1, img::t1::HALT, &[]);
        let _ = self.k.call(T2, img::t2::HALT, &[]);
        self.run_until(200, |k| k.applets[T1 as usize].is_some_and(|a| a.spawned == 0)
            && k.applets[T2 as usize].is_some_and(|a| a.spawned == 0)).await;
    }

    /// 25. A runaway poll is cut off by the budget; another applet keeps its schedule.
    async fn budget(&mut self) {
        let l1 = self.k.load(T1B, &img::t1::MANIFEST);
        let lr = self.k.load(R, &img::r::MANIFEST);
        let faults0 = self.k.exec_stats().faults;
        let faulted = self.run_until(100, |k| k.applets[R as usize].is_some_and(|a| a.faulted)).await;
        let held = self.stat(R, img::r::STAT, 1);
        self.run_for(200).await;
        let n = self.stat(T1B, img::t1::STAT, 0);
        // On schedule: no tick later than one budget's worth of runaway (the
        // one interpreter was busy with R until the budget cut it off).
        let mut spacing_ok = n >= 6;
        let mut worst = 0;
        for k in 1..n.min(14) {
            let d = self.stat(T1B, img::t1::STAT, 2 + k).wrapping_sub(self.stat(T1B, img::t1::STAT, 1 + k));
            worst = worst.max(d);
            spacing_ok &= (30..=36).contains(&d);
        }
        // How long a runaway holds the CPU before the kernel cuts it off.
        let c0 = (self.cycles)();
        let burn = self.k.call_budgeted(L_FOR_BURN, img::l::SPIN, &[u32::MAX], DEFAULT_BUDGET);
        let c1 = (self.cycles)();
        self.cost_line("a_budget_burn", 1, c0, c1);
        let r_tickets = self.k.live_tickets_of(R);
        let ok = l1.is_ok()
            && lr.is_ok()
            && faulted
            && self.k.exec_stats().faults == faults0 + 1
            && r_tickets == 0
            && self.k.ticket(held).is_none()
            && burn == Err(mvm_kernel::Error::Budget)
            && spacing_ok;
        let freed = self.k.ticket(held).is_none() as u8;
        let budget = self.k.budget;
        self.check(
            "25-budget",
            ok,
            format_args!(
                "faulted={} r_tickets={r_tickets} held_ticket_freed={freed} t1_ticks={n} spacing={} worst_tick_gap_ms={worst} budget={budget} burn={burn:?}",
                faulted as u8,
                spacing_ok as u8,
            ),
        );
        let _ = self.k.call(T1B, img::t1::HALT, &[]);
    }

    /// 26. A freed or reused ticket id never reads someone else's result.
    async fn stale_ticket(&mut self) {
        let l = self.k.load(X, &img::x::MANIFEST);
        // F3 (used again in test 27) holds a receive that stays pending: the
        // other applet's ticket X must not be able to read.
        let l3 = self.k.load(F3, &img::f::MANIFEST);
        self.run_for(2).await; // X's and F3's tasks arm their waits
        let x = |s: &mut Self, e: u32, a: &[u32]| s.k.call(X, e, a).unwrap_or(u32::MAX);
        let a = x(self, img::x::START, &[5]);
        let c1 = x(self, img::x::CANCEL, &[a]);
        let s1 = x(self, img::x::CHECK, &[a]) & 0xFF;
        let b = x(self, img::x::START, &[5]);
        let reused = (a & 0xFF) == (b & 0xFF) && a != b;
        let s2 = x(self, img::x::CHECK, &[a]) & 0xFF;
        let bid = b;
        self.run_until(50, |k| k.ticket(bid).is_some_and(|t| t.state == TState::Done)).await;
        let s3 = x(self, img::x::CHECK, &[a]) & 0xFF;
        let s4 = x(self, img::x::CHECK, &[b]) & 0xFF;
        let s5 = x(self, img::x::CHECK, &[b]) & 0xFF;
        // Another applet's live ticket: X must not be able to read it.
        let other = self
            .k
            .tickets_of(F3)
            .find(|(_, t)| matches!(t.kind, mvm_kernel::exec::TKind::Recv { .. }))
            .map(|(id, _)| id)
            .unwrap_or(0);
        let s6 = x(self, img::x::CHECK, &[other]) & 0xFF;
        let still = self.k.ticket(other).is_some();
        let ok = l.is_ok()
            && l3.is_ok()
            && c1 == 0
            && s1 == INVALID
            && reused
            && s2 == INVALID
            && s3 == INVALID
            && s4 == DONE
            && s5 == INVALID
            && other != 0
            && s6 == INVALID
            && still;
        self.check(
            "26-stale-ticket",
            ok,
            format_args!(
                "a={a:#x} b={b:#x} slot_reused={} checks: cancelled={s1} a_after_reuse={s2},{s3} b={s4} b_again={s5} other_applets={s6} other_untouched={}",
                reused as u8, still as u8
            ),
        );
    }

    /// 27. Flood the fake radio: 50 packets in one millisecond.
    async fn overflow(&mut self) {
        let l: Result<(), ()> = Ok(()); // F3 was loaded in test 26
        let st0 = (IO.stats.rx_dropped.load(Ordering::Relaxed), *self.k.exec_stats());
        let r0 = self.stat(F3, img::f::STAT, 1);
        // The test hook's request ring holds 32: schedule in two halves.
        let at = self.now() + 4;
        for i in 0..25u8 {
            self.packet(at, &[b'f', i]);
        }
        self.plat.wait().await;
        while !IO.requests.is_empty() {
            self.plat.wait().await;
        }
        for i in 25..50u8 {
            self.packet(at, &[b'f', i]);
        }
        self.run_for(20).await;
        let rx_dropped = IO.stats.rx_dropped.load(Ordering::Relaxed) - st0.0;
        let s = *self.k.exec_stats();
        let backlog_dropped = s.backlog_dropped - st0.1.backlog_dropped;
        let received = self.stat(F3, img::f::STAT, 1) - r0;
        // Later traffic still flows.
        let at = self.now() + 3;
        self.packet(at, b"late1");
        self.packet(at + 5, b"late2");
        self.run_for(20).await;
        let after = self.stat(F3, img::f::STAT, 1) - r0 - received;
        let ok = l.is_ok()
            && rx_dropped + backlog_dropped + received == 50
            && rx_dropped > 0
            && after == 2;
        self.check(
            "27-inbox-overflow",
            ok,
            format_args!(
                "flood=50 received={received} dropped_inbox_full={rx_dropped} dropped_backlog_full={backlog_dropped} later={after} inbox={} backlog={}",
                mvm_kernel::io::INBOX,
                mvm_kernel::exec::BACKLOG
            ),
        );
        self.k.stop(F3);
    }

    /// 28. Drivers keep running during a long (in-budget) guest call.
    async fn driver_priority(&mut self) {
        if !self.plat.preemptive() {
            // Keep the section count fixed for the report.
            let c = (self.cycles)();
            self.cost_line("a_long_call", 0, c, c);
            self.check(
                "28-driver-priority",
                true,
                format_args!("skipped=1 (no preemptive driver context on this target: drivers tick from the wait loop)"),
            );
            return;
        }
        let l: Result<(), ()> = Ok(());
        // Size the spin to cover at least 8 driver ticks on this target.
        let mut n = 1000u32;
        let mut dt = 0;
        while n < 1 << 26 {
            let t0 = self.now();
            let _ = self.k.call_budgeted(L, img::l::SPIN, &[n], 2 * n + 1000);
            dt = self.now().wrapping_sub(t0);
            if dt >= 12 {
                break;
            }
            n *= 2;
        }
        self.run_for(2).await;
        let s0 = (
            IO.stats.pushed_during_guest.load(Ordering::Relaxed),
            IO.stats.ticks_during_guest.load(Ordering::Relaxed),
            IO.stats.edges_dropped.load(Ordering::Relaxed),
            self.k.exec_stats().edges_unmatched,
        );
        let t0 = self.now();
        for k in 0..6 {
            self.edge(t0 + 2 + k, 20);
        }
        let c0 = (self.cycles)();
        let r = self.k.call_budgeted(L, img::l::SPIN, &[n], 2 * n + 1000);
        let c1 = (self.cycles)();
        self.cost_line("a_long_call", n, c0, c1);
        let spun = self.now().wrapping_sub(t0);
        let queued = IO.inbox.len();
        let pushed = IO.stats.pushed_during_guest.load(Ordering::Relaxed) - s0.0;
        let ticks = IO.stats.ticks_during_guest.load(Ordering::Relaxed) - s0.1;
        self.run_for(2).await;
        let dropped = IO.stats.edges_dropped.load(Ordering::Relaxed) - s0.2;
        let edges = self.k.exec_stats().edges_unmatched - s0.3;
        let ok = l.is_ok() && r.is_ok() && ticks >= 8 && queued >= 6 && pushed >= 6 && edges == 6 && dropped == 0;
        self.check(
            "28-driver-priority",
            ok,
            format_args!(
                "spin={n} iters ({spun} ms, budget {} used ok) ticks_during_call={ticks} events_queued_during_call={pushed} inbox_after={queued} edges_delivered={edges} dropped={dropped} cycles={}",
                2 * n + 1000,
                c1.wrapping_sub(c0)
            ),
        );
        let _ = dt;
    }

    /// 29. Select: button (GPIO 3) or radio, in both orders and in one batch;
    /// then radio against a timer, whose late completion must be ignored.
    async fn select(&mut self) {
        cap_take();
        let s0 = *self.k.exec_stats();
        let l = self.k.load(S, &img::s::MANIFEST);
        let mut ok = l.is_ok();
        if let Err(e) = l {
            self.report.push(format_args!("MO {} 29-select-load {e:?}", self.chip));
        }
        self.fut[3] = self.k.call(S, img::s::STAT, &[99]).unwrap_or(0);
        let st = |s: &mut Self, i| s.k.call(S, img::s::STAT, &[i]).unwrap_or(0);
        // Round 1: button first.
        ok &= self.run_until(50, |k| k.call(S, img::s::STAT, &[0]) == Ok(1)).await;
        let t = self.now();
        self.edge(t + 5, 3);
        ok &= self.run_until(50, |k| k.call(S, img::s::STAT, &[1]) == Ok(1)).await;
        let c1 = self.k.exec_stats().cancelled_pending - s0.cancelled_pending;
        // Round 2: radio first, then a late button press nobody waits for.
        ok &= self.run_until(100, |k| k.call(S, img::s::STAT, &[0]) == Ok(2)).await;
        let t = self.now();
        let un0 = self.k.exec_stats().edges_unmatched;
        self.packet(t + 5, b"radio");
        self.edge(t + 15, 3);
        ok &= self.run_until(50, |k| k.call(S, img::s::STAT, &[1]) == Ok(2)).await;
        self.run_for(15).await;
        let late_edge_ignored = self.k.exec_stats().edges_unmatched - un0;
        // Round 3: both in the same inbox batch.
        ok &= self.run_until(100, |k| k.call(S, img::s::STAT, &[0]) == Ok(3)).await;
        let t = self.now();
        let cd0 = self.k.exec_stats().cancelled_done;
        self.edge(t + 5, 3);
        self.packet(t + 5, b"both");
        ok &= self.run_until(50, |k| k.call(S, img::s::STAT, &[1]) == Ok(3)).await;
        let same_batch_discarded = self.k.exec_stats().cancelled_done - cd0;
        // Round 4: radio against a 10 ms timer; the packet comes 1 ms before
        // the timer is due, so the cancel reaches the drivers after it fired.
        ok &= self.run_until(100, |k| k.call(S, img::s::STAT, &[0]) == Ok(4)).await;
        let due = self
            .k
            .tickets_of(S)
            .find(|(_, t)| t.kind == mvm_kernel::exec::TKind::Timer)
            .map(|(_, t)| t.result)
            .unwrap_or(0);
        let stale0 = self.k.exec_stats().stale;
        let cd0 = self.k.exec_stats().cancelled_done;
        self.packet(due.wrapping_sub(1), b"race");
        ok &= self.run_until(50, |k| k.call(S, img::s::STAT, &[1]) == Ok(4)).await;
        self.run_for(5).await;
        let late_timer_ignored = self.k.exec_stats().stale - stale0;
        let timer_done_cancelled = self.k.exec_stats().cancelled_done - cd0;
        let log = st(self, 2).to_le_bytes();
        let printed = cap_take();
        let want_print = "select: gpio\nselect: radio\nselect: gpio\nselect: radio\n";
        ok &= &log == b"GRGR"
            && c1 >= 1
            && late_edge_ignored == 1
            && same_batch_discarded == 1
            && late_timer_ignored + timer_done_cancelled == 1
            && printed.as_str() == want_print
            && self.k.live_tickets_of(S) <= 1;
        let log_s = core::str::from_utf8(&log).unwrap_or("?");
        self.check(
            "29-select",
            ok,
            format_args!(
                "winners={log_s} loser_cancelled={c1} late_edge_ignored={late_edge_ignored} same_batch_loser_discarded={same_batch_discarded} late_timer_ignored={late_timer_ignored} timer_done_before_cancel={timer_done_cancelled}"
            ),
        );
    }

    fn cost_line(&mut self, name: &str, n: u32, c0: u32, c1: u32) {
        let chip = self.chip;
        self.report.push(format_args!("MO {chip} cost {name} n={n} cycles={} steps=0", c1.wrapping_sub(c0)));
    }

    /// Time one section with exactly two counter reads (Unicorn relies on it).
    fn timed<R>(&mut self, name: &str, n: u32, f: impl FnOnce(&mut Kernel<'a>) -> R) -> R {
        let c0 = (self.cycles)();
        let r = f(&mut self.k);
        let c1 = (self.cycles)();
        let chip = self.chip;
        self.report.push(format_args!("MO {chip} cost {name} n={n} cycles={} steps=0", c1.wrapping_sub(c0)));
        r
    }

    /// Costs: ticket ops, one select poll, one wake-and-poll round trip.
    async fn costs(&mut self) {
        self.pause(true).await;
        let n = self.n;
        let reps = 16;
        let pend = self.k.call(X, img::x::START, &[1_000_000]).unwrap_or(0);
        self.timed("a_call_empty", reps, |k| {
            for _ in 0..reps {
                let _ = k.call(X, img::x::EMPTY_LOOP, &[0]);
            }
        });
        let e = self.timed("a_empty_loop", n, |k| k.call(X, img::x::EMPTY_LOOP, &[n]));
        let ch = self.timed("a_check_loop", n, |k| k.call(X, img::x::CHECK_LOOP, &[pend, n]));
        let s = self.timed("a_start16", 16, |k| k.call(X, img::x::START16, &[]));
        let c = self.timed("a_cancel16", 16, |k| k.call(X, img::x::CANCEL16, &[]));
        let p = self.timed("a_select_poll", n, |k| {
            let mut r = Ok(0);
            for _ in 0..n {
                r = k.poll_task(X, 1);
            }
            r
        });
        let w0 = self.k.call(X, img::x::WAKES, &[]).unwrap_or(0);
        self.timed("a_wake_poll", n, |k| {
            for _ in 0..n {
                k.deliver(Event::Edge { pin: 7, edge: RISING as u8 });
            }
        });
        let w = self.k.call(X, img::x::WAKES, &[]).unwrap_or(0) - w0;
        let _ = self.k.call(X, img::x::CANCEL, &[pend]);
        self.pause(false).await;
        let ok = pend != 0 && e.is_ok() && ch == Ok(0) && s == Ok(16) && c == Ok(0) && p == Ok(0) && w == n;
        self.check(
            "async-costs",
            ok,
            format_args!("start16={s:?} cancel16={c:?} select_poll={p:?} wakes={w}"),
        );
    }

    /// RAM: tickets, task slots, inbox, futures as laid out.
    fn sizes(&mut self) {
        let [f0, f1, t, s] = self.fut;
        self.check(
            "async-sizes",
            true,
            format_args!(
                "ticket_entry={} tickets={} inbox_entry={} inbox={} request_entry={} requests={} io_total={} applet_record={} future_hello={f0} future_recv={f1} future_ticker={t} future_select={s}",
                core::mem::size_of::<mvm_kernel::TicketSlot>(),
                MAX_TICKETS,
                core::mem::size_of::<Event>(),
                mvm_kernel::io::INBOX,
                core::mem::size_of::<Request>(),
                mvm_kernel::io::REQUESTS,
                core::mem::size_of::<Io>(),
                core::mem::size_of::<mvm_kernel::Applet>()
            ),
        );
    }

    /// 30 (emulators). Inputs typed by an outside script on the UART: the
    /// select applet again, driven by a person-like script instead of the
    /// test hook. The script waits for "script-ready", then sends a button
    /// press and a packet, 200 ms apart.
    async fn script(&mut self) {
        cap_take();
        let l = self.k.load(S2, &img::s::MANIFEST);
        self.run_for(5).await;
        self.k.console.write(b"MO script-ready\n");
        let _ = cap_take();
        let done = self
            .run_until(3_000, |k| k.call(S2, img::s::STAT, &[1]).unwrap_or(0) >= 2)
            .await;
        let log = self.k.call(S2, img::s::STAT, &[2]).unwrap_or(0).to_le_bytes();
        let ok = l.is_ok() && done && log[0] == b'G' && log[1] == b'R';
        let log_s = core::str::from_utf8(&log[..2]).unwrap_or("?");
        let mut line = Line::default();
        let _ = write!(line, "winners={log_s}");
        self.check("30-script-inputs", ok, format_args!("{} (button, then packet, typed on the UART)", line.as_str()));
    }
}
