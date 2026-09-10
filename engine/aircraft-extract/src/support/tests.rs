//! The stored z30 columns decode to the endpoints the writer's envelopes bound.

use super::*;
use crate::flight::FlightSegment;
use arrow::array::Int32Array;

#[test]
fn stored_coordinates_decode_to_the_bounded_endpoints() {
    let coordinates = [
        (52.001, 14.26),
        (50.001, 14.26),
        (80.178_71, 0.0),
        (50.0, 179.99),
        (50.0, -179.99),
        (-89.0, 180.0),
        (89.0, -180.0),
    ];
    let rows: Vec<FlightSegment> = coordinates
        .iter()
        .map(|&(lat, lon)| {
            let mut row = FlightSegment::airborne_fixture(42, lat, lon);
            row.end_lat = lat;
            row.end_lon = lon;
            row
        })
        .collect();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("airborne.arrow");
    crate::arrow_io::write_airborne(&path, &rows, 12, 0).unwrap();
    let (_, batches) = crate::arrow_io::read_record_batches(&path).unwrap();
    let mut decoded = Vec::new();
    for batch in &batches {
        let gx = batch
            .column_by_name("start_gx")
            .unwrap()
            .as_any()
            .downcast_ref::<Int32Array>()
            .unwrap();
        let gy = batch
            .column_by_name("start_gy")
            .unwrap()
            .as_any()
            .downcast_ref::<Int32Array>()
            .unwrap();
        for index in 0..batch.num_rows() {
            let (lon, lat) =
                square_store::grid_cols::grid_cell_lonlat(gx.value(index), gy.value(index));
            decoded.push([lat as f32, lon as f32]);
        }
    }
    let mut expected: Vec<[f32; 2]> = coordinates
        .iter()
        .map(|&(lat, lon)| airborne_decoded_endpoint(lat, lon).unwrap())
        .collect();
    decoded.sort_by(|a, b| a.partial_cmp(b).unwrap());
    expected.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert_eq!(decoded, expected);
    assert!(airborne_decoded_endpoint(f32::NAN, 0.0).is_none());
    assert!(airborne_decoded_endpoint(91.0, 0.0).is_none());
}
