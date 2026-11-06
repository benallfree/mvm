# Stable environment for the MVM spikes: chunker checks, the Unicorn speed
# bench, and the nRF52840 (Renode) / ESP32-S3 (QEMU) emulator runs.
#
# Build:  docker/build.sh          Run:  docker/run.sh [command]   (default: docker/run-all.sh)
# The repo is mounted at /work at run time; the image holds only the toolchains.
FROM ubuntu:24.04@sha256:fe81560990fae86dc4743202b2cb4d3df98d0bcf949173cecef15ce1127ef852

ARG DEBIAN_FRONTEND=noninteractive

# Only non-empty behind a TLS-intercepting proxy (see docker/build.sh).
COPY docker/extra-ca/ /usr/local/share/ca-certificates/mvm-extra/

# - build-essential: host build scripts. llvm: llvm-mc/objcopy/readelf/nm/symbolizer (LLVM 18).
# - libslirp0, libsdl2-2.0-0, libgcrypt20, libglib2.0, libpixman: Espressif QEMU.
# - libicu74, libssl3: Renode (self-contained .NET). libudev-dev: building espflash.
RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates curl git xz-utils file build-essential pkg-config \
      python3 python3-venv llvm \
      libslirp0 libsdl2-2.0-0 libgcrypt20 libglib2.0-0t64 libpixman-1-0 \
      libicu74 libssl3 libudev-dev \
 && update-ca-certificates \
 && rm -rf /var/lib/apt/lists/*

ENV SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt \
    CARGO_HTTP_CAINFO=/etc/ssl/certs/ca-certificates.crt \
    PIP_CERT=/etc/ssl/certs/ca-certificates.crt \
    RUSTUP_HOME=/opt/rustup \
    CARGO_HOME=/opt/cargo \
    MVM_EMU_TOOLS=/opt/emu-tools \
    PATH=/opt/venv/bin:/opt/cargo/bin:/opt/emu-tools/xtensa-esp-elf/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin

# Rust: stable 1.97.0 for host tools and Thumb firmware; the pinned nightly for
# the RV32IMC guest (with rust-src for -Zbuild-std). Same pins as the
# rust-toolchain.toml files in the repo.
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path --profile minimal --default-toolchain 1.97.0 \
 && rustup component add rustfmt clippy \
 && rustup target add thumbv7em-none-eabihf \
 && rustup toolchain install nightly-2026-11-04 --profile minimal --component rust-src --target riscv32imc-unknown-none-elf

# Python for the Unicorn harness (bench/harness).
RUN python3 -m venv /opt/venv \
 && pip install --no-cache-dir unicorn==2.1.4 capstone==5.0.9 pyelftools==0.33

# Emulators and the Xtensa toolchain, same pinned versions as outside Docker.
COPY emu/setup-tools.sh /opt/setup-tools.sh
RUN /opt/setup-tools.sh \
 && rm -rf /opt/emu-tools/*.tar.* /opt/emu-tools/xt197 /opt/cargo/registry /opt/cargo/git \
 && chmod -R a+rwX /opt/cargo /opt/rustup

WORKDIR /work
CMD ["docker/run-all.sh"]
