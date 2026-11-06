//! Round 7d: the fake drivers (timer, radio, GPIO), run in driver context
//! only: a 1 ms tick from an interrupt (Embassy interrupt executor on the
//! chips, a thread on the host, an injected interrupt under Unicorn). They
//! own their state outright; the kernel reaches them only through
//! `Io::requests`, and they answer only through `Io::inbox`.
//!
//! Service order each tick (it matters for test 11): first deliver what's
//! due (timers, sends, injected inputs), then take new requests. So a
//! `CancelTimer` that arrives in the same tick its timer is due is too late:
//! the completion is already in the inbox, and the kernel ignores it (the
//! ticket's generation no longer matches).

use crate::io::{Event, Inject, Io, Packet, Request, Sent};
use core::sync::atomic::Ordering;

/// Fixed delay of a fake radio send.
pub const SEND_MS: u32 = 5;
const TIMERS: usize = 32;
const SENDS: usize = 8;
const INJECTS: usize = 64;

pub struct Drivers {
    /// (ticket, due ms)
    timers: [Option<(u32, u32)>; TIMERS],
    /// (ticket, done ms, start ms, packet)
    sends: [Option<(u32, u32, u32, Packet)>; SENDS],
    /// (at ms, input)
    injects: [Option<(u32, Inject)>; INJECTS],
    now: u32,
}

impl Default for Drivers {
    fn default() -> Self {
        Self::new()
    }
}

impl Drivers {
    pub const fn new() -> Self {
        Drivers {
            timers: [None; TIMERS],
            sends: [None; SENDS],
            injects: [None; INJECTS],
            now: 0,
        }
    }

    /// One millisecond. Returns true if anything was pushed into the inbox
    /// (the caller may then wake the interpreter task).
    pub fn tick(&mut self, io: &Io) -> bool {
        if io.pause.load(Ordering::Acquire) {
            io.paused_ack.store(true, Ordering::Release);
            return false;
        }
        io.paused_ack.store(false, Ordering::Release);
        self.now = self.now.wrapping_add(1);
        io.now_ms.store(self.now, Ordering::Release);
        io.stats.ticks.fetch_add(1, Ordering::Relaxed);
        let during_guest = io.guest_running.load(Ordering::Acquire);
        if during_guest {
            io.stats.ticks_during_guest.fetch_add(1, Ordering::Relaxed);
        }
        let before = io.stats.pushed.load(Ordering::Relaxed);
        self.deliver(io);
        self.take_requests(io);
        let pushed = io.stats.pushed.load(Ordering::Relaxed) - before;
        if during_guest {
            io.stats.pushed_during_guest.fetch_add(pushed, Ordering::Relaxed);
        }
        pushed > 0
    }

    fn push(io: &Io, e: Event) -> bool {
        let ok = io.inbox.push(e);
        if ok {
            io.stats.pushed.fetch_add(1, Ordering::Relaxed);
        }
        ok
    }

    /// Deliver an input now (driver context: the UART script hook on the
    /// emulators). Packets and edges come from outside, so when the inbox is
    /// full they're dropped and counted, as a real radio FIFO would.
    pub fn input(&mut self, io: &Io, what: Inject) -> bool {
        match what {
            Inject::Packet(p) => {
                let ok = Self::push(io, Event::Packet(p));
                if !ok {
                    io.stats.rx_dropped.fetch_add(1, Ordering::Relaxed);
                }
                ok
            }
            Inject::Edge { pin, edge } => {
                let ok = Self::push(io, Event::Edge { pin, edge });
                if !ok {
                    io.stats.edges_dropped.fetch_add(1, Ordering::Relaxed);
                }
                ok
            }
        }
    }

    fn deliver(&mut self, io: &Io) {
        let now = self.now;
        // Completions are the drivers' own state: when the inbox is full they
        // stay put and go out on a later tick (never lost).
        for t in self.timers.iter_mut() {
            if let Some((ticket, due)) = *t {
                if due.wrapping_sub(now) as i32 <= 0 {
                    if Self::push(io, Event::Done { ticket, result: now }) {
                        *t = None;
                    } else {
                        io.stats.deferred.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }
        for s in self.sends.iter_mut() {
            if let Some((ticket, done, start, packet)) = *s {
                if done.wrapping_sub(now) as i32 <= 0 {
                    if Self::push(io, Event::Done { ticket, result: packet.len as u32 }) {
                        *s = None;
                        let _ = io.sent.push(Sent {
                            start_ms: start,
                            done_ms: now,
                            packet,
                        });
                    } else {
                        io.stats.deferred.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }
        // Inputs, in schedule order for equal times (the table is filled in order).
        for k in 0..INJECTS {
            if let Some((at, what)) = self.injects[k] {
                if at.wrapping_sub(now) as i32 <= 0 {
                    self.injects[k] = None;
                    self.input(io, what);
                }
            }
        }
    }

    fn take_requests(&mut self, io: &Io) {
        while let Some(r) = io.requests.pop() {
            let ok = match r {
                Request::StartTimer { ticket, due } => put(&mut self.timers, (ticket, due)),
                Request::CancelTimer { ticket } => {
                    for t in self.timers.iter_mut() {
                        if matches!(t, Some((k, _)) if *k == ticket) {
                            *t = None;
                        }
                    }
                    true
                }
                Request::Send { ticket, packet } => {
                    put(&mut self.sends, (ticket, self.now + SEND_MS, self.now, packet))
                }
                Request::Inject { at, what } => put(&mut self.injects, (at, what)),
            };
            if !ok {
                io.stats.refused.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

fn put<T: Copy>(table: &mut [Option<T>], v: T) -> bool {
    match table.iter_mut().find(|s| s.is_none()) {
        Some(s) => {
            *s = Some(v);
            true
        }
        None => false,
    }
}
