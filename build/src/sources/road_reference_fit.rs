//! Fit numbered roads' daily traffic per subdivision: a highway department's vehicle-km over the mapped length.

use super::road_reference_screen::{Counted, Mapped, Pair, screen};
use super::road_traffic::{major_group, major_group_prediction};
use crate::dev4::{Dev4, Square, z30_corner_degrees};
use crate::source_tables::{ReferenceCountRow, RoadModels, Subdivisions, reference_number};
use crate::traffic::Surroundings;
use crate::year::{Kind, YearInputs};
use anyhow::{Context, Result, anyhow, ensure};
use rayon::prelude::*;
use serde_json::json;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The department's vehicle-km by highway and province (the 2021 table of control sections).
pub(crate) const VEHICLE_KM_INPUT: &str = "th-highway-distance";
/// Its provinces' names with their ISO 3166-2 codes.
pub(crate) const PROVINCE_NAMES_INPUT: &str = "th-province-names";
/// Mapped classes that carry a highway's number as the highway: motorway to tertiary and
/// unclassified (links and streets carrying the number are not the highway).
const HIGHWAY_CLASSES: [u8; 6] = [0, 1, 2, 3, 4, 9];

/// The department's control sections summed by highway and province (its ISO 3166-2 number in
/// `country`): vehicle-km a year in four categories and the sections behind them.
fn read_department(inputs: &YearInputs, country: &str) -> Result<BTreeMap<Pair, Counted>> {
    let names: BTreeMap<String, String> = serde_json::from_slice(&std::fs::read(
        inputs.by_id(PROVINCE_NAMES_INPUT).map_err(|e| anyhow!(e))?,
    )?)?;
    let path = inputs.by_id(VEHICLE_KM_INPUT).map_err(|e| anyhow!(e))?;
    let mut reader = csv::Reader::from_path(&path)?;
    ensure!(
        reader.headers()?.len() == 17,
        "{}: expected 17 columns",
        path.display()
    );
    let mut pairs: BTreeMap<Pair, Counted> = BTreeMap::new();
    for record in reader.records() {
        let record = record?;
        let number: u16 = record[1]
            .trim()
            .parse()
            .with_context(|| format!("highway {:?}", &record[1]))?;
        let province = record[16].trim();
        let code = names
            .get(province)
            .ok_or_else(|| anyhow!("unknown province {province}"))?;
        let province: u8 = code
            .strip_prefix(&format!("{country}-"))
            .and_then(|n| n.parse().ok())
            .ok_or_else(|| anyhow!("province code {code}"))?;
        let count = |k: usize| -> Result<f64> {
            let text = record[k].trim();
            Ok(if text.is_empty() { 0.0 } else { text.parse()? })
        };
        let v = (4..16).map(count).collect::<Result<Vec<f64>>>()?;
        // Light: cars of up to and over seven seats, small buses, small trucks; medium: medium
        // buses and trucks; heavy: large buses and trucks, trailers, semi-trailers.
        let classes = [
            v[0] + v[1] + v[2] + v[5],
            v[3] + v[6],
            v[4] + v[7] + v[8] + v[9],
            v[11],
        ];
        let entry = pairs.entry((number, province)).or_default();
        for (sum, value) in entry.vehicle_km.iter_mut().zip(classes) {
            *sum += value;
        }
        entry.sections += 1;
    }
    Ok(pairs)
}

/// The mapped length of the numbered roads of `country` in one square by number and
/// subdivision, with the major-road model's expected vehicle-km a day along them.
fn mapped_square(
    (dev4, traffic, models): (&Dev4, &Path, &RoadModels),
    (subdivisions, country): (&Subdivisions, [u8; 2]),
    square: Square,
) -> Result<BTreeMap<Pair, Mapped>> {
    use arrow_array::cast::AsArray;
    use arrow_array::types::{Float32Type, Int32Type, UInt8Type, UInt16Type};
    let mut mapped: BTreeMap<Pair, Mapped> = BTreeMap::new();
    let Some(table) = dev4.table(square, "roads.arrow").map_err(|e| anyhow!(e))? else {
        return Ok(mapped);
    };
    let surroundings = Surroundings::load(traffic, square).map_err(|e| anyhow!(e))?;
    let iso = u16::from_le_bytes(country);
    for batch in &table.batches {
        let column = |name: &str| {
            batch
                .column_by_name(name)
                .ok_or_else(|| anyhow!("roads.arrow has no {name}"))
        };
        let u8s = |name| column(name).map(|a| a.as_primitive::<UInt8Type>());
        let (class, built_up, oneway, lanes) = (
            u8s("road_class")?,
            u8s("built_up")?,
            u8s("oneway")?,
            u8s("lanes")?,
        );
        let countries = column("country_iso")?.as_primitive::<UInt16Type>();
        let length = column("length_m")?.as_primitive::<Float32Type>();
        let reference = column("ref")?.as_string::<i32>();
        let i32s = |name| column(name).map(|a| a.as_primitive::<Int32Type>());
        let ends = [
            i32s("start_gx")?,
            i32s("start_gy")?,
            i32s("end_gx")?,
            i32s("end_gy")?,
        ];
        for row in 0..batch.num_rows() {
            let class = class.value(row);
            if countries.value(row) != iso || !HIGHWAY_CLASSES.contains(&class) {
                continue;
            }
            let Some(number) = reference_number(reference.value(row)) else {
                continue;
            };
            let middle = (
                ((i64::from(ends[0].value(row)) + i64::from(ends[2].value(row))) / 2) as i32,
                ((i64::from(ends[1].value(row)) + i64::from(ends[3].value(row))) / 2) as i32,
            );
            let (lat, lon) = z30_corner_degrees(middle.0, middle.1);
            let km = f64::from(length.value(row)) / 1_000.0;
            let oneway = oneway.value(row) != 0;
            let around = models
                .major_radii_m
                .map(|radius| surroundings.around(middle, radius));
            let expected = major_group_prediction(
                models,
                major_group(class).unwrap_or(2),
                around,
                (built_up.value(row), oneway, lanes.value(row)),
            );
            let entry = mapped
                .entry((number, subdivisions.at(country, lat, lon)))
                .or_default();
            entry.km += if oneway { 0.5 * km } else { km };
            entry.expected_vehicle_km += expected * km;
        }
    }
    Ok(mapped)
}

/// The latitudes and longitudes `[south, north, west, east]` of a z9 square.
fn square_bounds(square: Square) -> [f64; 4] {
    let axis = f64::from(crate::dev4::SQUARES_PER_AXIS);
    let latitude = |y: u32| {
        (std::f64::consts::PI * (1.0 - 2.0 * f64::from(y) / axis))
            .sinh()
            .atan()
            .to_degrees()
    };
    let longitude = |x: u32| f64::from(x) * 360.0 / axis - 180.0;
    [
        latitude(square.y + 1),
        latitude(square.y),
        longitude(square.x),
        longitude(square.x + 1),
    ]
}

/// `value` correctly rounded to `places` decimals, as the 2026 table was printed.
fn decimals(value: f64, places: usize) -> f64 {
    format!("{value:.places$}")
        .parse()
        .expect("a formatted number")
}

/// The year run's step: the department's pairs over the year's mapped highways.
pub fn fit(inputs: &YearInputs, output: &Path) -> Result<()> {
    ensure!(
        !output.exists(),
        "road-reference-counts needs a new output path"
    );
    let entry = inputs
        .manifest
        .inputs
        .iter()
        .find(|i| i.kind == Kind::RoadReferenceCounts)
        .ok_or_else(|| anyhow!("no road-reference-counts entry"))?;
    for id in [VEHICLE_KM_INPUT, PROVINCE_NAMES_INPUT] {
        ensure!(
            entry.depends_on.iter().any(|d| d == id),
            "{}: undeclared dependency {id}",
            entry.id
        );
    }
    let country = crate::source_tables::country_code(&entry.region).map_err(|e| anyhow!(e))?;
    let counted = read_department(inputs, &entry.region)?;
    let models = crate::source_tables::road_models(inputs).map_err(|e| anyhow!(e))?;
    let subdivisions = Subdivisions::read(inputs).map_err(|e| anyhow!(e))?;
    let [south, north, west, east] = subdivisions
        .extent(country)
        .ok_or_else(|| anyhow!("no subdivision grid for {}", entry.region))?;
    let prepared = PathBuf::from(inputs.required(Kind::Prepared).map_err(|e| anyhow!(e))?);
    let traffic = PathBuf::from(inputs.required(Kind::RoadTraffic).map_err(|e| anyhow!(e))?);
    let squares: Vec<Square> = crate::roads::prepared_squares(&prepared)?
        .into_iter()
        .filter(|&square| {
            let [s, n, w, e] = square_bounds(square);
            s < north && n > south && w < east && e > west
        })
        .collect();
    let dev4 = Dev4 {
        prepared,
        rasters: PathBuf::new(),
    };
    let parts: Vec<BTreeMap<Pair, Mapped>> = squares
        .par_iter()
        .map(|&square| mapped_square((&dev4, &traffic, &models), (&subdivisions, country), square))
        .collect::<Result<_>>()?;
    let mut mapped: BTreeMap<Pair, Mapped> = BTreeMap::new();
    for part in parts {
        for (pair, m) in part {
            let entry = mapped.entry(pair).or_default();
            entry.km += m.km;
            entry.expected_vehicle_km += m.expected_vehicle_km;
        }
    }
    let (rows, record) = screen(&counted, &mapped);
    ensure!(
        !rows.is_empty(),
        "no highway of the department matches the mapped roads"
    );
    let code = String::from_utf8_lossy(&country).into_owned();
    let rows: Vec<ReferenceCountRow> = rows
        .into_iter()
        .map(|((number, subdivision), daily, shares)| ReferenceCountRow {
            country: code.clone(),
            number,
            subdivision,
            daily: decimals(daily, 0),
            shares: shares.map(|share| decimals(share, 3)),
            source_id: tiles::sources::THAI_HIGHWAYS_SOURCE_ID,
        })
        .collect();
    eprintln!(
        "road-reference-counts: {} pairs from {} squares",
        rows.len(),
        squares.len()
    );
    crate::source_tables::write_rows(output, &rows).map_err(|e| anyhow!(e))?;
    let mut record = record;
    record["squares"] = json!(squares.iter().map(|s| [s.x, s.y]).collect::<Vec<_>>());
    std::fs::write(
        output.with_extension("samples.json"),
        serde_json::to_vec(&record)?,
    )?;
    Ok(())
}
