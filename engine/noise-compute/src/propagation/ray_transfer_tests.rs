//! Direct pins for the ray transfer: dB↔linear units, homogeneous/favourable mixing,
//! slant geometry, air absorption, the no-atmosphere hypothesis, and the trace detail.

use super::*;
use crate::propagation::air_absorption::AbsorptionClimate;
use crate::propagation::meteorology::{Meteorology, DIRECTION_SECTOR_COUNT};
use crate::propagation::obstacle_index::{ObstacleIndex, ObstacleKind, ObstacleSet};
use crate::types::{RasterSampler, NUM_BANDS};

/// dB to linear energy and back: 0 dB is 1, 10 dB is 0.1, −6 dB is 10^0.6.
#[test]
fn energy_converts_db_to_linear_energy() {
    assert!((energy(0.0) - 1.0).abs() < 1e-12);
    assert!((energy(10.0) - 0.1).abs() < 1e-12);
    assert!((energy(20.0) - 0.01).abs() < 1e-14);
    assert!((energy(-6.0) - 10f64.powf(0.6)).abs() < 1e-12);
}

/// State mixing is the energy mean: p = 0 keeps homogeneous, p = 1 keeps favourable, p = 0.5 of
/// 0 dB and 10 dB is −10·lg 0.55 ≈ 2.596 dB.
#[test]
fn mixed_db_is_the_p_weighted_energy_mean() {
    let homogeneous = [0.0; NUM_BANDS];
    let favourable = [10.0; NUM_BANDS];
    for band in mixed_db(0.0, &homogeneous, &favourable) {
        assert!((band - 0.0).abs() < 1e-12);
    }
    for band in mixed_db(1.0, &homogeneous, &favourable) {
        assert!((band - 10.0).abs() < 1e-12);
    }
    let expected = -10.0 * 0.55f64.log10();
    for band in mixed_db(0.5, &homogeneous, &favourable) {
        assert!((band - expected).abs() < 1e-12, "{band} vs {expected}");
    }
}

struct FlatRasters;

impl RasterSampler for FlatRasters {
    fn elevation(&self, _: f64, _: f64) -> f64 {
        200.0
    }
    fn ground_g(&self, _: f64, _: f64) -> f64 {
        0.5
    }
}

fn weather(p: f64, absorption_db_per_km: f64) -> Meteorology {
    let mut meteorology = Meteorology::defaults();
    meteorology.favourable_probability = [[p; DIRECTION_SECTOR_COUNT]; 3];
    meteorology.absorption = [[AbsorptionClimate::steady(absorption_db_per_km); NUM_BANDS]; 3];
    meteorology
}

fn receiver() -> RayReceiver {
    RayReceiver { lat: 50.0, lon: 14.0, altitude_m: 204.0 }
}

fn source() -> RaySource {
    RaySource {
        lat: 50.001,
        lon: 14.0,
        height_m: 1.0,
        ground: SourceGround::Fixed(0.5),
        platform_half_width_m: 0.0,
        exclusion_radius_m: 0.0,
    }
}

fn transfer(p: f64, absorption_db_per_km: f64) -> RayTransfer {
    evaluate_ray_transfer(
        &receiver(),
        &source(),
        &ObstacleSet::empty(),
        false,
        &FlatRasters,
        &weather(p, absorption_db_per_km),
        true,
        &mut RayScratch::default(),
        None,
    )
}

/// Slant geometry is exact; the p = 0.5 transfer is the energy mean of p = 0 and p = 1;
/// absorption multiplies by 10^(−α·d/10); without it the no-atmosphere hypothesis equals full.
#[test]
fn slant_p_mixing_and_absorption_follow_the_ray_geometry() {
    let horizontal_m = grid::geo::flat_dist(50.001, 14.0, 50.0, 14.0);
    let expected_slant = horizontal_m.hypot(204.0 - 201.0);
    let clear = transfer(0.0, 0.0);
    assert!((clear.slant_distance_m - expected_slant).abs() < 1e-9, "{}", clear.slant_distance_m);
    let homogeneous = transfer(0.0, 0.0);
    let favourable = transfer(1.0, 0.0);
    let mixed = transfer(0.5, 0.0);
    // Flat 110 m, G = 0.5: the 1 kHz homogeneous band dips to 0.91 (ground interference) while
    // favourable stays constructive at 1.41; the other bands are 1.41 under both states.
    assert!((homogeneous.periods[0][VARIANT_FULL][4] - 0.9097167331685742).abs() < 1e-9);
    assert!((favourable.periods[0][VARIANT_FULL][4] - 1.4125375446227544).abs() < 1e-9);
    assert!((homogeneous.periods[0][VARIANT_FULL][0] - 1.4125375446227544).abs() < 1e-9);
    assert!((favourable.periods[0][VARIANT_FULL][0] - 1.4125375446227544).abs() < 1e-9);
    assert!(favourable.periods[0][VARIANT_FULL][4] > homogeneous.periods[0][VARIANT_FULL][4]);
    for band in 0..NUM_BANDS {
        let (e0, e1, e05) = (
            homogeneous.periods[0][VARIANT_FULL][band],
            favourable.periods[0][VARIANT_FULL][band],
            mixed.periods[0][VARIANT_FULL][band],
        );
        assert!((e05 - (0.5 * e1 + 0.5 * e0)).abs() < 1e-12 * e05.max(1e-30), "band {band}");
    }
    let absorbed = transfer(0.5, 2.0);
    let air = 10f64.powf(-2.0 * (expected_slant / 1000.0) / 10.0);
    for band in 0..NUM_BANDS {
        let ratio = absorbed.periods[0][VARIANT_FULL][band] / mixed.periods[0][VARIANT_FULL][band];
        assert!((ratio - air).abs() < 1e-9, "band {band}: {ratio} vs {air}");
        let no_atmosphere = absorbed.periods[0][VARIANT_NO_ATMOSPHERE][band];
        let full = absorbed.periods[0][VARIANT_FULL][band];
        assert!((no_atmosphere - full / air).abs() < 1e-9 * no_atmosphere.max(1e-30), "band {band}");
    }
    for band in 0..NUM_BANDS {
        assert_eq!(clear.periods[0][VARIANT_NO_ATMOSPHERE][band], clear.periods[0][VARIANT_FULL][band]);
        assert_eq!(clear.periods[0][VARIANT_NO_TERRAIN][band], clear.periods[0][VARIANT_FULL][band]);
        assert_eq!(clear.periods[0][VARIANT_NO_SCREENING][band], clear.periods[0][VARIANT_FULL][band]);
    }
}

/// A 6 m wall across a flat ray screens the 1 kHz band by several dB, names its edge with the
/// height above the direct chord, and reports the flat ground factor; edge fractions stay in
/// [0, 1].
#[test]
fn a_wall_across_the_ray_screens_and_names_its_edge() {
    let mut wall_index = ObstacleIndex::builder(50.0, 14.0);
    wall_index.add_polyline(&[(50.0005, 13.999), (50.0005, 14.001)], 6.0, ObstacleKind::Barrier, 0);
    let obstacles = ObstacleSet {
        indexes: vec![std::sync::Arc::new(wall_index.build())],
    };
    let mut detail = None;
    let transfer = evaluate_ray_transfer(
        &receiver(),
        &source(),
        &obstacles,
        true,
        &FlatRasters,
        &weather(0.5, 0.0),
        true,
        &mut RayScratch::default(),
        Some(&mut detail),
    );
    assert!(transfer.periods[0][VARIANT_FULL][4] < transfer.periods[0][VARIANT_NO_SCREENING][4]);
    let detail = detail.expect("detail");
    // A 6 m wall at 55 m of 110 m costs ~13 dB at 1 kHz (Fresnel); its top stands 3.5 m
    // above the 201 → 204 m chord.
    assert!((detail.screening_bands[4] - 12.921664269028527).abs() < 0.2, "{:?}", detail.screening_bands);
    assert!((detail.ground_factor - 0.5).abs() < 1e-9, "{}", detail.ground_factor);
    let edge = detail.obstacle.edge.expect("wall edge");
    assert_eq!(edge.height_m, 6.0);
    assert!((edge.screen_h_m - 3.5).abs() < 0.05, "{}", edge.screen_h_m);
    for point in &detail.terrain.edges {
        assert!((0.0..=1.0).contains(&point.t), "{}", point.t);
    }
    assert!((detail.source_altitude_m - 201.0).abs() < 1e-9);
}

/// Ground under the source derives from the source imperviousness: on G = 0.5 ground it
/// equals a fixed 0.5 ground factor, bit for bit.
#[test]
fn ground_under_the_source_matches_fixed_ground_on_uniform_g() {
    let fixed = transfer(0.5, 0.0);
    let under = evaluate_ray_transfer(
        &receiver(),
        &RaySource { ground: SourceGround::UnderSource, ..source() },
        &ObstacleSet::empty(),
        false,
        &FlatRasters,
        &weather(0.5, 0.0),
        true,
        &mut RayScratch::default(),
        None,
    );
    assert_eq!(under.periods, fixed.periods);
    assert_eq!(under.slant_distance_m, fixed.slant_distance_m);
}

/// The ray azimuth selects the favourable-probability sector: this ray runs south-southeast
/// (bearing ~176°, sectors 7-8), so p = 0/1 in sectors 7/8 mixes strictly between the uniform
/// transfers; the east offset also exercises the longitude term (zero east would hide `*`→`/`).
#[test]
fn the_ray_azimuth_selects_the_wind_sector() {
    let mut sectored = Meteorology::defaults();
    sectored.favourable_probability = [[0.0; DIRECTION_SECTOR_COUNT]; 3];
    for period in sectored.favourable_probability.iter_mut() {
        period[8] = 1.0;
    }
    sectored.absorption = [[AbsorptionClimate::steady(0.0); NUM_BANDS]; 3];
    let southeast = RaySource { lon: 14.0001, ..source() };
    let via_sector = evaluate_ray_transfer(
        &receiver(),
        &southeast,
        &ObstacleSet::empty(),
        false,
        &FlatRasters,
        &sectored,
        true,
        &mut RayScratch::default(),
        None,
    );
    // Sector 8 alone recovers uniform p = 1 on the due-south ray.
    let south = evaluate_ray_transfer(
        &receiver(),
        &source(),
        &ObstacleSet::empty(),
        false,
        &FlatRasters,
        &sectored,
        true,
        &mut RayScratch::default(),
        None,
    );
    assert_eq!(south.periods, transfer(1.0, 0.0).periods);
    // Off-south it mixes: strictly between the same ray's homogeneous and favourable 1 kHz.
    let southeast_homogeneous = evaluate_ray_transfer(
        &receiver(),
        &southeast,
        &ObstacleSet::empty(),
        false,
        &FlatRasters,
        &weather(0.0, 0.0),
        true,
        &mut RayScratch::default(),
        None,
    )
    .periods[0][VARIANT_FULL][4];
    let southeast_favourable = evaluate_ray_transfer(
        &receiver(),
        &southeast,
        &ObstacleSet::empty(),
        false,
        &FlatRasters,
        &weather(1.0, 0.0),
        true,
        &mut RayScratch::default(),
        None,
    )
    .periods[0][VARIANT_FULL][4];
    let mixed = via_sector.periods[0][VARIANT_FULL][4];
    assert!(
        mixed > southeast_homogeneous && mixed < southeast_favourable,
        "{mixed} vs {southeast_homogeneous}/{southeast_favourable}"
    );
    // Substantially below favourable: p ≈ 0.84 here, while a collapsed east term lands back in
    // sector 8 with p ≈ 1.
    assert!(
        southeast_favourable - mixed > 0.01,
        "{mixed} vs {southeast_favourable}"
    );
}

struct CanopyRasters;

impl RasterSampler for CanopyRasters {
    fn elevation(&self, _: f64, _: f64) -> f64 {
        200.0
    }
    fn ground_g(&self, _: f64, _: f64) -> f64 {
        0.5
    }
    fn build_path_profile(
        &self,
        src_lat: f64,
        src_lon: f64,
        rcv_lat: f64,
        rcv_lon: f64,
        dist_m: f64,
        out: &mut crate::propagation::PathProfile,
    ) {
        FlatRasters.build_path_profile(src_lat, src_lon, rcv_lat, rcv_lon, dist_m, out);
        for height in out.canopy_m.iter_mut() {
            *height = 10.0;
        }
        for cover in out.forest_u8.iter_mut() {
            *cover = 100;
        }
    }
}

/// A 10 m canopy along the whole ray attenuates every band: full sits below no-vegetation.
#[test]
fn canopy_foliage_attenuates_the_full_hypothesis() {
    let bare = transfer(0.5, 0.0);
    let shaded = evaluate_ray_transfer(
        &receiver(),
        &source(),
        &ObstacleSet::empty(),
        false,
        &CanopyRasters,
        &weather(0.5, 0.0),
        true,
        &mut RayScratch::default(),
        None,
    );
    for band in 0..NUM_BANDS {
        let full = shaded.periods[0][VARIANT_FULL][band];
        let no_vegetation = shaded.periods[0][VARIANT_NO_VEGETATION][band];
        assert!(full < no_vegetation, "band {band}: {full} vs {no_vegetation}");
        assert_eq!(no_vegetation, bare.periods[0][VARIANT_FULL][band]);
    }
}

/// Received variants apply A-weighting, the receiver reflection (every variant but free field),
/// and sum bands; the band energies keep the full variant's reflection.
#[test]
fn received_variants_weight_bands_and_lift_all_but_free_field() {
    let transfer: VariantBands = std::array::from_fn(|variant| {
        std::array::from_fn(|_| if variant == VARIANT_FREE_FIELD { 0.5 } else { 0.25 })
    });
    let emission = [60.0; NUM_BANDS];
    let variants = received_variants(&transfer, &emission, 3.0);
    let reflection = 10f64.powf(0.3);
    let source: f64 = (0..NUM_BANDS)
        .map(|band| 10f64.powf((60.0 + crate::constants::A_WEIGHTING[band]) / 10.0))
        .sum();
    assert!((variants.full_energy - source * 0.25 * reflection).abs() < 1e-6 * variants.full_energy);
    assert!((variants.free_field_energy - source * 0.5).abs() < 1e-6 * variants.free_field_energy);
    for band in 0..NUM_BANDS {
        let expected =
            10f64.powf((60.0 + crate::constants::A_WEIGHTING[band]) / 10.0) * 0.25 * reflection;
        assert!((variants.band_energy[band] - expected).abs() < 1e-9 * expected.max(1e-30));
    }
}
