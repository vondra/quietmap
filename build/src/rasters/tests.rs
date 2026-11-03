//! Raster regressions for temporal masks, native rounding, coverage and publication safety.
use super::*;

fn raster_source(root: &Path, name: &str, band: Band, data: Vec<f64>) -> Source {
    let step = 1.0 / NODES_PER_DEGREE as f64;
    let dataset = warp::memory(
        2,
        2,
        [-step / 2.0, step / 2.0, 0.0, step / 2.0, 0.0, -step / 2.0],
        "EPSG:4326",
        data,
    )
    .unwrap();
    let driver = gdal::DriverManager::get_driver_by_name("GTiff").unwrap();
    let path = root.join(name);
    let mut stored = dataset
        .create_copy(&driver, &path, &Default::default())
        .unwrap();
    stored.rasterband(1).unwrap().set_scale(0.1).unwrap();
    stored.flush_cache().unwrap();
    drop(stored);
    let spec = config::RasterSource {
        input: name.into(),
        band,
        horizontal_crs: "EPSG:4326".into(),
        vertical_crs: (band == Band::GroundHeight).then_some(3855),
        nodata: Some(f64::NAN),
        priority: 0,
        classes: Default::default(),
        feather_nodes: 0,
        datum_horizontal_crs: None,
        datum_area: None,
        loss_base_year: (band == Band::TreeLoss).then_some(2000),
        loss_input: None,
        global_nearest: false,
        antimeridian_input: None,
    };
    Source::open(spec, path, 2000).unwrap()
}

fn one_node() -> Window {
    Window {
        north_node: 0,
        west_node: 0,
        rows: 1,
        columns: 1,
    }
}

#[test]
fn forest_loss_is_cut_before_maximum_resampling() {
    let root = tempfile::tempdir().unwrap();
    let source = raster_source(
        root.path(),
        "loss.tif",
        Band::TreeLoss,
        vec![24.0, 25.0, 0.0, 25.0],
    );
    let sample = |cutoff| {
        forest::forest_loss(&source, one_node(), 2000, loss_year_before(cutoff).unwrap()).unwrap()
            [0]
    };
    assert_eq!(sample("2024-01-01"), 0.0);
    assert_eq!(sample("2024-07-01"), 0.0);
    assert_eq!(sample("2025-01-01"), 1.0);
    assert_eq!(sample("2026-01-01"), 1.0);
}

#[test]
fn raw_height_metres_ignore_erroneous_scale_and_round_half_up() {
    let root = tempfile::tempdir().unwrap();
    let source = raster_source(
        root.path(),
        "height.tif",
        Band::GroundHeight,
        vec![100.1; 4],
    );
    let output = dem::build(&[source], one_node()).unwrap();
    assert_eq!(u16::from_le_bytes(output.try_into().unwrap()), 3001);
}

#[test]
fn empty_national_mosaic_does_not_change_the_base_averaging_window() {
    let root = tempfile::tempdir().unwrap();
    let base = || raster_source(root.path(), "base.tif", Band::GroundHeight, vec![100.1; 4]);
    let expected = dem::build(&[base()], one_node()).unwrap();
    let mut empty = raster_source(
        root.path(),
        "empty.tif",
        Band::GroundHeight,
        vec![f64::NAN; 4],
    );
    empty.spec.feather_nodes = 256;
    assert_eq!(dem::build(&[base(), empty], one_node()).unwrap(), expected);
}

#[test]
fn canopy_mean_excludes_bare_ground_but_keeps_observed_zero() {
    let root = tempfile::tempdir().unwrap();
    let source = raster_source(
        root.path(),
        "trees.tif",
        Band::TreeHeight,
        vec![0.0, 10.0, 0.0, 20.0],
    );
    assert_eq!(canopy::build(&[source], one_node()).unwrap(), [15]);
    let source = raster_source(root.path(), "bare.tif", Band::TreeHeight, vec![0.0; 4]);
    assert_eq!(canopy::build(&[source], one_node()).unwrap(), [0]);
}

#[test]
fn coverage_wraps_dateline_and_absence_is_not_land() {
    let mut coverage = vec![0.0; 360 * 180];
    let window = Window {
        north_node: 3600,
        west_node: 180 * 3600,
        rows: 2,
        columns: 2,
    };
    assert!(!touches_land(&coverage, window));
    coverage[89 * 360] = 1.0;
    assert!(touches_land(&coverage, window));
}

#[test]
fn publication_is_idempotent_but_never_overwrites_different_bytes() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("dem.u16le");
    write(&path, &[1, 2]).unwrap();
    write(&path, &[1, 2]).unwrap();
    assert!(write(&path, &[3, 4]).is_err());
    assert_eq!(std::fs::read(path).unwrap(), [1, 2]);
}

#[test]
fn a_newer_survey_keeps_regrowth_and_loses_only_subsequent_observed_forest() {
    let root = tempfile::tempdir().unwrap();
    for (loss_year, expected) in [(22.0, 80), (23.0, 80), (24.0, 0), (25.0, 80)] {
        let mut cover = raster_source(root.path(), "cover.tif", Band::TreeCover, vec![80.0; 4]);
        cover.observed_year = 2023;
        cover.spec.loss_input = Some("loss.tif".into());
        let loss = raster_source(root.path(), "loss.tif", Band::TreeLoss, vec![loss_year; 4]);
        assert_eq!(
            forest::build(&[cover, loss], one_node(), 2024).unwrap(),
            [expected]
        );
    }
}

#[test]
fn shifted_geographic_pixels_read_identically_on_both_sides_of_the_dateline() {
    let root = tempfile::tempdir().unwrap();
    let mut source = raster_source(root.path(), "cyclic.tif", Band::TreeLoss, vec![24.0; 4]);
    let step = 1.0 / NODES_PER_DEGREE as f64;
    source
        .dataset
        .set_geo_transform(&[
            180.0 - step / 2.0,
            step / 2.0,
            0.0,
            step / 2.0,
            0.0,
            -step / 2.0,
        ])
        .unwrap();
    source.dataset.flush_cache().unwrap();
    for longitude in [-180, 180] {
        let window = Window {
            west_node: longitude * NODES_PER_DEGREE,
            ..one_node()
        };
        let values = warp::read(
            &source.dataset,
            window,
            "EPSG:4326",
            (None, 0.0),
            "near",
            "Byte",
            false,
        )
        .unwrap();
        assert_eq!(values, [24.0]);
    }
}

#[test]
fn normalized_source_metadata_must_match_the_manifest() {
    let root = tempfile::tempdir().unwrap();
    let source = raster_source(
        root.path(),
        "metadata.tif",
        Band::GroundHeight,
        vec![1.0; 4],
    );
    let mut spec = source.spec.clone();
    spec.nodata = None;
    assert!(
        Source::open(spec, source.path.clone(), 2000)
            .err()
            .unwrap()
            .contains("nodata differs")
    );
    let mut spec = source.spec.clone();
    spec.horizontal_crs = "EPSG:4269".into();
    assert!(
        Source::open(spec, source.path.clone(), 2000)
            .err()
            .unwrap()
            .contains("CRS differs")
    );
}
