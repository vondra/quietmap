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
    };
    let result = track_share(
        &evidence,
        *b"CZ",
        &[0, 1],
        |row| row.passenger,
        0,
        false,
        |_| true,
    );
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
            resolved_section: None,
        },
        prior: RowTraffic::default(),
        osm_id: 1,
        corridor: "line".into(),
        rail_type: 0,
        usage: 0,
        service: 1,
        traffic_mode: 0,
        takes_guessed_freight: true,
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
        };
        let result = track_share(
            &evidence,
            *b"CZ",
            &[0],
            |row| row.passenger,
            0,
            false,
            |_| true,
        );
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
            };
            let result = track_share(
                &evidence,
                *b"CZ",
                &[0, 1],
                |row| row.passenger,
                0,
                false,
                |_| true,
            );
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
        };
        let result = track_share(
            &evidence,
            *b"CZ",
            &members,
            |row| row.passenger,
            0,
            false,
            |_| true,
        );
        assert_eq!(result.periods, [expected, 0.0, 0.0], "stops {stops:?}");
    }
}

fn freight_prior(count: f64) -> RowTraffic {
    RowTraffic {
        freight: CategoryFlow {
            periods: [count, 0.0, 0.0],
            status: 2,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn model_with_whole_line_sources() -> RailModel {
    serde_json::from_value(serde_json::json!({
        "network_bounds":{}, "residuals":{}, "spatial_priors":{},
        "sources":[
            {"id":1,"rank":1,"edition":1,"owner":"DE","countries":["DE"],"residual":false,"whole_line":false},
            {"id":2021,"rank":2,"edition":1,"owner":"DE","countries":["DE"],"residual":false,"whole_line":true},
            {"id":9793,"rank":3,"edition":1,"owner":"TR","countries":["TR"],"residual":false,"whole_line":true}
        ],
        "type_priors":[],
        "usage_priors":[[80.0, 20.0], [30.0, 5.0], [0.0, 15.0], [40.0, 10.0]],
        "mainline_freight":{},
        "passenger_periods":[1,0,0], "freight_periods":[1,0,0], "urban_periods":[1,0,0],
        "regional_freight_periods":[]
    }))
    .unwrap()
}

/// Guessed freight (both class priors and whole-line guessed proxies like Turkey 9793) divides
/// only among freight-eligible parallel tracks, writing estimated zero on ineligible tracks and
/// never producing NaN when no track is eligible. Observed whole-line freight keeps its physical
/// split across tracks, and unequal-prior competition uses only eligible tracks' priors.
#[test]
fn guessed_freight_divides_only_among_eligible_parallel_tracks() {
    let model = model_with_whole_line_sources();
    let empty = [RowTraffic::default(); 2];

    // 1. Class prior on one ineligible (300 km/h mode=0) and one eligible (300 km/h mode=3) track.
    let prior_evidence = TrackEvidence {
        model: &model,
        domestic: &empty,
        foreign: &empty,
        priors: &[freight_prior(20.0), freight_prior(20.0)],
    };
    let allows_second = |member| member == 1;
    let ineligible = track_share(
        &prior_evidence,
        *b"DE",
        &[0, 1],
        |row| row.freight,
        0,
        true,
        allows_second,
    );
    let eligible = track_share(
        &prior_evidence,
        *b"DE",
        &[0, 1],
        |row| row.freight,
        1,
        true,
        allows_second,
    );
    assert_eq!(ineligible.periods, [0.0, 0.0, 0.0]);
    assert_eq!((ineligible.status, ineligible.source_id), (2, 0));
    assert_eq!(eligible.periods, [20.0, 0.0, 0.0]);
    assert_eq!((eligible.status, eligible.source_id), (2, 0));

    // 2. Named whole-line guessed freight proxy (9793) also divides only among eligible tracks.
    let guessed_proxy = RowTraffic {
        freight: CategoryFlow {
            periods: [20.0, 0.0, 0.0],
            status: 2,
            source_id: 9793,
            ..Default::default()
        },
        ..Default::default()
    };
    let proxy_evidence = TrackEvidence {
        model: &model,
        domestic: &[guessed_proxy, guessed_proxy],
        foreign: &empty,
        priors: &[freight_prior(20.0), freight_prior(20.0)],
    };
    let tr_ineligible = track_share(
        &proxy_evidence,
        *b"TR",
        &[0, 1],
        |row| row.freight,
        0,
        true,
        allows_second,
    );
    let tr_eligible = track_share(
        &proxy_evidence,
        *b"TR",
        &[0, 1],
        |row| row.freight,
        1,
        true,
        allows_second,
    );
    assert_eq!(tr_ineligible.periods, [0.0, 0.0, 0.0]);
    assert_eq!((tr_ineligible.status, tr_ineligible.source_id), (2, 0));
    assert_eq!(tr_eligible.periods, [20.0, 0.0, 0.0]);
    assert_eq!(tr_eligible.source_id, 9793);

    // 3. Observed whole-line source (2021, not guessed) keeps dividing across physical tracks.
    let observed_whole = RowTraffic {
        freight: CategoryFlow {
            periods: [20.0, 0.0, 0.0],
            status: 1,
            source_id: 2021,
            ..Default::default()
        },
        ..Default::default()
    };
    let observed_evidence = TrackEvidence {
        model: &model,
        domestic: &[observed_whole, observed_whole],
        foreign: &empty,
        priors: &[freight_prior(20.0), freight_prior(20.0)],
    };
    for own in [0, 1] {
        let share = track_share(
            &observed_evidence,
            *b"DE",
            &[0, 1],
            |row| row.freight,
            own,
            true,
            allows_second,
        );
        assert_eq!(share.periods, [10.0, 0.0, 0.0]);
        assert_eq!(share.source_id, 2021);
    }

    // 4. Two ineligible tracks (empty capable set): zero estimated freight, never NaN.
    for own in [0, 1] {
        let share = track_share(
            &prior_evidence,
            *b"DE",
            &[0, 1],
            |row| row.freight,
            own,
            true,
            |_| false,
        );
        assert_eq!(share.periods, [0.0, 0.0, 0.0]);
        assert_eq!((share.status, share.source_id), (2, 0));
    }

    // 5. Unequal priors (usage=0 ineligible 20 vs usage=3 eligible 10) with partial domestic
    //    evidence on track 0 only: prior competition uses eligible track 1's 10, so 12 beats 10.
    let partial_12 = RowTraffic {
        freight: CategoryFlow {
            periods: [12.0, 0.0, 0.0],
            status: 1,
            source_id: 1,
            ..Default::default()
        },
        ..Default::default()
    };
    let unequal_evidence = TrackEvidence {
        model: &model,
        domestic: &[partial_12, RowTraffic::default()],
        foreign: &empty,
        priors: &[freight_prior(20.0), freight_prior(10.0)],
    };
    let won = track_share(
        &unequal_evidence,
        *b"DE",
        &[0, 1],
        |row| row.freight,
        1,
        true,
        allows_second,
    );
    assert_eq!(won.periods, [6.0, 0.0, 0.0]);
    assert_eq!(won.source_id, 1);

    let partial_6 = RowTraffic {
        freight: CategoryFlow {
            periods: [6.0, 0.0, 0.0],
            status: 1,
            source_id: 1,
            ..Default::default()
        },
        ..Default::default()
    };
    let fallback_evidence = TrackEvidence {
        model: &model,
        domestic: &[partial_6, RowTraffic::default()],
        foreign: &empty,
        priors: &[freight_prior(20.0), freight_prior(10.0)],
    };
    let fallback = track_share(
        &fallback_evidence,
        *b"DE",
        &[0, 1],
        |row| row.freight,
        1,
        true,
        allows_second,
    );
    assert_eq!(fallback.periods, [10.0, 0.0, 0.0]);
    assert_eq!(fallback.source_id, 0);
}

/// `write::expand_rows` propagates each named high-speed line's median speed onto unposted pieces
/// before parallel-track allocation: an unposted LGV track parallel to a 160 km/h conventional
/// track yields zero freight and leaves the full 20-train freight prior on the conventional track,
/// while still splitting the 80-train passenger prior 40 / 40.
#[test]
fn expand_rows_propagates_high_speed_line_speed_into_parallel_freight_allocation() {
    use arrow_array::{
        ArrayRef, BooleanArray, Float32Array, Int16Array, Int32Array, Int64Array, RecordBatch,
        StringArray, UInt8Array, UInt16Array,
    };
    use std::collections::HashMap;
    use std::sync::Arc;

    let model = model_with_whole_line_sources();
    let square = crate::osm::geometry::Square { x: 256, y: 256 };
    // Square (256, 256) sits just east of 0 deg, just south of 0 deg; z30 pixel ~ 0.037 m at eq.
    let (x0, y0) = crate::osm::geometry::lonlat_to_grid(0.1, -0.1);
    let (x1, y1) = crate::osm::geometry::lonlat_to_grid(0.102, -0.1);
    // Parallel track ~6 m south; separate far-away posted-300 piece of the same LGV line.
    let (px0, py0) = crate::osm::geometry::lonlat_to_grid(0.1, -0.100055);
    let (px1, py1) = crate::osm::geometry::lonlat_to_grid(0.102, -0.100055);
    let (fx0, fy0) = crate::osm::geometry::lonlat_to_grid(0.2, -0.2);
    let (fx1, fy1) = crate::osm::geometry::lonlat_to_grid(0.205, -0.2);

    let batch = RecordBatch::try_from_iter([
        (
            "osm_id",
            Arc::new(Int64Array::from(vec![1, 2, 3])) as ArrayRef,
        ),
        ("segment_idx", Arc::new(Int16Array::from(vec![0, 0, 0]))),
        ("start_gx", Arc::new(Int32Array::from(vec![x0, px0, fx0]))),
        ("start_gy", Arc::new(Int32Array::from(vec![y0, py0, fy0]))),
        ("end_gx", Arc::new(Int32Array::from(vec![x1, px1, fx1]))),
        ("end_gy", Arc::new(Int32Array::from(vec![y1, py1, fy1]))),
        (
            "length_m",
            Arc::new(Float32Array::from(vec![220.0, 220.0, 550.0])),
        ),
        ("rail_type", Arc::new(UInt8Array::from(vec![0, 0, 0]))),
        ("usage", Arc::new(UInt8Array::from(vec![0, 0, 0]))),
        ("service", Arc::new(UInt8Array::from(vec![0, 0, 0]))),
        ("traffic_mode", Arc::new(UInt8Array::from(vec![0, 0, 0]))),
        ("maxspeed", Arc::new(UInt16Array::from(vec![0, 160, 300]))),
        (
            "highspeed",
            Arc::new(BooleanArray::from(vec![true, false, true])),
        ),
        (
            "country_iso",
            Arc::new(UInt16Array::from(vec![u16::from_le_bytes(*b"DE"); 3])),
        ),
        ("ref", Arc::new(StringArray::from(vec!["", "", ""]))),
        (
            "name",
            Arc::new(StringArray::from(vec!["LGV", "LGV", "LGV"])),
        ),
    ])
    .unwrap();

    let expanded = crate::railways::write::expand_rows(
        &model,
        &batch,
        &HashMap::new(),
        &HashMap::new(),
        square,
    )
    .unwrap();
    assert_eq!(expanded.len(), 3);
    assert!(!expanded[0].takes_guessed_freight);
    assert!(expanded[1].takes_guessed_freight);
    assert!(!expanded[2].takes_guessed_freight);
    assert_eq!(
        expanded[0].child.traffic.passenger.periods,
        [40.0, 0.0, 0.0]
    );
    assert_eq!(
        expanded[1].child.traffic.passenger.periods,
        [40.0, 0.0, 0.0]
    );
    assert_eq!(expanded[0].child.traffic.freight.periods, [0.0, 0.0, 0.0]);
    assert_eq!(expanded[1].child.traffic.freight.periods, [20.0, 0.0, 0.0]);
    assert_eq!(expanded[2].child.traffic.freight.periods, [0.0, 0.0, 0.0]);
}

/// When `split_parent` splits a curved high-speed piece at an evidence boundary, `write::expand_rows`
/// weighs the split child chords (matching `table_freight_eligibility` on the encoded table): a
/// 300 km/h curve whose unsplit chord is shorter than a 200 km/h straight piece (~1000 m vs 1100 m)
/// but whose two split chords sum longer (~1414 m) tips the line median to 300 km/h and clears
/// guessed freight on its parallel track before allocation.
#[test]
fn expand_rows_uses_split_curved_child_chords_for_line_speed() {
    use crate::railways::{
        encode::encode_children, freight_eligibility::table_freight_eligibility,
        square_intervals::Interval, topology::Piece,
    };
    use arrow_array::{
        ArrayRef, BooleanArray, Float32Array, Int16Array, Int32Array, Int64Array, RecordBatch,
        StringArray, UInt8Array, UInt16Array,
    };
    use std::collections::HashMap;
    use std::sync::Arc;

    let model = model_with_whole_line_sources();
    let square = crate::osm::geometry::Square { x: 256, y: 256 };
    // Row 0 (unposted LGV) parallel to Row 1 (160 km/h conventional twin).
    let (x0, y0) = crate::osm::geometry::lonlat_to_grid(0.1, -0.1);
    let (x1, y1) = crate::osm::geometry::lonlat_to_grid(0.102, -0.1);
    let (px0, py0) = crate::osm::geometry::lonlat_to_grid(0.1, -0.100055);
    let (px1, py1) = crate::osm::geometry::lonlat_to_grid(0.102, -0.100055);
    // Row 2: straight 200 km/h piece (~1110 m, 0.010 deg lon at lat -0.2).
    let (s0x, s0y) = crate::osm::geometry::lonlat_to_grid(0.200, -0.2);
    let (s1x, s1y) = crate::osm::geometry::lonlat_to_grid(0.210, -0.2);
    // Row 3: curved 300 km/h V-piece whose unsplit chord is 0.009 deg (~1000 m < 1110 m), while
    // splitting at its apex (0.2145, -0.1955) yields two chords summing ~1414 m (> 1110 m).
    let curve_start = [-0.2, 0.210];
    let curve_apex = [-0.1955, 0.2145];
    let curve_end = [-0.2, 0.219];
    let (c0x, c0y) = crate::osm::geometry::lonlat_to_grid(curve_start[1], curve_start[0]);
    let (c1x, c1y) = crate::osm::geometry::lonlat_to_grid(curve_end[1], curve_end[0]);
    let half_arc = crate::osm::geometry::flat_dist(
        curve_start[0],
        curve_start[1],
        curve_apex[0],
        curve_apex[1],
    );
    let full_arc = half_arc * 2.0;

    let batch = RecordBatch::try_from_iter([
        (
            "osm_id",
            Arc::new(Int64Array::from(vec![1, 2, 3, 4])) as ArrayRef,
        ),
        ("segment_idx", Arc::new(Int16Array::from(vec![0, 0, 0, 0]))),
        (
            "start_gx",
            Arc::new(Int32Array::from(vec![x0, px0, s0x, c0x])),
        ),
        (
            "start_gy",
            Arc::new(Int32Array::from(vec![y0, py0, s0y, c0y])),
        ),
        (
            "end_gx",
            Arc::new(Int32Array::from(vec![x1, px1, s1x, c1x])),
        ),
        (
            "end_gy",
            Arc::new(Int32Array::from(vec![y1, py1, s1y, c1y])),
        ),
        (
            "length_m",
            Arc::new(Float32Array::from(vec![
                220.0,
                220.0,
                1110.0,
                full_arc as f32,
            ])),
        ),
        ("rail_type", Arc::new(UInt8Array::from(vec![0, 0, 0, 0]))),
        ("usage", Arc::new(UInt8Array::from(vec![0, 0, 0, 0]))),
        ("service", Arc::new(UInt8Array::from(vec![0, 0, 0, 0]))),
        ("traffic_mode", Arc::new(UInt8Array::from(vec![0, 0, 0, 0]))),
        (
            "maxspeed",
            Arc::new(UInt16Array::from(vec![0, 160, 200, 300])),
        ),
        (
            "highspeed",
            Arc::new(BooleanArray::from(vec![true, false, true, true])),
        ),
        (
            "country_iso",
            Arc::new(UInt16Array::from(vec![u16::from_le_bytes(*b"DE"); 4])),
        ),
        ("ref", Arc::new(StringArray::from(vec!["", "", "", ""]))),
        (
            "name",
            Arc::new(StringArray::from(vec![
                "CurveLGV", "CurveLGV", "CurveLGV", "CurveLGV",
            ])),
        ),
    ])
    .unwrap();

    let piece = Piece::new(0.0, 0.0, full_arc, vec![curve_start, curve_apex, curve_end]);
    let pieces = HashMap::from([((4i64, 0i16), piece)]);
    let cut_claim = Interval {
        osm_id: 4,
        segment_idx: 0,
        from_m: 0.0,
        to_m: half_arc,
        country: *b"DE",
        source_id: 1,
        passenger: 50.0,
        freight: 0.0,
        passenger_status: 1,
        freight_status: 0,
        matching: 2,
        missing_calendar_estimate: [0.0; 2],
        stop: 0,
        resolved_section: None,
    };
    let intervals = HashMap::from([((4i64, 0i16), vec![cut_claim])]);

    let expanded =
        crate::railways::write::expand_rows(&model, &batch, &intervals, &pieces, square).unwrap();
    // Row 3 split into 2 children -> 5 expanded rows total.
    assert_eq!(expanded.len(), 5);
    assert!(!expanded[0].takes_guessed_freight);
    assert!(expanded[1].takes_guessed_freight);
    assert_eq!(expanded[0].child.traffic.freight.periods, [0.0, 0.0, 0.0]);
    assert_eq!(expanded[1].child.traffic.freight.periods, [20.0, 0.0, 0.0]);

    // Encoded table agrees bit-for-bit with preparation's `takes_guessed_freight`.
    let encoded = encode_children(&batch, &expanded).unwrap();
    let downstream = table_freight_eligibility(&[encoded]).unwrap();
    assert_eq!(
        downstream.takes_freight,
        expanded
            .iter()
            .map(|row| row.takes_guessed_freight)
            .collect::<Vec<_>>()
    );
}
