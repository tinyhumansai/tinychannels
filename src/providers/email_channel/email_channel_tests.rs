#![allow(deprecated)]

use super::*;
use crate::channel::ChannelAdapter;
use tinychannels_bus::SendMessage;
use tokio::sync::Notify;

#[test]
fn default_smtp_port_uses_tls_port() {
    assert_eq!(EmailConfig::default().smtp_port, 465);
}

#[test]
fn email_config_default_uses_tls_smtp_defaults() {
    let config = EmailConfig::default();
    assert_eq!(config.smtp_port, 465);
    assert!(config.smtp_tls);
}

#[test]
fn default_idle_timeout_is_29_minutes() {
    assert_eq!(EmailConfig::default().idle_timeout_secs, 1740);
}

#[tokio::test]
async fn seen_messages_starts_empty() {
    let channel = EmailChannel::new(EmailConfig::default());
    let seen = channel.seen_messages.lock().await;
    assert!(seen.is_empty());
}

#[tokio::test]
async fn seen_messages_tracks_unique_ids() {
    let channel = EmailChannel::new(EmailConfig::default());
    let mut seen = channel.seen_messages.lock().await;

    assert!(seen.insert("first-id".to_string()));
    assert!(!seen.insert("first-id".to_string()));
    assert!(seen.insert("second-id".to_string()));
    assert_eq!(seen.len(), 2);
}

// EmailConfig tests

#[test]
fn email_config_default() {
    let config = EmailConfig::default();
    assert_eq!(config.imap_host, "");
    assert_eq!(config.imap_port, 993);
    assert_eq!(config.imap_folder, "INBOX");
    assert_eq!(config.smtp_host, "");
    assert_eq!(config.smtp_port, 465);
    assert!(config.smtp_tls);
    assert_eq!(config.username, "");
    assert_eq!(config.password, "");
    assert_eq!(config.from_address, "");
    assert_eq!(config.idle_timeout_secs, 1740);
    assert!(config.allowed_senders.is_empty());
}

#[test]
fn email_config_custom() {
    let config = EmailConfig {
        imap_host: "imap.example.com".to_string(),
        imap_port: 993,
        imap_folder: "Archive".to_string(),
        smtp_host: "smtp.example.com".to_string(),
        smtp_port: 465,
        smtp_tls: true,
        username: "user@example.com".to_string(),
        password: "pass123".to_string(),
        from_address: "bot@example.com".to_string(),
        idle_timeout_secs: 1200,
        allowed_senders: vec!["allowed@example.com".to_string()],
    };
    assert_eq!(config.imap_host, "imap.example.com");
    assert_eq!(config.imap_folder, "Archive");
    assert_eq!(config.idle_timeout_secs, 1200);
}

#[test]
fn email_config_clone() {
    let config = EmailConfig {
        imap_host: "imap.test.com".to_string(),
        imap_port: 993,
        imap_folder: "INBOX".to_string(),
        smtp_host: "smtp.test.com".to_string(),
        smtp_port: 587,
        smtp_tls: true,
        username: "user@test.com".to_string(),
        password: "secret".to_string(),
        from_address: "bot@test.com".to_string(),
        idle_timeout_secs: 1740,
        allowed_senders: vec!["*".to_string()],
    };
    let cloned = config.clone();
    assert_eq!(cloned.imap_host, config.imap_host);
    assert_eq!(cloned.smtp_port, config.smtp_port);
    assert_eq!(cloned.allowed_senders, config.allowed_senders);
}

// EmailChannel tests

#[tokio::test]
async fn email_channel_new() {
    let config = EmailConfig::default();
    let channel = EmailChannel::new(config.clone());
    assert_eq!(channel.config.imap_host, config.imap_host);

    let seen_guard = channel.seen_messages.lock().await;
    assert_eq!(seen_guard.len(), 0);
}

#[test]
fn email_channel_name() {
    let channel = EmailChannel::new(EmailConfig::default());
    assert_eq!(channel.descriptor().id, "email");
}

#[test]
fn build_plain_message_uses_subject_and_body() {
    let channel = EmailChannel::new(EmailConfig {
        from_address: "bot@example.com".to_string(),
        ..Default::default()
    });
    let message = channel
        .build_plain_message("listener@example.com", "Podcast", "Hello there")
        .expect("plain message");
    let wire = String::from_utf8_lossy(&message.formatted()).to_string();
    assert!(wire.contains("Subject: Podcast"));
    assert!(wire.contains("Hello there"));
}

#[test]
fn build_message_with_attachment_adds_audio_part() {
    let channel = EmailChannel::new(EmailConfig {
        from_address: "bot@example.com".to_string(),
        ..Default::default()
    });
    let message = channel
        .build_message_with_attachment(
            "listener@example.com",
            "Weekly briefing",
            "Attached.",
            "briefing.mp3",
            "audio/mpeg".parse().expect("content type"),
            vec![1, 2, 3, 4],
        )
        .expect("attachment message");
    let wire = String::from_utf8_lossy(&message.formatted()).to_string();
    assert!(wire.contains("Subject: Weekly briefing"));
    assert!(wire.contains("filename=\"briefing.mp3\""));
    assert!(wire.contains("Content-Type: audio/mpeg"));
}

// is_sender_allowed tests

#[test]
fn is_sender_allowed_empty_list_denies_all() {
    let config = EmailConfig {
        allowed_senders: vec![],
        ..Default::default()
    };
    let channel = EmailChannel::new(config);
    assert!(!channel.is_sender_allowed("anyone@example.com"));
    assert!(!channel.is_sender_allowed("user@test.com"));
}

#[test]
fn is_sender_allowed_wildcard_allows_all() {
    let config = EmailConfig {
        allowed_senders: vec!["*".to_string()],
        ..Default::default()
    };
    let channel = EmailChannel::new(config);
    assert!(channel.is_sender_allowed("anyone@example.com"));
    assert!(channel.is_sender_allowed("user@test.com"));
    assert!(channel.is_sender_allowed("random@domain.org"));
}

#[test]
fn is_sender_allowed_specific_email() {
    let config = EmailConfig {
        allowed_senders: vec!["allowed@example.com".to_string()],
        ..Default::default()
    };
    let channel = EmailChannel::new(config);
    assert!(channel.is_sender_allowed("allowed@example.com"));
    assert!(!channel.is_sender_allowed("other@example.com"));
    assert!(!channel.is_sender_allowed("allowed@other.com"));
}

#[test]
fn is_sender_allowed_domain_with_at_prefix() {
    let config = EmailConfig {
        allowed_senders: vec!["@example.com".to_string()],
        ..Default::default()
    };
    let channel = EmailChannel::new(config);
    assert!(channel.is_sender_allowed("user@example.com"));
    assert!(channel.is_sender_allowed("admin@example.com"));
    assert!(!channel.is_sender_allowed("user@other.com"));
}

#[test]
fn is_sender_allowed_domain_without_at_prefix() {
    let config = EmailConfig {
        allowed_senders: vec!["example.com".to_string()],
        ..Default::default()
    };
    let channel = EmailChannel::new(config);
    assert!(channel.is_sender_allowed("user@example.com"));
    assert!(channel.is_sender_allowed("admin@example.com"));
    assert!(!channel.is_sender_allowed("user@other.com"));
}

#[test]
fn is_sender_allowed_case_insensitive() {
    let config = EmailConfig {
        allowed_senders: vec!["Allowed@Example.COM".to_string()],
        ..Default::default()
    };
    let channel = EmailChannel::new(config);
    assert!(channel.is_sender_allowed("allowed@example.com"));
    assert!(channel.is_sender_allowed("ALLOWED@EXAMPLE.COM"));
    assert!(channel.is_sender_allowed("AlLoWeD@eXaMpLe.cOm"));
}

#[test]
fn is_sender_allowed_multiple_senders() {
    let config = EmailConfig {
        allowed_senders: vec![
            "user1@example.com".to_string(),
            "user2@test.com".to_string(),
            "@allowed.com".to_string(),
        ],
        ..Default::default()
    };
    let channel = EmailChannel::new(config);
    assert!(channel.is_sender_allowed("user1@example.com"));
    assert!(channel.is_sender_allowed("user2@test.com"));
    assert!(channel.is_sender_allowed("anyone@allowed.com"));
    assert!(!channel.is_sender_allowed("user3@example.com"));
}

#[test]
fn is_sender_allowed_wildcard_with_specific() {
    let config = EmailConfig {
        allowed_senders: vec!["*".to_string(), "specific@example.com".to_string()],
        ..Default::default()
    };
    let channel = EmailChannel::new(config);
    assert!(channel.is_sender_allowed("anyone@example.com"));
    assert!(channel.is_sender_allowed("specific@example.com"));
}

#[test]
fn is_sender_allowed_empty_sender() {
    let config = EmailConfig {
        allowed_senders: vec!["@example.com".to_string()],
        ..Default::default()
    };
    let channel = EmailChannel::new(config);
    assert!(!channel.is_sender_allowed(""));
    // "@example.com" ends with "@example.com" so it's allowed
    assert!(channel.is_sender_allowed("@example.com"));
}

// strip_html tests

#[test]
fn strip_html_basic() {
    assert_eq!(EmailChannel::strip_html("<p>Hello</p>"), "Hello");
    assert_eq!(EmailChannel::strip_html("<div>World</div>"), "World");
}

#[test]
fn strip_html_nested_tags() {
    assert_eq!(
        EmailChannel::strip_html("<div><p>Hello <strong>World</strong></p></div>"),
        "Hello World"
    );
}

#[test]
fn strip_html_multiple_lines() {
    let html = "<div>\n  <p>Line 1</p>\n  <p>Line 2</p>\n</div>";
    assert_eq!(EmailChannel::strip_html(html), "Line 1 Line 2");
}

#[test]
fn strip_html_preserves_text() {
    assert_eq!(EmailChannel::strip_html("No tags here"), "No tags here");
    assert_eq!(EmailChannel::strip_html(""), "");
}

#[test]
fn strip_html_handles_malformed() {
    assert_eq!(EmailChannel::strip_html("<p>Unclosed"), "Unclosed");
    // The function removes everything between < and >, so "Text>with>brackets" becomes "Textwithbrackets"
    assert_eq!(
        EmailChannel::strip_html("Text>with>brackets"),
        "Textwithbrackets"
    );
}

#[test]
fn strip_html_self_closing_tags() {
    // Self-closing tags are removed but don't add spaces
    assert_eq!(EmailChannel::strip_html("Hello<br/>World"), "HelloWorld");
    assert_eq!(EmailChannel::strip_html("Text<hr/>More"), "TextMore");
}

#[test]
fn strip_html_attributes_preserved() {
    assert_eq!(
        EmailChannel::strip_html("<a href=\"http://example.com\">Link</a>"),
        "Link"
    );
}

#[test]
fn strip_html_multiple_spaces_collapsed() {
    assert_eq!(
        EmailChannel::strip_html("<p>Word</p>  <p>Word</p>"),
        "Word Word"
    );
}

#[test]
fn strip_html_special_characters() {
    assert_eq!(
        EmailChannel::strip_html("<span>&lt;tag&gt;</span>"),
        "&lt;tag&gt;"
    );
}

// Default function tests

#[test]
fn default_imap_port_returns_993() {
    assert_eq!(EmailConfig::default().imap_port, 993);
}

#[test]
fn default_smtp_port_returns_465() {
    assert_eq!(EmailConfig::default().smtp_port, 465);
}

#[test]
fn default_imap_folder_returns_inbox() {
    assert_eq!(EmailConfig::default().imap_folder, "INBOX");
}

#[test]
fn default_true_returns_true() {
    assert!(EmailConfig::default().smtp_tls);
}

// EmailConfig serialization tests

#[test]
fn email_config_serialize_deserialize() {
    let config = EmailConfig {
        imap_host: "imap.example.com".to_string(),
        imap_port: 993,
        imap_folder: "INBOX".to_string(),
        smtp_host: "smtp.example.com".to_string(),
        smtp_port: 587,
        smtp_tls: true,
        username: "user@example.com".to_string(),
        password: "password123".to_string(),
        from_address: "bot@example.com".to_string(),
        idle_timeout_secs: 1740,
        allowed_senders: vec!["allowed@example.com".to_string()],
    };

    let json = serde_json::to_string(&config).unwrap();
    let deserialized: EmailConfig = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.imap_host, config.imap_host);
    assert_eq!(deserialized.smtp_port, config.smtp_port);
    assert_eq!(deserialized.allowed_senders, config.allowed_senders);
}

#[test]
fn email_config_deserialize_with_defaults() {
    let json = r#"{
        "imap_host": "imap.test.com",
        "smtp_host": "smtp.test.com",
        "username": "user",
        "password": "pass",
        "from_address": "bot@test.com"
    }"#;

    let config: EmailConfig = serde_json::from_str(json).unwrap();
    assert_eq!(config.imap_port, 993); // default
    assert_eq!(config.smtp_port, 465); // default
    assert!(config.smtp_tls); // default
    assert_eq!(config.idle_timeout_secs, 1740); // default
}

#[test]
fn idle_timeout_deserializes_explicit_value() {
    let json = r#"{
        "imap_host": "imap.test.com",
        "smtp_host": "smtp.test.com",
        "username": "user",
        "password": "pass",
        "from_address": "bot@test.com",
        "idle_timeout_secs": 900
    }"#;
    let config: EmailConfig = serde_json::from_str(json).unwrap();
    assert_eq!(config.idle_timeout_secs, 900);
}

#[test]
fn idle_timeout_deserializes_legacy_poll_interval_alias() {
    let json = r#"{
        "imap_host": "imap.test.com",
        "smtp_host": "smtp.test.com",
        "username": "user",
        "password": "pass",
        "from_address": "bot@test.com",
        "poll_interval_secs": 120
    }"#;
    let config: EmailConfig = serde_json::from_str(json).unwrap();
    assert_eq!(config.idle_timeout_secs, 120);
}

#[test]
fn idle_timeout_propagates_to_channel() {
    let config = EmailConfig {
        idle_timeout_secs: 600,
        ..Default::default()
    };
    let channel = EmailChannel::new(config);
    assert_eq!(channel.config.idle_timeout_secs, 600);
}

#[test]
fn email_config_debug_output() {
    let config = EmailConfig {
        imap_host: "imap.debug.com".to_string(),
        ..Default::default()
    };
    let debug_str = format!("{:?}", config);
    assert!(debug_str.contains("imap.debug.com"));
}

// ── is_sender_allowed comprehensive matrix ─────────────────────

fn channel_with_allowlist(allowlist: Vec<String>) -> EmailChannel {
    let cfg = EmailConfig {
        imap_host: "imap.x".into(),
        imap_port: 993,
        imap_folder: "INBOX".into(),
        smtp_host: "smtp.x".into(),
        smtp_port: 465,
        smtp_tls: true,
        username: "u".into(),
        password: "p".into(),
        from_address: "me@x".into(),
        idle_timeout_secs: 300,
        allowed_senders: allowlist,
    };
    EmailChannel::new(cfg)
}

#[test]
fn is_sender_allowed_empty_denies_all() {
    let ch = channel_with_allowlist(vec![]);
    assert!(!ch.is_sender_allowed("anyone@any.com"));
}

#[test]
fn is_sender_allowed_wildcard_allows_everyone() {
    let ch = channel_with_allowlist(vec!["*".into()]);
    assert!(ch.is_sender_allowed("anyone@any.com"));
    assert!(ch.is_sender_allowed("other@different.com"));
}

#[test]
fn is_sender_allowed_full_email_exact_match_case_insensitive() {
    let ch = channel_with_allowlist(vec!["alice@example.com".into()]);
    assert!(ch.is_sender_allowed("alice@example.com"));
    assert!(ch.is_sender_allowed("ALICE@EXAMPLE.COM"));
    assert!(!ch.is_sender_allowed("bob@example.com"));
}

#[test]
fn is_sender_allowed_at_prefix_domain_match() {
    let ch = channel_with_allowlist(vec!["@trusted.com".into()]);
    assert!(ch.is_sender_allowed("user@trusted.com"));
    assert!(ch.is_sender_allowed("other@Trusted.com"));
    assert!(!ch.is_sender_allowed("user@untrusted.com"));
}

#[test]
fn is_sender_allowed_bare_domain_match_is_case_insensitive() {
    let ch = channel_with_allowlist(vec!["trusted.com".into()]);
    assert!(ch.is_sender_allowed("user@trusted.com"));
    assert!(ch.is_sender_allowed("USER@TRUSTED.COM"));
    assert!(!ch.is_sender_allowed("user@other.com"));
}

#[test]
fn is_sender_allowed_prevents_subdomain_confusion() {
    // "trusted.com" must NOT match "user@malicioustrusted.com"
    let ch = channel_with_allowlist(vec!["trusted.com".into()]);
    assert!(!ch.is_sender_allowed("user@notmytrusted.com"));
    assert!(!ch.is_sender_allowed("user@trusted.com.evil.com"));
}

// ── strip_html edge cases ──────────────────────────────────────

#[test]
fn strip_html_empty_string() {
    assert_eq!(EmailChannel::strip_html(""), "");
}

#[test]
fn strip_html_only_tags() {
    assert_eq!(EmailChannel::strip_html("<p></p><br/>"), "");
}

#[test]
fn strip_html_unclosed_tag_eats_rest_until_gt() {
    // A '<' without '>' enters tag mode; anything after until a '>' is
    // discarded. This is the implementation's behaviour — lock it in.
    assert_eq!(EmailChannel::strip_html("before<never closed"), "before");
}

#[test]
fn strip_html_collapses_whitespace_runs() {
    assert_eq!(
        EmailChannel::strip_html("<p>hello</p>\n\n\n   <p>world</p>"),
        "hello world"
    );
}

fn smtp_config(port: u16, tls: bool) -> EmailConfig {
    EmailConfig {
        imap_host: "imap.example.com".to_string(),
        imap_port: 993,
        smtp_host: "127.0.0.1".to_string(),
        smtp_port: port,
        smtp_tls: tls,
        username: String::new(),
        password: "pw".to_string(),
        from_address: "bot@example.com".to_string(),
        ..Default::default()
    }
}

#[test]
fn the_config_maps_onto_per_call_mail_credentials() {
    use crate::providers::mail::SmtpSecurity;

    let mut config = smtp_config(465, true);
    config.username = "bot".to_string();
    let channel = EmailChannel::new(config);
    let smtp = channel.smtp_credentials();
    assert_eq!(smtp.security, SmtpSecurity::Ssl, "smtp_tls is implicit TLS");
    assert_eq!((smtp.host.as_str(), smtp.port), ("127.0.0.1", 465));
    assert_eq!(smtp.username, "bot");
    assert_eq!(smtp.password.expose(), "pw");
    assert_eq!(smtp.from_email, "bot@example.com");

    let imap = channel.imap_credentials();
    assert_eq!((imap.host.as_str(), imap.port), ("imap.example.com", 993));
    assert_eq!(imap.password.expose(), "pw");

    let plain = EmailChannel::new(smtp_config(25, false)).smtp_credentials();
    assert_eq!(plain.security, SmtpSecurity::None);
}

/// `Channel::send` runs inside an async runtime, so it must not drive a
/// blocking SMTP transport there. It goes through the async sender instead.
#[tokio::test(flavor = "current_thread")]
async fn channel_send_delivers_over_the_async_transport() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    // A current-thread runtime: a blocking send here would deadlock against
    // the fake server below, which runs on the same thread.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let (sock, _) = listener.accept().await.unwrap();
        let (read, mut write) = sock.into_split();
        let mut lines = BufReader::new(read).lines();
        let mut seen = Vec::new();
        write.write_all(b"220 fake\r\n").await.unwrap();
        let mut in_data = false;
        while let Ok(Some(line)) = lines.next_line().await {
            seen.push(line.clone());
            let reply: &[u8] = match (in_data, line.as_str()) {
                (true, ".") => {
                    in_data = false;
                    b"250 queued\r\n"
                }
                (true, _) => continue,
                (false, l) if l.starts_with("DATA") => {
                    in_data = true;
                    b"354 go\r\n"
                }
                (false, l) if l.starts_with("QUIT") => {
                    let _ = write.write_all(b"221 bye\r\n").await;
                    break;
                }
                _ => b"250 ok\r\n",
            };
            write.write_all(reply).await.unwrap();
        }
        seen.join("\n")
    });

    let channel = EmailChannel::new(smtp_config(port, false));
    let message = SendMessage {
        subject: Some("Greetings".to_string()),
        ..SendMessage::new("Body text", "to@example.com")
    };
    crate::provider_test_support::send_v1(&channel, &message)
        .await
        .unwrap();

    let transcript = server.await.unwrap();
    assert!(
        transcript.contains("RCPT TO:<to@example.com>"),
        "{transcript}"
    );
    assert!(transcript.contains("Subject: Greetings"), "{transcript}");
    assert!(transcript.contains("Body text"), "{transcript}");
}

// ── IMAP receive path against a scripted plaintext server ──────

use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

const ALICE: &[u8] = b"From: Alice <alice@example.com>\r\nSubject: Hi\r\nMessage-ID: <a1@example.com>\r\nDate: Tue, 2 Jan 2024 03:04:05 +0000\r\n\r\nHello world\r\n";
const MALLORY: &[u8] =
    b"From: mallory@evil.test\r\nSubject: Spam\r\nMessage-ID: <m1@evil.test>\r\n\r\nBuy now\r\n";
const ALICE_AGAIN: &[u8] = b"From: alice@example.com\r\nSubject: Hi again\r\nMessage-ID: <a1@example.com>\r\n\r\nSame id\r\n";
const ALICE_NO_MESSAGE_ID: &[u8] =
    b"From: alice@example.com\r\nSubject: No Message-ID\r\n\r\nStable by UID\r\n";
const BOB_HTML: &[u8] = b"From: bob@example.com\r\nSubject: Html\r\nContent-Type: text/html\r\n\r\n<p>Hello <b>Bob</b></p>\r\n";
const BAD_DATE: &[u8] = b"From: carol@example.com\r\nSubject: Odd date\r\nMessage-ID: <c1@example.com>\r\nDate: Sat, 31 Feb 2024 10:00:00 +0000\r\n\r\nWhen?\r\n";

/// What the fake server does after answering one `IDLE` with `+ idling`.
#[derive(Clone, Copy, Debug)]
enum IdleStep {
    /// Report new mail at once, then answer `DONE`.
    NewMail,
    /// Stay silent until the client sends `DONE`.
    WaitForDone,
    /// Send a truncated response and hang up.
    Truncate,
}

/// The script for one accepted connection.
#[derive(Clone, Default)]
struct ConnScript {
    reject_login: bool,
    reject_store: bool,
    stall_logout: Option<(Arc<Notify>, Arc<Notify>)>,
    /// The i-th `UID SEARCH` answers with the i-th entry; later ones are empty.
    searches: Vec<Vec<(u32, &'static [u8])>>,
    idles: Vec<IdleStep>,
}

/// A plaintext IMAP server that serves `scripts` one connection each, then
/// stops listening. Returns every command line received, per connection.
async fn fake_imap(scripts: Vec<ConnScript>) -> (u16, tokio::task::JoinHandle<Vec<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = tokio::spawn(async move {
        let mut transcripts = Vec::new();
        for script in scripts {
            let (sock, _) = listener.accept().await.unwrap();
            transcripts.push(serve_connection(sock, script).await);
        }
        transcripts
    });
    (port, handle)
}

async fn serve_connection(sock: tokio::net::TcpStream, script: ConnScript) -> Vec<String> {
    let (read, mut write) = sock.into_split();
    let mut lines = BufReader::new(read).lines();
    let mut seen = Vec::new();
    let mut searches = script.searches.into_iter();
    let mut idles = script.idles.into_iter();
    let mut current: Vec<(u32, &'static [u8])> = Vec::new();
    let mut idle_tag: Option<String> = None;
    if write.write_all(b"* OK fake ready\r\n").await.is_err() {
        return seen;
    }
    while let Ok(Some(line)) = lines.next_line().await {
        seen.push(line.clone());
        if line == "DONE" {
            let tag = idle_tag.take().unwrap_or_default();
            let _ = write
                .write_all(format!("{tag} OK IDLE terminated\r\n").as_bytes())
                .await;
            continue;
        }
        let (tag, command) = line.split_once(' ').unwrap_or((line.as_str(), ""));
        let upper = command.to_ascii_uppercase();
        let mut reply = Vec::new();
        if upper.starts_with("LOGIN") && script.reject_login {
            reply.extend_from_slice(format!("{tag} NO bad credentials\r\n").as_bytes());
            let _ = write.write_all(&reply).await;
            continue;
        } else if upper.starts_with("SELECT") {
            reply.extend_from_slice(
                b"* 2 EXISTS\r\n* FLAGS (\\Seen)\r\n* OK [UIDVALIDITY 1234] UIDs valid\r\n",
            );
        } else if upper.starts_with("UID SEARCH") {
            current = searches.next().unwrap_or_default();
            reply.extend_from_slice(b"* SEARCH");
            for (uid, _) in &current {
                reply.extend_from_slice(format!(" {uid}").as_bytes());
            }
            reply.extend_from_slice(b"\r\n");
        } else if upper.starts_with("UID FETCH") {
            if upper.contains("UID BODY.PEEK[]") && !upper.contains("(UID BODY.PEEK[])") {
                reply.extend_from_slice(format!("{tag} BAD invalid attribute list\r\n").as_bytes());
                let _ = write.write_all(&reply).await;
                continue;
            }
            if upper.contains("RFC822") || upper.contains("BODY[]") {
                for (uid, _) in &current {
                    seen.push(format!("IMPLICIT_SEEN_BY_FETCH {uid}"));
                }
            }
            for (seq, (uid, raw)) in current.iter().enumerate() {
                reply.extend_from_slice(
                    format!(
                        "* {} FETCH (UID {uid} BODY[] {{{}}}\r\n",
                        seq + 1,
                        raw.len()
                    )
                    .as_bytes(),
                );
                reply.extend_from_slice(raw);
                reply.extend_from_slice(b")\r\n");
            }
        } else if upper.starts_with("UID STORE") && script.reject_store {
            reply.extend_from_slice(format!("{tag} NO cannot mark seen\r\n").as_bytes());
            let _ = write.write_all(&reply).await;
            continue;
        } else if upper.starts_with("IDLE") {
            idle_tag = Some(tag.to_string());
            match idles.next() {
                Some(IdleStep::NewMail) => {
                    let _ = write.write_all(b"+ idling\r\n* 3 EXISTS\r\n").await;
                }
                Some(IdleStep::WaitForDone) => {
                    let _ = write.write_all(b"+ idling\r\n").await;
                }
                Some(IdleStep::Truncate) | None => {
                    let _ = write.write_all(b"+ idling\r\n* 3 EXI").await;
                    break;
                }
            }
            continue;
        } else if upper.starts_with("LOGOUT") {
            if let Some((observed, release)) = &script.stall_logout {
                observed.notify_one();
                release.notified().await;
            }
            reply.extend_from_slice(b"* BYE\r\n");
        }
        reply.extend_from_slice(format!("{tag} OK done\r\n").as_bytes());
        if write.write_all(&reply).await.is_err() || upper.starts_with("LOGOUT") {
            break;
        }
    }
    seen
}

fn imap_channel(allowed: &[&str], idle_timeout_secs: u64) -> EmailChannel {
    let mut channel = EmailChannel::new(EmailConfig {
        imap_host: "127.0.0.1".to_string(),
        username: "bot".to_string(),
        password: "pw".to_string(),
        idle_timeout_secs,
        allowed_senders: allowed.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    });
    channel.reconnect_backoff = Duration::from_millis(1);
    channel.max_reconnect_backoff = Duration::from_millis(2);
    channel
}

async fn plain_session(
    channel: &EmailChannel,
    port: u16,
) -> Result<Session<tokio::net::TcpStream>> {
    let tcp = TcpStream::connect(("127.0.0.1", port)).await?;
    channel.login_imap(tcp).await
}

const HANG_GUARD: Duration = Duration::from_secs(20);

/// A server that never answers the connect must not hold the reconnect loop:
/// each attempt is bounded by `IMAP_CONNECT_TIMEOUT` and the loop retries.
#[tokio::test(start_paused = true)]
async fn a_hung_imap_connect_times_out_and_the_loop_retries() {
    let channel = imap_channel(&["@example.com"], 30);
    let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (tx, _rx) = mpsc::channel(1);
    let counted = attempts.clone();
    let listen = tokio::spawn(async move {
        channel
            .listen_with_reconnect(&tx, || {
                counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                std::future::pending::<Result<Session<tokio::net::TcpStream>>>()
            })
            .await
    });

    // Two timeouts plus the 1-2 ms backoffs between them.
    tokio::time::sleep(inbound::IMAP_CONNECT_TIMEOUT * 2 + Duration::from_secs(1)).await;
    assert!(
        attempts.load(std::sync::atomic::Ordering::SeqCst) >= 3,
        "each hung attempt timed out and was retried"
    );
    listen.abort();
}

/// A credentials refusal is typed, so the loop keeps it at error level while
/// ordinary disconnects become warnings.
#[tokio::test]
async fn a_refused_imap_login_is_typed_as_a_login_rejection() {
    let (port, _server) = fake_imap(vec![ConnScript {
        reject_login: true,
        ..Default::default()
    }])
    .await;
    let channel = imap_channel(&["@example.com"], 30);

    let error = plain_session(&channel, port).await.unwrap_err();

    assert!(
        error.downcast_ref::<ImapLoginRejected>().is_some(),
        "{error:#}"
    );
    assert!(
        error.to_string().starts_with("IMAP login failed:"),
        "{error}"
    );
}

async fn recv(rx: &mut mpsc::Receiver<ChannelMessage>) -> ChannelMessage {
    timeout(HANG_GUARD, rx.recv())
        .await
        .expect("message within the hang guard")
        .expect("channel open")
}

/// One listen run across three connections: delivery with the allowlist and
/// dedupe applied, IDLE new-mail wakeups, a dropped IDLE forcing a reconnect,
/// a rejected login forcing another, and a closed receiver ending delivery.
#[tokio::test]
async fn listen_delivers_new_mail_and_reconnects_after_failures() {
    let (port, server) = fake_imap(vec![
        ConnScript {
            searches: vec![
                vec![(1, ALICE), (2, MALLORY)],
                vec![(3, ALICE_AGAIN), (4, BOB_HTML)],
                vec![],
            ],
            idles: vec![IdleStep::NewMail, IdleStep::NewMail, IdleStep::Truncate],
            ..Default::default()
        },
        ConnScript {
            reject_login: true,
            ..Default::default()
        },
        ConnScript {
            searches: vec![vec![(5, BAD_DATE)]],
            idles: vec![IdleStep::Truncate],
            ..Default::default()
        },
    ])
    .await;

    let channel = imap_channel(&["@example.com"], 30);
    let (tx, mut rx) = mpsc::channel(8);
    let listen = tokio::spawn(async move {
        channel
            .listen_with_reconnect(&tx, || plain_session(&channel, port))
            .await
    });

    let first = recv(&mut rx).await;
    assert_eq!(first.id, "a1@example.com");
    assert_eq!(first.sender, "alice@example.com");
    assert_eq!(first.reply_target, "alice@example.com");
    assert_eq!(first.channel, "email");
    assert_eq!(first.content.trim_end(), "Subject: Hi\n\nHello world");
    assert_eq!(first.timestamp, 1_704_164_645);

    // Mallory is blocked and Alice's repeat is deduped by Message-ID.
    let second = recv(&mut rx).await;
    assert_eq!(second.sender, "bob@example.com");
    assert_eq!(second.content.trim_end(), "Subject: Html\n\nHello Bob");
    assert!(second.id.starts_with("email:"), "{}", second.id);
    assert!(second.timestamp > 1_704_164_645);

    let third = recv(&mut rx).await;
    assert_eq!(third.sender, "carol@example.com");
    assert_eq!(third.timestamp, 0, "an impossible date maps to 0");

    let transcripts = timeout(HANG_GUARD, server)
        .await
        .expect("server finished")
        .unwrap();
    listen.abort();

    assert_eq!(transcripts.len(), 3);
    let first_conn = transcripts[0].join("\n");
    assert!(first_conn.contains("SELECT \"INBOX\""), "{first_conn}");
    // The UID set comes from a search result set, so its order is open.
    let either = |prefix: &str, suffix: &str| {
        first_conn.contains(&format!("{prefix} 1,2 {suffix}"))
            || first_conn.contains(&format!("{prefix} 2,1 {suffix}"))
    };
    assert!(either("UID FETCH", "(UID BODY.PEEK[])"), "{first_conn}");
    assert!(
        first_conn.contains("UID STORE 1 +FLAGS (\\Seen)"),
        "{first_conn}"
    );
    assert!(
        first_conn.contains("UID STORE 2 +FLAGS (\\Seen)"),
        "{first_conn}"
    );
    assert!(first_conn.contains("DONE"), "{first_conn}");
    assert!(
        !transcripts[1].iter().any(|l| l.contains("SELECT")),
        "a rejected login never selects: {:?}",
        transcripts[1]
    );
}

/// A closed receiver ends delivery quietly without acknowledging an undelivered
/// message.
#[tokio::test]
async fn processing_stops_when_the_receiver_is_gone() {
    let (port, server) = fake_imap(vec![ConnScript {
        searches: vec![vec![(1, ALICE), (4, BOB_HTML)]],
        ..Default::default()
    }])
    .await;
    let channel = imap_channel(&["*"], 30);
    let (tx, rx) = mpsc::channel(8);
    drop(rx);
    let mut session = plain_session(&channel, port).await.unwrap();
    session.select("INBOX").await.unwrap();
    channel.process_unseen(&mut session, &tx).await.unwrap();
    assert!(channel.seen_messages.lock().await.is_empty());
    let _ = session.logout().await;
    let transcripts = server.await.unwrap();
    assert!(!transcripts[0].iter().any(|l| l.contains("UID STORE")));
}

/// An empty mailbox fetches nothing and marks nothing.
#[tokio::test]
async fn an_empty_search_fetches_nothing() {
    let (port, server) = fake_imap(vec![ConnScript::default()]).await;
    let channel = imap_channel(&["*"], 30);
    let mut session = plain_session(&channel, port).await.unwrap();
    session.select("INBOX").await.unwrap();
    assert!(
        channel
            .fetch_unseen(&mut session, 1234)
            .await
            .unwrap()
            .is_empty()
    );
    let _ = session.logout().await;
    let commands = server.await.unwrap().remove(0).join("\n");
    assert!(!commands.contains("UID FETCH"), "{commands}");
    assert!(!commands.contains("UID STORE"), "{commands}");
}

/// Fetching a message must not mark it read before the host accepts its
/// ordered ingress event. The listener owns that acknowledgement after push.
#[tokio::test]
async fn fetched_mail_stays_unseen_until_ingress_is_accepted() {
    let (port, server) = fake_imap(vec![ConnScript {
        searches: vec![vec![(7, ALICE)]],
        ..Default::default()
    }])
    .await;
    let channel = imap_channel(&["*"], 30);
    let mut session = plain_session(&channel, port).await.unwrap();
    session.select("INBOX").await.unwrap();
    let messages = channel.fetch_unseen(&mut session, 1234).await.unwrap();
    assert_eq!(messages.len(), 1);
    let _ = session.logout().await;
    let commands = server.await.unwrap().remove(0).join("\n");
    assert!(
        !commands.contains("UID STORE"),
        "fetch must leave the UID unseen until dispatch succeeds: {commands}"
    );
    assert!(
        commands.contains("BODY.PEEK[]"),
        "fetch must not set Seen: {commands}"
    );
    assert!(
        commands.contains("(UID BODY.PEEK[])") || !commands.contains("UID BODY.PEEK[]"),
        "multiple fetch attributes require the IMAP parenthesized list: {commands}"
    );
    assert!(!commands.contains("IMPLICIT_SEEN_BY_FETCH"), "{commands}");
}

/// The IDLE budget elapsing ends IDLE with `DONE` and hands back a usable
/// session.
#[tokio::test]
async fn an_idle_timeout_returns_the_session() {
    let (port, server) = fake_imap(vec![ConnScript {
        idles: vec![IdleStep::WaitForDone],
        ..Default::default()
    }])
    .await;
    let channel = imap_channel(&["*"], 0);
    let mut session = plain_session(&channel, port).await.unwrap();
    session.select("INBOX").await.unwrap();
    let (result, mut session) = channel.wait_for_changes(session).await.unwrap();
    assert_eq!(result, IdleWaitResult::Timeout);
    session.logout().await.unwrap();
    let commands = server.await.unwrap().remove(0);
    assert!(commands.iter().any(|l| l == "DONE"), "{commands:?}");
}

#[test]
fn idle_responses_map_to_wait_results() {
    assert_eq!(
        IdleWaitResult::from_response(&IdleResponse::Timeout),
        IdleWaitResult::Timeout
    );
    assert_eq!(
        IdleWaitResult::from_response(&IdleResponse::ManualInterrupt),
        IdleWaitResult::Interrupted
    );
}

#[tokio::test]
async fn health_check_passes_on_a_successful_login() {
    let (port, server) = fake_imap(vec![ConnScript::default()]).await;
    let channel = imap_channel(&["*"], 30);
    assert!(
        channel
            .typed_health_state(plain_session(&channel, port), HANG_GUARD)
            .await
            == "healthy"
    );
    let commands = server.await.unwrap().remove(0).join("\n");
    assert!(commands.contains("LOGIN"), "{commands}");
    assert!(commands.contains("LOGOUT"), "{commands}");
}

#[tokio::test]
async fn health_check_fails_on_a_rejected_login() {
    let (port, _server) = fake_imap(vec![ConnScript {
        reject_login: true,
        ..Default::default()
    }])
    .await;
    let channel = imap_channel(&["*"], 30);
    assert_eq!(
        channel
            .typed_health_state(plain_session(&channel, port), HANG_GUARD)
            .await,
        "unhealthy"
    );
}

/// A server that accepts but never greets stalls the probe past its budget.
#[tokio::test]
async fn health_check_fails_when_the_probe_stalls() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let holder = tokio::spawn(async move {
        let (sock, _) = listener.accept().await.unwrap();
        std::future::pending::<()>().await;
        drop(sock);
    });
    let channel = imap_channel(&["*"], 30);
    assert_eq!(
        channel
            .typed_health_state(plain_session(&channel, port), Duration::from_millis(50))
            .await,
        "unhealthy"
    );
    holder.abort();
}

/// A successful login remains healthy if LOGOUT stalls, but that best-effort
/// cleanup must still finish within the same total health-probe deadline.
#[tokio::test]
async fn health_check_bounds_a_stalled_logout_after_successful_login() {
    let logout_seen = Arc::new(Notify::new());
    let allow_logout = Arc::new(Notify::new());
    let (port, server) = fake_imap(vec![ConnScript {
        stall_logout: Some((logout_seen.clone(), allow_logout.clone())),
        ..Default::default()
    }])
    .await;
    let channel = imap_channel(&["*"], 30);
    let mut probe = tokio::spawn(async move {
        channel
            .typed_health_state(plain_session(&channel, port), Duration::from_secs(2))
            .await
    });

    if tokio::time::timeout(HANG_GUARD, logout_seen.notified())
        .await
        .is_err()
    {
        allow_logout.notify_one();
        let _ = server.await;
        panic!("the fake server observed the logout request");
    }
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(2)).await;
    let completion_before_logout_release =
        tokio::time::timeout(Duration::from_secs(1), &mut probe).await;
    allow_logout.notify_one();
    let (completed_before_logout_release, state) = match completion_before_logout_release {
        Ok(Ok(state)) => (true, Some(state)),
        Ok(Err(_)) => (true, None),
        Err(_) => match probe.await {
            Ok(state) => (false, Some(state)),
            Err(_) => (false, None),
        },
    };
    let _ = server.await.unwrap();

    assert!(
        completed_before_logout_release,
        "health status must finish at its total deadline even when LOGOUT never replies"
    );
    assert_eq!(
        state,
        Some("healthy"),
        "login succeeded before cleanup stalled"
    );
}

/// After the IDLE budget elapses the session re-checks the mailbox and
/// delivers what arrived meanwhile; unparsable fetches are skipped unmarked.
#[tokio::test]
async fn an_idle_timeout_rechecks_the_mailbox() {
    let (port, server) = fake_imap(vec![ConnScript {
        searches: vec![vec![(6, b"")], vec![(1, ALICE)]],
        idles: vec![IdleStep::WaitForDone, IdleStep::Truncate],
        ..Default::default()
    }])
    .await;
    let channel = imap_channel(&["*"], 0);
    let (tx, mut rx) = mpsc::channel(8);
    let listen = tokio::spawn(async move {
        channel
            .listen_with_reconnect(&tx, || plain_session(&channel, port))
            .await
    });
    let message = recv(&mut rx).await;
    assert_eq!(message.sender, "alice@example.com");
    let commands = timeout(HANG_GUARD, server)
        .await
        .expect("server finished")
        .unwrap()
        .remove(0);
    listen.abort();
    let stores: Vec<_> = commands
        .iter()
        .filter(|l| l.contains("UID STORE"))
        .collect();
    assert_eq!(
        stores.len(),
        1,
        "only the parsed fetch is marked: {commands:?}"
    );
    assert!(stores[0].contains("UID STORE 1 "), "{commands:?}");
}

/// The real TLS probe refuses a plaintext server instead of logging in.
#[tokio::test]
async fn health_check_over_tls_fails_against_a_plaintext_server() {
    let (port, _server) = fake_imap(vec![ConnScript::default()]).await;
    let mut channel = imap_channel(&["*"], 30);
    channel.config.imap_host = "localhost".to_string();
    channel.config.imap_port = port;
    assert!(!crate::provider_test_support::is_healthy(&channel).await);
}

/// `listen` keeps reconnecting while the server keeps failing the TLS
/// handshake, rather than giving up.
#[tokio::test]
async fn listen_retries_a_failing_server() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let accepts = tokio::spawn(async move {
        for _ in 0..3 {
            let (sock, _) = listener.accept().await.unwrap();
            drop(sock);
        }
    });
    let mut channel = imap_channel(&["*"], 30);
    channel.config.imap_host = "localhost".to_string();
    channel.config.imap_port = port;
    let (tx, _rx) = mpsc::channel(1);
    let listen =
        tokio::spawn(async move { crate::provider_test_support::listen_v1(&channel, tx).await });
    timeout(HANG_GUARD, accepts)
        .await
        .expect("three connection attempts")
        .unwrap();
    assert!(!listen.is_finished(), "listen must keep retrying");
    listen.abort();
}

#[tokio::test]
async fn an_ip_literal_imap_host_fails_before_tls() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let mut channel = imap_channel(&["*"], 30);
    // An IP literal is not a DNS name the SNI conversion accepts.
    channel.config.imap_port = port;
    let result = timeout(HANG_GUARD, channel.connect_imap())
        .await
        .expect("fails without waiting on a handshake");
    assert!(result.is_err());
    drop(listener);
}

// ── message text extraction ────────────────────────────────────

#[test]
fn a_text_attachment_is_used_when_there_is_no_body() {
    let raw = b"From: a@example.com\r\nSubject: Att\r\nContent-Type: multipart/mixed; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: text/csv\r\nContent-Disposition: attachment; filename=\"data.csv\"\r\n\r\na,b\r\n--b--\r\n";
    let parsed = test_support::parse_email_fixture(raw).unwrap();
    assert_eq!(parsed.text, "a,b");
}

#[test]
fn a_message_with_only_binary_content_has_no_readable_text() {
    let raw = b"From: a@example.com\r\nSubject: Bin\r\nContent-Type: multipart/mixed; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=\"x.bin\"\r\n\r\nabc\r\n--b--\r\n";
    let parsed = test_support::parse_email_fixture(raw).unwrap();
    assert_eq!(parsed.text, "(no readable content)");
}

#[test]
fn a_message_without_a_sender_is_unknown() {
    let parsed = test_support::parse_email_fixture(b"Subject: Anon\r\n\r\nhi\r\n").unwrap();
    assert_eq!(parsed.sender, "unknown");
    assert_eq!(parsed.subject.as_deref(), Some("Anon"));
}

// ── blocking SMTP send ─────────────────────────────────────────

/// A one-shot blocking SMTP server; returns the received transcript.
fn blocking_smtp_server() -> (u16, std::thread::JoinHandle<String>) {
    use std::io::{BufRead, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = std::thread::spawn(move || {
        let (sock, _) = listener.accept().unwrap();
        let mut write = sock.try_clone().unwrap();
        let mut reader = std::io::BufReader::new(sock);
        let mut seen = Vec::new();
        write.write_all(b"220 fake\r\n").unwrap();
        let mut in_data = false;
        let mut line = String::new();
        while reader.read_line(&mut line).unwrap_or(0) > 0 {
            let l = line.trim_end().to_string();
            line.clear();
            seen.push(l.clone());
            let reply: &[u8] = match (in_data, l.as_str()) {
                (true, ".") => {
                    in_data = false;
                    b"250 queued\r\n"
                }
                (true, _) => continue,
                (false, l) if l.starts_with("DATA") => {
                    in_data = true;
                    b"354 go\r\n"
                }
                (false, l) if l.starts_with("QUIT") => {
                    let _ = write.write_all(b"221 bye\r\n");
                    break;
                }
                (false, l) if l.starts_with("EHLO") => b"250-fake\r\n250 AUTH PLAIN\r\n",
                (false, l) if l.starts_with("AUTH") => b"235 authenticated\r\n",
                _ => b"250 ok\r\n",
            };
            write.write_all(reply).unwrap();
        }
        seen.join("\n")
    });
    (port, handle)
}

#[test]
fn send_message_delivers_over_blocking_smtp() {
    let (port, server) = blocking_smtp_server();
    let channel = EmailChannel::new(smtp_config(port, false));
    let message = channel
        .build_plain_message("to@example.com", "Blocking", "Plain body")
        .unwrap();
    channel.send_message(message).unwrap();
    drop(channel);
    let transcript = server.join().unwrap();
    assert!(
        transcript.contains("RCPT TO:<to@example.com>"),
        "{transcript}"
    );
    assert!(transcript.contains("Subject: Blocking"), "{transcript}");
}

#[test]
fn send_message_over_tls_refuses_a_plaintext_server() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        use std::io::Write;
        let (mut sock, _) = listener.accept().unwrap();
        let _ = sock.write_all(b"220 fake\r\n");
    });
    let mut config = smtp_config(port, true);
    config.smtp_host = "localhost".to_string();
    let channel = EmailChannel::new(config);
    let message = channel
        .build_plain_message("to@example.com", "Tls", "body")
        .unwrap();
    assert!(channel.send_message(message).is_err());
    server.join().unwrap();
}

#[test]
fn building_a_message_rejects_a_bad_address() {
    let channel = EmailChannel::new(smtp_config(25, false));
    assert!(
        channel
            .build_plain_message("not an address", "s", "b")
            .is_err()
    );
}

// ── Channel::send subject handling ─────────────────────────────

async fn send_and_capture(content: &str) -> String {
    let (port, server) = blocking_smtp_server();
    let channel = EmailChannel::new(smtp_config(port, false));
    crate::provider_test_support::send_v1(&channel, &SendMessage::new(content, "to@example.com"))
        .await
        .unwrap();
    tokio::task::spawn_blocking(move || server.join().unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn send_takes_the_subject_from_a_legacy_subject_line() {
    let transcript = send_and_capture("Subject: Legacy\nThe body").await;
    assert!(transcript.contains("Subject: Legacy"), "{transcript}");
    assert!(transcript.contains("The body"), "{transcript}");
}

#[tokio::test]
async fn send_uses_the_default_subject_for_a_lone_subject_line() {
    let transcript = send_and_capture("Subject: no newline").await;
    assert!(
        transcript.contains("Subject: OpenHuman Message"),
        "{transcript}"
    );
}

#[tokio::test]
async fn send_uses_the_default_subject_for_plain_content() {
    let transcript = send_and_capture("just text").await;
    assert!(
        transcript.contains("Subject: OpenHuman Message"),
        "{transcript}"
    );
    assert!(transcript.contains("just text"), "{transcript}");
}

const BOB_RETRY: &[u8] = b"From: bob@example.com\r\nSubject: Retry\r\nMessage-ID: <b1@example.com>\r\n\r\nSecond mail\r\n";

struct RejectSecondTypedEmail {
    accepted: std::sync::Mutex<Vec<tinychannels_bus::ChannelIngress>>,
}

#[async_trait::async_trait]
impl crate::channel::ChannelInboundSink for RejectSecondTypedEmail {
    async fn push(
        &self,
        envelope: tinychannels_bus::ChannelInboundEnvelope,
    ) -> Result<(), crate::channel::ChannelSendError> {
        self.push_ingress(tinychannels_bus::ChannelIngress::Inbound(envelope))
            .await
    }

    async fn push_ingress(
        &self,
        ingress: tinychannels_bus::ChannelIngress,
    ) -> Result<(), crate::channel::ChannelSendError> {
        let mut accepted = self.accepted.lock().unwrap();
        if accepted.len() == 1 {
            return Err(crate::channel::ChannelSendError {
                message: "test rejection".into(),
                ..Default::default()
            });
        }
        accepted.push(ingress);
        Ok(())
    }
}

struct TypedEmailRecorder(tokio::sync::mpsc::Sender<tinychannels_bus::ChannelIngress>);

#[async_trait::async_trait]
impl crate::channel::ChannelInboundSink for TypedEmailRecorder {
    async fn push(
        &self,
        envelope: tinychannels_bus::ChannelInboundEnvelope,
    ) -> Result<(), crate::channel::ChannelSendError> {
        self.push_ingress(tinychannels_bus::ChannelIngress::Inbound(envelope))
            .await
    }

    async fn push_ingress(
        &self,
        ingress: tinychannels_bus::ChannelIngress,
    ) -> Result<(), crate::channel::ChannelSendError> {
        self.0
            .send(ingress)
            .await
            .map_err(|_| crate::channel::ChannelSendError::default())
    }
}

#[tokio::test]
async fn rejected_typed_mail_is_retried_without_replaying_accepted_uids() {
    let channel = imap_channel(&["*"], 30);
    let (port, server) = fake_imap(vec![ConnScript {
        searches: vec![vec![(1, ALICE), (2, BOB_RETRY)]],
        ..Default::default()
    }])
    .await;
    let mut session = plain_session(&channel, port).await.unwrap();
    session.select("INBOX").await.unwrap();
    let rejecting = RejectSecondTypedEmail {
        accepted: std::sync::Mutex::new(Vec::new()),
    };
    let error = channel
        .process_unseen_to(&mut session, &EmailDelivery::Typed(&rejecting), 1234)
        .await
        .expect_err("a rejected typed ingress must stop this delivery pass");
    assert!(error.downcast_ref::<InboundSinkRejected>().is_some());
    {
        let accepted = rejecting.accepted.lock().unwrap();
        assert_eq!(accepted.len(), 1);
        assert!(matches!(
            &accepted[0],
            tinychannels_bus::ChannelIngress::Inbound(envelope)
                if envelope.message_id == "a1@example.com"
        ));
    }
    assert_eq!(channel.seen_messages.lock().await.len(), 1);
    let _ = session.logout().await;
    let commands = server.await.unwrap().remove(0).join("\n");
    assert!(
        commands.contains("UID STORE 1 +FLAGS (\\Seen)"),
        "{commands}"
    );
    assert!(
        !commands.contains("UID STORE 2"),
        "the rejected second UID must remain unseen: {commands}"
    );

    let (port, server) = fake_imap(vec![ConnScript {
        searches: vec![vec![(1, ALICE), (2, BOB_RETRY)]],
        ..Default::default()
    }])
    .await;
    let mut session = plain_session(&channel, port).await.unwrap();
    session.select("INBOX").await.unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    let recorder = TypedEmailRecorder(tx);
    channel
        .process_unseen_to(&mut session, &EmailDelivery::Typed(&recorder), 1234)
        .await
        .unwrap();
    let Some(tinychannels_bus::ChannelIngress::Inbound(retried)) = rx.try_recv().ok() else {
        panic!("the previously rejected email should be delivered on retry");
    };
    assert_eq!(retried.message_id, "b1@example.com");
    assert!(rx.try_recv().is_err(), "the accepted first UID is deduped");
    assert_eq!(channel.seen_messages.lock().await.len(), 2);
    let _ = session.logout().await;
    let commands = server.await.unwrap().remove(0).join("\n");
    assert!(
        commands.contains("UID STORE 1 +FLAGS (\\Seen)"),
        "{commands}"
    );
    assert!(
        commands.contains("UID STORE 2 +FLAGS (\\Seen)"),
        "{commands}"
    );
}

#[tokio::test]
async fn accepted_email_retries_only_failed_seen_acknowledgement() {
    let channel = imap_channel(&["*"], 30);
    let (port, server) = fake_imap(vec![ConnScript {
        searches: vec![vec![(7, ALICE)]],
        reject_store: true,
        ..Default::default()
    }])
    .await;
    let mut session = plain_session(&channel, port).await.unwrap();
    session.select("INBOX").await.unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    let recorder = TypedEmailRecorder(tx);
    let error = channel
        .process_unseen_to(&mut session, &EmailDelivery::Typed(&recorder), 1234)
        .await
        .expect_err("a rejected IMAP STORE must fail the acknowledgement");
    assert!(error.to_string().contains("cannot mark seen"), "{error:#}");
    let Some(tinychannels_bus::ChannelIngress::Inbound(accepted)) = rx.try_recv().ok() else {
        panic!("the email was accepted before the STORE acknowledgement failed");
    };
    assert_eq!(accepted.message_id, "a1@example.com");
    assert!(rx.try_recv().is_err());
    assert_eq!(channel.seen_messages.lock().await.len(), 1);
    drop(session);
    let failed_commands = server.await.unwrap().remove(0).join("\n");
    assert!(failed_commands.contains("UID STORE 7 +FLAGS (\\Seen)"));

    let (port, server) = fake_imap(vec![ConnScript {
        searches: vec![vec![(7, ALICE)]],
        ..Default::default()
    }])
    .await;
    let mut session = plain_session(&channel, port).await.unwrap();
    session.select("INBOX").await.unwrap();
    channel
        .process_unseen_to(&mut session, &EmailDelivery::Typed(&recorder), 1234)
        .await
        .unwrap();
    assert!(
        rx.try_recv().is_err(),
        "accepted ingress must not be replayed"
    );
    let _ = session.logout().await;
    let retry_commands = server.await.unwrap().remove(0).join("\n");
    assert!(retry_commands.contains("UID STORE 7 +FLAGS (\\Seen)"));
}

#[tokio::test]
async fn accepted_email_without_message_id_is_not_replayed_after_store_retry() {
    let channel = imap_channel(&["*"], 30);
    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    let recorder = TypedEmailRecorder(tx);
    let (port, server) = fake_imap(vec![ConnScript {
        searches: vec![vec![(7, ALICE_NO_MESSAGE_ID)]],
        reject_store: true,
        ..Default::default()
    }])
    .await;
    let mut session = plain_session(&channel, port).await.unwrap();
    session.select("INBOX").await.unwrap();
    channel
        .process_unseen_to(&mut session, &EmailDelivery::Typed(&recorder), 1234)
        .await
        .expect_err("the first Seen acknowledgement is rejected");
    let Some(tinychannels_bus::ChannelIngress::Inbound(first)) = rx.try_recv().ok() else {
        panic!("the first ingress should have been accepted");
    };
    let expected_identity = "127.0.0.1:bot:INBOX:1234:7";
    assert_eq!(
        first.message_id,
        format!(
            "email:{}",
            hex::encode(sha2::Sha256::digest(expected_identity))
        )
    );
    drop(session);
    let _ = server.await.unwrap();

    let (port, server) = fake_imap(vec![ConnScript {
        searches: vec![vec![(7, ALICE_NO_MESSAGE_ID)]],
        ..Default::default()
    }])
    .await;
    let mut session = plain_session(&channel, port).await.unwrap();
    session.select("INBOX").await.unwrap();
    channel
        .process_unseen_to(&mut session, &EmailDelivery::Typed(&recorder), 1234)
        .await
        .unwrap();
    assert!(
        rx.try_recv().is_err(),
        "retrying STORE must not deliver a message with no Message-ID twice"
    );
    drop(session);
    let _ = server.await.unwrap();
}

struct BlockingTypedEmailSink(tokio::sync::Notify);

#[async_trait::async_trait]
impl crate::channel::ChannelInboundSink for BlockingTypedEmailSink {
    async fn push(
        &self,
        envelope: tinychannels_bus::ChannelInboundEnvelope,
    ) -> Result<(), crate::channel::ChannelSendError> {
        self.push_ingress(tinychannels_bus::ChannelIngress::Inbound(envelope))
            .await
    }

    async fn push_ingress(
        &self,
        _ingress: tinychannels_bus::ChannelIngress,
    ) -> Result<(), crate::channel::ChannelSendError> {
        self.0.notify_one();
        std::future::pending().await
    }
}

#[tokio::test]
async fn stopping_a_backpressured_email_listener_releases_it_without_acknowledging() {
    use crate::providers::native::NativeProvider;

    let (port, server) = fake_imap(vec![ConnScript {
        searches: vec![vec![(9, ALICE)]],
        idles: vec![IdleStep::WaitForDone],
        ..Default::default()
    }])
    .await;
    let channel = std::sync::Arc::new(imap_channel(&["*"], 30));
    let sink = std::sync::Arc::new(BlockingTypedEmailSink(tokio::sync::Notify::new()));
    let listener_channel = channel.clone();
    let listener_sink = sink.clone();
    let listener = tokio::spawn(async move {
        listener_channel
            .listen_typed_with(listener_sink.as_ref(), || {
                plain_session(listener_channel.as_ref(), port)
            })
            .await
    });
    tokio::time::timeout(HANG_GUARD, sink.0.notified())
        .await
        .expect("typed ingress reaches the deliberately blocked sink");
    NativeProvider::stop_listener(channel.as_ref())
        .await
        .unwrap();
    tokio::time::timeout(HANG_GUARD, listener)
        .await
        .expect("stop interrupts ingress backpressure")
        .unwrap()
        .unwrap();
    assert!(channel.seen_messages.lock().await.is_empty());
    assert!(channel.listener_cancel.lock().unwrap().is_none());
    let commands = server.await.unwrap().remove(0).join("\n");
    assert!(
        !commands.contains("UID STORE"),
        "cancelled delivery must remain unseen: {commands}"
    );
}

#[tokio::test]
async fn aborting_email_listener_start_releases_ownership_for_restart() {
    use crate::providers::native::NativeProvider;

    let channel = std::sync::Arc::new(imap_channel(&["*"], 30));
    let sink = TypedEmailRecorder(tokio::sync::mpsc::channel(4).0);
    let ready = std::sync::Arc::new(tokio::sync::Notify::new());
    let task_channel = channel.clone();
    let task_ready = ready.clone();
    let abandoned = tokio::spawn(async move {
        task_channel
            .listen_typed_with(&sink, move || {
                let ready = task_ready.clone();
                async move {
                    ready.notify_one();
                    std::future::pending::<Result<Session<TcpStream>>>().await
                }
            })
            .await
    });
    tokio::time::timeout(HANG_GUARD, ready.notified())
        .await
        .expect("listener registered before connecting");
    abandoned.abort();
    let _ = abandoned.await;
    assert!(channel.listener_cancel.lock().unwrap().is_none());

    let (port, server) = fake_imap(vec![ConnScript {
        searches: vec![vec![(11, ALICE)]],
        idles: vec![IdleStep::WaitForDone],
        ..Default::default()
    }])
    .await;
    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    let sink = TypedEmailRecorder(tx);
    let task_channel = channel.clone();
    let listener = tokio::spawn(async move {
        task_channel
            .listen_typed_with(&sink, || plain_session(task_channel.as_ref(), port))
            .await
    });
    let received = tokio::time::timeout(HANG_GUARD, rx.recv())
        .await
        .expect("restart accepts a new typed listener")
        .expect("typed ingress remains open");
    assert!(matches!(
        received,
        tinychannels_bus::ChannelIngress::Inbound(envelope)
            if envelope.message_id == "a1@example.com"
    ));
    NativeProvider::stop_listener(channel.as_ref())
        .await
        .unwrap();
    tokio::time::timeout(HANG_GUARD, listener)
        .await
        .expect("restarted listener stops")
        .unwrap()
        .unwrap();
    server.await.unwrap();
}
