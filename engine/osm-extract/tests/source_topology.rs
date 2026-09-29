//! Exercise real PBF extraction across a tile seam, source junctions, zero-length ways and missing nodes.

use arrow::array::{
    Array, Float32Array, Float64Array, Int16Array, Int32Array, Int64Array, ListArray, StringArray,
    UInt16Array, UInt32Array, UInt8Array,
};
use arrow::ipc::reader::FileReader;
use arrow::record_batch::RecordBatch;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

/// String-table entries; index 0 is reserved, so indices are positions here.
const STRINGS: [&str; 14] = [
    "", "railway", "rail", "highway", "trunk", "route", "train", "bus", "stop", "forward",
    "backward", "platform", "unusual", "level_crossing",
];

/// All fixture nodes, ascending by id. Node 999 (referenced by way 15) is
/// deliberately absent: the chain must record it as null.
const NODES: &[(i64, f64, f64)] = &[
    (1, 50.0, 14.061),
    (2, 50.0, 14.0625), // exact z9 seam: x jumps 275 → 276
    (3, 50.0, 14.064),
    (4, 50.0, 14.0625), // same coordinates as node 2, distinct id
    (5, 50.001, 14.0625),
    (6, 49.999, 14.0625),
    (7, 50.0, 14.064), // same coordinates as node 3, distinct id
    (8, 50.0, 14.065),
    (20, 50.003, 14.061),
    (21, 50.003, 14.063),
    (30, 50.004, 14.06),
    (31, 50.004, 14.065),
];

/// (way id, tag key, tag value, node refs). Ways 13 and 15 exercise the two
/// piece-less rows; way 16 is the trunk road with one over-cap hop.
const WAYS: &[(i64, &str, &str, &[i64])] = &[
    (10, "railway", "rail", &[1, 2, 3]),
    (11, "railway", "rail", &[4, 5]),
    (12, "railway", "rail", &[2, 6]),
    (13, "railway", "rail", &[3, 7]),
    (14, "railway", "rail", &[7, 8]),
    (15, "railway", "rail", &[20, 999, 21]),
    (16, "highway", "trunk", &[30, 31]),
];

// ─── Minimal protobuf/PBF encoding (proto2 wire format, raw blobs) ───

fn push_varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn push_field_key(out: &mut Vec<u8>, field: u32, wire: u8) {
    push_varint(out, (u64::from(field) << 3) | u64::from(wire));
}

fn push_bytes_field(out: &mut Vec<u8>, field: u32, payload: &[u8]) {
    push_field_key(out, field, 2);
    push_varint(out, payload.len() as u64);
    out.extend_from_slice(payload);
}

fn push_varint_field(out: &mut Vec<u8>, field: u32, value: u64) {
    push_field_key(out, field, 0);
    push_varint(out, value);
}

/// Packed repeated sint64: zigzag varints of delta-encoded values.
fn push_packed_sint64_deltas(out: &mut Vec<u8>, field: u32, values: &[i64]) {
    let mut payload = Vec::new();
    let mut previous = 0i64;
    for &value in values {
        let delta = value.wrapping_sub(previous);
        push_varint(&mut payload, ((delta << 1) ^ (delta >> 63)) as u64);
        previous = value;
    }
    push_bytes_field(out, field, &payload);
}

/// Packed repeated uint32 (string-table indices).
fn push_packed_uint32(out: &mut Vec<u8>, field: u32, values: &[u32]) {
    let mut payload = Vec::new();
    for &value in values {
        push_varint(&mut payload, u64::from(value));
    }
    push_bytes_field(out, field, &payload);
}

/// One fileblock: big-endian BlobHeader length, BlobHeader{type, datasize},
/// then an uncompressed Blob{raw, raw_size}.
fn write_pbf_blob(out: &mut Vec<u8>, kind: &str, payload: &[u8]) {
    let mut blob = Vec::new();
    push_bytes_field(&mut blob, 1, payload); // Blob.raw
    push_varint_field(&mut blob, 2, payload.len() as u64); // Blob.raw_size
    let mut header = Vec::new();
    push_bytes_field(&mut header, 1, kind.as_bytes()); // BlobHeader.type
    push_varint_field(&mut header, 3, blob.len() as u64); // BlobHeader.datasize
    out.extend_from_slice(&(header.len() as u32).to_be_bytes());
    out.extend_from_slice(&header);
    out.extend_from_slice(&blob);
}

/// Encode the fixture as an uncompressed OSM PBF (header + one data block).
/// `node_tags` carries (node id, key, value) triples for dense nodes; an empty
/// slice leaves the fixture byte-identical to the untagged encoding.
fn write_fixture_pbf(path: &Path, nodes: &[(i64, f64, f64)], node_tags: &[(i64, &str, &str)]) {
    let mut pbf = Vec::new();
    let mut header_block = Vec::new();
    push_bytes_field(&mut header_block, 4, b"OsmSchema-V0.6"); // required_features
    write_pbf_blob(&mut pbf, "OSMHeader", &header_block);

    let mut block = Vec::new();
    let mut table = Vec::new();
    for entry in STRINGS {
        push_bytes_field(&mut table, 1, entry.as_bytes()); // StringTable.s
    }
    push_bytes_field(&mut block, 1, &table); // PrimitiveBlock.stringtable

    // Dense nodes group: delta-coded ids, lats, lons (granularity 100 default).
    let mut dense = Vec::new();
    push_packed_sint64_deltas(
        &mut dense,
        1,
        &nodes.iter().map(|&(id, _, _)| id).collect::<Vec<_>>(),
    );
    push_packed_sint64_deltas(
        &mut dense,
        8,
        &nodes
            .iter()
            .map(|&(_, lat, _)| (lat * 1e7).round() as i64)
            .collect::<Vec<_>>(),
    );
    push_packed_sint64_deltas(
        &mut dense,
        9,
        &nodes
            .iter()
            .map(|&(_, _, lon)| (lon * 1e7).round() as i64)
            .collect::<Vec<_>>(),
    );
    // Dense tags: per node in order, key/val string indices, then a 0 delimiter.
    if !node_tags.is_empty() {
        let mut keys_vals = Vec::new();
        for &(id, _, _) in nodes {
            for &(tag_id, key, value) in node_tags {
                if tag_id != id {
                    continue;
                }
                keys_vals.push(STRINGS.iter().position(|entry| entry == &key).unwrap() as u32);
                keys_vals
                    .push(STRINGS.iter().position(|entry| entry == &value).unwrap() as u32);
            }
            keys_vals.push(0);
        }
        push_packed_uint32(&mut dense, 10, &keys_vals); // DenseNodes.keys_vals
    }
    let mut nodes_group = Vec::new();
    push_bytes_field(&mut nodes_group, 2, &dense); // PrimitiveGroup.dense
    push_bytes_field(&mut block, 2, &nodes_group);

    // Ways group: separate PrimitiveGroup keeps nodes and ways distinct.
    let mut ways_group = Vec::new();
    for &(way_id, key, value, refs) in WAYS {
        let key_index = STRINGS.iter().position(|entry| entry == &key).unwrap() as u32;
        let value_index = STRINGS.iter().position(|entry| entry == &value).unwrap() as u32;
        let mut way = Vec::new();
        push_varint_field(&mut way, 1, way_id as u64); // Way.id
        push_packed_uint32(&mut way, 2, &[key_index]); // Way.keys
        push_packed_uint32(&mut way, 3, &[value_index]); // Way.vals
        push_packed_sint64_deltas(&mut way, 8, refs); // Way.refs
        push_bytes_field(&mut ways_group, 3, &way); // PrimitiveGroup.ways
    }
    push_bytes_field(&mut block, 2, &ways_group);
    let mut relations_group = Vec::new();
    for (id, route_type) in [(1000, 6), (1001, 7)] {
        let mut relation = Vec::new();
        push_varint_field(&mut relation, 1, id);
        push_packed_uint32(&mut relation, 2, &[5]); // route
        push_packed_uint32(&mut relation, 3, &[route_type]); // train or bus
        push_packed_uint32(&mut relation, 8, &[8, 9, 10, 0, 12, 11]); // roles
        push_packed_sint64_deltas(
            &mut relation,
            9,
            &[1, 10, 10, 9_007_199_254_740_993, 2000, 11],
        );
        push_packed_uint32(&mut relation, 10, &[0, 1, 1, 1, 2, 1]); // node, way, relation
        push_bytes_field(&mut relations_group, 4, &relation);
    }
    push_bytes_field(&mut block, 2, &relations_group);
    write_pbf_blob(&mut pbf, "OSMData", &block);

    std::fs::write(path, pbf).expect("write fixture PBF");
}

// ─── Test scaffolding ───

/// Process-unique temporary directory removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static SEQUENCE: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "qm-source-topology-pbf-{tag}-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).expect("create temporary directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn all_rows(path: &Path) -> RecordBatch {
    let reader =
        FileReader::try_new(File::open(path).expect("open arrow file"), None).expect("read IPC");
    let schema = reader.schema();
    let batches: Vec<RecordBatch> = reader.map(|batch| batch.expect("decode batch")).collect();
    arrow::compute::concat_batches(&schema, &batches).expect("concatenate batches")
}

fn column<'a, T: 'static>(batch: &'a RecordBatch, name: &str) -> &'a T {
    batch
        .column_by_name(name)
        .unwrap_or_else(|| panic!("column {name}"))
        .as_any()
        .downcast_ref::<T>()
        .unwrap_or_else(|| panic!("type of {name}"))
}

fn list_values<T: Array + Clone + 'static>(batch: &RecordBatch, name: &str, row: usize) -> T {
    let list = column::<ListArray>(batch, name).value(row);
    list.as_any()
        .downcast_ref::<T>()
        .expect("list item type")
        .clone()
}

fn e7(degrees: f64) -> i32 {
    (degrees * 1e7).round() as i32
}

/// Expected canonical id and e7 coordinates; only the zero-length hop 3→7 (way 13) unions, so
/// node 7 is written as 3 — the coordinate twin 4 (way 11) never touches 2.
fn expected_node(id: i64) -> (i64, Option<(i32, i32)>) {
    let coordinates = NODES
        .iter()
        .find(|&&(node_id, _, _)| node_id == id)
        .map(|&(_, lat, lon)| (e7(lat), e7(lon)));
    (if id == 7 { 3 } else { id }, coordinates)
}

#[test]
fn hand_built_pbf_yields_exact_source_topology_and_matching_arrow_pieces() {
    let root = TempDir::new("run");
    let input = root.path().join("fixture.osm.pbf");
    let output = root.path().join("prepared");
    write_fixture_pbf(&input, NODES, &[]);

    let run = Command::new(env!("CARGO_BIN_EXE_osm-extract"))
        .arg("--input")
        .arg(&input)
        .arg("--output")
        .arg(&output)
        .arg("--node-cache")
        .arg(root.path().join("nodes.cache"))
        .arg("--spill-dir")
        .arg(root.path().join("spill"))
        .arg("--num-buckets")
        .arg("1")
        .env("QM_OSM_ONLY", "roads,railways")
        .env("RAYON_NUM_THREADS", "2")
        .output()
        .expect("spawn osm-extract");
    assert!(
        run.status.success(),
        "osm-extract failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );

    let routes = all_rows(&root.path().join("prepared.train-routes.arrow"));
    assert_eq!(
        column::<Int64Array>(&routes, "osm_id").values(),
        &[1000],
        "non-train relation leaked into train routes"
    );
    assert_eq!(
        list_values::<UInt8Array>(&routes, "member_kind", 0).values(),
        b"nwwwrw"
    );
    assert_eq!(
        list_values::<Int64Array>(&routes, "member_id", 0).values(),
        &[1, 10, 10, 9007199254740993, 2000, 11]
    );
    let roles = list_values::<StringArray>(&routes, "member_role", 0);
    assert_eq!(
        roles.iter().flatten().collect::<Vec<_>>(),
        ["stop", "forward", "backward", "", "unusual", "platform"]
    );

    // Original chains: every railway way is retained with canonical ids, way 15 keeps the null
    // entry for the absent node 999 and so has no metres, way 13 keeps its zero-length pair.
    let ways = arrow::ipc::reader::StreamReader::try_new(
        File::open(root.path().join("prepared.railway-ways.arrow")).expect("open railway ways"),
        None,
    )
    .expect("read IPC stream");
    let ways = arrow::compute::concat_batches(
        &ways.schema(),
        &ways
            .map(|batch| batch.expect("decode batch"))
            .collect::<Vec<_>>(),
    )
    .expect("concatenate batches");
    let railways: Vec<_> = WAYS.iter().filter(|way| way.1 == "railway").collect();
    assert_eq!(
        column::<Int64Array>(&ways, "way_id").values(),
        &railways.iter().map(|way| way.0).collect::<Vec<_>>()[..]
    );
    for (row, (way_id, _, _, refs)) in railways.iter().enumerate() {
        let expected: Vec<_> = refs.iter().map(|&id| expected_node(id)).collect();
        assert_eq!(
            list_values::<Int64Array>(&ways, "node_id", row).values(),
            &expected.iter().map(|node| node.0).collect::<Vec<_>>()[..],
            "node ids of way {way_id}"
        );
        let lat = list_values::<Int32Array>(&ways, "lat_e7", row);
        let lon = list_values::<Int32Array>(&ways, "lon_e7", row);
        let stored: Vec<_> = (0..lat.len())
            .map(|node| {
                lat.is_valid(node)
                    .then(|| (lat.value(node), lon.value(node)))
            })
            .collect();
        assert_eq!(
            stored,
            expected.iter().map(|node| node.1).collect::<Vec<_>>()
        );
        let metres = column::<ListArray>(&ways, "node_m");
        assert_eq!(
            metres.is_valid(row),
            *way_id != 15,
            "metres of way {way_id}"
        );
    }
    let squares = |row| list_values::<UInt32Array>(&ways, "square_key", row);
    assert_eq!(squares(0).values(), &[173 * 512 + 275, 173 * 512 + 276]);
    assert!(squares(3).is_empty() && squares(5).is_empty());
    let way_10_metres = list_values::<Float64Array>(&ways, "node_m", 0);

    // Pieces: junction split at node 2 (vertex 1) across the z9 seam, no bridging piece for
    // way 15, fractional halves for road 16's long hop. Each pieces file pairs with its layer
    // file: same keys, the layer's own cells and length.
    type Piece = (i64, i16, u16, f64, u16, f64, i64, i64);
    let expected: [(&str, &str, Vec<Piece>); 4] = [
        (
            "railways",
            "z9/275/173",
            vec![(10, 0, 0, 0.0, 1, 0.0, 1, 2)],
        ),
        (
            "railways",
            "z9/276/173",
            vec![
                (10, 1, 1, 0.0, 2, 0.0, 2, 3),
                (11, 0, 0, 0.0, 1, 0.0, 4, 5),
                (12, 0, 0, 0.0, 1, 0.0, 2, 6),
                (14, 0, 0, 0.0, 1, 0.0, 3, 8),
            ],
        ),
        ("roads", "z9/275/173", vec![(16, 0, 0, 0.0, 0, 0.5, 30, 30)]),
        ("roads", "z9/276/173", vec![(16, 1, 0, 0.5, 1, 0.0, 30, 31)]),
    ];
    for (family, square, pieces) in expected {
        let batch = all_rows(&output.join(square).join(format!("{family}.pieces.arrow")));
        let stored: Vec<Piece> = (0..batch.num_rows())
            .map(|row| {
                (
                    column::<Int64Array>(&batch, "way_id").value(row),
                    column::<Int16Array>(&batch, "segment_idx").value(row),
                    column::<UInt16Array>(&batch, "start_vertex").value(row),
                    column::<Float64Array>(&batch, "start_fraction").value(row),
                    column::<UInt16Array>(&batch, "end_vertex").value(row),
                    column::<Float64Array>(&batch, "end_fraction").value(row),
                    column::<Int64Array>(&batch, "start_node").value(row),
                    column::<Int64Array>(&batch, "end_node").value(row),
                )
            })
            .collect();
        assert_eq!(stored, pieces, "{family} pieces in {square}");
        let layer = all_rows(&output.join(square).join(format!("{family}.arrow")));
        let mut layer_rows: Vec<_> = (0..layer.num_rows())
            .map(|row| {
                (
                    column::<Int64Array>(&layer, "osm_id").value(row),
                    column::<Int16Array>(&layer, "segment_idx").value(row),
                    column::<Int32Array>(&layer, "start_gx").value(row),
                    column::<Int32Array>(&layer, "end_gy").value(row),
                    column::<Float32Array>(&layer, "length_m")
                        .value(row)
                        .to_bits(),
                )
            })
            .collect();
        layer_rows.sort_unstable();
        let piece_rows: Vec<_> = (0..batch.num_rows())
            .map(|row| {
                (
                    column::<Int64Array>(&batch, "way_id").value(row),
                    column::<Int16Array>(&batch, "segment_idx").value(row),
                    column::<Int32Array>(&batch, "start_gx").value(row),
                    column::<Int32Array>(&batch, "end_gy").value(row),
                    column::<Float32Array>(&batch, "length_m")
                        .value(row)
                        .to_bits(),
                )
            })
            .collect();
        assert_eq!(piece_rows, layer_rows, "{family} layer rows in {square}");
        if family == "railways" && square == "z9/276/173" {
            // Way 10's second piece continues the whole-way metres across the seam.
            let from = column::<Float64Array>(&batch, "from_m").value(0);
            let to = column::<Float64Array>(&batch, "to_m").value(0);
            assert_eq!(from.to_bits(), way_10_metres.value(1).to_bits());
            assert_eq!(to.to_bits(), way_10_metres.value(2).to_bits());
            assert_eq!(
                list_values::<Int32Array>(&batch, "chain_lon_e7", 0).values(),
                &[e7(14.0625), e7(14.064)]
            );
        }
    }
    assert!(!root.path().join("spill").exists());
}

#[test]
fn node_cache_cap_is_checked_before_the_selected_node_filter() {
    let root = TempDir::new("node-cap");
    let input = root.path().join("fixture.osm.pbf");
    let output = root.path().join("prepared");
    let mut nodes = NODES.to_vec();
    nodes.push((25_000_000_000, 50.0, 14.0));
    write_fixture_pbf(&input, &nodes, &[]);
    let run = Command::new(env!("CARGO_BIN_EXE_osm-extract"))
        .arg("--input")
        .arg(&input)
        .arg("--output")
        .arg(&output)
        .arg("--node-cache")
        .arg(root.path().join("nodes.cache"))
        .arg("--spill-dir")
        .arg(root.path().join("spill"))
        .arg("--num-buckets")
        .arg("1")
        .env("QM_OSM_ONLY", "roads,railways")
        .env("RAYON_NUM_THREADS", "2")
        .output()
        .expect("spawn osm-extract");
    assert!(!run.status.success());
    assert!(String::from_utf8_lossy(&run.stderr).contains("MAX_NODE_ID"));
    assert!(!output.exists());
    assert!(!root.path().join("prepared.railway-ways.arrow").exists());
}

fn run_extract(root: &Path, tag: &str, input: &Path) -> PathBuf {
    let output = root.join(tag);
    let run = Command::new(env!("CARGO_BIN_EXE_osm-extract"))
        .arg("--input")
        .arg(input)
        .arg("--output")
        .arg(&output)
        .arg("--node-cache")
        .arg(root.join(format!("nodes-{tag}.cache")))
        .arg("--spill-dir")
        .arg(root.join(format!("spill-{tag}")))
        .arg("--num-buckets")
        .arg("1")
        .env("QM_OSM_ONLY", "roads,railways")
        .env("RAYON_NUM_THREADS", "2")
        .output()
        .expect("spawn osm-extract");
    assert!(
        run.status.success(),
        "osm-extract failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    output
}

fn transport_node_files(output: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let z9 = output.join("z9");
    if !z9.is_dir() {
        return files;
    }
    for x in std::fs::read_dir(&z9).expect("read z9") {
        let x = x.expect("read square x").path();
        for y in std::fs::read_dir(&x).expect("read square y") {
            let candidate = y.expect("read square").path().join("transport_nodes.arrow");
            if candidate.is_file() {
                files.push(candidate);
            }
        }
    }
    files.sort();
    files
}

/// Unlinked control nodes flush from one map in `ControlPoints::finish`: two
/// extracts of the same PBF must write identical `transport_nodes.arrow`
/// content — same rows in the same order, same metadata pairs. Raw file bytes
/// still vary in schema-metadata KEY ORDER (arrow-rs 54 serializes the metadata
/// `HashMap` in iteration order, `arrow-ipc/convert.rs::metadata_to_fb`), which
/// no reader observes (metadata is read by key) and no writer option orders —
/// so this test compares decoded batches, not the file bytes.
#[test]
fn unlinked_transport_controls_extract_deterministically_twice() {
    let root = TempDir::new("determinism");
    let input = root.path().join("fixture.osm.pbf");
    // Twelve whistle boards no way references: orphans flush in map order.
    let mut nodes = NODES.to_vec();
    let mut tags = Vec::new();
    for (offset, id) in (40..52).enumerate() {
        nodes.push((
            id,
            50.001 + offset as f64 * 0.0001,
            14.061 + offset as f64 * 0.0001,
        ));
        tags.push((id, "railway", "level_crossing"));
    }
    write_fixture_pbf(&input, &nodes, &tags);

    let first = run_extract(root.path(), "first", &input);
    let second = run_extract(root.path(), "second", &input);
    let first_files = transport_node_files(&first);
    let second_files = transport_node_files(&second);
    assert_eq!(
        first_files.len(),
        second_files.len(),
        "transport_nodes.arrow file sets differ"
    );
    assert!(
        !first_files.is_empty(),
        "no transport_nodes.arrow written — orphans lost, test vacuous"
    );
    let mut orphan_ids = Vec::new();
    for (first_file, second_file) in first_files.iter().zip(&second_files) {
        assert_eq!(
            first_file.strip_prefix(&first).unwrap(),
            second_file.strip_prefix(&second).unwrap(),
            "transport_nodes.arrow paths differ"
        );
        let first_batch = all_rows(first_file);
        let second_batch = all_rows(second_file);
        assert_eq!(
            first_batch, second_batch,
            "transport_nodes.arrow differs between identical extracts: {}",
            first_file.strip_prefix(&first).unwrap().display()
        );
        orphan_ids.extend(
            column::<Int64Array>(&first_batch, "osm_id")
                .values()
                .iter()
                .copied(),
        );
    }
    // Orphans flush in node-id order (`BTreeMap`), which the square sort and
    // the stable z14 blocking preserve — pin the order, not just the set.
    assert_eq!(
        orphan_ids,
        (40..52).collect::<Vec<_>>(),
        "orphan controls missing or reordered in transport_nodes.arrow"
    );
}
