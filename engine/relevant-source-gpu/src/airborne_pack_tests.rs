//! CUDA packing and exact cached terrain-horizon invariants.

use super::*;
#[test]
fn cuda_airborne_layout_and_original_horizon_entries() {
    assert_eq!(std::mem::size_of::<DeviceAirborneSource>(), 80);
    assert_eq!(std::mem::size_of::<DeviceAirborneReceiver>(), 32);
    assert_eq!(std::mem::offset_of!(DeviceAirborneReceiver, altitude), 24);
    struct Flat;
    impl RasterSampler for Flat {
        fn elevation(&self, _: f64, _: f64) -> f64 {
            0.0
        }
        fn ground_g(&self, _: f64, _: f64) -> f64 {
            0.0
        }
        fn building_enclosure(&self, _: f64, _: f64) -> f64 {
            0.0
        }
    }
    let rx =
        ReceiverScreening::build(50.0, 14.0, 4.0, &Flat, &ObstacleSet { indexes: vec![] }).unwrap();
    let packed = PackedScreening::new(std::slice::from_ref(&rx));
    assert_eq!(
        packed.terrain.len(),
        air::HORIZON_SECTORS * air::RECEIVER_HORIZON_BANDS
    );
    assert_eq!(
        packed.buildings.len(),
        air::BUILDING_LOCAL_HORIZON_SECTORS * air::BUILDING_LOCAL_HORIZON_BANDS
    );
    assert_eq!(packed.global_max, [u16::MAX]);
    for (index, entry) in rx.terrain.packed_sectors().iter().flatten().enumerate() {
        assert_eq!(
            packed.terrain[index],
            (u32::from(entry.0 as u16) << 16) | u32::from(entry.1)
        );
    }
}

#[test]
fn cached_dem_keeps_exact_horizons_across_tile_and_dateline_seams() {
    use grid::raster::RasterWindow;
    use raster_reader::{channel::Channel, RealRasters};
    for lon in [0.0, 179.999] {
        let root = tempfile::tempdir().unwrap();
        let mut squares = std::collections::HashSet::new();
        for lat in [-0.1, 0.0, 0.1] {
            for offset in [-0.2, 0.0, 0.2] {
                squares.insert(grid::square_of(lat, lon + offset));
            }
        }
        for square in squares {
            let path = Channel::Dem.path(root.path(), square);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let count = RasterWindow::for_square(square).cell_count();
            let bytes: Vec<_> = (0..count)
                .flat_map(|i| (100 + (i % 137) as i16).to_be_bytes())
                .collect();
            std::fs::write(path, bytes).unwrap();
        }
        let rasters = RealRasters::new(root.path());
        let obstacles = ObstacleSet { indexes: vec![] };
        for lat in [-0.001, 0.001] {
            let plain = ReceiverScreening::build(lat, lon, 204.0, &rasters, &obstacles).unwrap();
            let cached =
                ReceiverScreening::build_cached(lat, lon, 204.0, &rasters, &obstacles).unwrap();
            assert_eq!(
                plain.terrain.packed_sectors(),
                cached.terrain.packed_sectors()
            );
            assert_eq!(
                plain.terrain.max_sin_sq.to_bits(),
                cached.terrain.max_sin_sq.to_bits()
            );
            assert_eq!(
                plain.buildings.packed_sectors(),
                cached.buildings.packed_sectors()
            );
        }
        let missing = tempfile::tempdir().unwrap();
        assert!(ReceiverScreening::build_cached(
            0.0,
            lon,
            4.0,
            &RealRasters::new(missing.path()),
            &obstacles
        )
        .is_err());
    }
}
