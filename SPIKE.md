# Spike: chunking rustc RISC-V output

**Goal:** prove that real rustc RISC-V output can be split into self-contained,
hashable chunks with nothing left unresolved.

**Result: yes.** `./check.sh` passes all 12 checks: 6 criteria, each run under
both code models.

## Layout

| Path | What |
|---|---|
| `guest/` | `testcrate` (no_std) + `helper` (second crate), pinned nightly |
| `guest/build.sh` | the builder: pins flags, emits `.o`, prints chunker inputs |
| `chunker/` | host tool: `dump`, `hashes`, `roundtrip` |
| `check.sh` | runs every success criterion for `CODE_MODEL=small` and `medium` |

Builder flags: `-C target-feature=-relax -C codegen-units=1 -C lto=off -Z function-sections=yes -C code-model=<small|medium>`.

## Chunk model

- One chunk per allocated section. Mergeable string pools (`SHF_MERGE|SHF_STRINGS`)
  are split after each zero byte, one chunk per string.
- A chunk is **named** if a real symbol sits at offset 0, otherwise **anonymous**.
  A named symbol anywhere else inside a chunk is an error.
- Relocated fields are blanked; each becomes a slot `(at, kind, target, offset)`.
  The target is the callee's **name** if it is named, or its **content hash** if anonymous.
- `PCREL_LO12` points at the `auipc` label, so it is stored as a link to its
  `PCREL_HI20` slot in the same chunk, not as a target.
- Hash = SHA-256 over kind, alignment, blanked bytes, and slots.
- Handled: `32, HI20, LO12_I/S, PCREL_HI20, PCREL_LO12_I/S, CALL(_PLT), JAL, BRANCH,
  RVC_JUMP, RVC_BRANCH`. `RELAX` and anything else are hard errors.

## Results

| Criterion | Result |
|---|---|
| Every relocation handled | ✅ both models; 0 unknown, 0 unresolved |
| Identical source → identical hashes | ✅ 13 named chunks identical across clean rebuilds |
| One function changed → only its hash | ✅ `combine` edit changes only `combine`; `helper::mix` edit changes only `mix` |
| Same string → same constant chunk | ✅ `greeting_sum`, `greeting_len` (testcrate) and `shared_greeting` (helper) all use chunk `0a69653eeeb5…` |
| Stretch: flat image runs | ✅ `spike_entry` / `spike_entry2` linked, run in an RV32IMC emulator, 10/10 results match native x86 |

## Findings

1. **Rust `&str` literals are not in mergeable string pools.** Each gets its own
   `.rodata..Lanon.<hash>.N` section with no terminating zero, so it is already one chunk.
   Only C-string literals (`c"..."`) land in `.rodata.str1.1`. Small constants can land
   in `.rodata.cstN` (mergeable, fixed size). One chunk per section handles both.
2. **The default code model (medlow) has no PC-relative pairs.** It uses absolute
   `HI20`/`LO12`. HI/LO pairing only happens with `-C code-model=medium`, which this
   spike also builds and checks.
3. **The plan's four functions don't cover every relocation shape.** I added a
   `&str` static (`R_RISCV_32` in data), a jump table (`R_RISCV_32` into the
   function's own chunk at an offset), and C strings (a string pool), plus `spike_entry2`
   to drive them.
4. **Naming callees by name is what makes "change one, only one changes" hold.**
   With pure Merkle hashing (callee by hash), every caller up the chain would change too.
   Names are rustc v0 mangled names. Their crate disambiguator (`Cs37q7ggDDgLf`) came
   from cargo metadata, was stable across code edits, and was even the same across code
   models. It would change with the package path, version, or features.
5. **Precompiled `core` can't be chunked as shipped.** It was built with relaxation
   (102 `R_RISCV_RELAX`) and has `.eh_frame` (`R_RISCV_32_PCREL`). Rebuilding it with
   `-Zbuild-std=core` and the same flags fixes both: core + compiler_builtins + our
   crates = 1651 chunks (1405 distinct), 0 unknown relocations. The only unresolved
   name is `rust_begin_unwind`, the `#[panic_handler]` a final binary supplies.
6. **Content dedup is real.** compiler_builtins alone: 570 chunks, 384 distinct.

## Not covered

- Cycles among anonymous chunks: detected and rejected, never seen in practice.
- A pointer one past the end of a split string piece would resolve to the next piece.
  It never happens in Rust output.
- TLS, writable `.data`/`.bss` beyond the plumbing: no test case exercises them.
- The emulator covers RV32IMC user-level only (no CSRs, no ecall).
