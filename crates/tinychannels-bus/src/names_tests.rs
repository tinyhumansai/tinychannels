use super::{
    BUS_NAME, HOST_BUS_NAME, HOST_METHODS, HOST_OBJECT_PATH, METHODS, MODULE_ID, OBJECT_PATH,
    host_methods, methods,
};

/// `METHODS` is what a host iterates to build its interface, so a member
/// added to `methods` but forgotten here is a method the module serves and
/// no host ever binds — a `MemberNotFound` in the field, not a build error.
#[test]
fn every_declared_member_appears_in_its_method_table() {
    for m in [
        methods::START_CHANNEL,
        methods::STOP_CHANNEL,
        methods::SEND_MESSAGE,
        methods::LIST_CHANNELS,
        methods::CHANNEL_STATUS,
        methods::RESERVE_RELAY,
        methods::START_RELAY,
        methods::START_RELAY_V2,
        methods::STOP_RELAY,
        methods::RELAY_STATUS,
        methods::RELAY_CAPABILITIES_V2,
        methods::SEND_RELAY_INTENT,
        methods::SHUTDOWN,
        methods::SEND_CHANNEL_INTENT,
        methods::SEND_EMAIL,
        methods::BEGIN_EMAIL_UPLOAD,
        methods::APPEND_EMAIL_UPLOAD_CHUNK,
        methods::SEND_EMAIL_UPLOAD,
        methods::ABORT_EMAIL_UPLOAD,
        methods::START_CHANNEL_CONFIGURED,
        methods::CHANNEL_CONTROLLER,
        methods::CHANNEL_INTERACTION,
        methods::CHECK_CHANNEL_HEALTH,
        methods::START_CHANNEL_SUPERVISED,
        methods::RESERVE_RUNTIME,
        methods::START_RUNTIME,
        methods::STOP_RUNTIME,
        methods::DESCRIBE_CONFIGURED_CHANNELS,
        methods::START_OUTBOUND_CHANNEL,
        methods::PREPARE_OUTBOUND_INTENT,
        methods::PREPARE_CHANNEL_INBOUND,
        methods::PREPARE_CHANNEL_IDENTITY,
        methods::EXECUTE_PROGRESSIVE,
        methods::EXECUTE_REMOTE,
        methods::PREPARE_CHANNEL_CONTROL,
        methods::START_RUNTIME_WITH_DISPATCH,
        methods::PREPARE_RUNTIME_COMMAND,
        methods::BEGIN_MEDIA_UPLOAD,
        methods::APPEND_MEDIA_UPLOAD_CHUNK,
        methods::FINISH_MEDIA_UPLOAD,
        methods::ABORT_MEDIA_UPLOAD,
        methods::BEGIN_MEDIA_READ,
        methods::READ_MEDIA_CHUNK,
        methods::ABORT_MEDIA_READ,
        methods::BEGIN_WEB_TURN,
        methods::PROJECT_WEB_OUTPUT,
        methods::END_WEB_TURN,
    ] {
        assert!(METHODS.contains(&m), "{m} missing from METHODS");
    }
    for m in [
        host_methods::DELIVER_INBOUND,
        host_methods::REPORT_STATUS,
        host_methods::PERSIST_ALLOWED_IDENTITY,
        host_methods::TRANSCRIBE,
        host_methods::TRANSCRIBE_VOICE,
        host_methods::SYNTHESIZE_SPEECH,
        host_methods::STORE_VOICE_MEDIA,
        host_methods::READ_VOICE_MEDIA,
        host_methods::GET_VOICE_REPLY_POLICY,
        host_methods::PUBLISH_EVENT,
        host_methods::DISPATCH_CONTROLLER_BACKEND,
        host_methods::PROCESS_INBOUND,
        host_methods::DELIVER_EVENT,
    ] {
        assert!(HOST_METHODS.contains(&m), "{m} missing from HOST_METHODS");
    }
}

#[test]
fn additive_method_table_preserves_legacy_order_and_lists_all_new_members() {
    assert_eq!(
        &METHODS[..5],
        &[
            methods::START_CHANNEL,
            methods::STOP_CHANNEL,
            methods::SEND_MESSAGE,
            methods::LIST_CHANNELS,
            methods::CHANNEL_STATUS,
        ]
    );
    assert_eq!(METHODS.len(), 47);
    assert_eq!(METHODS[7], methods::START_RELAY_V2);
    assert_eq!(METHODS[10], methods::RELAY_CAPABILITIES_V2);
    assert_eq!(
        &METHODS[13..],
        &[
            methods::SEND_CHANNEL_INTENT,
            methods::SEND_EMAIL,
            methods::BEGIN_EMAIL_UPLOAD,
            methods::APPEND_EMAIL_UPLOAD_CHUNK,
            methods::SEND_EMAIL_UPLOAD,
            methods::ABORT_EMAIL_UPLOAD,
            methods::START_CHANNEL_CONFIGURED,
            methods::CHANNEL_CONTROLLER,
            methods::CHANNEL_INTERACTION,
            methods::CHECK_CHANNEL_HEALTH,
            methods::START_CHANNEL_SUPERVISED,
            methods::RESERVE_RUNTIME,
            methods::START_RUNTIME,
            methods::STOP_RUNTIME,
            methods::DESCRIBE_CONFIGURED_CHANNELS,
            methods::START_OUTBOUND_CHANNEL,
            methods::PREPARE_OUTBOUND_INTENT,
            methods::PREPARE_CHANNEL_INBOUND,
            methods::PREPARE_CHANNEL_IDENTITY,
            methods::EXECUTE_PROGRESSIVE,
            methods::EXECUTE_REMOTE,
            methods::PREPARE_CHANNEL_CONTROL,
            methods::START_RUNTIME_WITH_DISPATCH,
            methods::PREPARE_RUNTIME_COMMAND,
            methods::BEGIN_MEDIA_UPLOAD,
            methods::APPEND_MEDIA_UPLOAD_CHUNK,
            methods::FINISH_MEDIA_UPLOAD,
            methods::ABORT_MEDIA_UPLOAD,
            methods::BEGIN_MEDIA_READ,
            methods::READ_MEDIA_CHUNK,
            methods::ABORT_MEDIA_READ,
            methods::BEGIN_WEB_TURN,
            methods::PROJECT_WEB_OUTPUT,
            methods::END_WEB_TURN,
        ]
    );
}

/// A duplicate silently shadows one member with another's handler.
#[test]
fn member_names_are_unique_within_each_object() {
    for names in [&METHODS[..], &HOST_METHODS[..]] {
        let mut sorted = names.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            names.len(),
            "duplicate member in method table"
        );
    }
}

/// The two objects must not collide: the module serves one, the host the
/// other, on the same broker.
#[test]
fn the_module_and_host_objects_are_distinct() {
    assert_ne!(BUS_NAME, HOST_BUS_NAME);
    assert_ne!(OBJECT_PATH, HOST_OBJECT_PATH);
}

#[test]
fn module_id_is_the_registry_identity_for_the_module() {
    assert_eq!(MODULE_ID, "tinychannels");
}

/// Bus names are dotted and object paths are their slashed form. A host
/// that derives one from the other must keep agreeing with these literals.
#[test]
fn object_paths_are_the_slashed_form_of_their_bus_names() {
    assert_eq!(OBJECT_PATH, format!("/{}", BUS_NAME.replace('.', "/")));
    assert_eq!(
        HOST_OBJECT_PATH,
        format!("/{}", HOST_BUS_NAME.replace('.', "/"))
    );
}
