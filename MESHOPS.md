# Mesh ops: registry, ABI and loading (round seven)

The boundary every applet depends on: interpreted guest code calls named ops,
and each op is provided either natively by the kernel or by another applet.
Callers can't tell which.

- **Code:** `meshops/kernel` (registry, loader, dispatcher), `guest/applets` (test applets),
  `meshops/tests` (the shared test runner).
- **Results:** [meshops/RESULTS.md](meshops/RESULTS.md).

## Call ABI

| | |
|---|---|
| Instruction | RISC-V `ecall` (`0x00000073`) |
| Op | import **slot index** in **`a7`** (x17) |
| Arguments | `a0`–`a5` (x10–x15) |
| Result | `a0`, plus `a1` if the op needs 64 bits |
| Clobbered | `a0`, `a1` only. Every other register survives the call. |
| Memory | shared: pointers pass straight through, nothing is copied |

- **Why `a7`:** it's the register Linux and most RISC-V SBI-style ABIs use for the
  call number. It's the last argument register, so it never collides with
  `a0`–`a5`.
- **Why a slot, not a name hash:** the slot is resolved at load, so a call costs an
  array index and never a hash lookup.

In the guest (`guest/applets`):

```rust
#[no_mangle]
pub static B_IMPORTS: Imports<3> = Imports::new([names::ECHO, names::ECHO_LOUD, names::RECURSE]);
// slot 0 is test.echo.v1:
let sum = op(0, buf.as_ptr() as u32, buf.len() as u32, 0);
```

`op()` is a three-line `asm!("ecall")` wrapper.

## Names, and why the hash isn't trusted

Names are FNV-1a 64 over the full versioned name, e.g. `kernel.uart.write.v1`
or `test.echo.v1`.

- **Accidental collisions aren't a worry** at 64 bits. Round two found none
  across 1,281 names.
- **Deliberate ones are.** FNV is easy to collide on purpose, so someone could
  craft a name with the same hash as `kernel.uart.write.v1`. If the registry
  trusted hashes alone, that name could shadow a kernel op.
- **So the hash is only an index, never a security boundary:**
  - **Manifests carry full names.** Every import and export entry has the name's
    address and length (in the applet's own image) next to its hash.
  - **The kernel recomputes the hash.** A hash that isn't the FNV-1a 64 of its name
    fails the load (`NameHashMismatch`).
  - **The registry keeps the full name in each entry,** as a slice of the code
    blob, not a copy.
  - **One name per hash.** Registering a name whose hash a *different* name already
    holds fails (`HashCollision`).
  - **Imports must match names.** An import resolves only to an entry with the same
    name (`NameMismatch`).
  - **Only the kernel registers `kernel.*`** (`ReservedNamespace`).
- **Cost:** none at call time. Calls still go through the slot table and never hash
  or compare names. The checks cost load time and 8 bytes per entry (below).
- **Real trust** (who may publish which names) comes later, from namespace signatures.

Test 8 covers this on every target:
- S1 exports `kernel.nop.v1`;
- S2 exports `test.echo.v1`'s hash under the name `evil.echo.v1`;
- S3 imports with that forged pair.

All three loads fail and the registry is unchanged. A *real* crafted collision
(two consistent names, one hash) is covered by the kernel's unit tests, which
fake the hash, since building a true FNV-1a 64 collision in a test isn't practical.

## Registry

- **Layout:** an append-only entry array (32 entries) plus an open-addressed
  index of 64 `u16` buckets that maps hash → entry index.
- **Entries never move,** so an entry index stays valid for the registry's whole
  life. Growing it means a bigger entry array and rebuilding only the bucket index.
- **Entry:** `{ hash: u64, name: &[u8], provider, hooks: { first: u16, count: u16 } }`, 32 bytes on Thumb.
  - **Provider:** `Native(fn)` or `Guest { applet, entry address }`.
  - **`hooks`:** reserves a range in a future middleware pool. It's always empty
    this round and nothing runs it.
- **Size:** 1,160 bytes for the whole registry on Thumb.

## Registration: manifest exports, at load

An applet that provides ops declares them in a static export table
(`<X>_EXPORTS`: count, then `{ hash, function, name address, name length, reserved }` × N,
24 bytes each). Imports are `{ hash, name address, name length }`, 16 bytes each.
The kernel reads that table when the applet loads. There's no
`kernel.op.register` call. Why:

- **No guest code runs at load.** The kernel validates everything first, so a
  failed load changes nothing.
- **It mirrors imports.** Both are data, so a tool (the chunker, a store) can see
  an applet's whole interface without running it.
- **Self-imports just work.** An applet can import an op it exports (`test.recurse.v1`).
- **A call can still be added later** for ops that only appear at run time.

**The last registration wins:** the entry's provider is replaced in place.
Trust and namespace policy are out of scope this round.

## Loading and resolution

`Kernel::load(id, imports, exports)`:

1. **Check everything first,** so a failed load changes nothing:
   - each hash against its name;
   - exports against the namespace rule and the collision rule;
   - each import must resolve, by hash *and* name, to the registry or to the
     applet's own exports.

   A missing import gives `Error::Unresolved(hash)`, which the test runner prints as
   `unresolved:test.nobody.v1`.
2. **Register** the applet's exports.
3. **Bind** each import slot to its registry **entry index**, giving a 34-byte slot
   table per applet (16 × `u16` + count).

**Cost:** loading H, A and B (11 imports, 3 exports) takes about 4,000 estimated M4
cycles per applet. Most of it is re-hashing names and comparing them; before the
name checks it was about 930. That's still about 60 µs at 64 MHz, once per load.

## Dispatch and nesting

- **On `ecall`,** the interpreter stops (`Stop::Ecall`). The kernel reads `a7`,
  looks up the slot (an array index) and the entry, then runs the provider:
  - **Native:** a Rust function `fn(&mut Kernel, [u32; 6]) -> [u32; 2]`.
  - **Guest:** the interpreter re-enters with a fresh register file, one level
    deeper, on its own 2 KB slice of guest stack.
- **The result** goes back into `a0`/`a1`, and the caller resumes after its `ecall`.
- **Maximum depth:** `MAX_DEPTH = 8` levels, the outermost call included.
  - In the test, B calling `test.recurse.v1(6)` reaches depth 7 and works.
  - `test.recurse.v1(7)` would need depth 8, so the whole top-level call fails with
    `Error::DepthExceeded(8)`. The kernel is unaffected, and the next call works (tested).
  - Aborting the whole call was chosen over returning an error code to the guest:
    runaway recursion is a bug, and v1 trusts applets, so fail loudly.
- **Stack per level:**
  - Host: 464 bytes on Thumb (`run_guest` 392 + `dispatch` 72).
  - Guest: 2 KB, from the top of the shared guest RAM (16 KB for 8 levels).

## Swapping a provider

The applet slot tables hold **entry indices**, and registering a new provider
replaces the entry in place. So when A2 registers `test.echo.v1`:

- **Applets loaded before the swap** (B) reach A2 on their next call, without
  re-resolving (test 7).
- **Applets loaded after the swap** reach A2 too.

**Recommendation: keep this live binding.**
- **Simplicity:** one rule covers every caller, swapping costs one write, and the
  call path has one extra indirection (slot → entry → provider).
- **Rejected alternative:** copy the provider into the slot table at load, so each
  applet keeps the provider it saw. That makes swaps invisible to running applets
  and turns "update an op" into "reload every caller".
- **What live binding still needs:** unloading a provider should replace its entries
  with a "gone" provider that fails cleanly. That isn't built yet; there's no unload
  this round.

## What the design changed

- **Interpreter** (`bench/interp/src/fast.rs`): one new op (`Ecall`), plus
  `call` split into `start` and `run` (resumable, returns `Stop::Returned | Ecall`).
  - No per-target code.
  - **Side effect:** the split changed the compiler's output for the hot loop, and
    `fast` got slightly faster. Under Unicorn, crc16 goes from 39.3x to **36.8x** and
    parse from 26.0x to **24.3x**, at 27 cycles per guest instruction instead of 29.
    Most of it is fewer dispatch cycles. `bench/RESULTS.md` has the new figures.
  - **One rule that matters:** all applets must be in **one contiguous code
    region** (the "blob"). The interpreter has a single image and decode table,
    and pointers to another applet's rodata must resolve.
- **Chunker:** `chunker link --base` links each applet at its own address in that
  region.
- **Manifest format:** imports and exports have to become first-class manifest
  fields: hash *and full name* for each, with export → entry address. A loader can then resolve before
  fetching code, and a store can check that a set of applets is complete without
  running them.
- **Build:** `meshops/build_applets.py` links each applet separately, so shared
  code (`memcpy`, `core` panics) is duplicated per applet. Sharing it is next
  round's chunk-store work.
