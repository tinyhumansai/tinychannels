//! Pluggable channel and messaging primitives for OpenHuman harness
//! communication.
//!
//! This crate is the **implementation** half of TinyChannels: the provider
//! stack (Telegram, Discord, Slack, IMAP, WhatsApp, …), the relay transport
//! loop, delivery, and the host boundary OpenHuman implements for channel side
//! effects.
//!
//! The **contract** half — every type that crosses a bus or an embedding
//! boundary — lives in [`tinychannels_bus`] and is re-exported here under the
//! module paths it has always occupied, so `tinychannels::channel::…`,
//! `tinychannels::config::…` and friends keep resolving unchanged.
//!
//! # Which half to depend on
//!
//! Depend on `tinychannels-bus` alone if you only need to *name* channel types
//! — an envelope in an event enum, the config schema, session-key derivation.
//! That costs serde and little else. Depend on this crate when you need to
//! actually open a socket, which pulls `reqwest`, `rusqlite`, `rustls` and
//! `tokio-tungstenite`.

// Re-export the WebSocket transport crate so downstream consumers (and their
// tests that exercise provider WS seams) can construct version-matched
// `tungstenite` messages without pinning the same version themselves.
pub use tokio_tungstenite;

// The contract crate, re-exported whole so a downstream can reach it by either
// name. Prefer `tinychannels_bus::…` in new code.
pub use tinychannels_bus;

// Contract modules, re-exported at the paths they have always occupied. These
// are `pub use` of the *same* items, not copies — there is exactly one
// `ChannelInboundEnvelope` in the build.
pub use tinychannels_bus::{
    adapters, capabilities, channel, config, context, controllers, error, names, security, text,
    traits, version,
};

pub mod approvals;
pub mod backend;
pub mod delivery;
pub mod factory;
pub mod harness;
pub mod host;
pub mod providers;
pub mod relay;
pub mod remote;
pub mod routes;
pub mod runtime;

pub use backend::{ChannelBackend, ChannelManager};
pub use factory::{DefaultHttpClients, HttpClientFactory, build_channels};
pub use host::{ChannelHost, ChannelHostBuilder, HostCapabilities, NoopHost, ProviderContext};
pub use providers::{
    CliChannel, DingTalkChannel, DiscordChannel, IMessageChannel, IrcChannel, IrcChannelConfig,
    LinqChannel, MattermostChannel, QQChannel, SignalChannel, SlackChannel, TelegramChannel,
    WhatsAppChannel, WhatsAppWebChannel, YuanbaoChannel,
};
pub use tinychannels_bus::{
    BUS_NAME, CONTRACT_VERSION, Channel, ChannelAuthMode, ChannelCapabilities, ChannelDefinition,
    ChannelInboundEnvelope, ChannelMessage, ChannelOutboundIntent, ChannelSendExt, ChannelsConfig,
    DeliveryDurability, HOST_BUS_NAME, HOST_OBJECT_PATH, METHODS, OBJECT_PATH, OutboundPayload,
    Result, SendMessage, TinyChannelsError, build_session_key_for_inbound_envelope,
    capabilities_for, inbound_envelope_from_legacy_message, is_compatible,
    legacy_message_from_inbound_envelope, legacy_message_value_from_outbound_intent, methods,
    outbound_intent_from_legacy_message, outbound_intent_from_send_message,
};
// Re-exported separately so each can follow its provider's feature gate.
#[cfg(feature = "email-send")]
pub use providers::EmailChannel;
#[cfg(feature = "lark")]
pub use providers::LarkChannel;

#[cfg(all(test, feature = "email-send"))]
#[path = "lib_email_feature_smoke_tests.rs"]
mod email_feature_smoke_tests;

#[cfg(all(test, feature = "lark"))]
#[path = "lib_lark_feature_smoke_tests.rs"]
mod lark_feature_smoke_tests;
