//! Horizon edges: the steepest edge of a band wins, only edges nearer than the aircraft screen,
//! and the loss is the kernel's single-edge loss.

use super::*;

#[test]
fn sectors_and_bands_follow_direction_and_range() {
    assert_eq!(sector_of([1.0, 0.001, 0.0], 32), 0);
    assert_eq!(sector_of([0.0, 1.0, 0.0], 32), 8);
    assert_eq!(sector_of([-1.0, -0.001, 0.0], 32), 16);
    assert_eq!(sector_of([0.001, -1.0, 0.0], 256), 192);
    assert_eq!(sector_of([1.0, -0.001, 0.0], 256), 255);
    assert_eq!(band_of(30.0, &TERRAIN_BAND_LIMITS_M), 0);
    assert_eq!(band_of(750.0, &TERRAIN_BAND_LIMITS_M), 1);
    assert_eq!(band_of(9_000.0, &TERRAIN_BAND_LIMITS_M), 5);
    let east = sector_direction(0, 32);
    assert!(east[0] > 0.99 && east[1] > 0.0);
}

/// A ridge 40 m high 1 km east screens a low aircraft 5 km east, not one 5 km west nor one before
/// the ridge; the steeper of two edges in one band is kept.
#[test]
fn an_edge_screens_only_what_lies_behind_it() {
    let mut edges: SectorEdges<6> = SectorEdges::new(TERRAIN_SECTORS);
    edges.offer(
        0,
        1,
        Edge {
            range_m: 900.0,
            tangent: 0.02,
        },
    );
    edges.offer(
        0,
        1,
        Edge {
            range_m: 1_000.0,
            tangent: 0.04,
        },
    );
    let behind = edges.loss_db([5_000.0, 10.0, 150.0]);
    let expected = edge_loss_db(1_000.0, 40.0, 5_000.0f64.hypot(10.0), 150.0);
    assert!(
        behind > 0.0 && (behind - expected).abs() < 1e-9,
        "{behind} vs {expected}"
    );
    assert_eq!(edges.loss_db([-5_000.0, -10.0, 150.0]), 0.0);
    assert_eq!(edges.loss_db([800.0, 1.0, 10.0]), 0.0);
    // High enough to clear the ridge's line of sight: no loss.
    assert_eq!(edges.loss_db([5_000.0, 10.0, 400.0]), 0.0);
}
