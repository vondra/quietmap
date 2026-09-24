//! Synthetic line-source geometries (flat ground, optional parallel wall) on which every variant
//! is evaluated: the audit's near-road #28 cases, distance ladders over hard, mixed and soft
//! ground (#5, #7) and receivers behind a roadside wall (#27).

use crate::ray::Bands;
use crate::segment::{add_energy, evaluate_segment, LineSegment, ReceiverPoint, World, VARIANTS};
use noise_compute::constants::{A_WEIGHTING, M_PER_DEG_LAT};
use noise_compute::emission::road;
use noise_compute::propagation::geo;
use noise_compute::propagation::obstacle_index::{CrossingCandidate, ObstacleKind};
use noise_compute::propagation::point_sum::NodeSpacing;
use noise_compute::types::{RasterSampler, NUM_BANDS};
use serde_json::{json, Value};

/// Flat ground at 0 m with one ground factor, an optional wall along `y = wall_y_m` (metres
/// north of the road axis) and the origin of the metric frame at (0°, 0°).
struct FlatWorld {
    ground_g: f64,
    wall: Option<(f64, f32)>,
}

impl RasterSampler for FlatWorld {
    fn elevation(&self, _: f64, _: f64) -> f64 {
        0.0
    }
    fn ground_g(&self, _: f64, _: f64) -> f64 {
        self.ground_g
    }
}

fn to_metres(lat: f64, lon: f64) -> (f64, f64) {
    (lon * geo::m_per_deg_lon(0.0), lat * M_PER_DEG_LAT)
}

fn to_geo(x: f64, y: f64) -> (f64, f64) {
    (y / M_PER_DEG_LAT, x / geo::m_per_deg_lon(0.0))
}

impl World for FlatWorld {
    fn rasters(&self) -> &dyn RasterSampler {
        self
    }
    fn crossings(&self, src_lat: f64, src_lon: f64, rcv_lat: f64, rcv_lon: f64, out: &mut Vec<CrossingCandidate>) {
        out.clear();
        let Some((wall_y, height)) = self.wall else {
            return;
        };
        let (_, sy) = to_metres(src_lat, src_lon);
        let (_, ry) = to_metres(rcv_lat, rcv_lon);
        if (sy - wall_y) * (ry - wall_y) < 0.0 {
            out.push(CrossingCandidate {
                t: (wall_y - sy) / (ry - sy),
                height_m: height,
                kind: ObstacleKind::Barrier,
                id: 0,
            });
        }
    }
}

/// A secondary road, 5,000 vehicles/day at 70 km/h, urban default split: `L_W′` per period.
fn secondary_road_emission() -> [Bands; 3] {
    let pcts = [[0.7, 0.7, 0.7, 0.7], [0.18, 0.18, 0.18, 0.18], [0.12, 0.12, 0.12, 0.12]];
    let hours = [12.0, 4.0, 8.0];
    std::array::from_fn(|p| {
        let flows = road::build_period_flows(4_600.0, 200.0, 150.0, 50.0, 70.0, pcts[p], hours[p]);
        road::line_source_emission(&flows, 0.0)
    })
}

/// A straight road along x from `-half_length_m` to `+half_length_m`, cut into production-size
/// 250 m (or shorter) microsegments, with the receiver `offset_m` north at `height_m`.
fn road_segments(half_length_m: f64, offset_m: f64, emission: [Bands; 3]) -> Vec<LineSegment> {
    let pieces = ((2.0 * half_length_m) / 250.0).ceil().max(1.0) as usize;
    let piece = 2.0 * half_length_m / pieces as f64;
    (0..pieces)
        .map(|i| {
            let x0 = -half_length_m + piece * i as f64;
            let x1 = x0 + piece;
            let cx = 0.0_f64.clamp(x0, x1);
            LineSegment {
                start: to_geo(x0, 0.0),
                end: to_geo(x1, 0.0),
                closest: to_geo(cx, 0.0),
                length_m: piece,
                dist_m: (cx * cx + offset_m * offset_m).sqrt(),
                source_height_m: 0.05,
                force_hard_ground: false,
                emission_db_per_m: emission,
            }
        })
        .collect()
}

pub struct Scene {
    pub name: String,
    pub ground_g: f64,
    pub half_length_m: f64,
    pub offset_m: f64,
    pub receiver_height_m: f64,
    pub wall: Option<(f64, f32)>,
}

pub fn scenes() -> Vec<Scene> {
    let mut scenes = vec![
        Scene { name: "audit_n28_10m_line_1m_out_3.95m_up".into(), ground_g: 0.0, half_length_m: 5.0, offset_m: 1.0, receiver_height_m: 4.0, wall: None },
        Scene { name: "audit_n28_50m_line_10m_out_30m_up".into(), ground_g: 0.0, half_length_m: 25.0, offset_m: 10.0, receiver_height_m: 30.05, wall: None },
    ];
    for g in [0.0, 0.5, 1.0] {
        for offset in [5.0, 10.0, 25.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 4000.0] {
            scenes.push(Scene {
                name: format!("road_4km_g{g}_at_{offset}m"),
                ground_g: g,
                half_length_m: 2000.0,
                offset_m: offset,
                receiver_height_m: 4.0,
                wall: None,
            });
        }
    }
    for offset in [15.0, 30.0, 60.0, 120.0, 250.0] {
        scenes.push(Scene {
            name: format!("road_4km_g1_wall3m_at_5m_receiver_{offset}m"),
            ground_g: 1.0,
            half_length_m: 2000.0,
            offset_m: offset,
            receiver_height_m: 4.0,
            wall: Some((5.0, 3.0)),
        });
    }
    scenes
}

/// Lden per band (unweighted) from period band energies.
pub fn lden_bands(energy: &[Bands; 3]) -> Bands {
    std::array::from_fn(|b| {
        10.0 * ((12.0 * energy[0][b] + 4.0 * energy[1][b] * 10f64.powf(0.5) + 8.0 * energy[2][b] * 10.0) / 24.0).log10()
    })
}

pub fn a_weighted_db(bands: &Bands) -> f64 {
    10.0 * bands.iter().zip(A_WEIGHTING).map(|(l, a)| 10f64.powf((l + a) / 10.0)).sum::<f64>().log10()
}

/// Variant rows (Lden per band and A-weighted, plus deltas from today) for summed segment energies.
pub fn variant_rows(totals: &[[Bands; 3]]) -> Value {
    let today = lden_bands(&totals[0]);
    let today_a = a_weighted_db(&today);
    let rows: Vec<Value> = VARIANTS
        .iter()
        .zip(totals)
        .map(|(variant, energy)| {
            let lden = lden_bands(energy);
            let a = a_weighted_db(&lden);
            json!({
                "variant": variant.name,
                "lden_a": (a * 100.0).round() / 100.0,
                "delta_a": ((a - today_a) * 100.0).round() / 100.0,
                "delta_bands": lden.iter().zip(today).map(|(v, t)| ((v - t) * 100.0).round() / 100.0).collect::<Vec<_>>(),
            })
        })
        .collect();
    Value::Array(rows)
}

pub fn run(spacing: NodeSpacing) -> Value {
    let emission = secondary_road_emission();
    let mut out = Vec::new();
    for scene in scenes() {
        let world = FlatWorld { ground_g: scene.ground_g, wall: scene.wall };
        let (lat, lon) = to_geo(0.0, scene.offset_m);
        let receiver = ReceiverPoint { lat, lon, altitude_m: scene.receiver_height_m, reflection_db: 0.0 };
        let mut totals = vec![[[0.0; NUM_BANDS]; 3]; VARIANTS.len()];
        let mut validation: f64 = 0.0;
        for segment in road_segments(scene.half_length_m, scene.offset_m, emission) {
            let result = evaluate_segment(&world, &segment, &receiver, spacing);
            validation = validation.max(result.validation_db);
            for (total, energy) in totals.iter_mut().zip(&result.energies) {
                add_energy(total, energy);
            }
        }
        out.push(json!({
            "scene": scene.name,
            "replica_vs_production_terms_max_db": validation,
            "variants": variant_rows(&totals),
        }));
    }
    Value::Array(out)
}
