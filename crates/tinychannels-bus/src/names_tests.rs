use super::{
    BUS_NAME, HOST_BUS_NAME, HOST_METHODS, HOST_OBJECT_PATH, METHODS, OBJECT_PATH, host_methods,
    methods,
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
    ] {
        assert!(METHODS.contains(&m), "{m} missing from METHODS");
    }
    for m in [host_methods::DELIVER_INBOUND, host_methods::REPORT_STATUS] {
        assert!(HOST_METHODS.contains(&m), "{m} missing from HOST_METHODS");
    }
}

/// A duplicate silently shadows one member with another's handler.
#[test]
fn member_names_are_unique_within_each_object() {
    let mut sorted = METHODS.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), METHODS.len(), "duplicate member in METHODS");
}

/// The two objects must not collide: the module serves one, the host the
/// other, on the same broker.
#[test]
fn the_module_and_host_objects_are_distinct() {
    assert_ne!(BUS_NAME, HOST_BUS_NAME);
    assert_ne!(OBJECT_PATH, HOST_OBJECT_PATH);
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
