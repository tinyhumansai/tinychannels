//! Debug-build helpers for raw integration tests. They exercise the email
//! parser without opening IMAP or SMTP sockets.

use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedEmailFixture {
    pub sender: String,
    pub text: String,
    pub subject: Option<String>,
}

pub fn parse_email_fixture(raw: &[u8]) -> Option<ParsedEmailFixture> {
    let parsed = MessageParser::default().parse(raw)?;
    Some(ParsedEmailFixture {
        sender: EmailChannel::extract_sender(&parsed),
        text: EmailChannel::extract_text(&parsed),
        subject: parsed.subject().map(str::to_string),
    })
}
