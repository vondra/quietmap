//! Geographic ownership regressions for polygon holes, borders, coasts and disputed land.

use super::*;
use crate::osm::region_input::RegionInput;

fn rectangle(west: f64, south: f64, east: f64, north: f64) -> Vec<[f64; 2]> {
    vec![
        [west, south],
        [east, south],
        [east, north],
        [west, north],
        [west, south],
    ]
}

fn region(iso: &str, polygons: Vec<Vec<Vec<[f64; 2]>>>, disputed: bool) -> RegionInput {
    RegionInput {
        country: Some(iso.into()),
        continent: 1,
        disputed,
        polygons,
    }
}

#[test]
fn land_contains_excludes_holes_and_boundaries_regardless_of_ring_direction() {
    for reverse in [false, true] {
        let mut outer = rectangle(0.0, 0.0, 2.0, 2.0);
        let mut hole = rectangle(0.5, 0.5, 1.5, 1.5);
        if reverse {
            outer.reverse();
            hole.reverse();
        }
        let regions = Regions::from_input(RegionFile {
            regions: vec![region("XY", vec![vec![outer, hole]], false)],
        })
        .unwrap();
        assert_ne!(regions.resolve(0.25, 0.25, false).country, 0);
        assert_eq!(regions.resolve(1.0, 1.0, false).country, 0);
        assert_eq!(regions.resolve(0.0, 1.0, false).country, 0);
        assert_eq!(regions.resolve(f64::NAN, 1.0, true).country, 0);
    }
}

#[test]
fn coastal_country_requires_exactly_one_boundary_group_and_wraps_longitude() {
    let regions = Regions::from_input(RegionFile {
        regions: vec![
            region("XY", vec![vec![rectangle(179.99, -1.0, 180.0, 1.0)]], false),
            region("XZ", vec![vec![rectangle(0.03, -1.0, 0.04, 1.0)]], false),
            region("YZ", vec![vec![rectangle(0.0, -1.0, 0.01, 1.0)]], false),
        ],
    })
    .unwrap();
    assert_eq!(
        regions.resolve(0.0, -179.995, true).country,
        u16::from_le_bytes(*b"XY")
    );
    assert_eq!(regions.resolve(0.0, -179.995, false).country, 0);
    assert_eq!(regions.resolve(0.0, 0.02, true).country, 0);
    assert_eq!(regions.resolve(0.0, 30.0, true).country, 0);
}

#[test]
fn conflicting_land_interiors_are_unknown_independent_of_input_order() {
    for reverse in [false, true] {
        let mut input = vec![
            region("XY", vec![vec![rectangle(0.0, 0.0, 2.0, 2.0)]], false),
            region("XZ", vec![vec![rectangle(1.0, 0.0, 3.0, 2.0)]], false),
        ];
        if reverse {
            input.reverse();
        }
        let regions = Regions::from_input(RegionFile { regions: input }).unwrap();
        for coastal in [false, true] {
            assert_eq!(regions.resolve(1.0, 1.5, coastal).country, 0);
            assert_eq!(
                regions.resolve(1.0, 0.5, coastal).country,
                u16::from_le_bytes(*b"XY")
            );
        }
    }
}

#[test]
fn disputed_attribution_never_becomes_an_industrial_country() {
    let regions = Regions::from_input(RegionFile {
        regions: vec![region(
            "XY",
            vec![vec![rectangle(0.0, 0.0, 1.0, 1.0)]],
            true,
        )],
    })
    .unwrap();
    assert_ne!(regions.resolve(0.5, 0.5, true).country, 0);
    assert_eq!(regions.resolve(0.5, 0.5, false).country, 0);
}

#[test]
fn polar_polygon_uses_its_polar_holes_and_original_latitude_extent() {
    let cap = vec![
        [180.0, -80.0],
        [90.0, -80.0],
        [0.0, -80.0],
        [-90.0, -80.0],
        [-180.0, -80.0],
        [-180.0, -90.0],
        [180.0, -90.0],
        [180.0, -80.0],
    ];
    let regions = Regions::from_input(RegionFile {
        regions: vec![
            region("XY", vec![vec![rectangle(0.0, 0.0, 1.0, 1.0)]], false),
            region("XZ", vec![vec![cap]], false),
        ],
    })
    .unwrap();
    assert_eq!(
        regions.resolve(-85.0, 0.0, false).country,
        u16::from_le_bytes(*b"XZ")
    );
    assert_eq!(regions.resolve(-70.0, 0.0, false).country, 0);
}
