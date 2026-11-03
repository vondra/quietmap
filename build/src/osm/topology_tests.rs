//! Real PBF topology must retain original chains, route order and exact split positions.

use super::{pbf_fixture::*, topology_test_helpers::*};
use arrow_array::*;
use std::fs::File;

#[test]
fn hand_built_pbf_yields_exact_source_topology_and_matching_arrow_pieces() {
    let root = TempDir::new("run");
    let input = root.path().join("fixture.osm.pbf");
    let output = root.path().join("prepared");
    write_fixture_pbf(&input, NODES, &[]);

    run_extract(root.path(), "prepared", &input).unwrap();

    let buildings = all_rows(&output.join("z9/276/173/buildings.arrow"));
    assert_eq!(
        buildings.num_rows(),
        1,
        "untagged relation-member ways lost their node coordinates"
    );
    assert!(
        column::<arrow_array::BinaryArray>(&buildings, "geom")
            .value(0)
            .len()
            > 24
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
    let ways = arrow_ipc::reader::StreamReader::try_new(
        File::open(root.path().join("prepared.railway-ways.arrow")).expect("open railway ways"),
        None,
    )
    .expect("read IPC stream");
    let ways = arrow_select::concat::concat_batches(
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
