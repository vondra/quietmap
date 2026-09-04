//! Prepared airborne and cruise schemas consumed directly by popup readers.

use super::*;

pub fn airborne_schema() -> Arc<Schema> {
    let sub_struct = DataType::Struct(Fields::from(vec![
        Field::new("start_gx", DataType::Int32, false),
        Field::new("start_gy", DataType::Int32, false),
        Field::new("start_alt_m", DataType::Int16, false),
        Field::new("end_gx", DataType::Int32, false),
        Field::new("end_gy", DataType::Int32, false),
        Field::new("end_alt_m", DataType::Int16, false),
        Field::new("speed_kt", DataType::Float32, false),
        Field::new("length_m", DataType::Float32, false),
        Field::new("period", DataType::UInt8, false),
        Field::new("date_id", DataType::Int16, false),
        Field::new("flags", DataType::UInt8, false),
        Field::new("terrain_start_elev_m", DataType::Int16, false),
        Field::new("terrain_end_elev_m", DataType::Int16, false),
    ]));
    let fields = vec![
        Field::new("flight_id", DataType::UInt64, false),
        Field::new("callsign", DataType::Utf8, false),
        Field::new("aircraft_type", DataType::FixedSizeBinary(4), false),
        Field::new("profile_idx", DataType::UInt8, false),
        Field::new("source_id", DataType::UInt8, false),
        Field::new("origin", DataType::UInt8, false),
        Field::new(
            "sub_segments",
            DataType::List(Arc::new(Field::new("item", sub_struct, false))),
            false,
        ),
    ];
    Arc::new(Schema::new(fields).with_metadata(base_metadata(&[
        ("kind", "airborne"),
        ("airborne_contract", AIRBORNE_CONTRACT),
    ])))
}

pub fn cruise_top_candidate_fields() -> Fields {
    Fields::from(vec![
        Field::new("flight_id", DataType::UInt64, false),
        Field::new("callsign", DataType::Utf8, false),
        Field::new("aircraft_type", DataType::FixedSizeBinary(4), false),
        Field::new("peak_lmax_25m_db", DataType::Float32, false),
        Field::new("altitude_m", DataType::Float32, false),
    ])
}

pub fn cruise_schema() -> Arc<Schema> {
    let cand_struct = DataType::Struct(cruise_top_candidate_fields());
    let fields = vec![
        Field::new("lon", DataType::Float64, false),
        Field::new("lat", DataType::Float64, false),
        Field::new("class", DataType::UInt8, false),
        Field::new("rep_profile_idx", DataType::UInt8, false),
        Field::new("fl_bin", DataType::UInt8, false),
        Field::new("period", DataType::UInt8, false),
        Field::new("sum_length_m", DataType::Float32, false),
        Field::new("rep_len_m", DataType::Float32, false),
        Field::new("rep_alt_m", DataType::Float32, false),
        Field::new("rep_speed_kt", DataType::Float32, false),
        Field::new("unique_count", DataType::UInt32, false),
        Field::new(
            "top_candidates",
            DataType::List(Arc::new(Field::new("item", cand_struct, false))),
            false,
        ),
        Field::new("source_id", DataType::UInt8, false),
        Field::new("origin", DataType::UInt8, false),
    ];
    Arc::new(Schema::new(fields).with_metadata(base_metadata(&[
        ("kind", "cruise"),
        ("cruise_contract", CRUISE_CONTRACT),
    ])))
}
