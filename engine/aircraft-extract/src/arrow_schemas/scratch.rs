//! Typed in-process scratch schemas for flights and classified segments.

use super::*;

pub fn flights_schema() -> Arc<Schema> {
    let pt_struct = DataType::Struct(Fields::from(vec![
        Field::new("ts_offset_s", DataType::Float32, false),
        Field::new("lat", DataType::Float32, false),
        Field::new("lon", DataType::Float32, false),
        Field::new("alt_ft", DataType::Float32, false),
        Field::new("speed_kt", DataType::Float32, false),
        Field::new("track_deg", DataType::Float32, false),
        Field::new("baro_rate_fpm", DataType::Float32, false),
        Field::new("flags", DataType::UInt8, false),
    ]));
    let fields = vec![
        Field::new("flight_id", DataType::UInt64, false),
        Field::new("callsign", DataType::Utf8, false),
        Field::new("aircraft_type", DataType::FixedSizeBinary(4), false),
        Field::new("profile_idx", DataType::UInt8, false),
        Field::new("source_id", DataType::UInt8, false),
        Field::new("origin", DataType::UInt8, false),
        Field::new("veh_kind", DataType::UInt8, false),
        Field::new("gse_class", DataType::UInt8, false),
        Field::new("base_timestamp", DataType::Float64, false),
        Field::new(
            "points",
            DataType::List(Arc::new(Field::new("item", pt_struct, false))),
            false,
        ),
    ];
    Arc::new(Schema::new(fields).with_metadata(base_metadata(&[("kind", "flights")])))
}

pub fn segments_schema() -> Arc<Schema> {
    let fields = vec![
        Field::new("flight_id", DataType::UInt64, false),
        Field::new("callsign", DataType::Utf8, false),
        Field::new("aircraft_type", DataType::FixedSizeBinary(4), false),
        Field::new("profile_idx", DataType::UInt8, false),
        Field::new("source_id", DataType::UInt8, false),
        Field::new("origin", DataType::UInt8, false),
        Field::new("veh_kind", DataType::UInt8, false),
        Field::new("gse_class", DataType::UInt8, false),
        Field::new("period", DataType::UInt8, false),
        Field::new("date_id", DataType::Int16, false),
        Field::new("phase", DataType::UInt8, false),
        Field::new("flags", DataType::UInt8, false),
        Field::new("start_lat", DataType::Float32, false),
        Field::new("start_lon", DataType::Float32, false),
        Field::new("start_alt_m", DataType::Float32, false),
        Field::new("end_lat", DataType::Float32, false),
        Field::new("end_lon", DataType::Float32, false),
        Field::new("end_alt_m", DataType::Float32, false),
        Field::new("speed_kt", DataType::Float32, false),
        Field::new("length_m", DataType::Float32, false),
        Field::new("agl_avg_m", DataType::Float32, false),
        Field::new("start_elev_m", DataType::Float32, false),
        Field::new("end_elev_m", DataType::Float32, false),
    ];
    Arc::new(Schema::new(fields).with_metadata(base_metadata(&[("kind", "segments")])))
}
