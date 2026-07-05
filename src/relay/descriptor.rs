//! Relay capability descriptor handshake contract.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub const CONTRACT_VERSION: u32 = 1;
pub const DEFAULT_MAX_MESSAGE_LENGTH: u32 = 4096;

fn default_contract_version() -> u32 {
    CONTRACT_VERSION
}

fn default_emoji() -> String {
    "🔌".to_string()
}

/// Immutable capability descriptor negotiated at relay handshake.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct CapabilityDescriptor {
    pub contract_version: u32,
    pub platform: String,
    pub label: String,
    pub max_message_length: u32,
    pub supports_draft_streaming: bool,
    pub supports_edit: bool,
    pub supports_threads: bool,
    pub markdown_dialect: String,
    pub len_unit: String,
    pub emoji: String,
    pub platform_hint: String,
    pub pii_safe: bool,
}

impl Default for CapabilityDescriptor {
    fn default() -> Self {
        Self {
            contract_version: default_contract_version(),
            platform: String::new(),
            label: String::new(),
            max_message_length: DEFAULT_MAX_MESSAGE_LENGTH,
            supports_draft_streaming: false,
            supports_edit: false,
            supports_threads: false,
            markdown_dialect: "plain".to_string(),
            len_unit: "chars".to_string(),
            emoji: default_emoji(),
            platform_hint: String::new(),
            pii_safe: false,
        }
    }
}

impl CapabilityDescriptor {
    /// Serialize to compact JSON with lexicographically sorted keys.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(&self.as_sorted_json_object())
    }

    /// Deserialize from a handshake JSON string, ignoring unknown keys.
    pub fn from_json(data: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(data)
    }

    pub fn from_platform_entry(
        entry: &RelayPlatformEntry,
        options: RelayDescriptorOptions,
    ) -> Self {
        Self {
            contract_version: CONTRACT_VERSION,
            platform: entry.name.clone(),
            label: entry.label.clone(),
            max_message_length: if entry.max_message_length == 0 {
                DEFAULT_MAX_MESSAGE_LENGTH
            } else {
                entry.max_message_length
            },
            supports_draft_streaming: options.supports_draft_streaming,
            supports_edit: options.supports_edit,
            supports_threads: options.supports_threads,
            markdown_dialect: options.markdown_dialect,
            len_unit: options.len_unit,
            emoji: entry.emoji.clone().unwrap_or_else(default_emoji),
            platform_hint: entry.platform_hint.clone().unwrap_or_default(),
            pii_safe: entry.pii_safe,
        }
    }

    fn as_sorted_json_object(&self) -> BTreeMap<&'static str, Value> {
        BTreeMap::from([
            ("contract_version", json!(self.contract_version)),
            ("emoji", json!(self.emoji)),
            ("label", json!(self.label)),
            ("len_unit", json!(self.len_unit)),
            ("markdown_dialect", json!(self.markdown_dialect)),
            ("max_message_length", json!(self.max_message_length)),
            ("pii_safe", json!(self.pii_safe)),
            ("platform", json!(self.platform)),
            ("platform_hint", json!(self.platform_hint)),
            (
                "supports_draft_streaming",
                json!(self.supports_draft_streaming),
            ),
            ("supports_edit", json!(self.supports_edit)),
            ("supports_threads", json!(self.supports_threads)),
        ])
    }
}

/// Projection input mirroring Hermes `PlatformEntry` fields used by relay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayPlatformEntry {
    pub name: String,
    pub label: String,
    pub max_message_length: u32,
    pub emoji: Option<String>,
    pub platform_hint: Option<String>,
    pub pii_safe: bool,
}

/// Runtime capability bits supplied by the connector's live adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayDescriptorOptions {
    pub len_unit: String,
    pub supports_draft_streaming: bool,
    pub supports_edit: bool,
    pub supports_threads: bool,
    pub markdown_dialect: String,
}

impl Default for RelayDescriptorOptions {
    fn default() -> Self {
        Self {
            len_unit: "chars".to_string(),
            supports_draft_streaming: false,
            supports_edit: true,
            supports_threads: false,
            markdown_dialect: "plain".to_string(),
        }
    }
}
