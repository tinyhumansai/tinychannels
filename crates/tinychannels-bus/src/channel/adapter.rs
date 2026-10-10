//! Serialized inbound acknowledgement policy.

/// Inbound receive acknowledgement timing.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ChannelReceiveAckPolicy {
    #[default]
    AfterReceiveRecord,
    AfterAgentDispatch,
    AfterDurableSend,
    Manual,
}
