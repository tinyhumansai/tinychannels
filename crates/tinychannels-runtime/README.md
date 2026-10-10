# tinychannels-runtime

Shared implementation mechanics used by the TinyChannels library and module:
provider/adapter callbacks, listener supervision, dispatch and session lifetime,
pairing state and crypto, relay authentication and I/O seams, deterministic
idempotency/session keys, history compaction, media/receipt normalization,
configuration preparation, text formatting and reply segmentation.

The crate depends on `tinychannels-bus`, never the root provider library. Public
DTO paths re-export their bus definitions without copies. The root library
re-exports these modules to preserve its published implementation paths.

Extension traits implement algorithms on bus-owned DTOs. Import the appropriate
trait to retain method syntax; see the
[relocation specification](../../docs/spec/pure-contract-relocation.md).
Runtime traits are in-process provider seams, not serialized bus callbacks.
Additional loadable-module operations remain a separate migration slice.
