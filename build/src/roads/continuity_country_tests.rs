//! One physical chain conserves its observed flow through borders and gaps in country polygons.

use super::{
    continuity,
    continuity_tests::{CENTER, Piece, count, square},
    input,
    matching::Matcher,
    order_tests::write,
    tests::model,
};
use arrow_array::{ArrayRef, UInt16Array};
use std::{collections::BTreeSet, sync::Arc};

#[test]
fn an_observation_crosses_an_uninterrupted_border_and_coastal_gap() {
    let directory = tempfile::tempdir().unwrap();
    let pieces = (0..3)
        .map(|i| Piece {
            way: i + 1,
            nodes: [i + 1, i + 2],
            eastings: [
                CENTER + (i as i32) * 10_000,
                CENTER + (i as i32 + 1) * 10_000,
            ],
            class: 2,
        })
        .collect::<Vec<_>>();
    square(directory.path(), 256, &pieces);
    let path = directory.path().join("z9/256/255/roads.arrow");
    let batch = input::load(&path).unwrap().remove(0);
    let schema = batch.schema();
    let countries: ArrayRef = Arc::new(UInt16Array::from(vec![
        u16::from_le_bytes(*b"CZ"),
        0,
        u16::from_le_bytes(*b"DE"),
    ]));
    let columns = schema
        .fields()
        .iter()
        .zip(batch.columns())
        .map(|(field, values)| {
            (
                field.name().as_str(),
                if field.name() == "country_iso" {
                    countries.clone()
                } else {
                    values.clone()
                },
            )
        })
        .collect();
    write(&path, columns, ("roads_contract", "country_baked_v1"));
    let mut observation = count(&pieces[0], "one physical count");
    observation.country = u16::from_le_bytes(*b"CZ");
    observation.estimated_classes = 9;
    let matcher = Matcher::new(vec![observation]).unwrap();
    let selected = crate::dev4::Square { x: 256, y: 255 };
    let plan = continuity::build(
        directory.path(),
        &BTreeSet::from([selected]),
        &matcher,
        &model(),
    )
    .unwrap();
    let mut roads = super::matched_roads(&input::load(&path).unwrap(), &matcher, &model()).unwrap();
    plan.apply(selected, &mut roads);
    assert_eq!(
        roads.iter().map(|r| r.source_id).collect::<Vec<_>>(),
        [20, 12, 12]
    );
    assert!(roads.iter().all(|r| r.counts == [80.0, 10.0, 5.0, 5.0]));
    assert!(roads.iter().all(|r| r.observation_estimated == 9));
    assert_eq!(roads[1].estimated, 15);
}
