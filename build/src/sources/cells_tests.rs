//! Site discretisation: ring areas, energy-conserving cells, exclusion radii and owner tiles.

use super::*;
use tiles::sources::{BANDS, GROUND_FROM_TERRAIN, Layer, PERIODS};

fn ring(corners: &[(f64, f64)]) -> Z30Ring {
    corners
        .iter()
        .map(|&(lat, lon)| degrees_to_z30(lat, lon))
        .collect()
}

fn attribute(level_db: f64) -> Attribute {
    Attribute {
        layer: Layer::Industry,
        height_m: 5.0,
        ground_percent: GROUND_FROM_TERRAIN,
        platform_half_width_m: 0.0,
        exclusion_radius_m: 0.0,
        footprint_id: 0,
        group_key: 1,
        emission: [[level_db; BANDS]; PERIODS],
        display: "[]".into(),
    }
}

#[test]
fn a_hundred_metre_square_holds_ten_thousand_square_metres() {
    let square = ring(&[
        (50.0, 14.0),
        (50.0, 14.001_394),
        (50.000_904, 14.001_394),
        (50.000_904, 14.0),
    ]);
    let area = ring_area_m2(&square).unwrap();
    assert!((9_000.0..11_000.0).contains(&area), "{area}");
    assert_eq!(resolve_area_m2(Some(123.0), &square, 7.0), 123.0);
    assert_eq!(resolve_area_m2(None, &square, 7.0), area);
    assert_eq!(resolve_area_m2(Some(0.0), &[], 7.0), 7.0);
    let bytes: Vec<u8> = [2i32, 5, 6, 7, 8]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    assert_eq!(decode_z30_ring(&bytes), Some(vec![(5, 6), (7, 8)]));
    assert_eq!(decode_z30_ring(&bytes[..bytes.len() - 1]), None);
}

/// A 445 x 429 m site in 75 m cells: the cells' areas sum to the site's, their energies to its
/// emission, and each keeps the exclusion radius of its own area.
#[test]
fn large_sites_split_into_cells_carrying_their_area_share_of_the_energy() {
    let corners = [
        (50.0, 14.0),
        (50.0, 14.006),
        (50.004, 14.006),
        (50.004, 14.0),
        (50.0, 14.0),
    ];
    let site_ring = ring(&corners);
    let area_m2 = ring_area_m2(&site_ring).unwrap();
    let site = Site {
        centroid: (50.002, 14.003),
        ring: &site_ring,
        area_m2,
        single_point_up_to_m2: 5_000.0,
        cell_m: 75.0,
    };
    let points = site_points(&site);
    assert!(points.len() >= 30, "{}", points.len());
    let total_m2: f64 = points.iter().map(|point| point.area_m2).sum();
    assert!((total_m2 - area_m2).abs() < 1e-6 * area_m2);
    let mut out = Vec::new();
    push_site_points(&points, area_m2, &attribute(80.0), &mut out);
    let energy: f64 = out
        .iter()
        .map(|c| 10f64.powf(c.attribute.emission[2][4] / 10.0))
        .sum();
    assert!((10.0 * energy.log10() - 80.0).abs() < 1e-9);
    for (converted, point) in out.iter().zip(&points) {
        let radius = (point.area_m2 / std::f64::consts::PI).sqrt();
        assert!((converted.attribute.exclusion_radius_m - radius).abs() < 1e-9);
        assert!(point.lat > 50.0 && point.lat < 50.004 && point.lon > 14.0 && point.lon < 14.006);
    }
}

#[test]
fn small_or_ringless_sites_emit_from_their_centroid_with_their_whole_area() {
    let small_ring = ring(&[
        (50.0, 14.0),
        (50.0, 14.0005),
        (50.0005, 14.0005),
        (50.0005, 14.0),
    ]);
    for (ring, area_m2) in [(&small_ring[..], 1_000.0), (&[][..], 20_000.0)] {
        let site = Site {
            centroid: (50.0, 14.0),
            ring,
            area_m2,
            single_point_up_to_m2: 2_000.0,
            cell_m: 30.0,
        };
        assert_eq!(
            site_points(&site),
            vec![SitePoint {
                lat: 50.0,
                lon: 14.0,
                area_m2
            }]
        );
    }
    let mut out = Vec::new();
    let whole = [SitePoint {
        lat: 50.0,
        lon: 14.0,
        area_m2: 400.0,
    }];
    push_site_points(&whole, 400.0, &attribute(60.0), &mut out);
    assert_eq!(out[0].attribute.emission, attribute(60.0).emission);
    assert!((out[0].attribute.exclusion_radius_m - 11.284).abs() < 1e-3);
}

#[test]
fn points_belong_to_the_tile_holding_them() {
    let (tile, ends) = point_piece(50.075_53, 14.437_81);
    assert_eq!(tile, TileId { x: 2212, y: 1387 });
    assert_eq!(ends[0], ends[1]);
    let (lat, lon) = tile.to_mercator(ends[0]).to_degrees();
    assert!((lat - 50.075_53).abs() < 3e-6 && (lon - 14.437_81).abs() < 3e-6);
    // On the edge between two tiles a point belongs to the east one, at its west edge.
    let edge = TileId { x: 2213, y: 1387 }
        .global([-16_384, 0])
        .to_mercator()
        .to_degrees();
    let (east, ends) = point_piece(edge.0, edge.1);
    assert_eq!((east, ends[0][0]), (TileId { x: 2213, y: 1387 }, -16_384));
}
