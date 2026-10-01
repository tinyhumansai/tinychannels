use super::EmailChannel;
use crate::config::EmailConfig;

fn config() -> EmailConfig {
    EmailConfig {
        from_address: "bot@example.com".to_string(),
        username: "bot@example.com".to_string(),
        password: "secret".to_string(),
        smtp_host: "smtp.example.com".to_string(),
        smtp_port: 587,
        smtp_tls: true,
        ..Default::default()
    }
}

/// `EmailChannel::new` + `build_plain_message` + `send_message` are what a
/// send-only host links. Building a message must not need a mailbox.
#[test]
fn a_plain_message_can_be_built_without_the_receive_half() {
    let channel = EmailChannel::new(config());
    let message = channel
        .build_plain_message("someone@example.com", "Subject", "Body")
        .expect("a well-formed plain message should build");
    let raw = String::from_utf8(message.formatted()).expect("message should be UTF-8");
    assert!(raw.contains("someone@example.com"));
    assert!(raw.contains("Subject"));
}

/// The attachment builder is the one OpenHuman's podcast delivery uses.
#[test]
fn an_attachment_message_can_be_built_without_the_receive_half() {
    let channel = EmailChannel::new(config());
    let message = channel
        .build_message_with_attachment(
            "someone@example.com",
            "Your podcast",
            "Attached.",
            "podcast.mp3",
            "audio/mpeg".parse().expect("a valid content type"),
            vec![0u8, 1, 2, 3],
        )
        .expect("a well-formed attachment message should build");
    let raw = String::from_utf8(message.formatted()).expect("message should be UTF-8");
    assert!(raw.contains("podcast.mp3"));
}
