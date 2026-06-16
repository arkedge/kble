# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

kble ("cable") is a virtual harness toolkit: a `kble` orchestrator wires together independent "plug" processes, each bridging some external interface to a common WebSocket-over-stdio byte protocol.

## Development approach: TDD

We practice TDD. Write or extend a test that captures the intended behavior, watch it fail, then implement until it passes. New plugs and new features land with their tests in the same change, not as a follow-up.

Here TDD puts the weight on E2E tests. kble's behavior lives largely at process boundaries — plugs talk to the orchestrator as separate processes over WebSocket-over-stdio. In-process unit tests and mocks cannot exercise that IPC, so anything crossing the boundary can only be verified, and only be developed test-first, by launching the real binaries and driving bytes across the real boundary.

The infrastructure exists for exactly this:

- Every plug crate has `tests/e2e.rs` that launches the **real built binary** (`env!("CARGO_BIN_EXE_<name>")`) and drives it through `kble-test-support`.
  - `kble_test_support::Plug` spawns a plug binary and plays the orchestrator's client role (WebSocket-over-stdio).
  - `kble_test_support::WsPlug` stands up an in-process WebSocket server to exercise the `kble` orchestrator binary itself.
- Pure protocol/format logic with no IPC (e.g. `kble-c2a`'s Space Packet ↔ AOS TF conversion) is additionally covered by in-module unit tests plus `proptest` — but anything crossing the process boundary must have an E2E test.

When you touch behavior, the plug's `tests/e2e.rs` is the first place to add coverage. A change that only passes because it was never exercised end-to-end is not done.

## Commands

The toolchain is pinned by `rust-toolchain`; MSRV is `rust-version` in the root `Cargo.toml` (kept in sync). Use the pinned toolchain.

- Test: `cargo test`. Single test: `cargo test -p <crate> <test_name>` (e.g. `cargo test -p kble-c2a sp_large_dump`).
- Lint before pushing: `cargo clippy --workspace --all-targets` and `cargo fmt --all --check`.
- Feature coverage: `cargo hack check --feature-powerset --no-dev-deps --workspace`. The plugs always enable `kble-socket` with `stdio` + `tungstenite`, so a plain build never checks a single `kble-socket` feature alone — this does.

## Gotchas

- **A failing license check is not a compile error.** Each binary's `build.rs` runs cargo-about (via `notalawyer-build`) at build time to generate its license NOTICE. Adding a dependency that introduces a new SPDX license fails the build with `failed to satisfy license requirements`, surfaced from a build script — until that license is added to the `accepted` list in *every* `about.toml`.
