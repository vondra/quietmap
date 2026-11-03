//! Encode a small PBF with cross-square topology, missing nodes and ordered route members.

use std::path::Path;

/// String-table entries; index 0 is reserved, so indices are positions here.
const STRINGS: [&str; 19] = [
    "",
    "railway",
    "rail",
    "highway",
    "trunk",
    "route",
    "train",
    "bus",
    "stop",
    "forward",
    "backward",
    "platform",
    "unusual",
    "level_crossing",
    "type",
    "multipolygon",
    "building",
    "yes",
    "outer",
];

/// All fixture nodes, ascending by id. Node 999 (referenced by way 15) is
/// deliberately absent: the chain must record it as null.
pub const NODES: &[(i64, f64, f64)] = &[
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
    (100, 50.001, 14.066),
    (101, 50.001, 14.067),
    (102, 50.002, 14.067),
    (103, 50.002, 14.066),
];

/// (way id, tag key, tag value, node refs). Ways 13 and 15 exercise the two
/// piece-less rows; way 16 is the trunk road with one over-cap hop.
pub const WAYS: &[(i64, &str, &str, &[i64])] = &[
    (10, "railway", "rail", &[1, 2, 3]),
    (11, "railway", "rail", &[4, 5]),
    (12, "railway", "rail", &[2, 6]),
    (13, "railway", "rail", &[3, 7]),
    (14, "railway", "rail", &[7, 8]),
    (15, "railway", "rail", &[20, 999, 21]),
    (16, "highway", "trunk", &[30, 31]),
    (20, "", "", &[100, 101, 102]),
    (21, "", "", &[102, 103, 100]),
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
pub fn write_fixture_pbf(path: &Path, nodes: &[(i64, f64, f64)], node_tags: &[(i64, &str, &str)]) {
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
                keys_vals.push(STRINGS.iter().position(|entry| entry == &value).unwrap() as u32);
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
        if !key.is_empty() {
            push_packed_uint32(&mut way, 2, &[key_index]);
            push_packed_uint32(&mut way, 3, &[value_index]);
        }
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
    let mut building = Vec::new();
    push_varint_field(&mut building, 1, 2000);
    push_packed_uint32(&mut building, 2, &[14, 16]);
    push_packed_uint32(&mut building, 3, &[15, 17]);
    push_packed_uint32(&mut building, 8, &[18, 18]);
    push_packed_sint64_deltas(&mut building, 9, &[20, 21]);
    push_packed_uint32(&mut building, 10, &[1, 1]);
    push_bytes_field(&mut relations_group, 4, &building);
    push_bytes_field(&mut block, 2, &relations_group);
    write_pbf_blob(&mut pbf, "OSMData", &block);

    std::fs::write(path, pbf).expect("write fixture PBF");
}
