//! The German test tasks for Doc 29 noise mapping (UBA Texte 11/2022, "Testaufgaben zur BUF"):
//! the A320's, the CRJ900's and the DH8C's straight and curved departures and approaches of the
//! fictitious test airport, their segments (Appendix B: position, true airspeed, the thrust each
//! segment reads its NPD at) put through this kernel at the test tasks' receivers (Table 30)
//! against their single-event levels (Tables 32 to 34). The tasks' atmosphere is 10 C / 70 % (their NPD curves recalculated by Appendix D,
//! impedance 0.11 dB); the kernel's curves are in 15 C / 70 %, so each segment's level moves by the
//! difference of the two increments at its slant.

use super::atmosphere::{impedance_adjustment_db, npd_increments_db, rates_db_per_m};
use super::npd::{METRES_PER_FOOT, NPD_DISTANCES, NPD_DISTANCES_FT};
use super::screening::Unscreened;
use super::segment::{SegmentEmission, SegmentGeometry, segment_sel_at_receiver};
use super::spectra_generated::SPECTRA;
use super::thrust::{MAX_POWER_ROWS, PowerBracket, bracket_power};
use super::thrust_generated::THRUST;
use crate::atmosphere::REFERENCE_PRESSURE_KPA;
use crate::doc29::atmosphere::SHIFT_DISTANCES;
use crate::doc29::corrections::speed_correction_db;
use crate::doc29::npd::class_anchor;
use crate::doc29::profiles_generated::{noise_class_of, profile_idx};

/// The noise class of a case's aircraft (the A320's; the CRJ900's, anchored on the CRJ9-ER the
/// tasks use; the DH8C's, anchored on the DHC830).
fn class_of_case(case: &str) -> usize {
    let designator = case.split(' ').next().expect("a case names its aircraft");
    usize::from(noise_class_of(profile_idx(designator)))
}

/// Pounds per unit of a case's thrust: newtons, and the DH8C's % of its 4,918 lb maximum static
/// thrust (the ANP's DHC830).
fn pounds_per_unit(case: &str) -> f64 {
    if case.starts_with("DH8C") {
        4_918.0 / 100.0
    } else {
        1.0 / NEWTONS_PER_POUND
    }
}
const NEWTONS_PER_POUND: f64 = 4.448_222;
const METRES_PER_SECOND_PER_KNOT: f64 = 0.514_444;

/// The increment difference (dB) between the tasks' atmosphere and the kernel's at `slant_m`,
/// linear in log distance as the curves are.
fn atmosphere_difference_db(class: usize, departure: bool, slant_m: f64) -> f64 {
    let spectra = SPECTRA[class].as_ref().expect("the class has spectra");
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

/// How a segment's thrust reads the NPD rows: the product's Eq. 4-3 bracket, the edge row past
/// the table (`bracket_power`), or the tasks' rule, the edge pair's line continued beyond it.
#[derive(Clone, Copy, PartialEq)]
enum PowerRule {
    EdgeRow,
    Extrapolated,
}

fn extrapolated_bracket(powers: &[f64; MAX_POWER_ROWS], rows: u8, thrust_lb: f64) -> PowerBracket {
    let last = usize::from(rows) - 1;
    let row = if thrust_lb < powers[0] {
        0
    } else if thrust_lb >= powers[last] {
        last - 1
    } else {
        return bracket_power(powers, rows, thrust_lb);
    };
    PowerBracket {
        row,
        weight: (thrust_lb - powers[row]) / (powers[row + 1] - powers[row]),
    }
}

/// The single-event level of one case at one receiver: every segment of its point list.
fn case_sel_db(case: &str, points: &[[f64; 5]], receiver: [f64; 3], rule: PowerRule) -> f64 {
    let (class, departure) = (class_of_case(case), case.contains(" D"));
    let model = &THRUST[class];
    let anchor = class_anchor(class);
    let (powers, rows) = if departure {
        (&model.dep_power, model.dep_rows)
    } else {
        (&model.app_power, model.app_rows)
    };
    let mut energy = 0.0;
    for pair in points.windows(2) {
        let [a, b] = [pair[0], pair[1]];
        let speed_m_per_s = 0.5 * (a[3] + b[3]);
        let thrust_lb = 0.5 * (a[4] + b[4]) * pounds_per_unit(case);
        let emission = SegmentEmission {
            class,
            departure,
            power: match rule {
                PowerRule::EdgeRow => bracket_power(powers, rows, thrust_lb),
                PowerRule::Extrapolated => extrapolated_bracket(powers, rows, thrust_lb),
            },
            installation: anchor.installation,
            speed_correction_db: speed_correction_db(
                anchor.v_ref_kt,
                speed_m_per_s / METRES_PER_SECOND_PER_KNOT,
            ),
            helicopter_correction_db: 0.0,
            atmosphere_shift_db: [0.0; SHIFT_DISTANCES],
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
        energy += 10f64
            .powf((sel.free_sel_db + atmosphere_difference_db(class, departure, slant)) / 10.0);
    }
    10.0 * energy.log10()
}

/// Receivers where a case hears its ground roll more than its flight, which this kernel does not
/// model as Doc 29 does (r051 takes runway rolls as airport ground operations): beside and behind
/// the start of roll the tasks add the start-of-roll directivity (the kernel reads 3.6 dB high to
/// 3.4 dB low there), and past the runway end an arrival's reverse-thrust increment (1.4-1.9 dB low).
const GROUND_ROLL_RECEIVERS: [(&str, &str); 4] =
    [("D", "IP02"), ("D", "IP03"), ("D", "IP04"), ("A", "IP05")];

/// Every case of `aircraft` at every receiver but the ground-roll ones: (case, receiver,
/// published level, the kernel's level under `rule`).
fn flight_receivers(aircraft: &str, rule: PowerRule) -> Vec<(String, String, f64, f64)> {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("buf2022.json")).expect("the fixture");
    let receivers = &fixture["receivers"];
    let mut rows = Vec::new();
    for (case, reference) in fixture["reference"].as_object().unwrap() {
        if !case.starts_with(aircraft) {
            continue;
        }
        let points: Vec<[f64; 5]> = fixture["cases"][case]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| std::array::from_fn(|k| p[k].as_f64().unwrap()))
            .collect();
        let operation = if case.contains(" D") { "D" } else { "A" };
        for (receiver, level) in reference.as_object().unwrap() {
            if GROUND_ROLL_RECEIVERS.contains(&(operation, receiver.as_str())) {
                continue;
            }
            let at: [f64; 3] = std::array::from_fn(|k| receivers[receiver][k].as_f64().unwrap());
            let model = case_sel_db(case, &points, at, rule);
            let published = level.as_f64().unwrap();
            println!(
                "{case} {receiver}: published {published:.1} model {model:.2} ({:+.2})",
                model - published
            );
            rows.push((case.clone(), receiver.clone(), published, model));
        }
    }
    rows
}

/// Every other case and receiver of the A320 and the CRJ900 (50) reads within the tasks' own
/// precision target, 0.5 dB, of the published level (in the night of 2026-09-30: A320 -0.39 to
/// +0.42 dB, CRJ900 on the CRJ9-ER anchor -0.19 to +0.24 dB); their thrust stays within the rows.
#[test]
fn the_a320_and_crj900_cases_of_the_buf_test_tasks() {
    let mut checked = 0;
    for aircraft in ["A320", "CRJ9"] {
        for (case, receiver, published, model) in flight_receivers(aircraft, PowerRule::EdgeRow) {
            assert!(
                (model - published).abs() < 0.5,
                "{case} {receiver}: {model:.2} vs {published}"
            );
            checked += 1;
        }
    }
    assert_eq!(checked, 50);
}

/// The DH8C's cases (Table 34) on the DHC830 class. Its rows span 90-150 % of the maximum static
/// thrust departing and 35-40 % approaching, while its climb flies 63-97 % and its approach
/// 4-30 %: the tasks continue the edge pair's line past the table, a rule Doc 29 leaves open (Eq.
/// 4-3 holds between tabulated powers), where the product reads the edge row. Under the tasks'
/// rule the kernel reads the 25 flight receivers within 0.6 dB (the most, -0.53 dB, beside the
/// lift-off at IP05): the speed correction's 160 kt, the class's ratings and the propeller's
/// atmosphere increments hold. With the edge rows it reads 3-9 dB higher under the climb and 5-22
/// dB under the approach, what the tasks' extrapolation takes off.
#[test]
fn the_dh8c_cases_of_the_buf_test_tasks_under_their_power_rule() {
    let tasks = flight_receivers("DH8C", PowerRule::Extrapolated);
    let product = flight_receivers("DH8C", PowerRule::EdgeRow);
    assert_eq!(tasks.len(), 25);
    for ((case, receiver, published, model), edge) in tasks.iter().zip(&product) {
        assert!(
            (model - published).abs() < 0.6,
            "{case} {receiver}: {model:.2} vs {published}"
        );
        assert!(
            edge.3 >= model - 0.01,
            "{case} {receiver}: the edge row reads higher"
        );
    }
}
