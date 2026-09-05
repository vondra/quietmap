//! Actual three-channel native-byte repack and strict z9 mmap sampling, including seams and poles.

use grid::raster::{RasterWindow, SOURCE_TILE_SIDE};
use grid::{square_of, Square};
use noise_compute::types::RasterSampler;
use raster_reader::catalog::{begin_channel, read_channel};
use raster_reader::channel::Channel;
use raster_reader::repack::{NativeSources, SourceKey};
use raster_reader::{CheckedRasters, RealRasters};
use std::collections::HashSet;
use std::path::Path;

fn value(channel: Channel, latitude: i32, longitude: i32) -> i16 {
    let longitude = (longitude + 648000).rem_euclid(1296000) - 648000;
    match channel {
        Channel::Dem if latitude == -324000 => i16::MIN,
        Channel::Dem => (latitude.rem_euclid(17) * 100 + longitude.rem_euclid(19)) as i16,
        Channel::Forest => (latitude + longitude).rem_euclid(101) as i16,
        Channel::Imd => (latitude * 2 + longitude).rem_euclid(101) as i16,
    }
}

fn write_sources(root: &Path, channel: Channel, keys: &HashSet<SourceKey>) {
    std::fs::create_dir_all(root).unwrap();
    for &(lat, lon) in keys {
        let path = root.join(format!(
            "{}{:02}{}{:03}.{}",
            if lat < 0 { 'S' } else { 'N' },
            lat.unsigned_abs(),
            if lon < 0 { 'W' } else { 'E' },
            lon.unsigned_abs(),
            channel.source_extension()
        ));
        let mut data =
            Vec::with_capacity(SOURCE_TILE_SIDE * SOURCE_TILE_SIDE * channel.bytes_per_node());
        for row in 0..SOURCE_TILE_SIDE {
            for column in 0..SOURCE_TILE_SIDE {
                let bytes = value(
                    channel,
                    (lat + 1) * 3600 - row as i32,
                    lon * 3600 + column as i32,
                )
                .to_be_bytes();
                data.extend_from_slice(&bytes[2 - channel.bytes_per_node()..]);
            }
        }
        std::fs::write(path, data).unwrap();
    }
}

fn oracle(channel: Channel, lat: f64, lon: f64, nearest: bool) -> f64 {
    let lon = grid::geo::normalize_longitude(lon);
    let source_lat = (lat.floor() as i32).min(89);
    let source_lon = lon.floor() as i32;
    let row = (1.0 - (lat - f64::from(source_lat))) * 3600.0;
    let column = (lon - f64::from(source_lon)) * 3600.0;
    let pixel = |row: f64, col: f64| {
        let raw = value(
            channel,
            (source_lat + 1) * 3600 - row as i32,
            source_lon * 3600 + col as i32,
        );
        if raw == i16::MIN {
            f64::NAN
        } else {
            f64::from(raw)
        }
    };
    if nearest {
        return pixel(row.round(), column.round());
    }
    let r = row.floor();
    let c = column.floor();
    let rf = row - r;
    let cf = column - c;
    let row_value = |r| {
        let left = pixel(r, c);
        if cf == 0.0 {
            left
        } else {
            left + cf * (pixel(r, c + 1.0) - left)
        }
    };
    let top = row_value(r);
    if rf == 0.0 {
        top
    } else {
        top + rf * (row_value(r + 1.0) - top)
    }
}

#[test]
fn all_three_channels_repack_exact_nodes_and_preserve_sampling_or_report_missing_data() {
    let work = tempfile::tempdir().unwrap();
    let prepared = work.path().join("prepared");
    let keys: HashSet<_> = [(0, 0), (0, 1), (0, 179), (0, -180), (89, 0), (-90, 0)]
        .into_iter()
        .collect();
    let squares = [
        Square { x: 256, y: 255 },
        Square { x: 257, y: 255 },
        Square { x: 511, y: 255 },
        Square { x: 0, y: 255 },
        Square { x: 256, y: 0 },
        Square { x: 256, y: 511 },
    ];
    let ocean = square_of(0.5, 30.0);
    for channel in Channel::ALL {
        let source = work.path().join(channel.name());
        write_sources(&source, channel, &keys);
        let mut sources =
            NativeSources::new(&source, channel, keys.clone(), HashSet::new()).unwrap();
        let database = begin_channel(&prepared, channel, &"a".repeat(64)).unwrap();
        for square in squares {
            assert!(sources
                .publish_square(&database, &prepared, square)
                .unwrap()
                .is_some());
        }
        assert_eq!(
            sources.publish_square(&database, &prepared, ocean).unwrap(),
            None
        );
        assert_eq!(read_channel(&prepared, channel).unwrap().len(), 7);
        let mut missing = keys.clone();
        missing.insert((1, 0));
        assert!(NativeSources::new(&source, channel, missing, HashSet::new()).is_err());
        let mut unknown = NativeSources::new(
            &source,
            channel,
            keys.clone(),
            [(1, 0)].into_iter().collect(),
        )
        .unwrap();
        assert!(unknown
            .publish_square(&database, &prepared, square_of(1.25, 0.25))
            .is_err());
        assert_eq!(read_channel(&prepared, channel).unwrap().len(), 7);
    }
    let rasters = RealRasters::new(&prepared);
    let mut key = (i32::MIN, i32::MIN);
    let mut tile = None;
    for (lat, lon) in [
        (0.25, 0.703125 - 1e-10),
        (0.25, 0.703125),
        (0.25, 0.703125 + 1e-10),
        (0.25, 1.0 - 1e-10),
        (0.25, 1.0),
        (0.25, 1.0 + 1e-10),
        (0.25, 180.0 - 1e-10),
        (0.25, -180.0),
        (0.25, 180.0),
        (0.25, -180.0 + 1e-10),
        (89.5, 0.25),
        (90.0, 0.25),
        (-89.5, 0.25),
        (0.25 + 0.5 / 3600.0, 0.25 + 0.5 / 3600.0),
    ] {
        assert_eq!(
            rasters.elevation(lat, lon),
            oracle(Channel::Dem, lat, lon, false)
        );
        assert_eq!(
            rasters.elevation_nearest_cached(lat, lon, &mut key, &mut tile),
            oracle(Channel::Dem, lat, lon, true)
        );
        assert_eq!(
            rasters.forest.sample(lat, lon),
            oracle(Channel::Forest, lat, lon, true)
        );
        assert_eq!(
            rasters.imd.sample(lat, lon),
            oracle(Channel::Imd, lat, lon, false)
        );
    }
    assert!(
        rasters.elevation(-90.0, 0.25).is_nan(),
        "unsupported polar source row stays void"
    );
    assert_eq!(rasters.elevation(0.5, 30.0), 0.0);
    assert_eq!(rasters.ground_g(0.5, 30.0), 0.0);
    assert_eq!(rasters.forest.sample(0.5, 30.0), 0.0);
    assert!(
        rasters.elevation(10.0, 30.0).is_nan(),
        "unpublished square is unknown, not ocean"
    );
    drop(tile);
    drop(rasters);
    for channel in Channel::ALL {
        let square = squares[0];
        let bytes = std::fs::read(channel.path(&prepared, square)).unwrap();
        let window = RasterWindow::for_square(square);
        assert_eq!(bytes.len(), channel.byte_len(window));
        for row in 0..window.rows {
            for column in 0..window.columns {
                let expected = value(
                    channel,
                    window.north_node - row as i32,
                    window.west_node + column as i32,
                );
                assert_eq!(
                    channel.decode(
                        &bytes,
                        row as usize * window.columns as usize + column as usize
                    ),
                    f64::from(expected)
                );
            }
        }
        let mut altered = bytes.clone();
        altered[0] ^= 1;
        for corrupt in [b"broken".as_slice(), altered.as_slice()] {
            std::fs::write(channel.path(&prepared, square), corrupt).unwrap();
            let broken = RealRasters::new(&prepared);
            let checked = CheckedRasters::new(&broken);
            let mut profile = noise_compute::propagation::PathProfile::default();
            checked.build_path_profile(0.25, 0.25, 0.2501, 0.2501, 15.0, &mut profile);
            assert!(
                checked.ensure_valid().is_err(),
                "{channel:?} cannot publish a falsely empty profile"
            );
            assert!(profile.elevation_m.iter().any(|value| value.is_nan()));
            if channel == Channel::Imd {
                assert!(checked.ground_g(0.25, 0.25).is_nan());
            }
        }
        std::fs::write(channel.path(&prepared, square), &bytes).unwrap();
    }
    assert!(begin_channel(&prepared, Channel::Dem, &"b".repeat(64)).is_err());

    // The real CLI completes only its own channel; Stage1 can consume DEM
    // without pretending the unpublished forest/IMD channels are available.
    let cli_source = work.path().join("cli-source");
    std::fs::create_dir(&cli_source).unwrap();
    std::fs::copy(
        work.path().join("dem/N00E000.hgt"),
        cli_source.join("N00E000.hgt"),
    )
    .unwrap();
    let cli_output = work.path().join("cli-output");
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_raster-repack"))
        .args([
            "publish",
            cli_source.to_str().unwrap(),
            cli_output.to_str().unwrap(),
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    command
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"channel":"dem","tiles":[[0,0]],"unknown":[],"authority":"fixture"}"#)
        .unwrap();
    let result = command.wait_with_output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        read_channel(&cli_output, Channel::Dem).unwrap().len(),
        512 * 512
    );
    let dem_only = RealRasters::new(&cli_output);
    assert!(dem_only.has_data());
    let checked = CheckedRasters::new(&dem_only);
    assert_eq!(
        checked.elevation(0.25, 0.25),
        oracle(Channel::Dem, 0.25, 0.25, false)
    );
    assert!(checked.ensure_valid().is_ok());
    assert!(checked.ground_g(0.25, 0.25).is_nan());
    assert!(checked.ensure_valid().is_err());

    // A source disagreement cannot publish a partial file or a trusted SQLite row.
    use std::io::{Seek, SeekFrom, Write};
    let source = work.path().join("dem");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(source.join("N00E001.hgt"))
        .unwrap();
    file.seek(SeekFrom::Start((2700 * SOURCE_TILE_SIDE * 2) as u64))
        .unwrap();
    file.write_all(&32700_i16.to_be_bytes()).unwrap();
    drop(file);
    let failed = work.path().join("failed");
    let database = begin_channel(&failed, Channel::Dem, &"a".repeat(64)).unwrap();
    let mut sources = NativeSources::new(&source, Channel::Dem, keys, HashSet::new()).unwrap();
    assert!(sources
        .publish_square(&database, &failed, squares[1])
        .is_err());
    assert!(!Channel::Dem.path(&failed, squares[1]).exists());
    assert!(read_channel(&failed, Channel::Dem).unwrap().is_empty());
}
