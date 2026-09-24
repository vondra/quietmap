//! CNOSSOS-EU test cases TC01–TC28 (ISO/TR 17534-4, direct path) on today's kernel: each cut
//! profile becomes the engine's sampled ray plus vector crossings, and L_H, L_F and their
//! p = 0.5 mix are compared band by band with the published reference.

use crate::ray::{ray_terms, validate_against_production, Bands, Method, RayTerms};
use noise_compute::constants::{A_WEIGHTING, ALPHA_ATM};
use noise_compute::propagation::iso9613::{propagate_variants_cnossos_ground_full, GroundPath, SourceGeometry};
use noise_compute::propagation::obstacle_index::{CrossingCandidate, ObstacleKind};
use noise_compute::propagation::path_effects::{self, cnossos_ground_path_from_profile, ObstacleInput};
use noise_compute::propagation::path_profile::{fill_t_values, median_step_m};
use noise_compute::propagation::point_sum::point_source_divergence_db;
use noise_compute::propagation::PathProfile;
use noise_compute::types::NUM_BANDS;
use serde_json::{json, Value};

struct CutPoint {
    s_m: f64,
    z_ground_m: f64,
    ground_g: f64,
}

struct Case {
    name: String,
    sound_power_db: f64,
    source_z_m: f64,
    receiver_z_m: f64,
    distance_m: f64,
    cut_points: Vec<CutPoint>,
    walls: Vec<(f64, f64, f64, bool)>,
    reference_lh: Bands,
    reference_lf: Bands,
    other_paths: Vec<String>,
}

fn bands(value: &Value) -> Bands {
    std::array::from_fn(|i| value[i].as_f64().expect("band value"))
}

fn parse(case: &Value) -> Case {
    let f = |v: &Value| v.as_f64().expect("number");
    let cut_points = case["cut_points"]
        .as_array()
        .expect("cut points")
        .iter()
        .map(|p| CutPoint {
            s_m: f(&p["s_m"]),
            z_ground_m: f(&p["z_ground_m"]),
            ground_g: f(&p["ground_g"]),
        })
        .collect();
    let walls = case["walls"]
        .as_array()
        .expect("walls")
        .iter()
        .map(|w| {
            let barrier = w["intersection"].as_str() == Some("THIN_WALL_ENTER_EXIT");
            (f(&w["s_m"]), f(&w["top_z_m"]), f(&w["z_ground_m"]), barrier)
        })
        .collect();
    Case {
        name: case["name"].as_str().expect("name").to_owned(),
        sound_power_db: f(&case["sound_power_db"]),
        source_z_m: f(&case["source"]["z_m"]),
        receiver_z_m: f(&case["receiver"]["z_m"]),
        distance_m: f(&case["horizontal_distance_m"]),
        cut_points,
        walls,
        reference_lh: bands(&case["reference_direct"]["LH"]),
        reference_lf: bands(&case["reference_direct"]["LF"]),
        other_paths: case["reference_other_paths"]
            .as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
            .unwrap_or_default(),
    }
}

impl Case {
    /// Terrain polyline through every cut point, linear between them.
    fn ground_z(&self, s: f64) -> f64 {
        let pts = &self.cut_points;
        let mut sorted: Vec<&CutPoint> = pts.iter().collect();
        sorted.sort_by(|a, b| a.s_m.total_cmp(&b.s_m));
        if s <= sorted[0].s_m {
            return sorted[0].z_ground_m;
        }
        for pair in sorted.windows(2) {
            if s <= pair[1].s_m {
                let span = pair[1].s_m - pair[0].s_m;
                let frac = if span > 0.0 { (s - pair[0].s_m) / span } else { 1.0 };
                return pair[0].z_ground_m + frac * (pair[1].z_ground_m - pair[0].z_ground_m);
            }
        }
        sorted[sorted.len() - 1].z_ground_m
    }

    /// Ground factor from the last cut point at or before `s` (list order breaks ties).
    fn ground_g(&self, s: f64) -> f64 {
        let mut g = self.cut_points[0].ground_g;
        for p in &self.cut_points {
            if p.s_m <= s + 1e-9 {
                g = p.ground_g;
            }
        }
        g
    }

    /// The ray as the engine samples it: its production cadence, or (`dense`) every metre plus
    /// every cut point, which isolates the formulas from the 30 m sampling.
    fn profile(&self, dense: bool) -> PathProfile {
        let mut profile = PathProfile::new();
        profile.dist_m = self.distance_m;
        if dense {
            let steps = self.distance_m.ceil() as usize;
            profile.t = (0..=steps).map(|i| i as f64 / steps as f64).collect();
            profile.t.extend(self.cut_points.iter().map(|p| (p.s_m / self.distance_m).clamp(0.0, 1.0)));
            profile.t.sort_by(f64::total_cmp);
            profile.t.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
        } else {
            fill_t_values(self.distance_m, &mut profile.t);
        }
        for &t in &profile.t {
            let s = t * self.distance_m;
            profile.elevation_m.push(self.ground_z(s) as f32);
            profile.imd_u8.push(((1.0 - self.ground_g(s)) * 100.0).round() as u8);
            profile.forest_u8.push(0);
        }
        profile.step_m_med = median_step_m(&profile.t, self.distance_m);
        profile
    }

    fn candidates(&self) -> Vec<CrossingCandidate> {
        self.walls
            .iter()
            .enumerate()
            .map(|(id, &(s, top, ground, barrier))| CrossingCandidate {
                t: s / self.distance_m,
                height_m: (top - ground) as f32,
                kind: if barrier { ObstacleKind::Barrier } else { ObstacleKind::Building },
                id: id as u32,
            })
            .collect()
    }
}

fn mix(lh: &Bands, lf: &Bands) -> Bands {
    std::array::from_fn(|b| 10.0 * (0.5 * 10f64.powf(lh[b] / 10.0) + 0.5 * 10f64.powf(lf[b] / 10.0)).log10())
}

fn a_weighted(levels: &Bands) -> f64 {
    10.0 * levels.iter().zip(A_WEIGHTING).map(|(l, a)| 10f64.powf((l + a) / 10.0)).sum::<f64>().log10()
}

fn round2(bands: &Bands) -> Vec<f64> {
    bands.iter().map(|v| (v * 100.0).round() / 100.0).collect()
}

fn deviation(ours: &Bands, reference: &Bands) -> Vec<f64> {
    round2(&std::array::from_fn(|b| ours[b] - reference[b]))
}

/// How the case's ray is sampled before the kernel sees it.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Cadence {
    /// `fill_t_values`, exactly as production samples a raster ray.
    Production,
    /// Production samples, mean plane refitted with interval weights (diagnostic).
    ProductionIntervalWeightedPlane,
    /// Every metre plus every cut point: isolates the formulas from the sampling.
    Dense,
}

/// Today's kernel on one case: L_H, L_F, the production mixed-first level, the composite
/// ground-or-barrier term per state, and the replica-vs-production check of the ray terms
/// (meaningful for `Method::TODAY` only: the switches change the terms on purpose).
struct Evaluation {
    lh: Bands,
    lf: Bands,
    mixed_first: Bands,
    boundary_h: Bands,
    boundary_f: Bands,
    ground_path: GroundPath,
    samples: usize,
    replica_vs_production_db: f64,
}

fn evaluate(case: &Case, cadence: Cadence, method: Method, alpha: &Bands) -> Evaluation {
    let mut profile = case.profile(cadence == Cadence::Dense);
    let candidates = case.candidates();
    let mut ground_path = cnossos_ground_path_from_profile(&mut profile, case.source_z_m, case.receiver_z_m, false);
    if cadence == Cadence::ProductionIntervalWeightedPlane {
        ground_path = interval_weighted_plane(&profile, case.source_z_m, case.receiver_z_m, ground_path);
    }
    let terms: RayTerms = ray_terms(&profile, &candidates, case.source_z_m, case.receiver_z_m, ground_path, method);
    let replica_vs_production_db =
        validate_against_production(&mut profile, &candidates, case.source_z_m, case.receiver_z_m, ground_path, &terms);
    let slant = case.distance_m.hypot(case.receiver_z_m - case.source_z_m);
    let divergence = point_source_divergence_db(slant);
    let level = |composite: &Bands| -> Bands {
        std::array::from_fn(|b| case.sound_power_db - divergence - alpha[b] * slant / 1000.0 - composite[b])
    };
    let (boundary_h, boundary_f) = (terms.barrier_or_ground_in_state(false), terms.barrier_or_ground_in_state(true));
    Evaluation {
        lh: level(&boundary_h),
        lf: level(&boundary_f),
        mixed_first: level(&terms.barrier_or_ground_mixed_first()),
        boundary_h,
        boundary_f,
        ground_path,
        samples: profile.t.len(),
        replica_vs_production_db,
    }
}

const METHODS: [(&str, Method); 4] = [
    ("today", Method::TODAY),
    ("n7_favourable_floor", Method { favourable_ground_floor: true, diffraction_positive_slope: 20.0 }),
    ("n27_slope_40", Method { favourable_ground_floor: false, diffraction_positive_slope: 40.0 }),
    ("n7_n27", Method { favourable_ground_floor: true, diffraction_positive_slope: 40.0 }),
];

pub fn run(fixture: &Value) -> Value {
    let reference_alpha = bands(&fixture["reference_alpha_atm_db_per_km"]);
    let mut out = Vec::new();
    for raw in fixture["cases"].as_array().expect("cases") {
        let case = parse(raw);
        let reference_l = mix(&case.reference_lh, &case.reference_lf);
        let mut cadences = serde_json::Map::new();
        let mut validation: f64 = 0.0;
        for (cadence_name, cadence) in [
            ("production_cadence", Cadence::Production),
            ("production_cadence_interval_weighted_plane", Cadence::ProductionIntervalWeightedPlane),
            ("dense_cadence", Cadence::Dense),
        ] {
            let mut per_method = serde_json::Map::new();
            let mut geometry = Value::Null;
            for (method_name, method) in METHODS {
                let mut entry = serde_json::Map::new();
                for (alpha_name, alpha) in [("engine_alpha", &ALPHA_ATM), ("reference_alpha", &reference_alpha)] {
                    let e = evaluate(&case, cadence, method, alpha);
                    if method == Method::TODAY {
                        validation = validation.max(e.replica_vs_production_db);
                    }
                    geometry = json!({"samples": e.samples, "dp_m": e.ground_path.dp_m, "zs_m": e.ground_path.zs_h_m,
                                      "zr_m": e.ground_path.zr_h_m, "g_path": e.ground_path.ground_path_g,
                                      "g_source": e.ground_path.source_ground_g});
                    entry.insert("A_boundary_H".into(), json!(round2(&e.boundary_h)));
                    entry.insert("A_boundary_F".into(), json!(round2(&e.boundary_f)));
                    entry.insert(
                        alpha_name.to_owned(),
                        json!({
                            "LH": round2(&e.lh), "LF": round2(&e.lf), "L_mixed_first": round2(&e.mixed_first),
                            "dLH": deviation(&e.lh, &case.reference_lh), "dLF": deviation(&e.lf, &case.reference_lf),
                            "dL": deviation(&e.mixed_first, &reference_l),
                            "dLA": ((a_weighted(&e.mixed_first) - a_weighted(&reference_l)) * 100.0).round() / 100.0,
                        }),
                    );
                }
                per_method.insert(method_name.to_owned(), Value::Object(entry));
            }
            cadences.insert(cadence_name.to_owned(), json!({"geometry": geometry, "methods": per_method}));
        }
        out.push(json!({
            "case": case.name,
            "reference": {"LH": case.reference_lh, "LF": case.reference_lf, "L": round2(&reference_l),
                          "other_paths_not_modelled": case.other_paths},
            "crossings": case.walls.len(),
            "cadences": cadences,
            "replica_vs_production_terms_max_db": validation,
            "production_mixed_levels_engine_alpha": round2(&production_mixed_levels(&case)),
        }));
    }
    json!({"tolerance_db": 0.1, "cases": out})
}

/// Diagnostic only: the direct-path mean plane fitted with each sample weighted by the path
/// length it represents, instead of the production unweighted OLS over the bilateral cadence.
fn interval_weighted_plane(profile: &PathProfile, src_alt: f64, rcv_alt: f64, production: GroundPath) -> GroundPath {
    let (t, n, dist) = (&profile.t, profile.t.len(), profile.dist_m);
    let (mut sw, mut sx, mut sz, mut sxx, mut sxz) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for i in 0..n {
        let w = 0.5 * (t[(i + 1).min(n - 1)] - t[i.saturating_sub(1)]) * dist;
        let (x, z) = (t[i] * dist, profile.elevation_m[i] as f64);
        sw += w;
        sx += w * x;
        sz += w * z;
        sxx += w * x * x;
        sxz += w * x * z;
    }
    let slope = (sw * sxz - sx * sz) / (sw * sxx - sx * sx);
    let intercept = (sz - slope * sx) / sw;
    GroundPath::new(dist, src_alt - intercept, rcv_alt - (slope * dist + intercept), production.ground_path_g, production.source_ground_g)
}

/// The production popup kernel end to end on the production-cadence ray (engine alpha), the
/// cross-check of the replica's mixed-first level.
fn production_mixed_levels(case: &Case) -> Bands {
    let mut profile = case.profile(false);
    let candidates = case.candidates();
    let ground_path = cnossos_ground_path_from_profile(&mut profile, case.source_z_m, case.receiver_z_m, false);
    let terrain = path_effects::terrain_attenuation(&mut profile, case.source_z_m, case.receiver_z_m);
    let screening = path_effects::screening_attenuation(
        &mut profile,
        ObstacleInput { candidates: &candidates },
        case.source_z_m,
        case.receiver_z_m,
        0.0,
        &terrain,
    );
    let slant = case.distance_m.hypot(case.receiver_z_m - case.source_z_m);
    let v = propagate_variants_cnossos_ground_full(
        &[case.sound_power_db; NUM_BANDS],
        slant,
        SourceGeometry::Point,
        ground_path,
        &terrain,
        &screening,
        &[0.0; NUM_BANDS],
        0.0,
        0.0,
    );
    std::array::from_fn(|b| 10.0 * v.band_energy[b].log10() - A_WEIGHTING[b])
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str =
        concat!(env!("CARGO_MANIFEST_DIR"), "/../noise-compute/tests/fixtures/cnossos_iso_tr_17534_4_direct.json");

    fn cases() -> (Vec<Case>, Bands) {
        let fixture: Value = serde_json::from_str(&std::fs::read_to_string(FIXTURE).expect("fixture")).expect("json");
        let alpha = bands(&fixture["reference_alpha_atm_db_per_km"]);
        (fixture["cases"].as_array().expect("cases").iter().map(parse).collect(), alpha)
    }

    fn max_abs(ours: &Bands, reference: &Bands) -> f64 {
        (0..NUM_BANDS).map(|b| (ours[b] - reference[b]).abs()).fold(0.0, f64::max)
    }

    /// The oracle measures today's kernel only if its per-state replica is today's kernel.
    #[test]
    fn replica_reproduces_the_production_kernel_on_every_case() {
        let (cases, _) = cases();
        for case in &cases {
            let e = evaluate(case, Cadence::Production, Method::TODAY, &ALPHA_ATM);
            assert!(e.replica_vs_production_db < 1e-9, "{}: {}", case.name, e.replica_vs_production_db);
            let gap = max_abs(&e.mixed_first, &production_mixed_levels(case));
            assert!(gap < 1e-3, "{}: mixed level {gap} dB from production", case.name);
        }
    }

    /// Ground-only cases, formulas isolated from sampling: L_H meets ISO/TR 17534-4 to 0.1 dB
    /// today, and L_F does once the (2.5.20) favourable floor is applied (#7).
    #[test]
    fn ground_only_cases_pass_homogeneous_today_and_favourable_with_the_2_5_20_floor() {
        let (cases, alpha) = cases();
        let floor = Method { favourable_ground_floor: true, diffraction_positive_slope: 20.0 };
        for name in ["TC01", "TC02", "TC03", "TC04", "TC05", "TC16", "TC18", "TC20", "TC26"] {
            let case = cases.iter().find(|c| c.name == name).expect("case");
            let today = evaluate(case, Cadence::Dense, Method::TODAY, &alpha);
            assert!(max_abs(&today.lh, &case.reference_lh) <= 0.1, "{name} L_H");
            let with_floor = evaluate(case, Cadence::Dense, floor, &alpha);
            assert!(max_abs(&with_floor.lf, &case.reference_lf) <= 0.1, "{name} L_F with (2.5.20)");
        }
        let tc01 = cases.iter().find(|c| c.name == "TC01").expect("TC01");
        let today = evaluate(tc01, Cadence::Dense, Method::TODAY, &alpha);
        assert!((max_abs(&today.lf, &tc01.reference_lf) - 1.37).abs() < 0.01, "TC01 L_F gap without (2.5.20)");
    }
}
