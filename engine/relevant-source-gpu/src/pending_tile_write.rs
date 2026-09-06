//! The HM3 collapse-and-write of one painted or silent tile, handed to the
//! region's writer thread.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use tile_painter::accumulator::TileAccumulator;
use tile_painter::source_loader_structure::InteriorEstimate;
use tile_painter::wire_hm3::{
    collapse_lden_surface_u8, collapse_lden_u8, fill_area_median, write_tile, AREA_FILL_RADIUS_PX,
    NO_DATA,
};

use crate::source_frame::TILE_PIXEL_SIDE;

/// One tile of one layer waiting for the writer thread.
pub enum PendingTileWrite {
    /// The card's period energies: collapse to the Lden byte, smooth an area
    /// source's point-grid ripple into its footprint, fill enclosed pixels from
    /// their facade donors, brotli, write (the CPU surface_region order).
    Painted {
        energy: Vec<f32>,
        interior: Arc<InteriorEstimate>,
        /// Index into the runner's layer list, for the per-layer byte total.
        layer: usize,
        /// Industrial and building discretise areas into point grids that the
        /// median fill turns into solid footprints; lines are continuous already.
        area_source: bool,
        /// Airport ground ops accumulate EVENT energy over this many days; the
        /// surface layers (None) are steady power.
        event_days: Option<f64>,
        source_id: u8,
        output_path: PathBuf,
    },
    /// A tile no source of this layer reaches, written as the all-`NO_DATA`
    /// tile without the card, the rasters or the receiver lattice.
    ///
    /// Those are exactly the bytes the paint returns for it: with no candidate
    /// at any corner the partition admits no source and leaves every block's
    /// background at zero, so `paint_relevant_sources_kernel` writes zero energy
    /// at every pixel of every period; `collapse_lden_*_u8` leaves a pixel with
    /// no energy in any period at `NO_DATA`; `fill_area_median` needs two data
    /// cells in a window and finds none; and `InteriorEstimate::apply` reads a
    /// `NO_DATA` donor, whose dequantised level is `-inf`, so every enclosed
    /// pixel stays `NO_DATA` too. Held there by
    /// `a_zero_energy_paint_writes_the_silent_tile`.
    Silent {
        layer: usize,
        source_id: u8,
        output_path: PathBuf,
    },
}

impl PendingTileWrite {
    /// Index into the runner's layer list.
    pub fn layer(&self) -> usize {
        match self {
            Self::Painted { layer, .. } | Self::Silent { layer, .. } => *layer,
        }
    }

    /// Bytes written.
    pub fn write(self) -> Result<u64> {
        let (cells, source_id, output_path) = match self {
            Self::Painted {
                energy,
                interior,
                area_source,
                event_days,
                source_id,
                output_path,
                ..
            } => {
                let accumulator = TileAccumulator { energy };
                let mut cells = match event_days {
                    Some(n_days) => collapse_lden_u8(&accumulator, n_days),
                    None => collapse_lden_surface_u8(&accumulator),
                };
                if area_source {
                    fill_area_median(&mut cells, AREA_FILL_RADIUS_PX);
                }
                interior.apply(&mut cells);
                (cells, source_id, output_path)
            }
            Self::Silent {
                source_id,
                output_path,
                ..
            } => (
                vec![NO_DATA; TILE_PIXEL_SIDE * TILE_PIXEL_SIDE],
                source_id,
                output_path,
            ),
        };
        Ok(write_tile(&output_path, &cells, source_id, false)? as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source_frame::PERIOD_COUNT;
    use noise_compute::envelope::EnvelopeClass;

    /// [`PendingTileWrite::Silent`] claims to be what the paint writes for a
    /// tile no source reaches. Held here against the paint's own write path fed
    /// the zero energy the kernel leaves in that case, in the two shapes the
    /// surface layers use — an area source's median fill and a ground-ops
    /// event collapse — and with enclosed pixels present, the one place a
    /// facade donor could put a level into an otherwise empty tile.
    #[test]
    fn a_zero_energy_paint_writes_the_silent_tile() {
        let cells = TILE_PIXEL_SIDE * TILE_PIXEL_SIDE;
        let mut classes = vec![EnvelopeClass::Outdoor as u8; cells];
        for (index, class) in classes.iter_mut().enumerate().take(cells / 2) {
            *class = EnvelopeClass::Residential as u8 + (index % 5) as u8;
        }
        let interior = Arc::new(InteriorEstimate::from_classes(classes));
        let directory = tempfile::tempdir().expect("a scratch directory");
        for (shape, area_source, event_days) in [
            ("line", false, None),
            ("area", true, None),
            ("ground-ops", false, Some(365.0)),
        ] {
            let painted_path = directory.path().join(format!("{shape}-painted.bin"));
            let silent_path = directory.path().join(format!("{shape}-silent.bin"));
            PendingTileWrite::Painted {
                energy: vec![0.0; cells * PERIOD_COUNT],
                interior: Arc::clone(&interior),
                layer: 0,
                area_source,
                event_days,
                source_id: 1,
                output_path: painted_path.clone(),
            }
            .write()
            .expect("the painted tile is written");
            PendingTileWrite::Silent {
                layer: 0,
                source_id: 1,
                output_path: silent_path.clone(),
            }
            .write()
            .expect("the silent tile is written");
            assert_eq!(
                std::fs::read(&painted_path).expect("painted bytes"),
                std::fs::read(&silent_path).expect("silent bytes"),
                "{shape}: the silent tile is not what a zero-energy paint writes"
            );
        }
    }
}
