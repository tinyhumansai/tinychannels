use super::*;

// ══════════════════════════════════════════════════════════
// Channel behaviour behind the db-path and script-runner seams
// ══════════════════════════════════════════════════════════

const HANG_GUARD: Duration = Duration::from_secs(20);

fn channel_on(db_path: &Path, contacts: &[&str]) -> IMessageChannel {
    let mut channel = IMessageChannel::new(contacts.iter().map(|c| c.to_string()).collect());
    channel.db_path = Some(db_path.to_path_buf());
    channel.poll_interval = Duration::from_millis(5);
    channel
}

/// A runner that writes its `-e` script to `out` and exits with `code`.
fn fake_runner(out: &Path, code: i32) -> Vec<String> {
    vec![
        "sh".to_string(),
        "-c".to_string(),
        format!(
            "printf '%s\\n---\\n' \"$2\" >> '{}'; echo 'runner said no' >&2; exit {code}",
            out.display()
        ),
        "fake-osascript".to_string(),
    ]
}

fn typed_text_message(content: &str, recipient: &str) -> ChannelOutboundMessage {
    ChannelOutboundMessage {
        channel_id: "imessage".into(),
        conversation_id: recipient.into(),
        channel_ref: None,
        conversation_ref: Some(ConversationRef {
            kind: ConversationKind::Dm,
            id: recipient.into(),
            ..Default::default()
        }),
        reply_to_id: None,
        thread_id: None,
        turn_id: None,
        idempotency_key: "imessage-test-send".into(),
        parts: vec![OutboundContentPart::Content {
            part: ContentPart::Text {
                text: content.into(),
                markdown_dialect: None,
            },
        }],
    }
}

struct IngressSender(mpsc::Sender<ChannelIngress>);

#[async_trait::async_trait]
impl ChannelInboundSink for IngressSender {
    async fn push(
        &self,
        envelope: tinychannels_bus::ChannelInboundEnvelope,
    ) -> Result<(), tinychannels_bus::channel::ChannelSendError> {
        self.push_ingress(tinychannels_bus::ChannelIngress::Inbound(envelope))
            .await
    }

    async fn push_ingress(
        &self,
        ingress: ChannelIngress,
    ) -> Result<(), tinychannels_bus::channel::ChannelSendError> {
        self.0
            .send(ingress)
            .await
            .map_err(|_| tinychannels_bus::channel::ChannelSendError {
                kind: tinychannels_bus::channel::SendErrorKind::Transient,
                message: "test ingress receiver closed".into(),
                retryable: true,
                ..Default::default()
            })
    }
}

#[tokio::test]
async fn send_runs_the_escaped_script_through_the_runner() {
    let mut rig = crate::providers::imessage::fixtures::rig();
    let out = rig.out.clone();
    rig.channel.script_command = fake_runner(&out, 0);
    let recorder = rig.spawn_messages();

    let outcome = ChannelTypedSend::send_typed(
        &rig.channel,
        &typed_text_message("say \"hi\"", "+15555550123"),
    )
    .await
    .unwrap();
    assert_eq!(outcome.receipt.platform_message_ids, ["SENT1"]);
    recorder.abort();

    let script = std::fs::read_to_string(&out).unwrap();
    assert!(script.contains("tell application \"Messages\""), "{script}");
    assert!(script.contains("participant \"+15555550123\""), "{script}");
    assert!(
        script.contains("send \"say \\\"hi\\\"\" to targetBuddy"),
        "{script}"
    );
}

#[tokio::test]
async fn send_reports_a_runner_failure_without_exposing_stderr() {
    let dir = tempfile::tempdir().unwrap();
    let mut rig = crate::providers::imessage::fixtures::rig();
    rig.channel.script_command = fake_runner(&dir.path().join("script.txt"), 1);

    let error = ChannelTypedSend::send_typed(
        &rig.channel,
        &typed_text_message("hello", "user@example.com"),
    )
    .await
    .unwrap_err()
    .message;
    assert!(error.contains("iMessage send failed"), "{error}");
    assert!(!error.contains("runner said no"), "{error}");
}

#[tokio::test]
async fn send_rejects_an_invalid_target_before_running_anything() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("script.txt");
    let mut rig = crate::providers::imessage::fixtures::rig();
    rig.channel.script_command = fake_runner(&out, 0);

    let error =
        ChannelTypedSend::send_typed(&rig.channel, &typed_text_message("hello", "not a target"))
            .await
            .unwrap_err();
    assert!(error.message.contains("target"), "{}", error.message);
    assert!(!out.exists(), "the runner must not be invoked");
}

#[tokio::test]
async fn send_without_a_runner_is_an_error() {
    let mut rig = crate::providers::imessage::fixtures::rig();
    rig.channel.script_command = Vec::new();
    assert!(
        ChannelTypedSend::send_typed(&rig.channel, &typed_text_message("hello", "+15555550123"),)
            .await
            .is_err()
    );
}

#[test]
fn the_default_database_lives_under_the_home_directory() {
    let channel = IMessageChannel::new(vec![]);
    if let Some(path) = channel.messages_db_path() {
        assert!(path.ends_with("Library/Messages/chat.db"), "{path:?}");
    }
    assert_eq!(channel.script_command, vec!["osascript".to_string()]);
}

#[test]
fn health_requires_macos_and_an_existing_database() {
    let (_dir, db_path) = create_test_db();
    let present = channel_on(&db_path, &[]);
    assert!(present.is_healthy_on("macos"));
    assert!(!present.is_healthy_on("linux"));

    let missing = channel_on(&db_path.with_file_name("absent.db"), &[]);
    assert!(!missing.is_healthy_on("macos"));
}

#[tokio::test]
async fn health_check_reflects_the_host_os() {
    let (_dir, db_path) = create_test_db();
    let channel = channel_on(&db_path, &[]);
    assert_eq!(
        ChannelAdapter::status(&channel)
            .await
            .unwrap()
            .health_state
            .as_deref(),
        Some(if std::env::consts::OS == "macos" {
            "healthy"
        } else {
            "unhealthy"
        })
    );
}

#[tokio::test]
async fn listen_fails_when_the_database_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    let channel = channel_on(&dir.path().join("chat.db"), &["*"]);
    let (tx, _rx) = mpsc::channel(1);
    let error = ChannelAdapter::start(&channel, &IngressSender(tx))
        .await
        .unwrap_err()
        .message;
    assert!(error.contains("Messages database"), "{error}");
}

fn insert_message(conn: &Connection, rowid: i64, handle: i64, text: &str) {
    conn.execute(
        "INSERT INTO message (ROWID, handle_id, text, is_from_me, date) VALUES (?1, ?2, ?3, 0, ?4)",
        rusqlite::params![rowid, handle, text, 700_000_000_000_000_000_i64],
    )
    .unwrap();
}

/// Keeps adding one round of messages (a blocked sender, a blank text, then
/// an allowed one) until the listener delivers something, and returns it.
/// The listener only reports rows newer than its startup baseline, so the
/// rounds keep coming until one lands after it.
async fn next_delivery(
    writer: &Connection,
    rowid: &mut i64,
    rx: &mut mpsc::Receiver<ChannelIngress>,
) -> ChannelInboundEnvelope {
    let deadline = tokio::time::Instant::now() + HANG_GUARD;
    loop {
        for (handle, text) in [(2, "from a stranger"), (1, "   "), (1, "hello")] {
            *rowid += 1;
            insert_message(writer, *rowid, handle, text);
        }
        if let Ok(Some(ChannelIngress::Inbound(envelope))) =
            timeout(Duration::from_millis(200), rx.recv()).await
        {
            return envelope;
        }
        assert!(tokio::time::Instant::now() < deadline, "no delivery");
    }
}

#[tokio::test]
async fn listen_delivers_only_allowed_non_blank_messages() {
    let (_dir, db_path) = create_test_db();
    let writer = Connection::open(&db_path).unwrap();
    writer
        .execute_batch(
            "INSERT INTO handle (ROWID, id) VALUES (1, '+15555550123');
             INSERT INTO handle (ROWID, id) VALUES (2, '+19999999999');",
        )
        .unwrap();
    insert_message(&writer, 1, 1, "before startup");
    let mut rowid = 1;

    let channel = channel_on(&db_path, &["+15555550123"]);
    let (tx, mut rx) = mpsc::channel(8);
    let sink = IngressSender(tx);
    let listen = tokio::spawn(async move { ChannelAdapter::start(&channel, &sink).await });

    let first = next_delivery(&writer, &mut rowid, &mut rx).await;
    assert_eq!(first.sender.id, "+15555550123");
    assert_eq!(first.conversation.id, "+15555550123");
    assert_eq!(first.text, "hello");
    assert_eq!(first.channel.id, "imessage");
    assert_eq!(first.conversation.topic_id, None);
    assert_eq!(first.reply_to_id, None);
    assert_ne!(
        first.message_id, "1",
        "rows before startup are never replayed"
    );
    assert!(
        first
            .timestamp_unix_ms
            .is_some_and(|timestamp| timestamp > 0)
    );

    // The receiver going away ends the listener cleanly.
    drop(rx);
    for _ in 0..3 {
        rowid += 1;
        insert_message(&writer, rowid, 1, "after close");
    }
    let result = timeout(HANG_GUARD, listen)
        .await
        .expect("listen returns once the receiver is gone")
        .unwrap();
    assert!(
        result.is_err(),
        "a closed typed ingress sink stops the listener"
    );
}

/// A poll that fails (here: the `handle` table is missing) is logged and
/// retried rather than ending the listener.
#[tokio::test]
async fn listen_survives_failing_polls() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("chat.db");
    let writer = Connection::open(&db_path).unwrap();
    writer
        .execute_batch(
            "CREATE TABLE message (
                ROWID INTEGER PRIMARY KEY, guid TEXT, handle_id INTEGER, text TEXT,
                is_from_me INTEGER DEFAULT 0, date INTEGER, is_audio_message INTEGER DEFAULT 0,
                associated_message_guid TEXT, associated_message_type INTEGER DEFAULT 0,
                date_read INTEGER DEFAULT 0
            );",
        )
        .unwrap();

    let channel = channel_on(&db_path, &["*"]);
    let (tx, mut rx) = mpsc::channel(8);
    let sink = IngressSender(tx);
    let listen = tokio::spawn(async move { ChannelAdapter::start(&channel, &sink).await });

    // Give the listener time to fail a few polls; the assertions below hold
    // whether or not it did.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!listen.is_finished(), "a failing poll must not end listen");

    writer
        .execute_batch(
            "CREATE TABLE handle (ROWID INTEGER PRIMARY KEY, id TEXT NOT NULL);
             CREATE TABLE chat (ROWID INTEGER PRIMARY KEY, guid TEXT, style INTEGER);
             CREATE TABLE chat_message_join (chat_id INTEGER, message_id INTEGER);
             CREATE TABLE attachment (ROWID INTEGER PRIMARY KEY, filename TEXT, mime_type TEXT,
                transfer_name TEXT, total_bytes INTEGER);
             CREATE TABLE message_attachment_join (message_id INTEGER, attachment_id INTEGER);
             INSERT INTO handle (ROWID, id) VALUES (1, 'a@example.com');
             INSERT INTO handle (ROWID, id) VALUES (2, 'b@example.com');",
        )
        .unwrap();
    let mut rowid = 0;
    let delivered = next_delivery(&writer, &mut rowid, &mut rx).await;
    // With a wildcard allowlist the stranger is delivered too; blanks never are.
    assert!(!delivered.text.trim().is_empty());
    listen.abort();
}
