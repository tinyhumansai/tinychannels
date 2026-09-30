//! Per-turn state for progressive channel delivery: the evolving draft
//! bubble, the ephemeral "thinking" bubble, and the typing indicator.

use super::MAX_FILLER_CHARS;

/// Per-turn progressive-edit buffer. `dirty=true` means there's new
/// content to flush; `edit_disabled=true` means the backend doesn't
/// support editing for this channel and we should finalize atomically.
#[derive(Debug, Default)]
pub struct StreamingState {
    /// Accumulated visible assistant text from `text_delta` events.
    pub content: String,
    /// Most recent tool status line (prepended to the message body).
    pub last_tool: Option<String>,
    /// Backend-assigned message id returned from the initial
    /// `send_channel_message`; subsequent edits target this id.
    pub message_id: Option<String>,
    /// `true` once a draft message has been posted to the channel,
    /// even when the backend response didn't include an id to target
    /// for future edits. Decouples "a draft exists" from "we can edit
    /// it" so `finalize_channel_reply` won't post a duplicate bubble
    /// when the id was lost.
    pub draft_sent: bool,
    /// New content has arrived since the last edit flush.
    pub dirty: bool,
    /// Consecutive edit failures. Reset to zero on every success.
    pub edit_failures: u32,
    /// Latched when the backend doesn't support edits for this channel
    /// — we stop trying and rely on the final atomic send.
    pub edit_disabled: bool,
    /// Accumulated LLM reasoning from `thinking_delta` events. Shown
    /// to the user as an ephemeral "💭 Thinking…" message that is
    /// **deleted** once the final response is ready (#600).
    pub thinking_accumulator: String,
    /// Backend-assigned id of the ephemeral thinking message. Used to
    /// delete it at finalization so the user sees only the clean reply.
    pub thinking_message_id: Option<String>,
    /// `true` once a thinking message has been posted to the channel.
    pub thinking_sent: bool,
    /// New thinking content has arrived since the last thinking flush.
    pub thinking_dirty: bool,
    /// Latched when the first thinking POST succeeded with 200 but the
    /// backend didn't return an id we can edit. Without this latch,
    /// every subsequent `thinking_dirty` tick re-enters the "send new
    /// message" branch and the user sees one italic bubble per
    /// accumulated snippet instead of a single evolving one (#600).
    pub thinking_edit_disabled: bool,
    /// Ids of ephemeral filler messages posted during long turns, in
    /// send order. Deleted in `finalize_channel_reply` after the
    /// canonical response is on screen.
    pub filler_message_ids: Vec<String>,
    /// Next entry in `STATIC_FILLERS` to send when we fall back to the
    /// rotating pool (no fresh thinking content to surface). Wraps
    /// modulo pool size.
    pub filler_index: usize,
    /// Consecutive filler-send failures. Reset to zero on success.
    pub filler_failures: u32,
    /// Latched when the backend rejects filler sends — stops hitting
    /// a broken endpoint every 13 s.
    pub filler_disabled: bool,
    /// Last dynamic snippet we posted as a filler. Used to skip a
    /// duplicate post when the thinking accumulator hasn't advanced
    /// enough to produce a new tail slice — we fall through to the
    /// static pool instead so the chat still sees movement.
    pub last_filler_snippet: Option<String>,
}

impl StreamingState {
    /// The backend has no edit route: stop attempting edits but **keep**
    /// `message_id`. The draft is still on the user's screen and we still own
    /// it, so finalization needs the id to delete it before posting the
    /// canonical reply. Dropping the id here is what orphaned the draft and
    /// left a stale "_working…_" bubble (#5230).
    pub fn latch_draft_edits_unsupported(&mut self) {
        self.edit_disabled = true;
    }

    /// The draft really is gone provider-side: the id is worthless, so forget
    /// it as well as disabling edits.
    pub fn forget_draft(&mut self) {
        self.message_id = None;
        self.edit_disabled = true;
    }

    /// Thinking-bubble counterpart of [`Self::latch_draft_edits_unsupported`].
    /// Keeping `thinking_message_id` is what lets finalization delete the
    /// ephemeral "💭 Thinking:" bubble instead of leaving it in the chat (#5230).
    pub fn latch_thinking_edits_unsupported(&mut self) {
        self.thinking_edit_disabled = true;
    }

    /// Thinking-bubble counterpart of [`Self::forget_draft`].
    pub fn forget_thinking(&mut self) {
        self.thinking_message_id = None;
        self.thinking_edit_disabled = true;
    }

    pub fn compose_draft(&self) -> String {
        let trimmed = self.content.trim_end();
        if trimmed.is_empty() {
            // No visible text yet — show a placeholder. Tool indicators
            // (🔧 …) are intentionally omitted so the draft only ever
            // contains content that is a clean prefix of the final
            // response. If the draft persists after finalization the
            // user sees benign placeholder text instead of stale tool
            // status lines (#600).
            "_working…_".to_string()
        } else {
            trimmed.to_string()
        }
    }
}

/// Typing-indicator bookkeeping. One per in-flight turn. Latches
/// `disabled` after repeated failures so channels without typing
/// support stop getting hit every 4 seconds.
#[derive(Debug, Default)]
pub struct TypingState {
    pub failures: u32,
    pub disabled: bool,
}

/// Extract a message id from a backend `send_channel_message` response.
/// The backend has used at least three shapes: `{"id":"..."}`,
/// `{"data":{"id":"..."}}`, and `{"messageId":1456,"success":true}` —
/// the last one returns the id as a JSON number, not a string, so
/// `as_str()` alone misses it (#600).
pub fn extract_message_id(resp: &serde_json::Value) -> Option<String> {
    let candidate = resp
        .get("id")
        .or_else(|| resp.get("messageId"))
        .or_else(|| resp.get("data").and_then(|d| d.get("id")))
        .or_else(|| resp.get("data").and_then(|d| d.get("messageId")))?;
    if let Some(s) = candidate.as_str() {
        return Some(s.to_string());
    }
    if let Some(n) = candidate.as_i64() {
        return Some(n.to_string());
    }
    if let Some(n) = candidate.as_u64() {
        return Some(n.to_string());
    }
    None
}

/// Pull the most recent `MAX_FILLER_CHARS` Unicode scalars out of the
/// thinking accumulator so we can surface a live snapshot of the agent's
/// reasoning as a filler. Returns `None` when there's nothing to show
/// yet. When the tail is truncated, drops the partial leading word so the
/// snippet reads cleanly.
pub fn latest_thinking_snippet(state: &StreamingState) -> Option<String> {
    let acc = state.thinking_accumulator.trim();
    if acc.is_empty() {
        return None;
    }
    let total = acc.chars().count();
    if total <= MAX_FILLER_CHARS {
        return Some(acc.to_string());
    }
    // Only a truncated tail can start mid-word; drop that partial word.
    let tail: String = acc.chars().skip(total - MAX_FILLER_CHARS).collect();
    let trimmed = tail
        .trim_start_matches(|c: char| !c.is_whitespace())
        .trim_start()
        .to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}
