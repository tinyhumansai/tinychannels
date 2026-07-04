# Contributing To TinyChannels

TinyChannels is early scaffolding for OpenHuman channel and harness
communication. The best contributions are small, explicit, tested, and easy to
review.

## Development Setup

Install a stable Rust toolchain with Rust 2024 support, then run:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo build --all-targets
cargo test
```

## Project Philosophy

TinyChannels should keep channel messaging explicit, transport-neutral, and
observable. Prefer:

- small modules with narrow responsibilities
- typed message and lifecycle surfaces
- typed errors
- adapters that keep transport details out of core contracts
- public APIs that are easy to test
- documentation that names the OpenHuman boundary being served

New module directories should keep shared type definitions in `types.rs` and
module-local unit tests in `test.rs`. Integration tests belong in `tests/`.

## Pull Request Checklist

Before opening a pull request:

- run `cargo fmt --check`
- run `cargo clippy --all-targets -- -D warnings`
- run `cargo build --all-targets`
- run `cargo test`
- add or update tests for behavior changes
- update docs when public APIs, architecture, or examples change
- keep the PR focused on one logical change

## Commit Style

Use concise imperative commit subjects, for example:

```text
Add channel envelope type
Document harness message lifecycle
```

Avoid mixing formatting, refactors, and behavior changes unless they are
inseparable.

## Issue Triage

Good issues include:

- the TinyChannels version or commit
- the relevant module or API
- a minimal code example when behavior is surprising
- expected behavior
- actual behavior
- commands run locally

Feature requests should explain the channel workflow they unlock, the public
API shape they imply, and any safety or observability concerns.

## Security

Do not report vulnerabilities through public issues. Use the process in
[SECURITY.md](SECURITY.md).
