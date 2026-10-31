use tinychannels_bus::storage::{CollectionSpec, collections};

fn main() {
    let needs = collections();
    let names: Vec<_> = needs
        .iter()
        .map(|collection| collection.name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "channels.delivery_pending",
            "channels.delivery_failed",
            "channels.delivery_results",
            "channels.remote_bindings",
            "channels.remote_busy",
            "channels.remote_meta",
            "channels.media_metadata",
            "channels.migrations",
            "channels.whatsapp_device",
            "channels.whatsapp_identity",
            "channels.whatsapp_session",
            "channels.whatsapp_prekey",
            "channels.whatsapp_signed_prekey",
            "channels.whatsapp_sender_key",
            "channels.whatsapp_sync_key",
            "channels.whatsapp_lid_mapping",
            "channels_delivery_pending",
            "channels_delivery_failed",
            "channels_delivery_results",
            "channel_remote_bindings",
            "channel_remote_busy",
            "channel_remote_meta",
            "whatsapp_device",
            "whatsapp_identity",
            "whatsapp_session",
            "whatsapp_prekey",
            "whatsapp_signed_prekey",
            "whatsapp_sender_key",
            "whatsapp_sync_key",
            "whatsapp_lid_mapping",
        ]
    );
    assert_eq!(needs.len(), 30);
    let _: &CollectionSpec = &needs[0];
    assert!(needs.iter().all(|collection| {
        collection.indexes.is_empty()
            && collection.ttl_field.is_none()
            && collection.search.is_none()
    }));
}
