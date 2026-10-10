# tinychannels-bus

Serialized channel vocabulary, configuration/schema declarations, interface
names, relay frames and structured errors. Normal dependencies are Serde,
JSON/schema/error derivation, and Base64 used only by the existing
`PassthroughForward.body` custom serializer (`bodyB64`). This crate has no
provider callbacks, scheduler, crypto, entropy, stateful locks, transport,
session derivation or delivery algorithms.

DTOs are defined once here. `tinychannels-runtime` and `tinychannels` re-export
these same Rust types. Hosts should name wire values through this crate and
execute behavior through the compiled module. The root library compatibility
facade is for existing implementation consumers, not a host dependency shortcut.

`python3 scripts/check-bus-purity.py --offline` audits independent default and
all-feature normal/build dependency closures and rejects runtime source seams.
The approved 21-package closure includes Base64 solely for serialization;
crypto and UUID generation remain implementation dependencies.

The original module and host callback names, method arities, JSON field names,
optional fields and persisted configuration values are unchanged. See the
[relocation specification](../../docs/spec/pure-contract-relocation.md) for
necessary Rust source API migrations and subsequent module-operation slices.
