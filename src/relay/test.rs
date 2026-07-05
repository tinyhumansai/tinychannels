use crate::relay::{
    CONTRACT_VERSION, CapabilityDescriptor, DELIVERY_SIG_HEADER, DELIVERY_TS_HEADER,
    RelayDescriptorOptions, RelayPlatformEntry, delivery_payload, make_token_at,
    make_upgrade_token_at, sign, verify_delivery_signature_at, verify_signature, verify_token_at,
};

const SECRET: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
const CONNECTOR_TOKEN: &str = "Z3ctaW5zdGFuY2UtMTowOjM3YWE3YjE0NWU4NzY0ZDQwM2JhOWM2MzlmMjMwZGQ2M2RlOGVkOTliODhmZWQzNmFhMDI2MjVhOGE3ZTM1NjQ";
const CONNECTOR_BODY: &str =
    r#"{"type":"message","event":{"text":"hi","source":{"chat_id":"c1"}}}"#;
const CONNECTOR_TS: i64 = 1_750_000_000;
const CONNECTOR_SIG: &str = "ac9509c8dae52b5590f06378260877334ff1adc4b1c96bafa4b514165fae6dc6";

fn telegram_descriptor() -> CapabilityDescriptor {
    CapabilityDescriptor {
        contract_version: CONTRACT_VERSION,
        platform: "telegram".into(),
        label: "Telegram".into(),
        max_message_length: 4096,
        supports_draft_streaming: false,
        supports_edit: true,
        supports_threads: true,
        markdown_dialect: "markdown_v2".into(),
        len_unit: "utf16".into(),
        emoji: "✈️".into(),
        platform_hint: "You are on Telegram.".into(),
        pii_safe: false,
    }
}

#[test]
fn descriptor_roundtrips_json_and_ignores_unknown_keys() {
    let descriptor = telegram_descriptor();
    let json = descriptor.to_json().expect("descriptor json");
    assert_eq!(
        CapabilityDescriptor::from_json(&json).expect("descriptor parse"),
        descriptor
    );

    let raw = format!("{},\"future_field\":\"ignored\"}}", &json[..json.len() - 1]);
    assert_eq!(
        CapabilityDescriptor::from_json(&raw).expect("descriptor parse"),
        descriptor
    );
}

#[test]
fn descriptor_json_is_compact_and_sorted() {
    let json = telegram_descriptor().to_json().expect("descriptor json");
    assert!(json.starts_with(r#"{"contract_version":1,"emoji":"✈️","label":"Telegram""#));
    assert!(!json.contains(": "));
}

#[test]
fn descriptor_fills_optional_defaults() {
    let minimal = concat!(
        r#"{"contract_version":1,"platform":"x","label":"X","#,
        r#""max_message_length":2000,"supports_draft_streaming":false,"#,
        r#""supports_edit":false,"supports_threads":false,"#,
        r#""markdown_dialect":"plain","len_unit":"chars"}"#
    );
    let descriptor = CapabilityDescriptor::from_json(minimal).expect("descriptor parse");
    assert_eq!(descriptor.emoji, "🔌");
    assert_eq!(descriptor.platform_hint, "");
    assert!(!descriptor.pii_safe);
}

#[test]
fn descriptor_projects_platform_entry_fields() {
    let entry = RelayPlatformEntry {
        name: "telegram".into(),
        label: "Telegram".into(),
        max_message_length: 0,
        emoji: Some("✈️".into()),
        platform_hint: Some("You are on Telegram.".into()),
        pii_safe: false,
    };
    let descriptor = CapabilityDescriptor::from_platform_entry(
        &entry,
        RelayDescriptorOptions {
            len_unit: "utf16".into(),
            supports_draft_streaming: true,
            supports_edit: false,
            supports_threads: true,
            markdown_dialect: "discord".into(),
        },
    );

    assert_eq!(descriptor.contract_version, CONTRACT_VERSION);
    assert_eq!(descriptor.max_message_length, 4096);
    assert_eq!(descriptor.emoji, "✈️");
    assert_eq!(descriptor.platform_hint, "You are on Telegram.");
    assert_eq!(descriptor.len_unit, "utf16");
    assert!(descriptor.supports_draft_streaming);
    assert!(!descriptor.supports_edit);
    assert_eq!(descriptor.markdown_dialect, "discord");
}

#[test]
fn token_roundtrips_and_payload_may_contain_colons() {
    let payload = "agent:main:discord:group:chanA";
    let token = make_token_at(payload, SECRET, 0, 1_700_000_000);
    assert_eq!(
        verify_token_at(&token, &[SECRET], 1_700_000_000),
        Some(payload.into())
    );
}

#[test]
fn upgrade_token_is_token_of_gateway_id() {
    assert_eq!(
        make_upgrade_token_at("gw-1", SECRET, 0, 1_700_000_000),
        make_token_at("gw-1", SECRET, 0, 1_700_000_000)
    );
}

#[test]
fn token_rejects_wrong_secret_expiry_and_garbage() {
    let token = make_token_at("p", SECRET, 0, 1_700_000_000);
    assert_eq!(verify_token_at(&token, &["deadbeef"], 1_700_000_000), None);

    let expired = make_token_at("p", SECRET, 1, 1);
    assert_eq!(verify_token_at(&expired, &[SECRET], 1_700_000_000), None);
    assert_eq!(
        verify_token_at("not-base64url!!!", &[SECRET], 1_700_000_000),
        None
    );
}

#[test]
fn token_rotation_verify_list_accepts_secondary_secret() {
    let old = SECRET;
    let new = "ffeeddccbbaa99887766554433221100ffeeddccbbaa99887766554433221100";
    let token = make_token_at("p", old, 0, 1_700_000_000);
    assert_eq!(
        verify_token_at(&token, &[new, old], 1_700_000_000),
        Some("p".into())
    );
    assert_eq!(verify_token_at(&token, &[new], 1_700_000_000), None);
}

#[test]
fn signature_verifies_against_rotation_list() {
    let payload = "1700000000.body";
    let signature = sign(payload, SECRET);
    assert!(verify_signature(payload, &signature, &["wrong", SECRET]));
    assert!(!verify_signature(payload, &signature, &["wrong"]));
    assert!(!verify_signature(payload, "zz", &[SECRET]));
}

#[test]
fn delivery_signature_accepts_valid_and_rejects_tamper_or_skew() {
    let body = r#"{"type":"message","event":{"text":"x"}}"#;
    let ts = 1_700_000_000;
    let signature = sign(&delivery_payload(ts, body), SECRET);

    assert!(verify_delivery_signature_at(
        body,
        Some(&ts.to_string()),
        Some(&signature),
        &[SECRET],
        300,
        ts
    ));
    assert!(!verify_delivery_signature_at(
        &format!("{body} "),
        Some(&ts.to_string()),
        Some(&signature),
        &[SECRET],
        300,
        ts
    ));
    assert!(!verify_delivery_signature_at(
        body,
        Some(&ts.to_string()),
        Some(&signature),
        &[SECRET],
        300,
        ts + 301
    ));
    assert!(verify_delivery_signature_at(
        body,
        Some(&ts.to_string()),
        Some(&signature),
        &[SECRET],
        300,
        ts + 299
    ));
}

#[test]
fn delivery_headers_match_connector_names() {
    assert_eq!(DELIVERY_TS_HEADER, "x-relay-timestamp");
    assert_eq!(DELIVERY_SIG_HEADER, "x-relay-signature");
}

#[test]
fn frozen_connector_vectors_match_hermes_tests() {
    assert_eq!(
        make_token_at("gw-instance-1", SECRET, 0, 1_700_000_000),
        CONNECTOR_TOKEN
    );
    assert_eq!(
        verify_token_at(CONNECTOR_TOKEN, &[SECRET], 1_700_000_000),
        Some("gw-instance-1".into())
    );
    assert_eq!(
        sign(&delivery_payload(CONNECTOR_TS, CONNECTOR_BODY), SECRET),
        CONNECTOR_SIG
    );
    assert!(verify_delivery_signature_at(
        CONNECTOR_BODY,
        Some(&CONNECTOR_TS.to_string()),
        Some(CONNECTOR_SIG),
        &[SECRET],
        300,
        CONNECTOR_TS
    ));
}
