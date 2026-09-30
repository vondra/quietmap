//! Receipts decide admission: a collapsed hour or an unrecovered corrupt member makes a day
//! missing, never a quiet day.

use super::*;
use crate::aircraft::trace::TracePoint;

const MIDNIGHT: i64 = 1_777_593_600;

fn receipt(source_id: u8, aircraft_per_utc_hour: [u32; 24]) -> ProviderDayReceipt {
    ProviderDayReceipt {
        source_id,
        traces: u64::from(*aircraft_per_utc_hour.iter().max().unwrap()),
        corrupt_members: Vec::new(),
        aircraft_per_utc_hour: aircraft_per_utc_hour.to_vec(),
    }
}

fn day(
    name: &str,
    primary: ProviderDayReceipt,
    secondary: Option<ProviderDayReceipt>,
) -> DayReceipt {
    DayReceipt {
        day: name.into(),
        primary: Some(primary),
        secondary,
        merge: MergeCounts::default(),
        scope: None,
    }
}

fn days(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|d| d.to_string()).collect()
}

#[test]
fn each_trace_counts_once_per_utc_hour_of_the_day() {
    let point = |offset: f64| TracePoint {
        timestamp: MIDNIGHT as f64 + offset,
        lat: 50.0,
        lon: 14.0,
        altitude_ft: 1000.0,
        geometric_altitude_ft: f32::NAN,
        ground_speed_kt: 100.0,
        track_deg: 0.0,
        vertical_rate_fpm: 0.0,
        flags: 0,
    };
    let trace = |address: &str, offsets: &[f64]| AircraftTrace {
        address: address.into(),
        aircraft_type: "B738".into(),
        emitter_category: 0,
        points: offsets.iter().map(|&o| point(o)).collect(),
        callsigns: Vec::new(),
    };
    let traces = [
        trace("4ca001", &[10.0, 20.0, 5.0 * 3600.0, 86_405.0]),
        trace("4ca002", &[30.0, 40.0]),
    ];
    let receipt = ProviderDayReceipt::of(0, MIDNIGHT, &traces, Vec::new());
    assert_eq!(
        (
            receipt.aircraft_per_utc_hour[0],
            receipt.aircraft_per_utc_hour[5]
        ),
        (2, 1)
    );
    assert_eq!(receipt.aircraft_per_utc_hour.iter().sum::<u32>(), 3);
}

#[test]
fn a_collapsed_hour_or_an_unrecovered_member_is_not_admitted() {
    let normal = [1000; 24];
    let mut broken = normal;
    broken[16..].fill(70);
    let mut corrupt = receipt(0, normal);
    corrupt.corrupt_members.push(CorruptMember {
        member: "traces/42/trace_full_872742.json".into(),
        error: "invalid distance code".into(),
        recovered: false,
    });
    let mut recovered = corrupt.clone();
    recovered.corrupt_members[0].recovered = true;
    let receipts: BTreeMap<String, DayReceipt> = [
        day("2026-05-01", receipt(0, normal), Some(receipt(2, normal))),
        day("2026-05-02", receipt(0, broken), None),
        day("2026-05-03", corrupt, None),
        day("2026-05-04", recovered, None),
        day("2026-06-01", receipt(0, normal), Some(receipt(2, broken))),
        day("2026-06-02", receipt(0, normal), None),
        DayReceipt::primary_missing("2026-05-05", None),
    ]
    .into_iter()
    .map(|r| (r.day.clone(), r))
    .collect();
    let requested = days(&[
        "2026-05-01",
        "2026-05-02",
        "2026-05-03",
        "2026-05-04",
        "2026-05-05",
        "2026-06-01",
        "2026-06-02",
    ]);
    let admission = admit(&requested, &days(&["2026-05-01", "2026-06-01"]), &receipts).unwrap();
    assert_eq!(
        admission.baseline_days,
        days(&["2026-05-01", "2026-05-04", "2026-06-01", "2026-06-02"])
    );
    assert_eq!(admission.increment_days, days(&["2026-05-01"]));
    assert_eq!(admission.primary["2026-05-05"], ProviderDayStatus::Missing);
    assert!(matches!(
        admission.primary["2026-05-02"],
        ProviderDayStatus::Partial { .. }
    ));
    assert!(matches!(
        admission.secondary["2026-06-01"],
        ProviderDayStatus::Partial { .. }
    ));
}

#[test]
fn unreceipted_days_and_foreign_increment_candidates_are_refused() {
    let requested = days(&["2026-05-01"]);
    let missing: BTreeMap<String, DayReceipt> = [(
        "2026-05-01".to_string(),
        DayReceipt::primary_missing("2026-05-01", None),
    )]
    .into();
    assert!(admit(&requested, &days(&["2026-06-01"]), &missing).is_err());
    assert!(
        admit(&requested, &BTreeSet::new(), &BTreeMap::new())
            .unwrap_err()
            .contains("no receipt")
    );
    assert!(
        admit(&requested, &BTreeSet::new(), &missing).is_err(),
        "nothing to normalise by"
    );
}

/// A merged day whose secondary was not admitted is rewritten primary-only, unless its merge kept
/// nothing (then it already is), which makes the repair idempotent.
#[test]
fn rejected_merges_need_a_primary_only_rewrite() {
    let merged = |kept: u64, addresses: u64| DayReceipt {
        merge: MergeCounts {
            secondary_points_kept: kept,
            secondary_only_addresses: addresses,
            ..MergeCounts::default()
        },
        ..day(
            "2026-05-01",
            receipt(0, [1000; 24]),
            Some(receipt(2, [100; 24])),
        )
    };
    let none = BTreeSet::new();
    assert!(merged(10, 0).needs_primary_only_rewrite(&none));
    assert!(merged(0, 1).needs_primary_only_rewrite(&none));
    assert!(!merged(0, 0).needs_primary_only_rewrite(&none));
    assert!(!merged(10, 1).needs_primary_only_rewrite(&days(&["2026-05-01"])));
    assert!(!DayReceipt::primary_missing("2026-05-01", None).needs_primary_only_rewrite(&none));
}
