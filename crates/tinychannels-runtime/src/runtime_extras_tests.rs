// Scoped allowance for the one-release v21 wire DTO compatibility surface.
#![allow(deprecated)]

//! Tests for the session, dispatch and health helpers.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

fn message(id: &str) -> tinychannels_bus::ChannelMessage {
    tinychannels_bus::ChannelMessage {
        id: id.into(),
        sender: "alice".into(),
        reply_target: "chat".into(),
        content: "hi".into(),
        channel: "test".into(),
        timestamp: 0,
        thread_ts: None,
        topic_id: None,
        reply_to_id: None,
        sender_name: None,
    }
}

#[tokio::test]
async fn invalidate_cancels_current_and_starts_fresh_session() {
    let session = ChannelSession::new();
    let first = session.current();
    session.invalidate();
    assert!(first.is_cancelled());
    assert!(!session.current().is_cancelled());
}

#[tokio::test]
async fn run_in_session_stops_cleanly_on_cancel() {
    let session = ChannelSession::new();
    let token = session.current();
    session.invalidate();
    let result = run_in_session(token, std::future::pending()).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn run_in_session_returns_runtime_result() {
    let token = ChannelSession::new().current();
    let result = run_in_session(token, async { Err(anyhow::anyhow!("boom")) }).await;
    assert_eq!(result.unwrap_err().to_string(), "boom");
}

#[tokio::test]
async fn dispatch_loop_runs_every_message_and_bounds_concurrency() {
    let (tx, rx) = tokio::sync::mpsc::channel(8);
    let seen = Arc::new(AtomicUsize::new(0));
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    for i in 0..6 {
        tx.send(RuntimeChannelMessage::from(message(&i.to_string())))
            .await
            .unwrap();
    }
    drop(tx);
    let (seen2, active2, peak2) = (seen.clone(), active.clone(), peak.clone());
    run_dispatch_loop(rx, 2, move |_msg: RuntimeChannelMessage| {
        let (seen, active, peak) = (seen2.clone(), active2.clone(), peak2.clone());
        async move {
            let now = active.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(now, Ordering::SeqCst);
            tokio::task::yield_now().await;
            active.fetch_sub(1, Ordering::SeqCst);
            seen.fetch_add(1, Ordering::SeqCst);
        }
    })
    .await;
    assert_eq!(seen.load(Ordering::SeqCst), 6);
    assert!(peak.load(Ordering::SeqCst) <= 2);
}

#[tokio::test]
async fn dispatch_loop_survives_a_panicking_handler() {
    let (tx, rx) = tokio::sync::mpsc::channel(4);
    tx.send(1u32).await.unwrap();
    tx.send(2u32).await.unwrap();
    drop(tx);
    let seen = Arc::new(AtomicUsize::new(0));
    let seen2 = seen.clone();
    run_dispatch_loop(rx, 1, move |n| {
        let seen = seen2.clone();
        async move {
            if n == 1 {
                panic!("handler failure");
            }
            seen.fetch_add(1, Ordering::SeqCst);
        }
    })
    .await;
    assert_eq!(seen.load(Ordering::SeqCst), 1);
}

#[test]
fn runtime_message_keeps_envelope_only_when_given() {
    let plain = RuntimeChannelMessage::from(message("1"));
    assert!(plain.inbound_envelope.is_none());
    let envelope = crate::channel::inbound_envelope_from_legacy_message(&message("2"));
    let relayed = RuntimeChannelMessage::with_inbound_envelope(message("2"), envelope);
    assert!(relayed.inbound_envelope.is_some());
}
