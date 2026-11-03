use super::*;

#[test]
fn runtime_reservation_wire_preserves_the_module_supplied_opaque_paths() {
    let value = serde_json::json!({
        "handle": "opaque",
        "object_path": "/ai/tinyhumans/tinychannels/Channels/runtime_opaque",
        "host_object_path": "/ai/tinyhumans/tinychannels/ChannelsHost/runtime_opaque"
    });
    let reservation: ChannelRuntimeReservation = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(reservation).unwrap(), value);
}

#[test]
fn runtime_reservation_rejects_unrecognized_identity_fields() {
    assert!(
        serde_json::from_value::<ChannelRuntimeReservation>(serde_json::json!({
            "handle": "opaque", "object_path": "/module", "host_object_path": "/host",
            "user_id": "must-not-be-module-authority"
        }))
        .is_err()
    );
}

#[test]
fn runtime_reservation_validation_accepts_only_its_exact_private_paths() {
    const HANDLE: &str = "0123456789abcdef0123456789abcdef";
    let reservation = ChannelRuntimeReservation {
        handle: HANDLE.into(),
        object_path: format!("{}/runtime_{HANDLE}", crate::OBJECT_PATH),
        host_object_path: format!("{}/runtime_{HANDLE}", crate::HOST_OBJECT_PATH),
    };

    assert!(reservation.validate().is_ok());
}

#[test]
fn runtime_reservation_validation_rejects_each_bad_identity_component_sanitely() {
    const HANDLE: &str = "0123456789abcdef0123456789abcdef";
    let reservation =
        |handle: String, object_path: String, host_object_path: String| ChannelRuntimeReservation {
            handle,
            object_path,
            host_object_path,
        };
    let module_path = |handle: &str| format!("{}/runtime_{handle}", crate::OBJECT_PATH);
    let host_path = |handle: &str| format!("{}/runtime_{handle}", crate::HOST_OBJECT_PATH);
    let cases = [
        // Length, uppercase ASCII, and non-hex ASCII are independent handle failures;
        // each path still agrees with its handle so path validation does not mask them.
        reservation(
            HANDLE[..31].into(),
            module_path(&HANDLE[..31]),
            host_path(&HANDLE[..31]),
        ),
        {
            let handle = "0123456789ABCDEf0123456789abcdef";
            reservation(handle.into(), module_path(handle), host_path(handle))
        },
        {
            let handle = "g123456789abcdef0123456789abcdef";
            reservation(handle.into(), module_path(handle), host_path(handle))
        },
        // The handle is valid and each path mismatch is isolated.
        reservation(
            HANDLE.into(),
            format!("{}/runtime_object-path-supplied-marker", crate::OBJECT_PATH),
            host_path(HANDLE),
        ),
        reservation(
            HANDLE.into(),
            module_path(HANDLE),
            format!(
                "{}/runtime_host-path-supplied-marker",
                crate::HOST_OBJECT_PATH
            ),
        ),
    ];

    let mut sanitized_reason = None;
    for invalid in cases {
        let error = invalid
            .validate()
            .expect_err("invalid reservation identity must be rejected");
        assert_eq!(
            error,
            crate::TinyChannelsError::new("invalid channel runtime reservation")
        );
        let reason = error.to_string();
        assert!(!reason.contains(&invalid.handle));
        assert!(!reason.contains("object-path-supplied-marker"));
        assert!(!reason.contains("host-path-supplied-marker"));
        if let Some(expected) = &sanitized_reason {
            assert_eq!(
                &reason, expected,
                "all malformed identities share one closed reason"
            );
        } else {
            sanitized_reason = Some(reason);
        }
    }
}

#[test]
fn configured_catalog_and_message_preparation_keep_wire_vocabulary() {
    let catalog = serde_json::json!({
        "name": "linq", "ingress": "external_webhook",
        "interaction": {
            "capabilities": {
                "version": 2,
                "remoteControl": true, "chatApprovals": true,
                "progressiveEdits": false, "historyKeyIgnoresThread": false,
                "reactions": false,
                "account": {"proactiveTarget": null}
            }
        }
    });
    let typed: ConfiguredChannelInfo = serde_json::from_value(catalog.clone()).unwrap();
    assert_eq!(typed.ingress, ChannelIngressKind::ExternalWebhook);
    assert_eq!(typed.interaction.capabilities.version, 2);
    assert!(
        typed.interaction.capabilities.remote_control
            && typed.interaction.capabilities.chat_approvals
    );
    let message = serde_json::json!({"kind":"legacy","message":{"text":"fixture"}});
    let typed: ChannelMessagePreparation = serde_json::from_value(message.clone()).unwrap();
    assert_eq!(serde_json::to_value(typed).unwrap(), message);
}
