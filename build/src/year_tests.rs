//! Admission edges, publication separation, dependency closure and portable input identities.
use super::*;
use serde_json::json;

fn manifest() -> Manifest {
    serde_json::from_value(json!({"year":"2000","built":false,"as_of":"2001-01-01",
        "flows":{"start":"2000-01-01","end":"2001-01-01"},
        "inputs":[{"id":"counts","kind":"prepare-road-counts","region":"CZ","publisher":"Authority",
          "url":"https://example.org/counts","observed":{"start":"2000","end":"2000"},"published":"2002",
          "licence":"CC0","public_ok":true,"depends_on":[],"description":"Annual counts."}],
        "unavailable":[]})).unwrap()
}
#[test]
fn observation_ranges_are_exclusive_at_the_state_cutoff_not_at_publication() {
    let mut m = manifest();
    m.validate().unwrap();
    assert!(m.admitted("counts").unwrap());
    for (start, end, admitted) in [
        ("1999", "2000", true),
        ("2000-11", "2000-12", true),
        ("2000-12-31", "2000-12-31", true),
        ("2001-01-01", "2001-01-01", false),
        ("2000", "2001", false),
    ] {
        m.inputs[0].observed = Interval {
            start: start.into(),
            end: end.into(),
        };
        assert_eq!(m.admitted("counts").unwrap(), admitted);
    }
    for invalid in ["latest", "2000-02-30", "2000-1-1", ""] {
        m.inputs[0].observed = Interval {
            start: invalid.into(),
            end: invalid.into(),
        };
        assert!(m.validate().is_err());
    }
}

#[test]
fn published_osm_manifests_admit_their_snapshot_and_region_dependencies() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../years");
    for year in ["2024", "2026"] {
        let m = Manifest::read(&root.join(format!("{year}.json"))).unwrap();
        for id in [
            "osm",
            "osm-boundaries",
            "osm-geography-identities",
            "osm-regions",
        ] {
            assert!(m.admitted(id).unwrap(), "{year}: {id}");
        }
        let speeds = m.inputs.iter().find(|i| i.kind == Kind::PrepareOsmSpeeds);
        if let Some(input) = speeds {
            assert!(m.admitted(&input.id).unwrap());
        } else {
            assert!(
                m.unavailable
                    .iter()
                    .any(|i| i.kind == Kind::PrepareOsmSpeeds)
            );
        }
    }
}
#[test]
fn fitted_tables_require_every_training_input_and_reject_cycles() {
    let mut m = manifest();
    let mut later:Input=serde_json::from_value(json!({"id":"later","kind":"model-observations","region":"world","publisher":"Authority","url":"https://example.org/survey","observed":{"start":"2001","end":"2001"},"published":null,"licence":"CC0","public_ok":true,"depends_on":[],"description":"Later survey."})).unwrap();
    later.depends_on = vec![];
    m.inputs.push(later);
    let fitted: Input=serde_json::from_value(json!({"id":"fit","kind":"road-period-shares","region":"world","publisher":"Model","url":"https://example.org/method","observed":{"start":"1999","end":"2000"},"published":null,"licence":"CC0","public_ok":true,"depends_on":["counts","later"],"description":"Fit to both surveys."})).unwrap();
    assert_eq!(fitted.depends_on.len(), 2);
    m.inputs.push(fitted);
    assert!(m.admitted("counts").unwrap());
    assert!(!m.admitted("fit").unwrap());
    m.inputs[1].observed = Interval {
        start: "1999".into(),
        end: "1999".into(),
    };
    assert!(m.admitted("fit").unwrap());
    m.inputs[1].depends_on = vec!["fit".into()];
    assert!(m.validate().unwrap_err().contains("cycle"));
}
#[test]
fn practices_cover_the_instant_before_as_of() {
    let mut m = manifest();
    m.validity.insert(
        "counts".into(),
        Interval {
            start: "2000-06-01".into(),
            end: "2001-01-01".into(),
        },
    );
    assert!(m.admitted("counts").unwrap());
    m.validity.get_mut("counts").unwrap().end = "2000-12-31".into();
    assert!(m.admitted("counts").unwrap());
    m.validity.get_mut("counts").unwrap().end = "2000-12-30".into();
    assert!(!m.admitted("counts").unwrap());
    m.validity.get_mut("counts").unwrap().start = "2001-01-01".into();
    m.validity.get_mut("counts").unwrap().end = "2002-01-01".into();
    assert!(!m.admitted("counts").unwrap());
}
#[test]
fn flow_days_are_half_open_with_explicit_exclusions() {
    let mut m = manifest();
    assert_eq!(m.window().baseline_days.len(), 366);
    m.flows.excluded_days.insert("2000-01-01".into());
    assert_eq!(m.window().baseline_days.len(), 365);
    m.flows.increment_days.push("2000-01-01".into());
    assert!(m.validate().is_err());
    m.flows.increment_days = vec!["2001-01-01".into()];
    assert!(m.validate().is_err());
}
#[test]
fn a_citation_date_after_the_observation_is_flagged_independently_of_admission() {
    let m = manifest();
    let observed = m.inputs[0].observed.bounds().unwrap().1;
    for url in [
        "https://example.org/2001.04.08_Stations.pdf",
        "roads/20010408.csv",
        "https://example.org/2001-04-08/data",
    ] {
        assert!(dates::citation_dates(url).iter().any(|&day| day > observed));
    }
    assert!(m.admitted("counts").unwrap());
}
#[test]
fn every_manifest_kind_region_and_dependency_is_valid() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../years");
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.file_stem().is_some_and(|s| {
            s.to_string_lossy().len() == 4
                && s.to_string_lossy().bytes().all(|b| b.is_ascii_digit())
        }) {
            let manifest = Manifest::read(&path).unwrap();
            for input in &manifest.inputs {
                let observed_end = input.observed.bounds().unwrap().1;
                let publication = input.published.as_ref().map(|day| {
                    Interval {
                        start: day.clone(),
                        end: day.clone(),
                    }
                    .bounds()
                    .unwrap()
                });
                for citation in [&input.id, &input.url] {
                    for day in dates::citation_dates(citation) {
                        assert!(
                            day <= observed_end
                                || publication
                                    .is_some_and(|(start, end)| start <= day && day <= end),
                            "{}: {} has citation date {day} after observation {observed_end}; record its true observation or publication",
                            path.display(),
                            input.id
                        );
                    }
                }
            }
        }
    }
    let kinds = include_str!("../../frontend/scripts/input-kinds.ts");
    assert_eq!(
        kinds,
        Kind::typescript(),
        "regenerate with qm-build input-kinds"
    );
    let mut value = serde_json::to_value(json!({"kind":"prepare-road-typo"})).unwrap();
    assert!(serde_json::from_value::<Kind>(value["kind"].take()).is_err());
    let mut m = manifest();
    m.inputs[0].region = "unknown".into();
    assert!(m.validate().is_err());
}
#[test]
fn solar_geometry_is_a_calendar_cycle_including_leap_years() {
    let mut m = manifest();
    assert_eq!(m.solar_days().unwrap().len(), 74);
    m.as_of = "2002-01-01".into();
    assert_eq!(m.solar_days().unwrap().len(), 73);
}

#[test]
fn historical_preparation_admission_is_separate_from_release_completion() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../years");
    let historical = Manifest::read(&root.join("2024.json")).unwrap();
    assert!(!historical.built);
    for id in [
        "rail-movements",
        "rail-model",
        "osm-geography",
        "prepared",
        "airport-traffic",
    ] {
        assert!(historical.admitted(id).unwrap(), "{id}");
    }
    assert!(!historical.admitted("building-road-models").unwrap());
    let reference = Manifest::read(&root.join("2026.json")).unwrap();
    assert!(reference.admitted("prepared").unwrap());
}
