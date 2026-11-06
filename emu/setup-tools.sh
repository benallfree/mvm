#!/usr/bin/env bash
# Fetch the emulators and the Xtensa toolchain into $MVM_EMU_TOOLS
# (default ~/emu-tools). Idempotent: skips anything already there.
#
# Pinned versions and why:
# - Renode 1.17.0+20261105git85a5639da: the nightly portable build this was tested with.
# - Espressif QEMU 9.2.2-20260417, from Espressif's mirror of the GitHub releases.
# - Xtensa Rust 1.97.0.0 (esp-rs/rust-build). 1.99.0.0 fails to link esp-hal:
#   its assembler puts esp-hal's `.literal` entries for __pre_init in `.literal`
#   instead of `.rwtext.literal` ("l32r: literal placed after use").
# - Xtensa GCC 14.2.0_20241119 (linker only), from Espressif's mirror.
set -euo pipefail
T="${MVM_EMU_TOOLS:-$HOME/emu-tools}"
mkdir -p "$T" && cd "$T"
get() { [ -f "$2" ] || { echo "fetching $2"; curl -fsSL --retry 3 -o "$2.part" "$1" && mv "$2.part" "$2"; }; }

if ! ls -d renode_*/renode >/dev/null 2>&1; then
  get "https://builds.renode.io/renode-1.17.0+20261105git85a5639da.linux-portable.tar.gz" renode.tar.gz
  tar xzf renode.tar.gz
fi
if [ ! -x qemu/bin/qemu-system-xtensa ]; then
  get https://dl.espressif.com/github_assets/espressif/qemu/releases/download/esp-develop-9.2.2-20260417/qemu-xtensa-softmmu-esp_develop_9.2.2_20260417-x86_64-linux-gnu.tar.xz qemu.tar.xz
  tar xJf qemu.tar.xz
fi
if [ ! -x xtensa-esp-elf/bin/xtensa-esp32s3-elf-gcc ]; then
  get https://dl.espressif.com/github_assets/espressif/crosstool-NG/releases/download/esp-14.2.0_20241119/xtensa-esp-elf-14.2.0_20241119-x86_64-linux-gnu.tar.xz xtensa-gcc.tar.xz
  tar xJf xtensa-gcc.tar.xz
fi
if ! rustup toolchain list | grep -q '^esp197'; then
  G=https://github.com/esp-rs/rust-build/releases/download/v1.97.0.0
  get $G/rust-1.97.0.0-x86_64-unknown-linux-gnu.tar.xz rust-esp-197.tar.xz
  get $G/rust-src-1.97.0.0.tar.xz rust-src-esp-197.tar.xz
  rm -rf xt197 esp-rust-197 && mkdir xt197
  tar xJf rust-esp-197.tar.xz -C xt197 && tar xJf rust-src-esp-197.tar.xz -C xt197
  ./xt197/rust-nightly-*/install.sh --prefix="$T/esp-rust-197" --without=rust-docs-json-preview,rust-docs >/dev/null
  ./xt197/rust-src-nightly/install.sh --prefix="$T/esp-rust-197" >/dev/null
  rustup toolchain link esp197 "$T/esp-rust-197"
fi
command -v espflash >/dev/null || cargo install --locked espflash@4.7.0
# QEMU needs libslirp at run time.
ldconfig -p | grep -q libslirp.so.0 || { apt-get install -y -q libslirp0 || sudo apt-get install -y -q libslirp0; }
echo "tools ready in $T"
