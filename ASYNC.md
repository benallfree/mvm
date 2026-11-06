# Round 7d: the async event model

Branch `spike-async`, started from round 7c (`spike-bulk`). Results:
[meshops/RESULTS.md](meshops/RESULTS.md). Applets can now wait for timers,
radio sends, incoming packets and GPIO edges without blocking the
interpreter or holding the shared stack. The kernel is the executor: an
applet runs until it is waiting, returns, and is polled again when
something it waits on completes.

## The rule

**No kernel op ever awaits while guest code is paused under it.**

- Every native op is a plain `fn(&mut Kernel, args) -> [u32; 2]`, never an
  `async fn`, so it can't await.
- An op that starts slow work records a ticket, pushes a request to the
  drivers, and returns the ticket id at once.
- The only `.await` in the kernel's loop is between guest calls: the
  interpreter task sleeps on the inbox when nothing is ready.
- Guest code therefore never sits paused on the shared stack waiting for
  hardware, so one stack still serves every applet.

## Pieces

**Applet exports** (written by hand in the test applets, no macros):
- `init()`: called once by `Kernel::load`, under the budget. It stores its
  tasks' futures and calls `kernel.task.spawn.v1(task)` for each.
- `poll(task) -> 0 Pending | 1 Done`: polls that task's future once. The
  kernel calls it when one of the task's tickets completed: once per batch,
  however many of its tickets completed in it.

**Tasks** (guest, `mvm-rt::TaskSlot<N>`):
- Fixed storage in a static: `N` bytes plus a poll and a drop function
  pointer. `spawn` writes the future in place, and it never moves after;
  that is the pinning. No heap.
- A future that doesn't fit is a compile error (`const` assert).
- Up to 8 tasks per applet. The kernel keeps 2 bits per task (spawned,
  ready).

**Tickets** (kernel, 32 entries of 28 B on Thumb):
- An id is `generation << 8 | slot`. The slot's generation advances each
  time the slot is handed out, so a freed or reused id never matches.
- Each entry holds the owner applet and task, the state (Pending, Done or
  Free), the kind (timer, send, receive with its buffer, GPIO with its pin
  and edges) and the result.
- `ticket.check` by anyone but the owner, or on a stale id, returns
  `INVALID`.
- A ticket is freed when its result is taken (`check` returns `DONE`) or
  when it is cancelled.

**Guest futures** (`guest/mvm-rt`, hand-written):
- `Ticket`: each poll is one `ecall` to `kernel.ticket.check.v1`. Dropping an
  unfinished ticket cancels it.
- `Recv<'a>`: a receive ticket that keeps its buffer mutably borrowed until it
  finishes, because the kernel writes the packet into the buffer between
  polls.
- `Select2`: checks both tickets on each poll. The first ready wins, and the
  other is cancelled through `kernel.ticket.cancel.v1`.
- The waker is unused (`Waker::noop()`): the kernel knows which task owns
  each ticket.

**Drivers and the inbox** (`meshops/kernel/src/io.rs`, `drivers.rs`):
- **Two lock-free rings**, each with a single producer and a single consumer.
  - The inbox carries events from the drivers to the kernel: `Done{ticket,
    result}`, `Packet`, `Edge`.
  - The request ring carries the kernel's requests to the drivers: start or
    cancel a timer, send a packet, and the test hook's scheduled inputs.
- **No locks, and no shared mutable state** beyond the rings and a millisecond
  clock.
- **Driver context only:** the fake drivers (timer, radio, GPIO) run there,
  from a 1 ms tick. Each tick they first deliver what's due, then take new
  requests.
- **Back-pressure:**
  - Completions the drivers own (timers, sends) are never dropped. If the
    inbox is full they stay in the driver's table and go out on a later tick.
  - Inputs from outside (packets, edges) are dropped and counted when the
    inbox is full, as a real radio FIFO would drop them.
- **The fake radio:**
  - Sends complete after a fixed 5 ms.
  - Incoming packets go to the oldest pending receive.
  - A packet with no receive pending waits in a 4-packet kernel backlog;
    beyond that it's dropped and counted.

**Executor** (`Kernel::run_once`):
1. Drain the inbox as one batch: mark tickets done (stale ones are ignored
   and counted), hand packets to receives, and match edges to GPIO waits.
2. Collect the owner tasks as ready.
3. Poll each ready task once.

When nothing is ready, the interpreter task sleeps on the drivers' signal.

**Budget:**
- Every `init` and `poll`, including any mesh ops it makes into other
  applets' guest code, runs under a cap. The default is **20,000 control
  transfers** (`DEFAULT_BUDGET`, set per kernel).
- The unit is executed jumps and branches, not instructions. Every loop
  iteration and every recursion has one, so a runaway is always caught, and
  the count stays off the per-instruction dispatch path.
- Running out returns `Stop::Budget`. The kernel marks the applet faulted,
  stops its tasks and frees its tickets.
- Host calls (`Kernel::call`) are not budgeted.

## Embassy priorities

| Chip | Interpreter (lowest) | Drivers (higher) |
|---|---|---|
| nRF52840 (Renode) | `main` on the thread-mode executor; sleeps with WFE on `WAKE` | `InterruptExecutor` on EGU0/SWI0 at P6, running the 1 ms `Ticker` task (which also samples the P0.03 button) and the UART script reader. RTC1 (embassy-time) at P5. |
| ESP32-S3 (QEMU) | `main` in thread mode | **No preemptive driver context** (see below): the interpreter's wait loop runs one driver tick per 24,000 CCOUNT and drains the UART for the script. One driver millisecond is a tenth of a real one: under `-icount` a busy-waited real millisecond costs about a million emulated instructions, which made the suite take over ten minutes. Tests measure time in driver ms, so they're unaffected; guest code just looks 10× slower against this clock. |
| x86 host | the main thread | a second thread ticking every millisecond of wall time, truly concurrent with guest code |
| Unicorn | `mo_run` | the harness raises an "interrupt" every 64,000 estimated cycles (1 ms at 64 MHz): it saves the registers at an instruction boundary outside IT blocks, runs `mo_irq`, and restores them. `mo_idle` (the WFI) skips ahead to the next tick. |

**ESP32-S3: a deviation.** The plan was esp-rtos's Embassy executors: thread
mode for the interpreter, an `InterruptExecutor` on a software interrupt for
the drivers. Under Espressif's QEMU, every interrupt source I tried hung the
emulated CPU as soon as it was enabled:
- esp-rtos's scheduler never came back from its first context switch, whether
  its tick came from TIMG0 or SYSTIMER;
- a plain esp-hal TIMG0 periodic interrupt stalled thread mode, even with the
  timer stored before it started so the handler could clear it;
- the Xtensa core timer (CCOMPARE0, CPU interrupt 6), with a handler that only
  counts and re-arms, stalled it too, and the handler's count was never seen.

Without the interrupt enabled, the same code runs fine. So on the ESP32-S3 the
drivers tick cooperatively from the interpreter's wait loop. Every async test
runs except 28, which reports itself as skipped: it needs drivers that preempt
a long guest call. The nRF52840 under Renode and the x86 host (a real thread)
cover that case. On real ESP32-S3 hardware, the esp-rtos setup above is the
intended one; it is untested.

## Fake inputs

- **Host and Unicorn:** the tests schedule packets and edges through the
  request ring (the test hook). The drivers deliver them at the scheduled
  millisecond, from driver context, exactly as real inputs would arrive.
- **Renode and QEMU:** the same hook, plus test 30. There the firmware prints
  `MO script-ready` and an outside script acts:
  - **Renode:** a Python UART line hook schedules a real rising edge on P0.03
    (GPIOTE port event) 100 ms later, and types a packet line on the UART
    300 ms later.
  - **QEMU:** `emu/qemu_serial.py` types the button press (`E3`, because
    QEMU's ESP32-S3 GPIO can't be driven from outside) and then the packet
    (`Pscript`) on the UART socket.
- **Renode GPIO notes:** the scripted edge on P0.03 is real, but Renode's
  GPIOTE model latched the IN event without raising its interrupt (INTEN read
  back 0), and a read of the event register at tick time didn't see it either.
  So the nRF52840's driver samples the pin level (P0.IN) on every 1 ms tick and
  reports rising edges.

## A real SX126x model in Renode (later)

What it would take:
- **SPI peripheral:** a C# Renode peripheral implementing the SPI interface,
  decoding the SX126x command set (`SetStandby`, `SetPacketType`,
  `SetRfFrequency`, `SetModulationParams`, `WriteBuffer`/`ReadBuffer`,
  `SetTx`/`SetRx`, `GetIrqStatus`/`ClearIrqStatus`), with the 256-byte data
  buffer and the IRQ mask.
- **Control lines:** BUSY and DIO1 as GPIO outputs into the nRF52840's GPIOTE.
- **Air:** a connection to a radio medium so two emulated nodes hear each
  other, delivering frames after a computed time on air (SF, BW, CR, payload).
- **Driver:** the firmware side becomes a real driver (for example the
  `lora-phy` crate) in place of the fake radio. The kernel's ops and tickets
  don't change: only the driver task behind the rings does.

## Writing async guest code by hand: what was awkward

These shape the SDK:

1. **Sizing task slots.** A future's size is known only after compiling, so
   `TaskSlot<N>` is picked by trial; the compile-time assert at least makes a
   bad guess an error. The SDK should size slots itself (a build step, or a
   nameable future type).
2. **Statics from async code.** Every access is `addr_of!` plus a volatile
   read or write, because other polls, and the kernel through receive buffers,
   change them between awaits.
3. **Cancellation:**
   - **Same-batch losers.** When both of a `Select2`'s tickets complete in the
     same inbox batch, `a` wins only because it is checked first. `b` is
     cancelled with its result discarded, and for a receive that packet is lost
     (test 29, round 3).
     - The kernel should return a completed ticket's result from `cancel`, so
       the SDK can keep it.
     - Or it should report completion order, so "first" means first.
   - **Cancel doesn't abort.** Cancel means "I no longer care", not "undo".
     A cancelled send still goes out; a cancelled timer is withdrawn on a best
     effort basis, and if it already fired, its completion is ignored as stale
     (test 29, round 4).
   - **Drop has to run.** `Ticket`'s `Drop` cancels, so an abandoned future
     never leaves work running. That relies on the future actually being
     dropped: `mem::forget` on a `Recv` would let the kernel write a buffer
     that's been reused. A safer SDK would have the kernel keep received bytes
     itself and copy them out on `check`, at the cost of a copy.
4. **Immediate completion.** A receive served from the backlog completes at
   creation, and no wake is sent: the creating future must check it in the same
   poll, as `mvm-rt`'s futures do.
5. **Who gets woken.** Only a ticket's owner task wakes. Tickets made outside a
   poll (in `init`, in a host call) wake nobody, and neither do tickets made by
   another applet's code running nested under the poll. There is no way yet for
   one task to wait on another task's state (a channel): that needs a
   kernel-side wake or a guest-side waker.
6. **The budget is a cliff.** A poll that legitimately loops for long, such as
   a checksum over a big buffer, has to split its work across polls itself
   (yield with a 0 ms timer). The SDK could offer a `yield_now()`.
7. **Jump tables.** Async state machines compile to a `match`, which LLVM
   turned into a jump table: absolute code addresses in `.rodata`. Position-
   independent flash can't hold those (the chunker wrote image offsets, and a
   resumed future would have jumped to them).
   - PIC builds now pass `-C jump-tables=no`.
   - The chunker now refuses any code address in flash data outside export
     tables, so the next table like this is a link error, not a crash.
   - This could have hit any applet with a big `match` since round 7b.

## Results

Tests 1-19 (rounds 7-7c) still pass on all four targets. Round 7d's:

| Test | x86 host | Unicorn | Renode nRF52840 | QEMU ESP32-S3 |
|---|---|---|---|---|
| 20 hello loop (5 x timer 50 ms + send; order, timing, 11 polls) | ✅ | ✅ | ✅ | ✅ |
| 21 two tasks in one applet (each woken only by its own tickets) | ✅ | ✅ | ✅ | ✅ |
| 22 two applets, 30 and 70 ms timers (polls = own completions + 1) | ✅ | ✅ | ✅ | ✅ |
| 23 idle (no guest code between events) | ✅ | ✅ | ✅ | ✅ |
| 24 no deadlock (nested guest op in a poll, host call, tasks pending) | ✅ | ✅ | ✅ | ✅ |
| 25 budget (runaway faulted, tickets freed, other applet on schedule) | ✅ | ✅ | ✅ | ✅ |
| 26 stale ticket (cancelled, reused slot, taken, other applet's) | ✅ | ✅ | ✅ | ✅ |
| 27 inbox overflow (50 packets in 1 ms) | ✅ | ✅ | ✅ | ✅ |
| 28 driver priority (events queued during a long guest call) | ✅ | ✅ | ✅ | skipped (no preemption, above) |
| 29 select (both orders, same batch, late completion) | ✅ | ✅ | ✅ | ✅ |
| 30 script inputs (pin edge / typed button, then typed packet) | n/a | n/a | ✅ | ✅ |

**What the tests showed:**
- **Hello loop:** sends start at 51, 107, 163, 219 and 275 ms on every target:
  50 ms timer, 5 ms send, plus at most 1 ms of wake latency.
- **Idle (210 ms window):** 10 events, 10 polls, 20 guest calls (each poll plus
  the echo it makes), and nothing else.
- **Overflow:**
  - Of the 50 packets, 16 fit the inbox and 34 were dropped there
    (`rx_dropped`).
  - Of the 16, one went to the waiting receive and 4 to the backlog; 11 were
    dropped at the backlog (`backlog_dropped`).
  - The receive loop got 5, and later traffic still flowed. Nothing crashed,
    and every packet is accounted for.
  - Completions the drivers own are never dropped: they wait in the driver
    until the inbox has room.
- **Driver priority:** during a 16 ms guest call on the nRF52840, 16 driver
  ticks ran, 6 events were queued, none were lost, and all were delivered after
  the call. The x86 host and Unicorn show the same.
- **Select:**
  - Winners were G, R, G, R. The loser was cancelled each time.
  - A late button press with no wait armed was ignored.
  - In the same-batch round, `gpio` won because it is checked first, and the
    packet was discarded with the cancelled receive.
  - The radio-versus-timer round produced a stale timer completion, which the
    kernel ignored by generation.

**Costs** (Unicorn, estimated Cortex-M4 cycles; all four targets in RESULTS.md):

| Operation | Cycles | Guest instructions |
|---|---|---|
| `ticket.check` (pending) | 246 | 8 |
| `timer.start` | 2,617 | 87 |
| `ticket.cancel` (pending timer) | 2,348 | 78 |
| one select poll (kernel poll, two checks) | 2,342 | 78 |
| wake-and-poll round trip (event in -> task resumed and re-armed) | 4,402 | 147 |
| a runaway cut off by the default budget | 350,520 (5.5 ms at 64 MHz) | ~11,700 |

- `timer.start` and `cancel` are dear mostly because each request copies a
  40-byte ring entry, which has room for a 32-byte packet even for a timer.
  Splitting packet sends from small requests would cut that.
- `check` is cheap: it's on every poll, so that's the one that matters.

**RAM:**
- Ticket table: 28 B × 32 = 896 B.
- The shared `Io` (16-entry inbox of 36 B, 32 requests of 40 B, sent log,
  counters): 3,328 B.
- `Applet` record: 64 B, up from 56.
- Per task: 2 kernel bits, plus the guest's slot (its future + 16 B).
- Futures as laid out: hello 16 B, receive 40 B (it holds the 32-byte buffer),
  ticker 52 B, select 56 B.

**Budget default and its cost:**
- The default is **2,000 control transfers**, about 10,000 guest instructions,
  or 5.5 ms on a 64 MHz Cortex-M4.
- The first choice, 20,000, let a runaway hold the interpreter about 47 ms
  under Unicorn, and the other applet's ticks slipped by that much (test 25
  failed). With one interpreter, the budget is also the worst-case latency a
  bad applet can add to everyone.
- Counting costs the hot loop **+2.4% on CRC16 (35.93x → 36.79x) and +1.5% on
  parse (24.10x → 24.46x)**. That's a decrement and a test on each jump and
  branch, and nothing on other instructions.
