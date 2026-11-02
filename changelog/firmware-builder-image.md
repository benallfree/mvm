# Changelog: firmware-builder-image

The firmware builder: the Docker image that builds kernel firmware, published as the multi-arch `benallfree/mvm-firmware-builder` (`linux/arm64` and `linux/amd64`) and used by digest. It is only needed to build MVM firmware from source (`cargo xtask firmware`, and CI for `mvm` releases); to flash a board, download an `mvm` release. Each release states its digest and its pinned tool versions (`docker/firmware-builder/versions`).

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow semver as the design plan's Releases section describes. Each step adds its entries under "Unreleased"; `cargo xtask release` moves them under the new version.

## [Unreleased]

## [0.1.0] - 2026-11-02

### Added

- The first image (0.2a, 0.3a): `rust:1.99.1-slim-bookworm` pinned by digest, stable Rust 1.99.1 with `thumbv7em-none-eabihf`, espup 0.19.0 with Xtensa Rust 1.98.0.0, espflash 4.7.0, and `uf2conv.py` at microsoft/uf2 `90e9741f217f`. Builds of the same commit are byte-identical on arm64 and amd64.
