use super::*;

#[test]
fn conn_msg_roundtrip() {
    let buf = encode_conn_msg(
        cmd_type::REQUEST,
        cmd::PING,
        42,
        "mid-1",
        module::CONN_ACCESS,
        b"payload",
    );
    let frame = decode_conn_msg(&buf).unwrap();
    assert_eq!(frame.cmd_type, cmd_type::REQUEST);
    assert_eq!(frame.cmd, cmd::PING);
    assert_eq!(frame.seq_no, 42);
    assert_eq!(frame.msg_id, "mid-1");
    assert_eq!(frame.module, module::CONN_ACCESS);
    assert_eq!(frame.data, b"payload");
}

/// Smoke-test that [`encode_auth_bind`] produces a frame round-trippable
/// via [`decode_conn_msg`] and that the `app_version` / `bot_version`
/// arguments land in the expected `DeviceInfo` fields (regression guard
/// for the plugin_version/bot_version swap).
#[test]
fn auth_bind_smoke() {
    let buf = encode_auth_bind(
        "biz", "uid", "openclaw", "tok", "mid", "0.1.0", "linux", "1.0", "",
    );
    let frame = decode_conn_msg(&buf).unwrap();
    assert_eq!(frame.cmd, cmd::AUTH_BIND);
    assert_eq!(frame.module, module::CONN_ACCESS);
    assert!(!frame.data.is_empty());

    let req_fields = parse_fields(&frame.data).unwrap();
    let dev_buf = get_bytes(&req_fields, 3);
    let dev_fields = parse_fields(&dev_buf).unwrap();
    assert_eq!(get_string(&dev_fields, 1), "0.1.0", "app_version");
    assert_eq!(get_string(&dev_fields, 24), "1.0", "bot_version");
}

#[test]
fn push_ack_mirrors_original() {
    let original = ConnFrame {
        cmd_type: cmd_type::PUSH,
        cmd: "some_push".into(),
        module: "yuanbao_openclaw_proxy".into(),
        seq_no: 99,
        msg_id: "mid-abc".into(),
        need_ack: true,
        status: 0,
        data: vec![],
    };
    let buf = encode_push_ack(&original);
    let frame = decode_conn_msg(&buf).unwrap();
    assert_eq!(frame.cmd_type, cmd_type::PUSH_ACK);
    assert_eq!(frame.cmd, original.cmd);
    assert_eq!(frame.module, original.module);
    assert_eq!(frame.msg_id, original.msg_id);
}

#[test]
fn msg_body_element_roundtrip() {
    let el = MsgBodyElement {
        msg_type: "TIMTextElem".into(),
        msg_content: MsgContent {
            text: Some("hello 元宝".into()),
            ..Default::default()
        },
    };
    let buf = encode_msg_body_element(&el);
    let got = decode_msg_body_element(&buf).unwrap();
    assert_eq!(got, el);
}

#[test]
fn image_element_roundtrip() {
    let el = MsgBodyElement {
        msg_type: "TIMImageElem".into(),
        msg_content: MsgContent {
            uuid: Some("abc123".into()),
            image_format: Some(3),
            image_info_array: vec![ImageInfo {
                image_type: 1,
                size: 1024,
                width: 800,
                height: 600,
                url: "https://example/img.png".into(),
            }],
            ..Default::default()
        },
    };
    let buf = encode_msg_body_element(&el);
    let got = decode_msg_body_element(&buf).unwrap();
    assert_eq!(got, el);
}

// ─── decode_auth_bind_rsp ─────────────────────────────────────

fn build_auth_bind_rsp_bytes(code: u64, message: &str, connect_id: &str) -> Vec<u8> {
    let mut buf = Vec::new();
    if code != 0 {
        encode_field_varint(1, code, &mut buf);
    }
    if !message.is_empty() {
        encode_field_string(2, message, &mut buf);
    }
    if !connect_id.is_empty() {
        encode_field_string(3, connect_id, &mut buf);
    }
    buf
}

#[test]
fn decode_auth_bind_rsp_happy_path() {
    let body = build_auth_bind_rsp_bytes(0, "ok", "conn-42");
    let r = decode_auth_bind_rsp(&body).unwrap();
    assert_eq!(r.code, 0);
    assert_eq!(r.message, "ok");
    assert_eq!(r.connect_id, "conn-42");
}

#[test]
fn decode_auth_bind_rsp_with_error_code() {
    let body = build_auth_bind_rsp_bytes(40011, "rejected", "");
    let r = decode_auth_bind_rsp(&body).unwrap();
    assert_eq!(r.code, 40011);
    assert_eq!(r.message, "rejected");
    assert!(r.connect_id.is_empty());
}

#[test]
fn decode_auth_bind_rsp_on_empty_returns_default() {
    let r = decode_auth_bind_rsp(&[]).unwrap();
    assert_eq!(r, AuthBindRsp::default());
}

// ─── decode_push_msg ──────────────────────────────────────────

#[test]
fn decode_push_msg_extracts_all_fields() {
    let inner_payload = vec![0xCA, 0xFE, 0xBA, 0xBE];
    let mut buf = Vec::new();
    encode_field_string(1, "inbound_message", &mut buf);
    encode_field_string(2, "yuanbao_openclaw_proxy", &mut buf);
    encode_field_string(3, "pm-1", &mut buf);
    encode_field_bytes(4, &inner_payload, &mut buf);

    let pm = decode_push_msg(&buf).unwrap();
    assert_eq!(pm.cmd, "inbound_message");
    assert_eq!(pm.module, "yuanbao_openclaw_proxy");
    assert_eq!(pm.msg_id, "pm-1");
    assert_eq!(pm.data, inner_payload);
}

#[test]
fn decode_push_msg_on_empty_returns_defaults() {
    let pm = decode_push_msg(&[]).unwrap();
    assert!(pm.cmd.is_empty());
    assert!(pm.module.is_empty());
    assert!(pm.msg_id.is_empty());
    assert!(pm.data.is_empty());
}

// ─── decode_inbound_push (protobuf) ───────────────────────────

#[test]
fn decode_inbound_push_dm_with_text_body() {
    // Build a minimal DM push: from/to/sender_nickname + one TIMTextElem.
    let text_elem = MsgBodyElement {
        msg_type: "TIMTextElem".into(),
        msg_content: MsgContent {
            text: Some("hello".into()),
            ..Default::default()
        },
    };
    let elem_bytes = encode_msg_body_element(&text_elem);

    let mut log_ext = Vec::new();
    encode_field_string(1, "trace-123", &mut log_ext);

    let mut buf = Vec::new();
    encode_field_string(1, "C2CMsg", &mut buf);
    encode_field_string(2, "user_42", &mut buf);
    encode_field_string(3, "bot_1", &mut buf);
    encode_field_string(4, "Alice", &mut buf);
    encode_field_varint(8, 7, &mut buf);
    encode_field_varint(9, 123, &mut buf);
    encode_field_varint(10, 1_700_000_000, &mut buf);
    encode_field_string(12, "mid-abc", &mut buf);
    encode_field_bytes(13, &elem_bytes, &mut buf);
    encode_field_varint(15, 1_700_000_001, &mut buf);
    encode_field_bytes(20, &log_ext, &mut buf);

    let m = decode_inbound_push(&buf).unwrap();
    assert_eq!(m.callback_command, "C2CMsg");
    assert_eq!(m.from_account, "user_42");
    assert_eq!(m.to_account, "bot_1");
    assert_eq!(m.sender_nickname, "Alice");
    assert_eq!(m.msg_seq, 7);
    assert_eq!(m.msg_random, 123);
    assert_eq!(m.msg_time, 1_700_000_000);
    assert_eq!(m.msg_id, "mid-abc");
    assert_eq!(m.event_time, 1_700_000_001);
    assert_eq!(m.trace_id, "trace-123");
    assert_eq!(m.msg_body.len(), 1);
    assert_eq!(m.msg_body[0].msg_content.text.as_deref(), Some("hello"));
    assert!(m.recall_msg_seq_list.is_empty());
}

#[test]
fn decode_inbound_push_group_with_recall_list() {
    let mut recall_entry = Vec::new();
    encode_field_varint(1, 99, &mut recall_entry);
    encode_field_string(2, "old-msg-id", &mut recall_entry);

    let mut buf = Vec::new();
    encode_field_string(1, "GroupSysMsg", &mut buf);
    encode_field_string(5, "gid-x", &mut buf);
    encode_field_string(6, "gcode-y", &mut buf);
    encode_field_string(7, "Room", &mut buf);
    encode_field_bytes(17, &recall_entry, &mut buf);
    encode_field_string(19, "g-private-code", &mut buf);

    let m = decode_inbound_push(&buf).unwrap();
    assert_eq!(m.callback_command, "GroupSysMsg");
    assert_eq!(m.group_id, "gid-x");
    assert_eq!(m.group_code, "gcode-y");
    assert_eq!(m.group_name, "Room");
    assert_eq!(m.private_from_group_code, "g-private-code");
    assert_eq!(m.recall_msg_seq_list.len(), 1);
    assert_eq!(m.recall_msg_seq_list[0].msg_seq, 99);
    assert_eq!(m.recall_msg_seq_list[0].msg_id, "old-msg-id");
    assert!(m.trace_id.is_empty(), "no log_ext => empty trace_id");
}

// ─── decode_inbound_json ──────────────────────────────────────

#[test]
fn decode_inbound_json_full_dm_shape() {
    let json = serde_json::json!({
        "callback_command": "C2CMsg",
        "from_account": "user_42",
        "to_account": "bot_1",
        "sender_nickname": "Alice",
        "msg_seq": 7,
        "msg_random": 123,
        "msg_time": 1_700_000_000,
        "msg_id": "mid-1",
        "msg_body": [
            {
                "msg_type": "TIMTextElem",
                "msg_content": { "text": "hi" }
            },
            {
                "msg_type": "TIMImageElem",
                "msg_content": {
                    "uuid": "u-1",
                    "image_format": 1,
                    "image_info_array": [
                        { "type": 1, "size": 100, "width": 10, "height": 20, "url": "https://x/i.png" }
                    ]
                }
            }
        ],
        "recall_msg_seq_list": [{ "msg_seq": 9, "msg_id": "old" }],
        "log_ext": { "trace_id": "trace-json" }
    });
    let m = decode_inbound_json(json.to_string().as_bytes()).unwrap();
    assert_eq!(m.callback_command, "C2CMsg");
    assert_eq!(m.from_account, "user_42");
    assert_eq!(m.msg_id, "mid-1");
    assert_eq!(m.msg_body.len(), 2);
    assert_eq!(m.msg_body[0].msg_content.text.as_deref(), Some("hi"));
    let img = &m.msg_body[1].msg_content;
    assert_eq!(img.uuid.as_deref(), Some("u-1"));
    assert_eq!(img.image_info_array.len(), 1);
    assert_eq!(img.image_info_array[0].url, "https://x/i.png");
    assert_eq!(m.recall_msg_seq_list.len(), 1);
    assert_eq!(m.recall_msg_seq_list[0].msg_seq, 9);
    assert_eq!(m.trace_id, "trace-json");
}

#[test]
fn decode_inbound_json_rejects_non_object_root() {
    let err = decode_inbound_json(b"[1,2,3]").unwrap_err();
    match err {
        YuanbaoError::ProtoDecode(m) => assert!(m.contains("not an object"), "got {m}"),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn decode_inbound_json_rejects_invalid_json() {
    let err = decode_inbound_json(b"not json").unwrap_err();
    match err {
        YuanbaoError::ProtoDecode(m) => assert!(m.contains("json parse"), "got {m}"),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn decode_msg_body_element_json_handles_image_type_alias() {
    // Some payloads use `image_type` (snake_case) instead of `type`.
    let v = serde_json::json!({
        "msg_type": "TIMImageElem",
        "msg_content": {
            "image_info_array": [
                { "image_type": 2, "size": 50, "width": 5, "height": 6, "url": "u" }
            ]
        }
    });
    let el = decode_msg_body_element_json(&v);
    assert_eq!(el.msg_type, "TIMImageElem");
    assert_eq!(el.msg_content.image_info_array.len(), 1);
    assert_eq!(el.msg_content.image_info_array[0].image_type, 2);
}

#[test]
fn decode_msg_content_image_info_with_only_image_type_zero_defaults_to_one() {
    // When `image_type` is 0 but url is present, decoder bumps to 1.
    let mut ib = Vec::new();
    encode_field_varint(2, 64, &mut ib);
    encode_field_string(5, "https://x/y.png", &mut ib);
    let mut content = Vec::new();
    encode_field_bytes(8, &ib, &mut content);
    let mut elem = Vec::new();
    encode_field_string(1, "TIMImageElem", &mut elem);
    encode_field_bytes(2, &content, &mut elem);
    let got = decode_msg_body_element(&elem).unwrap();
    assert_eq!(got.msg_content.image_info_array.len(), 1);
    assert_eq!(got.msg_content.image_info_array[0].image_type, 1);
    assert_eq!(got.msg_content.image_info_array[0].url, "https://x/y.png");
}
