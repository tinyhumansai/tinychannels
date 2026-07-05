# OpenHuman Channels Port

This crate now owns the portable parts of `openhuman-4/src/openhuman/channels`
that can compile independently from the OpenHuman application crate.

## Ported

- `Channel`, `ChannelMessage`, and `SendMessage` from the channel trait surface.
- Channel configuration structs from `openhuman/config/schema/channels.rs`.
- Static channel definitions and auth-mode metadata from
  `channels/controllers/definitions.rs`.
- Shared controller response types from `channels/controllers/ops/types.rs`.
- Portable runtime helpers for conversation keys, memory-context rendering,
  route commands, model-cache previews, and listener in-flight sizing.
- Core channel descriptors, envelopes, outbound intents, receipts, capability
  surfaces, send-error taxonomy, and session-key helpers.
- Portable text chunking with UTF-16, markdown fence, and continuation
  indicator handling.
- Adapter and harness bridge contracts, including local host-owned delivery.
- Durable delivery queue policy/state machine behind a host-owned storage
  trait.
- Relay descriptor and HMAC auth primitives with Hermes connector vector tests.
- Typed relay frame contracts for handshake, inbound delivery, outbound result,
  passthrough-forward, interrupt, idle, and buffered ACK flows.
- Relay frame transport loop for handshake readiness, outbound result
  correlation, authenticated inbound dispatch, passthrough dispatch, interrupt
  dispatch, idle ACKs, and buffered delivery ACKs.
- Feature-gated WebSocket relay I/O with Hermes URL normalization,
  newline-delimited JSON I/O, upgrade bearer auth, and reconnect dialer support.
- Reconnect supervisor that redials through a host-provided dialer, swaps the
  active frame I/O, re-sends `hello`, and waits for a fresh descriptor.
- Migrated tests for the surfaces above.

## Backend Boundary

Runtime side effects are delegated through `ChannelBackend`:

- connect/disconnect/status/test channel operations
- message, reaction, and thread operations
- Telegram and Discord managed-link flows
- Discord guild/channel/permission lookup
- default channel persistence

OpenHuman implements `ChannelBackend` in
`openhuman-4/src/openhuman/channels/controllers/backend.rs` with its existing
REST client, session JWT, config persistence, credential storage, health, and
event-bus plumbing.

## Not Yet Ported

Provider wire implementations such as Telegram, Discord, Web, Email, Slack,
Yuanbao, and others still depend directly on OpenHuman application services.
Move them only after their app dependencies are reduced to transport/config
traits that can live in this crate without importing OpenHuman internals.

Per the 2026-07-04 audit, provider portability falls into a ladder:

- Self-contained today (no cross-module OpenHuman imports): email, irc,
  yuanbao, cli, imessage, mattermost, qq, dingtalk, presentation.
- Need only a configured-HTTP-client trait (they import
  `build_runtime_proxy_client`): discord, slack, whatsapp, lark, signal.
- Need approval, voice/STT, pairing, and conversation-memory traits first:
  telegram.
- Not porting targets (they are the app-side consumers of this crate): the
  `runtime/` dispatch engine and the `web` provider.

## Current Status and Next Steps

The spec's redesigned local core through Phase 5 is implemented in this crate.
OpenHuman now depends on this crate through a path dependency and has adopted
the shared traits, controller metadata/types, config structs, runtime helpers,
text chunker, send idempotency bridge, relay runtime config shape, and legacy
inbound envelope projection. OpenHuman has a relay inbound handler seam for
authenticated envelope frames and starts the WebSocket relay runtime for
complete relay config. TinyChannels now owns backend-agnostic default-channel
validation before delegating persistence to the host backend, plus the Hermes
relay `send` action projection used by OpenHuman outbound relay sends for
configured relay identities. Relay inbound dispatch now preserves the original
TinyChannels envelope, including `scope_id`, through OpenHuman received-message
events. OpenHuman records TinyChannels session keys as conversation metadata
for migration. Provider wire adapters and full non-relay session-key identity
adoption are still pending.

The phase-by-phase implementation plan, known-bug list, and test-migration
plan live in
[spec/tinychannels-execution-plan.md](spec/tinychannels-execution-plan.md).
The OpenHuman-side integration plan (dependency adoption, duplicate deletion,
`ChannelBackend` implementation) lives in
`openhuman-4/docs/plans/tinychannels-integration.md`.
