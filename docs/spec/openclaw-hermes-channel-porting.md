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

## Open Decisions

- Whether TinyChannels should define a stable plugin ABI now or only Rust traits
  consumed by OpenHuman first.
- Whether provider credentials are represented only as opaque secret refs or
  whether first-party local adapters may receive resolved secret strings.
- Whether relay should be a first-class adapter from day one or follow direct
  local/webhook adapters.
- How much OpenHuman UI setup metadata belongs in TinyChannels versus the app.
- Which receipt store owns durable delivery commits: TinyChannels, OpenHuman
  shell state, or the per-adapter provider state.

## Attribution

This spec is derived from source-level review of OpenClaw and Hermes Agent.
Important upstream files:

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
- Hermes `docs/relay-connector-contract.md`
