//! Canonical collection names shared by the schema and module storage request.

/// Entries awaiting a terminal outcome.
pub const DELIVERY_PENDING: &str = "channels.delivery_pending";
/// Permanently failed entries.
pub const DELIVERY_FAILED: &str = "channels.delivery_failed";
/// Idempotent terminal results (they expire through record metadata).
pub const DELIVERY_RESULTS: &str = "channels.delivery_results";

/// Chat to thread bindings for remote control.
pub const REMOTE_BINDINGS: &str = "channels.remote_bindings";
/// Chats with a turn in flight.
pub const REMOTE_BUSY: &str = "channels.remote_busy";
/// The one-time legacy import marker.
pub const REMOTE_META: &str = "channels.remote_meta";

/// The linked device record.
pub const WHATSAPP_DEVICE: &str = "channels.whatsapp_device";
/// Signal identity keys.
pub const WHATSAPP_IDENTITY: &str = "channels.whatsapp_identity";
/// Signal sessions.
pub const WHATSAPP_SESSION: &str = "channels.whatsapp_session";
/// One-time pre-keys.
pub const WHATSAPP_PREKEY: &str = "channels.whatsapp_prekey";
/// Signed pre-keys.
pub const WHATSAPP_SIGNED_PREKEY: &str = "channels.whatsapp_signed_prekey";
/// Group sender keys.
pub const WHATSAPP_SENDER_KEY: &str = "channels.whatsapp_sender_key";
/// App-state sync keys.
pub const WHATSAPP_SYNC_KEY: &str = "channels.whatsapp_sync_key";
/// LID to phone-number mappings.
pub const WHATSAPP_LID_MAPPING: &str = "channels.whatsapp_lid_mapping";

/// Every WhatsApp Web collection, in declaration order.
pub const WHATSAPP_COLLECTIONS: [&str; 8] = [
    WHATSAPP_DEVICE,
    WHATSAPP_IDENTITY,
    WHATSAPP_SESSION,
    WHATSAPP_PREKEY,
    WHATSAPP_SIGNED_PREKEY,
    WHATSAPP_SENDER_KEY,
    WHATSAPP_SYNC_KEY,
    WHATSAPP_LID_MAPPING,
];

/// Every collection the `channels` owner declares, in declaration order.
pub const COLLECTIONS: [&str; 14] = [
    DELIVERY_PENDING,
    DELIVERY_FAILED,
    DELIVERY_RESULTS,
    REMOTE_BINDINGS,
    REMOTE_BUSY,
    REMOTE_META,
    WHATSAPP_DEVICE,
    WHATSAPP_IDENTITY,
    WHATSAPP_SESSION,
    WHATSAPP_PREKEY,
    WHATSAPP_SIGNED_PREKEY,
    WHATSAPP_SENDER_KEY,
    WHATSAPP_SYNC_KEY,
    WHATSAPP_LID_MAPPING,
];
