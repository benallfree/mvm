# Round 7b: memory model

Branch `spike-memmodel`, started from round seven (`spike-meshops`). Results:
[meshops/RESULTS.md](meshops/RESULTS.md) (mesh ops, four targets) and
[bench/RESULTS.md](bench/RESULTS.md) (interpreter speed).

## The rules, and where they stand

| Rule | Status |
|---|---|
| 1. No heap per applet; static `.data`/`.bss` only | Done. An applet's RAM is one static block, sized by its manifest. |
| 2. One shared, kernel-owned stack | Done. One 8 KB stack; nested calls continue on the caller's stack. |
| 3. Real pointers | Done. A pointer to a static is the chip's real RAM address. On the x86 host, guest addresses are stand-ins. |
| 4. Code stays in flash, never patched, same bytes run anywhere | Done for code. Some unexecuted table words in flash still hold image offsets (see [Findings](#findings)). |
| 5. Globals through `gp`, swapped only at mesh-op boundaries | Done. Library jumps (`Stop::Jump`) keep `gp`. |
| 6. Applets run to completion | Holds, with costs listed under [Rule 6](#rule-6-run-to-completion). |

All 13 tests pass on the x86 host, Unicorn, Renode nRF52840 and QEMU ESP32-S3:
the 8 from round seven, plus position independence, two instances, real pointers,
shared stack and the fit check.

## Globals through gp: in-place gp relaxation

**The approach:**
- Applets are compiled with `-C code-model=medium`, so every code → code and
  code → rodata reference is PC-relative (`auipc` + `addi`/load).
- `chunker link-pic` links each applet once, with no base address. Flash (code +
  rodata) and statics (`.data` + `.bss`) are laid out separately.
- Every reference from code to a static is rewritten in place:
  - the `auipc rd, %pcrel_hi(sym)` (or `lui`) becomes `addi rd, gp, 0`;
  - each paired `addi`/load/store gets `sym_offset - GP_BIAS` as its 12-bit
    immediate.
- The kernel sets `gp = static_block + GP_BIAS` (`GP_BIAS = 2048`, in `mvm-abi`) on
  every entry into an applet.

**Why this approach:**
- **Statics never go through the GOT.** I checked what rustc emits for RV32:
  - with relocation-model `static` or `pie`, every static reference is
    `PCREL_HI20`/`PCREL_LO12`, with no GOT;
  - only `pic` uses the GOT, and only for exported (`#[no_mangle]`) statics; private
    ones stay PC-relative.
  - So a "rewrite the GOT to be gp-relative" design would miss most statics.
- **The pairs can be rewritten in place, so nothing moves.** The chunker already has
  every relocation as a slot, so it knows each `hi20`/`lo12` pair.
- **The code bytes depend only on the applet's own static layout,** not on where the
  image sits in flash or the block in RAM. Test 9 loads C from two flash addresses
  with two RAM blocks: identical bytes (same FNV), correct results from both.

**Cost:**
- **Per access:** 0 extra instructions. `auipc` becomes `mv rd, gp`, which costs
  the same in the interpreter. C has 5 rewritten accesses.
- **Per applet:** one `gp` value in its `Applet` record, with no table.
- **Per mesh-op boundary:** none. Each nesting level has its own register file, so
  the callee's `gp` is set at entry and the caller's is still in its own file when
  control returns.
- **Limit:** 4 KB of statics per applet (the 12-bit immediate reaches gp ± 2 KB). The
  chunker refuses more with an error that points here. Every applet in this round
  uses ≤ 28 B. Above 4 KB, a large array could be accessed through a pointer kept in a
  static (one extra load per access).

**How this extends to third-party library statics** (out of scope, not built):
- A library shared between applets can't use the "offset from my gp" trick for its
  own statics, because each applet's block has a different layout.
- The plan: rewrite the library's `auipc rd` into `lw rd, slot(gp)`, reading the
  static's address from a small per-applet pointer table at the start of the static
  block. The kernel fills that table at load.
- **Cost:** +1 load per access (≈1 guest instruction) and 4 B per library static per
  applet.
- The paired `lo12` keeps the offset within the static (usually 0), so the code
  bytes are still identical everywhere.

## Pointers stored in statics

`.data` that holds an address (e.g. C's `C_MSG: &[u8]` pointing at rodata) is
listed in the manifest as `DataReloc { at, to_static, off }`. At load, the kernel
writes the real address (`flash_base + off` or `block + off`) into the static
block. Flash is never written.

## Removing `--base` and the contiguous code region

- **`chunker link --base` is gone.** Applets are linked with `link-pic` and run
  wherever they're placed. `link` (base 0x1000) remains only for the speed bench's
  kernels image.
- **Code regions:** the kernel keeps up to 12 code regions, one per distinct image.
  Instances of the same image share the region and its decode table (test 10).
- **Interpreter changes** (`bench/interp/src/fast.rs`):
  - a jump whose target is in the current region is proven at decode time, exactly
    as before;
  - a `jalr` that leaves the region returns `Stop::Jump(target)`, and the kernel
    finds the region (`region_at`, a linear search of ≤ 12) and resumes there;
  - a far `jal` is decoded as `jalr x0+target`, so it needs no new arm.
- **Loads outside RAM and the current region** (one applet reading a buffer in
  another applet's flash) fall back to a cold `load_other` that searches the other
  regions.

**Cost of replacing the contiguous check:**

| Path | Cost |
|---|---|
| Jumps inside a region (all code in this round) | 0, unchanged |
| Loads from RAM or the current region | 0, unchanged |
| A jump to another region (shared library code) | one `Stop::Jump` exit, a search of ≤ 12 regions, and `resume_at`. Not measured: nothing in this round's applets crosses regions; it's covered by an interpreter unit test |
| A load from another region | a cold call plus a search of ≤ 12 regions |
| A native op validating a pointer (`guest_bytes`) | a RAM check, then a search of the region list |

**A layout trap, worth knowing:**
- The first version put the other-regions fallback inside `Mem::bytes`, the helper
  every load shares. It was a cold, never-inlined call, but it still changed LLVM's
  block layout of the whole fast loop: tail duplication was lost and branches per
  guest instruction rose. That cost **10%** (crc16 40.3x).
- Moving the fallback to the miss path of `load` (where round seven already left the
  loop with a fault) restored the old layout.
- A new `FarJal` op arm cost about 10% for the same reason, hence the `jalr`
  encoding.

**Interpreter speed** (Unicorn, Cortex-M4 cost table, warm), confirming the fixes
didn't slow it:

| Workload | Round 7 fast | 7b fast | Round 7 cycles/guest instr | 7b |
|---|---|---|---|---|
| crc16 | 36.77x | **35.93x** | 27.2 | 26.6 |
| parse | 24.34x | **24.10x** | 27.0 | 26.7 |

The simple and pre-decoded interpreters (not used by the kernel) moved by about
+2% and +0.7%. They share `Mem` and see layout noise.

## Shared stack and register files

- **One stack:** a nested mesh op continues on the caller's guest stack, at its
  `sp`, rounded down to 16. Round seven gave each level its own 2 KB slice (16 KB
  for 8 levels); 7b uses one 8 KB stack for everything.
- **Measured per level (test 12):** 48 B of guest stack per `test.recurse.v1` level
  on all four targets. That's A's 48-byte frame; the kernel adds nothing to the
  guest stack. Depth 7 peaks at 288 B.
- **Register files:** 8 × 256 B in the kernel, one per nesting level, reused across
  calls. `start` sets only `ra`, `sp`, `gp` and the arguments and no longer clears
  the file. Clearing 256 B was most of an applet-to-applet call in round seven.
- **Depth 8 limit and the clean abort** are unchanged (test 5).
- **Host stack per level:** 232 B on Thumb (`run_guest` calls itself directly;
  `dispatch` is inlined), down from 464 B.

**Call cost against round seven** (Unicorn cycles; full table in
`meshops/RESULTS.md`):

| Call | Round 7 | 7b | Why |
|---|---|---|---|
| kernel.nop.v1 | 216 | 211 | |
| kernel.uart.write.v1 (16 B) | 670 | 326 | one pointer lookup and a direct write, instead of a check plus a chunked copy |
| applet op (echo, len 0) | 729 | 585 | no register-file clear |
| each nesting level | 902 | 823 kernel + 21 more guest instrs | A's frame now does real work; see RESULTS |
| host → guest entry | 564 | 458 | no register-file clear |
| one guest instruction | 29.0 | 29.6 | |

## Manifests and the fit check

`meshops/build_applets.py` turns each `link-pic` output into a `Manifest`:

```rust
Manifest {
    image: &[u8],              // flash: code + rodata, 16-byte aligned
    static_size: u32, static_align: u32,
    static_init: &[u8],        // .data bytes, then zeros
    data_relocs: &[DataReloc], // pointers to write into the block at load
    imports: &[Import { hash, name }],
    exports: &[Export { hash, name, entry }],
}
```

- **Imports and exports with full names** are read by the build tool from the
  applet's own tables, so the kernel never reads guest memory to learn an applet's
  interface.
- **Load order is fit, then names, then region, then static block, then exports,
  then imports.** Nothing is changed until the fit and every name check pass.
- **Fit check** (test 13): the loader computes the aligned need against free RAM
  (RAM minus statics in use minus the shared stack). It rejects with
  `NoRoom { need, free }` before touching anything; the test's need was 8109 B against
  8108 B free. It also rejects `NoCodeRoom` when the decode-table pool or region list
  is full.

## RAM per applet

- **Static block:** its declared size (0 B for most applets here, 28 B for C).
- **`Applet` record:** 48 B on Thumb (16 import slots, slot count, region, block,
  block length, gp).
- **Decode table:** 4 B per byte of image (one 8 B entry per 2-byte halfword), per
  code region. It is shared by instances and is the dominant cost: H 3.8 KB, B
  12.8 KB.
- **Fixed:** the shared stack (8 KB in the tests) and register files (2 KB).
- **Load time:** about 8,450 estimated M4 cycles per applet for H, A and B (round
  seven: 3,975). Part of the rise is clearing each region's decode table at load
  (2,600 entries, 21 KB for the three), which round seven did once for the whole blob
  outside the measurement. I didn't break it down further.

## Findings

- **Absolute words left in flash (rule 3/4).** Each import/export table entry holds
  its name pointer, and exports hold a function pointer; B also has a `core`
  panic `Location` (file-name pointer).
  - `link-pic` writes these as image offsets and lists them in `abs_flash`
    (1 to 8 per applet).
  - None is executed, and the kernel never reads them: the build tool turns the
    tables into manifests.
  - A panic message that reads its `Location`, though, would get an offset, not an
    address.
  - **Fix when needed:** a PIE-style `.data.rel.ro` — move such words into the
    static block with a `DataReloc`. That's +4 B RAM per pointer.
- **Writable statics must never be content-deduped.** Two zeroed `.bss` statics
  have identical bytes, and the chunk store merged them. Fixed: `Data`/`Bss` chunks
  are their own canonical copy.
- **Absolute (`lui`) references to flash** are rejected with "build with
  `-C code-model=medium`". The default `medlow` (small) model would bake the flash
  address into code.

## Rule 6: run to completion

Nothing in this round needed to break it, but it constrains the design:

- **The UART op blocks** until the bytes are written; a slow console stalls every
  applet.
- **No budget, preemption or watchdog.** A guest loop that never returns hangs the
  kernel. The interpreter could count instructions per `run` and return a
  `Stop::Budget`, at about one decrement-and-branch per guest branch.
- **The Embassy executor is blocked** while guest code runs: no other task, radio or
  timer servicing until the outermost call returns.
- **No asynchronous ops.** An op that waits (radio send, sensor read) must either
  block (as UART does) or return "pending" and have the applet poll, because there
  is no way to suspend an applet mid-call.
  - Suspension would need a stack per suspended applet, which conflicts with rule 2.
  - Or it would need applets written as state machines that return and are re-entered.
