# Speed spike: plan and pre-hardware estimate

**Question:** how much slower is a minimal RV32IMC interpreter on an nRF52840 than
the same Rust code compiled natively for the Cortex-M4? And what does streaming
guest code through a small RAM cache add on top of that?

**Status:** The interpreter, the workloads and a cycle *estimate* exist (round three,
below). The board run (phase 1 on hardware) and phase 2 are not built yet.

## Estimate without hardware (round three)

`bench/run.sh` does the following:
1. Builds the two kernels for RV32IMC, chunks them, and links them into a 912-byte
   flat image with `chunker link`.
2. Builds two `no_std` interpreters (`bench/interp`) and the native kernels into one
   `thumbv7em-none-eabihf` firmware (`opt-level=3`).
3. Runs every call under Unicorn (Cortex-M, Thumb) with a hook on every instruction.
   Each instruction is costed with the Cortex-M4 table in
   [bench/COSTS.md](bench/COSTS.md), which comes from TRM Table 3-1:
   - 1 cycle by default;
   - 2 per load or store;
   - 3 for a taken branch, 1 untaken;
   - 2 for MLA;
   - 1+N for LDM/STM, and 4 for TBB/TBH.
4. Checks every result against native. The host tests also check both interpreters
   against native on 5 inputs, including the error paths.

Full output: [bench/RESULTS.md](bench/RESULTS.md).

### Results

| Workload | Guest instrs | Native cycles | Simple loop | Slowdown | Pre-decoded (warm) | Slowdown |
|---|---|---|---|---|---|---|
| crc16 (256 B) | 2825 | 2092 | 309,795 | **148x** | 162,110 | **77x** |
| parse (64 B) | 702 | 778 | 70,099 | **90x** | 39,747 | **51x** |

| Interpreter | Host instrs / guest instr | Cycles / guest instr |
|---|---|---|
| simple | 71–78 | 100–110 |
| pre-decoded (warm) | 33–34 | 57 |
| pre-decoded (cold, first run) | 34–44 | 58–74 |

- The TRM's best-case load/store pipelining moves these by under 10%: crc16 81x and
  parse 51x pre-decoded.
- **In wall-clock terms at 64 MHz:** pre-decoded crc16 takes about 2.5 ms and parse
  about 0.6 ms. The simple loop takes about 4.8 ms and 1.1 ms.

### Where the cycles go (cycles per guest instruction, crc16)

| Phase | Simple | Pre-decoded (warm) |
|---|---|---|
| fetch (instruction or table entry, with bounds checks) | 28.2 | 21.0 |
| decode (RVC expansion, fields, immediates, operand reads) | 49.1 | 17.0 |
| dispatch (`TBH` jump table) | 9.8 | 4.0 |
| execute (the operation, guest loads and stores) | 16.2 | 7.9 |
| loop (exit check, pc update, back-edge) | 6.3 | 6.3 |

How phases are assigned:
- Every host instruction maps, through DWARF line tables, to its innermost inlined
  frame in `bench/interp/src`.
- The closest `// @phase:` comment above that line names the phase.
- Compiler-generated code with no line (spills, merges) takes the phase of the
  instruction before it.
- This is a heuristic. Phase boundaries are accurate to a few percent.

### The cheap optimisation: a pre-decode table

- Each instruction is decoded once, on first execution, into a 12-byte entry in a
  table with one entry per 2 bytes of image. Later passes dispatch from the table.
- Decoding once also allows two more savings:
  - branch and jump targets are stored as absolute addresses;
  - writes to x0 are removed at decode time, so the loop never re-zeroes x0.
- **Result: 148x → 77x on crc16 and 90x → 51x on parse.** Decode cost falls from
  49 to 17 cycles per guest instruction.
- **Cost: RAM.** The table is 456 entries × 12 B = 5.5 KB for a 912-byte image,
  about 6x the image. Filling it lazily makes a cold run only slightly slower than a
  warm one when the code loops (crc16 79x cold), but parse runs most instructions
  once (67x cold).
- **Codegen fix, not counted as the optimisation:** the simple loop first called
  `expand()` out of line, returning `Option<u32>` through the stack. It was 192x on
  crc16 and 111x on parse. `#[inline(always)]` removed that, giving the 148x and 90x above.

### What the estimate says about the plan

1. **The phase 1 target (≤ 30x) is out of reach for a match-loop interpreter.**
   - Even pre-decoded it costs about 33 host instructions and 57 cycles per guest
     instruction. The plan assumed 15–30 cycles.
   - What remains:
     - the table lookup and its bounds check, about 21 cycles;
     - operand reads from the in-memory register file, about 17;
     - dispatch, about 4;
     - the operation itself, about 8;
     - the loop, about 6.
2. **RV32IMC needs more instructions than Thumb-2 for the same source.** The plan
   assumed the counts would be close.
   - crc16: 2825 RV instructions against 1432 Thumb instructions, about 2x.
   - parse: 702 against 558, 1.26x.
   - Thumb-2's shifted operands, post-increment loads and `IT` blocks do in one
     instruction what RV32 does in two. This multiplies straight into the slowdown,
     and no interpreter design removes it.
3. **The absolute times still fit the event-handler budget from the plan** (under
   10 ms per packet): crc16 plus parse take about 3 ms pre-decoded.
   - So the decision is either to judge by absolute budgets rather than by the ratio,
     or to aim for a design that can actually reach 30x.
   - The candidates for that are:
     - threaded dispatch through tail calls, which removes the loop and dispatch
       overhead (about 10 cycles);
     - caching hot guest registers in host registers;
     - translating RV32 to Thumb at load time, the only route to single-digit ratios.
   - The estimate can't choose between these. It's your decision.
4. **LLVM rewrites the "bitwise" CRC into a table-driven CRC on both targets.** Its
   CRC-loop recognition produces a 512-byte table, so crc16 really measures table
   CRC: 4 bytes per loop on Thumb, 1 per loop on RV32.
5. **`parse` has no reachable panic path.** Every bounds check either folds away or
   becomes an `Err` return, so the linked image is only `crc16`, `parse` and the CRC
   table. That's 3 chunks; `rust_begin_unwind` isn't reachable from either entry point.

### What only real hardware can measure

- **Flash wait states and the NVMC cache.** The estimate assumes every fetch and
  every flash data read is single-cycle. On the nRF52840 the firmware runs from flash
  behind the instruction cache. The guest image, the `TBH` jump tables and the CRC
  table are *data* reads from flash, and whether those hit or stall is unknown.
  Phase 1(b) against 1(a) measures it.
- **Branch refill (P = 1–3).** The model fixes P = 2. Real refill depends on
  target alignment and instruction width.
- **`IT` folding, the store buffer, and back-to-back load pipelining.** These are
  bounded by the two models (table and pipelined) but not pinned down.
- **SDIV/UDIV timing.** It is data dependent. These workloads don't divide.
- **Interrupts and bus contention** (radio, DMA, EasyDMA peripherals).
- **Power and energy** per guest instruction and per packet.
- **Unicorn's own correctness** as an execution model. Its results match native
  here, but it isn't cycle accurate. The cost table supplies the timing.

## Hardware and tools

- nRF52840 (Cortex-M4F, 64 MHz, 1 MB flash, 256 KB RAM), e.g. an nRF52840-DK.
- `probe-rs` to flash and run. Results come back over RTT (`defmt` or `rtt-target`).
- Someone with the board runs it. I give the exact command and read the output.

## Workloads

One `no_std` crate, `guest/kernels`, built twice: once for the interpreter and once natively.
Both functions are `extern "C" fn(ptr: *const u8, len: usize) -> u32`.

| Kernel | What | Correctness check |
|---|---|---|
| `crc16` | CRC-16/CCITT-FALSE (poly 0x1021, init 0xFFFF), written bitwise, over a fixed 256-byte buffer. LLVM turns it into a table CRC on both targets. | `"123456789"` gives `0x29B1`; the 256-byte result equals native |
| `parse` | Small packet parse. Header `ver:u8 type:u8 len:u16le seq:u32le`, then TLV records `t:u8 l:u8 v[l]` up to `len`, then a checksum byte. Returns a digest of the fields. | One 64-byte packet with 6 TLVs, plus one truncated packet that takes the error path. Both equal native. |

`parse` uses slice indexing, but every check folds away or becomes an `Err` return,
so no panic path is linked (see the round-three estimate).

## Builds

- **Guest:** `PKG=kernels BUILD_STD=1 guest/build.sh`, then chunked, then
  `chunker link` writes a flat image plus a header (base, size, entry offsets).
  The firmware embeds it with `include_bytes!`. The guest base stays at `0x1000`.
- **Native:** the same `kernels` crate as a firmware dependency for
  `thumbv7em-none-eabihf`: `opt-level=3`, `codegen-units=1`, `lto=off`
  (the same settings as the guest, so neither side gets cross-crate inlining).
- **Firmware** (`bench-nrf`): `cortex-m-rt`, no executor. It holds the interpreter,
  the timing harness and both builds of the kernels. The firmware itself builds with
  `opt-level=3`, `lto=fat` and `panic=abort`. That is fine, because LTO applies to
  the interpreter, not to the guest.

### What has to exist first

1. ✅ `chunker link` writes `OUT.bin` and `OUT.rs` (base, length, entry addresses). *(round three)*
2. ✅ `guest/kernels`. *(round three)*
3. ✅ `bench/interp`: the `no_std` simple and pre-decoded interpreters, with host tests
   against native. *(round three)*
4. `bench-nrf`, the real board firmware.
   - `bench/fw` is only a Unicorn harness: it has no vector table, no clock setup
     and no DWT timing.
   - Its linker script and `bench_*` entry points carry over.

## Phase 1: plain interpreter, image in memory

### Interpreter design: start with the simple match loop

- Fetch, decode and execute, decoding every instruction on every pass:
  `loop { let ins = fetch(); match opcode { ... } }`. No pre-decoding, no threaded
  code, no translation. Phase 1 measures how far the simplest design gets.
- State: `regs: [u32; 32]` and `pc: u32` in one struct. `x0` is reset to zero after each write.
- Fetch a `u16`. If its low two bits aren't `11`, expand the compressed form to 32 bits
  and run it through the same `exec`. This reuses `expand()` from the chunker.
  Handling compressed forms directly is a later optimisation, not a phase 1 one.
- Guest memory map, with one range check per region on each load or store:
  - the image at `[0x1000, 0x1000+len)`, read-only;
  - a 4 KB stack;
  - a "host window" holding the input buffers passed as arguments.
  - A store into the image, or any access outside these regions, is a fault.
- Calls enter with `ra = 0` and stop when `pc == 0`.
- Instruction counting lives behind a cargo feature, so timed runs don't pay for it.
- Before trusting any number, check the disassembly: the `match` should compile to a
  `TBB`/`TBH` jump table, and `regs` should be reached through one base register.

The image is placed two ways:
- **(a) in RAM**, copied at boot. This measures the interpreter on its own.
- **(b) in flash**, read in place. Guest fetches are data reads from flash, so this
  adds flash latency. Measure it rather than assume what the NVMC cache does for data reads.

### How to time it: the DWT cycle counter

- Enable it once: `DCB.enable_trace()`, `DWT.enable_cycle_counter()`. Read it with
  `DWT::cycle_count()` (`cortex-m` crate).
- Start the HFXO (`CLOCK.TASKS_HFCLKSTART`) for a stable 64 MHz clock, and turn the
  NVMC instruction cache on (`NVMC.ICACHECNF.CACHEEN = 1`). Log both settings with the results.
- Time with interrupts off (`cortex_m::interrupt::free`).
- For each case and build: 1 warm-up run, then 100 timed runs. Report the min and the
  median. Measure an empty call the same way and subtract it.
- A separate run with the counting feature gives the guest instruction count, so
  cycles per guest instruction (CPGI) can be computed.
- Every timed run compares the interpreted result with the native one. A mismatch fails the run.
- Output over RTT, one row per kernel and placement: native cycles, interpreted cycles,
  slowdown, guest instructions, CPGI.

### What "fast enough" means

**Target: at most 30x slower than native on both kernels with the image in RAM.**

> Round three estimates 77x and 51x pre-decoded, and 148x and 90x for the simple loop.
> See "What the estimate says about the plan" above. Keep this target or change it
> before the board run.

| Slowdown | Verdict |
|---|---|
| ≤ 30x | Pass. The simple design is good enough to build on. |
| 30–60x | Usable, but find the cause before adding features: look at dispatch codegen, register-file traffic, and the cost of compressed expansion. |
| > 60x | The simple loop is the wrong design for this chip. Try pre-decoding at load time next. |

Why 30x:
- **Cost per guest instruction.** On a Cortex-M4 a decode-every-time loop spends about
  15–30 cycles per guest instruction:
  - a 1–2 load fetch;
  - expansion for compressed instructions, which are about half of RV32IMC code;
  - field extraction, 3–6 ALU ops;
  - `TBB` dispatch plus a pipeline refill, about 5 cycles;
  - 2–3 register-file memory ops;
  - the `pc` update.
- **Native code.** It runs at about 1–1.5 cycles per instruction from cached flash,
  and Thumb-2 and RV32IMC instruction counts for code like this are close.
- **So 15–30x is the expected range.** Much worse than that means something is wrong
  in the implementation, not in the approach.
- **What 30x means in practice.** At 64 MHz, 30x leaves a guest running like a
  roughly 2 MHz Cortex-M4, which is enough for event handlers:
  - The bitwise CRC over 256 bytes is about 10k native instructions. That is about
    0.2 ms native, so roughly 5–7 ms interpreted.
  - A 64-byte packet parse is 1–2k instructions, well under 1 ms interpreted.
  - Together that fits a budget of under 10 ms per packet. It also shows that a
    bitwise CRC is about the heaviest per-packet work an applet should do at this speed.

## Phase 2: a 1–2 KB read-through cache in front of flash

**Purpose.** Chunked code in a real system won't always sit at one fixed, contiguous,
memory-mapped address. Chunks are stored by hash and fetched on demand. Phase 2 puts
every image access through a small RAM cache to measure that cost.

**Design**
- The image can only be read through `cache.line(addr)`. Pick one configuration
  to start: a 1 KB cache, 32-byte lines, direct-mapped.
- Backing store, two options:
  - **Internal flash, copied a line at a time.** This isolates the cache's own cost.
  - **The DK's external QSPI flash, read by command, not XIP.** This is the realistic
    streaming case.
- Fast path: keep the current fetch line's base and tag. Look up the cache only when
  `pc` leaves that line. Sequential code stays in it, so most fetches skip the lookup.
- Data loads from the image (rodata such as the CRC table or strings) look up the
  cache on every load. Stack and host-window accesses bypass it.
- Count hits, misses and bytes copied, behind the same counting feature.

**Measurements.** Use the same harness and kernels, plus a third workload that
doesn't fit the cache: the round-two applet (`core::fmt`, about 4 KB reachable).
Run every combination of:
- cache size: 1 KB, 2 KB;
- line size: 32 B, 64 B;
- mapping: direct-mapped, 2-way.

For each, report:
- cycles, both cold (first run after a flush) and warm;
- slowdown against phase 1(a) and phase 1(b);
- hit rate, and misses per 1,000 guest instructions.

**Target.**
- Warm runs of `crc16` and `parse` within 1.2x of phase 1(a). Their code is a few
  hundred bytes, so after warm-up the hit rate should be above 99%. What remains is the
  fixed cost per access.
- If the line-crossing check costs more than about 2 cycles per guest instruction,
  revisit the fast path.
- The applet shows how the cache behaves when it thrashes. Report it; it has no pass mark.

## Open questions this spike should answer

- How large is the gap between image-in-flash and image-in-RAM? That decides whether
  the cache is mainly about non-contiguous chunks, or also about flash latency.
- What is the CPGI of the simple loop on the M4? That figure decides whether
  pre-decoding is worth its RAM.
- What are the hit rate and cost per access of a 1–2 KB cache for real handler-sized code?
