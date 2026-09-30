//! Human-paced splitting of one assistant reply into several chat bubbles.
//!
//! Pure text logic with no host dependencies: [`segment_for_delivery`] decides
//! whether a reply reads better as one block or as a few short bubbles, and
//! [`segment_delay`] gives the inter-bubble pause for a segment.

/// Segments shorter than this are merged into their neighbour.
const MIN_SEGMENT_CHARS: usize = 40;
/// Upper bound on delivered bubbles; overflow is merged into the last one.
const MAX_SEGMENTS: usize = 5;

/// Decide whether and how to split a response into multiple chat bubbles.
///
/// Rules (applied in order):
/// - Short messages (< 80 chars) are never split.
/// - Messages containing code fences (```) are never split.
/// - Messages that are predominantly structured (lists, tables, headers)
///   are never split — they read better as a single block.
/// - Otherwise, split on paragraph breaks (\n\n), merging segments that
///   are too short to stand alone.
/// - Fallback: split on sentence boundaries if paragraphs don't yield
///   multiple segments.
pub fn segment_for_delivery(text: &str) -> Vec<String> {
    let trimmed = text.trim();

    // Don't split short messages.
    if trimmed.len() < 80 {
        return vec![trimmed.to_string()];
    }

    // Never split messages containing code fences.
    if trimmed.contains("```") {
        tracing::debug!("[delivery:segment] skipping segmentation: contains code fences");
        return vec![trimmed.to_string()];
    }

    // Never split messages that are predominantly structured content.
    if is_structured_content(trimmed) {
        tracing::debug!("[delivery:segment] skipping segmentation: structured content");
        return vec![trimmed.to_string()];
    }

    // Strategy 1: paragraph splits.
    let paragraphs: Vec<&str> = trimmed
        .split("\n\n")
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .collect();

    if paragraphs.len() >= 2 {
        let merged = merge_short(&paragraphs, "\n\n");
        if merged.len() >= 2 {
            tracing::debug!(
                segments = merged.len(),
                "[delivery:segment] split by paragraphs"
            );
            return cap_segments(merged, MAX_SEGMENTS, "\n\n");
        }
    }

    // Strategy 2: sentence splits.
    let sentences = split_sentences(trimmed);
    if sentences.len() >= 2 {
        let grouped = group_sentences(&sentences);
        if grouped.len() >= 2 {
            tracing::debug!(
                segments = grouped.len(),
                "[delivery:segment] split by sentences"
            );
            return cap_segments(grouped, MAX_SEGMENTS, " ");
        }
    }

    // Fallback: single bubble.
    vec![trimmed.to_string()]
}

/// Returns true if the text is predominantly structured content that
/// shouldn't be split across bubbles (markdown lists, tables, headers).
fn is_structured_content(text: &str) -> bool {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return false;
    }

    let structured_count = lines
        .iter()
        .filter(|line| {
            let trimmed = line.trim();
            trimmed.starts_with("- ")
                || trimmed.starts_with("* ")
                || trimmed.starts_with("| ")
                || trimmed.starts_with("# ")
                || trimmed.starts_with("## ")
                || trimmed.starts_with("### ")
                || is_numbered_list_item(trimmed)
        })
        .count();

    // If more than 40% of non-empty lines are structured, don't split.
    let non_empty = lines.iter().filter(|l| !l.trim().is_empty()).count();
    non_empty > 0 && (structured_count * 100 / non_empty) > 40
}

/// Check if a line starts with a numbered list prefix like "1. " or "12. ".
/// Rejects dates ("2024. ") and decimals by requiring the digits+dot+space
/// to appear at the very start and be followed by text.
fn is_numbered_list_item(line: &str) -> bool {
    let bytes = line.as_bytes();
    let mut i = 0;
    // Consume one or more leading ASCII digits.
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    // Must have consumed at least one digit, followed by ". ".
    i > 0 && i <= 3 && bytes.get(i) == Some(&b'.') && bytes.get(i + 1) == Some(&b' ')
}

/// Cap the number of delivered segments at `max` without losing content:
/// the first `max - 1` segments are kept as-is, and any overflow is
/// concatenated into a single trailing segment using `joiner`.
///
/// The earlier behavior (`.take(MAX_SEGMENTS)`) silently dropped every
/// segment past the cap, which truncated long agent replies in the UI
/// (issue #1041). Merging into the tail preserves all content while
/// still bounding the inter-bubble delay budget.
fn cap_segments(segments: Vec<String>, max: usize, joiner: &str) -> Vec<String> {
    if max == 0 || segments.len() <= max {
        return segments;
    }
    let original_len = segments.len();
    let mut iter = segments.into_iter();
    let mut result: Vec<String> = (&mut iter).take(max - 1).collect();
    let tail: Vec<String> = iter.collect();
    let tail_count = tail.len();
    let merged = tail.join(joiner);
    tracing::debug!(
        target: "delivery",
        max,
        original_len,
        tail_count,
        tail_len = merged.len(),
        joiner_len = joiner.len(),
        "[delivery:segment] merging {} overflow segments into tail",
        tail_count
    );
    result.push(merged);
    result
}

/// Merge adjacent segments shorter than MIN_SEGMENT_CHARS.
fn merge_short(parts: &[&str], joiner: &str) -> Vec<String> {
    let mut result: Vec<String> = Vec::new();
    for part in parts {
        if !result.is_empty() && part.len() < MIN_SEGMENT_CHARS {
            let last = result.last_mut().unwrap();
            last.push_str(joiner);
            last.push_str(part);
        } else {
            result.push(part.to_string());
        }
    }
    result
}

/// Split text on sentence-ending punctuation (. ! ?) followed by a space
/// and an uppercase letter.
fn split_sentences(text: &str) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    let mut current = String::new();
    let chars: Vec<char> = text.chars().collect();

    let mut i = 0;
    while i < chars.len() {
        current.push(chars[i]);
        let ch = chars[i];

        // Latin sentence terminators: split on ". " followed by uppercase.
        if (ch == '.' || ch == '!' || ch == '?')
            && i + 2 < chars.len()
            && chars[i + 1] == ' '
            && chars[i + 2].is_ascii_uppercase()
        {
            let trimmed = current.trim().to_string();
            if !trimmed.is_empty() {
                parts.push(trimmed);
            }
            current.clear();
            i += 2; // skip the space
            continue;
        }

        // CJK sentence terminators: split after fullwidth period/exclamation/question.
        if (ch == '\u{3002}' || ch == '\u{FF01}' || ch == '\u{FF1F}') && i + 1 < chars.len() {
            let trimmed = current.trim().to_string();
            if !trimmed.is_empty() {
                parts.push(trimmed);
            }
            current.clear();
            i += 1;
            continue;
        }

        i += 1;
    }

    let remaining = current.trim().to_string();
    if !remaining.is_empty() {
        parts.push(remaining);
    }
    parts
}

/// Group sentences into 2-3 bubbles.
fn group_sentences(sentences: &[String]) -> Vec<String> {
    let target_count = std::cmp::min(3, sentences.len().div_ceil(2));
    let group_size = sentences.len().div_ceil(target_count);
    let mut groups: Vec<String> = Vec::new();

    for chunk in sentences.chunks(group_size) {
        let joined = chunk.join(" ");
        if joined.len() >= MIN_SEGMENT_CHARS {
            groups.push(joined);
        } else if let Some(last) = groups.last_mut() {
            last.push(' ');
            last.push_str(&joined);
        } else {
            groups.push(joined);
        }
    }
    groups
}

/// Compute a human-feeling inter-bubble delay in milliseconds.
/// Bounded: 500ms–1400ms, scaling with segment length.
pub fn segment_delay(segment: &str) -> u64 {
    let base: u64 = 500;
    let per_char: u64 = 2; // ~1.5-2ms per char for a natural reading pace
    std::cmp::min(base + (segment.len() as u64) * per_char, 1400)
}

#[cfg(test)]
#[path = "segment_test.rs"]
mod tests;
