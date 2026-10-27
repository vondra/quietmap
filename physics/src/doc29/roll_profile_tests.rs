use super::*;
use crate::doc29::profiles_generated::{noise_class_of, profile_idx};

fn class(designator: &str) -> usize {
    usize::from(noise_class_of(profile_idx(designator)))
}

/// Doc 29's example (Fig. 3-5): 1,600 m to lift-off at 75 m/s is 8 segments, 25 m long first, 375
/// m last; the German test tasks' A320 take-off (1,812.25 m, 83.69 m/s) is their 9 segments.
#[test]
fn a_take_off_roll_is_segmented_as_doc_29_does() {
    let example = RollProfile {
        kind: RollKind::TakeOff,
        class: class("A320"),
        length_m: 1_600.0,
        speed_ms: 75.0,
        field_altitude_m: 0.0,
    }
    .steps();
    assert_eq!(example.len(), 8);
    assert!((example[0].to_m - 25.0).abs() < 1e-9);
    assert!((example[7].to_m - example[7].from_m - 375.0).abs() < 1e-9);
    let tasks = RollProfile {
        length_m: 1_812.25,
        speed_ms: 83.69,
        ..RollProfile {
            kind: RollKind::TakeOff,
            class: class("A320"),
            length_m: 0.0,
            speed_ms: 0.0,
            field_altitude_m: 0.0,
        }
    }
    .steps();
    let ends: Vec<f64> = tasks.iter().map(|step| step.to_m).collect();
    for (end, published) in ends.iter().zip([22.37, 89.49, 201.36, 357.98, 559.34]) {
        assert!((end - published).abs() < 0.01, "{end} vs {published}");
    }
    // The first segment flies at half a speed step: 12 dB of slowness against 160 kt.
    assert!((tasks[0].emission.speed_correction_db - 12.53).abs() < 0.05);
}

/// A landing decelerates to 15 m/s in the German test tasks' A320 geometry: their 6 segments and
/// a seventh where the power turns at a tenth of the stop; reverse thrust's increment rises to 5 dB
/// at that tenth and is gone at the stop; a Cessna 172's reverse power stays within its approach
/// rows (its propeller rating read at rest had put it past them).
#[test]
fn a_landing_roll_runs_on_reverse_thrust() {
    let landing = |class, length_m, speed_ms| {
        RollProfile {
            kind: RollKind::Landing,
            class,
            length_m,
            speed_ms,
            field_altitude_m: 0.0,
        }
        .steps()
    };
    let steps = landing(class("A320"), 948.24, 68.26);
    assert_eq!(steps.len(), 7);
    assert!((steps.last().unwrap().to_m - 948.24).abs() < 1e-6);
    assert!(steps.iter().any(|step| (step.to_m - 94.824).abs() < 1e-6));
    assert!(
        (steps.iter())
            .flat_map(|step| step.increment_db)
            .all(|db| (0.0..=REVERSE_INCREMENT_DB).contains(&db))
    );
    assert_eq!(steps[0].increment_db[0], 0.0);
    assert!((reverse_increment_db(0.1) - 5.0).abs() < 1e-12);
    assert_eq!(reverse_increment_db(1.0), 0.0);
    assert!(steps.windows(2).all(|pair| pair[1].from_m == pair[0].to_m));
    let cessna = landing(class("C172"), 250.0, 55.0 * 1_852.0 / 3_600.0);
    let model = &crate::doc29::thrust_generated::THRUST[class("C172")];
    let top_row_lb = model.app_power[usize::from(model.app_rows) - 1];
    assert!(
        (cessna.iter())
            .flat_map(|step| step.thrust_lb.expect("a thrust model"))
            .all(|thrust| thrust < top_row_lb),
        "{:?}",
        cessna.iter().map(|step| step.thrust_lb).collect::<Vec<_>>()
    );
}
