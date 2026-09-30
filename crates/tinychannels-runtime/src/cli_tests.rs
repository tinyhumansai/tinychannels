use super::*;

#[test]
fn cli_channel_name_and_capabilities() {
    let ch = CliChannel::new();
    assert_eq!(ch.name(), "cli");
    assert_eq!(
        ch.capabilities(),
        tinychannels_bus::ChannelCapabilities::NONE
    );
}

#[tokio::test]
async fn cli_channel_send_and_health_check_succeed() {
    let ch = CliChannel::new();
    assert!(ch.send(&SendMessage::new("hello", "user")).await.is_ok());
    assert!(ch.send(&SendMessage::new("", "")).await.is_ok());
    assert!(ch.health_check().await);
}

async fn collect(input: &str) -> Vec<ChannelMessage> {
    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    forward_lines(input.as_bytes(), &tx).await;
    drop(tx);
    let mut out = Vec::new();
    while let Some(msg) = rx.recv().await {
        out.push(msg);
    }
    out
}

#[tokio::test]
async fn forward_lines_skips_blank_lines_and_trims() {
    let msgs = collect("  hi  \n\n   \nthere\n").await;
    let contents: Vec<_> = msgs.iter().map(|m| m.content.as_str()).collect();
    assert_eq!(contents, ["hi", "there"]);
    for msg in &msgs {
        assert_eq!(msg.channel, "cli");
        assert_eq!(msg.sender, "user");
        assert_eq!(msg.reply_target, "user");
        assert!(msg.thread_ts.is_none());
    }
    assert_ne!(msgs[0].id, msgs[1].id);
}

#[tokio::test]
async fn forward_lines_stops_at_quit_or_exit() {
    assert_eq!(collect("a\n/quit\nb\n").await.len(), 1);
    assert_eq!(collect("a\n/exit\nb\n").await.len(), 1);
}

#[tokio::test]
async fn forward_lines_stops_when_receiver_closes() {
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    drop(rx);
    forward_lines("a\nb\n".as_bytes(), &tx).await;
}
