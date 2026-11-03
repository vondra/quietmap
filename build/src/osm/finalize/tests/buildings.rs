//! Building geometry, source identity and overlap regression cases.

use super::*;

#[test]
fn only_emittable_buildings_suppress_functional_areas() {
    let dir = scratch_dir("building-eligibility");
    let square = crate::osm::geometry::square_of(50.0005, 14.0005);
    let key = crate::osm::spill::spill_key(square);
    let (gx, gy) = crate::osm::geometry::lonlat_to_grid(14.0005, 50.0005);
    let ring = prague_ring_text();
    let real = format!("{key}\t1\t{gx}\t{gy}\t0\t0\t0\t0\tReal\t\t\t0\t0");
    let area = format!("{key}\t2\t{gx}\t{gy}\t0\t0\t0\t0\tArea\t\t\t0\t1\t{ring}");
    // A car park BELOW the real building wraps nothing: it keeps emitting.
    let below = format!(
        "{key}\t3\t{gx}\t{gy}\t{}\t0\t0\t0\tBelow\t\t\t0\t2\t{ring}",
        crate::osm::ids::SETTLEMENT_PARKING_STRUCTURE
    );
    let garage = crate::osm::ids::SETTLEMENT_PARKING_STRUCTURE;
    let food_retail = crate::osm::ids::SETTLEMENT_FOOD_RETAIL;
    for (geometry_column, expected) in [
        ("", vec![(2_i64, food_retail), (3, garage)]),
        ("\t", vec![(1, 0), (3, garage)]),
    ] {
        let spill = dir.join(format!("spill-{}", geometry_column.len()));
        let output = dir.join(format!("prepared-{}", geometry_column.len()));
        std::fs::create_dir_all(&spill).unwrap();
        std::fs::write(
            spill.join("buildings_000.tsv"),
            format!("{real}{geometry_column}\n{area}\n{below}\n"),
        )
        .unwrap();
        std::fs::write(
            spill.join("poi_000.tsv"),
            format!("{key}\t{gx}\t{gy}\t{food_retail}\n"),
        )
        .unwrap();
        crate::osm::transport::TransportSpill::new(&spill)
            .unwrap()
            .finish()
            .unwrap();
        assert_eq!(finalize(&spill, &output, 1, &[]).unwrap(), 1);
        let (_, batches) = read_ipc(
            &output
                .join(crate::osm::geometry::square_name(square))
                .join("buildings.arrow"),
        );
        let mut emitted = Vec::new();
        for batch in batches {
            let ids = batch
                .column_by_name("osm_id")
                .unwrap()
                .as_any()
                .downcast_ref::<arrow_array::Int64Array>()
                .unwrap();
            let classes = batch
                .column_by_name("building_type")
                .unwrap()
                .as_any()
                .downcast_ref::<arrow_array::UInt8Array>()
                .unwrap();
            emitted.extend((0..batch.num_rows()).map(|i| (ids.value(i), classes.value(i))));
        }
        emitted.sort_unstable();
        assert_eq!(emitted, expected, "geometry column {geometry_column:?}");
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
    // A house, then a school ground far from it: the ground keeps its flag.
    let rows: Vec<Vec<String>> = [
        format!("100\t22\t1500\t2500\t0\t0\t0\t0\tH\tS\t1\t0\t0\t{ring}"),
        "100\t23\t900000\t900000\t3\t0\t0\t0\tG\t\t\t0\t1\t".to_string(),
    ]
    .iter()
    .map(|row| row.split('\t').map(str::to_string).collect())
    .collect();
    let stats = JoinStats::default();
    write_buildings(
        &rows,
        &path,
        crate::osm::spill::square_from_spill_key(100).unwrap(),
        &PoiIndex::default(),
        &stats,
    )
    .unwrap();
    let (schema, batches) = read_ipc(&path);
    let batch =
        arrow_select::concat::concat_batches(&std::sync::Arc::new(schema.clone()), &batches)
            .unwrap();
    let column_of = |name: &str| batch.column(schema.index_of(name).unwrap()).clone();
    let osm_ids = column_of("osm_id");
    let osm_ids = osm_ids.as_any().downcast_ref::<Int64Array>().unwrap();
    let area_sources = column_of("area_source");
    let area_sources = area_sources
        .as_any()
        .downcast_ref::<BooleanArray>()
        .unwrap();
    let flags: Vec<(i64, bool)> = (0..batch.num_rows())
        .map(|row| (osm_ids.value(row), area_sources.value(row)))
        .collect();
    assert!(
        flags.contains(&(22, false)) && flags.contains(&(23, true)),
        "{flags:?}"
    );
    let house = flags.iter().position(|&(id, _)| id == 22).unwrap();
    let geom = batch
        .column(schema.index_of("geom").unwrap())
        .as_any()
        .downcast_ref::<BinaryArray>()
        .unwrap();
    let ring = crate::osm::geometry::poly::decode_grid_poly(geom.value(house)).unwrap();
    assert_eq!(ring.len(), 4);
    // Area of the ~100×100 m test ring survives the roundtrip.
    let area = batch
        .column(schema.index_of("area_m2").unwrap())
        .as_any()
        .downcast_ref::<arrow_array::Float32Array>()
        .unwrap()
        .value(house);
    assert!((9_000.0..11_000.0).contains(&area), "area={area}");
    assert_eq!(
        schema
            .metadata()
            .get("buildings_contract")
            .map(String::as_str),
        Some(BUILDINGS_CONTRACT_V5)
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn open_canopy_identity_survives_spill_and_arrow_without_erasing_garages() {
    use crate::osm::classify::{FeatureType, Tags};
    use crate::osm::spill::Spiller;
    let dir = scratch_dir("carport-envelope");
    let spill = dir.join("spill");
    let output = dir.join("prepared");
    let square = crate::osm::geometry::square_of(50.0, 14.0);
    let mut spiller = Spiller::new(&spill, 1, Default::default(), &[]).unwrap();
    // Explicit open structures include a roofed surface car park.
    let cases: &[(&[(&str, &str)], u8)] = &[
        (&[("building", "carport")], 3),
        (&[("building", "roof")], 3),
        (
            &[
                ("building", "roof"),
                ("amenity", "parking"),
                ("parking", "surface"),
            ],
            3,
        ),
        (&[("amenity", "parking"), ("parking", "carports")], 3),
        (
            &[
                ("building", "yes"),
                ("amenity", "parking"),
                ("parking", "carports"),
            ],
            3,
        ),
        (
            &[("building", "carport"), ("building:use", "commercial")],
            3,
        ),
        (&[("building", "garage")], 0),
        (
            &[
                ("building", "garages"),
                ("amenity", "parking"),
                ("parking", "garage_boxes"),
            ],
            0,
        ),
        (&[("amenity", "parking"), ("parking", "multi-storey")], 0),
        (&[("building", "house"), ("parking", "carports")], 0),
        (&[("building", "yes"), ("building:use", "industrial")], 2),
    ];
    for (index, (pairs, _)) in cases.iter().enumerate() {
        let tags: Tags = pairs
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect();
        spiller
            .emit_polygon(
                &FeatureType::Building,
                square,
                index as i64,
                "way",
                50.0,
                14.0,
                &tags,
                Some(&[
                    [50.0, 14.0],
                    [50.0, 14.0001],
                    [50.0001, 14.0001],
                    [50.0, 14.0],
                ]),
            )
            .unwrap();
    }
    crate::osm::transport::TransportSpill::new(&spill)
        .unwrap()
        .finish()
        .unwrap();
    spiller.finish().unwrap();
    drop(spiller);
    finalize(&spill, &output, 1, &[]).unwrap();
    let (schema, batches) = read_ipc(
        &output
            .join(crate::osm::geometry::square_name(square))
            .join("buildings.arrow"),
    );
    assert_eq!(
        schema.metadata()["buildings_contract"],
        BUILDINGS_CONTRACT_V5
    );
    let mut seen = 0;
    for batch in batches {
        let ids = batch
            .column_by_name("osm_id")
            .unwrap()
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        let uses = batch
            .column_by_name("building_use")
            .unwrap()
            .as_any()
            .downcast_ref::<UInt8Array>()
            .unwrap();
        let geometry = batch
            .column_by_name("geom")
            .unwrap()
            .as_any()
            .downcast_ref::<BinaryArray>()
            .unwrap();
        let areas = batch
            .column_by_name("area_source")
            .unwrap()
            .as_any()
            .downcast_ref::<BooleanArray>()
            .unwrap();
        for row in 0..batch.num_rows() {
            assert_eq!(uses.value(row), cases[ids.value(row) as usize].1);
            assert!(!areas.value(row), "a canopy retains its screening geometry");
            assert!(!geometry.value(row).is_empty());
            seen += 1;
        }
    }
    assert_eq!(seen, cases.len());
    std::fs::remove_dir_all(dir).unwrap();
}
