use super::*;

fn cfg(bot_id: &str) -> YuanbaoConfig {
    let mut c = YuanbaoConfig::default();
    c.app_key = "ak".into();
    c.ws_domain = "wss://x".into();
    c.token = "tok".into();
    c.bot_id = bot_id.into();
    c.bot_name = "bot".into();
    c.dm_access = "open".into();
    c.group_access = "open".into();
    c
}

fn ctx_with(msg: InboundMessage) -> PipelineCtx {
    PipelineCtx {
        msg,
        source: Source::default(),
        text: String::new(),
        image_urls: Vec::new(),
        is_at_bot: false,
        is_owner_command: false,
        kind: MessageKind::Text,
    }
}

#[tokio::test]
async fn dedup_skips_repeat() {
    let state = PipelineState::new(&cfg("bot1"), "bot1".into());
    let mw = DedupMw;
    let msg = InboundMessage {
        msg_id: "m1".into(),
        ..Default::default()
    };
    let mut c1 = ctx_with(msg.clone());
    assert!(matches!(
        mw.process(&state, &mut c1).await,
        MwResult::Continue
    ));
    let mut c2 = ctx_with(msg);
    assert!(matches!(
        mw.process(&state, &mut c2).await,
        MwResult::Skip(_)
    ));
}

#[tokio::test]
async fn access_guard_open() {
    let state = PipelineState::new(&cfg("bot1"), "bot1".into());
    let mw = AccessGuardMw;
    let mut c = ctx_with(InboundMessage {
        from_account: "alice".into(),
        ..Default::default()
    });
    c.source.is_group = false;
    c.source.from_account = "alice".into();
    assert!(matches!(
        mw.process(&state, &mut c).await,
        MwResult::Continue
    ));
}

#[tokio::test]
async fn full_dm_dispatch() {
    let mut config = cfg("bot1");
    config.group_at_required = false;
    let state = PipelineState::new(&config, "bot1".into());
    let pipeline = InboundPipeline::new(state);
    let msg = InboundMessage {
        from_account: "alice".into(),
        to_account: "bot1".into(),
        msg_id: "hi".into(),
        msg_body: vec![MsgBodyElement {
            msg_type: "TIMTextElem".into(),
            msg_content: MsgContent {
                text: Some("hello".into()),
                ..Default::default()
            },
        }],
        ..Default::default()
    };
    let body = crate::providers::yuanbao::proto::encode_msg_body_element(&msg.msg_body[0]);
    // Synthesize an InboundMessagePush from scratch:
    use crate::providers::yuanbao::proto;
    let mut buf = Vec::new();
    let put_str = |fnum: u32, s: &str, b: &mut Vec<u8>| {
        proto::encode_varint(((fnum as u64) << 3) | 2, b);
        proto::encode_varint(s.len() as u64, b);
        b.extend_from_slice(s.as_bytes());
    };
    put_str(2, &msg.from_account, &mut buf);
    put_str(3, &msg.to_account, &mut buf);
    put_str(12, &msg.msg_id, &mut buf);
    proto::encode_varint(((13u64) << 3) | 2, &mut buf);
    proto::encode_varint(body.len() as u64, &mut buf);
    buf.extend_from_slice(&body);

    let outcome = pipeline.process(&buf).await;
    assert!(
        matches!(outcome, PipelineOutcome::Dispatch(_)),
        "got {:?}",
        outcome
    );
}
