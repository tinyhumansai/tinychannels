<h1 align="center">TinyChannels</h1>

<p align="center">
 <a href="https://crates.io/crates/tinychannels"><img src="https://img.shields.io/crates/v/tinychannels.svg" alt="crates.io" /></a>
 <a href="https://docs.rs/tinychannels"><img src="https://docs.rs/tinychannels/badge.svg" alt="docs.rs" /></a>
 <a href="https://github.com/tinyhumansai/tinychannels/actions/workflows/ci.yml"><img src="https://github.com/tinyhumansai/tinychannels/actions/workflows/ci.yml/badge.svg" alt="CI" /></a>
 <a href="LICENSE"><img src="https://img.shields.io/badge/License-GPLv3-blue.svg" alt="License: GPL v3" /></a>
</p>

**TinyChannels is a blank Rust scaffold for OpenHuman channel and messaging
primitives.** It is intended to become the pluggable communication layer between
channels and harnesses without coupling OpenHuman runtime code to one transport,
provider, or host application.

## Intended Scope

- channel abstractions for inbound and outbound message streams
- harness-facing communication contracts
- transport-neutral message envelopes and routing metadata
- adapters for OpenHuman channel surfaces
- observability and lifecycle hooks around channel traffic

This repository currently contains scaffolding only. Public APIs and behavior
will be added deliberately as the OpenHuman integration shape becomes concrete.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo build --all-targets
cargo test
```

## Repository Layout

- `src/lib.rs` exports the crate surface.
- `src/channel/` is reserved for channel-side abstractions.
- `src/harness/` is reserved for harness communication boundaries.
- `docs/spec/README.md` tracks the high-level architecture notes.
