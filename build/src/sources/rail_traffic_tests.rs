//! Regressions for per-run railway traffic and separately identified missing-calendar estimates.
use super::*;
/// Guessed heavy rail counts take their country's factors; timetable rows, trams and
/// countries without a factor keep theirs.
#[test]
fn guessed_trains_follow_the_countrys_train_km() {
    let tables = SourceTables::test_fixture();
    let off = Tier::Off;
    assert_eq!(
        train_scales(
            tables.train_scales.get(*b"MK"),
            RailType::Rail,
            [9013, 0],
            off
        ),
        [0.082, 0.048]
    );
    assert_eq!(
        train_scales(
            tables.train_scales.get(*b"NO"),
            RailType::Rail,
            [0, 100],
            off
        ),
        [0.265, 1.0]
    );
    assert_eq!(
        train_scales(tables.train_scales.get(*b"RO"), RailType::Tram, [0, 0], off),
        [1.0, 1.0]
    );
    assert_eq!(
        train_scales(tables.train_scales.get(*b"CH"), RailType::Rail, [0, 0], off),
        [1.0, 1.0]
    );
    // The EU27 weigh their freight by tier: a German corridor carries 8 times a line off the
    // network, a timetable row keeps its passengers.
    let [corridor, _] = train_scales(
        tables.train_scales.get(*b"DE"),
        RailType::Rail,
        [0, 9864],
        Tier::Corridor,
    );
    let [off_network, passenger] = train_scales(
        tables.train_scales.get(*b"DE"),
        RailType::Rail,
        [0, 9864],
        off,
    );
    assert!((corridor / off_network - 8.0).abs() < 1e-9 && passenger == 1.0);
    assert!((corridor - 0.547 * 4.0).abs() < 1e-9);
}

/// The EBA split sums to one and puts 36.8 % of EU freight in the END night.
#[test]
fn eu_freight_runs_by_the_counted_split() {
    let tables = SourceTables::test_fixture();
    assert!((tables.freight_period_shares.iter().sum::<f64>() - 1.0).abs() < 1e-12);
    let (day, night) = (0.624 / 16.0, 0.376 / 8.0);
    let expected = [12.0 * day, 3.0 * day + night, 7.0 * night + day];
    for (share, expected) in tables.freight_period_shares.iter().zip(expected) {
        assert!((share - expected).abs() < 0.001, "{share} {expected}");
    }
}

#[test]
fn calendar_estimates_keep_the_same_source_scaling_as_the_observed_trains() {
    let tables = SourceTables::test_fixture();
    let trains = Trains {
        passenger: [40.0, 0.0, 0.0],
        freight: [16.0, 0.0, 0.0],
        missing_calendar_estimate: [[30.0, 0.0, 0.0], [10.0, 0.0, 0.0]],
        status: [2, 2],
        source: [100, 9863],
    };
    let (passenger, freight, shares) = trains.scaled(
        tables,
        RailType::Rail,
        *b"MK",
        Tier::Off,
        FreightRegion::Europe,
    );
    assert_eq!(passenger, [40.0, 0.0, 0.0]);
    assert_eq!(freight, [16.0, 0.0, 0.0]);
    assert_eq!(shares, [0.75, 0.625]);
}

#[test]
fn guessed_freight_preserves_legacy_scaling_before_period_redistribution() {
    let tables = SourceTables::test_fixture();
    let trains = Trains {
        passenger: [0.0; PERIODS],
        freight: [16.1, 2.3, 0.7],
        missing_calendar_estimate: [[0.0; PERIODS]; 2],
        status: [0, 0],
        source: [0, 0],
    };
    let scale = train_scales(
        tables.train_scales.get(*b"DE"),
        RailType::Rail,
        [0, 0],
        Tier::Corridor,
    )[0];
    let legacy_total = trains
        .freight
        .map(|value| value * scale)
        .iter()
        .sum::<f64>();
    let expected = tables
        .freight_period_shares
        .map(|share| share * legacy_total);
    let redistributed_first = tables
        .freight_period_shares
        .map(|share| share * trains.freight.iter().sum::<f64>() * scale);
    assert_ne!(expected, redistributed_first);
    let (_, actual, _) = trains.scaled(
        tables,
        RailType::Rail,
        *b"DE",
        Tier::Corridor,
        FreightRegion::Europe,
    );
    assert_eq!(actual, expected);
}

/// Globally resolved sections keep their own record in `run_trains` (protecting borders,
/// timetables and same-source passenger splits), while unflagged rows still smooth by `rail_type`
/// alone so uncrossed short bridges keep their run's median.
#[test]
fn run_trains_skips_median_only_for_globally_resolved_sections() {
    use arrow_array::{ArrayRef, Float64Array, Int32Array, UInt8Array, UInt16Array};
    use arrow_schema::{Field, Schema};
    use std::collections::HashMap;
    use std::sync::Arc;

    let g = 2 | MATCHING_GLOBAL_SECTION;
    // Rows 0-2 (Swanton): US FRA flagged (0/2) meets CA unflagged prior (30/5) at x=2000.
    // Rows 3-5 (Shore Line): US Amtrak+FRA flagged (1.5/2.5) meets US FRA-only flagged (0/4).
    // Rows 6-8 (Tacoma): same-source flagged split (16/4) beside two longer (0/8) flagged rows.
    // Rows 9-11 (Úvaly): unflagged 2026 bridge residuals (9863/9863, 2/1 and 0/0, 80/13.5)
    //                    between long timetabled approaches (110/0, 123/4.5) take the run median.
    // Rows 12-13 (Horns): touching `rail_type == 6` horn pieces stamped `g` (e.g. 511031K 2/d
    //                     touching 258134P 16.5/d) keep their own crossing rates.
    let columns: Vec<(&str, ArrayRef)> = vec![
        (
            "start_gx",
            Arc::new(Int32Array::from(vec![
                0, 1000, 2000, 3000, 4000, 5000, 10000, 11000, 11100, 20000, 21000, 21050, 30000,
                30500,
            ])),
        ),
        ("start_gy", Arc::new(Int32Array::from(vec![0; 14]))),
        (
            "end_gx",
            Arc::new(Int32Array::from(vec![
                1000, 2000, 3000, 4000, 5000, 6000, 11000, 11100, 12100, 21000, 21050, 21100,
                30500, 31500,
            ])),
        ),
        ("end_gy", Arc::new(Int32Array::from(vec![0; 14]))),
        (
            "rail_type",
            Arc::new(UInt8Array::from(vec![
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 6, 6,
            ])),
        ),
        ("traffic_mode", Arc::new(UInt8Array::from(vec![0u8; 14]))),
        (
            "freight_matching",
            Arc::new(UInt8Array::from(vec![
                g, g, 0, g, g, g, g, g, g, 0, 0, 0, g, g,
            ])),
        ),
        (
            "passenger_status",
            Arc::new(UInt8Array::from(vec![2u8; 14])),
        ),
        ("freight_status", Arc::new(UInt8Array::from(vec![2u8; 14]))),
        (
            "passenger_source_id",
            Arc::new(UInt16Array::from(vec![
                111, 111, 0, 2079, 111, 111, 111, 111, 111, 110, 9863, 0, 111, 111,
            ])),
        ),
        (
            "freight_source_id",
            Arc::new(UInt16Array::from(vec![
                111, 111, 0, 111, 111, 111, 111, 111, 111, 0, 9863, 0, 0, 0,
            ])),
        ),
        (
            "trains_passenger_day",
            Arc::new(Float64Array::from(vec![
                0.0, 0.0, 30.0, 1.5, 0.0, 0.0, 0.0, 16.0, 0.0, 123.0, 2.0, 80.0, 2.0, 16.5,
            ])),
        ),
        (
            "trains_passenger_evening",
            Arc::new(Float64Array::from(vec![0.0; 14])),
        ),
        (
            "trains_passenger_night",
            Arc::new(Float64Array::from(vec![0.0; 14])),
        ),
        (
            "trains_freight_day",
            Arc::new(Float64Array::from(vec![
                2.0, 2.0, 5.0, 2.5, 4.0, 4.0, 8.0, 4.0, 8.0, 4.5, 1.0, 13.5, 0.0, 0.0,
            ])),
        ),
        (
            "trains_freight_evening",
            Arc::new(Float64Array::from(vec![0.0; 14])),
        ),
        (
            "trains_freight_night",
            Arc::new(Float64Array::from(vec![0.0; 14])),
        ),
    ];
    let fields: Vec<Field> = columns
        .iter()
        .map(|(name, arr)| Field::new(*name, arr.data_type().clone(), false))
        .collect();
    let batch = RecordBatch::try_new(
        Arc::new(Schema::new(fields)),
        columns.into_iter().map(|(_, arr)| arr).collect(),
    )
    .unwrap();
    let table = Table {
        batches: vec![batch],
        metadata: HashMap::new(),
    };
    let resolved = run_trains(&table, &[true; 14]).unwrap();
    // Swanton: CA row keeps 30/5 and source 0 instead of inheriting US FRA 0/2.
    assert_eq!(resolved[2].source, [0, 0]);
    assert_eq!(resolved[2].passenger[0], 30.0);
    assert_eq!(resolved[2].freight[0], 5.0);
    // Shore Line: Amtrak row keeps 1.5/2.5 and source [2079, 111] instead of FRA-only 0/4.
    assert_eq!(resolved[3].source, [2079, 111]);
    assert_eq!(resolved[3].passenger[0], 1.5);
    assert_eq!(resolved[3].freight[0], 2.5);
    // Tacoma: same-source [111, 111] flagged split keeps 16/4 instead of 0/8.
    assert_eq!(resolved[7].source, [111, 111]);
    assert_eq!(resolved[7].passenger[0], 16.0);
    assert_eq!(resolved[7].freight[0], 4.0);
    // Úvaly: unflagged bridges across source changes still take the long approach's 123/4.5.
    for idx in [9, 10, 11] {
        assert_eq!(resolved[idx].source, [110, 0]);
        assert_eq!(resolved[idx].passenger[0], 123.0);
        assert_eq!(resolved[idx].freight[0], 4.5);
    }
    // Horns: touching crossings keep their own sounding rates (2/d vs 16.5/d).
    assert_eq!(resolved[12].passenger[0], 2.0);
    assert_eq!(resolved[13].passenger[0], 16.5);
}

/// `run_trains` suppresses guessed freight (and normalizes its provenance to estimated zero) on
/// ineligible rows while keeping observed freight, protects conventional freight connectors joined
/// to high-speed-vetoed runs, and keeps untagged pieces inside `traffic_mode == 1` runs at zero.
#[test]
fn run_trains_respects_freight_eligibility_across_joined_runs() {
    use arrow_array::{ArrayRef, Float64Array, Int32Array, UInt8Array, UInt16Array};
    use arrow_schema::{Field, Schema};
    use std::collections::HashMap;
    use std::sync::Arc;

    // Run A (rows 0-1): short mode=1 joined to long mode=0 with guessed freight (source 9793).
    // Run B (rows 2-3): short mode=1 joined to long mode=0 with observed freight (source 111).
    // Run C (rows 4-5): long 300 km/h mode=0 (0 freight) joined to short 80 km/h mode=0 (20 frt).
    // Run D (rows 6-7): long mode=1 with legacy guessed freight (7.5, source 9644, cal_est 2.0)
    //                   joined to short untagged mode=0 (20 frt): neither may inherit 9644.
    let columns: Vec<(&str, ArrayRef)> = vec![
        (
            "start_gx",
            Arc::new(Int32Array::from(vec![
                0, 100, 10000, 10100, 20000, 22000, 30000, 32000,
            ])),
        ),
        ("start_gy", Arc::new(Int32Array::from(vec![0; 8]))),
        (
            "end_gx",
            Arc::new(Int32Array::from(vec![
                100, 2100, 10100, 12100, 22000, 22200, 32000, 32200,
            ])),
        ),
        ("end_gy", Arc::new(Int32Array::from(vec![0; 8]))),
        ("rail_type", Arc::new(UInt8Array::from(vec![0u8; 8]))),
        (
            "traffic_mode",
            Arc::new(UInt8Array::from(vec![1, 0, 1, 0, 0, 0, 1, 0])),
        ),
        ("passenger_status", Arc::new(UInt8Array::from(vec![1u8; 8]))),
        ("freight_status", Arc::new(UInt8Array::from(vec![1u8; 8]))),
        (
            "passenger_source_id",
            Arc::new(UInt16Array::from(vec![100u16; 8])),
        ),
        (
            "freight_source_id",
            Arc::new(UInt16Array::from(vec![0, 9793, 0, 111, 0, 0, 9644, 0])),
        ),
        (
            "trains_passenger_day",
            Arc::new(Float64Array::from(vec![
                30.0, 50.0, 20.0, 40.0, 60.0, 10.0, 80.0, 40.0,
            ])),
        ),
        (
            "trains_passenger_evening",
            Arc::new(Float64Array::from(vec![0.0; 8])),
        ),
        (
            "trains_passenger_night",
            Arc::new(Float64Array::from(vec![0.0; 8])),
        ),
        (
            "trains_freight_day",
            Arc::new(Float64Array::from(vec![
                0.0, 10.0, 0.0, 12.0, 0.0, 20.0, 7.5, 20.0,
            ])),
        ),
        (
            "trains_freight_evening",
            Arc::new(Float64Array::from(vec![0.0; 8])),
        ),
        (
            "trains_freight_night",
            Arc::new(Float64Array::from(vec![0.0; 8])),
        ),
        (
            "passenger_missing_calendar_estimate_day",
            Arc::new(Float64Array::from(vec![0.0; 8])),
        ),
        (
            "passenger_missing_calendar_estimate_evening",
            Arc::new(Float64Array::from(vec![0.0; 8])),
        ),
        (
            "passenger_missing_calendar_estimate_night",
            Arc::new(Float64Array::from(vec![0.0; 8])),
        ),
        (
            "freight_missing_calendar_estimate_day",
            Arc::new(Float64Array::from(vec![
                0.0, 3.0, 0.0, 4.0, 0.0, 0.0, 2.0, 0.0,
            ])),
        ),
        (
            "freight_missing_calendar_estimate_evening",
            Arc::new(Float64Array::from(vec![0.0; 8])),
        ),
        (
            "freight_missing_calendar_estimate_night",
            Arc::new(Float64Array::from(vec![0.0; 8])),
        ),
    ];
    let fields: Vec<Field> = columns
        .iter()
        .map(|(name, arr)| Field::new(*name, arr.data_type().clone(), false))
        .collect();
    let batch = RecordBatch::try_new(
        Arc::new(Schema::new(fields)),
        columns.into_iter().map(|(_, arr)| arr).collect(),
    )
    .unwrap();
    let table = Table {
        batches: vec![batch],
        metadata: HashMap::new(),
    };
    let takes_freight = [false, true, false, true, false, true, false, true];
    let resolved = run_trains(&table, &takes_freight).unwrap();

    // Run A: mode=1 keeps run's 50 pax, drops guessed freight 9793 and normalizes provenance.
    assert_eq!(resolved[0].passenger[0], 50.0);
    assert_eq!(resolved[0].freight, [0.0; 3]);
    assert_eq!(resolved[0].missing_calendar_estimate[1], [0.0; 3]);
    assert_eq!((resolved[0].status[1], resolved[0].source[1]), (2, 0));
    assert_eq!(resolved[1].freight[0], 10.0);
    assert_eq!(resolved[1].source[1], 9793);

    // Run B: mode=1 keeps observed freight (source 111) intact.
    assert_eq!(resolved[2].freight[0], 12.0);
    assert_eq!(resolved[2].source[1], 111);
    assert_eq!(resolved[2].missing_calendar_estimate[1][0], 4.0);

    // Run C: conventional connector keeps its 20 freight while high-speed row stays at 0.
    assert_eq!(resolved[4].passenger[0], 60.0);
    assert_eq!(resolved[4].freight, [0.0; 3]);
    assert_eq!(resolved[5].passenger[0], 60.0);
    assert_eq!(resolved[5].freight[0], 20.0);

    // Run D: mode=1 median carrying a legacy guessed proxy (9644) clears its guessed freight
    // before copying, so neither the mode=1 row nor its untagged mode=0 neighbour inherits 9644.
    for idx in [6, 7] {
        assert_eq!(resolved[idx].passenger[0], 80.0);
        assert_eq!(resolved[idx].freight, [0.0; 3]);
        assert_eq!(resolved[idx].missing_calendar_estimate[1], [0.0; 3]);
        assert_eq!((resolved[idx].status[1], resolved[idx].source[1]), (2, 0));
    }
}
