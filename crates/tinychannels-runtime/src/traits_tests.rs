use super::*;

struct DummyChannel;

#[async_trait]
impl Channel for DummyChannel {
    fn name(&self) -> &str {
        "dummy"
    }

    async fn send(&self, _message: &SendMessage) -> anyhow::Result<()> {
        Ok(())
    }

    async fn listen(&self, tx: tokio::sync::mpsc::Sender<ChannelMessage>) -> anyhow::Result<()> {
        tx.send(ChannelMessage {
            id: "1".into(),
            sender: "tester".into(),
            reply_target: "tester".into(),
            content: "hello".into(),
            channel: "dummy".into(),
            timestamp: 123,
            thread_ts: None,
            sender_name: None,
        })
        .await
        .map_err(|e| anyhow::anyhow!(e.to_string()))
    }
}

#[test]
fn channel_message_clone_preserves_fields() {
    let message = ChannelMessage {
        id: "42".into(),
        sender: "alice".into(),
        reply_target: "alice".into(),
        content: "ping".into(),
        channel: "dummy".into(),
        timestamp: 999,
        thread_ts: None,
        sender_name: None,
    };

    let cloned = message.clone();
    assert_eq!(cloned.id, "42");
    assert_eq!(cloned.sender, "alice");
    assert_eq!(cloned.reply_target, "alice");
    assert_eq!(cloned.content, "ping");
    assert_eq!(cloned.channel, "dummy");
    assert_eq!(cloned.timestamp, 999);
}

#[test]
fn send_message_generates_deterministic_idempotency_key() {
    let message = SendMessage::new("hello", "alice")
        .in_thread(Some("thread-1".to_string()))
        .with_deterministic_idempotency_key("telegram");
    let again = SendMessage::new("hello", "alice")
        .in_thread(Some("thread-1".to_string()))
        .with_deterministic_idempotency_key("telegram");

    assert_eq!(message.idempotency_key, again.idempotency_key);
    assert!(
        message
            .idempotency_key
            .as_deref()
            .unwrap()
            .starts_with("legacy-send:telegram:")
    );
}

#[test]
fn explicit_send_message_idempotency_key_is_preserved() {
    let message = SendMessage::new("hello", "alice")
        .with_idempotency_key("caller-key")
        .with_deterministic_idempotency_key("telegram");

    assert_eq!(message.idempotency_key.as_deref(), Some("caller-key"));
}

#[tokio::test]
async fn default_trait_methods_return_success() {
    let channel = DummyChannel;

    assert!(channel.health_check().await);
    assert!(channel.start_typing("bob").await.is_ok());
    assert!(channel.stop_typing("bob").await.is_ok());
    assert!(
        channel
            .send(&SendMessage::new("hello", "bob"))
            .await
            .is_ok()
    );
    // A provider that does not override `proactive_target` opts out of
    // recipient-less proactive delivery (#3794).
    assert_eq!(channel.proactive_target(), None);
}

#[tokio::test]
async fn default_draft_methods_return_success() {
    let channel = DummyChannel;

    assert!(!channel.supports_draft_updates());
    assert!(
        channel
            .send_draft(&SendMessage::new("draft", "bob"))
            .await
            .unwrap()
            .is_none()
    );
    assert!(channel.update_draft("bob", "msg_1", "text").await.is_ok());
    assert!(
        channel
            .finalize_draft("bob", "msg_1", "final text", None)
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn listen_sends_message_to_channel() {
    let channel = DummyChannel;
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);

    channel.listen(tx).await.unwrap();

    let received = rx.recv().await.expect("message should be sent");
    assert_eq!(received.sender, "tester");
    assert_eq!(received.content, "hello");
    assert_eq!(received.channel, "dummy");
}
