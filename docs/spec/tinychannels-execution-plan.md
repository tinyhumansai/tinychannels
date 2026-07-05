# TinyChannels Execution Plan

This is the implementation plan derived from the 2026-07-04 audit of this
crate, the OpenHuman app (`openhuman-4`), and the pinned upstream checkouts:

- OpenClaw `/tmp/tinychannels-openclaw-src` @ `6445a063` (2026-07-04)
- Hermes `/tmp/tinychannels-hermes-agent-src` @ `10f7cb04` (2026-07-04)

Do **not** use `~/work/tinyhumansai/references/{openclaw,hermes-agent}` — those
checkouts are ~3 months stale and predate the subsystems this plan ports
(OpenClaw `channels/message`, `channels/inbound-event`, `channels/turn`;
Hermes `gateway/relay`, `platform_registry`, `scope_id`, send-error taxonomy).
If the `/tmp` checkouts are gone, re-clone upstream and pin fresh commits here.

The companion research spec is
[openclaw-hermes-channel-porting.md](openclaw-hermes-channel-porting.md).
The OpenHuman-side integration plan lives in
`openhuman-4/docs/plans/tinychannels-integration.md`.

## Current State (audited 2026-07-04)

The crate compiles with zero warnings (`cargo check --all-targets`, clippy
clean) and passes 60 unit tests. What exists is a faithful lift of OpenHuman's
*legacy* channel surface, not the spec's redesign:

| Surface | Status |
| --- | --- |
| `Channel` / `ChannelMessage` / `SendMessage` (`src/traits.rs`) | Ported, legacy shape |
| Provider config structs (`src/config.rs`, 16 providers) | Ported |
| Static definitions + auth modes (`src/controllers/definitions.rs`, 8 channels) | Ported |
| Controller response types (`src/controllers/types.rs`) | Ported |
| `ChannelBackend` + `ChannelManager` (`src/backend.rs`) | New seam, matches porting.md |
| Runtime helpers (`src/context.rs`, `src/routes.rs`, `src/runtime.rs`) | Ported |
| Spec core types (descriptor, envelope, intent, receipt, capabilities, adapter trait, harness bridge) | **Not started** — `src/harness/mod.rs` is empty, `src/channel/mod.rs` is a re-export shim |
| Error taxonomy | **Not started** — `TinyChannelsError` is one opaque string |
| Chunking / length units | **Not started** — no chunker exists anywhere |
| Relay contract | **Not started** |
| `tests/` integration dir | Empty |

openhuman-4 does **not** depend on this crate yet; every ported file is a
duplicated copy that will drift (see the openhuman-4 plan).

## Known Bugs and Debt (fix during the phases below)

Carried-over logic bugs (present in both repos unless noted):

1. **Telegram forum topics collapse into one session.**
   `conversation_history_key` drops `thread_ts` for `channel == "telegram"`
   (`src/context.rs:74-86`; source `openhuman-4
   src/openhuman/channels/context.rs:76-90`). The carve-out conflates
   reply-to message ids with forum topic ids. Fix by distinguishing a
   `topic_id` from a `reply_to_id` in the new envelope, and keying sessions on
   the topic.
2. **No workspace/guild/tenant discriminator in session keys.** Keys are
   `{channel}_{sender}_{reply_target}` (`src/context.rs:74-86`; openhuman-4
   `bus.rs:1005-1042`). Slack channel ids are only workspace-unique; a
   multi-workspace bot collides. Neither upstream solves this in the key
   either (Hermes `scope_id` is relay-routing only) — adding `scope_id` to
   the session discriminator is a deliberate TinyChannels improvement.
3. **`conversation_memory_key` uses `msg.id`** (`src/context.rs:70-72`), so
   every message yields a distinct "conversation" key. openhuman-4 has tests
   asserting this (`tests/memory.rs:conversation_memory_key_uses_message_id`),
   so it may be intentional per-turn keying — confirm intent with the
   OpenHuman side before changing, then either rename it (`turn_memory_key`)
   or fix it to coalesce per conversation.
4. **Chunking counts `char`s, not UTF-16 code units.** openhuman-4 Telegram
   (`providers/telegram/text.rs:4-50`, limit 4096) and Discord
   (`providers/discord/channel.rs:111-154`, limit 2000) both use
   `.chars().count()`; both platforms count UTF-16 units, so astral-plane
   content can exceed the API limit. The portable chunker in Phase 2 fixes
   this once for every provider.
5. **No outbound idempotency.** `SendMessage` has no idempotency key; a
   retried send after a transport error double-posts. Phase 1's
   `ChannelOutboundIntent.idempotency_key` addresses this.
6. **Dead code in `src/config.rs`:** `SecurityConfig` / `SandboxConfig` /
   `AuditConfig` / `ResourceLimitsConfig` / `SandboxBackend`
   (`src/config.rs:367-443`) are sandbox scope creep referenced nowhere —
   remove them (and their tests) or move them to the crate that owns
   sandboxing. `YuanbaoConfig::{apply_env_defaults, validate}` and
   `strip_yuanbao_version_prefix` (`src/config.rs:532-590`) are never called —
   wire `validate` into the connect flow or drop them.
   `YuanbaoConfig::max_message_length` is declared but unused until Phase 2.
7. **Config/definition asymmetry.** 16 config structs vs. 8
   `all_channel_definitions()` entries; `web` has a definition but no config
   struct. Reconcile or document which channels are UI-connectable.
8. **`has_listening_integrations` omits `webhook`** (`src/config.rs:49-65`).
   Decide (webhook is push-based, so likely correct) and document it in a
   comment plus a test either way.
9. **`WhatsAppConfig::backend_type` reports `"cloud"` when unconfigured**
   (`src/config.rs:293-301`). Return an explicit unconfigured state.
10. **`ChannelBackend` returns untyped `serde_json::Value`** for
    reaction/thread/discord lookups (`src/backend.rs:57-120`), and
    `ChannelManager` has no wrappers for reaction/thread/managed-link/discord
    ops. Type the returns in Phase 1 and complete the wrappers.
11. **Hygiene:** `src/lib.rs:5` still says "intentionally blank scaffolding";
    `thiserror` and `tracing` are declared but unused (adopt `thiserror` for
    the Phase 1 error enum, start instrumenting sends with `tracing` or drop
    it); `src/context.rs` vs `src/runtime.rs` naming is inverted (the runtime
    helpers live in `context.rs`) — rename during Phase 1 while the public
    API surface is still small.

## Phased Plan

Each phase should land as its own PR with tests; phases 1–3 are the critical
path for OpenHuman integration.

### Phase 0 — Hygiene (small, do first)

- Fix items 6–9 and 11 above.
- Add `JsonSchema` derives to `ChannelDefinition` / `AuthModeSpec` / response
  types if the OpenHuman UI consumes generated schemas (confirm; `config.rs`
  types already derive it).
- Keep `cargo clippy` clean; add CI wiring if absent.

### Phase 1 — Core types (`src/channel/`)

Create the spec's modules with these verified upstream shapes:

- `channel/capabilities.rs`: three separate surfaces, mirroring OpenClaw —
  static feature flags, presentation limits (including
  `LengthUnit { Characters, Utf8Bytes, Utf16Units }` and
  `MarkdownDialect { Plain, Markdown, Html, SlackMrkdwn, DiscordMarkdown,
  TelegramMarkdownV2 }`), and message-action names. Plus the durable-final
  capability map (13 keys from OpenClaw `message/types.ts:17-31`).
- `channel/types.rs`: `ChannelDescriptor` (id as open string, catalog-driven,
  not a closed enum), `ChannelRef`, `ConversationRef` (kind, id, `scope_id`,
  `parent_id`, `thread_id`, `topic_id`), `SenderRef` (id, alt ids, name,
  `is_bot`, roles), `SecretRef`.
- `channel/envelope.rs`: `ChannelInboundEnvelope` per the spec, with
  `AccessContext` carrying explicit facts (dm decision `allow | pairing |
  deny`, group policy `open | allowlist | disabled`, mention gating with
  implicit-mention kinds, `command_authorized` default-deny,
  `delivered_via_upstream_relay` **never serialized** — mirror Hermes'
  forgery resistance). Media references index-aligned with kind
  `image | video | audio | document | unknown`, transcribed indexes, and
  local-cache-path semantics.
- `channel/receipt.rs`: mirror OpenClaw `MessageReceipt` /
  `MessageReceiptPart` (part kinds, `edit_token` / `delete_token`, `sent_at`).
- `channel/error.rs` (replaces the string in `src/error.rs`, using
  `thiserror`): `SendErrorKind { TooLong, BadFormat, Forbidden, NotFound,
  RateLimited, Transient, Unknown }` + `retryable`, `retry_after`,
  `chat_level_not_found`, and `continuation_message_ids` /
  `partial_overflow` data from Hermes. Encode the idempotency rule: timeouts
  are NOT retryable.
- `channel/intent.rs`: `ChannelOutboundIntent` with `idempotency_key`,
  `DeliveryDurability { Required, BestEffort, Disabled }`, payload variants
  (text, media, voice, files, poll, presentation blocks, native channel
  data).
- Session keys (`channel/session.rs`, replacing the helpers in
  `src/context.rs`): key = `{namespace}:{channel}:{account}:{chat_type}:
  {scope_id?}:{conversation}:{thread?}:{participant?}` with the two Hermes
  policy toggles (`group_sessions_per_user` default true,
  `thread_sessions_per_user` default false → thread participants share a
  session) and a legacy-key canonicalization helper so existing OpenHuman
  session state survives migration. Fix bugs 1–3 here.
- Type the `ChannelBackend` returns (bug 10) and finish `ChannelManager`
  wrappers.

### Phase 2 — Text engine (`src/text/`)

- Chunker with pluggable `LengthUnit` measurement; UTF-16 budget mapping via
  surrogate-safe prefix (Hermes `utf16_len` / `_prefix_within_utf16_limit`).
- Split-point preference newline > space; triple-backtick fence preservation
  across chunks (close and reopen with carried language tag); inline-code
  span avoidance; reserved room for `(i/N)` continuation indicators.
- Chunk modes from OpenClaw: `None` (single send), `Length`, `Newline`
  (block-aware markdown chunking, then per-block re-chunk), and per-channel
  `text_chunk_limit` override resolution.
- `truncate_with_ellipsis` moves here from `src/context.rs`.

### Phase 3 — Adapter trait + harness bridge (`src/channel/adapter.rs`, `src/harness/`)

- Base `ChannelAdapter` trait per the spec (descriptor/start/stop/send/status)
  with optional extension traits (setup, directory, resolver, typing,
  reaction, edit/delete, streaming draft). Adapter status uses a
  `ChannelAccountSnapshot`-style state machine (`linked | not_linked |
  configured | not_configured | enabled | disabled`, reconnect counters,
  last-disconnect record) — extend the existing controller types rather than
  duplicating them.
- Receive-ack policy enum (`after_receive_record | after_agent_dispatch |
  after_durable_send | manual`) on the adapter contract.
- `harness/types.rs` + `harness/bridge.rs`: `ChannelTurn`,
  `ChannelOutputEvent` (text delta, final message, tool/progress, approval
  request, clarification, media, cancellation, lifecycle), and the
  capability-driven translation to outbound intents. Streaming behavior:
  draft edit-in-place with throttle + flood backoff, `supports_edit=false`
  degrades to segment sends (promote Hermes' `"__no_edit__"` sentinel to a
  capability), interactive approvals degrade from native buttons to text
  commands.
- Turn admission verdicts (`dispatch | observe_only | handled | drop`) and the
  ordered inbound lifecycle (ingest → classify → preflight → resolve →
  authorize → assemble → record → dispatch → finalize) as the bridge's
  processing model.

### Phase 4 — Durable delivery queue (`src/delivery/`)

- Write-ahead state machine behind a `DeliveryQueueStore` trait (host owns
  storage): enqueue before send; ack on success/abort; fail on throw/partial.
- Retry policy as fixtures-backed constants: `MAX_RETRIES = 5`, backoff
  `[5s, 25s, 2m, 10m]` (last repeats), permanent-error classifier (OpenClaw's
  11-pattern list) short-circuiting to a failed store.
- Unknown-send reconciliation tri-state (`Sent { receipt } | NotSent |
  Unresolved { retryable }`) for crash recovery without double-sends.
- Durability negotiation: derive required durable-final capabilities from the
  payload shape; degrade to best-effort when the adapter doesn't advertise
  them.

### Phase 5 — Generic adapters + relay contract

- Local/API/webhook adapter first (unblocks OpenHuman internal surfaces).
- Relay: port `CapabilityDescriptor` (contract_version 1, frozen per
  connection, unknown-keys-ignored/additive-only, `max_message_length == 0 →
  4096`) and the two HMAC-SHA256 auth schemes (WS-upgrade bearer token
  `base64url("{gateway_id}:{exp}:{sig}")` TTL 300s; per-tenant inbound
  signature over `"{timestamp}.{body}"` exact bytes, 300s replay window,
  multi-secret rotation, constant-time compare). **Wire bytes must match the
  TypeScript connector** — port Hermes `tests/gateway/relay/test_auth.py` and
  `test_descriptor.py` as byte-exact fixtures before writing the transport.
- Relay transport (frames: `descriptor, inbound(+bufferId ack), going_idle_ack,
  outbound_result, interrupt_inbound, passthrough_forward` / `outbound,
  chat_info, interrupt, going_idle, inbound_ack`) behind a feature flag;
  upstream marks the contract EXPERIMENTAL. Delegated authorization: honor
  `delivered_via_upstream_relay` only when stamped by the authenticated
  transport, keyed off the flag, never the platform value.

### Phase 6 — Provider adapters

Follow the spec's priority order (Discord/Telegram/Slack first). Providers
move from openhuman-4 only after their app dependencies reduce to the traits
above — see the ladder in the openhuman-4 plan (self-contained providers
first: email, irc, yuanbao, imessage, mattermost, qq, dingtalk; then
proxy-client-only: discord, slack, whatsapp, lark, signal; telegram last, it
needs approval/voice/pairing/memory traits).

## Test Migration and New Fixtures

### Migrate from openhuman-4 (surfaces already ported)

- `channels/controllers/schemas_tests.rs` (33 tests) — not yet mirrored here.
- The type-shape assertions from `channels/controllers/ops_tests.rs` (47) —
  run them against a mock `ChannelBackend`; leave the REST-wiring assertions
  in openhuman-4.
- Already mirrored (verify parity, then let openhuman-4 delete its copies once
  it depends on the crate): definitions, config schema, traits, context
  helpers, `compute_max_in_flight_messages`, the `tests/memory.rs` and
  `tests/runtime_dispatch.rs` key tests.

### New fixtures from upstream (as `tests/` integration suites)

From OpenClaw (paths under `/tmp/tinychannels-openclaw-src`):

1. `src/channels/message/receipt.test.ts` → receipt-with-parts normalization.
2. `src/infra/outbound/delivery-queue.recovery.test.ts` (+ `.policy`,
   `.reconnect-drain`, `.storage`) → retry/backoff/permanent-error state
   machine.
3. `src/channels/inbound-event/media.test.ts` → media index alignment +
   transcribed indexes.
4. `src/routing/session-key.test.ts` (+ `.continuity`) and
   `src/channels/plugins/session-conversation.test.ts` → session-key grammar,
   Telegram topics, legacy-key canonicalization.
5. `src/channels/plugins/message-capability-matrix.test.ts` → capability
   degradation by config presence.
6. `src/channels/plugins/{mention-gating,allowlist-match,allow-from,
   command-gating}.test.ts` → security gating decision tables.
7. `src/channels/message/capabilities.test.ts` + `contracts.test.ts` →
   durable-final requirement derivation.

From Hermes (paths under `/tmp/tinychannels-hermes-agent-src/tests/gateway`):

1. `test_telegram_text_batching.py`, `test_text_batching.py` → UTF-16
   chunking, fence preservation, `(i/N)` indicators.
2. `test_session.py`, `test_base_topic_sessions.py`, `test_dm_topics.py` →
   DM/group/thread key derivation and the shared-thread policy toggle.
3. `test_allowlist_startup_check.py`, mention/group gating suites, and
   `test_relay_upstream_authz.py` → allowlist wildcard/pairing/internal
   bypass and delegated-trust strictness.
4. `test_send_retry.py`, `test_dead_targets.py` → retryable-vs-timeout
   classification, chat-level vs subchat not_found.
5. `relay/test_auth.py`, `relay/test_descriptor.py`,
   `relay/test_descriptor_from_entry.py` → byte-exact HMAC and descriptor
   round-trip (Phase 5 gate).
6. `test_update_streaming.py` → progressive edit cadence, flood backoff,
   no-edit fallback.
7. `test_contract_doc_conformance.py` → replicate the pattern: a test that
   fails when public API names drift from the docs.

### Fixture acceptance gates per phase

- Phase 1 merges only with the session-key and receipt fixture suites green.
- Phase 2 merges only with the UTF-16/fence/indicator chunking suite green,
  including a case where a 4095-`char` emoji string exceeds 4096 UTF-16 units.
- Phase 4 merges only with the backoff/permanent-error/reconciliation state
  machine suite green.
- Phase 5 merges only with byte-exact auth vectors shared with the TS
  connector.

## Non-goals (unchanged from the spec)

Provider SDK internals, OpenHuman UI widgets, model/agent execution,
credential storage implementations, and provider daemons stay out of this
crate. The openhuman-4 `runtime/` dispatch engine and `web` provider are
consumers of this crate, not porting targets — moving them would drag the
whole app in and create a dependency cycle.
