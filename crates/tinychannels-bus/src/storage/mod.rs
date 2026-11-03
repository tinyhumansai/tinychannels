//! Pure storage vocabulary for TinyChannels-owned records.
//!
//! Migration execution belongs to the TinyChannels implementation. Hosts may
//! use [`collections`] to provision the declared collections, but do not
//! compose or execute this owner's migration policy.

pub mod records;
pub mod schema;

pub use records::ChatBinding;
pub use schema::collections;
pub use schema::{CollectionSpec, IndexSpec, SearchSpec};
pub use schema::{MEDIA_BLOB_PREFIX, MEDIA_METADATA, media_metadata_spec};
