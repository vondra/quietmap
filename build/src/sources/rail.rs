//! Railway, tram and horn pieces of the dev4 prepared tree as sources: the line speed, the calibrated
//! per-category emission of each period's trains, and the display record of the line.

use super::{Converted, group_key, split_at_tile_edges};
use crate::dev4::{Dev4, Square, require_stamp, z30_to_global};
use arrow_array::cast::AsArray;
use arrow_array::types::{Float64Type, Int32Type, Int64Type, UInt8Type, UInt16Type};
use arrow_array::{Array, RecordBatch};
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
/// Freight on these networks runs the EU fleet (EU27, CH, NO, GB).
const EU_FREIGHT_NETWORK: [&[u8; 2]; 30] = [
    b"AT", b"BE", b"BG", b"HR", b"CY", b"CZ", b"DK", b"EE", b"FI", b"FR", b"DE", b"GR", b"HU",
    b"IE", b"IT", b"LV", b"LT", b"LU", b"MT", b"NL", b"PL", b"PT", b"RO", b"SK", b"SI", b"ES",
    b"SE", b"CH", b"NO", b"GB",
];

/// dev4's rail sources whose train counts are a guess: the per-line priors (0) and the
/// operator-class CNOSSOS defaults of countries without open timetables.
const GUESSED_TRAIN_SOURCES: [u16; 19] = [
    0, 2044, 9013, 9181, 9232, 9263, 9365, 9369, 9399, 9405, 9505, 9567, 9644, 9730, 9793, 9805,
    9819, 9835, 9861,
];
/// Per country, the factors on the guessed freight and passenger counts of its rail lines that
/// bring the rows' train-km to Eurostat's (rail_tf_trainmv 2024, the United Kingdom 2019;
/// evidence 2026-10-01, rail train-km): freight to the goods train-km; passenger priors, where they
/// carry at least a quarter of the rows' passenger train-km, to 1.2 times the passenger train-km
/// less the timetable-matched rows' (the rows count each track of a double-track line in full:
/// timetable-matched countries read 0.9-1.6 times Eurostat), never raised. dev4's freight priors
/// read Eurostat's goods train-km in Germany, France, Austria, Switzerland and Poland but 2.5 times
/// it in Spain and Romania and 36 times in Ireland, its passenger priors up to 21 times in North
/// Macedonia.
const COUNTRY_TRAIN_SCALES: [([u8; 2], f64, f64); 27] = [
    (*b"BA", 0.364, 0.075),
    (*b"BG", 0.391, 0.241),
    (*b"CZ", 0.897, 1.000),
    (*b"DK", 0.458, 1.000),
    (*b"EE", 0.160, 0.492),
    (*b"ES", 0.316, 0.413),
    (*b"FI", 0.489, 1.000),
    (*b"FR", 1.009, 0.286),
    (*b"GB", 0.381, 1.000),
    (*b"GR", 1.000, 0.170),
    (*b"HR", 0.426, 1.000),
    (*b"HU", 0.489, 1.000),
    (*b"IE", 0.028, 1.000),
    (*b"IT", 0.649, 1.000),
    (*b"LT", 0.397, 0.166),
    (*b"LU", 0.214, 1.000),
    (*b"LV", 0.191, 0.197),
    (*b"ME", 0.122, 0.174),
    (*b"MK", 0.082, 0.048),
    (*b"NL", 0.588, 1.000),
    (*b"NO", 0.265, 1.000),
    (*b"PT", 0.313, 1.000),
    (*b"RO", 0.297, 0.333),
    (*b"SE", 0.665, 1.000),
    (*b"SI", 1.281, 0.560),
    (*b"SK", 0.619, 1.000),
    (*b"TR", 0.681, 0.372),
];

/// The (freight, passenger) factors of a heavy rail row's guessed counts in its country.
fn train_scales(kind: RailType, iso: [u8; 2], sources: [u16; 2]) -> [f64; 2] {
    let Some((_, freight, passenger)) = COUNTRY_TRAIN_SCALES
        .iter()
        .find(|(code, ..)| *code == iso)
        .filter(|_| kind == RailType::Rail)
    else {
        return [1.0, 1.0];
    };
    let guessed = |source: u16| GUESSED_TRAIN_SOURCES.contains(&source);
    [
        if guessed(sources[0]) { *freight } else { 1.0 },
        if guessed(sources[1]) { *passenger } else { 1.0 },
    ]
}

fn usage_name(usage: u8) -> &'static str {
    match usage {
        0 => "main",
        1 => "branch",
        2 => "industrial",
        4 => "tourism",
        _ => "untagged",
    }
}

fn column<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a dyn Array, String> {
    batch
        .column_by_name(name)
        .map(|c| c.as_ref())
        .ok_or_else(|| format!("railways.arrow: no column {name}"))
}

/// Converts the rail rows of one dev4 square; returns how many rows emit.
pub fn convert(dev4: &Dev4, square: Square, out: &mut Vec<Converted>) -> Result<usize, String> {
    let Some(table) = dev4.table(square, "railways.arrow")? else {
        return Ok(0);
    };
    for (key, value) in [
        ("grid", "z30"),
        ("railways_contract", "country_baked_v1"),
        ("rail_traffic_contract", "1"),
    ] {
        require_stamp(&table, key, value)?;
    }
    let mut emitting = 0;
    for batch in &table.batches {
        let i32s = |name| column(batch, name).map(|a| a.as_primitive::<Int32Type>());
        let u8s = |name| column(batch, name).map(|a| a.as_primitive::<UInt8Type>());
        let u16s = |name| column(batch, name).map(|a| a.as_primitive::<UInt16Type>());
        let f64s = |name| column(batch, name).map(|a| a.as_primitive::<Float64Type>());
        let (start_x, start_y, end_x, end_y) = (
            i32s("start_gx")?,
            i32s("start_gy")?,
            i32s("end_gx")?,
            i32s("end_gy")?,
        );
        let (rail_type, usage, passenger_status, freight_status) = (
            u8s("rail_type")?,
            u8s("usage")?,
            u8s("passenger_status")?,
            u8s("freight_status")?,
        );
        let (maxspeed, country, passenger_source, freight_source) = (
            u16s("maxspeed")?,
            u16s("country_iso")?,
            u16s("passenger_source_id")?,
            u16s("freight_source_id")?,
        );
        let passenger = [
            f64s("trains_passenger_day")?,
            f64s("trains_passenger_evening")?,
            f64s("trains_passenger_night")?,
        ];
        let freight = [
            f64s("trains_freight_day")?,
            f64s("trains_freight_evening")?,
            f64s("trains_freight_night")?,
        ];
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
            let kind = RailType::from_code(rail_type.value(row));
            let iso = country.value(row).to_le_bytes();
            let [freight_scale, passenger_scale] = train_scales(
                kind,
                iso,
                [freight_source.value(row), passenger_source.value(row)],
            );
            let passenger_trains: [f64; PERIODS] =
                std::array::from_fn(|period| passenger[period].value(row) * passenger_scale);
            let freight_trains: [f64; PERIODS] =
                std::array::from_fn(|period| freight[period].value(row) * freight_scale);
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
            let region = if EU_FREIGHT_NETWORK.contains(&&iso) {
                FreightRegion::Europe
            } else {
                FreightRegion::World
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
            let round = |trains: f64| (trains * 10.0).round() / 10.0;
            let display = serde_json::json!([
                name,
                reference,
                kind.name(),
                usage_name(usage.value(row)),
                round(passenger_trains[0]),
                round(passenger_trains[1]),
                round(passenger_trains[2]),
                round(freight_trains[0]),
                round(freight_trains[1]),
                round(freight_trains[2]),
                passenger_status.value(row),
                freight_status.value(row),
                speed,
                speed_source,
                bridge.value(row),
                passenger_source.value(row),
                freight_source.value(row),
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
                display: display.to_string(),
            };
            for (tile, ends) in split_at_tile_edges(start, end) {
                out.push(Converted {
                    tile,
                    ends,
                    attribute: attribute.clone(),
                });
            }
            emitting += 1;
        }
    }
    Ok(emitting)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Guessed heavy rail counts take their country's factors; timetable rows, trams and
    /// countries without a factor keep theirs.
    #[test]
    fn guessed_trains_follow_the_countrys_train_km() {
        assert_eq!(train_scales(RailType::Rail, *b"IE", [0, 0]), [0.028, 1.0]);
        assert_eq!(
            train_scales(RailType::Rail, *b"MK", [9013, 0]),
            [0.082, 0.048]
        );
        assert_eq!(train_scales(RailType::Rail, *b"RO", [0, 100]), [0.297, 1.0]);
        assert_eq!(train_scales(RailType::Tram, *b"RO", [0, 0]), [1.0, 1.0]);
        assert_eq!(train_scales(RailType::Rail, *b"DE", [0, 0]), [1.0, 1.0]);
    }
}
