//! Portable channel provider implementations.

pub mod dingtalk;
pub mod discord;
#[cfg(feature = "email-send")]
pub mod email_channel;
#[cfg(feature = "email")]
pub mod email_verify;
pub mod imessage;
pub mod irc;
#[cfg(feature = "lark")]
pub mod lark;
pub mod linq;
pub mod mattermost;
pub mod qq;
pub mod signal;
pub mod slack;
pub mod telegram;
pub mod whatsapp;
pub mod whatsapp_web;
pub mod yuanbao;

pub use dingtalk::DingTalkChannel;
pub use discord::DiscordChannel;
#[cfg(feature = "email-send")]
pub use email_channel::EmailChannel;
#[cfg(feature = "email")]
pub use email_verify::verify_email_credentials;
pub use imessage::IMessageChannel;
pub use irc::{IrcChannel, IrcChannelConfig};
#[cfg(feature = "lark")]
pub use lark::LarkChannel;
pub use linq::{LinqChannel, verify_linq_signature};
pub use mattermost::MattermostChannel;
pub use qq::QQChannel;
pub use signal::SignalChannel;
pub use slack::SlackChannel;
pub use telegram::TelegramChannel;
pub use tinychannels_runtime::CliChannel;
pub use whatsapp::WhatsAppChannel;
pub use whatsapp_web::WhatsAppWebChannel;
pub use yuanbao::YuanbaoChannel;
