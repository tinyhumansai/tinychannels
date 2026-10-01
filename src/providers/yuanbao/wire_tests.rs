use super::*;

#[test]
fn varint_roundtrip() {
    for &v in &[0u64, 1, 127, 128, 300, 16384, u32::MAX as u64, u64::MAX] {
        let mut buf = Vec::new();
        encode_varint(v, &mut buf);
        let (got, n) = decode_varint(&buf, 0).unwrap();
        assert_eq!(got, v, "varint roundtrip failed for {v}");
        assert_eq!(n, buf.len());
    }
}

#[test]
fn varint_truncated_errors() {
    let buf = vec![0x80, 0x80]; // continuation bit set but no end
    assert!(decode_varint(&buf, 0).is_err());
}

#[test]
fn field_roundtrip() {
    let mut buf = Vec::new();
    encode_field_varint(1, 42, &mut buf);
    encode_field_string(2, "hello", &mut buf);
    encode_field_bytes(3, b"\x00\x01\x02", &mut buf);

    let fields = parse_fields(&buf).unwrap();
    assert_eq!(get_varint(&fields, 1), 42);
    assert_eq!(get_string(&fields, 2), "hello");
    assert_eq!(get_bytes(&fields, 3), vec![0, 1, 2]);
}

#[test]
fn unknown_field_skipped_gracefully() {
    let mut buf = Vec::new();
    encode_field_varint(99, 123, &mut buf);
    encode_field_string(1, "wanted", &mut buf);
    let fields = parse_fields(&buf).unwrap();
    assert_eq!(get_string(&fields, 1), "wanted");
    assert_eq!(get_string(&fields, 2), ""); // missing field returns default
}

#[test]
fn seq_numbers_are_monotonic() {
    let a = next_seq_no();
    let b = next_seq_no();
    assert!(b > a);
}

#[test]
fn varint_too_long_errors() {
    // 11 continuation bytes — the 10th byte (0x80) triggers the overflow
    // guard before the loop even reaches the 11th.
    let buf = vec![0x80; 11];
    match decode_varint(&buf, 0).unwrap_err() {
        YuanbaoError::ProtoDecode(m) => {
            assert!(m.contains("too long") || m.contains("overflow"), "got {m}");
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn parse_fields_truncated_bytes_field_errors() {
    // Field 1 (wire type 2) declaring length 5 but only 1 byte of payload.
    let buf = vec![
        (1 << 3) | 2, // tag: field=1, wire=2
        5,            // claimed len
        b'a',
    ];
    match parse_fields(&buf).unwrap_err() {
        YuanbaoError::ProtoDecode(m) => assert!(m.contains("truncated"), "got {m}"),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn parse_fields_oversize_len_field_errors_without_panic() {
    // Field 1 (wire type 2) with a varint length encoding `u64::MAX` —
    // previously this would attempt `pos + len as usize`, overflowing
    // on 32-bit and slicing past the buffer on 64-bit. Now it must
    // return a structured decode error.
    let mut buf = Vec::new();
    buf.push((1 << 3) | 2); // tag: field=1, wire=2
    encode_varint(u64::MAX, &mut buf); // adversarial length
    match parse_fields(&buf).unwrap_err() {
        YuanbaoError::ProtoDecode(m) => {
            assert!(
                m.contains("too large") || m.contains("overflows") || m.contains("truncated"),
                "expected overflow/truncation error, got {m}"
            );
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn parse_fields_reads_fixed64() {
    let mut buf = Vec::new();
    buf.push((1 << 3) | 1); // tag: field=1, wire=1 (fixed64)
    buf.extend_from_slice(&0x1122_3344_5566_7788u64.to_le_bytes());
    let f = parse_fields(&buf).unwrap();
    match f[0].1 {
        FieldValue::Fixed64(v) => assert_eq!(v, 0x1122_3344_5566_7788),
        ref other => panic!("expected Fixed64 got {other:?}"),
    }
}

#[test]
fn parse_fields_truncated_fixed64_errors() {
    let mut buf = Vec::new();
    buf.push((1 << 3) | 1);
    buf.extend_from_slice(&[0u8; 4]); // only 4/8 bytes
    match parse_fields(&buf).unwrap_err() {
        YuanbaoError::ProtoDecode(m) => assert!(m.contains("fixed64"), "got {m}"),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn parse_fields_reads_fixed32() {
    let mut buf = Vec::new();
    buf.push((1 << 3) | 5); // tag: field=1, wire=5 (fixed32)
    buf.extend_from_slice(&0xCAFEBABEu32.to_le_bytes());
    let f = parse_fields(&buf).unwrap();
    match f[0].1 {
        FieldValue::Fixed32(v) => assert_eq!(v, 0xCAFEBABE),
        ref other => panic!("expected Fixed32 got {other:?}"),
    }
}

#[test]
fn parse_fields_truncated_fixed32_errors() {
    let mut buf = Vec::new();
    buf.push((1 << 3) | 5);
    buf.extend_from_slice(&[0u8; 2]); // only 2/4 bytes
    match parse_fields(&buf).unwrap_err() {
        YuanbaoError::ProtoDecode(m) => assert!(m.contains("fixed32"), "got {m}"),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn parse_fields_unsupported_wire_type_errors() {
    // wire type 3 (start group) is not supported.
    let buf = vec![(1 << 3) | 3];
    match parse_fields(&buf).unwrap_err() {
        YuanbaoError::ProtoDecode(m) => {
            assert!(m.contains("unsupported wire type 3"), "got {m}")
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn get_string_returns_empty_when_field_is_varint() {
    // Field 1 exists but encoded as varint, not bytes — get_string must
    // skip past it and return the default.
    let mut buf = Vec::new();
    encode_field_varint(1, 7, &mut buf);
    let fields = parse_fields(&buf).unwrap();
    assert_eq!(get_string(&fields, 1), "");
}

#[test]
fn get_varint_returns_zero_when_field_is_bytes() {
    let mut buf = Vec::new();
    encode_field_string(1, "not a varint", &mut buf);
    let fields = parse_fields(&buf).unwrap();
    assert_eq!(get_varint(&fields, 1), 0);
}

#[test]
fn get_bytes_returns_empty_when_field_is_varint() {
    let mut buf = Vec::new();
    encode_field_varint(1, 7, &mut buf);
    let fields = parse_fields(&buf).unwrap();
    assert!(get_bytes(&fields, 1).is_empty());
}

#[test]
fn get_repeated_bytes_collects_multiple_same_field() {
    let mut buf = Vec::new();
    encode_field_string(1, "a", &mut buf);
    encode_field_string(1, "bb", &mut buf);
    encode_field_string(2, "c", &mut buf); // different field — should be skipped
    encode_field_string(1, "ddd", &mut buf);
    let fields = parse_fields(&buf).unwrap();
    let got = get_repeated_bytes(&fields, 1);
    assert_eq!(got.len(), 3);
    assert_eq!(got[0], b"a");
    assert_eq!(got[1], b"bb");
    assert_eq!(got[2], b"ddd");
}
