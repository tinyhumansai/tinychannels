# TinyChannels Specification

> **Current contract:** [Channel Contract v2](contract-v2.md) is the normative
> target for content, events, addressing, receipts, capabilities, and migration.
> The OpenClaw/Hermes audit and execution plan below are historical research and
> implementation context; follow v2 when their proposals conflict.

TinyChannels provides pluggable channel and messaging primitives used by
OpenHuman to communicate between channel surfaces and harness runtimes.

## Goals

- keep channel messaging transport-neutral
- keep harness communication contracts typed and testable
- make routing, lifecycle, and observability metadata explicit
- avoid coupling OpenHuman application code to one channel provider

## Module Boundaries

- `traits`: channel-side abstractions, message ingress, egress, typing, draft
  updates, and lifecycle health.
- `config`: serializable channel configuration migrated from OpenHuman.
- `controllers`: static channel definitions, auth-mode metadata, and shared
  controller response types.
- `backend`: `ChannelBackend` and `ChannelManager`, the pluggable boundary back
  to OpenHuman's backend/config/JWT/runtime implementation.
- `context`, `routes`, `runtime`: portable helpers for channel memory keys,
  model route commands, and listener sizing.

Provider wire implementations that still depend deeply on OpenHuman runtime
state remain outside this crate until they can be split behind transport traits.

## Research and Porting Specs

- [TinyChannels client](tinychannels-client.md) — host construction, attested
  runtime leases, and the native test proof boundaries.
- [Storage needs](storage-needs.md) — the bus contract's declared collection
  inventory for delivery, remote sessions, and WhatsApp Web persistence.
- [Channel Contract v2](contract-v2.md) — normative target and v21-to-v22
  compatibility plan.
- [Harness progress output](progress-output.md) — canonical progress events,
  module operations, delivery results, and the TinyAgents conformance map.
- [Web adapter](web-adapter.md) — the in-app chat as a channel: inbound
  normalization, `WebChannelEvent` projection, and the host event sink.
- [OpenClaw and Hermes channel porting](openclaw-hermes-channel-porting.md) —
  historical source-level audit of both upstreams (verified against pinned
  2026-07-04 checkouts).
- [TinyChannels execution plan](tinychannels-execution-plan.md) — the phased
  historical implementation plan and test-migration/fixture catalog.

## Compiled provider supervision

`StartChannelSupervised(name, config, proxy, policy)` starts a configured provider
with module-owned reconnects. Host policy selects initial and maximum backoff;
the module applies exponential delay and bounded jitter, reports sanitized
`restarting`/`connecting` status, and forwards inbound traffic in order. Delays
greater than one day are rejected before constructing a provider. Configuration
and proxy credentials require a confidential call.

`StopChannel` cancels and joins the listener even during a reconnect delay,
drains accepted sends and inbound callbacks, and runs native shutdown hooks.
Legacy `StartChannel` and `StartChannelConfigured` preserve their original
arguments and one-listener behavior. The new member is advertised in contract
revision 10 and module operation version 1.11. Webhook-only providers still
require the separately pending ingress surface.


### Identity-scoped runtime leases

Hosts reserve an opaque runtime before installing its immutable callback
adapter, activate the runtime, and use the existing Channels operations at the
returned private object path. Provider names are local to that runtime.
`StopRuntime` cancels and joins pending activation, drains providers, relay,
delivery and callbacks, then removes only that runtime's object. Sign-out stops
the old handle without shutting down the cached process-wide module.

Reservations are inert, limited to sixteen, and expire after sixty seconds
unless activated. Activation and cleanup remain owned when a bus reply waiter
is canceled. All callback methods retain their existing arities; each runtime
routes them to its reserved host object path, including durable-delivery and
controller callbacks. Hosts must bind that object to the original identity and
keep it registered until cleanup completes, rather than resolving the current
signed-in user's credentials in a callback.

`ReserveRuntime`, `StartRuntime` and `StopRuntime` are confidential additive
operations. Original singleton operations remain available for older callers;
runtime objects cannot allocate nested runtimes. Core shutdown drains both
legacy singleton resources and every identity-scoped runtime.

Runtime object publication consumes TinyBus 0.1.3's upstream dispatch fix:
TinyBus snapshots the target and drops the object-tree lock before invoking
module code. The module retains ABI v1 compatibility with older hosts.
