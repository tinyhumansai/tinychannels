//! Text measurement, truncation, and outbound chunking.

use crate::channel::LengthUnit;

/// Default portable text chunk limit used when a channel does not override it.
pub const DEFAULT_TEXT_CHUNK_LIMIT: usize = 4000;

const CONTINUATION_RESERVE: usize = 8;
const FENCE_CLOSE: &str = "\n```";

/// Chunking mode for outbound text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkMode {
    /// Do not split; return the input as one chunk.
    None,
    /// Split only when the message exceeds the limit.
    Length,
    /// Prefer paragraph boundaries before falling back to length splitting.
    Newline,
}

/// Options for outbound text chunking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextChunkOptions {
    pub limit: usize,
    pub length_unit: LengthUnit,
    pub mode: ChunkMode,
    pub markdown: bool,
    pub indicators: bool,
}

impl Default for TextChunkOptions {
    fn default() -> Self {
        Self {
            limit: DEFAULT_TEXT_CHUNK_LIMIT,
            length_unit: LengthUnit::Characters,
            mode: ChunkMode::Length,
            markdown: true,
            indicators: true,
        }
    }
}

/// Measure text in the specified provider length unit.
pub fn measure_text(text: &str, unit: LengthUnit) -> usize {
    match unit {
        LengthUnit::Characters => text.chars().count(),
        LengthUnit::Utf8Bytes => text.len(),
        LengthUnit::Utf16Units => text.encode_utf16().count(),
    }
}

/// Return the longest prefix whose measured length is at most `limit`.
pub fn prefix_within_limit(text: &str, limit: usize, unit: LengthUnit) -> &str {
    if measure_text(text, unit) <= limit {
        return text;
    }

    let mut used = 0usize;
    let mut end = 0usize;
    for (idx, ch) in text.char_indices() {
        let char_units = measure_text(ch.encode_utf8(&mut [0; 4]), unit);
        if used + char_units > limit {
            break;
        }
        used += char_units;
        end = idx + ch.len_utf8();
    }
    &text[..end]
}

/// Resolve a text chunk limit from an optional override and fallback.
pub fn resolve_text_chunk_limit(
    override_limit: Option<usize>,
    fallback_limit: Option<usize>,
) -> usize {
    override_limit
        .filter(|limit| *limit > 0)
        .or_else(|| fallback_limit.filter(|limit| *limit > 0))
        .unwrap_or(DEFAULT_TEXT_CHUNK_LIMIT)
}

/// Truncate by Unicode scalar values and append an ellipsis only when truncated.
pub fn truncate_with_ellipsis(input: &str, max_chars: usize) -> String {
    let mut iter = input.chars();
    let mut out = String::new();
    for _ in 0..max_chars {
        let Some(ch) = iter.next() else {
            return input.to_string();
        };
        out.push(ch);
    }
    if iter.next().is_some() {
        out.push('…');
    }
    out
}

/// Chunk text using default character-counting length mode.
pub fn chunk_text(text: &str, limit: usize) -> Vec<String> {
    chunk_text_with_options(
        text,
        TextChunkOptions {
            limit,
            ..Default::default()
        },
    )
}

/// Chunk text with the requested mode and length unit.
pub fn chunk_text_with_mode(
    text: &str,
    limit: usize,
    length_unit: LengthUnit,
    mode: ChunkMode,
) -> Vec<String> {
    chunk_text_with_options(
        text,
        TextChunkOptions {
            limit,
            length_unit,
            mode,
            ..Default::default()
        },
    )
}

/// Chunk text according to provider limits while preserving markdown fences.
pub fn chunk_text_with_options(text: &str, options: TextChunkOptions) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    if options.limit == 0 || options.mode == ChunkMode::None {
        return vec![text.to_string()];
    }
    if measure_text(text, options.length_unit) <= options.limit {
        return vec![text.to_string()];
    }

    let chunks = match options.mode {
        ChunkMode::None => vec![text.to_string()],
        ChunkMode::Length => chunk_by_length(text, options),
        ChunkMode::Newline => chunk_by_paragraph(text, options),
    };

    if options.indicators && chunks.len() > 1 {
        append_indicators(chunks)
    } else {
        chunks
    }
}

fn chunk_by_paragraph(text: &str, options: TextChunkOptions) -> Vec<String> {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let paragraphs = split_paragraphs(&normalized);
    if paragraphs.len() <= 1 {
        return chunk_by_length(&normalized, options);
    }

    let mut out = Vec::new();
    let mut current = String::new();
    for paragraph in paragraphs {
        if current.is_empty() {
            if measure_text(paragraph, options.length_unit) <= options.limit {
                current.push_str(paragraph);
            } else {
                out.extend(chunk_by_length(paragraph, options));
            }
            continue;
        }

        let candidate = format!("{current}\n\n{paragraph}");
        if measure_text(&candidate, options.length_unit) <= options.limit {
            current = candidate;
        } else {
            out.push(current);
            current = String::new();
            if measure_text(paragraph, options.length_unit) <= options.limit {
                current.push_str(paragraph);
            } else {
                out.extend(chunk_by_length(paragraph, options));
            }
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

fn split_paragraphs(text: &str) -> Vec<&str> {
    let mut paragraphs = Vec::new();
    let mut start = 0usize;
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while i + 1 < bytes.len() {
        if bytes[i] == b'\n' && blank_line_starts_at(text, i + 1) {
            if text[start..i].trim().is_empty() {
                i += 1;
                continue;
            }
            paragraphs.push(text[start..i].trim_end());
            i = skip_blank_line(text, i + 1);
            start = i;
            continue;
        }
        i += 1;
    }
    let tail = text[start..].trim_end();
    if !tail.trim().is_empty() {
        paragraphs.push(tail);
    }
    paragraphs
}

fn blank_line_starts_at(text: &str, mut idx: usize) -> bool {
    let bytes = text.as_bytes();
    while idx < bytes.len() && matches!(bytes[idx], b' ' | b'\t') {
        idx += 1;
    }
    idx < bytes.len() && bytes[idx] == b'\n'
}

fn skip_blank_line(text: &str, mut idx: usize) -> usize {
    let bytes = text.as_bytes();
    while idx < bytes.len() && matches!(bytes[idx], b'\n' | b' ' | b'\t') {
        idx += 1;
    }
    idx
}

fn chunk_by_length(text: &str, options: TextChunkOptions) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut remaining = text;
    let mut carry_lang: Option<String> = None;

    while !remaining.is_empty() {
        let prefix = carry_lang
            .as_ref()
            .map(|lang| format!("```{lang}\n"))
            .unwrap_or_default();
        let reserve = if options.indicators {
            CONTINUATION_RESERVE
        } else {
            0
        };
        let used = measure_text(&prefix, options.length_unit) + reserve;
        let body_budget = options.limit.saturating_sub(used).max(1);

        if measure_text(&prefix, options.length_unit) + measure_text(remaining, options.length_unit)
            <= options.limit.saturating_sub(reserve)
        {
            chunks.push(format!("{prefix}{remaining}"));
            break;
        }

        let prefix_slice = prefix_within_limit(remaining, body_budget, options.length_unit);
        let mut split_at = choose_split_index(prefix_slice);
        if split_at == 0 {
            split_at = prefix_slice.len();
        }
        split_at = avoid_inline_code_split(prefix_slice, split_at);
        if split_at == 0 {
            split_at = prefix_slice.len();
        }

        let mut chunk_body = &remaining[..split_at];
        if options.markdown {
            let (in_code, _) = fence_state(carry_lang.as_deref(), chunk_body);
            let full_measure = measure_text(&prefix, options.length_unit)
                + measure_text(chunk_body, options.length_unit)
                + measure_text(FENCE_CLOSE, options.length_unit);
            if in_code && full_measure > options.limit.saturating_sub(reserve) {
                let close_budget = body_budget
                    .saturating_sub(measure_text(FENCE_CLOSE, options.length_unit))
                    .max(1);
                let close_prefix =
                    prefix_within_limit(remaining, close_budget, options.length_unit);
                split_at = choose_split_index(close_prefix);
                if split_at == 0 {
                    split_at = close_prefix.len();
                }
                split_at = avoid_inline_code_split(close_prefix, split_at);
                if split_at == 0 {
                    split_at = close_prefix.len();
                }
                chunk_body = &remaining[..split_at];
            }
        }
        let mut full_chunk = format!("{prefix}{chunk_body}");
        remaining = &remaining[split_at..];

        if options.markdown {
            let (in_code, lang) = fence_state(carry_lang.as_deref(), chunk_body);
            if in_code {
                full_chunk.push_str(FENCE_CLOSE);
                carry_lang = Some(lang.unwrap_or_default());
            } else {
                carry_lang = None;
            }
        }

        chunks.push(full_chunk);
    }

    chunks
}

fn choose_split_index(prefix: &str) -> usize {
    if let Some(index) = prefix.rfind('\n').filter(|idx| *idx > 0) {
        return index;
    }
    if let Some(index) = prefix.rfind(char::is_whitespace).filter(|idx| *idx > 0) {
        return index;
    }
    prefix.len()
}

fn avoid_inline_code_split(prefix: &str, split_at: usize) -> usize {
    let candidate = &prefix[..split_at];
    if unescaped_backtick_count(candidate).is_multiple_of(2) {
        return split_at;
    }
    let Some(last_bt) = find_last_unescaped_backtick(candidate) else {
        return split_at;
    };
    let safe = candidate[..last_bt].rfind(char::is_whitespace).unwrap_or(0);
    if safe > split_at / 4 { safe } else { split_at }
}

fn unescaped_backtick_count(text: &str) -> usize {
    let mut count = 0usize;
    let mut escaped = false;
    for ch in text.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == '`' {
            count += 1;
        }
    }
    count
}

fn find_last_unescaped_backtick(text: &str) -> Option<usize> {
    let mut found = None;
    let mut escaped = false;
    for (idx, ch) in text.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == '`' {
            found = Some(idx);
        }
    }
    found
}

fn fence_state(carry_lang: Option<&str>, body: &str) -> (bool, Option<String>) {
    let mut in_code = carry_lang.is_some();
    let mut lang = carry_lang.unwrap_or_default().to_string();
    for line in body.split('\n') {
        let stripped = line.trim();
        if !stripped.starts_with("```") {
            continue;
        }
        if in_code {
            in_code = false;
            lang.clear();
        } else {
            in_code = true;
            lang = stripped[3..]
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_string();
        }
    }
    (in_code, in_code.then_some(lang))
}

fn append_indicators(chunks: Vec<String>) -> Vec<String> {
    let total = chunks.len();
    chunks
        .into_iter()
        .enumerate()
        .map(|(idx, chunk)| format!("{chunk} ({}/{total})", idx + 1))
        .collect()
}

#[cfg(test)]
#[path = "test.rs"]
mod tests;
