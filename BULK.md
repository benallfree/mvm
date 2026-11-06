# Round 7c: automatic bulk statics

Branch `spike-bulk`, started from round 7b (`spike-memmodel`). Results:
[meshops/RESULTS.md](meshops/RESULTS.md). Round 7b's 4 KB-per-applet limit on
statics is gone, and developers still mark nothing: they write ordinary Rust
statics, and the chunker decides where each one lives.

All 19 tests pass on the x86 host, Unicorn, Renode nRF52840 and QEMU ESP32-S3:
round 7's 8, round 7b's 5, and 6 new ones (big buffer, overflow spill, bulk
position independence, bulk real pointer, bulk fit, no annotations).

## Design

**Two RAM segments per applet:**
- **gp window** (at most 4 KB): small statics, reached `gp`-relative exactly as
  in 7b (`gp` = window + 2048). The window starts with the bulk slots.
- **bulk segment** (any size): large statics, placed by the kernel at load.

**Placement rule** (`chunker link-pic`, `link::link_pic`):
1. **Threshold:** a static of `--bulk-threshold` bytes or more goes to bulk.
   The default is 256 B (`mvm_abi::BULK_THRESHOLD`); `build_applets.py` takes
   `BULK_THRESHOLD=N`.
2. **Spill:** if the remaining statics, plus the bulk slots, still don't fit the
   window (alignment padding included), the largest one moves to bulk. This
   repeats until they fit; ties go to the earliest in link order.
3. **Layout:** within each segment, `.data` comes before `.bss`, so the stored
   init bytes are a prefix and the zeros aren't stored.

The spill rule alone is enough for correctness: with the threshold off, D's 16 KB
buffer still lands in bulk (as a spill). The threshold only decides which
*small-enough* statics give up the cheaper gp access. See the sweep in
`RESULTS.md`.

**Access rewrite: in place, no size change.** Rust reaches a static with
`auipc rd, %pcrel_hi(sym)` plus a paired `addi`/load/store `%pcrel_lo`. For a
static at bulk offset `o`:

| | gp-window static (7b) | bulk static (7c) |
|---|---|---|
| `auipc rd` becomes | `addi rd, gp, 0` | `lw rd, (4k - 2048)(gp)`, with `k = o / 4096` |
| paired immediate | `o - 2048` | `o - 4096k - 2048` |

- **Bulk slots:** slot `k` (gp-window offset `4k`) holds `bulk + 4096k + 2048`,
  written by the kernel at load. A single "bulk base" slot can't work, because a
  12-bit immediate reaches only ±2 KB from it.
- **One slot per 4 KB page** makes every offset reachable, including constant
  offsets far into a buffer. There are `size / 4096 + 1` slots, so a
  one-past-the-end address also has a slot.
- **Exercised at runtime:** D's `TAIL` sits at bulk offset 16768 and is reached
  through slot 4 (`lw a2, -0x7f0(gp)`). Everything else in the test applets uses
  slot 0.

**Kernel at load** (`Kernel::load`):
1. Fit-check both segments together, before anything changes.
2. Place the gp window, then the bulk segment (each aligned), from free RAM.
3. Copy each init prefix and zero the rest.
4. Write the bulk slots.
5. Apply data relocs. These can point between any segments:
   - gp → bulk: D's `WORDS_PTR`;
   - bulk → gp: D's 64-entry `FILLS_PTRS`;
   - static → flash: C's `C_MSG`.

Pointers to bulk statics are real addresses (test 17: the kernel reads 16 KB
through one).

**Manifest:**

```rust
Manifest { image, gp: Segment { size, align, init }, bulk: Segment { size, align, init },
           bulk_slots, data_relocs: &[DataReloc { seg, at, to, off }], imports, exports }
```

The loader checks that `gp.size <= 4096`, `bulk_slots == bulk_slots(bulk.size)`,
and that the window holds the slots.

**Content addressing:**
- Code bytes depend only on the applet's own layout: which segment each static
  is in, and its offset there. They never depend on where flash, the window or
  bulk land.
- Test 16 loads D from two flash copies with two gp windows and two bulk
  segments: identical bytes (same FNV), separate state, correct results.
- Changing the threshold *does* change code bytes (`mv` becomes `lw`, other
  immediates), but never their size. That's part of the build, like any compiler
  flag.

## Cost

**Per access** (read-modify-write of a `u32` static, net of an empty-call loop):

| Target | gp-window static | bulk static | bulk extra |
|---|---|---|---|
| Unicorn (Cortex-M4 cost table) | 121.8 cycles | 142.8 cycles | **+21 (+0.71 guest instr)** |
| Renode nRF52840 | 46.8 | 55.1 | +8.3 (+0.73) |
| QEMU ESP32-S3 | 5.6 | 6.5 | +0.9 (+0.68) |

- Both forms are four guest instructions. The bulk form turns `mv rd, gp` into a
  load, which costs about 0.7 of an interpreted instruction everywhere.
- The cost is per address formation, not per access. A loop over a bulk buffer
  forms the address once and then indexes from a register, so it costs the same
  as before (D's fill and checksum loops).

**RAM per applet:**
- the gp window (statics + 4 B per bulk slot);
- the bulk segment;
- the `Applet` record, now 56 B on Thumb (+8 B for the bulk address and length).

| Applet | gp window | of which slots | bulk | RAM per instance |
|---|---|---|---|---|
| D (16 KB buffer + counters + pointer table) | 36 B | 20 B | 17,280 B | 17,372 B |
| E (41 small statics, 5,124 B) | 3,976 B | 4 B | 1,152 B | 5,184 B |

Slot overhead is 4 B per 4 KB of bulk (+4 B). Applets with no bulk pay nothing.

**Load time:** loading H, A and B went from 8,453 to 9,066 estimated M4 cycles per
applet (+7%). The loader now sets up two segments instead of one; I didn't break
it down further. D's load (17 KB of zeroing, 65 relocs) isn't measured separately.

**Interpreter and benchmarks: unchanged.**
- CRC16 is still 35.93x and parse 24.10x (the same `bench/RESULTS.md`
  byte for byte).
- The kernels have no writable statics, so the threshold sweep gives one code
  hash for every threshold.

## Access patterns the rewrite couldn't handle (as first built)

Three things came up while building the test applets. Each is now handled; none
needed source changes:

1. **LLVM's GlobalMerge merged small internal statics** into one blob, with
   several symbols inside one section, reached by offsets from one base. That
   would make placement all-or-nothing for the whole blob, and the chunker
   refused it ("symbol inside a chunk").
   - **Fix:** `guest/build.sh` turns GlobalMerge off for PIC builds
     (`-C llvm-args=-riscv-enable-global-merge=false`). Merging buys nothing
     here anyway: `gp` already gives every small static one shared base.
   - Round 7b never hit this because C's statics were `#[no_mangle]`, which
     GlobalMerge leaves alone.
2. **A `static` (or a never-written `static mut`) holding pointers to RAM statics
   lands in `.rodata`**, so flash would have to hold a RAM address known only at
   load. D's `FILLS_PTRS` did this.
   - **Fix:** the chunker promotes such read-only chunks to a RAM segment, and
     the pointers become load-time relocs. It repeats until nothing more needs
     promoting, since a pointer to a promoted chunk also points into RAM.
3. **Never-written `static mut`s disappear or become flash constants** (D's
   `SEED`, `SQUARES` and `WORDS_PTR` before they had writers). That's correct,
   since they're constants, but worth knowing: placement only sees what LLVM
   leaves as writable data.

**What still refuses** (an error at link time, never a silent miscompile):
- an `auipc`/`lui` slot on another instruction;
- a pointer to RAM inside *code*;
- a jump into data;
- an absolute (`lui`) reference to flash data (build with
  `-C code-model=medium`).

None of these appeared in the test applets. A code reference that reaches one
static at a constant offset from another would also break placement. With one
section per static (`function-sections`, GlobalMerge off) the compiler can't
emit that.

## Test 7: no annotations

`build_applets.py` checks `guest/applets/src/bulk.rs` on every build:
- no writable static (`static mut`) has an attribute;
- the file contains no `link_section`, `#[used]` or `export_name`.

The result goes into the image and is reported as test 19:
`plain_statics=47 annotated=0`. The only attributes in the file are on
functions (entry points) and on the two import tables. The import tables are the
applet's interface, read by the build tool, and live in flash.

## Not done (out of scope)

- Chunk sharing between applets.
- Third-party library statics. The 7b plan (a per-applet pointer table) fits the
  bulk slots naturally: a library's static would get its own slot.
- The event model and real hardware.
