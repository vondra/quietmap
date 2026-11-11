//! The venue-seating fit: surveyed seats by kind, and the kind the buildings of bars host.

use super::*;
use crate::dev4::degrees_to_z30;
use crate::sources::venues::{Seating, VenueSite};
use arrow_array::{
    ArrayRef, BinaryArray, Float32Array, Int32Array, Int64Array, RecordBatch, StringArray,
    UInt8Array, UInt32Array,
};
use std::sync::Arc;

fn site(kind: VenueKind, (lat, lon): (f64, f64)) -> VenueSite {
    VenueSite {
        lat,
        lon,
        kind,
        seating: Seating::Unknown,
        area_m2: 0.0,
        hours: None,
        name: String::new(),
    }
}

/// Buildings `(osm id, class, centre, screening ordinal)`, each a square 400 z30 units (15 m) a side.
fn structures(buildings: &[(i64, u8, (i32, i32), u32)]) -> RecordBatch {
    let outline = |(x, y): (i32, i32)| -> Vec<u8> {
        let corners = [(-200, -200), (200, -200), (200, 200), (-200, 200)];
        [1u32, 1, 4]
            .iter()
            .flat_map(|word| word.to_le_bytes())
            .chain(corners.iter().flat_map(|(dx, dy)| {
                [x + dx, y + dy]
                    .into_iter()
                    .flat_map(|word: i32| word.to_le_bytes())
            }))
            .collect()
    };
    let outlines: Vec<Vec<u8>> = buildings.iter().map(|b| outline(b.2)).collect();
    let n = buildings.len();
    let columns: Vec<(&str, ArrayRef)> = vec![
        ("kind", Arc::new(UInt8Array::from(vec![0; n]))),
        (
            "osm_id",
            Arc::new(Int64Array::from_iter_values(buildings.iter().map(|b| b.0))),
        ),
        (
            "centroid_gx",
            Arc::new(Int32Array::from_iter_values(
                buildings.iter().map(|b| b.2.0),
            )),
        ),
        (
            "centroid_gy",
            Arc::new(Int32Array::from_iter_values(
                buildings.iter().map(|b| b.2.1),
            )),
        ),
        (
            "emission_centroid_gx",
            Arc::new(Int32Array::from(vec![None; n])),
        ),
        (
            "emission_centroid_gy",
            Arc::new(Int32Array::from(vec![None; n])),
        ),
        ("height", Arc::new(Float32Array::from(vec![None; n]))),
        ("floors", Arc::new(UInt8Array::from(vec![None; n]))),
        ("storeys", Arc::new(UInt8Array::from(vec![None; n]))),
        (
            "area_m2",
            Arc::new(Float32Array::from(vec![Some(225.0); n])),
        ),
        (
            "building_type",
            Arc::new(UInt8Array::from_iter_values(buildings.iter().map(|b| b.1))),
        ),
        ("height_source", Arc::new(UInt8Array::from(vec![1; n]))),
        ("emission_geom", Arc::new(BinaryArray::from(vec![None; n]))),
        (
            "geom",
            Arc::new(BinaryArray::from_iter_values(outlines.iter())),
        ),
        (
            "screening_ordinal",
            Arc::new(UInt32Array::from_iter_values(buildings.iter().map(|b| b.3))),
        ),
        ("name", Arc::new(StringArray::from(vec![None::<&str>; n]))),
        (
            "addr_street",
            Arc::new(StringArray::from(vec![None::<&str>; n])),
        ),
        (
            "addr_housenumber",
            Arc::new(StringArray::from(vec![None::<&str>; n])),
        ),
    ];
    RecordBatch::try_from_iter(columns).unwrap()
}

/// A kind's seats where a terrace is mapped are the median of its surveyed venues with seats, and
/// where none is the mean over all of them; a kind without a surveyed seat is an error, not zero.
#[test]
fn tagged_seats_are_the_median_with_seats_and_expected_seats_the_mean_over_all() {
    let text = "bar\t0\nbar\t10\nbar\t20\nbar\t40\npub\t0\npub\t30\nnightclub\t50\n\
                restaurant\t0\nrestaurant\t0\nrestaurant\t24\ncafe\t8\ncafe\t12\nfast_food\t23\n";
    let surveyed = parse_survey(text).unwrap();
    let (seats, counts) = seats(&surveyed).unwrap();
    assert_eq!(
        seats,
        [
            [20.0, 17.5],
            [30.0, 15.0],
            [50.0, 50.0],
            [24.0, 8.0],
            [10.0, 10.0],
            [23.0, 23.0]
        ]
    );
    assert_eq!(counts[0], [4, 3]);
    let without_cafe_seats = parse_survey(&text.replace("cafe\t8\ncafe\t12", "cafe\t0")).unwrap();
    assert!(seats_error(&without_cafe_seats).contains("Cafe"));
    assert!(parse_survey("bar 12\n").is_err());
    assert!(parse_survey("hotel\t12\n").is_err());
    assert!(parse_survey("bar\t-1\n").is_err());
}

fn seats_error(surveyed: &[(VenueKind, f64)]) -> String {
    seats(surveyed).map(|_| ()).unwrap_err().to_string()
}

/// Only buildings of bars and restaurants host; a building counts a kind once however many places
/// of it it holds, and the kind the most buildings host is the unmapped buildings' kind.
#[test]
fn the_unmapped_kind_is_the_one_most_buildings_of_bars_and_restaurants_host() {
    let at = |dlat: f64| (50.0 + dlat, 14.0);
    let (a, b, c) = (at(0.0), at(0.001), at(0.002));
    let z30 = |point: (f64, f64)| degrees_to_z30(point.0, point.1);
    let square = Square::of_z30(z30(a).0, z30(a).1);
    let batch = structures(&[
        (1, HOSPITALITY, z30(a), 1),
        (2, HOSPITALITY, z30(b), 2),
        (3, 0, z30(c), 3),
        (4, HOSPITALITY, z30(at(0.003)), 4),
    ]);
    let sites = [
        site(VenueKind::Restaurant, a),
        site(VenueKind::Restaurant, (a.0, a.1 + 0.00002)),
        site(VenueKind::Cafe, a),
        site(VenueKind::Restaurant, b),
        site(VenueKind::Bar, c),
        site(VenueKind::Bar, c),
        site(VenueKind::Bar, at(0.0045)),
    ];
    let mut hosting = Hosting::default();
    hosting.add_square(&[batch], square, &sites).unwrap();
    let mut expected = [0; 7];
    expected[kind_index(VenueKind::Restaurant)] = 2;
    expected[kind_index(VenueKind::Cafe)] = 1;
    assert_eq!(
        hosting,
        Hosting {
            buildings: 3,
            hosting: 2,
            by_kind: expected
        }
    );
    assert_eq!(hosting.predominant().unwrap(), VenueKind::Restaurant);
    assert!(Hosting::default().predominant().is_err());
}
