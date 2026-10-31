use super::*;
use serde_json::json;

#[test]
fn route_control_accepts_legacy_wire_without_host_capabilities() {
    let request = module_control::ChannelControlRequest::Route {
        workspace_dir: "workspace".into(),
        message: Box::new(ChannelMessage::default()),
        capabilities: None,
        route: module_remote::RemoteRoute {
            provider: "fixture".into(),
            model: "fixture".into(),
        },
        history_len: 0,
        session_scope: None,
        configured_channel: None,
        providers: vec![],
    };
    let mut wire = serde_json::to_value(request).unwrap();
    wire["request"]
        .as_object_mut()
        .unwrap()
        .remove("capabilities");

    assert!(matches!(
        serde_json::from_value::<module_control::ChannelControlRequest>(wire).unwrap(),
        module_control::ChannelControlRequest::Route {
            capabilities: None,
            ..
        }
    ));
}

#[test]
fn relay_passthrough_body_preserves_base64_wire_and_legacy_invalid_input() {
    let wire = json!({"platform":"test", "botId":"bot", "method":"POST", "path":"/fixture", "headers":[], "bodyB64":"AP8q"});
    let decoded: relay::PassthroughForward = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(decoded.body, [0, 255, 42]);
    assert_eq!(serde_json::to_value(decoded).unwrap(), wire);
    let bad: relay::PassthroughForward =
        serde_json::from_value(json!({"bodyB64":"!invalid!"})).unwrap();
    assert!(bad.body.is_empty());
}

#[test]
fn unsupported_send_flag_is_additive_to_legacy_error_wire_shape() {
    let old_wire = json!({
        "kind": "forbidden",
        "message": "not authorized",
        "retryable": false
    });
    let decoded: channel::ChannelSendError = serde_json::from_value(old_wire).unwrap();
    assert!(!decoded.unsupported);

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct FrozenOldReader {
        kind: channel::SendErrorKind,
        message: String,
        retryable: bool,
    }
    let current = channel::ChannelSendError {
        kind: channel::SendErrorKind::BadFormat,
        message: "payload is not supported".into(),
        unsupported: true,
        ..Default::default()
    };
    let current_wire = serde_json::to_value(current).unwrap();
    let old_reader: FrozenOldReader = serde_json::from_value(current_wire).unwrap();
    assert_eq!(old_reader.kind, channel::SendErrorKind::BadFormat);
    assert_eq!(old_reader.message, "payload is not supported");
    assert!(!old_reader.retryable);
}

#[test]
fn hosted_progressive_open_keeps_scoped_address_and_is_a_new_contract_revision() {
    let command = module_progressive::ProgressiveCommand::OpenHostedOutput {
        channel: channel::ChannelRef {
            id: "telegram".into(),
            account_id: Some("relay:tenant-7:account".into()),
        },
        conversation: channel::ConversationRef {
            kind: channel::ConversationKind::Group,
            id: "chat-42".into(),
            scope_id: Some("tenant-scope".into()),
            parent_id: Some("parent-9".into()),
            thread_id: Some("thread-7".into()),
            topic_id: Some("topic-3".into()),
        },
        turn_id: "turn-1".into(),
        lifetime_ms: 30_000,
    };
    let wire = serde_json::to_value(command).unwrap();
    assert_eq!(wire["operation"], "open_hosted_output");
    assert_eq!(wire["channel"]["accountId"], "relay:tenant-7:account");
    assert_eq!(wire["conversation"]["scopeId"], "tenant-scope");
    assert_eq!(wire["conversation"]["parentId"], "parent-9");
    assert_eq!(wire["conversation"]["threadId"], "thread-7");
    assert_eq!(wire["conversation"]["topicId"], "topic-3");
    assert_eq!(wire["turn_id"], "turn-1");
    assert_eq!(wire["lifetime_ms"], 30_000);
    assert_eq!(CONTRACT_VERSION, 33);
    assert!(!is_compatible(31));
    assert!(!is_compatible(30));
    assert!(is_compatible(CONTRACT_VERSION));
    assert!(!is_compatible(CONTRACT_VERSION - 1));
}

#[test]
fn host_capability_payloads_keep_typed_snake_case_wire() {
    let request = HostTranscriptionRequest {
        audio_base64: "UklGRg==".into(),
        mime_type: Some("audio/wav".into()),
        file_name: Some("voice.wav".into()),
        language: Some("en".into()),
        scope: None,
    };
    assert_eq!(
        serde_json::to_value(&request).unwrap(),
        json!({
            "audio_base64":"UklGRg==",
            "mime_type":"audio/wav",
            "file_name":"voice.wav",
            "language":"en"
        })
    );
    let result = HostTranscriptionResult {
        text: "spoken words".into(),
        language: Some("en".into()),
        duration_secs: Some(1.25),
    };
    assert_eq!(
        serde_json::from_value::<HostTranscriptionResult>(serde_json::to_value(result).unwrap())
            .unwrap()
            .text,
        "spoken words"
    );
    let event = HostEventPublication {
        domain: "channel".into(),
        kind: "reaction_sent".into(),
        payload: json!({"target_message_id":"tg_42"}),
    };
    let wire = serde_json::to_value(event).unwrap();
    assert_eq!(wire["domain"], "channel");
    assert_eq!(wire["kind"], "reaction_sent");
    assert_eq!(wire["payload"]["target_message_id"], "tg_42");
}

#[test]
fn scoped_voice_host_callbacks_round_trip() {
    let scope = HostVoiceScope {
        configured_instance: "telegram-prod".into(),
        channel: ChannelRef {
            id: provider_id("telegram-prod").into(),
            account_id: Some("telegram-prod".into()),
        },
        conversation: ConversationRef {
            id: "chat-7".into(),
            ..Default::default()
        },
        turn_id: "message-42".into(),
    };
    let request = HostVoiceTranscriptionRequest {
        scope: scope.clone(),
        audio_base64: "AQID".into(),
        mime_type: Some("audio/ogg".into()),
        file_name: Some("note.oga".into()),
        language: Some("en".into()),
    };
    let decoded: HostVoiceTranscriptionRequest =
        serde_json::from_value(serde_json::to_value(request).unwrap()).unwrap();
    assert_eq!(decoded.scope, scope);
    assert_eq!(decoded.audio_base64, "AQID");

    let synthesis = HostSpeechSynthesisRequest {
        scope: scope.clone(),
        text: "hello there".into(),
        voice: Some("voice-a".into()),
        format: Some("ogg".into()),
    };
    let decoded: HostSpeechSynthesisRequest =
        serde_json::from_value(serde_json::to_value(synthesis).unwrap()).unwrap();
    assert_eq!(decoded.scope, scope);
    assert_eq!(decoded.text, "hello there");

    let policy_request = HostVoiceReplyPolicyRequest {
        scope: scope.clone(),
        inbound_has_voice: true,
        explicitly_requested: false,
    };
    let decoded: HostVoiceReplyPolicyRequest =
        serde_json::from_value(serde_json::to_value(policy_request).unwrap()).unwrap();
    assert_eq!(decoded.scope, scope);
    assert!(decoded.inbound_has_voice);
    assert!(!decoded.explicitly_requested);

    let policy = HostVoiceReplyPolicy {
        mode: HostVoiceReplyMode::Always,
    };
    let decoded: HostVoiceReplyPolicy =
        serde_json::from_value(serde_json::to_value(policy).unwrap()).unwrap();
    assert_eq!(decoded.mode, HostVoiceReplyMode::Always);
}

#[test]
fn channel_events_round_trip_with_discriminator_source_and_full_address() {
    let event = ChannelEvent {
        version: 2,
        event_id: "telegram:update:7".into(),
        source_event_id: Some("7".into()),
        source_timestamp: Some(1234),
        channel: ChannelRef {
            id: "telegram".into(),
            account_id: Some("bot".into()),
        },
        conversation: Some(ConversationRef {
            kind: ConversationKind::Group,
            id: "chat".into(),
            scope_id: Some("scope".into()),
            parent_id: Some("parent".into()),
            thread_id: Some("thread".into()),
            topic_id: Some("topic".into()),
        }),
        event: ChannelEventKind::ReactionRemoved {
            message_id: "message".into(),
            provider_message_id: Some("42".into()),
            actor: SenderRef {
                id: "actor".into(),
                ..Default::default()
            },
            reaction: ChannelReaction {
                value: "🔥".into(),
                custom_emoji_id: Some("emoji-1".into()),
            },
        },
    };
    let wire = serde_json::to_value(&event).unwrap();
    assert_eq!(wire["type"], "reaction_removed");
    assert_eq!(wire["conversation"]["scopeId"], "scope");
    assert_eq!(wire["conversation"]["parentId"], "parent");
    assert_eq!(wire["conversation"]["topicId"], "topic");
    assert_eq!(wire["conversation"]["threadId"], "thread");
    assert_eq!(serde_json::from_value::<ChannelEvent>(wire).unwrap(), event);
}

#[test]
fn button_callback_event_round_trips_actor_and_action_identity() {
    let event = ChannelEvent {
        version: 2,
        event_id: "telegram:update:9:callback".into(),
        source_event_id: Some("9".into()),
        source_timestamp: Some(1234),
        channel: ChannelRef {
            id: "telegram".into(),
            account_id: Some("bot-a".into()),
        },
        conversation: Some(ConversationRef {
            kind: ConversationKind::Group,
            id: "-100".into(),
            topic_id: Some("42".into()),
            ..Default::default()
        }),
        event: ChannelEventKind::ButtonCallback {
            query_id: "query-1".into(),
            action_id: "approve:opaque".into(),
            provider_message_id: Some("message-7".into()),
            actor: SenderRef {
                id: "user-3".into(),
                name: Some("operator".into()),
                ..Default::default()
            },
        },
    };
    let wire = serde_json::to_value(&event).unwrap();
    assert_eq!(wire["type"], "button_callback");
    assert_eq!(wire["actor"]["id"], "user-3");
    assert_eq!(wire["actionId"], "approve:opaque");
    assert_eq!(wire["conversation"]["topicId"], "42");
    assert_eq!(serde_json::from_value::<ChannelEvent>(wire).unwrap(), event);
}

#[test]
fn approval_action_ids_are_stable_bounded_and_correlatable_without_choice_text() {
    let action = crate::channel::approval_action_id("request:opaque", 2).unwrap();
    assert_eq!(action, "approval-v1:request:opaque:2");
    assert_eq!(
        crate::channel::parse_approval_action_id(&action),
        Some(("request:opaque", 2))
    );
    assert!(crate::channel::approval_action_id(&"r".repeat(54), 0).is_none());
    assert!(crate::channel::approval_action_id("", 0).is_none());
    assert!(crate::channel::parse_approval_action_id("approval-v1:r:nope").is_none());
}

#[test]
fn http_proxy_policy_round_trips_without_changing_its_wire_fields() {
    let policy = HttpProxyPolicy {
        http_proxy: Some("http://proxy.invalid:8080".into()),
        https_proxy: Some("http://secure-proxy.invalid:8443".into()),
        all_proxy: None,
        no_proxy: vec!["localhost".into(), ".internal".into()],
        service_selectors: vec!["channel.telegram".into(), "channel.discord*".into()],
    };
    let value = serde_json::to_value(&policy).unwrap();
    let decoded = serde_json::from_value::<HttpProxyPolicy>(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), value);
}

#[test]
fn channel_controller_operations_have_closed_typed_wire_tags() {
    let request = ChannelControllerRequest::TestChannel {
        config: ChannelsConfig::default(),
        channel: "discord".into(),
        auth_mode: controllers::ChannelAuthMode::BotToken,
        credentials: json!({"bot_token":"fixture-secret"}),
    };
    let wire = serde_json::to_value(&request).unwrap();
    assert_eq!(wire["operation"], "test_channel");
    assert_eq!(wire["request"]["channel"], "discord");
    assert_eq!(
        wire["request"]["credentials"]["bot_token"],
        "fixture-secret"
    );
    let round_trip = serde_json::from_value::<ChannelControllerRequest>(wire).unwrap();
    assert!(matches!(
        round_trip,
        ChannelControllerRequest::TestChannel { .. }
    ));
    assert!(
        serde_json::from_value::<ChannelControllerRequest>(json!({
            "operation":"execute_arbitrary",
            "request":{}
        }))
        .is_err()
    );
}

#[test]
fn channel_connect_carries_host_proxy_policy_inside_its_confidential_request() {
    let request = ChannelControllerRequest::Connect {
        config: ChannelsConfig::default(),
        channel: "yuanbao".into(),
        auth_mode: controllers::ChannelAuthMode::ApiKey,
        credentials: json!({"app_key":"fixture-key", "app_secret":"fixture-secret"}),
        proxy: HttpProxyPolicy {
            https_proxy: Some("http://proxy.invalid:8443".into()),
            service_selectors: vec!["channel.yuanbao".into()],
            ..Default::default()
        },
    };
    let wire = serde_json::to_value(&request).unwrap();
    assert_eq!(wire["operation"], "connect");
    assert_eq!(
        wire["request"]["credentials"]["app_secret"],
        "fixture-secret"
    );
    assert_eq!(
        wire["request"]["proxy"]["https_proxy"],
        "http://proxy.invalid:8443"
    );
    assert_eq!(
        wire["request"]["proxy"]["service_selectors"][0],
        "channel.yuanbao"
    );
    assert!(matches!(
        serde_json::from_value::<ChannelControllerRequest>(wire).unwrap(),
        ChannelControllerRequest::Connect { proxy, .. }
            if proxy.service_selectors == ["channel.yuanbao"]
    ));
}

#[test]
fn discord_controller_credentials_are_host_hydrated_in_the_confidential_module_request() {
    let request = ChannelControllerRequest::DiscordListGuilds {
        config: ChannelsConfig::default(),
        bot_token: "fixture-secret".into(),
        proxy: HttpProxyPolicy::default(),
    };
    let wire = serde_json::to_value(&request).unwrap();
    assert_eq!(wire["operation"], "discord_list_guilds");
    assert_eq!(wire["request"]["bot_token"], "fixture-secret");
    assert_eq!(
        wire["request"]["proxy"],
        serde_json::json!({"https_proxy":null,"http_proxy":null,"all_proxy":null,"no_proxy":[],"service_selectors":[]})
    );
    assert!(matches!(
        serde_json::from_value::<ChannelControllerRequest>(wire).unwrap(),
        ChannelControllerRequest::DiscordListGuilds { bot_token, .. }
            if bot_token == "fixture-secret"
    ));
}

#[test]
fn host_backend_requests_do_not_repeat_module_config_or_credentials() {
    let wire = serde_json::to_value(ChannelBackendRequest::SendReaction {
        channel: "discord".into(),
        reaction: json!({"message_id":"fixture-message","emoji":"✅"}),
    })
    .unwrap();
    assert_eq!(wire["operation"], "send_reaction");
    assert!(wire["request"].get("config").is_none());
    assert!(wire["request"].get("credentials").is_none());
}

#[test]
fn legacy_message_and_normalized_envelope_keep_optional_field_and_trust_wire() {
    let wire = json!({"id":"m1", "sender":"alice", "reply_target":"room", "content":"hi", "channel":"telegram", "timestamp":7, "thread_ts":"topic"});
    let message: ChannelMessage = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(serde_json::to_value(message).unwrap(), wire);
    let mut envelope = ChannelInboundEnvelope::default();
    envelope.access.delivered_via_upstream_relay = true;
    let encoded = serde_json::to_value(envelope).unwrap();
    assert_eq!(
        encoded["access"],
        json!({"dmDecision":"deny", "groupPolicy":"disabled", "mentionGate":"none", "commandAuthorized":false})
    );
    assert_eq!(encoded["media"], json!([]));
}

#[test]
fn send_message_legacy_wire_remains_snake_case() {
    let message = SendMessage::with_subject("hi", "room", "subject")
        .in_thread(Some("topic".into()))
        .with_idempotency_key("caller-key");
    assert_eq!(
        serde_json::to_value(message).unwrap(),
        json!({"content":"hi", "recipient":"room", "subject":"subject", "thread_ts":"topic", "idempotency_key":"caller-key"})
    );
}

#[test]
fn defaults_keep_safe_delivery_and_disconnection_wire_values() {
    let intent = ChannelOutboundIntent::default();
    assert_eq!(intent.durability, DeliveryDurability::BestEffort);
    assert_eq!(
        intent.payload,
        OutboundPayload::Text {
            text: String::new()
        }
    );
    let disconnected = controllers::ChannelDisconnectResult::default();
    let wire = serde_json::to_value(disconnected).unwrap();
    assert_eq!(wire["disconnected"], false);
    assert_eq!(wire["restartRequired"], false);
    for (kind, spelling) in [
        (channel::ConversationKind::Dm, "dm"),
        (channel::ConversationKind::Group, "group"),
        (channel::ConversationKind::Channel, "channel"),
        (channel::ConversationKind::Thread, "thread"),
        (channel::ConversationKind::Unknown, "unknown"),
    ] {
        assert_eq!(kind.as_session_segment(), spelling);
        assert_eq!(serde_json::to_value(kind).unwrap(), spelling);
    }
    assert!(is_compatible(CONTRACT_VERSION));
    assert!(!is_compatible(CONTRACT_VERSION + 1));
    assert_eq!(CONTRACT_VERSION, 33);
    assert!(!is_compatible(31));
    assert!(!is_compatible(25));
    assert!(!is_compatible(24));
    assert!(!is_compatible(23));
    assert_eq!(module_relay::MODULE_OPERATION_VERSION, (1, 25));
}

#[test]
fn structured_errors_preserve_kind_message_and_details() {
    let details = channel::ChannelSendError {
        message: "fixture refusal".into(),
        kind: channel::SendErrorKind::Forbidden,
        ..Default::default()
    };
    let error = TinyChannelsError::from(details.clone());
    assert_eq!(error.message(), "fixture refusal");
    match error {
        TinyChannelsError::Send {
            kind,
            details: actual,
            ..
        } => {
            assert_eq!(kind, channel::SendErrorKind::Forbidden);
            assert_eq!(*actual, details);
        }
        other => panic!("unexpected error: {other}"),
    }
    for error in [
        TinyChannelsError::new("fixture"),
        TinyChannelsError::Config("fixture".into()),
        TinyChannelsError::Serialization("fixture".into()),
    ] {
        assert_eq!(error.message(), "fixture");
        assert!(error.to_string().contains("fixture"));
    }
}

#[test]
fn relay_descriptor_keeps_explicit_platform_limit() {
    let descriptor = relay::CapabilityDescriptor::from_platform_entry(
        &relay::RelayPlatformEntry {
            name: "fixture".into(),
            label: "Fixture".into(),
            max_message_length: 128,
            emoji: None,
            platform_hint: None,
            pii_safe: true,
        },
        relay::RelayDescriptorOptions::default(),
    );
    assert_eq!(descriptor.max_message_length, 128);
    assert!(descriptor.supports_edit);
    assert!(descriptor.pii_safe);
    assert_eq!(
        relay::CapabilityDescriptor::from_json(&descriptor.to_json().unwrap()).unwrap(),
        descriptor
    );
}

#[test]
fn discord_provider_metadata_keeps_complete_legacy_wire_fields() {
    use crate::controllers::{BotPermissionCheck, DiscordGuild, DiscordTextChannel};
    let guild = json!({"id":"g", "name":"Guild", "icon":null});
    let channel =
        json!({"id":"c", "name":"general", "type":0, "position":4, "parent_id":"category"});
    let permissions = json!({"can_view_channel":true, "can_send_messages":false, "can_read_message_history":true, "missing_permissions":["SEND_MESSAGES"]});
    assert_eq!(
        serde_json::to_value(serde_json::from_value::<DiscordGuild>(guild.clone()).unwrap())
            .unwrap(),
        guild
    );
    assert_eq!(
        serde_json::to_value(
            serde_json::from_value::<DiscordTextChannel>(channel.clone()).unwrap()
        )
        .unwrap(),
        channel
    );
    assert_eq!(
        serde_json::to_value(
            serde_json::from_value::<BotPermissionCheck>(permissions.clone()).unwrap()
        )
        .unwrap(),
        permissions
    );
    let missing_position = json!({"id":"c", "name":"general", "type":0, "parent_id":null});
    assert_eq!(
        serde_json::from_value::<DiscordTextChannel>(missing_position)
            .unwrap()
            .position,
        0
    );
}

#[test]
fn prepared_connection_is_additive_and_keeps_the_controller_request_shape() {
    use module_controller::{
        ChannelControllerRequest, ChannelControllerResult, PreparedChannelConnection,
    };
    let request = ChannelControllerRequest::PrepareConnect {
        config: ChannelsConfig::default(),
        channel: "telegram".into(),
        auth_mode: ChannelAuthMode::BotToken,
        credentials: serde_json::json!({"bot_token": "fixture"}),
        proxy: Default::default(),
    };
    let wire = serde_json::to_value(&request).unwrap();
    assert_eq!(wire["operation"], "prepare_connect");
    assert_eq!(wire["request"]["channel"], "telegram");
    let _: ChannelControllerRequest = serde_json::from_value(wire).unwrap();
    let result = ChannelControllerResult::PreparedConnect(Box::new(PreparedChannelConnection {
        result: controllers::ChannelConnectionResult {
            status: "pending_auth".into(),
            restart_required: false,
            auth_action: Some("fixture-action".into()),
            message: None,
        },
        config_update: None,
    }));
    let wire = serde_json::to_value(result).unwrap();
    assert_eq!(wire["result"], "prepared_connect");
    assert_eq!(wire["value"]["result"]["auth_action"], "fixture-action");
    assert!(wire["value"]["config_update"].is_null());
}

#[test]
fn provider_health_report_has_a_transport_free_stable_wire() {
    let report = ChannelHealthReport {
        channel: "telegram".into(),
        state: ChannelHealthState::Healthy,
    };
    let encoded = serde_json::to_value(&report).unwrap();
    assert_eq!(
        encoded,
        serde_json::json!({"channel":"telegram", "state":"healthy"})
    );
    assert_eq!(
        serde_json::from_value::<ChannelHealthReport>(encoded).unwrap(),
        report
    );
    assert_eq!(
        serde_json::to_value(ChannelHealthState::Unhealthy).unwrap(),
        "unhealthy"
    );
    assert_eq!(
        serde_json::to_value(ChannelHealthState::Timeout).unwrap(),
        "timeout"
    );
}

#[test]
fn runtime_and_remote_commands_have_a_stable_wire() {
    use crate::module_remote::RemoteCommand;
    use crate::module_routes::ChannelRuntimeCommand;
    let cases = [
        (
            ChannelRuntimeCommand::ShowProviders,
            serde_json::json!({"command":"show_providers"}),
        ),
        (
            ChannelRuntimeCommand::SetProvider("openai".into()),
            serde_json::json!({"command":"set_provider","value":"openai"}),
        ),
        (
            ChannelRuntimeCommand::ShowModel,
            serde_json::json!({"command":"show_model"}),
        ),
        (
            ChannelRuntimeCommand::SetModel("gpt-5".into()),
            serde_json::json!({"command":"set_model","value":"gpt-5"}),
        ),
    ];
    for (cmd, wire) in cases {
        assert_eq!(serde_json::to_value(&cmd).unwrap(), wire);
        assert_eq!(
            serde_json::from_value::<ChannelRuntimeCommand>(wire).unwrap(),
            cmd
        );
    }
    for (cmd, name) in [
        (RemoteCommand::Status, "status"),
        (RemoteCommand::Sessions, "sessions"),
        (RemoteCommand::New, "new"),
        (RemoteCommand::Help, "help"),
    ] {
        assert_eq!(serde_json::to_value(cmd).unwrap(), name);
        assert_eq!(
            serde_json::from_value::<RemoteCommand>(name.into()).unwrap(),
            cmd
        );
    }
}

#[test]
fn remote_clear_history_carries_full_prepared_session_scope() {
    let request = crate::module_remote::RemoteHostRequest::ClearHistory {
        session_scope: crate::module_remote::RemoteSessionScope {
            channel: crate::ChannelRef {
                id: "telegram".into(),
                account_id: Some("telegram:bot-a".into()),
            },
            conversation: crate::ConversationRef {
                kind: crate::ConversationKind::Group,
                id: "room-1".into(),
                scope_id: Some("tenant-room".into()),
                parent_id: Some("parent-1".into()),
                thread_id: Some("thread-2".into()),
                topic_id: Some("topic-3".into()),
            },
            session_key: "prepared-v22-session-key".into(),
        },
    };
    let encoded = serde_json::to_value(&request).unwrap();
    assert_eq!(
        encoded["request"]["session_scope"]["channel"]["accountId"],
        "telegram:bot-a"
    );
    assert_eq!(
        encoded["request"]["session_scope"]["conversation"]["topicId"],
        "topic-3"
    );
    let decoded = serde_json::to_value(
        serde_json::from_value::<crate::module_remote::RemoteHostRequest>(encoded).unwrap(),
    )
    .unwrap();
    assert_eq!(decoded, serde_json::to_value(request).unwrap());
}

#[test]
fn legacy_send_projection_roundtrips_reply_thread_and_topic_keys() {
    let legacy = SendMessage {
        content: "hello".into(),
        recipient: "room".into(),
        subject: None,
        thread_ts: Some("thread-1".into()),
        topic_id: Some("topic-2".into()),
        reply_to_id: Some("message-3".into()),
        idempotency_key: Some("key".into()),
    };
    let typed = ChannelOutboundMessage::from_legacy_message("slack", &legacy);
    let roundtrip = typed.to_legacy_message().unwrap();
    assert_eq!(roundtrip.reply_to_id.as_deref(), Some("message-3"));
    assert_eq!(roundtrip.thread_ts.as_deref(), Some("thread-1"));
    assert_eq!(roundtrip.topic_id.as_deref(), Some("topic-2"));
    let wire = serde_json::to_value(roundtrip).unwrap();
    assert_eq!(wire["replyToId"], "message-3");
    assert_eq!(wire["topicId"], "topic-2");
    assert_eq!(wire["thread_ts"], "thread-1");
}

#[test]
fn inbound_voice_note_keeps_audio_transcript_source_and_waveform_typed() {
    let envelope = ChannelInboundEnvelope {
        reply_to_id: Some("reply-target".into()),
        parts: vec![ContentPart::VoiceNote {
            voice_note: VoiceNote {
                media: MediaReference {
                    url: Some("https://example.test/voice.ogg".into()),
                    content_type: Some("audio/ogg".into()),
                    kind: MediaKind::Audio,
                    ..Default::default()
                },
                codec: Some("opus".into()),
                duration_ms: Some(1500),
                waveform: Some(vec![0.0, 0.5, 1.0]),
                push_to_talk: true,
                transcript: Some(VoiceTranscript {
                    text: "hello".into(),
                    language: Some("en".into()),
                    state: VoiceTranscriptState::Complete,
                    source: VoiceTranscriptSource::HostStt,
                }),
            },
        }],
        ..Default::default()
    };
    let wire = serde_json::to_value(&envelope).unwrap();
    assert_eq!(wire["replyToId"], "reply-target");
    assert_eq!(wire["parts"][0]["kind"], "voice_note");
    assert_eq!(
        wire["parts"][0]["voiceNote"]["waveform"],
        json!([0.0, 0.5, 1.0])
    );
    assert_eq!(
        wire["parts"][0]["voiceNote"]["transcript"]["source"],
        "host_stt"
    );
    let decoded: ChannelInboundEnvelope = serde_json::from_value(wire).unwrap();
    assert_eq!(decoded, envelope);
}

#[test]
#[allow(deprecated)]
fn legacy_send_accepts_snake_case_topic_and_reply_with_canonical_output() {
    let message: crate::SendMessage = serde_json::from_value(serde_json::json!({
        "content": "hello", "recipient": "conversation", "topic_id": "topic", "reply_to_id": "reply"
    }))
    .unwrap();
    assert_eq!(message.topic_id.as_deref(), Some("topic"));
    assert_eq!(message.reply_to_id.as_deref(), Some("reply"));
    let wire = serde_json::to_value(message).unwrap();
    assert_eq!(wire["topicId"], "topic");
    assert_eq!(wire["replyToId"], "reply");
}
