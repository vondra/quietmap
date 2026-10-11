//! Pins of the ray: the platform clamp, ground-factor interpolation, the own footprint by id and
//! every footprint containing a building's source, overlapping roofs, the flat-ground transfer
//! with state mixing and air absorption, and the foliage of each state.

use super::*;
use crate::atmosphere::ALPHA_DB_PER_KM;

fn assert_close(got: &[f64], want: &[f64]) {
    assert_eq!(got.len(), want.len(), "length {got:?} vs {want:?}");
    for (g, w) in got.iter().zip(want) {
        assert!((g - w).abs() < 1e-9, "{g} vs {w}");
    }
}

fn profile() -> Profile {
    // 100 m, terrain rising to 105 m at x = 30 m, ground factor 1/0.5/0.4/0/0.5/1.
    Profile {
        horizontal_m: 100.0,
        t: vec![0.0, 0.25, 0.3, 0.5, 0.75, 1.0],
        ground_m: vec![100.0, 102.0, 105.0, 101.0, 103.0, 100.0],
        ground_factor: vec![1.0, 0.5, 0.4, 0.0, 0.5, 1.0],
        forest_cover: vec![0.0; 6],
        canopy_m: vec![0.0; 6],
    }
}

fn ends() -> RayEnds {
    RayEnds {
        source_height_m: 4.0,
        source_ground_m: None,
        receiver_altitude_m: 104.0,
        source_ground_factor: Some(0.5),
        platform_half_width_m: 30.0,
        own_footprint: 1,
    }
}

fn building(t: f64, height_m: f64, footprint_id: u64) -> Crossing {
    Crossing {
        t,
        height_m,
        building: true,
        footprint_id,
    }
}

fn barrier(t: f64, height_m: f64, footprint_id: u64) -> Crossing {
    Crossing {
        t,
        height_m,
        building: false,
        footprint_id,
    }
}

/// The platform clamps terrain above the source ground within its half-width (x = 30 m itself
/// stays: the gate is strict); the path carries the source and receiver altitudes.
#[test]
fn the_platform_clamps_near_terrain_and_ground_interpolates() {
    let profile = profile();
    let mut buffers = PathBuffers::default();
    buffers.fill(&profile, &[], &ends());
    let path = buffers.path(&profile, &ends(), 104.0);
    assert_close(
        path.profile.distance_m,
        &[0.0, 25.0, 30.0, 50.0, 75.0, 100.0],
    );
    assert_close(
        path.profile.altitude_m,
        &[100.0, 100.0, 105.0, 101.0, 103.0, 100.0],
    );
    assert_close(path.profile.ground_factor, &[1.0, 0.5, 0.4, 0.0, 0.5, 1.0]);
    assert_eq!((path.source, path.receiver), ((0.0, 104.0), (100.0, 104.0)));
    assert!((path.source_ground_factor - 0.5).abs() < 1e-12);
    assert_eq!(path.terrain_candidates.len(), 6);
    assert!(path.obstacle_tops.is_empty());
}

/// A source on a bridge's deck stands on the deck, not on the valley under it: the clamp leaves
/// the valley in the profile, and the ray runs from the deck plus the source's height.
#[test]
fn a_source_on_a_deck_stands_on_it_above_the_valley() {
    let profile = profile();
    let deck = RayEnds {
        source_ground_m: Some(150.0),
        ..ends()
    };
    let mut buffers = PathBuffers::default();
    buffers.fill(&profile, &[], &deck);
    let path = buffers.path(&profile, &deck, 154.0);
    assert_close(
        path.profile.altitude_m,
        &[100.0, 102.0, 105.0, 101.0, 103.0, 100.0],
    );
    let terms = ray_terms(
        &profile,
        &[],
        &deck,
        ([0.0; PERIODS], &*ALPHA_DB_PER_KM),
        &mut RayScratch::default(),
    );
    assert!((terms.transfer.slant_m - 100f64.hypot(50.0)).abs() < 1e-9);
}

/// The source's own building (by id, both walls, wherever they are) never screens it; every other
/// crossing is a top at the interpolated terrain plus its height.
#[test]
fn own_footprint_crossings_are_skipped_and_tops_stand_on_interpolated_terrain() {
    let profile = profile();
    let crossings = [
        building(0.05, 6.0, 1),
        building(0.1, 5.0, 3),
        building(0.12, 5.0, 3),
        barrier(0.6, 3.0, 2),
        building(0.9, 6.0, 1),
        barrier(1.0, 4.0, 4),
    ];
    let mut buffers = PathBuffers::default();
    buffers.fill(&profile, &crossings, &ends());
    let path = buffers.path(&profile, &ends(), 104.0);
    // Terrain at 10 m is 100.0 (the platform), at 12 m 100.0, at 60 m 101.8, at 100 m 100.0.
    assert_eq!(path.obstacle_tops.len(), 4);
    for (got, want) in
        path.obstacle_tops
            .iter()
            .zip([(10.0, 105.0), (12.0, 105.0), (60.0, 104.8), (100.0, 104.0)])
    {
        assert!(
            (got.0 - want.0).abs() < 1e-9 && (got.1 - want.1).abs() < 1e-9,
            "{got:?} vs {want:?}"
        );
    }
}

/// A building's source inside another outline of the same building too (a part mapped twice):
/// the ray leaves that footprint once, so it contains the source and does not screen it; a
/// footprint the ray enters and leaves still screens. Other sources keep every crossing.
#[test]
fn footprints_containing_a_buildings_source_do_not_screen_it() {
    let profile = profile();
    let crossings = [
        building(0.02, 20.0, 5),
        building(0.05, 20.0, 1),
        building(0.4, 8.0, 6),
        building(0.45, 8.0, 6),
    ];
    let mut buffers = PathBuffers::default();
    buffers.fill(&profile, &crossings, &ends());
    let tops: Vec<f64> = buffers
        .path(&profile, &ends(), 104.0)
        .obstacle_tops
        .iter()
        .map(|top| top.0)
        .collect();
    assert_close(&tops, &[40.0, 45.0]);
    let road = RayEnds {
        own_footprint: 0,
        ..ends()
    };
    buffers.fill(&profile, &crossings, &road);
    assert_eq!(buffers.path(&profile, &road, 104.0).obstacle_tops.len(), 4);
}

/// Overlapping footprints never stack: roofs clip in closing order (earliest exit wins), a roof
/// ending where cover already ends is dropped, the clipped top interpolates the slope.
#[test]
fn overlapping_roofs_clip_in_closing_order_and_equal_exits_drop() {
    let profile = Profile {
        horizontal_m: 100.0,
        t: vec![0.0, 0.5, 1.0],
        ground_m: vec![100.0, 100.0, 100.0],
        ground_factor: vec![1.0, 0.5, 0.0],
        forest_cover: vec![0.0; 3],
        canopy_m: vec![0.0; 3],
    };
    let crossings = [
        building(0.2, 6.0, 11),
        building(0.4, 6.0, 11),
        building(0.3, 8.0, 12),
        building(0.5, 10.0, 12),
        building(0.25, 7.0, 13),
        building(0.4, 7.0, 13),
    ];
    let ends = RayEnds {
        own_footprint: 0,
        ..ends()
    };
    let mut buffers = PathBuffers::default();
    buffers.fill(&profile, &crossings, &ends);
    let path = buffers.path(&profile, &ends, 104.0);
    assert_close(
        path.profile.distance_m,
        &[0.0, 20.0, 20.0, 40.0, 40.0, 40.0, 40.0, 50.0, 50.0, 100.0],
    );
    assert_close(
        path.profile.altitude_m,
        &[
            100.0, 100.0, 106.0, 106.0, 100.0, 100.0, 109.0, 110.0, 100.0, 100.0,
        ],
    );
    assert_close(
        path.profile.ground_factor,
        &[1.0, 0.8, 0.0, 0.0, 0.6, 0.6, 0.0, 0.0, 0.5, 0.0],
    );
}

/// Flat ground at 200 m with G = 0.5, read at ten even steps.
fn flat(horizontal_m: f64) -> Profile {
    let mut profile = Profile::default();
    profile.clear(horizontal_m);
    for step in 0..=10 {
        profile.push(f64::from(step) / 10.0, 200.0, 0.5, 0.0, 0.0);
    }
    profile
}

/// Flat 110.54 m over G = 0.5, source 1 m, receiver 4 m: the 1 kHz homogeneous band dips to 0.91
/// (ground interference) while favourable stays constructive at 1.41 (dev4 pins to 1e-9); p mixes
/// the states in energy and air absorption multiplies by 10^(-alpha d / 10).
#[test]
fn flat_ground_transfer_mixes_states_and_absorbs_air() {
    let ends = RayEnds {
        source_height_m: 1.0,
        source_ground_m: None,
        receiver_altitude_m: 204.0,
        source_ground_factor: Some(0.5),
        platform_half_width_m: 0.0,
        own_footprint: 0,
    };
    let profile = flat(110.54);
    let mut scratch = RayScratch::default();
    let homogeneous = ray_terms(
        &profile,
        &[],
        &ends,
        ([0.0; PERIODS], &*ALPHA_DB_PER_KM),
        &mut scratch,
    )
    .transfer;
    let favourable = ray_terms(
        &profile,
        &[],
        &ends,
        ([1.0; PERIODS], &*ALPHA_DB_PER_KM),
        &mut scratch,
    )
    .transfer;
    let mixed = ray_terms(
        &profile,
        &[],
        &ends,
        ([0.5, 0.0, 1.0], &*ALPHA_DB_PER_KM),
        &mut scratch,
    )
    .transfer;
    let slant = 110.54f64.hypot(3.0);
    assert!((homogeneous.slant_m - slant).abs() < 1e-12);
    let air = |band: usize| 10f64.powf(-ALPHA_DB_PER_KM[band] * slant / 1000.0 / 10.0);
    assert!((homogeneous.periods[0][4] / air(4) - 0.9097167331685742).abs() < 1e-9);
    assert!((favourable.periods[0][4] / air(4) - 1.4125375446227544).abs() < 1e-9);
    assert!((homogeneous.periods[0][0] / air(0) - 1.4125375446227544).abs() < 1e-9);
    for band in 0..BANDS {
        let expected = 0.5 * homogeneous.periods[0][band] + 0.5 * favourable.periods[0][band];
        assert!(
            (mixed.periods[0][band] - expected).abs() < 1e-12,
            "band {band}"
        );
        assert_eq!(mixed.periods[1][band], homogeneous.periods[1][band]);
        assert_eq!(mixed.periods[2][band], favourable.periods[2][band]);
    }
}

/// Crowns of full cover 20 m high under the whole flat 110.54 m ray take Table A.1 for its slant
/// length off the calm state, and for its arc, 6 cm longer, off the downwind one.
#[test]
fn a_forest_under_the_ray_takes_table_a1_off_each_state() {
    let ends = RayEnds {
        source_height_m: 1.0,
        source_ground_m: None,
        receiver_altitude_m: 204.0,
        source_ground_factor: Some(0.5),
        platform_half_width_m: 0.0,
        own_footprint: 0,
    };
    let mut scratch = RayScratch::default();
    let mut states = |profile: &Profile| {
        ray_terms(
            profile,
            &[],
            &ends,
            ([0.5; PERIODS], &*ALPHA_DB_PER_KM),
            &mut scratch,
        )
        .transfer
        .states
    };
    let open = states(&flat(110.54));
    let mut forest = flat(110.54);
    forest.forest_cover.fill(1.0);
    forest.canopy_m.fill(20.0);
    let wooded = states(&forest);
    let table = crate::foliage::attenuation_db(110.54f64.hypot(3.0));
    for (state, tolerance_db) in [(0, 1e-9), (1, 0.01)] {
        for band in 0..BANDS {
            let taken_db = 10.0 * (open[state][band] / wooded[state][band]).log10();
            assert!(
                (taken_db - table[band]).abs() < tolerance_db,
                "state {state} band {band}: {taken_db} vs {}",
                table[band]
            );
        }
    }
}

/// A 6 m wall halfway on a 200 m hard path takes well over 10 dB at 1 kHz from the direct field.
#[test]
fn a_wall_between_source_and_receiver_screens() {
    let mut profile = flat(200.0);
    profile.ground_factor.fill(0.0);
    let ends = RayEnds {
        source_height_m: 0.05,
        source_ground_m: None,
        receiver_altitude_m: 204.0,
        source_ground_factor: Some(0.0),
        platform_half_width_m: 0.0,
        own_footprint: 0,
    };
    let mut scratch = RayScratch::default();
    let open = ray_terms(
        &profile,
        &[],
        &ends,
        ([0.5; PERIODS], &*ALPHA_DB_PER_KM),
        &mut scratch,
    )
    .transfer;
    let walled = ray_terms(
        &profile,
        &[barrier(0.5, 6.0, 9)],
        &ends,
        ([0.5; PERIODS], &*ALPHA_DB_PER_KM),
        &mut scratch,
    )
    .transfer;
    let loss_db = 10.0 * (open.periods[0][4] / walled.periods[0][4]).log10();
    assert!(loss_db > 10.0, "{loss_db}");
}

/// Within 300 m of either end every sample may diffract; beyond it the highest of each 8 (the first
/// of equals), an open group's highest where the middle ends.
#[test]
fn the_middle_of_a_long_ray_diffracts_at_the_highest_of_each_eight() {
    let length = 1000.0;
    let terrain: Vec<PlanePoint> = (0..=100)
        .map(|k| {
            let x = 10.0 * f64::from(k);
            (x, if k % 8 == 3 { 5.0 } else { 0.0 })
        })
        .collect();
    let mut crests = Vec::new();
    crests_of(&terrain, length, &mut crests);
    let middle = |x: f64| x > CREST_ENDS_M && x < length - CREST_ENDS_M;
    assert!(
        terrain
            .iter()
            .filter(|p| !middle(p.0))
            .all(|p| crests.contains(p))
    );
    let in_middle: Vec<PlanePoint> = crests.iter().copied().filter(|p| middle(p.0)).collect();
    // 39 middle samples (310-690 m) in groups of 8: four full groups and one of 7.
    assert_eq!(in_middle.len(), 5);
    assert!(in_middle[..4].iter().all(|p| p.1 == 5.0));
}
