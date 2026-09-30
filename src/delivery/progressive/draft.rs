//! Posting and progressively editing the evolving draft reply bubble.

use super::latch::{channel_edits_unsupported, mark_channel_edits_unsupported};
use super::state::{StreamingState, extract_message_id};
use super::{MAX_EDIT_FAILURES, ProgressiveSendError, ProgressiveSender};

/// Post or edit the draft carrying the latest buffered text. The first call
/// posts a new message and records its id; later calls edit it.
pub(super) async fn flush_streaming_edit(
    sender: &dyn ProgressiveSender,
    channel: &str,
    state: &mut StreamingState,
) {
    let draft = state.compose_draft();
    if draft.is_empty() {
        return;
    }
    state.dirty = false;

    if let Some(message_id) = state.message_id.clone() {
        // Known-missing edit route: skip the guaranteed 404 and let
        // finalization replace the draft (delete + fresh atomic reply).
        if channel_edits_unsupported(channel) {
            tracing::debug!(
                "[channel-inbound][stream] skipping edit channel='{}' msg_id={} — no edit route on this backend, draft stays as-is until finalize",
                channel,
                message_id,
            );
            state.latch_draft_edits_unsupported();
            return;
        }
        match sender.edit(channel, &message_id, &draft).await {
            Ok(()) => {
                tracing::debug!(
                    "[channel-inbound][stream] edit ok channel='{}' msg_id={} chars={}",
                    channel,
                    message_id,
                    draft.len(),
                );
                state.edit_failures = 0;
            }
            Err(ProgressiveSendError::Unavailable) => {}
            Err(ProgressiveSendError::EditUnsupported) => {
                // Keep `message_id`: the draft is still on screen and
                // finalization must be able to delete it (#5230).
                tracing::info!(
                    "[channel-inbound][stream] edit channel='{}' msg_id={} — backend has no edit route, keeping id for finalize cleanup and disabling progressive edits",
                    channel,
                    message_id,
                );
                mark_channel_edits_unsupported(channel);
                state.latch_draft_edits_unsupported();
            }
            Err(ProgressiveSendError::MessageGone) => {
                tracing::info!(
                    "[channel-inbound][stream] edit channel='{}' msg_id={} — message gone provider-side (404), clearing stale id and disabling further edits",
                    channel,
                    message_id,
                );
                state.forget_draft();
            }
            Err(err @ ProgressiveSendError::Other(_)) => {
                state.edit_failures += 1;
                tracing::warn!(
                    "[channel-inbound][stream] edit failed channel='{}' msg_id={} err={} (failures={}/{})",
                    channel,
                    message_id,
                    err,
                    state.edit_failures,
                    MAX_EDIT_FAILURES,
                );
                if state.edit_failures >= MAX_EDIT_FAILURES {
                    tracing::info!(
                        "[channel-inbound][stream] giving up on progressive edits for channel='{}', falling back to atomic delivery",
                        channel,
                    );
                    state.edit_disabled = true;
                }
            }
        }
        return;
    }

    match sender.send(channel, &draft).await {
        Ok(resp) => {
            // A message reached the user: record that before looking for an
            // id, so finalize never posts a second bubble.
            state.draft_sent = true;
            if let Some(id) = extract_message_id(&resp) {
                tracing::debug!(
                    "[channel-inbound][stream] initial draft sent channel='{}' msg_id={}",
                    channel,
                    id,
                );
                state.message_id = Some(id);
            } else {
                tracing::warn!(
                    "[channel-inbound][stream] initial draft sent but response lacked id — disabling progressive edits (finalize will skip sending a duplicate) channel='{}' resp={}",
                    channel,
                    resp,
                );
                state.edit_disabled = true;
            }
        }
        Err(ProgressiveSendError::Unavailable) => {}
        Err(err) => {
            state.edit_failures += 1;
            tracing::warn!(
                "[channel-inbound][stream] initial send failed channel='{}' err={} (failures={})",
                channel,
                err,
                state.edit_failures,
            );
            if state.edit_failures >= MAX_EDIT_FAILURES {
                state.edit_disabled = true;
            }
        }
    }
}
