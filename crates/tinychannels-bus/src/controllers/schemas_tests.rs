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
