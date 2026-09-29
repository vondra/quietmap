//! The popup's receiver for a click inside an enclosed building: the stored noisiest façade.
//!
//! The façade-exposure stage chose, per building, the §2.8 façade receiver with the
//! highest all-source Lden (`facade_exposure.arrow`); the painter paints that
//! receiver's levels into the building's pixels. The popup recomputes the same
//! receiver set on the CPU, checks that the stored choice is one of its points,
//! and then evaluates that point exactly. A release without the stage's file is
//! incomplete: the click is refused, never answered from another point.

use std::path::Path;

use noise_compute::facade_receivers::{exposed_facade_receivers, FacadeReceiverPosition};
use noise_compute::propagation::obstacle_index::{EnclosedFootprint, FootprintKey, ObstacleSet};
use square_store::facade_exposure_contract::{
    self as contract, FACADE_EXPOSURE_ARROW, STRUCTURES_BYTES_KEY,
};
use square_store::store::LazyArrow;

use crate::square_obstacle_index::STRUCTURES_ARROW;

/// The building a click lies in and the façade receiver that speaks for it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BuildingExposure {
    pub building: FootprintKey,
    /// Exposed façade receivers compared; 0 = the building has no exposed façade.
    pub facade_points: u32,
    /// The noisiest of them; `None` exactly when `facade_points == 0`.
    pub receiver: Option<FacadeReceiverPosition>,
}

/// Resolve the stored building exposure of `building`, which contains the click.
pub fn stored_building_exposure(
    prepared_year_dir: &Path,
    click_obstacles: &ObstacleSet,
    building: EnclosedFootprint,
    lat: f64,
    lon: f64,
) -> Result<BuildingExposure, String> {
    let key = building.key;
    let square_dir = prepared_year_dir.join(grid::square_name(key.square()));
    let exposure = LazyArrow::open(&square_dir.join(FACADE_EXPOSURE_ARROW))?;
    let Some(schema) = exposure.schema() else {
        return Err(format!(
            "building exposure missing in this release: {} has no {FACADE_EXPOSURE_ARROW}",
            grid::square_name(key.square())
        ));
    };
    contract::validate_schema(schema)?;
    let structures_path = square_dir.join(STRUCTURES_ARROW);
    let structures_bytes = std::fs::metadata(&structures_path)
        .map_err(|error| format!("{}: {error}", structures_path.display()))?
        .len()
        .to_string();
    if schema.metadata().get(STRUCTURES_BYTES_KEY) != Some(&structures_bytes) {
        return Err(format!(
            "{FACADE_EXPOSURE_ARROW} of {} was computed from another structures.arrow",
            grid::square_name(key.square())
        ));
    }
    let row = contract::find_row(&exposure.batches_within(lat, lon, 0.0)?, key.id)?
        .ok_or_else(|| {
            format!(
                "building exposure missing in this release: footprint {} of {}",
                key.id,
                grid::square_name(key.square())
            )
        })?;
    let structures = LazyArrow::open(&structures_path)?;
    let polygons = crate::structure_store::footprint_polygons_at(&structures, key.id, lat, lon)?;
    let receivers = exposed_facade_receivers(&polygons, click_obstacles);
    let chosen = row.chosen.as_ref().map(|chosen| {
        receivers
            .get(chosen.canonical_index as usize)
            .filter(|receiver| (receiver.gx, receiver.gy) == (chosen.gx, chosen.gy))
            .copied()
    });
    if receivers.len() != row.facade_points as usize || chosen.is_some_and(|found| found.is_none()) {
        return Err(format!(
            "façade receivers of footprint {} in {} differ from the stored building exposure \
             ({} here, {} stored)",
            key.id,
            grid::square_name(key.square()),
            receivers.len(),
            row.facade_points
        ));
    }
    Ok(BuildingExposure {
        building: key,
        facade_points: row.facade_points,
        receiver: chosen.flatten(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::structure_test_fixture as fx;

    const LAT: f64 = 50.0;
    const LON: f64 = 14.25;

    fn row(ring: Vec<(f64, f64)>, height_m: i16) -> fx::StructureRow {
        fx::StructureRow {
            kind: square_store::store::STRUCTURE_KIND_BUILDING,
            centroid_lonlat: Some(ring[0]),
            ring_lonlat: Some(ring),
            height_m,
            envelope_class: 1,
            area_m2: Some(400.0),
            ..Default::default()
        }
    }

    fn rectangle(west: f64, south: f64, east: f64, north: f64) -> Vec<(f64, f64)> {
        vec![(west, south), (east, south), (east, north), (west, north), (west, south)]
    }

    /// The stored choice is used when it is one of the CPU's receivers; a
    /// building nested inside another has no exposed façade; a mismatch or a
    /// missing file refuses the click.
    #[test]
    fn the_popup_uses_the_stored_choice_only_when_it_is_one_of_its_own_receivers() {
        let tmp = tempfile::TempDir::new().unwrap();
        let square = grid::square_of(LAT, LON);
        let house = rectangle(LON, LAT, LON + 0.0006, LAT + 0.0004);
        let nested = rectangle(LON + 0.0004, LAT + 0.0001, LON + 0.0005, LAT + 0.0002);
        let structures = fx::write_square_structures(tmp.path(), square, &[row(house, 10), row(nested, 15)]);
        let dir = fx::square_dir(tmp.path(), square);
        let set = crate::structure_store::load_obstacle_set(tmp.path(), LAT, LON).unwrap();
        let footprints = crate::structure_store::enclosed_building_footprints(
            &std::fs::read(&structures).unwrap(),
            &structures,
        )
        .unwrap();
        let receivers: Vec<_> = footprints
            .iter()
            .map(|(_, rings)| exposed_facade_receivers(rings, &set))
            .collect();
        assert!(receivers[1].is_empty(), "the nested building has no exposed façade");
        let (house_click, nested_click) = ((LAT + 0.0003, LON + 0.0001), (LAT + 0.00015, LON + 0.00045));
        let house_building = set.enclosed_footprint_at(house_click.0, house_click.1).unwrap();
        let nested_building = set.enclosed_footprint_at(nested_click.0, nested_click.1).unwrap();
        assert_eq!((house_building.key.id, nested_building.key.id), (0, 1));
        let exposure = |building, (lat, lon)| stored_building_exposure(tmp.path(), &set, building, lat, lon);
        assert!(exposure(house_building, house_click).unwrap_err().contains("building exposure missing"));

        fx::write_facade_exposure(
            &dir,
            &[
                fx::facade_exposure_row(0, &receivers[0], Some(5)),
                fx::facade_exposure_row(1, &receivers[1], None),
            ],
        );
        let house = exposure(house_building, house_click).unwrap();
        assert_eq!(house.receiver, Some(receivers[0][5]));
        assert_eq!(house.facade_points as usize, receivers[0].len());
        let nested = exposure(nested_building, nested_click).unwrap();
        assert_eq!((nested.facade_points, nested.receiver), (0, None));

        let mut moved = fx::facade_exposure_row(0, &receivers[0], Some(5));
        moved.chosen.as_mut().unwrap().gx += 1;
        fx::write_facade_exposure(&dir, &[moved, fx::facade_exposure_row(1, &receivers[1], None)]);
        assert!(exposure(house_building, house_click).unwrap_err().contains("differ"));
    }
}
