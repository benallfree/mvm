# Changelog: mvm

The MVM command-line tool: `mvm new` and `mvm build` for applets, `mvm run` for simulated meshes, and the commands that manage nodes, for macOS, Linux, and Windows. Building applets requires Docker, because `mvm build` compiles inside the applet builder image.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow semver as the design plan's Releases section describes. Each step adds its entries under "Unreleased"; `cargo xtask release` moves them under the new version.

## [Unreleased]

### Added

- `mvm --version` prints the version (0.3a; first released as `mesh --version` in the withdrawn `mesh 0.0.1`).
