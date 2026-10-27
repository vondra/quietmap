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
/// heard as one roll or as two of fifty pass it as often, and an A320's and a 737's as often as
/// both apart (each roll's own rate had heard a runway's forty classes as rare as one of them, and
/// its loudness fell where its level rose); two runway directions leaving one point at 57 deg left
/// and right of east, their cosines equal, stay two streams.
#[test]
fn the_classes_rolling_one_path_pass_as_one_stream() {
    let a320 = class("A320-232");
    let b738 = class("737800");
    let at = [900.0, 300.0];
    let whole = lambda(&[roll(a320, false, 100.0)], at);
    let halves = lambda(&[roll(a320, false, 50.0), roll(a320, false, 50.0)], at);
    assert!((halves / whole - 1.0).abs() < 1e-9, "{halves} vs {whole}");
    let together = lambda(&[roll(a320, false, 100.0), roll(b738, false, 60.0)], at);
    let apart = whole + lambda(&[roll(b738, false, 60.0)], at);
    assert!(
        (together / apart - 1.0).abs() < 1e-9,
        "{together} vs {apart}"
    );
    let turned = |heading_rad| Roll {
        heading_rad,
        ..roll(a320, false, 100.0)
    };
    let one = lambda(&[turned(1.0)], [500.0, 0.0]);
    let both = lambda(&[turned(1.0), turned(-1.0)], [500.0, 0.0]);
    assert!((both / one - 1.0).abs() < 1e-9, "{both} vs {one}");
}

/// A roll passes a receiver for as long as its own speeds take over the pass's distance around
/// the receiver's nearest point: on the runway as at the NPD curves' nearest 30.5 m, where its
/// sound exposure is read (the axis had made a pass last a twentieth of a second), and 100 m beside
/// its start, where the stretch lies on the roll's first 100 m, the 11 s an A320 takes over them
/// from rest (half of it had been taken there).
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
    let expected = 100.0 / (12.0 * 3_600.0) * from_rest;
    let at_start = lambda(&rolls, [0.0, 100.0]);
    assert!(
        (at_start / expected - 1.0).abs() < 0.01,
        "{at_start} vs {expected}"
    );
}

/// Beside the middle of an A320's roll a Cessna's 300 m roll on the same path, over 600 m back,
/// passes nearest at its own end: a stream of its own, joining the A320's at its energy's weight
/// (its passes counted into the A320's had tripled their lambda), as two paths do.
#[test]
fn a_short_roll_passes_in_its_own_stream_beside_a_longer_ones_middle() {
    let a320 = roll(class("A320-232"), false, 90.0);
    let cessna = Roll {
        length_m: 300.0,
        speed_ms: 28.3,
        ..roll(class("CNA172"), false, 10.0)
    };
    let at = [900.0, 20.0];
    let (jet, piston) = (
        heard(std::slice::from_ref(&a320), at),
        heard(std::slice::from_ref(&cessna), at),
    );
    let weighted =
        (jet.energy_lambda[0] + piston.energy_lambda[0]) / (jet.energy[0] + piston.energy[0]);
    let together = lambda(&[a320, cessna], at);
    assert!(
        (together / weighted - 1.0).abs() < 1e-9,
        "{together} vs {weighted}"
    );
}
