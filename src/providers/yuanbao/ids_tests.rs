use super::*;

#[test]
fn passes_short_ids_through_unchanged() {
    assert_eq!(shorten_account_id("123456"), "123456");
    assert_eq!(shorten_account_id(""), "");
    let exactly_max = "a".repeat(ACCOUNT_ID_PASSTHROUGH_MAX);
    assert_eq!(shorten_account_id(&exactly_max), exactly_max);
}

#[test]
fn shortens_long_ids_to_prefix_plus_hash() {
    let long_uid = "a".repeat(64);
    let shortened = shorten_account_id(&long_uid);
    assert_eq!(shortened.len(), 8 + 1 + 16, "8 prefix + '_' + 16 hex");
    assert!(shortened.starts_with("aaaaaaaa_"));
}

#[test]
fn shortening_is_deterministic_and_collision_resistant() {
    let a = "f".repeat(64);
    let mut b = a.clone();
    b.replace_range(63..64, "e"); // differ in last char only
    let sa = shorten_account_id(&a);
    let sb = shorten_account_id(&b);
    assert_eq!(sa, shorten_account_id(&a), "deterministic");
    assert_ne!(sa, sb, "different uids hash to different ids");
}

#[test]
fn group_reply_target_preserves_g_prefix() {
    let short_group = shorten_reply_target("g:short_group");
    assert_eq!(short_group, "g:short_group");

    let long_code = "a".repeat(64);
    let long_group = format!("g:{long_code}");
    let shortened = shorten_reply_target(&long_group);
    assert!(shortened.starts_with("g:aaaaaaaa_"));
    assert_eq!(shortened.len(), 2 + 8 + 1 + 16);
}

#[test]
fn dm_reply_target_shortens_like_account_id() {
    let uid = "z".repeat(64);
    assert_eq!(shorten_reply_target(&uid), shorten_account_id(&uid));
}

#[test]
fn shortened_thread_id_fits_under_name_max() {
    // Simulate the worst case: long uid for sender + reply_target.
    let uid = "f".repeat(64);
    let sender = shorten_account_id(&uid);
    let reply_target = shorten_account_id(&uid);
    let thread_id = format!("channel:yuanbao_{sender}_{reply_target}");
    // hex-encoded filename used by ConversationStore (`<hex>.jsonl`).
    let hex_name_len = thread_id.len() * 2 + ".jsonl".len();
    // NAME_MAX on common filesystems is 255 bytes.
    assert!(
        hex_name_len <= 255,
        "shortened thread_id hex filename ({hex_name_len} bytes) must fit under NAME_MAX (255)"
    );
}
