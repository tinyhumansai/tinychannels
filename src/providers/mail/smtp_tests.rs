use std::time::Duration;

use base64::Engine;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

use super::*;
use crate::providers::mail::{MailSecret, SmtpCredentials};

/// How the fake server answers `RCPT TO`.
#[derive(Clone, Copy)]
enum Rcpt {
    Accept,
    Refuse,
}

/// A plaintext SMTP server that answers `connections` sessions in order and
/// returns every line it received, session by session.
async fn fake_smtp(
    connections: usize,
    rcpt: Rcpt,
) -> (u16, tokio::task::JoinHandle<Vec<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = tokio::spawn(async move {
        let mut sessions = Vec::new();
        for _ in 0..connections {
            let (sock, _) = listener.accept().await.unwrap();
            let (read, mut write) = sock.into_split();
            let mut lines = BufReader::new(read).lines();
            let mut seen = Vec::new();
            write.write_all(b"220 fake ESMTP\r\n").await.unwrap();
            let mut in_data = false;
            while let Ok(Some(line)) = lines.next_line().await {
                seen.push(line.clone());
                let reply: &[u8] = if in_data {
                    if line == "." {
                        in_data = false;
                        b"250 queued\r\n"
                    } else {
                        continue;
                    }
                } else {
                    let upper = line.to_ascii_uppercase();
                    if upper.starts_with("EHLO") {
                        b"250-fake\r\n250 AUTH PLAIN LOGIN\r\n"
                    } else if upper.starts_with("AUTH") {
                        b"235 ok\r\n"
                    } else if upper.starts_with("RCPT") {
                        match rcpt {
                            Rcpt::Accept => b"250 ok\r\n",
                            Rcpt::Refuse => b"550 no such user\r\n",
                        }
                    } else if upper.starts_with("DATA") {
                        in_data = true;
                        b"354 go ahead\r\n"
                    } else if upper.starts_with("QUIT") {
                        let _ = write.write_all(b"221 bye\r\n").await;
                        break;
                    } else {
                        b"250 ok\r\n"
                    }
                };
                if write.write_all(reply).await.is_err() {
                    break;
                }
            }
            sessions.push(seen);
        }
        sessions
    });
    (port, handle)
}

fn creds(port: u16, username: &str, password: &str) -> MailCredentials {
    MailCredentials::Smtp(SmtpCredentials {
        host: "127.0.0.1".into(),
        port,
        security: SmtpSecurity::None,
        username: username.into(),
        password: MailSecret::new(password),
        from_name: "Acme".into(),
        from_email: "hi@acme.test".into(),
    })
}

fn email(to: &str) -> OutboundEmail {
    OutboundEmail {
        to: to.into(),
        subject: "Hello there".into(),
        body: "A plain body".into(),
    }
}

fn auth_plain(session: &[String]) -> Option<String> {
    let line = session.iter().find(|l| l.to_ascii_uppercase().starts_with("AUTH PLAIN"))?;
    let encoded = line.split_whitespace().nth(2)?;
    let decoded = base64::engine::general_purpose::STANDARD.decode(encoded).ok()?;
    Some(String::from_utf8_lossy(&decoded).replace('\0', "|"))
}

#[tokio::test]
async fn a_message_is_delivered_with_its_envelope_headers_and_body() {
    let (port, server) = fake_smtp(1, Rcpt::Accept).await;
    LettreMailSender::new()
        .send(&creds(port, "", ""), &email("to@x.test"))
        .await
        .unwrap();

    let sessions = server.await.unwrap();
    let session = sessions[0].join("\n");
    assert!(session.contains("MAIL FROM:<hi@acme.test>"), "{session}");
    assert!(session.contains("RCPT TO:<to@x.test>"), "{session}");
    assert!(session.contains("Subject: Hello there"), "{session}");
    assert!(session.contains("A plain body"), "{session}");
    assert!(session.contains("Acme"), "the From display name: {session}");
    // An empty username is an unauthenticated relay: no AUTH attempt.
    assert_eq!(auth_plain(&sessions[0]), None, "{session}");
}

#[tokio::test]
async fn each_send_presents_the_credentials_it_was_handed() {
    let (port, server) = fake_smtp(2, Rcpt::Accept).await;
    let sender = LettreMailSender::default();
    sender
        .send(&creds(port, "alice", "pw-one"), &email("to@x.test"))
        .await
        .unwrap();
    sender
        .send(&creds(port, "bob", "pw-two"), &email("to@x.test"))
        .await
        .unwrap();

    let sessions = server.await.unwrap();
    assert_eq!(auth_plain(&sessions[0]).as_deref(), Some("|alice|pw-one"));
    assert_eq!(auth_plain(&sessions[1]).as_deref(), Some("|bob|pw-two"));
}

#[tokio::test]
async fn a_malformed_address_fails_before_dialing() {
    let error = LettreMailSender::new()
        .send(&creds(9, "", ""), &email("not an address"))
        .await
        .unwrap_err();
    assert!(matches!(error, MailError::InvalidAddress(_)), "{error:?}");
}

#[tokio::test]
async fn a_refused_recipient_is_a_transport_error() {
    let (port, _server) = fake_smtp(1, Rcpt::Refuse).await;
    let error = LettreMailSender::new()
        .send(&creds(port, "", ""), &email("to@x.test"))
        .await
        .unwrap_err();
    assert!(matches!(error, MailError::Transport(_)), "{error:?}");
}

#[tokio::test]
async fn a_stalled_server_times_out() {
    // Accepts the connection and never greets.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let _hold = tokio::spawn(async move {
        let (sock, _) = listener.accept().await.unwrap();
        tokio::time::sleep(Duration::from_secs(30)).await;
        drop(sock);
    });
    let error = LettreMailSender::new()
        .with_timeout(Duration::from_millis(200))
        .send(&creds(port, "", ""), &email("to@x.test"))
        .await
        .unwrap_err();
    assert_eq!(error, MailError::TimedOut);
}
