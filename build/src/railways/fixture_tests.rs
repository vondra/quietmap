//! Replay publisher movements through matching and finalization for exact frozen-output comparison.
use std::path::PathBuf;

#[test]
fn unknown_is_distinct_from_an_observed_zero() {
    let row = super::movements::Movement {
        missing_calendar_estimate: [0.0; 2],
        attributes: Default::default(),
        id: "train".into(),
        source_id: 1,
        country: "CZ".into(),
        passenger: Some(0.0),
        freight: None,
        passenger_status: 2,
        freight_status: 0,
        matching: 2,
        rail_types: vec![0],
        coverage: Default::default(),
        location: super::movements::Location::Interval {
            square: [0, 0],
            way: 1,
            segment: 0,
            from_m: 0.0,
            to_m: 10.0,
            occurrence: 0,
        },
    };
    row.validate().unwrap();
    let mut invalid = row.clone();
    invalid.freight = Some(0.0);
    assert!(invalid.validate().is_err());
    let mut encoded = Vec::new();
    super::movements::write_record(
        &mut encoded,
        &super::movements::Record::Movement(row.into()),
    )
    .unwrap();
    assert!(
        String::from_utf8(encoded)
            .unwrap()
            .contains("\"freight\":null")
    );
}

#[test]
#[ignore = "requires explicitly selected movement, geography and frozen comparison inputs"]
fn matcher_fixture_replay() {
    let plan: serde_json::Value = serde_json::from_slice(
        &std::fs::read(std::env::var("QM_RAIL_MATCHER_FIXTURE").unwrap()).unwrap(),
    )
    .unwrap();
    let path = |key: &str| PathBuf::from(plan[key].as_str().unwrap());
    let squares = crate::parse_squares(plan["squares"].as_str().unwrap()).unwrap();
    let selected = squares
        .iter()
        .map(|square| [square.x as u16, square.y as u16])
        .collect();
    let model = super::model::RailModel::read(&path("model")).unwrap();
    let geography = path("geography");
    let output = path("output");
    assert!(!output.exists());
    let mut matched = super::matcher::read(
        &model,
        &geography,
        &path("flows"),
        plan["start"].as_str().unwrap(),
        plan["end"].as_str().unwrap(),
        &selected,
    )
    .unwrap();
    for square in squares {
        let key = [square.x as u16, square.y as u16];
        let relative = format!("z9/{}/{}", square.x, square.y);
        let rows = matched.claims.remove(&key).unwrap_or_default();
        let intervals = super::square_intervals::grouped(rows);
        super::write::finalize_square(
            &model,
            &geography.join(&relative),
            &matched.crossings.remove(&key).unwrap_or_default(),
            &intervals,
            &output.join(&relative),
            crate::osm::geometry::Square {
                x: key[0],
                y: key[1],
            },
        )
        .unwrap();
    }
}
