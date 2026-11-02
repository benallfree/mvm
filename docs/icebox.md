# Icebox

Ideas worth keeping that are deferred past v1. Nothing here is designed or scheduled. When an idea is picked up, move it into [the design plan](mesh_virtual_machine_5ae0e741.plan.md) and [the implementation plan](implementation-plan.md), and delete it here.

Each entry says what the idea is, why it was deferred, and what v1 does instead.

## Tooling

- **Web flasher with board setup.**
  - Idea: A web page that flashes the kernel onto a board over WebUSB or Web Serial, then sets up `[board]` from a live repository of board presets (one file per board, carrying wiring, `power_on` pins, and radio limits, with an index of names, aliases, MCU family, and the `[board]` schema version, and new boards added by pull request). The flasher is where board setup belongs, because it is already the first thing a new user touches.
  - Board autodetection, as part of it. Narrow the board list before asking, cheapest and safest evidence first:
    - the MCU family, chip, and flash size from the kernel or the ROM bootloader;
    - USB vendor and product ids and the product string;
    - the nRF bootloader's board id, if readable (implementation-plan 0.10);
    - whether PSRAM is present (the kernel already detects this for its own use);
    - a passive I2C scan for known addresses (for example the T-Deck keyboard).
    - Last, and only after asking, a radio probe against the remaining candidates' wiring. It drives pins, so on the wrong board it may touch other hardware.
  - Always show `radio.max_dbm` (a legal limit) and the wiring before writing, even when detection is certain.
  - Deferred: v1 has no web front end, and board setup works by editing a file.
  - v1 instead: Flash with each family's own tools, through `cargo xtask bench`. `mesh mount` the node, open `/boot/config.ini` in a text editor, and paste the board's `[board]` section from the presets on the project site.

- **Cloud builder (`mesh build --remote`).**
  - Idea: Send the project to a hosted runner of the same pinned SDK image, so a machine without Docker can build. Builds are reproducible, so a cloud build and a local build of the same source have the same content hash, and the cloud output can be checked rather than trusted.
  - Deferred: a hosted service brings hosting, auth, quotas, and abuse protection with it, and field development often has no network.
  - v1 instead: `mesh build` always runs the pinned image locally through Docker.

- **Native applet builds without Docker.**
  - Idea: `mesh build` uses a local toolchain when every component matches the versions pinned in the image exactly, and refuses otherwise.
  - Deferred: there are two paths to support, and a slightly different compiler gives different bytes and a different install id.
  - v1 instead: Docker only.

## Distribution

- **Epidemic applet distribution and automatic upgrades.**
  - Idea: Nodes share applet updates with their neighbours, so a new signed build (see applet signing below) spreads across the mesh and upgrades itself without anyone visiting each node. A node accepts an update only if it has a trusted signer, the same applet name as the file it replaces, the same signer as that file, and a strictly higher version (no rollback over the air).
  - Deferred: it needs chunked transfer over LoRa within duty-cycle limits, resumption, and a policy for when an update is applied (immediately, at the next restart, or at a quiet time).
  - v1 instead: users upgrade with `mesh cp`. This needs applet signing first.

## Kernel

- **Applet isolation and grants.**
  - Idea: Protect applets from each other's bugs. Each applet reads and writes only its own heap partition, and another applet's memory is reachable only through a temporary grant (a buffer lent for one call, an event payload for one callback). Node config holds a `[grants]` section of operator switches for risky powers: `sys.reboot`, layering middleware on a builtin, capabilities, pinned content hashes, and per-name minimum versions. `config.ini` and `boot.mhc` would then need to be writable only through `mesh`, or the switches mean nothing.
  - Deferred: the operator is trusted, and in v1 the only guardrail wanted is memory limits. Isolation adds grant bookkeeping, lending and copy-back, and tests.
  - v1 instead: partitions stay, for memory limits, unload cleanup, and address translation, but any applet may read and write any heap partition, call every kernel function, and layer on any name. A pointer into an unmapped page traps, and the kernel's own pages stay protected.

- **Safe self-replacement of a loaded applet.**
  - Idea: When the file's applet is in the current call chain, refuse in-place writes and truncation of its loaded file with `ERR_BUSY`, and defer a rename over it or a delete until the chain returns to the scheduler: then stop the applet, do the rename or delete, and let the usual settle timer reset the node. Code is then never paged from a changed file. The SDK offers a helper that writes `name.mhc.part` and renames it over, the same pattern as `mesh cp`.
  - Deferred: applets can already write raw flash, SPI, and QSPI through kernel primitives, so guarding one path buys little under the trust model (C23 in contradictions.md).
  - v1 instead: an applet that changes its own loaded file from inside its callback gets undefined behaviour.

- **Marking and enforcing drivers.**
  - Idea: Make "applets that serve a mount live on `/boot`" visible and enforced. The SDK's block-device and filesystem-provider glue emits a marker the packer turns into a `driver` manifest flag (or `driver = true` in `mesh.toml` for hand-rolled drivers), drivers get their own extension (`.mhd` beside `.mhc`), the loader loads a driver only from `/boot` (by convention `/boot/drivers/`), and `mesh cp` warns when a driver goes anywhere else.
  - Deferred: under the trust model a documented policy is enough, and breaking it only matters when a code-page fill needs another fill, which already traps.
  - v1 instead: the policy is documented in the design plan, and a recursive code-page fill traps.

- **Responsiveness: non-blocking I/O, an urgent lane, and preemptive contexts.**
  - Idea: No turn ever waits on a slow device, and input reaches its handler within a target such as 50 ms. Three pieces, usable separately:
    - Non-blocking files, Node-style: `fs.*` take a callback and `ctx`, and an I/O context (a second VM context with its own stack and request queue) runs littlefs and the storage drivers, so a driver's `sys.sleep` pauses only that context while the event loop keeps running turns. Results arrive as completions. The SDK's `async`/`await` keeps applet code straight-line. Code-page fills and the loader still wait for the I/O context, since they cannot continue without the data. On ESP32-S3 the I/O context could run on the second core.
    - An urgent lane: an `urgent` flag on sources the kernel owns (`gpio.configure`, later key or touch input) routes their records to a small second ring the scheduler serves first. It removes queueing delay, not the wait for a running turn.
    - Preemptive guest contexts: an interrupt sets a "switch wanted" flag that the interpreter tests on each branch, so a high-priority context (a UI thread) takes over within a few guest instructions; long native work runs at a lower Embassy priority so input preempts it in hardware. Applets are pinned to one context, and calls between contexts are posted messages.
  - Deferred: v1 is not realtime. The pieces bring concurrency rules (re-entry into applets, what synchronous `evt.emit` means across contexts, locking littlefs) that the run-to-completion model avoids.
  - v1 instead: turns run to completion; `sys.sleep` pauses a turn in place while a driver polls a busy device, and other events wait. 5.18 measures the worst delay (DG20 in contradictions.md).

- **Kernel-owned board pins.**
  - Idea: Pins the bootstrapper uses for board bring-up (`power_on`, regulator, and the 32 kHz crystal pins after `clock.lfxo`) become kernel-owned after boot, and an applet configuring them gets `ERR_DENIED`. A grabbed crystal pin would stop the low-frequency clock that timers depend on.
  - Deferred: a correctly configured node never touches them again, and v1 trusts its applets.
  - v1 instead: nothing stops an applet from driving a power pin; doing so is a configuration bug.

- **Other radio chips.**
  - Idea: Drivers for radio chips other than the SX1262, such as the LR1121 or SX1276.
  - Deferred: none of the supported boards uses one. A different radio chip family would be a different kernel image, since its driver is native.
  - v1 instead: the SX1262 only; `radio.attach` refuses any other `radio.chip` with `ERR_UNSUPPORTED`.

- **Applet heaps in PSRAM.**
  - Idea: On ESP32-S3 boards with PSRAM, a second page pool in PSRAM for large applet heaps (message stores, image buffers), with its page table also in PSRAM or with larger pages, and rules for how `reserved_ram` and `max_ram` count each pool.
  - Deferred: a page table for 8 MB in internal RAM would cost about 256 KiB; PSRAM is slow on a cache miss and unreadable during flash erases; and no v1 applet needs megabytes.
  - v1 instead: the page pool is internal SRAM on every board; PSRAM holds only the code-page cache (DG18 in contradictions.md).

- **Extensible `ps` statistics.**
  - Idea: Let applets add columns or counters to `ps`, for example a provider's queue depth or a protocol's retry count, so one command shows a node's whole health.
  - Deferred: the kernel's `ps` is part of the recovery core and should stay small and native.
  - v1 instead: `ps` shows the kernel's own numbers. An applet can add its own shell command for its statistics.

- **Duty-cycled radio receive.**
  - Idea: Listen in windows instead of continuously, to save power on solar and battery nodes, as an extra `radio.config` field (receive mode and window).
  - Deferred: it adds timing policy that mesh protocols must agree on.
  - v1 instead: receive is continuous between transmits once the radio is configured (DG8 in contradictions.md).

- **Disabling applets that keep failing (strikes).**
  - Idea: A reset an applet causes (its own `sys.reboot`, or a watchdog, fault, or lockup while a retained blame slot names it) counts as a strike. At `max_strikes` the kernel refuses to load it (`ERR_DISABLED`) and the rest of the node boots. Strikes clear after `stable_s` of uptime, on a replace, and on `mesh reboot`.
  - Deferred: a bedrock applet, such as a filesystem driver, that strikes out on a rarely used code path (a rare remote administration command, say) would take every applet that depends on it down with it, so an operator could brick the node. It also needs blame, which is unreliable with deep hook chains.
  - v1 instead: any trap, blown budget, or watchdog reset logs to the syslog and reboots. Nothing is disabled.

- **Unloading an applet without a reboot.**
  - Idea: `sys.exit`, `mesh stop`, or a trap unloads just one applet when nothing depends on it: no other applet calls its names, layers on them, holds its callback references, listens to its events, or pages code from a tree it provides.
  - Deferred: deciding "nothing depends on it" reliably, and cleaning up everything an applet touched, is a lot of machinery for v1.
  - v1 instead: applets are only ever stopped by a reset. To stop one, remove it from `[applets]` and `mesh reboot`.

- **Profiling and hook tracing.**
  - Idea: Per-applet counters for self time (its own instructions), inclusive time (with its callees, counted once per chain), and root time (turns it started), plus a trace of the hook chain for a slow or failing turn. A busy library shows high self time but low root time, so it is not mistaken for the culprit.
  - Deferred: v1 does not need it; with many layered hooks, blame is hard to assign fairly without a trace.
  - v1 instead: `instr_per_event` logs which external event failed to finish and reboots. The syslog and the inbox drop counters are the diagnostics.

- **A kernel-owned event bus, so single applets can reload live.**
  - Idea: Replace every callback an applet passes to a call with events on a kernel bus, WordPress-hook style. Subscriptions live in the kernel and belong to the subscriber, so no applet holds a pointer into another's code and any applet can reload at any time. It also makes observation free, so transport taps stop being a separate feature.
    - A call that finishes later returns a request id. Its done event carries the caller id, the request id, the status, and the payload, and the SDK's `async` layer maps ids to futures.
    - `evt.on(name, handler, scope)` filters in the kernel: `mine` by default, `all` for observers, so a busy node does not wake every listener for every event.
    - The kernel records requests taken by applet-provided names. A provider finishes one with `sys.done`, and if it unloads first, the kernel emits the done event with `ERR_UNAVAILABLE`.
    - Synchronous calls and `sys.next` middleware stay as they are.
  - Deferred: it rewrites completion callbacks, per-resource callbacks, taps, the call table, the `async` layer, and many implementation steps, and its fan-out cost needs measuring.
  - v1 instead: replacing a loaded applet's file resets the node (C18 in contradictions.md), so every applet restarts together.

- **Safe mode.**
  - Idea: A boot that skips `/boot/boot.mhc` and every applet, leaving only the kernel and the management service, so anything on `/boot` can be fixed. Entered from a retained flag set by `mesh reboot --safe` or `sys.reboot(reason, Safe)`, from a bootstrapper that traps (it sets the flag and resets, so safe mode starts clean), from a node-level boot counter reaching a limit, or from an unmountable `/boot`. The bootstrapper would call `sys.bootOk` after `stable_s` to clear the counter. The counter would count only crashes: watchdog, fault, and lockup resets and applet `sys.reboot`, but not power-on, brownout, the reset pin, `replace`, or `mesh reboot`.
  - Deferred: in the field it only helps with a remote way in (see remote recovery below), and over USB the management service already runs before any applet.
  - v1 instead: per-applet strikes disable an applet that keeps resetting the node. A reset loop nobody can be blamed for keeps looping until the node is fixed over USB or reflashed.

- **Remote recovery over LoRa.**
  - Idea: In safe mode, the node listens on LoRa for management commands, so a node out of reach can still be fixed.
    - It reuses the management service's frames, so `mesh --node lora:NODE ps|cfg|ls|cat|rm|reboot` works unchanged. On the host, `mesh run` hosts a `remote_admin_client` applet that drives a LoRa modem (the `lora-modem-mcu` example). It is point to point, with no relaying.
    - Each command is signed with an Ed25519 key listed in an `[admin]` section of `config.ini`. Against replay, the node answers each command with a fresh challenge that the client signs, so nothing needs storing on the node. Responses are signed but not encrypted, so anyone listening can read them; `cfg` output might hide `[net]` passwords.
    - Safe mode reads `[board]` and `[admin]` from a recovery page in raw flash, rewritten by the kernel after a healthy boot when they change, so a corrupted `/boot` still leaves the radio and the keys. This reverses the design plan's "no separate raw profile page".
    - Listening only in safe mode keeps the radio free of applets (DG8). Listening in normal mode too would be full remote management.
  - Costs: Ed25519 verification in the kernel, the recovery page, fragmenting and resending frames, the challenge exchange, and the host client. Slow under duty-cycle limits: about three hours for a 20 KB `mesh cp` at SF9 in the EU, fine for short commands.
  - Deferred: needs safe mode, and v1 is kept small.
  - v1 instead: fix nodes over USB or BLE. A node that is out of reach and stuck needs a visit.

- **Applet signing and a developer trust store.**
  - Idea: Before an applet runs or replaces another, the kernel checks that it is intact and comes from a trusted developer. This matters most once updates spread over the mesh.
    - Developers make Ed25519 keypairs with `mesh keygen`. `mesh build` signs each `.mhc` on the host, so the private key never enters the build image.
    - A signature block (signer public key and signature) follows the content. The content hash covers everything before it, so builds stay reproducible whoever signs them.
    - Each node lists trusted keys in a `[trust]` section of `config.ini`. `mesh trust` lists, adds, and removes them.
    - The loader refuses unsigned, tampered, or untrusted applets with `ERR_UNTRUSTED`. Ed25519 verification is a native host function.
  - Open questions found while designing it:
    - The embedded default bootstrapper needs an exemption, tied to a hash compiled into the kernel, not one in `config.ini`.
    - Tests, CI (including pull requests from forks), and `mesh run` scenarios need a committed test-only key, trusted by default.
    - `mesh cp` should refuse to replace a loaded applet with a build from a signer the node does not trust.
    - Keys need a way to be revoked and rotated.
    - `[trust]` must not be writable by any applet if it is to mean anything (see C19 in contradictions.md).
  - Deferred: it adds complexity that is not needed yet.
  - v1 instead: no signing. The loader hashes each applet at load, and the user trusts what they copy onto the node.
