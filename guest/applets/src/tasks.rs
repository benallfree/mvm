//! Round 7d applets: async tasks, written by hand on `mvm-rt` (no macros).
//!
//! Each applet exports `<x>_init()` (spawns its tasks) and `<x>_poll(task)`
//! (polls that task's future once). Futures live in `TaskSlot` statics.
//! Import tables start with `mvm_rt::PREFIX`; extra imports follow.

#![allow(unused_unsafe)]

use crate::{import, Imports};
use core::ptr::{addr_of, addr_of_mut};
use mvm_abi::asyncabi::{ANY_EDGE, RISING};
use mvm_rt::{gpio_wait, op2, recv, select2, send, timer, Either, TaskSlot, PREFIX};

const SLOT_OWN: u32 = mvm_rt::FIRST_FREE_SLOT;

macro_rules! prefix_and {
    ($($extra:expr),*) => {
        Imports::new([
            import(PREFIX[0]), import(PREFIX[1]), import(PREFIX[2]), import(PREFIX[3]),
            import(PREFIX[4]), import(PREFIX[5]), import(PREFIX[6]), $(import($extra)),*
        ])
    };
}

fn print(s: &[u8]) {
    op2(SLOT_OWN, s.as_ptr() as u32, s.len() as u32, 0);
}

/// Read a static through a volatile load (the kernel and other polls change
/// what futures see between polls).
macro_rules! get {
    ($s:expr) => {
        unsafe { core::ptr::read_volatile(addr_of!($s)) }
    };
}
macro_rules! set {
    ($s:expr, $v:expr) => {
        unsafe { core::ptr::write_volatile(addr_of_mut!($s), $v) }
    };
}

// ---- F: a hello loop and a receive loop in one applet ----
#[no_mangle]
pub static F_IMPORTS: Imports<8> = prefix_and!(crate::names::UART_WRITE);
static mut F_TASKS: [TaskSlot<160>; 2] = [TaskSlot::new(), TaskSlot::new()];
static mut F_SENT: u32 = 0;
static mut F_RECEIVED: u32 = 0;
static mut F_LAST: [u8; 4] = [0; 4];

async fn hello_loop() {
    for i in 0..5u8 {
        let _ = timer(50).await;
        let msg = [b'h', b'e', b'l', b'l', b'o', b'0' + i];
        if send(&msg).await.is_ok() {
            set!(F_SENT, get!(F_SENT) + 1);
        }
    }
}

async fn recv_loop() {
    let mut buf = [0u8; 32];
    loop {
        if let Ok(n) = recv(&mut buf).await {
            set!(F_RECEIVED, get!(F_RECEIVED) + 1);
            let mut last = [0u8; 4];
            let k = (n as usize).min(4);
            last[..k].copy_from_slice(&buf[..k]);
            set!(F_LAST, last);
        }
    }
}

#[no_mangle]
pub extern "C" fn f_init() -> u32 {
    let t = unsafe { &mut *addr_of_mut!(F_TASKS) };
    t[0].spawn(0, hello_loop()) as u32 + t[1].spawn(1, recv_loop()) as u32
}

#[no_mangle]
pub extern "C" fn f_poll(task: u32) -> u32 {
    unsafe { (*addr_of_mut!(F_TASKS))[task as usize & 1].poll() }
}

/// 0 sends done, 1 packets received, 2 first 4 bytes of the last packet,
/// 3/4 the futures' sizes as Rust laid them out.
#[no_mangle]
pub extern "C" fn f_stat(i: u32) -> u32 {
    match i {
        0 => get!(F_SENT),
        1 => get!(F_RECEIVED),
        2 => u32::from_le_bytes(get!(F_LAST)),
        3 => unsafe { (*addr_of!(F_TASKS))[0].size },
        _ => unsafe { (*addr_of!(F_TASKS))[1].size },
    }
}

// ---- T1, T2: timer loops at different intervals ----
// Each tick also calls test.echo.v1 (A's guest code, a nested call) while
// other tasks have tickets pending: test 24.
async fn ticker(ms: u32, ticks: *mut u32, times: *mut [u32; 16], echo: *mut u32, stop: *const u32) {
    let buf = *b"tick";
    loop {
        let at = timer(ms).await.unwrap_or(0);
        unsafe {
            if core::ptr::read_volatile(stop) != 0 {
                return;
            }
            let n = core::ptr::read_volatile(ticks);
            (*times)[n as usize % 16] = at;
            core::ptr::write_volatile(ticks, n + 1);
            let cs = op2(SLOT_OWN, buf.as_ptr() as u32, buf.len() as u32, 0).0;
            core::ptr::write_volatile(echo, cs);
        }
    }
}

// ---- T1: ticks every 30 ms (written out by hand, as T2 is) ----
#[no_mangle]
pub static T1_IMPORTS: Imports<8> = prefix_and!(crate::names::ECHO);
static mut T1_SLOT: TaskSlot<96> = TaskSlot::new();
static mut T1_TICKS: u32 = 0;
static mut T1_TIMES: [u32; 16] = [0; 16];
static mut T1_ECHO: u32 = 0;
static mut T1_STOP: u32 = 0;

#[no_mangle]
pub extern "C" fn t1_init() -> u32 {
    let f = ticker(
        30,
        addr_of_mut!(T1_TICKS),
        addr_of_mut!(T1_TIMES),
        addr_of_mut!(T1_ECHO),
        addr_of!(T1_STOP),
    );
    unsafe { (*addr_of_mut!(T1_SLOT)).spawn(0, f) as u32 }
}

#[no_mangle]
pub extern "C" fn t1_poll(_task: u32) -> u32 {
    unsafe { (*addr_of_mut!(T1_SLOT)).poll() }
}

/// 0 ticks, 1 last echo checksum, 2.. tick times (driver ms), 99 future size.
#[no_mangle]
pub extern "C" fn t1_stat(i: u32) -> u32 {
    match i {
        0 => get!(T1_TICKS),
        1 => get!(T1_ECHO),
        99 => unsafe { (*addr_of!(T1_SLOT)).size },
        k => unsafe { (*addr_of!(T1_TIMES))[(k as usize - 2) % 16] },
    }
}

/// Stop after the next tick.
#[no_mangle]
pub extern "C" fn t1_halt() -> u32 {
    set!(T1_STOP, 1);
    0
}

// ---- T2: ticks every 70 ms (written out by hand, as T1 is) ----
#[no_mangle]
pub static T2_IMPORTS: Imports<8> = prefix_and!(crate::names::ECHO);
static mut T2_SLOT: TaskSlot<96> = TaskSlot::new();
static mut T2_TICKS: u32 = 0;
static mut T2_TIMES: [u32; 16] = [0; 16];
static mut T2_ECHO: u32 = 0;
static mut T2_STOP: u32 = 0;

#[no_mangle]
pub extern "C" fn t2_init() -> u32 {
    let f = ticker(
        70,
        addr_of_mut!(T2_TICKS),
        addr_of_mut!(T2_TIMES),
        addr_of_mut!(T2_ECHO),
        addr_of!(T2_STOP),
    );
    unsafe { (*addr_of_mut!(T2_SLOT)).spawn(0, f) as u32 }
}

#[no_mangle]
pub extern "C" fn t2_poll(_task: u32) -> u32 {
    unsafe { (*addr_of_mut!(T2_SLOT)).poll() }
}

/// 0 ticks, 1 last echo checksum, 2.. tick times (driver ms), 99 future size.
#[no_mangle]
pub extern "C" fn t2_stat(i: u32) -> u32 {
    match i {
        0 => get!(T2_TICKS),
        1 => get!(T2_ECHO),
        99 => unsafe { (*addr_of!(T2_SLOT)).size },
        k => unsafe { (*addr_of!(T2_TIMES))[(k as usize - 2) % 16] },
    }
}

/// Stop after the next tick.
#[no_mangle]
pub extern "C" fn t2_halt() -> u32 {
    set!(T2_STOP, 1);
    0
}

// ---- R: a runaway (an infinite loop inside poll) ----
#[no_mangle]
pub static R_IMPORTS: Imports<7> = prefix_and!();
static mut R_SLOT: TaskSlot<64> = TaskSlot::new();
static mut R_SPIN: u32 = 0;
static mut R_HELD: u32 = 0;

async fn runaway() {
    // A ticket it holds while it runs away: the kernel must free it.
    let held = timer(100_000);
    set!(R_HELD, held.id());
    let _ = timer(10).await;
    loop {
        set!(R_SPIN, get!(R_SPIN).wrapping_add(1));
    }
}

#[no_mangle]
pub extern "C" fn r_init() -> u32 {
    unsafe { (*addr_of_mut!(R_SLOT)).spawn(0, runaway()) as u32 }
}

#[no_mangle]
pub extern "C" fn r_poll(_task: u32) -> u32 {
    unsafe { (*addr_of_mut!(R_SLOT)).poll() }
}

/// 0 spins so far, 1 the held ticket's id.
#[no_mangle]
pub extern "C" fn r_stat(i: u32) -> u32 {
    if i == 0 {
        get!(R_SPIN)
    } else {
        get!(R_HELD)
    }
}

// ---- S: select between a button (GPIO 3) and the radio ----
#[no_mangle]
pub static S_IMPORTS: Imports<8> = prefix_and!(crate::names::UART_WRITE);
static mut S_SLOT: TaskSlot<192> = TaskSlot::new();
/// Winner per round: b'G' gpio, b'R' radio, b'T' timer.
static mut S_LOG: [u8; 8] = [0; 8];
/// Rounds started (a select is armed while `S_ROUND > S_DONE`).
static mut S_ROUND: u32 = 0;
static mut S_DONE: u32 = 0;
pub const S_ROUNDS: u32 = 4;

async fn selector() {
    let mut buf = [0u8; 32];
    for round in 0..S_ROUNDS {
        set!(S_ROUND, round + 1);
        let w = if round < 3 {
            match select2(gpio_wait(3, RISING), recv(&mut buf)).await {
                Either::First(_) => b'G',
                Either::Second(_) => b'R',
            }
        } else {
            // A timeout race, so a cancelled ticket's late completion can be seen.
            match select2(recv(&mut buf), timer(10)).await {
                Either::First(_) => b'R',
                Either::Second(_) => b'T',
            }
        };
        unsafe { (*addr_of_mut!(S_LOG))[round as usize] = w };
        print(match w {
            b'G' => b"select: gpio\n",
            b'R' => b"select: radio\n",
            _ => b"select: timer\n",
        });
        set!(S_DONE, round + 1);
        let _ = timer(40).await;
    }
}

#[no_mangle]
pub extern "C" fn s_init() -> u32 {
    unsafe { (*addr_of_mut!(S_SLOT)).spawn(0, selector()) as u32 }
}

#[no_mangle]
pub extern "C" fn s_poll(_task: u32) -> u32 {
    unsafe { (*addr_of_mut!(S_SLOT)).poll() }
}

/// 0 rounds started, 1 rounds done, 2 the log (4 bytes), 99 future size.
#[no_mangle]
pub extern "C" fn s_stat(i: u32) -> u32 {
    match i {
        0 => get!(S_ROUND),
        1 => get!(S_DONE),
        2 => {
            let l = get!(S_LOG);
            u32::from_le_bytes([l[0], l[1], l[2], l[3]])
        }
        _ => unsafe { (*addr_of!(S_SLOT)).size },
    }
}

// ---- X: ticket ops called directly (stale-ticket test, costs), a waiter ----
#[no_mangle]
pub static X_IMPORTS: Imports<7> = prefix_and!();
static mut X_SLOTS: [TaskSlot<96>; 2] = [TaskSlot::new(), TaskSlot::new()];
static mut X_WAKES: u32 = 0;
static mut X_IDS: [u32; 16] = [0; 16];

/// Task 0: wake on every edge of GPIO 7 (the wake-and-poll round trip).
async fn waiter() {
    loop {
        if gpio_wait(7, ANY_EDGE).await.is_ok() {
            set!(X_WAKES, get!(X_WAKES) + 1);
        }
    }
}

/// Task 1: a select that stays pending (one select poll = two checks).
async fn idle_select() {
    loop {
        let _ = select2(gpio_wait(8, RISING), timer(1_000_000)).await;
    }
}

#[no_mangle]
pub extern "C" fn x_init() -> u32 {
    let t = unsafe { &mut *addr_of_mut!(X_SLOTS) };
    t[0].spawn(0, waiter()) as u32 + t[1].spawn(1, idle_select()) as u32
}

#[no_mangle]
pub extern "C" fn x_poll(task: u32) -> u32 {
    unsafe { (*addr_of_mut!(X_SLOTS))[task as usize & 1].poll() }
}

#[no_mangle]
pub extern "C" fn x_start(ms: u32) -> u32 {
    op2(mvm_rt::slot::TIMER, ms, 0, 0).0
}

/// `kernel.ticket.check.v1`: status in the low byte, result above it.
#[no_mangle]
pub extern "C" fn x_check(t: u32) -> u32 {
    let (s, r) = op2(mvm_rt::slot::CHECK, t, 0, 0);
    s | (r << 8)
}

#[no_mangle]
pub extern "C" fn x_cancel(t: u32) -> u32 {
    op2(mvm_rt::slot::CANCEL, t, 0, 0).0
}

#[no_mangle]
pub extern "C" fn x_wakes() -> u32 {
    get!(X_WAKES)
}

/// Cost loops: n checks of one (pending) ticket.
#[no_mangle]
pub extern "C" fn x_check_loop(t: u32, n: u32) -> u32 {
    let mut s = 0;
    for _ in 0..n {
        s += op2(mvm_rt::slot::CHECK, t, 0, 0).0;
    }
    s
}

/// The same loop with no op (baseline).
#[no_mangle]
pub extern "C" fn x_empty_loop(n: u32) -> u32 {
    let mut s = 0u32;
    for i in 0..n {
        s = core::hint::black_box(s.wrapping_add(i));
    }
    s
}

/// Start 16 one-second timers (ids kept for `x_cancel16`).
#[no_mangle]
pub extern "C" fn x_start16() -> u32 {
    let ids = unsafe { &mut *addr_of_mut!(X_IDS) };
    let mut ok = 0;
    for id in ids.iter_mut() {
        *id = op2(mvm_rt::slot::TIMER, 1000, 0, 0).0;
        ok += (*id != 0) as u32;
    }
    ok
}

#[no_mangle]
pub extern "C" fn x_cancel16() -> u32 {
    let ids = unsafe { &*addr_of!(X_IDS) };
    let mut s = 0;
    for &id in ids.iter() {
        s += op2(mvm_rt::slot::CANCEL, id, 0, 0).0;
    }
    s
}

// ---- L: a long (but budgeted) guest call ----
#[no_mangle]
pub static L_IMPORTS: Imports<0> = Imports::new([]);
static mut L_SINK: u32 = 0;

/// `n` iterations of a few instructions each (one branch per iteration).
#[no_mangle]
pub extern "C" fn l_spin(n: u32) -> u32 {
    for i in 0..n {
        set!(L_SINK, get!(L_SINK) ^ i);
    }
    get!(L_SINK)
}
