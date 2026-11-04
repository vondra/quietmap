//! A tenant owned by the next square must block a registry stamp on the surrounding industrial zone.

use super::{Dataset, fingerprint, prepare, records};
use crate::{dev4::Square, year::YearInputs};
use arrow_array::{
    Array, ArrayRef, BinaryArray, Float32Array, Int32Array, Int64Array, RecordBatch, StringArray,
    UInt8Array, UInt16Array,
};
use arrow_ipc::writer::FileWriter;
use arrow_schema::{Field, Schema};
use serde_json::json;
use std::{collections::HashMap, path::Path, sync::Arc};

fn industrial(path: &Path, centroid: (f64, f64), ring: &[(f64, f64)], source_type: u8, area: f32) {
    let (x, y) = crate::dev4::degrees_to_z30(centroid.0, centroid.1);
    let mut geometry = (ring.len() as u32).to_le_bytes().to_vec();
    for &(lat, lon) in ring {
        let (x, y) = crate::dev4::degrees_to_z30(lat, lon);
        geometry.extend(x.to_le_bytes());
        geometry.extend(y.to_le_bytes());
    }
    let columns: Vec<(&str, ArrayRef)> = vec![
        (
            "osm_id",
            Arc::new(Int64Array::from(vec![i64::from(source_type) + 1])),
        ),
        ("centroid_gx", Arc::new(Int32Array::from(vec![x]))),
        ("centroid_gy", Arc::new(Int32Array::from(vec![y]))),
        ("source_type", Arc::new(UInt8Array::from(vec![source_type]))),
        ("site_subtype", Arc::new(UInt8Array::from(vec![0]))),
        ("source_id", Arc::new(UInt16Array::from(vec![0]))),
        ("country_iso", Arc::new(UInt16Array::from(vec![0]))),
        ("name", Arc::new(StringArray::from(vec![""]))),
        ("osm_kind", Arc::new(StringArray::from(vec!["way"]))),
        ("osm_tags", Arc::new(StringArray::from(vec!["{}"]))),
        (
            "rated_power_kw",
            Arc::new(Float32Array::from(vec![None::<f32>])),
        ),
        ("area_m2", Arc::new(Float32Array::from(vec![area]))),
        (
            "geom",
            Arc::new(BinaryArray::from(vec![geometry.as_slice()])),
        ),
    ];
    let fields = columns
        .iter()
        .map(|(name, array)| Field::new(*name, array.data_type().clone(), array.null_count() > 0))
        .collect::<Vec<_>>();
    let schema = Schema::new_with_metadata(
        fields,
        HashMap::from([
            ("grid".into(), "z30".into()),
            ("osm_industrial_contract".into(), "2".into()),
        ]),
    );
    let batch = RecordBatch::try_new(
        Arc::new(schema),
        columns.into_iter().map(|(_, array)| array).collect(),
    )
    .unwrap();
    let mut writer =
        FileWriter::try_new(std::fs::File::create(path).unwrap(), &batch.schema()).unwrap();
    writer.write(&batch).unwrap();
    writer.finish().unwrap();
}

#[test]
fn neighbouring_tenant_prevents_a_false_power_station_and_is_not_claimed_as_completed() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("2000");
    std::fs::create_dir_all(&root).unwrap();
    let target = Square { x: 255, y: 255 };
    for square in target.with_neighbours() {
        let directory = root.join(format!("geography/z9/{}/{}", square.x, square.y));
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("square-country-city.bin"), [0; 13]).unwrap();
    }
    industrial(
        &root.join("geography/z9/255/255/industrial.arrow"),
        (0.05, -0.05),
        &[
            (-0.1, -0.2),
            (-0.1, 0.1),
            (0.2, 0.1),
            (0.2, -0.2),
            (-0.1, -0.2),
        ],
        0,
        10_000_000.0,
    );
    industrial(
        &root.join("geography/z9/256/255/industrial.arrow"),
        (0.002, 0.001),
        &[
            (0.0, -0.001),
            (0.0, 0.003),
            (0.004, 0.003),
            (0.004, -0.001),
            (0.0, -0.001),
        ],
        12,
        5_000.0,
    );
    let entries:Vec<_>=[("osm","prepare-osm"),("geography","osm-geography"),("model","facility-model"),("observations","facility-observations"),("facilities","facilities")].into_iter().map(|(id,kind)|json!({"id":id,"kind":kind,"region":"world","publisher":"test","url":"https://example.org","observed":{"start":"2000","end":"2000"},"published":null,"licence":"CC0","public_ok":true,"depends_on":[],"description":"test"})).collect();
    let manifest = temporary.path().join("year.json");
    std::fs::write(&manifest,json!({"year":"2000","built":false,"as_of":"2001-01-01","flows":{"start":"2000-01-01","end":"2001-01-01"},"inputs":entries,"unavailable":[]}).to_string()).unwrap();
    std::fs::write(root.join("model"),json!({"inputs":[{"input":"register","format":"geojson","classifier":"constant","source_id":10,"authority_rank":4,"authority_edition":2000}],"names":[],"activity_order":{"3500":102.7},"coal_sources":[],"subtype_divisions":{},"source_divisions":{},"overlap_minimum_m2":100000,"overlap_area_ratio":2.5}).to_string()).unwrap();
    let inputs = YearInputs::read(&manifest, temporary.path()).unwrap();
    let observation = records::Facility {
        identity: "register/plant".into(),
        evidence_identity: "register/observation".into(),
        source: "register".into(),
        role: records::REGISTRY_POINT,
        longitude: 0.001,
        latitude: 0.002,
        activity: 3511,
        observed_on: "2000-06-01".into(),
        operating_state: 1,
        classification_admitted: 1,
        classification: 1,
        ..Default::default()
    };
    records::write(
        &root.join("observations"),
        &[observation],
        "2000",
        &fingerprint::observation_inputs(&inputs).unwrap(),
    )
    .unwrap();
    prepare::build(&inputs, &[target], &root.join("facilities")).unwrap();
    let rows = Dataset::open(&inputs)
        .unwrap()
        .rows(target, records::INDUSTRY)
        .unwrap();
    assert_eq!(rows[0].activity, 0);
    assert!(!root.join("facilities/z9/256/255/facilities.arrow").exists());
    let receipt: prepare::Receipt =
        serde_json::from_slice(&std::fs::read(root.join("facilities/complete.json")).unwrap())
            .unwrap();
    assert_eq!(receipt.squares, vec![[255, 255]]);
}
