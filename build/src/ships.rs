//! Complete ship occupancy for selected squares and their halo, bound to the exact year manifest.
use crate::dev4::{Table, read_table};
use crate::ship_fetch::{
    self, Cell,
    grid::{Coverage, Grid, NativeGrid},
};
use crate::year::{Kind, YearInputs};
use arrow_array::{ArrayRef, Float32Array, Int32Array, RecordBatch, UInt16Array};
use arrow_schema::{DataType, Field, Schema};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tiles::square::Square;

pub fn path(root: &Path, square: Square) -> PathBuf {
    root.join(square.x.to_string())
        .join(square.y.to_string())
        .join("ship-occupancy.arrow")
}

pub fn table(root: &Path, square: Square) -> Result<Table, String> {
    let path = path(root, square);
    read_table(&path)?.ok_or_else(|| format!("{}: missing ship occupancy square", path.display()))
}

pub fn grids(table: &Table) -> Result<BTreeMap<u16, Grid>, String> {
    let grids: BTreeMap<u16, NativeGrid> = serde_json::from_str(
        table
            .metadata
            .get("native_grids")
            .ok_or("ship occupancy lacks native grids")?,
    )
    .map_err(|e| e.to_string())?;
    grids
        .into_iter()
        .map(|(id, grid)| Ok((id, Grid::new(grid)?)))
        .collect()
}

pub fn input_root(inputs: &YearInputs) -> Result<&Path, String> {
    let root = inputs.required(Kind::Ships)?;
    inputs.check_complete(root)?;
    Ok(root)
}

pub fn check_squares(root: &Path, squares: &[Square]) -> Result<(), String> {
    let listed: Vec<[u32; 2]> =
        serde_json::from_str(&tiles::year_completion_note(root)?).map_err(|e| e.to_string())?;
    let covered: HashSet<Square> = listed.into_iter().map(|[x, y]| Square { x, y }).collect();
    if squares
        .iter()
        .flat_map(|s| s.with_neighbours())
        .any(|s| !covered.contains(&s))
    {
        return Err(
            "ship occupancy does not cover the requested squares and their neighbours".into(),
        );
    }
    for square in covered {
        let file = path(root, square);
        if !file.is_file() {
            return Err(format!(
                "{}: missing completed ship occupancy square",
                file.display()
            ));
        }
    }
    Ok(())
}

pub fn build(inputs: &YearInputs, squares: &[Square], out: &Path) -> Result<usize, String> {
    if out.exists()
        && std::fs::read_dir(out)
            .map_err(|e| e.to_string())?
            .next()
            .is_some()
    {
        return Err("ship occupancy requires a fresh output directory".into());
    }
    let mut selected = Vec::new();
    for input in &inputs.manifest.inputs {
        if input.kind == Kind::PrepareShips && inputs.manifest.admitted(&input.id)? {
            let root = inputs
                .paths(Kind::PrepareShips)?
                .iter()
                .find(|p| p.file_name().is_some_and(|name| name == input.id.as_str()))
                .ok_or("missing admitted ship root")?;
            selected.push((ship_fetch::specification(input)?, root, input));
        }
    }
    if selected.is_empty() {
        return Err("no admitted ship observations".into());
    }
    selected.sort_by_key(|(spec, _, _)| std::cmp::Reverse(spec.priority));
    let mut ids = HashSet::new();
    for (index, (spec, _, _)) in selected.iter().enumerate() {
        if !ids.insert(spec.source_id)
            || (index > 0 && selected[index - 1].0.priority == spec.priority)
        {
            return Err("ship inputs need distinct source ids and coverage priorities".into());
        }
        if !spec.has_coverage() && index + 1 < selected.len() {
            return Err("ship input without coverage must have the lowest priority".into());
        }
    }
    let mut loaded: Vec<_> = squares.iter().flat_map(|s| s.with_neighbours()).collect();
    loaded.sort();
    loaded.dedup();
    let selection: HashSet<Square> = loaded.iter().copied().collect();
    let mut prior: Vec<Coverage> = Vec::new();
    let mut cells: BTreeMap<Square, Vec<Cell>> = BTreeMap::new();
    let mut grids = BTreeMap::new();
    for (spec, root, input) in selected {
        let normalized = ship_fetch::read(&spec, root, inputs, input, &selection)?;
        grids.insert(spec.source_id, normalized.grid);
        for (square, observation) in normalized.observations {
            if !prior
                .iter()
                .any(|coverage| coverage.covers(observation.longitude, observation.latitude))
            {
                cells.entry(square).or_default().push(observation.cell);
            }
        }
        if let Some(coverage) = normalized.coverage {
            prior.push(coverage);
        }
    }
    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
    for &square in &loaded {
        let rows = cells
            .get_mut(&square)
            .map(Vec::as_mut_slice)
            .unwrap_or_default();
        // Source iteration contributes to stable packed output; preserve the producer's block order.
        rows.sort_by_key(|cell| {
            ((1i64 << 30) - 1 - i64::from(cell.centroid.1)) >> 16 << 14
                | (i64::from(cell.centroid.0) >> 16)
        });
        write(&path(out, square), rows, &grids)?;
    }
    let manifest = std::fs::read(&inputs.manifest_path).map_err(|e| e.to_string())?;
    let listed: Vec<_> = loaded.iter().map(|s| [s.x, s.y]).collect();
    tiles::mark_year_complete(
        out,
        &manifest,
        &serde_json::to_string(&listed).map_err(|e| e.to_string())?,
    )?;
    Ok(cells.values().map(Vec::len).sum())
}

fn write(path: &Path, cells: &[Cell], grids: &BTreeMap<u16, NativeGrid>) -> Result<(), String> {
    let schema = Arc::new(Schema::new_with_metadata(
        vec![
            Field::new("centroid_gx", DataType::Int32, false),
            Field::new("centroid_gy", DataType::Int32, false),
            Field::new("area_m2", DataType::Float32, false),
            Field::new("hours_large", DataType::Float32, false),
            Field::new("hours_work", DataType::Float32, false),
            Field::new("hours_leisure", DataType::Float32, false),
            Field::new("source_id", DataType::UInt16, false),
        ],
        HashMap::from([
            ("grid".into(), "z30".into()),
            ("ships_contract".into(), "ships_v1".into()),
            (
                "native_grids".into(),
                serde_json::to_string(grids).map_err(|e| e.to_string())?,
            ),
        ]),
    ));
    let mut columns: Vec<ArrayRef> = vec![
        Arc::new(Int32Array::from_iter_values(
            cells.iter().map(|cell| cell.centroid.0),
        )),
        Arc::new(Int32Array::from_iter_values(
            cells.iter().map(|cell| cell.centroid.1),
        )),
        Arc::new(Float32Array::from_iter_values(
            cells.iter().map(|cell| cell.area_m2),
        )),
    ];
    for class in 0..3 {
        columns.push(Arc::new(Float32Array::from_iter_values(
            cells.iter().map(|cell| cell.hours[class]),
        )));
    }
    columns.push(Arc::new(UInt16Array::from_iter_values(
        cells.iter().map(|cell| cell.source_id),
    )));
    let batch = RecordBatch::try_new(schema.clone(), columns).map_err(|error| error.to_string())?;
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|error| error.to_string())?;
    let temporary = path.with_extension("tmp");
    let mut writer = arrow_ipc::writer::FileWriter::try_new(
        std::fs::File::create(&temporary).map_err(|error| error.to_string())?,
        &schema,
    )
    .map_err(|error| error.to_string())?;
    writer.write(&batch).map_err(|error| error.to_string())?;
    writer.finish().map_err(|error| error.to_string())?;
    writer
        .into_inner()
        .map_err(|error| error.to_string())?
        .sync_all()
        .map_err(|error| error.to_string())?;
    std::fs::rename(temporary, path).map_err(|error| error.to_string())
}

#[cfg(test)]
#[path = "ships_tests.rs"]
mod tests;
