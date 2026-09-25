//! BOUND.md invariant 1: no path gains more over free field from ground and diffraction than the
//! relevance bound's G_max, in either state; the searched extremes are pinned.

use super::*;
use crate::propagation::relevance_bound::SURFACE_RELEVANCE_GAIN_DB;

/// Homogeneous maximum of the W2 search (3.71 dB) rounded up, BOUND.md.
const HOMOGENEOUS_GAIN_BOUND_DB: f64 = 3.8;
/// The method's maximum over relief, above the bound (w2-physics search 2026-09-24): a grazing hard
/// crest takes the favourable floor of (2.5.20) on both sub-paths, 2·9 − 10·lg 3 = 13.2 dB at most
/// (13.09 dB found: 11.8 km, crest 14.57 m at 236 m, source 0.05 m, receiver 1.5 m, G = 0; the
/// homogeneous state 3.86 dB). Whether G_max follows it is an open owner decision (reach cost).
const RELIEF_GAIN_MAXIMUM_DB: f64 = 13.3;

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
    // Favourable edge 100 m before a 4 m receiver on a 10 km hard path (BOUND.md: 9.53 dB).
    let flat = Flat::new(10_000.0, 0.0);
    let favourable = gain_db(&flat.path(0.05, 4.0, &[(9_900.0, 8.96)]), MeteorologicalState::Favourable);
    assert!((9.4..=SURFACE_RELEVANCE_GAIN_DB).contains(&favourable), "{favourable}");
    // Homogeneous edge 1.80 m at 19 km of 20 km (3.71 dB).
    let flat = Flat::new(20_000.0, 0.0);
    let homogeneous = gain_db(&flat.path(0.05, 4.0, &[(19_000.0, 1.80)]), MeteorologicalState::Homogeneous);
    assert!((3.5..=HOMOGENEOUS_GAIN_BOUND_DB).contains(&homogeneous), "{homogeneous}");
    // Direct favourable over 29 km of hard ground (8.99 dB).
    let flat = Flat::new(29_000.0, 0.0);
    let direct = gain_db(&flat.path(0.05, 4.0, &[]), MeteorologicalState::Favourable);
    assert!((8.8..=SURFACE_RELEVANCE_GAIN_DB).contains(&direct), "{direct}");
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

/// Flat and relief profiles, G ∈ [0, 1] with 40 % exact 0, Gs 0, 1 or random, 0–3 obstacle tops,
/// source 0.05–4 m, receiver 1.5–30 m, 5 m–12 km: on flat ground every band of both states stays
/// under the bound; over relief under the method's own maximum.
#[test]
fn no_sampled_path_gains_more_than_the_bound() {
    let mut samples = Samples(0x9E37_79B9_7F4A_7C15);
    let (mut worst_homogeneous, mut worst_favourable, mut worst_relief) =
        (f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for _ in 0..20_000 {
        let length = 10f64.powf(samples.between(0.7, 4.08));
        let vertices = 2 + (samples.unit() * 8.0) as usize;
        let relief = samples.unit() < 0.5;
        let mut distance: Vec<f64> = (0..vertices).map(|_| samples.between(0.0, length)).collect();
        distance[0] = 0.0;
        distance[vertices - 1] = length;
        distance.sort_by(f64::total_cmp);
        let altitude: Vec<f64> = distance
            .iter()
            .map(|_| if relief { samples.between(-0.02, 0.02) * length } else { 0.0 })
            .collect();
        let ground: Vec<f64> = distance.iter().map(|_| samples.ground()).collect();
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
            worst_relief = worst_relief.max(homogeneous.max(favourable));
        } else {
            worst_homogeneous = worst_homogeneous.max(homogeneous);
            worst_favourable = worst_favourable.max(favourable);
        }
    }
    assert!(worst_homogeneous <= HOMOGENEOUS_GAIN_BOUND_DB, "homogeneous {worst_homogeneous}");
    assert!(worst_favourable <= SURFACE_RELEVANCE_GAIN_DB, "favourable {worst_favourable}");
    assert!(worst_relief <= RELIEF_GAIN_MAXIMUM_DB, "relief {worst_relief}");
}

