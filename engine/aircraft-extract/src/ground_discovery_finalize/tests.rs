//! Canonical discovery preserves unique geometry, removes exact copies and refuses conflicts.
use super::*;
use crate::synth_airport_io::*;

fn strip(lat: f64, lon: f64) -> SynthAirportLineRow {
    let (gx, gy) = grid::lonlat_to_grid(lon, lat);
    SynthAirportLineRow {
        osm_id: synth_osm_id_for(lat, lon),
        segment_idx: 0,
        airport_key: synth_airport_key_for(lat, lon),
        start_gx: gx - 100,
        start_gy: gy,
        end_gx: gx + 100,
        end_gy: gy,
        length_m: 7.0,
        heading_deg: 90.0,
        aeroway_type: 7,
        name: "old display name".into(),
    }
}

fn put(root: &Path, owner: u64, lines: Vec<SynthAirportLineRow>) {
    let directory = root.join(crate::spatial::square_path(owner));
    write_synth_airport_lines(&directory.join(SYNTH_LINES_FILE), lines).unwrap();
}

#[test]
fn canonical_owners_preserve_unique_misplaced_and_reattributed_geometry_once() {
    let temp = tempfile::tempdir().unwrap();
    let raw = temp.path().join("raw");
    let output = temp.path().join("canonical");
    let line = strip(30.0, -40.0);
    let target = owner(line.osm_id).unwrap();
    let foreign = crate::spatial::square_id(30.0, -42.0).unwrap();
    let unique = strip(31.0, -40.0);
    let unique_owner = owner(unique.osm_id).unwrap();
    let mut reattributed = strip(30.001, -40.0);
    reattributed.airport_key = "REAL".into();
    put(
        &raw,
        foreign,
        vec![line.clone(), unique.clone(), reattributed.clone()],
    );
    put(&raw, target, vec![line]);
    let before = batches::load(&raw, foreign).unwrap();
    assert_eq!(
        finalize_ground_discovery(&raw, &output, None, 0).unwrap(),
        2
    );
    assert_eq!(batches::load(&output, target).unwrap().num_rows(), 2);
    assert_eq!(
        batches::load(&output, unique_owner).unwrap(),
        before.slice(1, 1)
    );
    assert_eq!(batches::load(&output, foreign).unwrap().num_rows(), 0);
    assert_eq!(batches::load(&raw, foreign).unwrap(), before);
    assert!(finalize_ground_discovery(&raw, &output, None, 0).is_err());
}

#[test]
fn conflicting_same_identity_fails_before_any_canonical_arrow_or_source_mutation() {
    let temp = tempfile::tempdir().unwrap();
    let raw = temp.path().join("raw");
    let output = temp.path().join("canonical");
    let line = strip(30.0, -40.0);
    let a = owner(line.osm_id).unwrap();
    let b = crate::spatial::square_id(30.0, -42.0).unwrap();
    put(&raw, a, vec![line.clone()]);
    let mut changed = line;
    changed.end_gx += 1;
    put(&raw, b, vec![changed]);
    let source_path = raw
        .join(crate::spatial::square_path(a))
        .join(SYNTH_LINES_FILE);
    let original = std::fs::read(&source_path).unwrap();
    assert!(finalize_ground_discovery(&raw, &output, None, 0)
        .unwrap_err()
        .to_string()
        .contains("conflicting"));
    assert!(!output.join("z9").exists());
    assert_eq!(std::fs::read(source_path).unwrap(), original);
}

#[test]
fn scoped_promotion_keeps_outside_geometry_and_existing_files_untouched() {
    let temp = tempfile::tempdir().unwrap();
    let raw = temp.path().join("raw");
    let canonical = temp.path().join("canonical");
    let prepared = temp.path().join("prepared");
    let inside = strip(0.1, 179.9);
    let outside = strip(0.1, 170.0);
    let source_owner = owner(inside.osm_id).unwrap();
    let outside_owner = owner(outside.osm_id).unwrap();
    put(&raw, source_owner, vec![inside, outside.clone()]);
    put(&prepared, outside_owner, vec![outside]);
    let outside_path = prepared
        .join(crate::spatial::square_path(outside_owner))
        .join(SYNTH_LINES_FILE);
    let before = std::fs::read(&outside_path).unwrap();
    let scope = ScopeBbox::parse("0,179.9,0.2,180").unwrap();
    assert_eq!(
        finalize_ground_discovery(&raw, &canonical, Some(&scope), 0).unwrap(),
        1
    );
    promote_ground_discovery(&canonical, &prepared).unwrap();
    assert_eq!(std::fs::read(outside_path).unwrap(), before);
    assert_eq!(batches::load(&prepared, source_owner).unwrap().num_rows(), 1);
}

#[test]
fn retained_caller_allocation_is_admitted_before_final_output_mutation() {
    let temp = tempfile::tempdir().unwrap();
    let raw = temp.path().join("raw");
    let output = temp.path().join("canonical");
    let line = strip(30.0, -40.0);
    put(&raw, owner(line.osm_id).unwrap(), vec![line]);
    let held = crate::memory::available_memory_bytes();
    assert!(finalize_ground_discovery(&raw, &output, None, held)
        .unwrap_err()
        .to_string()
        .contains("allocation allowance"));
    assert!(!output.exists());
}
