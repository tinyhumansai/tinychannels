//! TinyBus wire contract for TinyChannels: the call vocabulary a host and the
//! channels module share.
//!
//! This crate is deliberately transport-free and dependency-light. It owns the
//! types that cross the boundary — inbound envelopes, outbound intents, channel
//! configuration, controller metadata, relay frames and the session-key rules —
//! and nothing that opens a socket, spawns a task or touches a database.
//!
//! # Why the split is here and not elsewhere
//!
//! A host that only needs to *name* a channel message pays for serde and
//! nothing more; the provider stack (Telegram, Discord, Slack, IMAP, …) with
//! its `reqwest`, `rusqlite`, `rustls` and `tokio-tungstenite` dependencies
//! lives in the `tinychannels` crate and is reached over the bus.
//!
//! Two rules follow from that, and both are load-bearing:
//!
//! - **Never re-declare a contract type in a host.** A field added on one side
//!   of a copy is a decode failure on the other with nothing to catch it.
//! - **Call members by their constant**, never by a string literal —
//!   [`methods::SEND_MESSAGE`], not `"SendMessage"`. A rename upstream is then a
//!   compile error rather than a `MemberNotFound` in the field.
//!
//! # Session keys are contract, not policy
//!
//! [`build_session_key_for_inbound_envelope`] derives a **persisted** grouping
//! key that a host writes to its conversation store. It lives here for the same
//! reason `tinywallet-bus` owns address validation: both sides must agree
//! exactly, and a drifted copy silently regroups existing user data.
//!
//! # `traits` and `security` are in-process seams, not wire vocabulary
//!
//! Everything else here is transport-free. These two are not, and the
//! distinction is worth keeping visible rather than quietly averaging away:
//!
//! - [`traits::Channel`] hands a provider a `tokio::sync::mpsc::Sender` to
//!   publish received messages on. That is an *embedding* seam — a provider
//!   compiled into the same binary — and it has no bus equivalent, because a
//!   module delivers inbound traffic by calling the host's
//!   [`names::HOST_BUS_NAME`] object instead.
//! - [`security`] runs its constant-time pairing compare on the blocking pool.
//!
//! They live here because hosts that only ever name them still need them
//! always-on, and splitting a third crate off for two items would cost more
//! than the honesty is worth. The `tokio` dependency is pinned to `sync` + `rt`
//! for exactly this reason: no scheduler, no I/O driver, no timers. Do not
//! reach for a wider tokio feature here — if something needs one, it belongs in
//! the `tinychannels` crate.

pub mod adapters;
pub mod capabilities;
pub mod channel;
pub mod config;
pub mod context;
pub mod controllers;
pub mod delivery;
pub mod error;
pub mod names;
pub mod relay;
pub mod security;
pub mod text;
pub mod traits;
pub mod version;

pub use capabilities::{ChannelCapabilities, capabilities_for, provider_id};
pub use channel::{
    ChannelInboundEnvelope, ChannelOutboundIntent, DeliveryDurability, OutboundPayload,
    build_session_key_for_inbound_envelope, derive_inbound_client_id, derive_inbound_thread_id,
    inbound_envelope_from_legacy_message, legacy_message_from_inbound_envelope,
    legacy_message_value_from_outbound_intent, outbound_intent_from_legacy_message,
    outbound_intent_from_send_message,
};
pub use config::ChannelsConfig;
pub use controllers::{ChannelAuthMode, ChannelDefinition};
pub use error::{Result, TinyChannelsError};
pub use names::{BUS_NAME, HOST_BUS_NAME, HOST_OBJECT_PATH, METHODS, OBJECT_PATH, methods};
pub use traits::{Channel, ChannelMessage, ChannelSendExt, SendMessage};
pub use version::{CONTRACT_VERSION, is_compatible};
