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
| ESP32-S3 (QEMU) | `main` in thread mode | **No preemptive driver context** (see below): the interpreter's wait loop runs one driver tick per 240,000 CCOUNT (1 ms at 240 MHz) and drains the UART for the script. |
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

See [meshops/RESULTS.md](meshops/RESULTS.md) for the per-target tables
(pass/fail, op costs, RAM). Summary and interpreter numbers: below, filled in
from the final runs.
