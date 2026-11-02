# Changelog: kernel

The MVM kernel firmware: the nRF52840 `.uf2` and the ESP32-S3 `.bin`. Each release states the ABI version it implements and the `firmware-builder-image` version it was built with.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow semver as the design plan's Releases section describes. Each step adds its entries under "Unreleased"; `cargo xtask release` moves them under the new version.

## [Unreleased]

### Added

- Stub images that link and boot nothing yet: nRF52840 at `0x26000` (S140 6.1.1) and ESP32-S3 with the ESP-IDF app descriptor, built reproducibly in the firmware builder (0.2a).
