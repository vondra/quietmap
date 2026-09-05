//! What one streamed cell cost, and the `done` line's trailing statistics.
//!
//! Every number is measured on the cell it is reported with: the orchestrator
//! prices a run from its own receipts, so a figure averaged over a batch would
//! be unattributable the moment two workers split the batch.

use crate::relevant_source_tile::TilePaintMeasurement;
use crate::surface_layers::{LAYER_COUNT, LAYER_NAMES};

#[derive(Clone, Debug, Default)]
pub struct LayerMeasurement {
    pub loaded_sources: u64,
    pub tiles: u64,
    pub corner_pairs: u64,
    pub pixel_pairs: u64,
    pub relevant_source_references: u64,
    /// Relevant sources of every painted block, in paint order.
    pub block_source_counts: Vec<u32>,
    pub corner_gpu_milliseconds: f64,
    pub paint_gpu_milliseconds: f64,
    pub output_bytes: u64,
}

impl LayerMeasurement {
    pub fn add_tile(&mut self, tile: TilePaintMeasurement) {
        self.tiles += 1;
        self.corner_pairs += tile.corner_pairs;
        self.pixel_pairs += tile.pixel_pairs;
        self.relevant_source_references += tile.relevant_source_references;
        self.block_source_counts.extend(tile.block_source_counts);
        self.corner_gpu_milliseconds += tile.corner_gpu_milliseconds;
        self.paint_gpu_milliseconds += tile.paint_gpu_milliseconds;
    }

    /// `(min, median, p99, max)` of relevant sources per block: the block
    /// triage's own distribution, which is what explains a slow cell.
    pub fn block_source_quantiles(&self) -> (u32, u32, u32, u32) {
        let mut counts = self.block_source_counts.clone();
        if counts.is_empty() {
            return (0, 0, 0, 0);
        }
        counts.sort_unstable();
        let at = |fraction: f64| counts[((counts.len() - 1) as f64 * fraction).round() as usize];
        (counts[0], at(0.5), at(0.99), counts[counts.len() - 1])
    }

    fn statistics(&self, name: &str) -> String {
        let blocks = self.block_source_counts.len() as f64;
        let relevant_per_block = if blocks == 0.0 {
            0.0
        } else {
            self.relevant_source_references as f64 / blocks
        };
        let (minimum, median, p99, maximum) = self.block_source_quantiles();
        format!(
            "{name}.sources={} {name}.tiles={} {name}.pairs={}/{} \
             {name}.relevant_per_block={relevant_per_block:.3} \
             {name}.block_sources={minimum}/{median}/{p99}/{maximum} \
             {name}.gpu_s={:.6}/{:.6} {name}.bytes={}",
            self.loaded_sources,
            self.tiles,
            self.corner_pairs,
            self.pixel_pairs,
            self.corner_gpu_milliseconds / 1000.0,
            self.paint_gpu_milliseconds / 1000.0,
            self.output_bytes,
        )
    }
}

/// One cell's phases, in the order they are paid for. `layers` holds one entry
/// per [`LAYER_NAMES`] layer; the ones this cell did not paint stay at zero and
/// are left out of the report.
#[derive(Debug)]
pub struct CellMeasurement {
    pub painted_layers: Vec<usize>,
    pub layers: Vec<LayerMeasurement>,
    /// Source and structure loading plus the device uploads, on the
    /// cell producer thread (only the first cell's is on the wall).
    pub prepare_seconds: f64,
    /// Time the producer waited for a residency permit: the host idle on the card.
    pub permit_wait_seconds: f64,
    /// Time the painter waited for this prepared cell: the card idle on the host.
    pub card_wait_seconds: f64,
    /// Terrain halo build and facade baking per 4x4 batch, summed over the
    /// builder threads: CPU time, so a cell whose lookahead ran several
    /// batches at once reports more of it than the paint's own wall.
    pub raster_prepare_seconds: f64,
    /// Receiver and enclosure preparation per tile, serial with the card.
    pub receiver_seconds: f64,
    pub host_tile_seconds: f64,
    pub paint_seconds: f64,
    pub wall_seconds: f64,
}

impl CellMeasurement {
    pub fn new(painted_layers: Vec<usize>) -> Self {
        Self {
            painted_layers,
            layers: vec![LayerMeasurement::default(); LAYER_COUNT],
            prepare_seconds: 0.0,
            permit_wait_seconds: 0.0,
            card_wait_seconds: 0.0,
            raster_prepare_seconds: 0.0,
            receiver_seconds: 0.0,
            host_tile_seconds: 0.0,
            paint_seconds: 0.0,
            wall_seconds: 0.0,
        }
    }

    fn gpu_seconds(&self) -> f64 {
        self.layers
            .iter()
            .map(|layer| layer.corner_gpu_milliseconds + layer.paint_gpu_milliseconds)
            .sum::<f64>()
            / 1000.0
    }

    fn attempted_pairs(&self) -> u64 {
        self.layers
            .iter()
            .map(|layer| layer.corner_pairs + layer.pixel_pairs)
            .sum()
    }

    /// The trailing statistics of this cell's `done` line: the cell's phases
    /// first, then every painted layer's own figures under its layer name.
    pub fn statistics(&self, zoom: u8) -> String {
        let gpu_seconds = self.gpu_seconds();
        let attempted_pairs = self.attempted_pairs();
        let gpu_nanoseconds_per_pair = if attempted_pairs == 0 {
            0.0
        } else {
            gpu_seconds * 1.0e9 / attempted_pairs as f64
        };
        let painted: Vec<&LayerMeasurement> = self
            .painted_layers
            .iter()
            .map(|&layer| &self.layers[layer])
            .collect();
        let mut line = format!(
            "zoom={zoom} wall_s={:.6} prepare_s={:.6} paint_s={:.6} gpu_s={gpu_seconds:.6} \
             gpu_ns_per_pair={gpu_nanoseconds_per_pair:.3} raster_prepare_s={:.6} receiver_s={:.6} \
             host_tile_s={:.6} card_wait_s={:.6} permit_wait_s={:.6} tiles={} bytes={} layers={}",
            self.wall_seconds,
            self.prepare_seconds,
            self.paint_seconds,
            self.raster_prepare_seconds,
            self.receiver_seconds,
            self.host_tile_seconds,
            self.card_wait_seconds,
            self.permit_wait_seconds,
            painted.iter().map(|layer| layer.tiles).sum::<u64>(),
            painted.iter().map(|layer| layer.output_bytes).sum::<u64>(),
            self.painted_layers
                .iter()
                .map(|&layer| LAYER_NAMES[layer])
                .collect::<Vec<_>>()
                .join(","),
        );
        for &layer in &self.painted_layers {
            line.push(' ');
            line.push_str(&self.layers[layer].statistics(LAYER_NAMES[layer]));
        }
        line
    }
}
