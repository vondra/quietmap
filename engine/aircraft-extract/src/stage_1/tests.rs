//! Optional real-source integration (the terrain endpoint gate moved into
//! `segment::pair_segment_phase` and is pinned there).
use super::*;
use crate::source_adsb_tar::AdsbTarSource;
use tempfile::tempdir;

/// The departure field is the terrain under the leg's first point when it is
/// on the ground; a leg first seen airborne carries no field.
#[test]
fn departure_field_is_the_takeoff_roll_terrain_or_unknown() {
    assert_eq!(departure_field_elev_m(&[true, false], &[355.5, 360.0]), 355.5);
    assert!(departure_field_elev_m(&[false, false], &[355.5, 360.0]).is_nan());
    assert!(departure_field_elev_m(&[], &[]).is_nan());
}

/// Skips unless QM_FLIGHTS_CACHE (radius cache with 2025/2025-01-21) and
/// QM_PREPARED_DIR (the prepared data root, cf. PREPARED_DIR in
/// scripts/run-aircraft-extract.sh) are both set and present.
#[test]
fn end_to_end_one_day_against_real_dem() {
    let (Ok(cache), Ok(prepared)) = (
        std::env::var("QM_FLIGHTS_CACHE"),
        std::env::var("QM_PREPARED_DIR"),
    ) else {
        return;
    };
    if !std::path::Path::new(&cache)
        .join("2025/2025-01-21")
        .exists()
        || !std::path::Path::new(&prepared).exists()
    {
        return;
    }

    let work = tempdir().unwrap();
    let stage0_dir = work.path().join("flights");
    let stage1_dir = work.path().join("segments");
    std::fs::create_dir_all(&stage0_dir).unwrap();
    std::fs::create_dir_all(&stage1_dir).unwrap();

    crate::stage_0::run_stage_0(
        &AdsbTarSource::new(cache),
        None,
        "2025-01-21",
        &stage0_dir,
        work.path(),
        None,
    )
    .unwrap();

    let rasters = RealRasters::new(std::path::Path::new(&prepared));
    let n = run_stage_1(&stage0_dir, &stage1_dir, "2025-01-21", &rasters).unwrap();
    assert!(n > 1000, "got only {n} segments");
    let path = stage1_dir.join("2025-01-21.arrow");
    assert!(path.exists());
}
