//! Per-provider interaction capabilities.
//!
//! A host used to answer questions like "can this channel show an approval
//! prompt?" or "does this channel understand `/status`?" by comparing the
//! channel name with `"telegram"`. [`capabilities_for`] is the single table
//! those answers come from, and [`crate::Channel::capabilities`] exposes it on
//! every channel instance.
//!
//! The table is keyed by provider id. Inbound ids from a socket relay arrive
//! provider-prefixed (`telegram:<chat>`, `tg:<chat>`), so lookups strip
//! everything from the first `:` and resolve aliases first.
//!
//! Unknown providers get [`ChannelCapabilities::NONE`]: a new adapter must opt
//! in explicitly rather than inherit behaviour it may not support.

/// What a channel can do beyond plain send/receive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ChannelCapabilities {
    /// The channel carries free-text replies from the user who started the
    /// turn, so remote-control slash commands (`/status`, `/sessions`, `/new`,
    /// `/help`) can be offered on it.
    pub remote_control: bool,
    /// The channel can deliver an approval prompt into the originating chat and
    /// read a `yes`/`no` reply back, so a supervised tool call can be approved
    /// in-conversation instead of bypassing the gate.
    pub chat_approvals: bool,
    /// The provider backend supports both editing and deleting a posted
    /// message, which progressive draft, thinking and filler bubbles need.
    pub progressive_edits: bool,
    /// `thread_ts` is a reply target (Telegram topics), not a separate
    /// conversation, so it is left out of the conversation history key.
    pub history_key_ignores_thread: bool,
}

impl ChannelCapabilities {
    /// No optional capability. The default for unknown providers.
    pub const NONE: Self = Self {
        remote_control: false,
        chat_approvals: false,
        progressive_edits: false,
        history_key_ignores_thread: false,
    };

    /// A conversational chat provider: remote control and chat approvals.
    pub const CHAT: Self = Self {
        remote_control: true,
        chat_approvals: true,
        progressive_edits: false,
        history_key_ignores_thread: false,
    };
}

/// Map a channel id (optionally provider-prefixed, optionally an alias) to its
/// canonical provider id: `"tg:123"` → `"telegram"`, `"discord:g/c"` →
/// `"discord"`.
pub fn provider_id(channel: &str) -> &str {
    let provider = channel.split(':').next().unwrap_or(channel);
    match provider {
        "tg" => "telegram",
        other => other,
    }
}

/// Capabilities of the provider behind `channel`. See the module docs for the
/// lookup rules.
pub fn capabilities_for(channel: &str) -> ChannelCapabilities {
    match provider_id(channel) {
        "telegram" => ChannelCapabilities {
            progressive_edits: true,
            history_key_ignores_thread: true,
            ..ChannelCapabilities::CHAT
        },
        "discord" | "slack" | "mattermost" | "imessage" | "signal" | "whatsapp"
        | "whatsapp_web" | "irc" | "dingtalk" | "qq" | "lark" | "linq" | "yuanbao" => {
            ChannelCapabilities::CHAT
        }
        // Email and the CLI have no conversational reply path the approval
        // surface can rely on; webhooks are one-way.
        _ => ChannelCapabilities::NONE,
    }
}

#[cfg(test)]
#[path = "capabilities_tests.rs"]
mod test;
