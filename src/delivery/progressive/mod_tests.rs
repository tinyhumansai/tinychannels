//! Progressive delivery against a scripted fake sender.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{Value, json};

use super::bubbles::{next_filler_text, thinking_bubble_text};
use super::*;

#[derive(Debug, Clone, PartialEq)]
enum Call {
    Send(String),
    Edit(String, String),
    Delete(String),
    Typing,
}

/// Scripted outcome for the next call of a kind. Unscripted calls succeed;
/// sends answer `{"id": "m<n>"}`.
enum Outcome {
    Ok(Value),
    Err(fn() -> ProgressiveSendError),
}

#[derive(Default)]
struct FakeSender {
    calls: Mutex<Vec<Call>>,
    sends: Mutex<VecDeque<Outcome>>,
    edits: Mutex<VecDeque<Outcome>>,
    deletes: Mutex<VecDeque<Outcome>>,
    typings: Mutex<VecDeque<Outcome>>,
}

impl FakeSender {
    fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }
    fn script(queue: &Mutex<VecDeque<Outcome>>, outcome: Outcome) {
        queue.lock().unwrap().push_back(outcome);
    }
    fn next(queue: &Mutex<VecDeque<Outcome>>) -> Option<Outcome> {
        queue.lock().unwrap().pop_front()
    }
    fn unit(outcome: Option<Outcome>) -> Result<(), ProgressiveSendError> {
        match outcome {
            Some(Outcome::Err(make)) => Err(make()),
            _ => Ok(()),
        }
    }
}

#[async_trait]
impl ProgressiveSender for FakeSender {
    async fn send(&self, _channel: &str, text: &str) -> Result<Value, ProgressiveSendError> {
        let n = {
            let mut calls = self.calls.lock().unwrap();
            calls.push(Call::Send(text.into()));
            calls.iter().filter(|c| matches!(c, Call::Send(_))).count()
        };
        match Self::next(&self.sends) {
            Some(Outcome::Ok(v)) => Ok(v),
            Some(Outcome::Err(make)) => Err(make()),
            None => Ok(json!({ "id": format!("m{n}") })),
        }
    }
    async fn edit(&self, _c: &str, id: &str, text: &str) -> Result<(), ProgressiveSendError> {
        self.calls
            .lock()
            .unwrap()
            .push(Call::Edit(id.into(), text.into()));
        Self::unit(Self::next(&self.edits))
    }
    async fn delete(&self, _c: &str, id: &str) -> Result<(), ProgressiveSendError> {
        self.calls.lock().unwrap().push(Call::Delete(id.into()));
        Self::unit(Self::next(&self.deletes))
    }
    async fn typing(&self, _c: &str) -> Result<(), ProgressiveSendError> {
        self.calls.lock().unwrap().push(Call::Typing);
        Self::unit(Self::next(&self.typings))
    }
}

fn other() -> ProgressiveSendError {
    ProgressiveSendError::Other(anyhow::anyhow!("502 Bad Gateway"))
}
fn unsupported() -> ProgressiveSendError {
    ProgressiveSendError::EditUnsupported
}
fn gone() -> ProgressiveSendError {
    ProgressiveSendError::MessageGone
}
fn unavailable() -> ProgressiveSendError {
    ProgressiveSendError::Unavailable
}

fn turn(channel: &str) -> (Arc<FakeSender>, ProgressiveReply) {
    let sender = Arc::new(FakeSender::default());
    let reply = ProgressiveReply::new(sender.clone(), channel);
    (sender, reply)
}

// ── capability gating ──────────────────────────────────────────────────────

#[test]
fn progressive_ui_is_an_allowlist_failing_safe_for_unknown_channels() {
    assert!(channel_supports_progressive_ui("telegram"));
    assert!(channel_supports_progressive_ui("tg"));
    assert!(channel_supports_progressive_ui("tg:12345"));
    assert!(!channel_supports_progressive_ui("discord"));
    assert!(!channel_supports_progressive_ui("discord:guild-1"));
    assert!(!channel_supports_progressive_ui("slack"));
    assert!(!channel_supports_progressive_ui("whatsapp:123"));
}

#[tokio::test]
async fn channels_without_progressive_ui_get_no_bubbles_but_still_a_reply() {
    let (sender, mut reply) = turn("discord:guild-1");
    reply.on_text_delta("partial");
    reply.on_thinking_delta("reasoning");
    reply.edit_tick().await;
    reply.filler_tick().await;
    reply.finalize("final").await;
    assert_eq!(sender.calls(), vec![Call::Send("final".into())]);
}

// ── state ──────────────────────────────────────────────────────────────────

#[test]
fn compose_draft_uses_placeholder_until_text_arrives() {
    let mut state = StreamingState::default();
    assert_eq!(state.compose_draft(), "_working…_");
    state.last_tool = Some("🔧 search…".into());
    assert_eq!(state.compose_draft(), "_working…_");
    state.content = "hello  \n".into();
    assert_eq!(state.compose_draft(), "hello");
}

#[test]
fn route_absence_keeps_ids_and_message_absence_forgets_them() {
    let mut state = StreamingState {
        message_id: Some("1103".into()),
        thinking_message_id: Some("2201".into()),
        ..Default::default()
    };
    state.latch_draft_edits_unsupported();
    state.latch_thinking_edits_unsupported();
    assert!(state.edit_disabled && state.thinking_edit_disabled);
    assert_eq!(state.message_id.as_deref(), Some("1103"));
    assert_eq!(state.thinking_message_id.as_deref(), Some("2201"));

    state.forget_draft();
    state.forget_thinking();
    assert_eq!(state.message_id, None);
    assert_eq!(state.thinking_message_id, None);
}

#[test]
fn extract_message_id_handles_every_backend_shape() {
    assert_eq!(
        extract_message_id(&json!({"id": "a"})).as_deref(),
        Some("a")
    );
    assert_eq!(
        extract_message_id(&json!({"messageId": 1456, "success": true})).as_deref(),
        Some("1456")
    );
    assert_eq!(
        extract_message_id(&json!({"data": {"id": "b"}})).as_deref(),
        Some("b")
    );
    assert_eq!(
        extract_message_id(&json!({"data": {"messageId": 7}})).as_deref(),
        Some("7")
    );
    assert_eq!(
        extract_message_id(&json!({"id": u64::MAX})).as_deref(),
        Some(u64::MAX.to_string().as_str())
    );
    assert_eq!(extract_message_id(&json!({"id": true})), None);
    assert_eq!(extract_message_id(&json!({})), None);
}

#[test]
fn thinking_snippet_trims_partial_leading_word() {
    let state = |acc: &str| StreamingState {
        thinking_accumulator: acc.into(),
        ..Default::default()
    };
    assert_eq!(latest_thinking_snippet(&state("   ")), None);
    assert_eq!(
        latest_thinking_snippet(&state("short thought")).as_deref(),
        Some("short thought")
    );
    let long = format!("{} tail words here", "x".repeat(MAX_FILLER_CHARS));
    let snippet = latest_thinking_snippet(&state(&long)).unwrap();
    assert_eq!(snippet, "tail words here");
    assert_eq!(latest_thinking_snippet(&state(&"y".repeat(500))), None);
}

#[test]
fn thinking_bubble_truncates_on_a_char_boundary() {
    assert_eq!(thinking_bubble_text("  "), None);
    assert_eq!(
        thinking_bubble_text("plan").as_deref(),
        Some("💭 Thinking:\n_plan_")
    );
    // Multi-byte text straddling the cap must not panic.
    let text = thinking_bubble_text(&"é".repeat(MAX_THINKING_DISPLAY_CHARS)).unwrap();
    assert!(text.ends_with("…_"));
}

#[test]
fn filler_prefers_fresh_thinking_then_rotates_static_pool() {
    let mut state = StreamingState {
        thinking_accumulator: "checking the calendar".into(),
        ..Default::default()
    };
    assert_eq!(next_filler_text(&mut state), "💭 _checking the calendar…_");
    // Same snippet again → fall through to the static pool.
    assert_eq!(next_filler_text(&mut state), STATIC_FILLERS[0]);
    assert_eq!(next_filler_text(&mut state), STATIC_FILLERS[1]);
    assert_eq!(next_filler_text(&mut state), STATIC_FILLERS[2]);
    assert_eq!(next_filler_text(&mut state), STATIC_FILLERS[0]);
}

// ── latch ──────────────────────────────────────────────────────────────────

#[test]
fn edit_capability_latches_per_provider_not_per_chat() {
    assert!(!channel_edits_unsupported("noedit-a:chat-1"));
    mark_channel_edits_unsupported("noedit-a:chat-1");
    mark_channel_edits_unsupported("noedit-a:chat-1");
    assert!(channel_edits_unsupported("noedit-a:chat-2"));
    assert!(channel_edits_unsupported("noedit-a"));
    assert!(!channel_edits_unsupported("noedit-b:chat-1"));
    assert_eq!(edit_capability_key("discord:guild:chan"), "discord");
    assert_eq!(edit_capability_key(""), "");
}

// ── streaming turn ─────────────────────────────────────────────────────────

#[tokio::test]
async fn draft_is_posted_once_then_edited_then_finalized_in_place() {
    let (sender, mut reply) = turn("telegram:1");
    reply.on_tool_call("search");
    reply.on_text_delta("Hel");
    reply.edit_tick().await;
    reply.on_text_delta("lo");
    reply.on_tool_result("search", true);
    reply.edit_tick().await;
    // Nothing dirty → no call.
    reply.edit_tick().await;
    reply.finalize("Hello!").await;
    assert_eq!(
        sender.calls(),
        vec![
            Call::Send("Hel".into()),
            Call::Edit("m1".into(), "Hello".into()),
            Call::Edit("m1".into(), "Hello!".into()),
        ]
    );
    assert_eq!(reply.state().last_tool.as_deref(), Some("🔧 search ✓"));
}

#[tokio::test]
async fn transient_edit_failures_disable_edits_after_budget() {
    let (sender, mut reply) = turn("telegram:2");
    reply.on_text_delta("a");
    reply.edit_tick().await;
    FakeSender::script(&sender.edits, Outcome::Err(other));
    FakeSender::script(&sender.edits, Outcome::Err(other));
    reply.on_text_delta("b");
    reply.edit_tick().await;
    assert!(!reply.state().edit_disabled);
    reply.on_text_delta("c");
    reply.edit_tick().await;
    assert!(reply.state().edit_disabled);
    assert_eq!(reply.state().message_id.as_deref(), Some("m1"));
}

#[tokio::test]
async fn message_gone_on_edit_forgets_the_draft_and_finalize_sends_fresh() {
    let (sender, mut reply) = turn("telegram:3");
    reply.on_text_delta("a");
    reply.edit_tick().await;
    FakeSender::script(&sender.edits, Outcome::Err(gone));
    reply.on_text_delta("b");
    reply.edit_tick().await;
    assert_eq!(reply.state().message_id, None);
    reply.finalize("done").await;
    // id-less but already-sent draft → fresh reply is still delivered.
    assert_eq!(sender.calls().last(), Some(&Call::Send("done".into())));
}

#[tokio::test]
async fn edit_route_absence_latches_provider_and_finalize_replaces_the_draft() {
    let (sender, mut reply) = turn("tg:latch-route");
    reply.on_text_delta("a");
    reply.edit_tick().await;
    FakeSender::script(&sender.edits, Outcome::Err(unsupported));
    reply.on_text_delta("b");
    reply.edit_tick().await;
    assert!(reply.state().edit_disabled);
    assert_eq!(reply.state().message_id.as_deref(), Some("m1"));
    reply.finalize("final").await;
    let calls = sender.calls();
    assert_eq!(
        &calls[calls.len() - 2..],
        &[Call::Delete("m1".into()), Call::Send("final".into())]
    );
    // The latch is per provider ("tg"), so the next turn skips edits entirely.
    assert!(channel_edits_unsupported("tg:other-chat"));
}

#[tokio::test]
async fn finalize_edit_failures_each_recover_with_a_visible_reply() {
    for (err, expect_delete) in [
        (other as fn() -> ProgressiveSendError, true),
        (gone as fn() -> ProgressiveSendError, false),
    ] {
        let (sender, mut reply) = turn("telegram:4");
        reply.on_text_delta("a");
        reply.edit_tick().await;
        FakeSender::script(&sender.edits, Outcome::Err(err));
        reply.finalize("final").await;
        let calls = sender.calls();
        assert_eq!(calls.last(), Some(&Call::Send("final".into())));
        assert_eq!(calls.contains(&Call::Delete("m1".into())), expect_delete);
    }
}

#[tokio::test]
async fn finalize_edit_route_absence_marks_the_latch() {
    let (sender, mut reply) = turn("latch-final:1");
    // Force progressive state as if the draft had been posted.
    reply.state.message_id = Some("d1".into());
    FakeSender::script(&sender.edits, Outcome::Err(unsupported));
    reply.finalize("final").await;
    assert!(channel_edits_unsupported("latch-final:2"));
    assert_eq!(
        sender.calls()[1..],
        [Call::Delete("d1".into()), Call::Send("final".into())]
    );
}

#[tokio::test]
async fn unavailable_sender_leaves_the_draft_in_place() {
    let (sender, mut reply) = turn("telegram:5");
    reply.on_text_delta("a");
    reply.edit_tick().await;
    FakeSender::script(&sender.edits, Outcome::Err(unavailable));
    reply.finalize("final").await;
    assert_eq!(sender.calls().len(), 2, "no delete/resend when unavailable");
}

#[tokio::test]
async fn id_less_initial_draft_disables_edits() {
    let (sender, mut reply) = turn("telegram:6");
    FakeSender::script(&sender.sends, Outcome::Ok(json!({"success": true})));
    reply.on_text_delta("a");
    reply.edit_tick().await;
    assert!(reply.state().draft_sent);
    assert!(reply.state().edit_disabled);
}

#[tokio::test]
async fn failed_initial_draft_sends_count_against_the_budget() {
    let (sender, mut reply) = turn("telegram:7");
    FakeSender::script(&sender.sends, Outcome::Err(other));
    FakeSender::script(&sender.sends, Outcome::Err(other));
    reply.on_text_delta("a");
    reply.edit_tick().await;
    reply.on_text_delta("b");
    reply.edit_tick().await;
    assert!(reply.state().edit_disabled);
    assert!(!reply.state().draft_sent);
}

#[tokio::test]
async fn thinking_bubble_is_posted_edited_and_deleted_with_fillers() {
    let (sender, mut reply) = turn("telegram:8");
    reply.on_thinking_delta("step one");
    reply.edit_tick().await;
    reply.on_thinking_delta(" and two");
    reply.edit_tick().await;
    reply.filler_tick().await;
    reply.finalize("answer").await;
    assert_eq!(
        sender.calls(),
        vec![
            Call::Send("💭 Thinking:\n_step one_".into()),
            Call::Edit("m1".into(), "💭 Thinking:\n_step one and two_".into()),
            Call::Send("💭 _step one and two…_".into()),
            Call::Send("answer".into()),
            Call::Delete("m2".into()),
            Call::Delete("m1".into()),
        ]
    );
}

#[tokio::test]
async fn thinking_edit_failures_follow_the_same_recoveries() {
    let (sender, mut reply) = turn("telegram:9");
    reply.on_thinking_delta("x");
    reply.edit_tick().await;
    FakeSender::script(&sender.edits, Outcome::Err(gone));
    reply.on_thinking_delta("y");
    reply.edit_tick().await;
    assert_eq!(reply.state().thinking_message_id, None);
    assert!(reply.state().thinking_edit_disabled);

    let (sender, mut reply) = turn("tg-think:1");
    reply.state.thinking_message_id = Some("t1".into());
    reply.state.thinking_accumulator = "x".into();
    reply.state.thinking_dirty = true;
    FakeSender::script(&sender.edits, Outcome::Err(other));
    reply.edit_tick().await;
    assert!(
        !reply.state().thinking_edit_disabled,
        "transient keeps trying"
    );
}

#[tokio::test]
async fn thinking_send_failure_or_missing_id_stops_thinking_flushes() {
    let (sender, mut reply) = turn("telegram:10");
    FakeSender::script(&sender.sends, Outcome::Err(other));
    reply.on_thinking_delta("x");
    reply.edit_tick().await;
    assert!(reply.state().thinking_edit_disabled);

    let (sender, mut reply) = turn("telegram:11");
    FakeSender::script(&sender.sends, Outcome::Ok(json!({})));
    reply.on_thinking_delta("x");
    reply.edit_tick().await;
    assert!(reply.state().thinking_sent && reply.state().thinking_edit_disabled);
}

#[tokio::test]
async fn filler_failures_disable_fillers() {
    let (sender, mut reply) = turn("telegram:12");
    FakeSender::script(&sender.sends, Outcome::Err(other));
    FakeSender::script(&sender.sends, Outcome::Err(other));
    reply.filler_tick().await;
    reply.filler_tick().await;
    reply.filler_tick().await;
    assert!(reply.state().filler_disabled);
    assert_eq!(sender.calls().len(), 2);
}

#[tokio::test]
async fn typing_indicator_latches_after_repeated_failures_and_recovers_before() {
    let (sender, mut reply) = turn("discord:1");
    FakeSender::script(&sender.typings, Outcome::Err(other));
    reply.typing_tick().await;
    reply.typing_tick().await; // recovers
    FakeSender::script(&sender.typings, Outcome::Err(other));
    FakeSender::script(&sender.typings, Outcome::Err(other));
    reply.typing_tick().await;
    reply.typing_tick().await;
    reply.typing_tick().await; // disabled: not sent
    assert_eq!(sender.calls().len(), 4);
}

#[tokio::test]
async fn delete_failures_are_tolerated_during_cleanup() {
    let (sender, mut reply) = turn("telegram:13");
    reply.state.filler_message_ids = vec!["f1".into(), "f2".into()];
    FakeSender::script(&sender.deletes, Outcome::Err(gone));
    FakeSender::script(&sender.deletes, Outcome::Err(other));
    reply.finalize("ok").await;
    assert_eq!(
        sender.calls(),
        vec![
            Call::Send("ok".into()),
            Call::Delete("f1".into()),
            Call::Delete("f2".into()),
        ]
    );
    assert!(reply.state().filler_message_ids.is_empty());
}

#[tokio::test]
async fn send_reply_is_standalone() {
    let (sender, reply) = turn("slack:1");
    FakeSender::script(&sender.sends, Outcome::Err(unavailable));
    reply.send_reply("sorry").await;
    reply.send_reply("sorry again").await;
    assert_eq!(reply.channel(), "slack:1");
    assert_eq!(sender.calls().len(), 2);
}
