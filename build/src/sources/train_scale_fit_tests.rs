//! A synthetic two-country network with hand-computed factors.
use super::*;
use crate::sources::rail_traffic::train_scales;
use physics::bands::PERIODS;

const OBSERVED: u16 = 100;
const GUESSED: u16 = 0;
/// Timetabled rows within a tenth of their national train-km.
const SPREAD: Option<f64> = Some(0.1);
/// A row: its length (km), run trains and TEN-T tier.
type Row = (f64, Trains, Tier);

/// A row's run trains a day, all in the day period: (passenger, its source), (freight, its source).
fn trains(
    (passenger, passenger_source): (f64, u16),
    (freight, freight_source): (f64, u16),
) -> Trains {
    let mut day = [0.0; PERIODS];
    day[0] = passenger;
    let mut freight_day = [0.0; PERIODS];
    freight_day[0] = freight;
    Trains {
        passenger: day,
        freight: freight_day,
        missing_calendar_estimate: [[0.0; PERIODS]; 2],
        status: [0, 0],
        source: [passenger_source, freight_source],
    }
}

/// Country AA lies on the TEN-T map: 10 km of corridor, 20 km of network line, 40 km off it and
/// 5 km of counted freight; BB has no TEN-T line.
fn network() -> [(Vec<Row>, NationalTrainKm); 2] {
    let aa = vec![
        (
            10.0,
            trains((20.0, OBSERVED), (10.0, GUESSED)),
            Tier::Corridor,
        ),
        (
            20.0,
            trains((40.0, GUESSED), (10.0, GUESSED)),
            Tier::Network,
        ),
        (40.0, trains((10.0, GUESSED), (10.0, GUESSED)), Tier::Off),
        (5.0, trains((0.0, OBSERVED), (8.0, OBSERVED)), Tier::Off),
    ];
    let bb = vec![
        (30.0, trains((10.0, GUESSED), (4.0, GUESSED)), Tier::Off),
        (10.0, trains((30.0, OBSERVED), (0.0, GUESSED)), Tier::Off),
    ];
    [
        (
            aa,
            NationalTrainKm {
                passenger: Some((2024, 1000.0)),
                freight: Some((2024, 1000.0)),
            },
        ),
        (
            bb,
            NationalTrainKm {
                passenger: Some((2024, 900.0)),
                freight: Some((2024, 60.0)),
            },
        ),
    ]
}

fn mapped(rows: &[Row]) -> MappedTrainKm {
    let mut sum = MappedTrainKm::default();
    for (length_km, trains, tier) in rows {
        sum.add(*length_km, trains, *tier);
    }
    sum
}

/// The sources' own scaling of every row, summed: (passenger, freight) train-km a day.
fn scaled_train_km(rows: &[Row], value: &TrainScales) -> (f64, f64) {
    rows.iter()
        .fold((0.0, 0.0), |(passenger, freight), (km, trains, tier)| {
            let [freight_scale, passenger_scale] = train_scales(
                Some(value),
                RailType::Rail,
                [trains.source[1], trains.source[0]],
                *tier,
            );
            (
                passenger + trains.passenger.iter().sum::<f64>() * passenger_scale * km,
                freight + trains.freight.iter().sum::<f64>() * freight_scale * km,
            )
        })
}

/// AA: passenger observed 200, guessed 800 + 400; freight observed 40, guessed 400 off, 200 on the
/// network, 100 on the corridor. Freight (1000 - 40) / (0.5 * 400 + 2 * 200 + 4 * 100) = 0.96;
/// passengers (1000 - 200) / 1200. BB: freight 60 / 120 = 0.5 unweighted; passengers
/// (900 - 300) / 300 = 2 are never raised.
#[test]
fn two_countries_take_their_hand_computed_factors() {
    let [(aa, aa_national), (bb, bb_national)] = network();
    let aa_mapped = mapped(&aa);
    assert_eq!(aa_mapped.passenger, [200.0, 1200.0]);
    assert_eq!(aa_mapped.freight_observed, 40.0);
    assert_eq!(aa_mapped.freight_guessed, [400.0, 200.0, 100.0]);
    let aa_value = fitted(&aa_mapped, &aa_national, TEN_T_FREIGHT_WEIGHTS, SPREAD);
    assert!((aa_value.scales[0] - 0.96).abs() < 1e-12);
    assert!((aa_value.scales[1] - 800.0 / 1200.0).abs() < 1e-12);
    assert_eq!(aa_value.tier_weights, TEN_T_FREIGHT_WEIGHTS);
    let bb_value = fitted(&mapped(&bb), &bb_national, [1.0; 3], SPREAD);
    assert_eq!(bb_value.scales, [0.5, 1.0]);
    // Scaled as the sources scale them, AA runs its national train-km.
    let (passenger, freight) = scaled_train_km(&aa, &aa_value);
    assert!((passenger - 1000.0).abs() < 1e-9 && (freight - 1000.0).abs() < 1e-9);
}

/// Within the national total a corridor carries 8 times, and a network line 4 times, the guessed
/// freight of a line off the network with the same prior.
#[test]
fn the_ten_t_weights_move_freight_onto_the_corridors() {
    let [(aa, national), _] = network();
    let value = fitted(&mapped(&aa), &national, TEN_T_FREIGHT_WEIGHTS, SPREAD);
    let freight = |tier| train_scales(Some(&value), RailType::Rail, [GUESSED, GUESSED], tier)[0];
    assert!((freight(Tier::Corridor) / freight(Tier::Off) - 8.0).abs() < 1e-12);
    assert!((freight(Tier::Network) / freight(Tier::Off) - 4.0).abs() < 1e-12);
    // Without a published goods total the guessed 700 stay, only moved between tiers.
    let unpublished = NationalTrainKm {
        freight: None,
        ..national
    };
    let kept = fitted(&mapped(&aa), &unpublished, TEN_T_FREIGHT_WEIGHTS, SPREAD);
    assert!((scaled_train_km(&aa, &kept).1 - 740.0).abs() < 1e-9);
}

/// The passenger factor is continuous: timetables that alone reach the national total leave the
/// defaults standing, a residual within the timetables' spread lowers them in proportion, one
/// beyond it sets them; without timetables the national total does. Counted freight above the
/// goods total silences guessed freight.
#[test]
fn the_factors_are_bounded_and_the_passenger_factor_continuous() {
    let mapped = MappedTrainKm {
        passenger: [900.0, 400.0],
        freight_observed: 50.0,
        freight_guessed: [100.0, 0.0, 0.0],
    };
    let factors = |passenger: f64, spread| {
        let national = NationalTrainKm {
            passenger: Some((2024, passenger)),
            freight: Some((2024, 40.0)),
        };
        fitted(&mapped, &national, [1.0; 3], spread).scales
    };
    assert_eq!(factors(800.0, SPREAD), [0.0, 1.0]);
    assert_eq!(factors(900.0, SPREAD)[1], 1.0);
    // Residual 9 is a tenth of the spread's 90: 1 - 0.1 * (1 - 9 / 400).
    assert!((factors(909.0, SPREAD)[1] - (1.0 - 0.1 * (1.0 - 9.0 / 400.0))).abs() < 1e-12);
    assert!((factors(1000.0, SPREAD)[1] - 0.25).abs() < 1e-12);
    assert_eq!(factors(1000.0, None)[1], 1.0, "no measured spread");
    let untimetabled = MappedTrainKm {
        passenger: [0.0, 400.0],
        ..mapped
    };
    let national = NationalTrainKm {
        passenger: Some((2024, 100.0)),
        freight: None,
    };
    assert_eq!(
        fitted(&untimetabled, &national, [1.0; 3], None).scales[1],
        0.25
    );
}

/// The spread is the median deviation of the countries the timetables cover; a country run on
/// defaults or without a passenger total does not count.
#[test]
fn the_timetable_spread_is_measured_where_timetables_cover_the_country() {
    let country = |passenger: [f64; 2], national: Option<f64>| {
        (
            MappedTrainKm {
                passenger,
                ..MappedTrainKm::default()
            },
            NationalTrainKm {
                passenger: national.map(|train_km| (2024, train_km)),
                freight: None,
            },
        )
    };
    let countries = [
        country([100.0, 10.0], Some(110.0)),
        country([50.0, 5.0], Some(40.0)),
        country([10.0, 90.0], Some(50.0)),
        country([200.0, 0.0], Some(190.0)),
        country([300.0, 0.0], None),
    ];
    let spread = timetable_spread(
        countries
            .iter()
            .map(|(mapped, national)| (mapped, national)),
    );
    assert_eq!(spread, Some((100.0_f64 / 110.0 - 1.0).abs()));
    assert_eq!(
        timetable_spread(countries[2..3].iter().map(|(m, n)| (m, n))),
        None
    );
}

/// A square of two 1 km heavy-rail rows, 10 guessed freight trains a day on each: an ordinary line
/// and a line posted 300 km/h, mapped passenger-only by its speed. The sources emit the first row's
/// freight only, so the fit counts 10 train-km a day, not 20: against a national 10 the factor is 1,
/// where counting the high-speed row made it 0.5.
#[test]
fn the_fit_counts_only_the_guessed_freight_the_sources_emit() {
    use arrow_array::{
        ArrayRef, BooleanArray, Float32Array, Float64Array, Int32Array, RecordBatch, StringArray,
        UInt8Array, UInt16Array,
    };
    use std::sync::Arc;
    let rows: [(u16, bool, &str, i32); 2] = [(160, false, "", 0), (300, true, "LGV", 50_000)];
    let u8s = |value: u8| Arc::new(UInt8Array::from(vec![value; 2])) as ArrayRef;
    let u16s = |value: u16| Arc::new(UInt16Array::from(vec![value; 2])) as ArrayRef;
    let f64s = |value: f64| Arc::new(Float64Array::from(vec![value; 2])) as ArrayRef;
    let starts: Vec<i32> = rows.iter().map(|row| row.3).collect();
    let batch = RecordBatch::try_from_iter([
        (
            "start_gx",
            Arc::new(Int32Array::from(starts.clone())) as ArrayRef,
        ),
        ("start_gy", Arc::new(Int32Array::from(vec![0; 2]))),
        (
            "end_gx",
            Arc::new(Int32Array::from(
                starts.iter().map(|x| x + 20_000).collect::<Vec<_>>(),
            )),
        ),
        ("end_gy", Arc::new(Int32Array::from(vec![0; 2]))),
        ("length_m", Arc::new(Float32Array::from(vec![1000.0; 2]))),
        ("rail_type", u8s(0)),
        ("country_iso", u16s(u16::from_le_bytes(*b"DE"))),
        ("passenger_status", u8s(0)),
        ("freight_status", u8s(0)),
        ("passenger_source_id", u16s(GUESSED)),
        ("freight_source_id", u16s(GUESSED)),
        ("trains_passenger_day", f64s(0.0)),
        ("trains_passenger_evening", f64s(0.0)),
        ("trains_passenger_night", f64s(0.0)),
        ("trains_freight_day", f64s(10.0)),
        ("trains_freight_evening", f64s(0.0)),
        ("trains_freight_night", f64s(0.0)),
        (
            "maxspeed",
            Arc::new(UInt16Array::from_iter_values(rows.map(|row| row.0))),
        ),
        (
            "highspeed",
            Arc::new(BooleanArray::from(rows.map(|row| row.1).to_vec())),
        ),
        (
            "name",
            Arc::new(StringArray::from(rows.map(|row| row.2).to_vec())),
        ),
        ("ref", Arc::new(StringArray::from(vec![""; 2]))),
        ("traffic_mode", u8s(0)),
    ])
    .unwrap();
    let table = Table {
        metadata: [
            ("grid", "z30"),
            ("railways_contract", "country_baked_v1"),
            ("rail_traffic_contract", "1"),
        ]
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .into(),
        batches: vec![batch],
    };
    let countries = table_train_km(&table, &FreightNetwork::default()).unwrap();
    let mapped = countries[b"DE"];
    assert_eq!(mapped.freight_guessed.iter().sum::<f64>(), 10.0);
    let national = NationalTrainKm {
        passenger: None,
        freight: Some((2024, 10.0)),
    };
    assert_eq!(fitted(&mapped, &national, [1.0; 3], SPREAD).scales[0], 1.0);
}

/// A missing year takes the nearest earlier published year within the manifest's interval, recorded
/// with its year in the samples; a country with no published value in the interval is unmeasured
/// and omitted from the table even when the TEN-T network covers it, never given a silent 1.0 row.
#[test]
fn missing_year_takes_nearest_earlier_vintage_and_unmeasured_country_is_omitted() {
    use arrow_array::{
        ArrayRef, BooleanArray, Float32Array, Float64Array, Int32Array, RecordBatch, StringArray,
        UInt8Array, UInt16Array,
    };
    use arrow_ipc::writer::FileWriter;
    use arrow_schema::Schema;
    use serde_json::json;
    use std::sync::Arc;

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let sources = root.join("2024");
    let square_dir = sources.join("prepared/z9/0/0");
    std::fs::create_dir_all(&square_dir).unwrap();

    let u8s = |value: u8| Arc::new(UInt8Array::from(vec![value; 2])) as ArrayRef;
    let u16s = |value: u16| Arc::new(UInt16Array::from(vec![value; 2])) as ArrayRef;
    let f64s = |value: f64| Arc::new(Float64Array::from(vec![value; 2])) as ArrayRef;
    let batch = RecordBatch::try_from_iter([
        (
            "start_gx",
            Arc::new(Int32Array::from(vec![0, 0])) as ArrayRef,
        ),
        ("start_gy", Arc::new(Int32Array::from(vec![0, 10_000]))),
        ("end_gx", Arc::new(Int32Array::from(vec![20_000, 20_000]))),
        ("end_gy", Arc::new(Int32Array::from(vec![0, 10_000]))),
        ("length_m", Arc::new(Float32Array::from(vec![1000.0; 2]))),
        ("rail_type", u8s(0)),
        (
            "country_iso",
            Arc::new(UInt16Array::from(vec![
                u16::from_le_bytes(*b"PL"),
                u16::from_le_bytes(*b"BE"),
            ])),
        ),
        ("passenger_status", u8s(0)),
        ("freight_status", u8s(0)),
        ("passenger_source_id", u16s(GUESSED)),
        ("freight_source_id", u16s(GUESSED)),
        ("trains_passenger_day", f64s(20.0)),
        ("trains_passenger_evening", f64s(0.0)),
        ("trains_passenger_night", f64s(0.0)),
        ("trains_freight_day", f64s(10.0)),
        ("trains_freight_evening", f64s(0.0)),
        ("trains_freight_night", f64s(0.0)),
        ("maxspeed", u16s(120)),
        ("highspeed", Arc::new(BooleanArray::from(vec![false; 2]))),
        ("name", Arc::new(StringArray::from(vec![""; 2]))),
        ("ref", Arc::new(StringArray::from(vec![""; 2]))),
        ("traffic_mode", u8s(0)),
    ])
    .unwrap();
    let schema = Schema::new_with_metadata(
        batch.schema().fields().clone(),
        [
            ("grid", "z30"),
            ("railways_contract", "country_baked_v1"),
            ("rail_traffic_contract", "1"),
        ]
        .map(|(k, v)| (k.to_string(), v.to_string())),
    );
    let file = std::fs::File::create(square_dir.join("railways.arrow")).unwrap();
    let mut writer = FileWriter::try_new(file, &schema).unwrap();
    writer
        .write(&batch.with_schema(Arc::new(schema)).unwrap())
        .unwrap();
    writer.finish().unwrap();

    // Both PL and BE lie on the TEN-T network; BE has only a 2011 figure outside 2014..=2024.
    std::fs::write(
        sources.join("tent"),
        "1 PL 0 0 20000 0\n1 BE 0 10000 20000 10000\n",
    )
    .unwrap();
    let index = |codes: &[&str]| {
        serde_json::Value::Object(
            codes
                .iter()
                .enumerate()
                .map(|(i, code)| (code.to_string(), json!(i)))
                .collect(),
        )
    };
    let mut values = serde_json::Map::new();
    let mut put = |train: usize, geo: usize, year: usize, value: f64| {
        values.insert(((train * 2 + geo) * 4 + year).to_string(), json!(value));
    };
    // PL (geo 0): 2014 passenger 7.300 (20/day), 2017 passenger 3.650 (10/day), 2024 freight 3.660 (10/day).
    put(2, 0, 1, 7.300);
    put(2, 0, 2, 3.650);
    put(1, 0, 3, 3.660);
    // BE (geo 1): 2011 passenger and freight only (outside 2014..=2024).
    put(2, 1, 0, 3.650);
    put(1, 1, 0, 3.650);
    std::fs::write(
        sources.join("rail-kilometres"),
        serde_json::to_vec(&json!({
            "id": ["freq", "unit", "train", "geo", "time"],
            "size": [1, 1, 3, 2, 4],
            "dimension": {
                "freq": {"category": {"index": index(&["A"])}},
                "unit": {"category": {"index": index(&["THS_TRKM"])}},
                "train": {"category": {"index": index(&["TOTAL", "TRN_GD", "TRN_PAS"])}},
                "geo": {"category": {"index": index(&["PL", "BE"])}},
                "time": {"category": {"index": index(&["2011", "2014", "2017", "2024"])}},
            },
            "value": values,
        }))
        .unwrap(),
    )
    .unwrap();

    let manifest_path = root.join("2024.json");
    std::fs::write(
        &manifest_path,
        serde_json::to_vec(&json!({
            "year": "2024",
            "as_of": "2025-01-01",
            "built": false,
            "flows": {"start": "2024-01-01", "end": "2025-01-01"},
            "inputs": [
                {"id": "prepared", "kind": "prepared", "region": "world", "publisher": "Quiet Map",
                 "url": "https://quietmap.org/about/data-preparation", "observed": {"start": "2024", "end": "2024"},
                 "published": null, "licence": "MIT", "public_ok": true, "depends_on": [], "description": "Prepared."},
                {"id": "tent", "kind": "tent", "region": "world", "publisher": "EC",
                 "url": "https://example.org/tent", "observed": {"start": "2017", "end": "2017"},
                 "published": null, "licence": "CC BY 4.0", "public_ok": true, "depends_on": [], "description": "TEN-T."},
                {"id": "rail-kilometres", "kind": "model-observations", "region": "world", "publisher": "Eurostat",
                 "url": "https://example.org/eurostat", "observed": {"start": "2014", "end": "2024"},
                 "published": null, "licence": "CC BY 4.0", "public_ok": true, "depends_on": [], "description": "Train-km."},
                {"id": "train-kilometres-eurostat", "kind": "train-scales", "region": "world", "publisher": "Quiet Map",
                 "url": "https://quietmap.org/about/data-preparation", "observed": {"start": "2014", "end": "2024"},
                 "published": null, "licence": "MIT", "public_ok": true,
                 "depends_on": ["rail-kilometres", "prepared", "tent"], "description": "Fitted scales."}
            ],
            "unavailable": []
        }))
        .unwrap(),
    )
    .unwrap();

    let inputs = crate::year::YearInputs::read(&manifest_path, root).unwrap();
    let output = root.join("train-scales.json");
    fit(&inputs, &output).unwrap();

    let table: Value = serde_json::from_slice(&std::fs::read(&output).unwrap()).unwrap();
    let rows = table["rows"].as_array().unwrap();
    assert_eq!(
        rows.len(),
        2,
        "only PL and the world fallback row: {rows:?}"
    );
    assert_eq!(rows[0]["country"], "PL");
    assert!((rows[0]["value"]["scales"][1].as_f64().unwrap() - 0.5).abs() < 1e-12);

    let samples: Value =
        serde_json::from_slice(&std::fs::read(output.with_extension("samples.json")).unwrap())
            .unwrap();
    let countries = samples["countries"].as_array().unwrap();
    let pl = countries.iter().find(|c| c["country"] == "PL").unwrap();
    let be = countries.iter().find(|c| c["country"] == "BE").unwrap();
    assert_eq!(pl["national"]["passenger"][0], 2017);
    assert!((pl["national"]["passenger"][1].as_f64().unwrap() - 10.0).abs() < 1e-12);
    assert_eq!(pl["national"]["freight"][0], 2024);
    assert_eq!(be["national"], json!({"passenger": null, "freight": null}));
}
