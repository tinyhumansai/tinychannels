//! Stub.
use std::time::Duration;
use async_trait::async_trait;
use super::types::*;
/// Stub.
#[derive(Debug, Clone, Default)]
pub struct LettreMailSender;
impl LettreMailSender {
    /// Stub.
    pub fn new() -> Self { Self }
    /// Stub.
    pub fn with_timeout(self, _t: Duration) -> Self { self }
}
#[async_trait]
impl MailSender for LettreMailSender {
    async fn send(&self, _c: &MailCredentials, _e: &OutboundEmail) -> Result<(), MailError> { Err(MailError::Config(String::new())) }
}
#[cfg(test)]
#[path = "smtp_tests.rs"]
mod tests;
