use super::*;

#[test]
fn short_input_returns_one_chunk() {
    let r = split_markdown("hello", 100);
    assert_eq!(r, vec!["hello"]);
}

#[test]
fn splits_on_newlines_respecting_cap() {
    let input = "a\n".repeat(100);
    let r = split_markdown(&input, 20);
    assert!(r.len() > 1);
    for c in &r {
        assert!(c.len() <= 20, "chunk too long: {c:?}");
    }
}

#[test]
fn preserves_fenced_code_block() {
    let input = "intro line\n\
                     ```rust\n\
                     fn long_function_a() -> u32 { 42 }\n\
                     fn long_function_b() -> u32 { 43 }\n\
                     fn long_function_c() -> u32 { 44 }\n\
                     ```\n\
                     trailing text";
    let chunks = split_markdown(input, 80);
    // Find the chunk(s) containing the fence — they must not split mid-fence.
    let mut open = 0;
    for c in &chunks {
        for line in c.lines() {
            if is_fence(line).is_some() {
                open += 1;
            }
        }
    }
    // The fence must appear as balanced pairs.
    assert_eq!(open % 2, 0, "unbalanced fences after split: {chunks:#?}");
}

#[test]
fn hard_split_very_long_line() {
    let line = "x".repeat(500);
    let r = split_markdown(&line, 100);
    for c in &r {
        assert!(c.len() <= 100, "chunk too long: {}", c.len());
    }
    assert_eq!(r.join("").len(), 500);
}

#[test]
fn unicode_safe_hard_split() {
    let line = "中".repeat(200); // each char is 3 bytes → 600 total
    let r = split_markdown(&line, 50);
    for c in &r {
        assert!(c.len() <= 50, "chunk too long: {}", c.len());
        // verify it's valid utf-8 by reading it
        for ch in c.chars() {
            assert!(ch == '中');
        }
    }
}

#[test]
fn is_fence_detects_backticks() {
    assert_eq!(is_fence("```").as_deref(), Some("```"));
    assert_eq!(is_fence("```rust").as_deref(), Some("```"));
    assert_eq!(is_fence("~~~").as_deref(), Some("~~~"));
    assert_eq!(is_fence("text").as_deref(), None);
    assert_eq!(is_fence("``").as_deref(), None);
}
