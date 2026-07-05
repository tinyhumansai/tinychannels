//! Portable channel provider implementations.

pub mod dingtalk;
pub mod discord;
pub mod email_channel;
pub mod imessage;
pub mod irc;
pub mod lark;
pub mod linq;
pub mod mattermost;
pub mod qq;
pub mod signal;
pub mod slack;
pub mod whatsapp;
pub mod whatsapp_web;
pub mod yuanbao;

pub use dingtalk::DingTalkChannel;
pub use discord::DiscordChannel;
pub use email_channel::EmailChannel;
pub use imessage::IMessageChannel;
pub use irc::{IrcChannel, IrcChannelConfig};
pub use lark::LarkChannel;
pub use linq::{LinqChannel, verify_linq_signature};
pub use mattermost::MattermostChannel;
pub use qq::QQChannel;
pub use signal::SignalChannel;
pub use slack::SlackChannel;
pub use whatsapp::WhatsAppChannel;
pub use whatsapp_web::WhatsAppWebChannel;
pub use yuanbao::YuanbaoChannel;
