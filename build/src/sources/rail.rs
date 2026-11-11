//! Railway, tram and horn pieces of the dev4 prepared tree as sources: the line speed, the calibrated
//! per-category emission of each period's trains, and the display record of the line.

use super::decks::Decks;
use super::rail_traffic::{column, run_trains};
use super::tent::{FreightNetwork, Tier};
use super::{Converted, fields, group_key, split_at_tile_edges};
use crate::dev4::{Dev4, Square, Table, require_stamp, z30_corner_mercator_m, z30_to_global};
use crate::source_tables::SourceTables;
use arrow_array::cast::AsArray;
use arrow_array::types::{Int32Type, Int64Type, UInt8Type, UInt16Type};
use physics::bands::{BANDS, PERIOD_HOURS, PERIODS};
use physics::emission::rail::{FreightRegion, RailType, line_emission_db};
use tiles::sources::{Attribute, Layer};

/// Source height above the terrain: the wheel-rail contact (CNOSSOS-EU), a locomotive horn.
const RAIL_SOURCE_HEIGHT_M: f64 = 0.5;
const HORN_SOURCE_HEIGHT_M: f64 = 4.0;
/// Half-width of the track bed within which the terrain may not rise above the source ground.
const RAIL_PLATFORM_HALF_WIDTH_M: f64 = 2.5;
/// Speed of an untagged high-speed line.
const HIGH_SPEED_DEFAULT_KMH: f64 = 300.0;
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
    let run = run_trains(&table)?;
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
            let trains = run[batch_first_row + row];
            let [passenger_source, freight_source] = trains.source;
            let kind = RailType::from_code(rail_type.value(row));
            let iso = country.value(row).to_le_bytes();
            let tier = if kind == RailType::Rail
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
            let (speed, speed_source) = if maxspeed.value(row) > 0 {
                (f64::from(maxspeed.value(row)), "osm_posted")
            } else if high_speed.value(row) {
                (HIGH_SPEED_DEFAULT_KMH, "high_speed_default")
            } else {
                (kind.default_speed_kmh(), "default_by_type")
            };
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
            let (name, reference) = (names.value(row), refs.value(row));
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
}
