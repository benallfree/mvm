#!/usr/bin/env bash
# Round seven: mesh ops. Links the test applets, runs the shared tests on the
# x86 host and under Unicorn (cycle estimate), then writes RESULTS.md, adding
# the Renode/QEMU results from emu/out/ when emu/run.sh has run.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
cd "$HERE"
mkdir -p out
echo "== link the test applets"
python3 build_applets.py
echo "== x86 host"
(cd host && cargo run -q --release) > out/host.txt
grep '^MO host .*pass=' out/host.txt
echo "== Unicorn (Thumb + M4 cost table)"
(cd fw-unicorn && cargo build -q --release --target thumbv7em-none-eabihf)
python3 run_unicorn.py fw-unicorn/target/thumbv7em-none-eabihf/release/fw out/unicorn.json > out/unicorn.txt
grep '^MO unicorn .*pass=' out/unicorn.txt
echo "== report"
python3 report.py > /dev/null && echo "wrote meshops/RESULTS.md" || { echo "FAILURES (see meshops/RESULTS.md)"; exit 1; }
