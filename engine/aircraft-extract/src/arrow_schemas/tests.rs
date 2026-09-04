//! Schema stamps and complete field shape regressions.

use super::*;

#[test]
fn all_schemas_carry_current_version_metadata() {
    for s in [
        flights_schema(),
        segments_schema(),
        airborne_schema(),
        cruise_schema(),
        airport_traffic_schema(),
        airport_summary_schema(),
        synth_airport_lines_schema(),
        synth_airport_areas_schema(),
    ] {
        let md = s.metadata();
        assert_eq!(
            md.get("schema_version").map(String::as_str),
            Some(SCHEMA_VERSION)
        );
        assert!(md.contains_key("kind"));
    }
}

#[test]
fn synth_airport_schemas_carry_required_columns() {
    let lines = synth_airport_lines_schema();
    for required in [
        "osm_id",
        "segment_idx",
        "airport_key",
        "start_gx",
        "start_gy",
        "end_gx",
        "end_gy",
        "length_m",
        "heading_deg",
        "aeroway_type",
        "name",
    ] {
        assert!(
            lines.field_with_name(required).is_ok(),
            "synth_airport_lines schema must carry {required}"
        );
    }
    let areas = synth_airport_areas_schema();
    for required in [
        "osm_id",
        "airport_key",
        "name",
        "aeroway_type",
        "centroid_gx",
        "centroid_gy",
        "area_m2",
    ] {
        assert!(
            areas.field_with_name(required).is_ok(),
            "synth_airport_areas schema must carry {required}"
        );
    }
}

#[test]
fn synth_airport_lines_osm_id_is_unsigned() {
    let lines = synth_airport_lines_schema();
    let field = lines.field_with_name("osm_id").unwrap();
    assert_eq!(
        field.data_type(),
        &DataType::UInt64,
        "synthetic osm_id must be UInt64 so the 1<<63 high-bit pattern round-trips"
    );
}

#[test]
fn airport_traffic_schema_carries_contract_metadata() {
    let s = airport_traffic_schema();
    assert_eq!(
        s.metadata()
            .get("airport_traffic_contract")
            .map(String::as_str),
        Some(AIRPORT_TRAFFIC_CONTRACT)
    );
}

#[test]
fn airport_traffic_schema_has_required_columns() {
    let s = airport_traffic_schema();
    for required in [
        "airport_key",
        "osm_id",
        "segment_idx",
        "geometry_kind",
        "start_gx",
        "start_gy",
        "end_gx",
        "end_gy",
        "length_m",
        "ops_kind",
        "is_departure",
        "veh_kind",
        "class_idx",
        "period",
        "band_energy_lin",
        "unique_movement_count",
        "unique_arr_count",
        "unique_dep_count",
        "unique_gse_count_per_class",
        "microseg_unique_count",
        "microseg_unique_arr_count",
        "microseg_unique_dep_count",
        "microseg_unique_gse_count_per_class",
        "microseg_unique_ga_count",
        "microseg_unique_ga_arr_count",
        "microseg_unique_ga_dep_count",
    ] {
        assert!(
            s.field_with_name(required).is_ok(),
            "airport_traffic schema must carry {required} column"
        );
    }
}

#[test]
fn cruise_schema_v16_required_columns() {
    let s = cruise_schema();
    for required in [
        "lon",
        "lat",
        "class",
        "rep_profile_idx",
        "fl_bin",
        "period",
        "sum_length_m",
        "rep_len_m",
        "rep_alt_m",
        "rep_speed_kt",
        "unique_count",
        "top_candidates",
        "source_id",
        "origin",
    ] {
        assert!(
            s.field_with_name(required).is_ok(),
            "cruise schema must carry {required} column"
        );
    }
    // v16 drops the tautological `flags` column (Doc 29 §A.3.2:
    // all cruise rows always carry IS_DEPARTURE=1).
    assert!(
        s.field_with_name("flags").is_err(),
        "cruise schema v16 must NOT carry `flags` column"
    );
    // v14 explicitly DROPS the per-fid lists.
    for dropped in [
        "cruise_flight_ids",
        "cruise_aircraft_types",
        "cruise_callsigns",
    ] {
        assert!(
            s.field_with_name(dropped).is_err(),
            "cruise v14 schema must NOT carry the v13 {dropped} column"
        );
    }
}

#[test]
fn airport_summary_schema_carries_contract_metadata() {
    let s = airport_summary_schema();
    assert_eq!(
        s.metadata()
            .get("airport_summary_contract")
            .map(String::as_str),
        Some(AIRPORT_SUMMARY_CONTRACT)
    );
}

#[test]
fn assert_airport_summary_contract_round_trip() {
    let s = airport_summary_schema();
    assert!(assert_airport_summary_contract(s.metadata()).is_ok());
    let mut bogus = s.metadata().clone();
    bogus.insert(
        "airport_summary_contract".into(),
        "airport_summary_vBOGUS".into(),
    );
    assert!(assert_airport_summary_contract(&bogus).is_err());
    bogus.remove("airport_summary_contract");
    assert!(assert_airport_summary_contract(&bogus).is_err());
}

#[test]
fn assert_schema_version_rejects_old_versions() {
    for old in [
        "v4", "v5", "v6", "v7", "v8", "v9", "v10", "v11", "v12", "v13",
    ] {
        let md: HashMap<String, String> = [("schema_version".into(), old.into())]
            .into_iter()
            .collect();
        assert!(
            assert_schema_version(&md).is_err(),
            "expected reject for {old}"
        );
    }
}

#[test]
fn assert_schema_version_rejects_missing_metadata() {
    let md: HashMap<String, String> = HashMap::new();
    assert!(assert_schema_version(&md).is_err());
}
