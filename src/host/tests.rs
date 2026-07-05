//! Tests for the host capability boundary.

use super::*;
use crate::harness::types::{ChannelOutputEvent, ChannelTurn};
use async_trait::async_trait;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

// --- Mocks ----------------------------------------------------------------

struct MockDispatcher;

#[async_trait]
impl TurnDispatcher for MockDispatcher {
    async fn dispatch(&self, request: DispatchRequest) -> anyhow::Result<TurnHandle> {
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        let echoed = request.turn.envelope.message_id.clone();
        tokio::spawn(async move {
            let _ = tx
                .send(ChannelOutputEvent::TextDelta {
                    text: "thinking".into(),
                })
                .await;
            let _ = tx
                .send(ChannelOutputEvent::FinalMessage {
                    text: format!("done:{echoed}"),
                })
                .await;
        });
        Ok(TurnHandle::new("run-1", rx, Arc::new(|| {})))
    }
}

struct MockTranscriber;

#[async_trait]
impl Transcriber for MockTranscriber {
    fn name(&self) -> &str {
        "mock"
    }
    async fn transcribe(
        &self,
        request: TranscriptionRequest,
    ) -> anyhow::Result<TranscriptionResult> {
        Ok(TranscriptionResult {
            text: format!("heard:{}", request.audio_base64.len()),
            language: request.language,
            duration_secs: Some(1.0),
        })
    }
}

struct YesApprovals;

#[async_trait]
impl ApprovalGate for YesApprovals {
    async fn request(&self, _ask: ApprovalAsk) -> anyhow::Result<ApprovalDecision> {
        Ok(ApprovalDecision::Approve)
    }
    fn parse_reply(&self, message: &str) -> Option<ApprovalDecision> {
        match message.trim().to_ascii_lowercase().as_str() {
            "yes" | "y" | "approve" => Some(ApprovalDecision::Approve),
            "no" | "n" | "deny" => Some(ApprovalDecision::Deny),
            _ => None,
        }
    }
}

// --- NoopHost -------------------------------------------------------------

#[test]
fn noop_host_advertises_nothing() {
    let host = NoopHost;
    assert!(host.dispatcher().is_none());
    assert!(host.transcriber().is_none());
    assert!(host.approvals().is_none());
    assert!(host.ledger().is_none());
    assert!(host.capabilities().is_lean());
    assert_eq!(host.capabilities(), HostCapabilities::NONE);
}

#[test]
fn provider_context_defaults_to_noop_host() {
    let ctx = ProviderContext::new(
        NoopHost::arc(),
        crate::config::ChannelsConfig::default(),
        reqwest::Client::new(),
    );
    assert!(ctx.capabilities().is_lean());
    assert!(ctx.host.dispatcher().is_none());
}

// --- Builder + capability probe ------------------------------------------

#[test]
fn builder_advertises_only_plugged_capabilities() {
    let host = ChannelHostBuilder::new()
        .dispatcher(Arc::new(MockDispatcher))
        .transcriber(Arc::new(MockTranscriber))
        .approvals(Arc::new(YesApprovals))
        .build();

    let caps = host.capabilities();
    assert!(caps.turn_dispatch);
    assert!(caps.stt);
    assert!(caps.approvals);
    // Not plugged:
    assert!(!caps.tts);
    assert!(!caps.reaction_gate);
    assert!(!caps.run_ledger);
    assert!(!caps.is_lean());
}

// --- Turn dispatch --------------------------------------------------------

#[tokio::test]
async fn dispatch_streams_output_events() {
    let host = ChannelHostBuilder::new()
        .dispatcher(Arc::new(MockDispatcher))
        .build();
    let dispatcher = host.dispatcher().expect("dispatcher present");

    let mut turn = ChannelTurn::default();
    turn.envelope.message_id = "m42".into();

    let handle = dispatcher
        .dispatch(DispatchRequest::new(turn))
        .await
        .expect("dispatch ok");
    assert_eq!(handle.run_id, "run-1");

    let events = handle.collect().await;
    assert_eq!(events.len(), 2);
    assert!(matches!(events[0], ChannelOutputEvent::TextDelta { .. }));
    match &events[1] {
        ChannelOutputEvent::FinalMessage { text } => assert_eq!(text, "done:m42"),
        other => panic!("unexpected final event: {other:?}"),
    }
}

#[tokio::test]
async fn turn_handle_cancel_invokes_hook() {
    let flag = Arc::new(AtomicBool::new(false));
    let flag_clone = Arc::clone(&flag);
    let (_, rx) = tokio::sync::mpsc::channel(1);
    let handle = TurnHandle::new(
        "run-x",
        rx,
        Arc::new(move || flag_clone.store(true, Ordering::SeqCst)),
    );
    handle.cancel();
    assert!(flag.load(Ordering::SeqCst));
}

// --- Auxiliary capabilities ----------------------------------------------

#[tokio::test]
async fn transcriber_capability_roundtrips() {
    let host = ChannelHostBuilder::new()
        .transcriber(Arc::new(MockTranscriber))
        .build();
    let stt = host.transcriber().expect("stt present");
    let out = stt
        .transcribe(TranscriptionRequest {
            audio_base64: "abcd".into(),
            mime_type: Some("audio/ogg".into()),
            file_name: None,
            language: Some("en".into()),
        })
        .await
        .expect("transcribe ok");
    assert_eq!(out.text, "heard:4");
    assert_eq!(out.language.as_deref(), Some("en"));
}

#[tokio::test]
async fn approval_gate_request_and_parse() {
    let host = ChannelHostBuilder::new()
        .approvals(Arc::new(YesApprovals))
        .build();
    let gate = host.approvals().expect("approvals present");

    let decision = gate
        .request(ApprovalAsk {
            id: "a1".into(),
            prompt: "proceed?".into(),
            choices: vec![],
            session_key: "s1".into(),
            timeout_secs: Some(60),
        })
        .await
        .expect("request ok");
    assert_eq!(decision, ApprovalDecision::Approve);

    assert_eq!(gate.parse_reply("YES"), Some(ApprovalDecision::Approve));
    assert_eq!(gate.parse_reply("no"), Some(ApprovalDecision::Deny));
    assert_eq!(gate.parse_reply("maybe"), None);
}
