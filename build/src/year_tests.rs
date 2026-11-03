//! Manifest admission, date boundaries, relocation and configuration errors.

use super::*;

fn manifest() -> Manifest {
    serde_json::from_value(serde_json::json!({
        "as_of": "2001-01-01",
        "flows": {"start":"2000-01-01", "end":"2001-01-01"},
        "inputs": [{"id":"counts", "kind":"road-counts", "region":"CZ", "publisher":"Publisher",
            "url":"https://example.org/counts", "vintage":"2000", "licence":"CC0",
            "public_ok":true, "path":"roads/counts.json", "description":"Annual counts"}],
        "unavailable": [{"id":"bus", "kind":"bus", "region":"world", "reason":"No mapped routes; no added buses."}]
    })).unwrap()
}

#[test]
fn admission_uses_the_described_vintage_and_refuses_future_inputs() {
    let mut year = manifest();
    year.validate().unwrap();
    year.inputs[0].vintage = year.as_of.clone();
    year.validate().unwrap();
    for future in ["2001-01-02", "2001"] {
        year.inputs[0].vintage = future.into();
        let error = year.validate().unwrap_err();
        assert!(
            error.contains("counts") && error.contains(future) && error.contains(&year.as_of),
            "{error}"
        );
    }
    for invalid in ["2000-02-30", "2000-1-1", "latest", ""] {
        year.inputs[0].vintage = invalid.into();
        assert!(year.validate().is_err());
    }
}

#[test]
fn a_flow_window_is_half_open_and_samples_cannot_escape_it() {
    let mut year = manifest();
    assert_eq!(year.window().baseline_days.len(), 366);
    assert_eq!(year.window().baseline_days.last().unwrap(), "2000-12-31");
    year.flows.increment_days = vec!["2001-01-01".into()];
    assert!(year.validate().is_err());
    year.flows.increment_days = vec!["2000-01-01".into(); 2];
    assert!(year.validate().is_err());
    year.flows.increment_days.clear();
    year.flows.end = "2001-01-02".into();
    assert!(year.validate().is_err());
}

#[test]
fn source_paths_are_portable_and_never_escape_the_sources_root() {
    let mut year = manifest();
    for path in [
        "/host/source",
        "../source",
        "roads/../../source",
        "C:\\source",
        "",
    ] {
        year.inputs[0].path = path.into();
        assert!(year.validate().is_err(), "{path}");
    }
    year.inputs[0].path = "roads/counts.json".into();
    year.validate().unwrap();
}

#[test]
fn missing_inputs_are_explicit_and_cannot_also_be_available() {
    let inputs = YearInputs {
        manifest: manifest(),
        paths: BTreeMap::new(),
    };
    assert!(inputs.optional("bus").unwrap().is_none());
    assert!(
        inputs
            .optional("misspelled-kind")
            .unwrap_err()
            .contains("unavailable")
    );
    let mut year = manifest();
    year.unavailable.push(Unavailable {
        id: "counts".into(),
        kind: "road-counts".into(),
        region: "CZ".into(),
        reason: "no counts".into(),
    });
    assert!(year.validate().is_err());
}

#[test]
fn every_product_manifest_obeys_the_same_admission_rule() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../years");
    let mut count = 0;
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            Manifest::read(&path).unwrap();
            count += 1;
        }
    }
    assert!(count > 0);
}

#[test]
fn the_solar_cycle_uses_the_manifest_calendar_including_leap_years() {
    let mut year = manifest();
    let days = year.solar_days().unwrap();
    assert_eq!(days.len(), 74);
    assert_eq!(days[0], crate::period::day_start("2000-01-01").unwrap());
    assert_eq!(
        *days.last().unwrap(),
        crate::period::day_start("2000-12-31").unwrap()
    );
    year.as_of = "2002-01-01".into();
    assert_eq!(year.solar_days().unwrap().len(), 73);
}
