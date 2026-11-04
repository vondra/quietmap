//! Reordering mapped rows must not move a building's trips between equally near streets.

use super::{input, model::Model};
use arrow_array::{
    Array, ArrayRef, BooleanArray, Float32Array, Int16Array, Int32Array, Int64Array, RecordBatch,
    StringArray, UInt8Array, UInt16Array, UInt32Array,
};
use arrow_schema::{Field, Schema};
use std::{collections::HashMap, path::Path, sync::Arc};

pub(super) fn write(path: &Path, columns: Vec<(&str, ArrayRef)>, contract: (&str, &str)) {
    let fields = columns
        .iter()
        .map(|(name, array)| Field::new(*name, array.data_type().clone(), array.null_count() != 0))
        .collect::<Vec<_>>();
    let schema = Arc::new(Schema::new_with_metadata(
        fields,
        HashMap::from([
            ("grid".into(), "z30".into()),
            (contract.0.into(), contract.1.into()),
        ]),
    ));
    let batch = RecordBatch::try_new(
        schema.clone(),
        columns.into_iter().map(|(_, a)| a).collect(),
    )
    .unwrap();
    let mut writer =
        arrow_ipc::writer::FileWriter::try_new(std::fs::File::create(path).unwrap(), &schema)
            .unwrap();
    writer.write(&batch).unwrap();
    writer.finish().unwrap();
}

fn mapped_streets(path: &Path, reverse: bool, names: [&str; 2]) {
    let center = 1 << 29;
    let mut columns: Vec<(&str, ArrayRef)> = vec![
        ("osm_id", Arc::new(Int64Array::from(vec![1, 2]))),
        ("segment_idx", Arc::new(Int16Array::from(vec![0, 0]))),
        ("start_gx", Arc::new(Int32Array::from(vec![center, center]))),
        ("start_gy", Arc::new(Int32Array::from(vec![center, center]))),
        (
            "end_gx",
            Arc::new(Int32Array::from(vec![center + 100, center])),
        ),
        (
            "end_gy",
            Arc::new(Int32Array::from(vec![center, center + 100])),
        ),
        ("length_m", Arc::new(Float32Array::from(vec![3.7, 3.7]))),
        ("country_iso", Arc::new(UInt16Array::from(vec![0, 0]))),
        ("name", Arc::new(StringArray::from(names.to_vec()))),
        ("ref", Arc::new(StringArray::from(vec!["", ""]))),
        ("tunnel", Arc::new(BooleanArray::from(vec![false, false]))),
    ];
    for (name, value) in [
        ("speed_limit", 0),
        ("oneway", 0),
        ("road_class", 7),
        ("lanes", 1),
        ("access", 0),
        ("junction", 0),
        ("built_up", 1),
    ] {
        columns.push((name, Arc::new(UInt8Array::from(vec![value, value]))));
    }
    if reverse {
        let indices = UInt32Array::from(vec![1, 0]);
        for (_, array) in &mut columns {
            *array = arrow_select::take::take(array.as_ref(), &indices, None).unwrap();
        }
    }
    write(path, columns, ("roads_contract", "country_baked_v1"));
}

fn write_building(structures: &Path) {
    let center = 1 << 29;
    write(
        structures,
        vec![
            ("kind", Arc::new(UInt8Array::from(vec![0]))),
            ("osm_id", Arc::new(Int64Array::from(vec![99]))),
            ("building_type", Arc::new(UInt8Array::from(vec![12]))),
            ("storeys", Arc::new(UInt8Array::from(vec![1]))),
            ("area_m2", Arc::new(Float32Array::from(vec![1000.0]))),
            ("centroid_gx", Arc::new(Int32Array::from(vec![center]))),
            ("centroid_gy", Arc::new(Int32Array::from(vec![center]))),
            (
                "emission_centroid_gx",
                Arc::new(Int32Array::from(vec![None])),
            ),
            (
                "emission_centroid_gy",
                Arc::new(Int32Array::from(vec![None])),
            ),
        ],
        ("structures_contract", "structures_v5"),
    );
}

#[test]
fn a_building_at_the_shared_endpoint_keeps_its_street_after_arrow_reordering() {
    let temporary = tempfile::tempdir().unwrap();
    let structures = temporary.path().join("structures.arrow");
    write_building(&structures);
    let model: Model =
        serde_json::from_str(include_str!("../../test-data/road-preparation.json")).unwrap();
    let run = |reverse| {
        let path = temporary.path().join(if reverse {
            "shuffled.arrow"
        } else {
            "original.arrow"
        });
        mapped_streets(&path, reverse, ["", ""]);
        let batches = input::load(&path).unwrap();
        let mut roads = batches
            .iter()
            .flat_map(|b| input::roads(b).unwrap())
            .collect::<Vec<_>>();
        crate::traffic::preparation::apply(&mut roads, &structures, &model.demand).unwrap();
        roads
            .into_iter()
            .map(|road| (road.way_id, road.counts.iter().sum::<f64>()))
            .collect::<Vec<_>>()
    };
    assert_eq!(run(false), vec![(1, 300.0), (2, 20.0)]);
    assert_eq!(run(true), run(false));
}

#[test]
fn trailing_name_whitespace_does_not_split_one_streets_building_demand() {
    let temporary = tempfile::tempdir().unwrap();
    let structures = temporary.path().join("structures.arrow");
    write_building(&structures);
    let path = temporary.path().join("roads.arrow");
    mapped_streets(&path, false, ["Am Wirtsberg", "Am Wirtsberg "]);
    let batches = input::load(&path).unwrap();
    let mut roads = input::roads(&batches[0]).unwrap();
    for road in &mut roads {
        road.class = 5;
    }
    let model = super::tests::model();
    crate::traffic::preparation::apply(&mut roads, &structures, &model.demand).unwrap();
    assert!(roads[0].counts.iter().sum::<f64>() > 0.0);
    assert_eq!(roads[0].counts, roads[1].counts);
}

#[test]
fn equal_distance_speed_anchors_follow_the_original_piece_order() {
    let mut roads = [3, 1, 2].map(super::tests::road);
    for (index, road) in roads.iter_mut().enumerate() {
        road.ends = [(index as i32, 0), (index as i32 + 1, 0)];
        road.country.0 = u16::from_le_bytes(*b"CZ");
    }
    roads[0].speed_limit = 130;
    roads[2].speed_limit = 40;
    super::speed_transitions::apply(&mut roads, &super::tests::model().speed_transitions);
    assert_eq!(roads[1].speed_taper, 122);
}

#[test]
fn an_absent_street_fit_keeps_the_global_prior_instead_of_inventing_zero_through_traffic() {
    let temporary = tempfile::tempdir().unwrap();
    let structures = temporary.path().join("structures.arrow");
    write_building(&structures);
    let path = temporary.path().join("roads.arrow");
    mapped_streets(&path, false, ["a", "b"]);
    let batch = input::load(&path).unwrap().remove(0);
    let mut roads = input::roads(&batch).unwrap();
    for road in &mut roads {
        road.class = 5;
    }
    let mut model = super::tests::model();
    model.demand.street = None;
    crate::traffic::preparation::apply(&mut roads, &structures, &model.demand).unwrap();
    assert!(roads.iter().all(|road| road.source_id == 0));
    assert!(roads.iter().all(|road| {
        super::allocation::allocate(road, &[], &model)
            .0
            .iter()
            .sum::<f64>()
            > 0.0
    }));
}

#[test]
fn service_roads_keep_their_prior_when_no_demand_bounds_are_admitted() {
    let temporary = tempfile::tempdir().unwrap();
    let structures = temporary.path().join("structures.arrow");
    write_building(&structures);
    let path = temporary.path().join("roads.arrow");
    mapped_streets(&path, false, ["", ""]);
    let batch = input::load(&path).unwrap().remove(0);
    let mut roads = input::roads(&batch).unwrap();
    let mut model = super::tests::model();
    model.demand.street = None;
    model.demand.service_trip_bounds = None;
    crate::traffic::preparation::apply(&mut roads, &structures, &model.demand).unwrap();
    assert!(roads.iter().all(|road| road.source_id == 0));
    assert!(roads.iter().all(|road| {
        super::allocation::allocate(road, &[], &model)
            .0
            .iter()
            .sum::<f64>()
            > 0.0
    }));
}
