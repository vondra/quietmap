//! `<year>.railway-ways.arrow` and `<year>.train-routes.arrow`: whole railway ways and train routes, keyed by bare OSM id.

use crate::transport::{year_sibling_path, RAILWAY_WAYS_SPILL, TRAIN_ROUTES_SPILL};
use anyhow::{ensure, Context, Result};
use arrow::array::{
    ArrayRef, Float64Builder, Int32Builder, Int64Array, Int64Builder, ListBuilder, StringBuilder,
    UInt32Builder, UInt8Builder,
};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::ipc::writer::StreamWriter;
use arrow::record_batch::RecordBatch;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::sync::Arc;

fn id_sorted_spill_lines(path: &Path) -> Result<Vec<(i64, String)>> {
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut lines = Vec::new();
    for line in BufReader::with_capacity(1 << 20, file).lines() {
        let line = line?;
        let (id, rest) = line
            .split_once('\t')
            .with_context(|| format!("malformed row {line:?} in {}", path.display()))?;
        lines.push((id.parse::<i64>()?, rest.to_owned()));
    }
    lines.sort_unstable_by_key(|line| line.0);
    ensure!(
        lines.windows(2).all(|pair| pair[0].0 < pair[1].0),
        "repeated id in {}",
        path.display()
    );
    Ok(lines)
}

fn write_with_contract(
    path: &Path,
    contract_key: &str,
    columns: Vec<(&str, ArrayRef)>,
) -> Result<()> {
    let batch = RecordBatch::try_from_iter(columns)?;
    let schema = Schema::new(batch.schema().fields().clone())
        .with_metadata(HashMap::from([(contract_key.to_owned(), "1".to_owned())]));
    super::write_single_batch_arrow(path, batch.with_schema(Arc::new(schema))?)
}

/// A reader holds one record batch at a time, and Node cannot hold the world file (over 2 GiB) as
/// one buffer: a batch closes at this many list values (about 24 B each).
const RAILWAY_WAYS_BATCH_LIST_VALUES: usize = 1 << 20;

struct RailwayWaysBatch {
    way_id: Int64Builder,
    node_id: ListBuilder<Int64Builder>,
    lat_e7: ListBuilder<Int32Builder>,
    lon_e7: ListBuilder<Int32Builder>,
    node_m: ListBuilder<Float64Builder>,
    square_key: ListBuilder<UInt32Builder>,
    schema: Arc<Schema>,
}

impl RailwayWaysBatch {
    fn new() -> Self {
        let list = |name: &str, item: DataType, nullable: bool| {
            let item = Arc::new(Field::new("item", item, true));
            Field::new(name, DataType::List(item), nullable)
        };
        let schema = Schema::new(vec![
            Field::new("way_id", DataType::Int64, false),
            list("node_id", DataType::Int64, false),
            list("lat_e7", DataType::Int32, false),
            list("lon_e7", DataType::Int32, false),
            list("node_m", DataType::Float64, true),
            list("square_key", DataType::UInt32, false),
        ])
        .with_metadata(HashMap::from([(
            "railway_ways_contract".to_owned(),
            "1".to_owned(),
        )]));
        Self {
            way_id: Int64Builder::new(),
            node_id: ListBuilder::new(Int64Builder::new()),
            lat_e7: ListBuilder::new(Int32Builder::new()),
            lon_e7: ListBuilder::new(Int32Builder::new()),
            node_m: ListBuilder::new(Float64Builder::new()),
            square_key: ListBuilder::new(UInt32Builder::new()),
            schema: Arc::new(schema),
        }
    }

    /// Finishing resets every builder, so one value serves all batches of the file.
    fn finish(&mut self) -> Result<RecordBatch> {
        Ok(RecordBatch::try_new(
            self.schema.clone(),
            vec![
                Arc::new(self.way_id.finish()) as ArrayRef,
                Arc::new(self.node_id.finish()),
                Arc::new(self.lat_e7.finish()),
                Arc::new(self.lon_e7.finish()),
                Arc::new(self.node_m.finish()),
                Arc::new(self.square_key.finish()),
            ],
        )?)
    }
}

/// Canonical node ids in original order, e7 coordinates (null = unresolved node), cumulative
/// metres per node (null list on an incomplete way) and the squares holding the way's pieces.
pub fn write_railway_ways(
    spill_dir: &Path,
    output_dir: &Path,
    aliases: &HashMap<i64, i64>,
) -> Result<()> {
    write_railway_ways_in_batches(
        spill_dir,
        output_dir,
        aliases,
        RAILWAY_WAYS_BATCH_LIST_VALUES,
    )
}

pub(crate) fn write_railway_ways_in_batches(
    spill_dir: &Path,
    output_dir: &Path,
    aliases: &HashMap<i64, i64>,
    batch_list_values: usize,
) -> Result<()> {
    let lines = id_sorted_spill_lines(&spill_dir.join(RAILWAY_WAYS_SPILL))?;
    let mut batch = RailwayWaysBatch::new();
    // An Arrow IPC *stream*: the format made for sequential batch-by-batch reading, which needs no footer.
    let path = year_sibling_path(output_dir, "railway-ways")?;
    let tmp_path = path.with_extension("arrow.tmp");
    let mut writer = StreamWriter::try_new(File::create(&tmp_path)?, &batch.schema)?;
    let mut list_values = 0;
    for (way_id, rest) in &lines {
        list_values += append_railway_way(&mut batch, *way_id, rest, aliases)?;
        if list_values >= batch_list_values {
            writer.write(&batch.finish()?)?;
            list_values = 0;
        }
    }
    if list_values > 0 {
        writer.write(&batch.finish()?)?;
    }
    writer.finish()?;
    writer.get_ref().sync_all()?;
    std::fs::rename(&tmp_path, &path)?;
    Ok(())
}

/// Returns the list values the way added (at least one, so a batch also bounds its rows).
fn append_railway_way(
    batch: &mut RailwayWaysBatch,
    way_id: i64,
    rest: &str,
    aliases: &HashMap<i64, i64>,
) -> Result<usize> {
    let (nodes, squares) = rest
        .split_once('\t')
        .with_context(|| format!("malformed railway way {way_id}"))?;
    batch.way_id.append_value(way_id);
    let mut points = Vec::new();
    let mut node_count = 0;
    for node in nodes.split(';').filter(|node| !node.is_empty()) {
        let fields: Vec<&str> = node.split(',').collect();
        ensure!(
            fields.len() == 3,
            "malformed node {node:?} of railway way {way_id}"
        );
        let raw: i64 = fields[0].parse()?;
        batch
            .node_id
            .values()
            .append_value(aliases.get(&raw).copied().unwrap_or(raw));
        node_count += 1;
        if fields[1].is_empty() {
            batch.lat_e7.values().append_null();
            batch.lon_e7.values().append_null();
        } else {
            let (lat, lon): (i32, i32) = (fields[1].parse()?, fields[2].parse()?);
            batch.lat_e7.values().append_value(lat);
            batch.lon_e7.values().append_value(lon);
            points.push([lat as f64 / 1e7, lon as f64 / 1e7]);
        }
    }
    batch.node_id.append(true);
    batch.lat_e7.append(true);
    batch.lon_e7.append(true);
    let complete = node_count >= 2 && points.len() == node_count;
    if complete {
        batch
            .node_m
            .values()
            .append_slice(&grid::geo::cumulative_flat_metres(0.0, &points));
    }
    batch.node_m.append(complete);
    let mut square_count = 0;
    for square in squares.split(';').filter(|square| !square.is_empty()) {
        batch.square_key.values().append_value(square.parse()?);
        square_count += 1;
    }
    batch.square_key.append(true);
    Ok(1 + node_count + square_count)
}

/// Members stay verbatim in source order: role policy belongs to the reader, not to a nine-hour extract.
pub fn write_train_routes(spill_dir: &Path, output_dir: &Path) -> Result<()> {
    let lines = id_sorted_spill_lines(&spill_dir.join(TRAIN_ROUTES_SPILL))?;
    let mut member_kind = ListBuilder::new(UInt8Builder::new());
    let mut member_id = ListBuilder::new(Int64Builder::new());
    let mut member_role = ListBuilder::new(StringBuilder::new());
    for (route_id, members_json) in &lines {
        let members: Vec<(String, String, String)> = serde_json::from_str(members_json)
            .with_context(|| format!("malformed members of train route {route_id}"))?;
        for (kind, id, role) in members {
            ensure!(
                kind.len() == 1,
                "malformed member kind of train route {route_id}"
            );
            member_kind.values().append_value(kind.as_bytes()[0]);
            member_id.values().append_value(id.parse()?);
            member_role.values().append_value(role);
        }
        member_kind.append(true);
        member_id.append(true);
        member_role.append(true);
    }
    write_with_contract(
        &year_sibling_path(output_dir, "train-routes")?,
        "train_routes_contract",
        vec![
            (
                "osm_id",
                Arc::new(Int64Array::from_iter_values(
                    lines.iter().map(|line| line.0),
                )) as ArrayRef,
            ),
            ("member_kind", Arc::new(member_kind.finish())),
            ("member_id", Arc::new(member_id.finish())),
            ("member_role", Arc::new(member_role.finish())),
        ],
    )
}
