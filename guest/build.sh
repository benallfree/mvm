#!/usr/bin/env bash
# The builder: pins the codegen settings the chunker depends on.
#   -relax            no linker relaxation, so instruction layout is fixed
#   codegen-units=1   one object per crate, stable symbol numbering
#   lto=off           crates stay separate objects (pinned: chunked builds never use LTO)
#   function-sections one text section per function, one data section per item
# Env:
#   PKG=testcrate|applet   package to build (default testcrate)
#   CODE_MODEL=small|medium  medium switches to PC-relative addressing (auipc pairs)
#   BUILD_STD=1            rebuild core/compiler_builtins with these same flags
#                          (the shipped core is built with relaxation on)
#   LTO=fat                measurement only: build PKG as a fat-LTO staticlib
# Prints the chunker inputs: the package's object plus its dependency rlibs.
set -euo pipefail
cd "$(dirname "$0")"
PKG="${PKG:-testcrate}"
CODE_MODEL="${CODE_MODEL:-small}"
LTO="${LTO:-off}"
STD=()
TAG="$PKG-$CODE_MODEL"
if [ "${BUILD_STD:-0}" = 1 ]; then STD=(-Zbuild-std=core); TAG="$TAG-std"; fi
export RUSTFLAGS="-C target-feature=-relax -C codegen-units=1 -Z function-sections=yes -C code-model=${CODE_MODEL}"
export CARGO_TARGET_DIR="target/$TAG-lto$LTO"
D="$PWD/$CARGO_TARGET_DIR/riscv32imc-unknown-none-elf/release"
if [ "$LTO" = off ]; then
  RUSTFLAGS="$RUSTFLAGS -C lto=off" cargo rustc -q "${STD[@]}" -p "$PKG" --lib --crate-type rlib --release \
    --target riscv32imc-unknown-none-elf -- --emit=obj
  find "$D" -name "$PKG-*.o"
  find "$D" -name '*.rlib' ! -name "lib$PKG-*"
else
  CARGO_PROFILE_RELEASE_LTO="$LTO" cargo rustc -q "${STD[@]}" -p "$PKG" --lib --crate-type staticlib --release \
    --target riscv32imc-unknown-none-elf
  find "$D" -maxdepth 1 -name "lib$PKG.a"
fi
