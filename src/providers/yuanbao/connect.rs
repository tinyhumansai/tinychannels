//! Validating and verifying Yuanbao credentials from a connect form.

use serde_json::Value;
use tinychannels_runtime::config::YuanbaoConfigExt as _;

use super::YuanbaoConfig;
use super::sign::SignManager;

/// Read a required non-empty Yuanbao credential field from the connect-channel
/// payload. Returns the trimmed value or an error naming the missing field.
pub fn require_yuanbao_field(
    creds_map: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<String, String> {
    creds_map
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("missing required {key}"))
}

/// Build the **effective** Yuanbao config used for both preflight
/// verification and persistence.
///
/// Starts from `base` (so hand-installed deployments keep custom routes),
/// overlays the client-supplied endpoint overrides (`env`, `api_domain`,
/// `ws_domain`, `route_env`), then calls `apply_env_defaults` so verification
/// hits the right cluster: `env = "pre"` is verified against the pre-release
/// sign-token endpoint, not the production one.
///
/// `app_secret` is left empty on purpose. A host keeps the secret in its
/// credential store and hydrates it at startup, never in the config file.
pub fn build_effective_yuanbao_config(
    base: YuanbaoConfig,
    creds_map: &serde_json::Map<String, Value>,
    app_key: String,
) -> YuanbaoConfig {
    let opt_string = |key: &str| -> Option<String> {
        creds_map
            .get(key)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
    };

    let mut cfg = base;
    cfg.app_key = app_key;
    cfg.app_secret = String::new();
    if let Some(env) = opt_string("env") {
        cfg.env = env;
    }
    if let Some(api_domain) = opt_string("api_domain") {
        cfg.api_domain = api_domain;
    }
    if let Some(ws_domain) = opt_string("ws_domain") {
        cfg.ws_domain = ws_domain;
    }
    if let Some(route_env) = opt_string("route_env") {
        cfg.route_env = route_env;
    }
    cfg.apply_env_defaults();
    cfg
}

/// Verify Yuanbao credentials against the `sign-token` endpoint of the
/// effective config, so an invalid `app_key`/`app_secret` surfaces the API
/// error before anything is persisted.
pub async fn verify_yuanbao_credentials(
    http: reqwest::Client,
    yb_cfg: &YuanbaoConfig,
    app_secret: &str,
) -> Result<(), String> {
    SignManager::new(http)
        .get_token(
            &yb_cfg.app_key,
            app_secret,
            &yb_cfg.api_domain,
            &yb_cfg.route_env,
        )
        .await
        .map_err(|e| format!("yuanbao credential verification failed: {e}"))?;
    Ok(())
}

#[cfg(test)]
#[path = "connect_tests.rs"]
mod tests;
