# Speed spike plan (not built yet)

**Question:** how much slower is a minimal RV32IMC interpreter on an nRF52840 than
the same Rust code compiled natively for the Cortex-M4? And what does streaming
guest code through a small RAM cache add on top of that?

## Hardware and tools

- nRF52840 (Cortex-M4F, 64 MHz, 1 MB flash, 256 KB RAM), e.g. an nRF52840-DK.
- `probe-rs` to flash and run. Results come back over RTT (`defmt` or `rtt-target`).
- Someone with the board runs it. I give the exact command and read the output.

## Workloads

One `no_std` crate, `guest/kernels`, built twice: once for the interpreter and once natively.
Both functions are `extern "C" fn(ptr: *const u8, len: usize) -> u32`.

| Kernel | What | Correctness check |
|---|---|---|
| `crc16` | CRC-16/CCITT-FALSE (poly 0x1021, init 0xFFFF), bitwise, over a fixed 256-byte buffer | `"123456789"` gives `0x29B1`; the 256-byte result equals native |
| `parse` | Small packet parse. Header `ver:u8 type:u8 len:u16le seq:u32le`, then TLV records `t:u8 l:u8 v[l]` up to `len`, then a checksum byte. Returns a digest of the fields. | One 64-byte packet with 6 TLVs, plus one truncated packet that takes the error path. Both equal native. |

`parse` uses slice indexing, so the bounds-check panic path is linked even though
the test vectors never take it.

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

1. `chunker link`: write the image and header to disk. Today linking only happens in memory.
2. `guest/kernels`.
3. A `no_std` interpreter crate, `interp`, ported from `chunker/src/emu.rs`. The host
   test runs the kernel images in it and compares against the native host build,
   before any board is involved.
4. `bench-nrf`.

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
