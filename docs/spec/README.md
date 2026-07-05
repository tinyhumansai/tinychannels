# TinyChannels Specification

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

- [OpenClaw and Hermes channel porting](openclaw-hermes-channel-porting.md) —
  source-level audit of both upstreams (verified against pinned 2026-07-04
  checkouts) and the proposed TinyChannels contract.
- [TinyChannels execution plan](tinychannels-execution-plan.md) — the phased
  implementation plan: current-state gap matrix, known bugs, module plan,
  and test-migration/fixture catalog.
