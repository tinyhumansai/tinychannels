# TinyChannels Specification

TinyChannels is reserved for pluggable channel and messaging primitives used by
OpenHuman to communicate between channel surfaces and harness runtimes.

## Goals

- keep channel messaging transport-neutral
- keep harness communication contracts typed and testable
- make routing, lifecycle, and observability metadata explicit
- avoid coupling OpenHuman application code to one channel provider

## Initial Module Boundaries

- `channel`: channel-side abstractions, message ingress, egress, and lifecycle
  metadata.
- `harness`: communication boundary between channel events and harness runtime
  execution.

This document is intentionally high-level until concrete OpenHuman integration
requirements are added.
