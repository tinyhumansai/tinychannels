//! Business-layer protobuf codecs (biz payloads inside `ConnMsg.data`).
//!
//! Kept separate from `proto.rs` to stay under the 500-line ceiling and
//! to isolate the "openclaw biz protocol" surface from the lower-level
//! ConnMsg envelope.

use super::errors::YuanbaoError;
use super::proto::{decode_conn_msg, encode_conn_msg, encode_msg_body_element};
use super::proto_constants::*;
use super::types::*;
use super::wire::{
    encode_field_bytes as put_bytes_field, encode_field_string as put_string_field,
    encode_field_varint as put_varint_field, get_bytes, get_repeated_bytes, get_string, get_varint,
    next_seq_no, parse_fields,
};

// ─── SendC2CMessageReq ────────────────────────────────────────────
//
//   1: msg_id (string)         5: msg_body (repeated MsgBodyElement)
//   2: to_account              6: group_code (DM-from-group)
//   3: from_account            7: msg_seq
//   4: msg_random              8: log_ext

#[allow(clippy::too_many_arguments)]
fn encode_send_c2c_req(
    msg_id: &str,
    to_account: &str,
    from_account: &str,
    msg_random: u32,
    msg_body: &[MsgBodyElement],
    group_code: &str,
    msg_seq: Option<u64>,
    trace_id: &str,
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(128);
    if !msg_id.is_empty() {
        put_string_field(1, msg_id, &mut buf);
    }
    put_string_field(2, to_account, &mut buf);
    if !from_account.is_empty() {
        put_string_field(3, from_account, &mut buf);
    }
    if msg_random != 0 {
        put_varint_field(4, msg_random as u64, &mut buf);
    }
    for el in msg_body {
        let el_bytes = encode_msg_body_element(el);
        put_bytes_field(5, &el_bytes, &mut buf);
    }
    if !group_code.is_empty() {
        put_string_field(6, group_code, &mut buf);
    }
    if let Some(seq) = msg_seq {
        put_varint_field(7, seq, &mut buf);
    }
    if !trace_id.is_empty() {
        // log_ext is field 8 with a nested {1: trace_id}
        let mut log = Vec::new();
        put_string_field(1, trace_id, &mut log);
        put_bytes_field(8, &log, &mut buf);
    }
    buf
}

/// Encode a full C2C send request as a `ConnMsg` ready to send over WS.
pub fn encode_send_c2c_message(
    to_account: &str,
    from_account: &str,
    msg_body: &[MsgBodyElement],
    msg_id: &str,
    msg_random: u32,
    group_code: &str,
    trace_id: &str,
) -> Vec<u8> {
    let body = encode_send_c2c_req(
        msg_id,
        to_account,
        from_account,
        msg_random,
        msg_body,
        group_code,
        None,
        trace_id,
    );
    let req_id = if msg_id.is_empty() {
        format!("c2c_{}", next_seq_no())
    } else {
        msg_id.to_string()
    };
    encode_conn_msg(
        cmd_type::REQUEST,
        biz_cmd::SEND_C2C_MESSAGE,
        next_seq_no(),
        &req_id,
        module::BIZ_PKG,
        &body,
    )
}

// ─── SendGroupMessageReq ───────────────────────────────────────────
//
//   1: msg_id              5: random (string)
//   2: group_code          6: msg_body (repeated)
//   3: from_account        7: ref_msg_id
//   4: to_account          8: msg_seq
//                          9: log_ext

#[allow(clippy::too_many_arguments)]
fn encode_send_group_req(
    msg_id: &str,
    group_code: &str,
    from_account: &str,
    to_account: &str,
    random: &str,
    msg_body: &[MsgBodyElement],
    ref_msg_id: &str,
    trace_id: &str,
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(128);
    if !msg_id.is_empty() {
        put_string_field(1, msg_id, &mut buf);
    }
    put_string_field(2, group_code, &mut buf);
    if !from_account.is_empty() {
        put_string_field(3, from_account, &mut buf);
    }
    if !to_account.is_empty() {
        put_string_field(4, to_account, &mut buf);
    }
    if !random.is_empty() {
        put_string_field(5, random, &mut buf);
    }
    for el in msg_body {
        let el_bytes = encode_msg_body_element(el);
        put_bytes_field(6, &el_bytes, &mut buf);
    }
    if !ref_msg_id.is_empty() {
        put_string_field(7, ref_msg_id, &mut buf);
    }
    if !trace_id.is_empty() {
        let mut log = Vec::new();
        put_string_field(1, trace_id, &mut log);
        put_bytes_field(9, &log, &mut buf);
    }
    buf
}

#[allow(clippy::too_many_arguments)]
pub fn encode_send_group_message(
    group_code: &str,
    from_account: &str,
    msg_body: &[MsgBodyElement],
    msg_id: &str,
    to_account: &str,
    random: &str,
    ref_msg_id: &str,
    trace_id: &str,
) -> Vec<u8> {
    let body = encode_send_group_req(
        msg_id,
        group_code,
        from_account,
        to_account,
        random,
        msg_body,
        ref_msg_id,
        trace_id,
    );
    let req_id = if msg_id.is_empty() {
        format!("grp_{}", next_seq_no())
    } else {
        msg_id.to_string()
    };
    encode_conn_msg(
        cmd_type::REQUEST,
        biz_cmd::SEND_GROUP_MESSAGE,
        next_seq_no(),
        &req_id,
        module::BIZ_PKG,
        &body,
    )
}

// ─── Heartbeats ────────────────────────────────────────────────────

pub fn encode_send_private_heartbeat(
    req_id: &str,
    from_account: &str,
    to_account: &str,
    heartbeat: u32,
) -> Vec<u8> {
    let mut body = Vec::with_capacity(48);
    put_string_field(1, from_account, &mut body);
    put_string_field(2, to_account, &mut body);
    put_varint_field(3, heartbeat as u64, &mut body);
    encode_conn_msg(
        cmd_type::REQUEST,
        biz_cmd::SEND_PRIVATE_HEARTBEAT,
        next_seq_no(),
        req_id,
        module::BIZ_PKG,
        &body,
    )
}

pub fn encode_send_group_heartbeat(
    req_id: &str,
    from_account: &str,
    group_code: &str,
    heartbeat: u32,
    send_time_ms: u64,
) -> Vec<u8> {
    let mut body = Vec::with_capacity(64);
    put_string_field(1, from_account, &mut body);
    put_string_field(2, "", &mut body); // to_account empty for group
    put_string_field(3, group_code, &mut body);
    put_varint_field(4, send_time_ms, &mut body);
    put_varint_field(5, heartbeat as u64, &mut body);
    encode_conn_msg(
        cmd_type::REQUEST,
        biz_cmd::SEND_GROUP_HEARTBEAT,
        next_seq_no(),
        req_id,
        module::BIZ_PKG,
        &body,
    )
}

// ─── QueryGroupInfo ────────────────────────────────────────────────

pub fn encode_query_group_info(req_id: &str, group_code: &str) -> Vec<u8> {
    let mut body = Vec::with_capacity(16 + group_code.len());
    put_string_field(1, group_code, &mut body);
    encode_conn_msg(
        cmd_type::REQUEST,
        biz_cmd::QUERY_GROUP_INFO,
        next_seq_no(),
        req_id,
        module::BIZ_PKG,
        &body,
    )
}

/// Try to narrow a varint into a smaller integer type, returning
/// `YuanbaoError::ProtoDecode` (instead of silently truncating) when
/// the upstream value is out of range. Used to harden response decoders
/// against malformed / adversarial input.
fn varint_to_i32(value: u64, field_label: &str) -> Result<i32, YuanbaoError> {
    i32::try_from(value)
        .map_err(|_| YuanbaoError::ProtoDecode(format!("{field_label} out of i32 range: {value}")))
}

fn varint_to_u32(value: u64, field_label: &str) -> Result<u32, YuanbaoError> {
    u32::try_from(value)
        .map_err(|_| YuanbaoError::ProtoDecode(format!("{field_label} out of u32 range: {value}")))
}

pub fn decode_query_group_info_rsp(data: &[u8]) -> Result<GroupInfo, YuanbaoError> {
    let fields = parse_fields(data)?;
    let mut info = GroupInfo {
        code: varint_to_i32(get_varint(&fields, 1), "GroupInfoRsp.code")?,
        message: get_string(&fields, 2),
        ..Default::default()
    };
    let gi_bytes = get_bytes(&fields, 3);
    if !gi_bytes.is_empty() {
        let gi = parse_fields(&gi_bytes)?;
        info.group_name = get_string(&gi, 1);
        info.owner_id = get_string(&gi, 2);
        info.owner_nickname = get_string(&gi, 3);
        info.member_count = varint_to_u32(get_varint(&gi, 4), "GroupInfo.member_count")?;
    }
    Ok(info)
}

// ─── GetGroupMemberList ────────────────────────────────────────────

pub fn encode_get_group_member_list(
    req_id: &str,
    group_code: &str,
    offset: u32,
    limit: u32,
) -> Vec<u8> {
    let mut body = Vec::with_capacity(32 + group_code.len());
    put_string_field(1, group_code, &mut body);
    if offset != 0 {
        put_varint_field(2, offset as u64, &mut body);
    }
    put_varint_field(3, limit as u64, &mut body);
    encode_conn_msg(
        cmd_type::REQUEST,
        biz_cmd::GET_GROUP_MEMBER_LIST,
        next_seq_no(),
        req_id,
        module::BIZ_PKG,
        &body,
    )
}

pub fn decode_get_group_member_list_rsp(data: &[u8]) -> Result<GroupMemberListPage, YuanbaoError> {
    let fields = parse_fields(data)?;
    let mut members = Vec::new();
    for b in get_repeated_bytes(&fields, 3) {
        let m = parse_fields(&b)?;
        members.push(GroupMember {
            user_id: get_string(&m, 1),
            nickname: get_string(&m, 2),
            role: varint_to_u32(get_varint(&m, 3), "GroupMember.role")?,
            join_time: varint_to_u32(get_varint(&m, 4), "GroupMember.join_time")?,
            name_card: get_string(&m, 5),
        });
    }
    Ok(GroupMemberListPage {
        code: varint_to_i32(get_varint(&fields, 1), "GroupMemberListRsp.code")?,
        message: get_string(&fields, 2),
        members,
        next_offset: varint_to_u32(get_varint(&fields, 4), "GroupMemberListRsp.next_offset")?,
        is_complete: get_varint(&fields, 5) != 0,
    })
}

// ─── Generic biz response code helper ──────────────────────────────

/// Decode the `code` and `message` from a biz response.
///
/// All biz responses share the convention: field 1 = code, field 2 = message.
pub fn decode_biz_rsp_code(data: &[u8]) -> Result<(i32, String), YuanbaoError> {
    let fields = parse_fields(data)?;
    Ok((
        varint_to_i32(get_varint(&fields, 1), "BizRsp.code")?,
        get_string(&fields, 2),
    ))
}

/// Decode a `ConnMsg` and return the typed biz response code + frame for
/// the request/response correlator.
pub fn decode_response_envelope(frame_bytes: &[u8]) -> Result<ConnFrame, YuanbaoError> {
    decode_conn_msg(frame_bytes)
}

#[cfg(test)]
#[path = "proto_biz_tests.rs"]
mod tests;
