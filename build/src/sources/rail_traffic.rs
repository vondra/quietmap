//! Preserve measured and missing-day trains separately through run medians, country scaling and period allocation.
use super::runs::{RunRow, run_medians};
use super::tent::Tier;
use crate::dev4::{Table, z30_corner_mercator_m};
use crate::source_tables::{SourceTables, TrainScales};
use arrow_array::cast::AsArray;
use arrow_array::types::{Float64Type, Int32Type, UInt8Type, UInt16Type};
use arrow_array::{Array, RecordBatch};
use physics::bands::PERIODS;
use physics::emission::rail::{FreightRegion, RailType};
use tiles::sources::GUESSED_TRAIN_SOURCES;

/// The (freight, passenger) factors of a heavy rail row's guessed counts from its country's
/// `train-scales` row, the freight's at the row's TEN-T tier.
pub(super) fn train_scales(
    model: Option<&TrainScales>,
    kind: RailType,
    sources: [u16; 2],
    tier: Tier,
) -> [f64; 2] {
    let Some(model) = model.filter(|_| kind == RailType::Rail) else {
        return [1.0, 1.0];
    };
    let guessed = |source: u16| GUESSED_TRAIN_SOURCES.contains(&source);
    let [freight, passenger] = model.scales;
    let weight = model.tier_weights[tier as usize];
    [
        if guessed(sources[0]) {
            freight * weight
        } else {
            1.0
        },
        if guessed(sources[1]) { passenger } else { 1.0 },
    ]
}

pub(super) fn column<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a dyn Array, String> {
    batch
        .column_by_name(name)
        .map(|c| c.as_ref())
        .ok_or_else(|| format!("railways.arrow: no column {name}"))
}

/// One row's trains as dev4 stamped them: passenger and freight per period, and the status and
/// source of each.
#[derive(Clone, Copy)]
pub(super) struct Trains {
    pub passenger: [f64; PERIODS],
    pub freight: [f64; PERIODS],
    pub missing_calendar_estimate: [[f64; PERIODS]; 2],
    pub status: [u8; 2],
    pub source: [u16; 2],
}

/// Every row's trains as its run carries them, rows in table order: those of the run's
/// length-weighted median row ([`run_medians`]).
pub(super) fn run_trains(table: &Table) -> Result<Vec<Trains>, String> {
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
                missing_calendar_estimate: read_missing_calendar_estimate(batch, row)?,
                status: status.map(|column| column.value(row)),
                source: source.map(|column| column.value(row)),
            };
            rows.push(RunRow {
                meets: kind.value(row),
                joins: Some(kind.value(row)),
                ends,
                length_m: (a[0] - b[0]).hypot(a[1] - b[1]),
                value: row_trains.passenger.iter().chain(&row_trains.freight).sum(),
            });
            trains.push(row_trains);
        }
    }
    Ok(run_medians(&rows)
        .into_iter()
        .map(|median| trains[median])
        .collect())
}

fn read_missing_calendar_estimate(
    batch: &RecordBatch,
    row: usize,
) -> Result<[[f64; PERIODS]; 2], String> {
    let mut result = [[0.0; PERIODS]; 2];
    let mut present = 0;
    for (category, label) in ["passenger", "freight"].iter().enumerate() {
        for (period, name) in ["day", "evening", "night"].iter().enumerate() {
            if let Some(array) =
                batch.column_by_name(&format!("{label}_missing_calendar_estimate_{name}"))
            {
                let value = array.as_primitive::<Float64Type>().value(row);
                let total = column(batch, &format!("trains_{label}_{name}"))?
                    .as_primitive::<Float64Type>()
                    .value(row);
                if !value.is_finite() || value < 0.0 || value > total {
                    return Err(
                        "railways.arrow: invalid missing-calendar estimate component".into(),
                    );
                }
                result[category][period] = value;
                present += 1;
            }
        }
    }
    if present != 0 && present != 6 {
        return Err("railways.arrow: incomplete missing-calendar estimate components".into());
    }
    Ok(result)
}

impl Trains {
    pub fn scaled(
        self,
        tables: &SourceTables,
        kind: RailType,
        iso: [u8; 2],
        tier: Tier,
        region: FreightRegion,
    ) -> ([f64; PERIODS], [f64; PERIODS], [f64; 2]) {
        let [freight, passenger] = train_scales(
            tables.train_scales.get(iso),
            kind,
            [self.source[1], self.source[0]],
            tier,
        );
        let mut result = [self.passenger, self.freight];
        let mut estimated_shares = [0.0; 2];
        for (category, scale) in [passenger, freight].into_iter().enumerate() {
            let total: f64 = result[category].iter().sum();
            if total > 0.0 {
                estimated_shares[category] =
                    self.missing_calendar_estimate[category].iter().sum::<f64>() / total;
            }
            result[category] = result[category].map(|value| value * scale);
            if category == 1
                && region == FreightRegion::Europe
                && GUESSED_TRAIN_SOURCES.contains(&self.source[category])
            {
                let scaled_total: f64 = result[category].iter().sum();
                result[category] = tables
                    .freight_period_shares
                    .map(|share| share * scaled_total);
            }
        }
        (result[0], result[1], estimated_shares)
    }
}

#[cfg(test)]
#[path = "rail_traffic_tests.rs"]
mod tests;
