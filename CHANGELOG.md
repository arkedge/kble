# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Release: ship glibc (dynamically linked) Linux binaries for `x86_64` and `aarch64`, alongside the existing musl (static) ones. Built with `cross`, whose Ubuntu 20.04 based images keep the binaries runnable on systems with glibc 2.31 or newer; a `verify-release-binary.sh` check enforces that floor (and the musl binaries' static linkage) so a non-portable build fails the release ([#244](https://github.com/arkedge/kble/pull/244)).

## [0.5.0] - 2026-06-16

### Added

- `kble-c2a`: fragment oversized Space Packets across multiple AOS Transfer Frames ([#178](https://github.com/arkedge/kble/pull/178)).
- `kble-socket`: read/write pipe stdio without dedicated blocking threads ([#189](https://github.com/arkedge/kble/pull/189)).
- `kble-serialport`: best-effort macOS builds, alongside the existing Linux and Windows targets ([#239](https://github.com/arkedge/kble/pull/239)).

### Changed

- Raised the MSRV and pinned toolchain to Rust 1.88 ([#223](https://github.com/arkedge/kble/pull/223), [#229](https://github.com/arkedge/kble/pull/229)).
- Updated `notalawyer` to 0.3, which invokes cargo-about as a library (no `cargo-about` binary needed at build time) ([#229](https://github.com/arkedge/kble/pull/229)).
- Routine dependency, toolchain, and CI maintenance updates.

### Fixed

- `kble`: exit cleanly when the final link closes ([#184](https://github.com/arkedge/kble/pull/184)).
- `kble`: decode the percent-encoded `exec` command before running it ([#187](https://github.com/arkedge/kble/pull/187)).
- `kble-tcp`: exit when either side of the bridge closes ([#186](https://github.com/arkedge/kble/pull/186)).
- `kble-dump`: exit after replay completes ([#188](https://github.com/arkedge/kble/pull/188)).
- `kble-eb90`: default `--buffer-size` to the maximum frame size ([#182](https://github.com/arkedge/kble/pull/182)).
- `kble-serialport`: keep the pty slave open so the macOS E2E tests pass ([#241](https://github.com/arkedge/kble/pull/241)).

## [0.4.2] - 2025-02-28

### Changed

- Updated the Rust toolchain to 1.78.0 ([#148](https://github.com/arkedge/kble/pull/148)).
- Moved `about.toml` into each crate directory ([#152](https://github.com/arkedge/kble/pull/152)).
- Adopted manual SemVer management ("trust semver by ourselves") ([#147](https://github.com/arkedge/kble/pull/147)).
- Routine dependency maintenance updates.

## [0.4.1] - 2025-01-30

### Fixed

- Close all streams even when one stream closes normally ([#139](https://github.com/arkedge/kble/pull/139)).

### Changed

- Routine dependency maintenance updates.

## [0.4.0] - 2024-11-19

### Added

- `kble-dump`: new plug for dumping traffic ([#102](https://github.com/arkedge/kble/pull/102)).
- `kble-tcp`: new plug bridging a TCP socket, with clap-based argument handling ([#98](https://github.com/arkedge/kble/pull/98)).
- aarch64 Linux (musl) release binaries ([#131](https://github.com/arkedge/kble/pull/131)).
- `cargo publish --dry-run` job in the release workflow ([#132](https://github.com/arkedge/kble/pull/132)).
- `kble`: config validation ([#74](https://github.com/arkedge/kble/pull/74)) and byte-forwarding trace logs ([#90](https://github.com/arkedge/kble/pull/90)).

### Changed

- `kble`: graceful WebSocket shutdown ([#78](https://github.com/arkedge/kble/pull/78)); wait for child processes and kill them after a configurable grace period ([#82](https://github.com/arkedge/kble/pull/82)).
- Updated `notalawyer` to 0.2.0 ([#123](https://github.com/arkedge/kble/pull/123)).
- Routine dependency, toolchain, and CI maintenance updates.

## [0.3.0] - 2024-02-28

### Added

- Release automation workflow distributing prebuilt binaries (Linux musl, plus `kble-serialport` for Windows) ([#60](https://github.com/arkedge/kble/pull/60)).
- License NOTICE embedded in each binary via `notalawyer` / cargo-about ([#59](https://github.com/arkedge/kble/pull/59)).

### Changed

- Aligned `tokio-tungstenite` with axum 0.6 (bumped to 0.21) ([#64](https://github.com/arkedge/kble/pull/64)).

### Fixed

- Fix packet type handling.

## [0.3.0-beta.1] - 2024-02-28

### Added

- Initial release-automation workflow, published as a pre-release to exercise the binary distribution pipeline.

## [0.2.0] - 2023-06-02

### Added

- `kble-eb90`: new plug.
- `kble-c2a`: `spacepacket` ([#27](https://github.com/arkedge/kble/pull/27)) and `tfsync` ([#24](https://github.com/arkedge/kble/pull/24)) subcommands.

### Changed

- Restructured the repository into a Cargo workspace (resolver v2, shared `workspace.package` and dependencies).
- Use the `bytes` crate for buffer handling ([#3](https://github.com/arkedge/kble/issues/3)).

## [0.1.0] - 2023-04-27

Initial public release of kble: the `kble` orchestrator and its core plugs.

[Unreleased]: https://github.com/arkedge/kble/compare/v0.5.0...HEAD
[0.5.0]: https://github.com/arkedge/kble/compare/v0.4.2...v0.5.0
[0.4.2]: https://github.com/arkedge/kble/compare/v0.4.1...v0.4.2
[0.4.1]: https://github.com/arkedge/kble/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/arkedge/kble/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/arkedge/kble/compare/v0.3.0-beta.1...v0.3.0
[0.3.0-beta.1]: https://github.com/arkedge/kble/compare/v0.2.0...v0.3.0-beta.1
[0.2.0]: https://github.com/arkedge/kble/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/arkedge/kble/releases/tag/v0.1.0
