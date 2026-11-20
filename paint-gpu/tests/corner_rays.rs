//! A CUDA ray must retain the CPU's screening when it touches or narrowly clips a corner.

use paint::exact::{Point, whole_energy};
use paint::square::{Files, Square};
use paint_gpu::device::{DeviceSquare, Gpu};
use physics::bands::{BANDS, PERIODS};
use physics::weather::{COLUMNS, ROWS, WeatherNode, WeatherTable, encode as encode_weather};
use popup::evaluate::Scratch;
use popup::release::Release;
use tiles::Kind;
use tiles::geo::TileId;
use tiles::obstacles::{EnvelopeClass, Outline, OutlineKind, encode as encode_obstacles};
use tiles::sources::{Attribute, Layer, Piece, encode as encode_sources};
use tiles::square::write_tiles;

#[test]
#[ignore = "requires a CUDA card and NVRTC; run explicitly in the painter release gate"]
fn tangent_and_clipped_corners_keep_cpu_roofs_and_source_containment() {
    let gpu = Gpu::new(0).unwrap();
    for (vertex_crossing, coincident_entry) in
        [(false, false), (false, true), (true, false), (true, true)]
    {
        let root = tempfile::tempdir().unwrap();
        let year = root.path().join("2026");
        std::fs::create_dir(&year).unwrap();
        let weather_bytes = encode_weather(&vec![WeatherNode::default(); ROWS * COLUMNS]);
        std::fs::write(year.join("weather"), &weather_bytes).unwrap();
        std::fs::write(year.join("complete"), b"qmsq1\n").unwrap();
        let weather = WeatherTable::parse(&weather_bytes).unwrap();
        let tile = TileId { x: 2212, y: 1387 };
        let outline = |id, height_m, mut vertices: Vec<[i16; 2]>| {
            vertices.push(vertices[0]);
            Outline {
                footprint_id: id,
                kind: OutlineKind::Exterior,
                envelope: EnvelopeClass::Residential,
                height_m,
                vertices,
            }
        };
        let rectangle = |x: i16, half: i16| {
            vec![
                [x - half, -half],
                [x + half, -half],
                [x + half, half],
                [x - half, half],
            ]
        };
        let centre = if coincident_entry { 1200 } else { 2000 };
        let diamond = if vertex_crossing {
            vec![[900, 0], [1000, 100], [1100, 0], [1000, -100]]
        } else {
            vec![[1000, 0], [1100, -100], [1000, -200], [900, -100]]
        };
        let mut outlines = vec![
            outline(1, 6.0, rectangle(0, 50)),
            outline(2, 9.0, diamond),
            outline(2, 9.0, rectangle(centre, 200)),
        ];
        if vertex_crossing {
            // The source also lies inside another footprint, exiting it exactly through a
            // vertex: its duplicate edge hits must toggle containment only once.
            outlines.push(outline(
                3,
                12.0,
                vec![[300, 0], [0, 300], [-300, 0], [0, -300]],
            ));
        }
        let obstacles = encode_obstacles(&outlines);
        write_tiles(&year, Kind::Obstacles, &[(tile, obstacles)]).unwrap();
        let source = Attribute {
            layer: Layer::Building,
            height_m: 1.5,
            ground_percent: 0,
            platform_half_width_m: 0.0,
            exclusion_radius_m: 0.0,
            footprint_id: 1,
            group_key: 1,
            emission: [[100.0; BANDS]; PERIODS],
            display: Vec::new(),
        };
        write_tiles(
            &year,
            Kind::Sources,
            &[(
                tile,
                encode_sources(
                    &[Piece {
                        ends: [[0, 0]; 2],
                        attribute: 0,
                    }],
                    &[source],
                    &[],
                ),
            )],
        )
        .unwrap();
        let release = Release::open(root.path(), "2026").unwrap();
        let files = Files::read(&release, tile).unwrap();
        let square = Square::new(&weather, tile, &files).unwrap();
        assert_eq!(square.candidates.len(), 1);
        let device_square = DeviceSquare::upload(&gpu, &square, &files, &weather).unwrap();
        // The axis ray touches or passes through vertices. Micrometre clips separate crossings
        // above the CPU's 1e-9 merge but below one float t step; wider offsets test both sides.
        let receivers: Vec<_> = [-0.002, -0.00003, 0.0, 0.000004, 0.000006, 0.00003, 0.002]
            .map(|north| [3000.0 * square.frame.east_m_per_unit / 32768.0, north])
            .into();
        let mut bands = gpu.bands(receivers.len()).unwrap();
        let point_indices: Vec<_> = (0..receivers.len() as u32).collect();
        let own = vec![0; receivers.len()];
        let candidates = vec![0; receivers.len()];
        let weights = vec![1.0; receivers.len()];
        let (energies, failed) = gpu
            .evaluate_pairs(
                &device_square,
                (&receivers, &own),
                (&candidates, &point_indices, &point_indices, &weights),
                &mut bands,
                None,
            )
            .unwrap();
        assert_eq!(
            failed,
            vec![0; receivers.len()],
            "the regression must run on the card, without CPU fallback"
        );
        for (position, actual) in receivers.into_iter().zip(energies) {
            let point = Point::with_receiver_properties(&square, position, 0, 0.0).unwrap();
            let (expected, _) = whole_energy(&square, &point, 0, &mut Scratch::default()).unwrap();
            let expected: Vec<_> = expected
                .mixed
                .into_iter()
                .chain(expected.states.into_iter().flatten())
                .collect();
            let error = actual
                .into_iter()
                .zip(expected)
                .map(|(card, cpu)| {
                    assert!(card.is_finite() && card > 0.0 && cpu.is_finite() && cpu > 0.0);
                    (10.0 * (f64::from(card) / cpu).log10()).abs()
                })
                .fold(0.0_f64, f64::max);
            eprintln!(
                "vertex={vertex_crossing} coincident={coincident_entry} north={} error={error:.6} dB",
                position[1]
            );
            assert!(
                error < 0.05,
                "corner ray differs by {error} dB at {position:?}"
            );
        }
    }
}

/// Builds the 17-crossing-in-cell-0 fixture with strictly ascending `footprint_id`s:
/// - `evict_long_exit = true`: short roof (`10`), long roof (`20`), seven strips (`30..=36`) so
///   the long exit at `x = 230` is inserted first and evicted from slot 16 by the earlier strips;
/// - `evict_long_exit = false`: seven strips (`10..=16`), short roof (`20`), long roof (`30`) so
///   the 16-slot buffer is already full when the long exit arrives and rejects it.
fn tied_roof_exits_fixture(
    evict_long_exit: bool,
) -> (tempfile::TempDir, WeatherTable, TileId, [f64; 2]) {
    let root = tempfile::tempdir().unwrap();
    let year = root.path().join("2026");
    std::fs::create_dir(&year).unwrap();
    let weather_bytes = encode_weather(&vec![WeatherNode::default(); ROWS * COLUMNS]);
    std::fs::write(year.join("weather"), &weather_bytes).unwrap();
    std::fs::write(year.join("complete"), b"qmsq1\n").unwrap();
    let weather = WeatherTable::parse(&weather_bytes).unwrap();
    let tile = TileId { x: 2212, y: 1387 };
    let rect = |id, height_m, x0: i16, x1: i16| Outline {
        footprint_id: id,
        kind: OutlineKind::Exterior,
        envelope: EnvelopeClass::Residential,
        height_m,
        vertices: vec![[x0, -100], [x1, -100], [x1, 100], [x0, 100], [x0, -100]],
    };
    let mut outlines = Vec::with_capacity(9);
    if evict_long_exit {
        outlines.push(rect(10, 0.5, 220, 230));
        outlines.push(rect(20, 5.0, -10, 230));
        for k in 0..7i16 {
            outlines.push(rect(30 + k as u64, 0.2, 10 + 10 * k, 12 + 10 * k));
        }
    } else {
        for k in 0..7i16 {
            outlines.push(rect(10 + k as u64, 0.2, 10 + 10 * k, 12 + 10 * k));
        }
        outlines.push(rect(20, 0.5, 220, 230));
        outlines.push(rect(30, 5.0, -10, 230));
    }
    write_tiles(
        &year,
        Kind::Obstacles,
        &[(tile, encode_obstacles(&outlines))],
    )
    .unwrap();
    let source = Attribute {
        layer: Layer::Building,
        height_m: 8.0,
        ground_percent: 0,
        platform_half_width_m: 0.0,
        exclusion_radius_m: 0.0,
        footprint_id: 1,
        group_key: 1,
        emission: [[100.0; BANDS]; PERIODS],
        display: Vec::new(),
    };
    write_tiles(
        &year,
        Kind::Sources,
        &[(
            tile,
            encode_sources(
                &[Piece {
                    ends: [[-50, 0]; 2],
                    attribute: 0,
                }],
                &[source],
                &[],
            ),
        )],
    )
    .unwrap();
    let release = Release::open(root.path(), "2026").unwrap();
    let files = Files::read(&release, tile).unwrap();
    let square = Square::new(&weather, tile, &files).unwrap();
    let receiver = [400.0 * square.frame.east_m_per_unit / 32768.0, 0.0];
    (root, weather, tile, receiver)
}

/// Verifies on the CPU that both P3 overflow-order fixtures encode cleanly (ascending footprint
/// IDs) and yield identical finite positive CPU energies.
#[test]
fn tied_roof_exits_fixture_encodes_for_both_overflow_orders_on_cpu() {
    let eval = |evict_long_exit| {
        let (root, weather, tile, receiver) = tied_roof_exits_fixture(evict_long_exit);
        let release = Release::open(root.path(), "2026").unwrap();
        let files = Files::read(&release, tile).unwrap();
        let square = Square::new(&weather, tile, &files).unwrap();
        let point = Point::with_receiver_properties(&square, receiver, 0, 0.0).unwrap();
        let (energy, _) = whole_energy(&square, &point, 0, &mut Scratch::default()).unwrap();
        for value in energy
            .mixed
            .into_iter()
            .chain(energy.states.into_iter().flatten())
        {
            assert!(value.is_finite() && value > 0.0, "non-positive {value}");
        }
        energy
    };
    assert_eq!(eval(true), eval(false));
}

/// Two footprints exiting at the same distance across the 16-crossing cell chunk boundary (crossings
/// 16 and 17 in one grid cell) must stay tied in both overflow orders (eviction and rejection) so
/// the longer roof is not dropped by `covered_to`.
#[test]
#[ignore = "requires a CUDA card and NVRTC; run explicitly in the painter release gate"]
fn tied_roof_exits_at_crossings_16_and_17_match_cpu() {
    let gpu = Gpu::new(0).unwrap();
    for (label, evict_long_exit) in [("eviction", true), ("rejection", false)] {
        let (root, weather, tile, receiver) = tied_roof_exits_fixture(evict_long_exit);
        let release = Release::open(root.path(), "2026").unwrap();
        let files = Files::read(&release, tile).unwrap();
        let square = Square::new(&weather, tile, &files).unwrap();
        let device_square = DeviceSquare::upload(&gpu, &square, &files, &weather).unwrap();
        let mut bands = gpu.bands(1).unwrap();
        let (energies, failed) = gpu
            .evaluate_pairs(
                &device_square,
                (&[receiver], &[0]),
                (&[0], &[0], &[0], &[1.0]),
                &mut bands,
                None,
            )
            .unwrap();
        assert_eq!(
            failed,
            vec![0],
            "{label}: must run on card without fallback"
        );
        let point = Point::with_receiver_properties(&square, receiver, 0, 0.0).unwrap();
        let (expected, _) = whole_energy(&square, &point, 0, &mut Scratch::default()).unwrap();
        let expected: Vec<_> = expected
            .mixed
            .into_iter()
            .chain(expected.states.into_iter().flatten())
            .collect();
        let error = energies[0]
            .into_iter()
            .zip(expected)
            .map(|(card, cpu)| {
                assert!(
                    card.is_finite() && card > 0.0 && cpu.is_finite() && cpu > 0.0,
                    "{label}: non-positive or non-finite energy card={card} cpu={cpu}"
                );
                (10.0 * (f64::from(card) / cpu).log10()).abs()
            })
            .fold(0.0_f64, f64::max);
        assert!(
            error < 0.05,
            "{label}: tied exit across crossings 16 and 17 differs by {error} dB"
        );
    }
}

/// Builds a synthetic tile with vertical walls along the x axis (outlines already sorted by
/// ascending `footprint_id`), returning the tempdir handle, parsed weather, files, square and
/// receiver position at `x = 522` (~100 m east of the source at `x = 0`).
fn tied_walls_fixture(walls: &[(i16, f64)]) -> (tempfile::TempDir, WeatherTable, TileId, [f64; 2]) {
    let root = tempfile::tempdir().unwrap();
    let year = root.path().join("2026");
    std::fs::create_dir(&year).unwrap();
    let weather_bytes = encode_weather(&vec![WeatherNode::default(); ROWS * COLUMNS]);
    std::fs::write(year.join("weather"), &weather_bytes).unwrap();
    std::fs::write(year.join("complete"), b"qmsq1\n").unwrap();
    let weather = WeatherTable::parse(&weather_bytes).unwrap();
    let tile = TileId { x: 2212, y: 1387 };
    let outlines: Vec<_> = walls
        .iter()
        .enumerate()
        .map(|(k, &(x, height_m))| Outline {
            footprint_id: (k + 1) as u64,
            kind: OutlineKind::Wall,
            envelope: EnvelopeClass::Residential,
            height_m,
            vertices: vec![[x, -100], [x, 100]],
        })
        .collect();
    write_tiles(
        &year,
        Kind::Obstacles,
        &[(tile, encode_obstacles(&outlines))],
    )
    .unwrap();
    let source = Attribute {
        layer: Layer::Building,
        height_m: 1.0,
        ground_percent: 0,
        platform_half_width_m: 0.0,
        exclusion_radius_m: 0.0,
        footprint_id: 0,
        group_key: 1,
        emission: [[100.0; BANDS]; PERIODS],
        display: Vec::new(),
    };
    write_tiles(
        &year,
        Kind::Sources,
        &[(
            tile,
            encode_sources(
                &[Piece {
                    ends: [[0, 0]; 2],
                    attribute: 0,
                }],
                &[source],
                &[],
            ),
        )],
    )
    .unwrap();
    let release = Release::open(root.path(), "2026").unwrap();
    let files = Files::read(&release, tile).unwrap();
    let square = Square::new(&weather, tile, &files).unwrap();
    let receiver = [522.0 * square.frame.east_m_per_unit / 32768.0, 0.0];
    (root, weather, tile, receiver)
}

/// Wall fixtures shared by the CPU and CUDA P1 regressions:
/// - `preceding_*`: `(200, 80), (203, 81)` followed by tied walls `(205, 6)` and `(205, 15)` in
///   both orders; the old in-hull replacement lets the low wall pop `(203, 81)` in the favourable
///   state, shifting favourable energy by ~0.0013 dB (well above the 1e-4 dB order tolerance).
/// - `triple_*`: three walls at the same distance in two orders.
const TIED_WALL_CASES: [(&str, &[(i16, f64)]); 4] = [
    (
        "preceding_low_first",
        &[(200, 80.0), (203, 81.0), (205, 6.0), (205, 15.0)],
    ),
    (
        "preceding_high_first",
        &[(200, 80.0), (203, 81.0), (205, 15.0), (205, 6.0)],
    ),
    ("triple_low_mid", &[(261, 20.0), (261, 15.0), (261, 25.0)]),
    (
        "triple_high_first",
        &[(261, 25.0), (261, 15.0), (261, 20.0)],
    ),
];

const TIED_WALL_ORDER_TOLERANCE_DB: f64 = 1e-4;

/// Verifies on the CPU that the shared P1 tied-wall fixtures encode cleanly, give identical
/// energies in either order, and that the popped one-wall hull (`[(200, 80)]`) differs from the
/// coalesced two-wall hull by well over `TIED_WALL_ORDER_TOLERANCE_DB`.
#[test]
fn tied_walls_fixture_encodes_and_agrees_in_either_order_on_cpu() {
    let eval = |walls: &[(i16, f64)]| {
        let (root, weather, tile, receiver) = tied_walls_fixture(walls);
        let release = Release::open(root.path(), "2026").unwrap();
        let files = Files::read(&release, tile).unwrap();
        let square = Square::new(&weather, tile, &files).unwrap();
        let point = Point::with_receiver_properties(&square, receiver, 0, 0.0).unwrap();
        let (energy, _) = whole_energy(&square, &point, 0, &mut Scratch::default()).unwrap();
        for value in energy
            .mixed
            .into_iter()
            .chain(energy.states.into_iter().flatten())
        {
            assert!(value.is_finite() && value > 0.0, "non-positive {value}");
        }
        energy
    };
    let energies: Vec<_> = TIED_WALL_CASES
        .iter()
        .map(|(_, walls)| eval(walls))
        .collect();
    assert_eq!(
        energies[0], energies[1],
        "preceding tied-wall orders differ"
    );
    assert_eq!(energies[2], energies[3], "three-way tied orders differ");

    // The old buggy favourable hull popped `(203, 81)` when `(205, 6)` arrived before `(205, 15)`,
    // collapsing the favourable hull to the single wall `(200, 80)`. Verify that hull shifts the
    // favourable-state energy observably beyond the card test's order tolerance.
    let popped = eval(&[(200, 80.0)]);
    let favourable_gap_db = (10.0 * (energies[0].states[1][0] / popped.states[1][0]).log10()).abs();
    assert!(
        favourable_gap_db > 5.0 * TIED_WALL_ORDER_TOLERANCE_DB,
        "popped one-wall hull differs by only {favourable_gap_db:.6} dB (tolerance {TIED_WALL_ORDER_TOLERANCE_DB})"
    );
}

/// Equal-distance walls in either order (preceding tall walls + tied walls, and a 3-way
/// high-low-high tie) must match the CPU within 0.05 dB on the card without fallback and agree
/// across wall orders within `TIED_WALL_ORDER_TOLERANCE_DB`.
#[test]
#[ignore = "requires a CUDA card and NVRTC; run explicitly in the painter release gate"]
fn tied_walls_in_either_order_match_cpu() {
    let gpu = Gpu::new(0).unwrap();
    let mut card_results = Vec::new();
    for (name, walls) in TIED_WALL_CASES {
        let (root, weather, tile, receiver) = tied_walls_fixture(walls);
        let release = Release::open(root.path(), "2026").unwrap();
        let files = Files::read(&release, tile).unwrap();
        let square = Square::new(&weather, tile, &files).unwrap();
        let device_square = DeviceSquare::upload(&gpu, &square, &files, &weather).unwrap();
        let mut bands = gpu.bands(1).unwrap();
        let (energies, failed) = gpu
            .evaluate_pairs(
                &device_square,
                (&[receiver], &[0]),
                (&[0], &[0], &[0], &[1.0]),
                &mut bands,
                None,
            )
            .unwrap();
        assert_eq!(failed, vec![0], "{name}: must run on card without fallback");
        let point = Point::with_receiver_properties(&square, receiver, 0, 0.0).unwrap();
        let (expected, _) = whole_energy(&square, &point, 0, &mut Scratch::default()).unwrap();
        let expected: Vec<_> = expected
            .mixed
            .into_iter()
            .chain(expected.states.into_iter().flatten())
            .collect();
        let error = energies[0]
            .into_iter()
            .zip(expected)
            .map(|(card, cpu)| {
                assert!(
                    card.is_finite() && card > 0.0 && cpu.is_finite() && cpu > 0.0,
                    "{name}: non-positive or non-finite energy card={card} cpu={cpu}"
                );
                (10.0 * (f64::from(card) / cpu).log10()).abs()
            })
            .fold(0.0_f64, f64::max);
        assert!(
            error < 0.05,
            "{name}: card differs from CPU by {error:.6} dB"
        );
        card_results.push(energies[0]);
    }
    for (pair, label) in [((0, 1), "preceding"), ((2, 3), "triple")] {
        let diff = card_results[pair.0]
            .into_iter()
            .zip(card_results[pair.1])
            .map(|(a, b)| (10.0 * (f64::from(a) / f64::from(b)).log10()).abs())
            .fold(0.0_f64, f64::max);
        assert!(
            diff < TIED_WALL_ORDER_TOLERANCE_DB,
            "{label}: card order diff {diff} dB"
        );
    }
}
