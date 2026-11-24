//! Railway, tram and horn pieces of the dev4 prepared tree as sources: the line speed, the calibrated
//! per-category emission of each period's trains, and the display record of the line.

use super::decks::Decks;
use super::rail_traffic::{column, run_trains};
use super::tent::{FreightNetwork, Tier};
use super::{Converted, fields, group_key, split_at_tile_edges};
use crate::dev4::{Dev4, Square, Table, require_stamp, z30_corner_mercator_m, z30_to_global};
use crate::railways::freight_eligibility::{table_freight_eligibility, utf8_value};
use crate::source_tables::SourceTables;
use arrow_array::cast::AsArray;
use arrow_array::types::{Int32Type, Int64Type, UInt8Type, UInt16Type};
use physics::bands::{BANDS, PERIOD_HOURS, PERIODS};
use physics::emission::rail::{FreightRegion, RailType, line_emission_db};
use tiles::sources::{Attribute, GUESSED_TRAIN_SOURCES, Layer};

/// Source height above the terrain: the wheel-rail contact (CNOSSOS-EU), a locomotive horn.
const RAIL_SOURCE_HEIGHT_M: f64 = 0.5;
const HORN_SOURCE_HEIGHT_M: f64 = 4.0;
/// Half-width of the track bed within which the terrain may not rise above the source ground.
const RAIL_PLATFORM_HALF_WIDTH_M: f64 = 2.5;
/// Speed of a high-speed row that posts none where no row of its line in the square posts one: the
/// km-weighted median posted speed of the world's high-speed rows (111,860 km of dev4's 2026 rows,
/// 250; 300 put Bangkok's Airport Rail Link, run at 160, 9 dB loud; evidence 2026-11-12, data-1).
const HIGH_SPEED_DEFAULT_KMH: f64 = 250.0;
fn usage_name(usage: u8) -> &'static str {
    match usage {
        0 => "main",
        1 => "branch",
        2 => "industrial",
        4 => "tourism",
        _ => "untagged",
    }
}

/// Converts the rail rows of one dev4 square, a row on a bridge on its deck from `decks`; returns
/// how many rows emit.
pub fn convert(
    (dev4, network, tables): (&Dev4, &FreightNetwork, &SourceTables),
    square: Square,
    decks: &Decks,
    out: &mut Vec<Converted>,
) -> Result<usize, String> {
    let Some(table) = dev4.table(square, "railways.arrow")? else {
        return Ok(0);
    };
    require_rail_stamps(&table)?;
    let eligibility = table_freight_eligibility(&table.batches)?;
    let run = run_trains(&table, &eligibility.takes_freight)?;
    let mut emitting = 0;
    let mut first_row = 0;
    for batch in &table.batches {
        let batch_first_row = first_row;
        first_row += batch.num_rows();
        let i32s = |name| column(batch, name).map(|a| a.as_primitive::<Int32Type>());
        let u8s = |name| column(batch, name).map(|a| a.as_primitive::<UInt8Type>());
        let u16s = |name| column(batch, name).map(|a| a.as_primitive::<UInt16Type>());
        let (start_x, start_y, end_x, end_y) = (
            i32s("start_gx")?,
            i32s("start_gy")?,
            i32s("end_gx")?,
            i32s("end_gy")?,
        );
        let (rail_type, usage) = (u8s("rail_type")?, u8s("usage")?);
        let (maxspeed, country) = (u16s("maxspeed")?, u16s("country_iso")?);
        let (tunnel, bridge, high_speed) = (
            column(batch, "tunnel")?.as_boolean(),
            column(batch, "bridge")?.as_boolean(),
            column(batch, "highspeed")?.as_boolean(),
        );
        let (names, refs) = (
            column(batch, "name")?.as_string::<i32>(),
            column(batch, "ref")?.as_string::<i32>(),
        );
        let osm_id = column(batch, "osm_id")?.as_primitive::<Int64Type>();
        for row in 0..batch.num_rows() {
            let row_index = batch_first_row + row;
            let trains = run[row_index];
            let [passenger_source, freight_source] = trains.source;
            let kind = RailType::from_code(rail_type.value(row));
            let iso = country.value(row).to_le_bytes();
            let tier = if kind == RailType::Rail
                && GUESSED_TRAIN_SOURCES.contains(&freight_source)
                && trains.freight.iter().sum::<f64>() > 0.0
                && tables
                    .train_scales
                    .get(iso)
                    .is_some_and(|model| model.tier_weights != [1.0; 3])
            {
                network.tier(
                    iso,
                    z30_corner_mercator_m(start_x.value(row), start_y.value(row)),
                    z30_corner_mercator_m(end_x.value(row), end_y.value(row)),
                )
            } else {
                Tier::Off
            };
            let region = if tables
                .european_freight_fleet
                .get(iso)
                .copied()
                .unwrap_or(false)
            {
                FreightRegion::Europe
            } else {
                FreightRegion::World
            };
            let (passenger_trains, freight_trains, calendar_estimated_shares) =
                trains.scaled(tables, kind, iso, tier, region);
            let (name, reference) = (utf8_value(names, row), utf8_value(refs, row));
            let posted = (maxspeed.value(row) > 0).then(|| f64::from(maxspeed.value(row)));
            let line_speed = eligibility.line_speed[row_index];
            let (speed, speed_source) = match (posted, line_speed) {
                (Some(posted), _) => (posted, "osm_posted"),
                (None, Some(line)) => (line, "high_speed_line"),
                (None, None) if high_speed.value(row) => {
                    (HIGH_SPEED_DEFAULT_KMH, "high_speed_default")
                }
                (None, None) => (kind.default_speed_kmh(), "default_by_type"),
            };
            if tunnel.value(row)
                || kind == RailType::Preserved
                || passenger_trains
                    .iter()
                    .chain(&freight_trains)
                    .all(|trains| *trains <= 0.0)
            {
                continue;
            }
            let start = z30_to_global(start_x.value(row), start_y.value(row));
            let end = z30_to_global(end_x.value(row), end_y.value(row));
            if start == end {
                continue;
            }
            let emission: [[f64; BANDS]; PERIODS] = std::array::from_fn(|period| {
                line_emission_db(
                    kind,
                    speed,
                    passenger_trains[period],
                    freight_trains[period],
                    PERIOD_HOURS[period],
                    region,
                )
            });
            let display = serde_json::json!([
                name,
                reference,
                kind.name(),
                usage_name(usage.value(row)),
                display_trains(passenger_trains[0]),
                display_trains(passenger_trains[1]),
                display_trains(passenger_trains[2]),
                display_trains(freight_trains[0]),
                display_trains(freight_trains[1]),
                display_trains(freight_trains[2]),
                trains.status[0],
                trains.status[1],
                speed,
                speed_source,
                bridge.value(row),
                passenger_source,
                freight_source,
                calendar_estimated_shares[0],
                calendar_estimated_shares[1],
            ]);
            let key = if name.is_empty() && reference.is_empty() {
                group_key(&["rail-way", &osm_id.value(row).to_string()])
            } else {
                group_key(&["rail", reference, name, kind.name()])
            };
            let attribute = Attribute {
                layer: Layer::Railway,
                height_m: if kind == RailType::Horn {
                    HORN_SOURCE_HEIGHT_M
                } else {
                    RAIL_SOURCE_HEIGHT_M
                },
                // Ballast is soft; embedded tram track and bridge decks are hard.
                ground_percent: if kind == RailType::Tram || bridge.value(row) {
                    0
                } else {
                    100
                },
                platform_half_width_m: RAIL_PLATFORM_HALF_WIDTH_M,
                exclusion_radius_m: 0.0,
                footprint_id: 0,
                group_key: key,
                emission,
                display: fields(display),
            };
            let deck_m = bridge
                .value(row)
                .then(|| {
                    decks
                        .railways
                        .get(&[start_x, start_y, end_x, end_y].map(|end| end.value(row)))
                        .copied()
                })
                .flatten();
            for part in split_at_tile_edges(start, end) {
                out.push(Converted {
                    tile: part.tile,
                    ends: part.ends,
                    attribute: attribute.clone(),
                    deck_m: part.deck(deck_m),
                });
            }
            emitting += 1;
        }
    }
    Ok(emitting)
}

/// The prepared railway contract the sources and the train-scale fit read.
pub(super) fn require_rail_stamps(table: &Table) -> Result<(), String> {
    for (key, value) in [
        ("grid", "z30"),
        ("railways_contract", "country_baked_v1"),
        ("rail_traffic_contract", "1"),
    ] {
        require_stamp(table, key, value)?;
    }
    Ok(())
}

/// Trains a period as the display keeps them, to three significant digits: the popup's line
/// statistics read them, so a line of 0.04 trains a period still runs (tenths read it as none).
fn display_trains(trains: f64) -> f64 {
    if trains <= 0.0 {
        return 0.0;
    }
    let scale = 10f64.powi(2 - trains.log10().floor() as i32);
    (trains * scale).round() / scale
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A line too rare for tenths keeps its trains in the display the time statistics read.
    #[test]
    fn rare_trains_keep_three_digits() {
        assert_eq!(display_trains(0.0412), 0.0412);
        assert_eq!(display_trains(12.345), 12.3);
        assert_eq!(display_trains(123.4), 123.0);
        assert_eq!(display_trains(0.0), 0.0);
    }

    /// A passenger-only row joined to a longer row with guessed freight emits zero freight with
    /// normalized provenance (`status = 2`, `source = 0`, `cal_est = 0`) while keeping the run's
    /// passenger trains; observed freight on a passenger-only row is kept intact.
    #[test]
    fn convert_normalizes_suppressed_guessed_freight_and_keeps_observed_freight() {
        use arrow_array::{
            ArrayRef, BooleanArray, Float32Array, Float64Array, Int32Array, Int64Array,
            RecordBatch, StringArray, UInt8Array, UInt16Array,
        };
        use arrow_ipc::writer::FileWriter;
        use arrow_schema::Schema;
        use serde_json::json;
        use std::sync::Arc;

        let dir = tempfile::tempdir().unwrap();
        let square = Square { x: 256, y: 256 };
        let sq_dir = dir.path().join("z9/256/256");
        std::fs::create_dir_all(&sq_dir).unwrap();

        let (x0, y0) = crate::osm::geometry::lonlat_to_grid(0.1, -0.1);
        let (x1, y1) = crate::osm::geometry::lonlat_to_grid(0.1005, -0.1);
        let (x2, y2) = crate::osm::geometry::lonlat_to_grid(0.103, -0.1);
        let (ox0, oy0) = crate::osm::geometry::lonlat_to_grid(0.2, -0.2);
        let (ox1, oy1) = crate::osm::geometry::lonlat_to_grid(0.201, -0.2);

        let batch = RecordBatch::try_from_iter([
            (
                "osm_id",
                Arc::new(Int64Array::from(vec![10, 11, 12])) as ArrayRef,
            ),
            ("start_gx", Arc::new(Int32Array::from(vec![x0, x1, ox0]))),
            ("start_gy", Arc::new(Int32Array::from(vec![y0, y1, oy0]))),
            ("end_gx", Arc::new(Int32Array::from(vec![x1, x2, ox1]))),
            ("end_gy", Arc::new(Int32Array::from(vec![y1, y2, oy1]))),
            (
                "length_m",
                Arc::new(Float32Array::from(vec![55.0, 280.0, 110.0])),
            ),
            ("rail_type", Arc::new(UInt8Array::from(vec![0, 0, 0]))),
            ("usage", Arc::new(UInt8Array::from(vec![0, 0, 0]))),
            ("maxspeed", Arc::new(UInt16Array::from(vec![160, 160, 160]))),
            (
                "country_iso",
                Arc::new(UInt16Array::from(vec![u16::from_le_bytes(*b"TR"); 3])),
            ),
            ("tunnel", Arc::new(BooleanArray::from(vec![false; 3]))),
            ("bridge", Arc::new(BooleanArray::from(vec![false; 3]))),
            ("highspeed", Arc::new(BooleanArray::from(vec![false; 3]))),
            (
                "name",
                Arc::new(StringArray::from(vec![
                    Some("PaxOnly"),
                    Some("Ordinary"),
                    Some("ObservedMode1"),
                ])),
            ),
            (
                "ref",
                Arc::new(StringArray::from(vec![None::<&str>, Some("R1"), None])),
            ),
            ("traffic_mode", Arc::new(UInt8Array::from(vec![1, 0, 1]))),
            (
                "passenger_status",
                Arc::new(UInt8Array::from(vec![1, 1, 1])),
            ),
            ("freight_status", Arc::new(UInt8Array::from(vec![2, 1, 1]))),
            (
                "passenger_source_id",
                Arc::new(UInt16Array::from(vec![100, 100, 100])),
            ),
            (
                "freight_source_id",
                Arc::new(UInt16Array::from(vec![0, 9793, 111])),
            ),
            (
                "trains_passenger_day",
                Arc::new(Float64Array::from(vec![20.0, 50.0, 30.0])),
            ),
            (
                "trains_passenger_evening",
                Arc::new(Float64Array::from(vec![0.0; 3])),
            ),
            (
                "trains_passenger_night",
                Arc::new(Float64Array::from(vec![0.0; 3])),
            ),
            (
                "trains_freight_day",
                Arc::new(Float64Array::from(vec![0.0, 10.0, 12.0])),
            ),
            (
                "trains_freight_evening",
                Arc::new(Float64Array::from(vec![0.0; 3])),
            ),
            (
                "trains_freight_night",
                Arc::new(Float64Array::from(vec![0.0; 3])),
            ),
            (
                "passenger_missing_calendar_estimate_day",
                Arc::new(Float64Array::from(vec![0.0; 3])),
            ),
            (
                "passenger_missing_calendar_estimate_evening",
                Arc::new(Float64Array::from(vec![0.0; 3])),
            ),
            (
                "passenger_missing_calendar_estimate_night",
                Arc::new(Float64Array::from(vec![0.0; 3])),
            ),
            (
                "freight_missing_calendar_estimate_day",
                Arc::new(Float64Array::from(vec![0.0, 4.0, 3.0])),
            ),
            (
                "freight_missing_calendar_estimate_evening",
                Arc::new(Float64Array::from(vec![0.0; 3])),
            ),
            (
                "freight_missing_calendar_estimate_night",
                Arc::new(Float64Array::from(vec![0.0; 3])),
            ),
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
        let file = std::fs::File::create(sq_dir.join("railways.arrow")).unwrap();
        let mut writer = FileWriter::try_new(file, &schema).unwrap();
        writer
            .write(&batch.with_schema(Arc::new(schema)).unwrap())
            .unwrap();
        writer.finish().unwrap();

        let dev4 = Dev4 {
            prepared: dir.path().to_path_buf(),
            rasters: std::path::PathBuf::new(),
        };
        let mut out = Vec::new();
        let emitted = convert(
            (
                &dev4,
                &FreightNetwork::default(),
                SourceTables::test_fixture(),
            ),
            square,
            &Decks::default(),
            &mut out,
        )
        .unwrap();
        assert_eq!(emitted, 3);

        let by_name = |name: &str| {
            out.iter()
                .find(|piece| piece.attribute.display[0] == json!(name))
                .unwrap()
        };
        // PaxOnly (mode=1 joined to Ordinary): inherits 50 pax, suppresses guessed freight 9793
        // and normalizes freight status/source/calendar-estimate share.
        let pax_only = &by_name("PaxOnly").attribute.display;
        assert_eq!(pax_only[4], json!(50.0));
        assert_eq!(pax_only[7], json!(0.0));
        assert_eq!(pax_only[11], json!(2));
        assert_eq!(pax_only[16], json!(0));
        assert_eq!(pax_only[18], json!(0.0));

        // Ordinary (mode=0): keeps 50 pax and TR-scaled guessed freight (source 9793).
        let tr_scale = SourceTables::test_fixture()
            .train_scales
            .get(*b"TR")
            .unwrap()
            .scales[0];
        let ordinary = &by_name("Ordinary").attribute.display;
        assert_eq!(ordinary[4], json!(50.0));
        assert_eq!(ordinary[7], json!(display_trains(10.0 * tr_scale)));
        assert_eq!(ordinary[16], json!(9793));

        // ObservedMode1 (mode=1 with observed source 111): keeps 12 freight and source 111.
        let observed = &by_name("ObservedMode1").attribute.display;
        assert_eq!(observed[4], json!(30.0));
        assert_eq!(observed[7], json!(12.0));
        assert_eq!(observed[16], json!(111));
    }
}
