//! Publisher regressions preserve twin direction, class missingness and the admitted observation interval.

use super::{dft, hpms, inweva, mlit, sict, thai_drr};
use crate::{
    roads::observation::{CountBasis, TimeBasis},
    year::YearInputs,
};
use serde_json::json;

pub(super) fn inputs(country: &str) -> (tempfile::TempDir, YearInputs) {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(directory.path().join("2000/survey")).unwrap();
    let manifest = json!({"year":"2000","as_of":"2001-01-01","built":false,"flows":{"start":"2000-01-01","end":"2001-01-01"},"inputs":[{
        "id":"survey","kind":"prepare-road-counts","region":country,"publisher":"Test","url":"https://example.org/counts","observed":{"start":"2000","end":"2000"},"published":"2002","licence":"CC0","public_ok":true,"depends_on":[],"description":"Historical counter fixture."}],"unavailable":[]});
    let path = directory.path().join("manifest.json");
    std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let inputs = YearInputs::read(&path, directory.path()).unwrap();
    (directory, inputs)
}

#[test]
fn a_later_publication_does_not_admit_a_future_dft_count_or_relabel_an_estimate_as_measured() {
    let (directory, _) = inputs("GB");
    let path = directory.path().join("manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    manifest["inputs"][0]["observed"]["start"] = json!("1990");
    std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let inputs = YearInputs::read(&path, directory.path()).unwrap();
    let data = "count_point_id,year,road_category,estimation_method,road_name,longitude,latitude,all_motor_vehicles,cars_and_taxis,LGVs,buses_and_coaches,all_HGVs,two_wheeled_motor_vehicles\n1,2000,PA,Estimated,A1,0,0,100,70,10,5,10,5\n1,2001,PA,Counted,A1,0,0,1000,700,100,50,100,50\n2,1999,MCU,Counted,U,1,1,10,7,1,0,1,1\n2,2000,MCU,Estimated,U,1,1,20,14,2,0,2,2\n";
    let file = directory.path().join("counts.csv");
    std::fs::write(&file, data).unwrap();
    let recipe=serde_json::from_value(json!({"source":{"input_id":"survey","source_id":1,"source_rank":1},"archive_entry":null,"excluded_manual_years":[],"maximum_age_years":10,"major_radius_m":15000.0,"minor_assignment":{"maximum_distance_m":12.0,"clearance_m":20.0},"slip_maximum_length_km":1.0,"slip_junction_pattern":"(?i)ramp|slip","slip_total_ratio":0.25,"slip_neighbor_radius_m":10000.0})).unwrap();
    let rows = dft::convert(&recipe, &inputs, &file).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].total, Some(100.0));
    assert!(!rows[0].measured);
    assert_eq!(rows[1].observed_end, "1999-12-31");
    assert!(rows[1].target.way_assignment.is_some());
}

#[test]
fn sict_parts_share_an_identity_without_losing_the_measured_split_under_legacy_flags() {
    let (directory, inputs) = inputs("MX");
    let root = directory.path().join("2000/survey");
    std::fs::remove_dir(&root).unwrap();
    std::fs::write(&root,"segment_mongo_id,comp_A,comp_OTROS,comp_B,comp_C2,comp_C3,comp_T3S2,comp_T3S3,comp_T3S2R4,comp_M\ncount,80,0,5,0,10,0,0,0,5\n").unwrap();
    let feature = |x| json!({"properties":{"segment_mongo_id":"count","total":100,"red_ok":"Federal","operacion":"Cuota"},"geometry":{"type":"LineString","coordinates":[[x,0.0],[x+0.01,0.0]]}});
    let path = directory.path().join("lines.json");
    std::fs::write(
        &path,
        serde_json::to_vec(&json!({"features":[feature(0.0),feature(1.0)]})).unwrap(),
    )
    .unwrap();
    let recipe=serde_json::from_value(json!({"source":{"input_id":"survey","source_id":1,"source_rank":1},"composition_input":"survey","total_field":"total","search_radius_m":200.0})).unwrap();
    let rows = sict::convert(&recipe, &inputs, &path).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].geometry.len(), 2);
    assert_eq!(rows[0].vehicle_classes, [80.0, 5.0, 10.0, 5.0].map(Some));
    assert_eq!(rows[0].estimated_classes, 0);
}

#[test]
fn hpms_truncates_fractional_totals_and_preserves_unknown_direction_and_equal_twins() {
    let (directory, inputs) = inputs("US");
    let feature = |id, facility, y| json!({"id":id,"properties":{"AADT":1000.5,"F_SYSTEM":1,"FACILITY_TYPE":facility},"geometry":{"type":"LineString","coordinates":[[0.0,y],[0.01,y]]}});
    let raw = json!({"features":[feature("unknown",None::<u8>,0.0),feature("twin-one",Some(1),0.0001),feature("twin-two",Some(6),0.0002),feature("directional",Some(1),1.0)]});
    let root = directory.path().join("2000/survey");
    std::fs::write(root.join("records.json"), serde_json::to_vec(&raw).unwrap()).unwrap();
    std::fs::write(root.join("terminal.json"), br#"{"features":[]}"#).unwrap();
    let recipe = serde_json::from_value(json!({"source":{"input_id":"survey","source_id":1,"source_rank":1},"files":["records.json","terminal.json"],"functional_class_max":5,"search_radius_m":100.0,"minimum_alignment_cosine":0.86,"twin_radius_m":300.0,"twin_total_ratio_tolerance":0.15})).unwrap();
    let rows = hpms::convert(&recipe, &inputs, &root).unwrap();
    let row = |name| rows.iter().find(|r| r.identity == name).unwrap();
    assert_eq!(row("unknown").count_basis, CountBasis::Unknown);
    assert_eq!(row("twin-one").count_basis, CountBasis::BothDirections);
    assert_eq!(row("directional").count_basis, CountBasis::Directional);
    assert_eq!(row("unknown").vehicle_classes, [None; 4]);
}

#[test]
fn reciprocal_loops_keep_each_direction_and_sum_only_for_an_undivided_mapped_road() {
    let (_directory, inputs) = inputs("NL");
    let source = json!({"input_id":"survey","source_id":1,"source_rank":1});
    let recipe=serde_json::from_value(json!({"source":source,"search_radius_m":50.0,"minimum_alignment_cosine":0.8660254037844387})).unwrap();
    let feature = |id, twin, reference, counts: [u32; 3]| json!({"properties":{"vbn_id":id,"vbn_id_tgn":twin,"bnsubsrt_b":"HR","afst_mtw":0,"al_e_wk":counts.iter().sum::<u32>(),"l1_e_wk":counts[0],"l2_e_wk":counts[1],"l3_e_wk":counts[2],"wegnrhmp_b":reference,"wegnrhmp_e":reference},"geometry":{"type":"LineString","coordinates":[[5.0,52.0],[5.001,52.0]]}});
    let rows=inweva::convert(&recipe,&inputs,&json!({"features":[feature("1","2","A1",[100,20,10]),feature("2","1","A1",[200,40,30]),feature("3","","N1",[50,10,5])]})).unwrap();
    assert_eq!(rows.len(), 4);
    let summed = rows.iter().find(|r| r.identity == "1-2").unwrap();
    assert_eq!(summed.geometry.len(), 2);
    assert_eq!(summed.count_basis, CountBasis::BothDirections);
    assert_eq!(summed.target.required_oneway, Some(false));
    assert_eq!(
        rows.iter()
            .filter(|r| r.count_basis == CountBasis::Directional)
            .count(),
        3
    );
    assert_eq!(
        rows.iter().find(|r| r.identity == "1").unwrap().total,
        Some(130.0)
    );
    assert_eq!(
        rows.iter().find(|r| r.identity == "2").unwrap().total,
        Some(270.0)
    );
    assert_eq!(summed.vehicle_classes, [None, Some(60.0), Some(40.0), None]);
    assert_eq!(summed.vehicle_groups[0].count, 300.0);
    assert_eq!(summed.estimated_classes, 9);
    assert_eq!(rows[0].observed_end, "2000-12-31");
    assert_eq!(rows[0].time_basis, TimeBasis::AnnualDaily);
}

#[test]
fn a_blank_drr_class_is_not_a_measured_zero_even_when_the_legacy_mask_says_fifteen() {
    let (_directory, inputs) = inputs("TH");
    let recipe = serde_json::from_value(
        json!({"source":{"input_id":"survey","source_id":1,"source_rank":1},"road_class_mask":31}),
    )
    .unwrap();
    let data = "road_code,sum_AADT,MC,SV,SVT,TB2,TB3,T4,ART3,ART4,ART5,ART6,BD,DRT\nA,100,0,100,0,0,0,0,0,0,0,0,0,0\nB,100,,100,0,0,0,0,0,0,0,0,0,0\n";
    let rows = thai_drr::convert(&recipe, &inputs, data.as_bytes()).unwrap();
    assert_eq!(
        rows.iter()
            .find(|r| r.reference == "A")
            .unwrap()
            .vehicle_classes[3],
        Some(0.0)
    );
    assert_eq!(
        rows.iter()
            .find(|r| r.reference == "B")
            .unwrap()
            .vehicle_classes[3],
        None
    );
    assert!(
        rows.iter()
            .all(|r| r.vehicle_classes[0] == Some(100.0) && r.estimated_classes == 0)
    );
}

#[test]
fn mlit_uses_declared_objects_and_keeps_the_weekday_basis_of_unlocated_summaries() {
    let (directory, inputs) = inputs("JP");
    let mut writer = csv::Writer::from_writer(Vec::new());
    writer.write_record(vec!["header"; 62]).unwrap();
    for (small, large) in [(100, 10), (200, 20)] {
        let mut row = vec![String::new(); 62];
        row[3] = "1".into();
        row[5] = "Example Expressway".into();
        row[34] = "1".into();
        row[46] = "1".into();
        row[59] = small.to_string();
        row[60] = large.to_string();
        writer.write_record(row).unwrap();
    }
    let bytes = writer.into_inner().unwrap();
    std::fs::write(directory.path().join("2000/survey/declared.csv"), bytes).unwrap();
    let source = json!({"input_id":"survey","source_id":1,"source_rank":2});
    let recipe=serde_json::from_value(json!({"source":source,"fallback_source":{"input_id":"survey","source_id":2,"source_rank":1},"files":["declared.csv"],"weekday_to_model_daily":1.0})).unwrap();
    let rows = mlit::convert(&recipe, &inputs, &directory.path().join("2000/survey")).unwrap();
    let measured = rows.iter().find(|r| r.source_id == 1).unwrap();
    assert_eq!(measured.vehicle_classes, [Some(150.0), None, None, None]);
    assert_eq!(measured.vehicle_groups[0].count, 15.0);
    assert_eq!(measured.time_basis, TimeBasis::WeekdayDaily);
    assert!(measured.geometry.is_empty());
    assert!(
        rows.iter()
            .filter(|r| r.source_id == 2)
            .all(|r| !r.measured && r.estimated_classes == 15)
    );
}

#[test]
fn corrected_dutch_total_preserves_truck_counts_and_an_observed_zero() {
    let (_directory, inputs) = inputs("NL");
    let recipe = serde_json::from_value(json!({"source":{"input_id":"survey","source_id":1,"source_rank":1},"search_radius_m":50.0,"minimum_alignment_cosine":0.86})).unwrap();
    let feature = |id, total, light, medium, heavy| json!({"properties":{"vbn_id":id,"vbn_id_tgn":"","bnsubsrt_b":"HR","afst_mtw":0,"al_e_wk":total,"l1_e_wk":light,"l2_e_wk":medium,"l3_e_wk":heavy,"wegnrhmp_b":"A1","wegnrhmp_e":"A1"},"geometry":{"type":"LineString","coordinates":[[5.0,52.0],[5.001,52.0]]}});
    let rows = inweva::convert(
        &recipe,
        &inputs,
        &json!({"features":[feature("corrected",200,100,20,10),feature("zero",0,0,0,0)]}),
    )
    .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].total, Some(200.0));
    assert_eq!(
        rows[0].vehicle_classes,
        [None, Some(20.0), Some(10.0), None]
    );
    assert_eq!(rows[0].vehicle_groups[0].count, 170.0);
    assert_eq!(rows[0].estimated_classes, 9);
    assert_eq!(rows[1].total, Some(0.0));
    assert_eq!(rows[1].daily_classes([1.0; 4]).unwrap().0, [0.0; 4]);
}
