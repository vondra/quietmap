//! The box ceiling against the click-time equation: never below a box's energy at any receiver of
//! its region, and near it at a receiver abeam a short box.

use super::*;
use crate::doc29::box_sums::{BoxSums, BoxValues, SegmentEnergies};
use crate::doc29::boxes::box_sel_at_receiver;
use crate::doc29::profiles_generated::profile_idx;
use crate::doc29::screening::Unscreened;
use crate::doc29::segment::{AircraftType, SegmentEmission};
use crate::doc29::thrust::SegmentFlight;

/// SplitMix64: the same boxes and receivers every run.
struct Random(u64);

impl Random {
    fn unit(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        (z ^ (z >> 31)) as f64 / u64::MAX as f64
    }

    fn between(&mut self, low: f64, high: f64) -> f64 {
        low + (high - low) * self.unit()
    }
}

fn emission(
    designator: &str,
    departure: bool,
    speed_kt: f64,
    climb_sine: f64,
    altitude_m: f64,
) -> SegmentEmission {
    let flight = SegmentFlight {
        departure,
        on_ground: false,
        speed_kt,
        pressure_altitude_m: altitude_m,
        climb_sine,
        acceleration_ms2: None,
        height_above_field_m: altitude_m,
    };
    SegmentEmission::new(
        &AircraftType::of(profile_idx(designator), designator),
        &flight,
        false,
    )
    .expect("inside the thrust domain")
}

/// A box of a few pieces of airliners, regional jets and propellers about `centre_m`, heading
/// near `heading` one way or both, each period weighted at random (the evening silent at times).
fn random_box(random: &mut Random, centre_m: [f64; 3], heading: f64) -> BoxValues {
    let kinds = [
        ("A320", true),
        ("B738", false),
        ("CRJ9", true),
        ("C172", true),
        ("B77W", false),
    ];
    let both_ways = random.unit() < 0.3;
    let evening = if random.unit() < 0.3 {
        0.0
    } else {
        random.between(0.1, 3.0)
    };
    let mut sums = BoxSums::default();
    for _ in 0..1 + (random.unit() * 6.0) as usize {
        let (designator, departure) =
            kinds[(random.unit() * kinds.len() as f64) as usize % kinds.len()];
        let climb = if departure {
            random.between(0.02, 0.12)
        } else {
            -0.052
        };
        let altitude = centre_m[2] + random.between(-60.0, 60.0);
        let emission = emission(
            designator,
            departure,
            random.between(130.0, 190.0),
            climb,
            altitude.max(50.0),
        );
        let direction = heading
            + random.between(-0.2, 0.2)
            + if both_ways && random.unit() < 0.5 {
                std::f64::consts::PI
            } else {
                0.0
            };
        let half = random.between(1.0, 900.0);
        let middle = [
            centre_m[0] + random.between(-75.0, 75.0),
            centre_m[1] + random.between(-75.0, 75.0),
            altitude,
        ];
        let step = [direction.cos() * half, direction.sin() * half, climb * half];
        let [start, end] =
            [-1.0, 1.0].map(|side| std::array::from_fn(|axis| middle[axis] + side * step[axis]));
        let weights = [random.between(0.1, 3.0), evening, random.between(0.0, 3.0)];
        sums.add(
            &SegmentEnergies::of(&emission.npd_distance_levels()),
            emission.installation,
            weights,
            start,
            end,
        );
    }
    sums.values().expect("a box with energy")
}

fn at_receiver<'a>(
    values: &'a BoxValues,
    lg_scaled: &'a [f64; NPD_DISTANCES],
    receiver_m: [f64; 3],
) -> AircraftBoxAtReceiver<'a> {
    AircraftBoxAtReceiver {
        centroid_m: std::array::from_fn(|axis| values.centroid_m[axis] - receiver_m[axis]),
        axis_rad: values.axis_rad,
        gradient: values.gradient,
        gradient_spread: values.gradient_spread,
        piece_length_m: values.piece_length_m,
        levels_db: &values.levels_db,
        tail_levels_db: &values.tail_levels_db,
        lg_scaled_distance: lg_scaled,
        installation_shares: values.installation_shares,
        ground_m: -4.0 - receiver_m[2],
    }
}

/// Boxes near and far, low and cruising, a third heading at the region (its receivers on their
/// extended lines), over regions from a point to a city square, read at their regions' corners
/// and inside: no receiver hears a box above its ceiling.
#[test]
fn no_receiver_of_a_region_hears_a_box_above_its_ceiling() {
    let mut random = Random(11);
    let mut checked = 0;
    for case in 0..30_000 {
        let distance = [300.0, 2_000.0, 8_000.0, 15_000.0][case % 4] * random.between(0.2, 1.0);
        let bearing = random.between(0.0, std::f64::consts::TAU);
        let height = [30.0, 300.0, 1_500.0, 9_000.0][(case / 4) % 4] * random.between(0.5, 1.5);
        let heading = match case % 3 {
            0 => bearing + random.between(-0.05, 0.05),
            _ => random.between(0.0, std::f64::consts::TAU),
        };
        let values = random_box(
            &mut random,
            [distance * bearing.cos(), distance * bearing.sin(), height],
            heading,
        );
        let lg_scaled = values.scaled_distance_m.map(f64::log10);
        let extent = [
            random.between(0.0, 2_500.0),
            random.between(0.0, 2_500.0),
            random.between(0.0, 80.0),
        ];
        let ceiling = box_sel_ceiling(&at_receiver(&values, &lg_scaled, [0.0; 3]), extent);
        for corner in 0..12 {
            let receiver: [f64; 3] = std::array::from_fn(|axis| {
                extent[axis]
                    * if corner < 8 {
                        [-1.0, 1.0][(corner >> axis) & 1]
                    } else {
                        random.between(-1.0, 1.0)
                    }
            });
            let exact =
                box_sel_at_receiver(&at_receiver(&values, &lg_scaled, receiver), &Unscreened)
                    .sel_energy;
            for period in 0..PERIODS {
                assert!(
                    exact[period] <= ceiling[period] * (1.0 + 1e-9),
                    "case {case} receiver {receiver:?} period {period}: {} dB over the ceiling {} dB",
                    10.0 * exact[period].log10(),
                    10.0 * ceiling[period].log10()
                );
                checked += usize::from(exact[period] > 0.0);
            }
        }
    }
    assert!(checked > 800_000, "{checked} energies checked");
}

/// At one receiver abeam a short box, the ceiling is its energy but for the percent the
/// installation's table keeps above it.
#[test]
fn the_ceiling_of_a_short_box_abeam_one_receiver_is_near_its_energy() {
    let pieces = [(
        emission("A320", true, 160.0, 0.06, 600.0),
        [-40.0, 3_000.0, 597.6],
        [40.0, 3_000.0, 602.4],
    )];
    let mut sums = BoxSums::default();
    for (emission, start, end) in pieces {
        sums.add(
            &SegmentEnergies::of(&emission.npd_distance_levels()),
            emission.installation,
            [1.0, 0.0, 0.0],
            start,
            end,
        );
    }
    let values = sums.values().expect("a box with energy");
    let lg_scaled = values.scaled_distance_m.map(f64::log10);
    let at = at_receiver(&values, &lg_scaled, [0.0; 3]);
    let exact = box_sel_at_receiver(&at, &Unscreened).sel_energy[0];
    let ceiling = box_sel_ceiling(&at, [0.0; 3])[0];
    let over_db = 10.0 * (ceiling / exact).log10();
    assert!((0.0..0.1).contains(&over_db), "{over_db:.3} dB over");
}
