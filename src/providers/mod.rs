//! Portable channel provider implementations.

pub mod signal;
pub mod slack;
pub mod whatsapp;

pub use signal::SignalChannel;
pub use slack::SlackChannel;
pub use whatsapp::WhatsAppChannel;
