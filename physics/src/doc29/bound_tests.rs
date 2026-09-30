//! The aircraft bound stays above the exact kernel: a sweep over every class, operation and power
//! row across a grid of segment geometries, and boxes of many segments.

use super::*;
use crate::bands::{energy, level_db};
use crate::doc29::npd::class_anchor;
use crate::doc29::screening::Unscreened;
use crate::doc29::segment::{SegmentEmission, SegmentGeometry, segment_sel_at_receiver};
use crate::doc29::thrust::PowerBracket;
use crate::doc29::thrust_generated::THRUST;

/// Every class, operation and power bracket (rows and midpoints between them). Delta_V and the
/// helicopter correction shift the kernel and the levels alike, so they stay 0 here.
fn every_emission() -> Vec<SegmentEmission> {
    let mut emissions = Vec::new();
    for (class, model) in THRUST.iter().enumerate() {
        for departure in [false, true] {
            let rows = usize::from(if departure {
                model.dep_rows
            } else {
                model.app_rows
            });
            for row in 0..rows {
                for weight in [0.0, 0.5]
                    .into_iter()
                    .filter(|&w| w == 0.0 || row + 1 < rows)
                {
                    emissions.push(SegmentEmission {
                        class,
                        departure,
                        power: PowerBracket { row, weight },
                        installation: class_anchor(class).installation,
                        speed_correction_db: 0.0,
                        helicopter_correction_db: 0.0,
                    });
                }
            }
        }
    }
    emissions
}

/// A segment along +x, `lateral_m` north of the receiver, from `start_along_m` for
/// `length_m`, climbing at `gradient` from `height_m`; the ground far below every extension.
fn segment(
    start_along_m: f64,
    length_m: f64,
    lateral_m: f64,
    height_m: f64,
    gradient: f64,
) -> SegmentGeometry {
    SegmentGeometry {
        start_m: [start_along_m, lateral_m, height_m],
        end_m: [
            start_along_m + length_m,
            lateral_m,
            height_m + gradient * length_m,
        ],
        ground_under_start_m: -1e5,
        ground_under_end_m: -1e5,
    }
}

/// Distance from the receiver to the nearest point of the finite segment.
fn nearest_m(geometry: &SegmentGeometry) -> f64 {
    let (start, end) = (geometry.start_m, geometry.end_m);
    let delta = [0, 1, 2].map(|axis| end[axis] - start[axis]);
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let t = (-dot(start, delta) / dot(delta, delta)).clamp(0.0, 1.0);
    let point = [0, 1, 2].map(|axis| start[axis] + t * delta[axis]);
    dot(point, point).sqrt()
}

fn levels(emission: &SegmentEmission) -> [f64; NPD_DISTANCES] {
    emission.npd_distance_levels().sel_db
}

/// The bound over the kernel for every emission across a grid of segments: low and high, short
/// and long, level, climbing and descending, the receiver before, beside and beyond them, above
/// and below. Where the closest point lies off the segment its NPD level is read nearer than the
/// segment and Delta_F must pay for it; the kernel never even reaches the curve at the segment's
/// nearest distance plus the largest installation gain, so the margin is spare.
#[test]
fn the_bound_stays_above_every_segment() {
    let gains_db = INSTALLATION_CORRECTION_MAX_DB + FINITE_GEOMETRY_MARGIN_DB;
    let mut worst_over_curve_db = f64::NEG_INFINITY;
    for emission in every_emission() {
        let levels = levels(&emission);
        for height_m in [
            -200.0, -20.0, 5.0, 10.0, 30.48, 60.0, 150.0, 400.0, 1e3, 3e3, 7e3, 11e3,
        ] {
            for length_m in [
                1.5, 20.0, 100.0, 400.0, 1_500.0, 4_000.0, 12_000.0, 20_000.0,
            ] {
                let receiver_along_m = [
                    -3_000.0,
                    -200.0,
                    -20.0,
                    0.0,
                    0.3 * length_m,
                    0.5 * length_m,
                    length_m,
                    length_m + 20.0,
                    length_m + 60.0,
                    length_m + 200.0,
                    length_m + 3_000.0,
                ];
                for along_m in receiver_along_m {
                    for lateral_m in [0.0, 10.0, 100.0, 500.0, 2_000.0, 8_000.0] {
                        for gradient in [0.0, 0.05, 0.25, -0.05] {
                            let geometry =
                                segment(-along_m, length_m, lateral_m, height_m, gradient);
                            let exact =
                                segment_sel_at_receiver(&emission, &geometry, &Unscreened).sel_db;
                            let bound_db = aircraft_sel_bound_db(&levels, nearest_m(&geometry));
                            assert!(exact <= bound_db, "{emission:?} {geometry:?}");
                            worst_over_curve_db =
                                worst_over_curve_db.max(exact - (bound_db - gains_db));
                        }
                    }
                }
            }
        }
    }
    assert!(
        worst_over_curve_db < INSTALLATION_CORRECTION_MAX_DB,
        "{worst_over_curve_db}"
    );
}

/// A tiny deterministic generator (a linear congruential sequence) for the box test.
struct Sequence(u64);

impl Sequence {
    fn next_unit(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// A box of 40 segments of mixed classes: its summed levels bound the sum of the kernel at
/// receivers inside, beside, above, below and far from it.
#[test]
fn the_bound_of_a_box_stays_above_the_sum_of_its_segments() {
    let emissions = every_emission();
    let mut sequence = Sequence(29);
    for (low, high) in [
        ([-200.0, 300.0, 50.0], [200.0, 700.0, 250.0]),
        ([0.0, -50.0, 900.0], [800.0, 750.0, 1_700.0]),
    ] {
        let mut box_energy = [0.0; NPD_DISTANCES];
        let pieces: Vec<(SegmentEmission, SegmentGeometry)> = (0..40)
            .map(|_| {
                let emission = emissions[(sequence.next_unit() * emissions.len() as f64) as usize];
                let mut point = || {
                    [0, 1, 2]
                        .map(|axis| low[axis] + sequence.next_unit() * (high[axis] - low[axis]))
                };
                let geometry = SegmentGeometry {
                    start_m: point(),
                    end_m: point(),
                    ground_under_start_m: -1e5,
                    ground_under_end_m: -1e5,
                };
                for (sum, level) in box_energy.iter_mut().zip(levels(&emission)) {
                    *sum += energy(level);
                }
                (emission, geometry)
            })
            .collect();
        for receiver in [
            [0.0, 500.0, 100.0],
            [0.0, 0.0, 0.0],
            [300.0, 900.0, -20.0],
            [5e3, -4e3, 0.0],
        ] {
            let exact: f64 = pieces
                .iter()
                .map(|(emission, geometry)| {
                    let shifted =
                        |point: [f64; 3]| [0, 1, 2].map(|axis| point[axis] - receiver[axis]);
                    let relative = SegmentGeometry {
                        start_m: shifted(geometry.start_m),
                        end_m: shifted(geometry.end_m),
                        ..*geometry
                    };
                    energy(segment_sel_at_receiver(emission, &relative, &Unscreened).sel_db)
                })
                .sum();
            let gap = [0, 1, 2].map(|axis| {
                (low[axis] - receiver[axis])
                    .max(receiver[axis] - high[axis])
                    .max(0.0)
            });
            let nearest_m = (gap[0] * gap[0] + gap[1] * gap[1] + gap[2] * gap[2]).sqrt();
            let bound_db = aircraft_sel_bound_db(&box_energy.map(level_db), nearest_m);
            let exact_db = level_db(exact);
            assert!(
                exact_db <= bound_db,
                "box {low:?}-{high:?} receiver {receiver:?}: {exact_db} > {bound_db}"
            );
        }
    }
}

#[test]
fn the_bound_falls_with_distance_and_follows_the_levels() {
    let levels = levels(&every_emission()[0]);
    let mut previous = f64::INFINITY;
    for slant_m in [
        0.0, 20.0, 45.0, 61.0, 100.0, 1_000.0, 7_620.0, 9_000.0, 16_000.0,
    ] {
        let bound_db = aircraft_sel_bound_db(&levels, slant_m);
        assert!(bound_db <= previous, "{slant_m}");
        previous = bound_db;
    }
    let louder = levels.map(|level| level + 3.0);
    let rise = aircraft_sel_bound_db(&louder, 900.0) - aircraft_sel_bound_db(&levels, 900.0);
    assert!((rise - 3.0).abs() < 1e-12);
    let silent = [f64::NEG_INFINITY; NPD_DISTANCES];
    assert_eq!(aircraft_sel_bound_db(&silent, 900.0), f64::NEG_INFINITY);
    // dev4's early exit is the per-segment bound below the event floor.
    assert!(aircraft_sel_bound_db(&levels, 16_000.0) > EVENT_FLOOR_SEL_DB);
}
