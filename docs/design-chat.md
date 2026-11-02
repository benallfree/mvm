# Mesh Virtual Machine: original design conversation

Transcript of the shared claude.ai chat between Ben and Claude that the crate plan in [mesh_virtual_machine_5ae0e741.plan.md](mesh_virtual_machine_5ae0e741.plan.md) was derived from. Converted from the saved share page.

---

## Ben

I'd like to discuss an idea I've been kicking around for about a year. IMHO the mesh firmwares such as Meshtastic and Meshcore, and even Reticulum, are stunted by being monolithic architectures instead of platforms that can run applets. To my way of thinking, there should be a microkernel that provides a thin layer to the hardware itself and then pushes as much into bytecode userland as possible. That way people can install an applet like JVM and it just works everywhere the microkernel runs. Applets can be userland applets or system applets. System applets can extend the kernel host namespace with new functions and even replace functions provided by other or builtin applets. There is a hook system where the kernel manages events and filters (map and reduce) in addition to providers.

something i've run into repeatedly is the memory model. we tried wasm, and even wasm transpiled into an interpreter-optimized bytecode, and even wasm with the page size experimental feature, but my conclusion is that on a 256kib sram Nordic chip with 1mb flash memory, applet guest memory cannot be reserved. it should be requested from the kernel allocator and then in the opcodes that read and write memory should check to make sure the access is in bounds of an allocated portion. applets and the kernel should be able to access the same address space, in other words. like there should be a malloc for private memory and a smalloc for shared access. that way, a kernel can pass arguments or a reducer chain can run without copying the memory over and over.

it would be ideal if people could use any high level language to compile down to mesh bytecode, and wasm is a good model except for its guest memory architecture and i don't know what it would take to convince it not to use that. so, then is it possible to write something for gcc let's say, or rust or llvm, that targets this bytecode architecture the eway i want it?

the end goal is to enable devs to distribute their applets without the user needing to recompile or re-flash firmware. so the kernel has some kind of basic usb/ble comms so the user can use a desktop app to mount the filesystem and such.

i'd prefer it all be in rust so its portable across all platforms that might eventually want to run it.

reticulum took an interesting approach where the lora radio is a dumb modem and the brains live in python. this works until you want to run a solar repeater somewhere. so even if things are running in a desktop or mobile environment, i think i still want the applet architecture that can run on very memory constrained devices.

right now i have about 50 solar repeaters deployed across nevada running meshcore. they are updatable over lora. if i could migrate them to a microkernel architecture and implement the basic meshcore messaging as an applet and then the repeater logic as a separate applet, i could install both of those to create a meshcore-compatible repeater and eject from meshcore altogether. people could continue using meshcore clients until i came up with my own system, and if i chose a dumb radio modem approach like reticulum, that would be the exact same microkernel but with a different radio modem applet.

do you get what I mean?

---

## Claude

Yes, I get it. You want a thin Rust kernel owning the hardware, scheduler, allocator, and hook/event bus, with everything protocol-level (Meshcore messaging, repeater logic, a modem shim) shipped as sandboxed, signed, over-the-air-updatable bytecode that runs unchanged on an nRF52 or a desktop. The Meshcore-compat-as-an-applet path for your 50 repeaters is a good migration story, since you could leave Meshcore's firmware without leaving its network.

**The memory model is right.** On 256 KiB of SRAM, per-applet reserved linear memory is a non-starter, and a shared address space with checked access is the sensible alternative. This is also close to Femto-Containers on RIOT OS, which uses small eBPF VMs on Cortex-M with hooks and a per-access check against a short list of permitted regions. It's worth reading how they handled it. The cost concern is the per-load/store lookup. Keep each applet's region list short (stack, data, a handful of heap/shared blocks), cache the last-hit region, and flag regions read-only or read-write. smalloc'd buffers can transfer ownership (the sender loses access) so reducer chains stay zero-copy without aliasing hazards.

**Can you keep the wasm toolchain but not its memory model?** Mostly yes, and this may be your cheapest path. Wasm's linear memory is a convention of the engine, not the compiler. LLVM can emit wasm32 with `-fPIC`/shared-module conventions (`--import-memory`, `__memory_base`, `__stack_pointer`, `__table_base`). The module's "memory 0" then becomes your whole shared address space, and your interpreter does its own allocation-table check on each access instead of the standard bounds check. The kernel sets `__memory_base` and the stack pointer to blocks it allocated. C and C++ via clang should work; Rust's PIC-wasm support is rougher. You'd need to avoid `memory.grow` and use passive data segments or a custom loader.

**Writing your own backend vs. borrowing an ISA.** A new LLVM backend is a lot of work, and GCC is more so. A cheaper option is to make your bytecode an existing ISA that already has compilers:

- **RV32E(C) interpreted in Rust.** There are Rust targets for it (tier 3/nightly last I knew), GCC and LLVM both support it, and the interpreter is tiny. It's a flat 32-bit address space, which maps naturally to your "same address space as the kernel" idea. Host calls become `ecall`s into a name-linked table, which also gives you function replacement. The downsides are no structured control flow or validation (you rely on memory checks, fuel metering, and checked jumps), and you need PIC or load-time relocation.
- **Wasm-derived.** You get a better validation story and a mature ecosystem, but decoding is heavier. Your experience with interpreter-optimized transpilation shows the RAM cost of side tables.
- **eBPF.** Hooks, maps, helpers, and verified bounds are already its model, and there are Rust and LLVM backends (`rbpf`, `aya`). It's restrictive for general apps, though: limited loops, a tiny stack, and a heavy verifier.

I'd lean RV32E(C) for the dense code, the tiny interpreter, and the flat shared memory, with wasm as the runner-up if you value validated control flow. One caveat: on a 64-bit desktop host, "guest address = host address" breaks, so use 32-bit guest addresses plus a translation step. On the MCU that translation can be nearly free.

**Practical issues that will matter on solar repeaters in the field:**

- **Keep crypto and hashing native.** Ed25519, AES, and SHA-256 in an interpreter will be painfully slow. The "thin" kernel should still expose accelerated primitives as host functions.
- **Keep the radio driver native.** LoRa timing (CAD, RX windows, airtime) shouldn't depend on interpreter latency. Bytecode handles packet logic, and the kernel handles the radio state machine.
- **Run handlers to completion with fuel limits.** Preemptive threading in an interpreter on 256K is more pain than it's worth, but without fuel metering a buggy applet can starve the radio.
- **Execute bytecode in place from flash** so RAM only holds applet state. With a BLE stack taking a good chunk of the 1 MB, flash budget is tight but workable.
- **Replaceable system functions are the biggest risk.** A bad system applet that replaces a core function on a remote node is a truck roll. You need signed applets, capability manifests, A/B slots with watchdog-driven rollback, and an immutable recovery layer that can always accept a fresh image over LoRa or BLE. Hook ordering and priority for competing providers and filters also need defined rules.
- **LoRa OTA gets much better.** A few-KB applet delta is far friendlier to airtime than a full firmware image.

Meshcore's code is MIT, so porting its packet handling into an applet should be mostly mechanical once the VM exists.

Do you want the first prototype to prove out the memory model (VM plus checked shared allocator on the nRF52) or the update/rollback story? I'd tackle whichever you're less sure of first.

---

## Ben

say more about RV32E(C), LLVM, and PIC. i've never heard of these

---

## Claude

Here's each piece in plain terms, and how it fits your design.

**RV32E(C)**

RISC-V is an open instruction set architecture, a specification of what a CPU's machine instructions look like, the way ARM or x86 are. The letters are modifiers:

- **RV32**: the 32-bit flavor, with 32-bit registers and addresses.
- **I**: the base integer instruction set, about 40 instructions (add, load, store, branch, jump).
- **E**: "embedded." It's the same as I but with 16 registers instead of 32, which shrinks register state and is meant for tiny chips.
- **C**: "compressed." Common instructions get 16-bit encodings instead of 32-bit, which usually cuts code size by 25-30%. That matters on 1 MB flash.
- You'd likely also want **M** (multiply/divide), giving something like RV32EMC.

The trick is that you would never run this on real RISC-V hardware. You write a small Rust interpreter that loops: fetch an instruction, decode it, execute it against a register array and your checked memory. A basic RV32 interpreter is a few hundred lines of Rust because the ISA is so small and regular. Your bounds-checked memory model lives in the load/store paths. Host calls use the `ecall` instruction, which exists to say "ask the environment to do something," so the kernel handles it and dispatches by number or name.

The advantage over inventing your own bytecode is that compilers for RISC-V already exist, so you inherit them instead of writing a backend.

**LLVM**

LLVM is a compiler framework that splits compilation in two. A frontend turns source into LLVM's intermediate representation (IR), and a backend turns IR into machine code for a specific CPU. Clang (C/C++), rustc, Zig, Swift, and others are frontends. Any frontend can target any CPU that has a backend. GCC is a separate compiler family with its own structure, and it also supports RISC-V.

So if your bytecode *is* RV32EMC, everything LLVM-based can target it with a flag, and you write no compiler code. If you invent a new ISA, you'd have to write an LLVM backend yourself, which is a serious multi-month project. For Rust specifically, there are RV32E targets (`riscv32e-unknown-none-elf` and friends), but as I said, they've been tier 3, meaning nightly-only and built with `-Zbuild-std`. Check their current status, since that moves. C and C++ via clang are the smoothest path today.

**PIC (position-independent code)**

Normally a compiler and linker assume the program lands at a known address. Global variable access, function pointers, and jump tables all bake in absolute addresses. Your applets are loaded wherever the kernel allocator puts them, several at once, with code in flash and data in RAM. PIC means the generated code finds things relative to the program counter or through a table (the GOT), so it works wherever it's placed.

There are a few ways to handle this:

1. **Load-time relocation.** Build as a position-independent executable, and the kernel's loader patches addresses when installing. This is the general approach, but patching means the code lives in RAM unless you do the patching at install time and write the result to flash, which works but needs care.
2. **Fixed virtual windows (my suggestion to consider).** Because you control the VM, link every applet at the same virtual addresses, say code at `0x0000_0000` and data at `0x2000_0000`. The VM maps those windows to wherever the code and data really sit. No PIC or relocation is needed for the applet's own code and globals. Pointers returned by `malloc`/`smalloc` are real shared-space addresses and need no translation. The cost is one base-add per access in the window case, but your region check is already doing a lookup per access, so it can fold in.
3. **Base register conventions.** Some toolchains reserve a register as the data base (ARM's `-msingle-pic-base`). RISC-V toolchains don't offer that as cleanly, and with only 16 registers on RV32E, sacrificing one hurts.

I'd start with option 2 because it keeps the toolchain simple (plain static linking with a custom linker script) and puts the complexity in your loader, where you control it.

**What the toolchain flow would look like**

1. Write the applet in C, C++, or Rust.
2. Compile with clang (or rustc) for `riscv32` with E, M, C, soft-float, and no OS.
3. Link with your linker script, with host functions left as undefined imported symbols (`mesh_send`, `hook_register`).
4. Run a small packer tool that takes the ELF and emits your applet format: code, initialized data, a manifest of required capabilities, and an import table of host function *names* the kernel resolves at load time. Name-based linking is what lets system applets override or extend host functions.
5. Sign it.

An SDK is mostly a header or crate declaring those imports, plus the linker script and packer.

One tradeoff to keep in mind: unlike wasm, RISC-V gives you no structural validation. The VM can't prove up front that jumps stay inside the applet. You enforce it at runtime by checking that the program counter stays inside the applet's code region, along with the load/store checks. That's cheap, and it's the price of the simplicity.

If you want, I can sketch the interpreter's memory-access path with the region table and cached last-hit, since that's where your design differs most from the standard approach.

---

## Ben

explain more about the memory model. who owns the call stack, who allocates it, how does kernel malloc work when called from an applet, what address does the applet see, how would shared allocations work (scratchpad basically)

---

## Claude

I'll walk through it in layers, since "who owns what" differs for each kind of memory.

**What the applet sees**

The applet sees plain 32-bit integers and nothing else. I'd split the address space into windows:

| Range | Contents | Backing |
| --- | --- | --- |
| `0x0000_0000`-`0x0000_FFFF` | unmapped, so NULL derefs trap | none |
| `0x0001_0000`+ | applet code and rodata, execute/read only | flash, run in place |
| `0x1000_0000`+ | applet statics (`.data`/`.bss`), per applet | a RAM block allocated at load |
| `0x2000_0000`+ | the arena: stacks, malloc, smalloc, packet buffers | kernel-managed SRAM |

The arena is the truly shared space. On an nRF52840, SRAM actually lives at `0x2000_0000`, so guest address equals physical address and there's no translation at all, just the permission check. On a desktop host, the interpreter adds a base offset to an allocated buffer. A `Memory` trait with two implementations, monomorphized so there's no dynamic dispatch, handles both.

One caveat: because every applet is linked at the same static addresses, a pointer into applet A's globals means something different to applet B. Only pointers into the arena (malloc, smalloc, stack arguments) are meaningful across applets. The SDK rule is "never pass the address of a global across an applet boundary," which is easy to follow and fails safe, since the receiver just reads its own memory. If you'd rather have one truly flat space, the alternative is to place statics in the arena at load and relocate pointers in `.data`, which costs little because `.data` gets copied to RAM anyway. It just brings PIC/GOT handling back into the toolchain.

**Who owns the call stack?**

There are two separate stacks:

1. **The guest stack** holds the applet's locals, spills, and saved return addresses, using the normal RISC-V `sp` convention. The kernel allocates it from the arena and sets `sp` before invoking a handler. Overflow needs no guard page, because the stack is a region and an access below its base fails the bounds check and traps. The manifest declares a size (say 1-2 KiB).
2. **The host stack** is the kernel's own Rust stack. The interpreter loop runs on it, and guest calls don't recurse into it, since a `jal` is just a PC change inside the loop. Host calls are ordinary Rust function calls that return to the loop.

Since handlers run to completion and never yield mid-call, the guest stack is empty between events. That means you don't need one per applet. A single shared stack pool, sized for the deepest declared need, works for all of them, which on 256 KiB saves a lot (ten applets at 2 KiB is 20 KiB, versus roughly 4 KiB shared). Two details:

- Zero the used portion on switch, or you leak one applet's stack contents to the next. Tracking the high-water mark keeps it cheap.
- If a host call dispatches into another applet (a hook chain), either let the kernel dispatcher call handlers iteratively in a loop, or give nested invocations a fresh slice of the pool below the current one. The iterative approach has no nesting at all.

To detect handler return, initialize `ra` to a sentinel address outside the code window. When the handler executes `ret`, the PC check fails in a recognizable way, and the kernel treats that as "returned" rather than a fault.

A subtle point: the guest can overwrite its own return address and jump anywhere inside its own code window. That's fine. Bounds checks protect applets from each other, not from themselves.

**How malloc works from an applet**

Two layers, and I'd make the default the layered one:

- **Kernel allocator.** This hands out blocks from the arena. Keep its bookkeeping out of band, or put headers in a gap that no region covers, so a guest that corrupts its own block can't corrupt allocator metadata. Each block it hands out is recorded as a region (`start`, `len`, `flags`) in the calling applet's table.
- **Applet-side malloc.** The SDK's `malloc` is a small allocator inside chunks the applet requests from the kernel via an `ecall` (`mem_chunk(size)`), grown on demand. Direct per-allocation ecalls are also available.

I prefer the chunk approach because fragmentation is the main danger on a node that runs for months on 256 KiB. With per-applet heaps, one applet's churn fragments only its own chunks. The region table stays short (a few entries per applet), which matters for the checker's speed. Quotas are one number per applet, and when an applet crashes or is unloaded the kernel reclaims everything by freeing its chunks, with no walking of individual allocations.

For the hottest case, packets, use fixed-size pool slots (say 256 B, enough for a LoRa frame) instead of general malloc. Pools can't fragment. A pooled `smalloc` for packet buffers would probably be your most-used call.

**Shared allocations (scratchpads and zero-copy)**

A shared block has one owner at a time, and everyone else gets a revocable grant. There are two use patterns:

*Transient lending, for hook and reducer chains.* The radio code gets a packet buffer `P` from the pool. The dispatcher runs the filter chain like this:

1. Grant `P` to the next handler (read-write or read-only, per the hook's declaration), pass `(ptr, len)` in `a0`/`a1`, and run it.
2. Revoke the grant when the handler returns. The grant list is stack-like since handlers don't overlap, so this is a push and a pop.
3. Optionally "move" instead of lend, which transfers ownership permanently (the sender loses access).

No copy happens anywhere. A handler that stashes the pointer and touches it later gets a trap, because the region is no longer in its table. That is your lifetime enforcement. One edge case: if the address is later granted again for a different block, a stale pointer silently works on the new block. A small generation counter in the grant can catch that, if you care.

*Persistent scratchpads, for state shared between applets.* For example, the routing applet maintains a neighbor table that a display applet reads. `smalloc(size, name)` creates a named block, and `smap(name, perms)` adds a region to another applet's table. The manifest must declare the capability, and the owner (or policy) decides whether each mapper gets read-only or read-write access. Refcount mappings so an owner crashing doesn't yank memory out from under readers mid-handler. Either keep it alive until the last unmap, or revoke and let readers trap on next access. I'd pick keep-alive for named scratchpads.

Atomicity is nearly free. With one core and run-to-completion handlers, a handler sees a consistent snapshot, so you don't need locks. That changes if you ever interrupt handlers or go multicore.

**The check itself**

A sketch of the hot path:

```rust
struct Region { start: u32, end: u32, perms: u8 }

fn check(&mut self, addr: u32, len: u32, need: u8) -> Result<(), Trap> {
    let end = addr.checked_add(len).ok_or(Trap::Bounds)?;
    let r = &self.cache;                    // last-hit region
    if addr >= r.start && end <= r.end && r.perms & need == need {
        return Ok(());
    }
    // slow path: scan the short sorted list (stack, statics, grants, chunks)
    for (i, r) in self.regions.iter().enumerate() {
        if addr >= r.start && end <= r.end && r.perms & need == need {
            self.cache = *r;
            return Ok(());
        }
    }
    Err(Trap::Bounds)
}
```

The access must fall entirely inside a single region, so straddling two adjacent blocks traps. Compile applets with `-mstrict-align`, which tells the compiler not to emit misaligned loads, so the interpreter can reject them and keep each access to one check. Separate caches for stack accesses and heap accesses help in practice, since a handler alternates between them.

If the list gets long in practice, the alternative is a granule ownership table (one tag byte per 32 B of arena). That gives an O(1) check but costs several KiB of RAM, which is steep on this chip, so I'd start with the region list.

Want me to go through the hook dispatch path next? The grant/revoke sequence and how replaced host functions interact with it are where this design gets trickiest.

---

## Ben

> Applet-side malloc. The SDK’s `malloc` is a small allocator inside chunks the applet requests from the kernel via an `ecall` (`mem_chunk(size)`), grown on demand. Direct per-allocation ecalls are also available.

i don't know baout this, this sounds like guest memory again where it asks for more than it needs

---

## Claude

You're right. The chunk approach is guest memory with a smaller footprint: each applet's chunks carry slack that no other applet can use. With ten applets each holding 2-4 KiB of unused chunk space, you've stranded 20-40 KiB of a 256 KiB chip. I was trading external fragmentation for stranded slack, and on this hardware slack costs more.

There's also a speed argument for your original idea. A malloc written in C and run in the interpreter costs hundreds of interpreted instructions per call. An `ecall` into a native Rust allocator is one dispatch plus a fast native routine. Direct kernel allocation is likely faster, and `ecall` here is just a function call inside your interpreter loop, not a context switch.

**What the direct approach costs, and the fixes**

1. **Region table growth.** If every live allocation is its own region entry, the checker's slow path gets long. But you only need to protect applets from each other and from the kernel, not an applet from itself. An overflow from one of applet A's blocks into another of A's blocks is A's problem. So ownership can be coarse: a granule table over the arena, one owner ID per 32-64 B granule. For a 128 KiB arena at 64 B granules, that's 2K entries, or 1-2 KiB of RAM (less with 4-bit owner IDs). The check becomes `owner[addr >> 6] == current_applet`, which is O(1) no matter how many allocations exist. Grants (shared or lent blocks) stay as a short list checked only on the slow path.
2. **Fragmentation in one shared heap.** Use size classes (16/32/64/128/256 B) backed by slabs, plus a fallback for rare large blocks. Slabs are carved from granule-aligned pages, so freed slots are reused by the same class instead of splitting and coalescing. Fragmentation then becomes internal rounding waste (a few bytes per allocation), bounded and visible, and not the creeping kind that kills a node after three months of uptime. Raw pointers mean no compaction, so this discipline matters.
3. **Per-applet quotas.** One counter per applet, enforced at allocation. A leaking applet hits its quota instead of eating the repeater's memory, and unloading it walks the granule table to reclaim everything with no per-allocation tracking.

**Two patterns that reduce malloc use**

- **Per-event scratch arena.** The kernel gives each handler invocation a bump allocator (`scratch_alloc(n)`) that resets automatically when the handler returns. Parsing a packet and building a reply is exactly this lifetime. It has zero fragmentation, no `free` calls, and costs about as much as incrementing a pointer. I'd steer most applet code toward it.
- **Declare long-lived needs in the manifest.** Routing tables and neighbor lists are usually known-size, so the loader allocates them at install as part of statics, with exact sizes rather than guessed ones. The applet isn't asking for more than it needs, because the manifest says what it needs and the kernel can refuse to install if it doesn't fit.

So the layering becomes: scratch arena for transient data, pool slots for packets, manifest-declared statics for long-lived state, and a plain `malloc` ecall into slab classes for the leftover cases. I'd keep the applet-side allocator only as an optional library for ported code that expects to own a heap, and not as the default.

The one thing to decide is whether you want the owner-ID granule table or the region list. The granule table is simpler to reason about and faster in the worst case, but it spends RAM on the table itself. If you expect more than about six or eight live blocks per applet, it wins. If applets stay tiny (a repeater applet probably does), the region list is cheaper. Which sounds closer to your repeater workload?

---

## Ben

ok well let's contrast this to tinygo instead of rust as the kernel, which i successfully used and got fairly far in one implementation. as i understand it, tinygo has it's own garbage collector

---

## Claude

Your instinct that the GC matters is right, but it's less about the collector's speed than about who gets to own SRAM.

**What TinyGo gives you**

By default TinyGo uses a conservative, non-moving mark-sweep collector. It also has a "leaking" mode (bump allocate, never free), and newer versions have a precise collector. Non-moving matters for you: a raw address into a Go-allocated buffer stays valid, so the "guest address equals real address" trick still works. Go's slice bounds checks also give you a free first layer of memory safety if guest memory is a `[]byte` and addresses are offsets into it.

Other real advantages are goroutines and channels as the kernel's event loop, good nRF52 support including BLE via `tinygo-org/bluetooth` on the Nordic SoftDevice, existing LoRa drivers, and the same source compiling under regular Go on desktop, where you get a real GC and 64-bit address space. That's a legitimate portability story, arguably easier than Rust's no_std/std split.

**Where it fights your memory model**

1. **Two pools compete for 256 KiB.** You'd have a GC heap for the kernel's own objects and the arena for applets, and the split is fixed at build time. GC heaps also want headroom, since a conservative collector at 80% full collects constantly. In Rust no_std, the kernel's own state can be static or small and everything left over is arena. Your whole design is about not stranding memory, so a GC heap reserved for the kernel is the same problem again, just on the kernel side.
2. **The arena must stay out of the GC's sight.** If the arena is a big global, a conservative collector may scan it as a root, which wastes time and lets random applet bytes that look like pointers keep garbage alive. Check how your TinyGo version treats large `.bss` globals, and consider a dedicated linker section outside the scanned range.
3. **Nondeterministic pauses.** A stop-the-world mark-sweep over a small heap is probably milliseconds, not seconds, and the SX126x has its own FIFO and hardware-timed RX, so this hurts less than people fear. But it's still jitter you can't bound, and your fuel-metered, run-to-completion design is otherwise deterministic. Interrupt handlers also can't allocate.
4. **Goroutine stacks are fixed-size and heap-allocated.** Fine for a few kernel tasks, wasteful if you thought of one per applet. Your shared-stack-pool idea gets lost, though you wouldn't need goroutines for applets anyway, since the interpreter runs handlers to completion.
5. **Less control over layout.** Granule tables, slab pages, and region alignment are all things you want to place exactly. Rust gives you that directly, while in Go you're working around the runtime.

**What Rust gives you**

No runtime means the kernel's footprint is whatever you write, and an `alloc` crate with an embedded allocator (TLSF is popular) handles kernel dynamic structures if you want them. The ecosystem is `embassy`, `nrf-hal`, and `trouble` for BLE, and the interpreter's hot loop gets tighter codegen with fewer implicit checks. The cost is the borrow checker fighting you on exactly the kind of shared, aliased arena you're building, so you'll write `unsafe` around the memory subsystem and keep it small and well-tested.

**My read**

For a kernel whose entire value proposition is "give applets as much of 256 KiB as possible, deterministically," Rust fits better. TinyGo wins on development speed and on the prototype getting further faster, which you've already seen. If you wanted to hedge, the interpreter, region/granule logic, and loader could be written as portable, allocation-free code, and the surrounding kernel shell is what differs.

What did you use for guest memory in the TinyGo version, and where did the GC actually bite you (pauses, heap pressure, or something else)?

---

## Ben

ok let's stay with rust. here's what i'm not understanding: why do allocations used by applets need to be treated any differently than kernel allocations? why not just one big heap owned by the kernel and whoever wants an allocation can have one. doesn't rust do that already? i'm not really clear on who or where c and rust implement malloc/free and what code actually tracks those allocations but whatever it is, why would it need to care at all who uses it. the interpreter will intercept any instructions that access sram and do an easy/quick bounds check to make sure it actually owns that segment or that the segment is shared.

---

## Claude

You're right, and I overcomplicated it. A single heap owned by the kernel is the simplest design, and the allocator doesn't need to know who is asking.

**Who implements malloc/free**

In C, malloc lives in libc (newlib, picolibc, glibc). In Rust, `Box` and `Vec` call whatever is registered as the `#[global_allocator]`. On desktop that's the system malloc, and on a no_std chip you pick a crate (`embedded-alloc`, a TLSF implementation) and hand it a chunk of RAM. In both cases it's just code that manages a region of memory, tracking free blocks with a free list or tree, usually stored inside the free blocks themselves, plus a small header before each allocation recording its size. `free(p)` reads the header to learn the size. It has no idea who the caller is, and it doesn't need to. So yes, one TLSF heap serving both the kernel's own `Vec`s and applet `malloc` ecalls works, and fragmentation behavior is whatever that allocator gives you.

**What the allocator doesn't track**

Your interpreter's check asks: "does applet X own the byte range `[addr, addr+len)`?" Allocators can't answer that, for two reasons:

1. **They don't record who.** That's easy to fix by wrapping the allocator and recording an owner when the ecall allocates.
2. **They only look up by exact start pointer.** `free(p)` works because `p` is exactly what malloc returned. Your check sees arbitrary addresses, like `p + 37` inside a struct, and the allocator has no way to find which block contains that address without scanning.

So the one new thing you need is an ownership index, a structure that maps any address to its owner quickly. Everything else can stay a stock allocator. The options are the ones we discussed:

- **Sorted registry of `(start, len, owner)`.** Binary search, with the last hit cached. The check is exact, so it catches overflow into a neighboring block, even one of the same applet. This is probably the right place to start, since a repeater applet has few live blocks.
- **Granule table.** One owner byte per 32-64 B of heap. This gives an O(1) check and costs a fixed amount of RAM, and it's coarser, because allocations get rounded up to granule boundaries.

Kernel allocations are simply owned by `KERNEL`, which no applet ever matches. Kernel `Vec`s and applet blocks can interleave in the same heap, and an applet touching a kernel object traps because the registry says it isn't theirs. With the registry approach there's no alignment requirement. With the granule table, kernel and applet blocks must not share a granule, or ownership gets ambiguous.

**What the registry also buys you**

- **Unload and crash cleanup:** free everything where `owner == applet`.
- **Quotas:** a per-applet byte counter.
- **Sharing:** a block's entry carries an owner plus a short list of grants, or a flag marking it shared.

**Remaining caveats**

- **Allocator headers stay outside the checked range.** The block's range excludes the header before it, so the guest can't corrupt allocator metadata. Allocators that put headers there already satisfy this.
- **Fragmentation is now shared.** Applet churn and kernel allocations interleave, so a long-lived kernel allocation in the middle of the heap pins a hole. The mitigation is to make kernel allocation mostly static (allocate once at boot, rarely free), so the heap's dynamic part is mostly applet blocks. TLSF's bounded-time, low-fragmentation design helps here, and you can still use the scratch-arena idea on top if you want to reduce churn.

**The design**

One kernel-owned heap with a stock embedded allocator, plus a thin wrapper that records `(start, len, owner)` on each applet allocation and removes it on free. The interpreter checks every load and store against the registry. It's a much smaller change than my earlier layers, and it matches what you were describing.

Do you want to go through how the wrapper handles `realloc` and `free` from a guest (validating that the pointer is exactly a block the applet owns)? That's where a malicious applet could otherwise attack the allocator itself.

---

## Ben

> The mitigation is to make kernel allocation mostly static (allocate once at boot, rarely free), so the heap’s dynamic part is mostly applet blocks.

another mitegation could be voluntary gc or compacting, like a realloc()

---

## Claude

Compaction is a good fit for your design, with one catch: moving a block breaks every pointer to it. There are three ways to deal with that, and one is nearly free for you.

**1. Realloc-style moves (the weakest)**  
`realloc` can move a block because the caller promises to use only the returned pointer. A guest that stored the old pointer in a struct or a linked list now has dangling pointers. This works for flat buffers, but it can't be the general mechanism.

**2. Handles with lock/unlock (Palm OS and classic Mac style)**  
The applet holds an opaque handle and calls `mem_lock(h)` to get a raw pointer for the duration of a handler. The kernel compacts only between events. Your run-to-completion model gives you that safe point for free: the guest stack is empty, no registers are live, and no grants are outstanding. The cost is an applet-side discipline, since nothing can store raw pointers to movable blocks across events, and ported C code will break that rule constantly.

**3. Software address translation (my pick)**  
You're already doing a registry lookup on every access, so make it return a translation and not just a permission. Guest addresses become virtual, and each registry entry maps `virtual_start -> physical_start`. Compaction means memmove the block, update one registry entry, and every guest pointer stays valid. Applets never know anything moved.

A few things fall out of this:

- **Virtual addresses can be sparse and never reused.** Your 32-bit guest space is far bigger than 256 KiB of RAM, so allocate virtual ranges with a bump counter. That gives you free use-after-free detection, since a stale pointer hits no registry entry and traps. It also removes the stale-pointer-hits-a-new-block problem I raised with grants.
- **Shared blocks work unchanged.** Every applet sees the same virtual address for a shared block, so zero-copy reducer chains still pass pointers as-is. Only the kernel's own view differs, and host functions already have to validate pointers, so translating is one more step in the same place.
- **The hot path barely changes.** The cached last-hit region now caches `(virt_start, virt_end, delta)`, and the access becomes `phys = addr + delta`. The granule table doesn't work here, so this commits you to the registry.

**What stays pinned**

- **Kernel Rust allocations.** Rust references and `Box` can't be moved. Use a two-ended layout, with kernel allocations growing from one end of the heap and movable applet blocks from the other, so pinned kernel objects never fragment the compactable area.
- **DMA and radio buffers.** Anything the hardware is reading or writing must be pinned or skipped.
- **Blocks with live grants.** Compact only when no handler is running.

**Cost and scheduling**  
Moving 100 KiB with a memmove on a 64 MHz Cortex-M4 takes on the order of a millisecond or two, which is fine between events. You can compact incrementally, for instance sliding a few blocks per idle tick, or trigger it only when an allocation fails and the free total would have sufficed. That "fail, compact, retry" path keeps the common case free.

**What about real GC?**  
Compaction solves fragmentation, but it won't find leaks. A guest has no type information, so you can't precisely find its pointers. A conservative scan could identify unreachable blocks, but you couldn't move them (with translation you don't need to). I'd skip GC and rely on per-applet quotas and free-on-unload, which bound the damage of a leaking applet without any scanning.

The main thing this costs you is that the guest address no longer equals the physical address on the nRF52. That gives up the "no translation at all" property I mentioned earlier, in exchange for no fragmentation. Do you expect fragmentation to be the real problem on a repeater, or are the applets small and static enough that the two-ended heap alone would be sufficient?

---

## Ben

yes i was going to take it there with you too, virtual addresses, but the only issue is that a shared scratchpad space now uses a different address space. how would you manage that

---

## Claude

The answer is to make the arena a single global virtual address space. Virtual addresses are never per-applet, so the scratchpad has exactly one address that every applet and the kernel agree on. That's the property I was relying on when I said shared blocks work unchanged.

**One address space, per-applet permissions**

The registry is global, and each entry looks like `(virt_start, len, phys_start, owner, grants)`. An access from applet X to address `a` succeeds if the entry containing `a` is owned by X, or if X appears in its grants (or the block is marked world-readable). Applet isolation is a permission check on a shared map, not separate maps. So:

- `smalloc` returns virtual address `V`.
- `smap(name)` from another applet returns the same `V` and adds a grant to the entry. It doesn't create a second mapping.
- A pointer to the scratchpad can sit in a packet, a hook argument, or another applet's data structure and still mean the same thing everywhere.

The only per-applet piece is the hot-path cache. Each applet's cached `(virt_start, virt_end, delta, perms)` has to be invalidated when a grant is revoked or a block is compacted. Since both happen between events, you can just clear the cache at those points.

**The kernel's view**

The kernel isn't in the guest address space, so it translates at the boundary. Host functions take guest pointers, validate them against the registry like any access, and get a physical slice. The rule is that native code never holds a physical pointer across an event boundary, because compaction can happen there. If the kernel needs to keep a scratchpad (say, the radio applet's neighbor table that kernel code also reads), it stores the virtual address or a registry handle and re-resolves each time. If it truly needs a stable physical address (DMA, radio FIFO buffers), it marks the block pinned, and compaction skips it. A pinned block still has a virtual address and a fixed delta.

**Where separate spaces still make sense**

Statics are the exception. Earlier I proposed linking every applet at the same fixed addresses, which means `0x1000_0000` means a different thing in each applet. Those windows can be per-applet private virtual ranges, with the registry tagged by applet. The arena above them is global. The SDK rule stays the same: never pass a pointer to a global across an applet boundary, only pointers from `malloc`, `smalloc`, or the stack arguments the kernel hands you. If you'd rather drop that rule, load statics into the arena as ordinary blocks and relocate `.data` pointers at load.

**A problem I glossed over**

"Never reuse virtual addresses" doesn't survive contact with a repeater that runs for months. A 29-bit arena window is 512 MiB, and a node allocating a few packet buffers per second will wrap it eventually, and sooner than you'd expect once churn picks up. Two ways out:

1. Recycle virtual ranges after a quarantine (not reused until N other allocations have happened), which keeps use-after-free detection probabilistic but strong.
2. Put a small generation number in the top bits of the virtual address (e.g. 8 bits), bump it when a range is reused, and have the registry compare it. Stale pointers then fail deterministically for 255 reuses of the same slot.

Option 2 is cheap and gives you the lifetime enforcement for lent packet buffers that I mentioned back when we first discussed grants.

If you're asking whether the scratchpad should be visible at a *different* address to different applets (for instance, to let each applet link its scratchpad at a fixed address it expects), I'd say no. Fixed per-applet placement means pointers can't be passed between applets, which defeats the purpose of a zero-copy scratchpad. Do you have a case in mind where you wanted per-applet placement?

---

## Ben

oh, right right i see now. okay yeah that is good. i like that approach. so we'd want to fuzz that memory manager pretty hard since it will be in the kernel and harder to update

---

## Claude

Yes, and the design helps: the memory manager is pure logic over a byte array, so it can be fuzzed on a desktop at millions of ops per second with no hardware involved.

**Structure it for fuzzing from day one**

Put the registry, allocator wrapper, compaction, and access check in a `no_std` crate with no HAL dependencies. Physical memory is a `&mut [u8]` handed in at construction. On the chip it's the SRAM region, and under test it's a `Vec<u8>`. Make the constants (virtual window size, generation bits, granule size, heap size) parameters, so tests can shrink them and hit wraparound in seconds instead of months.

**Model-based fuzzing**

Use `cargo-fuzz` with the `arbitrary` crate to generate random sequences of operations: alloc, free, realloc, smalloc, smap, grant, revoke, compact, load, store, unload applet. Run each sequence against the real implementation and a deliberately dumb reference model (a `BTreeMap` of blocks and a plain `Vec<u8>` per block). After every op, compare outcomes: both allow the access or both trap, and loaded values match. Differential testing like this catches the subtle bugs.

**Invariants to assert after each op**

- No two registry entries overlap in virtual or physical space, and the registry stays sorted.
- Every entry lies inside the heap, and none overlaps allocator metadata or kernel allocations.
- Per-applet quota counters equal the sum of their entries.
- Contents survive compaction. Write a known pattern keyed by virtual address, compact, and verify.
- Poison bytes outside all owned ranges are unchanged, so any stray write is caught. Fill the rest of the heap with a canary pattern.
- A stale pointer (freed, or wrong generation) always traps.
- The cached fast path and the uncached slow path give identical results. Run both and compare, since stale-cache bugs after compaction or revocation are the likeliest failure.

**Hostile-input cases to seed directly**

The checker takes `(addr, len)` from untrusted code, so hammer it with `addr + len` overflowing u32, `len = 0`, `len = u32::MAX`, accesses straddling two adjacent blocks, and addresses just before and after a block. Also fuzz the ecall surface with double free, freeing an interior pointer, freeing someone else's block, realloc on a shared block, smap without a grant, and revoke while a handler holds the pointer.

**Target-specific gotchas**

- **Build the fuzz target as 32-bit.** On a 64-bit host `usize` is 64 bits, so overflow bugs that exist on the Cortex-M can't occur. Fuzz with `i686` or keep all arithmetic in explicit `u32` and test it.
- **Run Miri** over the unsafe parts (the physical-slice translation and the memmove in compaction) to catch aliasing and out-of-bounds problems that a fuzzer sees only as silent corruption.
- **Try Kani** for the access check itself. It's a few lines of pure arithmetic, which suits bounded model checking, and you can prove "never returns a physical range outside the registered block" rather than just failing to find a counterexample.
- **Add a few on-target runs.** Replay recorded fuzz corpora on real hardware or Renode/QEMU to confirm nothing differs from the host, such as alignment or stack depth.

**Making "hard to update" less scary**

Keep the immutable part as small as possible: the registry, access check, allocator, and loader. Anything policy-like, such as when to compact, quota defaults, and eviction, can live where it's easier to change. Combined with your A/B kernel slots and watchdog rollback, a bug found later is a recoverable truck roll instead of a brick. Save the fuzz corpus and seeds in the repo, so every fixed bug becomes a permanent regression test.

Do you plan to fuzz whole guest programs through the interpreter too, or just the memory manager's API directly? Whole-program fuzzing finds interpreter and checker integration bugs, but it needs a decent corpus of valid RV32 code to get past the decoder.

---

## Ben

i think a good test would be 2 lora radios (wismesh tags), or maybe 4, all playing some kind of lora pingpong game with each other all night long, all outputting over usb uart to the terminal so we can monitor. so the kernel would be responsible for raw rust packets and then the applet would be responsible for players joining and leaving the frisbee game. i guess frisbee is a better metaophor. tossing the frisbee to each other and listening for an ack and resending or throwing to another player when it fails. something like that

---

## Claude

That's a good soak test because it exercises almost every subsystem at once, and it fails loudly. A game with a verifiable invariant tells you something is wrong without needing to know where to look.

**Why it fits**

- It's a real distributed protocol with timers, retries, and membership churn, but tiny enough to write in a day.
- The WisMesh Tag is an nRF52840 with an SX1262, so it has the same 256 KiB SRAM and 1 MB flash as your repeaters. Memory behavior on the bench should transfer directly.
- Four nodes in one room will collide often, which gives you the packet loss you want without driving anywhere.

**Kernel/applet split**

The kernel owns the radio state machine (CAD, RX windows, airtime accounting), raw packet RX/TX, timers, and the UART log sink. The game applet gets an `on_rx(ptr, len)` event and an `on_timer(id)` event, and calls `radio_send` and `log`. Packet buffers arrive as lent pool slots, so the zero-copy path is tested on every packet.

**Game protocol sketch**

- **Frisbee packet:** `game_id, epoch, seq, thrower, target, payload`. The payload can be a counter plus a hash of the previous throw, so any corruption, duplication, or reordering is detectable by any node that sees two consecutive throws.
- **Ack:** the catcher can send an explicit ack, or you can use passive ack, where hearing the catcher's next throw counts as proof of delivery. Passive ack is how mesh flooding works, so it's worth testing. Start with explicit acks for simplicity.
- **Retry:** wait with jittered backoff, resend up to N times, then mark the target suspect and throw to someone else.
- **Join/leave:** HELLO broadcasts on a slow period, roster entries expire after a few missed periods, and an explicit BYE is sent on clean unload.
- **Lost frisbee:** if the holder dies mid-throw, nobody has it. If no throw is seen for T, each node starts a randomized timer, and the first to fire serves a new frisbee with `epoch+1`. If two serve at once, the higher epoch (ties broken by node ID) wins and the other drops out. That's a small leader-election problem and a good test of state handling, and it will happen constantly.

**Make it test the kernel, not just the game**

- **Roster as a scratchpad:** the game applet owns it via `smalloc`, and a separate stats applet maps it read-only and prints the roster and counters to UART every few seconds. That exercises sharing, grants, and a second applet, with no extra game logic.
- **Chaos applet:** a third applet that periodically does bad things. It reads out of bounds, double-frees, leaks until it hits quota, and spins until fuel runs out. The expected result is that it traps and the kernel unloads it, reclaims its memory, and the game keeps going. Run it at random intervals all night.
- **Forced churn:** reload the game applet on one node every few minutes, which tests unload cleanup, reloading, and compaction under real traffic. Then OTA-update the applet mid-game over LoRa or USB.
- **Hook replacement:** swap a system applet that replaces a host function (say a logging filter or a packet-drop filter that simulates 30% loss) while the game runs.

**What to log and assert**

Have every node print a one-line summary periodically: uptime, throws sent and received, acks, retries, epoch changes, traps, free heap, largest free block, and number of registry entries. Capture it to files with timestamps, one per node.

Then pass criteria for an overnight run:

- Free heap and largest free block are flat over time. A downward slope means a leak or fragmentation, and that's the number you most want to watch.
- Zero hash-chain violations.
- The game never stays dead for more than a few T intervals.
- Every chaos trap is followed by a clean recovery.
- No watchdog resets, or if there are some, each is explained.

**Practical gotchas**

- **Airtime and duty cycle:** a pingpong at a fast SF can be allowed by regulations and still saturate the channel with four players. Use a slow throw rate (a few seconds between throws) and a legal frequency and power for your region, since this will transmit all night.
- **Timestamps:** the nodes have no shared clock, so log with local uptime and correlate by sequence numbers. You'll also find out whether your timers drift.
- **USB serial:** the nRF52840's native USB CDC works without a separate UART chip, but check how the tag exposes it. If you log from the kernel, keep UART writes non-blocking so a disconnected terminal can't stall the radio.
- **Record a replayable trace:** log the sequence of RX events per node so you can feed it into the host build and reproduce a failure under a debugger. It also doubles as a fuzz corpus for the interpreter.

Do you want the game applet written in Rust or C for the first run? That decides which toolchain path (the riscv32e target versus clang) has to be working before the first throw.

---

## Ben

let's write the applet in rust as well as the kernel

---

## Claude

Rust applets are the cleaner choice anyway, because the applet's protocol logic can be a plain `no_std` library that compiles natively too. You can unit-test the frisbee game on your desktop with a mock host before it ever runs in the VM.

**Target choice: skip the E extension for now**

Rust does have `riscv32e*-unknown-none-elf` targets, but they're tier 3, so you'd be on nightly with `-Zbuild-std`, and the LLVM ilp32e ABI support has been the shakiest part. I'd check current status before depending on it. The practical alternative is `riscv32imc-unknown-none-elf`, which is stable, tier 2, and ships with a prebuilt core. The E extension saves silicon in real hardware by halving the register file, but your interpreter just has an array of registers, so 32 instead of 16 costs 64 extra bytes. E also makes the compiler spill more, so I may actually be denser. Make the interpreter implement RV32IMC and revisit E only if you find a reason.

**Applet crate layout**

- `mesh-abi`: a shared crate used by both kernel and applets. It holds ecall IDs, event signatures, packet structs, and error codes. Putting this in one place keeps the two sides from drifting.
- `frisbee-core`: `no_std`, no `unsafe`, pure game logic over a `trait Host { fn send(..); fn log(..); fn now(..); }`. This is what you test natively.
- `frisbee-applet`: the thin `no_main` crate that implements `Host` with real ecalls and exports `on_rx`, `on_timer`, and `on_load`.

**Rust-specific gotchas**

1. **Host imports without a relocation fight.** Declaring `extern "C"` functions leaves undefined symbols that the linker rejects or silently resolves to zero. Instead, have a macro generate an inline-asm `ecall` wrapper whose call number is a compile-time hash of the function's name (FNV-1a of `"radio_send"`, say), and also emit the name into a custom `.mesh.imports` section. The packer reads that section to build the import table, and the kernel resolves hash to implementation at load. Because dispatch is by hash through a table, system applets can override entries and the kernel can detect collisions at registration.
2. **Exports.** `#[no_mangle] pub extern "C" fn on_rx(ptr: u32, len: u32)` shows up in the ELF symbol table, and the packer turns these into the hook table. The manifest can validate the names against what the kernel knows.
3. **Panics.** Use `panic = "abort"` and a panic handler that issues `ebreak` (or an `ecall` that reports a trap), so a panic is a clean trap and the kernel unloads the applet.
4. **Code size.** Avoid `core::fmt`, which costs several KiB. For logging, send a format ID plus raw integers (defmt-style) and let the host format it, or write a tiny hex/decimal writer. Build with `opt-level = "z"`, `lto = "fat"`, `codegen-units = 1`, and `strip`. Watch for panicking bounds checks and `unwrap`, which pull in message strings.
5. **Allocation.** Applet code can stay allocation-free by using fixed arrays and the scratch arena, which suits a `no_std` game. If you want `alloc`, implement `GlobalAlloc` with ecalls to the kernel's `malloc`/`free`, so `Vec` and `Box` work. Note that `Vec` growth calls `realloc`, and every growth is an ecall.
6. **Linker script.** Map `.text` and `.rodata` at the code window, `.data` and `.bss` at the per-applet statics window, and set `sp` from the kernel. Use `-C relocation-model=static`, `-C link-arg=-T...`, and keep alignment at 4 so the interpreter's single-region check holds. Rust only emits misaligned accesses through `packed` structs and `read_unaligned`, so ban those in the SDK and parse packets with explicit byte-wise `from_le_bytes`.
7. **Packet parsing.** Treat `(ptr, len)` from `on_rx` as untrusted. Copy the frame into a local struct with `from_le_bytes` instead of casting the pointer, which avoids both alignment issues and checker traps from reading past `len`.

**Build order I'd suggest**

1. `mesh-abi` plus `frisbee-core` with a mock host, and run a four-player simulation entirely on the desktop with simulated loss. This proves the protocol logic before any VM exists.
2. The interpreter, running a hello-world applet that calls `log` through an ecall, on the desktop.
3. The memory manager from the previous discussion wired into the interpreter, with fuzzing.
4. Port the kernel to the nRF52840 with the radio, and run the game over the air.
5. Add the stats and chaos applets.

Step 1 gives you a protocol you trust, so when step 4 misbehaves you know it's the platform and not the game.

One decision affects the log format: do you want the applet to emit structured binary log events (compact, with the desktop tool formatting them) or human-readable text lines straight to the terminal? Binary is much smaller on the applet side, but text is easier to debug in the first week.

---

## Ben

honestly writing an applet in rust sounds like not something a regular dev could do easily. it sounds like there are a lot of tricky low-level decisions. that's what i liked about wasm, it was clean and standard and everyone can compile to wasm and then the cross compiler changed it to the kind of code we needed. am i wrong? i mean 7 steps just to get a right risc target output seems like a lot is going on

---

## Claude

You're not wrong. That list of seven items made it sound like what a developer has to do, when most of it is what the SDK author does once. A dev would run something like `mesh new` and `mesh build`, and the linker script, panic handler, and import wiring would sit in a template and a crate. Nobody thinks about linker scripts when they flash an Arduino sketch. But your underlying point holds, and I think I overweighted the wrong thing.

**Where wasm is better for your goals**

- **Reach.** You said "any high level language." Wasm targets include C, C++, Rust, Zig, AssemblyScript, TinyGo, and others, all through standard, stable toolchains. The RISC-V route covers C, C++, Rust, and Zig, and most other languages don't have a usable riscv32 bare-metal story.
- **Name-based imports are built in.** A wasm module imports `("mesh", "radio_send")` by string. That's exactly the linking model you want for system applets overriding host functions, and it's cleaner than the FNV-hash ecall scheme I invented.
- **No linker script, no PIC question, no custom packer.** The module is already a self-describing, validated container.
- **Structured control flow.** You can prove at install time that jumps stay in bounds, so the interpreter doesn't need PC checks.

I steered toward RISC-V because the interpreter is simpler, which optimizes for the platform author at the expense of the applet developer. For a platform whose point is that other people ship applets, that tradeoff is backwards.

**The memory model doesn't need to be the dealbreaker**

Wasm's linear memory is really a convention of the engine. A `load` is just "read at an i32 address plus an offset," and nothing requires a contiguous reserved buffer behind it. Your translator can lower every load and store into a checked access against your global virtual space. The pieces that need handling:

- **Data segments and the shadow stack** are placed at fixed addresses by `wasm-ld`. Treat the low range as the applet's private statics and stack, allocated at load, and the high range as the global arena, with the checker routing on the top bits. `--global-base` and `--stack-size` control the layout.
- **`memory.grow`** maps to a kernel allocation, or is banned in favor of the SDK's `malloc` import. Code that links dlmalloc and expects contiguous growth is the awkward case, so the SDK should provide an allocator that calls the kernel.
- **`--import-memory`** tells the toolchain the memory is the host's, and the module won't assume it owns one.

You already built wasm-to-custom-bytecode successfully, so the open problem was the memory semantics, and I think the translator plus the global-registry design solves that. It's real work, but it lives in your VM, written once, and not in every developer's build.

**One catch: signing**

If a desktop tool translates wasm to your internal bytecode, the developer's signature covers the wasm, not what the device runs. You can have the device translate on install (streaming, so it doesn't need to hold both forms), or have the user's desktop app re-sign with the owner's key. The second is reasonable for a fleet you own, but it's a different trust model from "run only what the developer signed."

My revised recommendation is wasm as the distribution format and your own compact bytecode as the internal one. I'd still use Rust for `frisbee-core`, since it's your first applet, but compile it to wasm, and the same game logic then works with the standard target.

Which part of the wasm memory model bit you hardest in your earlier attempts: data segments, `memory.grow`, or the in-module allocator?

---

## Ben

but doesn't wasm allocate its own stack in its own guest heap? it seemed pretty tied to that, and efficient shared-arena and shared-guest-stack memory allocation were our top goals and sticking points

---

## Claude

Partly right, and I glossed over this last time. Wasm has two stacks, and only one is tied to linear memory.

**The two stacks**

1. **The wasm call and operand stack** holds locals, temporaries, and return addresses. It's engine-owned and invisible to the module, since there's no way to take its address. In your interpreter this is just your own value and frame stacks, so a single shared one across all applets is natural.
2. **The shadow stack** lives in linear memory and holds anything whose address is taken: arrays, structs passed by reference, large spills. This is the one you remember. But it's an LLVM codegen convention, not part of the wasm spec. The compiler keeps its top in a mutable global, `__stack_pointer`, and bumps it in function prologues. I was wrong earlier to call linear memory "a convention of the engine." The shadow stack is a convention of the compiler, which is why it's the stubborn part.

**Why that's still workable**

Because the stack pointer is just a global, you can own it. In the dynamic-linking convention (`-fPIC`/shared), the module *imports* `__stack_pointer`, `__memory_base`, and `__table_base` from the host. Otherwise the translator can find the stack-pointer global (LLVM makes it global 0, or you can export it) and ignore its baked-in initial value. Before each handler runs, the kernel sets it to the top of a slice from the shared stack pool. That's the same lending mechanism as packet buffers: grant the slice, run the handler, revoke, zero the used portion. Overflow needs no guard page. Any access below the slice isn't in the registry for that applet, so it traps. Many small handlers never touch the shadow stack at all, so the pool can be quite small.

The heap isn't built into wasm either. Rust's dlmalloc and wasi-libc's malloc are library code that calls `memory.grow`. You replace the allocator with one that imports your kernel `malloc`/`free`, and make `memory.grow` trap or return -1.

**What's still hard**

- **Baked-in absolute addresses.** `wasm-ld` places statics at fixed offsets (`--global-base`), and the code contains those as `i32.const`. So either you give each applet a private virtual window for statics, as we discussed with RISC-V, or you use PIC mode, where every static access adds `__memory_base`. PIC costs code size and an add per access, and wasm32 PIC support in Rust is rough. C and C++ are better supported.
- **Data segments and bulk memory.** Active data segments get copied at load into your allocated statics block. `memory.copy` and `memory.fill` become checked bulk operations, which is nice.
- **The declared memory size.** You ignore it, since memory 0 is just a view onto your virtual space.

**The honest tradeoff**

On RISC-V, the stack question is trivial because `sp` is a register you set. With wasm it works only through the LLVM convention, so "any language" is really "any language that uses LLVM's shadow-stack convention": C, C++, Rust, and Zig. Languages with their own runtimes (Go, AssemblyScript, anything garbage-collected) bring their own stack and allocator ideas, and will fight your model much harder. You'd likely have to support those case by case, or not at all.

Wasm's real advantage is that the compile step for C, C++, and Rust is standard and stable, and name-based imports are built in. The cost is a translator that has to understand these conventions. RISC-V gives you a much simpler VM but an odder toolchain target for developers, whereas wasm puts the complexity on you once.

You can test this cheaply. Compile a small Rust and a small C applet to wasm with `--import-memory`, dump them with `wasm-objdump -x`, and look at the globals section, the data segments, and where `__stack_pointer` is initialized. If those look the same across both toolchains, your translator rules are sound.

When your earlier attempts hit the stack problem, was it that the stack pointer was baked in as a constant, or something else, like the stack and data being laid out together?

---

## Ben

ok, another approach to this is to invent our own scripting language. tbh i'd probably be running micropython if it had a xip feature - and just to check...it doesn't, right?

---

## Claude

*[Searched the web]*

It does now. MicroPython 1.25 (April 2025) added ROMFS, a read-only, memory-mappable filesystem that lets bytecode run directly from flash. It builds on a 2022 rework of the .mpy format, whose stated goal was a middle ground between loading .mpy files into RAM and freezing them into firmware. Frozen modules always ran in place, but they require recompiling and reflashing the whole firmware. ROMFS is the version you can update without a reflash, and `mpremote romfs deploy` pushes an image to the device. ([MicroPython v1.25.0 Released with ROMFS, RISC-V Assembler, and Expanded Board Support - LinuxGizmos.com](https://linuxgizmos.com/micropython-v1-25-0-released-with-romfs-risc-v-assembler-and-expanded-board-support/) +2)

**nRF support is murky.** The current docs' port list names alif, esp32, esp8266, rp2, samd, and stm32, but not nrf. A porting wiki page marks NRF as missing, while a downstream fork's docs list nrf as supported. I'd assume you'd be doing some porting, since ROMFS needs a flash partition and a port-specific ioctl implementation. Check master before betting on it. ([docs.micropython.org](https://docs.micropython.org/en/latest/reference/romfs.html) +2)

**What XIP doesn't fix.** It keeps bytecode and constants in flash. Objects, globals, and call frames still live in MicroPython's garbage-collected heap, which is a fixed region reserved at boot. That brings back the guest-memory problem: one GC heap carved out of 256 KiB (minus whatever the BLE stack takes), shared by every script. As far as I know it also runs as a single interpreter with global state. There are no per-applet regions, quotas, or fuel limits, and no zero-copy lending, so a leaky or runaway script hurts everyone. That makes it a good scripting layer for one app but a poor fit for a multi-applet platform with isolation.

**What's worth borrowing.** The format change that made XIP possible has bytecode refer to names through a small per-module table. The code stays immutable, and only a small table needs linking at load time. That is the same shape as your name-based imports. The applet's code sits in flash and never changes. At load, the kernel resolves each imported name into a slot in a per-applet table, and a system applet overriding a host function just changes what a slot points to. You'd get XIP, overrides, and no relocation, whether the bytecode is wasm, RV32, or your own. ([randommeaninglesscharacters](https://git.randommeaninglesscharacters.com/W3SLAV/micropython/src/commit/f2040bfc7ee033e48acef9f289790f3b4e6b74e5/py/mpconfig.h))

**Your own language.** It makes sense if the language removes the need for your memory model. If it has no raw pointers, only typed buffer and handle values that the VM bounds-checks, you don't need a registry lookup on every load and store. A shared scratchpad becomes a buffer passed by reference. A language shaped around packet handlers, with byte buffers, fixed-width ints, and events as first-class, could be tiny. The cost is everything wasm gave you for free: existing compilers, "any language," and tooling. You'd own the compiler, debugger, docs, and error messages, and those are most of what makes a platform adoptable.

What would devs mostly be writing on your platform: small packet handlers like the frisbee game, or broader app logic like UIs and storage? That decides how much language you actually need.

---

## Ben

imagine someon writing a full fleged BBS that pulls weather stats via wifi on a timer, manages user accounts and forums, shares files, etc. micro versions obviously, but full applications. the bbs might be split into several applets that implement various subsystems or it may rely on other subsystems made by others. like maybe someone makes an applet that implements an on-flash database and the bbs uses it

---

## Claude

That scenario changes some of my earlier answers.

**It settles the language question.** A BBS needs strings, collections, parsing, and a real standard library, so a packet-handler scripting language wouldn't be enough. You'd be building a general-purpose language and its tooling. That favors wasm or RV32 with existing compilers, and an allocator that calls into the kernel for `Vec`, `String`, and `malloc`.

**Applets calling applets.** The mechanism is mostly in place already. A DB applet exports `db.v1.get` and `db.v1.put`, and the BBS imports them by name. Because the kernel resolves imports at load time, the DB applet is just another provider in the host namespace. Putting the interface version in the name means a breaking change is a new name, and two applets can coexist during a migration.

The "handlers run to completion and never nest" simplification stops holding once the BBS calls the DB synchronously in the middle of a handler. Here is what I'd do instead:

- **Nesting:** push a new stack-pool slice for the callee, with a small kernel-enforced depth limit.
- **Fuel:** charge the callee's time to the caller's budget, so a slow DB call can't be used to dodge limits.
- **Arguments:** lend pointer arguments for the duration of the call, with the buffers caller-allocated by default. That needs a small typed signature in each export (`ptr,len` pairs), or the kernel can't tell pointers from integers.
- **Identity:** the kernel keeps the call stack of applet IDs, so the callee can ask `caller()` and get an unforgeable answer. That's how a DB applet enforces per-caller namespaces without trusting anything in the arguments.

**Blocking I/O.** A weather fetch takes seconds, and a run-to-completion handler can't wait. You have two options:

- **Callbacks:** the applet starts the request and handles `on_net_event` later. This costs no RAM while waiting but is awkward in C. Rust's `async` compiles to exactly this kind of state machine, and a tiny executor driven by kernel events would hide it.
- **Stackful suspension:** this reads like normal blocking code, but every suspended task pins its own stack slice. That brings back the per-applet stack cost your shared pool was meant to avoid.

I'd go with callbacks and let language runtimes build ergonomics on top.

**Dependencies and packaging.** The manifest gets `provides` and `requires` (name, version range, optionally a content hash). Keep dependency resolution on the desktop or mobile app, so the device only verifies that its imports are satisfied. If a dependency is unloaded or fails, its slots can point to a stub that returns `ERR_UNAVAILABLE` instead of crashing the dependents. Optional dependencies fall out of this too. The same BBS can run on a node with no wifi, just without the weather feature. Wifi probably means an ESP32, RP2350, or desktop host anyway, so the capability manifest is how one applet runs across very different hardware. Pinning by hash matters here, since a compromised DB update would otherwise hit every dependent.

**Storage.** The DB applet needs a flash capability, so the kernel should hand out partitions or a filesystem with per-applet quotas. Two hazards are specific to your deployment. On a solar node, power loss mid-write is routine, so either the kernel offers an atomic primitive or the DB applet is expected to journal. And on nRF52, flash erases stall the CPU for a noticeable time, so they have to be scheduled around radio activity.

**Memory pressure.** A BBS makes hundreds of small allocations, and one registry entry per allocation gets expensive. The registry could instead grow a single heap block per applet and let the applet manage it internally. Combined with virtual addresses and compaction, the kernel can resize and move that block, so slack stays small. It's the chunk idea you rejected, but without the stranded memory. Fine-grained entries would then be needed only for shared or lent blocks.

Should cross-applet calls look like function calls or like messages? I'd lean toward synchronous calls for fast operations like DB lookups and events for anything slow, but the choice shapes the whole SDK.

---

## Ben

synchronous calls for sure.

another part of the smoke test would be for my macos laptop to drive a dumb lora modem interaction too. so say a 5th tag running the microkernel, but this one is a lora modem over usb, and then the desktop app runs a different module that knows how to communicate via usb to the lora modem. some kind of transport driver architecture

---

## Claude

Synchronous it is. One consequence for this design is that a host function must never block on a transport. `radio.send` queues the frame and returns, and the result arrives later as a `tx_done` event. Otherwise a USB hiccup stalls the interpreter.

This is the best demo of your pitch so far. The frisbee applet binary should run unchanged on the nRF52 tags and on the Mac, with the only difference being which provider answers `radio.*`.

**Four layers, each swappable**

1. **Transport:** a raw byte stream. On the Mac that's a serial port, and on the modem tag it's USB CDC. Later it could be BLE UART, TCP to a Pi, or anything else.
2. **Framing:** COBS with a 0x00 delimiter plus a CRC, so a byte stream becomes discrete messages and corruption or a mid-frame reconnect is detected. Reticulum's RNode firmware uses KISS-style framing, so there's precedent for something this simple.
3. **Modem protocol:** typed messages, covered below.
4. **Radio provider:** the host-namespace functions (`radio.v1.send`, `radio.v1.configure`) and events (`on_rx`, `on_tx_done`) that applets import.

Layer 1 is native on both ends, since it needs OS or USB APIs, and the kernel exposes it as a generic `stream` capability. Layers 2 to 4 can be applets, built from a shared `no_std` `modem-proto` crate. On the tag, a modem applet reads frames and drives the real radio. On the Mac, a system applet provides `radio.v1.*` by encoding calls into frames. Both ends use the same framing code, so a protocol bug can't be one-sided. If the Mac side is faster to write as native Rust at first, that works too, since it implements the same provider interface.

**What must stay on the modem**

USB latency and macOS scheduling jitter are milliseconds, which is small against LoRa airtime but large against CAD and RX-window timing. So the modem's kernel owns CAD and listen-before-talk, TX scheduling, airtime and duty-cycle enforcement, and RX timestamps (microseconds from its own clock). The host says "send this, with this LBT policy," not "key the transmitter now." Enforcing regulatory limits on the modem also means a buggy desktop app can't violate them.

**Messages to start with**

- `HELLO`: the modem announces its version, capabilities, and radio hardware. The host replies with `CONFIG` (frequency, SF, BW, CR, power, sync word, preamble). Because the modem sends `HELLO` after every boot, a reset mid-game triggers reconfiguration automatically.
- `TX{id, payload, policy}` and `TX_DONE{id, status, airtime}`. Hand out TX credits so the host can't overrun the modem's queue.
- `RX{timestamp, rssi, snr, payload}`. The modem keeps a small ring buffer if the host is slow, and reports a drop counter.
- `PING` and `PONG` carrying timestamps, for round-trip latency and clock correlation.
- `STATS`: free heap, queue depths, error counts.

I'd put a radio index in `radio.v1` calls from the start, since a host with two modems on different frequencies is easy to design in now and annoying to retrofit.

**macOS specifics**

- Open `/dev/cu.usbmodem*`, not `/dev/tty.*`, because the `tty` nodes can block waiting for carrier detect.
- Assert DTR on open, because some CDC implementations hold output until the host raises it.
- The device node vanishes and reappears when the tag resets or enters DFU, so the transport needs reconnect logic. Rescan on a timer or use IOKit notifications.
- The `serialport` crate covers enumeration and I/O.

**Tests this adds**

- **Cable pull mid-game:** the stub from the dependency design returns `ERR_UNAVAILABLE`, so the game applet should degrade instead of crash. Then replug and check that `HELLO` and `CONFIG` restore the player to the game.
- **Modem reset and laptop sleep:** run both mid-game and check for recovery with no manual steps.
- **Flood the host link:** this exercises credits, ring-buffer overflow, and the drop counters.
- **Latency histogram:** log `PING` round trips and per-packet RX skew. If the modem-attached player's retry timeouts behave differently from the native players', that's the transport showing through.
- **Applet identity:** print the applet binary's hash on all five nodes at startup, so the log proves the same artifact ran on nRF and macOS.

The Mac side is a good place to use the same `Memory` trait and fuzzing setup, since it gives you a second host build of the memory manager running real traffic, in addition to the nRF target.

Should the Mac's desktop kernel be a standalone binary that hosts applets, or a library embedded in a larger desktop app? That affects how the transport and UI hooks are exposed.

---

## Ben

as a matter of fact, i think we could even write an sx1262 simulator, is there some way in darwin to have a shared pipe that they can all connect to to send and receive messages as if they came over the air?

---

## Claude

Yes. On macOS the options are Unix domain sockets, localhost TCP, or no IPC at all. A shared pipe isn't enough, though. A plain broadcast bus delivers every frame to everyone instantly and perfectly, which is nothing like LoRa. The thing to build is a medium model, and the transport is the easy part.

**Transport options**

- **Single process (simplest):** if every simulated node is the same Rust binary, run each node as a task or thread and give them a shared `Medium` object, with no IPC. It's the easiest to debug, since the whole thing runs under one debugger. It's also the easiest to make deterministic and replayable.
- **Hub process over sockets:** needed if you want separate binaries or want to mix in the real modem tag. Each node connects to the hub, and the hub plays the air. I'd use localhost TCP with `TCP_NODELAY`, since it's portable to Linux and Windows. Unix domain sockets also work on macOS, but paths are limited to about 104 bytes, so keep the socket path short. Use `SOCK_STREAM` with the same COBS+CRC framing as the modem protocol. A simulated node then looks like a modem on a pipe, and the same transport layer runs everywhere.
- **Avoid:** named pipes (FIFOs), which have no broadcast semantics, and UDP multicast on loopback, which works on macOS but is fiddly.

The hub can also bridge to the real modem tag over USB. Real RF from the tag gets injected into the simulated medium, and simulated transmissions get re-sent out of the real radio, so one game can span simulated and physical nodes.

**What the medium has to model**

- **Airtime:** compute time-on-air from the Semtech formula (SF, BW, CR, preamble, payload length, header/CRC flags). A frame is delivered at the end of its airtime, not instantly.
- **Half-duplex:** a node that's transmitting hears nothing.
- **Match conditions:** RX succeeds only when frequency, SF, BW, and sync word match. Different spreading factors are mostly orthogonal.
- **Collisions with capture:** overlapping frames on the same channel usually destroy each other, but a sufficiently stronger one (roughly 6 dB) survives, and LoRa does show that capture effect.
- **Link budget:** use a configurable N×N loss matrix instead of positions. It gives you RSSI and SNR, plus hidden-node topologies, such as A hearing B, B hearing C, and A not hearing C. That's the exact situation your repeaters exist for, so you can test the repeater applet at 50 nodes without driving anywhere.
- **Marginal-link errors:** at SNR near the demodulation floor, drop frames probabilistically, with some corrupted and some failing CRC.
- **CAD:** channel activity detection should return true if a matching preamble is currently on the air.

**Where to simulate: chip or provider**

You can simulate at the `radio.v1` provider level, which is quick. Or you can simulate the SX1262 itself, with an SPI command interpreter plus BUSY and DIO1 pins. Then the real kernel radio driver runs unmodified against the sim, and you test the driver's state machine too (SetRx, SetTx, IRQ status, buffer read/write). If your driver is generic over `embedded-hal` traits, as lora-phy is in my understanding, the chip-level sim is just another implementation of those traits. Start at the provider level. A chip-level sim can come later as a second backend, since both would sit behind the same medium.

**Time is the big decision**

- **Wall clock:** it's simple and fine for five nodes. The test takes as long as it takes.
- **Virtual time (discrete-event simulation):** the medium is the time authority and jumps to the next event. A night's worth of game runs in minutes, and every run with the same seed is bit-for-bit reproducible, which makes a failure on hour 6 debuggable. The interpreter's fuel metering helps here, because you can charge a fixed amount of virtual time per instruction and make simulated CPU time deterministic as well.

Virtual time needs the kernel to read time through a `Clock` trait and set timers through it, never touching the hardware timer directly. That's cheap to do now and painful to retrofit.

**Caveats**

- The sim tests your logic, kernel, and memory manager, but not RF. Capture, sensitivity, and antenna effects will differ from the model. Calibrate by running the same game on the real tags and comparing loss and retry statistics.
- Seed the random number generator and log the seed with every run, so a failure can be rerun.
- Duty-cycle limits and regulatory constraints no longer limit the soak test in the sim, though you should still model them so the applets learn to respect them.

Do you want the first version in a single process with virtual time, or a hub with wall-clock time so it can bridge to the real modem tag from the start?

---

## Ben

ok yeah so let's firm this up a little bit. it's called Mesh Virtual Machine (MVM) and it has these components:

mvm - the kernel itself, runs on nrf or esp32-s3  
mrun - darwin cli, give it a profile.json and it pretends to be an MCU and runs the applets specified with the hardware specified etc. profile.json can support multiple nodes in a single process. mrun emulates the sx1262, i2c, gpio, spi, and all the rest of the things that have real endpoints on a real board.

that's another thing, board variants. we want the kernel to expose the primitives, but as far as which pin is the red LED, that comes in a dynamic hardware profile that it loads from somewhere, not a compile-time thing. that's another area where meshtastic and meshcore go to die. they ahve all these hardware variants and each is a separate firmware. we ship one microkernel per architecture with the core LoRa and BLE drivers built in. the rest can be driven by applets through kernel helpers

---

## Claude

The tidiest way to make `mrun` real is to split the kernel into a portable core and a hardware layer. `mvm-core` holds the VM, memory manager, loader, hooks, and profile validation. A small HAL trait set (gpio, i2c, spi, uart, adc, radio, flash, clock) sits under it, with one backend per target: `mvm-nrf`, `mvm-esp32s3`, and `mvm-sim`. Then `mrun` is just `mvm-core` plus the sim backend. It emulates the board at the primitive layer, not the register layer, so you aren't writing an nRF52 emulator. The catch is that the sim doesn't exercise the real backends, so those still need on-target tests (or Renode, if you ever want partial peripheral emulation).

**Runtime board profiles have good precedent.** Linux loads a device tree blob at boot, and Tasmota has runtime GPIO templates. Meshtastic and Meshcore's one-firmware-per-variant problem is exactly what this fixes. A few things to design in from the start:

- **Names, not pin numbers.** The profile maps logical names (`led.red`, `i2c.sensors`, `radio0`, `button.user`) to hardware, and applets only ask for names. The manifest declares which resources an applet needs, and the kernel grants them. A BBS applet then can't touch the radio's SPI bus or the RF switch pin, and names resolve to table slots at load, as with imports.
- **Three separate files.** A *board profile* describes hardware. A *node config* lists the applets, keys, and settings. A *sim scenario* has topology, loss matrix, seed, and time mode. Splitting them lets the same board profile drive the real device and `mrun`, and the desktop tool can validate a profile against the emulator before you flash it.
- **Compile JSON to a compact binary** (postcard or CBOR) with a schema version, CRC, and signature, stored in its own flash page. `mrun` and the desktop tool read JSON, but parsing it on a 256 KiB chip is wasted flash and RAM.

**The radio section is the dangerous part.** The kernel drives LoRa and BLE itself, so it needs the full wiring: SPI pins, NSS, reset, BUSY, DIO1, RF switch control (DIO2 or separate RXEN/TXEN), TCXO voltage on DIO3, LDO vs DC-DC, and PA settings. A wrong value can mean a dead radio or an over-power transmitter. So validate strictly, with known chip types only and unknown fields rejected. Put the board's max TX power in the profile and enforce it in the kernel, so no applet can exceed it.

**Validation before touching pins.** Check that the MCU in the profile matches the part (nRF's FICR reports the part and memory sizes). Reject duplicate pin assignments and pins reserved for USB, SWD, crystals, or strapping. Use a safe default when the profile is missing or invalid: all pins high-Z, radio off, USB/BLE comms and recovery only. One nRF gotcha is that the NFC pins (P0.09/P0.10) default to NFC function until UICR is changed. UICR lives in flash, so the kernel or flasher has to handle that once.

**Pin flexibility depends on the chip.** nRF52 PSEL registers and the ESP32-S3 GPIO matrix let most peripherals go on almost any pin, with exceptions like analog inputs and high-speed SPI. Rust HAL pin types are often compile-time generic, so look at the type-erased "any pin" types or drop to the PAC. This approach wouldn't work as cleanly on STM32, with its alternate-function tables.

**Peripherals as applets.** The kernel exposes `gpio.v1`, `i2c.v1`, `spi.v1`, `uart.v1`, and `adc.v1`, plus pin interrupts delivered as events. Raw pin access should be limited to pins the profile doesn't reserve. Shared buses need kernel arbitration. Short I2C transfers can block briefly, but long operations like an e-ink refresh should be start-then-event. The profile can also declare devices (`oled` on `i2c.sensors`, address 0x3c), so a display applet finds its device by name on any board.

**Flash budget.** "One kernel per architecture" really means per MCU variant, and on a 1 MB nRF52840 the budget is tight: bootloader, a BLE controller blob, the kernel, A/B slots if you want rollback, the profile page, applet storage, and the filesystem. I'd draw that map early, because it decides whether A/B kernel slots are possible at all. If they aren't, the LoRa-update safety story for the repeaters has to come from somewhere else, such as a small immutable recovery layer.

Are you planning to keep the stock Adafruit-style bootloader on the tags, or replace it? That determines the flash map and how much of the kernel can be updated over the air.

---

## Ben

that depends, what does the bootloader do? i know there is a softdevice too, which i think is closed source?

---

## Claude

*[Searched the web]*

**What a bootloader does.** It's the first code that runs after reset. It decides whether to jump to your application or stay in an update mode, and in update mode it receives a new image, writes it to flash, and verifies it. The Adafruit one that most nRF52840 boards ship with is a CDC/DFU/UF2 bootloader. UF2 means the board shows up as a USB flash drive, and you update by copying a file onto it. It also supports serial DFU and BLE over-the-air DFU, and it can update itself. Two resets within 500 ms drop you into the USB mode, and a retained register lets an application ask for a particular mode on reboot. It has some safety options too. It can split flash into two partitions, one for the last good firmware and one for the incoming one, and it can enforce signed images, but signing disables UF2. I'd check which build your tags have, since I don't know that. ([Adafruit nRF52 Bootloader](https://github.com/rkudaj/Adafruit_nRF52_Bootloader) +3)

**The SoftDevice** is Nordic's precompiled BLE stack, and yes, it's closed source. It's a binary blob that Nordic lets you ship, with the license terms to check. The layout is MBR, SoftDevice, your app, then the bootloader at the top. The SoftDevice size sets where your app starts, and for S140 v6 that's 0x26000, about 152 KiB into flash. It also takes some RAM, forwards interrupts through the MBR, owns some peripherals, and requires flash writes to go through its API while it's enabled. That last part matters for your flash-erase scheduling. ([github](https://github.com/gbraad/Adafruit_nRF52_Bootloader))

There's a second option on the Rust side. Nordic's SoftDevice Controller is also closed, but it's a library linked into your own image, with no separate region. The pure-Rust `trouble` host stack sits on top of it, and trouble works with the nRF Softdevice Controller as well as ESP32 and others. Then the kernel owns the vector table and everything else. It's newer and less proven than the classic SoftDevice. I don't know of a practical, fully open BLE controller for either nRF52 or ESP32-S3 on the Rust path. ([embassy](https://docs.embassy.dev/trouble-host))

**How this shapes the decision**

- **Your 50 repeaters constrain the first version.** If they run the stock Adafruit-style layout (probably S140 v6, but verify on the tags), the MVM kernel has to start at the same app address and arrive by whatever path they accept today. So I'd keep the stock bootloader and SoftDevice for now and build the kernel as an ordinary app. You also get a good first-install story: copy `mvm.uf2` to the drive from your Mac.
- **The bootloader matters less in this architecture.** The kernel changes rarely and applets change constantly. A/B slots and rollback for applets can live inside the kernel's own storage, so they don't depend on the bootloader at all. The risk concentrates in rare kernel updates.
- **A bad kernel push is the dangerous case.** With a stock bootloader, an invalid app leaves the node sitting in DFU mode, which you can't reach over LoRa. Either use the dual-bank option, or keep the kernel small, fuzz it hard, and treat kernel pushes as releases that get bench-tested first.
- **Later, a signed A/B bootloader is an upgrade.** MCUboot and `embassy-boot` both do swap with rollback, but I haven't verified their nRF52840 status. You'd pay for it in flash, because it needs a second kernel-sized slot.

How do your repeaters take OTA today? Is it Meshcore's own mechanism writing to flash, or the bootloader's DFU? That decides whether the first MVM kernel can reach them over LoRa at all, or whether the migration needs a one-time visit to each node.

---

## Ben

let's keep stock bootloader and softdevice. we actually wrote a lora-ota that ships the updates in little messages that get reassembled on the other end. we'd re-implement this as an epidemic file sharing system applet.

i believe the kernel should own a virtual filesystem where an applet can implement a filesystem provider and mount it. so by default the kernel does /boot which is a designated area of internal flash (most of it actually) as littlefs2. that is what stores the applets, settings, and may in fact be the only mount point. something like the xiao that has a 2mb extra flash would be handled by an extra_fs.mhc applet that turns that area into a littlefs2 partition. then sd_fs.mhc would be an applet that implements SD storage. so we need whatever kernel primitives to support that kind of thing.

maybe instead of `mrun` we call it `mesh` and it has a `mesh run` and `mesh cp` where it can cp a local config file or applet or whatever to a usb-attached node. something like that

---

## Claude

Keeping the stock bootloader and SoftDevice keeps the flash map simple: MBR, SoftDevice, kernel at the usual app address, and everything else is yours. The epidemic file-share applet will also be a good first VFS client, because it forces the staging, handle, and atomic-rename questions to get answered.

**Make applets provide block devices, and the kernel provide filesystems.** There are two provider layers:

- **`blk.v1`:** an applet registers a named device with its geometry (read/prog/erase sizes, block count) and exports read, prog, erase, and sync. `extra_fs.mhc` then becomes a small QSPI driver, and the kernel runs its own native littlefs on top. That keeps the hot path native instead of a nested interpreted call per block, and leaves one fuzzed littlefs in the kernel instead of several in bytecode.
- **`fs.v1`:** a full provider (open, read, write, seek, readdir, stat, rename, remove) with capability flags such as atomic rename, power-fail safe, and read-only. `sd_fs.mhc` would be a block provider for the card plus a FAT provider on top, since you want cards readable on a PC. I believe `embedded-sdmmc` is an existing no_std FAT crate. Synthetic filesystems, like a read-only view of node stats, use the same interface.

**/boot must be kernel-native.** It stores the applets, so it can't depend on one. Keep the board profile in its own raw page, as suggested earlier, so a corrupted littlefs still leaves the kernel able to bring up USB and BLE and offer a reformat. littlefs also costs RAM for caches and a lookahead buffer, so the kernel should own and cap those and limit total open files. The usual Rust crate wraps the C implementation, I believe, which matters for the all-Rust goal.

**Kernel primitives**

- Named flash partitions from the profile (`boot`, `extflash`), with raw access gated by a capability.
- Internal flash writes through the SoftDevice's flash API while BLE is enabled, scheduled around radio windows because erases stall the CPU.
- `spi` and `qspi` bulk transfers on lent buffers, so SD sectors move zero-copy, plus card-detect events.
- A mount table with longest-prefix matching. Mounts are declared in node config and gated by a manifest capability.
- A per-applet fd table of small integers, mapped by the kernel to (provider, provider handle), so handles can't be forged in either direction.
- Path checks in the kernel before any provider sees a path: normalize, reject `..`, no symlinks. Each applet gets a quota'd `/app/<id>/`, and its manifest lists other paths by prefix and mode (`rw:/data/bbs`). Fuzz this like the memory manager.
- Lent buffers passed caller, kernel, provider with no copy.
- Media removal returns `ERR_NO_MEDIA`, so open fds go stale instead of dangling. Applets can declare `requires: mount:/ext` to handle ordering.

**"Everything in /boot" collides with XIP.** littlefs files aren't contiguous. They're copy-on-write, blocks are linked by skip-list pointers, and those pointers sit inside the blocks, so a file's data doesn't even start block-aligned. An applet stored as a littlefs file can't execute in place. There are three ways out:

1. **Copy code at install into a contiguous region.** It's simple, but it leaves two copies in flash.
2. **A kernel-managed code store of extents,** exposed as a read-only synthetic mount so the epidemic applet can still read the `.mhc`. This is one copy, but it's a second small allocator. Applets are mapped at virtual windows, so moving an extent is a copy plus a pointer update, which makes defragmenting easy.
3. **Demand-paged fetch.** The interpreter fetches code through a few RAM pages (say 512 B each) filled via the VFS. This is one copy and one filesystem, and applets could run from the SD card. The cost is a page check per fetch and jump, plus a few KB of RAM.

I'd put code fetch behind a `Code` trait, like `Memory`, and start with option 1 to remove variables during bring-up. I believe the nRF52840's QSPI peripheral can map external flash for execute-in-place reads, which might make the XIAO's 2 MB a natural code store, but check that.

**Epidemic sharing as the first client.** It stages into `/boot/inbox/<hash>` with a received-chunk bitmap. When the file is complete, it checks the hash and signature, renames atomically into place, then asks the kernel to install it. That needs extend-by-truncate, random-access write, atomic `rename`, and `fsync` in the VFS. Staging is also your main flash-wear source, so cap its size and skip files whose hash the node already has.

**`mesh` CLI.** The subcommands would be `mesh run`, `cp`, `ls`, `cat`, `rm`, `log`, and `ps`, with targets like `mesh cp app.mhc usb:/boot/apps/`. The kernel's built-in management service speaks the same COBS+CRC framing as the modem protocol over USB CDC, and later BLE, with resumable offset-based chunks, a per-chunk CRC, and a final hash. It's a thin client of the VFS with admin rights, so it works in recovery mode with no applets, and `cp` reaches any mount, including provider applets. Simulated nodes expose the same service on virtual streams, so `mesh cp` gets exercised in CI exactly as on hardware. Applet install should require a signature, with a `dev_mode` node-config flag to skip it on the bench.

Do you want applets to be runnable from any mount (SD card, external flash), or only from `/boot`? That decides which of the three code-fetch options makes sense.

---

## Ben

we want 3. there is not enough room on flash to do the contiguous thing. we tried this in a previous version and solved it with the interpreter being given a BytecodeReader provider that keeps track of the PC, knows how to read the next instruction, jump to known locations, etc. Then all of the complexity of how to actually fetch that from persistent storage is eliminated. a 512 byte page thing isn't a bad idea. i'm nt married to this approach, but i'm just telling you that having a virtual contiguous footprint is a requirement

---

## Claude

Agreed. A virtual contiguous code space behind a reader trait is the right shape, and it fits next to the `Memory` trait: `Memory` for data, `Code` for instructions, and the interpreter generic over both. A few design points will matter once it's running.

**Fetch in 16-bit units with a pinned current page.** That avoids the instruction-straddles-a-page problem for RV32C, where a 4-byte instruction can start at a 2-byte offset:

```rust
#[inline(always)]
fn fetch16(&mut self, pc: u32) -> Result<u16, Trap> {
    let off = (pc & PAGE_MASK) as usize;
    if pc >> PAGE_SHIFT == self.cur_tag {          // fast path: pinned page
        return Ok(u16::from_le_bytes([self.cur[off], self.cur[off + 1]]));
    }
    self.fault(pc)                                   // slow path: fill, pin, retry
}

let lo = code.fetch16(pc)?;
let insn = if lo & 3 == 3 { lo as u32 | (code.fetch16(pc + 2)? as u32) << 16 } else { lo as u32 };
```

The second half of a 32-bit instruction takes the slow path on its own if it crosses into the next page, so no special case is needed. If you end up with your own translated bytecode, the translator can guarantee that no instruction straddles a page and drop even that.

**The page cache.**

- Use 2 to 4 slots of 512 B, shared across all applets, so about 2 KiB total. One slot isn't enough, because a call that crosses a page thrashes it on every call and return.
- Store a slot index and a tag in the cursor, not a reference. A nested cross-applet call can evict the caller's page, so after a return the tag check fails and the page faults back in.
- Make it a trait with an in-RAM implementation. Small applets can be copied to RAM and bypass paging, and `mesh run` can use a plain `Vec`, so the desktop build skips the whole mechanism.

**Rodata comes through the same cache.** If you go with RV32, constants live in the code window, so data loads from that window have to be served by the page cache. That means a data load must never evict the page the PC is in, so pin that one. The upside is that big constant tables, like a BBS's help text or a font, cost no RAM. With wasm, active data segments would normally be copied into RAM at load, so you'd want to mark read-only segments as paged to get the same benefit.

**Fault cost.** As I understand littlefs, files are skip lists, so a random seek costs a logarithmic number of block reads. On internal flash that's negligible, but on an SD card each hop is a sector read. Most faults are either sequential or hit recently used pages, so measure before optimizing. If SD faults show up in profiles, the easy wins are prefetching the next page on sequential faults and pinning hot handlers' pages. I'd avoid building a page-to-block index at install, because it depends on littlefs internals.

**Integrity needs a rethink on removable media.** Install-time signature checks only prove the file was good at install, and a card can be modified afterward. Put a per-page hash list (or a hash tree) in the signed header and verify each page as it fills. SHA-256 over 512 B should be cheap next to an SD read, but measure it. On internal `/boot` you can skip it.

**Failure behavior.** If the medium is removed or a read fails mid-fetch, trap and unload that applet. Never fault the kernel. A synchronous fill blocks the interpreter, so charge fault time against the applet's fuel budget. The radio state machine is native, so a slow SD read doesn't stall RX or TX timing.

Which instruction format are you leaning toward now: RV32, wasm-derived, or your own bytecode? The rodata handling above differs between them, and a bytecode produced by a translator is the only one where we can guarantee page alignment at build time.

---

## Ben

i think the mhc files should have signing built in and the kernel can be configured to accept certain developer keys before it will run an applet. then when an applet is loaded, the signing can be checked regardless of media removability or not

---

## Claude

That works, with one wrinkle: a signature checked once at load only proves what the file looked like at that moment. Applets now page in from storage on demand, so someone who swaps the SD card after load, or flips bits in a staged file, gets past it. So I'd do two checks:

1. **At every load:** verify an Ed25519 signature over the header, manifest, and page-hash table. Then look up the signer in the trust store, check the version against anti-rollback, and compute the capability grant. This happens once per load, so even a slow software implementation (tens to hundreds of milliseconds, depending on the crate) is fine.
2. **Per page, on fault:** hash each 512 B page as it's filled and compare it to the signed table. SHA-256 over 512 B should take on the order of 100 µs on a 64 MHz M4, but measure it. Apply this only to mounts whose provider flags say `removable`. On internal `/boot`, anyone who can write that flash can already replace the kernel, so the extra check buys nothing.

**File layout sketch**

```
header:      magic, format_ver, page_size, code_len, hash_alg
manifest:    name, version, kind (user|system), imports, provides, requires,
             capabilities, mounts, stack/fuel, static size
page_hashes: one truncated SHA-256 per page
signer:      key_id (hash of pubkey), signature over everything above
```

Sign the manifest so nobody can edit the capability request without breaking the signature. The hash of the signed bytes can double as the file's ID, so the epidemic sharer can name, dedupe, and verify files by it. A 60 KiB applet has about 120 pages, so a 32 B hash per page is roughly 3.75 KiB, or 6% overhead. Truncating to 16 B halves that and is still strong enough for this use.

**The trust store should hold keys with scopes.** Each entry is a key ID, a public key, and the maximum capabilities that key can confer. A community developer's key can load a BBS applet but not get raw flash access or override host functions. System applets need a higher-tier key. The effective grant is the manifest request, intersected with the key's scope, intersected with what node config allows. Node config can also pin specific applet hashes, so a repeater runs only what you expect, even from a trusted key. Keep the trust store in internal flash, never on removable media, and require an owner-key signature or a physical USB session to change it.

**Signer identity can feed the cross-applet call.** If `caller()` returns both the applet ID and the signer key ID, the DB applet can scope namespaces by developer without trusting anything in the arguments.

**Things a signature won't cover**

- **Rollback:** an old version with a known bug is still validly signed. Keep a per-name minimum version in node config and bump it on install. Otherwise the epidemic sharer becomes a downgrade channel.
- **Revocation:** a leaked developer key has to be removable. Signed revocation entries can ride the same epidemic distribution.
- **Physical access:** with the stock UF2 bootloader, anyone with USB can flash a kernel carrying a different trust store. Signing defends against remote delivery and swapped media, not against someone holding the node. That's probably the right threat model for solar repeaters.
- **Dev workflow:** instead of the `dev_mode` skip-signature flag I suggested earlier, `mesh` can generate a local dev key and add it to a bench node's trust store. There's then no bypass flag that could be left on in the field.
- **Profiles and node config:** a malicious board profile could mis-set pins or TX power. Profile validation caps the damage, but having the owner key sign these files too closes the door.

Do you want a flat list of developer keys in each node's trust store, or one owner key per fleet that signs developer-key certificates? Flat is simpler in the kernel. With an owner key you update trust for all 50 repeaters by shipping one signed certificate, but the kernel then has to parse and verify chains.

---

## Ben

not #2. i think just verify it once when the applet is installed or seen for the first time. if someone has physical access to the node, they can do anything they want anyway so we don't need to worry about that case. we only need to worry about authenticity at install time.

---

## Claude

Agreed, and it simplifies the format. The page-hash table goes away (the roughly 6% overhead), and the signature covers one SHA-256 over header, manifest, and code, computed as a stream at install. Page size also stops being part of the file, so you can retune the cache later without re-signing anything. That same hash serves as the file ID for the epidemic sharer.

```
header:    magic, format_ver, code_len
manifest:  name, version, kind, imports, provides, requires,
           capabilities, mounts, stack/fuel, static size
code:      instruction and rodata image
signer:    key_id, Ed25519 signature over SHA-256(header | manifest | code)
```

Hashing a 60 KiB applet should take tens of milliseconds, but measure it. A few details keep install-time-only checking sound without adding runtime cost:

- **Verify the file the kernel owns, not the path.** The epidemic applet stages a file in the inbox, then calls `install`. The kernel moves it into a code directory that only the installer can write, closes any handles the caller held, then hashes and verifies it in place. Otherwise the applet that staged the file could modify it between verify and run, which is the one way around install-time checking that needs no physical access. The same rule means no manifest mount grant should ever include the code store.
- **Key "first seen" by content, not path.** The kernel keeps an install registry in `/boot` with file ID, signer, version, and granted capabilities. A file that shows up on an SD card is unknown, so it gets a full verify then. On later loads the kernel reads only the header and signature and compares them to the registry. A different file at the same path counts as new and gets verified, and a matching one skips the code re-hash. A byte-modified copy with the same header would pass, which is the physical-access case you've decided to accept.
- **Filesystem providers join the trust boundary.** An `sd_fs` applet serves the bytes on every page fault, so it could serve something other than what was verified. That's fine if `fs.v1` and `blk.v1` provider rights are a high-tier capability that ordinary developer keys can't confer. Make sure the key-scope table treats them that way.

Accidental corruption is less scary than it sounds. A flipped bit in code gives garbage instructions, the interpreter traps, and the kernel unloads that applet, so the sandbox protects everything else. For diagnosing a flaky card or worn flash, `mesh verify` could re-hash a node's installed applets on demand over USB, with nothing running at runtime.

Still open from before: do you want a flat list of developer keys in each node's trust store, or one owner key per fleet that signs developer certificates? Flat is simpler in the kernel. With an owner key you can update trust across all 50 repeaters by shipping one signed certificate, but the kernel then has to verify chains.
