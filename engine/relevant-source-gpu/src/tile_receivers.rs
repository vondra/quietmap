//! z13 pixel receivers: the pixel centre outdoors, the stored building exposure inside a building.
//!
//! A pixel whose centre lies inside an enclosed footprint (the tallest one, the
//! winner rule the popup shares) shows that building's exposure: the per-layer
//! powers of its noisiest façade receiver from `facade_exposure.arrow`, the same
//! record in every tile that covers the building. Every other pixel is the
//! outdoor point receiver at its centre. No receiver is moved, no indoor
//! attenuation is applied.
use crate::{
    building_exposure_table::BuildingExposureTables, receiver_points::ReceiverPoints,
    source_frame::*, surface_scene::SurfaceScene,
};
use anyhow::{ensure, Result};
use noise_compute::propagation::obstacle_index::FootprintKey;
use rayon::prelude::*;
use tile_painter::hm3::SOURCE_LAYERS;

pub struct TileReceivers {
    /// Every pixel centre, as the surface kernel paints it.
    pub points: ReceiverPoints,
    /// Per pixel, the enclosed building its centre lies in.
    pub buildings: Vec<Option<FootprintKey>>,
}

/// The (lat, lon) of one pixel centre of z13 tile (x, y).
pub fn pixel_centre(tile_x: u32, tile_y: u32, pixel: usize) -> (f64, f64) {
    let row = pixel / TILE_PIXEL_SIDE;
    let col = pixel % TILE_PIXEL_SIDE;
    let world_pixels = 8192.0 * TILE_PIXEL_SIDE as f64;
    let lon = (f64::from(tile_x) * TILE_PIXEL_SIDE as f64 + col as f64 + 0.5) / world_pixels * 360.0
        - 180.0;
    let lat = (std::f64::consts::PI
        * (1.0 - 2.0 * (f64::from(tile_y) * TILE_PIXEL_SIDE as f64 + row as f64 + 0.5) / world_pixels))
        .sinh()
        .atan()
        .to_degrees();
    (lat, lon)
}

impl TileReceivers {
    pub fn prepare(scene: &SurfaceScene, tile_x: u32, tile_y: u32) -> Result<Self> {
        ensure!(
            tile_x / 16 == u32::from(scene.owner.x) && tile_y / 16 == u32::from(scene.owner.y),
            "receiver tile outside scene owner"
        );
        let centres: Vec<_> = (0..TILE_PIXEL_SIDE * TILE_PIXEL_SIDE)
            .map(|pixel| pixel_centre(tile_x, tile_y, pixel))
            .collect();
        let buildings = centres
            .par_iter()
            .map(|&(lat, lon)| scene.obstacles.enclosed_footprint_at(lat, lon).map(|b| b.key))
            .collect();
        let points = ReceiverPoints::prepare(
            scene,
            &centres.iter().map(|&(lat, lon)| (lat, lon, None)).collect::<Vec<_>>(),
        )?;
        Ok(Self { points, buildings })
    }

    /// Pixels whose centre is outdoors: the only ones the aircraft fields evaluate.
    pub fn outdoor_pixels(&self) -> Vec<usize> {
        (0..self.buildings.len())
            .filter(|&pixel| self.buildings[pixel].is_none())
            .collect()
    }

    /// Replace every building pixel of the published planes (`SOURCE_LAYERS`
    /// order) by its building's stored powers; returns per pixel whether it was
    /// assessed (false for a building without an exposed façade).
    pub fn apply_building_exposure(
        &self,
        planes: &mut [Vec<f32>; SOURCE_LAYERS.len()],
        exposures: &mut BuildingExposureTables,
    ) -> Result<Vec<bool>> {
        exposures.load(self.buildings.iter().flatten().copied())?;
        let mut assessed = vec![true; self.buildings.len()];
        for (pixel, building) in self.buildings.iter().enumerate() {
            let Some(building) = building else { continue };
            match exposures.powers(*building)? {
                None => assessed[pixel] = false,
                Some(powers) => {
                    ensure!(
                        powers.len() == SOURCE_LAYERS.len() * PERIOD_COUNT,
                        "building exposure has the wrong layer count"
                    );
                    for (layer, plane) in planes.iter_mut().enumerate() {
                        plane[pixel * PERIOD_COUNT..(pixel + 1) * PERIOD_COUNT].copy_from_slice(
                            &powers[layer * PERIOD_COUNT..(layer + 1) * PERIOD_COUNT],
                        );
                    }
                }
            }
        }
        Ok(assessed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::building_exposure_table::{physics_generation_stamp, source_layer_order};
    use crate::input_manifest::{file_digest, InputManifest};
    use square_store::facade_exposure_contract::{self as contract, ChosenFacadeReceiver, FacadeExposureRow};

    const SQUARE: grid::Square = grid::Square { x: 276, y: 173 };

    fn key(id: u32) -> FootprintKey {
        FootprintKey {
            square_y: SQUARE.y,
            square_x: SQUARE.x,
            id,
        }
    }

    /// A prepared tree with one square's `structures.arrow` stand-in and its
    /// `facade_exposure.arrow`, pinned by a manifest; `generation` is stamped.
    fn prepared(temp: &std::path::Path, rows: &[FacadeExposureRow], generation: &str) -> InputManifest {
        let dir = temp.join(format!("z9/{}/{}", SQUARE.x, SQUARE.y));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("structures.arrow"), b"pinned structures bytes").unwrap();
        let structures_digest = file_digest(&dir.join("structures.arrow")).unwrap();
        let metadata = std::collections::HashMap::from([
            (contract::CONTRACT_KEY.to_string(), contract::CONTRACT.to_string()),
            ("grid".to_string(), square_store::store::GRID_CONTRACT_Z30.to_string()),
            (contract::LAYER_ORDER_KEY.to_string(), source_layer_order()),
            (contract::PHYSICS_GENERATION_KEY.to_string(), generation.to_string()),
            (
                contract::STRUCTURES_SHA256_KEY.to_string(),
                tile_painter::generation_receipt::hex_digest(structures_digest),
            ),
        ]);
        let schema = std::sync::Arc::new(contract::schema(SOURCE_LAYERS.len(), metadata));
        let batch = arrow::record_batch::RecordBatch::try_new(
            schema.clone(),
            contract::columns(rows, SOURCE_LAYERS.len()).unwrap(),
        )
        .unwrap();
        let file = std::fs::File::create(dir.join(contract::FACADE_EXPOSURE_ARROW)).unwrap();
        let mut writer = arrow::ipc::writer::FileWriter::try_new(file, &schema).unwrap();
        writer.write(&batch).unwrap();
        writer.finish().unwrap();
        let manifest = temp.join("inputs.sqlite");
        let db = rusqlite::Connection::open(&manifest).unwrap();
        db.execute_batch("CREATE TABLE input_files(relative_path TEXT PRIMARY KEY, sha256 BLOB NOT NULL)")
            .unwrap();
        for name in ["structures.arrow", contract::FACADE_EXPOSURE_ARROW] {
            db.execute(
                "INSERT INTO input_files VALUES(?1,?2)",
                rusqlite::params![
                    format!("z9/{}/{}/{name}", SQUARE.x, SQUARE.y),
                    file_digest(&dir.join(name)).unwrap().as_slice()
                ],
            )
            .unwrap();
        }
        drop(db);
        InputManifest::open(&manifest, file_digest(&manifest).unwrap()).unwrap()
    }

    fn exposed(id: u32) -> FacadeExposureRow {
        FacadeExposureRow {
            footprint_id: id,
            facade_points: 12,
            chosen: Some(ChosenFacadeReceiver {
                canonical_index: 3,
                gx: 0,
                gy: 0,
                ground_altitude_m: 200.0,
                outward_bearing_deg: 90.0,
                layer_period_power: (0..SOURCE_LAYERS.len() * PERIOD_COUNT).map(|v| v as f32 + 1.0).collect(),
                total_lden_db: 60.0,
                runner_up_lden_db: Some(55.0),
            }),
        }
    }

    fn receivers(buildings: Vec<Option<FootprintKey>>) -> TileReceivers {
        TileReceivers {
            points: ReceiverPoints::default(),
            buildings,
        }
    }

    /// Every pixel of a building copies its one stored row into each layer; a
    /// building without an exposed façade is not assessed; outdoor pixels keep
    /// their painted powers.
    #[test]
    fn building_pixels_copy_their_building_row_and_faceless_buildings_are_not_assessed() {
        let temp = tempfile::tempdir().unwrap();
        let faceless = FacadeExposureRow { footprint_id: 8, facade_points: 0, chosen: None };
        let manifest = prepared(temp.path(), &[exposed(7), faceless], &physics_generation_stamp());
        let mut tables = BuildingExposureTables::new(temp.path(), &manifest);
        let tile = receivers(vec![Some(key(7)), None, Some(key(7)), Some(key(8))]);
        let mut planes: [Vec<f32>; SOURCE_LAYERS.len()] =
            std::array::from_fn(|_| vec![0.5; 4 * PERIOD_COUNT]);
        let assessed = tile.apply_building_exposure(&mut planes, &mut tables).unwrap();
        assert_eq!(assessed, [true, true, true, false]);
        for (layer, plane) in planes.iter().enumerate() {
            let stored: Vec<f32> = (0..PERIOD_COUNT).map(|p| (layer * PERIOD_COUNT + p) as f32 + 1.0).collect();
            assert_eq!(plane[0..3], stored[..]);
            assert_eq!(plane[6..9], stored[..], "the same row in every pixel of the building");
            assert_eq!(plane[3..6], [0.5; 3], "outdoor pixels keep their paint");
        }
        let unknown = receivers(vec![Some(key(9))]);
        let error = unknown.apply_building_exposure(&mut planes, &mut tables).unwrap_err();
        assert!(error.to_string().contains("building exposure missing"), "{error}");
    }

    #[test]
    fn rows_of_another_physics_generation_are_refused() {
        let temp = tempfile::tempdir().unwrap();
        let manifest = prepared(temp.path(), &[exposed(7)], "surface-corners-physics-gen-0001");
        let mut tables = BuildingExposureTables::new(temp.path(), &manifest);
        let error = tables.load([key(7)]).unwrap_err();
        assert!(error.to_string().contains("another physics generation"), "{error}");
    }
}
