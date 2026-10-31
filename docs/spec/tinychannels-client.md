# TinyChannels Client

`tinychannels-client` is the host-facing typed layer over TinyBus. It wraps a
host-supplied `ModuleClient`; it does not own module discovery, release policy,
credentials, retries, callback registration or the `ModuleRuntime` lifecycle.
Top-level controller and health operations use the confidential route when
their arguments include configuration, credentials or proxy policy.

## Identity-scoped runtime lifecycle

The host creates a `TinyChannelsClient`, obtains a `ChannelRuntimeDriver`, and
passes that driver to `ModuleRuntime::lease`. The lease transaction reserves a
module-generated identity, validates its opaque handle and exact private
object paths, registers the host callback at the returned callback path, then
starts the runtime. The lease exposes a `ChannelRuntimeClient` whose methods
stay bound to the same attested peer as reservation and lifecycle calls.

The callback factory receives the reservation and object, so the host can bind
the callback to the identity that owns the lease. The callback remains
registered until lease cleanup finishes. Sign-out and shutdown close that
identity's lease; they do not stop the shared module or another identity's
runtime. `runtime_driver_with_dispatch` opts into host-selected bounded
concurrency; `runtime_driver` retains serial inbound dispatch.

## Native test proof boundaries

The client E2E suite uses locally built, digest-pinned artifacts and does not
call external providers. The controlled module fixture supports peer handover:
an ordinary readiness control demonstrates that a competing peer is live,
while a typed object response distinguishes the originally bound peer from the
new alias owner. The readiness control is not itself a confidential-call
target, so it cannot prove a negative confidential canary.

The production module composition test loads the actual module library with an
empty provider configuration. It covers reserve, object creation, start,
typed object calls and acknowledged stop, without claiming provider traffic or
module-originated callback delivery. Callback registration is tested through
the host connection. These proofs remain separate from controlled peer
handover.

Run `scripts/test-features.sh` for the all-feature lanes. It stages the local
artifacts before the feature-enabled Cargo run, including the four client
native targets and their expected test counts. Those targets remain required;
a bare Cargo all-features invocation does not substitute for the staged native
suite.
