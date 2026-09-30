//! The ephemeral bubbles around a draft: typing indicator, "💭 Thinking:"
//! bubble and rotating "still working" fillers.

use super::latch::{channel_edits_unsupported, mark_channel_edits_unsupported};
use super::state::{StreamingState, TypingState, extract_message_id, latest_thinking_snippet};
use super::{
    MAX_FILLER_FAILURES, MAX_THINKING_DISPLAY_CHARS, MAX_TYPING_FAILURES, ProgressiveSendError,
    ProgressiveSender, STATIC_FILLERS,
};

/// Fire one typing indicator. Latches `disabled` after repeated failures so a
/// timer can keep calling this without piling up warnings.
pub(super) async fn send_typing_indicator(
    sender: &dyn ProgressiveSender,
    channel: &str,
    state: &mut TypingState,
) {
    if state.disabled {
        return;
    }
    match sender.typing(channel).await {
        Ok(()) => {
            if state.failures > 0 {
                tracing::debug!(
                    "[channel-inbound][typing] recovered channel='{}' after {} failure(s)",
                    channel,
                    state.failures,
                );
            }
            state.failures = 0;
        }
        Err(ProgressiveSendError::Unavailable) => {}
        Err(err) => {
            state.failures += 1;
            tracing::debug!(
                "[channel-inbound][typing] indicator failed channel='{}' err={} (failures={}/{})",
                channel,
                err,
                state.failures,
                MAX_TYPING_FAILURES,
            );
            if state.failures >= MAX_TYPING_FAILURES {
                tracing::info!(
                    "[channel-inbound][typing] disabling typing indicator for channel='{}' — backend unsupported",
                    channel,
                );
                state.disabled = true;
            }
        }
    }
}

/// Render the thinking bubble text, truncated to the display cap.
pub(super) fn thinking_bubble_text(accumulator: &str) -> Option<String> {
    let trimmed = accumulator.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut snippet = trimmed.to_string();
    if snippet.len() > MAX_THINKING_DISPLAY_CHARS {
        let mut cut = MAX_THINKING_DISPLAY_CHARS;
        while !snippet.is_char_boundary(cut) {
            cut -= 1;
        }
        snippet.truncate(cut);
        snippet.push('…');
    }
    Some(format!("💭 Thinking:\n_{snippet}_"))
}

/// Post or edit the ephemeral thinking bubble. It is deleted at finalization.
pub(super) async fn flush_thinking_message(
    sender: &dyn ProgressiveSender,
    channel: &str,
    state: &mut StreamingState,
) {
    state.thinking_dirty = false;
    let Some(text) = thinking_bubble_text(&state.thinking_accumulator) else {
        return;
    };

    if let Some(msg_id) = state.thinking_message_id.clone() {
        if channel_edits_unsupported(channel) {
            tracing::debug!(
                "[channel-inbound][thinking] skipping edit channel='{}' msg_id={} — no edit route on this backend, bubble stays until finalize deletes it",
                channel,
                msg_id,
            );
            state.latch_thinking_edits_unsupported();
            return;
        }
        match sender.edit(channel, &msg_id, &text).await {
            Ok(()) | Err(ProgressiveSendError::Unavailable) => {}
            Err(ProgressiveSendError::EditUnsupported) => {
                // Keep the id so finalization still deletes the bubble (#5230).
                tracing::info!(
                    "[channel-inbound][thinking] edit channel='{}' msg_id={} — backend has no edit route, keeping id so finalize still deletes the bubble",
                    channel,
                    msg_id,
                );
                mark_channel_edits_unsupported(channel);
                state.latch_thinking_edits_unsupported();
            }
            Err(ProgressiveSendError::MessageGone) => {
                tracing::info!(
                    "[channel-inbound][thinking] edit channel='{}' msg_id={} — thinking msg gone provider-side (404), clearing id and disabling further thinking edits",
                    channel,
                    msg_id,
                );
                state.forget_thinking();
            }
            Err(err @ ProgressiveSendError::Other(_)) => {
                tracing::debug!(
                    "[channel-inbound][thinking] edit failed channel='{}' msg_id={} err={}",
                    channel,
                    msg_id,
                    err,
                );
            }
        }
        return;
    }

    match sender.send(channel, &text).await {
        Ok(resp) => {
            state.thinking_sent = true;
            if let Some(id) = extract_message_id(&resp) {
                tracing::debug!(
                    "[channel-inbound][thinking] thinking msg sent channel='{}' msg_id={}",
                    channel,
                    id,
                );
                state.thinking_message_id = Some(id);
            } else {
                tracing::warn!(
                    "[channel-inbound][thinking] thinking msg sent but response lacked id — disabling further thinking flushes (message won't be deletable) channel='{}' resp={}",
                    channel,
                    resp,
                );
                state.thinking_edit_disabled = true;
            }
        }
        Err(ProgressiveSendError::Unavailable) => {}
        Err(err) => {
            tracing::warn!(
                "[channel-inbound][thinking] failed to send thinking msg channel='{}' err={} — disabling further thinking flushes",
                channel,
                err,
            );
            state.thinking_edit_disabled = true;
        }
    }
}

/// Choose the next filler: a fresh snippet of the reasoning stream when there
/// is one, otherwise the next entry of the static pool.
pub(super) fn next_filler_text(state: &mut StreamingState) -> String {
    match latest_thinking_snippet(state) {
        Some(snippet) if state.last_filler_snippet.as_deref() != Some(snippet.as_str()) => {
            let text = format!("💭 _{snippet}…_");
            state.last_filler_snippet = Some(snippet);
            text
        }
        _ => {
            let idx = state.filler_index % STATIC_FILLERS.len();
            state.filler_index = state.filler_index.wrapping_add(1);
            STATIC_FILLERS[idx].to_string()
        }
    }
}

/// Post a filler and record its id for deletion at finalization.
pub(super) async fn send_filler_message(
    sender: &dyn ProgressiveSender,
    channel: &str,
    state: &mut StreamingState,
) {
    let text = next_filler_text(state);
    match sender.send(channel, &text).await {
        Ok(resp) => {
            state.filler_failures = 0;
            if let Some(id) = extract_message_id(&resp) {
                tracing::debug!(
                    "[channel-inbound][filler] sent channel='{}' len={} msg_id={}",
                    channel,
                    text.len(),
                    id,
                );
                state.filler_message_ids.push(id);
            } else {
                tracing::warn!(
                    "[channel-inbound][filler] sent but response lacked id — cannot clean up on finalize channel='{}' resp={}",
                    channel,
                    resp,
                );
            }
        }
        Err(ProgressiveSendError::Unavailable) => {}
        Err(err) => {
            state.filler_failures = state.filler_failures.saturating_add(1);
            tracing::warn!(
                "[channel-inbound][filler] send failed channel='{}' err={} (failures={}/{})",
                channel,
                err,
                state.filler_failures,
                MAX_FILLER_FAILURES,
            );
            if state.filler_failures >= MAX_FILLER_FAILURES {
                tracing::info!(
                    "[channel-inbound][filler] disabling filler messages for channel='{}' — backend unsupported",
                    channel,
                );
                state.filler_disabled = true;
            }
        }
    }
}
