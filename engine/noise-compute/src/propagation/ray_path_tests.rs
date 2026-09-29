//! Direct pins for the ray vertical path: the source platform clamp, ground-factor
//! interpolation, own-footprint exclusion, and overlapping-roof clipping.

use super::*;
use crate::propagation::obstacle_index::ObstacleKind;
use crate::propagation::PathProfile;

fn assert_close(got: &[f64], want: &[f64]) {
    assert_eq!(got.len(), want.len(), "length {got:?} vs {want:?}");
    for (g, w) in got.iter().zip(want) {
        assert!((g - w).abs() < 1e-9, "{g} vs {w}");
    }
}

fn assert_close_points(got: &[(f64, f64)], want: &[(f64, f64)]) {
    assert_eq!(got.len(), want.len(), "length {got:?} vs {want:?}");
    for (g, w) in got.iter().zip(want) {
        assert!((g.0 - w.0).abs() < 1e-9 && (g.1 - w.1).abs() < 1e-9, "{g:?} vs {w:?}");
    }
}

fn profile() -> PathProfile {
    // 100 m, terrain rising to 105 m at x = 30 m, imperviousness 0/50/60/100/50/0.
    PathProfile {
        t: vec![0.0, 0.25, 0.3, 0.5, 0.75, 1.0],
        elevation_m: vec![100.0, 102.0, 105.0, 101.0, 103.0, 100.0],
        imd_u8: vec![0, 50, 60, 100, 50, 0],
        dist_m: 100.0,
        ..Default::default()
    }
}

fn inputs() -> RayPathInputs {
    RayPathInputs {
        source_altitude_m: 104.0,
        receiver_altitude_m: 104.0,
        source_ground_factor: 0.5,
        platform_half_width_m: 30.0,
        exclusion_radius_m: 10.0,
    }
}

/// The platform clamps terrain above the source ground within its half-width (x = 30 m itself
/// stays: the gate is strict), ground factors interpolate the imperviousness, and the path carries
/// the source and receiver altitudes.
#[test]
fn the_platform_clamps_near_terrain_and_ground_interpolates_imperviousness() {
    let profile = profile();
    let mut buffers = RayPathBuffers::default();
    buffers.fill(&profile, &[], &inputs(), true);
    let path = buffers.path(&inputs(), true);
    assert_close(path.profile.distance_m, &[0.0, 25.0, 30.0, 50.0, 75.0, 100.0]);
    assert_close(path.profile.altitude_m, &[100.0, 100.0, 105.0, 101.0, 103.0, 100.0]);
    assert_close(path.profile.ground_factor, &[1.0, 0.5, 0.4, 0.0, 0.5, 1.0]);
    assert_close_points(&[path.source], &[(0.0, 104.0)]);
    assert_close_points(&[path.receiver], &[(100.0, 104.0)]);
    assert!((path.source_ground_factor - 0.5).abs() < 1e-12);
    assert_eq!(path.terrain_candidates.len(), 6);
    assert!(path.obstacle_tops.is_empty());
    assert!(buffers.path(&inputs(), false).terrain_candidates.is_empty());
}

/// Buildings nearer the source than the exclusion radius are its own footprint, not obstacles;
/// every other crossing is a top at the interpolated terrain plus its height. Barriers are never
/// excluded, and x = 10 m itself stays (strict gate).
#[test]
fn own_footprint_crossings_are_skipped_and_tops_stand_on_interpolated_terrain() {
    let profile = profile();
    let crossings = [
        CrossingCandidate { t: 0.4, height_m: 6.0, kind: ObstacleKind::Building, id: 1, index: 0 },
        CrossingCandidate { t: 0.05, height_m: 6.0, kind: ObstacleKind::Building, id: 1, index: 0 },
        CrossingCandidate { t: 0.6, height_m: 3.0, kind: ObstacleKind::Barrier, id: 2, index: 1 },
        CrossingCandidate { t: 0.1, height_m: 5.0, kind: ObstacleKind::Building, id: 3, index: 0 },
        CrossingCandidate { t: 1.0, height_m: 4.0, kind: ObstacleKind::Barrier, id: 4, index: 1 },
    ];
    let mut buffers = RayPathBuffers::default();
    buffers.fill(&profile, &crossings, &inputs(), true);
    let path = buffers.path(&inputs(), true);
    // Terrain at x = 40 m is 103.0, at 60 m 101.8, at 10 m and 100 m 100.0 (linear between samples).
    assert_close_points(
        path.obstacle_tops,
        &[(40.0, 109.0), (60.0, 104.8), (10.0, 105.0), (100.0, 104.0)],
    );
    buffers.fill(&profile, &crossings, &inputs(), false);
    assert!(buffers.path(&inputs(), true).obstacle_tops.is_empty());
}

/// Overlapping footprints never stack: roofs are clipped in closing order (earliest exit wins),
/// a roof ending where cover already ends is dropped, the clipped top interpolates the slope,
/// and wall grounds interpolate the imperviousness gradient off-sample.
#[test]
fn overlapping_roofs_clip_in_closing_order_and_equal_exits_drop() {
    let profile = PathProfile {
        t: vec![0.0, 0.5, 1.0],
        elevation_m: vec![100.0, 100.0, 100.0],
        imd_u8: vec![0, 50, 100],
        dist_m: 100.0,
        ..Default::default()
    };
    let crossings = [
        CrossingCandidate { t: 0.2, height_m: 6.0, kind: ObstacleKind::Building, id: 1, index: 0 },
        CrossingCandidate { t: 0.4, height_m: 6.0, kind: ObstacleKind::Building, id: 1, index: 0 },
        CrossingCandidate { t: 0.3, height_m: 8.0, kind: ObstacleKind::Building, id: 2, index: 0 },
        CrossingCandidate { t: 0.5, height_m: 10.0, kind: ObstacleKind::Building, id: 2, index: 0 },
        CrossingCandidate { t: 0.25, height_m: 7.0, kind: ObstacleKind::Building, id: 3, index: 0 },
        CrossingCandidate { t: 0.4, height_m: 7.0, kind: ObstacleKind::Building, id: 3, index: 0 },
    ];
    let mut buffers = RayPathBuffers::default();
    buffers.fill(&profile, &crossings, &inputs(), true);
    let path = buffers.path(&inputs(), true);
    // [20, 40] at 106 m keeps, [25, 40] drops (equal exit), [30, 50] at 108/110 m clips to
    // [40, 50] with its top interpolated to 109 m; wall grounds 0.8 at x = 20, 0.6 at x = 40.
    assert_close(
        path.profile.distance_m,
        &[0.0, 20.0, 20.0, 40.0, 40.0, 40.0, 40.0, 50.0, 50.0, 100.0],
    );
    assert_close(
        path.profile.altitude_m,
        &[100.0, 100.0, 106.0, 106.0, 100.0, 100.0, 109.0, 110.0, 100.0, 100.0],
    );
    assert_close(
        path.profile.ground_factor,
        &[1.0, 0.8, 0.0, 0.0, 0.6, 0.6, 0.0, 0.0, 0.5, 0.0],
    );
}
