//! The ring loop on a synthetic release (flat ocean, one tile of road pieces): the fast answer
//! stays within 0.1 dB of the exact one while evaluating fewer pieces, the list's last row counts
//! the sources it holds, a click inside a building answers at the façade facing the road, and a
//! complete release is required.

use physics::bands::{BANDS, PERIODS};
use physics::weather::{COLUMNS, ROWS, SECTORS, WeatherNode, encode as encode_weather};
use popup::answer::{Options, answer};
use popup::release::Release;
use std::path::{Path, PathBuf};
use tiles::geo::{LocalFrame, TileId};
use tiles::obstacles::{EnvelopeClass, Outline, OutlineKind, encode as encode_obstacles};
use tiles::sources::{Attribute, Layer, Piece, encode};
use tiles::{COMPLETION_MARKER, Kind, tile_path};

const TILE: TileId = TileId { x: 2212, y: 1387 };

fn release_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("qm-ring-loop-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("2026")).unwrap();
    let weather = vec![
        WeatherNode {
            percent: [[50u8; SECTORS]; PERIODS],
            ..WeatherNode::default()
        };
        ROWS * COLUMNS
    ];
    std::fs::write(root.join("weather"), encode_weather(&weather)).unwrap();
    root
}

/// Road pieces of 100 m running north-south at `distances` east of the tile centre: one road, or
/// with `separate` each piece a road of its own.
fn write_roads(root: &Path, distances: &[f64], separate: bool) {
    let steps_per_metre = steps_per_metre();
    let half = (50.0 * steps_per_metre).round() as i16;
    let pieces: Vec<Piece> = distances
        .iter()
        .enumerate()
        .map(|(index, &distance)| {
            let x = (distance * steps_per_metre).round() as i16;
            Piece {
                ends: [[x, -half], [x, half]],
                attribute: if separate { index as u32 } else { 0 },
            }
        })
        .collect();
    let roads = if separate { distances.len() } else { 1 };
    let attributes: Vec<Attribute> = (0..roads)
        .map(|index| Attribute {
            layer: Layer::Road,
            height_m: 0.05,
            ground_percent: 0,
            platform_half_width_m: 5.0,
            exclusion_radius_m: 0.0,
            footprint_id: 0,
            group_key: index as u64 + 1,
            emission: [[70.0; BANDS]; PERIODS],
            display: format!(r#"["test road {index}"]"#),
        })
        .collect();
    let path = tile_path(&root.join("2026"), TILE, Kind::Sources);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, encode(&pieces, &attributes)).unwrap();
}

/// Int16 steps per metre east in the tile's frame.
fn steps_per_metre() -> f64 {
    32_768.0 / LocalFrame::at(TILE.centre()).east_m_per_unit
}

/// A 20 m square building, 15 m tall, centred on the tile centre.
fn write_building(root: &Path) {
    let s = (10.0 * steps_per_metre()).round() as i16;
    let outline = Outline {
        footprint_id: 7,
        kind: OutlineKind::Exterior,
        envelope: EnvelopeClass::Residential,
        height_m: 15.0,
        vertices: vec![[-s, s], [s, s], [s, -s], [-s, -s], [-s, s]],
    };
    let path = tile_path(&root.join("2026"), TILE, Kind::Obstacles);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, encode_obstacles(&[outline])).unwrap();
}

/// The last update of a click `east_m` east of the tile centre: road Lden energy and the
/// chosen façade's bearing, if the click is inside a building.
fn click(release: &Release, east_m: f64) -> (f64, Option<f64>) {
    let frame = LocalFrame::at(TILE.centre());
    let (lat, lon) = frame.to_mercator([east_m, 0.0]).to_degrees();
    let mut last = None;
    answer(
        release,
        lat,
        lon,
        &Options {
            exact: true,
            source: Vec::new(),
        },
        &mut |update| {
            let road = update
                .layers
                .iter()
                .find(|layer| layer.layer == Layer::Road)
                .unwrap();
            let bearing = update
                .building
                .and_then(|building| building.facade)
                .map(|facade| facade.outward_bearing_deg);
            last = Some((physics::bands::lden_energy(&road.energy), bearing));
            Ok(())
        },
    )
    .unwrap();
    last.unwrap()
}

/// The road layer's per-period energies and evaluated count of the last update.
fn road_answer(release: &Release, exact: bool) -> ([f64; PERIODS], usize, usize) {
    let (lat, lon) = TILE.centre().to_degrees();
    let mut last = None;
    answer(
        release,
        lat,
        lon,
        &Options {
            exact,
            source: Vec::new(),
        },
        &mut |update| {
            let road = update
                .layers
                .iter()
                .find(|layer| layer.layer == Layer::Road)
                .unwrap();
            last = Some((road.energy, road.evaluated, update.rows.len()));
            Ok(())
        },
    )
    .unwrap();
    last.unwrap()
}

#[test]
fn the_fast_answer_is_within_a_tenth_of_a_decibel_of_the_exact_one() {
    let root = release_root("fast");
    let distances: Vec<f64> = (0..40)
        .map(|i| 20.0 * 1.15f64.powi(i))
        .filter(|d| *d < 2_900.0)
        .collect();
    write_roads(&root, &distances, false);
    std::fs::write(root.join("2026").join(COMPLETION_MARKER), "test").unwrap();
    let release = Release::open(&root, "2026").unwrap();
    let (exact, exact_count, contributors) = road_answer(&release, true);
    let (fast, fast_count, _) = road_answer(&release, false);
    assert_eq!(exact_count, distances.len());
    assert_eq!(contributors, 1, "every piece shares one display group");
    for period in 0..PERIODS {
        let difference = 10.0 * (exact[period] / fast[period]).log10();
        assert!(
            (0.0..=0.1).contains(&difference),
            "period {period}: {difference} dB"
        );
    }
    assert!(fast_count < exact_count, "{fast_count} of {exact_count}");
    std::fs::remove_dir_all(&root).unwrap();
}

/// The list's last row counts the sources it holds: of forty roads the thirty listed leave ten.
#[test]
fn the_last_row_counts_the_sources_the_list_leaves_out() {
    let root = release_root("rest");
    let distances: Vec<f64> = (0..40).map(|i| 30.0 + 20.0 * f64::from(i)).collect();
    write_roads(&root, &distances, true);
    std::fs::write(root.join("2026").join(COMPLETION_MARKER), "test").unwrap();
    let release = Release::open(&root, "2026").unwrap();
    let (lat, lon) = TILE.centre().to_degrees();
    let mut last = None;
    answer(
        &release,
        lat,
        lon,
        &Options {
            exact: true,
            source: Vec::new(),
        },
        &mut |update| {
            let road = update
                .layers
                .iter()
                .find(|layer| layer.layer == Layer::Road)
                .unwrap();
            last = Some((update.rows.len(), road.unlisted_sources));
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(last.unwrap(), (30, 10));
    std::fs::remove_dir_all(&root).unwrap();
}

/// An asked source's sound path lists every piece of it, each evaluated, loudest first, adding up
/// to the source (one group here, the whole layer), the loudest traced ray by ray.
#[test]
fn an_asked_source_lists_every_piece_of_it() {
    let root = release_root("listed");
    write_roads(&root, &[30.0, 60.0, 90.0, 120.0], false);
    std::fs::write(root.join("2026").join(COMPLETION_MARKER), "test").unwrap();
    let release = Release::open(&root, "2026").unwrap();
    let (lat, lon) = TILE.centre().to_degrees();
    let mut last = None;
    answer(
        &release,
        lat,
        lon,
        &Options {
            exact: false,
            source: vec![1],
        },
        &mut |update| {
            let road = update
                .layers
                .iter()
                .find(|layer| layer.layer == Layer::Road);
            last = Some((road.unwrap().energy, update.pieces.clone()));
            Ok(())
        },
    )
    .unwrap();
    let (road, pieces) = last.unwrap();
    assert_eq!(pieces.len(), 4);
    for period in 0..PERIODS {
        let summed: f64 = pieces.iter().map(|piece| piece.energy[period]).sum();
        assert!(
            (summed / road[period] - 1.0).abs() < 1e-9,
            "{summed} {road:?}"
        );
    }
    assert!(
        pieces
            .windows(2)
            .all(|pair| pair[0].energy[0] >= pair[1].energy[0])
    );
    assert!(
        pieces
            .iter()
            .all(|piece| piece.trace.is_some() && !piece.rays.is_empty())
    );
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_click_inside_a_building_answers_at_the_facade_facing_the_road() {
    let root = release_root("building");
    write_roads(&root, &[40.0], false);
    write_building(&root);
    std::fs::write(root.join("2026").join(COMPLETION_MARKER), "test").unwrap();
    let release = Release::open(&root, "2026").unwrap();
    let (inside, bearing) = click(&release, 0.0);
    let bearing = bearing.expect("the click stands in the building");
    assert!((bearing - 90.0).abs() < 1.0, "façade bearing {bearing}");
    // Outdoors 5 m behind the building the road is screened; at the east façade it is not.
    let (behind, none) = click(&release, -15.0);
    assert!(none.is_none());
    let difference = 10.0 * (inside / behind).log10();
    assert!(difference > 10.0, "façade {difference} dB above the back");
    std::fs::remove_dir_all(&root).unwrap();
}

/// Point sources on the north edge of the northernmost read tile: their rays start on the edge
/// and must stay in their own tile, the tile beyond not being read yet (a click in the Brdy
/// failed so before lattice points returned to the lattice exactly).
#[test]
fn a_source_on_the_edge_of_the_read_block_is_answered() {
    let root = release_root("edge");
    let (lat, lon) = (49.758, 13.865);
    let centre = TileId::containing(tiles::geo::Mercator::from_degrees(lat, lon));
    let north = TileId {
        x: centre.x + 1,
        y: centre.y - 1,
    };
    let attributes = [Attribute {
        layer: Layer::Building,
        height_m: 5.0,
        ground_percent: 0,
        platform_half_width_m: 0.0,
        exclusion_radius_m: 3.5,
        footprint_id: 0,
        group_key: 2,
        emission: [[80.0; BANDS]; PERIODS],
        display: r#"["edge"]"#.into(),
    }];
    // Many points along the edge: whether one rounds across it depends on its position.
    let pieces: Vec<Piece> = (-16_384..16_384)
        .step_by(16)
        .map(|x| Piece {
            ends: [[x, -16_384], [x, -16_384]],
            attribute: 0,
        })
        .collect();
    let path = tile_path(&root.join("2026"), north, Kind::Sources);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, encode(&pieces, &attributes)).unwrap();
    std::fs::write(root.join("2026").join(COMPLETION_MARKER), "test").unwrap();
    let release = Release::open(&root, "2026").unwrap();
    let mut building_energy = 0.0;
    answer(
        &release,
        lat,
        lon,
        &Options {
            exact: true,
            source: Vec::new(),
        },
        &mut |update| {
            let layer = update
                .layers
                .iter()
                .find(|layer| layer.layer == Layer::Building)
                .unwrap();
            building_energy = layer.energy[0];
            Ok(())
        },
    )
    .unwrap();
    assert!(building_energy > 0.0);
    std::fs::remove_dir_all(&root).unwrap();
}

/// Many point sources, far more than the proven rule's per-call limit: the sampled rest keeps
/// the answer within 0.05 dB of exact with a fraction of the evaluations, identically every time.
#[test]
fn the_sampled_answer_is_within_a_twentieth_of_a_decibel_and_reproducible() {
    let root = release_root("sampled");
    let steps_per_metre = steps_per_metre();
    // 6,000 points on a spiral 20 m to 2.9 km out, levels varying by 20 dB.
    let pieces: Vec<Piece> = (0..6_000)
        .map(|i| {
            let f = f64::from(i);
            let radius = 20.0 * (2_900.0f64 / 20.0).powf(f / 6_000.0);
            let angle = f * 2.399_963;
            let point = [
                (radius * angle.cos() * steps_per_metre).round() as i16,
                (radius * angle.sin() * steps_per_metre).round() as i16,
            ];
            Piece {
                ends: [point, point],
                attribute: (i % 4) as u32,
            }
        })
        .collect();
    let attributes: Vec<Attribute> = (0..4)
        .map(|level| Attribute {
            layer: Layer::Industry,
            height_m: 2.0,
            ground_percent: 50,
            platform_half_width_m: 0.0,
            exclusion_radius_m: 0.0,
            footprint_id: 0,
            group_key: 10 + level,
            emission: [[70.0 + 20.0 * level as f64 / 3.0; BANDS]; PERIODS],
            display: r#"["point"]"#.into(),
        })
        .collect();
    let path = tile_path(&root.join("2026"), TILE, Kind::Sources);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, encode(&pieces, &attributes)).unwrap();
    std::fs::write(root.join("2026").join(COMPLETION_MARKER), "test").unwrap();
    let release = Release::open(&root, "2026").unwrap();
    let industry = |exact: bool| {
        let (lat, lon) = TILE.centre().to_degrees();
        let mut last = None;
        answer(
            &release,
            lat,
            lon,
            &Options {
                exact,
                source: Vec::new(),
            },
            &mut |update| {
                let layer = update
                    .layers
                    .iter()
                    .find(|layer| layer.layer == Layer::Industry)
                    .unwrap();
                last = Some((layer.energy, layer.evaluated, layer.candidates));
                Ok(())
            },
        )
        .unwrap();
        last.unwrap()
    };
    let (exact, exact_count, candidates) = industry(true);
    let (sampled, sampled_count, sampled_candidates) = industry(false);
    assert_eq!(
        (exact_count, candidates, sampled_candidates),
        (6_000, 6_000, 6_000)
    );
    assert!(sampled_count < exact_count / 2, "{sampled_count} evaluated");
    for period in 0..PERIODS {
        let difference = 10.0 * (sampled[period] / exact[period]).log10();
        assert!(difference.abs() <= 0.05, "period {period}: {difference} dB");
    }
    assert_eq!(
        industry(false).0,
        sampled,
        "the same click, the same sample"
    );
    std::fs::remove_dir_all(&root).unwrap();
}

/// Weak low points and a few strong high ones: the sample draws mostly the weak ones, and its
/// spectrum weighs each drawn piece as the estimate does, so the loudness of the fast answer is the
/// exact one's (unweighted, the high band took 99 % of the spectrum: 7.2 sone for 11.9).
#[test]
fn a_sampled_layer_sounds_as_the_exact_one() {
    let root = release_root("sampled-spectrum");
    let steps_per_metre = steps_per_metre();
    let pieces: Vec<Piece> = (0..6_060)
        .map(|i| {
            let f = f64::from(i);
            let radius = 20.0 * (2_900.0f64 / 20.0).powf(f / 6_060.0);
            let angle = f * 2.399_963;
            let point = [
                (radius * angle.cos() * steps_per_metre).round() as i16,
                (radius * angle.sin() * steps_per_metre).round() as i16,
            ];
            Piece {
                ends: [point, point],
                attribute: u32::from(i % 101 == 0),
            }
        })
        .collect();
    // Band 0 is 63 Hz, band 4 is 1 kHz; the weak points' A-weighted energy (63 Hz, -26.2 dB)
    // equals the strong ones' over the hundredfold count.
    let band = |index: usize, level: f64| {
        let mut bands = [0.0; BANDS];
        bands[index] = level;
        [bands; PERIODS]
    };
    let attributes: Vec<Attribute> = [band(0, 106.2), band(4, 100.0)]
        .into_iter()
        .enumerate()
        .map(|(k, emission)| Attribute {
            layer: Layer::Industry,
            height_m: 2.0,
            ground_percent: 50,
            platform_half_width_m: 0.0,
            exclusion_radius_m: 0.0,
            footprint_id: 0,
            group_key: 20 + k as u64,
            emission,
            display: r#"["point"]"#.into(),
        })
        .collect();
    let path = tile_path(&root.join("2026"), TILE, Kind::Sources);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, encode(&pieces, &attributes)).unwrap();
    std::fs::write(root.join("2026").join(COMPLETION_MARKER), "test").unwrap();
    let release = Release::open(&root, "2026").unwrap();
    let sone = |exact: bool| {
        let (lat, lon) = TILE.centre().to_degrees();
        let mut last = None;
        answer(
            &release,
            lat,
            lon,
            &Options {
                exact,
                source: Vec::new(),
            },
            &mut |update| {
                last = update.loudness.as_ref().map(|loudness| loudness.nden_sone);
                Ok(())
            },
        )
        .unwrap();
        last.expect("a final loudness")
    };
    let (exact, sampled) = (sone(true), sone(false));
    assert!(
        (sampled / exact - 1.0).abs() < 0.05,
        "sampled {sampled:.2} sone, exact {exact:.2}"
    );
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn an_incomplete_release_is_never_served() {
    let root = release_root("incomplete");
    assert!(Release::open(&root, "2026").is_err());
    std::fs::remove_dir_all(&root).unwrap();
}
