//! Debug-build wrappers for raw integration tests that drive the Discord
//! REST helpers against loopback servers.

use super::*;

pub fn format_discord_http_error_for_test(
    endpoint: &str,
    status: reqwest::StatusCode,
    body: &str,
) -> String {
    format_discord_http_error(endpoint, status, body)
}

pub async fn list_bot_guilds_at_base_for_test(
    base: &str,
    token: &str,
) -> anyhow::Result<Vec<DiscordGuild>> {
    list_bot_guilds_at_base(base, token).await
}

pub async fn list_guild_channels_at_base_for_test(
    base: &str,
    token: &str,
    guild_id: &str,
) -> anyhow::Result<Vec<DiscordTextChannel>> {
    list_guild_channels_at_base(base, token, guild_id).await
}

pub async fn check_channel_permissions_at_base_for_test(
    base: &str,
    token: &str,
    guild_id: &str,
    channel_id: &str,
) -> anyhow::Result<BotPermissionCheck> {
    check_channel_permissions_at_base(base, token, guild_id, channel_id).await
}
