//! Overture GeoParquet to above-ground footprints with canonical centroid ownership and envelope classes.

use super::{Objects, Observation, Square, batches, number, selected, text};
use crate::structures::{observations::FOOTPRINT, prepare_geometry::centroid};
use anyhow::{Result, ensure};
use arrow_array::{Array, BinaryArray, BooleanArray};
use geos::{Geom, Geometry, GeometryTypes};
use std::path::Path;

fn envelope(class: Option<&str>, subtype: Option<&str>) -> u8 {
    match class {
        Some(
            "carport" | "roof" | "greenhouse" | "glasshouse" | "bridge_structure" | "grandstand",
        ) => 0,
        Some(
            "allotment_house" | "apartments" | "beach_hut" | "boathouse" | "bungalow" | "cabin"
            | "college" | "detached" | "dormitory" | "dwelling_house" | "ger" | "hospital"
            | "house" | "houseboat" | "hut" | "kindergarten" | "residential" | "school" | "semi"
            | "semidetached_house" | "static_caravan" | "stilt_house" | "terrace" | "trullo"
            | "university",
        ) => 1,
        Some("commercial" | "hotel" | "office" | "retail" | "supermarket") => 2,
        Some(
            "agricultural" | "barn" | "cowshed" | "digester" | "factory" | "farm"
            | "farm_auxiliary" | "hangar" | "industrial" | "manufacture" | "shed" | "silo"
            | "slurry_tank" | "stable" | "storage_tank" | "sty" | "warehouse",
        ) => 3,
        Some(
            "cathedral" | "chapel" | "church" | "civic" | "fire_station" | "government" | "library"
            | "monastery" | "mosque" | "post_office" | "presbytery" | "public" | "religious"
            | "shrine" | "synagogue" | "temple" | "wayside_shrine",
        ) => 4,
        Some(
            "garage" | "garages" | "kiosk" | "service" | "parking" | "stadium" | "sports_centre"
            | "sports_hall" | "pavilion" | "toilets" | "bunker" | "military" | "transportation"
            | "train_station" | "transformer_tower" | "outbuilding" | "guardhouse",
        ) => 5,
        _ => match subtype {
            Some("residential" | "education" | "medical") => 1,
            Some("commercial") => 2,
            Some("agricultural" | "industrial") => 3,
            Some("civic" | "religious") => 4,
            _ => 5,
        },
    }
}

pub fn convert(
    source: &Objects,
    root: &Path,
    squares: &[Square],
    rows: &mut [Vec<Observation>],
) -> Result<()> {
    let tiled = source.objects.iter().all(|o| o.degree.is_some());
    ensure!(
        tiled || source.objects.iter().all(|o| o.degree.is_none()),
        "cannot mix tiled and untiled footprints"
    );
    let mut objects: Vec<_> = source.objects.iter().collect();
    if tiled {
        objects.sort_by_key(|o| o.degree.unwrap());
        let tiles: std::collections::BTreeSet<_> =
            objects.iter().map(|o| o.degree.unwrap()).collect();
        ensure!(
            tiles.len() == objects.len(),
            "repeated Overture degree input"
        );
        for &square in squares {
            for [latitude, longitude] in super::touched_degrees(square, 0.0) {
                ensure!(
                    tiles.contains(&[latitude, longitude]),
                    "{}:{}: missing Overture input degree {latitude},{longitude}",
                    square.x,
                    square.y
                );
            }
        }
        let touched: std::collections::BTreeSet<_> = squares
            .iter()
            .flat_map(|&s| super::touched_degrees(s, 0.0))
            .collect();
        objects.retain(|o| touched.contains(&o.degree.unwrap()));
    }
    let mut selected_rows = vec![Vec::<([i16; 2], usize, Observation)>::new(); squares.len()];
    let mut sequence = 0usize;
    for object in objects {
        let path = root.join(&object.path);
        batches(
            &path,
            &[
                "geometry",
                "height",
                "num_floors",
                "class",
                "subtype",
                "is_underground",
            ],
            Some(squares),
            None,
            |batch| {
                let geometry = batch
                    .column_by_name("geometry")
                    .and_then(|a| a.as_any().downcast_ref::<BinaryArray>())
                    .ok_or_else(|| anyhow::anyhow!("{}: missing geometry", path.display()))?;
                let underground = batch
                    .column_by_name("is_underground")
                    .and_then(|a| a.as_any().downcast_ref::<BooleanArray>());
                for i in 0..batch.num_rows() {
                    let order = sequence;
                    sequence += 1;
                    if geometry.is_null(i)
                        || underground.is_some_and(|a| !a.is_null(i) && a.value(i))
                    {
                        continue;
                    }
                    let bytes = geometry.value(i);
                    let polygon = Geometry::new_from_wkb(bytes)?;
                    if !matches!(
                        polygon.geometry_type()?,
                        GeometryTypes::Polygon | GeometryTypes::MultiPolygon
                    ) || polygon.is_empty()?
                    {
                        continue;
                    }
                    let (longitude, latitude) = centroid(&polygon)?;
                    if !longitude.is_finite() || !latitude.is_finite() {
                        continue;
                    }
                    let degree = [latitude.floor() as i16, longitude.floor() as i16];
                    if object.degree.is_some_and(|tile| tile != degree) {
                        continue;
                    }
                    let class = text(&batch, "class", i);
                    let floors = number(&batch, "num_floors", i)
                        .filter(|v| *v > 0.0 && *v <= 255.0)
                        .unwrap_or(0.0) as u8;
                    for (k, &square) in squares.iter().enumerate() {
                        if !selected(longitude, latitude, square, 0.0).0 {
                            continue;
                        }
                        selected_rows[k].push((
                            degree,
                            order,
                            Observation {
                                kind: FOOTPRINT,
                                geometry: Some(bytes.to_vec()),
                                longitude,
                                latitude,
                                height_m: number(&batch, "height", i),
                                floors,
                                building_type: None,
                                envelope: envelope(class, text(&batch, "subtype", i)),
                                open_roof: matches!(class, Some("carport" | "roof")),
                                owned: true,
                            },
                        ));
                    }
                }
                Ok(())
            },
        )?;
    }
    for (out, mut found) in rows.iter_mut().zip(selected_rows) {
        found.sort_by_key(|r| (r.0, r.1));
        out.extend(found.into_iter().map(|r| r.2));
    }
    Ok(())
}
