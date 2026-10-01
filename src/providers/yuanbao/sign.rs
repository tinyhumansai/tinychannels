//! Token sign manager — talks to `/api/v5/robotLogic/sign-token` to
//! exchange `(app_key, app_secret)` for a short-lived WS token + bot_id.
//!
//! Mirrors hermes-agent `SignManager` (yuanbao.py 641-881). Implements:
//!   - per-app_key tokio `Mutex` to coalesce concurrent refresh attempts
//!   - 60-second early-refresh margin to avoid using a token that's
//!     about to expire mid-handshake
//!   - retry on `code=10099` up to 3 times
//!
//! Signature scheme (TS plugin compatible):
//!   plain     = nonce + timestamp + app_key + app_secret
//!   signature = HMAC-SHA256(key = app_secret, msg = plain) as lower-hex

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::FixedOffset;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use tokio::sync::Mutex;
use tracing::{info, warn};

use super::errors::YuanbaoError;

const SIGN_PATH: &str = "/api/v5/robotLogic/sign-token";
const RETRYABLE_CODE: i64 = 10099;
const MAX_RETRIES: usize = 3;
const RETRY_DELAY_MS: u64 = 1_000;
/// Treat as expiring this many seconds before actual expiry so a fresh
/// token is fetched before the running one dies mid-request.
const CACHE_REFRESH_MARGIN_SECS: u64 = 60;
const HTTP_TIMEOUT_SECS: u64 = 10;
const DEFAULT_DURATION_SECS: u64 = 3600;

/// One cached token entry.
#[derive(Debug, Clone)]
pub struct TokenEntry {
    pub token: String,
    pub bot_id: String,
    pub product: String,
    pub source: String,
    /// Seconds-since-epoch when this token expires server-side.
    pub expire_ts: u64,
}

impl TokenEntry {
    pub fn is_valid(&self) -> bool {
        let now = unix_now();
        self.expire_ts > now + CACHE_REFRESH_MARGIN_SECS
    }

    pub fn seconds_remaining(&self) -> i64 {
        self.expire_ts as i64 - unix_now() as i64
    }
}

type HmacSha256 = Hmac<Sha256>;

/// Compute the `signature` field for the sign-token API.
pub fn compute_signature(nonce: &str, timestamp: &str, app_key: &str, app_secret: &str) -> String {
    let plain = format!("{nonce}{timestamp}{app_key}{app_secret}");
    let mut mac =
        HmacSha256::new_from_slice(app_secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(plain.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// Build Beijing-time ISO-8601 timestamp without milliseconds.
/// Format: `2006-01-02T15:04:05+08:00`.
pub fn build_timestamp() -> String {
    let bj_offset = FixedOffset::east_opt(8 * 3600).expect("valid offset");
    let now = chrono::Utc::now().with_timezone(&bj_offset);
    now.format("%Y-%m-%dT%H:%M:%S+08:00").to_string()
}

/// Generate a 32-char hex nonce.
pub fn generate_nonce() -> String {
    let mut bytes = [0u8; 16];
    for b in &mut bytes {
        *b = rand::random::<u8>();
    }
    hex::encode(bytes)
}

/// Process-wide token manager. One instance is built per `YuanbaoChannel`
/// and shared with the connection layer; the per-app_key Mutex makes it
/// safe to have multiple connections sharing this manager.
pub struct SignManager {
    http: reqwest::Client,
    /// Per-app_key refresh mutexes — coalesce concurrent refresh attempts.
    locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    /// Token cache keyed by app_key.
    cache: Mutex<HashMap<String, TokenEntry>>,
}

impl SignManager {
    pub fn new(http: reqwest::Client) -> Arc<Self> {
        Arc::new(Self {
            http,
            locks: Mutex::new(HashMap::new()),
            cache: Mutex::new(HashMap::new()),
        })
    }

    /// Look up a cached token without touching the network.
    pub async fn cached(&self, app_key: &str) -> Option<TokenEntry> {
        let cache = self.cache.lock().await;
        cache.get(app_key).cloned().filter(|e| e.is_valid())
    }

    /// Test-only: inject a cache entry without touching the sign endpoint.
    #[cfg(test)]
    pub(crate) async fn set_cached_for_test(&self, app_key: &str, entry: TokenEntry) {
        self.cache.lock().await.insert(app_key.to_string(), entry);
    }

    /// Get a valid token, fetching one if the cache is empty or stale.
    pub async fn get_token(
        &self,
        app_key: &str,
        app_secret: &str,
        api_domain: &str,
        route_env: &str,
    ) -> Result<TokenEntry, YuanbaoError> {
        if let Some(entry) = self.cached(app_key).await {
            info!(
                "[yuanbao/sign] using cached token ({}s remaining)",
                entry.seconds_remaining()
            );
            return Ok(entry);
        }
        self.refresh(app_key, app_secret, api_domain, route_env)
            .await
    }

    /// Force-refresh: drop the cache entry and re-fetch.
    pub async fn force_refresh(
        &self,
        app_key: &str,
        app_secret: &str,
        api_domain: &str,
        route_env: &str,
    ) -> Result<TokenEntry, YuanbaoError> {
        {
            let mut cache = self.cache.lock().await;
            cache.remove(app_key);
        }
        warn!(
            "[yuanbao/sign] force-refresh app_key=****{}",
            suffix(app_key)
        );
        self.refresh(app_key, app_secret, api_domain, route_env)
            .await
    }

    async fn refresh(
        &self,
        app_key: &str,
        app_secret: &str,
        api_domain: &str,
        route_env: &str,
    ) -> Result<TokenEntry, YuanbaoError> {
        let lock = self.get_refresh_lock(app_key).await;
        let _g = lock.lock().await;

        // Double-checked locking: another task may have refreshed while we waited.
        if let Some(entry) = self.cached(app_key).await {
            return Ok(entry);
        }

        let entry = self
            .fetch_with_retry(app_key, app_secret, api_domain, route_env)
            .await?;
        let mut cache = self.cache.lock().await;
        cache.insert(app_key.to_string(), entry.clone());
        Ok(entry)
    }

    async fn get_refresh_lock(&self, app_key: &str) -> Arc<Mutex<()>> {
        let mut locks = self.locks.lock().await;
        locks
            .entry(app_key.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    async fn fetch_with_retry(
        &self,
        app_key: &str,
        app_secret: &str,
        api_domain: &str,
        route_env: &str,
    ) -> Result<TokenEntry, YuanbaoError> {
        let url = format!("{}{}", api_domain.trim_end_matches('/'), SIGN_PATH);
        let mut last_err: Option<YuanbaoError> = None;

        for attempt in 0..=MAX_RETRIES {
            let nonce = generate_nonce();
            let timestamp = build_timestamp();
            let signature = compute_signature(&nonce, &timestamp, app_key, app_secret);
            let payload = serde_json::json!({
                "app_key": app_key,
                "nonce": nonce,
                "signature": signature,
                "timestamp": timestamp,
            });

            let mut req = self
                .http
                .post(&url)
                .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
                .header("Content-Type", "application/json")
                .header("X-AppVersion", super::config::DEFAULT_PLUGIN_VERSION)
                .header("X-OperationSystem", "linux")
                .header(
                    "X-Instance-Id",
                    super::proto_constants::OPENHUMAN_INSTANCE_ID,
                )
                .header("X-Bot-Version", env!("CARGO_PKG_VERSION"));
            if !route_env.is_empty() {
                req = req.header("X-Route-Env", route_env);
            }

            info!(
                "[yuanbao/sign] POST {}{}",
                url,
                if attempt > 0 {
                    format!(" (retry {attempt}/{MAX_RETRIES})")
                } else {
                    String::new()
                }
            );

            let resp = match req.json(&payload).send().await {
                Ok(r) => r,
                Err(e) => {
                    last_err = Some(YuanbaoError::Connection(format!("sign-token: {e}")));
                    if attempt < MAX_RETRIES {
                        tokio::time::sleep(Duration::from_millis(RETRY_DELAY_MS)).await;
                        continue;
                    }
                    break;
                }
            };

            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                return Err(YuanbaoError::AuthFailed(format!(
                    "sign-token HTTP {status}: {}",
                    body.chars().take(200).collect::<String>()
                )));
            }

            let json: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| YuanbaoError::AuthFailed(format!("sign-token body: {e}")))?;

            let code = json.get("code").and_then(|c| c.as_i64()).unwrap_or(0);
            if code == 0 {
                let data = match json.get("data") {
                    Some(d) if d.is_object() => d,
                    _ => {
                        return Err(YuanbaoError::AuthFailed(
                            "sign-token response missing 'data'".into(),
                        ));
                    }
                };
                let duration = data
                    .get("duration")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(DEFAULT_DURATION_SECS);
                let entry = TokenEntry {
                    token: data
                        .get("token")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    bot_id: data
                        .get("bot_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    product: data
                        .get("product")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    source: data
                        .get("source")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    expire_ts: unix_now() + duration,
                };
                info!(
                    "[yuanbao/sign] success: bot_id={} duration={}s",
                    entry.bot_id, duration
                );
                return Ok(entry);
            }

            if code == RETRYABLE_CODE && attempt < MAX_RETRIES {
                warn!(
                    "[yuanbao/sign] retryable code={code}, retrying in {RETRY_DELAY_MS}ms (attempt {})",
                    attempt + 1
                );
                tokio::time::sleep(Duration::from_millis(RETRY_DELAY_MS)).await;
                continue;
            }

            let msg = json
                .get("msg")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            return Err(YuanbaoError::AuthFailed(format!(
                "sign-token code={code} msg={msg}"
            )));
        }

        Err(last_err.unwrap_or(YuanbaoError::AuthFailed(
            "sign-token max retries exceeded".into(),
        )))
    }

    /// Drop all per-app_key locks. Called on channel shutdown to avoid
    /// leaking entries across reconnects within the same process.
    pub async fn clear_locks(&self) {
        let mut locks = self.locks.lock().await;
        locks.clear();
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn suffix(s: &str) -> &str {
    if s.len() <= 4 { s } else { &s[s.len() - 4..] }
}

#[cfg(test)]
#[path = "sign_tests.rs"]
mod tests;
