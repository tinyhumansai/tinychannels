//! Per-sender route selection and runtime command helpers.

use crate::context::ChannelRouteSelection;
use serde::Deserialize;
use std::fmt::Write;
use std::path::Path;

const MODEL_CACHE_FILE: &str = "models_cache.json";
const MODEL_CACHE_PREVIEW_LIMIT: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelRuntimeCommand {
    ShowProviders,
    SetProvider(String),
    ShowModel,
    SetModel(String),
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ModelCacheState {
    entries: Vec<ModelCacheEntry>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ModelCacheEntry {
    provider: String,
    models: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderDescriptor {
    pub name: String,
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct RouteCommandContext {
    pub default_route: ChannelRouteSelection,
    pub providers: Vec<ProviderDescriptor>,
}

pub fn supports_runtime_model_switch(channel_name: &str) -> bool {
    matches!(channel_name, "telegram" | "discord")
}

pub fn parse_runtime_command(channel_name: &str, content: &str) -> Option<ChannelRuntimeCommand> {
    let trimmed = content.trim();
    if !trimmed.starts_with('/') || !supports_runtime_model_switch(channel_name) {
        return None;
    }

    let mut parts = trimmed.split_whitespace();
    let command_token = parts.next()?;
    let base_command = command_token
        .split('@')
        .next()
        .unwrap_or(command_token)
        .to_ascii_lowercase();

    match base_command.as_str() {
        "/models" => {
            if let Some(provider) = parts.next() {
                Some(ChannelRuntimeCommand::SetProvider(
                    provider.trim().to_string(),
                ))
            } else {
                Some(ChannelRuntimeCommand::ShowProviders)
            }
        }
        "/model" => {
            let model = parts.collect::<Vec<_>>().join(" ").trim().to_string();
            if model.is_empty() {
                Some(ChannelRuntimeCommand::ShowModel)
            } else {
                Some(ChannelRuntimeCommand::SetModel(model))
            }
        }
        _ => None,
    }
}

pub fn resolve_provider_alias(name: &str, providers: &[ProviderDescriptor]) -> Option<String> {
    let candidate = name.trim();
    if candidate.is_empty() {
        return None;
    }

    providers
        .iter()
        .find(|provider| {
            provider.name.eq_ignore_ascii_case(candidate)
                || provider
                    .aliases
                    .iter()
                    .any(|alias| alias.eq_ignore_ascii_case(candidate))
        })
        .map(|provider| provider.name.clone())
}

pub fn load_cached_model_preview(workspace_dir: &Path, provider_name: &str) -> Vec<String> {
    let cache_path = workspace_dir.join("state").join(MODEL_CACHE_FILE);
    let Ok(raw) = std::fs::read_to_string(cache_path) else {
        return Vec::new();
    };
    let Ok(state) = serde_json::from_str::<ModelCacheState>(&raw) else {
        return Vec::new();
    };

    state
        .entries
        .into_iter()
        .find(|entry| entry.provider == provider_name)
        .map(|entry| {
            entry
                .models
                .into_iter()
                .take(MODEL_CACHE_PREVIEW_LIMIT)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

pub fn build_models_help_response(current: &ChannelRouteSelection, workspace_dir: &Path) -> String {
    let mut response = String::new();
    let _ = writeln!(
        response,
        "Current provider: `{}`\nCurrent model: `{}`",
        current.provider, current.model
    );
    response.push_str("\nSwitch model with `/model <model-id>`.\n");

    let cached_models = load_cached_model_preview(workspace_dir, &current.provider);
    if cached_models.is_empty() {
        let _ = writeln!(
            response,
            "\nNo cached model list found for `{}`. Ask the operator to refresh the model list in the web UI.",
            current.provider
        );
    } else {
        let _ = writeln!(
            response,
            "\nCached model IDs (top {}):",
            cached_models.len()
        );
        for model in cached_models {
            let _ = writeln!(response, "- `{model}`");
        }
    }

    response
}

pub fn build_providers_help_response(
    current: &ChannelRouteSelection,
    providers: &[ProviderDescriptor],
) -> String {
    let mut response = String::new();
    let _ = writeln!(
        response,
        "Current provider: `{}`\nCurrent model: `{}`",
        current.provider, current.model
    );
    response.push_str("\nSwitch provider with `/models <provider>`.\n");
    response.push_str("Switch model with `/model <model-id>`.\n\n");
    response.push_str("Available providers:\n");
    for provider in providers {
        if provider.aliases.is_empty() {
            let _ = writeln!(response, "- {}", provider.name);
        } else {
            let _ = writeln!(
                response,
                "- {} (aliases: {})",
                provider.name,
                provider.aliases.join(", ")
            );
        }
    }
    response
}

#[cfg(test)]
mod tests {
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
}
