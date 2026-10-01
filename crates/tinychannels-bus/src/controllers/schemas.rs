//! Portable channel controller schema catalog.

use serde::Serialize;

/// Transport-agnostic schema for one channel controller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChannelControllerSchema {
    pub namespace: &'static str,
    pub function: &'static str,
    pub description: &'static str,
    pub inputs: Vec<ChannelControllerField>,
    pub outputs: Vec<ChannelControllerField>,
}

/// Input or output field in a channel controller schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChannelControllerField {
    pub name: &'static str,
    pub ty: ChannelControllerFieldType,
    pub comment: &'static str,
    pub required: bool,
}

/// Field type for a channel controller schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum ChannelControllerFieldType {
    Bool,
    I64,
    U64,
    F64,
    String,
    Json,
    Option(Box<ChannelControllerFieldType>),
}

/// Return all channel controller schemas in the canonical registration order.
pub fn all_channel_controller_schemas() -> Vec<ChannelControllerSchema> {
    [
        "list",
        "describe",
        "connect",
        "disconnect",
        "status",
        "set_default",
        "get_default",
        "test",
        "telegram_login_start",
        "telegram_login_check",
        "discord_link_start",
        "discord_link_check",
        "discord_list_guilds",
        "discord_list_channels",
        "discord_check_permissions",
        "send_message",
        "send_reaction",
        "create_thread",
        "update_thread",
        "list_threads",
    ]
    .into_iter()
    .map(channel_controller_schema)
    .collect()
}

/// Return the schema for one channel controller function.
pub fn channel_controller_schema(function: &str) -> ChannelControllerSchema {
    match function {
        "list" => ChannelControllerSchema {
            namespace: "channels",
            function: "list",
            description: "List all available channel definitions.",
            inputs: vec![],
            outputs: vec![json_output("channels", "Array of channel definitions.")],
        },
        "describe" => ChannelControllerSchema {
            namespace: "channels",
            function: "describe",
            description: "Get the full definition for a single channel.",
            inputs: vec![required_string(
                "channel",
                "Channel identifier (e.g. telegram).",
            )],
            outputs: vec![json_output(
                "definition",
                "Channel definition with auth modes and capabilities.",
            )],
        },
        "connect" => ChannelControllerSchema {
            namespace: "channels",
            function: "connect",
            description: "Initiate a channel connection.",
            inputs: vec![
                required_string("channel", "Channel identifier."),
                required_string(
                    "authMode",
                    "Auth mode (api_key, bot_token, oauth, managed_dm).",
                ),
                optional_json("credentials", "Credential fields for the chosen auth mode."),
            ],
            outputs: vec![json_output(
                "result",
                "Connection result with status and optional auth action.",
            )],
        },
        "disconnect" => ChannelControllerSchema {
            namespace: "channels",
            function: "disconnect",
            description: "Disconnect a channel and optionally remove source-scoped memory.",
            inputs: vec![
                required_string("channel", "Channel identifier."),
                required_string("authMode", "Auth mode to disconnect."),
                ChannelControllerField {
                    name: "clearMemory",
                    ty: ChannelControllerFieldType::Bool,
                    comment: "When true, delete memory chunks ingested from this channel.",
                    required: false,
                },
            ],
            outputs: vec![json_output("result", "Disconnect result.")],
        },
        "status" => ChannelControllerSchema {
            namespace: "channels",
            function: "status",
            description: "Get connection status for one or all channels.",
            inputs: vec![optional_string("channel", "Optional channel filter.")],
            outputs: vec![json_output(
                "entries",
                "Array of status entries per channel and auth mode.",
            )],
        },
        "set_default" => ChannelControllerSchema {
            namespace: "channels",
            function: "set_default",
            description: "Set the default messaging channel for proactive agent delivery (persists active_channel + applies live).",
            inputs: vec![required_string(
                "channel",
                "Channel identifier to make default (e.g. telegram, discord, web).",
            )],
            outputs: vec![json_output(
                "result",
                "Object with the new active_channel and restart_required flag.",
            )],
        },
        "get_default" => ChannelControllerSchema {
            namespace: "channels",
            function: "get_default",
            description: "Get the persisted default messaging channel.",
            inputs: vec![],
            outputs: vec![json_output(
                "result",
                "Object with the current active_channel.",
            )],
        },
        "test" => ChannelControllerSchema {
            namespace: "channels",
            function: "test",
            description: "Test a channel connection without persisting credentials.",
            inputs: vec![
                required_string("channel", "Channel identifier."),
                required_string("authMode", "Auth mode to test."),
                required_json("credentials", "Credential fields to test."),
            ],
            outputs: vec![json_output(
                "result",
                "Test result with success flag and message.",
            )],
        },
        "telegram_login_start" => ChannelControllerSchema {
            namespace: "channels",
            function: "telegram_login_start",
            description: "Create a Telegram link token and return the deep link URL for managed DM login.",
            inputs: vec![],
            outputs: vec![json_output(
                "result",
                "Object with linkToken, telegramUrl, and botUsername.",
            )],
        },
        "telegram_login_check" => ChannelControllerSchema {
            namespace: "channels",
            function: "telegram_login_check",
            description: "Check whether the Telegram managed DM link has been completed.",
            inputs: vec![required_string(
                "linkToken",
                "The link token returned by telegram_login_start.",
            )],
            outputs: vec![json_output(
                "result",
                "Object with linked (bool) and optional details.",
            )],
        },
        "discord_link_start" => ChannelControllerSchema {
            namespace: "channels",
            function: "discord_link_start",
            description: "Create a Discord link token the user pastes into Discord as `!start <token>` to link their account.",
            inputs: vec![],
            outputs: vec![json_output(
                "result",
                "Object with linkToken and instructions.",
            )],
        },
        "discord_link_check" => ChannelControllerSchema {
            namespace: "channels",
            function: "discord_link_check",
            description: "Check whether the Discord managed link has been completed (discordId set on user profile).",
            inputs: vec![required_string(
                "linkToken",
                "The link token returned by discord_link_start.",
            )],
            outputs: vec![json_output(
                "result",
                "Object with linked (bool) and optional details.",
            )],
        },
        "discord_list_guilds" => ChannelControllerSchema {
            namespace: "channels",
            function: "discord_list_guilds",
            description: "List Discord servers (guilds) the connected bot is a member of.",
            inputs: vec![],
            outputs: vec![json_output(
                "guilds",
                "Array of guild objects with id, name, and icon.",
            )],
        },
        "discord_list_channels" => ChannelControllerSchema {
            namespace: "channels",
            function: "discord_list_channels",
            description: "List text channels in a Discord guild.",
            inputs: vec![required_string("guildId", "The Discord guild (server) ID.")],
            outputs: vec![json_output(
                "channels",
                "Array of text channel objects with id, name, position, and parentId.",
            )],
        },
        "discord_check_permissions" => ChannelControllerSchema {
            namespace: "channels",
            function: "discord_check_permissions",
            description: "Check bot permissions in a Discord channel.",
            inputs: vec![
                required_string("guildId", "The Discord guild (server) ID."),
                required_string("channelId", "The Discord channel ID to check."),
            ],
            outputs: vec![json_output(
                "permissions",
                "Permission check result with flags and missing permissions.",
            )],
        },
        "send_message" => ChannelControllerSchema {
            namespace: "channels",
            function: "send_message",
            description: "Send a rich message to a channel (text, photo, sticker, animation, buttons, reply).",
            inputs: vec![
                required_string("channel", "Channel identifier (e.g. telegram)."),
                required_json(
                    "message",
                    "Message body with optional fields: text, parseMode, photoUrl, stickerFileId, animationUrl, buttons, replyToMessageId, threadId.",
                ),
            ],
            outputs: vec![json_output(
                "result",
                "Object with success flag and optional messageId.",
            )],
        },
        "send_reaction" => ChannelControllerSchema {
            namespace: "channels",
            function: "send_reaction",
            description: "React to a message in a channel with an emoji.",
            inputs: vec![
                required_string("channel", "Channel identifier (e.g. telegram)."),
                required_json("reaction", "Reaction body: { messageId, emoji, chatId? }."),
            ],
            outputs: vec![json_output("result", "Object with success flag.")],
        },
        "create_thread" => ChannelControllerSchema {
            namespace: "channels",
            function: "create_thread",
            description: "Create a new thread in a channel.",
            inputs: vec![
                required_string("channel", "Channel identifier (e.g. telegram)."),
                required_string("title", "Thread title."),
            ],
            outputs: vec![json_output(
                "result",
                "Object with success flag and optional threadId.",
            )],
        },
        "update_thread" => ChannelControllerSchema {
            namespace: "channels",
            function: "update_thread",
            description: "Close or reopen a thread in a channel.",
            inputs: vec![
                required_string("channel", "Channel identifier (e.g. telegram)."),
                required_string("threadId", "Thread identifier."),
                required_string("action", "Action: close or reopen."),
            ],
            outputs: vec![json_output("result", "Object with success flag.")],
        },
        "list_threads" => ChannelControllerSchema {
            namespace: "channels",
            function: "list_threads",
            description: "List threads in a channel, optionally filtered by active status.",
            inputs: vec![
                required_string("channel", "Channel identifier (e.g. telegram)."),
                ChannelControllerField {
                    name: "active",
                    ty: ChannelControllerFieldType::Option(Box::new(
                        ChannelControllerFieldType::Bool,
                    )),
                    comment: "Optional filter: true for active threads, false for closed threads.",
                    required: false,
                },
            ],
            outputs: vec![json_output("result", "Array of thread objects.")],
        },
        _ => ChannelControllerSchema {
            namespace: "channels",
            function: "unknown",
            description: "Unknown channels controller function.",
            inputs: vec![],
            outputs: vec![ChannelControllerField {
                name: "error",
                ty: ChannelControllerFieldType::String,
                comment: "Lookup error details.",
                required: true,
            }],
        },
    }
}

fn required_string(name: &'static str, comment: &'static str) -> ChannelControllerField {
    ChannelControllerField {
        name,
        ty: ChannelControllerFieldType::String,
        comment,
        required: true,
    }
}

fn optional_string(name: &'static str, comment: &'static str) -> ChannelControllerField {
    ChannelControllerField {
        name,
        ty: ChannelControllerFieldType::String,
        comment,
        required: false,
    }
}

fn required_json(name: &'static str, comment: &'static str) -> ChannelControllerField {
    ChannelControllerField {
        name,
        ty: ChannelControllerFieldType::Json,
        comment,
        required: true,
    }
}

fn optional_json(name: &'static str, comment: &'static str) -> ChannelControllerField {
    ChannelControllerField {
        name,
        ty: ChannelControllerFieldType::Json,
        comment,
        required: false,
    }
}

fn json_output(name: &'static str, comment: &'static str) -> ChannelControllerField {
    ChannelControllerField {
        name,
        ty: ChannelControllerFieldType::Json,
        comment,
        required: true,
    }
}

#[cfg(test)]
#[path = "schemas_tests.rs"]
mod tests;
