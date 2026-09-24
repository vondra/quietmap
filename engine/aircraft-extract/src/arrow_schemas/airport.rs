//! Ground energy and discovered airstrip schemas.

use super::*;

pub fn airport_traffic_schema() -> Arc<Schema> {
    let gse_per_class = DataType::FixedSizeList(
        Arc::new(Field::new("item", DataType::UInt32, false)),
        NUM_GSE_CLASSES,
    );
    let fields = vec![
        Field::new("airport_key", DataType::Utf8, false),
        Field::new("osm_id", DataType::UInt64, false),
        Field::new("segment_idx", DataType::UInt16, false),
        Field::new("geometry_kind", DataType::UInt8, false),
        Field::new("start_gx", DataType::Int32, false),
        Field::new("start_gy", DataType::Int32, false),
        Field::new("end_gx", DataType::Int32, false),
        Field::new("end_gy", DataType::Int32, false),
        Field::new("length_m", DataType::Float32, false),
        Field::new("ops_kind", DataType::UInt8, false),
        Field::new("is_departure", DataType::UInt8, false),
        Field::new("veh_kind", DataType::UInt8, false),
        Field::new("class_idx", DataType::UInt8, false),
        Field::new("period", DataType::UInt8, false),
        // 1 when every movement of the row touches a secondary-provider
        // sample: the consumer applies the increment-day weight.
        Field::new("secondary_only", DataType::UInt8, false),
        // Raw Σ over the sampling days per band. Consumer divides via
        // `period_leq(e, n_days_f, period_seconds)` to recover Leq.
        // FixedSizeList enforces the 8-band invariant at the schema
        // level so the reader doesn't need a runtime `ensure!` guard.
        Field::new(
            "band_energy_lin",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, false)),
                noise_compute::types::NUM_BANDS as i32,
            ),
            false,
        ),
        // Per-row scalar unique counts replace the v4 `flight_ids:
        // List<UInt64>` payload. Each row carries ALL four counters;
        // only the per-row-key-relevant ones are non-zero (e.g.
        // arr_count populated only when (ops_kind=RUNWAY_ROLL,
        // is_departure=0, veh_kind=0)).
        Field::new("unique_movement_count", DataType::UInt32, false),
        Field::new("unique_arr_count", DataType::UInt32, false),
        Field::new("unique_dep_count", DataType::UInt32, false),
        Field::new("unique_gse_count_per_class", gse_per_class.clone(), false),
        // Per-microsegment UNION (replicated across rows). Lets the
        // popup populate per-microseg movement counts without a UNION
        // join over per-row scalars: movements with a primary-provider row
        // in the category, then those seen only through secondary rows, so
        // the popup reads `primary / baseline + secondary / increment`.
        Field::new("microseg_unique_count", DataType::UInt32, false),
        Field::new("microseg_unique_arr_count", DataType::UInt32, false),
        Field::new("microseg_unique_dep_count", DataType::UInt32, false),
        Field::new(
            "microseg_unique_gse_count_per_class",
            gse_per_class.clone(),
            false,
        ),
        Field::new("microseg_unique_secondary_count", DataType::UInt32, false),
        Field::new(
            "microseg_unique_secondary_arr_count",
            DataType::UInt32,
            false,
        ),
        Field::new(
            "microseg_unique_secondary_dep_count",
            DataType::UInt32,
            false,
        ),
        Field::new(
            "microseg_unique_secondary_gse_count_per_class",
            gse_per_class,
            false,
        ),
    ];
    Arc::new(Schema::new(fields).with_metadata(base_metadata(&[
        ("kind", "airport_traffic"),
        ("airport_traffic_contract", AIRPORT_TRAFFIC_CONTRACT),
    ])))
}

pub fn synth_airport_lines_schema() -> Arc<Schema> {
    let fields = vec![
        Field::new("osm_id", DataType::UInt64, false),
        Field::new("segment_idx", DataType::UInt16, false),
        Field::new("airport_key", DataType::Utf8, false),
        Field::new("start_gx", DataType::Int32, false),
        Field::new("start_gy", DataType::Int32, false),
        Field::new("end_gx", DataType::Int32, false),
        Field::new("end_gy", DataType::Int32, false),
        Field::new("length_m", DataType::Float32, false),
        Field::new("heading_deg", DataType::Float32, false),
        Field::new("aeroway_type", DataType::UInt8, false),
        Field::new("name", DataType::Utf8, false),
    ];
    Arc::new(Schema::new(fields).with_metadata(base_metadata(&[
        ("kind", "synth_airport_lines"),
        (
            "synth_airport_lines_contract",
            square_store::aircraft_contract::SYNTH_AIRPORT_LINES_CONTRACT,
        ),
    ])))
}
