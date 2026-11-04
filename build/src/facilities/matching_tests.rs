//! Regression cases for containing ownership, register precedence and historical admission.

use super::{
    geometry, matching,
    model::{InputSpec, Model},
    records::{Facility, INDUSTRY, REGISTRY_AREA, REGISTRY_POINT},
};
use geos::{Geom, Geometry};
use std::collections::BTreeMap;

fn model() -> Model {
    Model {
        inputs: vec![InputSpec {
            input: "register".into(),
            ..Default::default()
        }],
        names: vec![],
        activity_order: BTreeMap::from([(2400, 106.4), (3500, 102.7), (700, 103.9)]),
        coal_sources: vec![],
        subtype_divisions: BTreeMap::from([(1, vec![46, 47, 52])]),
        source_divisions: BTreeMap::from([(4, vec![37])]),
        overlap_minimum_m2: 100_000.0,
        overlap_area_ratio: 2.5,
    }
}
fn area(half_width: f64, source_type: u8) -> Facility {
    let h = half_width;
    let wkt = format!(
        "POLYGON (({} {},{} {},{} {},{} {},{} {}))",
        -h, -h, h, -h, h, h, -h, h, -h, -h
    );
    Facility {
        identity: format!("site/{h}"),
        role: INDUSTRY,
        geometry: Some(Geometry::new_from_wkt(&wkt).unwrap().to_wkb().unwrap()),
        area_m2: Some((h * h * 1e8) as f32),
        source_type,
        source: "osm".into(),
        ..Default::default()
    }
}
fn observation(activity: u16) -> Facility {
    Facility {
        identity: "register/site".into(),
        evidence_identity: "register/site".into(),
        source: "register".into(),
        role: REGISTRY_POINT,
        activity,
        source_id: 10,
        authority_rank: 4,
        classification: 1,
        classification_admitted: 1,
        observed_on: "2001-01-01".into(),
        operating_state: 1,
        ..Default::default()
    }
}
fn classify(sites: &mut [Facility], observations: &[Facility]) {
    matching::classify(sites, observations, &model(), "2002-01-01").unwrap();
}

#[test]
fn a_dedicated_or_quiet_tenant_blocks_fallthrough_to_its_industrial_zone() {
    for (source_type, subtype) in [(12, 0), (0, 1), (4, 0)] {
        let mut tenant = area(0.01, source_type);
        tenant.site_subtype = subtype;
        let mut sites = vec![area(0.1, 0), tenant];
        classify(&mut sites, &[observation(3511)]);
        assert!(sites.iter().all(|s| s.activity == 0));
    }
    let mut sites = vec![area(0.1, 0), area(0.01, 0)];
    classify(&mut sites, &[observation(3511)]);
    assert_eq!((sites[0].activity, sites[1].activity), (0, 3511));
    assert_eq!(sites[1].evidence_identity, "register/site");
}

#[test]
fn first_containing_national_area_follows_points_without_overriding_stronger_evidence() {
    let mut concession = area(0.2, 0);
    let evidence = observation(700);
    concession.activity = evidence.activity;
    concession.source = evidence.source.clone();
    concession.source_id = evidence.source_id;
    concession.authority_rank = 4;
    concession.classification = 1;
    concession.classification_admitted = 1;
    concession.operating_state = 1;
    concession.observed_on = evidence.observed_on;
    concession.role = REGISTRY_AREA;
    for (rank, expected) in [(4, 700), (5, 3511)] {
        let mut point = observation(3511);
        point.authority_rank = rank;
        point.source_id = if rank == 5 { 20 } else { 10 };
        let mut sites = vec![area(0.01, 0)];
        classify(&mut sites, &[point, concession.clone()]);
        assert_eq!(sites[0].activity, expected);
    }
}

#[test]
fn future_snapshots_and_outside_intervals_do_not_classify_historical_sites() {
    for (observed, start, end) in [
        ("2003-01-01", None, None),
        ("2001-01-01", Some("2003-01-01"), None),
        ("2001-01-01", None, Some("2001-12-01")),
    ] {
        let mut input = observation(3511);
        input.observed_on = observed.into();
        input.operating_from = start.map(str::to_owned);
        input.operating_until = end.map(str::to_owned);
        let mut sites = vec![area(0.01, 0)];
        classify(&mut sites, &[input]);
        assert_eq!(sites[0].activity, 0);
    }
}

#[test]
fn adjacent_disjoint_sites_cannot_suppress_one_another() {
    let a = area(0.1, 0);
    let mut b = a.clone();
    b.longitude = 0.3;
    let parts = geometry::rings(b.geometry.as_deref().unwrap()).unwrap();
    let points: Vec<_> = parts[0][0]
        .iter()
        .map(|p| format!("{} {}", p[0] + 0.3, p[1]))
        .collect();
    b.geometry = Some(
        Geometry::new_from_wkt(&format!("POLYGON (({}))", points.join(",")))
            .unwrap()
            .to_wkb()
            .unwrap(),
    );
    let mut second = observation(2410);
    second.longitude = 0.3;
    let mut sites = vec![a, b];
    classify(&mut sites, &[observation(3511), second]);
    assert!(sites.iter().all(|s| s.activity > 0 && s.suppressed == 0));
}

#[test]
fn a_retired_site_is_silent_but_an_operating_unit_keeps_its_shared_site_open() {
    let mut retired = observation(3511);
    retired.operating_state = 2;
    let mut sites = vec![area(0.01, 0)];
    classify(&mut sites, &[retired.clone()]);
    assert_eq!(sites[0].operating_state, 2);
    assert_eq!(sites[0].operation_identity, "register/site");
    let active = observation(3511);
    let mut sites = vec![area(0.01, 0)];
    classify(&mut sites, &[retired, active]);
    assert_eq!(sites[0].operating_state, 1);
}

#[test]
fn unlinked_registers_cannot_date_one_tenants_closure_from_another_tenants_snapshot() {
    let mut retired = observation(2410);
    retired.identity = "register/steel".into();
    retired.evidence_identity = "register/steel/retired".into();
    retired.operating_state = 2;
    let mut active = observation(3511);
    active.observed_on = "2000-01-01".into();
    for observations in [vec![retired.clone(), active.clone()], vec![active, retired]] {
        let mut sites = vec![area(0.01, 0)];
        classify(&mut sites, &observations);
        assert_eq!(sites[0].operating_state, 0);
        assert_eq!(sites[0].activity, 3511);
        assert_eq!(sites[0].operating_until, None);
    }
}

#[test]
fn overlapping_footprints_with_different_country_ownership_are_not_duplicates() {
    let mut a = area(0.1, 0);
    a.classification = 1;
    a.activity = 3511;
    a.country = 1;
    for (country, suppressed) in [(1, 1), (2, 0)] {
        let mut b = a.clone();
        b.country = country;
        let mut sites = vec![a.clone(), b];
        classify(&mut sites, &[]);
        assert_eq!(
            sites.iter().filter(|s| s.suppressed != 0).count(),
            suppressed
        );
    }
}
