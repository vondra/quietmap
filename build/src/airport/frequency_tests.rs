//! Regression examples for sampling denominators, operation imbalance and absent templates.

use super::*;
use crate::airport::lines::{Aeroway, AerowayLine, Airport};
use crate::boxes::Window;
use crate::dev4::Square;

fn fixture() -> (Aeroways, Runways, RollKey, RollKey) {
    let mut ways = Aeroways::default();
    for (index, key) in ["COUNTED", "OBSERVED"].iter().enumerate() {
        let airport = ways.intern(Airport {
            key: (*key).into(),
            name: (*key).into(),
        });
        ways.lines.push(AerowayLine {
            osm_id: index as i64,
            segment: 0,
            ends: [[50.0 + index as f64, 14.0], [50.0 + index as f64, 14.04]],
            aeroway: Aeroway::Runway,
            airport,
            square: Square { x: 275, y: 173 },
        });
    }
    let runways = Runways::new(&mut ways);
    let arrival = RollKey {
        runway: 0,
        forward: true,
        landing: true,
        class: 0,
    };
    let departure = RollKey {
        landing: false,
        ..arrival
    };
    assert!(Class::Jet.contains(arrival.class));
    (ways, runways, arrival, departure)
}

fn input(counts: serde_json::Value, first: &str, last: &str) -> Operations {
    serde_json::from_value(json!({"first_month": first, "last_month": last,
        "sources": {"official": {"urls": ["https://example.org/counts"], "licence": "CC0",
            "public_ok": true, "scope": "Itinerant fixed-wing movements; local traffic excluded"}},
        "counts": counts}))
    .unwrap()
}

#[test]
fn missing_sample_days_do_not_invent_missing_movements() {
    let (ways, runways, arrival, departure) = fixture();
    let window = Window {
        baseline_days: ["2026-01-01", "2026-01-02", "2026-02-01"]
            .map(String::from)
            .to_vec(),
        increment_days: vec!["2026-01-01".into()],
    };
    let mut months = Months::new(&window);
    // Every observed day has 3 primary and 2 secondary-only movements of each operation.
    // February has no secondary sample: the existing window estimate still supplies its rate.
    for day in &window.baseline_days {
        let primary = BTreeMap::from([(arrival, [3.0, 0.0, 0.0]), (departure, [3.0, 0.0, 0.0])]);
        let secondary = if day == "2026-01-01" {
            BTreeMap::from([(arrival, [2.0, 0.0, 0.0]), (departure, [2.0, 0.0, 0.0])])
        } else {
            Rolls::new()
        };
        months.add(day, [primary, secondary]);
    }
    let mut traffic = Traffic::new(&ways);
    traffic.rolls = BTreeMap::from([(arrival, [5.0, 0.0, 0.0]), (departure, [5.0, 0.0, 0.0])]);
    traffic.movements[0] = [5.0, 5.0, 0.0];
    let before = traffic.rolls.clone();
    let input = input(
        json!([
            {"airport":"COUNTED", "month":"2026-01", "class":"jet", "movements":310, "arrivals":155, "source":"official"},
            {"airport":"COUNTED", "month":"2026-02", "class":"jet", "movements":280, "arrivals":140, "source":"official"}
        ]),
        "2026-01",
        "2026-02",
    );
    input.validate(&window).unwrap();
    let out = std::env::temp_dir().join(format!(
        "qm-frequency-{}-{}",
        std::thread::current().name().unwrap_or("test"),
        std::process::id()
    ));
    std::fs::create_dir_all(&out).unwrap();
    months
        .supplement(&input, &mut traffic, &ways, &runways, &out)
        .unwrap();
    assert_eq!(
        traffic.rolls, before,
        "a perfectly observed daily rate must not be supplemented twice"
    );
    assert_eq!(traffic.movements[0], [5.0, 5.0, 0.0]);
    std::fs::remove_dir_all(out).unwrap();
}

#[test]
fn a_month_without_rolls_uses_the_window_mixture_and_full_window_divisor() {
    let (ways, runways, arrival, departure) = fixture();
    let window = Window {
        baseline_days: vec!["2026-01-01".into(), "2026-02-01".into()],
        increment_days: vec![],
    };
    let mut months = Months::new(&window);
    months.add(
        "2026-01-01",
        [
            BTreeMap::from([(arrival, [3.0, 0.0, 1.0]), (departure, [0.0, 0.0, 2.0])]),
            Rolls::new(),
        ],
    );
    months.add("2026-02-01", ProviderRolls::default());
    let mut traffic = Traffic::new(&ways);
    traffic.rolls = months.window_rates();
    traffic.movements[0] = [2.0, 1.0, 0.0];
    let input = input(
        json!([
            {"airport":"COUNTED", "month":"2026-01", "class":"jet", "movements":186, "arrivals":124, "source":"official"},
            {"airport":"COUNTED", "month":"2026-02", "class":"jet", "movements":140, "arrivals":70, "source":"official"}
        ]),
        "2026-01",
        "2026-02",
    );
    input.validate(&window).unwrap();
    let out = std::env::temp_dir().join(format!("qm-frequency-window-{}", std::process::id()));
    std::fs::create_dir_all(&out).unwrap();
    months
        .supplement(&input, &mut traffic, &ways, &runways, &out)
        .unwrap();
    let added = 70.0 / 59.0;
    for (value, expected) in
        traffic.rolls[&arrival]
            .iter()
            .zip([1.5 + 0.75 * added, 0.0, 0.5 + 0.25 * added])
    {
        assert!((value - expected).abs() < 1e-12);
    }
    assert_eq!(traffic.rolls[&departure], [0.0, 0.0, 1.0 + added]);
    assert_eq!(traffic.movements[0], [2.0 + added, 1.0 + added, 0.0]);
    let rows: Vec<serde_json::Value> = std::fs::read_to_string(out.join("operations.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows[0]["added"], json!([0.0, 0.0]));
    assert_eq!(rows[1]["mixture"], json!(["window", "window"]));
    std::fs::remove_dir_all(out).unwrap();
}

#[test]
fn missing_departures_do_not_become_extra_landings_and_unknown_classes_stay_unknown() {
    let (mut ways, runways, arrival, departure) = fixture();
    ways.intern(Airport {
        key: "ORPHAN".into(),
        name: String::new(),
    });
    let window = Window {
        baseline_days: vec!["2026-05-01".into()],
        increment_days: vec![],
    };
    let mut months = Months::new(&window);
    let uncovered = RollKey {
        runway: 1,
        ..arrival
    };
    // 426 arrivals and 62 departures observed in a day; the publisher gives each side.
    let raw = BTreeMap::from([
        (arrival, [400.0, 0.0, 26.0]),
        (departure, [52.0, 0.0, 10.0]),
        (uncovered, [7.0, 0.0, 0.0]),
    ]);
    months.add("2026-05-01", [raw.clone(), Rolls::new()]);
    let mut traffic = Traffic::new(&ways);
    traffic.rolls = raw;
    traffic.movements[0] = [426.0, 62.0, 0.0];
    traffic.movements[1][0] = 7.0;
    let mut input = input(
        json!([
            {"airport":"COUNTED", "month":"2026-05", "class":"jet", "movements":28000, "arrivals":14000, "source":"official"},
            {"airport":"COUNTED", "month":"2026-05", "class":"piston", "movements":5, "source":"official"},
            {"airport":"ORPHAN", "month":"2026-05", "class":"jet", "movements":10, "source":"official"}
        ]),
        "2026-05",
        "2026-05",
    );
    input.validate(&window).unwrap();
    let out = std::env::temp_dir().join(format!(
        "qm-frequency-{}-{}",
        std::thread::current().name().unwrap_or("test"),
        std::process::id()
    ));
    std::fs::create_dir_all(&out).unwrap();
    months
        .supplement(&input, &mut traffic, &ways, &runways, &out)
        .unwrap();
    assert!((traffic.rolls[&arrival].iter().sum::<f64>() * 31.0 - 14000.0).abs() < 1e-8);
    assert!((traffic.rolls[&departure].iter().sum::<f64>() * 31.0 - 14000.0).abs() < 1e-8);
    assert_eq!(traffic.rolls[&uncovered], [7.0, 0.0, 0.0]);
    assert_eq!(
        traffic.rolls.len(),
        3,
        "an unseen class cannot acquire a runway template"
    );
    let records: Vec<serde_json::Value> = std::fs::read_to_string(out.join("operations.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(records[0]["class"], "jet");
    assert_eq!(records[1]["unallocated"], json!([2.5, 2.5]));
    assert_eq!(records[2]["airport_mapped"], false);
    assert_eq!(records[2]["unallocated"], json!([5.0, 5.0]));
    // A combined total supplies a flow constraint, not evidence for the biased observed split.
    assert_eq!(
        deficits(28000.0, None, [426.0 * 31.0, 62.0 * 31.0]),
        [794.0, 12078.0]
    );
    assert_eq!(deficits(100.0, None, [90.0, 0.0]), [0.0, 10.0]);
    assert_eq!(deficits(100.0, None, [90.0, 20.0]), [0.0, 0.0]);
    assert_eq!(deficits(100.0, Some(50), [70.0, 10.0]), [0.0, 40.0]);
    input.counts[0].airport.push(' ');
    assert!(input.validate(&window).is_err());
    input.counts[0].airport.pop();
    input.counts[1].class = Class::All;
    assert!(input.validate(&window).unwrap_err().contains("overlapping"));
    input.counts.truncate(1);
    input.counts[0].arrivals = Some(30000);
    assert!(input.validate(&window).is_err());
    std::fs::remove_dir_all(out).unwrap();
}

#[test]
fn an_ambiguous_airport_identifier_keeps_observed_rolls_and_reports_unallocated_counts() {
    let (mut ways, runways, arrival, _) = fixture();
    ways.ambiguous_airports.insert("COUNTED".into());
    let window = Window {
        baseline_days: vec!["2026-05-01".into()],
        increment_days: vec![],
    };
    let mut months = Months::new(&window);
    let raw = BTreeMap::from([(arrival, [1.0, 0.0, 0.0])]);
    months.add("2026-05-01", [raw.clone(), Rolls::new()]);
    let mut traffic = Traffic::new(&ways);
    traffic.rolls = raw.clone();
    traffic.movements[0][0] = 1.0;
    let input = input(
        json!([
            {"airport":"COUNTED", "month":"2026-05", "class":"jet", "movements":310, "source":"official"}
        ]),
        "2026-05",
        "2026-05",
    );
    let out = std::env::temp_dir().join(format!("qm-frequency-ambiguous-{}", std::process::id()));
    std::fs::create_dir_all(&out).unwrap();
    months
        .supplement(&input, &mut traffic, &ways, &runways, &out)
        .unwrap();
    assert_eq!(traffic.rolls, raw);
    assert_eq!(traffic.movements[0], [1.0, 0.0, 0.0]);
    let row: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out.join("operations.jsonl")).unwrap())
            .unwrap();
    assert_eq!(row["airport_mapped"], true);
    assert_eq!(row["airport_ambiguous"], true);
    assert_eq!(row["unallocated"], json!([155.0, 155.0]));
    std::fs::remove_dir_all(out).unwrap();
}

#[test]
fn an_unseen_departure_keeps_the_observed_arrivals_class_direction_and_period() {
    let (ways, runways, arrival, departure) = fixture();
    let window = Window {
        baseline_days: vec!["2026-05-01".into()],
        increment_days: vec![],
    };
    let mut months = Months::new(&window);
    let raw = BTreeMap::from([(arrival, [0.0, 0.0, 1.0])]);
    months.add("2026-05-01", [raw.clone(), Rolls::new()]);
    let mut traffic = Traffic::new(&ways);
    traffic.rolls = raw;
    traffic.movements[0][0] = 1.0;
    let input = input(
        json!([
            {"airport":"COUNTED", "month":"2026-05", "class":"jet", "movements":62, "arrivals":31, "source":"official"}
        ]),
        "2026-05",
        "2026-05",
    );
    let out = std::env::temp_dir().join(format!("qm-frequency-opposite-{}", std::process::id()));
    std::fs::create_dir_all(&out).unwrap();
    months
        .supplement(&input, &mut traffic, &ways, &runways, &out)
        .unwrap();
    assert_eq!(
        traffic.rolls,
        BTreeMap::from([(arrival, [0.0, 0.0, 1.0]), (departure, [0.0, 0.0, 1.0])])
    );
    assert_eq!(traffic.movements[0], [1.0, 1.0, 0.0]);
    let row: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out.join("operations.jsonl")).unwrap())
            .unwrap();
    assert_eq!(row["mixture"], json!(["not-needed", "opposite-operation"]));
    std::fs::remove_dir_all(out).unwrap();
}

#[test]
fn an_unsampled_month_reports_the_window_template_it_uses() {
    let (ways, runways, arrival, departure) = fixture();
    let window = Window {
        baseline_days: vec!["2026-01-01".into(), "2026-03-01".into()],
        increment_days: vec![],
    };
    let mut months = Months::new(&window);
    months.add(
        "2026-01-01",
        [
            BTreeMap::from([(arrival, [1.0, 0.0, 0.0]), (departure, [1.0, 0.0, 0.0])]),
            Rolls::new(),
        ],
    );
    months.add("2026-03-01", ProviderRolls::default());
    let mut traffic = Traffic::new(&ways);
    traffic.rolls = months.window_rates();
    traffic.movements[0] = [0.5, 0.5, 0.0];
    let input = input(
        json!([
            {"airport":"COUNTED", "month":"2026-02", "class":"jet", "movements":56,
             "arrivals":28, "source":"official"}
        ]),
        "2026-01",
        "2026-03",
    );
    input.validate(&window).unwrap();
    let out =
        std::env::temp_dir().join(format!("qm-frequency-calendar-gap-{}", std::process::id()));
    std::fs::create_dir_all(&out).unwrap();
    months
        .supplement(&input, &mut traffic, &ways, &runways, &out)
        .unwrap();
    let row: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out.join("operations.jsonl")).unwrap())
            .unwrap();
    assert_eq!(row["sample_days"], json!([0, 0]));
    assert_eq!(row["mixture"], json!(["window", "window"]));
    assert_eq!(row["observed"], json!([14.0, 14.0]));
    assert_eq!(row["added"], json!([14.0, 14.0]));
    assert_eq!(
        traffic.movements[0],
        [0.5 + 14.0 / 90.0, 0.5 + 14.0 / 90.0, 0.0]
    );
    std::fs::remove_dir_all(out).unwrap();
}

#[test]
fn total_only_deficits_add_across_months_with_each_providers_actual_period() {
    let (ways, runways, arrival, departure) = fixture();
    let window = Window {
        baseline_days: vec![
            "2026-01-01".into(),
            "2026-01-02".into(),
            "2026-02-01".into(),
        ],
        increment_days: vec!["2026-01-01".into()],
    };
    let mut months = Months::new(&window);
    months.add(
        "2026-01-01",
        [
            Rolls::new(),
            BTreeMap::from([(arrival, [0.0, 0.0, 1.0]), (departure, [1.0, 0.0, 0.0])]),
        ],
    );
    let mut traffic = Traffic::new(&ways);
    traffic.rolls = months.window_rates();
    traffic.movements[0] = [1.0, 1.0, 0.0];
    let input = input(
        json!([
            {"airport":"COUNTED", "month":"2026-01", "class":"jet", "movements":124, "source":"official"},
            {"airport":"COUNTED", "month":"2026-02", "class":"jet", "movements":112, "source":"official"}
        ]),
        "2026-01",
        "2026-02",
    );
    input.validate(&window).unwrap();
    let out =
        std::env::temp_dir().join(format!("qm-frequency-total-months-{}", std::process::id()));
    std::fs::create_dir_all(&out).unwrap();
    months
        .supplement(&input, &mut traffic, &ways, &runways, &out)
        .unwrap();
    assert_eq!(traffic.movements[0], [2.0, 2.0, 0.0]);
    assert_eq!(traffic.rolls[&arrival], [0.0, 0.0, 2.0]);
    assert_eq!(traffic.rolls[&departure], [2.0, 0.0, 0.0]);
    let rows: Vec<serde_json::Value> = std::fs::read_to_string(out.join("operations.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows[0]["added"], json!([31.0, 31.0]));
    assert_eq!(rows[1]["added"], json!([28.0, 28.0]));
    assert_eq!(rows[1]["mixture"], json!(["month/window", "month/window"]));
    assert_eq!(rows[1]["provider_periods"], json!(["month", "window"]));
    assert_eq!(rows[1]["operation_split"], "flow-balance");
    std::fs::remove_dir_all(out).unwrap();
}
