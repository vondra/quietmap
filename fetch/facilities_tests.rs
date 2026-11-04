//! Publisher conversion preserves identity, units, dates and inactive evidence before matching.

use super::*;
use crate::facilities::model::StatusRule;

fn regions() -> Regions {
    let temporary = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(temporary.path(),r#"{"regions":[{"country":"XY","continent":1,"disputed":false,"polygons":[[[[-2,-2],[2,-2],[2,2],[-2,2],[-2,-2]]]]}]}"#).unwrap();
    Regions::read(temporary.path()).unwrap()
}
fn input() -> crate::year::Input {
    serde_json::from_value(json!({"id":"register","kind":"prepare-industrial-sites","region":"world","publisher":"test","url":"https://example.org","observed":{"start":"2000","end":"2000"},"published":null,"licence":"CC0","public_ok":true,"depends_on":[],"description":"test"})).unwrap()
}

#[test]
fn publisher_identity_units_and_status_survive_record_order_and_unknown_dates() {
    let spec = InputSpec {
        input: "register".into(),
        format: "geojson".into(),
        classifier: "constant".into(),
        activity: 3511,
        identities: vec!["publisher_id".into()],
        names: vec!["Name".into()],
        capacity_fields: vec!["MW".into()],
        capacity_unit: "kW".into(),
        capacity_scale: 1000.0,
        start_fields: vec!["opened".into()],
        status: Some(StatusRule {
            fields: vec!["Status".into()],
            pattern: "^operating$".into(),
            operating_pattern: "^operating$".into(),
            inactive_pattern: "^(retired|mothballed)$".into(),
            blank: false,
        }),
        ..Default::default()
    };
    let raw = vec![
        json!({"type":"Feature","geometry":{"type":"Point","coordinates":[0,0]},"properties":{"publisher_id":"A","Name":"Quoted, plant","MW":"2.5 MW","opened":"1998","Status":"Operating"}}),
        json!({"type":"Feature","geometry":{"type":"Point","coordinates":[1,1]},"properties":{"publisher_id":"B","MW":"","opened":"unknown","Status":"retired"}}),
    ];
    let converted = normalize(
        &spec,
        &input(),
        raw.clone(),
        &regions(),
        &mut HashSet::new(),
    )
    .unwrap();
    assert_eq!(converted[0].identity, "register/A");
    assert_eq!(converted[0].capacity, Some(2500.0));
    assert_eq!(converted[0].operating_from.as_deref(), Some("1998"));
    assert_eq!(converted[1].capacity, None);
    assert_eq!(converted[1].operating_from, None);
    assert_eq!(converted[1].operating_state, 2);
    let reversed = normalize(
        &spec,
        &input(),
        raw.into_iter().rev().collect(),
        &regions(),
        &mut HashSet::new(),
    )
    .unwrap();
    assert_eq!(converted[0].identity, reversed[1].identity);
    assert_eq!(
        converted[0].evidence_identity,
        reversed[1].evidence_identity
    );
}

#[test]
fn planned_retirement_does_not_close_an_operating_unit_and_admission_does_not_claim_operation() {
    assert_eq!(dates::dated_history("2916-09-24", "2000-01-01"), None);
    assert_eq!(dates::dated_history("2001", "2000-01-01"), None);
    assert_eq!(
        dates::dated_history("1999", "2000-01-01").as_deref(),
        Some("1999")
    );
    let mut spec = InputSpec {
        input: "register".into(),
        format: "geojson".into(),
        classifier: "constant".into(),
        activity: 3511,
        end_fields: vec!["Retired_year".into()],
        status: Some(StatusRule {
            fields: vec!["Status".into()],
            pattern: "operating|construction".into(),
            operating_pattern: "^operating$".into(),
            inactive_pattern: "^retired$".into(),
            blank: true,
        }),
        ..Default::default()
    };
    let raw: Vec<_> = ["operating", "retired", "construction", ""].into_iter().map(|status|
        json!({"type":"Feature","geometry":{"type":"Point","coordinates":[0,0]},"properties":{"Status":status,"Retired_year":"1999"}})).collect();
    let rows = normalize(
        &spec,
        &input(),
        raw.clone(),
        &regions(),
        &mut HashSet::new(),
    )
    .unwrap();
    assert_eq!(
        rows.iter().map(|r| r.operating_state).collect::<Vec<_>>(),
        [1, 2, 0, 0]
    );
    assert_eq!(
        rows.iter()
            .map(|r| r.classification_admitted)
            .collect::<Vec<_>>(),
        [1, 0, 1, 1]
    );
    assert_eq!(rows[0].operating_until, None);
    assert_eq!(rows[1].operating_until.as_deref(), Some("1999"));
    assert_eq!(rows[2].operating_until, None);
    assert_ne!(rows[0].evidence_identity, rows[1].evidence_identity);
    spec.status = None;
    let rows = normalize(&spec, &input(), raw, &regions(), &mut HashSet::new()).unwrap();
    assert!(
        rows.iter()
            .all(|r| r.operating_state == 0 && r.classification_admitted == 1)
    );
}

#[test]
fn a_concession_can_cover_owned_land_while_its_centroid_lies_outside_the_scope() {
    let spec = InputSpec {
        input: "register".into(),
        format: "geojson".into(),
        classifier: "constant".into(),
        activity: 700,
        containment: true,
        scope: Some(crate::facilities::model::Scope {
            bbox: [-1.0, -1.0, 1.0, 1.0],
            ..Default::default()
        }),
        geometry_types: vec!["Polygon".into()],
        ..Default::default()
    };
    let raw = vec![
        json!({"type":"Feature","geometry":{"type":"Polygon","coordinates":[[[0,0],[6,0],[6,1],[0,1],[0,0]]]},"properties":{}}),
    ];
    let rows = normalize(&spec, &input(), raw, &regions(), &mut HashSet::new()).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].role, REGISTRY_AREA);
    assert!(rows[0].longitude > 1.0);
}

#[test]
fn csv_quotes_and_annex_subactivities_preserve_publisher_meaning() {
    let temporary = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(
        temporary.path(),
        "latitude,longitude,primary_fuel,name\n0,1,Gas,\"Works, west\"\n",
    )
    .unwrap();
    let raw = features(temporary.path(), "gppd-csv").unwrap();
    assert_eq!(raw[0]["name"], "Works, west");
    assert_eq!(classify::activity("gppd", &raw[0], 0).unwrap(), 3511);
    for (activity, expected) in [
        ("2(c)(ii) — forging", 2550),
        ("2(e)(i)", 2442),
        ("3(a)", 810),
        ("not reported", 0),
    ] {
        assert_eq!(
            classify::activity("annex", &json!({"EPRTRAnnexIMainActivity":activity}), 0).unwrap(),
            expected
        );
    }
}

#[test]
fn a_capacity_weighted_commissioning_year_cannot_date_a_plant_opening() {
    let spec = InputSpec {
        input: "register".into(),
        format: "gppd-csv".into(),
        classifier: "gppd".into(),
        start_fields: vec!["commissioning_year".into()],
        ..Default::default()
    };
    let raw = vec![
        json!({"latitude":"0", "longitude":"0", "primary_fuel":"Gas", "commissioning_year":"1990"}),
    ];
    let rows = normalize(&spec, &input(), raw, &regions(), &mut HashSet::new()).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].activity, 3511);
    assert_eq!(rows[0].operating_from, None);
    assert_eq!(rows[0].operating_state, 0);
}

#[test]
fn proposed_commissioning_is_not_an_operating_interval_or_a_factory_closure() {
    let spec = InputSpec {
        input: "register".into(),
        format: "geojson".into(),
        classifier: "constant".into(),
        activity: 3599,
        start_fields: vec!["start".into()],
        status: Some(StatusRule {
            fields: vec!["status".into()],
            pattern: "^operating$".into(),
            operating_pattern: "^operating$".into(),
            inactive_pattern: "^(retired|mothballed)$".into(),
            blank: false,
        }),
        ..Default::default()
    };
    let raw = vec![
        json!({"type":"Feature","geometry":{"type":"Point","coordinates":[0,0]},"properties":{"start":"2001","status":"construction"}}),
    ];
    let rows = normalize(&spec, &input(), raw, &regions(), &mut HashSet::new()).unwrap();
    assert_eq!(rows[0].operating_state, 0);
    assert_eq!(rows[0].operating_from, None);
}

#[test]
fn written_generation_types_preserve_known_fuels_without_guessing_a_hybrid_mix() {
    for (kind, expected) in [
        ("Termoelectrica Diesel o Gas Oil", 3511),
        ("Termoelectrica Gas Natural", 3511),
        ("Solar Fotovotaica", 3599),
        ("Micro Central Hidroelectrica", 3512),
        ("Solar Hibrido", 0),
    ] {
        assert_eq!(
            classify::activity("generation-type", &json!({"Tipo":kind}), 0).unwrap(),
            expected
        );
    }
}

#[test]
fn numeric_voltage_strings_obey_the_threshold_while_missing_values_remain_unknown() {
    let raw: Vec<_> = [json!(0), json!("0"), json!(66), json!("66"), json!(110), json!(null)]
        .into_iter()
        .map(|voltage| json!({"type":"Feature","geometry":{"type":"Point","coordinates":[0,0]},"properties":{"voltage":voltage}}))
        .collect();
    let mut spec = InputSpec {
        input: "register".into(),
        format: "geojson".into(),
        classifier: "constant".into(),
        activity: 3511,
        minimum_voltage: Some(110.0),
        voltage_fields: vec!["voltage".into()],
        ..Default::default()
    };
    assert_eq!(
        normalize(
            &spec,
            &input(),
            raw.clone(),
            &regions(),
            &mut HashSet::new()
        )
        .unwrap()
        .len(),
        2
    );
    spec.voltage_positive_only = true;
    assert_eq!(
        normalize(&spec, &input(), raw, &regions(), &mut HashSet::new())
            .unwrap()
            .len(),
        4
    );
}

#[test]
fn publisher_fuel_precedence_and_literal_status_remain_distinct() {
    let properties = json!({"Type":"solar hydro"});
    assert_eq!(
        classify::activity("gem-fuel", &properties, 0).unwrap(),
        3599
    );
    assert_eq!(
        classify::activity("explicit-fuel", &properties, 0).unwrap(),
        3512
    );
    let spec = InputSpec {
        input: "register".into(),
        format: "geojson".into(),
        classifier: "constant".into(),
        activity: 500,
        status: Some(StatusRule {
            fields: vec!["status".into()],
            pattern: "(?-i:^Operating$)".into(),
            operating_pattern: "(?-i:^Operating$)".into(),
            ..Default::default()
        }),
        ..Default::default()
    };
    let raw = ["Operating", "operating"].into_iter().map(|status| json!({"type":"Feature","geometry":{"type":"Point","coordinates":[0,0]},"properties":{"status":status}})).collect();
    let rows = normalize(&spec, &input(), raw, &regions(), &mut HashSet::new()).unwrap();
    assert_eq!((rows[0].operating_state, rows[1].operating_state), (1, 0));
}
