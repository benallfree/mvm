# Changelog: mesh

The MVM command-line tool: `mesh new` and `mesh build` for applets, `mesh run` for simulated meshes, and the commands that manage nodes, for macOS, Linux, and Windows. Building applets requires Docker, because `mesh build` compiles inside the applet builder image.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow semver as the design plan's Releases section describes. Each step adds its entries under "Unreleased"; `cargo xtask release` moves them under the new version.

## [Unreleased]

## [0.0.1] - 2026-11-02

### Added

- `mesh --version` prints the version (0.3a).
