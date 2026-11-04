//! The single Arrow road-observation contract, bound to its manifest and original input intervals.

use super::observation::{CountBasis, Observation, TimeBasis};
use crate::year::{Kind, YearInputs};
use anyhow::{Result, anyhow, ensure};
use arrow_array::{
    Array, ArrayRef, BinaryArray, BooleanArray, Float64Array, RecordBatch, StringArray, UInt8Array,
    UInt16Array, UInt32Array,
};
use arrow_schema::{DataType, Field, Schema};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    fs::File,
    path::Path,
    sync::Arc,
};

fn schema(inputs: &YearInputs) -> Result<Schema> {
    let mut fields = Vec::new();
    for name in [
        "identity",
        "input_id",
        "observed_start",
        "observed_end",
        "reference",
    ] {
        fields.push(Field::new(name, DataType::Utf8, false));
    }
    for name in ["source_id", "country", "road_class_mask"] {
        fields.push(Field::new(name, DataType::UInt16, false));
    }
    fields.push(Field::new("source_rank", DataType::UInt32, false));
    for name in ["measured", "reference_required"] {
        fields.push(Field::new(name, DataType::Boolean, false));
    }
    for name in [
        "count_basis",
        "time_basis",
        "direction",
        "estimated_classes",
    ] {
        fields.push(Field::new(name, DataType::UInt8, false));
    }
    for name in ["search_radius_m", "daily_factor"] {
        fields.push(Field::new(name, DataType::Float64, false));
    }
    for name in ["light", "medium", "heavy", "motorcycle", "total"] {
        fields.push(Field::new(name, DataType::Float64, true));
    }
    fields.push(Field::new("geometry", DataType::Binary, false));
    fields.push(Field::new("target", DataType::Utf8, false));
    fields.push(Field::new("sample", DataType::Utf8, false));
    fields.push(Field::new("vehicle_groups", DataType::Utf8, false));
    Ok(Schema::new_with_metadata(
        fields,
        HashMap::from([
            ("road-observations".into(), "1".into()),
            ("geometry".into(), "wgs84-multipart-f64le".into()),
            (
                "manifest-sha256".into(),
                format!(
                    "{:x}",
                    Sha256::digest(std::fs::read(&inputs.manifest_path)?)
                ),
            ),
        ]),
    ))
}

fn encode_geometry(parts: &[Vec<[f64; 2]>]) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    bytes.extend(u32::try_from(parts.len())?.to_le_bytes());
    for part in parts {
        bytes.extend(u32::try_from(part.len())?.to_le_bytes());
        for point in part {
            for value in point {
                bytes.extend(value.to_le_bytes());
            }
        }
    }
    Ok(bytes)
}

fn decode_geometry(mut bytes: &[u8]) -> Result<Vec<Vec<[f64; 2]>>> {
    fn count(bytes: &mut &[u8]) -> Result<usize> {
        let value = u32::from_le_bytes(
            bytes
                .get(..4)
                .ok_or_else(|| anyhow!("short geometry"))?
                .try_into()?,
        );
        *bytes = &bytes[4..];
        Ok(value as usize)
    }
    let part_count = count(&mut bytes)?;
    ensure!(part_count <= bytes.len() / 4, "invalid geometry part count");
    let mut parts = Vec::with_capacity(part_count);
    for _ in 0..part_count {
        let point_count = count(&mut bytes)?;
        ensure!(
            point_count > 0 && point_count <= bytes.len() / 16,
            "invalid geometry point count"
        );
        let mut points = Vec::with_capacity(point_count);
        for _ in 0..point_count {
            points.push([
                f64::from_le_bytes(bytes[..8].try_into()?),
                f64::from_le_bytes(bytes[8..16].try_into()?),
            ]);
            bytes = &bytes[16..];
        }
        parts.push(points);
    }
    ensure!(bytes.is_empty(), "trailing geometry bytes");
    Ok(parts)
}

pub fn validate(rows: &[Observation], inputs: &YearInputs) -> Result<()> {
    let cutoff = chrono::NaiveDate::parse_from_str(&inputs.manifest.as_of, "%Y-%m-%d")?;
    let mut identities = BTreeMap::new();
    for row in rows {
        row.validate(cutoff)?;
        let input = inputs
            .manifest
            .inputs
            .iter()
            .find(|i| i.id == row.input_id)
            .ok_or_else(|| anyhow!("unknown observation input {}", row.input_id))?;
        ensure!(
            matches!(
                input.kind,
                Kind::PrepareRoadCounts | Kind::PrepareRoadHourly | Kind::PrepareRoadNetwork
            ) && inputs
                .manifest
                .admitted(&input.id)
                .map_err(anyhow::Error::msg)?,
            "unadmitted road observation input {}",
            input.id
        );
        let (start, end) = input.observed.bounds().map_err(anyhow::Error::msg)?;
        let from = chrono::NaiveDate::parse_from_str(&row.observed_start, "%Y-%m-%d")?;
        let to = chrono::NaiveDate::parse_from_str(&row.observed_end, "%Y-%m-%d")?;
        ensure!(
            from >= start && to <= end,
            "observation {} exceeds its declared input interval",
            row.identity
        );
        let key = (
            row.source_id,
            &row.identity,
            &row.observed_start,
            &row.observed_end,
            row.direction,
            row.time_basis as u8,
        );
        ensure!(
            identities.insert(key, row).is_none(),
            "duplicate observation identity {} in {} (source {})",
            row.identity,
            row.input_id,
            row.source_id
        );
    }
    Ok(())
}

pub fn write(path: &Path, rows: &[Observation], inputs: &YearInputs) -> Result<()> {
    validate(rows, inputs)?;
    let schema = Arc::new(schema(inputs)?);
    let mut writer = arrow_ipc::writer::FileWriter::try_new(
        File::options().write(true).create_new(true).open(path)?,
        &schema,
    )?;
    for rows in rows.chunks(8192) {
        let mut columns: Vec<ArrayRef> = Vec::new();
        macro_rules! col {
            ($array:ident, $field:ident) => {
                columns.push(Arc::new($array::from_iter_values(
                    rows.iter().map(|r| r.$field),
                )));
            };
        }
        let text_fields: [fn(&Observation) -> &str; 5] = [
            |r: &Observation| r.identity.as_str(),
            |r: &Observation| r.input_id.as_str(),
            |r: &Observation| r.observed_start.as_str(),
            |r: &Observation| r.observed_end.as_str(),
            |r: &Observation| r.reference.as_str(),
        ];
        for pick in text_fields {
            columns.push(Arc::new(StringArray::from_iter_values(
                rows.iter().map(pick),
            )));
        }
        col!(UInt16Array, source_id);
        col!(UInt16Array, country);
        col!(UInt16Array, road_class_mask);
        col!(UInt32Array, source_rank);
        for pick in [
            |r: &Observation| r.measured,
            |r: &Observation| r.reference_required,
        ] {
            columns.push(Arc::new(BooleanArray::from(
                rows.iter().map(pick).collect::<Vec<_>>(),
            )));
        }
        columns.push(Arc::new(UInt8Array::from_iter_values(
            rows.iter().map(|r| r.count_basis as u8),
        )));
        columns.push(Arc::new(UInt8Array::from_iter_values(
            rows.iter().map(|r| r.time_basis as u8),
        )));
        col!(UInt8Array, direction);
        col!(UInt8Array, estimated_classes);
        col!(Float64Array, search_radius_m);
        col!(Float64Array, daily_factor);
        for index in 0..4 {
            columns.push(Arc::new(Float64Array::from_iter(
                rows.iter().map(|r| r.vehicle_classes[index]),
            )));
        }
        columns.push(Arc::new(Float64Array::from_iter(
            rows.iter().map(|r| r.total),
        )));
        let geometry = rows
            .iter()
            .map(|r| encode_geometry(&r.geometry))
            .collect::<Result<Vec<_>>>()?;
        columns.push(Arc::new(BinaryArray::from_iter_values(
            geometry.iter().map(Vec::as_slice),
        )));
        let targets = rows
            .iter()
            .map(|r| serde_json::to_string(&r.target))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        columns.push(Arc::new(StringArray::from_iter_values(targets)));
        let samples = rows
            .iter()
            .map(|r| serde_json::to_string(&r.sample))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        columns.push(Arc::new(StringArray::from_iter_values(samples)));
        let groups = rows
            .iter()
            .map(|r| serde_json::to_string(&r.vehicle_groups))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        columns.push(Arc::new(StringArray::from_iter_values(groups)));
        writer.write(&RecordBatch::try_new(schema.clone(), columns)?)?;
    }
    writer.finish()?;
    writer.into_inner()?.sync_all()?;
    Ok(())
}

pub fn read(path: &Path, inputs: &YearInputs) -> Result<Vec<Observation>> {
    let reader = arrow_ipc::reader::FileReader::try_new(File::open(path)?, None)?;
    ensure!(
        reader.schema().as_ref() == &schema(inputs)?,
        "observation schema or manifest differs"
    );
    let mut rows = Vec::new();
    for batch in reader {
        let batch = batch?;
        for (field, column) in batch.schema().fields().iter().zip(batch.columns()) {
            ensure!(
                field.is_nullable() || column.null_count() == 0,
                "null observation {}",
                field.name()
            );
        }
        for index in 0..batch.num_rows() {
            macro_rules! value {
                ($array:ident, $name:literal) => {
                    crate::dev4::column::<$array>(&batch, $name)
                        .map_err(anyhow::Error::msg)?
                        .value(index)
                };
            }
            let mut counts = [None; 5];
            for (i, name) in ["light", "medium", "heavy", "motorcycle", "total"]
                .iter()
                .enumerate()
            {
                let column = crate::dev4::column::<Float64Array>(&batch, name)
                    .map_err(anyhow::Error::msg)?;
                counts[i] = column.is_valid(index).then(|| column.value(index));
            }
            let count_basis = match value!(UInt8Array, "count_basis") {
                0 => CountBasis::Unknown,
                1 => CountBasis::Directional,
                2 => CountBasis::BothDirections,
                3 => CountBasis::Allocated,
                4 => CountBasis::StreetCrossSection,
                _ => return Err(anyhow!("invalid count basis")),
            };
            let time_basis = match value!(UInt8Array, "time_basis") {
                0 => TimeBasis::AnnualDaily,
                1 => TimeBasis::WeekdayDaily,
                2 => TimeBasis::Hourly,
                3 => TimeBasis::PeriodDaily,
                _ => return Err(anyhow!("invalid time basis")),
            };
            rows.push(Observation {
                identity: value!(StringArray, "identity").into(),
                input_id: value!(StringArray, "input_id").into(),
                observed_start: value!(StringArray, "observed_start").into(),
                observed_end: value!(StringArray, "observed_end").into(),
                reference: value!(StringArray, "reference").into(),
                source_id: value!(UInt16Array, "source_id"),
                source_rank: value!(UInt32Array, "source_rank"),
                country: value!(UInt16Array, "country"),
                road_class_mask: value!(UInt16Array, "road_class_mask"),
                measured: value!(BooleanArray, "measured"),
                reference_required: value!(BooleanArray, "reference_required"),
                count_basis,
                time_basis,
                direction: value!(UInt8Array, "direction"),
                estimated_classes: value!(UInt8Array, "estimated_classes"),
                search_radius_m: value!(Float64Array, "search_radius_m"),
                daily_factor: value!(Float64Array, "daily_factor"),
                vehicle_classes: counts[..4].try_into()?,
                vehicle_groups: serde_json::from_str(value!(StringArray, "vehicle_groups"))?,
                total: counts[4],
                geometry: decode_geometry(value!(BinaryArray, "geometry"))?,
                target: serde_json::from_str(value!(StringArray, "target"))?,
                sample: serde_json::from_str(value!(StringArray, "sample"))?,
            });
        }
    }
    validate(&rows, inputs)?;
    Ok(rows)
}
