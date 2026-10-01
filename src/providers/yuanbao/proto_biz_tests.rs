use super::*;

fn text_body(s: &str) -> Vec<MsgBodyElement> {
    vec![MsgBodyElement {
        msg_type: "TIMTextElem".into(),
        msg_content: MsgContent {
            text: Some(s.into()),
            ..Default::default()
        },
    }]
}

#[test]
fn c2c_encode_smoke() {
    let buf = encode_send_c2c_message("uid_alice", "uid_bot", &text_body("hi"), "", 0, "", "");
    let frame = decode_conn_msg(&buf).unwrap();
    assert_eq!(frame.cmd, biz_cmd::SEND_C2C_MESSAGE);
    assert_eq!(frame.module, module::BIZ_PKG);
    assert!(!frame.data.is_empty());
}

#[test]
fn group_encode_smoke() {
    let buf = encode_send_group_message(
        "group_42",
        "uid_bot",
        &text_body("hello"),
        "",
        "",
        "rand",
        "",
        "",
    );
    let frame = decode_conn_msg(&buf).unwrap();
    assert_eq!(frame.cmd, biz_cmd::SEND_GROUP_MESSAGE);
}

#[test]
fn private_heartbeat_smoke() {
    let buf = encode_send_private_heartbeat("hb_1", "uid_bot", "uid_user", ws_heartbeat::RUNNING);
    let frame = decode_conn_msg(&buf).unwrap();
    assert_eq!(frame.cmd, biz_cmd::SEND_PRIVATE_HEARTBEAT);
    assert_eq!(frame.msg_id, "hb_1");
}

#[test]
fn query_group_info_roundtrip() {
    let buf = encode_query_group_info("qgi_1", "group_99");
    let frame = decode_conn_msg(&buf).unwrap();
    assert_eq!(frame.cmd, biz_cmd::QUERY_GROUP_INFO);
    assert_eq!(frame.msg_id, "qgi_1");

    // Simulate response payload: code=0, message="ok", group_name="g", owner=…
    let mut gi = Vec::new();
    put_string_field(1, "TestGroup", &mut gi);
    put_string_field(2, "owner_uid", &mut gi);
    put_string_field(3, "OwnerNick", &mut gi);
    put_varint_field(4, 42, &mut gi);
    let mut rsp = Vec::new();
    put_varint_field(1, 0, &mut rsp);
    put_string_field(2, "ok", &mut rsp);
    put_bytes_field(3, &gi, &mut rsp);

    let parsed = decode_query_group_info_rsp(&rsp).unwrap();
    assert_eq!(parsed.code, 0);
    assert_eq!(parsed.group_name, "TestGroup");
    assert_eq!(parsed.owner_id, "owner_uid");
    assert_eq!(parsed.member_count, 42);
}

// ─── encode_send_c2c branches ──────────────────────────────────

#[test]
fn c2c_encode_with_msg_id_msg_random_group_code_trace_id() {
    // Hit the branches: msg_id non-empty, msg_random != 0, group_code
    // non-empty, trace_id non-empty.
    let buf = encode_send_c2c_message(
        "uid_alice",
        "uid_bot",
        &text_body("hi"),
        "mid-1",
        42,
        "gcode-x",
        "trace-1",
    );
    let frame = decode_conn_msg(&buf).unwrap();
    assert_eq!(frame.cmd, biz_cmd::SEND_C2C_MESSAGE);
    assert_eq!(frame.msg_id, "mid-1");
    // Re-parse the biz body and check the fields we encoded show up.
    let f = parse_fields(&frame.data).unwrap();
    assert_eq!(get_string(&f, 1), "mid-1");
    assert_eq!(get_string(&f, 2), "uid_alice");
    assert_eq!(get_string(&f, 3), "uid_bot");
    assert_eq!(get_varint(&f, 4), 42);
    assert_eq!(get_string(&f, 6), "gcode-x");
    // log_ext (field 8) carries nested {1: trace_id}
    let log_ext = get_bytes(&f, 8);
    assert!(!log_ext.is_empty());
    let inner = parse_fields(&log_ext).unwrap();
    assert_eq!(get_string(&inner, 1), "trace-1");
}

#[test]
fn c2c_encode_generates_synthetic_req_id_when_msg_id_empty() {
    // msg_id empty branch — req_id falls back to `c2c_<seq>`.
    let buf = encode_send_c2c_message("uid_alice", "uid_bot", &text_body("hi"), "", 0, "", "");
    let frame = decode_conn_msg(&buf).unwrap();
    assert!(
        frame.msg_id.starts_with("c2c_"),
        "expected synthetic req_id starting with c2c_, got {}",
        frame.msg_id
    );
}

// ─── encode_send_group branches ────────────────────────────────

#[test]
fn group_encode_with_all_optional_fields() {
    let buf = encode_send_group_message(
        "group_42",
        "uid_bot",
        &text_body("hello"),
        "mid-g",
        "uid_to",
        "rand_x",
        "ref-msg-99",
        "trace-g",
    );
    let frame = decode_conn_msg(&buf).unwrap();
    assert_eq!(frame.cmd, biz_cmd::SEND_GROUP_MESSAGE);
    assert_eq!(frame.msg_id, "mid-g");
    let f = parse_fields(&frame.data).unwrap();
    assert_eq!(get_string(&f, 1), "mid-g");
    assert_eq!(get_string(&f, 2), "group_42");
    assert_eq!(get_string(&f, 3), "uid_bot");
    assert_eq!(get_string(&f, 4), "uid_to");
    assert_eq!(get_string(&f, 5), "rand_x");
    assert_eq!(get_string(&f, 7), "ref-msg-99");
    let log_ext = get_bytes(&f, 9);
    let inner = parse_fields(&log_ext).unwrap();
    assert_eq!(get_string(&inner, 1), "trace-g");
}

#[test]
fn group_encode_generates_synthetic_req_id_when_msg_id_empty() {
    let buf = encode_send_group_message("group_x", "uid_bot", &text_body("hi"), "", "", "", "", "");
    let frame = decode_conn_msg(&buf).unwrap();
    assert!(
        frame.msg_id.starts_with("grp_"),
        "expected synthetic req_id starting with grp_, got {}",
        frame.msg_id
    );
}

// ─── encode_send_group_heartbeat ───────────────────────────────

#[test]
fn group_heartbeat_encodes_send_time_and_heartbeat() {
    let buf = encode_send_group_heartbeat(
        "hb_g_1",
        "uid_bot",
        "group_42",
        ws_heartbeat::RUNNING,
        1_700_000_123,
    );
    let frame = decode_conn_msg(&buf).unwrap();
    assert_eq!(frame.cmd, biz_cmd::SEND_GROUP_HEARTBEAT);
    assert_eq!(frame.msg_id, "hb_g_1");
    let f = parse_fields(&frame.data).unwrap();
    assert_eq!(get_string(&f, 1), "uid_bot");
    assert_eq!(get_string(&f, 2), ""); // to_account empty for group
    assert_eq!(get_string(&f, 3), "group_42");
    assert_eq!(get_varint(&f, 4), 1_700_000_123);
    assert_eq!(get_varint(&f, 5), ws_heartbeat::RUNNING as u64);
}

// ─── encode_get_group_member_list ──────────────────────────────

#[test]
fn get_group_member_list_omits_offset_when_zero() {
    let buf = encode_get_group_member_list("qgm_1", "group_42", 0, 100);
    let frame = decode_conn_msg(&buf).unwrap();
    assert_eq!(frame.cmd, biz_cmd::GET_GROUP_MEMBER_LIST);
    let f = parse_fields(&frame.data).unwrap();
    assert_eq!(get_string(&f, 1), "group_42");
    // offset (field 2) skipped when 0
    assert_eq!(get_varint(&f, 2), 0);
    assert_eq!(get_varint(&f, 3), 100);
}

#[test]
fn get_group_member_list_includes_offset_when_nonzero() {
    let buf = encode_get_group_member_list("qgm_2", "group_42", 200, 50);
    let frame = decode_conn_msg(&buf).unwrap();
    let f = parse_fields(&frame.data).unwrap();
    assert_eq!(get_varint(&f, 2), 200);
    assert_eq!(get_varint(&f, 3), 50);
}

// ─── decode_biz_rsp_code + decode_response_envelope ────────────

#[test]
fn decode_biz_rsp_code_reads_code_and_message() {
    let mut buf = Vec::new();
    put_varint_field(1, 4002, &mut buf);
    put_string_field(2, "rate limited", &mut buf);
    let (code, msg) = decode_biz_rsp_code(&buf).unwrap();
    assert_eq!(code, 4002);
    assert_eq!(msg, "rate limited");
}

#[test]
fn decode_biz_rsp_code_on_empty_returns_defaults() {
    let (code, msg) = decode_biz_rsp_code(&[]).unwrap();
    assert_eq!(code, 0);
    assert!(msg.is_empty());
}

#[test]
fn decode_response_envelope_extracts_frame() {
    let original = encode_conn_msg(
        cmd_type::RESPONSE,
        biz_cmd::SEND_C2C_MESSAGE,
        1,
        "mid-r",
        module::BIZ_PKG,
        &[0xAA, 0xBB],
    );
    let frame = decode_response_envelope(&original).unwrap();
    assert_eq!(frame.cmd, biz_cmd::SEND_C2C_MESSAGE);
    assert_eq!(frame.msg_id, "mid-r");
    assert_eq!(frame.data, vec![0xAA, 0xBB]);
}

#[test]
fn group_member_list_decode() {
    let mut m1 = Vec::new();
    put_string_field(1, "uid_a", &mut m1);
    put_string_field(2, "Alice", &mut m1);
    put_varint_field(3, 2, &mut m1);
    let mut rsp = Vec::new();
    put_varint_field(1, 0, &mut rsp);
    put_string_field(2, "ok", &mut rsp);
    put_bytes_field(3, &m1, &mut rsp);
    put_varint_field(4, 100, &mut rsp);
    put_varint_field(5, 1, &mut rsp);

    let page = decode_get_group_member_list_rsp(&rsp).unwrap();
    assert_eq!(page.members.len(), 1);
    assert_eq!(page.members[0].user_id, "uid_a");
    assert_eq!(page.members[0].role, 2);
    assert_eq!(page.next_offset, 100);
    assert!(page.is_complete);
}

/// Adversarial input: a varint that overflows i32. The decoder must
/// surface `YuanbaoError::ProtoDecode` instead of silently truncating
/// (which would corrupt the `code` field returned to callers).
#[test]
fn decode_biz_rsp_code_rejects_varint_out_of_i32_range() {
    let mut buf = Vec::new();
    put_varint_field(1, u64::MAX, &mut buf);
    put_string_field(2, "ok", &mut buf);
    match decode_biz_rsp_code(&buf).unwrap_err() {
        YuanbaoError::ProtoDecode(m) => {
            assert!(
                m.contains("out of i32 range"),
                "expected i32 overflow message, got: {m}"
            );
        }
        other => panic!("expected ProtoDecode, got {other:?}"),
    }
}

/// Same guard applied to the group-member-list `next_offset` field —
/// an oversized varint must produce a structured decode error, not a
/// silent `as u32` wrap that would mis-paginate subsequent fetches.
#[test]
fn decode_group_member_list_rejects_varint_out_of_u32_range() {
    let mut rsp = Vec::new();
    put_varint_field(1, 0, &mut rsp);
    put_string_field(2, "ok", &mut rsp);
    put_varint_field(4, u64::from(u32::MAX) + 1, &mut rsp);
    match decode_get_group_member_list_rsp(&rsp).unwrap_err() {
        YuanbaoError::ProtoDecode(m) => {
            assert!(
                m.contains("out of u32 range"),
                "expected u32 overflow message, got: {m}"
            );
        }
        other => panic!("expected ProtoDecode, got {other:?}"),
    }
}
