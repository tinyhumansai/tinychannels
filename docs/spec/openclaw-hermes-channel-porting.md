# OpenClaw and Hermes Channel Porting Spec

This document captures a source-level audit of channel and messaging surfaces in
OpenClaw and Hermes Agent, then translates those findings into a TinyChannels
spec for OpenHuman.

Source repos audited:

- OpenClaw: https://github.com/openclaw/openclaw
- Hermes Agent: https://github.com/NousResearch/hermes-agent

Local audit checkouts used for this document:

- `/tmp/tinychannels-openclaw-src`
- `/tmp/tinychannels-hermes-agent-src`

## Provenance and Verification (2026-07-04 re-audit)

This spec was re-verified line-by-line against pinned checkouts:

- OpenClaw: `/tmp/tinychannels-openclaw-src` at commit `6445a063`
  (2026-07-04). All anchor files, the channel inventory (including
  `clickclack`, `raft`, `sms`), and both `message` and `messaging` plugin
  fields verified present.
- Hermes: `/tmp/tinychannels-hermes-agent-src` at commit `10f7cb04`
  (2026-07-04). The relay subsystem (`gateway/relay/*`), `platform_registry.py`,
  the 24-member platform enum, `SessionSource` extended fields, and the
  `SendResult` error taxonomy all verified present.

**Warning:** the checkouts under `~/work/tinyhumansai/references/{openclaw,
hermes-agent}` are ~3 months stale (April 2026) and predate the Hermes relay
subsystem, the platform registry, `scope_id`, and the structured send-error
taxonomy, plus several OpenClaw channels and the `src/channels/message/` and
`src/channels/inbound-event/` modules. Do not audit or port against them; use
the pinned `/tmp` checkouts (or a fresh upstream clone) as the source of truth.

**One material correction from the re-audit:** Hermes' `scope_id`
(`gateway/session.py:148`) is a relay-routing and tenant-ownership
discriminator; it is **not** folded into Hermes' session key
(`build_session_key` still relies on provider-unique `chat_id`). The
requirement below that TinyChannels include `scope_id` in the session
discriminator is therefore a deliberate TinyChannels improvement over both
upstreams, not a port of existing behavior. Hermes is also mid-migration from
the deprecated `guild_id` wire key to `scope_id` (dual-write on serialize,
dual-read on parse, tagged "D-Q2.5") — any wire-compatible port must accept
both keys and emit `scope_id` as canonical.

The detailed, phase-by-phase implementation plan derived from this spec lives
in [tinychannels-execution-plan.md](tinychannels-execution-plan.md).

## Summary

OpenClaw and Hermes solve the same problem at different layers.

OpenClaw has a rich native channel plugin contract. A channel plugin declares
metadata, setup, config, secrets, status, gateway lifecycle, inbound
normalization, outbound delivery, directory lookup, actions, approvals, pairing,
threading, streaming, and durable receipts. Its core is already close to the API
shape TinyChannels should expose.

Hermes has a gateway platform adapter contract. Every platform normalizes
provider events into `MessageEvent` plus `SessionSource`, then implements
`connect`, `disconnect`, `send`, and delivery helpers. Hermes also has a newer
generic relay connector contract that negotiates a capability descriptor over an
authenticated outbound WebSocket, so a hosted gateway does not need an inbound
public port.

For OpenHuman, TinyChannels should not directly clone either system. It should
standardize a Rust channel envelope, capability descriptor, adapter trait, and
harness bridge that can host direct provider adapters, OpenClaw-style native
plugins, and Hermes-style relay connectors.

## OpenClaw Channel Inventory

OpenClaw channel availability is declared by `extensions/*/openclaw.plugin.json`
entries with a `channels` field. The channel core lives under `src/channels/`.

| Channel          | Plugin           | Credentials or setup signals                                                            | Notes                                                                         |
| ---------------- | ---------------- | --------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------- |
| `clickclack`     | `clickclack`     | `CLICKCLACK_BOT_TOKEN`                                                                  | Lightweight channel plugin.                                                   |
| `discord`        | `discord`        | `DISCORD_BOT_TOKEN`                                                                     | Channels, DMs, commands, app events, threads, reactions, native interactions. |
| `feishu`         | `feishu`         | `FEISHU_APP_ID`, `FEISHU_APP_SECRET`, `FEISHU_VERIFICATION_TOKEN`, `FEISHU_ENCRYPT_KEY` | Feishu/Lark chats plus workplace tools.                                       |
| `googlechat`     | `googlechat`     | `GOOGLE_CHAT_SERVICE_ACCOUNT`, `GOOGLE_CHAT_SERVICE_ACCOUNT_FILE`                       | Google Chat spaces and DMs.                                                   |
| `imessage`       | `imessage`       | host-local setup                                                                        | Apple Messages surface.                                                       |
| `irc`            | `irc`            | `IRC_HOST`, `IRC_PORT`, `IRC_TLS`, `IRC_NICK`, `IRC_CHANNELS`, auth vars                | IRC channel and DM workflow.                                                  |
| `line`           | `line`           | `LINE_CHANNEL_ACCESS_TOKEN`, `LINE_CHANNEL_SECRET`                                      | LINE Bot API chats.                                                           |
| `matrix`         | `matrix`         | `MATRIX_HOMESERVER`, `MATRIX_USER_ID`, `MATRIX_ACCESS_TOKEN`, password/device vars      | Matrix rooms and DMs.                                                         |
| `mattermost`     | `mattermost`     | `MATTERMOST_BOT_TOKEN`, `MATTERMOST_URL`                                                | Mattermost channels and DMs.                                                  |
| `msteams`        | `msteams`        | `MSTEAMS_APP_ID`, `MSTEAMS_APP_PASSWORD`, `MSTEAMS_TENANT_ID`                           | Microsoft Teams bot conversations.                                            |
| `nextcloud-talk` | `nextcloud-talk` | `NEXTCLOUD_TALK_BOT_SECRET`, `NEXTCLOUD_TALK_API_PASSWORD`                              | Nextcloud Talk conversations.                                                 |
| `nostr`          | `nostr`          | `NOSTR_PRIVATE_KEY`                                                                     | NIP-04 encrypted DMs.                                                         |
| `qa-channel`     | `qa-channel`     | test setup                                                                              | QA channel fixture.                                                           |
| `qqbot`          | `qqbot`          | `QQBOT_APP_ID`, `QQBOT_CLIENT_SECRET`                                                   | QQ Bot groups and DMs.                                                        |
| `raft`           | `raft`           | `RAFT_PROFILE`                                                                          | Secure CLI wake bridge.                                                       |
| `signal`         | `signal`         | host-local setup                                                                        | Signal messaging surface.                                                     |
| `slack`          | `slack`          | `SLACK_BOT_TOKEN`, `SLACK_APP_TOKEN`, `SLACK_USER_TOKEN`                                | Channels, DMs, commands, app events, threads.                                 |
| `sms`            | `sms`            | Twilio SID/token/from/webhook vars                                                      | SMS via Twilio.                                                               |
| `synology-chat`  | `synology-chat`  | Synology token, incoming URL, NAS host, allowlist vars                                  | Synology Chat channel and DMs.                                                |
| `telegram`       | `telegram`       | `TELEGRAM_BOT_TOKEN`                                                                    | Telegram chats, DMs, groups, topics.                                          |
| `tlon`           | `tlon`           | Tlon/Urbit setup                                                                        | Tlon/Urbit chat workflows.                                                    |
| `twitch`         | `twitch`         | `OPENCLAW_TWITCH_ACCESS_TOKEN`                                                          | Twitch chat and moderation workflows.                                         |
| `whatsapp`       | `whatsapp`       | WhatsApp Web auth state                                                                 | WhatsApp Web chats.                                                           |
| `zalo`           | `zalo`           | `ZALO_BOT_TOKEN`, `ZALO_WEBHOOK_SECRET`                                                 | Zalo bot and webhook chats.                                                   |
| `zalouser`       | `zalouser`       | `ZALOUSER_PROFILE`, `ZCA_PROFILE`                                                       | Zalo personal account via zca-js.                                             |

Note that OpenClaw's channel id is an open string, not a closed enum:
`ChannelId = ChatChannelId | (string & {})`, with the id set derived at
runtime from the bundled channel catalog plus extension manifests
(`src/channels/ids.ts`, `bundled-channel-catalog-read.ts`). TinyChannels
should likewise treat channel ids as catalog-driven open strings.

OpenClaw also has adjacent communication extensions that are not listed as
`channels`, including `voice-call`, `webhooks`, and `device-pair`.
TinyChannels should keep the core broad enough to model voice calls and
webhooks later, but the first implementation should focus on message channels.

## OpenClaw Architecture Findings

The important OpenClaw source anchors are:

- `src/channels/plugins/types.plugin.ts`
- `src/channels/plugins/types.core.ts`
- `src/channels/plugins/types.adapters.ts`
- `src/channels/message/types.ts`
- `src/channels/inbound-event/context.ts`
- `src/channels/inbound-event/media.ts`
- `src/channels/plugins/outbound.types.ts`

Key patterns to port:

- A channel is a plugin object with `id`, `meta`, `capabilities`, `config`,
  optional `setup`, `pairing`, `security`, `groups`, `mentions`, `outbound`,
  `status`, `gateway`, `auth`, `commands`, `lifecycle`, `secrets`, `doctor`,
  `bindings`, `threading`, `message`, `messaging`, `directory`, `resolver`,
  `actions`, `heartbeat`, and `agentTools`.
- Public channel metadata includes docs paths, display labels, aliases,
  markdown capability, setup exposure, account-binding preferences, and channel
  selection behavior.
- Setup input is provider-neutral enough to cover token, secret, bot token, app
  token, HTTP endpoint, webhook, proxy, homeserver, user ID, password, device,
  profile, relay URLs, group channels, DM allowlist, and discovery flags.
- Inbound handling is fact-based. OpenClaw builds a finalized message context
  from channel, account, provider, surface, message id, sender, conversation,
  route, reply plan, access, command, media, supplemental quote/thread context,
  and visibility policy.
- Inbound media is normalized into path, URL, content type, kind, transcription
  status, and platform message id. Parallel `MediaPaths`, `MediaUrls`, and
  `MediaTypes` arrays preserve attachment indexes.
- Outbound delivery has first-class capability discovery for text, media,
  payload, poll, silent sends, reply-to, thread, native quote, batching,
  reconciliation, hooks, commit, and durable final delivery.
- Outbound receipts are normalized into one logical send with one or more
  platform message parts. This is a better model than a single `message_id`.
- Presentation capabilities are explicit: buttons, selects, context, dividers,
  text length unit, markdown dialect, edit support, and action limits.
- Channel runtime helpers are grouped into reply, routing, text, session,
  media, commands, groups, and pairing. This is the right shape for an
  OpenHuman harness bridge.

### OpenClaw details the first draft under-specified (verified 2026-07-04)

- **Receipts are a first-class type.** `src/channels/message/types.ts` defines
  `MessageReceipt { primaryPlatformMessageId?, platformMessageIds[], parts[],
  threadId?, replyToId?, editToken?, deleteToken?, sentAt, raw? }` with
  `MessageReceiptPart { platformMessageId, kind, index, threadId?, replyToId?,
  raw? }` and part kinds `text | media | voice | poll | card | preview |
  unknown`. TinyChannels should mirror this shape directly, including the
  `editToken`/`deleteToken` handles for later edit/unsend.
- **Durable delivery is a write-ahead queue, not adapter hooks.**
  `src/infra/outbound/delivery-queue*.ts`: `enqueueDelivery` persists before
  send; success → `ackDelivery`, abort → ack, throw/partial → `failDelivery`;
  crash recovery drains pending entries with `MAX_RETRIES = 5` and exponential
  backoff `[5s, 25s, 2m, 10m]`. A regex classifier
  (`isPermanentDeliveryError`: "chat not found", "bot was blocked",
  "forbidden: bot was kicked", "recipient is not a valid", ...) short-circuits
  permanent failures into a `failed/` store instead of retrying.
- **Durability is negotiated by capability.** `MessageDurabilityPolicy =
  "required" | "best_effort" | "disabled"`, and adapters advertise a
  `DurableFinalDeliveryRequirementMap` over capability keys (`text, media,
  poll, payload, silent, replyTo, thread, nativeQuote, messageSendingHooks,
  batch, reconcileUnknownSend, afterSendSuccess, afterCommit`).
- **`deliveryMode: "direct" | "gateway" | "hybrid"`** on the outbound adapter
  is a core routing axis; idempotency keys flow through the gateway path (and
  are auto-generated when absent), not direct sends.
- **Three distinct capability surfaces.** Static feature flags
  (`ChannelCapabilities`: chatTypes, polls, reactions, edit, unsend, reply,
  threads, media, nativeCommands, blockStreaming, ...), the message capability
  enum (`message-capabilities.ts`, now `presentation | delivery-pin` — the
  older `interactive/buttons/cards/components/blocks` list is stale), and the
  ~60-entry message-action name enum (`message-action-names.ts`). Do not
  collapse them.
- **Presentation limits carry the length unit.**
  `ChannelPresentationCapabilities` (`outbound.types.ts`) declares
  `limits.actions { maxActions, maxActionsPerRow, maxRows, maxLabelLength,
  maxValueBytes, supportsStyles, supportsDisabled, supportsLayoutHints }` and
  `limits.selects { maxOptions, ..., encoding: "characters" | "utf8-bytes" |
  "utf16-units", markdownDialect: "plain" | "markdown" | "html" |
  "slack-mrkdwn" | "discord-markdown", supportsEdit }`. Text length is an
  encoding enum, not "characters" — chunking correctness depends on it.
- **The send lifecycle is a state machine.** `MessageSendContext` exposes
  `render / previewUpdate / send / edit / delete / commit / fail` with
  `durability`, `attempt`, and an abort `signal`; lifecycle adapters hook
  `beforeSendAttempt / afterSendSuccess / afterSendFailure / afterCommit`.
- **Unknown-send reconciliation is a tri-state verdict.**
  `reconcileUnknownSend` returns `{ status: "sent", receipt } | { status:
  "not_sent" } | { status: "unresolved", retryable? }` — this is how crash
  recovery avoids double-sends and it must exist in any durable-delivery port.
- **Inbound is a typed facts model.** `src/channels/turn/types.ts` defines
  `SenderFacts / ConversationFacts (kind, spaceId, parentId, threadId) /
  RouteFacts (routeSessionKey, dispatchSessionKey, parentSessionKey, ...) /
  ReplyPlanFacts (sourceReplyDeliveryMode: thread | reply | channel | direct |
  none) / AccessFacts / MessageFacts / CommandFacts / InboundMediaFacts /
  SupplementalContextFacts`, consumed by `buildChannelInboundEventContext`.
- **Turn kernel lifecycle ordering.** Inbound processing runs the ordered
  stages `ingest → classify → preflight → resolve → authorize → assemble →
  record → dispatch → finalize`, with admission verdicts `dispatch |
  observeOnly | handled | drop` and event classes `message | command |
  interaction | reaction | lifecycle | unknown` (`turn/types.ts:404-467`).
- **Receive-ack policy is explicit.** `after_receive_record |
  after_agent_dispatch | after_durable_send | manual`
  (`message/types.ts:377-395`) — it decides at-least-once vs. at-most-once
  inbound semantics and belongs in the adapter contract.
- **Gateway RPC methods are scope-tagged.** `gatewayMethodDescriptors[]
  { name, scope: OperatorScope, description }` enables least-privilege
  capability tokens on the gateway path.
- **Config has both a schema layer and a repair layer.** Zod-generated config
  schemas plus a doctor subsystem (`ChannelDoctorAdapter`, `legacyConfigRules`)
  that normalizes/repairs legacy config. Allowlist access facts are projected
  with raw entries redacted (`ProjectedAllowlistAccessFacts` with reason codes
  and matched-entry ids; raw `allowFrom` lists are deprecated).
- **Config lifecycle is a contract.** `ChannelConfigAdapter` exposes
  `isConfigured / isEnabled / unconfiguredReason / disabledReason /
  hasConfiguredState / hasPersistedAuthState`, and plugins declare hot-reload
  scoping via `reload { configPrefixes, noopPrefixes }` (which config subtrees
  force a channel restart vs. a no-op).
- **Status is a state machine.** `ChannelAccountSnapshot` (~55 fields:
  `reconnectAttempts`, `lastDisconnect {at, status, error, loggedOut}`,
  `healthState`, `restartPending`) with account states `linked | not linked |
  configured | not configured | enabled | disabled` and status issue kinds
  `intent | permissions | config | auth | runtime`. Security audits return
  findings with `severity: info | warn | critical` plus remediation text.
- **Inbound context separates `Provider` from `Surface`** (e.g. provider
  `whatsapp` bridged onto surface `discord`), splits agent-facing vs.
  command-facing bodies (`BodyForAgent` vs `BodyForCommands`), and gates
  commands default-deny via `CommandAuthorized === true` with
  `UntrustedContext[]` for supplemental quoted/thread material.
- **Media index alignment is an invariant.** `MediaTypes` is padded to the
  media count with `application/octet-stream` and `MediaType` is kept in sync
  with `MediaTypes[0]` (`src/channels/inbound-event/media.ts`). Attachment
  ordering breaks if a port loses this.
- **Chunking is mode-driven.** Adapters supply `chunker`, `chunkerMode:
  "text" | "markdown"`, `textChunkLimit`, and chunk-mode resolution picks
  `undefined` (single send) / `"length"` / `"newline"` (block-aware markdown
  chunking, then per-block limit re-chunk). Streaming config layers
  `coalesceIdleMs / maxChunkChars / deliveryMode: "live" | "final_only" /
  hiddenBoundarySeparator`. Draft streaming uses finalizable throttled
  controls with `update / stop / stopForClear` and interim-draft deletion on
  cancel.
- **Session grammar hooks are where topic/thread routing lives.**
  `resolveSessionConversation`, ordered `parentConversationCandidates`
  (narrowest → broadest), and `resolveOutboundSessionRoute →
  { sessionKey, baseSessionKey, peer {kind, id}, chatType, from, to,
  threadId }` plus legacy-session-key canonicalization
  (`types.core.ts:471-556`).

## Hermes Channel Inventory

Hermes calls channels "platforms". Built-in platform enum values in
`gateway/config.py` include:

- `local`
- `telegram`
- `discord`
- `whatsapp`
- `whatsapp_cloud`
- `slack`
- `signal`
- `mattermost`
- `matrix`
- `homeassistant`
- `email`
- `sms`
- `dingtalk`
- `api_server`
- `webhook`
- `msgraph_webhook`
- `feishu`
- `wecom`
- `wecom_callback`
- `weixin`
- `bluebubbles`
- `qqbot`
- `yuanbao`
- `relay`

Hermes also discovers platform plugins under `plugins/platforms/`:

| Platform plugin | Label               | Primary transport                               |
| --------------- | ------------------- | ----------------------------------------------- |
| `dingtalk`      | DingTalk            | DingTalk stream SDK.                            |
| `discord`       | Discord             | `discord.py`.                                   |
| `email`         | Email               | IMAP polling plus SMTP replies.                 |
| `feishu`        | Feishu / Lark       | Lark SDK over WebSocket or webhook.             |
| `google_chat`   | Google Chat         | Cloud Pub/Sub pull plus REST.                   |
| `homeassistant` | Home Assistant      | HA WebSocket event bus plus REST notifications. |
| `irc`           | IRC                 | Asyncio IRC protocol.                           |
| `line`          | LINE                | aiohttp webhook plus LINE Messaging API.        |
| `matrix`        | Matrix              | mautrix, optional E2EE.                         |
| `mattermost`    | Mattermost          | REST plus WebSocket event stream.               |
| `ntfy`          | ntfy                | HTTP streaming plus POST.                       |
| `photon`        | iMessage via Photon | Spectrum SDK sidecar over long-lived gRPC.      |
| `raft`          | Raft                | Loopback wake bridge plus Raft CLI.             |
| `simplex`       | SimpleX Chat        | Local simplex-chat daemon WebSocket.            |
| `slack`         | Slack               | Slack Bolt Socket Mode.                         |
| `sms`           | SMS via Twilio      | Twilio REST plus inbound webhook.               |
| `teams`         | Microsoft Teams     | Bot Framework webhook.                          |
| `telegram`      | Telegram            | python-telegram-bot.                            |
| `wecom`         | WeCom               | WebSocket smart robot and callback app mode.    |
| `whatsapp`      | WhatsApp            | Local Node bridge over HTTP API.                |

Hermes platform manifests define setup prompts through `requires_env` and
`optional_env`. They consistently include allowed-user env vars and home-channel
env vars so cron and notifications can target each platform.

## Hermes Architecture Findings

The important Hermes source anchors are:

- `gateway/platforms/base.py`
- `gateway/session.py`
- `gateway/config.py`
- `gateway/platform_registry.py`
- `gateway/platforms/ADDING_A_PLATFORM.md`
- `gateway/relay/descriptor.py`
- `gateway/relay/adapter.py`
- `docs/relay-connector-contract.md`

Key patterns to port:

- `BasePlatformAdapter` is the platform boundary. Required operations are
  `connect`, `disconnect`, `send`, and `get_chat_info`; optional operations
  cover media, typing, voice, documents, video, GIFs, interactive prompts,
  draft streaming, editing, and platform-specific rendering.
- `MessageEvent` normalizes inbound text, message type, source, raw message,
  message id, media paths/types, reply context, auto-loaded skills, channel
  prompt, channel context, internal flag, metadata, and timestamp.
- `SessionSource` is the routing key. It includes platform, chat id, chat type,
  chat name, user id, user name, thread id, chat topic, alternate ids,
  `scope_id`, parent chat id, message id, profile, and internal upstream relay
  trust state.
- `SendResult` carries success, message id, raw provider response, retryability,
  retry-after, continuation message ids, and machine-readable error kind.
- Platform config includes enabled state, token/api key, home channel,
  reply-to mode, restart notification policy, typing indicator policy, channel
  overrides, and platform-specific extra data.
- `PlatformRegistry` lets plugin adapters register metadata and factories
  lazily, including config validation, required env, setup functions,
  allowed-user envs, max message length, privacy flags, platform hints,
  cron home-channel envs, and standalone sender functions.
- Hermes has a mature generic relay pattern. A connector sends a
  `CapabilityDescriptor` with platform, max message length, draft streaming,
  edit support, thread support, markdown dialect, length unit, display emoji,
  platform hint, and PII-safety. Inbound and passthrough frames ride the
  authenticated outbound WebSocket, avoiding public inbound gateway ports.
- Hermes treats authorization as explicit. Direct adapters default to gateway
  allowlist checks; relay adapters can mark authorization as delegated to a
  trusted upstream only after authenticated connector routing.

### Hermes details the first draft under-specified (verified 2026-07-04)

- **Session keys and `scope_id` are different axes.** `build_session_key`
  (`gateway/session.py`) composes `{namespace}:{platform}:{chat_type}:
  {chat_id}[:{thread_id}][:{participant}]` where the namespace is
  `agent:{profile}` (default `agent:main`). `scope_id` is **not** a key
  component — it drives relay routing tables and tenant-ownership gating.
  Guild/workspace isolation relies on provider-unique `chat_id`.
- **Thread sessions are shared by default.** `group_sessions_per_user=True`
  appends a participant id to group keys, but inside a thread
  `thread_sessions_per_user=False` (default) forces per-user isolation OFF so
  all participants of a forum topic / Discord thread / Slack thread share one
  session. Both toggles must survive the port as policy, not hardcode.
- **`SendResult` error taxonomy.** `SEND_ERROR_KINDS = { too_long, bad_format,
  forbidden, not_found, rate_limited, transient, unknown }` with
  `classify_send_error()`, `retry_after` (honors Telegram FloodWait),
  `continuation_message_ids` (multi-part sends; `message_id` is the LAST
  visible part), and `is_chat_level_not_found()` distinguishing a dead chat
  from a deleted thread/message (only chat-level not_found marks a target
  dead). A documented `raw_response["partial_overflow"]` contract carries
  `delivered_chunks / total_chunks / last_message_id / delivered_prefix /
  continuation_message_ids` so streaming can deliver the missing tail instead
  of treating a clipped edit as complete.
- **Send retry excludes timeouts on purpose.** `_send_with_retry` retries
  transient/network errors with backoff + jitter, but read/write timeouts are
  NOT retried because a non-idempotent send may have landed; a permanent
  formatting failure falls back to plain text. Any Rust error enum must encode
  this idempotency reasoning, not just a retryable bool.
- **Chunking is UTF-16-aware only for Telegram.** `utf16_len` counts UTF-16
  code units (`len(s.encode("utf-16-le")) // 2`); `truncate_message` prefers
  newline/space split points, preserves triple-backtick fences across chunks
  (closes and reopens with the carried language tag), avoids splitting inline
  code spans, and reserves room for `(i/N)` indicators. Only Telegram passes
  the UTF-16 length function — every other Hermes adapter measures codepoints,
  a latent emoji bug TinyChannels should fix uniformly via a per-channel
  length-unit capability.
- **Inbound media are local cache paths, not URLs.** Media is eagerly
  downloaded to a cache with magic-byte validation, SSRF guards (including
  redirect re-validation), retry, and TTL cleanup, because provider URLs
  expire. The envelope's media references should model this explicitly.
- **PII redaction happens at the LLM boundary only.** A `pii_safe` platform
  set (WhatsApp, Signal, Telegram, BlueBubbles) hashes ids in the system
  prompt while routing keeps raw ids; Discord is deliberately excluded because
  mentions need raw `<@id>`. Model "id-safe-to-redact" as a capability.
- **Concurrency discipline.** The active-session guard is set synchronously
  before spawning the agent task (double-spawn race), photo bursts/albums are
  merged into one pending event instead of interrupting, and control commands
  (`/approve /deny /stop /new ...`) bypass the active-session guard so an
  agent blocked on approval cannot deadlock.
- **Relay contract specifics.** `CapabilityDescriptor` is frozen for the
  connection lifetime, `CONTRACT_VERSION = 1`, unknown JSON keys are ignored
  and missing keys defaulted (additive-only evolution), and
  `max_message_length == 0` maps to 4096. Frames (connector → gateway):
  `descriptor, inbound (with optional bufferId ack for durable replay),
  going_idle_ack, outbound_result, interrupt_inbound, passthrough_forward`;
  (gateway → connector): `outbound, chat_info, interrupt, going_idle,
  inbound_ack`. Auth is two HMAC-SHA256 schemes with multi-secret rotation and
  constant-time verification: a WS-upgrade bearer token
  `base64url("{gateway_id}:{exp}:{sig}")` (TTL 300s) and a per-tenant inbound
  delivery signature over `"{timestamp}.{body}"` exact bytes with a 300s
  replay window. The wire bytes must match the TypeScript connector exactly.
- **Delegated trust is forgery-resistant.** `delivered_via_upstream_relay` is
  excluded from `SessionSource` serialization so a peer cannot set it over the
  wire; only the authenticated relay transport stamps it, and the authz check
  uses a strict identity comparison (`is True`). Relayed events carry the
  UNDERLYING platform (e.g. `discord`), not `relay`, so authorization keys off
  the trust flag, never the platform value.

## TinyChannels Scope for OpenHuman

TinyChannels should define the stable typed layer between OpenHuman channel
surfaces and OpenHuman harness runtimes.

It should own:

- channel identity and account identity
- capability descriptors
- inbound event envelopes
- outbound message intents
- normalized media references
- routing and session discriminators
- delivery receipts and durable send state
- adapter lifecycle and status snapshots
- harness turn boundaries
- authorization, allowlist, and mention-gating facts
- observability metadata without raw secret or unnecessary PII leakage

It should not own:

- provider SDK internals
- OpenHuman UI widgets
- model or agent execution
- provider credential storage implementation
- provider-specific long-running daemons

## Proposed Core Types

The initial Rust modules should stay close to the existing crate boundaries:

- `src/channel/types.rs`
- `src/channel/adapter.rs`
- `src/channel/capabilities.rs`
- `src/channel/envelope.rs`
- `src/channel/receipt.rs`
- `src/harness/types.rs`
- `src/harness/bridge.rs`

### Channel Descriptor

Each adapter exposes a descriptor:

```rust
pub struct ChannelDescriptor {
    pub id: ChannelId,
    pub label: String,
    pub provider: String,
    pub transport: ChannelTransportKind,
    pub auth: ChannelAuthKind,
    pub capabilities: ChannelCapabilities,
}
```

Required capability families:

- text length limit and length unit: characters, UTF-8 bytes, UTF-16 units
- markdown dialect: plain, markdown, HTML, Slack mrkdwn, Discord markdown,
  Telegram MarkdownV2
- media input and output support
- reply, thread, edit, delete, reaction, typing, and draft support
- interactive support: buttons, selects, approval prompts
- streaming behavior: draft, edit-in-place, segment sends, final fresh send
- durable delivery support and receipt granularity
- directory, target resolver, pairing, setup, login/logout, status
- async delivery and cron/home-channel support
- authorization mode: local policy, upstream delegated, owner-only, open with
  explicit opt-in

### Inbound Envelope

```rust
pub struct ChannelInboundEnvelope {
    pub id: ChannelEventId,
    pub provider_event_id: Option<String>,
    pub occurred_at_ms: i64,
    pub received_at_ms: i64,
    pub channel: ChannelRef,
    pub conversation: ConversationRef,
    pub sender: SenderRef,
    pub body: InboundBody,
    pub reply: Option<ReplyContext>,
    pub route: RouteContext,
    pub access: AccessContext,
    pub raw: RawEventRef,
}
```

The session discriminator must include channel id, account id, conversation id,
thread id, sender id where appropriate, and `scope_id` for workspace/server
isolation. Discord guilds, Slack workspaces, Matrix homeservers, and Teams
tenants must never collapse into one OpenHuman session because they share a
channel id or user id.

### Outbound Intent

```rust
pub struct ChannelOutboundIntent {
    pub id: ChannelOutboundId,
    pub target: ChannelTarget,
    pub payload: OutboundPayload,
    pub options: OutboundOptions,
    pub durability: DeliveryDurability,
    pub idempotency_key: Option<String>,
}
```

The payload must support text, media, voice, files, polls, portable
presentation blocks, native channel data, and tool/progress visibility hints.
The result must be a logical receipt with zero or more platform message parts.

### Adapter Trait

The first adapter trait should be async and capability-driven:

```rust
pub trait ChannelAdapter {
    fn descriptor(&self) -> &ChannelDescriptor;
    async fn start(&mut self, sink: ChannelInboundSink) -> Result<()>;
    async fn stop(&mut self) -> Result<()>;
    async fn send(&self, intent: ChannelOutboundIntent) -> Result<ChannelReceipt>;
    async fn status(&self) -> Result<ChannelAccountSnapshot>;
}
```

Optional extension traits should cover setup, login/logout, directory lookup,
target resolution, typing, reaction, edit/delete, streaming draft, and relay
handshake. Do not put every optional platform feature on the base trait.

## Harness Bridge

OpenHuman harnesses should consume `ChannelTurn` objects, not provider events:

```rust
pub struct ChannelTurn {
    pub inbound: ChannelInboundEnvelope,
    pub session: HarnessSessionRef,
    pub context: HarnessChannelContext,
}
```

Harness output should return `ChannelOutputEvent` values:

- assistant text delta
- final assistant message
- tool/progress event
- approval request
- clarification request
- media/file output
- cancellation/interruption
- lifecycle status

The bridge translates these into outbound intents using channel capabilities.
This keeps provider quirks outside the agent runtime.

## Porting Priorities

1. Core normalized types, descriptors, receipts, and harness bridge.
2. Generic local/API/webhook/relay adapter, because it unblocks OpenHuman
   internal surfaces and hosted connectors without public inbound ports.
3. Discord, Telegram, and Slack, because both OpenClaw and Hermes have mature
   implementations and they exercise DMs, groups, threads, mentions,
   interactions, edits, reactions, streaming, and allowlists.
4. WhatsApp, SMS, email, and iMessage/Photon, because they exercise weaker
   formatting, phone-number PII, session windows, sidecars, webhooks, and
   non-threaded delivery.
5. Matrix, Mattermost, Teams, Google Chat, Feishu/Lark, LINE, and WeCom, because
   they exercise enterprise tenant boundaries, cards/buttons, webhook crypto,
   and workspace directory lookups.
6. Long tail adapters such as IRC, ntfy, SimpleX, Nostr, Tlon, Zalo, QQBot,
   Twitch, Home Assistant, Raft, Synology Chat, and Nextcloud Talk.

## Acceptance Tests

The first implementation should include fixtures for these behaviors:

- session keys do not collide across account, workspace, guild, room, thread,
  topic, or sender boundaries
- inbound text and media normalize into stable envelope fields
- sender authorization, allowlists, mention gating, and delegated upstream auth
  are explicit in the envelope
- outbound text chunks by channel length unit, including UTF-16 Telegram-style
  limits
- receipts preserve every platform message id in a multi-part delivery
- failed sends classify retryable, rate-limited, forbidden, not-found,
  too-long, bad-format, transient, and unknown cases
- reply, thread, edit, delete, reaction, typing, and draft capabilities degrade
  predictably when unsupported
- interactive approvals degrade from native buttons to text commands
- relay connectors can deliver inbound over an authenticated outbound socket
  without a public inbound port
- adapter shutdown drains active sends and records terminal status
- docs examples remain aligned with public API names

A concrete fixture catalog mapping each behavior above to the upstream test
files worth replicating (OpenClaw `src/channels/message/receipt.test.ts`,
`delivery-queue.recovery.test.ts`, `inbound-event/media.test.ts`,
`routing/session-key.test.ts`, `message-capability-matrix.test.ts`; Hermes
`tests/gateway/relay/*`, session/allowlist/batching suites) lives in
[tinychannels-execution-plan.md](tinychannels-execution-plan.md).

## Open Decisions (with recommendations)

- Whether TinyChannels should define a stable plugin ABI now or only Rust
  traits consumed by OpenHuman first. **Recommendation: Rust traits only.**
  Both upstreams iterated their plugin contracts heavily this quarter (OpenClaw
  grew `message`, `gatewayMethodDescriptors`, and the turn kernel; Hermes grew
  the registry and relay). Freezing an ABI now would freeze the wrong shape.
- Whether provider credentials are represented only as opaque secret refs or
  whether first-party local adapters may receive resolved secret strings.
  **Recommendation: opaque `SecretRef` in all TinyChannels types, with a
  `SecretResolver` trait implemented by the host; first-party adapters resolve
  at the transport edge.** This matches OpenClaw's projected/redacted
  allowlist facts and keeps envelopes PII/secret-clean for observability.
- Whether relay should be a first-class adapter from day one or follow direct
  local/webhook adapters. **Recommendation: port the relay `CapabilityDescriptor`
  and HMAC auth types early (they are small, frozen-contract, and testable
  against the upstream conformance suite), but ship the relay transport after
  the direct local/webhook adapter.** Hermes marks the contract EXPERIMENTAL
  (contract_version 1, additive-only), so track it behind a feature flag.
- How much OpenHuman UI setup metadata belongs in TinyChannels versus the app.
  **Recommendation: keep the existing `ChannelDefinition`/`AuthModeSpec` form
  metadata in the crate (already ported and consumed by controllers), but keep
  wizard flows and rendering in OpenHuman.**
- Which receipt store owns durable delivery commits. **Recommendation:
  TinyChannels owns the delivery-queue state machine (enqueue → ack/fail,
  retry/backoff, permanent-error classification, unknown-send reconciliation)
  behind a `DeliveryQueueStore` storage trait; OpenHuman provides the store
  implementation.** This mirrors OpenClaw's split between queue policy and
  queue storage and keeps crash-recovery semantics testable in the crate.

## Attribution

This spec is derived from source-level review of OpenClaw (commit `6445a063`)
and Hermes Agent (commit `10f7cb04`), both dated 2026-07-04.
Important upstream files:

- OpenClaw `src/channels/turn/types.ts`
- OpenClaw `src/channels/inbound-event/context.ts`
- OpenClaw `src/channels/inbound-event/media.ts`
- OpenClaw `src/infra/outbound/deliver.ts`
- OpenClaw `src/infra/outbound/delivery-queue-recovery.ts`
- OpenClaw `src/channels/plugins/types.plugin.ts`
- OpenClaw `src/channels/plugins/types.core.ts`
- OpenClaw `src/channels/plugins/types.adapters.ts`
- OpenClaw `src/channels/message/types.ts`
- OpenClaw `src/channels/plugins/outbound.types.ts`
- OpenClaw `extensions/*/openclaw.plugin.json`
- Hermes `gateway/platforms/base.py`
- Hermes `gateway/session.py`
- Hermes `gateway/config.py`
- Hermes `gateway/platform_registry.py`
- Hermes `gateway/relay/descriptor.py`
- Hermes `gateway/relay/adapter.py`
- Hermes `gateway/relay/auth.py`
- Hermes `gateway/relay/ws_transport.py`
- Hermes `gateway/authz_mixin.py`
- Hermes `docs/relay-connector-contract.md`
