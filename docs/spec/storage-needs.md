# TinyChannels storage contract

The transport-free `tinychannels-bus` crate exposes its own synchronous
`CollectionSpec` declarations through `tinychannels_bus::storage::collections()`.
These serializable DTOs do not depend on TSD types or a storage runtime. Host
composition roots use those declarations when installing a scoped storage
owner. They do not select a scope or grant the module access to an unscoped
backend.

The collection inventory is defined in
[`tinychannels_bus::storage::schema`](../../crates/tinychannels-bus/src/storage/schema.rs).
It includes 30 delivery, remote-session, media, migration-progress, and WhatsApp
Web collections, including the legacy source collections read during migration. WhatsApp collections remain declared when the feature is
disabled, so provisioning does not depend on the module build.

The implementation connects through TSD2's storage ports. A linked host passes
scoped handles directly; a loaded module receives a unique `storage_callback`
destination in its setup configuration and calls `connect_storage_at`. When
that destination is configured, a failed connection does not fall back to
legacy global storage. `tinychannels::storage::prepare` provisions the owner
collections and runs idempotent owner-local migrations before stores open.

Legacy record conversion rules and migration execution belong to the implementation
in [`src/storage/migrations.rs`](../../src/storage/migrations.rs).
The bus crate contains only collection declarations and record vocabulary; it
has no storage client, driver, or backend implementation.

The standalone consumer in [`proofs/storage-needs-consumer`](../../proofs/storage-needs-consumer/)
depends only on `tinychannels-bus` and checks the declaration inventory. Run
`python3 scripts/check-storage-needs-boundary.py` to verify that its complete
normal/build dependency closure contains only the bus and its reviewed
serialization/schema dependencies. The gate rejects TSD types and storage
clients as well as runtime and transport packages.
