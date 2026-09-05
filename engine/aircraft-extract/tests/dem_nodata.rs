//! A DEM void must fail calculation publication without rejecting real sea-level terrain.

use aircraft_extract::arrow_io::{write_flights, FlightRow};
use aircraft_extract::stage_1::run_stage_1;
use aircraft_extract::trace::TracePoint;
use noise_compute::types::RasterSampler;
use raster_reader::catalog::{begin_channel, record_square};
use raster_reader::channel::Channel;
use raster_reader::{CheckedRasters, RealRasters};
use std::path::Path;

fn rasters(root: &Path) -> RealRasters {
    for channel in Channel::ALL {
        let database = begin_channel(root, channel, &"a".repeat(64)).unwrap();
        for y in 254..=256 {
            let square = grid::Square { x: 256, y };
            let window = grid::raster::RasterWindow::for_square(square);
            let mut bytes = Vec::with_capacity(channel.byte_len(window));
            for row in 0..window.rows {
                for col in 0..window.columns {
                    let lat = window.north_node - row as i32;
                    let lon = window.west_node + col as i32;
                    let value = match channel {
                        Channel::Dem if lat > 900 && lat < 2700 && lon > 900 && lon < 2700 => {
                            i16::MIN
                        }
                        Channel::Dem => (lon / 18) as i16,
                        _ => channel.ocean_value(),
                    };
                    bytes.extend_from_slice(&value.to_be_bytes()[2 - channel.bytes_per_node()..]);
                }
            }
            let path = channel.path(root, square);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, &bytes).unwrap();
            record_square(
                &database,
                channel,
                square,
                Some(raster_reader::catalog::content_digest(&bytes)),
            )
            .unwrap();
        }
    }
    RealRasters::new(root)
}

#[test]
fn nodata_cannot_publish_a_day_or_masquerade_as_sea_level() {
    let root = tempfile::tempdir().unwrap();
    let rasters = rasters(root.path());
    assert_eq!(rasters.elevation_nearest(0.5, 0.0), 0.0);
    // Exact zero interpolation weights must not consume the adjacent void.
    for (lat, lon, expected) in [
        (0.5, 0.0, Some(0.0)),
        (0.75, 0.0, Some(0.0)),
        (0.0, 0.25, Some(50.0)),
        (0.5, 0.25, Some(50.0)),
        (0.75, 0.5, Some(100.0)),
        (0.5, 0.5, None),
        (0.5, 0.375, None),
        (0.375, 0.375, None),
    ] {
        let checked = CheckedRasters::new(&rasters);
        let elevation = checked.elevation(lat, lon);
        // Same publication guard used before the popup's JSON serialization.
        let json = checked
            .ensure_valid()
            .map(|()| serde_json::to_string(&elevation).unwrap());
        if let Some(expected) = expected {
            assert_eq!(elevation, expected);
            assert!(json.is_ok());
        } else {
            assert!(elevation.is_nan());
            assert!(json.is_err());
        }
    }
    let checked = CheckedRasters::new(&rasters);
    let mut key = (i32::MIN, i32::MIN);
    let mut tile = None;
    assert_eq!(
        checked
            .elevation_nearest_cached(0.5, 0.0, &mut key, &mut tile)
            .unwrap(),
        0.0
    );
    assert!(tile.is_some());
    assert!(checked
        .elevation_nearest_cached(0.5, 0.5, &mut key, &mut tile)
        .is_err());
    assert!(checked.ensure_valid().is_err());
    // Cached profile sampling must also record interior voids, not just endpoint heights.
    for crosses_void in [false, true] {
        let checked = CheckedRasters::new(&rasters);
        let mut profile = noise_compute::propagation::PathProfile::default();
        let lon = if crosses_void { 0.5 } else { 0.0 };
        assert!(checked.elevation(0.1, lon).is_finite());
        assert!(checked.elevation(0.9, lon).is_finite());
        assert!(checked.ensure_valid().is_ok());
        let distance = 0.8 * grid::geo::M_PER_DEG_LAT;
        checked.build_path_profile(0.1, lon, 0.9, lon, distance, &mut profile);
        assert_eq!(checked.ensure_valid().is_err(), crosses_void);
        assert_eq!(
            profile.elevation_m.iter().any(|height| height.is_nan()),
            crosses_void
        );
    }
    std::thread::scope(|scope| {
        let bad = scope.spawn(|| {
            let checked = CheckedRasters::new(&rasters);
            checked.elevation(0.5, 0.5);
            assert!(checked.ensure_valid().is_err());
        });
        let good = scope.spawn(|| {
            let checked = CheckedRasters::new(&rasters);
            assert_eq!(checked.elevation(0.5, 0.0), 0.0);
            assert!(checked.ensure_valid().is_ok());
        });
        bad.join().unwrap();
        good.join().unwrap();
    });
    let input = root.path().join("flights");
    let day = "2025-01-01";
    let mut points: Vec<_> = (0..3)
        .map(|index| TracePoint {
            timestamp: 1_735_689_600.0 + f64::from(index) * 30.0,
            lat: 0.5 + index as f32 * 0.035,
            lon: 0.5,
            alt_ft: 8000.0,
            speed_kt: 250.0,
            track_deg: 0.0,
            baro_rate_fpm: 0.0,
            flags: 0,
        })
        .collect();
    let write_day = |points: &[TracePoint]| {
        write_flights(
            &input.join(format!("{day}.arrow")),
            &[FlightRow {
                flight_id: 1,
                callsign: "TEST",
                aircraft_type: b"B738",
                profile_idx: aircraft_extract::profile::profile_idx("B738"),
                source_id: 0,
                origin: 0,
                veh_kind: 0,
                gse_class: 0,
                base_timestamp: points[0].timestamp,
                points,
            }],
        )
        .unwrap()
    };
    write_day(&points);
    for existing in [false, true] {
        let output = root.path().join(format!("segments-{existing}"));
        std::fs::create_dir(&output).unwrap();
        let path = output.join(format!("{day}.arrow"));
        if existing {
            std::fs::write(&path, b"retained prior output").unwrap();
        }
        let error = run_stage_1(&input, &output, day, &rasters)
            .expect_err("a sampled DEM void must fail the whole day before any write");
        assert!(error.to_string().contains("DEM"), "{error}");
        if existing {
            assert_eq!(std::fs::read(&path).unwrap(), b"retained prior output");
        } else {
            assert!(!path.exists());
        }
    }
    assert!(rasters.elevation(0.5, 0.5).is_nan());
    for point in &mut points {
        point.lon = 0.0;
    }
    write_day(&points);
    let valid_output = root.path().join("sea-level-segments");
    assert_eq!(
        run_stage_1(&input, &valid_output, day, &rasters).unwrap(),
        2
    );
    assert!(valid_output.join(format!("{day}.arrow")).exists());
}
