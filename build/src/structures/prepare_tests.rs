//! Regression tests for retained footprint identities, emission evidence and height precedence.

use super::{
    height_model::{HeightModel, screening_metres},
    observations::{FOOTPRINT, Observation},
    prepare::{HeightEvidence, match_footprints},
    prepare_geometry::{self as geometry, FootprintIndex},
    prepare_rows::{OsmBuilding, StructureRow},
};
use geos::{Geom, Geometry};

fn model() -> HeightModel {
    HeightModel::read(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../years/tables/2026/structure-height-model.json"),
    )
    .unwrap()
}

fn footprint(wkt: &str) -> Observation {
    let geometry = Geometry::new_from_wkt(wkt).unwrap();
    let (longitude, latitude) = geometry::centroid(&geometry).unwrap();
    Observation {
        kind: FOOTPRINT,
        geometry: Some(geometry.to_wkb().unwrap().to_vec()),
        longitude,
        latitude,
        height_m: Some(12.0),
        floors: 0,
        building_type: None,
        envelope: 2,
        open_roof: false,
        owned: true,
    }
}

#[test]
fn height_evidence_precedence_and_storeys_survive_quantization() {
    let model = model();
    model.validate().unwrap();
    assert_eq!(
        model.height(Some(280.0), Some(24.0), 3, Some(40.0), 100.0),
        (250.0, 3)
    );
    assert_eq!(
        model.height(None, Some(24.0), 3, Some(40.0), 100.0),
        (24.0, 0)
    );
    assert_eq!(
        model.height(None, Some(f64::NAN), 1, Some(40.0), 100.0),
        (6.0, 1)
    );
    assert_eq!(model.height(None, None, 0, Some(2.49), 100.0), (7.4, 2));
    assert_eq!(model.height(None, None, 0, Some(2.5), 100.0), (2.5, 5));
    assert_eq!(model.height(None, None, 0, None, 30.0), (3.5, 2));
    assert_eq!(model.storeys(0, Some(8.49)), 2);
    assert_eq!(model.storeys(0, Some(8.5)), 3);
    assert_eq!(model.storeys(0, None), 1);
    assert_eq!(model.storeys(7, Some(4.0)), 7);
    assert_eq!(screening_metres(2.5).unwrap(), 3);
    assert!(screening_metres(f64::NAN).is_err());
    assert!(screening_metres(32767.5).is_err());
    assert_eq!(
        model.wall_height(8.0, true, u16::from_le_bytes(*b"US")),
        (8.0, 0)
    );
    assert_eq!(
        model.wall_height(8.0, false, u16::from_le_bytes(*b"US")),
        (4.45, 8)
    );
}

#[test]
fn overture_without_osm_identity_is_a_real_screening_row() {
    let model = model();
    let evidence = HeightEvidence {
        measured: &[],
        measured_index: FootprintIndex::new(vec![]).unwrap(),
        raster: &[],
        model: &model,
    };
    let footprint = footprint(
        "POLYGON ((14 50,14.001 50,14.001 50.001,14 50.001,14 50),(14.0002 50.0002,14.0008 50.0002,14.0008 50.0008,14.0002 50.0008,14.0002 50.0002))",
    );
    let row = evidence
        .building(
            StructureRow::default(),
            None,
            255,
            false,
            Some(&footprint),
            Some(0),
        )
        .unwrap();
    assert_eq!(row.osm_id, None);
    assert_eq!(row.building_type, None);
    assert_eq!(row.area_m2, None);
    assert_eq!(row.ordinal, Some(0));
    assert_eq!(row.height_source, 5);
    assert_eq!(row.height_m, 12);
    assert_eq!(row.storeys, Some(4));
    assert_eq!(
        super::decode_parts(row.geometry.as_ref().unwrap()).unwrap()[0].len(),
        2
    );
    assert!(row.emission_geometry.is_none());
    assert!(row.emission_centroid.is_none());
    let mut bytes = std::env::temp_dir();
    bytes.push(format!(
        "qm-structure-roundtrip-{}.arrow",
        std::process::id()
    ));
    super::prepare_rows::write(&bytes, &[row]).unwrap();
    let reader =
        arrow_ipc::reader::FileReader::try_new(std::fs::File::open(&bytes).unwrap(), None).unwrap();
    assert_eq!(reader.schema().fields().len(), 20);
    let batch = reader.into_iter().next().unwrap().unwrap();
    assert!(super::Rows::of(&batch).is_ok());
    std::fs::remove_file(bytes).unwrap();
}

#[test]
fn ground_activity_never_borrows_a_nearby_footprint() {
    let polygon = footprint("POLYGON ((14 50,14.001 50,14.001 50.001,14 50.001,14 50))");
    let parts = geometry::snapped_parts(&polygon.geometry().unwrap()).unwrap();
    let ring = parts[8..].to_vec();
    let osm = vec![OsmBuilding {
        row: StructureRow {
            osm_id: Some(7),
            emission_geometry: Some(ring.clone()),
            ..Default::default()
        },
        building_use: 0,
        ground_activity: true,
    }];
    let (geometry, pairs) = match_footprints(&osm, &[polygon]).unwrap();
    assert!(pairs.is_empty());
    assert!(geometry[0].is_none());
    let model = model();
    let evidence = HeightEvidence {
        measured: &[],
        measured_index: FootprintIndex::new(vec![]).unwrap(),
        raster: &[],
        model: &model,
    };
    let row = evidence
        .building(
            StructureRow {
                osm_id: Some(7),
                emission_geometry: Some(ring.clone()),
                floors: Some(0),
                ..Default::default()
            },
            None,
            0,
            true,
            None,
            None,
        )
        .unwrap();
    assert_eq!(row.emission_geometry, Some(ring));
    assert!(row.geometry.is_none());
    assert_eq!(row.height_source, 7);
    assert_eq!(row.height_m, 0);
    assert_eq!(row.storeys, Some(1));
    assert!(row.ordinal.is_none());
}

#[test]
fn degenerate_osm_ring_keeps_emission_identity_without_screening() {
    for points in [
        &[(1, 2), (3, 4), (1, 2)][..],
        &[(1, 2), (3, 4), (1, 2), (1, 2)][..],
    ] {
        let ring = crate::osm::geometry::poly::encode_grid_poly(points);
        let osm = vec![OsmBuilding {
            row: StructureRow {
                osm_id: Some(9),
                emission_geometry: Some(ring.clone()),
                ..Default::default()
            },
            building_use: 1,
            ground_activity: false,
        }];
        let (geometries, matches) = match_footprints(&osm, &[]).unwrap();
        assert!(geometries[0].is_none() && matches.is_empty());
        assert_eq!(
            osm[0].row.emission_geometry.as_deref(),
            Some(ring.as_slice())
        );
    }
}

#[test]
fn observations_from_another_year_are_refused() {
    let path = std::env::temp_dir().join(format!("qm-structure-year-{}.arrow", std::process::id()));
    super::observations::write(&path, &[], "2026").unwrap();
    assert!(super::observations::read(&path, "2026").unwrap().is_empty());
    assert!(
        super::observations::read(&path, "2024")
            .unwrap_err()
            .to_string()
            .contains("year")
    );
    std::fs::remove_file(path).unwrap();
}

#[test]
fn longitude_frame_keeps_the_short_footprint_across_the_seam() {
    let polygon = Geometry::new_from_wkt(
        "POLYGON ((179.999 10,-179.999 10,-179.999 10.001,179.999 10.001,179.999 10))",
    )
    .unwrap();
    let (longitude, latitude) = geometry::centroid(&polygon).unwrap();
    assert!((longitude.abs() - 180.0).abs() < 1e-9);
    assert!(geometry::area_m2(&polygon, latitude).unwrap() < 30_000.0);
    let snapped = geometry::snapped_parts(&polygon).unwrap();
    assert_eq!(super::decode_parts(&snapped).unwrap()[0][0].len(), 5);
}

#[test]
fn consumed_columns_equal_the_frozen_producer_with_measured_heights_and_walls() {
    use arrow_ipc::reader::FileReader;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("test-data/structures");
    let raster = super::height_raster::HeightRaster::open(&root.join("height.tif")).unwrap();
    let rows = super::prepare::square(
        &root,
        &root,
        crate::dev4::Square { x: 276, y: 174 },
        &model(),
        &[raster],
        "2026",
    )
    .unwrap();
    let output =
        std::env::temp_dir().join(format!("qm-structures-oracle-{}.arrow", std::process::id()));
    super::prepare_rows::write(&output, &rows).unwrap();
    let expected = FileReader::try_new(
        std::fs::File::open(root.join("z9/276/174/structures.arrow")).unwrap(),
        None,
    )
    .unwrap()
    .next()
    .unwrap()
    .unwrap();
    let actual = FileReader::try_new(std::fs::File::open(&output).unwrap(), None)
        .unwrap()
        .next()
        .unwrap()
        .unwrap();
    assert_eq!(actual.num_rows(), expected.num_rows());
    for field in actual.schema().fields() {
        assert!(
            actual.column_by_name(field.name()).unwrap().to_data()
                == expected.column_by_name(field.name()).unwrap().to_data(),
            "{}",
            field.name()
        );
    }
    std::fs::remove_file(output).unwrap();
}

#[test]
fn national_attributes_keep_floor_evidence_activity_classes_and_first_equal_match() {
    use super::national_attributes::NationalAttributes;
    let position = crate::osm::geometry::lonlat_to_grid(14.0, 50.0);
    let (longitude, latitude) = crate::osm::geometry::grid_cell_lonlat(position.0, position.1);
    let point = |floors, latitude| Observation {
        kind: 3,
        geometry: None,
        longitude,
        latitude,
        height_m: None,
        floors,
        building_type: Some(2),
        envelope: 5,
        open_roof: false,
        owned: true,
    };
    let points = [point(3, latitude), point(7, latitude)];
    let mut building = OsmBuilding {
        row: StructureRow {
            centroid: position,
            floors: Some(0),
            building_type: Some(11),
            ..Default::default()
        },
        building_use: 1,
        ground_activity: false,
    };
    NationalAttributes::new(&points).apply(&mut building);
    assert_eq!(building.row.floors, Some(3));
    assert_eq!(building.row.building_type, Some(11));
    building.row.floors = Some(2);
    building.row.building_type = Some(1);
    NationalAttributes::new(&points).apply(&mut building);
    assert_eq!(building.row.floors, Some(2));
    assert_eq!(building.row.building_type, Some(2));
    for (distance, expected) in [(29.999, Some(3)), (30.001, Some(0))] {
        building.row.floors = Some(0);
        NationalAttributes::new(&[point(3, latitude + distance / 110_540.0)]).apply(&mut building);
        assert_eq!(building.row.floors, expected);
    }
    building.ground_activity = true;
    building.row.floors = Some(0);
    NationalAttributes::new(&points).apply(&mut building);
    assert_eq!(building.row.floors, Some(0));
}
