#!/usr/bin/env bash
# Speed spike: build the guest image, the interpreters and the Thumb firmware,
# then estimate cycles under Unicorn. Writes RESULTS.md and COSTS.md.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$HERE/.."
(cd "$ROOT/chunker" && cargo build -q --release)
"$ROOT/chunker/target/release/chunker" link --entry crc16 --entry parse -o "$HERE/guest-image/kernels" \
  $(PKG=kernels BUILD_STD=1 "$ROOT/guest/build.sh")
cd "$HERE"
# Host check (both interpreters vs native) and guest instruction counts.
cargo test -q -p interp --features count -- --nocapture --test-threads 1 > target/guest-counts.txt
cargo build -q --release -p fw --target thumbv7em-none-eabihf
python3 harness/m4_cost.py > COSTS.md
python3 harness/run.py target/thumbv7em-none-eabihf/release/fw target/guest-counts.txt RESULTS.md
