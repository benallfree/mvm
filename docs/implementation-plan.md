# MVM implementation plan

The step-by-step order for building what [the design plan](mesh_virtual_machine_5ae0e741.plan.md) describes. The design plan says *what* each crate is. This file says *in what order* to build it, how to check each increment, and what result to expect.

## The hardware rule

MVM has to work on many nRF52840 and ESP32-S3 boards, so real boards are the proof, not the simulator.

- **Hardware first.** The native layer (console, flash, littlefs, LoRa, BLE, Wi-Fi) is proven one capability at a time on every bench board before the VM is built on top of it.
- **Every board, every milestone.** A step marked "both chips" runs on at least one nRF52840 board and one ESP32-S3 board. Every milestone exit check runs on every bench board that has the capability. A step is not done until those pass, unless the step says otherwise and gives a reason.
- **The simulator is a backend too.** Every HAL trait gets a simulator backend next to the hardware ones, and it passes the same checks. Beyond that, the simulator is for what a bench cannot do cheaply: big loss matrices, hidden nodes, long virtual runs, deterministic replay. Every simulator scenario that matters gets a bench counterpart.
- **One command runs the bench.** Flashing and on-device checks are scripted from M0 on, so testing on nine boards is as cheap as testing on one.

## Bench

| Id | Board | MCU | Radio | Notes | Status |
| --- | --- | --- | --- | --- | --- |
| `SIM` | Simulator (`mvm-sim` HAL backends on the Mac) | host | simulated | Wi-Fi and IP map to host sockets | always |
| `XN` | Seeed XIAO nRF52840 Sense with Wio-SX1262 | nRF52840 | SX1262 | 2 MB QSPI flash, native USB, S140 7.3.0 | on the bench |
| `RAK` | RAK4631 on RAK19003 | nRF52840 | SX1262 (in module) | Debugging board; SWD on the core's J2 pads | on the bench |
| `RSD` | RAK4631 on RAK19007 with RAK15002 microSD | nRF52840 | SX1262 (in module) | SD card on the WisBlock IO slot, separate from the radio's SPI | on the bench |
| `TAG` | RAK WisMesh Tag | nRF52840 | SX1262 | Deployment target, GNSS | on the bench |
| `T96` | Heltec Mesh Node T096 | nRF52840 | SX1262 with PA (up to 28 dBm) | 0.96" TFT, GNSS (UC6580) | on the bench |
| `TDK` | LilyGO T-Deck Plus | ESP32-S3 | SX1262 | SD slot on the radio's SPI bus, keyboard, display, GNSS | on the bench |
| `HV3` | Heltec WiFi LoRa 32 V3 (US915) | ESP32-S3 | SX1262 | USB through a CP2102 bridge, whose USB serial is the generic `0001` | on the bench |
| `HV4` | Heltec WiFi LoRa 32 V4 | ESP32-S3 | SX1262 with FEM (up to 28 dBm) | Native USB per the documents; the FEM's pins depend on the board revision | planned |
| `XS3` | Seeed XIAO ESP32S3 with Wio-SX1262 | ESP32-S3 | SX1262 | Native USB | planned |

Planned boards join the matrix when they arrive. A step that has passed on the boards present is checked, and a planned board's cells stay open until it has run.

The M1 capability matrix uses `⬜` not yet run, `✅` passed, `❌` failed (with a note in the step's `Result:` line), and `➖` not applicable to that board.

## How to use this file

- **A step exercises what it builds.** Every meshop, shell command, and `mvm` command a step's Do builds is run in that step's Check, with its result in the Expect, unless the step names the later step that checks it.
- **One step is one increment.** It should land as one commit or a small PR, and it ends with a check you can run.
- **Check a box only when its check passes.** Then add a `Result:` line under the step: the date, what you saw, and anything surprising. Keep it to one or two lines and link to logs or PRs for detail.
- **Step ids are stable once work starts.** Never renumber after that. A new step takes the next free number in its milestone (`4.12`), or a letter suffix when it must sit between two steps (`4.3a`). Reorder by moving the text, not by changing ids.
- **Unfinished steps are fair game.** Add, edit, split, reorder, or drop them as results come in. Move a dropped step to [Dropped steps](#dropped-steps) with a one-line reason instead of deleting it, so the history stays readable.
- **Design changes go to the design plan in the same commit.** If a result changes a decision, update [the design plan](mesh_virtual_machine_5ae0e741.plan.md) and note it in the step's `Result:` line.
- **Open questions live in [Open questions](#open-questions)** until a step answers them. The answering step records the answer in its `Result:` line and removes the question.

Each step has the same shape:

```markdown
- [ ] **N.M Title**
  - Do: what to build.
  - Check: the command or procedure, on the host and on which boards.
  - Expect: what passing looks like.
```

Milestones end with an **exit check**: one end-to-end demonstration, on hardware, that everything in the milestone works together.

## Drift guards

Two habits from wasm-style VMs keep creeping back: reserving a block of guest memory per applet up front, and copying guest code into RAM before running it. MVM does neither, from the floor up. These guards turn that into tests that cannot be argued with.

| Guard | Invariant | First enforced |
| --- | --- | --- |
| G1 | **No preallocated guest memory.** Loading an applet maps exactly its statics pages (plus the code page of a one-page image, G7) and no heap pages. The rest of its `reserved_ram` (which includes statics) is set aside but not mapped, and nothing else is set aside for it. | 4.12 |
| G2 | **Unbounded by default.** With `max_ram = 0` and `reserved_ram = 0`, an applet can grow until the shared pool is empty. Right after load, its `largest` is close to the whole free pool. | 4.12 |
| G3 | **Reservation and cap are exact.** Reserved pages never go to another applet, and an applet within its reservation never fails. A nonzero `max_ram` stops growth even while the pool has free pages. | 4.12 |
| G4 | **Free means freed.** After an applet frees everything, its mapped pages return to its statics plus the TLSF control block (plus the code page of a one-page image, G7), its set-aside pages return to the load-time count, and the pages are back in the pool. | 4.12 |
| G5 | **No per-applet stack or arena.** One shared guest stack and the inbox ring come from a fixed kernel reserve set aside at boot. Loading more applets does not grow it. | 4.12 |
| G9 | **The kernel never starves.** With the shared pool empty, every loaded applet still gets its callbacks and events, because the stack and inbox ring come from the kernel reserve. | 4.12, 5.5c, 9.3 |
| G6 | **Code stays in storage.** RAM used for guest code is the fixed code-page cache, fixed in size and the same whatever the number and size of applets; from 5.1 the kernel sizes it from the memory it detects, and from 5.8 `[kernel] code_cache_kb` overrides it. An applet whose image is larger than an nRF52840's free RAM runs on every board. | 2.9 |
| G7 | **One-page exception only.** An image whose code fits in one 512-byte code page may run from RAM, counted in that applet's mapped pages. An image one byte larger never does. | 2.9 |
| G8 | **Unmodified third-party code works and stays in its own partition.** Upstream C and Rust libraries that allocate (a C linked list, the C library's `qsort`, cJSON, Rust collections, `serde_json`, `format!`) compile to bytecode with their source untouched, any MVM adaptation happens only at link time, they pass their own checks, kernel allocation counts match the SDK's, memory returns to baseline, and an overrun of its own buffers never reaches another partition. | 5.24 |

Rules:

- **The kernel reports what the guards measure.** Per applet: statics, reserved, heap, and code-RAM pages, plus allocation and free counts. Globally: kernel partition pages and code-cache size. `mvm ps --json` reports them, and `cargo xtask guards <target>` reads that output and prints the pass or fail of each guard. Guards are repo tooling, not a user command.
- **Each guard has a test** in `tests/guards/`. From the step that introduces it, the test runs on the host and the simulator in CI on every PR, and on every bench board at every milestone exit.
- **A failing guard blocks merging.** Changing a guard's invariant needs your explicit sign-off and a matching change to the design plan's locked decisions.
- **Reviews check for drift.** Any change to the loader, `mvm-mem`, or `mvm-code` is reviewed for fixed-size guest buffers, per-applet arenas, and image copies, even when the guard tests pass.

## Milestone map

| Milestone | Outcome |
| --- | --- |
| [M0](#m0-workspace-bench-and-hardware-facts) | Workspace, CI, the bench with one-command flashing, and the hardware facts everything depends on |
| [M1](#m1-native-bring-up-one-capability-at-a-time) | First, `ping` typed over USB gets an answer from every board (1.0). Then every HAL capability (console, flash, littlefs, GPIO, ADC, SPI, LoRa, I2C, UART, BLE, Wi-Fi and IP, QSPI) and the native event inbox brought up one at a time, each with tests in an on-device HAL test suite run from `mvm tty` and the bench, and checked on the simulator and every bench board, driven by `mvm tty`, `mvm ls`, `mvm cp`, and the `hal-test` shell's `net` |
| [M2](#m2-vm-spike) | RV32IMAFC interpreter, SDK, and packer, with `hello` running on both chips and real size and speed numbers |
| [M3](#m3-mvm-abi) | Shared names, error codes, and byte layouts |
| [M4](#m4-mvm-mem) | Partitioned paged memory, fuzzed on the host and stress-tested on both chips |
| [M5](#m5-portable-kernel-on-hardware) | `mvm-core` running applets on both chips: the inbox and time-sliced turns, events, timers, calls, traps and the syslog, VFS, bootstrapper, providers, plus extra flash on a XIAO, FAT and littlefs on a T-Deck Plus SD card, and Wi-Fi networking for applets |
| [M6](#m6-radio-service-and-frisbee-on-hardware) | The kernel radio service and frisbee playing across nRF and ESP32-S3 boards |
| [M7](#m7-simulator-and-mesh-run) | `mvm run` for what the bench cannot do: loss matrices, hidden nodes, long runs, replay |
| [M8](#m8-lora-modem-and-the-darwin-player) | Darwin plays frisbee through a board used as a dumb radio, over USB and BLE |
| [M9](#m9-paging-chaos-and-soak) | Code paged from flash, hostile applets contained, overnight soak on a mixed bench |
| [M10](#m10-register-level-radio-simulator) | The SX1262 register-level simulator |

M1 and M2 run in sequence: no interpreter work starts until M1's exit check passes, so the HAL, the event inbox, and the HAL test suite are solid first. The build image (2.1) and the applet target (2.1b) need no hardware and may be prepared during M1, but nothing in M2 is checked off before M1 is done.

---

## M0. Workspace, bench, and hardware facts

- [x] **0.1 Record the bench boards**
  - Do: For every board in [Bench](#bench), record the USB path (native USB, USB-Serial-JTAG, or a USB-UART bridge chip) with its VID:PID and USB serial number, the SX1262 wiring, flash and PSRAM size, the bootloader (on nRF, the bootloader and SoftDevice versions from `INFO_UF2.TXT`, and the app start address derived from the SoftDevice version, which 0.4 links against), whether SWD is reachable, and which LEDs, buttons, battery ADC, I2C, UART, and SD hardware it has. Record any power-enable pin that must be driven before peripherals work (the T-Deck Plus's peripheral rail, Heltec `Vext`), and what it powers, from the vendor's example code, Meshtastic's variant files, and the schematic. Record `radio.max_dbm` for every board under US FCC rules (915 MHz). For the T-Deck Plus, record whether the SD card shares an SPI bus with the radio. For `RSD`, record the RAK15002's SPI pins and card-detect pin from the schematic, since its datasheet contradicts itself. For `T96`, record the PA enable pins and the TX power limit at the SX1262 that keeps the PA output legal.
    - Layout: one summary table, then a section per board listing its wiring as `[board]` keys from the design plan's Board semantics (`radio.nss = P1.10`), each line with its source.
    - Sources: the schematic wins over a datasheet, and Meshtastic's variant file breaks a remaining tie. Each conflict is noted in the board's section.
    - Planned boards are recorded from documents and marked unconfirmed until they arrive.
  - Check: On every board on the bench, read the USB enumeration on the Mac (`ioreg -p IOUSB -l`) for VID:PID and serial number; on each nRF board, double-tap reset and read `INFO_UF2.TXT`; on each ESP32-S3 board, run `espflash board-info` for flash and chip revision, and `esptool chip-id` for PSRAM, which `espflash` does not report.
  - Expect: A board facts table, committed as `profiles/README.md`, matching the enumeration, `INFO_UF2.TXT`, `espflash`, and `esptool` output for every bench board. Every bench board has a unique USB serial number, or the board is flagged for 0.5. The `➖` cells of the M1 matrix are filled in from it.
  - Result: 2026-11-01. All seven bench boards read and recorded in [profiles/README.md](../profiles/README.md), with document facts sourced line by line. S140 6.1.1 (app at `0x26000`) on `RAK`, `RSD`, `TAG`, and `T96`; S140 7.3.0 (app at `0x27000`) on `XN`, so 0.4 builds two nRF images until 0.6 decides. `TAG` and `T96` run EnvyBoot, an Adafruit fork treated as the stock bootloader. `HV3`'s CP2102 reports the generic serial `0001`, flagged for 0.5. The T-Deck Plus's SD card, radio, and display share one SPI bus. `RAK`'s base is a RAK19003 (the 2026-10-31 log's RAK19002 is a boost module), `XN` is the Sense, and `espflash` does not report PSRAM, so the Check added `esptool chip-id`.

- [x] **0.2 Create the Cargo workspace**
  - Do: `Cargo.toml` workspace with empty crates matching the design plan's layout (`crates/*`, `apps/*`, `applets/*`, `fuzz/mvm-mem`, `profiles/`). Pin the stable toolchain in `rust-toolchain.toml` for the host crates and the nRF kernel; the ESP32-S3 kernel's toolchain is pinned by the firmware build image (0.2a). Add `rustfmt.toml` and a `clippy` config.
    - Members: the nine `crates/*` (`no_std` libraries, except the host-only `mvm-sim` and `mvm-packer`), `apps/mvm`, `xtask`, and `tests/guards` as the crate `mvm-guards`. A `.cargo/config.toml` alias makes `cargo xtask` run it.
    - Excluded from the workspace: `apps/mvm-nrf` and `apps/mvm-esp32s3` (folders only; 0.2a puts the firmware stubs in them), `fuzz/mvm-mem` (its own `cargo-fuzz` workspace with an empty target, first run in 4.2), and `applets/*` (each folder holds only a `.gitkeep` until the applet target exists in 2.1b). `scenarios/` holds a `.gitkeep`.
    - Toolchain: stable `1.99.1`, edition 2024, with `rustfmt`, `clippy`, and the `thumbv7em-none-eabihf` target.
    - Lints: a `[workspace.lints]` table with `clippy::all = "warn"`, inherited by every crate, and no `clippy.toml`. Warnings become errors on the command line (`-D warnings`).
    - Metadata: one `[workspace.package]` with version `0.0.0`, `publish = false`, and no license yet. `.gitignore` adds `/target`.
  - Check: `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo build --workspace`, `cargo test --workspace`, and `cargo xtask`.
  - Expect: Every member builds, the (empty) test run passes, format and clippy are clean, and `cargo xtask` prints its usage.
  - Result: 2026-11-01. All 12 members build on 1.99.1, the empty test run passes, `fmt` and `clippy -D warnings` are clean, and `cargo xtask` prints its usage (an unknown command exits 2). A planted `x == x` failed clippy, so the lint table bites. The seven `no_std` crates also build for `thumbv7em-none-eabihf`. `fuzz/mvm-mem` is not built until 4.2. Pinning `1.99.1` by number makes rustup download it apart from `stable` once per machine.

- [x] **0.2a Firmware build image**
  - Do: A Dockerfile for kernel firmware, pinned by digest and published for arm64 and amd64 like the applet image (2.1): Espressif's Rust toolchain for the ESP32-S3's Xtensa cores, installed with `espup` at a pinned version, next to stable Rust with `thumbv7em-none-eabihf` for nRF52840, plus the UF2 converter. It builds a stub firmware crate for each family now, and from 0.4 on both kernel images (and from 1.5a their `hal-test` builds). Flashing stays on the host, because Docker on macOS cannot reach USB devices: `cargo xtask bench` installs pinned versions of `espflash` and `probe-rs` on the host and flashes the artifacts the image produced.
    - Image: `docker/firmware/Dockerfile`, built for `linux/arm64` and `linux/amd64` from `rust:1.99.1-slim-bookworm` pinned by digest, with espup 0.19.0, the Xtensa Rust toolchain 1.98.0.0 (1.99.0.0 cannot link `esp-hal` 1.2.2, and 1.98.1.0 has no arm64 Linux build), espflash 4.7.0, and Microsoft's `uf2conv.py` at commit `90e9741f217f`. `docker/firmware/versions` lists every pinned version in one place, including the host's `espflash` and `probe-rs`, which 0.5 installs.
    - Digest: `docker/firmware/digest` records the image id for each platform. Nothing is pushed yet; 0.3 publishes the image and records its multi-arch digest in the same file.
    - Stubs: `apps/mvm-nrf` (`cortex-m-rt` and `panic-halt`, linked at `0x26000` for S140 6.1.1) and `apps/mvm-esp32s3` (`esp-hal` 1.2.2 with the ESP-IDF app descriptor and an empty `main` loop), each its own workspace with its own `Cargo.lock` and `rust-toolchain.toml`. Builds run at `/work` with path prefixes remapped.
    - Artifacts: the nRF `.uf2` (family `0xADA52840`) and the ESP32-S3 `.bin` from `espflash save-image`, under `target/firmware/<platform>/`.
    - Commands: `cargo xtask image firmware` builds both platforms and writes the digest file; `cargo xtask firmware <nrf|esp32s3|all> [--platform linux/arm64|linux/amd64]` builds in the pinned image, on the host's platform by default.
  - Check: Build both stubs in the arm64 image and again in the amd64 image (emulated on the Mac), then `cmp` the `.uf2` files and the `.bin` files. Also run the regression set.
  - Expect: Identical stub image bytes from both platforms; rechecked on the 0.4 blink image, and on a real Linux x86 runner in 0.3. The digest is recorded and used by 0.3 and 0.5.
  - Result: 2026-11-01. Both images build (the amd64 one under Rosetta: 471 s for the Xtensa toolchain step, against 59 s natively). The nRF `.uf2` (`e6a7b973…`, 2,560 bytes) and the ESP32-S3 `.bin` (`e7e44d8a…`, 75 KB) are byte-identical from arm64 and amd64 and on a clean rebuild; nothing was flashed (0.4 is the first hardware test). Xtensa Rust 1.99.0.0 cannot link `esp-hal` 1.2.2, so 1.98.0.0 is pinned. Two fixes made the ESP32-S3 image reproducible: `SOURCE_DATE_EPOCH` is set to the `HEAD` commit time, because the ESP-IDF app descriptor records a build time; and the image is made from a stripped ELF, because espflash hashes the whole ELF into the descriptor and Rust symbol names carry crate hashes that differ between build platforms. The images are recorded by local id until 0.3 pushes them to Docker Hub.

- [x] **0.3 Add CI**
  - Do: CI runs format check, clippy with warnings as errors, and host tests. Build jobs for `riscv32imafc-unknown-none-elf` and `thumbv7em-none-eabihf` (the `no_std` crates: `mvm-abi` and `mvm-sdk` for RISC-V until 2.1b adds the applet target, all seven for Cortex-M), and both kernel images inside the firmware build image from 0.2a (`thumbv7em-none-eabihf` and the ESP32-S3 target), and `mvm` built and host-tested on macOS, Linux, and Windows (each with its own USB and BLE backend).
    - CI is GitHub Actions, one workflow (`.github/workflows/ci.yml`) on every push to `main` and every pull request, with Rust builds cached. The macOS and Windows jobs build and test only `mvm`. Steps keep landing on `main`; a red run blocks the next step.
    - `cargo xtask image firmware --push <version>` builds both platforms on the Mac and pushes them to Docker Hub as the public multi-arch image `benallfree/mvm-firmware:<version>`, starting at `0.1.0`, and writes its digest to `docker/firmware/digest`, which `cargo xtask firmware` then pulls by. 0.3a publishes `0.1.0` as the first `firmware-image` release.
    - The firmware job uploads its stub `.uf2` and `.bin` as artifacts.
  - Check: Push a branch and open a PR. Download the firmware job's artifacts and `cmp` them with a Mac build of the same commit. Push a second commit with a deliberate clippy warning, then close the PR's warning commit away.
  - Expect: All jobs pass. A deliberate clippy warning fails the build. The x86 runner's stub bytes match the Mac's. The minutes each run costs are recorded.
  - Result: 2026-11-01. On [PR 1](https://github.com/benallfree/mvm/pull/1) all seven checks passed (GitGuardian, already on the account, is the seventh); a planted `x == x` failed `lint-test` with `clippy::eq_op`. The x86 runner's `mvm-nrf.uf2` and `mvm-esp32s3.bin` match the Mac's arm64 build of the same commit byte for byte, which closes 0.2a's cross-machine comparison. `benallfree/mvm-firmware:0.1.0` is public and multi-arch, pinned by `sha256:0c4c0348…`. Docker Desktop's default builder cannot build several platforms at once, so `--push` pushes `0.1.0-arm64` and `0.1.0-amd64` and joins them. On pull requests the firmware job checks out the branch's head, not the merge commit, because the ESP32-S3 image carries the commit time. A run takes 16–36 s per job and 2 min 34 s for `firmware`, about 18 weighted minutes (Linux 6, macOS 10, Windows 2).

- [x] **0.3a Release pipeline**
  - Do: The changelogs from the design plan's [Releases](mesh_virtual_machine_5ae0e741.plan.md#releases) section, in Keep a Changelog form, each opening with an "Unreleased" section. `cargo xtask release <unit> <major|minor|patch>` bumps the unit's version, moves "Unreleased" under the new version and date, commits, and tags `<unit>-vX.Y.Z`. A CI workflow on those tags builds the unit and publishes a GitHub Release with that version's changelog section as its notes. `mvm --version` prints its version. The released crates take their own versions in place of 0.2's shared `[workspace.package]` version. Needs the GitHub remote from 0.3.
    - Rename: the firmware build image becomes the firmware builder: folder `docker/firmware-builder/`, Docker Hub `benallfree/mvm-firmware-builder`, release unit `firmware-builder-image`, and `cargo xtask firmware-builder [--push <version>]` in place of `cargo xtask image firmware`.
    - Versions and starting points are as the design plan's Releases section lists: every unit and the ABI at `0.0.0`; the image's first release (`minor`, so `0.1.0`) pushes it under its new name.
    - `cargo xtask release` needs a clean tree on an up-to-date `main`; it pushes the release commit and tag to `main`. For `firmware-builder-image`, it runs `cargo xtask firmware-builder --push <version>` first, unless that version is already on Docker Hub.
    - Changelogs `changelog/firmware.md`, `sdk.md`, `mvm.md`, and `firmware-builder-image.md`, each seeded under "Unreleased" with what exists so far.
    - Release workflow `.github/workflows/release.yml` on tags `*-v*`: for `mvm`, the five archives and `SHA256SUMS`, each built on its own runner except Linux arm64 (cross-compiled on x86), and `mvm --version` run on each native runner; for `firmware-builder-image`, notes with the digest and `versions`; for `firmware`, the stubs built in the pinned builder, with the component versions in the notes. `0.0.x` versions are marked pre-release. The `sdk` job arrives with the SDK image in 2.1.
  - Check: `cargo xtask release firmware-builder-image minor` (giving `firmware-builder-image-v0.1.0`) and `cargo xtask release mesh patch` (giving `mesh-v0.0.1`). Run the macOS binary here and the Linux arm64 binary in a container; the workflow runs the others.
  - Expect: The `mvm` GitHub Release is a pre-release holding the five archives and `SHA256SUMS`, with the changelog section as its notes, and each binary prints `mesh 0.0.1`. The `firmware-builder-image` Release carries the digest of `benallfree/mvm-firmware-builder:0.1.0`. The tags and the changelog commits are on `main`.
  - Result: 2026-11-02. `firmware-builder-image 0.1.0` (Latest, with the Docker Hub digest `sha256:0c4c0348…`, unchanged by the rename) and `mesh 0.0.1` (pre-release, five archives and `SHA256SUMS`) are published; every binary printed `mesh 0.0.1`, four on the release runners (macOS x86_64 through Rosetta) and Linux arm64 in a container here. Release commits `8c40607` and `717ca85` and their tags are on `main`; the step's code landed first as `d001521`, because releases run from `main`. During the step the firmware release unit became `mvm` (kernel plus embedded `boot.mhc`, with component versions in its notes) and `boot` became its own unit; the `mvm` job is built but not yet exercised. Release notes open with a per-unit tagline in italics, and the builder's notes name Docker Hub.

- [ ] **0.3b Package managers**
  - Do: Every `mvm` release, `0.0.x` included, is published to Homebrew, apt, and winget, and its notes gain **Install** and **Upgrade** sections with that version's commands. Other units are not installed through package managers.
    - Homebrew (macOS and Linux): the public tap `benallfree/homebrew-mvm`. The release workflow rewrites `Formula/mvm.rb` (version, the four macOS and Linux archive URLs, and their checksums; test `mvm --version`) and pushes it. Install `brew tap benallfree/mvm` then `brew install mvm`; upgrade `brew upgrade mvm`.
    - apt (Debian and Ubuntu, amd64 and arm64): `cargo deb` builds a `.deb` per architecture, attached to the release and added to a signed apt repository served by GitHub Pages from the public repo `benallfree/mvm-apt` (suite `stable`, component `main`, index signed with the `APT_SIGNING_KEY` secret, public key published as `mvm.gpg`). Install: add the key and the source, then `sudo apt install mvm`; upgrade `sudo apt update && sudo apt install --only-upgrade mvm`.
    - winget (Windows): the workflow opens a pull request to `microsoft/winget-pkgs` with the version's manifest, package id `benallfree.mvm`, moniker `mvm`. Install `winget install mvm`; upgrade `winget upgrade mvm`. The notes say a new version can take a day to appear.
    - The workflow pushes to the other repos with the `PACKAGING_TOKEN` secret. The packaging scripts live in `packaging/`.
  - Check: Release `mvm 0.0.2` and install it through Homebrew on the Mac and in a Linux container, and through apt in Debian containers for amd64 and arm64. Then release `mvm 0.0.3` and upgrade each. Confirm the winget pull requests are open, and once one merges, `winget install mvm` on the Windows runner.
  - Expect: Every install and upgrade prints the released version from `mvm --version`. The release notes carry the Install and Upgrade sections. The apt repository's signature verifies with the published key.

- [ ] **0.4 Blink on every board**
  - Do: The smallest image per family, linked on nRF at the installed SoftDevice's app address recorded in 0.1 (`cargo xtask firmware nrf` gains the `0x27000` image for `XN`), that logs a counter, using the HAL ecosystem each family will use for real (for example `embassy-nrf` and `esp-hal`). On ESP32-S3 it logs on both USB-Serial-JTAG and UART0, so it reaches the Mac whichever USB path a board has.
  - Check: Flash every board. Watch every log.
  - Expect: Every board logs from one image per family, except that nRF needs one image per installed SoftDevice (6.1.1 at `0x26000`, 7.3.0 at `0x27000` on `XN`) until 0.6 decides. The flashing method for each board is written down.

- [ ] **0.5 One-command bench**
  - Do: A script (for example `cargo xtask bench`) that builds in the firmware builder image from 0.2a, flashes every connected board from the host with the pinned flashing tools, and collects each board's log to a file. It finds boards by USB serial number, not by `/dev` name, and handles the nRF reset into the UF2 bootloader. It builds through `cargo xtask firmware` and installs the host tools at the versions in `docker/firmware/versions`. A board whose bridge chip reports a generic serial (`HV3`'s CP2102 reports `0001`) is told apart by its chip's MAC, read through the ROM bootloader. On nRF boards whose presets use `P0.09`/`P0.10` as GPIO, the bench writes UICR once to free the NFC pins.
  - Check: Run it with every bench board connected, twice in a row.
  - Expect: Every board is flashed and its log captured with no manual steps. Total time is recorded.

- [ ] **0.6 Standardize the nRF SoftDevice**
  - Do: Start from the versions 0.1 recorded: S140 6.1.1 on `RAK`, `RSD`, `TAG`, and `T96` (app at `0x26000`), and S140 7.3.0 on `XN` (app at `0x27000`). Measure each one's app RAM start. Since they differ, choose one of: update every board to one SoftDevice through its bootloader, build one image per SoftDevice version, or use Nordic's SoftDevice Controller linked into the kernel and ignore the installed SoftDevice. Check whether the Rust `nrf-softdevice` crate supports the chosen version, or whether BLE needs the SoftDevice Controller with a Rust host (`trouble`).
  - Check: The blink image from 0.4 with a BLE advertisement added, flashed with the 0.5 bench script, seen from each nRF board by a phone or the Mac.
  - Expect: A decision recorded in the design plan's locked decisions with the measured addresses. Anything other than "one image per family on the stock SoftDevice" changes a locked decision and needs your sign-off.

- [ ] **0.7 Fix the ESP32-S3 flash layout**
  - Do: One partition table for every ESP32-S3 board: bootloader, app, and the `boot` littlefs partition, sized for the smallest flash on the bench (8 MB on `HV3`, from 0.1). PSRAM, when a board has it, holds only the code-page cache (9.1); the page pool, inbox, and stack stay in internal SRAM (DG18).
  - Check: The blink image from 0.4, built with the new partition table and flashed with the 0.5 bench script to every ESP32-S3 board on the bench.
  - Expect: It boots on all of them. The partition table goes into the design plan.
    - The `firmware` release's ESP32-S3 file becomes a merged image (`espflash save-image --merge`: bootloader, partition table, and app), so a blank board flashes in one go.

- [ ] **0.8 On-device test runner**
  - Do: A way to run Rust unit tests on each board and report pass or fail to the host (for example the `embedded-test` crate with `probe-rs` where SWD is reachable, or a small harness reporting over the console where it is not).
  - Check: A trivial passing and a trivial failing test on every board.
  - Expect: The bench script reports both results correctly for every board.

- [ ] **0.9 Check the bootloader with a running watchdog**
  - Do: An image that starts the WDT and then requests DFU through `GPREGRET`, on each nRF bootloader on the bench.
  - Check: Watch whether each board stays in its bootloader drive or reset-loops.
  - Expect: An answer per bootloader to the design plan's "Bootloader" watchdog note. If one reset-loops, add a workaround step to M1.

- [ ] **0.10 Check whether the bootloader's board id is readable**
  - Do: Look for the board-id string from `INFO_UF2.TXT` inside each nRF bootloader's flash region.
  - Check: Read that region from a test image.
  - Expect: Yes or no for the design plan's preset auto-detection idea. If no, drop that idea from the design plan.

**Exit check:** `cargo xtask bench` flashes every bench board, runs an on-device test on each, and collects every log, from one command.

---

## M1. Native bring-up, one capability at a time

This is the kernel's native layer with no VM. Everything here ships in the final kernel, except the HAL test suite, which ships only in the test build. No interpreter work starts until M1's exit check passes: the HAL and the event inbox must be solid first.

From 1.5a on, every step adds its tests to the HAL test suite, and from 1.5b on, every event source it brings up (a pin interrupt, a radio frame, UART bytes, a BLE link, a socket) feeds the inbox, with that source's cap rule tested. Each step adds one HAL trait (or one use of it), with a backend for each family and for the simulator, and checks it on every board in the matrix. The CLI grows alongside it, so every capability is driven from the Mac.

A step is checked when every applicable cell is `✅` for the boards on the bench. Update the matrix as cells pass.

| Step | Capability | `SIM` | `XN` | `RAK` | `RSD` | `TAG` | `T96` | `TDK` | `HV3` | `HV4` | `XS3` |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 1.0 | Hello: `ping` over USB | ➖ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ |
| 1.0a | Minimal simulated node, `host:` target | ⬜ | ➖ | ➖ | ➖ | ➖ | ➖ | ➖ | ➖ | ➖ | ➖ |
| 1.1 | Boot, console log, identity | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ |
| 1.2 | Clock and alarm | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ |
| 1.3 | Watchdog, reset reason, retained RAM | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ |
| 1.4 | Console byte stream | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ |
| 1.5 | Shell line protocol, `mvm ping` | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ |
| 1.5a | `mvm tty` and the HAL test build | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ |
| 1.5b | Native event inbox and main loop | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ |
| 1.6 | Internal flash block device | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ |
| 1.7 | littlefs `/boot`, power pulls | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ |
| 1.8 | `mvm ls`, `cp`, `cat`, `rm` | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ |
| 1.8a | `mvm mount` | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ |
| 1.9 | GPIO: LEDs, buttons, interrupts | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ |
| 1.10 | ADC: battery | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ➖ |
| 1.11 | SPI master | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ |
| 1.12 | SX1262 probe and configuration | ➖ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ |
| 1.13 | LoRa send and receive, `mvm tty` | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ |
| 1.14 | LoRa across families | ➖ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ |
| 1.15 | Radio wedge recovery | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ |
| 1.16 | Flash writes during reception | ➖ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ |
| 1.17 | Console unplug while logging | ➖ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ |
| 1.18 | I2C master | ⬜ | ⬜ | ➖ | ➖ | ⬜ | ➖ | ⬜ | ⬜ | ⬜ | ➖ |
| 1.19 | UART: GNSS | ⬜ | ➖ | ➖ | ➖ | ⬜ | ⬜ | ⬜ | ➖ | ➖ | ➖ |
| 1.20 | BLE byte stream | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ |
| 1.21 | Wi-Fi, IP, DNS, TCP, UDP | ⬜ | ➖ | ➖ | ➖ | ➖ | ➖ | ⬜ | ⬜ | ⬜ | ⬜ |
| 1.22 | QSPI flash | ⬜ | ⬜ | ➖ | ➖ | ➖ | ➖ | ➖ | ➖ | ➖ | ➖ |
| 1.23 | SPI bus shared with the radio | ➖ | ➖ | ➖ | ➖ | ➖ | ➖ | ⬜ | ➖ | ➖ | ➖ |
| 1.24 | Inbox under load, full HAL suite | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ |

- [ ] **1.0 Hello: `ping` over USB**
  - Do: The least HAL that answers a typed line: USB serial (CDC on nRF, USB-Serial-JTAG on ESP32-S3; the simulator gets its stream in 1.0a) and a line reader in the shell's place that answers `ping` with `pong`, the part, the build hash, and the raw reset-cause register (decoded from 1.3 on), and anything else with `err unsupported`. No `mvm` CLI yet. 1.4 turns the port into the non-blocking byte stream, and 1.5 grows the reader into the full shell protocol.
  - Check: Flash every board with `cargo xtask bench`, open the port in a plain serial terminal, and type `ping`, then `hello`.
  - Expect: Every board answers `ping` with its identity and `hello` with `err unsupported`.

- [ ] **1.0a Minimal simulated node**
  - Do: The simulator HAL backend, enough to run the same native kernel code as the boards: one or two simulated nodes in one host process, each with its `boot` partition as a file-backed block device (littlefs on top from 1.6 and 1.7, so the power-pull checks apply to `SIM` too), a localhost TCP socket per node carrying the shell, and an ideal radio medium that delivers every frame. Each node exposes its shell on that socket, which `mvm --node host:<node>` reaches from 1.5 on exactly as `usb:` reaches a board. Wall-clock time; virtual time, scenarios, loss, and the full medium come in M7. Every later M1 step that has a `SIM` cell fills it against this node.
  - Check: Start two simulated nodes and type `ping` to each over its virtual stream with a plain socket client. From 1.5 on, `mvm --node host:a` reaches them the same way.
  - Expect: Each answers like a board, with its node name in place of a part and board id.

- [ ] **1.1 Boot, console log, and identity**
  - Do: `apps/mvm-nrf`, `apps/mvm-esp32s3`, and the simulator node, each logging the part, reset cause, build hash, and the HAL backend in use. On ESP32-S3, the console runs on both USB-Serial-JTAG and UART0. Where 0.10 found the bootloader's board id readable, log it too.
  - Check: Bench run.
  - Expect: Every board logs its identity through its own USB path.

- [ ] **1.2 Clock and alarm**
  - Do: `mvm-hal::clock`: a 64-bit `now` in microseconds and one hardware alarm (arm, replace, cancel), plus the simulator's virtual clock. Anything needing many timers multiplexes them over the alarm, as the bootstrapper will.
  - Check: A native test multiplexes a thousand random deadlines over the alarm and logs the lateness of each, on every board. Run long enough on one board to pass the 32-bit microsecond wrap (71 minutes).
  - Expect: Deadlines fire in order. Worst-case lateness per board is recorded. `now` never goes backwards across the wrap.

- [ ] **1.3 Watchdog, reset reason, and retained RAM**
  - Do: `mvm-hal::watchdog`: start at boot, feed from the main loop, read the hardware reset cause, and keep a retained word and an 8 KiB retained region (for the syslog ring) across resets.
  - Check: A debug command that stops feeding the watchdog, one that resets with a stored word, and a hard fault, each after filling the retained region with a known pattern, plus a power pull. On every board.
  - Expect: Each board logs "watchdog", "software, word", or "fault" after reboot, with the region intact after each reset and detected as invalid after the power pull. Record any chip where a reset kind does not keep the region.

- [ ] **1.4 Console byte stream**
  - Do: `mvm-hal::usb` as a non-blocking byte stream (USB CDC on nRF; USB-Serial-JTAG and UART0 on ESP32-S3; a virtual stream in the simulator).
  - Check: An echo test from the Mac at full speed for a minute, on every board.
  - Expect: Every byte echoes back. Throughput per board is recorded.

- [ ] **1.5 Shell line protocol and `mvm ping`**
  - Do: The kernel shell over USB as the design plan describes it: lines with optional request ids, `#id ok` and `#id err` endings, `!` unsolicited lines, and the first commands (`ping`, `log`). `apps/mvm` finds boards by serial number, rescans across resets, and sends `ping` for `mvm ping`. `--node host:<node>` reaches the simulated nodes from 1.0a.
  - Check: Type `ping` by hand from a plain serial terminal on every board. `mvm ping` against every board, then unplug and replug during a ping loop. Send a line with a bad request id, a line of exactly 1 KiB, a line over 1 KiB, and an unknown command.
  - Expect: Every board answers by hand and through `mvm`. The CLI reconnects without a restart. Log lines stream as `!` lines without breaking replies. The 1 KiB line is accepted whole. The bad lines get specific `err` replies and the shell keeps working.

- [ ] **1.5a `mvm tty` and the HAL test build**
  - Do: `mvm tty` opens an interactive session with the shell from 1.5. A `hal-test` build of each kernel image (`cargo xtask bench --hal-test`) adds a test suite to that shell: `test list`, `test run <id>`, and `test run all`, with a pass or fail line per test. Tests are automatic or interactive (press a button, look at an LED, attach a second board); `test run all` skips interactive ones unless asked. The bench runs the same suite non-interactively over the same shell lines and fills the matrix from its results. The suite starts with tests for 1.1 to 1.5, and every later M1 step adds its own. `--node host:<node>` reaches the simulated nodes from 1.0a. Native hardware commands added to the `hal-test` shell through M1 use the same names and syntax as the bootstrapper's hardware shell (the reference in the design plan) and stay a minimal subset.
  - Check: On every board: `mvm tty`, `test list`, `test run all`, one interactive test, and one deliberately failing test. Then `cargo xtask bench --hal-test` across the bench.
  - Expect: The shell answers on every board. Each result is reported once, the failing test fails with its reason, and the bench's per-board summary matches what `mvm tty` showed. The normal build has the shell without the `test` commands.

- [ ] **1.5b Native event inbox and main loop**
  - Do: The kernel's main loop as the design plan describes it, with no VM: interrupt handlers only wake driver tasks, driver tasks copy data out of the device into the inbox ring (a static buffer of a provisional built-in `inbox_kb` until 5.1 moves it into the kernel reserve), and a consumer takes one event at a time. Per-source caps, pin-edge records (time and level, appended while the record is newest and untaken, with a per-pin byte cap and drop counter), backpressure, drop counters, and the inbox high-water mark. In this milestone the consumer is native: the test suite and the shell. A `busy <ms>` test consumer works in slices and yields between them, standing in for a long VM turn. Driver tasks run in an interrupt-priority executor above the main loop; the main loop feeds the watchdog.
  - Check: On every board, timers and console bytes as the first sources: a burst of a thousand timers, console input at full speed mixing short lines with 1 KiB ones, record sizes chosen to force a record to skip the ring's tail and start at the beginning, and both while `busy 5000` runs.
  - Expect: Every event arrives once, in order per source. Every record is one unbroken buffer, and the skipped tail is reclaimed once the record before it is freed. During `busy`, timers and console input wait in the inbox, the watchdog is fed, and nothing is lost that the source rules say should not be. Drop counters and the high-water mark are reported by the shell (`mvm ps` shows them from 5.1).

- [ ] **1.6 Internal flash block device**
  - Do: `mvm-hal::flash` with the `boot` partition as a block device (on nRF through the SoftDevice flash API, on ESP32-S3 from the 0.7 partition table, in the simulator as the file-backed device from 1.0a).
  - Check: On-device tests: erase, write, and read back every block of the partition.
  - Expect: Data round-trips. Erase and write times per board are recorded.

- [ ] **1.7 littlefs `/boot` and power pulls**
  - Do: littlefs on the `boot` block device. Choose the crate here (the C wrapper or a pure-Rust one) and measure its size per family.
  - Check: Format, write, remount, read back. Then pull power mid-write fifty times on each board.
  - Expect: After every pull, `/boot` mounts and holds either the old or the new contents, never garbage. Code size per family recorded.

- [ ] **1.8 `mvm ls`, `cp`, `cat`, `rm`**
  - Do: The shell's file commands (`ls`, `stat`, `cat`, `put`, `append`, `commit`, `cp`, `mv`, `rm`, `mkdir`, `hash`) and the `mvm` wrappers over them. Transfer sessions with the per-channel staged list. `cp` through `.part` and a rename, one chunk per turn; `mv` as a rename on one mount and `cp` plus `rm` across mounts. `mvm cp` host to node with `put`, `append`, and one `commit`; node to host with `cat`; node to node with the shell's `cp`.
  - Check: Copy a 100 KiB file to every board, then copy a different file over it and interrupt that copy by unplugging. `cat` the target, then resume the copy and `cat` it again, comparing hashes each time. `mvm cp` three files in one command. On the node, `mkdir` a directory and `ls` it, `cp` a 100 KiB file into it while console lines keep arriving, `stat` the copy, `mv` it within `/boot`, then `rm` it and `ls` again. `mvm cp` a file from the node to the host.
  - Expect: While interrupted, the target still has the old file's hash and a `.part` file sits beside it. The copy resumes rather than restarting, the part file is gone afterwards, and hashes match everywhere. The same copy done by hand with `put`, `append`, and `commit` in `mvm tty` gives the same file. A chunk with a wrong CRC is refused. The three files land together at the one `commit`. The node-side `cp` produces a matching hash and console lines are answered between its chunks. `ls`, `stat`, and `rm` show each change, and the node-to-host copy matches.

- [ ] **1.8a `mvm mount`**
  - Do: A local WebDAV server inside `mvm`, bridged to the shell's file commands, mounted with the operating system's WebDAV client (Finder on macOS and `net use` on Windows, with no `sudo`; `davfs2` on Linux). `LOCK` is always granted. Finder litter (`.DS_Store`, `._*`, `.Spotlight-V100`, `.Trashes`, `.fseventsd`) is dropped in the bridge. Each `PUT` is landed whole through `put` and `commit`.
  - Check: Mount every board in turn from macOS, and one board from Linux and from Windows. Browse it in Finder, edit `/boot/config.ini` in a text editor and save, copy a file in with Finder, copy an `.mhc` in with Finder and kill the copy halfway, then reset the board while mounted. Repeat against a host node. Edit `config.ini` in place in a text editor that rewrites part of the file, and truncate a file.
  - Expect: The edit and the copy land intact (check with `mvm cat` and a hash). No litter file reaches the node, and the killed `.mhc` copy leaves the old file (or no file) intact on the node, never a partial one. The mount keeps working after the reset without remounting. Both land as whole-file replaces with the right contents.

- [ ] **1.9 GPIO: LEDs, buttons, interrupts**
  - Do: `mvm-hal::gpio` with any-pin handles, edge interrupts that timestamp each edge and its level into the pin's inbox record, and the pin syntax for each family, plus simulator pins that tests can drive. Board facts still come from a hard-coded table per board until `[board]` parsing lands in M5.
  - Check: In the `hal-test` shell, `gpio set` blinks every LED a board has and `gpio watch` logs each button press.
  - Expect: Every LED blinks, and `gpio watch` prints each edge with its timestamp and level; a bounce shows as extra edges in one record.

- [ ] **1.10 ADC: battery**
  - Do: `mvm-hal::adc` and the battery divider for each board that has one.
  - Check: Compare the reading with a multimeter on battery and on USB power.
  - Expect: Within a few percent of the meter. Boards without a battery input are `➖`.

- [ ] **1.11 SPI master**
  - Do: `mvm-hal::spi` with clock and mode per transfer and chip-select hold, ready for both the radio and later the SD card.
  - Check: A loopback or a known device on each board's radio bus (reading the SX1262 status register is enough); on the simulator, a fake SPI loopback device.
  - Expect: Correct replies at the radio's clock on every board.

- [ ] **1.12 SX1262 probe and configuration**
  - Do: The SX1262 driver behind `mvm-hal::radio`: reset, wait for BUSY, read a known register, set TCXO and the RF switch, and clamp power to the board's maximum.
  - Check: Boot each board with correct wiring, then with one pin deliberately wrong. On the T-Deck Plus, probe with its power-enable pin low, then high. The native bring-up code drives `power_on` from the hard-coded board table, as the bootstrapper will later.
  - Expect: Correct wiring passes the probe. The wrong pin fails it, logs it, and leaves the radio off. The T-Deck Plus probe fails with power off and passes with it on.

- [ ] **1.13 LoRa send and receive, `mvm tty`**
  - Do: A native `lora` command in the `mvm tty` shell, in the `hal-test` build only (normal builds get `lora` mode from the bootstrapper's hardware shell in 6.1), with `mvm tty --freq … --sf … --bw … --cr … --power …` as a shortcut that starts in it. Received frames reach it through the inbox. Each typed line goes out as one LoRa frame, and each received frame prints as `[rssi snr] text`. The simulator uses an ideal medium here; the full medium comes in M7.
  - Check: Pairs within each family (`XN`–`RAK`, `TAG`–`RSD`, `T96`–`RAK`, `TDK`–`HV3`, and so on as boards arrive), plus two simulated nodes.
  - Expect: Lines arrive with plausible RSSI and SNR on every pair.

- [ ] **1.14 LoRa across families**
  - Do: Nothing new: this checks that the two families agree on the air.
  - Check: `mvm tty` between an nRF board and an ESP32-S3 board with identical settings, for every board at least once. A scripted run of 1,000 frames each way for one pair.
  - Expect: Lines cross in both directions. Delivery is close to 100% on the bench, and airtime per frame matches the time-on-air formula.

- [ ] **1.15 Radio wedge recovery**
  - Do: Wedge detection (BUSY stuck, a missing interrupt, a failed status read), with a chip reset and the last configuration reapplied. The simulator's radio gets fault injection.
  - Check: A debug hook forces a wedge during a `mvm tty` session on every board.
  - Expect: The log shows the wedge and the reset, and the session keeps working.

- [ ] **1.16 Flash writes during reception**
  - Do: Driver tasks in the interrupt-priority executor. On nRF, flash operations through the SoftDevice with partial page erases; on ESP32-S3, the radio interrupt and copy-out in IRAM. Measure the slowest native call per chip (a flash erase, a littlefs write), which must stay far below the watchdog timeout.
  - Check: `mvm cp` large files repeatedly to a board while a second board sends back-to-back frames at SF7, 125 kHz. Record the longest time the copy-out path was blocked.
  - Expect: No frame lost to an overwrite in the chip. The longest block stays under one minimum frame time. If a chip cannot meet it, that is found here, before any VM work.

- [ ] **1.17 Console unplug while logging**
  - Do: Make the log sink non-blocking end to end.
  - Check: Unplug USB during a heavy log stream and a `mvm tty` session, on every board.
  - Expect: No board stalls or resets. The radio keeps receiving, which shows when you plug back in.

- [ ] **1.18 I2C master**
  - Do: `mvm-hal::i2c` with transfers serialized in the kernel.
  - Check: `i2c scan` each board's I2C bus in the `hal-test` shell and read one known register from a device on it (the T-Deck Plus keyboard or touch controller, a Heltec OLED, or a sensor on a Qwiic or Grove port).
  - Expect: The scan finds the expected addresses, and the register read is correct. Boards with no I2C device are `➖` until one is attached.

- [ ] **1.19 UART: GNSS**
  - Do: `mvm-hal::uart` with buffered receive.
  - Check: Read NMEA sentences from the GNSS on boards that have one (`TAG`, `T96`, `TDK`).
  - Expect: Valid NMEA with correct checksums, with no lost bytes over ten minutes.

- [ ] **1.20 BLE byte stream**
  - Do: `mvm-hal::ble` as a peripheral carrying the same shell lines as USB, and a BLE central in `apps/mvm` (CoreBluetooth on macOS, BlueZ on Linux, WinRT on Windows).
  - Check: `mvm ping --node ble:<name>` and `mvm tty --node ble:<name>` against every board.
  - Expect: The same behaviour as over USB. On ESP32-S3 a smoke-level pass is acceptable at first if its BLE stack is immature; note what is missing.

- [ ] **1.21 Wi-Fi, IP, DNS, TCP, and UDP**
  - Do: `mvm-hal::net` on ESP32-S3 (the Wi-Fi driver and a TCP/IP stack, for example `esp-wifi` with `embassy-net`), and on the simulator through host sockets. Credentials from a hard-coded test table until `config.ini` lands. A native `net` command in the `hal-test` shell prints the association state and address (`mvm net` comes with the hardware shell in 5.21).
  - Check: Join the bench Wi-Fi, resolve a name, open a TCP echo connection to a server on the Mac, and send UDP both ways. Then drop the access point and bring it back. Run this with BLE and LoRa active too.
  - Expect: Each works and recovers after the access point returns. The RAM the Wi-Fi stack takes per board is recorded, because it comes out of the pinned kernel region and so shrinks the page pool. nRF boards are `➖`.

- [ ] **1.22 QSPI flash**
  - Do: `mvm-hal::qspi`: one flash command with address, transmit, and receive phases, with no ready-wait (busy waits poll status `0x05` in the caller). The simulator gets a fake NOR chip.
  - Check: On the XIAO, read the JEDEC ID, erase a sector, program a page, and read it back.
  - Expect: The expected ids and a clean round trip. Erase time recorded.

- [ ] **1.23 SPI bus shared with the radio**
  - Do: Kernel arbitration on a bus the radio shares: other chip selects are allowed, and their transfers are serialized with radio operations. This applies to `TDK` if 0.1 found a shared bus.
  - Check: `spi xfer` in the `hal-test` shell initializes the SD card (CMD0, CMD8) and reads its CID while `mvm tty` runs on the same board.
  - Expect: The card answers, and `mvm tty` keeps working without lost frames.

- [ ] **1.24 Inbox under load, and the full HAL suite**
  - Do: Every source each board has feeding the inbox at once, with `busy` standing in for long turns.
  - Check: On every board, for ten minutes: back-to-back LoRa frames from a second board, a GNSS UART stream, a pin toggled in fast bursts from a second board or a simulator pin, with a burst large enough to pass the pin's byte cap, a BLE stream, and on ESP32-S3 a TCP stream, while `busy 2000` runs in a loop. Then `cargo xtask bench --hal-test` with every test, including interactive ones with a person at the bench.
  - Expect: Every LoRa frame is copied out of the chip before the next one arrives; frames over `inbox_radio` are dropped and counted, never corrupted. Streams lose nothing (backpressure), a pin's burst of edges arrives as one record with every timestamp, edges past the pin's cap are counted, the watchdog never fires, and the high-water mark stays under `inbox_kb`. Every test in the suite passes on every bench board.

**Exit check:** Every applicable cell in the matrix is `✅` for the boards on the bench, and `cargo xtask bench --hal-test` passes the full HAL suite on every board. M2 does not start before this. From the Mac, with no VM, you can copy files to every board and read them back after a power pull, exchange `mvm tty` lines between any nRF board and any ESP32-S3 board with one of them connected over BLE, and reach a TCP server from every ESP32-S3 board over Wi-Fi.

---

## M2. VM spike

Starts only after M1's exit check: the HAL and the event inbox are solid and covered by the HAL test suite. Answers whether the bytecode toolchain keeps the design plan's promises before the kernel is built around it. Host-first, but the speed number comes from the chips.

- [ ] **2.1 Build image**
  - Do: The SDK image, since every applet build from here on runs in it, released as `sdk` to Docker Hub as `benallfree/mvm-sdk` and pushed like the firmware builder (0.3a). The release workflow gains its `sdk` job. A Dockerfile in the repo, at `docker/sdk/`, with pinned clang and lld (RISC-V target) and the pinned Rust toolchain with `riscv32imafc-unknown-none-elf`. Build flags are deterministic: path prefixes remapped, timestamps fixed, codegen settings pinned. Publish it for both arm64 and amd64 hosts and record its digest. CI's RV32 build job from 0.3 moves into it. The template and the packer join the image in 2.6c, and the C library in 5.24.
  - Check: Build the image twice from clean, and compile a trivial `no_std` RV32 crate inside each with a plain `docker run`.
  - Expect: Both images give byte-identical ELFs. CI's RV32 job passes inside the image. Across build platforms, compare stripped ELFs: symbol names carry crate hashes that differ between hosts (0.2a).

- [ ] **2.1b The applet target and link windows**
  - Do: A `no_std` `riscv32imafc-unknown-none-elf` test applet with an `init` symbol and a linker script placing one image at placeholder partition `0x00`, `.text` at `0x0001_0000` and `.data`/`.bss` at `0x0080_0000`, built in the image with the `medany` code model, the `ilp32f` ABI, and `--emit-relocs`. Confirm the Rust target `riscv32imafc-unknown-none-elf` and clang's matching target build cleanly. The source includes a string-literal table and a trait object.
  - Check: `cargo build --target riscv32imafc-unknown-none-elf --release` inside the image, then `llvm-readelf -S` and `llvm-objdump -d`.
  - Expect: Sections at the two link addresses, with `.rodata` in the statics window. Only RV32IMAFC instructions. Code reaches constants and statics with `auipc` pairs, and the only absolute relocations are `R_RISCV_32` words in rodata and `.data`.

- [ ] **2.2 RV32I interpreter and riscv-tests**
  - Do: `mvm-vm` generic over `Memory`, `Code`, and `Clock`, with `RamCode` and a flat test memory. Run the `rv32ui-p-*` tests from `riscv-tests`, built in the image, with a small environment that reports pass or fail through a fixed address.
  - Check: All `rv32ui` tests on the host.
  - Expect: All pass, apart from fence and CSR behaviour, which is noted rather than implemented.

- [ ] **2.3 M, A, F, and C extensions**
  - Do: Multiply and divide, atomics (`LR.W`, `SC.W`, and every `AMO*.W`), single-precision floats on the host FPU (with `fcsr` and rounding modes), and 16-bit decode through `fetch16`. Double-precision and unknown compressed encodings trap as illegal.
  - Check: `rv32um-p-*`, `rv32ua-p-*`, `rv32uf-p-*`, and `rv32uc-p-*` on the host (2.5 runs them on both chips, with floats on each chip's FPU), plus a 4-byte instruction at a 2-byte offset, an `LR.W`/`SC.W` pair split across a slice boundary, and a misaligned atomic.
  - Expect: All pass. The split `LR`/`SC` pair succeeds, the misaligned atomic traps, and each excluded group traps without crashing the host.

- [ ] **2.4 Traps, instruction budgets, and handler return**
  - Do: `ecall` exits with the argument registers, `ebreak` is a panic trap, `ra` starts at a sentinel so `ret` ends the handler, and a PC outside the code window traps. The interpreter stops after `instr_per_tick` instructions with the PC and registers saved, resumes exactly there, and traps when a turn passes `instr_per_event`.
  - Check: Unit tests for each exit kind. A long computation run in one go and again in slices of 1, 7, and 1000 instructions. An infinite loop with a budget.
  - Expect: Each exit is reported correctly. The sliced runs give the same result and register state as the single run. The loop traps after exactly the budgeted instruction count.
- [ ] **2.5 Instruction tests on both chips**
  - Do: Run the interpreter with the riscv-tests binaries embedded, as on-device tests.
  - Check: Bench run on both boards.
  - Expect: Every test that passes on the host passes on both chips.

- [ ] **2.6a SDK crate and hello source**
  - Do: `mvm-sdk` with the `mesh` entry, the linker script for the two link windows, an `ebreak` panic handler, and a `log.write` wrapper. `applets/hello` uses it and logs one line of text.
  - Check: Build `hello` in the image from 2.1, then `llvm-readelf -S -s` on the ELF.
  - Expect: `init` is exported. Every section sits inside a link window, 4-byte aligned. The author's crate has no linker script of its own.

- [ ] **2.6b Packer**
  - Do: `mvm-packer`: read the ELF's section sizes and `init`, refuse an image outside the link windows or not 4-byte aligned, collect the `R_RISCV_32` relocations into the sorted relocation list, refuse absolute instruction relocations, absolute words in `.text`, and unaligned relocated words, and write the header (with `body_hash`), manifest, relocations, and code of the `.mhc`. It builds as the `mvm-pack` binary, which 2.6c puts in the SDK image.
  - Check: A golden test on the packed `hello`. Negative tests: missing `init`, a section outside the windows, bad alignment, an absolute `lui`-based address in code, and a file that is not an ELF.
  - Expect: The golden bytes match. Each bad input gives its own specific error, never a panic.

- [ ] **2.6c SDK template and packer in the image**
  - Do: Add the `mvm-sdk` template, the link settings, and `mvm-pack` to the SDK image from 2.1, with the image's public `new` and `build` commands (design plan, `crates/mvm-sdk`), publish the new image, and record its digest. This is the digest `mvm build` pins.
  - Check: Make and build `hello` into an `.mhc` with plain `docker run` commands only (`new`, then `build`), bypassing `mvm`.
  - Expect: The ELF matches the one 2.6a produced, and the `.mhc` matches 2.6b's golden bytes. The published digest matches the one recorded.

- [ ] **2.6d `mvm new` and `mvm build`**
  - Do: `mvm new` copies the SDK template into a new project and writes its `mvm.toml`, including `sdk`, the current SDK release. `mvm build` runs that SDK release's image by digest, whose `build` command compiles and packs. `mvm new` runs its `new` command. It pulls the image if it is missing.
  - Check: `mvm new demo && cd demo && mvm build` on a machine that has never pulled the image. Then `mvm build` with Docker stopped, and again with a compile error in the source.
  - Expect: A valid `demo.mhc`, after a one-time pull, identical to the one the plain `docker run` commands from 2.6c give. With Docker stopped, `mvm build` says it needs Docker and how to start it, rather than failing inside the compiler. A compile error is shown with paths relative to the project, not to the container.

- [ ] **2.6e Reproducible builds and CI**
  - Do: CI's applet build jobs run inside the same image.
  - Check: `mvm build` on `applets/hello` on an Apple-silicon Mac, on an x86 Linux machine, and in CI, each from a different project path.
  - Expect: All three `.mhc` files have the same content hash, so the same source is recognisably the same build everywhere. The packer takes only sections, so symbol-name differences between hosts (0.2a) do not reach the `.mhc`.

- [ ] **2.7 hello on both chips**
  - Do: A minimal native loader in both images: open an `.mhc` on `/boot`, run `init` in the interpreter through a minimal `FileCode` reader that pages code from the file through a small cache of 512-byte slots (4, the nRF default), with a stub `mesh` that understands `log.write`. The image is never copied into RAM. A fill is a plain read; code pages carry no relocations. No `mvm-mem` yet; `.rodata` and `.data` are copied into a plain RAM statics buffer sized from the manifest, `.bss` is zeroed, and the relocations are applied there once, with the image at a non-zero partition. The same stub loader runs in the 1.0a simulated node, so the G6 and G7 checks in 2.9 run there and in CI.
  - Check: `mvm cp hello.mhc :/boot/` to each board, then power-cycle it.
  - Expect: Both boards log hello's line, including text read through a relocated table of pointers.

- [ ] **2.8 Measurements**
  - Do: A frisbee-sized dummy applet that decodes and encodes byte buffers, keeps a small roster, and calls `mesh` with several names. A CPU-bound benchmark applet, with an integer loop and a single-precision float loop (a running average and a rate).
  - Check: Size with `opt-level = "z"` and LTO. Disassemble for misaligned accesses and any `core::fmt` code. Run the benchmark on the Mac, the nRF board, and the ESP32-S3 board.
  - Expect: A table in the `Result:` line: `.mhc` size, instructions per second on all three, float operations per second against integer ones, and whether `core::fmt` crept in (and from where, if so). These numbers set the default `instr_per_tick` and `instr_per_event`.

- [ ] **2.9 Code stays in flash (G6, G7)**
  - Do: A generated applet with about 200 KiB of code (many generated functions that each fold their result into a checksum), and two tiny applets: one of exactly 512 bytes of code and one of 513. Counters for RAM used by code in the stub loader.
  - Check: Run each on every bench board and the simulator, one at a time (the M2 stub loader runs one applet). Record code RAM before load, after load, and while running.
  - Expect: The big applet runs and its checksum is right on the nRF boards, where 200 KiB cannot fit in RAM next to the SoftDevice and the kernel. Code RAM equals the slot cache size in every case. The 512-byte applet runs from RAM and the 513-byte one pages from flash. Two big applets at once and the one-page image in the mapped count are checked in 5.1, once `mvm-mem` and the real loader exist. The G6 and G7 tests join CI.

**Exit check:** The same `hello.mhc`, copied with `mvm cp`, runs on an nRF board and an ESP32-S3 board, an applet larger than an nRF board's RAM runs paged from flash, and the measurements table is filled in.

---

## M3. mvm-abi

- [ ] **3.1 Error codes and name constants**
  - Do: The error code enum and string constants for every kernel call and event name in the design plan's tables. Give `mvm-abi` its own version, starting at 0.1.0, as the ABI version.
  - Check: A unit test asserts that every name is `scope.verb` with no duplicates.
  - Expect: Passes. The lists match the design plan exactly.

- [ ] **3.2 Little-endian codecs**
  - Do: Encode and decode helpers and the fixed layouts: frisbee frame, `mem.stat`, `fs.stat`, directory entry, `statfs`, block-device geometry, `sys.resetReason`, `sys.version`, and the `radio.attach` wiring. Each later step that adds a meshop with an out-buffer or event payload (`sys.names`, `config.keys`, `gpio.map`, `radio.status`, `ble.status`, `net.status`, `net.sockets`, the `ble.scan` advertisement, `radio.configured`, `tap.*`) adds its layout to `mvm-abi` in that step.
  - Check: Round-trip property tests and golden bytes, on the host and as on-device tests on both chips.
  - Expect: Exact round trips everywhere. No `#[repr(C)]` casts.

- [ ] **3.3 Manifest format**
  - Do: The manifest type and its byte encoding (no settings table), with unknown fields rejected, including `abi` (written by the packer from the SDK's `mvm-abi` version) and optional `min_abi`. The author's fields come from `mvm.toml`. The packer from 2.6b switches to it.
  - Check: Round-trip tests. Rebuild `hello` and rerun 2.7.
  - Expect: Tests pass, and `hello` still runs on both chips.

- [ ] **3.4 config.ini reader and writer**
  - Do: A lenient INI reader (dotted names, first dot splits the section; size suffixes; unknown sections kept; malformed lines skipped and logged) and an in-place writer that changes or adds one key, keeping comments and order. Names up to 63 bytes, values up to 255.
  - Check: Table-driven tests, including a leftover section for a removed instance, a malformed line, `instr_per_tick = banana`, no `[net]`, an empty password, a set on an existing key, a set that adds a key and a section, and a file full of comments. A host fuzz target, and an on-device test reading and writing a realistic file on both chips.
  - Expect: Every file yields every well-formed key; nothing rejects the whole file. Writes change only the target line and preserve the rest byte for byte. Fuzzing finds no panic. Parse time on both chips is recorded.

**Exit check:** `mvm-abi` tests pass on the host and on both chips.

---

## M4. mvm-mem

Built bottom-up, with the host fuzz model growing at every step. Each step also runs its unit tests on both chips, so `u32` arithmetic and alignment are checked on real 32-bit targets, not just in an i686 fuzz build.

- [ ] **4.1 Page pool**
  - Do: 256-byte pages on a free list over a `&mut [u8]`.
  - Check: Unit tests on the host and both chips.
  - Expect: Every page is handed out exactly once until the pool is empty.

- [ ] **4.2 Page table and fuzz scaffold**
  - Do: The page table and `(partition, vpn)` index. Start `fuzz/mvm-mem` with map, unmap, load, store, and the reference model.
  - Check: Unit tests on the host and both chips, and five minutes of fuzzing.
  - Expect: "Every page is free or mapped exactly once" and "no page belongs to two partitions" hold.

- [ ] **4.3 Address decode and check**
  - Do: `check(addr, len, need)` with the partition table (image, heap, and kernel partitions), and every trap kind, in explicit `u32` arithmetic.
  - Check: Unit tests on the host and both chips. Fuzz including an i686 build.
  - Expect: Each trap kind is reachable and named. A null dereference traps.

- [ ] **4.4 Last-hit caches**
  - Do: Per-applet heap caches and one stack cache for the running chain, cleared on unmap and at the end of a turn.
  - Check: Fuzz compares cached and uncached lookups. A benchmark of a hot load on the Mac and both chips.
  - Expect: Fast path equals slow path. The per-access cost on each chip is recorded.

- [ ] **4.5 Kernel-page access**
  - Do: Kernel-partition pages assigned to the running call chain (the guest stack read-write, event payload read-only) and released when it returns. No owner check on heap partitions.
  - Check: Unit tests on the host and both chips, plus assignment and release in the fuzz target. An applet writes another applet's heap.
  - Expect: A kernel-page access outside the running callback's assignment always traps, including a kept pointer after release. The cross-heap write succeeds.

- [ ] **4.6 Limits and reservations**
  - Do: `max_ram`, `reserved_ram`, reservation-first mapping, and fencing.
  - Check: Unit tests on the host and both chips, and the four limit invariants from the design plan in the fuzz target.
  - Expect: All four hold for a 30-minute fuzz run.

- [ ] **4.7 Partition heap (TLSF through checked access)**
  - Do: Our own small TLSF whose control block and headers live in the partition and which touches memory only through `check`. Existing TLSF crates manage raw host memory, so they do not fit. Contiguous allocation for `mem.cmalloc`: a page-aligned TLSF block backed by a run of physically adjacent pages, mapped at allocation.
  - Check: Unit tests on the host and both chips. Fuzz alloc, free, realloc, and "write arbitrary bytes anywhere in partition P". A contiguous allocation of several pages, read and written natively from end to end and through `check`.
  - Expect: Operations on P change only P's pages. A pointer outside the partition traps on `free`. Native and guest views of a contiguous allocation match byte for byte. The control block size is recorded.

- [ ] **4.8 Kernel reserve, stack, stat, and pressure**
  - Do: The kernel reserve (guest stack at offset 0 of the kernel partition, inbox ring, completion pool, I/O scratch area), the shared guest stack with partition `0x01` as its unmapped gap below, zeroing and the high-water mark, the `mem.stat` snapshot, and the `mem.low`/`mem.ok` signals with their hysteresis. The kernel reserve is one contiguous block mapped linearly into the kernel partition.
  - Check: Unit tests on the host and both chips. Fuzz invariant: a `malloc` of exactly `largest` succeeds and one byte more fails. A guest that recurses until the stack runs out, and one with a single frame larger than the whole stack. Allocate and free variable-size inbox records inside the reserve, including one that skips the ring's tail. Drive free pages down through the low-water mark, bounce them around it, raise them above the high-water mark, and lower them again.
  - Expect: Holds. Both overflows trap on their first access past the stack, and the high-water mark is reported. Measure the default `stack_size` and `inbox_kb` here, and set the `io_scratch` default to the 255-byte path limit plus one code page. `mem.low` fires once per arming, not on each bounce; `mem.ok` fires once on recovery; the second fall fires `mem.low` again.

- [ ] **4.10 On-device stress**
  - Do: A long-running on-device test that drives random operations against the real page pool size of each chip, checking the invariants as it goes.
  - Check: One hour on each board.
  - Expect: No invariant violation, no watchdog reset, and flat free-page counts once the random workload is balanced.

- [ ] **4.11 Miri and the long fuzz run**
  - Do: Miri on the page-slice translation and host page walks.
  - Check: `cargo +nightly miri test -p mvm-mem`, then four hours of fuzzing.
  - Expect: Clean. The corpus is committed.

- [ ] **4.12 Memory drift guards (G1 to G5, G9)**
  - Do: The guard tests for G1 to G5 and G9 in `tests/guards/`, over `mvm-mem` with a test loader, the counters `mvm ps --json` will report, and `cargo xtask guards`.
  - Check: Load applets with `0/0`, with only `reserved_ram`, with only `max_ram`, and with both, sized so each reservation is a different page count. Grow each until it fails, free everything, and repeat. Load twenty applets with no allocations. Drain the shared pool with one applet and check that the kernel reserve's stack and inbox pages stay assignable. Run on the host, the simulator, and every bench board.
  - Expect: G1: right after load, each applet's mapped pages equal its statics exactly, its heap pages are zero, and its set-aside pages equal `reserved_ram` minus statics (or zero). G2: the `0/0` applet grows until the shared pool is empty. G3: reservations are never handed out, and `max_ram` stops growth with free pages left. G4: after free-all, mapped pages return to the statics plus the TLSF control block, set-aside pages return to their load-time count, and the pool gets its pages back. G5: twenty idle applets leave the kernel reserve at its boot size. G9 (memory side): with the pool empty, the reserve's stack and inbox pages are still assignable. The events-and-timeouts side of G9 is checked in 5.5c and 9.3. The tests join CI.

**Exit check:** The design plan's `mvm-mem` invariants hold over four hours of host fuzzing and an hour of on-device stress on each chip, and every guard introduced so far (G1 to G7 and G9) passes on the host, the simulator, and every bench board.

---

## M5. Portable kernel on hardware

M5 builds the kernel layer first (5.1 to 5.5b), then the bootstrapper modules (5.5c, 5.5d) linked into a test bootstrapper, then the remaining kernel steps (5.6 to 5.9, where 5.8 and 5.9 also add the board and `sys.ready` bootstrapper modules), whose checks use that test bootstrapper's bus and timers, and finally `applets/boot` (5.10), which assembles the modules. Applet features come after. From here on, behaviour is tested with small purpose-built test applets under `applets/tests/`. Each step runs its test applets on the host harness first, then on both chips through `mvm cp` and the bench script.

- [ ] **5.1 Kernel skeleton on both chips**
  - Do: `mvm-core` with `mvm-mem`, the interpreter, and `FileCode` with the shared slot cache, replacing the M2 stub loader in both images (the stub `mesh` with `log.write` stays until 5.2 and 5.3), the kernel's detection of RAM, PSRAM, and the part (FICR, ESP32-S3 eFuses) with the code cache sized from it, and a minimal `mvm ps` whose `--json` already carries the guard counters. (Before 5.1, guards in 2.9 and 4.12 report through the on-device test runner's output.) `RamCode` is only for host unit tests and the one-page case. Physical RAM split into the pinned kernel region and the page pool from each chip's measured layout. At boot, right after `/boot` mounts, the kernel sets the reserve aside (built-in defaults until 5.8 reads `[kernel]`) and the 1.5b inbox ring moves into it. Until `sys.load` (5.9) and the bootstrapper (5.10), the host harness and a `hal-test` shell command, `load <path> <instance>`, load named instances directly, which the multi-applet checks in 5.2–5.8 use.
  - Check: `hello` on both chips.
  - Expect: Same output as in 2.7, now through the real memory manager. `mvm ps` (minimal) shows `hello`. With two copies of the 200 KiB applet from 2.9 loaded at once, code RAM still equals the slot cache size, and the 512-byte applet's page shows in its mapped count. `cargo xtask guards` reports G1, G5, G6, and G7 passing on every board, reading the guard counters that the minimal `mvm ps --json` already includes.

- [ ] **5.2 Name table and dispatch**
  - Do: `mesh` dispatch with the status-in-`a0`, value-in-`a1` return convention, basic `sys.register` (publishing a name, refusing locked ones), `ERR_UNAVAILABLE` for unknown names, `sys.version`, and `sys.has`. The unique 64-bit device id in `sys.version` (FICR DEVICEID, ESP32-S3 eFuse MAC, a host node's scenario id).
  - Check: A test applet calls a builtin, an unknown name, publishes a name with `sys.register` and tries to register a locked name, reads `sys.version`, and asks `sys.has` about a builtin, a registered name, and a missing one, on the host and both chips. Read the device id on two boards.
  - Expect: Builtin runs, unknown name returns `ERR_UNAVAILABLE`, locked registration fails. `sys.version` matches the build. `sys.has` answers each case correctly without calling anything. The two ids differ.

- [ ] **5.3 Memory, crypto, and log builtins**
  - Do: `mem.*` (with `mem.malloc` alignment, and `mem.cmalloc`) and the SDK's `alloc` feature (`GlobalAlloc` over `mem.*`), `crypto.sha256`, `crypto.aes`, `fmt.sprintf`, `log.write`, and `log.printf` into the non-blocking text sink, and `mvm log`. The SDK's bump-arena helper for per-turn temporary buffers.
  - Check: Test applets: a growing and shrinking `Vec`, NIST vectors for SHA-256 and AES, `fmt.sprintf` against known-answer strings (every conversion, width, padding, precision, `%f`, a short buffer), and logging faster than the sink drains. On the host and both chips. Run `mvm log` while the test applets log. A test applet calls `mem.cmalloc` for a multi-page block, writes it end to end as a guest, has a native test hook read it back as one block, then frees it with `mem.free`; it also makes one aligned `mem.malloc`.
  - Expect: Vectors match on both chips. `mem.stat` numbers match expectations. `fmt.sprintf` output matches every known answer, and a short buffer returns the full length. A full sink drops and counts entries without blocking. `mvm log` shows the lines, prefixed with time and instance. The `mem.cmalloc` block is page-aligned and matches byte for byte natively, and `mem.stat` and the `max_ram` charge rise by its pages and fall back after the free. The aligned pointer is aligned.

- [ ] **5.4 Inbox, turns, and the alarm**
  - Do: Connect the VM to the inbox from 1.5b: turns, the time-sliced scheduler, kernel dispatch of each inbox event by calling the name `evt.emit`, `time.now`, `time.alarm` with its callback and `ctx`, and end-of-turn cleanup. No timer list in the kernel. `sys.sleep`, which pauses the turn in place. A native `hal-test` command, `test emit <name> <rate>`, injects inbox events on the chips for this step and 5.5c.
  - Check: On the host and both chips, with a test applet that registers `evt.emit` directly and logs what it receives, and a host-harness test event source that injects inbox events at a chosen rate: a handler that keeps an inbox payload pointer into a later turn; a turn that runs for several seconds across many slices while USB and the alarm keep being serviced; events arriving faster than turns finish; an alarm-accuracy test, and an alarm replaced and cancelled before it fires. A turn that sleeps in a loop for two seconds while USB, the shell, and the watchdog keep running.
  - Expect: Each kernel event reaches the test `evt.emit` with its name and payload. The stale pointer traps. The long turn never stalls USB, the watchdog, or the inbox, and later events wait for it. Over-cap events follow their source's rule and drops are counted. The alarm fires once per arm, a replaced or cancelled alarm never fires, and jitter is recorded per chip. During the sleeping turn the shell answers, the watchdog is fed, no other turn starts, and the sleep is not counted toward `instr_per_event`.
- [ ] **5.5 Cross-applet calls**
  - Do: `sys.register`, calls on the shared guest stack, depth limit, re-entry, `sys.caller`, and pointer arguments passed through unchanged. Meshop calls into applet code as guest frames in the same interpreter loop, never native recursion.
  - Check: `test.echo` called with a page-aligned and a small heap buffer, a string literal, a static buffer, and a pointer read from a relocated table, plus a recursion test and a callee that calls back into its caller, on the host and both chips.
  - Expect: Every call works: the callee reads the caller's literal, static, and heap bytes correctly through their global addresses, and writes the caller's buffers directly with no copy, the call back into the caller works, and the depth limit traps.

- [ ] **5.5a Callback references and sys.resolve**
  - Do: Callback references as plain arguments (two words: the function address, whose partition byte names the instance, and `ctx`), synchronous `sys.resolve` with its code-window check, forwarding a reference into another call, native completions queued in the inbox as their own turns, with each one-shot request taking a record from the fixed completion pool in the kernel reserve (sized from a built-in default) when it is made, freed when its turn runs, and the scheduler running whichever is older, the oldest inbox record or the oldest ready completion.
  - Check: Test applet A registers `test.later`, which keeps the reference and resolves it from a timer, and `test.now`, which resolves before it returns. Applet B calls both with two different `ctx` values. Then A forwards a reference into a builtin timer, A resolves a garbage reference, and B makes requests to a native test builtin that completes later until the completion pool is empty. One request stays pending while a few hundred other inbox events run, then completes; another becomes ready while older records are queued and newer ones keep arriving. Drain the shared pool with a test applet and complete a request. On the host and both chips.
  - Expect: B's callback runs once per request with the right `ctx`, nested inside A's `sys.resolve` call. `test.now` calls B back before B's call returns. The forwarded reference calls B directly. The garbage reference traps. With the pool empty, a native request fails at once with `ERR_BUSY` and no completion is ever lost. The other events run while the request is pending, and its completion arrives after them. The second completion runs after the older records and before the newer ones. The completion arrives with the shared pool empty (G9).
- [ ] **5.5b Middleware stacks and sys.next**
  - Do: Per-name stacks in the name table, layering through `sys.register`, `sys.next` resolved at call time, the one-layer-per-applet rule, callback references passed through `sys.next`. `sys.names`, which `mvm names` and the shell's `names` (5.5d) read.
  - Check: Over a test builtin `test.echo`, applet L1 layers a filter that tags each call and calls `sys.next`, and L2 layers a logger on top. Then: a third applet without a layer calls `sys.next` for `test.echo`; L1 calls `test.echo` on itself through `mesh`; L1 registers `test.echo` a second time; a call with a completion callback goes through both layers. On the host and both chips. Also a tracing layer on `log.write` that logs `sys.caller` and passes down. (`mem.*` is locked; the `fs.open` case is checked in 5.7.)
  - Expect: `sys.names`, read by a test applet, shows `test.echo: L2 → L1 → builtin` (`mvm names` arrives with the hardware shell in 5.5d), and calls pass through both layers in order. The applet without a layer is refused. L1 calling its own name recurses until the depth limit traps. L1's second registration replaces its callback without adding a layer. The completion callback fires once, from the builtin. Through the layers, the bottom provider and `sys.caller` in each layer see the original caller: a `log.write` passed down by the tracing layer is prefixed with the original caller's instance.

- [ ] **5.5c Bootstrapper modules: event bus, timers, and the contract**
  - Do: The SDK's bootstrapper modules: the event bus (`evt.on`, `evt.emit`, `evt.has`, `evt.off` over kept callback references and synchronous `sys.resolve`) and timers (`time.setTimeout`, `time.clearTimeout` over `time.alarm`), linked into a test bootstrapper, and the kernel's check that the bootstrapper registered all three when its `init` returns. The kernel loads `/boot/boot.mhc` as the bootstrapper (here the test bootstrapper) and runs the contract check after its `init`.
  - Check: On the host and both chips: kernel events and applet events through the bus; an emit chain where a subscriber calls back into the emitter and subscribers emit again; a filter where three subscribers edit one buffer in turn; a layer on `evt.emit` that logs; `evt.off` mid-emit; a thousand timeouts at random delays with some cleared; a test bootstrapper that registers only two of the three contract names. With one applet draining the shared pool, a second applet's subscriptions and timeouts keep firing (G9). The logging layer on `evt.emit` also logs `sys.caller` for a kernel event. A test applet takes per-turn buffers from the SDK bump arena (5.3) on every timer firing.
  - Expect: Both kinds of event reach their subscribers in subscription order. The chain runs in order and as deep as `call_depth`. The filter result reflects every subscriber. The logging layer sees kernel and applet events alike. An unsubscribed handler gets no later events. Timeouts fire in order through the one alarm, cleared ones never fire, and lateness matches 5.4's alarm. The incomplete bootstrapper is detected and logged as a failed bootstrapper; the reset and fallback are checked in 5.10. With the pool empty, the second applet misses no event or timeout. `sys.caller` is 0 for the kernel event. The arena resets each turn and the applet's mapped pages stay flat.

- [ ] **5.5d Shell extensions and applet channels**
  - Do: Shell lines dispatched through the layerable `tty.exec` once the bootstrapper is up, with the kernel handler at the bottom. `tty.write`, `tty.attach`, and `tty.input`. The frame of the SDK's hardware-shell module: command parsing, modes that take over a channel until `~.`, `help`, aliases, and the first commands (`names`, `uptime`, `evt emit`, `evt watch`). Each later step that brings up hardware adds that area's commands to the module. `mvm names` wraps the shell's `names`. The `tty.up` and `tty.down` events, the fixed channel ids (1 USB, 2 the BLE shell, applet channels from 3), and `tty.write` to channel 0 reaching every connected channel.
  - Check: On the host and both chips: a test applet that layers on `tty.exec` to add `greet` and passes everything else down; a second layer above it that adds `time`; an unknown command; `ls` typed while both layers are loaded; `help`. A test applet that attaches a channel and feeds lines from a timer, including one full 1 KiB line whose buffer it overwrites right after the call. In `mvm tty`: `names`, `uptime`, `evt emit` to a subscriber, and `evt watch` entered and left with `~.`. Define an alias and use it. Run `mvm names`. A `tty.exec` layer logs `sys.caller` for a shell line. Unplug and replug USB with a subscriber on `tty.up` and `tty.down`, and `tty.write` to channel 0.
  - Expect: `greet` and `time` answer with the right request ids, `ls` still reaches the kernel through both layers, and the unknown command gets `err unsupported`. `help` lists the kernel and layer commands. The attached channel's commands run and their output reaches its callback, and the 1 KiB line runs as it was when fed. Each command answers; `evt watch` prints the emitted event and `~.` returns to the prompt. The alias runs its command, and `mvm names` matches the shell's `names`. `sys.caller` is 0 for the shell line. The subscriber sees `tty.down` then `tty.up` for channel 1, and the channel-0 line reaches every connected channel.

- [ ] **5.6 Traps, reboots, and the syslog**
  - Do: Every trap kind and a blown `instr_per_event` log the trap and reset the node with reason word `trap` or `budget`. `sys.reboot` and `sys.rebooting` (reason and initiator) to everyone as one turn before a planned reboot, then a wait of up to 1 s for the shell channels' pending output. The retained 8 KiB text syslog ring with its magic and checksum, the flush to `/boot/syslog` at boot, rotation to `/boot/syslog.old` at `syslog_max`, and `mvm log --previous`.
  - Check: On both chips: test applets that trap each way, one that spins past `instr_per_event`, and one that layers on a builtin and then traps. Two `sys.rebooting` subscribers that log, followed by `sys.reboot` from an applet and then `mvm reboot` (the shell's `reboot` and `mvm reboot` are built here). A native test hook that hangs until the hardware watchdog fires. A power pull. Enough reboots to pass `syslog_max`.
  - Expect: Each trap and the blown budget reset the node with no `sys.rebooting`, and after the reset `mvm log --previous` shows the trap kind, the applet, the PC, and the event of the turn. Both subscribers get the same `sys.rebooting`, naming the applet as initiator the first time and nobody the second. The `reboot` reply and the subscribers' log lines reach `mvm` before the reset. The watchdog reset keeps the ring on both chips (record if a chip does not). After a power pull the ring is discarded, not misread. The file rotates once.
- [ ] **5.7 VFS and fs builtins**
  - Do: The mount table, path normalization with its 255-byte limit, per-applet fds, the `fs.*` builtins (including `fs.opendir`, `fs.readdir`, and `fs.closedir`, with listing as an SDK helper) over the littlefs `/boot` from M1, and mounting anywhere except over `/boot`. No path permissions.
  - Check: A test applet creates, writes, renames, lists, and reads files, and also seeks, stats, statfs's, syncs, makes and removes a directory, removes a file. A host path-checker fuzz target. On both chips, power-pull during applet writes. A tracing layer on `fs.open` that logs `sys.caller` and passes down.
  - Expect: File operations behave as on littlefs. A second applet reads and rewrites the first one's files and reads `/boot/config.ini`. `..` resolves and never climbs above `/`. A mount over `/boot` is refused, and `/boot` survives power pulls. The fd from the layered `fs.open` lands in the original caller's table. Every `fs.*` call returns the expected result. (Unmount and remount are checked in 5.12, on a test block device, since `/boot` is the only tree here.)

- [ ] **5.8 config meshops and [board]**
  - Do: `config.getStr`, `getI32`, `getBool` and `config.setStr`, `setI32`, `setBool` over every section, `sys.self`, the SDK's `config::mine`, the board meshops (`radio.attach`, `clock.lfxo`, `power.dcdc`), and the SDK's board module, which reads `[board]`, drives `power_on`, and calls them, replacing the M1 hard-coded table. It runs in a test bootstrapper here and joins `applets/boot` in 5.10. Board presets under `profiles/`, applied by pasting one into `[board]` through `mvm mount`. The shell's `cfg get` and `cfg set`. Basic `gpio.configure`, `gpio.read`, and `gpio.write` (the board module drives `power_on` with them; 5.13 adds edge callbacks, labels, the board module's name layer, and the pin map). The kernel's own boot-time read of `[kernel]`, right after `/boot` mounts, including the `code_cache_kb` and `io_scratch` overrides. `config.keys` and the shell's `cfg list <section>`. Kernel delivery of `mem.low` and `mem.ok` through the bus, with the marks read from `[kernel]`.
  - Check: Two instances of a test applet with different sections, each reading its own settings with `config::mine` and the other's with a dotted name, then setting a key in its own section. Boot each board with no `config.ini`, then with its preset. Set `[kernel] code_cache_kb` and `io_scratch`, reboot, and confirm the cache and reserve sizes in `mvm ps --json`. Remove `radio.max_dbm`, then set an unknown `radio.chip`, and change one pin in the preset followed by `mvm reboot`. `cfg list` one instance's section. A test applet drains the shared pool and refills it; then boot with `mem_ok_pct` not above `mem_low_pct`. On an nRF preset that sets `clock.lfxo` and `power.dcdc`, read back the LF clock source and the regulator mode. Run `cfg get` in `mvm tty`. `cfg set` a key in `mvm tty`, then `cfg get` it.
  - Expect: Each instance reads its own values and the other's. A missing or invalid key yields the caller's default. A set is visible to the next read and leaves the rest of the file unchanged. With no `config.ini`, both boards boot with safe defaults and the radio off. With the preset, the board module powers peripherals and the radio attaches. With `radio.max_dbm` removed, or an unknown `radio.chip`, the attach is refused and the radio stays off. Changing a pin in the preset takes effect after `mvm reboot`, with no reflash. The applet gets `mem.low` once and then `mem.ok` once, each with the right `free_pages` and `mark`; the bad marks fall back to the defaults with a log line. Both switched. `cfg get` and `cfg list` return the file's values. The `cfg get` after `cfg set` returns the new value.

- [ ] **5.9 Loader, sys.load, and sys.ready**
  - Do: Loading from `/boot` with hashing, the `min_abi` check, and the `max_ram`/`reserved_ram` checks. The `sys.load` queue with its completion callback, and the bootstrapper module that emits `sys.ready` when every load it queued has reported. The `extra` `key=value` overlay, honoured by the `config.*` meshops and by the kernel's `max_ram`/`reserved_ram` read.
  - Check: A test bootstrapper loads three test applets that log their `init` and `sys.ready` order, an applet whose `min_abi` is newer than the kernel's, and one whose `reserved_ram` does not fit in the pool. On the host and both chips. The test bootstrapper passes `extra` overriding one instance key and its `max_ram`. After `sys.ready`, one applet calls `sys.load` for another whose `init` subscribes to `sys.ready`. One `[applets]` line names a missing file.
  - Expect: `init`s run in order and never nest, `sys.ready` reaches all three, and refused loads are logged with the reason. The instance reads the overridden key, and its cap follows the `extra` `max_ram`. The late subscriber is called at once. The missing file's load callback reports its error, and `sys.ready` still fires.

- [ ] **5.10 Default bootstrapper, fallback, and reset reasons**
  - Do: `mvm cfg`. `applets/boot` with every bootstrapper module (event bus and timers from 5.5c, the hardware shell from 5.5d onward, `sys.ready` load tracking from 5.9, and the board module from 5.8), embedded in each image and written to `/boot/boot.mhc` when missing, `boot_default_hash` in `/boot/.kernel`, the upgrade rule, the failed-bootstrapper fallback, `sys.resetReason` with the retained reason word (`sys.reboot` is from 5.6), and the `FlashCode` reader that runs the embedded default when `/boot` is unwritable or the bootstrapper failed. The default bootstrapper includes the board module, so board bring-up happens before `[applets]` load, and the built-in fallback does the same.
  - Check: On both chips: first boot from an empty `/boot`, a `config.ini` listing `hello`, a custom `boot.mhc` that traps in `init`, one whose bus traps on the first event, and one that registers no `evt.*`; then replace the failed one with a fixed build. An applet that calls `sys.reboot` with a known word. Then boot with `/boot` deliberately unmountable (a corrupted partition). Last, remove `boot.mhc`, force `/boot` read-only through a test hook, and run `cargo xtask guards`. Edit an instance setting with `mvm cfg` and read it back. Also the trap and budget applets from 5.6 and a `mvm reboot`, to produce each reset reason. Flash a kernel with a new embedded default twice: once over an unmodified `boot.mhc`, once over a custom one.
  - Expect: First boot writes the default. `hello` loads from `config.ini`. Each failed bootstrapper resets once with reason `boot_failed`, and the next boot runs the embedded default through `FlashCode`, loads `hello`, and logs the fallback. The fixed build is used on the next boot. `sys.resetReason` returns the known word, and `trap`, `budget`, `reboot`, and `boot_failed` after those resets (`replace` is checked in 5.11). With `/boot` unmountable, the embedded default runs through `FlashCode` with no applets, the hardware shell answers, `mvm ping` and the log report the mount failure, and `mvm ps --json` shows the reserve at the built-in default sizes. With `/boot` read-only, the embedded default runs through `FlashCode`, loads `[applets]`, and code RAM equals the slot cache size (G6). The unmodified `boot.mhc` is replaced and `boot_default_hash` updated; the custom one is left alone.
- [ ] **5.11 Files in use and the rest of the CLI**
  - Do: Files in use: the first write, truncate, rename over, rename away, or delete of a loaded applet's file (not an open) is held by the VFS until the turn or shell line that made it ends; then the kernel emits `sys.rebooting` (reason `replace`) to everyone while every file is intact, runs no other turn, lets the held changes through, and resets the node with the reason word `replace`. Then the full `mvm ps` with `--json`, and `mvm verify`.
  - Check: On both chips: `mvm cp` an applet, add it to `[applets]`, and `mvm reboot`; then remove it from `[applets]` and `mvm reboot`, after which it is not loaded. With shell layers from 5.5d loaded, `mvm cp` a loaded applet and type shell commands during the copy. `mvm cp` a new build over the file of a running applet. `mvm cp` three applets in one command. Save a loaded applet's file through `mvm mount`. Delete a running applet's file. Copy an applet that is not loaded. Open a loaded applet's file read-write without writing. Unplug USB halfway through `mvm cp` of a loaded applet's new build, wait, then plug in and resume. Through a test hook standing in for an applet, write half a new build in place over a loaded applet's file in one turn. `mv` a loaded applet's file to another directory on `/boot`. Flip a byte in a loaded applet's file through a test hook that bypasses the VFS, and run `mvm verify`.
  - Expect: Each command works. Every applet gets `sys.rebooting` with reason `replace` before any byte changes, no turn runs after it, and the node resets once per `commit`, not once per file. After the reset the new build runs, `mvm ps` shows the reason `replace`, and `mvm` and `mvm mount` reconnect by themselves. Copying an applet that is not loaded does not reset, and neither does the read-write open. The unplugged copy resets nothing until it resumes and completes, and then the new build runs. The torn file is refused at boot as corrupt and logged, the other applets run, and copying it again fixes it. `mvm verify` reports the flipped file. The torn in-place write resets at the end of its turn, and so does the move. Each `commit` reply reaches `mvm` before the reset. `sys.resetReason` reports `replace` after the reset. `mvm ps` numbers match `mem.stat` inside the applet.

- [ ] **5.12 Filesystem and block-device providers**
  - Do: `fs.mount` with `littlefs` on an applet block device and with an applet filesystem scope, queued mounts, verb checks, no re-entry, read-only mounting, and failure as a pulled card. `fs.format` and the shell's `format <mount>`. `sys.sleep` under native code: with the kernel's littlefs mid-call into an applet block device, `sys.sleep` waits in place, and a slice that ends there continues until the native call returns. The interpreter nesting level, raised by littlefs calling an applet block device and by any kernel-started call into an applet-provided tree (a load, a replacement's rename, a kernel shell command), with `sys.sleep` waiting in place (idling the CPU, feeding the watchdog) at level 1 or more. Guest-initiated VFS calls into applet filesystem providers are frame-free. littlefs on an applet block device uses the caches the mounting applet allocates with `mem.cmalloc` and passes to `fs.mount` (a buffer and a cache size), with lookahead and littlefs state in the pinned kernel region: one read cache, one program cache, and one per open file. Block-device verbs read and write them directly, and a read that skips littlefs's cache goes through the I/O scratch area in pieces. A slice that ends at level 1 or more feeds the watchdog. Kernel shell commands reach applet-provided trees through nested provider calls, and `cp` runs one chunk per turn. The I/O scratch area (`io_scratch`) carries the normalized path for every provider call and the data and out pointers of every provider verb the kernel starts; I/O buffers are assigned to calls as byte ranges under the kernel-page access rule. A replacement's renames, which run after the last turn, are charged against `instr_per_event`.
  - Check: A test block device backed by its heap, a read-only test filesystem, and a provider that traps mid-operation, on the host and both chips. `format_if_blank` on a test device that already holds data, then `format` on it. `fs.unmount` the test tree and mount it again. The heap-backed test block device sleeps inside its `write` under littlefs, across a slice boundary. A long littlefs operation (a format) on the test block device whose `erase` sleeps for longer than the watchdog timeout in total; and a `fat`-style test provider calling a test block device that sleeps. The test block device reads and writes through the buffer pointer it is passed; a 512-byte `fs.read` and a code-page fill through the test device (reads that skip littlefs's cache); a test applet keeps as many files open on the test mount as its caches allow and interleaves writes and reads across them; with `[kernel] instr_per_event` raised for the test, a block-device `write` busy-loops without `sys.sleep` for longer than 8 s; from the shell, `ls` a path on the test tree and `cp` a file larger than one turn's budget allows from it to `/boot`. `cp` an applet from the test tree over a loaded applet's file on `/boot`; `mv` a file from the test tree to `/boot`; and inside a transfer session, `cp` and `mv` from the test tree, then `commit`.
  - Expect: Both mount and serve files. Re-entry is rejected. The provider's trap resets the node and is in `/boot/syslog`. `format_if_blank` leaves the data alone; only `format` wipes it. After `fs.unmount`, calls on that path return `ERR_NO_MEDIA` until it is mounted again. The write completes and the turn resumes correctly; the main loop is held only for the sleep. The format completes without a watchdog reset; the provider chain's sleep pauses the turn (level 0) and the shell keeps answering. Data round-trips with the driver writing straight into the caches it supplied (no copy), the large reads arrive intact through the I/O scratch area, and each open file reads back exactly what was written to it; the busy loop resets the node neither for `budget` nor by the watchdog; the `ls` lists the tree through a nested call, and the `cp` completes over several turns with no budget reset. The copy over the loaded file resets once with reason `replace`. The cross-mount `mv` leaves a matching target and no source. In the session nothing changes until `commit`, and the moved source is removed at `commit`.

- [ ] **5.13 Device builtins**
  - Do: `gpio.configure` with raw pin specs, labels, and edge callbacks, `adc.open` and `adc.read`, `i2c/spi/uart.open` and transfers (`qspi` is 5.15), `uart.open` receive callbacks, `ERR_DENIED` for kernel-owned resources, the pin map, and reserved-pin checks (the NFC pins unusable until UICR is written, and the octal PSRAM pins on ESP32-S3R8), and `mvm pins`. Reserved pins come from the detected part (FICR, ESP32-S3 eFuse package), not from `[board]`. `gpio.map`, and the hardware shell's `pins`, `gpio get/set/mode`, `gpio watch`, `adc read`, `i2c scan/read/write`, `spi xfer`, `uart send`, and `uart` passthrough mode. Device-open meshops take raw pin specs and an optional label; the bootstrapper board module's layer on them resolves `[board]` names with `config.getStr` and passes the name down as the label.
  - Check: On both chips: blink an LED by board name, read a button through its edge callback (two applets on the same button: a double-click and a long press, made while a long turn runs with other events already queued), read the battery ADC, read an I2C sensor if the board has one, open a kernel-owned pin, and configure one pin differently from two applets. In `mvm tty` on each board: blink an LED with `gpio set`, watch a button with `gpio watch`, read the battery with `adc read`, scan an I2C bus, and pass a GNSS UART through with `uart`. Open the same LED by `[board]` name and by raw pin, and open `i2c.qwiic` from two applets. `uart.read` of bytes already buffered; and in `mvm tty`, `gpio get`, `gpio mode`, `i2c read`, `i2c write`, `spi xfer`, and `uart send`. On a board whose UICR was written by the bench, drive `P0.09` as GPIO.
  - Expect: Real hardware responds. Both button callbacks get the same edge list with timestamps that tell the double-click from the long press, and the button's turn runs in arrival order with the queued events. The kernel-owned pin returns `ERR_DENIED`. The conflict is logged and shows in `mvm pins`. The name and the raw pin give the same pin, `pins` shows the name as its label and the requesting applet as its owner, and both applets get the same bus id. `uart.read` returns the buffered bytes; each shell command gives the expected reply.

- [ ] **5.14 config meshop override**
  - Do: An applet layering on the `config.*` meshops without calling `sys.next`, so it replaces the builtins.
  - Check: Two instances of a test applet read settings while the replacement is loaded, on the host and both chips.
  - Expect: The replacement answers every dotted name for every instance, for gets and sets.

### Storage extensions

These prove that the provider interfaces work for real storage on real boards: extra flash through a block-device applet, and an SD card through a driver applet and a separate FAT applet.

- [ ] **5.15 QSPI builtins**
  - Do: `qspi.open` and `qspi.cmd` over the 1.22 backend, the board module's name layer on `qspi.open`, and the XIAO nRF52840 preset with `qspi.flash.*`. The shell's `qspi id` and `qspi cmd`.
  - Check: A test applet reads the JEDEC ID on the XIAO. Run `qspi id` in `mvm tty`.
  - Expect: The expected ids.

- [ ] **5.16 extra-fs on the XIAO**
  - Do: `applets/extra-fs` as the design plan describes.
  - Check: Install on the XIAO with one `[applets]` line, write files to `/extra`, and pull power during writes. Set `files` to 8 in its section, reboot, and keep 8 files open on `/extra` at once, interleaving writes and reads. Then install the same file on the WisMesh Tag and the T-Deck Plus.
  - Expect: On the XIAO, `/extra` appears with no settings and files survive power pulls. With `files = 8`, all 8 open files read back correctly and `mvm ps` shows the larger cache charged to `extra-fs`. Elsewhere it logs "no chip", registers nothing, and holds only its statics in `mvm ps`.

- [ ] **5.17 SPI clock and chip-select hold**
  - Do: `spi.configure` and the `hold` flag on `spi.transfer`, with per-caller settings reapplied on a shared bus.
  - Check: Two test applets use one bus at different clocks, one of them holding chip select across three transfers, on both chips.
  - Expect: A logic analyser (or the device on the bus) shows each applet's clock and an unbroken chip-select window for the held sequence.

- [ ] **5.18 sd applet and littlefs on SD**
  - Do: `applets/sd` as the design plan describes: SD init over SPI, 512-byte block verbs, busy waits with `sys.sleep` between polls, and staying idle with no card. Loading an applet from `/sd`: a code-page miss there runs the page fill through littlefs and the `sd` driver as a nested guest call, with the slot being filled reserved. `sd` mounts from `init`. During a replacement's freeze, nested driver calls still run, so a held rename on `/sd` goes through; a code-page miss during another fill traps.
  - Check: On the T-Deck Plus and `RSD`, each with a spare card, mount the kernel's littlefs on it with `format_if_blank`, write files, and pull power during writes. Set `files` in `[sd]`, reboot, and keep that many files open on `/sd` at once, interleaving writes and reads. Run the same `sd.mhc` on a board without an SD slot. Put a test applet's `.mhc` on `/sd` (littlefs mode), list it in `[applets]` after `sd`, and run it on an nRF-sized code cache. Then replace it with `mvm cp`. Then load a test applet from `/sd` that layers on `spi.transfer` (breaking the driver policy) and force a code-page miss in it during a fill.
  - Expect: `/sd` mounts as littlefs and survives power pulls. Every open file reads back correctly, and `mvm ps` shows the 512-byte caches charged to `sd`. With no card it logs that and stays idle. Record the card's busy times and, with a timer-driven test applet running during heavy writes, the worst delay before an event is dispatched. This is the latency v1 accepts; 6.3 repeats it with frisbee and the radio. It loads and runs, paging through `sd`, with no slot thrash; the replacement lands and the node resets once. The recursive fill traps, is logged, and reboots the node.

- [ ] **5.19 fat applet on SD**
  - Do: `applets/fat` as the design plan describes, stacked on the `sd` block device. `fat` mounts from `init`. A code-page fill from a FAT mount calls `fat` (and through it `sd`) as a nested call.
  - Check: Copy files onto the FAT card from a PC, read them through MVM on the T-Deck Plus, write new files from a test applet, then read those on the PC. Load a test applet from the FAT `/sd`, listed after `sd` and `fat`.
  - Expect: Both directions read correctly. The PC reports no filesystem errors on the card afterwards. It loads and runs, paging through `fat` and `sd`.

- [ ] **5.20 Card removal and stacked failure**
  - Do: Removal handling through the stack: `sd` returns `ERR_NO_MEDIA`, `fat` passes it on, and the VFS marks `/sd` gone.
  - Check: Pull the card while a test applet writes, reinsert it, then `mvm reboot`. (The radio during card removal is rechecked in 6.3.)
  - Expect: Writes fail with `ERR_NO_MEDIA` rather than hanging, fds go stale, and `/sd` comes back after the reboot. A PC check of the card shows at most the in-flight file lost.

### Network

The kernel's network builtins on ESP32-S3 boards and the simulator. nRF boards have no Wi-Fi, so these builtins are simply absent there and calls return `ERR_UNAVAILABLE`.

- [ ] **5.21 Network builtins**
  - Do: The `net.*`, `dns.*`, `tcp.*`, and `udp.*` builtins from the design plan, with per-request callbacks, over the 1.21 backend, with credentials from the `[net]` section of `config.ini`. `net.connect`, and the shell's `net`, `net configure`, `dns`, `udp send`, and `tcp` passthrough mode. `mvm net` wraps the shell's `net`. `net.sockets` behind `net`. `tcp.listen` and `tcp.close`, and the `net.down` event. The bootstrapper's bring-up calls `net.connect` when `[net]` exists.
  - Check: A test applet waits for `net.up`, resolves a name, opens a TCP connection to an echo server on the Mac, sends and receives, sends UDP both ways, then traps. Run on every ESP32-S3 board, on the simulator, and on one nRF board. In `mvm tty` on an ESP32-S3 board: `net configure` to a second access point without rebooting, `dns`, and a `tcp` passthrough session to a test server. A `tcp.listen` echo server closed with `tcp.close`; drop the access point; in `mvm tty`, `net` and `udp send`; run `mvm net`. Boot a board with `[net]` and no applet calling `net.connect`.
  - Expect: Every operation completes through its callback without blocking, and each callback gets the right `ctx`. After the trap, the server sees its connection close. On the nRF board the calls return `ERR_UNAVAILABLE`. The echo works and the close is reported; `net.down` fires on the drop and `net.up` on return; `net` lists the open sockets; `udp send` arrives; `mvm net` matches the shell's `net`. The board reaches `net.up` by itself.

- [ ] **5.22 A real fetch**
  - Do: A test applet that fetches a small plain-HTTP resource from a server on the Mac on a timer, the BBS-weather pattern without TLS.
  - Check: Run for an hour on an ESP32-S3 board, dropping the access point twice.
  - Expect: Fetches succeed, failures during the outages come back through the callbacks, and fetching resumes after `net.up`. Memory stays flat in `mvm ps`.

- [ ] **5.23 SDK async layer**
  - Do: The SDK's `async` feature: futures over every call that takes a callback, and the small executor that runs inside callbacks. Rewrite the 5.22 fetch applet with `async`/`await`.
  - Check: Run both versions side by side on an ESP32-S3 board and the simulator, and an `async` timer and `test.later` round-trip applet on an nRF board. Compare `.mhc` size, instructions per fetch, and peak mapped pages.
  - Expect: Same behaviour as the callback version. The size and instruction overhead of the executor is recorded, and an applet without the `async` feature is the same size as before.

- [ ] **5.24 Unmodified third-party code (G8)**
  - Do: The SDK's link-level runtime, so library source is never edited for MVM. A C applet template for `mvm new` (`mvm new --lang c`) and C compilation in `mvm build`, both in the pinned image.
    - **C:** an embedded C library (picolibc or similar) built for the applet target and added to the SDK build image. Its `malloc`, `free`, `calloc`, `realloc`, and `aligned_alloc` are replaced at link time with thin calls to `mem.*`, and its `snprintf` and `printf` family with calls to `fmt.sprintf` and `log.printf`. Everything else comes unmodified: `string.h`, `qsort`, `bsearch`, `strtol`, `strtod`, and single-precision hardware-float `libm`.
    - **Rust:** the `GlobalAlloc` over `mem.*`.
    - Both allocators keep shadow counts (allocations, frees, live bytes) in the SDK layer, never in the libraries.
    - Five test applets, each built from unmodified upstream source, and each checking its own answers so no native build is needed:
      - C data structures: a stock linked-list library under random inserts, removes, and traversals, checking its invariants and a running checksum after every step, plus `qsort` and `bsearch` from the C library over generated arrays, verifying order and every lookup.
      - C library: cJSON parsing generated documents of varied size, printing them, re-parsing the output, and checking that the two trees are equal.
      - Rust collections: `Vec`, `String`, `VecDeque`, `BTreeMap`, `HashMap` (with a fixed hasher), `LinkedList`, `Box<dyn Trait>`, `Rc`, `Arc`, and `OnceLock` under a seeded random workload, checked against a simple model kept in the same applet.
      - Rust crates and formatting: `serde_json` round trips of generated documents, and heavy `alloc::format!` use (integers, floats, padding, nested `Debug`), checked against known-answer strings compiled into the applet.
      - A canary applet that fills its own heap with a known pattern and checks it on every callback.
  - Check: `mvm new --lang c` and `mvm build` of the resulting C applet.
    - Each workload runs for an hour next to the canary, on the host, the simulator, and every bench board.
    - After every callback, compare the kernel's allocation counts and live bytes for the applet with the SDK shadow counts.
    - At the end of each workload phase, free everything and run `cargo xtask guards`.
    - Once per run, each workload overruns one of its own heap buffers on purpose.
    - Confirm that each upstream library is unmodified by checking its source hash against the upstream release.
  - Expect: The C project builds to an `.mhc` that loads and runs.
    - Every self-check passes on every board.
    - Kernel and shadow counts are equal at every check.
    - After each free-all, mapped pages return to the statics plus the TLSF control block (G4), and memory is flat over the hour.
    - The canary never changes.
    - The deliberate overrun damages only that applet's heap: it traps or fails its own checks, and nothing else on the board is touched.
    - `.mhc` sizes for each workload are recorded.
    - The G8 test joins CI.
  - Not covered in v1, so the claim stays honest: threads, stdio file streams (`FILE*`), and `setjmp`/`longjmp`. Atomics are covered (RV32IMAFC, DG12 in [contradictions.md](contradictions.md)).

- [ ] **5.25 Transport taps**
  - Do: `tap.rx` and `tap.tx` for UART, shell (`tty`), TCP, and UDP traffic, with metadata and payload for every subscriber. Radio taps are checked in 6.5 and BLE taps in 8.4, once those meshops exist. The hardware shell's `tap` mode.
  - Check: A test network-stats applet counts bytes per transport and checksums the payloads while other test applets use a UART, a TCP socket, and the shell. On the host and both chips (TCP and UDP on the host and ESP32-S3 only; nRF has no network). Watch the traffic with `tap` mode in `mvm tty`.
  - Expect: The counts and checksums match the owners' own. Owners' callbacks still get every byte first, and a node with no tap listener shows no measurable overhead.

- [ ] **5.26 Applet interop on one node**
  - Do: An interop test struct defined once as a C header and mirrored as a Rust `#[repr(C)]` type, with compile-time size and offset assertions on both sides. It mixes `u8`, `i8`, `u16`, `u32`, `u64`, `float`, `double`, a `char` array of odd length, a nested struct, and the padding holes those create. A second, larger struct spans more than one 256-byte page. A C test applet and a Rust test applet each fill the structs from a seed, then exchange them three ways: a registered call (`interop.check`), `evt.emit`, and a file on `/boot`.
  - Check: Each side checks every field the other wrote, echoes the struct back, and compares a SHA-256 of the raw bytes. Run both directions on the host, the simulator, and both chips.
  - Expect: Every field matches, the byte hashes are identical, and the size and offset assertions agree between the C header and the Rust type. The multi-page struct survives being passed by pointer intact.

**Exit check:** Each chip boots from an empty `/boot`, writes its bootstrapper, loads three applets from `config.ini` that call each other, emit events, use files, and blink an LED, all installed and configured with `mvm cp` and `mvm cfg`, and survives a power pull at any point. The XIAO mounts `/extra` from one `[applets]` line, and the T-Deck Plus reads and writes a PC-formatted FAT card through `sd` and `fat`. Every ESP32-S3 board fetches over Wi-Fi from an applet. Guards G1 to G9 pass on every bench board.

---

## M6. Radio service and frisbee on hardware

- [ ] **6.1 Kernel radio service and builtins**
  - Do: The kernel radio service over the M1 driver: TX queue, listen-before-talk, RX with timestamps and signal values, duty-cycle tracking, power clamp to the `max_dbm` passed to `radio.attach`, and wedge recovery failing in-flight `radio.tx` with `cancelled`. `radio.config`, `radio.tx` with its completion callback, and the `radio.rx` event. The hardware shell's radio commands (`radio status`, `radio config`, `radio tx`) and `lora` mode over the radio service; the native `lora` stays in the `hal-test` build. The radio is attached by the bootstrapper's `radio.attach` from 5.8. Kernel `tap.rx`/`tap.tx` emission for radio frames, and `radio.status`. The `radio.configured` event (with `boot` as initiator for the shell's `radio config` and `lora`), and the planned-reboot wait of up to 1 s for queued `radio.tx` before the reset.
  - Check: Send and listen test applets on an nRF board and an ESP32-S3 board. A forced wedge mid-send. Two applets that call `radio.config` with different settings. An `async` send-and-wait applet using the SDK `async` layer from 5.23. In `mvm tty`: `radio config`, `radio tx`, and a `lora` mode session between two boards, entered and left with `~.`. Run the same `lora` exchange bench script against a `hal-test` build and a normal build. `radio status` in `mvm tty`; a burst of `radio.tx` past the duty limit; a send into a channel kept busy by a second board. `mvm tty --freq …` on a normal build, which runs through the radio service.
  - Expect: Frames cross both ways, with the airtime in each `radio.tx` completion matching the formula. The wedge cancels the in-flight send and later frames go through. Back-to-back frames while a turn runs long are all copied out of the chip; over `inbox_radio`, frames are dropped and counted in `mvm ps`. After the two configs, the second is in effect, both changes are logged, and both applets got `radio.configured`. The script passes unchanged on both builds. `radio status` shows the attached chip, settings, and counters; the burst's extra sends complete with `duty`; the send into the busy channel completes with `busy`.

- [ ] **6.2 Frisbee, two players across chips**
  - Do: `applets/frisbee` with explicit acks, the frame layout, the hash chain, and `HELLO`. Frisbee's `init` reads frequency, spreading factor, bandwidth, coding rate, and power with `config::mine` and calls `radio.config`.
  - Check: One nRF board and one ESP32-S3 board for an hour. Two boards with a non-default `[frisbee]` frequency.
  - Expect: Throws alternate with every throw acked and zero hash-chain violations. Both report `radio.configured` with that frequency and play on it.

- [ ] **6.3 Every board a player, retries, and BYE**
  - Do: Jittered retry with a cap, suspect marking, roster expiry, and `BYE` from a `sys.rebooting` subscriber.
  - Check: Every bench board with a radio for an hour, with one power-cycled every ten minutes and one rebooted with `mvm reboot`. Larger player counts come from the simulator in M7 and the Darwin player in M8. On the T-Deck Plus, run heavy SD writes during the game, and once pull and reinsert the card.
  - Expect: The game never stops for more than a few `T` intervals. The power-cycled player rejoins. The rebooted player sends `BYE` from `sys.rebooting`, leaves the roster at once, and rejoins after the reset. On the T-Deck Plus, record the worst event delay and any radio drops over `inbox_radio` during SD writes (the latency v1 accepts), and the radio keeps working through the card pull. The `BYE` sent from `sys.rebooting` is received by the other players, confirming the reboot waited for the queued send.

- [ ] **6.4 Epoch takeover**
  - Do: Lost-frisbee timers and epoch takeover with the tie-break.
  - Check: Pull power from the frisbee holder mid-game, repeatedly.
  - Expect: A new epoch starts within a few `T`, with one frisbee in play afterwards.

- [ ] **6.5 Stats applet**
  - Do: `applets/stats` listening to `radio.rx` next to frisbee. `stats` writes every received frame with `time_us` to its log file, which is the RX trace format.
  - Check: Run on one nRF board and one ESP32-S3 board during the game from 6.3.
  - Expect: Stats logs every frame frisbee receives. A `tap.rx` and `tap.tx` subscriber counts radio bytes that match frisbee's own.

**Exit check:** Every bench board plays frisbee for an hour through power pulls and holder loss, using one `.mhc` installed with `mvm cp`.

---

## M7. Simulator and mesh run

The simulator covers what the bench cannot do cheaply. Each scenario that finds a bug gets a bench reproduction step before the fix counts as done.

- [ ] **7.1 Virtual clock and multi-node runner**
  - Do: Extend the simulated node from 1.0a: `mvm-sim` with a discrete-event clock, many nodes in one process, a seed, and a per-instruction time charge. Fake HAL backends for every trait not already covered. A scripted-peer `net` backend for virtual-time scenarios, in place of host sockets.
  - Check: Two idle nodes for an hour of virtual time. Run a network test applet twice with one seed against the scripted network.
  - Expect: Finishes in well under a second. Same seed, identical logs. Identical logs from both runs.

- [ ] **7.2 Time-on-air and the medium**
  - Do: The time-on-air formula, and the medium: half-duplex, RX matching, collisions with capture, the loss matrix, drops near the SNR floor, CAD, and duty cycle.
  - Check: Time-on-air against Semtech's calculator for six configurations. Unit tests for each medium rule. Compare simulated airtime with the airtime measured on hardware in 1.14.
  - Expect: Matches the calculator and the hardware measurement.

- [ ] **7.3 Simulated radio, watchdog, and faults**
  - Do: A provider of the `radio.*` names on the medium, the 1.3 simulator watchdog and retained RAM moved onto virtual time, and fault injection (stop IRQs, hold BUSY, stall the scheduler).
  - Check: The send and listen test applets from 6.1, and each fault.
  - Expect: Same behaviour as the hardware runs in 6.1 and 1.15.

- [ ] **7.4 Scenarios and mesh run**
  - Do: Scenario JSON, presets, and `mvm run`, on top of the per-node `/boot` and the per-node localhost shell socket from 1.0a.
  - Check: `mvm run scenarios/frisbee-4.json` twice with one seed, and `mvm cp` into a running simulated node. `mvm --node host:a ps` from a separate `mvm` process while the scenario runs.
  - Expect: Byte-identical logs, and the CLI works against simulated nodes exactly as against boards. A host node's `sys.version` reports its scenario node id.

- [ ] **7.5 Loss, hidden nodes, and long runs**
  - Do: Scenarios for 20% loss everywhere, a hidden-node topology, and a day of virtual time.
  - Check: Run each.
  - Expect: The game survives each. Any failure gets a bench reproduction step (for example, attenuators or distance for hidden nodes) before its fix is accepted.

- [ ] **7.6 Replay**
  - Do: RX traces from `stats` (6.5) on boards and from the simulator, the scenario's `trace` field that injects one into a node's medium, and replaying one node from a trace.
  - Check: Replay a node from a simulated run and from a fresh 6.3-style bench run recorded with this step's tracing.
  - Expect: The replayed node reaches the same state in both cases.

**Exit check:** `mvm run` reproduces the M6 game deterministically, survives loss and hidden nodes for a day of virtual time, and replays a node from a real bench trace.

---

## M8. LoRa modem and the Darwin player

- [ ] **8.1 Modem protocol crate**
  - Do: `applets/lora-modem-proto`: parsing and formatting of the `modem` shell lines from the design plan.
  - Check: Round-trip and fuzz tests on the host, and round trips on both chips.
  - Expect: Exact round trips, and no fuzz panics.

- [ ] **8.2 Modem MCU side**
  - Do: `applets/lora-modem-mcu` layering on `tty.exec` to add `modem`, calling the kernel radio builtins, and writing `!modem` lines. `!modem hello` to every connected channel after boot and on each `tty.up`.
  - Check: On an nRF board and an ESP32-S3 board, type `modem config` and `modem tx` by hand in `mvm tty` and watch `!modem rx` from another board. Then fill the TX queue from a script.
  - Expect: Frames go both ways on both chips, with power still clamped to the `max_dbm` passed to `radio.attach`. Other shell commands still work alongside `modem`. A full queue answers `err busy`.

- [ ] **8.3 Wall-clock mode and the modem host applet**
  - Do: Wall-clock time in `mvm run`, a scenario naming a real link, and `applets/lora-modem-host`. Frisbee on Darwin calls the radio from `sys.ready`. The host-node `uart` backend for real serial ports and a host preset whose `[board]` maps the modem's `uart` name to its device path. The board module's resolution of the host keys `uart.<name>.path` and `uart.<name>.usb_serial`, and the host HAL's lookup of a USB serial number. `lora-modem-host` logs its own content hash next to the tag's at startup.
  - Check: Darwin plays frisbee with the other bench boards through one board used as a modem, once with an nRF board as the modem and once with an ESP32-S3 board. Reach the modem once by `path` and once by `usb_serial`.
  - Expect: The Darwin player is a normal player, and its retry timing is logged next to the boards'. Each of its `radio.tx` calls is answered by `lora-modem-host` resolving the caller's reference with `sys.resolve` when the modem reports `!modem txdone`. Darwin's log shows both content hashes.

- [ ] **8.4 Over BLE**
  - Do: The BLE builtins for applets (`ble.advertise` with its accept callback, `ble.scan` with its advertisement callback, `ble.connect` with its link callback, `ble.write`, `ble.disconnect`, and `ble.stop`) on both chips, and the modem link over BLE. The shell's `ble status` and `ble scan`. Kernel `tap.rx`/`tap.tx` emission for BLE, and `ble.status`. The host-node `ble` backend (CoreBluetooth on macOS, BlueZ on Linux) that `lora-modem-host` uses as a central. The `ble.scan` advertisement layout in `mvm-abi`.
  - Check: Repeat 8.3 over BLE. Run `ble status` and `ble scan` in `mvm tty`. A chip test applet advertises with `ble.advertise`; another chip connects with `ble.connect`, writes with `ble.write`, then `ble.disconnect`; then `ble.stop`.
  - Expect: Same result, with the `modem ping` latency histogram recorded. A `tap.rx`/`tap.tx` subscriber counts BLE bytes that match the modem link's own. The link comes up, the data arrives through the accept callback, the disconnect is reported to both sides, and `ble.stop` ends advertising.

- [ ] **8.5 Link faults**
  - Do: Exercise cable pull, laptop sleep, modem reset, and a flooded link.
  - Check: Each fault during a game.
  - Expect: The Darwin player degrades on `ERR_UNAVAILABLE` and rejoins after `!modem hello` and `modem config`. Flooding shows `err busy`, the ring, and the drop counter working.

- [ ] **8.6 Applet interop across hosts and links**
  - Do: Reuse the 5.26 C and Rust interop applets, sized to fit one LoRa frame, with the struct carried as the frame payload. Add a leading type and version byte and a length check, as the SDK docs advise.
  - Check: The C applet on an nRF board and the Rust applet on an ESP32-S3 board exchange structs over LoRa, then swap languages and directions. Repeat with Darwin as one end, over LoRa through the modem and directly over the shell on BLE and USB (a shell command the interop applet adds). Send a version-2 struct to a version-1 receiver.
  - Expect: Every field matches and the byte hashes are identical on every pair of hosts and every link, in both directions and both language pairings. The version-1 receiver rejects the version-2 struct by its version byte instead of misreading it.

**Exit check:** Darwin plays frisbee with the other bench boards through one board over BLE, survives cable pulls and sleep, and logs both sides' content hashes. C and Rust applets exchange structs byte for byte between nRF, ESP32-S3, and Darwin.

---

## M9. Paging, chaos, and soak

- [ ] **9.1 Code-page cache tuning**
  - Do: Code has been paged since 2.7. Measure the slot cache under real load: cache size, the PC pin released at `ecall`, and fill time, including a provider chain (app → `fat` → `sd`) deeper than an nRF-sized cache. Choose the kernel's cache-sizing rule from detected memory: nRF52840, ESP32-S3 without PSRAM, and ESP32-S3 with the cache in PSRAM.
  - Check: Frisbee, stats, and the network applet together on every board, at the chosen default and at an nRF-sized cache on one ESP32-S3 board. Page faults per minute, fill time per applet, and on boards without PSRAM the page pool left for applets.
  - Expect: Numbers recorded and the sizing rule chosen and written into the design plan. On ESP32-S3 boards with PSRAM, faults stop after warm-up. The nRF-sized run behaves like an nRF board. Prefetch or pinning only if the numbers call for it. G6 still holds. The deep provider chain runs on the smallest cache without deadlock.

- [ ] **9.2 Chaos, trap cases**
  - Do: `applets/chaos` with the hostile actions from the design plan.
  - Check: Next to frisbee on both chips.
  - Expect: Each action except the header scribble and the `max_ram` leak (which gets `ERR_NO_MEMORY`) traps and resets the node, and the trap is in `/boot/syslog`. The scribble corrupts only chaos's heap. Frisbee rejoins after each reset.

- [ ] **9.3 Chaos, memory pressure**
  - Do: Frisbee with `reserved_ram`, chaos with no limits, leaking until the shared pool is empty, while frisbee keeps playing.
  - Check: Both chips, with the pool held empty for several minutes.
  - Expect: Chaos sees `mem.low` then `ERR_NO_MEMORY`. Frisbee keeps receiving `radio.rx` and throwing the whole time (G9), and never misses an allocation.

- [ ] **9.4 Chaos, reset**
  - Do: Chaos reboots the node through `sys.reboot` with a known word.
  - Check: Both chips.
  - Expect: Frisbee sends `BYE` from `sys.rebooting`, which names chaos as initiator. `sys.resetReason` returns the word after reboot, and frisbee rejoins.

- [ ] **9.5 Middleware on radio.tx**
  - Do: A test applet layers a 30% drop filter on `radio.tx` (resolving dropped frames' callback references itself and passing the rest down with `sys.next`), and a second layers an airtime logger on top. Then the filter traps on purpose.
  - Check: During a game on every bench board, with `mvm names` before and after the trap.
  - Expect: `radio.tx: logger → filter → builtin` while both run, and loss goes up. The trap reboots the node and is in `/boot/syslog`, and after the reboot both layers are back and the game resumes. Every frisbee `radio.tx` callback fires exactly once while the layers run.

- [ ] **9.6 Overnight soak**
  - Do: Every bench board as a player, except one nRF board serving as the Darwin player's modem. The T-Deck Plus also runs stats, writing its RX trace to the FAT card (its `trace` setting set to a path on `/sd`). The XIAO nRF52840 keeps `/extra` mounted. One ESP32-S3 board runs the network test applet from 5.22 against a server on the Mac. Add a churn script (replace frisbee's file on one board every few minutes, which resets it, and `mvm reboot` another), and RX traces recorded. One player board also runs `chaos` with the 9.2 trap cases.
  - Check: Run overnight and check the design plan's soak pass bar line by line.
  - Expect: Every line holds. Any failure becomes a new step here, with its replayed trace. Traces join the interpreter fuzz corpus.

**Exit check:** The overnight soak passes the full pass bar on the whole bench plus the Darwin player.

---

## M10. Register-level radio simulator

- [ ] **10.1 SX1262 register-level simulator**
  - Do: A simulated SX1262 at the SPI-command level behind the same `embedded-hal` traits as the real chip.
  - Check: The real driver against it in `mvm run`, including the probe and wedge tests.
  - Expect: The driver that runs on both chips passes the simulator's frisbee scenarios.

**Exit check:** The real SX1262 driver passes the simulator's frisbee scenarios.

---

## Open questions

Each one names the step expected to answer it.

- Can one nRF image serve S140 6.1.1 and 7.3.0, or should every board move to one SoftDevice? (0.6)
- Does `nrf-softdevice` support that SoftDevice, or do we need the SoftDevice Controller and a Rust BLE host? (0.6)
- Do the nRF bootloaders tolerate a running watchdog? (0.9)
- Is the bootloader's board id readable from the application? (0.10)
- Which littlefs crate, and how big is it per family? (1.7)
- Do flash writes cost received frames on either family? (1.16)
- How mature is BLE on the ESP32-S3 in Rust, and does it coexist with Wi-Fi in the RAM left over? (1.20, 1.21)
- How much RAM does the Wi-Fi stack take, and how much page pool is left on boards without PSRAM? (1.21)
- How fast is the interpreter on each family, and what should the default `instr_per_tick` and `instr_per_event` be? (2.8)
- How much RAM does the per-partition TLSF control block cost? (4.7)
- Does `embedded-sdmmc` fit as the FAT layer over our block-device verbs, and how big is it as an applet? (5.19)
- Is the code-page fault rate low enough without prefetch? (9.1)
- Do the shipped applets (everything but `boot` and `tests`) move to their own repo, built only against the published SDK and applet image? Decide at M5's exit, once the ABI has settled.

## Dropped steps

Move dropped steps here with a one-line reason.

- **4.9 Unload and partition reuse.** Dropped 2026-11-01: nothing unloads while the node runs (traps reboot), so partition ids are never reused before a reset.

## Results log

Short dated notes that do not belong to a single step: decisions, surprises, measurements.

- 2026-10-31: Plan created from the design plan as of commit `664dad7`.
- 2026-10-31: Added drift guards G1 to G8 with permanent tests. Code paging moved from M9 to 2.7, so guest code is never copied into RAM except an image that fits in one page. Added 2.9 (code larger than RAM runs), 4.12 (memory guards), and 5.24 (unmodified Rust and C allocator users).
- 2026-10-31: Overrides became middleware: per-name stacks with `sys.next` (5.5b), spliced on unload, shown by `mesh names`.
- 2026-10-31: Completions moved from events to per-request callbacks with a `ctx` word and completion handles (5.5a), with an optional SDK `async` layer (5.23). Events stay for broadcasts.
- 2026-10-31: M1 split into one step per HAL capability with a board matrix (simulator plus seven boards). Added the XIAO nRF52840 (on the bench), Heltec V3, Heltec V4, and XIAO ESP32S3 (planned). Added Wi-Fi and TCP/IP: native on ESP32-S3 in 1.21, builtins in 5.21 and 5.22.
- 2026-10-31: Bench recorded: WisMesh Tag, RAK4631 on RAK19002, T-Deck Plus. Board-count steps resized to three boards. XIAO steps wait for a XIAO.
- 2026-11-01: Added to the bench: Heltec Mesh Node T096 (`T96`) and RAK4631 on RAK19007 with a RAK15002 microSD module (`RSD`). Heltec V3 arrived. `RSD` gives the `sd` applet an nRF board with a card and an SD bus apart from the radio (5.18).
- 2026-11-02: `benallfree/mvm` is public and MIT-licensed, so package managers can fetch releases: `mvm` goes to a Homebrew tap, a signed apt repository on GitHub Pages, and winget, with Install and Upgrade sections in its notes (0.3b).
- 2026-11-02: Release units are `mvm`, `firmware`, `firmware-builder` (was `firmware-builder-image`), `sdk`, and `applet-<name>` for each applet (`applet-boot` for the bootstrapper); Docker Hub keeps the `mvm-` prefix (`benallfree/mvm-firmware-builder`, `benallfree/mvm-sdk`). The 0.1.0 builder release moved to the `firmware-builder-v0.1.0` tag.
- 2026-11-02: The SDK is one Docker image, `benallfree/mvm-sdk` (`docker/sdk/`), holding the toolchains, crates, templates, and the packer (`mvm-pack`), so an applet's `sdk` pin fixes every byte of its `.mhc`. The image's `new` and `build` commands are its public interface; `mvm new` and `mvm build` wrap them, and an applet builds with plain `docker run` (2.1, 2.6b, 2.6c, 2.6d).
- 2026-11-02: The CLI is `mvm`, not `mesh` (`apps/mvm`, `mvm.toml`, archives `mvm-<version>-<target>`), so "mesh" means only the applet call and the network; the firmware release unit is `firmware` (`mvm-firmware-<version>-<board>.uf2`/`.bin`), and `sys.version` reports the firmware version. `mesh-v0.0.1` was withdrawn. Earlier entries and finished steps keep the old names.
- 2026-11-01: The firmware release unit is `mvm`, not `kernel`: kernel plus embedded `boot.mhc`, built by a given firmware builder, with its own semver and the component versions in its notes. `boot` is its own unit (the `.mhc` alone). The kernel keeps a version but no release. An `mvm` release follows any change to its bytes, kernel or not. `sys.version`, `ping`, and `mesh ps` report the mvm version too (0.3a, 0.7).
- 2026-11-01: Renamed so *firmware* only means what goes on a board: the firmware build image is the *firmware builder* (`docker/firmware-builder/`, `benallfree/mvm-firmware-builder`, release unit `firmware-builder-image`, `cargo xtask firmware-builder`), and the applet build image is the *applet builder* (`docker/applet-builder/`, `benallfree/mvm-applet-builder`, inside `sdk`). Earlier entries and finished steps keep the old names (0.3a, 2.1).
- 2026-11-01: CI is live on GitHub Actions (0.3), and the firmware image is public on Docker Hub as `benallfree/mvm-firmware:0.1.0`, used by digest. Firmware bytes are now proven identical on a real x86 machine and the Mac. A red CI run blocks the next step (0.3a).
- 2026-11-01: Images are releases: `firmware-image` is its own unit, pushed to Docker Hub as `benallfree/mvm-firmware`, and each `sdk` release pushes `benallfree/mvm-sdk`, both multi-arch and used by digest. One `sdk` unit covers Rust and C. `mesh.toml` gains `sdk`, the SDK release an applet builds with, and `mesh` resolves it to that image's digest (0.3, 0.3a, 2.6d). Firmware builds pin `SOURCE_DATE_EPOCH` to the commit time and image a stripped ELF, so they are reproducible across hosts (0.2a, 2.1, 2.6e).
- 2026-11-01: Releases: the `kernel`, `sdk`, `mesh`, and each shipped applet are release units with their own semver, changelog, and `<unit>-vX.Y.Z` tag, published as GitHub Releases; the ABI version is `mvm-abi`'s and ships in `sdk` and `kernel` releases. `mesh` runs on Windows too, and `mesh mount` serves WebDAV on every system in place of NFS (0.3, 0.3a, 1.8a, 1.20).
- 2026-11-01: Bench facts recorded (0.1). The nRF boards split between S140 6.1.1 and 7.3.0 (`XN`), so 0.4 builds one nRF image per SoftDevice until 0.6 decides. EnvyBoot, on `TAG` and `T96`, counts as the stock Adafruit bootloader. `HV3` is told apart by its MAC, since its USB serial is generic (0.4, 0.5, 0.6, 0.7).
- 2026-10-31: Added XIAO extra-flash and T-Deck Plus SD steps to M1 and M5, with separate `sd` and `fat` applets.
- 2026-10-31: Restructured around the hardware rule. Native bring-up on both chips and a VM spike come first. The ESP32-S3 is a first-class target from M0, not a late milestone. The simulator moved after hardware frisbee.
- 2026-10-31: Applet signing is back in: Ed25519 developer keys from `mesh keygen`, signing in `mesh build`, a `[trust]` section and `mesh trust` on the node, and `ERR_UNTRUSTED` at load (2.6b, 2.6d, 3.4, 5.9). The kernel `install` routine and its code directory are gone; installing is copying a signed `.mhc` and listing it in `[applets]`.
- 2026-10-31: Applet signing moved to the icebox: no keys, no `[trust]`, no `mesh keygen` or `mesh trust`, no `ERR_UNTRUSTED`, and no signature block. The loader's content hash is the only check on code.
- 2026-11-01: A pin record stops taking edges once its turn starts. The completion pool is for one-shot completions only and lives in the reserve; `sys.load` callbacks run from the load queue, and resource callbacks (`up`, `accept`, `closed`, `down`) are ring records. Events are lost only over a source's cap. 1.5b and 1.9 build the edge records, 5.6 builds the shell drain before a reset (1.5b, 1.9, 1.24, 4.8, 5.5a, 5.6, 5.11).
- 2026-11-01: v1 is not realtime: turns run to completion and events may be late but are never lost or reordered; non-blocking I/O, an urgent lane, and preemptive contexts are iceboxed together. Pin edges are timestamped records in the ring, batched per pin while newest; the callback gets the edge list and the `count` word is gone. Completions use a fixed record pool outside the ring, and the scheduler runs the alarm when due, else the oldest record or completion. Moving a loaded applet's file away resets too, and the shell's output drains before a reset (1.8, 1.24, 5.5a, 5.11, 5.12, 5.13).
- 2026-11-01: Transfers are sessions: `put`, data-only `append`, and `commit`, which renames every staged `.part` at once. The node-side `cp` goes through `.part` and works across mounts, one chunk per turn; `mv` is a rename on one mount and `cp` plus `rm` across mounts. A change to a loaded applet's file resets the node when the turn or line that made it ends, so `replace_settle_ms` and the long frozen VM are gone. Completion records are allocated when the result arrives; pending flags run before the next record, one turn per pin edge with a `count` word. An unmountable `/boot` runs the embedded bootstrapper (1.5b, 1.8, 4.8, 5.1, 5.5a, 5.5d, 5.10, 5.11, 5.12).
- 2026-11-01: The inbox is a byte ring (`[kernel] inbox_kb`, replacing `inbox_slots`): each event takes exactly its size, allocated and freed in order; source caps count bytes. A shell line is at most 1 KiB. littlefs's lookahead and state are pinned and uncharged. An unmountable `/boot` still gets a reserve from the built-in defaults. 2.7 relocates statics only; 5.3 checks `mem.cmalloc`; 5.18 checks `sd`'s `files` (1.5, 1.5b, 1.24, 2.7, 4.8, 5.3, 5.18).
- 2026-11-01: A normalized path is at most 255 bytes, which sizes the `io_scratch` default. The `extra-fs` and `sd` mounts pass their caches and read a `files` setting; several open files are checked in 5.12 and 5.16 (4.8, 5.7, 5.12, 5.16).
- 2026-11-01: Rodata is loaded into RAM with the statics and relocated once at load; only instruction fetches go through the code cache, so native code reading a guest buffer never triggers a page fill. Code pages carry no relocations. Large constant data goes in files (2.1b, 2.6b, 2.9, 9.1).
- 2026-11-01: `mem.cmalloc` returns physically contiguous, page-aligned memory. Drivers allocate their littlefs caches with it, so a cache may span pages (512-byte SD sectors); it is the one buffer native code holds across events (4.7, 5.3, 5.12).
- 2026-11-01: Drivers supply their own littlefs caches to `fs.mount` (a buffer and a cache size of at most a page), sized from their device and optionally overridable in their config section; `fs_cache_kb` is gone. Reads that skip littlefs's cache go through the I/O scratch area. `/boot`'s caches are native. The reserve is set aside after `/boot` mounts and `[kernel]` is read, with the stack at offset 0 so partition `0x01` catches overflow (4.8, 5.8, 5.12).
- 2026-11-01: The kernel reserve is one contiguous block mapped linearly into the kernel partition, holding the stack, inbox, littlefs caches, and an I/O scratch area, so drivers write straight into kernel buffers; bounce buffers are gone. The code cache stays out of guest space (4.8, 5.12).
- 2026-11-01: Bounce buffers cover every kernel-started provider verb and every provider path, come from the reserve, and join the kernel-page access rule; direct shell lines on applet trees run only between turns; kernel-started calls outside a turn are charged against `instr_per_event` (5.12).
- 2026-11-01: Confirmation-round fixes: 5.12 builds the widened nesting rule, bounce buffers, the level-1 watchdog feed, and direct shell lines on applet trees; the recursive-fill test uses a misplaced layer; `fat` mounts from `init`; `stats` has a `trace` path setting.
- 2026-11-01: Final review fixes: any kernel-started call into an applet-provided tree (fill, load, frozen rename, direct shell line) is a nested call; block-device verbs get a kernel-partition bounce buffer; a slice end at level 1 feeds the watchdog; the driver policy covers layers on names drivers call.
- 2026-11-01: Drivers mount from `init` (loads run one at a time in `[applets]` order), so later applets can load from their trees; a code fill through `fat` is a nested call; the freeze lets nested driver calls run (5.5d, 5.18, 5.19).
- 2026-11-01: "Applets that serve a mount live on `/boot`" is a documented policy only; a code-page miss during another fill traps. Marking and enforcing drivers is in the icebox.
- 2026-11-01: Loaded applets may live on applet-provided mounts again: a code-page miss there is an implicit page-fill call through littlefs and the driver, with the target slot reserved; applets that serve a mount live on `/boot`; a freeze stops new turns, not nested driver calls (5.18). A wait at nesting level 1 or more is in place and costs nothing extra, so the `mesh ps` counter and the storage-task icebox entry are gone.
- 2026-11-01: Interpreter nesting level: meshop calls never recurse natively; only littlefs on an applet block device raises the level; a turn pauses only at level 0, and `sys.sleep` at level 1 or more is a blocking wait that feeds the watchdog; native shell lines serve only kernel-native trees (5.5, 5.12). The storage task that would run littlefs at level 0 replaces the icebox's "VM on its own stack".
- 2026-11-01: Focused review design fixes: frisbee frames carry a type and version byte and 64-bit node ids; host `uart.open` reports `down`/`up` and reopens by USB serial; `tty.write` has a `no_tap` flag; `stats` writes `rx.trace`; the kernel's `[net]` read is a stated bypass; Darwin converts tag time to host time.
- 2026-11-01: Focused review fixes in the plan: whole-file staging for every mount write (1.8a), the localhost socket for host nodes (1.0a, 7.4), `sys.sleep` under native code (5.12), the device id (5.2), `tty.up`/`tty.down` and fixed channel ids (5.5d, 8.2), Wi-Fi joined at boot (5.21), frisbee's `radio.config` (6.2), `stats` traces and the scenario `trace` field (6.5, 7.6), and content-hash logging on Darwin (8.3).
- 2026-11-01: Loaded applets live on kernel-native storage only (`/boot`, internal flash); applet-provided mounts hold data. A `sys.sleep` under native code holds the main loop. Running the VM on its own stack is in the icebox.
- 2026-11-01: Logic-and-feature review fixes: bump-arena check moved to 5.5c (turns exist), the soak runs chaos (9.6), `radio.configured` and the planned-reboot radio wait built in 6.1, C projects in `mesh new`/`mesh build` (5.24), UICR written by the bench to free the nRF NFC pins (0.5, 5.13), and 7.6 records its own trace.
- 2026-11-01: Hardening removed: no `ERR_CORRUPT` or `ERR_INVALID`, no refusal of bad instance names or duplicate pins, no named codes for `sys.next` without a layer or VFS re-entry, and the error-code and misuse checks added in the sixth to eighth reviews are gone from 5.2, 5.5b, 5.8, 5.9, 5.12, and 5.13. Applets are trusted; proper use and design cover these.
- 2026-11-01: Eighth review fixes: unmount checked in 5.12, 1.0's SIM cell deferred to 1.0a, `sys.caller` 0 checked in 5.5c and 5.5d, `cfg set` checked in 5.8, the SDK bump arena (5.3), and a scripted-peer network for deterministic scenarios (7.1). Design: queued kernel events stated, shell error codes are `ERR_*` names, bad instance names refused.
- 2026-11-01: Seventh review fixes: a rule that each step exercises what it builds; error codes asserted where defined (`ERR_DENIED`, `ERR_UNSUPPORTED`, `ERR_INVALID`, `ERR_NO_MEDIA`, `ERR_CORRUPT`); coverage for the remaining `fs.*`, net, radio, BLE, and shell commands; a native `test emit` event source for chip runs (5.4, 5.5c); the M4 exit runs every guard so far.
- 2026-11-01: Sixth review fixes: `mem.low`/`mem.ok` delivery tested (5.8), `ERR_BUSY` for exhausted peripherals and completion records, `ERR_DENIED` for locked names and `sys.next` without a layer, `ERR_UNSUPPORTED` for missing drivers, a refused `sys.load` still completes so `sys.ready` fires (5.9), the shared radio SPI bus exception, and the one-word out-pointer rule.
- 2026-11-01: Fifth review fixes: bus names resolve from `[board]` sub-keys; host `uart.open` accepts `usb_serial=`; `ERR_BUSY` for exhausted peripherals; event buffers carry `initiator`; steps added or checked for `config.keys` and `cfg list` (5.8), `fs.format` (5.12), the `sys.load` `extra` overlay (5.9), the bootstrapper upgrade rule (5.10), and host serial resolution (8.3).
- 2026-11-01: Fourth review design fixes: every callback is `callback(ctx, ptr, len, words…)` throughout; `sys.register(name, handler, ctx)` with handlers called as `handler(ctx, argp, argc)`; argument encoding rules (strings are `ptr, len`, optional arguments passed as 0); the watchdog timeout and the syslog size are compiled constants; new `fs.format` and `config.keys`; `mem.free(0)` is a no-op; chaos keeps its progress across its own resets.
- 2026-11-01: Fourth review fixes: a direct test loader until `sys.load` (5.1), the kernel's load of `boot.mhc` in 5.5c, `mesh names` in 5.5d, the name layer on `qspi.open` (5.15), host-node serial and BLE backends for the modem (8.3, 8.4), the stub loader in the simulated node (2.7), shell checks for built commands, G1/G4/G6 wording, and the TLS question removed (decided).
- 2026-11-01: `[board]` names are resolved by a bootstrapper layer on the device-open meshops, which take only raw pins plus an optional label, so the kernel never reads `[board]` (T1, 5.13).
- 2026-11-01: Third review fixes: callback references are two words and every callback is `callback(ctx, ptr, len, words…)`; `evt.off` takes the ctx; new `radio.status`, `ble.status`, and `net.sockets` back their shell commands; the kernel reports the detected part (not a preset) in `sys.version` and `ping`; detection and the `[kernel]` read get steps (5.1, 5.8); basic `sys.register`, `alloc`, and `gpio` move earlier (5.2, 5.3, 5.8); tap emission for radio and BLE is built (6.1, 8.4); hal-test commands use hardware-shell syntax (1.9, 1.18, 1.23).
- 2026-11-01: The manifest `kind` field is gone (R4). The hal-test native shell commands share the hardware shell's syntax, and one bench script runs on both builds (1.5a, 6.1).
- 2026-11-01: Hardware shell (R3): every shell command beyond the recovery core is in a bootstrapper SDK module, as thin wrappers over meshops, with commands and modes (`lora`, `uart`, `tcp`, `gpio watch`, `evt watch`, `tap`) for every hardware area. New meshops `sys.names`, `gpio.map`, and `net.connect`. Each hardware step adds its commands (5.5b, 5.5d, 5.8, 5.13, 5.15, 5.21, 5.25, 6.1, 8.4).
- 2026-11-01: Crystal pins are no longer kernel-owned; protecting board pins stays in the icebox (R2).
- 2026-11-01: The kernel never reads `[board]` (R1): it detects RAM, PSRAM, and the part (FICR, ESP32-S3 eFuses) itself, sizes the code cache from that (`[kernel] code_cache_kb` overrides), and reserves chip pins on its own. Presets carry only what the bootstrapper uses (5.13, 9.1, G6).
- 2026-11-01: Second review fixes: stale removed-design text in both plans; M5 order stated as kernel, bootstrapper modules, remaining kernel steps, then `applets/boot`; `mesh reboot` built in 5.6; replace checks in 5.11; SD latency with frisbee moved to 6.3; G4/G6/G9 wording; 4.9 recorded as dropped; SIM `/boot` is a file-backed block device; SX1262 SIM cells deferred to M10.
- 2026-11-01: Added 1.0a, a minimal simulated node reachable as `--node host:<node>` with a virtual shell stream and an ideal radio, so M1's `SIM` cells (including 1.8a's mount check) have a node to run against; 7.1 and 7.4 extend it (PS8, PS13).
- 2026-11-01: Added 0.2a, a firmware build image pinned by digest with Espressif's Xtensa toolchain and stable Rust for nRF; CI and `cargo xtask bench` build kernels in it and flash from the host with pinned tools (PS6).
- 2026-11-01: `mesh` is a full user tool on macOS and Linux (PS5): per-OS USB serial (IOKit, udev) and BLE (CoreBluetooth, BlueZ) backends, everything else portable, and CI building both (0.3).
- 2026-11-01: Sequencing fixes: M1 and M2 run in sequence (PS1); M0 does blink and the bench script before the SoftDevice and ESP32-S3 layout steps (0.4–0.7, PS7); 2.9 runs one applet at a time with the two-at-once check in 5.1 (PS2); 5.23 uses a timer round trip and 6.1 adds the async radio check (PS3); radio and BLE taps move to 6.5 and 8.4 (PS11).
- 2026-11-01: The instruction set is RV32IMAFC with the `ilp32f` ABI: single-precision floats run on each chip's FPU (DG23 follow-up). Logging is text: `fmt.sprintf` and `log.printf` are native kernel meshops (overridable), format ids and string tables are gone, and the syslog is an 8 KiB text ring on every device (1.3, 2.1b, 2.3, 2.6a, 2.7, 2.8, 5.3, 5.6, 5.24).
- 2026-11-01: `mem.low` uses hysteresis: it fires below `mem_low_pct` of the shared pool (default 10) and re-arms above `mem_ok_pct` (default 20), when the new `mem.ok` event fires (DG22, 4.8). The allocator stays in the kernel.
- 2026-11-01: Config meshops are `config.getStr`/`getI32`/`getBool` with a default at the call site and matching `config.set*`, all kernel builtins that the bootstrapper or an applet may layer on. The file is read leniently and never rejected whole; the kernel writes it only on a `set` call, keeping its own state in `/boot/.kernel`. Manifest settings defaults are gone (DG21, 3.4, 5.8, 5.10, 5.14).
- 2026-11-01: `sys.sleep` pauses a turn in place for chip-specific busy waits; `qspi.waitReady` is gone, and drivers poll with their own checks (DG20). v1 accepts the latency of a long turn; non-blocking I/O is in the icebox (5.4, 5.15, 5.18).
- 2026-11-01: Board bring-up moved to the bootstrapper (DG19). The kernel knows how to drive chips and offers `radio.attach`, `clock.lfxo`, and `power.dcdc`; the bootstrapper reads `[board]`, drives `power_on` pins, and attaches the radio, so board support updates by replacing `boot.mhc`. The kernel never acts on `[board]` by itself (0.1, 1.12, 5.8, 5.10, 6.1).
- 2026-11-01: The page pool, inbox, and stack stay in internal SRAM on every board; PSRAM holds only the code cache. A PSRAM heap moved to the icebox (DG18, 0.5).
- 2026-11-01: Pins used by in-package octal PSRAM or flash (GPIO33–37 on ESP32-S3R8, as on the XIAO ESP32S3) are reserved, and presets record the PSRAM kind (5.13).
- 2026-11-01: The code-page pin is released at `ecall` and refetched on return, so call chains deeper than the cache cannot deadlock (DG17). Slot count is `code_cache_kb` × 2, set in the board preset and overridable in `[kernel]` (9.1).
- 2026-11-01: One shared address space (DG16). Each applet gets an image partition (code and rodata at `0x01_0000`, statics at `0x80_0000`) and a heap partition, assigned at load. Applets build with `medany`, so code is PC-relative; the packer lists absolute data words and the loader relocates them in statics at load and in code pages at each fill. Constants and statics can be passed to any applet (2.1b, 2.6b, 2.7, 5.5).
- 2026-11-01: The code-page cache size is per board (`[board] code_cache_kb` in the preset, with family defaults): a few slots on nRF52840, a large cache on ESP32-S3, in PSRAM where present, so most applets stay cached. Pages stay 512 bytes everywhere (G6, 0.5, 9.1).
- 2026-11-01: Execution priorities (DG15). Driver tasks run in an interrupt-priority executor above the main loop, with the copy-out path kept running through flash operations (SoftDevice-scheduled partial erases on nRF, IRAM on ESP32-S3). VM slices stay counted in instructions (`instr_per_tick`), sized from 2.8 so a slice plus the slowest native call stays far below the watchdog timeout (1.5b, 1.16, 2.8).
- 2026-11-01: The instruction set is RV32IMAFC (`riscv32imafc-unknown-none-elf`), so atomics work and `Arc`, `OnceLock`, and `<stdatomic.h>` compile unmodified (DG12, 2.2, 2.3, 5.24).
- 2026-11-01: Callbacks are plain references (instance id, function address, `ctx`) passed as ordinary arguments; `sys.complete` became `sys.resolve`, `sys.release` and the kernel's handle table are gone. Anything callable through `mesh()` is called a meshop (5.5a, 5.5b, 5.5c, 8.3, 9.5).
- 2026-11-01: Three layers of responsibility: kernel (hardware and the inbox), bootstrapper (purely synthetic fundamentals), applets (everything else). The kernel keeps `time.now` and one `time.alarm`; timers (`setTimeout`, `clearTimeout`), the bus with `evt.off`, `sys.ready` (via `sys.load` completions), and shell niceties move to bootstrapper modules. The shell's recovery core, `ps`, and `log` stay in the kernel. Added `adc.open`, `mem.malloc` alignment, and `fs.opendir`/`readdir`/`closedir` (DG11). `lora` chat is an applet outside the `hal-test` build (1.2, 5.3, 5.4, 5.5c, 5.7, 5.9, 5.13, 6.1).
- 2026-11-01: The LoRa modem is a shell extension (DG9): `lora-modem-mcu` adds a `modem` command on `tty.exec`, with `!modem` lines for RX and TX done and `err busy` instead of credits. USB and BLE carry only shell lines, and applets lost the raw `usb.*` calls and events (8.1, 8.2, 8.4, 8.5, 8.6).
- 2026-11-01: The radio has one last-writer-wins configuration with a `radio.configured` event, and receive is continuous (DG8, 6.1). Duty-cycled receive moved to the icebox.
- 2026-11-01: `sys.next` keeps the original caller (DG4), so builtins under middleware act for the original caller. `config.get` takes a dotted `section.key` for any section of `config.ini`, with manifest defaults for instance sections; `sys.self` returns an applet's own instance name (5.5b, 5.8, 5.14).
- 2026-11-01: Added 1.0, the hello world: `ping` typed over USB gets an answer from every board, before anything else in M1.
- 2026-11-01: The management interface is a text shell (1.5): request-id lines, kernel commands for recovery (`ls`, `stat`, `cat`, `append`, `mv`, `rm`, `mkdir`, `hash`, `ps`, `log`, `reboot`, `lora`, `test`), and every `mesh` file and node command is a wrapper. Dispatch goes through the layerable name `tty.exec`, so applets add commands as middleware, and `tty.attach` lets applets add channels such as LoRa (5.5d). The binary COBS management protocol is gone; COBS stays for the modem link.
- 2026-11-01: M1 gains a `hal-test` kernel build with an on-device HAL test suite run from an interactive `mesh tty` shell and by the bench (1.5a), the native event inbox and main loop (1.5b), and an inbox load test with the full suite (1.24). 1.3 checks a retained region for the syslog. `mesh tty` is now the node's native shell, with LoRa chat as its `lora` command. No interpreter work starts until M1's exit check passes.
- 2026-11-01: Minimal kernel intrinsics (DG26). The kernel's whole event job is the inbox, then `evt.emit`: the bootstrapper must provide `evt.on`, `evt.emit`, and `evt.has` (an SDK bus module), the kernel keeps no listener lists, and a bootstrapper that traps or misses the contract falls back to the embedded default on the next boot. `sys.complete` is synchronous. `sys.reset` became `sys.reboot`, and `sys.stopping` became one `sys.rebooting` broadcast with reason and initiator. `mem.scratch` is gone. Added 5.5c; reworked 5.4, 5.5a, 5.6, 5.10.
- 2026-11-01: Event model and failure handling. Applets are event handlers: external events wait in a kernel inbox with per-source caps, and each one starts a turn that runs in time slices of `instr_per_tick`. `evt.emit` is synchronous and re-entrant. Fuel is replaced by a per-turn `instr_per_event` budget with no blame. Any trap or blown budget logs and reboots; there is no unload at runtime, no `sys.exit`, and no `mesh stop`. Per-applet strikes moved to the icebox. A retained RAM syslog is flushed to `/boot/syslog` at boot. Reworked 2.4, 5.4, 5.5, 5.5a, 5.6, 5.10, 9.2, and 9.4; dropped 4.9 (DG2, DG7).
- 2026-11-01: One shared guest stack and a fixed kernel reserve (DG1). The stack, scratch arena, and event payload ring come from a reserve set aside at boot (`stack_size`, `scratch_size`, `payload_pages`), so an applet that empties the shared pool cannot starve other applets. Nested calls continue on the same stack. The manifest and `mesh.toml` lose the `stack` field. New guard G9 (4.12, 9.3); 4.8 tests overflow and the full ring.
- 2026-11-01: `[net]` defined (`ssid`, `password`) and parsed in 3.4 (C14). `mesh mount` stages `.mhc` writes and lands them through `name.part` and a rename (1.8a).
- 2026-10-31: Replacement resets trigger on a real change, not an open (C20). `mesh cp` writes `name.part` and renames it over the target at the end. The `.mhc` header carries a `body_hash`, and the loader refuses a torn file (1.8, 2.6b, 5.11).
- 2026-10-31: Safe mode and the node-level boot counter moved to the icebox (DG7). An applet that resets the node, through `sys.reset` or a watchdog or fault reset while it runs, gets a strike, and at `max_strikes` it is disabled while the rest of the node runs. `sys.bootOk` and `mesh reboot --safe` are gone. Unblamed reset loops keep looping.
- 2026-10-31: Applet isolation and grants moved to the icebox. Any applet may read and write any other applet's heap, call every kernel function, and layer on any name. Partitions stay for memory limits, unload cleanup, and address translation. The kernel's own pages stay protected (4.5 is now kernel-page access). `[grants]`, capabilities, pinned hashes, and minimum versions are gone.
