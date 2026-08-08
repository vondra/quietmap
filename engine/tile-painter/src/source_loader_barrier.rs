//! Read `barriers.arrow` for a set of H3 R4 hex cells — noise walls consumed
//! by every surface scatter kernel (B8/C9). Only ~1.5% of world R4 hexes carry
//! a barrier file; missing files load as an empty set at zero cost.
//!
//! Two consumers, one shape:
//! * CPU kernels take the popup's vector form — [`Barrier`] segments (both
//!   endpoints, so the kernel can intersect the ray with the wall exactly)
//!   whose `dist_m` early-break contract is documented on the type. The
//!   per-tile slice is prepared by [`BarrierData::for_tile`] (filter +
//!   conservative lower-bound distance + ascending sort), mirroring
//!   source-reader `hex_store::query_barriers_from_batches` semantics
//!   (endpoints + height, default 3.0) so popup and heatmap screen the
//!   identical wall set.
//! * The GPU runner takes the SAME vector form behind the `QM_GPU_BARRIERS`
//!   gate (engine default ON since 2026-08-02): the per-tile [`BarrierData::for_tile`] slice is uploaded
//!   and the scatter kernel runs the identical ray×segment intersection
//!   (`ray_path_bands` in scatter.cu). The raster burn was the rejected alternative —
//!   it failed the W2 gate (3.7–13.8 dB under-screening in wall shadow;
//!   decision-record test in tests/barrier_screening.rs); the burn helper
//!   (`FusedGrid::burn_building_max`) survives only as that gate-test apparatus.
//!   With the gate OFF the GPU lane uploads no barriers (nbarr==0, kernel no-op)
//!   and barrier chunks demote to the CPU builders (cluster-build-chunk.sh, C9).

use std::path::Path;

use anyhow::Result;
use arrow::array::{Float32Array, Float64Array, Int64Array};
use arrow::record_batch::RecordBatch;
use noise_compute::propagation::geo::flat_dist;
use noise_compute::types::{Barrier, BARRIER_SEGMENT_MAX_HALF_LEN_M};
use raster_reader::fused_tile_z13::TileBbox;

use crate::source_line::opt;

/// Extract writes 3.0 m for untagged walls; mirror the read-side default of
/// `query_barriers_from_batches` for arrows missing the column entirely.
const DEFAULT_BARRIER_HEIGHT_M: f32 = 3.0;

/// One barrier microsegment as stored in `barriers.arrow`.
pub struct BarrierSeg {
    pub osm_id: i64,
    pub start_lat: f64,
    pub start_lon: f64,
    pub end_lat: f64,
    pub end_lon: f64,
    pub height_m: f32,
}

/// A region's barrier microsegments (centre R4 + `grid_disk(1)` ring — the
/// same span the source loaders read, which covers every source→receiver
/// path of the region's tiles).
pub struct BarrierData {
    segs: Vec<BarrierSeg>,
}

impl BarrierData {
    /// Load every `barriers.arrow` row across `r4_hexes`; missing files are
    /// skipped (98.5% of hexes have no barriers).
    pub fn load_for_r4s(h3r4_dir: &Path, r4_hexes: &[u64]) -> Result<Self> {
        let mut segs = Vec::new();
        for &r4 in r4_hexes {
            crate::schema_check::read_surface_arrow_for_r4(
                h3r4_dir,
                r4,
                "barriers.arrow",
                |batch| absorb_batch(batch, &mut segs),
            )?;
        }
        Ok(Self { segs })
    }

    /// Test/synthetic construction.
    pub fn from_segments(segs: Vec<BarrierSeg>) -> Self {
        Self { segs }
    }

    pub fn is_empty(&self) -> bool {
        self.segs.is_empty()
    }

    pub fn len(&self) -> usize {
        self.segs.len()
    }

    /// Raw segment geometry — the rejected raster-burn input, kept only for the
    /// `barrier_screening.rs` decision-record test (the GPU runner screens the
    /// VECTOR [`for_tile`] slice behind `QM_GPU_BARRIERS`, never a burn).
    pub fn segments(&self) -> &[BarrierSeg] {
        &self.segs
    }

    /// Prepare one tile's barrier slice for the scatter kernels.
    ///
    /// `reach_m` is the build's max source reach (the shared halo radius): a
    /// barrier only matters where it CROSSES some source→receiver path, every
    /// point of which lies within `reach_m` of a receiver pixel. A crossing
    /// segment's midpoint sits at most a half-segment
    /// (`BARRIER_SEGMENT_MAX_HALF_LEN_M`) further out, so midpoints beyond
    /// `reach_m + tile_half_diagonal + half_segment` can never cross any such
    /// path (same horizon the popup uses with its exact 10 km per-receiver
    /// load).
    ///
    /// `dist_m = max(0, d(midpoint, tile_centre) − tile_half_diagonal)` — a
    /// lower bound on the midpoint distance to EVERY pixel in the tile
    /// (triangle inequality), sorted ascending: exactly the `types::Barrier`
    /// contract that keeps `screening_attenuation`'s early-break conservative
    /// for every pixel while letting short paths stop at the first far wall.
    /// The endpoints ride along unchanged — the kernel intersects THEM with the
    /// ray; the midpoint is only this filter's proximity key.
    pub fn for_tile(&self, bbox: &TileBbox, reach_m: f64) -> Vec<Barrier> {
        if self.segs.is_empty() {
            return Vec::new();
        }
        let c_lat = (bbox.north_lat + bbox.south_lat) * 0.5;
        let c_lon = (bbox.west_lon + bbox.east_lon) * 0.5;
        // N/S corner pairs are lon-symmetric under flat_dist; max of the two
        // latitudes covers all four corners.
        let half_diag = flat_dist(c_lat, c_lon, bbox.north_lat, bbox.west_lon).max(flat_dist(
            c_lat,
            c_lon,
            bbox.south_lat,
            bbox.west_lon,
        ));
        let mut out: Vec<Barrier> = self
            .segs
            .iter()
            .filter_map(|s| {
                let mid_lat = (s.start_lat + s.end_lat) * 0.5;
                let mid_lon = (s.start_lon + s.end_lon) * 0.5;
                let d_centre = flat_dist(c_lat, c_lon, mid_lat, mid_lon);
                // flat_dist scales lon by the PAIR midpoint latitude, so the triangle
                // inequality is only approximate (Codex C9: sub-metre violations at low
                // zoom). DIST_SLACK_M keeps both the filter and dist_m strictly
                // conservative — slack only weakens an early-break optimisation.
                const DIST_SLACK_M: f64 = 50.0;
                if d_centre > reach_m + half_diag + DIST_SLACK_M + BARRIER_SEGMENT_MAX_HALF_LEN_M {
                    return None;
                }
                Some(Barrier {
                    osm_id: s.osm_id,
                    height_m: s.height_m,
                    start_lat: s.start_lat,
                    start_lon: s.start_lon,
                    end_lat: s.end_lat,
                    end_lon: s.end_lon,
                    dist_m: (d_centre - half_diag - DIST_SLACK_M).max(0.0),
                })
            })
            .collect();
        out.sort_unstable_by(|a, b| {
            a.dist_m
                .partial_cmp(&b.dist_m)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        out
    }
}

fn absorb_batch(batch: &RecordBatch, out: &mut Vec<BarrierSeg>) -> Result<()> {
    let n = batch.num_rows();
    if n == 0 {
        return Ok(());
    }
    // Same required set as `query_barriers_from_batches`: id + geometry;
    // height defaults when the column is absent.
    let (Some(osm_id), Some(slat), Some(slon), Some(elat), Some(elon)) = (
        opt::<Int64Array>(batch, "osm_id"),
        opt::<Float64Array>(batch, "start_lat"),
        opt::<Float64Array>(batch, "start_lon"),
        opt::<Float64Array>(batch, "end_lat"),
        opt::<Float64Array>(batch, "end_lon"),
    ) else {
        return Ok(());
    };
    let height = opt::<Float32Array>(batch, "height");
    for i in 0..n {
        out.push(BarrierSeg {
            osm_id: osm_id.value(i),
            start_lat: slat.value(i),
            start_lon: slon.value(i),
            end_lat: elat.value(i),
            end_lon: elon.value(i),
            height_m: height
                .map(|a| a.value(i))
                .unwrap_or(DEFAULT_BARRIER_HEIGHT_M),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::ArrayRef;
    use arrow::datatypes::{DataType, Field, Schema};
    use std::sync::Arc;

    fn barrier_batch(with_height: bool) -> RecordBatch {
        let mut cols: Vec<(Field, ArrayRef)> = vec![
            (
                Field::new("osm_id", DataType::Int64, false),
                Arc::new(Int64Array::from(vec![7i64])),
            ),
            (
                Field::new("start_lat", DataType::Float64, false),
                Arc::new(Float64Array::from(vec![50.0])),
            ),
            (
                Field::new("start_lon", DataType::Float64, false),
                Arc::new(Float64Array::from(vec![14.0])),
            ),
            (
                Field::new("end_lat", DataType::Float64, false),
                Arc::new(Float64Array::from(vec![50.001])),
            ),
            (
                Field::new("end_lon", DataType::Float64, false),
                Arc::new(Float64Array::from(vec![14.001])),
            ),
        ];
        if with_height {
            cols.push((
                Field::new("height", DataType::Float32, false),
                Arc::new(Float32Array::from(vec![4.5f32])),
            ));
        }
        let fields: Vec<Field> = cols.iter().map(|(f, _)| f.clone()).collect();
        let arrs: Vec<ArrayRef> = cols.into_iter().map(|(_, a)| a).collect();
        RecordBatch::try_new(Arc::new(Schema::new(fields)), arrs).unwrap()
    }

    #[test]
    fn absorbs_height_and_defaults_missing_column() {
        let mut segs = Vec::new();
        absorb_batch(&barrier_batch(true), &mut segs).unwrap();
        absorb_batch(&barrier_batch(false), &mut segs).unwrap();
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].height_m, 4.5);
        assert_eq!(
            segs[1].height_m, DEFAULT_BARRIER_HEIGHT_M,
            "missing column → 3.0 default"
        );
    }

    /// `for_tile` must produce a sorted slice whose `dist_m` lower-bounds the
    /// true midpoint distance from EVERY pixel of the tile — the
    /// `types::Barrier` early-break contract.
    #[test]
    fn for_tile_dist_is_conservative_and_sorted() {
        let bbox = TileBbox::from_xyz(12, 2211, 1386);
        let c_lat = (bbox.north_lat + bbox.south_lat) * 0.5;
        let mk = |dlon_deg: f64| BarrierSeg {
            osm_id: 1,
            start_lat: c_lat,
            start_lon: bbox.east_lon + dlon_deg,
            end_lat: c_lat,
            end_lon: bbox.east_lon + dlon_deg,
            height_m: 3.0,
        };
        // Inside the tile, ~2 km east, ~8 km east, far beyond reach (~30 km).
        let data = BarrierData::from_segments(vec![
            mk(0.112), // ~8 km
            mk(-0.02), // inside tile
            mk(0.028), // ~2 km
            mk(0.42),  // ~30 km → filtered at 10 km reach
        ]);
        let out = data.for_tile(&bbox, 10_000.0);
        assert_eq!(out.len(), 3, "beyond reach+half_diag filtered");
        assert!(
            out.windows(2).all(|w| w[0].dist_m <= w[1].dist_m),
            "ascending"
        );
        assert_eq!(
            out[0].dist_m, 0.0,
            "inside the tile → lower bound clamps to 0"
        );

        // Lower bound vs the true distance from each tile corner (the worst
        // receivers a pixel can approach).
        for b in &out {
            let (mid_lat, mid_lon) = b.midpoint();
            for (lat, lon) in [
                (bbox.north_lat, bbox.west_lon),
                (bbox.north_lat, bbox.east_lon),
                (bbox.south_lat, bbox.west_lon),
                (bbox.south_lat, bbox.east_lon),
            ] {
                let true_d = flat_dist(lat, lon, mid_lat, mid_lon);
                assert!(
                    b.dist_m <= true_d + 1e-6,
                    "dist_m {} not a lower bound of corner distance {}",
                    b.dist_m,
                    true_d
                );
            }
        }
    }

    #[test]
    fn empty_data_yields_empty_slice() {
        let data = BarrierData::from_segments(Vec::new());
        let bbox = TileBbox::from_xyz(12, 2211, 1386);
        assert!(data.is_empty());
        assert_eq!(data.len(), 0);
        assert!(data.for_tile(&bbox, 10_000.0).is_empty());
    }
}
