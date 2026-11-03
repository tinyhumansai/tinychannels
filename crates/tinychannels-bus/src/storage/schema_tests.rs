use super::*;

#[test]
fn collections_is_a_unique_complete_owner_inventory() {
    let specs = collections();
    let names: Vec<_> = specs.iter().map(|spec| spec.name.as_str()).collect();
    assert_eq!(names, COLLECTIONS);
    assert_eq!(names.len(), 30);
    let unique: std::collections::HashSet<_> = names.iter().collect();
    assert_eq!(unique.len(), names.len());
    for legacy in MIGRATION_SOURCE_COLLECTIONS {
        assert!(names.contains(&legacy));
    }
    for name in [MEDIA_METADATA, MIGRATIONS] {
        assert!(names.contains(&name));
    }
    assert!(specs.iter().all(|spec| spec.indexes.is_empty()));
    assert_eq!(media_metadata_spec().name, MEDIA_METADATA);
}

#[test]
fn storage_collection_contract_is_a_serde_only_data_shape() {
    let spec = CollectionSpec::new("channels.media_metadata");
    let value = serde_json::to_value(&spec).unwrap();
    assert_eq!(value["name"], "channels.media_metadata");
    assert_eq!(value["indexes"], serde_json::json!([]));
    assert!(value["ttl_field"].is_null());
    assert!(value["search"].is_null());
}
