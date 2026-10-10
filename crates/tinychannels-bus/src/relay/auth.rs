//! Relay authentication header and policy declarations.

pub const DELIVERY_TS_HEADER: &str = "x-relay-timestamp";
pub const DELIVERY_SIG_HEADER: &str = "x-relay-signature";
pub const DEFAULT_MAX_SKEW_SECONDS: i64 = 300;
pub const DEFAULT_UPGRADE_TTL_SECONDS: i64 = 300;
