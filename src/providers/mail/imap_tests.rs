use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

use super::*;
use crate::providers::mail::MailSecret;

const ALICE: &[u8] = b"From: Alice <alice@example.com>\r\nSubject: Hi\r\n\r\nHello world\r\n";
const BOB: &[u8] = b"From: bob@example.com\r\nSubject: Second\r\n\r\nAnother body\r\n";

fn creds(port: u16) -> ImapCredentials {
    ImapCredentials {
        host: "127.0.0.1".into(),
        port,
        username: "alice".into(),
        password: MailSecret::new("pw"),
    }
}

#[test]
fn parse_message_extracts_headers_and_text_body() {
    let message = parse_message(ALICE);
    assert_eq!(message.from_email, "alice@example.com");
    assert_eq!(message.from_name, "Alice");
    assert_eq!(message.subject, "Hi");
    assert!(message.body.contains("Hello world"));
}

#[test]
fn parse_message_leaves_absent_fields_empty() {
    let message = parse_message(BOB);
    assert_eq!(message.from_name, "");
    assert_eq!(message.from_email, "bob@example.com");
    assert_eq!(parse_message(b""), InboundEmail::default());
}

/// A plaintext IMAP server holding UIDs 3 (Alice) and 7 (Bob), both unseen.
/// Returns every command line it received.
async fn fake_imap() -> (u16, tokio::task::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = tokio::spawn(async move {
        let (sock, _) = listener.accept().await.unwrap();
        let (read, mut write) = sock.into_split();
        let mut lines = BufReader::new(read).lines();
        let mut seen = Vec::new();
        write.write_all(b"* OK fake ready\r\n").await.unwrap();
        while let Ok(Some(line)) = lines.next_line().await {
            seen.push(line.clone());
            let (tag, command) = line.split_once(' ').unwrap_or((line.as_str(), ""));
            let upper = command.to_ascii_uppercase();
            let mut reply = Vec::new();
            if upper.starts_with("SELECT") {
                reply.extend_from_slice(b"* 2 EXISTS\r\n* FLAGS (\\Seen)\r\n");
            } else if upper.starts_with("UID SEARCH") {
                reply.extend_from_slice(b"* SEARCH 7 3\r\n");
            } else if upper.starts_with("UID FETCH") {
                for (seq, uid, raw) in [(1, 3, ALICE), (2, 7, BOB)] {
                    reply.extend_from_slice(
                        format!("* {seq} FETCH (UID {uid} BODY[] {{{}}}\r\n", raw.len()).as_bytes(),
                    );
                    reply.extend_from_slice(raw);
                    reply.extend_from_slice(b")\r\n");
                }
            } else if upper.starts_with("LOGOUT") {
                reply.extend_from_slice(b"* BYE\r\n");
            }
            reply.extend_from_slice(format!("{tag} OK done\r\n").as_bytes());
            if write.write_all(&reply).await.is_err() || upper.starts_with("LOGOUT") {
                break;
            }
        }
        seen
    });
    (port, handle)
}

#[tokio::test]
async fn unseen_mail_is_fetched_by_uid_without_being_marked() {
    let (port, server) = fake_imap().await;
    let tcp = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let mut session = open_session(tcp, &creds(port), "INBOX").await.unwrap();
    let fetched = fetch_unseen_on(&mut session).await.unwrap();
    store_seen_on(&mut session, &[3, 7]).await.unwrap();
    let _ = session.logout().await;

    assert_eq!(
        fetched.iter().map(|f| f.uid).collect::<Vec<_>>(),
        vec![3, 7]
    );
    assert_eq!(fetched[0].email.subject, "Hi");
    assert_eq!(fetched[1].email.from_email, "bob@example.com");

    let commands = server.await.unwrap();
    let joined = commands.join("\n");
    assert!(joined.contains("SELECT \"INBOX\""), "{joined}");
    assert!(joined.contains("UID SEARCH UNSEEN"), "{joined}");
    // PEEK is the point: a plain BODY[] fetch would set \Seen, and a message
    // that then failed to file would never be fetched again.
    assert!(joined.contains("UID FETCH 3,7 (UID BODY.PEEK[])"), "{joined}");
    let fetch_at = commands.iter().position(|c| c.contains("UID FETCH")).unwrap();
    let store_at = commands.iter().position(|c| c.contains("UID STORE")).unwrap();
    assert!(fetch_at < store_at, "nothing is marked during the fetch");
    assert!(joined.contains("UID STORE 3,7 +FLAGS.SILENT (\\Seen)"), "{joined}");
}

#[tokio::test]
async fn marking_nothing_never_dials() {
    // Port 9 (discard) is not listening; a dial would fail.
    AsyncImapReceiver::new()
        .mark_seen(&creds(9), &[])
        .await
        .unwrap();
}

#[tokio::test]
async fn a_stalled_server_times_out() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let _hold = tokio::spawn(async move {
        let (sock, _) = listener.accept().await.unwrap();
        tokio::time::sleep(Duration::from_secs(30)).await;
        drop(sock);
    });
    let error = AsyncImapReceiver::new()
        .with_timeout(Duration::from_millis(200))
        .fetch_new(&creds(port))
        .await
        .unwrap_err();
    assert_eq!(error, MailError::TimedOut);
}

#[test]
fn the_mailbox_defaults_to_inbox_and_is_configurable() {
    assert_eq!(AsyncImapReceiver::new().mailbox(), "INBOX");
    assert_eq!(
        AsyncImapReceiver::default()
            .with_mailbox("Archive")
            .mailbox(),
        "Archive"
    );
}
