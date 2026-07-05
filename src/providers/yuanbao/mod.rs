//! Yuanbao (元宝) channel provider.
//!
//! This module is intentionally export-focused. Operational code lives in
//! sibling modules:
//! - [`channel`] wires the provider into the generic OpenHuman `Channel` trait.
//! - [`connection`] owns the WebSocket transport and request correlator.
//! - [`inbound`] owns inbound filtering/extraction.
//! - [`outbound`] owns Yuanbao send/query calls.
//! - [`proto`] / [`proto_biz`] / [`wire`] own hand-written protobuf codecs.

// Test setup across the yuanbao submodules builds configs via
// `Default::default()` then field assignment; the stylistic
// `field_reassign_with_default` lint isn't worth churning that code.
#![allow(clippy::field_reassign_with_default)]

pub mod channel;
pub mod config;
pub mod connection;
pub mod cos;
pub mod errors;
pub mod ids;
pub mod inbound;
pub mod media;
pub mod outbound;
pub mod proto;
pub mod proto_biz;
pub mod proto_constants;
pub mod sign;
pub mod splitter;
pub mod types;
pub mod wire;

pub use channel::YuanbaoChannel;
pub use config::YuanbaoConfig;
