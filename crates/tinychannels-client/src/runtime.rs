//! Reservation-scoped runtime driver and typed object operations.

use async_trait::async_trait;
use serde::de::DeserializeOwned;
use tinybus::{
    BusName, ObjectPath,
    module::{
        BoundModuleClient, ModuleObjectClient, RuntimeLeaseDriver, StartOutcome, StopOutcome,
    },
};
use tinychannels_bus::{
    ChannelInteractionRequest, ChannelInteractionResult, ChannelOutboundIntent,
    ChannelRuntimeDispatchPolicy, ChannelRuntimeReservation, ChannelSupervisionPolicy,
    ChannelsConfig, HttpProxyPolicy,
    controllers::ChannelSendMessageResult,
    methods,
    module_control::{ChannelControlRequest, ChannelControlResult},
    module_progressive::{ProgressiveCommand, ProgressiveResult},
    module_relay::RelayReservation,
};

use crate::{ChannelRuntimeError, ChannelRuntimeErrorKind, ChannelRuntimeOperation};

#[derive(Clone, Copy)]
pub(super) enum RuntimeStart {
    Serial,
    Dispatch(ChannelRuntimeDispatchPolicy),
}

/// Runtime protocol driver bound to one attested TinyChannels peer.
///
/// Its bound client stays private so callers cannot bypass the typed protocol
/// projection or resolve the module alias again.
#[derive(Clone)]
pub struct ChannelRuntimeDriver {
    bound: BoundModuleClient,
    start: RuntimeStart,
}

impl ChannelRuntimeDriver {
    pub(super) fn new(bound: BoundModuleClient, start: RuntimeStart) -> Self {
        Self { bound, start }
    }
}

#[async_trait]
impl RuntimeLeaseDriver for ChannelRuntimeDriver {
    type Reservation = ChannelRuntimeReservation;
    type Object = ChannelRuntimeClient;
    type Error = ChannelRuntimeError;

    async fn reserve(&self) -> Result<Self::Reservation, Self::Error> {
        self.bound
            .call_confidential(methods::RESERVE_RUNTIME, ())
            .await
            .map_err(|error| ChannelRuntimeError::module(ChannelRuntimeOperation::Reserve, error))
    }

    fn validate(&self, reservation: &Self::Reservation) -> Result<(), Self::Error> {
        reservation.validate().map_err(|_| {
            ChannelRuntimeError::contract(
                ChannelRuntimeOperation::ValidateReservation,
                ChannelRuntimeErrorKind::InvalidReservation,
            )
        })
    }

    fn callback_address(
        &self,
        reservation: &Self::Reservation,
        _object: &Self::Object,
    ) -> Result<(BusName, ObjectPath), Self::Error> {
        self.validate(reservation)?;
        let name = BusName::new(tinychannels_bus::HOST_BUS_NAME).map_err(|_| {
            ChannelRuntimeError::contract(
                ChannelRuntimeOperation::CallbackAddress,
                ChannelRuntimeErrorKind::InvalidCallbackAddress,
            )
        })?;
        let path = ObjectPath::new(&reservation.host_object_path).map_err(|_| {
            ChannelRuntimeError::contract(
                ChannelRuntimeOperation::CallbackAddress,
                ChannelRuntimeErrorKind::InvalidCallbackAddress,
            )
        })?;
        Ok((name, path))
    }

    async fn object(&self, reservation: &Self::Reservation) -> Result<Self::Object, Self::Error> {
        let object = self
            .bound
            .object_client(&reservation.object_path)
            .map_err(|error| ChannelRuntimeError::module(ChannelRuntimeOperation::Object, error))?;
        Ok(ChannelRuntimeClient { object })
    }

    async fn start(&self, reservation: &Self::Reservation) -> StartOutcome<Self::Error> {
        let result = match self.start {
            RuntimeStart::Serial => {
                self.bound
                    .call_confidential::<()>(methods::START_RUNTIME, (reservation.handle.clone(),))
                    .await
            }
            RuntimeStart::Dispatch(policy) => {
                self.bound
                    .call_confidential::<()>(
                        methods::START_RUNTIME_WITH_DISPATCH,
                        (reservation.handle.clone(), policy),
                    )
                    .await
            }
        };
        match result {
            Ok(()) => StartOutcome::Started,
            Err(error) => StartOutcome::Unacknowledged(ChannelRuntimeError::module(
                ChannelRuntimeOperation::Start,
                error,
            )),
        }
    }

    async fn stop(&self, reservation: &Self::Reservation) -> StopOutcome<Self::Error> {
        match self
            .bound
            .call_confidential::<()>(methods::STOP_RUNTIME, (reservation.handle.clone(),))
            .await
        {
            Ok(()) => StopOutcome::Acknowledged,
            Err(error) => StopOutcome::Unacknowledged(ChannelRuntimeError::module(
                ChannelRuntimeOperation::Stop,
                error,
            )),
        }
    }

    fn failed(&self, _reservation: &Self::Reservation) -> bool {
        // The current protocol has no authoritative terminal-fault observation.
        false
    }
}

/// Typed methods on one reservation-owned TinyChannels object.
///
/// The underlying SDK object client remains private. Each method preserves the
/// bus contract's argument order, response type, and delivery classification.
#[derive(Clone)]
pub struct ChannelRuntimeClient {
    object: ModuleObjectClient,
}

impl ChannelRuntimeClient {
    async fn confidential<R: DeserializeOwned>(
        &self,
        operation: ChannelRuntimeOperation,
        member: &str,
        args: impl serde::Serialize,
    ) -> Result<R, ChannelRuntimeError> {
        self.object
            .call_confidential(member, args)
            .await
            .map_err(|error| ChannelRuntimeError::module(operation, error))
    }

    async fn ordinary<R: DeserializeOwned>(
        &self,
        operation: ChannelRuntimeOperation,
        member: &str,
        args: impl serde::Serialize,
    ) -> Result<R, ChannelRuntimeError> {
        self.object
            .call(member, args)
            .await
            .map_err(|error| ChannelRuntimeError::module(operation, error))
    }

    /// Describe available providers without starting them.
    pub async fn describe_configured_channels(
        &self,
        config: ChannelsConfig,
        proxy: HttpProxyPolicy,
    ) -> Result<Vec<tinychannels_bus::module_runtime::ConfiguredChannelInfo>, ChannelRuntimeError>
    {
        self.confidential(
            ChannelRuntimeOperation::DescribeConfiguredChannels,
            methods::DESCRIBE_CONFIGURED_CHANNELS,
            (config, proxy),
        )
        .await
    }

    /// Start one supervised listener provider with host-selected retry timing.
    pub async fn start_channel_supervised(
        &self,
        name: String,
        config: ChannelsConfig,
        proxy: HttpProxyPolicy,
        policy: ChannelSupervisionPolicy,
    ) -> Result<(), ChannelRuntimeError> {
        self.confidential(
            ChannelRuntimeOperation::StartChannelSupervised,
            methods::START_CHANNEL_SUPERVISED,
            (name, config, proxy, policy),
        )
        .await
    }

    /// Start one external-webhook provider for outbound operations.
    pub async fn start_outbound_channel(
        &self,
        name: String,
        config: ChannelsConfig,
        proxy: HttpProxyPolicy,
    ) -> Result<(), ChannelRuntimeError> {
        self.confidential(
            ChannelRuntimeOperation::StartOutboundChannel,
            methods::START_OUTBOUND_CHANNEL,
            (name, config, proxy),
        )
        .await
    }

    /// Prepare a host-visible channel control response.
    pub async fn prepare_channel_control(
        &self,
        request: ChannelControlRequest,
    ) -> Result<ChannelControlResult, ChannelRuntimeError> {
        self.confidential(
            ChannelRuntimeOperation::PrepareChannelControl,
            methods::PREPARE_CHANNEL_CONTROL,
            (request,),
        )
        .await
    }

    /// Execute one progressive reply operation.
    pub async fn execute_progressive(
        &self,
        command: ProgressiveCommand,
    ) -> Result<ProgressiveResult, ChannelRuntimeError> {
        self.confidential(
            ChannelRuntimeOperation::ExecuteProgressive,
            methods::EXECUTE_PROGRESSIVE,
            (command,),
        )
        .await
    }

    /// Prepare one outbound intent inside TinyChannels.
    pub async fn prepare_outbound_intent(
        &self,
        channel: String,
        preparation: tinychannels_bus::module_runtime::ChannelMessagePreparation,
    ) -> Result<tinychannels_bus::module_runtime::PreparedChannelIntent, ChannelRuntimeError> {
        self.confidential(
            ChannelRuntimeOperation::PrepareOutboundIntent,
            methods::PREPARE_OUTBOUND_INTENT,
            (channel, preparation),
        )
        .await
    }

    /// Run a typed interaction on one configured provider.
    pub async fn channel_interaction(
        &self,
        request: ChannelInteractionRequest,
    ) -> Result<ChannelInteractionResult, ChannelRuntimeError> {
        self.confidential(
            ChannelRuntimeOperation::ChannelInteraction,
            methods::CHANNEL_INTERACTION,
            (request,),
        )
        .await
    }

    /// Start a relay and return the module's typed capability descriptor.
    pub async fn start_relay(
        &self,
        reservation: RelayReservation,
        config: tinychannels_bus::config::RelayRuntimeConfig,
    ) -> Result<tinychannels_bus::relay::CapabilityDescriptor, ChannelRuntimeError> {
        self.confidential(
            ChannelRuntimeOperation::StartRelay,
            methods::START_RELAY,
            (reservation, config),
        )
        .await
    }

    /// Reserve an outbound relay handle.
    pub async fn reserve_relay(&self) -> Result<RelayReservation, ChannelRuntimeError> {
        self.ordinary(
            ChannelRuntimeOperation::ReserveRelay,
            methods::RESERVE_RELAY,
            (),
        )
        .await
    }

    /// Stop one reserved relay.
    pub async fn stop_relay(
        &self,
        reservation: RelayReservation,
    ) -> Result<(), ChannelRuntimeError> {
        self.ordinary(
            ChannelRuntimeOperation::StopRelay,
            methods::STOP_RELAY,
            (reservation,),
        )
        .await
    }

    /// Read one provider's status.
    pub async fn channel_status(&self, name: String) -> Result<String, ChannelRuntimeError> {
        self.ordinary(
            ChannelRuntimeOperation::ChannelStatus,
            methods::CHANNEL_STATUS,
            (name,),
        )
        .await
    }

    /// Send one typed intent through a configured provider.
    pub async fn send_channel_intent(
        &self,
        name: String,
        intent: ChannelOutboundIntent,
    ) -> Result<ChannelSendMessageResult, ChannelRuntimeError> {
        self.ordinary(
            ChannelRuntimeOperation::SendChannelIntent,
            methods::SEND_CHANNEL_INTENT,
            (name, intent),
        )
        .await
    }

    /// Send one typed intent through a reserved relay.
    pub async fn send_relay_intent(
        &self,
        reservation: RelayReservation,
        intent: ChannelOutboundIntent,
    ) -> Result<ChannelSendMessageResult, ChannelRuntimeError> {
        self.ordinary(
            ChannelRuntimeOperation::SendRelayIntent,
            methods::SEND_RELAY_INTENT,
            (reservation, intent),
        )
        .await
    }

    /// Shut down provider and relay services owned by this runtime object.
    pub async fn shutdown(&self) -> Result<(), ChannelRuntimeError> {
        self.ordinary(ChannelRuntimeOperation::Shutdown, methods::SHUTDOWN, ())
            .await
    }
}
