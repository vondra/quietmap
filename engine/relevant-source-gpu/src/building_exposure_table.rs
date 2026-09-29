//! The painter's manifest-verified view of `facade_exposure.arrow`: building → stored layer powers.
use crate::input_manifest::InputManifest;
use anyhow::{bail, ensure, Context, Result};
use arrow::ipc::reader::FileReader;
use grid::Square;
use noise_compute::propagation::obstacle_index::FootprintKey;
use square_store::facade_exposure_contract::{
    self as contract, FACADE_EXPOSURE_ARROW, PHYSICS_GENERATION_KEY, STRUCTURES_SHA256_KEY,
};
use std::{collections::HashMap, io::Cursor, path::Path};
use tile_painter::{
    generation_receipt::{hex_digest, SURFACE_FORMAT_AND_PHYSICS_GENERATION},
    hm3::SOURCE_LAYERS,
};

/// Per building its exposure's mean powers in `SOURCE_LAYERS` × period order;
/// `None` for a building without an exposed façade.
type SquareExposures = HashMap<u32, Option<Vec<f32>>>;

/// Loads each owner square's rows once, on first use.
pub struct BuildingExposureTables<'a> {
    prepared: &'a Path,
    manifest: &'a InputManifest,
    squares: HashMap<(u16, u16), SquareExposures>,
}

/// The source-layer names in the order `layer_period_power` stores them.
pub fn source_layer_order() -> String {
    SOURCE_LAYERS.map(|layer| layer.name()).join(",")
}

/// The generation stamp the stage writes and the painter requires.
pub fn physics_generation_stamp() -> String {
    String::from_utf8_lossy(&SURFACE_FORMAT_AND_PHYSICS_GENERATION).into_owned()
}

impl<'a> BuildingExposureTables<'a> {
    pub fn new(prepared: &'a Path, manifest: &'a InputManifest) -> Self {
        Self {
            prepared,
            manifest,
            squares: HashMap::new(),
        }
    }

    /// Read every square these buildings belong to that is not loaded yet.
    pub fn load(&mut self, buildings: impl IntoIterator<Item = FootprintKey>) -> Result<()> {
        for building in buildings {
            let square = building.square();
            if !self.squares.contains_key(&(square.x, square.y)) {
                let rows = read_square(self.prepared, self.manifest, square)?;
                self.squares.insert((square.x, square.y), rows);
            }
        }
        Ok(())
    }

    /// The stored powers of one loaded building; `None` = no exposed façade.
    pub fn powers(&self, building: FootprintKey) -> Result<Option<&[f32]>> {
        let rows = self
            .squares
            .get(&(building.square_x, building.square_y))
            .context("building exposure square was not loaded")?;
        match rows.get(&building.id) {
            Some(powers) => Ok(powers.as_deref()),
            None => bail!(
                "building exposure missing in this release: footprint {} of {}",
                building.id,
                grid::square_name(building.square())
            ),
        }
    }
}

fn read_square(prepared: &Path, manifest: &InputManifest, square: Square) -> Result<SquareExposures> {
    let relative = format!("z9/{}/{}/{FACADE_EXPOSURE_ARROW}", square.x, square.y);
    let (bytes, _) = manifest.read_arrow(prepared, &relative)?.with_context(|| {
        format!("building exposure missing in this release: no {relative} (run the façade-exposure stage)")
    })?;
    let structures = format!("z9/{}/{}/structures.arrow", square.x, square.y);
    let structures_digest = manifest
        .pinned_digest(&structures)?
        .with_context(|| format!("{relative} exists without {structures}"))?;
    let reader = FileReader::try_new(Cursor::new(bytes), None)?;
    let schema = reader.schema();
    contract::validate_schema(&schema).map_err(anyhow::Error::msg)?;
    let metadata = schema.metadata();
    ensure!(
        contract::layer_order(&schema).map_err(anyhow::Error::msg)?.join(",") == source_layer_order(),
        "{relative}: layer order differs from this painter's"
    );
    ensure!(
        metadata.get(PHYSICS_GENERATION_KEY) == Some(&physics_generation_stamp()),
        "{relative}: computed by another physics generation"
    );
    ensure!(
        metadata.get(STRUCTURES_SHA256_KEY) == Some(&hex_digest(structures_digest)),
        "{relative}: computed from another {structures}"
    );
    let mut rows = HashMap::new();
    for batch in reader {
        for row in contract::rows(&batch?).map_err(anyhow::Error::msg)? {
            let powers = row.chosen.map(|chosen| chosen.layer_period_power);
            ensure!(
                powers
                    .as_ref()
                    .is_none_or(|powers| powers.iter().all(|p| p.is_finite() && *p >= 0.0)),
                "{relative}: footprint {} has invalid powers",
                row.footprint_id
            );
            ensure!(
                rows.insert(row.footprint_id, powers).is_none(),
                "{relative}: footprint {} appears twice",
                row.footprint_id
            );
        }
    }
    Ok(rows)
}
