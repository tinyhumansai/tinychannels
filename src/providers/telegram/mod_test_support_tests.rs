//! Debug-build seams for raw integration coverage of Telegram send helpers.

use super::TelegramChannel;

pub fn parse_reaction_marker_for_test(content: &str) -> (String, Option<String>) {
    TelegramChannel::parse_reaction_marker(content)
}
