# Contributing To TinyChannels

TinyChannels is early scaffolding for OpenHuman channel and harness
communication. The best contributions are small, explicit, tested, and easy to
review.

## Shared Module CI and Releases

TinyChannels follows the shared module CI and release contract maintained in
[tinyhumansai/.github](https://github.com/tinyhumansai/.github/blob/main/docs/module-ci-contract.md).

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

## Releasing

Change the Cargo package versions through a reviewed PR. After it merges, tag that commit as vX.Y.Z and push the tag, or dispatch Release with an existing version tag. The shared release verifies the package version, builds and loads all 17 native and distro artifacts, and publishes their checksum manifest. Release does not push version bumps to main.

The TinyBus module SDK is pinned as a source gitlink in `vendor/tinybus` and
as an exact version and revision in the workspace dependencies. Keep the
client test fixture's path dependency on that same SDK version when updating
the pin.

For a release through OpenHuman epic #8056, dispatch
`.github/workflows/release.yml` from `epic/8056-modules-wiring` with
`mode=reviewed`, the existing tag, and the positive canonical PR number whose
merge commit the tag names. The legacy zero default is rejected. Ordinary tags
and the existing Release dispatch continue through the same workflow's default
`mode=tag` path.
