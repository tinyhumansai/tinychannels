//! Console channel: reads user lines from stdin and prints replies to stdout.
//!
//! Useful for local debugging and headless hosts. It has no network
//! dependency and is never part of a desktop UI.

use async_trait::async_trait;
use tokio::io::{self, AsyncBufRead, AsyncBufReadExt, BufReader};
use uuid::Uuid;

use tinychannels_bus::{Channel, ChannelMessage, SendMessage};

/// Console channel. `/quit` or `/exit` ends the listen loop.
#[derive(Debug, Default, Clone, Copy)]
pub struct CliChannel;

impl CliChannel {
    pub fn new() -> Self {
        Self
    }
}

/// Read lines from `reader` and forward each non-empty one as a
/// [`ChannelMessage`] until EOF, `/quit`/`/exit`, or the receiver closes.
async fn forward_lines<R: AsyncBufRead + Unpin>(
    reader: R,
    tx: &tokio::sync::mpsc::Sender<ChannelMessage>,
) {
    let mut lines = reader.lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        if line == "/quit" || line == "/exit" {
            break;
        }
        let msg = ChannelMessage {
            id: Uuid::new_v4().to_string(),
            sender: "user".to_string(),
            reply_target: "user".to_string(),
            content: line,
            channel: "cli".to_string(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            thread_ts: None,
            sender_name: None,
        };
        if tx.send(msg).await.is_err() {
            break;
        }
    }
}

#[async_trait]
impl Channel for CliChannel {
    fn name(&self) -> &str {
        "cli"
    }

    async fn send(&self, message: &SendMessage) -> anyhow::Result<()> {
        println!("{}", message.content);
        Ok(())
    }

    async fn listen(&self, tx: tokio::sync::mpsc::Sender<ChannelMessage>) -> anyhow::Result<()> {
        forward_lines(BufReader::new(io::stdin()), &tx).await;
        Ok(())
    }
}

#[cfg(test)]
#[path = "cli_tests.rs"]
mod tests;
