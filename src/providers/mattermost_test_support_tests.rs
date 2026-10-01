//! Debug-build seams for raw integration tests. These expose Mattermost's
//! private parser helpers without widening the production API surface.

use super::*;

pub fn parse_mattermost_post_for_test(
    channel: &MattermostChannel,
    post: &serde_json::Value,
    bot_user_id: &str,
    bot_username: &str,
    last_create_at: i64,
    channel_id: &str,
) -> Option<ChannelMessage> {
    channel.parse_mattermost_post(post, bot_user_id, bot_username, last_create_at, channel_id)
}

pub fn contains_bot_mention_for_test(
    text: &str,
    bot_user_id: &str,
    bot_username: &str,
    post: &serde_json::Value,
) -> bool {
    contains_bot_mention_mm(text, bot_user_id, bot_username, post)
}

pub fn normalize_mattermost_content_for_test(
    text: &str,
    bot_user_id: &str,
    bot_username: &str,
    post: &serde_json::Value,
) -> Option<String> {
    normalize_mattermost_content(text, bot_user_id, bot_username, post)
}
