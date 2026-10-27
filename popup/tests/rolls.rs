//! The runways' rolls at receivers over flat open ground: where a take-off is loudest beside its
//! runway, how the start of roll's directivity spreads its sound behind it, how far the rolls are
//! heard, and how often and how long they pass a receiver (Kurze's lambda, the loudness's).

use physics::bands::lden_energy;
use physics::doc29::atmosphere::PlaceAtmosphere;
use physics::doc29::npd::Family;
use physics::doc29::profiles_generated::{CLASS_FAMILY, CLASS_NAMES, NUM_CLASSES};
use physics::doc29::screening::Unscreened;
use popup::aircraft::boxes::AircraftReceiver;
use popup::aircraft::rolls::{ROLL_REACH_M, RollSums, prepare, roll_sums};
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
    let frame = LocalFrame::at(TILE.centre());
    let receiver = AircraftReceiver {
        position: receiver,
        altitude_m: 4.0,
    };
    roll_sums(&rolls, &frame, receiver, &Unscreened)
}

/// The Lden (dB) of [`roll`] at `receiver`.
fn lden_db(class: u8, landing: bool, per_day: f64, receiver: [f64; 2]) -> f64 {
    let sums = heard(&[roll(class, landing, per_day)], receiver);
    10.0 * lden_energy(&sums.energy).log10()
}

/// The rolls' Kurze lambda by day at `receiver`: how many pass it at a time.
fn lambda(rolls: &[Roll], receiver: [f64; 2]) -> f64 {
    let sums = heard(rolls, receiver);
    sums.energy_lambda[0] / sums.energy[0]
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

/// The classes rolling one path pass the receiver in one stream: a hundred A320 take-offs a day
/// heard as one roll or as two of fifty pass it as often (each roll's own rate had heard a
/// runway's forty classes as rare as one of them, and its loudness fell where its level rose).
#[test]
fn the_classes_rolling_one_path_pass_as_one_stream() {
    let a320 = class("A320-232");
    let whole = lambda(&[roll(a320, false, 100.0)], [900.0, 300.0]);
    let halves = lambda(
        &[roll(a320, false, 50.0), roll(a320, false, 50.0)],
        [900.0, 300.0],
    );
    assert!((halves / whole - 1.0).abs() < 1e-9, "{halves} vs {whole}");
}

/// A roll passes a receiver for as long as its own speeds take: on the runway as at the NPD
/// curves' nearest 30.5 m, where its sound exposure is read (the axis had made a pass last a
/// twentieth of a second), and beside its start the 11 s an A320 takes from rest over its first
/// 100 m, half of it in Kurze's lambda.
#[test]
fn a_roll_passes_for_as_long_as_its_speeds_take() {
    let a320 = class("A320-232");
    let rolls = [roll(a320, false, 100.0)];
    let on_runway = lambda(&rolls, [900.0, 0.0]);
    let beside = lambda(&rolls, [900.0, 20.0]);
    assert!(
        (on_runway / beside - 1.0).abs() < 1e-9,
        "{on_runway} vs {beside}"
    );
    let acceleration = 77.2f64.powi(2) / (2.0 * 1_800.0);
    let from_rest = (2.0 * 100.0 / acceleration).sqrt();
    let expected = 100.0 / (12.0 * 3_600.0) * 0.5 * from_rest;
    let at_start = lambda(&rolls, [0.0, 100.0]);
    assert!(
        (at_start / expected - 1.0).abs() < 0.01,
        "{at_start} vs {expected}"
    );
}
