use super::*;

fn providers() -> Vec<ProviderDescriptor> {
    vec![
        ProviderDescriptor {
            name: "openai".into(),
            aliases: vec!["oai".into()],
        },
        ProviderDescriptor {
            name: "anthropic".into(),
            aliases: vec!["claude".into()],
        },
    ]
}

#[test]
fn runtime_command_parsing_is_channel_scoped() {
    assert!(supports_runtime_model_switch("telegram"));
    assert!(supports_runtime_model_switch("discord"));
    assert!(!supports_runtime_model_switch("slack"));

    assert_eq!(
        parse_runtime_command("telegram", "/models"),
        Some(ChannelRuntimeCommand::ShowProviders)
    );
    assert_eq!(
        parse_runtime_command("discord", "/models openai"),
        Some(ChannelRuntimeCommand::SetProvider("openai".into()))
    );
    assert_eq!(
        parse_runtime_command("telegram", "/model gpt-5"),
        Some(ChannelRuntimeCommand::SetModel("gpt-5".into()))
    );
    assert_eq!(
        parse_runtime_command("telegram", "/model"),
        Some(ChannelRuntimeCommand::ShowModel)
    );
    assert_eq!(parse_runtime_command("slack", "/models"), None);
    assert_eq!(parse_runtime_command("telegram", "hello"), None);
}

#[test]
fn provider_alias_resolution_matches_registered_names_and_aliases() {
    let providers = providers();
    assert_eq!(
        resolve_provider_alias("OPENAI", &providers).as_deref(),
        Some("openai")
    );
    assert_eq!(
        resolve_provider_alias("claude", &providers).as_deref(),
        Some("anthropic")
    );
    assert!(resolve_provider_alias("   ", &providers).is_none());
    assert!(resolve_provider_alias("missing", &providers).is_none());
}

#[test]
fn cached_models_and_help_responses_render_expected_text() {
    let tempdir = tempfile::tempdir().unwrap();
    let state_dir = tempdir.path().join("state");
    std::fs::create_dir_all(&state_dir).unwrap();
    std::fs::write(
        state_dir.join(MODEL_CACHE_FILE),
        serde_json::json!({
            "entries": [
                {
                    "provider": "openai",
                    "models": ["gpt-5", "gpt-5-mini", "gpt-4.1"]
                }
            ]
        })
        .to_string(),
    )
    .unwrap();

    let current = ChannelRouteSelection {
        provider: "openai".into(),
        model: "gpt-5".into(),
    };
    assert_eq!(
        load_cached_model_preview(tempdir.path(), "openai"),
        vec!["gpt-5", "gpt-5-mini", "gpt-4.1"]
    );
    let model_help = build_models_help_response(&current, tempdir.path());
    assert!(model_help.contains("Current provider: `openai`"));
    assert!(model_help.contains("- `gpt-5`"));

    let provider_help = build_providers_help_response(&current, &providers());
    assert!(provider_help.contains("Available providers"));
    assert!(provider_help.contains("openai"));
}
