//! CPU replay of the exact flat-store walk used by the H0 pair diagnostic.

use anyhow::{anyhow, ensure, Result};
use noise_compute::constants::M_PER_DEG_LAT;
use noise_compute::propagation::h0_streaming_reduction::H0Candidate;
use noise_compute::propagation::streaming_reduction::{
    candidate_overlaps_source_span, source_frame_mlon, source_frame_vector_from_world,
    MetricVector, SourceId64,
};
use noise_compute::types::Barrier;
use tile_painter::source_line::LineRow;

use noise_gpu::ObstacleFlat;

pub(crate) type DiagnosticRecord = [u64; noise_gpu::H0_PAIR_DIAGNOSTIC_RECORD_WORDS];

#[derive(Clone, Copy)]
pub(crate) struct RawCandidate {
    pub(crate) candidate: H0Candidate,
    #[allow(dead_code)] // consumed by the GPU parity bin, not the CPU-only census
    pub(crate) record: DiagnosticRecord,
}

fn geometry<T>(
    result: Result<T, noise_compute::propagation::streaming_reduction::GeometryError>,
) -> Result<T> {
    result.map_err(|error| anyhow!("invalid diagnostic geometry: {error:?}"))
}

fn record_for(
    source0: MetricVector,
    source1: MetricVector,
    candidate: H0Candidate,
) -> DiagnosticRecord {
    let span = candidate_overlaps_source_span(
        source0,
        source1,
        candidate.endpoint0_m(),
        candidate.endpoint1_m(),
        candidate.near_f32(),
    );
    [
        candidate.source_id().bits(),
        candidate.endpoint0_m().x.to_bits(),
        candidate.endpoint0_m().y.to_bits(),
        candidate.endpoint1_m().x.to_bits(),
        candidate.endpoint1_m().y.to_bits(),
        u64::from(candidate.near_f32().to_bits())
            | (u64::from(candidate.height_f32().to_bits()) << 32),
        span as u64,
    ]
}

fn tri_row_x_span(
    receiver: MetricVector,
    start: MetricVector,
    end: MetricVector,
    y_lo: f64,
    y_hi: f64,
) -> Option<(f64, f64)> {
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    let vertices = [receiver, start, end];
    for index in 0..vertices.len() {
        let vertex = vertices[index];
        if vertex.y >= y_lo && vertex.y <= y_hi {
            lo = lo.min(vertex.x);
            hi = hi.max(vertex.x);
        }
        let next = vertices[(index + 1) % vertices.len()];
        let dy = next.y - vertex.y;
        if dy == 0.0 {
            continue;
        }
        let inv_dy = 1.0 / dy;
        for boundary_y in [y_lo, y_hi] {
            let fraction = (boundary_y - vertex.y) * inv_dy;
            if !(0.0..=1.0).contains(&fraction) {
                continue;
            }
            let x = vertex.x + fraction * (next.x - vertex.x);
            lo = lo.min(x);
            hi = hi.max(x);
        }
    }
    (hi >= lo).then_some((lo, hi))
}

/// Replay the device triangle/CSR walk over exactly the bytes later uploaded.
pub(crate) fn collect_raw_candidates(
    flat: &ObstacleFlat,
    barriers: &[Barrier],
    line: &LineRow,
    receiver_latitude: f64,
    receiver_longitude: f64,
) -> Result<Vec<RawCandidate>> {
    let source_mlon = geometry(source_frame_mlon(line.start_lat, line.end_lat))?;
    let source0 = geometry(source_frame_vector_from_world(
        line.start_lat,
        line.start_lon,
        receiver_latitude,
        receiver_longitude,
        source_mlon,
    ))?;
    let source1 = geometry(source_frame_vector_from_world(
        line.end_lat,
        line.end_lon,
        receiver_latitude,
        receiver_longitude,
        source_mlon,
    ))?;
    ensure!(flat.metas.len() == flat.n_indexes * noise_gpu::META_STRIDE);
    let mut raw = Vec::new();

    for index in 0..flat.n_indexes {
        let meta =
            &flat.metas[index * noise_gpu::META_STRIDE..(index + 1) * noise_gpu::META_STRIDE];
        let origin_latitude = meta[0];
        let origin_longitude = meta[1];
        let index_mlon = meta[2];
        let cell_m = meta[3];
        let min_x = meta[4];
        let min_y = meta[5];
        let columns = meta[6] as i32;
        let rows = meta[7] as i32;
        let starts_offset = meta[8] as usize;
        let refs_offset = meta[9] as usize;
        let edges_offset = meta[10] as usize;
        ensure!(
            index_mlon.is_finite()
                && index_mlon > 0.0
                && cell_m.is_finite()
                && cell_m > 0.0
                && columns > 0
                && rows > 0,
            "invalid flat obstacle index {index}"
        );
        let receiver_local = MetricVector::new(
            (receiver_longitude - origin_longitude) * index_mlon,
            (receiver_latitude - origin_latitude) * M_PER_DEG_LAT,
        );
        let start_local = MetricVector::new(
            (line.start_lon - origin_longitude) * index_mlon,
            (line.start_lat - origin_latitude) * M_PER_DEG_LAT,
        );
        let end_local = MetricVector::new(
            (line.end_lon - origin_longitude) * index_mlon,
            (line.end_lat - origin_latitude) * M_PER_DEG_LAT,
        );
        let inv_cell = 1.0 / cell_m;
        let cx0d =
            ((receiver_local.x.min(start_local.x).min(end_local.x) - min_x) * inv_cell).floor();
        let cx1d =
            ((receiver_local.x.max(start_local.x).max(end_local.x) - min_x) * inv_cell).floor();
        let cy0d =
            ((receiver_local.y.min(start_local.y).min(end_local.y) - min_y) * inv_cell).floor();
        let cy1d =
            ((receiver_local.y.max(start_local.y).max(end_local.y) - min_y) * inv_cell).floor();
        if cx1d < 0.0 || cx0d > f64::from(columns - 1) || cy1d < 0.0 || cy0d > f64::from(rows - 1) {
            continue;
        }
        let cx0 = cx0d.max(0.0) as i32;
        let cx1 = cx1d.min(f64::from(columns - 1)) as i32;
        let cy0 = cy0d.max(0.0) as i32;
        let cy1 = cy1d.min(f64::from(rows - 1)) as i32;
        for cy in cy0..=cy1 {
            let y_lo = min_y + f64::from(cy) * cell_m;
            let Some((x_lo, x_hi)) =
                tri_row_x_span(receiver_local, start_local, end_local, y_lo, y_lo + cell_m)
            else {
                continue;
            };
            let row_cx0 = ((x_lo - min_x) * inv_cell).floor().max(f64::from(cx0)) as i32;
            let row_cx1 = ((x_hi - min_x) * inv_cell).floor().min(f64::from(cx1)) as i32;
            for cx in row_cx0..=row_cx1 {
                let cell = (cy as usize) * (columns as usize) + cx as usize;
                let first = flat.starts[starts_offset + cell] as usize;
                let last = flat.starts[starts_offset + cell + 1] as usize;
                for &edge_ref in &flat.refs[refs_offset + first..refs_offset + last] {
                    let edge_ordinal = edges_offset + edge_ref as usize;
                    let edge = &flat.edges[edge_ordinal * 5..edge_ordinal * 5 + 5];
                    let latitude0 = origin_latitude + f64::from(edge[1]) / M_PER_DEG_LAT;
                    let longitude0 = origin_longitude + f64::from(edge[0]) / index_mlon;
                    let latitude1 = origin_latitude + f64::from(edge[3]) / M_PER_DEG_LAT;
                    let longitude1 = origin_longitude + f64::from(edge[2]) / index_mlon;
                    let endpoint0 = geometry(source_frame_vector_from_world(
                        latitude0,
                        longitude0,
                        receiver_latitude,
                        receiver_longitude,
                        source_mlon,
                    ))?;
                    let endpoint1 = geometry(source_frame_vector_from_world(
                        latitude1,
                        longitude1,
                        receiver_latitude,
                        receiver_longitude,
                        source_mlon,
                    ))?;
                    let candidate = geometry(H0Candidate::from_metric_segment(
                        SourceId64::obstacle(edge_ordinal as u64)
                            .expect("flattened edge entered wall namespace"),
                        endpoint0,
                        endpoint1,
                        edge[4],
                    ))?;
                    raw.push(RawCandidate {
                        record: record_for(source0, source1, candidate),
                        candidate,
                    });
                }
            }
        }
    }

    for barrier in barriers {
        let endpoint0 = geometry(source_frame_vector_from_world(
            barrier.start_lat,
            barrier.start_lon,
            receiver_latitude,
            receiver_longitude,
            source_mlon,
        ))?;
        let endpoint1 = geometry(source_frame_vector_from_world(
            barrier.end_lat,
            barrier.end_lon,
            receiver_latitude,
            receiver_longitude,
            source_mlon,
        ))?;
        let candidate = geometry(H0Candidate::from_metric_segment(
            SourceId64::wall(barrier.osm_id, barrier.segment_idx)
                .expect("barrier provenience outside the GPU ABI"),
            endpoint0,
            endpoint1,
            barrier.height_m,
        ))?;
        raw.push(RawCandidate {
            record: record_for(source0, source1, candidate),
            candidate,
        });
    }
    Ok(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triangle_row_span_handles_vertex_and_empty_slabs() {
        let receiver = MetricVector::new(0.0, 0.0);
        let start = MetricVector::new(-2.0, 2.0);
        let end = MetricVector::new(2.0, 2.0);
        assert_eq!(
            tri_row_x_span(receiver, start, end, 1.0, 2.0),
            Some((-2.0, 2.0))
        );
        assert_eq!(tri_row_x_span(receiver, start, end, 3.0, 4.0), None);
    }
}
