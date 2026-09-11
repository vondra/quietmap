//! End-to-end source spill and Arrow contract regressions.

use super::*;
use arrow::array::{BinaryArray, Int32Array, StringArray};
use arrow::ipc::reader::FileReader;

fn prague_ring_text() -> String {
    [
        (14.0, 50.0),
        (14.001_394, 50.0),
        (14.001_394, 50.000_904),
        (14.0, 50.000_904),
    ]
    .iter()
    .map(|&(lon, lat)| {
        let (gx, gy) = grid::lonlat_to_grid(lon, lat);
        format!("{gx},{gy}")
    })
    .collect::<Vec<_>>()
    .join(";")
}

fn scratch_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("osm-extract-test-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn read_ipc(path: &Path) -> (Schema, Vec<arrow::record_batch::RecordBatch>) {
    let file = File::open(path).unwrap();
    let reader = FileReader::try_new(file, None).unwrap();
    let schema = reader.schema().as_ref().clone();
    let batches: Vec<_> = reader.collect::<Result<_, _>>().unwrap();
    (schema, batches)
}

#[test]
fn roads_writer_roundtrips_grid_columns() {
    let dir = scratch_dir("roads");
    let path = dir.join("roads.arrow");
    // TSV: sq osm seg s_gx s_gy e_gx e_gy len class speed surface oneway
    // lanes name ref bridge tunnel toll lit junction access
    let rows = vec![
        "100\t11\t0\t1000\t2000\t3000\t4000\t12.5\t5\t50\t0\t0\t2\tMain\t\t0\t0\t0\t0\t0\t0"
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
    assert!(schema.metadata().contains_key(arrow_batching::QM_BLOCKS_KEY));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn only_emittable_buildings_suppress_functional_areas() {
    let dir = scratch_dir("building-eligibility");
    let square = grid::square_of(50.0005, 14.0005);
    let key = crate::spill::spill_key(square);
    let (gx, gy) = grid::lonlat_to_grid(14.0005, 50.0005);
    let ring = prague_ring_text();
    let real = format!("{key}\t1\t{gx}\t{gy}\t0\t0\t0\t0\tReal\t\t\t0\t0");
    let area = format!("{key}\t2\t{gx}\t{gy}\t0\t0\t0\t0\tArea\t\t\t0\t1\t{ring}");
    let food_retail = crate::ids::SETTLEMENT_FOOD_RETAIL;
    for (geometry_column, expected) in [("", (2_i64, food_retail)), ("\t", (1, 0))] {
        let spill = dir.join(format!("spill-{}", geometry_column.len()));
        let output = dir.join(format!("prepared-{}", geometry_column.len()));
        std::fs::create_dir_all(&spill).unwrap();
        std::fs::write(
            spill.join("buildings_000.tsv"),
            format!("{real}{geometry_column}\n{area}\n"),
        )
        .unwrap();
        std::fs::write(
            spill.join("poi_000.tsv"),
            format!("{key}\t{gx}\t{gy}\t{food_retail}\n"),
        )
        .unwrap();
        assert_eq!(finalize(&spill, &output, 1).unwrap(), 1);
        let (_, batches) = read_ipc(
            &output
                .join(grid::square_name(square))
                .join("buildings.arrow"),
        );
        let mut emitted = Vec::new();
        for batch in batches {
            let ids = batch
                .column_by_name("osm_id")
                .unwrap()
                .as_any()
                .downcast_ref::<arrow::array::Int64Array>()
                .unwrap();
            let classes = batch
                .column_by_name("building_type")
                .unwrap()
                .as_any()
                .downcast_ref::<arrow::array::UInt8Array>()
                .unwrap();
            emitted.extend((0..batch.num_rows()).map(|i| (ids.value(i), classes.value(i))));
        }
        assert_eq!(
            emitted,
            vec![expected],
            "geometry column {geometry_column:?}"
        );
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn buildings_writer_roundtrips_geom_and_contract() {
    let dir = scratch_dir("buildings");
    let path = dir.join("buildings.arrow");
    let ring = prague_ring_text();
    // TSV: sq osm cgx cgy btype buse height floors name street houseno
    // opening area_source ring
    let row = format!("100\t22\t1500\t2500\t0\t0\t0\t0\tH\tS\t1\t0\t0\t{ring}");
    let rows = vec![row.split('\t').map(str::to_string).collect::<Vec<_>>()];
    let stats = JoinStats::default();
    write_buildings(
        &rows,
        &path,
        crate::spill::square_from_spill_key(100).unwrap(),
        &PoiIndex::default(),
        &stats,
    )
    .unwrap();
    let (schema, batches) = read_ipc(&path);
    assert_eq!(batches.iter().map(|b| b.num_rows()).sum::<usize>(), 1);
    let batch = &batches[0];
    let geom = batch
        .column(schema.index_of("geom").unwrap())
        .as_any()
        .downcast_ref::<BinaryArray>()
        .unwrap();
    let ring = grid::poly::decode_grid_poly(geom.value(0)).unwrap();
    assert_eq!(ring.len(), 4);
    // Area of the ~100×100 m test ring survives the roundtrip.
    let area = batch
        .column(schema.index_of("area_m2").unwrap())
        .as_any()
        .downcast_ref::<arrow::array::Float32Array>()
        .unwrap()
        .value(0);
    assert!((9_000.0..11_000.0).contains(&area), "area={area}");
    assert_eq!(
        schema
            .metadata()
            .get("buildings_contract")
            .map(String::as_str),
        Some(BUILDINGS_CONTRACT_V3)
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn multiline_osm_tags_survive_spill_and_arrow_for_every_source() {
    use crate::classify::{FeatureType, Tags};
    use crate::spill::Spiller;
    let dir = scratch_dir("all-source-text");
    let spill_dir = dir.join("spill");
    let output_dir = dir.join("prepared");
    let square = grid::square_of(50.0, 14.0);
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
    let mut spiller = Spiller::new(&spill_dir, 1).unwrap();
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
            spiller.emit_segment(
                source,
                square,
                index as i64 + 1,
                0,
                &([50.0, 14.0], [50.0001, 14.0], 11.0),
                &tags,
            );
        } else {
            spiller.emit_polygon(source, square, index as i64 + 1, 50.0, 14.0, &tags, None);
        }
    }
    spiller.complete().unwrap();
    drop(spiller);
    assert_eq!(finalize(&spill_dir, &output_dir, 1).unwrap(), 1);
    for source in sources {
        let path = output_dir
            .join(grid::square_name(square))
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
            .downcast_ref::<arrow::array::Int16Array>()
            .unwrap()
            .value(0),
        23
    );
    assert_eq!(
        batch
            .column_by_name("osm_id")
            .unwrap()
            .as_any()
            .downcast_ref::<arrow::array::Int64Array>()
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
