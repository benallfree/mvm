//! Round 7d: the only state shared between driver (interrupt) context and the
//! kernel (the interpreter task). See ASYNC.md.
//!
//! - [`Ring`]: a fixed-size single-producer, single-consumer ring. No locks:
//!   the producer only writes `tail`, the consumer only writes `head`.
//! - `Io::inbox`: drivers -> kernel (completion events). Producer: driver
//!   context. Consumer: the kernel's executor.
//! - `Io::requests`: kernel -> drivers (start a timer, send a packet, test
//!   injections). Producer: the kernel / test runner. Consumer: drivers.
//! - `Io::now_ms`: the drivers' millisecond clock (written by the tick).
//!
//! Interrupts never call guest code and never touch kernel state: they only
//! push events. The kernel never touches driver state: it only pushes
//! requests.

use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

pub const INBOX: usize = 16;
pub const REQUESTS: usize = 32;
pub const SENT_LOG: usize = 32;
pub use mvm_abi::asyncabi::MAX_PACKET;

/// Single-producer single-consumer ring of `N` entries.
pub struct Ring<T: Copy, const N: usize> {
    buf: UnsafeCell<[MaybeUninit<T>; N]>,
    /// Next entry to read (written by the consumer only).
    head: AtomicU32,
    /// Next entry to write (written by the producer only).
    tail: AtomicU32,
}

// SAFETY: one producer and one consumer, each confined to one context; the
// Release/Acquire pair on head/tail orders the slot write before its read.
unsafe impl<T: Copy + Send, const N: usize> Sync for Ring<T, N> {}

impl<T: Copy, const N: usize> Default for Ring<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Copy, const N: usize> Ring<T, N> {
    pub const fn new() -> Self {
        Ring {
            buf: UnsafeCell::new([const { MaybeUninit::uninit() }; N]),
            head: AtomicU32::new(0),
            tail: AtomicU32::new(0),
        }
    }

    /// Producer side. False (and nothing written) when full.
    pub fn push(&self, v: T) -> bool {
        let t = self.tail.load(Ordering::Relaxed);
        let h = self.head.load(Ordering::Acquire);
        if t.wrapping_sub(h) as usize >= N {
            return false;
        }
        // SAFETY: slot `t` is outside head..tail, so the consumer isn't reading it.
        unsafe { (*self.buf.get())[t as usize % N] = MaybeUninit::new(v) };
        self.tail.store(t.wrapping_add(1), Ordering::Release);
        true
    }

    /// Consumer side.
    pub fn pop(&self) -> Option<T> {
        let h = self.head.load(Ordering::Relaxed);
        let t = self.tail.load(Ordering::Acquire);
        if h == t {
            return None;
        }
        // SAFETY: slot `h` is inside head..tail: written and published.
        let v = unsafe { (*self.buf.get())[h as usize % N].assume_init() };
        self.head.store(h.wrapping_add(1), Ordering::Release);
        Some(v)
    }

    pub fn len(&self) -> usize {
        let t = self.tail.load(Ordering::Acquire);
        t.wrapping_sub(self.head.load(Ordering::Acquire)) as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A radio packet (fixed buffer, `len` bytes used).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Packet {
    pub len: u8,
    pub data: [u8; MAX_PACKET],
}

impl Packet {
    pub fn new(b: &[u8]) -> Self {
        let n = b.len().min(MAX_PACKET);
        let mut data = [0; MAX_PACKET];
        data[..n].copy_from_slice(&b[..n]);
        Packet { len: n as u8, data }
    }
    pub fn bytes(&self) -> &[u8] {
        &self.data[..self.len as usize]
    }
}

/// Drivers -> kernel.
#[derive(Clone, Copy, Debug)]
pub enum Event {
    /// The work behind `ticket` finished (a timer fired, a send completed).
    Done { ticket: u32, result: u32 },
    /// A packet arrived.
    Packet(Packet),
    /// A GPIO edge (`mvm_abi::asyncabi::RISING` / `FALLING`).
    Edge { pin: u8, edge: u8 },
}

/// A fake input the drivers deliver at `at` ms (the test hook).
#[derive(Clone, Copy, Debug)]
pub enum Inject {
    Packet(Packet),
    Edge { pin: u8, edge: u8 },
}

/// Kernel -> drivers.
#[derive(Clone, Copy, Debug)]
pub enum Request {
    StartTimer { ticket: u32, due: u32 },
    CancelTimer { ticket: u32 },
    Send { ticket: u32, packet: Packet },
    Inject { at: u32, what: Inject },
}

/// One packet the fake radio sent, for the tests: when it was handed to the
/// radio and when the send completed.
#[derive(Clone, Copy, Debug)]
pub struct Sent {
    pub start_ms: u32,
    pub done_ms: u32,
    pub packet: Packet,
}

/// Counters the drivers keep (all monotonic).
#[derive(Default)]
pub struct DriverStats {
    /// Inbound packets dropped because the inbox was full.
    pub rx_dropped: AtomicU32,
    /// Edges dropped because the inbox was full.
    pub edges_dropped: AtomicU32,
    /// Completions that found the inbox full and were retried on a later tick
    /// (never dropped: the driver keeps them).
    pub deferred: AtomicU32,
    /// Events pushed into the inbox.
    pub pushed: AtomicU32,
    /// Ticks (milliseconds) serviced.
    pub ticks: AtomicU32,
    /// Ticks that ran while guest code was running (see `Io::guest_running`).
    pub ticks_during_guest: AtomicU32,
    /// Events pushed while guest code was running.
    pub pushed_during_guest: AtomicU32,
    /// Requests the drivers couldn't hold (timer or send table full).
    pub refused: AtomicU32,
}

/// Everything shared between driver context and the kernel. One static per target.
pub struct Io {
    pub inbox: Ring<Event, INBOX>,
    pub requests: Ring<Request, REQUESTS>,
    /// Sends, in order (drivers -> test runner).
    pub sent: Ring<Sent, SENT_LOG>,
    pub now_ms: AtomicU32,
    pub stats: DriverStats,
    /// Set by the test runner to quiesce the drivers (cost measurements push
    /// into the inbox themselves); the drivers acknowledge in `paused_ack`.
    pub pause: AtomicBool,
    pub paused_ack: AtomicBool,
    /// Set by the kernel around every guest call (for test 10).
    pub guest_running: AtomicBool,
}

impl Default for Io {
    fn default() -> Self {
        Self::new()
    }
}

impl Io {
    pub const fn new() -> Self {
        Io {
            inbox: Ring::new(),
            requests: Ring::new(),
            sent: Ring::new(),
            now_ms: AtomicU32::new(0),
            stats: DriverStats {
                rx_dropped: AtomicU32::new(0),
                edges_dropped: AtomicU32::new(0),
                deferred: AtomicU32::new(0),
                pushed: AtomicU32::new(0),
                ticks: AtomicU32::new(0),
                ticks_during_guest: AtomicU32::new(0),
                pushed_during_guest: AtomicU32::new(0),
                refused: AtomicU32::new(0),
            },
            pause: AtomicBool::new(false),
            paused_ack: AtomicBool::new(false),
            guest_running: AtomicBool::new(false),
        }
    }

    pub fn now(&self) -> u32 {
        self.now_ms.load(Ordering::Acquire)
    }
}
