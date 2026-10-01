//! Approval surface against recording channels.

use std::sync::Mutex as StdMutex;

use async_trait::async_trait;

use super::*;
use crate::traits::ChannelMessage;

struct RecordingChannel {
    name: String,
    fail: bool,
    sent: StdMutex<Vec<SendMessage>>,
}

impl RecordingChannel {
    fn new(name: &str) -> Arc<Self> {
        Arc::new(Self {
            name: name.into(),
            fail: false,
            sent: StdMutex::new(Vec::new()),
        })
    }
    fn sent(&self) -> Vec<SendMessage> {
        self.sent.lock().unwrap().clone()
    }
}

#[async_trait]
impl Channel for RecordingChannel {
    fn name(&self) -> &str {
        &self.name
    }
    async fn send(&self, message: &SendMessage) -> anyhow::Result<()> {
        if self.fail {
            anyhow::bail!("send refused");
        }
        self.sent.lock().unwrap().push(message.clone());
        Ok(())
    }
    async fn listen(&self, _tx: tokio::sync::mpsc::Sender<ChannelMessage>) -> anyhow::Result<()> {
        Ok(())
    }
}

fn surface(channels: &[Arc<RecordingChannel>]) -> ApprovalSurface {
    let map: HashMap<String, Arc<dyn Channel>> = channels
        .iter()
        .map(|c| (c.name.clone(), Arc::clone(c) as Arc<dyn Channel>))
        .collect();
    ApprovalSurface::new(Arc::new(map))
}

fn prompt(channel: &str, thread_id: &str) -> ApprovalPrompt {
    ApprovalPrompt {
        request_id: "req-1".into(),
        tool_name: "file_write".into(),
        action_summary: "Write notes/today.md (1.2 KiB)".into(),
        thread_id: thread_id.into(),
        channel: channel.into(),
    }
}

#[test]
fn approval_prompt_includes_action_and_reply_instructions() {
    let body = format_approval_prompt("git_operations", "git commit -m fix");
    assert!(body.contains("git_operations") && body.contains("git commit"));
    assert!(body.contains("yes") && body.contains("no"));
}

#[tokio::test]
async fn every_chat_provider_gets_its_prompt_in_the_originating_chat() {
    for (channel, thread_ts, key) in [
        ("telegram", Some("topic-1"), "telegram_alice_chat-42"),
        ("discord", None, "discord_alice_chat-42"),
        ("slack", Some("1700.1"), "slack_alice_chat-42_thread:1700.1"),
    ] {
        let ch = RecordingChannel::new(channel);
        let s = surface(&[Arc::clone(&ch)]);
        s.record_inbound(channel, "alice", "chat-42", thread_ts);
        assert!(
            matches!(s.surface(&prompt(channel, key)).await, SurfaceOutcome::Sent),
            "{channel}"
        );
        let sent = ch.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].recipient, "chat-42");
        assert_eq!(sent[0].thread_ts.as_deref(), thread_ts);
        assert!(sent[0].content.contains("file_write"));
    }
}

#[tokio::test]
async fn telegram_topics_share_one_conversation_key() {
    let ch = RecordingChannel::new("telegram");
    let s = surface(&[Arc::clone(&ch)]);
    s.record_inbound("telegram", "alice", "chat-1", Some("topic-9"));
    let ctx = s.reply_context("telegram_alice_chat-1").expect("recorded");
    assert_eq!(ctx.thread_ts.as_deref(), Some("topic-9"));
}

#[tokio::test]
async fn channels_without_chat_approvals_are_ignored() {
    let ch = RecordingChannel::new("email");
    let s = surface(&[Arc::clone(&ch)]);
    s.record_inbound("email", "a@b.c", "a@b.c", None);
    assert!(s.reply_context("email_a@b.c_a@b.c").is_none());
    assert!(matches!(
        s.surface(&prompt("email", "email_a@b.c_a@b.c")).await,
        SurfaceOutcome::Unsupported
    ));
    assert!(ch.sent().is_empty());
}

#[tokio::test]
async fn missing_context_or_channel_is_reported_not_sent() {
    let ch = RecordingChannel::new("discord");
    let s = surface(&[Arc::clone(&ch)]);
    assert!(matches!(
        s.surface(&prompt("discord", "discord_bob_c")).await,
        SurfaceOutcome::NoReplyContext
    ));
    s.record_inbound("slack", "bob", "c", None);
    assert!(matches!(
        s.surface(&prompt("slack", "slack_bob_c")).await,
        SurfaceOutcome::ChannelNotRegistered
    ));
    assert!(ch.sent().is_empty());
}

#[tokio::test]
async fn send_failure_is_returned() {
    let ch = Arc::new(RecordingChannel {
        name: "signal".into(),
        fail: true,
        sent: StdMutex::new(Vec::new()),
    });
    let s = surface(&[Arc::clone(&ch)]);
    s.record_inbound("signal", "bob", "c", None);
    match s.surface(&prompt("signal", "signal_bob_c")).await {
        SurfaceOutcome::SendFailed(err) => assert_eq!(err.to_string(), "send refused"),
        other => panic!("unexpected {other:?}"),
    }
}
