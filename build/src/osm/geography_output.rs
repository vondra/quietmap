//! Bake country identity on extracted rows and the square's representative country record.

use super::{
    geography::{Identity, Regions},
    geometry,
};
use anyhow::{Result, anyhow, ensure};
use arrow_array::{Array, Int32Array, RecordBatch, UInt16Array};
use arrow_ipc::{reader::FileReader, writer::FileWriter};
use arrow_schema::{DataType, Field, Schema};
use std::{collections::BTreeMap, fs::File, path::Path, sync::Arc};

pub fn bake(regions: &Regions, squares: &[crate::dev4::Square], root: &Path) -> Result<()> {
    for square in squares {
        let directory = root.join(format!("z9/{}/{}", square.x, square.y));
        std::fs::create_dir_all(&directory)?;
        for family in ["roads", "railways", "industrial", "barriers"] {
            let path = directory.join(format!("{family}.arrow"));
            if !path.exists() {
                continue;
            }
            let reader = FileReader::try_new(File::open(&path)?, None)?;
            let temporary = path.with_extension("arrow.tmp");
            let mut writer = None;
            for batch in reader {
                let batch = bake_batch(regions, family, batch?)?;
                let writer = match writer.as_mut() {
                    Some(writer) => writer,
                    None => writer.insert(FileWriter::try_new(
                        File::create(&temporary)?,
                        &batch.schema(),
                    )?),
                };
                writer.write(&batch)?;
            }
            writer
                .ok_or_else(|| anyhow!("empty Arrow file {}", path.display()))?
                .finish()?;
            std::fs::rename(temporary, path)?;
        }
        let identity = square_country(regions, square.x, square.y);
        let mut record = Vec::with_capacity(13);
        let mut morton = 0u64;
        for bit in 0..9 {
            morton |= u64::from((square.x >> bit) & 1) << (bit * 2);
            morton |= u64::from((square.y >> bit) & 1) << (bit * 2 + 1);
        }
        record.extend(morton.to_le_bytes());
        record.push(identity.continent);
        record.extend(identity.country.to_le_bytes());
        record.extend(0u16.to_le_bytes());
        std::fs::write(directory.join("square-country-city.bin"), record)?;
    }
    Ok(())
}

fn coordinates(batch: &RecordBatch, prefix: &str, row: usize) -> Result<(f64, f64)> {
    let coordinate = |axis| -> Result<i32> {
        let name = format!("{prefix}_{axis}");
        let values = batch
            .column_by_name(&name)
            .and_then(|column| column.as_any().downcast_ref::<Int32Array>())
            .ok_or_else(|| anyhow!("invalid {name}"))?;
        ensure!(!values.is_null(row), "null {name}");
        Ok(values.value(row))
    };
    let (x, y) = geometry::grid_to_meters(coordinate("gx")?, coordinate("gy")?);
    let (longitude, latitude) = geometry::poly::meters_to_lonlat(x, y);
    Ok((latitude, longitude))
}

fn bake_batch(regions: &Regions, family: &str, batch: RecordBatch) -> Result<RecordBatch> {
    let mut countries = Vec::with_capacity(batch.num_rows());
    for row in 0..batch.num_rows() {
        let (latitude, longitude) = if family == "industrial" {
            coordinates(&batch, "centroid", row)?
        } else {
            let (start_lat, start_lon) = coordinates(&batch, "start", row)?;
            let (end_lat, end_lon) = coordinates(&batch, "end", row)?;
            (
                (start_lat + end_lat) / 2.0,
                geometry::wrapped_longitude_midpoint(start_lon, end_lon),
            )
        };
        countries.push(
            regions
                .resolve(latitude, longitude, family != "industrial")
                .country,
        );
    }
    let mut fields: Vec<_> = batch
        .schema()
        .fields()
        .iter()
        .map(|field| field.as_ref().clone())
        .collect();
    let mut columns = batch.columns().to_vec();
    fields.push(Field::new("country_iso", DataType::UInt16, false));
    columns.push(Arc::new(UInt16Array::from(countries)));
    let mut metadata = batch.schema().metadata().clone();
    metadata.insert(
        format!("{family}_contract"),
        if family == "industrial" {
            "country_land_baked_v1"
        } else {
            "country_baked_v1"
        },
    );
    Ok(RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(fields, metadata)),
        columns,
    )?)
}

fn square_country(regions: &Regions, x: u32, y: u32) -> Identity {
    let latitude = |row: u32| {
        (std::f64::consts::PI * (1.0 - 2.0 * f64::from(row) / 512.0))
            .sinh()
            .atan()
            .to_degrees()
    };
    let (west, east) = (
        f64::from(x) / 512.0 * 360.0 - 180.0,
        f64::from(x + 1) / 512.0 * 360.0 - 180.0,
    );
    let (north, south) = (latitude(y), latitude(y + 1));
    let center = regions.resolve((north + south) / 2.0, (west + east) / 2.0, true);
    if center.country != 0 {
        return center;
    }
    let mut shares = BTreeMap::<[u8; 2], (usize, Identity)>::new();
    for fraction_y in [0.0, 1.0 / 3.0, 0.5, 2.0 / 3.0, 1.0] {
        for fraction_x in [0.0, 1.0 / 3.0, 0.5, 2.0 / 3.0, 1.0] {
            let identity = regions.resolve(
                south + (north - south) * fraction_y,
                west + (east - west) * fraction_x,
                true,
            );
            if identity.country != 0 {
                shares
                    .entry(identity.country.to_le_bytes())
                    .or_insert((0, identity))
                    .0 += 1;
            }
        }
    }
    shares
        .into_iter()
        .min_by_key(|(iso, (count, _))| (std::cmp::Reverse(*count), *iso))
        .map(|(_, (_, identity))| identity)
        .unwrap_or_default()
}
