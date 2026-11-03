//! PBF extraction rejects invalid node ids and retains unlinked controls deterministically.

use super::{pbf_fixture::*, topology_test_helpers::*};
use arrow_array::Int64Array;
use std::path::{Path, PathBuf};

#[test]
fn repeated_square_selection_bakes_country_once_and_keeps_the_reader_record() {
    let root = TempDir::new("country-bake");
    let input = root.path().join("fixture.osm.pbf");
    write_fixture_pbf(&input, NODES, &[]);
    let square = crate::dev4::Square { x: 276, y: 173 };
    let ocean = crate::dev4::Square { x: 0, y: 0 };
    let output =
        run_extract_squares(root.path(), "prepared", &input, &[square, square, ocean]).unwrap();
    let roads = all_rows(&output.join("z9/276/173/roads.arrow"));
    assert_eq!(
        roads
            .schema()
            .fields()
            .iter()
            .filter(|field| field.name() == "country_iso")
            .count(),
        1
    );
    assert_eq!(
        roads.schema().metadata()["roads_contract"],
        "country_baked_v1"
    );
    let expected = u16::from_le_bytes(*b"XY");
    assert!(
        column::<arrow_array::UInt16Array>(&roads, "country_iso")
            .values()
            .iter()
            .all(|country| *country == expected)
    );
    let reader = crate::dev4::Dev4 {
        prepared: output.clone(),
        rasters: root.path().join("unused"),
    };
    assert_eq!(reader.square_country(square).unwrap(), expected);
    assert_eq!(reader.square_country(ocean).unwrap(), 0);
    assert_eq!(
        std::fs::metadata(output.join("z9/276/173/square-country-city.bin"))
            .unwrap()
            .len(),
        13
    );
    assert!(!output.join("z9/275/173").exists());
}

#[test]
fn node_cache_cap_is_checked_before_the_selected_node_filter() {
    let root = TempDir::new("node-cap");
    let input = root.path().join("fixture.osm.pbf");
    let output = root.path().join("prepared");
    let mut nodes = NODES.to_vec();
    nodes.push((25_000_000_000, 50.0, 14.0));
    write_fixture_pbf(&input, &nodes, &[]);
    let error = run_extract(root.path(), "prepared", &input).unwrap_err();
    assert!(format!("{error:#}").contains("MAX_NODE_ID"));
    assert!(!output.exists());
    assert!(!root.path().join("prepared.railway-ways.arrow").exists());
}

fn transport_node_files(output: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let z9 = output.join("z9");
    if !z9.is_dir() {
        return files;
    }
    for x in std::fs::read_dir(&z9).expect("read z9") {
        let x = x.expect("read square x").path();
        for y in std::fs::read_dir(&x).expect("read square y") {
            let candidate = y.expect("read square").path().join("transport_nodes.arrow");
            if candidate.is_file() {
                files.push(candidate);
            }
        }
    }
    files.sort();
    files
}

/// Unlinked control nodes flush from one map in `ControlPoints::finish`: two
/// extracts of the same PBF must write identical `transport_nodes.arrow`
/// content — same rows in the same order, same metadata pairs. Raw file bytes
/// still vary in schema-metadata KEY ORDER (arrow-rs 54 serializes the metadata
/// `HashMap` in iteration order, `arrow-ipc/convert.rs::metadata_to_fb`), which
/// no reader observes (metadata is read by key) and no writer option orders —
/// so this test compares decoded batches, not the file bytes.
#[test]
fn unlinked_transport_controls_extract_deterministically_twice() {
    let root = TempDir::new("determinism");
    let input = root.path().join("fixture.osm.pbf");
    // Twelve whistle boards no way references: orphans flush in map order.
    let mut nodes = NODES.to_vec();
    let mut tags = Vec::new();
    for (offset, id) in (40..52).enumerate() {
        nodes.push((
            id,
            50.001 + offset as f64 * 0.0001,
            14.061 + offset as f64 * 0.0001,
        ));
        tags.push((id, "railway", "level_crossing"));
    }
    nodes.sort_by_key(|node| node.0);
    write_fixture_pbf(&input, &nodes, &tags);

    let first = run_extract(root.path(), "first", &input).unwrap();
    let second = run_extract(root.path(), "second", &input).unwrap();
    let first_files = transport_node_files(&first);
    let second_files = transport_node_files(&second);
    assert_eq!(
        first_files.len(),
        second_files.len(),
        "transport_nodes.arrow file sets differ"
    );
    assert!(
        !first_files.is_empty(),
        "no transport_nodes.arrow written — orphans lost, test vacuous"
    );
    let mut orphan_ids = Vec::new();
    for (first_file, second_file) in first_files.iter().zip(&second_files) {
        assert_eq!(
            first_file.strip_prefix(&first).unwrap(),
            second_file.strip_prefix(&second).unwrap(),
            "transport_nodes.arrow paths differ"
        );
        let first_batch = all_rows(first_file);
        let second_batch = all_rows(second_file);
        assert_eq!(
            first_batch,
            second_batch,
            "transport_nodes.arrow differs between identical extracts: {}",
            first_file.strip_prefix(&first).unwrap().display()
        );
        orphan_ids.extend(
            column::<Int64Array>(&first_batch, "osm_id")
                .values()
                .iter()
                .copied(),
        );
    }
    // Orphans flush in node-id order (`BTreeMap`), which the square sort and
    // the square writer preserve — pin the order, not just the set.
    assert_eq!(
        orphan_ids,
        (40..52).collect::<Vec<_>>(),
        "orphan controls missing or reordered in transport_nodes.arrow"
    );
}
