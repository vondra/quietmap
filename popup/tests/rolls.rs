//! The runways' rolls at receivers over flat open ground: where a take-off is loudest beside its
//! runway, how the start of roll's directivity spreads its sound behind it, how far the rolls are
//! heard, and how each Doc 29 segment enters the passes' classes for the seconds it is rolled.

use physics::bands::{PERIOD_HOURS, PERIODS, lden_energy};
use physics::doc29::atmosphere::PlaceAtmosphere;
use physics::doc29::npd::Family;
use physics::doc29::profiles_generated::{CLASS_FAMILY, CLASS_NAMES, NUM_CLASSES};
use physics::doc29::roll_profile::{LANDING_EXIT_SPEED_MS, RollStep};
use physics::doc29::screening::Unscreened;
use popup::aircraft::boxes::AircraftReceiver;
use popup::aircraft::rolls::{HeardPath, HeardRoll, ROLL_REACH_M, RollSums, prepare, roll_sums};
use popup::loudness::{Curve, FlightSound, curves, loudness};
use popup::percentiles::{Line, Resolution, layered_distributions};
use tiles::aircraft_rolls::Roll;
use tiles::geo::{LocalFrame, TileId};

const TILE: TileId = TileId { x: 2212, y: 1387 };

fn class(name: &str) -> u8 {
    CLASS_NAMES
        .iter()
        .position(|&n| n == name)
        .expect("a class") as u8
}

/// `per_day` take-offs (or landings) of `class` eastwards from the tile's centre, by day only, on
/// a runway at sea level.
fn roll(class: u8, landing: bool, per_day: f64) -> Roll {
    Roll {
        start: [0, 0],
        heading_rad: 0.0,
        length_m: if landing { 1_400.0 } else { 1_800.0 },
        speed_ms: if landing { 66.9 } else { 77.2 },
        field_m: 0.0,
        class,
        landing,
        movements_per_day: [per_day, 0.0, 0.0],
    }
}

/// What `rolls` leave at `receiver` (metres east and north of their start, 4 m up).
fn heard(rolls: &[Roll], receiver: [f64; 2]) -> RollSums {
    let rolls = prepare(TILE, rolls, &PlaceAtmosphere::model());
    heard_paths(&rolls, receiver)
}

fn heard_paths(paths: &[HeardPath], receiver: [f64; 2]) -> RollSums {
    let frame = LocalFrame::at(TILE.centre());
    let receiver = AircraftReceiver {
        position: receiver,
        altitude_m: 4.0,
    };
    roll_sums(paths, &frame, receiver, &Unscreened)
}

/// The Lden (dB) of [`roll`] at `receiver`.
fn lden_db(class: u8, landing: bool, per_day: f64, receiver: [f64; 2]) -> f64 {
    let sums = heard(&[roll(class, landing, per_day)], receiver);
    10.0 * lden_energy(&sums.energy).log10()
}

/// The rolls' Nden (sone) alone over silence at `receiver`.
fn roll_nden(sums: &RollSums) -> f64 {
    let lines = Line::flight_passes(&sums.passes);
    let (_, whole) = layered_distributions(&[(&lines, [0.0; PERIODS])], Resolution::CLICK);
    let flight_sound = FlightSound {
        energy: sums.energy,
        spectrum_db: Some([0.0; 24]),
    };
    let place_curves: [Curve; PERIODS] = curves(&[], &flight_sound);
    loudness(&whole, &place_curves).nden_sone
}

/// A hundred A320 take-offs a day, 300 m beside the runway: loudest beside the start of roll
/// (63 dB Lden), where the aircraft is slowest at take-off power, falling along the roll (60 dB
/// beside its middle, 55 dB at lift-off); behind the start the start-of-roll directivity sends
/// their sound back, 12 dB less 300 m straight behind than beside (Eq. 4-24a: -13.5 dB at 180
/// deg).
#[test]
fn a_take_off_is_loudest_beside_its_start_of_roll() {
    let a320 = class("A320-232");
    let beside: Vec<f64> = [0.0, 450.0, 900.0, 1_350.0]
        .iter()
        .map(|&east| lden_db(a320, false, 100.0, [east, 300.0]))
        .collect();
    assert!(
        beside.windows(2).all(|pair| pair[0] > pair[1]),
        "{beside:?}"
    );
    let behind = lden_db(a320, false, 100.0, [-300.0, 0.0]);
    let level_beside = lden_db(a320, false, 100.0, [0.0, 300.0]);
    assert!(
        (level_beside - behind - 13.0).abs() < 2.0,
        "{level_beside} beside, {behind} behind"
    );
}

/// Beyond the reach nothing is heard; at it a hundred take-offs a day of the loudest class, the
/// 747-400, leave 31 dB Lden behind their start of roll and 33 dB beside it (2026-10-26), where
/// their climb is heard well above.
#[test]
fn the_rolls_are_heard_to_eight_kilometres() {
    let loudest = (0..NUM_CLASSES as u8)
        .filter(|&class| CLASS_FAMILY[usize::from(class)] != Family::Helicopter)
        .map(|class| {
            let reach = ROLL_REACH_M - 1.0;
            lden_db(class, false, 100.0, [-reach, 0.0]).max(lden_db(
                class,
                false,
                100.0,
                [0.0, reach],
            ))
        })
        .fold(f64::NEG_INFINITY, f64::max);
    assert!((loudest - 32.8).abs() < 0.5, "{loudest}");
    let a320 = class("A320-232");
    assert_eq!(
        lden_db(a320, false, 100.0, [-ROLL_REACH_M - 1.0, 0.0]),
        f64::NEG_INFINITY
    );
}

/// Each segment enters its own level class, so splitting one roll's movements, combining two
/// runway directions, or mixing an airliner with a light aircraft on the same runway are all
/// strictly additive across pass classes (never merging a quiet aircraft into a loud jet's stream).
#[test]
fn rolls_add_passes_independently_by_segment_level_class() {
    let a320 = roll(class("A320-232"), false, 100.0);
    let cessna = Roll {
        length_m: 300.0,
        speed_ms: 28.3,
        ..roll(class("CNA172"), false, 100.0)
    };
    let assert_passes_eq = |left: &RollSums, right: &RollSums| {
        for class in 0..popup::aircraft::PASS_CLASSES {
            for (a, b) in [
                (left.passes.energy[0][class], right.passes.energy[0][class]),
                (
                    left.passes.passing[0][class],
                    right.passes.passing[0][class],
                ),
            ] {
                assert!(
                    (a - b).abs() <= 1e-12 * b.max(1.0),
                    "class {class}: {a} vs {b}"
                );
            }
        }
    };
    let at = [100.0, 300.0];
    let whole = heard(std::slice::from_ref(&a320), at);
    let halves = heard(
        &[
            roll(class("A320-232"), false, 50.0),
            roll(class("A320-232"), false, 50.0),
        ],
        at,
    );
    assert_passes_eq(&halves, &whole);

    let jet = heard(std::slice::from_ref(&a320), at);
    let piston = heard(std::slice::from_ref(&cessna), at);
    let together = heard(&[a320, cessna], at);
    let mut apart = jet;
    apart.passes.add(&piston.passes);
    assert_passes_eq(&together, &apart);
    let jet_max_class = (0..popup::aircraft::PASS_CLASSES)
        .rfind(|&c| jet.passes.passing[0][c] > 0.0)
        .unwrap();
    let piston_max_class = (0..popup::aircraft::PASS_CLASSES)
        .rfind(|&c| piston.passes.passing[0][c] > 0.0)
        .unwrap();
    assert!(
        piston_max_class < jet_max_class,
        "light aircraft ({piston_max_class}) stays quieter than airliner ({jet_max_class})"
    );
}

/// Every roll's total passing share equals its movements per second times the roll's exact
/// kinematic duration (independent of receiver distance, with no pi factor or 30.5 m clamp), and
/// cutting the roll ten times finer leaves Nden within 8 % across off-runway take-off and landing
/// points.
#[test]
fn roll_passes_match_roll_duration_and_stay_stable_when_cut_ten_times_finer() {
    let place = PlaceAtmosphere::model();
    for (landing, points) in [
        (false, vec![[0.0, 100.0], [900.0, 300.0], [-300.0, 0.0]]),
        (true, vec![[100.0, 100.0], [700.0, 300.0]]),
    ] {
        let spec = roll(class("A320-232"), landing, 100.0);
        let coarse = prepare(TILE, std::slice::from_ref(&spec), &place);
        let fine: Vec<HeardPath> = coarse
            .iter()
            .map(|path| HeardPath {
                rolls: path
                    .rolls
                    .iter()
                    .map(|r| HeardRoll {
                        steps: r
                            .profile
                            .steps_by(1.0)
                            .into_iter()
                            .map(|step| RollStep {
                                emission: step.emission.in_atmosphere(&place),
                                ..step
                            })
                            .collect(),
                        ..r.clone()
                    })
                    .collect(),
                ..path.clone()
            })
            .collect();
        let end_speed = if landing {
            LANDING_EXIT_SPEED_MS.min(spec.speed_ms)
        } else {
            spec.speed_ms
        };
        let start_speed = if landing { spec.speed_ms } else { 0.0 };
        let expected_passing =
            100.0 / (12.0 * 3_600.0) * (2.0 * spec.length_m / (start_speed + end_speed));

        for point in points {
            let c = heard_paths(&coarse, point);
            let f = heard_paths(&fine, point);
            let passing_c: f64 = c.passes.passing[0].iter().sum();
            let passing_f: f64 = f.passes.passing[0].iter().sum();
            assert!(
                (passing_c / expected_passing - 1.0).abs() < 1e-9,
                "landing={landing} at {point:?}: {passing_c} vs {expected_passing}"
            );
            assert!(
                (passing_f / expected_passing - 1.0).abs() < 1e-9,
                "landing={landing} at {point:?}: {passing_f} vs {expected_passing}"
            );
            let lden_c = 10.0 * lden_energy(&c.energy).log10();
            let lden_f = 10.0 * lden_energy(&f.energy).log10();
            assert!(
                (lden_c - lden_f).abs() < 0.6,
                "landing={landing} at {point:?}: Lden {lden_c:.3} vs {lden_f:.3}"
            );
            let nden_c = roll_nden(&c);
            let nden_f = roll_nden(&f);
            let ratio = nden_c / nden_f;
            assert!(
                (ratio - 1.0).abs() < 0.08,
                "landing={landing} at {point:?}: Nden {nden_c:.4} vs {nden_f:.4} (ratio {ratio:.3})"
            );
        }
        // On the runway axis too, total passing stays the exact roll duration (no 30.5 m clamp).
        let on_runway = heard_paths(&coarse, [900.0, 0.0]);
        let passing_axis: f64 = on_runway.passes.passing[0].iter().sum();
        assert!(
            (passing_axis / expected_passing - 1.0).abs() < 1e-9,
            "landing={landing} on runway: {passing_axis} vs {expected_passing}"
        );
    }
}

/// Pass-class energy must equal roll energy in every period (day, evening and night), flight-kind
/// totals must equal overall Lden energy, and total passing duration must match kinematics — for a
/// take-off, a decelerating landing, and a steady-speed landing at or below 15 m/s.
#[test]
fn roll_pass_classes_and_kinds_conserve_energy_in_every_period() {
    let movements = [60.0, 20.0, 10.0];
    let cases = [
        (
            "takeoff",
            Roll {
                movements_per_day: movements,
                ..roll(class("A320-232"), false, 0.0)
            },
        ),
        (
            "landing",
            Roll {
                movements_per_day: movements,
                ..roll(class("A320-232"), true, 0.0)
            },
        ),
        (
            "steady_landing_15",
            Roll {
                length_m: 300.0,
                speed_ms: LANDING_EXIT_SPEED_MS,
                movements_per_day: movements,
                ..roll(class("CNA172"), true, 0.0)
            },
        ),
        (
            "steady_landing_12",
            Roll {
                length_m: 300.0,
                speed_ms: 12.0,
                movements_per_day: movements,
                ..roll(class("CNA172"), true, 0.0)
            },
        ),
    ];
    for (name, spec) in cases {
        let sums = heard(std::slice::from_ref(&spec), [150.0, 200.0]);
        let start_speed = if spec.landing { spec.speed_ms } else { 0.0 };
        let end_speed = if spec.landing {
            LANDING_EXIT_SPEED_MS.min(spec.speed_ms)
        } else {
            spec.speed_ms
        };
        let duration_s = 2.0 * spec.length_m / (start_speed + end_speed);
        for period in 0..PERIODS {
            assert!(sums.energy[period] > 0.0, "{name} period {period} silent");
            let pass_energy: f64 = sums.passes.energy[period].iter().sum();
            assert!(
                (pass_energy / sums.energy[period] - 1.0).abs() < 1e-12,
                "{name} period {period}: pass energy {pass_energy} vs roll {}",
                sums.energy[period]
            );
            let expected_passing =
                movements[period] / (PERIOD_HOURS[period] * 3_600.0) * duration_s;
            let pass_time: f64 = sums.passes.passing[period].iter().sum();
            assert!(
                (pass_time / expected_passing - 1.0).abs() < 1e-9,
                "{name} period {period}: passing {pass_time} vs {expected_passing}"
            );
        }
        let kinds_total: f64 = sums.kinds.iter().sum();
        let roll_lden = lden_energy(&sums.energy);
        assert!(
            (kinds_total / roll_lden - 1.0).abs() < 1e-12,
            "{name}: kinds {kinds_total} vs Lden {roll_lden}"
        );
    }
}
