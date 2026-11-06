//! Round 7d: the kernel as executor. See ASYNC.md.
//!
//! - **Tickets**: async ops start work and return a ticket at once. A ticket
//!   id is `generation << 8 | slot`; the slot's generation advances every time
//!   it's handed out, so a stale id never matches a newer ticket.
//! - **Tasks**: up to 8 per applet. `kernel.task.spawn.v1` marks a task as
//!   existing and ready; the kernel calls the applet's `poll(task)` export
//!   once per batch in which one of the task's tickets completed.
//! - **Executor** ([`Kernel::run_once`]): drain the inbox (one batch), mark
//!   tickets done and their owner tasks ready, then poll each ready task
//!   once. Nothing waiting means nothing runs.
//! - **Budget**: every `init`/`poll` runs under an instruction budget (in
//!   control transfers, see `interp::fast::Cpu::budget`); running out
//!   faults the applet: its tasks stop and its tickets are freed.
//!
//! Rule: no kernel op awaits while guest code is paused under it. Ops only
//! record a ticket and push a request; completions come back through the
//! inbox, between guest calls.

use crate::io::{Event, Packet, Request, MAX_PACKET};
use crate::{Error, Kernel, MAX_APPLETS};
use mvm_abi::asyncabi::{DONE, INVALID, MAX_TASKS, PENDING, POLL_DONE, POLL_PENDING};

pub const MAX_TICKETS: usize = 32;
/// Packets that arrived with no receive pending, held for the next receive.
pub const BACKLOG: usize = 4;
/// "No task": tickets made outside a poll (init, a host call) wake nobody.
pub const NO_TASK: u8 = 0xFF;
/// Default `init`/`poll` budget, in control transfers (jumps and branches):
/// roughly 10,000 guest instructions, about 5 ms of interpreted code on a
/// 64 MHz Cortex-M4. That is also how late a runaway can make every other
/// applet (there is one interpreter): round 7d first tried 20,000 (~47 ms),
/// and test 25 showed the other applet's ticks slipping by that much.
pub const DEFAULT_BUDGET: u32 = 2_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TState {
    Free,
    Pending,
    Done,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TKind {
    Timer,
    Send,
    /// Guest buffer to copy the packet into.
    Recv { ptr: u32, cap: u32 },
    Gpio { pin: u8, edges: u8 },
}

/// One ticket-table entry.
#[derive(Clone, Copy, Debug)]
pub struct TicketSlot {
    pub gen: u32,
    pub state: TState,
    pub kind: TKind,
    pub applet: u8,
    pub task: u8,
    pub result: u32,
    /// Creation order (receives are served oldest first).
    pub seq: u32,
}

impl TicketSlot {
    pub const EMPTY: TicketSlot = TicketSlot {
        gen: 0,
        state: TState::Free,
        kind: TKind::Timer,
        applet: 0,
        task: NO_TASK,
        result: 0,
        seq: 0,
    };
}

/// Executor counters (monotonic).
#[derive(Clone, Copy, Default, Debug)]
pub struct ExecStats {
    pub batches: u32,
    pub events: u32,
    pub polls: u32,
    /// Polls per applet and task.
    pub polls_by: [[u32; MAX_TASKS as usize]; MAX_APPLETS],
    /// Guest calls of any kind (init, poll, host calls).
    pub guest_calls: u32,
    /// Completions for tickets that were already freed (cancelled, taken,
    /// faulted): ignored.
    pub stale: u32,
    pub edges_unmatched: u32,
    /// Packets with no receive pending that went to the backlog.
    pub backlogged: u32,
    /// Packets dropped because the backlog was full too.
    pub backlog_dropped: u32,
    /// Tickets cancelled after they had already completed (result discarded).
    pub cancelled_done: u32,
    pub cancelled_pending: u32,
    pub faults: u32,
    /// Ops refused because the ticket table or request ring was full.
    pub refused: u32,
}

pub(crate) struct ExecState {
    pub tickets: [TicketSlot; MAX_TICKETS],
    pub seq: u32,
    pub backlog: [Option<Packet>; BACKLOG],
    pub budget_left: u32,
    /// The applet whose code made the current ecall.
    pub caller: u8,
    /// The task being polled (applet, task), if any.
    pub cur: Option<(u8, u8)>,
    pub stats: ExecStats,
}

impl ExecState {
    pub const fn new() -> Self {
        ExecState {
            tickets: [TicketSlot::EMPTY; MAX_TICKETS],
            seq: 0,
            backlog: [None; BACKLOG],
            budget_left: u32::MAX,
            caller: 0,
            cur: None,
            stats: ExecStats {
                batches: 0,
                events: 0,
                polls: 0,
                polls_by: [[0; MAX_TASKS as usize]; MAX_APPLETS],
                guest_calls: 0,
                stale: 0,
                edges_unmatched: 0,
                backlogged: 0,
                backlog_dropped: 0,
                cancelled_done: 0,
                cancelled_pending: 0,
                faults: 0,
                refused: 0,
            },
        }
    }
}

fn id_of(slot: usize, gen: u32) -> u32 {
    (gen << 8) | slot as u32
}

impl<'a> Kernel<'a> {
    pub fn exec_stats(&self) -> &ExecStats {
        &self.x.stats
    }

    pub fn ticket(&self, id: u32) -> Option<&TicketSlot> {
        let t = self.x.tickets.get((id & 0xFF) as usize)?;
        (t.gen == id >> 8 && t.state != TState::Free).then_some(t)
    }

    /// Anything for the executor to do: events waiting, or tasks ready.
    pub fn has_work(&self) -> bool {
        !self.io.inbox.is_empty()
            || self.applets.iter().flatten().any(|a| a.ready & a.spawned != 0 && !a.faulted)
    }

    /// The live tickets owned by `applet`: (id, slot).
    pub fn tickets_of(&self, applet: u8) -> impl Iterator<Item = (u32, &TicketSlot)> {
        self.x
            .tickets
            .iter()
            .enumerate()
            .filter(move |(_, t)| t.state != TState::Free && t.applet == applet)
            .map(|(i, t)| (id_of(i, t.gen), t))
    }

    pub fn live_tickets(&self) -> usize {
        self.x.tickets.iter().filter(|t| t.state != TState::Free).count()
    }

    pub fn live_tickets_of(&self, applet: u8) -> usize {
        self.x
            .tickets
            .iter()
            .filter(|t| t.state != TState::Free && t.applet == applet)
            .count()
    }

    /// A new ticket owned by the calling applet (and the polled task, if the
    /// caller is the applet being polled). 0 when the table is full.
    fn new_ticket(&mut self, kind: TKind) -> u32 {
        let applet = self.x.caller;
        let task = match self.x.cur {
            Some((a, t)) if a == applet => t,
            _ => NO_TASK,
        };
        let Some(i) = self.x.tickets.iter().position(|t| t.state == TState::Free) else {
            self.x.stats.refused += 1;
            return 0;
        };
        let t = &mut self.x.tickets[i];
        t.gen = (t.gen + 1) & 0x00FF_FFFF;
        if t.gen == 0 {
            t.gen = 1;
        }
        self.x.seq = self.x.seq.wrapping_add(1);
        *t = TicketSlot {
            gen: t.gen,
            state: TState::Pending,
            kind,
            applet,
            task,
            result: 0,
            seq: self.x.seq,
        };
        id_of(i, t.gen)
    }

    /// The live ticket `id`, if the calling applet owns it.
    fn own_ticket(&mut self, id: u32) -> Option<&mut TicketSlot> {
        let caller = self.x.caller;
        let t = self.x.tickets.get_mut((id & 0xFF) as usize)?;
        (t.gen == id >> 8 && t.state != TState::Free && t.applet == caller).then_some(t)
    }

    fn free_ticket(&mut self, i: usize) {
        let t = &mut self.x.tickets[i];
        if t.state != TState::Free && t.kind == TKind::Timer && t.state == TState::Pending {
            // Best effort: if the timer fires first, its completion is stale.
            let _ = self.io.requests.push(Request::CancelTimer {
                ticket: id_of(i, t.gen),
            });
        }
        t.state = TState::Free;
    }

    fn complete(&mut self, i: usize, result: u32) {
        self.complete_wake(i, result, true)
    }

    /// Mark ticket `i` done; `wake` makes its owner task ready (not wanted
    /// when the owner is running right now and checks the ticket itself).
    fn complete_wake(&mut self, i: usize, result: u32, wake: bool) {
        let t = &mut self.x.tickets[i];
        t.state = TState::Done;
        t.result = result;
        let (a, task) = (t.applet as usize, t.task);
        if wake && task != NO_TASK {
            if let Some(Some(ap)) = self.applets.get_mut(a) {
                ap.ready |= 1 << task;
            }
        }
    }

    /// Copy `p` into receive ticket `i`'s guest buffer and complete it.
    fn fill_recv(&mut self, i: usize, p: &Packet) {
        self.fill_recv_wake(i, p, true)
    }

    fn fill_recv_wake(&mut self, i: usize, p: &Packet, wake: bool) {
        let TKind::Recv { ptr, cap } = self.x.tickets[i].kind else {
            return;
        };
        let n = (p.len as u32).min(cap);
        let ok = match self.guest_bytes_mut(ptr, n) {
            Some(b) => {
                b.copy_from_slice(&p.data[..n as usize]);
                true
            }
            None => false,
        };
        self.complete_wake(i, if ok { n } else { u32::MAX }, wake);
    }

    /// The oldest pending receive.
    fn oldest_recv(&self) -> Option<usize> {
        self.x
            .tickets
            .iter()
            .enumerate()
            .filter(|(_, t)| t.state == TState::Pending && matches!(t.kind, TKind::Recv { .. }))
            .min_by_key(|(_, t)| t.seq)
            .map(|(i, _)| i)
    }

    /// Apply one inbox event.
    fn apply(&mut self, e: Event) {
        self.x.stats.events += 1;
        match e {
            Event::Done { ticket, result } => {
                let i = (ticket & 0xFF) as usize;
                match self.x.tickets.get(i) {
                    Some(t) if t.gen == ticket >> 8 && t.state == TState::Pending => {
                        self.complete(i, result)
                    }
                    _ => self.x.stats.stale += 1,
                }
            }
            Event::Packet(p) => match self.oldest_recv() {
                Some(i) => self.fill_recv(i, &p),
                None => match self.x.backlog.iter_mut().find(|b| b.is_none()) {
                    Some(b) => {
                        *b = Some(p);
                        self.x.stats.backlogged += 1;
                    }
                    None => self.x.stats.backlog_dropped += 1,
                },
            },
            Event::Edge { pin, edge } => {
                let mut hit = false;
                for i in 0..MAX_TICKETS {
                    let t = self.x.tickets[i];
                    if t.state == TState::Pending
                        && matches!(t.kind, TKind::Gpio { pin: p, edges } if p == pin && edges & edge != 0)
                    {
                        self.complete(i, edge as u32);
                        hit = true;
                    }
                }
                if !hit {
                    self.x.stats.edges_unmatched += 1;
                }
            }
        }
    }

    /// One executor batch: drain the inbox, then poll every ready task once.
    /// Returns the number of polls.
    pub fn run_once(&mut self) -> u32 {
        self.x.stats.batches += 1;
        while let Some(e) = self.io.inbox.pop() {
            self.apply(e);
        }
        let mut polls = 0;
        for a in 0..MAX_APPLETS {
            let Some(ap) = self.applets[a] else { continue };
            let ready = ap.ready & ap.spawned;
            if let Some(Some(ap)) = self.applets.get_mut(a) {
                ap.ready = 0;
            }
            for t in 0..MAX_TASKS as u8 {
                if ready & (1 << t) != 0 && !self.applets[a].is_some_and(|x| x.faulted) {
                    let _ = self.poll_task(a as u8, t);
                    polls += 1;
                }
            }
        }
        polls
    }

    /// Process one event directly (cost measurement: what a driver push plus
    /// `run_once` would do, without the ring).
    pub fn deliver(&mut self, e: Event) -> u32 {
        self.apply(e);
        self.run_once()
    }

    /// Call `applet`'s `poll(task)` once under the budget. Done ends the
    /// task (and frees its tickets); a fault ends the applet.
    pub fn poll_task(&mut self, applet: u8, task: u8) -> Result<u32, Error> {
        let ap = self.applets[applet as usize].ok_or(Error::BadAppletId(applet))?;
        if ap.poll == u32::MAX || ap.faulted {
            return Err(Error::BadAppletId(applet));
        }
        self.x.stats.polls += 1;
        self.x.stats.polls_by[applet as usize][task as usize & 7] += 1;
        self.x.cur = Some((applet, task));
        let budget = self.budget;
        let r = self.call_budgeted(applet, ap.poll, &[task as u32], budget);
        self.x.cur = None;
        match r {
            Ok(POLL_PENDING) => Ok(POLL_PENDING),
            Ok(POLL_DONE) => {
                if let Some(Some(a)) = self.applets.get_mut(applet as usize) {
                    a.spawned &= !(1 << task);
                }
                for i in 0..MAX_TICKETS {
                    let t = self.x.tickets[i];
                    if t.state != TState::Free && t.applet == applet && t.task == task {
                        self.free_ticket(i);
                    }
                }
                Ok(POLL_DONE)
            }
            Ok(v) => {
                self.fault(applet);
                Err(Error::BadPollResult(v))
            }
            Err(e) => {
                self.fault(applet);
                Err(e)
            }
        }
    }

    /// Fault applet `applet` (budget, bad poll result): no more polls or
    /// calls, its tickets freed.
    pub fn fault(&mut self, applet: u8) {
        self.x.stats.faults += 1;
        self.stop(applet);
    }

    /// Stop applet `applet` (as a fault does, without counting one): the
    /// tests retire applets this way so later tests start clean.
    pub fn stop(&mut self, applet: u8) {
        if let Some(Some(a)) = self.applets.get_mut(applet as usize) {
            a.faulted = true;
            a.spawned = 0;
            a.ready = 0;
        }
        for i in 0..MAX_TICKETS {
            if self.x.tickets[i].state != TState::Free && self.x.tickets[i].applet == applet {
                self.free_ticket(i);
            }
        }
    }

    /// Run `applet`'s code under `budget` control transfers.
    pub fn call_budgeted(
        &mut self,
        applet: u8,
        entry_off: u32,
        args: &[u32],
        budget: u32,
    ) -> Result<u32, Error> {
        self.x.budget_left = budget.max(1);
        let r = self.call_inner(applet, entry_off, args);
        self.x.budget_left = u32::MAX;
        r
    }
}

// ---- the async ops (native providers) ----

pub(crate) fn op_timer_start(k: &mut Kernel, a: [u32; 6]) -> Result<[u32; 2], Error> {
    let t = k.new_ticket(TKind::Timer);
    if t != 0 {
        let due = k.io.now().wrapping_add(a[0].max(1));
        // While pending, a timer ticket's result field holds its due time.
        k.x.tickets[(t & 0xFF) as usize].result = due;
        if !k.io.requests.push(Request::StartTimer { ticket: t, due }) {
            k.free_ticket((t & 0xFF) as usize);
            k.x.stats.refused += 1;
            return Ok([0, 0]);
        }
    }
    Ok([t, 0])
}

pub(crate) fn op_ticket_check(k: &mut Kernel, a: [u32; 6]) -> Result<[u32; 2], Error> {
    let Some(t) = k.own_ticket(a[0]) else {
        return Ok([INVALID, 0]);
    };
    match t.state {
        TState::Done => {
            let r = t.result;
            t.state = TState::Free; // taken: the id is dead from now on
            Ok([DONE, r])
        }
        _ => Ok([PENDING, 0]),
    }
}

pub(crate) fn op_ticket_cancel(k: &mut Kernel, a: [u32; 6]) -> Result<[u32; 2], Error> {
    let Some(t) = k.own_ticket(a[0]) else {
        return Ok([INVALID, 0]);
    };
    let done = t.state == TState::Done;
    if done {
        k.x.stats.cancelled_done += 1;
    } else {
        k.x.stats.cancelled_pending += 1;
    }
    k.free_ticket((a[0] & 0xFF) as usize);
    Ok([if done { DONE } else { PENDING }, 0])
}

pub(crate) fn op_task_spawn(k: &mut Kernel, a: [u32; 6]) -> Result<[u32; 2], Error> {
    let task = a[0];
    let id = k.x.caller as usize;
    match k.applets.get_mut(id) {
        Some(Some(ap)) if task < MAX_TASKS && ap.poll != u32::MAX => {
            ap.spawned |= 1 << task;
            ap.ready |= 1 << task;
            Ok([1, 0])
        }
        _ => Ok([0, 0]),
    }
}

pub(crate) fn op_radio_send(k: &mut Kernel, a: [u32; 6]) -> Result<[u32; 2], Error> {
    let (ptr, len) = (a[0], a[1].min(MAX_PACKET as u32));
    let packet = Packet::new(k.guest_bytes(ptr, len).ok_or(Error::BadPointer(ptr))?);
    let t = k.new_ticket(TKind::Send);
    if t != 0 && !k.io.requests.push(Request::Send { ticket: t, packet }) {
        k.free_ticket((t & 0xFF) as usize);
        k.x.stats.refused += 1;
        return Ok([0, 0]);
    }
    Ok([t, 0])
}

pub(crate) fn op_radio_recv(k: &mut Kernel, a: [u32; 6]) -> Result<[u32; 2], Error> {
    let (ptr, cap) = (a[0], a[1]);
    // The buffer must be writable guest RAM now (and is checked again on delivery).
    k.guest_bytes_mut(ptr, cap).ok_or(Error::BadPointer(ptr))?;
    let t = k.new_ticket(TKind::Recv { ptr, cap });
    if t != 0 {
        // A packet already waiting completes the receive at once; the caller
        // polls its new ticket straight away (mvm-rt's futures do).
        if let Some(p) = k.x.backlog.iter_mut().find_map(|b| b.take()) {
            // keep the backlog in arrival order
            k.x.backlog.rotate_left(1);
            k.fill_recv_wake((t & 0xFF) as usize, &p, false);
        }
    }
    Ok([t, 0])
}

pub(crate) fn op_gpio_wait(k: &mut Kernel, a: [u32; 6]) -> Result<[u32; 2], Error> {
    let t = k.new_ticket(TKind::Gpio {
        pin: a[0] as u8,
        edges: (a[1] & 3) as u8,
    });
    Ok([t, 0])
}
