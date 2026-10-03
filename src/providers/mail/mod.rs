//! Async, per-credential mail transport: the provider-agnostic seam
//! [`EmailChannel`](crate::providers::EmailChannel) sends through, usable on
//! its own by a host that holds many mail accounts.
//!
//! Sending and receiving each have two independent axes:
//!
//! - **What** — [`OutboundEmail`] / [`InboundEmail`], the same for every
//!   provider.
//! - **How** — [`MailCredentials`] (provider-tagged, so a stored blob names its
//!   own transport) and [`ImapCredentials`], handed to the transport **per
//!   call**, so one [`MailSender`] / [`MailReceiver`] serves every account.
//!
//! The vocabulary here is dependency-free and always compiled. The transports
//! follow the crate's feature split: [`LettreMailSender`] needs `email-send`
//! (`lettre` only); [`AsyncImapReceiver`] and [`parse_message`] need `email`
//! (adds `async-imap` and `mail-parser`). Passwords are [`MailSecret`]s, which
//! render `"[redacted]"` through both `Debug` and `Serialize`.
//!
//! Ported from OpenCompany's `server/ops/{mailer,smtp,imap}.rs`. See
//! `README.md` in this directory.
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
