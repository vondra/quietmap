//! Provider union: a duplicate provider adds nothing, gaps fill from the secondary, `~` echoes of address tracks go.

use super::*;

fn point(timestamp: f64, lat: f32, lon: f32, alt_ft: f32) -> TracePoint {
    TracePoint {
        timestamp,
        lat,
        lon,
        alt_ft,
        speed_kt: 200.0,
        track_deg: 90.0,
        baro_rate_fpm: 0.0,
        flags: 0,
    }
}

/// An eastbound track at 2,000 ft: one sample per entry of `times`, 0.001°
/// of longitude per 5 s from `lon0`.
fn trace(address: &str, times: &[f64], lat: f32, lon0: f32) -> AircraftTrace {
    AircraftTrace {
        icao24: address.into(),
        aircraft_type: "B738".into(),
        points: times
            .iter()
            .map(|&t| point(1_700_000_000.0 + t, lat, lon0 + (t / 5.0) as f32 * 0.001, 2000.0))
            .collect(),
        callsigns: vec![CallsignChange {
            point_idx: 0,
            value: format!("CS{address}"),
        }],
    }
}

fn every(from: f64, to: f64, step: f64) -> Vec<f64> {
    let mut times = Vec::new();
    let mut t = from;
    while t < to {
        times.push(t);
        t += step;
    }
    times
}

/// Address, sample bits (time, lat, lon, altitude, flags) and callsign transitions.
type TraceBits = (String, Vec<(u64, u32, u32, u32, u8)>, Vec<(usize, String)>);

fn snapshot(traces: &[AircraftTrace]) -> Vec<TraceBits> {
    let mut out: Vec<_> = traces
        .iter()
        .map(|t| {
            (
                t.icao24.clone(),
                t.points
                    .iter()
                    .map(|p| (p.timestamp.to_bits(), p.lat.to_bits(), p.lon.to_bits(), p.alt_ft.to_bits(), p.flags))
                    .collect(),
                t.callsigns
                    .iter()
                    .map(|c| (c.point_idx, c.value.clone()))
                    .collect(),
            )
        })
        .collect();
    out.sort();
    out
}

fn primary_day() -> Vec<AircraftTrace> {
    vec![
        trace("4ca001", &every(0.0, 600.0, 5.0), 50.0, 14.0),
        // A 280 s hole the primary provider cannot bridge (airborne budget 120 s).
        trace("4ca002", &[0.0, 10.0, 20.0, 300.0, 310.0], 50.2, 14.0),
    ]
}

/// The invariant behind "adding a duplicate provider changes nothing": every
/// secondary sample of a copy lies within 1 s of a primary one.
#[test]
fn a_copy_of_the_primary_provider_keeps_no_sample() {
    let (alone, _) = merge_provider_traces(primary_day(), Vec::new());
    let (merged, counts) = merge_provider_traces(primary_day(), primary_day());
    assert_eq!(snapshot(&merged), snapshot(&alone));
    assert_eq!(counts.secondary_points_kept, 0);
    assert_eq!(counts.secondary_only_addresses, 0);
    assert!(counts.secondary_points_covered > 0);
}

/// Stage 0 keeps every secondary sample outside ±1 s of a primary one; gap
/// judgement belongs to Stage 1 with DEM phases (see
/// `segment::suppress_covered_secondary_points`).
#[test]
fn secondary_samples_outside_one_second_survive_stage_0() {
    let secondary = vec![
        // 2.5 s off every primary sample of 4ca001 and 30 s beyond its end.
        trace("4ca001", &every(2.5, 630.0, 5.0), 50.0, 14.0),
        trace("4ca002", &every(0.0, 320.0, 10.0), 50.2, 14.0),
    ];
    let (merged, counts) = merge_provider_traces(primary_day(), secondary);
    let kept = |address: &str| -> Vec<f64> {
        merged
            .iter()
            .find(|t| t.icao24 == address)
            .unwrap()
            .points
            .iter()
            .filter(|p| p.is_secondary_provider())
            .map(|p| p.timestamp - 1_700_000_000.0)
            .collect()
    };
    // 4ca001: 2.5 s off the primary grid — everything survives Stage 0 (Stage 1
    // drops what joinable primary pairs span).
    assert_eq!(kept("4ca001"), every(2.5, 630.0, 5.0));
    // 4ca002: only the samples within 1 s of a primary sample go.
    let expected: Vec<f64> = every(0.0, 320.0, 10.0)
        .into_iter()
        .filter(|t| ![0.0, 10.0, 20.0, 300.0, 310.0].contains(t))
        .collect();
    assert_eq!(kept("4ca002"), expected);
    assert_eq!(
        counts.secondary_points_kept as usize,
        every(2.5, 630.0, 5.0).len() + expected.len()
    );
    for trace in &merged {
        assert!(trace
            .points
            .windows(2)
            .all(|pair| pair[0].timestamp < pair[1].timestamp));
        // Every primary sample survives unflagged.
        let primary = primary_day().into_iter().find(|t| t.icao24 == trace.icao24).unwrap();
        let unflagged: Vec<u64> = trace
            .points
            .iter()
            .filter(|p| !p.is_secondary_provider())
            .map(|p| p.timestamp.to_bits())
            .collect();
        assert_eq!(unflagged, primary.points.iter().map(|p| p.timestamp.to_bits()).collect::<Vec<_>>());
    }
}

#[test]
fn an_address_only_the_secondary_provider_saw_is_kept_whole_and_flagged() {
    let (merged, counts) = merge_provider_traces(
        primary_day(),
        vec![trace("3c6444", &every(0.0, 100.0, 5.0), 48.0, 11.0)],
    );
    let added = merged.iter().find(|t| t.icao24 == "3c6444").unwrap();
    assert_eq!(added.points.len(), 20);
    assert!(added.points.iter().all(TracePoint::is_secondary_provider));
    assert_eq!(added.callsigns[0].value, "CS3c6444");
    assert_eq!(counts.secondary_only_addresses, 1);
}

#[test]
fn a_callsign_announced_on_a_covered_secondary_sample_reaches_the_next_kept_one() {
    let mut secondary = trace("4ca002", &[20.5, 40.0, 50.0], 50.2, 14.0);
    secondary.callsigns = vec![CallsignChange {
        point_idx: 0,
        value: "LATE".into(),
    }];
    let (merged, _) = merge_provider_traces(primary_day(), vec![secondary]);
    let merged = merged.iter().find(|t| t.icao24 == "4ca002").unwrap();
    let values: Vec<_> = merged
        .callsigns
        .iter()
        .map(|c| (merged.points[c.point_idx].timestamp - 1_700_000_000.0, c.value.as_str()))
        .collect();
    assert_eq!(values, [(0.0, "CS4ca002"), (40.0, "LATE")]);
}

/// A `~` (TIS-B) track riding an address track for ≥ 60 s is the same
/// aircraft; a shorter or distant coincidence is another aircraft.
#[test]
fn anonymous_echoes_of_an_address_track_are_suppressed() {
    let address = trace("4ca010", &every(0.0, 300.0, 5.0), 50.5, 14.0);
    let echo = trace("~4ca010", &every(1.0, 91.0, 3.0), 50.5, 14.0);
    let short = trace("~aa0001", &every(200.0, 240.0, 3.0), 50.5, 14.0);
    let distant = trace("~aa0002", &every(0.0, 300.0, 3.0), 50.6, 14.0);
    let (merged, counts) =
        merge_provider_traces(vec![address, echo, short, distant], Vec::new());
    let addresses: Vec<&str> = merged.iter().map(|t| t.icao24.as_str()).collect();
    assert!(!addresses.contains(&"~4ca010"), "{addresses:?}");
    assert!(addresses.contains(&"~aa0001") && addresses.contains(&"~aa0002"));
    assert_eq!(counts.anonymous_points_suppressed, 30);
}

/// Stage 0 never spans gaps: a level FL280 pair looks like Cruise to a
/// barometric eye, but over high terrain Stage 1 classifies it Airborne and
/// will not bridge a 200 s hole — so the secondary sample inside survives.
#[test]
fn stage_0_keeps_secondary_inside_gaps_only_stage_1_can_judge() {
    let mut primary = trace("4ca020", &[0.0, 200.0], 50.0, 14.0);
    for p in &mut primary.points {
        p.alt_ft = 28_000.0;
        p.speed_kt = 450.0;
    }
    let mut secondary = trace("4ca020", &[100.0], 50.0, 14.0);
    secondary.points[0].alt_ft = 28_000.0;
    secondary.points[0].speed_kt = 450.0;
    let (_, counts) = merge_provider_traces(vec![primary], vec![secondary]);
    assert_eq!(counts.secondary_points_kept, 1);
}

/// The echo test also holds across providers: a primary `~` track riding a
/// secondary address track goes once the union holds both.
#[test]
fn a_primary_anonymous_track_riding_a_secondary_address_track_goes() {
    let echo = trace("~4ca011", &every(1.0, 91.0, 3.0), 50.5, 14.0);
    let (merged, counts) = merge_provider_traces(
        vec![echo],
        vec![trace("4ca011", &every(0.0, 300.0, 5.0), 50.5, 14.0)],
    );
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].icao24, "4ca011");
    assert_eq!(counts.anonymous_points_suppressed, 30);
}
