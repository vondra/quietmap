//! Calendar estimates survive matching, precedence, service tracks and parallel-track allocation.
use super::{TrackEvidence, allocate_over_parallel_tracks, track_share};
use crate::railways::{
    coverage::Coverage,
    encode::Expanded,
    matcher::interval,
    merge::row_evidence,
    model::RailModel,
    movements::{Location, Movement},
    split::{ChildGeom, ChildRow},
    types::{CategoryFlow, RowTraffic},
};

fn model() -> RailModel {
    serde_json::from_value(serde_json::json!({
        "network_bounds":{}, "residuals":{}, "spatial_priors":{},
        "sources":[{"id":1,"rank":1,"edition":1,"owner":"CZ","countries":["CZ"],"residual":false,"whole_line":false}],
        "type_priors":[], "usage_priors":[], "mainline_freight":{},
        "passenger_periods":[1,0,0], "freight_periods":[1,0,0], "urban_periods":[1,0,0],
        "regional_freight_periods":[]
    })).unwrap()
}

fn observation(annual_count: f64, days: &[bool]) -> Movement {
    let mut movement = Movement {
        id: "service".into(),
        source_id: 1,
        country: "CZ".into(),
        passenger: Some(annual_count),
        freight: None,
        passenger_status: 2,
        freight_status: 0,
        matching: 2,
        rail_types: vec![0],
        coverage: Coverage::from_days(days).unwrap(),
        missing_calendar_estimate: [0.0; 2],
        attributes: Default::default(),
        location: Location::Interval {
            square: [256, 256],
            way: 1,
            segment: 0,
            from_m: 0.0,
            to_m: 100.0,
            occurrence: 0,
        },
    };
    movement.estimate_missing_calendar();
    movement.validate().unwrap();
    movement
}

fn prior(count: f64) -> RowTraffic {
    RowTraffic {
        passenger: CategoryFlow {
            periods: [count, 0.0, 0.0],
            status: 2,
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn independent_operators_keep_their_own_calendar_before_any_merge() {
    let model = model();
    let first = observation(10.0, &[true, false, false, false]);
    let second = observation(20.0, &[false, true, true, false]);
    let expected = [
        interval(&first, 1, 0, 0.0, 100.0, 2),
        interval(&second, 1, 0, 0.0, 100.0, 2),
    ];
    let (traffic, foreign) = row_evidence(&model, &expected, 0.0, 100.0, 0, *b"CZ");
    assert_eq!(traffic.passenger.periods, [80.0, 0.0, 0.0]);
    assert_eq!(
        traffic.passenger.missing_calendar_estimate,
        [50.0, 0.0, 0.0]
    );
    // Station pairs and coincident tram stops aggregate before creating intervals.
    let mut combined = first;
    crate::railways::station_pairs::add_counts(&mut combined, &second);
    let (aggregated, _) = row_evidence(
        &model,
        &[interval(&combined, 1, 0, 0.0, 100.0, 2)],
        0.0,
        100.0,
        0,
        *b"CZ",
    );
    assert_eq!(traffic, aggregated);
    let evidence = TrackEvidence {
        model: &model,
        domestic: &[traffic; 2],
        foreign: &[foreign; 2],
        priors: &[prior(200.0); 2],
        modes: &[0, 0],
    };
    let result = track_share(&evidence, *b"CZ", &[0, 1], |row| row.passenger, 0, |_| true);
    assert_eq!(result.periods, [80.0, 0.0, 0.0]);
    assert_eq!(result.missing_calendar_estimate, [50.0, 0.0, 0.0]);
}

#[test]
fn service_tracks_keep_the_same_calendar_estimate_as_running_tracks() {
    let model = model();
    let movement = observation(10.0, &[true, false]);
    let (traffic, foreign) = row_evidence(
        &model,
        &[interval(&movement, 1, 0, 0.0, 100.0, 2)],
        0.0,
        100.0,
        0,
        *b"CZ",
    );
    let mut rows = [Expanded {
        parent: 0,
        child: ChildRow {
            geom: ChildGeom {
                start_gx: 0,
                start_gy: 0,
                end_gx: 1000,
                end_gy: 0,
                length_m: 100.0,
            },
            traffic,
            foreign,
        },
        prior: RowTraffic::default(),
        osm_id: 1,
        corridor: "line".into(),
        rail_type: 0,
        usage: 0,
        service: 1,
        traffic_mode: 0,
        country_iso: *b"CZ",
    }];
    allocate_over_parallel_tracks(
        &model,
        &mut rows,
        crate::osm::geometry::Square { x: 256, y: 256 },
    );
    assert_eq!(rows[0].child.traffic.passenger.periods, [20.0, 0.0, 0.0]);
    assert_eq!(
        rows[0].child.traffic.passenger.missing_calendar_estimate,
        [10.0, 0.0, 0.0]
    );
}

#[test]
fn only_a_complete_calendar_can_replace_the_class_prior_with_annual_silence() {
    let model = model();
    for complete in [false, true] {
        let mut movement = observation(0.0, &[true, complete]);
        assert_eq!(movement.retain_categories_with_annual_evidence(), complete);
        let (traffic, foreign) = row_evidence(
            &model,
            &[interval(&movement, 1, 0, 0.0, 100.0, 2)],
            0.0,
            100.0,
            0,
            *b"CZ",
        );
        let evidence = TrackEvidence {
            model: &model,
            domestic: &[traffic],
            foreign: &[foreign],
            priors: &[prior(80.0)],
            modes: &[0],
        };
        let result = track_share(&evidence, *b"CZ", &[0], |row| row.passenger, 0, |_| true);
        assert_eq!(
            result.periods,
            [if complete { 0.0 } else { 80.0 }, 0.0, 0.0]
        );
        assert_eq!(result.missing_calendar_estimate, [0.0; 3]);
        assert_eq!(result.source_id, u16::from(complete));
    }
    let mut mixed = observation(0.0, &[true, false]);
    mixed.freight = Some(5.0);
    mixed.freight_status = 2;
    mixed.estimate_missing_calendar();
    assert!(mixed.retain_categories_with_annual_evidence());
    assert_eq!((mixed.passenger, mixed.passenger_status), (None, 0));
    assert_eq!((mixed.freight, mixed.freight_status), (Some(5.0), 2));
    assert_eq!(mixed.missing_calendar_estimate, [0.0, 5.0]);
}

#[test]
fn incomplete_track_evidence_competes_with_the_prior_after_calendar_estimation() {
    let model = model();
    for foreign_scope in [false, true] {
        for complete in [false, true] {
            let movement = observation(60.0, &[true, complete]);
            let (observed, _) = row_evidence(
                &model,
                &[interval(&movement, 1, 0, 0.0, 100.0, 2)],
                0.0,
                100.0,
                0,
                *b"CZ",
            );
            let stamped = [observed, RowTraffic::default()];
            let empty = [RowTraffic::default(); 2];
            let evidence = TrackEvidence {
                model: &model,
                domestic: if foreign_scope { &empty } else { &stamped },
                foreign: if foreign_scope { &stamped } else { &empty },
                priors: &[prior(80.0); 2],
                modes: &[0, 0],
            };
            let result = track_share(&evidence, *b"CZ", &[0, 1], |row| row.passenger, 0, |_| true);
            assert_eq!(
                result.periods,
                [if complete { 40.0 } else { 60.0 }, 0.0, 0.0]
            );
            assert_eq!(
                result.missing_calendar_estimate,
                [if complete { 0.0 } else { 30.0 }, 0.0, 0.0]
            );
            assert_eq!(result.source_id, if complete { 0 } else { 1 });
        }
    }
}

#[test]
fn unknown_counts_keep_their_spatial_model_observation() {
    let mut movement = observation(0.0, &[true, false]);
    movement.passenger = None;
    movement.passenger_status = 0;
    movement.location = Location::Region {
        bounds: [-1.0, -1.0, 1.0, 1.0],
    };
    for coverage in [Coverage::default(), movement.coverage] {
        movement.coverage = coverage;
        movement.validate().unwrap();
        assert!(movement.retain_categories_with_annual_evidence());
        assert_eq!((movement.passenger, movement.freight), (None, None));
    }
}

/// A stop's departures stamp every track near it; the line carries each stop's once. Three tracks
/// beside one stop of 300 departures (both directions) take 100 each, where each used to take 300
/// (Stockholm's station sums, New York's three-track lines); two tracks beside a stop for each
/// direction (71 each, San Francisco's Bay Street) take 71 each. Routed trips claim only the
/// track they run on and add up: 100 on each of two tracks is 100 a track.
#[test]
fn each_stop_counts_once_for_its_line_and_routed_trips_add_by_track() {
    let model = model();
    for (count, matching, stops, expected) in [
        (300.0, 0, vec![1, 1, 1], 100.0),
        (71.0, 0, vec![1, 2], 71.0),
        (100.0, 2, vec![0, 0], 100.0),
    ] {
        let movement = observation(count, &[true; 4]);
        let tracks: Vec<(RowTraffic, RowTraffic)> = stops
            .iter()
            .map(|&stop| {
                let mut claim = interval(&movement, 1, 0, 0.0, 100.0, matching);
                claim.stop = stop;
                row_evidence(&model, &[claim], 0.0, 100.0, 0, *b"CZ")
            })
            .collect();
        let members: Vec<usize> = (0..tracks.len()).collect();
        let evidence = TrackEvidence {
            model: &model,
            domestic: &tracks.iter().map(|track| track.0).collect::<Vec<_>>(),
            foreign: &tracks.iter().map(|track| track.1).collect::<Vec<_>>(),
            priors: &vec![prior(40.0); tracks.len()],
            modes: &vec![0; tracks.len()],
        };
        let result = track_share(
            &evidence,
            *b"CZ",
            &members,
            |row| row.passenger,
            0,
            |_| true,
        );
        assert_eq!(result.periods, [expected, 0.0, 0.0], "stops {stops:?}");
    }
}
