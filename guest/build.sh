#!/usr/bin/env bash
# The builder: pins the codegen settings the chunker depends on.
#   -relax            no linker relaxation, so instruction layout is fixed
#   codegen-units=1   one object per crate, stable symbol numbering
#   lto=off           crates stay separate objects (pinned: chunked builds never use LTO)
#   function-sections one text section per function, one data section per item
# Env:
#   PKG=testcrate|applet   package to build (default testcrate)
#   CODE_MODEL=small|medium  medium switches to PC-relative addressing (auipc pairs)
#                          and turns off LLVM's GlobalMerge (round 7c): it merges
#                          small internal statics into one blob, which would stop
#                          the chunker placing each static on its own
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
if [ "$CODE_MODEL" = medium ]; then RUSTFLAGS="$RUSTFLAGS -C llvm-args=-riscv-enable-global-merge=false"; fi
export CARGO_TARGET_DIR="target/$TAG-lto$LTO"
# Inputs come from cargo's own artifact list, not from globbing the target
# dir: a crate whose build hash changed leaves its old outputs behind, and a
# glob would pick up both.
artifacts() {
  python3 -c '
import json, os, sys
pkg, want = sys.argv[1], sys.argv[2]
own, deps = [], []  # the package first, as before: the chunker breaks ties by input order
for line in sys.stdin:
    m = json.loads(line)
    if m.get("reason") != "compiler-artifact":
        continue
    name = m["target"]["name"]
    for f in m["filenames"]:
        if want == "obj" and f.endswith(".rlib"):
            if name == pkg.replace("-", "_"):
                # --emit=obj puts the object beside the rlib, same hash.
                d, b = os.path.split(f)
                own.append(os.path.join(d, b[3:-5] + ".o"))
            else:
                deps.append(f)
        elif want == "staticlib" and f.endswith(".a"):
            own.append(f)
print("\n".join(own + deps))
' "$PKG" "$1"
}
if [ "$LTO" = off ]; then
  RUSTFLAGS="$RUSTFLAGS -C lto=off" cargo rustc -q "${STD[@]}" -p "$PKG" --lib --crate-type rlib --release \
    --target riscv32imc-unknown-none-elf --message-format=json -- --emit=obj | artifacts obj
else
  CARGO_PROFILE_RELEASE_LTO="$LTO" cargo rustc -q "${STD[@]}" -p "$PKG" --lib --crate-type staticlib --release \
    --target riscv32imc-unknown-none-elf --message-format=json | artifacts staticlib
fi
