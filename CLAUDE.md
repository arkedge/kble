# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

kble ("cable") is a virtual harness toolkit: a `kble` orchestrator wires together independent "plug" processes, each bridging some external interface to a common WebSocket byte-frame protocol.

## Development approach: TDD

We practice TDD. Write or extend a test that captures the intended behavior, watch it fail, then implement until it passes. New plugs and new features land with their tests in the same change, not as a follow-up.

Here TDD puts the weight on E2E tests. kble's behavior lives largely at process boundaries — plugs run as separate processes and talk to the orchestrator over a WebSocket (most over their stdio; `kble-serialport` as a real `ws://` server). In-process unit tests and mocks cannot exercise that transport, so anything crossing the boundary can only be verified, and only be developed test-first, by launching the real binaries and driving bytes across the real boundary.

The infrastructure exists for exactly this. Each plug crate has `tests/e2e.rs` that launches the **real built binary** (`env!("CARGO_BIN_EXE_<name>")`) and drives it over its actual transport:

- stdio plugs use `kble-test-support` — `Plug` spawns the binary and plays the orchestrator's client role; `WsPlug` stands up an in-process WebSocket server to exercise the `kble` orchestrator binary itself.
- the `ws://` server (`kble-serialport`) is driven by a real WebSocket client (`tokio-tungstenite`).

Pure format/protocol logic with no transport (e.g. `kble-c2a`'s Space Packet ↔ AOS TF conversion) is covered by in-module unit tests instead. When you change behavior that crosses the process/transport boundary, its `tests/e2e.rs` is the first place to add coverage — a change that only passes because it was never exercised end-to-end is not done.

## Commands

The toolchain is pinned by `rust-toolchain`; MSRV is `rust-version` in the root `Cargo.toml` (kept in sync). Use the pinned toolchain.

- Test: `cargo test`. Single test: `cargo test -p <crate> <test_name>` (e.g. `cargo test -p kble-c2a sp_large_dump`).
- Lint before pushing: `cargo clippy --workspace --all-targets` and `cargo fmt --all --check`.
- Feature coverage: `cargo hack check --feature-powerset --no-dev-deps --workspace`. A workspace build unifies `kble-socket`'s features across all its consumers, so no single-feature combination is ever built on its own — this checks them.

## Gotchas

- **A failing license check is not a compile error.** Each binary's `build.rs` runs cargo-about (via `notalawyer-build`) at build time to scan that crate's dependency graph and generate its license NOTICE. Adding a dependency that introduces a new SPDX license fails the build with `failed to satisfy license requirements`, surfaced from a build script — until that license is added to the affected binary's `about.toml` `accepted` list. The lists are kept in sync across crates, so add it to every `about.toml`.
