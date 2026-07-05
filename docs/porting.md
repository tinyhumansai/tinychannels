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
- Migrated tests for the surfaces above.

## Backend Boundary

Runtime side effects are delegated through `ChannelBackend`:

- connect/disconnect/status/test channel operations
- message, reaction, and thread operations
- Telegram and Discord managed-link flows
- Discord guild/channel/permission lookup
- default channel persistence

OpenHuman should implement `ChannelBackend` with its existing REST client,
session JWT, config persistence, credential storage, health, and event-bus
plumbing.

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

The spec's redesigned core (descriptors, envelopes, intents, receipts,
capabilities, adapter trait, harness bridge, error taxonomy, chunking, relay)
is **not implemented yet** — what exists is the legacy surface listed above.
OpenHuman does not yet depend on this crate; the ported files are duplicated
copies in both repos and will drift until the dependency lands.

The phase-by-phase implementation plan, known-bug list, and test-migration
plan live in
[spec/tinychannels-execution-plan.md](spec/tinychannels-execution-plan.md).
The OpenHuman-side integration plan (dependency adoption, duplicate deletion,
`ChannelBackend` implementation) lives in
`openhuman-4/docs/plans/tinychannels-integration.md`.
