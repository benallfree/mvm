# Spike: chunking rustc RISC-V output

**Goal:** prove that real rustc RISC-V output can be split into self-contained,
hashable chunks with nothing left unresolved.

**Result: yes.**
- **Round one** (`2c397d4` on `greenfield`): 12/12 checks passed.
- **Round two** (`spike-round2`): slots and manifests are keyed on stable names that
  survive version bumps and new checkouts. `./check.sh` passes 21/21.

The speed spike is planned in [SPEED_SPIKE.md](SPEED_SPIKE.md).
**Round three** (`spike-speed`) adds a cycle estimate without hardware: Unicorn plus
a Cortex-M4 cost table. Pre-decoded, the interpreter is 51–77x slower than native
Thumb; the simple loop is 90–148x. Details are in SPEED_SPIKE.md and `bench/RESULTS.md`.
**Round four** (`spike-aot`) adds `bench/aot`, an RV32IMC → Thumb-2 ahead-of-time
translator. Its output is position independent and comes within 1.98x (crc16) and
3.34x (parse) of native.
**Round five** (`spike-interp-opt`) adds `bench/interp/src/fast.rs`, an optimised
interpreter at 39x (crc16) and 26x (parse), down from 77x and 51x pre-decoded.
**Round six** (`spike-emu`) runs that same interpreter, unchanged, on an emulated
nRF52840 (Renode, Embassy) and an emulated ESP32-S3 (Espressif QEMU, esp-hal).
`emu/run.sh` runs both, and all 10 checks match native. See `emu/RESULTS.md`.
**Round seven** (`spike-meshops`) adds mesh ops. Guest code calls named ops with `ecall`,
and each op is provided by the kernel or by another applet. All 7 tests pass on the x86
host, under Unicorn, on Renode's nRF52840 and on QEMU's ESP32-S3. See
[MESHOPS.md](MESHOPS.md) and `meshops/RESULTS.md`.

## Running it: Docker (stable environment, in or out of the cloud)

```sh
docker/build.sh        # builds the `mvm-env` image (toolchains + emulators: 1.5 GB in `docker images`, 4.5 GB unpacked)
docker/run.sh          # runs docker/run-all.sh: check.sh, bench/run.sh, emu/run.sh
docker/run.sh emu/run.sh   # or any one command, with the repo mounted at /work
```

**What the image holds:**
- **Base:** Ubuntu 24.04, pinned by digest.
- **Rust:** 1.97.0 (host tools, Thumb firmware) and nightly-2026-11-04 with
  `rust-src` (the RV32IMC guest). The same pins are in every `rust-toolchain.toml`.
- **Python:** unicorn 2.1.4, capstone 5.0.9, pyelftools 0.33.
- **Emulators and Xtensa tools,** from `emu/setup-tools.sh` (the same script works
  without Docker):
  - Renode 1.17.0+20261105git85a5639da;
  - Espressif QEMU 9.2.2-20260417;
  - Xtensa GCC 14.2.0_20241119;
  - Xtensa Rust 1.97.0.0, linked as `esp197`;
  - espflash 4.7.0.

**Behind a TLS-intercepting proxy** (a Claude Code cloud session, for example), build
with `MVM_EXTRA_CA=/root/.ccr/ca-bundle.crt docker/build.sh`. Both scripts pass any
`HTTPS_PROXY`/`NO_PROXY` settings through and use host networking when a proxy is set.

**Network at run time:** crates are still fetched from crates.io. The image holds
toolchains, not the dependency cache.

## Layout

| Path | What |
|---|---|
| `guest/` | `testcrate` (no_std) + `helper` (second crate) + `applet` (uses core formatting, memcpy, a bounds-check panic), pinned nightly |
| `guest/build.sh` | The builder. It pins the flags, emits `.o`, and prints the chunker inputs. Env: `PKG`, `CODE_MODEL=small\|medium`, `BUILD_STD=1`, and `LTO=fat` (measurement only). |
| `chunker/` | The host tool. Commands: `dump`, `hashes`, `roundtrip`, `applet-roundtrip`, `names`, `manifest`, `compare`, `link` (writes a flat image and entry addresses to disk) |
| `guest/kernels` | Speed-spike workloads: `crc16` and `parse` |
| `meshops/` | Round seven: `kernel` (registry, loader, ecall dispatch), `tests` (shared runner), `host`, `fw-unicorn`, `build_applets.py`, `run.sh` |
| `guest/applets` | Round-seven test applets (H, A, A2, B, M) |
| `docker/` | `Dockerfile`, `build.sh`, `run.sh`, `run-all.sh`: the pinned environment |
| `emu/` | Round six: `bench-core` (shared firmware body), `nrf52840` (Embassy, Renode), `esp32s3` (esp-hal, QEMU), `setup-tools.sh`, `run.sh` |
| `bench/` | Speed spike: `interp` (no_std simple and pre-decoded RV32IMC interpreters), `aot` (RV32IMC → Thumb-2 translator), `fw` (Thumb firmware for Unicorn), `harness/` (Unicorn and the M4 cost model), `run.sh` |
| `check.sh` | Every check, for both code models, plus the applet |

Builder flags: `-C target-feature=-relax -C codegen-units=1 -C lto=off -Z function-sections=yes -C code-model=<small|medium>`.
`lto = "off"` is pinned in the profile for chunked builds. `LTO=fat` exists only for the one-off measurement below.

## Chunk model

- One chunk per allocated section. Mergeable string pools (`SHF_MERGE|SHF_STRINGS`)
  are split after each zero byte, one chunk per string.
- A chunk is **named** if a real symbol sits at offset 0, otherwise **anonymous**.
  A named symbol anywhere else inside a chunk is an error.
- Relocated fields are blanked; each becomes a slot `(at, kind, target, offset)`.
  The target is the callee's **stable name** if it is named, or its **content hash** if anonymous.
- `PCREL_LO12` points at the `auipc` label, so it is stored as a link to its
  `PCREL_HI20` slot in the same chunk, not as a target.
- Hash = SHA-256 over kind, alignment, blanked bytes, and slots.
- Handled: `32, HI20, LO12_I/S, PCREL_HI20, PCREL_LO12_I/S, CALL(_PLT), JAL, BRANCH,
  RVC_JUMP, RVC_BRANCH`. `RELAX` and anything else are hard errors.
- **Manifest** = stable name → chunk hash, for every named chunk an applet's closure
  references, plus its entry points.

### Stable names (new in round two)

`chunker/src/stable.rs` is a v0 demangler that drops every crate-root hash and keeps everything else:
- **Dropped:** the crate-root hash, e.g. `_RNvCs37q7ggDDgLf_9testcrate7combine` → `testcrate::combine`.
- **Item disambiguators** stay, as `#N`, e.g. `do_panic#2::runtime`. Closures become
  `{closure#N}`, shims `{shim:name}`.
- **Generic arguments** are printed in full, with their crate hashes dropped too.
- **The instantiating crate** of a generic copy stays as a suffix, e.g.
  `<applet::Buf as core::fmt::Write>::write_fmt @applet`. Two crates' private copies
  of the same generic therefore stay distinct.
- **C symbols** (`memcpy`) pass through unchanged.

Resolution, slots, hashes and manifests all use stable names. When two different raw
symbols map to one stable name, loading fails, naming both.

## Results

### Round one criteria, rerun on stable names, for both code models

| Check | small | medium |
|---|---|---|
| Every relocation handled, nothing unresolved | ✅ | ✅ |
| Clean rebuild → identical names and hashes (13 manifest entries) | ✅ | ✅ |
| Edit `combine` → only `testcrate::combine` changes | ✅ | ✅ |
| Edit `helper::mix` → only `helper::mix` changes | ✅ | ✅ |
| 3 functions in 2 crates share one `"hello, mesh"` chunk | ✅ | ✅ |
| Flat image round trip: 10/10 match native | ✅ | ✅ |

### Round two checks

| Check | small | medium |
|---|---|---|
| `helper` 0.1.0 → 0.1.1, no code change: every name and hash identical (raw crate hash `Cs5BXFcsRaI0R` → `Csik9qsaUKZBH`) | ✅ | ✅ |
| `helper` 0.1.2 + `mix` body change: only the `helper::mix` manifest line changes, no `testcrate` caller | ✅ | ✅ |
| Build from a different checkout path: identical | ✅ | ✅ |

| Applet check | Result |
|---|---|
| Names over core + compiler_builtins + our crates: 0 stable-name, 0 8-byte-name-hash and 0 16-byte-chunk-hash collisions | ✅ |
| Applet round trip in the emulator: 3 in-range calls byte-identical to native, 2 out-of-range calls end in `rust_begin_unwind` | ✅ |
| Applet from a different checkout path: 1151 manifest entries (all of core), identical | ✅ |

### Stable names per crate

| Crate | v0 names | Plain (C) names |
|---|---|---|
| core | 640 | 0 |
| compiler_builtins | 334 | 289 |
| testcrate | 9 | 2 |
| helper | 2 | 0 |
| applet | 4 | 1 |
| **total** | **989** | **292** |

1281 names; 0 collisions as stable names and 0 as 8-byte name hashes. The 1416
distinct chunks also have 0 collisions as 16-byte truncated hashes.

### Manifest size

| Encoding | testcrate (13 entries, 1093 B reachable) | applet (17 entries, 4207 B reachable) |
|---|---|---|
| a) stable names + 32 B hashes | 664 B = 60.8% | 1111 B = 26.4% |
| b) 8 B name hash + 16 B chunk hash | 313 B = 28.6% | 409 B = 9.7% |
| c) b + names in a separate dev file | 313 B = 28.6% (dev file 352 B) | 409 B = 9.7% (dev file 703 B) |

- Sizes include a varint length per name and a varint count.
- Mean stable-name length: testcrate 19 B, applet 33 B (longest 53).
- These count the manifest only. Slots inside chunks also carry their target's name,
  and encoding b would shrink those too. That isn't measured here.

### LTO, measured once (the applet's closure)

| Build | Chunks | Reachable bytes |
|---|---|---|
| `lto = off` (chunked) | 30 | 4207 |
| `lto = "fat"` | 19 | 3056 (−27.4%) |

8 chunk hashes are shared: 26.7% of the off build's chunks, 42.1% of the fat build's.

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
   *Round two:* round one keyed slots on raw mangled names. Its diff output hid the crate
   hash, but the hashes didn't, so a version bump would have changed every chunk that
   names something in the bumped crate. Stable names fix that: the version-bump checks
   pass with the raw crate hash changed.
5. **Precompiled `core` can't be chunked as shipped.** It was built with relaxation
   (102 `R_RISCV_RELAX`) and has `.eh_frame` (`R_RISCV_32_PCREL`). Rebuilding it with
   `-Zbuild-std=core` and the same flags fixes both. *Round two:* with the applet supplying
   a `#[panic_handler]`, core + compiler_builtins + applet resolve completely.
6. **Content dedup is real.** compiler_builtins alone: 570 chunks, 384 distinct.
7. **A new checkout path doesn't change cargo's crate hash** (round two). Cargo hashes
   workspace members by their path relative to the workspace, so moving the whole
   checkout leaves the raw names as they were. The check still passes on identical
   names and hashes, but it doesn't exercise the stable names. The version bump does.
   Panic `Location` file strings in the applet were also path-independent: workspace
   members get relative paths, and core gets the rust-src path.
8. **The real core names exercised the demangler** (round two). Nested closures produce
   back-to-back `0` identifiers (`...3fmt00`). A v0 decimal `0` is a single digit, which
   a first version got wrong. It's now a unit test.
9. **Manifest cost depends on name length, not on code.** Full names cost 26% of the
   applet's bytes. Hashed names (b) cost under 10%, and keep it that way for any
   applet larger than this one.
10. **LTO and chunking don't mix.** Fat LTO saves 27% here, but only about a quarter
    of the chunks survive it unchanged. The 8 survivors are both `memcpy` chunks
    (compiler_builtins is never LTO'd), 4 constant tables, the 2-byte panic handler,
    and one small core leaf (`write_prefix`). Every applet and formatting function
    changes. LTO stays off for chunked builds.

## Not covered

- Cycles among anonymous chunks: detected and rejected, never seen in practice.
- A pointer one past the end of a split string piece would resolve to the next piece.
  It never happens in Rust output.
- TLS, writable `.data`/`.bss` beyond the plumbing: no test case exercises them.
- The emulator covers RV32IMC user-level only (no CSRs, no ecall).
- Two semver-incompatible versions of one crate in the same build would collide as
  stable names. Loading fails loudly; the spike doesn't try to resolve it.
- v0 const generics beyond integers, bool and char fail loudly. None appear in core.
- Features: not varied here. They change cargo's metadata the way a version does, so
  the version-bump checks should cover them, but that hasn't been run.
