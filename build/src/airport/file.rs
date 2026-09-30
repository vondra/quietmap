//! The airport traffic of one z9 square (`<dir>/<x9>/<y9>.arrow`), written by the traffic pass
//! and read by the sources converter: every aeroway line of the square with ground traffic, its
//! airport and its sound power per metre per period and band.

use super::lines::Airport;
use crate::dev4::{Square, column, read_table, text};
use arrow_array::{
    Array, ArrayRef, BooleanArray, Float64Array, Int64Array, RecordBatch, StringArray, UInt16Array,
};
use arrow_schema::{Field, Schema};
use physics::bands::{BAND_FREQUENCY_HZ, BANDS, PERIODS};
use physics::emission::airport::GroundOperation;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const PERIOD_NAMES: [&str; PERIODS] = ["day", "evening", "night"];
/// The airport's movements per average day, in the order of [`LineTraffic::movements_per_day`].
const MOVEMENT_COLUMNS: [&str; 3] = [
    "arrivals_per_day",
    "departures_per_day",
    "ground_vehicles_per_day",
];

/// One aeroway line with ground traffic.
#[derive(Debug, Clone, PartialEq)]
pub struct LineTraffic {
    pub osm_id: i64,
    pub segment: u16,
    /// Latitude and longitude of both ends (deg).
    pub ends: [[f64; 2]; 2],
    pub operation: GroundOperation,
    pub airport: Airport,
    /// The airport's arrivals and departures (aircraft on its runways) and ground vehicles per
    /// average day.
    pub movements_per_day: [f64; 3],
    /// Sound power per metre (dB re 1 pW/m) per period and band; `-inf` is silence.
    pub power_db: [[f64; BANDS]; PERIODS],
}

fn power_column(period: usize, band: usize) -> String {
    format!("{}_{}hz", PERIOD_NAMES[period], BAND_FREQUENCY_HZ[band])
}

pub fn path(dir: &Path, square: Square) -> PathBuf {
    dir.join(square.x.to_string())
        .join(format!("{}.arrow", square.y))
}

/// Writes a square's lines with `metadata` (the window) into the schema.
pub fn write(
    dir: &Path,
    square: Square,
    lines: &[LineTraffic],
    metadata: &HashMap<String, String>,
) -> Result<(), String> {
    let floats = |value: &dyn Fn(&LineTraffic) -> f64| -> ArrayRef {
        Arc::new(lines.iter().map(value).collect::<Float64Array>())
    };
    let strings = |value: &dyn Fn(&LineTraffic) -> &str| -> ArrayRef {
        Arc::new(
            lines
                .iter()
                .map(|line| Some(value(line)))
                .collect::<StringArray>(),
        )
    };
    let mut columns: Vec<(String, ArrayRef)> = vec![
        (
            "osm_id".into(),
            Arc::new(lines.iter().map(|l| l.osm_id).collect::<Int64Array>()),
        ),
        (
            "segment".into(),
            Arc::new(lines.iter().map(|l| l.segment).collect::<UInt16Array>()),
        ),
        ("start_lat".into(), floats(&|l| l.ends[0][0])),
        ("start_lon".into(), floats(&|l| l.ends[0][1])),
        ("end_lat".into(), floats(&|l| l.ends[1][0])),
        ("end_lon".into(), floats(&|l| l.ends[1][1])),
        (
            "runway".into(),
            Arc::new(
                lines
                    .iter()
                    .map(|l| Some(l.operation == GroundOperation::RunwayRoll))
                    .collect::<BooleanArray>(),
            ),
        ),
        ("airport".into(), strings(&|l| &l.airport.key)),
        ("airport_name".into(), strings(&|l| &l.airport.name)),
    ];
    for (index, name) in MOVEMENT_COLUMNS.iter().enumerate() {
        columns.push((name.to_string(), floats(&|l| l.movements_per_day[index])));
    }
    for period in 0..PERIODS {
        for band in 0..BANDS {
            columns.push((
                power_column(period, band),
                floats(&|l| l.power_db[period][band]),
            ));
        }
    }
    let fields: Vec<Field> = columns
        .iter()
        .map(|(name, values)| Field::new(name, values.data_type().clone(), false))
        .collect();
    let schema = Arc::new(Schema::new(fields).with_metadata(metadata.clone()));
    let batch = RecordBatch::try_new(schema.clone(), columns.into_iter().map(|c| c.1).collect())
        .map_err(|e| e.to_string())?;
    let path = path(dir, square);
    let failed = |error: &dyn std::fmt::Display| format!("{}: {error}", path.display());
    std::fs::create_dir_all(path.parent().expect("a square path has a directory"))
        .map_err(|e| failed(&e))?;
    let temporary = path.with_extension("arrow.partial");
    let file = std::fs::File::create(&temporary).map_err(|e| failed(&e))?;
    let mut writer = arrow_ipc::writer::FileWriter::try_new(std::io::BufWriter::new(file), &schema)
        .map_err(|e| failed(&e))?;
    writer.write(&batch).map_err(|e| failed(&e))?;
    writer.finish().map_err(|e| failed(&e))?;
    drop(writer);
    std::fs::rename(&temporary, &path).map_err(|e| failed(&e))
}

/// A square's lines; none when it has no traffic file.
pub fn read(dir: &Path, square: Square) -> Result<Vec<LineTraffic>, String> {
    let path = path(dir, square);
    let Some(table) = read_table(&path)? else {
        return Ok(Vec::new());
    };
    let context = |error: String| format!("{}: {error}", path.display());
    let mut lines = Vec::new();
    for batch in &table.batches {
        let floats = |name: &str| column::<Float64Array>(batch, name).map_err(context);
        let ends = ["start_lat", "start_lon", "end_lat", "end_lon"].map(floats);
        let [start_lat, start_lon, end_lat, end_lon] = ends;
        let (start_lat, start_lon, end_lat, end_lon) = (start_lat?, start_lon?, end_lat?, end_lon?);
        let movements = MOVEMENT_COLUMNS.map(floats);
        let [arrivals, departures, vehicles] = movements;
        let (arrivals, departures, vehicles) = (arrivals?, departures?, vehicles?);
        let power = (0..PERIODS * BANDS)
            .map(|index| floats(&power_column(index / BANDS, index % BANDS)))
            .collect::<Result<Vec<_>, _>>()?;
        let osm_id = column::<Int64Array>(batch, "osm_id").map_err(context)?;
        let segment = column::<UInt16Array>(batch, "segment").map_err(context)?;
        let runway = column::<BooleanArray>(batch, "runway").map_err(context)?;
        let (key, name) = (
            column::<StringArray>(batch, "airport").map_err(context)?,
            column::<StringArray>(batch, "airport_name").map_err(context)?,
        );
        for row in 0..batch.num_rows() {
            lines.push(LineTraffic {
                osm_id: osm_id.value(row),
                segment: segment.value(row),
                ends: [
                    [start_lat.value(row), start_lon.value(row)],
                    [end_lat.value(row), end_lon.value(row)],
                ],
                operation: if runway.value(row) {
                    GroundOperation::RunwayRoll
                } else {
                    GroundOperation::Taxi
                },
                airport: Airport {
                    key: text(key, row).to_string(),
                    name: text(name, row).to_string(),
                },
                movements_per_day: [
                    arrivals.value(row),
                    departures.value(row),
                    vehicles.value(row),
                ],
                power_db: std::array::from_fn(|period| {
                    std::array::from_fn(|band| power[period * BANDS + band].value(row))
                }),
            });
        }
    }
    Ok(lines)
}
