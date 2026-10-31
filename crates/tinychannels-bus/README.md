# tinychannels-bus

Serialized channel vocabulary, configuration/schema declarations, interface
names, relay frames and structured errors. It also declares the async provider
trait and host-facing call shapes without implementing a provider or executing
host callbacks. `context` contains shared DTOs and pure per-sender history
helpers over an in-memory `Arc<Mutex<...>>`. The optional `storage` feature
exposes serde-only collection declarations through `storage::collections()`.
The storage implementation adapts these DTOs to TinyStorageDrivers types at
the point where it provisions collections.

The default normal/build closure contains Serde/JSON/schema/error derivation,
Base64 for the existing
`PassthroughForward.body` custom serializer (`bodyB64`), `anyhow`, `async-trait`,
and Tokio's `sync` channel type. The optional `storage` feature adds only
`tinystoragedrivers-types` for collection declarations. It contains no TinyBus,
storage client/runtime or backend driver, HTTP/socket transport, database,
task executor, scheduler, crypto/entropy implementation, or concrete provider.

Storage collection declarations and serialized record DTOs remain contract
data here. The `tinychannels` implementation owns owner-local migrations,
legacy decoding, and conversions between wire records and stored DTOs.

DTOs are defined once here. `tinychannels-runtime` and `tinychannels` re-export
these same Rust types. Hosts should name wire values through this crate and
execute behavior through the compiled module. The root library compatibility
facade is for existing implementation consumers, not a host dependency shortcut.

`python3 scripts/check-bus-purity.py --offline` audits independent default and
all-feature normal/build closures. It rejects storage clients, runtime buses,
and engine drivers from this contract boundary. Its
source guard allows async trait signatures, Tokio sync carriers, in-memory
history locking, and static metadata while rejecting concrete I/O, task,
database, and provider execution. The CI feature-test lane runs this gate and
its Python negative controls. Crypto and UUID generation remain implementation
dependencies.

The original module and host callback names, method arities, JSON field names,
optional fields and persisted configuration values are unchanged. See the
[relocation specification](../../docs/spec/pure-contract-relocation.md) for
necessary Rust source API migrations and subsequent module-operation slices.

`ChannelInteraction(request)` adds instance capability discovery, typing start/stop,
and draft creation, updates and finalization. It is a confidential call. The
module serializes these operations with ordinary sends in its bounded provider
queue; stopping a provider joins admitted work and rejects subsequent calls.
Contract version 6 adds the member without changing existing method arity.

Contract version 16 adds confidential `StartRuntimeWithDispatch(handle,
ChannelRuntimeDispatchPolicy)`. The listener count selects the existing module
admission rule (four turns per listener, clamped to 8–64). This opt-in runtime
calls its immutable host object's `ProcessInbound(channel, message)` for each
turn with bounded concurrency. The host supplies only its turn execution and
product policy; the compiled module owns the queue, admission, completion
frontier, failures, and drain. Like the existing `DeliverInbound` host callback,
`ProcessInbound` is a normal callback to the host process, not a confidential
call to an attested module recipient.

Existing `StartRuntime` and ordered `DeliverInbound` behavior remain unchanged.
Dispatch mode and effective limit cannot change after activation. Flush waits
for preceding accepted turns; stop freezes admission and retains pending work
across canceled waiters. The existing module cleanup deadline reports pending
cleanup after ten seconds while retaining the owner for a later stop retry.
Contract version 22 carries `ChannelInboundEnvelope` content parts through
`DeliverInbound` and `ProcessInbound`, and sends `ChannelOutboundMessage` values
with ordered typed parts through `SendChannelIntent`. The legacy `ChannelMessage`
and `SendMessage` conversions are deprecated, one-release compatibility
projections. Their outbound adapter accepts text parts only; rich payloads
remain available to providers that implement `Channel::send_typed`. Binding is
exact-version: a v21 host cannot bind a v22 module, so this shim is for source
migration and same-version legacy provider adapters, not cross-version callback
compatibility.

`ChannelsHost` exposes scoped voice callbacks. `TranscribeVoice`,
`SynthesizeSpeech`, `StoreVoiceMedia`, `ReadVoiceMedia`, and
`GetVoiceReplyPolicy` carry the
configured channel, provider account, conversation, and originating turn in a
`HostVoiceScope`. Hosts own transcription, synthesis, durable voice media, and
the authoritative reply mode. Outbound speech remains an explicit typed
`OutboundVoice` intent, so synthesis is never inferred from text alone.

This source contract still requires a compatible published native module before
a downstream host can claim released composition.

Provider notifications use `ChannelEvent`, a versioned DTO with a `type`
discriminator, stable event/source IDs, source time, `ChannelRef`, and the full
optional `ConversationRef`. The module sends it through `ChannelsHost.DeliverEvent`
in the same ordered callback queue as inbound messages. `PublishEvent` remains
available for older, unrelated domain publications.

Providers that support v22 ordered ingress submit `ChannelIngress` items through
their `EventSink`: normalized inbound envelopes and typed events share one
awaited path. Telegram uses this path for messages, edits and reactions, so a
later edit/reaction cannot overtake its message. The legacy message-channel
bridge remains for providers still migrating. Event message references retain
the stable module message ID while `providerMessageId` preserves the source ID;
Telegram topic IDs remain separate from chat IDs in `ConversationRef`.

Typed send outcomes may carry multiple provider message IDs and per-part edit
or delete tokens. Tokens are opaque operation inputs and are only populated
when the provider's actual response returns the corresponding message IDs.
Required delivery first persists the unknown-send state, then attaches any
partial receipt in a follow-up save; both writes preserve the pre-I/O crash
marker. Terminal receipts are included in idempotent replays.

The controller vocabulary includes complete Discord guild, text-channel and
permission DTOs. Provider APIs re-export these types at their existing paths;
REST calls and permission calculations remain implementation behavior.
`controllers::requests` defines the typed camel-case parameters accepted by
the public channel controller methods. `ReplyContext` carries the reply target
and optional thread address across approval surfaces, and `MODULE_ID` is the
stable "tinychannels" module-registry identity.

The deployed OpenHuman web chat vocabulary is available from
`tinychannels_bus::web`. It is a distinct snake-case `event`-tagged protocol,
separate from provider `ChannelEvent`'s `type` tag and the progressive output
event's `type` tag. The web DTOs retain optional-field omission and accept the
historical `chat_error` cancellation shape alongside `chat_cancelled`.
Tenant/profile stamps stay in the host. Ordered message `parts` use
`OutboundContentPart`, including typed media and voice notes; `full_response`
remains a compatibility projection. Socket event aliases are transport names
and never rewrite the payload discriminator. Alias generation suppresses
parent and subagent stream deltas, matching the deployed socket server.

Regenerate the checked-in JSON Schema for TypeScript generation with
`cargo run -p tinychannels-bus --example web_schema > crates/tinychannels-bus/schema/web-channel-event.schema.json`;
the public `web_event_schema_json()` function exposes the same schema to
codegen tools.
The transport-free helpers also carry the web payload cap and timing
calculations for migration into host adapters. Logging and clock acquisition
remain host responsibilities. This contract does not claim that OpenHuman's
current host, TUI, or frontend has migrated to these types; those integrations
belong to their dependent epic leaves.

`src/fixtures/web/` contains hand-authored historical compatibility fixtures,
not captured runtime transcripts. Their shapes were derived from the deployed
host at OpenHuman base `ba83482245`, including constructors in
`crates/openhuman-core/src/channels/web_chat/presentation.rs`,
`crates/openhuman-core/src/channels/web_chat/ops.rs`,
`crates/openhuman-core/src/channels/proactive.rs`,
`crates/openhuman-core/src/flows/ops/streaming.rs`,
`crates/openhuman-core/src/cron/scheduler/`,
`crates/openhuman-core/src/agent/orchestration/background_delivery.rs`,
`crates/openhuman-core/src/agent/tinyagents/host/progress_sink.rs`,
`crates/openhuman-core/src/channels/host/adapters.rs`,
`crates/openhuman-core/src/channels/providers/relay/channel.rs`,
and the RPC Socket.IO adapter. These fixtures pin serialized field presence,
omission, nulls, and nested payloads against that source audit. Some valid
representable states are not emitted by the current constructors (for example,
an empty citations list); those are compatibility-shape fixtures, not claims
of observed runtime output. `historical_events.json` includes the per-variant
field shapes for the remaining deployed event names plus the two legacy names
still accepted by clients.
