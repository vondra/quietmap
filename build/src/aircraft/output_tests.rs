//! The day files read back: dev4's v16 columns in order, the altitudes above EGM2008, the flight
//! table.

use super::*;
use crate::aircraft::archive::tests::scratch_directory;
use crate::aircraft::flight_table::FlightRow;
use crate::aircraft::flights::{Airframe, Flight};
use crate::aircraft::phases::Phase;
use arrow_array::{Array, Float32Array, StringArray, UInt8Array, UInt64Array};

fn segment(start_lat: f32) -> Segment {
    Segment {
        period: 1,
        phase: Phase::Airborne,
        flags: 1,
        start_lat,
        start_lon: 14.0,
        end_lat: start_lat + 0.01,
        end_lon: 14.01,
        start_barometric_m: 900.0,
        end_barometric_m: 950.0,
        start_altitude_m: 940.0,
        end_altitude_m: 990.0,
        speed_kt: 180.0,
        length_m: 1400.0,
        mean_height_m: 600.0,
        start_terrain_m: 350.0,
        end_terrain_m: 355.0,
    }
}

fn read(path: &Path) -> RecordBatch {
    let file = File::open(path).unwrap();
    let mut reader = arrow_ipc::reader::FileReader::try_new(file, None).unwrap();
    reader.next().unwrap().unwrap()
}

#[test]
fn both_day_files_read_back_by_column_name() {
    let directory = scratch_directory("output");
    let (segments, flights) = (directory.join("s.arrow"), directory.join("f.arrow"));
    let metadata = HashMap::from([("day".to_string(), "2025-09-02".to_string())]);
    let mut writer = DayWriter::create(&segments, &flights, metadata).unwrap();
    let flight = Flight {
        flight_id: (0x4b1805 << 40) | 1_756_800_000,
        address: "4b1805".into(),
        callsign: "SWR123".into(),
        aircraft_type: "A320".into(),
        profile: 10,
        airframe: Airframe::Jet,
        source_id: 0,
        vehicle_kind: 0,
        ground_vehicle_class: 0,
        emitter_category: 0xa3,
        points: Vec::new(),
    };
    let row = FlightRow::of(&flight, 0, 355.5, 2);
    writer
        .push(&row, &[segment(50.0), segment(50.01)], 2071)
        .unwrap();
    writer.finish().unwrap();
    assert!(!segments.with_extension("arrow.partial").exists());
    let batch = read(&segments);
    let schema = batch.schema();
    let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
    assert_eq!(
        names[..24],
        [
            "flight_id",
            "callsign",
            "aircraft_type",
            "profile_idx",
            "source_id",
            "origin",
            "veh_kind",
            "gse_class",
            "period",
            "date_id",
            "phase",
            "flags",
            "start_lat",
            "start_lon",
            "start_alt_m",
            "end_lat",
            "end_lon",
            "end_alt_m",
            "speed_kt",
            "length_m",
            "agl_avg_m",
            "start_elev_m",
            "end_elev_m",
            "departure_field_elev_m",
        ]
    );
    let column = |name: &str| batch.column_by_name(name).unwrap().clone();
    let floats = |name: &str| {
        column(name)
            .as_any()
            .downcast_ref::<Float32Array>()
            .unwrap()
            .values()
            .to_vec()
    };
    assert_eq!(batch.num_rows(), 2);
    assert_eq!(floats("start_alt_m"), [900.0, 900.0]);
    assert_eq!(floats("end_alt_msl_m"), [990.0, 990.0]);
    assert_eq!(floats("departure_field_elev_m"), [355.5, 355.5]);
    let flags = column("flags");
    assert_eq!(
        flags
            .as_any()
            .downcast_ref::<UInt8Array>()
            .unwrap()
            .value(1),
        1
    );
    let table = read(&flights);
    let address = table.column_by_name("address").unwrap();
    assert_eq!(
        address
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap()
            .value(0),
        "4b1805"
    );
    let ids = table.column_by_name("flight_id").unwrap();
    assert_eq!(
        ids.as_any().downcast_ref::<UInt64Array>().unwrap().value(0),
        flight.flight_id
    );
    let category = table.column_by_name("emitter_category").unwrap();
    assert_eq!(
        category
            .as_any()
            .downcast_ref::<UInt8Array>()
            .unwrap()
            .value(0),
        0xa3
    );
    assert_eq!(table.column_by_name("segments").unwrap().len(), 1);
}
