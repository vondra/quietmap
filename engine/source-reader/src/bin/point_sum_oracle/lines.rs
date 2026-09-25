//! Synthetic line scenes on flat ground (hard, mixed, soft; optional roadside wall): a straight
//! road of 250 m pieces, the production quadrature against the fine point sum.

use crate::piece::{add, energies, lden_a, point_sum, production, Bands, Piece};
use noise_compute::compute::line_piece::LinePiece;
use noise_compute::constants::M_PER_DEG_LAT;
use noise_compute::emission::road;
use noise_compute::propagation::geo;
use noise_compute::propagation::obstacle_index::{ObstacleIndex, ObstacleKind, ObstacleSet};
use noise_compute::propagation::point_sum::NodeSpacing;
use noise_compute::propagation::ray_transfer::RayReceiver;
use noise_compute::types::{RasterSampler, NUM_BANDS};
use rayon::prelude::*;
use serde_json::{json, Value};

/// Flat ground at 0 m with one ground factor.
struct FlatWorld {
    ground_g: f64,
}

impl RasterSampler for FlatWorld {
    fn elevation(&self, _: f64, _: f64) -> f64 {
        0.0
    }
    fn ground_g(&self, _: f64, _: f64) -> f64 {
        self.ground_g
    }
}

fn to_geo(x: f64, y: f64) -> (f64, f64) {
    (y / M_PER_DEG_LAT, x / geo::m_per_deg_lon(0.0))
}

/// A secondary road, 5,000 vehicles/day at 70 km/h, urban default split: `L_W′` per period.
fn secondary_road_emission() -> [Bands; 3] {
    let pcts = [[0.7; 4], [0.18; 4], [0.12; 4]];
    let hours = [12.0, 4.0, 8.0];
    std::array::from_fn(|p| {
        let flows = road::build_period_flows(4_600.0, 200.0, 150.0, 50.0, 70.0, pcts[p], hours[p]);
        road::line_source_emission(&flows, 0.0)
    })
}

struct Scene {
    name: String,
    ground_g: f64,
    half_length_m: f64,
    offset_m: f64,
    receiver_height_m: f64,
    /// A wall along the road `(metres north of the axis, height)`.
    wall: Option<(f64, f32)>,
}

fn scenes() -> Vec<Scene> {
    let mut scenes = vec![
        Scene { name: "n28_10m_line_1m_out_3.95m_up".into(), ground_g: 0.0, half_length_m: 5.0, offset_m: 1.0, receiver_height_m: 4.0, wall: None },
        Scene { name: "n28_50m_line_10m_out_30m_up".into(), ground_g: 0.0, half_length_m: 25.0, offset_m: 10.0, receiver_height_m: 30.05, wall: None },
    ];
    for g in [0.0, 0.5, 1.0] {
        for offset in [5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0] {
            scenes.push(Scene { name: format!("road_4km_g{g}_at_{offset}m"), ground_g: g, half_length_m: 2000.0, offset_m: offset, receiver_height_m: 4.0, wall: None });
        }
    }
    for offset in [15.0, 30.0, 60.0, 120.0, 250.0] {
        scenes.push(Scene { name: format!("road_4km_g1_wall3m_at_5m_receiver_{offset}m"), ground_g: 1.0, half_length_m: 2000.0, offset_m: offset, receiver_height_m: 4.0, wall: Some((5.0, 3.0)) });
    }
    scenes
}

fn obstacles(scene: &Scene) -> ObstacleSet {
    let Some((y, height)) = scene.wall else {
        return ObstacleSet::empty();
    };
    let mut builder = ObstacleIndex::builder(0.0, 0.0);
    let a = to_geo(-scene.half_length_m - 500.0, y);
    let b = to_geo(scene.half_length_m + 500.0, y);
    builder.add_polyline(&[a, b], height, ObstacleKind::Barrier, 0);
    ObstacleSet { indexes: vec![std::sync::Arc::new(builder.build())] }
}

pub fn run(spacing: NodeSpacing) -> Value {
    let emission = secondary_road_emission();
    let rows: Vec<Value> = scenes()
        .par_iter()
        .map(|scene| {
            let world = FlatWorld { ground_g: scene.ground_g };
            let set = obstacles(scene);
            let (lat, lon) = to_geo(0.0, scene.offset_m);
            let receiver = RayReceiver { lat, lon, altitude_m: scene.receiver_height_m };
            let pieces = ((2.0 * scene.half_length_m) / 250.0).ceil().max(1.0) as usize;
            let length = 2.0 * scene.half_length_m / pieces as f64;
            let (mut quadrature, mut reference) = ([[0.0; NUM_BANDS]; 3], [[0.0; NUM_BANDS]; 3]);
            let mut nodes = 0;
            for i in 0..pieces {
                let x0 = -scene.half_length_m + length * i as f64;
                let (start, end) = (to_geo(x0, 0.0), to_geo(x0 + length, 0.0));
                let piece = Piece {
                    line: LinePiece { start_lat: start.0, start_lon: start.1, end_lat: end.0, end_lon: end.1, source_height_m: 0.05, source_ground_factor: 0.0, platform_half_width_m: 5.0, directivity: noise_compute::propagation::line_quadrature::LineDirectivity::Omnidirectional },
                    cp: to_geo(0.0_f64.clamp(x0, x0 + length), 0.0),
                    emission_db_per_m: emission,
                };
                if let Some(transfer) = production(&receiver, &piece, &set, &world) {
                    add(&mut quadrature, &energies(&transfer, &piece.emission_db_per_m, 0.0));
                }
                let (transfer, count) = point_sum(&receiver, &piece, &set, &world, spacing);
                nodes += count;
                add(&mut reference, &energies(&transfer, &piece.emission_db_per_m, 0.0));
            }
            let (q, r) = (lden_a(&quadrature), lden_a(&reference));
            json!({"scene": scene.name, "point_sum_nodes": nodes, "point_sum_lden": (r * 1000.0).round() / 1000.0,
                   "quadrature_lden": (q * 1000.0).round() / 1000.0, "quadrature_minus_point_sum_db": ((q - r) * 1000.0).round() / 1000.0})
        })
        .collect();
    Value::Array(rows)
}
