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

## Current State (audited 2026-07-04; Phases 0-3 local slices landed)

The crate compiles with zero warnings (`cargo build --all-targets`, clippy
clean) and passes 87 unit tests. Phase 0 hygiene has landed: sandbox-only
config types were removed from this crate, webhook listener behavior is
documented and tested, WhatsApp exposes an explicit unconfigured backend state,
Yuanbao connect credentials are normalized through `YuanbaoConfig`, controller
metadata/response types derive `JsonSchema`, and the stale scaffold docs were
replaced. Phase 1's local TinyChannels slice has also landed: core
`src/channel/` descriptor/envelope/intent/receipt/capability/session/error
types exist, OpenClaw receipt/action fixtures and Hermes send-error/session
rules are covered by unit tests, backend returns are typed, and manager wrappers
cover reaction/thread/managed-link/Discord/default-channel operations. Phase 2's
portable text engine has landed in `src/text/`, including UTF-16 measurement and
surrogate-safe prefixing, markdown fence close/reopen chunking, inline-code
split avoidance, newline/space split preference, continuation indicators,
newline/paragraph mode, chunk-limit resolution, and the moved
`truncate_with_ellipsis` helper. Phase 3's adapter and harness bridge contracts
have landed: `ChannelAdapter`, optional extension traits, receive-ack policy,
account status snapshots, `ChannelTurn`, `ChannelOutputEvent`, and
capability-driven output-to-intent translation are in place. OpenHuman does not
yet depend on these types, so cross-repo integration remains pending:

| Surface | Status |
| --- | --- |
| `Channel` / `ChannelMessage` / `SendMessage` (`src/traits.rs`) | Ported, legacy shape |
| Provider config structs (`src/config.rs`, 16 providers) | Ported |
| Static definitions + auth modes (`src/controllers/definitions.rs`, 8 channels) | Ported |
| Controller response types (`src/controllers/types.rs`) | Ported |
| `ChannelBackend` + `ChannelManager` (`src/backend.rs`) | New seam, matches porting.md |
| Runtime helpers (`src/context.rs`, `src/routes.rs`, `src/runtime.rs`) | Ported |
| Spec core types (descriptor, envelope, intent, receipt, capabilities, adapter trait, harness bridge) | Phases 1 and 3 landed locally |
| Error taxonomy | Phase 1 send taxonomy landed and `TinyChannelsError` wraps structured send errors |
| Chunking / length units | Phase 2 text engine landed in `src/text/` with UTF-16/fence/indicator tests |
| Adapter / harness bridge | Phase 3 landed in `src/channel/adapter.rs` and `src/harness/` |
| Relay contract | **Not started** |
| `tests/` integration dir | Empty |

openhuman-4 does **not** depend on this crate yet; every ported file is a
duplicated copy that will drift (see the openhuman-4 plan).

## Known Bugs and Debt (fix during the phases below)

Carried-over logic bugs (present in both repos unless noted):

1. **Resolved locally in Phase 1:** Telegram forum topics no longer collapse in
   `conversation_history_key`; the legacy helper includes `thread_ts` for
   Telegram. The new `ChannelInboundEnvelope` also separates `topic_id` from
   `thread_id` / reply metadata for the OpenHuman migration.
2. **Resolved in Phase 1 core types:** new session keys include `scope_id` as a
   deliberate TinyChannels discriminator. OpenHuman still needs to thread scope
   facts into envelopes during integration.
3. **`conversation_memory_key` uses `msg.id`** (`src/context.rs:70-72`), so
   every message yields a distinct "conversation" key. openhuman-4 has tests
   asserting this (`tests/memory.rs:conversation_memory_key_uses_message_id`),
   so it may be intentional per-turn keying — confirm intent with the
   OpenHuman side before changing, then either rename it (`turn_memory_key`)
   or fix it to coalesce per conversation.
4. **Resolved locally in Phase 2:** portable chunking now supports UTF-16 code
   units, including the emoji case where character count passes but UTF-16
   length exceeds the platform limit. OpenHuman still needs to adopt this
   chunker for Telegram/Discord in Step 3.
5. **No outbound idempotency.** `SendMessage` has no idempotency key; a
   retried send after a transport error double-posts. Phase 1's
   `ChannelOutboundIntent.idempotency_key` addresses this.
6. **Resolved in Phase 0:** dead sandbox-only config (`SecurityConfig` /
   `SandboxConfig` / `AuditConfig` / `ResourceLimitsConfig` /
   `SandboxBackend`) was removed from the crate. `YuanbaoConfig` defaults,
   validation, and `strip_yuanbao_version_prefix` are now wired through the
   manager connect path before delegating to the backend.
   `YuanbaoConfig::max_message_length` remains declared for Phase 2 chunking.
7. **Documented in Phase 0:** config/definition asymmetry is intentional for
   now. `all_channel_definitions()` is the UI-connectable registry; `web` is
   app-owned and has no config struct, while several config-backed providers
   remain hidden until their setup flows are promoted.
8. **Resolved in Phase 0:** `has_listening_integrations` intentionally omits
   `webhook` because webhook delivery is push-based and host-server-owned;
   this is documented and tested.
9. **Resolved in Phase 0:** `WhatsAppConfig::backend_type` now reports
   `"unconfigured"` when neither Cloud API nor Web session settings exist.
10. **Resolved in Phase 1:** `ChannelBackend` now returns typed send,
    reaction, thread, and Discord lookup results, and `ChannelManager` wraps
    reaction/thread/managed-link/Discord/default-channel operations.
11. **Partially resolved in Phase 0:** the stale scaffold crate docs were
    replaced, `TinyChannelsError` now derives `thiserror::Error`, and manager
    sends are instrumented with `tracing` while skipping message payloads.
    `src/context.rs` vs `src/runtime.rs` naming is still inverted (the runtime
    helpers live in `context.rs`) — rename during Phase 1 while the public API
    surface is still small.

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
