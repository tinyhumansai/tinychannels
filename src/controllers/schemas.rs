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
mod tests {
    use super::*;

    fn required(schema: &ChannelControllerSchema) -> Vec<&'static str> {
        schema
            .inputs
            .iter()
            .filter(|field| field.required)
            .map(|field| field.name)
            .collect()
    }

    #[test]
    fn all_schemas_are_in_channels_namespace() {
        for schema in all_channel_controller_schemas() {
            assert_eq!(schema.namespace, "channels");
        }
    }

    #[test]
    fn all_schemas_have_unique_functions() {
        let schemas = all_channel_controller_schemas();
        let mut functions: Vec<&str> = schemas.iter().map(|schema| schema.function).collect();
        let len = functions.len();
        functions.sort();
        functions.dedup();
        assert_eq!(functions.len(), len);
    }

    #[test]
    fn every_registered_key_resolves_to_schema() {
        for schema in all_channel_controller_schemas() {
            let resolved = channel_controller_schema(schema.function);
            assert_eq!(resolved.namespace, "channels");
            assert_ne!(resolved.function, "unknown");
            assert!(!resolved.description.is_empty());
            assert!(!resolved.outputs.is_empty());
        }
    }

    #[test]
    fn unknown_function_returns_unknown_fallback() {
        let schema = channel_controller_schema("no_such_fn_123");
        assert_eq!(schema.function, "unknown");
        assert_eq!(schema.namespace, "channels");
    }

    #[test]
    fn required_inputs_match_controller_contracts() {
        let cases = [
            ("describe", vec!["channel"]),
            ("connect", vec!["channel", "authMode"]),
            ("disconnect", vec!["channel", "authMode"]),
            ("test", vec!["channel", "authMode", "credentials"]),
            ("telegram_login_check", vec!["linkToken"]),
            ("discord_link_check", vec!["linkToken"]),
            ("discord_list_channels", vec!["guildId"]),
            ("discord_check_permissions", vec!["guildId", "channelId"]),
            ("send_message", vec!["channel", "message"]),
            ("send_reaction", vec!["channel", "reaction"]),
            ("create_thread", vec!["channel", "title"]),
            ("update_thread", vec!["channel", "threadId", "action"]),
            ("list_threads", vec!["channel"]),
        ];

        for (function, expected) in cases {
            let schema = channel_controller_schema(function);
            assert_eq!(required(&schema), expected, "{function}");
        }
    }

    #[test]
    fn optional_and_empty_inputs_match_controller_contracts() {
        let list = channel_controller_schema("list");
        assert!(list.inputs.is_empty());

        let telegram_start = channel_controller_schema("telegram_login_start");
        assert!(telegram_start.inputs.is_empty());

        let discord_guilds = channel_controller_schema("discord_list_guilds");
        assert!(discord_guilds.inputs.is_empty());

        let status = channel_controller_schema("status");
        let channel = status.inputs.iter().find(|field| field.name == "channel");
        assert!(channel.is_some_and(|field| !field.required));
    }

    #[test]
    fn field_helpers_set_requiredness_and_types() {
        let required = required_string("channel", "channel name");
        assert!(required.required);
        assert_eq!(required.ty, ChannelControllerFieldType::String);

        let optional = optional_string("channel", "channel name");
        assert!(!optional.required);
        assert_eq!(optional.ty, ChannelControllerFieldType::String);

        let output = json_output("result", "the result");
        assert!(output.required);
        assert_eq!(output.ty, ChannelControllerFieldType::Json);
    }
}
