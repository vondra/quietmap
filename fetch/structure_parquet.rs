//! Read selected publisher Parquet columns and skip row groups outside the requested squares.
use super::{Square, bounds};
use anyhow::{Result, ensure};
use arrow_array::RecordBatch;
use parquet::arrow::{ProjectionMask, arrow_reader::ParquetRecordBatchReaderBuilder};
use std::{fs::File, path::Path};

pub fn batches(
    path: &Path,
    columns: &[&str],
    selection: Option<&[Square]>,
    contract: Option<(&str, &str)>,
    mut visit: impl FnMut(RecordBatch) -> Result<()>,
) -> Result<()> {
    let builder = ParquetRecordBatchReaderBuilder::try_new(File::open(path)?)?;
    if let Some((key, version)) = contract {
        ensure!(
            builder.schema().metadata().get(key).map(String::as_str) == Some(version),
            "{}: expected {version}",
            path.display()
        );
    }
    let groups = (0..builder.metadata().num_row_groups())
        .filter(|&i| {
            selection.is_none_or(|squares| {
                row_group_intersects(builder.metadata().row_group(i), squares)
            })
        })
        .collect::<Vec<_>>();
    let roots = builder
        .schema()
        .fields()
        .iter()
        .enumerate()
        .filter(|(_, f)| columns.contains(&f.name().as_str()))
        .map(|(i, _)| i)
        .collect::<Vec<_>>();
    let mask = ProjectionMask::roots(builder.parquet_schema(), roots);
    for batch in builder
        .with_projection(mask)
        .with_row_groups(groups)
        .with_batch_size(8192)
        .build()?
    {
        visit(batch?)?;
    }
    Ok(())
}

fn row_group_intersects(
    group: &parquet::file::metadata::RowGroupMetaData,
    squares: &[Square],
) -> bool {
    use parquet::file::statistics::Statistics;
    let bound = |name: &str, minimum: bool| -> Option<f64> {
        let column = group
            .columns()
            .iter()
            .find(|c| c.column_path().string() == name)?;
        match column.statistics()? {
            Statistics::Float(v) => if minimum { v.min_opt() } else { v.max_opt() }
                .copied()
                .map(f64::from),
            Statistics::Double(v) => if minimum { v.min_opt() } else { v.max_opt() }.copied(),
            _ => None,
        }
    };
    let (Some(west), Some(south), Some(east), Some(north)) = (
        bound("bbox.xmin", true),
        bound("bbox.ymin", true),
        bound("bbox.xmax", false),
        bound("bbox.ymax", false),
    ) else {
        return true;
    };
    if [west, south, east, north].iter().any(|v| !v.is_finite()) {
        return true;
    }
    squares.iter().any(|&square| {
        let b = bounds(square);
        west <= b[2] && east >= b[0] && south <= b[3] && north >= b[1]
    })
}
