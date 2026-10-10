//! Channel-side messaging abstractions and portable channel layer types.

pub mod adapter;
pub mod capabilities;
pub mod envelope;
pub mod error;
pub mod intent;
pub mod receipt;
pub mod session;
pub mod types;

pub use crate::traits::{ChannelMessage, SendMessage};
pub use adapter::ChannelReceiveAckPolicy;
pub use capabilities::{
    CHANNEL_MESSAGE_ACTION_NAMES, ChannelPresentationCapabilities, ChannelStaticCapabilities,
    DurableFinalDeliveryCapability, DurableFinalDeliveryRequirementMap, LengthUnit,
    MarkdownDialect, channel_message_action_names, durable_final_delivery_capabilities,
};
pub use envelope::{
    AccessContext, ChannelInboundEnvelope, GroupAccessPolicy, InboundMediaPayload, MediaKind,
    MediaReference, MentionGate, SenderDmDecision,
};
pub use error::{ChannelSendError, SendErrorKind};
pub use intent::{ChannelOutboundIntent, DeliveryDurability, OutboundPayload};
pub use receipt::{
    MessageReceipt, MessageReceiptPart, MessageReceiptPartKind, MessageReceiptSourceResult,
};
pub use session::{LegacySessionKeys, SessionKeyPolicy};
pub use types::{
    ChannelDescriptor, ChannelRef, ConversationKind, ConversationRef, SecretRef, SenderRef,
};
