#!/usr/bin/env bash
# The builder: pins the codegen settings the chunker depends on.
#   -relax            no linker relaxation, so instruction layout is fixed
#   codegen-units=1   one object per crate, stable symbol numbering
#   lto=off           crates stay separate objects
#   function-sections one text section per function, one data section per item
# CODE_MODEL=medium (optional) switches to PC-relative addressing (auipc pairs).
set -euo pipefail
cd "$(dirname "$0")"
CODE_MODEL="${CODE_MODEL:-small}"
export RUSTFLAGS="-C target-feature=-relax -C codegen-units=1 -C lto=off -Z function-sections=yes -C code-model=${CODE_MODEL}"
export CARGO_TARGET_DIR="target/${CODE_MODEL}"
cargo rustc -q -p testcrate --release --target riscv32imc-unknown-none-elf -- --emit=obj
# Print the inputs for the chunker: our object plus every dependency rlib.
D="$PWD/$CARGO_TARGET_DIR/riscv32imc-unknown-none-elf/release"
find "$D" -name 'testcrate-*.o'
find "$D" -name '*.rlib' ! -name 'libtestcrate-*'
