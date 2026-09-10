//! Ground cache and movement regressions.
use super::*;
use crate::airport_io::AERODROME_AEROWAY_TYPE;
use crate::synth_airport_io::SYNTH_LINES_FILE;

#[test]
fn ops_kind_mapping_runway_taxi_only_skips_unknown() {
    assert_eq!(ops_kind_from_aeroway(0), Some(GROUND_OPS_KIND_RUNWAY_ROLL));
    assert_eq!(ops_kind_from_aeroway(1), Some(GROUND_OPS_KIND_TAXI));
    assert_eq!(ops_kind_from_aeroway(6), Some(GROUND_OPS_KIND_RUNWAY_ROLL));
    assert_eq!(ops_kind_from_aeroway(7), Some(GROUND_OPS_KIND_RUNWAY_ROLL));
    assert_eq!(ops_kind_from_aeroway(2), None);
    assert_eq!(ops_kind_from_aeroway(255), None);
}

#[test]
fn run_airport_traffic_empty_segments_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let by_square = tmp.path().join("segments_by_square");
    let prepared_year = tmp.path().join("prepared_year");
    std::fs::create_dir_all(&by_square).unwrap();
    std::fs::create_dir_all(&prepared_year).unwrap();
    let n =
        run_airport_traffic(&by_square, &[], &prepared_year, &prepared_year, 14, 0, None).unwrap();
    assert_eq!(n, 0);
}

#[test]
fn squarecache_load_concatenates_real_and_synth_lines() {
    use crate::synth_airport_io::{
        synth_osm_id_for, write_synth_airport_lines, SynthAirportLineRow, AIRSTRIP_AEROWAY_TYPE,
    };

    let dir = tempfile::tempdir().unwrap();
    let square = crate::spatial::square_id(50.1, 14.26).unwrap();
    let square_dir = dir.path().join(square_path(square));
    std::fs::create_dir_all(&square_dir).unwrap();

    let synth_key = "LKTEST".to_string();
    let synth = SynthAirportLineRow {
        osm_id: synth_osm_id_for(50.1, 14.26),
        segment_idx: 0,
        airport_key: synth_key.clone(),
        start_gx: grid::lonlat_to_grid(14.26, 50.1).0,
        start_gy: grid::lonlat_to_grid(14.26, 50.1).1,
        end_gx: grid::lonlat_to_grid(14.27, 50.105).0,
        end_gy: grid::lonlat_to_grid(14.27, 50.105).1,
        length_m: 500.0,
        heading_deg: 60.0,
        aeroway_type: AIRSTRIP_AEROWAY_TYPE,
        name: "Auto airfield".to_string(),
    };
    let synth_path = square_dir.join(SYNTH_LINES_FILE);
    write_synth_airport_lines(&synth_path, [synth.clone()]).unwrap();
    assert!(
        synth_path.exists(),
        "test setup: synth file must exist at {}",
        synth_path.display()
    );

    let red_herring = AirportArea::new(
        999,
        AERODROME_AEROWAY_TYPE,
        "Red Herring".to_string(),
        "REDHERRING".to_string(),
        50.1,
        14.26,
        Vec::new(),
        1_000_000.0,
    );

    let cache = SquareCache::load(
        dir.path(),
        square,
        &crate::airport_index::AerodromeIndex::build(&[red_herring]),
    )
    .unwrap();
    assert_eq!(cache.lines.len(), 1, "synth line should be loaded");
    assert_eq!(
        cache.airport_keys[0], synth_key,
        "synth row must keep its pre-resolved airport_key (no re-resolution)"
    );
    assert_ne!(
        cache.airport_keys[0], "REDHERRING",
        "synth row must NOT be re-resolved via nearest_aerodrome_within"
    );
    let idx = cache
        .line_index
        .get(&(synth.osm_id, synth.segment_idx))
        .copied()
        .expect("synth (osm_id, segment_idx) must be indexed");
    assert_eq!(idx, 0);
}

#[test]
fn squarecache_load_no_collisions_between_real_and_synth() {
    use crate::synth_airport_io::{
        synth_osm_id_for, write_synth_airport_lines, SynthAirportLineRow, AIRSTRIP_AEROWAY_TYPE,
        SYNTHETIC_OSM_ID_BIT,
    };

    let dir = tempfile::tempdir().unwrap();
    let square = crate::spatial::square_id(50.1, 14.26).unwrap();
    let square_dir = dir.path().join(square_path(square));
    std::fs::create_dir_all(&square_dir).unwrap();

    let synth_osm_id = synth_osm_id_for(50.1, 14.26);
    let real_low_bits = synth_osm_id & !SYNTHETIC_OSM_ID_BIT;
    let real_osm_id_i64 = real_low_bits as i64;
    assert!(
        real_osm_id_i64 > 0,
        "low bits must round-trip as positive i64"
    );
    write_real_airport_lines_arrow(
        &square_dir.join("airport_lines.arrow"),
        &[FakeRealLine {
            osm_id: real_osm_id_i64,
            segment_idx: 0,
            start_lat: 50.0,
            start_lon: 14.0,
            end_lat: 50.0,
            end_lon: 14.001,
            length_m: 71.5,
            aeroway_type: 0,
        }],
    );
    write_synth_airport_lines(
        &square_dir.join(SYNTH_LINES_FILE),
        [SynthAirportLineRow {
            osm_id: synth_osm_id,
            segment_idx: 0,
            airport_key: "auto-x".to_string(),
            start_gx: grid::lonlat_to_grid(14.26, 50.1).0,
            start_gy: grid::lonlat_to_grid(14.26, 50.1).1,
            end_gx: grid::lonlat_to_grid(14.27, 50.105).0,
            end_gy: grid::lonlat_to_grid(14.27, 50.105).1,
            length_m: 500.0,
            heading_deg: 60.0,
            aeroway_type: AIRSTRIP_AEROWAY_TYPE,
            name: "x".to_string(),
        }],
    )
    .unwrap();

    let cache = SquareCache::load(
        dir.path(),
        square,
        &crate::airport_index::AerodromeIndex::build(&[]),
    )
    .unwrap();
    assert_eq!(cache.lines.len(), 2, "real + synth both loaded");
    assert!(cache.line_index.contains_key(&(real_low_bits, 0u16)));
    assert!(cache.line_index.contains_key(&(synth_osm_id, 0u16)));
}

#[test]
fn squarecache_load_unions_real_and_synth_with_correct_keys() {
    use crate::synth_airport_io::{
        synth_osm_id_for, write_synth_airport_lines, SynthAirportLineRow, AIRSTRIP_AEROWAY_TYPE,
    };

    let dir = tempfile::tempdir().unwrap();
    let square = crate::spatial::square_id(50.1, 14.26).unwrap();
    let square_dir = dir.path().join(square_path(square));
    std::fs::create_dir_all(&square_dir).unwrap();

    write_real_airport_lines_arrow(
        &square_dir.join("airport_lines.arrow"),
        &[FakeRealLine {
            osm_id: 42,
            segment_idx: 0,
            start_lat: 50.10,
            start_lon: 14.26,
            end_lat: 50.10,
            end_lon: 14.261,
            length_m: 71.5,
            aeroway_type: 0,
        }],
    );
    let synth_osm_id = synth_osm_id_for(50.5, 14.0);
    write_synth_airport_lines(
        &square_dir.join(SYNTH_LINES_FILE),
        [SynthAirportLineRow {
            osm_id: synth_osm_id,
            segment_idx: 0,
            airport_key: "auto-synthetic".to_string(),
            start_gx: grid::lonlat_to_grid(14.0, 50.5).0,
            start_gy: grid::lonlat_to_grid(14.0, 50.5).1,
            end_gx: grid::lonlat_to_grid(14.0, 50.501).0,
            end_gy: grid::lonlat_to_grid(14.0, 50.501).1,
            length_m: 100.0,
            heading_deg: 0.0,
            aeroway_type: AIRSTRIP_AEROWAY_TYPE,
            name: "synth".to_string(),
        }],
    )
    .unwrap();

    let lkpr = AirportArea::new(
        12345,
        AERODROME_AEROWAY_TYPE,
        "Praha".to_string(),
        "LKPR".to_string(),
        50.10,
        14.26,
        Vec::new(),
        10_000_000.0,
    );

    let cache = SquareCache::load(
        dir.path(),
        square,
        &crate::airport_index::AerodromeIndex::build(&[lkpr]),
    )
    .unwrap();
    assert_eq!(cache.lines.len(), 2, "real + synth both loaded");
    assert_eq!(cache.airport_keys[0], "LKPR");
    assert_eq!(cache.airport_keys[1], "auto-synthetic");
    assert_eq!(cache.line_index.get(&(42, 0)), Some(&0));
    assert_eq!(cache.line_index.get(&(synth_osm_id, 0)), Some(&1));
}

mod fixtures;
pub(crate) use fixtures::{write_real_airport_lines_arrow, FakeRealLine};

mod boundary;
mod movement;

#[test]
fn counter_row_order_uses_every_identity_field_independently_of_hash_order() {
    let base = CounterKey {
        airport_key: "A".into(),
        osm_id: 1,
        segment_idx: 0,
        ops_kind: 0,
        is_departure: 0,
        veh_kind: 0,
        class_idx: 0,
        period: 0,
    };
    let mut keys = vec![base.clone()];
    for field in 0..8 {
        let mut key = base.clone();
        match field {
            0 => key.airport_key = "B".into(),
            1 => key.osm_id += 1,
            2 => key.segment_idx += 1,
            3 => key.ops_kind += 1,
            4 => key.is_departure += 1,
            5 => key.veh_kind += 1,
            6 => key.class_idx += 1,
            _ => key.period += 1,
        }
        keys.push(key);
    }
    keys.sort();
    let projection = |row: &AirportTrafficRow| CounterKey {
        airport_key: row.airport_key.clone(),
        osm_id: row.osm_id,
        segment_idx: row.segment_idx,
        ops_kind: row.ops_kind,
        is_departure: row.is_departure,
        veh_kind: row.veh_kind,
        class_idx: row.class_idx,
        period: row.period,
    };
    for reverse in [false, true] {
        let mut insertion = keys.clone();
        if reverse {
            insertion.reverse();
        }
        let counters = insertion
            .into_iter()
            .map(|key| {
                (
                    key,
                    CounterAcc {
                        band_energy_lin: [3.0; NUM_BANDS],
                        fid_set: HashSet::from([42, 99]),
                        ..Default::default()
                    },
                )
            })
            .collect();
        let rows = counters_to_rows(counters, &HashMap::new());
        assert!(rows.iter().map(projection).eq(keys.iter().cloned()));
        assert!(rows
            .iter()
            .all(|r| r.band_energy_lin == [3.0; NUM_BANDS] && r.unique_movement_count == 2));
    }
}

#[test]
fn indexed_real_line_identity_preserves_ties_name_only_and_empty_plan() {
    let dir = tempfile::tempdir().unwrap();
    let owner = crate::spatial::square_id(0.0, 179.999).unwrap();
    let path = dir
        .path()
        .join(square_path(owner))
        .join("airport_lines.arrow");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    write_real_airport_lines_arrow(
        &path,
        &[FakeRealLine {
            osm_id: 1,
            segment_idx: 0,
            start_lat: 0.0,
            end_lat: 0.0,
            start_lon: 179.999,
            end_lon: -179.999,
            length_m: 220.0,
            aeroway_type: 0,
        }],
    );
    let area = |key: &str, name: &str| {
        AirportArea::new(1, 5, name.into(), key.into(), 0.0, 180.0, Vec::new(), 0.0)
    };
    for (areas, expected) in [
        (vec![area("FIRST", ""), area("SECOND", "")], Some("FIRST")),
        (vec![area("SECOND", ""), area("FIRST", "")], Some("SECOND")),
        (vec![area("", ""), area("KEY", "")], Some("KEY")),
        (vec![area("", "Named"), area("KEY", "")], None),
        (Vec::new(), None),
    ] {
        let index = crate::airport_index::AerodromeIndex::build(&areas);
        let cache = SquareCache::load(dir.path(), owner, &index).unwrap();
        assert_eq!(cache.airport_keys.len(), 1);
        match expected {
            Some(key) => assert_eq!(cache.airport_keys[0], key),
            None => assert!(cache.airport_keys[0].starts_with("strip:z15:")),
        }
    }
}
