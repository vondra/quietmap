//! Ground energy, unique movement unions and discovered airport schemas.

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
        // Raw Σ over n_days per band. Consumer divides via
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
        // join over per-row scalars. v9: these three count NON-GA-class
        // fids only — the GA-class union lives in the `microseg_unique_ga_*`
        // columns below so the popup can divide each by its own window:
        // `non_ga / n_days + ga / ga_n_days`.
        Field::new("microseg_unique_count", DataType::UInt32, false),
        Field::new("microseg_unique_arr_count", DataType::UInt32, false),
        Field::new("microseg_unique_dep_count", DataType::UInt32, false),
        Field::new("microseg_unique_gse_count_per_class", gse_per_class, false),
        // v9 GA-class microseg UNION (PROP_C172 + HELICOPTER): the full-year
        // window split of the three columns above. Zero on non-hybrid
        // extracts (no flights routed to the GA window) — then the popup's
        // `ga / ga_n_days` term vanishes and the math degenerates to legacy.
        Field::new("microseg_unique_ga_count", DataType::UInt32, false),
        Field::new("microseg_unique_ga_arr_count", DataType::UInt32, false),
        Field::new("microseg_unique_ga_dep_count", DataType::UInt32, false),
    ];
    Arc::new(Schema::new(fields).with_metadata(base_metadata(&[
        ("kind", "airport_traffic"),
        ("airport_traffic_contract", AIRPORT_TRAFFIC_CONTRACT),
    ])))
}

pub fn airport_summary_schema() -> Arc<Schema> {
    let gse_per_class = DataType::FixedSizeList(
        Arc::new(Field::new("item", DataType::UInt32, false)),
        NUM_GSE_CLASSES,
    );
    let ops_per_kind = || {
        DataType::FixedSizeList(
            Arc::new(Field::new("item", DataType::UInt32, false)),
            NUM_OPS_KINDS,
        )
    };
    let fields = vec![
        Field::new("airport_key", DataType::Utf8, false),
        // Non-GA-class window.
        Field::new("airport_unique_arr_count", DataType::UInt32, false),
        Field::new("airport_unique_dep_count", DataType::UInt32, false),
        Field::new("airport_unique_gse_count_per_class", gse_per_class, false),
        Field::new("airport_unique_ops_count_per_kind", ops_per_kind(), false),
        // GA-class full-year window.
        Field::new("airport_unique_ga_arr_count", DataType::UInt32, false),
        Field::new("airport_unique_ga_dep_count", DataType::UInt32, false),
        Field::new(
            "airport_unique_ga_ops_count_per_kind",
            ops_per_kind(),
            false,
        ),
    ];
    Arc::new(Schema::new(fields).with_metadata(base_metadata(&[
        ("kind", "airport_summary"),
        ("airport_summary_contract", AIRPORT_SUMMARY_CONTRACT),
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

pub fn synth_airport_areas_schema() -> Arc<Schema> {
    let fields = vec![
        Field::new("osm_id", DataType::UInt64, false),
        Field::new("airport_key", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("aeroway_type", DataType::UInt8, false),
        Field::new("centroid_gx", DataType::Int32, false),
        Field::new("centroid_gy", DataType::Int32, false),
        Field::new("area_m2", DataType::Float32, false),
    ];
    Arc::new(Schema::new(fields).with_metadata(base_metadata(&[
        ("kind", "synth_airport_areas"),
        (
            "synth_airport_areas_contract",
            square_store::aircraft_contract::SYNTH_AIRPORT_AREAS_CONTRACT,
        ),
    ])))
}
