use crate::traits::Channel;

use super::*;

fn good_cfg() -> YuanbaoConfig {
    let mut c = YuanbaoConfig::default();
    c.app_key = "ak".into();
    c.ws_domain = "wss://example".into();
    c.token = "tok".into();
    c
}

#[test]
fn channel_construction_validates() {
    let ch = YuanbaoChannel::new(good_cfg()).unwrap();
    assert_eq!(ch.name(), "yuanbao");
}

#[test]
fn invalid_config_rejected() {
    let mut c = YuanbaoConfig::default();
    c.app_key = "ak".into();
    // missing ws_domain
    assert!(YuanbaoChannel::new(c).is_err());
}

#[test]
fn split_short_message_returns_one() {
    let ch = YuanbaoChannel::new(good_cfg()).unwrap();
    let chunks = ch.split_message("hello");
    assert_eq!(chunks, vec!["hello"]);
}

#[test]
fn split_respects_newlines() {
    let mut c = good_cfg();
    c.max_message_length = 12;
    let ch = YuanbaoChannel::new(c).unwrap();
    let chunks = ch.split_message("line one\nline two\nline three");
    assert!(chunks.len() >= 2);
    // No chunk exceeds the limit.
    for chunk in &chunks {
        assert!(chunk.len() <= 12, "chunk too long: {chunk:?}");
    }
}

#[tokio::test]
async fn resolve_recipient_returns_input_when_no_alias_registered() {
    let ch = YuanbaoChannel::new(good_cfg()).unwrap();
    assert_eq!(ch.resolve_recipient("short_uid").await, "short_uid");
}

#[tokio::test]
async fn register_and_resolve_dm_alias_recovers_original_uid() {
    let ch = YuanbaoChannel::new(good_cfg()).unwrap();
    let original = "x".repeat(64);
    let shortened = shorten_account_id(&original);
    assert_ne!(shortened, original, "test premise: should actually shorten");
    ch.register_recipient_alias(&shortened, &original).await;
    assert_eq!(ch.resolve_recipient(&shortened).await, original);
}

#[tokio::test]
async fn register_recipient_alias_is_noop_for_equal_pair() {
    let ch = YuanbaoChannel::new(good_cfg()).unwrap();
    // Short uid that wouldn't be shortened — caller still hands us
    // (s, s); we should silently skip and not eat a map slot.
    ch.register_recipient_alias("short", "short").await;
    let m = ch.recipient_aliases.lock().await;
    assert!(m.is_empty());
}

#[tokio::test]
async fn resolve_recipient_preserves_group_prefix_via_alias() {
    let ch = YuanbaoChannel::new(good_cfg()).unwrap();
    let long_group_code = "g".repeat(64);
    let original = format!("g:{long_group_code}");
    let shortened = shorten_reply_target(&original);
    assert_ne!(shortened, original);
    assert!(shortened.starts_with("g:"));
    ch.register_recipient_alias(&shortened, &original).await;
    assert_eq!(ch.resolve_recipient(&shortened).await, original);
}

#[tokio::test]
async fn alias_map_clears_when_cap_is_hit() {
    let ch = YuanbaoChannel::new(good_cfg()).unwrap();
    // Pre-fill up to the cap with distinct entries.
    for i in 0..RECIPIENT_ALIAS_CAP {
        ch.register_recipient_alias(&format!("s{i}"), &format!("o{i}"))
            .await;
    }
    assert_eq!(ch.recipient_aliases.lock().await.len(), RECIPIENT_ALIAS_CAP);
    // One more entry must trigger a clear, then insert the new entry.
    ch.register_recipient_alias("new_short", "new_original")
        .await;
    let m = ch.recipient_aliases.lock().await;
    assert_eq!(m.len(), 1);
    assert_eq!(m.get("new_short").map(String::as_str), Some("new_original"));
}

// ─── trivial trait methods ─────────────────────────────────────

#[test]
fn supports_draft_updates_is_true() {
    let ch = YuanbaoChannel::new(good_cfg()).unwrap();
    assert!(ch.supports_draft_updates());
}

#[test]
fn supports_reactions_is_false() {
    let ch = YuanbaoChannel::new(good_cfg()).unwrap();
    assert!(!ch.supports_reactions());
}

#[tokio::test]
async fn send_draft_returns_marker_id() {
    let ch = YuanbaoChannel::new(good_cfg()).unwrap();
    let msg = SendMessage::new("ignored", "user-42");
    let id = ch.send_draft(&msg).await.unwrap();
    assert_eq!(id.as_deref(), Some("yb-draft:user-42"));
}

#[tokio::test]
async fn update_draft_is_a_noop_ok() {
    let ch = YuanbaoChannel::new(good_cfg()).unwrap();
    assert!(ch.update_draft("user-42", "any-id", "text").await.is_ok());
}

#[tokio::test]
async fn health_check_is_false_when_socket_not_connected() {
    // Real connect requires a WebSocket; we only verify the
    // disconnected default here. The connected branch is exercised
    // by `connection::tests::set_state_connected_flips_is_connected_flag`.
    let ch = YuanbaoChannel::new(good_cfg()).unwrap();
    assert!(!ch.health_check().await);
}

// ─── dispatch_push branches ────────────────────────────────────

fn make_push_frame(cmd: &str, data: Vec<u8>) -> types::ConnFrame {
    types::ConnFrame {
        cmd_type: super::super::proto_constants::cmd_type::PUSH,
        cmd: cmd.into(),
        module: "yuanbao_openclaw_proxy".into(),
        seq_no: 0,
        msg_id: String::new(),
        need_ack: false,
        status: 0,
        data,
    }
}

#[tokio::test]
async fn dispatch_push_empty_body_is_skipped() {
    let ch = YuanbaoChannel::new(good_cfg()).unwrap();
    let (tx, mut rx) = mpsc::channel::<ChannelMessage>(4);
    let frame = make_push_frame("noop", Vec::new());
    ch.dispatch_push(frame, &tx).await;
    // No message should reach the sender.
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn dispatch_push_garbage_body_does_not_dispatch() {
    // Body is not a valid protobuf push *and* not valid JSON → Failed.
    // dispatch_push should log + swallow, not propagate panic.
    let ch = YuanbaoChannel::new(good_cfg()).unwrap();
    let (tx, mut rx) = mpsc::channel::<ChannelMessage>(4);
    let frame = make_push_frame("inbound_message", vec![0xFF, 0xFF, 0xFF, 0xFF]);
    ch.dispatch_push(frame, &tx).await;
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn dispatch_push_dm_text_reaches_listener() {
    // Build a minimal `InboundMessagePush` directly in ConnFrame.data
    // (no PushMsg envelope), with a single TIMTextElem so the pipeline
    // dispatches.
    use super::super::proto::{encode_msg_body_element, encode_varint};
    let elem = types::MsgBodyElement {
        msg_type: "TIMTextElem".into(),
        msg_content: types::MsgContent {
            text: Some("hello".into()),
            ..Default::default()
        },
    };
    let elem_bytes = encode_msg_body_element(&elem);

    // Hand-roll an InboundMessagePush so we don't depend on a helper:
    // field 2 = from_account, field 3 = to_account, field 12 = msg_id,
    // field 13 = repeated MsgBodyElement.
    let mut biz = Vec::new();
    let put_string = |fnum: u32, s: &str, b: &mut Vec<u8>| {
        encode_varint(((fnum as u64) << 3) | 2, b);
        encode_varint(s.len() as u64, b);
        b.extend_from_slice(s.as_bytes());
    };
    put_string(2, "alice", &mut biz);
    put_string(3, "bot1", &mut biz);
    put_string(12, "mid-x", &mut biz);
    encode_varint(((13u64) << 3) | 2, &mut biz);
    encode_varint(elem_bytes.len() as u64, &mut biz);
    biz.extend_from_slice(&elem_bytes);

    // Disable group_at_required and use open dm_access so the
    // pipeline passes all stages for this DM.
    let mut cfg = good_cfg();
    cfg.dm_access = "open".into();
    cfg.bot_id = "bot1".into();
    let ch = YuanbaoChannel::new(cfg).unwrap();

    let frame = make_push_frame("inbound_message", biz);
    let (tx, mut rx) = mpsc::channel::<ChannelMessage>(4);
    ch.dispatch_push(frame, &tx).await;
    let msg = rx.try_recv().expect("dispatch should produce one message");
    assert_eq!(msg.id, "mid-x");
    assert_eq!(msg.content, "hello");
    assert_eq!(msg.channel, "yuanbao");
}

#[tokio::test]
async fn dispatch_push_filtered_by_dedup_does_not_double_dispatch() {
    use super::super::proto::{encode_msg_body_element, encode_varint};
    let elem = types::MsgBodyElement {
        msg_type: "TIMTextElem".into(),
        msg_content: types::MsgContent {
            text: Some("dup".into()),
            ..Default::default()
        },
    };
    let elem_bytes = encode_msg_body_element(&elem);
    let mut biz = Vec::new();
    let put_string = |fnum: u32, s: &str, b: &mut Vec<u8>| {
        encode_varint(((fnum as u64) << 3) | 2, b);
        encode_varint(s.len() as u64, b);
        b.extend_from_slice(s.as_bytes());
    };
    put_string(2, "alice", &mut biz);
    put_string(3, "bot1", &mut biz);
    put_string(12, "dup-id", &mut biz);
    encode_varint(((13u64) << 3) | 2, &mut biz);
    encode_varint(elem_bytes.len() as u64, &mut biz);
    biz.extend_from_slice(&elem_bytes);

    let mut cfg = good_cfg();
    cfg.dm_access = "open".into();
    cfg.bot_id = "bot1".into();
    let ch = YuanbaoChannel::new(cfg).unwrap();
    let (tx, mut rx) = mpsc::channel::<ChannelMessage>(4);
    ch.dispatch_push(make_push_frame("inbound_message", biz.clone()), &tx)
        .await;
    assert!(rx.try_recv().is_ok(), "first should dispatch");
    ch.dispatch_push(make_push_frame("inbound_message", biz), &tx)
        .await;
    assert!(rx.try_recv().is_err(), "second (same id) should dedup");
}

// ─── heartbeat task lifecycle ──────────────────────────────────

#[tokio::test]
async fn start_heartbeat_task_inserts_and_stop_removes() {
    let ch = YuanbaoChannel::new(good_cfg()).unwrap();
    ch.start_heartbeat_task("recipient-1").await;
    assert!(
        ch.heartbeat_tasks.lock().await.contains_key("recipient-1"),
        "should have spawned a task for recipient-1"
    );
    // Second start for same recipient is a no-op (does not double-spawn).
    ch.start_heartbeat_task("recipient-1").await;
    assert_eq!(ch.heartbeat_tasks.lock().await.len(), 1);

    ch.stop_heartbeat_task("recipient-1").await;
    assert!(ch.heartbeat_tasks.lock().await.is_empty());
    // Stopping a recipient with no task is also a no-op.
    ch.stop_heartbeat_task("never-started").await;
}
