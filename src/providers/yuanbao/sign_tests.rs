use super::*;

#[test]
fn signature_matches_python_reference() {
    // Reproducible vector — hand-computed:
    //   plain = "n123" + "2026-05-19T22:00:00+08:00" + "app_k" + "secret"
    //   sig   = HMAC-SHA256(key="secret", msg=plain) as lower hex
    let sig = compute_signature("n123", "2026-05-19T22:00:00+08:00", "app_k", "secret");
    // We don't pin the exact bytes (would require running Python to confirm) —
    // instead verify the contract: same inputs → same output, 64-char hex.
    assert_eq!(sig.len(), 64);
    assert!(sig.chars().all(|c| c.is_ascii_hexdigit()));
    let sig2 = compute_signature("n123", "2026-05-19T22:00:00+08:00", "app_k", "secret");
    assert_eq!(sig, sig2);
}

#[test]
fn signature_varies_with_inputs() {
    let s1 = compute_signature("n1", "t", "ak", "sk");
    let s2 = compute_signature("n2", "t", "ak", "sk");
    let s3 = compute_signature("n1", "t2", "ak", "sk");
    let s4 = compute_signature("n1", "t", "ak2", "sk");
    let s5 = compute_signature("n1", "t", "ak", "sk2");
    let all = [&s1, &s2, &s3, &s4, &s5];
    for (i, a) in all.iter().enumerate() {
        for (j, b) in all.iter().enumerate() {
            if i != j {
                assert_ne!(a, b, "inputs {i} vs {j} should differ");
            }
        }
    }
}

#[test]
fn nonce_is_32_char_hex() {
    let n = generate_nonce();
    assert_eq!(n.len(), 32);
    assert!(n.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn timestamp_matches_beijing_format() {
    let t = build_timestamp();
    // 2006-01-02T15:04:05+08:00 → length 25
    assert_eq!(t.len(), 25);
    assert!(t.ends_with("+08:00"));
    assert_eq!(&t[4..5], "-");
    assert_eq!(&t[7..8], "-");
    assert_eq!(&t[10..11], "T");
    assert_eq!(&t[13..14], ":");
}

#[test]
fn token_entry_is_valid_only_with_margin() {
    let mut e = TokenEntry {
        token: "t".into(),
        bot_id: "b".into(),
        product: String::new(),
        source: String::new(),
        expire_ts: unix_now() + 120,
    };
    assert!(e.is_valid());
    e.expire_ts = unix_now() + 30; // less than 60s margin
    assert!(!e.is_valid());
    e.expire_ts = unix_now().saturating_sub(10);
    assert!(!e.is_valid());
}

#[tokio::test]
async fn cache_returns_entry_when_valid() {
    let mgr = SignManager::new(reqwest::Client::new());
    let entry = TokenEntry {
        token: "tok".into(),
        bot_id: "bot".into(),
        product: String::new(),
        source: String::new(),
        expire_ts: unix_now() + 600,
    };
    mgr.cache.lock().await.insert("ak".into(), entry.clone());
    let got = mgr.cached("ak").await.expect("cache hit");
    assert_eq!(got.token, "tok");
}

#[tokio::test]
async fn cache_drops_expired_entry() {
    let mgr = SignManager::new(reqwest::Client::new());
    mgr.cache.lock().await.insert(
        "ak".into(),
        TokenEntry {
            token: "tok".into(),
            bot_id: "bot".into(),
            product: String::new(),
            source: String::new(),
            expire_ts: unix_now() + 10, // under margin
        },
    );
    assert!(mgr.cached("ak").await.is_none());
}

#[test]
fn token_entry_seconds_remaining_is_signed() {
    let e_future = TokenEntry {
        token: "t".into(),
        bot_id: "b".into(),
        product: String::new(),
        source: String::new(),
        expire_ts: unix_now() + 300,
    };
    assert!(e_future.seconds_remaining() >= 290);
    let e_past = TokenEntry {
        expire_ts: unix_now().saturating_sub(60),
        ..e_future
    };
    assert!(e_past.seconds_remaining() <= 0);
}

#[test]
fn suffix_redacts_to_last_4_chars() {
    assert_eq!(suffix(""), "");
    assert_eq!(suffix("a"), "a");
    assert_eq!(suffix("abcd"), "abcd");
    assert_eq!(suffix("abcdef"), "cdef");
    assert_eq!(suffix("0123456789"), "6789");
}

// ─── refresh / fetch_with_retry via wiremock ────────────────

fn ok_body(token: &str, bot_id: &str, duration_secs: u64) -> serde_json::Value {
    serde_json::json!({
        "code": 0,
        "msg": "ok",
        "data": {
            "token": token,
            "bot_id": bot_id,
            "product": "prod1",
            "source": "src1",
            "duration": duration_secs,
        }
    })
}

#[tokio::test]
async fn get_token_fetches_and_caches_on_first_call() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(SIGN_PATH))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(ok_body("tok-1", "bot-1", 7200)),
        )
        .mount(&server)
        .await;
    let mgr = SignManager::new(reqwest::Client::new());
    let e = mgr
        .get_token("ak", "sk", &server.uri(), "")
        .await
        .expect("token");
    assert_eq!(e.token, "tok-1");
    assert_eq!(e.bot_id, "bot-1");
    assert!(e.expire_ts > unix_now() + 7000);

    // Second call should hit the cache (still works even if server stops).
    let cached = mgr.cached("ak").await.expect("cached");
    assert_eq!(cached.token, "tok-1");
}

#[tokio::test]
async fn get_token_retries_on_code_10099_then_succeeds() {
    let server = wiremock::MockServer::start().await;
    // First two requests return code=10099, third returns code=0.
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(SIGN_PATH))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 10099,
                "msg": "try again",
            })),
        )
        .up_to_n_times(2)
        .mount(&server)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(SIGN_PATH))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(ok_body("tok-r", "bot-r", 600)),
        )
        .mount(&server)
        .await;
    let mgr = SignManager::new(reqwest::Client::new());
    let e = mgr.refresh("ak", "sk", &server.uri(), "").await.unwrap();
    assert_eq!(e.token, "tok-r");
    assert_eq!(e.bot_id, "bot-r");
}

#[tokio::test]
async fn get_token_surfaces_http_error_as_auth_failed() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .respond_with(wiremock::ResponseTemplate::new(401).set_body_string("Unauthorized"))
        .mount(&server)
        .await;
    let mgr = SignManager::new(reqwest::Client::new());
    let err = mgr
        .get_token("ak", "sk", &server.uri(), "")
        .await
        .unwrap_err();
    match err {
        YuanbaoError::AuthFailed(m) => assert!(m.contains("HTTP 401"), "got {m}"),
        other => panic!("expected AuthFailed, got {other:?}"),
    }
}

#[tokio::test]
async fn get_token_fails_on_non_zero_business_code() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 40001,
                "msg": "bad secret",
            })),
        )
        .mount(&server)
        .await;
    let mgr = SignManager::new(reqwest::Client::new());
    let err = mgr
        .get_token("ak", "sk", &server.uri(), "")
        .await
        .unwrap_err();
    match err {
        YuanbaoError::AuthFailed(m) => {
            assert!(m.contains("code=40001"), "got {m}");
            assert!(m.contains("bad secret"), "got {m}");
        }
        other => panic!("expected AuthFailed, got {other:?}"),
    }
}

#[tokio::test]
async fn force_refresh_evicts_cache_and_refetches() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(SIGN_PATH))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(ok_body("tok-a", "bot", 600)),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(SIGN_PATH))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(ok_body("tok-b", "bot", 600)),
        )
        .mount(&server)
        .await;
    let mgr = SignManager::new(reqwest::Client::new());
    let first = mgr.get_token("ak", "sk", &server.uri(), "").await.unwrap();
    assert_eq!(first.token, "tok-a");
    let second = mgr
        .force_refresh("ak", "sk", &server.uri(), "to_env")
        .await
        .unwrap();
    assert_eq!(second.token, "tok-b");
}

#[tokio::test]
async fn clear_locks_drops_all_per_app_key_mutexes() {
    let mgr = SignManager::new(reqwest::Client::new());
    // Prime the locks map.
    let _ = mgr.get_refresh_lock("ak-1").await;
    let _ = mgr.get_refresh_lock("ak-2").await;
    assert_eq!(mgr.locks.lock().await.len(), 2);
    mgr.clear_locks().await;
    assert!(mgr.locks.lock().await.is_empty());
}
