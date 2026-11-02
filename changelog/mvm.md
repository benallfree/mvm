# Changelog: mvm

The MVM firmware to flash: the kernel plus the embedded bootstrapper (`boot.mhc`), built by the firmware builder, as the nRF52840 `.uf2` and the ESP32-S3 `.bin`. Each release lists its component versions: kernel, `boot`, `firmware-builder-image`, and ABI.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow semver as the design plan's Releases section describes. Each step adds its entries under "Unreleased"; `cargo xtask release` moves them under the new version.

## [Unreleased]

### Added

- Stub images that link and boot nothing yet: nRF52840 at `0x26000` (S140 6.1.1) and ESP32-S3 with the ESP-IDF app descriptor, built reproducibly in the firmware builder (0.2a).
