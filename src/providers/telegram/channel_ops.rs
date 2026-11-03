//! Telegram channel — `Channel` trait implementation: send, listen, draft streaming, typing.

use super::channel_types::TelegramChannel;
use super::text::{TELEGRAM_MAX_MESSAGE_LENGTH, markdown_v2_text, strip_tool_call_tags};
use crate::config::StreamMode;
use crate::traits::{ChannelMessage, SendMessage};
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use std::time::Duration;

struct TelegramListenerGuard<'a>(&'a std::sync::atomic::AtomicBool);

impl Drop for TelegramListenerGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::Release);
    }
}

#[derive(Debug)]
struct TelegramPreDispatchError;

impl std::fmt::Display for TelegramPreDispatchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Telegram send was rejected before provider dispatch")
    }
}

impl std::error::Error for TelegramPreDispatchError {}

#[derive(Debug)]
struct TelegramBadFormatError;

impl std::fmt::Display for TelegramBadFormatError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Telegram request contains an invalid address")
    }
}

impl std::error::Error for TelegramBadFormatError {}

#[derive(Debug)]
pub(super) struct TelegramAttemptedError;

impl std::fmt::Display for TelegramAttemptedError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Telegram Bot API operation failed after dispatch")
    }
}

impl std::error::Error for TelegramAttemptedError {}

fn typed_interaction_error(
    error: anyhow::Error,
    operation: &str,
) -> crate::channel::ChannelSendError {
    if error.downcast_ref::<TelegramPreDispatchError>().is_some() {
        return crate::channel::ChannelSendError {
            kind: crate::channel::SendErrorKind::Forbidden,
            message: format!("Telegram {operation} was rejected before provider dispatch"),
            retryable: false,
            sent_before_error: false,
            ..Default::default()
        };
    }
    crate::channel::ChannelSendError {
        kind: crate::channel::SendErrorKind::Unknown,
        message: format!("Telegram {operation} outcome is uncertain"),
        retryable: false,
        sent_before_error: true,
        ..Default::default()
    }
}

#[derive(Debug)]
struct TelegramUnsupportedError;

impl std::fmt::Display for TelegramUnsupportedError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Telegram Bot API does not support this content operation")
    }
}

impl std::error::Error for TelegramUnsupportedError {}

fn vcard_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace(';', "\\;")
        .replace(',', "\\,")
        .replace('\n', "\\n")
        .replace('\r', "")
}

fn safe_vcard_type<'a>(label: Option<&'a str>, fallback: &'a str) -> &'a str {
    match label.map(str::trim).map(str::to_ascii_uppercase).as_deref() {
        Some("CELL" | "HOME" | "WORK" | "FAX" | "PAGER" | "VOICE") => {
            label.map(str::trim).unwrap_or(fallback)
        }
        Some("INTERNET" | "X400") => label.map(str::trim).unwrap_or(fallback),
        _ => fallback,
    }
}

fn presentation_content(
    value: &serde_json::Value,
) -> anyhow::Result<(String, Option<serde_json::Value>)> {
    if let Some(object) = value
        .as_object()
        .filter(|object| object.get("kind").and_then(serde_json::Value::as_str) == Some("approval"))
    {
        if object.len() != 4
            || object
                .keys()
                .any(|key| !["kind", "id", "prompt", "choices"].contains(&key.as_str()))
        {
            anyhow::bail!("Telegram approval presentation has unsupported fields");
        }
        let request_id = object
            .get("id")
            .and_then(serde_json::Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Telegram approval request ID is invalid"))?;
        let prompt = object
            .get("prompt")
            .and_then(serde_json::Value::as_str)
            .filter(|prompt| !prompt.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("Telegram approval prompt is invalid"))?;
        let choices = object
            .get("choices")
            .and_then(serde_json::Value::as_array)
            .filter(|choices| !choices.is_empty() && choices.len() <= 100)
            .ok_or_else(|| anyhow::anyhow!("Telegram approval choices are invalid"))?;
        let mut keyboard = Vec::with_capacity(choices.len());
        for (index, choice) in choices.iter().enumerate() {
            let label = choice
                .as_str()
                .filter(|label| !label.trim().is_empty() && label.chars().count() <= 64)
                .ok_or_else(|| anyhow::anyhow!("Telegram approval choice label is invalid"))?;
            let action_id = tinychannels_bus::channel::approval_action_id(request_id, index)
                .ok_or_else(|| anyhow::anyhow!("Telegram approval action exceeds 64 bytes"))?;
            keyboard.push(vec![serde_json::json!({
                "text": label,
                "callback_data": action_id,
            })]);
        }
        return Ok((
            prompt.to_owned(),
            Some(serde_json::json!({"inline_keyboard": keyboard})),
        ));
    }

    fn collect(value: &serde_json::Value, out: &mut Vec<String>) -> anyhow::Result<()> {
        match value {
            serde_json::Value::String(text) if !text.trim().is_empty() => out.push(text.clone()),
            serde_json::Value::Array(items) => {
                for item in items {
                    collect(item, out)?;
                }
            }
            serde_json::Value::Object(object) => {
                const ALLOWED: &[&str] =
                    &["type", "text", "title", "heading", "caption", "description"];
                if object.keys().any(|key| !ALLOWED.contains(&key.as_str())) {
                    anyhow::bail!("Telegram presentation block contains unsupported fields");
                }
                if object.get("type").is_some_and(|value| !value.is_string()) {
                    anyhow::bail!("Telegram presentation block type must be text");
                }
                if let Some(kind) = object.get("type").and_then(serde_json::Value::as_str)
                    && !matches!(
                        kind,
                        "text"
                            | "paragraph"
                            | "heading"
                            | "title"
                            | "caption"
                            | "quote"
                            | "bullet"
                            | "list_item"
                    )
                {
                    anyhow::bail!("Telegram presentation block type is unsupported");
                }
                for key in ["title", "heading", "text", "caption", "description"] {
                    if let Some(value) = object.get(key) {
                        let Some(text) = value.as_str() else {
                            anyhow::bail!(
                                "Telegram presentation block text fields must be strings"
                            );
                        };
                        if !text.trim().is_empty() {
                            out.push(text.to_owned());
                        }
                    }
                }
            }
            _ => anyhow::bail!("Telegram presentation only accepts text or supported text blocks"),
        }
        Ok(())
    }
    let mut parts = Vec::new();
    collect(value, &mut parts)?;
    Ok((parts.join("\n"), None))
}

pub(super) fn response_status_ok(value: &serde_json::Value) -> bool {
    value.get("ok").and_then(serde_json::Value::as_bool) == Some(true)
}

impl TelegramChannel {
    pub(super) fn supports_edit(&self) -> bool {
        true
    }

    pub(super) fn supports_reply_to_id(&self) -> bool {
        true
    }

    pub(super) fn supports_thread_id(&self) -> bool {
        true
    }

    pub(super) fn supports_draft_updates(&self) -> bool {
        self.stream_mode != StreamMode::Off
    }

    pub(super) fn supports_progressive_edits(&self) -> bool {
        self.supports_draft_updates()
    }

    pub(crate) async fn health_status(&self) -> bool {
        let timeout_duration = Duration::from_secs(5);

        match tokio::time::timeout(
            timeout_duration,
            self.http_client().get(self.api_url("getMe")).send(),
        )
        .await
        {
            Ok(Ok(response)) => response.status().is_success(),
            Ok(Err(error)) => {
                tracing::debug!("Telegram health check failed: {error}");
                false
            }
            Err(_) => {
                tracing::debug!("Telegram health check timed out after 5s");
                false
            }
        }
    }
}

fn typed_send_error(error: anyhow::Error) -> crate::channel::ChannelSendError {
    if let Some(partial) = error.downcast_ref::<super::channel_send::PartialTextSendError>() {
        return partial.error.clone();
    }
    if error.downcast_ref::<TelegramPreDispatchError>().is_some() {
        return crate::channel::ChannelSendError {
            kind: crate::channel::SendErrorKind::Forbidden,
            message: "Telegram typed send was rejected before provider dispatch".into(),
            retryable: false,
            sent_before_error: false,
            ..Default::default()
        };
    }
    if error.downcast_ref::<TelegramBadFormatError>().is_some() {
        return crate::channel::ChannelSendError {
            kind: crate::channel::SendErrorKind::BadFormat,
            message: "Telegram request contains an invalid address".into(),
            retryable: false,
            sent_before_error: false,
            ..Default::default()
        };
    }
    if error.downcast_ref::<TelegramUnsupportedError>().is_some() {
        return crate::channel::ChannelSendError {
            kind: crate::channel::SendErrorKind::BadFormat,
            message: "Telegram Bot API does not support this content operation".into(),
            unsupported: true,
            retryable: false,
            sent_before_error: false,
            ..Default::default()
        };
    }
    crate::channel::ChannelSendError {
        kind: crate::channel::SendErrorKind::Unknown,
        message: "Telegram typed send outcome is uncertain".into(),
        retryable: false,
        sent_before_error: true,
        ..Default::default()
    }
}

impl TelegramChannel {
    fn callback_event(update: &serde_json::Value) -> Option<tinychannels_bus::ChannelEvent> {
        let query = update.get("callback_query")?;
        let query_id = query.get("id")?.as_str()?.to_owned();
        let action_id = query.get("data")?.as_str()?.to_owned();
        if action_id.is_empty() || action_id.len() > 64 {
            return None;
        }
        let user = query.get("from")?;
        let actor_id = user.get("id")?.as_i64()?.to_string();
        let message = query.get("message")?;
        let chat = message.get("chat")?;
        let conversation_id = chat.get("id")?.as_i64()?.to_string();
        let message_id = message.get("message_id")?.as_i64()?.to_string();
        let update_id = update
            .get("update_id")
            .and_then(serde_json::Value::as_i64)
            .map(|id| id.to_string());
        let topic_id = message
            .get("message_thread_id")
            .and_then(serde_json::Value::as_i64)
            .map(|id| id.to_string());
        let kind = match chat.get("type").and_then(serde_json::Value::as_str) {
            Some("private") => tinychannels_bus::ConversationKind::Dm,
            Some("group" | "supergroup") => tinychannels_bus::ConversationKind::Group,
            Some("channel") => tinychannels_bus::ConversationKind::Channel,
            _ => tinychannels_bus::ConversationKind::Unknown,
        };
        Some(tinychannels_bus::ChannelEvent {
            version: 2,
            event_id: update_id
                .as_ref()
                .map(|id| format!("telegram:update:{id}:callback:{query_id}"))
                .unwrap_or_else(|| format!("telegram:callback:{query_id}")),
            source_event_id: update_id,
            source_timestamp: None,
            channel: tinychannels_bus::ChannelRef {
                id: "telegram".into(),
                account_id: None,
            },
            conversation: Some(tinychannels_bus::ConversationRef {
                kind,
                id: conversation_id,
                topic_id,
                ..Default::default()
            }),
            event: tinychannels_bus::ChannelEventKind::ButtonCallback {
                query_id,
                action_id,
                provider_message_id: Some(message_id),
                actor: tinychannels_bus::SenderRef {
                    id: actor_id,
                    name: user
                        .get("username")
                        .or_else(|| user.get("first_name"))
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned),
                    is_bot: user
                        .get("is_bot")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false),
                    ..Default::default()
                },
            },
        })
    }

    fn callback_is_allowed(&self, update: &serde_json::Value) -> bool {
        let Some(user) = update
            .get("callback_query")
            .and_then(|query| query.get("from"))
        else {
            return false;
        };
        let id = user
            .get("id")
            .and_then(serde_json::Value::as_i64)
            .map(|id| id.to_string());
        // Authorization only uses Telegram's numeric user ID and username. The
        // first name is a display label and must never grant access.
        let username = user.get("username").and_then(serde_json::Value::as_str);
        self.is_any_user_allowed(id.iter().map(String::as_str).chain(username))
    }

    async fn acknowledge_callback(&self, query_id: &str) {
        let Ok(response) = self
            .http_client()
            .post(self.api_url("answerCallbackQuery"))
            .json(&serde_json::json!({ "callback_query_id": query_id }))
            .send()
            .await
        else {
            tracing::debug!("Telegram callback acknowledgment request failed");
            return;
        };
        if !Self::telegram_api_ok(response).await {
            tracing::debug!("Telegram callback acknowledgment was rejected");
        }
    }

    #[cfg(test)]
    pub(crate) fn inbound_envelope_from_update(
        update: &serde_json::Value,
        message: &ChannelMessage,
        voice_note: Option<tinychannels_bus::VoiceNote>,
    ) -> tinychannels_bus::ChannelInboundEnvelope {
        Self::inbound_envelope_with_media(update, message, voice_note, Vec::new())
    }

    pub(crate) fn inbound_envelope_with_media(
        update: &serde_json::Value,
        message: &ChannelMessage,
        voice_note: Option<tinychannels_bus::VoiceNote>,
        media_parts: Vec<tinychannels_bus::ContentPart>,
    ) -> tinychannels_bus::ChannelInboundEnvelope {
        let source = update
            .get("message")
            .or_else(|| update.get("edited_message"))
            .unwrap_or(&serde_json::Value::Null);
        let chat = source.get("chat").unwrap_or(&serde_json::Value::Null);
        let chat_id = chat
            .get("id")
            .and_then(serde_json::Value::as_i64)
            .map(|id| id.to_string())
            .unwrap_or_else(|| {
                message
                    .reply_target
                    .split(':')
                    .next()
                    .unwrap_or_default()
                    .to_owned()
            });
        let topic_id = source
            .get("message_thread_id")
            .and_then(serde_json::Value::as_i64)
            .map(|id| id.to_string());
        let reply_to_id = source
            .get("reply_to_message")
            .and_then(|reply| reply.get("message_id"))
            .and_then(serde_json::Value::as_i64)
            .map(|id| id.to_string());
        let kind = match chat.get("type").and_then(serde_json::Value::as_str) {
            Some("private") => tinychannels_bus::ConversationKind::Dm,
            Some("group" | "supergroup") => tinychannels_bus::ConversationKind::Group,
            Some("channel") => tinychannels_bus::ConversationKind::Channel,
            _ => tinychannels_bus::ConversationKind::Unknown,
        };
        let actor = source.get("from").or_else(|| source.get("sender_chat"));
        let sender_id = actor
            .and_then(|actor| actor.get("id"))
            .and_then(serde_json::Value::as_i64)
            .map(|id| id.to_string())
            .unwrap_or_else(|| message.sender.clone());
        let sender_name = actor
            .and_then(|actor| actor.get("username"))
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string)
            .or_else(|| message.sender_name.clone());
        let provider_message_id = source
            .get("message_id")
            .and_then(serde_json::Value::as_i64)
            .map(|id| id.to_string());
        let timestamp_unix_ms = source
            .get("date")
            .and_then(serde_json::Value::as_u64)
            .map(|seconds| seconds.saturating_mul(1000));
        let mut parts = Vec::new();
        if !message.content.trim().is_empty() {
            parts.push(tinychannels_bus::ContentPart::Text {
                text: message.content.clone(),
                markdown_dialect: None,
            });
        }
        if let Some(voice_note) = voice_note {
            parts.push(tinychannels_bus::ContentPart::VoiceNote { voice_note });
        }
        parts.extend(media_parts);
        let message_date_ms = source
            .get("date")
            .and_then(serde_json::Value::as_u64)
            .map(|seconds| seconds.saturating_mul(1000));
        let venue = source.get("venue");
        let location = source
            .get("location")
            .or_else(|| venue.and_then(|venue| venue.get("location")));
        if let Some(location) = location {
            let latitude = location.get("latitude").and_then(serde_json::Value::as_f64);
            let longitude = location
                .get("longitude")
                .and_then(serde_json::Value::as_f64);
            if let (Some(latitude), Some(longitude)) = (latitude, longitude)
                && latitude.is_finite()
                && longitude.is_finite()
                && (-90.0..=90.0).contains(&latitude)
                && (-180.0..=180.0).contains(&longitude)
            {
                let live_until_unix_ms = location
                    .get("live_period")
                    .and_then(serde_json::Value::as_u64)
                    .or_else(|| {
                        source
                            .get("live_period")
                            .and_then(serde_json::Value::as_u64)
                    })
                    .filter(|seconds| *seconds > 0)
                    .and_then(|seconds| {
                        message_date_ms
                            .map(|date| date.saturating_add(seconds.saturating_mul(1000)))
                    });
                parts.push(tinychannels_bus::ContentPart::Location {
                    latitude,
                    longitude,
                    label: venue
                        .and_then(|venue| venue.get("title"))
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned),
                    address: venue
                        .and_then(|venue| venue.get("address"))
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned),
                    accuracy_meters: location
                        .get("horizontal_accuracy")
                        .and_then(serde_json::Value::as_f64)
                        .filter(|accuracy| accuracy.is_finite() && *accuracy >= 0.0),
                    live_until_unix_ms,
                });
            }
        }
        if let Some(contact) = source.get("contact") {
            let first_name = contact
                .get("first_name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .trim();
            let last_name = contact
                .get("last_name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .trim();
            let display_name = [first_name, last_name]
                .into_iter()
                .filter(|name| !name.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            let phone_numbers = contact
                .get("phone_number")
                .and_then(serde_json::Value::as_str)
                .filter(|phone| !phone.trim().is_empty())
                .map(|value| {
                    vec![tinychannels_bus::ContactMethod {
                        value: value.to_owned(),
                        label: None,
                    }]
                })
                .unwrap_or_default();
            if !display_name.is_empty() || !phone_numbers.is_empty() {
                parts.push(tinychannels_bus::ContentPart::Contact {
                    display_name,
                    phone_numbers,
                    emails: Vec::new(),
                    organization: None,
                });
            }
        }
        let media = parts
            .iter()
            .flat_map(|part| match part {
                tinychannels_bus::ContentPart::Image { media, .. }
                | tinychannels_bus::ContentPart::Audio { media, .. }
                | tinychannels_bus::ContentPart::File { media, .. }
                | tinychannels_bus::ContentPart::Sticker { media, .. } => vec![media.clone()],
                tinychannels_bus::ContentPart::Video {
                    media, thumbnail, ..
                } => std::iter::once(media.clone())
                    .chain(thumbnail.clone())
                    .collect(),
                tinychannels_bus::ContentPart::VoiceNote { voice_note } => {
                    vec![voice_note.media.clone()]
                }
                _ => Vec::new(),
            })
            .collect();
        tinychannels_bus::ChannelInboundEnvelope {
            channel: tinychannels_bus::ChannelRef {
                id: "telegram".into(),
                account_id: None,
            },
            message_id: message.id.clone(),
            reply_to_id,
            timestamp_unix_ms,
            conversation: tinychannels_bus::ConversationRef {
                kind,
                id: chat_id,
                topic_id,
                ..Default::default()
            },
            sender: tinychannels_bus::SenderRef {
                id: sender_id,
                name: sender_name,
                ..Default::default()
            },
            text: message.content.clone(),
            parts,
            media,
            raw: Some(serde_json::json!({"providerMessageId": provider_message_id})),
            ..Default::default()
        }
    }

    /// One iteration of the background typing refresher: post the chat action
    /// and log (never fail) on rejection or transport error. Returns whether
    /// Telegram accepted the refresh.
    #[cfg(test)]
    pub(crate) async fn refresh_typing_once(
        client: &reqwest::Client,
        url: &str,
        body: &serde_json::Value,
        recipient: &str,
    ) -> bool {
        match client.post(url).json(body).send().await {
            Ok(resp) => {
                let accepted = Self::telegram_api_ok(resp).await;
                if !accepted {
                    tracing::warn!(recipient, "Telegram typing refresh rejected");
                }
                accepted
            }
            Err(error) => {
                tracing::warn!(
                    recipient,
                    %error,
                    "Telegram typing refresh request failed"
                );
                false
            }
        }
    }
}

impl TelegramChannel {
    async fn send_structured_v2(
        &self,
        message: &tinychannels_bus::ChannelOutboundMessage,
    ) -> anyhow::Result<Option<tinychannels_bus::channel::ChannelSendOutcome>> {
        use tinychannels_bus::{OutboundContentPart, channel::ContentPart};

        let structured = message
            .parts
            .iter()
            .filter_map(|part| match part {
                OutboundContentPart::Content { part } => match part {
                    ContentPart::Location { .. }
                    | ContentPart::Contact { .. }
                    | ContentPart::Buttons { .. }
                    | ContentPart::Poll { .. }
                    | ContentPart::Presentation { .. }
                    | ContentPart::Native { .. } => Some(part),
                    _ => None,
                },
                _ => None,
            })
            .collect::<Vec<_>>();
        if structured.is_empty() {
            return Ok(None);
        }
        let buttons_only_with_text = structured.len() == 1
            && matches!(structured[0], ContentPart::Buttons { .. })
            && message.parts.iter().all(|part| {
                matches!(
                    part,
                    OutboundContentPart::Content {
                        part: ContentPart::Text { .. } | ContentPart::Buttons { .. }
                    }
                )
            });
        if structured.len() != 1 || (!buttons_only_with_text && message.parts.len() != 1) {
            anyhow::bail!("Telegram structured message has an unsupported content combination");
        }
        let conversation =
            message
                .conversation_ref
                .clone()
                .unwrap_or_else(|| tinychannels_bus::ConversationRef {
                    id: message.conversation_id.clone(),
                    thread_id: message.thread_id.clone(),
                    ..Default::default()
                });
        if conversation.id.trim().is_empty() {
            anyhow::bail!("Telegram send requires a conversation identity");
        }
        let thread_id = conversation
            .topic_id
            .as_deref()
            .or(conversation.thread_id.as_deref())
            .or(message.thread_id.as_deref())
            .map(str::to_owned);
        let mut body = serde_json::json!({"chat_id": conversation.id});
        let mut method = "sendMessage";
        let kind = match structured[0] {
            ContentPart::Location {
                latitude,
                longitude,
                label,
                address,
                accuracy_meters,
                live_until_unix_ms,
            } => {
                if !latitude.is_finite()
                    || !longitude.is_finite()
                    || !(-90.0..=90.0).contains(latitude)
                    || !(-180.0..=180.0).contains(longitude)
                {
                    anyhow::bail!("Telegram location coordinates are invalid");
                }
                if label.is_some() != address.is_some() {
                    anyhow::bail!("Telegram place requires both a title and address");
                }
                if label.is_some() && (accuracy_meters.is_some() || live_until_unix_ms.is_some()) {
                    anyhow::bail!(
                        "Telegram venue cannot also be a live or accuracy-scoped location"
                    );
                }
                method = if label.is_some() {
                    "sendVenue"
                } else {
                    "sendLocation"
                };
                body["latitude"] = serde_json::json!(latitude);
                body["longitude"] = serde_json::json!(longitude);
                if let (Some(title), Some(address)) = (label, address) {
                    if title.chars().count() > 128 || address.chars().count() > 256 {
                        anyhow::bail!("Telegram venue title or address exceeds Bot API limits");
                    }
                    body["title"] = serde_json::json!(title);
                    body["address"] = serde_json::json!(address);
                }
                if let Some(accuracy) = accuracy_meters {
                    if !accuracy.is_finite() || !(0.0..=1500.0).contains(accuracy) {
                        anyhow::bail!("Telegram location accuracy is invalid");
                    }
                    body["horizontal_accuracy"] = serde_json::json!(accuracy);
                }
                if let Some(until) = live_until_unix_ms {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;
                    let seconds = until.saturating_sub(now).div_ceil(1000);
                    if !(60..=86400).contains(&seconds) {
                        anyhow::bail!("Telegram live location duration must be 60..86400 seconds");
                    }
                    body["live_period"] = serde_json::json!(seconds);
                }
                tinychannels_bus::channel::MessageReceiptPartKind::Card
            }
            ContentPart::Contact {
                display_name,
                phone_numbers,
                emails,
                organization,
            } => {
                let phone = phone_numbers
                    .first()
                    .map(|method| method.value.trim())
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| {
                        anyhow::anyhow!("Telegram contact requires at least one phone number")
                    })?;
                if phone.len() > 256 {
                    anyhow::bail!("Telegram contact phone number is too long");
                }
                let mut lines = vec![
                    "BEGIN:VCARD".to_owned(),
                    "VERSION:3.0".to_owned(),
                    format!("FN:{}", vcard_escape(display_name)),
                ];
                for entry in phone_numbers {
                    lines.push(format!(
                        "TEL;TYPE={}:{}",
                        safe_vcard_type(entry.label.as_deref(), "CELL"),
                        vcard_escape(&entry.value)
                    ));
                }
                for entry in emails {
                    lines.push(format!(
                        "EMAIL;TYPE={}:{}",
                        safe_vcard_type(entry.label.as_deref(), "INTERNET"),
                        vcard_escape(&entry.value)
                    ));
                }
                if let Some(org) = organization {
                    lines.push(format!("ORG:{}", vcard_escape(org)));
                }
                lines.push("END:VCARD".into());
                let vcard = lines.join("\n");
                if vcard.len() > 2048 {
                    anyhow::bail!("Telegram contact vCard exceeds 2048 bytes");
                }
                let mut names = display_name.splitn(2, char::is_whitespace);
                body["phone_number"] = serde_json::json!(phone);
                body["first_name"] = serde_json::json!(names.next().unwrap_or("Contact"));
                if let Some(last) = names.next().filter(|name| !name.trim().is_empty()) {
                    body["last_name"] = serde_json::json!(last.trim());
                }
                body["vcard"] = serde_json::json!(vcard);
                method = "sendContact";
                tinychannels_bus::channel::MessageReceiptPartKind::Card
            }
            ContentPart::Buttons { rows } => {
                let text = message
                    .parts
                    .iter()
                    .filter_map(|part| match part {
                        OutboundContentPart::Content {
                            part: ContentPart::Text { text, .. },
                        } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if text.is_empty() {
                    anyhow::bail!("Telegram inline buttons require message text");
                }
                if rows.iter().flatten().any(|button| button.disabled) {
                    anyhow::bail!("Telegram inline keyboards cannot render disabled buttons");
                }
                if rows.iter().flatten().any(|button| button.style.is_some()) {
                    anyhow::bail!("Telegram inline keyboards do not support button styles");
                }
                let mut keyboard = Vec::new();
                for row in rows {
                    let mut native_row = Vec::new();
                    for button in row {
                        if button.id.is_empty()
                            || button.id.len() > 64
                            || button.label.is_empty()
                            || button.label.chars().count() > 64
                        {
                            anyhow::bail!(
                                "Telegram button label or callback ID violates Bot API limits"
                            );
                        }
                        native_row.push(
                            serde_json::json!({"text": button.label, "callback_data": button.id}),
                        );
                    }
                    if !native_row.is_empty() {
                        keyboard.push(native_row);
                    }
                }
                if keyboard.is_empty() {
                    anyhow::bail!("Telegram inline keyboard has no enabled buttons");
                }
                body["text"] = serde_json::json!(markdown_v2_text(&strip_tool_call_tags(&text)));
                body["parse_mode"] = serde_json::json!("MarkdownV2");
                body["reply_markup"] = serde_json::json!({"inline_keyboard": keyboard});
                tinychannels_bus::channel::MessageReceiptPartKind::Text
            }
            ContentPart::Poll {
                question,
                options,
                multiple_choice,
                closes_at_unix_ms,
                ..
            } => {
                if question.trim().is_empty()
                    || question.chars().count() > 300
                    || !(2..=12).contains(&options.len())
                    || options
                        .iter()
                        .any(|option| option.trim().is_empty() || option.chars().count() > 100)
                {
                    anyhow::bail!("Telegram poll does not satisfy Bot API question/option limits");
                }
                method = "sendPoll";
                body["question"] = serde_json::json!(question);
                body["options"] = serde_json::json!(
                    options
                        .iter()
                        .map(|text| serde_json::json!({"text": text}))
                        .collect::<Vec<_>>()
                );
                body["is_anonymous"] = serde_json::json!(true);
                body["type"] = serde_json::json!("regular");
                body["allows_multiple_answers"] = serde_json::json!(multiple_choice);
                if let Some(closes_at) = closes_at_unix_ms {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    let seconds = closes_at.saturating_div(1000).saturating_sub(now);
                    if !(5..=2_628_000).contains(&seconds) {
                        anyhow::bail!("Telegram poll close time must be 5..2628000 seconds ahead");
                    }
                    body["close_date"] = serde_json::json!(closes_at.saturating_div(1000));
                }
                tinychannels_bus::channel::MessageReceiptPartKind::Poll
            }
            ContentPart::Presentation { blocks } => {
                let (text, keyboard) = presentation_content(blocks)?;
                if text.trim().is_empty() {
                    anyhow::bail!("Telegram presentation contains no renderable text");
                }
                body["text"] = serde_json::json!(markdown_v2_text(&strip_tool_call_tags(&text)));
                body["parse_mode"] = serde_json::json!("MarkdownV2");
                if let Some(keyboard) = keyboard {
                    body["reply_markup"] = keyboard;
                }
                tinychannels_bus::channel::MessageReceiptPartKind::Text
            }
            ContentPart::Native { .. } => return Err(anyhow::Error::new(TelegramUnsupportedError)),
            _ => unreachable!(),
        };
        if let Some(thread) = thread_id.as_deref() {
            body["message_thread_id"] = serde_json::json!(thread);
        }
        if let Some(reply) = message.reply_to_id.as_deref() {
            let id = reply.parse::<i64>().map_err(|_| {
                anyhow::anyhow!("Telegram reply target must be a numeric message ID")
            })?;
            body["reply_parameters"] = serde_json::json!({"message_id": id});
        }
        let response = self
            .http_client()
            .post(self.api_url(method))
            .json(&body)
            .send()
            .await
            .map_err(|_| {
                super::channel_send::partial_text_send_error(
                    Vec::new(),
                    format!("Telegram {method} outcome is uncertain"),
                    false,
                    true,
                )
            })?;
        let value = response.json::<serde_json::Value>().await.map_err(|_| {
            super::channel_send::partial_text_send_error(
                Vec::new(),
                format!("Telegram {method} response is uncertain"),
                false,
                true,
            )
        })?;
        if !response_status_ok(&value) {
            return Err(super::channel_send::partial_text_send_error(
                Vec::new(),
                format!("Telegram {method} outcome is uncertain"),
                false,
                true,
            ));
        }
        let id = value
            .pointer("/result/message_id")
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| {
                super::channel_send::partial_text_send_error(
                    Vec::new(),
                    format!("Telegram {method} response omitted message ID"),
                    false,
                    true,
                )
            })?
            .to_string();
        let channel = message.channel_ref.clone().or_else(|| {
            Some(tinychannels_bus::ChannelRef {
                id: "telegram".into(),
                account_id: None,
            })
        });
        let part = tinychannels_bus::channel::MessageReceiptPart {
            platform_message_id: id.clone(),
            kind,
            index: 0,
            thread_id: thread_id.clone(),
            reply_to_id: message.reply_to_id.clone(),
            edit_token: Some(id.clone()),
            delete_token: Some(id.clone()),
            ..Default::default()
        };
        let receipt = tinychannels_bus::channel::MessageReceipt {
            idempotency_key: Some(message.idempotency_key.clone()),
            channel,
            conversation: Some(conversation),
            primary_platform_message_id: Some(id.clone()),
            platform_message_ids: vec![id],
            parts: vec![part],
            thread_id,
            reply_to_id: message.reply_to_id.clone(),
            sent_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            ..Default::default()
        };
        Ok(Some(tinychannels_bus::channel::ChannelSendOutcome {
            receipt,
            error: None,
        }))
    }

    pub(crate) async fn send_v2_with_receipt(
        &self,
        message: &tinychannels_bus::ChannelOutboundMessage,
    ) -> anyhow::Result<tinychannels_bus::channel::ChannelSendOutcome> {
        if let Some(outcome) = self.send_structured_v2(message).await.map_err(|error| {
            if error.downcast_ref::<TelegramUnsupportedError>().is_some()
                || error
                    .downcast_ref::<super::channel_send::PartialTextSendError>()
                    .is_some()
            {
                error
            } else {
                anyhow::Error::new(TelegramPreDispatchError)
            }
        })? {
            return Ok(outcome);
        }
        if message.parts.iter().any(|part| {
            matches!(
                part,
                tinychannels_bus::OutboundContentPart::Voice {
                    voice: tinychannels_bus::OutboundVoice::Synthesize { .. }
                }
            )
        }) {
            anyhow::bail!("voice synthesis must be prepared by the shared module runtime");
        }
        let voice_parts = message
            .parts
            .iter()
            .filter_map(|part| match part {
                tinychannels_bus::OutboundContentPart::Voice {
                    voice: tinychannels_bus::OutboundVoice::Media { media, codec, .. },
                } => Some((media, codec)),
                _ => None,
            })
            .collect::<Vec<_>>();
        if voice_parts.len() == 1 {
            let (media, codec) = voice_parts[0];
            let caption_parts = message
                .parts
                .iter()
                .filter_map(|part| match part {
                    tinychannels_bus::OutboundContentPart::Content {
                        part: tinychannels_bus::ContentPart::Text { text, .. },
                    } => Some(Ok(text.as_str())),
                    tinychannels_bus::OutboundContentPart::Voice { .. } => None,
                    _ => Some(Err(anyhow::anyhow!(
                        "Telegram voice send only supports transcript text"
                    ))),
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            let caption = strip_tool_call_tags(&caption_parts.join("\n"));
            // Telegram caps voice captions at 1024 characters. Keep the audio
            // and deliver the full transcript as a follow-up text message instead
            // of dropping the entire voice reply.
            let transcript_fallback = caption.chars().count() > 1024;
            let store = self
                .voice_media_store
                .as_ref()
                .ok_or_else(|| anyhow::Error::new(TelegramPreDispatchError))?;
            let conversation = message.conversation_ref.clone().unwrap_or_else(|| {
                tinychannels_bus::ConversationRef {
                    id: message.conversation_id.clone(),
                    thread_id: message.thread_id.clone(),
                    ..Default::default()
                }
            });
            let opaque_id = media
                .opaque_id
                .as_deref()
                .filter(|id| !id.trim().is_empty())
                .ok_or_else(|| anyhow::Error::new(TelegramPreDispatchError))?;
            let channel =
                message
                    .channel_ref
                    .clone()
                    .unwrap_or_else(|| tinychannels_bus::ChannelRef {
                        id: "telegram".into(),
                        account_id: None,
                    });
            let turn_id = message
                .turn_id
                .clone()
                .filter(|turn_id| !turn_id.trim().is_empty())
                .ok_or_else(|| anyhow::Error::new(TelegramPreDispatchError))?;
            let scope = tinychannels_bus::HostVoiceScope {
                configured_instance: channel
                    .account_id
                    .clone()
                    .unwrap_or_else(|| channel.id.clone()),
                channel,
                conversation: conversation.clone(),
                turn_id,
            };
            let resolved = store.read_voice(scope, opaque_id.to_owned()).await?;
            let audio = BASE64.decode(resolved.audio_base64.as_bytes())?;
            let (method, field) =
                if codec.as_deref() == Some("ogg_opus") || resolved.mime_type == "audio/ogg" {
                    ("sendVoice", "voice")
                } else {
                    ("sendAudio", "audio")
                };
            let (chat_id, thread_id) = Self::parse_reply_target(&conversation.id);
            if audio.len() > 8 * 1024 * 1024 {
                anyhow::bail!("Telegram voice upload exceeds its size limit");
            }
            if let Some(reply_to) = message.reply_to_id.as_deref() {
                reply_to
                    .parse::<i64>()
                    .map_err(|_| anyhow::anyhow!("Telegram reply target is invalid"))?;
            }
            let message_id = self
                .send_voice_bytes_with_id(super::channel_send::TelegramVoiceUpload {
                    method,
                    field,
                    chat_id: &chat_id,
                    thread_id: thread_id
                        .as_deref()
                        .or(conversation.topic_id.as_deref())
                        .or(conversation.thread_id.as_deref()),
                    reply_to_id: message.reply_to_id.as_deref(),
                    caption: (!transcript_fallback && !caption.trim().is_empty())
                        .then_some(caption.as_str()),
                    bytes: audio,
                    file_name: resolved.file_name.as_deref().unwrap_or("voice.ogg"),
                })
                .await
                .map_err(|_| {
                    super::channel_send::partial_text_send_error(
                        Vec::new(),
                        "Telegram voice upload outcome is uncertain".into(),
                        false,
                        true,
                    )
                })?;
            let kind = tinychannels_bus::channel::MessageReceiptPartKind::Voice;
            let receipt_part = tinychannels_bus::channel::MessageReceiptPart {
                platform_message_id: message_id.clone(),
                kind,
                index: 0,
                thread_id: conversation
                    .topic_id
                    .clone()
                    .or(conversation.thread_id.clone()),
                reply_to_id: message.reply_to_id.clone(),
                edit_token: Some(message_id.clone()),
                delete_token: Some(message_id.clone()),
                raw: Some(tinychannels_bus::channel::MessageReceiptSourceResult {
                    channel: Some("telegram".into()),
                    message_id: Some(message_id.clone()),
                    chat_id: Some(chat_id.clone()),
                    ..Default::default()
                }),
            };
            let mut receipt = tinychannels_bus::channel::MessageReceipt {
                idempotency_key: Some(message.idempotency_key.clone()),
                channel: Some(tinychannels_bus::ChannelRef {
                    id: "telegram".into(),
                    account_id: None,
                }),
                conversation: Some(conversation.clone()),
                primary_platform_message_id: Some(message_id.clone()),
                platform_message_ids: vec![message_id.clone()],
                parts: vec![receipt_part],
                thread_id: message.thread_id.clone(),
                reply_to_id: message.reply_to_id.clone(),
                edit_token: Some(message_id.clone()),
                delete_token: Some(message_id.clone()),
                sent_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64,
                raw: None,
            };
            let mut error = None;
            if transcript_fallback {
                let thread = thread_id
                    .as_deref()
                    .or(conversation.topic_id.as_deref())
                    .or(conversation.thread_id.as_deref());
                match self
                    .send_text_chunks_with_ids(
                        &caption,
                        &chat_id,
                        thread,
                        Self::parse_message_id(message.reply_to_id.as_deref()),
                    )
                    .await
                {
                    Ok(text_ids) => {
                        for (index, text_id) in text_ids.into_iter().enumerate() {
                            receipt.platform_message_ids.push(text_id.clone());
                            receipt
                                .parts
                                .push(tinychannels_bus::channel::MessageReceiptPart {
                                    platform_message_id: text_id.clone(),
                                    kind: tinychannels_bus::channel::MessageReceiptPartKind::Text,
                                    index: index + 1,
                                    thread_id: conversation
                                        .topic_id
                                        .clone()
                                        .or(conversation.thread_id.clone()),
                                    reply_to_id: message.reply_to_id.clone(),
                                    edit_token: Some(text_id.clone()),
                                    delete_token: Some(text_id.clone()),
                                    raw: Some(
                                        tinychannels_bus::channel::MessageReceiptSourceResult {
                                            channel: Some("telegram".into()),
                                            message_id: Some(text_id),
                                            chat_id: Some(chat_id.clone()),
                                            ..Default::default()
                                        },
                                    ),
                                });
                        }
                    }
                    Err(send_error) => {
                        let partial =
                            send_error.downcast_ref::<super::channel_send::PartialTextSendError>();
                        if let Some(partial) = partial {
                            for (index, text_id) in partial.message_ids.iter().enumerate() {
                                receipt.platform_message_ids.push(text_id.clone());
                                receipt
                                    .parts
                                    .push(tinychannels_bus::channel::MessageReceiptPart {
                                        platform_message_id: text_id.clone(),
                                        kind:
                                            tinychannels_bus::channel::MessageReceiptPartKind::Text,
                                        index: index + 1,
                                        thread_id: conversation
                                            .topic_id
                                            .clone()
                                            .or(conversation.thread_id.clone()),
                                        reply_to_id: message.reply_to_id.clone(),
                                        edit_token: Some(text_id.clone()),
                                        delete_token: Some(text_id.clone()),
                                        raw: Some(
                                            tinychannels_bus::channel::MessageReceiptSourceResult {
                                                channel: Some("telegram".into()),
                                                message_id: Some(text_id.clone()),
                                                chat_id: Some(chat_id.clone()),
                                                ..Default::default()
                                            },
                                        ),
                                    });
                            }
                            error = Some(partial.error.clone());
                            if let Some(error) = &mut error {
                                error.sent_before_error = true;
                                error.retryable = false;
                            }
                        } else {
                            error = Some(tinychannels_bus::channel::ChannelSendError {
                                kind: tinychannels_bus::channel::SendErrorKind::Unknown,
                                message:
                                    "Telegram sent voice audio but transcript delivery is uncertain"
                                        .into(),
                                sent_before_error: true,
                                retryable: false,
                                ..Default::default()
                            });
                        }
                    }
                }
            }
            return Ok(tinychannels_bus::channel::ChannelSendOutcome { receipt, error });
        }
        if message.parts.iter().any(|part| {
            matches!(
                part,
                tinychannels_bus::OutboundContentPart::Content {
                    part: tinychannels_bus::ContentPart::Image { .. }
                        | tinychannels_bus::ContentPart::Video { .. }
                        | tinychannels_bus::ContentPart::Audio { .. }
                        | tinychannels_bus::ContentPart::File { .. }
                        | tinychannels_bus::ContentPart::Sticker { .. }
                }
            )
        }) {
            return send_typed_media_with_receipt(self, message)
                .await
                .map_err(|error| {
                    if error
                        .downcast_ref::<super::channel_send::PartialTextSendError>()
                        .is_some()
                    {
                        error
                    } else {
                        anyhow::Error::new(TelegramPreDispatchError)
                    }
                });
        }
        let text_parts = message
            .parts
            .iter()
            .map(|part| match part {
                tinychannels_bus::OutboundContentPart::Content {
                    part: tinychannels_bus::ContentPart::Text { text, .. },
                } => Ok(text.as_str()),
                _ => anyhow::bail!("Telegram typed send does not support this content combination"),
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        if text_parts.is_empty() {
            anyhow::bail!("Telegram typed sends require at least one text part");
        }
        // Match the v21 text projection's ordered newline join while keeping
        // the provider's actual per-message IDs in the v22 receipt path.
        let text = strip_tool_call_tags(&text_parts.join("\n"));
        let mut conversation =
            message
                .conversation_ref
                .clone()
                .unwrap_or_else(|| tinychannels_bus::ConversationRef {
                    id: message.conversation_id.clone(),
                    thread_id: message.thread_id.clone(),
                    ..Default::default()
                });
        if conversation.thread_id.is_none() {
            conversation.thread_id = message.thread_id.clone();
        }
        let (chat_id, embedded_thread) = Self::parse_reply_target(&conversation.id);
        conversation.id = chat_id;
        if conversation.thread_id.is_none() {
            conversation.thread_id = embedded_thread;
        }
        let thread_id = conversation
            .thread_id
            .as_deref()
            .or(conversation.topic_id.as_deref())
            .or(message.thread_id.as_deref())
            .map(str::to_owned);
        let reply_to_id = match message.reply_to_id.as_deref() {
            Some(id) => Some(
                id.parse::<i64>()
                    .map_err(|_| anyhow::Error::new(TelegramBadFormatError))?,
            ),
            None => None,
        };
        let (ids, error, _sent_before_error) = match self
            .send_text_chunks_with_ids(&text, &conversation.id, thread_id.as_deref(), reply_to_id)
            .await
        {
            Ok(ids) => (ids, None, false),
            Err(error) => match error.downcast_ref::<super::channel_send::PartialTextSendError>() {
                Some(partial) => (
                    partial.message_ids.clone(),
                    Some(partial.error.clone()),
                    partial.sent_before_error || !partial.message_ids.is_empty(),
                ),
                None => return Err(error),
            },
        };
        let parts = ids
            .iter()
            .enumerate()
            .map(
                |(index, id)| tinychannels_bus::channel::MessageReceiptPart {
                    platform_message_id: id.clone(),
                    kind: tinychannels_bus::channel::MessageReceiptPartKind::Text,
                    index,
                    thread_id: thread_id.clone(),
                    reply_to_id: message.reply_to_id.clone(),
                    edit_token: Some(id.clone()),
                    delete_token: Some(id.clone()),
                    raw: Some(tinychannels_bus::channel::MessageReceiptSourceResult {
                        channel: Some("telegram".into()),
                        message_id: Some(id.clone()),
                        chat_id: Some(conversation.id.clone()),
                        ..Default::default()
                    }),
                },
            )
            .collect::<Vec<_>>();
        let single_part = (parts.len() == 1).then(|| parts[0].platform_message_id.clone());
        let receipt = tinychannels_bus::channel::MessageReceipt {
            idempotency_key: Some(message.idempotency_key.clone()),
            channel: Some(tinychannels_bus::ChannelRef {
                id: "telegram".into(),
                account_id: None,
            }),
            conversation: Some(conversation),
            primary_platform_message_id: parts.first().map(|part| part.platform_message_id.clone()),
            platform_message_ids: ids,
            parts,
            thread_id,
            reply_to_id: message.reply_to_id.clone(),
            edit_token: single_part.clone(),
            delete_token: single_part,
            sent_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            raw: None,
        };
        Ok(tinychannels_bus::channel::ChannelSendOutcome { receipt, error })
    }
}

impl TelegramChannel {
    pub(crate) async fn send_reaction_v2(
        &self,
        conversation: &tinychannels_bus::channel::ConversationRef,
        provider_message_id: &str,
        reaction: &tinychannels_bus::channel::ChannelReaction,
        remove: bool,
    ) -> anyhow::Result<()> {
        let message_id = provider_message_id
            .parse::<i64>()
            .map_err(|_| anyhow::Error::new(TelegramPreDispatchError))?;
        if conversation.id.trim().is_empty() {
            return Err(anyhow::Error::new(TelegramPreDispatchError));
        }
        if !remove {
            let applied = if let Some(custom_emoji_id) = reaction.custom_emoji_id.as_deref() {
                self.send_message_custom_reaction(&conversation.id, message_id, custom_emoji_id)
                    .await?
            } else if !reaction.value.trim().is_empty() {
                self.send_message_reaction(&conversation.id, message_id, &reaction.value)
                    .await?
            } else {
                return Err(anyhow::Error::new(TelegramPreDispatchError));
            };
            if !applied {
                anyhow::bail!("Telegram did not apply the requested reaction");
            }
            return Ok(());
        }
        let response = self
            .http_client()
            .post(self.api_url("setMessageReaction"))
            .json(&serde_json::json!({
                "chat_id": conversation.id,
                "message_id": message_id,
                "reaction": [],
                "is_big": false
            }))
            .send()
            .await?;
        let result = response.json::<serde_json::Value>().await?;
        if !result
            .get("ok")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            anyhow::bail!(
                "Telegram reaction removal failed: {}",
                result["description"]
            );
        }
        Ok(())
    }

    pub(crate) async fn edit_message_v2(
        &self,
        receipt: &tinychannels_bus::channel::MessageReceipt,
        intent: &tinychannels_bus::channel::ChannelOutboundIntent,
    ) -> anyhow::Result<tinychannels_bus::channel::MessageReceipt> {
        use tinychannels_bus::OutboundPayload;
        let text = match &intent.payload {
            OutboundPayload::Text { text } => text.clone(),
            OutboundPayload::Parts { parts } => {
                let mut texts = Vec::with_capacity(parts.len());
                for part in parts {
                    match part {
                        tinychannels_bus::OutboundContentPart::Content {
                            part: tinychannels_bus::ContentPart::Text { text, .. },
                        } => texts.push(text.as_str()),
                        _ => return Err(anyhow::Error::new(TelegramPreDispatchError)),
                    }
                }
                if texts.is_empty() {
                    return Err(anyhow::Error::new(TelegramPreDispatchError));
                }
                texts.join("\n")
            }
            _ => return Err(anyhow::Error::new(TelegramPreDispatchError)),
        };
        let conversation = receipt
            .conversation
            .as_ref()
            .ok_or_else(|| anyhow::Error::new(TelegramPreDispatchError))?;
        if receipt
            .channel
            .as_ref()
            .is_some_and(|channel| channel.id != "telegram")
            || intent
                .channel_ref
                .as_ref()
                .is_some_and(|channel| channel.id != "telegram")
            || intent
                .channel_ref
                .as_ref()
                .zip(receipt.channel.as_ref())
                .is_some_and(|(intent, receipt)| intent != receipt)
            || intent.conversation_id != conversation.id
            || intent
                .conversation_ref
                .as_ref()
                .is_some_and(|target| target != conversation)
        {
            return Err(anyhow::Error::new(TelegramPreDispatchError));
        }
        let provider_message_id = receipt
            .primary_platform_message_id
            .as_deref()
            .or_else(|| {
                receipt
                    .parts
                    .first()
                    .map(|part| part.platform_message_id.as_str())
            })
            .ok_or_else(|| anyhow::Error::new(TelegramPreDispatchError))?
            .parse::<i64>()
            .map_err(|_| anyhow::Error::new(TelegramPreDispatchError))?;
        let response = self
            .http_client()
            .post(self.api_url("editMessageText"))
            .json(&serde_json::json!({
                "chat_id": conversation.id,
                "message_id": provider_message_id,
                "text": markdown_v2_text(&strip_tool_call_tags(&text)),
                "parse_mode": "MarkdownV2"
            }))
            .send()
            .await?;
        let result = response.json::<serde_json::Value>().await?;
        if !result
            .get("ok")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            anyhow::bail!("Telegram editMessageText failed: {}", result["description"]);
        }
        Ok(receipt.clone())
    }

    pub(crate) async fn delete_message_v2(
        &self,
        receipt: &tinychannels_bus::channel::MessageReceipt,
    ) -> anyhow::Result<()> {
        let conversation = receipt
            .conversation
            .as_ref()
            .ok_or_else(|| anyhow::Error::new(TelegramPreDispatchError))?;
        if receipt
            .channel
            .as_ref()
            .is_some_and(|channel| channel.id != "telegram")
            || conversation.id.trim().is_empty()
        {
            return Err(anyhow::Error::new(TelegramPreDispatchError));
        }
        let provider_message_id = receipt
            .primary_platform_message_id
            .as_deref()
            .or_else(|| {
                receipt
                    .parts
                    .first()
                    .map(|part| part.platform_message_id.as_str())
            })
            .ok_or_else(|| anyhow::Error::new(TelegramPreDispatchError))?
            .parse::<i64>()
            .map_err(|_| anyhow::Error::new(TelegramPreDispatchError))?;
        let response = self
            .http_client()
            .post(self.api_url("deleteMessage"))
            .json(&serde_json::json!({
                "chat_id": conversation.id,
                "message_id": provider_message_id
            }))
            .send()
            .await?;
        let result = response.json::<serde_json::Value>().await?;
        if !result
            .get("ok")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
            || !result
                .get("result")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
        {
            anyhow::bail!("Telegram deleteMessage failed: {}", result["description"]);
        }
        Ok(())
    }
}

impl TelegramChannel {
    async fn send_draft_provider(&self, message: &SendMessage) -> anyhow::Result<Option<String>> {
        if self.stream_mode == StreamMode::Off {
            return Ok(None);
        }

        let (chat_id, recipient_thread_id) = Self::parse_reply_target(&message.recipient);
        let thread_id = message
            .topic_id
            .as_deref()
            .or(recipient_thread_id.as_deref());
        let parent_message_id = Self::parse_message_id(
            message
                .reply_to_id
                .as_deref()
                .or(message.thread_ts.as_deref()),
        );
        let initial_text = if message.content.is_empty() {
            "...".to_string()
        } else {
            message.content.clone()
        };

        let mut body = serde_json::json!({
            "chat_id": chat_id,
            "text": initial_text,
        });
        if let Some(tid) = thread_id {
            body["message_thread_id"] = serde_json::Value::String(tid.to_string());
        }
        if let Some(parent_id) = parent_message_id {
            body["reply_to_message_id"] = serde_json::Value::from(parent_id);
        }

        let resp = self
            .client
            .post(self.api_url("sendMessage"))
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let err = resp.text().await.unwrap_or_default();
            anyhow::bail!("Telegram sendMessage (draft) failed: {err}");
        }

        let resp_json: serde_json::Value = resp.json().await?;
        let message_id = resp_json
            .get("result")
            .and_then(|r| r.get("message_id"))
            .and_then(|id| id.as_i64())
            .map(|id| id.to_string());

        self.last_draft_edit
            .lock()
            .insert(chat_id.to_string(), std::time::Instant::now());

        Ok(message_id)
    }

    async fn update_draft_provider(
        &self,
        recipient: &str,
        message_id: &str,
        text: &str,
    ) -> anyhow::Result<()> {
        let (chat_id, _) = Self::parse_reply_target(recipient);

        // Rate-limit edits per chat
        {
            let last_edits = self.last_draft_edit.lock();
            if let Some(last_time) = last_edits.get(&chat_id) {
                let elapsed = u64::try_from(last_time.elapsed().as_millis()).unwrap_or(u64::MAX);
                if elapsed < self.draft_update_interval_ms {
                    return Ok(());
                }
            }
        }

        // Truncate to Telegram limit for mid-stream edits (UTF-8 safe)
        let display_text = if text.len() > TELEGRAM_MAX_MESSAGE_LENGTH {
            let mut end = 0;
            for (idx, ch) in text.char_indices() {
                let next = idx + ch.len_utf8();
                if next > TELEGRAM_MAX_MESSAGE_LENGTH {
                    break;
                }
                end = next;
            }
            &text[..end]
        } else {
            text
        };

        let message_id_parsed = match message_id.parse::<i64>() {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!("Invalid Telegram message_id '{message_id}': {e}");
                return Ok(());
            }
        };

        let body = serde_json::json!({
            "chat_id": chat_id,
            "message_id": message_id_parsed,
            "text": display_text,
        });

        let resp = self
            .client
            .post(self.api_url("editMessageText"))
            .json(&body)
            .send()
            .await?;

        if resp.status().is_success() {
            self.last_draft_edit
                .lock()
                .insert(chat_id.clone(), std::time::Instant::now());
        } else {
            let status = resp.status();
            let err = resp.text().await.unwrap_or_default();
            tracing::debug!("Telegram editMessageText failed ({status}): {err}");
        }

        Ok(())
    }

    async fn finalize_draft_provider(
        &self,
        recipient: &str,
        message_id: &str,
        text: &str,
        thread_ts: Option<&str>,
    ) -> anyhow::Result<()> {
        let text = &strip_tool_call_tags(text);
        let (chat_id, thread_id) = Self::parse_reply_target(recipient);
        let parent_message_id = Self::parse_message_id(thread_ts);

        // Clean up rate-limit tracking for this chat
        self.last_draft_edit.lock().remove(&chat_id);

        // If text exceeds limit, delete draft and send as chunked messages
        if text.len() > TELEGRAM_MAX_MESSAGE_LENGTH {
            let msg_id = match message_id.parse::<i64>() {
                Ok(id) => id,
                Err(e) => {
                    tracing::warn!("Invalid Telegram message_id '{message_id}': {e}");
                    return self
                        .send_text_chunks(text, &chat_id, thread_id.as_deref(), parent_message_id)
                        .await;
                }
            };

            // Delete the draft
            let _ = self
                .client
                .post(self.api_url("deleteMessage"))
                .json(&serde_json::json!({
                    "chat_id": chat_id,
                    "message_id": msg_id,
                }))
                .send()
                .await;

            // Fall back to chunked send
            return self
                .send_text_chunks(text, &chat_id, thread_id.as_deref(), parent_message_id)
                .await;
        }

        let msg_id = match message_id.parse::<i64>() {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!("Invalid Telegram message_id '{message_id}': {e}");
                return self
                    .send_text_chunks(text, &chat_id, thread_id.as_deref(), parent_message_id)
                    .await;
            }
        };

        // Try editing with Markdown formatting
        let body = serde_json::json!({
            "chat_id": chat_id,
            "message_id": msg_id,
            "text": markdown_v2_text(text),
            "parse_mode": "MarkdownV2",
        });

        let resp = self
            .client
            .post(self.api_url("editMessageText"))
            .json(&body)
            .send()
            .await?;

        if resp.status().is_success() {
            return Ok(());
        }

        // MarkdownV2 failed — retry without parse_mode
        let plain_body = serde_json::json!({
            "chat_id": chat_id,
            "message_id": msg_id,
            "text": text,
        });

        let resp = self
            .client
            .post(self.api_url("editMessageText"))
            .json(&plain_body)
            .send()
            .await?;

        if resp.status().is_success() {
            return Ok(());
        }

        // Edit failed entirely — fall back to new message
        tracing::warn!("Telegram finalize_draft edit failed; falling back to sendMessage");
        self.send_text_chunks(text, &chat_id, thread_id.as_deref(), parent_message_id)
            .await
    }
}

impl TelegramChannel {
    async fn deliver_listener_event(
        &self,
        typed_sink: Option<&(dyn crate::channel::ChannelInboundSink + Send + Sync)>,
        event: tinychannels_bus::ChannelEvent,
    ) -> anyhow::Result<()> {
        let ingress = tinychannels_bus::ChannelIngress::Event(event);
        if let Some(sink) = typed_sink {
            sink.push_ingress(ingress)
                .await
                .map_err(|error| anyhow::anyhow!(error.message))?;
        } else if let Some(events) = &self.events
            && let Err(error) = events.publish_ingress(ingress).await
        {
            tracing::debug!(%error, "failed to deliver Telegram listener event");
        }
        Ok(())
    }

    async fn deliver_listener_inbound(
        &self,
        typed_sink: Option<&(dyn crate::channel::ChannelInboundSink + Send + Sync)>,
        envelope: tinychannels_bus::ChannelInboundEnvelope,
    ) -> anyhow::Result<bool> {
        if let Some(sink) = typed_sink {
            sink.push_ingress(tinychannels_bus::ChannelIngress::Inbound(envelope))
                .await
                .map_err(|error| anyhow::anyhow!(error.message))?;
            return Ok(true);
        }
        if let Some(events) = &self.events {
            return events
                .publish_ingress(tinychannels_bus::ChannelIngress::Inbound(envelope))
                .await;
        }
        Ok(false)
    }

    async fn listen_loop(
        &self,
        legacy_tx: Option<tokio::sync::mpsc::Sender<ChannelMessage>>,
        typed_sink: Option<&(dyn crate::channel::ChannelInboundSink + Send + Sync)>,
        mut cancellation: tokio::sync::watch::Receiver<bool>,
    ) -> anyhow::Result<()> {
        let mut offset: i64 = 0;

        if self.mention_only {
            let _ = self.get_bot_username().await;
        }

        tracing::info!("Telegram channel listening for messages...");

        loop {
            if self.mention_only {
                let missing_username = self.bot_username.lock().is_none();
                if missing_username {
                    let _ = self.get_bot_username().await;
                }
            }

            let url = self.api_url("getUpdates");
            let body = serde_json::json!({
                "offset": offset,
                "timeout": 30,
                "allowed_updates": ["message", "edited_message", "message_reaction", "callback_query"]
            });

            let response = tokio::select! {
                biased;
                changed = cancellation.changed() => {
                    if changed.is_err() || *cancellation.borrow() {
                        return Ok(());
                    }
                    continue;
                }
                response = self.http_client().post(&url).json(&body).send() => response,
            };
            let resp = match response {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!("Telegram poll error: {e}");
                    tokio::select! {
                        _ = cancellation.changed() => return Ok(()),
                        _ = tokio::time::sleep(self.poll_error_backoff) => {}
                    }
                    continue;
                }
            };

            let parsed = tokio::select! {
                biased;
                changed = cancellation.changed() => {
                    if changed.is_err() || *cancellation.borrow() {
                        return Ok(());
                    }
                    continue;
                }
                parsed = resp.json() => parsed,
            };
            let data: serde_json::Value = match parsed {
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!("Telegram parse error: {e}");
                    tokio::select! {
                        _ = cancellation.changed() => return Ok(()),
                        _ = tokio::time::sleep(self.poll_error_backoff) => {}
                    }
                    continue;
                }
            };

            let ok = data
                .get("ok")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true);
            if !ok {
                let error_code = data
                    .get("error_code")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or_default();
                let description = data
                    .get("description")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("unknown Telegram API error");

                if error_code == 409 {
                    let webhook_blocks_polling = description.to_lowercase().contains("webhook");
                    if webhook_blocks_polling {
                        tracing::warn!(
                            "[telegram] getUpdates conflict (409): webhook is active; calling deleteWebhook"
                        );
                        if self.delete_webhook_for_long_polling().await {
                            tracing::info!("[telegram] deleteWebhook ok; retrying getUpdates");
                            continue;
                        }
                        tracing::warn!("[telegram] deleteWebhook did not succeed; backing off");
                    } else {
                        tracing::warn!(
                            "Telegram polling conflict (409): {description}. \
Ensure only one `openhuman` process is using this bot token."
                        );
                    }
                    tokio::select! {
                        _ = cancellation.changed() => return Ok(()),
                        _ = tokio::time::sleep(self.poll_conflict_backoff) => {}
                    }
                } else {
                    tracing::warn!(
                        "Telegram getUpdates API error (code={}): {description}",
                        error_code
                    );
                    tokio::select! {
                        _ = cancellation.changed() => return Ok(()),
                        _ = tokio::time::sleep(self.poll_error_backoff) => {}
                    }
                }
                continue;
            }

            if let Some(results) = data.get("result").and_then(serde_json::Value::as_array) {
                for update in results {
                    let update_id = update
                        .get("update_id")
                        .and_then(serde_json::Value::as_i64)
                        .unwrap_or_default();
                    if update_id > 0 && self.has_seen_update_id(update_id) {
                        offset = offset.max(update_id.saturating_add(1));
                        continue;
                    }
                    // The update is acknowledged locally only after its
                    // provider-owned action or typed ingress has succeeded.
                    let commit_update = |offset: &mut i64| {
                        if update_id > 0 {
                            self.track_update_id(update_id);
                        }
                        *offset = (*offset).max(update_id.saturating_add(1));
                    };

                    if let Some(query_id) = update
                        .get("callback_query")
                        .and_then(|query| query.get("id"))
                        .and_then(serde_json::Value::as_str)
                    {
                        self.acknowledge_callback(query_id).await;
                        if !self.callback_is_allowed(update) {
                            tracing::debug!("dropped Telegram callback from a disallowed user");
                            commit_update(&mut offset);
                            continue;
                        }
                        if let Some(event) = Self::callback_event(update) {
                            tokio::select! {
                                _ = cancellation.changed() => return Ok(()),
                                result = self.deliver_listener_event(typed_sink, event) => result?,
                            }
                        }
                        commit_update(&mut offset);
                        continue;
                    }

                    let reactions = self.parse_update_reactions(update);
                    if update.get("message_reaction").is_some() {
                        if !reactions.is_empty() {
                            for reaction in reactions {
                                tracing::info!(
                                    sender = reaction.sender,
                                    reply_target = reaction.reply_target,
                                    target_message_id = reaction.target_message_id,
                                    emoji = reaction.emoji,
                                    "Telegram reaction received"
                                );
                                if typed_sink.is_some() || self.events.is_some() {
                                    let source_id = reaction.update_id.clone();
                                    let conversation = tinychannels_bus::ConversationRef {
                                        kind: reaction.conversation_kind,
                                        id: reaction.reply_target.clone(),
                                        topic_id: reaction.thread_id.clone(),
                                        ..Default::default()
                                    };
                                    let reaction_kind = if reaction.added {
                                        tinychannels_bus::ChannelEventKind::ReactionAdded {
                                            message_id: format!(
                                                "telegram_{}_{}",
                                                reaction.reply_target, reaction.target_message_id
                                            ),
                                            provider_message_id: Some(
                                                reaction.target_message_id.clone(),
                                            ),
                                            actor: tinychannels_bus::SenderRef {
                                                id: reaction.sender_id.clone(),
                                                name: Some(reaction.sender.clone()),
                                                ..Default::default()
                                            },
                                            reaction: tinychannels_bus::ChannelReaction {
                                                value: reaction.emoji.clone(),
                                                custom_emoji_id: reaction.custom_emoji_id.clone(),
                                            },
                                        }
                                    } else {
                                        tinychannels_bus::ChannelEventKind::ReactionRemoved {
                                            message_id: format!(
                                                "telegram_{}_{}",
                                                reaction.reply_target, reaction.target_message_id
                                            ),
                                            provider_message_id: Some(
                                                reaction.target_message_id.clone(),
                                            ),
                                            actor: tinychannels_bus::SenderRef {
                                                id: reaction.sender_id.clone(),
                                                name: Some(reaction.sender.clone()),
                                                ..Default::default()
                                            },
                                            reaction: tinychannels_bus::ChannelReaction {
                                                value: reaction.emoji.clone(),
                                                custom_emoji_id: reaction.custom_emoji_id.clone(),
                                            },
                                        }
                                    };
                                    let event = tinychannels_bus::ChannelEvent {
                                        version: 2,
                                        event_id: source_id
                                            .as_ref()
                                            .map(|id| {
                                                format!(
                                                    "telegram:update:{id}:reaction:{}:{}:{}:{}",
                                                    if reaction.added {
                                                        "added"
                                                    } else {
                                                        "removed"
                                                    },
                                                    reaction.reply_target,
                                                    reaction.target_message_id,
                                                    reaction.emoji
                                                )
                                            })
                                            .unwrap_or_else(|| {
                                                format!(
                                                    "telegram:reaction:{}:{}:{}:{}:{}",
                                                    reaction.reply_target,
                                                    reaction.target_message_id,
                                                    reaction.sender_id,
                                                    reaction.emoji,
                                                    if reaction.added {
                                                        "added"
                                                    } else {
                                                        "removed"
                                                    }
                                                )
                                            }),
                                        source_event_id: source_id,
                                        source_timestamp: reaction.timestamp,
                                        channel: tinychannels_bus::ChannelRef {
                                            id: "telegram".into(),
                                            account_id: None,
                                        },
                                        conversation: Some(conversation),
                                        event: reaction_kind,
                                    };
                                    tokio::select! {
                                        _ = cancellation.changed() => return Ok(()),
                                        result = self.deliver_listener_event(typed_sink, event) => result?,
                                    }
                                }
                            }
                            commit_update(&mut offset);
                            continue;
                        }
                        commit_update(&mut offset);
                        continue;
                    }

                    // Telegram edits arrive as a distinct update field. Keep
                    // them out of the inbound-new-message path even when no
                    // host event sink is installed.
                    if let Some(edited) = update.get("edited_message") {
                        if let Some(message) = self.parse_update_message(update)
                            && (typed_sink.is_some() || self.events.is_some())
                        {
                            let source_id = update
                                .get("update_id")
                                .and_then(serde_json::Value::as_i64)
                                .map(|id| id.to_string());
                            let thread_id = edited
                                .get("message_thread_id")
                                .and_then(serde_json::Value::as_i64)
                                .map(|id| id.to_string());
                            let event = tinychannels_bus::ChannelEvent {
                                version: 2,
                                event_id: source_id
                                    .as_ref()
                                    .map(|id| format!("telegram:update:{id}:edit:{}", message.id))
                                    .unwrap_or_else(|| format!("telegram:edit:{}", message.id)),
                                source_event_id: source_id,
                                source_timestamp: edited
                                    .get("edit_date")
                                    .and_then(serde_json::Value::as_u64)
                                    .map(|seconds| seconds.saturating_mul(1000)),
                                channel: tinychannels_bus::ChannelRef {
                                    id: "telegram".into(),
                                    account_id: None,
                                },
                                conversation: Some(tinychannels_bus::ConversationRef {
                                    kind: match edited
                                        .get("chat")
                                        .and_then(|chat| chat.get("type"))
                                        .and_then(serde_json::Value::as_str)
                                    {
                                        Some("private") => tinychannels_bus::ConversationKind::Dm,
                                        Some("group" | "supergroup") => {
                                            tinychannels_bus::ConversationKind::Group
                                        }
                                        Some("channel") => {
                                            tinychannels_bus::ConversationKind::Channel
                                        }
                                        _ => tinychannels_bus::ConversationKind::Unknown,
                                    },
                                    id: edited
                                        .get("chat")
                                        .and_then(|chat| chat.get("id"))
                                        .and_then(serde_json::Value::as_i64)
                                        .map(|id| id.to_string())
                                        .unwrap_or_else(|| message.reply_target.clone()),
                                    topic_id: thread_id,
                                    ..Default::default()
                                }),
                                event: tinychannels_bus::ChannelEventKind::MessageUpdated {
                                    message_id: message.id,
                                    provider_message_id: edited
                                        .get("message_id")
                                        .and_then(serde_json::Value::as_i64)
                                        .map(|id| id.to_string()),
                                    parts: vec![tinychannels_bus::ContentPart::Text {
                                        text: message.content,
                                        markdown_dialect: None,
                                    }],
                                },
                            };
                            tokio::select! {
                                _ = cancellation.changed() => return Ok(()),
                                result = self.deliver_listener_event(typed_sink, event) => result?,
                            }
                        }
                        commit_update(&mut offset);
                        continue;
                    }

                    let Some(msg) = self.parse_update_message_or_voice(update).await else {
                        self.handle_unauthorized_message(update).await;
                        commit_update(&mut offset);
                        continue;
                    };

                    let voice_note = match self.voice_note_for_ingress(update, &msg).await {
                        Ok(note) => note,
                        Err(error) if Self::parse_update_voice_attachment(update).is_some() => {
                            tracing::warn!(
                                "Telegram voice could not be durably persisted: {error}"
                            );
                            None
                        }
                        Err(error) => return Err(error),
                    };
                    let media_parts = match self.media_parts_for_ingress(update, &msg).await {
                        Ok(parts) => parts,
                        Err(_) => {
                            tracing::warn!(
                                "Telegram attachment could not be persisted as typed media"
                            );
                            return Err(anyhow::anyhow!("Telegram attachment persistence failed"));
                        }
                    };
                    let ingress =
                        Self::inbound_envelope_with_media(update, &msg, voice_note, media_parts);
                    let delivered = if typed_sink.is_some() || self.events.is_some() {
                        tokio::select! {
                            _ = cancellation.changed() => return Ok(()),
                            result = self.deliver_listener_inbound(typed_sink, ingress) => result?,
                        }
                    } else {
                        false
                    };
                    if !delivered
                        && let Some(tx) = &legacy_tx
                        && tx.send(msg).await.is_err()
                    {
                        return Ok(());
                    }
                    commit_update(&mut offset);
                }
            }
        }
    }
}

fn telegram_receipt_part(
    id: &str,
    kind: tinychannels_bus::channel::MessageReceiptPartKind,
    index: usize,
    conversation: &tinychannels_bus::ConversationRef,
    thread_id: &Option<String>,
    reply_to_id: Option<&str>,
) -> tinychannels_bus::channel::MessageReceiptPart {
    tinychannels_bus::channel::MessageReceiptPart {
        platform_message_id: id.to_owned(),
        kind,
        index,
        thread_id: thread_id.clone(),
        reply_to_id: reply_to_id.map(str::to_owned),
        edit_token: Some(id.to_owned()),
        delete_token: Some(id.to_owned()),
        raw: Some(tinychannels_bus::channel::MessageReceiptSourceResult {
            channel: Some("telegram".into()),
            message_id: Some(id.to_owned()),
            chat_id: Some(conversation.id.clone()),
            conversation_id: Some(conversation.id.clone()),
            ..Default::default()
        }),
    }
}

fn telegram_image_dimensions(content_type: &str, bytes: &[u8]) -> Option<(u32, u32)> {
    match content_type {
        "image/png" if bytes.get(..8) == Some(b"\x89PNG\r\n\x1a\n") => Some((
            u32::from_be_bytes(bytes.get(16..20)?.try_into().ok()?),
            u32::from_be_bytes(bytes.get(20..24)?.try_into().ok()?),
        )),
        "image/jpeg" if bytes.starts_with(&[0xff, 0xd8]) => {
            let mut offset = 2;
            while offset + 4 <= bytes.len() {
                if bytes[offset] != 0xff {
                    offset += 1;
                    continue;
                }
                while bytes.get(offset) == Some(&0xff) {
                    offset += 1;
                }
                let marker = *bytes.get(offset)?;
                offset += 1;
                if matches!(marker, 0xd8 | 0xd9 | 0x01 | 0xd0..=0xd7) {
                    continue;
                }
                let segment_len =
                    u16::from_be_bytes(bytes.get(offset..offset + 2)?.try_into().ok()?) as usize;
                if segment_len < 2 || offset + segment_len > bytes.len() {
                    return None;
                }
                if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
                    let height =
                        u16::from_be_bytes(bytes.get(offset + 3..offset + 5)?.try_into().ok()?)
                            as u32;
                    let width =
                        u16::from_be_bytes(bytes.get(offset + 5..offset + 7)?.try_into().ok()?)
                            as u32;
                    return Some((width, height));
                }
                offset += segment_len;
            }
            None
        }
        _ => None,
    }
}

async fn send_typed_media_with_receipt(
    channel: &TelegramChannel,
    message: &tinychannels_bus::ChannelOutboundMessage,
) -> anyhow::Result<tinychannels_bus::channel::ChannelSendOutcome> {
    let mut conversation =
        message
            .conversation_ref
            .clone()
            .unwrap_or_else(|| tinychannels_bus::ConversationRef {
                id: message.conversation_id.clone(),
                thread_id: message.thread_id.clone(),
                ..Default::default()
            });
    if conversation.thread_id.is_none() {
        conversation.thread_id = message.thread_id.clone();
    }
    if conversation.id.trim().is_empty() {
        anyhow::bail!("Telegram media send requires a conversation identity");
    }
    let channel_ref = message
        .channel_ref
        .clone()
        .unwrap_or_else(|| tinychannels_bus::ChannelRef {
            id: "telegram".into(),
            account_id: None,
        });
    let configured_instance = channel_ref
        .account_id
        .clone()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::Error::new(TelegramPreDispatchError))?;
    let turn_id = message
        .turn_id
        .clone()
        .filter(|turn_id| !turn_id.trim().is_empty())
        .ok_or_else(|| anyhow::Error::new(TelegramPreDispatchError))?;
    let scope = tinychannels_bus::HostVoiceScope {
        configured_instance,
        channel: channel_ref.clone(),
        conversation: conversation.clone(),
        turn_id,
    };
    let thread_id = conversation
        .thread_id
        .as_deref()
        .or(conversation.topic_id.as_deref())
        .or(message.thread_id.as_deref())
        .map(str::to_owned);
    if thread_id
        .as_deref()
        .is_some_and(|thread_id| thread_id.parse::<i64>().is_err())
    {
        return Err(TelegramBadFormatError.into());
    }
    if let Some(reply_to) = message.reply_to_id.as_deref() {
        reply_to
            .parse::<i64>()
            .map_err(|_| anyhow::Error::new(TelegramBadFormatError))?;
    }
    let repository = channel
        .media_repository
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Telegram media upload requires scoped host storage"))?;

    for part in &message.parts {
        match part {
            tinychannels_bus::OutboundContentPart::Content {
                part:
                    tinychannels_bus::ContentPart::Text { .. }
                    | tinychannels_bus::ContentPart::Image { .. }
                    | tinychannels_bus::ContentPart::Video { .. }
                    | tinychannels_bus::ContentPart::Audio { .. }
                    | tinychannels_bus::ContentPart::File { .. }
                    | tinychannels_bus::ContentPart::Sticker { .. },
            } => {}
            _ => anyhow::bail!("Telegram media send does not support this content combination"),
        }
    }

    let mut receipt_parts = Vec::new();
    let mut failed = None;
    for part in &message.parts {
        let (method, media_kind, media, caption, title, duration_ms, fallback_name) = match part {
            tinychannels_bus::OutboundContentPart::Content {
                part: tinychannels_bus::ContentPart::Text { text, .. },
            } => {
                let text = strip_tool_call_tags(text);
                match channel
                    .send_text_chunks_with_ids(
                        &text,
                        &conversation.id,
                        thread_id.as_deref(),
                        message
                            .reply_to_id
                            .as_deref()
                            .and_then(|id| id.parse::<i64>().ok()),
                    )
                    .await
                {
                    Ok(ids) => {
                        for id in ids {
                            receipt_parts.push(telegram_receipt_part(
                                &id,
                                tinychannels_bus::channel::MessageReceiptPartKind::Text,
                                receipt_parts.len(),
                                &conversation,
                                &thread_id,
                                message.reply_to_id.as_deref(),
                            ));
                        }
                    }
                    Err(error) => {
                        if let Some(partial) =
                            error.downcast_ref::<super::channel_send::PartialTextSendError>()
                        {
                            for id in &partial.message_ids {
                                receipt_parts.push(telegram_receipt_part(
                                    id,
                                    tinychannels_bus::channel::MessageReceiptPartKind::Text,
                                    receipt_parts.len(),
                                    &conversation,
                                    &thread_id,
                                    message.reply_to_id.as_deref(),
                                ));
                            }
                        }
                        failed = Some(error);
                    }
                }
                if failed.is_some() {
                    break;
                }
                continue;
            }
            tinychannels_bus::OutboundContentPart::Content {
                part: tinychannels_bus::ContentPart::Image { media, caption, .. },
            } => (
                "sendPhoto",
                tinychannels_bus::MediaKind::Image,
                media,
                caption.as_deref(),
                None,
                None,
                "photo",
            ),
            tinychannels_bus::OutboundContentPart::Content {
                part:
                    tinychannels_bus::ContentPart::Video {
                        media,
                        caption,
                        duration_ms,
                        ..
                    },
            } => (
                "sendVideo",
                tinychannels_bus::MediaKind::Video,
                media,
                caption.as_deref(),
                None,
                *duration_ms,
                "video",
            ),
            tinychannels_bus::OutboundContentPart::Content {
                part:
                    tinychannels_bus::ContentPart::Audio {
                        media,
                        title,
                        transcript,
                        duration_ms,
                        ..
                    },
            } => (
                "sendAudio",
                tinychannels_bus::MediaKind::Audio,
                media,
                transcript.as_deref(),
                title.as_deref(),
                *duration_ms,
                "audio",
            ),
            tinychannels_bus::OutboundContentPart::Content {
                part:
                    tinychannels_bus::ContentPart::File {
                        media,
                        file_name,
                        caption,
                    },
            } => (
                "sendDocument",
                tinychannels_bus::MediaKind::Document,
                media,
                caption.as_deref(),
                None,
                None,
                file_name.as_deref().unwrap_or("file"),
            ),
            tinychannels_bus::OutboundContentPart::Content {
                part: tinychannels_bus::ContentPart::Sticker { media, .. },
            } => (
                "sendSticker",
                tinychannels_bus::MediaKind::Image,
                media,
                None,
                None,
                None,
                "sticker",
            ),
            _ => unreachable!("content combination was validated above"),
        };
        let Some(opaque_id) = media
            .opaque_id
            .as_deref()
            .filter(|id| !id.trim().is_empty())
        else {
            if receipt_parts.is_empty() {
                anyhow::bail!("Telegram media has no host-owned opaque reference");
            }
            failed = Some(anyhow::anyhow!(
                "Telegram media has no host-owned opaque reference"
            ));
            break;
        };
        let stored = match repository
            .read_media(scope.clone(), opaque_id.to_owned(), 20 * 1024 * 1024)
            .await
        {
            Ok(stored) => stored,
            Err(error) if receipt_parts.is_empty() => return Err(error),
            Err(_) => {
                failed = Some(anyhow::anyhow!("Telegram scoped media could not be read"));
                break;
            }
        };
        if stored.media.kind != media_kind
            || !crate::host::is_allowed_content_type(media_kind, &stored.content_type)
            || stored.bytes.is_empty()
            || stored.bytes.len() > 20 * 1024 * 1024
        {
            if receipt_parts.is_empty() {
                anyhow::bail!("Telegram media failed upload validation");
            }
            failed = Some(anyhow::anyhow!("Telegram media failed upload validation"));
            break;
        }
        let file_name = stored
            .file_name
            .as_deref()
            .unwrap_or(fallback_name)
            .to_owned();
        let caption = caption.map(strip_tool_call_tags);
        if caption
            .as_ref()
            .is_some_and(|text| text.chars().count() > 1024)
        {
            if receipt_parts.is_empty() {
                anyhow::bail!("Telegram media caption exceeds the 1024-character limit");
            }
            failed = Some(anyhow::anyhow!(
                "Telegram media caption exceeds the 1024-character limit"
            ));
            break;
        }
        let upload_plan = (|| -> anyhow::Result<(&str, &str, usize, Option<u64>)> {
            Ok(match media_kind {
                tinychannels_bus::MediaKind::Image
                    if method == "sendSticker" && stored.content_type == "image/webp" =>
                {
                    ("sendSticker", "sticker", 20 * 1024 * 1024, None)
                }
                tinychannels_bus::MediaKind::Image if method == "sendSticker" => {
                    anyhow::bail!("Telegram sticker upload supports WEBP stickers only")
                }
                tinychannels_bus::MediaKind::Image
                    if matches!(stored.content_type.as_str(), "image/jpeg" | "image/png") =>
                {
                    let Some((width, height)) =
                        telegram_image_dimensions(&stored.content_type, &stored.bytes)
                    else {
                        anyhow::bail!("Telegram photo dimensions could not be validated")
                    };
                    if width == 0
                        || height == 0
                        || width.saturating_add(height) > 10_000
                        || width.max(height) > width.min(height).saturating_mul(20)
                    {
                        anyhow::bail!("Telegram photo dimensions exceed provider limits")
                    }
                    ("sendPhoto", "photo", 10 * 1024 * 1024, duration_ms)
                }
                tinychannels_bus::MediaKind::Image => {
                    ("sendDocument", "document", 20 * 1024 * 1024, None)
                }
                tinychannels_bus::MediaKind::Video if stored.content_type == "video/mp4" => {
                    ("sendVideo", "video", 20 * 1024 * 1024, duration_ms)
                }
                tinychannels_bus::MediaKind::Video => {
                    ("sendDocument", "document", 20 * 1024 * 1024, None)
                }
                tinychannels_bus::MediaKind::Audio
                    if matches!(stored.content_type.as_str(), "audio/mpeg" | "audio/mp4") =>
                {
                    ("sendAudio", "audio", 20 * 1024 * 1024, duration_ms)
                }
                tinychannels_bus::MediaKind::Audio => {
                    anyhow::bail!("Telegram sendAudio accepts only MP3 or M4A media")
                }
                tinychannels_bus::MediaKind::Document => {
                    ("sendDocument", "document", 20 * 1024 * 1024, None)
                }
                tinychannels_bus::MediaKind::Unknown => {
                    anyhow::bail!("Telegram media kind is unsupported")
                }
            })
        })();
        let (upload_method, upload_field, upload_limit, upload_duration) = match upload_plan {
            Ok(plan) => plan,
            Err(error) if receipt_parts.is_empty() => return Err(error),
            Err(error) => {
                failed = Some(error);
                break;
            }
        };
        if stored.bytes.len() > upload_limit {
            if receipt_parts.is_empty() {
                anyhow::bail!("Telegram media exceeds the selected upload method limit");
            }
            failed = Some(anyhow::anyhow!(
                "Telegram media exceeds the selected upload method limit"
            ));
            break;
        }
        let content_type = stored.content_type.clone();
        let receipt_index = receipt_parts.len();
        match channel
            .send_media_bytes_with_id(super::channel_send::TelegramMediaUpload {
                method: upload_method,
                field: upload_field,
                chat_id: &conversation.id,
                thread_id: thread_id.as_deref(),
                reply_to_id: message.reply_to_id.as_deref(),
                caption: caption.as_deref(),
                title,
                duration_ms: upload_duration,
                bytes: stored.bytes,
                content_type: &content_type,
                file_name: &file_name,
            })
            .await
        {
            Ok(id) => receipt_parts.push(telegram_receipt_part(
                &id,
                tinychannels_bus::channel::MessageReceiptPartKind::Media,
                receipt_index,
                &conversation,
                &thread_id,
                message.reply_to_id.as_deref(),
            )),
            Err(_) => {
                failed = Some(super::channel_send::partial_text_send_error(
                    Vec::new(),
                    "Telegram media upload outcome is uncertain".into(),
                    false,
                    true,
                ));
                break;
            }
        }
    }
    if receipt_parts.is_empty() {
        return Err(
            failed.unwrap_or_else(|| anyhow::anyhow!("Telegram media send produced no receipt"))
        );
    }
    let ids = receipt_parts
        .iter()
        .map(|part| part.platform_message_id.clone())
        .collect::<Vec<_>>();
    let single = (ids.len() == 1).then(|| ids[0].clone());
    let error = failed.map(|failure| tinychannels_bus::channel::ChannelSendError {
        kind: tinychannels_bus::channel::SendErrorKind::Unknown,
        message: if failure
            .downcast_ref::<super::channel_send::PartialTextSendError>()
            .is_some()
        {
            "Telegram sent only part of the typed message; retry is unsafe".into()
        } else {
            "Telegram typed media send is partial or uncertain; retry is unsafe".into()
        },
        sent_before_error: true,
        retryable: false,
        ..Default::default()
    });
    let receipt = tinychannels_bus::channel::MessageReceipt {
        idempotency_key: Some(message.idempotency_key.clone()),
        channel: Some(channel_ref),
        conversation: Some(conversation),
        primary_platform_message_id: ids.first().cloned(),
        platform_message_ids: ids,
        parts: receipt_parts,
        thread_id,
        reply_to_id: message.reply_to_id.clone(),
        edit_token: single.clone(),
        delete_token: single,
        sent_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64,
        raw: None,
    };
    Ok(tinychannels_bus::channel::ChannelSendOutcome { receipt, error })
}

impl TelegramChannel {
    pub(super) async fn send_compat_message(&self, message: &SendMessage) -> anyhow::Result<()> {
        // This shim only carries text across the legacy provider boundary. Media and
        // reactions must use the typed v2 ports; marker-looking text stays text.
        let (chat_id, encoded_thread) = Self::parse_reply_target(&message.recipient);
        let topic_id = message.topic_id.as_deref().or(encoded_thread.as_deref());
        let reply_to = message
            .reply_to_id
            .as_deref()
            .or(message.thread_ts.as_deref())
            .and_then(|id| id.parse::<i64>().ok());
        let content = super::text::strip_tool_call_tags(&message.content);
        self.send_text_chunks(&content, &chat_id, topic_id, reply_to)
            .await
    }

    pub(crate) fn declared_capabilities(&self) -> tinychannels_bus::ChannelCapabilities {
        super::capabilities::declared(self)
    }

    pub(crate) fn proactive_target(&self) -> Option<String> {
        self.chat_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    }
}

#[async_trait]
impl crate::channel::ChannelAdapter for TelegramChannel {
    fn descriptor(&self) -> tinychannels_bus::channel::ChannelDescriptor {
        tinychannels_bus::channel::ChannelDescriptor {
            id: "telegram".into(),
            display_name: "Telegram".into(),
            metadata: std::collections::BTreeMap::from([(
                "adapterMode".into(),
                "native-v2".into(),
            )]),
            ..Default::default()
        }
    }

    fn declared_capabilities(&self) -> tinychannels_bus::ChannelCapabilities {
        TelegramChannel::declared_capabilities(self)
    }

    fn proactive_target(&self) -> Option<String> {
        TelegramChannel::proactive_target(self)
    }

    async fn start(
        &self,
        sink: &(dyn crate::channel::ChannelInboundSink + Send + Sync),
    ) -> Result<(), crate::channel::ChannelSendError> {
        use std::sync::atomic::Ordering;

        if self
            .listener_running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(crate::channel::ChannelSendError {
                kind: tinychannels_bus::channel::SendErrorKind::Forbidden,
                message: "Telegram listener is already running".into(),
                retryable: false,
                ..Default::default()
            });
        }
        let _guard = TelegramListenerGuard(&self.listener_running);
        self.listener_cancel.send_replace(false);
        let mut stop = self.listener_cancel.subscribe();
        let poll_cancellation = stop.clone();
        tokio::select! {
            biased;
            _ = stop.wait_for(|stopped| *stopped) => Ok(()),
            result = self.listen_loop(None, Some(sink), poll_cancellation) => result,
        }
        .map_err(|error| crate::channel::ChannelSendError {
            kind: tinychannels_bus::channel::SendErrorKind::Transient,
            message: error.to_string(),
            retryable: true,
            sent_before_error: false,
            ..Default::default()
        })
    }

    async fn stop(&self) -> Result<(), crate::channel::ChannelSendError> {
        self.listener_cancel.send_replace(true);
        Ok(())
    }

    async fn send(
        &self,
        intent: tinychannels_bus::ChannelOutboundIntent,
    ) -> Result<tinychannels_bus::channel::MessageReceipt, crate::channel::ChannelSendError> {
        let message = tinychannels_bus::ChannelOutboundMessage::from_intent(&intent);
        let outcome = self
            .send_v2_with_receipt(&message)
            .await
            .map_err(typed_send_error)?;
        if let Some(error) = outcome.error {
            return Err(error);
        }
        Ok(outcome.receipt)
    }

    async fn send_typed(
        &self,
        message: &tinychannels_bus::ChannelOutboundMessage,
    ) -> Result<tinychannels_bus::channel::ChannelSendOutcome, crate::channel::ChannelSendError>
    {
        self.send_v2_with_receipt(message)
            .await
            .map_err(typed_send_error)
    }

    fn validate_typed_send(
        &self,
        message: &tinychannels_bus::ChannelOutboundMessage,
    ) -> Result<(), crate::channel::ChannelSendError> {
        if message
            .reply_to_id
            .as_deref()
            .is_some_and(|id| id.parse::<i64>().is_err())
        {
            return Err(crate::channel::ChannelSendError {
                kind: crate::channel::SendErrorKind::BadFormat,
                message: "Telegram typed send was rejected before provider dispatch".into(),
                retryable: false,
                ..Default::default()
            });
        }
        if self.media_repository.is_none()
            && message.parts.iter().any(|part| {
                matches!(
                    part,
                    tinychannels_bus::OutboundContentPart::Content {
                        part: tinychannels_bus::ContentPart::Image { .. }
                            | tinychannels_bus::ContentPart::Video { .. }
                            | tinychannels_bus::ContentPart::Audio { .. }
                            | tinychannels_bus::ContentPart::File { .. }
                            | tinychannels_bus::ContentPart::Sticker { .. }
                    }
                )
            })
        {
            return Err(crate::channel::ChannelSendError::unsupported(
                "Telegram typed adapter does not support this media part",
            ));
        }
        Ok(())
    }

    async fn status(
        &self,
    ) -> Result<
        tinychannels_runtime::controllers::ChannelAccountSnapshot,
        crate::channel::ChannelSendError,
    > {
        let health_state = if self.health_status().await {
            "healthy"
        } else {
            "unhealthy"
        };
        Ok(tinychannels_bus::controllers::ChannelAccountSnapshot {
            channel_id: "telegram".into(),
            states: vec![
                tinychannels_bus::controllers::ChannelAccountState::Configured,
                tinychannels_bus::controllers::ChannelAccountState::Enabled,
            ],
            health_state: Some(health_state.into()),
            ..Default::default()
        })
    }
}

#[async_trait]
impl crate::channel::LegacyChannelReaction for TelegramChannel {
    async fn send_reaction(
        &self,
        channel: &tinychannels_bus::channel::ChannelRef,
        conversation: &tinychannels_bus::channel::ConversationRef,
        provider_message_id: &str,
        reaction: &tinychannels_bus::channel::ChannelReaction,
        remove: bool,
    ) -> Result<(), crate::channel::ChannelSendError> {
        if channel.id != "telegram" {
            return Err(crate::channel::ChannelSendError {
                kind: crate::channel::SendErrorKind::Forbidden,
                message: "reaction channel does not match Telegram".into(),
                retryable: false,
                ..Default::default()
            });
        }
        self.send_reaction_v2(conversation, provider_message_id, reaction, remove)
            .await
            .map_err(|error| typed_interaction_error(error, "reaction"))
    }
}

#[async_trait]
impl crate::channel::ChannelEdit for TelegramChannel {
    async fn edit(
        &self,
        receipt: &tinychannels_bus::channel::MessageReceipt,
        intent: tinychannels_bus::ChannelOutboundIntent,
    ) -> Result<tinychannels_bus::channel::MessageReceipt, crate::channel::ChannelSendError> {
        self.edit_message_v2(receipt, &intent)
            .await
            .map_err(|error| typed_interaction_error(error, "edit"))
    }
}

#[async_trait]
impl crate::channel::ChannelDelete for TelegramChannel {
    async fn delete(
        &self,
        receipt: &tinychannels_bus::channel::MessageReceipt,
    ) -> Result<(), crate::channel::ChannelSendError> {
        self.delete_message_v2(receipt)
            .await
            .map_err(|error| typed_interaction_error(error, "delete"))
    }
}

#[async_trait]
impl crate::channel::ChannelTyping for TelegramChannel {
    async fn start_typing(
        &self,
        conversation_id: &str,
    ) -> Result<(), crate::channel::ChannelSendError> {
        match self.send_typing_action_once(conversation_id).await {
            Ok(true) => Ok(()),
            Ok(false) => Err(crate::channel::ChannelSendError {
                kind: crate::channel::SendErrorKind::Forbidden,
                message: "Telegram rejected the typing action".into(),
                retryable: false,
                sent_before_error: false,
                ..Default::default()
            }),
            Err(error) => Err(typed_interaction_error(error, "typing")),
        }
    }

    async fn stop_typing(
        &self,
        _conversation_id: &str,
    ) -> Result<(), crate::channel::ChannelSendError> {
        // Bot API chat actions expire after five seconds and have no stop call.
        Ok(())
    }
}

#[async_trait]
impl crate::channel::ChannelStreamingDraft for TelegramChannel {
    async fn start_draft(
        &self,
        intent: tinychannels_bus::ChannelOutboundIntent,
    ) -> Result<Option<String>, crate::channel::ChannelSendError> {
        let text = telegram_draft_text(&intent)?;
        let conversation = telegram_draft_conversation(&intent)?;
        let mut message = SendMessage::new(text, telegram_draft_recipient(&conversation));
        // Typed thread scope routes to the topic; only reply_to_id identifies
        // a parent provider message.
        message.thread_ts = intent.reply_to_id.clone();
        let Some(message_id) = self
            .send_draft_provider(&message)
            .await
            .map_err(|error| typed_interaction_error(error, "draft creation"))?
        else {
            return Ok(None);
        };
        Ok(Some(message_id))
    }

    async fn update_draft(
        &self,
        draft_id: &str,
        intent: tinychannels_bus::ChannelOutboundIntent,
    ) -> Result<(), crate::channel::ChannelSendError> {
        let text = telegram_draft_text(&intent)?;
        let conversation = telegram_draft_conversation(&intent)?;
        self.update_draft_provider(&telegram_draft_recipient(&conversation), draft_id, &text)
            .await
            .map_err(|error| typed_interaction_error(error, "draft update"))
    }

    async fn finalize_draft(
        &self,
        draft_id: &str,
        intent: tinychannels_bus::ChannelOutboundIntent,
    ) -> Result<(), crate::channel::ChannelSendError> {
        let text = telegram_draft_text(&intent)?;
        let conversation = telegram_draft_conversation(&intent)?;
        self.finalize_draft_provider(
            &telegram_draft_recipient(&conversation),
            draft_id,
            &text,
            intent.reply_to_id.as_deref(),
        )
        .await
        .map_err(|error| typed_interaction_error(error, "draft finalization"))
    }

    async fn upsert_draft(
        &self,
        previous: Option<&tinychannels_bus::channel::MessageReceipt>,
        intent: tinychannels_bus::ChannelOutboundIntent,
    ) -> Result<tinychannels_bus::channel::MessageReceipt, crate::channel::ChannelSendError> {
        use tinychannels_bus::OutboundPayload;
        let text = match &intent.payload {
            OutboundPayload::Text { text } => text.clone(),
            OutboundPayload::Parts { parts } => {
                let mut texts = Vec::with_capacity(parts.len());
                for part in parts {
                    match part {
                        tinychannels_bus::OutboundContentPart::Content {
                            part: tinychannels_bus::ContentPart::Text { text, .. },
                        } => texts.push(text.as_str()),
                        _ => {
                            return Err(crate::channel::ChannelSendError {
                                kind: crate::channel::SendErrorKind::BadFormat,
                                message: "Telegram draft payload must contain only text".into(),
                                unsupported: true,
                                retryable: false,
                                ..Default::default()
                            });
                        }
                    }
                }
                if texts.is_empty() {
                    return Err(crate::channel::ChannelSendError {
                        kind: crate::channel::SendErrorKind::BadFormat,
                        message: "Telegram draft requires text".into(),
                        retryable: false,
                        ..Default::default()
                    });
                }
                texts.join("\n")
            }
            _ => {
                return Err(crate::channel::ChannelSendError {
                    kind: crate::channel::SendErrorKind::BadFormat,
                    message: "Telegram draft payload must be text".into(),
                    unsupported: true,
                    retryable: false,
                    ..Default::default()
                });
            }
        };
        let conversation =
            intent
                .conversation_ref
                .clone()
                .unwrap_or_else(|| tinychannels_bus::ConversationRef {
                    id: intent.conversation_id.clone(),
                    thread_id: intent.thread_id.clone(),
                    ..Default::default()
                });
        if conversation.id != intent.conversation_id {
            return Err(crate::channel::ChannelSendError {
                kind: crate::channel::SendErrorKind::Forbidden,
                message: "Telegram draft conversation scope mismatch".into(),
                retryable: false,
                ..Default::default()
            });
        }
        let rendered = markdown_v2_text(&strip_tool_call_tags(&text));
        let (chat_id, encoded_thread) = Self::parse_reply_target(&conversation.id);
        let thread = conversation
            .topic_id
            .clone()
            .or(conversation.thread_id.clone())
            .or(encoded_thread);
        let message_id = if let Some(previous) = previous {
            let expected_channel = tinychannels_bus::ChannelRef {
                id: "telegram".into(),
                account_id: previous.channel.as_ref().and_then(|c| c.account_id.clone()),
            };
            if previous.channel.as_ref().map(|c| c.id.as_str()) != Some("telegram")
                || previous.conversation.as_ref() != Some(&conversation)
                || previous.parts.iter().any(|part| part.thread_id != thread)
            {
                return Err(crate::channel::ChannelSendError {
                    kind: crate::channel::SendErrorKind::Forbidden,
                    message: "Telegram draft receipt scope mismatch".into(),
                    retryable: false,
                    ..Default::default()
                });
            }
            let _ = expected_channel;
            let id = previous
                .primary_platform_message_id
                .as_deref()
                .or_else(|| {
                    previous
                        .parts
                        .first()
                        .map(|p| p.platform_message_id.as_str())
                })
                .ok_or_else(|| crate::channel::ChannelSendError {
                    kind: crate::channel::SendErrorKind::BadFormat,
                    message: "Telegram draft receipt has no provider message ID".into(),
                    retryable: false,
                    ..Default::default()
                })?;
            let parsed = id
                .parse::<i64>()
                .map_err(|_| crate::channel::ChannelSendError {
                    kind: crate::channel::SendErrorKind::BadFormat,
                    message: "Telegram draft receipt has an invalid provider message ID".into(),
                    retryable: false,
                    ..Default::default()
                })?;
            let response = self
                .http_client()
                .post(self.api_url("editMessageText"))
                .json(&serde_json::json!({
                    "chat_id": chat_id,
                    "message_id": parsed,
                    "text": rendered,
                    "parse_mode": "MarkdownV2"
                }))
                .send()
                .await
                .map_err(|_error| crate::channel::ChannelSendError {
                    kind: crate::channel::SendErrorKind::Transient,
                    message: "Telegram draft edit transport failed".into(),
                    retryable: false,
                    sent_before_error: true,
                    ..Default::default()
                })?;
            let body = response
                .json::<serde_json::Value>()
                .await
                .map_err(|_error| crate::channel::ChannelSendError {
                    kind: crate::channel::SendErrorKind::Transient,
                    message: "Telegram draft edit response failed".into(),
                    retryable: false,
                    sent_before_error: true,
                    ..Default::default()
                })?;
            if !body
                .get("ok")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
            {
                return Err(crate::channel::ChannelSendError {
                    kind: crate::channel::SendErrorKind::Unknown,
                    message: "Telegram draft edit was rejected or uncertain".into(),
                    retryable: false,
                    sent_before_error: true,
                    ..Default::default()
                });
            }
            return Ok(previous.clone());
        } else {
            let mut body = serde_json::json!({ "chat_id": chat_id, "text": rendered, "parse_mode": "MarkdownV2" });
            if let Some(thread) = thread.as_deref() {
                body["message_thread_id"] = serde_json::Value::String(thread.to_owned());
            }
            if let Some(reply) = intent.reply_to_id.as_deref() {
                let parsed =
                    reply
                        .parse::<i64>()
                        .map_err(|_| crate::channel::ChannelSendError {
                            kind: crate::channel::SendErrorKind::BadFormat,
                            message: "Telegram reply ID must be numeric".into(),
                            retryable: false,
                            ..Default::default()
                        })?;
                body["reply_parameters"] = serde_json::json!({ "message_id": parsed });
            }
            let response = self
                .http_client()
                .post(self.api_url("sendMessage"))
                .json(&body)
                .send()
                .await
                .map_err(|_error| crate::channel::ChannelSendError {
                    kind: crate::channel::SendErrorKind::Transient,
                    message: "Telegram draft creation transport failed".into(),
                    retryable: false,
                    sent_before_error: true,
                    ..Default::default()
                })?;
            let body = response
                .json::<serde_json::Value>()
                .await
                .map_err(|_error| crate::channel::ChannelSendError {
                    kind: crate::channel::SendErrorKind::Transient,
                    message: "Telegram draft creation response failed".into(),
                    retryable: false,
                    sent_before_error: true,
                    ..Default::default()
                })?;
            if !body
                .get("ok")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
            {
                return Err(crate::channel::ChannelSendError {
                    kind: crate::channel::SendErrorKind::Unknown,
                    message: "Telegram draft creation failed".into(),
                    retryable: false,
                    sent_before_error: true,
                    ..Default::default()
                });
            }
            body.get("result")
                .and_then(|m| m.get("message_id"))
                .and_then(serde_json::Value::as_i64)
                .map(|id| id.to_string())
                .ok_or_else(|| crate::channel::ChannelSendError {
                    kind: crate::channel::SendErrorKind::Unknown,
                    message: "Telegram draft response omitted message ID".into(),
                    retryable: false,
                    sent_before_error: true,
                    ..Default::default()
                })?
        };
        let part = tinychannels_bus::channel::MessageReceiptPart {
            platform_message_id: message_id.clone(),
            kind: tinychannels_bus::channel::MessageReceiptPartKind::Text,
            thread_id: thread.clone(),
            reply_to_id: intent.reply_to_id.clone(),
            edit_token: Some(message_id.clone()),
            delete_token: Some(message_id.clone()),
            ..Default::default()
        };
        Ok(tinychannels_bus::channel::MessageReceipt {
            idempotency_key: Some(intent.idempotency_key),
            channel: intent.channel_ref.or_else(|| {
                Some(tinychannels_bus::ChannelRef {
                    id: "telegram".into(),
                    account_id: None,
                })
            }),
            conversation: Some(conversation),
            primary_platform_message_id: Some(message_id.clone()),
            platform_message_ids: vec![message_id],
            parts: vec![part],
            thread_id: thread.clone(),
            reply_to_id: intent.reply_to_id,
            ..Default::default()
        })
    }
}

fn telegram_draft_text(
    intent: &tinychannels_bus::ChannelOutboundIntent,
) -> Result<String, crate::channel::ChannelSendError> {
    match &intent.payload {
        tinychannels_bus::OutboundPayload::Text { text } => Ok(text.clone()),
        _ => Err(crate::channel::ChannelSendError {
            kind: crate::channel::SendErrorKind::BadFormat,
            message: "Telegram draft payload must be text".into(),
            unsupported: true,
            retryable: false,
            ..Default::default()
        }),
    }
}

fn telegram_draft_conversation(
    intent: &tinychannels_bus::ChannelOutboundIntent,
) -> Result<tinychannels_bus::ConversationRef, crate::channel::ChannelSendError> {
    let conversation =
        intent
            .conversation_ref
            .clone()
            .unwrap_or_else(|| tinychannels_bus::ConversationRef {
                id: intent.conversation_id.clone(),
                thread_id: intent.thread_id.clone(),
                ..Default::default()
            });
    if conversation.id.trim().is_empty()
        || conversation.id != intent.conversation_id
        || intent
            .channel_ref
            .as_ref()
            .is_some_and(|channel| channel.id != "telegram")
    {
        return Err(crate::channel::ChannelSendError {
            kind: crate::channel::SendErrorKind::Forbidden,
            message: "Telegram draft conversation scope mismatch".into(),
            retryable: false,
            ..Default::default()
        });
    }
    Ok(conversation)
}

fn telegram_draft_recipient(conversation: &tinychannels_bus::ConversationRef) -> String {
    let (chat_id, encoded_thread) = TelegramChannel::parse_reply_target(&conversation.id);
    conversation
        .topic_id
        .as_ref()
        .or(conversation.thread_id.as_ref())
        .or(encoded_thread.as_ref())
        .map_or_else(
            || conversation.id.clone(),
            |thread| format!("{chat_id}:{thread}"),
        )
}

#[cfg(test)]
#[path = "channel_ops_tests.rs"]
mod tests;

#[async_trait]
impl crate::channel::ChannelTypedSend for TelegramChannel {
    async fn send_typed(
        &self,
        message: &tinychannels_bus::ChannelOutboundMessage,
    ) -> Result<tinychannels_bus::channel::ChannelSendOutcome, crate::channel::ChannelSendError>
    {
        self.send_v2_with_receipt(message)
            .await
            .map_err(typed_send_error)
    }
}
