//! Channel facade for the Yuanbao provider.
//!
//! This module owns the OpenHuman [`Channel`] implementation and keeps
//! provider wiring out of `mod.rs`. Protocol decoding, transport, inbound
//! filtering, and outbound sending remain delegated to sibling modules.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{Mutex as TokioMutex, mpsc, watch};
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::traits::{Channel, ChannelMessage, SendMessage};

use super::config::YuanbaoConfig;
use super::connection::{InboundEvent, YuanbaoConnection};
use super::ids::{shorten_account_id, shorten_reply_target};
use super::inbound::{InboundPipeline, PipelineOutcome, PipelineState};
use super::outbound::OutboundSender;
use super::proto::decode_push_msg;
use super::sign::SignManager;
use super::{splitter, types};

/// Reply Heartbeat keepalive interval. The yuanbao gateway expects the
/// bot to ping (`SendPrivateHeartbeat RUNNING`) at this cadence so the
/// "正在输入" indicator stays alive for long-running responses.
const REPLY_HEARTBEAT_INTERVAL_SECS: u64 = 2;

/// Hard ceiling on the in-memory shortened-recipient → original-recipient
/// map. Each entry is two short strings (~80 B), so 4096 distinct senders
/// give ~320 KB — plenty for any realistic chat load and small enough
/// that we can blow the whole map away when we hit the cap instead of
/// dragging in an LRU dependency. See `register_recipient_alias`.
const RECIPIENT_ALIAS_CAP: usize = 4096;

/// The yuanbao channel — owns one WebSocket and one inbound pipeline.
pub struct YuanbaoChannel {
    config: YuanbaoConfig,
    connection: Arc<YuanbaoConnection>,
    outbound: Arc<OutboundSender>,
    pipeline: Arc<InboundPipeline>,
    shutdown_tx: watch::Sender<bool>,
    /// Holds the inbound receiver between `new()` and the first `listen()` call.
    ///
    /// `Channel::listen` takes `&self`, so we can't move the receiver out of
    /// a field. Use a `Mutex<Option<…>>` so the first listener takes ownership
    /// and subsequent calls fail cleanly.
    inbound_rx: parking_lot::Mutex<Option<mpsc::UnboundedReceiver<InboundEvent>>>,
    /// Per-recipient Reply Heartbeat keepalive tasks (started on `start_typing`).
    heartbeat_tasks: TokioMutex<HashMap<String, JoinHandle<()>>>,
    /// Reverse lookup table from shortened recipient ids (the ones we
    /// emit on `ChannelMessage.sender` / `reply_target`) back to the
    /// original server-recognized ids that outbound `send_c2c_message`
    /// / `send_group_message` must use as `to_account` / `group_code`.
    ///
    /// Why this exists: yuanbao uids are ~64-char hashes, and
    /// `super::ids::shorten_account_id` rewrites them as
    /// `<prefix>_<sha256-16hex>` so the conversation store's per-thread
    /// JSONL filenames stay under filesystem `NAME_MAX`. Without this
    /// table the agent loop sends replies addressed to the shortened
    /// hash, which the yuanbao gateway silently drops because no such
    /// user exists. See `register_recipient_alias` / `resolve_recipient`.
    recipient_aliases: TokioMutex<HashMap<String, String>>,
}

impl YuanbaoChannel {
    /// Build a channel from a validated config. Returns an error if the
    /// config is missing required fields (so misconfiguration surfaces
    /// at startup, not on the first inbound message).
    pub fn new(mut config: YuanbaoConfig) -> anyhow::Result<Self> {
        config.apply_env_defaults();
        config.validate().map_err(anyhow::Error::msg)?;
        let (shutdown_tx, _shutdown_rx) = watch::channel(false);
        let (inbound_tx, inbound_rx) = mpsc::unbounded_channel::<InboundEvent>();

        // SignManager is only useful when we have an app_secret — without
        // it we'd never call the sign endpoint anyway.
        let sign_manager: Option<Arc<SignManager>> = if !config.app_secret.is_empty() {
            Some(SignManager::new(reqwest::Client::new()))
        } else {
            None
        };

        let connection = YuanbaoConnection::new(config.clone(), inbound_tx, sign_manager.clone());
        let outbound = Arc::new(OutboundSender::new(
            Arc::clone(&connection),
            sign_manager.clone(),
            config.app_key.clone(),
            config.bot_id.clone(),
        ));
        // PipelineState's `from_account` is used by the echo-guard stage to
        // drop self-sent messages. We feed it the static config value here
        // (which may be empty); the canonical server-issued bot_id only
        // becomes known after sign-token, so this is a known minor gap —
        // echo guard will simply not fire when bot_id isn't statically set.
        let pipeline_state = PipelineState::new(&config, config.bot_id.clone());
        let pipeline = Arc::new(InboundPipeline::new(pipeline_state));

        Ok(Self {
            config,
            connection,
            outbound,
            pipeline,
            shutdown_tx,
            inbound_rx: parking_lot::Mutex::new(Some(inbound_rx)),
            heartbeat_tasks: TokioMutex::new(HashMap::new()),
            recipient_aliases: TokioMutex::new(HashMap::new()),
        })
    }

    /// Record a `shortened → original` recipient mapping so the outbound
    /// side can recover the server-recognized id when the agent loop
    /// addresses a reply with the shortened sender / reply_target it
    /// received on `ChannelMessage`.
    ///
    /// No-op when the two are equal (uid is short enough to skip
    /// shortening, or this is the `g:` group-target case where the
    /// inner code is short). When the map crosses `RECIPIENT_ALIAS_CAP`
    /// we clear it — the next inbound message from each active sender
    /// re-populates the entry it needs, and stale entries from idle
    /// conversations are fine to lose.
    async fn register_recipient_alias(&self, shortened: &str, original: &str) {
        if shortened == original {
            return;
        }
        let mut m = self.recipient_aliases.lock().await;
        if m.len() >= RECIPIENT_ALIAS_CAP {
            warn!(
                "[yuanbao] recipient alias map hit cap ({}), clearing",
                RECIPIENT_ALIAS_CAP
            );
            m.clear();
        }
        m.insert(shortened.to_string(), original.to_string());
    }

    /// Look up the server-recognized recipient for a (possibly
    /// shortened) inbound id. Falls back to the input unchanged when
    /// nothing is registered — which keeps the previous behavior for
    /// recipients that don't go through `shorten_account_id` (short
    /// uids, group codes, `imessage`-style ids).
    async fn resolve_recipient(&self, recipient: &str) -> String {
        let m = self.recipient_aliases.lock().await;
        m.get(recipient)
            .cloned()
            .unwrap_or_else(|| recipient.to_string())
    }

    fn split_message(&self, text: &str) -> Vec<String> {
        splitter::split_markdown(text, self.config.max_message_length)
    }

    async fn start_heartbeat_task(&self, recipient: &str) {
        let mut tasks = self.heartbeat_tasks.lock().await;
        if tasks.contains_key(recipient) {
            return;
        }
        let outbound = Arc::clone(&self.outbound);
        let target = recipient.to_string();
        let handle = tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(Duration::from_secs(REPLY_HEARTBEAT_INTERVAL_SECS));
            interval.tick().await; // skip first tick (start_typing already sent RUNNING)
            loop {
                interval.tick().await;
                if let Err(e) = outbound.start_heartbeat(&target).await {
                    // Connection bouncing — bail out of this loop; the
                    // next start_typing call will spawn a new one.
                    warn!(
                        "[yuanbao] reply heartbeat send failed: {} — stopping loop",
                        e
                    );
                    return;
                }
            }
        });
        tasks.insert(recipient.to_string(), handle);
    }

    async fn stop_heartbeat_task(&self, recipient: &str) {
        let mut tasks = self.heartbeat_tasks.lock().await;
        if let Some(handle) = tasks.remove(recipient) {
            handle.abort();
        }
    }
}

#[async_trait]
impl Channel for YuanbaoChannel {
    fn name(&self) -> &str {
        "yuanbao"
    }

    async fn send(&self, message: &SendMessage) -> anyhow::Result<()> {
        let chunks = self.split_message(&message.content);
        let ref_msg_id = message.thread_ts.as_deref();
        let recipient = self.resolve_recipient(&message.recipient).await;
        for chunk in &chunks {
            self.outbound
                .send_text(&recipient, chunk, ref_msg_id)
                .await?;
        }
        Ok(())
    }

    fn supports_draft_updates(&self) -> bool {
        // Routes turns through the streaming code path even though Yuanbao
        // itself has no edit-message capability. We accept the UX cost (no
        // progressive rendering — the reply appears all at once in
        // `finalize_draft`) in exchange for streaming's tolerance of
        // malformed `usage` chunks; the non-streaming parser fails the
        // whole turn when an upstream LLM returns string-typed token counts.
        true
    }

    async fn send_draft(&self, message: &SendMessage) -> anyhow::Result<Option<String>> {
        // Marker id so dispatch spins up the progress consumer task;
        // nothing is sent to the user here. Real content goes out in
        // `finalize_draft`. See `supports_draft_updates` for rationale.
        Ok(Some(format!("yb-draft:{}", message.recipient)))
    }

    async fn update_draft(
        &self,
        _recipient: &str,
        _message_id: &str,
        _text: &str,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn finalize_draft(
        &self,
        recipient: &str,
        _message_id: &str,
        text: &str,
        thread_ts: Option<&str>,
    ) -> anyhow::Result<()> {
        let chunks = self.split_message(text);
        let recipient = self.resolve_recipient(recipient).await;
        for chunk in &chunks {
            self.outbound
                .send_text(&recipient, chunk, thread_ts)
                .await?;
        }
        Ok(())
    }

    async fn listen(&self, tx: mpsc::Sender<ChannelMessage>) -> anyhow::Result<()> {
        // Take the inbound receiver. A second listener would just exit early.
        let mut inbound_rx = match self.inbound_rx.lock().take() {
            Some(rx) => rx,
            None => {
                warn!("[yuanbao] listen() called twice — second call exits");
                return Ok(());
            }
        };

        let conn = Arc::clone(&self.connection);
        let shutdown_rx = self.shutdown_tx.subscribe();
        let mut conn_task = tokio::spawn(async move {
            conn.run(shutdown_rx).await;
        });

        info!("[yuanbao] channel listening — pipeline ready");
        let mut shutdown_rx2 = self.shutdown_tx.subscribe();
        loop {
            tokio::select! {
                _ = shutdown_rx2.changed() => {
                    info!("[yuanbao] listen loop received shutdown");
                    break;
                }
                event = inbound_rx.recv() => {
                    match event {
                        Some(InboundEvent::Push(frame)) => {
                            self.dispatch_push(frame, &tx).await;
                        }
                        Some(InboundEvent::Kickout(reason)) => {
                            warn!("[yuanbao] kickout: {} — stopping listen loop", reason);
                            break;
                        }
                        None => {
                            warn!("[yuanbao] inbound channel closed");
                            break;
                        }
                    }
                }
            }
        }

        let _ = self.shutdown_tx.send(true);
        // Give the connection task a brief window to run its own shutdown cleanup
        // (flush pending, update is_connected, etc.) before force-aborting.
        match tokio::time::timeout(std::time::Duration::from_secs(2), &mut conn_task).await {
            Ok(_) => {}
            Err(_) => conn_task.abort(),
        }
        Ok(())
    }

    async fn health_check(&self) -> bool {
        self.connection.is_connected()
    }

    async fn start_typing(&self, recipient: &str) -> anyhow::Result<()> {
        // Send RUNNING immediately, then spawn a 2s keepalive so the
        // indicator doesn't expire while we generate.
        let recipient = self.resolve_recipient(recipient).await;
        self.outbound.start_heartbeat(&recipient).await?;
        self.start_heartbeat_task(&recipient).await;
        Ok(())
    }

    async fn stop_typing(&self, recipient: &str) -> anyhow::Result<()> {
        let recipient = self.resolve_recipient(recipient).await;
        self.stop_heartbeat_task(&recipient).await;
        self.outbound.stop_heartbeat(&recipient).await?;
        Ok(())
    }

    fn supports_reactions(&self) -> bool {
        false
    }
}

impl YuanbaoChannel {
    async fn dispatch_push(&self, frame: types::ConnFrame, tx: &mpsc::Sender<ChannelMessage>) {
        // The Yuanbao gateway pushes inbound messages with `cmd_type=Push`;
        // the actual `cmd` word is decided server-side and varies (mirrors
        // hermes-agent `yuanbao.py::_handle_received_frame` which routes
        // purely on cmd_type). The connection layer has already filtered
        // out non-push frames before we get here, so every frame we see
        // should be a candidate for the inbound pipeline.
        if frame.data.is_empty() {
            tracing::trace!("[yuanbao] empty push body cmd={} — skipping", frame.cmd);
            return;
        }
        // Some push frames wrap the biz body in an extra
        // `PushMsg { cmd, module, msg_id, data }` envelope; others (e.g.
        // cmd="inbound_message", module="yuanbao_openclaw_proxy") put the
        // InboundMessagePush bytes directly in `ConnMsg.data` with the
        // ConnMsg.head already carrying cmd/module. Mirrors plugin
        // client.ts::onPush (l. 813): try PushMsg first, but only accept
        // it when it has a non-empty cmd or module; otherwise treat the
        // raw frame.data as the biz body.
        let unwrapped: Option<Vec<u8>> = match decode_push_msg(&frame.data) {
            Ok(p) if (!p.cmd.is_empty() || !p.module.is_empty()) && !p.data.is_empty() => {
                info!(
                    "[yuanbao] push envelope decoded: cmd={} module={} msg_id={} biz_len={}",
                    p.cmd,
                    p.module,
                    p.msg_id,
                    p.data.len()
                );
                Some(p.data)
            }
            _ => {
                info!(
                    "[yuanbao] push has no PushMsg envelope — treating ConnMsg.data as biz body (conn_cmd={} module={} len={})",
                    frame.cmd,
                    frame.module,
                    frame.data.len()
                );
                None
            }
        };
        let biz_body: &[u8] = unwrapped.as_deref().unwrap_or(&frame.data);
        let outcome = self.pipeline.process(biz_body).await;
        match outcome {
            PipelineOutcome::Dispatch(ctx) => {
                // Shorten ids at the channel boundary so the composite thread_id
                // derived downstream (channel:yuanbao_<sender>_<reply_target>)
                // stays under filesystem NAME_MAX once hex-encoded for the
                // per-thread JSONL filename. Yuanbao internals (echo guard,
                // access control, owner-command check) keep the original
                // `from_account` — see `super::ids` for the format and rationale.
                let original_from = ctx.msg.from_account.clone();
                let original_reply_target = ctx.source.reply_target();
                let short_sender = shorten_account_id(&original_from);
                let short_reply_target = shorten_reply_target(&original_reply_target);
                // Remember the original ids so the outbound side can
                // recover them when the agent loop addresses a reply
                // with the shortened values it sees here.
                self.register_recipient_alias(&short_sender, &original_from)
                    .await;
                self.register_recipient_alias(&short_reply_target, &original_reply_target)
                    .await;
                let msg = ChannelMessage {
                    id: ctx.msg.msg_id.clone(),
                    sender: short_sender,
                    reply_target: short_reply_target,
                    content: if ctx.text.is_empty() && !ctx.image_urls.is_empty() {
                        // Surface image URLs as content so downstream tools have something to work with.
                        ctx.image_urls.join("\n")
                    } else {
                        ctx.text.clone()
                    },
                    channel: "yuanbao".into(),
                    timestamp: ctx.msg.msg_time as u64,
                    thread_ts: None,
                    sender_name: None,
                };
                if tx.send(msg).await.is_err() {
                    warn!("[yuanbao] dispatch receiver gone — dropping message");
                }
            }
            PipelineOutcome::Filtered(reason) => {
                tracing::trace!("[yuanbao] filtered at {reason}");
            }
            PipelineOutcome::Failed(err) => {
                // Intentionally omit the raw biz payload — it can carry
                // user content / PII. The decoder error already encodes
                // the structural reason; only the length is safe to log.
                warn!(
                    "[yuanbao] pipeline error: {err} | biz_len={}",
                    biz_body.len()
                );
            }
        }
    }
}

impl Drop for YuanbaoChannel {
    fn drop(&mut self) {
        let _ = self.shutdown_tx.send(true);
    }
}

#[cfg(test)]
#[path = "channel_tests.rs"]
mod tests;
