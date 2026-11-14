//! Raster regressions for temporal masks, native rounding, coverage, country extents and
//! publication safety.
use super::*;

#[test]
fn interrupted_raster_squares_resume_only_when_all_atomic_bands_are_complete() {
    let root = tempfile::tempdir().unwrap();
    for name in BANDS.into_iter().take(3) {
        write(&root.path().join(name), &[0]).unwrap();
    }
    assert!(!square_complete(root.path()).unwrap());
    assert!(square_receipt(root.path()).is_err());
    write(&root.path().join("canopy.u8"), &[]).unwrap();
    crate::pipeline_files::write_json(
        &root.path().join("complete.json"),
        &square_receipt(root.path()).unwrap(),
    )
    .unwrap();
    assert!(square_complete(root.path()).unwrap());
    std::fs::write(root.path().join("dem.u16le"), b"changed").unwrap();
    assert!(square_complete(root.path()).is_err());
}

fn raster_source(root: &Path, name: &str, band: Band, data: Vec<f64>) -> Source {
    let step = 1.0 / NODES_PER_DEGREE as f64;
    let corner = [-step / 2.0, step / 2.0];
    raster_source_on_grid(root, name, band, (corner, step / 2.0, 2, 2), data)
}

/// A one-band source of `columns` x `rows` pixels of `pixel` degrees from the north-west `corner`.
fn raster_source_on_grid(
    root: &Path,
    name: &str,
    band: Band,
    (corner, pixel, columns, rows): ([f64; 2], f64, usize, usize),
    data: Vec<f64>,
) -> Source {
    let dataset = warp::memory(
        columns,
        rows,
        [corner[0], pixel, 0.0, corner[1], 0.0, -pixel],
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
fn every_square_is_built_before_the_step_fails_with_all_its_failing_squares() {
    let built = std::sync::atomic::AtomicUsize::new(0);
    let squares: Vec<_> = (0..6).map(|x| Square { x, y: 7 }).collect();
    let error = build_every_square(
        &squares,
        || (),
        |_, square| {
            built.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            match square.x {
                1 | 4 => Err(format!("datum {}", square.x)),
                _ => Ok(()),
            }
        },
    )
    .unwrap_err();
    assert_eq!(built.into_inner(), 6);
    assert_eq!(
        error,
        "2 of 6 raster squares failed: 1:7: datum 1; 4:7: datum 4"
    );
    assert!(build_every_square(&squares, || (), |_, _| Ok(())).is_ok());
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
    let output = dem::build(&[source], &Countries::default(), one_node()).unwrap();
    assert_eq!(u16::from_le_bytes(output.try_into().unwrap()), 3001);
}

#[test]
fn flight_height_cache_uses_the_native_dem_and_only_fills_missing_squares() {
    let root = tempfile::tempdir().unwrap();
    let source = raster_source(
        root.path(),
        "height.tif",
        Band::GroundHeight,
        vec![100.1; 4],
    );
    let square = Square { x: 256, y: 256 };
    let expected = dem::build(&[source], &Countries::default(), z9_raster_window(square)).unwrap();
    let source = raster_source(
        root.path(),
        "height.tif",
        Band::GroundHeight,
        vec![100.1; 4],
    );
    let raw = ground::GroundHeights::from_sources(
        vec![source],
        Countries::default(),
        vec![1.0; 360 * 180],
        root.path().join("cache"),
        2,
    )
    .unwrap();
    let (generated, duplicate) = rayon::join(|| raw.square(square), || raw.square(square));
    let generated = generated.unwrap();
    assert_eq!(generated, duplicate.unwrap());
    assert_eq!(std::fs::read(&generated).unwrap(), expected);
    let terrain = crate::aircraft::dem::TerrainHeights::new(crate::dev4::Dev4 {
        prepared: Default::default(),
        rasters: root.path().join("selected"),
    })
    .with_raw_heights(Some(raw));
    assert_eq!(
        terrain.height_m(-0.00001, 0.00001, &mut None).unwrap(),
        100.2
    );
    let path = root.path().join("selected/z9/256/256/dem.u16le");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, [0]).unwrap();
    let corrupt = crate::aircraft::dem::TerrainHeights::new(crate::dev4::Dev4 {
        prepared: Default::default(),
        rasters: root.path().join("selected"),
    })
    .with_raw_heights(Some(
        ground::GroundHeights::from_sources(
            vec![raster_source(
                root.path(),
                "height.tif",
                Band::GroundHeight,
                vec![100.1; 4],
            )],
            Countries::default(),
            vec![1.0; 360 * 180],
            root.path().join("cache"),
            1,
        )
        .unwrap(),
    ));
    assert!(corrupt.height_m(-0.00001, 0.00001, &mut None).is_err());
}

#[test]
fn empty_national_mosaic_does_not_change_the_base_averaging_window() {
    let root = tempfile::tempdir().unwrap();
    let base = || raster_source(root.path(), "base.tif", Band::GroundHeight, vec![100.1; 4]);
    let expected = dem::build(&[base()], &Countries::default(), one_node()).unwrap();
    let mut empty = raster_source(
        root.path(),
        "empty.tif",
        Band::GroundHeight,
        vec![f64::NAN; 4],
    );
    empty.spec.feather_nodes = 256;
    assert_eq!(
        dem::build(&[base(), empty], &Countries::default(), one_node()).unwrap(),
        expected
    );
}

#[test]
fn a_national_model_holds_only_its_own_countrys_land_and_fades_in_from_the_border() {
    // RGE ALTI's mosaic held zeros over Belgium and other heights over Switzerland and Spain, ranked
    // above their own surveys. Here country A's model covers A (nodes west of 11.5), B (11.5 to
    // 19.5) and the sea east of B, at 200 m over a world model of 100 m.
    let root = tempfile::tempdir().unwrap();
    let step = 1.0 / NODES_PER_DEGREE as f64;
    let land = |west: f64, east: f64| {
        let [west, east, south, north] = [west, east, -20.5, 20.5].map(|node| node * step);
        vec![vec![
            [west, south],
            [east, south],
            [east, north],
            [west, north],
            [west, south],
        ]]
    };
    let countries = Countries::new(std::collections::BTreeMap::from([
        ("A".to_string(), land(-20.5, 11.5)),
        ("B".to_string(), land(11.5, 19.5)),
    ]));
    let grid = ([-20.5 * step, 20.5 * step], step, 61, 41);
    let world = raster_source_on_grid(
        root.path(),
        "world.tif",
        Band::GroundHeight,
        grid,
        vec![100.0; 61 * 41],
    );
    let mut national = raster_source_on_grid(
        root.path(),
        "a.tif",
        Band::GroundHeight,
        grid,
        vec![200.0; 61 * 41],
    );
    national.spec.feather_nodes = 4;
    national.spec.priority = 1;
    national.country = countries.label("A");
    let window = Window {
        north_node: 0,
        west_node: 0,
        rows: 1,
        columns: 24,
    };
    let heights: Vec<f64> = dem::build(&[world, national], &countries, window)
        .unwrap()
        .chunks_exact(2)
        .map(|code| f64::from(u16::from_le_bytes([code[0], code[1]])) / 5.0 - 500.0)
        .collect();
    // B's land keeps the world model; A's model fades in over four nodes from the border on both
    // its sides, its own land and the sea, and holds fully beyond.
    assert!(heights[11..21].iter().all(|h| *h == 100.0), "{heights:?}");
    assert!(heights[..8].iter().all(|h| *h == 200.0), "{heights:?}");
    assert!(
        heights[7..12].windows(2).all(|pair| pair[0] > pair[1]),
        "{heights:?}"
    );
    assert!(
        heights[20..].windows(2).all(|pair| pair[0] < pair[1]),
        "{heights:?}"
    );
}

#[test]
fn a_height_no_ground_has_keeps_the_model_beneath_and_its_coverage() {
    // CA-MRDEM-30's tile seams hold rows of -578 to -912 m on Lake Nipigon; one such node failed its
    // whole square. A feathered national model at 200 m over a world model of 100 m, with one
    // impossible node inside its coverage: that node keeps the world model and opens no hole in the
    // coverage, so every other node holds the model's full weight as without it.
    let root = tempfile::tempdir().unwrap();
    let step = 1.0 / NODES_PER_DEGREE as f64;
    let grid = ([-20.5 * step, 20.5 * step], step, 61, 41);
    let window = Window {
        north_node: 0,
        west_node: 0,
        rows: 1,
        columns: 24,
    };
    let heights = |name: &str, national_heights: Vec<f64>| -> Vec<f64> {
        let world = raster_source_on_grid(
            root.path(),
            &format!("world-{name}"),
            Band::GroundHeight,
            grid,
            vec![100.0; 61 * 41],
        );
        let mut national = raster_source_on_grid(
            root.path(),
            name,
            Band::GroundHeight,
            grid,
            national_heights,
        );
        national.spec.feather_nodes = 4;
        national.spec.priority = 1;
        dem::build(&[world, national], &Countries::default(), window)
            .unwrap()
            .chunks_exact(2)
            .map(|code| f64::from(u16::from_le_bytes([code[0], code[1]])) / 5.0 - 500.0)
            .collect()
    };
    let mut seam = vec![200.0; 61 * 41];
    seam[20 * 61 + 30] = -912.0; // the window's node 10; heights ignore the band's scale tag
    let with_seam = heights("seam.tif", seam);
    let without = heights("clean.tif", vec![200.0; 61 * 41]);
    assert_eq!(with_seam[10], 100.0, "{with_seam:?}");
    assert!(
        (0..24)
            .filter(|&node| node != 10)
            .all(|node| with_seam[node] == without[node]),
        "{with_seam:?} {without:?}"
    );
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
