//! Telegram channel — long-polls the Bot API for updates.
//!
//! This is the **transport** half of the Telegram provider, ported into
//! TinyChannels. Host glue (remote control, event-bus subscribers, approval
//! surface) stays in OpenHuman and re-exports [`TelegramChannel`] from here.

mod attachments;
mod channel;
mod channel_core;
mod channel_ops;
mod channel_recv;
mod channel_send;
mod channel_types;
pub mod session_store;
mod text;

pub use channel_types::TelegramChannel;

#[cfg(any(test, debug_assertions))]
pub mod test_support {
    //! Debug-build seams for raw integration coverage of Telegram send helpers.

    use super::TelegramChannel;

    pub fn parse_reaction_marker_for_test(content: &str) -> (String, Option<String>) {
        TelegramChannel::parse_reaction_marker(content)
    }
}
