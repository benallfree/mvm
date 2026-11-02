# Review: contradictions, gaps, and sequencing

A review of [the design plan](mesh_virtual_machine_5ae0e741.plan.md) (D) and [the implementation plan](implementation-plan.md) (I) as of commit `483f90d`. Each item is something to discuss, decide, and fold into one or both plans.

## Ground rule: the trust model

Adopted 2026-10-31 and narrowed the same day; it is the first locked decision in D. The node trusts its operator, and applets are trusted collaborators. In v1 the only guardrail is memory: per-applet limits stop a buggy applet from taking every page. Applets may read and write each other's memory, call every kernel function, layer on any name, and write any file. A pointer into an unmapped page and a write to the kernel's own pages still trap. Applet isolation, grants, and signing are in the [icebox](icebox.md). Items marked **(trust model)** should be re-read in that light.

## How to use this file

- Work through the items one at a time. Ids have a prefix per section: `C` for contradictions, `DG` for design gaps, and `PS` for implementation-plan sequencing. They were renumbered once, on 2026-10-31, so each section reads in order. From now on they are stable: a new item takes the next number in its section.
- When an item is settled, fill in its **Decision** line, make the change in D and/or I in the same commit, and check the box.
- An item that turns out to be a non-issue is checked with a Decision of "No change" and a one-line reason.
- Line numbers are as of `483f90d` and will drift as the plans change.

## Discuss first

The fresh review after this session's decisions (C12 and C16–C18) found two conflicts they introduced, DG7 and C20. Both are settled. The second review's open items are R1–R4 at the end.

Next, the remaining open items in order: C14, C15, C23, then DG1 onward, then PS1 onward.

---

## Contradictions

Places where the two plans, or two sections of one plan, disagree.

- [x] **C1. Unknown names: trap or return?**
  - Where:
    - [D:132](mesh_virtual_machine_5ae0e741.plan.md:132) and [D:461](mesh_virtual_machine_5ae0e741.plan.md:461) say an unknown name traps.
    - [D:63](mesh_virtual_machine_5ae0e741.plan.md:63), I 5.2, frisbee ([D:633](mesh_virtual_machine_5ae0e741.plan.md:633)), and I 5.21 (nRF network calls) expect `ERR_UNAVAILABLE` to be returned.
  - Issue: It has to return, or frisbee and the network applets can't degrade gracefully. There is also no defined way to tell an error from a valid result. "Negative means error" collides with `mem.malloc` addresses in partitions `0x80`–`0xFF`, which are negative as `i32`.
  - Recommendation: Unknown names return `ERR_UNAVAILABLE`. Every `mesh` call returns a status in `a0` and a value in `a1`, and the SDK wraps that in a `Result`.
  - Decision: Option A, adopted 2026-10-31. Unknown names return `ERR_UNAVAILABLE`. Status in `a0`, value in `a1`. Added `ERR_NO_MEMORY`, which replaces `mem.malloc` returning null. Folded into D (Return convention, kernel calls, mvm-vm, errors, pressure, chaos) and I (5.2, 9.3).

- [x] **C2. Do reservations include statics, and are reserved pages "mapped"?**
  - Where:
    - [D:418](mesh_virtual_machine_5ae0e741.plan.md:418) says both limits count every page, statics first.
    - [D:488](mesh_virtual_machine_5ae0e741.plan.md:488) (load: "statics plus its `reserved_ram`"), [D:52](mesh_virtual_machine_5ae0e741.plan.md:52), G1, G4, and I 4.12 treat them as additive. They also say reserved pages are "mapped" while the heap has zero pages.
  - Issue: Two readings give different load checks, different G1/G4 numbers, and different `mem.stat` values.
  - Recommendation: Reserved pages are *set aside*, not mapped. `reserved_ram` includes statics, the same way `max_ram` does. G1 becomes "mapped = statics, heap = 0, set aside = `max(reserved_ram, statics)` − statics".
  - Decision: Adopted 2026-10-31. Both limits include statics. Statics are mapped and filled once at load, before `init` (`.data` copied, `.bss` zeroed), and never fault afterwards. Lazy statics were rejected because running out of memory on an ordinary store can only trap, not return an error. The rest of a reservation is set aside, not mapped. The load check uses the larger of statics and `reserved_ram`. Folded into D (locked decisions, limits, load) and I (G1, G4, 4.12).

- [x] **C3. BYE on clean unload has no hook**
  - Where: [D:630](mesh_virtual_machine_5ae0e741.plan.md:630), I 6.3 ("the stopped player sends `BYE`"), and the unload routine ([D:477](mesh_virtual_machine_5ae0e741.plan.md:477): "stops at once and never resumes").
  - Issue: The only export is `init`, and `mesh stop` gives the applet no chance to run.
  - Recommendation: An optional `sys.stopping` event, delivered on `mesh stop` and install replacement only (not on a trap), with a small fixed fuel budget, before unload. Alternatively, drop `BYE` on stop and rely on roster expiry.
  - Decision: Option A, adopted 2026-10-31.
    - `sys.stopping(reason)` goes to the stopping applet only, on `mesh stop`, its file being replaced, or the reset that follows (reasons `stop`, `replace`, `reboot`; `mesh restart` was later dropped under C18).
    - It has a fuel budget of `[kernel] stop_fuel_pct` (default 10%).
    - There is no notice on a trap, fuel exhaustion, or `sys.exit`.
    - Native work it starts still completes, and its completion callbacks are dropped.
    - Frisbee sends `BYE` from it.
    - Folded into D (`evt.emit`, events intro and table, unload, frisbee rules) and I (5.6, 6.3).
    - Revised again under DG26: `sys.stopping` became one `sys.rebooting(reason, initiator)` broadcast to everyone before any planned reboot.
    - Revised 2026-11-01 under DG2: there is no `mesh stop`, no `sys.exit`, and no unload at runtime. `sys.stopping` now comes only before a planned reset (`replace` or `reboot`), as its own turn under `instr_per_event`; `stop_fuel_pct` is gone.

- [x] **C4. Completions as callbacks vs device events**
  - Where:
    - The locked rule [D:58](mesh_virtual_machine_5ae0e741.plan.md:58) says completions are callbacks and events are broadcasts.
    - Yet `ble.connect` reports through `ble.up`/`ble.down` ([D:191](mesh_virtual_machine_5ae0e741.plan.md:191)).
    - `uart.rx`, `ble.rx`, `usb.rx`, and `gpio.irq` are broadcast events, while TCP and UDP use per-socket callbacks.
    - `gpio.irq` still says "granted pin" ([D:236](mesh_virtual_machine_5ae0e741.plan.md:236)).
  - Issue: The model is inconsistent. Per-resource data goes to every listener.
  - Recommendation: `uart.open`, `ble.connect`, `ble.scan`, `usb` channels, and `gpio.configure` take `callback, ctx` like `tcp.connect`. Broadcast events stay for `radio.rx`, `net.up`/`down`, `sys.ready`, and `mem.low`.
  - Decision: Adopted 2026-10-31 with an addition.
    - Per-resource callbacks for `gpio.configure`, `uart.open`, `ble.scan`, `ble.connect`, and `ble.advertise`, with new `ble.stop` and `ble.disconnect`. The `gpio.irq`, `uart.rx`, and `ble.*` events are removed.
    - Added transport taps: broadcast `tap.rx` and `tap.tx` for every UART, BLE, USB, TCP, UDP, and radio transfer, so an applet such as network stats can observe traffic it does not own. Taps are read-only, and the owner's callback runs first. Metadata and payload go to any listener with no grant, under the trust model.
    - USB channels are left to DG9.
    - Folded into D (locked decisions, kernel calls, events, new Transport taps section, mvm-hal) and I (5.13, 8.4, new 5.25).

- [x] **C5. Running the embedded bootstrapper from the kernel image vs G6/G7**
  - Where: [D:493](mesh_virtual_machine_5ae0e741.plan.md:493) ("run the embedded default straight from the kernel image").
  - Issue: Read literally, this runs code from RAM, which breaks the no-code-in-RAM guard.
  - Recommendation: State that it is paged through a `Code` reader over the kernel image in flash, using the shared slot cache. Nothing is copied to RAM.
  - Decision: Adopted 2026-10-31. A new `FlashCode` reader sits next to `FileCode`. It pages from the kernel's flash image through the shared slot cache. The unwritable-`/boot` fallback uses it. Folded into D (mvm-code, boot) and I (5.10 check with an unmountable `/boot` and G6).

- [x] **C6. When a trapping `boot.mhc` is renamed**
  - Where: [D:492](mesh_virtual_machine_5ae0e741.plan.md:492) renames it and boots the default in the same boot. I 5.10 says "on the next boot".
  - Recommendation: Same boot, as the design says. Fix I 5.10.
  - Decision: Replaced by safe mode, adopted 2026-10-31.
    - There is no rename and no replacement. Safe mode loads no bootstrapper and no applets; only the kernel and the management service run.
    - It is entered from a retained flag set by `mesh reboot --safe`, from a bootstrapper that traps during boot (which sets the flag and resets, so safe mode always starts clean), from `boot_max_failures`, or from an unmountable `/boot`.
    - The flag and counter are one-shot, and files on `/boot` are never touched.
    - A read-only `/boot` still runs the embedded default through `FlashCode`.
    - Folded into D (locked boot policy, boot, watchdog, mvm-hal, applets/boot, apps/mesh) and I (5.10). This also settles the safe-mode half of DG7.
    - Reversed under DG7 (2026-10-31): safe mode moved to the icebox. A bootstrapper is still never renamed or replaced.

- [x] **C7. Is the ESP32-S3 the second backend or a first-class target?**
  - Where: [D:45](mesh_virtual_machine_5ae0e741.plan.md:45) says "second backend". [D:593](mesh_virtual_machine_5ae0e741.plan.md:593) and all of I say "first-class from the start".
  - Recommendation: Reword D:45.
  - Decision: Adopted 2026-10-31. The ESP32-S3 is a first-class citizen equal to the nRF52840. D:45 now names both families and their boards as equal targets, verified at every milestone.

- [x] **C8. `radio.v1` is stale**
  - Where: [D:533](mesh_virtual_machine_5ae0e741.plan.md:533), [D:622](mesh_virtual_machine_5ae0e741.plan.md:622), [D:660](mesh_virtual_machine_5ae0e741.plan.md:660).
  - Issue: Names are now `radio.tx` and `radio.config`. Versioned scopes are gone ("a breaking change is a new name").
  - Recommendation: Replace with "the `radio.*` names".
  - Decision: Adopted 2026-10-31. Replaced in D (mvm-sim, frisbee, lora-modem-proto) and I (7.3). The design chat transcript is left as the historical record. Raised C16 (kernel ABI versions).

- [x] **C9. A `dropped` status that doesn't exist**
  - Where: [D:647](mesh_virtual_machine_5ae0e741.plan.md:647) says the filter completes handles "as dropped". The `radio.tx` statuses at [D:158](mesh_virtual_machine_5ae0e741.plan.md:158) are `ok`, `duty`, `busy`, and `cancelled`.
  - Recommendation: Add `dropped`, or have the filter use `cancelled`.
  - Decision: Adopted 2026-10-31. `dropped` is a real `radio.tx` status, meaning a middleware layer chose not to send the frame, and `cancelled` keeps its meaning of a radio abort. New rule in D: callers treat any unknown error code or status as a generic failure, so additions are minor ABI bumps.

- [x] **C10. Native builds as test oracles**
  - Where: [D:62](mesh_virtual_machine_5ae0e741.plan.md:62) says applet source is never recompiled natively for tests. I 5.24 compiles the workloads natively and compares outputs.
  - Recommendation: Allow native builds only as a reference for output comparison, never as a substitute for running the `.mhc`.
  - Decision: No change to the locked decision, 2026-10-31. The native comparison was removed from I 5.24 instead.
    - 5.24 is rewritten around Ben's intent: upstream C and Rust libraries compile to bytecode with their source untouched, and any MVM adaptation happens at link time. Each workload checks its own answers.
    - The SDK's C runtime becomes an embedded C library (picolibc or similar) with its allocator mapped to `mem.*` at link time.
    - G8 is reworded. Threads, `FILE*`, `setjmp`, and atomics are listed as not covered in v1.
    - Folded into D (mvm-sdk C runtime) and I (G8, 5.24).

- [x] **C11. The ban on `read_unaligned` and packed structs vs unmodified crates (G8)**
  - Where: [D:85](mesh_virtual_machine_5ae0e741.plan.md:85) and [D:403](mesh_virtual_machine_5ae0e741.plan.md:403) set the ban. G8 and I 5.24 run unmodified crates. `memchr`, which `serde_json` uses, calls `read_unaligned`.
  - Issue: On `riscv32imc`, LLVM lowers unaligned reads to byte loads, so they never trap. Only a misaligned instruction from an undefined-behaviour pointer cast traps.
  - Recommendation: Drop the ban. The rule becomes "a misaligned load or store instruction traps", which only buggy code hits.
  - Decision: Adopted 2026-10-31. The ban is dropped. Ben's original concern was wire-format portability: compilers pad and align structs differently, so a struct cast can mismatch across sides. That is kept and now stated as the reason for the explicit little-endian layouts in Payload bytes and the SDK docs. The trap rule says only misaligned instructions trap, which only undefined behaviour produces. Folded into D (address map note, Payload bytes, mvm-sdk helpers).
    - Shared data has two tiers:
      - Applet to applet, anywhere (same node, over air or cable, across nRF, ESP32-S3, and Darwin): share C-layout structs directly. All applets use one target ABI and run interpreted, so the bytes are identical. Rust must use `#[repr(C)]`, and bitfields are not used.
      - Applet to non-applet (kernel, other firmware, PC tools): explicit little-endian codecs.
    - Over-air structs get a version byte and a length check, as advice.
    - Added I 5.26 (C and Rust interop on one node) and I 8.6 (interop across nRF, ESP32-S3, and Darwin over LoRa, BLE, and USB), with an 8.6 exit-check line.

- [x] **C12. The CLI command lists disagree**
  - Where:
    - The locked decision [D:61](mesh_virtual_machine_5ae0e741.plan.md:61) lists 10 commands.
    - `apps/mesh` adds more but lacks `tty` and `ping`, which I 1.5 and 1.13 use.
    - The management service ([D:506](mesh_virtual_machine_5ae0e741.plan.md:506)) has no radio-test mode.
  - Recommendation: One complete list in `apps/mesh`, with the locked decision pointing to it. Add `ping`, `tty`, and the management radio-test mode.
  - Decision (partial, `mesh build` only): Docker only. The applet toolchain (clang and lld, the C library for the applet target, Rust's RV32 target, the SDK template) ships as one image per SDK version, pinned by digest. `mesh build` always compiles inside it and packs on the host. There is no `--native` mode: it would risk hash drift, since the content hash is the install id, and double the support paths. CI builds applets in the same image. A cloud builder (`--remote`, the same image hosted) is possible later and would produce identical hashes. Folded into D (mvm-sdk build image, apps/mesh) and I 2.6a–2.6e (SDK crate, packer, the template added to the image, `mesh build`, and a cross-machine hash check), with the image itself moved to 2.1 because every applet build depends on it and 5.24 (C library added to the image). Then the rest:
    - One command table in D's `apps/mesh`, grouped Project, Host runtime, Files, Node control, and Diagnostics, with the implementation step that first builds each command. The locked decision points to it.
    - `mesh ping` (versions, board, reset reason; works in safe mode) and `mesh tty` with a radio-test mode in the management service. How `tty` shares radio settings with applets stays with DG8.
    - Node targets are `--node usb:SERIAL | ble:NAME | host:NODE`, optional with one node reachable, and node paths start with a colon (`:/boot/apps/`). `run` is a host runtime, not just a simulator, so its nodes are `host:`.
    - `mesh.toml` holds the manifest fields for C and Rust projects.
    - `mesh mount`: a local NFSv3 server bridged to the management service, with Finder litter filtered. It replaces `mesh init`; board setup is pasting a preset into `config.ini`, and the web flasher with autodetection is in the icebox.
    - `mesh guards` is dropped. `mesh ps --json` carries the counters, and `cargo xtask guards` checks them.
    - `mesh cfg` stays as it was. `mesh restart` was dropped under C18. `keygen` and `trust` were added under C17 and removed again when signing went to the icebox.
    - Folded into D (locked CLI bullet, `mvm-sdk`, packer, `mgmt`, `apps/mesh`) and I (1.8a new, 1.13, 1.20, 2.6d, 2.7, 3.3, 4.12, 5.1, 5.3, 5.6, 5.10, 5.11, 5.13, 5.24, the drift-guard rules).

- [x] **C13. Override grants built twice**
  - Where: I 5.5b already needs override grants ("without the grant, layering fails"). I 5.14 implements them as its Do.
  - Recommendation: Grants land in 5.5b. 5.14 becomes only the `config.get` replacement test, which depends on DG4.
  - Decision: No longer applies. Override grants are gone (C19): any applet may layer on any name. 5.5b and 5.14 lost their grant parts.

- [x] **C14. The config parser step leaves out `[net]`**
  - Where: I 3.4.
  - Recommendation: Add `[net]`.
  - Decision: Adopted 2026-11-01. `[net]` holds `ssid` and `password` (plain strings, empty password for an open network). No `[net]` means Wi-Fi off, and boards without Wi-Fi ignore it. 3.4 parses it with tests for the missing section, the empty password, and an unknown key. Also stated that the bootstrapper's instance name is `boot` and `[boot]` is for custom bootstrappers.

- [x] **C15. Stale text in the design plan**
  - Where: The frontmatter `todos` (the order is still sim first, ESP last, paging late) and [D:37](mesh_virtual_machine_5ae0e741.plan.md:37) ("moving the agent into it").
  - Recommendation: Delete the todos (the implementation plan replaces them) and the D:37 sentence.
  - Decision: Adopted 2026-11-01. The frontmatter `todos` are gone (`name`, `overview`, and `isProject` stay), the "moving the agent into it" sentence is gone, and "The phase list stays" now points to the implementation plan.

- [x] **C16. Kernel ABI versions and feature queries**
  - Where: New, raised while settling C8. `mvm-abi` ([D:125](mesh_virtual_machine_5ae0e741.plan.md:125)) says a breaking change is a new name, but nothing lets an applet ask what the running kernel offers.
  - Issue: An applet can learn that a name is missing only by calling it, which has side effects. Byte layouts, the return convention, and the `.mhc` format also change over time, and none of them are names. Logs and `mesh ps` can't say which kernel an applet ran against.
  - Recommendation:
    - `sys.version(out)` fills a fixed layout: kernel version (major, minor, patch), ABI level, build hash, MCU family, and the `[board]` preset name.
    - The ABI level is one integer that only grows. It goes up when names or layouts are added, and never for a breaking change, because a breaking change is a new name or a new layout.
    - `sys.has(name)` returns whether a name is provided, and by what (a builtin, an applet, and the number of layers), without calling it.
    - The `.mhc` header's `format_ver` already guards the file format, and the kernel refuses formats it doesn't know.
    - An optional manifest `min_abi` makes the kernel refuse the load with a clear log line instead of failing later on a missing name. The packer leaves it unset unless the author sets it.
  - Decision: Adopted 2026-10-31.
    - Two semver numbers: the kernel version, and the ABI version, which is the `mvm-abi` crate's version (minor bumps add names or layouts, and there is no major bump).
    - `sys.version` reports both, plus the build hash, MCU family, and board preset.
    - `sys.has(name)` detects features without calling anything.
    - The packer records `abi` in the manifest, and optional `min_abi` refuses the load.
    - `mesh ps` shows the applet's built-against ABI and the kernel's.
    - Folded into D (mvm-abi versions, kernel calls, manifest, packer, load, `mesh ps`) and I (3.1, 3.3, 5.2, 5.9).

- [x] **C17. Applet signing was left behind, but updates will travel over the mesh**
  - Where: D's "Left behind on purpose" list (applet signing, a developer trust store), the packer ("no signature block"), and the capability bullet ("no signer and no key scope").
  - Issue: Updates are meant to spread epidemically over the mesh, so a node must check that an applet comes from a trusted developer and is intact before it runs or replaces anything.
  - Decision: Applets are signed with Ed25519.
    - `mesh keygen` makes a developer keypair. `mesh build` signs on the host after packing, and the key never enters the build image.
    - The signature block follows the content, and the content hash covers everything before it, so builds stay reproducible.
    - Nodes list trusted keys in `[trust]` in `config.ini`, and `mesh trust` lists, adds, and removes them.
    - The loader refuses unsigned, tampered, or untrusted applets with `ERR_UNTRUSTED`. The embedded default bootstrapper is exempt. There is no unsigned mode.
    - Users upgrade with `mesh cp`. Epidemic distribution and automatic upgrades are in the icebox, together with their upgrade rule.
    - Key revocation and rotation are open questions.
    - Folded into D (locked decisions, errors, packer, `load`, `crypto`, `profile`, `apps/mesh`) and I (2.6b, 2.6d, 2.6e, 2.7, 3.4, 5.9, open questions).
  - Reversed later on 2026-10-31: signing adds complexity that isn't needed yet, so all of it moved to the icebox. No keys, no `[trust]`, no `mesh keygen` or `mesh trust`, no `ERR_UNTRUSTED`, and no signature block. The loader's content hash is the only check on code.

- [x] **C18. Reloading one applet breaks inter-applet callbacks**
  - Where: Completion handles and repeating requests in D, `mesh restart`, and replacing a loaded applet's file.
  - Issue: An applet that registers a callback with another applet loses it when that applet reloads. The provider's subscriber list was in its memory, the caller gets one `ERR_UNAVAILABLE`, and it has no way to know when to subscribe again. Any applet holding state for others has the same problem.
  - Decision: Applets are never reloaded one at a time in v1.
    - Replacing a loaded applet's file resets the whole node with the chips' software reset.
    - The replaced applet is stopped (`sys.stopping`, `replace`) before any byte of its file changes. After `replace_settle_ms` with no more such writes, the rest get `sys.stopping` (`reboot`), and the node resets once per batch with the reason word `replace`.
    - `mesh restart` is gone. `mesh reboot` reloads everything, and `mesh stop` stays.
    - Traps still unload one applet. Whoever was waiting on it gets `ERR_UNAVAILABLE`.
    - A kernel-owned event bus that would make single-applet reloads safe is in the icebox.
    - Folded into D (watchdog bullet, `sys.stopping`, unload, "Files in use", `[kernel]`, `mesh stop`, soak churn) and I (5.11, 5.20, 6.3, 9.6).

- [x] **C19. Grants and boot live in files any applet can write**
  - Where: D's trust-model bullet (grants guard reset and boot), `sys.reset` ("needs a node-config grant"), the open-filesystem rule (DG14), "Files in use" (reset on replace), and `profile` (`[grants]` and `[trust]` in `config.ini`). I 5.7 has an applet rewrite `config.ini`, and 9.4 uses a granted reset.
  - Issue: With the DOS-style filesystem, any applet can add itself to `[grants]`, overwrite `/boot/boot.mhc`, rewrite `[applets]` or `[kernel]`, and reset the node at once by writing any loaded `.mhc`. The `sys.reset` grant and the trust model's "grants guard reset and boot" no longer hold. One buggy write to `config.ini` can also stop the node booting its applets (DG21).
  - Options:
    - (a) `/boot/config.ini`, `/boot/boot.mhc`, and loaded `.mhc` files are written only through the management service (the user, via `mesh`). Applets read them freely and write anywhere else. The DOS feel stays for the user and for applets' own data.
    - (b) Keep everything writable, and say plainly that grants only stop accidents through the API, with safe mode as the backstop for a damaged `/boot`.
  - Recommendation: (a). It is a small carve-out that keeps the four things the trust model says a bug must not break.
  - Decision: Settled by removing what it protected. On 2026-10-31 grants, capabilities, and applet isolation moved to the icebox: any applet may call `sys.reset`, layer on any name, and write any file, so there is nothing in `config.ini` left to guard. A damaged `/boot` is recovered through safe mode, which already exists. The kernel's own pages stay protected.

- [x] **C20. What starts the reset after a replacement**
  - Where: D "Files in use"; I 1.8 (interrupted, resumed copy) and 5.11.
  - Issue: An unplugged USB cable or a slow BLE `mesh cp` pauses longer than `replace_settle_ms`, so the node resets with a half-written file, which the loader may refuse as malformed or, worse, run. Triggering on "opened for writing" is also too eager: an applet opening its own file read-write, or an NFS attribute change, would stop it.
  - Recommendation: Trigger on the first real write, truncate, rename, or delete, not on open. For `mesh cp`, start the reset when the copy completes (its final hash checks out). Use the idle timer only for writes through `mesh mount`. Better still, `mesh cp` writes to a temporary file and renames it over the old one at the end, so an interrupted copy never touches the running applet's file.
  - Decision: Adopted 2026-10-31.
    - Only a real change (write, truncate, rename over, delete) stops the applet and starts the settle timer. Opens and attribute changes do not.
    - `mesh cp` writes `name.part` (resumable) and renames it over the target after the final hash, so a stalled copy never touches the running file or resets the node.
    - The `.mhc` header gains `body_hash`, a SHA-256 of everything after the header. The loader refuses a torn file as corrupt, which covers in-place writes through `mesh mount`. The content hash stays the whole-file SHA-256.
    - Folded into D (Files in use, load, mvm-packer, apps/mesh) and I (1.8, 2.6b, 5.11, results log).
    - Follow-up 2026-11-01: `mesh mount` also stages `.mhc` writes in the bridge and lands them through `name.part` and a rename, so `body_hash` is a safety net for applet writes and bad flash (I 1.8a).

- [x] **C21. How the embedded default bootstrapper is exempt from signing**
  - Where: D's signing bullet and `load` (exempt), `boot` (written to `/boot/boot.mhc`, recognised by `boot_default_hash` in `[kernel]`); I 5.9 and 5.10.
  - Issue: The default is loaded from `/boot/boot.mhc` like any file, so the loader must recognise it. If it trusts `boot_default_hash` from `config.ini`, anyone can exempt any unsigned `boot.mhc`. Also, 5.9 expects the default to load with an empty `[trust]`, but the default is embedded and written only in 5.10.
  - Recommendation: Exempt only the content hash compiled into the kernel image, and say so in D. Move that check from 5.9 to 5.10. Note that building the kernel now needs the applet build image to produce the embedded bootstrapper (see PS6).
  - Decision: No longer applies. Applet signing moved to the icebox (see C17).

- [x] **C22. Replacing an applet with a build from an untrusted signer loses it**
  - Where: D's signing bullet and "Files in use".
  - Issue: `mesh cp` of a build signed by a key the node does not trust overwrites the old file and resets the node. The new file fails with `ERR_UNTRUSTED`, and the old one is gone.
  - Recommendation: Before replacing a loaded `.mhc`, `mesh cp` checks the new file's signature against the node's `[trust]` and refuses unless `--force`.
  - Decision: No longer applies. Applet signing moved to the icebox (see C17).

- [x] **C23. An applet writing its own loaded file**
  - Where: D "Files in use".
  - Issue: If the writer is the loaded applet itself, or an applet it called, `sys.stopping` would arrive in the middle of its own callback.
  - Recommendation: Defer the stop until the current callback returns, or refuse the write. Moot if C19 makes loaded `.mhc` files writable only through the management service.
  - Note after C19: option (a) was not taken, so applets can still write loaded `.mhc` files and this still needs an answer.
  - Note after C20: only a real change triggers the stop now, so an applet that merely opens its own file read-write is fine. The open question is an actual write from inside its own callback.
  - Decision: No change, accepted risk (2026-11-01). Applets can already write raw flash, SPI, and QSPI through kernel primitives, so guarding this one path is not worth it. D's "Files in use" says the outcome is undefined and v1 accepts it. The `ERR_BUSY` and deferred-rename design is in the icebox.

- [x] **C24. Stale text after C12–C18**
  - Where: D's "Out of the first cut" (still listed applet signing, and quotas as a reason for the filesystem), the frisbee middleware applet "loaded mid-game", "permissions" in the VFS and `sd` sections, `[board]` "read-only"; I 1.21 (`mesh net <board>`) and 2.7 ("then reboot" before `mesh reboot` exists).
  - Decision: Fixed in place: the first-cut list names epidemic distribution instead of signing and drops quotas, the middleware applet comes in with `mesh reboot`, "permissions" is gone from the VFS and `sd` text, `[board]` is read-only through `config.get` and edited in `config.ini`, 1.21 uses `mesh net`, and 2.7 power-cycles.

---

## Design gaps

Things the plans don't decide yet that will matter once code exists.

- [x] **DG1. Kernel pages come out of the shared pool**
  - Where: [D:75](mesh_virtual_machine_5ae0e741.plan.md:75), [D:417](mesh_virtual_machine_5ae0e741.plan.md:417), and the chaos memory-pressure run ([D:645](mesh_virtual_machine_5ae0e741.plan.md:645), I 9.3).
  - Issue: Stack slices, scratch, event payload pages, and lend copies all come from the shared pool. An applet that leaks until the pool is empty stops every applet, including a fenced frisbee, and `radio.rx` can't be delivered. I 9.3 is also vacuous, because frisbee doesn't allocate.
  - Recommendation: A fixed kernel reserve sized from the depth cap × stack size, plus scratch, plus a payload ring. Allocate it at boot, separate from applet reservations. Make it a guard test (G9). Rewrite 9.3 so frisbee also keeps receiving and throwing.
  - Decision: Adopted 2026-11-01, with one shared guest stack.
    - Per-applet stack slices are gone (they only kept callers' frames apart, which isolation no longer needs). One guest stack of `[kernel] stack_size` serves every applet; nested calls continue on it under `call_depth`. Overflow traps on an unmapped gap below. The manifest and `mesh.toml` lose `stack`.
    - (Revised under DG2: the payload ring became the inbox, `inbox_slots`.) The stack, the scratch arena (`scratch_size`), and the event payload ring (`payload_pages`) come from a fixed kernel reserve set aside at boot. A full ring makes `evt.emit` return `ERR_NO_MEMORY`, and device events are dropped and counted. Lend copies went away with grants.
    - G9 (4.12 and 9.3): with the pool empty, every applet still gets callbacks and events. 9.3 has frisbee playing while chaos holds the pool empty. 4.8 measures defaults and tests overflow.
    - Folded into D (overview, address map, sp, evt.emit, events, manifest, mvm-mem, sched, call, profile, packer, apps/mesh) and I (guards, 4.5, 4.8, 4.12, 5.5, 9.3, results log).

- [x] **DG2. Running out of fuel inside a nested call**
  - Where: [D:462](mesh_virtual_machine_5ae0e741.plan.md:462) and [D:476](mesh_virtual_machine_5ae0e741.plan.md:476).
  - Issue: Nested callee time comes from the caller's budget. If the callee is unloaded when that budget runs out, a caller that is nearly out of fuel can kill `sd`, `fat`, or `lora-modem-host` at will. If the call is just aborted, the provider is left halfway through a write.
  - Recommendation: The callee runs on its own per-call budget, which is charged back to the caller. Running out there is the callee's fault. If the caller can't afford the call, it fails with an error before the callee starts.
  - Decision: Replaced by a new event model, adopted 2026-11-01. Fuel is gone.
    - Applets are event handlers with no `main`. External events (radio, TCP, UART, USB, pins, timers, completions) wait in a kernel inbox. Interrupt handlers only wake native driver tasks, which copy data out of the device (the SX1262 buffer) into an inbox slot between VM slices.
    - Each source has a slot cap: radio RX over `inbox_radio` is dropped and counted; streams use backpressure; pins coalesce; timers take no slot; a completion's record is taken at request time, or the request fails with `ERR_NO_MEMORY`. The inbox replaces DG1's payload ring.
    - A turn is one external event and everything it sets off. The VM runs `instr_per_tick` instructions per main-loop pass, then yields; a turn resumes where it stopped. The next turn starts only when the current one ends.
    - `evt.emit` is synchronous and re-entrant, with pointers passed through, so filters edit the buffer in place. Consistency under re-entry is the applet author's job, with SDK helpers per language.
    - `instr_per_event` caps a whole turn. Over it, the kernel logs which event failed to finish and resets. No applet is blamed or charged.
    - Any trap or blown budget logs and resets the node. There is no unload at runtime, no `sys.exit`, and no `mesh stop`. A retained RAM syslog (`syslog_entries`, default 100) is flushed to `/boot/syslog` at boot, rotating at `syslog_max`; `mesh log --previous` reads it.
    - Icebox: profiling and hook tracing, unloading an applet without a reboot, and (from DG7) per-applet strikes.
    - Folded into D (trust model, overview, address map, events, call table, stopping, completions, middleware, providers, SDK, mvm-mem, mvm-code, interpreter, mvm-core inbox/sched/call/no-unload/boot/watchdog/syslog/profile/log, mvm-hal, sim, packer, apps/mesh, chaos, extra-fs, sd) and I (G5, M5, 2.4, 2.8, 4.8, 4.9 dropped, 5.4, 5.5, 5.5a, 5.5b, 5.6, 5.10, 5.11, 5.12, 5.15, 5.16, 5.18, 5.21, 5.23, 6.1, 6.3, 9.1, 9.2, 9.4, results log).

- [x] **DG3. Anyone can call provider verbs directly** **(trust model)**
  - Where: [D:347](mesh_virtual_machine_5ae0e741.plan.md:347), [D:370](mesh_virtual_machine_5ae0e741.plan.md:370), and [D:383](mesh_virtual_machine_5ae0e741.plan.md:383).
  - Issue: `sd.write`, `xflash.erase`, and `fat.open` are global names. Any applet can call them and skip the filesystem layer's paths, permissions, and mount grants.
  - Recommendation: Once a scope is mounted, only the kernel and granted applets may call it (`fat` gets `sd.*` through its `device` setting plus a grant). Alternatively, make provider scopes private from registration on.
  - Note after DG14: there are no longer permissions or mount grants to skip. What's left is a bug risk: an applet writing raw blocks under a mounted filesystem can corrupt it for everyone.
  - Decision: No change. Under the v1 trust model any applet may call any name, provider verbs included.

- [x] **DG4. `sys.caller` through `sys.next`**
  - Where: [D:292](mesh_virtual_machine_5ae0e741.plan.md:292) and [D:476](mesh_virtual_machine_5ae0e741.plan.md:476).
  - Issue: When a layer calls `sys.next`, the built-in at the bottom sees the layer as its caller. For example, `config.get` would read the layer's section, and filesystem permissions would be checked against the layer.
  - Recommendation: `sys.next` keeps the original caller. `sys.caller` returns the original caller, and a layer that needs its immediate caller gets it from another call.
  - Decision: Adopted 2026-11-01, without the immediate-caller call (nothing needs it yet).
    - `sys.next` keeps the original caller, so builtins at the bottom (`mem.*`, `fs.open`, sockets, `gpio.configure`) act for it; a layer calling another name directly acts as itself. Calls the kernel makes itself (`evt.emit`, `tty.exec`) have caller 0.
    - `config.get` no longer depends on the caller: it takes a dotted `section.key` and reads any section, with manifest defaults for loaded instances' sections. `sys.self` returns an applet's own instance name, and the SDK's `config::mine` builds its own keys. Instance names may not contain a dot.
    - Folded into D (overview, call table, applet settings, board values, caller identity, call) and I (5.5b, 5.8, 5.14, results log).

- [x] **DG5. `evt.emit` can fake kernel events** **(trust model)**
  - Where: [D:137](mesh_virtual_machine_5ae0e741.plan.md:137) and [D:208](mesh_virtual_machine_5ae0e741.plan.md:208).
  - Issue: Any applet can emit `radio.rx`, `net.up`, `mem.low`, and so on.
  - Recommendation: An event in a kernel scope may be emitted only by the applet providing that scope (as `lora-modem-host` does with `radio.*`), or with a grant.
  - Decision: No change. Under the v1 trust model any applet may emit any event except `sys.ready` and `sys.stopping`.

- [x] **DG6. `sys.register` can't say "provide" vs "layer"**
  - Where: [D:139](mesh_virtual_machine_5ae0e741.plan.md:139) and Middleware ([D:212](mesh_virtual_machine_5ae0e741.plan.md:212)).
  - Issue: A logger loaded before `lora-modem-host` silently becomes the `radio.tx` provider, and `lora-modem-host`'s registration then needs an override grant. After a provider unloads, it is unclear whether the next registration becomes the bottom or the top.
  - Recommendation: Separate calls, `sys.provide` and `sys.layer`. A layer on a name with no provider fails with `ERR_UNAVAILABLE`, or waits until a provider appears. A new provider always goes to the bottom.
  - Note after C19: override grants are gone, so the "needs an override grant" part no longer applies. The provide-versus-layer ambiguity remains.
  - Decision: No change (2026-11-01). The first registration provides and later ones layer, so roles follow `[applets]` load order, which the operator controls; D already says so. Unload is moot, since nothing unloads at runtime.

- [x] **DG7. Boot counter semantics and the safe mode**
  - Note: Safe mode itself was adopted under C6. What remains here is which resets the counter counts.
  - Where: [D:496](mesh_virtual_machine_5ae0e741.plan.md:496) and [D:492](mesh_virtual_machine_5ae0e741.plan.md:492).
  - Issue: The counter goes up on every boot, including power-on and brownout. A solar node browning out at dawn within `stable_s` would replace a good custom `boot.mhc`. Once the default bootstrapper is in place and still fails (for example, an applet keeps resetting the node), nothing further happens, so it loops.
  - Recommendation: Count only watchdog, fault, and lockup resets. After `boot_max_failures` with the default bootstrapper, boot in safe mode: the management service only, plus nothing from `[applets]` (or only instances marked `essential`).
  - Note after C18: replacing a loaded applet now resets the node. The bootstrapper clears the counter only after `stable_s` (default 60 s), so three edit-copy cycles within a minute reach `boot_max_failures` and drop into safe mode, contradicting D's "a replacement is not a failure". Software resets with reason `replace`, and `mesh reboot`, probably should not count. **Discuss first.**
  - Decision: Blame the applet, not the node; safe mode moves to the icebox. Adopted 2026-10-31.
    - A reset an applet causes is a strike against it: its own `sys.reset` (any reason), or a watchdog, fault, or lockup reset while the kernel's retained blame slot names it. At `[kernel] max_strikes` (default 3) the kernel refuses to load it (`ERR_DISABLED`) and the rest of the node boots.
    - Strikes clear after `[kernel] stable_s` (default 60) of uptime, for the replaced applet on a replace, and for all on `mesh reboot`. Power-on, brownout, the reset pin, and kernel hangs with no applet running add none.
    - No safe mode, no node-level counter, no `sys.bootOk`, no `mesh reboot --safe`. A reset loop nobody can be blamed for keeps looping. A trapping bootstrapper is unloaded and logged, and an unmountable `/boot` loads nothing; the management service answers in both.
    - Icebox: safe mode, and remote recovery over LoRa with signed commands from `[admin]` keys and a raw-flash recovery page.
    - Folded into D (trust and watchdog bullets, call table, boot, watchdog, profile, mvm-hal, host runtime, apps/mesh, applets/boot, errors) and I (5.10, 5.11, 9.4, results log).
    - Revised 2026-11-01 under DG2: per-applet strikes and disabling moved to the icebox too, because a bedrock applet (a filesystem driver, say) that strikes out on a rare code path could brick the node. Any trap, blown budget, or watchdog reset just logs and reboots; a node that fails on every boot keeps rebooting until fixed over USB.

- [x] **DG8. The radio is one shared configuration**
  - Where: [D:157](mesh_virtual_machine_5ae0e741.plan.md:157), I 1.13 (`mesh tty`), and I 6.1 ("`mesh tty` keeps working through it").
  - Issue: Two applets (or `mesh tty`) calling `radio.config` with different frequency or spreading factor have no defined winner. It is also undefined when receive is on: always after config, or duty-cycled for power.
  - Recommendation: Open for discussion. Options:
    - Last writer wins, logged.
    - A per-applet config that the radio service switches between on transmit.
    - One owner instance named in `[board]` or `[grants]`.
  - Decision: Last writer wins, adopted 2026-11-01. `radio.config` applies at once, is logged, and emits `radio.configured` (settings and initiator). Arbitration, if a node needs it, is an applet layering on `radio.config`. Receive is continuous between transmits once configured; duty-cycled receive is in the icebox. The shell's `lora` mode is just another writer. Folded into D (`radio.config`, event table, radio service, `mesh tty`) and I (6.1).

- [x] **DG9. USB and BLE are shared with the management service**
  - Where: [D:332](mesh_virtual_machine_5ae0e741.plan.md:332) (USB is kernel-owned), [D:193](mesh_virtual_machine_5ae0e741.plan.md:193) (`usb.write` for applets), [D:506](mesh_virtual_machine_5ae0e741.plan.md:506), and `lora-modem-mcu` over USB or BLE.
  - Issue: One CDC port, one USB-Serial-JTAG port on ESP32-S3, and one BLE link carry both management frames and an applet's stream.
  - Recommendation: Add a channel id to the COBS framing. Channel 0 is management, and applets open a numbered channel. The kernel owns the device, and applets own channels.
  - Note after the tty decision (2026-11-01): management is now a text shell over USB and BLE, not COBS frames, so the channel id no longer fits there. The open question is how an applet's binary stream (`lora-modem-mcu`'s COBS frames) shares a port with the text shell, for example a shell command that hands the port to an applet until it is released.
  - Decision: One protocol, adopted 2026-11-01. The modem is a shell extension: `lora-modem-mcu` layers on `tty.exec` to add `modem config|tx|ping|stats`, writes `!modem rx|txdone|hello` lines, and answers `err busy` instead of using credits. USB and BLE carry only shell lines, so applets lost `usb.write`, `usb.rx`, `usb.up`, and `usb.down`; an applet talks to the host through shell commands or `tty.write`. On Darwin, `lora-modem-host` reaches a USB modem through a `uart` name mapped to the serial port. Folded into D (call and event tables, taps, mvm-hal `usb`, frisbee, the three `lora-modem` sections) and I (8.1, 8.2, 8.3, 8.4, 8.5, 8.6).

- [x] **DG10. Remote administration over LoRa**
  - Where: [D:493](mesh_virtual_machine_5ae0e741.plan.md:493) ("a broken bootstrapper pushed over LoRa…") and [D:46](mesh_virtual_machine_5ae0e741.plan.md:46) ("applet rollback lives in the kernel's own store").
  - Issue: Applets have no install, config-write, restart, or rollback calls, and management runs only over USB and BLE. Rollback is never defined.
  - Recommendation: Declare remote install and rollback out of v1, and remove both sentences. Alternatively, add grant-gated `sys.install`, `config.set`, and `sys.restart` later.
  - Decision: Settled by later decisions (2026-11-01). Applets already write any file and call `sys.reboot`, so install, config writes, and restarts need no new calls. Rollback is copying the old `.mhc` back. Remote administration over LoRa is an applet attaching a shell channel with `tty.attach` (MeshCore-style), authenticating senders itself; signed recovery over LoRa in safe mode is in the icebox. The kernel-store rollback sentence is gone from D.

- [x] **DG11. Missing host calls**
  - Where: The kernel call table ([D:134](mesh_virtual_machine_5ae0e741.plan.md:134)).
  - Issue:
    - No `time.now`, and `time_us` is a `u32` that wraps every 71 minutes.
    - No `time.clearTimeout` or `evt.off`.
    - `adc.read` takes an id nothing hands out.
    - `mem.malloc` has no alignment argument. `GlobalAlloc` with alignment over 8 and C's `posix_memalign`/`aligned_alloc` need one for G8.
    - `fs.ls` with a "callback or buffer" form overlaps the providers' `opendir`/`readdir`.
  - Recommendation:
    - Add `time.now` (64-bit µs as two words), `time.clearTimeout`, `evt.off`, `adc.open(spec)`, and `mem.malloc(size, align)`.
    - Replace `fs.ls` with `fs.opendir`, `fs.readdir`, and `fs.closedir`.
  - Decision: Adopted 2026-11-01 under the three-layer model (kernel, bootstrapper, applets). Kernel: `time.now` (64-bit µs), one `time.alarm(deadline, callback, ctx)`, `adc.open`, `mem.malloc(size, align)`, and `fs.opendir`/`readdir`/`closedir` (listing is an SDK helper). Bootstrapper: `time.setTimeout`/`clearTimeout` multiplexed over the alarm, and `evt.off`. Also moved to the bootstrapper: `sys.ready` (from `sys.load` completions) and shell niceties. Event timestamps are 64-bit.

- [x] **DG12. No atomics on `riscv32imc`**
  - Where: The instruction set ([D:37](mesh_virtual_machine_5ae0e741.plan.md:37)), I 2.3 (atomics trap as illegal), and G8.
  - Issue: Without compare-and-swap, `alloc::sync::Arc`, `once_cell`, `spin`, and similar crates don't compile. G8's "unmodified crates" will hit this.
  - Recommendation: Use RV32IMAC (`riscv32imac-unknown-none-elf`, also tier 2). The A extension is a few lines in a single-core interpreter.
  - Decision: Adopted 2026-11-01. RV32IMAC throughout. Atomics are uninterruptible on the single-core interpreter; an `LR.W` reservation survives a slice boundary and is cleared at the end of the turn; misaligned atomics trap. 2.3 tests `rv32ua`, a split `LR`/`SC`, and a misaligned atomic; 5.24 adds `Arc` and `OnceLock` to G8. Folded into D (instruction set, interpreter, frisbee, diagram) and I (M2 map, 0.3, 2.2, 2.3, 5.24).

- [x] **DG13. Grants and capabilities are undefined** **(trust model)**
  - Where: [D:63](mesh_virtual_machine_5ae0e741.plan.md:63), [D:263](mesh_virtual_machine_5ae0e741.plan.md:263), [D:500](mesh_virtual_machine_5ae0e741.plan.md:500), and I 3.4.
  - Issue:
    - There is no `[grants]` syntax and no list of capability names (reset, load, override, mount, net, …).
    - The default policy when `[grants]` is missing is undefined.
    - What manifest `kind` (`user` or `system`) does is undefined.
    - "Per-name minimum versions" makes no sense, because names have no versions. It probably means per applet.
  - Recommendation: Define these before I 3.4. Proposal:
    - `[grants]` lines keyed by instance, for example `frisbee.override = radio.tx`, `chaos.reset = true`, `sd.mount = rw:/sd`, `frisbee.hash = …`.
    - Default policy: a manifest's ordinary requests are granted, and privileged capabilities (`reset`, `load`, `override`) are never granted by default.
    - Drop `kind`.
  - Decision: No longer applies. Grants and capabilities moved to the icebox (C19). The manifest has no capabilities field, and `config.ini` has no `[grants]` section.

- [x] **DG14. `/boot` access, the code directory, and `/app/<id>`** **(trust model)**
  - Where: [D:498](mesh_virtual_machine_5ae0e741.plan.md:498), [D:499](mesh_virtual_machine_5ae0e741.plan.md:499), [D:53](mesh_virtual_machine_5ae0e741.plan.md:53), and [D:599](mesh_virtual_machine_5ae0e741.plan.md:599).
  - Issue:
    - The bootstrapper must read `config.ini`, which also holds the `[net]` Wi-Fi password.
    - The "code directory" is never named, and its install registry doesn't fit loading applets from arbitrary paths on other mounts.
    - It is unclear whether `/app/<id>` is the instance (survives upgrades) or the hash (doesn't).
    - The quota size and where it is set are undefined.
  - Recommendation:
    - The bootstrapper reads `[applets]` through a kernel call (`config.applets`), so no applet reads `/boot/config.ini` directly.
    - `[net]` is kernel-only.
    - ~~Install registers any `.mhc` by hash wherever it lives.~~ Superseded: there is no install step at all (see the decision below).
    - `/app/<instance>/`, with the quota in the instance section.
  - Decision (partial, the code directory only): Dropped, with the kernel `install` routine and its registry. Installing an applet is copying its `.mhc` anywhere and listing it in `[applets]`; a user who installs an applet trusts it. The loader hashes every load, so hash pinning and `mesh verify` still work. The VFS stops any applet whose code file is overwritten, renamed over, or deleted (`sys.stopping` with reason `replace`) before the write lands. Folded into D (locked VFS bullet, unload, the "Files in use" bullet that replaces `install`, crypto, packer, `mesh verify`) and I 5.11. Then the rest: the filesystem has an old-school DOS feel. Every applet and the user may read and write any path on any mount, including `/boot/config.ini` and its `[net]` section, so the bootstrapper reads `config.ini` directly. Mount grants, `/app/<id>/` directories, and quotas are dropped; the SDK suggests `/data/<instance>/` by convention. `..` resolves lexically instead of being rejected. Mounting over `/boot` is still refused. The only check on code is the loader's content hash at load. Folded into D (locked VFS and capability bullets, manifest, VFS rules, `vfs`) and I 5.7.

- [x] **DG15. Execution model and preemption**
  - Where: [D:49](mesh_virtual_machine_5ae0e741.plan.md:49), [D:473](mesh_virtual_machine_5ae0e741.plan.md:473), and [D:494](mesh_virtual_machine_5ae0e741.plan.md:494).
  - Issue: The plans don't say that the radio, USB, and watchdog feeding preempt the interpreter. They also don't bound worst-case callback time: fuel × the slowest instruction rate, plus page fills, plus synchronous filesystem calls, all of which must stay well under `wdt_ms`.
  - Recommendation:
    - The interpreter runs at thread priority. The radio service and USB run in higher-priority interrupt executors.
    - The default fuel budget is derived from the 2.8 numbers so that one callback takes at most about 1/4 of `wdt_ms`.
    - Add this to D and a check to I 2.8.
  - Decision: Adopted 2026-11-01. Driver tasks run in an interrupt-priority executor and only copy into the inbox; the main loop (VM slices, shell, filesystem, watchdog) runs at thread priority. Because neither chip can fetch from flash during an erase, the copy-out path uses SoftDevice-scheduled partial erases on nRF and IRAM on ESP32-S3; 1.16 proves no frame is lost. Slices stay counted in instructions (`instr_per_tick`), sized from the 2.8 numbers so a slice plus the slowest native call stays far below `wdt_ms` (default 8 s). Time-based slices and watchdog-deadline yielding were considered and dropped: the watchdog is a multi-second safety net, not a loop rhythm. `instr_per_event` still catches runaway turns.

- [x] **DG16. Payload sizes and buffer lending** **(trust model)**
  - Trust-model note: The plans copy partial-page buffers "so a callee never sees the caller's neighbouring data". That is a confidentiality rule. Under the trust model, a read-only buffer could be lent in place even when it shares a page with other data, and only writable buffers would still need care, because a callee could scribble on the neighbours.
  - Where: [D:208](mesh_virtual_machine_5ae0e741.plan.md:208), [D:384](mesh_virtual_machine_5ae0e741.plan.md:384), and [D:416](mesh_virtual_machine_5ae0e741.plan.md:416).
  - Issue:
    - An event payload is "one kernel page", so 256 bytes or less. TCP, UART, and USB data can be larger.
    - Big copied arguments (a 4 KiB write from statics) need many kernel pages at once.
    - Code and statics pointers (`0x00`/`0x01`) can't be lent in place to another applet, because they alias the callee's own memory. String constants are the common case.
  - Recommendation:
    - Payloads may span several kernel pages, with a stated maximum.
    - Copies are chunked through a bounded number of kernel pages.
    - `0x00`/`0x01` buffers are always copied, never lent in place.
  - Note after C19: applet isolation is gone, so cross-applet calls and providers pass pointers straight through with no lending or copying. Only `evt.emit` still copies its payload into a kernel page, because listeners run later. What is left here is payload size limits.
  - Decision: Adopted 2026-11-01.
    - Payload sizes: an inbox event carries at most one 256-byte page; a LoRa frame fits, and stream data (TCP, UART, BLE) arrives as several events of up to a page each. `evt.emit` passes pointers through, so there is no copy limit.
    - Aliasing of code and statics: replaced by one shared address space. Each applet gets an image partition (code and rodata at `0x01_0000`, statics at `0x80_0000`) and a heap partition. Applets build with `medany` (PC-relative), and the packer lists absolute data words, which the loader relocates in statics at load and in code pages at each fill. Partitions `0x00` and `0x01` are no longer per-applet views. Callback references need only the function address, whose partition names the instance.
    - Folded into D (instruction set, address map, SDK linker script, packer, mvm-mem partition table, mvm-code `FileCode`, load, callback references) and I (2.1b, 2.6b, 2.7, 5.5).

- [x] **DG17. Code-cache pinning across `ecall`, and slot count**
  - Where: [D:448](mesh_virtual_machine_5ae0e741.plan.md:448) and [D:449](mesh_virtual_machine_5ae0e741.plan.md:449).
  - Issue: "The PC slot is pinned for the instruction." An `ecall` that makes a nested call is one instruction, so a call chain deeper than the slot count would deadlock. Provider chains (app → `fat` → `sd`) span three code windows, so 2 slots would thrash.
  - Recommendation: Release the pin at `ecall`, and refetch on return through the tag check. Start with 4 slots and let I 9.1 measure.
  - Decision: Adopted 2026-11-01. The pin is released at `ecall` and the caller's page refetched on return. The slot count is `code_cache_kb` × 2, from the board preset with family defaults (4 slots on nRF52840), overridable with `[kernel] code_cache_kb`. 9.1 runs a provider chain deeper than the smallest cache.

- [x] **DG18. PSRAM and page-table size**
  - Where: I 0.5 and [D:414](mesh_virtual_machine_5ae0e741.plan.md:414).
  - Issue: An 8 MB page pool in PSRAM is 32k pages, which at about 8 bytes per page makes a table and index of about 256 KiB in internal RAM.
  - Recommendation: Add this to 0.5. Options:
    - PSRAM is not page pool in v1.
    - Larger pages for a PSRAM pool.
    - The table itself in PSRAM.
  - Decision: Option 1, adopted 2026-11-01. PSRAM is not page pool in v1. The page pool, inbox, and stack stay in internal SRAM on every board; PSRAM holds only the code-page cache. A PSRAM heap is in the icebox. Folded into D (`mvm-mem`) and I (0.5, open questions).

- [x] **DG19. Board power-enable before the radio probe**
  - Where: Board semantics ([D:305](mesh_virtual_machine_5ae0e741.plan.md:305), [D:313](mesh_virtual_machine_5ae0e741.plan.md:313)) and I 0.1.
  - Issue: I believe the T-Deck Plus keeps peripherals, including the radio, behind a power-enable GPIO. The kernel probes the radio before any applet runs, and `power.*` names are only applet conventions.
  - Recommendation: Confirm in 0.1. Add a kernel-used `radio.power` key (pin and level) or a `board.power_on` list the kernel drives before probing.
  - Decision: Adopted 2026-11-01, with board bring-up moved to the bootstrapper. The kernel knows how to drive chips and offers `radio.attach` (wiring, probe, `max_dbm` clamp), `clock.lfxo`, and `power.dcdc`; it never acts on `[board]` by itself. The bootstrapper's board module reads `[board]`, drives `[board] power_on` pins with `gpio`, switches clocks and regulators, and attaches the radio, so board support updates without a reflash. Power pins are not protected (kernel-owned board pins are in the icebox), and other radio chips such as the LR1121 are icebox and a different kernel image. 0.1 records power-enable pins from vendor code, Meshtastic variants, and schematics; 1.12 probes the T-Deck Plus with the pin low and high. Folded into D (board semantics, call table, names, radio service, boot, applets/boot, presets) and I (0.1, 1.12, 5.8, 5.10, 6.1).

- [x] **DG20. SD busy-waits burn fuel**
  - Where: `applets/sd` ([D:692](mesh_virtual_machine_5ae0e741.plan.md:692)) and [D:388](mesh_virtual_machine_5ae0e741.plan.md:388).
  - Issue: An SD card can stay busy for hundreds of milliseconds. Polling it with `spi.transfer` burns fuel, which is charged up the chain. Nothing on SPI plays the role `qspi.waitReady` plays for QSPI.
  - Recommendation: Add `spi.waitReady(bus, fill byte, ready byte, timeout)`, a native poll that charges no fuel.
  - Note after DG2: fuel is gone, but polling still counts toward the turn's `instr_per_event`, so the issue stands in that form.
  - Decision: Adopted 2026-11-01: accept latency, no closures. One intrinsic, `sys.sleep(us)`, pauses the current turn in place (like a slice end): the main loop keeps running, sleeping is not counted toward `instr_per_event`, but no other turn starts. Busy checks are chip-specific code in the driver applet (SD reads until `0xFF` with CS released between polls; flash reads its status register), so `qspi.waitReady` and the proposed `spi.waitReady` are gone. 5.18 measures SD busy times and the worst event latency. Non-blocking I/O (async native drivers, or parked turns with their own stacks) is in the icebox, partly over doubts about how it fits the event bus. Folded into D (call table, provider rules, extra-fs, sd) and I (5.4, 5.15, 5.18).

- [x] **DG21. `config.ini` robustness and kernel writes**
  - Where: [D:500](mesh_virtual_machine_5ae0e741.plan.md:500) ("unknown sections are rejected") and [D:490](mesh_virtual_machine_5ae0e741.plan.md:490) (`boot_default_hash` in `[kernel]`).
  - Issue: If one leftover section for a removed instance rejects the whole file, the node boots with the radio off. Also, the kernel rewriting a file the user edits risks clobbering their comments and formatting.
  - Recommendation:
    - Unknown sections are logged and ignored, and only a malformed reserved section is rejected.
    - Keep `boot_default_hash` in a kernel-owned file such as `/boot/.kernel`.
  - Decision: Adopted 2026-11-01, with a new config API. Kernel meshops `config.getStr`, `getI32`, `getBool` (dotted name, default at the call site, invalid or missing returns the default) and `config.setStr`, `setI32`, `setBool` (rewrite one line in place, keeping comments). The bootstrapper or an applet may layer on or replace them. The file is read leniently: unknown sections are data, malformed lines are logged and skipped, bad kernel values fall back to defaults, and the file is never rejected whole. The kernel writes `config.ini` only on a `set` call; `boot_default_hash` lives in `/boot/.kernel`. Manifest settings defaults are gone, since every read carries its default. Folded into D (layers, overview, call table, manifest, applet settings, board values, boot, profile, packer, apps/mesh, applets/boot, extra-fs, sd, fat) and I (3.4, 3.x manifest, 5.8, 5.10, 5.14).

- [x] **DG22. `mem.low` re-arming**
  - Where: [D:419](mesh_virtual_machine_5ae0e741.plan.md:419) ("emits `mem.low` once").
  - Issue: "Once" ever, or once per crossing? The `[kernel]` key name is also not given.
  - Recommendation: Fire on each downward crossing, and re-arm when free pages rise above the threshold plus a hysteresis margin. Keys: `[kernel] mem_low_pages` and `mem_low_hysteresis`.
  - Decision: Adopted 2026-11-01 with two marks instead of a margin, as percentages of the shared pool so one setting fits every board. `mem.low` fires once on falling below `[kernel] mem_low_pct` (default 10); a new `mem.ok` event fires on rising above `mem_ok_pct` (default 20), which re-arms `mem.low`. Bad marks fall back to defaults. The heap allocator stays in the kernel. Folded into D (event table, `mvm-mem` pressure, profile) and I (4.8).

- [x] **DG23. Log format ids and the string table**
  - Where: [D:65](mesh_virtual_machine_5ae0e741.plan.md:65) and [D:264](mesh_virtual_machine_5ae0e741.plan.md:264).
  - Issue: Nothing says how format ids avoid colliding across applets, or where `mesh log` finds the strings.
  - Recommendation: Ids are per applet. The packer writes a sidecar string table named after the `.mhc` content hash, and log records carry the applet's content hash.
  - Decision: Moot, adopted 2026-11-01: format ids are gone. Formatting is a native kernel meshop, `fmt.sprintf`, overridable by layering, and `log.printf(fmt, args…)` formats straight into the syslog. Log lines are plain text, the syslog is an 8 KiB text ring on every device, and C's `printf` family links to the meshops. Alongside it, the instruction set became RV32IMAFC (`ilp32f`) so single-precision floats run on each chip's FPU and `%f` is supported. Folded into D (instruction set, logging, call table, interpreter, syslog, log, fmt, C runtime, apps/mesh) and I (1.3, 2.1b, 2.3, 2.6a, 2.7, 2.8, 5.3, 5.6, 5.24).

- [x] **DG24. Minor wording and API details**
  - Issue:
    - "A flipped bit becomes a trap" ([D:498](mesh_virtual_machine_5ae0e741.plan.md:498)) should say "at worst misbehaves inside its sandbox".
    - The `crypto.aes` mode and key sizes are unspecified.
    - The `time.setTimeout(callback, msec, ctx)` argument order differs from the other calls' `…, callback, ctx`.
    - `evt.on` has no `ctx`.
    - The bootstrapper's instance name (`boot`) is implied but not stated. (Stated under C14.)
    - `sys.bootOk` is callable by anyone.
    - The CLI target syntax mixes `<board>`, `usb:`, and `ble:<name>`.
  - Recommendation: Fix each in place:
    - `time.setTimeout(msec, callback, ctx)`;
    - `evt.on(name, callback, ctx)`;
    - `crypto.aes` as AES-128/256 in CTR and ECB modes;
    - `sys.bootOk` bootstrapper-only (moot: `sys.bootOk` was removed under DG7);
    - one target syntax (`usb:SERIAL`, `ble:NAME`, `host:NODE`; settled under C12).
  - Decision: Done 2026-11-01. `time.setTimeout(msec, callback, ctx)` and `evt.on(name, callback, ctx)` throughout D. `crypto.aes` takes a mode (ECB or CTR), a 16- or 32-byte key (AES-128 or AES-256), and a CTR IV. The flipped-bit sentence, `sys.bootOk`, the bootstrapper's name, and the target syntax were already settled or removed by earlier items.

- [x] **DG25. Which key signs test, CI, and host-node applets**
  - Where: D's packer ("CI signs with a key held as a secret"); I 2.6e, 5.9, 7.4, 8.3.
  - Issue: Pull requests from forks get no secrets. `mesh run` scenarios and bench `config.ini` files need `[trust]`, but nothing provisions it. Test applets built locally and in CI carry different signers.
  - Recommendation: Commit a test-only key, trusted by default in host scenarios, simulator runs, and bench presets. `mesh run` trusts the local developer key automatically. The CI secret is for release builds only.
  - Decision: No longer applies. Applet signing moved to the icebox (see C17).

---

- [x] **DG26. The minimal set of kernel intrinsics**
  - Where: The design plan's kernel call table and event table.
  - Issue: The kernel should offer only what applets cannot build themselves; everything else belongs in applets that register names as kernel extensions. The table has not been audited against that rule.
  - Rule proposed: a name stays in the kernel only if it needs hardware or a native stack, kernel state, native speed or timing that the interpreter cannot meet, or is needed to boot and load applets.
  - Candidates to move out: `dns.resolve` (a DNS client is a protocol over `udp.*`), applet-to-applet `evt.emit` (an event-bus applet over `sys.register` and calls; the kernel keeps `evt.on` for its own events), `config.get` for instance sections (an SDK library reading `config.ini`), `crypto.aes` (pure computation, unless hardware AES or speed justifies it), `tap.rx`/`tap.tx` (sends are observable by layering; receives are not), and `mem.scratch` (an SDK arena over `mem.malloc`).
  - Decision: Adopted 2026-11-01. The rule is now the north star at the top of D's locked decisions.
    - Events: the kernel's whole event job is the inbox, then `evt.emit`. The bootstrapper must provide `evt.on`, `evt.emit`, and `evt.has` (the SDK bus module, linked into `applets/boot`); the kernel checks them when the bootstrapper's `init` returns and keeps no listener lists. It delivers its own events by calling `evt.emit`. `sys.complete` became synchronous so the bus can call subscribers.
    - A bootstrapper that traps or misses the contract makes the kernel record its hash and reset with `boot_failed`; while `boot.mhc` has that hash, the kernel runs the embedded default (which still loads `[applets]`). This is not the iceboxed safe mode.
    - `sys.reset` became `sys.reboot(reason)`: when the turn ends, every subscriber gets one `sys.rebooting(reason, initiator)`, then the node resets. `mesh reboot` and a replaced file do the same; traps, budgets, and the watchdog do not. Replacement freezes the VM after the notice, before bytes change.
    - `mem.scratch` and the kernel scratch arena are gone; applets use statics or the SDK's bump arena.
    - Stay in the kernel: `dns.resolve` (comes with the TCP/IP stack), `config.get` (the kernel parses INI anyway), `crypto.aes`, and `tap.*`.
    - Revised 2026-11-01: callbacks became plain references (instance id, function address, `ctx`); `sys.complete` became `sys.resolve`, and `sys.release` and the kernel's handle table are gone.
    - Folded into D (north star, trust, overview, address map, events, call table, event bus, event table, completion handles, SDK, mvm-mem, sched, hooks, no-unload, boot, watchdog, Files in use, profile, mvm-hal, sim, apps/mesh, frisbee, chaos, applets/boot) and I (G5, G9, 4.5, 4.8, 5.4, 5.5a, 5.5c new, 5.6, 5.10, 5.11, 6.3, 9.4, results log).

## Implementation-plan sequencing


- [x] **PS1. M1 and M2 are not fully parallel**
  - Where: I milestone map ("M1 and M2 can run in parallel").
  - Issue: 2.5 needs the M0 on-device runner, and 2.7–2.9 need flash, littlefs, and `mesh cp` from 1.6–1.8.
  - Recommendation: Say that 2.1–2.4 and 2.6 run in parallel with M1 (2.1 is now the build image, and 2.1b the target), and that 2.5 and 2.7–2.9 wait for 1.8.
  - Decision: Superseded 2026-11-01: M1 and M2 are now sequential (no interpreter work before M1's exit check). The milestone map says so; only the build image and target setup (2.1, 2.1b) may be prepared during M1.

- [x] **PS2. Step 2.9 needs the memory manager and the kernel**
  - Where: I 2.9.
  - Issue: It runs two applets at once and checks the one-page image in the mapped count, but the M2 stub loader runs one applet and there is no `mvm-mem` yet.
  - Recommendation: Keep "a 200 KiB applet runs paged from flash" and the 512/513 boundary in 2.9. Move the two-at-once run and the mapped-page accounting to 5.1, which already reports G6 and G7.
  - Decision: Adopted 2026-11-01. 2.9 runs the 200 KiB applet and the 512/513-byte pair one at a time under the stub loader; two big applets at once and the one-page image in the mapped count move to 5.1.

- [x] **PS3. Step 5.23 uses the radio before 6.1**
  - Where: I 5.23 ("an `async` radio send-and-wait applet on an nRF board").
  - Recommendation: Use a timer or `test.later` round trip on nRF in 5.23, and add the async radio check to 6.1.
  - Decision: Adopted 2026-11-01. 5.23 uses an `async` timer and `test.later` round trip on nRF; 6.1 adds the `async` radio send-and-wait check.

- [x] **PS4. Directories missing from the design workspace**
  - Where: The workspace tree in D. I uses `applets/tests/`, `tests/guards/`, `scenarios/`, and `xtask`.
  - Recommendation: Add them to the tree.
  - Decision: Adopted 2026-11-01. D's workspace tree adds `applets/lora`, `applets/tests/`, `tests/guards/`, `scenarios/`, and `xtask/`.

- [x] **PS5. Linux CI vs a Darwin-only CLI**
  - Where: I 0.3 and `apps/mesh` (CoreBluetooth, IOKit).
  - Issue: CI likely runs on Linux, but `mesh run` and the simulator must build there.
  - Recommendation: Put the Darwin USB and BLE backends behind `cfg(target_os = "macos")`, and keep `mvm-sim` and `mesh run` portable. CI builds both.
  - Note after C12: `mesh build` must also run on x86 Linux (I 2.1, 2.6e), and `mesh mount` describes Linux behaviour, yet D still calls `mesh` "the Darwin CLI". Decide whether `mesh` is a supported user tool on Linux or only `build` and `run` are, and reword D to match.
  - Decision: `mesh` is a full user tool on Linux too, adopted 2026-11-01. Only the USB serial (IOKit on macOS, udev and `/dev/ttyACM*` on Linux) and BLE (CoreBluetooth, BlueZ) backends differ, behind `cfg(target_os)`; `mesh run`, the simulator, `build`, and `mount` are portable. CI builds and host-tests `mesh` on both. D's `apps/mesh` no longer says "Darwin CLI"; "Darwin" means the desktop host node.

- [x] **PS6. No build environment for the ESP32-S3 kernel**
  - Where: I 0.2 (one toolchain pinned in `rust-toolchain.toml`), 0.3 (an ESP32-S3 CI job), 0.6.
  - Issue: The ESP32-S3 is not a RISC-V chip, and the official Rust releases can't build for it. It needs Espressif's own Rust toolchain, installed with `espup`, which one `rust-toolchain.toml` can't pin next to the stable toolchain. The nRF side also needs its flashing tools (UF2 conversion, `probe-rs`). This is the applet build-image problem again, for kernel firmware.
  - Recommendation: A firmware build image (or one image with both toolchains), pinned by digest like the applet image, used by `cargo xtask bench` and CI. Settle it in M0, before 0.3.
  - Decision: Adopted 2026-11-01. New step 0.2a: a firmware build image pinned by digest (Espressif's Xtensa toolchain via a pinned `espup`, stable Rust with `thumbv7em-none-eabihf`, the UF2 converter) builds both kernels and their `hal-test` builds. CI (0.3) and `cargo xtask bench` (0.5) use it. Flashing stays on the host with pinned `espflash` and `probe-rs`, since Docker on macOS cannot reach USB. D's workspace section says so.

- [x] **PS7. M0 flashes images before any exist**
  - Where: I 0.4 (a BLE advertisement from each nRF board) and 0.5 (the same image flashed to every ESP32-S3 board).
  - Issue: The first image per family arrives in 0.6 (blink), and the bench script in 0.7.
  - Recommendation: Move 0.6 and 0.7 before 0.4 and 0.5, and let 0.4 build on blink with a BLE advertisement added.
  - Decision: Adopted 2026-11-01. M0 is reordered: 0.4 blink (nRF linked at the installed SoftDevice's address from 0.1), 0.5 the bench script, 0.6 the SoftDevice decision built on blink, 0.7 the ESP32-S3 layout built on blink. References renumbered.

- [x] **PS8. The simulator node is used long before it can run**
  - Where: The `SIM` column of the M1 matrix (`mesh ping`, `mesh cp` against a simulated node in 1.5 and 1.8, and two simulated nodes in 1.13). The virtual management stream, the multi-node runner, and `mesh run` arrive only in 7.1 and 7.4.
  - Recommendation: A step early in M1 for a minimal simulated node: one or two nodes in one process, a virtual management stream, and a way for `mesh` to reach them (`host:<node>`). 7.1 and 7.4 then add virtual time, scenarios, and the full medium.
  - Note: I 1.8a also checks `mesh mount` against a host node (see PS13).
  - Decision: Adopted 2026-11-01. New step 1.0a, right after the USB ping: one or two simulated nodes in one process running the native kernel code, each with a host-directory `/boot`, a virtual shell stream, and an ideal radio, reached as `--node host:<node>`. Every later M1 step fills its `SIM` cell against it. 7.1 and 7.4 extend it with virtual time, scenarios, and `mesh run`.

- [x] **PS9. CLI commands used before they are built**
  - Where: I 5.1 (`mesh ps`, `mesh guards`), 5.6 (`mesh stop`), 5.10 (`mesh cfg`, `mesh reboot`). 5.11 builds `cfg`, `ps`, and `stop` later, and no step builds `mesh guards` or `mesh reboot`.
  - Recommendation: Build each command in the first step that uses it, with 5.11 keeping the rest (`names` already lands in 5.5b this way).
  - Decision: Settled with C12. Each command is built in the first step that uses it, recorded in the "Built in" column of D's command table: `ps` minimal in 5.1, `log` in 5.3, `stop` in 5.6, `trust` in 5.9, `cfg` and `reboot` in 5.10, the full `ps` and `verify` in 5.11, and `pins` in 5.13. `guards` is no longer a command.

- [x] **PS10. 5.6's stop listener writes a file before fs exists**
  - Where: I 5.6 (a `sys.stopping` listener that writes a file) and 5.7 (the `fs.*` builtins).
  - Recommendation: Swap 5.6 and 5.7, or have the 5.6 listener log and set a retained word instead, with the file write rechecked in 5.7.
  - Decision: Moot after DG2 (2026-11-01). 5.6 is now traps, resets, and the syslog; its `sys.stopping` listener only logs.

- [x] **PS11. 5.25 taps radio and BLE before they exist**
  - Where: I 5.25. The radio builtins arrive in 6.1, and the BLE builtins for applets in 8.4.
  - Recommendation: 5.25 covers UART, TCP, and UDP (and the management USB stream if it is tapped). The radio tap check moves to 6.5, where the stats applet can use `tap.rx`, and the BLE tap check moves to 8.4.
  - Decision: Adopted 2026-11-01. 5.25 taps UART, the shell, TCP, and UDP; radio taps are checked in 6.5 (which also drops a stale isolation claim) and BLE taps in 8.4.

- [x] **PS12. `sys.reset` and `sys.resetReason` are never built**
  - Where: The design plan's kernel call table. I 9.4 uses both, and 5.10 expects `sys.resetReason` to report safe mode.
  - Recommendation: Build both in 5.10, next to the blame slot and strike table they share retained RAM with (after DG7; safe mode is gone), with a check that the reason word survives a reset on both chips.
  - Decision: Settled 2026-11-01. 5.10 builds both, with the retained reason word, and checks a known word and the kernel's `trap`, `budget`, `replace`, and `reboot` across resets. The syslog ring's survival is checked in 5.6.

- [x] **PS13. More steps that use things before they are built**
  - Where:
    - I 5.9 loads an applet "after `mesh trust add`", but config changes apply at the next boot, and `mesh reboot` arrives in 5.10.
    - I 1.8a repeats its check "against a host node", but host nodes arrive in 7.4 (extends PS8).
  - Recommendation: Build `mesh reboot` in 5.9 (or have 5.9 power-cycle the board). Include 1.8a in the PS8 fix.
  - Decision: Settled 2026-11-01. The first point is moot: `mesh trust` and signing are gone, and 5.9 no longer refers to them. The second (1.8a's host-node check) is covered by 1.0a from PS8.

---

## Second review (2026-11-01)

A fresh review of both plans after the day's decisions. About fifty findings were stale text or sequencing slips and were fixed directly (see the implementation plan's results log). These four need a decision.

- [x] **R1. The kernel still reads some `[board]` facts at boot**
  - Where: D `mvm-code` cache (`[board] code_cache_kb`), presets (`psram` reserves pins), `mvm-hal` safe defaults (MCU check and reserved pins against `[board]`), `apps/mvm-nrf` (FICR check against `[board]`).
  - Issue: Since DG19 the kernel "never acts on `[board]` by itself", yet it needs the code-cache size and the reserved-pin facts before or without the bootstrapper.
  - Options: name these as the only `[board]` facts the kernel reads at boot; or move them to `[kernel]` (`code_cache_kb`) and to chip facts the kernel knows itself (reserved pins from the part, FICR), with the bootstrapper passing nothing.
  - Decision: Move them out, adopted 2026-11-01. The kernel never reads `[board]`. It detects RAM and PSRAM itself and sizes the code cache from them (`[kernel] code_cache_kb` overrides); it learns the part from nRF FICR or the ESP32-S3 eFuse package and reserves chip pins (including in-package octal PSRAM) on its own; `sys.version` reports the detected part for a bootstrapper to compare. Presets carry only what the bootstrapper uses.

- [x] **R2. Crystal pins as a kernel-owned resource**
  - Where: D Board semantics ("the crystal pins when `clock.lfxo` is set" return `ERR_DENIED`) and `names`.
  - Issue: Looks like the iceboxed kernel-owned board pins, and the kernel no longer reads `clock.lfxo`; it only knows `clock.lfxo` was called.
  - Options: keep them refused only after the bootstrapper calls `clock.lfxo` (the kernel knows that), or drop the rule with the other board-pin protections.
  - Decision: Dropped to the icebox, 2026-11-01, with the other kernel-owned board pins. The kernel refuses only the radio's pins, USB, internal flash, and SWD, plus the chip-reserved pins it detects itself (strapping, in-package PSRAM or flash).

- [x] **R3. Shell commands behind `mesh names`, `mesh pins`, and `mesh net`**
  - Where: D `apps/mesh` (every node command wraps a shell line) and the kernel recovery-core command list.
  - Issue: No shell command provides the name table, the pin map, or network state.
  - Options: add `names`, `pins`, and `net` to the kernel's native commands (they read kernel state), or provide them as bootstrapper shell niceties over kernel meshops.
  - Decision: A full hardware shell, adopted 2026-11-01. The kernel keeps only the recovery core natively. Everything else is a bootstrapper SDK module of thin wrappers over meshops, so the shell has the same control over hardware as code: commands and modes (`~.` exits) for names, GPIO, ADC, I2C, SPI, QSPI, UART, radio (`lora` mode moves here from a separate applet), BLE, network, config, events, taps, and time. New meshops `sys.names`, `gpio.map`, and `net.connect`. `mesh names`, `pins`, and `net` wrap it. Each hardware step adds its commands.

- [x] **R4. Manifest `kind` (`user` or `system`)**
  - Where: D manifest, `mesh.toml`, packer.
  - Issue: Nothing uses `kind` now that grants are gone.
  - Options: remove it, or give it a meaning (for example, how `mesh ps` groups applets).
  - Decision: Removed, 2026-11-01, from the manifest, `mesh.toml`, and the packer. An optional label can be added later if `mesh ps` ever needs to group applets.

---

## Third review (2026-11-01)

A third fresh review after R1–R4. About sixty findings were stale text, sequencing slips, or missing meshops and were fixed directly (see the implementation plan's results log). One needs a decision.

- [x] **T1. Device calls still resolve `[board]` names**
  - Where: D Board semantics ("The kernel still resolves a `[board]` name when an applet passes one to a device call"; "The kernel reads that bus's pins from `[board]`"), the `gpio.configure`, `adc.open`, `spi.open`, `qspi.open`, and `uart.open` rows, `gpio.map` (each pin's `[board]` name), and `names`.
  - Issue: R1 says the kernel never reads `[board]`, but opening a device by name (`spi.open("spi.sd")`, `gpio.configure("led.red")`) makes the kernel look the name up there.
  - Options:
    - (a) Name resolution moves to the SDK: device meshops take raw pin specs only, and the SDK's open helpers read `[board]` with `config.getStr` first. The kernel keys shared buses by their pins instead of a name, and `gpio.map` shows pins plus whatever label the caller passed.
    - (b) Keep it as the one stated exception: the kernel reads `[board]` only to resolve a name passed to a device call, never at boot and never on its own.
  - Decision: Neither option as written; adopted 2026-11-01: the bootstrapper's board module layers on the six device-open meshops and resolves `[board]` names with `config.getStr`, passing raw pins down with `sys.next` and the name as a label. The kernel's device meshops take only raw pin specs, buses are shared by pins, and `gpio.map` shows the label. Works for every caller and language, upgradable without a reflash, and R1 holds literally.
