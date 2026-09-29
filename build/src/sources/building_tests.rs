//! Building rows: the height ladder, ground activities and footprint ids.

use super::*;
use arrow_array::ArrayRef;
use physics::emission::settlement::{area_law_lw_dba, building_profile};
use std::sync::Arc;

fn row(class: u8, height_tag_m: f64, floors: u8, ground_activity: bool) -> BuildingRow {
    BuildingRow {
        osm_id: 1,
        centroid: (1 << 29, 1 << 29),
        ring: Vec::new(),
        height_tag_m,
        floors,
        area_m2: Some(1_000.0),
        class,
        ground_activity,
        name: String::new(),
        address: String::new(),
        footprint_id: 0,
    }
}

/// A ground activity has no floors and no invented facade: it emits its footprint at 1.5 m
/// whatever its tags; an untagged building stands 8 m (3 floors) with its source at 4 m.
#[test]
fn ground_activities_emit_their_footprint_at_one_and_a_half_metres() {
    for class in [1, 3, 7] {
        let ground = building_emission(&row(class, 0.0, 0, true)).unwrap();
        assert_eq!(
            ground,
            building_emission(&row(class, 24.0, 8, true)).unwrap()
        );
        assert_eq!(
            (ground.source_height_m, ground.height_m, ground.floors),
            (1.5, 0.0, 0)
        );
        let profile = building_profile(class).unwrap();
        let expected = area_law_lw_dba(profile.lw_fixed_dba, profile.lw_per_m2_dba, 1_000.0);
        assert!((ground.sound.day_dba - expected).abs() < 1e-9);
        let building = building_emission(&row(class, 0.0, 0, false)).unwrap();
        assert_eq!((building.floors, building.source_height_m), (3, 4.0));
    }
    assert_eq!(
        building_emission(&row(10, 30.0, 10, false)),
        None,
        "sheds are silent"
    );
}

/// The ceiling applies after the whole ladder: tag errors stop at 828 m, 255 floors stay 765 m.
#[test]
fn source_height_is_half_the_laddered_height_under_the_tallest_building() {
    let source = |height_tag_m: f64, floors: u8| {
        building_emission(&row(1, height_tag_m, floors, false))
            .unwrap()
            .source_height_m
    };
    assert_eq!((source(31_231.0, 2), source(0.0, u8::MAX)), (414.0, 382.5));
    assert!((source(827.8, 0) - 413.9).abs() < 1e-9);
    assert_eq!((source(300.0, 0), source(12.0, 4)), (150.0, 6.0));
}

fn batch() -> RecordBatch {
    let ring = |x: i32| -> Vec<u8> {
        let words = [4, x, 0, x + 800, 0, x + 800, 800, x, 800];
        words
            .iter()
            .flat_map(|word: &i32| word.to_le_bytes())
            .collect()
    };
    let screening = [1u32, 1, 4, 0, 0, 800, 0, 800, 800, 0, 800];
    let screening: Vec<u8> = screening
        .iter()
        .flat_map(|word| word.to_le_bytes())
        .collect();
    let (a, b) = (ring(1 << 29), ring((1 << 29) + 5_000));
    let columns: Vec<(&str, ArrayRef)> = vec![
        ("kind", Arc::new(UInt8Array::from(vec![0, 0, 0, 1]))),
        (
            "osm_id",
            Arc::new(Int64Array::from(vec![Some(7), Some(8), None, None])),
        ),
        (
            "centroid_gx",
            Arc::new(Int32Array::from(vec![1 << 29, 1 << 29, 0, 0])),
        ),
        (
            "centroid_gy",
            Arc::new(Int32Array::from(vec![400, 400, 0, 0])),
        ),
        (
            "emission_centroid_gx",
            Arc::new(Int32Array::from(vec![
                Some((1 << 29) + 400),
                None,
                None,
                None,
            ])),
        ),
        (
            "emission_centroid_gy",
            Arc::new(Int32Array::from(vec![Some(400), None, None, None])),
        ),
        (
            "height",
            Arc::new(Float32Array::from(vec![Some(12.0), None, None, None])),
        ),
        (
            "floors",
            Arc::new(UInt8Array::from(vec![Some(4), Some(0), None, None])),
        ),
        (
            "area_m2",
            Arc::new(Float32Array::from(vec![
                Some(900.0),
                Some(900.0),
                None,
                None,
            ])),
        ),
        (
            "building_type",
            Arc::new(UInt8Array::from(vec![Some(0), Some(1), None, None])),
        ),
        (
            "height_source",
            Arc::new(UInt8Array::from(vec![1, 7, 1, 0])),
        ),
        (
            "emission_geom",
            Arc::new(BinaryArray::from(vec![
                Some(a.as_slice()),
                Some(b.as_slice()),
                None,
                None,
            ])),
        ),
        (
            "geom",
            Arc::new(BinaryArray::from(vec![
                Some(screening.as_slice()),
                None,
                Some(screening.as_slice()),
                None,
            ])),
        ),
        (
            "screening_ordinal",
            Arc::new(UInt32Array::from(vec![Some(5), None, Some(6), None])),
        ),
        (
            "name",
            Arc::new(StringArray::from(vec![Some("Dum"), None, None, None])),
        ),
        (
            "addr_street",
            Arc::new(StringArray::from(vec![
                Some("Vinohradska"),
                Some("Slezska"),
                None,
                None,
            ])),
        ),
        (
            "addr_housenumber",
            Arc::new(StringArray::from(vec![Some("12"), None, None, None])),
        ),
    ];
    RecordBatch::try_from_iter(columns).unwrap()
}

/// OSM buildings only; a screening footprint's id is the obstacles builder's, a ground activity
/// has none; the emission centroid is taken when present.
#[test]
fn rows_carry_the_footprint_id_of_their_screening_outline() {
    let square = Square { x: 276, y: 173 };
    let rows = read_batch(&batch(), square).unwrap();
    assert_eq!(
        rows.iter().map(|row| row.osm_id).collect::<Vec<_>>(),
        vec![7, 8]
    );
    assert_eq!(rows[0].footprint_id, footprint_id(square, 5));
    assert_eq!((rows[1].footprint_id, rows[1].ground_activity), (0, true));
    assert_eq!(rows[0].centroid, ((1 << 29) + 400, 400));
    assert_eq!(rows[1].centroid, (1 << 29, 400));
    assert_eq!(
        (rows[0].address.as_str(), rows[1].address.as_str()),
        ("Vinohradska 12", "Slezska")
    );
    assert_eq!(
        (rows[0].height_tag_m, rows[0].floors, rows[0].ring.len()),
        (12.0, 4, 4)
    );
}
