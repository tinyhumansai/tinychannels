//! Harness communication boundaries.
//!
//! This module is reserved for the typed contracts that connect channel events
//! to OpenHuman harness execution.

pub mod bridge;
pub mod types;

pub use bridge::{BridgeTranslationOptions, translate_output_event};
pub use types::{
    ChannelOutputEvent, ChannelTurn, HarnessLifecycleEvent, InboundLifecycleStage,
    TurnAdmissionVerdict,
};

#[cfg(test)]
mod test;
