//! The counted rows of a prepared tree, each with the trip ends its buildings make around it.

use super::road_traffic::{BuildingTraffic, major_group, thai_rural_road_ref};
use crate::dev4::{Dev4, Square, require_stamp};
use crate::roads::observation_sample::SamplingDesign;
use crate::year::{Kind, YearInputs};
use anyhow::{Context, Result, anyhow};
use rayon::prelude::*;
use std::path::{Path, PathBuf};

/// The radii whose trip ends a counted row carries: the main classes' candidates and the
/// major-road model's three (1, 5 and 15 km, the 2026 fit's).
pub(crate) const RADII_M: [f64; 4] = [1_000.0, 2_000.0, 5_000.0, 15_000.0];
pub(crate) const MAIN_CANDIDATE_RADII: [usize; 3] = [0, 1, 2];
pub(crate) const MAJOR_RADII: [usize; 3] = [0, 2, 3];
/// The classes that take a main-class model: secondary, tertiary, unclassified.
pub(crate) const MAIN_CLASSES: [u8; 3] = [3, 4, 9];

/// One counted row of the prepared tree: its count is its own, measured, not a continuation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CountedRow {
    pub country: [u8; 2],
    pub class: u8,
    pub built_up: u8,
    pub oneway: bool,
    pub lanes: u8,
    pub rural_network: bool,
    /// How its publisher chose where to count.
    pub design: SamplingDesign,
    pub length_m: f64,
    pub total: f64,
    pub around: [f64; 4],
}

/// Whether a prepared row's count enters the fit: a fitted class, measured, its own (neither a
/// continuation along its road nor a main road's count copied beside it), in known surroundings,
/// with traffic along a length.
pub(crate) fn enters_fit(
    class: u8,
    (measured, source_id, copy): (bool, u16, bool),
    built_up: u8,
    (total, length_m): (f64, f64),
) -> bool {
    (MAIN_CLASSES.contains(&class) || major_group(class).is_some())
        && measured
        && source_id != crate::roads::CONTINUITY_SOURCE
        && !copy
        && (1..=2).contains(&built_up)
        && total > 0.0
        && length_m > 0.0
}

fn column<'a>(
    batch: &'a arrow_array::RecordBatch,
    name: &str,
) -> Result<&'a dyn arrow_array::Array> {
    batch
        .column_by_name(name)
        .map(|c| c.as_ref())
        .ok_or_else(|| anyhow!("roads.arrow has no {name}"))
}

/// The counted rows of one prepared square, none where its buildings are not mapped (the model
/// is fitted where it is applied).
fn counted_rows(dev4: &Dev4, traffic: &Path, square: Square) -> Result<Vec<CountedRow>> {
    use arrow_array::cast::AsArray;
    use arrow_array::types::{Float32Type, Float64Type, Int32Type, UInt8Type, UInt16Type};
    let context = || format!("square {}:{}", square.x, square.y);
    let Some(table) = dev4.table(square, "roads.arrow").map_err(|e| anyhow!(e))? else {
        return Ok(Vec::new());
    };
    for (key, value) in [
        ("grid", "z30"),
        ("roads_contract", "country_baked_v1"),
        ("road_traffic_contract", "1"),
    ] {
        require_stamp(&table, key, value)
            .map_err(|e| anyhow!(e))
            .with_context(context)?;
    }
    let mut lengths = Vec::new();
    let mut any_measured = false;
    for batch in &table.batches {
        let class = column(batch, "road_class")?.as_primitive::<UInt8Type>();
        let length = column(batch, "length_m")?.as_primitive::<Float32Type>();
        lengths.extend(
            (0..batch.num_rows()).map(|row| (class.value(row), f64::from(length.value(row)))),
        );
        let measured = column(batch, "traffic_observation_measured").with_context(context)?;
        any_measured |= measured.as_boolean().true_count() > 0;
    }
    if !any_measured {
        return Ok(Vec::new());
    }
    let local_km = super::road_traffic::local_km(lengths.into_iter());
    let Some(buildings) =
        BuildingTraffic::load(traffic, square, local_km).map_err(|e| anyhow!(e))?
    else {
        return Ok(Vec::new());
    };
    let copies = super::road::copied_counts(&table).map_err(|e| anyhow!(e))?;
    let mut rows = Vec::new();
    let mut first = 0;
    for batch in &table.batches {
        let u8s = |name| column(batch, name).map(|a| a.as_primitive::<UInt8Type>());
        let i32s = |name| column(batch, name).map(|a| a.as_primitive::<Int32Type>());
        let f64s = |name| column(batch, name).map(|a| a.as_primitive::<Float64Type>());
        let (class, built_up, oneway, lanes) = (
            u8s("road_class")?,
            u8s("built_up")?,
            u8s("oneway")?,
            u8s("lanes")?,
        );
        let measured = column(batch, "traffic_observation_measured")?.as_boolean();
        let design = column(batch, "traffic_observation_design")?.as_primitive::<UInt8Type>();
        let source = column(batch, "source_id")?.as_primitive::<UInt16Type>();
        let country = column(batch, "country_iso")?.as_primitive::<UInt16Type>();
        let length = column(batch, "length_m")?.as_primitive::<Float32Type>();
        let reference = column(batch, "ref")?.as_string::<i32>();
        let aadt = [
            f64s("aadt_light")?,
            f64s("aadt_medium")?,
            f64s("aadt_heavy")?,
            f64s("aadt_moto")?,
        ];
        let ends = [
            i32s("start_gx")?,
            i32s("start_gy")?,
            i32s("end_gx")?,
            i32s("end_gy")?,
        ];
        for row in 0..batch.num_rows() {
            let class = class.value(row);
            let built_up = built_up.value(row);
            let total: f64 = aadt.iter().map(|counts| counts.value(row)).sum();
            let status = (
                measured.value(row),
                source.value(row),
                copies.contains(&(first + row)),
            );
            if !enters_fit(
                class,
                status,
                built_up,
                (total, f64::from(length.value(row))),
            ) {
                continue;
            }
            let middle = (
                ((i64::from(ends[0].value(row)) + i64::from(ends[2].value(row))) / 2) as i32,
                ((i64::from(ends[1].value(row)) + i64::from(ends[3].value(row))) / 2) as i32,
            );
            rows.push(CountedRow {
                country: country.value(row).to_le_bytes(),
                class,
                built_up,
                oneway: oneway.value(row) != 0,
                lanes: lanes.value(row),
                rural_network: thai_rural_road_ref(reference.value(row)),
                design: SamplingDesign::from_code(design.value(row))
                    .ok_or_else(|| anyhow!("unknown sampling design {}", design.value(row)))?,
                length_m: f64::from(length.value(row)),
                total,
                around: RADII_M.map(|radius| buildings.around(middle, radius)),
            });
        }
        first += batch.num_rows();
    }
    Ok(rows)
}

/// The counted rows of every square of the prepared tree whose buildings are mapped, and the
/// squares that hold them.
pub(crate) fn collect(inputs: &YearInputs) -> Result<(Vec<CountedRow>, Vec<[u32; 2]>)> {
    let prepared = PathBuf::from(inputs.required(Kind::Prepared).map_err(|e| anyhow!(e))?);
    let traffic = PathBuf::from(inputs.required(Kind::RoadTraffic).map_err(|e| anyhow!(e))?);
    let squares: Vec<Square> = crate::roads::prepared_squares(&prepared)?;
    let dev4 = Dev4 {
        prepared,
        rasters: PathBuf::new(),
    };
    let parts: Vec<(Square, Vec<CountedRow>)> = squares
        .par_iter()
        .map(|&square| counted_rows(&dev4, &traffic, square).map(|rows| (square, rows)))
        .collect::<Result<_>>()?;
    let held: Vec<[u32; 2]> = parts
        .iter()
        .filter(|(_, rows)| !rows.is_empty())
        .map(|(square, _)| [square.x, square.y])
        .collect();
    Ok((parts.into_iter().flat_map(|(_, rows)| rows).collect(), held))
}
