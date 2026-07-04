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
