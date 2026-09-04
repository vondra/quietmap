//! One geographic fixture checks the default, mmap and fused sampler contracts.

use crate::tile::{DType, Interp, TileStore};
use crate::{FusedGrid, RealRasters};
use noise_compute::propagation::path_profile::fill_t_values;
use noise_compute::propagation::PathProfile;
use noise_compute::types::RasterSampler;
use std::path::PathBuf;

struct RasterFixture {
    root: PathBuf,
    rasters: RealRasters,
}

impl RasterFixture {
    fn new(name: &str) -> Self {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "qm-raster-antimeridian-{}-{name}-{unique}",
            std::process::id()
        ));
        for layer in ["dem", "forest", "imd"] {
            std::fs::create_dir_all(root.join(layer)).unwrap();
        }
        for (tile, first_elevation) in [
            ("N00E179", 100_i16),
            ("N00W180", 200),
            ("N00E014", 100),
            ("N00E015", 200),
            ("N00E000", 900),
        ] {
            let elevation: Vec<u8> = (0..101)
                .flat_map(|_| (0..101).flat_map(|column| (first_elevation + column).to_be_bytes()))
                .collect();
            std::fs::write(root.join("dem").join(format!("{tile}.hgt")), elevation).unwrap();
            for (layer, value) in [("forest", 55), ("imd", 37)] {
                let value = if first_elevation == 900 { 99 } else { value };
                std::fs::write(
                    root.join(layer).join(format!("{tile}.raw")),
                    vec![value; 101 * 101],
                )
                .unwrap();
            }
        }
        let rasters = RealRasters {
            dem: TileStore::new(
                root.join("dem"),
                101,
                DType::I16BE,
                Interp::Bilinear,
                -1.0,
                ".hgt",
                4,
            ),
            forest: TileStore::new(
                root.join("forest"),
                101,
                DType::U8,
                Interp::Nearest,
                0.0,
                ".raw",
                4,
            ),
            imd: TileStore::new(
                root.join("imd"),
                101,
                DType::U8,
                Interp::Bilinear,
                100.0,
                ".raw",
                4,
            ),
        };
        Self { root, rasters }
    }
}

impl Drop for RasterFixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
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
    let fixture = RasterFixture::new("profiles");
    let real = &fixture.rasters;
    let default = DefaultSampler(real);
    for (src_lon, rcv_lon, west, east, start_elevation, end_elevation) in [
        (179.9, -179.9, 179.89, -179.89, 190.0, 210.0),
        (-179.9, 179.9, -180.11, -179.89, 210.0, 190.0),
        (14.9, 15.1, 14.89, 15.11, 190.0, 210.0),
        (15.1, 14.9, 14.89, 15.11, 210.0, 190.0),
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
                    (f64::from(actual) - expected).abs() < 0.0001,
                    "{name}: {src_lon}→{rcv_lon}, t={t}, expected={expected}, actual={actual}"
                );
            }
        }
    }
}

#[test]
fn longitude_aliases_share_cache_keys_and_both_halo_sides() {
    let fixture = RasterFixture::new("cache");
    let real = &fixture.rasters;
    real.preload_bbox(0.49, 0.51, 179.89, -179.89);
    let east_halo = FusedGrid::build(real, 0.49, 0.51, 179.89, 180.11);
    let west_halo = FusedGrid::build(real, 0.49, 0.51, -180.11, -179.89);
    let mut key = (i32::MIN, i32::MIN);
    let mut tile = None;
    for (lon, expected) in [
        (180.0, 200.0),
        (-180.0, 200.0),
        (180.1, 210.0),
        (-179.9, 210.0),
        (-180.1, 190.0),
        (179.9, 190.0),
        (540.0, 200.0),
        (-540.0, 200.0),
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
        assert!((-180..180).contains(&key.1));
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
