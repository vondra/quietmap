//! Source rows preserve exact fields and text through spill and Arrow.

use super::*;

#[test]
fn roads_writer_roundtrips_grid_columns() {
    let dir = scratch_dir("roads");
    let path = dir.join("roads.arrow");
    // TSV: sq osm seg s_gx s_gy e_gx e_gy len class speed surface oneway_dir
    // lanes name ref bridge tunnel toll lit junction access
    let rows = vec![
        "100\t11\t0\t1000\t2000\t3000\t4000\t12.5\t5\t50\t0\t2\t2\tMain\t\t0\t0\t0\t0\t0\t0\t{}\t[1,2,1000,2000,3000,4000]"
            .split('\t')
            .map(str::to_string)
            .collect::<Vec<_>>(),
    ];
    write_roads(&rows, &path).unwrap();
    let (schema, batches) = read_ipc(&path);
    assert_eq!(batches.iter().map(|b| b.num_rows()).sum::<usize>(), 1);
    let batch = &batches[0];
    let f = |n: &str| batch.column(schema.index_of(n).unwrap());
    assert_eq!(
        f("start_gx")
            .as_any()
            .downcast_ref::<Int32Array>()
            .unwrap()
            .value(0),
        1000
    );
    assert_eq!(
        f("end_gy")
            .as_any()
            .downcast_ref::<Int32Array>()
            .unwrap()
            .value(0),
        4000
    );
    // Direction code survives as u8 (2 = oneway reverse), not a flattened bool.
    assert_eq!(
        f("oneway")
            .as_any()
            .downcast_ref::<UInt8Array>()
            .unwrap()
            .value(0),
        2
    );
    assert_eq!(
        f("name")
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap()
            .value(0),
        "Main"
    );
    assert_eq!(
        schema.metadata().get(GRID_CONTRACT_KEY).map(String::as_str),
        Some(GRID_CONTRACT_Z30)
    );
    assert!(!schema.metadata().contains_key("qm_blocks"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn multiline_osm_tags_survive_spill_and_arrow_for_every_source() {
    use crate::osm::classify::{FeatureType, Tags};
    use crate::osm::spill::Spiller;
    let dir = scratch_dir("all-source-text");
    let spill_dir = dir.join("spill");
    let output_dir = dir.join("prepared");
    let square = crate::osm::geometry::square_of(50.0, 14.0);
    let raw_name = "Route du \r\nVillage de la Cottencière\tŽluťoučký";
    let mut tags = Tags::new();
    for key in [
        "name",
        "ref",
        "local_ref",
        "addr:street",
        "addr:housenumber",
        "icao",
        "iata",
        "operator",
        "surface",
        "aerodrome:type",
        "access",
    ] {
        tags.insert(key.into(), raw_name.into());
    }
    tags.insert("building".into(), "yes".into());
    tags.insert("highway".into(), "residential".into());
    tags.insert("railway".into(), "rail".into());
    tags.insert("aeroway".into(), "aerodrome".into());
    let mut spiller = Spiller::new(&spill_dir, 1, Default::default(), &[]).unwrap();
    let sources = [
        FeatureType::Road,
        FeatureType::Railway,
        FeatureType::Barrier,
        FeatureType::AirportLine,
        FeatureType::Building,
        FeatureType::Industrial,
        FeatureType::Leisure,
        FeatureType::AirportArea,
    ];
    for (index, source) in sources.iter().enumerate() {
        if source.is_linear() {
            spiller
                .emit_segment(
                    source,
                    square,
                    index as i64 + 1,
                    0,
                    &([50.0, 14.0], [50.0001, 14.0], 11.0),
                    &tags,
                    match source {
                        FeatureType::Road => Some("[1,2,0,0,1,1]\t0,0,1,0,1,2"),
                        FeatureType::Railway => {
                            Some("[1,2,0,0,1,1]\t0,0,1,0,1,2,0,0,11.1,500000000;140000000;500001000;140000000")
                        }
                        _ => None,
                    },
                )
                .unwrap();
        } else {
            spiller
                .emit_polygon(
                    source,
                    square,
                    index as i64 + 1,
                    "way",
                    50.0,
                    14.0,
                    &tags,
                    Some(&[[50.0, 14.0], [50.0, 14.001], [50.001, 14.001], [50.0, 14.0]]),
                )
                .unwrap();
        }
    }
    crate::osm::transport::TransportSpill::new(&spill_dir)
        .unwrap()
        .finish()
        .unwrap();
    spiller.finish().unwrap();
    drop(spiller);
    assert_eq!(finalize(&spill_dir, &output_dir, 1, &[]).unwrap(), 1);
    for source in sources {
        let path = output_dir
            .join(crate::osm::geometry::square_name(square))
            .join(format!("{}.arrow", source.name()));
        let (_, batches) = read_ipc(&path);
        assert_eq!(
            batches.iter().map(|batch| batch.num_rows()).sum::<usize>(),
            1,
            "{}",
            source.name()
        );
        for batch in batches {
            for column in batch.columns() {
                if let Some(strings) = column.as_any().downcast_ref::<StringArray>() {
                    for value in strings.iter().flatten() {
                        assert!(!value.contains(['\t', '\r', '\n']), "{value:?}");
                    }
                }
            }
            if let Some(name) = batch.column_by_name("name") {
                let names = name.as_any().downcast_ref::<StringArray>().unwrap();
                assert_eq!(names.value(0), raw_name.replace(['\t', '\r', '\n'], " "));
            }
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn airport_writer_drops_sub_decimetre_legs_and_preserves_identity() {
    let dir = scratch_dir("airport-degenerate");
    let path = dir.join("airport_lines.arrow");
    let row = |id: u64, index: u16, end: i32, length: &str| {
        format!("100\t{id}\t{index}\t1000\t2000\t{end}\t2000\t{length}\t90\t1\t\t\t")
            .split('\t')
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    write_airport_lines(
        &[
            row(1056661322, 0, 1000, "0"),
            row(866803531, 22, 1000, "0"),
            // One z30 unit long: under the spill's 0.1 m resolution, dropped.
            row(23733611, 3, 1001, "0"),
            // A decimetre inside one grid point (a diagonal near the equator): dropped.
            row(23733611, 4, 1000, "0.1"),
            row(866803531, 23, 1100, "3.7"),
        ],
        &path,
    )
    .unwrap();
    let (_, batches) = read_ipc(&path);
    assert_eq!(batches.iter().map(|b| b.num_rows()).sum::<usize>(), 1);
    let batch = &batches[0];
    assert_eq!(
        batch
            .column_by_name("segment_idx")
            .unwrap()
            .as_any()
            .downcast_ref::<arrow_array::Int16Array>()
            .unwrap()
            .value(0),
        23
    );
    assert_eq!(
        batch
            .column_by_name("osm_id")
            .unwrap()
            .as_any()
            .downcast_ref::<arrow_array::Int64Array>()
            .unwrap()
            .value(0),
        866803531
    );
    for (end, length) in [(1000, "1"), (1100, "NaN"), (1100, "-1")] {
        let rejected = dir.join(format!("rejected-{end}-{length}.arrow"));
        assert!(write_airport_lines(&[row(1, 0, end, length)], &rejected).is_err());
        assert!(!rejected.exists());
    }
    std::fs::remove_dir_all(dir).unwrap();
}
