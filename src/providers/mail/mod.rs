//! Stub.
#[cfg(feature = "email")]
mod imap;
#[cfg(feature = "email-send")]
mod smtp;
mod types;

#[cfg(feature = "email")]
pub use imap::{AsyncImapReceiver, parse_message};
#[cfg(feature = "email-send")]
pub use smtp::LettreMailSender;
pub use types::*;

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
