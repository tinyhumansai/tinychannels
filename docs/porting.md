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

## Provider Wire Adapters (ported behind the host boundary)

Update 2026-07-17: the provider wire implementations have since been ported.
`src/providers/` now owns dingtalk, discord, email, imessage, irc, lark, linq,
mattermost, qq, signal, slack, telegram, whatsapp, whatsapp_web, and yuanbao.
The app-side dependencies were reduced to a host-owned service boundary
(`src/host/`: HTTP proxy client, approval, voice/STT, pairing, and
conversation-memory contracts) that providers call instead of importing
OpenHuman internals. In OpenHuman the corresponding `channels/providers/*.rs`
files are now one-line re-exports of the crate providers; Telegram keeps only
its host-coupled glue (`remote_control`, `bus`, `approval_surface`).

The original 2026-07-04 portability ladder, now resolved:

- Self-contained (no cross-module OpenHuman imports): email, irc, yuanbao,
  imessage, mattermost, qq, dingtalk — ported.
- Needed only a configured-HTTP-client trait: discord, slack, whatsapp, lark,
  signal — ported via the host proxy-client contract.
- Needed approval, voice/STT, pairing, and conversation-memory traits:
  telegram — ported via the host service boundary.
- Not porting targets (app-side consumers of this crate): the `runtime/`
  dispatch engine and the `web` provider — unchanged, they stay in OpenHuman.

Remaining provider debt: WhatsApp Web still uses `wacore`'s in-memory session
backend and re-links after restart, pending a rusqlite-backed durable store
(see the remaining-work plan). The finishing slices for the migration are
tracked in [spec/tinychannels-migration-remaining-plan.md](spec/tinychannels-migration-remaining-plan.md).

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
