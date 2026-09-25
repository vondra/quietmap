//! Window loading, interpolation and malformed-input regressions for the climatology reader.
use super::*;
use grid::square_of;

fn field(x: usize, y: usize) -> MeteorologyNode {
    let mut node = MeteorologyNode::default();
    for k in 0..3 {
        for s in 0..SECTORS {
            node.p_percent[k][s] = ((x + y + s + k) % 101) as u8;
        }
        for b in 0..8 {
            node.alpha_mean[k][b] = (x + y + b + 1) as f32;
            node.alpha_variance[k][b] = (x + 2 * y + b) as f32;
        }
    }
    node
}

fn write_window(root: &Path, square: Square) -> Meteorology {
    let window = RasterWindow::for_square_with_density(square, ERA5_NODES_PER_DEGREE);
    let mut bytes = expected_magic().unwrap().to_vec();
    bytes.extend_from_slice(&(window.west_node as i16).to_le_bytes());
    bytes.extend_from_slice(&(window.north_node as i16).to_le_bytes());
    bytes.extend_from_slice(&(window.columns as u16).to_le_bytes());
    bytes.extend_from_slice(&(window.rows as u16).to_le_bytes());
    for row in 0..window.rows {
        for column in 0..window.columns {
            let node = field(
                (window.west_node + column as i32).rem_euclid(COLUMNS as i32) as usize,
                (90 * ERA5_NODES_PER_DEGREE - window.north_node + row as i32) as usize,
            );
            for period in 0..3 {
                bytes.extend_from_slice(&node.p_percent[period]);
            }
            for period in 0..3 {
                for band in 0..8 {
                    bytes.extend_from_slice(&node.alpha_mean[period][band].to_le_bytes());
                }
            }
            for period in 0..3 {
                for band in 0..8 {
                    bytes.extend_from_slice(&node.alpha_variance[period][band].to_le_bytes());
                }
            }
        }
    }
    let path = Meteorology::path(root, square);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    Meteorology::load(&path, square).unwrap()
}

fn assert_sample_bits(a: &MeteorologySample, b: &MeteorologySample) {
    for period in 0..3 {
        for sector in 0..SECTORS {
            assert_eq!(
                a.p[period][sector].to_bits(),
                b.p[period][sector].to_bits(),
                "period {period} sector {sector}"
            );
        }
        for band in 0..8 {
            assert_eq!(
                a.alpha_mean[period][band].to_bits(),
                b.alpha_mean[period][band].to_bits(),
                "period {period} band {band}"
            );
            assert_eq!(
                a.alpha_variance[period][band].to_bits(),
                b.alpha_variance[period][band].to_bits(),
                "period {period} band {band}"
            );
        }
    }
}

/// Deterministic LCG so the fuzz needs no `rand` dependency.
fn lcg(state: &mut u64) -> f64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    ((*state >> 33) as f64) / f64::from(1u32 << 31)
}

#[test]
fn probabilities_and_moments_are_continuous_at_grid_sector_and_square_edges() {
    let eps: f64 = 1e-8;
    // ERA5 node lines, z9 square edges, and both longitude wrapping seams.
    for (lat, lon) in [
        (50., 14.25),
        (50., 14.0625),
        (0., 0.),
        (0., 180.),
        (-90., 40.),
        (90., 40.),
    ] {
        let a = sample_at((lat - eps).clamp(-90., 90.), lon - eps, |x, y| {
            Some(field(x, y))
        })
        .unwrap();
        let b = sample_at((lat + eps).clamp(-90., 90.), lon + eps, |x, y| {
            Some(field(x, y))
        })
        .unwrap();
        for k in 0..3 {
            for s in 0..SECTORS {
                let angle = s as f64 * 22.5;
                assert!(
                    (a.probability(k, angle - eps).unwrap()
                        - b.probability(k, angle + eps).unwrap())
                    .abs()
                        < 1e-5
                );
            }
            for band in 0..8 {
                assert!((a.alpha_mean[k][band] - b.alpha_mean[k][band]).abs() < 1e-3);
                assert!((a.alpha_variance[k][band] - b.alpha_variance[k][band]).abs() < 1e-3);
            }
        }
    }
    let at = sample_at(50., 14.25, |x, y| Some(field(x, y))).unwrap();
    assert_eq!(at.alpha_mean[0][0], field(57, 160).alpha_mean[0][0]);
    assert_eq!(
        at.probability(0, 0.).unwrap(),
        at.probability(0, 360.).unwrap()
    );
    assert!((at.probability(0, 11.25).unwrap() - (at.p[0][0] + at.p[0][1]) / 2.).abs() < 1e-7);
    assert_eq!(
        at.probability(0, -1e-20).unwrap(),
        at.probability(0, 0.).unwrap()
    );
    assert!(at.probability(3, 0.).is_err());
    assert!(at.probability(0, f64::NAN).is_err());
    assert!(sample_at(91., 0., |x, y| Some(field(x, y))).is_err());
    assert!(sample_at(0., f64::INFINITY, |x, y| Some(field(x, y))).is_err());
    assert_eq!(std::mem::size_of::<MeteorologyNode>(), 240);
}

fn square_edges(square: Square) -> (f64, f64, f64, f64) {
    let west = f64::from(square.x) * 360.0 / 512.0 - 180.0;
    let edge_latitude = |y: u16| {
        (std::f64::consts::PI * (1.0 - 2.0 * f64::from(y) / 512.0))
            .sinh()
            .atan()
            .to_degrees()
    };
    let north = if square.y == 0 {
        90.0
    } else {
        edge_latitude(square.y)
    };
    let south = if square.y == 511 {
        -90.0
    } else {
        edge_latitude(square.y + 1)
    };
    (north, south, west, west + 360.0 / 512.0)
}

#[test]
fn owner_windows_serve_every_point_bit_identically_to_global_indices() {
    let squares = [
        Square { x: 276, y: 173 },
        Square { x: 256, y: 256 },
        Square { x: 0, y: 256 },
        Square { x: 511, y: 256 },
        Square { x: 0, y: 0 },
        Square { x: 256, y: 0 },
        Square { x: 511, y: 0 },
        Square { x: 0, y: 511 },
        Square { x: 256, y: 511 },
        Square { x: 511, y: 511 },
    ];
    let root = tempfile::tempdir().unwrap();
    let windows: Vec<_> = squares.iter().map(|&s| write_window(root.path(), s)).collect();
    let at_owner = |lat: f64, lon: f64| {
        let owner = square_of(lat, lon);
        let index = squares.iter().position(|&s| s == owner).unwrap();
        windows[index].at(lat, lon).unwrap()
    };
    let mut points = Vec::new();
    for (index, &square) in squares.iter().enumerate() {
        let (north, south, west, east) = square_edges(square);
        // Exact corners and edge midpoints; ownership follows square_of, edges included.
        for lat in [north, south, (north + south) / 2.0] {
            for lon in [west, east, (west + east) / 2.0] {
                points.push((lat.clamp(-90.0, 90.0), lon));
            }
        }
        let window = windows[index].window();
        let mut state = u64::from(square.x) << 32 | u64::from(square.y);
        for _ in 0..500 {
            let gx = (window.west_node.rem_euclid(COLUMNS as i32) as f64
                + lcg(&mut state) * f64::from(window.columns))
            .rem_euclid(COLUMNS as f64);
            let gy = (90 * ERA5_NODES_PER_DEGREE - window.north_node) as f64
                + lcg(&mut state) * f64::from(window.rows);
            let lon01 = gx / 4.0;
            points.push((
                (90.0 - gy / 4.0).clamp(-90.0, 90.0),
                if lon01 <= 180.0 { lon01 } else { lon01 - 360.0 },
            ));
        }
    }
    // Only owner squares serve a point; neighbours it falls into are out of scope here.
    let mut compared = 0;
    for (lat, lon) in points {
        if !squares.contains(&square_of(lat, lon)) {
            continue;
        }
        let expected = sample_at(lat, lon, |x, y| Some(field(x, y))).unwrap();
        assert_sample_bits(&at_owner(lat, lon), &expected);
        compared += 1;
    }
    assert!(compared > 1000, "compared {compared}");
    for (table, &square) in windows.iter().zip(&squares) {
        let window = table.window();
        assert_eq!(
            window,
            RasterWindow::for_square_with_density(square, ERA5_NODES_PER_DEGREE)
        );
        let mut maxima = [0u8; 3];
        for row in 0..window.rows {
            for column in 0..window.columns {
                let node = field(
                    (window.west_node + column as i32).rem_euclid(COLUMNS as i32) as usize,
                    (90 * ERA5_NODES_PER_DEGREE - window.north_node + row as i32) as usize,
                );
                for (period, maximum) in maxima.iter_mut().enumerate() {
                    *maximum = (*maximum).max(*node.p_percent[period].iter().max().unwrap());
                }
            }
        }
        assert_eq!(
            table.maximum_probability(),
            maxima.map(|p| f32::from(p) / 100.0)
        );
    }
}

#[test]
fn missing_and_malformed_windows_are_errors() {
    let root = tempfile::tempdir().unwrap();
    let square = Square { x: 276, y: 173 };
    let path = Meteorology::path(root.path(), square);
    assert!(Meteorology::load(&path, square).is_err());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let table = write_window(root.path(), square);
    let valid = std::fs::read(&path).unwrap();
    for (fault, message, mutate) in [
        ("header", "truncated", &valid[..10].to_vec()),
        ("magic", "magic", &{
            let mut bytes = valid.clone();
            bytes[0] ^= 0xff;
            bytes
        }),
        ("dims", "does not match", &{
            let mut bytes = valid.clone();
            bytes[8] = bytes[8].wrapping_add(1);
            bytes
        }),
        ("length", "byte length", &{
            let mut bytes = valid.clone();
            bytes.truncate(bytes.len() - NODE_BYTES);
            bytes
        }),
        ("p", "0..100", &{
            let mut bytes = valid.clone();
            bytes[HEADER_LEN] = 101;
            bytes
        }),
        ("mean", "nonfinite", &{
            let mut bytes = valid.clone();
            bytes[HEADER_LEN + 48..HEADER_LEN + 52].copy_from_slice(&f32::NAN.to_le_bytes());
            bytes
        }),
        ("variance", "nonfinite", &{
            let mut bytes = valid.clone();
            bytes[HEADER_LEN + 144..HEADER_LEN + 148].copy_from_slice(&(-1f32).to_le_bytes());
            bytes
        }),
    ] {
        std::fs::write(&path, mutate).unwrap();
        let error = Meteorology::load(&path, square).err().unwrap();
        assert!(error.contains(message), "{fault}: {error}");
    }
    std::fs::write(&path, &valid).unwrap();
    // A neighbour's bytes never load as this square.
    let neighbour = Square { x: 277, y: 173 };
    assert_ne!(
        table.window(),
        RasterWindow::for_square_with_density(neighbour, ERA5_NODES_PER_DEGREE)
    );
    write_window(root.path(), neighbour);
    let error = Meteorology::load(&Meteorology::path(root.path(), neighbour), square)
        .err()
        .unwrap();
    assert!(error.contains("does not match"), "{error}");
    // Receivers far outside the window are an error, not a wrap.
    assert!(table.at(0.0, 0.0).is_err());
    assert!(table.at(91.0, 0.0).is_err());
}
