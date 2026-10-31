use super::*;

#[test]
fn collections_is_pure_and_declares_each_owned_collection_once() {
    let specs = collections();
    let names: Vec<_> = specs.iter().map(|spec| spec.name.as_str()).collect();
    assert_eq!(names, COLLECTIONS);
    assert!(specs.iter().all(|spec| spec.indexes.is_empty()));
    assert!(specs.iter().all(|spec| spec.ttl_field.is_none()));
    assert!(specs.iter().all(|spec| spec.search.is_none()));
    assert_eq!(media_metadata_spec().name, MEDIA_METADATA);
}

#[test]
fn schema_conversion_preserves_index_constraints_expiry_and_search_paths() {
    let declaration: CollectionSchema = serde_json::from_value(serde_json::json!({
        "name": "channels.media_metadata",
        "indexes": [
            {
                "name": "account_media",
                "fields": ["scope.account_id", "media_id"],
                "unique": true
            },
            {
                "name": "recent_uploads",
                "fields": ["created_ms"],
                "unique": false
            }
        ],
        "ttl_field": "lifetime.expires_ms",
        "search": {"fields": ["caption", "metadata.description"]}
    }))
    .unwrap();

    let driver = to_driver_spec(declaration);

    assert_eq!(driver.name, MEDIA_METADATA);
    assert_eq!(
        driver.indexes,
        vec![
            DriverIndexSpec {
                name: "account_media".into(),
                fields: vec!["scope.account_id".into(), "media_id".into()],
                unique: true,
            },
            DriverIndexSpec {
                name: "recent_uploads".into(),
                fields: vec!["created_ms".into()],
                unique: false,
            },
        ]
    );
    assert_eq!(driver.ttl_field.as_deref(), Some("lifetime.expires_ms"));
    assert_eq!(
        driver.search.unwrap().fields,
        vec!["caption", "metadata.description"]
    );
}
