//! Publisher regressions cover percentages, missing classes, numeric identities and non-data CSV footers.

use super::{bast_daily, city, fields, nzta, tests::inputs};
use serde_json::json;

#[test]
fn a_city_way_identity_serialized_as_decimal_still_targets_the_original_way() {
    let (directory, _) = inputs("CZ");
    let manifest = directory.path().join("manifest.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
    value["inputs"][0]["region"] = json!("CZ:Example");
    std::fs::write(&manifest, serde_json::to_vec(&value).unwrap()).unwrap();
    let inputs = crate::year::YearInputs::read(&manifest, directory.path()).unwrap();
    let recipe = serde_json::from_value(json!({"source":{"input_id":"survey","source_id":10,"source_rank":1},"weekday_to_model_daily":1.0,"modeled":false,"placeholder_zero_trucks":false,"search_radius_m":70.0,"minimum_alignment_cosine":0.8})).unwrap();
    let feature = |id| json!({"properties":{"osmid":id,"AADT":100.0},"geometry":{"type":"LineString","coordinates":[[0.0,0.0],[0.001,0.0]]}});
    let rows = city::convert(
        &recipe,
        &inputs,
        &json!({"features":[feature(123.0),feature(123.5)]}),
    )
    .unwrap();
    assert_eq!(rows[0].target.source_way, Some(123));
    assert_eq!(rows[1].target.source_way, None);
}

#[test]
fn a_heavy_percentage_is_converted_to_vehicles_and_an_invalid_split_stays_unknown() {
    let (directory, inputs) = inputs("NZ");
    let path = directory.path().join("counts.json");
    let feature = |heavy| json!({"properties":{"trafficADTCount":1000,"loadingPcHeavy":heavy,"ONRC":"National"},"geometry":{"type":"Point","coordinates":[174.0,-40.0]}});
    std::fs::write(
        &path,
        serde_json::to_vec(&json!({"features":[feature(20),feature(-235)]})).unwrap(),
    )
    .unwrap();
    let recipe = serde_json::from_value(json!({"source":{"input_id":"survey","source_id":1090,"source_rank":1},"files":[],"carriageways":true,"search_radius_m":100.0})).unwrap();
    let rows = nzta::convert(&recipe, &inputs, &path).unwrap();
    assert_eq!(
        rows[0].daily_classes([0.79, 0.04, 0.16, 0.01]).unwrap().0,
        [790.0, 40.0, 160.0, 10.0]
    );
    assert_eq!(rows[1].total, Some(1000.0));
    assert!(rows[1].vehicle_classes[1].is_none() && rows[1].vehicle_classes[2].is_none());
    assert_eq!(rows[0].identity, "survey:0");
}

#[test]
fn a_publisher_footer_does_not_discard_the_preceding_annual_counters() {
    let (directory, inputs) = inputs("DE");
    let path = directory.path().join("annual.csv");
    std::fs::write(&path,"DZ_Nr;DTV_Kfz_MobisSo_Q;Koor_WGS84_E;Koor_WGS84_N;Str_Kl;Str_Nr\n123;1.234;13,4;52,5;A;1\nCopyright publisher\n").unwrap();
    let recipe = serde_json::from_value(json!({"source":{"input_id":"survey","source_id":22,"source_rank":1},"search_radius_m":1000.0})).unwrap();
    let rows = bast_daily::convert(&recipe, &inputs, &path).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].total, Some(1234.0));
    assert_eq!(rows[0].geometry, vec![vec![[13.4, 52.5]]]);
    assert_eq!(rows[0].vehicle_classes, [None; 4]);
}

#[test]
fn overlapping_tls_vehicle_groups_are_counted_once_and_unclassified_traffic_is_not_measured_light()
{
    let (directory, inputs) = inputs("DE");
    let path = directory.path().join("annual.csv");
    std::fs::write(&path,"DZ_Nr;DTV_Kfz_MobisSo_Q;Koor_WGS84_E;Koor_WGS84_N;Str_Kl;Str_Nr;pPLZ_MobisSo_Q;pLfw_MobisSo_Q;pPmA_MobisSo_Q;pLoA_MobisSo_Q;pBus_MobisSo_Q;pLzg_MobisSo_Q;pSat_MobisSo_Q;pMot_MobisSo_Q;pSon_MobisSo_Q\n123;10.000;13,4;52,5;A;1;86;8;1;3;0,2;8;6;1;1,8\n").unwrap();
    let recipe = serde_json::from_value(json!({"source":{"input_id":"survey","source_id":22,"source_rank":1},"search_radius_m":1000.0})).unwrap();
    let rows = bast_daily::convert(&recipe, &inputs, &path).unwrap();
    assert_eq!(
        rows[0].vehicle_classes,
        [None, Some(320.0), Some(800.0), Some(100.0)]
    );
    let (classes, status) = rows[0].daily_classes([0.8, 0.1, 0.09, 0.01]).unwrap();
    assert_eq!(classes, [8780.0, 320.0, 800.0, 100.0]);
    assert_eq!(status, 1);
}

#[test]
fn network_only_lines_supply_no_count_and_a_known_total_keeps_its_observed_residual() {
    let (_directory, inputs) = inputs("CZ");
    let recipe = serde_json::from_value(json!({"input_id":"survey","records":"/features","properties":"/properties","identity":"id","reference":null,"geometry":{"format":"geojson"},"source_id":1,"source_rank":1,"country":"CZ","measured":true,"count_basis":"street-cross-section","time_basis":"annual-daily","daily_factor":1.0,"direction":0,"road_class_mask":4,"search_radius_m":50.0,"reference_required":false,"classes":[null,null,["truck"],null],"total":"total","year_field":null,"estimated_classes":10,"percentages":true,"fixed_fractions":[null,0.0,null,0.0],"residual_class":0})).unwrap();
    let feature = |total, truck| json!({"properties":{"id":"one","total":total,"truck":truck},"geometry":{"type":"Point","coordinates":[0.0,0.0]}});
    let empty = fields::observations(
        &recipe,
        &json!({"features":[feature(None::<f64>,None::<f64>)]}),
        &inputs,
    )
    .unwrap();
    assert!(empty.is_empty());
    let rows = fields::observations(
        &recipe,
        &json!({"features":[feature(Some(100.0),Some(20.0))]}),
        &inputs,
    )
    .unwrap();
    assert_eq!(
        rows[0].vehicle_classes,
        [Some(80.0), Some(0.0), Some(20.0), Some(0.0)]
    );
    assert_eq!(rows[0].estimated_classes, 10);
}
#[test]
fn the_selected_gpr_recipe_names_the_edition_specific_identity_column() {
    let (_directory, inputs) = super::tests::inputs("PL");
    let recipe = serde_json::from_value(serde_json::json!({
        "source":{"input_id":"survey","source_id":1097,"source_rank":1},
        "search_radius_m":500.0,"identity_field":"section_key"
    }))
    .unwrap();
    let rows = super::gpr::convert(
        &recipe,
        &inputs,
        &serde_json::json!([{
            "section_key":"section-a", "ref":"DK1", "isProvincial":false,
            "coords":[[19.0,51.0],[19.01,51.0]], "aadt_light":80,
            "aadt_medium":10,"aadt_heavy":5,"aadt_moto":5
        }]),
    )
    .unwrap();
    assert_eq!(rows[0].identity, "section-a");
    assert_eq!(rows[0].total, Some(100.0));
}

#[test]
fn a_counted_slip_is_retained_for_a_nearby_link_without_dividing_its_directional_flow() {
    let (directory, inputs) = inputs("GB");
    let path = directory.path().join("counts.csv");
    std::fs::write(&path, "count_point_id,year,road_category,estimation_method,road_name,longitude,latitude,all_motor_vehicles,cars_and_taxis,LGVs,buses_and_coaches,all_HGVs,HGVs_2_rigid_axle,two_wheeled_motor_vehicles,link_length_km,start_junction_road_name,end_junction_road_name\n1,2000,TM,Counted,M1,0,0,100,70,10,5,10,3,5,0.4,slip road,A1\n").unwrap();
    let recipe = serde_json::from_value(json!({"source":{"input_id":"survey","source_id":1,"source_rank":1},"archive_entry":null,"excluded_manual_years":[],"maximum_age_years":10,"major_radius_m":15000.0,"minor_assignment":{"maximum_distance_m":12.0,"clearance_m":20.0},"slip_maximum_length_km":1.0,"slip_junction_pattern":"(?i)ramp|slip","slip_total_ratio":0.25,"slip_neighbor_radius_m":10000.0})).unwrap();
    let rows = super::dft::convert(&recipe, &inputs, &path).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].vehicle_classes, [80.0, 8.0, 7.0, 5.0].map(Some));
    assert_eq!(
        rows[0].count_basis,
        crate::roads::observation::CountBasis::Directional
    );
    assert_eq!(rows[0].road_class_mask & ((1 << 10) - 1), 0);
    assert!(
        !rows[0]
            .target
            .way_assignment
            .as_ref()
            .unwrap()
            .require_class_clearance
    );
    assert_eq!(
        rows[0]
            .target
            .way_assignment
            .as_ref()
            .unwrap()
            .maximum_distance_m,
        12.0
    );
}
