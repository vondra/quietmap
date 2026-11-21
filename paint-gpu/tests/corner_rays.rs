//! A CUDA ray must retain the CPU's screening when it touches or narrowly clips a corner.

use paint::exact::{Point, whole_energy};
use paint::square::{Files, Square};
use paint_gpu::device::{DeviceSquare, Gpu};
use physics::bands::{BANDS, PERIODS};
use physics::weather::{
    COLUMNS, ROWS, SECTORS, WeatherNode, WeatherTable, encode as encode_weather,
};
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

/// Builds the 17-crossing-in-cell-0 fixture with strictly ascending `footprint_id`s, distinct
/// source emissions in every period and octave, and per-period favourable propagation shares:
/// - `evict_long_exit = true`: short roof (`10`), long roof (`20`), seven strips (`30..=36`) so
///   the long exit at `x = 230` is inserted first and evicted from slot 16 by the earlier strips;
/// - `evict_long_exit = false`: seven strips (`10..=16`), short roof (`20`), long roof (`30`) so
///   the 16-slot buffer is already full when the long exit arrives and rejects it.
fn tied_roof_exits_fixture(
    evict_long_exit: bool,
    favourable_percent: [u8; PERIODS],
) -> (tempfile::TempDir, WeatherTable, TileId, [f64; 2]) {
    let root = tempfile::tempdir().unwrap();
    let year = root.path().join("2026");
    std::fs::create_dir(&year).unwrap();
    let weather_node = WeatherNode {
        percent: favourable_percent.map(|p| [p; SECTORS]),
        ..WeatherNode::default()
    };
    let weather_bytes = encode_weather(&vec![weather_node; ROWS * COLUMNS]);
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
        emission: [
            [92.0, 94.0, 96.0, 98.0, 100.0, 97.0, 94.0, 90.0],
            [87.0, 90.0, 92.0, 95.0, 96.0, 93.0, 89.0, 84.0],
            [82.0, 85.0, 88.0, 90.0, 91.0, 87.0, 83.0, 78.0],
        ],
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

/// Evaluates the single pair `(candidate 0, receiver)` on the card (requesting mixed pair spectra)
/// and compares both the 9 state sums and all `PERIODS * BANDS` mixed octave energies against
/// `whole_energy` on the CPU, requiring zero fallback and finite positive values before folding.
fn evaluate_and_compare_single_pair(
    gpu: &Gpu,
    square: &Square,
    files: &Files,
    weather: &WeatherTable,
    receiver: [f64; 2],
    label: &str,
) -> ([f32; 9], Vec<f32>) {
    let device_square = DeviceSquare::upload(gpu, square, files, weather).unwrap();
    let mut bands = gpu.bands(1).unwrap();
    let mut pair_spectra = Vec::new();
    let (energies, failed) = gpu
        .evaluate_pairs(
            &device_square,
            (&[receiver], &[0]),
            (&[0], &[0], &[0], &[1.0]),
            &mut bands,
            Some(&mut pair_spectra),
        )
        .unwrap();
    assert_eq!(
        failed,
        vec![0],
        "{label}: must run on card without fallback"
    );
    assert_eq!(
        pair_spectra.len(),
        PERIODS * BANDS,
        "{label}: expected {PERIODS}x{BANDS} mixed octave energies"
    );
    let point = Point::with_receiver_properties(square, receiver, 0, 0.0).unwrap();
    let (expected, expected_bands) =
        whole_energy(square, &point, 0, &mut Scratch::default()).unwrap();
    let expected_sums: Vec<_> = expected
        .mixed
        .into_iter()
        .chain(expected.states.into_iter().flatten())
        .collect();
    let sum_error = energies[0]
        .into_iter()
        .zip(expected_sums)
        .map(|(card, cpu)| {
            assert!(
                card.is_finite() && card > 0.0 && cpu.is_finite() && cpu > 0.0,
                "{label}: non-positive or non-finite state sum card={card} cpu={cpu}"
            );
            (10.0 * (f64::from(card) / cpu).log10()).abs()
        })
        .fold(0.0_f64, f64::max);
    assert!(
        sum_error < 0.05,
        "{label}: state sums differ from CPU by {sum_error:.6} dB"
    );
    let mut octave_error = 0.0_f64;
    for (period, cpu_period) in expected_bands.iter().enumerate() {
        for (band, &cpu) in cpu_period.iter().enumerate() {
            let card = pair_spectra[period * BANDS + band];
            assert!(
                card.is_finite() && card > 0.0 && cpu.is_finite() && cpu > 0.0,
                "{label}: period {period} band {band} non-positive or non-finite card={card} cpu={cpu}"
            );
            octave_error = octave_error.max((10.0 * (f64::from(card) / cpu).log10()).abs());
        }
    }
    assert!(
        octave_error < 0.05,
        "{label}: mixed octave spectra differ from CPU by {octave_error:.6} dB"
    );
    (energies[0], pair_spectra)
}

/// P3 overflow-order and weather cases shared by the CPU and CUDA tests: both overflow orders
/// (eviction and rejection) under calm weather (`[0, 0, 0]`) and mixed weather (`[25, 50, 75]`,
/// non-zero favourable propagation share in every period).
const TIED_ROOF_CASES: [(&str, bool, [u8; PERIODS]); 4] = [
    ("eviction_calm", true, [0, 0, 0]),
    ("rejection_calm", false, [0, 0, 0]),
    ("eviction_mixed", true, [25, 50, 75]),
    ("rejection_mixed", false, [25, 50, 75]),
];

/// Verifies on the CPU that all P3 overflow-order and mixed-weather fixtures encode cleanly
/// (ascending footprint IDs), yield identical finite positive CPU state sums and mixed octave
/// energies across overflow orders, distinguish every period, and mix favourable propagation when
/// weather shares are non-zero.
#[test]
fn tied_roof_exits_fixture_encodes_for_both_overflow_orders_on_cpu() {
    let eval = |evict_long_exit, favourable_percent| {
        let (root, weather, tile, receiver) =
            tied_roof_exits_fixture(evict_long_exit, favourable_percent);
        let release = Release::open(root.path(), "2026").unwrap();
        let files = Files::read(&release, tile).unwrap();
        let square = Square::new(&weather, tile, &files).unwrap();
        let point = Point::with_receiver_properties(&square, receiver, 0, 0.0).unwrap();
        let (energy, bands) = whole_energy(&square, &point, 0, &mut Scratch::default()).unwrap();
        for value in energy
            .mixed
            .into_iter()
            .chain(energy.states.into_iter().flatten())
            .chain(bands.into_iter().flatten())
        {
            assert!(value.is_finite() && value > 0.0, "non-positive {value}");
        }
        (energy, bands)
    };
    let results: Vec<_> = TIED_ROOF_CASES
        .iter()
        .map(|&(_, evict, weather)| eval(evict, weather))
        .collect();
    assert_eq!(results[0], results[1], "calm overflow orders differ");
    assert_eq!(results[2], results[3], "mixed overflow orders differ");

    let (calm_energy, calm_bands) = &results[0];
    let (mixed_energy, mixed_bands) = &results[2];
    for &(a, b) in &[(0, 1), (1, 2), (0, 2)] {
        assert_ne!(
            mixed_bands[a], mixed_bands[b],
            "periods {a} and {b} must have distinct mixed octave spectra"
        );
    }
    for period in 0..PERIODS {
        assert_ne!(
            mixed_energy.mixed[period], mixed_energy.states[0][period],
            "period {period}: mixed weather must differ from calm propagation"
        );
        assert_ne!(
            mixed_bands[period], calm_bands[period],
            "period {period}: mixed octave spectrum must differ from calm-only weather"
        );
    }
    assert_eq!(
        calm_energy.states, mixed_energy.states,
        "weather shares change only the mixture, not the calm/favourable states"
    );
}

/// Two footprints exiting at the same distance across the 16-crossing cell chunk boundary (crossings
/// 16 and 17 in one grid cell) must stay tied in both overflow orders (eviction and rejection) and
/// under both calm and mixed weather, matching CPU state sums and every mixed octave.
#[test]
#[ignore = "requires a CUDA card and NVRTC; run explicitly in the painter release gate"]
fn tied_roof_exits_at_crossings_16_and_17_match_cpu() {
    let gpu = Gpu::new(0).unwrap();
    for (label, evict_long_exit, favourable_percent) in TIED_ROOF_CASES {
        let (root, weather, tile, receiver) =
            tied_roof_exits_fixture(evict_long_exit, favourable_percent);
        let release = Release::open(root.path(), "2026").unwrap();
        let files = Files::read(&release, tile).unwrap();
        let square = Square::new(&weather, tile, &files).unwrap();
        evaluate_and_compare_single_pair(&gpu, &square, &files, &weather, receiver, label);
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

    // The old buggy favourable hull popped the second wall when the low tied wall arrived first,
    // collapsing the favourable hull to the scene's first wall alone. Derive the one-wall control
    // from `TIED_WALL_CASES[0]` itself so reverting the scene cannot mask a tiny hull difference.
    assert_eq!(
        TIED_WALL_CASES[0].1[..2],
        TIED_WALL_CASES[1].1[..2],
        "preceding wall pairs must share their first two walls"
    );
    let popped = eval(&TIED_WALL_CASES[0].1[..1]);
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
        card_results.push(evaluate_and_compare_single_pair(
            &gpu, &square, &files, &weather, receiver, name,
        ));
    }
    for (pair, label) in [((0, 1), "preceding"), ((2, 3), "triple")] {
        let sum_diff = card_results[pair.0]
            .0
            .into_iter()
            .zip(card_results[pair.1].0)
            .map(|(a, b)| (10.0 * (f64::from(a) / f64::from(b)).log10()).abs())
            .fold(0.0_f64, f64::max);
        assert!(
            sum_diff < TIED_WALL_ORDER_TOLERANCE_DB,
            "{label}: card state-sum order diff {sum_diff} dB"
        );
        let octave_diff = card_results[pair.0]
            .1
            .iter()
            .zip(&card_results[pair.1].1)
            .map(|(&a, &b)| (10.0 * (f64::from(a) / f64::from(b)).log10()).abs())
            .fold(0.0_f64, f64::max);
        assert!(
            octave_diff < TIED_WALL_ORDER_TOLERANCE_DB,
            "{label}: card octave order diff {octave_diff} dB"
        );
    }
}

/// Builds a synthetic tile with a degenerate (<1 mm) line, a point source, and a wide road behind
/// alternating walls whose skyline splits its quadrature past `PAIR_RAY_SLOTS` (6 rays).
fn mixed_ray_counts_fixture() -> (tempfile::TempDir, WeatherTable, TileId, [f64; 2]) {
    let root = tempfile::tempdir().unwrap();
    let year = root.path().join("2026");
    std::fs::create_dir(&year).unwrap();
    let weather_bytes = encode_weather(&vec![WeatherNode::default(); ROWS * COLUMNS]);
    std::fs::write(year.join("weather"), &weather_bytes).unwrap();
    std::fs::write(year.join("complete"), b"qmsq1\n").unwrap();
    let weather = WeatherTable::parse(&weather_bytes).unwrap();
    let tile = TileId { x: 2212, y: 1387 };
    // Four short walls across the sight line to the wide road: creates > 6 alternating runs.
    let outlines: Vec<_> = [-420i16, -180, 60, 300]
        .into_iter()
        .enumerate()
        .map(|(k, x)| Outline {
            footprint_id: (k + 1) as u64,
            kind: OutlineKind::Wall,
            envelope: EnvelopeClass::Residential,
            height_m: 12.0,
            vertices: vec![[x, 120], [x + 90, 120]],
        })
        .collect();
    write_tiles(
        &year,
        Kind::Obstacles,
        &[(tile, encode_obstacles(&outlines))],
    )
    .unwrap();
    let road = Attribute {
        layer: Layer::Road,
        height_m: 0.05,
        ground_percent: 0,
        platform_half_width_m: 0.0,
        exclusion_radius_m: 0.0,
        footprint_id: 0,
        group_key: 1,
        emission: [[90.0; BANDS]; PERIODS],
        display: Vec::new(),
    };
    let point_source = Attribute {
        layer: Layer::Industry,
        height_m: 2.0,
        ground_percent: 0,
        platform_half_width_m: 0.0,
        exclusion_radius_m: 0.0,
        footprint_id: 0,
        group_key: 2,
        emission: [[95.0; BANDS]; PERIODS],
        display: Vec::new(),
    };
    write_tiles(
        &year,
        Kind::Sources,
        &[(
            tile,
            encode_sources(
                &[
                    Piece {
                        ends: [[0, 200], [1, 200]],
                        attribute: 0,
                    },
                    Piece {
                        ends: [[0, 200], [0, 200]],
                        attribute: 1,
                    },
                    Piece {
                        ends: [[-1000, 300], [1000, 300]],
                        attribute: 0,
                    },
                ],
                &[road, point_source],
                &[],
            ),
        )],
    )
    .unwrap();
    (root, weather, tile, [0.0, 0.0])
}

/// Finds `(short_line, point_source, wide_road)` candidate indices in `mixed_ray_counts_fixture`
/// regardless of source-encoder ordering.
fn mixed_ray_counts_indices(square: &Square) -> (u32, u32, u32) {
    let point = square
        .candidates
        .iter()
        .position(|c| !c.line)
        .expect("point candidate") as u32;
    let wide = square
        .candidates
        .iter()
        .position(|c| {
            c.line
                && (c.ends_m[1][0] - c.ends_m[0][0]).hypot(c.ends_m[1][1] - c.ends_m[0][1]) > 50.0
        })
        .expect("wide road candidate") as u32;
    let short = square
        .candidates
        .iter()
        .enumerate()
        .position(|(i, c)| c.line && i as u32 != wide)
        .expect("short line candidate") as u32;
    (short, point, wide)
}

/// Verifies on the CPU that the wide road behind four walls emits strictly more than 6 rays
/// (forcing pass-2 `pair_rays` rewalk past `PAIR_RAY_SLOTS` on the card).
#[test]
fn wide_road_behind_walls_exceeds_cached_pair_ray_slots_on_cpu() {
    let (root, weather, tile, receiver) = mixed_ray_counts_fixture();
    let release = Release::open(root.path(), "2026").unwrap();
    let files = Files::read(&release, tile).unwrap();
    let square = Square::new(&weather, tile, &files).unwrap();
    let (_, _, wide) = mixed_ray_counts_indices(&square);
    let point = Point::with_receiver_properties(&square, receiver, 0, 0.0).unwrap();
    let mut cand = square.candidates[wide as usize].clone();
    cand.distance_m = cand.distance_from(point.position);
    let mut rays = 0usize;
    popup::evaluate::source_rays(
        &point.receiver(&square),
        &cand,
        &square.attributes[cand.attribute],
        &mut Scratch::default(),
        &mut |_| rays += 1,
    )
    .unwrap();
    assert!(rays > 6, "expected > 6 quadrature rays, got {rays}");
}

/// Verifies that removing dead device memsets keeps unwritten outputs clean: an all-zero-ray
/// launch returns zero sums and spectra, and a mixed launch with a 0-ray degenerate line, a
/// failed receiver, a 1-ray point, and a >6-ray line matches CPU sums and mixed octave spectra.
#[test]
#[ignore = "requires a CUDA card and NVRTC; run explicitly in the painter release gate"]
fn zero_ray_failed_and_rewalked_pairs_stay_clean_without_dead_memsets() {
    let gpu = Gpu::new(0).unwrap();
    let (root, weather, tile, receiver) = mixed_ray_counts_fixture();
    let release = Release::open(root.path(), "2026").unwrap();
    let files = Files::read(&release, tile).unwrap();
    let mut square = Square::new(&weather, tile, &files).unwrap();
    let (short_cand, point_cand, wide_cand) = mixed_ray_counts_indices(&square);
    // Collapse the short line to < 1 mm so `line_piece_geometry` emits 0 rays with status 0.
    square.candidates[short_cand as usize].ends_m = [[0.0, 20.0], [1e-4, 20.0]];
    assert!(square.candidates[short_cand as usize].line);
    // Add a point source outside the loaded tiles: pass-1 lists 1 ray with PAIR_RAY_SINGLE and
    // `ray_transfers` fails on the unread source terrain, exercising the 1-ray direct failure store.
    let mut outside_point = square.candidates[point_cand as usize].clone();
    outside_point.ends_m = [[1.0e7, 0.0], [1.0e7, 0.0]];
    let outside_point_cand = square.candidates.len() as u32;
    square.candidates.push(outside_point);
    let device_square = DeviceSquare::upload(&gpu, &square, &files, &weather).unwrap();
    let outside = [1.0e7, 0.0];

    // 1. All-zero-ray launch (`total == 0`) with `pair_spectra` requested.
    let mut bands = gpu.bands(2).unwrap();
    let mut spectra = vec![123.0; 4];
    let (sums, failed) = gpu
        .evaluate_pairs(
            &device_square,
            (&[receiver, outside], &[0, 0]),
            (&[short_cand, point_cand], &[0, 1], &[0, 1], &[1.0, 1.0]),
            &mut bands,
            Some(&mut spectra),
        )
        .unwrap();
    assert_eq!(failed, vec![0, paint_gpu::device::FAILED_NOT_READ]);
    assert_eq!(sums, vec![[0.0; 9]; 2]);
    assert_eq!(spectra, vec![0.0; 2 * PERIODS * BANDS]);

    // 2. Mixed launch (`total > 0`): 0-ray line, failed receiver, 1-ray point, >6-ray rewalk line,
    // and a listed 1-ray point that fails during `ray_transfers`.
    let mut bands = gpu.bands(2).unwrap();
    let mut spectra = Vec::new();
    let (sums, failed) = gpu
        .evaluate_pairs(
            &device_square,
            (&[receiver, outside], &[0, 0]),
            (
                &[
                    short_cand,
                    point_cand,
                    point_cand,
                    wide_cand,
                    outside_point_cand,
                ],
                &[0, 1, 0, 0, 0],
                &[0, 1, 0, 0, 0],
                &[1.0; 5],
            ),
            &mut bands,
            Some(&mut spectra),
        )
        .unwrap();
    assert_eq!(
        failed,
        vec![
            0,
            paint_gpu::device::FAILED_NOT_READ,
            0,
            0,
            paint_gpu::device::FAILED_NOT_READ,
        ]
    );
    assert_eq!(sums[0], [0.0; 9]);
    assert_eq!(&spectra[0..PERIODS * BANDS], &[0.0; PERIODS * BANDS]);
    let point = Point::with_receiver_properties(&square, receiver, 0, 0.0).unwrap();
    for (pair_idx, cand_idx) in [(2usize, point_cand), (3, wide_cand)] {
        let (expected, expected_bands) =
            whole_energy(&square, &point, cand_idx, &mut Scratch::default()).unwrap();
        let expected_sums: Vec<_> = expected
            .mixed
            .into_iter()
            .chain(expected.states.into_iter().flatten())
            .collect();
        for (card, cpu) in sums[pair_idx].into_iter().zip(expected_sums) {
            let err = (10.0 * (f64::from(card) / cpu).log10()).abs();
            assert!(err < 0.05, "pair {pair_idx} sum diff {err} dB");
        }
        for (period, cpu_period) in expected_bands.iter().enumerate() {
            for (band, &cpu) in cpu_period.iter().enumerate() {
                let card = spectra[(pair_idx * PERIODS + period) * BANDS + band];
                let err = (10.0 * (f64::from(card) / cpu).log10()).abs();
                assert!(
                    err < 0.05,
                    "pair {pair_idx} p{period} b{band} diff {err} dB"
                );
            }
        }
    }
}

/// Builds a forested tile (80 % cover, 15 m canopy) with mixed weather, a continuous wall
/// saturating a wide road's skyline mask, and a 1-ray point source behind a building so hoisted
/// `add_foliage`, mask-saturation early exit, 16-byte `RayCrossing`, and 1-ray direct stores all
/// run on the same scene.
fn foliage_and_saturated_skyline_fixture() -> (tempfile::TempDir, WeatherTable, TileId, [f64; 2]) {
    use tiles::terrain::{Node, Window, code_of_height_m, encode as encode_terrain};

    let root = tempfile::tempdir().unwrap();
    let year = root.path().join("2026");
    std::fs::create_dir(&year).unwrap();
    let weather_node = WeatherNode {
        percent: [25u8, 50, 75].map(|p| [p; SECTORS]),
        ..WeatherNode::default()
    };
    let weather_bytes = encode_weather(&vec![weather_node; ROWS * COLUMNS]);
    std::fs::write(year.join("weather"), &weather_bytes).unwrap();
    std::fs::write(year.join("complete"), b"qmsq1\n").unwrap();
    let weather = WeatherTable::parse(&weather_bytes).unwrap();
    let tile = TileId { x: 2212, y: 1387 };
    let window = Window::of_tile(tile);
    let nodes = vec![
        Node {
            height_code: code_of_height_m(100.0),
            impervious_percent: 20,
            forest_percent: 80,
            canopy_m: 15,
        };
        window.node_count()
    ];
    write_tiles(
        &year,
        Kind::Terrain,
        &[(tile, encode_terrain(window, &nodes))],
    )
    .unwrap();
    let outlines = vec![
        // Continuous tall wall covering the entire azimuth of the wide road, saturating every mask.
        Outline {
            footprint_id: 1,
            kind: OutlineKind::Wall,
            envelope: EnvelopeClass::Residential,
            height_m: 12.0,
            vertices: vec![[-1500, 100], [1500, 100]],
        },
        // Second wall behind the first (skipped once masks saturate) and a building screening the
        // 1-ray point source to the east.
        Outline {
            footprint_id: 2,
            kind: OutlineKind::Wall,
            envelope: EnvelopeClass::Residential,
            height_m: 10.0,
            vertices: vec![[-1500, 150], [1500, 150]],
        },
        Outline {
            footprint_id: 3,
            kind: OutlineKind::Exterior,
            envelope: EnvelopeClass::Residential,
            height_m: 8.5,
            vertices: vec![
                [300, -100],
                [420, -100],
                [420, 100],
                [300, 100],
                [300, -100],
            ],
        },
    ];
    write_tiles(
        &year,
        Kind::Obstacles,
        &[(tile, encode_obstacles(&outlines))],
    )
    .unwrap();
    let road = Attribute {
        layer: Layer::Road,
        height_m: 0.5,
        ground_percent: 0,
        platform_half_width_m: 0.0,
        exclusion_radius_m: 0.0,
        footprint_id: 0,
        group_key: 1,
        emission: [[95.0; BANDS]; PERIODS],
        display: Vec::new(),
    };
    let point_source = Attribute {
        layer: Layer::Industry,
        height_m: 2.0,
        ground_percent: 0,
        platform_half_width_m: 0.0,
        exclusion_radius_m: 0.0,
        footprint_id: 0,
        group_key: 2,
        emission: [[98.0; BANDS]; PERIODS],
        display: Vec::new(),
    };
    write_tiles(
        &year,
        Kind::Sources,
        &[(
            tile,
            encode_sources(
                &[
                    Piece {
                        ends: [[-800, 300], [800, 300]],
                        attribute: 0,
                    },
                    Piece {
                        ends: [[1000, 0], [1000, 0]],
                        attribute: 1,
                    },
                ],
                &[road, point_source],
                &[],
            ),
        )],
    )
    .unwrap();
    (root, weather, tile, [0.0, 0.0])
}

/// Verifies on the CPU that the forested fixture has non-zero foliage attenuation and distinct
/// calm/favourable propagation for both its wide road and its screened 1-ray point source.
#[test]
fn foliage_and_saturated_skyline_fixture_exercises_foliage_on_cpu() {
    let (root, weather, tile, receiver) = foliage_and_saturated_skyline_fixture();
    let release = Release::open(root.path(), "2026").unwrap();
    let files = Files::read(&release, tile).unwrap();
    let square = Square::new(&weather, tile, &files).unwrap();
    assert_eq!(square.candidates.len(), 2);
    let point = Point::with_receiver_properties(&square, receiver, 0, 0.0).unwrap();
    for index in 0..2u32 {
        let (energy, _) = whole_energy(&square, &point, index, &mut Scratch::default()).unwrap();
        assert!(energy.mixed[0] > 0.0 && energy.states[0][0] != energy.states[1][0]);
    }
}

/// Verifies GPU/CPU parity on forested rays (`add_foliage`), saturated skyline masks
/// (`mask_wide_buckets`), packed 16-byte `RayCrossing`s, and 1-ray direct stores (`PAIR_RAY_SINGLE`).
#[test]
#[ignore = "requires a CUDA card and NVRTC; run explicitly in the painter release gate"]
fn foliage_saturated_skyline_and_single_ray_stores_match_cpu() {
    let gpu = Gpu::new(0).unwrap();
    let (root, weather, tile, receiver) = foliage_and_saturated_skyline_fixture();
    let release = Release::open(root.path(), "2026").unwrap();
    let files = Files::read(&release, tile).unwrap();
    let square = Square::new(&weather, tile, &files).unwrap();
    let device_square = DeviceSquare::upload(&gpu, &square, &files, &weather).unwrap();
    let mut bands = gpu.bands(1).unwrap();
    let mut spectra = Vec::new();
    let (sums, failed) = gpu
        .evaluate_pairs(
            &device_square,
            (&[receiver], &[0]),
            (&[0, 1], &[0, 0], &[0, 0], &[1.0, 1.0]),
            &mut bands,
            Some(&mut spectra),
        )
        .unwrap();
    assert_eq!(failed, vec![0, 0]);
    let point = Point::with_receiver_properties(&square, receiver, 0, 0.0).unwrap();
    for (pair_idx, cand_idx) in [(0usize, 0u32), (1, 1)] {
        let (expected, expected_bands) =
            whole_energy(&square, &point, cand_idx, &mut Scratch::default()).unwrap();
        let expected_sums: Vec<_> = expected
            .mixed
            .into_iter()
            .chain(expected.states.into_iter().flatten())
            .collect();
        for (card, cpu) in sums[pair_idx].into_iter().zip(expected_sums) {
            let err = (10.0 * (f64::from(card) / cpu).log10()).abs();
            assert!(err < 0.05, "pair {pair_idx} sum diff {err} dB");
        }
        for (period, cpu_period) in expected_bands.iter().enumerate() {
            for (band, &cpu) in cpu_period.iter().enumerate() {
                let card = spectra[(pair_idx * PERIODS + period) * BANDS + band];
                let err = (10.0 * (f64::from(card) / cpu).log10()).abs();
                assert!(
                    err < 0.05,
                    "pair {pair_idx} p{period} b{band} diff {err} dB"
                );
            }
        }
    }
}
