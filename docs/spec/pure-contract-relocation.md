# Pure contract relocation

This slice makes `tinychannels-bus` contain vocabulary, schema declarations,
serialization and errors. Runtime/provider seams and algorithms belong to
`tinychannels-runtime`; concrete providers and relay/delivery drivers remain
in `tinychannels`. The dependency direction is root implementation → runtime
→ bus. The compiled module uses those owning implementations.

## Compatibility

Contract DTO definitions stay in the bus crate and are re-exported, never
copied. No Serde field/default/tag, Base64 encoding, persisted session/history
key, idempotency digest, provider behavior, or existing module method arity
changes. Base64 remains a serialization dependency for `bodyB64`; its legacy
malformed-input handling is preserved. Golden wire/digest fixtures and moved
behavior suites pin these properties.

Root implementation paths for channel/provider traits, security, context,
relay auth/I/O, adapters, segmentation and controller configuration helpers
remain available. Code importing algorithms or async traits directly from
`tinychannels_bus` must use `tinychannels_runtime` (or the legacy root facade).
A host migration must instead use the compiled module; these compatibility
exports do not authorize linking implementations into hosts.

Algorithms cannot remain inherent methods on bus-owned DTOs without making
the bus execute them. The following extension traits preserve method syntax
when brought into scope:

| DTO | Import from the runtime | Moved methods |
| --- | --- | --- |
| `SendMessage` | `traits::SendMessageExt` | `with_deterministic_idempotency_key` |
| `ChannelSendError` | `channel::ChannelSendErrorExt` | `new` |
| `InboundMediaPayload` | `channel::InboundMediaPayloadExt` | `from_media` |
| `ConnectorToGatewayFrame` | `relay::ConnectorToGatewayFrameExt` | `inbound_ack`, `authenticated_inbound_event` |
| `ChannelsConfig` | `config::ChannelsConfigExt` | `has_listening_integrations` |
| `RelayRuntimeConfig` | `config::RelayRuntimeConfigExt` | `is_listener_configured`, `relay_identities` |
| `WhatsAppConfig` | `config::WhatsAppConfigExt` | `backend_type`, `is_cloud_config`, `is_web_config` |
| `YuanbaoConfig` | `config::YuanbaoConfigExt` | `apply_env_defaults`, `validate` |
| `ChannelDefinition` | `controllers::ChannelDefinitionExt` | `validate_credentials` |

The root facade exposes the same trait modules. Ordinary DTO constructors,
field-setting builders, defaults, enum wire spellings, schema lookup and JSON
serialization helpers remain in the bus. This is a necessary Rust source API
break for callers that previously used moved methods without a trait import;
it does not change the wire contract version or released module interface.

## Verification and remaining work

The purity checker generates separate probe workspaces under `target/` and
traverses the complete normal/build dependency closure without a platform
filter, once for defaults and once for every declared bus feature. Dev edges
are excluded and missing graph metadata fails closed. The bus owns async
provider interfaces, Tokio channel types, in-memory context helpers, and static
TSD2 collection declarations through `tinystoragedrivers-types`; it does not
own provider execution, sockets, tasks, storage clients, database clients or
drivers. The source guard checks those runtime seams independently of the
dependency graph, and CI runs both the guard and its negative controls.

The module follow-up adds a bounded relay lifecycle (`ReserveRelay`,
confidential `StartRelay`, `StopRelay`, safe `RelayStatus`, and
`SendRelayIntent`), `Shutdown`, typed provider `SendChannelIntent`, send-only
confidential `SendEmail`, and an ordered host `PersistAllowedIdentity`
callback. Relay/provider/email actors own accepted work across caller
cancellation and join it during explicit cleanup. Pairing and delivery policy
remain host responsibilities. For `Required` channel sends, the module writes
a bounded intent and a send-attempt crash marker through the host's scoped
storage callbacks before provider I/O; it records a terminal fingerprint
before acknowledging success. Recovery acknowledges a matching terminal
result, rejects a conflicting fingerprint, and moves an unresolved send to
failed storage rather than blindly resending it. The host owns persistence and
its user scope; the module validates record limits and implements queue
recovery. Best-effort and disabled intents continue through the provider.
Local websocket, callback, SMTP/MIME, durable-recovery, and loaded-cdylib
fixtures exercise those contracts. Host adapters, release digests, downstream
gitlinks and the switch to contract-only host dependencies remain gated on
compatible published native artifacts. A locally built module is not a
released artifact.
