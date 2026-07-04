<h1 align="center">TinyChannels</h1>

<p align="center">
 <a href="https://crates.io/crates/tinychannels"><img src="https://img.shields.io/crates/v/tinychannels.svg" alt="crates.io" /></a>
 <a href="https://docs.rs/tinychannels"><img src="https://docs.rs/tinychannels/badge.svg" alt="docs.rs" /></a>
 <a href="https://github.com/tinyhumansai/tinychannels/actions/workflows/ci.yml"><img src="https://github.com/tinyhumansai/tinychannels/actions/workflows/ci.yml/badge.svg" alt="CI" /></a>
 <a href="LICENSE"><img src="https://img.shields.io/badge/License-GPLv3-blue.svg" alt="License: GPL v3" /></a>
</p>

**TinyChannels is a Rust library for OpenHuman channel and messaging
primitives.** It provides the portable channel contract, channel configuration
schema, connection metadata, route helpers, and backend delegation layer used to
connect channel surfaces to OpenHuman harnesses without coupling this crate to
the OpenHuman application crate.

## Intended Scope

- channel abstractions for inbound and outbound message streams
- harness-facing communication contracts
- transport-neutral message envelopes and routing metadata
- adapters for OpenHuman channel surfaces
- observability and lifecycle hooks around channel traffic

Runtime side effects are pluggable through `ChannelBackend`. OpenHuman owns the
actual backend implementation for REST/JWT/config storage, while this crate
validates channel metadata and delegates operations through that trait.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo build --all-targets
cargo test
```

## Repository Layout

- `src/lib.rs` exports the crate surface.
- `src/traits.rs` owns `Channel`, `ChannelMessage`, and `SendMessage`.
- `src/config.rs` owns channel configuration structs migrated from OpenHuman.
- `src/controllers/` owns connection definitions and backend response types.
- `src/backend.rs` owns `ChannelBackend` and `ChannelManager`.
- `src/context.rs`, `src/routes.rs`, and `src/runtime.rs` hold portable runtime helpers.
- `docs/spec/README.md` tracks the high-level architecture notes.
