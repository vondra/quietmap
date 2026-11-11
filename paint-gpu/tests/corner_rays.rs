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
        tiles::mark_year_complete(&year, br#"{"year":"2026","as_of":"2027-01-01"}"#, "test")
            .unwrap();
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
