//! Account-id shortening for yuanbao.
//!
//! Yuanbao uids (`from_account`) are 64-char hashes assigned by the platform.
//! The composite `ChannelMessage` thread_id that downstream consumers derive
//! from `sender` and `reply_target` (`channel:yuanbao_<sender>_<reply_target>`)
//! becomes ~145 chars. After the conversation store hex-encodes that for the
//! per-thread JSONL filename it grows to ~296 chars, exceeding `NAME_MAX`
//! (255 bytes) on ext4/HFS+/APFS/NTFS — writes fail with `ENAMETOOLONG` and
//! channel history is lost.
//!
//! Rather than push the filesystem limit into shared `ConversationStore` code,
//! we shorten yuanbao-specific ids at the channel boundary. Internal yuanbao
//! state (echo guard, access control, owner-command check) keeps the original
//! `from_account` — only the value emitted on `ChannelMessage.sender` /
//! `ChannelMessage.reply_target` is shortened.
//!
//! Format: `<first 8 chars of uid>_<first 16 hex chars of sha256(uid)>`.
//! The 8-char prefix keeps logs roughly groupable for the same user; the
//! sha256 suffix guarantees uniqueness across uids that share a prefix.

use sha2::{Digest, Sha256};

/// Max raw account-id length before the shortening kicks in.
///
/// Anything shorter is passed through unchanged so short upstream-style ids
/// (e.g. numeric ids, future protocol changes) keep their natural form.
const ACCOUNT_ID_PASSTHROUGH_MAX: usize = 24;

/// Shorten a yuanbao account id for use in `ChannelMessage.sender` /
/// `ChannelMessage.reply_target`. See module docs for rationale.
pub(super) fn shorten_account_id(uid: &str) -> String {
    if uid.len() <= ACCOUNT_ID_PASSTHROUGH_MAX {
        return uid.to_string();
    }
    let prefix: String = uid.chars().take(8).collect();
    let digest = hex::encode(Sha256::digest(uid.as_bytes()));
    format!("{prefix}_{}", &digest[..16])
}

/// Shorten a yuanbao `reply_target`, preserving the `g:<group_code>` shape
/// used for group chats. The `g:` discriminator is required by outbound
/// routing (see [`super::types::InboundContext::reply_target`]).
pub(super) fn shorten_reply_target(reply_target: &str) -> String {
    if let Some(group_code) = reply_target.strip_prefix("g:") {
        format!("g:{}", shorten_account_id(group_code))
    } else {
        shorten_account_id(reply_target)
    }
}

#[cfg(test)]
#[path = "ids_tests.rs"]
mod tests;
