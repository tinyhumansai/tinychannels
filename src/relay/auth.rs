//! Relay HMAC authentication primitives.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::time::{SystemTime, UNIX_EPOCH};

pub const DELIVERY_TS_HEADER: &str = "x-relay-timestamp";
pub const DELIVERY_SIG_HEADER: &str = "x-relay-signature";
pub const DEFAULT_MAX_SKEW_SECONDS: i64 = 300;
pub const DEFAULT_UPGRADE_TTL_SECONDS: i64 = 300;

type HmacSha256 = Hmac<Sha256>;

/// HMAC-SHA256 hex digest of `payload` under `secret` using UTF-8 bytes.
pub fn sign(payload: &str, secret: &str) -> String {
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(payload.as_bytes());
    encode_hex(&mac.finalize().into_bytes())
}

/// Constant-time check against any secret in a rotation verify list.
pub fn verify_signature(payload: &str, sig_hex: &str, secrets: &[impl AsRef<str>]) -> bool {
    let Some(sig_bytes) = decode_hex(sig_hex) else {
        return false;
    };
    if sig_bytes.is_empty() {
        return false;
    }
    for secret in secrets {
        let mut mac = HmacSha256::new_from_slice(secret.as_ref().as_bytes())
            .expect("HMAC accepts any key length");
        mac.update(payload.as_bytes());
        if mac.verify_slice(&sig_bytes).is_ok() {
            return true;
        }
    }
    false
}

/// Build `base64url("{payload}:{exp}:{sig}")`.
pub fn make_token_at(
    payload: &str,
    secret: &str,
    ttl_seconds: i64,
    now_unix_seconds: i64,
) -> String {
    let exp = if ttl_seconds > 0 {
        now_unix_seconds.saturating_add(ttl_seconds)
    } else {
        0
    };
    let signed = format!("{payload}:{exp}");
    let sig = sign(&signed, secret);
    URL_SAFE_NO_PAD.encode(format!("{signed}:{sig}").as_bytes())
}

/// Build a token using the current system clock.
pub fn make_token(payload: &str, secret: &str, ttl_seconds: i64) -> String {
    make_token_at(payload, secret, ttl_seconds, now_unix_seconds())
}

pub fn make_upgrade_token_at(
    gateway_id: &str,
    secret: &str,
    ttl_seconds: i64,
    now_unix_seconds: i64,
) -> String {
    make_token_at(gateway_id, secret, ttl_seconds, now_unix_seconds)
}

pub fn make_upgrade_token(gateway_id: &str, secret: &str, ttl_seconds: i64) -> String {
    make_token(gateway_id, secret, ttl_seconds)
}

/// Verify a token and return the signed payload.
pub fn verify_token_at(
    token: &str,
    secrets: &[impl AsRef<str>],
    now_unix_seconds: i64,
) -> Option<String> {
    let decoded = URL_SAFE_NO_PAD.decode(token.as_bytes()).ok()?;
    let decoded = String::from_utf8(decoded).ok()?;
    let (payload_and_exp, sig) = decoded.rsplit_once(':')?;
    let (payload, exp) = payload_and_exp.rsplit_once(':')?;
    let exp = exp.parse::<i64>().ok()?;
    if exp != 0 && now_unix_seconds > exp {
        return None;
    }
    if verify_signature(&format!("{payload}:{exp}"), sig, secrets) {
        Some(payload.to_string())
    } else {
        None
    }
}

/// Verify a token using the current system clock.
pub fn verify_token(token: &str, secrets: &[impl AsRef<str>]) -> Option<String> {
    verify_token_at(token, secrets, now_unix_seconds())
}

/// Verify connector-to-gateway inbound delivery signature.
pub fn verify_delivery_signature_at(
    body_json: &str,
    timestamp: Option<&str>,
    signature: Option<&str>,
    verify_keys: &[impl AsRef<str>],
    max_skew_seconds: i64,
    now_unix_seconds: i64,
) -> bool {
    let (Some(timestamp), Some(signature)) = (timestamp, signature) else {
        return false;
    };
    let Ok(ts) = timestamp.parse::<i64>() else {
        return false;
    };
    if now_unix_seconds.abs_diff(ts) > max_skew_seconds as u64 {
        return false;
    }
    verify_signature(&delivery_payload(ts, body_json), signature, verify_keys)
}

pub fn verify_delivery_signature(
    body_json: &str,
    timestamp: Option<&str>,
    signature: Option<&str>,
    verify_keys: &[impl AsRef<str>],
) -> bool {
    verify_delivery_signature_at(
        body_json,
        timestamp,
        signature,
        verify_keys,
        DEFAULT_MAX_SKEW_SECONDS,
        now_unix_seconds(),
    )
}

pub fn delivery_payload(timestamp: i64, body_json: &str) -> String {
    format!("{timestamp}.{body_json}")
}

fn now_unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn decode_hex(input: &str) -> Option<Vec<u8>> {
    if !input.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(input.len() / 2);
    let mut chars = input.as_bytes().iter().copied();
    while let (Some(high), Some(low)) = (chars.next(), chars.next()) {
        out.push((hex_value(high)? << 4) | hex_value(low)?);
    }
    Some(out)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
