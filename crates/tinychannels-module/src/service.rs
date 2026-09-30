//! The TinyBus service boundary for the channel surface.
//!
//! One object, `/ai/tinyhumans/tinychannels/Channels`, exporting five methods:
//!
//! ```text
//! StartChannel(name, ChannelsConfig) -> ()
//! StopChannel(name)                  -> ()
//! SendMessage(name, SendMessage)     -> ()
//! ListChannels()                     -> Vec<String>
//! ChannelStatus(name)                -> ChannelState
//! ```
//!
//! # This module is stateful, unlike the document one
//!
//! `tinydocs` holds nothing between calls: a document is generated and handed
//! back. A channel provider is the opposite — it owns a live connection and a
//! listen loop that outlives every call. So this object keeps a registry of
//! running providers, and `StopChannel` is not optional politeness: without it
//! a provider's task and socket leak until the process exits.
//!
//! # Config arrives per call, not at load
//!
//! `StartChannel` takes the whole [`ChannelsConfig`] rather than reading one
//! handed over at load time, because a desktop host changes it at runtime — a
//! user pastes a bot token and expects the channel to come up. Reading a
//! load-time config would mean a module restart per edit.
//!
//! Credentials in it are expected to be **already hydrated** by the host; see
//! `tinychannels::factory`. This module has no keyring and cannot resolve a
//! placeholder.

use std::collections::HashMap;
use std::sync::Arc;

use tinybus::{Connection, Error as BusError, Result as BusResult};
use tinychannels::factory::{DefaultHttpClients, build_channels};
use tinychannels::host::ChannelHost;
use tinychannels::{Channel, NoopHost};
use tinychannels_bus::{BUS_NAME, ChannelsConfig, OBJECT_PATH, SendMessage};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use crate::host::HostChannels;

const UNKNOWN_CHANNEL_ERROR: &str = "ai.tinyhumans.tinychannels.Error.UnknownChannel";
const ALREADY_RUNNING_ERROR: &str = "ai.tinyhumans.tinychannels.Error.AlreadyRunning";
const NOT_CONFIGURED_ERROR: &str = "ai.tinyhumans.tinychannels.Error.NotConfigured";
const SEND_FAILED_ERROR: &str = "ai.tinyhumans.tinychannels.Error.SendFailed";
const NO_INGRESS_ERROR: &str = "ai.tinyhumans.tinychannels.Error.NoWebhookIngress";

/// Whether `name`, as configured, receives inbound traffic through a webhook
/// this module cannot serve. See the refusal in `start_channel` for why.
fn is_webhook_backed(name: &str, config: &ChannelsConfig) -> bool {
    name == "linq"
        || (name == "whatsapp"
            && config
                .whatsapp
                .as_ref()
                .is_some_and(|wa| wa.backend_type() == "cloud"))
}

/// Bound on the inbound queue between a provider and the forwarding task.
///
/// A provider that outruns the host is dropping messages either way; a bounded
/// channel makes that visible at a known point instead of growing until the
/// process is killed.
const INBOUND_QUEUE: usize = 256;

/// How long a stopping provider's forwarder gets to hand over what it already
/// accepted before it is abandoned.
const FORWARDER_DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// One running provider.
struct Running {
    channel: Arc<dyn Channel>,
    /// The provider's `listen` loop.
    listener: JoinHandle<()>,
    /// Drains the inbound queue into the host callback.
    forwarder: JoinHandle<()>,
}

impl Running {
    /// Stop both tasks. Used by `StopChannel`, from outside either of them.
    fn shutdown(self) {
        self.listener.abort();
        self.forwarder.abort();
    }

    /// Let the forwarder finish, then stop it if it will not.
    ///
    /// Used by the listener when it ends. **Not** an abort: `listen` owns the
    /// sender, so its return closes the channel and the forwarder drains what is
    /// left and exits on its own. Aborting instead would discard inbound
    /// messages the provider had already accepted — exactly the traffic a host
    /// is least willing to lose, and most likely to be holding when the host
    /// callback is the slow part.
    ///
    /// The timeout is the backstop for the case where the host callback is
    /// wedged: a drain that cannot finish must not keep the task alive for ever.
    async fn drain_forwarder(self) {
        if tokio::time::timeout(FORWARDER_DRAIN_TIMEOUT, self.forwarder)
            .await
            .is_err()
        {
            tracing::warn!(
                "[tinychannels:module] forwarder did not drain within {:?}; \
                 remaining inbound messages are dropped",
                FORWARDER_DRAIN_TIMEOUT
            );
        }
    }
}

/// The served object.
struct Channels {
    host: HostChannels,
    /// Shared with each listener task so it can retire its own entry.
    running: Arc<Mutex<HashMap<String, Running>>>,
}

#[allow(
    clippy::unused_async,
    reason = "tinybus::interface requires every method to be `async fn`"
)]
#[tinybus::interface(name = "ai.tinyhumans.tinychannels.Channels")]
impl Channels {
    /// Connect `name` from `config` and start receiving.
    async fn start_channel(&self, name: String, config: ChannelsConfig) -> BusResult<()> {
        let mut running = self.running.lock().await;
        if running.contains_key(&name) {
            return Err(BusError::MethodFailed {
                name: ALREADY_RUNNING_ERROR.to_owned(),
                message: format!("channel {name} is already running"),
            });
        }

        // The factory builds every *configured* provider; we take the one asked
        // for. Building the set is cheap (no I/O) and keeps a single
        // config-to-provider mapping rather than a second one here that could
        // disagree about, say, which WhatsApp backend a stanza describes.
        let noop: Arc<dyn ChannelHost> = NoopHost::arc();
        let built = build_channels(&config, &noop, &DefaultHttpClients);
        let channel = built
            .into_iter()
            .find(|candidate| candidate.name() == name)
            .ok_or_else(|| BusError::MethodFailed {
                name: NOT_CONFIGURED_ERROR.to_owned(),
                message: format!("channel {name} is not present in the supplied config"),
            })?;

        // Refuse the providers whose inbound path this module cannot serve.
        //
        // Linq and the WhatsApp *Cloud API* variant are push-based: their
        // `listen` implementations sleep forever, because messages arrive at an
        // HTTPS webhook the host operates. The module exports no route or bus
        // member that can feed such a payload into the channel, so starting one
        // here would report success and then deliver nothing, for ever — the
        // worst failure shape available.
        //
        // Failing loudly is deliberately preferred to a silent no-op. When
        // webhook ingress is added to the contract this check is what gets
        // relaxed. (WhatsApp *Web* is unaffected: it has a real listen loop.
        // Both variants report `name() == "whatsapp"`, so the config shape is
        // what distinguishes them, not the name.)
        if is_webhook_backed(&name, &config) {
            return Err(BusError::MethodFailed {
                name: NO_INGRESS_ERROR.to_owned(),
                message: format!(
                    "channel {name} receives inbound traffic through an external webhook, \
                     and this module has no ingress for it; run this provider in-process \
                     or serve its webhook host-side"
                ),
            });
        }

        let (tx, mut rx) = tokio::sync::mpsc::channel(INBOUND_QUEUE);

        let listen_channel = Arc::clone(&channel);
        let listen_name = name.clone();
        let listen_host = self.host.clone();
        let listen_registry = Arc::clone(&self.running);
        let listener = tokio::spawn(async move {
            listen_host
                .report_status(&listen_name, "connecting", None)
                .await;
            let outcome = listen_channel.listen(tx).await;

            // The listen loop returning is the provider's terminal state, so the
            // registry entry has to go with it. Leaving it behind was a real bug:
            // `ListChannels` kept advertising a dead provider, `StartChannel`
            // rejected every retry as `AlreadyRunning`, and `ChannelStatus` could
            // still answer "connected" off an HTTP health check that knows
            // nothing about the receive loop.
            //
            // Dropped rather than aborted: this task is the listener, and
            // `Running::shutdown` would abort it from inside itself.
            if let Some(entry) = listen_registry.lock().await.remove(&listen_name) {
                entry.drain_forwarder().await;
            }

            match outcome {
                Ok(()) => {
                    listen_host
                        .report_status(&listen_name, "stopped", None)
                        .await
                }
                Err(error) => {
                    // The module deliberately does not reconnect on its own: a
                    // backoff policy that disagreed with the host's would be
                    // invisible from the host side. Reporting is what lets the
                    // host decide to retry — and the entry is gone, so it can.
                    tracing::warn!("[tinychannels:module] {listen_name} listen ended: {error}");
                    listen_host
                        .report_status(&listen_name, "failed", Some(error.to_string()))
                        .await;
                }
            }
        });

        let forward_name = name.clone();
        let forward_host = self.host.clone();
        let forwarder = tokio::spawn(async move {
            while let Some(message) = rx.recv().await {
                match serde_json::to_value(&message) {
                    Ok(value) => forward_host.deliver_inbound(&forward_name, value).await,
                    Err(error) => tracing::warn!(
                        "[tinychannels:module] {forward_name} inbound message not serialisable: {error}"
                    ),
                }
            }
        });

        running.insert(
            name,
            Running {
                channel,
                listener,
                forwarder,
            },
        );
        Ok(())
    }

    /// Disconnect `name` and drop its tasks.
    ///
    /// Stopping a channel that is not running is **not** an error: a host
    /// reconciling desired against actual state should be able to call this
    /// unconditionally.
    async fn stop_channel(&self, name: String) -> BusResult<()> {
        // Taken out of the map before the report so the lock is not held across
        // the enqueue, which can block on a full outbox.
        let running = self.running.lock().await.remove(&name);
        if let Some(running) = running {
            running.shutdown();
            self.host.report_status(&name, "stopped", None).await;
        }
        Ok(())
    }

    /// Send one outbound message through a running provider.
    async fn send_message(&self, name: String, message: SendMessage) -> BusResult<()> {
        // Cloned out of the map so the send does not hold the registry lock for
        // the duration of a network round trip.
        let channel = {
            let running = self.running.lock().await;
            running
                .get(&name)
                .map(|entry| Arc::clone(&entry.channel))
                .ok_or_else(|| BusError::MethodFailed {
                    name: UNKNOWN_CHANNEL_ERROR.to_owned(),
                    message: format!("channel {name} is not running"),
                })?
        };

        channel
            .send(&message)
            .await
            .map_err(|error| BusError::MethodFailed {
                name: SEND_FAILED_ERROR.to_owned(),
                message: error.to_string(),
            })
    }

    /// The providers currently running, in no particular order.
    async fn list_channels(&self) -> BusResult<Vec<String>> {
        Ok(self.running.lock().await.keys().cloned().collect())
    }

    /// Whether `name` is running, and whether the provider reports itself healthy.
    async fn channel_status(&self, name: String) -> BusResult<String> {
        let channel = {
            let running = self.running.lock().await;
            running.get(&name).map(|entry| Arc::clone(&entry.channel))
        };
        let Some(channel) = channel else {
            return Ok("stopped".to_owned());
        };
        Ok(if channel.health_check().await {
            "connected".to_owned()
        } else {
            "unhealthy".to_owned()
        })
    }
}

async fn setup(connection: Connection) -> BusResult<()> {
    let channels = Channels {
        host: HostChannels::new(connection.clone()),
        running: Arc::new(Mutex::new(HashMap::new())),
    };
    connection
        .serve_at(OBJECT_PATH.try_into()?, channels)
        .await?;
    connection.request_name(BUS_NAME).await?;
    Ok(())
}

// Isolate the generated public C symbols so the lint exception cannot hide
// undocumented Rust API. Their contract is TinyBus ABI v1.
#[allow(
    missing_docs,
    unreachable_pub,
    reason = "generated C ABI symbols are documented by the TinyBus module SDK"
)]
pub(crate) mod exports {
    tinybus_module::module_export_optional_static! {
        setup = super::setup,
        worker_threads = 4,
        provides = ["ai.tinyhumans.tinychannels.Channels"],
        methods = [
            "StartChannel",
            "StopChannel",
            "SendMessage",
            "ListChannels",
            "ChannelStatus",
        ],
        signals = [],
        // The host callback object is resolved per call, not required at load:
        // an outbound-only host that serves nothing is supported.
        requires = [],
        optional = ["ai.tinyhumans.tinychannels.ChannelsHost"],
        lazy = false,
    }
}

#[cfg(test)]
mod tests {
    use tinychannels_bus::ChannelsConfig;
    use tinychannels_bus::config::WhatsAppConfig;

    /// Mirrors the predicate in `start_channel`.
    ///
    /// Extracted rather than duplicated in the test so the two cannot disagree:
    /// the point of these cases is the *classification*, and a copy of the rule
    /// would keep passing after the real one changed.
    use super::is_webhook_backed as webhook_backed;

    fn whatsapp(cloud: bool) -> ChannelsConfig {
        ChannelsConfig {
            whatsapp: Some(WhatsAppConfig {
                access_token: None,
                phone_number_id: cloud.then(|| "1".to_owned()),
                verify_token: None,
                app_secret: None,
                session_path: (!cloud).then(|| "/tmp/session".to_owned()),
                pair_phone: None,
                pair_code: None,
                allowed_numbers: Vec::new(),
            }),
            ..ChannelsConfig::default()
        }
    }

    #[test]
    fn linq_is_refused_because_its_inbound_path_is_a_webhook() {
        assert!(webhook_backed("linq", &ChannelsConfig::default()));
    }

    #[test]
    fn whatsapp_cloud_is_refused_but_whatsapp_web_is_not() {
        // Both report `name() == "whatsapp"`, so only the config shape separates
        // them. Getting this backwards would either refuse a working provider or
        // silently accept a dead one.
        assert!(webhook_backed("whatsapp", &whatsapp(true)));
        assert!(!webhook_backed("whatsapp", &whatsapp(false)));
    }

    #[test]
    fn providers_with_a_real_listen_loop_are_unaffected() {
        for name in ["telegram", "discord", "slack", "irc", "signal"] {
            assert!(
                !webhook_backed(name, &ChannelsConfig::default()),
                "{name} should not be refused"
            );
        }
    }
}
