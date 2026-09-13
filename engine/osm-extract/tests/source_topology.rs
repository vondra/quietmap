//! Exercise real PBF extraction across a tile seam, source junctions, zero-length ways and missing nodes.

use arrow::array::{Int16Array, Int64Array};
use arrow::ipc::reader::FileReader;
use rusqlite::Connection;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

/// String-table entries; index 0 is reserved, so indices are positions here.
const STRINGS: [&str; 13] = [
    "", "railway", "rail", "highway", "trunk", "route", "train", "bus", "stop", "forward",
    "backward", "platform", "unusual",
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
fn write_fixture_pbf(path: &Path) {
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
        &NODES.iter().map(|&(id, _, _)| id).collect::<Vec<_>>(),
    );
    push_packed_sint64_deltas(
        &mut dense,
        8,
        &NODES
            .iter()
            .map(|&(_, lat, _)| (lat * 1e7).round() as i64)
            .collect::<Vec<_>>(),
    );
    push_packed_sint64_deltas(
        &mut dense,
        9,
        &NODES
            .iter()
            .map(|&(_, _, lon)| (lon * 1e7).round() as i64)
            .collect::<Vec<_>>(),
    );
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

/// (way_id, segment_idx) pairs of one square's Arrow file, in file order.
fn arrow_piece_keys(path: &Path) -> Vec<(i64, i16)> {
    let reader =
        FileReader::try_new(File::open(path).expect("open arrow file"), None).expect("read IPC");
    let mut keys = Vec::new();
    for batch in reader {
        let batch = batch.expect("decode IPC batch");
        let osm_ids = batch
            .column_by_name("osm_id")
            .expect("osm_id column")
            .as_any()
            .downcast_ref::<Int64Array>()
            .expect("osm_id is Int64");
        let segment_indices = batch
            .column_by_name("segment_idx")
            .expect("segment_idx column")
            .as_any()
            .downcast_ref::<Int16Array>()
            .expect("segment_idx is Int16");
        for row in 0..osm_ids.len() {
            keys.push((osm_ids.value(row), segment_indices.value(row)));
        }
    }
    keys
}

fn coordinate(id: i64) -> Option<[f64; 2]> {
    NODES
        .iter()
        .find(|&&(node_id, _, _)| node_id == id)
        .map(|&(_, lat, lon)| [lat, lon])
}

fn expected_chain(nodes: &[i64]) -> Vec<(String, Option<[f64; 2]>)> {
    nodes
        .iter()
        .map(|&id| (id.to_string(), coordinate(id)))
        .collect()
}

#[test]
fn hand_built_pbf_yields_exact_source_topology_and_matching_arrow_pieces() {
    let root = TempDir::new("run");
    let input = root.path().join("fixture.osm.pbf");
    let output = root.path().join("prepared");
    write_fixture_pbf(&input);

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

    let published = root.path().join("prepared.transport.sqlite");
    let connection = Connection::open(&published).expect("open published transport database");

    let mut routes = connection
        .prepare("SELECT osm_id, members_json FROM source_train_routes")
        .unwrap();
    let route_rows: Vec<(i64, String)> = routes
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        route_rows.len(),
        1,
        "non-train relation leaked into train routes"
    );
    assert_eq!(route_rows[0].0, 1000);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&route_rows[0].1).unwrap(),
        serde_json::json!([
            ["n", "1", "stop"],
            ["w", "10", "forward"],
            ["w", "10", "backward"],
            ["w", "9007199254740993", ""],
            ["r", "2000", "unusual"],
            ["w", "11", "platform"]
        ])
    );

    // Original chains: every fixture way is retained, way 15 keeps the null
    // entry for the absent node 999, way 13 keeps its zero-length pair.
    let mut chains = connection
        .prepare("SELECT osm_id, family, nodes_json FROM source_ways ORDER BY osm_id")
        .expect("prepare source_ways query");
    let rows: Vec<(i64, String, String)> = chains
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .expect("query source_ways")
        .collect::<Result<_, _>>()
        .expect("collect source_ways");
    drop(chains);
    assert_eq!(rows.len(), WAYS.len());
    for ((way_id, family, nodes_json), (want_id, tag, _, refs)) in rows.iter().zip(WAYS) {
        assert_eq!(way_id, want_id);
        assert_eq!(
            family,
            if *tag == "highway" {
                "roads"
            } else {
                "railways"
            }
        );
        let chain: Vec<(String, Option<[f64; 2]>)> =
            serde_json::from_str(nodes_json).expect("parse nodes_json");
        assert_eq!(chain, expected_chain(refs), "chain of way {way_id}");
    }

    // Aliases: only the explicit zero-length hop 3→7 (way 13) unions — the
    // coordinate twins 4 (way 11) never touch 2, and roads stay sparse.
    let mut aliases = connection
        .prepare("SELECT family, node_id, canonical_node FROM node_aliases")
        .expect("prepare node_aliases query");
    let alias_rows: Vec<(String, i64, i64)> = aliases
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .expect("query node_aliases")
        .collect::<Result<_, _>>()
        .expect("collect node_aliases");
    drop(aliases);
    assert_eq!(alias_rows, vec![("railways".to_string(), 7, 3)]);

    // Pieces: junction split at node 2 (vertex 1) across the z9 seam, no
    // bridging piece for way 15, fractional halves for road 16's long hop.
    let mut pieces = connection
        .prepare(
            "SELECT way_id, segment_idx, square, start_vertex, start_fraction,
                    end_vertex, end_fraction
             FROM source_pieces ORDER BY way_id, segment_idx",
        )
        .expect("prepare source_pieces query");
    let piece_rows: Vec<(i64, i16, String, i64, f64, i64, f64)> = pieces
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
            ))
        })
        .expect("query source_pieces")
        .collect::<Result<_, _>>()
        .expect("collect source_pieces");
    drop(pieces);
    let expected_pieces: Vec<(i64, i16, &str, i64, f64, i64, f64)> = vec![
        (10, 0, "z9/275/173", 0, 0.0, 1, 0.0),
        (10, 1, "z9/276/173", 1, 0.0, 2, 0.0),
        (11, 0, "z9/276/173", 0, 0.0, 1, 0.0),
        (12, 0, "z9/276/173", 0, 0.0, 1, 0.0),
        (14, 0, "z9/276/173", 0, 0.0, 1, 0.0),
        (16, 0, "z9/275/173", 0, 0.0, 0, 0.5),
        (16, 1, "z9/276/173", 0, 0.5, 1, 0.0),
    ];
    assert_eq!(
        piece_rows,
        expected_pieces
            .into_iter()
            .map(|(w, s, square, sv, sf, ev, ef)| (w, s, square.to_string(), sv, sf, ev, ef))
            .collect::<Vec<_>>()
    );

    // Arrow association: per (family, square), the emitted rows equal the SQL
    // piece keys — the trunk's two pieces land in both seam squares.
    for family in ["railways", "roads"] {
        for square in ["z9/275/173", "z9/276/173"] {
            let mut sql_keys = connection
                .prepare(
                    "SELECT source_pieces.way_id, source_pieces.segment_idx
                     FROM source_pieces
                     JOIN source_ways ON source_ways.osm_id = source_pieces.way_id
                     WHERE source_ways.family = ?1 AND source_pieces.square = ?2
                     ORDER BY 1, 2",
                )
                .expect("prepare piece-key query");
            let keys: Vec<(i64, i16)> = sql_keys
                .query_map([family, square], |row| Ok((row.get(0)?, row.get(1)?)))
                .expect("query piece keys")
                .collect::<Result<_, _>>()
                .expect("collect piece keys");
            drop(sql_keys);
            let arrow_path = output.join(square).join(format!("{family}.arrow"));
            if keys.is_empty() {
                assert!(
                    !arrow_path.exists(),
                    "{family} has no pieces in {square} yet {} exists",
                    arrow_path.display()
                );
                continue;
            }
            let mut arrow_keys = arrow_piece_keys(&arrow_path);
            arrow_keys.sort_unstable();
            assert_eq!(arrow_keys, keys, "{family} rows in {square}");
        }
    }
}
