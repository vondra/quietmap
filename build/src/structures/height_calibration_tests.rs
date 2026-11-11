//! The height fit's laws on hand-computed samples, and footprint surveys reaching its samples.

use super::{Sample, fitted_model, square_samples};
use crate::structures::{
    height_raster::HeightRaster,
    observations::{self, Observation},
    prepare_geometry as geometry, prepare_rows,
};
use geos::Geom;
use serde_json::json;

#[test]
fn floor_and_area_laws_and_bounds_follow_the_surveyed_samples() {
    // 100 floor pairs on exactly 3 m a floor plus 2 m; 500 untagged footprints of 1..=500 m2 whose
    // roofs alternate between bin + 3 and bin + 5 m; a mapped height leaves the area bins.
    let mut samples: Vec<_> = (0..100)
        .map(|i| Sample {
            area_m2: 80.0,
            roof_m: 3.0 * f64::from(1 + i % 4) + 2.0,
            floors: 1 + i % 4,
            mapped_height: i % 2 == 0,
        })
        .collect();
    samples.extend((0..500).map(|i| Sample {
        area_m2: f64::from(i + 1),
        roof_m: f64::from(3 + i / 100 + 2 * (i % 2)),
        floors: 0,
        mapped_height: false,
    }));
    samples.push(Sample {
        area_m2: 1.0,
        roof_m: 90.0,
        floors: 0,
        mapped_height: true,
    });
    let mut walls: Vec<_> = (0..100).map(|i| f64::from(1 + i % 5)).collect();
    let (table, support) = fitted_model(&samples, &mut walls).unwrap();
    assert_eq!(
        table,
        json!({"measured_bounds_m":[3.0,32767.0],"overture_minimum_m":3.0,
            "area_upper_m2":[101.0,201.0,301.0,401.0],"area_height_m":[4.0,5.0,6.0,7.0,8.0],
            "floor_ranges":[{"through":255,"slope_m":3.0,"intercept_m":2.0}],
            "storey_height_m":3.0,"storey_roof_m":2.0,"wall_default_m":3.0,"wall_country_m":{}})
    );
    assert_eq!(support["area_bin_counts"], json!([100, 100, 100, 100, 100]));
    assert_eq!(support["measured_buildings"], json!(601));
    assert!(fitted_model(&samples[..599], &mut walls).is_err());
}

#[test]
fn a_footprint_survey_reaches_the_samples_replaces_the_raster_roof_and_below_a_roof_is_none() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("test-data/structures");
    let square = root.join("z9/276/174");
    let raster = [HeightRaster::open(&root.join("height.tif")).unwrap()];
    let fixture: Vec<_> = observations::read(&square.join("structure-observations.arrow"), "2026")
        .unwrap()
        .into_iter()
        .filter(|row| row.kind == observations::HEIGHT)
        .collect();
    let (raster_only, walls) = square_samples(&square, &[], &raster).unwrap();
    assert_eq!(walls, [2.5, 4.0]);
    let [rastered] = raster_only[..] else {
        panic!("{raster_only:?}")
    };
    assert_eq!((rastered.roof_m, rastered.floors), (8.5, 3));
    // The fixture's 17.5 m survey meets a building the raster misses; its -1 m courtyard record
    // meets no OSM outline.
    let (surveyed, _) = square_samples(&square, &fixture, &raster).unwrap();
    assert_eq!(surveyed.len(), 2);
    assert_eq!((surveyed[0].roof_m, surveyed[0].floors), (17.5, 3));
    assert_eq!(surveyed[1], rastered);
    // A survey of the rastered outline replaces its raster roof, as in structures; a survey below
    // the measured minimum takes the building out of the fit.
    let outline = prepare_rows::read_osm(&square.join("buildings.arrow"))
        .unwrap()
        .into_iter()
        .filter_map(|b| geometry::ring_geometry(b.row.emission_geometry.as_deref()?).unwrap())
        .find(|g| {
            let (longitude, latitude) = geometry::centroid(g).unwrap();
            raster[0].sample(g, longitude, latitude).unwrap() == Some(8.5)
        })
        .unwrap();
    let survey = |height_m| {
        let (longitude, latitude) = geometry::centroid(&outline).unwrap();
        Observation {
            kind: observations::HEIGHT,
            geometry: Some(outline.to_wkb().unwrap().to_vec()),
            longitude,
            latitude,
            height_m: Some(height_m),
            floors: 0,
            building_type: None,
            envelope: 0,
            open_roof: false,
            owned: true,
        }
    };
    let (replaced, _) = square_samples(&square, &[survey(12.0)], &raster).unwrap();
    assert_eq!(
        replaced,
        [Sample {
            roof_m: 12.0,
            ..rastered
        }]
    );
    let (emptied, _) = square_samples(&square, &[survey(1.0)], &raster).unwrap();
    assert!(emptied.is_empty(), "{emptied:?}");
}
