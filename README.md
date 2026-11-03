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

> **Channel contract:** the normative v2 target and migration rules are in
> [docs/spec/contract-v2.md](docs/spec/contract-v2.md). This README describes
> the current crate and implementation; it does not define the v2 wire
> contract.

The configured-provider factory returns adapter-backed `BuiltChannel` values
to the module runtime. `ChannelAdapter` and its optional ports define that
runtime surface; legacy `Channel` APIs remain for source compatibility and are
not the production factory path. The v2 bus types, account-specific capability
descriptors, relay transport, and progressive delivery operations are in use.
Support still varies by configured provider and installed host services. See
the [architecture](docs/channel-architecture.md),
[capability index](docs/channel-capabilities.md),
[contracts](docs/channel-contracts.md), [delivery](docs/channel-delivery.md),
and [provider capability and conformance matrix](docs/channel-providers.md).
The [contract specification](docs/spec/contract-v2.md) records the normative
wire contract and historical migration requirements; use the current provider
matrix and runtime descriptors for implementation status.

Configured provider discovery, legacy intent normalization, and external-webhook
outbound startup are confidential module operations. The catalog returns provider
facts without listening; outbound-only startup preserves Linq and WhatsApp Cloud
sends while keeping their ingress explicitly external. Existing listener start
methods retain their refusal when no webhook ingress is available.

Inbound preparation and hosted identity projection are confidential module
operations. They return stable history, thread, client, and session keys using
the module’s existing provider rules. Preparation preserves rich envelope data
and rejects conflicting legacy identities. Hosts retain authenticated transport
provenance separately: the envelope’s relay trust flag is intentionally omitted
from serialization. Namespaces are limited to 256 bytes, identity requests to
4 KiB, and inbound requests to the bounded preparation payload limit.

Progressive replies run inside the private channel runtime through confidential
`ExecuteProgressive` commands. The module owns drafts, thinking and filler
bubbles, typing timers, error recovery, normalization, and idempotency keys;
`DispatchControllerBackend` supplies only the host's managed transport with
typed outcomes. Reply handles cannot cross runtimes. Terminal cleanup survives
caller cancellation, identical finalization retries do not send twice, and
shutdown joins replies before closing callbacks. Limits are 16 active replies,
128 retained terminal handles, 32 events and 128 KiB per command, 384 KiB per
accumulator, and a three-minute worker lifetime.

Runtime model/provider commands are prepared through confidential
`PrepareRuntimeCommand`. Hosts supply the current route, provider aliases and
an already-loaded model catalog. The module preserves the existing Telegram
and Discord command parser, unknown-provider proposals and help text, including
the first matching provider's first ten cached model IDs. It returns a proposal
or a ready-to-send reply; applying route changes, validating model IDs, clearing
history and reading product catalog storage remain host responsibilities.
Unrecognized commands return no proposal. Requests obey the existing bounded
preparation payload limit and fail after module shutdown.

## Intended Scope

- channel abstractions for inbound and outbound message streams
- harness-facing communication contracts
- transport-neutral message envelopes and routing metadata
- adapters for OpenHuman channel surfaces
- observability and lifecycle hooks around channel traffic

Provider runtime work runs through TinyChannels module operations. OpenHuman
owns application policy, identity, authorization, and agent/session behavior;
it supplies scoped host services such as media, voice, events, and storage
through the module boundary. TinyChannels owns provider transports,
normalization, and delivery policy.

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

# Build and load the actual cdylib against local websocket and SMTP fixtures
./scripts/verify-module-artifact.sh

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
- `crates/tinychannels-client/` is the host-side typed API for confidential
  module calls and identity-scoped runtime leases. It binds the attested module
  once, then keeps reservation and object calls on that same peer.
- The module keeps its original `StartChannel`, `StopChannel`, `SendMessage`,
  `ListChannels` and `ChannelStatus` members, and adds relay reservation/start/
  stop/status/send plus `Shutdown`, typed `SendChannelIntent`, and send-only
  `SendEmail`. Relay startup configuration and SMTP credentials are host
  hydrated and sent only over TinyBus confidential calls. Relay handles are
  bounded single-use leases; accepted sends run in bounded module-owned queues
  so dropping a call waiter does not cancel the operation. `SendChannelIntent`
  preserves the idempotency and thread fields for the host's delivery policy.
  Until a host durable-queue port is installed, `Required` durability is
  refused before provider send; the module does not own a durable outbox.
  Legacy `SendEmail` remains available for small requests; larger attachments
  use the confidential `BeginEmailUpload` / `AppendEmailUploadChunk` /
  `SendEmailUpload` protocol, with bounded chunks and a replayable terminal
  result keyed by the opaque upload handle.
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

Hosts construct `TinyChannelsClient` from their `ModuleClient` and ask it for a
strictly attested runtime driver. The host then leases one identity-scoped
module runtime, supplies the callback object for that identity, uses the typed
object returned by the lease, and closes the lease during sign-out or shutdown:

```rust,ignore
let client = TinyChannelsClient::new(module_client);
let driver = client.runtime_driver().await?;
let lease = runtime
    .lease(
        driver,
        |reservation, _object| make_identity_callback(reservation),
        retry_policy,
        report_lease_event,
    )
    .await?;

let channels = lease.object();
channels
    .describe_configured_channels(config, proxy)
    .await?;

lease.close().await?;
```

The host owns `ModuleClient`, `ModuleRuntime`, callback registration, retry
policy, configuration and credentials. A lease's object methods remain bound to
the peer selected by `runtime_driver`; closing the lease stops that reservation
and releases its callback object without shutting down the shared module.

The [pure-contract relocation specification](docs/spec/pure-contract-relocation.md)
records extension-trait source migrations and subsequent module-operation work.

### Prepared channel connections

`ChannelController` accepts the additive `PrepareConnect` request. TinyChannels
validates credentials, verifies native email or Yuanbao authentication, and
returns a typed provider configuration update alongside the existing connection
result. The host persists the update and retains credential custody. Email
passwords and Yuanbao application secrets are removed from the update. Existing
`Connect` requests and all member argument counts remain unchanged; this
capability requires module operation contract 1.8.

### Module-owned channel health checks

`CheckChannelHealth(config, proxy, timeout_ms)` checks configured providers without
starting their listeners. Hydrated credentials and proxy settings require a
confidential TinyBus call. The per-provider budget is bounded to 1–60,000 ms;
results retain provider order and distinguish healthy, unhealthy, and timeout.
Hosts own credential hydration and presentation. `ChannelStatus(name)` keeps its
existing arity and continues to report a running provider's state.

### Remote chat command boundary

`ExecuteRemote` parses and renders `/status`, `/sessions`, `/new`, and `/help`
inside the module and owns persisted chat bindings and busy-state updates. The
host supplies validated workspace configuration, route/history facts, and
conversation/session operations through the existing confidential backend
callback. The module awaits host history clearing before committing a new
binding. Ordinary messages and unsupported providers produce no remote reply.
The additive contract is revision 14 / operation version 1.15; existing members
and callback arity are unchanged.
