//! Counts may cross a square boundary only along one compatible chain of original OSM nodes.

use super::{
    continuity, input,
    matching::Matcher,
    model::Model,
    observation::{CountBasis, Observation, TimeBasis},
    order_tests::write,
};
use crate::dev4::Square;
use arrow_array::{
    ArrayRef, BooleanArray, Float32Array, Float64Array, Int16Array, Int32Array, Int64Array,
    StringArray, UInt8Array, UInt16Array,
};
use std::{collections::BTreeSet, path::Path, sync::Arc};

pub(super) const CENTER: i32 = 1 << 29;

#[derive(Clone)]
pub(super) struct Piece {
    pub way: i64,
    pub nodes: [i64; 2],
    pub eastings: [i32; 2],
    pub class: u8,
}

pub(super) fn square(root: &Path, x: u32, rows: &[Piece]) {
    let directory = root.join("z9").join(x.to_string()).join("255");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("square-country-city.bin"), []).unwrap();
    let mut columns: Vec<(&str, ArrayRef)> = vec![
        (
            "osm_id",
            Arc::new(Int64Array::from(
                rows.iter().map(|r| r.way).collect::<Vec<_>>(),
            )),
        ),
        (
            "segment_idx",
            Arc::new(Int16Array::from(vec![0; rows.len()])),
        ),
        (
            "length_m",
            Arc::new(Float32Array::from(vec![373.0; rows.len()])),
        ),
        (
            "country_iso",
            Arc::new(UInt16Array::from(vec![0; rows.len()])),
        ),
        (
            "name",
            Arc::new(StringArray::from(vec!["Main Street"; rows.len()])),
        ),
        ("ref", Arc::new(StringArray::from(vec!["A1"; rows.len()]))),
        (
            "tunnel",
            Arc::new(BooleanArray::from(vec![false; rows.len()])),
        ),
        (
            "road_class",
            Arc::new(UInt8Array::from(
                rows.iter().map(|r| r.class).collect::<Vec<_>>(),
            )),
        ),
    ];
    for (name, value) in [
        ("speed_limit", 0),
        ("oneway", 0),
        ("lanes", 2),
        ("access", 0),
        ("junction", 0),
        ("built_up", 1),
    ] {
        columns.push((name, Arc::new(UInt8Array::from(vec![value; rows.len()]))));
    }
    for (side, x_name, y_name) in [(0, "start_gx", "start_gy"), (1, "end_gx", "end_gy")] {
        columns.push((
            x_name,
            Arc::new(Int32Array::from(
                rows.iter().map(|r| r.eastings[side]).collect::<Vec<_>>(),
            )),
        ));
        columns.push((
            y_name,
            Arc::new(Int32Array::from(vec![CENTER + 100; rows.len()])),
        ));
    }
    write(
        &directory.join("roads.arrow"),
        columns.clone(),
        ("roads_contract", "country_baked_v1"),
    );
    columns.retain(|(name, _)| {
        [
            "osm_id",
            "segment_idx",
            "start_gx",
            "start_gy",
            "end_gx",
            "end_gy",
        ]
        .contains(name)
    });
    columns[0].0 = "way_id";
    for (side, node, vertex, fraction) in [
        (0, "start_node", "start_vertex", "start_fraction"),
        (1, "end_node", "end_vertex", "end_fraction"),
    ] {
        columns.push((
            node,
            Arc::new(Int64Array::from(
                rows.iter().map(|r| r.nodes[side]).collect::<Vec<_>>(),
            )),
        ));
        columns.push((
            vertex,
            Arc::new(UInt16Array::from(vec![side as u16; rows.len()])),
        ));
        columns.push((
            fraction,
            Arc::new(Float64Array::from(vec![0.0; rows.len()])),
        ));
    }
    write(
        &directory.join("roads.pieces.arrow"),
        columns,
        ("transport_pieces_contract", "1"),
    );
}

pub(super) fn count(way: &Piece, identity: &str) -> Observation {
    let lon =
        ((f64::from(way.eastings[0]) + f64::from(way.eastings[1])) / 2.0 / f64::from(1 << 30)
            - 0.5)
            * 360.0;
    let lat = ((f64::from(CENTER + 100) / f64::from(1 << 30) - 0.5) * std::f64::consts::TAU)
        .sinh()
        .atan()
        .to_degrees();
    Observation {
        identity: identity.into(),
        input_id: "survey".into(),
        source_id: 20,
        source_rank: 10,
        measured: true,
        country: 0,
        observed_start: "2000-01-01".into(),
        observed_end: "2000-12-31".into(),
        count_basis: CountBasis::BothDirections,
        time_basis: TimeBasis::AnnualDaily,
        direction: 0,
        reference: "A1".into(),
        geometry: vec![vec![[lon, lat]]],
        road_class_mask: 1 << 2,
        search_radius_m: 1.0,
        reference_required: true,
        vehicle_groups: Vec::new(),
        vehicle_classes: [Some(80.0), Some(10.0), Some(5.0), Some(5.0)],
        total: Some(100.0),
        estimated_classes: 0,
        daily_factor: 1.0,
        target: Default::default(),
        sample: Default::default(),
    }
}

#[test]
fn incomplete_chains_require_context_only_where_an_admitted_count_can_apply() {
    let piece = Piece {
        way: 1,
        nodes: [1, 2],
        eastings: [CENTER - 10_000, CENTER],
        class: 2,
    };
    let temporary = tempfile::tempdir().unwrap();
    square(temporary.path(), 255, std::slice::from_ref(&piece));
    let selected = BTreeSet::from([Square { x: 255, y: 255 }]);
    let model: Model =
        serde_json::from_str(include_str!("../../test-data/road-preparation.json")).unwrap();
    let mut observation = count(&piece, "distant station");
    observation.geometry[0][0][0] += 1.0;
    let matcher = Matcher::new(vec![observation.clone()]).unwrap();
    assert!(continuity::build(temporary.path(), &selected, &matcher, &model).is_err());
    observation.country = u16::from_le_bytes(*b"CZ");
    let matcher = Matcher::new(vec![observation]).unwrap();
    assert!(continuity::build(temporary.path(), &selected, &matcher, &model).is_ok());
}

#[test]
fn original_nodes_connect_across_squares_but_distinct_nodes_conflicts_and_junctions_do_not() {
    let first = Piece {
        way: 1,
        nodes: [1, 2],
        eastings: [CENTER - 10_000, CENTER],
        class: 2,
    };
    let second = Piece {
        way: 2,
        nodes: [2, 3],
        eastings: [CENTER, CENTER + 10_000],
        class: 2,
    };
    let third = Piece {
        way: 3,
        nodes: [3, 4],
        eastings: [CENTER + 10_000, CENTER + 20_000],
        class: 2,
    };
    let model: Model =
        serde_json::from_str(include_str!("../../test-data/road-preparation.json")).unwrap();
    let selected = BTreeSet::from([Square { x: 255, y: 255 }, Square { x: 256, y: 255 }]);
    let run = |right: Vec<Piece>, observations: Vec<Observation>| {
        let temporary = tempfile::tempdir().unwrap();
        square(temporary.path(), 255, std::slice::from_ref(&first));
        square(temporary.path(), 256, &right);
        let matcher = Matcher::new(observations).unwrap();
        let plan = continuity::build(temporary.path(), &selected, &matcher, &model).unwrap();
        let mut roads = super::matched_roads(
            &input::load(&temporary.path().join("z9/256/255/roads.arrow")).unwrap(),
            &matcher,
            &model,
        )
        .unwrap();
        plan.apply(Square { x: 256, y: 255 }, &mut roads);
        for filled in roads.iter().filter(|road| road.source_id == 12) {
            assert!(filled.count_origin_measured);
            let mut restricted_street = filled.clone();
            restricted_street.direction = 1;
            restricted_street.class = 3;
            restricted_street.access = 2;
            assert_eq!(
                super::allocation::allocate(&restricted_street, &[], &model).0,
                filled.counts
            );
        }
        roads
            .into_iter()
            .map(|r| (r.way_id, r.source_id, r.counts, r.estimated))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        run(
            vec![second.clone(), third.clone()],
            vec![count(&first, "station")]
        ),
        vec![
            (2, 12, [80.0, 10.0, 5.0, 5.0], 15),
            (3, 12, [80.0, 10.0, 5.0, 5.0], 15)
        ]
    );
    let mut disconnected = second.clone();
    let mut modeled = count(&first, "modeled class prior");
    modeled.measured = false;
    assert!(
        run(vec![second.clone(), third.clone()], vec![modeled])
            .iter()
            .all(|r| r.1 == 0)
    );
    disconnected.nodes[0] = 99;
    assert!(
        run(
            vec![disconnected, third.clone()],
            vec![count(&first, "station")]
        )
        .iter()
        .all(|r| r.1 == 0)
    );
    let conflicting = run(
        vec![second.clone(), third.clone()],
        vec![count(&first, "station"), count(&third, "another station")],
    );
    assert_eq!(conflicting[0].1, 0);
    assert_eq!(conflicting[1].1, 20);
    let branch = Piece {
        way: 4,
        nodes: [2, 5],
        eastings: [CENTER, CENTER + 5_000],
        class: 7,
    };
    assert!(
        run(vec![second, third, branch], vec![count(&first, "station")])
            .iter()
            .all(|r| r.1 == 0)
    );
}
