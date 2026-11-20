//! Regression checks for a block blended from its corners or its coarse lattice where its quadrants' probes agree.

use super::*;
use crate::lattice_blend::{ENERGY_FLOOR, SONE_FLOOR};

/// The exact point of a field's `value` without own sources: its loudness, and a thousand times
/// it in the first layer's energy, over the blend's floors (a field linear in the logarithm
/// blends exactly).
fn heard(value: f64) -> Option<Anchor> {
    let mut energies = [0.0; LAYERS];
    energies[0] = 1000.0 * value - ENERGY_FLOOR;
    Some(Anchor {
        paint: (energies, [value - SONE_FLOOR; LAYERS + 1]),
        own: [0.0; LAYERS],
        free: [0.0; LAYERS],
    })
}

fn no_own(_: usize, _: usize) -> Option<[f64; LAYERS]> {
    Some([0.0; LAYERS])
}

/// A field the coarse lattice blends exactly passes the quadrants' probes, at blocks of 8 and 16,
/// and the block then blends every pixel from the coarse lattice, a probe within the tolerance
/// taking the blend; a probe over half a phon or half a decibel off, or a coarse sample unknown,
/// sends the block to the fine lattice.
#[test]
fn a_coarse_lattice_blends_a_block_only_where_its_quadrant_probes_agree() {
    for b in [8, 16] {
        let (axis, positions) = sample_positions(b);
        let lines = [axis[0], axis[2], axis[4]].map(|at| at as f64);
        let field = |x: usize, y: usize| (1.0 + 0.05 * x as f64 + 0.02 * y as f64).exp();
        let mut samples: Vec<Option<Anchor>> = vec![None; positions.len()];
        for &k in &COARSE_FIRST {
            let (x, y) = positions[k];
            samples[k] = heard(field(x, y));
        }
        let (px, py) = positions[18];
        samples[18] = heard(field(px, py) * 1.02);
        let coarse_of = |samples: &[Option<Anchor>]| -> Vec<Option<Anchor>> {
            COARSE_FIRST[..9].iter().map(|&k| samples[k]).collect()
        };
        let coarse = coarse_of(&samples);
        let lattice = Lattice::new(&lines, &coarse);
        assert!(probes_agree(&lattice, &samples, &positions));
        let block = QuietSamples::blended_whole(b, &lattice, &no_own);
        assert!(block.complete());
        for y in 0..b {
            for x in 0..b {
                let known = block.known_at(x, y).unwrap();
                assert!(
                    (known.1[LAYERS] + SONE_FLOOR - field(x, y)).abs() < 1e-9,
                    "block {b} pixel {x},{y}"
                );
            }
        }
        let mut loud = samples.clone();
        loud[18] = heard(field(px, py) * 1.05);
        assert!(!probes_agree(&lattice, &loud, &positions));
        let mut energetic = samples.clone();
        let mut probe = heard(field(px, py)).unwrap();
        probe.paint.0[0] *= 1.2;
        energetic[18] = Some(probe);
        assert!(!probes_agree(&lattice, &energetic, &positions));
        let mut unknown = samples;
        unknown[24] = None;
        let coarse = coarse_of(&unknown);
        assert!(!probes_agree(
            &Lattice::new(&lines, &coarse),
            &unknown,
            &positions
        ));
    }
}

/// A field the corners blend exactly passes the quadrants' probes, and every pixel of the block
/// then takes the corners' blend at its centre, the corners standing at the block's edges. A probe
/// over half a phon off, one in a building, or a corner in one, refuses the corners' blend.
#[test]
fn the_corners_blend_a_block_only_where_its_quadrant_probes_agree() {
    let b = 16;
    let (_, positions) = sample_positions(b);
    let lines = [-0.5, b as f64 - 0.5];
    let field = |x: f64, y: f64| (1.0 + 0.05 * x + 0.02 * y).exp();
    let side = b as f64;
    let corners =
        [(0.0, 0.0), (side, 0.0), (0.0, side), (side, side)].map(|(x, y)| heard(field(x, y)));
    let at_centre = |k: usize| field(positions[k].0 as f64 + 0.5, positions[k].1 as f64 + 0.5);
    let mut samples: Vec<Option<Anchor>> = vec![None; positions.len()];
    for &k in &QUADRANT_PROBES {
        samples[k] = heard(at_centre(k));
    }
    let lattice = Lattice::new(&lines, &corners);
    assert!(probes_agree(&lattice, &samples, &positions));
    let block = QuietSamples::blended_whole(b, &lattice, &no_own);
    for y in 0..b {
        for x in 0..b {
            let known = block.known_at(x, y).unwrap();
            let expected = field(x as f64 + 0.5, y as f64 + 0.5);
            assert!(
                (known.1[LAYERS] + SONE_FLOOR - expected).abs() < 1e-9,
                "pixel {x},{y}"
            );
            assert!((known.0[0] + ENERGY_FLOOR - 1000.0 * expected).abs() < 1e-6);
        }
    }
    let mut loud = samples.clone();
    loud[18] = heard(at_centre(18) * 1.05);
    assert!(!probes_agree(&lattice, &loud, &positions));
    let mut indoors = samples.clone();
    indoors[6] = None;
    assert!(!probes_agree(&lattice, &indoors, &positions));
    let mut built = corners;
    built[3] = None;
    assert!(!probes_agree(
        &Lattice::new(&lines, &built),
        &samples,
        &positions
    ));
}

/// A quiet probe a decibel and a half off the corners' blend passes where the corners are as quiet, and
/// refuses the blend where one corner shows: the blend would paint the block's visible pixels too.
#[test]
fn a_quiet_probe_agrees_only_as_far_as_its_blocks_loudest_corner_shows() {
    let b = 16;
    let (_, positions) = sample_positions(b);
    let lines = [-0.5, b as f64 - 0.5];
    let point = |db: f64, sone: f64| {
        let mut energies = [0.0; LAYERS];
        energies[0] = 10.0f64.powf(db / 10.0);
        Some(Anchor {
            paint: (energies, [sone; LAYERS + 1]),
            own: [0.0; LAYERS],
            free: [0.0; LAYERS],
        })
    };
    let mut samples: Vec<Option<Anchor>> = vec![None; positions.len()];
    for &k in &QUADRANT_PROBES {
        samples[k] = point(26.5, 0.3);
    }
    let quiet = [point(25.0, 0.3); 4];
    assert!(probes_agree(
        &Lattice::new(&lines, &quiet),
        &samples,
        &positions
    ));
    // The blend of a corner of 37 dB and three of 21.7 dB is 25 dB at the block's middle too.
    let beside_a_road = [
        point(37.0, 0.3),
        point(21.7, 0.3),
        point(21.7, 0.3),
        point(21.7, 0.3),
    ];
    for &k in &QUADRANT_PROBES {
        let (x, y) = positions[k];
        let lattice = Lattice::new(&lines, &beside_a_road);
        let blended = lattice.blend_at(x, y, &[0.0; LAYERS]).unwrap().0[0];
        samples[k] = point(10.0 * blended.log10() + 1.5, 0.3);
    }
    assert!(!probes_agree(
        &Lattice::new(&lines, &beside_a_road),
        &samples,
        &positions
    ));
}

/// A road across open ground: its blocks and the ones beside them are painted between a few exact
/// points, the road by its free field at each pixel, where a pass painted every pixel of a block
/// a source crosses or passes; and the map reads as every pixel traced.
#[test]
fn a_road_across_open_ground_is_blended_by_its_free_field() {
    use crate::paint::{Grid, corners_of, paint_pixels};
    use crate::quiet::QuietBlocks;
    use crate::square::{Files, Square};
    use physics::bands::{BANDS, PERIODS};
    use physics::weather::{COLUMNS, ROWS, WeatherNode, WeatherTable, encode as encode_weather};
    use std::sync::atomic::AtomicUsize;
    use tiles::sources::{Attribute, Layer, Piece, encode as encode_sources};

    let root = tempfile::tempdir().unwrap();
    let year = root.path().join("2026");
    std::fs::create_dir(&year).unwrap();
    let weather_bytes = encode_weather(&vec![WeatherNode::default(); ROWS * COLUMNS]);
    std::fs::write(year.join("weather"), &weather_bytes).unwrap();
    std::fs::write(year.join("complete"), b"qmsq1\n").unwrap();
    let weather = WeatherTable::parse(&weather_bytes).unwrap();
    let tile = tiles::geo::TileId { x: 2212, y: 1387 };
    let road = Attribute {
        layer: Layer::Road,
        height_m: 0.05,
        ground_percent: 0,
        platform_half_width_m: 0.0,
        exclusion_radius_m: 0.0,
        footprint_id: 0,
        group_key: 1,
        emission: [[70.0; BANDS]; PERIODS],
        display: vec![serde_json::Value::from("test road")],
    };
    // North to south through the middle of the fourth of eight columns of blocks.
    let pieces: Vec<Piece> = (0..16)
        .map(|k| Piece {
            ends: [[-2048, -16_384 + 2048 * k], [-2048, -14_336 + 2048 * k]],
            attribute: 0,
        })
        .collect();
    let sources = encode_sources(&pieces, &[road], &[]);
    tiles::square::write_tiles(&year, tiles::Kind::Sources, &[(tile, sources)]).unwrap();
    let release = popup::release::Release::open(root.path(), "2026").unwrap();
    let grid = Grid {
        pixels: 64,
        block: 8,
        hum_spacing: 16,
    };
    let files = Files::read(&release, tile).unwrap();
    let square = Square::new(&weather, tile, &files).unwrap();
    let painter = PointPainter {
        square: &square,
        grid,
        batch: None,
        corners: corners_of(&square, grid, None).unwrap().unwrap(),
        pairs: AtomicUsize::new(0),
        points: AtomicUsize::new(0),
        deeper: AtomicUsize::new(0),
        sounds: AtomicUsize::new(0),
    };
    // The free field is the road's in three dimensions: two metres beside it a receiver level
    // with it stands 2.0 m from it, one 4 m over the ground 4.4 m (a road on an embankment rose
    // 10 dB within a pixel, which a free field on level ground blended away).
    let every: Vec<u32> = (0..square.candidates.len() as u32).collect();
    let road_at = |altitude_m: f64| {
        let own = OwnSources::of(&square, &every).unwrap();
        own.free_field(
            &square,
            [square.candidates[0].ends_m[0][0] + 2.0, 0.0],
            altitude_m,
        )[0]
    };
    let ground_m = square.ground.at([0.0, 0.0]).unwrap().height_m;
    let (level, above) = (road_at(ground_m + 0.05), road_at(ground_m + 4.0));
    assert!(
        level > 2.0 * above && level < 2.5 * above,
        "{level} {above}"
    );
    let quiet = QuietBlocks::new(&painter).unwrap();
    // The road's column of blocks: 8 of the 64 pixel columns.
    let crossed = |at: &usize| (24..32).contains(&(at % grid.pixels));
    let without_a_pass = (0..grid.pixels * grid.pixels)
        .filter(|at| crossed(at) && quiet.has_paint(&painter, *at))
        .count();
    // Before, the pixels of the patches the road crosses waited for a pass (368 without one).
    assert!(
        without_a_pass > 480,
        "{without_a_pass} of 512 pixels without a pass"
    );
    // Completed source blocks keep their deepest source count for façade-pass bounds, while
    // open background blocks keep zero.
    let mut near = Vec::new();
    for at in 0..(grid.pixels / grid.block) * (grid.pixels / grid.block) {
        let block = painter.block(&mut near, at);
        assert_eq!(quiet.most_source_counts[at], block.most);
    }
    let corners = corners_of(&square, grid, None).unwrap().unwrap();
    let painted = paint_pixels(&square, grid, None, corners).unwrap();
    let traced = crate::etalon::etalon(&square, grid, None, 1).unwrap();
    let road = Layer::Road as usize;
    let mut off = [[0usize; 4]; 2];
    for at in 0..grid.pixels * grid.pixels {
        let steps = |a: &[[u8; LAYERS + 1]], b: &[[u8; LAYERS + 1]]| {
            usize::from(a[at][road].abs_diff(b[at][road])).min(3)
        };
        off[0][steps(&painted.lden, &traced.lden)] += 1;
        off[1][steps(&painted.nden, &traced.nden)] += 1;
    }
    eprintln!("steps off {off:?}");
    // Cells are half a decibel and half a phon: none over two off, and two at one pixel of 4,096.
    assert_eq!((off[0][3], off[1][3]), (0, 0));
    assert!(off[0][2] <= 4 && off[1][2] <= 4, "{off:?}");
}
