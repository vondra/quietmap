//! Provider union: a duplicate provider adds nothing, gaps fill from the secondary, `~` echoes of
//! address tracks go and hand their baseline provenance over.

use super::*;

/// An eastbound track at 2,000 ft: one sample per time, 0.001 degree of longitude per 5 s.
fn track(address: &str, times: &[f64], lat: f32) -> AircraftTrace {
    AircraftTrace {
        address: address.into(),
        aircraft_type: "B738".into(),
        emitter_category: 0,
        points: times
            .iter()
            .map(|&t| TracePoint {
                timestamp: 1_700_000_000.0 + t,
                lat,
                lon: 14.0 + (t / 5.0) as f32 * 0.001,
                altitude_ft: 2000.0,
                geometric_altitude_ft: f32::NAN,
                ground_speed_kt: 200.0,
                track_deg: 90.0,
                vertical_rate_fpm: 0.0,
                flags: 0,
            })
            .collect(),
        callsigns: vec![CallsignChange {
            point_index: 0,
            callsign: format!("CS{address}"),
        }],
    }
}

fn every(from: f64, to: f64, step: f64) -> Vec<f64> {
    (0..)
        .map(|k| from + step * f64::from(k))
        .take_while(|&t| t < to)
        .collect()
}

fn primary_day() -> Vec<AircraftTrace> {
    vec![
        track("4ca001", &every(0.0, 600.0, 5.0), 50.0),
        track("4ca002", &[0.0, 10.0, 20.0, 300.0, 310.0], 50.2),
    ]
}

/// Address, (time bits, flags) per sample, callsign changes.
type TraceBits = (String, Vec<(u64, u8)>, Vec<(usize, String)>);

fn snapshot(traces: &[AircraftTrace]) -> Vec<TraceBits> {
    let mut out: Vec<_> = traces
        .iter()
        .map(|t| {
            (
                t.address.clone(),
                t.points
                    .iter()
                    .map(|p| (p.timestamp.to_bits(), p.flags))
                    .collect(),
                t.callsigns
                    .iter()
                    .map(|c| (c.point_index, c.callsign.clone()))
                    .collect(),
            )
        })
        .collect();
    out.sort();
    out
}

#[test]
fn a_copy_of_the_primary_provider_keeps_no_sample() {
    let (alone, _) = merge_providers(primary_day(), Vec::new());
    let (merged, counts) = merge_providers(primary_day(), primary_day());
    assert_eq!(snapshot(&merged), snapshot(&alone));
    assert_eq!(
        (
            counts.secondary_points_kept,
            counts.secondary_only_addresses
        ),
        (0, 0)
    );
    assert!(counts.secondary_points_covered > 0);
}

/// Samples beyond +-1 s of every primary sample survive Stage 0, flagged, in time order; every
/// primary sample stays unflagged.
#[test]
fn secondary_samples_outside_one_second_are_kept() {
    let secondary = vec![
        track("4ca001", &every(2.5, 630.0, 5.0), 50.0),
        track("4ca002", &every(0.0, 320.0, 10.0), 50.2),
    ];
    let (merged, counts) = merge_providers(primary_day(), secondary);
    let kept = |address: &str| -> Vec<f64> {
        let trace = merged.iter().find(|t| t.address == address).unwrap();
        assert!(
            trace
                .points
                .windows(2)
                .all(|pair| pair[0].timestamp < pair[1].timestamp)
        );
        trace
            .points
            .iter()
            .filter(|p| p.is_secondary())
            .map(|p| p.timestamp - 1_700_000_000.0)
            .collect()
    };
    assert_eq!(kept("4ca001"), every(2.5, 630.0, 5.0));
    let expected: Vec<f64> = every(0.0, 320.0, 10.0)
        .into_iter()
        .filter(|t| ![0.0, 10.0, 20.0, 300.0, 310.0].contains(t))
        .collect();
    assert_eq!(kept("4ca002"), expected);
    assert_eq!(
        counts.secondary_points_kept as usize,
        every(2.5, 630.0, 5.0).len() + expected.len()
    );
}

#[test]
fn an_address_only_the_secondary_saw_is_kept_whole_and_flagged() {
    let (merged, counts) = merge_providers(
        primary_day(),
        vec![track("3c6444", &every(0.0, 100.0, 5.0), 48.0)],
    );
    let added = merged.iter().find(|t| t.address == "3c6444").unwrap();
    assert_eq!(added.points.len(), 20);
    assert!(added.points.iter().all(TracePoint::is_secondary));
    assert_eq!(counts.secondary_only_addresses, 1);
}

#[test]
fn a_callsign_on_a_covered_secondary_sample_reaches_the_next_kept_one() {
    let mut secondary = track("4ca002", &[20.5, 40.0, 50.0], 50.2);
    secondary.callsigns[0].callsign = "LATE".into();
    let (merged, _) = merge_providers(primary_day(), vec![secondary]);
    let merged = merged.iter().find(|t| t.address == "4ca002").unwrap();
    let values: Vec<_> = merged
        .callsigns
        .iter()
        .map(|c| {
            (
                merged.points[c.point_index].timestamp - 1_700_000_000.0,
                c.callsign.as_str(),
            )
        })
        .collect();
    assert_eq!(values, [(0.0, "CS4ca002"), (40.0, "LATE")]);
}

/// A `~` track riding an address track for 60 s or more is that aircraft; shorter or distant
/// coincidences are other aircraft.
#[test]
fn anonymous_echoes_of_an_address_track_are_suppressed() {
    let address = track("4ca010", &every(0.0, 300.0, 5.0), 50.5);
    let echo = track("~4ca010", &every(1.0, 91.0, 3.0), 50.5);
    let short = track("~aa0001", &every(200.0, 240.0, 3.0), 50.5);
    let distant = track("~aa0002", &every(0.0, 300.0, 3.0), 50.6);
    let (merged, counts) = merge_providers(vec![address, echo, short, distant], Vec::new());
    let addresses: Vec<&str> = merged.iter().map(|t| t.address.as_str()).collect();
    assert!(!addresses.contains(&"~4ca010"), "{addresses:?}");
    assert!(addresses.contains(&"~aa0001") && addresses.contains(&"~aa0002"));
    assert_eq!(counts.anonymous_points_suppressed, 30);
}

/// A secondary address track duplicating a primary `~` echo is one observation under two
/// identities: the surviving address samples take the baseline provenance, so the duplicate adds
/// no increment energy (the +2.94 dB double count).
#[test]
fn a_duplicate_under_another_identity_stays_on_the_baseline() {
    let times: Vec<f64> = (0..7).map(|k| 10.0 * f64::from(k)).collect();
    let (merged, counts) = merge_providers(
        vec![track("~4ca011", &times, 50.5)],
        vec![track("4ca011", &times, 50.5)],
    );
    assert_eq!(counts.anonymous_points_suppressed, 7);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].address, "4ca011");
    assert!(merged[0].points.iter().all(|p| !p.is_secondary()));
}
