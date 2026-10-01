use super::*;
use crate::providers::yuanbao::proto::encode_conn_msg;
use crate::providers::yuanbao::wire::{
    encode_field_bytes, encode_field_string, encode_field_varint,
};

fn cfg() -> YuanbaoConfig {
    let mut c = YuanbaoConfig::default();
    c.app_key = "ak".into();
    c.ws_domain = "wss://example".into();
    c.token = "tok".into();
    c.bot_id = "bot1".into();
    c
}

pub fn auth_response_success_connect_id_for_test() -> Result<String, YuanbaoError> {
    let (tx, _rx) = mpsc::unbounded_channel();
    let conn = YuanbaoConnection::new(cfg(), tx, None);
    let mut body = Vec::new();
    encode_field_varint(1, 0, &mut body);
    encode_field_string(2, "ok", &mut body);
    encode_field_string(3, "connect-123", &mut body);
    let msg = Message::Binary(
        encode_conn_msg(
            cmd_type::RESPONSE,
            cmd::AUTH_BIND,
            1,
            "auth-1",
            module::CONN_ACCESS,
            &body,
        )
        .into(),
    );

    conn.handle_auth_response(&msg)?;
    Ok(conn.account().connect_id)
}

pub fn auth_response_rejects_status_for_test() -> String {
    let (tx, _rx) = mpsc::unbounded_channel();
    let conn = YuanbaoConnection::new(cfg(), tx, None);

    let mut head = Vec::new();
    encode_field_varint(1, cmd_type::RESPONSE as u64, &mut head);
    encode_field_string(2, cmd::AUTH_BIND, &mut head);
    encode_field_string(4, "auth-2", &mut head);
    encode_field_string(5, module::CONN_ACCESS, &mut head);
    encode_field_varint(10, 401, &mut head);

    let mut frame = Vec::new();
    encode_field_bytes(1, &head, &mut frame);
    let msg = Message::Binary(frame.into());

    match conn.handle_auth_response(&msg).unwrap_err() {
        YuanbaoError::AuthFailed(message) => message,
        other => format!("{other:?}"),
    }
}

pub async fn handle_binary_routes_builtin_and_push_frames_for_test() -> Vec<String> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let conn = YuanbaoConnection::new(cfg(), tx, None);

    conn.handle_binary(encode_conn_msg(
        cmd_type::RESPONSE,
        biz_cmd::QUERY_GROUP_INFO,
        1,
        "orphan-response",
        module::BIZ_PKG,
        b"response",
    ))
    .await;
    conn.handle_binary(encode_conn_msg(
        cmd_type::PUSH,
        cmd::UPDATE_META,
        2,
        "meta",
        module::CONN_ACCESS,
        b"ignored",
    ))
    .await;
    conn.handle_binary(encode_conn_msg(
        cmd_type::REQUEST,
        biz_cmd::SEND_C2C_MESSAGE,
        3,
        "request",
        module::BIZ_PKG,
        b"not-a-push",
    ))
    .await;
    conn.handle_binary(encode_conn_msg(
        cmd_type::PUSH,
        cmd::KICKOUT,
        4,
        "kick",
        module::CONN_ACCESS,
        b"logged out",
    ))
    .await;
    conn.handle_binary(encode_conn_msg(
        cmd_type::PUSH,
        "incoming-message",
        5,
        "push-1",
        module::BIZ_PKG,
        b"payload",
    ))
    .await;

    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        match event {
            InboundEvent::Kickout(reason) => events.push(format!("kickout:{reason}")),
            InboundEvent::Push(frame) => {
                events.push(format!("push:{}:{}", frame.cmd, frame.msg_id));
            }
        }
    }
    events
}
