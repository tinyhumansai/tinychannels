//! Delivering the canonical reply and cleaning up ephemeral bubbles.

use super::latch::{channel_edits_unsupported, mark_channel_edits_unsupported};
use super::state::StreamingState;
use super::{ProgressiveSendError, ProgressiveSender};

/// Send a reply as a fresh message, logging the outcome.
pub(super) async fn send_channel_reply(sender: &dyn ProgressiveSender, channel: &str, text: &str) {
    match sender.send(channel, text).await {
        Ok(resp) => {
            tracing::info!(
                "[channel-inbound] reply sent to channel='{}' response={:?}",
                channel,
                resp
            );
        }
        Err(ProgressiveSendError::Unavailable) => {
            tracing::error!(
                "[channel-inbound] cannot reply to channel='{}' — sender unavailable",
                channel
            );
        }
        Err(e) => {
            tracing::error!(
                "[channel-inbound] failed to send reply to channel='{}': {}",
                channel,
                e
            );
        }
    }
}

/// Delete a posted message (an ephemeral bubble or an orphaned draft).
pub(super) async fn delete_channel_message(
    sender: &dyn ProgressiveSender,
    channel: &str,
    message_id: &str,
) {
    match sender.delete(channel, message_id).await {
        Ok(()) => {
            tracing::info!(
                "[channel-inbound] deleted ephemeral msg channel='{}' msg_id={}",
                channel,
                message_id,
            );
        }
        Err(ProgressiveSendError::Unavailable) => {}
        Err(ProgressiveSendError::MessageGone) => {
            tracing::info!(
                "[channel-inbound] delete channel='{}' msg_id={} — message already gone provider-side (404), nothing to clean up",
                channel,
                message_id,
            );
        }
        Err(err) => {
            tracing::warn!(
                "[channel-inbound] failed to delete ephemeral msg channel='{}' msg_id={} err={}",
                channel,
                message_id,
                err,
            );
        }
    }
}

/// Replace an on-screen draft: delete it, then post the reply fresh.
async fn replace_draft(
    sender: &dyn ProgressiveSender,
    channel: &str,
    message_id: &str,
    final_text: &str,
) {
    delete_channel_message(sender, channel, message_id).await;
    send_channel_reply(sender, channel, final_text).await;
}

/// Deliver the final canonical reply, then delete fillers and the thinking
/// bubble.
///
/// **Invariant**: once a draft is on the user's screen, never leave it next to
/// a second copy of the reply. With an id we edit it one last time (or delete
/// and replace it when editing is impossible). Without an id, a second bubble
/// is accepted because the draft holds only a clean prefix and a missing final
/// reply is worse. Only when no draft exists is the reply a first message.
pub(super) async fn finalize_channel_reply(
    sender: &dyn ProgressiveSender,
    channel: &str,
    state: &mut StreamingState,
    final_text: &str,
) {
    // Reply first, clean up second: the chat is never momentarily empty.
    if let Some(message_id) = state.message_id.clone() {
        if channel_edits_unsupported(channel) {
            tracing::info!(
                "[channel-inbound] final edit skipped channel='{}' msg_id={} — no edit route on this backend, replacing the draft with a fresh atomic reply",
                channel,
                message_id,
            );
            replace_draft(sender, channel, &message_id, final_text).await;
        } else {
            match sender.edit(channel, &message_id, final_text).await {
                Ok(()) => {
                    tracing::info!(
                        "[channel-inbound] final edit ok channel='{}' msg_id={} chars={}",
                        channel,
                        message_id,
                        final_text.len(),
                    );
                }
                Err(ProgressiveSendError::Unavailable) => {
                    tracing::warn!(
                        "[channel-inbound] cannot finalize channel='{}' msg_id={} — backend client unavailable, draft left in place",
                        channel,
                        message_id,
                    );
                }
                Err(ProgressiveSendError::MessageGone) => {
                    tracing::info!(
                        "[channel-inbound] final edit channel='{}' msg_id={} — draft already gone provider-side (404), sending fresh atomic reply",
                        channel,
                        message_id,
                    );
                    send_channel_reply(sender, channel, final_text).await;
                }
                Err(ProgressiveSendError::EditUnsupported) => {
                    tracing::warn!(
                        "[channel-inbound] final edit channel='{}' msg_id={} — backend has no edit route, deleting the draft and sending a fresh atomic reply",
                        channel,
                        message_id,
                    );
                    mark_channel_edits_unsupported(channel);
                    replace_draft(sender, channel, &message_id, final_text).await;
                }
                Err(err @ ProgressiveSendError::Other(_)) => {
                    tracing::warn!(
                        "[channel-inbound] final edit failed channel='{}' msg_id={} err={} — deleting orphan draft and sending fresh atomic reply so user still sees the canonical response",
                        channel,
                        message_id,
                        err,
                    );
                    replace_draft(sender, channel, &message_id, final_text).await;
                }
            }
        }
    } else {
        if state.draft_sent {
            tracing::warn!(
                "[channel-inbound] sending fresh reply on channel='{}' — id-less draft exists but user needs the final response",
                channel,
            );
        }
        send_channel_reply(sender, channel, final_text).await;
    }

    // Fillers first (oldest first), then the thinking bubble.
    for id in std::mem::take(&mut state.filler_message_ids) {
        delete_channel_message(sender, channel, &id).await;
    }
    if let Some(thinking_id) = state.thinking_message_id.take() {
        delete_channel_message(sender, channel, &thinking_id).await;
    }
}
