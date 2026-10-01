//! Yuanbao channel configuration re-exported from tinychannels.

pub use crate::config::YuanbaoConfig;

/// Default value for `DeviceInfo.app_version` (server-side `plugin_version`).
pub(crate) const DEFAULT_PLUGIN_VERSION: &str = "0.1.0";

/// Strip legacy `openhuman/` prefix from version strings in config/TOML.
pub(crate) fn strip_version_prefix(version: &str) -> &str {
    crate::config::strip_yuanbao_version_prefix(version)
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
