# The firmware builder: the Docker image that builds kernel firmware
# (implementation plan 0.2a, 0.3a). Built for
# linux/arm64 and linux/amd64; both must produce identical firmware bytes.
# Every version here is listed in ./versions.

FROM rust:1.99.1-slim-bookworm@sha256:24707e70a4f1e23d9a7b25202ec43126d28a52294b96aad5c670df1fd1f1d400

ARG TARGETARCH

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates curl unzip python3 binutils-arm-none-eabi \
 && rm -rf /var/lib/apt/lists/*

# nRF52840: stable Rust from the base image plus the Cortex-M4F target.
RUN rustup target add thumbv7em-none-eabihf

# ESP32-S3: Espressif's Xtensa Rust toolchain, installed with espup.
RUN set -eu; \
    case "$TARGETARCH" in \
      arm64) arch=aarch64; sum=a7e841449edbc78ce2443416e90027890b52896698767b6e45a5caafc880498b ;; \
      amd64) arch=x86_64;  sum=f31411c64dffff4da0f0e23c432b9a5fda48942ff8695b8b21009ceb7007bcc0 ;; \
    esac; \
    curl -fsSL -o /usr/local/bin/espup "https://github.com/esp-rs/espup/releases/download/v0.19.0/espup-$arch-unknown-linux-gnu"; \
    echo "$sum  /usr/local/bin/espup" | sha256sum -c -; \
    chmod +x /usr/local/bin/espup; \
    espup install --toolchain-version 1.98.0.0 --targets esp32s3 --export-file /opt/export-esp.sh; \
    rm -rf /root/.cache

# espflash turns the ESP32-S3 ELF into a flashable image.
RUN set -eu; \
    case "$TARGETARCH" in \
      arm64) arch=aarch64; sum=a2081c002f6050a287007a93ba01cdd4c9d111181b69d5ddf7bdb71d33841c65 ;; \
      amd64) arch=x86_64;  sum=a9bf20c76ab42d0d4330bbc33707c4424d5541f5129149d889dbfa8dab476910 ;; \
    esac; \
    curl -fsSL -o /tmp/espflash.zip "https://github.com/esp-rs/espflash/releases/download/v4.7.0/espflash-$arch-unknown-linux-musl.zip"; \
    echo "$sum  /tmp/espflash.zip" | sha256sum -c -; \
    unzip -d /usr/local/bin /tmp/espflash.zip espflash; \
    rm /tmp/espflash.zip

# uf2conv.py turns the nRF52840 binary into a UF2 file.
RUN set -eu; \
    base=https://raw.githubusercontent.com/microsoft/uf2/90e9741f217f5a40c98ba74d663e408041037578/utils; \
    mkdir -p /opt/uf2; \
    curl -fsSL -o /opt/uf2/uf2conv.py "$base/uf2conv.py"; \
    curl -fsSL -o /opt/uf2/uf2families.json "$base/uf2families.json"; \
    printf '%s  %s\n' \
      ad36ba2d61fb2ea371832262392088281eee474c609df4142adbee4ea3c20f26 /opt/uf2/uf2conv.py \
      9d8f561e050507c5909063005cd6f5b157e6fe6db1e8d25b5d9027d57064c194 /opt/uf2/uf2families.json \
      | sha256sum -c -

COPY entrypoint.sh /usr/local/bin/entrypoint.sh
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
WORKDIR /work
