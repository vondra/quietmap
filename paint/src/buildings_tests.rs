//! A building's level: the loudest façade by the ground sources, and the receivers a pass need not sound.

use super::*;
use crate::blocks::Block;
use crate::corner_sources::SECTORS;
use crate::exact::{Batch, Evaluated, Pair, PairEnergy};
use crate::flight_boxes::{FlightBoxes, FlightRequest};
use crate::flights::FlightSums;
use crate::paint::{Grid, corners_of};
use crate::square::Files;
use physics::bands::{BANDS, PERIODS};
use physics::weather::{COLUMNS, ROWS, WeatherNode, WeatherTable, encode as encode_weather};
use popup::evaluate::Bands;
use popup::release::Release;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use tiles::Kind;
use tiles::geo::TileId;
use tiles::sources::{Attribute, Piece, encode as encode_sources};
use tiles::square::write_tiles;

/// A building takes its loudest ground-source candidate; aircraft do not choose its façade. Each
/// part of one input record fills only its own pixels, and a part without a candidate keeps none.
#[test]
fn a_building_reads_its_loudest_facade() {
    let aircraft = Layer::Aircraft as usize;
    let mut loud = [1000.0; LAYERS];
    loud[aircraft] = 1.0;
    let mut overflown = [10.0; LAYERS];
    overflown[aircraft] = 1e9;
    let levels = loudest_by_ground(
        [
            (0, Some((overflown, [1.0; LAYERS + 1]))),
            (0, Some((loud, [8.0; LAYERS + 1]))),
            (1, None),
        ]
        .into_iter(),
    );
    let level = (loud, [8.0; LAYERS + 1]);
    assert_eq!(levels[&0], level);
    assert!(!levels.contains_key(&1));
    let part = |pixels| BuildingPixels {
        footprint: Footprint {
            id: 7,
            height_m: 10.0,
            envelope: tiles::obstacles::EnvelopeClass::Residential,
            rings: Vec::new(),
        },
        pixels,
    };
    let mut pixels: Vec<Option<Painted>> = vec![None; 6];
    fill_buildings(&mut pixels, vec![part(vec![1, 2]), part(vec![4])], &levels);
    assert_eq!(pixels, [None, Some(level), Some(level), None, None, None]);
}

/// The receivers a pass does not sound are those no building reads: every building reads the
/// same receiver with them unpainted, wherever the passes cut its receivers apart, equal
/// energies to the first, and any pass containing a NaN sounded whole so the unpruned reduction
/// (including a NaN in a building's first slot) is kept.
#[test]
fn a_building_reads_the_same_facade_with_the_others_of_each_pass_unpainted() {
    let footprints = [7, 7, 9, 7, 9, 5, 9, 4, 4, 4, 2, 2];
    let ground = [
        1.0,
        3.0,
        2.0,
        3.0,
        2.0,
        0.0,
        2.0,
        1.0,
        f64::NAN,
        2.0,
        f64::NAN,
        5.0,
    ];
    let paint = |k: usize| -> Painted {
        let mut layers = [0.0; LAYERS];
        layers[Layer::Road as usize] = ground[k];
        (layers, [k as f64; LAYERS + 1])
    };
    assert_eq!(
        loudest_receivers(&footprints[..7], &ground[..7]),
        [false, true, true, false, false, true, false]
    );
    assert_eq!(
        loudest_receivers(&footprints[7..10], &ground[7..10]),
        [true, true, true]
    );
    // Each building with the receiver it reads, by the receiver's number.
    let read = |levels: HashMap<usize, Painted>| -> Vec<(usize, f64)> {
        let mut read: Vec<_> = (levels.into_iter())
            .map(|(building, paint)| (building, paint.1[0]))
            .collect();
        read.sort_by_key(|&(building, _)| building);
        read
    };
    let all = read(loudest_by_ground(
        (0..footprints.len()).map(|k| (footprints[k], Some(paint(k)))),
    ));
    assert_eq!(all, [(2, 10.0), (4, 9.0), (5, 5.0), (7, 1.0), (9, 2.0)]);
    for cut in 0..=footprints.len() {
        let kept = [0..cut, cut..footprints.len()]
            .into_iter()
            .flat_map(|pass| {
                let loudest = loudest_receivers(&footprints[pass.clone()], &ground[pass.clone()]);
                pass.zip(loudest)
                    .map(|(k, loudest)| (footprints[k], loudest.then(|| paint(k))))
            });
        assert_eq!(read(loudest_by_ground(kept)), all, "passes cut at {cut}");
    }
}

/// Skipping a losing façade with eight source groups before the winner in the same chunk must leave
/// the winner's packed sound (steady energy, line count, lines and curves) identical on the
/// four-group cutoff input of `sone::tests::pixel_sound_does_not_depend_on_prior_scratch_groups`.
#[test]
fn a_winning_facade_keeps_the_same_sound_when_its_loser_is_skipped() {
    struct ConstructedBatch {
        keys: Vec<u64>,
        packed: Mutex<Vec<Vec<f32>>>,
    }

    impl Batch for ConstructedBatch {
        fn evaluate(&self, points: &[Point], pairs: &[Pair]) -> Result<Evaluated, String> {
            let mut evaluated = Vec::with_capacity(pairs.len());
            let mut bands = vec![[[[0.0; BANDS]; PERIODS]; LAYERS]; points.len()];
            for pair in pairs {
                let energy = if pair.point == 0 {
                    1.0
                } else {
                    let relative = match self.keys[pair.candidate as usize] {
                        1 | 4 => 0.05,
                        13 => 1.0,
                        14 => 9998.9,
                        other => panic!("unexpected winner key {other}"),
                    };
                    relative * 1024.0
                };
                evaluated.push(PairEnergy {
                    mixed: [energy; PERIODS],
                    states: [[energy; PERIODS]; 2],
                });
                bands[pair.point as usize][Layer::Building as usize]
                    .iter_mut()
                    .for_each(|period| period[4] += energy);
            }
            Ok(Evaluated {
                pairs: evaluated,
                bands,
            })
        }

        fn pair_spectra(
            &self,
            _: &[Point],
            _: &[Pair],
        ) -> Result<Vec<(PairEnergy, Bands)>, String> {
            unreachable!("not used by façade painting")
        }

        fn loudness(
            &self,
            input: &[f32],
            offsets: &[u64],
        ) -> Result<Vec<[f32; LAYERS + 1]>, String> {
            let mut recorded = self.packed.lock().unwrap();
            for (slot, &start) in offsets.iter().enumerate() {
                let end = offsets
                    .get(slot + 1)
                    .map_or(input.len(), |&next| next as usize);
                recorded.push(input[start as usize..end].to_vec());
            }
            Ok(vec![[1.0; LAYERS + 1]; offsets.len()])
        }

        fn flight_sums(
            &self,
            _: &Square,
            _: &FlightBoxes,
            _: &[FlightRequest],
        ) -> Result<Vec<[FlightSums; 2]>, String> {
            unreachable!("not used by façade painting")
        }
    }

    let root = tempfile::tempdir().unwrap();
    let year = root.path().join("2026");
    std::fs::create_dir(&year).unwrap();
    let weather_bytes = encode_weather(&vec![WeatherNode::default(); ROWS * COLUMNS]);
    std::fs::write(year.join("weather"), &weather_bytes).unwrap();
    tiles::mark_year_complete(&year, br#"{"year":"2026","as_of":"2027-01-01"}"#, "test").unwrap();
    let weather = WeatherTable::parse(&weather_bytes).unwrap();
    let tile = TileId { x: 2212, y: 1387 };
    let keys: [u64; 10] = [0, 1, 2, 3, 4, 5, 6, 7, 13, 14];
    let display = vec![
        serde_json::json!(""),
        serde_json::json!(""),
        serde_json::json!(0.0),
        serde_json::json!(0),
        serde_json::json!(0),
        serde_json::json!(""),
        serde_json::json!(0.0),
        serde_json::json!(0.0),
        serde_json::json!([0.0, 0.0, 0.0]),
        serde_json::json!([0.00098, 0.00098, 0.00098]),
        serde_json::json!(""),
    ];
    let attributes: Vec<_> = keys
        .iter()
        .map(|&group_key| Attribute {
            layer: Layer::Building,
            height_m: 1.0,
            ground_percent: 0,
            platform_half_width_m: 0.0,
            exclusion_radius_m: 0.0,
            footprint_id: 0,
            group_key,
            emission: [[100.0; BANDS]; PERIODS],
            display: display.clone(),
        })
        .collect();
    let pieces: Vec<_> = (0..keys.len() as u32)
        .map(|attribute| Piece {
            ends: [[0, 0]; 2],
            attribute,
        })
        .collect();
    write_tiles(
        &year,
        Kind::Sources,
        &[(tile, encode_sources(&pieces, &attributes, &[]))],
    )
    .unwrap();
    let release = Release::open(root.path(), "2026").unwrap();
    let files = Files::read(&release, tile).unwrap();
    let square = Square::new(&weather, tile, &files).unwrap();
    let grid = Grid {
        pixels: 16,
        block: 8,
        hum_spacing: 16,
    };
    let corners = corners_of(&square, grid, None).unwrap().unwrap();
    assert_eq!(corners.hum.energies_at(0, 0), [[0.0; PERIODS]; LAYERS]);
    let candidate_of = |key: u64| -> u32 {
        square
            .candidates
            .iter()
            .position(|c| c.group_key == key)
            .unwrap() as u32
    };
    let block_of = |mut exact: Vec<u32>| {
        exact.sort_unstable();
        Block {
            most: exact.len(),
            own: exact.clone(),
            exact,
            far: Vec::new(),
            left_out: Box::new([[[[0.0; PERIODS]; SECTORS]; LAYERS]; 4]),
            left_groups: Vec::new(),
            left_bins: Vec::new(),
        }
    };
    let blocks = [
        block_of((0..8).map(candidate_of).collect()),
        block_of([1, 14, 4, 13].into_iter().map(candidate_of).collect()),
    ];
    let point = Point::with_receiver_properties(&square, [0.0, 0.0], 0, 0.0).unwrap();
    let points = [point.clone(), point];
    let places = [(0, 0), (1, 0)];
    let batch = ConstructedBatch {
        keys: square.candidates.iter().map(|c| c.group_key).collect(),
        packed: Mutex::new(Vec::new()),
    };
    let painter = PointPainter {
        square: &square,
        grid,
        batch: Some(&batch),
        corners,
        pairs: AtomicUsize::new(0),
        points: AtomicUsize::new(0),
        deeper: AtomicUsize::new(0),
        sounds: AtomicUsize::new(0),
    };

    let sounded = painter
        .paint_beside(&blocks, &points, &places, &|_, _| None)
        .unwrap();
    let packed_sounded = std::mem::take(&mut *batch.packed.lock().unwrap());
    assert!(sounded[0].is_some() && sounded[1].is_some());
    assert_eq!(packed_sounded.len(), 2);

    let pruned = painter
        .paint_chosen(&blocks, &points, &places, &|ground| {
            loudest_receivers(&[7, 7], ground)
        })
        .unwrap();
    let packed_pruned = std::mem::take(&mut *batch.packed.lock().unwrap());
    assert!(pruned[0].is_none() && pruned[1].is_some());
    assert_eq!(packed_pruned.len(), 1);

    // Identical Lden energies, packed PixelSound (bit-for-bit, including NaN sentinels) and
    // building winner.
    let bits = |floats: &[f32]| {
        floats
            .iter()
            .map(|value| value.to_bits())
            .collect::<Vec<_>>()
    };
    assert_eq!(pruned[1].unwrap().0, sounded[1].unwrap().0);
    assert_eq!(bits(&packed_pruned[0]), bits(&packed_sounded[1]));
    // Confirm the input sits on the exact cutoff where key order keeps group 13 steady (1 line,
    // steady = 1126.4) rather than intermittent (2 lines, steady = 102.4).
    let building_offset = (Layer::Building as usize) * (PERIODS + 1);
    assert!((packed_pruned[0][building_offset] - 1126.4).abs() < 1e-2);
    assert_eq!(packed_pruned[0][building_offset + PERIODS], 1.0);
    assert_eq!(
        loudest_by_ground([(7, pruned[0]), (7, pruned[1])].into_iter())[&7],
        loudest_by_ground([(7, sounded[0]), (7, sounded[1])].into_iter())[&7]
    );
}
