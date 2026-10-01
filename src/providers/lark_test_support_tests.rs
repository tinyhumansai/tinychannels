//! Debug-build helpers for raw integration tests. These expose pure parser
//! seams without widening the production API surface.

use super::*;
use prost::Message as ProstMessage;
use tokio_tungstenite::tungstenite::Message as WsMsg;

pub fn parse_post_content_for_test(content: &str) -> Option<String> {
    parse_post_content(content)
}

pub fn strip_at_placeholders_for_test(text: &str) -> String {
    strip_at_placeholders(text)
}

pub fn should_respond_in_group_for_test(mentions: &[serde_json::Value]) -> bool {
    should_respond_in_group(mentions)
}

pub fn should_refresh_last_recv_for_test(msg: &WsMsg) -> bool {
    should_refresh_last_recv(msg)
}

pub fn endpoint_response_for_test(raw: &str) -> anyhow::Result<(String, Option<u64>)> {
    let resp = serde_json::from_str::<WsEndpointResp>(raw)?;
    if resp.code != 0 {
        anyhow::bail!(
            "Lark WS endpoint failed: code={} msg={}",
            resp.code,
            resp.msg.as_deref().unwrap_or("(none)")
        );
    }
    let ep = resp
        .data
        .ok_or_else(|| anyhow::anyhow!("Lark WS endpoint: empty data"))?;
    Ok((ep.url, ep.client_config.and_then(|cfg| cfg.ping_interval)))
}

pub fn encode_frame_for_test(
    seq_id: u64,
    method: i32,
    frame_type: &str,
    payload: Option<Vec<u8>>,
) -> Vec<u8> {
    PbFrame {
        seq_id,
        log_id: 0,
        service: 7,
        method,
        headers: vec![PbHeader {
            key: "type".to_string(),
            value: frame_type.to_string(),
        }],
        payload,
    }
    .encode_to_vec()
}

pub fn decode_frame_for_test(raw: &[u8]) -> anyhow::Result<(u64, i32, String, Option<Vec<u8>>)> {
    let frame = PbFrame::decode(raw)?;
    Ok((
        frame.seq_id,
        frame.method,
        frame.header_value("type").to_string(),
        frame.payload,
    ))
}

pub fn endpoint_urls_for_test(use_feishu: bool) -> (String, String) {
    let mut channel = LarkChannel::new(
        "app".to_string(),
        "secret".to_string(),
        String::new(),
        None,
        Vec::new(),
    );
    channel.use_feishu = use_feishu;
    (
        channel.tenant_access_token_url(),
        channel.send_message_url(),
    )
}
