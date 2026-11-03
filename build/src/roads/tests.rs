//! Regressions for observation admission, physical count scope, disconnected geometry and speed-only transitions.

use super::{
    allocation,
    input::Road,
    matching::Matcher,
    model::Model,
    observation::{CountBasis, Observation, TimeBasis},
    observation_io, speed_transitions,
};
use crate::year::YearInputs;
use serde_json::json;
use std::path::Path;

fn model() -> Model {
    let model: Model =
        serde_json::from_str(include_str!("../../test-data/road-preparation.json")).unwrap();
    model.validate().unwrap();
    model
}

fn road(id: i64) -> Road {
    Road {
        way_id: id,
        segment_idx: 0,
        ends: [(0, 0), (1, 0)],
        length_m: 100.0,
        name: "Main Street".into(),
        speed_limit: 0,
        speed_taper: 0,
        start: (0.0, 0.0),
        end: (100.0, 0.0),
        direction: 1,
        class: 2,
        lanes: 0,
        access: 0,
        tunnel: false,
        built_up: 1,
        roundabout: false,
        country: (0, 0),
        source_id: 1,
        observation_source_id: 1,
        measured: true,
        counts: [80.0, 10.0, 5.0, 5.0],
        basis: 2,
        estimated: 0,
        observation: "station".into(),
        reference: "A1".into(),
        corridor: "A1".into(),
    }
}

fn observation() -> Observation {
    Observation {
        identity: "station".into(),
        input_id: "survey".into(),
        source_id: 1,
        source_rank: 10,
        measured: true,
        country: 0,
        observed_start: "2000-01-01".into(),
        observed_end: "2000-12-31".into(),
        count_basis: CountBasis::BothDirections,
        time_basis: TimeBasis::AnnualDaily,
        direction: 0,
        reference: "A1".into(),
        geometry: vec![vec![[0.0, 0.0], [0.001, 0.0]]],
        road_class_mask: 1 << 2,
        search_radius_m: 100.0,
        reference_required: true,
        vehicle_classes: [Some(80.0), None, Some(0.0), None],
        total: Some(100.0),
        estimated_classes: 0,
        daily_factor: 1.0,
    }
}

fn inputs(directory: &Path) -> YearInputs {
    let input = |id, kind, dependencies| {
        json!({"id":id,"kind":kind,"region":"world","publisher":"Test",
        "url":"https://example.org/survey","observed":{"start":"2000","end":"2000"},"published":"2002",
        "public_ok":true,"licence":"CC0","description":"Measured fixture.","depends_on":dependencies})
    };
    let manifest = json!({"year":"2000","as_of":"2001-01-01","built":false,
        "flows":{"start":"2000-01-01","end":"2001-01-01"},
        "inputs":[input("survey","prepare-road-counts",vec![]), input("observations","road-observations",vec!["survey"])],"unavailable":[]});
    let path = directory.join("manifest.json");
    std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    YearInputs::read(&path, directory).unwrap()
}

#[test]
fn observed_zero_missing_class_and_count_scope_survive_arrow_without_future_leakage() {
    let directory = tempfile::tempdir().unwrap();
    let inputs = inputs(directory.path());
    let path = directory.path().join("counts.arrow");
    let mut rows = vec![observation()];
    let mut directional = rows[0].clone();
    directional.identity = "reverse".into();
    directional.direction = 2;
    directional.count_basis = CountBasis::Directional;
    directional.geometry.push(vec![[1.0, 1.0], [1.001, 1.0]]);
    rows.push(directional);
    observation_io::write(&path, &rows, &inputs).unwrap();
    assert_eq!(observation_io::read(&path, &inputs).unwrap(), rows);
    rows[0].observed_end = "2001-01-01".into();
    let rejected = directory.path().join("later.arrow");
    assert!(observation_io::write(&rejected, &rows, &inputs).is_err());
    assert!(!rejected.exists());
    let mut different = inputs;
    different.manifest.as_of = "2002-01-01".into();
    std::fs::write(&different.manifest_path, b"changed manifest bytes").unwrap();
    assert!(observation_io::read(&path, &different).is_err());
}

#[test]
fn parallel_carriageways_share_a_section_but_keep_directional_measurements_and_street_sections() {
    let model = model();
    let own = road(1);
    let mut other = road(2);
    other.start = (0.0, 10.0);
    other.end = (100.0, 10.0);
    other.direction = 2;
    let (counts, _, whole) = allocation::allocate(&own, &[&other], &model);
    assert_eq!(counts, [40.0, 5.0, 2.5, 2.5]);
    assert_eq!(whole, 100.0);
    let mut directional = own.clone();
    directional.basis = 1;
    assert_eq!(
        allocation::allocate(&directional, &[], &model),
        ([80.0, 10.0, 5.0, 5.0], 0, 0.0)
    );
    let mut street = own.clone();
    street.basis = 4;
    street.estimated = 15;
    assert_eq!(
        allocation::allocate(&street, &[], &model),
        ([80.0, 10.0, 5.0, 5.0], 15, 100.0)
    );
    // A downstream piece of the same carriageway is not another share of its section.
    other.way_id = own.way_id;
    assert!(!allocation::compatible_alternative(&own, &other));
}

#[test]
fn allocation_changes_only_where_another_carriageway_enters_the_section() {
    let model = model();
    let mut own = road(1);
    own.basis = 4;
    let mut other = road(2);
    other.start = (50.0, 10.0);
    other.end = (100.0, 10.0);
    other.direction = 2;
    other.basis = 4;
    let intervals = allocation::intervals(&own, &[&other], &model);
    assert_eq!(
        intervals,
        vec![
            (0.0, 0.5, [80.0, 10.0, 5.0, 5.0], 0, 100.0),
            (0.5, 1.0, [40.0, 5.0, 2.5, 2.5], 15, 100.0)
        ]
    );
}

#[test]
fn matching_uses_priority_scope_and_geometry_without_inventing_a_connector() {
    let model = model();
    let mut first = observation();
    first.geometry = vec![vec![[-1.0, 0.0]], vec![[1.0, 0.0]]];
    let mut target = road(1);
    target.source_id = 0;
    Matcher::new(vec![first])
        .apply(&mut target, &model)
        .unwrap();
    assert_eq!(target.source_id, 0);
    let low = observation();
    let mut high = low.clone();
    high.source_id = 2;
    high.source_rank = 20;
    high.geometry = vec![vec![[0.0, 0.0001], [0.001, 0.0001]]];
    Matcher::new(vec![low, high.clone()])
        .apply(&mut target, &model)
        .unwrap();
    assert_eq!(target.source_id, 2);
    high.direction = 2;
    high.count_basis = CountBasis::Directional;
    target.source_id = 0;
    Matcher::new(vec![high.clone()])
        .apply(&mut target, &model)
        .unwrap();
    assert_eq!(target.source_id, 0);
    high.direction = 1;
    high.road_class_mask = 1 << 10;
    Matcher::new(vec![high]).apply(&mut target, &model).unwrap();
    assert_eq!(target.source_id, 0);
}

#[test]
fn transitions_change_only_implicit_speed_and_stop_at_a_junction() {
    let model = model();
    let mut first = road(1);
    first.country.0 = u16::from_le_bytes(*b"CZ");
    first.speed_limit = 50;
    first.length_m = 50.0;
    let mut second = first.clone();
    second.segment_idx = 1;
    second.ends = [(1, 0), (2, 0)];
    second.speed_limit = 0;
    let mut third = second.clone();
    third.segment_idx = 2;
    third.ends = [(2, 0), (3, 0)];
    let mut rows = vec![first.clone(), second.clone(), third];
    speed_transitions::apply(&mut rows, &model.speed_transitions);
    assert_eq!(
        rows.iter().map(|r| r.speed_taper).collect::<Vec<_>>(),
        [0, 54, 62]
    );
    assert!(
        rows.iter()
            .all(|r| r.counts == first.counts && r.source_id == first.source_id)
    );
    let mut side = first.clone();
    side.way_id = 2;
    side.ends = [(1, 0), (1, 1)];
    let mut junction = vec![first, second, side];
    speed_transitions::apply(&mut junction, &model.speed_transitions);
    assert!(junction.iter().all(|r| r.speed_taper == 0));
}
