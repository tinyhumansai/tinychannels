use super::*;

#[test]
fn collections_is_pure_and_declares_each_owned_collection_once() {
    let specs = collections();
    let names: Vec<_> = specs.iter().map(|spec| spec.name.as_str()).collect();
    assert_eq!(names, COLLECTIONS);
    assert!(specs.iter().all(|spec| spec.indexes.is_empty()));
    assert_eq!(media_metadata_spec().name, MEDIA_METADATA);
}

#[test]
fn collections_include_every_legacy_migration_source() {
    let specs = collections();
    let names: Vec<_> = specs.iter().map(|spec| spec.name.as_str()).collect();

    for legacy in MIGRATION_SOURCE_COLLECTIONS {
        assert!(names.contains(&legacy), "missing migration source {legacy}");
    }
}
