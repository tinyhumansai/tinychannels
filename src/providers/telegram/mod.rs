//! Telegram channel — long-polls the Bot API for updates.
//!
//! This is the Telegram transport. Remote control and in-chat approvals are
//! provider-independent and live in [`crate::remote`] and
//! [`crate::approvals`]; the names below keep the older Telegram-specific
//! paths compiling.

mod attachments;
mod channel;
mod channel_core;
mod channel_ops;
mod channel_recv;
mod channel_send;
mod channel_types;
mod text;

pub use channel_types::TelegramChannel;

/// Approval-context client id for Telegram turns: the channel name, as for
/// every channel.
pub const TELEGRAM_APPROVAL_CLIENT_ID: &str = "telegram";

pub use crate::approvals::format_approval_prompt;
pub use crate::remote::{
    RemoteCommand as TelegramRemoteCommand, SESSIONS_LIST_LIMIT, build_new_session_response,
    build_remote_help_response, build_status_response, format_session_line,
    parse_remote_command as parse_telegram_remote_command,
};

#[cfg(any(test, debug_assertions))]
pub mod test_support {
    //! Debug-build seams for raw integration coverage of Telegram send helpers.

    use super::TelegramChannel;

    pub fn parse_reaction_marker_for_test(content: &str) -> (String, Option<String>) {
        TelegramChannel::parse_reaction_marker(content)
    }
}
