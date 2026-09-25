//! Synthetic terrain/canopy squares exercise strict profiles and the fused upload ABI.
use crate::{channel::Channel, test_fixture::write_square, CheckedRasters, FusedGrid, RealRasters};
use noise_compute::{propagation::PathProfile, types::RasterSampler};

#[test]
fn canopy_nodes_reach_both_profiles_and_missing_nodes_fail() {
    let root = tempfile::tempdir().unwrap();
    let square = grid::square_of(50.0, 14.0);
    for channel in Channel::ALL {
        write_square(root.path(), channel, square, |_, longitude| match channel {
            Channel::Dem => 457,
            Channel::Canopy => longitude.rem_euclid(251) as i16,
            _ => 50,
        });
    }
    let real = RealRasters::new(root.path());
    let fused = FusedGrid::build(&real, 49.99, 50.01, 13.99, 14.01);
    assert_eq!(std::mem::size_of::<crate::FusedPixel>(), 8);
    assert_eq!(std::mem::offset_of!(crate::FusedPixel, canopy_m), 6);
    let mut seed = 1729_u32;
    for _ in 0..100 {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        let lat = 50.0 + f64::from(seed % 25) / 3600.0;
        let lon = 14.0 + f64::from((seed >> 8) % 25) / 3600.0;
        let mut a = PathProfile::new();
        let mut b = PathProfile::new();
        real.build_path_profile(lat, lon, lat, lon, 0.0, &mut a);
        fused.build_path_profile(lat, lon, lat, lon, 0.0, &mut b);
        assert_eq!(a.canopy_m, b.canopy_m);
        assert_eq!(a.elevation_m, b.elevation_m);
    }
    write_square(root.path(), Channel::Canopy, square, |_, _| i16::MIN);
    let missing = RealRasters::new(root.path());
    let checked = CheckedRasters::new(&missing);
    let mut profile = PathProfile::new();
    checked.build_path_profile(50.0, 14.0, 50.001, 14.001, 130.0, &mut profile);
    assert!(profile.canopy_m.iter().all(|v| v.is_nan()));
    assert!(checked.ensure_valid().is_err());
}
