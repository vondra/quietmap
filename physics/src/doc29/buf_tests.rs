//! The German test tasks for Doc 29 noise mapping (UBA Texte 11/2022, "Testaufgaben zur BUF"):
//! the A320's straight and curved departures and approaches of the fictitious test airport, their
//! segments (Appendix B: position, true airspeed, the thrust each segment reads its NPD at) put
//! through this kernel at the test tasks' receivers (Table 30) against their single-event levels
//! (Table 32). The tasks' atmosphere is 10 C / 70 % (their NPD curves recalculated by Appendix D,
//! impedance 0.11 dB); the kernel's curves are in 15 C / 70 %, so each segment's level moves by the
//! difference of the two increments at its slant.

use super::atmosphere::{impedance_adjustment_db, npd_increments_db, rates_db_per_m};
use super::npd::{METRES_PER_FOOT, NPD_DISTANCES, NPD_DISTANCES_FT};
use super::screening::Unscreened;
use super::segment::{SegmentEmission, SegmentGeometry, segment_sel_at_receiver};
use super::spectra_generated::SPECTRA;
use super::thrust::bracket_power;
use super::thrust_generated::THRUST;
use crate::atmosphere::REFERENCE_PRESSURE_KPA;
use crate::doc29::corrections::speed_correction_db;
use crate::doc29::npd::class_anchor;

const A320_CLASS: usize = 1;
const NEWTONS_PER_POUND: f64 = 4.448_222;
const METRES_PER_SECOND_PER_KNOT: f64 = 0.514_444;

/// The increment difference (dB) between the tasks' atmosphere and the kernel's at `slant_m`,
/// linear in log distance as the curves are.
fn atmosphere_difference_db(departure: bool, slant_m: f64) -> f64 {
    let spectra = SPECTRA[A320_CLASS].as_ref().expect("A320 spectra");
    let spectrum = if departure {
        &spectra.departure_db
    } else {
        &spectra.approach_db
    };
    let tasks = npd_increments_db(spectrum, &rates_db_per_m(10.0, 70.0));
    let kernel = npd_increments_db(spectrum, &rates_db_per_m(15.0, 70.0));
    let difference: [f64; NPD_DISTANCES] = std::array::from_fn(|k| tasks[k] - kernel[k]);
    let log_d = (slant_m / METRES_PER_FOOT).log10();
    let logs = NPD_DISTANCES_FT.map(f64::log10);
    let impedance = impedance_adjustment_db(10.0, REFERENCE_PRESSURE_KPA)
        - impedance_adjustment_db(15.0, REFERENCE_PRESSURE_KPA);
    if log_d <= logs[0] {
        return difference[0] + impedance;
    }
    if log_d >= logs[NPD_DISTANCES - 1] {
        return difference[NPD_DISTANCES - 1] + impedance;
    }
    let k = (0..NPD_DISTANCES - 1)
        .find(|&k| log_d <= logs[k + 1])
        .expect("inside");
    let t = (log_d - logs[k]) / (logs[k + 1] - logs[k]);
    difference[k] + t * (difference[k + 1] - difference[k]) + impedance
}

/// The single-event level of one case at one receiver: every segment of its point list.
fn case_sel_db(points: &[[f64; 5]], departure: bool, receiver: [f64; 3]) -> f64 {
    let model = &THRUST[A320_CLASS];
    let anchor = class_anchor(A320_CLASS);
    let (powers, rows) = if departure {
        (&model.dep_power, model.dep_rows)
    } else {
        (&model.app_power, model.app_rows)
    };
    let mut energy = 0.0;
    for pair in points.windows(2) {
        let [a, b] = [pair[0], pair[1]];
        let speed_m_per_s = 0.5 * (a[3] + b[3]);
        let thrust_lb = 0.5 * (a[4] + b[4]) / NEWTONS_PER_POUND;
        let emission = SegmentEmission {
            class: A320_CLASS,
            departure,
            power: bracket_power(powers, rows, thrust_lb),
            installation: anchor.installation,
            speed_correction_db: speed_correction_db(
                anchor.v_ref_kt,
                speed_m_per_s / METRES_PER_SECOND_PER_KNOT,
            ),
            helicopter_correction_db: 0.0,
        };
        let relative = |p: [f64; 5]| [p[0] - receiver[0], p[1] - receiver[1], p[2] - receiver[2]];
        let geometry = SegmentGeometry {
            start_m: relative(a),
            end_m: relative(b),
            ground_under_start_m: -receiver[2],
            ground_under_end_m: -receiver[2],
        };
        let sel = segment_sel_at_receiver(&emission, &geometry, &Unscreened);
        let slant = sel.closest.on_line_m[0]
            .hypot(sel.closest.on_line_m[1])
            .hypot(sel.closest.on_line_m[2]);
        energy += 10f64.powf((sel.free_sel_db + atmosphere_difference_db(departure, slant)) / 10.0);
    }
    10.0 * energy.log10()
}

/// Receivers where a case hears its ground roll more than its flight, which this kernel does not
/// model as Doc 29 does (r051 takes runway rolls as airport ground operations): beside and behind
/// the start of roll the tasks add the start-of-roll directivity (the kernel reads 2.2-3.4 dB low
/// there), and past the runway end an arrival's reverse-thrust increment (1.9 dB low).
const GROUND_ROLL_RECEIVERS: [(&str, &str); 4] =
    [("D", "IP02"), ("D", "IP03"), ("D", "IP04"), ("A", "IP05")];

/// Every other case and receiver of the fixture (25) reads within the tasks' own precision target,
/// 0.5 dB, of the published level (in the night of 2026-09-30: -0.39 to +0.42 dB).
#[test]
fn the_a320_cases_of_the_buf_test_tasks() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("buf2022_a320.json")).expect("the fixture");
    let receivers = &fixture["receivers"];
    let mut rows = Vec::new();
    for (case, reference) in fixture["reference"].as_object().unwrap() {
        let departure = case.contains(" D");
        let points: Vec<[f64; 5]> = fixture["cases"][case]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| std::array::from_fn(|k| p[k].as_f64().unwrap()))
            .collect();
        for (receiver, level) in reference.as_object().unwrap() {
            let at: [f64; 3] = std::array::from_fn(|k| receivers[receiver][k].as_f64().unwrap());
            let model = case_sel_db(&points, departure, at);
            rows.push((
                case.clone(),
                receiver.clone(),
                level.as_f64().unwrap(),
                model,
            ));
        }
    }
    let mut checked = 0;
    for (case, receiver, published, model) in &rows {
        let operation = if case.contains(" D") { "D" } else { "A" };
        let ground_roll = GROUND_ROLL_RECEIVERS
            .iter()
            .any(|&(op, ip)| op == operation && ip == receiver);
        println!(
            "{case} {receiver}: published {published:.1} model {model:.2} ({:+.2})",
            model - published
        );
        if !ground_roll {
            assert!(
                (model - published).abs() < 0.5,
                "{case} {receiver}: {model:.2} vs {published}"
            );
            checked += 1;
        }
    }
    assert_eq!(checked, 25);
}
