use super::*;

fn text_elem(s: &str) -> MsgBodyElement {
    MsgBodyElement {
        msg_type: "TIMTextElem".into(),
        msg_content: MsgContent {
            text: Some(s.into()),
            ..Default::default()
        },
    }
}

fn image_elem(info_urls: &[&str], inline_url: Option<&str>) -> MsgBodyElement {
    MsgBodyElement {
        msg_type: "TIMImageElem".into(),
        msg_content: MsgContent {
            image_info_array: info_urls
                .iter()
                .map(|u| ImageInfo {
                    image_type: 1,
                    url: (*u).into(),
                    ..Default::default()
                })
                .collect(),
            url: inline_url.map(String::from),
            ..Default::default()
        },
    }
}

#[test]
fn dm_is_not_group() {
    let m = InboundMessage {
        from_account: "alice".into(),
        ..Default::default()
    };
    assert!(!m.is_group());
    assert_eq!(m.chat_id(), "alice");
}

#[test]
fn group_is_group_and_chat_id_is_group_code() {
    let m = InboundMessage {
        group_code: "grp_42".into(),
        from_account: "alice".into(),
        ..Default::default()
    };
    assert!(m.is_group());
    assert_eq!(m.chat_id(), "grp_42");
}

#[test]
fn is_recall_iff_recall_list_non_empty() {
    let mut m = InboundMessage::default();
    assert!(!m.is_recall());
    m.recall_msg_seq_list.push(ImMsgSeq {
        msg_seq: 7,
        msg_id: "x".into(),
    });
    assert!(m.is_recall());
}

#[test]
fn extract_text_concatenates_text_elements() {
    let m = InboundMessage {
        msg_body: vec![
            text_elem("hello"),
            text_elem("world"),
            image_elem(&[], None),
        ],
        ..Default::default()
    };
    assert_eq!(m.extract_text(), "hello\nworld");
}

#[test]
fn extract_text_ignores_text_none_and_non_text() {
    let m = InboundMessage {
        msg_body: vec![
            MsgBodyElement {
                msg_type: "TIMTextElem".into(),
                msg_content: MsgContent::default(), // text: None
            },
            image_elem(&["https://x/y.png"], None),
        ],
        ..Default::default()
    };
    assert_eq!(m.extract_text(), "");
}

#[test]
fn extract_text_on_empty_msg_body_returns_empty() {
    let m = InboundMessage::default();
    assert_eq!(m.extract_text(), "");
}

#[test]
fn extract_image_urls_from_image_info_array() {
    let m = InboundMessage {
        msg_body: vec![image_elem(&["https://a/1.png", "https://a/2.png"], None)],
        ..Default::default()
    };
    assert_eq!(
        m.extract_image_urls(),
        vec!["https://a/1.png".to_string(), "https://a/2.png".into()]
    );
}

#[test]
fn extract_image_urls_falls_back_to_inline_url_field() {
    let m = InboundMessage {
        msg_body: vec![image_elem(&[], Some("https://a/inline.png"))],
        ..Default::default()
    };
    assert_eq!(
        m.extract_image_urls(),
        vec!["https://a/inline.png".to_string()]
    );
}

#[test]
fn extract_image_urls_dedups_inline_when_already_in_info_array() {
    let dup = "https://a/dup.png";
    let m = InboundMessage {
        msg_body: vec![image_elem(&[dup], Some(dup))],
        ..Default::default()
    };
    assert_eq!(m.extract_image_urls(), vec![dup.to_string()]);
}

#[test]
fn extract_image_urls_skips_empty_url_in_info_array() {
    let m = InboundMessage {
        msg_body: vec![image_elem(&[""], None)],
        ..Default::default()
    };
    assert!(m.extract_image_urls().is_empty());
}

#[test]
fn extract_image_urls_ignores_text_elements() {
    let m = InboundMessage {
        msg_body: vec![text_elem("hi"), image_elem(&["https://a/1.png"], None)],
        ..Default::default()
    };
    assert_eq!(m.extract_image_urls(), vec!["https://a/1.png".to_string()]);
}

#[test]
fn source_reply_target_dm_is_raw_uid() {
    let s = Source {
        from_account: "uid_alice".into(),
        is_group: false,
        ..Default::default()
    };
    assert_eq!(s.reply_target(), "uid_alice");
}

#[test]
fn source_reply_target_group_uses_g_prefix() {
    let s = Source {
        group_code: "grp_42".into(),
        is_group: true,
        ..Default::default()
    };
    assert_eq!(s.reply_target(), "g:grp_42");
}

#[test]
fn message_kind_default_is_text() {
    assert_eq!(MessageKind::default(), MessageKind::Text);
}
