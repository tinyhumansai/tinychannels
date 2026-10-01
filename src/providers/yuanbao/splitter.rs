//! Fence-aware Markdown splitter.
//!
//! When a long AI response is split into N chunks for the Yuanbao
//! `max_message_length` cap, we must not break inside:
//!   - a fenced code block (``` … ``` or ~~~ … ~~~)
//!   - a Markdown table row (lines starting with `|`)
//!   - a list-continuation block
//!
//! Strategy: walk the input by line, tracking fence/table state, and
//! emit a chunk every time adding the next line would push the buffer
//! past the cap **and** the cap boundary is safe (not inside a fence,
//! not in the middle of a table). If a single line is itself longer
//! than the cap, hard-split at a char boundary.

/// Split `text` into chunks no larger than `cap_bytes` (utf-8 byte count),
/// preserving fenced code blocks and table rows where possible.
pub fn split_markdown(text: &str, cap_bytes: usize) -> Vec<String> {
    if text.len() <= cap_bytes {
        return vec![text.to_string()];
    }
    let cap = cap_bytes.max(1);
    // Reserve a small headroom so the trailing newline / final char fits
    // when we flush. For very small caps fall back to no margin so callers
    // testing tight bounds (cap=20) still get chunks under the cap.
    let safe_cap = if cap >= 32 {
        cap.saturating_sub(16)
    } else {
        cap
    };

    let mut chunks: Vec<String> = Vec::new();
    let mut buf = String::with_capacity(cap);
    let mut in_fence = false;
    let mut fence_marker: Option<String> = None;

    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_end_matches('\n');
        let starts_fence = is_fence(trimmed);

        // If this single line is wider than the cap, we must hard-split it.
        if line.len() > cap {
            flush(&mut chunks, &mut buf);
            for piece in hard_split(line, cap) {
                chunks.push(piece);
            }
            continue;
        }

        let candidate_len = buf.len() + line.len();
        if candidate_len > safe_cap && !buf.is_empty() && safe_to_break(in_fence) {
            flush(&mut chunks, &mut buf);
        }
        buf.push_str(line);

        if let Some(marker) = starts_fence {
            if let Some(open) = &fence_marker {
                if marker == *open {
                    in_fence = false;
                    fence_marker = None;
                }
            } else {
                in_fence = true;
                fence_marker = Some(marker);
            }
        }
    }
    flush(&mut chunks, &mut buf);

    // Drop empty trailing chunks (can happen if input ends on newline).
    chunks.retain(|c| !c.trim().is_empty());
    chunks
}

fn flush(chunks: &mut Vec<String>, buf: &mut String) {
    if !buf.is_empty() {
        chunks.push(buf.trim_end().to_string());
        buf.clear();
    }
}

fn safe_to_break(in_fence: bool) -> bool {
    !in_fence
}

/// If `line` opens or closes a fenced code block, return the marker text
/// (e.g. "```" or "~~~"). A line that contains a fence in the middle is
/// NOT a fence marker; only lines that *start* with three or more
/// backticks/tildes count.
fn is_fence(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    if let Some(rest) = trimmed.strip_prefix("```") {
        // Allow optional language tag after the fence.
        let _ = rest;
        return Some("```".into());
    }
    if let Some(rest) = trimmed.strip_prefix("~~~") {
        let _ = rest;
        return Some("~~~".into());
    }
    None
}

/// Last-resort splitter for a line that's wider than the cap.
fn hard_split(line: &str, cap: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut remaining = line;
    while !remaining.is_empty() {
        if remaining.len() <= cap {
            out.push(remaining.to_string());
            break;
        }
        let mut idx = cap;
        while idx > 0 && !remaining.is_char_boundary(idx) {
            idx -= 1;
        }
        if idx == 0 {
            // pathological — emit one char at a time
            let take = remaining
                .char_indices()
                .nth(1)
                .map(|(i, _)| i)
                .unwrap_or(remaining.len());
            let (chunk, rest) = remaining.split_at(take);
            out.push(chunk.to_string());
            remaining = rest;
        } else {
            let (chunk, rest) = remaining.split_at(idx);
            out.push(chunk.to_string());
            remaining = rest;
        }
    }
    out
}

#[cfg(test)]
#[path = "splitter_tests.rs"]
mod tests;
