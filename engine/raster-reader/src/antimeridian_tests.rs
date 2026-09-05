//! One geographic fixture checks the default, mmap and fused sampler contracts.

use crate::channel::Channel;
use crate::test_fixture::write_square;
use crate::{FusedGrid, RealRasters};
use noise_compute::propagation::path_profile::fill_t_values;
use noise_compute::propagation::PathProfile;
use noise_compute::types::RasterSampler;

struct RasterFixture {
    _root: tempfile::TempDir,
    rasters: RealRasters,
}

impl RasterFixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        for lon in [179.9, -179.9, 15.0] {
            let square = grid::square_of(0.5, lon);
            for channel in Channel::ALL {
                write_square(root.path(), channel, square, |_, longitude| match channel {
                    Channel::Dem => {
                        let centre = if lon == 15.0 { 15 * 3600 } else { 180 * 3600 };
                        (7200 + (longitude - centre + 648000).rem_euclid(1296000) - 648000) as i16
                    }
                    Channel::Forest => 55,
                    Channel::Imd => 37,
                });
            }
        }
        let rasters = RealRasters::new(root.path());
        Self {
            _root: root,
            rasters,
        }
    }
}

struct DefaultSampler<'a>(&'a RealRasters);

impl RasterSampler for DefaultSampler<'_> {
    fn elevation(&self, lat: f64, lon: f64) -> f64 {
        self.0.elevation(lat, lon)
    }
    fn ground_g(&self, lat: f64, lon: f64) -> f64 {
        self.0.ground_g(lat, lon)
    }
}

#[test]
fn every_profile_sampler_uses_the_same_short_arc_and_cadence() {
    let fixture = RasterFixture::new();
    let real = &fixture.rasters;
    let default = DefaultSampler(real);
    for (src_lon, rcv_lon, west, east, start_elevation, end_elevation) in [
        (179.9, -179.9, 179.89, -179.89, 6840.0, 7560.0),
        (-179.9, 179.9, -180.11, -179.89, 7560.0, 6840.0),
        (14.9, 15.1, 14.89, 15.11, 6840.0, 7560.0),
        (15.1, 14.9, 14.89, 15.11, 7560.0, 6840.0),
    ] {
        let fused = FusedGrid::build(real, 0.49, 0.51, west, east);
        assert!(fused.geom().4 < 1000, "local halo must not span Greenwich");
        let dist_m = grid::geo::flat_dist(0.5, src_lon, 0.5, rcv_lon);
        assert!((22_000.0..23_000.0).contains(&dist_m));
        let mut expected_t = Vec::new();
        fill_t_values(dist_m, &mut expected_t);
        for (name, sampler, forest) in [
            ("default", &default as &dyn RasterSampler, 0),
            ("real", real as &dyn RasterSampler, 55),
            ("fused", &fused as &dyn RasterSampler, 55),
        ] {
            let mut profile = PathProfile::new();
            sampler.build_path_profile(0.5, src_lon, 0.5, rcv_lon, dist_m, &mut profile);
            assert_eq!(profile.t, expected_t, "{name} cadence");
            assert_eq!(profile.elevation_m.len(), expected_t.len());
            assert_eq!(
                profile.forest_u8,
                vec![forest; expected_t.len()],
                "{name} forest"
            );
            assert_eq!(profile.imd_u8, vec![37; expected_t.len()], "{name} ground");
            for (&t, &actual) in expected_t.iter().zip(&profile.elevation_m) {
                let expected = start_elevation + t * (end_elevation - start_elevation);
                assert!(
                    (f64::from(actual) - expected).abs() < f64::from(f32::EPSILON) * expected * 2.0,
                    "{name}: {src_lon}→{rcv_lon}, t={t}, expected={expected}, actual={actual}"
                );
            }
        }
    }
}

#[test]
fn longitude_aliases_share_cache_keys_and_both_halo_sides() {
    let fixture = RasterFixture::new();
    let real = &fixture.rasters;
    real.preload_bbox(0.49, 0.51, 179.89, -179.89);
    let east_halo = FusedGrid::build(real, 0.49, 0.51, 179.89, 180.11);
    let west_halo = FusedGrid::build(real, 0.49, 0.51, -180.11, -179.89);
    let mut key = (i32::MIN, i32::MIN);
    let mut tile = None;
    for (lon, expected) in [
        (180.0, 7200.0),
        (-180.0, 7200.0),
        (180.1, 7560.0),
        (-179.9, 7560.0),
        (-180.1, 6840.0),
        (179.9, 6840.0),
        (540.0, 7200.0),
        (-540.0, 7200.0),
    ] {
        for actual in [
            real.elevation(0.5, lon),
            real.elevation_nearest(0.5, lon),
            real.dem.sample_cached(0.5, lon, &mut key, &mut tile),
            real.elevation_nearest_cached(0.5, lon, &mut key, &mut tile),
            east_halo.elevation(0.5, lon),
            west_halo.elevation(0.5, lon),
            f64::from(east_halo.lookup_fused(0.5, lon).0),
        ] {
            assert!(
                (actual - expected).abs() < 0.0001,
                "lon={lon}: {actual} != {expected}"
            );
        }
        assert!((0..512).contains(&key.1));
        assert!((east_halo.ground_g(0.5, lon) - 0.63).abs() < 1e-12);
        assert!((west_halo.ground_g(0.5, lon) - 0.63).abs() < 1e-12);
    }
    let touches = real.dem.cache_touch_count();
    real.dem.sample_cached(0.5, 180.0, &mut key, &mut tile);
    real.dem.sample_cached(0.5, -180.0, &mut key, &mut tile);
    assert_eq!(
        real.dem.cache_touch_count(),
        touches,
        "aliases must retain the warm mmap"
    );
}
