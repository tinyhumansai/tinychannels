use super::*;

#[test]
fn backoff_follows_schedule() {
    assert_eq!(backoff_seconds(1), 1);
    assert_eq!(backoff_seconds(2), 2);
    assert_eq!(backoff_seconds(3), 5);
    assert_eq!(backoff_seconds(6), 60);
    assert_eq!(backoff_seconds(100), 60);
    assert_eq!(backoff_seconds(0), 1);
}

fn cfg() -> YuanbaoConfig {
    let mut c = YuanbaoConfig::default();
    c.app_key = "ak".into();
    c.ws_domain = "wss://example".into();
    c.token = "tok".into();
    c.bot_id = "bot1".into();
    c
}

#[tokio::test]
async fn pending_correlator_times_out() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let conn = YuanbaoConnection::new(cfg(), tx, None);
    let err = conn
        .send_and_wait("missing_id", vec![1, 2, 3], Duration::from_millis(20))
        .await
        .unwrap_err();
    // Without a connected socket, send_frame fails first → SendFailed/NotConnected.
    assert!(matches!(
        err,
        YuanbaoError::NotConnected | YuanbaoError::Timeout(_) | YuanbaoError::SendFailed(_)
    ));
}

#[tokio::test]
async fn account_back_fill_picks_up_uid() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let conn = YuanbaoConnection::new(cfg(), tx, None);
    assert_eq!(conn.account().uid, "bot1");
    conn.update_account(|a| a.connect_id = "cid_xyz".into());
    assert_eq!(conn.account().connect_id, "cid_xyz");
}

#[tokio::test]
async fn next_msg_id_is_monotonic_and_prefixed() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let conn = YuanbaoConnection::new(cfg(), tx, None);
    let a = conn.next_msg_id("pfx");
    let b = conn.next_msg_id("pfx");
    assert!(a.starts_with("pfx_"));
    assert!(b.starts_with("pfx_"));
    // Suffix is monotonically increasing.
    let na: u64 = a.strip_prefix("pfx_").unwrap().parse().unwrap();
    let nb: u64 = b.strip_prefix("pfx_").unwrap().parse().unwrap();
    assert!(nb > na);
}

#[tokio::test]
async fn initial_state_is_disconnected() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let conn = YuanbaoConnection::new(cfg(), tx, None);
    assert_eq!(conn.state(), ConnectionState::Disconnected);
    assert!(!conn.is_connected());
}

#[tokio::test]
async fn set_state_connected_flips_is_connected_flag() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let conn = YuanbaoConnection::new(cfg(), tx, None);
    conn.set_state(ConnectionState::Connected);
    assert_eq!(conn.state(), ConnectionState::Connected);
    assert!(conn.is_connected());
    conn.set_state(ConnectionState::Reconnecting);
    assert_eq!(conn.state(), ConnectionState::Reconnecting);
    assert!(!conn.is_connected());
}

#[tokio::test]
async fn send_frame_without_socket_returns_not_connected() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let conn = YuanbaoConnection::new(cfg(), tx, None);
    let err = conn.send_frame(vec![1, 2, 3]).await.unwrap_err();
    assert!(matches!(err, YuanbaoError::NotConnected));
    let err2 = conn.send_conn_msg(vec![4]).await.unwrap_err();
    assert!(matches!(err2, YuanbaoError::NotConnected));
}

#[tokio::test]
async fn shutdown_clears_pending_and_sets_disconnected() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let conn = YuanbaoConnection::new(cfg(), tx, None);
    conn.set_state(ConnectionState::Connected);
    // Drop a phantom pending entry then shutdown.
    let (phantom_tx, _phantom_rx) = oneshot::channel();
    conn.pending.lock().insert("ghost".into(), phantom_tx);
    conn.shutdown().await;
    assert_eq!(conn.state(), ConnectionState::Disconnected);
    assert!(!conn.is_connected());
    assert!(conn.pending.lock().is_empty());
}

#[test]
fn backoff_caps_at_last_entry_for_huge_attempts() {
    let last = *RECONNECT_DELAYS.last().unwrap();
    assert_eq!(backoff_seconds(RECONNECT_DELAYS.len() as u32 + 5), last);
}

#[tokio::test]
async fn resolve_token_uses_static_token_when_present() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let conn = YuanbaoConnection::new(cfg(), tx, None);
    let (token, bot_id, source) = conn.resolve_token().await.unwrap();
    assert_eq!(token, "tok");
    assert_eq!(bot_id, "bot1");
    assert_eq!(source, "");
}

#[tokio::test]
async fn resolve_token_without_token_and_without_sign_manager_errors() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut c = cfg();
    c.token = String::new();
    let conn = YuanbaoConnection::new(c, tx, None);
    match conn.resolve_token().await.unwrap_err() {
        YuanbaoError::AuthFailed(m) => assert!(m.contains("no token"), "got {m}"),
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test]
async fn resolve_token_with_sign_manager_but_no_app_secret_errors() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut c = cfg();
    c.token = String::new();
    c.app_secret = String::new();
    let mgr = SignManager::new(reqwest::Client::new());
    let conn = YuanbaoConnection::new(c, tx, Some(mgr));
    match conn.resolve_token().await.unwrap_err() {
        YuanbaoError::AuthFailed(m) => assert!(m.contains("app_secret"), "got {m}"),
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test]
async fn send_auth_bind_without_socket_returns_not_connected() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let conn = YuanbaoConnection::new(cfg(), tx, None);
    let err = conn.send_auth_bind("tok", "bot1", "bot").await.unwrap_err();
    assert!(matches!(err, YuanbaoError::NotConnected));
}

#[tokio::test]
async fn send_auth_bind_falls_back_to_account_uid_when_bot_id_empty() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let conn = YuanbaoConnection::new(cfg(), tx, None);
    // bot_id="" → reads from account.uid (which was seeded from cfg.bot_id="bot1")
    let err = conn.send_auth_bind("tok", "", "").await.unwrap_err();
    assert!(matches!(err, YuanbaoError::NotConnected));
    // Account uid still in place.
    assert_eq!(conn.account().uid, "bot1");
}

#[test]
fn handle_auth_response_rejects_non_binary_message() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let conn = YuanbaoConnection::new(cfg(), tx, None);
    let msg = Message::Text("nope".into());
    match conn.handle_auth_response(&msg).unwrap_err() {
        YuanbaoError::AuthFailed(m) => {
            assert!(m.contains("binary"), "got {m}")
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn handle_auth_response_rejects_undecodable_binary() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let conn = YuanbaoConnection::new(cfg(), tx, None);
    // Wholly invalid wire data — decode_conn_msg fails.
    let msg = Message::Binary(vec![0xFF, 0xFF, 0xFF, 0xFF].into());
    let err = conn.handle_auth_response(&msg).unwrap_err();
    // Either Proto decode error or some other surface — must not be Ok.
    assert!(!matches!(err, YuanbaoError::AuthFailed(_) if format!("{err:?}").contains("binary")));
}

/// Regression guard for the post-`connect_once` shutdown short-circuit:
/// once shutdown is signaled, `run()` must not block on the reconnect
/// backoff. We force connect_once to fail synchronously (invalid WS URL),
/// then signal shutdown — total runtime must be well under the first
/// backoff slot (`backoff_seconds(1) == 1s`).
#[tokio::test]
async fn run_exits_promptly_after_shutdown_signal() {
    use std::time::Instant;
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut c = cfg();
    // tokio-tungstenite rejects the URL synchronously — connect_once
    // returns Err in microseconds, putting `run()` on the post-connect
    // cleanup path that the fix targets.
    c.ws_domain = "not-a-valid-ws-url".to_string();
    c.max_reconnect_attempts = 100;
    let conn = YuanbaoConnection::new(c, tx, None);
    let (sd_tx, sd_rx) = watch::channel(false);

    let handle = tokio::spawn(conn.clone().run(sd_rx));
    // Let `run()` enter the loop and attempt connect_once at least once.
    time::sleep(Duration::from_millis(20)).await;

    let started = Instant::now();
    sd_tx.send(true).unwrap();

    // The first reconnect backoff slot is 1s. Without responsive
    // shutdown handling, run() would sleep through it before checking
    // the flag. 500ms gives us comfortable headroom while staying
    // far enough below the backoff to detect a regression.
    let res = time::timeout(Duration::from_millis(500), handle).await;
    res.expect("run() did not exit within 500ms of shutdown signal")
        .expect("run() task panicked");
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "run() took {:?} to exit after shutdown — backoff was not skipped",
        started.elapsed()
    );
}

#[tokio::test]
async fn handle_binary_with_garbage_does_not_panic() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let conn = YuanbaoConnection::new(cfg(), tx, None);
    // Should silently log + return — no panic.
    conn.handle_binary(vec![0xFF, 0xFF, 0xFF, 0xFF]).await;
}
