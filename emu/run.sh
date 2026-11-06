#!/usr/bin/env bash
# Round six: run the same `fast` interpreter (bench/interp) on an emulated
# nRF52840 (Renode) and ESP32-S3 (Espressif QEMU), and check every result
# against the native kernels on the host. Prints a combined pass/fail table.
#
# Tools come from setup-tools.sh (run it first; it's idempotent).
# Cycle counts are NOT cycle-accurate: Renode's DWT model and QEMU's CCOUNT
# under -icount both count instructions, scaled by a nominal clock.
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
T="${MVM_EMU_TOOLS:-$HOME/emu-tools}"
OUT="$HERE/out"
mkdir -p "$OUT"
export PATH="$T/xtensa-esp-elf/bin:$HOME/.cargo/bin:$PATH"
RENODE=$(ls -d "$T"/renode_*/renode 2>/dev/null | head -1)
QEMU="$T/qemu/bin/qemu-system-xtensa"
[ -x "$RENODE" ] && [ -x "$QEMU" ] || { echo "missing tools: run $HERE/setup-tools.sh" >&2; exit 2; }

step() { printf '== %s\n' "$*"; }
fail=0

step "host: expected native results"
(cd "$HERE/expected" && cargo run -q --release) > "$OUT/expected.txt" || exit 1

step "nrf52840: build (Embassy, thumbv7em-none-eabihf)"
(cd "$HERE/nrf52840" && cargo build -q --release) || exit 1
step "nrf52840: run under Renode"
rm -f "$OUT/nrf52840.uart"
ELF="$HERE/nrf52840/target/thumbv7em-none-eabihf/release/mvm-emu-nrf52840"
timeout 900 "$RENODE" --disable-gui --console --plain \
  -e "\$elf=@$ELF; \$uartlog=@$OUT/nrf52840.uart; include @$HERE/nrf52840/run.resc" > "$OUT/nrf52840.renode.log" 2>&1

step "esp32s3: build (esp-hal + esp-rtos, Xtensa Rust esp197)"
(cd "$HERE/esp32s3" && cargo build -q --release) || exit 1
espflash save-image --chip esp32s3 --merge \
  "$HERE/esp32s3/target/xtensa-esp32s3-none-elf/release/mvm-emu-esp32s3" "$OUT/esp32s3-flash.bin" > "$OUT/espflash.log" 2>&1 || exit 1
step "esp32s3: run under QEMU (-icount shift=0)"
rm -f "$OUT/esp32s3.uart"
# UART0 is a TCP socket: qemu_serial.py logs it and types the test-30 script.
PORT=5555
"$QEMU" -nographic -machine esp32s3 -icount shift=0 \
  -drive file="$OUT/esp32s3-flash.bin",if=mtd,format=raw \
  -serial tcp:127.0.0.1:$PORT,server=on,wait=on -monitor none -no-reboot > "$OUT/esp32s3.qemu.log" 2>&1 &
QPID=$!
python3 "$HERE/qemu_serial.py" $PORT "$OUT/esp32s3.uart" "MO esp32s3 async-done" 1500
kill $QPID 2>/dev/null; wait $QPID 2>/dev/null

echo
printf '%-9s %-14s %-11s %-11s %-11s %-5s %10s %10s\n' chip case interp chip-native host-native ok "interp cyc" "native cyc"
for chip in nrf52840 esp32s3; do
  while read -r name want; do
    line=$(grep -a "^MVM $chip $name " "$OUT/$chip.uart" 2>/dev/null | head -1)
    if [ -z "$line" ]; then
      printf '%-9s %-14s %-11s %-11s %-11s %-5s\n' $chip $name - - "$want" FAIL; fail=1; continue
    fi
    got=$(sed 's/.* interp=\([^ ]*\).*/\1/' <<<"$line"); nat=$(sed 's/.* native=\([^ ]*\).*/\1/' <<<"$line")
    ic=$(sed 's/.* interp_cycles=\([^ ]*\).*/\1/' <<<"$line"); nc=$(sed 's/.* native_cycles=\([^ ]*\).*/\1/' <<<"$line")
    ok=PASS; { [ "$got" = "$want" ] && [ "$nat" = "$want" ]; } || { ok=FAIL; fail=1; }
    printf '%-9s %-14s %-11s %-11s %-11s %-5s %10s %10s\n' $chip $name "$got" "$nat" "$want" $ok "$ic" "$nc"
  done < "$OUT/expected.txt"
done
echo
echo "cycles: emulator counts, not cycle-accurate (sanity check only)"

# Round seven: mesh-op tests (same shared runner on both chips).
echo
printf '%-9s %-24s %-5s %s\n' chip test ok detail
for chip in nrf52840 esp32s3; do
  for t in load 1-hello 2-timer 3-nop-cost 4-applet-op 5-nested 6-missing-import 7-swap 8-spoof 9-position-independence 10-instances 11-real-pointers 12-shared-stack 13-fit 14-big-buffer 15-overflow-spill 16-bulk-position-independence 17-bulk-real-pointer 18-bulk-fit 19-no-annotations done async-load 20-hello-loop 21-two-tasks 22-two-applets 23-idle 24-no-deadlock 25-budget 26-stale-ticket 27-inbox-overflow 28-driver-priority 29-select async-costs async-sizes 30-script-inputs async-done; do
    line=$(grep -a "^MO $chip $t " "$OUT/$chip.uart" 2>/dev/null | head -1)
    if [ -z "$line" ]; then printf '%-9s %-24s %-5s\n' $chip $t FAIL; fail=1; continue; fi
    ok=FAIL; grep -q ' pass=1' <<<"$line" && ok=PASS
    [ $ok = PASS ] || fail=1
    detail=$(sed "s/^MO $chip $t pass=[01] *//" <<<"$line" | cut -c1-70)
    printf '%-9s %-24s %-5s %s\n' $chip $t $ok "$detail"
  done
done
[ $fail = 0 ] && echo "ALL PASS" || echo "FAILURES"
exit $fail
