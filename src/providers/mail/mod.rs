//! Stub.
#[cfg(feature = "email-send")]
mod smtp;
mod types;

#[cfg(feature = "email-send")]
pub use smtp::LettreMailSender;
pub use types::*;

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
