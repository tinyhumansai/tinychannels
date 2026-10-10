<p align="center">
 <img src="https://github.com/tinyhumansai/tinychannels/blob/main/docs/channels.png?raw=true" />
</p>

<h1 align="center">TinyChannels</h1>

<p align="center">
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

## Provider Features

TinyChannels includes optional provider implementations that must be explicitly enabled:

| Provider | Feature | Channels | Dependencies |
|----------|---------|----------|--------------|
| **Email (send only)** | `email-send` | `EmailChannel` (SMTP send), `providers::mail::LettreMailSender` (async, per-credential) | `lettre` |
| **Email** | `email` | `EmailChannel` (SMTP + IMAP), `providers::mail::AsyncImapReceiver` | `lettre`, `async-imap`, `mail-parser` |
| **Lark/Feishu** | `lark` | `LarkChannel` (webhook receiver + Protobuf decoder) | `axum`, `prost` |
| **WhatsApp Web** | `whatsapp-web` | `WhatsAppWebChannel` (multi-device via whatsapp-rust) | `whatsapp-rust`, `whatsapp-rust-tokio-transport`, `whatsapp-rust-ureq-http-client`, `wacore` |

> **Not on crates.io.** This crate and `tinychannels-bus` are `publish = false`
> OpenHuman uses the vendored path dependency under `vendor/tinychannels`.
> Other consumers can use the direct Git dependency shown below. What a host
> *loads* at runtime is the compiled `tinychannels-module` `cdylib`, delivered
> as a release artifact and pinned by SHA-256, not a published crate.

The default feature set (`default = []`) does not include these providers. To use them, add to your `Cargo.toml`:

```toml
[dependencies]
tinychannels = { git = "https://github.com/tinyhumansai/tinychannels", features = ["email", "lark", "whatsapp-web"] }
```

Or enable them individually as needed:

```toml
[dependencies]
tinychannels = { git = "https://github.com/tinyhumansai/tinychannels", features = ["email"] }
```

The `providers::mail` vocabulary (`MailSender`/`MailReceiver`, provider-tagged
`MailCredentials`, `ImapCredentials`) is always compiled and links nothing; see
`src/providers/mail/README.md`.

If you only ever *send* mail — no mailbox is polled — take `email-send` instead.
It gives you `EmailChannel::new`, `send_message` and the `build_*_message`
helpers on `lettre` alone, without the IMAP receive stack (18 fewer packages):

```toml
[dependencies]
tinychannels = { git = "https://github.com/tinyhumansai/tinychannels", features = ["email-send"] }
```

`email-send` carries no `Channel` impl — a send-only build cannot `listen`, so
the trait is gated on the full `email` feature rather than promising a
half-working channel.

All other providers (Telegram, Discord, Slack, Signal, iMessage, IRC, Yuanbao/钉钉, etc.) are included in the default build.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo build --all-targets
cargo test

# Test with all optional providers
cargo test --features email,lark
```

## Repository Layout

- `crates/tinychannels-bus/` holds shared message/envelope DTOs, serialized
  channel configuration, capabilities, controller schemas/response types,
  relay frames and errors.
- `crates/tinychannels-runtime/` owns provider/adapter callbacks, listener and
  dispatch supervision, pairing and relay crypto, persisted key/idempotency
  algorithms, history/context processing, configuration preparation and text
  segmentation. It re-exports the same bus DTOs.
- `crates/tinychannels-module/` is the loadable TinyBus module.
- `src/lib.rs` exports the crate surface and re-exports the contract.
- `src/providers/` holds the provider transports.
- `src/delivery/` holds the durable outbound queue and `progressive/`, the
  streaming reply driver (draft, thinking and filler bubbles) over a
  host-supplied `ProgressiveSender`. The reply splitter
  (`segment_for_delivery`) lives in `tinychannels-runtime` (`delivery::segment`) and
  is re-exported here.
- `src/remote/` implements `/status`, `/sessions`, `/new` and `/help` for every
  provider with the `remote_control` capability, over a host-supplied
  `RemoteControlHost`.
- `src/approvals/` sends in-chat approval prompts for every provider with the
  `chat_approvals` capability.
- `src/relay/` holds the relay transport loop, the WebSocket dialer and the
  process-wide transport registry.
- `src/backend.rs` owns `ChannelBackend` and `ChannelManager`; `src/host/` is
  the host service boundary; `src/routes.rs` and `src/runtime.rs` hold portable
  runtime helpers.

The [pure-contract relocation specification](docs/spec/pure-contract-relocation.md)
records extension-trait source migrations and subsequent module-operation work.
