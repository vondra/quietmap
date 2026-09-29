//! No path gains more over free field from ground and diffraction than the relevance
//! bound's state maxima (18 dB favourable, 6 dB homogeneous: the (9)(h) below-plane limit of
//! image Δdif ≥ 0 plus both sides at the ground floor); the searched extremes are pinned.

use super::*;
use crate::propagation::relevance_bound::{FAVOURABLE_GAIN_BOUND_DB, HOMOGENEOUS_GAIN_BOUND_DB};

/// Flat homogeneous maximum of the boundary-gain search (3.71 dB) rounded up.
const FLAT_HOMOGENEOUS_GAIN_BOUND_DB: f64 = 3.8;
/// Flat favourable maximum (9.53 dB) rounded up: the flat search stays tight while the bound
/// covers relief.
const FLAT_FAVOURABLE_GAIN_BOUND_DB: f64 = 9.6;

/// Largest −A_boundary over the bands of one state.
fn gain_db(path: &VerticalPath<'_>, state: MeteorologicalState) -> f64 {
    let boundary = state_boundary(path, state, &mut VerticalPathScratch::default());
    boundary.attenuation_db.iter().map(|a| -a).fold(f64::NEG_INFINITY, f64::max)
}

struct Flat {
    distance: Vec<f64>,
    altitude: Vec<f64>,
    ground: Vec<f64>,
}

impl Flat {
    fn new(length: f64, ground: f64) -> Self {
        Self { distance: vec![0.0, length], altitude: vec![0.0, 0.0], ground: vec![ground, ground] }
    }
    fn path<'a>(&'a self, source_z: f64, receiver_z: f64, tops: &'a [PlanePoint]) -> VerticalPath<'a> {
        VerticalPath {
            profile: VerticalProfile { distance_m: &self.distance, altitude_m: &self.altitude, ground_factor: &self.ground },
            source: (0.0, source_z),
            receiver: (self.distance[1], receiver_z),
            source_ground_factor: self.ground[0],
            terrain_candidates: &[],
            obstacle_tops: tops,
        }
    }
}

#[test]
fn the_searched_extremes_stay_under_the_bound() {
    // Favourable edge 100 m before a 4 m receiver on a 10 km hard path (9.53 dB).
    let flat = Flat::new(10_000.0, 0.0);
    let favourable = gain_db(&flat.path(0.05, 4.0, &[(9_900.0, 8.96)]), MeteorologicalState::Favourable);
    assert!((9.4..=FLAT_FAVOURABLE_GAIN_BOUND_DB).contains(&favourable), "{favourable}");
    // Homogeneous edge 1.80 m at 19 km of 20 km (3.71 dB).
    let flat = Flat::new(20_000.0, 0.0);
    let homogeneous = gain_db(&flat.path(0.05, 4.0, &[(19_000.0, 1.80)]), MeteorologicalState::Homogeneous);
    assert!((3.5..=FLAT_HOMOGENEOUS_GAIN_BOUND_DB).contains(&homogeneous), "{homogeneous}");
    // Direct favourable over 29 km of hard ground (8.99 dB).
    let flat = Flat::new(29_000.0, 0.0);
    let direct = gain_db(&flat.path(0.05, 4.0, &[]), MeteorologicalState::Favourable);
    assert!((8.8..=FLAT_FAVOURABLE_GAIN_BOUND_DB).contains(&direct), "{direct}");
    // Relief crest 14.57 m at 236 m of 11.8 km, source 0.05 m, receiver 1.5 m, G = 0 (13.09 dB).
    let distance = vec![0.0, 236.0, 11_800.0];
    let altitude = vec![0.0, 14.57, 0.0];
    let ground = vec![0.0, 0.0, 0.0];
    let terrain = vec![(0.0, 0.0), (236.0, 14.57), (11_800.0, 0.0)];
    let path = VerticalPath {
        profile: VerticalProfile { distance_m: &distance, altitude_m: &altitude, ground_factor: &ground },
        source: (0.0, 0.05),
        receiver: (11_800.0, 1.5),
        source_ground_factor: 0.0,
        terrain_candidates: &terrain,
        obstacle_tops: &[],
    };
    let relief = gain_db(&path, MeteorologicalState::Favourable);
    assert!((12.9..=13.3).contains(&relief), "{relief}");
}

/// The (9)(h) below-plane corner that voids the pre-slice-2 13.3 dB derivation: a blocked
/// grazing path over hard ground with the receiver below its side plane takes the image
/// Δdif (≈ 0 dB) plus both sides at the ground floor. Seeds 920892/1744332 of the 2M-sample
/// relief search, rounded to 1 mm / 1 mm / 0.001 G.
#[test]
fn below_plane_corners_stay_under_the_state_bounds() {
    // Favourable 17.60 dB: 6.2 km, source 0.23 m up, receiver 7.24 m up in a dip.
    let distance = vec![0.0, 1325.533, 3827.411, 4293.855, 6218.238];
    let altitude = vec![-85.685, -7.088, 28.333, 12.712, -44.696];
    let ground = vec![0.0, 0.0, 0.0, 0.0, 0.146];
    let terrain: Vec<PlanePoint> =
        distance.iter().copied().zip(altitude.iter().copied()).collect();
    let path = VerticalPath {
        profile: VerticalProfile { distance_m: &distance, altitude_m: &altitude, ground_factor: &ground },
        source: (0.0, -85.685 + 0.228),
        receiver: (6218.238, -44.696 + 7.241),
        source_ground_factor: 0.0,
        terrain_candidates: &terrain,
        obstacle_tops: &[],
    };
    let favourable = gain_db(&path, MeteorologicalState::Favourable);
    assert!((17.4..=FAVOURABLE_GAIN_BOUND_DB).contains(&favourable), "{favourable}");
    // Homogeneous 6.00 dB: 7.6 km, source 1.81 m up, receiver 7.67 m up in a dip.
    let distance = vec![0.0, 2003.806, 3406.244, 3462.741, 4428.192, 4645.091, 4888.823, 5042.429, 7571.335];
    let altitude = vec![123.543, 105.373, -146.72, 13.508, -72.448, -52.391, 25.951, 72.715, 102.924];
    let ground = vec![0.0, 0.0, 0.004, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    let terrain: Vec<PlanePoint> =
        distance.iter().copied().zip(altitude.iter().copied()).collect();
    let tops = vec![(743.466, 124.829), (2706.498, 123.986), (3843.265, 14.608)];
    let path = VerticalPath {
        profile: VerticalProfile { distance_m: &distance, altitude_m: &altitude, ground_factor: &ground },
        source: (0.0, 123.543 + 1.814),
        receiver: (7571.335, 102.924 + 7.67),
        source_ground_factor: 0.0,
        terrain_candidates: &terrain,
        obstacle_tops: &tops,
    };
    let homogeneous = gain_db(&path, MeteorologicalState::Homogeneous);
    assert!((5.8..=HOMOGENEOUS_GAIN_BOUND_DB).contains(&homogeneous), "{homogeneous}");
}

/// xorshift64*: a fixed, dependency-free sample sequence.
struct Samples(u64);

impl Samples {
    fn unit(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }
    fn between(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }
    fn ground(&mut self) -> f64 {
        if self.unit() < 0.4 { 0.0 } else { self.unit() }
    }
}

/// Flat paths stay under the flat search maxima (3.8/9.6 dB); relief paths — rolling
/// ground plus single crests over hard ground, the (9)(h) below-plane shape — stay under
/// the state bounds per state (6/18 dB). G ∈ [0, 1] with 40 % exact 0 on flat paths,
/// 70 % on relief; Gs 0, 1 or random; 0–3 obstacle tops; source 0.05–4 m, receiver
/// 1.5–30 m; flat 5 m–12 km, relief 300 m–12 km.
#[test]
fn no_sampled_path_gains_more_than_the_bound() {
    let mut samples = Samples(0x9E37_79B9_7F4A_7C15);
    let (mut worst_homogeneous, mut worst_favourable) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
    let (mut worst_relief_homogeneous, mut worst_relief_favourable) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
    for sample in 0..30_000 {
        let relief = sample >= 10_000;
        let length = 10f64.powf(samples.between(if relief { 2.5 } else { 0.7 }, 4.08));
        let vertices = 2 + (samples.unit() * 8.0) as usize;
        let mut distance: Vec<f64> = (0..vertices).map(|_| samples.between(0.0, length)).collect();
        distance[0] = 0.0;
        distance[vertices - 1] = length;
        distance.sort_by(f64::total_cmp);
        let crest = relief && samples.unit() < 0.5;
        let (crest_x, crest_h) = (samples.between(0.05, 0.95) * length, samples.between(1.0, 30.0));
        let altitude: Vec<f64> = distance
            .iter()
            .map(|&x| {
                if crest {
                    crest_h * (1.0 - ((x - crest_x) / (0.5 * length)).abs()).max(0.0)
                        + samples.between(-0.005, 0.005) * length
                } else if relief {
                    samples.between(-0.02, 0.02) * length
                } else {
                    0.0
                }
            })
            .collect();
        let ground: Vec<f64> = distance
            .iter()
            .map(|_| {
                if relief {
                    if samples.unit() < 0.7 {
                        0.0
                    } else {
                        samples.unit() * 0.3
                    }
                } else {
                    samples.ground()
                }
            })
            .collect();
        let source_ground_factor = match (samples.unit() * 3.0) as usize {
            0 => 0.0,
            1 => 1.0,
            _ => samples.unit(),
        };
        let terrain: Vec<PlanePoint> = distance.iter().copied().zip(altitude.iter().copied()).collect();
        let mut tops: Vec<PlanePoint> = (0..(samples.unit() * 4.0) as usize)
            .map(|_| {
                let x = samples.between(0.0, length);
                let base = altitude[distance.partition_point(|&d| d <= x).clamp(1, vertices - 1) - 1];
                (x, base + samples.between(0.5, 25.0))
            })
            .collect();
        tops.sort_by(|a, b| a.0.total_cmp(&b.0));
        let path = VerticalPath {
            profile: VerticalProfile { distance_m: &distance, altitude_m: &altitude, ground_factor: &ground },
            source: (0.0, altitude[0] + samples.between(0.05, 4.0)),
            receiver: (length, altitude[vertices - 1] + samples.between(1.5, 30.0)),
            source_ground_factor,
            terrain_candidates: &terrain,
            obstacle_tops: &tops,
        };
        let (homogeneous, favourable) =
            (gain_db(&path, MeteorologicalState::Homogeneous), gain_db(&path, MeteorologicalState::Favourable));
        if relief {
            worst_relief_homogeneous = worst_relief_homogeneous.max(homogeneous);
            worst_relief_favourable = worst_relief_favourable.max(favourable);
        } else {
            worst_homogeneous = worst_homogeneous.max(homogeneous);
            worst_favourable = worst_favourable.max(favourable);
        }
    }
    assert!(worst_homogeneous <= FLAT_HOMOGENEOUS_GAIN_BOUND_DB, "homogeneous {worst_homogeneous}");
    assert!(worst_favourable <= FLAT_FAVOURABLE_GAIN_BOUND_DB, "favourable {worst_favourable}");
    assert!(
        worst_relief_homogeneous <= HOMOGENEOUS_GAIN_BOUND_DB,
        "relief homogeneous {worst_relief_homogeneous}"
    );
    assert!(
        worst_relief_favourable <= FAVOURABLE_GAIN_BOUND_DB,
        "relief favourable {worst_relief_favourable}"
    );
}

