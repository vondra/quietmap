//! The hotel trip rate on a square of hand-built buildings and lodgings.
use super::*;
use crate::osm::geometry::{lonlat_to_grid, poly::encode_grid_poly};
use crate::structures::prepare_rows::{StructureRow, write};
use std::fs;

/// A building row: its use, OSM id, footprint, storeys and floors, outlined around a point.
fn building(
    building_use: u8,
    id: i64,
    area: f32,
    storeys: Option<u8>,
    floors: Option<u8>,
    around: (f64, f64),
    half_side: i32,
) -> StructureRow {
    let (x, y) = lonlat_to_grid(around.1, around.0);
    let (low, high) = (
        (x - half_side, y - half_side),
        (x + half_side, y + half_side),
    );
    StructureRow {
        kind: KIND_BUILDING,
        osm_id: Some(id),
        building_type: Some(building_use),
        area_m2: Some(area),
        storeys,
        floors,
        centroid: (x, y),
        emission_geometry: Some(encode_grid_poly(&[
            low,
            (high.0, low.1),
            high,
            (low.0, high.1),
        ])),
        ..Default::default()
    }
}

/// A year whose hotel trip rate depends on its lodgings, structures and trip observations.
fn year(root: &Path, factors: serde_json::Value) -> YearInputs {
    let sources = root.join("sources/2000");
    fs::create_dir_all(&sources).unwrap();
    let input = |id: &str, kind: &str, depends_on: &[&str]| {
        json!({"id": id, "kind": kind, "region": "world", "publisher": "Test",
            "url": "https://example.org/", "observed": {"start": "2000", "end": "2000"},
            "published": null, "licence": "CC0", "public_ok": true, "depends_on": depends_on,
            "description": "Fixture."})
    };
    let manifest = json!({"year": "2000", "built": false, "as_of": "2001-01-01",
    "flows": {"start": "2000-01-01", "end": "2001-01-01"}, "unavailable": [], "inputs": [
        input("lodgings", "lodgings", &[]),
        input("structures", "structures", &[]),
        input("trip-generation-observations", "model-observations", &[]),
        input("hotel-trip-rate", "hotel-trip-rate",
            &["lodgings", "structures", "trip-generation-observations"]),
    ]});
    fs::write(root.join("year.json"), manifest.to_string()).unwrap();
    fs::write(
        sources.join("trip-generation-observations"),
        json!({"hotel_car_trip_ends_per_room": factors}).to_string(),
    )
    .unwrap();
    YearInputs::read(&root.join("year.json"), &root.join("sources")).unwrap()
}

fn factor(value: f64, basis: &str, source: &str) -> serde_json::Value {
    json!({"factor": "a share", "value": value, "basis": basis, "source": source})
}

/// A hotel way joins its own row, a hotel node the hotel outline holding it rather than the
/// other building or the hotel complex overlapping it there; a node in a house, a node in no
/// building, a way that is no building and a lodging in a square without structures make no rate.
#[test]
fn the_rate_reads_the_floor_a_stated_room_of_lodgings_in_hotel_buildings() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let inputs = year(
        root,
        json!([
            factor(8.17, "measured", "ITE"),
            factor(0.5, "assumed", "dev4"),
            factor(0.6, "assumed", "dev4"),
        ]),
    );
    let (hotel, house) = ((50.0, 14.0), (50.01, 14.0));
    let square = crate::osm::geometry::square_of(hotel.0, hotel.1);
    write(
        &root.join(format!(
            "sources/2000/structures/z9/{}/{}/structures.arrow",
            square.x, square.y
        )),
        &[
            // Added before the hotel they overlap: the node must still take the hotel, not
            // another use or the complex whose centroid lies farther.
            building(0, 400, 1_000.0, Some(1), None, hotel, 3_000),
            building(
                HOTEL_USE,
                500,
                20_000.0,
                Some(2),
                None,
                (50.0005, 14.0005),
                5_000,
            ),
            building(
                HOTEL_USE,
                100,
                500.0,
                Some(3),
                Some(2),
                (50.03, 14.0),
                1_000,
            ),
            building(HOTEL_USE, 200, 400.0, None, Some(2), hotel, 1_000),
            building(0, 300, 200.0, Some(1), None, house, 1_000),
        ],
    )
    .unwrap();
    fs::write(
        root.join("sources/2000/lodgings"),
        "node\t1\t50.0\t14.0\t10\nnode\t2\t50.01\t14.0\t5\nnode\t3\t50.02\t14.0\t50\n\
         node\t4\t10.0\t10.0\t7\nway\t100\t50.03\t14.0\t20\nway\t999\t50.0\t14.0\t30\n",
    )
    .unwrap();
    let output = root.join("hotel-trip-rate.json");
    fit(&inputs, &output).unwrap();
    let table: serde_json::Value = serde_json::from_slice(&fs::read(&output).unwrap()).unwrap();
    // 8.17 x 0.5 x 0.6 = 2.451 car trip ends a room over (500 x 3 + 400 x 2) / (20 + 10) m2.
    let expected = 2.451 * 100.0 * 30.0 / 2_300.0;
    assert!(
        (table["rows"][0].as_f64().unwrap() - expected).abs() < 1e-9,
        "{table}"
    );
    let support: serde_json::Value =
        serde_json::from_slice(&fs::read(output.with_extension("samples.json")).unwrap()).unwrap();
    assert_eq!(
        support["rated"],
        json!({"lodgings": 2, "rooms": 30, "gross_floor_m2": 2_300.0})
    );
    let matched = |how: &str, building_use: u8| {
        support["matched"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["match"] == how && m["building_use"] == building_use)
            .map(|m| m["sums"]["rooms"].clone())
    };
    assert_eq!(matched("by-id", HOTEL_USE), Some(json!(20)));
    assert_eq!(matched("node-inside", HOTEL_USE), Some(json!(10)));
    assert_eq!(matched("node-inside", 0), Some(json!(5)));
    assert_eq!(
        support["unmatched"],
        json!([
            {"match": "id-without-building", "lodgings": 1, "rooms": 30},
            {"match": "node-in-no-building", "lodgings": 1, "rooms": 50},
            {"match": "square-without-structures", "lodgings": 1, "rooms": 7},
        ])
    );
    assert_eq!(
        support["squares"],
        json!([[square.x, square.y, 2, 30, 2_300.0]])
    );
}

/// An assumption is an input like a measurement, but never without its source.
#[test]
fn a_factor_without_its_source_or_basis_is_refused() {
    for factors in [
        json!([factor(8.17, "measured", "ITE"), factor(0.5, "assumed", " ")]),
        json!([factor(8.17, "guessed", "ITE")]),
        json!([]),
    ] {
        let temporary = tempfile::tempdir().unwrap();
        let inputs = year(temporary.path(), factors);
        assert!(observed_factors(&inputs).is_err());
    }
}
