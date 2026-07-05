//! Channel capability surfaces.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Length unit used by a provider's presentation and chunking limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum LengthUnit {
    Characters,
    Utf8Bytes,
    Utf16Units,
}

/// Markdown or markup dialect accepted by a provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum MarkdownDialect {
    Plain,
    Markdown,
    Html,
    SlackMrkdwn,
    DiscordMarkdown,
    TelegramMarkdownV2,
}

/// Static provider feature flags.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct ChannelStaticCapabilities {
    pub chat_types: Vec<String>,
    pub polls: bool,
    pub reactions: bool,
    pub edit: bool,
    pub unsend: bool,
    pub reply: bool,
    pub threads: bool,
    pub media: bool,
    pub native_commands: bool,
    pub block_streaming: bool,
}

/// Presentation and text limits advertised by a provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct ChannelPresentationCapabilities {
    pub text_limit: Option<usize>,
    pub length_unit: LengthUnit,
    pub markdown_dialect: MarkdownDialect,
    pub supports_edit: bool,
    pub max_actions: Option<usize>,
    pub max_actions_per_row: Option<usize>,
    pub max_rows: Option<usize>,
    pub max_label_length: Option<usize>,
    pub max_value_bytes: Option<usize>,
    pub supports_styles: bool,
    pub supports_disabled: bool,
    pub supports_layout_hints: bool,
}

impl Default for ChannelPresentationCapabilities {
    fn default() -> Self {
        Self {
            text_limit: None,
            length_unit: LengthUnit::Characters,
            markdown_dialect: MarkdownDialect::Plain,
            supports_edit: false,
            max_actions: None,
            max_actions_per_row: None,
            max_rows: None,
            max_label_length: None,
            max_value_bytes: None,
            supports_styles: false,
            supports_disabled: false,
            supports_layout_hints: false,
        }
    }
}

/// Durable-final delivery capability key understood by message adapters.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum DurableFinalDeliveryCapability {
    Text,
    Media,
    Poll,
    Payload,
    Silent,
    ReplyTo,
    Thread,
    NativeQuote,
    MessageSendingHooks,
    Batch,
    ReconcileUnknownSend,
    AfterSendSuccess,
    AfterCommit,
}

/// Capability map used by adapters to declare durable final-delivery support.
pub type DurableFinalDeliveryRequirementMap = BTreeMap<DurableFinalDeliveryCapability, bool>;

/// OpenClaw's stable durable-final capability key list, in contract order.
pub fn durable_final_delivery_capabilities() -> &'static [DurableFinalDeliveryCapability] {
    use DurableFinalDeliveryCapability::*;
    &[
        Text,
        Media,
        Poll,
        Payload,
        Silent,
        ReplyTo,
        Thread,
        NativeQuote,
        MessageSendingHooks,
        Batch,
        ReconcileUnknownSend,
        AfterSendSuccess,
        AfterCommit,
    ]
}
