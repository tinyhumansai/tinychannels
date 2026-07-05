//! Channel-side messaging abstractions and portable channel layer types.

pub mod capabilities;
pub mod envelope;
pub mod error;
pub mod intent;
pub mod receipt;
pub mod session;
pub mod types;

pub use crate::traits::{Channel, ChannelMessage, SendMessage};
pub use capabilities::{
    ChannelPresentationCapabilities, ChannelStaticCapabilities, DurableFinalDeliveryCapability,
    DurableFinalDeliveryRequirementMap, LengthUnit, MarkdownDialect,
    durable_final_delivery_capabilities,
};
pub use envelope::{
    AccessContext, ChannelInboundEnvelope, GroupAccessPolicy, InboundMediaPayload, MediaKind,
    MediaReference, MentionGate, SenderDmDecision,
};
pub use error::{ChannelSendError, SendErrorKind, classify_send_error, is_chat_level_not_found};
pub use intent::{ChannelOutboundIntent, DeliveryDurability, OutboundPayload};
pub use receipt::{
    MessageReceipt, MessageReceiptPart, MessageReceiptPartKind, MessageReceiptSourceResult,
    create_message_receipt_from_outbound_results, list_message_receipt_platform_ids,
    resolve_message_receipt_primary_id,
};
pub use session::{
    LegacySessionKeys, SessionKeyPolicy, build_session_key, conversation_history_key_candidates,
};
pub use types::{
    ChannelDescriptor, ChannelRef, ConversationKind, ConversationRef, SecretRef, SenderRef,
};

#[cfg(test)]
mod test;
