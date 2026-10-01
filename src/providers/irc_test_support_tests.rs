//! Debug-build seams for raw integration tests. They cover IRC parsing and
//! framing helpers without opening a TLS socket.

use super::*;

#[allow(clippy::type_complexity)]
pub fn parse_line_for_test(
    line: &str,
) -> Option<(Option<String>, String, Vec<String>, Option<String>)> {
    IrcMessage::parse(line).map(|msg| {
        let nick = msg.nick().map(str::to_string);
        (msg.prefix, msg.command, msg.params, nick)
    })
}

pub fn split_message_for_test(message: &str, max_bytes: usize) -> Vec<String> {
    split_message(message, max_bytes)
}

pub fn encode_sasl_plain_for_test(nick: &str, password: &str) -> String {
    encode_sasl_plain(nick, password)
}

pub fn is_user_allowed_for_test(allowed_users: Vec<String>, nick: &str) -> bool {
    IrcChannel::new(IrcChannelConfig {
        server: "irc.example.test".to_string(),
        port: 6697,
        nickname: "openhuman".to_string(),
        username: None,
        channels: vec!["#ops".to_string()],
        allowed_users,
        server_password: None,
        nickserv_password: None,
        sasl_password: None,
        verify_tls: false,
    })
    .is_user_allowed(nick)
}
