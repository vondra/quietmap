//! Exact ordered-intersection regressions for conservative frame pruning.
use super::*;
use crate::stage_2c::airport_traffic::project_leg_onto_airport_lines;

fn line(id: u64, points: [f32; 4]) -> AirportLineSegment {
    AirportLineSegment {
        osm_id: id,
        segment_idx: 0,
        start_lat: points[0],
        start_lon: points[1],
        end_lat: points[2],
        end_lon: points[3],
        grid: ((0, 0), (1, 1)),
        length_m: 1.0,
        aeroway_type: 1,
    }
}

fn compare(lines: &[AirportLineSegment], legs: &[[f32; 4]]) -> (usize, usize) {
    let mut index = AirportLineIndex::new(lines, u64::MAX).unwrap();
    let mut total = (0, 0);
    for &leg in legs {
        let expected = project_leg_onto_airport_lines(leg[0], leg[1], leg[2], leg[3], lines, 50.0);
        let (actual, counts) = index.project(leg, 50.0);
        assert_eq!(actual, expected, "leg={leg:?}");
        total.0 += counts.0;
        total.1 += counts.1;
    }
    total
}

#[test]
fn wrapped_polar_long_degenerate_and_snap_boundary_hits_preserve_order_and_weights() {
    let mut lines = Vec::new();
    let mut legs = Vec::new();
    for lat in [-90.0f32, -85.05, 0.0, 50.0, 85.05, 90.0] {
        for lon in [-179.999f32, 0.0, 179.999] {
            let end_lon = grid::geo::normalize_longitude(f64::from(lon) + 0.002) as f32;
            lines.push(line(lines.len() as u64, [lat, lon, lat, end_lon]));
            lines.push(line(lines.len() as u64, [lat, lon, lat, end_lon]));
            lines.push(line(lines.len() as u64, [lat, lon, lat, lon]));
            for offset in [
                0.0,
                50.0 / M_PER_DEG_LAT,
                (50.0f32 / M_PER_DEG_LAT).next_up(),
                (50.0f32 / M_PER_DEG_LAT).next_down(),
            ] {
                legs.push([lat + offset, lon, lat + offset, end_lon]);
            }
            legs.push([lat, -179.9, lat, 179.9]);
            legs.push([-89.99, lon, 89.99, end_lon]);
            legs.push([lat, lon, lat, lon]);
        }
    }
    compare(&lines, &legs);
}

#[test]
fn mixed_orientations_dense_intersections_and_distant_lines_match_scalar_bytes() {
    let mut state = 123456789u64;
    let mut random = || {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        (state >> 32) as u32 as f32 / u32::MAX as f32
    };
    let mut lines = Vec::new();
    let mut legs = Vec::new();
    for id in 0..1024 {
        let lat = 50.0 + random() * 0.04;
        let lon = 14.0 + random() * 0.04;
        let points = [
            lat,
            lon,
            lat + (random() - 0.5) * 0.002,
            lon + (random() - 0.5) * 0.002,
        ];
        lines.push(line(id, points));
        legs.push(points);
        legs.push([
            lat,
            lon,
            lat + (random() - 0.5) * 2.0,
            lon + (random() - 0.5) * 2.0,
        ]);
    }
    let (_, candidates) = compare(&lines, &legs);
    assert!(
        candidates < lines.len() * legs.len() / 2,
        "index failed to prune: {candidates}"
    );
}

#[test]
fn allocation_is_refused_before_construction_and_covers_all_vector_capacities() {
    let lines = [line(1, [50.0, 14.0, 50.01, 14.01]); 37];
    let allowance = AirportLineIndex::allocation_allowance(lines.len()).unwrap();
    assert!(AirportLineIndex::new(&lines, allowance - 1).is_err());
    let index = AirportLineIndex::new(&lines, allowance).unwrap();
    let actual = index.frames.capacity() * size_of::<Option<LineFrame>>()
        + index.nodes.capacity() * size_of::<Node>()
        + (index.indices.capacity() + index.candidates.capacity()) * size_of::<usize>();
    assert_eq!(allowance, actual as u64 * 2);
    assert!(AirportLineIndex::allocation_allowance(usize::MAX).is_err());
    compare(&[], &[[0.0; 4]]);
}
