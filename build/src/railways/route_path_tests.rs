//! Manifest-id symlinks must resolve global OSM topology beside the original geography output.
#[test]
fn staged_geography_alias_finds_original_global_topology() {
    let root = tempfile::tempdir().unwrap();
    let original = root.path().join("original.geography");
    std::fs::create_dir(&original).unwrap();
    let alias = root.path().join("input-id");
    std::os::unix::fs::symlink(&original, &alias).unwrap();
    assert_eq!(
        super::sibling(&alias, "train-routes").unwrap(),
        root.path().join("original.geography.train-routes.arrow")
    );
}
