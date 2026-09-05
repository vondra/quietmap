//! The actual IPC columns, writer pruning and publication support share decoded geometry.

use super::*;
use crate::flight::{AirborneEvent, AirborneSubSegment};
use arrow::array::{Int32Array, ListArray, StructArray};

#[test]
fn support_coordinates_match_actual_airborne_arrow() {
    let coordinates = [
        (52.001, 14.26),
        (50.001, 14.26),
        (80.178_71, 0.0),
        (50.0, 179.99),
        (50.0, -179.99),
        (-89.0, 180.0),
        (89.0, -180.0),
    ];
    let row = AirborneEvent {
        flight_id: 42,
        callsign: "SUPPORT42".into(),
        aircraft_type: *b"B738",
        profile_idx: 0,
        source_id: 2,
        origin: 0,
        sub_segments: coordinates
            .iter()
            .map(|&(lat, lon)| AirborneSubSegment {
                start_lat: lat,
                start_lon: lon,
                end_lat: lat,
                end_lon: lon,
                start_alt_m: 1000.0,
                end_alt_m: 1000.0,
                speed_kt: 450.0,
                length_m: 100.0,
                period: 0,
                date_id: 0,
                flags: 1,
                terrain_start_elev_m: 0.0,
                terrain_end_elev_m: 0.0,
            })
            .collect(),
    };
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("airborne.arrow");
    crate::arrow_io::write_airborne(&path, &[row], 12, 0).unwrap();
    let (_, batches) = crate::arrow_io::read_record_batches(&path).unwrap();
    let segments = batches[0]
        .column_by_name("sub_segments")
        .unwrap()
        .as_any()
        .downcast_ref::<ListArray>()
        .unwrap();
    let columns = segments
        .values()
        .as_any()
        .downcast_ref::<StructArray>()
        .unwrap();
    let gx = columns
        .column_by_name("start_gx")
        .unwrap()
        .as_any()
        .downcast_ref::<Int32Array>()
        .unwrap();
    let gy = columns
        .column_by_name("start_gy")
        .unwrap()
        .as_any()
        .downcast_ref::<Int32Array>()
        .unwrap();
    for (index, &(lat, lon)) in coordinates.iter().enumerate() {
        let (decoded_lon, decoded_lat) =
            square_store::grid_cols::grid_cell_lonlat(gx.value(index), gy.value(index));
        let expected = [decoded_lat as f32, decoded_lon as f32];
        assert_eq!(airborne_decoded_endpoint(lat, lon), Some(expected));
        let support = airborne_support_cells(expected, expected).unwrap();
        assert!(support.contains(grid::square_of(
            f64::from(expected[0]),
            f64::from(expected[1])
        )));
    }
    assert!(airborne_decoded_endpoint(f32::NAN, 0.0).is_none());
    assert!(airborne_decoded_endpoint(91.0, 0.0).is_none());
}
