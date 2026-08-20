//! Sampled CPU field driver for the pre-registered H0 V3 arms.

use noise_compute::compute::element::LineLayer;
use noise_compute::propagation::h0_streaming_reduction::H0Candidate;
use noise_compute::propagation::h0_v3::{
    build_h0_judge_nodes, reduce_h0_v3, H0V3Theta, JUDGE_COARSE_EPSILON_DEGREES,
    JUDGE_FINE_EPSILON_DEGREES,
};
use noise_compute::propagation::obstacle_index::ObstacleSet;
use noise_compute::types::{Barrier, NUM_BANDS};
use raster_reader::fused_tile_z13::{FusedTileZ13, TILE_PX};
use rayon::prelude::*;

use crate::accumulator::{TileAccumulator, NUM_PERIODS};
use crate::h0_pair_reference::{
    evaluate_h0_v3_pair, h0_pair_piece_in_receiver_frame, H0PairReferenceError, H0V3PairArm,
    H0V3PairReference,
};
use crate::scatter_band::{PixelGeometry, PreparedSource};
use crate::scatter_line::LineGeometry;
use crate::source_line::LineRow;

/// One exact CPU field over a frozen sampled receiver set. Period powers use
/// the production `f32` accumulation order; unrounded `f64` band powers are
/// diagnostic only. `receiver_indices` is the ascending key list the field was
/// rendered over, parallel to the two power vectors.
#[derive(Debug)]
pub struct H0V3TileField {
    pub receiver_indices: Vec<u32>,
    pub period_power_f32: Vec<[f32; NUM_PERIODS]>,
    pub period_band_power: Vec<[[f64; NUM_BANDS]; NUM_PERIODS]>,
    pub evaluated_pair_count: u64,
    pub evaluated_node_count: u64,
    pub admitted_node_count: u64,
    pub maximum_distinct_hint_records: usize,
    pub maximum_unique_u_hints: usize,
    pub maximum_logical_hint_storage_bytes: usize,
}

/// Source-store reconstruction and physical-pair failures remain distinct.
#[derive(Debug)]
pub enum H0V3TileError<E> {
    CandidateStore(E),
    Pair(H0PairReferenceError),
    SourceOrder,
}

/// Geometry-only census that must pass before a full unmasked judge field may
/// run. It prices the judge from realised nodes without executing path rays.
#[derive(Debug, Clone, Copy, Default)]
pub struct H0V3JudgeCensus {
    pub evaluated_pair_count: u64,
    pub raw_candidate_visit_count: u64,
    pub maximum_raw_candidates_per_pair: u64,
    pub h0_three_degree_node_count: u64,
    pub h0_three_degree_admitted_node_count: u64,
    pub coarse_judge_node_count: u64,
    pub fine_judge_node_count: u64,
    pub maximum_distinct_hint_records: usize,
    pub maximum_unique_u_hints: usize,
    pub maximum_logical_hint_storage_bytes: usize,
}

/// Render the current exact CPU skyline model as the separately scored stock
/// baseline. This is the complete staged model delta, not an isolated P2b
/// magnitude; the direct predicate fixture owns P2b attribution. The stock
/// accumulator exposes period powers but retains no per-band diagnostics.
///
/// The production painter fills the whole tile in one pass, so this arm paints
/// everything and then emits only `receivers`. Receivers are independent, so
/// the emitted values are identical to a hypothetical sampled-only paint —
/// unlike the judge and H0 arms, sampling buys this arm no wall time.
pub fn evaluate_h0_v3_stock_tile(
    tile: &FusedTileZ13,
    lines: &[LineRow],
    barriers: &[Barrier],
    obstacles: Option<&ObstacleSet>,
    receivers: &[u32],
) -> H0V3TileField {
    let mut accumulator = TileAccumulator::new();
    let stats = crate::scatter_line::scatter_tile_with_cfg(
        tile,
        lines,
        barriers,
        obstacles,
        &mut accumulator,
        None,
    );
    let painted: Vec<[f32; NUM_PERIODS]> = accumulator
        .energy
        .chunks_exact(NUM_PERIODS)
        .map(|periods| [periods[0], periods[1], periods[2]])
        .collect();
    let period_power_f32 = receivers
        .iter()
        .map(|&index| painted[index as usize])
        .collect();
    H0V3TileField {
        receiver_indices: receivers.to_vec(),
        period_power_f32,
        period_band_power: vec![[[0.0; NUM_BANDS]; NUM_PERIODS]; receivers.len()],
        evaluated_pair_count: stats.pairs,
        evaluated_node_count: 0,
        admitted_node_count: 0,
        maximum_distinct_hint_records: 0,
        maximum_unique_u_hints: 0,
        maximum_logical_hint_storage_bytes: 0,
    }
}

impl H0V3JudgeCensus {
    #[must_use]
    pub fn realised_judge_node_ratio(self) -> f64 {
        self.fine_judge_node_count as f64 / self.h0_three_degree_node_count as f64
    }

    fn merge(&mut self, other: Self) {
        self.evaluated_pair_count += other.evaluated_pair_count;
        self.raw_candidate_visit_count += other.raw_candidate_visit_count;
        self.maximum_raw_candidates_per_pair = self
            .maximum_raw_candidates_per_pair
            .max(other.maximum_raw_candidates_per_pair);
        self.h0_three_degree_node_count += other.h0_three_degree_node_count;
        self.h0_three_degree_admitted_node_count += other.h0_three_degree_admitted_node_count;
        self.coarse_judge_node_count += other.coarse_judge_node_count;
        self.fine_judge_node_count += other.fine_judge_node_count;
        self.maximum_distinct_hint_records = self
            .maximum_distinct_hint_records
            .max(other.maximum_distinct_hint_records);
        self.maximum_unique_u_hints = self
            .maximum_unique_u_hints
            .max(other.maximum_unique_u_hints);
        self.maximum_logical_hint_storage_bytes = self
            .maximum_logical_hint_storage_bytes
            .max(other.maximum_logical_hint_storage_bytes);
    }
}

/// Census every live pair over the frozen sampled receiver set, without running
/// path physics. It is handed the identical key list as the arms, so the judge
/// node counts it prices are exactly the nodes the judge arms will evaluate —
/// the budget gate compares measured work against measured work, never an
/// extrapolation from a full-resolution census.
pub fn census_h0_v3_judge_tile<F, E>(
    tile: &FusedTileZ13,
    lines: &[LineRow],
    layer: LineLayer,
    receivers: &[u32],
    candidates_for_pair: &F,
) -> Result<H0V3JudgeCensus, H0V3TileError<E>>
where
    F: Fn(&LineRow, f64, f64) -> Result<Vec<H0Candidate>, E> + Sync,
    E: Send,
{
    let geometry = LineGeometry { lines };
    let mut prepared = Vec::new();
    geometry.prepare(tile, &mut prepared);
    let rows: Result<Vec<_>, H0V3TileError<E>> = receivers
        .par_iter()
        .map(|&receiver_key| {
            let receiver_index = receiver_key as usize;
            let pixel_y = receiver_index / TILE_PX;
            let pixel_x = receiver_index % TILE_PX;
            let receiver_latitude = tile.rx_lat[pixel_y];
            let mut row = H0V3JudgeCensus::default();
            {
                let receiver_longitude = tile.rx_lon[pixel_x];
                let receiver_altitude_m = tile.rx_alt_m[receiver_index] as f64;
                let receiver_reflection_db = tile.rx_refl_db[receiver_index] as f64;
                for source in &prepared {
                    let (py0, py1, px0, px1) = source.reach_box();
                    if pixel_y < py0 || pixel_y > py1 || pixel_x < px0 || pixel_x > px1 {
                        continue;
                    }
                    if geometry
                        .pixel(
                            source,
                            tile,
                            receiver_latitude,
                            receiver_longitude,
                            receiver_altitude_m,
                            receiver_reflection_db,
                        )
                        .is_none()
                    {
                        continue;
                    }
                    let piece =
                        h0_pair_piece_in_receiver_frame(tile, source.line, pixel_y, pixel_x)
                            .map_err(H0V3TileError::Pair)?;
                    let candidates =
                        candidates_for_pair(source.line, receiver_latitude, receiver_longitude)
                            .map_err(H0V3TileError::CandidateStore)?;
                    let raw_count = candidates.len() as u64;
                    let coarse_judge = build_h0_judge_nodes(
                        piece,
                        layer,
                        JUDGE_COARSE_EPSILON_DEGREES,
                        candidates.iter().copied(),
                    )
                    .map_err(H0PairReferenceError::from)
                    .map_err(H0V3TileError::Pair)?;
                    let fine_judge = build_h0_judge_nodes(
                        piece,
                        layer,
                        JUDGE_FINE_EPSILON_DEGREES,
                        candidates.iter().copied(),
                    )
                    .map_err(H0PairReferenceError::from)
                    .map_err(H0V3TileError::Pair)?;
                    let h0 = reduce_h0_v3(piece, layer, H0V3Theta::Degrees3, candidates)
                        .map_err(H0PairReferenceError::from)
                        .map_err(H0V3TileError::Pair)?;
                    row.evaluated_pair_count += 1;
                    row.raw_candidate_visit_count += raw_count;
                    row.maximum_raw_candidates_per_pair =
                        row.maximum_raw_candidates_per_pair.max(raw_count);
                    row.h0_three_degree_node_count += h0.nodes().len() as u64;
                    row.h0_three_degree_admitted_node_count += h0.admitted_node_count() as u64;
                    row.coarse_judge_node_count += coarse_judge.nodes.len() as u64;
                    row.fine_judge_node_count += fine_judge.nodes.len() as u64;
                    row.maximum_distinct_hint_records = row
                        .maximum_distinct_hint_records
                        .max(coarse_judge.distinct_hint_records)
                        .max(fine_judge.distinct_hint_records);
                    row.maximum_unique_u_hints = row
                        .maximum_unique_u_hints
                        .max(coarse_judge.unique_u_hints)
                        .max(fine_judge.unique_u_hints);
                    row.maximum_logical_hint_storage_bytes = row
                        .maximum_logical_hint_storage_bytes
                        .max(coarse_judge.logical_hint_storage_bytes)
                        .max(fine_judge.logical_hint_storage_bytes);
                }
            }
            Ok(row)
        })
        .collect();
    let mut census = H0V3JudgeCensus::default();
    for row in rows? {
        census.merge(row);
    }
    Ok(census)
}

/// Render one pre-registered V3 arm over the frozen sampled receiver set.
/// Receivers may run in parallel, but each receiver consumes source rows in
/// canonical loader order, so no value depends on the pool size.
/// Its period accumulator mirrors `TileAccumulator::add_energy_at`; the f64
/// band accumulator exists only for the pre-registered diagnostic report.
///
/// `receivers` must be the case's frozen sampled key list, ascending. Every
/// arm of a case is handed the identical list — that is what makes the arms
/// comparable, and the field writer re-checks it on the way to disk.
#[allow(clippy::too_many_arguments)]
pub fn evaluate_h0_v3_tile<F, E>(
    tile: &FusedTileZ13,
    lines: &[LineRow],
    barriers: &[Barrier],
    obstacles: Option<&ObstacleSet>,
    layer: LineLayer,
    arm: H0V3PairArm,
    receivers: &[u32],
    candidates_for_pair: &F,
) -> Result<H0V3TileField, H0V3TileError<E>>
where
    F: Fn(&LineRow, f64, f64) -> Result<Vec<H0Candidate>, E> + Sync,
    E: Send,
{
    let geometry = LineGeometry { lines };
    let mut prepared = Vec::new();
    geometry.prepare(tile, &mut prepared);
    let cells: Result<Vec<_>, H0V3TileError<E>> = receivers
        .par_iter()
        .map(|&receiver_key| {
            let receiver_index = receiver_key as usize;
            let pixel_y = receiver_index / TILE_PX;
            let pixel_x = receiver_index % TILE_PX;
            let receiver_latitude = tile.rx_lat[pixel_y];
            let receiver_longitude = tile.rx_lon[pixel_x];
            let receiver_altitude_m = tile.rx_alt_m[receiver_index] as f64;
            let receiver_reflection_db = tile.rx_refl_db[receiver_index] as f64;
            let mut counters = RowCounters::default();
            let mut pixel_field = ProductionOrderPixelField::default();
            for (source_ordinal, source) in prepared.iter().enumerate() {
                let (py0, py1, px0, px1) = source.reach_box();
                if pixel_y < py0 || pixel_y > py1 || pixel_x < px0 || pixel_x > px1 {
                    continue;
                }
                if geometry
                    .pixel(
                        source,
                        tile,
                        receiver_latitude,
                        receiver_longitude,
                        receiver_altitude_m,
                        receiver_reflection_db,
                    )
                    .is_none()
                {
                    continue;
                }
                let candidates =
                    candidates_for_pair(source.line, receiver_latitude, receiver_longitude)
                        .map_err(H0V3TileError::CandidateStore)?;
                let pair = evaluate_h0_v3_pair(
                    tile,
                    source.line,
                    barriers,
                    obstacles,
                    pixel_y,
                    pixel_x,
                    layer,
                    candidates,
                    None,
                    arm,
                )
                .map_err(H0V3TileError::Pair)?;
                counters.add_pair(&pair);
                pixel_field
                    .add_source(
                        source_ordinal,
                        pair.period_power_f32,
                        pair.period_band_power,
                    )
                    .map_err(|()| H0V3TileError::SourceOrder)?;
            }
            Ok((pixel_field.into_field(), counters))
        })
        .collect();
    Ok(assemble_field(receivers, cells?))
}

#[derive(Default)]
struct ProductionOrderPixelField {
    period_power_f32: [f32; NUM_PERIODS],
    period_band_power: [[f64; NUM_BANDS]; NUM_PERIODS],
    last_source_ordinal: Option<usize>,
}

impl ProductionOrderPixelField {
    fn add_source(
        &mut self,
        source_ordinal: usize,
        period_power_f32: [f32; NUM_PERIODS],
        period_band_power: [[f64; NUM_BANDS]; NUM_PERIODS],
    ) -> Result<(), ()> {
        if self
            .last_source_ordinal
            .is_some_and(|previous| source_ordinal <= previous)
        {
            return Err(());
        }
        self.last_source_ordinal = Some(source_ordinal);
        for period in 0..NUM_PERIODS {
            self.period_power_f32[period] += period_power_f32[period];
            for (field_band, pair_band) in self.period_band_power[period]
                .iter_mut()
                .zip(period_band_power[period])
            {
                *field_band += pair_band;
            }
        }
        Ok(())
    }

    fn into_field(self) -> PixelField {
        (self.period_power_f32, self.period_band_power)
    }
}

#[derive(Default)]
struct RowCounters {
    pairs: u64,
    nodes: u64,
    admitted: u64,
    maximum_hint_records: usize,
    maximum_u_hints: usize,
    maximum_hint_storage_bytes: usize,
}

impl RowCounters {
    fn add_pair(&mut self, pair: &H0V3PairReference) {
        self.pairs += 1;
        self.nodes += pair.node_count as u64;
        self.admitted += pair.admitted_node_count as u64;
        self.maximum_hint_records = self.maximum_hint_records.max(pair.distinct_hint_records);
        self.maximum_u_hints = self.maximum_u_hints.max(pair.unique_u_hints);
        self.maximum_hint_storage_bytes = self
            .maximum_hint_storage_bytes
            .max(pair.logical_hint_storage_bytes);
    }

    fn merge(&mut self, other: Self) {
        self.pairs += other.pairs;
        self.nodes += other.nodes;
        self.admitted += other.admitted;
        self.maximum_hint_records = self.maximum_hint_records.max(other.maximum_hint_records);
        self.maximum_u_hints = self.maximum_u_hints.max(other.maximum_u_hints);
        self.maximum_hint_storage_bytes = self
            .maximum_hint_storage_bytes
            .max(other.maximum_hint_storage_bytes);
    }
}

type PixelField = ([f32; NUM_PERIODS], [[f64; NUM_BANDS]; NUM_PERIODS]);

fn assemble_field(receivers: &[u32], cells: Vec<(PixelField, RowCounters)>) -> H0V3TileField {
    let mut period_power_f32 = Vec::with_capacity(cells.len());
    let mut period_band_power = Vec::with_capacity(cells.len());
    let mut counters = RowCounters::default();
    for ((period_power, period_band), cell_counters) in cells {
        period_power_f32.push(period_power);
        period_band_power.push(period_band);
        counters.merge(cell_counters);
    }
    H0V3TileField {
        receiver_indices: receivers.to_vec(),
        period_power_f32,
        period_band_power,
        evaluated_pair_count: counters.pairs,
        evaluated_node_count: counters.nodes,
        admitted_node_count: counters.admitted,
        maximum_distinct_hint_records: counters.maximum_hint_records,
        maximum_unique_u_hints: counters.maximum_u_hints,
        maximum_logical_hint_storage_bytes: counters.maximum_hint_storage_bytes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::h0_pair_reference::H0V3PairArm;
    use raster_reader::RealRasters;
    use std::path::Path;

    #[test]
    fn empty_source_tile_is_a_complete_absent_field() {
        let rasters = RealRasters::new(Path::new("/nonexistent-h0-v3-empty-field"));
        let tile = FusedTileZ13::build_receiver_altitude_only(12, 2211, 1386, &rasters);
        let provider = |_: &LineRow, _: f64, _: f64| -> Result<Vec<H0Candidate>, ()> {
            panic!("an empty source tile must not request candidates")
        };
        let receivers = crate::h0_v3_sampler::h0_v3_sampled_receivers(0);
        let field = evaluate_h0_v3_tile(
            &tile,
            &[],
            &[],
            None,
            LineLayer::Road,
            H0V3PairArm::JudgeFine,
            &receivers,
            &provider,
        )
        .unwrap();
        assert_eq!(field.receiver_indices, receivers);
        assert_eq!(field.period_power_f32.len(), receivers.len());
        assert!(field
            .period_power_f32
            .iter()
            .all(|pixel| *pixel == [0.0; NUM_PERIODS]));
        assert_eq!(field.period_band_power.len(), receivers.len());
        assert!(field
            .period_band_power
            .iter()
            .all(|pixel| *pixel == [[0.0; NUM_BANDS]; NUM_PERIODS]));
        assert_eq!(field.evaluated_pair_count, 0);
        assert_eq!(field.evaluated_node_count, 0);
    }

    #[test]
    fn period_field_pins_f32_source_order_before_diagnostic_band_sum() {
        let powers = [16_777_216.0_f32, 1.0, 1.0];
        let mut field = ProductionOrderPixelField::default();
        for (source_ordinal, power) in powers.into_iter().enumerate() {
            let mut bands = [[0.0; NUM_BANDS]; NUM_PERIODS];
            bands[0][0] = f64::from(power);
            field
                .add_source(source_ordinal, [power, 0.0, 0.0], bands)
                .unwrap();
        }
        let (period_power, diagnostic_bands) = field.into_field();
        let expected_source_order = ((powers[0] + powers[1]) + powers[2]).to_bits();
        let diagnostic_resummation = diagnostic_bands[0].iter().sum::<f64>() as f32;
        assert_eq!(period_power[0].to_bits(), expected_source_order);
        assert_ne!(period_power[0].to_bits(), diagnostic_resummation.to_bits());

        let mut reversed = ProductionOrderPixelField::default();
        let empty_bands = [[0.0; NUM_BANDS]; NUM_PERIODS];
        reversed
            .add_source(2, [powers[2], 0.0, 0.0], empty_bands)
            .unwrap();
        assert_eq!(
            reversed.add_source(1, [powers[1], 0.0, 0.0], empty_bands),
            Err(())
        );
    }
}
