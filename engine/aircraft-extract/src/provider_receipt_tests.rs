//! Provider-day receipts decide admission: a failing hour or an unrecovered corrupt member makes a day missing.

use super::*;
use crate::trace::{AircraftTrace, TracePoint};

const DAY: &str = "2026-05-01";
const MIDNIGHT: f64 = 1_777_593_600.0;

fn receipt(source_id: u8, aircraft_per_utc_hour: [u32; 24]) -> ProviderDayReceipt {
    ProviderDayReceipt {
        source_id,
        traces: aircraft_per_utc_hour.iter().copied().max().unwrap().into(),
        corrupt_members: Vec::new(),
        aircraft_per_utc_hour: aircraft_per_utc_hour.to_vec(),
    }
}

fn day(name: &str, primary: ProviderDayReceipt, secondary: Option<ProviderDayReceipt>) -> DayReceipt {
    DayReceipt {
        day: name.into(),
        primary: Some(primary),
        secondary,
        merge: MergeCounts::default(),
    }
}

fn days(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|d| d.to_string()).collect()
}

#[test]
fn aircraft_per_hour_counts_each_trace_once_per_utc_hour_of_the_day() {
    let point = |offset: f64| TracePoint {
        timestamp: MIDNIGHT + offset,
        lat: 50.0,
        lon: 14.0,
        alt_ft: 1000.0,
        speed_kt: 100.0,
        track_deg: 0.0,
        baro_rate_fpm: 0.0,
        flags: 0,
    };
    let traces = vec![
        AircraftTrace {
            icao24: "4ca001".into(),
            aircraft_type: "B738".into(),
            // Two samples in hour 0, one in hour 5, one on the next day.
            points: vec![point(10.0), point(20.0), point(5.0 * 3600.0), point(86_400.0 + 5.0)],
            callsigns: Vec::new(),
        },
        AircraftTrace {
            icao24: "4ca002".into(),
            aircraft_type: "B738".into(),
            points: vec![point(30.0), point(40.0)],
            callsigns: Vec::new(),
        },
    ];
    let receipt = ProviderDayReceipt::from_traces(0, DAY, &traces, Vec::new()).unwrap();
    assert_eq!(receipt.aircraft_per_utc_hour[0], 2);
    assert_eq!(receipt.aircraft_per_utc_hour[5], 1);
    assert_eq!(receipt.aircraft_per_utc_hour.iter().sum::<u32>(), 3);
}

/// 2026-08-01 of the W5 receipts: hours after the export broke fall far
/// below the same-hour median, so the day is missing, not a quiet day.
#[test]
fn a_day_with_a_collapsed_hour_or_an_unrecovered_member_is_not_admitted() {
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
        DayReceipt::primary_missing("2026-05-05"),
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
    let increment = days(&["2026-05-01", "2026-06-01"]);
    let admission = admit(&requested, &increment, &receipts).unwrap();
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
    let window = admission.sampling_window().unwrap();
    assert_eq!((window.baseline_days, window.increment_days), (4, 1));
    assert_eq!(window.baseline_days_sha256, day_list_sha256(&admission.baseline_days));
}

#[test]
fn unreceipted_days_and_foreign_increment_candidates_are_refused() {
    let requested = days(&["2026-05-01"]);
    let missing: BTreeMap<String, DayReceipt> =
        [("2026-05-01".to_string(), DayReceipt::primary_missing("2026-05-01"))].into();
    assert!(admit(&requested, &days(&["2026-06-01"]), &missing).is_err());
    // A requested day without a receipt is incomplete work.
    let error = admit(&requested, &BTreeSet::new(), &BTreeMap::new()).unwrap_err();
    assert!(error.to_string().contains("no provider receipt"), "{error}");
    // Only missing days: nothing to normalise by.
    assert!(admit(&requested, &BTreeSet::new(), &missing).is_err());
}

/// A merged increment candidate that fails admission must be rewritten
/// primary-only — unless its merge kept no secondary content, in which case
/// the flights already equal a primary-only run and the rewrite is skipped.
#[test]
fn rejected_merges_need_a_primary_only_rewrite() {
    let merged = |kept: u64, addresses: u64| DayReceipt {
        day: DAY.into(),
        primary: Some(receipt(0, [1000; 24])),
        secondary: Some(receipt(2, [100; 24])),
        merge: MergeCounts {
            secondary_points_kept: kept,
            secondary_only_addresses: addresses,
            ..MergeCounts::default()
        },
    };
    let rejected = BTreeSet::new();
    assert!(merged(10, 0).needs_primary_only_rewrite(&rejected));
    assert!(merged(0, 1).needs_primary_only_rewrite(&rejected));
    assert!(!merged(0, 0).needs_primary_only_rewrite(&rejected));
    // Admitted increment days and primary-only days are never rewritten.
    assert!(!merged(10, 1).needs_primary_only_rewrite(&days(&[DAY])));
    let mut primary_only = merged(10, 1);
    primary_only.secondary = None;
    assert!(!primary_only.needs_primary_only_rewrite(&rejected));
    assert!(!DayReceipt::primary_missing(DAY).needs_primary_only_rewrite(&rejected));
}

#[test]
fn secondary_rows_count_only_on_increment_days() {
    let mut segment = FlightSegment::airborne_fixture(1, 50.0, 14.0);
    let increment = AdmittedDay {
        segments: "2026-05-01.arrow".into(),
        increment: true,
    };
    let baseline = AdmittedDay {
        increment: false,
        ..increment.clone()
    };
    assert!(increment.keeps(&segment) && baseline.keeps(&segment));
    segment.flags |= crate::flight::segment_flags::SECONDARY_ONLY;
    assert!(increment.keeps(&segment) && !baseline.keeps(&segment));
}
