//! Module-owned preparation of command, timing, connection and presentation facts.
use crate::host::HostChannels;
use tinybus::{Error, Result};
use tinychannels_bus::module_control::{
    ChannelControlRequest as Request, ChannelControlResult as Response,
};
use tinychannels_bus::module_remote::RemoteOperation;

pub(crate) async fn prepare(
    host: &HostChannels,
    request: Request,
    instance_capabilities: tinychannels_bus::ChannelCapabilities,
) -> Result<Response> {
    if serde_json::to_vec(&request).map_or(true, |bytes| {
        bytes.len() > tinychannels_bus::module_runtime::PREPARATION_MESSAGE_BYTES
    }) {
        return Err(Error::failed("channel control request exceeds byte limit"));
    }
    let result = match request {
        Request::RuntimeLimits {
            timeout_secs,
            listener_count,
        } => Response::RuntimeLimits {
            timeout_secs: tinychannels::context::effective_channel_message_timeout_secs(
                timeout_secs,
            ),
            max_in_flight: tinychannels::runtime::compute_max_in_flight_messages(listener_count),
        },
        Request::Connected {
            configured,
            channel,
            mode,
        } => Response::Connected(tinychannels::controllers::channel_presence_connected(
            &configured,
            &channel,
            mode,
        )),
        Request::Overflow { message } => Response::Overflow(
            tinychannels::context::is_context_window_overflow_message(&message),
        ),
        Request::Capabilities { .. } => Response::Capabilities((&instance_capabilities).into()),
        Request::CapabilitiesV2 { .. } => {
            Response::CapabilitiesV2(Box::new(instance_capabilities.clone()))
        }
        Request::Acknowledgment { content } => Response::Acknowledgment(
            tinychannels::runtime::select_acknowledgment_reaction(&content).to_owned(),
        ),
        Request::Approval {
            tool_name,
            action_summary,
        } => Response::Approval(tinychannels::approvals::format_approval_prompt(
            &tool_name,
            &action_summary,
        )),
        Request::Route {
            workspace_dir,
            message,
            route,
            history_len,
            session_scope,
            configured_channel: _,
            providers,
            ..
        } => {
            let remote = crate::remote::execute(
                host,
                RemoteOperation::Execute {
                    workspace_dir: workspace_dir.clone(),
                    message: Box::new((*message).clone()),
                    route: route.clone(),
                    history_len,
                    session_scope: session_scope.map(|scope| *scope),
                },
                instance_capabilities.clone(),
            )
            .await?;
            if remote.reply.is_some() {
                Response::Route {
                    reply: remote.reply,
                    next_route: None,
                    clear_history: false,
                }
            } else {
                use tinychannels::routes::{
                    ChannelRuntimeCommand as Command, build_models_help_response,
                    build_providers_help_response, parse_runtime_command, resolve_provider_alias,
                };
                let current = tinychannels_bus::context::ChannelRouteSelection {
                    provider: route.provider.clone(),
                    model: route.model.clone(),
                };
                let mut next_route = None;
                let mut clear_history = false;
                let reply = match parse_runtime_command(&message.channel, &message.content) {
                    None => None,
                    Some(Command::ShowProviders) => {
                        Some(build_providers_help_response(&current, &providers))
                    }
                    Some(Command::ShowModel) => Some(build_models_help_response(
                        &current,
                        std::path::Path::new(&workspace_dir),
                    )),
                    Some(Command::SetProvider(raw_provider)) => {
                        Some(match resolve_provider_alias(&raw_provider, &providers) {
                            Some(provider) => {
                                if provider != route.provider {
                                    next_route =
                                        Some(tinychannels_bus::module_remote::RemoteRoute {
                                            provider: provider.clone(),
                                            model: route.model.clone(),
                                        });
                                    clear_history = true;
                                }
                                format!(
                                    "Provider switched to `{provider}` for this sender session. Current model is `{}`.\nUse `/model <model-id>` to set a provider-compatible model.",
                                    route.model
                                )
                            }
                            None => format!(
                                "Unknown provider `{raw_provider}`. Use `/models` to list valid providers."
                            ),
                        })
                    }
                    Some(Command::SetModel(raw_model)) => {
                        let model = raw_model.trim().trim_matches('`').to_string();
                        Some(if model.is_empty() {
                            "Model ID cannot be empty. Use `/model <model-id>`.".to_owned()
                        } else {
                            next_route = Some(tinychannels_bus::module_remote::RemoteRoute {
                                provider: route.provider.clone(),
                                model: model.clone(),
                            });
                            clear_history = true;
                            format!(
                                "Model switched to `{model}` for provider `{}` in this sender session.",
                                route.provider
                            )
                        })
                    }
                };
                Response::Route {
                    reply,
                    next_route,
                    clear_history,
                }
            }
        }
    };
    if serde_json::to_vec(&result).map_or(true, |bytes| {
        bytes.len() > tinychannels_bus::module_runtime::PREPARATION_MESSAGE_BYTES
    }) {
        return Err(Error::failed("channel control result exceeds byte limit"));
    }
    Ok(result)
}
