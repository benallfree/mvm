#![no_std]
//! Round 7d: the guest side of the async model, written by hand (no macros,
//! no heap, no guest executor). See ASYNC.md.
//!
//! - [`Ticket`]: a future over a kernel ticket. Each poll is one
//!   `kernel.ticket.check.v1`; `Ready` takes the result (the kernel frees the
//!   ticket). Dropping an unfinished ticket cancels it.
//! - [`Recv`]: a receive ticket that also holds the borrow of its buffer,
//!   so the buffer can't be touched or freed while the kernel may write it.
//! - [`Select2`]: two ticket futures; the first ready wins, the other is
//!   cancelled (`kernel.ticket.cancel.v1`).
//! - [`TaskSlot`]: fixed storage for one task's future, pinned in place in
//!   a static, polled by the applet's `poll(task)` export.
//!
//! The waker is unused: the kernel already knows which task owns each
//! ticket, and polls that task when the ticket completes.
//!
//! Import slots: every async applet's import table starts with [`PREFIX`]
//! (in that order); its own imports follow from slot [`FIRST_FREE_SLOT`].

use core::future::Future;
use core::marker::PhantomData;
use core::mem::{align_of, size_of, MaybeUninit};
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use mvm_abi::asyncabi::{DONE, PENDING, POLL_DONE, POLL_PENDING};
use mvm_abi::names;

/// The first imports of every async applet, in this order.
pub const PREFIX: [&str; 7] = [
    names::TICKET_CHECK,
    names::TICKET_CANCEL,
    names::TASK_SPAWN,
    names::TIMER_START,
    names::RADIO_SEND,
    names::RADIO_RECV,
    names::GPIO_WAIT,
];
pub mod slot {
    pub const CHECK: u32 = 0;
    pub const CANCEL: u32 = 1;
    pub const SPAWN: u32 = 2;
    pub const TIMER: u32 = 3;
    pub const SEND: u32 = 4;
    pub const RECV: u32 = 5;
    pub const GPIO: u32 = 6;
}
pub const FIRST_FREE_SLOT: u32 = 7;

/// A mesh op: `ecall` with the slot in a7, arguments in a0..a2; (a0, a1) back.
#[inline(always)]
pub fn op2(slot: u32, a0: u32, a1: u32, a2: u32) -> (u32, u32) {
    #[cfg(target_arch = "riscv32")]
    unsafe {
        let (r0, r1): (u32, u32);
        core::arch::asm!(
            "ecall",
            inlateout("a0") a0 => r0,
            inlateout("a1") a1 => r1,
            in("a2") a2,
            in("a7") slot,
        );
        (r0, r1)
    }
    #[cfg(not(target_arch = "riscv32"))]
    {
        let _ = (slot, a0, a1, a2);
        unreachable!("mesh ops only exist in the guest")
    }
}

/// Why a ticket future ended without a result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The kernel refused to start the work (no ticket).
    Refused,
    /// The kernel no longer knows the ticket (cancelled, faulted, stale).
    Invalid,
}

/// `kernel.task.spawn.v1`: task `id` exists; the kernel polls it soon.
pub fn spawn(id: u32) -> bool {
    op2(slot::SPAWN, id, 0, 0).0 == 1
}

/// A future over one kernel ticket.
pub struct Ticket {
    id: u32,
}

impl Ticket {
    pub const fn from_id(id: u32) -> Self {
        Ticket { id }
    }
    pub fn id(&self) -> u32 {
        self.id
    }
    /// Cancel now (a later completion is ignored by the kernel). Safe to call
    /// on a finished ticket.
    pub fn cancel(&mut self) {
        if self.id != 0 {
            op2(slot::CANCEL, self.id, 0, 0);
            self.id = 0;
        }
    }
    fn check(&mut self) -> Poll<Result<u32, Error>> {
        if self.id == 0 {
            return Poll::Ready(Err(Error::Refused));
        }
        match op2(slot::CHECK, self.id, 0, 0) {
            (PENDING, _) => Poll::Pending,
            (DONE, r) => {
                self.id = 0; // taken: the kernel freed it
                Poll::Ready(Ok(r))
            }
            _ => {
                self.id = 0;
                Poll::Ready(Err(Error::Invalid))
            }
        }
    }
}

impl Future for Ticket {
    type Output = Result<u32, Error>;
    fn poll(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
        self.check()
    }
}

/// Dropping a ticket that hasn't finished cancels it, so a future that is
/// abandoned (a select loser, a task torn down) never leaves work running.
impl Drop for Ticket {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// `kernel.timer.start.v1`: ready after `ms` milliseconds (result: the
/// drivers' clock when it fired).
pub fn timer(ms: u32) -> Ticket {
    Ticket::from_id(op2(slot::TIMER, ms, 0, 0).0)
}

/// `kernel.radio.send.v1`: the kernel copies `packet` now, so the buffer
/// is free again as soon as this returns. Ready when the send completes.
pub fn send(packet: &[u8]) -> Ticket {
    Ticket::from_id(op2(slot::SEND, packet.as_ptr() as u32, packet.len() as u32, 0).0)
}

/// `kernel.gpio.wait.v1`: ready on an edge of `pin` (result: the edge).
pub fn gpio_wait(pin: u32, edges: u32) -> Ticket {
    Ticket::from_id(op2(slot::GPIO, pin, edges, 0).0)
}

/// A receive: the kernel writes the packet into `buf` between polls, so the
/// future keeps `buf` mutably borrowed until it finishes or is dropped (and
/// dropping cancels first, so the kernel never writes a freed buffer).
pub struct Recv<'a> {
    t: Ticket,
    _buf: PhantomData<&'a mut [u8]>,
}

/// `kernel.radio.recv.v1`: ready with the packet length once a packet has
/// been copied into `buf`.
pub fn recv(buf: &mut [u8]) -> Recv<'_> {
    Recv {
        t: Ticket::from_id(op2(slot::RECV, buf.as_mut_ptr() as u32, buf.len() as u32, 0).0),
        _buf: PhantomData,
    }
}

impl Future for Recv<'_> {
    type Output = Result<u32, Error>;
    fn poll(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
        self.t.check()
    }
}

/// Futures [`Select2`] can cancel when they lose.
pub trait Cancel {
    fn cancel(&mut self);
}
impl Cancel for Ticket {
    fn cancel(&mut self) {
        Ticket::cancel(self)
    }
}
impl Cancel for Recv<'_> {
    fn cancel(&mut self) {
        self.t.cancel()
    }
}

pub enum Either<A, B> {
    First(A),
    Second(B),
}

/// Wait for whichever of two tickets finishes first; cancel the other.
///
/// Both are checked on every poll, `a` first: when both completed in the same
/// inbox batch, `a` wins and `b` is cancelled with its result discarded (for a
/// receive, that packet is lost). See ASYNC.md.
pub struct Select2<A, B> {
    a: A,
    b: B,
}

pub fn select2<A, B>(a: A, b: B) -> Select2<A, B> {
    Select2 { a, b }
}

impl<A, B> Future for Select2<A, B>
where
    A: Future + Cancel + Unpin,
    B: Future + Cancel + Unpin,
{
    type Output = Either<A::Output, B::Output>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = &mut *self;
        if let Poll::Ready(r) = Pin::new(&mut this.a).poll(cx) {
            this.b.cancel();
            return Poll::Ready(Either::First(r));
        }
        if let Poll::Ready(r) = Pin::new(&mut this.b).poll(cx) {
            this.a.cancel();
            return Poll::Ready(Either::Second(r));
        }
        Poll::Pending
    }
}

/// Storage for one task's future: `N` bytes, 8-aligned, in a static. The
/// future is written in place by [`TaskSlot::spawn`] and never moves after
/// (that is the pinning guarantee), and is dropped in place when it finishes.
#[repr(C, align(8))]
pub struct TaskSlot<const N: usize> {
    buf: MaybeUninit<[u8; N]>,
    poll_fn: Option<unsafe fn(*mut u8) -> Poll<()>>,
    drop_fn: Option<unsafe fn(*mut u8)>,
    /// `size_of` the future stored (for the report).
    pub size: u32,
}

impl<const N: usize> Default for TaskSlot<N> {
    fn default() -> Self {
        Self::new()
    }
}

unsafe fn poll_raw<F: Future<Output = ()>>(p: *mut u8) -> Poll<()> {
    // SAFETY: `p` holds an `F` written by `spawn`, which never moves.
    let f = unsafe { Pin::new_unchecked(&mut *(p as *mut F)) };
    f.poll(&mut Context::from_waker(Waker::noop()))
}

unsafe fn drop_raw<F>(p: *mut u8) {
    unsafe { core::ptr::drop_in_place(p as *mut F) }
}

impl<const N: usize> TaskSlot<N> {
    pub const fn new() -> Self {
        TaskSlot {
            buf: MaybeUninit::uninit(),
            poll_fn: None,
            drop_fn: None,
            size: 0,
        }
    }

    /// Store `f` and tell the kernel task `id` exists. Refuses (at compile
    /// time) a future bigger than the slot.
    pub fn spawn<F: Future<Output = ()>>(&mut self, id: u32, f: F) -> bool {
        const { assert!(size_of::<F>() <= N && align_of::<F>() <= 8, "future too big for its TaskSlot") };
        if self.poll_fn.is_some() {
            return false;
        }
        // SAFETY: the buffer is big and aligned enough (checked above) and free.
        unsafe { (self.buf.as_mut_ptr() as *mut F).write(f) };
        self.poll_fn = Some(poll_raw::<F>);
        self.drop_fn = Some(drop_raw::<F>);
        self.size = size_of::<F>() as u32;
        spawn(id)
    }

    /// Poll the stored future once: `POLL_DONE` (and dropped) or `POLL_PENDING`.
    pub fn poll(&mut self) -> u32 {
        let Some(f) = self.poll_fn else {
            return POLL_DONE;
        };
        let p = self.buf.as_mut_ptr() as *mut u8;
        // SAFETY: poll_fn matches the future stored at `p`.
        match unsafe { f(p) } {
            Poll::Pending => POLL_PENDING,
            Poll::Ready(()) => {
                if let Some(d) = self.drop_fn.take() {
                    // SAFETY: the future is finished and never polled again.
                    unsafe { d(p) };
                }
                self.poll_fn = None;
                POLL_DONE
            }
        }
    }
}
