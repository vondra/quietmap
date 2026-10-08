//! The click-time equation against the kernel: a box of one segment reads as the kernel reads the
//! segment, and a box of a bundle of pieces reads as the sum of their kernel levels at receivers
//! beyond the box.

use super::*;
use crate::doc29::box_sums::{BoxSums, BoxValues};
use crate::doc29::screening::Unscreened;
use crate::doc29::segment::{
    AircraftType, SegmentEmission, SegmentGeometry, segment_sel_at_receiver,
};
use crate::doc29::thrust::SegmentFlight;

fn flight(departure: bool, speed_kt: f64, climb_sine: f64, altitude_m: f64) -> SegmentFlight {
    SegmentFlight {
        departure,
        on_ground: false,
        speed_kt,
        pressure_altitude_m: altitude_m,
        climb_sine,
        acceleration_ms2: None,
        height_above_field_m: altitude_m,
    }
}

fn emission(designator: &str, flight: &SegmentFlight) -> SegmentEmission {
    SegmentEmission::new(&AircraftType::from_designator(designator), flight, false)
        .expect("inside the thrust domain")
}

fn kernel_sel(emission: &SegmentEmission, start_m: [f64; 3], end_m: [f64; 3]) -> Option<f64> {
    let geometry = SegmentGeometry {
        start_m,
        end_m,
        ground_under_start_m: -4.0,
        ground_under_end_m: -4.0,
    };
    Some(segment_sel_at_receiver(emission, &geometry, &Unscreened).sel_db)
}

/// The box of `pieces` (emission, start, end) in the receiver's frame, day period only.
fn box_of(pieces: &[(SegmentEmission, [f64; 3], [f64; 3])]) -> BoxValues {
    let mut sums = BoxSums::default();
    for (emission, start, end) in pieces {
        sums.add(
            &emission.npd_distance_levels(),
            emission.installation,
            [1.0, 0.0, 0.0],
            *start,
            *end,
        );
    }
    sums.values().expect("a box with energy")
}

fn box_day_sel(values: &BoxValues) -> Option<f64> {
    let aircraft_box = AircraftBoxAtReceiver {
        centroid_m: values.centroid_m,
        axis_rad: values.axis_rad,
        gradient: values.gradient,
        gradient_spread: values.gradient_spread,
        piece_length_m: values.piece_length_m,
        levels_db: &values.levels_db,
        tail_levels_db: &values.tail_levels_db,
        lg_scaled_distance: &values.scaled_distance_m.map(f64::log10),
        installation_shares: values.installation_shares,
        ground_m: -4.0,
    };
    Some(box_sel_at_receiver(&aircraft_box, &Unscreened).sel_db[0])
}

/// A box of one segment is that segment: the same curve, d_lambda, Delta_F, Lambda and Delta_I,
/// wherever the receiver stands (the segment and its box shifted to put the receiver there).
#[test]
fn a_box_of_one_segment_reads_as_the_kernel() {
    for (designator, departure) in [("A320", true), ("B738", false), ("C172", true)] {
        let emission = emission(designator, &flight(departure, 150.0, 0.05, 600.0));
        assert_eq!(emission.power.weight, 0.0, "{designator}: one power row");
        let (start, end) = ([-150.0, 40.0, 300.0], [150.0, 80.0, 330.0]);
        for receiver in [
            [0.0, 0.0],
            [-2_000.0, 500.0],
            [300.0, 4_000.0],
            [9_000.0, -9_000.0],
        ] {
            let shift = |p: [f64; 3]| [p[0] - receiver[0], p[1] - receiver[1], p[2]];
            let exact = kernel_sel(&emission, shift(start), shift(end)).expect("heard");
            let boxed =
                box_day_sel(&box_of(&[(emission, shift(start), shift(end))])).expect("heard");
            assert!(
                (boxed - exact).abs() < 1e-6,
                "{designator} at {receiver:?}: box {boxed} vs kernel {exact}"
            );
        }
    }
}

/// The pieces of a 150 m box crossed by 40 flights: one flow (a departure corridor, directions
/// within 11 deg, climbs 5-10 %) or mixed (a quarter reversed, approaches descending among the
/// departures).
fn bundle(mixed: bool) -> Vec<(SegmentEmission, [f64; 3], [f64; 3])> {
    let mut pieces = Vec::new();
    for i in 0..40usize {
        let f = i as f64;
        let offset = [((f * 37.0) % 150.0) - 75.0, ((f * 53.0) % 150.0) - 75.0];
        let height = 400.0 + (f * 29.0) % 150.0;
        let (designator, departure) = [("A320", true), ("B738", !mixed), ("A20N", true)][i % 3];
        let speed = 140.0 + (f * 7.0) % 60.0;
        let climb = if departure {
            0.05 + (f * 0.013) % 0.05
        } else {
            -0.05
        };
        let emission = emission(designator, &flight(departure, speed, climb, height));
        let direction = if mixed && i % 4 == 0 { -1.0 } else { 1.0 };
        let half = 60.0 + (f * 11.0) % 40.0;
        let sideways = 0.2 * ((f * 0.37) % 1.0 - 0.5);
        let start = [
            offset[0] - direction * half,
            offset[1] - direction * sideways * half,
            height - climb * half,
        ];
        let end = [
            offset[0] + direction * half,
            offset[1] + direction * sideways * half,
            height + climb * half,
        ];
        pieces.push((emission, start, end));
    }
    pieces
}

/// Box minus pieces (dB) at a receiver.
fn box_error_db(pieces: &[(SegmentEmission, [f64; 3], [f64; 3])], receiver: [f64; 2]) -> f64 {
    let values = box_of(pieces);
    let shift = |p: [f64; 3]| [p[0] - receiver[0], p[1] - receiver[1], p[2]];
    let exact_energy: f64 = pieces
        .iter()
        .filter_map(|(emission, start, end)| kernel_sel(emission, shift(*start), shift(*end)))
        .map(|sel| 10f64.powf(sel / 10.0))
        .sum();
    let mut shifted = values;
    shifted.centroid_m = shift(values.centroid_m);
    let boxed = box_day_sel(&shifted).unwrap_or(f64::NEG_INFINITY);
    boxed - 10.0 * exact_energy.log10()
}

/// Receivers abeam or far off the flow's axis, and on its extension (behind the box along the
/// axis), where Doc 29 reads each piece at the closest point of its own extended climb.
const ABEAM: [[f64; 2]; 4] = [
    [400.0, 0.0],
    [0.0, -1_000.0],
    [2_500.0, 2_500.0],
    [1_500.0, 12_000.0],
];
const ON_THE_EXTENSION: [[f64; 2]; 2] = [[-4_000.0, 300.0], [-8_000.0, 1_000.0]];

/// One flow reads within 0.1 dB of the sum of its pieces abeam and within 0.75 dB 4 km behind it
/// on its extension (0.70 dB on the NPD curves of the model's atmosphere; 8 km behind, 2.3 dB:
/// the pieces' extended lines scatter by hundreds of metres there; far behind, a box is not what
/// is heard).
#[test]
fn a_box_of_one_flow_reads_as_the_sum_of_its_pieces() {
    let pieces = bundle(false);
    for receiver in ABEAM {
        let error = box_error_db(&pieces, receiver);
        assert!(error.abs() < 0.1, "abeam {receiver:?}: {error:+.3} dB");
    }
    let behind = box_error_db(&pieces, ON_THE_EXTENSION[0]);
    assert!(behind.abs() < 0.75, "behind: {behind:+.3} dB");
    let values = box_of(&pieces);
    assert!(
        values
            .installation_shares
            .iter()
            .flatten()
            .all(|shares| shares[0] > 0.99),
        "{:?}",
        values.installation_shares
    );
}

/// Mixed flows share one average aircraft: abeam within 0.3 dB (12 km off, between 25,000 ft
/// and the tail anchor, the mix's absorption is interpolated linearly and overstates it by a
/// quarter decibel). On the extension 4-8 km behind the box, the two average pieces at the
/// gradient plus and minus its spread still miss climbs and descents extended that far by up to
/// 3 dB; a kilometre along, beside a runway, they took the error from 0.7 to 0.2 dB (eight days
/// at three airports).
#[test]
fn a_box_of_mixed_flows_errs_on_its_extension_only() {
    let pieces = bundle(true);
    for receiver in ABEAM {
        let error = box_error_db(&pieces, receiver);
        assert!(error.abs() < 0.3, "abeam {receiver:?}: {error:+.3} dB");
    }
    for receiver in ON_THE_EXTENSION {
        let error = box_error_db(&pieces, receiver);
        assert!(
            error.abs() < 3.5,
            "on the extension {receiver:?}: {error:+.3} dB"
        );
    }
}

/// Sums merged from two halves equal the sums of the whole.
#[test]
fn merged_sums_equal_the_whole() {
    let pieces = bundle(true);
    let (mut first, mut second, mut whole) =
        (BoxSums::default(), BoxSums::default(), BoxSums::default());
    for (index, (emission, start, end)) in pieces.iter().enumerate() {
        let half = if index % 2 == 0 {
            &mut first
        } else {
            &mut second
        };
        for sums in [half, &mut whole] {
            sums.add(
                &emission.npd_distance_levels(),
                emission.installation,
                [0.5, 0.3, 0.2],
                *start,
                *end,
            );
        }
    }
    first.merge(&second);
    let (merged, direct) = (first.values().unwrap(), whole.values().unwrap());
    assert_eq!(first.pieces(), whole.pieces());
    for (a, b) in merged
        .levels_db
        .iter()
        .flatten()
        .zip(direct.levels_db.iter().flatten())
    {
        assert!((a - b).abs() < 1e-9, "{a} vs {b}");
    }
    assert!((merged.axis_rad - direct.axis_rad).abs() < 1e-9);
    assert!((merged.gradient - direct.gradient).abs() < 1e-9);
    assert!((merged.centroid_m[2] - direct.centroid_m[2]).abs() < 1e-6);
}

/// Jets and turboprops in one box: propellers fall off slower than jets, so far away the mix
/// is theirs. The shares at the tail anchor and the anchor itself keep the box within 0.4 dB of
/// its pieces out to 16 km (one share and a tail fitted to the sum read 2 dB low there); the most
/// where a turboprop at climb power overtakes the jet, 12-16 km out.
#[test]
fn a_box_of_jets_and_propellers_reads_their_mix_far_away() {
    let altitude = 1_000.0;
    let pieces: Vec<_> = [("A320", 0.0), ("DH8D", 20.0)]
        .iter()
        .map(|&(designator, offset)| {
            let emission = emission(designator, &flight(true, 150.0, 0.06, altitude));
            let start = [-50.0 + offset, offset, altitude - 3.0];
            let end = [50.0 + offset, offset, altitude + 3.0];
            (emission, start, end)
        })
        .collect();
    for north in [2_000.0, 4_000.0, 8_000.0, 12_000.0, 16_000.0] {
        let error = box_error_db(&pieces, [0.0, north]);
        assert!(error.abs() < 0.4, "{north} m: {error:+.3} dB");
    }
}

/// What a box is made of follows its pieces: the received energy's share of wing-mounted jets is
/// the jet's share of the pieces' own SELs within 15 points, near and far. A box of one climb is
/// within 7 (between 1,000 ft and the tail anchor it interpolates the shares: 0.30 for 0.23 at
/// 4 km); a box of a climbing jet and a descending turboprop keeps one mix for both its average
/// pieces, so 500 m from it the jets read 0.58 for 0.72.
#[test]
fn a_box_tells_its_jets_from_its_propellers_as_its_pieces_do() {
    let altitude = 1_000.0;
    for propeller_climb in [0.06, -0.05] {
        let pieces: Vec<_> = [
            ("A320", 0.0, 0.06, true),
            ("DH8D", 20.0, propeller_climb, propeller_climb > 0.0),
        ]
        .iter()
        .map(|&(designator, offset, climb, departure)| {
            let emission = emission(designator, &flight(departure, 150.0, climb, altitude));
            let rise = 50.0 * climb;
            let start = [-50.0 + offset, offset, altitude - rise];
            let end = [50.0 + offset, offset, altitude + rise];
            (emission, start, end)
        })
        .collect();
        let values = box_of(&pieces);
        assert_eq!(values.gradient_spread > 1e-6, propeller_climb < 0.0);
        for north in [500.0, 4_000.0, 12_000.0] {
            let shift = |p: [f64; 3]| [p[0], p[1] - north, p[2]];
            let [jet, propeller] = [0, 1].map(|k| {
                let (emission, start, end) = &pieces[k];
                10f64.powf(kernel_sel(emission, shift(*start), shift(*end)).unwrap() / 10.0)
            });
            let at = AircraftBoxAtReceiver {
                centroid_m: shift(values.centroid_m),
                axis_rad: values.axis_rad,
                gradient: values.gradient,
                gradient_spread: values.gradient_spread,
                piece_length_m: values.piece_length_m,
                levels_db: &values.levels_db,
                tail_levels_db: &values.tail_levels_db,
                lg_scaled_distance: &values.scaled_distance_m.map(f64::log10),
                installation_shares: values.installation_shares,
                ground_m: -4.0,
            };
            let fractions = box_sel_at_receiver(&at, &Unscreened).installation_fractions[0];
            let expected = jet / (jet + propeller);
            assert!(
                (fractions[0] - expected).abs() < 0.15 && fractions[1] == 0.0,
                "climb {propeller_climb}, {north} m: wing jets {:.3} of the box, {expected:.3} of the pieces",
                fractions[0]
            );
            assert!((fractions.iter().sum::<f64>() - 1.0).abs() < 1e-9);
        }
    }
}

/// Jets by day and propellers by night are two mixes (Codex, review of the r054 plan): each period
/// reads its own installation shares, the night the propellers' Delta_I and the day the wing jets',
/// where one mix for the whole day gave the night the day's jets.
#[test]
fn each_period_reads_its_own_installation_mix() {
    let altitude = 1_000.0;
    let mut sums = BoxSums::default();
    for (designator, weights) in [("A320", [1.0, 0.0, 0.0]), ("DH8D", [0.0, 0.0, 1.0])] {
        let emission = emission(designator, &flight(true, 150.0, 0.06, altitude));
        sums.add(
            &emission.npd_distance_levels(),
            emission.installation,
            weights,
            [-50.0, 2_000.0, altitude - 3.0],
            [50.0, 2_000.0, altitude + 3.0],
        );
    }
    let values = sums.values().expect("a box with energy");
    let at = AircraftBoxAtReceiver {
        centroid_m: values.centroid_m,
        axis_rad: values.axis_rad,
        gradient: values.gradient,
        gradient_spread: values.gradient_spread,
        piece_length_m: values.piece_length_m,
        levels_db: &values.levels_db,
        tail_levels_db: &values.tail_levels_db,
        lg_scaled_distance: &values.scaled_distance_m.map(f64::log10),
        installation_shares: values.installation_shares,
        ground_m: -4.0,
    };
    let sel = box_sel_at_receiver(&at, &Unscreened);
    let close = |a: [f64; 3], b: [f64; 3]| a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-9);
    assert!(close(sel.installation_fractions[0], [1.0, 0.0, 0.0]));
    assert!(close(sel.installation_fractions[2], [0.0, 0.0, 1.0]));
    let [east, north, height] = sel.closest.on_line_m;
    let slant = east.hypot(north).hypot(height);
    let correction = |installation| installation_correction_db(installation, height, slant);
    assert!((sel.installation_correction_db[0] - correction(Installation::Wing)).abs() < 1e-9);
    assert!((sel.installation_correction_db[2] - correction(Installation::Propeller)).abs() < 1e-9);
    // 2 km aside at 1,000 m the wing jets' Delta_I is -0.07 dB, the propellers' 0.
    assert!((sel.installation_correction_db[0] - sel.installation_correction_db[2]).abs() > 0.05);
}
