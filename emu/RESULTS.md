# Round six: the `fast` interpreter on emulated nRF52840 and ESP32-S3

`emu/run.sh` builds both firmwares, boots them headless, checks every result
against the host, and prints this table. `emu/setup-tools.sh` fetches the tools first.

## Pass/fail

| Chip (emulator) | crc16 | crc16-check | parse | parse-bad-sum | parse-short |
|---|---|---|---|---|---|
| nRF52840 (Renode 1.17, Embassy) | ✅ | ✅ | ✅ | ✅ | ✅ |
| ESP32-S3 (Espressif QEMU 9.2.2, esp-hal) | ✅ | ✅ | ✅ | ✅ | ✅ |

Each row passes on three checks: the interpreter's result, the same kernel compiled
natively for that chip, and the host's native result all agree. The expected values
are crc16 `0x9fe1`, check `0x29b1`, parse `0x3581998c`, bad sum `0x80000006`,
short `0x80000003`. The whole run takes about 17 s.

**Per-target interpreter changes: none.** `bench/interp` (the `fast` interpreter) and
`guest/kernels` are byte-identical to round five (`git diff 926d84b -- bench/interp
guest/kernels` is empty). Both firmwares depend on the same `emu/bench-core`, which
embeds the same 912-byte RV32IMC image with `include_bytes!`. All chip-specific code
lives in the two `main.rs` files: UART setup, the cycle counter, and the entry point.

## Cycle counts: not cycle-accurate, a sanity check only

| Chip | Workload | Interp counter | Native counter | What the counter really is |
|---|---|---|---|---|
| nRF52840 | crc16 | 31,161 | 920 | Renode DWT CYCCNT = instructions × 0.64 |
| nRF52840 | parse | 7,496 | 365 | (64 MHz ÷ Renode's default 100 MIPS) |
| ESP32-S3 | crc16 | 3,314 | 360 | QEMU CCOUNT under `-icount shift=0`: |
| ESP32-S3 | parse | 776 | 30 | 1 instruction = 1 ns, CCOUNT ticks at the CPU clock |

Neither emulator models a pipeline, wait states or caches. Both counters measure
instructions executed, not cycles. What they confirm:

- **nRF52840.** Dividing by 0.64 gives 17.2 Thumb instructions per guest instruction
  for crc16 and 16.7 for parse. That is exactly what round five measured under Unicorn
  (17.2 and 16.7). Native crc16 comes to 1,438 instructions, against 1,432 under
  Unicorn. The firmware build and the Unicorn harness agree.
- **ESP32-S3.** Counts are deterministic run to run. Assuming CCOUNT runs at the 80 MHz
  default, the interpreter executes roughly 14–15 Xtensa instructions per guest
  instruction.
- **Superseded in round seven:** in this round's table the S3's CRC ratio isn't
  comparable with the nRF's, because the native baselines differed. The Xtensa
  toolchain compiles crc16 as the literal bitwise loop (about 4,500 instructions),
  while Thumb gets a 512-byte table CRC (about 1,430). Round seven made the S3
  baseline table-driven (`kernels::crc16_table`, the guest's algorithm): the S3 is
  now **28.9x** against the nRF's 32.7x (`meshops/RESULTS.md`).

## Toolchain pain points (Xtensa)

1. **Downloads.** `espup` needs the GitHub API to list releases, and the API is
   blocked in this environment. Direct release-asset URLs work, so `setup-tools.sh`
   installs `esp-rs/rust-build` by hand (`install.sh --prefix`, then
   `rustup toolchain link`). QEMU and GCC come from Espressif's mirror
   (`dl.espressif.com/github_assets/...`). It doesn't carry the newest GCC
   (esp-16.x), and it doesn't carry the Rust toolchain.
2. **Xtensa Rust 1.99.0.0 can't link esp-hal** (neither 1.2.2 nor 1.1.2). The
   linker reports "dangerous relocation: l32r: literal placed after use" in
   `__pre_init`. esp-hal declares that function's literals with `.literal` inside
   `global_asm!` in a `.rwtext` section. The 1.99 assembler puts them in the generic
   `.literal` section, which esp-hal's linker script places after the code. **1.97.0.0
   works**, so the toolchain is pinned to `esp197`.
3. **esp-hal ≥ 1.1 hangs at boot under QEMU.** Its new ESP32-S3 clock code spins on
   `I2C_ANA_MST.ANA_CONF0.bbpll_cal_done` (`0x6000e040` bit 24), which Espressif's
   QEMU never sets. Running from the crystal doesn't help: other esp-hal code
   unwraps the PLL configuration and panics. **esp-hal 1.0.0** has no wait on S3 and
   works, so it's pinned (`=1.0.0`, with `esp-bootloader-esp-idf` 0.4 and `esp-println` 0.16).
4. **Version coupling.** Each esp-hal minor version requires a matching
   `esp-bootloader-esp-idf` version (1.0 ↔ 0.4, 1.1 ↔ 0.5, 1.2 ↔ 0.6), and cargo only
   says so through a resolver conflict.
5. **Embassy on ESP32-S3: not tried.** It wasn't straightforward with the HAL pinned
   back. The S3 firmware is plain esp-hal; the nRF one uses Embassy (executor 0.10,
   embassy-nrf 0.11).
6. **Smaller items:**
   - esp-println's default `auto` output can choose USB-Serial-JTAG; the firmware
     forces `uart` and writes results through esp-hal's UART0.
   - QEMU needs `libslirp0` installed.
   - QEMU's CCOUNT follows host wall-clock time unless you pass `-icount`.
   - The ESP image needs `espflash save-image --merge` to produce the 4 MB flash
     image with the bootloader.

## What neither emulator models

- **Timing:**
  - no pipeline, no branch penalties;
  - no flash wait states, and neither the nRF52840 NVMC cache nor the ESP32-S3's
    flash/PSRAM MMU cache;
  - the counters above are instruction counts, so the interpreter's data reads from
    flash (the guest image) cost the same as RAM reads.
- **On the nRF52840 under Renode:** the stock platform has no DWT; `run.resc` adds
  Renode's DWT model. FICR reads return 0, and TEMP and other registers are
  unimplemented (only warnings here).
- **On the ESP32-S3 under QEMU:** BBPLL calibration isn't modelled (point 3 above),
  and neither is RF or Wi-Fi.
- **Not modelled on either:** power and energy; interrupt latency under load; bus
  contention from DMA or the radio.
- **Not run on either chip:** the round-four AOT path. It's Thumb-only and out of
  scope for this round.
