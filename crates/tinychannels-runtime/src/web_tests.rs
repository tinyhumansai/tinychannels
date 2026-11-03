use super::*;
use tinychannels_bus::module_progressive::ChannelOutputEvent;

#[test]
fn projection_preserves_wire_fields_and_monotonic_order() {
    let mut projector = WebTurnProjector::new("client", "thread", "request");
    for seq in 1..=2 {
        let events = projector.project(
            ChannelOutputEvent::TextDelta {
                text: "hello".into(),
                iteration: Some(2),
            },
            10,
        );
        assert_eq!(events.len(), 1);
        let wire = serde_json::to_value(&events[0]).unwrap();
        assert_eq!(wire["event"], "text_delta");
        assert_eq!(wire["request_id"], "request");
        assert_eq!(wire["round"], 2);
        assert_eq!(wire["seq"], seq);
        assert_eq!(wire["ts"], 10);
    }
}

#[test]
fn lifecycle_projection_keeps_the_frontend_event_names() {
    for (stage, name) in [
        ("inference_start", "inference_start"),
        ("inference_heartbeat", "inference_heartbeat"),
        ("iteration_start", "iteration_start"),
        ("context_compacting", "context_compacting"),
    ] {
        let events = WebTurnProjector::new("client", "thread", "request").project(
            ChannelOutputEvent::Lifecycle {
                stage: stage.into(),
                message: None,
                iteration: Some(1),
                max_iterations: None,
                stop: None,
            },
            10,
        );
        assert_eq!(events[0].event_name(), name);
    }
}

#[test]
fn tool_result_projection_preserves_failure_and_caps_the_output() {
    let output_size = tinychannels_bus::web::MAX_WEB_TOOL_PAYLOAD_BYTES + 100_000;
    let events = WebTurnProjector::new("client", "thread", "request").project(
        ChannelOutputEvent::ToolCallCompleted {
            call_id: "call".into(),
            tool_name: "shell".into(),
            success: false,
            output_chars: output_size,
            output: "x".repeat(output_size),
            arguments: None,
            elapsed_ms: 4,
            iteration: 1,
            failure: Some(tinychannels_bus::module_progressive::ToolFailureSummary {
                class: "policy".into(),
                category: "denied".into(),
                cause_plain: "denied".into(),
                next_action: "stop".into(),
                recoverable: false,
            }),
            display_label: None,
            display_detail: None,
            structured: None,
        },
        10,
    );
    let wire = serde_json::to_value(&events[0]).unwrap();
    assert_eq!(wire["failure"]["class"], "policy");
    assert!(
        wire["output"].as_str().unwrap().len() <= tinychannels_bus::web::MAX_WEB_TOOL_PAYLOAD_BYTES
    );
}

#[test]
fn compatibility_projector_retains_typed_parts_and_present_zero_and_false_values() {
    use tinychannels_bus::{ContentPart, OutboundContentPart};
    let mut projector = WebTurnProjector::new("client", "thread", "request");
    let parts = vec![OutboundContentPart::Content {
        part: ContentPart::Text {
            text: "typed final".into(),
            markdown_dialect: None,
        },
    }];
    let events = projector.project(
        ChannelOutputEvent::FinalMessage {
            text: "typed final".into(),
            parts: parts.clone(),
            receipt: None,
        },
        0,
    );
    let wire = serde_json::to_value(&events[0]).unwrap();
    assert_eq!(wire["parts"], serde_json::to_value(parts).unwrap());
    assert_eq!(wire["ts"], 0);

    let events = projector.project(
        ChannelOutputEvent::ToolCallCompleted {
            call_id: "call".into(),
            tool_name: "shell".into(),
            success: false,
            output_chars: 0,
            output: String::new(),
            arguments: Some(serde_json::Value::Null),
            elapsed_ms: 0,
            iteration: 0,
            failure: None,
            display_label: None,
            display_detail: None,
            structured: None,
        },
        0,
    );
    let wire = serde_json::to_value(&events[0]).unwrap();
    assert_eq!(wire["success"], false);
    assert_eq!(wire["elapsed_ms"], 0);
    assert_eq!(wire["round"], 0);
    assert_eq!(
        wire.get("args"),
        Some(&serde_json::Value::Null),
        "explicit null arguments remain distinct from omitted arguments"
    );
}
