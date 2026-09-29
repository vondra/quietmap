//! Acceptance: ISO/TR 17534-4:2020 test cases TC01–TC28, direct path, homogeneous and favourable
//! levels of every octave band within 0.1 dB (fixture transcribed from NoiseModelling b01a9797,
//! GPL-3.0 test data; the values are the ISO/TR's).

use super::*;
use crate::atmosphere::alpha_bands;
use crate::bands::BANDS as NUM_BANDS;
use serde_json::Value;

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/cnossos_iso_tr_17534_4_direct.json"
);

/// One case as vertices with roofs raised as hard ground, walls as candidates.
struct Case {
    name: String,
    sound_power_db: f64,
    source: PlanePoint,
    receiver: PlanePoint,
    source_ground_factor: f64,
    distance: Vec<f64>,
    altitude: Vec<f64>,
    /// G per vertex; each cut point's G holds up to the next, so every change is a step.
    ground: Vec<f64>,
    /// The cut points as terrain candidates (before roofs replace the ground under buildings).
    terrain: Vec<PlanePoint>,
    tops: Vec<PlanePoint>,
    lh: [f64; NUM_BANDS],
    lf: [f64; NUM_BANDS],
}

fn bands(rows: &[&Value], lh: usize, lf: usize) -> ([f64; NUM_BANDS], [f64; NUM_BANDS]) {
    let column = |index: usize| std::array::from_fn(|band| rows[band][index].as_f64().unwrap());
    (column(lh), column(lf))
}

fn parse(case: &Value, rows: &[&Value], lh: usize, lf: usize) -> Case {
    let f = |v: &Value| v.as_f64().unwrap();
    // (distance, altitude, ground factor of the stretch that starts here);
    // cut rows are [type, s_m, z_ground_m, ground_g].
    let mut vertices: Vec<(f64, f64, f64)> = case["cut_points"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| (f(&p[1]), f(&p[2]), f(&p[3])))
        .collect();
    let terrain: Vec<PlanePoint> = vertices.iter().map(|v| (v.0, v.1)).collect();
    let walls = case["walls"].as_array().unwrap();
    let mut enter: Option<(f64, f64)> = None;
    // Wall rows are [s_m, top_z_m, z_ground_m, intersection].
    for wall in walls {
        let (x, top) = (f(&wall[0]), f(&wall[1]));
        match wall[3].as_str().unwrap() {
            "BUILDING_ENTER" => enter = Some((x, top)),
            "BUILDING_EXIT" => {
                let (x0, top0) = enter.take().unwrap();
                let ground_at = |x: f64, list: &[(f64, f64, f64)]| {
                    let mut previous = list[0];
                    for &v in list {
                        if v.0 >= x {
                            let span = v.0 - previous.0;
                            return if span > 0.0 {
                                previous.1 + (x - previous.0) / span * (v.1 - previous.1)
                            } else {
                                v.1
                            };
                        }
                        previous = v;
                    }
                    previous.1
                };
                let g_after = vertices.iter().rfind(|v| v.0 <= x).unwrap().2;
                let (z0, z1) = (ground_at(x0, &vertices), ground_at(x, &vertices));
                vertices.retain(|v| !(x0..=x).contains(&v.0));
                vertices.extend([
                    (x0, z0, 0.0),
                    (x0, top0, 0.0),
                    (x, top, 0.0),
                    (x, z1, g_after),
                ]);
                vertices.sort_by(|a, b| a.0.total_cmp(&b.0));
            }
            _ => {}
        }
    }
    // Stretch-constant G as vertex G: the stretch [k, k+1] keeps G_k up to x_{k+1}, where a step
    // takes the next stretch's value.
    let mut stepped: Vec<(f64, f64, f64)> = Vec::with_capacity(2 * vertices.len());
    for (index, &(x, z, g)) in vertices.iter().enumerate() {
        if index > 0 {
            stepped.push((x, z, vertices[index - 1].2));
        }
        stepped.push((x, z, g));
    }
    let vertices = stepped;
    let (lh, lf) = bands(rows, lh, lf);
    Case {
        name: case["name"].as_str().unwrap().to_owned(),
        sound_power_db: f(&case["sound_power_db"]),
        // Points are [x_m, y_m, z_m, z_ground_m].
        source: (0.0, f(&case["source"][2])),
        receiver: (f(&case["horizontal_distance_m"]), f(&case["receiver"][2])),
        source_ground_factor: f(&case["cut_points"][0][3]),
        distance: vertices.iter().map(|v| v.0).collect(),
        altitude: vertices.iter().map(|v| v.1).collect(),
        ground: vertices.iter().map(|v| v.2).collect(),
        terrain,
        tops: walls.iter().map(|w| (f(&w[0]), f(&w[1]))).collect(),
        lh,
        lf,
    }
}

#[test]
fn every_direct_case_matches_iso_tr_17534_4_within_a_tenth_of_a_decibel() {
    let fixture: Value = serde_json::from_str(&std::fs::read_to_string(FIXTURE).unwrap()).unwrap();
    let alpha = alpha_bands(10.0, 70.0);
    let columns = fixture["band_columns"].as_array().unwrap();
    let at = |name: &str| columns.iter().position(|c| c == name).unwrap();
    let (lh, lf) = (at("LH"), at("LF"));
    let rows = fixture["bands"].as_array().unwrap();
    assert_eq!(rows.len(), 28 * NUM_BANDS, "one row per case and band");
    let mut scratch = VerticalPathScratch::default();
    let mut failures = Vec::new();
    for raw in fixture["cases"].as_array().unwrap() {
        let name = raw["name"].as_str().unwrap();
        let case_rows: Vec<&Value> = rows
            .iter()
            .filter(|row| row[0].as_str().unwrap() == name)
            .collect();
        assert_eq!(case_rows.len(), NUM_BANDS, "{name} has one row per band");
        let case = parse(raw, &case_rows, lh, lf);
        let path = VerticalPath {
            profile: VerticalProfile {
                distance_m: &case.distance,
                altitude_m: &case.altitude,
                ground_factor: &case.ground,
            },
            source: case.source,
            receiver: case.receiver,
            source_ground_factor: case.source_ground_factor,
            terrain_candidates: &case.terrain,
            obstacle_tops: &case.tops,
        };
        let direct = rubber_band::distance(case.source, case.receiver);
        let base: [f64; NUM_BANDS] = std::array::from_fn(|band| {
            case.sound_power_db - (20.0 * direct.log10() + 11.0) - alpha[band] * direct / 1000.0
        });
        for (state, reference) in [
            (MeteorologicalState::Homogeneous, case.lh),
            (MeteorologicalState::Favourable, case.lf),
        ] {
            let boundary = state_boundary(&path, state, &mut scratch);
            for band in 0..NUM_BANDS {
                let level = base[band] - boundary.attenuation_db[band];
                if !level.is_finite() || (level - reference[band]).abs() > 0.1 {
                    failures.push(format!(
                        "{} {state:?} band {band}: {level:.2} vs {:.2}",
                        case.name, reference[band]
                    ));
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} band results off:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
