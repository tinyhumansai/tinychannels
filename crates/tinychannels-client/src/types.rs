//! Public, content-free runtime error types shared by the typed client.

use std::fmt;

use tinybus::module::ModuleCallError;

/// A closed, content-free classification for one runtime operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChannelRuntimeOperation {
    /// Bind to the registered, attested TinyChannels peer.
    Bind,
    /// Reserve one module-owned runtime.
    Reserve,
    /// Validate the module-generated reservation.
    ValidateReservation,
    /// Construct the reservation's private module object.
    Object,
    /// Construct the returned callback name and path.
    CallbackAddress,
    /// Start the reserved runtime.
    Start,
    /// Stop the reserved runtime.
    Stop,
    /// Describe configured channel providers.
    DescribeConfiguredChannels,
    /// Start a supervised listener provider.
    StartChannelSupervised,
    /// Start an outbound-only provider.
    StartOutboundChannel,
    /// Prepare a channel control response.
    PrepareChannelControl,
    /// Execute a progressive reply operation.
    ExecuteProgressive,
    /// Prepare a typed outbound intent.
    PrepareOutboundIntent,
    /// Run an interaction on a configured provider.
    ChannelInteraction,
    /// Start a reserved relay and read its capability descriptor.
    StartRelay,
    /// Reserve an outbound relay.
    ReserveRelay,
    /// Stop a reserved relay.
    StopRelay,
    /// Read one provider's status.
    ChannelStatus,
    /// Send one provider intent.
    SendChannelIntent,
    /// Send one relay intent.
    SendRelayIntent,
    /// Shut down module-owned channel services on this object.
    Shutdown,
}

/// A closed error kind. It carries SDK classifications, never peer-provided text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChannelRuntimeErrorKind {
    /// The SDK classified the operation failure.
    Module(ModuleCallError),
    /// The reservation decoded but failed the bus contract's validation.
    InvalidReservation,
    /// The module-returned callback address is not a valid TinyBus address.
    InvalidCallbackAddress,
}

/// A runtime operation error that retains its typed cause without remote text.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct ChannelRuntimeError {
    operation: ChannelRuntimeOperation,
    kind: ChannelRuntimeErrorKind,
}

impl ChannelRuntimeError {
    pub(super) fn module(operation: ChannelRuntimeOperation, kind: ModuleCallError) -> Self {
        Self {
            operation,
            kind: ChannelRuntimeErrorKind::Module(kind),
        }
    }

    pub(super) fn contract(
        operation: ChannelRuntimeOperation,
        kind: ChannelRuntimeErrorKind,
    ) -> Self {
        Self { operation, kind }
    }

    /// The operation that failed.
    #[must_use]
    pub fn operation(self) -> ChannelRuntimeOperation {
        self.operation
    }

    /// The closed SDK or contract classification.
    #[must_use]
    pub fn kind(self) -> ChannelRuntimeErrorKind {
        self.kind
    }
}

impl fmt::Debug for ChannelRuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ChannelRuntimeError")
            .field("operation", &self.operation)
            .field("kind", &self.kind)
            .finish()
    }
}

impl fmt::Display for ChannelRuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "TinyChannels runtime {:?} failed ({:?})",
            self.operation, self.kind
        )
    }
}

impl std::error::Error for ChannelRuntimeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.kind {
            ChannelRuntimeErrorKind::Module(error) => Some(error),
            ChannelRuntimeErrorKind::InvalidReservation
            | ChannelRuntimeErrorKind::InvalidCallbackAddress => None,
        }
    }
}

#[cfg(test)]
#[path = "types_tests.rs"]
mod tests;
