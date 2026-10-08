//! Railway, tram and horn pieces of the dev4 prepared tree as sources: the line speed, the calibrated
//! per-category emission of each period's trains, and the display record of the line.

use super::tent::{FreightNetwork, Tier};
use super::{Converted, group_key, split_at_tile_edges};
use crate::dev4::{Dev4, Square, Table, require_stamp, z30_corner_mercator_m, z30_to_global};
use arrow_array::cast::AsArray;
use arrow_array::types::{Float64Type, Int32Type, Int64Type, UInt8Type, UInt16Type};
use arrow_array::{Array, RecordBatch};
use physics::bands::{BANDS, PERIOD_HOURS, PERIODS};
use physics::emission::rail::{FreightRegion, RailType, line_emission_db};
use std::collections::HashMap;
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
/// evidence 2026-10-01, rail train-km): freight to the goods train-km (the EU27's weighed by their
/// TEN-T tiers first; Belgium and Greece publish none and keep dev4's total); passenger priors,
/// where they carry at least a quarter of the rows' passenger train-km, to 1.2 times the passenger
/// train-km less the timetable-matched rows' (the rows count each track of a double-track line in
/// full: timetable-matched countries read 0.9-1.6 times Eurostat), never raised. dev4's freight
/// priors read Eurostat's goods train-km in Germany, France, Austria, Switzerland and Poland but
/// 2.5 times it in Spain and Romania and 36 times in Ireland, its passenger priors up to 21 times
/// in North Macedonia.
const COUNTRY_TRAIN_SCALES: [([u8; 2], f64, f64); 31] = [
    (*b"AT", 0.499, 1.000),
    (*b"BA", 0.364, 0.075),
    (*b"BE", 0.530, 1.000),
    (*b"BG", 0.164, 0.241),
    (*b"CZ", 0.597, 1.000),
    (*b"DE", 0.547, 1.000),
    (*b"DK", 0.141, 1.000),
    (*b"EE", 0.070, 0.492),
    (*b"ES", 0.119, 0.413),
    (*b"FI", 0.203, 1.000),
    (*b"FR", 0.587, 0.286),
    (*b"GB", 0.381, 1.000),
    (*b"GR", 0.940, 0.170),
    (*b"HR", 0.208, 1.000),
    (*b"HU", 0.229, 1.000),
    (*b"IE", 0.015, 1.000),
    (*b"IT", 0.291, 1.000),
    (*b"LT", 0.151, 0.166),
    (*b"LU", 0.099, 1.000),
    (*b"LV", 0.085, 0.197),
    (*b"ME", 0.122, 0.174),
    (*b"MK", 0.082, 0.048),
    (*b"NL", 0.264, 1.000),
    (*b"NO", 0.265, 1.000),
    (*b"PL", 0.566, 1.000),
    (*b"PT", 0.134, 1.000),
    (*b"RO", 0.160, 0.333),
    (*b"SE", 0.331, 1.000),
    (*b"SI", 0.477, 0.560),
    (*b"SK", 0.316, 1.000),
    (*b"TR", 0.681, 0.372),
];

/// EU freight trains by period (day 07-19, evening 19-23, night 23-07): the freight trains the 19
/// EBA Laerm-Monitoring 2023 stations counted, 37.6 % of them in 22-06, spread evenly within the
/// German day and night windows. dev4 split every EU line as one line's 2012 count, the
/// Rheintalbahn's 54.6 % at night (EBA counted 31 % there in 2023).
const EU_FREIGHT_SHARES: [f64; PERIODS] = [0.468, 0.164, 0.368];

/// The countries the TEN-T maps cover (EU27): their guessed freight is weighed by tier.
const TENT_COUNTRIES: [&[u8; 2]; 27] = [
    b"AT", b"BE", b"BG", b"CY", b"CZ", b"DE", b"DK", b"EE", b"ES", b"FI", b"FR", b"GR", b"HR",
    b"HU", b"IE", b"IT", b"LT", b"LU", b"LV", b"MT", b"NL", b"PL", b"PT", b"RO", b"SE", b"SI",
    b"SK",
];
/// Freight trains per km of line by TEN-T tier (off the network, on a TEN-T freight line, on a
/// core network corridor), relative: dev4 spread a country's freight evenly over every row, half
/// of Germany's on lines off the network and 24 a day on its corridors where the 19 EBA monitors
/// on them counted 30-164 (mean 107); weighed so, Germany's corridors carry 62 % of its freight
/// train-km, 54 trains a day on a double-track line.
const TIER_WEIGHTS: [f64; 3] = [0.5, 2.0, 4.0];

/// The (freight, passenger) factors of a heavy rail row's guessed counts in its country, the
/// freight's at the row's TEN-T tier.
fn train_scales(kind: RailType, iso: [u8; 2], sources: [u16; 2], tier: Tier) -> [f64; 2] {
    let Some((_, freight, passenger)) = COUNTRY_TRAIN_SCALES
        .iter()
        .find(|(code, ..)| *code == iso)
        .filter(|_| kind == RailType::Rail)
    else {
        return [1.0, 1.0];
    };
    let guessed = |source: u16| GUESSED_TRAIN_SOURCES.contains(&source);
    let weight = if TENT_COUNTRIES.contains(&&iso) {
        TIER_WEIGHTS[tier as usize]
    } else {
        1.0
    };
    [
        if guessed(sources[0]) {
            freight * weight
        } else {
            1.0
        },
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

/// One row's trains as dev4 stamped them: passenger and freight per period, and the status and
/// source of each.
#[derive(Clone, Copy)]
struct Trains {
    passenger: [f64; PERIODS],
    freight: [f64; PERIODS],
    status: [u8; 2],
    source: [u16; 2],
}

/// Every row's trains as its run carries them, rows in table order: those of the run's
/// length-weighted median row ([`run_medians`]).
fn run_trains(table: &Table) -> Result<Vec<Trains>, String> {
    let mut rows = Vec::new();
    let mut trains = Vec::new();
    for batch in &table.batches {
        let i32s = |name| column(batch, name).map(|a| a.as_primitive::<Int32Type>());
        let f64s = |name| column(batch, name).map(|a| a.as_primitive::<Float64Type>());
        let ends = [
            (i32s("start_gx")?, i32s("start_gy")?),
            (i32s("end_gx")?, i32s("end_gy")?),
        ];
        let u8s = |name| column(batch, name).map(|a| a.as_primitive::<UInt8Type>());
        let u16s = |name| column(batch, name).map(|a| a.as_primitive::<UInt16Type>());
        let kind = u8s("rail_type")?;
        let status = [u8s("passenger_status")?, u8s("freight_status")?];
        let source = [u16s("passenger_source_id")?, u16s("freight_source_id")?];
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
        for row in 0..batch.num_rows() {
            let ends = ends.map(|(x, y)| (x.value(row), y.value(row)));
            let [a, b] = ends.map(|(x, y)| z30_corner_mercator_m(x, y));
            let row_trains = Trains {
                passenger: passenger.map(|column| column.value(row)),
                freight: freight.map(|column| column.value(row)),
                status: status.map(|column| column.value(row)),
                source: source.map(|column| column.value(row)),
            };
            let total = row_trains.passenger.iter().chain(&row_trains.freight).sum();
            rows.push((
                kind.value(row),
                ends,
                (a[0] - b[0]).hypot(a[1] - b[1]),
                total,
            ));
            trains.push(row_trains);
        }
    }
    Ok(run_medians(&rows)
        .into_iter()
        .map(|median| trains[median])
        .collect())
}

/// A rail row as its run sees it: kind, ends (z30), length (m) and trains a day.
type RunRow = (u8, [(i32, i32); 2], f64, f64);

/// For every row (kind, ends, length, trains a day) the index of its run's length-weighted median
/// row by trains. A run is the rows of one kind meeting end to end where no third row of the kind
/// meets (no switch), so no train enters or leaves between them: dev4's repair of its walked
/// counts skipped every piece under 30 m, and a bridge kept the whole line, the residual or the
/// prior beside plain track divided right (Uvaly: 123 trains a track, its bridges 369, 80 and 2;
/// evidence 2026-10-08, rail). Runs stop at the square's edge.
fn run_medians(rows: &[RunRow]) -> Vec<usize> {
    fn root(parent: &mut [usize], mut index: usize) -> usize {
        while parent[index] != index {
            parent[index] = parent[parent[index]];
            index = parent[index];
        }
        index
    }
    let mut at: HashMap<((i32, i32), u8), Vec<usize>> = HashMap::new();
    for (index, (kind, ends, _, _)) in rows.iter().enumerate() {
        for end in ends {
            at.entry((*end, *kind)).or_default().push(index);
        }
    }
    let mut parent: Vec<usize> = (0..rows.len()).collect();
    for meeting in at.values() {
        if let [a, b] = meeting[..] {
            let (a, b) = (root(&mut parent, a), root(&mut parent, b));
            parent[a] = b;
        }
    }
    let mut runs: HashMap<usize, Vec<usize>> = HashMap::new();
    for index in 0..rows.len() {
        runs.entry(root(&mut parent, index))
            .or_default()
            .push(index);
    }
    let mut median_of = vec![0; rows.len()];
    for members in runs.values_mut() {
        members.sort_by(|&a, &b| rows[a].3.total_cmp(&rows[b].3).then(a.cmp(&b)));
        let half = members.iter().map(|&index| rows[index].2).sum::<f64>() / 2.0;
        let mut length = 0.0;
        let median = members
            .iter()
            .copied()
            .find(|&index| {
                length += rows[index].2;
                length >= half
            })
            .unwrap_or(members[0]);
        for &index in members.iter() {
            median_of[index] = median;
        }
    }
    median_of
}

/// Converts the rail rows of one dev4 square; returns how many rows emit.
pub fn convert(
    (dev4, network): (&Dev4, &FreightNetwork),
    square: Square,
    out: &mut Vec<Converted>,
) -> Result<usize, String> {
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
            let tier = if kind == RailType::Rail && TENT_COUNTRIES.contains(&&iso) {
                network.tier(
                    z30_corner_mercator_m(start_x.value(row), start_y.value(row)),
                    z30_corner_mercator_m(end_x.value(row), end_y.value(row)),
                )
            } else {
                Tier::Off
            };
            let [freight_scale, passenger_scale] =
                train_scales(kind, iso, [freight_source, passenger_source], tier);
            let passenger_trains: [f64; PERIODS] =
                std::array::from_fn(|period| trains.passenger[period] * passenger_scale);
            let mut freight_trains: [f64; PERIODS] =
                std::array::from_fn(|period| trains.freight[period] * freight_scale);
            let region = if EU_FREIGHT_NETWORK.contains(&&iso) {
                FreightRegion::Europe
            } else {
                FreightRegion::World
            };
            if region == FreightRegion::Europe && GUESSED_TRAIN_SOURCES.contains(&freight_source) {
                let total: f64 = freight_trains.iter().sum();
                freight_trains = EU_FREIGHT_SHARES.map(|share| share * total);
            }
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

    /// Rows meeting end to end without a switch carry their run's trains, its length-weighted
    /// median: a 13 m bridge between two plain pieces of 123 takes 123 (dev4 left the whole line,
    /// 369, on it at Uvaly); a third row meeting at a node (a switch) ends the run; a tram on the
    /// same node is another kind.
    #[test]
    fn a_bridge_carries_its_runs_trains() {
        let row = |kind: u8, from: (i32, i32), to: (i32, i32), length: f64, trains: f64| {
            (kind, [from, to], length, trains)
        };
        let rows = [
            row(0, (0, 0), (100, 0), 100.0, 123.0),
            row(0, (100, 0), (113, 0), 13.0, 369.0),
            row(0, (113, 0), (213, 0), 100.0, 123.0),
            row(0, (213, 0), (300, 0), 87.0, 80.0),
            row(0, (213, 0), (213, 50), 50.0, 10.0),
            row(1, (0, 0), (100, 0), 100.0, 5.0),
        ];
        let medians = run_medians(&rows);
        assert_eq!(rows[medians[1]].3, 123.0, "the bridge");
        assert_eq!(medians[0], medians[1]);
        assert_eq!((medians[3], medians[4], medians[5]), (3, 4, 5));
    }

    /// A line too rare for tenths keeps its trains in the display the time statistics read.
    #[test]
    fn rare_trains_keep_three_digits() {
        assert_eq!(display_trains(0.0412), 0.0412);
        assert_eq!(display_trains(12.345), 12.3);
        assert_eq!(display_trains(123.4), 123.0);
        assert_eq!(display_trains(0.0), 0.0);
    }

    /// Guessed heavy rail counts take their country's factors; timetable rows, trams and
    /// countries without a factor keep theirs.
    #[test]
    fn guessed_trains_follow_the_countrys_train_km() {
        let off = Tier::Off;
        assert_eq!(
            train_scales(RailType::Rail, *b"MK", [9013, 0], off),
            [0.082, 0.048]
        );
        assert_eq!(
            train_scales(RailType::Rail, *b"NO", [0, 100], off),
            [0.265, 1.0]
        );
        assert_eq!(
            train_scales(RailType::Tram, *b"RO", [0, 0], off),
            [1.0, 1.0]
        );
        assert_eq!(
            train_scales(RailType::Rail, *b"CH", [0, 0], off),
            [1.0, 1.0]
        );
        // The EU27 weigh their freight by tier: a German corridor carries 8 times a line off the
        // network, a timetable row keeps its passengers.
        let [corridor, _] = train_scales(RailType::Rail, *b"DE", [0, 9864], Tier::Corridor);
        let [off_network, passenger] = train_scales(RailType::Rail, *b"DE", [0, 9864], off);
        assert!((corridor / off_network - 8.0).abs() < 1e-9 && passenger == 1.0);
        assert!((corridor - 0.547 * 4.0).abs() < 1e-9);
    }

    /// The EBA split sums to one and puts 36.8 % of EU freight in the END night.
    #[test]
    fn eu_freight_runs_by_the_counted_split() {
        assert!((EU_FREIGHT_SHARES.iter().sum::<f64>() - 1.0).abs() < 1e-12);
        let (day, night) = (0.624 / 16.0, 0.376 / 8.0);
        let expected = [12.0 * day, 3.0 * day + night, 7.0 * night + day];
        for (share, expected) in EU_FREIGHT_SHARES.iter().zip(expected) {
            assert!((share - expected).abs() < 0.001, "{share} {expected}");
        }
    }
}
