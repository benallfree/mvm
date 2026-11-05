#!/usr/bin/env bash
# Speed spike: build the guest image, the interpreters, the AOT translation and
# the Thumb firmware, then estimate cycles under Unicorn.
# Writes RESULTS.md and COSTS.md.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$HERE/.."
(cd "$ROOT/chunker" && cargo build -q --release)
"$ROOT/chunker/target/release/chunker" link --entry crc16 --entry parse -o "$HERE/guest-image/kernels" \
  $(PKG=kernels BUILD_STD=1 "$ROOT/guest/build.sh")
cd "$HERE"
# Host check (both interpreters vs native) and guest instruction counts.
mkdir -p target/aot
cargo test -q -p interp --features count -- --nocapture --test-threads 1 > target/guest-counts.txt
cargo build -q --release -p fw --target thumbv7em-none-eabihf
# AOT: translate the same linked image, assemble, and insist on zero relocations (PIC).
cargo build -q --release -p aot
ENTRIES=$(sed -n 's/^pub const \(CRC16\|PARSE\): u32 = 0x\([0-9a-f]*\);/\L\1\E=\2/p' guest-image/kernels.rs)
./target/release/aot guest-image/kernels.bin 0x1000 target/aot/kernels.s $ENTRIES 2> target/aot/info.txt
llvm-mc -triple=thumbv7em-none-eabi -mcpu=cortex-m4 -filetype=obj target/aot/kernels.s -o target/aot/kernels.o
if llvm-readelf -r target/aot/kernels.o | grep -q 'Relocation section'; then
  echo "AOT output has relocations; it must be position independent" >&2; exit 1
fi
llvm-objcopy -O binary --only-section=.text target/aot/kernels.o target/aot/kernels-aot.bin
llvm-nm target/aot/kernels.o > target/aot/kernels.syms
python3 harness/m4_cost.py > COSTS.md
python3 harness/run.py target/thumbv7em-none-eabihf/release/fw target/guest-counts.txt RESULTS.md \
  target/aot/kernels-aot.bin target/aot/kernels.syms
