//! Credential helper utilities shared by channel controller implementations.

use serde_json::Value;

use super::ChannelAuthMode;

/// Credential provider key for channel connections: `"channel:{id}:{mode}"`.
pub fn channel_credential_provider(channel_id: &str, mode: ChannelAuthMode) -> String {
    format!("channel:{channel_id}:{mode}")
}

/// Parse an optional allowlist JSON field into canonical identities.
///
/// Accepts comma/newline separated strings or arrays of strings. Leading `@`
/// prefixes are stripped, whitespace is trimmed, and entries are lowercased and
/// deduplicated case-insensitively. Non-string values are ignored.
pub fn parse_allowed_users(value: Option<&Value>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();

    let mut push_identity = |raw: &str| {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return;
        }
        let normalized = trimmed.trim_start_matches('@').trim();
        if normalized.is_empty() {
            return;
        }
        let canonical = normalized.to_lowercase();
        if !out
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(&canonical))
        {
            out.push(canonical);
        }
    };

    match value {
        Some(Value::String(s)) => {
            for part in s.split([',', '\n', '\r']) {
                push_identity(part);
            }
        }
        Some(Value::Array(items)) => {
            for item in items {
                if let Some(s) = item.as_str() {
                    for part in s.split([',', '\n', '\r']) {
                        push_identity(part);
                    }
                }
            }
        }
        _ => {}
    }

    out
}

#[cfg(test)]
#[path = "credentials_tests.rs"]
mod tests;
