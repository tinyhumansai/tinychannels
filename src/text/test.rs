use super::*;

fn strip_indicators(chunks: &[String]) -> Vec<String> {
    chunks
        .iter()
        .map(|chunk| {
            chunk
                .rsplit_once(" (")
                .map(|(body, _)| body.to_string())
                .unwrap_or_else(|| chunk.clone())
        })
        .collect()
}

fn fences_balanced(chunk: &str) -> bool {
    chunk.matches("```").count().is_multiple_of(2)
}

#[test]
fn measures_utf16_units_for_astral_characters() {
    assert_eq!(measure_text("hello", LengthUnit::Utf16Units), 5);
    assert_eq!(measure_text("你好", LengthUnit::Utf16Units), 2);
    assert_eq!(measure_text("😀", LengthUnit::Utf16Units), 2);
    assert_eq!(measure_text("hi😀", LengthUnit::Utf16Units), 4);
    assert_eq!(measure_text("𝄞", LengthUnit::Utf16Units), 2);
}

#[test]
fn prefix_within_utf16_limit_does_not_split_surrogate_pairs() {
    assert_eq!(
        prefix_within_limit("hello world", 5, LengthUnit::Utf16Units),
        "hello"
    );
    assert_eq!(prefix_within_limit("a😀b", 2, LengthUnit::Utf16Units), "a");
    assert_eq!(prefix_within_limit("😀x", 2, LengthUnit::Utf16Units), "😀");
    assert_eq!(
        prefix_within_limit(&"😀".repeat(10), 6, LengthUnit::Utf16Units),
        "😀😀😀"
    );
}

#[test]
fn utf16_chunking_splits_emoji_case_that_character_count_would_pass() {
    let text = "😀".repeat(2049);
    assert_eq!(measure_text(&text, LengthUnit::Characters), 2049);
    assert_eq!(measure_text(&text, LengthUnit::Utf16Units), 4098);

    let char_chunks = chunk_text_with_options(
        &text,
        TextChunkOptions {
            limit: 4096,
            length_unit: LengthUnit::Characters,
            indicators: false,
            ..Default::default()
        },
    );
    assert_eq!(char_chunks.len(), 1);

    let utf16_chunks = chunk_text_with_options(
        &text,
        TextChunkOptions {
            limit: 4096,
            length_unit: LengthUnit::Utf16Units,
            indicators: true,
            ..Default::default()
        },
    );
    assert!(utf16_chunks.len() > 1);
    for chunk in &utf16_chunks {
        assert!(measure_text(chunk, LengthUnit::Utf16Units) <= 4096);
    }
}

#[test]
fn length_chunking_prefers_newline_then_space_boundaries() {
    let chunks = chunk_text_with_options(
        "paragraph one line\n\nparagraph two starts here and continues",
        TextChunkOptions {
            limit: 40,
            indicators: false,
            ..Default::default()
        },
    );
    assert_eq!(
        chunks,
        vec![
            "paragraph one line",
            "paragraph two starts here and continues"
        ]
    );

    let chunks = chunk_text_with_options(
        "This is a message that should break nicely near a word boundary.",
        TextChunkOptions {
            limit: 30,
            indicators: false,
            ..Default::default()
        },
    );
    assert!(!chunks[0].ends_with("nic"));
    assert!(chunks.iter().all(|chunk| chunk.len() <= 30));
    assert_eq!(
        chunks.join(" ").replace(char::is_whitespace, " "),
        "This is a message that should break nicely near a word boundary."
    );
}

#[test]
fn chunking_avoids_inline_code_span_when_possible() {
    let chunks = chunk_text_with_options(
        "prefix words `inline code has spaces` suffix",
        TextChunkOptions {
            limit: 28,
            indicators: false,
            ..Default::default()
        },
    );
    assert_eq!(chunks[0], "prefix words");
    assert!(chunks[1].starts_with("`inline code"));
}

#[test]
fn markdown_fences_are_closed_and_reopened_across_chunks() {
    let text = format!(
        "Before\n```python\n{}{}\nAfter",
        "x = '😀'\n".repeat(80),
        "```"
    );
    let chunks = chunk_text_with_options(
        &text,
        TextChunkOptions {
            limit: 220,
            length_unit: LengthUnit::Utf16Units,
            markdown: true,
            indicators: true,
            ..Default::default()
        },
    );
    assert!(chunks.len() > 1);
    for chunk in &chunks {
        assert!(fences_balanced(chunk), "unbalanced chunk: {chunk:?}");
        assert!(measure_text(chunk, LengthUnit::Utf16Units) <= 220);
    }
    let bodies = strip_indicators(&chunks);
    assert!(bodies[1].starts_with("```python\n"));
    assert!(bodies[0].trim_end().ends_with("```"));
}

#[test]
fn newline_mode_packs_paragraphs_then_rechunks_long_paragraphs() {
    let text = "one\n\nsecond paragraph\n\nthird paragraph";
    let chunks = chunk_text_with_mode(text, 24, LengthUnit::Characters, ChunkMode::Newline);
    let bodies = strip_indicators(&chunks);
    assert_eq!(bodies, vec!["one\n\nsecond paragraph", "third paragraph"]);

    let long = "alpha\n\n".to_string() + &"x".repeat(50);
    let chunks = chunk_text_with_options(
        &long,
        TextChunkOptions {
            limit: 20,
            mode: ChunkMode::Newline,
            indicators: false,
            ..Default::default()
        },
    );
    assert_eq!(chunks[0], "alpha");
    assert!(chunks[1..].iter().all(|chunk| chunk.len() <= 20));
}

#[test]
fn continuation_indicators_are_added_for_multi_chunk_messages() {
    let chunks = chunk_text("aa bb cc dd ee", 12);
    assert!(chunks.len() > 1);
    let total = chunks.len();
    for (index, chunk) in chunks.iter().enumerate() {
        assert!(chunk.ends_with(&format!("({}/{total})", index + 1)));
    }
}

#[test]
fn resolves_text_chunk_limit_with_overrides() {
    assert_eq!(resolve_text_chunk_limit(Some(1234), None), 1234);
    assert_eq!(resolve_text_chunk_limit(Some(0), Some(2000)), 2000);
    assert_eq!(
        resolve_text_chunk_limit(None, None),
        DEFAULT_TEXT_CHUNK_LIMIT
    );
}

#[test]
fn truncate_with_ellipsis_only_marks_truncated_text() {
    assert_eq!(truncate_with_ellipsis("short", 10), "short");
    assert_eq!(truncate_with_ellipsis("abcdef", 3), "abc…");
}
